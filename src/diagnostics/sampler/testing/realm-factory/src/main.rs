// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod mocks;
mod realm_factory;
use crate::realm_factory::*;

use anyhow::{Error, Result};
use fidl_test_sampler::{RealmFactoryRequest, RealmFactoryRequestStream};
use fuchsia_component::runtime::Dictionary;
use fuchsia_component::server::ServiceFs;
use futures::{StreamExt, TryStreamExt};
use log::error;

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(|stream: RealmFactoryRequestStream| stream);
    fs.take_and_serve_directory_handle()?;
    fs.for_each_concurrent(0, serve_realm_factory).await;
    Ok(())
}

async fn serve_realm_factory(mut stream: RealmFactoryRequestStream) {
    let mut realms = vec![];
    let result: Result<(), Error> = async move {
        while let Ok(Some(request)) = stream.try_next().await {
            match request {
                RealmFactoryRequest::CreateRealm { options, dictionary, responder } => {
                    let realm = create_realm(options).await?;
                    let output_dictionary_handle =
                        realm.root.controller().get_output_dictionary().await?.unwrap();
                    let output_dictionary = Dictionary::from(output_dictionary_handle);
                    output_dictionary.associate_with_handle(dictionary).await;
                    realms.push(realm);
                    responder.send(Ok(()))?;
                }

                RealmFactoryRequest::_UnknownMethod { .. } => todo!(),
            }
        }
        Ok(())
    }
    .await;

    if let Err(err) = result {
        error!("{:?}", err);
    }
}
