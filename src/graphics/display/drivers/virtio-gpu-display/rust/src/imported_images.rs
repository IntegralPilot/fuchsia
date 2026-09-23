// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://fxbug.dev/504722357): Remove this in favor of more granular
// attributes when the Rust port is completed.
#![allow(dead_code)]

use crate::imported_image::ImportedImage;
use fidl_next_fuchsia_hardware_display_engine as fidl_display_engine;
use fidl_next_fuchsia_images2 as fidl_images2;
use fidl_next_fuchsia_math as fidl_math;
use fidl_next_fuchsia_sysmem2 as fidl_sysmem2;
use fuchsia_runtime;
use log;
use std::collections::HashMap;
use std::num::NonZero;

fn zx_status_from_sysmem_error(_error: fidl_sysmem2::Error) -> zx::Status {
    // TODO(https://fxbug.dev/518838992): Switch to an intentionally designed mapping.
    zx::Status::INTERNAL
}

/// Information relevant to this driver for an item in a sysmem BufferCollection.
pub struct SysmemBufferInfo {
    pub image_vmo: zx::Vmo,
    pub image_vmo_offset: u64,

    pub pixel_format: fidl_images2::PixelFormat,
    pub pixel_format_modifier: fidl_images2::PixelFormatModifier,

    pub minimum_size: fidl_math::SizeU,

    pub minimum_bytes_per_row: u32,
    pub bytes_per_row_divisor: NonZero<u32>,

    pub coherency_domain: fidl_sysmem2::CoherencyDomain,
}

impl SysmemBufferInfo {
    // Obtains the relevant information from sysmem.
    pub async fn new(
        sysmem_buffer_collection: &mut fidl_next::Client<fidl_sysmem2::BufferCollection>,
        buffer_index: u32,
    ) -> Result<Self, zx::Status> {
        // Ensure that the wait_for_all_buffers_allocated() call below will
        // return quickly.
        let check_result = sysmem_buffer_collection
            .check_all_buffers_allocated()
            .await
            .map_err(|_| zx::Status::INTERNAL)?;

        match check_result {
            Ok(_) => {}
            Err(fidl_sysmem2::Error::Pending) => {
                return Err(zx::Status::SHOULD_WAIT);
            }
            Err(sysmem_error) => {
                return Err(zx_status_from_sysmem_error(sysmem_error));
            }
        }

        let wait_result = sysmem_buffer_collection
            .wait_for_all_buffers_allocated()
            .await
            .map_err(|_| zx::Status::INTERNAL)?;

        let mut buffer_collection_info = match wait_result {
            Ok(info) => info.buffer_collection_info.unwrap(),
            Err(sysmem_error) => {
                return Err(zx_status_from_sysmem_error(sysmem_error));
            }
        };

        let image_format_constraints = buffer_collection_info
            .settings
            .as_ref()
            .expect("Sysmem deviated from its contract")
            .image_format_constraints
            .as_ref()
            .ok_or_else(|| {
                log::warn!("Rejecting BufferCollection without ImageFormatConstraints");
                zx::Status::INVALID_ARGS
            })?;

        let pixel_format =
            image_format_constraints.pixel_format.expect("Sysmem deviated from its contract");
        let pixel_format_modifier = image_format_constraints
            .pixel_format_modifier
            .expect("Sysmem deviated from its contract");
        let minimum_size =
            image_format_constraints.min_size.expect("Sysmem deviated from its contract");
        let minimum_bytes_per_row =
            image_format_constraints.min_bytes_per_row.expect("Sysmem deviated from its contract");
        let bytes_per_row_divisor = image_format_constraints.bytes_per_row_divisor.unwrap_or(1);
        let bytes_per_row_divisor = NonZero::<u32>::new(bytes_per_row_divisor)
            .expect("Sysmem deviated from its contract and explicitly set the divisor to 0");

        let buffers =
            buffer_collection_info.buffers.as_mut().expect("Sysmem deviated from its contract");
        let buffer_index_usize = buffer_index as usize;
        debug_assert!(
            buffer_index_usize < buffers.len(),
            "buffer_index {} out of range (buffers count {})",
            buffer_index,
            buffers.len()
        );
        let buffer = &mut buffers[buffer_index_usize];
        let image_vmo = buffer.vmo.take().expect("Sysmem deviated from its contract");
        let image_vmo_offset = buffer.vmo_usable_start.expect("Sysmem deviated from its contract");

        let coherency_domain = buffer_collection_info
            .settings
            .as_ref()
            .expect("Sysmem deviated from its contract")
            .buffer_settings
            .as_ref()
            .expect("Sysmem deviated from its contract")
            .coherency_domain
            .expect("Sysmem deviated from its contract");

        let sysmem_info = SysmemBufferInfo {
            image_vmo,
            pixel_format,
            image_vmo_offset,
            pixel_format_modifier,
            minimum_size,
            minimum_bytes_per_row,
            bytes_per_row_divisor,
            coherency_domain,
        };

        Ok(sysmem_info)
    }
}

struct ImportedImageData {
    pub sysmem_info: SysmemBufferInfo,
    pub image: Option<ImportedImage>,
}

/// Facilitates debugging sysmem resource leaks.
async fn initialize_sysmem_debug_info(sysmem: &mut fidl_next::Client<fidl_sysmem2::Allocator>) {
    let process_koid =
        fuchsia_runtime::process_self().koid().expect("Failed to get the current process koid");

    let debug_name = format!("virtio-gpu-display[{}]", process_koid.raw_koid());
    let debug_info_request = fidl_sysmem2::AllocatorSetDebugClientInfoRequest {
        name: Some(debug_name),
        id: Some(process_koid.raw_koid()),
        ..Default::default()
    };
    let debug_info_result = sysmem.set_debug_client_info_with(debug_info_request).await;
    if let Err(e) = debug_info_result {
        log::warn!("Failed to set sysmem debug info: {:?}", e);
    }
}

/// Manages a display engine's collection of imported images.
///
/// Instances are not thread-safe, and must be used on a single thread or
/// synchronized dispatcher.
pub struct ImportedImages {
    scope: fuchsia_async::ScopeHandle,
    sysmem: fidl_next::Client<fidl_sysmem2::Allocator>,
    buffer_collections: HashMap<
        fidl_display_engine::BufferCollectionId,
        fidl_next::Client<fidl_sysmem2::BufferCollection>,
    >,
    images_data: HashMap<fidl_display_engine::ImageId, ImportedImageData>,
    next_image_id: fidl_display_engine::ImageId,
}

impl ImportedImages {
    /// Returns an empty collection of images.
    ///
    /// `scope` must outlive this instance. `sysmem` must be valid.
    pub async fn new(
        scope: fuchsia_async::ScopeHandle,
        mut sysmem: fidl_next::Client<fidl_sysmem2::Allocator>,
    ) -> Result<Self, zx::Status> {
        initialize_sysmem_debug_info(&mut sysmem).await;

        Ok(Self {
            scope,
            sysmem,
            buffer_collections: HashMap::new(),
            images_data: HashMap::new(),
            next_image_id: fidl_display_engine::ImageId { value: 1 },
        })
    }

    /// Similar contract to [`fuchsia.hardware.display.engine/Engine.ImportBufferCollection`].
    pub async fn import_buffer_collection(
        &mut self,
        buffer_collection_id: fidl_display_engine::BufferCollectionId,
        token: fidl_next::ClientEnd<fidl_sysmem2::BufferCollectionToken>,
    ) -> Result<(), zx::Status> {
        debug_assert!(
            !self.buffer_collections.contains_key(&buffer_collection_id),
            "Duplicate BufferCollection ID: {:?}",
            buffer_collection_id,
        );

        let (collection_client_end, collection_server_end) =
            fidl_next::fuchsia::create_channel::<fidl_sysmem2::BufferCollection>();

        let bind_request = fidl_sysmem2::AllocatorBindSharedCollectionRequest {
            token: Some(token),
            buffer_collection_request: Some(collection_server_end),
            ..Default::default()
        };
        self.sysmem
            .bind_shared_collection_with(bind_request)
            .await
            .map_err(|_| zx::Status::INTERNAL)?;

        let (tx, rx) = futures::channel::oneshot::channel();
        self.scope.spawn(async move {
            let _ = tx.send(collection_client_end.spawn());
        });
        let collection = rx.await.map_err(|_| zx::Status::INTERNAL)?;

        self.buffer_collections.insert(buffer_collection_id, collection);
        Ok(())
    }

    /// Similar contract to [`fuchsia.hardware.display.engine/Engine.ReleaseBufferCollection`].
    ///
    /// The method is asynchronous because it cleanly shuts down the collection's sysmem FIDL
    /// client.
    pub async fn release_buffer_collection(
        &mut self,
        buffer_collection_id: &fidl_display_engine::BufferCollectionId,
    ) {
        let Some(collection) = self.buffer_collections.remove(buffer_collection_id) else {
            return;
        };
        let _ = collection.release().await;
    }

    /// Similar contract to [`fuchsia.hardware.display.engine/Engine.ImportImage`].
    ///
    /// Upon success, [`find_sysmem_info()`] will return the image buffer
    /// information retrieved from sysmem, and [`find_image()`] will return an
    /// empty instance. The driver code calling this method should check that
    /// the sysmem buffer and image constraints are acceptable, and should then
    /// populate the `ImportedImage` instance with valid data.
    pub async fn import_image(
        &mut self,
        buffer_collection_id: fidl_display_engine::BufferCollectionId,
        buffer_index: u32,
    ) -> Result<fidl_display_engine::ImageId, zx::Status> {
        let buffer_collection =
            self.buffer_collections.get_mut(&buffer_collection_id).ok_or(zx::Status::NOT_FOUND)?;

        let sysmem_info = SysmemBufferInfo::new(buffer_collection, buffer_index).await?;

        let image_id = self.next_image_id;
        self.next_image_id.value += 1;

        self.images_data.insert(image_id, ImportedImageData { sysmem_info, image: None });

        Ok(image_id)
    }

    /// Similar contract to [`fuchsia.hardware.display.engine/Engine.ReleaseImage`].
    ///
    /// SAFETY: The display engine hardware must no longer access the image.
    /// This can happen for two reasons:
    ///
    /// 1. The image is not in the display engine's latched configuration.
    /// 2. The display engine hardware is stopped.
    // TODO(https://fxbug.dev/547974944): Make sure we release all images on driver shutdown,
    // after we successfully turn off the virtio device.
    pub unsafe fn release_image(
        &mut self,
        id: fidl_display_engine::ImageId,
    ) -> Result<(), zx::Status> {
        let mut image_data = self.images_data.remove(&id).ok_or(zx::Status::NOT_FOUND)?;
        if let Some(mut image) = image_data.image.take() {
            // SAFETY: The called method has the same invariant as this method.
            unsafe {
                image.release();
            }
        }
        Ok(())
    }

    /// Returns [`None`] if no collection with the given ID exists.
    pub fn find_buffer_collection(
        &self,
        buffer_collection_id: &fidl_display_engine::BufferCollectionId,
    ) -> Option<&fidl_next::Client<fidl_sysmem2::BufferCollection>> {
        self.buffer_collections.get(buffer_collection_id)
    }

    /// Returns [`None`] if no imported image with the given ID exists.
    pub fn find_sysmem_info(
        &self,
        image_id: fidl_display_engine::ImageId,
    ) -> Option<&SysmemBufferInfo> {
        self.images_data.get(&image_id).map(|image_data| &image_data.sysmem_info)
    }

    /// Returns [`None`] if no imported image with the given ID exists.
    pub fn find_image(&self, image_id: fidl_display_engine::ImageId) -> Option<&ImportedImage> {
        self.images_data.get(&image_id).and_then(|image_data| image_data.image.as_ref())
    }

    /// Panics if no image with the given ID exists.
    pub fn set_image(&mut self, image_id: fidl_display_engine::ImageId, image: ImportedImage) {
        let image_data = self.images_data.get_mut(&image_id).expect("No image with the given ID");
        image_data.image = Some(image);
    }
}
