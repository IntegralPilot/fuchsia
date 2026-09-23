// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};
use fidl_next::fuchsia::create_channel;
use fidl_next_fuchsia_examples::{Echo, EchoLauncher};
use fuchsia_component::client::fidl_next::connect_to_protocol;
use futures::join;
// use futures::prelude::*;

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let echo_launcher =
        connect_to_protocol::<EchoLauncher>().context("Failed to connect to echo service")?.spawn();

    // Create a future that obtains an Echo protocol using the non-pipelined
    // GetEcho method
    let non_pipelined_fut = async {
        let client_end = echo_launcher.get_echo("not pipelined").await?.response;
        // Spawn the client and make an EchoString request
        let client = client_end.spawn();
        let response = client.echo_string("hello").await?.response;
        println!("Got echo response {}", response);
        Ok::<(), Error>(())
    };

    // Create a future that obtains an Echo protocol using the pipelined GetEcho
    // method
    let (client_end, server_end) = create_channel::<Echo>();
    let client = client_end.spawn();
    // `get_echo_pipelined` is a one-way FIDL request, so the `.await` only needs
    // to send a message. It does not wait for a reply, unlike `echo_string`.
    echo_launcher.get_echo_pipelined("pipelined", server_end).await?;

    // We can make a request to the server right after sending the pipelined request
    let pipelined_fut = async {
        let response = client.echo_string("hello").await?.response;
        println!("Got echo response {}", response);
        Ok::<(), Error>(())
    };

    // Run the two futures to completion
    let (non_pipelined_result, pipelined_result) = join!(non_pipelined_fut, pipelined_fut);
    pipelined_result?;
    non_pipelined_result?;
    Ok(())
}
