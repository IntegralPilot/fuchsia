// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

///! A helper utility used to check whether the display device skips any frame provided by the
///! display engine.
use {
    anyhow::{Context, Result},
    display_utils::{Coordinator, DisplayInfo, PixelFormat},
    euclid::{
        default::{Point2D, Rect, Size2D},
        point2, size2,
    },
};

use crate::draw::{Frame, MappedImage};
use crate::runner::{DoubleBufferedFenceLoop, Scene};

const WHITE: [u8; 4] = [255, 255, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];
const RED: [u8; 4] = [0, 0, 255, 255];
const BLUE: [u8; 4] = [255, 0, 0, 255];

// Draws a `rows` * `cols` rectangular grid on `image`. Cell borders are colored
// using `color`.
//
// Callers must call `image.cache_clean()` to flush the cache after finishing
// drawing everything.
fn draw_grid(
    grid: Rect<u32>,
    rows: u32,
    cols: u32,
    color: &[u8; 4],
    image: &mut MappedImage,
) -> Result<()> {
    let cell_size: Size2D<u32> = size2(grid.width() / cols, grid.height() / rows);
    // Draw horizontal lines
    for row in 0..rows + 1 {
        let frame = Frame {
            pos_x: grid.min_x(),
            pos_y: grid.min_y() + cell_size.height * row - 1,
            width: cell_size.width * cols,
            height: 2,
        };
        image.fill_region(color, &frame)?;
    }

    // Draw vertical lines
    for col in 0..cols + 1 {
        let frame = Frame {
            pos_x: grid.min_x() + cell_size.width * col - 1,
            pos_y: grid.min_y(),
            width: 2,
            height: cell_size.height * rows,
        };
        image.fill_region(color, &frame)?;
    }

    Ok(())
}

fn get_cell_color_for_frame(frame_num: u64, rows: u32, cols: u32) -> &'static [u8; 4] {
    let num_cells = rows * cols;
    let sequence_num: u32 = (frame_num % (num_cells as u64)) as u32;
    if sequence_num == 0 {
        // First frame of the animation sequence.
        &BLUE
    } else if sequence_num == num_cells - 1 {
        // Last frame of the animation sequence.
        &RED
    } else {
        // Rest of the frames of the animation sequence.
        &WHITE
    }
}

// Maps the frame number to the position of the colored cell in the grid.
//
// `rows` must be even.
//
// TODO(https://fxbug.dev/42084641): The precondition can be relaxed to "either `rows` or
// `cols` must be even".
fn get_position_in_grid_for_frame(frame_num: u64, rows: u32, cols: u32) -> Point2D<u32> {
    // Assume the grid is of size 10 x 6, by connecting 00->01->...->58->59->00
    // in the grid below, we can get a Hamiltonian loop describing the movement
    // of the colored cell.
    //
    // 59 00 01 02 03 04
    // 58 09 08 07 06 05
    // 57 10 11 12 13 14
    // 56 19 18 17 16 15
    // 55 20 21 22 23 24
    // 54 29 28 27 26 25
    // 53 30 31 32 33 34
    // 52 39 38 37 36 35
    // 51 40 41 42 43 44
    // 50 49 48 47 46 45
    //
    // The right of the grid are five connected "コ" shapes each connecting 10
    // cells. The left of the grid is a straight line connecting cell 50 to 59.
    //
    // For each frame, the frame number is first mapped to the animated
    // sequence, and then mapped to the cell position in the grid.
    //
    // This construction can be generalized to any grid as long as the number
    // of rows is even.
    //
    // TODO(https://fxbug.dev/42084641): Actually it works as long as either `rows` or
    // `cols` is even. We can relax this precondition and rotate the pattern if
    // `rows` is odd and `cols` is even.
    assert!(rows % 2 == 0);

    let num_cells = rows * cols;
    let sequence_num: u32 = (frame_num % (num_cells as u64)) as u32;
    if sequence_num < num_cells - rows {
        // The "コ" shapes on the right. Each "コ" shape has a width of
        // `2 * (cols - 1)` and a height of 2.
        let x = if sequence_num % (2 * (cols - 1)) < cols - 1 {
            1 + sequence_num % (2 * (cols - 1))
        } else {
            2 * (cols - 1) - sequence_num % (2 * (cols - 1))
        };
        let y = sequence_num / (cols - 1);
        point2(x, y)
    } else {
        // The straight line on the left connecting cells
        // `num_cells - rows`..=`num_cells - 1`.
        point2(0, num_cells - sequence_num - 1)
    }
}

struct FrameRateTestScene {
    frame_num: u64,
    grid: Rect<u32>,
}

impl FrameRateTestScene {
    pub fn new(
        screen_size: Size2D<u32>,
        grid_width: Option<u32>,
        grid_height: Option<u32>,
    ) -> Self {
        let center = point2(0, 0) + screen_size / 2;

        let min_length = u32::min(screen_size.width, screen_size.height);
        let grid_default_length = (min_length as f32 * std::f32::consts::FRAC_1_SQRT_2) as u32;
        let grid_size = size2(
            grid_width.unwrap_or(grid_default_length),
            grid_height.unwrap_or(grid_default_length),
        );
        let grid_origin = center - grid_size / 2;

        FrameRateTestScene { frame_num: 0, grid: Rect::new(grid_origin, grid_size) }
    }
}

impl Scene for FrameRateTestScene {
    fn update(&mut self) -> Result<()> {
        self.frame_num += 1;
        Ok(())
    }

    fn init_image(&self, image: &mut MappedImage) -> Result<()> {
        image.zero().context("failed to clear background")?;
        Ok(())
    }

    fn render(&mut self, image: &mut MappedImage) -> Result<()> {
        // We only clear the grid region; the rest of the image is never
        // modified.
        image
            .fill_region(
                &BLACK,
                &Frame {
                    pos_x: self.grid.min_x(),
                    pos_y: self.grid.min_y(),
                    width: self.grid.width(),
                    height: self.grid.height(),
                },
            )
            .context("failed to clear the grid pixels")?;

        const NUM_GRID_ROWS: u32 = 10;
        const NUM_GRID_COLS: u32 = 6;
        draw_grid(self.grid, NUM_GRID_ROWS, NUM_GRID_COLS, &WHITE, image)
            .context("failed to draw grid")?;

        // The test scene is a loop animation where the position and the color
        // of the colored cell in the grid is determined by the frame number.
        let position_in_grid =
            get_position_in_grid_for_frame(self.frame_num, NUM_GRID_ROWS, NUM_GRID_COLS);
        let color = get_cell_color_for_frame(self.frame_num, NUM_GRID_ROWS, NUM_GRID_COLS);

        let cell_size: Size2D<u32> =
            size2(self.grid.width() / NUM_GRID_COLS, self.grid.height() / NUM_GRID_ROWS);
        image
            .fill_region(
                color,
                &Frame {
                    pos_x: self.grid.min_x() + position_in_grid.x * cell_size.width,
                    pos_y: self.grid.min_y() + position_in_grid.y * cell_size.height,
                    width: cell_size.width,
                    height: cell_size.height,
                },
            )
            .context("failed to draw the colored cell")?;

        image.cache_clean()?;
        Ok(())
    }
}

pub async fn run(
    coordinator: &Coordinator,
    display: &DisplayInfo,
    grid_width: Option<u32>,
    grid_height: Option<u32>,
) -> Result<()> {
    // TODO(https://fxbug.dev/42084882): Rather than manually checking the display engine
    // output performance, we should add an automatic check to print error /
    // terminate the test when the frame rate drops.
    #[cfg(debug_assertions)]
    {
        eprintln!(
            "[WARNING] Running a debug build. Rendering and display performance may be impacted."
        );
    }

    // Obtain the display resolution based on the display's preferred mode.
    let (width, height, expected_frame_rate) = {
        let mode = &display.0.modes[0];
        (
            mode.active_area.width,
            mode.active_area.height,
            mode.refresh_rate_millihertz as f32 / 1000.0,
        )
    };
    println!("Expected frame rate: {:.3} fps", expected_frame_rate);

    let scene = FrameRateTestScene::new(size2(width, height), grid_width, grid_height);
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
