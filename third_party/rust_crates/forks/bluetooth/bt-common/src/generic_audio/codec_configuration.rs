// Copyright 2024 Google LLC
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::HashSet;
use std::ops::RangeBounds;

use crate::core::ltv::LtValue;
use crate::decodable_enum;

use super::AudioLocation;

#[derive(Copy, Clone, Hash, PartialEq, Eq, Debug)]
pub enum CodecConfigurationType {
    SamplingFrequency,
    FrameDuration,
    AudioChannelAllocation,
    OctetsPerCodecFrame,
    CodecFramesPerSdu,
}

impl From<CodecConfigurationType> for u8 {
    fn from(value: CodecConfigurationType) -> Self {
        match value {
            CodecConfigurationType::SamplingFrequency => 1,
            CodecConfigurationType::FrameDuration => 2,
            CodecConfigurationType::AudioChannelAllocation => 3,
            CodecConfigurationType::OctetsPerCodecFrame => 4,
            CodecConfigurationType::CodecFramesPerSdu => 5,
        }
    }
}

decodable_enum! {
    pub enum FrameDuration<u8, crate::packet_encoding::Error, OutOfRange> {
        SevenFiveMs = 0x00,
        TenMs = 0x01,
    }
}

decodable_enum! {
    pub enum SamplingFrequency<u8, crate::packet_encoding::Error, OutOfRange> {
        F8000Hz = 0x01,
        F11025Hz = 0x02,
        F16000Hz = 0x03,
        F22050Hz = 0x04,
        F24000Hz = 0x05,
        F32000Hz = 0x06,
        F44100Hz = 0x07,
        F48000Hz = 0x08,
        F88200Hz = 0x09,
        F96000Hz = 0x0A,
        F176400Hz = 0x0B,
        F192000Hz = 0x0C,
        F384000Hz = 0x0D,
    }
}

/// Codec Configuration LTV Structures
///
/// Defined in Assigned Numbers Section 6.12.5.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CodecConfiguration {
    SamplingFrequency(SamplingFrequency),
    FrameDuration(FrameDuration),
    AudioChannelAllocation(std::collections::HashSet<AudioLocation>),
    OctetsPerCodecFrame(u16),
    CodecFramesPerSdu(u8),
}

impl LtValue for CodecConfiguration {
    type Type = CodecConfigurationType;

    const NAME: &'static str = "Codec Compatability";

    fn type_from_octet(x: u8) -> Option<Self::Type> {
        match x {
            1 => Some(CodecConfigurationType::SamplingFrequency),
            2 => Some(CodecConfigurationType::FrameDuration),
            3 => Some(CodecConfigurationType::AudioChannelAllocation),
            4 => Some(CodecConfigurationType::OctetsPerCodecFrame),
            5 => Some(CodecConfigurationType::CodecFramesPerSdu),
            _ => None,
        }
    }

    fn length_range_from_type(ty: Self::Type) -> std::ops::RangeInclusive<u8> {
        match ty {
            CodecConfigurationType::SamplingFrequency => 2..=2,
            CodecConfigurationType::FrameDuration => 2..=2,
            CodecConfigurationType::AudioChannelAllocation => 5..=5,
            CodecConfigurationType::OctetsPerCodecFrame => 3..=3,
            CodecConfigurationType::CodecFramesPerSdu => 2..=2,
        }
    }

    fn into_type(&self) -> Self::Type {
        match self {
            CodecConfiguration::SamplingFrequency(_) => CodecConfigurationType::SamplingFrequency,
            CodecConfiguration::FrameDuration(_) => CodecConfigurationType::FrameDuration,
            CodecConfiguration::AudioChannelAllocation(_) => {
                CodecConfigurationType::AudioChannelAllocation
            }
            CodecConfiguration::OctetsPerCodecFrame(_) => {
                CodecConfigurationType::OctetsPerCodecFrame
            }
            CodecConfiguration::CodecFramesPerSdu(_) => CodecConfigurationType::CodecFramesPerSdu,
        }
    }

    fn value_encoded_len(&self) -> u8 {
        // All the CodecCapabilities are constant length. Remove the type octet.
        let range = Self::length_range_from_type(self.into_type());
        let std::ops::Bound::Included(len) = range.start_bound() else {
            unreachable!();
        };
        (len - 1).into()
    }

    fn encode_value(&self, buf: &mut [u8]) -> Result<(), crate::packet_encoding::Error> {
        match self {
            CodecConfiguration::SamplingFrequency(freq) => buf[0] = u8::from(*freq),
            CodecConfiguration::FrameDuration(fd) => buf[0] = u8::from(*fd),
            CodecConfiguration::AudioChannelAllocation(audio_locations) => {
                let locations: Vec<&AudioLocation> = audio_locations.into_iter().collect();
                buf[0..4]
                    .copy_from_slice(&AudioLocation::to_bits(locations.into_iter()).to_le_bytes());
            }
            CodecConfiguration::OctetsPerCodecFrame(octets) => {
                buf[0..2].copy_from_slice(&octets.to_le_bytes())
            }
            CodecConfiguration::CodecFramesPerSdu(frames) => buf[0] = *frames,
        };
        Ok(())
    }

    fn decode_value(
        ty: &CodecConfigurationType,
        buf: &[u8],
    ) -> Result<Self, crate::packet_encoding::Error> {
        match ty {
            CodecConfigurationType::SamplingFrequency => {
                let freq = SamplingFrequency::try_from(buf[0])?;
                Ok(Self::SamplingFrequency(freq))
            }
            CodecConfigurationType::FrameDuration => {
                let fd = FrameDuration::try_from(buf[0])?;
                Ok(Self::FrameDuration(fd))
            }
            CodecConfigurationType::AudioChannelAllocation => {
                let raw_value = u32::from_le_bytes(buf[0..4].try_into().unwrap());
                let locations = AudioLocation::from_bits(raw_value).collect::<HashSet<_>>();
                Ok(Self::AudioChannelAllocation(locations))
            }
            CodecConfigurationType::OctetsPerCodecFrame => {
                let value = u16::from_le_bytes(buf[0..2].try_into().unwrap());
                Ok(Self::OctetsPerCodecFrame(value))
            }
            CodecConfigurationType::CodecFramesPerSdu => Ok(Self::CodecFramesPerSdu(buf[0])),
        }
    }
}

#[cfg(test)]
mod tests {}
