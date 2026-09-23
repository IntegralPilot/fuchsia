// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bt_common::core::ltv::LtValue;
use bt_common::core::CodecId;
use bt_common::generic_audio::codec_configuration::CodecConfiguration;
use bt_common::generic_audio::metadata_ltv::Metadata;
use bt_common::packet_encoding::{Decodable, Encodable, Error as PacketError};

/// Broadcast_ID is a 3-byte data on the wire.
/// Defined in BAP spec v1.0.1 section 3.7.2.1.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct BroadcastId(u32);

impl BroadcastId {
    // On the wire, Broadcast_ID is transported in 3 bytes.
    pub const BYTE_SIZE: usize = 3;

    pub fn new(raw_value: u32) -> Self {
        Self(raw_value)
    }
}

impl std::fmt::Display for BroadcastId {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{:08x}", self.0)
    }
}

impl std::fmt::Debug for BroadcastId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("BroadcastId").field(&format_args!("0x{}", self)).finish()
    }
}

impl From<BroadcastId> for u32 {
    fn from(value: BroadcastId) -> u32 {
        value.0
    }
}

impl TryFrom<u32> for BroadcastId {
    type Error = PacketError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        const MAX_VALUE: u32 = 0xFFFFFF;
        if value > MAX_VALUE {
            return Err(PacketError::InvalidParameter(format!(
                "Broadcast ID cannot exceed 3 bytes"
            )));
        }
        Ok(BroadcastId(value))
    }
}

impl Decodable for BroadcastId {
    type Error = PacketError;

    fn decode(buf: &[u8]) -> (core::result::Result<Self, Self::Error>, usize) {
        if buf.len() != Self::BYTE_SIZE {
            return (Err(PacketError::UnexpectedDataLength), buf.len());
        }

        let padded_bytes = [buf[0], buf[1], buf[2], 0x00];
        (Ok(BroadcastId(u32::from_le_bytes(padded_bytes))), Self::BYTE_SIZE)
    }
}

impl Encodable for BroadcastId {
    type Error = PacketError;

    fn encode(&self, buf: &mut [u8]) -> core::result::Result<(), Self::Error> {
        if buf.len() < self.encoded_len() {
            return Err(PacketError::BufferTooSmall);
        }
        // Since 3-byte value is being fit into u32, we ignore the most significant
        // byte.
        buf[0..3].copy_from_slice(&self.0.to_le_bytes()[0..3]);
        Ok(())
    }

    fn encoded_len(&self) -> core::primitive::usize {
        Self::BYTE_SIZE
    }
}

/// To associate a PA, used to expose broadcast Audio Stream parameters, with a
/// broadcast Audio Stream, the Broadcast Source shall transmit EA PDUs that
/// include the following data. This struct represents the AD data value
/// excluding the 2-octet Service UUID. See BAP v1.0.1 Section 3.7.2.1 for more
/// details.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BroadcastAudioAnnouncement {
    pub broadcast_id: BroadcastId,
}

impl BroadcastAudioAnnouncement {
    const PACKET_SIZE: usize = BroadcastId::BYTE_SIZE;
}

impl Decodable for BroadcastAudioAnnouncement {
    type Error = PacketError;

    fn decode(buf: &[u8]) -> (core::result::Result<Self, Self::Error>, usize) {
        if buf.len() < Self::PACKET_SIZE {
            return (Err(PacketError::UnexpectedDataLength), buf.len());
        }

        match BroadcastId::decode(&buf[0..3]) {
            (Ok(broadcast_id), _) => {
                // According to the spec, broadcast audio announcement service data inlcudes
                // broadcast id and any additional service data. We don't store any
                // additional parameters, so for now we just "consume" all of data buffer
                // without doing anything.
                (Ok(Self { broadcast_id }), buf.len())
            }
            (Err(e), _) => (Err(e), buf.len()),
        }
    }
}

/// Parameters exposed as part of Basic Audio Announcement from Broadcast
/// Sources. See BAP v1.0.1 Section 3.7.2.2 for more details.
// TODO(b/308481381): Fill out the struct.
#[derive(Clone, Debug, PartialEq)]
pub struct BroadcastAudioSourceEndpoint {
    // Actual value is 3 bytes long.
    pub presentation_delay_ms: u32,
    pub big: Vec<BroadcastIsochronousGroup>,
}

impl BroadcastAudioSourceEndpoint {
    // Should contain presentation delay, num BIG, and at least one BIG praram.
    const MIN_PACKET_SIZE: usize = 3 + 1 + BroadcastIsochronousGroup::MIN_PACKET_SIZE;
}

impl Decodable for BroadcastAudioSourceEndpoint {
    type Error = PacketError;

    fn decode(buf: &[u8]) -> (core::result::Result<Self, Self::Error>, usize) {
        if buf.len() < Self::MIN_PACKET_SIZE {
            return (Err(PacketError::UnexpectedDataLength), buf.len());
        }

        let mut idx = 0 as usize;
        let presentation_delay = u32::from_le_bytes([buf[idx], buf[idx + 1], buf[idx + 2], 0x00]);
        idx += 3;

        let num_big: usize = buf[idx] as usize;
        idx += 1;
        if num_big < 1 {
            return (
                Err(PacketError::InvalidParameter(format!(
                    "num of subgroups shall be at least 1 got {num_big}"
                ))),
                buf.len(),
            );
        }
        let mut big = Vec::new();
        while big.len() < num_big {
            match BroadcastIsochronousGroup::decode(&buf[idx..]) {
                (Ok(group), consumed) => {
                    big.push(group);
                    idx += consumed;
                }
                (Err(e), _) => {
                    return (Err(PacketError::InvalidParameter(e.to_string())), buf.len());
                }
            };
        }

        (Ok(Self { presentation_delay_ms: presentation_delay, big }), idx)
    }
}

/// A single subgroup in a Basic Audio Announcement as outlined in
/// BAP spec v1.0.1 Section 3.7.2.2. Each subgroup is used to
/// group BISes present in the broadcast isochronous group.
#[derive(Clone, Debug, PartialEq)]
pub struct BroadcastIsochronousGroup {
    pub codec_id: CodecId,
    pub codec_specific_configs: Vec<CodecConfiguration>,
    pub metadata: Vec<Metadata>,
    pub bis: Vec<BroadcastIsochronousStream>,
}

impl BroadcastIsochronousGroup {
    // Should contain num BIS, codec id, codec specific config len, metadata len,
    // and at least one BIS praram.
    const MIN_PACKET_SIZE: usize =
        1 + CodecId::BYTE_SIZE + 1 + 1 + BroadcastIsochronousStream::MIN_PACKET_SIZE;
}

impl Decodable for BroadcastIsochronousGroup {
    type Error = PacketError;

    fn decode(buf: &[u8]) -> (core::result::Result<Self, Self::Error>, usize) {
        if buf.len() < BroadcastIsochronousGroup::MIN_PACKET_SIZE {
            return (Err(PacketError::UnexpectedDataLength), buf.len());
        }

        let mut idx = 0;
        let num_bis = buf[idx] as usize;
        idx += 1;
        if num_bis < 1 {
            return (
                Err(PacketError::InvalidParameter(format!(
                    "num of BIS shall be at least 1 got {num_bis}"
                ))),
                buf.len(),
            );
        }

        let mut decode_fn = || {
            let codec_id;
            match CodecId::decode(&buf[idx..]) {
                (Ok(id), consumed) => {
                    codec_id = id;
                    idx += consumed;
                }
                (Err(e), _) => {
                    return Err(e);
                }
            };

            let codec_config_len = buf[idx] as usize;
            idx += 1;
            if idx + codec_config_len > buf.len() {
                return Err(bt_common::packet_encoding::Error::UnexpectedDataLength);
            }
            let (results, consumed) =
                CodecConfiguration::decode_all(&buf[idx..idx + codec_config_len]);
            if consumed != codec_config_len {
                return Err(bt_common::packet_encoding::Error::UnexpectedDataLength);
            }

            let codec_specific_configs = results.into_iter().filter_map(Result::ok).collect();
            idx += codec_config_len;

            let metadata_len = buf[idx] as usize;
            idx += 1;
            if idx + metadata_len > buf.len() {
                return Err(bt_common::packet_encoding::Error::UnexpectedDataLength);
            }

            let (results_metadata, consumed_len) =
                Metadata::decode_all(&buf[idx..idx + metadata_len]);
            if consumed_len != metadata_len {
                return Err(PacketError::UnexpectedDataLength);
            }
            // Ignore any undecodable metadata types
            let metadata = results_metadata.into_iter().filter_map(Result::ok).collect();
            idx += consumed_len;

            let mut bis = Vec::new();
            while bis.len() < num_bis {
                match BroadcastIsochronousStream::decode(&buf[idx..]) {
                    (Ok(stream), consumed) => {
                        bis.push(stream);
                        idx += consumed;
                    }
                    (Err(e), _consumed) => {
                        return Err(PacketError::InvalidParameter(e.to_string()));
                    }
                }
            }

            Ok((BroadcastIsochronousGroup { codec_id, codec_specific_configs, metadata, bis }, idx))
        };
        match decode_fn() {
            Ok((obj, consumed)) => (Ok(obj), consumed),
            Err(e) => (Err(e), buf.len()),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BroadcastIsochronousStream {
    pub bis_index: u8,
    pub codec_specific_config: Vec<CodecConfiguration>,
}

impl BroadcastIsochronousStream {
    const MIN_PACKET_SIZE: usize = 1 + 1;
}

impl Decodable for BroadcastIsochronousStream {
    type Error = PacketError;

    fn decode(buf: &[u8]) -> (core::result::Result<Self, Self::Error>, usize) {
        if buf.len() < BroadcastIsochronousStream::MIN_PACKET_SIZE {
            return (Err(PacketError::UnexpectedDataLength), buf.len());
        }

        let mut idx = 0;

        let bis_index = buf[idx];
        idx += 1;

        let codec_config_len = buf[idx] as usize;
        idx += 1;

        let (results, consumed) = CodecConfiguration::decode_all(&buf[idx..idx + codec_config_len]);
        if consumed != codec_config_len {
            return (Err(bt_common::packet_encoding::Error::UnexpectedDataLength), buf.len());
        }
        let codec_specific_configs = results.into_iter().filter_map(Result::ok).collect();
        idx += codec_config_len;

        (
            Ok(BroadcastIsochronousStream {
                bis_index,
                codec_specific_config: codec_specific_configs,
            }),
            idx,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashSet;

    use bt_common::generic_audio::codec_configuration::{FrameDuration, SamplingFrequency};
    use bt_common::generic_audio::AudioLocation;

    #[test]
    fn broadcast_id() {
        // Value bigger than 3 bytes is not a valid broadcast ID.
        let _ = BroadcastId::try_from(0x010A0B0C).expect_err("should fail");

        let id = BroadcastId::try_from(0x000A0B0C).expect("should succeed");

        assert_eq!(id.encoded_len(), 3);
        let mut buf = vec![0; id.encoded_len()];
        let _ = id.encode(&mut buf[..]).expect("should have succeeded");

        let bytes = vec![0x0C, 0x0B, 0x0A];
        assert_eq!(buf, bytes);

        let (got, bytes) = BroadcastId::decode(&bytes);
        assert_eq!(got, Ok(id));
        assert_eq!(bytes, BroadcastId::BYTE_SIZE);
        let got = BroadcastId::try_from(u32::from_le_bytes([0x0C, 0x0B, 0x0A, 0x00]))
            .expect("should succeed");
        assert_eq!(got, id);
    }

    #[test]
    fn broadcast_audio_announcement() {
        let bytes = vec![0x0C, 0x0B, 0x0A];
        let broadcast_id = BroadcastId::try_from(0x000A0B0C).unwrap();

        let (got, consumed) = BroadcastAudioAnnouncement::decode(&bytes);
        assert_eq!(got, Ok(BroadcastAudioAnnouncement { broadcast_id }));
        assert_eq!(consumed, 3);

        let bytes = vec![
            0x0C, 0x0B, 0x0A, 0x01, 0x02, 0x03, 0x04, 0x05, /* some other additional data */
        ];
        let (got, consumed) = BroadcastAudioAnnouncement::decode(&bytes);
        assert_eq!(got, Ok(BroadcastAudioAnnouncement { broadcast_id }));
        assert_eq!(consumed, 8);
    }

    #[test]
    fn decode_bis() {
        #[rustfmt::skip]
        let buf = [
            0x01, 0x09,                          // bis index and codec specific config len
            0x02, 0x01, 0x06,                    // sampling frequency LTV
            0x05, 0x03, 0x03, 0x00, 0x00, 0x0C,  // audio location LTV
        ];

        let (bis, _read_bytes) = BroadcastIsochronousStream::decode(&buf[..]);
        assert_eq!(
            bis,
            Ok(BroadcastIsochronousStream {
                bis_index: 0x01,
                codec_specific_config: vec![
                    CodecConfiguration::SamplingFrequency(SamplingFrequency::F32000Hz),
                    CodecConfiguration::AudioChannelAllocation(HashSet::from([
                        AudioLocation::FrontLeft,
                        AudioLocation::FrontRight,
                        AudioLocation::LeftSurround,
                        AudioLocation::RightSurround
                    ])),
                ],
            })
        );
    }

    #[test]
    fn decode_big() {
        #[rustfmt::skip]
        let buf = [
            0x02,  0x03, 0x00, 0x00, 0x00, 0x00,  // num of bis, codec id
            0x04, 0x03, 0x04, 0x04, 0x10,         // codec specific config len, octets per codec frame LTV
            0x03, 0x02, 0x08, 0x01,               // metadata len, audio active state LTV
            0x01, 0x00,                           // bis index, codec specific config len (bis #1)
            0x02, 0x03, 0x02, 0x02, 0x01,         // bis index, codec specific config len, frame duration LTV (bis #2)
        ];

        let big = BroadcastIsochronousGroup::decode(&buf[..]).0.expect("should not fail");
        assert_eq!(
            big,
            BroadcastIsochronousGroup {
                codec_id: CodecId::Assigned(bt_common::core::CodingFormat::Transparent),
                codec_specific_configs: vec![CodecConfiguration::OctetsPerCodecFrame(0x1004),],
                metadata: vec![Metadata::AudioActiveState(true)],
                bis: vec![
                    BroadcastIsochronousStream { bis_index: 0x01, codec_specific_config: vec![] },
                    BroadcastIsochronousStream {
                        bis_index: 0x02,
                        codec_specific_config: vec![CodecConfiguration::FrameDuration(
                            FrameDuration::TenMs
                        )],
                    },
                ],
            }
        );
    }

    #[test]
    fn decode_base() {
        #[rustfmt::skip]
        let buf = [
            0x10, 0x20, 0x30, 0x02,               // presentation delay, num of subgroups
            0x01, 0x03, 0x00, 0x00, 0x00, 0x00,   // num of bis, codec id (big #1)
            0x00,                                 // codec specific config len
            0x00,                                 // metadata len,
            0x01, 0x00,                           // bis index, codec specific config len (big #1 / bis #1)
            0x01, 0x02, 0x00, 0x00, 0x00, 0x00,   // num of bis, codec id (big #2)
            0x00,                                 // codec specific config len
            0x00,                                 // metadata len,
            0x01, 0x03, 0x02, 0x05, 0x08,         // bis index, codec specific config len, codec frame blocks LTV (big #2 / bis #2)
        ];

        let base = BroadcastAudioSourceEndpoint::decode(&buf[..]).0.expect("should not fail");
        assert_eq!(base.presentation_delay_ms, 0x00302010);
        assert_eq!(base.big.len(), 2);
        assert_eq!(
            base.big[0],
            BroadcastIsochronousGroup {
                codec_id: CodecId::Assigned(bt_common::core::CodingFormat::Transparent),
                codec_specific_configs: vec![],
                metadata: vec![],
                bis: vec![BroadcastIsochronousStream {
                    bis_index: 0x01,
                    codec_specific_config: vec![],
                },],
            }
        );
        assert_eq!(
            base.big[1],
            BroadcastIsochronousGroup {
                codec_id: CodecId::Assigned(bt_common::core::CodingFormat::Cvsd),
                codec_specific_configs: vec![],
                metadata: vec![],
                bis: vec![BroadcastIsochronousStream {
                    bis_index: 0x01,
                    codec_specific_config: vec![CodecConfiguration::CodecFramesPerSdu(0x08)],
                },],
            }
        );
    }

    #[test]
    fn decode_base_complex() {
        #[rustfmt::skip]
        let buf = [
            0x20, 0x4e, 0x00,  // presentation_delay_ms: 20000 (little-endian)
            0x01,  // # of subgroups
            0x02,  // # of BIS in group 1
            0x06, 0x00, 0x00, 0x00, 0x00,  // Codec ID (Lc3)
            0x10,  // codec_specific_configuration_length of group 1
            0x02, 0x01, 0x05,  // sampling frequency 24 kHz
            0x02, 0x02, 0x01,  // 10 ms frame duration
            0x05, 0x03, 0x03, 0x00, 0x00, 0x00,  // front left audio channel
            0x03, 0x04, 0x3c, 0x00,  // 60 octets per codec frame
            0x04,  // metadata_length of group 1
            0x03, 0x02, 0x04, 0x00,  // media streaming audio context
            0x01,  // BIS index of the 1st BIS in group 1
            0x06,  // codec_specific_configuration_length of the 1st BIS
            0x05, 0x03, 0x01, 0x00, 0x00, 0x00,  // front left audio channel
            0x02,  // BIS index of the 2nd BIS in group 1
            0x06,  // codec_specific_configuration_length of the 2nd BIS
            0x05, 0x03, 0x02, 0x00, 0x00, 0x00,  // front right audio channel
        ];
        let (base, read_bytes) = BroadcastAudioSourceEndpoint::decode(&buf[..]);
        let base = base.expect("should not fail");
        assert_eq!(read_bytes, buf.len());
        assert_eq!(base.presentation_delay_ms, 20000);
        assert_eq!(base.big.len(), 1);
        assert_eq!(base.big[0].bis.len(), 2);

        #[rustfmt::skip]
        let buf = [
            0x20, 0x4e, 0x00,  // presentation_delay_ms: 20000 (little-endian)
            0x01,  // # of subgroups
            0x01,  // # of BIS in group 1
            0x06, 0x00, 0x00, 0x00, 0x00,  // Codec ID (Lc3)
            0x10,  // codec_specific_configuration_length of group 1
            0x02, 0x01, 0x05,  // sampling frequency 24 kHz
            0x02, 0x02, 0x01,  // 10 ms frame duration
            0x05, 0x03, 0x01, 0x00, 0x00, 0x00,  // front left audio channel
            0x03, 0x04, 0x3c, 0x00,  // 60 octets per codec frame
            0x02,  // metadata_length of group 1
            0x01, 0x09,  // broadcast audio immediate rendering flag
            0x01,  // BIS index of the 1st BIS in group 1
            0x03,  // codec_specific_configuration_length of the 1st BIS
            0x02, 0x01, 0x05,  // sampling frequency 24 kHz
        ];
        let (base, read_bytes) = BroadcastAudioSourceEndpoint::decode(&buf[..]);
        let base = base.expect("should not fail");
        assert_eq!(read_bytes, buf.len());
        assert_eq!(base.presentation_delay_ms, 20000);
        assert_eq!(base.big.len(), 1);
        assert_eq!(base.big[0].bis.len(), 1);
    }
}
