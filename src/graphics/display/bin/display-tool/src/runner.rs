// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

///! Implements a double-buffer swapchain runner to display a Scene. The swapchain is represented
///! by two images that alternate in their assignment to a primary layer. Writes to each buffer and
///! the resulting swap is synchronized using each configuration's retirement fence, which is
///! aligned to the display's vsync events by the display driver.
use {
    anyhow::Result,
    display_utils::{
        Coordinator, DisplayConfig, DisplayId, Image, ImageId, ImageParameters, Layer, LayerConfig,
        LayerId, PixelFormat, VsyncEvent,
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
pub trait Scene {
    // Update the scene contents.
    fn update(&mut self) -> Result<()>;

    // Initialize the image for rendering.
    // Invoked exactly once for every frame buffer image before it's used for
    // rendering for the first time.
    fn init_image(&self, image: &mut MappedImage) -> Result<()>;

    // Render the current scene contents to `image`.
    // `image` must not be used by the display engine during `render()`.
    fn render(&mut self, image: &mut MappedImage) -> Result<()>;
}

struct Presentation {
    image: MappedImage,
}

impl Presentation {
    pub fn new(image: MappedImage) -> Self {
        Presentation { image }
    }
}

pub struct DoubleBufferedFenceLoop<'a, S: Scene> {
    coordinator: &'a Coordinator,
    display_id: DisplayId,
    layer_id: LayerId,

    params: ImageParameters,

    scene: S,
    presentations: Vec<Presentation>,
}

impl<'a, S: Scene> DoubleBufferedFenceLoop<'a, S> {
    pub async fn new(
        coordinator: &'a Coordinator,
        display_id: DisplayId,
        width: u32,
        height: u32,
        pixel_format: PixelFormat,
        scene: S,
    ) -> Result<Self> {
        let params = ImageParameters {
            width,
            height,
            pixel_format,
            color_space: fidl_fuchsia_images2::ColorSpace::Srgb,
            name: Some("image layer".to_string()),
        };
        let mut next_image_id = ImageId(1);

        const NUM_SWAPCHAIN_IMAGES: usize = 2;
        let mut image_presentations = Vec::new();
        for _ in 0..NUM_SWAPCHAIN_IMAGES {
            next_image_id = ImageId(next_image_id.0 + 1);

            let mut image = MappedImage::create(
                Image::create(coordinator.clone(), next_image_id, &params).await?,
            )?;
            scene.init_image(&mut image)?;
            image_presentations.push(Presentation::new(image));
        }

        let layer_id = coordinator.create_layer().await?;

        Ok(DoubleBufferedFenceLoop {
            coordinator,
            display_id,
            layer_id,
            params,

            scene,
            presentations: image_presentations,
        })
    }

    fn build_display_configs(&self, presentation_index: usize) -> Vec<DisplayConfig> {
        let presentation = &self.presentations[presentation_index];
        vec![DisplayConfig {
            id: self.display_id,
            layers: vec![Layer {
                id: self.layer_id,
                config: LayerConfig::Primary {
                    image_id: presentation.image.id(),
                    image_metadata: self.params.borrow().into(),
                    unblock_event: None,
                    alpha: None,
                },
            }],
        }]
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
                    self.scene.render(&mut current_presentation.image)?;
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
