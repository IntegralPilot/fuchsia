// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fasync::TimeoutExt;
use fidl_fidl_examples_routing_echo as fecho;
use fuchsia_async as fasync;
use fuchsia_component::client::connect_to_protocol;
use std::time::Duration;

#[fuchsia::main]
async fn main() {
    let timeout = rand::random_range(0..10);
    let timeout = Duration::from_secs(timeout);

    // TODO(xbhatnag): Expose a service to control this component from the test
    send_echo().on_timeout(timeout, || ()).await;
}

async fn send_echo() {
    loop {
        if let Ok(echo) = connect_to_protocol::<fecho::EchoMarker>() {
            if let Ok(Some(result)) = echo.echo_string(Some("Hippos rule!")).await {
                assert_eq!(result, "Hippos rule!");
            }
        }
    }
}
