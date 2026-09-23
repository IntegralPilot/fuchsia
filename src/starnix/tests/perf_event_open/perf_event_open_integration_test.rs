// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assert_matches::assert_matches;
use component_events::events::{EventStream, ExitStatus, Stopped, StoppedPayload};
use component_events::matcher::EventMatcher;
use fuchsia_component_test::ScopedInstance;
use log::info;

/// Test that the `fuchsia.cpu.profiler.Session` protocol (used for Fuchsia Profiler)
/// is correctly routed to the Starnix kernel. This means that it is available in the
/// eng build itself, and is not solely available in the testing environment.
#[fuchsia::main]
async fn main() {
    let mut events = EventStream::open().await.unwrap();
    // Ensure that Starnix container's children can access the profiler.
    let collection = "starnix_container_children";
    let child_name = "perf_event_open_test";
    let url = "#meta/perf_event_open_integration_test_linux.cm";
    let moniker = format!("{collection}:{child_name}");

    info!("Creating scoped instance for {}", url);
    let _instance = ScopedInstance::new_with_name(child_name.into(), collection.into(), url.into())
        .await
        .unwrap();

    info!("Waiting for {} to stop...", moniker);
    let stopped = EventMatcher::ok().moniker(&moniker).wait::<Stopped>(&mut events).await.unwrap();
    assert_matches!(stopped.result(), Ok(StoppedPayload { status: ExitStatus::Clean, .. }));
    info!("Verified that fuchsia.cpu.profiler.Session protocol has been routed correctly.");
}
