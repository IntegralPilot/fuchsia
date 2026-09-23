// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod blob_actor;
mod deletion_actor;
mod environment;
mod instance_actor;
mod read_actor;

use crate::environment::BlobfsEnvironment;
use argh::FromArgs;
use diagnostics_log::Severity;
use stress_test::run_test;

#[derive(Clone, Debug, FromArgs)]
/// Creates an instance of fvm and performs stressful operations on it
pub struct Args {
    /// seed to use for this stressor instance
    #[argh(option, short = 's')]
    seed: Option<u64>,

    /// number of operations to complete before exiting.
    #[argh(option, short = 'o')]
    num_operations: Option<u64>,

    /// filter logging by level (off, error, warn, info, debug, trace)
    #[argh(option, short = 'l')]
    log_filter: Option<Severity>,

    /// size of one block of the ramdisk (in bytes)
    #[argh(option, default = "512")]
    ramdisk_block_size: u64,

    /// number of blocks in the ramdisk
    /// defaults to 106MiB ramdisk
    #[argh(option, default = "217088")]
    ramdisk_block_count: u64,

    /// size of one slice in FVM (in bytes)
    #[argh(option, default = "32768")]
    fvm_slice_size: u64,

    /// controls how often blobfs is killed and the ramdisk is unbound
    #[argh(option, short = 'd')]
    disconnect_secs: Option<u64>,

    /// if set, the test runs for this time limit before exiting successfully.
    #[argh(option, short = 't')]
    time_limit_secs: Option<u64>,

    /// parameter passed in by rust test runner
    #[argh(switch)]
    // TODO(https://fxbug.dev/42165549)
    #[allow(unused)]
    nocapture: bool,
}

// The path to the blobfs filesystem in the test's namespace
pub const BLOBFS_MOUNT_PATH: &str = "/blobfs";

#[fuchsia::test(logging = false)]
async fn test() {
    // Get arguments from command line
    let args: Args = argh::from_env();

    // Initialize logging
    let mut options = diagnostics_log::PublishOptions::default();
    if let Some(filter) = args.log_filter {
        options = options.minimum_severity(filter);
    }
    diagnostics_log::initialize(options).unwrap();

    // Setup the blobfs environment
    let env = BlobfsEnvironment::new(args).await;

    // Run the test
    run_test(env).await;
}
