// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::file::GrallocFile;
use starnix_core::device::DeviceOps;
use starnix_core::task::{CurrentTask, Kernel};
use starnix_core::vfs::{FileOps, NamespaceNode};

use starnix_uapi::device_id::DeviceId;
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;

#[derive(Clone)]
struct GrallocDevice;

impl DeviceOps for GrallocDevice {
    fn open(
        &self,
        current_task: &CurrentTask,
        _id: DeviceId,
        _node: &NamespaceNode,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        GrallocFile::new_file(current_task)
    }
}

pub fn gralloc_device_init(kernel: &Kernel) {
    let registry = &kernel.device_registry;
    registry
        .register_dyn_device(
            kernel,
            "virtgralloc0".into(),
            registry.objects.starnix_class(),
            GrallocDevice,
        )
        .expect("can register virtgralloc0");
}
