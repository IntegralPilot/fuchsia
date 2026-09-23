// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use diagnostics_data::InspectData;
use ffx_e2e_emu::IsolatedEmulator;

#[fuchsia::test(logging = true)]
async fn taking_lease_adds_lease_to_broker_inspect() {
    let emu = IsolatedEmulator::start("application-activity-test").await.unwrap();

    emu.ffx(&["power", "suspend", "prevent"]).await.unwrap();

    // Wait until application_activity level changing to active.
    let mut retries = 30;
    while get_application_activity_level(&emu).await != Some(1) {
        if retries == 0 {
            panic!("Timed out waiting for application_activity level to become active (1)");
        }
        retries -= 1;
        fuchsia_async::Timer::new(std::time::Duration::from_secs(1)).await;
    }
}

async fn get_application_activity_level(emu: &IsolatedEmulator) -> Option<i64> {
    let sag_inspect_json = emu
        .ffx_output(&[
            "--machine",
            "json",
            "inspect",
            "show",
            "/bootstrap/system-activity-governor",
        ])
        .await
        .ok()?;
    let data: Vec<InspectData> = serde_json::from_str(&sag_inspect_json).ok()?;
    if data.len() != 1 {
        return None;
    }
    data[0]
        .payload
        .as_ref()?
        .get_child("power_elements")?
        .get_child("application_activity")?
        .get_property("power_level")?
        .clone()
        .number_as_int()
}
