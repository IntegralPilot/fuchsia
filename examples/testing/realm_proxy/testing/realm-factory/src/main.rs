// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, Result};
use fidl_fidl_examples_routing_echo as fecho;
use fidl_test_echoserver::{RealmFactoryRequest, RealmFactoryRequestStream, RealmOptions};
use fuchsia_async as fasync;
use fuchsia_component::runtime::{Connector, Dictionary};
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, RealmInstance, Ref, Route};
use futures::{StreamExt, TryStreamExt};
use log::*;

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(|stream: RealmFactoryRequestStream| stream);
    fs.take_and_serve_directory_handle()?;
    fs.for_each_concurrent(0, serve_realm_factory).await;
    Ok(())
}

async fn serve_realm_factory(mut stream: RealmFactoryRequestStream) {
    let scope = fasync::Scope::new();
    let result: Result<(), Error> = async move {
        while let Ok(Some(request)) = stream.try_next().await {
            match request {
                RealmFactoryRequest::CreateRealm { options, responder } => {
                    let realm = create_realm(options).await?;

                    // Get a dict containing the capabilities exposed by the realm.
                    let handle = realm.root.controller().get_output_dictionary().await?.unwrap();
                    let output_dictionary = Dictionary::from(handle);

                    // Mix in additional capabilities to the dict.
                    //
                    // TODO(https://fxbug.dev/298100106): Could RealmInstance expose a higher-level
                    // API for getting the realm's exposed dict, along with APIs that make it
                    // easy to add in more capabilities served by this component? For example:
                    //
                    // let mut bundle = realm.get_root_bundle().extend().await?.unwrap();
                    // let echo_request_stream =
                    //     bundles::add_fidl_service::<fecho::EchoMarker>(&bundle);
                    // "serves" the bundle over `bundle_server` which was sent from the client.
                    // Also moves `bundle` so it can't be modified further.
                    // bundle.serve(bundle_server).await?.unwrap();
                    //
                    // ... code to serve echo here ...

                    let (connector, receiver) = Connector::new().await;
                    output_dictionary.insert("reverse-echo", connector).await;

                    // Serve the mixed-in capability.
                    scope.spawn(async move {
                        let _realm = realm;
                        let _ = realm_proxy::service::handle_receiver::<fecho::EchoMarker, _, _>(
                            receiver,
                            handle_echo_request_stream,
                        )
                        .await
                        .map_err(|e| {
                            error!("Failed to serve echo stream: {}", e);
                        });
                    });

                    responder.send(Ok(output_dictionary.handle))?;
                }
                RealmFactoryRequest::_UnknownMethod { .. } => unimplemented!(),
            }
        }

        scope.join().await;
        Ok(())
    }
    .await;

    if let Err(err) = result {
        error!("{:?}", err);
    }
}

async fn create_realm(options: RealmOptions) -> Result<RealmInstance, Error> {
    info!("building the realm using options {:?}", options);

    let builder = RealmBuilder::new().await?;
    let echo =
        builder.add_child("echo", "echo_server#meta/default.cm", ChildOptions::new()).await?;
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fecho::EchoMarker>())
                .from(&echo)
                .to(Ref::parent()),
        )
        .await?;

    let realm = builder.build().await?;
    Ok(realm)
}

async fn handle_echo_request_stream(mut stream: fecho::EchoRequestStream) {
    while let Ok(Some(request)) = stream.try_next().await {
        match request {
            fecho::EchoRequest::EchoString { value, responder } => {
                let value = value.map(|value| {
                    let mut reversed = String::with_capacity(value.len());
                    let mut chars: Vec<_> = value.chars().collect();
                    chars.reverse();
                    for ch in chars {
                        reversed.push(ch);
                    }
                    reversed
                });
                responder.send(value.as_deref()).unwrap();
            }
        }
    }
}
