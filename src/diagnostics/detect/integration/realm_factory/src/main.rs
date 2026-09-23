// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod realm_events;
mod realm_factory;
mod realm_options;
use crate::realm_factory::*;

use anyhow::{Error, Result};
use fidl_test_detect_factory::*;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
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
    let mut factory = RealmFactory::new();
    let result: Result<(), Error> = async move {
        while let Ok(Some(request)) = stream.try_next().await {
            match request {
                RealmFactoryRequest::_UnknownMethod { .. } => unimplemented!(),
                RealmFactoryRequest::GetTriageDetectEvents { responder } => {
                    responder.send(factory.get_events_client()?)?;
                }

                RealmFactoryRequest::CreateRealm { options, realm_server, responder } => {
                    let realm = factory.create_realm(options).await?;
                    let request_stream = realm_server.into_stream();
                    scope.spawn(async move {
                        realm_proxy::service::serve(realm, request_stream).await.unwrap();
                    });
                    responder.send(Ok(()))?;
                }
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
