// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::file::RtcFile;
use starnix_core::device::DeviceOps;
use starnix_core::task::CurrentTask;
use starnix_core::vfs::{FileOps, NamespaceNode};
use starnix_logging::log_debug;

use starnix_uapi::device_id::DeviceId;
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;

#[derive(Clone)]
struct RtcDevice;

impl DeviceOps for RtcDevice {
    fn open(
        &self,
        current_task: &CurrentTask,
        _id: DeviceId,
        _node: &NamespaceNode,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        RtcFile::new_file(current_task)
    }
}

/// Initialize a RTC device.
///
/// It will be available at `/dev/rtc0`.
pub fn rtc_device_init(current_task: &CurrentTask) -> Result<(), Errno> {
    let kernel = current_task.kernel();
    let registry = &kernel.device_registry;
    registry
        .register_dyn_device(
            current_task.kernel(),
            "rtc0".into(),
            registry.objects.rtc_class(),
            RtcDevice,
        )
        .inspect(|dev| log_debug!("registered RTC device: {dev:?}"))
        .map(|_| ())
}
