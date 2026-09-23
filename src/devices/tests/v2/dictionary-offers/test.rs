// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, format_err};
use fidl::endpoints::ServiceMarker;
use fuchsia_component::client::Service;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{
    Capability, ChildOptions, LocalComponentHandles, RealmBuilder, Ref, Route,
};
use fuchsia_driver_test::{DriverTestRealmBuilder2, DriverTestRealmInstance2, Options2};
use fuchsia_sync::Mutex;
use futures::TryStreamExt;
use futures::channel::mpsc;
use futures::stream::StreamExt;
use std::sync::Arc;

use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_dictionaryoffers_test as ft;
use fidl_fuchsia_driver_framework as fdf;
use fidl_fuchsia_driver_test as fdt;
use fuchsia_async as fasync;

async fn data_plane_serve(
    mut stream: ft::DataPlaneRequestStream,
    sender: Arc<Mutex<mpsc::Sender<()>>>,
) {
    while let Some(ft::DataPlaneRequest::DataDo { responder }) =
        stream.try_next().await.expect("Stream failed")
    {
        responder.send().unwrap();
        sender.lock().try_send(()).expect("Sender failed")
    }
}

async fn data_plane_component(
    handles: LocalComponentHandles,
    sender: Arc<Mutex<mpsc::Sender<()>>>,
) -> Result<()> {
    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service_instance("non-default", move |i: ft::DataServiceRequest| {
        let ft::DataServiceRequest::Data(request_stream) = i;
        fasync::Task::spawn(data_plane_serve(request_stream, sender.clone())).detach()
    });
    fs.serve_connection(handles.outgoing_dir)?;
    Ok(fs.collect::<()>().await)
}

#[fuchsia::test]
async fn test_dictionary_offers() -> Result<()> {
    let builder = RealmBuilder::new().await?;

    // A channel capacity > 1 is required because multiple child drivers (e.g. non-composite
    // and composite) asynchronously connect and call `DataDo` in rapid succession before
    // the test awaits `receiver.next()`. Since `data_plane_serve` uses `try_send()`, a buffer
    // of 1 would cause `try_send` to fail if multiple requests arrive before the first is polled.
    let (sender, mut receiver) = mpsc::channel(10);
    let sender = Arc::new(Mutex::new(sender));

    let data_plane_ref = builder
        .add_local_child(
            "data_plane_component",
            move |handles: LocalComponentHandles| {
                Box::pin(data_plane_component(handles, sender.clone()))
            },
            ChildOptions::new().eager(),
        )
        .await?;

    // Route the service so the output dictionary contains it.
    builder
        .add_route(
            Route::new()
                .capability(Capability::service::<ft::DataServiceMarker>())
                .from(&data_plane_ref)
                .to(Ref::parent()),
        )
        .await?;

    // Route the realm so we can connect to this and get the output dictionary using
    // get_child_output_dictionary.
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fcomponent::RealmMarker>())
                .from(Ref::framework())
                .to(Ref::parent()),
        )
        .await?;

    let args =
        fdt::RealmArgs { root_driver: Some("#meta/root.cm".to_string()), ..Default::default() };
    let expose = Capability::service::<ft::ControlServiceMarker>().into();
    let exposes = vec![expose];

    builder.driver_test_realm_setup(Options2::new().driver_exposes(exposes), args).await?;
    let realm = builder.build().await?;
    realm.wait_for_bootup().await?;

    let device = Service::open_from_dir(realm.root.get_exposed_dir(), ft::ControlServiceMarker)
        .context("Failed to open service")?
        .watch_for_any()
        .await
        .context("Failed to find instance")?;

    let control = device.connect_to_control()?;

    let realm_proxy: fcomponent::RealmProxy = realm.root.connect_to_protocol_at_exposed_dir()?;
    let child_ref = fdecl::ChildRef { name: "data_plane_component".to_string(), collection: None };

    // This node matches the child.bind
    {
        let dictionary_ref = realm_proxy
            .get_child_output_dictionary_deprecated(&child_ref)
            .await
            .map_err(|e| format_err!("Failed to call get child output dictionary: {:?}", e))?
            .map_err(|e| format_err!("Failed to get child output dictionary: {:?}", e))?;

        let args = fdf::NodeAddArgs {
            name: Some("test".to_string()),
            offers_dictionary: Some(dictionary_ref),
            offers2: Some(vec![fdf::Offer::DictionaryOffer(fdecl::Offer::Service(
                fdecl::OfferService {
                    source_name: Some(ft::DataServiceMarker::SERVICE_NAME.to_string()),
                    target_name: Some(ft::DataServiceMarker::SERVICE_NAME.to_string()),
                    source_instance_filter: Some(vec!["default".to_string()]),
                    renamed_instances: Some(vec![fdecl::NameMapping {
                        source_name: "non-default".to_string(),
                        target_name: "default".to_string(),
                    }]),
                    ..Default::default()
                },
            ))]),
            ..Default::default()
        };

        control
            .add_child(args)
            .await
            .map_err(|e| format_err!("Failed to call add_child: {:?}", e))?
            .map_err(|e| format_err!("Failed to add_child: {:?}", e))?;
    }

    // This node matches the left node in the composite created by the root driver.
    // This is not the primary node in the composite-child.bind, but its the node that carries
    // the offers_dictionary.
    {
        let dictionary_ref = realm_proxy
            .get_child_output_dictionary_deprecated(&child_ref)
            .await
            .map_err(|e| format_err!("Failed to call get child output dictionary: {:?}", e))?
            .map_err(|e| format_err!("Failed to get child output dictionary: {:?}", e))?;

        let args = fdf::NodeAddArgs {
            name: Some("left".to_string()),
            offers_dictionary: Some(dictionary_ref),
            properties2: Some(vec![fdf::NodeProperty2 {
                key: bind_fuchsia_nodegroupbind_test::TEST_BIND_PROPERTY.to_string(),
                value: fdf::NodePropertyValue::StringValue(
                    bind_fuchsia_nodegroupbind_test::TEST_BIND_PROPERTY_ONE_LEFT.to_string(),
                ),
            }]),
            offers2: Some(vec![fdf::Offer::DictionaryOffer(fdecl::Offer::Service(
                fdecl::OfferService {
                    source_name: Some(ft::DataServiceMarker::SERVICE_NAME.to_string()),
                    target_name: Some(ft::DataServiceMarker::SERVICE_NAME.to_string()),
                    source_instance_filter: Some(vec!["default".to_string()]),
                    renamed_instances: Some(vec![fdecl::NameMapping {
                        source_name: "non-default".to_string(),
                        target_name: "default".to_string(),
                    }]),
                    ..Default::default()
                },
            ))]),
            ..Default::default()
        };

        control
            .add_child(args)
            .await
            .map_err(|e| format_err!("Failed to call add_child: {:?}", e))?
            .map_err(|e| format_err!("Failed to add_child: {:?}", e))?;
    }

    // This node matches the optional node in the composite created by the root driver.
    // This carries another dictionary offer of the same service.
    {
        let dictionary_ref = realm_proxy
            .get_child_output_dictionary_deprecated(&child_ref)
            .await
            .map_err(|e| format_err!("Failed to call get child output dictionary: {:?}", e))?
            .map_err(|e| format_err!("Failed to get child output dictionary: {:?}", e))?;

        let args = fdf::NodeAddArgs {
            name: Some("optional".to_string()),
            offers_dictionary: Some(dictionary_ref),
            properties2: Some(vec![fdf::NodeProperty2 {
                key: bind_fuchsia_nodegroupbind_test::TEST_BIND_PROPERTY.to_string(),
                value: fdf::NodePropertyValue::StringValue(
                    bind_fuchsia_nodegroupbind_test::TEST_BIND_PROPERTY_FOUR_OPTIONAL.to_string(),
                ),
            }]),
            offers2: Some(vec![fdf::Offer::DictionaryOffer(fdecl::Offer::Service(
                fdecl::OfferService {
                    source_name: Some(ft::DataServiceMarker::SERVICE_NAME.to_string()),
                    target_name: Some(ft::DataServiceMarker::SERVICE_NAME.to_string()),
                    source_instance_filter: Some(vec!["default".to_string()]),
                    renamed_instances: Some(vec![fdecl::NameMapping {
                        source_name: "non-default".to_string(),
                        target_name: "default".to_string(),
                    }]),
                    ..Default::default()
                },
            ))]),
            ..Default::default()
        };

        control
            .add_child(args)
            .await
            .map_err(|e| format_err!("Failed to call add_child: {:?}", e))?
            .map_err(|e| format_err!("Failed to add_child: {:?}", e))?;
    }

    // This node matches the right node in the composite created by the root driver.
    // Its the primary node in the composite-child.bind
    {
        let args = fdf::NodeAddArgs {
            name: Some("right".to_string()),
            properties2: Some(vec![fdf::NodeProperty2 {
                key: bind_fuchsia_nodegroupbind_test::TEST_BIND_PROPERTY.to_string(),
                value: fdf::NodePropertyValue::StringValue(
                    bind_fuchsia_nodegroupbind_test::TEST_BIND_PROPERTY_ONE_RIGHT.to_string(),
                ),
            }]),
            ..Default::default()
        };

        control
            .add_child(args)
            .await
            .map_err(|e| format_err!("Failed to call add_child: {:?}", e))?
            .map_err(|e| format_err!("Failed to add_child: {:?}", e))?;
    }

    // One is the non-composite, and the other is the composite.
    receiver.next().await;
    receiver.next().await;

    realm.destroy().await?;
    Ok(())
}
