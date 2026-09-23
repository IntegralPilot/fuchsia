// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "256"]

use starnix_core::device::kobject::{Device, DeviceMetadata};
use starnix_core::device::{DeviceMode, DeviceOps};
use starnix_core::fs::sysfs::{BlockDeviceInfo, build_block_device_directory};
use starnix_core::task::{CurrentTask, Kernel, KernelStats};
use starnix_core::vfs::pseudo::dynamic_file::{DynamicFile, DynamicFileBuf, DynamicFileSource};
use starnix_core::vfs::pseudo::simple_directory::SimpleDirectoryMutator;
use starnix_core::vfs::pseudo::stub_empty_file::StubEmptyFile;
use starnix_core::vfs::{
    FileOps, FsNodeOps, NamespaceNode, fileops_impl_dataless, fileops_impl_noop_sync,
    fileops_impl_seekless,
};
use starnix_logging::{bug_ref, log_error};

use starnix_uapi::device_id::{DeviceId, ZRAM_MAJOR};
use starnix_uapi::errno;
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::mode;
use starnix_uapi::open_flags::OpenFlags;
use std::sync::{Arc, Weak};

#[derive(Default, Clone)]
pub struct ZramDevice {
    kernel_stats: Arc<KernelStatsWrapper>,
}

impl ZramDevice {
    fn get_stats(&self) -> Result<fidl_fuchsia_kernel::MemoryStatsCompression, Errno> {
        self.kernel_stats.get_stats()
    }
}

impl DeviceOps for ZramDevice {
    fn open(
        &self,
        _current_task: &CurrentTask,
        _id: DeviceId,
        _node: &NamespaceNode,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        Ok(Box::new(self.clone()))
    }
}

impl FileOps for ZramDevice {
    fileops_impl_seekless!();
    fileops_impl_dataless!();
    fileops_impl_noop_sync!();
}

pub fn zram_device_init(kernel: &Kernel) -> Result<(), Errno> {
    let zram_device = ZramDevice::default();
    let zram_device_clone = zram_device.clone();
    let registry = &kernel.device_registry;
    registry.register_device_with_dir(
        kernel,
        "zram0".into(),
        DeviceMetadata::new("zram0".into(), DeviceId::new(ZRAM_MAJOR, 0), DeviceMode::Block),
        registry.objects.virtual_block_class(),
        |device, dir| build_zram_device_directory(device, zram_device_clone, dir),
        zram_device,
    )?;
    Ok(())
}

fn build_zram_device_directory(
    device: &Device,
    zram_device: ZramDevice,
    dir: &SimpleDirectoryMutator,
) {
    let block_info = Arc::downgrade(&zram_device.kernel_stats) as Weak<dyn BlockDeviceInfo>;
    build_block_device_directory(device, block_info, dir);
    dir.entry(
        "idle",
        StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/322892951")),
        mode!(IFREG, 0o664),
    );
    dir.entry("mm_stat", MmStatFile::new_node(zram_device), mode!(IFREG, 0o444));
}

#[derive(Clone)]
struct MmStatFile {
    device: ZramDevice,
}
impl MmStatFile {
    pub fn new_node(device: ZramDevice) -> impl FsNodeOps {
        DynamicFile::new_node(Self { device })
    }
}
impl DynamicFileSource for MmStatFile {
    fn generate(
        &self,
        _current_task: &CurrentTask,
        sink: &mut DynamicFileBuf,
    ) -> Result<(), Errno> {
        let stats = self.device.get_stats()?;

        let compressed_storage_bytes = stats.compressed_storage_bytes.unwrap_or_default();
        let compressed_fragmentation_bytes =
            stats.compressed_fragmentation_bytes.unwrap_or_default();

        let orig_data_size = stats.uncompressed_storage_bytes.unwrap_or_default();
        // This value isn't entirely correct because we're still counting metadata and other
        // non-fragmentation usage.
        let compr_data_size = compressed_storage_bytes - compressed_fragmentation_bytes;
        let mem_used_total = compressed_storage_bytes;
        // The remaining values are not yet available from Zircon.
        let mem_limit = 0;
        let mem_used_max = 0;
        let same_pages = 0;
        let pages_compacted = 0;
        let huge_pages = 0;

        writeln!(
            sink,
            "{orig_data_size} {compr_data_size} {mem_used_total} {mem_limit} \
                        {mem_used_max} {same_pages} {pages_compacted} {huge_pages}"
        )?;
        Ok(())
    }
}

#[derive(Default)]
struct KernelStatsWrapper(KernelStats);

impl KernelStatsWrapper {
    fn get_stats(&self) -> Result<fidl_fuchsia_kernel::MemoryStatsCompression, Errno> {
        self.0.get().get_memory_stats_compression(zx::MonotonicInstant::INFINITE).map_err(|e| {
            log_error!("FIDL error getting memory compression stats: {e}");
            errno!(EIO)
        })
    }
}

impl BlockDeviceInfo for KernelStatsWrapper {
    fn size(&self) -> Result<usize, Errno> {
        Ok(self.get_stats()?.uncompressed_storage_bytes.unwrap_or_default() as usize)
    }
}
