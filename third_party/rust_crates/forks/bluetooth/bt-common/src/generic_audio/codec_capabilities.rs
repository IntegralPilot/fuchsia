// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::HashSet;
use std::ops::RangeBounds;

use crate::core::ltv::LtValue;

#[derive(Copy, Clone, Hash, PartialEq, Eq, Debug)]
pub enum CodecCapabilityType {
    SupportedSamplingFrequencies,
    SupportedFrameDurations,
    SupportedAudioChannelCounts,
    SupportedOctetsPerCodecFrame,
    SupportedMaxCodecFramesPerSdu,
}

impl From<CodecCapabilityType> for u8 {
    fn from(value: CodecCapabilityType) -> Self {
        match value {
            CodecCapabilityType::SupportedSamplingFrequencies => 1,
            CodecCapabilityType::SupportedFrameDurations => 2,
            CodecCapabilityType::SupportedAudioChannelCounts => 3,
            CodecCapabilityType::SupportedOctetsPerCodecFrame => 4,
            CodecCapabilityType::SupportedMaxCodecFramesPerSdu => 5,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum FrameDurationSupport {
    SevenFiveMs,
    TenMs,
    BothNoPreference,
    PreferSevenFiveMs,
    PreferTenMs,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SamplingFrequency {
    F8000Hz,
    F11025Hz,
    F16000Hz,
    F22050Hz,
    F24000Hz,
    F32000Hz,
    F44100Hz,
    F48000Hz,
    F88200Hz,
    F96000Hz,
    F176400Hz,
    F192000Hz,
    F384000Hz,
}

impl SamplingFrequency {
    fn bitpos(&self) -> u8 {
        match self {
            SamplingFrequency::F8000Hz => 0,
            SamplingFrequency::F11025Hz => 1,
            SamplingFrequency::F16000Hz => 2,
            SamplingFrequency::F22050Hz => 3,
            SamplingFrequency::F24000Hz => 4,
            SamplingFrequency::F32000Hz => 5,
            SamplingFrequency::F44100Hz => 6,
            SamplingFrequency::F48000Hz => 7,
            SamplingFrequency::F88200Hz => 8,
            SamplingFrequency::F96000Hz => 9,
            SamplingFrequency::F176400Hz => 10,
            SamplingFrequency::F192000Hz => 11,
            SamplingFrequency::F384000Hz => 12,
        }
    }

    fn from_bitpos(value: u8) -> Option<Self> {
        match value {
            0 => Some(SamplingFrequency::F8000Hz),
            1 => Some(SamplingFrequency::F11025Hz),
            2 => Some(SamplingFrequency::F16000Hz),
            3 => Some(SamplingFrequency::F22050Hz),
            4 => Some(SamplingFrequency::F24000Hz),
            5 => Some(SamplingFrequency::F32000Hz),
            6 => Some(SamplingFrequency::F44100Hz),
            7 => Some(SamplingFrequency::F48000Hz),
            8 => Some(SamplingFrequency::F88200Hz),
            9 => Some(SamplingFrequency::F96000Hz),
            10 => Some(SamplingFrequency::F176400Hz),
            11 => Some(SamplingFrequency::F192000Hz),
            12 => Some(SamplingFrequency::F384000Hz),
            _ => None,
        }
    }
}

/// Codec Capability LTV Structures
///
/// Defined in Assigned Numbers Section 6.12.4.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CodecCapability {
    SupportedSamplingFrequencies(std::collections::HashSet<SamplingFrequency>),
    SupportedFrameDurations(FrameDurationSupport),
    SupportedAudioChannelCounts(std::collections::HashSet<u8>),
    SupportedMaxCodecFramesPerSdu(u8),
    SupportedOctetsPerCodecFrame { min: u16, max: u16 },
}

impl LtValue for CodecCapability {
    type Type = CodecCapabilityType;

    const NAME: &'static str = "Codec Compatability";

    fn type_from_octet(x: u8) -> Option<Self::Type> {
        match x {
            1 => Some(CodecCapabilityType::SupportedSamplingFrequencies),
            2 => Some(CodecCapabilityType::SupportedFrameDurations),
            3 => Some(CodecCapabilityType::SupportedAudioChannelCounts),
            4 => Some(CodecCapabilityType::SupportedOctetsPerCodecFrame),
            5 => Some(CodecCapabilityType::SupportedMaxCodecFramesPerSdu),
            _ => None,
        }
    }

    fn length_range_from_type(ty: Self::Type) -> std::ops::RangeInclusive<u8> {
        match ty {
            CodecCapabilityType::SupportedAudioChannelCounts => 2..=2,
            CodecCapabilityType::SupportedFrameDurations => 2..=2,
            CodecCapabilityType::SupportedMaxCodecFramesPerSdu => 2..=2,
            CodecCapabilityType::SupportedOctetsPerCodecFrame => 5..=5,
            CodecCapabilityType::SupportedSamplingFrequencies => 3..=3,
        }
    }

    fn into_type(&self) -> Self::Type {
        match self {
            CodecCapability::SupportedAudioChannelCounts(_) => {
                CodecCapabilityType::SupportedAudioChannelCounts
            }
            CodecCapability::SupportedFrameDurations(_) => {
                CodecCapabilityType::SupportedFrameDurations
            }
            CodecCapability::SupportedMaxCodecFramesPerSdu(_) => {
                CodecCapabilityType::SupportedMaxCodecFramesPerSdu
            }
            CodecCapability::SupportedOctetsPerCodecFrame { .. } => {
                CodecCapabilityType::SupportedOctetsPerCodecFrame
            }
            CodecCapability::SupportedSamplingFrequencies(_) => {
                CodecCapabilityType::SupportedSamplingFrequencies
            }
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
            CodecCapability::SupportedAudioChannelCounts(counts) => {
                buf[0] =
                    counts.iter().fold(
                        0,
                        |acc, count| if *count > 8 { acc } else { acc | (1 << (*count - 1)) },
                    );
            }
            CodecCapability::SupportedFrameDurations(support) => {
                buf[0] = match support {
                    FrameDurationSupport::SevenFiveMs => 0b000001,
                    FrameDurationSupport::TenMs => 0b000010,
                    FrameDurationSupport::BothNoPreference => 0b000011,
                    FrameDurationSupport::PreferSevenFiveMs => 0b010011,
                    FrameDurationSupport::PreferTenMs => 0b100011,
                };
            }
            CodecCapability::SupportedMaxCodecFramesPerSdu(max) => {
                buf[0] = *max;
            }
            CodecCapability::SupportedOctetsPerCodecFrame { min, max } => {
                let min_bytes = min.to_le_bytes();
                buf[0..=1].copy_from_slice(&min_bytes);

                let max_bytes = max.to_le_bytes();
                buf[2..=3].copy_from_slice(&max_bytes);
            }
            CodecCapability::SupportedSamplingFrequencies(supported) => {
                let sup_bytes = supported
                    .iter()
                    .fold(0u16, |acc, freq| acc | (1 << freq.bitpos()))
                    .to_le_bytes();
                buf[0..=1].copy_from_slice(&sup_bytes)
            }
        };
        Ok(())
    }

    fn decode_value(
        ty: &CodecCapabilityType,
        buf: &[u8],
    ) -> Result<Self, crate::packet_encoding::Error> {
        match ty {
            CodecCapabilityType::SupportedAudioChannelCounts => {
                let mut supported = HashSet::new();
                for bitpos in 0..8 {
                    if (buf[0] & (1 << bitpos)) != 0 {
                        supported.insert(bitpos + 1);
                    }
                }
                Ok(Self::SupportedAudioChannelCounts(supported))
            }
            CodecCapabilityType::SupportedFrameDurations => {
                let support = match buf[0] & 0b110011 {
                    0b000001 => FrameDurationSupport::SevenFiveMs,
                    0b000010 => FrameDurationSupport::TenMs,
                    0b000011 => FrameDurationSupport::BothNoPreference,
                    0b010011 => FrameDurationSupport::PreferSevenFiveMs,
                    0b100011 => FrameDurationSupport::PreferTenMs,
                    _ => {
                        return Err(crate::packet_encoding::Error::InvalidParameter(format!(
                            "Unrecognized bit pattern: {:#b}",
                            buf[0]
                        )));
                    }
                };
                Ok(Self::SupportedFrameDurations(support))
            }
            CodecCapabilityType::SupportedMaxCodecFramesPerSdu => {
                Ok(Self::SupportedMaxCodecFramesPerSdu(buf[0]))
            }
            CodecCapabilityType::SupportedOctetsPerCodecFrame => {
                let min = u16::from_le_bytes([buf[0], buf[1]]);
                let max = u16::from_le_bytes([buf[2], buf[3]]);
                Ok(Self::SupportedOctetsPerCodecFrame { min, max })
            }
            CodecCapabilityType::SupportedSamplingFrequencies => {
                let bitflags = u16::from_le_bytes([buf[0], buf[1]]);
                let mut supported = HashSet::new();
                for bitpos in 0..13 {
                    if bitflags & (1 << bitpos) != 0 {
                        let Some(f) = SamplingFrequency::from_bitpos(bitpos) else {
                            continue;
                        };
                        let _ = supported.insert(f);
                    }
                }
                Ok(Self::SupportedSamplingFrequencies(supported))
            }
        }
    }
}

#[cfg(test)]
mod tests {}
