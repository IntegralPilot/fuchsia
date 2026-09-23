// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints;
use fidl_fidl_examples_routing_echo::{EchoMarker, EchoRequest, EchoRequestStream};
use fidl_fuchsia_component_runtime as fcomponent;
use fuchsia_async as fasync;
use fuchsia_component::runtime::{
    Connector, ConnectorReceiver, Dictionary, DictionaryRouterReceiver,
};
use fuchsia_component::server::ServiceFs;
use futures::{FutureExt, StreamExt, TryStreamExt};
use log::*;

enum IncomingRequest {
    Router(fcomponent::DictionaryRouterRequestStream),
}

#[fuchsia::main]
async fn main() {
    info!("Started");

    // [START init]

    // Create a dictionary
    let dictionary = Dictionary::new().await;

    // Add 3 Echo servers to the dictionary
    let receiver_tasks = fasync::Scope::new();
    for i in 1..=3 {
        let (connector, receiver) = Connector::new().await;
        dictionary.insert(&&format!("fidl.examples.routing.echo.Echo-{i}"), connector).await;
        receiver_tasks.spawn(handle_echo_receiver(i, receiver));
    }
    // [END init]

    info!("Populated the dictionary");

    // [START serve]
    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(IncomingRequest::Router);
    fs.take_and_serve_directory_handle().unwrap();
    fs.for_each_concurrent(None, move |request: IncomingRequest| {
        let dictionary = dictionary.clone();
        async move {
            match request {
                IncomingRequest::Router(stream) => {
                    let router_receiver = DictionaryRouterReceiver::from(stream);
                    router_receiver
                        .handle_with(move |_route_request, _instance_token| {
                            futures::future::ready(Ok(Some(dictionary.clone()))).boxed()
                        })
                        .await;
                }
            }
        }
    })
    .await;
    // [END serve]
}

// [START receiver]
async fn handle_echo_receiver(index: u64, mut receiver: ConnectorReceiver) {
    let scope = fasync::Scope::new();
    while let Some(channel) = receiver.next().await {
        scope.spawn(async move {
            let server_end = endpoints::ServerEnd::<EchoMarker>::new(channel.into());
            run_echo_server(index, server_end.into_stream()).await;
        });
    }
}

async fn run_echo_server(index: u64, mut stream: EchoRequestStream) {
    while let Ok(Some(event)) = stream.try_next().await {
        let EchoRequest::EchoString { value, responder } = event;
        let res = match value {
            Some(s) => responder.send(Some(&format!("{s} {index}"))),
            None => responder.send(None),
        };
        if let Err(err) = res {
            warn!(err:%; "Failed to send echo response");
        }
    }
}
// [END receiver]
