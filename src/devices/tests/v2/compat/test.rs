// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fidl_fuchsia_driver_test as fdt;
use fuchsia_component_test::RealmBuilder;
use fuchsia_driver_test::{DriverTestRealmBuilder2, DriverTestRealmInstance2, Options2};

#[fuchsia::test]
async fn test_sample_driver() -> Result<()> {
    // Create the RealmBuilder.
    let builder = RealmBuilder::new().await?;

    let args = fdt::RealmArgs {
        root_driver: Some("fuchsia-boot:///dtr#meta/test-parent-sys.cm".to_string()),
        ..Default::default()
    };
    builder.driver_test_realm_setup(Options2::default(), args).await?;
    // Build the Realm.
    let instance = builder.build().await?;
    instance.wait_for_bootup().await?;

    // Connect to our driver.
    let dev = instance.driver_test_realm_connect_to_dev()?;
    let driver =
        device_watcher::recursive_wait_and_open::<fidl_fuchsia_hardware_compat::LeafMarker>(
            &dev,
            "sys/test/root/leaf",
        )
        .await?;
    let response = driver.get_string().await.unwrap();
    assert_eq!(response, "hello world!");
    instance.destroy().await?;
    Ok(())
}
