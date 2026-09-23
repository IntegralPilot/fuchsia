// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, anyhow};
use fidl::endpoints::create_endpoints;
use fidl_fuchsia_driver_development::ManagerProxy;
use fidl_fuchsia_driver_framework::{NodePropertyKey, NodePropertyValue};
use fidl_fuchsia_driver_test::RealmArgs;
use fidl_fuchsia_interconnect_test as ft;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{ChildOptions, LocalComponentHandles, RealmBuilder};
use fuchsia_driver_test::{DriverTestRealmBuilder, DriverTestRealmInstance};
use futures::channel::mpsc;
use futures::{StreamExt, TryStreamExt};

const WAITER_NAME: &'static str = "waiter";

async fn waiter_serve(mut stream: ft::WaiterRequestStream, mut sender: mpsc::Sender<()>) {
    while let Some(ft::WaiterRequest::Ack { .. }) = stream.try_next().await.expect("Stream failed")
    {
        sender.try_send(()).expect("Sender failed")
    }
}

async fn waiter_component(handles: LocalComponentHandles, sender: mpsc::Sender<()>) -> Result<()> {
    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(move |stream: ft::WaiterRequestStream| {
        fasync::Task::spawn(waiter_serve(stream, sender.clone())).detach()
    });
    fs.serve_connection(handles.outgoing_dir)?;
    Ok(fs.collect::<()>().await)
}

#[fuchsia::test]
async fn test_interconnect_driver() -> Result<()> {
    // Create the RealmBuilder.
    let builder = RealmBuilder::new().await?;
    builder.driver_test_realm_setup().await?;

    let (sender, mut receiver) = mpsc::channel(1);
    let waiter = builder
        .add_local_child(
            WAITER_NAME,
            move |handles: LocalComponentHandles| {
                Box::pin(waiter_component(handles, sender.clone()))
            },
            ChildOptions::new(),
        )
        .await?;

    let offer = fuchsia_component_test::Capability::protocol::<ft::WaiterMarker>().into();
    let dtr_offers = vec![offer];

    builder.driver_test_realm_add_dtr_offers(&dtr_offers, (&waiter).into()).await?;

    // Build the Realm.
    let instance = builder.build().await?;
    // Start DriverTestRealm
    instance
        .driver_test_realm_start(RealmArgs {
            root_driver: Some("#meta/fake_interconnect.cm".to_owned()),
            dtr_offers: Some(dtr_offers),
            ..Default::default()
        })
        .await?;

    let manager: ManagerProxy = instance.root.connect_to_protocol_at_exposed_dir()?;
    let (node_iter, node_iter_server) = create_endpoints();
    manager.get_node_info(
        &[
            "dev.fake_interconnect.path_a-0".to_owned(),
            "dev.fake_interconnect.path_b-1".to_owned(),
            "dev.fake_interconnect.path_c-2".to_owned(),
        ],
        node_iter_server,
        true,
    )?;
    let node_iter = node_iter.into_proxy();
    let nodes = node_iter.get_next().await?;
    if nodes.len() != 3 {
        panic!("Didn't find all 3 paths");
    }

    let expected_props = [0, 1, 2];
    for (node, expected_prop) in nodes.iter().zip(&expected_props) {
        let expected_key = NodePropertyKey::StringValue(bind_fuchsia::ID.to_owned());
        let expected_value = NodePropertyValue::IntValue(*expected_prop);
        let prop_found = node
            .node_property_list
            .as_ref()
            .expect("node property list to be filled in")
            .iter()
            .any(|prop| prop.key == expected_key && prop.value == expected_value);
        assert!(prop_found);
    }

    // Wait for the driver to call Waiter.Done, which only happens after SetNodesBandwidth is
    // triggered in response to sync state.
    receiver.next().await.ok_or_else(|| anyhow!("Receiver failed"))
}
