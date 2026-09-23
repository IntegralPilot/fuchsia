// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fuchsia_component_test::RealmBuilder;
use fuchsia_driver_test::{DriverTestRealmBuilder, DriverTestRealmInstance};

#[fuchsia::test]
async fn test_devfs_exporter() -> Result<()> {
    // Create the RealmBuilder.
    let builder = RealmBuilder::new().await?;
    builder.driver_test_realm_setup().await?;
    // Build the Realm.
    let instance = builder.build().await?;
    // Start the DriverTestRealm.
    let args = fidl_fuchsia_driver_test::RealmArgs {
        root_driver: Some("#meta/root.cm".to_string()),
        ..Default::default()
    };
    instance.driver_test_realm_start(args).await?;
    // Connect to the driver.
    let dev = instance.driver_test_realm_connect_to_dev()?;
    let device = device_watcher::recursive_wait_and_open::<
        fidl_fuchsia_powermanager_root_test::DeviceMarker,
    >(&dev, "sys/platform")
    .await?;
    Ok(device.ping().await?)
}
