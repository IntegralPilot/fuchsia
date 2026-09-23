// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_starnix_gralloc as fgralloc;
use starnix_core::mm::MemoryAccessorExt;
use starnix_core::task::CurrentTask;
use starnix_core::vfs::{
    FileObject, FileOps, InputBuffer, OutputBuffer, SeekTarget, fileops_impl_noop_sync,
};
use starnix_logging::{log_error, log_info, track_stub};

use starnix_syscalls::{SUCCESS, SyscallArg, SyscallResult};
use starnix_uapi::errors::Errno;
use starnix_uapi::user_address::{UserAddress, UserRef};
use starnix_uapi::{errno, error};
use virtgralloc::{
    VIRTGRALLOC_IOCTL_SET_VULKAN_MODE, VIRTGRALLOC_SET_VULKAN_MODE_RESULT_SUCCESS,
    virtgralloc_SetVulkanModeResult, virtgralloc_VulkanMode, virtgralloc_set_vulkan_mode,
};

pub struct GrallocFile {
    mode_setter: fgralloc::VulkanModeSetterSynchronousProxy,
}

impl GrallocFile {
    pub fn new_file(current_task: &CurrentTask) -> Result<Box<dyn FileOps>, Errno> {
        let mode_setter = current_task
            .kernel()
            .connect_to_protocol_at_container_svc::<fgralloc::VulkanModeSetterMarker>()
            .expect("gralloc feature requires fuchsia.starnix.gralloc.VulkanModeSetter protocol in container /svc dir, and a corresponding server")
            .into_sync_proxy();
        Ok(Box::new(Self { mode_setter }))
    }

    fn set_vulkan_mode(
        &self,
        vulkan_mode: virtgralloc_VulkanMode,
    ) -> Result<virtgralloc_SetVulkanModeResult, Errno> {
        let vulkan_mode = fgralloc::VulkanMode::from_primitive(vulkan_mode as u32)
            .ok_or_else(|| errno!(EINVAL))
            .map_err(|e| {
                log_error!("VulkanMode::from_primitive failed");
                e
            })?;

        // The SetVulkanMode FIDL call must complete before the IOCTL can complete, but we don't
        // need to hold the lock the whole time, just to start the request. We don't actually expect
        // more than up to one call to this IOCTL per container boot, but nice to avoid holding the
        // lock for any longer than necessary.
        //
        // This must work, since this is the only way that gralloc will know which vulkan impl,
        // which is necessary for gralloc to work properly, and the gralloc feature requires gralloc
        // to work properly. There's no meaningful way for the IOCTL caller to handle an IOCTL
        // failure other than just logging and then ignoring it, which would only make the overall
        // broken situation more confusing that just using expect() here. Under normal operation,
        // this is only called once early in boot.
        self.mode_setter
            .set_vulkan_mode(
                &fgralloc::VulkanModeSetterSetVulkanModeRequest {
                    vulkan_mode: Some(vulkan_mode),
                    ..Default::default()
                },
                zx::MonotonicInstant::INFINITE,
            )
            .expect("gralloc feature requires working VulkanModeSetter")
            .expect("gralloc feature requires VulkanModeSetter to set mode");

        log_info!("gralloc vulkan_mode set to {:?}", vulkan_mode);

        Ok(VIRTGRALLOC_SET_VULKAN_MODE_RESULT_SUCCESS)
    }
}

impl FileOps for GrallocFile {
    fileops_impl_noop_sync!();

    fn is_seekable(&self) -> bool {
        false
    }

    fn seek(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _current_offset: starnix_uapi::off_t,
        _target: SeekTarget,
    ) -> Result<starnix_uapi::off_t, starnix_uapi::errors::Errno> {
        error!(ESPIPE)
    }

    fn ioctl(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        request: u32,
        arg: SyscallArg,
    ) -> Result<SyscallResult, Errno> {
        match request {
            VIRTGRALLOC_IOCTL_SET_VULKAN_MODE => {
                // switch from map_err to inspect_err when/if stable
                let user_addr = UserAddress::from(arg);
                let mut request: virtgralloc_set_vulkan_mode =
                    current_task.read_object(UserRef::new(user_addr)).map_err(|e| {
                        log_error!("virtgralloc_request_SetVulkanMode read_object failed: {:?}", e);
                        e
                    })?;
                request.result = self.set_vulkan_mode(request.vulkan_mode).map_err(|e| {
                    log_error!("set_vulkan_mode failed: {:?}", e);
                    e
                })?;
                current_task.write_object(UserRef::new(user_addr), &request)
            }
            unknown_ioctl => {
                track_stub!(TODO("https://fxbug.dev/322874368"), "gralloc ioctl", unknown_ioctl);
                error!(ENOSYS)
            }
        }?;

        Ok(SUCCESS)
    }

    fn read(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        offset: usize,
        _data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        // because fileops_impl_nonseekable!()
        debug_assert!(offset == 0);
        error!(EINVAL)
    }

    fn write(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        offset: usize,
        _data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        // because fileops_impl_nonseekable!()
        debug_assert!(offset == 0);
        error!(EINVAL)
    }
}
