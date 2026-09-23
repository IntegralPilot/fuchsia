// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fidl_fuchsia_driver_test as fdt;
use fidl_test_wlan_testcontroller::TestControllerMarker;
use fuchsia_component_test::RealmBuilder;
use fuchsia_driver_test::{DriverTestRealmBuilder, DriverTestRealmInstance};

// Test that the testcontroller initializes properly and the test suite can connect to it.
#[fuchsia::test]
async fn testcontroller_init_test() -> Result<()> {
    let builder = RealmBuilder::new().await?;
    builder.driver_test_realm_setup().await?;

    let realm = builder.build().await?;

    realm.driver_test_realm_start(fdt::RealmArgs { ..Default::default() }).await?;

    let devfs = realm.driver_test_realm_connect_to_dev()?;
    let _controller = device_watcher::recursive_wait_and_open::<TestControllerMarker>(
        &devfs,
        "sys/test/wlan_testcontroller",
    )
    .await?;

    Ok(())
}
