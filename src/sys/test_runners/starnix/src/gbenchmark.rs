// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::helpers::*;
use anyhow::{Context as _, Error};
use fidl_fuchsia_component_runner as frunner;
use fidl_fuchsia_test::{self as ftest};
use fuchsiaperf::{Direction, FuchsiaPerfBenchmarkResult};
use heck::CamelCase;
use serde::{Deserialize, Serialize};

/// The results returned by gBenchmark.
#[derive(Serialize, Deserialize, Debug, Default)]
pub struct BenchmarkOutput {
    pub context: BenchmarkContextData,
    pub benchmarks: Vec<BenchmarkRunData>,
}

/// The context returned by gBenchmark.
///
/// This struct is empty because its fields are not used in Fuchsiaperf results.
#[derive(Serialize, Deserialize, Debug, Default)]
pub struct BenchmarkContextData {}

/// The benchmark results returned by gBenchmark.
///
/// More fields are reported but these are the ones needed for fuchsiaperf.
#[derive(Serialize, Deserialize, Debug)]
pub struct BenchmarkRunData {
    pub name: String,
    pub iterations: u64,
    pub real_time: f64,
    pub time_unit: String,
}

const GBENCHMARK_RESULT_FILE: &str = "benchmark.json";

/// Runs a gbenchmark associated with a single `ftest::SuiteRequest::Run` request.
///
/// Reports results in the test component's `/custom_artifacts` to leverage
/// the test framework's exporting of custom files.
///
/// # Parameters
/// - `test`: The test invocation to run.
/// - `start_info`: The component start info of the test to run.
/// - `run_listener_proxy`: The proxy used to communicate results of the test run to the test
///                         framework.
/// - `component_runner`: The runner that will run the test component.
pub async fn run_gbenchmark(
    test: ftest::Invocation,
    start_info: frunner::ComponentStartInfo,
    run_listener_proxy: &ftest::RunListenerProxy,
    component_runner: &frunner::ComponentRunnerProxy,
) -> Result<(), Error> {
    run_starnix_benchmark(
        test,
        start_info,
        run_listener_proxy,
        component_runner,
        GBENCHMARK_RESULT_FILE,
        gbenchmark_to_fuchsiaperf,
    )
    .await
}

fn gbenchmark_to_fuchsiaperf(
    results: &str,
    test_suite: &str,
) -> Result<Vec<FuchsiaPerfBenchmarkResult>, Error> {
    let benchmark_output: BenchmarkOutput = if results.is_empty() {
        BenchmarkOutput::default()
    } else {
        serde_json::from_str(results).context("Failed to parse benchmark results.")?
    };

    let mut perfs = vec![];
    for benchmark in benchmark_output.benchmarks {
        // Conform benchmark names to Fuchsia performance metric naming style.
        // https://fuchsia.dev/fuchsia-src/development/performance/metric_naming_style
        //
        // gVisor benchmark names look like:
        // BM_GetdentsSameFD/8/real_time or BM_Sched_yield/real_time/threads:32.
        // They should end up like: GetdentsSameFd/8 or SchedYield/Threads/32.
        let label_trimmed =
            benchmark.name.trim_start_matches("BM_").replace("/real_time", "").replace(":", "/");
        let mut segments = vec![];
        for segment in label_trimmed.split("/") {
            segments.push(segment.to_camel_case());
        }
        let label = segments.join("/");

        perfs.push(FuchsiaPerfBenchmarkResult {
            label,
            test_suite: test_suite.to_owned(),
            unit: benchmark.time_unit.parse().map_err(Error::msg)?,
            direction: Direction::SmallerBetter,
            values: vec![benchmark.real_time],
        });
    }

    Ok(perfs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsiaperf::Unit;

    const TEST_SUITE: &str = "test_suite";

    #[test]
    fn test_gbenchmark_to_fuchsiaperf() {
        let gbenchmark = r#"{
            "context": {
              "date": "2023-02-08T18:07:17+00:00",
              "host_name": "local",
              "num_cpus": -1,
              "mhz_per_cpu": 0,
              "caches": [
              ],
              "load_avg": [],
              "library_build_type": "debug"
            },
            "benchmarks": [
              {
                "name": "BM_MetricName1/category/1",
                "family_index": 0,
                "per_family_instance_index": 0,
                "run_name": "RunName1",
                "run_type": "iteration",
                "repetitions": 1,
                "repetition_index": 0,
                "threads": 1,
                "iterations": 31798,
                "real_time": 2.1959886533832538e+04,
                "cpu_time": 2.1957717089125101e+04,
                "time_unit": "ns"
              },
              {
                "name": "BM_MetricName1/category/2",
                "family_index": 0,
                "per_family_instance_index": 1,
                "run_name": "RunName2",
                "run_type": "iteration",
                "repetitions": 1,
                "repetition_index": 0,
                "threads": 1,
                "iterations": 4648,
                "real_time": 1.2770589649314955e+06,
                "cpu_time": 1.5376424806368331e+05,
                "time_unit": "ns"
              },
              {
                "name": "BM_MetricName1/category/64/real_time",
                "family_index": 1,
                "per_family_instance_index": 0,
                "run_name": "RunName3",
                "run_type": "iteration",
                "repetitions": 1,
                "repetition_index": 0,
                "threads": 1,
                "iterations": 25687,
                "real_time": 3.1623006774103047e+04,
                "cpu_time": 3.1623803324638917e+04,
                "time_unit": "ns"
              },
              {
                "name": "BM_MetricName1/category/128/real_time",
                "family_index": 0,
                "per_family_instance_index": 2,
                "run_name": "RunName4",
                "run_type": "iteration",
                "repetitions": 1,
                "repetition_index": 0,
                "threads": 1,
                "iterations": 1000,
                "real_time": 8.2976487720006844e+06,
                "cpu_time": 1.2003764500000003e+05,
                "time_unit": "ns"
              },
              {
                "name": "BM_MetricName1/category/512/real_time",
                "family_index": 1,
                "per_family_instance_index": 1,
                "run_name": "RunName5",
                "run_type": "iteration",
                "repetitions": 1,
                "repetition_index": 0,
                "threads": 1,
                "iterations": 10000,
                "real_time": 5.4292060699663125e+04,
                "cpu_time": 5.4171954499999985e+04,
                "time_unit": "ns"
              },
              {
                "name": "BM_MetricName1/category/1024/real_time",
                "family_index": 1,
                "per_family_instance_index": 1,
                "run_name": "RunName6",
                "run_type": "iteration",
                "repetitions": 1,
                "repetition_index": 0,
                "threads": 1,
                "iterations": 10000,
                "real_time": 3.7122060699663125e+04,
                "cpu_time": 3.3291954499999985e+04,
                "time_unit": "ns"
              },
              {
                "name": "BM_MetricName2/category:1/real_time",
                "family_index": 1,
                "per_family_instance_index": 1,
                "run_name": "RunName7",
                "run_type": "iteration",
                "repetitions": 1,
                "repetition_index": 0,
                "threads": 1,
                "iterations": 10000,
                "real_time": 6.4292060699663125e+04,
                "cpu_time": 6.4171954499999985e+04,
                "time_unit": "ns"
              }
            ]
          }"#;

        let expected_perfs = vec![
            FuchsiaPerfBenchmarkResult {
                label: "MetricName1/Category/1".to_string(),
                test_suite: TEST_SUITE.to_owned(),
                unit: Unit::Nanoseconds,
                direction: Direction::SmallerBetter,
                values: vec![21959.886533832538],
            },
            FuchsiaPerfBenchmarkResult {
                label: "MetricName1/Category/2".to_string(),
                test_suite: TEST_SUITE.to_owned(),
                unit: Unit::Nanoseconds,
                direction: Direction::SmallerBetter,
                values: vec![1277058.9649314955],
            },
            FuchsiaPerfBenchmarkResult {
                label: "MetricName1/Category/64".to_string(),
                test_suite: TEST_SUITE.to_owned(),
                unit: Unit::Nanoseconds,
                direction: Direction::SmallerBetter,
                values: vec![31623.006774103047],
            },
            FuchsiaPerfBenchmarkResult {
                label: "MetricName1/Category/128".to_string(),
                test_suite: TEST_SUITE.to_owned(),
                unit: Unit::Nanoseconds,
                direction: Direction::SmallerBetter,
                values: vec![8297648.7720006844],
            },
            FuchsiaPerfBenchmarkResult {
                label: "MetricName1/Category/512".to_string(),
                test_suite: TEST_SUITE.to_owned(),
                unit: Unit::Nanoseconds,
                direction: Direction::SmallerBetter,
                values: vec![54292.060699663125],
            },
            FuchsiaPerfBenchmarkResult {
                label: "MetricName1/Category/1024".to_string(),
                test_suite: TEST_SUITE.to_owned(),
                unit: Unit::Nanoseconds,
                direction: Direction::SmallerBetter,
                values: vec![37122.060699663125],
            },
            FuchsiaPerfBenchmarkResult {
                label: "MetricName2/Category/1".to_string(),
                test_suite: TEST_SUITE.to_owned(),
                unit: Unit::Nanoseconds,
                direction: Direction::SmallerBetter,
                values: vec![64292.060699663125],
            },
        ];

        let perfs = gbenchmark_to_fuchsiaperf(gbenchmark, TEST_SUITE)
            .expect("Failed to convert gbenchmark results.");
        assert_eq!(perfs.len(), expected_perfs.len());
        for i in 0..perfs.len() {
            assert_eq!(perfs[i], expected_perfs[i]);
        }
    }

    #[test]
    fn test_gbenchmark_to_fuchsiaperf_no_name() {
        let gbenchmark = r#"{
            "context": {
            },
            "benchmarks": [
              {
                "real_time": 2.1959886533832538e+04,
                "time_unit": "ns"
              },
            ]
          }"#;
        let _ = gbenchmark_to_fuchsiaperf(gbenchmark, TEST_SUITE)
            .expect_err("Failed to parse benchmark results.");
    }

    #[test]
    fn test_gbenchmark_to_fuchsiaperf_no_time() {
        let gbenchmark = r#"{
            "context": {
            },
            "benchmarks": [
              {
                "name": "BM_MetricName1",
                "time_unit": "ns"
              },
            ]
          }"#;

        let _ = gbenchmark_to_fuchsiaperf(gbenchmark, TEST_SUITE)
            .expect_err("Failed to parse benchmark results.");
    }

    #[test]
    fn test_gbenchmark_to_fuchsiaperf_no_time_unit() {
        let gbenchmark = r#"{
            "context": {
            },
            "benchmarks": [
              {
                "name": "BM_MetricName1",
                "real_time": 2.1959886533832538e+04,
              },
            ]
          }"#;

        let _ = gbenchmark_to_fuchsiaperf(gbenchmark, TEST_SUITE)
            .expect_err("Failed to parse benchmark results.");
    }
}
