// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;
use fuchsia_audio::Format;
use std::time::Duration;
use {fidl_fuchsia_audio_controller as fac, fidl_fuchsia_media as fmedia};

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "record",
    description = "Records audio data from audio_core's AudioCapturer API and outputs a WAV file \
    to stdout.",
    example = "$ ffx audio record --duration 1s --format 48000,uint8,1ch \
    --usage SYSTEM-AGENT > ~/recording.wav"
)]
pub struct RecordCommand {
    #[argh(
        option,
        description = "duration of output signal. Examples: 5ms or 3s. \
        If not specified, press ENTER to stop recording.",
        from_str_fn(parse_duration)
    )]
    pub duration: Option<Duration>,

    #[argh(option, description = "output format (see 'ffx audio help' for more information).")]
    pub format: Format,

    #[argh(
        option,
        description = "purpose of the stream being recorded. \
        Accepted values: BACKGROUND, FOREGROUND, SYSTEM-AGENT, COMMUNICATION, ULTRASOUND, or \
        LOOPBACK. \
        Default: COMMUNICATION.",
        from_str_fn(str_to_usage),
        default = "AudioCaptureUsageExtended::Communication\
        (fmedia::AudioCaptureUsage2::Communication)"
    )]
    pub usage: AudioCaptureUsageExtended,

    #[argh(
        option,
        description = "buffer size (bytes) to allocate on device VMO. \
        Used to retrieve audio data from AudioCapturer. \
        Defaults to size that holds 1 second of audio data."
    )]
    pub buffer_size: Option<u64>,

    #[argh(
        option,
        description = "explicitly set the capturer's reference clock. \
        By default, SetReferenceClock is not called, which leads to a flexible clock. \
        Options include: 'flexible', 'monotonic', and 'custom,<rate adjustment>,<offset>' where \
        rate adjustment and offset are integers. \
        To set offset without rate adjustment, pass 0 in place of rate adjustment.",
        from_str_fn(str_to_clock),
        default = "fac::ClockType::Flexible(fac::Flexible)"
    )]
    pub clock: fac::ClockType,

    #[argh(
        option,
        description = "gain (decibels) for the capturer. Default: 0 dB",
        default = "0.0f32"
    )]
    pub gain: f32,

    #[argh(option, description = "mute the capturer. Default: false", default = "false")]
    pub mute: bool,
}

#[derive(Debug, PartialEq)]
pub enum AudioCaptureUsageExtended {
    Background(fmedia::AudioCaptureUsage2),
    Foreground(fmedia::AudioCaptureUsage2),
    SystemAgent(fmedia::AudioCaptureUsage2),
    Communication(fmedia::AudioCaptureUsage2),
    Ultrasound,
    Loopback,
}

fn str_to_usage(src: &str) -> Result<AudioCaptureUsageExtended, String> {
    match src.to_uppercase().as_str() {
        "BACKGROUND" => {
            Ok(AudioCaptureUsageExtended::Background(fmedia::AudioCaptureUsage2::Background))
        }
        "FOREGROUND" => {
            Ok(AudioCaptureUsageExtended::Foreground(fmedia::AudioCaptureUsage2::Foreground))
        }
        "SYSTEM-AGENT" => {
            Ok(AudioCaptureUsageExtended::SystemAgent(fmedia::AudioCaptureUsage2::SystemAgent))
        }
        "COMMUNICATION" => {
            Ok(AudioCaptureUsageExtended::Communication(fmedia::AudioCaptureUsage2::Communication))
        }
        "ULTRASOUND" => Ok(AudioCaptureUsageExtended::Ultrasound),
        "LOOPBACK" => Ok(AudioCaptureUsageExtended::Loopback),
        _ => Err(String::from(
            "Couldn't parse usage. \
Expected one of: BACKGROUND, FOREGROUND, SYSTEM-AGENT, COMMUNICATION, ULTRASOUND, or LOOPBACK",
        )),
    }
}

fn parse_duration(value: &str) -> Result<Duration, String> {
    fuchsia_audio::parse_duration(value)
}

fn str_to_clock(value: &str) -> Result<fac::ClockType, String> {
    fuchsia_audio::str_to_clock(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_str_to_usage() {
        assert_eq!(
            str_to_usage("BACKGROUND").unwrap(),
            AudioCaptureUsageExtended::Background(fmedia::AudioCaptureUsage2::Background)
        );
        assert_eq!(
            str_to_usage("foreground").unwrap(),
            AudioCaptureUsageExtended::Foreground(fmedia::AudioCaptureUsage2::Foreground)
        );
        assert_eq!(
            str_to_usage("SYSTEM-AGENT").unwrap(),
            AudioCaptureUsageExtended::SystemAgent(fmedia::AudioCaptureUsage2::SystemAgent)
        );
        assert_eq!(
            str_to_usage("communication").unwrap(),
            AudioCaptureUsageExtended::Communication(fmedia::AudioCaptureUsage2::Communication)
        );
        assert_eq!(str_to_usage("ULTRASOUND").unwrap(), AudioCaptureUsageExtended::Ultrasound);
        assert_eq!(str_to_usage("loopback").unwrap(), AudioCaptureUsageExtended::Loopback);
        assert!(str_to_usage("invalid").is_err());
    }
}
