// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fidl_fuchsia_driver_test as fdt;
use fidl_fuchsia_hardware_gpio as gpio;
use fuchsia_async as fasync;
use fuchsia_async::OnSignals;
use fuchsia_component_test::RealmBuilder;
use fuchsia_driver_test::{DriverTestRealmBuilder2, DriverTestRealmInstance2};
use zx::Signals;

async fn run_test(driver_url: &str) -> Result<()> {
    let builder = RealmBuilder::new().await?;

    builder.driver_test_realm_setup().await?;

    let realm = builder.build().await?;

    let args = fdt::RealmArgs {
        use_driver_framework_v2: Some(true),
        root_driver: Some("fuchsia-boot:///platform-bus#meta/platform-bus.cm".to_string()),
        software_devices: Some(vec![fdt::DeviceCategory {
            category: "gpio".to_string(),
            bind_rules: vec![fdt::BindRule {
                key: "fuchsia.SERVICE".to_string(),
                value: fdt::BindValue::StringValue("fuchsia.hardware.gpio.Service".to_string()),
            }],
        }]),
        ..Default::default()
    };

    realm.driver_test_realm_start(args).await?;

    let dev = realm.driver_test_realm_connect_to_dev()?;

    // Wait for the driver under test to bind to the gpio device node.
    let gpio_node = device_watcher::recursive_wait_and_open::<gpio::GpioMarker>(
        &dev,
        "sys/platform/gpio-device/gpio",
    )
    .await?;

    // Create a virtual interrupt.
    let (virtual_interrupt, client_interrupt) = zx::Interrupt::create_virtual()?;

    // Hand the virtual interrupt to the driver.
    gpio_node.get_interrupt(0, client_interrupt).await?.map_err(zx::Status::from_raw)?;

    // Trigger the interrupt.
    virtual_interrupt.trigger(zx::Instant::from_nanos(0))?;

    // Verify that the driver acknowledges the interrupt.
    OnSignals::new(&virtual_interrupt, Signals::VIRTUAL_INTERRUPT_UNTRIGGERED).await?;

    realm.destroy().await?;
    Ok(())
}

#[fasync::run_singlethreaded(test)]
async fn test_driver() {
    run_test("#meta/new_gpio_interrupt_driver.cm").await.unwrap();
}
