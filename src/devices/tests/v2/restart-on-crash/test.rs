// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use fidl::endpoints::ServiceMarker;
use fidl_fuchsia_component_test as ftest;
use fidl_fuchsia_crashdriver_test as fcdt;
use fidl_fuchsia_driver_development as fdd;
use fidl_fuchsia_driver_test as fdt;
use fuchsia_async::{DurationExt, Timer};
use fuchsia_component::client;
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

async fn wait_for_instance(realm: &fuchsia_component_test::RealmInstance) -> Result<()> {
    let _ = client::Service::open_from_dir(realm.root.get_exposed_dir(), fcdt::DeviceMarker)
        .context("Failed to open service")?
        .watch_for_any()
        .await
        .context("Failed to wait for service instance")?;
    Ok(())
}

#[fuchsia::test]
async fn test_restart_on_crash() -> Result<()> {
    let args =
        fdt::RealmArgs { root_driver: Some("#meta/crasher.cm".to_string()), ..Default::default() };
    let exposes = vec![ftest::Capability::Service(ftest::Service {
        name: Some(fcdt::DeviceMarker::SERVICE_NAME.to_string()),
        ..Default::default()
    })];

    // Create the RealmBuilder.
    let builder = RealmBuilder::new().await?;

    builder.driver_test_realm_setup(Options2::new().driver_exposes(exposes), args).await?;

    // Build the Realm.
    let realm = builder.build().await?;
    realm.wait_for_bootup().await?;

    // Find an instance of the `Device` service.
    wait_for_instance(&realm).await?;

    let driver_dev = realm.root.connect_to_protocol_at_exposed_dir()?;
    let device_infos = get_device_info(&driver_dev, &[], /* exact_match= */ true).await?;
    assert_eq!(1, device_infos.len());
    let driver_host_koid_1 = device_infos[0].driver_host_koid;

    // Connect to the `Device` service.
    let crasher = client::Service::open_from_dir(realm.root.get_exposed_dir(), fcdt::DeviceMarker)
        .context("Failed to open service")?
        .watch_for_any()
        .await
        .context("Failed to wait for service instance")?
        .connect_to_crasher()?;
    let pong_1 = crasher.ping().await?;

    // CRASH
    crasher.crash()?;

    // Wait until the node comes back with a new host.
    let mut driver_host_koid_2: Option<u64>;
    loop {
        let device_infos = get_device_info(&driver_dev, &[], /* exact_match= */ true).await?;
        assert_eq!(1, device_infos.len());
        driver_host_koid_2 = device_infos[0].driver_host_koid;
        if driver_host_koid_2.is_some() && driver_host_koid_2 != driver_host_koid_1 {
            break;
        }
        Timer::new(zx::MonotonicDuration::from_millis(100).after_now()).await;
    }

    assert_ne!(driver_host_koid_1, driver_host_koid_2);

    // Connect to the new one.
    let crasher = client::Service::open_from_dir(realm.root.get_exposed_dir(), fcdt::DeviceMarker)
        .context("Failed to open service")?
        .watch_for_any()
        .await
        .context("Failed to wait for service instance")?
        .connect_to_crasher()?;

    // Check that it is able to communicate with the new one.
    let pong_2 = crasher.ping().await?;
    assert_ne!(pong_1, pong_2);
    realm.destroy().await?;
    Ok(())
}
