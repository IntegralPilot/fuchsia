// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fidl_test_components as ftest;
use fuchsia_component::client::connect_to_protocol;

#[fuchsia::main]
/// Tries to connect to the Trigger service, which it should not have access
/// to. This should generate an expect log message from component manager that
/// will be attributed to this component.
async fn main() {
    let trigger = match connect_to_protocol::<ftest::TriggerMarker>() {
        Ok(t) => t,
        Err(_) => panic!("failed to connect to Trigger"),
    };

    let _ = trigger.run().await;

    futures::future::pending::<()>().await;
}
