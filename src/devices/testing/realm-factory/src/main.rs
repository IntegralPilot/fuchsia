// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, Result};
use cm_rust::push_box;
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_driver_test as fdt;
use fidl_fuchsia_driver_testing::*;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_testing_harness::OperationError;
use fuchsia_async as fasync;
use fuchsia_component::directory::AsRefDirectory;
use fuchsia_component::runtime::Dictionary;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{
    Capability, ChildOptions, ChildRef, LocalComponentHandles, RealmBuilder, RealmInstance, Ref,
    Route,
};
use fuchsia_driver_test::{DriverTestRealmBuilder, DriverTestRealmInstance};
use futures::{FutureExt, StreamExt, TryStreamExt};
use log::*;
use std::sync::Arc;

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(|stream: RealmFactoryRequestStream| stream);
    fs.take_and_serve_directory_handle()?;
    fs.for_each_concurrent(0, serve_realm_factory).await;
    Ok(())
}

async fn serve_realm_factory(stream: RealmFactoryRequestStream) {
    if let Err(err) = handle_request_stream(stream).await {
        error!("{:?}", err);
    }
}

async fn handle_request_stream(mut stream: RealmFactoryRequestStream) -> Result<()> {
    let scope = fasync::Scope::new();
    let mut realms = vec![];
    loop {
        match stream.try_next().await {
            Ok(Some(request)) => match request {
                RealmFactoryRequest::CreateRealm { options, realm_server, responder } => {
                    let realm_result = create_realm(options).await;
                    match realm_result {
                        Ok(realm) => {
                            let request_stream = realm_server.into_stream();
                            scope.spawn(async move {
                                realm_proxy::service::serve(realm, request_stream).await.unwrap();
                            });

                            responder.send(Ok(()))?;
                        }
                        Err(e) => {
                            error!("Failed to create realm: {:?}", e);
                            responder.send(Err(OperationError::Invalid))?;
                        }
                    }
                }
                RealmFactoryRequest::CreateRealm2 { options, dictionary, responder } => {
                    let realm_result = create_realm(options).await;
                    match realm_result {
                        Ok(realm) => {
                            let output_handle =
                                realm.root.controller().get_output_dictionary().await?.unwrap();
                            let output_dictionary = Dictionary::from(output_handle);
                            output_dictionary.associate_with_handle(dictionary).await;
                            realms.push(realm);
                            responder.send(Ok(()))?;
                        }
                        Err(e) => {
                            error!("Failed to create realm: {:?}", e);
                            responder.send(Err(OperationError::Invalid))?;
                        }
                    }
                }

                RealmFactoryRequest::_UnknownMethod { control_handle, .. } => {
                    control_handle.shutdown_with_epitaph(zx_status::Status::NOT_SUPPORTED);
                }
            },
            Ok(None) => {
                warn!("handle_request_stream got None stream item.");
                break;
            }
            Err(e) => {
                error!("handle_request_stream failed to get next stream item: {:?}", e);
                break;
            }
        }
    }

    scope.join().await;
    Ok(())
}

async fn run_offers_forward(
    handles: LocalComponentHandles,
    client: Arc<ClientEnd<fio::DirectoryMarker>>,
) -> Result<(), Error> {
    let mut fs = ServiceFs::new();

    let (proxy, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
    client.as_ref_directory().open(
        "svc",
        fio::PERM_READABLE | fio::Flags::PROTOCOL_DIRECTORY,
        server_end.into_channel().into(),
    )?;
    fs.add_remote("svc", proxy);
    fs.serve_connection(handles.outgoing_dir)?;
    Ok(fs.collect::<()>().await)
}

async fn add_dynamic_expose_deprecated(
    expose: String,
    builder: &RealmBuilder,
    driver_test_realm_component_name: &str,
    driver_test_realm_ref: &Ref,
) -> Result<(), Error> {
    let expose_parsed =
        expose.parse::<cm_types::Name>().expect("service name is not a valid capability name");

    // Dynamic exposes are not hardcoded in the driver test realm cml so we have to add them here.
    // The source of this capability is the driver test realm itself because it has manually
    // forwarded all of the requested exposes from its inner realm.
    let mut decl = builder.get_component_decl(driver_test_realm_component_name).await?;
    push_box(
        &mut decl.capabilities,
        cm_rust::CapabilityDecl::Service(cm_rust::ServiceDecl {
            name: expose_parsed.clone(),
            source_path: Some(
                ("/svc/".to_owned() + &expose)
                    .parse::<cm_types::Path>()
                    .expect("service name is not a valid capability name"),
            ),
        }),
    );
    push_box(
        &mut decl.exposes,
        cm_rust::ExposeDecl::Service(cm_rust::ExposeServiceDecl {
            source: cm_rust::ExposeSource::Self_,
            source_name: expose_parsed.clone(),
            source_dictionary: Default::default(),
            target_name: expose_parsed.clone(),
            target: cm_rust::ExposeTarget::Parent,
            availability: cm_rust::Availability::Required,
        }),
    );
    builder.replace_component_decl(driver_test_realm_component_name, decl).await?;

    // Add the route through the realm builder.
    builder
        .add_route(
            Route::new()
                .capability(Capability::service_by_name(&expose))
                .from(driver_test_realm_ref.clone())
                .to(Ref::parent()),
        )
        .await?;
    Ok(())
}

async fn add_dynamic_offer_deprecated(
    offer: String,
    builder: &RealmBuilder,
    offer_provider_ref: &ChildRef,
    driver_test_realm_component_name: &str,
    driver_test_realm_ref: &Ref,
) -> Result<(), Error> {
    let offer_parsed =
        offer.parse::<cm_types::Name>().expect("offer name is not a valid capability name");

    // Dynamic offers are not hardcoded in the driver test realm cml so we have to add them here.
    // The target of this capability is the realm_builder collection which is where the
    // driver_test_realm's own realm builder will spin up the driver framework and drivers.
    let mut decl = builder.get_component_decl(driver_test_realm_component_name).await?;
    push_box(
        &mut decl.offers,
        cm_rust::OfferDecl::Protocol(cm_rust::OfferProtocolDecl {
            source: cm_rust::OfferSource::Parent,
            source_name: offer_parsed.clone(),
            source_dictionary: Default::default(),
            target_name: offer_parsed.clone(),
            target: cm_rust::OfferTarget::Collection(
                "realm_builder".parse::<cm_types::Name>().unwrap(),
            ),
            dependency_type: cm_rust::DependencyType::Strong,
            availability: cm_rust::Availability::Required,
        }),
    );
    builder.replace_component_decl(driver_test_realm_component_name, decl).await?;

    // Add the route through the realm builder.
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name(&offer))
                .from(offer_provider_ref)
                .to(driver_test_realm_ref.clone()),
        )
        .await?;
    Ok(())
}

async fn add_capabilities(
    builder: &RealmBuilder,
    driver_test_realm_ref: Ref,
    driver_test_realm_start_args: &Option<fdt::RealmArgs>,
    offers_client: Option<ClientEnd<fio::DirectoryMarker>>,
    driver_test_realm_component_name: &str,
) -> Result<(), Error> {
    let deprecated_exposes =
        driver_test_realm_start_args.as_ref().and_then(|args| args.exposes.as_ref()).and_then(
            |exposes| Some(exposes.iter().map(|e| e.service_name.clone()).collect::<Vec<_>>()),
        );

    let exposes = driver_test_realm_start_args
        .as_ref()
        .and_then(|args| args.dtr_exposes.as_ref())
        .and_then(|exposes| Some(exposes.clone()));

    if let Some(deprecated_exposes) = deprecated_exposes {
        for deprecated_expose in deprecated_exposes {
            add_dynamic_expose_deprecated(
                deprecated_expose,
                builder,
                driver_test_realm_component_name,
                &driver_test_realm_ref,
            )
            .await?;
        }
    }

    if let Some(exposes) = exposes {
        builder.driver_test_realm_add_dtr_exposes(&exposes).await?;
    }

    let deprecated_offers =
        driver_test_realm_start_args.as_ref().and_then(|args| args.offers.as_ref()).and_then(
            |offers| Some(offers.iter().map(|o| o.protocol_name.clone()).collect::<Vec<_>>()),
        );
    let offers = driver_test_realm_start_args
        .as_ref()
        .and_then(|args| args.dtr_offers.as_ref())
        .and_then(|offers| Some(offers.clone()));

    if deprecated_offers.is_some() || offers.is_some() {
        let client = offers_client.expect("Offers provided without an offers_client.");
        let arc_client = Arc::new(client);
        let provider = builder
            .add_local_child(
                "offer_provider",
                move |handles: LocalComponentHandles| {
                    run_offers_forward(handles, arc_client.clone()).boxed()
                },
                ChildOptions::new(),
            )
            .await?;
        if let Some(deprecated_offers) = deprecated_offers {
            for deprecated_offer in deprecated_offers {
                add_dynamic_offer_deprecated(
                    deprecated_offer,
                    builder,
                    &provider,
                    driver_test_realm_component_name,
                    &driver_test_realm_ref,
                )
                .await?;
            }
        }
        if let Some(offers) = offers {
            builder.driver_test_realm_add_dtr_offers(&offers, (&provider).into()).await?;
        }
    }

    Ok(())
}

async fn create_realm(options: RealmOptions) -> Result<RealmInstance, Error> {
    info!("building the realm using options {:?}", options);
    let driver_test_realm_component_name = "driver_test_realm";
    let url =
        options.driver_test_realm_url.unwrap_or_else(|| "#meta/driver_test_realm.cm".to_string());

    // Add the driver_test_realm child.
    let builder = RealmBuilder::new().await?;
    builder
        .driver_test_realm_manifest_setup(url.as_str(), fuchsia_driver_test::Options::default())
        .await?;
    let driver_test_realm_ref = Ref::child(fuchsia_driver_test::COMPONENT_NAME);
    // Adds the capabilities and routes we need for the realm.
    add_capabilities(
        &builder,
        driver_test_realm_ref,
        &options.driver_test_realm_start_args,
        options.offers_client,
        driver_test_realm_component_name,
    )
    .await?;

    // Build the realm.
    let realm = builder.build().await?;

    // Connect to the realm and start it.
    let start_args = options
        .driver_test_realm_start_args
        .expect("No driver_test_realm_start_args was provided.");
    realm.driver_test_realm_start(start_args).await?;
    // Connect dev-class.
    if let Some(dev_class) = options.dev_class {
        realm.root.get_exposed_dir().open(
            "dev-class",
            fidl_fuchsia_io::PERM_READABLE | fidl_fuchsia_io::Flags::PROTOCOL_DIRECTORY,
            &Default::default(),
            dev_class.into_channel(),
        )?;
    }

    // Connect dev-topological.
    if let Some(dev_topological) = options.dev_topological {
        realm.root.get_exposed_dir().open(
            "dev-topological",
            fidl_fuchsia_io::PERM_READABLE | fidl_fuchsia_io::Flags::PROTOCOL_DIRECTORY,
            &Default::default(),
            dev_topological.into_channel(),
        )?;
    }

    Ok(realm)
}
