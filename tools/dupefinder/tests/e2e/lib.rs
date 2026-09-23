// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use dupefinder::DupesReport;
use ffx_e2e_emu::IsolatedEmulator;
use log::{info, warn};
use std::path::PathBuf;
use std::process::Command;

#[fuchsia::test(logging_minimum_severity = "TRACE")]
async fn dupefinder_smoke_test() {
    let test_dir = PathBuf::from(std::env::var("FUCHSIA_TEST_OUTDIR").unwrap());
    let dupefinder = PathBuf::from(std::env::var("DUPEFINDER_PATH").unwrap());
    let expected_filename = std::env::var("EXPECTED_FILENAME").unwrap();

    info!(test_dir:?; "starting emulator and enabling heapdump plugin...");
    let emu = IsolatedEmulator::start("test-dupefinder").await.unwrap();
    emu.ffx(&["config", "set", "ffx_profile_heapdump", "true"]).await.unwrap();

    let moniker = "/core/ffx-laboratory:duplicate_allocations";
    let url = "fuchsia-pkg://fuchsia.com/duplicate_allocations#meta/duplicate_allocations.cm";
    info!(moniker:%, url:%; "starting duplicate allocator...");
    emu.ffx(&["component", "run", moniker, url]).await.unwrap();

    info!("waiting for puppet to mark itself as waiting...");
    'wait_for_puppet: loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
        if let Ok(contents) =
            emu.ffx_output(&["component", "explore", moniker, "-c", "cat /ns/tmp/waiting"]).await
        {
            info!(contents:%; "puppet reported it's now paused");
            break 'wait_for_puppet;
        }
    }

    info!("Taking a live snapshot with contents...");
    let (profile_path, contents_dir) = loop {
        let profile_path = test_dir.join("snapshot.pb.gz");
        let contents_dir = test_dir.join("heap_contents_dir");
        match emu
            .ffx(&[
                "profile",
                "heapdump",
                "snapshot",
                "--output-file",
                profile_path.to_str().unwrap(),
                "--output-contents-dir",
                contents_dir.to_str().unwrap(),
                "--by-name",
                "duplicate_allocations.cm",
                "--symbolize",
            ])
            .await
        {
            Ok(_) => break (profile_path, contents_dir),
            Err(e) => {
                warn!(e:?; "failed to collect snapshot, retrying...");
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }
    };

    info!("finding duplicate allocations on the command-line...");
    let json_output_path = test_dir.join("dupes.json");
    let dupefinder_res = Command::new(&dupefinder)
        .arg("--profile")
        .arg(&profile_path)
        .arg("--contents-dir")
        .arg(&contents_dir)
        .arg("--json-output")
        .arg(&json_output_path)
        .arg("--max-allocs-reported")
        .arg("1") // only show the explicitly to-be-blamed alloc
        .arg("--verbose")
        .status()
        .unwrap();
    assert!(dupefinder_res.success());

    let report_json = std::fs::read_to_string(&json_output_path).unwrap();
    let report: DupesReport = serde_json::from_str(&report_json).unwrap();

    let first_dupe = &report.dupe_locations()[0];
    assert_eq!(first_dupe.source().filename(), expected_filename);
    assert_eq!(first_dupe.source().function(), "duplicate_allocations_bin::main()");
    assert_eq!(first_dupe.bytes(), 13000, "hello world is 13 bytes, puppet allocs 1000");
}
