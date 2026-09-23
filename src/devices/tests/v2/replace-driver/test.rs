// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error, Result, anyhow};
use fidl_fuchsia_driver_development as fdd;
use fidl_fuchsia_driver_registrar as fdr;
use fidl_fuchsia_driver_test as fdt;
use fidl_fuchsia_reloaddriver_test as ft;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{ChildOptions, LocalComponentHandles, RealmBuilder};
use fuchsia_driver_test::{DriverTestRealmBuilder2, DriverTestRealmInstance2, Options2};
use futures::channel::mpsc;
use futures::{StreamExt, TryStreamExt};
use std::collections::{HashMap, HashSet};

const WAITER_NAME: &'static str = "waiter";

async fn waiter_serve(
    mut stream: ft::WaiterRequestStream,
    mut sender: mpsc::Sender<(String, String)>,
) {
    while let Some(ft::WaiterRequest::Ack { from_node, from_name, status, .. }) =
        stream.try_next().await.expect("Stream failed")
    {
        assert_eq!(status, zx::sys::ZX_OK);
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
async fn test_replace_target() -> Result<()> {
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
    let offer = fuchsia_component_test::Capability::protocol::<ft::WaiterMarker>().into();
    let dtr_offers = vec![offer];

    let args = fdt::RealmArgs {
        root_driver: Some("fuchsia-boot:///dtr#meta/root.cm".to_string()),
        driver_disable: Some(vec![
            "fuchsia-boot:///dtr#meta/target_2_replacement.cm".to_string(),
            "fuchsia-boot:///dtr#meta/composite_replacement.cm".to_string(),
        ]),
        ..Default::default()
    };

    builder
        .driver_test_realm_setup(Options2::new().driver_offers((&waiter).into(), dtr_offers), args)
        .await?;
    // Build the Realm.
    let instance = builder.build().await?;

    // Start the DriverTestRealm.
    // The drivers listed in driver_disable are unavailable at first, but when they go through
    // the register flow, they will be available as ephemeral drivers.

    instance.wait_for_bootup().await?;

    let driver_dev = instance.root.connect_to_protocol_at_exposed_dir()?;
    let driver_registrar: fdr::DriverRegistrarProxy =
        instance.root.connect_to_protocol_at_exposed_dir()?;

    // This maps nodes to Option<Option<u64>>. The outer option is whether the node has been seen
    // yet (if composite parent we start with `Some` for this since we don't receive acks
    // from them). The inner option is the driver host koid.
    let mut nodes = HashMap::from([
        ("dev".to_string(), None),
        ("B".to_string(), None),
        ("C".to_string(), None),
        ("D".to_string(), Some(None)), // composite parent
        ("E".to_string(), Some(None)), // composite parent
        ("F".to_string(), Some(None)), // composite parent
        ("G".to_string(), None),
        ("H".to_string(), None),
        ("I".to_string(), None),
        ("J".to_string(), None),
        ("K".to_string(), None),
    ]);

    // First we want to wait for all the nodes.
    reloadtest_tools::wait_for_nodes(&mut nodes, &mut receiver).await?;

    // Now we collect their initial driver host koids.
    let device_infos = get_device_info(&driver_dev, &[], /* exact_match= */ true).await?;
    reloadtest_tools::validate_host_koids("init", device_infos, &mut nodes, vec![], None).await?;

    // Let's disable the first target driver.
    let target_1_url = "fuchsia-boot:///dtr#meta/target_1_no_colocate.cm";
    let disable_result = driver_dev.disable_driver(&target_1_url, None).await;
    if disable_result.is_err() {
        return Err(anyhow!("Failed to disable target_1_no_colocate."));
    }
    // Now we can restart the first target driver with the rematch flag.
    let restart_result =
        driver_dev.restart_driver_hosts(target_1_url, fdd::RestartRematchFlags::REQUESTED).await?;
    if restart_result.is_err() {
        return Err(anyhow!("Failed to restart target_1."));
    }

    // These are the nodes that should be started.
    // 'G' is the node that was bound to our target.
    // 'Z' is the node that the replacement creates.
    let mut nodes_after_restart = HashMap::from([("G".to_string(), None), ("Z".to_string(), None)]);

    // Wait for them to start.
    reloadtest_tools::wait_for_nodes(&mut nodes_after_restart, &mut receiver).await?;

    // These nodes should not exist anymore.
    // 'I' was the child of the driver being replaced.
    let should_not_exist_after_restart = HashSet::from([("I".to_string())]);

    // Collect the new driver host koids.
    // Ensure same koid if not one of the ones expected to restart.
    // Make sure the host koid has changed from before the restart for the nodes that should have
    // restarted.
    let device_infos = get_device_info(&driver_dev, &[], /* exact_match= */ true).await?;
    reloadtest_tools::validate_host_koids(
        "first restart",
        device_infos,
        &mut nodes_after_restart,
        vec![&nodes],
        Some(&should_not_exist_after_restart),
    )
    .await?;

    // Now let's disable the second target driver.
    let target_2_url = "fuchsia-boot:///dtr#meta/target_2.cm";
    let disable_2_result = driver_dev.disable_driver(&target_2_url, None).await;
    if disable_2_result.is_err() {
        return Err(anyhow!("Failed to disable target_2."));
    }

    // Now we can restart the second target driver with the rematch flag.
    let restart_result =
        driver_dev.restart_driver_hosts(target_2_url, fdd::RestartRematchFlags::REQUESTED).await?;
    if restart_result.is_err() {
        return Err(anyhow!("Failed to restart target_2."));
    }

    // These are the nodes that should be restarted after the second restart.
    let mut nodes_after_restart_2 = HashMap::from([("H".to_string(), None)]);

    // Wait for them to come back again.
    reloadtest_tools::wait_for_nodes(&mut nodes_after_restart_2, &mut receiver).await?;

    // At this point we should have lost the following nodes as we have disabled the target driver
    // for 'J' and don't have a replacement driver yet.
    let should_not_exist_after_restart_2 = HashSet::from(["J".to_string(), "K".to_string()]);

    // Collect the newer driver host koids.
    // Ensure same koid if not one of the ones expected to restart (comparing to most recent one).
    // Make sure the host koid has changed from before the second restart for the nodes that should
    // have restarted.
    let device_infos = get_device_info(&driver_dev, &[], /* exact_match= */ true).await?;
    reloadtest_tools::validate_host_koids(
        "second restart",
        device_infos,
        &mut nodes_after_restart_2,
        vec![&nodes_after_restart, &nodes],
        Some(&should_not_exist_after_restart_2),
    )
    .await?;

    // Now we can register our target_2 replacement.
    let target_2_replacemnt_url =
        "fuchsia-pkg://fuchsia.com/target_2_replacement#meta/target_2_replacement.cm";
    let register_result = driver_registrar.register(target_2_replacemnt_url).await;
    match register_result {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            return Err(anyhow!("Failed to register target_2 replacement: {}.", err));
        }
        Err(err) => {
            return Err(anyhow!("Failed to register target_2 replacement: {}.", err));
        }
    };

    // And now that we have registered the replacement we call to bind all available nodes.
    let bind_result = driver_dev.bind_all_unbound_nodes2().await;
    match bind_result {
        Ok(Ok(_)) => {}
        Ok(Err(err)) => {
            return Err(anyhow!("Failed to bind_all_unbound_nodes: {}.", err));
        }
        Err(err) => {
            return Err(anyhow!("Failed to bind_all_unbound_nodes: {}.", err));
        }
    };

    // These are the nodes we should get started now that we have the replacement in for 2.
    let mut nodes_after_register =
        HashMap::from([("J".to_string(), None), ("Y".to_string(), None)]);

    // Wait for them to come up.
    reloadtest_tools::wait_for_nodes(&mut nodes_after_register, &mut receiver).await?;

    // These should not exist after our register call.
    let should_not_exist_after_register = HashSet::from(["K".to_string()]);

    // Collect the newer driver host koids.
    // Ensure same koid if not one of the ones expected to restart (comparing to most recent one).
    // Make sure the host koid has changed from before the register.
    let device_infos = get_device_info(&driver_dev, &[], /* exact_match= */ true).await?;
    reloadtest_tools::validate_host_koids(
        "register",
        device_infos,
        &mut nodes_after_register,
        vec![&nodes_after_restart_2, &nodes_after_restart, &nodes],
        Some(&should_not_exist_after_register),
    )
    .await?;

    // Now let's disable the composite driver.
    let composite_url = "fuchsia-boot:///dtr#meta/composite.cm";
    let disable_2_result = driver_dev.disable_driver(&composite_url, None).await;
    if disable_2_result.is_err() {
        return Err(anyhow!("Failed to disable composite."));
    }

    // Now we can restart the composite driver with the rematch flag.
    let restart_result = driver_dev
        .restart_driver_hosts(
            composite_url,
            fdd::RestartRematchFlags::REQUESTED | fdd::RestartRematchFlags::COMPOSITE_SPEC,
        )
        .await?;
    if restart_result.is_err() {
        return Err(anyhow!("Failed to restart composite."));
    }

    // There are no new nodes after restarting the composite.
    let mut nodes_after_restart_composite: HashMap<String, Option<Option<u64>>> = HashMap::new();

    // Wait until H (the composite node) goes away.
    loop {
        let device_infos = get_device_info(&driver_dev, &[], /* exact_match= */ true).await?;
        if !device_infos.iter().any(|info| {
            let name = info.moniker.clone().unwrap().split(".").last().unwrap().to_string();
            return name == "H".to_string();
        }) {
            break;
        }
    }

    // At this point we should have lost the following nodes as we have disabled the composite
    // driver for 'H' and don't have a replacement driver yet.
    let should_not_exist_after_restart_composite =
        HashSet::from(["H".to_string(), "J".to_string(), "K".to_string()]);

    // Run validations.
    let device_infos = get_device_info(&driver_dev, &[], /* exact_match= */ true).await?;
    reloadtest_tools::validate_host_koids(
        "composite restart",
        device_infos,
        &mut nodes_after_restart_composite,
        vec![&nodes_after_register, &nodes_after_restart_2, &nodes_after_restart, &nodes],
        Some(&should_not_exist_after_restart_composite),
    )
    .await?;

    // Now we can register our composite replacement.
    let composite_replacemnt_url =
        "fuchsia-pkg://fuchsia.com/composite_replacement#meta/composite_replacement.cm";
    let register_result = driver_registrar.register(composite_replacemnt_url).await;
    match register_result {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            return Err(anyhow!("Failed to register composite replacement: {}.", err));
        }
        Err(err) => {
            return Err(anyhow!("Failed to register composite replacement: {}.", err));
        }
    };

    // And now that we have registered the replacement we call to bind all available nodes.
    let bind_result = driver_dev.bind_all_unbound_nodes2().await;
    match bind_result {
        Ok(Ok(_)) => {}
        Ok(Err(err)) => {
            return Err(anyhow!("Failed to bind_all_unbound_nodes: {}.", err));
        }
        Err(err) => {
            return Err(anyhow!("Failed to bind_all_unbound_nodes: {}.", err));
        }
    };

    // These are the nodes we should get started now that we have the replacement in for the composite.
    let mut nodes_after_register_composite = HashMap::from([
        ("H".to_string(), None),
        ("J_replaced".to_string(), None),
        ("Y".to_string(), None),
    ]);

    // Wait for them to come up.
    reloadtest_tools::wait_for_nodes(&mut nodes_after_register_composite, &mut receiver).await?;

    // These should not exist after our register call for the composite replacement.
    let should_not_exist_after_register_composite =
        HashSet::from(["K".to_string(), "J".to_string(), "H".to_string()]);

    // Collect the newer driver host koids.
    // Ensure same koid if not one of the ones expected to restart (comparing to most recent one).
    // Make sure the host koid has changed from before the register.
    let device_infos = get_device_info(&driver_dev, &[], /* exact_match= */ true).await?;
    reloadtest_tools::validate_host_koids(
        "register composite",
        device_infos,
        &mut nodes_after_register_composite,
        vec![
            &nodes_after_restart_composite,
            &nodes_after_register,
            &nodes_after_restart_2,
            &nodes_after_restart,
            &nodes,
        ],
        Some(&should_not_exist_after_register_composite),
    )
    .await?;

    instance.destroy().await?;
    Ok(())
}
