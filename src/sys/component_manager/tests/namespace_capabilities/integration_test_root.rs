// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fidl_test_components as ftest;
use fuchsia_component::client;
use log::*;

#[fuchsia::main]
async fn main() {
    // This root component connects to the trigger.
    // The trigger checks all component-manager-namespace capabilities routed to it.
    info!("Started");
    let trigger = client::connect_to_protocol::<ftest::TriggerMarker>()
        .expect("failed to open trigger service");
    info!("Invoking trigger");
    let out = trigger.run().await.unwrap_or_else(|e| panic!("trigger failed: {:?}", e));
    assert_eq!(out, "Triggered");
    info!("Done");
}
