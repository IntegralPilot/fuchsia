// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use packet_encoding::{Decodable, Encodable};

use crate::packets::{
    AdvancedDecodable, Error, MediaAttributeEntries, MediaAttributeId, PacketResult, Scope,
    StatusCode, ATTRIBUTE_ID_LEN,
};

/// See AVRCP 1.6.2 section 6.10.4.3.1 GetItemAttributes command parameters.
#[derive(Debug)]
pub struct GetItemAttributesCommand {
    scope: Scope,
    uid: u64,
    uid_counter: u16,
    // If this list is empty, then all attributes are requested.
    attributes: Vec<MediaAttributeId>,
}

impl GetItemAttributesCommand {
    /// The smallest packet size of a GetItemAttributesCommand.
    /// 1 byte for scope, 8 for uid, 2 for uid counter, 1 for number of attributes.
    const MIN_PACKET_SIZE: usize = 12;

    #[cfg(test)]
    pub fn new(
        scope: Scope,
        uid: u64,
        uid_counter: u16,
        attributes: Vec<MediaAttributeId>,
    ) -> Self {
        Self { scope, uid, uid_counter, attributes }
    }

    pub fn uid(&self) -> u64 {
        self.uid
    }

    #[cfg(test)]
    pub fn uid_counter(&self) -> u16 {
        self.uid_counter
    }

    pub fn scope(&self) -> Scope {
        self.scope
    }

    pub fn from_now_playing_list(
        uid: u64,
        uid_counter: u16,
        attributes: Vec<MediaAttributeId>,
    ) -> GetItemAttributesCommand {
        Self { scope: Scope::NowPlaying, uid, uid_counter, attributes }
    }
}

impl Decodable for GetItemAttributesCommand {
    type Error = Error;

    fn decode(buf: &[u8]) -> PacketResult<Self> {
        if buf.len() < Self::MIN_PACKET_SIZE {
            return Err(Error::InvalidMessageLength);
        }

        // The GetItemAttributes command is used to retrieve the metadata
        // attributes for a particular media element item or folder item.
        let scope = Scope::try_from(buf[0])?;
        if scope == Scope::MediaPlayerList {
            return Err(Error::InvalidParameter);
        }

        let uid = u64::from_be_bytes(buf[1..9].try_into().unwrap());
        let uid_counter = u16::from_be_bytes(buf[9..11].try_into().unwrap());
        let num_attrs = buf[11] as usize;

        let total_packet_size = Self::MIN_PACKET_SIZE + num_attrs * ATTRIBUTE_ID_LEN;
        if buf.len() < total_packet_size {
            return Err(Error::InvalidMessageLength);
        }

        let attributes = match num_attrs {
            // 0 number of attributes is considered as request for all attributes.
            0 => MediaAttributeId::VARIANTS.to_vec(),
            _ => {
                let mut attrs = vec![];
                let mut id_chunks =
                    buf[Self::MIN_PACKET_SIZE..total_packet_size].chunks_exact(ATTRIBUTE_ID_LEN);
                while let Some(chunk) = id_chunks.next() {
                    // As per AVRCP 1.6 Section 26, Appendix E, `MediaAttributeId`s are
                    // only the lower byte of the 4 byte representation, so we can ignore
                    // the upper 3 bytes.
                    attrs.push(MediaAttributeId::try_from(chunk[3])?);
                }
                attrs
            }
        };

        Ok(Self { scope, uid, uid_counter, attributes })
    }
}

impl Encodable for GetItemAttributesCommand {
    type Error = Error;

    fn encoded_len(&self) -> usize {
        Self::MIN_PACKET_SIZE + self.attributes.len() * ATTRIBUTE_ID_LEN
    }

    fn encode(&self, buf: &mut [u8]) -> PacketResult<()> {
        if buf.len() < self.encoded_len() {
            return Err(Error::BufferLengthOutOfRange);
        }

        buf[0] = u8::from(&self.scope);
        buf[1..9].copy_from_slice(&self.uid.to_be_bytes());
        buf[9..11].copy_from_slice(&self.uid_counter.to_be_bytes());
        buf[11] = self.attributes.len() as u8;
        let mut next_idx = Self::MIN_PACKET_SIZE;
        for attr_id in &self.attributes {
            let id: u32 = u32::from(u8::from(attr_id));
            buf[next_idx..next_idx + ATTRIBUTE_ID_LEN].copy_from_slice(&id.to_be_bytes());

            next_idx += ATTRIBUTE_ID_LEN;
        }
        Ok(())
    }
}

/// See AVRCP 1.6.2 section 6.10.4.3.2 GetItemAttributes response parameters.
#[derive(Debug, PartialEq)]
pub enum GetItemAttributesResponse {
    Success(MediaAttributeEntries),
    Failure(StatusCode),
}

impl GetItemAttributesResponse {
    /// 1 byte for status.
    const STATUS_SIZE: usize = 1;

    /// The smallest packet size of a GetItemAttributesResponse.
    /// 1 byte for status and 1 byte for num of attributes.
    const MIN_SUCCESS_RESPONSE_SIZE: usize = 2;
}

impl Encodable for GetItemAttributesResponse {
    type Error = Error;

    fn encoded_len(&self) -> usize {
        match self {
            Self::Failure(_) => Self::STATUS_SIZE,
            Self::Success(a) => Self::STATUS_SIZE + a.encoded_len(),
        }
    }

    fn encode(&self, buf: &mut [u8]) -> PacketResult<()> {
        if buf.len() < self.encoded_len() {
            return Err(Error::BufferLengthOutOfRange);
        }

        let status = match self {
            Self::Failure(status) => status,
            Self::Success(_) => &StatusCode::Success,
        };
        buf[0] = u8::from(status);
        if let Self::Success(a) = self {
            a.encode(&mut buf[1..])?;
        }
        Ok(())
    }
}

impl Decodable for GetItemAttributesResponse {
    type Error = Error;

    fn decode(buf: &[u8]) -> PacketResult<Self> {
        let res = Self::try_decode(buf, false);
        if let Ok(decoded) = res {
            return Ok(decoded.0);
        }
        Self::try_decode(buf, true).map(|decoded| decoded.0)
    }
}

impl AdvancedDecodable for GetItemAttributesResponse {
    type Error = Error;

    fn try_decode(buf: &[u8], should_adjust: bool) -> Result<(Self, usize), Error> {
        if buf.len() < Self::STATUS_SIZE {
            return Err(Error::InvalidMessageLength);
        }

        let status = StatusCode::try_from(buf[0])?;
        if status != StatusCode::Success {
            return Ok((GetItemAttributesResponse::Failure(status), Self::STATUS_SIZE));
        }

        // Successful response.
        if buf.len() < Self::MIN_SUCCESS_RESPONSE_SIZE {
            return Err(Error::InvalidMessageLength);
        }

        let (attributes, decoded_len) =
            MediaAttributeEntries::try_decode(&buf[1..], should_adjust)?;
        if buf.len() != 1 + decoded_len {
            return Err(Error::InvalidMessageLength);
        }

        Ok((GetItemAttributesResponse::Success(attributes), buf.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    fn test_get_item_attributes_command_encode() {
        let cmd = GetItemAttributesCommand::from_now_playing_list(1004, 1, vec![]);
        assert_eq!(12, cmd.encoded_len());
        let mut buf = vec![0; cmd.encoded_len()];
        let _ = cmd.encode(&mut buf[..]).expect("should be ok");
        assert_eq!(buf, &[0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0xEC, 0x00, 0x01, 0x00]);

        let cmd = GetItemAttributesCommand {
            scope: Scope::MediaPlayerVirtualFilesystem,
            uid: 123,
            uid_counter: 333,
            attributes: vec![MediaAttributeId::Title, MediaAttributeId::ArtistName],
        };
        assert_eq!(20, cmd.encoded_len());
        let mut buf = vec![0; cmd.encoded_len()];
        let _ = cmd.encode(&mut buf[..]).expect("should be ok");
        assert_eq!(
            buf,
            &[
                0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x7B, 0x01, 0x4D, 0x02, 0x00, 0x00,
                0x00, 0x01, 0x00, 0x00, 0x00, 0x02,
            ]
        );
    }

    #[fuchsia::test]
    fn test_get_item_attributes_command_encode_fail() {
        let cmd = GetItemAttributesCommand::from_now_playing_list(1004, 1, vec![]);
        assert_eq!(12, cmd.encoded_len());
        let mut buf = vec![0; 10]; // small buffer size.
        let _ = cmd.encode(&mut buf[..]).expect_err("should have failed");
    }

    #[fuchsia::test]
    fn test_get_item_attributes_command_decode_success() {
        let buf = vec![0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0xEC, 0x00, 0x01, 0x00];
        let cmd = GetItemAttributesCommand::decode(&buf[..]).expect("should succeed");
        assert_eq!(Scope::NowPlaying, cmd.scope);
        assert_eq!(1004, cmd.uid);
        assert_eq!(1, cmd.uid_counter);
        assert_eq!(MediaAttributeId::VARIANTS.to_vec(), cmd.attributes);

        let buf = vec![
            0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x7B, 0x01, 0x4D, 0x02, 0x00, 0x00,
            0x00, 0x01, 0x00, 0x00, 0x00, 0x02,
        ];
        let cmd = GetItemAttributesCommand::decode(&buf[..]).expect("should succeed");
        assert_eq!(Scope::MediaPlayerVirtualFilesystem, cmd.scope);
        assert_eq!(123, cmd.uid);
        assert_eq!(333, cmd.uid_counter);
        assert_eq!(vec![MediaAttributeId::Title, MediaAttributeId::ArtistName], cmd.attributes);
    }

    #[fuchsia::test]
    fn test_get_item_attributes_command_decode_fail() {
        // Buffer not long enough.
        let buf = vec![0x03];
        let _ = GetItemAttributesCommand::decode(&buf[..]).expect_err("should have failed");

        // Invalid scope.
        let buf = vec![0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0xEC, 0x00, 0x01, 0x00];
        let _ = GetItemAttributesCommand::decode(&buf[..]).expect_err("should have failed");

        // Buffer not long enough including attributes.
        let buf = vec![0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0xEC, 0x00, 0x01, 0x01];
        let _ = GetItemAttributesCommand::decode(&buf[..]).expect_err("should have failed");
    }

    #[fuchsia::test]
    fn test_get_item_attributes_response_encode() {
        // Failure response.
        let resp = GetItemAttributesResponse::Failure(StatusCode::DoesNotExist);
        assert_eq!(1, resp.encoded_len());
        let mut buf = vec![0; resp.encoded_len()];
        let _ = resp.encode(&mut buf[..]).expect("should be ok");
        assert_eq!(buf, &[0x09]);

        // Success response.
        let resp = GetItemAttributesResponse::Success(MediaAttributeEntries {
            title: Some("test".to_string()),
            artist_name: Some("a".to_string()),
            default_cover_art: Some("bc".to_string()),
            ..Default::default()
        });
        assert_eq!(2 + 12 + 9 + 10, resp.encoded_len());
        let mut buf = vec![0; resp.encoded_len()];
        let _ = resp.encode(&mut buf[..]).expect("should be ok");
        assert_eq!(
            buf,
            &[
                0x04, 0x03, 0x00, 0x00, 0x00, 0x01, 0x00, 0x6A, 0x00, 0x04, 't' as u8, 'e' as u8,
                's' as u8, 't' as u8, 0x00, 0x00, 0x00, 0x02, 0x00, 0x6A, 0x00, 0x01, 'a' as u8,
                0x00, 0x00, 0x00, 0x08, 0x00, 0x6A, 0x00, 0x02, 'b' as u8, 'c' as u8,
            ]
        );
    }

    #[fuchsia::test]
    fn test_get_item_attributes_response_encode_fail() {
        let resp = GetItemAttributesResponse::Failure(StatusCode::DoesNotExist);
        assert_eq!(1, resp.encoded_len());
        let mut buf = vec![]; // insufficient buffer size.
        let _ = resp.encode(&mut buf[..]).expect_err("should have failed");

        let resp = GetItemAttributesResponse::Success(MediaAttributeEntries {
            title: Some("test".to_string()),
            ..Default::default()
        });
        assert_eq!(2 + 12, resp.encoded_len());
        let mut buf = vec![0; 4]; // insufficient buffer size.
        let _ = resp.encode(&mut buf[..]).expect_err("should have failed");
    }

    #[fuchsia::test]
    fn test_get_item_attributes_response_decode() {
        // Failure response.
        let buf = vec![0x09];
        let resp = GetItemAttributesResponse::decode(&buf).expect("should be ok");
        assert_eq!(GetItemAttributesResponse::Failure(StatusCode::DoesNotExist), resp);

        // Success response.
        let buf = vec![
            0x04, 0x03, 0x00, 0x00, 0x00, 0x01, 0x00, 0x6A, 0x00, 0x04, 't' as u8, 'e' as u8,
            's' as u8, 't' as u8, 0x00, 0x00, 0x00, 0x02, 0x00, 0x6A, 0x00, 0x01, 'a' as u8, 0x00,
            0x00, 0x00, 0x08, 0x00, 0x6A, 0x00, 0x02, 'b' as u8, 'c' as u8,
        ];
        let resp = GetItemAttributesResponse::decode(&buf).expect("should be ok");
        assert_eq!(
            GetItemAttributesResponse::Success(MediaAttributeEntries {
                title: Some("test".to_string()),
                artist_name: Some("a".to_string()),
                default_cover_art: Some("bc".to_string()),
                ..Default::default()
            }),
            resp
        );

        // Success response with incorrect len. Incorrect len item is removed.
        let buf = vec![
            0x04, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x6A, 0x00, 0x06, 't' as u8, 'e' as u8,
            's' as u8, 't' as u8,
        ];
        let resp = GetItemAttributesResponse::decode(&buf).expect("should be ok");
        assert_eq!(
            GetItemAttributesResponse::Success(MediaAttributeEntries { ..Default::default() }),
            resp
        );
    }

    #[fuchsia::test]
    fn test_get_item_attributes_response_decode_fail() {
        // Failure response.
        let buf = vec![];
        let _ = GetItemAttributesResponse::decode(&buf).expect_err("should have failed");

        // Success response with invalid number of attributes.
        let buf = vec![
            0x04, 0x01, // Says 1 attribute but there are 2.
            0x00, 0x00, 0x00, 0x01, 0x00, 0x6A, 0x00, 0x04, 't' as u8, 'e' as u8, 's' as u8,
            't' as u8, 0x00, 0x00, 0x00, 0x02, 0x00, 0x6A, 0x00, 0x01, 'a' as u8,
        ];
        let _ = GetItemAttributesResponse::decode(&buf).expect_err("should have failed");

        // Success response with invalid number of attributes.
        let buf = vec![
            0x04, 0x03, // Says 3 attributes but there are 2.
            0x00, 0x00, 0x00, 0x01, 0x00, 0x6A, 0x00, 0x04, 't' as u8, 'e' as u8, 's' as u8,
            't' as u8, 0x00, 0x00, 0x00, 0x02, 0x00, 0x6A, 0x00, 0x01, 'a' as u8,
        ];
        let _ = GetItemAttributesResponse::decode(&buf).expect_err("should have failed");

        let buf = vec![
            0x04, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x6A, 0x00, 0x02, 't' as u8, 'e' as u8,
            's' as u8, 't' as u8,
        ];
        let _ = GetItemAttributesResponse::decode(&buf).expect_err("should have failed");
    }

    #[fuchsia::test]
    fn test_get_item_attributes_response_decode_variant_missing_fcs() {
        let buf = vec![
            0x04, 0x05, 0x00, 0x00, 0x00, 0x01, 0x00, 0x6a, 0x00, 0x06, 0x47, 0x6c, 0x6f, 0x72,
            0x69, 0x61, 0x00, 0x00, 0x00, 0x02, 0x00, 0x6a, 0x00, 0x0b, 0x45, 0x61, 0x72, 0x6c,
            0x79, 0x20, 0x44, 0x6f, 0x6f, 0x72, 0x73, 0x00, 0x00, 0x00, 0x03, 0x00, 0x6a, 0x00,
            0x0f, 0x47, 0x6c, 0x6f, 0x72, 0x69, 0x61, 0x20, 0x2d, 0x20, 0x53, 0x69, 0x6e, 0x67,
            0x6c, 0x65, 0x00, 0x00, 0x00, 0x06, 0x00, 0x6a, 0x00, 0x04, 0x52, 0x6f, 0x63, 0x6b,
            0x00, 0x00, 0x00, 0x07, 0x00, 0x6a, 0x00, 0x06, 0x32, 0x37, 0x37, 0x39,
        ];

        let resp = GetItemAttributesResponse::decode(&buf).expect("should be ok");
        assert_eq!(
            GetItemAttributesResponse::Success(MediaAttributeEntries {
                title: Some("Gloria".to_string()),
                artist_name: Some("Early Doors".to_string()),
                album_name: Some("Gloria - Single".to_string()),
                genre: Some("Rock".to_string()),
                // This attribute is short so we ignore it, because the last two
                // bytes are missing (missing FCS bits in the L2CAP packet)
                // playing_time: Some("277966".to_string()),
                ..Default::default()
            }),
            resp
        );
    }
}
