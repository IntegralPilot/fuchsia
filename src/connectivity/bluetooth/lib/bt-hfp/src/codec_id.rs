// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::format_err;
use thiserror::Error;
use {fidl_fuchsia_bluetooth_bredr as bredr, fidl_fuchsia_media as media};

use crate::audio;

/// Codec IDs. See HFP 1.8, Section 10 / Appendix B.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CodecId(u8);

#[derive(Clone, Copy, Debug, Error, Eq, Hash, PartialEq)]
#[error("Codec ID {id:?} was not an 8 bit value")]
pub struct CodecOutOfRangeError {
    id: i64,
}

impl CodecId {
    pub const CVSD: CodecId = CodecId(0x01);
    pub const MSBC: CodecId = CodecId(0x02);
}

impl From<u8> for CodecId {
    fn from(x: u8) -> Self {
        Self(x)
    }
}

impl TryFrom<i64> for CodecId {
    type Error = CodecOutOfRangeError;

    fn try_from(x: i64) -> Result<Self, CodecOutOfRangeError> {
        if x > 255 || x < 0 {
            return Err(CodecOutOfRangeError { id: x });
        } else {
            Ok((x as u8).into())
        }
    }
}

impl Into<u8> for CodecId {
    fn into(self) -> u8 {
        self.0
    }
}

// Convenience conversions for interacting with AT library.
// TODO(https://fxbug.dev/71403): Remove this once AT library supports specifying correct widths.
impl Into<i64> for CodecId {
    fn into(self) -> i64 {
        self.0 as i64
    }
}

fn unsupported_codec_id(codec: CodecId) -> audio::Error {
    audio::Error::UnsupportedParameters { source: format_err!("Unknown CodecId: {codec:?}") }
}

impl TryFrom<CodecId> for media::EncoderSettings {
    type Error = audio::Error;

    fn try_from(value: CodecId) -> Result<Self, Self::Error> {
        match value {
            CodecId::MSBC => Ok(media::EncoderSettings::Msbc(Default::default())),
            CodecId::CVSD => Ok(media::EncoderSettings::Cvsd(Default::default())),
            _ => Err(unsupported_codec_id(value)),
        }
    }
}

impl TryFrom<CodecId> for media::PcmFormat {
    type Error = audio::Error;

    fn try_from(value: CodecId) -> Result<Self, Self::Error> {
        let frames_per_second = match value {
            CodecId::CVSD => 64000,
            CodecId::MSBC => 16000,
            _ => return Err(unsupported_codec_id(value)),
        };
        Ok(media::PcmFormat {
            pcm_mode: media::AudioPcmMode::Linear,
            bits_per_sample: 16,
            frames_per_second,
            channel_map: vec![media::AudioChannelId::Lf],
        })
    }
}

impl TryFrom<CodecId> for media::DomainFormat {
    type Error = audio::Error;

    fn try_from(value: CodecId) -> Result<Self, Self::Error> {
        Ok(media::DomainFormat::Audio(media::AudioFormat::Uncompressed(
            media::AudioUncompressedFormat::Pcm(media::PcmFormat::try_from(value)?),
        )))
    }
}

impl TryFrom<CodecId> for fidl_fuchsia_hardware_audio::DaiSupportedFormats {
    type Error = audio::Error;

    fn try_from(value: CodecId) -> Result<Self, Self::Error> {
        let frames_per_second = match value {
            CodecId::CVSD => 64000,
            CodecId::MSBC => 16000,
            _ => return Err(unsupported_codec_id(value)),
        };
        use fidl_fuchsia_hardware_audio::*;
        Ok(DaiSupportedFormats {
            number_of_channels: vec![1],
            sample_formats: vec![fidl_fuchsia_hardware_audio::DaiSampleFormat::PcmSigned],
            frame_formats: vec![DaiFrameFormat::FrameFormatStandard(DaiFrameFormatStandard::I2S)],
            frame_rates: vec![frames_per_second],
            bits_per_slot: vec![16],
            bits_per_sample: vec![16],
        })
    }
}

#[cfg(test)]
impl TryFrom<CodecId> for fidl_fuchsia_hardware_audio::Format {
    type Error = audio::Error;
    fn try_from(value: CodecId) -> Result<Self, Self::Error> {
        let frame_rate = match value {
            CodecId::CVSD => 64000,
            CodecId::MSBC => 16000,
            _ => {
                return Err(audio::Error::UnsupportedParameters {
                    source: format_err!("Unsupported CodecID {value}"),
                });
            }
        };
        Ok(Self {
            pcm_format: Some(fidl_fuchsia_hardware_audio::PcmFormat {
                number_of_channels: 1u8,
                sample_format: fidl_fuchsia_hardware_audio::SampleFormat::PcmSigned,
                bytes_per_sample: 2u8,
                valid_bits_per_sample: 16u8,
                frame_rate,
            }),
            ..Default::default()
        })
    }
}

impl PartialEq<i64> for CodecId {
    fn eq(&self, other: &i64) -> bool {
        self.0 as i64 == *other
    }
}

impl std::fmt::Display for CodecId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            0x01 => write!(f, "{}", "CVSD"),
            0x02 => write!(f, "{}", "MSBC"),
            unknown => write!(f, "Unknown({:#x})", unknown),
        }
    }
}

impl CodecId {
    pub fn is_supported(&self) -> bool {
        match self {
            &CodecId::MSBC | &CodecId::CVSD => true,
            _ => false,
        }
    }

    pub fn oob_bytes(&self) -> Vec<u8> {
        use bt_a2dp::media_types::{
            SbcAllocation, SbcBlockCount, SbcChannelMode, SbcCodecInfo, SbcSamplingFrequency,
            SbcSubBands,
        };
        match self {
            &CodecId::MSBC => SbcCodecInfo::new(
                SbcSamplingFrequency::FREQ16000HZ,
                SbcChannelMode::MONO,
                SbcBlockCount::SIXTEEN,
                SbcSubBands::EIGHT,
                SbcAllocation::LOUDNESS,
                26,
                26,
            )
            .unwrap()
            .to_bytes()
            .to_vec(),
            // CVSD has no oob_bytes
            _ => vec![],
        }
    }

    pub fn mime_type(&self) -> Result<&str, audio::Error> {
        match self {
            &CodecId::MSBC => Ok("audio/msbc"),
            &CodecId::CVSD => Ok("audio/cvsd"),
            _ => Err(audio::Error::UnsupportedParameters { source: format_err!("codec {self}") }),
        }
    }

    pub fn from_parameter_set(param_set: &bredr::HfpParameterSet) -> CodecId {
        use bredr::HfpParameterSet::*;
        match param_set {
            T2 | T1 => CodecId::MSBC,
            _ => CodecId::CVSD,
        }
    }
}

pub fn codecs_to_string(codecs: &Vec<CodecId>) -> String {
    let codecs_string: Vec<String> = codecs.iter().map(ToString::to_string).collect();
    let codecs_string: Vec<&str> = codecs_string.iter().map(AsRef::as_ref).collect();
    let joined = codecs_string.join(", ");
    joined
}

#[cfg(test)]
mod test {
    use super::*;

    #[fuchsia::test]
    fn codecs_format() {
        let cvsd = CodecId(0x1);
        let mbsc = CodecId(0x2);
        let unknown = CodecId(0xf);

        let cvsd_string = format!("{:}", cvsd);
        assert_eq!(String::from("CVSD"), cvsd_string);

        let mbsc_string = format!("{:}", mbsc);
        assert_eq!(String::from("MSBC"), mbsc_string);

        let unknown_string = format!("{:}", unknown);
        assert_eq!(String::from("Unknown(0xf)"), unknown_string);

        let joined_string = codecs_to_string(&vec![cvsd, mbsc, unknown]);
        assert_eq!(String::from("CVSD, MSBC, Unknown(0xf)"), joined_string);
    }
}
