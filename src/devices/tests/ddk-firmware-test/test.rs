// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fuchsia_driver_test as fdt;
use fuchsia_component_test::RealmBuilder;
use fuchsia_driver_test::{DriverTestRealmBuilder, DriverTestRealmInstance};

#[fuchsia::test]
async fn load_package_firmware_test() -> Result<(), Error> {
    // Create the RealmBuilder.
    let builder = RealmBuilder::new().await?;
    builder.driver_test_realm_setup().await?;
    let instance = builder.build().await?;

    // Start DriverTestRealm
    let args = fdt::RealmArgs {
        root_driver: Some("fuchsia-boot:///dtr#meta/test-parent-sys.cm".to_string()),
        ..Default::default()
    };
    instance.driver_test_realm_start(args).await?;

    // Connect to our driver.
    let dev = instance.driver_test_realm_connect_to_dev()?;
    let driver_proxy = device_watcher::recursive_wait_and_open::<
        fidl_fuchsia_device_firmware_test::TestDeviceMarker,
    >(&dev, "sys/test/ddk-firmware-test-device-0")
    .await?;

    // Check that we can load firmware from our package.
    driver_proxy.load_firmware("test-firmware").await?.unwrap();

    // Check that loading unknown name fails.
    assert_eq!(driver_proxy.load_firmware("test-bad").await?, Err(zx::sys::ZX_ERR_NOT_FOUND));
    Ok(())
}
