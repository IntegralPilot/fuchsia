// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use fidl::endpoints::Proxy as _;
use fidl_fuchsia_driver_test as fdt;
use fidl_fuchsia_rebind_test as frt;
use fuchsia_component_test::{RealmBuilder, RealmInstance};
use fuchsia_driver_test::{DriverTestRealmBuilder2, DriverTestRealmInstance2, Options2};

async fn start_driver_test_realm() -> Result<RealmInstance> {
    const ROOT_DRIVER_URL: &str = "fuchsia-boot:///dtr#meta/test-parent-sys.cm";

    let builder = RealmBuilder::new().await.context("Failed to create realm builder")?;
    builder
        .driver_test_realm_setup(
            Options2::new(),
            fdt::RealmArgs { root_driver: Some(ROOT_DRIVER_URL.to_owned()), ..Default::default() },
        )
        .await
        .context("Failed to setup driver test realm")?;
    let instance = builder.build().await.context("Failed to build realm instance")?;
    instance.wait_for_bootup().await.context("Failed to wait for bootup")?;
    Ok(instance)
}

const PARENT_DEV_PATH: &str = "sys/test/rebind-parent";
const CHILD_DEV_PATH: &str = "sys/test/rebind-parent/added-child/rebind-child";

// Tests that a node will succesfully bind to a driver after the node has
// already been bound to that driver, then shutdown, then re-added.
#[fuchsia::test]
async fn test_rebind() -> Result<()> {
    let instance = start_driver_test_realm().await?;
    let dev = instance.driver_test_realm_connect_to_dev()?;

    let parent =
        device_watcher::recursive_wait_and_open::<frt::RebindParentMarker>(&dev, PARENT_DEV_PATH)
            .await?;
    parent.add_child().await?.map_err(zx::Status::err_from_raw)?;
    let child_controller =
        device_watcher::recursive_wait_and_open::<fidl_fuchsia_device::ControllerMarker>(
            &dev,
            &format!("{}/{}", CHILD_DEV_PATH, fidl_fuchsia_device_fs::DEVICE_CONTROLLER_NAME),
        )
        .await?;
    parent.remove_child().await?.map_err(zx::Status::err_from_raw)?;
    child_controller.on_closed().await?;
    parent.add_child().await?.map_err(zx::Status::err_from_raw)?;
    device_watcher::recursive_wait(&dev, CHILD_DEV_PATH).await?;
    instance.destroy().await?;
    Ok(())
}
