// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;
use fuchsia_audio::Format;
use std::time::Duration;

/// TODO(https://fxbug.dev/42061172) - Add support for writing infinite files.
#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "gen",
    description = "Generate an audio signal. Outputs a WAV file written to stdout.",
    example = "ffx audio gen sine --duration 5ms --frequency 440 --amplitude 0.5 --format 48000,int16,2ch"
)]
pub struct GenCommand {
    #[argh(subcommand)]
    pub subcommand: SubCommand,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand)]
pub enum SubCommand {
    Sine(SineCommand),
    Square(SquareCommand),
    Sawtooth(SawtoothCommand),
    Triangle(TriangleCommand),
    PinkNoise(PinkNoiseCommand),
    WhiteNoise(WhiteNoiseCommand),
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "sine", description = "Generate a sine wave signal.")]
pub struct SineCommand {
    #[argh(
        option,
        description = "duration of output signal. Examples: 5ms or 3s.",
        from_str_fn(parse_duration)
    )]
    pub duration: Duration,

    #[argh(option, description = "frequency of output wave in Hz.")]
    pub frequency: u64,

    #[argh(option, description = "signal amplitude in range [0, 1.0]. Default: 1.0.")]
    pub amplitude: Option<f64>,

    #[argh(option, description = "output format (see 'ffx audio help' for more information).")]
    pub format: Format,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "square", description = "Generate a square wave signal.")]
pub struct SquareCommand {
    #[argh(
        option,
        description = "duration of output signal. Examples: 5ms or 3s.",
        from_str_fn(parse_duration)
    )]
    pub duration: Duration,

    #[argh(option, description = "frequency of output wave in Hz.")]
    pub frequency: u64,

    #[argh(option, description = "signal amplitude in range [0, 1.0]. Default: 1.0.")]
    pub amplitude: Option<f64>,

    #[argh(option, default = "0.5", description = "duty cycle in range [0, 1.0]. Default: 0.5.")]
    pub duty_cycle: f64,

    #[argh(option, description = "output format (see 'ffx audio help' for more information).")]
    pub format: Format,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "sawtooth", description = "Generate a sawtooth wave signal.")]
pub struct SawtoothCommand {
    #[argh(
        option,
        description = "duration of output signal. Examples: 5ms or 3s.",
        from_str_fn(parse_duration)
    )]
    pub duration: Duration,

    #[argh(option, description = "frequency of output wave in Hz.")]
    pub frequency: u64,

    #[argh(option, description = "signal amplitude in range [0, 1.0]. Default: 1.0.")]
    pub amplitude: Option<f64>,

    #[argh(option, description = "output format (see 'ffx audio help' for more information).")]
    pub format: Format,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "triangle", description = "Generate a triangle wave signal.")]
pub struct TriangleCommand {
    #[argh(
        option,
        description = "duration of output signal. Examples: 5ms or 3s.",
        from_str_fn(parse_duration)
    )]
    pub duration: Duration,

    #[argh(option, description = "frequency of output wave in Hz.")]
    pub frequency: u64,

    #[argh(option, description = "signal amplitude in range [0, 1.0]. Default: 1.0.")]
    pub amplitude: Option<f64>,

    #[argh(option, description = "output format (see 'ffx audio help' for more information).")]
    pub format: Format,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "white-noise", description = "Generate white noise.")]
pub struct WhiteNoiseCommand {
    #[argh(
        option,
        description = "duration of output signal. Examples: 5ms or 3s.",
        from_str_fn(parse_duration)
    )]
    pub duration: Duration,

    #[argh(option, description = "signal amplitude in range [0, 1.0]. Default: 1.0.")]
    pub amplitude: Option<f64>,

    #[argh(option, description = "output format (see 'ffx audio help' for more information).")]
    pub format: Format,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "pink-noise", description = "Generate pink noise.")]
pub struct PinkNoiseCommand {
    #[argh(
        option,
        description = "duration of output signal. Examples: 5ms or 3s.",
        from_str_fn(parse_duration)
    )]
    pub duration: Duration,

    #[argh(option, description = "signal amplitude in range [0, 1.0]. Default: 1.0.")]
    pub amplitude: Option<f64>,

    #[argh(option, description = "output format (see 'ffx audio help' for more information).")]
    pub format: Format,
}

/// Parses a Duration from string.
fn parse_duration(value: &str) -> Result<Duration, String> {
    fuchsia_audio::parse_duration(value)
}
