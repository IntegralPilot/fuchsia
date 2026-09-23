// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_test_wlan_realm::{TraceManagerHermeticity, WlanConfig};
use fuchsia_async::Timer;
use log::info;
use std::time::Duration;
use wlan_hw_sim::*;

/// Stress test for beginning a hw-sim test. This test should tease out flakes
/// in the starting sequence much more often than they're seen while running
/// other hw-sim tests.
#[fuchsia::test]
async fn stress_begin_test() {
    for test_number in 1..=50 {
        info!("Creating new test realm for test #{test_number}...");
        let ctx = test_utils::TestRealmContext::new(WlanConfig {
            use_legacy_privacy: Some(false),
            with_regulatory_region: Some(true),
            with_policy: Some(true),
            trace_manager_hermeticity: Some(TraceManagerHermeticity::NonHermetic),
            ..Default::default()
        })
        .await;

        let test_ns_prefix = ctx.test_ns_prefix().to_string();
        info!("Starting test #{test_number} with test namespace {test_ns_prefix}...");
        let _helper =
            test_utils::TestHelper::begin_test_with_context(ctx, default_wlantap_config_client())
                .await;
        info!("Completed test #{test_number} with test namespace {test_ns_prefix}...");

        // Random delay to insert some entropy into this test.
        Timer::new(Duration::from_millis(rand::random_range(0..1000))).await;
    }
}
