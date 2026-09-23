// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

///! This function demonstrates setting a single config with a static full-screen fill color and
///! sampling vsync events.
use {
    anyhow::{Result, format_err},
    display_utils::{
        Color, Coordinator, DisplayConfig, DisplayInfo, Layer, LayerConfig, PixelFormat, VsyncEvent,
    },
    fidl_fuchsia_math,
    futures::StreamExt,
    std::io::Write,
};

use crate::fps::Counter;
use crate::rgb::Rgb888;

const CLEAR: &str = "\x1B[2K\r";

pub struct Args<'a> {
    pub display: &'a DisplayInfo,
    pub color: Rgb888,
    pub pixel_format: PixelFormat,
}

pub fn get_bytes_for_rgb_color(rgb: Rgb888, pixel_format: PixelFormat) -> Result<[u8; 8]> {
    match pixel_format {
        PixelFormat::Bgra32 => {
            Ok([rgb.b, rgb.g, rgb.r, /*alpha=*/ 255, 0, 0, 0, 0])
        }
        PixelFormat::R8G8B8A8 => {
            Ok([rgb.r, rgb.g, rgb.b, /*alpha=*/ 255, 0, 0, 0, 0])
        }
        _ => Err(anyhow::format_err!("unsupported pixel format {}", pixel_format)),
    }
}

pub async fn run<'a>(coordinator: &Coordinator, args: Args<'a>) -> Result<()> {
    let Args { display, color, pixel_format } = args;
    let color_bytes = get_bytes_for_rgb_color(color, pixel_format)?;

    // Ensure that vsync events are enabled before we issue the first call to CommitConfig.
    let mut vsync = coordinator.add_vsync_listener(Some(display.id()))?;

    let layer = coordinator.create_layer().await?;
    let configs = vec![DisplayConfig {
        id: display.id(),
        layers: vec![Layer {
            id: layer,
            config: LayerConfig::Color {
                color: Color { format: pixel_format, bytes: color_bytes },
                display_destination: fidl_fuchsia_math::RectU {
                    x: 0,
                    y: 0,
                    width: display.0.modes[0].active_area.width,
                    height: display.0.modes[0].active_area.height,
                },
            },
        }],
    }];
    coordinator.commit_config(&configs).await?;
    let recent_committed_config_stamp = coordinator.get_recent_committed_config_stamp().await?;

    // The color layer should be displayed on the screen and Vsync events
    // should start.
    let mut counter = Counter::new();
    let mut config_applied = false;
    while let Some(VsyncEvent { id, timestamp, config }) = vsync.next().await {
        counter.add(timestamp);
        let stats = counter.stats();
        config_applied |= config.value == recent_committed_config_stamp;

        print!(
            "{}Display {} config {} applied, refresh rate {:.2} Hz ({:.5} ms)",
            CLEAR,
            id.0,
            if config_applied { "is" } else { "is not" },
            stats.sample_rate_hz,
            stats.sample_time_delta_ms
        );
        std::io::stdout().flush()?;
    }

    Err(format_err!("stopped receiving vsync events"))
}
