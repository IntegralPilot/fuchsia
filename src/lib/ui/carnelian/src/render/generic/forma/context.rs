// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::cell::RefCell;
use std::collections::HashMap;

use std::{mem, ptr};

use anyhow::Error;
use display_utils::PixelFormat;
use euclid::default::{Rect, Size2D};
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_images2::{ColorSpace, PixelFormat as Images2PixelFormat, PixelFormatModifier};
use fidl_fuchsia_math::SizeU;
use fidl_fuchsia_sysmem2::{
    AllocatorBindSharedCollectionRequest, AllocatorMarker, BufferCollectionConstraints,
    BufferCollectionMarker, BufferCollectionSetConstraintsRequest,
    BufferCollectionSynchronousProxy, BufferCollectionTokenMarker, BufferMemoryConstraints,
    BufferUsage, CPU_USAGE_WRITE_OFTEN, CoherencyDomain, ImageFormatConstraints,
};
use fuchsia_component::client::connect_to_protocol;
use fuchsia_framebuffer::sysmem::set_allocator_name;
use fuchsia_trace::{duration_begin, duration_end};
use zx::sys;

use crate::ViewAssistantContext;
use crate::drawing::DisplayRotation;
use crate::render::generic::forma::image::VmoImage;
use crate::render::generic::forma::{
    Forma, FormaComposition, FormaImage, FormaPathBuilder, FormaRasterBuilder,
};
use crate::render::generic::{Context, CopyRegion, PostCopy, PreClear, PreCopy, RenderExt};

fn buffer_collection_constraints(width: u32, height: u32) -> BufferCollectionConstraints {
    let image_format_constraints = ImageFormatConstraints {
        pixel_format: Some(Images2PixelFormat::B8G8R8A8),
        pixel_format_modifier: Some(PixelFormatModifier::Linear),
        color_spaces: Some(vec![ColorSpace::Srgb]),
        min_size: Some(SizeU { width, height }),
        min_bytes_per_row: Some(width * mem::size_of::<u32>() as u32),
        ..Default::default()
    };

    BufferCollectionConstraints {
        usage: Some(BufferUsage { cpu: Some(CPU_USAGE_WRITE_OFTEN), ..Default::default() }),
        min_buffer_count: Some(1),
        buffer_memory_constraints: Some(BufferMemoryConstraints {
            min_size_bytes: Some(width as u64 * height as u64 * mem::size_of::<u32>() as u64),
            ram_domain_supported: Some(true),
            cpu_domain_supported: Some(true),
            inaccessible_domain_supported: Some(false),
            ..Default::default()
        }),
        image_format_constraints: Some(vec![image_format_constraints]),
        ..Default::default()
    }
}

fn copy_region_to_image(
    src_ptr: *mut u8,
    src_width: usize,
    src_height: usize,
    src_bytes_per_row: usize,
    dst_ptr: *mut u8,
    dst_len: usize,
    dst_bytes_per_row: usize,
    dst_coherency_domain: CoherencyDomain,
    region: &CopyRegion,
) {
    let (mut y, dy) = if region.dst_offset.y < region.src_offset.y {
        // Copy forward.
        (0, 1)
    } else {
        // Copy backwards.
        (region.extent.height as i32 - 1, -1)
    };

    let mut extent = region.extent.height;
    while extent > 0 {
        let src_y = (region.src_offset.y + y as u32) % src_height as u32;
        let dst_y = region.dst_offset.y + y as u32;

        let mut src_x = region.src_offset.x as usize;
        let mut dst_x = region.dst_offset.x as usize;
        let mut width = region.extent.width as usize;

        while width > 0 {
            let columns = width.min(src_width - src_x);
            let src_offset = src_y as usize * src_bytes_per_row + src_x * 4;
            let dst_offset = dst_y as usize * dst_bytes_per_row + dst_x * 4;

            assert!((dst_offset + (columns * 4)) <= dst_len);
            let src = (src_ptr as usize).checked_add(src_offset).unwrap() as *mut u8;
            let dst = (dst_ptr as usize).checked_add(dst_offset).unwrap() as *mut u8;
            unsafe {
                ptr::copy(src, dst, (columns * 4) as usize);
                if dst_coherency_domain == CoherencyDomain::Ram {
                    sys::zx_cache_flush(dst, columns * 4, sys::ZX_CACHE_FLUSH_DATA);
                }
            }

            width -= columns;
            dst_x += columns;
            src_x = 0;
        }

        y += dy;
        extent -= 1;
    }
}

#[derive(Debug)]
pub struct FormaContext {
    renderer: forma::CpuRenderer,
    buffer_collection: Option<BufferCollectionSynchronousProxy>,
    size: Size2D<u32>,
    display_rotation: DisplayRotation,
    images: Vec<RefCell<VmoImage>>,
    index_map: HashMap<u32, usize>,
    composition_id: usize,
}

impl FormaContext {
    pub(crate) fn new(
        token: ClientEnd<BufferCollectionTokenMarker>,
        size: Size2D<u32>,
        display_rotation: DisplayRotation,
    ) -> Self {
        let sysmem = connect_to_protocol::<AllocatorMarker>().expect("failed to connect to sysmem");
        set_allocator_name(&sysmem).unwrap_or_else(|e| eprintln!("set_allocator_name: {:?}", e));
        let (collection_client, collection_request) =
            fidl::endpoints::create_endpoints::<BufferCollectionMarker>();
        sysmem
            .bind_shared_collection(AllocatorBindSharedCollectionRequest {
                token: Some(token),
                buffer_collection_request: Some(collection_request),
                ..Default::default()
            })
            .expect("failed to bind shared collection");
        let buffer_collection = collection_client.into_sync_proxy();
        let constraints = buffer_collection_constraints(size.width, size.height);
        buffer_collection
            .set_constraints(BufferCollectionSetConstraintsRequest {
                constraints: Some(constraints),
                ..Default::default()
            })
            .expect("failed to set constraints on sysmem buffer");

        Self {
            renderer: forma::CpuRenderer::new(),
            buffer_collection: Some(buffer_collection),
            size,
            display_rotation,
            images: vec![],
            index_map: HashMap::new(),
            composition_id: 0,
        }
    }

    pub(crate) fn without_token(size: Size2D<u32>, display_rotation: DisplayRotation) -> Self {
        Self {
            renderer: forma::CpuRenderer::new(),
            buffer_collection: None,
            size,
            display_rotation,
            images: vec![],
            index_map: HashMap::new(),
            composition_id: 0,
        }
    }
}

impl Context<Forma> for FormaContext {
    fn pixel_format(&self) -> PixelFormat {
        PixelFormat::R8G8B8A8
    }

    fn path_builder(&self) -> Option<FormaPathBuilder> {
        Some(FormaPathBuilder::new())
    }

    fn raster_builder(&self) -> Option<FormaRasterBuilder> {
        Some(FormaRasterBuilder::new())
    }

    fn new_image(&mut self, size: Size2D<u32>) -> FormaImage {
        let image = FormaImage(self.images.len());
        self.images.push(RefCell::new(VmoImage::new(size.width, size.height)));

        image
    }

    fn new_image_from_png<R: std::io::BufRead + std::io::Seek>(
        &mut self,
        reader: &mut png::Reader<R>,
    ) -> Result<FormaImage, Error> {
        let image = FormaImage(self.images.len());
        self.images.push(RefCell::new(VmoImage::from_png(reader)?));

        Ok(image)
    }

    fn get_image(&mut self, image_index: u32) -> FormaImage {
        let buffer_collection = self.buffer_collection.as_mut().expect("buffer_collection");
        let images = &mut self.images;
        let width = self.size.width;
        let height = self.size.height;

        let index = self.index_map.entry(image_index).or_insert_with(|| {
            let index = images.len();
            images.push(RefCell::new(VmoImage::from_buffer_collection(
                buffer_collection,
                width,
                height,
                image_index,
            )));

            index
        });

        FormaImage(*index)
    }

    fn get_current_image(&mut self, context: &ViewAssistantContext) -> FormaImage {
        self.get_image(context.image_index)
    }

    fn render_with_clip(
        &mut self,
        composition: &mut FormaComposition,
        clip: Rect<u32>,
        image: FormaImage,
        ext: &RenderExt<Forma>,
    ) {
        let image_id = image;
        let mut image = self
            .images
            .get(image.0 as usize)
            .unwrap_or_else(|| panic!("invalid image {:?}", image_id))
            .borrow_mut();
        let width = self.size.width as usize;
        let height = self.size.height as usize;

        if let Some(PreClear { color }) = ext.pre_clear {
            image.clear([color.b, color.g, color.r, color.a]);
        }

        if let Some(PreCopy { image: src_image_id, copy_region }) = ext.pre_copy {
            let dst_coherency_domain = image.coherency_domain();
            let dst_slice = image.as_mut_slice();
            let dst_ptr = dst_slice.as_mut_ptr();
            let dst_len = dst_slice.len();
            let dst_bytes_per_row = image.bytes_per_row();

            let src_image = self
                .images
                .get(src_image_id.0 as usize)
                .unwrap_or_else(|| panic!("invalid PreCopy image {:?}", src_image_id))
                .try_borrow_mut();

            let (src_ptr, src_bytes_per_row) = match src_image {
                Ok(mut image) => (image.as_mut_slice().as_mut_ptr(), image.bytes_per_row()),
                Err(_) => (image.as_mut_slice().as_mut_ptr(), image.bytes_per_row()),
            };

            copy_region_to_image(
                src_ptr,
                width,
                height,
                src_bytes_per_row,
                dst_ptr,
                dst_len,
                dst_bytes_per_row,
                dst_coherency_domain,
                &copy_region,
            );
        }

        let transform = self.display_rotation.transform(&self.size.to_f32());
        composition.set_cached_display_transform(transform);

        if composition.id.is_none() {
            let next_id = self.composition_id + 1;
            composition.id = Some(mem::replace(&mut self.composition_id, next_id));
        }

        if image.buffer_layer_cache.as_ref().map(|&(id, _)| id) != composition.id {
            image.buffer_layer_cache = self
                .renderer
                .create_buffer_layer_cache()
                .map(|cache| (composition.id.unwrap(), cache));
        }

        duration_begin!(c"gfx", c"render::Context<Forma>::render_composition");
        self.renderer.render(
            &mut composition.composition,
            &mut image.as_buffer(),
            forma::BGRA,
            forma::Color::from(&composition.background_color),
            Some(forma::Rect::new(
                clip.origin.x as usize..(clip.origin.x + clip.size.width) as usize,
                clip.origin.y as usize..(clip.origin.y + clip.size.height) as usize,
            )),
        );
        duration_end!(c"gfx", c"render::Context<Forma>::render_composition");

        // TODO: Motion blur support.
        if let Some(PostCopy { image: dst_image_id, copy_region, .. }) = ext.post_copy {
            let mut dst_image = self
                .images
                .get(dst_image_id.0 as usize)
                .unwrap_or_else(|| panic!("invalid PostCopy image {:?}", dst_image_id))
                .try_borrow_mut()
                .unwrap_or_else(|e| {
                    panic!("image {:?} as already used for rendering: {:?}", dst_image_id, e)
                });

            let src_bytes_per_row = image.bytes_per_row();
            let dst_bytes_per_row = dst_image.bytes_per_row();
            let src_slice = image.as_mut_slice();
            let dst_coherency_domain = dst_image.coherency_domain();
            let dst_slice = dst_image.as_mut_slice();

            copy_region_to_image(
                src_slice.as_mut_ptr(),
                width,
                height,
                src_bytes_per_row,
                dst_slice.as_mut_ptr(),
                dst_slice.len(),
                dst_bytes_per_row,
                dst_coherency_domain,
                &copy_region,
            );
        }
    }
}
