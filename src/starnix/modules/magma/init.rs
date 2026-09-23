// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::MagmaFile;
use starnix_core::device::DeviceOps;
use starnix_core::task::{CurrentTask, Kernel};
use starnix_core::vfs::{FileOps, NamespaceNode};

use starnix_uapi::device_id::DeviceId;
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;

#[derive(Clone)]
struct MagmaDeviceBuilder {
    supported_vendors: Vec<u16>,
}

impl DeviceOps for MagmaDeviceBuilder {
    fn open(
        &self,
        current_task: &CurrentTask,
        id: DeviceId,
        node: &NamespaceNode,
        flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        MagmaFile::new_file(
            current_task,
            id,
            &node.entry.node,
            flags,
            self.supported_vendors.clone(),
        )
    }
}

pub fn magma_device_init(kernel: &Kernel, supported_vendors: Vec<u16>) {
    let registry = &kernel.device_registry;
    let builder = MagmaDeviceBuilder { supported_vendors };

    registry
        .register_dyn_device(kernel, "magma0".into(), registry.objects.starnix_class(), builder)
        .expect("can register magma0");
}
