// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error};
use fidl_next_fuchsia_examples::echo::OnString;
use fidl_next_fuchsia_examples::{Echo, EchoClientHandler};
use fuchsia_component::client::fidl_next::connect_to_protocol;

struct EventHandler {
    client: fidl_next::Client<Echo>,
}

impl EchoClientHandler for EventHandler {
    async fn on_string(
        &mut self,
        request: fidl_next::Request<OnString, fidl_next::fuchsia::zx::Channel>,
    ) {
        // The request parameter contains the event data.
        // We need to access the 'response' field from the event request.
        println!("Received OnString event for string {:?}", request.payload().response);
        self.client.close();
    }
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    // Connect to the Echo protocol
    let client_end = connect_to_protocol::<Echo>().context("Failed to connect to echo service")?;
    let (client, task) = client_end.spawn_handler_full_with(|client| EventHandler { client });

    // Make an EchoString request
    let response = client.echo_string("hello").await.context("echo_string failed")?;
    println!("response: {:?}", response.response);

    // Make a SendString request
    // send_string returns a SendFuture, we need to await it to send
    client.send_string("hi").await.context("send_string failed")?;

    // Wait for the dispatcher task to complete (which happens when EventHandler closes the client)
    task.await.context("dispatcher task failed")?.unwrap();

    Ok(())
}
