// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fidl_fuchsia_driver_test as fdt;

use fuchsia_component_test::RealmBuilder;
use fuchsia_driver_test::{DriverTestRealmBuilder, DriverTestRealmInstance};

// [START example]
#[fuchsia::test]
async fn test_sample_driver() -> Result<()> {
    // Create the RealmBuilder.
    let builder = RealmBuilder::new().await?;
    builder.driver_test_realm_setup().await?;
    // Build the Realm.
    let instance = builder.build().await?;
    // Start DriverTestRealm
    instance.driver_test_realm_start(fdt::RealmArgs::default()).await?;

    // Connect to our driver.
    let dev = instance.driver_test_realm_connect_to_dev()?;
    let driver =
        device_watcher::recursive_wait_and_open::<fidl_fuchsia_hardware_sample::EchoMarker>(
            &dev,
            "sys/test/sample_driver",
        )
        .await?;

    // Call a FIDL method on the driver.
    let response = driver.echo_string("Hello world!").await.unwrap();

    // Verify the response.
    assert_eq!(response, "Hello world!");
    Ok(())
}

#[fuchsia::test]
async fn test_platform_bus() -> Result<()> {
    // Create the RealmBuilder.
    let builder = RealmBuilder::new().await?;
    builder.driver_test_realm_setup().await?;
    // Build the Realm.
    let instance = builder.build().await?;
    // Start DriverTestRealm.
    let args = fdt::RealmArgs {
        root_driver: Some("fuchsia-boot:///platform-bus#meta/platform-bus.cm".to_string()),
        ..Default::default()
    };
    instance.driver_test_realm_start(args).await?;
    // Connect to our driver.
    let dev = instance.driver_test_realm_connect_to_dev()?;
    device_watcher::recursive_wait(&dev, "sys/platform").await?;
    Ok(())
}
// [END example]
