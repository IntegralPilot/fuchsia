// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::iio_file::{
    build_battery_power_supply_directory, build_iio0_directory, build_iio1_directory,
    build_usb_power_supply_directory,
};
use super::qbg_battery_file::create_battery_profile_device;
use super::qbg_file::create_qbg_device;
use super::utils::{ReadWriteBytesFile, connect_to_device_channel};
use fidl_fuchsia_hardware_qcom_hvdcpopti as fhvdcpopti;
use starnix_core::device::DeviceMode;
use starnix_core::device::kobject::DeviceMetadata;
use starnix_core::fs::sysfs::build_device_directory;
use starnix_core::task::Kernel;
use starnix_logging::log_warn;

use starnix_uapi::device_id::DeviceId;
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::mode;
use std::sync::Arc;

pub fn hvdcp_opti_init(kernel: &Kernel) -> Result<(), Errno> {
    let proxy = match connect_to_device_channel("iio") {
        Ok(chan) => Arc::new(fhvdcpopti::IioSynchronousProxy::new(chan)),
        Err(e) => {
            // hvdcp_opti only supported on Sorrel. Let it fail.
            log_warn!(
                "Could not connect to hvdcp_opti server {}. This is expected on everything but Sorrel.",
                e
            );
            return Ok(());
        }
    };

    let registry = &kernel.device_registry;

    let qdb_class =
        registry.objects.class_with_dir("qbg".into(), registry.objects.virtual_bus(), |dir| {
            dir.entry("qbg_context", ReadWriteBytesFile::new_node(), mode!(IFREG, 0o666));
        });

    // /dev/qbg
    registry.register_device(
        kernel,
        "qbg".into(),
        DeviceMetadata::new("qbg".into(), DeviceId::new(484, 0), DeviceMode::Char),
        qdb_class,
        create_qbg_device,
    )?;

    // /dev/qbg_battery
    registry.register_device(
        kernel,
        "qbg_battery".into(),
        DeviceMetadata::new("qbg_battery".into(), DeviceId::new(485, 0), DeviceMode::Char),
        registry.objects.get_or_create_class("qbg_battery".into(), registry.objects.virtual_bus()),
        create_battery_profile_device,
    )?;

    // /sys/bus/iio/devices/iio:device
    // IIO devices should not show up under /sys/class. This makes it show up under /sys/class,
    // but it's OK.
    let iio = registry
        .objects
        .get_or_create_class("iio".into(), registry.objects.get_or_create_bus("iio".into()));
    registry.add_numberless_device("iio:device0".into(), iio.clone(), |device, dir| {
        build_iio0_directory(device, &proxy, dir)
    });

    registry.add_numberless_device("iio:device1".into(), iio, |device, dir| {
        build_iio1_directory(device, &proxy, dir)
    });

    // power_supply devices don't show up under any bus. This makes it show up under virtual_bus,
    // but it's OK.
    let power_supply =
        registry.objects.get_or_create_class("power_supply".into(), registry.objects.virtual_bus());
    // /sys/class/power_supply/usb
    registry.add_numberless_device("usb".into(), power_supply.clone(), |device, dir| {
        build_usb_power_supply_directory(device, &proxy, dir)
    });

    // /sys/class/power_supply/battery
    registry.add_numberless_device("battery".into(), power_supply.clone(), |device, dir| {
        build_battery_power_supply_directory(device, &proxy, dir)
    });

    // /sys/class/power_supply/bms
    registry.add_numberless_device("bms".into(), power_supply, build_device_directory);
    Ok(())
}
