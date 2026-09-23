// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use argh::FromArgs;
use display_utils::{Coordinator, DisplayId, PixelFormat, TEST_UTILITY_CLIENT_PRIORITY};
use futures::future::{FutureExt, TryFutureExt};
use futures::select;
use rgb::Rgb888;

mod commands;
mod draw;
mod fps;
mod rgb;
mod runner;
mod runner_multilayer;

/// Top-level list of this tool's command-line arguments
#[derive(FromArgs)]
struct Args {
    #[argh(subcommand)]
    cmd: SubCommands,
}

/// Show information about all currently attached displays
#[derive(FromArgs)]
#[argh(subcommand, name = "info")]
struct InfoArgs {
    /// ID of the display to show
    #[argh(positional)]
    id: Option<u64>,

    /// show the raw FIDL structure contents
    #[argh(switch)]
    fidl: bool,
}

/// Show the active refresh rate for one or more displays
#[derive(FromArgs)]
#[argh(subcommand, name = "vsync")]
struct VsyncArgs {
    /// ID of the display to show
    #[argh(positional)]
    id: Option<u64>,

    /// screen fill color, using CSS hex syntax (rrggbb) without a leading #.
    /// Default to 0000ff.
    #[argh(option, default = "Rgb888{r: 0x00, g: 0x00, b: 0xff}")]
    color: Rgb888,

    /// pixel format. Default to BGRA8888.
    #[argh(option, default = "PixelFormat::Bgra32")]
    pixel_format: PixelFormat,
}

/// Display a color layer on one display
#[derive(FromArgs)]
#[argh(subcommand, name = "color")]
struct ColorArgs {
    /// ID of the display to show
    #[argh(positional)]
    id: Option<u64>,

    /// screen fill color, using CSS hex syntax (rrggbb) without a leading #.
    /// Default to 0000ff.
    #[argh(option, default = "Rgb888{r: 0x00, g: 0x00, b: 0xff}")]
    color: Rgb888,

    /// pixel format. Default to BGRA8888.
    #[argh(option, default = "PixelFormat::Bgra32")]
    pixel_format: PixelFormat,
}

/// Play a double buffered animation using fence synchronization.
#[derive(FromArgs)]
#[argh(subcommand, name = "squares")]
struct SquaresArgs {
    /// ID of the display to play the animation on
    #[argh(positional)]
    id: Option<u64>,
}

/// Play a multilayer double buffered animation using fence synchronization.
#[derive(FromArgs)]
#[argh(subcommand, name = "multilayer-squares")]
struct MultiLayerSquaresArgs {
    /// ID of the display to play the animation on
    #[argh(positional)]
    id: Option<u64>,
}

/// Test the display's actual frame rate.
///
/// Before checking the contents shown on the display device, users must make sure that the
/// frame rate displayed on the console (rate of frames provided by the display engine) matches
/// the expected frame rate in the display mode specified in EDID or panel configurations.
///
/// This utility should be built and run in release mode for best performance.
#[derive(FromArgs)]
#[argh(subcommand, name = "frame-rate-test")]
struct FrameRateTestArgs {
    /// ID of the display to play the animation on
    #[argh(positional)]
    id: Option<u64>,

    /// width of the rectangular grid of the test pattern, in pixels.
    /// The default value is "min(display width, display height) / sqrt(2)".
    #[argh(option)]
    grid_width: Option<u32>,

    /// height of the rectangular grid of the test pattern, in pixels.
    /// The default value is "min(display width, display height) / sqrt(2)".
    #[argh(option)]
    grid_height: Option<u32>,
}

#[derive(FromArgs)]
#[argh(subcommand)]
enum SubCommands {
    Info(InfoArgs),
    Vsync(VsyncArgs),
    Color(ColorArgs),
    Squares(SquaresArgs),
    MultiLayerSquares(MultiLayerSquaresArgs),
    FrameRateTest(FrameRateTestArgs),
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    fuchsia_trace_provider::trace_provider_create_with_fdio();

    let args: Args = argh::from_env();
    let coordinator = Coordinator::init(TEST_UTILITY_CLIENT_PRIORITY).await?;

    let fidl_events_future = coordinator.handle_events().err_into();
    let cmd_future = async {
        match args.cmd {
            SubCommands::Info(args) => {
                commands::show_display_info(&coordinator, args.id.map(DisplayId), args.fidl)
            }
            SubCommands::Vsync(args) => {
                commands::vsync(&coordinator, args.id.map(DisplayId), args.color, args.pixel_format)
                    .await
            }
            SubCommands::Color(args) => {
                commands::color(&coordinator, args.id.map(DisplayId), args.color, args.pixel_format)
                    .await
            }
            SubCommands::Squares(args) => {
                commands::squares(&coordinator, args.id.map(DisplayId)).await
            }
            SubCommands::MultiLayerSquares(args) => {
                commands::multilayer_squares(&coordinator, args.id.map(DisplayId)).await
            }
            SubCommands::FrameRateTest(args) => {
                commands::frame_rate_test(
                    &coordinator,
                    args.id.map(DisplayId),
                    args.grid_width,
                    args.grid_height,
                )
                .await
            }
        }
    };

    select! {
        result1 = fidl_events_future.fuse() => result1,
        result2 = cmd_future.fuse() => result2,
    }
}
