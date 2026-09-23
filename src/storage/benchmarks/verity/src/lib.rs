// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsiaperf::{Direction, FuchsiaPerfBenchmarkResult, Unit};
use linux_uapi::{FS_IOC_ENABLE_VERITY, FS_VERITY_HASH_ALG_SHA256, fsverity_enable_arg};
use std::os::fd::AsRawFd;
use std::time::Instant;

pub const VERITY_FILE_SIZES: [u64; 4] = [4096, 24576, 262144, 2097152];
pub const SAMPLES: u64 = 5;
pub const ENABLE_BENCHMARK_NAME: &str = "Enable";
pub const READ_BENCHMARK_NAME: &str = "Read";

fn file_path(file_size: u64, iteration: u64) -> String {
    // .. required for the Starnix component runner
    format!("../data/verity_{file_size}_{iteration}")
}

pub fn results_file_name(benchmark_name: &str) -> String {
    format!("{benchmark_name}_results.json")
}

pub fn run_benchmark(benchmark: fn(u64, u64) -> u64, benchmark_name: &str) {
    let mut results = vec![];
    for size in VERITY_FILE_SIZES {
        let mut values = vec![];
        for count in 0..SAMPLES {
            values.push(benchmark(size, count) as f64);
        }
        results.push(FuchsiaPerfBenchmarkResult {
            label: format!("{benchmark_name}/{size}"),
            test_suite: "fuchsia.verity".to_string(),
            unit: Unit::Nanoseconds,
            direction: Direction::SmallerBetter,
            values: values,
        });
    }
    let results_file_path = format!("../data/{}", results_file_name(&benchmark_name));
    let tmp_file_path = format!("{results_file_path}-tmp");
    std::fs::write(&tmp_file_path, serde_json::to_string_pretty(&results).unwrap())
        .expect("write json failed");
    std::fs::rename(tmp_file_path, results_file_path).unwrap();
}

pub fn enable_verity_benchmark(file_size: u64, iteration: u64) -> u64 {
    let verity_path = file_path(file_size, iteration);
    std::fs::write(&verity_path, vec![1; file_size as usize]).expect("write failed");
    let file = std::fs::File::open(verity_path).expect("open failed");
    let args = fsverity_enable_arg {
        version: 1,
        hash_algorithm: FS_VERITY_HASH_ALG_SHA256,
        block_size: 4096,
        ..Default::default()
    };
    let start_time = Instant::now();
    let ret =
        unsafe { libc::ioctl(file.as_raw_fd(), FS_IOC_ENABLE_VERITY.try_into().unwrap(), &args) };
    let duration = start_time.elapsed().as_nanos() as u64;
    assert!(ret == 0, "enable verity ioctl failed: {:?}", std::io::Error::last_os_error());
    duration
}

pub fn read_verity_benchmark(file_size: u64, iteration: u64) -> u64 {
    let start_time = Instant::now();
    let contents = std::fs::read(file_path(file_size, iteration)).unwrap();
    let duration = start_time.elapsed().as_nanos() as u64;
    assert_eq!(contents.len(), file_size as usize);
    assert!(contents.iter().all(|x| *x == 1));
    duration
}
