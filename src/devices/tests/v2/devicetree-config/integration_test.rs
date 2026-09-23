// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fidl_fuchsia_driver_test as fdt;
use fuchsia_component_test::RealmBuilder;
use fuchsia_driver_test::{DriverTestRealmBuilder2, DriverTestRealmInstance2, Options2};

#[fuchsia::test]
async fn test_devicetree_config() -> Result<()> {
    // Create the RealmBuilder.
    let builder = RealmBuilder::new().await?;

    let args = fdt::RealmArgs {
        root_driver: Some("#meta/board_driver.cm".to_string()),
        ..Default::default()
    };
    builder.driver_test_realm_setup(Options2::default(), args).await?;
    // Build the Realm.
    let instance = builder.build().await?;
    instance.wait_for_bootup().await?;

    // Connect to devfs.
    let dev = instance.driver_test_realm_connect_to_dev()?;

    // Wait for the child device to appear.
    // Path is relative to /dev.
    device_watcher::recursive_wait(&dev, "devicetree-config-child").await?;

    println!("Integration test passed: Device appeared!");

    instance.destroy().await?;
    Ok(())
}
