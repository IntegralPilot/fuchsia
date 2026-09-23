// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use regex::{Regex, RegexSetBuilder};
use std::fs::File;
use std::path::PathBuf;
use storage_benchmarks::block_device::PanickingBlockDeviceFactory;
use storage_benchmarks::directory_benchmarks::{
    CreateFile, DirectoryTreeStructure, GitStatus, OpenDeeplyNestedFile, OpenFile, StatPath,
    UnlinkFile, WalkDirectoryTreeWarm,
};
use storage_benchmarks::filesystem::MountedFilesystem;
use storage_benchmarks::io_benchmarks::{
    ReadRandomWarm, ReadSequentialWarm, WriteRandomCold, WriteRandomWarm, WriteSequentialCold,
    WriteSequentialWarm,
};
use storage_benchmarks::{BenchmarkSet, add_benchmarks};

/// Fuchsia Filesystem Benchmarks
#[derive(argh::FromArgs)]
struct Args {
    /// path to write the fuchsiaperf formatted benchmark results to.
    #[argh(option)]
    output_fuchsiaperf: Option<PathBuf>,

    /// outputs a summary of the benchmark results in csv format.
    #[argh(switch)]
    output_csv: bool,

    /// regex to specify a subset of benchmarks to run. Multiple regex can be provided and
    /// benchmarks matching any of them will be run. The benchmark names are formatted as
    /// "<benchmark>/<filesystem>". All benchmarks are run if no filter is provided.
    #[argh(option)]
    filter: Vec<Regex>,

    /// directory to run the benchmarks out of.
    ///
    /// Warning: the contents of the directory may be deleted by the benchmarks.
    #[argh(option)]
    benchmark_dir: PathBuf,

    /// name of the filesystem, used for outputting results.
    #[argh(option, default = "String::from(\"host\")")]
    filesystem_name: String,
}

fn build_benchmark_set(fs: MountedFilesystem) -> BenchmarkSet {
    const OP_SIZE: usize = 8 * 1024;
    const OP_COUNT: usize = 1024;

    let mut benchmark_set = BenchmarkSet::new();
    add_benchmarks!(
        benchmark_set,
        [
            ReadSequentialWarm::new(OP_SIZE, OP_COUNT),
            ReadRandomWarm::new(OP_SIZE, OP_COUNT),
            WriteSequentialWarm::new(OP_SIZE, OP_COUNT),
            WriteRandomWarm::new(OP_SIZE, OP_COUNT),
            WriteSequentialCold::new(OP_SIZE, OP_COUNT),
            WriteRandomCold::new(OP_SIZE, OP_COUNT),
            StatPath::new(),
            CreateFile::new(),
            UnlinkFile::new(),
            OpenFile::new(),
            OpenDeeplyNestedFile::new(),
            GitStatus::new(),
        ],
        [fs]
    );

    // Creates a total of 62 directories and 189 files.
    let dts = DirectoryTreeStructure {
        files_per_directory: 3,
        directories_per_directory: 2,
        max_depth: 5,
    };
    benchmark_set.add_benchmark(WalkDirectoryTreeWarm::new(dts, 20), fs);

    benchmark_set
}

#[fuchsia::main]
async fn main() {
    let args: Args = argh::from_env();

    std::fs::create_dir_all(&args.benchmark_dir).unwrap_or_else(|e| {
        panic!("Failed to create directory '{}': {:?}", args.benchmark_dir.display(), e)
    });

    let mut filter = RegexSetBuilder::new(args.filter.iter().map(|f| f.as_str()));
    filter.case_insensitive(true);
    let filter = filter.build().unwrap();

    let block_device_factory = PanickingBlockDeviceFactory::new();
    let benchmark_suite =
        build_benchmark_set(MountedFilesystem::new(args.benchmark_dir, args.filesystem_name));
    let results = benchmark_suite.run(&block_device_factory, &filter).await;

    results.write_table(std::io::stdout());
    if args.output_csv {
        results.write_csv(std::io::stdout())
    }
    if let Some(path) = args.output_fuchsiaperf {
        let file = File::create(path).unwrap();
        results.write_fuchsia_perf_json(file);
    }
}
