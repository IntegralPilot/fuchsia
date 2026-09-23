// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bitflags::bitflags;

/// See Assigned Numbers section 6.12.5
/// Codec_Specific_Configuration LTV structures.
#[derive(Default)]
pub struct CodecSpecificConfigLTV {
    pub sampling_frequency: Option<SamplingFrequency>,
    pub frame_duration: Option<FrameDuration>,
    pub audio_channel_alloc: Option<AudioLocation>,
    pub octets_per_codec_frame: Option<u16>,
    pub codec_frame_blocks_per_sdu: Option<u8>,
}

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct AudioLocation: u32 {
        const FRONT_LEFT = 0x00000001;
        const FRONT_RIGHT = 0x00000002;
        const FRONT_CENTER = 0x00000004;
        const LOW_FREQUENCY_EFFECTS_1 = 0x00000008;
        const BACK_LEFT = 0x00000010;
        const BACK_RIGHT = 0x00000020;
        const FRONT_LEFT_OF_CENTER = 0x00000040;
        const FRONT_RIGHT_OF_CENTER = 0x00000080;
        const BACK_CENTER = 0x00000100;
        const LOW_FREQUENCY_EFFECTS_2 = 0x00000200;
        const SIDE_LEFT = 0x00000400;
        const SIDE_RIGHT = 0x00000800;
        const TOP_FRONT_LEFT = 0x00001000;
        const TOP_FRONT_RIGHT = 0x00002000;
        const TOP_FRONT_CENTER = 0x00004000;
        const TOP_CENTER = 0x00008000;
        const TOP_BACK_LEFT = 0x00010000;
        const TOP_BACK_RIGHT = 0x00020000;
        const TOP_SIDE_LEFT = 0x00040000;
        const TOP_SIDE_RIGHT = 0x00080000;
        const TOP_BACK_CENTER = 0x00100000;
        const BOTTOM_FRONT_CENTER = 0x00200000;
        const BOTTOM_FRONT_LEFT = 0x00400000;
        const BOTTOM_FRONT_RIGHT = 0x00800000;
        const FRONT_LEFT_WIDE = 0x01000000;
        const FRONT_RIGHT_WIDE = 0x02000000;
        const LEFT_SURROUND = 0x04000000;
        const RIGHT_SURROUND = 0x08000000;
    }
}

#[derive(Clone, Copy, Debug)]
#[repr(u8)]
pub enum SamplingFrequency {
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

#[derive(Clone, Copy, Debug)]
#[repr(u8)]
pub enum FrameDuration {
    D7p5Ms = 0x00,
    D10Ms = 0x01,
}

impl CodecSpecificConfigLTV {
    fn to_bytes(&self, big_endian: bool) -> Vec<u8> {
        let mut bytes = Vec::new();
        if let Some(sf) = &self.sampling_frequency {
            bytes.extend_from_slice(&[0x02, 0x01, *sf as u8]);
        }
        if let Some(fd) = &self.frame_duration {
            bytes.extend_from_slice(&[0x02, 0x02, *fd as u8]);
        }
        if let Some(alloc) = &self.audio_channel_alloc {
            bytes.extend_from_slice(&[0x05, 0x03]);
            let alloc_bytes =
                if big_endian { alloc.bits().to_be_bytes() } else { alloc.bits().to_le_bytes() };
            bytes.extend_from_slice(&alloc_bytes);
        }
        if let Some(o) = &self.octets_per_codec_frame {
            bytes.extend_from_slice(&[0x03, 0x04]);
            let octet_bytes = if big_endian { o.to_be_bytes() } else { o.to_le_bytes() };
            bytes.extend_from_slice(&octet_bytes);
        }
        if let Some(fb) = &self.codec_frame_blocks_per_sdu {
            bytes.extend_from_slice(&[0x02, 0x05, *fb]);
        }
        return bytes;
    }

    pub fn to_le_bytes(&self) -> Vec<u8> {
        self.to_bytes(false)
    }

    pub fn to_be_bytes(&self) -> Vec<u8> {
        self.to_bytes(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_codec_specific_config_ltv() {
        let config = CodecSpecificConfigLTV {
            sampling_frequency: Some(SamplingFrequency::F32000Hz),
            frame_duration: Some(FrameDuration::D7p5Ms),
            audio_channel_alloc: Some(AudioLocation::FRONT_LEFT | AudioLocation::FRONT_RIGHT),
            octets_per_codec_frame: Some(58),
            codec_frame_blocks_per_sdu: Some(40),
        };
        assert_eq!(
            config.to_le_bytes(),
            vec![
                0x02, 0x01, 0x06, // 32kHz sampling freq.
                0x02, 0x02, 0x00, // 7.5ms frame duration.
                0x05, 0x03, 0x03, 0x00, 0x00, 0x00, // LF and RF.
                0x03, 0x04, 0x3A, 0x00, // 58 octets per codec frame.
                0x02, 0x05, 0x28,
            ]
        );
        assert_eq!(
            config.to_be_bytes(),
            vec![
                0x02, 0x01, 0x06, // 32kHz sampling freq.
                0x02, 0x02, 0x00, // 7.5ms frame duration.
                0x05, 0x03, 0x00, 0x00, 0x00, 0x03, // LF and RF.
                0x03, 0x04, 0x00, 0x3A, // 58 octets per codec frame.
                0x02, 0x05, 0x28, // 40 frame block.
            ]
        );

        let config = CodecSpecificConfigLTV {
            sampling_frequency: Some(SamplingFrequency::F32000Hz),
            frame_duration: Some(FrameDuration::D10Ms),
            audio_channel_alloc: None,
            octets_per_codec_frame: Some(40),
            codec_frame_blocks_per_sdu: None,
        };
        assert_eq!(
            config.to_le_bytes(),
            vec![
                0x02, 0x01, 0x06, // 32kHz sampling freq.
                0x02, 0x02, 0x01, // 10ms frame duration.
                0x03, 0x04, 0x28, 0x00, // 40 octets per codec frame.
            ]
        );
        assert_eq!(
            config.to_be_bytes(),
            vec![
                0x02, 0x01, 0x06, // 32kHz sampling freq.
                0x02, 0x02, 0x01, // 10ms frame duration.
                0x03, 0x04, 0x00, 0x28, // 40 octets per codec frame.
            ]
        );
    }
}
