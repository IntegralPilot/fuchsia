// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use fidl_fidl_examples_echo as fecho;
use fidl_fuchsia_driver_test as fdt;
use fuchsia_component::client;
use fuchsia_component_test::RealmBuilder;
use fuchsia_driver_test::{DriverTestRealmBuilder2, DriverTestRealmInstance2, Options2};

const PDEV_VID_TEST: u32 = 0x11;
const PDEV_PID_TEST: u32 = 0x18;

/// Returns ok if the driver is bound.
async fn verify_driver_is_bound(instance: &fuchsia_component_test::RealmInstance) -> Result<()> {
    // Verify the driver is bound by connecting to the driver's exposed EchoService FIDL service.

    // Connect to the `EchoService`.
    let service =
        client::Service::open_from_dir(instance.root.get_exposed_dir(), fecho::EchoServiceMarker)
            .context("Failed to open service")?
            .watch_for_any()
            .await
            .context("Failed to find instance")?;

    // Use the `echo` protocol from the `EchoService`.
    let echo = service.connect_to_echo().context("Failed to connect to echo protocol")?;

    // Call a method to verify connection.
    let result = echo.echo_string(Some("Hello")).await.context("Failed to call echo_string")?;
    assert_eq!(result, Some("Hello".to_string()));
    Ok(())
}

async fn setup_realm() -> Result<fuchsia_component_test::RealmInstance> {
    // Read the DTB file.
    let dtb = std::fs::read("/pkg/test-data/test-device.dtb").context("Failed to read DTB file")?;
    let vmo = zx::Vmo::create(dtb.len() as u64).context("Failed to create VMO")?;
    vmo.write(&dtb, 0).context("Failed to write to VMO")?;

    let args = fdt::RealmArgs {
        root_driver: Some("fuchsia-boot:///platform-bus#meta/platform-bus.cm".to_string()),
        devicetree: Some(vmo),
        platform_vid: Some(PDEV_VID_TEST),
        platform_pid: Some(PDEV_PID_TEST),
        board_name: Some("fake-board".to_string()),
        ..Default::default()
    };

    // Create the RealmBuilder.
    let builder = RealmBuilder::new().await.context("Failed to create RealmBuilder")?;

    let expose = fuchsia_component_test::Capability::service::<fecho::EchoServiceMarker>().into();
    let exposes = vec![expose];

    DriverTestRealmBuilder2::driver_test_realm_setup(
        &builder,
        Options2::new().using_subpackage(true).driver_exposes(exposes),
        args,
    )
    .await
    .context("Failed to setup driver test realm")?;

    // Build the Realm.
    let instance = builder.build().await.context("Failed to build realm")?;

    // Wait for boot-up.
    instance.wait_for_bootup().await.context("Failed to wait for bootup")?;

    Ok(instance)
}

// Verifies that the driver binds to the devicetree node specified in the DTB.
#[fuchsia::test]
async fn test_devicetree_driver_binds() -> Result<()> {
    let instance = setup_realm().await.context("Failed to setup the test realm")?;

    verify_driver_is_bound(&instance).await.context("Failed to verify that the driver is bound")?;

    Ok(())
}
