// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fuchsia_driver_test as fdt;
use fuchsia_component_test::RealmBuilder;
use fuchsia_driver_test::{DriverTestRealmBuilder, DriverTestRealmInstance};

#[fuchsia::test]
async fn toplogy_test() -> Result<(), Error> {
    let builder = RealmBuilder::new().await?;
    builder.driver_test_realm_setup().await?;

    let instance = builder.build().await?;
    instance
        .driver_test_realm_start(fdt::RealmArgs {
            root_driver: Some("fuchsia-boot:///dtr#meta/test-parent-sys.cm".to_string()),
            ..Default::default()
        })
        .await?;

    let dev = instance.driver_test_realm_connect_to_dev()?;
    println!("wait for grandparent");
    device_watcher::recursive_wait(&dev, "sys/test/topology-grandparent").await?;
    println!("wait for child 1");
    device_watcher::recursive_wait(&dev, "sys/test/topology-grandparent/parent1/child").await?;
    println!("wait for child 2");
    device_watcher::recursive_wait(&dev, "sys/test/topology-grandparent/parent2/child").await?;
    println!("all done!");

    Ok(())
}
