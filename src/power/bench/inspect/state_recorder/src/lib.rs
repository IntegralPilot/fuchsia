// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::FromArgs;
use fidl_fuchsia_power_staterecorder_bench::ControlMarker;
use fuchsia_component::client::connect_to_protocol;

#[derive(FromArgs, Debug)]
/// Command line arguments for the state recorder benchmark
struct Options {
    /// capacity of the recorder history.
    #[argh(option, default = "100")]
    capacity: u64,

    /// whether to use lazy recording.
    #[argh(switch)]
    lazy_record: bool,

    /// number of entries to populate.
    #[argh(option, default = "100")]
    entries: u32,

    /// required by rust test runner
    #[argh(switch)]
    #[allow(unused)]
    nocapture: bool,
}

#[fuchsia::test]
async fn test_state_recorder_memory() {
    let args: Options = argh::from_env::<Options>();

    let control =
        connect_to_protocol::<ControlMarker>().expect("Failed to connect to Control protocol");

    // Trigger the benchmark inside the worker component
    let _ = control.run_benchmark(args.capacity, args.entries, args.lazy_record).await;

    // This will block until the worker component is killed
    std::future::pending::<()>().await;
}
