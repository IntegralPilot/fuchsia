// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bt_common::core::ltv::LtValue;
use bt_common::core::CodecId;
use bt_common::generic_audio::codec_capabilities::CodecCapability;
use bt_common::generic_audio::metadata_ltv::Metadata;
use bt_common::generic_audio::{AudioLocation, ContextType};
use bt_common::packet_encoding::{Decodable, Encodable};
use bt_common::Uuid;
use bt_gatt::{client::FromCharacteristic, Characteristic};

use std::collections::HashSet;

pub mod debug;
pub mod server;

pub use server::types::AudioContexts;

/// UUID from Assigned Numbers section 3.4.
pub const PACS_UUID: Uuid = Uuid::from_u16(0x1850);

/// A Published Audio Capability (PAC) record.
/// Published Audio Capabilities represent the capabilities of a given peer to
/// transmit or receive Audio capabilities, exposed in PAC records, represent
/// the server audio capabilities independent of available resources at any
/// given time. Audio capabilities do not distinguish between unicast
/// Audio Streams or broadcast Audio Streams.
#[derive(Debug, Clone, PartialEq)]
pub struct PacRecord {
    pub codec_id: CodecId,
    pub codec_specific_capabilities: Vec<CodecCapability>,
    pub metadata: Vec<Metadata>,
}

impl Decodable for PacRecord {
    type Error = bt_common::packet_encoding::Error;

    fn decode(buf: &[u8]) -> (core::result::Result<Self, Self::Error>, usize) {
        let mut idx = 0;
        let codec_id = match CodecId::decode(&buf[idx..]) {
            (Ok(codec_id), consumed) => {
                idx += consumed;
                codec_id
            }
            (Err(e), _) => return (Err(e), buf.len()),
        };
        let codec_specific_capabilites_length = buf[idx] as usize;
        idx += 1;
        if idx + codec_specific_capabilites_length > buf.len() {
            return (Err(bt_common::packet_encoding::Error::UnexpectedDataLength), buf.len());
        }
        let (results, consumed) =
            CodecCapability::decode_all(&buf[idx..idx + codec_specific_capabilites_length]);
        if consumed != codec_specific_capabilites_length {
            return (Err(bt_common::packet_encoding::Error::UnexpectedDataLength), buf.len());
        }
        let codec_specific_capabilities = results.into_iter().filter_map(Result::ok).collect();
        idx += consumed;

        let metadata_length = buf[idx] as usize;
        idx += 1;
        if idx + metadata_length > buf.len() {
            return (Err(bt_common::packet_encoding::Error::UnexpectedDataLength), buf.len());
        }
        let (results, consumed) = Metadata::decode_all(&buf[idx..idx + metadata_length]);
        if consumed != metadata_length {
            return (Err(bt_common::packet_encoding::Error::UnexpectedDataLength), buf.len());
        }
        let metadata = results.into_iter().filter_map(Result::ok).collect();
        idx += consumed;

        (Ok(Self { codec_id, codec_specific_capabilities, metadata }), idx)
    }
}

impl Encodable for PacRecord {
    type Error = bt_common::packet_encoding::Error;

    fn encoded_len(&self) -> core::primitive::usize {
        5usize
            + self.codec_specific_capabilities.iter().fold(0, |a, x| a + x.encoded_len())
            + self.metadata.iter().fold(0, |a, x| a + x.encoded_len())
            + 2
    }

    fn encode(&self, buf: &mut [u8]) -> core::result::Result<(), Self::Error> {
        if buf.len() < self.encoded_len() {
            return Err(Self::Error::BufferTooSmall);
        }
        self.codec_id.encode(&mut buf[0..]).unwrap();
        let codec_capabilities_len =
            self.codec_specific_capabilities.iter().fold(0, |a, x| a + x.encoded_len());
        buf[5] = codec_capabilities_len as u8;
        LtValue::encode_all(self.codec_specific_capabilities.clone().into_iter(), &mut buf[6..])?;
        let metadata_len = self.metadata.iter().fold(0, |a, x| a + x.encoded_len());
        buf[6 + codec_capabilities_len] = metadata_len as u8;
        if metadata_len != 0 {
            LtValue::encode_all(
                self.metadata.clone().into_iter(),
                &mut buf[6 + codec_capabilities_len + 1..],
            )?;
        }
        Ok(())
    }
}

fn pac_records_into_char_value(records: &Vec<PacRecord>) -> Vec<u8> {
    let mut val = Vec::new();
    val.push(records.len() as u8);
    let mut idx = 1;
    for record in records {
        let record_len = record.encoded_len();
        val.resize(val.len() + record_len, 0);
        record.encode(&mut val[idx..]).unwrap();
        idx += record_len;
    }
    val
}

fn pac_records_from_bytes(
    value: &[u8],
) -> Result<Vec<PacRecord>, bt_common::packet_encoding::Error> {
    if value.len() < 1 {
        return Err(bt_common::packet_encoding::Error::UnexpectedDataLength);
    }
    let num_of_pac_records = value[0] as usize;
    let mut next_idx = 1;
    let mut capabilities = Vec::with_capacity(num_of_pac_records);
    for _ in 0..num_of_pac_records {
        let (cap, consumed) = PacRecord::decode(&value[next_idx..]);
        capabilities.push(cap?);
        next_idx += consumed;
    }
    Ok(capabilities)
}

/// One Sink Published Audio Capability Characteristic, or Sink PAC, exposed on
/// a service. More than one Sink PAC can exist on a given PACS service.  If
/// multiple are exposed, they are returned separately and can be notified by
/// the server separately.
#[derive(Debug, PartialEq, Clone)]
pub struct SinkPac {
    pub handle: bt_gatt::types::Handle,
    pub capabilities: Vec<PacRecord>,
}

impl FromCharacteristic for SinkPac {
    /// UUID from Assigned Numbers section 3.8.
    const UUID: Uuid = Uuid::from_u16(0x2BC9);

    fn from_chr(
        characteristic: Characteristic,
        value: &[u8],
    ) -> Result<Self, bt_common::packet_encoding::Error> {
        let handle = characteristic.handle;
        let capabilities = pac_records_from_bytes(value)?;
        Ok(Self { handle, capabilities })
    }

    fn update(&mut self, new_value: &[u8]) -> Result<&mut Self, bt_common::packet_encoding::Error> {
        self.capabilities = pac_records_from_bytes(new_value)?;
        Ok(self)
    }
}

/// One Sink Published Audio Capability Characteristic, or Sink PAC, exposed on
/// a service. More than one Sink PAC can exist on a given PACS service.  If
/// multiple are exposed, they are returned separately and can be notified by
/// the server separately.
#[derive(Debug, PartialEq, Clone)]
pub struct SourcePac {
    pub handle: bt_gatt::types::Handle,
    pub capabilities: Vec<PacRecord>,
}

impl FromCharacteristic for SourcePac {
    /// UUID from Assigned Numbers section 3.8.
    const UUID: Uuid = Uuid::from_u16(0x2BCB);

    fn from_chr(
        characteristic: Characteristic,
        value: &[u8],
    ) -> Result<Self, bt_common::packet_encoding::Error> {
        let handle = characteristic.handle;
        let capabilities = pac_records_from_bytes(value)?;
        Ok(Self { handle, capabilities })
    }

    fn update(&mut self, new_value: &[u8]) -> Result<&mut Self, bt_common::packet_encoding::Error> {
        self.capabilities = pac_records_from_bytes(new_value)?;
        Ok(self)
    }
}

#[derive(Debug, PartialEq, Clone, Default)]
pub struct AudioLocations {
    pub locations: HashSet<AudioLocation>,
}

impl Decodable for AudioLocations {
    type Error = bt_common::packet_encoding::Error;

    fn decode(buf: &[u8]) -> (core::result::Result<Self, Self::Error>, usize) {
        if buf.len() != 4 {
            return (Err(bt_common::packet_encoding::Error::UnexpectedDataLength), buf.len());
        }
        let locations =
            AudioLocation::from_bits(u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]))
                .collect();
        (Ok(AudioLocations { locations }), 4)
    }
}

impl Encodable for AudioLocations {
    type Error = bt_common::packet_encoding::Error;

    fn encoded_len(&self) -> core::primitive::usize {
        4
    }

    fn encode(&self, buf: &mut [u8]) -> core::result::Result<(), Self::Error> {
        if buf.len() < 4 {
            return Err(Self::Error::BufferTooSmall);
        }
        [buf[0], buf[1], buf[2], buf[3]] =
            AudioLocation::to_bits(self.locations.iter()).to_le_bytes();
        Ok(())
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct SourceAudioLocations {
    pub handle: bt_gatt::types::Handle,
    pub locations: AudioLocations,
}

impl SourceAudioLocations {
    fn into_char_value(&self) -> Vec<u8> {
        let mut val = Vec::with_capacity(self.locations.encoded_len());
        val.resize(self.locations.encoded_len(), 0);
        self.locations.encode(&mut val[..]).unwrap();
        val
    }
}

impl FromCharacteristic for SourceAudioLocations {
    /// UUID from Assigned Numbers section 3.8.
    const UUID: Uuid = Uuid::from_u16(0x2BCC);

    fn from_chr(
        characteristic: Characteristic,
        value: &[u8],
    ) -> Result<Self, bt_common::packet_encoding::Error> {
        let handle = characteristic.handle;
        let locations = AudioLocations::decode(value).0?;
        Ok(Self { handle, locations })
    }

    fn update(&mut self, new_value: &[u8]) -> Result<&mut Self, bt_common::packet_encoding::Error> {
        self.locations = AudioLocations::decode(new_value).0?;
        Ok(self)
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct SinkAudioLocations {
    pub handle: bt_gatt::types::Handle,
    pub locations: AudioLocations,
}

impl SinkAudioLocations {
    fn into_char_value(&self) -> Vec<u8> {
        let mut val = Vec::with_capacity(self.locations.encoded_len());
        val.resize(self.locations.encoded_len(), 0);
        self.locations.encode(&mut val[..]).unwrap();
        val
    }
}

impl FromCharacteristic for SinkAudioLocations {
    const UUID: Uuid = Uuid::from_u16(0x2BCA);

    fn from_chr(
        characteristic: Characteristic,
        value: &[u8],
    ) -> Result<Self, bt_common::packet_encoding::Error> {
        let handle = characteristic.handle;
        let locations = AudioLocations::decode(value).0?;
        Ok(Self { handle, locations })
    }

    fn update(&mut self, new_value: &[u8]) -> Result<&mut Self, bt_common::packet_encoding::Error> {
        self.locations = AudioLocations::decode(new_value).0?;
        Ok(self)
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum AvailableContexts {
    NotAvailable,
    Available(HashSet<ContextType>),
}

impl FromIterator<ContextType> for AvailableContexts {
    fn from_iter<I: IntoIterator<Item = ContextType>>(iter: I) -> Self {
        let types_: HashSet<_> = iter.into_iter().collect();
        if types_.len() == 0 {
            AvailableContexts::NotAvailable
        } else {
            AvailableContexts::Available(types_)
        }
    }
}

impl Decodable for AvailableContexts {
    type Error = bt_common::packet_encoding::Error;

    fn decode(buf: &[u8]) -> (core::result::Result<Self, Self::Error>, usize) {
        if buf.len() < 2 {
            return (Err(bt_common::packet_encoding::Error::UnexpectedDataLength), 2);
        }
        let encoded = u16::from_le_bytes([buf[0], buf[1]]);
        if encoded == 0 {
            (Ok(Self::NotAvailable), 2)
        } else {
            (Ok(Self::Available(ContextType::from_bits(encoded).collect())), 2)
        }
    }
}

impl Encodable for AvailableContexts {
    type Error = bt_common::packet_encoding::Error;

    fn encoded_len(&self) -> core::primitive::usize {
        2
    }

    fn encode(&self, buf: &mut [u8]) -> core::result::Result<(), Self::Error> {
        if buf.len() < 2 {
            return Err(Self::Error::BufferTooSmall);
        }
        match self {
            AvailableContexts::NotAvailable => [buf[0], buf[1]] = [0x00, 0x00],
            AvailableContexts::Available(set) => {
                [buf[0], buf[1]] = ContextType::to_bits(set.iter()).to_le_bytes()
            }
        }
        Ok(())
    }
}

impl From<&HashSet<ContextType>> for AvailableContexts {
    fn from(value: &HashSet<ContextType>) -> Self {
        if value.is_empty() {
            return AvailableContexts::NotAvailable;
        }
        AvailableContexts::Available(value.clone())
    }
}

#[derive(Debug, Clone)]
pub struct AvailableAudioContexts {
    pub handle: bt_gatt::types::Handle,
    pub sink: AvailableContexts,
    pub source: AvailableContexts,
}

impl AvailableAudioContexts {
    fn into_char_value(&self) -> Vec<u8> {
        let mut val = Vec::with_capacity(self.sink.encoded_len() + self.source.encoded_len());
        val.resize(self.sink.encoded_len() + self.source.encoded_len(), 0);
        self.sink.encode(&mut val[0..]).unwrap();
        self.source.encode(&mut val[2..]).unwrap();
        val
    }
}

impl FromCharacteristic for AvailableAudioContexts {
    /// UUID from Assigned Numbers section 3.8.
    const UUID: Uuid = Uuid::from_u16(0x2BCD);

    fn from_chr(
        characteristic: Characteristic,
        value: &[u8],
    ) -> core::result::Result<Self, bt_common::packet_encoding::Error> {
        let handle = characteristic.handle;
        if value.len() < 4 {
            return Err(bt_common::packet_encoding::Error::UnexpectedDataLength);
        }
        let sink = AvailableContexts::decode(&value[0..2]).0?;
        let source = AvailableContexts::decode(&value[2..4]).0?;
        Ok(Self { handle, sink, source })
    }

    fn update(
        &mut self,
        new_value: &[u8],
    ) -> core::result::Result<&mut Self, bt_common::packet_encoding::Error> {
        if new_value.len() != 4 {
            return Err(bt_common::packet_encoding::Error::UnexpectedDataLength);
        }
        let sink = AvailableContexts::decode(&new_value[0..2]).0?;
        let source = AvailableContexts::decode(&new_value[2..4]).0?;
        self.sink = sink;
        self.source = source;
        Ok(self)
    }
}

#[derive(Debug, Clone)]
pub struct SupportedAudioContexts {
    pub handle: bt_gatt::types::Handle,
    pub sink: HashSet<ContextType>,
    pub source: HashSet<ContextType>,
}

impl SupportedAudioContexts {
    fn into_char_value(&self) -> Vec<u8> {
        let mut val = Vec::with_capacity(self.encoded_len());
        val.resize(self.encoded_len(), 0);
        self.encode(&mut val[0..]).unwrap();
        val
    }
}

impl Encodable for SupportedAudioContexts {
    type Error = bt_common::packet_encoding::Error;

    fn encoded_len(&self) -> core::primitive::usize {
        4
    }

    fn encode(&self, buf: &mut [u8]) -> core::result::Result<(), Self::Error> {
        if buf.len() < 4 {
            return Err(Self::Error::BufferTooSmall);
        }
        [buf[0], buf[1]] = ContextType::to_bits(self.sink.iter()).to_le_bytes();
        [buf[2], buf[3]] = ContextType::to_bits(self.source.iter()).to_le_bytes();
        Ok(())
    }
}

impl FromCharacteristic for SupportedAudioContexts {
    const UUID: Uuid = Uuid::from_u16(0x2BCE);

    fn from_chr(
        characteristic: Characteristic,
        value: &[u8],
    ) -> core::result::Result<Self, bt_common::packet_encoding::Error> {
        let handle = characteristic.handle;
        if value.len() < 4 {
            return Err(bt_common::packet_encoding::Error::UnexpectedDataLength);
        }
        let sink = ContextType::from_bits(u16::from_le_bytes([value[0], value[1]])).collect();
        let source = ContextType::from_bits(u16::from_le_bytes([value[2], value[3]])).collect();
        Ok(Self { handle, sink, source })
    }

    fn update(
        &mut self,
        new_value: &[u8],
    ) -> core::result::Result<&mut Self, bt_common::packet_encoding::Error> {
        if new_value.len() < 4 {
            return Err(bt_common::packet_encoding::Error::UnexpectedDataLength);
        }
        self.sink =
            ContextType::from_bits(u16::from_le_bytes([new_value[0], new_value[1]])).collect();
        self.source =
            ContextType::from_bits(u16::from_le_bytes([new_value[2], new_value[3]])).collect();
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use bt_common::{
        generic_audio::codec_capabilities::{CodecCapabilityType, SamplingFrequency},
        Uuid,
    };
    use bt_gatt::{
        types::{AttributePermissions, Handle},
        Characteristic,
    };

    use pretty_assertions::assert_eq;

    const SINGLE_PAC_SIMPLE: [u8; 12] = [
        0x01, // Num of records; 1
        0x06, // CodecID: codec LC3,
        0x00, 0x00, 0x00, 0x00, // CodecID: company and specific id (zero)
        0x04, // Len of Codec capabilities
        0x03, 0x01, // lt: supported_sampling_frequencies
        0xC0, 0x02, // Supported: 44.1kHz, 48kHz, 96kHz
        0x00, // Len of metadata
    ];

    const MULTIPLE_PAC_COMPLEX: [u8; 29] = [
        0x02, // Num of records; 2
        0x05, // CodecID: codec MSBC,
        0x00, 0x00, 0x00, 0x00, // CodecID: company and specific id (zero)
        0x0A, // Len of Codec capabilities (10)
        0x03, 0x01, // lt: supported_sampling_frequencies
        0xC0, 0x02, // Supported: 44.1kHz, 48kHz, 96kHz
        0x05, 0x04, // lt: Octets per codec frame
        0x11, 0x00, // Minimum: 9
        0x00, 0x10, // Maximum: 4096
        0x04, // Len of metadata: 4
        0x03, 0x01, 0x03, 0x00, // PreferredAudioContexts
        0xFF, // CodecId: Vendor specific
        0xE0, 0x00, // Google
        0x01, 0x10, // ID 4097
        0x00, // Len of codec capabilities (none)
        0x00, // Len of metadata (none)
    ];

    #[track_caller]
    fn assert_has_frequencies(
        cap: &bt_common::generic_audio::codec_capabilities::CodecCapability,
        freqs: &[SamplingFrequency],
    ) {
        let CodecCapability::SupportedSamplingFrequencies(set) = cap else {
            unreachable!();
        };

        for freq in freqs {
            assert!(set.contains(freq));
        }
    }

    #[test]
    fn simple_sink_pac() {
        let pac = SinkPac::from_chr(
            Characteristic {
                handle: Handle(1),
                uuid: Uuid::from_u16(0x2BC9),
                properties: bt_gatt::types::CharacteristicProperties(vec![
                    bt_gatt::types::CharacteristicProperty::Read,
                ]),
                permissions: AttributePermissions::default(),
                descriptors: vec![],
            },
            &SINGLE_PAC_SIMPLE,
        )
        .expect("should decode correctly");

        assert_eq!(pac.capabilities.len(), 1);
        let cap = &pac.capabilities[0];
        assert_eq!(cap.codec_id, CodecId::Assigned(bt_common::core::CodingFormat::Lc3));
        let freq_cap = cap
            .codec_specific_capabilities
            .iter()
            .find(|c| c.into_type() == CodecCapabilityType::SupportedSamplingFrequencies)
            .unwrap();
        assert_has_frequencies(
            freq_cap,
            &[
                SamplingFrequency::F44100Hz,
                SamplingFrequency::F48000Hz,
                SamplingFrequency::F96000Hz,
            ],
        );
    }

    #[test]
    fn simple_source_pac() {
        let pac = SinkPac::from_chr(
            Characteristic {
                handle: Handle(1),
                uuid: Uuid::from_u16(0x2BC9),
                properties: bt_gatt::types::CharacteristicProperties(vec![
                    bt_gatt::types::CharacteristicProperty::Read,
                ]),
                permissions: AttributePermissions::default(),
                descriptors: vec![],
            },
            &SINGLE_PAC_SIMPLE,
        )
        .expect("should decode correctly");

        assert_eq!(pac.capabilities.len(), 1);
        let cap = &pac.capabilities[0];
        assert_eq!(cap.codec_id, CodecId::Assigned(bt_common::core::CodingFormat::Lc3));
        let freq_cap = cap
            .codec_specific_capabilities
            .iter()
            .find(|c| c.into_type() == CodecCapabilityType::SupportedSamplingFrequencies)
            .unwrap();
        assert_has_frequencies(
            freq_cap,
            &[
                SamplingFrequency::F44100Hz,
                SamplingFrequency::F48000Hz,
                SamplingFrequency::F96000Hz,
            ],
        );
    }

    #[test]
    fn complex_sink_pac() {
        let pac = SinkPac::from_chr(
            Characteristic {
                handle: Handle(1),
                uuid: Uuid::from_u16(0x2BC9),
                properties: bt_gatt::types::CharacteristicProperties(vec![
                    bt_gatt::types::CharacteristicProperty::Read,
                ]),
                permissions: AttributePermissions::default(),
                descriptors: vec![],
            },
            &MULTIPLE_PAC_COMPLEX,
        )
        .expect("should decode correctly");
        assert_eq!(pac.capabilities.len(), 2);
        let first = &pac.capabilities[0];
        assert_eq!(first.codec_id, CodecId::Assigned(bt_common::core::CodingFormat::Msbc));
        let freq_cap = first
            .codec_specific_capabilities
            .iter()
            .find(|c| c.into_type() == CodecCapabilityType::SupportedSamplingFrequencies)
            .unwrap();
        assert_has_frequencies(
            freq_cap,
            &[
                SamplingFrequency::F44100Hz,
                SamplingFrequency::F48000Hz,
                SamplingFrequency::F96000Hz,
            ],
        );

        let metadata = &first.metadata;

        assert_eq!(metadata.len(), 1);

        let Metadata::PreferredAudioContexts(p) = &metadata[0] else {
            panic!("expected PreferredAudioContexts, got {:?}", metadata[0]);
        };

        assert_eq!(p.len(), 2);

        let second = &pac.capabilities[1];
        assert_eq!(
            second.codec_id,
            CodecId::VendorSpecific {
                company_id: 0x00E0.into(),
                vendor_specific_codec_id: 0x1001_u16,
            }
        );
        assert_eq!(second.codec_specific_capabilities.len(), 0);
    }

    #[test]
    fn available_contexts_no_sink() {
        let available = AvailableAudioContexts::from_chr(
            Characteristic {
                handle: Handle(1),
                uuid: Uuid::from_u16(0x28CD),
                properties: bt_gatt::types::CharacteristicProperties(vec![
                    bt_gatt::types::CharacteristicProperty::Read,
                ]),
                permissions: AttributePermissions::default(),
                descriptors: vec![],
            },
            &[0x00, 0x00, 0x02, 0x04],
        )
        .expect("should decode correctly");
        assert_eq!(available.handle, Handle(1));
        assert_eq!(available.sink, AvailableContexts::NotAvailable);
        let AvailableContexts::Available(a) = available.source else {
            panic!("Source should be available");
        };
        assert_eq!(a, [ContextType::Conversational, ContextType::Alerts].into_iter().collect());
    }

    #[test]
    fn available_contexts_wrong_size() {
        let chr = Characteristic {
            handle: Handle(1),
            uuid: Uuid::from_u16(0x28CD),
            properties: bt_gatt::types::CharacteristicProperties(vec![
                bt_gatt::types::CharacteristicProperty::Read,
            ]),
            permissions: AttributePermissions::default(),
            descriptors: vec![],
        };
        let _ = AvailableAudioContexts::from_chr(chr.clone(), &[0x00, 0x00, 0x02])
            .expect_err("should not decode with too short");

        let available =
            AvailableAudioContexts::from_chr(chr.clone(), &[0x00, 0x00, 0x02, 0x04, 0xCA, 0xFE])
                .expect("should attempt to decode with too long");

        assert_eq!(available.sink, AvailableContexts::NotAvailable);
        let AvailableContexts::Available(a) = available.source else {
            panic!("Source should be available");
        };
        assert_eq!(a, [ContextType::Conversational, ContextType::Alerts].into_iter().collect());
    }

    #[test]
    fn supported_contexts() {
        let chr = Characteristic {
            handle: Handle(1),
            uuid: Uuid::from_u16(0x28CE),
            properties: bt_gatt::types::CharacteristicProperties(vec![
                bt_gatt::types::CharacteristicProperty::Read,
            ]),
            permissions: AttributePermissions::default(),
            descriptors: vec![],
        };

        let supported =
            SupportedAudioContexts::from_chr(chr.clone(), &[0x00, 0x00, 0x00, 0x00]).unwrap();
        assert_eq!(supported.sink.len(), 0);
        assert_eq!(supported.source.len(), 0);

        let supported =
            SupportedAudioContexts::from_chr(chr.clone(), &[0x08, 0x06, 0x06, 0x03]).unwrap();
        assert_eq!(supported.sink.len(), 3);
        assert_eq!(supported.source.len(), 4);
        assert_eq!(
            supported.source,
            [
                ContextType::Media,
                ContextType::Conversational,
                ContextType::Ringtone,
                ContextType::Notifications
            ]
            .into_iter()
            .collect()
        );
    }
}
