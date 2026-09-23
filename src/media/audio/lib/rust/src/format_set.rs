// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::format::{Format, SampleSize, SampleType};
use flex_fuchsia_audio_device as fadevice;
use flex_fuchsia_hardware_audio as fhaudio;
use itertools::iproduct;
use std::num::NonZeroU32;

#[derive(Default, Debug, Clone, PartialEq)]
pub struct PcmFormatSet {
    pub channel_sets: Vec<ChannelSet>,
    pub sample_types: Vec<SampleType>,
    pub frame_rates: Vec<u32>,
}

impl PcmFormatSet {
    pub fn supports(&self, format: &Format) -> bool {
        let channels_ok = self
            .channel_sets
            .iter()
            .any(|channel_set| channel_set.channels() as u32 == format.channels);
        let sample_type_ok = self.sample_types.contains(&format.sample_type);
        let frame_rate_ok = self.frame_rates.contains(&format.frames_per_second);
        channels_ok && sample_type_ok && frame_rate_ok
    }
}

impl TryFrom<fhaudio::PcmSupportedFormats> for PcmFormatSet {
    type Error = String;

    fn try_from(value: fhaudio::PcmSupportedFormats) -> Result<Self, Self::Error> {
        let channel_sets = value.channel_sets.ok_or_else(|| "missing channel_sets".to_string())?;
        let sample_formats =
            value.sample_formats.ok_or_else(|| "missing sample_formats".to_string())?;
        let bytes_per_sample =
            value.bytes_per_sample.ok_or_else(|| "missing bytes_per_sample".to_string())?;
        let valid_bits_per_sample = value
            .valid_bits_per_sample
            .ok_or_else(|| "missing valid_bits_per_sample".to_string())?;
        let frame_rates = value.frame_rates.ok_or_else(|| "missing frame_rates".to_string())?;

        let channel_sets: Vec<ChannelSet> =
            channel_sets.into_iter().map(ChannelSet::try_from).collect::<Result<Vec<_>, _>>()?;

        // Convert each combination of values from `bytes_per_sample` and `valid_bits_per_sample`
        // into a [SampleSize], ignoring any invalid combinations, e.g. when:
        //     * A value is zero, so [NonZeroU32::new] returns `None`
        //     * Valid bits per sample is greater than bytes (total bits) per sample, so
        //       [SampleSize::from_partial_bits] returns `None`.
        let sample_sizes = iproduct!(bytes_per_sample.iter(), valid_bits_per_sample.iter())
            .filter_map(|(bytes, valid_bits)| {
                let total_bits = NonZeroU32::new(*bytes as u32 * 8)?;
                let valid_bits = NonZeroU32::new(*valid_bits as u32)?;
                SampleSize::from_partial_bits(valid_bits, total_bits)
            });
        // Convert each combination of sample format and [SampleSize] into a [SampleType].
        let sample_types: Vec<SampleType> = iproduct!(sample_formats, sample_sizes)
            .map(SampleType::try_from)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self { channel_sets, sample_types, frame_rates })
    }
}

impl TryFrom<fhaudio::SupportedFormats> for PcmFormatSet {
    type Error = String;

    fn try_from(value: fhaudio::SupportedFormats) -> Result<Self, Self::Error> {
        let pcm_supported_formats = value
            .pcm_supported_formats
            .ok_or_else(|| "missing pcm_supported_formats".to_string())?;
        Self::try_from(pcm_supported_formats)
    }
}

impl From<PcmFormatSet> for fhaudio::PcmSupportedFormats {
    fn from(value: PcmFormatSet) -> Self {
        let channel_sets = value.channel_sets.into_iter().map(Into::into).collect();
        let sample_formats = value.sample_types.iter().copied().map(Into::into).collect();

        let sample_sizes: Vec<SampleSize> =
            value.sample_types.iter().map(|sample_type| sample_type.size()).collect();

        let mut bytes_per_sample: Vec<u8> =
            sample_sizes.iter().map(|sample_size| sample_size.total_bytes().get() as u8).collect();
        bytes_per_sample.sort();

        let mut valid_bits_per_sample: Vec<u8> =
            sample_sizes.iter().map(|sample_size| sample_size.valid_bits().get() as u8).collect();
        valid_bits_per_sample.sort();

        let mut frame_rates = value.frame_rates;
        frame_rates.sort();

        Self {
            channel_sets: Some(channel_sets),
            sample_formats: Some(sample_formats),
            bytes_per_sample: Some(bytes_per_sample),
            valid_bits_per_sample: Some(valid_bits_per_sample),
            frame_rates: Some(frame_rates),
            ..Default::default()
        }
    }
}

impl From<PcmFormatSet> for fhaudio::SupportedFormats {
    fn from(value: PcmFormatSet) -> Self {
        Self { pcm_supported_formats: Some(value.into()), ..Default::default() }
    }
}

impl TryFrom<fadevice::PcmFormatSet> for PcmFormatSet {
    type Error = String;

    fn try_from(value: fadevice::PcmFormatSet) -> Result<Self, Self::Error> {
        let channel_sets = value.channel_sets.ok_or_else(|| "missing channel_sets".to_string())?;
        let sample_types = value.sample_types.ok_or_else(|| "missing sample_types".to_string())?;
        let frame_rates = value.frame_rates.ok_or_else(|| "missing frame_rates".to_string())?;

        let channel_sets: Vec<ChannelSet> =
            channel_sets.into_iter().map(ChannelSet::try_from).collect::<Result<Vec<_>, _>>()?;
        let sample_types: Vec<SampleType> =
            sample_types.into_iter().map(SampleType::try_from).collect::<Result<Vec<_>, _>>()?;

        Ok(Self { channel_sets, sample_types, frame_rates })
    }
}

#[derive(Default, Debug, Clone, PartialEq)]
pub struct ChannelAttributes {
    /// Minimum frequency, in hertz.
    pub min_frequency: Option<u32>,
    /// Maximum frequency, in hertz.
    pub max_frequency: Option<u32>,
}

impl From<fadevice::ChannelAttributes> for ChannelAttributes {
    fn from(value: fadevice::ChannelAttributes) -> Self {
        Self { min_frequency: value.min_frequency, max_frequency: value.max_frequency }
    }
}

impl From<ChannelAttributes> for fadevice::ChannelAttributes {
    fn from(value: ChannelAttributes) -> Self {
        fadevice::ChannelAttributes {
            min_frequency: value.min_frequency,
            max_frequency: value.max_frequency,
            ..Default::default()
        }
    }
}

impl From<fhaudio::ChannelAttributes> for ChannelAttributes {
    fn from(value: fhaudio::ChannelAttributes) -> Self {
        Self { min_frequency: value.min_frequency, max_frequency: value.max_frequency }
    }
}

impl From<ChannelAttributes> for fhaudio::ChannelAttributes {
    fn from(value: ChannelAttributes) -> Self {
        Self {
            min_frequency: value.min_frequency,
            max_frequency: value.max_frequency,
            ..Default::default()
        }
    }
}

#[derive(Default, Debug, Clone, PartialEq)]
pub struct ChannelSet {
    pub attributes: Vec<ChannelAttributes>,
}

impl ChannelSet {
    /// Returns the number of channels supported by this ChannelSet.
    pub fn channels(&self) -> usize {
        self.attributes.len()
    }
}

impl TryFrom<Vec<ChannelAttributes>> for ChannelSet {
    type Error = String;

    fn try_from(value: Vec<ChannelAttributes>) -> Result<Self, Self::Error> {
        if value.is_empty() {
            return Err("channel attributes must contain at least one entry".to_string());
        }
        Ok(Self { attributes: value })
    }
}

impl TryFrom<fadevice::ChannelSet> for ChannelSet {
    type Error = String;

    fn try_from(value: fadevice::ChannelSet) -> Result<Self, Self::Error> {
        let attributes: Vec<ChannelAttributes> = value
            .attributes
            .ok_or_else(|| "missing attributes".to_string())?
            .into_iter()
            .map(Into::into)
            .collect();
        Self::try_from(attributes)
    }
}

impl From<ChannelSet> for fadevice::ChannelSet {
    fn from(value: ChannelSet) -> Self {
        Self {
            attributes: Some(value.attributes.into_iter().map(Into::into).collect()),
            ..Default::default()
        }
    }
}

impl TryFrom<fhaudio::ChannelSet> for ChannelSet {
    type Error = String;

    fn try_from(value: fhaudio::ChannelSet) -> Result<Self, Self::Error> {
        let attributes: Vec<ChannelAttributes> = value
            .attributes
            .ok_or_else(|| "missing attributes".to_string())?
            .into_iter()
            .map(Into::into)
            .collect();
        Self::try_from(attributes)
    }
}

impl From<ChannelSet> for fhaudio::ChannelSet {
    fn from(value: ChannelSet) -> Self {
        Self {
            attributes: Some(value.attributes.into_iter().map(Into::into).collect()),
            ..Default::default()
        }
    }
}

#[cfg(test)]
pub mod test {
    use super::*;
    use test_case::test_case;

    #[test_case(
        PcmFormatSet {
            channel_sets: vec![ChannelSet::try_from(vec![
                ChannelAttributes::default(),
                ChannelAttributes::default(),
            ])
            .unwrap()],
            sample_types: vec![SampleType::Uint8],
            frame_rates: vec![48000],
        };
        "exact"
    )]
    #[test_case(
        PcmFormatSet {
            channel_sets: vec![
                ChannelSet::try_from(vec![
                    ChannelAttributes::default(),
                ]).unwrap(),
                ChannelSet::try_from(vec![
                    ChannelAttributes::default(),
                    ChannelAttributes::default(),
                ]).unwrap(),
            ],
            sample_types: vec![SampleType::Uint8, SampleType::Int16],
            frame_rates: vec![16000, 22050, 32000, 44100, 48000, 88200, 96000],
        };
        "multiple"
    )]
    fn test_pcm_format_set_supports(format_set: PcmFormatSet) {
        let format =
            Format { frames_per_second: 48000, sample_type: SampleType::Uint8, channels: 2 };
        assert!(format_set.supports(&format));
    }

    #[test_case(PcmFormatSet::default(); "empty set")]
    #[test_case(
        PcmFormatSet {
            // No channel set with two channels
            channel_sets: vec![
                ChannelSet::try_from(vec![
                    ChannelAttributes::default(),
                ]).unwrap(),
            ],
            sample_types: vec![SampleType::Uint8, SampleType::Int16],
            frame_rates: vec![16000, 22050, 32000, 44100, 48000, 88200, 96000],
        };
        "missing channel set"
    )]
    #[test_case(
        PcmFormatSet {
            channel_sets: vec![
                ChannelSet::try_from(vec![
                    ChannelAttributes::default(),
                ]).unwrap(),
                ChannelSet::try_from(vec![
                    ChannelAttributes::default(),
                    ChannelAttributes::default(),
                ]).unwrap(),
            ],
            // No SampleType:Uint8
            sample_types: vec![SampleType::Int16],
            frame_rates: vec![16000, 22050, 32000, 44100, 48000, 88200, 96000],
        };
        "missing sample type"
    )]
    #[test_case(
        PcmFormatSet {
            channel_sets: vec![
                ChannelSet::try_from(vec![
                    ChannelAttributes::default(),
                ]).unwrap(),
                ChannelSet::try_from(vec![
                    ChannelAttributes::default(),
                    ChannelAttributes::default(),
                ]).unwrap(),
            ],
            sample_types: vec![SampleType::Uint8, SampleType::Int16],
            // No 48000
            frame_rates: vec![16000, 22050, 32000, 44100, 88200, 96000],
        };
        "missing frame rate"
    )]
    fn test_pcm_format_set_does_not_support(format_set: PcmFormatSet) {
        let format =
            Format { frames_per_second: 48000, sample_type: SampleType::Uint8, channels: 2 };
        assert!(!format_set.supports(&format));
    }

    #[test]
    fn test_pcm_format_set_from_hw_supported_formats() {
        let hw_supported_formats = fhaudio::SupportedFormats {
            pcm_supported_formats: Some(fhaudio::PcmSupportedFormats {
                channel_sets: Some(vec![
                    fhaudio::ChannelSet {
                        attributes: Some(vec![fhaudio::ChannelAttributes::default()]),
                        ..Default::default()
                    },
                    fhaudio::ChannelSet {
                        attributes: Some(vec![
                            fhaudio::ChannelAttributes::default(),
                            fhaudio::ChannelAttributes::default(),
                        ]),
                        ..Default::default()
                    },
                ]),
                sample_formats: Some(vec![fhaudio::SampleFormat::PcmSigned]),
                bytes_per_sample: Some(vec![2]),
                valid_bits_per_sample: Some(vec![16]),
                frame_rates: Some(vec![16000, 22050, 32000, 44100, 48000, 88200, 96000]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let format_set = PcmFormatSet {
            channel_sets: vec![
                ChannelSet::try_from(vec![ChannelAttributes::default()]).unwrap(),
                ChannelSet::try_from(vec![
                    ChannelAttributes::default(),
                    ChannelAttributes::default(),
                ])
                .unwrap(),
            ],
            sample_types: vec![SampleType::Int16],
            frame_rates: vec![16000, 22050, 32000, 44100, 48000, 88200, 96000],
        };
        assert_eq!(format_set, PcmFormatSet::try_from(hw_supported_formats).unwrap());
    }
}
