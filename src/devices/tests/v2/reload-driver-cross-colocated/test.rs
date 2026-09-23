// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error, Result};
use fidl_fuchsia_driver_development as fdd;
use fidl_fuchsia_driver_test as fdt;
use fidl_fuchsia_reloaddriver_test as ft;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{ChildOptions, LocalComponentHandles, RealmBuilder};
use fuchsia_driver_test::{DriverTestRealmBuilder2, DriverTestRealmInstance2, Options2};
use futures::channel::mpsc;
use futures::{StreamExt, TryStreamExt};
use reloadtest_tools;
use std::collections::HashMap;

const WAITER_NAME: &'static str = "waiter";

async fn waiter_serve(
    mut stream: ft::WaiterRequestStream,
    mut sender: mpsc::Sender<(String, String)>,
) {
    while let Some(ft::WaiterRequest::Ack { from_node, from_name, status, .. }) =
        stream.try_next().await.expect("Stream failed")
    {
        assert_eq!(zx::Status::ok(status), Ok(()));
        sender.try_send((from_node, from_name)).expect("Sender failed")
    }
}

async fn waiter_component(
    handles: LocalComponentHandles,
    sender: mpsc::Sender<(String, String)>,
) -> Result<(), Error> {
    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(move |stream: ft::WaiterRequestStream| {
        fasync::Task::spawn(waiter_serve(stream, sender.clone())).detach()
    });
    fs.serve_connection(handles.outgoing_dir)?;
    Ok(fs.collect::<()>().await)
}

fn send_get_device_info_request(
    service: &fdd::ManagerProxy,
    device_filter: &[String],
    exact_match: bool,
) -> Result<fdd::NodeInfoIteratorProxy> {
    let (iterator, iterator_server) =
        fidl::endpoints::create_proxy::<fdd::NodeInfoIteratorMarker>();

    service
        .get_node_info(device_filter, iterator_server, exact_match)
        .context("FIDL call to get device info failed")?;

    Ok(iterator)
}

async fn get_device_info(
    service: &fdd::ManagerProxy,
    device_filter: &[String],
    exact_match: bool,
) -> Result<Vec<fdd::NodeInfo>> {
    let iterator = send_get_device_info_request(service, device_filter, exact_match)?;

    let mut device_infos = Vec::new();
    loop {
        let mut device_info =
            iterator.get_next().await.context("FIDL call to get device info failed")?;
        if device_info.len() == 0 {
            break;
        }
        device_infos.append(&mut device_info);
    }
    Ok(device_infos)
}

#[fuchsia::test]
async fn test_reload_cross_colocated_target() -> Result<()> {
    let (sender, mut receiver) = mpsc::channel(1);

    // Create the RealmBuilder.
    let builder = RealmBuilder::new().await?;
    let waiter = builder
        .add_local_child(
            WAITER_NAME,
            move |handles: LocalComponentHandles| {
                Box::pin(waiter_component(handles, sender.clone()))
            },
            ChildOptions::new(),
        )
        .await?;

    let args = fdt::RealmArgs {
        root_driver: Some("fuchsia-boot:///dtr#meta/root.cm".to_string()),
        ..Default::default()
    };
    let offer = fuchsia_component_test::Capability::protocol::<ft::WaiterMarker>().into();
    let offers = vec![offer];
    builder
        .driver_test_realm_setup(Options2::new().driver_offers((&waiter).into(), offers), args)
        .await?;
    let instance = builder.build().await?;
    instance.wait_for_bootup().await?;

    let driver_dev = instance.root.connect_to_protocol_at_exposed_dir()?;

    // Map of node names to Option<Option<u64>>.
    // The outer option is whether the node has been seen (nodes with bound drivers start with
    // `None` and wait for an Ack from their driver; intermediate composite parents start with
    // `Some` because they do not have bound drivers and do not send Acks).
    // The inner option is the driver host KOID populated by validate_host_koids.
    let mut nodes = HashMap::from([
        ("dev".to_string(), None),
        ("left_parent".to_string(), None),
        ("right_parent".to_string(), None),
        ("child_a".to_string(), None),
        ("child_b".to_string(), None),
        ("right_child".to_string(), Some(None)), // composite parent, sends no ack
        ("child_a_sub".to_string(), Some(None)), // composite parent, sends no ack
    ]);

    // Wait for all initial nodes to report their acks.
    reloadtest_tools::wait_for_nodes(&mut nodes, &mut receiver).await?;

    // Collect initial driver host koids.
    let device_infos = get_device_info(&driver_dev, &[], /* exact_match= */ true).await?;
    reloadtest_tools::validate_host_koids("init", device_infos, &mut nodes, vec![], None).await?;

    // Verify that child_a and child_b share the same driver host KOID initially.
    let koid_child_a = nodes.get("child_a").unwrap().unwrap();
    let koid_child_b = nodes.get("child_b").unwrap().unwrap();
    assert!(koid_child_a.is_some(), "child_a must have a valid host KOID");
    assert_eq!(koid_child_a, koid_child_b, "child_a and child_b must share the same host KOID");

    let koid_left = nodes.get("left_parent").unwrap().unwrap();
    let koid_right = nodes.get("right_parent").unwrap().unwrap();
    assert_ne!(koid_child_a, koid_left, "child_a host KOID must differ from left_parent");
    assert_ne!(koid_child_a, koid_right, "child_a host KOID must differ from right_parent");

    // Restart the cross-colocated target driver.
    let restart_result = driver_dev
        .restart_driver_hosts(
            "fuchsia-boot:///dtr#meta/target.cm",
            fdd::RestartRematchFlags::empty(),
        )
        .await?;
    assert_eq!(restart_result, Ok(1));

    // Nodes that should restart
    let mut nodes_after_restart = HashMap::from([
        ("child_a".to_string(), None),
        ("child_b".to_string(), None),
        ("child_a_sub".to_string(), Some(None)), // composite parent, sends no ack
    ]);

    // Wait for restarted nodes to send acks.
    reloadtest_tools::wait_for_nodes(&mut nodes_after_restart, &mut receiver).await?;

    // Collect host KOIDs after restart.
    let device_infos_after = get_device_info(&driver_dev, &[], /* exact_match= */ true).await?;
    reloadtest_tools::validate_host_koids(
        "cross-colocated restart",
        device_infos_after,
        &mut nodes_after_restart,
        vec![&nodes],
        None,
    )
    .await?;

    // Verify that child_a and child_b share the NEW driver host KOID.
    let new_koid_child_a = nodes_after_restart.get("child_a").unwrap().unwrap();
    let new_koid_child_b = nodes_after_restart.get("child_b").unwrap().unwrap();
    assert!(new_koid_child_a.is_some());
    assert_eq!(
        new_koid_child_a, new_koid_child_b,
        "child_a and child_b must share the new host KOID"
    );
    assert_ne!(new_koid_child_a, koid_child_a, "child_a host KOID must change after restart");

    instance.destroy().await?;
    Ok(())
}
