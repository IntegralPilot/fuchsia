// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This program serves two Trigger protocols: one from the outgoing directory in the standard
//! manner (Trigger-c), and one from a dynamic dictionary that is exposed using the Router
//! protocol. This lets us test a dictionary that is a composite of dynamic and static routes.

use fidl::endpoints;
use fidl_fidl_examples_routing_echo as fecho;
use fidl_fidl_test_components as ftest;
use fidl_fuchsia_component_runtime as fruntime;
use fuchsia_async as fasync;
use fuchsia_component::client;
use fuchsia_component::runtime::{Connector, ConnectorReceiver, Dictionary};
use fuchsia_component::server::ServiceFs;
use futures::{StreamExt, TryStreamExt};
use log::info;
use vfs::file::vmo::read_only;

enum IncomingRequest {
    Router(fruntime::DictionaryRouterRequestStream),
    Trigger(ftest::TriggerRequestStream),
}

#[fuchsia::main]
async fn main() {
    info!("trigger.cm started");
    let args: Vec<String> = std::env::args().collect();
    let verify_dir = args
        .iter()
        .position(|a| a == "--verify-dir")
        .and_then(|i| args.get(i + 1).map(|s| s.to_string()));
    let dictionary = Dictionary::new().await;
    let (connector, trigger_receiver) = Connector::new().await;
    dictionary.insert("fidl.test.components.Trigger-d", connector).await;
    info!("trigger.cm populated the dictionary");

    let _receiver_task =
        fasync::Task::local(async move { handle_receiver(trigger_receiver).await });

    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(IncomingRequest::Trigger);
    fs.dir("svc").add_fidl_service(IncomingRequest::Router);

    let out_dir = vfs::pseudo_directory! {
        "example_file" => read_only(b"Hello from dictionary!")
    };
    fs.add_entry_at("dict_dir", out_dir);

    fs.take_and_serve_directory_handle().expect("failed to serve outgoing directory");
    let verify_dir = verify_dir.clone();
    fs.for_each_concurrent(None, move |request: IncomingRequest| {
        let dictionary = dictionary.clone();
        let verify_dir = verify_dir.clone();
        async move {
            match request {
                IncomingRequest::Trigger(stream) => {
                    if let Some(dir_path) = &verify_dir {
                        match std::fs::read_to_string(format!("{}/example_file", dir_path)) {
                            Ok(content) => {
                                if content.trim() == "Hello from dictionary!" {
                                    run_trigger_service("Directory verified", stream).await
                                } else {
                                    run_trigger_service("Directory content mismatch", stream).await
                                }
                            }
                            Err(e) => {
                                run_trigger_service(
                                    &format!("Directory read failed: {:?}", e),
                                    stream,
                                )
                                .await
                            }
                        }
                    } else {
                        run_trigger_service("Triggered c", stream).await
                    }
                }
                IncomingRequest::Router(mut stream) => {
                    while let Ok(Some(request)) = stream.try_next().await {
                        match request {
                            fruntime::DictionaryRouterRequest::Route {
                                handle, responder, ..
                            } => {
                                dictionary.associate_with_handle(handle).await;
                                let _ = responder.send(Ok(fruntime::RouterResponse::Success));
                            }
                            fruntime::DictionaryRouterRequest::_UnknownMethod { .. } => {
                                unimplemented!()
                            }
                        }
                    }
                }
            }
        }
    })
    .await;
}

async fn run_trigger_service(echo_str: &str, mut stream: ftest::TriggerRequestStream) {
    let echo =
        client::connect_to_protocol::<fecho::EchoMarker>().expect("error connecting to echo");
    while let Some(event) = stream.try_next().await.expect("failed to serve trigger service") {
        let ftest::TriggerRequest::Run { responder } = event;
        let out = echo.echo_string(Some(echo_str)).await.expect("echo_string failed");
        let out = out.expect("empty echo result");
        responder.send(&out).expect("failed to send trigger response");
    }
}

async fn handle_receiver(mut receiver: ConnectorReceiver) {
    let scope = fasync::Scope::new();
    while let Some(channel) = receiver.next().await {
        scope.spawn(async move {
            let server_end = endpoints::ServerEnd::<ftest::TriggerMarker>::new(channel.into());
            run_trigger_service("Triggered d", server_end.into_stream()).await;
        });
    }
}
