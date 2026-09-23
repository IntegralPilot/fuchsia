// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, bail};
use ffx_e2e_emu::IsolatedEmulator;
use fuchsia_async::Timer;
use log::info;
use serde::Deserialize;
use std::time::Duration;

// Note: we don't need to fully deserialize because we just test that these two
// fields are non-empty.
#[derive(Debug, Deserialize)]
struct Snapshot {
    executable_regions: Vec<serde_json::Value>,
    stack_trace_groups: Vec<serde_json::Value>,
}

/// Waits for the collector to report a snapshot.
async fn wait_for_snapshot(emu: &IsolatedEmulator) -> Result<Snapshot> {
    const ONE_SECOND: Duration = Duration::from_secs(1);
    const MAX_ATTEMPTS: usize = 30;

    info!("waiting for stacktrack to report peak stack usage...");
    for _ in 0..MAX_ATTEMPTS {
        if let Ok(output) = emu.ffx_output(&["--machine", "json", "profile", "stacktrack"]).await {
            if let Ok(snapshot) = serde_json::from_str::<Snapshot>(&output) {
                if !snapshot.executable_regions.is_empty()
                    && !snapshot.stack_trace_groups.is_empty()
                {
                    return Ok(snapshot);
                }
            }
        }
        Timer::new(ONE_SECOND).await;
    }

    bail!("Timeout while waiting for stacktrack snapshot");
}

#[fuchsia::test(logging = true)]
async fn test_ffx_profile_stacktrack() {
    let emu = IsolatedEmulator::start("test-ffx-profile-stacktrack").await.unwrap();

    info!("Starting stacktrack's example component...");
    let moniker = "/core/ffx-laboratory:stacktrack-example";
    let url = "fuchsia-pkg://fuchsia.com/stacktrack-example#meta/stacktrack-example.cm";
    emu.ffx(&["component", "run", moniker, url]).await.unwrap();

    info!("Waiting for stack snapshot...");
    let snapshot = wait_for_snapshot(&emu).await.unwrap();

    assert!(!snapshot.executable_regions.is_empty());
    assert!(!snapshot.stack_trace_groups.is_empty());
}
