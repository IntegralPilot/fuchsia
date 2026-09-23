// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fidl_examples_routing_echo::EchoMarker;
use fuchsia_component::client;

#[fuchsia::test]
async fn route_to_all() {
    // Validate we can connect directly through the dictionary
    let trigger = client::connect_to_protocol::<EchoMarker>().unwrap();
    let result = trigger
        .echo_string(Some("Hello world!"))
        .await
        .expect("Unable to connect to client")
        .expect("Expected string");
    assert_eq!(&result, "Hello world!");
    // Validate the protocol is proxied properly
    let trigger = client::connect_to_protocol_at_path::<EchoMarker>(
        "/svc/fidl.examples.routing.echo.Echo-sibling",
    )
    .unwrap();
    let result = trigger
        .echo_string(Some("Hello world!"))
        .await
        .expect("Unable to connect to client")
        .expect("Expected string");
    assert_eq!(&result, "Hello world!");
}
