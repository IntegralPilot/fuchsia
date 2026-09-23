// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fsysmem2::{
    AllocatorAllocateSharedCollectionRequest, AllocatorBindSharedCollectionRequest,
    AllocatorSetDebugClientInfoRequest, BufferCollectionSetConstraintsRequest,
    BufferCollectionTokenDuplicateRequest, NodeSetNameRequest,
};

use fidl::endpoints::{Proxy, create_endpoints, create_proxy};
use fidl_fuchsia_hardware_display_types as fdisplay_types;
use fidl_fuchsia_images2::{self as fimages2};
use fidl_fuchsia_sysmem2::{
    self as fsysmem2, AllocatorMarker, BufferCollectionInfo, BufferCollectionMarker,
    BufferCollectionProxy, BufferCollectionTokenMarker, BufferCollectionTokenProxy,
};
use fuchsia_component::client::connect_to_protocol;

use crate::controller::Coordinator;
use crate::error::{Error, Result};
use crate::pixel_format::PixelFormat;
use crate::types::{BufferCollectionId, ImageId};

/// Input parameters for constructing an image.
#[derive(Clone)]
pub struct ImageParameters {
    /// The width dimension of the image, in pixels.
    pub width: u32,

    /// The height dimension of the image, in pixels.
    pub height: u32,

    /// Describes how individual pixels of the image will be interpreted. Determines the pixel
    /// stride for the image buffer.
    pub pixel_format: PixelFormat,

    /// The sysmem color space standard representation. The user must take care that `color_space`
    /// is compatible with the supplied `pixel_format`.
    pub color_space: fimages2::ColorSpace,

    /// Optional name to assign to the VMO that backs this image.
    pub name: Option<String>,
}

/// Represents an allocated image buffer that can be assigned to a display layer.
pub struct Image {
    /// The ID of the image provided to the display driver.
    pub id: ImageId,

    /// The ID of the sysmem buffer collection that backs this image.
    pub collection_id: BufferCollectionId,

    /// The VMO that contains the shared image buffer.
    pub vmo: zx::Vmo,

    /// The parameters that the image was initialized with.
    pub parameters: ImageParameters,

    /// The image format constraints that resulted from the sysmem buffer negotiation. Contains the
    /// effective image parameters.
    pub format_constraints: fsysmem2::ImageFormatConstraints,

    /// The effective buffer memory settings that resulted from the sysmem buffer negotiation.
    pub buffer_settings: fsysmem2::BufferMemorySettings,

    // The BufferCollection that backs this image.
    proxy: BufferCollectionProxy,

    // The display driver proxy that this image has been imported into.
    coordinator: Coordinator,
}

impl Image {
    /// Construct a new sysmem-buffer-backed image and register it with the display driver
    /// using `image_id`. If successful, the image can be assigned to a primary layer in a
    /// display configuration.
    pub async fn create(
        coordinator: Coordinator,
        image_id: ImageId,
        params: &ImageParameters,
    ) -> Result<Image> {
        let mut collection = allocate_image_buffer(coordinator.clone(), params).await?;
        coordinator.import_image(collection.id, image_id, params.into()).await?;
        let vmo = collection.info.buffers.as_ref().unwrap()[0]
            .vmo
            .as_ref()
            .ok_or(Error::BuffersNotAllocated)?
            .duplicate_handle(zx::Rights::SAME_RIGHTS)?;

        collection.release();

        Ok(Image {
            id: image_id,
            collection_id: collection.id,
            vmo,
            parameters: params.clone(),
            format_constraints: collection
                .info
                .settings
                .as_ref()
                .unwrap()
                .image_format_constraints
                .as_ref()
                .unwrap()
                .clone(),
            buffer_settings: collection
                .info
                .settings
                .as_mut()
                .unwrap()
                .buffer_settings
                .take()
                .unwrap(),
            proxy: collection.proxy.clone(),
            coordinator,
        })
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        let _ = self.proxy.release();
        let _ = self.coordinator.release_buffer_collection(self.collection_id);
    }
}

impl From<&ImageParameters> for fdisplay_types::ImageMetadata {
    fn from(src: &ImageParameters) -> Self {
        Self {
            dimensions: fidl_fuchsia_math::SizeU { width: src.width, height: src.height },
            tiling_type: fdisplay_types::IMAGE_TILING_TYPE_LINEAR,
        }
    }
}

impl From<ImageParameters> for fdisplay_types::ImageMetadata {
    fn from(src: ImageParameters) -> Self {
        fdisplay_types::ImageMetadata::from(&src)
    }
}

// Result of `allocate_image_buffer` that automatically releases the display driver's connection to
// the buffer collection unless `release()` is called on it. This is intended to clean up resources
// in the early-return cases above.
struct BufferCollection {
    id: BufferCollectionId,
    info: BufferCollectionInfo,
    proxy: BufferCollectionProxy,
    coordinator: Coordinator,
    released: bool,
}

impl BufferCollection {
    fn release(&mut self) {
        self.released = true;
    }
}

impl Drop for BufferCollection {
    fn drop(&mut self) {
        if !self.released {
            let _ = self.coordinator.release_buffer_collection(self.id);
            let _ = self.proxy.release();
        }
    }
}

// Allocate a sysmem buffer collection and register it with the display driver. The allocated
// buffer can be used to construct a display layer image.
async fn allocate_image_buffer(
    coordinator: Coordinator,
    params: &ImageParameters,
) -> Result<BufferCollection> {
    let allocator =
        connect_to_protocol::<AllocatorMarker>().map_err(|_| Error::SysmemConnection)?;
    {
        let name = fuchsia_runtime::process_self().get_name()?;
        let koid = fuchsia_runtime::process_self().koid()?;
        allocator.set_debug_client_info(&AllocatorSetDebugClientInfoRequest {
            name: Some(name.to_string()),
            id: Some(koid.raw_koid()),
            ..Default::default()
        })?;
    }
    let collection_token = {
        let (proxy, remote) = create_proxy::<BufferCollectionTokenMarker>();
        allocator.allocate_shared_collection(AllocatorAllocateSharedCollectionRequest {
            token_request: Some(remote),
            ..Default::default()
        })?;
        proxy
    };
    // TODO(armansito): The priority number here is arbitrary but I don't expect there to be
    // contention for the assigned name as this client library should be the collection's sole
    // owner. Still, come up with a better way to assign this.
    if let Some(ref name) = params.name {
        collection_token.set_name(&NodeSetNameRequest {
            priority: Some(100),
            name: Some(name.clone()),
            ..Default::default()
        })?;
    }

    // Duplicate of `collection_token` to be transferred to the display driver.
    let display_duplicate = {
        let (local, remote) = create_endpoints::<BufferCollectionTokenMarker>();
        collection_token.duplicate(BufferCollectionTokenDuplicateRequest {
            rights_attenuation_mask: Some(fidl::Rights::SAME_RIGHTS),
            token_request: Some(remote),
            ..Default::default()
        })?;
        collection_token.sync().await?;
        local
    };

    // Register the collection with the display driver.
    let id = coordinator.import_buffer_collection(display_duplicate).await?;

    // Tell sysmem to perform the buffer allocation and wait for the result. Clean up on error.
    match allocate_image_buffer_helper(params, allocator, collection_token).await {
        Ok((info, proxy)) => Ok(BufferCollection { id, info, proxy, coordinator, released: false }),
        Err(error) => {
            let _ = coordinator.release_buffer_collection(id);
            Err(error)
        }
    }
}

async fn allocate_image_buffer_helper(
    params: &ImageParameters,
    allocator: fsysmem2::AllocatorProxy,
    token: BufferCollectionTokenProxy,
) -> Result<(BufferCollectionInfo, BufferCollectionProxy)> {
    // Turn in the collection token to obtain a connection to the logical buffer collection.
    let collection = {
        let (local, remote) = create_endpoints::<BufferCollectionMarker>();
        let token_client = token.into_client_end().map_err(|_| Error::SysmemConnection)?;
        allocator.bind_shared_collection(AllocatorBindSharedCollectionRequest {
            token: Some(token_client),
            buffer_collection_request: Some(remote),
            ..Default::default()
        })?;
        local.into_proxy()
    };

    // Set local constraints and allocate buffers.
    collection.set_constraints(BufferCollectionSetConstraintsRequest {
        constraints: Some(buffer_collection_constraints(params)),
        ..Default::default()
    })?;
    let collection_info = {
        let response = collection
            .wait_for_all_buffers_allocated()
            .await?
            .map_err(|_| Error::BuffersNotAllocated)?;
        response.buffer_collection_info.ok_or(Error::BuffersNotAllocated)?
    };

    // We expect there to be at least one available vmo.
    if collection_info.buffers.as_ref().unwrap().is_empty() {
        collection.release()?;
        return Err(Error::BuffersNotAllocated);
    }

    Ok((collection_info, collection))
}

fn buffer_collection_constraints(
    params: &ImageParameters,
) -> fsysmem2::BufferCollectionConstraints {
    let usage = fsysmem2::BufferUsage {
        cpu: Some(fsysmem2::CPU_USAGE_READ_OFTEN | fsysmem2::CPU_USAGE_WRITE_OFTEN),
        ..Default::default()
    };

    let buffer_memory_constraints = fsysmem2::BufferMemoryConstraints {
        ram_domain_supported: Some(true),
        cpu_domain_supported: Some(true),
        ..Default::default()
    };

    // TODO(armansito): parameterize the format modifier
    let image_constraints = fsysmem2::ImageFormatConstraints {
        pixel_format: Some(params.pixel_format.into()),
        pixel_format_modifier: Some(fimages2::PixelFormatModifier::Linear),
        required_max_size: Some(fidl_fuchsia_math::SizeU {
            width: params.width,
            height: params.height,
        }),
        color_spaces: Some(vec![params.color_space]),
        ..Default::default()
    };

    let constraints = fsysmem2::BufferCollectionConstraints {
        min_buffer_count: Some(1),
        usage: Some(usage),
        buffer_memory_constraints: Some(buffer_memory_constraints),
        image_format_constraints: Some(vec![image_constraints]),
        ..Default::default()
    };

    constraints
}
