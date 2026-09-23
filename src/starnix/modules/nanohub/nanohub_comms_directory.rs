// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::display_sysfs_files::{
    DisplayBrightnessSysFsOps, DisplayInfoSysFsOps, DisplayPanelStateSysFsOps,
    DisplaySelectSysFsOps, DisplayStateSysFsOps,
};
use crate::nanohub_sysfs_files::{FirmwareNameSysFsOps, HardwareResetSysFsOps, TimeSyncSysFsOps};
use crate::socket_tunnel_file::FirmwareFile;
use crate::sysfs::SysfsNode;
use starnix_core::device::kobject::Device;
use starnix_core::fs::sysfs::build_device_directory;
use starnix_core::vfs::pseudo::simple_directory::SimpleDirectoryMutator;
use starnix_core::vfs::pseudo::simple_file::BytesFile;
use starnix_uapi::file_mode::mode;
use {
    fidl_fuchsia_hardware_backlight as fbacklight, fidl_fuchsia_hardware_google_nanohub as fnanohub,
};

pub fn build_display_comms_directory(device: &Device, dir: &SimpleDirectoryMutator) {
    build_device_directory(device, dir);
    dir.entry(
        "display_state",
        SysfsNode::<fnanohub::DisplayDeviceMarker, DisplayStateSysFsOps>::new(),
        mode!(IFREG, 0o440),
    );
    dir.entry(
        "display_info",
        SysfsNode::<fnanohub::DisplayDeviceMarker, DisplayInfoSysFsOps>::new(),
        mode!(IFREG, 0o440),
    );
    dir.entry(
        "display_select",
        SysfsNode::<fnanohub::DisplayDeviceMarker, DisplaySelectSysFsOps>::new(),
        mode!(IFREG, 0o660),
    );
    dir.entry(
        "display_brightness",
        SysfsNode::<fbacklight::DeviceMarker, DisplayBrightnessSysFsOps>::new(),
        mode!(IFREG, 0o440),
    );
    dir.entry(
        "display_panel_state",
        SysfsNode::<fbacklight::DeviceMarker, DisplayPanelStateSysFsOps>::new(),
        mode!(IFREG, 0o440),
    );
}

pub fn build_nanohub_comms_directory(device: &Device, dir: &SimpleDirectoryMutator) {
    build_device_directory(device, dir);

    dir.entry(
        "display_panel_name",
        BytesFile::new_node(b"FL22-BOE-WJ014SGM".to_vec()),
        mode!(IFREG, 0o440),
    );

    dir.entry("download_firmware", FirmwareFile::new(), mode!(IFREG, 0o220));
    dir.entry(
        "firmware_name",
        SysfsNode::<fnanohub::DeviceMarker, FirmwareNameSysFsOps>::new(),
        mode!(IFREG, 0o440),
    );
    dir.entry(
        "hw_reset",
        SysfsNode::<fnanohub::DeviceMarker, HardwareResetSysFsOps>::new(),
        mode!(IFREG, 0o220),
    );
    dir.entry(
        "time_sync".into(),
        SysfsNode::<fnanohub::DeviceMarker, TimeSyncSysFsOps>::new(),
        mode!(IFREG, 0o440),
    );
}
