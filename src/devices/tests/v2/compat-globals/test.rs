// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fidl_fuchsia_driver_test as fdt;
use fuchsia_component_test::RealmBuilder;
use fuchsia_driver_test::{DriverTestRealmBuilder2, DriverTestRealmInstance2, Options2};

// This test checks for a very specific bug in the compat driver, where
// adding devices would fail if it was in the same driver in the same driver host,
// even if those two drivers had different instances.
#[fuchsia::test]
async fn test_adding_children() -> Result<()> {
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

    // Connect to our root-a/leaf driver.
    let dev = instance.driver_test_realm_connect_to_dev()?;
    let driver =
        device_watcher::recursive_wait_and_open::<fidl_fuchsia_hardware_compat::LeafMarker>(
            &dev,
            "sys/test/root-a/leaf",
        )
        .await?;

    // Make sure we can add a child.
    let response = driver.add_child("child").await.unwrap();
    assert_eq!(response, zx::sys::ZX_OK);

    // Connect to our root-b/leaf driver.
    let dev = instance.driver_test_realm_connect_to_dev()?;
    let driver =
        device_watcher::recursive_wait_and_open::<fidl_fuchsia_hardware_compat::LeafMarker>(
            &dev,
            "sys/test/root-b/leaf",
        )
        .await?;

    // Make sure we can add a child with the *same name* that we added
    // to root-a/leaf.
    let response = driver.add_child("child").await.unwrap();
    assert_eq!(response, zx::sys::ZX_OK);

    // Check that both children are in /dev/.
    device_watcher::recursive_wait(&dev, "sys/test/root-a/leaf/child").await?;
    device_watcher::recursive_wait(&dev, "sys/test/root-b/leaf/child").await?;

    instance.destroy().await?;
    Ok(())
}

// This test checks that a driver shares globals with the same driver in
// the same driver host.
#[fuchsia::test]
async fn test_sharing_globals() -> Result<()> {
    // Create the RealmBuilder.
    let builder = RealmBuilder::new().await?;
    let args = fdt::RealmArgs {
        root_driver: Some("fuchsia-boot:///#meta/test-parent-sys.cm".to_string()),
        ..Default::default()
    };
    builder.driver_test_realm_setup(Options2::default(), args).await?;
    // Build the Realm.
    let instance = builder.build().await?;
    instance.wait_for_bootup().await?;

    // Connect to our root-a/leaf driver.
    let dev = instance.driver_test_realm_connect_to_dev()?;
    let driver =
        device_watcher::recursive_wait_and_open::<fidl_fuchsia_hardware_compat::LeafMarker>(
            &dev,
            "sys/test/root-a/leaf",
        )
        .await?;

    // Our global should be 0, and we are incrementing it to 1.
    let counter = driver.global_counter().await.unwrap();
    assert_eq!(counter, 0);

    // Connect to our root-b/leaf driver.
    let dev = instance.driver_test_realm_connect_to_dev()?;
    let driver =
        device_watcher::recursive_wait_and_open::<fidl_fuchsia_hardware_compat::LeafMarker>(
            &dev,
            "sys/test/root-b/leaf",
        )
        .await?;

    // Our global should be 1 (since root-a incremented it).
    let counter = driver.global_counter().await.unwrap();
    assert_eq!(counter, 1);

    instance.destroy().await?;
    Ok(())
}
