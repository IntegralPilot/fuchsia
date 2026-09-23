// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod block_device;
pub mod directory_benchmarks;
pub mod filesystem;
pub mod io_benchmarks;
pub mod testing;

use async_trait::async_trait;
use fuchsiaperf::{Direction, FuchsiaPerfBenchmarkResult, Unit};
use log::info;
use regex::RegexSet;
use std::io::Write;
use std::time::Instant;

pub use crate::block_device::{BlockDeviceConfig, BlockDeviceFactory};
pub use crate::filesystem::{CacheClearableFilesystem, Filesystem, FilesystemConfig};

/// How long a benchmarked operation took to complete.
#[derive(Debug)]
pub struct OperationDuration(u64);

/// A timer for tracking how long an operation took to complete.
#[derive(Debug)]
pub struct OperationTimer {
    start_time: Instant,
}

impl OperationTimer {
    pub fn start() -> Self {
        let start_time = Instant::now();
        Self { start_time }
    }

    pub fn stop(self) -> OperationDuration {
        OperationDuration(Instant::now().duration_since(self.start_time).as_nanos() as u64)
    }
}

/// A trait representing a single benchmark.
#[async_trait]
pub trait Benchmark<T>: Send + Sync {
    async fn run(&self, fs: &mut T) -> Vec<OperationDuration>;
    fn name(&self) -> String;
}

struct BenchmarkResultsStatistics {
    p50: u64,
    p95: u64,
    p99: u64,
    mean: u64,
    min: u64,
    max: u64,
    count: u64,
}

struct BenchmarkResults {
    benchmark_name: String,
    filesystem_name: String,
    values: Vec<OperationDuration>,
}

fn percentile(data: &[u64], p: u8) -> u64 {
    assert!(p <= 100 && !data.is_empty());
    if data.len() == 1 || p == 0 {
        return *data.first().unwrap();
    }
    if p == 100 {
        return *data.last().unwrap();
    }
    let rank = ((p as f64) / 100.0) * ((data.len() - 1) as f64);
    // The rank is unlikely to be a whole number. Use a linear interpolation between the 2 closest
    // data points.
    let fraction = rank - rank.floor();
    let rank = rank as usize;
    data[rank] + ((data[rank + 1] - data[rank]) as f64 * fraction).round() as u64
}

impl BenchmarkResults {
    fn statistics(&self) -> BenchmarkResultsStatistics {
        let mut data: Vec<u64> = self.values.iter().map(|d| d.0).collect();
        data.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let min = *data.first().unwrap();
        let max = *data.last().unwrap();
        let p50 = percentile(&data, 50);
        let p95 = percentile(&data, 95);
        let p99 = percentile(&data, 99);

        let mut sum = 0;
        for value in &data {
            sum += value;
        }
        let count = data.len() as u64;
        let mean = sum / count;
        BenchmarkResultsStatistics { min, p50, p95, p99, max, mean, count }
    }

    fn pretty_row_header() -> prettytable::Row {
        use prettytable::row;
        row![
            l->"FS",
            l->"Benchmark",
            r->"Min",
            r->"50th Percentile",
            r->"95th Percentile",
            r->"99th Percentile",
            r->"Max",
            r->"Mean",
            r->"Count"
        ]
    }

    fn pretty_row(&self) -> prettytable::Row {
        let stats = self.statistics();
        use prettytable::row;
        row![
            l->self.filesystem_name,
            l->self.benchmark_name,
            r->format_u64_with_commas(stats.min),
            r->format_u64_with_commas(stats.p50),
            r->format_u64_with_commas(stats.p95),
            r->format_u64_with_commas(stats.p99),
            r->format_u64_with_commas(stats.max),
            r->format_u64_with_commas(stats.mean),
            r->format_u64_with_commas(stats.count),
        ]
    }

    fn csv_row_header() -> String {
        "FS,Benchmark,Min,50th Percentile,95th Percentile,99th Percentile,Max,Mean,Count\n"
            .to_owned()
    }

    fn csv_row(&self) -> String {
        let stats = self.statistics();
        format!(
            "{},{},{},{},{},{},{},{},{}\n",
            self.filesystem_name,
            self.benchmark_name,
            stats.min,
            stats.p50,
            stats.p95,
            stats.p99,
            stats.max,
            stats.mean,
            stats.count
        )
    }
}

#[async_trait]
trait BenchmarkConfig {
    async fn run(&self, block_device_factory: &dyn BlockDeviceFactory) -> BenchmarkResults;
    fn name(&self) -> String;
    fn matches(&self, filter: &RegexSet) -> bool;
}

struct BenchmarkConfigImpl<T, U>
where
    T: Benchmark<U::Filesystem>,
    U: FilesystemConfig,
{
    benchmark: T,
    filesystem_config: U,
}

#[async_trait]
impl<T, U> BenchmarkConfig for BenchmarkConfigImpl<T, U>
where
    T: Benchmark<U::Filesystem>,
    U: FilesystemConfig,
{
    async fn run(&self, block_device_factory: &dyn BlockDeviceFactory) -> BenchmarkResults {
        let mut fs = self.filesystem_config.start_filesystem(block_device_factory).await;
        info!("Running {}", self.name());
        let timer = OperationTimer::start();
        let durations = self.benchmark.run(&mut fs).await;
        let benchmark_duration = timer.stop();
        info!("Finished {} {}ns", self.name(), format_u64_with_commas(benchmark_duration.0));
        fs.shutdown().await;

        BenchmarkResults {
            benchmark_name: self.benchmark.name(),
            filesystem_name: self.filesystem_config.name(),
            values: durations,
        }
    }

    fn name(&self) -> String {
        format!("{}/{}", self.benchmark.name(), self.filesystem_config.name())
    }

    fn matches(&self, filter: &RegexSet) -> bool {
        if filter.is_empty() {
            return true;
        }
        filter.is_match(&self.name())
    }
}

/// A collection of benchmarks and the filesystems to run the benchmarks against.
pub struct BenchmarkSet {
    benchmarks: Vec<Box<dyn BenchmarkConfig>>,
}

impl BenchmarkSet {
    pub fn new() -> Self {
        Self { benchmarks: Vec::new() }
    }

    /// Adds a new benchmark with the filesystem it should be run against to the `BenchmarkSet`.
    pub fn add_benchmark<T, U>(&mut self, benchmark: T, filesystem_config: U)
    where
        T: Benchmark<U::Filesystem> + 'static,
        U: FilesystemConfig + 'static,
    {
        self.benchmarks.push(Box::new(BenchmarkConfigImpl { benchmark, filesystem_config }));
    }

    /// Runs all of the added benchmarks against their configured filesystems. The filesystems will
    /// be brought up on block devices created from `block_device_factory`.
    pub async fn run(
        &self,
        block_device_factory: &dyn BlockDeviceFactory,
        filter: &RegexSet,
    ) -> BenchmarkSetResults {
        let mut results = Vec::new();
        for benchmark in &self.benchmarks {
            if benchmark.matches(filter) {
                results.push(benchmark.run(block_device_factory).await);
            }
        }
        BenchmarkSetResults { results }
    }
}

/// All of the benchmark results from `BenchmarkSet::run`.
pub struct BenchmarkSetResults {
    results: Vec<BenchmarkResults>,
}

impl BenchmarkSetResults {
    /// Writes the benchmark results in the fuchsiaperf.json format to `writer`.
    pub fn write_fuchsia_perf_json<F: Write>(&self, writer: F) {
        const TEST_SUITE: &str = "fuchsia.storage";
        let results: Vec<FuchsiaPerfBenchmarkResult> = self
            .results
            .iter()
            .map(|result| FuchsiaPerfBenchmarkResult {
                label: format!("{}/{}", result.benchmark_name, result.filesystem_name),
                test_suite: TEST_SUITE.to_owned(),
                unit: Unit::Nanoseconds,
                direction: Direction::SmallerBetter,
                values: result.values.iter().map(|d| d.0 as f64).collect(),
            })
            .collect();

        serde_json::to_writer_pretty(writer, &results).unwrap()
    }

    /// Writes a summary of the benchmark results as a human readable table to `writer`.
    pub fn write_table<F: Write>(&self, mut writer: F) {
        let mut table = prettytable::Table::new();
        table.set_format(*prettytable::format::consts::FORMAT_CLEAN);
        table.add_row(BenchmarkResults::pretty_row_header());
        for result in &self.results {
            table.add_row(result.pretty_row());
        }
        table.print(&mut writer).unwrap();
    }

    /// Writes a summary of the benchmark results in csv format to `writer`.
    pub fn write_csv<F: Write>(&self, mut writer: F) {
        writer.write_all(BenchmarkResults::csv_row_header().as_bytes()).unwrap();
        for result in &self.results {
            writer.write_all(result.csv_row().as_bytes()).unwrap();
        }
    }
}

fn format_u64_with_commas(num: u64) -> String {
    let mut result = String::new();
    let num = num.to_string();
    let digit_count = num.len();
    for (i, d) in num.char_indices() {
        result.push(d);
        let rpos = digit_count - i;
        if rpos != 1 && rpos % 3 == 1 {
            result.push(',');
        }
    }
    result
}

/// Macro to add many benchmark and filesystem pairs to a `BenchmarkSet`.
///
/// Expands:
/// ```
/// add_benchmarks!(benchmark_set, [benchmark1, benchmark2], [filesystem1, filesystem2]);
/// ```
/// Into:
/// ```
/// benchmark_set.add_benchmark(benchmark1.clone(), filesystem1.clone());
/// benchmark_set.add_benchmark(benchmark1.clone(), filesystem2.clone());
/// benchmark_set.add_benchmark(benchmark2.clone(), filesystem1.clone());
/// benchmark_set.add_benchmark(benchmark2.clone(), filesystem2.clone());
/// ```
#[macro_export]
macro_rules! add_benchmarks {
    ($benchmark_set:ident, [$b:expr], [$f:expr]) => {
        $benchmark_set.add_benchmark($b.clone(), $f.clone());
    };
    ($benchmark_set:ident, [$b:expr, $($bs:expr),+ $(,)?], [$($fs:expr),+ $(,)?]) => {
        add_benchmarks!($benchmark_set, [$b], [$($fs),*]);
        add_benchmarks!($benchmark_set, [$($bs),*], [$($fs),*]);
    };
    ($benchmark_set:ident, [$b:expr], [$f:expr, $($fs:expr),+ $(,)?]) => {
        add_benchmarks!($benchmark_set, [$b], [$f]);
        add_benchmarks!($benchmark_set, [$b], [$($fs),*]);
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_device::PanickingBlockDeviceFactory;

    #[derive(Clone)]
    struct TestBenchmark(&'static str);

    #[async_trait]
    impl<T: Filesystem> Benchmark<T> for TestBenchmark {
        async fn run(&self, _fs: &mut T) -> Vec<OperationDuration> {
            vec![OperationDuration(100), OperationDuration(200), OperationDuration(300)]
        }

        fn name(&self) -> String {
            self.0.to_owned()
        }
    }

    #[derive(Clone)]
    struct TestFilesystem(&'static str);

    #[async_trait]
    impl FilesystemConfig for TestFilesystem {
        type Filesystem = TestFilesystemInstance;
        async fn start_filesystem(
            &self,
            _block_device_factory: &dyn BlockDeviceFactory,
        ) -> Self::Filesystem {
            TestFilesystemInstance
        }

        fn name(&self) -> String {
            self.0.to_string()
        }
    }

    struct TestFilesystemInstance;

    #[async_trait]
    impl Filesystem for TestFilesystemInstance {
        async fn shutdown(self) {}

        fn benchmark_dir(&self) -> &std::path::Path {
            panic!("not supported");
        }
    }

    #[fuchsia::test]
    async fn run_benchmark_set() {
        let mut benchmark_set = BenchmarkSet::new();

        let filesystem1 = TestFilesystem("filesystem1");
        let filesystem2 = TestFilesystem("filesystem2");
        let benchmark1 = TestBenchmark("benchmark1");
        let benchmark2 = TestBenchmark("benchmark2");
        benchmark_set.add_benchmark(benchmark1, filesystem1.clone());
        benchmark_set.add_benchmark(benchmark2.clone(), filesystem1);
        benchmark_set.add_benchmark(benchmark2, filesystem2);

        let block_device_factory = PanickingBlockDeviceFactory::new();
        let results = benchmark_set.run(&block_device_factory, &RegexSet::empty()).await;
        let results = results.results;
        assert_eq!(results.len(), 3);

        assert_eq!(results[0].benchmark_name, "benchmark1");
        assert_eq!(results[0].filesystem_name, "filesystem1");
        assert_eq!(results[0].values.len(), 3);

        assert_eq!(results[1].benchmark_name, "benchmark2");
        assert_eq!(results[1].filesystem_name, "filesystem1");
        assert_eq!(results[1].values.len(), 3);

        assert_eq!(results[2].benchmark_name, "benchmark2");
        assert_eq!(results[2].filesystem_name, "filesystem2");
        assert_eq!(results[2].values.len(), 3);
    }

    #[fuchsia::test]
    fn benchmark_filters() {
        let config = BenchmarkConfigImpl {
            benchmark: TestBenchmark("read_warm"),
            filesystem_config: TestFilesystem("fs-name"),
        };
        // Accepted patterns.
        assert!(config.matches(&RegexSet::empty()));
        assert!(config.matches(&RegexSet::new([r""]).unwrap()));
        assert!(config.matches(&RegexSet::new([r"fs-name"]).unwrap()));
        assert!(config.matches(&RegexSet::new([r"/fs-name"]).unwrap()));
        assert!(config.matches(&RegexSet::new([r"read_warm"]).unwrap()));
        assert!(config.matches(&RegexSet::new([r"read_warm/"]).unwrap()));
        assert!(config.matches(&RegexSet::new([r"read_warm/fs-name"]).unwrap()));
        assert!(config.matches(&RegexSet::new([r"warm"]).unwrap()));

        // Rejected patterns.
        assert!(!config.matches(&RegexSet::new([r"cold"]).unwrap()));
        assert!(!config.matches(&RegexSet::new([r"fxfs"]).unwrap()));
        assert!(!config.matches(&RegexSet::new([r"^fs-name"]).unwrap()));
        assert!(!config.matches(&RegexSet::new([r"read_warm$"]).unwrap()));

        // Matches "warm".
        assert!(config.matches(&RegexSet::new([r"warm", r"cold"]).unwrap()));
        // Matches both.
        assert!(config.matches(&RegexSet::new([r"warm", r"fs-name"]).unwrap()));
        // Matches neither.
        assert!(!config.matches(&RegexSet::new([r"cold", r"fxfs"]).unwrap()));
    }

    #[fuchsia::test]
    fn result_statistics() {
        let benchmark_name = "benchmark-name".to_string();
        let filesystem_name = "filesystem-name".to_string();
        let result = BenchmarkResults {
            benchmark_name: benchmark_name.clone(),
            filesystem_name: filesystem_name.clone(),
            values: vec![
                OperationDuration(600),
                OperationDuration(700),
                OperationDuration(500),
                OperationDuration(800),
                OperationDuration(400),
            ],
        };
        let stats = result.statistics();
        assert_eq!(stats.p50, 600);
        assert_eq!(stats.p95, 780);
        assert_eq!(stats.p99, 796);
        assert_eq!(stats.mean, 600);
        assert_eq!(stats.min, 400);
        assert_eq!(stats.max, 800);
        assert_eq!(stats.count, 5);
    }

    #[fuchsia::test]
    fn percentile_test() {
        let data = [400];
        assert_eq!(percentile(&data, 50), 400);
        assert_eq!(percentile(&data, 95), 400);
        assert_eq!(percentile(&data, 99), 400);

        let data = [400, 500];
        assert_eq!(percentile(&data, 50), 450);
        assert_eq!(percentile(&data, 95), 495);
        assert_eq!(percentile(&data, 99), 499);

        let data = [400, 500, 600];
        assert_eq!(percentile(&data, 50), 500);
        assert_eq!(percentile(&data, 95), 590);
        assert_eq!(percentile(&data, 99), 598);

        let data = [400, 500, 600, 700];
        assert_eq!(percentile(&data, 50), 550);
        assert_eq!(percentile(&data, 95), 685);
        assert_eq!(percentile(&data, 99), 697);

        let data = [400, 500, 600, 700, 800];
        assert_eq!(percentile(&data, 50), 600);
        assert_eq!(percentile(&data, 95), 780);
        assert_eq!(percentile(&data, 99), 796);
    }

    #[fuchsia::test]
    fn format_u64_with_commas_test() {
        assert_eq!(format_u64_with_commas(0), "0");
        assert_eq!(format_u64_with_commas(1), "1");
        assert_eq!(format_u64_with_commas(99), "99");
        assert_eq!(format_u64_with_commas(999), "999");
        assert_eq!(format_u64_with_commas(1_000), "1,000");
        assert_eq!(format_u64_with_commas(999_999), "999,999");
        assert_eq!(format_u64_with_commas(1_000_000), "1,000,000");
        assert_eq!(format_u64_with_commas(999_999_999), "999,999,999");
        assert_eq!(format_u64_with_commas(1_000_000_000), "1,000,000,000");
        assert_eq!(format_u64_with_commas(999_999_999_999), "999,999,999,999");
    }

    #[fuchsia::test]
    fn add_benchmarks_test() {
        let mut benchmark_set = BenchmarkSet::new();

        let filesystem1 = TestFilesystem("filesystem1");
        let filesystem2 = TestFilesystem("filesystem2");
        let benchmark1 = TestBenchmark("benchmark1");
        let benchmark2 = TestBenchmark("benchmark2");
        add_benchmarks!(benchmark_set, [benchmark1], [filesystem1]);
        add_benchmarks!(benchmark_set, [benchmark1], [filesystem1, filesystem2]);
        add_benchmarks!(benchmark_set, [benchmark1, benchmark2], [filesystem1]);
        add_benchmarks!(benchmark_set, [benchmark1, benchmark2], [filesystem1, filesystem2]);
        add_benchmarks!(benchmark_set, [benchmark1, benchmark2,], [filesystem1, filesystem2,]);
        let names: Vec<String> =
            benchmark_set.benchmarks.iter().map(|config| config.name()).collect();
        assert_eq!(
            &names,
            &[
                "benchmark1/filesystem1",
                "benchmark1/filesystem1",
                "benchmark1/filesystem2",
                "benchmark1/filesystem1",
                "benchmark2/filesystem1",
                "benchmark1/filesystem1",
                "benchmark1/filesystem2",
                "benchmark2/filesystem1",
                "benchmark2/filesystem2",
                "benchmark1/filesystem1",
                "benchmark1/filesystem2",
                "benchmark2/filesystem1",
                "benchmark2/filesystem2"
            ]
        );
    }
}
