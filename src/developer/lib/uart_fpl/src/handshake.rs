// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Channel 0 dynamic protocol negotiation state machines and binary serialization.

use crate::error::HandshakeError;
use crate::frame::{FrameType, ProtocolId};

const STATUS_SUCCESS: u8 = 0;
const STATUS_NO_COMMON_PROTOCOL: u8 = 1;
const STATUS_MALFORMED_REQUEST: u8 = 2;

const PROTOCOL_COUNT_LEN: usize = 1;
const PROTOCOL_ID_LEN: usize = std::mem::size_of::<u32>();
const STATUS_LEN: usize = 1;
const RESPONSE_LEN: usize = STATUS_LEN + PROTOCOL_ID_LEN;

/// Negotiation status reported by the target in a handshake response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeStatus {
    /// Negotiation succeeded; a mutually supported protocol was selected.
    Success,
    /// Negotiation failed; no common protocol exists between host and target.
    NoCommonProtocol,
    /// Request payload was malformed or could not be parsed.
    MalformedRequest,
    /// Unrecognized handshake status reported by peer.
    Unknown(u8),
}

impl From<u8> for HandshakeStatus {
    fn from(val: u8) -> Self {
        match val {
            STATUS_SUCCESS => HandshakeStatus::Success,
            STATUS_NO_COMMON_PROTOCOL => HandshakeStatus::NoCommonProtocol,
            STATUS_MALFORMED_REQUEST => HandshakeStatus::MalformedRequest,
            other => HandshakeStatus::Unknown(other),
        }
    }
}

impl From<HandshakeStatus> for u8 {
    fn from(status: HandshakeStatus) -> Self {
        match status {
            HandshakeStatus::Success => STATUS_SUCCESS,
            HandshakeStatus::NoCommonProtocol => STATUS_NO_COMMON_PROTOCOL,
            HandshakeStatus::MalformedRequest => STATUS_MALFORMED_REQUEST,
            HandshakeStatus::Unknown(val) => val,
        }
    }
}

/// Structured handshake request containing a prioritized list of proposed protocols.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HandshakeRequest {
    /// Proposed protocols in host preference order.
    pub protocols: Vec<ProtocolId>,
}

impl HandshakeRequest {
    /// Serializes the request into wire bytes: `[count (1B), proto0 (4B BE), ...]`.
    pub fn serialize(&self) -> Result<Vec<u8>, HandshakeError> {
        if self.protocols.len() > u8::MAX as usize {
            return Err(HandshakeError::MalformedPayload);
        }
        let mut data =
            Vec::with_capacity(PROTOCOL_COUNT_LEN + PROTOCOL_ID_LEN * self.protocols.len());
        data.push(self.protocols.len() as u8);
        for &proto in &self.protocols {
            data.extend_from_slice(&proto.wire_id().to_be_bytes());
        }
        Ok(data)
    }
}

impl TryFrom<&[u8]> for HandshakeRequest {
    type Error = HandshakeError;

    /// Deserializes a request from wire bytes.
    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        if bytes.len() < PROTOCOL_COUNT_LEN {
            return Err(HandshakeError::MalformedPayload);
        }
        let count = bytes[0] as usize;
        let expected_len = PROTOCOL_COUNT_LEN + PROTOCOL_ID_LEN * count;
        if bytes.len() < expected_len {
            return Err(HandshakeError::MalformedPayload);
        }
        let protocols = bytes[PROTOCOL_COUNT_LEN..expected_len]
            .chunks_exact(PROTOCOL_ID_LEN)
            .map(|chunk| ProtocolId::from(u32::from_be_bytes(chunk.try_into().unwrap())))
            .collect();
        Ok(Self { protocols })
    }
}

/// Structured handshake response containing negotiation status and selected protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandshakeResponse {
    /// Result status of negotiation.
    pub status: HandshakeStatus,
    /// Mutually selected protocol, if successful.
    pub selected: Option<ProtocolId>,
}

impl HandshakeResponse {
    /// Serializes the response into wire bytes: `[status (1B), selected_proto (4B BE)]`.
    ///
    /// # Errors
    /// Currently infallible for valid status values, but returns [`HandshakeError`]
    /// if binary serialization invariants are violated.
    pub fn serialize(&self) -> Result<Vec<u8>, HandshakeError> {
        let mut data = Vec::with_capacity(RESPONSE_LEN);
        data.push(u8::from(self.status));
        let wire_id = self.selected.map(|p| p.wire_id()).unwrap_or(0);
        data.extend_from_slice(&wire_id.to_be_bytes());
        Ok(data)
    }
}

impl TryFrom<&[u8]> for HandshakeResponse {
    type Error = HandshakeError;

    /// Deserializes a response from wire bytes.
    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        if bytes.len() < RESPONSE_LEN {
            return Err(HandshakeError::MalformedPayload);
        }
        let status = HandshakeStatus::from(bytes[0]);
        let val = u32::from_be_bytes(bytes[STATUS_LEN..RESPONSE_LEN].try_into().unwrap());
        let selected =
            if status == HandshakeStatus::Success { Some(ProtocolId::from(val)) } else { None };
        Ok(Self { status, selected })
    }
}

/// Host-side handshake coordinator.
#[derive(Debug, Clone)]
pub struct HostHandshake {
    proposed: Vec<ProtocolId>,
}

impl HostHandshake {
    /// Creates a new host handshake coordinator with the list of proposed protocols in priority order.
    pub fn new(proposed: Vec<ProtocolId>) -> Self {
        Self { proposed }
    }

    /// Returns the list of proposed protocols.
    pub fn proposed(&self) -> &[ProtocolId] {
        &self.proposed
    }

    /// Generates the initial negotiation request frame type and serialized payload.
    ///
    /// # Errors
    /// Returns [`HandshakeError::MalformedPayload`] if the number of proposed protocols
    /// exceeds `u8::MAX`.
    pub fn start(&self) -> Result<(FrameType, Vec<u8>), HandshakeError> {
        let req = HandshakeRequest { protocols: self.proposed.clone() };
        Ok((FrameType::NegotiateReq, req.serialize()?))
    }

    /// Evaluates a target response payload and returns the mutually agreed protocol.
    ///
    /// # Errors
    /// - [`HandshakeError::MalformedPayload`]: If `payload` is shorter than 5 bytes or cannot be parsed.
    /// - [`HandshakeError::NoCommonProtocol`]: If target reported no mutually supported protocol.
    /// - [`HandshakeError::ProtocolNotProposed`]: If target selected a protocol not proposed by this host.
    /// - [`HandshakeError::UnsupportedStatus`]: If target returned an unrecognized status code.
    pub fn handle_response(&self, payload: &[u8]) -> Result<ProtocolId, HandshakeError> {
        let resp = HandshakeResponse::try_from(payload)?;
        match resp.status {
            HandshakeStatus::Success => {
                let selected = resp.selected.ok_or(HandshakeError::NoCommonProtocol)?;
                if self.proposed.contains(&selected) {
                    Ok(selected)
                } else {
                    Err(HandshakeError::ProtocolNotProposed)
                }
            }
            HandshakeStatus::NoCommonProtocol => Err(HandshakeError::NoCommonProtocol),
            HandshakeStatus::MalformedRequest => Err(HandshakeError::MalformedPayload),
            HandshakeStatus::Unknown(code) => Err(HandshakeError::UnsupportedStatus(code)),
        }
    }
}

/// Target-side handshake coordinator.
///
/// # Idempotent Handshake Retransmission Contract
/// In lossy UART links, the target's initial [`HandshakeResponse`] may be dropped or
/// corrupted in flight. When this occurs, the host's timeout will cause it to retransmit
/// the initial `NegotiateReq` with the identical `session_id`.
///
/// Target implementations (e.g. `receiver_task`) MUST treat repeated `NegotiateReq`
/// frames with the active `session_id` as idempotent retransmissions: the target must
/// re-emit the cached `NegotiateResp` frame rather than rejecting the frame as unexpected
/// or resetting the transport state. If a `NegotiateReq` arrives with a *new* session ID,
/// the target must reset and negotiate the new session.
#[derive(Debug, Clone)]
pub struct TargetHandshake {
    supported: Vec<ProtocolId>,
}

impl TargetHandshake {
    /// Creates a new target handshake handler with the list of supported protocols.
    pub fn new(supported: Vec<ProtocolId>) -> Self {
        Self { supported }
    }

    /// Returns the list of supported protocols.
    pub fn supported(&self) -> &[ProtocolId] {
        &self.supported
    }

    /// Evaluates an incoming host request payload and generates the serialized response.
    /// Returns `(selected_protocol, response_payload)`.
    ///
    /// # Errors
    /// Returns [`HandshakeError::MalformedPayload`] if `payload` cannot be decoded into
    /// a valid [`HandshakeRequest`].
    pub fn handle_request(
        &self,
        payload: &[u8],
    ) -> Result<(Option<ProtocolId>, Vec<u8>), HandshakeError> {
        let req = HandshakeRequest::try_from(payload)?;
        let selected = req.protocols.into_iter().find(|p| self.supported.contains(p));
        let status = if selected.is_some() {
            HandshakeStatus::Success
        } else {
            HandshakeStatus::NoCommonProtocol
        };
        let resp = HandshakeResponse { status, selected };
        Ok((selected, resp.serialize()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_handshake(
        host_proposed: Vec<ProtocolId>,
        target_supported: Vec<ProtocolId>,
    ) -> Result<ProtocolId, HandshakeError> {
        let host = HostHandshake::new(host_proposed);
        let target = TargetHandshake::new(target_supported);

        let (_, req_payload) = host.start()?;
        let (selected, resp_payload) = target.handle_request(&req_payload)?;
        if selected.is_some() {
            assert_eq!(resp_payload[0], u8::from(HandshakeStatus::Success));
        }

        host.handle_response(&resp_payload)
    }

    #[fuchsia::test]
    fn test_handshake_matrix() {
        struct TestCase {
            host: Vec<ProtocolId>,
            target: Vec<ProtocolId>,
            expected: Result<ProtocolId, HandshakeError>,
        }

        let cases = [
            TestCase {
                host: vec![ProtocolId::ResendSP],
                target: vec![ProtocolId::ResendSP],
                expected: Ok(ProtocolId::ResendSP),
            },
            TestCase {
                host: vec![ProtocolId::Unknown(99)],
                target: vec![ProtocolId::Unknown(99)],
                expected: Ok(ProtocolId::Unknown(99)),
            },
            // Host priority ordering: host prefers Unknown(99) over ResendSP
            TestCase {
                host: vec![ProtocolId::Unknown(99), ProtocolId::ResendSP],
                target: vec![ProtocolId::ResendSP, ProtocolId::Unknown(99)],
                expected: Ok(ProtocolId::Unknown(99)),
            },
            // Forward compatibility: host proposes unknown protocol first, target selects known fallback
            TestCase {
                host: vec![ProtocolId::Unknown(999), ProtocolId::ResendSP],
                target: vec![ProtocolId::ResendSP],
                expected: Ok(ProtocolId::ResendSP),
            },
            // Forward compatibility: both support custom/unknown protocol
            TestCase {
                host: vec![ProtocolId::Unknown(42)],
                target: vec![ProtocolId::Unknown(42)],
                expected: Ok(ProtocolId::Unknown(42)),
            },
            // Incompatible protocols
            TestCase {
                host: vec![ProtocolId::ResendSP],
                target: vec![ProtocolId::Unknown(99)],
                expected: Err(HandshakeError::NoCommonProtocol),
            },
            // Target supports nothing
            TestCase {
                host: vec![ProtocolId::ResendSP],
                target: vec![],
                expected: Err(HandshakeError::NoCommonProtocol),
            },
        ];

        for (i, case) in cases.into_iter().enumerate() {
            let result = run_handshake(case.host, case.target);
            assert_eq!(result, case.expected, "Failed test case {i}");
        }
    }

    #[fuchsia::test]
    fn test_unproposed_protocol_selection_rejected_by_host() {
        let host = HostHandshake::new(vec![ProtocolId::ResendSP]);
        let mut fake_resp = Vec::new();
        fake_resp.push(u8::from(HandshakeStatus::Success));
        fake_resp.extend_from_slice(&ProtocolId::Unknown(42).wire_id().to_be_bytes());

        assert_eq!(host.handle_response(&fake_resp), Err(HandshakeError::ProtocolNotProposed));
    }

    #[fuchsia::test]
    fn test_malformed_payload_rejections() {
        let target = TargetHandshake::new(vec![ProtocolId::ResendSP]);
        assert_eq!(target.handle_request(&[][..]), Err(HandshakeError::MalformedPayload));
        // Count claims 2 protocols (expected 1 + 8 = 9 bytes), but only 5 bytes provided
        assert_eq!(
            target.handle_request(&[2, 0, 0, 0, 1][..]),
            Err(HandshakeError::MalformedPayload)
        );

        let host = HostHandshake::new(vec![ProtocolId::ResendSP]);
        assert_eq!(host.handle_response(&[][..]), Err(HandshakeError::MalformedPayload));
        assert_eq!(host.handle_response(&[0; 4][..]), Err(HandshakeError::MalformedPayload));

        // Host proposals exceeding u8::MAX
        let overflow_host = HostHandshake::new(vec![ProtocolId::ResendSP; 256]);
        assert_eq!(overflow_host.start(), Err(HandshakeError::MalformedPayload));
    }

    #[fuchsia::test]
    fn test_unsupported_status_rejection() {
        let host = HostHandshake::new(vec![ProtocolId::ResendSP]);
        let fake_resp = vec![99u8, 0, 0, 0, 1];
        assert_eq!(host.handle_response(&fake_resp), Err(HandshakeError::UnsupportedStatus(99)));

        assert_eq!(HandshakeStatus::from(99), HandshakeStatus::Unknown(99));
        assert_eq!(u8::from(HandshakeStatus::Unknown(99)), 99);
    }
}
