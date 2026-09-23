// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bitflags::bitflags;
use core::fmt::Debug;
use packet_encoding::{Decodable, Encodable, decodable_enum};
use std::cmp::PartialEq;

use crate::error::{Error, PacketError};
use crate::header::{HeaderIdentifier, HeaderSet};

/// The current OBEX Protocol version number is 1.0.
/// The protocol version is not necessarily the same as the specification version.
/// Defined in OBEX 1.5 Section 3.4.1.1.
const OBEX_PROTOCOL_VERSION_NUMBER: u8 = 0x10;

/// The maximum length of an OBEX packet is bounded by the 2-byte field describing the packet
/// length (u16::MAX).
/// Defined in OBEX 1.5 Section 3.4.1.3.
pub const MAX_PACKET_SIZE: usize = u16::MAX as usize;

/// The minimum size of the OBEX maximum packet length is 255 bytes.
/// Defined in OBEX 1.5. Section 3.4.1.4.
pub const MIN_MAX_PACKET_SIZE: usize = 255;

/// The default maximum size allowed for an OBEX object transfer (64 MB).
// TODO(https://fxbug.dev/536942228): Remove or make configurable when upper
// layer profiles can specify their own maximum object size limit.
pub const MAX_OBJECT_SIZE: usize = 64 * 1024 * 1024;

bitflags! {
    /// The flags used in a SetPath operation.
    /// Defined in OBEX 1.5 Section 3.4.6.1.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct SetPathFlags: u8 {
        /// Backup a directory level before applying (e.g. `../` on some systems).
        const BACKUP = 0b0000_0001;
        /// Don't create a folder if it does not exist. Return an Error instead.
        const DONT_CREATE = 0b0000_0010;
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum OpCode {
    Connect = 0x80,
    Disconnect = 0x81,
    Put = 0x02,
    PutFinal = 0x82,
    Get = 0x03,
    GetFinal = 0x83,
    Reserved = 0x04,
    ReservedFinal = 0x84,
    SetPath = 0x85,
    Action = 0x06,
    ActionFinal = 0x86,
    Session = 0x87,
    /// 0x08 to 0x0F are reserved and not used in OBEX.
    /// 0x10 to 0x1f are user defined.
    User(u8),
    Abort = 0xff,
}

impl OpCode {
    fn final_bit_set(v: u8) -> bool {
        (v & 0x80) != 0
    }

    fn is_user(code: u8) -> bool {
        // Defined in OBEX 1.5 Section 3.4.
        code >= 0x10 && code <= 0x1f
    }

    fn is_reserved(code: u8) -> bool {
        // Defined in OBEX 1.5 Section 3.4.
        code >= 0x08 && code <= 0x0f
    }

    /// Returns true if the Final bit is set.
    pub fn is_final(&self) -> bool {
        let opcode_raw: u8 = self.into();
        Self::final_bit_set(opcode_raw)
    }

    /// Returns the expected optional request data length (in bytes) if the Operation is expected to
    /// include request data.
    /// Returns 0 if the Operation is not expected to contain any data.
    /// See OBEX 1.5 Section 3.4 for more details on the specifics of the Operations.
    fn request_data_length(&self) -> usize {
        match &self {
            Self::Connect => 4, // OBEX Version (1) + Flags (1) + Max Packet Length (2)
            Self::SetPath => 2, // Flags (1) + Constants (1)
            _ => 0,             // All other operation requests don't require additional data
        }
    }

    /// Returns the expected optional response data length (in bytes) if the Operation is expected
    /// to include response data.
    /// Returns 0 if the Operation is not expected to contain any data.
    /// See OBEX 1.5 Section 3.4 for more details on the specifics of the Operations.
    pub fn response_data_length(&self) -> usize {
        match &self {
            Self::Connect => 4, // OBEX Version (1) + flags (1) + Max Packet Length (2)
            _ => 0,             // All other operation responses don't require additional data
        }
    }
}

impl Into<u8> for &OpCode {
    fn into(self) -> u8 {
        match &self {
            OpCode::Connect => 0x80,
            OpCode::Disconnect => 0x81,
            OpCode::Put => 0x02,
            OpCode::PutFinal => 0x82,
            OpCode::Get => 0x03,
            OpCode::GetFinal => 0x83,
            OpCode::Reserved => 0x04,
            OpCode::ReservedFinal => 0x84,
            OpCode::SetPath => 0x85,
            OpCode::Action => 0x06,
            OpCode::ActionFinal => 0x86,
            OpCode::Session => 0x87,
            OpCode::User(v) => *v,
            OpCode::Abort => 0xff,
        }
    }
}

impl TryFrom<u8> for OpCode {
    type Error = PacketError;

    fn try_from(src: u8) -> Result<OpCode, Self::Error> {
        // The Abort operation is unique in that it uses all bits in the opcode.
        if src == 0xff {
            return Ok(OpCode::Abort);
        }

        // Per OBEX 1.5 Section 3.4, only bits 0-4 are used to determine the OpCode. Bits 5,6
        // should be unset and are ignored. Bit 7 (msb) represents the final bit.
        const FINAL_BIT_AND_OPCODE_BITMASK: u8 = 0x9f;
        const OPCODE_BITMASK: u8 = 0x1f;
        let src = src & FINAL_BIT_AND_OPCODE_BITMASK;
        let is_final = OpCode::final_bit_set(src);
        // Check the lower 5 bits for opcode.
        match src & OPCODE_BITMASK {
            0x00 if is_final => Ok(OpCode::Connect),
            0x01 if is_final => Ok(OpCode::Disconnect),
            0x02 if is_final => Ok(OpCode::PutFinal),
            0x02 => Ok(OpCode::Put),
            0x03 if is_final => Ok(OpCode::GetFinal),
            0x03 => Ok(OpCode::Get),
            0x04 if is_final => Ok(OpCode::ReservedFinal),
            0x04 => Ok(OpCode::Reserved),
            0x05 if is_final => Ok(OpCode::SetPath),
            0x06 if is_final => Ok(OpCode::ActionFinal),
            0x06 => Ok(OpCode::Action),
            0x07 if is_final => Ok(OpCode::Session),
            v if OpCode::is_user(v) => Ok(OpCode::User(src)), // Save the final bit.
            v if OpCode::is_reserved(v) => Err(PacketError::Reserved),
            _ => Err(PacketError::OpCode(src)),
        }
    }
}

/// An OBEX Packet that can be encoded/decoded to/from a raw byte buffer. This is sent over the
/// L2CAP or RFCOMM transport.
#[derive(Clone, Debug, PartialEq)]
pub struct Packet<T>
where
    T: Clone + Debug + PartialEq,
    for<'a> &'a T: Into<u8>,
{
    /// The code associated with the packet.
    code: T,
    /// The data associated with the packet (e.g. Flags, Packet Size, etc..). This can be empty.
    /// Only used in the `OpCode::Connect` & `OpCode::SetPath` Operations.
    data: Vec<u8>,
    /// The headers describing the packet - there can be 0 or more headers included in the packet.
    headers: HeaderSet,
}

impl<T> Packet<T>
where
    T: Clone + Debug + PartialEq,
    for<'a> &'a T: Into<u8>,
{
    /// The minimum packet consists of an opcode (1 byte) and packet length (2 bytes).
    pub const MIN_PACKET_SIZE: usize = 3;

    pub fn new(code: T, data: Vec<u8>, headers: HeaderSet) -> Self {
        Self { code, data, headers }
    }

    pub fn code(&self) -> &T {
        &self.code
    }

    pub fn data(&self) -> &Vec<u8> {
        &self.data
    }

    pub fn headers(&self) -> &HeaderSet {
        &self.headers
    }

    /// Attempts to decode the body of `buf` into a `Packet`.
    /// `optional_data_length` specifies the expected length of the packet data and can be 0.
    fn decode_body(buf: &[u8], code: T, optional_data_length: usize) -> Result<Self, PacketError> {
        // Potentially decode the optional request data.
        let (headers_idx, data) = if optional_data_length != 0 {
            if buf.len() < optional_data_length {
                return Err(PacketError::BufferTooSmall);
            }
            let mut data = vec![0u8; optional_data_length];
            data.copy_from_slice(&buf[..optional_data_length]);
            (optional_data_length, data)
        } else {
            (0, vec![])
        };

        // Decode the headers.
        let headers = HeaderSet::decode(&buf[headers_idx..])?;
        Ok(Self::new(code, data, headers))
    }
}

impl<T> Encodable for Packet<T>
where
    T: Clone + Debug + PartialEq,
    for<'a> &'a T: Into<u8>,
{
    type Error = PacketError;

    fn encoded_len(&self) -> usize {
        Self::MIN_PACKET_SIZE + self.data.len() + self.headers.encoded_len()
    }

    fn encode(&self, buf: &mut [u8]) -> Result<(), Self::Error> {
        if buf.len() < self.encoded_len() {
            return Err(PacketError::BufferTooSmall);
        }

        // Per OBEX 1.5 Section 3.1, the first byte contains the opcode and bytes 1,2 contain
        // the packet length - this includes the opcode / length fields.
        buf[0] = (&self.code).into();
        let packet_length_bytes = (self.encoded_len() as u16).to_be_bytes();
        buf[1..Self::MIN_PACKET_SIZE].copy_from_slice(&packet_length_bytes[..]);

        // Encode the optional request data for relevant operations.
        let headers_idx = if self.data.len() != 0 {
            let end_idx = Self::MIN_PACKET_SIZE + self.data.len();
            buf[Self::MIN_PACKET_SIZE..end_idx].copy_from_slice(&self.data[..]);
            end_idx
        } else {
            Self::MIN_PACKET_SIZE
        };

        // Encode the headers.
        self.headers.encode(&mut buf[headers_idx..])
    }
}

impl<T> From<Packet<T>> for HeaderSet
where
    T: Clone + Debug + PartialEq,
    for<'a> &'a T: Into<u8>,
{
    fn from(value: Packet<T>) -> Self {
        value.headers
    }
}

/// An OBEX request packet.
/// Defined in OBEX 1.5 Section 3.1.
pub type RequestPacket = Packet<OpCode>;

impl RequestPacket {
    /// Returns a CONNECT request packet with the provided `headers`.
    pub fn new_connect(max_packet_size: u16, headers: HeaderSet) -> Self {
        // The CONNECT request contains optional data - Version Number, Flags, Max Packet Size.
        let mut data = vec![
            OBEX_PROTOCOL_VERSION_NUMBER,
            0, // All flags are currently reserved in a CONNECT request. See OBEX 3.4.1.2.
        ];
        data.extend_from_slice(&max_packet_size.to_be_bytes());
        Self::new(OpCode::Connect, data, headers)
    }

    pub fn new_disconnect(headers: HeaderSet) -> Self {
        Self::new(OpCode::Disconnect, vec![], headers)
    }

    pub fn new_get(headers: HeaderSet) -> Self {
        Self::new(OpCode::Get, vec![], headers)
    }

    pub fn new_get_final(headers: HeaderSet) -> Self {
        Self::new(OpCode::GetFinal, vec![], headers)
    }

    pub fn new_put(headers: HeaderSet) -> Self {
        Self::new(OpCode::Put, vec![], headers)
    }

    pub fn new_put_final(headers: HeaderSet) -> Self {
        Self::new(OpCode::PutFinal, vec![], headers)
    }

    pub fn new_set_path(flags: SetPathFlags, headers: HeaderSet) -> Result<Self, Error> {
        // The Name header is mandatory in almost all cases. All other headers are optional.
        // It is only considered optional when the request is to back up one level.
        // See Section 3.4.6.3.
        if !headers.contains_header(&HeaderIdentifier::Name)
            && !flags.contains(SetPathFlags::BACKUP)
        {
            return Err(Error::operation(OpCode::SetPath, "name is required"));
        }
        // The request contains optional data - Flags & Constants. Constants are currently reserved
        // and are set to 0. See Section 3.4.6.2.
        let data = vec![flags.bits(), 0];
        Ok(Self::new(OpCode::SetPath, data, headers))
    }

    pub fn new_abort(headers: HeaderSet) -> Self {
        Self::new(OpCode::Abort, vec![], headers)
    }
}

impl Decodable for RequestPacket {
    type Error = PacketError;

    fn decode(buf: &[u8]) -> Result<Self, Self::Error> {
        if buf.len() < Self::MIN_PACKET_SIZE {
            return Err(PacketError::BufferTooSmall);
        }

        let code = OpCode::try_from(buf[0])?;
        let packet_length =
            u16::from_be_bytes(buf[1..Self::MIN_PACKET_SIZE].try_into().expect("checked length"));

        if buf.len() < packet_length.into() {
            return Err(PacketError::BufferTooSmall);
        }
        // Decode the optional response data and headers.
        Self::decode_body(&buf[Self::MIN_PACKET_SIZE..], code, code.request_data_length())
    }
}

decodable_enum! {
    /// Response codes that an OBEX server may send to the Client after receiving a request.
    /// The most significant bit of the response code is the Final Bit. This is always set in OBEX
    /// response codes - see OBEX 1.5 Section 3.2.
    /// Defined in OBEX 1.5 Section 3.2.1.
    pub enum ResponseCode<u8, PacketError, Reserved> {
        Continue = 0x90,
        Ok = 0xa0,
        Created = 0xa1,
        Accepted = 0xa2,
        NonAuthoritativeInformation = 0xa3,
        NoContent = 0xa4,
        ResetContent = 0xa5,
        PartialContent = 0xa6,
        MultipleChoices = 0xb0,
        MovedPermanently = 0xb1,
        MovedTemporarily = 0xb2,
        SeeOther = 0xb3,
        NotModified = 0xb4,
        UseProxy = 0xb5,
        BadRequest = 0xc0,
        Unauthorized = 0xc1,
        PaymentRequired = 0xc2,
        Forbidden = 0xc3,
        NotFound = 0xc4,
        MethodNotAllowed = 0xc5,
        NotAcceptable = 0xc6,
        ProxyAuthenticationRequired = 0xc7,
        RequestTimeOut = 0xc8,
        Conflict = 0xc9,
        Gone = 0xca,
        LengthRequired = 0xcb,
        PreconditionFailed = 0xcc,
        RequestedEntityTooLarge = 0xcd,
        RequestedUrlTooLarge = 0xce,
        UnsupportedMediaType = 0xcf,
        InternalServerError = 0xd0,
        NotImplemented = 0xd1,
        BadGateway = 0xd2,
        ServiceUnavailable = 0xd3,
        GatewayTimeout = 0xd4,
        HttpVersionNotSupported = 0xd5,
        DatabaseFull = 0xe0,
        DatabaseLocked = 0xe1,
    }
}

/// An OBEX response packet.
/// Defined in OBEX 1.5 Section 3.2.
pub type ResponsePacket = Packet<ResponseCode>;

impl ResponsePacket {
    /// Creates a response packet with the given response code and no data or headers.
    pub fn new_empty(code: ResponseCode) -> Self {
        Self::new_no_data(code, HeaderSet::new())
    }

    pub fn new_no_data(code: ResponseCode, headers: HeaderSet) -> Self {
        Self::new(code, vec![], headers)
    }

    pub fn new_connect(code: ResponseCode, max_packet_size: u16, headers: HeaderSet) -> Self {
        // The CONNECT response contains optional data - Version Number, Flags, Max Packet Size.

        // Only Bit0 is defined for the CONNECT response. We currently do not support multiple
        // lrMP connections.
        const OBEX_CONNECT_RESPONSE_FLAGS: u8 = 0;
        let mut data = vec![OBEX_PROTOCOL_VERSION_NUMBER, OBEX_CONNECT_RESPONSE_FLAGS];
        data.extend_from_slice(&max_packet_size.to_be_bytes());
        Self::new(code, data, headers)
    }

    pub fn new_disconnect(headers: HeaderSet) -> Self {
        Self::new(ResponseCode::Ok, vec![], headers)
    }

    pub fn new_setpath(code: ResponseCode, headers: HeaderSet) -> Self {
        Self::new(code, vec![], headers)
    }

    pub fn new_get(code: ResponseCode, headers: HeaderSet) -> Self {
        Self::new(code, vec![], headers)
    }

    pub fn expect_code(self, request: OpCode, expected: ResponseCode) -> Result<Self, Error> {
        if *self.code() == expected {
            return Ok(self);
        }
        Err(Error::peer_rejected(request, *self.code()))
    }

    /// Attempts to decode the raw `buf` into a ResponsePacket for the provided `request` type.
    // `Decodable` is not implemented for `ResponsePacket` because the `OpCode` is not included in
    // a response packet. Because only one Operation can be outstanding, it is assumed that a
    // response is associated with the most recently sent request.
    pub fn decode(buf: &[u8], request: OpCode) -> Result<Self, PacketError> {
        if buf.len() < Self::MIN_PACKET_SIZE {
            return Err(PacketError::BufferTooSmall);
        }

        let code = ResponseCode::try_from(buf[0]).map_err(|_| PacketError::ResponseCode(buf[0]))?;
        let packet_length =
            u16::from_be_bytes(buf[1..Self::MIN_PACKET_SIZE].try_into().expect("checked length"));

        if buf.len() < packet_length.into() {
            return Err(PacketError::BufferTooSmall);
        }
        // Decode the optional response data and headers.
        Self::decode_body(&buf[Self::MIN_PACKET_SIZE..], code, request.response_data_length())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_matches::assert_matches;

    use crate::header::{ConnectionIdentifier, Header};

    #[fuchsia::test]
    fn convert_opcode_success() {
        // Roundtrip with final disabled should succeed.
        let raw = 0x02;
        let converted = OpCode::try_from(raw).expect("valid opcode");
        assert_eq!(converted, OpCode::Put);
        assert!(!converted.is_final());
        assert_eq!(converted.request_data_length(), 0);
        assert_eq!(converted.response_data_length(), 0);
        let converted_raw: u8 = (&converted).into();
        assert_eq!(converted_raw, raw);

        // Roundtrip with final enabled should succeed.
        let raw = 0x84;
        let converted = OpCode::try_from(raw).expect("valid opcode");
        assert_eq!(converted, OpCode::ReservedFinal);
        assert!(converted.is_final());
        let converted_raw: u8 = (&converted).into();
        assert_eq!(converted_raw, raw);

        // Roundtrip for Abort should succeed (special).
        let raw = 0xff;
        let converted = OpCode::try_from(raw).expect("valid opcode");
        assert_eq!(converted, OpCode::Abort);
        assert!(converted.is_final());
        let converted_raw: u8 = (&converted).into();
        assert_eq!(converted_raw, raw);

        // Roundtrip for an opcode with bits 5,6 set is OK. The bits are unused, and the
        // receiving side should ignore.
        let raw = 0xe5; // SetPath (0x85) with bits 5,6 set.
        let converted = OpCode::try_from(raw).expect("valid opcode");
        assert_eq!(converted, OpCode::SetPath);
        assert!(converted.is_final());
        let converted_raw: u8 = (&converted).into();
        assert_eq!(converted_raw, 0x85); // We will never set bits 5,6.
    }

    #[fuchsia::test]
    fn convert_user_opcode_success() {
        // User opcode with final bit unset.
        let user = 0x1a;
        let converted = OpCode::try_from(user).expect("valid opcode");
        assert_eq!(converted, OpCode::User(0x1a));
        assert!(!converted.is_final());
        let converted_raw: u8 = (&converted).into();
        assert_eq!(converted_raw, user);

        // User opcode with final bit set.
        let user = 0x9d;
        let converted = OpCode::try_from(user).expect("valid opcode");
        assert_eq!(converted, OpCode::User(0x9d));
        assert!(converted.is_final());
        let converted_raw: u8 = (&converted).into();
        // Final bit should be preserved when converting back.
        assert_eq!(converted_raw, user);

        // User opcode with bits 5,6 set. Bits 5,6 should be ignored.
        let user = 0xf3;
        let converted = OpCode::try_from(user).expect("valid opcode");
        assert_eq!(converted, OpCode::User(0x93)); // Bits 5,6 should be zeroed out.
        assert!(converted.is_final());
        let converted_raw: u8 = (&converted).into();
        assert_eq!(converted_raw, 0x93);
    }

    #[fuchsia::test]
    fn convert_invalid_opcode_is_error() {
        // A Disconnect OpCode without the final bit set is invalid.
        let invalid = 0x01;
        assert_matches!(OpCode::try_from(invalid), Err(PacketError::OpCode(_)));
        // Opcode is reserved for future use.
        let reserved = 0x08;
        assert_matches!(OpCode::try_from(reserved), Err(PacketError::Reserved));
        // Opcode is reserved for future use (final bit set).
        let reserved = 0x8f;
        assert_matches!(OpCode::try_from(reserved), Err(PacketError::Reserved));
    }

    #[fuchsia::test]
    fn construct_setpath() {
        // A request with all flags enabled & Name header is valid.
        let headers = HeaderSet::from_header(Header::name("foo"));
        let _request = RequestPacket::new_set_path(SetPathFlags::all(), headers.clone())
            .expect("valid set path args");

        // A request with no flags enabled & Name header is valid.
        let _request = RequestPacket::new_set_path(SetPathFlags::empty(), headers)
            .expect("valid set path args");

        // A request to back up a level doesn't require a Name header.
        let _request = RequestPacket::new_set_path(SetPathFlags::BACKUP, HeaderSet::new())
            .expect("valid set path args");

        // Otherwise, a request without a Name header is an Error.
        assert_matches!(
            RequestPacket::new_set_path(SetPathFlags::DONT_CREATE, HeaderSet::new()),
            Err(Error::OperationError { .. })
        );
    }

    #[fuchsia::test]
    fn encode_request_packet_success() {
        let headers = HeaderSet::from_headers(vec![Header::Permissions(2)]).unwrap();
        let request = RequestPacket::new(OpCode::Abort, vec![], headers);
        // 3 bytes for prefix + 5 bytes for Permissions Header.
        assert_eq!(request.encoded_len(), 8);
        let mut buf = vec![0; request.encoded_len()];
        request.encode(&mut buf[..]).expect("can encode request");
        let expected = [0xff, 0x00, 0x08, 0xd6, 0x00, 0x00, 0x00, 0x02];
        assert_eq!(buf, expected);
    }

    #[fuchsia::test]
    fn encode_request_packet_no_headers_success() {
        // 3 bytes for prefix - no additional headers.
        let request = RequestPacket::new(OpCode::Abort, vec![], HeaderSet::new());
        assert_eq!(request.encoded_len(), 3);
        let mut buf = vec![0; request.encoded_len()];
        request.encode(&mut buf[..]).expect("can encode request");
        let expected = [0xff, 0x00, 0x03];
        assert_eq!(buf, expected);
    }

    #[fuchsia::test]
    fn decode_request_packet_success() {
        let request_buf = [
            0x81, // OpCode = Disconnect
            0x00, 0x0e, // Total Length = 14 bytes (3 for prefix, 11 for "Name" Header)
            0x01, 0x00, 0xb, 0x00, 0x66, 0x00, 0x75, 0x00, 0x6e, 0x00, 0x00, // Name = "fun"
        ];
        let decoded = RequestPacket::decode(&request_buf[..]).expect("valid request");
        let expected_headers = HeaderSet::from_headers(vec![Header::name("fun")]).unwrap();
        let expected = RequestPacket::new(OpCode::Disconnect, vec![], expected_headers);
        assert_eq!(decoded, expected);
    }

    /// Example taken from OBEX 1.5 Section 3.4.1.9.
    #[fuchsia::test]
    fn encode_connect_request_packet_success() {
        let headers =
            HeaderSet::from_headers(vec![Header::Count(4), Header::Length(0xf483)]).unwrap();
        let request = RequestPacket::new_connect(0x2000, headers);
        assert_eq!(request.encoded_len(), 17);
        let mut buf = vec![0; request.encoded_len()];
        request.encode(&mut buf[..]).expect("can encode request");
        let expected = [
            0x80, // OpCode = CONNECT
            0x00, 0x11, // Packet length = 17
            0x10, 0x00, 0x20, 0x00, // Version = 1.0, Flags = 0, Max packet size = 8k bytes
            0xc0, 0x00, 0x00, 0x00, 0x04, // Count Header = 0x4
            0xc3, 0x00, 0x00, 0xf4, 0x83, // Length Header = 0xf483
        ];
        assert_eq!(buf, expected);
    }

    #[fuchsia::test]
    fn decode_connect_request_packet_success() {
        // Raw request contains CONNECT OpCode (length = 12) with a max packet size of 0xffff. An
        // optional Count header is included.
        let request_buf = [
            0x80, // OpCode = Connect
            0x00, 0x0c, // Total Length = 12 bytes
            0x10, 0x00, 0xff, 0xff, // Version = 1.0, Flags = 0, Max packet size = u16::MAX
            0xc0, 0x00, 0x00, 0xff, 0xff, // Optional Count Header = 0xffff
        ];
        let decoded = RequestPacket::decode(&request_buf[..]).expect("valid request");
        let expected_headers = HeaderSet::from_headers(vec![Header::Count(0xffff)]).unwrap();
        let expected =
            RequestPacket::new(OpCode::Connect, vec![0x10, 0x00, 0xff, 0xff], expected_headers);
        assert_eq!(decoded, expected);
    }

    #[fuchsia::test]
    fn decode_invalid_connect_request_error() {
        let missing_data = [
            0x80, // OpCode = Connect
            0x00, 0x03, // Total Length = 3 bytes (Only prefix, missing data)
        ];
        let decoded = RequestPacket::decode(&missing_data[..]);
        assert_matches!(decoded, Err(PacketError::BufferTooSmall));

        let invalid_data = [
            0x80, // OpCode = Connect
            0x00, 0x07, // Total Length = 7 bytes (Prefix, no optional headers, invalid data)
            0x10, 0x00, // Data is missing max packet size, should be 4 bytes total.
        ];
        let decoded = RequestPacket::decode(&invalid_data[..]);
        assert_matches!(decoded, Err(PacketError::BufferTooSmall));

        // Any additional data will be treated as part of the optional Headers, and so this will
        // fail.
        let invalid_data_too_long = [
            0x80, // OpCode = Connect
            0x00, 0x08, // Total Length = 8 bytes (Prefix, no optional headers, invalid data)
            0x10, 0x00, 0x00, 0xff, 0x01, // Data should only be 4 bytes
        ];
        let decoded = RequestPacket::decode(&invalid_data_too_long[..]);
        assert_matches!(decoded, Err(PacketError::BufferTooSmall));
    }

    #[fuchsia::test]
    fn encode_setpath_request_success() {
        let headers = HeaderSet::from_headers(vec![Header::name("bar")]).unwrap();
        let request = RequestPacket::new_set_path(SetPathFlags::all(), headers).unwrap();
        assert_eq!(request.encoded_len(), 16);
        let mut buf = vec![0; request.encoded_len()];
        request.encode(&mut buf[..]).expect("can encode request");
        let expected = [
            0x85, // OpCode = SETPATH
            0x00, 0x10, // Packet length = 16
            0x03, 0x00, // Flags = 3 (Backup & Don't create), Constants = 0
            0x01, 0x00, 0x0b, 0x00, 0x62, 0x00, 0x61, 0x00, 0x72, 0x00,
            0x00, // Name Header = "bar"
        ];
        assert_eq!(buf, expected);
    }

    #[fuchsia::test]
    fn decode_setpath_request_success() {
        let request_buf = [
            0x85, // OpCode = SETPATH
            0x00, 0x0e, // Packet length = 14
            0x02, 0x00, // Flags = 2 (Don't create), Constants = 0
            0x01, 0x00, 0x09, 0x00, 0x61, 0x00, 0x72, 0x00, 0x00, // Name Header = "ar"
        ];
        let decoded = RequestPacket::decode(&request_buf[..]).expect("valid request");
        let expected_headers = HeaderSet::from_headers(vec![Header::name("ar")]).unwrap();
        let expected = RequestPacket::new(OpCode::SetPath, vec![0x02, 0x00], expected_headers);
        assert_eq!(decoded, expected);
    }

    #[fuchsia::test]
    fn decode_invalid_setpath_request_error() {
        let missing_data = [
            0x85, // OpCode = SetPath
            0x00,
            0x03, // Total Length = 3 bytes (Only prefix, missing data, optional headers)
        ];
        let decoded = RequestPacket::decode(&missing_data[..]);
        assert_matches!(decoded, Err(PacketError::BufferTooSmall));

        let invalid_data = [
            0x85, // OpCode = Connect
            0x00, 0x04, // Total Length = 4 bytes (Prefix, no optional headers, invalid data)
            0x02, // Data is missing `constants` (should be 2 bytes total)
        ];
        let decoded = RequestPacket::decode(&invalid_data[..]);
        assert_matches!(decoded, Err(PacketError::BufferTooSmall));

        // Any additional data will be treated as part of the optional Headers, and so this will
        // fail.
        let invalid_data_too_long = [
            0x85, // OpCode = SetPath
            0x00, 0x08, // Total Length = 8 bytes (Prefix, no optional headers, invalid data)
            0x10, 0x00, 0x00, 0xff, 0x01, // Data should only be 2 bytes
        ];
        let decoded = RequestPacket::decode(&invalid_data_too_long[..]);
        assert_matches!(decoded, Err(_));
    }

    #[fuchsia::test]
    fn encode_response_packet_success() {
        let headers = HeaderSet::from_headers(vec![Header::DestName("foo".into())]).unwrap();
        let response = ResponsePacket::new(ResponseCode::Gone, vec![], headers);
        assert_eq!(response.encoded_len(), 14);
        let mut buf = vec![0; response.encoded_len()];
        response.encode(&mut buf[..]).expect("can encode valid response packet");
        let expected_buf = [
            0xca, 0x00, 0x0e, // Response = Gone, Length = 14
            0x15, 0x00, 0x0b, 0x00, 0x66, 0x00, 0x6f, 0x00, 0x6f, 0x00,
            0x00, // DestName = "foo"
        ];
        assert_eq!(buf, expected_buf);
    }

    #[fuchsia::test]
    fn decode_response_packet_success() {
        let response_buf = [
            0xa0, 0x00, 0x09, // ResponseCode = Ok, Total Length = 9
            0x46, 0x00, 0x06, 0x00, 0x02, 0x04, // Target = [0x00, 0x02, 0x04]
        ];
        let decoded = ResponsePacket::decode(&response_buf[..], OpCode::GetFinal)
            .expect("can decode valid response");
        let expected_headers =
            HeaderSet::from_headers(vec![Header::Target(vec![0x00, 0x02, 0x04])]).unwrap();
        let expected = ResponsePacket::new(ResponseCode::Ok, vec![], expected_headers);
        assert_eq!(decoded, expected);
    }

    #[fuchsia::test]
    fn decode_invalid_response_packet_error() {
        // Input buffer too small
        let response_buf = [0x90];
        let decoded = ResponsePacket::decode(&response_buf[..], OpCode::SetPath);
        assert_matches!(decoded, Err(PacketError::BufferTooSmall));

        // Invalid response code
        let response_buf = [
            0x0f, 0x00, 0x03, // ResponseCode = invalid, Total Length = 3
        ];
        let decoded = ResponsePacket::decode(&response_buf[..], OpCode::PutFinal);
        assert_matches!(decoded, Err(PacketError::ResponseCode(_)));

        // Valid response code with final bit not set.
        let response_buf = [
            0x10, 0x00, 0x03, // ResponseCode = Continue, final bit unset, Total Length = 3
        ];
        let decoded = ResponsePacket::decode(&response_buf[..], OpCode::Disconnect);
        assert_matches!(decoded, Err(PacketError::ResponseCode(_)));

        // Packet length doesn't match specified length
        let response_buf = [0x90, 0x00, 0x04];
        let decoded = ResponsePacket::decode(&response_buf[..], OpCode::ActionFinal);
        assert_matches!(decoded, Err(PacketError::BufferTooSmall));

        // Missing optional data
        let response_buf = [
            0xa0, 0x00, 0x05, // ResponseCode = Ok, Total Length = 5
            0x10, 0x00, // Data: Missing max packet size
        ];
        let decoded = ResponsePacket::decode(&response_buf[..], OpCode::Connect);
        assert_matches!(decoded, Err(PacketError::BufferTooSmall));
    }

    #[fuchsia::test]
    fn encode_connect_response_packet_success() {
        // A CONNECT response with Version = 1.0, Flags = 0, Max packet = 255. No additional headers
        let connect_response = ResponsePacket::new(
            ResponseCode::Accepted,
            vec![0x10, 0x00, 0x00, 0xff],
            HeaderSet::new(),
        );
        assert_eq!(connect_response.encoded_len(), 7);
        let mut buf = vec![0; connect_response.encoded_len()];
        connect_response.encode(&mut buf[..]).expect("can encode response");
        let expected_buf = [
            0xa2, 0x00, 0x07, // Response = Accepted, Total Length = 7
            0x10, 0x00, 0x00, 0xff, // Data
        ];
        assert_eq!(buf, expected_buf);
    }

    #[fuchsia::test]
    fn encode_setpath_response_packet_success() {
        let setpath_response = ResponsePacket::new(ResponseCode::Ok, vec![], HeaderSet::new());
        assert_eq!(setpath_response.encoded_len(), 3);
        let mut buf = vec![0; setpath_response.encoded_len()];
        setpath_response.encode(&mut buf[..]).expect("can encode response");
        let expected_buf = [
            0xa0, 0x00, 0x03, // Response = Ok, Total Length = 3 (no data, headers)
        ];
        assert_eq!(buf, expected_buf);
    }

    #[fuchsia::test]
    fn expect_response_code() {
        let response = ResponsePacket::new_no_data(ResponseCode::Ok, HeaderSet::new());
        assert_matches!(response.clone().expect_code(OpCode::Get, ResponseCode::Ok), Ok(_));
        assert_matches!(
            response.expect_code(OpCode::Get, ResponseCode::Continue),
            Err(Error::PeerRejected { .. })
        );

        let response = ResponsePacket::new_no_data(ResponseCode::Continue, HeaderSet::new());
        assert_matches!(response.clone().expect_code(OpCode::Get, ResponseCode::Continue), Ok(_));
        assert_matches!(
            response.expect_code(OpCode::Get, ResponseCode::Ok),
            Err(Error::PeerRejected { .. })
        );

        let response = ResponsePacket::new_no_data(ResponseCode::Conflict, HeaderSet::new());
        assert_matches!(response.clone().expect_code(OpCode::Get, ResponseCode::Conflict), Ok(_));
        assert_matches!(
            response.expect_code(OpCode::Get, ResponseCode::Ok),
            Err(Error::PeerRejected { .. })
        );
    }

    #[fuchsia::test]
    fn decode_connect_response_packet_success() {
        let connect_response = [
            0xa0, 0x00, 0x0c, // ResponseCode = Ok, Total Length = 12
            0x10, 0x00, 0x12, 0x34, // Data: Version = 0x10, Flags = 0, Max Packet = 0x1234
            0xcb, 0x00, 0x00, 0x00, 0x01, // ConnectionId = 1
        ];
        let decoded = ResponsePacket::decode(&connect_response[..], OpCode::Connect)
            .expect("can decode valid response");
        let expected_headers =
            HeaderSet::from_headers(vec![Header::ConnectionId(ConnectionIdentifier(1))]).unwrap();
        let expected =
            ResponsePacket::new(ResponseCode::Ok, vec![0x10, 0x00, 0x12, 0x34], expected_headers);
        assert_eq!(decoded, expected);
    }

    #[fuchsia::test]
    fn decode_setpath_response_packet_success() {
        let setpath_response = [
            0xc3, 0x00, 0x08, // ResponseCode = Forbidden, Total length = 8
            0xcf, 0x00, 0x00, 0x00, 0x02, // CreatorId = 2.
        ];
        let decoded = ResponsePacket::decode(&setpath_response[..], OpCode::SetPath)
            .expect("can decode valid response");
        let expected_headers = HeaderSet::from_headers(vec![Header::CreatorId(2)]).unwrap();
        let expected = ResponsePacket::new(ResponseCode::Forbidden, vec![], expected_headers);
        assert_eq!(decoded, expected);
    }

    #[fuchsia::test]
    fn decode_setpath_response_packet_additional_data_error() {
        let setpath_response = [
            0xc3, 0x00, 0x0b, // ResponseCode = Forbidden, Total length = 11
            0xaa, 0xbb, 0xcc, // Additional data is not supported in SetPath response.
            0xcf, 0x00, 0x00, 0x00, 0x03, // CreatorId = 3.
        ];
        let decoded = ResponsePacket::decode(&setpath_response[..], OpCode::SetPath);
        assert_matches!(decoded, Err(_));
    }
}
