// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://fxbug.dev/504722357): Remove this in favor of more granular
// attributes when the Rust port is completed.
#![expect(dead_code)]

use crate::virtio_gpu_abi::{ResourceFormat, ResourceId};
use std::num::NonZero;
use zx_sys::zx_paddr_t;

/// State associated with an image in an imported sysmem buffer.
#[derive(Debug)]
pub struct ImportedImage {
    physical_address: u64,
    resource_format: ResourceFormat,
    stride: NonZero<u32>,
    virtio_resource_id: ResourceId,

    /// Keeps the image's memory pinned.
    pmt: zx::Pmt,
}

impl ImportedImage {
    /// Creates an instance without an associated virtio resource ID.
    ///
    /// `bti` must be valid for the duration of the call. `image_vmo` must point to
    /// a valid VMO whose size is at least `image_vmo_offset` + `image_size`.
    /// `resource_format` must be a known format.
    pub fn new(
        bti: &zx::Bti,
        image_vmo: &zx::Vmo,
        image_vmo_offset: u64,
        image_size: NonZero<u64>,
        resource_format: ResourceFormat,
        stride: NonZero<u32>,
    ) -> Result<Self, zx::Status> {
        debug_assert!(!bti.is_invalid());
        debug_assert!(resource_format.is_known());

        let mut physical_addresses: Vec<zx_paddr_t> = vec![0];
        let page_size = zx::system_get_page_size() as u64;
        debug_assert_eq!(image_vmo_offset % page_size, 0);
        let pinned_size = (image_size.get() + page_size - 1) / page_size * page_size;
        let pmt = bti
            .pin(
                zx::BtiOptions::PERM_READ | zx::BtiOptions::CONTIGUOUS,
                image_vmo,
                image_vmo_offset,
                pinned_size,
                &mut physical_addresses,
            )
            .map_err(|e| {
                log::error!("Failed to pin image VMO: {:?}", e);
                e
            })?;

        let physical_address = physical_addresses[0] as u64;
        Ok(Self { physical_address, pmt, virtio_resource_id: None, resource_format, stride })
    }

    /// The starting physical address of the image's pixel data.
    ///
    /// The image's pixel data is stored in contiguous memory.
    ///
    /// The value is intended to be used in virtio-gpu commands.
    pub fn physical_address(&self) -> u64 {
        self.physical_address
    }

    /// The virtio resource ID used to attach this image.
    pub fn virtio_resource_id(&self) -> ResourceId {
        self.virtio_resource_id
    }

    #[expect(dead_code)]
    pub fn resource_format(&self) -> ResourceFormat {
        self.resource_format
    }

    #[expect(dead_code)]
    pub fn stride(&self) -> NonZero<u32> {
        self.stride
    }

    /// See `virtio_resource_id()` for details.
    ///
    /// `virtio_resource_id` must be attached while it is used in this instance.
    pub fn set_virtio_resource_id(&mut self, id: ResourceId) {
        // TODO(costan): Use a RAII handle for `virtio_resource_id` that
        // auto-detaches and releases the resource on destruction.
        // `VirtioImageResource` seems like a good name.

        self.virtio_resource_id = id;
    }

    /// Releases the pinned memory token, unpinning the image's memory.
    ///
    /// Dropping the [`Image`] instance does not unpin the image memory. This is
    /// intentional, because it's unsafe to unpin memory that may still be
    /// accessed by the display engine.
    ///
    /// SAFETY: The display engine hardware must no longer access the image.
    pub unsafe fn release(&mut self) {
        if !self.pmt.is_invalid() {
            let pmt = std::mem::replace(&mut self.pmt, zx::Pmt::invalid());

            // SAFETY: Precondition guarantees hardware is no longer accessing the memory.
            if let Err(status) = unsafe { pmt.unpin() } {
                log::warn!("Failed to unpin image memory: {:?}", status);
            }
        }
    }
}
