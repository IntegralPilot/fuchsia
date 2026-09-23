// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ffx_e2e_emu::IsolatedEmulator;
use log::info;

#[fuchsia::test(logging = true)]
async fn override_echo_greeting_and_observe_in_logs() {
    let emu = IsolatedEmulator::start("test-ffx-create-parent-overrides").await.unwrap();
    let moniker = "/core/ffx-laboratory:create-parent-override-echo";
    let url = "fuchsia-pkg://fuchsia.com/ffx_create_parent_overrides_echo#meta/echo_config.cm";
    let expected_greeting = "Hello from ffx parent overrides!";

    info!("running test component with a config override...");
    emu.ffx(&[
        "component",
        "create",
        moniker,
        url,
        "--config",
        &format!("greeting=\"{expected_greeting}\""),
    ])
    .await
    .unwrap();

    emu.ffx(&["component", "start", moniker]).await.unwrap();

    info!("checking for expected log message...");
    'log_search: loop {
        for message in emu.logs_for_moniker(moniker).await.unwrap() {
            if message.msg() == Some(expected_greeting) {
                info!("found expected log message!");
                break 'log_search;
            }
        }
        info!("expected log message not found, retrying...");
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
