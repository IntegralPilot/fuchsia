// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::blob_benchmarks::{
    ColdPageFaultBenchmark, OpenAndGetVmoBenchmark, PageInBlobBenchmark, WriteBlob,
    WriteRealisticBlobs,
};
use fidl_fuchsia_power_cpu_manager as fcpumanager;
use fuchsia_component::client::connect_to_protocol;
use fuchsia_storage_benchmarks::block_devices::BenchmarkVolumeFactory;
use fuchsia_storage_benchmarks::filesystems::{
    Blobfs, F2fs, Fxblob, Fxfs, Memfs, Minfs, PkgDirTest,
};
use regex::{Regex, RegexSetBuilder};
use std::fs::File;
use std::path::PathBuf;
use storage_benchmarks::directory_benchmarks::{
    CreateFile, DirectoryTreeStructure, GitStatus, OpenDeeplyNestedFile, OpenFile, StatPath,
    UnlinkFile, WalkDirectoryTreeCold, WalkDirectoryTreeWarm,
};
use storage_benchmarks::io_benchmarks::{
    ReadRandomCold, ReadRandomWarm, ReadSequentialCold, ReadSequentialWarm, ReadSparseCold,
    WriteRandomCold, WriteRandomFsyncCold, WriteRandomFsyncWarm, WriteRandomWarm,
    WriteSequentialCold, WriteSequentialFsyncCold, WriteSequentialFsyncWarm, WriteSequentialWarm,
};
use storage_benchmarks::{BenchmarkSet, add_benchmarks};

mod blob_benchmarks;
mod blob_loader;

const FXFS_VOLUME_SIZE: u64 = 48 * 1024 * 1024;

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

    /// registers a trace provider and adds a trace duration with the benchmarks name around each
    /// benchmark.
    #[argh(switch)]
    enable_tracing: bool,

    /// pages in all of the blobs in the package and exits. Does not run any benchmarks.
    ///
    /// When trying to collect a trace immediately after modifying a filesystem or a benchmark, the
    /// start of the trace will be polluted with downloading the new blobs, writing the blobs, and
    /// then paging the blobs back in. Running the benchmarks with this flag once before running
    /// them again with tracing enabled will remove most of the blob loading from the start of the
    /// trace.
    #[argh(switch)]
    load_blobs_for_tracing: bool,
}

fn add_io_benchmarks(benchmark_set: &mut BenchmarkSet) {
    const OP_SIZE: usize = 8 * 1024;
    const OP_COUNT: usize = 1024;
    add_benchmarks!(
        benchmark_set,
        [
            ReadSequentialWarm::new(OP_SIZE, OP_COUNT),
            ReadRandomWarm::new(OP_SIZE, OP_COUNT),
            WriteSequentialCold::new(OP_SIZE, OP_COUNT),
            WriteSequentialWarm::new(OP_SIZE, OP_COUNT),
            WriteRandomCold::new(OP_SIZE, OP_COUNT),
            WriteRandomWarm::new(OP_SIZE, OP_COUNT),
            WriteSequentialFsyncCold::new(OP_SIZE, OP_COUNT),
            WriteSequentialFsyncWarm::new(OP_SIZE, OP_COUNT),
        ],
        [Fxfs::new(FXFS_VOLUME_SIZE), F2fs, Memfs, Minfs]
    );
    add_benchmarks!(
        benchmark_set,
        [
            ReadSequentialCold::new(OP_SIZE, OP_COUNT),
            ReadRandomCold::new(OP_SIZE, OP_COUNT),
            ReadSparseCold::new(OP_SIZE, OP_COUNT),
        ],
        [Fxfs::new(FXFS_VOLUME_SIZE), F2fs, Minfs]
    );
    // Minfs is excluded from the random-write fsync benchmark to reduce test time,
    // as performance was never a primary design goal.
    add_benchmarks!(
        benchmark_set,
        [
            WriteRandomFsyncCold::new(OP_SIZE, OP_COUNT),
            WriteRandomFsyncWarm::new(OP_SIZE, OP_COUNT)
        ],
        [Fxfs::new(FXFS_VOLUME_SIZE), F2fs]
    );
}

fn add_directory_benchmarks(benchmark_set: &mut BenchmarkSet) {
    // Creates a total of 62 directories and 189 files.
    let dts = DirectoryTreeStructure {
        files_per_directory: 3,
        directories_per_directory: 2,
        max_depth: 5,
    };
    add_benchmarks!(
        benchmark_set,
        [
            StatPath::new(),
            CreateFile::new(),
            UnlinkFile::new(),
            OpenFile::new(),
            OpenDeeplyNestedFile::new(),
            WalkDirectoryTreeWarm::new(dts, 20),
            GitStatus::new(),
        ],
        [Fxfs::new(FXFS_VOLUME_SIZE), F2fs, Memfs, Minfs]
    );
    add_benchmarks!(
        benchmark_set,
        [WalkDirectoryTreeCold::new(dts, 20)],
        [Fxfs::new(FXFS_VOLUME_SIZE), F2fs, Minfs]
    );
}

fn add_blob_benchmarks(benchmark_set: &mut BenchmarkSet) {
    const SMALL_BLOB_SIZE: usize = 2 * 1024 * 1024; // 2 MiB
    const LARGE_BLOB_SIZE: usize = 25 * 1024 * 1024; // 25 MiB
    add_benchmarks!(
        benchmark_set,
        [
            PageInBlobBenchmark::new_sequential_uncompressed(SMALL_BLOB_SIZE),
            PageInBlobBenchmark::new_sequential_compressed(SMALL_BLOB_SIZE),
            PageInBlobBenchmark::new_random_compressed(SMALL_BLOB_SIZE),
            WriteBlob::new(SMALL_BLOB_SIZE),
            WriteBlob::new(LARGE_BLOB_SIZE),
            WriteRealisticBlobs::new(),
        ],
        [Blobfs, Fxblob]
    );
    add_benchmarks!(
        benchmark_set,
        [
            ColdPageFaultBenchmark::new_lz4_40(),
            ColdPageFaultBenchmark::new_lz4_55(),
            ColdPageFaultBenchmark::new_lz4_70(),
            ColdPageFaultBenchmark::new_zstd_40(),
            ColdPageFaultBenchmark::new_zstd_55(),
            ColdPageFaultBenchmark::new_zstd_70(),
            ColdPageFaultBenchmark::new_uncompressed(1024),
            ColdPageFaultBenchmark::new_uncompressed(4096),
            ColdPageFaultBenchmark::new_uncompressed(131072),
        ],
        [Fxblob]
    );
    add_benchmarks!(
        benchmark_set,
        [
            OpenAndGetVmoBenchmark::new_content_blob_cold(),
            OpenAndGetVmoBenchmark::new_content_blob_warm(),
            OpenAndGetVmoBenchmark::new_meta_file_cold(),
            OpenAndGetVmoBenchmark::new_meta_file_warm(),
        ],
        [PkgDirTest::new_fxblob(), PkgDirTest::new_blobfs()]
    );
}

#[fuchsia::main(logging_tags = ["storage_benchmarks"])]
async fn main() {
    let args: Args = argh::from_env();
    let config = fuchsia_storage_benchmarks_config::Config::take_from_startup_handle();

    let booster = connect_to_protocol::<fcpumanager::BoostMarker>();
    let _boost_token = match booster {
        Ok(proxy) => match proxy.boost().await {
            Ok(Ok(token)) => {
                log::info!("CPU performance boost active for benchmarks.");
                Some(token)
            }
            Ok(Err(e)) => {
                log::warn!("CPU boost protocol returned error: {:?}", e);
                None
            }
            Err(e) => {
                log::warn!("Failed to call CPU boost: {:?}", e);
                None
            }
        },
        Err(e) => {
            log::warn!("CPU boost protocol not available: {:?}", e);
            None
        }
    };

    let _loaded_blobs = blob_loader::BlobLoader::load_blobs().await;
    if args.load_blobs_for_tracing {
        return;
    }

    if args.enable_tracing {
        fuchsia_trace_provider::trace_provider_create_with_fdio();
    }

    let mut filter = RegexSetBuilder::new(args.filter.iter().map(|f| f.as_str()));
    filter.case_insensitive(true);
    let filter = filter.build().unwrap();

    let fvm_instance = BenchmarkVolumeFactory::from_config(config.fxfs_blob).await;
    let mut benchmark_set = BenchmarkSet::new();
    add_io_benchmarks(&mut benchmark_set);
    add_directory_benchmarks(&mut benchmark_set);
    add_blob_benchmarks(&mut benchmark_set);
    let results = benchmark_set.run(&fvm_instance, &filter).await;

    results.write_table(std::io::stdout());
    if args.output_csv {
        results.write_csv(std::io::stdout())
    }
    if let Some(path) = args.output_fuchsiaperf {
        let file = File::create(path).unwrap();
        results.write_fuchsia_perf_json(file);
    }
}
