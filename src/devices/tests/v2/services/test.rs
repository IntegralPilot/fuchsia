// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use fidl_fuchsia_driver_test as fdt;
use fidl_fuchsia_services_test as ft;
use fuchsia_component::client;
use fuchsia_component_test::RealmBuilder;
use fuchsia_driver_test::{DriverTestRealmBuilder2, DriverTestRealmInstance2, Options2};

#[fuchsia::test]
async fn test_services() -> Result<()> {
    // Create the RealmBuilder.
    let builder = RealmBuilder::new().await?;

    let expose = fuchsia_component_test::Capability::service::<ft::DeviceMarker>().into();
    let exposes = vec![expose];
    let args =
        fdt::RealmArgs { root_driver: Some("#meta/root.cm".to_string()), ..Default::default() };

    builder.driver_test_realm_setup(Options2::new().driver_exposes(exposes), args).await?;
    // Build the Realm.
    let realm = builder.build().await?;
    realm.wait_for_bootup().await?;

    // Connect to the `Device` service.
    let device = client::Service::open_from_dir(realm.root.get_exposed_dir(), ft::DeviceMarker)
        .context("Failed to open service")?
        .watch_for_any()
        .await
        .context("Failed to find instance")?;
    // Use the `ControlPlane` protocol from the `Device` service.
    let control = device.connect_to_control()?;
    control.control_do().await?;
    // Use the `DataPlane` protocol from the `Device` service.
    let data = device.connect_to_data()?;
    data.data_do().await?;

    realm.destroy().await?;
    Ok(())
}

#[fuchsia::test]
async fn test_services_v1() -> Result<()> {
    // Create the RealmBuilder.
    let builder = RealmBuilder::new().await?;

    let expose = fuchsia_component_test::Capability::service::<ft::DeviceMarker>().into();
    let exposes = vec![expose];
    let args = fdt::RealmArgs {
        root_driver: Some("#meta/v1_driver.cm".to_string()),
        ..Default::default()
    };

    builder.driver_test_realm_setup(Options2::new().driver_exposes(exposes), args).await?;
    // Build the Realm.
    let realm = builder.build().await?;
    realm.wait_for_bootup().await?;

    // Connect to the `Device` service.
    let device = client::Service::open_from_dir(realm.root.get_exposed_dir(), ft::DeviceMarker)
        .context("Failed to open service")?
        .watch_for_any()
        .await
        .context("Failed to find instance")?;
    // Use the `ControlPlane` protocol from the `Device` service.
    let control = device.connect_to_control()?;
    control.control_do().await?;
    // Use the `DataPlane` protocol from the `Device` service.
    let data = device.connect_to_data()?;
    data.data_do().await?;

    realm.destroy().await?;
    Ok(())
}
