// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, Result, anyhow};
use flex_fuchsia_audio as faudio;
use flex_fuchsia_audio_controller as fac;
use flex_fuchsia_hardware_audio as fhaudio;
use flex_fuchsia_media as fmedia;
use regex_lite::Regex;
use std::fmt::Display;
use std::io::{Cursor, Seek, SeekFrom, Write};
use std::num::NonZeroU32;
use std::str::FromStr;
use std::time::Duration;

pub const DURATION_REGEX: &str = r"^(\d+)(h|m|s|ms)$";

// Common sample sizes.
pub const BITS_8: NonZeroU32 = NonZeroU32::new(8).unwrap();
pub const BITS_16: NonZeroU32 = NonZeroU32::new(16).unwrap();
pub const BITS_24: NonZeroU32 = NonZeroU32::new(24).unwrap();
pub const BITS_32: NonZeroU32 = NonZeroU32::new(32).unwrap();

#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub struct Format {
    pub sample_type: SampleType,
    pub frames_per_second: u32,
    pub channels: u32,
}

impl Format {
    pub const fn bytes_per_frame(&self) -> u32 {
        self.sample_type.size().total_bytes().get() * self.channels
    }

    pub fn frames_in_duration(&self, duration: std::time::Duration) -> u64 {
        (self.frames_per_second as f64 * duration.as_secs_f64()).ceil() as u64
    }

    pub fn wav_header_for_duration(&self, duration: Duration) -> Result<Vec<u8>, Error> {
        // A valid Wav File Header must have the data format and data length fields.
        // We need all values corresponding to wav header fields set on the cursor_writer
        // before writing to stdout.
        let mut cursor_writer = Cursor::new(Vec::<u8>::new());

        {
            // Creation of WavWriter writes the Wav File Header to cursor_writer.
            // This written header has the file size field and data chunk size field both set
            // to 0, since the number of samples (and resulting file and chunk sizes) are
            // unknown to the WavWriter at this point.
            let _writer = hound::WavWriter::new(&mut cursor_writer, (*self).into())
                .map_err(|e| anyhow!("Failed to create WavWriter from spec: {}", e))?;
        }

        // The file and chunk size fields are set to 0 as placeholder values by the
        // construction of the WavWriter above. We can compute the actual values based on the
        // command arguments for format and duration, and set the file size and chunk size
        // fields to the computed values in the cursor_writer before writing to stdout.

        let bytes_to_capture: u32 =
            self.frames_in_duration(duration) as u32 * self.bytes_per_frame();
        let total_header_bytes = 44;
        // The File Size field of a WAV header. 32-bit int starting at position 4, represents
        // the size of the overall file minus 8 bytes (exclude RIFF description and
        // file size description)
        let file_size_bytes: u32 = bytes_to_capture + total_header_bytes - 8;

        cursor_writer.seek(SeekFrom::Start(4))?;
        cursor_writer.write_all(&file_size_bytes.to_le_bytes()[..])?;

        // Data size field of a WAV header. For PCM, this is a 32-bit int starting at
        // position 40 and represents the size of the data section.
        cursor_writer.seek(SeekFrom::Start(40))?;
        cursor_writer.write_all(&bytes_to_capture.to_le_bytes()[..])?;

        // Write the completed WAV header to stdout. We then write the raw sample
        // values from the packets received directly to stdout.
        Ok(cursor_writer.into_inner())
    }
}

impl From<hound::WavSpec> for Format {
    fn from(value: hound::WavSpec) -> Self {
        Format {
            sample_type: (value.sample_format, value.bits_per_sample).into(),
            frames_per_second: value.sample_rate,
            channels: value.channels as u32,
        }
    }
}

impl From<fmedia::AudioStreamType> for Format {
    fn from(value: fmedia::AudioStreamType) -> Self {
        Format {
            sample_type: value.sample_format.into(),
            frames_per_second: value.frames_per_second,
            channels: value.channels,
        }
    }
}

impl From<Format> for fhaudio::PcmFormat {
    fn from(value: Format) -> Self {
        Self {
            number_of_channels: value.channels as u8,
            sample_format: value.sample_type.into(),
            bytes_per_sample: value.sample_type.size().total_bytes().get() as u8,
            valid_bits_per_sample: value.sample_type.size().valid_bits().get() as u8,
            frame_rate: value.frames_per_second,
        }
    }
}

impl From<Format> for fhaudio::Format {
    fn from(value: Format) -> Self {
        fhaudio::Format { pcm_format: Some(value.into()), ..Default::default() }
    }
}

impl From<Format> for faudio::Format {
    fn from(value: Format) -> Self {
        Self {
            sample_type: Some(value.sample_type.into()),
            channel_count: Some(value.channels),
            frames_per_second: Some(value.frames_per_second),
            channel_layout: {
                let channel_config = match value.channels {
                    1 => faudio::ChannelConfig::Mono,
                    2 => faudio::ChannelConfig::Stereo,
                    // The following are just a guess.
                    3 => faudio::ChannelConfig::Surround3,
                    4 => faudio::ChannelConfig::Surround4,
                    6 => faudio::ChannelConfig::Surround51,
                    _ => panic!("channel count not representable as a ChannelConfig"),
                };
                Some(faudio::ChannelLayout::Config(channel_config))
            },
            ..Default::default()
        }
    }
}

impl TryFrom<faudio::Format> for Format {
    type Error = String;

    fn try_from(value: faudio::Format) -> Result<Self, Self::Error> {
        let sample_type = value.sample_type.ok_or_else(|| "missing sample_type".to_string())?;
        let channel_count =
            value.channel_count.ok_or_else(|| "missing channel_count".to_string())?;
        let frames_per_second =
            value.frames_per_second.ok_or_else(|| "missing frames_per_second".to_string())?;
        Ok(Self {
            sample_type: SampleType::try_from(sample_type)?,
            channels: channel_count,
            frames_per_second,
        })
    }
}

impl From<Format> for hound::WavSpec {
    fn from(value: Format) -> Self {
        Self {
            channels: value.channels as u16,
            sample_format: value.sample_type.into(),
            sample_rate: value.frames_per_second,
            bits_per_sample: value.sample_type.size().total_bits().get() as u16,
        }
    }
}

impl From<Format> for fmedia::AudioStreamType {
    fn from(value: Format) -> Self {
        Self {
            sample_format: value.sample_type.into(),
            channels: value.channels,
            frames_per_second: value.frames_per_second,
        }
    }
}

impl Display for Format {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{},{},{}ch", self.frames_per_second, self.sample_type, self.channels)
    }
}

impl FromStr for Format {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error> {
        if s.is_empty() {
            return Err(anyhow!("No format specified."));
        };

        let splits: Vec<&str> = s.split(",").collect();

        if splits.len() != 3 {
            return Err(anyhow!(
                "Expected 3 comma-separated values: <SampleRate>,<SampleType>,<Channels> but have {}.",
                splits.len()
            ));
        }

        let frame_rate = match splits[0].parse::<u32>() {
            Ok(sample_rate) => Ok(sample_rate),
            Err(_) => Err(anyhow!("First value (sample rate) should be an integer.")),
        }?;

        let sample_type = match SampleType::from_str(splits[1]) {
            Ok(sample_type) => Ok(sample_type),
            Err(_) => Err(anyhow!(
                "Second value (sample type) should be one of: uint8, int16, int32, float32."
            )),
        }?;

        let channels = match splits[2].strip_suffix("ch") {
            Some(channels) => match channels.parse::<u32>() {
                Ok(channels) => Ok(channels),
                Err(_) => Err(anyhow!("Third value (channels) should have form \"<uint>ch\".")),
            },
            None => Err(anyhow!("Channel argument should have form \"<uint>ch\".")),
        }?;

        Ok(Self { frames_per_second: frame_rate, sample_type, channels })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleType {
    Uint8,
    Int16,
    Int32,
    Float32,
}

impl SampleType {
    pub const fn size(&self) -> SampleSize {
        match self {
            Self::Uint8 => SampleSize::from_full_bits(BITS_8),
            Self::Int16 => SampleSize::from_full_bits(BITS_16),
            // Assuming Int32 is really "24 in 32".
            Self::Int32 => SampleSize::from_partial_bits(BITS_24, BITS_32).unwrap(),
            Self::Float32 => SampleSize::from_full_bits(BITS_32),
        }
    }

    pub const fn silence_value(&self) -> u8 {
        match self {
            Self::Uint8 => 128,
            _ => 0,
        }
    }
}

impl Display for SampleType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Uint8 => "uint8",
            Self::Int16 => "int16",
            Self::Int32 => "int32",
            Self::Float32 => "float32",
        };
        f.write_str(s)
    }
}

impl FromStr for SampleType {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Error> {
        match s {
            "uint8" => Ok(Self::Uint8),
            "int16" => Ok(Self::Int16),
            "int32" => Ok(Self::Int32),
            "float32" => Ok(Self::Float32),
            _ => Err(anyhow!("Invalid sampletype: {}.", s)),
        }
    }
}

impl From<SampleType> for fmedia::AudioSampleFormat {
    fn from(item: SampleType) -> fmedia::AudioSampleFormat {
        match item {
            SampleType::Uint8 => Self::Unsigned8,
            SampleType::Int16 => Self::Signed16,
            SampleType::Int32 => Self::Signed24In32,
            SampleType::Float32 => Self::Float,
        }
    }
}

impl From<SampleType> for faudio::SampleType {
    fn from(value: SampleType) -> Self {
        match value {
            SampleType::Uint8 => Self::Uint8,
            SampleType::Int16 => Self::Int16,
            SampleType::Int32 => Self::Int32,
            SampleType::Float32 => Self::Float32,
        }
    }
}

impl From<SampleType> for fhaudio::SampleFormat {
    fn from(value: SampleType) -> Self {
        match value {
            SampleType::Uint8 => Self::PcmUnsigned,
            SampleType::Int16 | SampleType::Int32 => Self::PcmSigned,
            SampleType::Float32 => Self::PcmFloat,
        }
    }
}

impl From<SampleType> for hound::SampleFormat {
    fn from(value: SampleType) -> Self {
        match value {
            SampleType::Uint8 | SampleType::Int16 | SampleType::Int32 => Self::Int,
            SampleType::Float32 => Self::Float,
        }
    }
}

impl From<fmedia::AudioSampleFormat> for SampleType {
    fn from(item: fmedia::AudioSampleFormat) -> Self {
        match item {
            fmedia::AudioSampleFormat::Unsigned8 => Self::Uint8,
            fmedia::AudioSampleFormat::Signed16 => Self::Int16,
            fmedia::AudioSampleFormat::Signed24In32 => Self::Int32,
            fmedia::AudioSampleFormat::Float => Self::Float32,
        }
    }
}

impl TryFrom<faudio::SampleType> for SampleType {
    type Error = String;

    fn try_from(value: faudio::SampleType) -> Result<Self, Self::Error> {
        match value {
            faudio::SampleType::Uint8 => Ok(Self::Uint8),
            faudio::SampleType::Int16 => Ok(Self::Int16),
            faudio::SampleType::Int32 => Ok(Self::Int32),
            faudio::SampleType::Float32 => Ok(Self::Float32),
            other => Err(format!("unsupported SampleType: {:?}", other)),
        }
    }
}

impl From<(hound::SampleFormat, u16 /* bits per sample */)> for SampleType {
    fn from(value: (hound::SampleFormat, u16)) -> Self {
        let (sample_format, bits_per_sample) = value;
        match sample_format {
            hound::SampleFormat::Int => match bits_per_sample {
                0..=8 => Self::Uint8,
                9..=16 => Self::Int16,
                17.. => Self::Int32,
            },
            hound::SampleFormat::Float => Self::Float32,
        }
    }
}

impl TryFrom<(fhaudio::SampleFormat, SampleSize)> for SampleType {
    type Error = String;

    fn try_from(value: (fhaudio::SampleFormat, SampleSize)) -> Result<Self, Self::Error> {
        let (sample_format, sample_size) = value;
        let (valid_bits, total_bits) =
            (sample_size.valid_bits().get(), sample_size.total_bits().get());
        match sample_format {
            fhaudio::SampleFormat::PcmUnsigned => match (valid_bits, total_bits) {
                (8, 8) => Some(Self::Uint8),
                _ => None,
            },
            fhaudio::SampleFormat::PcmSigned => match (valid_bits, total_bits) {
                (16, 16) => Some(Self::Int16),
                (24 | 32, 32) => Some(Self::Int32),
                _ => None,
            },
            fhaudio::SampleFormat::PcmFloat => match (valid_bits, total_bits) {
                (32, 32) => Some(Self::Float32),
                _ => None,
            },
        }
        .ok_or_else(|| "unsupported sample size".to_string())
    }
}

/// The size of a single sample as a total number of bits, and the number of
/// "valid" bits.
///
/// The number of valid bits is always less than or equal to the total.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleSize {
    valid_bits: NonZeroU32,
    total_bits: NonZeroU32,
}

impl SampleSize {
    /// Returns a [SampleSize] where all given bits are valid.
    pub const fn from_full_bits(total_bits: NonZeroU32) -> Self {
        Self { valid_bits: total_bits, total_bits }
    }

    /// Returns a [SampleSize] where some bits are valid, if the number
    /// of valid bits is less than or equal to the total bits.
    pub const fn from_partial_bits(valid_bits: NonZeroU32, total_bits: NonZeroU32) -> Option<Self> {
        if valid_bits.get() > total_bits.get() {
            return None;
        }
        Some(Self { valid_bits, total_bits })
    }

    pub const fn total_bits(&self) -> NonZeroU32 {
        self.total_bits
    }

    pub const fn total_bytes(&self) -> NonZeroU32 {
        // It's safe to unwrap because `div_ceil` will always return at least 1 since `total_bits` is non-zero.
        NonZeroU32::new(self.total_bits.get().div_ceil(8)).unwrap()
    }

    pub const fn valid_bits(&self) -> NonZeroU32 {
        self.valid_bits
    }
}

impl Display for SampleSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}in{}", self.valid_bits, self.total_bits)
    }
}

impl FromStr for SampleSize {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let Some((valid_bits_str, total_bits_str)) = s.split_once("in") else {
            return Err(format!("Invalid sample size: {}. Expected: <ValidBits>in<TotalBits>", s));
        };
        let valid_bits = valid_bits_str
            .parse::<NonZeroU32>()
            .map_err(|err| format!("Invalid valid bits: {}", err))?;
        let total_bits = total_bits_str
            .parse::<NonZeroU32>()
            .map_err(|err| format!("Invalid total bits: {}", err))?;
        let sample_size = Self::from_partial_bits(valid_bits, total_bits).ok_or_else(|| {
            format!(
                "Invalid sample size: {}. Valid bits must be less than or equal to total bits.",
                s
            )
        })?;
        Ok(sample_size)
    }
}

/// Parses a Duration from string.
pub fn parse_duration(value: &str) -> Result<Duration, String> {
    let re = Regex::new(DURATION_REGEX).map_err(|e| format!("Could not create regex: {}", e))?;
    let captures = re
        .captures(value)
        .ok_or_else(|| format!("Durations must be specified in the form {}.", DURATION_REGEX))?;
    let number: u64 = captures[1].parse().map_err(|e| format!("Could not parse number: {}", e))?;
    let unit = &captures[2];

    match unit {
        "ms" => Ok(Duration::from_millis(number)),
        "s" => Ok(Duration::from_secs(number)),
        "m" => Ok(Duration::from_secs(number * 60)),
        "h" => Ok(Duration::from_secs(number * 3600)),
        _ => Err(format!(
            "Invalid duration string \"{}\"; must be of the form {}.",
            value, DURATION_REGEX
        )),
    }
}

pub fn str_to_clock(src: &str) -> Result<fac::ClockType, String> {
    match src.to_lowercase().as_str() {
        "flexible" => Ok(fac::ClockType::Flexible(fac::Flexible)),
        "monotonic" => Ok(fac::ClockType::SystemMonotonic(fac::SystemMonotonic)),
        _ => {
            let splits: Vec<&str> = src.split(",").collect();
            if splits[0] == "custom" {
                let rate_adjust =
                    if splits.len() > 1 { splits[1].parse::<i32>().ok() } else { None };

                let offset = if splits.len() > 2 { splits[2].parse::<i32>().ok() } else { None };

                Ok(fac::ClockType::Custom(fac::CustomClockConfig {
                    rate_adjust,
                    offset,
                    ..Default::default()
                }))
            } else {
                Err(format!("Invalid clock argument: {}.", src))
            }
        }
    }
}

#[cfg(test)]
pub mod test {
    use super::*;
    use test_case::test_case;

    #[test_case(
        "48000,uint8,2ch",
        Format { frames_per_second: 48000, sample_type: SampleType::Uint8, channels: 2 };
        "48000,uint8,2ch"
    )]
    #[test_case(
        "44100,float32,1ch",
        Format { frames_per_second: 44100, sample_type: SampleType::Float32, channels: 1 };
        "44100,float32,1ch"
    )]
    fn test_format_display_parse(s: &str, format: Format) {
        assert_eq!(s.parse::<Format>().unwrap(), format);
        assert_eq!(format.to_string(), s);
    }

    #[test_case("44100,float,1ch"; "bad sample type")]
    #[test_case("44100"; "missing sample type and channels")]
    #[test_case("44100,float32,1"; "invalid channels")]
    #[test_case("44100,float32"; "missing channels")]
    #[test_case(",,"; "empty components")]
    fn test_format_parse_invalid(s: &str) {
        assert!(s.parse::<Format>().is_err());
    }

    #[test_case(
        "16in32",
        SampleSize::from_partial_bits(BITS_16, BITS_32).unwrap();
        "16 valid 32 total"
    )]
    #[test_case(
        "32in32",
        SampleSize::from_full_bits(BITS_32);
        "32 valid 32 total"
    )]
    fn test_samplesize_display_parse(s: &str, sample_size: SampleSize) {
        assert_eq!(s.parse::<SampleSize>().unwrap(), sample_size);
        assert_eq!(sample_size.to_string(), s);
    }
}
