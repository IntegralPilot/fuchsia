// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, Result};
use fidl_fuchsia_component_sandbox as fsandbox;
use fidl_fuchsia_element::ManagerMarker;
use fidl_fuchsia_element_test::*;
use fidl_fuchsia_testing_harness::RealmProxy_RequestStream;
use fuchsia_async as fasync;
use fuchsia_component::client;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, RealmInstance, Ref, Route};
use futures::{StreamExt, TryStreamExt};
use log::error;
use zx_status;

enum IncomingService {
    RealmFactory(RealmFactoryRequestStream),
    RealmProxy(RealmProxy_RequestStream),
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let mut fs = ServiceFs::new();
    fs.dir("svc")
        .add_fidl_service(IncomingService::RealmFactory)
        .add_fidl_service(IncomingService::RealmProxy);
    fs.take_and_serve_directory_handle()?;
    fs.for_each_concurrent(0, serve_realm_factory).await;
    Ok(())
}

async fn serve_realm_factory(service: IncomingService) {
    match service {
        IncomingService::RealmFactory(stream) => {
            if let Err(err) = handle_request_stream(stream).await {
                error!("{:?}", err);
            }
        }
        // TODO(299966655): Delete after we branch for F15.
        IncomingService::RealmProxy(stream) => {
            let options = RealmOptions::default();
            let realm = create_realm(options).await.expect("failed to build the test realm");
            realm_proxy::service::serve(realm, stream)
                .await
                .expect("failed to serve the realm proxy");
        }
    }
}

async fn handle_request_stream(mut stream: RealmFactoryRequestStream) -> Result<()> {
    let scope = fasync::Scope::new();
    let mut realms = vec![];
    let id_gen = sandbox::CapabilityIdGenerator::new();
    let store = client::connect_to_protocol::<fsandbox::CapabilityStoreMarker>().unwrap();
    while let Ok(Some(request)) = stream.try_next().await {
        match request {
            RealmFactoryRequest::CreateRealm { options, realm_server, responder } => {
                let realm = create_realm(options).await?;
                let request_stream = realm_server.into_stream();
                scope.spawn(async move {
                    realm_proxy::service::serve(realm, request_stream).await.unwrap();
                });
                responder.send(Ok(()))?;
            }
            RealmFactoryRequest::CreateRealm2 { options, dictionary, responder } => {
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

            RealmFactoryRequest::_UnknownMethod { control_handle, .. } => {
                control_handle.shutdown_with_epitaph(zx_status::Status::NOT_SUPPORTED);
                return Ok(());
            }
        }
    }

    scope.join().await;
    Ok(())
}

async fn create_realm(_: RealmOptions) -> Result<RealmInstance, Error> {
    let builder = RealmBuilder::new().await?;
    let session =
        builder.add_child("session", "#meta/reference-session.cm", ChildOptions::new()).await?;
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<ManagerMarker>())
                .from(&session)
                .to(Ref::parent()),
        )
        .await?;
    builder
        .add_route(
            Route::new().capability(Capability::storage("data")).from(Ref::parent()).to(&session),
        )
        .await?;
    Ok(builder.build().await?)
}
