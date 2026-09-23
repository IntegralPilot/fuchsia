// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ffx_e2e_emu::IsolatedEmulator;
use futures::StreamExt;
use log::info;

#[fuchsia::test(logging = true)]
async fn override_echo_greeting_and_observe_in_logs() {
    let emu = IsolatedEmulator::start("test-ffx-run-parent-overrides").await.unwrap();

    let moniker = "/core/ffx-laboratory:run-parent-override-echo";
    let url = "fuchsia-pkg://fuchsia.com/ffx_run_parent_overrides_echo#meta/echo_config.cm";
    let expected_greeting = "Hello from ffx parent overrides!";

    info!("running test component with a config override...");
    emu.ffx(&[
        "component",
        "run",
        moniker,
        url,
        "--config",
        &format!("greeting=\"{expected_greeting}\""),
    ])
    .await
    .unwrap();

    info!("checking for expected log message...");
    let mut stream = std::pin::pin!(emu.log_stream_for_moniker(moniker).await.unwrap());
    while let Some(message) = stream.next().await {
        if message.unwrap().msg() == Some(expected_greeting) {
            info!("found expected log message!");
            break;
        }
    }
}
