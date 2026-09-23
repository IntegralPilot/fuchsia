// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Binary frame definitions, wire constants, serialization, and checksums.

use zerocopy::byteorder::big_endian::{U16 as BeU16, U32 as BeU32};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

use crate::error::FrameError;

/// Frame type indicator for unsequenced or sequenced data payloads.
const TYPE_DATA: u8 = 0x01;
/// Frame type indicator for cumulative acknowledgments.
const TYPE_ACK: u8 = 0x02;
/// Frame type indicator for channel termination.
const TYPE_CLOSE: u8 = 0x03;
/// Frame type indicator for transport link reset.
const TYPE_RESET: u8 = 0x04;
/// Frame type indicator for Channel 0 handshake negotiation request.
const TYPE_NEGOTIATE_REQ: u8 = 0x10;
/// Frame type indicator for Channel 0 handshake negotiation response.
const TYPE_NEGOTIATE_RESP: u8 = 0x11;

/// Reserved logical channel identifier for handshake negotiation and link control.
pub const CONTROL_CHANNEL_ID: u16 = 0;

/// Length of the framing preamble sync word in bytes.
pub(crate) const SYNC_WORD_LEN: usize = 2;
/// Length of the IEEE CRC-32 checksum field in bytes.
pub(crate) const CHECKSUM_LEN: usize = 4;

/// Canonical sync preamble bytes for UART protocol frames.
pub(crate) const SYNC_WORD: [u8; SYNC_WORD_LEN] = [0xAA, 0x55];

/// Packed wire format of the 13-byte UART protocol frame header.
#[derive(FromBytes, IntoBytes, KnownLayout, Immutable, Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, packed)]
pub(crate) struct WireHeader {
    /// Preamble synchronization bytes (`[0xAA, 0x55]`).
    pub sync: [u8; SYNC_WORD_LEN],
    /// 32-bit session identifier negotiated during Channel 0 handshake.
    pub session_id: BeU32,
    /// 16-bit logical channel identifier (0 = control).
    pub channel_id: BeU16,
    /// 8-bit Go-Back-N frame sequence number.
    pub seq: u8,
    /// Protocol frame type indicator.
    pub frame_type: u8,
    /// 16-bit payload length in bytes.
    pub payload_len: BeU16,
    /// 8-bit CRC header checksum over preceding header fields.
    pub header_checksum: u8,
}

/// Total wire length of an un-encoded frame header in bytes (13 bytes: sync + header + checksum).
pub(crate) const HEADER_LEN: usize = std::mem::size_of::<WireHeader>();

/// Maximum payload size in bytes accepted by framing encoders and parsers.
pub const MAX_PAYLOAD_SIZE: usize = 1024;

/// Maximum total wire size in bytes of an encoded frame (header + payload + CRC-32).
pub const MAX_FRAME_SIZE: usize = HEADER_LEN + MAX_PAYLOAD_SIZE + CHECKSUM_LEN;

#[cfg(test)]
pub(crate) const PAYLOAD_LEN_OFFSET: usize = std::mem::offset_of!(WireHeader, payload_len);
pub(crate) const HEADER_CHECKSUM_OFFSET: usize = std::mem::offset_of!(WireHeader, header_checksum);

/// Protocol frame type indicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrameType {
    /// Unreliable or reliable payload data frame.
    Data,
    /// Cumulative sequence acknowledgment frame.
    Ack,
    /// Channel termination frame.
    Close,
    /// Transport link reset frame.
    Reset,
    /// Dynamic protocol negotiation request (Channel 0 only).
    NegotiateReq,
    /// Dynamic protocol negotiation response (Channel 0 only).
    NegotiateResp,
    /// Unrecognized frame type received over the wire.
    Unknown(u8),
}

impl From<u8> for FrameType {
    fn from(val: u8) -> Self {
        match val {
            TYPE_DATA => FrameType::Data,
            TYPE_ACK => FrameType::Ack,
            TYPE_CLOSE => FrameType::Close,
            TYPE_RESET => FrameType::Reset,
            TYPE_NEGOTIATE_REQ => FrameType::NegotiateReq,
            TYPE_NEGOTIATE_RESP => FrameType::NegotiateResp,
            other => FrameType::Unknown(other),
        }
    }
}

impl From<FrameType> for u8 {
    fn from(ft: FrameType) -> Self {
        match ft {
            FrameType::Data => TYPE_DATA,
            FrameType::Ack => TYPE_ACK,
            FrameType::Close => TYPE_CLOSE,
            FrameType::Reset => TYPE_RESET,
            FrameType::NegotiateReq => TYPE_NEGOTIATE_REQ,
            FrameType::NegotiateResp => TYPE_NEGOTIATE_RESP,
            FrameType::Unknown(val) => val,
        }
    }
}

impl std::fmt::Display for FrameType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// Protocol version identifier negotiated during handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProtocolId {
    /// ResendSP v1 (Sliding-window Go-Back-N ARQ with CRC-32).
    ResendSP,
    /// Forward-compatible representation of an unknown protocol ID proposed by a future peer.
    Unknown(u32),
}

impl ProtocolId {
    /// Returns the 32-bit wire integer representation of this protocol ID.
    pub fn wire_id(self) -> u32 {
        match self {
            ProtocolId::ResendSP => 1,
            ProtocolId::Unknown(val) => val,
        }
    }
}

impl From<u32> for ProtocolId {
    fn from(val: u32) -> Self {
        match val {
            1 => ProtocolId::ResendSP,
            other => ProtocolId::Unknown(other),
        }
    }
}

impl From<ProtocolId> for u32 {
    fn from(proto: ProtocolId) -> Self {
        proto.wire_id()
    }
}

impl std::fmt::Display for ProtocolId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtocolId::ResendSP => write!(f, "ResendSP"),
            ProtocolId::Unknown(id) => write!(f, "Unknown({id})"),
        }
    }
}

impl Default for ProtocolId {
    fn default() -> Self {
        ProtocolId::ResendSP
    }
}

/// Wire representation of a parsed UART protocol frame emitted by [`crate::FrameParser`].
///
/// While `Frame` represents an in-memory parsed frame, [`encode_frame`] is the primary
/// zero-allocation serializer for transmitting byte slices over the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// 32-bit session identifier negotiated during handshake.
    pub session_id: u32,
    /// Logical channel identifier (Channel 0 is reserved for control).
    pub channel_id: u16,
    /// 8-bit Go-Back-N sequence number.
    pub seq: u8,
    /// Protocol frame type indicator.
    pub frame_type: FrameType,
    /// Raw un-encoded payload bytes.
    pub payload: Vec<u8>,
}

impl Frame {
    /// Creates a new validated protocol frame.
    ///
    /// # Errors
    /// Returns [`FrameError::PayloadTooLarge`] if `payload.len() > MAX_PAYLOAD_SIZE`.
    pub fn new(
        session_id: u32,
        channel_id: u16,
        seq: u8,
        frame_type: FrameType,
        payload: Vec<u8>,
    ) -> Result<Self, FrameError> {
        if payload.len() > MAX_PAYLOAD_SIZE {
            return Err(FrameError::PayloadTooLarge(payload.len()));
        }
        Ok(Self { session_id, channel_id, seq, frame_type, payload })
    }

    /// Serializes and encodes this frame with header, header checksum, and trailing CRC-32.
    ///
    /// # Errors
    /// Returns [`FrameError::PayloadTooLarge`] if `self.payload.len() > MAX_PAYLOAD_SIZE`.
    pub fn encode(&self) -> Result<Vec<u8>, FrameError> {
        encode_frame(self.session_id, self.channel_id, self.seq, self.frame_type, &self.payload)
    }

    /// Returns the total wire length of this frame in bytes when encoded.
    pub fn wire_len(&self) -> usize {
        HEADER_LEN + self.payload.len() + CHECKSUM_LEN
    }
}

/// Computes the IEEE 802.3 CRC-32 checksum of a byte slice.
pub(crate) fn crc32(data: &[u8]) -> u32 {
    crc32fast::hash(data)
}

/// Computes an 8-bit CRC over data using polynomial 0x07 (matching Pigweed pw_ulink).
pub(crate) fn crc8(data: &[u8]) -> u8 {
    let mut crc: u8 = 0x00;
    for &byte in data {
        crc ^= byte;
        for _ in 0..8 {
            if (crc & 0x80) != 0 {
                crc = (crc << 1) ^ 0x07;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

/// Serializes and encodes a protocol frame with the 13-byte binary header,
/// 1-byte header checksum (`HdrChk`), and trailing CRC-32 checksum.
///
/// Layout: `[Sync (2B)] + [Session (4B)] + [Channel (2B)] + [Seq (1B)] + [Type (1B)] + [Len (2B)] + [HdrChk (1B)] + [Payload (L B)] + [CRC-32 (4B)]`
///
/// # Errors
/// Returns [`FrameError::PayloadTooLarge`] if `payload.len() > MAX_PAYLOAD_SIZE`.
pub fn encode_frame(
    session_id: u32,
    channel_id: u16,
    seq: u8,
    frame_type: FrameType,
    payload: &[u8],
) -> Result<Vec<u8>, FrameError> {
    if payload.len() > MAX_PAYLOAD_SIZE {
        return Err(FrameError::PayloadTooLarge(payload.len()));
    }

    let mut header = WireHeader {
        sync: SYNC_WORD,
        session_id: BeU32::new(session_id),
        channel_id: BeU16::new(channel_id),
        seq,
        frame_type: frame_type.into(),
        payload_len: BeU16::new(payload.len() as u16),
        header_checksum: 0,
    };
    let header_bytes = header.as_bytes();
    header.header_checksum = crc8(&header_bytes[SYNC_WORD_LEN..HEADER_CHECKSUM_OFFSET]);

    let mut frame = Vec::with_capacity(HEADER_LEN + payload.len() + CHECKSUM_LEN);
    frame.extend_from_slice(header.as_bytes());
    frame.extend_from_slice(payload);

    let cksum = crc32(&frame[SYNC_WORD_LEN..]);
    frame.extend_from_slice(&cksum.to_be_bytes());

    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    fn test_crc32() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xCBF43926);
    }

    #[fuchsia::test]
    fn test_crc8() {
        assert_eq!(crc8(b""), 0);
        assert_eq!(crc8(b"123456789"), 0xF4);
    }

    #[fuchsia::test]
    fn test_encode_frame() {
        let payload = b"hello";
        let encoded = encode_frame(0x12345678, 1, 0, FrameType::Data, payload)
            .expect("payload length is within MAX_PAYLOAD_SIZE");
        assert_eq!(encoded[0], 0xAA);
        assert_eq!(encoded[1], 0x55);
        assert_eq!(encoded.len(), HEADER_LEN + payload.len() + CHECKSUM_LEN);
    }

    #[fuchsia::test]
    fn test_encode_frame_payload_exceeding_max_payload_size() {
        let oversized = vec![0u8; MAX_PAYLOAD_SIZE + 1];
        let res = encode_frame(1, 1, 1, FrameType::Data, &oversized);
        assert_eq!(res, Err(FrameError::PayloadTooLarge(MAX_PAYLOAD_SIZE + 1)));
    }

    #[fuchsia::test]
    fn test_encode_frame_max_payload_boundary() {
        let max_payload = vec![0x42u8; MAX_PAYLOAD_SIZE];
        let encoded = encode_frame(1, 1, 1, FrameType::Data, &max_payload)
            .expect("MAX_PAYLOAD_SIZE boundary must encode successfully");
        assert_eq!(encoded.len(), HEADER_LEN + MAX_PAYLOAD_SIZE + CHECKSUM_LEN);
    }

    #[fuchsia::test]
    fn test_frame_type_conversions() {
        assert_eq!(FrameType::from(0x01), FrameType::Data);
        assert_eq!(FrameType::from(0x02), FrameType::Ack);
        assert_eq!(FrameType::from(0x03), FrameType::Close);
        assert_eq!(FrameType::from(0x04), FrameType::Reset);
        assert_eq!(FrameType::from(0x10), FrameType::NegotiateReq);
        assert_eq!(FrameType::from(0x11), FrameType::NegotiateResp);
        assert_eq!(FrameType::from(0x99), FrameType::Unknown(0x99));

        assert_eq!(u8::from(FrameType::Data), 0x01);
        assert_eq!(u8::from(FrameType::Ack), 0x02);
        assert_eq!(u8::from(FrameType::Close), 0x03);
        assert_eq!(u8::from(FrameType::Reset), 0x04);
        assert_eq!(u8::from(FrameType::NegotiateReq), 0x10);
        assert_eq!(u8::from(FrameType::NegotiateResp), 0x11);
        assert_eq!(u8::from(FrameType::Unknown(0x99)), 0x99);

        assert_eq!(FrameType::Data.to_string(), "Data");
        assert_eq!(FrameType::Ack.to_string(), "Ack");
        assert_eq!(FrameType::Close.to_string(), "Close");
        assert_eq!(FrameType::Reset.to_string(), "Reset");
        assert_eq!(FrameType::NegotiateReq.to_string(), "NegotiateReq");
        assert_eq!(FrameType::NegotiateResp.to_string(), "NegotiateResp");
        assert_eq!(FrameType::Unknown(0x99).to_string(), "Unknown(153)");
    }

    #[fuchsia::test]
    fn test_protocol_id_conversions() {
        assert_eq!(ProtocolId::from(1), ProtocolId::ResendSP);
        assert_eq!(ProtocolId::from(42), ProtocolId::Unknown(42));

        assert_eq!(ProtocolId::default(), ProtocolId::ResendSP);

        assert_eq!(ProtocolId::ResendSP.wire_id(), 1);
        assert_eq!(ProtocolId::Unknown(42).wire_id(), 42);

        assert_eq!(ProtocolId::ResendSP.to_string(), "ResendSP");
        assert_eq!(ProtocolId::Unknown(42).to_string(), "Unknown(42)");
    }

    #[fuchsia::test]
    fn test_frame_inherent_methods() {
        let payload = b"payload".to_vec();
        let frame = Frame::new(42, CONTROL_CHANNEL_ID, 7, FrameType::Data, payload.clone())
            .expect("valid frame creation");
        assert_eq!(frame.session_id, 42);
        assert_eq!(frame.channel_id, CONTROL_CHANNEL_ID);
        assert_eq!(frame.seq, 7);
        assert_eq!(frame.frame_type, FrameType::Data);
        assert_eq!(frame.payload, payload);
        assert_eq!(frame.wire_len(), HEADER_LEN + payload.len() + CHECKSUM_LEN);

        let encoded = frame.encode().expect("encode succeeds");
        assert_eq!(encoded.len(), frame.wire_len());

        let oversized = vec![0u8; MAX_PAYLOAD_SIZE + 1];
        let err = Frame::new(42, 1, 0, FrameType::Data, oversized).unwrap_err();
        assert_eq!(err, FrameError::PayloadTooLarge(MAX_PAYLOAD_SIZE + 1));
    }
}
