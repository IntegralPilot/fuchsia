// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fidl_examples_routing_echo::{EchoMarker, EchoRequest, EchoRequestStream};
use fuchsia_component::client;
use fuchsia_component::server::ServiceFs;
use futures::{StreamExt, TryStreamExt};

enum IncomingRequest {
    Echo(EchoRequestStream),
}

async fn run_echo_service(mut stream: EchoRequestStream) {
    // Proxy the connection, connect to the incoming TriggerService
    let trigger = client::connect_to_protocol::<EchoMarker>().unwrap();
    while let Some(event) = stream.try_next().await.expect("failed to serve trigger service") {
        let EchoRequest::EchoString { responder, value } = event;
        let out = trigger.echo_string(value.as_ref().map(|x| x.as_str())).await.unwrap();
        responder.send(out.as_ref().map(|x| x.as_str())).expect("failed to send trigger response");
    }
}

#[fuchsia::main]
async fn main() {
    let mut fs = ServiceFs::new_local();
    // Generic trigger, routed to all clients
    fs.dir("svc").add_fidl_service(IncomingRequest::Echo);
    fs.take_and_serve_directory_handle().expect("failed to serve outgoing directory");

    fs.for_each_concurrent(None, move |request: IncomingRequest| async move {
        match request {
            IncomingRequest::Echo(stream) => run_echo_service(stream).await,
        }
    })
    .await;
}
