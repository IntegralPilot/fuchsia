// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

///! Demonstrates building an animation using a double-buffer swapchain with multiple layers.
use {
    anyhow::{Context, Result},
    display_utils::{Alpha, Coordinator, DisplayInfo, PixelFormat},
    fidl_fuchsia_hardware_display_types as fdisplay_types,
    std::cmp::min,
};

use crate::draw::{Frame, MappedImage};
use crate::runner_multilayer::{MultiLayerFenceLoop, MultiLayerScene};

const LAYER_COUNT: usize = 2;
struct BouncingSquare {
    color: [u8; 4],
    frame: Frame,
    velocity: (i64, i64),
}

impl BouncingSquare {
    fn update(&mut self, screen_width: u32, screen_height: u32) {
        let x = self.frame.pos_x as i64 + self.velocity.0;
        let y = self.frame.pos_y as i64 + self.velocity.1;
        if x < 0 || x as u32 + self.frame.width > screen_width {
            self.velocity.0 *= -1;
        }
        if y < 0 || y as u32 + self.frame.height > screen_height {
            self.velocity.1 *= -1;
        }
        self.frame.pos_x = min(x.abs() as u32, screen_width - self.frame.width - 1);
        self.frame.pos_y = min(y.abs() as u32, screen_height - self.frame.height - 1);
    }
}

struct MultiLayerSquaresScene {
    width: u32,
    height: u32,
    squares: Vec<BouncingSquare>,
}

impl MultiLayerSquaresScene {
    pub fn new(width: u32, height: u32) -> Self {
        let dim = min(width, height) / 8;
        let squares = vec![
            // Pink square for layer 1
            BouncingSquare {
                color: [255, 0, 255, 255],
                frame: Frame { pos_x: width - dim - 1, pos_y: 0, width: dim * 4, height: dim * 4 },
                velocity: (-8, 8),
            },
            // Other squares for layer 2
            BouncingSquare {
                color: [255, 100, 0, 255],
                frame: Frame { pos_x: 0, pos_y: 0, width: dim, height: dim },
                velocity: (16, 16),
            },
            BouncingSquare {
                color: [100, 255, 0, 150],
                frame: Frame { pos_x: 0, pos_y: height - dim - 1, width: dim, height: dim },
                velocity: (4, -8),
            },
            BouncingSquare {
                color: [0, 100, 255, 255],
                frame: Frame {
                    pos_x: width - dim - 1,
                    pos_y: height - dim - 1,
                    width: dim,
                    height: dim,
                },
                velocity: (-16, -8),
            },
        ];
        MultiLayerSquaresScene { width, height, squares }
    }

    fn render_layer_1(&self, image: &mut MappedImage) -> Result<()> {
        // Transparent background
        image.fill_region(
            &[0, 0, 0, 0],
            &Frame { pos_x: 0, pos_y: 0, width: self.width, height: self.height },
        )?;
        // Draw the pink square
        image.fill_region(&self.squares[0].color, &self.squares[0].frame)?;
        image.cache_clean()?;
        Ok(())
    }

    fn render_layer_2(&self, image: &mut MappedImage) -> Result<()> {
        // Transparent background
        image.fill_region(
            &[0, 0, 0, 0],
            &Frame { pos_x: 0, pos_y: 0, width: self.width, height: self.height },
        )?;
        // Draw the other three squares
        for s in &self.squares[1..] {
            image.fill_region(&s.color, &s.frame)?;
        }
        image.cache_clean()?;
        Ok(())
    }
}

impl MultiLayerScene for MultiLayerSquaresScene {
    fn update(&mut self) -> Result<()> {
        for s in &mut self.squares {
            s.update(self.width, self.height);
        }
        Ok(())
    }

    fn init_images(&self, images: &mut [MappedImage]) -> Result<()> {
        for image in images {
            image.zero().context("failed to zero image")?;
        }
        Ok(())
    }

    fn render(&mut self, images: &mut [MappedImage]) -> Result<()> {
        if images.len() < LAYER_COUNT {
            anyhow::bail!("Expected at least {} images for rendering", LAYER_COUNT);
        }
        self.render_layer_1(&mut images[0])?;
        self.render_layer_2(&mut images[1])?;
        Ok(())
    }

    fn get_alpha_for_layer(&self, layer_index: usize) -> Option<Alpha> {
        match layer_index {
            // For the first layer (index 0), make it fully opaque.
            0 => Some(Alpha {
                mode: fdisplay_types::AlphaMode::Disable,
                val: 0.0, // currently ignored
            }),
            // For the rest of the layers, use an alpha mode.
            _ => Some(Alpha {
                mode: fdisplay_types::AlphaMode::Premultiplied,
                val: 0.0, // currently ignored
            }),
        }
    }
}

pub async fn run(coordinator: &Coordinator, display: &DisplayInfo) -> Result<()> {
    let (width, height) = {
        let mode = &display.0.modes[0];
        (mode.active_area.width, mode.active_area.height)
    };

    let scene = MultiLayerSquaresScene::new(width, height);
    let mut fence_loop = MultiLayerFenceLoop::new(
        coordinator,
        display.id(),
        width,
        height,
        PixelFormat::Bgra32,
        LAYER_COUNT,
        scene,
    )
    .await?;

    fence_loop.run().await?;
    Ok(())
}
