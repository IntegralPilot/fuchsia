// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::FromArgs;
use byteorder::{LittleEndian, ReadBytesExt};
use std::fs::File;
use std::io::Read;

/// Magic number we write to the disk before the log data. This allows the extractor to
/// differentiate between failures of this harness and a successful run where no output was
/// received, which would otherwise both be all zeros.
///
/// It says logs.
const MAGIC: u64 = 0x1092;

/// Extract stdout and stderr from a block device file and copy them to the provided files.
#[derive(FromArgs)]
struct Args {
    /// path to the file backing the block device with the log files on it.
    #[argh(positional)]
    block_device: String,
    /// path to write stdout to.
    #[argh(positional)]
    stdout: String,
    /// path to write stderr to.
    #[argh(positional)]
    stderr: String,
}

fn main() {
    let Args { block_device, stdout, stderr } = argh::from_env();
    let mut block = File::open(block_device).expect("opening block device file");

    let magic = block.read_u64::<LittleEndian>().expect("reading magic");
    assert_eq!(
        magic, MAGIC,
        "Missing magic - device wasn't written to. This might mean run-with-logs panicked before \
        it could write the output."
    );

    let stdout_len = block.read_u64::<LittleEndian>().expect("reading stdout length");
    let mut stdout_content = vec![0; stdout_len.try_into().expect("u64 to usize conversion")];
    block.read_exact(&mut stdout_content).expect("reading stdout");

    let stderr_len = block.read_u64::<LittleEndian>().expect("reading stderr length");
    let mut stderr_content = vec![0; stderr_len.try_into().expect("u64 to usize conversion")];
    block.read_exact(&mut stderr_content).expect("reading stderr");

    std::fs::write(&stdout, &stdout_content).expect("writing stdout to file");
    std::fs::write(&stderr, &stderr_content).expect("writing stderr to file");
}
