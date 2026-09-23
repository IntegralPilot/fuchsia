// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, anyhow};
use fidl_fuchsia_driver_development as fdd;
use fidl_fuchsia_driver_registrar as fdr;
use fidl_fuchsia_driver_test as fdt;
use fuchsia_component_test::RealmBuilder;
use fuchsia_driver_test::{DriverTestRealmBuilder2, DriverTestRealmInstance2, Options2};
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
async fn test_replace_failed_driver() -> Result<()> {
    let node_filter = ["dev.sys.test".to_string()];
    let failing_url = "fuchsia-boot:///dtr#meta/fail-to-start.cm";
    let replacemnt_url =
        "fuchsia-pkg://fuchsia.com/fail-to-start-replacement#meta/fail-to-start-replacement.cm";

    // Create the RealmBuilder.
    let builder = RealmBuilder::new().await?;

    let args = fdt::RealmArgs {
        driver_disable: Some(vec![
            // Keep the replacement disabled until we disable the original.
            "fuchsia-boot:///dtr#meta/fail-to-start-replacement.cm".to_string(),
        ]),
        ..Default::default()
    };
    builder.driver_test_realm_setup(Options2::default(), args).await?;

    // Build the Realm.
    let instance = builder.build().await?;
    instance.wait_for_bootup().await?;

    let driver_dev = instance.root.connect_to_protocol_at_exposed_dir()?;
    let driver_registrar: fdr::DriverRegistrarProxy =
        instance.root.connect_to_protocol_at_exposed_dir()?;

    // Check that it's quarantined.
    let device_infos = get_device_info(&driver_dev, &node_filter, /* exact_match= */ true).await?;
    assert_eq!(Some(true), device_infos.first().expect("one node entry").quarantined);
    assert_eq!(
        Some(failing_url.to_string()),
        device_infos.first().expect("one node entry").bound_driver_url
    );

    // Let's disable the failed  driver.
    let disable_result = driver_dev.disable_driver(&failing_url, None).await;
    if disable_result.is_err() {
        return Err(anyhow!("Failed to disable failing_url."));
    }
    // Now we can restart the first target driver with the rematch flag.
    let restart_result =
        driver_dev.restart_driver_hosts(failing_url, fdd::RestartRematchFlags::REQUESTED).await?;
    if restart_result.is_err() {
        return Err(anyhow!("Failed to restart target_1."));
    }

    // Wait until the node is unbound.
    loop {
        let device_infos =
            get_device_info(&driver_dev, &node_filter, /* exact_match= */ true).await?;
        match device_infos.first().expect("one node entry").bound_driver_url.as_deref() {
            Some("unbound") => break,
            _ => {}
        }
    }

    // Now we can register our replacement.
    let register_result = driver_registrar.register(replacemnt_url).await;
    match register_result {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            return Err(anyhow!("Failed to register replacement: {}.", err));
        }
        Err(err) => {
            return Err(anyhow!("Failed to register replacement: {}.", err));
        }
    };

    // And now that we have registered the replacement we call to bind all available nodes.
    let bind_result = driver_dev.bind_all_unbound_nodes2().await;
    match bind_result {
        Ok(Ok(res)) => {
            let binding = res.first().expect("one binding event");
            assert_eq!(Some("dev.sys.test".to_string()), binding.node_name);
            assert_eq!(Some(replacemnt_url.to_string()), binding.driver_url);
        }
        Ok(Err(err)) => {
            return Err(anyhow!("Failed to bind_all_unbound_nodes: {}.", err));
        }
        Err(err) => {
            return Err(anyhow!("Failed to bind_all_unbound_nodes: {}.", err));
        }
    };

    // Wait until the host koid is not None, which means the replacement is up.
    loop {
        let device_infos =
            get_device_info(&driver_dev, &node_filter, /* exact_match= */ true).await?;
        let node = device_infos.first().expect("one node entry");
        if node.driver_host_koid.is_some() {
            // Ensure its no longer quarantined.
            assert_eq!(Some(false), node.quarantined);
            assert_eq!(Some(replacemnt_url.to_string()), node.bound_driver_url);
            break;
        }
    }

    instance.destroy().await?;
    Ok(())
}
