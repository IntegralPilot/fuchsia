// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use fidl_fuchsia_driver_test as fdt;
use fuchsia_component_test::{RealmBuilder, RealmInstance};
use fuchsia_driver_test::{DriverTestRealmBuilder, DriverTestRealmInstance};

async fn start_driver_test_realm() -> Result<RealmInstance> {
    let builder = RealmBuilder::new().await.context("Failed to create realm builder")?;
    builder.driver_test_realm_setup().await.context("Failed to setup driver test realm")?;
    let instance = builder.build().await.context("Failed to build realm instance")?;

    let args = fdt::RealmArgs {
        root_driver: Some("fuchsia-boot:///dtr#meta/test-parent-sys.cm".to_string()),
        ..Default::default()
    };

    instance.driver_test_realm_start(args).await.context("Failed to start driver test realm")?;

    Ok(instance)
}

// Tests that the legacy and spec composites are successfully assembled, bound, and
// added to the topology.
#[fuchsia::test]
async fn test_composites() -> Result<()> {
    let instance = start_driver_test_realm().await?;
    let dev = instance.driver_test_realm_connect_to_dev()?;

    device_watcher::recursive_wait(&dev, "sys/test/root").await?;
    device_watcher::recursive_wait(&dev, "sys/test/node_a").await?;
    device_watcher::recursive_wait(&dev, "sys/test/node_b").await?;
    device_watcher::recursive_wait(&dev, "sys/test/node_c").await?;
    device_watcher::recursive_wait(&dev, "sys/test/node_d").await?;

    device_watcher::recursive_wait(&dev, "sys/test/node_a/spec_1/composite").await?;
    device_watcher::recursive_wait(&dev, "sys/test/node_d/spec_2/composite").await?;

    Ok(())
}
