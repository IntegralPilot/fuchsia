// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use packet_encoding::{Decodable, Encodable};

use crate::packets::{
    AdvancedDecodable, AvcCommandType, Error, MediaAttributeEntries, MediaAttributeId,
    PacketResult, PduId, VendorCommand, VendorDependentPdu, ATTRIBUTE_ID_LEN,
};

// See AVRCP 1.6.1 section 6.6 Media Information PDUs - GetElementAttributes for format.

// Identifier is NOW_PLAYING which is all zeros, eight bytes long.
const IDENTIFIER_LEN: usize = 8;
// The number of attributes is limited to 256.
const ATTRIBUTE_COUNT_LEN: usize = 1;
// Attribute count follows immediately after the identifier
const ATTRIBUTE_COUNT_OFFSET: usize = 8;

#[derive(Debug)]
/// AVRCP 1.6.1 section 6.6 Media Information PDUs - GetElementAttributes
pub struct GetElementAttributesCommand {
    attributes: Vec<MediaAttributeId>,
}

impl GetElementAttributesCommand {
    pub fn all_attributes() -> GetElementAttributesCommand {
        Self { attributes: MediaAttributeId::VARIANTS.to_vec() }
    }

    #[cfg(test)]
    pub fn from_attributes(attributes: &[MediaAttributeId]) -> GetElementAttributesCommand {
        if attributes.len() == 0 {
            return Self::all_attributes();
        }
        Self { attributes: attributes.to_vec() }
    }

    pub fn attributes(&self) -> &[MediaAttributeId] {
        return &self.attributes[..];
    }
}

/// Packet PDU ID for vendor dependent packet encoding.
impl VendorDependentPdu for GetElementAttributesCommand {
    fn pdu_id(&self) -> PduId {
        PduId::GetElementAttributes
    }
}

/// Specifies the AVC command type for this AVC command packet
impl VendorCommand for GetElementAttributesCommand {
    fn command_type(&self) -> AvcCommandType {
        AvcCommandType::Status
    }
}

impl Decodable for GetElementAttributesCommand {
    type Error = Error;

    fn decode(buf: &[u8]) -> PacketResult<Self> {
        if buf.len() < IDENTIFIER_LEN + ATTRIBUTE_COUNT_LEN {
            // 8 byte identifier + 1 byte attribute count
            return Err(Error::InvalidMessageLength);
        }

        let identifier = &buf[..IDENTIFIER_LEN];
        if identifier != &[0; IDENTIFIER_LEN] {
            // Only supported command is NOW_PLAYING (0x00 x8)
            return Err(Error::InvalidParameter);
        }
        let attribute_count = buf[IDENTIFIER_LEN] as usize;

        let mut attributes;
        if attribute_count == 0 {
            attributes = MediaAttributeId::VARIANTS.to_vec();
        } else {
            if buf.len()
                < IDENTIFIER_LEN + ATTRIBUTE_COUNT_LEN + (attribute_count * ATTRIBUTE_ID_LEN)
            {
                return Err(Error::InvalidMessageLength);
            }

            attributes = vec![];
            const START_OFFSET: usize = IDENTIFIER_LEN + ATTRIBUTE_COUNT_LEN + 3;
            // Attributes are excessively long at 4 bytes. We only care about the last byte.
            // Skip the first 3 bytes.
            for i in (START_OFFSET..((attribute_count * ATTRIBUTE_ID_LEN) + START_OFFSET))
                .step_by(ATTRIBUTE_ID_LEN)
            {
                attributes.push(MediaAttributeId::try_from(buf[i])?);
            }
        }

        Ok(Self { attributes })
    }
}

impl Encodable for GetElementAttributesCommand {
    type Error = Error;

    fn encoded_len(&self) -> usize {
        let mut len = IDENTIFIER_LEN + ATTRIBUTE_COUNT_LEN;
        if self.attributes != MediaAttributeId::VARIANTS {
            len += self.attributes.len() * 4
        }
        len
    }

    fn encode(&self, buf: &mut [u8]) -> PacketResult<()> {
        if buf.len() < self.encoded_len() {
            return Err(Error::BufferLengthOutOfRange);
        }

        // Only supported command is NOW_PLAYING (0x00 x8)
        buf[0..IDENTIFIER_LEN].fill(0);
        if self.attributes == MediaAttributeId::VARIANTS {
            buf[ATTRIBUTE_COUNT_OFFSET] = 0;
        } else {
            buf[ATTRIBUTE_COUNT_OFFSET] =
                u8::try_from(self.attributes.len()).map_err(|_| Error::InvalidMessageLength)?;
            // Attributes are excessively long at 4 bytes. We only care about the last byte.
            // Skip the first 3 bytes.
            const START_OFFSET: usize = IDENTIFIER_LEN + ATTRIBUTE_COUNT_LEN + 3;
            let mut i = START_OFFSET;
            for attr in self.attributes.iter() {
                buf[i] = u8::from(attr);
                i += ATTRIBUTE_ID_LEN;
            }
        }

        Ok(())
    }
}

#[derive(Debug, Default)]
/// AVRCP 1.6.1 section 6.6 Media Information PDUs- GetElementAttributes
pub struct GetElementAttributesResponse(pub MediaAttributeEntries);

/// Packet PDU ID for vendor dependent packet encoding.
impl VendorDependentPdu for GetElementAttributesResponse {
    fn pdu_id(&self) -> PduId {
        PduId::GetElementAttributes
    }
}

impl Decodable for GetElementAttributesResponse {
    type Error = Error;

    fn decode(buf: &[u8]) -> PacketResult<Self> {
        let (attributes, decoded_len) = MediaAttributeEntries::try_decode(&buf[..], false)?;
        if buf.len() != decoded_len {
            return Err(Error::InvalidMessageLength);
        }
        Ok(Self(attributes))
    }
}

impl Encodable for GetElementAttributesResponse {
    type Error = Error;

    fn encoded_len(&self) -> usize {
        self.0.encoded_len()
    }

    fn encode(&self, buf: &mut [u8]) -> PacketResult<()> {
        self.0.encode(&mut buf[..])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packets::{PacketEncodable, VendorDependentRawPdu};

    #[test]
    fn test_get_element_attributes_command_encode_all_attributes() {
        let b = GetElementAttributesCommand::all_attributes();
        assert_eq!(
            b.attributes(),
            &[
                MediaAttributeId::Title,
                MediaAttributeId::ArtistName,
                MediaAttributeId::AlbumName,
                MediaAttributeId::TrackNumber,
                MediaAttributeId::TotalNumberOfTracks,
                MediaAttributeId::Genre,
                MediaAttributeId::PlayingTime,
                MediaAttributeId::DefaultCoverArt
            ]
        );
        assert_eq!(b.raw_pdu_id(), u8::from(&PduId::GetElementAttributes));
        assert_eq!(b.command_type(), AvcCommandType::Status);
        assert_eq!(b.encoded_len(), 9); // identifier, length
        let mut buf = vec![0; b.encoded_len()];
        assert!(b.encode(&mut buf[..]).is_ok());
        assert_eq!(
            buf,
            &[
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // NOW_PLAYING identifier
                0x00  // passing 0 for all attributes
            ]
        );
    }

    #[test]
    fn test_get_element_attributes_command_encode_some_attributes() {
        let b = GetElementAttributesCommand::from_attributes(&[
            MediaAttributeId::Title,
            MediaAttributeId::ArtistName,
        ]);
        assert_eq!(b.raw_pdu_id(), u8::from(&PduId::GetElementAttributes));
        assert_eq!(b.command_type(), AvcCommandType::Status);
        assert_eq!(b.attributes(), &[MediaAttributeId::Title, MediaAttributeId::ArtistName]);
        assert_eq!(b.encoded_len(), 8 + 1 + 4 + 4); // identifier, length, 2 attributes
        let mut buf = vec![0; b.encoded_len()];
        assert!(b.encode(&mut buf[..]).is_ok());
        assert_eq!(
            buf,
            &[
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // NOW_PLAYING identifier
                0x02, // 2 attributes
                0x00, 0x00, 0x00, 0x01, // Title
                0x00, 0x00, 0x00, 0x02, // ArtistName
            ]
        );
    }

    #[test]
    fn test_get_element_attributes_command_decode_some_attributes() {
        let b = GetElementAttributesCommand::decode(&[
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // NOW_PLAYING identifier
            0x02, // 2 attributes
            0x00, 0x00, 0x00, 0x01, // Title
            0x00, 0x00, 0x00, 0x02, // ArtistName
        ])
        .expect("unable to decode");
        assert_eq!(b.attributes(), &[MediaAttributeId::Title, MediaAttributeId::ArtistName]);
        assert_eq!(b.encoded_len(), 8 + 1 + 4 + 4); // identifier, length, 2 attributes
    }

    #[test]
    fn test_get_element_attributes_command_decode_all_attributes() {
        let b = GetElementAttributesCommand::decode(&[
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // NOW_PLAYING identifier
            0x00, // 0 attributes for all
        ])
        .expect("unable to decode");
        assert_eq!(b.raw_pdu_id(), u8::from(&PduId::GetElementAttributes));
        assert_eq!(
            b.attributes(),
            &[
                MediaAttributeId::Title,
                MediaAttributeId::ArtistName,
                MediaAttributeId::AlbumName,
                MediaAttributeId::TrackNumber,
                MediaAttributeId::TotalNumberOfTracks,
                MediaAttributeId::Genre,
                MediaAttributeId::PlayingTime,
                MediaAttributeId::DefaultCoverArt
            ]
        );
        assert_eq!(b.encoded_len(), 8 + 1); // identifier, attribute count
    }

    #[test]
    fn test_get_element_attributes_response_encode() {
        let b = GetElementAttributesResponse(MediaAttributeEntries {
            title: Some(String::from("Test")),
            ..MediaAttributeEntries::default()
        });
        assert_eq!(b.raw_pdu_id(), u8::from(&PduId::GetElementAttributes));
        assert_eq!(b.encoded_len(), 1 + 8 + 4); // count, attribute header (8), attribute encoded len (len of "Test")
        let mut buf = vec![0; b.encoded_len()];
        assert!(b.encode(&mut buf[..]).is_ok());
        assert_eq!(
            buf,
            &[
                0x01, // count
                0x00, 0x00, 0x00, 0x01, // element attribute id (title)
                0x00, 0x6a, // encoding UTF-8
                0x00, 0x04, // attribute length
                'T' as u8, 'e' as u8, 's' as u8, 't' as u8, // attribute payload
            ]
        );
    }

    #[test]
    fn test_get_element_attributes_response_decode() {
        let b = GetElementAttributesResponse::decode(&[
            0x01, // count
            0x00, 0x00, 0x00, 0x01, // element attribute id (title)
            0x00, 0x6a, // encoding UTF-8
            0x00, 0x04, // attribute length
            'T' as u8, 'e' as u8, 's' as u8, 't' as u8, // attribute payload
        ])
        .expect("unable to decode packet");
        assert_eq!(b.raw_pdu_id(), u8::from(&PduId::GetElementAttributes));
        assert_eq!(b.encoded_len(), 1 + 8 + 4); // count, attribute header (8), attribute encoded len (len of "Test")
        assert_eq!(b.0.title, Some(String::from("Test")));
    }

    #[test]
    fn test_encode_packets() {
        let b = GetElementAttributesResponse(MediaAttributeEntries {
            title: Some(String::from(
                "Lorem ipsum dolor sit amet,\
                 consectetur adipiscing elit. Nunc eget elit cursus ipsum \
                 fermentum viverra id vitae lorem. Cras luctus elementum \
                 metus vel volutpat. Vestibulum ante ipsum primis in \
                 faucibus orci luctus et ultrices posuere cubilia \
                 Curae; Praesent efficitur velit sed metus luctus",
            )),
            artist_name: Some(String::from(
                "elit euismod. \
                 Sed ex mauris, convallis a augue ac, hendrerit \
                 blandit mauris. Integer porttitor velit et posuere pharetra. \
                 Nullam ultricies justo sit amet lorem laoreet, id porta elit \
                 gravida. Suspendisse sed lectus eu lacus efficitur finibus. \
                 Sed egestas pretium urna eu pellentesque. In fringilla nisl dolor, \
                 sit amet luctus purus sagittis et. Mauris diam turpis, luctus et pretium nec, \
                 aliquam sed odio. Nulla finibus, orci a lacinia sagittis,\
                 urna elit ultricies dolor, et condimentum magna mi vitae sapien. \
                 Suspendisse potenti. Vestibulum ante ipsum primis in faucibus orci \
                 luctus et ultrices posuere cubilia Curae",
            )),
            album_name: Some(String::from(
                "Mauris in ante ultrices, vehicula lorem non, sagittis metus.\
                 Nam facilisis volutpat quam. Suspendisse sem ipsum, blandit ut faucibus vitae,\
                 facilisis quis massa. Aliquam sagittis, orci sed dignissim vulputate, odio neque \
                 tempor dui, vel feugiat metus massa id urna. Nam at risus sem.\
                 Duis commodo suscipit metus, at placerat elit suscipit eget. Suspendisse interdum \
                 id metus vitae porta. Ut cursus viverra imperdiet. Aliquam erat volutpat. \
                 Curabitur vehicula mauris nec ex sollicitudin rhoncus. Integer ipsum libero, \
                 porta id velit et, egestas facilisis tellus.",
            )),
            genre: Some(String::from(
                "Mauris in ante ultrices, vehicula lorem non, sagittis metus.\
                 Nam facilisis volutpat quam. Suspendisse sem ipsum, blandit ut faucibus vitae,\
                 facilisis quis massa. Aliquam sagittis, orci sed dignissim vulputate, odio neque \
                 tempor dui, vel feugiat metus massa id urna. Nam at risus sem.\
                 Duis commodo suscipit metus, at placerat elit suscipit eget. Suspendisse interdum \
                 id metus vitae porta. Ut cursus viverra imperdiet. Aliquam erat volutpat. \
                 Curabitur vehicula mauris nec ex sollicitudin rhoncus. Integer ipsum libero, \
                 porta id velit et, egestas facilisis tellus.",
            )),
            ..MediaAttributeEntries::default()
        });
        let packets = b.encode_packets().expect("unable to encode packets for event");
        println!(
            "count: {}, len of 0: {}, packets: {:x?}",
            packets.len(),
            packets[0].len(),
            packets
        );
        assert!(packets.len() > 2);
        // each packet should be the get element attributes PDU type
        assert_eq!(packets[0][0], 0x20);
        assert_eq!(packets[1][0], 0x20);

        assert_eq!(packets[0][1], 0b01); // first packet should be a start packet.
        assert_eq!(packets[1][1], 0b10); // second packet should be a continue packet.
        assert_eq!(packets[packets.len() - 1][1], 0b11); // last packet should be a stop packet.

        for p in packets {
            assert!(p.len() <= 512);
        }
    }
}
