// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fidl_fuchsia_compat_runtime_test as ft;
use fidl_fuchsia_driver_test as fdt;
use fuchsia_component_test::{RealmBuilder, Ref};
use fuchsia_driver_test::{DriverTestRealmBuilder2, DriverTestRealmInstance2, Options2};

#[fuchsia::test]
async fn test_compat_runtime() -> Result<()> {
    // Create the RealmBuilder.
    let builder = RealmBuilder::new().await?;

    let offer = fuchsia_component_test::Capability::protocol::<ft::WaiterMarker>().into();
    let offers = vec![offer];

    let args =
        fdt::RealmArgs { root_driver: Some("#meta/root.cm".to_string()), ..Default::default() };
    builder
        .driver_test_realm_setup(Options2::new().driver_offers(Ref::parent(), offers), args)
        .await?;
    // Build the Realm.
    let instance = builder.build().await?;
    instance.wait_for_bootup().await?;

    // Connect to our driver.
    let dev = instance.driver_test_realm_connect_to_dev()?;
    let driver = device_watcher::recursive_wait_and_open::<ft::LeafMarker>(&dev, "v1/leaf").await?;
    let response = driver.get_string().await.unwrap();
    assert_eq!(response, "hello world!");
    instance.destroy().await?;
    Ok(())
}
