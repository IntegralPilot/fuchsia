// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, Result};
use fidl_fuchsia_component_sandbox as fsandbox;
use fidl_fuchsia_examples::EchoMarker;
use fidl_test_example::{RealmFactoryRequest, RealmFactoryRequestStream, RealmOptions};
use fuchsia_component::client;
use fuchsia_component::runtime::Dictionary;
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
    let store = client::connect_to_protocol::<fsandbox::CapabilityStoreMarker>().unwrap();
    let id_gen = sandbox::CapabilityIdGenerator::new();
    let mut realms = vec![];
    let result: Result<(), Error> = async move {
        while let Ok(Some(request)) = stream.try_next().await {
            match request {
                RealmFactoryRequest::_UnknownMethod { .. } => unimplemented!(),
                RealmFactoryRequest::CreateRealm2 { options, dictionary, responder } => {
                    let realm = create_realm(options).await?;
                    let handle = realm.root.controller().get_output_dictionary().await?.unwrap();
                    let output_dictionary = Dictionary::from(handle);
                    output_dictionary.associate_with_handle(dictionary).await;
                    realms.push(realm);
                    responder.send(Ok(()))?;
                }
                RealmFactoryRequest::CreateRealm { options, dictionary, responder } => {
                    let realm = create_realm(options).await?;
                    let dict_ref = realm.root.controller().get_exposed_dictionary().await?.unwrap();
                    let dict_id = id_gen.next();
                    store
                        .import(dict_id, fsandbox::Capability::Dictionary(dict_ref))
                        .await
                        .unwrap()
                        .unwrap();
                    store
                        .dictionary_legacy_export(dict_id, dictionary.into_channel())
                        .await
                        .unwrap()
                        .unwrap();
                    realms.push(realm);
                    responder.send(Ok(()))?;
                }
            }
        }
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
    let echo = builder.add_child("echo", "#meta/echo_server.cm", ChildOptions::new()).await?;
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<EchoMarker>())
                .from(&echo)
                .to(Ref::parent()),
        )
        .await?;
    let realm = builder.build().await?;
    Ok(realm)
}
