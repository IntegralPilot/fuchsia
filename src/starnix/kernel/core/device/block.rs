// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::device::DeviceMode;
use crate::device::kobject::DeviceMetadata;
use crate::fs::sysfs::{BlockDeviceInfo, build_block_device_directory};
use crate::task::{CurrentTask, Kernel};
use crate::vfs::{FileOps, FsString, NamespaceNode};
use starnix_logging::track_stub;
use starnix_uapi::device_id::DeviceId;
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::user_address::ArchSpecific;
use starnix_uapi::{errno, uapi};
use std::sync::Arc;

pub fn canonicalize_ioctl_request(current_task: &CurrentTask, request: u32) -> u32 {
    if current_task.is_arch32() {
        match request {
            uapi::arch32::BLKGETSIZE64 => uapi::BLKGETSIZE64,
            _ => request,
        }
    } else {
        request
    }
}

pub struct MmcBlockDevice;

impl BlockDeviceInfo for MmcBlockDevice {
    fn size(&self) -> Result<usize, Errno> {
        track_stub!(TODO("https://fxbug.dev/488067251"), "mmcblk query size");
        Err(errno!(ENOTSUP))
    }
}

fn open_mmc_block_device(
    _current_task: &CurrentTask,
    _id: DeviceId,
    _node: &NamespaceNode,
    _flags: OpenFlags,
) -> Result<Box<dyn FileOps>, Errno> {
    track_stub!(TODO("https://fxbug.dev/488067251"), "mmcblk open device");
    Err(errno!(ENOTSUP))
}

/// Adds an mmc block device at /dev/block/mmcblk0. The current implementation is just a stub that
/// exports the typical sysfs layout for block devices, but cannot be read from or written to.
pub fn add_mmc_block_device(kernel: &Kernel) -> Result<Arc<MmcBlockDevice>, Errno> {
    let name = FsString::from("mmcblk0");
    let class = kernel.device_registry.objects.virtual_block_class();
    let device = Arc::new(MmcBlockDevice);
    let device_weak = Arc::downgrade(&device);
    kernel.device_registry.register_device_with_dir(
        kernel,
        name.as_ref(),
        DeviceMetadata::new(name.clone(), DeviceId::MMCBLK0, DeviceMode::Block),
        class,
        |device, dir| build_block_device_directory(device, device_weak, dir),
        open_mmc_block_device,
    )?;
    Ok(device)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{anon_test_file, spawn_kernel_and_run};
    use crate::vfs::VecOutputBuffer;
    use starnix_uapi::open_flags::OpenFlags;

    #[::fuchsia::test]
    async fn test_mmc_block_device() {
        spawn_kernel_and_run(async |current_task| {
            let _device = add_mmc_block_device(current_task.kernel()).unwrap();
            let class = current_task.kernel().device_registry.objects.virtual_block_class();
            // The device should have a typical sysfs layout for block devices.
            assert!(class.dir.lookup(b"mmcblk0/holders".into()).is_some());
            // We should be able to open the size node of the stub device, but reading it will fail
            // since right now it is just a stub implementation.
            let size_node = class.dir.lookup(b"mmcblk0/size".into()).unwrap();
            let file_ops = size_node.create_file_ops(&current_task, OpenFlags::RDONLY).unwrap();
            let file = anon_test_file(&current_task, file_ops, OpenFlags::RDONLY);
            let mut buf = VecOutputBuffer::new(10);
            assert_eq!(file.read(&current_task, &mut buf).unwrap_err(), errno!(ENOTSUP));
        })
        .await;
    }
}
