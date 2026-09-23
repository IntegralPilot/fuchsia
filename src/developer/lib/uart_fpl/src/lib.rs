// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(unsafe_code)]

//! UART Framing Protocol Library (`uart_fpl`).
//!
//! Provides binary packet framing, streaming parsing with preamble synchronization,
//! CRC-8 header protection, Channel 0 dynamic protocol negotiation, and
//! sliding-window flow control primitives for reliable transport over serial links.

mod error;
mod frame;
mod handshake;
mod parser;
mod resend_sp;
mod window;

pub use error::{FrameError, HandshakeError, RetransmissionLimitExceeded, UnexpectedSeqError};
pub use frame::{
    CONTROL_CHANNEL_ID, Frame, FrameType, MAX_FRAME_SIZE, MAX_PAYLOAD_SIZE, ProtocolId,
    encode_frame,
};
pub use handshake::{HandshakeResponse, HandshakeStatus, HostHandshake, TargetHandshake};
pub use parser::FrameParser;
pub use resend_sp::{
    AckOutcome, DEFAULT_MAX_RETRANSMISSION_ATTEMPTS, DEFAULT_RETRANSMISSION_TIMEOUT,
    DEFAULT_WINDOW_SIZE, FrameStatus, ResendReceiver, ResendSender,
};
pub use window::AckTracker;

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_frame_eq(
        frame: &Frame,
        session_id: u32,
        channel_id: u16,
        seq: u8,
        frame_type: FrameType,
        payload: &[u8],
    ) {
        assert_eq!(frame.session_id, session_id);
        assert_eq!(frame.channel_id, channel_id);
        assert_eq!(frame.seq, seq);
        assert_eq!(frame.frame_type, frame_type);
        assert_eq!(frame.payload, payload);
    }

    #[fuchsia::test]
    fn test_complete_flow_negotiation_and_traffic() {
        let host = HostHandshake::new(vec![ProtocolId::ResendSP]);
        let target = TargetHandshake::new(vec![ProtocolId::ResendSP]);

        let (req_type, req_payload) = host.start().unwrap();
        let req_frame = encode_frame(100, 0, 0, req_type, &req_payload).unwrap();

        let mut target_parser = FrameParser::new();
        target_parser.feed(&req_frame);
        let parsed_req = target_parser.next_frame().unwrap();
        assert_frame_eq(&parsed_req, 100, 0, 0, FrameType::NegotiateReq, &req_payload);

        let (selected, resp_payload) = target.handle_request(&parsed_req.payload).unwrap();
        assert_eq!(selected, Some(ProtocolId::ResendSP));

        let resp_frame = encode_frame(100, 0, 1, FrameType::NegotiateResp, &resp_payload).unwrap();
        let mut host_parser = FrameParser::new();
        host_parser.feed(&resp_frame);
        let parsed_resp = host_parser.next_frame().unwrap();
        assert_frame_eq(&parsed_resp, 100, 0, 1, FrameType::NegotiateResp, &resp_payload);

        let negotiated = host.handle_response(&parsed_resp.payload).unwrap();
        assert_eq!(negotiated, ProtocolId::ResendSP);

        // Traffic exchange on logical channel
        let data_frame = encode_frame(100, 1, 0, FrameType::Data, b"payload_data").unwrap();
        target_parser.feed(&data_frame);
        let parsed_data = target_parser.next_frame().unwrap();
        assert_frame_eq(&parsed_data, 100, 1, 0, FrameType::Data, b"payload_data");
    }

    #[fuchsia::test]
    fn test_host_reboot_recovery_flow() {
        let host = HostHandshake::new(vec![ProtocolId::ResendSP]);
        let target = TargetHandshake::new(vec![ProtocolId::ResendSP]);
        let mut target_parser = FrameParser::new();
        let mut host_parser = FrameParser::new();

        let (req_type, req_payload) = host.start().unwrap();
        let req_frame = encode_frame(111, 0, 0, req_type, &req_payload).unwrap();
        target_parser.feed(&req_frame);
        let parsed_req = target_parser.next_frame().unwrap();
        let (selected, resp_payload) = target.handle_request(&parsed_req.payload).unwrap();
        assert_eq!(selected, Some(ProtocolId::ResendSP));

        let resp_frame = encode_frame(111, 0, 1, FrameType::NegotiateResp, &resp_payload).unwrap();
        host_parser.feed(&resp_frame);
        let parsed_resp = host_parser.next_frame().unwrap();
        let negotiated = host.handle_response(&parsed_resp.payload).unwrap();
        assert_eq!(negotiated, ProtocolId::ResendSP);

        let data_frame = encode_frame(111, 1, 2, FrameType::Data, b"hello").unwrap();
        target_parser.feed(&data_frame);
        let parsed_data = target_parser.next_frame().unwrap();
        assert_eq!(parsed_data.payload, b"hello");

        // Host reboots and starts new session 222
        let host_rebooted = HostHandshake::new(vec![ProtocolId::ResendSP]);
        let (req_type_2, req_payload_2) = host_rebooted.start().unwrap();
        let req_frame_2 = encode_frame(222, 0, 0, req_type_2, &req_payload_2).unwrap();

        target_parser.feed(&req_frame_2);
        let parsed_req_2 = target_parser.next_frame().expect("Target parses reboot handshake");
        assert_eq!(parsed_req_2.session_id, 222);
        assert_eq!(parsed_req_2.channel_id, 0);
        assert_eq!(parsed_req_2.frame_type, FrameType::NegotiateReq);
        let (selected_2, _) = target.handle_request(&parsed_req_2.payload).unwrap();
        assert_eq!(selected_2, Some(ProtocolId::ResendSP));
    }

    #[test]
    fn test_constants() {
        assert_eq!(MAX_FRAME_SIZE, 13 + 1024 + 4);
        assert_eq!(DEFAULT_WINDOW_SIZE, 64);
        assert_eq!(DEFAULT_MAX_RETRANSMISSION_ATTEMPTS, 60);
    }
}
