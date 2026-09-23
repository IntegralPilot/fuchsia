// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::FrameUsage;
use anyhow::{Context, Error, format_err};
use fidl::endpoints::{ClientEnd, Proxy, create_endpoints};
use fidl_fuchsia_images2::{ColorSpace, PixelFormat, PixelFormatModifier};
use fidl_fuchsia_sysmem2::{
    AllocatorAllocateSharedCollectionRequest, AllocatorBindSharedCollectionRequest,
    AllocatorMarker, AllocatorProxy, AllocatorSetDebugClientInfoRequest,
    BufferCollectionConstraints, BufferCollectionInfo, BufferCollectionMarker,
    BufferCollectionProxy, BufferCollectionSetConstraintsRequest,
    BufferCollectionTokenDuplicateRequest, BufferCollectionTokenMarker, BufferCollectionTokenProxy,
    BufferMemoryConstraints, BufferUsage, CPU_USAGE_READ_OFTEN, CPU_USAGE_WRITE_OFTEN,
    ImageFormatConstraints, NONE_USAGE, NodeSetNameRequest,
};
use fuchsia_component::client::connect_to_protocol;
use std::cmp;

fn linear_image_format_constraints(
    width: u32,
    height: u32,
    pixel_type: PixelFormat,
) -> ImageFormatConstraints {
    ImageFormatConstraints {
        pixel_format: Some(pixel_type),
        pixel_format_modifier: Some(PixelFormatModifier::Linear),
        color_spaces: Some(vec![ColorSpace::Srgb]),
        required_min_size: Some(fidl_fuchsia_math::SizeU { width, height }),
        required_max_size: Some(fidl_fuchsia_math::SizeU { width, height }),
        ..Default::default()
    }
}

fn buffer_memory_constraints(width: u32, height: u32) -> BufferMemoryConstraints {
    BufferMemoryConstraints {
        min_size_bytes: Some(width as u64 * height as u64 * 4u64),
        physically_contiguous_required: Some(false),
        secure_required: Some(false),
        ram_domain_supported: Some(true),
        cpu_domain_supported: Some(true),
        inaccessible_domain_supported: Some(false),
        ..Default::default()
    }
}

fn buffer_collection_constraints(
    width: u32,
    height: u32,
    pixel_format: PixelFormat,
    buffer_count: u32,
    frame_usage: FrameUsage,
) -> BufferCollectionConstraints {
    let (usage, has_buffer_memory_constraints, has_image_format_constraints) = match frame_usage {
        FrameUsage::Cpu => (
            BufferUsage {
                cpu: Some(CPU_USAGE_WRITE_OFTEN | CPU_USAGE_READ_OFTEN),
                ..Default::default()
            },
            true,
            true,
        ),
        FrameUsage::Gpu => {
            (BufferUsage { none: Some(NONE_USAGE), ..Default::default() }, false, false)
        }
    };
    BufferCollectionConstraints {
        usage: Some(usage),
        min_buffer_count: Some(buffer_count),
        buffer_memory_constraints: if has_buffer_memory_constraints {
            Some(buffer_memory_constraints(width, height))
        } else {
            None
        },
        image_format_constraints: if has_image_format_constraints {
            Some(vec![linear_image_format_constraints(width, height, pixel_format)])
        } else {
            None
        },
        ..Default::default()
    }
}

// See ImageFormatStrideBytesPerWidthPixel
fn stride_bytes_per_width_pixel(pixel_type: PixelFormat) -> Result<u32, Error> {
    match pixel_type {
        PixelFormat::R8G8B8A8 => Ok(4),
        PixelFormat::B8G8R8A8 => Ok(4),
        PixelFormat::B8G8R8 => Ok(3),
        PixelFormat::I420 => Ok(1),
        PixelFormat::M420 => Ok(1),
        PixelFormat::Nv12 => Ok(1),
        PixelFormat::Yuy2 => Ok(2),
        PixelFormat::Yv12 => Ok(1),
        PixelFormat::R5G6B5 => Ok(2),
        PixelFormat::R3G3B2 => Ok(1),
        PixelFormat::R2G2B2X2 => Ok(1),
        PixelFormat::L8 => Ok(1),
        _ => return Err(format_err!("Unsupported format")),
    }
}

fn round_up_to_align(x: u32, align: u32) -> u32 {
    if align == 0 { x } else { ((x + align - 1) / align) * align }
}

// See ImageFormatMinimumRowBytes
pub fn minimum_row_bytes(constraints: &ImageFormatConstraints, width: u32) -> Result<u32, Error> {
    if width < constraints.min_size.ok_or("missing min_size").unwrap().width
        || width > constraints.max_size.ok_or("missing max_size").unwrap().width
    {
        return Err(format_err!("Invalid width for constraints"));
    }

    let bytes_per_pixel = stride_bytes_per_width_pixel(
        constraints.pixel_format.ok_or("missing pixel_format").unwrap(),
    )?;
    Ok(round_up_to_align(
        cmp::max(
            bytes_per_pixel * width,
            constraints.min_bytes_per_row.ok_or("missing min_bytes_per_row").unwrap(),
        ),
        constraints.bytes_per_row_divisor.ok_or("missing bytes_per_row_divisor").unwrap(),
    ))
}

pub struct BufferCollectionAllocator {
    token: Option<BufferCollectionTokenProxy>,
    width: u32,
    height: u32,
    pixel_format: PixelFormat,
    usage: FrameUsage,
    buffer_count: usize,
    sysmem: AllocatorProxy,
    collection_client: Option<BufferCollectionProxy>,
}

pub fn set_allocator_name(sysmem_client: &AllocatorProxy) -> Result<(), Error> {
    Ok(sysmem_client.set_debug_client_info(&AllocatorSetDebugClientInfoRequest {
        name: Some(fuchsia_runtime::process_self().get_name()?.to_string()),
        id: Some(fuchsia_runtime::process_self().koid()?.raw_koid()),
        ..Default::default()
    })?)
}

impl BufferCollectionAllocator {
    pub fn new(
        width: u32,
        height: u32,
        pixel_format: PixelFormat,
        usage: FrameUsage,
        buffer_count: usize,
    ) -> Result<BufferCollectionAllocator, Error> {
        let sysmem = connect_to_protocol::<AllocatorMarker>()?;

        let _ = set_allocator_name(&sysmem);

        let (local_token, local_token_request) = create_endpoints::<BufferCollectionTokenMarker>();

        sysmem.allocate_shared_collection(AllocatorAllocateSharedCollectionRequest {
            token_request: Some(local_token_request),
            ..Default::default()
        })?;

        Ok(BufferCollectionAllocator {
            token: Some(local_token.into_proxy()),
            width,
            height,
            pixel_format,
            usage,
            buffer_count,
            sysmem,
            collection_client: None,
        })
    }

    pub fn set_name(&mut self, priority: u32, name: &str) -> Result<(), Error> {
        Ok(self.token.as_ref().expect("token in set_name").set_name(&NodeSetNameRequest {
            priority: Some(priority),
            name: Some(name.into()),
            ..Default::default()
        })?)
    }

    pub fn set_pixel_type(&mut self, pixel_format: PixelFormat) {
        self.pixel_format = pixel_format;
    }

    pub async fn allocate_buffers(
        &mut self,
        set_constraints: bool,
    ) -> Result<BufferCollectionInfo, Error> {
        let token = self.token.take().expect("token in allocate_buffers");
        let (collection_client, collection_request) = create_endpoints::<BufferCollectionMarker>();
        self.sysmem.bind_shared_collection(AllocatorBindSharedCollectionRequest {
            token: Some(token.into_client_end().unwrap()),
            buffer_collection_request: Some(collection_request),
            ..Default::default()
        })?;
        let collection_client = collection_client.into_proxy();
        self.allocate_buffers_proxy(collection_client, set_constraints).await
    }

    async fn allocate_buffers_proxy(
        &mut self,
        collection_client: BufferCollectionProxy,
        set_constraints: bool,
    ) -> Result<BufferCollectionInfo, Error> {
        let buffer_collection_constraints = buffer_collection_constraints(
            self.width,
            self.height,
            self.pixel_format,
            self.buffer_count as u32,
            self.usage,
        );
        collection_client
            .set_constraints(BufferCollectionSetConstraintsRequest {
                constraints: if set_constraints {
                    Some(buffer_collection_constraints)
                } else {
                    None
                },
                ..Default::default()
            })
            .context("Sending buffer constraints to sysmem")?;
        let wait_result = collection_client.wait_for_all_buffers_allocated().await;
        self.collection_client = Some(collection_client);
        if wait_result.is_err() {
            let error: fidl::Error = wait_result.unwrap_err();
            return Err(format_err!("Failed to wait for buffers {}", error));
        }
        if wait_result.as_ref().unwrap().is_err() {
            let error: fidl_fuchsia_sysmem2::Error = wait_result.unwrap().unwrap_err();
            return Err(format_err!("Wait for buffers failed {:?}", error));
        }
        let buffers = wait_result.unwrap().unwrap().buffer_collection_info.unwrap();
        Ok(buffers)
    }

    pub async fn duplicate_token(
        &mut self,
    ) -> Result<ClientEnd<BufferCollectionTokenMarker>, Error> {
        let (requested_token, requested_token_request) =
            create_endpoints::<BufferCollectionTokenMarker>();

        self.token.as_ref().expect("token in duplicate_token[duplicate]").duplicate(
            BufferCollectionTokenDuplicateRequest {
                rights_attenuation_mask: Some(fidl::Rights::SAME_RIGHTS),
                token_request: Some(requested_token_request),
                ..Default::default()
            },
        )?;
        self.token.as_ref().expect("token in duplicate_token_2[sync]").sync().await?;
        Ok(requested_token)
    }
}

impl Drop for BufferCollectionAllocator {
    fn drop(&mut self) {
        if let Some(collection_client) = self.collection_client.as_mut() {
            collection_client
                .release()
                .unwrap_or_else(|err| eprintln!("collection_client.release failed with {}", err));
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use fidl_fuchsia_sysmem2::{
        BufferCollectionRequest, BufferCollectionWaitForAllBuffersAllocatedResponse,
        BufferMemorySettings, CoherencyDomain, Heap, SingleBufferSettings, VmoBuffer,
    };
    use fuchsia_async as fasync;
    use futures::prelude::*;

    const BUFFER_COUNT: usize = 3;

    fn spawn_allocator_server() -> Result<AllocatorProxy, Error> {
        let (proxy, mut stream) = fidl::endpoints::create_proxy_and_stream::<AllocatorMarker>();

        fasync::Task::spawn(async move {
            while let Some(_) = stream.try_next().await.expect("Failed to get request") {}
        })
        .detach();
        Ok(proxy)
    }

    fn spawn_buffer_collection() -> Result<BufferCollectionProxy, Error> {
        let (proxy, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<BufferCollectionMarker>();

        fasync::Task::spawn(async move {
            let mut stored_constraints = None;
            while let Some(req) = stream.try_next().await.expect("Failed to get request") {
                match req {
                    BufferCollectionRequest::SetConstraints { payload, control_handle: _ } => {
                        stored_constraints = payload.constraints;
                    }
                    BufferCollectionRequest::WaitForAllBuffersAllocated { responder } => {
                        let constraints =
                            stored_constraints.take().expect("Expected SetConstraints first");
                        let mut buffers: Vec<VmoBuffer> = vec![];
                        for _ in 0..*constraints.min_buffer_count.as_ref().unwrap() {
                            buffers.push(fidl_fuchsia_sysmem2::VmoBuffer { ..Default::default() });
                        }
                        let buffer_collection_info = BufferCollectionInfo {
                            settings: Some(SingleBufferSettings {
                                buffer_settings: Some(BufferMemorySettings {
                                    size_bytes: Some(0),
                                    is_physically_contiguous: Some(false),
                                    is_secure: Some(false),
                                    coherency_domain: Some(CoherencyDomain::Cpu),
                                    heap: Some(Heap {
                                        heap_type: Some(
                                            bind_fuchsia_sysmem_heap::HEAP_TYPE_SYSTEM_RAM.into(),
                                        ),
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                }),
                                image_format_constraints: Some(linear_image_format_constraints(
                                    0,
                                    0,
                                    PixelFormat::Invalid,
                                )),
                                ..Default::default()
                            }),
                            buffers: Some(buffers),
                            ..Default::default()
                        };
                        let response = BufferCollectionWaitForAllBuffersAllocatedResponse {
                            buffer_collection_info: Some(buffer_collection_info),
                            ..Default::default()
                        };
                        responder.send(Ok(response)).expect("Failed to send");
                    }
                    _ => panic!("Unexpected request"),
                }
            }
        })
        .detach();

        return Ok(proxy);
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_buffer_collection_allocator() -> std::result::Result<(), anyhow::Error> {
        let alloc_proxy = spawn_allocator_server()?;
        let buf_proxy = spawn_buffer_collection()?;

        // don't use new() as we want to inject alloc_proxy instead of discovering it.
        let mut bca = BufferCollectionAllocator {
            token: None,
            width: 200,
            height: 200,
            pixel_format: PixelFormat::B8G8R8A8,
            usage: FrameUsage::Cpu,
            buffer_count: BUFFER_COUNT,
            sysmem: alloc_proxy,
            collection_client: None,
        };

        let buffers = bca.allocate_buffers_proxy(buf_proxy, true).await?;
        assert_eq!(buffers.buffers.unwrap().len(), BUFFER_COUNT);

        Ok(())
    }
}
