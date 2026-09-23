// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "256"]

mod dma_heap;
mod fastrpc;

pub use fastrpc::fastrpc_device_init;
use starnix_core::task::CurrentTask;
use starnix_uapi::user_address::ArchSpecific;

pub fn canonicalize_ioctl_request(current_task: &CurrentTask, request: u32) -> u32 {
    if current_task.is_arch32() {
        match request {
            // FastRPC
            linux_uapi::arch32::FASTRPC_IOCTL_INVOKE => linux_uapi::FASTRPC_IOCTL_INVOKE,
            linux_uapi::arch32::FASTRPC_IOCTL_INVOKE_FD => linux_uapi::FASTRPC_IOCTL_INVOKE_FD,
            linux_uapi::arch32::FASTRPC_IOCTL_GETINFO => linux_uapi::FASTRPC_IOCTL_GETINFO,
            linux_uapi::arch32::FASTRPC_IOCTL_GET_DSP_INFO => {
                linux_uapi::FASTRPC_IOCTL_GET_DSP_INFO
            }
            linux_uapi::arch32::FASTRPC_IOCTL_INVOKE2 => linux_uapi::FASTRPC_IOCTL_INVOKE2,
            linux_uapi::arch32::FASTRPC_IOCTL_INIT => linux_uapi::FASTRPC_IOCTL_INIT,
            // DMA Heaps
            linux_uapi::arch32::DMA_HEAP_IOCTL_ALLOC => linux_uapi::DMA_HEAP_IOCTL_ALLOC,
            linux_uapi::arch32::DMA_BUF_IOCTL_SYNC => linux_uapi::DMA_BUF_IOCTL_SYNC,
            linux_uapi::arch32::DMA_BUF_SET_NAME_B => linux_uapi::DMA_BUF_SET_NAME_B,
            _ => request,
        }
    } else {
        request
    }
}
