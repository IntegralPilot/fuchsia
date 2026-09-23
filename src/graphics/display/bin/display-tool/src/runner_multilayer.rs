// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

///! Implements a double-buffer swapchain runner to display a Scene with multiple layers.
use {
    anyhow::Result,
    display_utils::{
        Alpha, Coordinator, DisplayConfig, DisplayId, Image, ImageId, ImageParameters, Layer,
        LayerConfig, LayerId, PixelFormat, VsyncEvent,
    },
    fuchsia_trace::duration,
    futures::StreamExt,
    std::{borrow::Borrow, io::Write},
};

use crate::draw::MappedImage;
use crate::fps::Counter;

// ANSI X3.64 (ECMA-48) escape code for clearing the terminal screen.
const CLEAR: &str = "\x1B[2K\r";

// A scene whose contents may change over time and can be rendered into
// images mapped to the address space.
pub trait MultiLayerScene {
    // Update the scene contents.
    fn update(&mut self) -> Result<()>;

    // Initialize the image for rendering.
    // Invoked exactly once for every frame buffer image before it's used for
    // rendering for the first time.
    fn init_images(&self, images: &mut [MappedImage]) -> Result<()>;

    // Render the current scene contents to `images`.
    // `images` must not be used by the display engine during `render()`.
    fn render(&mut self, images: &mut [MappedImage]) -> Result<()>;

    /// Gets the alpha configuration for a specific layer.
    /// If None is returned, the layer's default alpha behavior is used.
    fn get_alpha_for_layer(&self, layer_index: usize) -> Option<Alpha>;
}

struct Presentation {
    images: Vec<MappedImage>,
}

impl Presentation {
    pub fn new(images: Vec<MappedImage>) -> Self {
        Presentation { images }
    }
}

pub struct MultiLayerFenceLoop<'a, S: MultiLayerScene> {
    coordinator: &'a Coordinator,
    display_id: DisplayId,
    layer_ids: Vec<LayerId>,
    params: ImageParameters,
    scene: S,
    presentations: Vec<Presentation>,
}

impl<'a, S: MultiLayerScene> MultiLayerFenceLoop<'a, S> {
    pub async fn new(
        coordinator: &'a Coordinator,
        display_id: DisplayId,
        width: u32,
        height: u32,
        pixel_format: PixelFormat,
        num_layers: usize,
        scene: S,
    ) -> Result<Self> {
        let params = ImageParameters {
            width,
            height,
            pixel_format,
            color_space: fidl_fuchsia_images2::ColorSpace::Srgb,
            name: Some("multilayer image".to_string()),
        };

        let mut layer_ids = Vec::new();
        for _ in 0..num_layers {
            layer_ids.push(coordinator.create_layer().await?);
        }

        const NUM_SWAPCHAIN_IMAGES: usize = 2;
        let mut presentations = Vec::new();
        for i in 0..NUM_SWAPCHAIN_IMAGES {
            let mut images = Vec::new();
            for j in 0..num_layers {
                let image_id = ImageId((1 + i * num_layers + j) as u64);
                let image = MappedImage::create(
                    Image::create(coordinator.clone(), image_id, &params).await?,
                )?;
                images.push(image);
            }
            presentations.push(Presentation::new(images));
        }

        for presentation in &mut presentations {
            scene.init_images(&mut presentation.images)?;
        }

        Ok(MultiLayerFenceLoop { coordinator, display_id, layer_ids, params, scene, presentations })
    }

    fn build_display_configs(&self, presentation_index: usize) -> Vec<DisplayConfig> {
        let presentation = &self.presentations[presentation_index];
        let layers: Vec<Layer> = self
            .layer_ids
            .iter()
            .zip(presentation.images.iter())
            .enumerate()
            .map(|(layer_index, (layer_id, image))| Layer {
                id: *layer_id,
                config: LayerConfig::Primary {
                    image_id: image.id(),
                    image_metadata: self.params.borrow().into(),
                    unblock_event: None,
                    alpha: self.scene.get_alpha_for_layer(layer_index),
                },
            })
            .collect();

        vec![DisplayConfig { id: self.display_id, layers }]
    }

    pub async fn run(&mut self) -> Result<()> {
        // Apply the first config.
        let mut current_config = 0;
        let _ = self.coordinator.commit_config(&self.build_display_configs(current_config)).await?;

        let mut vsync_listener = self.coordinator.add_vsync_listener(None)?;

        let mut counter = Counter::new();
        loop {
            // Log the frame rate.
            counter.add(zx::MonotonicInstant::get());
            let stats = counter.stats();
            print!(
                "{}Display {:.2} fps ({:.5} ms)",
                CLEAR, stats.sample_rate_hz, stats.sample_time_delta_ms
            );
            std::io::stdout().flush()?;

            // Prepare the next image.
            // `current_config` alternates between 0 and 1.
            current_config ^= 1;
            let current_presentation = &mut self.presentations[current_config];

            let applied_stamp; // Config stamp of the about-to-be-applied config.
            {
                duration!(c"gfx", c"frame", "id" => stats.num_frames);
                {
                    duration!(c"gfx", c"update scene");
                    self.scene.update()?;
                }

                // Render the scene into the current presentation.
                {
                    duration!(c"gfx", c"render frame", "image" => current_config as u32);
                    self.scene.render(&mut current_presentation.images)?;
                }

                // Request the swap.
                {
                    duration!(c"gfx", c"apply config");
                    applied_stamp = self
                        .coordinator
                        .commit_config(&self.build_display_configs(current_config))
                        .await?;
                }
            }
            // Wait for the previous frame image to retire before drawing on it.
            while let Some(VsyncEvent { id: _, timestamp: _, config }) = vsync_listener.next().await
            {
                if config.value == applied_stamp {
                    break;
                }
            }
        }
    }
}
