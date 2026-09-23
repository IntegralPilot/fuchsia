// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::format::SampleSize;
use flex_fuchsia_hardware_audio as fhaudio;
use std::fmt::Display;
use std::num::NonZeroU32;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DaiFormat {
    pub number_of_channels: u32,
    pub channels_to_use_bitmask: u64,
    pub sample_format: DaiSampleFormat,
    pub frame_format: DaiFrameFormat,
    pub frame_rate: u32,
    // Stores `bits_per_slot` as `total_bits` and `bits_per_sample` as `valid_bits`.
    pub sample_size: SampleSize,
}

impl Display for DaiFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{},{}ch,0x{:x},{},{},{}",
            self.frame_rate,
            self.number_of_channels,
            self.channels_to_use_bitmask,
            self.sample_format,
            self.sample_size,
            self.frame_format
        )
    }
}

impl FromStr for DaiFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err("No DAI format specified.".to_string());
        };

        let splits: Vec<&str> = s.split(",").collect();

        if splits.len() != 6 {
            return Err("Expected 6 semicolon-separated values: \
                <FrameRate>,<NumChannels>,<ChannelsToUseBitmask>,\
                <DaiSampleFormat>,<SampleSize>,<DaiFrameFormat>"
                .to_string());
        }

        let frame_rate =
            splits[0].parse::<u32>().map_err(|_| format!("Invalid frame rate: {}", splits[0]))?;
        let number_of_channels = splits[1]
            .strip_suffix("ch")
            .ok_or_else(|| "Number of channels must be followed by 'ch'".to_string())?
            .parse::<u32>()
            .map_err(|_| format!("Invalid number of channels: {}", splits[1]))?;
        let channels_to_use_bitmask = {
            let hex_str = splits[2].strip_prefix("0x").ok_or_else(|| {
                "Channels to use bitmask must be a hexadecimal number starting with '0x'"
                    .to_string()
            })?;
            u64::from_str_radix(hex_str, 16)
                .map_err(|_| format!("Invalid channels to use bitmask: {}", splits[2]))?
        };
        let sample_format = splits[3].parse::<DaiSampleFormat>()?;
        let sample_size = splits[4].parse::<SampleSize>()?;
        let frame_format = splits[5].parse::<DaiFrameFormat>()?;

        Ok(Self {
            number_of_channels,
            channels_to_use_bitmask,
            sample_format,
            frame_format,
            frame_rate,
            sample_size,
        })
    }
}

impl TryFrom<fhaudio::DaiFormat> for DaiFormat {
    type Error = String;

    fn try_from(value: fhaudio::DaiFormat) -> Result<Self, Self::Error> {
        let bits_per_slot = NonZeroU32::new(value.bits_per_slot as u32)
            .ok_or_else(|| "'bits_per_slot' cannot be zero".to_string())?;
        let bits_per_sample = NonZeroU32::new(value.bits_per_sample as u32)
            .ok_or_else(|| "'bits_per_sample' cannot be zero".to_string())?;
        let sample_size = SampleSize::from_partial_bits(bits_per_sample, bits_per_slot)
            .ok_or_else(|| {
                "'bits_per_sample' must be less than or equal to 'bits_per_slot'".to_string()
            })?;
        Ok(Self {
            number_of_channels: value.number_of_channels,
            channels_to_use_bitmask: value.channels_to_use_bitmask,
            sample_format: value.sample_format.into(),
            frame_format: value.frame_format.into(),
            frame_rate: value.frame_rate,
            sample_size,
        })
    }
}

impl From<DaiFormat> for fhaudio::DaiFormat {
    fn from(value: DaiFormat) -> Self {
        Self {
            number_of_channels: value.number_of_channels,
            channels_to_use_bitmask: value.channels_to_use_bitmask,
            sample_format: value.sample_format.into(),
            frame_format: value.frame_format.into(),
            frame_rate: value.frame_rate,
            bits_per_slot: value.sample_size.total_bits().get() as u8,
            bits_per_sample: value.sample_size.valid_bits().get() as u8,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct DaiFormatSet {
    pub number_of_channels: Vec<u32>,
    pub sample_formats: Vec<DaiSampleFormat>,
    pub frame_formats: Vec<DaiFrameFormat>,
    pub frame_rates: Vec<u32>,
    pub bits_per_slot: Vec<u8>,
    pub bits_per_sample: Vec<u8>,
}

impl From<fhaudio::DaiSupportedFormats> for DaiFormatSet {
    fn from(value: fhaudio::DaiSupportedFormats) -> Self {
        Self {
            number_of_channels: value.number_of_channels,
            sample_formats: value.sample_formats.into_iter().map(From::from).collect(),
            frame_formats: value.frame_formats.into_iter().map(From::from).collect(),
            frame_rates: value.frame_rates,
            bits_per_slot: value.bits_per_slot,
            bits_per_sample: value.bits_per_sample,
        }
    }
}

impl From<DaiFormatSet> for fhaudio::DaiSupportedFormats {
    fn from(value: DaiFormatSet) -> Self {
        Self {
            number_of_channels: value.number_of_channels,
            sample_formats: value.sample_formats.into_iter().map(From::from).collect(),
            frame_formats: value.frame_formats.into_iter().map(From::from).collect(),
            frame_rates: value.frame_rates,
            bits_per_slot: value.bits_per_slot,
            bits_per_sample: value.bits_per_sample,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaiSampleFormat {
    Pdm,
    PcmSigned,
    PcmUnsigned,
    PcmFloat,
}

impl Display for DaiSampleFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            DaiSampleFormat::Pdm => "pdm",
            DaiSampleFormat::PcmSigned => "pcm_signed",
            DaiSampleFormat::PcmUnsigned => "pcm_unsigned",
            DaiSampleFormat::PcmFloat => "pcm_float",
        };
        f.write_str(s)
    }
}

impl FromStr for DaiSampleFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pdm" => Ok(Self::Pdm),
            "pcm_signed" => Ok(Self::PcmSigned),
            "pcm_unsigned" => Ok(Self::PcmUnsigned),
            "pcm_float" => Ok(Self::PcmFloat),
            _ => Err(format!("Invalid DAI sample format: {}", s)),
        }
    }
}

impl From<fhaudio::DaiSampleFormat> for DaiSampleFormat {
    fn from(value: fhaudio::DaiSampleFormat) -> Self {
        match value {
            fhaudio::DaiSampleFormat::Pdm => Self::Pdm,
            fhaudio::DaiSampleFormat::PcmSigned => Self::PcmSigned,
            fhaudio::DaiSampleFormat::PcmUnsigned => Self::PcmUnsigned,
            fhaudio::DaiSampleFormat::PcmFloat => Self::PcmFloat,
        }
    }
}

impl From<DaiSampleFormat> for fhaudio::DaiSampleFormat {
    fn from(value: DaiSampleFormat) -> Self {
        match value {
            DaiSampleFormat::Pdm => Self::Pdm,
            DaiSampleFormat::PcmSigned => Self::PcmSigned,
            DaiSampleFormat::PcmUnsigned => Self::PcmUnsigned,
            DaiSampleFormat::PcmFloat => Self::PcmFloat,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaiFrameFormat {
    Standard(DaiFrameFormatStandard),
    Custom(DaiFrameFormatCustom),
}

impl Display for DaiFrameFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DaiFrameFormat::Standard(standard) => standard.fmt(f),
            DaiFrameFormat::Custom(custom) => {
                f.write_str("custom:")?;
                custom.fmt(f)
            }
        }
    }
}

impl FromStr for DaiFrameFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(custom_str) = s.strip_prefix("custom:") {
            let custom_fmt = custom_str.parse::<DaiFrameFormatCustom>()?;
            return Ok(Self::Custom(custom_fmt));
        }
        let standard_fmt = s.parse::<DaiFrameFormatStandard>()?;
        Ok(Self::Standard(standard_fmt))
    }
}

impl From<fhaudio::DaiFrameFormat> for DaiFrameFormat {
    fn from(value: fhaudio::DaiFrameFormat) -> Self {
        match value {
            fhaudio::DaiFrameFormat::FrameFormatStandard(standard) => {
                Self::Standard(standard.into())
            }
            fhaudio::DaiFrameFormat::FrameFormatCustom(custom) => Self::Custom(custom.into()),
        }
    }
}

impl From<DaiFrameFormat> for fhaudio::DaiFrameFormat {
    fn from(value: DaiFrameFormat) -> Self {
        match value {
            DaiFrameFormat::Standard(standard) => Self::FrameFormatStandard(standard.into()),
            DaiFrameFormat::Custom(custom) => Self::FrameFormatCustom(custom.into()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaiFrameFormatStandard {
    None,
    I2S,
    StereoLeft,
    StereoRight,
    Tdm1,
    Tdm2,
    Tdm3,
}

impl Display for DaiFrameFormatStandard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            DaiFrameFormatStandard::None => "none",
            DaiFrameFormatStandard::I2S => "i2s",
            DaiFrameFormatStandard::StereoLeft => "stereo_left",
            DaiFrameFormatStandard::StereoRight => "stereo_right",
            DaiFrameFormatStandard::Tdm1 => "tdm1",
            DaiFrameFormatStandard::Tdm2 => "tdm2",
            DaiFrameFormatStandard::Tdm3 => "tdm3",
        };
        f.write_str(s)
    }
}

impl FromStr for DaiFrameFormatStandard {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "none" => Ok(DaiFrameFormatStandard::None),
            "i2s" => Ok(DaiFrameFormatStandard::I2S),
            "stereo_left" => Ok(DaiFrameFormatStandard::StereoLeft),
            "stereo_right" => Ok(DaiFrameFormatStandard::StereoRight),
            "tdm1" => Ok(DaiFrameFormatStandard::Tdm1),
            "tdm2" => Ok(DaiFrameFormatStandard::Tdm2),
            "tdm3" => Ok(DaiFrameFormatStandard::Tdm3),
            _ => Err(format!("Invalid DAI frame format: {}", s)),
        }
    }
}

impl From<fhaudio::DaiFrameFormatStandard> for DaiFrameFormatStandard {
    fn from(value: fhaudio::DaiFrameFormatStandard) -> Self {
        match value {
            fhaudio::DaiFrameFormatStandard::None => Self::None,
            fhaudio::DaiFrameFormatStandard::I2S => Self::I2S,
            fhaudio::DaiFrameFormatStandard::StereoLeft => Self::StereoLeft,
            fhaudio::DaiFrameFormatStandard::StereoRight => Self::StereoRight,
            fhaudio::DaiFrameFormatStandard::Tdm1 => Self::Tdm1,
            fhaudio::DaiFrameFormatStandard::Tdm2 => Self::Tdm2,
            fhaudio::DaiFrameFormatStandard::Tdm3 => Self::Tdm3,
        }
    }
}

impl From<DaiFrameFormatStandard> for fhaudio::DaiFrameFormatStandard {
    fn from(value: DaiFrameFormatStandard) -> Self {
        match value {
            DaiFrameFormatStandard::None => Self::None,
            DaiFrameFormatStandard::I2S => Self::I2S,
            DaiFrameFormatStandard::StereoLeft => Self::StereoLeft,
            DaiFrameFormatStandard::StereoRight => Self::StereoRight,
            DaiFrameFormatStandard::Tdm1 => Self::Tdm1,
            DaiFrameFormatStandard::Tdm2 => Self::Tdm2,
            DaiFrameFormatStandard::Tdm3 => Self::Tdm3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DaiFrameFormatCustom {
    pub justification: DaiFrameFormatJustification,
    pub clocking: DaiFrameFormatClocking,
    pub frame_sync_sclks_offset: i8,
    pub frame_sync_size: u8,
}

impl Display for DaiFrameFormatCustom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{};{};{};{}",
            self.justification, self.clocking, self.frame_sync_sclks_offset, self.frame_sync_size
        )
    }
}

impl FromStr for DaiFrameFormatCustom {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err("No DAI frame format specified.".to_string());
        };

        let splits: Vec<&str> = s.split(";").collect();

        if splits.len() != 4 {
            return Err("Expected 4 semicolon-separated values: \
                <Justification>;<Clocking>;<FrameSyncOffset>;<FrameSyncSize>"
                .to_string());
        }

        let justification = splits[0].parse::<DaiFrameFormatJustification>()?;
        let clocking = splits[1].parse::<DaiFrameFormatClocking>()?;
        let frame_sync_sclks_offset = splits[2]
            .parse::<i8>()
            .map_err(|_| format!("Invalid frame sync offset: {}", splits[2]))?;
        let frame_sync_size = splits[3]
            .parse::<u8>()
            .map_err(|_| format!("Invalid frame sync size: {}", splits[3]))?;

        Ok(Self { justification, clocking, frame_sync_sclks_offset, frame_sync_size })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaiFrameFormatJustification {
    Left,
    Right,
}

impl Display for DaiFrameFormatJustification {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            DaiFrameFormatJustification::Left => "left_justified",
            DaiFrameFormatJustification::Right => "right_justified",
        };
        f.write_str(s)
    }
}

impl FromStr for DaiFrameFormatJustification {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "left_justified" => Ok(DaiFrameFormatJustification::Left),
            "right_justified" => Ok(DaiFrameFormatJustification::Right),
            _ => Err(format!("Invalid DAI frame justification: {}", s)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaiFrameFormatClocking {
    RaisingSclk,
    FallingSclk,
}

impl Display for DaiFrameFormatClocking {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            DaiFrameFormatClocking::RaisingSclk => "raising_sclk",
            DaiFrameFormatClocking::FallingSclk => "falling_sclk",
        };
        f.write_str(s)
    }
}

impl FromStr for DaiFrameFormatClocking {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "raising_sclk" => Ok(DaiFrameFormatClocking::RaisingSclk),
            "falling_sclk" => Ok(DaiFrameFormatClocking::FallingSclk),
            _ => Err(format!("Invalid DAI frame clocking: {}", s)),
        }
    }
}

impl From<DaiFrameFormatCustom> for fhaudio::DaiFrameFormatCustom {
    fn from(value: DaiFrameFormatCustom) -> Self {
        Self {
            left_justified: match value.justification {
                DaiFrameFormatJustification::Left => true,
                DaiFrameFormatJustification::Right => false,
            },
            sclk_on_raising: match value.clocking {
                DaiFrameFormatClocking::RaisingSclk => true,
                DaiFrameFormatClocking::FallingSclk => false,
            },
            frame_sync_sclks_offset: value.frame_sync_sclks_offset,
            frame_sync_size: value.frame_sync_size,
        }
    }
}

impl From<fhaudio::DaiFrameFormatCustom> for DaiFrameFormatCustom {
    fn from(value: fhaudio::DaiFrameFormatCustom) -> Self {
        Self {
            justification: if value.left_justified {
                DaiFrameFormatJustification::Left
            } else {
                DaiFrameFormatJustification::Right
            },
            clocking: if value.sclk_on_raising {
                DaiFrameFormatClocking::RaisingSclk
            } else {
                DaiFrameFormatClocking::FallingSclk
            },
            frame_sync_sclks_offset: value.frame_sync_sclks_offset,
            frame_sync_size: value.frame_sync_size,
        }
    }
}

#[cfg(test)]
pub mod test {
    use super::*;
    use crate::format::{BITS_16, BITS_32};
    use test_case::test_case;

    #[test_case("none", DaiFrameFormat::Standard(DaiFrameFormatStandard::None); "standard: none")]
    #[test_case("i2s", DaiFrameFormat::Standard(DaiFrameFormatStandard::I2S); "standard: i2s")]
    #[test_case(
        "stereo_left",
        DaiFrameFormat::Standard(DaiFrameFormatStandard::StereoLeft);
        "standard: stereo_left"
    )]
    #[test_case(
        "stereo_right",
        DaiFrameFormat::Standard(DaiFrameFormatStandard::StereoRight);
        "standard: stereo_right"
    )]
    #[test_case("tdm1", DaiFrameFormat::Standard(DaiFrameFormatStandard::Tdm1); "standard: tdm1")]
    #[test_case("tdm2", DaiFrameFormat::Standard(DaiFrameFormatStandard::Tdm2); "standard: tdm2")]
    #[test_case("tdm3", DaiFrameFormat::Standard(DaiFrameFormatStandard::Tdm3); "standard: tdm3")]
    #[test_case(
        "custom:left_justified;raising_sclk;0;1",
        DaiFrameFormat::Custom(DaiFrameFormatCustom {
            justification: DaiFrameFormatJustification::Left,
            clocking: DaiFrameFormatClocking::RaisingSclk,
            frame_sync_sclks_offset: 0,
            frame_sync_size: 1,
        });
        "custom 1"
    )]
    #[test_case(
        "custom:right_justified;falling_sclk;-1;0",
        DaiFrameFormat::Custom(DaiFrameFormatCustom {
            justification: DaiFrameFormatJustification::Right,
            clocking: DaiFrameFormatClocking::FallingSclk,
            frame_sync_sclks_offset: -1,
            frame_sync_size: 0,
        });
        "custom 2"
    )]
    fn test_dai_frame_format_display_parse(s: &str, dai_frame_format: DaiFrameFormat) {
        assert_eq!(s.parse::<DaiFrameFormat>().unwrap(), dai_frame_format);
        assert_eq!(dai_frame_format.to_string(), s);
    }

    #[test]
    fn test_dai_format_from_to_hw() {
        let hw_dai_format = fhaudio::DaiFormat {
            number_of_channels: 2,
            channels_to_use_bitmask: 0x3,
            sample_format: fhaudio::DaiSampleFormat::PcmSigned,
            frame_format: fhaudio::DaiFrameFormat::FrameFormatStandard(
                fhaudio::DaiFrameFormatStandard::I2S,
            ),
            frame_rate: 48000,
            bits_per_slot: 32,
            bits_per_sample: 16,
        };

        let dai_format = DaiFormat {
            number_of_channels: 2,
            channels_to_use_bitmask: 0x3,
            sample_format: DaiSampleFormat::PcmSigned,
            frame_format: DaiFrameFormat::Standard(DaiFrameFormatStandard::I2S),
            frame_rate: 48000,
            sample_size: SampleSize::from_partial_bits(BITS_16, BITS_32).unwrap(),
        };

        assert_eq!(DaiFormat::try_from(hw_dai_format).unwrap(), dai_format);
        assert_eq!(fhaudio::DaiFormat::from(dai_format), hw_dai_format);
    }

    #[test_case(
        "48000,2ch,0x3,pcm_signed,16in32,i2s",
        DaiFormat {
            number_of_channels: 2,
            channels_to_use_bitmask: 0x3,
            sample_format: DaiSampleFormat::PcmSigned,
            frame_format: DaiFrameFormat::Standard(DaiFrameFormatStandard::I2S),
            frame_rate: 48000,
            sample_size: SampleSize::from_partial_bits(BITS_16, BITS_32).unwrap(),
        };
        "standard frame format"
    )]
    #[test_case(
        "96000,8ch,0xff,pcm_float,32in32,custom:right_justified;falling_sclk;-1;0",
        DaiFormat {
            number_of_channels: 8,
            channels_to_use_bitmask: 0xff,
            sample_format: DaiSampleFormat::PcmFloat,
            frame_format: DaiFrameFormat::Custom(DaiFrameFormatCustom {
                justification: DaiFrameFormatJustification::Right,
                clocking: DaiFrameFormatClocking::FallingSclk,
                frame_sync_sclks_offset: -1,
                frame_sync_size: 0,
            }),
            frame_rate: 96000,
            sample_size: SampleSize::from_full_bits(BITS_32),
        };
        "custom frame format"
    )]
    fn test_dai_format_display_parse(s: &str, dai_format: DaiFormat) {
        assert_eq!(s.parse::<DaiFormat>().unwrap(), dai_format);
        assert_eq!(dai_format.to_string(), s);
    }

    #[test_case("48000,2,0x3,pcm_signed,16in32,i2s"; "missing num channels suffix")]
    #[test_case("48000,2ch,3,pcm_signed,16in32,i2s"; "missing channels to use prefix")]
    #[test_case("48000,2ch,0xINVALID,pcm_signed,16in32,i2s"; "invalid channels to use")]
    #[test_case("48000,2ch,0x3,pcm_signed,32in16,i2s"; "invalid sample size")]
    #[test_case("48000,2ch,0x3,pcm_signed,16in32,custom:INVALID"; "invalid custom frame format")]
    fn test_dai_format_parse_invalid(s: &str) {
        assert!(s.parse::<DaiFormat>().is_err());
    }
}
