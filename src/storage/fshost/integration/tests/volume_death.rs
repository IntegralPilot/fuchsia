// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests for fshost's shutdown_on_crash behavior when core storage volumes die.
//! This requires a dedicated test package configuring max_severity = "ERROR" since crash filing
//! logs errors.

pub mod config;
use config::{data_fs_spec, data_fs_type, new_builder, volumes_spec};

use assert_matches::assert_matches;
use fidl::endpoints::Proxy;
use fidl_fuchsia_io as fio;
use fuchsia_async;
use futures::StreamExt as _;

#[fuchsia::test]
async fn test_fshost_reboots_on_volume_death() {
    let mut builder = new_builder();
    builder.with_disk().format_volumes(volumes_spec()).format_data(data_fs_spec());
    let mut fixture = builder.build().await;

    // Ensure storage is correctly active
    fixture.check_fs_type("data", data_fs_type()).await;

    // Open file and write data BEFORE dropping the block device (forces initial I/O setup)
    let data_dir = fixture.dir("data", fio::PERM_READABLE | fio::PERM_WRITABLE);
    let file = fuchsia_fs::directory::open_file(
        &data_dir,
        "crash-trigger",
        fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_READABLE | fio::PERM_WRITABLE,
    )
    .await
    .expect("Failed to open file while healthy");

    fuchsia_fs::file::write(&file, b"crash please")
        .await
        .expect("Failed to write data while healthy");

    // Sever block connection by dropping RamdiskClient
    let ramdisk = fixture.ramdisks.remove(0);
    drop(ramdisk); // De-publishes block device in DriverTestRealm

    for i in 0..20 {
        // Attempt a metadata change AFTER the block device is gone
        // For Minfs, this might fail immediately due to read-only state, which is fine to ignore.
        let _ = fuchsia_fs::directory::open_file(
            &data_dir,
            &format!("crash-trigger-after-{}", i),
            fio::Flags::FLAG_MUST_CREATE | fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .await;

        let _ = data_dir.sync().await;

        if data_dir.is_closed() {
            break;
        }

        fuchsia_async::Timer::new(std::time::Duration::from_secs(1)).await;
    }
    assert!(data_dir.is_closed(), "Waiting for filesystem to die timed out");

    // Verify Crash Report was filed cleanly
    let report = fixture.crash_reports.next().await.expect("Expected a crash report");
    assert_eq!(report.program_name.as_deref(), Some("fshost"));

    let signature = report.crash_signature.as_deref().expect("Expected crash signature");
    assert!(
        signature == "data-volume-crash" || signature == "blob-volume-crash",
        "Unexpected signature: {}",
        signature
    );
    assert_eq!(report.is_fatal, Some(true));

    // Assert fshost process termination and verify it exited with a non-zero error rather than
    // a graceful exit.
    let mut event_stream = fixture.execution_controller.take_event_stream();
    let event = event_stream
        .next()
        .await
        .expect("Expected OnStop event")
        .expect("FIDL error waiting for OnStop");
    match event {
        fidl_fuchsia_component::ExecutionControllerEvent::OnStop { stopped_payload } => {
            // A normal/clean exit has status Some(0) and exit_code Some(0).
            // A non-zero exit status / error has non-zero status / exit code.
            assert_ne!(
                stopped_payload.status,
                Some(0),
                "Expected fshost to exit abnormally with non-zero status, but got status 0: {:?}",
                stopped_payload
            );
            assert_ne!(
                stopped_payload.exit_code,
                Some(0),
                "Expected fshost to exit abnormally with non-zero exit_code, but got exit_code 0: {:?}",
                stopped_payload
            );
        }
        _ => panic!("Unexpected event on ExecutionController"),
    }

    // Assert fshost process termination (reboot hook)
    let status = data_dir.query_filesystem().await;
    assert_matches!(
        status,
        Err(fidl::Error::ClientChannelClosed { epitaph: fidl::Epitaph::PeerClosed, .. })
    );

    // Tear down (clean test termination)
    fixture.tear_down().await;
}

#[fuchsia::test]
async fn test_fshost_clean_shutdown_does_not_report_crash() {
    let mut builder = new_builder();
    builder.with_disk().format_volumes(volumes_spec()).format_data(data_fs_spec());
    let fixture = builder.build().await;

    // Verify storage works
    fixture.check_fs_type("data", data_fs_type()).await;

    // Perform clean teardown. fixture.tear_down asserts internally that fixture.crash_reports has
    // no queued entries.
    fixture.tear_down().await;
}
