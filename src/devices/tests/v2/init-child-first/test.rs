// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_driver_test as fdt;

use fuchsia_component_test::RealmBuilder;
use fuchsia_driver_test::{DriverTestRealmBuilder2, DriverTestRealmInstance2, Options2};

#[fuchsia::test]
async fn test_init() {
    let builder = RealmBuilder::new().await.expect("Creating RealmBuilder");

    let args = fdt::RealmArgs {
        root_driver: Some("fuchsia-boot:///dtr#meta/test-parent-sys.cm".to_string()),
        ..Default::default()
    };
    builder
        .driver_test_realm_setup(Options2::new(), args)
        .await
        .expect("Setting up DriverTestRealm");

    let instance = builder.build().await.expect("Building builder");

    instance.wait_for_bootup().await.expect("Waiting for bootup");

    let dev = instance.driver_test_realm_connect_to_dev().expect("Connecting to devfs");
    device_watcher::recursive_wait(&dev, "sys/test/root/child").await.expect("Opening node");
    instance.destroy().await.expect("Destroying instance");
}
