// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_bluetooth_avrcp as fidl_avrcp;
use packet_encoding::{Decodable, Encodable};
use std::collections::HashSet;

use crate::packets::{
    ATTRIBUTE_ID_LEN, AdvancedDecodable, CharsetId, Error, FolderType, ItemType,
    MediaAttributeEntries, MediaAttributeId, MediaType, PacketResult, PlaybackStatus, Scope,
    StatusCode, adjust_byte_size,
};

const DEFAULT_PLAYER_FEATURE_BITS: fidl_avrcp::PlayerFeatureBits =
    fidl_avrcp::PlayerFeatureBits::from_bits_truncate(
        fidl_avrcp::PlayerFeatureBits::PLAY.bits()
            | fidl_avrcp::PlayerFeatureBits::STOP.bits()
            | fidl_avrcp::PlayerFeatureBits::PAUSE.bits()
            | fidl_avrcp::PlayerFeatureBits::REWIND.bits()
            | fidl_avrcp::PlayerFeatureBits::FAST_FORWARD.bits()
            | fidl_avrcp::PlayerFeatureBits::FORWARD.bits()
            | fidl_avrcp::PlayerFeatureBits::BACKWARD.bits()
            | fidl_avrcp::PlayerFeatureBits::VENDOR_UNIQUE.bits()
            | fidl_avrcp::PlayerFeatureBits::ADVANCED_CONTROL_PLAYER.bits(),
    );

const DEFAULT_PLAYER_FEATURE_BITS_EXT: fidl_avrcp::PlayerFeatureBitsExt =
    fidl_avrcp::PlayerFeatureBitsExt::empty();

/// AVRCP 1.6.2 section 6.10.4.2 GetFolderItems
/// If `attribute_list` is None, then all attribute ids will be used. Otherwise,
/// the ids provided in the list are used.
#[derive(Debug)]
pub struct GetFolderItemsCommand {
    scope: Scope,
    start_item: u32,
    end_item: u32,
    attribute_list: Option<Vec<MediaAttributeId>>,
}

impl GetFolderItemsCommand {
    /// The smallest packet size of a GetFolderItemsCommand.
    /// 1 byte for scope, 4 for start_item, 4 for end_item, 1 for attribute_count.
    pub const MIN_PACKET_SIZE: usize = 10;

    // Create a new command for getting media players.
    pub fn new_media_player_list(start_item: u32, end_item: u32) -> Self {
        Self { scope: Scope::MediaPlayerList, start_item, end_item, attribute_list: None }
    }

    // Create a new command for getting virtual file system.
    pub fn new_virtual_file_system(
        start_item: u32,
        end_item: u32,
        attr_option: fidl_avrcp::AttributeRequestOption,
    ) -> Self {
        Self {
            scope: Scope::MediaPlayerVirtualFilesystem,
            start_item,
            end_item,
            attribute_list: Self::attr_list_from_fidl(attr_option),
        }
    }

    // Create a new command for getting now playing list.
    pub fn new_now_playing_list(
        start_item: u32,
        end_item: u32,
        attr_option: fidl_avrcp::AttributeRequestOption,
    ) -> Self {
        Self {
            scope: Scope::NowPlaying,
            start_item,
            end_item,
            attribute_list: Self::attr_list_from_fidl(attr_option),
        }
    }

    fn attr_list_from_fidl(
        attr_option: fidl_avrcp::AttributeRequestOption,
    ) -> Option<Vec<MediaAttributeId>> {
        match attr_option {
            fidl_avrcp::AttributeRequestOption::GetAll(true) => None,
            fidl_avrcp::AttributeRequestOption::GetAll(false) => Some(vec![]),
            fidl_avrcp::AttributeRequestOption::AttributeList(attr_list) => {
                Some(attr_list.iter().map(Into::into).collect())
            }
        }
    }

    /// Returns the scope associated with the command.
    pub fn scope(&self) -> Scope {
        self.scope
    }

    #[cfg(test)]
    pub fn start_item(&self) -> u32 {
        self.start_item
    }

    #[cfg(test)]
    pub fn end_item(&self) -> u32 {
        self.end_item
    }

    #[cfg(test)]
    pub fn attribute_list(&self) -> Option<&Vec<MediaAttributeId>> {
        self.attribute_list.as_ref()
    }
}

impl Decodable for GetFolderItemsCommand {
    type Error = Error;

    fn decode(buf: &[u8]) -> PacketResult<Self> {
        if buf.len() < Self::MIN_PACKET_SIZE {
            return Err(Error::InvalidMessageLength);
        }

        let scope = Scope::try_from(buf[0])?;

        let start_item = u32::from_be_bytes(buf[1..5].try_into().unwrap());
        let end_item = u32::from_be_bytes(buf[5..9].try_into().unwrap());

        if start_item > end_item {
            return Err(Error::InvalidParameter);
        }

        let attribute_count = buf[9];

        let attribute_list = if attribute_count == 0x00 {
            // All attributes requested.
            None
        } else if attribute_count == 0xFF {
            // No attributes requested.
            Some(vec![])
        } else {
            let expected_buf_length =
                ATTRIBUTE_ID_LEN * (attribute_count as usize) + Self::MIN_PACKET_SIZE;

            if buf.len() < expected_buf_length {
                return Err(Error::InvalidMessage);
            }

            let mut attributes = vec![];
            let mut chunks = buf[10..].chunks_exact(4);
            while let Some(chunk) = chunks.next() {
                // As per AVRCP 1.6 Section 26, Appendix E, `MediaAttributeId`s are
                // only the lower byte of the 4 byte representation, so we can ignore
                // the upper 3 bytes.
                attributes.push(MediaAttributeId::try_from(chunk[3])?);
            }

            Some(attributes)
        };

        Ok(Self { scope, start_item, end_item, attribute_list })
    }
}

impl Encodable for GetFolderItemsCommand {
    type Error = Error;

    fn encoded_len(&self) -> usize {
        Self::MIN_PACKET_SIZE + 4 * self.attribute_list.as_ref().map_or(0, |a| a.len())
    }

    fn encode(&self, buf: &mut [u8]) -> PacketResult<()> {
        if buf.len() < self.encoded_len() {
            return Err(Error::OutOfRange);
        }

        buf[0] = u8::from(&self.scope);
        // `self.start_item` is 4 bytes.
        buf[1..5].copy_from_slice(&self.start_item.to_be_bytes());
        // `self.end_item` is 4 bytes.
        buf[5..9].copy_from_slice(&self.end_item.to_be_bytes());

        let (num_attributes, attribute_list) = match &self.attribute_list {
            Some(x) if x.is_empty() => (0xff, vec![]), // No attributes.
            Some(x) => (u8::try_from(x.len()).map_err(|_| Error::OutOfRange)?, x.clone()),
            None => (0x00, vec![]), // All attributes, attribute ID is omitted.
        };
        buf[9] = num_attributes;

        // Traverse the attribute list in chunks of 4 bytes (u32 size).
        // Copy the converted attribute ID into the 4 byte chunk in `buf`.
        for i in 0..attribute_list.len() {
            let id: u32 = u32::from(u8::from(&attribute_list[i]));
            let start_idx = Self::MIN_PACKET_SIZE + 4 * i;
            let end_idx = Self::MIN_PACKET_SIZE + 4 * (i + 1);

            buf[start_idx..end_idx].copy_from_slice(&id.to_be_bytes());
        }

        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum BrowseableItem {
    MediaPlayer(MediaPlayerItem),
    Folder(FolderItem),
    MediaElement(MediaElementItem),
}

impl BrowseableItem {
    /// The length of the header fields of a browsable item.
    /// The fields are: ItemType (1 byte), ItemLength (2 bytes).
    /// Defined in AVRCP 1.6.2, Section 6.10.2.
    const HEADER_SIZE: usize = 3;

    fn get_item_type(&self) -> ItemType {
        match self {
            Self::MediaPlayer(_) => ItemType::MediaPlayer,
            Self::Folder(_) => ItemType::Folder,
            Self::MediaElement(_) => ItemType::MediaElement,
        }
    }

    /// Tries to downcast `BrowseableItem` to underlying `MediaPlayerItem` type.
    pub fn try_into_media_player(self) -> Result<MediaPlayerItem, Error> {
        match self {
            Self::MediaPlayer(p) => Ok(p),
            _ => Err(Error::InvalidMessage),
        }
    }
}

impl Encodable for BrowseableItem {
    type Error = Error;

    fn encoded_len(&self) -> usize {
        Self::HEADER_SIZE
            + match self {
                Self::MediaPlayer(m) => m.encoded_len(),
                Self::Folder(f) => f.encoded_len(),
                Self::MediaElement(e) => e.encoded_len(),
            }
    }

    fn encode(&self, buf: &mut [u8]) -> PacketResult<()> {
        if buf.len() < self.encoded_len() {
            return Err(Error::OutOfRange);
        }

        // Encode item type.
        buf[0] = u8::from(&self.get_item_type());
        // Encode length of item in octets, not including header.
        let item_size = self.encoded_len() - Self::HEADER_SIZE;
        buf[1..3].copy_from_slice(&(item_size as u16).to_be_bytes());
        match self {
            Self::MediaPlayer(m) => {
                m.encode(&mut buf[3..])?;
            }
            Self::Folder(f) => {
                f.encode(&mut buf[3..])?;
            }
            Self::MediaElement(e) => {
                e.encode(&mut buf[3..])?;
            }
        }
        Ok(())
    }
}

impl AdvancedDecodable for BrowseableItem {
    type Error = Error;

    fn try_decode(buf: &[u8], should_adjust: bool) -> Result<(Self, usize), Error> {
        if buf.len() < Self::HEADER_SIZE {
            return Err(Error::InvalidMessageLength);
        }

        let item_type = ItemType::try_from(buf[0])?;
        let mut item_size = u16::from_be_bytes(buf[1..3].try_into().unwrap()) as usize;
        if should_adjust {
            item_size = adjust_byte_size(item_size)?;
        }

        if buf.len() < Self::HEADER_SIZE + (item_size as usize) {
            return Err(Error::InvalidMessageLength);
        }

        let item_buf = &buf[Self::HEADER_SIZE..Self::HEADER_SIZE + item_size];
        let (item, decoded_len) = match item_type {
            ItemType::MediaPlayer => {
                let res = MediaPlayerItem::try_decode(item_buf, should_adjust)?;
                (Self::MediaPlayer(res.0), res.1)
            }
            ItemType::Folder => {
                let res = FolderItem::try_decode(item_buf, should_adjust)?;
                (Self::Folder(res.0), res.1)
            }
            ItemType::MediaElement => {
                let res = MediaElementItem::try_decode(item_buf, should_adjust)?;
                (Self::MediaElement(res.0), res.1)
            }
        };
        Ok((item, Self::HEADER_SIZE + decoded_len))
    }
}

/// The FIDL MediaPlayerItem contains a subset of fields in the AVRCP MediaPlayerItem.
/// This is because the current Fuchsia MediaPlayer does not provide information for the
/// omitted fields. Consequently, this conversion populates the missing fields with
/// static response values.
///
/// The static response values are taken from AVRCP 1.6.2, Section 25.19.
impl From<fidl_avrcp::MediaPlayerItem> for BrowseableItem {
    fn from(src: fidl_avrcp::MediaPlayerItem) -> BrowseableItem {
        // The player_id should always be provided. If not, default to the error
        // case of player_id = 0.
        let player_id = src.player_id.unwrap_or_default();
        // Audio
        let major_player_type = 0x1;
        // No sub type
        let player_sub_type = 0x0;
        // The play_status should always be provided. If not, default to the error case.
        let play_status = src.playback_status.map(|s| s.into()).unwrap_or(PlaybackStatus::Error);
        let feature_bit_mask = [
            src.feature_bits.unwrap_or(DEFAULT_PLAYER_FEATURE_BITS).bits(),
            src.feature_bits_ext.unwrap_or(DEFAULT_PLAYER_FEATURE_BITS_EXT).bits(),
        ];
        // The displayable name should always be provided. If not, default to empty.
        let name = src.displayable_name.unwrap_or_else(|| "".to_string());

        BrowseableItem::MediaPlayer(MediaPlayerItem {
            player_id,
            major_player_type,
            player_sub_type,
            play_status,
            feature_bit_mask: feature_bit_mask,
            name,
        })
    }
}

impl TryFrom<BrowseableItem> for fidl_avrcp::MediaPlayerItem {
    type Error = fidl_avrcp::BrowseControllerError;

    fn try_from(src: BrowseableItem) -> Result<Self, Self::Error> {
        match src {
            BrowseableItem::MediaPlayer(p) => Ok(p.into()),
            _ => Err(fidl_avrcp::BrowseControllerError::PacketEncoding),
        }
    }
}

impl TryFrom<BrowseableItem> for fidl_avrcp::FileSystemItem {
    type Error = fidl_avrcp::BrowseControllerError;

    fn try_from(src: BrowseableItem) -> Result<Self, Self::Error> {
        match src {
            BrowseableItem::MediaElement(e) => Ok(e.into()),
            BrowseableItem::Folder(f) => Ok(f.into()),
            _ => Err(fidl_avrcp::BrowseControllerError::PacketEncoding),
        }
    }
}

// Enum to express the level of browsing supported from a peer's available
// players.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub enum BrowseLevel {
    NotBrowsable = 1,
    OnlyBrowsableWhenAddressed = 2,
    // Player that supports browsing without `OnlyBrowsableWhenAddressed`.
    Browsable = 3,
}

impl From<&fidl_avrcp::PlayerFeatureBits> for BrowseLevel {
    fn from(feature_bits: &fidl_avrcp::PlayerFeatureBits) -> Self {
        if !feature_bits.contains(fidl_avrcp::PlayerFeatureBits::BROWSING) {
            return Self::NotBrowsable;
        }
        if feature_bits.contains(fidl_avrcp::PlayerFeatureBits::ONLY_BROWSABLE_WHEN_ADDRESSED) {
            Self::OnlyBrowsableWhenAddressed
        } else {
            Self::Browsable
        }
    }
}

/// The response parameters for a Media Player Item.
/// Defined in AVRCP 1.6.2, Section 6.10.2.1.
// TODO(https://fxbug.dev/42122480): Maybe wrap major_player_type, player_sub_type,
// and feature_bit_mask into strongly typed variables.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaPlayerItem {
    player_id: u16,
    major_player_type: u8,
    player_sub_type: u32,
    play_status: PlaybackStatus,
    feature_bit_mask: [u64; 2],
    name: String,
}

impl MediaPlayerItem {
    /// The smallest MediaPlayerItem size. Calculated by taking the number of bytes needed
    /// to represent the mandatory (fixed sized) fields. This does not include the header fields,
    /// ItemType and ItemLength. The fields are:
    /// Player ID (2 bytes), Media Player Type (1 byte), Player Sub Type (4 bytes),
    /// Play Status (1 byte), Feature Bit Mask (16 bytes), Character Set ID (2 bytes),
    /// Displayable Name Length (2 bytes).
    /// Defined in AVRCP 1.6.2, Section 6.10.2.1.
    const MIN_PACKET_SIZE: usize = 28;

    #[cfg(test)]
    pub fn new(
        player_id: u16,
        major_player_type: u8,
        player_sub_type: u32,
        play_status: PlaybackStatus,
        feature_bit_mask: [u64; 2],
        name: String,
    ) -> Self {
        Self { player_id, major_player_type, player_sub_type, play_status, feature_bit_mask, name }
    }

    pub fn player_id(&self) -> u16 {
        self.player_id
    }

    pub fn browse_level(&self) -> BrowseLevel {
        (&fidl_avrcp::PlayerFeatureBits::from_bits_truncate(self.feature_bit_mask[0])).into()
    }
}

impl From<MediaPlayerItem> for fidl_avrcp::MediaPlayerItem {
    fn from(src: MediaPlayerItem) -> Self {
        fidl_avrcp::MediaPlayerItem {
            player_id: Some(src.player_id),
            major_type: fidl_avrcp::MajorPlayerType::from_bits(src.major_player_type),
            sub_type: fidl_avrcp::PlayerSubType::from_bits(src.player_sub_type),
            playback_status: Some(src.play_status.into()),
            displayable_name: Some(src.name),
            feature_bits: Some(fidl_avrcp::PlayerFeatureBits::from_bits_truncate(
                src.feature_bit_mask[0],
            )),
            feature_bits_ext: Some(fidl_avrcp::PlayerFeatureBitsExt::from_bits_truncate(
                src.feature_bit_mask[1],
            )),
            ..Default::default()
        }
    }
}

impl Encodable for MediaPlayerItem {
    type Error = Error;

    fn encoded_len(&self) -> usize {
        // Only variable length field is `name`, which can be calculated by
        // taking the length of the array.
        Self::MIN_PACKET_SIZE + self.name.len()
    }

    fn encode(&self, buf: &mut [u8]) -> PacketResult<()> {
        if buf.len() < self.encoded_len() {
            return Err(Error::OutOfRange);
        }

        buf[0..2].copy_from_slice(&self.player_id.to_be_bytes());

        buf[2] = self.major_player_type;
        buf[3..7].copy_from_slice(&self.player_sub_type.to_be_bytes());
        buf[7] = u8::from(&self.play_status);

        buf[8..16].copy_from_slice(&self.feature_bit_mask[0].to_be_bytes());
        buf[16..24].copy_from_slice(&self.feature_bit_mask[1].to_be_bytes());

        let charset_id = u16::from(&CharsetId::Utf8);
        buf[24..26].copy_from_slice(&charset_id.to_be_bytes());

        let name_len = self.name.len();
        let name_length = u16::try_from(name_len).map_err(|_| Error::ParameterEncodingError)?;
        buf[26..28].copy_from_slice(&name_length.to_be_bytes());

        // Copy the name at the end.
        buf[28..28 + name_len].copy_from_slice(self.name.as_bytes());

        Ok(())
    }
}

impl AdvancedDecodable for MediaPlayerItem {
    type Error = Error;

    fn try_decode(buf: &[u8], should_adjust: bool) -> Result<(Self, usize), Error> {
        if buf.len() < Self::MIN_PACKET_SIZE {
            return Err(Error::InvalidMessageLength);
        }

        let player_id = u16::from_be_bytes(buf[0..2].try_into().unwrap());
        let major_player_type = buf[2];
        let player_sub_type = u32::from_be_bytes(buf[3..7].try_into().unwrap());
        let play_status = PlaybackStatus::try_from(buf[7])?;

        // These aren't actually numbers but a giant 128-bit bitfield that we interpret
        // as two 64-bit big-endian numbers.
        // Bits 69-127 are reserved (values valid up to Octet 8 Bit 4 see AVRCP v1.6.2 section 6.10.2.1 for details).
        let feature_bit_mask = [
            u64::from_be_bytes(buf[8..16].try_into().unwrap()),
            u64::from_be_bytes(buf[16..24].try_into().unwrap()),
        ];
        let is_utf8 = CharsetId::is_utf8_charset_id(&buf[24..26].try_into().unwrap());

        let mut name_len = u16::from_be_bytes(buf[26..28].try_into().unwrap()) as usize;
        if should_adjust {
            name_len = adjust_byte_size(name_len)?;
        }

        if buf.len() < Self::MIN_PACKET_SIZE + name_len {
            return Err(Error::InvalidMessageLength);
        }
        // TODO(fxdev.bug/100467): add support to appropriately convert non-utf8
        // charset ID media player name to utf8 name.
        let name = if is_utf8 {
            String::from_utf8(buf[Self::MIN_PACKET_SIZE..Self::MIN_PACKET_SIZE + name_len].to_vec())
                .or(Err(Error::ParameterEncodingError))?
        } else {
            "Media Player".to_string()
        };
        let decoded_len = Self::MIN_PACKET_SIZE + name_len;
        if decoded_len != buf.len() {
            return Err(Error::InvalidMessageLength);
        }

        Ok((
            MediaPlayerItem {
                player_id,
                major_player_type,
                player_sub_type,
                play_status,
                feature_bit_mask,
                name,
            },
            decoded_len,
        ))
    }
}

/// The response parameters for a Folder Item.
/// Defined in AVRCP 1.6.2, Section 6.10.2.2.
#[derive(Clone, Debug, PartialEq)]
pub struct FolderItem {
    folder_uid: u64,
    folder_type: FolderType,
    is_playable: bool,
    name: String,
}

impl FolderItem {
    /// The smallest FolderItem size. Calculated by taking the number of bytes needed
    /// to represent the mandatory (fixed sized) fields. This does not include the header fields,
    /// ItemType and ItemLength.
    /// Folder UID (8 bytes), Folder Type (1 byte), Is Playable (1 byte),
    /// Character Set ID (2 bytes), Displayable Name Length (2 bytes).
    /// Defined in AVRCP 1.6.2, Section 6.10.2.2.
    const MIN_PACKET_SIZE: usize = 14;
}

impl From<FolderItem> for fidl_avrcp::FileSystemItem {
    fn from(src: FolderItem) -> Self {
        Self::Folder(fidl_avrcp::FolderItem {
            folder_uid: Some(src.folder_uid),
            folder_type: Some(src.folder_type.into()),
            is_playable: Some(src.is_playable),
            displayable_name: Some(src.name),
            ..Default::default()
        })
    }
}

impl Encodable for FolderItem {
    type Error = Error;

    fn encoded_len(&self) -> usize {
        // Only variable length field is `name`, which can be calculated by
        // taking the length of the array.
        Self::MIN_PACKET_SIZE + self.name.len()
    }

    fn encode(&self, buf: &mut [u8]) -> PacketResult<()> {
        if buf.len() < self.encoded_len() {
            return Err(Error::OutOfRange);
        }

        buf[0..8].copy_from_slice(&self.folder_uid.to_be_bytes());
        buf[8] = u8::from(&self.folder_type);
        buf[9] = u8::from(self.is_playable);

        let charset_id = u16::from(&CharsetId::Utf8);
        buf[10..12].copy_from_slice(&charset_id.to_be_bytes());

        let name_len = self.name.len();
        let name_length = u16::try_from(name_len).map_err(|_| Error::ParameterEncodingError)?;
        buf[12..14].copy_from_slice(&name_length.to_be_bytes());

        // Copy the name at the end.
        buf[14..14 + name_len].copy_from_slice(self.name.as_bytes());

        Ok(())
    }
}

impl AdvancedDecodable for FolderItem {
    type Error = Error;

    fn try_decode(buf: &[u8], should_adjust: bool) -> Result<(Self, usize), Error> {
        if buf.len() < Self::MIN_PACKET_SIZE {
            return Err(Error::InvalidMessageLength);
        }

        let folder_uid = u64::from_be_bytes(buf[0..8].try_into().unwrap());
        let folder_type = FolderType::try_from(buf[8])?;
        if buf[9] > 1 {
            return Err(Error::OutOfRange);
        }
        let is_playable = buf[9] == 1;
        let is_utf8 = CharsetId::is_utf8_charset_id(&buf[10..12].try_into().unwrap());

        let mut name_len = u16::from_be_bytes(buf[12..14].try_into().unwrap()) as usize;
        if should_adjust {
            name_len = adjust_byte_size(name_len)?;
        }
        if buf.len() < Self::MIN_PACKET_SIZE + name_len {
            return Err(Error::InvalidMessageLength);
        }
        // TODO(fxdev.bug/100467): add support to appropriately convert non-utf8
        // charset ID folder name to utf8 name.
        let name = if is_utf8 {
            String::from_utf8(buf[Self::MIN_PACKET_SIZE..Self::MIN_PACKET_SIZE + name_len].to_vec())
                .or(Err(Error::ParameterEncodingError))?
        } else {
            "Folder".to_string()
        };
        let decoded_len = Self::MIN_PACKET_SIZE + name_len;
        if decoded_len != buf.len() {
            return Err(Error::InvalidMessageLength);
        }

        Ok((FolderItem { folder_uid, folder_type, is_playable, name }, buf.len()))
    }
}

/// The response parameters for a Folder Item.
/// Defined in AVRCP 1.6.2, Section 6.10.2.3.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaElementItem {
    element_uid: u64,
    media_type: MediaType,
    name: String,
    attributes: MediaAttributeEntries,
}

impl MediaElementItem {
    /// The smallest MediaElementItem size. Calculated by taking the number of bytes needed
    /// to represent the mandatory (fixed sized) fields. This does not include the header fields,
    /// ItemType and ItemLength.
    /// Media Element UID (8 bytes), Media Type (1 byte), Character Set ID (2 bytes),
    /// Displayable Name Length (2 bytes), Number of Attributes (1 byte).
    /// Defined in AVRCP 1.6.2, Section 6.10.2.3.
    const MIN_PACKET_SIZE: usize = 14;
}

impl From<MediaElementItem> for fidl_avrcp::FileSystemItem {
    fn from(src: MediaElementItem) -> Self {
        Self::MediaElement(fidl_avrcp::MediaElementItem {
            media_element_uid: Some(src.element_uid),
            media_type: Some(src.media_type.into()),
            displayable_name: Some(src.name),
            attributes: Some(src.attributes.into()),
            ..Default::default()
        })
    }
}

impl Encodable for MediaElementItem {
    type Error = Error;

    fn encoded_len(&self) -> usize {
        // Name and attributes are variable length.
        // Subtract 1 from `self.attributes.encoded_len()` since we already account for
        // Number of Attributes in `Self::MIN_PACKET_SIZE`.
        Self::MIN_PACKET_SIZE - 1 + self.name.len() + self.attributes.encoded_len()
    }

    fn encode(&self, buf: &mut [u8]) -> PacketResult<()> {
        if buf.len() < self.encoded_len() {
            return Err(Error::OutOfRange);
        }

        buf[0..8].copy_from_slice(&self.element_uid.to_be_bytes());
        buf[8] = u8::from(&self.media_type);

        let charset_id = u16::from(&CharsetId::Utf8);
        buf[9..11].copy_from_slice(&charset_id.to_be_bytes());

        let name_len = self.name.len();
        let len = u16::try_from(name_len).map_err(|_| Error::ParameterEncodingError)?;
        buf[11..13].copy_from_slice(&len.to_be_bytes());
        buf[13..13 + name_len].copy_from_slice(self.name.as_bytes());

        let next_idx = 13 + name_len; // after displayable name.
        self.attributes.encode(&mut buf[next_idx..])?;
        Ok(())
    }
}

impl AdvancedDecodable for MediaElementItem {
    type Error = Error;

    fn try_decode(buf: &[u8], should_adjust: bool) -> Result<(Self, usize), Error> {
        if buf.len() < Self::MIN_PACKET_SIZE {
            return Err(Error::InvalidMessageLength);
        }

        let element_uid = u64::from_be_bytes(buf[0..8].try_into().unwrap());
        let media_type = MediaType::try_from(buf[8])?;

        let is_utf8 = CharsetId::is_utf8_charset_id(&buf[9..11].try_into().unwrap());

        let mut name_len = u16::from_be_bytes(buf[11..13].try_into().unwrap()) as usize;
        if should_adjust {
            name_len = adjust_byte_size(name_len)?;
        }
        if buf.len() < 13 + name_len {
            return Err(Error::InvalidMessageLength);
        }

        // TODO(fxdev.bug/100467): add support to appropriately convert non-utf8
        // charset ID folder names to utf8 names.
        let name = if is_utf8 {
            String::from_utf8(buf[13..13 + name_len].to_vec())
                .or(Err(Error::ParameterEncodingError))?
        } else {
            "Media Element".to_string()
        };

        let mut next_idx = 13 + name_len;
        if buf.len() <= next_idx {
            return Err(Error::InvalidMessageLength);
        }

        let (attributes, decoded_len) =
            MediaAttributeEntries::try_decode(&buf[next_idx..], should_adjust)?;
        next_idx += decoded_len;

        Ok((MediaElementItem { element_uid, media_type, name, attributes }, next_idx))
    }
}

/// AVRCP 1.6.2 section 6.10.4.2 GetFolderItems
#[derive(Debug, PartialEq)]
pub enum GetFolderItemsResponse {
    Success(GetFolderItemsResponseParams),
    Failure(StatusCode),
}

impl GetFolderItemsResponse {
    /// The packet size of a GetFolderItemsResponse Status field (1 byte).
    const STATUS_FIELD_SIZE: usize = 1;

    /// The packet size of a GetFolderItemsResponse that indicates failure.
    /// If an error happened, Status is the only field present.
    const FAILURE_RESPONSE_SIZE: usize = Self::STATUS_FIELD_SIZE;

    /// The smallest packet size of a GetFolderItemsResponse for success status.
    /// The fields are: Status (1 byte) and and all the fields represented in
    /// `GetFolderItemsResponseParams::MIN_PACKET_SIZE`.
    const MIN_SUCCESS_RESPONSE_SIZE: usize = 5;

    pub fn new_success(uid_counter: u16, item_list: Vec<BrowseableItem>) -> Self {
        Self::Success(GetFolderItemsResponseParams { uid_counter, item_list })
    }

    #[cfg(test)]
    pub fn new_failure(status: StatusCode) -> Result<Self, Error> {
        if status == StatusCode::Success {
            return Err(Error::InvalidMessage);
        }
        Ok(Self::Failure(status))
    }
}

impl Encodable for GetFolderItemsResponse {
    type Error = Error;

    fn encoded_len(&self) -> usize {
        match self {
            Self::Failure(_) => Self::FAILURE_RESPONSE_SIZE,
            Self::Success(r) => Self::STATUS_FIELD_SIZE + r.encoded_len(),
        }
    }

    fn encode(&self, buf: &mut [u8]) -> PacketResult<()> {
        if buf.len() < self.encoded_len() {
            return Err(Error::BufferLengthOutOfRange);
        }

        buf[0] = match self {
            Self::Failure(status) => u8::from(status),
            Self::Success(_) => u8::from(&StatusCode::Success),
        };
        if let Self::Success(params) = self {
            params.encode(&mut buf[1..])?;
        }
        Ok(())
    }
}

impl Decodable for GetFolderItemsResponse {
    type Error = Error;

    fn decode(buf: &[u8]) -> PacketResult<Self> {
        // Failure response size is the smallest valid message size.
        if buf.len() < Self::FAILURE_RESPONSE_SIZE {
            return Err(Error::InvalidMessageLength);
        }

        let status = StatusCode::try_from(buf[0])?;
        if status != StatusCode::Success {
            return Ok(Self::Failure(status));
        }
        if buf.len() < Self::MIN_SUCCESS_RESPONSE_SIZE {
            return Err(Error::InvalidMessageLength);
        }
        Ok(Self::Success(GetFolderItemsResponseParams::decode(&buf[1..])?))
    }
}
/// AVRCP 1.6.2 section 6.10.4.2 GetFolderItems
/// Struct that contains all the parameters from a successful GetFolderItemsResponse.
/// Excludes the Status field since the struct itself indicates a Status of success.
#[derive(Debug, PartialEq)]
pub struct GetFolderItemsResponseParams {
    uid_counter: u16,
    item_list: Vec<BrowseableItem>,
}

impl GetFolderItemsResponseParams {
    /// The smallest packet size containing params from a successful GetFolderItemsResponse.
    /// Excludes the Status field since it is processed at `GetFolderItemsResponse`.
    /// The fields are: UID Counter (2 bytes) and Number of Items (2 bytes).
    const MIN_PACKET_SIZE: usize = 4;

    // Returns a list of browsable items from a successful GetFolderItem response.
    pub fn item_list(self) -> Vec<BrowseableItem> {
        self.item_list
    }
}

impl Encodable for GetFolderItemsResponseParams {
    type Error = Error;

    /// 5 bytes for the fixed values: `status`, `uid_counter`, and `num_items`.
    /// Each item in `item_list` has variable size, so iterate and update total size.
    /// Each item in `item_list` also contains a header, which is not part of the object.
    fn encoded_len(&self) -> usize {
        Self::MIN_PACKET_SIZE + self.item_list.iter().map(|item| item.encoded_len()).sum::<usize>()
    }

    fn encode(&self, buf: &mut [u8]) -> PacketResult<()> {
        if buf.len() < self.encoded_len() {
            return Err(Error::BufferLengthOutOfRange);
        }

        buf[0..2].copy_from_slice(&self.uid_counter.to_be_bytes());

        let num_items =
            u16::try_from(self.item_list.len()).map_err(|_| Error::ParameterEncodingError)?;

        buf[2..4].copy_from_slice(&num_items.to_be_bytes());

        let mut idx = Self::MIN_PACKET_SIZE;
        for item in self.item_list.iter() {
            item.encode(&mut buf[idx..])?;
            idx += item.encoded_len();
        }

        Ok(())
    }
}

impl Decodable for GetFolderItemsResponseParams {
    type Error = Error;

    /// First tries to decode the packet using no adjustments
    /// then if it fails tries to decode the packet using adjustments.
    fn decode(buf: &[u8]) -> core::result::Result<Self, Self::Error> {
        let res = Self::try_decode(buf, false);
        if let Ok(decoded) = res {
            return Ok(decoded.0);
        }
        Self::try_decode(buf, true).map(|decoded| decoded.0)
    }
}

impl AdvancedDecodable for GetFolderItemsResponseParams {
    type Error = Error;

    // Given a GetFolderItemsResponse message buf with supposed Success status,
    // it will try to decode the remaining response parameters.
    fn try_decode(buf: &[u8], should_adjust: bool) -> Result<(Self, usize), Error> {
        if buf.len() < Self::MIN_PACKET_SIZE {
            return Err(Error::InvalidMessageLength);
        }

        let uid_counter = u16::from_be_bytes(buf[0..2].try_into().unwrap());
        let num_items = u16::from_be_bytes(buf[2..4].try_into().unwrap());

        let mut next_idx = Self::MIN_PACKET_SIZE;
        let mut item_list = Vec::with_capacity(num_items.into());

        let mut seen_item_types = HashSet::new();

        for _processed in 0..num_items {
            if buf.len() <= next_idx {
                return Err(Error::InvalidMessageLength);
            }
            let (item, decoded_len) = BrowseableItem::try_decode(&buf[next_idx..], should_adjust)?;

            // According to AVRCP 1.6.2 section 6.10.1,
            // Media player item cannot exist with other items.
            let _ = seen_item_types.insert(item.get_item_type());
            if seen_item_types.contains(&ItemType::MediaPlayer) && seen_item_types.len() != 1 {
                return Err(Error::InvalidMessage);
            }

            next_idx += decoded_len;
            item_list.push(item);
        }

        if next_idx != buf.len() {
            return Err(Error::InvalidMessageLength);
        }
        Ok((GetFolderItemsResponseParams { uid_counter, item_list }, buf.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encoding a GetFolderItemsCommand successfully produces a byte buffer.
    #[fuchsia::test]
    fn test_get_folder_items_command_encode() {
        let cmd = GetFolderItemsCommand::new_media_player_list(1, 4);

        assert_eq!(cmd.encoded_len(), 10);
        let mut buf = vec![0; cmd.encoded_len()];
        let _ = cmd.encode(&mut buf[..]).expect("should be ok");
        assert_eq!(buf, &[0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x04, 0x00]);

        let cmd = GetFolderItemsCommand {
            scope: Scope::MediaPlayerVirtualFilesystem,
            start_item: 1,
            end_item: 4,
            attribute_list: Some(vec![MediaAttributeId::Title, MediaAttributeId::Genre]),
        };

        assert_eq!(cmd.encoded_len(), 18);
        let mut buf = vec![0; cmd.encoded_len()];
        let _ = cmd.encode(&mut buf[..]).expect("should be ok");
        assert_eq!(
            buf,
            &[
                0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x04, 0x02, 0x00, 0x00, 0x00, 0x01,
                0x00, 0x00, 0x00, 0x06
            ]
        );

        let cmd = GetFolderItemsCommand {
            scope: Scope::NowPlaying,
            start_item: 1,
            end_item: 4,
            attribute_list: Some(vec![]),
        };

        assert_eq!(cmd.encoded_len(), 10);
        let mut buf = vec![0; cmd.encoded_len()];
        let _ = cmd.encode(&mut buf[..]).expect("should be ok");
        assert_eq!(buf, &[0x03, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x04, 0xFF,]);
    }

    /// Sending expected buffer decodes successfully.
    #[fuchsia::test]
    fn test_get_folder_items_command_decode_success() {
        // `self.attribute_count` is zero, so all attributes are requested.
        let buf = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00];
        let cmd = GetFolderItemsCommand::decode(&buf[..]);
        assert!(cmd.is_ok());
        let cmd = cmd.expect("should have succeeded");
        assert_eq!(cmd.scope, Scope::MediaPlayerList);
        assert_eq!(cmd.start_item, 0);
        assert_eq!(cmd.end_item, 4);
        assert_eq!(cmd.attribute_list, None);

        // `self.attribute_count` is u8 max, so no attributes are requested.
        let buf = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0xff];
        let cmd = GetFolderItemsCommand::decode(&buf[..]);
        assert!(cmd.is_ok());
        let cmd = cmd.expect("should have succeeded");
        assert_eq!(cmd.scope, Scope::MediaPlayerList);
        assert_eq!(cmd.start_item, 0);
        assert_eq!(cmd.end_item, 4);
        assert_eq!(cmd.attribute_list, Some(vec![]));

        // Normal case.
        let buf =
            [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x01, 0x00, 0x00, 0x00, 0x01];
        let cmd = GetFolderItemsCommand::decode(&buf[..]);
        assert!(cmd.is_ok());
        let cmd = cmd.expect("should have succeeded");
        assert_eq!(cmd.scope, Scope::MediaPlayerList);
        assert_eq!(cmd.start_item, 0);
        assert_eq!(cmd.end_item, 4);
        assert_eq!(cmd.attribute_list, Some(vec![MediaAttributeId::Title]));
    }

    /// Sending payloads that are malformed and/or contain invalid parameters should be
    /// gracefully handled.
    #[fuchsia::test]
    fn test_get_folder_items_command_decode_invalid_buf() {
        // Incomplete buffer.
        let invalid_format_buf = [0x00, 0x00, 0x00, 0x01];
        let cmd = GetFolderItemsCommand::decode(&invalid_format_buf[..]);
        assert!(cmd.is_err());

        // Attribute ID length and provided attribute IDs don't match up.
        let missing_buf = [
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x06, 0x00, 0x00, 0x00, 0x02,
            0x00, 0x00, 0x00, 0x03,
        ];
        let cmd = GetFolderItemsCommand::decode(&missing_buf[..]);
        assert!(cmd.is_err());

        // Invalid MediaAttributeId provided.
        let invalid_id_buf = [
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x02, 0x00, 0x00, 0x00, 0x02,
            0x00, 0x00, 0x00, 0xa4,
        ];
        let cmd = GetFolderItemsCommand::decode(&invalid_id_buf[..]);
        assert!(cmd.is_err());
    }

    /// Tests encoding a response buffer for GetFolderItemsResponse works as intended.
    #[fuchsia::test]
    fn test_get_folder_items_response_empty_encode() {
        let response = GetFolderItemsResponse::new_failure(StatusCode::InvalidParameter)
            .expect("should have initialized");
        // 1 byte for status.
        assert_eq!(response.encoded_len(), 1);

        let mut buf = vec![0; response.encoded_len()];
        let _ = response.encode(&mut buf[..]).expect("should have succeeded");
        assert_eq!(buf, &[1]);

        // Buffer that is too small.
        let mut buf = vec![0; 0];
        let _ = response.encode(&mut buf[..]).expect_err("should have failed");

        // Invalid failure response.
        let _ = GetFolderItemsResponse::new_failure(StatusCode::Success)
            .expect_err("should have failed to initialize");
    }

    /// Tests encoding a response buffer for GetFolderItemsResponse works as intended.
    #[fuchsia::test]
    fn test_get_folder_items_response_media_player_list_encode() {
        let feature_bit_mask = [0; 2];
        let player_name = "Foobar".to_string();
        let item_list = vec![BrowseableItem::MediaPlayer(MediaPlayerItem {
            player_id: 5,
            major_player_type: 1,
            player_sub_type: 0,
            play_status: PlaybackStatus::Playing,
            feature_bit_mask,
            name: player_name,
        })];
        let response = GetFolderItemsResponse::new_success(5, item_list);

        // 5 bytes for header, 3 bytes for MediaPlayerItem header, 34 bytes for MediaPlayerItem.
        assert_eq!(response.encoded_len(), 42);

        // Normal buffer.
        let mut buf = vec![0; response.encoded_len()];
        let _ = response.encode(&mut buf[..]).expect("should have succeeded");

        // Have to split buffer check into two because Rust prevents formatting for arrays > 32 in length.
        let buf1 = [4, 0, 5, 0, 1, 1, 0, 34, 0, 5, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(buf[..24], buf1);
        let buf2 = [0, 0, 0, 0, 0, 0, 0, 0, 0, 106, 0, 6, 70, 111, 111, 98, 97, 114];
        assert_eq!(buf[24..], buf2);

        // Buffer that is too small.
        let mut buf = vec![0; 5];
        let _ = response.encode(&mut buf[..]).expect_err("should have failed");
    }

    /// Tests encoding a response buffer for GetFolderItemsResponse works as intended.
    #[fuchsia::test]
    fn test_get_folder_items_response_file_system_encode() {
        let item_list = vec![
            BrowseableItem::Folder(FolderItem {
                folder_uid: 1,
                folder_type: FolderType::Artists,
                is_playable: true,
                name: "Test".to_string(),
            }),
            BrowseableItem::MediaElement(MediaElementItem {
                element_uid: 1,
                media_type: MediaType::Video,
                name: "Test".to_string(),
                attributes: MediaAttributeEntries {
                    title: Some("1".to_string()),
                    artist_name: Some("22".to_string()),
                    ..MediaAttributeEntries::default()
                },
            }),
        ];
        let response = GetFolderItemsResponse::new_success(5, item_list);

        // 5 bytes for header, 3 bytes for FolderItem header, 18 bytes for FolderItem,
        // 3 bytes for MediaElementItem header, 37 bytes for MediaElementItem.
        assert_eq!(response.encoded_len(), 66);

        // Normal buffer.
        let mut buf = vec![0; response.encoded_len()];
        let _ = response.encode(&mut buf[..]).expect("should have succeeded");

        // Have to split buffer check into two because Rust prevents formatting for arrays > 32 in length.
        // Header.
        let buf1 = [4, 0, 5, 0, 2];
        assert_eq!(buf[..5], buf1);
        // FolderItem with header.
        let buf2 = [2, 0, 18, 0, 0, 0, 0, 0, 0, 0, 1, 3, 1, 0, 106, 0, 4, 0x54, 0x65, 0x73, 0x74];
        assert_eq!(buf[5..26], buf2);
        // MediaElementItem with header
        let buf3 = [
            3, 0, 37, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 106, 0, 4, 0x54, 0x65, 0x73, 0x74, 2, 0, 0, 0,
            1, 0, 106, 0, 1, 0x31, 0, 0, 0, 2, 0, 106, 0, 2, 0x32, 0x32,
        ];
        assert_eq!(buf[26..], buf3);

        // Buffer that is too small.
        let mut buf = vec![0; 5];
        let _ = response.encode(&mut buf[..]).expect_err("should have failed");
    }

    /// Sending expected buffer decodes successfully.
    #[fuchsia::test]
    fn test_get_folder_items_response_decode_success() {
        // With 1 media player.
        let buf = [
            4, 0, 1, 0, 1, // Media player item.
            1, 0, 32, 0, 1, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 32, 1, 0,
            106, 0, 4, 0x74, 0x65, 0x73, 0x74,
        ];
        let response = GetFolderItemsResponse::decode(&buf).expect("should have succeeded");

        match response {
            GetFolderItemsResponse::Success(resp) => {
                assert_eq!(resp.uid_counter, 1);
                assert_eq!(resp.item_list.len(), 1);
                assert_eq!(
                    resp.item_list[0],
                    BrowseableItem::MediaPlayer(MediaPlayerItem {
                        player_id: 1,
                        major_player_type: 0,
                        player_sub_type: 1,
                        play_status: PlaybackStatus::Playing,
                        feature_bit_mask: [0, 8193],
                        name: "test".to_string(),
                    })
                );
            }
            _ => panic!("should have been success response"),
        }

        // With no item.
        let buf = [4, 0, 1, 0, 0];
        let response = GetFolderItemsResponse::decode(&buf[..]).expect("should have decoded");
        match response {
            GetFolderItemsResponse::Success(resp) => {
                assert_eq!(resp.uid_counter, 1);
                assert_eq!(resp.item_list.len(), 0);
            }
            _ => panic!("should have been success response"),
        }

        // With failure response.
        let buf = [1];
        let resp = GetFolderItemsResponse::decode(&buf[..]).expect("should have decoded");
        match resp {
            GetFolderItemsResponse::Failure(status) => {
                assert_eq!(status, StatusCode::InvalidParameter);
            }
            _ => panic!("should have been failure response"),
        }
    }

    /// Sending expected buffer decodes successfully.
    #[fuchsia::test]
    fn test_get_folder_items_response_mixed_decode_success() {
        // With 1 folder and 1 media element.
        let buf = [
            4, 0, 1, 0, 2, // Folder item.
            2, 0, 18, 0, 0, 0, 0, 0, 0, 0, 1, 3, 1, 0, 106, 0, 4, 0x74, 0x65, 0x73, 0x74,
            // Media element item.
            3, 0, 29, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 106, 0, 3, 0x61, 0x62, 0x63, 1,
            // Media element's attribute.
            0, 0, 0, 1, 0, 106, 0, 4, 0x74, 0x65, 0x73, 0x74,
        ];
        let response = GetFolderItemsResponse::decode(&buf[..]).expect("should have decoded");
        match response {
            GetFolderItemsResponse::Success(resp) => {
                assert_eq!(resp.uid_counter, 1);
                assert_eq!(resp.item_list.len(), 2);
                assert_eq!(
                    resp.item_list[0],
                    BrowseableItem::Folder(FolderItem {
                        folder_uid: 1,
                        folder_type: FolderType::Artists,
                        is_playable: true,
                        name: "test".to_string(),
                    })
                );
                assert_eq!(
                    resp.item_list[1],
                    BrowseableItem::MediaElement(MediaElementItem {
                        element_uid: 1,
                        media_type: MediaType::Video,
                        name: "abc".to_string(),
                        attributes: MediaAttributeEntries {
                            title: Some("test".to_string()),
                            ..MediaAttributeEntries::default()
                        },
                    })
                );
            }
            _ => panic!("should have been success response"),
        }
    }

    /// Sending payloads that are malformed and/or contain invalid parameters should be
    /// gracefully handled.
    #[fuchsia::test]
    fn test_malformed_get_folder_items_response_decode_success() {
        // With 1 media player.
        let player_buf = [
            4, 0, 1, 0, 1,
            // Media player item.
            // Should be 32 bytes, but 34 bytes is defined as item size.
            1, 0, 34, 0, 1, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 32, 1, 0,
            106, // Should be 4 bytes, but 6 bytes is defined as displayable name length.
            0, 6, 0x74, 0x65, 0x73, 0x74,
        ];
        let response = GetFolderItemsResponse::decode(&player_buf).expect("should have succeeded");
        assert_eq!(
            response,
            GetFolderItemsResponse::Success(GetFolderItemsResponseParams {
                uid_counter: 1,
                item_list: vec![BrowseableItem::MediaPlayer(MediaPlayerItem {
                    player_id: 1,
                    major_player_type: 0,
                    player_sub_type: 1,
                    play_status: PlaybackStatus::Playing,
                    feature_bit_mask: [0, 8193],
                    name: "test".to_string(),
                }),],
            })
        );

        // With 1 folder item and 1 media element item.
        let mixed_items_buf = [
            4, 0, 1, 0, 2,
            // Folder item.
            // Should be 18 bytes, but 20 bytes is defined as item size.
            // Should be 4 bytes but 6 bytes is defined as displayable name length.
            2, 0, 20, 0, 0, 0, 0, 0, 0, 0, 1, 3, 1, 0, 106, 0, 6, 0x74, 0x65, 0x73, 0x74,
            // Media element item.
            // Should be 29 bytes, but 31 bytes is defined as item size.
            // Should be 3 bytes, but 5 bytes is defined as displayable length.
            3, 0, 31, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 106, 0, 5, 0x61, 0x62, 0x63, 1,
            // Media element item's attribute.
            // Should be 4 bytes, but 6 bytes is defined as displayable length.
            0, 0, 0, 1, 0, 106, 0, 6, 0x74, 0x65, 0x73, 0x74,
        ];
        let response =
            GetFolderItemsResponse::decode(&mixed_items_buf[..]).expect("should have decoded");
        assert_eq!(
            response,
            GetFolderItemsResponse::Success(GetFolderItemsResponseParams {
                uid_counter: 1,
                item_list: vec![
                    BrowseableItem::Folder(FolderItem {
                        folder_uid: 1,
                        folder_type: FolderType::Artists,
                        is_playable: true,
                        name: "test".to_string(),
                    }),
                    BrowseableItem::MediaElement(MediaElementItem {
                        element_uid: 1,
                        media_type: MediaType::Video,
                        name: "abc".to_string(),
                        attributes: MediaAttributeEntries {
                            title: Some("test".to_string()),
                            ..MediaAttributeEntries::default()
                        },
                    })
                ],
            })
        );
    }

    #[fuchsia::test]
    /// Sending expected buffer decodes successfully.
    fn test_get_folder_items_response_decode_invalid_buf() {
        // Incomplete buffer.
        let invalid_format_buf = [4, 0, 1, 0];
        let _ = GetFolderItemsResponse::decode(&invalid_format_buf[..])
            .expect_err("should have failed");

        // Number of items does not match with provided items list.
        let missing_buf = [
            4, 0, 1, 0, 2, // folder item begin
            2, 0, 18, 0, 0, 0, 0, 0, 0, 0, 1, 3, 1, 0, 106, 0, 4, 0x74, 0x65, 0x73, 0x74,
        ];
        let _ = GetFolderItemsResponse::decode(&missing_buf[..]).expect_err("should have failed");

        // With inconsistent message length.
        let invalid_len_buf = [
            4, 0, 1, 0, 1, // media element item begin
            3, 0, 29, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 106, 0, 3, 0x61, 0x62, 0x63, 1,
            // - media element attribute.
            0, 0, 0, 1, 0, 106, 0, 3, 0x74, 0x65, 0x73, 0x74, // extra byte
        ];
        let _ =
            GetFolderItemsResponse::decode(&invalid_len_buf[..]).expect_err("should have failed");

        // With 1 folder item and 1 media player item.
        let invalid_items_buf = [
            4, 0, 1, 0, 2, // folder item begin
            2, 0, 18, 0, 0, 0, 0, 0, 0, 0, 1, 3, 1, 0, 106, 0, 4, 0x74, 0x65, 0x73, 0x74,
            // media player item begin
            1, 0, 32, 0, 1, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 32, 1, 0,
            106, 0, 4, 0x74, 0x65, 0x73, 0x74,
        ];
        let _ =
            GetFolderItemsResponse::decode(&invalid_items_buf[..]).expect_err("should have failed");
    }

    #[fuchsia::test]
    /// Tests encoding a MediaPlayerItem succeeds.
    fn test_media_player_item_encode() {
        let player_id = 10;
        let feature_bit_mask = [0; 2];
        let player_name = "Tea".to_string();
        let item = MediaPlayerItem {
            player_id,
            major_player_type: 1,
            player_sub_type: 0,
            play_status: PlaybackStatus::Playing,
            feature_bit_mask,
            name: player_name,
        };

        assert_eq!(item.encoded_len(), 31);
        let mut buf = vec![0; item.encoded_len()];
        assert_eq!(item.encode(&mut buf[..]).map_err(|e| format!("{:?}", e)), Ok(()));
        assert_eq!(
            buf,
            &[
                0, 10, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 106, 0,
                3, 84, 101, 97
            ]
        );
    }

    // Test values as seen in AVRCP v1.6.2 Appendix D 25.19 Get Folder Items.
    const TEST_PLAYER_FEATURE_BITS: fidl_avrcp::PlayerFeatureBits =
        fidl_avrcp::PlayerFeatureBits::from_bits_truncate(
            fidl_avrcp::PlayerFeatureBits::PLAY.bits()
                | fidl_avrcp::PlayerFeatureBits::STOP.bits()
                | fidl_avrcp::PlayerFeatureBits::PAUSE.bits()
                | fidl_avrcp::PlayerFeatureBits::REWIND.bits()
                | fidl_avrcp::PlayerFeatureBits::FAST_FORWARD.bits()
                | fidl_avrcp::PlayerFeatureBits::FORWARD.bits()
                | fidl_avrcp::PlayerFeatureBits::BACKWARD.bits()
                | fidl_avrcp::PlayerFeatureBits::VENDOR_UNIQUE.bits()
                | fidl_avrcp::PlayerFeatureBits::BASIC_GROUP_NAVIGATION.bits()
                | fidl_avrcp::PlayerFeatureBits::ADVANCED_CONTROL_PLAYER.bits()
                | fidl_avrcp::PlayerFeatureBits::BROWSING.bits()
                | fidl_avrcp::PlayerFeatureBits::ADD_TO_NOW_PLAYING.bits()
                | fidl_avrcp::PlayerFeatureBits::UIDS_UNIQUE_IN_PLAYER_BROWSE_TREE.bits()
                | fidl_avrcp::PlayerFeatureBits::ONLY_BROWSABLE_WHEN_ADDRESSED.bits(),
        );
    const TEST_PLAYER_FEATURE_BITS_EXT: fidl_avrcp::PlayerFeatureBitsExt =
        fidl_avrcp::PlayerFeatureBitsExt::from_bits_truncate(
            fidl_avrcp::PlayerFeatureBitsExt::NOW_PLAYING.bits(),
        );

    #[fuchsia::test]
    /// Tests decoding a MediaPlayerItem succeeds.
    fn test_media_player_item_decode_success() {
        // With utf8 name.
        let buf = [
            0, 127, 1, 0, 0, 0, 1, 2, 0x00, 0x00, 0x00, 0x00, 0x00, 0xB7, 0x01, 0xEF, 0x02, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0, 106, 0, 11, 77, 101, 100, 105, 97, 80, 108, 97,
            121, 101, 114,
        ];
        assert_eq!(TEST_PLAYER_FEATURE_BITS.bits().to_be_bytes(), &buf[8..16]);
        assert_eq!(TEST_PLAYER_FEATURE_BITS_EXT.bits().to_be_bytes(), &buf[16..24]);

        let (item, decoded_len) =
            MediaPlayerItem::try_decode(&buf[..], false).expect("should have succeeded");
        assert_eq!(decoded_len, buf.len());
        assert_eq!(
            item,
            MediaPlayerItem {
                player_id: 127,
                major_player_type: 1,
                player_sub_type: 1,
                play_status: PlaybackStatus::Paused,
                feature_bit_mask: [
                    TEST_PLAYER_FEATURE_BITS.bits(),
                    TEST_PLAYER_FEATURE_BITS_EXT.bits()
                ],
                name: "MediaPlayer".to_string(),
            }
        );

        // Without utf8 name.
        let buf = [
            0, 1, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 32, 1, 0, 5, 0, 4,
            0x74, 0x65, 0x73, 0x74,
        ];

        let (item, decoded_len) =
            MediaPlayerItem::try_decode(&buf[..], false).expect("should have succeeded");
        assert_eq!(decoded_len, buf.len());
        assert_eq!(
            item,
            MediaPlayerItem {
                player_id: 1,
                major_player_type: 0,
                player_sub_type: 1,
                play_status: PlaybackStatus::Playing,
                feature_bit_mask: [0, 8193],
                name: "Media Player".to_string(),
            }
        );
    }

    #[fuchsia::test]
    /// Tests decoding a MediaPlayerItem succeeds.
    fn test_malformed_media_player_item_decode_success() {
        let buf = [
            0, 127, 1, 0, 0, 0, 1, 2, 0x00, 0x00, 0x00, 0x00, 0x00, 0xB7, 0x01, 0xEF, 0x02, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0, 106,
            // Should be 11 bytes, but 13 bytes is defined as displayable name length.
            0, 13, 77, 101, 100, 105, 97, 80, 108, 97, 121, 101, 114,
        ];
        assert_eq!(TEST_PLAYER_FEATURE_BITS.bits().to_be_bytes(), &buf[8..16]);
        assert_eq!(TEST_PLAYER_FEATURE_BITS_EXT.bits().to_be_bytes(), &buf[16..24]);

        let _ = MediaPlayerItem::try_decode(&buf[..], false)
            .expect_err("should have failed without adjustments");
        let (item, decoded_len) =
            MediaPlayerItem::try_decode(&buf[..], true).expect("should have succeeded");
        assert_eq!(decoded_len, buf.len());
        assert_eq!(
            item,
            MediaPlayerItem {
                player_id: 127,
                major_player_type: 1,
                player_sub_type: 1,
                play_status: PlaybackStatus::Paused,
                feature_bit_mask: [
                    TEST_PLAYER_FEATURE_BITS.bits(),
                    TEST_PLAYER_FEATURE_BITS_EXT.bits()
                ],
                name: "MediaPlayer".to_string(),
            }
        );
    }

    #[fuchsia::test]
    /// Tests decoding a MediaPlayerItem fails.
    fn test_media_player_item_decode_invalid_buf() {
        // Invalid buf that's too short.
        let invalid_buf = [0, 1, 0, 0];
        let _ =
            MediaPlayerItem::try_decode(&invalid_buf[..], false).expect_err("should have failed");
        let _ =
            MediaPlayerItem::try_decode(&invalid_buf[..], true).expect_err("should have failed");

        // Invalid buf with mismatching name length field and actual name length.
        let invalid_buf = [
            0, 1, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 32, 1, 0, 106, 0, 4,
            0x74, 0x65, 0x73,
        ];
        let _ =
            MediaPlayerItem::try_decode(&invalid_buf[..], false).expect_err("should have failed");
        let _ =
            MediaPlayerItem::try_decode(&invalid_buf[..], true).expect_err("should have failed");
    }

    #[fuchsia::test]
    /// Tests encoding a FolderItem succeeds.
    fn test_folder_item_encode() {
        let folder_uid = 1;
        let folder_name = "Test".to_string();
        let item = FolderItem {
            folder_uid,
            folder_type: FolderType::Artists,
            is_playable: true,
            name: folder_name,
        };

        //  14 bytes of min payload size + 4 bytes for folder name.
        assert_eq!(item.encoded_len(), 18);
        let mut buf = vec![0; item.encoded_len()];
        assert_eq!(item.encode(&mut buf[..]).map_err(|e| format!("{:?}", e)), Ok(()));
        assert_eq!(buf, &[0, 0, 0, 0, 0, 0, 0, 1, 3, 1, 0, 106, 0, 4, 0x54, 0x65, 0x73, 0x74,]);
    }

    #[fuchsia::test]
    /// Tests decoding a FolderItem succeeds.
    fn test_folder_item_decode_success() {
        // With utf8 name.
        let buf = [0, 0, 0, 0, 0, 0, 0, 1, 3, 1, 0, 106, 0, 4, 0x74, 0x65, 0x73, 0x74];

        let (item, decoded_len) =
            FolderItem::try_decode(&buf[..], false).expect("should have succeeded");
        assert_eq!(decoded_len, buf.len());
        assert_eq!(
            item,
            FolderItem {
                folder_uid: 1,
                folder_type: FolderType::Artists,
                is_playable: true,
                name: "test".to_string(),
            }
        );

        // Without utf8 name.
        let buf = [0, 0, 0, 0, 0, 0, 0, 1, 3, 1, 0, 5, 0, 4, 0x74, 0x65, 0x73, 0x74];

        let (item, decoded_len) =
            FolderItem::try_decode(&buf[..], false).expect("should have succeeded");
        assert_eq!(decoded_len, buf.len());
        assert_eq!(
            item,
            FolderItem {
                folder_uid: 1,
                folder_type: FolderType::Artists,
                is_playable: true,
                name: "Folder".to_string(),
            }
        );
    }

    #[fuchsia::test]
    /// Tests decoding a FolderItem succeeds.
    fn test_malformed_folder_item_decode_success() {
        let buf = [
            0, 0, 0, 0, 0, 0, 0, 1, 3, 1, 0, 106,
            // Should be 4 bytes, but 6 bytes is defined as displayable name.
            0, 6, 0x74, 0x65, 0x73, 0x74,
        ];

        let _ = FolderItem::try_decode(&buf[..], false)
            .expect_err("should have failed without adjustments");
        let (item, decoded_len) =
            FolderItem::try_decode(&buf[..], true).expect("should have succeeded");
        assert_eq!(decoded_len, buf.len());
        assert_eq!(
            item,
            FolderItem {
                folder_uid: 1,
                folder_type: FolderType::Artists,
                is_playable: true,
                name: "test".to_string(),
            }
        );
    }

    #[fuchsia::test]
    /// Tests decoding a FolderItem fails.
    fn test_folder_item_decode_invalid_buf() {
        // Invalid buf that's too short.
        let invalid_buf = [0, 1, 0, 0];
        let _ = FolderItem::try_decode(&invalid_buf[..], false).expect_err("should have failed");
        let _ = FolderItem::try_decode(&invalid_buf[..], true).expect_err("should have failed");

        // Invalid buf with mismatching name length field and actual name length.
        let invalid_buf = [0, 0, 0, 0, 0, 0, 0, 1, 3, 1, 0, 106, 0, 2, 0x74];
        let _ = FolderItem::try_decode(&invalid_buf[..], false).expect_err("should have failed");
        let _ = FolderItem::try_decode(&invalid_buf[..], true).expect_err("should have failed");
    }

    #[fuchsia::test]
    /// Tests encoding a MediaElementItem succeeds.
    fn test_media_element_item_encode() {
        let element_uid = 1;
        let element_name = "Test".to_string();
        let item = MediaElementItem {
            element_uid,
            media_type: MediaType::Video,
            name: element_name,
            attributes: MediaAttributeEntries {
                title: Some("1".to_string()),
                artist_name: Some("22".to_string()),
                ..MediaAttributeEntries::default()
            },
        };

        // 14 bytes of min payload size + 4 bytes for element name +
        // 9 byte for first attr + 10 bytes for second attr.
        assert_eq!(item.encoded_len(), 37);
        let mut buf = vec![0; item.encoded_len()];
        assert_eq!(item.encode(&mut buf[..]).map_err(|e| format!("{:?}", e)), Ok(()));
        assert_eq!(
            buf,
            &[
                0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 106, 0, 4, 0x54, 0x65, 0x73, 0x74, 2, 0, 0, 0, 1, 0,
                106, 0, 1, 0x31, 0, 0, 0, 2, 0, 106, 0, 2, 0x32, 0x32,
            ]
        );
    }

    /// Tests decoding a MediaElementItem succeeds.
    #[fuchsia::test]
    fn test_media_element_item_decode_success() {
        let no_attrs_buf = [0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 3, 0, 3, 0x61, 0x62, 0x63, 0];

        let (item, decoded_len) =
            MediaElementItem::try_decode(&no_attrs_buf[..], false).expect("should have succeeded");
        assert_eq!(decoded_len, no_attrs_buf.len());
        assert_eq!(
            item,
            MediaElementItem {
                element_uid: 1,
                media_type: MediaType::Video,
                name: "abc".to_string(),
                attributes: MediaAttributeEntries::default(),
            }
        );

        let attrs_buf = [
            0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 3, 0, 3, 0x61, 0x62, 0x63, 2, 0, 0, 0, 1, 0, 106, 0, 1,
            0x61, 0, 0, 0, 2, 0, 106, 0, 2, 0x62, 0x63,
        ];

        let (item, decoded_len) =
            MediaElementItem::try_decode(&attrs_buf[..], false).expect("should have succeeded");
        assert_eq!(decoded_len, attrs_buf.len());
        assert_eq!(
            item,
            MediaElementItem {
                element_uid: 2,
                media_type: MediaType::Audio,
                name: "abc".to_string(),
                attributes: MediaAttributeEntries {
                    title: Some("a".to_string()),
                    artist_name: Some("bc".to_string()),
                    ..MediaAttributeEntries::default()
                },
            }
        );
    }

    /// Tests decoding a MediaElementItem succeeds.
    #[fuchsia::test]
    fn test_malformed_media_element_item_decode_success() {
        let no_attrs_buf = [
            0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 3,
            // Should be 3 bytes, but 5 bytes is defined as displayable name length.
            0, 5, 0x61, 0x62, 0x63, 0,
        ];

        let _ = MediaElementItem::try_decode(&no_attrs_buf[..], false)
            .expect_err("should have failed without adjustments");
        let (item, decoded_len) =
            MediaElementItem::try_decode(&no_attrs_buf[..], true).expect("should have succeeded");
        assert_eq!(decoded_len, no_attrs_buf.len());
        assert_eq!(
            item,
            MediaElementItem {
                element_uid: 1,
                media_type: MediaType::Video,
                name: "abc".to_string(),
                attributes: MediaAttributeEntries::default(),
            }
        );

        let attrs_buf = [
            0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 3,
            // Should be 3 bytes, but 5 bytes is defined as displayable name length.
            0, 5, 0x61, 0x62, 0x63, 2,
            // Should be 1 byte, but 3 bytes is defined as attribute value length.
            0, 0, 0, 1, 0, 106, 0, 3, 0x61,
            // Should be 2 byte, but 4 bytes is defined as attribute value length.
            0, 0, 0, 2, 0, 106, 0, 4, 0x62, 0x63,
        ];

        let _ = MediaElementItem::try_decode(&no_attrs_buf[..], false)
            .expect_err("should have failed without adjustments");
        let (item, decoded_len) =
            MediaElementItem::try_decode(&attrs_buf[..], true).expect("should have succeeded");
        assert_eq!(decoded_len, attrs_buf.len());
        assert_eq!(
            item,
            MediaElementItem {
                element_uid: 2,
                media_type: MediaType::Audio,
                name: "abc".to_string(),
                attributes: MediaAttributeEntries {
                    title: Some("a".to_string()),
                    artist_name: Some("bc".to_string()),
                    ..MediaAttributeEntries::default()
                },
            }
        );
    }

    #[fuchsia::test]
    /// Tests decoding a MediaElementItem fails.
    fn test_media_element_item_decode_invalid_buf() {
        // Invalid buf that's too short.
        let invalid_buf = [0, 0, 0, 0, 0, 0, 0, 1];
        let _ =
            MediaElementItem::try_decode(&invalid_buf[..], false).expect_err("should have failed");
        let _ =
            MediaElementItem::try_decode(&invalid_buf[..], true).expect_err("should have failed");

        // Invalid buf with mismatching name length field and actual name length.
        let invalid_buf = [0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 106, 0, 2, 0x61, 0x62, 0x63, 0];
        let _ =
            MediaElementItem::try_decode(&invalid_buf[..], false).expect_err("should have failed");
        let _ =
            MediaElementItem::try_decode(&invalid_buf[..], true).expect_err("should have failed");

        // Invalid number of attribute values were provided.
        let invalid_buf = [
            0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 106, 0, 3, 0x61, 0x62, 0x63, 2, 0, 0, 0, 1, 0, 106, 0, 4,
            0x74, 0x65, 0x73, 0x74,
        ];
        let _ =
            MediaElementItem::try_decode(&invalid_buf[..], false).expect_err("should have failed");
        let _ =
            MediaElementItem::try_decode(&invalid_buf[..], true).expect_err("should have failed");
    }

    /// Tests converting from a FIDL MediaPlayerItem to a local MediaPlayerItem
    /// works as intended.
    #[fuchsia::test]
    fn test_fidl_to_media_player_item() {
        let player_id = 1;
        let item_fidl = fidl_avrcp::MediaPlayerItem {
            player_id: Some(player_id),
            playback_status: Some(fidl_avrcp::PlaybackStatus::Stopped),
            displayable_name: Some("hi".to_string()),
            ..Default::default()
        };
        let item: BrowseableItem = item_fidl.into();
        match item {
            BrowseableItem::MediaPlayer(item) => {
                assert_eq!(item.player_id, player_id);
                assert_eq!(item.major_player_type, 1);
                assert_eq!(item.player_sub_type, 0);
                assert_eq!(
                    item.feature_bit_mask,
                    [DEFAULT_PLAYER_FEATURE_BITS.bits(), DEFAULT_PLAYER_FEATURE_BITS_EXT.bits()]
                );
                assert_eq!(item.name, "hi".to_string());
            }
            _ => panic!("expected MediaPlayer item"),
        }
    }

    /// Tests converting from a FIDL MediaPlayerItem to a local MediaPlayerItem
    /// works as intended.
    #[fuchsia::test]
    fn test_media_player_item_to_fidl() {
        let buf = [
            0, 127, 1, 0, 0, 0, 1, 2, 0x00, 0x00, 0x00, 0x00, 0x00, 0xB7, 0x01, 0xEF, 0x02, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0, 106, 0, 11, 77, 101, 100, 105, 97, 80, 108, 97,
            121, 101, 114,
        ];

        assert_eq!(TEST_PLAYER_FEATURE_BITS.bits().to_be_bytes(), &buf[8..16]);
        assert_eq!(TEST_PLAYER_FEATURE_BITS_EXT.bits().to_be_bytes(), &buf[16..24]);

        let item = BrowseableItem::MediaPlayer(MediaPlayerItem {
            player_id: 127,
            major_player_type: 1,
            player_sub_type: 1,
            play_status: PlaybackStatus::Paused,
            feature_bit_mask: [
                TEST_PLAYER_FEATURE_BITS.bits(),
                TEST_PLAYER_FEATURE_BITS_EXT.bits(),
            ],
            name: "Media Player".to_string(),
        });

        let converted: fidl_avrcp::MediaPlayerItem =
            item.try_into().expect("should have converted");
        assert_eq!(
            converted,
            fidl_avrcp::MediaPlayerItem {
                player_id: Some(127),
                major_type: Some(fidl_avrcp::MajorPlayerType::AUDIO),
                sub_type: Some(fidl_avrcp::PlayerSubType::AUDIO_BOOK),
                playback_status: Some(fidl_avrcp::PlaybackStatus::Paused),
                displayable_name: Some("Media Player".to_string()),
                feature_bits: Some(TEST_PLAYER_FEATURE_BITS),
                feature_bits_ext: Some(TEST_PLAYER_FEATURE_BITS_EXT),
                ..Default::default()
            }
        );
    }

    /// Tests converting to a FIDL MediaPlayerItem from a local MediaPlayerItem
    /// works as intended.
    #[fuchsia::test]
    fn test_media_filesystem_item_to_fidl() {
        let item = BrowseableItem::MediaElement(MediaElementItem {
            element_uid: 1,
            media_type: MediaType::Audio,
            name: "test".to_string(),
            attributes: MediaAttributeEntries {
                title: Some("test".to_string()),
                ..MediaAttributeEntries::default()
            },
        });

        let converted: fidl_avrcp::FileSystemItem = item.try_into().expect("should have converted");
        assert_eq!(
            converted,
            fidl_avrcp::FileSystemItem::MediaElement(fidl_avrcp::MediaElementItem {
                media_element_uid: Some(1),
                media_type: Some(fidl_avrcp::MediaType::Audio),
                displayable_name: Some("test".to_string()),
                attributes: Some(fidl_avrcp::MediaAttributes {
                    title: Some("test".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            })
        );
    }
}
