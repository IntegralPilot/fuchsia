// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use rutabaga_gfx::{RutabagaBuilder, RutabagaComponentType, RutabagaFenceHandler};
use starnix_core::task::{CurrentTask, Kernel};
use starnix_core::vfs::{FileOps, NamespaceNode};
use starnix_logging::log_error;

use starnix_uapi::device_id::DeviceId;
use starnix_uapi::error;
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;

fn create_gpu_device(
    _current_task: &CurrentTask,
    _id: DeviceId,
    _node: &NamespaceNode,
    _flags: OpenFlags,
) -> Result<Box<dyn FileOps>, Errno> {
    log_error!("virtio-gpu unsupported");
    error!(ENOTSUP)
}

pub fn gpu_device_init(kernel: &Kernel) {
    let registry = &kernel.device_registry;

    let _ = RutabagaBuilder::new(0, RutabagaFenceHandler::new(move |_| {}))
        .set_default_component(RutabagaComponentType::Gfxstream)
        .build();

    registry
        .register_dyn_device(
            kernel,
            "virtio-gpu".into(),
            registry.objects.starnix_class(),
            create_gpu_device,
        )
        .expect("can register virtio-gpu");
}
