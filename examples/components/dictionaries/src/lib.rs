// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assert_matches::assert_matches;
use fidl_fidl_examples_routing_echo::EchoMarker;
use fuchsia_component::client;
use log::*;

#[fuchsia::test]
async fn use_runtime_populated_dictionary() {
    info!("Started");

    // Connect to all 3 protocols that we expect to be present in the dynamically
    // backed dictionary. (The protocols were already extracted from the dictionary by this
    // component's CML file.)
    // [START connect]
    for i in 1..=3 {
        info!("Connecting to Echo protocol {i} of 3");
        let echo = client::connect_to_protocol_at_path::<EchoMarker>(&format!(
            "/svc/fidl.examples.routing.echo.Echo-{i}"
        ))
        .unwrap();
        let res = echo.echo_string(Some(&format!("hello"))).await;
        assert_matches!(res, Ok(Some(s)) if s == format!("hello {i}"));
    }
    // [END connect]
}
