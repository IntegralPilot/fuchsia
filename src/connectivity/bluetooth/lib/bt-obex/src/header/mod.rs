// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use chrono::{DateTime, NaiveDateTime};
use fuchsia_bluetooth::types::Uuid;
use log::trace;
use packet_encoding::{decodable_enum, Decodable, Encodable};

pub use self::header_set::HeaderSet;
use self::obex_string::ObexString;
use crate::error::PacketError;

/// A collection of Headers sent & received in an OBEX packet.
pub mod header_set;
/// Definition of an OBEX-specific String type used in Headers.
mod obex_string;

/// Uniquely identifies an OBEX connection between a Client and Server.
/// Defined in OBEX 1.5 Section 2.2.11.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ConnectionIdentifier(pub(crate) u32);

impl ConnectionIdentifier {
    pub fn id(&self) -> u32 {
        self.0
    }
}

impl TryFrom<u32> for ConnectionIdentifier {
    type Error = PacketError;

    fn try_from(src: u32) -> Result<Self, Self::Error> {
        // Per OBEX 1.5 Section 2.2.11, 0xffffffff is a reserved ID and considered invalid.
        if src == u32::MAX {
            return Err(PacketError::Reserved);
        }
        Ok(Self(src))
    }
}

/// Specifies the type of action of the Action Operation.
/// Defined in OBEX 1.5 Section 2.2.20.
#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(u8)]
pub enum ActionIdentifier {
    Copy = 0x00,
    MoveOrRename = 0x01,
    SetPermissions = 0x02,
    /// 0x03 - 0x7f is RFA.
    /// 0x80 - 0xff is reserved for vendor specific actions.
    Vendor(u8),
}

impl ActionIdentifier {
    fn is_vendor(id_raw: u8) -> bool {
        id_raw >= 0x80
    }
}

impl From<&ActionIdentifier> for u8 {
    fn from(src: &ActionIdentifier) -> u8 {
        match src {
            ActionIdentifier::Copy => 0x00,
            ActionIdentifier::MoveOrRename => 0x01,
            ActionIdentifier::SetPermissions => 0x02,
            ActionIdentifier::Vendor(v) => *v,
        }
    }
}

impl TryFrom<u8> for ActionIdentifier {
    type Error = PacketError;

    fn try_from(src: u8) -> Result<ActionIdentifier, Self::Error> {
        match src {
            0x00 => Ok(ActionIdentifier::Copy),
            0x01 => Ok(ActionIdentifier::MoveOrRename),
            0x02 => Ok(ActionIdentifier::SetPermissions),
            v if ActionIdentifier::is_vendor(v) => Ok(ActionIdentifier::Vendor(v)),
            _v => Err(Self::Error::Reserved),
        }
    }
}

/// Describes the type of an OBEX object. Used in `Header::Type`.
///
/// See OBEX 1.5 Section 2.2.3.
#[derive(Clone, Debug, PartialEq)]
pub struct MimeType(String);

impl MimeType {
    pub fn len(&self) -> usize {
        // The encoded format for the MimeType includes a 1 byte null terminator.
        self.0.len() + 1
    }

    pub fn to_be_bytes(&self) -> Vec<u8> {
        // The Type header is encoded as a byte sequence of null terminated ASCII text.
        let mut encoded_buf: Vec<u8> = self.0.clone().into_bytes();
        encoded_buf.push(0); // Add the null terminator
        encoded_buf
    }
}

impl TryFrom<&[u8]> for MimeType {
    type Error = PacketError;

    fn try_from(src: &[u8]) -> Result<Self, Self::Error> {
        if src.len() == 0 {
            return Err(PacketError::data("empty Type header"));
        }

        let mut text = String::from_utf8(src.to_vec()).map_err(PacketError::external)?;
        // The Type Header is represented as null terminated UTF-8 text. See OBEX 1.5 Section 2.2.3.
        if !text.ends_with('\0') {
            return Err(PacketError::data("Type missing null terminator"));
        }

        let _ = text.pop();
        Ok(Self(text))
    }
}

impl From<String> for MimeType {
    fn from(src: String) -> MimeType {
        MimeType(src)
    }
}

impl From<&str> for MimeType {
    fn from(src: &str) -> MimeType {
        MimeType(src.to_string())
    }
}

/// Represents a user-defined Header type.
// TODO(https://fxbug.dev/42072539): This representation may change depending on what kind of user
// headers we expect.
#[derive(Clone, Debug, PartialEq)]
pub struct UserDefinedHeader {
    /// The Header Identifier (HI) can be any value between 0x30 and 0x3f. See
    /// `HeaderIdentifier::User` for more details.
    identifier: u8,
    /// The user data.
    value: Vec<u8>,
}

/// Used to support a variety of application request/response parameters
/// which are represented in Tag-Length-Value triplets.
pub type TypeValue = (u8, Vec<u8>);

/// Represents the vector of TypeLengthValue triplets in application
/// parameter headers.
/// See OBEX spec v1.5 Section 2.2.12.
#[derive(Clone, Debug, PartialEq)]
pub struct TagLengthValue(Vec<TypeValue>);

impl TagLengthValue {
    /// Minimum data length of a single triplet includes the tag and length byte.
    const MIN_TRIPLET_LENGTH_BYTES: usize = 2;

    pub fn iter(&self) -> std::slice::Iter<'_, (u8, Vec<u8>)> {
        self.0.iter()
    }
}

impl From<Vec<TypeValue>> for TagLengthValue {
    fn from(value: Vec<TypeValue>) -> Self {
        Self(value)
    }
}

impl Encodable for TagLengthValue {
    type Error = PacketError;

    fn encoded_len(&self) -> core::primitive::usize {
        self.0
            .iter()
            .fold(0, |init, triplet| init + Self::MIN_TRIPLET_LENGTH_BYTES + triplet.1.len())
    }

    fn encode(&self, buf: &mut [u8]) -> core::result::Result<(), Self::Error> {
        if buf.len() < self.encoded_len() {
            return Err(PacketError::BufferTooSmall);
        }
        let mut start_index = 0;
        for (tag, value) in &self.0 {
            buf[start_index] = *tag;
            buf[start_index + 1] = value.len() as u8;
            buf[start_index + 2..start_index + 2 + value.len()].copy_from_slice(value.as_slice());
            start_index += 2 + value.len();
        }
        Ok(())
    }
}

impl TryFrom<&[u8]> for TagLengthValue {
    type Error = PacketError;

    /// Attempts to parse application parameters header from raw data.
    fn try_from(raw_data: &[u8]) -> Result<Self, Self::Error> {
        let data_len = raw_data.len();

        let mut decoded_len = 0;
        let mut list = vec![];

        while decoded_len < data_len {
            if data_len < (decoded_len + Self::MIN_TRIPLET_LENGTH_BYTES) {
                return Err(PacketError::DataLength);
            }
            let tag: u8 = raw_data[decoded_len];
            let value_len = raw_data[decoded_len + 1] as usize;
            if data_len < (decoded_len + Self::MIN_TRIPLET_LENGTH_BYTES + value_len) {
                return Err(PacketError::DataLength);
            }
            let value_start_idx = decoded_len + Self::MIN_TRIPLET_LENGTH_BYTES;
            let value = (&raw_data[value_start_idx..value_start_idx + value_len]).into();
            let _ = list.push((tag, value));
            decoded_len += Self::MIN_TRIPLET_LENGTH_BYTES + value_len;
        }
        Ok(TagLengthValue(list))
    }
}

decodable_enum! {
    /// Value indicating support for the Single Response Mode (SRM) feature in OBEX. Used as part
    /// of `Header::SingleResponseMode`.
    /// Defined in OBEX 1.5 Section 2.2.23.
    pub enum SingleResponseMode<u8, PacketError, Reserved> {
        // A request to disable SRM for the current operation.
        Disable = 0x00,
        // A request to enable SRM for the current operation.
        Enable = 0x01,
        // Per GOEP 2.1.1 Section 4.6, the `Supported` SRM flag is not used.
        // Supported = 0x02,
    }
}

impl From<bool> for SingleResponseMode {
    fn from(src: bool) -> SingleResponseMode {
        if src {
            SingleResponseMode::Enable
        } else {
            SingleResponseMode::Disable
        }
    }
}

impl From<SingleResponseMode> for Header {
    fn from(src: SingleResponseMode) -> Header {
        Header::SingleResponseMode(src)
    }
}

decodable_enum! {
    /// The Header Encoding is the upper 2 bits of the Header Identifier (HI) and describes the type
    /// of payload included in the Header.
    /// Defined in OBEX 1.5 Section 2.1.
    enum HeaderEncoding<u8, PacketError, HeaderEncoding> {
        /// A Header with null terminated Unicode text. The text is encoded in UTF-16 format. The
        /// text length is encoded as a two byte unsigned integer.
        Text = 0x00,
        /// A Header with a byte sequence. The sequence length is encoded as a two byte unsigned
        /// integer.
        Bytes = 0x40,
        /// A Header with a 1-byte payload.
        OneByte = 0x80,
        /// A Header with a 4-byte payload.
        FourBytes = 0xC0,
    }
}

/// The OBEX Header Identifier (HI) identifies the type of OBEX packet.
///
/// The HI is a one-byte unsigned value and is split into the upper 2 bits and lower 6 bits. The
/// upper 2 bits indicate the header encoding and the lower 6 bits indicate the type of the
/// header.
/// Defined in OBEX 1.5 Section 2.1.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
#[repr(u8)]
pub enum HeaderIdentifier {
    /// Number of objects.
    Count = 0xC0,
    /// Name of the object (typically a file name).
    Name = 0x01,
    /// Type of object (e.g. text, html, ...)
    Type = 0x42,
    /// The length of the object in bytes.
    Length = 0xC3,
    /// Date/time stamp - ISO 8601. This representation is preferred.
    TimeIso8601 = 0x44,
    /// Date/time stamp - 4 byte representation.
    Time4Byte = 0xC4,
    /// Text description of the object.
    Description = 0x05,
    /// Name of the service that the operation is targeting.
    Target = 0x46,
    /// An HTTP 1.x header.
    Http = 0x47,
    /// A chunk of the object body.
    Body = 0x48,
    /// The final chunk of the object body.
    EndOfBody = 0x49,
    /// Identifies the OBEX application session.
    Who = 0x4A,
    /// An identifier associated with the OBEX connection - used for connection multiplexing.
    ConnectionId = 0xCB,
    /// Extended information about the OBEX connection.
    ApplicationParameters = 0x4C,
    /// Authentication digest challenge.
    AuthenticationChallenge = 0x4D,
    /// Authentication digest response.
    AuthenticationResponse = 0x4E,
    /// Indicates the creator of the object.
    CreatorId = 0xCF,
    /// Uniquely identifies the network client.
    WanUuid = 0x50,
    /// Class of an OBEX object,
    ObjectClass = 0x51,
    /// Parameters associated with the OBEX session.
    SessionParameters = 0x52,
    /// Sequence number included in each OBEX packet - used for reliability.
    SessionSequenceNumber = 0x93,
    /// Specifies the type of ACTION Operation.
    ActionId = 0x94,
    /// The destination for an object - used in certain ACTION Operations.
    DestName = 0x15,
    /// Bit mask for setting permissions.
    Permissions = 0xD6,
    /// Indicates that Single Response Mode (SRM) should be used.
    SingleResponseMode = 0x97,
    /// Specifies the parameters used during SRM.
    SingleResponseModeParameters = 0x98,
    // 0x30 to 0x3F, 0x70 to 0x7F, 0xB0 to 0xBF, 0xF0 to 0xFF, is user defined.
    User(u8),
    // 0x19 to 0x2F, 0x59 to 0x6F, 0x99 to 0xAF, 0xD9 to 0xEF, is RFA.
}

impl HeaderIdentifier {
    fn is_user(id: u8) -> bool {
        // The user-defined space is between 0x30 and 0x3f and includes all combinations of the
        // upper 2 bits of the `id`.
        let lower_6_bits = id & 0x3f;
        lower_6_bits >= 0x30 && lower_6_bits <= 0x3f
    }

    fn is_reserved(id: u8) -> bool {
        // The reserved space is between 0x19 and 0x2f and includes all combinations of the
        // upper 2 bits of the `id`.
        let lower_6_bits = id & 0x3f;
        lower_6_bits >= 0x19 && lower_6_bits <= 0x2f
    }

    fn encoding(&self) -> HeaderEncoding {
        let id_raw: u8 = self.into();
        // The encoding is the upper 2 bits of the HeaderIdentifier.
        HeaderEncoding::try_from(id_raw & 0xc0).expect("valid Header encoding")
    }
}

impl TryFrom<u8> for HeaderIdentifier {
    type Error = PacketError;

    fn try_from(src: u8) -> Result<Self, Self::Error> {
        match src {
            0xC0 => Ok(Self::Count),
            0x01 => Ok(Self::Name),
            0x42 => Ok(Self::Type),
            0xC3 => Ok(Self::Length),
            0x44 => Ok(Self::TimeIso8601),
            0xC4 => Ok(Self::Time4Byte),
            0x05 => Ok(Self::Description),
            0x46 => Ok(Self::Target),
            0x47 => Ok(Self::Http),
            0x48 => Ok(Self::Body),
            0x49 => Ok(Self::EndOfBody),
            0x4A => Ok(Self::Who),
            0xCB => Ok(Self::ConnectionId),
            0x4C => Ok(Self::ApplicationParameters),
            0x4D => Ok(Self::AuthenticationChallenge),
            0x4E => Ok(Self::AuthenticationResponse),
            0xCF => Ok(Self::CreatorId),
            0x50 => Ok(Self::WanUuid),
            0x51 => Ok(Self::ObjectClass),
            0x52 => Ok(Self::SessionParameters),
            0x93 => Ok(Self::SessionSequenceNumber),
            0x94 => Ok(Self::ActionId),
            0x15 => Ok(Self::DestName),
            0xD6 => Ok(Self::Permissions),
            0x97 => Ok(Self::SingleResponseMode),
            0x98 => Ok(Self::SingleResponseModeParameters),
            id if HeaderIdentifier::is_user(id) => Ok(Self::User(id)),
            id if HeaderIdentifier::is_reserved(id) => Err(Self::Error::Reserved),
            id => Err(Self::Error::Identifier(id)),
        }
    }
}

impl Into<u8> for &HeaderIdentifier {
    fn into(self) -> u8 {
        match &self {
            HeaderIdentifier::Count => 0xC0,
            HeaderIdentifier::Name => 0x01,
            HeaderIdentifier::Type => 0x42,
            HeaderIdentifier::Length => 0xC3,
            HeaderIdentifier::TimeIso8601 => 0x44,
            HeaderIdentifier::Time4Byte => 0xC4,
            HeaderIdentifier::Description => 0x05,
            HeaderIdentifier::Target => 0x46,
            HeaderIdentifier::Http => 0x47,
            HeaderIdentifier::Body => 0x48,
            HeaderIdentifier::EndOfBody => 0x49,
            HeaderIdentifier::Who => 0x4A,
            HeaderIdentifier::ConnectionId => 0xCB,
            HeaderIdentifier::ApplicationParameters => 0x4C,
            HeaderIdentifier::AuthenticationChallenge => 0x4D,
            HeaderIdentifier::AuthenticationResponse => 0x4E,
            HeaderIdentifier::CreatorId => 0xCF,
            HeaderIdentifier::WanUuid => 0x50,
            HeaderIdentifier::ObjectClass => 0x51,
            HeaderIdentifier::SessionParameters => 0x52,
            HeaderIdentifier::SessionSequenceNumber => 0x93,
            HeaderIdentifier::ActionId => 0x94,
            HeaderIdentifier::DestName => 0x15,
            HeaderIdentifier::Permissions => 0xD6,
            HeaderIdentifier::SingleResponseMode => 0x97,
            HeaderIdentifier::SingleResponseModeParameters => 0x98,
            HeaderIdentifier::User(id) => *id,
        }
    }
}

/// The building block of an OBEX object. A single OBEX object consists of one or more Headers.
/// Defined in OBEX 1.5 Section 2.0.
#[derive(Clone, Debug, PartialEq)]
pub enum Header {
    Count(u32),
    /// The Name header can be empty which is a special parameter for the GET and SETPATH
    /// operations. It is also valid to provide a Name header with an empty OBEX string.
    /// See OBEX 1.5 Section 2.2.2.
    Name(Option<ObexString>),
    Type(MimeType),
    /// Number of bytes.
    Length(u32),
    /// Time represented as a well-formed NaiveDateTime (no timezone). This is typically UTC.
    /// See OBEX 1.5 Section 2.2.5.
    TimeIso8601(NaiveDateTime),
    /// Time represented as the number of seconds elapsed since midnight UTC on January 1, 1970.
    /// This is saved as a well-formed NaiveDateTime (no timezone).
    Time4Byte(NaiveDateTime),
    Description(ObexString),
    Target(Vec<u8>),
    Http(Vec<u8>),
    Body(Vec<u8>),
    EndOfBody(Vec<u8>),
    Who(Vec<u8>),
    ConnectionId(ConnectionIdentifier),
    ApplicationParameters(TagLengthValue),
    AuthenticationChallenge(Vec<u8>),
    AuthenticationResponse(Vec<u8>),
    CreatorId(u32),
    WanUuid(Uuid),
    ObjectClass(Vec<u8>),
    SessionParameters(Vec<u8>),
    SessionSequenceNumber(u8),
    ActionId(ActionIdentifier),
    DestName(ObexString),
    /// 4-byte bit mask.
    Permissions(u32),
    SingleResponseMode(SingleResponseMode),
    /// 1-byte quantity containing the parameters for the SRM session.
    SingleResponseModeParameters(u8),
    /// User defined Header type.
    User(UserDefinedHeader),
}

impl Header {
    /// The minimal Header contains at least a 1-byte identifier.
    const MIN_HEADER_LENGTH_BYTES: usize = 1;

    /// A Unicode or Byte Sequence Header must be at least 3 bytes - 1 byte for the HI and 2 bytes
    /// for the encoded data length.
    const MIN_UNICODE_OR_BYTE_SEQ_LENGTH_BYTES: usize = 3;

    /// The ISO 8601 time format used in the Time Header packet.
    /// The format is YYYYMMDDTHHMMSS where "T" delimits the date from the time. It is assumed that
    /// the time is the local time, but per OBEX 2.2.5, a suffix of "Z" can be included to indicate
    /// UTC time.
    const ISO_8601_TIME_FORMAT: &'static str = "%Y%m%dT%H%M%S";

    /// The ISO 8601 time format used in the Time Header Packet.
    /// The format is YYYYMMDDTHHMMSSZ, where the "Z" denotes UTC time. The format is sent as a UTF-
    /// 16 null terminated string. There are 32 bytes for the time and 2 bytes for the terminator.
    // TODO(https://fxbug.dev/42084674): This encoding assumes the timezone is always UTC. Update this
    // when `Header` supports timezones.
    const ISO_8601_LENGTH_BYTES: usize = 34;

    pub fn identifier(&self) -> HeaderIdentifier {
        match &self {
            Self::Count(_) => HeaderIdentifier::Count,
            Self::Name(_) => HeaderIdentifier::Name,
            Self::Type(_) => HeaderIdentifier::Type,
            Self::Length(_) => HeaderIdentifier::Length,
            Self::TimeIso8601(_) => HeaderIdentifier::TimeIso8601,
            Self::Time4Byte(_) => HeaderIdentifier::Time4Byte,
            Self::Description(_) => HeaderIdentifier::Description,
            Self::Target(_) => HeaderIdentifier::Target,
            Self::Http(_) => HeaderIdentifier::Http,
            Self::Body(_) => HeaderIdentifier::Body,
            Self::EndOfBody(_) => HeaderIdentifier::EndOfBody,
            Self::Who(_) => HeaderIdentifier::Who,
            Self::ConnectionId(_) => HeaderIdentifier::ConnectionId,
            Self::ApplicationParameters(_) => HeaderIdentifier::ApplicationParameters,
            Self::AuthenticationChallenge(_) => HeaderIdentifier::AuthenticationChallenge,
            Self::AuthenticationResponse(_) => HeaderIdentifier::AuthenticationResponse,
            Self::CreatorId(_) => HeaderIdentifier::CreatorId,
            Self::WanUuid(_) => HeaderIdentifier::WanUuid,
            Self::ObjectClass(_) => HeaderIdentifier::ObjectClass,
            Self::SessionParameters(_) => HeaderIdentifier::SessionParameters,
            Self::SessionSequenceNumber(_) => HeaderIdentifier::SessionSequenceNumber,
            Self::ActionId(_) => HeaderIdentifier::ActionId,
            Self::DestName(_) => HeaderIdentifier::DestName,
            Self::Permissions(_) => HeaderIdentifier::Permissions,
            Self::SingleResponseMode(_) => HeaderIdentifier::SingleResponseMode,
            Self::SingleResponseModeParameters(_) => HeaderIdentifier::SingleResponseModeParameters,
            Self::User(UserDefinedHeader { identifier, .. }) => HeaderIdentifier::User(*identifier),
        }
    }

    /// Returns the length, in bytes, of the Header's payload.
    fn data_length(&self) -> usize {
        use Header::*;
        match &self {
            SessionSequenceNumber(_)
            | ActionId(_)
            | SingleResponseMode(_)
            | SingleResponseModeParameters(_) => 1,
            Count(_) | Length(_) | ConnectionId(_) | CreatorId(_) | Permissions(_)
            | Time4Byte(_) => 4,
            Name(option_str) => option_str.as_ref().map_or(0, |s| s.len()),
            Description(s) | DestName(s) => s.len(),
            Target(b)
            | Http(b)
            | Body(b)
            | EndOfBody(b)
            | Who(b)
            | AuthenticationChallenge(b)
            | AuthenticationResponse(b)
            | ObjectClass(b)
            | SessionParameters(b) => b.len(),
            ApplicationParameters(params) => params.0.iter().fold(0, |acc, param| {
                acc + TagLengthValue::MIN_TRIPLET_LENGTH_BYTES + param.1.len()
            }),
            Type(mime_type) => mime_type.len(),
            TimeIso8601(_) => Self::ISO_8601_LENGTH_BYTES,
            WanUuid(_) => Uuid::BLUETOOTH_UUID_LENGTH_BYTES,
            User(UserDefinedHeader { value, .. }) => value.len(),
        }
    }

    pub fn name(s: &str) -> Self {
        Self::Name(Some(s.into()))
    }

    pub fn empty_name() -> Self {
        Self::Name(None)
    }
}

impl Encodable for Header {
    type Error = PacketError;

    fn encoded_len(&self) -> usize {
        // Only the `Text` and `Bytes` formats encode the payload length in the buffer.
        let payload_length_encoded_bytes = match self.identifier().encoding() {
            HeaderEncoding::Text | HeaderEncoding::Bytes => 2,
            _ => 0,
        };
        Self::MIN_HEADER_LENGTH_BYTES + payload_length_encoded_bytes + self.data_length()
    }

    fn encode(&self, buf: &mut [u8]) -> Result<(), Self::Error> {
        if buf.len() < self.encoded_len() {
            return Err(PacketError::BufferTooSmall);
        }

        // Encode the Header Identifier.
        buf[0] = (&self.identifier()).into();

        // Optionally encode the payload length if it is a variable length payload.
        let start_index = match self.identifier().encoding() {
            HeaderEncoding::Text | HeaderEncoding::Bytes => {
                let data_length_bytes = (self.encoded_len() as u16).to_be_bytes();
                buf[Self::MIN_HEADER_LENGTH_BYTES..Self::MIN_UNICODE_OR_BYTE_SEQ_LENGTH_BYTES]
                    .copy_from_slice(&data_length_bytes);
                Self::MIN_UNICODE_OR_BYTE_SEQ_LENGTH_BYTES
            }
            _ => Self::MIN_HEADER_LENGTH_BYTES,
        };

        // Encode the payload.
        use Header::*;
        match &self {
            Count(v)
            | Length(v)
            | ConnectionId(ConnectionIdentifier(v))
            | CreatorId(v)
            | Permissions(v) => {
                // Encode all 4-byte value headers.
                buf[start_index..start_index + 4].copy_from_slice(&v.to_be_bytes());
            }
            Name(None) => {} // An "empty" Name only encodes the HeaderIdentifier & Length (3).
            Name(Some(str)) | Description(str) | DestName(str) => {
                // Encode all ObexString Headers.
                let s = str.to_be_bytes();
                buf[start_index..start_index + s.len()].copy_from_slice(&s);
            }
            Target(src)
            | Http(src)
            | Body(src)
            | EndOfBody(src)
            | Who(src)
            | AuthenticationChallenge(src)
            | AuthenticationResponse(src)
            | ObjectClass(src)
            | SessionParameters(src) => {
                // Encode all byte buffer headers.
                let n = src.len();
                buf[start_index..start_index + n].copy_from_slice(&src[..]);
            }
            ApplicationParameters(params) => {
                params.encode(&mut buf[start_index..])?;
            }
            SessionSequenceNumber(v) | SingleResponseModeParameters(v) => {
                // Encode all 1-byte value headers.
                buf[start_index] = *v;
            }
            SingleResponseMode(v) => {
                buf[start_index] = v.into();
            }
            Type(mime_type) => {
                let b = mime_type.to_be_bytes();
                buf[start_index..start_index + b.len()].copy_from_slice(&b[..]);
            }
            ActionId(v) => {
                buf[start_index] = v.into();
            }
            TimeIso8601(time) => {
                let mut formatted = time.format(Self::ISO_8601_TIME_FORMAT).to_string();
                // Always assume it's UTC.
                // TODO(https://fxbug.dev/42084674): Conditionally do this when we store timezones.
                formatted.push('Z');
                let s = ObexString::from(formatted).to_be_bytes();
                buf[start_index..start_index + s.len()].copy_from_slice(&s);
            }
            Time4Byte(time) => {
                let timestamp_bytes = (time.and_utc().timestamp() as u32).to_be_bytes();
                buf[start_index..start_index + 4].copy_from_slice(&timestamp_bytes[..])
            }
            WanUuid(uuid) => buf[start_index..start_index + Uuid::BLUETOOTH_UUID_LENGTH_BYTES]
                .copy_from_slice(&uuid.as_be_bytes()[..]),
            User(UserDefinedHeader { value, .. }) => {
                let n = value.len();
                buf[start_index..start_index + n].copy_from_slice(&value[..]);
            }
        }
        Ok(())
    }
}

impl Decodable for Header {
    type Error = PacketError;

    fn decode(buf: &[u8]) -> Result<Self, Self::Error> {
        // The buffer should contain at least the Header Identifier.
        if buf.len() < Self::MIN_HEADER_LENGTH_BYTES {
            return Err(PacketError::BufferTooSmall);
        }

        let id = HeaderIdentifier::try_from(buf[0])?;
        let mut start_idx = 1;
        let data_length = match id.encoding() {
            HeaderEncoding::Text | HeaderEncoding::Bytes => {
                if buf.len() < Self::MIN_UNICODE_OR_BYTE_SEQ_LENGTH_BYTES {
                    return Err(PacketError::BufferTooSmall);
                }
                // For Unicode Text and Byte Sequences, the payload length is encoded in the next
                // two bytes - this value includes 1 byte for the HI and 2 bytes for the length.
                let total_length = u16::from_be_bytes(
                    buf[Self::MIN_HEADER_LENGTH_BYTES..Self::MIN_UNICODE_OR_BYTE_SEQ_LENGTH_BYTES]
                        .try_into()
                        .expect("checked length"),
                ) as usize;
                let data_length = total_length
                    .checked_sub(Self::MIN_UNICODE_OR_BYTE_SEQ_LENGTH_BYTES)
                    .ok_or(PacketError::DataLength)?;
                start_idx = Self::MIN_UNICODE_OR_BYTE_SEQ_LENGTH_BYTES;
                data_length
            }
            HeaderEncoding::OneByte => 1, // Otherwise, a 1-byte payload is included.
            HeaderEncoding::FourBytes => 4, // Otherwise, a 4-byte payload is included.
        };
        trace!(id:?, data_length:%; "Parsed OBEX packet");

        if buf.len() < start_idx + data_length {
            return Err(PacketError::BufferTooSmall);
        }

        let data = &buf[start_idx..start_idx + data_length];
        let mut out_buf = vec![0; data_length];
        match id {
            HeaderIdentifier::Count => {
                Ok(Header::Count(u32::from_be_bytes(data[..].try_into().unwrap())))
            }
            HeaderIdentifier::Name => {
                let name = if data.len() == 0 { None } else { Some(ObexString::try_from(data)?) };
                Ok(Header::Name(name))
            }
            HeaderIdentifier::Type => Ok(Header::Type(MimeType::try_from(data)?)),
            HeaderIdentifier::Length => {
                Ok(Header::Length(u32::from_be_bytes(data[..].try_into().unwrap())))
            }
            HeaderIdentifier::TimeIso8601 => {
                let mut time_str = ObexString::try_from(data).map(|s| s.to_string())?;
                // TODO(https://fxbug.dev/42084674): In some cases, the peer can send a local time instead of
                // UTC. The timezone is not specified. Figure out how to represent this using
                // DateTime/NaiveDateTime.
                if time_str.ends_with("Z") {
                    let _ = time_str.pop();
                }
                let parsed = NaiveDateTime::parse_from_str(&time_str, Self::ISO_8601_TIME_FORMAT)
                    .map_err(PacketError::external)?;
                Ok(Header::TimeIso8601(parsed))
            }
            HeaderIdentifier::Time4Byte => {
                let elapsed_time_seconds = u32::from_be_bytes(data[..].try_into().unwrap());
                let parsed =
                    DateTime::from_timestamp(elapsed_time_seconds.into(), /*nsecs= */ 0)
                        .ok_or_else(|| {
                            PacketError::external(anyhow::format_err!("invalid timestamp"))
                        })?;
                Ok(Header::Time4Byte(parsed.naive_utc()))
            }
            HeaderIdentifier::Description => Ok(Header::Description(ObexString::try_from(data)?)),
            HeaderIdentifier::Target => {
                out_buf.copy_from_slice(&data[..]);
                Ok(Header::Target(out_buf))
            }
            HeaderIdentifier::Http => {
                out_buf.copy_from_slice(&data[..]);
                Ok(Header::Http(out_buf))
            }
            HeaderIdentifier::Body => {
                out_buf.copy_from_slice(&data[..]);
                Ok(Header::Body(out_buf))
            }
            HeaderIdentifier::EndOfBody => {
                out_buf.copy_from_slice(&data[..]);
                Ok(Header::EndOfBody(out_buf))
            }
            HeaderIdentifier::Who => {
                out_buf.copy_from_slice(&data[..]);
                Ok(Header::Who(out_buf))
            }
            HeaderIdentifier::ConnectionId => Ok(Header::ConnectionId(
                ConnectionIdentifier::try_from(u32::from_be_bytes(data[..].try_into().unwrap()))?,
            )),
            HeaderIdentifier::ApplicationParameters => {
                Ok(Header::ApplicationParameters(TagLengthValue::try_from(&data[..])?))
            }
            HeaderIdentifier::AuthenticationChallenge => {
                out_buf.copy_from_slice(&data[..]);
                Ok(Header::AuthenticationChallenge(out_buf))
            }
            HeaderIdentifier::AuthenticationResponse => {
                out_buf.copy_from_slice(&data[..]);
                Ok(Header::AuthenticationResponse(out_buf))
            }
            HeaderIdentifier::CreatorId => {
                Ok(Header::CreatorId(u32::from_be_bytes(data[..].try_into().unwrap())))
            }
            HeaderIdentifier::WanUuid => {
                let bytes: [u8; Uuid::BLUETOOTH_UUID_LENGTH_BYTES] =
                    data[..].try_into().map_err(|_| PacketError::BufferTooSmall)?;
                Ok(Header::WanUuid(Uuid::from_be_bytes(bytes)))
            }
            HeaderIdentifier::ObjectClass => {
                out_buf.copy_from_slice(&data[..]);
                Ok(Header::ObjectClass(out_buf))
            }
            HeaderIdentifier::SessionParameters => {
                out_buf.copy_from_slice(&data[..]);
                Ok(Header::SessionParameters(out_buf))
            }
            HeaderIdentifier::SessionSequenceNumber => {
                Ok(Header::SessionSequenceNumber(buf[start_idx]))
            }
            HeaderIdentifier::ActionId => {
                Ok(Header::ActionId(ActionIdentifier::try_from(buf[start_idx])?))
            }
            HeaderIdentifier::DestName => Ok(Header::DestName(ObexString::try_from(data)?)),
            HeaderIdentifier::Permissions => {
                Ok(Header::Permissions(u32::from_be_bytes(data[..].try_into().unwrap())))
            }
            HeaderIdentifier::SingleResponseMode => {
                Ok(Header::SingleResponseMode(SingleResponseMode::try_from(buf[start_idx])?))
            }
            HeaderIdentifier::SingleResponseModeParameters => {
                Ok(Header::SingleResponseModeParameters(buf[start_idx]))
            }
            HeaderIdentifier::User(identifier) => {
                out_buf.copy_from_slice(&data[..]);
                Ok(Header::User(UserDefinedHeader { identifier, value: out_buf }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_matches::assert_matches;
    use chrono::{NaiveDate, NaiveTime};

    #[fuchsia::test]
    fn convert_action_identifier_success() {
        let id_raw = 0x00;
        let result = ActionIdentifier::try_from(id_raw).expect("valid identifier");
        assert_eq!(result, ActionIdentifier::Copy);
        let converted: u8 = (&result).into();
        assert_eq!(id_raw, converted);

        let vendor_id_raw = 0x85;
        let result = ActionIdentifier::try_from(vendor_id_raw).expect("valid identifier");
        assert_eq!(result, ActionIdentifier::Vendor(0x85));
        let converted: u8 = (&result).into();
        assert_eq!(vendor_id_raw, converted);
    }

    #[fuchsia::test]
    fn convert_action_identifier_error() {
        let invalid = 0x03;
        assert_matches!(ActionIdentifier::try_from(invalid), Err(PacketError::Reserved));

        let invalid = 0x7f;
        assert_matches!(ActionIdentifier::try_from(invalid), Err(PacketError::Reserved));
    }

    #[fuchsia::test]
    fn is_user_id() {
        let ids = [0x00, 0x10, 0x29, 0x40, 0x80, 0xc0];
        for id in ids {
            assert!(!HeaderIdentifier::is_user(id));
        }

        let ids = [0x30, 0x3f, 0x71, 0x7f, 0xb5, 0xbf, 0xf0, 0xff];
        for id in ids {
            assert!(HeaderIdentifier::is_user(id));
        }
    }

    #[fuchsia::test]
    fn is_reserved_id() {
        let ids = [0x00, 0x10, 0x30, 0x70, 0xb0, 0xf0];
        for id in ids {
            assert!(!HeaderIdentifier::is_reserved(id));
        }

        let ids = [0x19, 0x2f, 0x60, 0x6f, 0x99, 0xae, 0xd9, 0xef];
        for id in ids {
            assert!(HeaderIdentifier::is_reserved(id));
        }
    }

    #[fuchsia::test]
    fn valid_header_id_parsed_ok() {
        let valid = 0x15;
        let result = HeaderIdentifier::try_from(valid);
        assert_matches!(result, Ok(HeaderIdentifier::DestName));
    }

    #[fuchsia::test]
    fn user_header_id_is_ok() {
        let user_header_id_raw = 0x33;
        let result = HeaderIdentifier::try_from(user_header_id_raw);
        assert_matches!(result, Ok(HeaderIdentifier::User(_)));
    }

    #[fuchsia::test]
    fn rfa_header_id_is_reserved_error() {
        let rfa_header_id_raw = 0x20;
        let result = HeaderIdentifier::try_from(rfa_header_id_raw);
        assert_matches!(result, Err(PacketError::Reserved));
    }

    #[fuchsia::test]
    fn unknown_header_id_is_error() {
        // The lower 6 bits of this represent the Length Header. However, the upper 2 bits are
        // incorrect - therefore the Header ID is considered invalid.
        let unknown_header_id_raw = 0x03;
        let result = HeaderIdentifier::try_from(unknown_header_id_raw);
        assert_matches!(result, Err(PacketError::Identifier(_)));
    }

    #[fuchsia::test]
    fn header_encoding_from_identifier() {
        assert_eq!(HeaderIdentifier::SessionSequenceNumber.encoding(), HeaderEncoding::OneByte);
        assert_eq!(HeaderIdentifier::Count.encoding(), HeaderEncoding::FourBytes);
        assert_eq!(HeaderIdentifier::Name.encoding(), HeaderEncoding::Text);
        assert_eq!(HeaderIdentifier::Target.encoding(), HeaderEncoding::Bytes);
    }

    #[fuchsia::test]
    fn decode_empty_header_is_error() {
        assert_matches!(Header::decode(&[]), Err(PacketError::BufferTooSmall));
    }

    #[fuchsia::test]
    fn decode_header_no_payload_is_error() {
        // Valid Count Header but no contents.
        assert_matches!(Header::decode(&[0xc0]), Err(PacketError::BufferTooSmall));
    }

    #[fuchsia::test]
    fn decode_byte_seq_invalid_length_is_error() {
        // Valid `Name` Header (Text) but the provided length is only 1 byte.
        let buf = [0x01, 0x07];
        assert_matches!(Header::decode(&buf), Err(PacketError::BufferTooSmall));

        // Valid `Target` Header (Byte Seq) but the provided length is only 1 byte.
        let buf = [0x46, 0x05];
        assert_matches!(Header::decode(&buf), Err(PacketError::BufferTooSmall));

        // Valid `Body` Header (Byte Seq) but the provided length is too small - it must be >= 3.
        let buf = [0x48, 0x00, 0x02];
        assert_matches!(Header::decode(&buf), Err(PacketError::DataLength));
    }

    #[fuchsia::test]
    fn decode_header_invalid_payload_is_error() {
        // The provided payload doesn't match the expected data length.
        let buf = [
            0x42, // `Type` Header (Text)
            0x00, 0x0b, // Total length = 10 implies a data length of 7.
            0x00, 0x68, 0x00, 0x69, 0x00,
            0x00, // Payload = "hi" with a null terminator -> 6 length
        ];
        assert_matches!(Header::decode(&buf), Err(PacketError::BufferTooSmall));

        // The provided payload doesn't match the expected data length.
        let buf = [
            0xc3, // `Length` Header (4 bytes)
            0x00, 0x00, 0x00, // Payload = 3 bytes (should be 4).
        ];
        assert_matches!(Header::decode(&buf), Err(PacketError::BufferTooSmall));

        // The provided payload doesn't match the expected data length.
        let buf = [
            0x94, // `ActionId` Header (Expect 1 byte payload)
        ];
        assert_matches!(Header::decode(&buf), Err(PacketError::BufferTooSmall));

        // The provided payload doesn't match the expected data length.
        let buf = [
            0x42, // `EndOfBody` Header (Byte seq)
            0x00, 0x06, // Total length = 6 implies a data length of 3.
            0x12, 0x34, // Payload = 2 bytes (should be 3),
        ];
        assert_matches!(Header::decode(&buf), Err(PacketError::BufferTooSmall));
    }

    #[fuchsia::test]
    fn decode_valid_header_success() {
        // Text Header
        let name_buf = [
            0x01, // HI = Name
            0x00, 0x17, // Total length = 23 bytes
            0x00, 0x54, 0x00, 0x48, 0x00, 0x49,
            0x00, // 20 byte payload = "THING.DOC" (utf-16)
            0x4e, 0x00, 0x47, 0x00, 0x2e, 0x00, 0x44, 0x00, 0x4f, 0x00, 0x43, 0x00, 0x00,
        ];
        let result = Header::decode(&name_buf).expect("can decode name header");
        assert_eq!(result, Header::name("THING.DOC"));

        // Byte Sequence Header
        let object_class_buf = [
            0x51, // HI = Object Class
            0x00, 0x0a, // Total length = 10 bytes
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, // 7 byte payload
        ];
        let result = Header::decode(&object_class_buf).expect("can decode object class header");
        assert_eq!(result, Header::ObjectClass(vec![0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06]));

        // One-byte Header
        let session_seq_num_buf = [
            0x93, // HI = Session Sequence Number
            0x05, // 1 byte payload
        ];
        let result = Header::decode(&session_seq_num_buf).expect("can decode valid name header");
        assert_eq!(result, Header::SessionSequenceNumber(5));

        // Four-byte Header
        let connection_id_buf = [
            0xcb, // HI = Session Sequence Number
            0x00, 0x00, 0x12, 0x34, // 4 byte payload
        ];
        let result = Header::decode(&connection_id_buf).expect("can decode connection id header");
        assert_eq!(result, Header::ConnectionId(ConnectionIdentifier(0x1234)));
    }

    #[fuchsia::test]
    fn decode_user_data_header_success() {
        let user_buf = [
            0xb3, // HI = Random User defined
            0x05, // Upper 2 bits of HI indicate 1 byte payload
        ];
        let result = Header::decode(&user_buf).expect("can decode user header");
        assert_eq!(result, Header::User(UserDefinedHeader { identifier: 0xb3, value: vec![0x05] }));
    }

    #[fuchsia::test]
    fn decode_time_header_success() {
        let utc_time_buf = [
            0x44, // HI = Time ISO 8601
            0x00, 0x25, // Total length = 37 bytes
            0x00, 0x32, 0x00, 0x30, 0x00, 0x32, 0x00, 0x33, 0x00, 0x30, 0x00, 0x32, 0x00, 0x32,
            0x00, 0x34, 0x00, 0x54, 0x00, 0x31, 0x00, 0x32, 0x00, 0x34, 0x00, 0x31, 0x00, 0x33,
            0x00, 0x30, 0x00, 0x5a, 0x00, 0x00, // Time = "20230224T124130Z" (UTC), 34 bytes
        ];
        let result = Header::decode(&utc_time_buf).expect("can decode a utc time header");
        assert_matches!(result, Header::TimeIso8601(t) if t == NaiveDate::from_ymd_opt(2023, 2, 24).unwrap().and_hms_opt(12, 41, 30).unwrap());

        let local_time_buf = [
            0x44, // HI = Time ISO 8601
            0x00, 0x23, // Total length = 35 bytes
            0x00, 0x32, 0x00, 0x30, 0x00, 0x32, 0x00, 0x33, 0x00, 0x30, 0x00, 0x32, 0x00, 0x32,
            0x00, 0x34, 0x00, 0x54, 0x00, 0x31, 0x00, 0x32, 0x00, 0x34, 0x00, 0x31, 0x00, 0x33,
            0x00, 0x30, 0x00, 0x00, // Time = "20230224T124130" (UTC), 32 bytes
        ];
        let result = Header::decode(&local_time_buf).expect("can decode a local time header");
        assert_matches!(result, Header::TimeIso8601(t) if t == NaiveDate::from_ymd_opt(2023, 2, 24).unwrap().and_hms_opt(12, 41, 30).unwrap());

        // The timestamp corresponds to September 9, 2001 at 01:46:40.
        let timestamp_buf = [
            0xc4, // HI = Time 4-byte timestamp
            0x3b, 0x9a, 0xca, 0x00, // Timestamp = 1e9 seconds after Jan 1, 1970.
        ];
        let result = Header::decode(&timestamp_buf).expect("can decode a timestamp header");
        assert_matches!(result, Header::Time4Byte(t) if t == NaiveDate::from_ymd_opt(2001, 9, 9).unwrap().and_hms_opt(1, 46, 40).unwrap());
    }

    #[fuchsia::test]
    fn decode_invalid_time_header_is_error() {
        let invalid_utc_time_buf = [
            0x44, // HI = Time ISO 8601
            0x00, 0x23, // Total length = 35 bytes
            0x00, 0x32, 0x00, 0x30, 0x00, 0x32, 0x00, 0x33, 0x00, 0x30, 0x00, 0x32, 0x00, 0x32,
            0x00, 0x34, 0x00, 0x31, 0x00, 0x32, 0x00, 0x34, 0x00, 0x31, 0x00, 0x33, 0x00, 0x30,
            0x00, 0x5a, 0x00,
            0x00, // Time = "20230224124130Z" (UTC, missing the "T" delineator)
        ];
        assert_matches!(Header::decode(&invalid_utc_time_buf), Err(PacketError::Other(_)));

        // The timestamp Header should never produce an Error since the timestamp is bounded by
        // u32::MAX, whereas the only potential failure case in NaiveDateTime is i64::MAX.
    }

    #[fuchsia::test]
    fn decode_name_header_success() {
        let empty_name_buf = [
            0x01, // HI = Name
            0x00, 0x03, // Total length = 3
        ];
        let result = Header::decode(&empty_name_buf).expect("can decode empty name");
        assert_eq!(result, Header::Name(None));

        let empty_string_name_buf = [
            0x01, // HI = Name
            0x00, 0x05, // Total length = 5
            0x00, 0x00, // Name = "" (null terminated)
        ];
        let result = Header::decode(&empty_string_name_buf).expect("can decode empty string name");
        assert_eq!(result, Header::Name(Some("".into())));

        let name_buf = [
            0x01, // HI = Name
            0x00, 0x0b, // Total length = 11
            0x00, 0x44, 0x00, 0x4f, 0x00, 0x43, 0x00, 0x00, // Name = "DOC" (null terminated)
        ];
        let result = Header::decode(&name_buf).expect("can decode empty string name");
        assert_eq!(result, Header::Name(Some("DOC".into())));
    }

    #[fuchsia::test]
    fn decode_invalid_name_header_is_error() {
        let invalid_name_buf = [
            0x01, // HI = Name
            0x00, 0x09, // Total length = 9
            0x00, 0x44, 0x00, 0x4f, 0x00, 0x43, // Name = "DOC" (missing null terminator)
        ];
        let result = Header::decode(&invalid_name_buf);
        assert_matches!(result, Err(PacketError::Data(_)));
    }

    #[fuchsia::test]
    fn decode_type_header_success() {
        let type_buf = [
            0x42, // HI = Type
            0x00, 0x10, // Total length = 16
            0x74, 0x65, 0x78, 0x74, 0x2f, 0x78, 0x2d, 0x76, 0x43, 0x61, 0x72, 0x64,
            0x00, // 'text/x-vCard'
        ];
        let result = Header::decode(&type_buf).expect("can decode type header");
        assert_eq!(result, Header::Type("text/x-vCard".into()));

        let empty_type_buf = [
            0x42, // HI = Type
            0x00, 0x04, // Total length = 4
            0x00, // Text = empty string with null terminator.
        ];
        let result = Header::decode(&empty_type_buf).expect("can decode type header");
        assert_eq!(result, Header::Type("".into()));
    }

    #[fuchsia::test]
    fn decode_invalid_type_header_is_error() {
        // Empty type is an Error since it's missing the null terminator.
        let invalid_type_buf = [
            0x42, // HI = Type
            0x00, 0x03, // Total length = 3
        ];
        assert_matches!(Header::decode(&invalid_type_buf), Err(PacketError::Data(_)));

        // Valid type string but missing null terminator.
        let invalid_type_buf = [
            0x42, // HI = Type
            0x00, 0x0f, // Total length = 15
            0x74, 0x65, 0x78, 0x74, 0x2f, 0x78, 0x2d, 0x76, 0x43, 0x61, 0x72,
            0x64, // 'text/x-vCard'
        ];
        assert_matches!(Header::decode(&invalid_type_buf), Err(PacketError::Data(_)));

        // Invalid utf-8 string.
        let invalid_string_type_buf = [
            0x42, // HI = Type
            0x00, 0x07, // Total length = 7
            0x9f, 0x92, 0x96, 0x00, // Invalid utf-8
        ];
        assert_matches!(Header::decode(&invalid_string_type_buf), Err(PacketError::Other(_)));
    }

    #[fuchsia::test]
    fn encode_user_data_header_success() {
        // Encoding a 1-byte User Header should succeed.
        let user = Header::User(UserDefinedHeader { identifier: 0xb3, value: vec![0x12] });
        assert_eq!(user.encoded_len(), 2);
        let mut buf = vec![0; user.encoded_len()];
        user.encode(&mut buf).expect("can encode");
        let expected_buf = [0xb3, 0x12];
        assert_eq!(buf, expected_buf);

        // Encoding a 4-byte User Header should succeed.
        let user = Header::User(UserDefinedHeader {
            identifier: 0xf5,
            value: vec![0x00, 0x01, 0x02, 0x03],
        });
        assert_eq!(user.encoded_len(), 5);
        let mut buf = vec![0; user.encoded_len()];
        user.encode(&mut buf).expect("can encode");
        let expected_buf = [0xf5, 0x00, 0x01, 0x02, 0x03];
        assert_eq!(buf, expected_buf);

        // Encoding a Text User Header should succeed.
        let user = Header::User(UserDefinedHeader {
            identifier: 0x38,
            value: vec![0x00, 0x01, 0x00, 0x02, 0x00, 0x00],
        });
        assert_eq!(user.encoded_len(), 9);
        let mut buf = vec![0; user.encoded_len()];
        user.encode(&mut buf).expect("can encode");
        let expected_buf = [0x38, 0x00, 0x09, 0x00, 0x01, 0x00, 0x02, 0x00, 0x00];
        assert_eq!(buf, expected_buf);

        // Encoding a Bytes User Header should succeed.
        let user =
            Header::User(UserDefinedHeader { identifier: 0x70, value: vec![0x01, 0x02, 0x03] });
        assert_eq!(user.encoded_len(), 6);
        let mut buf = vec![0; user.encoded_len()];
        user.encode(&mut buf).expect("can encode");
        let expected_buf = [0x70, 0x00, 0x06, 0x01, 0x02, 0x03];
        assert_eq!(buf, expected_buf);
    }

    #[fuchsia::test]
    fn encode_time_header_success() {
        // Represents the date "20150603T123456Z" - 34 bytes in UTF 16.
        let time = Header::TimeIso8601(NaiveDateTime::new(
            NaiveDate::from_ymd_opt(2015, 6, 3).unwrap(),
            NaiveTime::from_hms_milli_opt(12, 34, 56, 0).unwrap(),
        ));
        // Total length should be 3 bytes (ID, Length) + 34 bytes (Time).
        assert_eq!(time.encoded_len(), 37);
        // Encoding with a larger buffer is OK.
        let mut buf = vec![0; time.encoded_len() + 2];
        time.encode(&mut buf).expect("can encode");
        let expected_buf = [
            0x44, // Header ID = Time ISO8601
            0x00, 0x25, // Length = 37 bytes total
            0x00, 0x32, 0x00, 0x30, 0x00, 0x31, 0x00, 0x35, 0x00, 0x30, 0x00, 0x36, 0x00, 0x30,
            0x00, 0x33, 0x00, 0x54, 0x00, 0x31, 0x00, 0x32, 0x00, 0x33, 0x00, 0x34, 0x00, 0x35,
            0x00, 0x36, 0x00, 0x5a, 0x00, 0x00, // "20150603T123456Z"
            0x00, 0x00, // Extra padding on the input `buf`.
        ];
        assert_eq!(buf, expected_buf);

        let timestamp =
            Header::Time4Byte(DateTime::from_timestamp(1_000_000, 0).unwrap().naive_utc());
        assert_eq!(timestamp.encoded_len(), 5);
        let mut buf = vec![0; timestamp.encoded_len()];
        timestamp.encode(&mut buf).expect("can encode");
        let expected_buf = [
            0xc4, // Header ID = Time 4byte
            0x00, 0x0f, 0x42, 0x40, // Time = 1_000_000 in hex bytes, 0xf4240
        ];
        assert_eq!(buf, expected_buf);
    }

    #[fuchsia::test]
    fn encode_uuid_header_success() {
        let uuid = Header::WanUuid(Uuid::new16(0x180d));
        // Total length should be 3 bytes (ID, length) + 16 bytes (UUID).
        assert_eq!(uuid.encoded_len(), 19);
        let mut buf = vec![0; uuid.encoded_len()];
        uuid.encode(&mut buf).expect("can encode");
        let expected_buf = [
            0x50, // Header ID = WanUuid
            0x00, 0x13, // Length = 19 bytes
            0x00, 0x00, 0x18, 0x0d, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b,
            0x34, 0xfb, // UUID (stringified) = "0000180d-0000-1000-8000-00805f9b34fb"
        ];
        assert_eq!(buf, expected_buf);
    }

    #[fuchsia::test]
    fn encode_name_header_success() {
        let empty_name = Header::empty_name();
        // Total length should be 3 bytes (ID, length). No name.
        assert_eq!(empty_name.encoded_len(), 3);
        let mut buf = vec![0; empty_name.encoded_len()];
        empty_name.encode(&mut buf).expect("can encode");
        let expected_buf = [0x01, 0x00, 0x03];
        assert_eq!(buf, expected_buf);

        let empty_string_name = Header::name("");
        // Total length should be 5 bytes (ID, 2-byte length). Null terminated empty string.
        assert_eq!(empty_string_name.encoded_len(), 5);
        let mut buf = vec![0; empty_string_name.encoded_len()];
        empty_string_name.encode(&mut buf).expect("can encode");
        let expected_buf = [0x01, 0x00, 0x05, 0x00, 0x00];
        assert_eq!(buf, expected_buf);

        let normal_string_name = Header::name("f");
        // Total length should be 7 bytes (ID, 2-byte length). Null terminated "f".
        assert_eq!(normal_string_name.encoded_len(), 7);
        let mut buf = vec![0; normal_string_name.encoded_len()];
        normal_string_name.encode(&mut buf).expect("can encode");
        let expected_buf = [0x01, 0x00, 0x07, 0x00, 0x66, 0x00, 0x00];
        assert_eq!(buf, expected_buf);
    }

    #[fuchsia::test]
    fn encode_type_header_success() {
        let type_ = Header::Type("text/html".into());
        // Total length should be 3 bytes (ID, length) + 10 bytes (null terminated string)
        assert_eq!(type_.encoded_len(), 13);
        let mut buf = vec![0; type_.encoded_len()];
        type_.encode(&mut buf).expect("can encode");
        let expected_buf = [
            0x42, // Header ID = Type
            0x00, 0x0d, // Length = 13 bytes
            0x74, 0x65, 0x78, 0x74, 0x2f, 0x68, 0x74, 0x6d, 0x6c, 0x00, // 'text/html'
        ];
        assert_eq!(buf, expected_buf);
    }

    #[fuchsia::test]
    fn encode_valid_header_success() {
        // Encoding a 1-byte Header should succeed.
        let srm = Header::SingleResponseMode(SingleResponseMode::Disable);
        assert_eq!(srm.encoded_len(), 2);
        let mut buf = vec![0; srm.encoded_len()];
        srm.encode(&mut buf).expect("can encode");
        let expected_buf = [
            0x97, // Header ID = SRM,
            0x00, // SRM = 0x00 (disable)
        ];
        assert_eq!(buf, expected_buf);

        // Encoding a 4-byte Header should succeed.
        let count = Header::Count(0x1234);
        assert_eq!(count.encoded_len(), 5);
        let mut buf = vec![0; count.encoded_len()];
        count.encode(&mut buf).expect("can encode");
        let expected_buf = [
            0xc0, // Header ID = Count
            0x00, 0x00, 0x12, 0x34, // Count = 0x1234
        ];
        assert_eq!(buf, expected_buf);

        // Encoding a Text Header should succeed.
        let desc = Header::Description("obextest".into());
        assert_eq!(desc.encoded_len(), 21);
        let mut buf = vec![0; desc.encoded_len()];
        desc.encode(&mut buf).expect("can encode");
        let expected_buf = [
            0x05, // Header ID = Description
            0x00, 0x15, // Length = 21 bytes
            0x00, 0x6f, 0x00, 0x62, 0x00, 0x65, 0x00, 0x78, 0x00, 0x74, 0x00, 0x65, 0x00, 0x73,
            0x00, 0x74, 0x00, 0x00, // "obextest" with null terminator
        ];
        assert_eq!(buf, expected_buf);

        // Encoding a Bytes Header should succeed.
        let auth_response = Header::AuthenticationResponse(vec![0x11, 0x22]);
        assert_eq!(auth_response.encoded_len(), 5);
        let mut buf = vec![0; auth_response.encoded_len()];
        auth_response.encode(&mut buf).expect("can encode");
        let expected_buf = [
            0x4e, // Header ID = Authentication Response
            0x00, 0x05, // Total Length = 5 bytes
            0x11, 0x22, // Response = [0x11, 0x22]
        ];
        assert_eq!(buf, expected_buf);
    }

    #[fuchsia::test]
    fn decode_application_header_success() {
        #[rustfmt::skip]
        let buf = [
            0x4C,                   // HI = Application Parameters
            0x00, 0x09,             // Total Length = 9 bytes
            0x29,                   // Tag
            0x04,                   // Length
            0x20, 0x00, 0x01, 0x00, // Value
        ];
        let result = Header::decode(&buf).expect("can decode application parameters header");
        assert_eq!(
            result,
            Header::ApplicationParameters(vec![(0x29, vec![0x20, 0x00, 0x01, 0x00])].into()),
        );

        #[rustfmt::skip]
        let buf = [
            0x4C,       // HI = Application Parameters
            0x00, 0x0B, // Total Length = 11 bytes
            0x01,       // Tag 1
            0x02,       // Length
            0x00, 0x01, // Value
            0x02,       // Tag 2
            0x02,       // Length
            0x00, 0x02  // Value
        ];
        let result = Header::decode(&buf).expect("can decode application parameters header");
        assert_eq!(
            result,
            Header::ApplicationParameters(
                vec![(0x01, vec![0x00, 0x01]), (0x02, vec![0x00, 0x02]),].into()
            ),
        );
    }

    #[fuchsia::test]
    fn decode_application_header_fail() {
        #[rustfmt::skip]
        let buf = [
            0x4C,                   // HI = Application Parameters
            0x00, 0x09,             // Total Length = 9 bytes
            0x29,                   // Tag
            0x03,                   // Length (incorrect)
            0x20, 0x00, 0x01, 0x00, // Value
        ];
        let _ = Header::decode(&buf).expect_err("should have failed");

        #[rustfmt::skip]
        let buf = [
            0x4C,       // HI = Application Parameters
            0x00, 0x0B, // Total Length = 11 bytes
            0x01,       // Tag 1
            0x02,       // Length
            0x00, 0x01, // Value
            0x02,       // Tag 2
            0x01,       // Length (incorrect)
            0x00, 0x02  // Value
        ];
        let _ = Header::decode(&buf).expect_err("should have failed");
    }

    #[fuchsia::test]
    fn encode_application_header() {
        // Single parameter.
        let header =
            Header::ApplicationParameters(vec![(0x29, vec![0x20, 0x00, 0x01, 0x00])].into());
        let mut buf = vec![0; header.encoded_len()];
        let _ = header.encode(&mut buf).expect("should encode successfully");

        #[rustfmt::skip]
        assert_eq!(buf, vec![
            0x4C,                   // HI = Application Parameters
            0x00, 0x09,             // Total Length = 9 bytes
            0x29,                   // Tag
            0x04,                   // Length
            0x20, 0x00, 0x01, 0x00, // Value
        ]);

        // Multiple parameters.
    }
}
