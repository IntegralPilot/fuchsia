// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// [START imports]
use anyhow::{Context as _, Error};
use fidl_fuchsia_examples::{EchoEvent, EchoMarker};
use fuchsia_component::client::connect_to_protocol_sync;

// [END imports]

// [START main]
fn main() -> Result<(), Error> {
    // Connect to the Echo protocol, returning a synchronous proxy
    let echo =
        connect_to_protocol_sync::<EchoMarker>().context("Failed to connect to echo service")?;

    // Make an EchoString request, with no timeout for receiving the response
    let res = echo.echo_string("hello", zx::MonotonicInstant::INFINITE)?;
    println!("response: {:?}", res);

    // Make a SendString request
    echo.send_string("hi")?;
    // Wait for a single OnString event.
    let EchoEvent::OnString { response } =
        echo.wait_for_event(zx::MonotonicInstant::INFINITE).context("error receiving events")?;
    println!("Received OnString event for string {:?}", response);

    Ok(())
}
// [END main]
