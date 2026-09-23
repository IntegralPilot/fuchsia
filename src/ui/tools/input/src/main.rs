// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use clap::{Parser, Subcommand};
use std::env;
use std::time::Duration;

#[derive(Debug, Parser)]
#[command(name = "input")]
/// simple tool to inject input events into Scenic
struct Opt {
    #[command(subcommand)]
    command: Command,
}

const KEYEVENT_HELP: &str = r#"Injects a single key event.

This command simulates a single key down + up sequence. The argument is a decimal HID usage
`code`, prior to any remapping the IME may do.

Common usage codes:

key       | code
----------|-----
enter     | 40
escape    | 41
backspace | 42
tab       | 43

The time between the down and up report is `--duration`."#;

#[derive(Debug, Subcommand)]
enum Command {
    #[command(name = "text")]
    /// Injects text using QWERTY keystrokes
    ///
    /// Text is injected by translating a string into keystrokes using a QWERTY keymap. This
    /// facility is intended for end-to-end and input testing purposes only.
    ///
    /// Only printable ASCII characters are mapped. Tab, newline, and other control characters are
    /// not supported, and `keyevent` should be used instead.
    ///
    /// The events simulated consist of a series of keyboard reports, ending in a report with no
    /// keys. The number of reports is near the lower bound of reports needed to produce the
    /// requested string, minimizing pauses and shift-state transitions.
    ///
    /// The `--key_event_duration` is the time between each report. Care should be taken not to
    /// provide so long a duration that key repeat kicks in.
    ///
    /// Note: when using through `fx shell` with quotes, you may need to surround the invocation in
    /// strong quotes, e.g.:
    ///
    /// fx shell 'input text "Hello, world!"'
    Text {
        #[arg(long = "key_event_duration", default_value = "0")]
        /// Duration of the each event in milliseconds
        key_event_duration_msec: u32,
        /// Input text to inject
        input: String,
    },
    #[command(name = "keyevent", about = KEYEVENT_HELP)]
    KeyboardEvent {
        #[arg(short = 'd', long = "duration", default_value = "0")]
        /// Duration of the event(s) in milliseconds
        duration_msec: u32,
        /// HID usage code
        usage: u32,
    },
    #[command(name = "tap")]
    /// Injects a single tap event
    ///
    /// This command simulates a single touch down + up sequence. By default, the `x` and `y`
    /// coordinates are in the range 0 to 1000 and will be proportionally transformed to the
    /// current display, but you can specify a virtual range for the input with the `--width` and
    /// `--height` options.
    ///
    /// The time between the down and up report is `--duration`.
    Tap {
        #[arg(short = 'w', long = "width", default_value = "1000")]
        /// Width of the display
        width: u32,
        #[arg(short = 'H', long = "height", default_value = "1000")]
        /// Height of the display
        height: u32,
        #[arg(short = 'c', long = "tap_event_count", default_value = "1")]
        /// Number of tap events to send (`--duration` is divided over the tap events)
        tap_event_count: usize,
        #[arg(short = 'd', long = "duration", default_value = "0")]
        /// Duration of the event(s) in milliseconds
        duration_msec: u32,
        /// X axis coordinate
        x: u32,
        /// Y axis coordinate
        y: u32,
    },
    #[command(name = "swipe")]
    /// Injects a swipe event
    ///
    /// This command simulates a touch down, a series of touch move, followed up a touch up event.
    /// By default, the `x` and `y` coordinates are in the range 0 to 1000 and will be
    /// proportionally transformed to the current display, but you can specify a virtual range for
    /// the input with the `--width` and `--height` options.
    ///
    /// The time between the down and up report is `--duration`.
    Swipe {
        #[arg(short = 'w', long = "width", default_value = "1000")]
        /// Width of the display
        width: u32,
        #[arg(short = 'H', long = "height", default_value = "1000")]
        /// Height of the display
        height: u32,
        #[arg(alias = "mc", long = "move_event_count", default_value = "100")]
        /// Number of move events to send in between the down and up events of the swipe
        move_event_count: usize,
        #[arg(short = 'd', long = "duration", default_value = "0")]
        /// Duration of the event(s) in milliseconds
        duration_msec: u32,
        /// X axis start coordinate
        x0: u32,
        /// Y axis start coordinate
        y0: u32,
        /// X axis end coordinate
        x1: u32,
        /// Y axis end coordinate
        y1: u32,
    },
    #[command(name = "media_button")]
    /// Injects a MediaButton event
    MediaButton {
        #[arg(value_parser = parse_bool)]
        /// 0 or 1
        mic_mute: bool,
        #[arg(value_parser = parse_bool)]
        /// 0 or 1
        volume_up: bool,
        #[arg(value_parser = parse_bool)]
        /// 0 or 1
        volume_down: bool,
        #[arg(value_parser = parse_bool)]
        /// 0 or 1
        reset: bool,
        #[arg(value_parser = parse_bool)]
        /// 0 or 1
        pause: bool,
        #[arg(value_parser = parse_bool)]
        /// 0 or 1
        camera_disable: bool,
    },
}

fn parse_bool(src: &str) -> Result<bool, String> {
    match src {
        "0" | "false" => Ok(false),
        "1" | "true" => Ok(true),
        _ => Err(format!("cannot parse {:?} to bool", src)),
    }
}

#[fuchsia::main]
async fn main() {
    // TODO: Remove this workaround once topaz tests have been updated.
    let (mut args, options): (Vec<_>, Vec<_>) =
        env::args_os().partition(|s: &std::ffi::OsString| match s.to_str() {
            Some(s) => s.get(0..2) != Some("--"),
            _ => false,
        });
    args.extend(options);

    let opt = Opt::parse_from(args);
    match opt.command {
        Command::Text { input, key_event_duration_msec } => {
            input_synthesis::text_command(
                input,
                Duration::from_millis(key_event_duration_msec as u64),
            )
            .await
        }
        Command::KeyboardEvent { usage, duration_msec } => {
            input_synthesis::keyboard_event_command(
                usage,
                Duration::from_millis(duration_msec as u64),
            )
            .await
        }
        Command::Tap { width, height, tap_event_count, duration_msec, x, y } => {
            input_synthesis::tap_event_command(
                x,
                y,
                width,
                height,
                tap_event_count,
                Duration::from_millis(duration_msec as u64),
            )
            .await
        }
        Command::Swipe { width, height, move_event_count, duration_msec, x0, y0, x1, y1 } => {
            input_synthesis::swipe_command(
                x0,
                y0,
                x1,
                y1,
                width,
                height,
                move_event_count,
                Duration::from_millis(duration_msec as u64),
            )
            .await
        }
        Command::MediaButton { mic_mute, volume_up, volume_down, reset, pause, camera_disable } => {
            input_synthesis::media_button_event_command(
                mic_mute,
                volume_up,
                volume_down,
                reset,
                pause,
                camera_disable,
            )
            .await
        }
    }
    .expect("failed to run command");
}
