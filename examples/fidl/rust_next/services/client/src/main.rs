// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error};
use fidl_next::fuchsia::create_channel;
use fidl_next_fuchsia_examples::*;
use fuchsia_component::client::fidl_next::connect_to_service_instance;

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let service = connect_to_service_instance::<EchoService>("default")
        .context("failed to connect to service instance")?;

    let (client_end, server_end) = create_channel::<Echo>();
    service.regular_echo(server_end)?;

    let regular = client_end.spawn();
    let regular_response = regular.echo_string("hello world!").await?;
    println!("regular response: {:?}", regular_response.response);

    let (client_end, server_end) = create_channel::<Echo>();
    service.reversed_echo(server_end)?;

    let reversed = client_end.spawn();
    let reversed_response = reversed.echo_string("hello world!").await?;
    println!("reversed response: {:?}", reversed_response.response);

    Ok(())
}
