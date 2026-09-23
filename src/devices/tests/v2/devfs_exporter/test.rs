// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fidl_fuchsia_devfs_test as ft;
use fidl_fuchsia_driver_test as fdt;
use fuchsia_component_test::RealmBuilder;
use fuchsia_driver_test::{DriverTestRealmBuilder2, DriverTestRealmInstance2, Options2};

#[fuchsia::test]
async fn test_devfs_exporter() -> Result<()> {
    // Create the RealmBuilder.
    let builder = RealmBuilder::new().await?;
    builder
        .driver_test_realm_setup(
            Options2::default(),
            fdt::RealmArgs { root_driver: Some("#meta/root.cm".to_string()), ..Default::default() },
        )
        .await?;
    let instance = builder.build().await?;
    instance.wait_for_bootup().await?;
    // Connect to our driver.
    let dev = instance.driver_test_realm_connect_to_dev()?;
    let device =
        device_watcher::recursive_wait_and_open::<ft::DeviceMarker>(&dev, "root-device").await?;
    device.ping().await?;
    instance.destroy().await?;
    Ok(())
}
