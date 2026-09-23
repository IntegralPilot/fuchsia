// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bt_avctp::AvcCommandType;
use fidl_fuchsia_bluetooth_avrcp as fidl_avrcp;
use log::{debug, info, warn};
use packet_encoding::{Decodable, Encodable, decodable_enum};
use std::result;
use thiserror::Error;

mod browsing;
mod continuation;
mod get_capabilities;
mod get_element_attributes;
mod get_play_status;
mod inform_battery_status;
mod notification;
mod play_item;
pub mod player_application_settings;
mod rejected;
mod set_absolute_volume;

pub use self::browsing::*;
pub use self::continuation::*;
pub use self::get_capabilities::*;
pub use self::get_element_attributes::*;
pub use self::get_play_status::*;
pub use self::inform_battery_status::*;
pub use self::notification::*;
pub use self::play_item::*;
pub use self::player_application_settings::*;
pub use self::rejected::*;
pub use self::set_absolute_volume::*;

/// The error types for packet parsing.
#[derive(Error, Debug, PartialEq)]
pub enum Error {
    /// The value that was sent on the wire was out of range.
    #[error("Value was out of range")]
    InvalidMessageLength,

    /// A specific parameter ID was not understood.
    /// This specifically includes:
    /// PDU ID, Capability ID, Event ID, Player Application Setting Attribute ID, Player Application
    /// Setting Value ID, and Element Attribute ID
    #[error("Unrecognized parameter id for a message")]
    InvalidParameter,

    /// The body format was invalid.
    #[error("Failed to parse message contents")]
    InvalidMessage,

    /// A message couldn't be encoded. Passed in buffer was too short.
    #[error("Encoding buffer too small")]
    BufferLengthOutOfRange,

    /// A message couldn't be encoded. Logical error with parameters.
    #[error("Encountered an error encoding a message")]
    ParameterEncodingError,

    /// A enum value is out of expected range.
    #[error("Value is out of expected range for enum")]
    OutOfRange,

    /// AVRCP 1.6.2 section 6.15.3.
    /// Direction parameter is invalid.
    #[error("The Direction parameter is invalid")]
    InvalidDirection,

    #[doc(hidden)]
    #[error("__Nonexhaustive error should never be created.")]
    __Nonexhaustive,
}

impl From<Error> for fidl_avrcp::BrowseControllerError {
    fn from(e: Error) -> Self {
        match e {
            Error::InvalidDirection => fidl_avrcp::BrowseControllerError::InvalidDirection,
            _ => fidl_avrcp::BrowseControllerError::PacketEncoding,
        }
    }
}

pub type PacketResult<T> = result::Result<T, Error>;

decodable_enum! {
    /// Common charset IDs from the MIB Enum we may see in AVRCP. See:
    /// https://www.iana.org/assignments/character-sets/character-sets.xhtml
    /// TODO(https://fxbug.dev/42052955): we ideally would not want to return OutOfRange error
    /// but instead return a default value (i.e. 0 => Unknown).
    pub enum CharsetId<u16, Error, OutOfRange> {
        Ascii = 3,
        Iso8859_1 = 4,
        Utf8 = 106,
        Ucs2 = 1000,
        Utf16be = 1013,
        Utf16le = 1014,
        Utf16 = 1015,
    }
}

impl CharsetId {
    pub fn is_utf8(&self) -> bool {
        match &self {
            Self::Utf8 | Self::Ascii => true,
            _ => false,
        }
    }

    /// TODO(https://fxbug.dev/42052955): once we change CharsetId to be infalliable we can
    /// change or remove this method.
    pub(crate) fn is_utf8_charset_id(id_buf: &[u8; 2]) -> bool {
        let raw_val = u16::from_be_bytes(*id_buf);
        match Self::try_from(raw_val) {
            Ok(id) => id.is_utf8(),
            Err(_) => {
                warn!("Unsupported charset ID {:?}", raw_val,);
                false
            }
        }
    }
}

decodable_enum! {
    pub enum Direction<u8, Error, InvalidDirection> {
        FolderUp = 0,
        FolderDown = 1,
    }
}

/// The size, in bytes, of an attributes id.
pub const ATTRIBUTE_ID_LEN: usize = 4;

decodable_enum! {
    pub enum MediaAttributeId<u8, Error, OutOfRange> {
        Title = 0x1,
        ArtistName = 0x2,
        AlbumName = 0x3,
        TrackNumber = 0x4,
        TotalNumberOfTracks = 0x5,
        Genre = 0x6,
        PlayingTime = 0x7,
        DefaultCoverArt = 0x8,
    }
}

impl From<&fidl_avrcp::MediaAttributeId> for MediaAttributeId {
    fn from(attr_id: &fidl_avrcp::MediaAttributeId) -> Self {
        match attr_id {
            fidl_avrcp::MediaAttributeId::Title => Self::Title,
            fidl_avrcp::MediaAttributeId::ArtistName => Self::ArtistName,
            fidl_avrcp::MediaAttributeId::AlbumName => Self::AlbumName,
            fidl_avrcp::MediaAttributeId::TrackNumber => Self::TrackNumber,
            fidl_avrcp::MediaAttributeId::TotalNumberOfTracks => Self::TotalNumberOfTracks,
            fidl_avrcp::MediaAttributeId::Genre => Self::Genre,
            fidl_avrcp::MediaAttributeId::PlayingTime => Self::PlayingTime,
            fidl_avrcp::MediaAttributeId::DefaultCoverArt => Self::DefaultCoverArt,
        }
    }
}

decodable_enum! {
    pub enum PduId<u8, Error, InvalidParameter> {
        GetCapabilities = 0x10,
        ListPlayerApplicationSettingAttributes = 0x11,
        ListPlayerApplicationSettingValues = 0x12,
        GetCurrentPlayerApplicationSettingValue = 0x13,
        SetPlayerApplicationSettingValue = 0x14,
        GetPlayerApplicationSettingAttributeText = 0x15,
        GetPlayerApplicationSettingValueText = 0x16,
        InformDisplayableCharacterSet = 0x17,
        InformBatteryStatusOfCT = 0x18,
        GetElementAttributes = 0x20,
        GetPlayStatus = 0x30,
        RegisterNotification = 0x31,
        RequestContinuingResponse = 0x40,
        AbortContinuingResponse = 0x41,
        SetAbsoluteVolume = 0x50,
        SetAddressedPlayer = 0x60,
        SetBrowsedPlayer = 0x70,
        GetFolderItems = 0x71,
        ChangePath = 0x72,
        GetItemAttributes = 0x73,
        PlayItem = 0x74,
        GetTotalNumberOfItems = 0x75,
        AddToNowPlaying = 0x90,
        GeneralReject = 0xa0,
    }
}

decodable_enum! {
    pub enum PacketType<u8, Error, OutOfRange> {
        Single = 0b00,
        Start = 0b01,
        Continue = 0b10,
        Stop = 0b11,
    }
}

decodable_enum! {
    pub enum StatusCode<u8, Error, OutOfRange> {
        InvalidCommand = 0x00,
        InvalidParameter = 0x01,
        ParameterContentError = 0x02,
        InternalError = 0x03,
        Success = 0x04,
        UidChanged = 0x05,
        InvalidDirection = 0x07,
        NonDirectory = 0x08,
        DoesNotExist = 0x09,
        InvalidScope = 0x0a,
        RangeOutOfBounds = 0x0b,
        ItemNotPlayable = 0x0c,
        MediaInUse = 0x0d,
        InvalidPlayerId = 0x11,
        PlayerNotBrowsable = 0x12,
        PlayerNotAddressed = 0x13,
        NoValidSearchResults = 0x14,
        NoAvailablePlayers = 0x15,
        AddressedPlayerChanged = 0x16,
    }
}

decodable_enum! {
    pub enum ItemType<u8, Error, OutOfRange> {
        MediaPlayer = 0x01,
        Folder = 0x02,
        MediaElement = 0x03,
    }
}

decodable_enum! {
    pub enum FolderType<u8, Error, OutOfRange> {
        Mixed = 0x00,
        Titles = 0x01,
        Albums = 0x02,
        Artists = 0x03,
        Genres = 0x04,
        Playlists = 0x05,
        Years = 0x06,
    }
}

impl From<FolderType> for fidl_avrcp::FolderType {
    fn from(t: FolderType) -> Self {
        match t {
            FolderType::Mixed => Self::Mixed,
            FolderType::Titles => Self::Titles,
            FolderType::Albums => Self::Albums,
            FolderType::Artists => Self::Artists,
            FolderType::Genres => Self::Genres,
            FolderType::Playlists => Self::Playlists,
            FolderType::Years => Self::Years,
        }
    }
}

decodable_enum! {
    pub enum MediaType<u8, Error, OutOfRange> {
        Audio = 0x00,
        Video = 0x01,
    }
}

impl From<MediaType> for fidl_avrcp::MediaType {
    fn from(t: MediaType) -> Self {
        match t {
            MediaType::Audio => Self::Audio,
            MediaType::Video => Self::Video,
        }
    }
}

impl From<fidl_avrcp::TargetAvcError> for StatusCode {
    fn from(src: fidl_avrcp::TargetAvcError) -> StatusCode {
        match src {
            fidl_avrcp::TargetAvcError::RejectedInvalidCommand => StatusCode::InvalidCommand,
            fidl_avrcp::TargetAvcError::RejectedInvalidParameter => StatusCode::InvalidParameter,
            fidl_avrcp::TargetAvcError::RejectedParameterContentError => {
                StatusCode::ParameterContentError
            }
            fidl_avrcp::TargetAvcError::RejectedInternalError => StatusCode::InternalError,
            fidl_avrcp::TargetAvcError::RejectedUidChanged => StatusCode::UidChanged,
            fidl_avrcp::TargetAvcError::RejectedInvalidPlayerId => StatusCode::InvalidPlayerId,
            fidl_avrcp::TargetAvcError::RejectedNoAvailablePlayers => {
                StatusCode::NoAvailablePlayers
            }
            fidl_avrcp::TargetAvcError::RejectedAddressedPlayerChanged => {
                StatusCode::AddressedPlayerChanged
            }
        }
    }
}

impl From<StatusCode> for fidl_avrcp::BrowseControllerError {
    fn from(status: StatusCode) -> Self {
        match status {
            StatusCode::Success => {
                panic!("cannot convert StatusCode::Success to BrowseControllerError")
            }
            StatusCode::UidChanged => fidl_avrcp::BrowseControllerError::UidChanged,
            StatusCode::InvalidDirection => fidl_avrcp::BrowseControllerError::InvalidDirection,
            StatusCode::NonDirectory | StatusCode::DoesNotExist | StatusCode::InvalidPlayerId => {
                fidl_avrcp::BrowseControllerError::InvalidId
            }
            StatusCode::InvalidScope => fidl_avrcp::BrowseControllerError::InvalidScope,
            StatusCode::RangeOutOfBounds => fidl_avrcp::BrowseControllerError::RangeOutOfBounds,
            StatusCode::ItemNotPlayable => fidl_avrcp::BrowseControllerError::ItemNotPlayable,
            StatusCode::MediaInUse => fidl_avrcp::BrowseControllerError::MediaInUse,
            StatusCode::PlayerNotBrowsable => fidl_avrcp::BrowseControllerError::PlayerNotBrowsable,
            StatusCode::PlayerNotAddressed => fidl_avrcp::BrowseControllerError::PlayerNotAddressed,
            StatusCode::NoValidSearchResults => fidl_avrcp::BrowseControllerError::NoValidResults,
            StatusCode::NoAvailablePlayers => fidl_avrcp::BrowseControllerError::NoAvailablePlayers,
            _ => fidl_avrcp::BrowseControllerError::PacketEncoding,
        }
    }
}

// Shared by get_play_status and notification
decodable_enum! {
    pub enum PlaybackStatus<u8, Error, OutOfRange> {
        Stopped = 0x00,
        Playing = 0x01,
        Paused = 0x02,
        FwdSeek = 0x03,
        RevSeek = 0x04,
        Error = 0xff,
    }
}

impl From<fidl_avrcp::PlaybackStatus> for PlaybackStatus {
    fn from(src: fidl_avrcp::PlaybackStatus) -> PlaybackStatus {
        match src {
            fidl_avrcp::PlaybackStatus::Stopped => PlaybackStatus::Stopped,
            fidl_avrcp::PlaybackStatus::Playing => PlaybackStatus::Playing,
            fidl_avrcp::PlaybackStatus::Paused => PlaybackStatus::Paused,
            fidl_avrcp::PlaybackStatus::FwdSeek => PlaybackStatus::FwdSeek,
            fidl_avrcp::PlaybackStatus::RevSeek => PlaybackStatus::RevSeek,
            fidl_avrcp::PlaybackStatus::Error => PlaybackStatus::Error,
        }
    }
}

impl From<PlaybackStatus> for fidl_avrcp::PlaybackStatus {
    fn from(src: PlaybackStatus) -> fidl_avrcp::PlaybackStatus {
        match src {
            PlaybackStatus::Stopped => fidl_avrcp::PlaybackStatus::Stopped,
            PlaybackStatus::Playing => fidl_avrcp::PlaybackStatus::Playing,
            PlaybackStatus::Paused => fidl_avrcp::PlaybackStatus::Paused,
            PlaybackStatus::FwdSeek => fidl_avrcp::PlaybackStatus::FwdSeek,
            PlaybackStatus::RevSeek => fidl_avrcp::PlaybackStatus::RevSeek,
            PlaybackStatus::Error => fidl_avrcp::PlaybackStatus::Error,
        }
    }
}

/// The preamble at the start of all vendor dependent commands, responses, and rejections.
pub struct VendorDependentPreamble {
    pub pdu_id: u8,
    pub packet_type: PacketType,
    pub parameter_length: u16,
}

impl VendorDependentPreamble {
    pub fn new(
        pdu_id: u8,
        packet_type: PacketType,
        parameter_length: u16,
    ) -> VendorDependentPreamble {
        VendorDependentPreamble { pdu_id, packet_type, parameter_length }
    }

    pub fn new_single(pdu_id: u8, parameter_length: u16) -> VendorDependentPreamble {
        Self::new(pdu_id, PacketType::Single, parameter_length)
    }

    pub fn packet_type(&self) -> PacketType {
        self.packet_type
    }
}

impl Decodable for VendorDependentPreamble {
    type Error = Error;

    fn decode(buf: &[u8]) -> PacketResult<Self> {
        if buf.len() < 4 {
            return Err(Error::InvalidMessage);
        }
        Ok(Self {
            pdu_id: buf[0],
            packet_type: PacketType::try_from(buf[1])?,
            parameter_length: ((buf[2] as u16) << 8) | buf[3] as u16,
        })
    }
}

impl Encodable for VendorDependentPreamble {
    type Error = Error;

    fn encoded_len(&self) -> usize {
        4
    }

    fn encode(&self, buf: &mut [u8]) -> PacketResult<()> {
        buf[0] = self.pdu_id;
        buf[1] = u8::from(&self.packet_type);
        buf[2] = (self.parameter_length >> 8) as u8;
        buf[3] = (self.parameter_length & 0xff) as u8;
        Ok(())
    }
}

const AVC_PAYLOAD_SIZE: usize = 508; // 512 - 4 byte preamble

pub trait VendorDependentRawPdu {
    /// Protocol Data Unit type.
    fn raw_pdu_id(&self) -> u8;
}

pub trait VendorDependentPdu {
    fn pdu_id(&self) -> PduId;
}

impl<T: VendorDependentPdu> VendorDependentRawPdu for T {
    fn raw_pdu_id(&self) -> u8 {
        u8::from(&self.pdu_id())
    }
}

pub trait PacketEncodable {
    /// Encode packet for single command/response.
    fn encode_packet(&self) -> Result<Vec<u8>, Error>;

    /// Encode packet(s) and split at AVC 512 byte limit.
    /// For use with AVC.
    fn encode_packets(&self) -> Result<Vec<Vec<u8>>, Error>;
}

/// Provides methods to encode one or more vendor dependent packets with their preambles.
impl<T: VendorDependentRawPdu + Encodable<Error = Error>> PacketEncodable for T {
    // This default trait impl is tested in rejected.rs.
    /// Encode packet for single command/response.
    fn encode_packet(&self) -> Result<Vec<u8>, Error> {
        let len = self.encoded_len();
        let preamble = VendorDependentPreamble::new_single(self.raw_pdu_id(), len as u16);
        let prelen = preamble.encoded_len();
        let mut buf = vec![0; len + prelen];
        preamble.encode(&mut buf[..])?;
        self.encode(&mut buf[prelen..])?;
        Ok(buf)
    }

    // This default trait impl is tested in get_element_attributes.rs.
    fn encode_packets(&self) -> Result<Vec<Vec<u8>>, Error> {
        let mut buf = vec![0; self.encoded_len()];
        self.encode(&mut buf[..])?;

        let mut payloads = vec![];
        let mut len_remaining = self.encoded_len();
        let mut packet_type =
            if len_remaining > AVC_PAYLOAD_SIZE { PacketType::Start } else { PacketType::Single };
        let mut offset = 0;

        loop {
            // length - preamble size
            let current_len =
                if len_remaining > AVC_PAYLOAD_SIZE { AVC_PAYLOAD_SIZE } else { len_remaining };
            let preamble =
                VendorDependentPreamble::new(self.raw_pdu_id(), packet_type, current_len as u16);

            let mut payload_buf = vec![0; preamble.encoded_len()];
            preamble.encode(&mut payload_buf[..])?;
            payload_buf.extend_from_slice(&buf[offset..current_len + offset]);
            payloads.push(payload_buf);

            len_remaining -= current_len;
            offset += current_len;
            if len_remaining == 0 {
                break;
            } else if len_remaining <= AVC_PAYLOAD_SIZE {
                packet_type = PacketType::Stop;
            } else {
                packet_type = PacketType::Continue;
            }
        }
        Ok(payloads)
    }
}

/// Specifies the AVC command type for this packet. Used only on Command packet and not
/// response packet encoders.
pub trait VendorCommand: VendorDependentPdu {
    /// Command type.
    fn command_type(&self) -> AvcCommandType;
}

/// For sending raw pre-assembled packets, typically as part of test packets. No decoder for this
/// packet type.
pub struct RawVendorDependentPacket {
    pdu_id: PduId,
    payload: Vec<u8>,
}

impl RawVendorDependentPacket {
    pub fn new(pdu_id: PduId, payload: &[u8]) -> Self {
        Self { pdu_id, payload: payload.to_vec() }
    }
}

impl Encodable for RawVendorDependentPacket {
    type Error = Error;

    fn encoded_len(&self) -> usize {
        self.payload.len()
    }

    fn encode(&self, buf: &mut [u8]) -> PacketResult<()> {
        if buf.len() != self.payload.len() {
            return Err(Error::InvalidMessageLength);
        }

        buf.copy_from_slice(&self.payload[..]);
        Ok(())
    }
}

/// Packet PDU ID for vendor dependent packet encoding.
impl VendorDependentPdu for RawVendorDependentPacket {
    fn pdu_id(&self) -> PduId {
        self.pdu_id
    }
}

// TODO(https://fxbug.dev/42117416): Specify the command type with the REPL when sending raw packets.
// For now, default to Control.
/// Specifies the AVC command type for this AVC command packet
impl VendorCommand for RawVendorDependentPacket {
    fn command_type(&self) -> AvcCommandType {
        AvcCommandType::Control
    }
}

/// A decodable type that tries best effort to decode a packet to a desirable type.
/// Might contain logic that assumes/expects some trivial mistakes in the raw packet.
/// Therefore, after decoding, it would return the actual number of bytes that were used to
/// decode the raw data into the desired struct.
pub trait AdvancedDecodable: ::core::marker::Sized {
    type Error;

    // Attempts to decode a byte buffer into a well known packet type.
    // The decoding logic can attempt to correct malformed packets in the process
    // based on the `should_adjust` flag.
    // Callers of this method should always attempt to decode with `should_adjust = false` first.
    fn try_decode(
        buf: &[u8],
        should_adjust: bool,
    ) -> ::core::result::Result<(Self, usize), Self::Error>;
}

/// Some browse command's response messages have fields such as Item Size where the
/// byte size of an item is defined. Item Size field size does not include the size of the
/// Item Size field itself (2 bytes).
/// Some peers include the Item Size field's size as part of the size value.
/// We should subtract it so that the correct Item Size value is used for decoding.
fn adjust_byte_size(val: usize) -> Result<usize, Error> {
    val.checked_sub(2).ok_or(Error::InvalidMessageLength)
}

#[cfg(test)]
#[track_caller]
pub fn decode_avc_vendor_command(command: &bt_avctp::AvcCommand) -> Result<(PduId, &[u8]), Error> {
    let packet_body = command.body();
    let preamble = VendorDependentPreamble::decode(packet_body)?;
    let body = &packet_body[preamble.encoded_len()..];
    let pdu_id = PduId::try_from(preamble.pdu_id)?;
    Ok((pdu_id, body))
}

/// Mirror version of AVRCP 1.6.2 section 26 Appendix E: list of list of media attributes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MediaAttributeEntries {
    pub title: Option<String>,
    pub artist_name: Option<String>,
    pub album_name: Option<String>,
    pub track_number: Option<String>,
    pub total_number_of_tracks: Option<String>,
    pub genre: Option<String>,
    pub playing_time: Option<String>,
    // Handle to image encoded as a string to retrieve cover art image using BIP over OBEX protocol.
    pub default_cover_art: Option<String>,
}

impl MediaAttributeEntries {
    const NUM_ATTRIBUTES_LEN: usize = 1;
    const CHARSET_ID_LEN: usize = 2;
    const VALUE_LENGTH_LEN: usize = 2;
    const ATTRIBUTE_HEADER_LEN: usize =
        ATTRIBUTE_ID_LEN + Self::CHARSET_ID_LEN + Self::VALUE_LENGTH_LEN;
    const UNKNOWN_VALUE_PLACEHOLDER: &'static str = "Unknown Value";

    fn all_fields(&self) -> Vec<(MediaAttributeId, &Option<String>)> {
        vec![
            (MediaAttributeId::Title, &self.title),
            (MediaAttributeId::ArtistName, &self.artist_name),
            (MediaAttributeId::AlbumName, &self.album_name),
            (MediaAttributeId::TrackNumber, &self.track_number),
            (MediaAttributeId::TotalNumberOfTracks, &self.total_number_of_tracks),
            (MediaAttributeId::Genre, &self.genre),
            (MediaAttributeId::PlayingTime, &self.playing_time),
            (MediaAttributeId::DefaultCoverArt, &self.default_cover_art),
        ]
    }

    fn add_attribute(&mut self, attribute_id: MediaAttributeId, value: String) {
        let prev: Option<String>;
        match attribute_id {
            MediaAttributeId::Title => prev = self.title.replace(value),
            MediaAttributeId::ArtistName => prev = self.artist_name.replace(value),
            MediaAttributeId::AlbumName => prev = self.album_name.replace(value),
            MediaAttributeId::TrackNumber => prev = self.track_number.replace(value),
            MediaAttributeId::TotalNumberOfTracks => {
                prev = self.total_number_of_tracks.replace(value)
            }
            MediaAttributeId::Genre => prev = self.genre.replace(value),
            MediaAttributeId::PlayingTime => prev = self.playing_time.replace(value),
            MediaAttributeId::DefaultCoverArt => prev = self.default_cover_art.replace(value),
        }
        if let Some(prev_value) = prev {
            warn!(
                "Replaced value for attribute ID {attribute_id:?}. Previous value was \"{prev_value:?}\""
            );
        }
    }

    fn num_attributes(&self) -> u8 {
        self.all_fields().iter().filter(|f| f.1.is_some()).count() as u8
    }

    /// Size of one attribute value entry.
    fn entry_size(os: &Option<String>) -> usize {
        os.as_ref().map_or(0, |s| Self::ATTRIBUTE_HEADER_LEN + s.len())
    }

    /// Writes to the buffer and returns how many bytes were written.
    /// If at any point, the write was not successful, returns an error.
    /// The method can fail with a partial write to the buffer.
    fn write(
        attribute_id: MediaAttributeId,
        value: &String,
        buf: &mut [u8],
    ) -> PacketResult<usize> {
        let value_len = value.len();
        if buf.len() < Self::ATTRIBUTE_HEADER_LEN + value_len {
            return Err(Error::BufferLengthOutOfRange);
        }

        let attr_id = u8::from(&attribute_id) as u32;
        buf[0..4].copy_from_slice(&attr_id.to_be_bytes());
        let charset_id = u16::from(&CharsetId::Utf8);
        buf[4..6].copy_from_slice(&charset_id.to_be_bytes());
        if value_len > u16::MAX.into() {
            return Err(Error::ParameterEncodingError);
        }
        buf[6..Self::ATTRIBUTE_HEADER_LEN].copy_from_slice(&(value_len as u16).to_be_bytes());
        buf[Self::ATTRIBUTE_HEADER_LEN..Self::ATTRIBUTE_HEADER_LEN + value_len]
            .copy_from_slice(value.as_bytes());

        Ok(Self::ATTRIBUTE_HEADER_LEN + value_len)
    }
}

impl Encodable for MediaAttributeEntries {
    type Error = Error;

    /// Returns the number of attributes field size plus the sum of sizes for all attribute value entries.
    fn encoded_len(&self) -> usize {
        Self::NUM_ATTRIBUTES_LEN
            + self.all_fields().iter().fold(0, |acc, field| acc + Self::entry_size(field.1))
    }

    /// Encodes number of attributes and all the attribute values.
    fn encode(&self, buf: &mut [u8]) -> PacketResult<()> {
        if buf.len() < self.encoded_len() {
            return Err(Error::BufferLengthOutOfRange);
        }

        buf[0] = self.num_attributes();
        let mut next_idx = 1;
        self.all_fields().iter().filter(|f| f.1.is_some()).try_for_each(|v| {
            let written = Self::write(v.0, v.1.as_ref().unwrap(), &mut buf[next_idx..])?;
            next_idx += written;
            Ok(())
        })?;
        Ok(())
    }
}

impl AdvancedDecodable for MediaAttributeEntries {
    type Error = Error;

    /// Given the buffer of number of attributes and list of attribute value entry, decodes into
    /// `MediaAttributeEntries` struct.
    fn try_decode(buf: &[u8], should_adjust: bool) -> Result<(Self, usize), Error> {
        if buf.len() == 0 {
            return Err(Error::InvalidMessageLength);
        }

        let num_attrs = buf[0];

        // Process all the attributes.
        let mut next_idx = 1;
        let mut attributes = MediaAttributeEntries::default();
        for _processed in 0..num_attrs {
            if buf.len() < next_idx + Self::ATTRIBUTE_HEADER_LEN {
                return Err(Error::InvalidMessageLength);
            }

            let attr_id = u32::from_be_bytes(buf[next_idx..next_idx + 4].try_into().unwrap());
            // AVRCP spec 1.6.2 Appendix E
            // Values 0x9-0xFFFFFFFF are reserved.
            let attr_id = u8::try_from(attr_id).map_err(|_| Error::InvalidMessage)?;
            let attribute_id_or = MediaAttributeId::try_from(attr_id);
            next_idx += 4;

            // TODO(fxdev.bug/100467): add support to appropriately convert non-utf8
            // charset ID value to utf8.
            let is_utf8 =
                CharsetId::is_utf8_charset_id(&buf[next_idx..next_idx + 2].try_into().unwrap());
            next_idx += 2;

            let mut val_len =
                u16::from_be_bytes(buf[next_idx..next_idx + 2].try_into().unwrap()) as usize;
            if should_adjust {
                val_len = adjust_byte_size(val_len)?;
            }
            next_idx += 2;

            // Check that we have enough message length.
            if buf.len() < (next_idx + val_len) {
                debug!(
                    "Ignoring last attribute, not enough message ({} < {}): {:?}",
                    buf.len() - next_idx,
                    val_len,
                    attribute_id_or
                );
                next_idx = buf.len();
                break;
            }
            // TODO(fxdev.bug/100467): add support to appropriately convert non-utf8
            // charset ID folder names to utf8 names.
            let value = if is_utf8 {
                String::from_utf8(buf[next_idx..next_idx + val_len].to_vec())
                    .or(Err(Error::ParameterEncodingError))?
            } else {
                Self::UNKNOWN_VALUE_PLACEHOLDER.to_string()
            };
            if let Ok(attribute_id) = attribute_id_or {
                attributes.add_attribute(attribute_id, value);
            } else {
                warn!("Ignoring attribute since attribute id {attr_id:?} is unrecognized");
            }
            next_idx += val_len;
        }
        Ok((attributes, next_idx))
    }
}

impl From<MediaAttributeEntries> for fidl_avrcp::MediaAttributes {
    fn from(src: MediaAttributeEntries) -> Self {
        if let Some(_value) = src.default_cover_art {
            info!("Default cover art is ignored since it's not supported.");
        }
        Self {
            title: src.title,
            artist_name: src.artist_name,
            album_name: src.album_name,
            total_number_of_tracks: src.total_number_of_tracks,
            genre: src.genre,
            playing_time: src.playing_time,
            ..Default::default()
        }
    }
}

impl From<fidl_avrcp::MediaAttributes> for MediaAttributeEntries {
    fn from(src: fidl_avrcp::MediaAttributes) -> Self {
        Self {
            title: src.title.clone(),
            artist_name: src.artist_name.clone(),
            album_name: src.album_name.clone(),
            total_number_of_tracks: src.total_number_of_tracks.clone(),
            genre: src.genre.clone(),
            playing_time: src.playing_time,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl_fuchsia_bluetooth_avrcp as fidl;

    #[fuchsia::test]
    fn playback_status_to_fidl() {
        let status = PlaybackStatus::Playing;
        let expected = fidl::PlaybackStatus::Playing;
        assert_eq!(expected, status.into());
    }

    #[fuchsia::test]
    fn media_attribute_entries_encode_success() {
        let a = MediaAttributeEntries::default();
        assert_eq!(1, a.encoded_len());
        let mut buf = vec![0; a.encoded_len()];
        let _ = a.encode(&mut buf[..]).expect("should be ok");
        assert_eq!(buf, &[0x00]);

        let a = MediaAttributeEntries {
            title: Some("test".to_string()),
            artist_name: Some("foo".to_string()),
            playing_time: Some("1:23".to_string()),
            ..Default::default()
        };
        assert_eq!(1 + 8 + 4 + 8 + 3 + 8 + 4, a.encoded_len());
        let mut buf = vec![0; a.encoded_len()];
        let _ = a.encode(&mut buf[..]).expect("should be ok");
        assert_eq!(
            buf,
            &[
                0x03, 0x00, 0x00, 0x00, 0x01, 0x00, 0x6A, 0x00, 0x04, 't' as u8, 'e' as u8,
                's' as u8, 't' as u8, 0x00, 0x00, 0x00, 0x02, 0x00, 0x6A, 0x00, 0x03, 'f' as u8,
                'o' as u8, 'o' as u8, 0x00, 0x00, 0x00, 0x07, 0x00, 0x6A, 0x00, 0x04, '1' as u8,
                ':' as u8, '2' as u8, '3' as u8,
            ]
        );
    }

    #[fuchsia::test]
    fn media_attribute_entries_encode_fail() {
        // Insufficient buffer.
        let a = MediaAttributeEntries::default();
        let mut buf = vec![0; a.encoded_len() - 1];
        let _ = a.encode(&mut buf[..]).expect_err("should have failed");

        // Value too long.
        let a = MediaAttributeEntries {
            title: Some("1".to_string().repeat(65_536)),
            ..Default::default()
        };
        let mut buf = vec![0; a.encoded_len()];
        let _ = a.encode(&mut buf[..]).expect_err("should have failed");
    }

    #[fuchsia::test]
    fn get_item_attributes_command_decode_success() {
        // 0 entries.
        let buf = vec![0x00];
        let (a, decoded_len) =
            MediaAttributeEntries::try_decode(&buf[..], false).expect("should succeed");
        assert_eq!(buf.len(), decoded_len);
        assert_eq!(MediaAttributeEntries::default(), a);

        // Multiple entries with utf-8 and non utf-8 values.
        let buf = vec![
            0x02, 0x00, 0x00, 0x00, 0x01, 0x00, 0x6A, 0x00, 0x04, 't' as u8, 'e' as u8, 's' as u8,
            't' as u8, 0x00, 0x00, 0x00, 0x02, 0x00, 0x88, 0x00, 0x03, 0x00, 0x01, 0x02,
        ];
        let (a, decoded_len) =
            MediaAttributeEntries::try_decode(&buf[..], false).expect("should succeed");
        assert_eq!(buf.len(), decoded_len);
        assert_eq!(
            MediaAttributeEntries {
                title: Some("test".to_string()),
                artist_name: Some(MediaAttributeEntries::UNKNOWN_VALUE_PLACEHOLDER.to_string()),
                ..Default::default()
            },
            a
        );

        // With invalid attribute ID, 0x1F.
        // Should decode correctly since attributes with unrecognized ID are ignored.
        let buf = vec![
            0x02, 0x00, 0x00, 0x00, 0x01, 0x00, 0x6A, 0x00, 0x04, 't' as u8, 'e' as u8, 's' as u8,
            't' as u8, 0x00, 0x00, 0x00, 0x1F, 0x00, 0x6A, 0x00, 0x01, 'a' as u8,
        ];
        let (a, decoded_len) =
            MediaAttributeEntries::try_decode(&buf[..], false).expect("should succeed");
        assert_eq!(buf.len(), decoded_len);
        assert_eq!(
            MediaAttributeEntries { title: Some("test".to_string()), ..Default::default() },
            a
        );

        // Buffer with extra bytes.
        // Should decode correctly since MediaAttributeEntries may be part of a bigger message.
        let buf = vec![
            0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x6A, 0x00, 0x04, 't' as u8, 'e' as u8, 's' as u8,
            't' as u8, 0x01, 0x02, 0x03, 0x04, // Some extra bytes.
        ];
        let (a, decoded_len) =
            MediaAttributeEntries::try_decode(&buf[..], false).expect("should succeed");
        assert_eq!(buf.len() - 4, decoded_len);
        assert_eq!(
            MediaAttributeEntries { title: Some("test".to_string()), ..Default::default() },
            a
        );

        // Entry with mis-calculated value length.
        let buf = vec![
            0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x6A, 0x00, 0x06, 't' as u8, 'e' as u8, 's' as u8,
            't' as u8,
        ];
        let (a, _decoded_len) =
            MediaAttributeEntries::try_decode(&buf[..], false).expect("partial success");
        // Final entry with the wrong length is removed.
        assert_eq!(MediaAttributeEntries { ..Default::default() }, a);
        let (a, decoded_len) =
            MediaAttributeEntries::try_decode(&buf[..], true).expect("should succeed");
        assert_eq!(buf.len(), decoded_len);
        // Entry with the wrong length is retrieved if it is the only entry.
        assert_eq!(
            MediaAttributeEntries { title: Some("test".to_string()), ..Default::default() },
            a
        );
    }

    #[fuchsia::test]
    fn get_item_attributes_command_decode_fail() {
        // Invalid number of attributes.
        let buf = vec![0x02, 0x00, 0x00, 0x00, 0x01, 0x00, 0x6A, 0x00, 0x01, 'a' as u8];
        let _ = MediaAttributeEntries::try_decode(&buf[..], false).expect_err("should have failed");
        let _ = MediaAttributeEntries::try_decode(&buf[..], true).expect_err("should have failed");
    }

    #[fuchsia::test]
    fn playback_status_from_fidl() {
        let status = fidl::PlaybackStatus::Stopped;
        let expected = PlaybackStatus::Stopped;
        assert_eq!(expected, status.into());
    }

    #[fuchsia::test]
    fn media_attribute_entries_to_fidl() {
        // Emtry attributes.
        let attributes = MediaAttributeEntries::default();
        let expected = fidl::MediaAttributes::default();
        assert_eq!(expected, attributes.into());

        // Attributes with some fields.
        let attributes = MediaAttributeEntries {
            title: Some("avrcp".to_string()),
            artist_name: Some("bluetooth".to_string()),
            ..Default::default()
        };
        let expected = fidl::MediaAttributes {
            title: Some("avrcp".to_string()),
            artist_name: Some("bluetooth".to_string()),
            ..Default::default()
        };
        assert_eq!(expected, attributes.into());

        // Attributes with default cover art field.
        let attributes = MediaAttributeEntries {
            title: Some("avrcp".to_string()),
            artist_name: Some("bluetooth".to_string()),
            default_cover_art: Some("should be omitted".to_string()),
            ..Default::default()
        };
        assert_eq!(expected, attributes.into());
    }

    #[fuchsia::test]
    fn media_attribute_entries_from_fidl() {
        // Emtry attributes.
        let attributes = fidl::MediaAttributes::default();
        let expected = MediaAttributeEntries::default();
        assert_eq!(expected, attributes.into());

        // Attributes with some fields.
        let attributes = MediaAttributeEntries {
            title: Some("avrcp".to_string()),
            artist_name: Some("bluetooth".to_string()),
            ..Default::default()
        };
        let expected = fidl::MediaAttributes {
            title: Some("avrcp".to_string()),
            artist_name: Some("bluetooth".to_string()),
            ..Default::default()
        };
        assert_eq!(expected, attributes.into());

        // Attributes with default cover art field.
        let attributes = MediaAttributeEntries {
            title: Some("avrcp".to_string()),
            artist_name: Some("bluetooth".to_string()),
            default_cover_art: Some("should be omitted".to_string()),
            ..Default::default()
        };
        assert_eq!(expected, attributes.into());
    }
}
