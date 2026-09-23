// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context;
use fidl_fuchsia_examples_reverser::{ReplacerMarker, ReverserRequest, ReverserRequestStream};
use fuchsia_component::client::connect_to_protocol;
use fuchsia_component::server::ServiceFs;
use futures::prelude::*;
use reverser_config::Config;

enum IncomingRequest {
    Reverser(ReverserRequestStream),
}

#[fuchsia::main(logging = true)]
async fn main() -> Result<(), anyhow::Error> {
    let config = Config::take_from_startup_handle();
    let mut service_fs = ServiceFs::new_local();

    service_fs.dir("svc").add_fidl_service(IncomingRequest::Reverser);
    service_fs.take_and_serve_directory_handle().context("failed to serve outgoing namespace")?;

    service_fs
        .for_each_concurrent(None, |request: IncomingRequest| async {
            match request {
                IncomingRequest::Reverser(stream) => handle_reverser_request(stream, &config).await,
            }
        })
        .await;

    Ok(())
}

async fn handle_reverser_request(mut stream: ReverserRequestStream, config: &Config) {
    while let Some(event) = stream.try_next().await.expect("failed to serve reverser service") {
        let ReverserRequest::Reverse { value, responder } = event;

        let processed_input = if config.use_replacer {
            if let Ok(replacer) = connect_to_protocol::<ReplacerMarker>() {
                replacer.replace(&value).await.unwrap_or(value)
            } else {
                value
            }
        } else {
            value
        };

        let mut reversed = String::with_capacity(processed_input.len());
        if config.switch_case {
            for c in processed_input.chars().rev() {
                if c.is_uppercase() {
                    reversed.extend(c.to_lowercase());
                } else if c.is_lowercase() {
                    reversed.extend(c.to_uppercase());
                } else {
                    reversed.push(c);
                }
            }
        } else {
            reversed.extend(processed_input.chars().rev());
        };

        responder.send(&reversed).expect("failed to send response");
    }
}
