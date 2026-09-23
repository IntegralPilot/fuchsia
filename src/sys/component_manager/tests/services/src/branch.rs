// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{format_err, Context, Error};
use argh::FromArgs;
use fuchsia_component::client;
use fuchsia_component::server::ServiceFs;
use futures::{StreamExt, TryStreamExt};
use log::*;
use {
    fidl_fidl_test_components as ftest, fidl_fuchsia_component as fcomponent,
    fidl_fuchsia_component_decl as fdecl,
};

#[derive(FromArgs, Debug)]
/// Options for the branch component.
struct Args {
    #[argh(option, description = "how many collections to aggregate from (1 or 2)")]
    num_collections: usize,
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let args: Args = argh::from_env();

    // Create two BankAccount providers into the `account_providers` collection.
    // The providers are not eagerly started. The test should start them as needed.
    let realm = client::connect_to_protocol::<fcomponent::RealmMarker>()
        .context("connect to Realm service")?;
    match args.num_collections {
        1 => {
            create_provider(&realm, "a", "#meta/provider-a.cm", "account_providers").await?;
            create_provider(&realm, "b", "#meta/provider-b.cm", "account_providers").await?;
        }
        2 => {
            create_provider(&realm, "a", "#meta/provider-a.cm", "account_providers_1").await?;
            create_provider(&realm, "b", "#meta/provider-b.cm", "account_providers_2").await?;
        }
        n => {
            panic!("invalid num_collections {}", n);
        }
    }

    enum IncomingRequest {
        Trigger(ftest::TriggerRequestStream),
    }

    // Run a Trigger service to allow the test to synchronize on the instantiation of the
    // providers.
    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(IncomingRequest::Trigger);
    fs.take_and_serve_directory_handle()?;
    fs.for_each_concurrent(None, |request: IncomingRequest| async move {
        match request {
            IncomingRequest::Trigger(mut stream) => {
                while let Ok(Some(request)) = stream.try_next().await {
                    let ftest::TriggerRequest::Run { responder } = request;
                    let _ = responder.send("done");
                }
            }
        }
    })
    .await;

    Ok(())
}

/// Creates a BankAccount provider component in `coll`.
///
/// This does not start the component.
async fn create_provider(
    realm: &fcomponent::RealmProxy,
    name: &str,
    url: &str,
    coll: &str,
) -> Result<(), Error> {
    info!("creating BankAccount provider \"{}\" with url={}", name, url);
    realm
        .create_child(
            &fdecl::CollectionRef { name: coll.into() },
            &fdecl::Child {
                name: Some(name.to_string()),
                url: Some(url.to_string()),
                startup: Some(fdecl::StartupMode::Lazy),
                environment: None,
                ..Default::default()
            },
            fcomponent::CreateChildArgs::default(),
        )
        .await
        .context("failed to call CreateChild")?
        .map_err(|e| format_err!("Failed to create child: {:?}", e))?;

    Ok(())
}
