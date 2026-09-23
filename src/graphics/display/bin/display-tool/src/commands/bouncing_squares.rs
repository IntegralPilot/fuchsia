// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

///! Demonstrates building an animation using a double-buffer swapchain.
use {
    anyhow::{Context, Result},
    display_utils::{Coordinator, DisplayInfo, PixelFormat},
    std::cmp::min,
};

use crate::draw::{Frame, MappedImage};
use crate::runner::DoubleBufferedFenceLoop;

use crate::runner::Scene;

struct BouncingSquare {
    color: [u8; 4],
    frame: Frame,
    velocity: (i64, i64),
}

impl BouncingSquare {
    // Update the position along the velocity vector. The velocities are updated such that the
    // square bounces off the boundaries of an enclosing screen of the given dimensions.
    //
    // The velocity is treated as a fixed increment in pixels and the calculation intentionally
    // does not factor in elapsed time or interpolate between steps to make the speed of the boxes
    // vary with the framerate.
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

struct BouncingSquaresScene {
    width: u32,
    height: u32,

    squares: Vec<BouncingSquare>,
}

impl BouncingSquaresScene {
    pub fn new(width: u32, height: u32) -> Self {
        // Construct squares that start out at the 4 corners of the screen.
        let dim = height / 8;
        let squares = vec![
            BouncingSquare {
                color: [255, 100, 0, 255],
                frame: Frame { pos_x: 0, pos_y: 0, width: dim, height: dim },
                velocity: (16, 16),
            },
            BouncingSquare {
                color: [255, 0, 255, 255],
                frame: Frame { pos_x: width - dim - 1, pos_y: 0, width: dim, height: dim },
                velocity: (-8, 8),
            },
            BouncingSquare {
                color: [100, 255, 0, 255],
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
        BouncingSquaresScene { width, height, squares }
    }
}

impl Scene for BouncingSquaresScene {
    fn update(&mut self) -> Result<()> {
        for s in &mut self.squares {
            s.update(self.width, self.height);
        }
        Ok(())
    }

    fn init_image(&self, _image: &mut MappedImage) -> Result<()> {
        // No need to initialize the image since it's zeroed every frame.
        Ok(())
    }

    fn render(&mut self, image: &mut MappedImage) -> Result<()> {
        image.zero().context("failed to clear background")?;
        for s in &self.squares {
            image.fill_region(&s.color, &s.frame).context("failed to draw bouncing square")?;
        }
        image.cache_clean()?;
        Ok(())
    }
}

pub async fn run(coordinator: &Coordinator, display: &DisplayInfo) -> Result<()> {
    // Obtain the display resolution based on the display's preferred mode.
    let (width, height) = {
        let mode = &display.0.modes[0];
        (mode.active_area.width, mode.active_area.height)
    };

    let scene = BouncingSquaresScene::new(width, height);
    let mut double_buffered_fence_loop = DoubleBufferedFenceLoop::new(
        coordinator,
        display.id(),
        width,
        height,
        PixelFormat::Bgra32,
        scene,
    )
    .await?;

    double_buffered_fence_loop.run().await?;
    Ok(())
}
