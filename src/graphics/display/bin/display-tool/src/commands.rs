// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, format_err};
use display_utils::{Coordinator, DisplayId, PixelFormat};

mod bouncing_squares;
mod display_color_layer;
mod frame_rate_test;
mod multilayer_squares;
mod static_config_vsync_loop;

use crate::rgb::Rgb888;

pub fn show_display_info(
    coordinator: &Coordinator,
    id: Option<DisplayId>,
    fidl: bool,
) -> Result<()> {
    let displays = coordinator.displays();
    println!("{} display(s) available", displays.len());
    for display in displays.iter().filter(|&info| id.map_or(true, |id| id == info.0.id.into())) {
        if fidl {
            println!("{:#?}", display.0);
        } else {
            println!("{}", display);
        }
    }
    Ok(())
}

pub async fn vsync(
    coordinator: &Coordinator,
    id: Option<DisplayId>,
    color: Rgb888,
    pixel_format: PixelFormat,
) -> Result<()> {
    let displays = coordinator.displays();
    if displays.is_empty() {
        return Err(format_err!("no displays found"));
    }

    let display = match id {
        // Pick the first available display if no ID was specified.
        None => &displays[0],
        Some(id) => displays
            .iter()
            .find(|d| d.id() == id)
            .ok_or_else(|| format_err!("display with id '{:?}' not found", id))?,
    };

    static_config_vsync_loop::run(
        coordinator,
        static_config_vsync_loop::Args { display, color, pixel_format },
    )
    .await
}

pub async fn color(
    coordinator: &Coordinator,
    id: Option<DisplayId>,
    color: Rgb888,
    pixel_format: PixelFormat,
) -> Result<()> {
    let displays = coordinator.displays();
    if displays.is_empty() {
        return Err(format_err!("no displays found"));
    }

    let display = match id {
        // Pick the first available display if no ID was specified.
        None => &displays[0],
        Some(id) => displays
            .iter()
            .find(|d| d.id() == id)
            .ok_or_else(|| format_err!("display with id '{:?}' not found", id))?,
    };

    display_color_layer::run(
        coordinator,
        display_color_layer::Args { display, color, pixel_format },
    )
    .await
}

pub async fn squares(coordinator: &Coordinator, id: Option<DisplayId>) -> Result<()> {
    let displays = coordinator.displays();
    if displays.is_empty() {
        return Err(format_err!("no displays found"));
    }

    let display = match id {
        // Pick the first available display if no ID was specified.
        None => &displays[0],
        Some(id) => displays
            .iter()
            .find(|d| d.id() == id)
            .ok_or_else(|| format_err!("display with id '{:?}' not found", id))?,
    };

    bouncing_squares::run(coordinator, display).await
}

pub async fn frame_rate_test(
    coordinator: &Coordinator,
    id: Option<DisplayId>,
    grid_width: Option<u32>,
    grid_height: Option<u32>,
) -> Result<()> {
    let displays = coordinator.displays();
    if displays.is_empty() {
        return Err(format_err!("no displays found"));
    }

    let display = match id {
        // Pick the first available display if no ID was specified.
        None => &displays[0],
        Some(id) => displays
            .iter()
            .find(|d| d.id() == id)
            .ok_or_else(|| format_err!("display with id '{:?}' not found", id))?,
    };

    frame_rate_test::run(coordinator, display, grid_width, grid_height).await
}

pub async fn multilayer_squares(coordinator: &Coordinator, id: Option<DisplayId>) -> Result<()> {
    let displays = coordinator.displays();
    if displays.is_empty() {
        return Err(format_err!("no displays found"));
    }

    let display = match id {
        // Pick the first available display if no ID was specified.
        None => &displays[0],
        Some(id) => displays
            .iter()
            .find(|d| d.id() == id)
            .ok_or_else(|| format_err!("display with id '{:?}' not found", id))?,
    };

    multilayer_squares::run(coordinator, display).await
}
