// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::core::ltv::LtValue;
use crate::packet_encoding::Error as PacketError;
use crate::{decodable_enum, CompanyId};

use crate::generic_audio::ContextType;

decodable_enum! {
    pub enum MetadataType<u8, PacketError, OutOfRange> {
        PreferredAudioContexts = 0x01,
        StreamingAudioContexts = 0x02,
        ProgramInfo = 0x03,
        Language = 0x04,
        CCIDList = 0x05,
        ParentalRating = 0x06,
        ProgramInfoURI = 0x07,
        AudioActiveState = 0x08,
        BroadcastAudioImmediateRenderingFlag = 0x09,
        ExtendedMetadata = 0xfe,
        VendorSpecific = 0xff,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Metadata {
    PreferredAudioContexts(Vec<ContextType>),
    StreamingAudioContexts(Vec<ContextType>),
    ProgramInfo(String),
    /// 3 byte, lower case language code as defined in ISO 639-­3
    Language(String),
    CCIDList(Vec<u8>),
    ParentalRating(Rating),
    ProgramInfoURI(String),
    ExtendedMetadata {
        type_: u16,
        data: Vec<u8>,
    },
    VendorSpecific {
        company_id: CompanyId,
        data: Vec<u8>,
    },
    AudioActiveState(bool),
    // Flag for Broadcast Audio Immediate Rendering.
    BroadcastAudioImmediateRenderingFlag,
}

impl LtValue for Metadata {
    type Type = MetadataType;

    const NAME: &'static str = "Metadata";

    fn type_from_octet(x: u8) -> Option<Self::Type> {
        x.try_into().ok()
    }

    fn length_range_from_type(ty: Self::Type) -> std::ops::RangeInclusive<u8> {
        match ty {
            MetadataType::PreferredAudioContexts | MetadataType::StreamingAudioContexts => 3..=3,
            MetadataType::ProgramInfo | MetadataType::CCIDList | MetadataType::ProgramInfoURI => {
                1..=u8::MAX
            }
            MetadataType::Language => 4..=4,
            MetadataType::ParentalRating => 2..=2,
            MetadataType::ExtendedMetadata | MetadataType::VendorSpecific => 3..=u8::MAX,
            MetadataType::AudioActiveState => 2..=2,
            MetadataType::BroadcastAudioImmediateRenderingFlag => 1..=1,
        }
    }

    fn into_type(&self) -> Self::Type {
        match self {
            Metadata::PreferredAudioContexts(_) => MetadataType::PreferredAudioContexts,
            Metadata::StreamingAudioContexts(_) => MetadataType::StreamingAudioContexts,
            Metadata::ProgramInfo(_) => MetadataType::ProgramInfo,
            Metadata::Language(_) => MetadataType::Language,
            Metadata::CCIDList(_) => MetadataType::CCIDList,
            Metadata::ParentalRating(_) => MetadataType::ParentalRating,
            Metadata::ProgramInfoURI(_) => MetadataType::ProgramInfoURI,
            Metadata::ExtendedMetadata { .. } => MetadataType::ExtendedMetadata,
            Metadata::VendorSpecific { .. } => MetadataType::VendorSpecific,
            Metadata::AudioActiveState(_) => MetadataType::AudioActiveState,
            Metadata::BroadcastAudioImmediateRenderingFlag => {
                MetadataType::BroadcastAudioImmediateRenderingFlag
            }
        }
    }

    fn value_encoded_len(&self) -> u8 {
        match self {
            Metadata::PreferredAudioContexts(_) => 2,
            Metadata::StreamingAudioContexts(_) => 2,
            Metadata::ProgramInfo(value) => value.len() as u8,
            Metadata::Language(_) => 3,
            Metadata::CCIDList(ccids) => ccids.len() as u8,
            Metadata::ParentalRating(_) => 1,
            Metadata::ProgramInfoURI(uri) => uri.len() as u8,
            Metadata::ExtendedMetadata { type_: _, data } => 2 + data.len() as u8,
            Metadata::VendorSpecific { company_id: _, data } => 2 + data.len() as u8,
            Metadata::AudioActiveState(_) => 1,
            Metadata::BroadcastAudioImmediateRenderingFlag => 0,
        }
    }

    fn decode_value(ty: &Self::Type, buf: &[u8]) -> Result<Self, crate::packet_encoding::Error> {
        let m: Metadata = match ty {
            MetadataType::PreferredAudioContexts | MetadataType::StreamingAudioContexts => {
                let context_type =
                    ContextType::from_bits(u16::from_le_bytes([buf[0], buf[1]])).collect();
                if ty == &MetadataType::PreferredAudioContexts {
                    Self::PreferredAudioContexts(context_type)
                } else {
                    Self::StreamingAudioContexts(context_type)
                }
            }
            MetadataType::ProgramInfo | MetadataType::ProgramInfoURI => {
                let value = String::from_utf8(buf[..].to_vec())
                    .map_err(|e| PacketError::InvalidParameter(format!("{e}")))?;
                if ty == &MetadataType::ProgramInfo {
                    Self::ProgramInfo(value)
                } else {
                    Self::ProgramInfoURI(value)
                }
            }
            MetadataType::Language => {
                let code = String::from_utf8(buf[..].to_vec())
                    .map_err(|e| PacketError::InvalidParameter(format!("{e}")))?;
                Self::Language(code)
            }
            MetadataType::CCIDList => Self::CCIDList(buf[..].to_vec()),
            MetadataType::ParentalRating => Self::ParentalRating(buf[0].into()),
            MetadataType::ExtendedMetadata | MetadataType::VendorSpecific => {
                let type_or_id = u16::from_le_bytes(buf[0..2].try_into().unwrap());
                let data = if buf.len() >= 2 { buf[2..].to_vec() } else { vec![] };
                if ty == &MetadataType::ExtendedMetadata {
                    Self::ExtendedMetadata { type_: type_or_id, data }
                } else {
                    Self::VendorSpecific { company_id: type_or_id.into(), data }
                }
            }
            MetadataType::AudioActiveState => Self::AudioActiveState(buf[0] != 0),
            MetadataType::BroadcastAudioImmediateRenderingFlag => {
                Self::BroadcastAudioImmediateRenderingFlag
            }
        };
        Ok(m)
    }

    fn encode_value(&self, buf: &mut [u8]) -> Result<(), crate::packet_encoding::Error> {
        match self {
            Self::PreferredAudioContexts(type_) | Self::StreamingAudioContexts(type_) => {
                [buf[0], buf[1]] = ContextType::to_bits(type_.iter()).to_le_bytes();
            }
            Self::ProgramInfo(value) | Self::ProgramInfoURI(value) => {
                buf.copy_from_slice(value.as_bytes())
            }
            Self::Language(value) if value.len() != 3 => {
                return Err(PacketError::InvalidParameter(format!("{self}")));
            }
            Self::Language(value) => buf.copy_from_slice(&value.as_bytes()[..3]),
            Self::CCIDList(value) => buf.copy_from_slice(&value.as_slice()),
            Self::ParentalRating(value) => buf[0] = value.into(),
            Self::ExtendedMetadata { type_, data } => {
                buf[0..2].copy_from_slice(&type_.to_le_bytes());
                buf[2..].copy_from_slice(&data.as_slice());
            }
            Self::VendorSpecific { company_id, data } => {
                [buf[0], buf[1]] = company_id.to_le_bytes();
                buf[2..].copy_from_slice(&data.as_slice());
            }
            Self::AudioActiveState(value) => buf[0] = *value as u8,
            Self::BroadcastAudioImmediateRenderingFlag => {}
        }
        Ok(())
    }
}

impl std::fmt::Display for Metadata {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            // TODO(b/308483171): Consider using the Display for the inner fields instead of Debug.
            Metadata::PreferredAudioContexts(v) => write!(f, "Preferred Audio Contexts: {v:?}"),
            Metadata::StreamingAudioContexts(v) => write!(f, "Streaming Audio Contexts: {v:?}"),
            Metadata::ProgramInfo(v) => write!(f, "Progaam Info: {v:?}"),
            Metadata::Language(v) => write!(f, "Language: {v:?}"),
            Metadata::CCIDList(v) => write!(f, "CCID List: {v:?}"),
            Metadata::ParentalRating(v) => write!(f, "Parental Rating: {v:?}"),
            Metadata::ProgramInfoURI(v) => write!(f, "Program Info URI: {v:?}"),
            Metadata::ExtendedMetadata { type_, data } => {
                write!(f, "Extended Metadata: type(0x{type_:02x}) data({data:?})")
            }
            Metadata::VendorSpecific { company_id, data } => {
                write!(f, "Vendor Specific: {company_id} data({data:?})")
            }
            Metadata::AudioActiveState(v) => write!(f, "Audio Active State: {v}"),
            Metadata::BroadcastAudioImmediateRenderingFlag => {
                write!(f, "Broadcast Audio Immediate Rendering Flag")
            }
        }
    }
}

/// Represents recommended minimum age of the viewer.
/// The numbering scheme aligns with Annex F of EN 300 707 v1.2.1
/// published by ETSI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rating {
    NoRating,
    AllAge,
    // Recommended for listeners of age x years, where x is the
    // value of u8 that's greater than or equal to 5.
    Age(u16),
}

impl Rating {
    // Minimum age that can be recommended.
    const MIN_RECOMMENDED_AGE: u16 = 5;

    // When recommending for listeners of age Y years,
    // Subtract 3 from the recommended age to get the encoded value.
    // E.g., to indicate recommended age of 8 years or older, encode 5.
    const AGE_OFFSET: u16 = 3;

    pub const fn no_rating() -> Self {
        Self::NoRating
    }

    pub const fn all_age() -> Self {
        Self::AllAge
    }

    pub fn min_recommended(age: u16) -> Result<Self, PacketError> {
        if age < Self::MIN_RECOMMENDED_AGE {
            return Err(PacketError::InvalidParameter(format!(
                "minimum recommended age must be at least 5. Got {age}"
            )));
        }
        // We can represent up to 255 + 3 (age 258) using this encoding.
        if age > u8::MAX as u16 + Self::AGE_OFFSET {
            return Err(PacketError::OutOfRange);
        }
        Ok(Rating::Age(age))
    }
}

impl From<&Rating> for u8 {
    fn from(value: &Rating) -> Self {
        match value {
            Rating::NoRating => 0x00,
            Rating::AllAge => 0x01,
            Rating::Age(a) => (a - Rating::AGE_OFFSET) as u8,
        }
    }
}

impl From<u8> for Rating {
    fn from(value: u8) -> Self {
        match value {
            0x00 => Rating::NoRating,
            0x01 => Rating::AllAge,
            value => {
                let age = value as u16 + Self::AGE_OFFSET;
                Rating::min_recommended(age).unwrap()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::packet_encoding::{Decodable, Encodable};

    #[test]
    fn metadataum_preferred_audio_contexts() {
        // Encoding.
        let test = Metadata::PreferredAudioContexts(vec![ContextType::Conversational]);
        assert_eq!(test.encoded_len(), 4);
        let mut buf = vec![0; test.encoded_len()];
        let _ = test.encode(&mut buf[..]).expect("should not fail");

        let bytes = vec![0x03, 0x01, 0x02, 0x00];
        assert_eq!(buf, bytes);

        // Decoding.
        let (decoded, len) = Metadata::decode(&buf);
        assert_eq!(decoded, Ok(test));
        assert_eq!(len, 4);
    }

    #[test]
    fn metadatum_streaming_audio_contexts() {
        // Encoding.
        let test =
            Metadata::StreamingAudioContexts(vec![ContextType::Ringtone, ContextType::Alerts]);
        assert_eq!(test.encoded_len(), 4);
        let mut buf = vec![0; test.encoded_len()];
        let _ = test.encode(&mut buf[..]).expect("should not fail");

        let bytes = vec![0x03, 0x02, 0x00, 0x06];
        assert_eq!(buf, bytes);

        // Decoding.
        let (decoded, len) = Metadata::decode(&buf);
        assert_eq!(decoded, Ok(test));
        assert_eq!(len, 4);
    }

    #[test]
    fn metadatum_program_info() {
        // Encoding.
        let test = Metadata::ProgramInfo("a".to_string());
        assert_eq!(test.encoded_len(), 3);
        let mut buf = vec![0; test.encoded_len()];
        let _ = test.encode(&mut buf[..]).expect("should not fail");

        let bytes = vec![0x02, 0x03, 0x61];
        assert_eq!(buf, bytes);

        // Decoding.
        let (decoded, len) = Metadata::decode(&buf);
        assert_eq!(decoded, Ok(test));
        assert_eq!(len, 3);
    }

    #[test]
    fn metadatum_language_code() {
        // Encoding.
        let test = Metadata::Language("eng".to_string());
        assert_eq!(test.encoded_len(), 5);
        let mut buf = vec![0; test.encoded_len()];
        let _ = test.encode(&mut buf[..]).expect("should not fail");

        let bytes = vec![0x04, 0x04, 0x65, 0x6E, 0x67];
        assert_eq!(buf, bytes);

        // Decoding.
        let (decoded, len) = Metadata::decode(&buf);
        assert_eq!(decoded, Ok(test));
        assert_eq!(len, 5);
    }

    #[test]
    fn metadatum_ccid_list() {
        // Encoding.
        let test = Metadata::CCIDList(vec![0x01, 0x02]);
        assert_eq!(test.encoded_len(), 4);
        let mut buf = vec![0; test.encoded_len()];
        let _ = test.encode(&mut buf[..]).expect("should not fail");

        let bytes = vec![0x03, 0x05, 0x01, 0x02];
        assert_eq!(buf, bytes);

        // Decoding.
        let (decoded, len) = Metadata::decode(&buf);
        assert_eq!(decoded, Ok(test));
        assert_eq!(len, 4);
    }

    #[test]
    fn metadatum_parental_rating() {
        // Encding.
        let test = Metadata::ParentalRating(Rating::Age(8));
        assert_eq!(test.encoded_len(), 0x03);
        let mut buf = vec![0; test.encoded_len()];
        let _ = test.encode(&mut buf[..]).expect("should not fail");

        let bytes = vec![0x02, 0x06, 0x05];
        assert_eq!(buf, bytes);

        // Decoding.
        let (decoded, len) = Metadata::decode(&buf);
        assert_eq!(decoded, Ok(test));
        assert_eq!(len, 3);
    }

    #[test]
    fn metadatum_vendor_specific() {
        // Encoding.
        let test = Metadata::VendorSpecific { company_id: 0x00E0.into(), data: vec![0x01, 0x02] };
        assert_eq!(test.encoded_len(), 0x06);
        let mut buf = vec![0; test.encoded_len()];
        let _ = test.encode(&mut buf[..]).expect("should not fail");

        let bytes = vec![0x05, 0xFF, 0xE0, 0x00, 0x01, 0x02];
        assert_eq!(buf, bytes);

        // Decoding.
        let (decoded, len) = Metadata::decode(&buf);
        assert_eq!(decoded, Ok(test));
        assert_eq!(len, 6);
    }

    #[test]
    fn metadatum_audio_active_state() {
        // Encoding.
        let test = Metadata::AudioActiveState(true);
        assert_eq!(test.encoded_len(), 0x03);
        let mut buf = vec![0; test.encoded_len()];
        let _ = test.encode(&mut buf[..]).expect("should not fail");

        let bytes = vec![0x02, 0x08, 0x01];
        assert_eq!(buf, bytes);

        // Decoding.
        let (decoded, len) = Metadata::decode(&buf);
        assert_eq!(decoded, Ok(test));
        assert_eq!(len, 3);
    }

    #[test]
    fn metadatum_broadcast_audio_immediate_rendering() {
        // Encoding.
        let test = Metadata::BroadcastAudioImmediateRenderingFlag;
        assert_eq!(test.encoded_len(), 0x02);
        let mut buf = vec![0; test.encoded_len()];
        let _ = test.encode(&mut buf[..]).expect("should not fail");

        let bytes = vec![0x01, 0x09];
        assert_eq!(buf, bytes);

        // Decoding.
        let (decoded, len) = Metadata::decode(&buf);
        assert_eq!(decoded, Ok(test));
        assert_eq!(len, 2);
    }

    #[test]
    fn invalid_metadataum() {
        // Language code must be 3-lettered.
        let test = Metadata::Language("illegal".to_string());
        let mut buf = vec![0; test.encoded_len()];
        let _ = test.encode(&mut buf[..]).expect_err("should fail");

        // Not enough length for Length and Type for decoding.
        let buf = vec![0x03];
        let _ = Metadata::decode(&buf).0.expect_err("should fail");

        // Not enough length for Value field for decoding.
        let buf = vec![0x02, 0x01, 0x02];
        let _ = Metadata::decode(&buf).0.expect_err("should fail");

        // Buffer length does not match Length value for decoding.
        let buf = vec![0x03, 0x03, 0x61];
        let _ = Metadata::decode(&buf).0.expect_err("should fail");
    }

    #[test]
    fn rating() {
        let no_rating = Rating::no_rating();
        let all_age = Rating::all_age();
        let for_age = Rating::min_recommended(5).expect("should succeed");

        assert_eq!(<&Rating as Into<u8>>::into(&no_rating), 0x00);
        assert_eq!(<&Rating as Into<u8>>::into(&all_age), 0x01);
        assert_eq!(<&Rating as Into<u8>>::into(&for_age), 0x02);
    }

    #[test]
    fn decode_rating() {
        let res = Rating::from(0);
        assert_eq!(res, Rating::NoRating);

        let res = Rating::from(1);
        assert_eq!(res, Rating::AllAge);

        let res = Rating::from(2);
        assert_eq!(res, Rating::Age(5));

        let res = Rating::from(255);
        assert_eq!(res, Rating::Age(258));

        let res = Rating::min_recommended(258).unwrap();
        assert_eq!(255u8, (&res).into());
    }

    #[test]
    fn invalid_rating() {
        let _ = Rating::min_recommended(4).expect_err("should have failed");
        let _ = Rating::min_recommended(350).expect_err("can't recommend for elves");
    }
}
