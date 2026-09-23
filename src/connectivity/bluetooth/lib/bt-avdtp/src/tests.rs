// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assert_matches::assert_matches;
use async_utils::PollExt;
use bt_channel_test_support::{Transport, create_test_channels};
use fuchsia_async as fasync;
use futures::{SinkExt, StreamExt, TryStreamExt, future};
use std::result;
use test_case::test_case;
use zx::{self as zx};

use crate::*;

// Helper functions

pub(crate) fn setup_peer(transport_mode: Transport) -> (Peer, Channel) {
    let (signaling, remote) = create_test_channels(transport_mode);

    let peer = Peer::new(signaling);
    (peer, remote)
}

fn setup_stream_test(
    transport_mode: Transport,
) -> (fasync::TestExecutor, RequestStream, Peer, Channel) {
    let exec = fasync::TestExecutor::new();
    let (peer, remote) = setup_peer(transport_mode);
    let stream = peer.take_request_stream();
    (exec, stream, peer, remote)
}

fn next_request(stream: &mut RequestStream, exec: &mut fasync::TestExecutor) -> Request {
    let mut fut = stream.next();
    let complete = exec.run_until_stalled(&mut fut);

    match complete {
        Poll::Ready(Some(Ok(r))) => r,
        _ => panic!("should have a request"),
    }
}

pub(crate) fn recv_remote(
    exec: &mut fasync::TestExecutor,
    remote: &mut Channel,
) -> result::Result<Vec<u8>, zx::Status> {
    let mut fut = remote.next();
    match exec.run_until_stalled(&mut fut) {
        Poll::Ready(Some(res)) => res,
        Poll::Ready(None) => Err(zx::Status::PEER_CLOSED),
        Poll::Pending => Err(zx::Status::SHOULD_WAIT),
    }
}

pub(crate) fn expect_remote_recv(
    exec: &mut fasync::TestExecutor,
    expected: &[u8],
    remote: &mut Channel,
) {
    let mut fut = remote.next();
    let poll_res = exec.run_until_stalled(&mut fut);
    let r = match poll_res {
        Poll::Ready(Some(res)) => res,
        Poll::Ready(None) => panic!("stream ended"),
        Poll::Pending => panic!("poll not ready"),
    };
    let response = r.expect("successful read");
    assert_eq!(
        expected.len(),
        response.len(),
        "received wrong length\nexpected: {expected:?}\nreceived: {response:?}"
    );
    assert_eq!(expected, &response[..]);
}

fn stream_request_response(
    exec: &mut fasync::TestExecutor,
    stream: &mut RequestStream,
    remote: &mut Channel,
    cmd: &[u8],
    expect: &[u8],
) {
    let mut write_fut = remote.send(cmd.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let mut fut = stream.next();
    let complete = exec.run_until_stalled(&mut fut);

    // We shouldn't have received anything on the stream.
    assert!(complete.is_pending());

    // The peer should have responded with the expected data.
    expect_remote_recv(exec, expect, remote);
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn closes_socket_when_dropped(transport_mode: Transport) {
    let mut exec = fasync::TestExecutor::new();
    let (signaling, mut peer_chan) = create_test_channels(transport_mode);

    {
        let peer = Peer::new(signaling);
        let mut _stream = peer.take_request_stream();
    }

    // Writing to the sock from the other end should fail.
    let mut write_fut = peer_chan.send(vec![0; 1]);
    match exec.run_until_stalled(&mut write_fut) {
        Poll::Ready(Err(e)) => assert_eq!(zx::Status::PEER_CLOSED, e),
        x => panic!("Expected Ready Err but got {:?}", x),
    }
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
#[should_panic]
fn can_only_take_stream_once(transport_mode: Transport) {
    let _exec = fasync::TestExecutor::new();
    let (signaling, _) = create_test_channels(transport_mode);

    let peer = Peer::new(signaling);
    let _stream = peer.take_request_stream();
    let _ = peer.take_request_stream();
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn terminates_on_peer_closed(transport_mode: Transport) {
    let mut exec = fasync::TestExecutor::new();
    let (signaling, remote) = create_test_channels(transport_mode);

    let peer = Peer::new(signaling);
    let mut stream = peer.take_request_stream();

    assert!(!stream.is_terminated());

    assert!(exec.run_until_stalled(&mut stream.next()).is_pending());

    assert!(!stream.is_terminated());

    drop(remote);
    match exec.run_until_stalled(&mut stream.next()) {
        Poll::Ready(None) => {}
        x => panic!("Expected stream to be ready with none, got {x:?}"),
    }
    assert!(stream.is_terminated());
}

// Generic Request tests

const CMD_DISCOVER: &'static [u8] = &[
    0x40, // TxLabel (4), Single Packet (0b00), Command (0b00)
    0x01, // RFA (0b00), Discover (0x01)
];

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn closed_peer_ends_request_stream(transport_mode: Transport) {
    let (mut exec, stream, _, _remote) = setup_stream_test(transport_mode);
    drop(_remote);
    let mut collect_fut = Box::pin(stream.collect::<Vec<Result<Request>>>());
    match exec.run_until_stalled(&mut collect_fut) {
        Poll::Ready(collected) => {
            assert_eq!(0, collected.len());
        }
        Poll::Pending => panic!("Expected stream to close"),
    }
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn command_not_supported_response(transport_mode: Transport) {
    let (mut exec, mut stream, _, mut remote) = setup_stream_test(transport_mode);

    // TxLabel 4, SecurityControl, which is not implemented
    let mut write_fut = remote.send(vec![0x40, 0x0B, 0x40, 0x00, 0x00]);
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let mut fut = stream.next();
    let complete = exec.run_until_stalled(&mut fut);

    // We shouldn't have received anything on the future
    assert!(complete.is_pending());

    // The peer should have responded with a Response Reject message with the
    // same TxLabel with NOT_SUPPORTED_COMMAND
    expect_remote_recv(&mut exec, &[0x43, 0x0B, 0x19], &mut remote);
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn requests_are_queued_if_they_arrive_early(transport_mode: Transport) {
    let (mut exec, stream, _, mut remote) = setup_stream_test(transport_mode);
    let mut write_fut = remote.send(CMD_DISCOVER.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");
    let mut collected = Vec::<Request>::new();

    let mut fut = stream.try_for_each(|r| {
        collected.push(r);
        future::ready(Ok(()))
    });

    let _complete = exec.run_until_stalled(&mut fut);

    assert_eq!(1, collected.len());
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn responds_with_same_tx_id(transport_mode: Transport) {
    let (mut exec, mut stream, _, mut remote) = setup_stream_test(transport_mode);

    let mut write_fut = remote.send(CMD_DISCOVER.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let respond_res = match next_request(&mut stream, &mut exec) {
        Request::Discover { responder } => {
            let s = StreamInformation::new(
                StreamEndpointId(0x0A), // from CMD_DISCOVER
                false,
                MediaType::Video,
                EndpointType::Source,
            );
            responder.send(&[s])
        }
        _ => panic!("should have received a Discover"),
    };

    assert!(respond_res.is_ok());

    let response: &[u8] = &[
        // txlabel (same as CMD_DISCOVER, 4), Single (0b00), Response Accept (0b10)
        0x4 << 4 | 0x0 << 2 | 0x2,
        0x01,                 // Discover
        0x0A << 2 | 0x0 << 1, // SEID (0A), Not In Use (0b0)
        0x01 << 4 | 0x0 << 3, // Video (0x01), Source (0x00)
    ];

    expect_remote_recv(&mut exec, response, &mut remote);
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn invalid_signal_id_responds_error(transport_mode: Transport) {
    let (mut exec, mut stream, _, mut remote) = setup_stream_test(transport_mode);

    // This is TxLabel 4, Signal Id 0b110000, which is invalid.
    let mut write_fut = remote.send(vec![0x40, 0x30]);
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let mut fut = stream.next();
    let complete = exec.run_until_stalled(&mut fut);

    // We shouldn't have received anything on the future
    assert!(complete.is_pending());

    // The peer should have responded with a General Reject message with the
    // same TxLabel, and the same (invalid) signal identifier.
    expect_remote_recv(&mut exec, &[0x41, 0x30], &mut remote);
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn exhaust_request_ids(transport_mode: Transport) {
    let mut exec = fasync::TestExecutor::new();
    let (peer, mut remote) = setup_peer(transport_mode);
    let mut response_futures = Vec::new();
    // There are only 16 labels, so fill up the "outgoing requests pending" buffer
    for _ in 0..16 {
        let mut response_fut = Box::pin(peer.discover());
        assert!(exec.run_until_stalled(&mut response_fut).is_pending());
        response_futures.push(response_fut);
    }

    let mut should_fail_fut = Box::pin(peer.discover());
    let res = exec.run_until_stalled(&mut should_fail_fut);
    assert!(res.is_ready());

    if let Poll::Ready(x) = res {
        assert!(x.is_err());
    }

    // Finish some of them.
    for _ in 0..4 {
        let received = recv_remote(&mut exec, &mut remote).unwrap();
        // Last half of header must be Single (0b00) and Command (0b00)
        assert_eq!(0x00, received[0] & 0xF);
        assert_eq!(0x01, received[1]); // 0x01 = Discover

        let txlabel_raw = received[0] & 0xF0;
        let response: &[u8] = &[
            txlabel_raw | 0x0 << 2 | 0x2, // txlabel (same), Single (0b00), Response Accept (0b10)
            0x01,                         // Discover
            0x3E << 2 | 0x0 << 1,         // SEID (3E), Not In Use (0b0)
            0x00 << 4 | 0x1 << 3,         // Audio (0x00), Sink (0x01)
        ];
        let mut write_fut = remote.send(response.to_vec());
        exec.run_until_stalled(&mut write_fut)
            .expect("should be ready")
            .expect("write should succeed");
    }

    for idx in 0..4 {
        assert!(exec.run_until_stalled(&mut response_futures[idx]).is_ready());
    }

    // We should be able to make new requests now.
    let mut another_fut = Box::pin(peer.discover());
    assert!(exec.run_until_stalled(&mut another_fut).is_pending());
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn command_timeout(transport_mode: Transport) {
    let mut exec = fasync::TestExecutor::new();
    let (peer, mut remote) = setup_peer(transport_mode);
    let mut response_fut = Box::pin(peer.discover());
    assert!(exec.run_until_stalled(&mut response_fut).is_pending());

    let received = recv_remote(&mut exec, &mut remote).unwrap();
    // Last half of header must be Single (0b00) and Command (0b00)
    assert_eq!(0x00, received[0] & 0xF);
    assert_eq!(0x01, received[1]); // 0x01 = Discover

    // We should be able to timeout the discover.
    let complete = exec.run_until_stalled(&mut response_fut);
    assert!(complete.is_pending());

    let _ = exec.wake_next_timer();
    assert_matches!(exec.run_until_stalled(&mut response_fut), Poll::Ready(Err(Error::Timeout)));

    // We should be able to make a new request now.
    let mut another_fut = Box::pin(peer.discover());
    assert!(exec.run_until_stalled(&mut another_fut).is_pending());

    // Responding to the first command after it timed out doesn't complete the new command
    let txlabel_raw = received[0] & 0xF0;
    let response: &[u8] = &[
        txlabel_raw | 0x0 << 2 | 0x2, // txlabel (same), Single (0b00), Response Accept (0b10)
        0x01,                         // Discover
        0x3E << 2 | 0x0 << 1,         // SEID (3E), Not In Use (0b0)
        0x00 << 4 | 0x1 << 3,         // Audio (0x00), Sink (0x01)
    ];

    let mut write_fut = remote.send(response.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    assert!(exec.run_until_stalled(&mut another_fut).is_pending());
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn command_response_can_be_discarded(transport_mode: Transport) {
    let mut exec = fasync::TestExecutor::new();
    let (peer, mut remote) = setup_peer(transport_mode);
    let response_fut = peer.discover();

    // Command should be sent before we poll a response.
    let received = recv_remote(&mut exec, &mut remote).unwrap();
    // Last half of header must be Single (0b00) and Command (0b00)
    assert_eq!(0x00, received[0] & 0xF);
    assert_eq!(0x01, received[1]); // 0x01 = Discover

    // Responding to the first command after it timed out doesn't complete the new command
    let txlabel_raw = received[0] & 0xF0;
    let response_first: &[u8] = &[
        txlabel_raw | 0x0 << 2 | 0x2, // txlabel (same), Single (0b00), Response Accept (0b10)
        0x01,                         // Discover
        0x3E << 2 | 0x0 << 1,         // SEID (3E), Not In Use (0b0)
        0x00 << 4 | 0x1 << 3,         // Audio (0x00), Sink (0x01)
    ];

    // Should be able to drop the future without ever polling it.
    drop(response_fut);

    // Make another discovery, and it should be sent.
    let mut another_fut = Box::pin(peer.discover());
    let received = recv_remote(&mut exec, &mut remote).unwrap();

    // Writing the response to the first discovery will get ignored.
    let mut write_fut = remote.send(response_first.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    assert!(exec.run_until_stalled(&mut another_fut).is_pending());

    // Responding to the first command after it timed out doesn't complete the new command
    let txlabel_raw = received[0] & 0xF0;
    let response_second: &[u8] = &[
        txlabel_raw | 0x0 << 2 | 0x2, // txlabel (same), Single (0b00), Response Accept (0b10)
        0x01,                         // Discover
        0x3E << 2 | 0x0 << 1,         // SEID (3E), Not In Use (0b0)
        0x00 << 4 | 0x1 << 3,         // Audio (0x00), Sink (0x01)
    ];

    let mut write_fut = remote.send(response_second.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    // Finish out the discovery we haven't dropped.
    assert!(exec.run_until_stalled(&mut another_fut).is_ready());
}

macro_rules! incoming_cmd_length_fail_test {
    ($test_name:ident, $signal_value:expr, $length:expr) => {
        #[test]
        fn $test_name() {
            let (mut exec, mut stream, _, mut remote) = setup_stream_test(Transport::Socket);

            // TxLabel 4, signal value, no params
            let mut incoming_cmd = vec![0x40, $signal_value];

            // Add $length bytes
            incoming_cmd.extend_from_slice(&[0; $length]);

            // Send the command
            let mut write_fut = remote.send(incoming_cmd.clone());
            exec.run_until_stalled(&mut write_fut)
                .expect("should be ready")
                .expect("write should succeed");

            let mut fut = stream.next();
            let complete = exec.run_until_stalled(&mut fut);

            // We shouldn't have received any request.
            assert!(complete.is_pending());

            // The peer should have responded with a Response Reject message with the
            // same TxLabel with BAD_LENGTH
            expect_remote_recv(&mut exec, &[0x43, $signal_value, 0x11], &mut remote);
        }
    };
}

// Discover tests
const CMD_DISCOVER_VALUE: &'static u8 = &0x01;

incoming_cmd_length_fail_test!(discover_invalid_length_one, *CMD_DISCOVER_VALUE, 1);
incoming_cmd_length_fail_test!(discover_invalid_length_two, *CMD_DISCOVER_VALUE, 2);

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn discover_event_responder_send_works(transport_mode: Transport) {
    let (mut exec, mut stream, _, mut remote) = setup_stream_test(transport_mode);

    let mut write_fut = remote.send(CMD_DISCOVER.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let respond_res = match next_request(&mut stream, &mut exec) {
        Request::Discover { responder } => {
            let s = StreamInformation::new(
                StreamEndpointId(0x0A),
                false,
                MediaType::Video,
                EndpointType::Source,
            );
            responder.send(&[s])
        }
        _ => panic!("should have received a Discover"),
    };

    assert!(respond_res.is_ok());

    expect_remote_recv(&mut exec, &[0x42, 0x01, 0x0A << 2, 0x01 << 4], &mut remote);
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn discover_event_responder_reject_works(transport_mode: Transport) {
    let (mut exec, mut stream, _, mut remote) = setup_stream_test(transport_mode);

    let mut write_fut = remote.send(CMD_DISCOVER.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let respond_res = match next_request(&mut stream, &mut exec) {
        Request::Discover { responder } => responder.reject(ErrorCode::BadState),
        _ => panic!("should have received a Discover"),
    };

    assert!(respond_res.is_ok());

    let discover_rsp = &[
        0x43, // TxLabel (4),  RemoteRejected (3)
        0x01, // Discover
        0x31, // BAD_STATE
    ];
    expect_remote_recv(&mut exec, discover_rsp, &mut remote);
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn discover_command_works(transport_mode: Transport) {
    let mut exec = fasync::TestExecutor::new();
    let (peer, mut remote) = setup_peer(transport_mode);
    let mut response_fut = Box::pin(peer.discover());
    assert!(exec.run_until_stalled(&mut response_fut).is_pending());

    let received = recv_remote(&mut exec, &mut remote).unwrap();
    // Last half of header must be Single (0b00) and Command (0b00)
    assert_eq!(0x00, received[0] & 0xF);
    assert_eq!(0x01, received[1]); // 0x01 = Discover

    let txlabel_raw = received[0] & 0xF0;

    let response: &[u8] = &[
        txlabel_raw << 4 | 0x0 << 2 | 0x2, // txlabel (same), Single (0b00), Response Accept (0b10)
        0x01,                              // Discover
        0x3E << 2 | 0x0 << 1,              // SEID (3E), Not In Use (0b0)
        0x00 << 4 | 0x1 << 3,              // Audio (0x00), Sink (0x01)
        0x01 << 2 | 0x1 << 1,              // SEID (1), In Use (0b1)
        0x01 << 4 | 0x0 << 3,              // Video (0x01), Source (0x01)
        0x17 << 2 | 0x0 << 1,              // SEID (17), Not In Use (0b0)
        0x02 << 4 | 0x1 << 3,              // Multimedia (0x02), Sink (0x01)
    ];
    let mut write_fut = remote.send(response.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let complete = exec.run_until_stalled(&mut response_fut);

    let endpoints = match complete {
        Poll::Ready(Ok(response)) => response,
        x => panic!("Should have a ready Ok response: {:?}", x),
    };

    assert_eq!(3, endpoints.len());
    let e1 =
        StreamInformation::new(StreamEndpointId(0x3E), false, MediaType::Audio, EndpointType::Sink);
    assert_eq!(e1, endpoints[0]);
    let e2 = StreamInformation::new(
        StreamEndpointId(0x01),
        true,
        MediaType::Video,
        EndpointType::Source,
    );
    assert_eq!(e2, endpoints[1]);
    let e3 = StreamInformation::new(
        StreamEndpointId(0x17),
        false,
        MediaType::Multimedia,
        EndpointType::Sink,
    );
    assert_eq!(e3, endpoints[2]);
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn discover_command_rejected(transport_mode: Transport) {
    let mut exec = fasync::TestExecutor::new();
    let (peer, mut remote) = setup_peer(transport_mode);
    let mut response_fut = Box::pin(peer.discover());
    assert!(exec.run_until_stalled(&mut response_fut).is_pending());

    let received = recv_remote(&mut exec, &mut remote).unwrap();
    // Last half of header must be Single (0b00) and Command (0b00)
    assert_eq!(0x00, received[0] & 0xF);
    assert_eq!(0x01, received[1]); // 0x01 = Discover

    let txlabel_raw = received[0] & 0xF0;

    let response: &[u8] = &[
        txlabel_raw | 0x0 << 2 | 0x3, // txlabel (same), Single (0b00), Response Reject (0b11)
        0x01,                         // Discover
        0x31,                         // BAD_STATE
    ];

    let mut write_fut = remote.send(response.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let complete = exec.run_until_stalled(&mut response_fut);

    let error = match complete {
        Poll::Ready(Err(response)) => response,
        x => panic!("Should have a ready Error response: {:?}", x),
    };

    assert_matches!(error, Error::RemoteRejected(e) if e == RemoteReject::rejected(SignalIdentifier::Discover, 0x31));
}

// GetCapabilities tests
const CMD_GET_CAPABILITIES_VALUE: &'static u8 = &0x02;
const CMD_GET_CAPABILITIES: &'static [u8] = &[
    0x40, // TxLabel (4), Single Packet (0b00), Command (0b00)
    0x02, // RFA (0b00), Get Capabilities (0x02)
    0x30, // SEID 12 (0b001100)
];

incoming_cmd_length_fail_test!(get_capabilities_too_short, *CMD_GET_CAPABILITIES_VALUE, 0);
incoming_cmd_length_fail_test!(get_capabilities_too_long, *CMD_GET_CAPABILITIES_VALUE, 2);

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn get_capabilities_event_responder_send_works(transport_mode: Transport) {
    let (mut exec, mut stream, _, mut remote) = setup_stream_test(transport_mode);

    let mut write_fut = remote.send(CMD_GET_CAPABILITIES.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let respond_res = match next_request(&mut stream, &mut exec) {
        Request::GetCapabilities { responder, stream_id } => {
            assert_eq!(StreamEndpointId::try_from(12).unwrap(), stream_id);
            let capabilities = &[
                ServiceCapability::MediaTransport,
                ServiceCapability::Reporting,
                ServiceCapability::Recovery {
                    recovery_type: 0x01,
                    max_recovery_window_size: 10,
                    max_number_media_packets: 255,
                },
                ServiceCapability::MediaCodec {
                    media_type: MediaType::Audio,
                    codec_type: MediaCodecType::new(0x40),
                    codec_extra: vec![0xDE, 0xAD, 0xBE, 0xEF],
                },
                ServiceCapability::ContentProtection {
                    protection_type: ContentProtectionType::DigitalTransmissionContentProtection,
                    extra: vec![0xF0, 0x0D],
                },
                ServiceCapability::DelayReporting,
            ];
            responder.send(capabilities)
        }
        _ => panic!("should have received a GetCapabilities"),
    };

    assert!(respond_res.is_ok());

    // Should receive the encoded version of the MediaTransport
    #[rustfmt::skip]
    let get_capabilities_rsp = &[
        0x42, // TxLabel (4) + ResponseAccept (0x02)
        0x02, // GetCapabilities Signal
        // MediaTransport (Length of Service Capability = 0)
        0x01, 0x00,
        // Reporting (LOSC = 0)
        0x02, 0x00,
        // Recovery (LOSC = 3), Type (0x01), Window size (10), Number Media Packets (255)
        0x03, 0x03, 0x01, 0x0A, 0xFF,
        // Media Codec (LOSC = 2 + 4), Media Type Audio (0x00), Codec type (0x40), Codec specific
        // as above
        0x07, 0x06, 0x00, 0x40, 0xDE, 0xAD, 0xBE, 0xEF,
        // Content Protection (LOSC = 2 + 2), DTCP (0x01, 0x00), CP Specific as above
        0x04, 0x04, 0x01, 0x00, 0xF0, 0x0D,
        /* NOTE: DelayReporting should NOT be included here
         * (GetCapabilities response cannot return DelayReporting) */
    ];
    expect_remote_recv(&mut exec, get_capabilities_rsp, &mut remote);
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn get_capabilities_responder_reject_works(transport_mode: Transport) {
    let (mut exec, mut stream, _, mut remote) = setup_stream_test(transport_mode);

    let mut write_fut = remote.send(CMD_GET_CAPABILITIES.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let respond_res = match next_request(&mut stream, &mut exec) {
        Request::GetCapabilities { responder, stream_id } => {
            assert_eq!(StreamEndpointId::try_from(12).unwrap(), stream_id);
            responder.reject(ErrorCode::BadAcpSeid)
        }
        _ => panic!("should have received a GetAllCapabilities"),
    };

    assert!(respond_res.is_ok());

    let get_capabilities_rsp = &[
        0x43, // TxLabel (4),  RemoteRejected (3)
        0x02, // Get Capabilities
        0x12, // BAD_ACP_SEID
    ];
    expect_remote_recv(&mut exec, get_capabilities_rsp, &mut remote);
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn get_capabilities_command_works(transport_mode: Transport) {
    let mut exec = fasync::TestExecutor::new();
    let (peer, mut remote) = setup_peer(transport_mode);
    let seid = &StreamEndpointId::try_from(1).unwrap();
    let mut response_fut = Box::pin(peer.get_capabilities(&seid));
    assert!(exec.run_until_stalled(&mut response_fut).is_pending());

    let received = recv_remote(&mut exec, &mut remote).unwrap();
    // Last half of header must be Single (0b00) and Command (0b00)
    assert_eq!(0x00, received[0] & 0xF);
    assert_eq!(0x02, received[1]); // 0x02 = GetCapabilities
    assert_eq!(0x01 << 2, received[2]); // SEID (0x01) , RFA

    let txlabel_raw = received[0] & 0xF0;

    #[rustfmt::skip]
    let response: &[u8] = &[
        txlabel_raw << 4 | 0x0 << 2 | 0x2, // txlabel (same), Single (0b00), Response Accept (0b10)
        0x02,                              // Get Capabilities
        // MediaTransport (Length of Service Capability = 0)
        0x01, 0x00,
        // Recovery (LOSC = 3), Type (0x01), Window size (2), Number Media Packets (5)
        0x03, 0x03, 0x01, 0x02, 0x05,
        // Media Codec (LOSC = 2 + 2), Video (0x1), Codec type (0x20), Codec specific (0xB0DE)
        0x07, 0x04, 0x10, 0x20, 0xB0, 0xDE,
        // Content Protection (LOSC = 2 + 1), SCMS (0x02, 0x00), CP Specific (0x7A)
        0x04, 0x03, 0x02, 0x00, 0x7A,
    ];
    let mut write_fut = remote.send(response.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let complete = exec.run_until_stalled(&mut response_fut);

    let capabilities = match complete {
        Poll::Ready(Ok(response)) => response,
        x => panic!("Should have a ready Ok response: {:?}", x),
    };

    assert_eq!(4, capabilities.len());
    let c1 = ServiceCapability::MediaTransport;
    assert_eq!(c1, capabilities[0]);
    let c2 = ServiceCapability::Recovery {
        recovery_type: 1,
        max_recovery_window_size: 2,
        max_number_media_packets: 5,
    };
    assert_eq!(c2, capabilities[1]);
    let c3 = ServiceCapability::MediaCodec {
        media_type: MediaType::Video,
        codec_type: MediaCodecType::new(0x20),
        codec_extra: vec![0xb0, 0xde],
    };
    assert_eq!(c3, capabilities[2]);
    let c4 = ServiceCapability::ContentProtection {
        protection_type: ContentProtectionType::SerialCopyManagementSystem,
        extra: vec![0x7a],
    };
    assert_eq!(c4, capabilities[3]);
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn get_capabilities_reject_command(transport_mode: Transport) {
    let mut exec = fasync::TestExecutor::new();
    let (peer, mut remote) = setup_peer(transport_mode);
    let seid = &StreamEndpointId::try_from(1).unwrap();
    let mut response_fut = Box::pin(peer.get_capabilities(&seid));
    assert!(exec.run_until_stalled(&mut response_fut).is_pending());

    let received = recv_remote(&mut exec, &mut remote).unwrap();
    // Last half of header must be Single (0b00) and Command (0b00)
    assert_eq!(0x00, received[0] & 0xF);
    assert_eq!(0x02, received[1]); // 0x02 = GetCapabilities
    assert_eq!(0x01 << 2, received[2]); // SEID (0x01), RFA

    let txlabel_raw = received[0] & 0xF0;

    let response: &[u8] = &[
        txlabel_raw | 0x0 << 2 | 0x3, // txlabel (same), Single (0b00), Response Reject (0b11)
        0x02,                         // Get Capabilities
        0x12,                         // BAD_ACP_SEID
    ];

    let mut write_fut = remote.send(response.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let complete = exec.run_until_stalled(&mut response_fut);

    let error = match complete {
        Poll::Ready(Err(response)) => response,
        x => panic!("Should have a ready Error response: {:?}", x),
    };

    assert_matches!(error, Error::RemoteRejected(e) if e == RemoteReject::rejected(SignalIdentifier::GetCapabilities, 0x12));
}

//  Get All Capabilities
const CMD_GET_ALL_CAPABILITIES_VALUE: &'static u8 = &0x0C;
const CMD_GET_ALL_CAPABILITIES: &'static [u8] = &[
    0x40, // TxLabel (4), Single Packet (0b00), Command (0b00)
    0x0C, // RFA (0b00), Get All Capabilities (0x0C)
    0x30, // SEID 12 (0b001100)
];

incoming_cmd_length_fail_test!(get_all_capabilities_too_short, *CMD_GET_ALL_CAPABILITIES_VALUE, 0);
incoming_cmd_length_fail_test!(get_all_capabilities_too_long, *CMD_GET_ALL_CAPABILITIES_VALUE, 2);

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn get_all_capabilities_event_responder_send_works(transport_mode: Transport) {
    let (mut exec, mut stream, _, mut remote) = setup_stream_test(transport_mode);

    let mut write_fut = remote.send(CMD_GET_ALL_CAPABILITIES.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let respond_res = match next_request(&mut stream, &mut exec) {
        Request::GetAllCapabilities { responder, stream_id } => {
            assert_eq!(StreamEndpointId::try_from(12).unwrap(), stream_id);
            let capabilities = &[
                ServiceCapability::MediaTransport,
                ServiceCapability::Reporting,
                ServiceCapability::Recovery {
                    recovery_type: 0x01,
                    max_recovery_window_size: 10,
                    max_number_media_packets: 255,
                },
                ServiceCapability::MediaCodec {
                    media_type: MediaType::Audio,
                    codec_type: MediaCodecType::new(0x40),
                    codec_extra: vec![0xDE, 0xAD, 0xBE, 0xEF],
                },
                ServiceCapability::ContentProtection {
                    protection_type: ContentProtectionType::DigitalTransmissionContentProtection,
                    extra: vec![0xF0, 0x0D],
                },
                ServiceCapability::DelayReporting,
            ];
            responder.send(capabilities)
        }
        _ => panic!("should have received a GetAllCapabilities"),
    };

    assert!(respond_res.is_ok());

    // Should receive the encoded version of the MediaTransport
    #[rustfmt::skip]
    let get_capabilities_rsp = &[
        0x42, // TxLabel (4) + ResponseAccept (0x02)
        0x0C, // GetAllCapabilities Signal
        // MediaTransport (Length of Service Capability = 0)
        0x01, 0x00,
        // Reporting (LOSC = 0)
        0x02, 0x00,
        // Recovery (LOSC = 3), Type (0x01), Window size (10), Number Media Packets (255)
        0x03, 0x03, 0x01, 0x0A, 0xFF,
        // Media Codec (LOSC = 2 + 4), Media Type Audio (0x00), Codec type (0x40), Codec specific
        // as above
        0x07, 0x06, 0x00, 0x40, 0xDE, 0xAD, 0xBE, 0xEF,
        // Content Protection (LOSC = 2 + 2), DTCP (0x01, 0x00), CP Specific as above
        0x04, 0x04, 0x01, 0x00, 0xF0, 0x0D,
        // DelayReporting (LOSC = 0)
        0x08, 0x00,
    ];
    expect_remote_recv(&mut exec, get_capabilities_rsp, &mut remote);
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn get_all_capabilities_responder_reject_works(transport_mode: Transport) {
    let (mut exec, mut stream, _, mut remote) = setup_stream_test(transport_mode);

    let mut write_fut = remote.send(CMD_GET_ALL_CAPABILITIES.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let respond_res = match next_request(&mut stream, &mut exec) {
        Request::GetAllCapabilities { responder, stream_id } => {
            assert_eq!(StreamEndpointId::try_from(12).unwrap(), stream_id);
            responder.reject(ErrorCode::BadAcpSeid)
        }
        _ => panic!("should have received a GetAllCapabilities"),
    };

    assert!(respond_res.is_ok());

    let get_all_capabilities_rsp = &[
        0x43, // TxLabel (4),  RemoteRejected (3)
        0x0C, // Get All Capabilities
        0x12, // BAD_ACP_SEID
    ];
    expect_remote_recv(&mut exec, get_all_capabilities_rsp, &mut remote);
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn get_all_capabilities_command_works(transport_mode: Transport) {
    let mut exec = fasync::TestExecutor::new();
    let (peer, mut remote) = setup_peer(transport_mode);
    let seid = StreamEndpointId::try_from(1).unwrap();
    let mut response_fut = Box::pin(peer.get_all_capabilities(&seid));
    assert!(exec.run_until_stalled(&mut response_fut).is_pending());

    let received = recv_remote(&mut exec, &mut remote).unwrap();
    // Last half of header must be Single (0b00) and Command (0b00)
    assert_eq!(0x00, received[0] & 0xF);
    assert_eq!(0x0C, received[1]); // 0x0C = GetAllCapabilities
    assert_eq!(0x01 << 2, received[2]); // SEID (0x01) , RFA

    let txlabel_raw = received[0] & 0xF0;

    #[rustfmt::skip]
    let response: &[u8] = &[
        txlabel_raw << 4 | 0x0 << 2 | 0x2, // txlabel (same), Single (0b00), Response Accept (0b10)
        0x0C,                              // Get All Capabilities
        // MediaTransport (Length of Service Capability = 0)
        0x01, 0x00,
        // Reporting (LOSC = 0)
        0x02, 0x00,
        // Media Codec (LOSC = 2 + 2), Video (0x1), Codec type (0x20), Codec specific (0xB0DE)
        0x07, 0x04, 0x10, 0x20, 0xB0, 0xDE,
        // Content Protection (LOSC = 2 + 1), SCMS (0x02, 0x00), CP Specific (0x7A)
        0x04, 0x03, 0x02, 0x00, 0x7A,
        // Delay Reporting
        0x08, 0x00,
    ];
    let mut write_fut = remote.send(response.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let complete = exec.run_until_stalled(&mut response_fut);

    let capabilities = match complete {
        Poll::Ready(Ok(response)) => response,
        x => panic!("Should have a ready Ok response: {:?}", x),
    };

    assert_eq!(5, capabilities.len());
    let c1 = ServiceCapability::MediaTransport;
    assert_eq!(c1, capabilities[0]);
    let c2 = ServiceCapability::Reporting;
    assert_eq!(c2, capabilities[1]);
    let c3 = ServiceCapability::MediaCodec {
        media_type: MediaType::Video,
        codec_type: MediaCodecType::new(0x20),
        codec_extra: vec![0xb0, 0xde],
    };
    assert_eq!(c3, capabilities[2]);
    let c4 = ServiceCapability::ContentProtection {
        protection_type: ContentProtectionType::SerialCopyManagementSystem,
        extra: vec![0x7a],
    };
    assert_eq!(c4, capabilities[3]);
    let c5 = ServiceCapability::DelayReporting;
    assert_eq!(c5, capabilities[4]);
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn get_all_capabilities_reject_command(transport_mode: Transport) {
    let mut exec = fasync::TestExecutor::new();
    let (peer, mut remote) = setup_peer(transport_mode);
    let seid = StreamEndpointId::try_from(1).unwrap();
    let mut response_fut = Box::pin(peer.get_all_capabilities(&seid));
    assert!(exec.run_until_stalled(&mut response_fut).is_pending());

    let received = recv_remote(&mut exec, &mut remote).unwrap();
    // Last half of header must be Single (0b00) and Command (0b00)
    assert_eq!(0x00, received[0] & 0xF);
    assert_eq!(0x0C, received[1]); // 0x02 = GetAllCapabilities
    assert_eq!(0x01 << 2, received[2]); // SEID (0x01), RFA

    let txlabel_raw = received[0] & 0xF0;

    let response: &[u8] = &[
        txlabel_raw | 0x0 << 2 | 0x3, // txlabel (same), Single (0b00), Response Reject (0b11)
        0x0C,                         // Get All Capabilities
        0x12,                         // BAD_ACP_SEID
    ];

    let mut write_fut = remote.send(response.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let complete = exec.run_until_stalled(&mut response_fut);

    let error = match complete {
        Poll::Ready(Err(response)) => response,
        x => panic!("Should have a ready Error response: {:?}", x),
    };

    assert_matches!(
        error, Error::RemoteRejected(e) if e ==
        RemoteReject::rejected(SignalIdentifier::GetAllCapabilities, 0x12)
    );
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn get_all_capabilites_general_reject(transport_mode: Transport) {
    let mut exec = fasync::TestExecutor::new();
    let (peer, mut remote) = setup_peer(transport_mode);
    let seid = StreamEndpointId::try_from(1).unwrap();
    let mut response_fut = Box::pin(peer.get_all_capabilities(&seid));
    assert!(exec.run_until_stalled(&mut response_fut).is_pending());

    let received = recv_remote(&mut exec, &mut remote).unwrap();
    // Last half of header must be Single (0b00) and Command (0b00)
    assert_eq!(0x00, received[0] & 0xF);
    assert_eq!(0x0C, received[1]); // 0x0C = GetAllCapabilities
    assert_eq!(0x01 << 2, received[2]); // SEID (0x01), RFA

    let txlabel_raw = received[0] & 0xF0;

    let response: &[u8] = &[
        txlabel_raw | 0x0 << 2 | 0x1, // txlabel (same), Single (0b00), General Reject (0b01)
        0x0C,                         // Get All Capabilities
    ];

    let mut write_fut = remote.send(response.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let complete = exec.run_until_stalled(&mut response_fut);

    let error = match complete {
        Poll::Ready(Err(response)) => response,
        x => panic!("Should have a ready Error response: {:?}", x),
    };

    assert_matches!(
        error, Error::RemoteRejected(e) if e == RemoteReject::general(SignalIdentifier::GetAllCapabilities)
    );
}

// Get Configuration
const CMD_GET_CONFIGURATION_VALUE: &'static u8 = &0x04;
const CMD_GET_CONFIGURATION: &'static [u8] = &[
    0x40, // TxLabel (4), Single Packet (0b00), Command (0b00)
    0x04, // RFA (0b00), Get All Capabilities (0x0C)
    0x30, // SEID 12 (0b001100)
];

incoming_cmd_length_fail_test!(get_configuration_too_short, *CMD_GET_CONFIGURATION_VALUE, 0);
incoming_cmd_length_fail_test!(get_configuration_too_long, *CMD_GET_CONFIGURATION_VALUE, 2);

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn get_configuration_event_responder_send_works(transport_mode: Transport) {
    let (mut exec, mut stream, _, mut remote) = setup_stream_test(transport_mode);

    let mut write_fut = remote.send(CMD_GET_CONFIGURATION.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let respond_res = match next_request(&mut stream, &mut exec) {
        Request::GetConfiguration { responder, stream_id } => {
            assert_eq!(StreamEndpointId::try_from(12).unwrap(), stream_id);
            let capabilities = &[
                ServiceCapability::MediaTransport,
                ServiceCapability::Reporting,
                ServiceCapability::Recovery {
                    recovery_type: 0x01,
                    max_recovery_window_size: 10,
                    max_number_media_packets: 255,
                },
                ServiceCapability::MediaCodec {
                    media_type: MediaType::Audio,
                    codec_type: MediaCodecType::new(0x40),
                    codec_extra: vec![0xDE, 0xAD, 0xBE, 0xEF],
                },
                ServiceCapability::ContentProtection {
                    protection_type: ContentProtectionType::DigitalTransmissionContentProtection,
                    extra: vec![0xF0, 0x0D],
                },
                ServiceCapability::DelayReporting,
            ];
            responder.send(capabilities)
        }
        _ => panic!("should have received a GetConfiguration"),
    };

    assert!(respond_res.is_ok());

    // Should receive the encoded version of the capabilities:
    #[rustfmt::skip]
    let get_configuration_rsp = &[
        0x42, // TxLabel (4) + ResponseAccept (0x02)
        0x04, // GetConfiguration Signal
        // MediaTransport (Length of Service Capability = 0)
        0x01, 0x00,
        // Reporting (LOSC = 0)
        0x02, 0x00,
        // Recovery (LOSC = 3), Type (0x01), Window size (10), Number Media Packets (255)
        0x03, 0x03, 0x01, 0x0A, 0xFF,
        // Media Codec (LOSC = 2 + 4), Media Type Audio (0x00), Codec type (0x40), Codec specific
        // as above
        0x07, 0x06, 0x00, 0x40, 0xDE, 0xAD, 0xBE, 0xEF,
        // Content Protection (LOSC = 2 + 2), DTCP (0x01, 0x00), CP Specific as above
        0x04, 0x04, 0x01, 0x00, 0xF0, 0x0D,
        // DelayReporting (LOSC = 0)
        0x08, 0x00,
    ];
    expect_remote_recv(&mut exec, get_configuration_rsp, &mut remote);
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn get_configuration_responder_reject_works(transport_mode: Transport) {
    let (mut exec, mut stream, _, mut remote) = setup_stream_test(transport_mode);

    let mut write_fut = remote.send(CMD_GET_CONFIGURATION.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let respond_res = match next_request(&mut stream, &mut exec) {
        Request::GetConfiguration { responder, stream_id } => {
            assert_eq!(StreamEndpointId::try_from(12).unwrap(), stream_id);
            responder.reject(ErrorCode::BadAcpSeid)
        }
        _ => panic!("should have received a GetConfiguration"),
    };

    assert!(respond_res.is_ok());

    let get_configuration_rsp = &[
        0x43, // TxLabel (4),  RemoteRejected (3)
        0x04, // Get Configuration
        0x12, // BAD_ACP_SEID
    ];
    expect_remote_recv(&mut exec, get_configuration_rsp, &mut remote);
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn get_configuration_command_works(transport_mode: Transport) {
    let mut exec = fasync::TestExecutor::new();
    let (peer, mut remote) = setup_peer(transport_mode);
    let seid = StreamEndpointId::try_from(1).unwrap();
    let mut response_fut = Box::pin(peer.get_configuration(&seid));
    assert!(exec.run_until_stalled(&mut response_fut).is_pending());

    let received = recv_remote(&mut exec, &mut remote).unwrap();
    // Last half of header must be Single (0b00) and Command (0b00)
    assert_eq!(0x00, received[0] & 0xF);
    assert_eq!(0x04, received[1]); // 0x04 = GetConfiguration
    assert_eq!(0x01 << 2, received[2]); // SEID (0x01) , RFA

    let txlabel_raw = received[0] & 0xF0;

    #[rustfmt::skip]
    let response: &[u8] = &[
        txlabel_raw << 4 | 0x0 << 2 | 0x2, // txlabel (same), Single (0b00), Response Accept (0b10)
        0x04,                              // Get Configuration
        // MediaTransport (Length of Service Capability = 0)
        0x01, 0x00,
        // Reporting (LOSC = 0)
        0x02, 0x00,
        // Media Codec (LOSC = 2 + 2), Video (0x1), Codec type (0x20), Codec specific (0xB0DE)
        0x07, 0x04, 0x10, 0x20, 0xB0, 0xDE,
        // Content Protection (LOSC = 2 + 1), SCMS (0x02, 0x00), CP Specific (0x7A)
        0x04, 0x03, 0x02, 0x00, 0x7A,
        // Delay Reporting
        0x08, 0x00,
    ];
    let mut write_fut = remote.send(response.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let complete = exec.run_until_stalled(&mut response_fut);

    let capabilities = match complete {
        Poll::Ready(Ok(response)) => response,
        x => panic!("Should have a ready Ok response: {:?}", x),
    };

    assert_eq!(5, capabilities.len());
    let c1 = ServiceCapability::MediaTransport;
    assert_eq!(c1, capabilities[0]);
    let c2 = ServiceCapability::Reporting;
    assert_eq!(c2, capabilities[1]);
    let c3 = ServiceCapability::MediaCodec {
        media_type: MediaType::Video,
        codec_type: MediaCodecType::new(0x20),
        codec_extra: vec![0xb0, 0xde],
    };
    assert_eq!(c3, capabilities[2]);
    let c4 = ServiceCapability::ContentProtection {
        protection_type: ContentProtectionType::SerialCopyManagementSystem,
        extra: vec![0x7a],
    };
    assert_eq!(c4, capabilities[3]);
    let c5 = ServiceCapability::DelayReporting;
    assert_eq!(c5, capabilities[4]);
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn get_configuration_reject_command(transport_mode: Transport) {
    let mut exec = fasync::TestExecutor::new();
    let (peer, mut remote) = setup_peer(transport_mode);
    let seid = StreamEndpointId::try_from(1).unwrap();
    let mut response_fut = Box::pin(peer.get_configuration(&seid));
    assert!(exec.run_until_stalled(&mut response_fut).is_pending());

    let received = recv_remote(&mut exec, &mut remote).unwrap();
    // Last half of header must be Single (0b00) and Command (0b00)
    assert_eq!(0x00, received[0] & 0xF);
    assert_eq!(0x04, received[1]); // 0x04 = GetConfiguration
    assert_eq!(0x01 << 2, received[2]); // SEID (0x01), RFA

    let txlabel_raw = received[0] & 0xF0;

    let response: &[u8] = &[
        txlabel_raw | 0x0 << 2 | 0x3, // txlabel (same), Single (0b00), Response Reject (0b11)
        0x04,                         // Get Configuration
        0x12,                         // BAD_ACP_SEID
    ];

    let mut write_fut = remote.send(response.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let complete = exec.run_until_stalled(&mut response_fut);

    let error = match complete {
        Poll::Ready(Err(response)) => response,
        x => panic!("Should have a ready Error response: {:?}", x),
    };

    assert_matches!(
        error, Error::RemoteRejected(e) if e ==
        RemoteReject::rejected(SignalIdentifier::GetConfiguration, 0x12)
    );
}

macro_rules! seid_command_test {
    ($test_name:ident, $signal_value:expr, $peer_function:ident) => {
        #[test]
        fn $test_name() {
            let mut exec = fasync::TestExecutor::new();
            let (peer, mut remote) = setup_peer(Transport::Socket);
            let seid = StreamEndpointId::try_from(1).unwrap();
            let mut response_fut = Box::pin(peer.$peer_function(&seid));
            assert!(exec.run_until_stalled(&mut response_fut).is_pending());

            let received = recv_remote(&mut exec, &mut remote).unwrap();
            // Last half of header must be Single (0b00) and Command (0b00)
            assert_eq!(0x00, received[0] & 0xF);
            assert_eq!($signal_value, received[1]); // $signal_value = $request_variant
            assert_eq!(0x01 << 2, received[2]); // SEID (0x01) , RFA

            let txlabel_raw = received[0] & 0xF0;

            let response: &[u8] = &[
                // txlabel (same), Single (0b00), Response Accept (0b10)
                txlabel_raw << 4 | 0x0 << 2 | 0x2,
                $signal_value, // $request_variant
            ];
            let mut write_fut = remote.send(response.to_vec());
            exec.run_until_stalled(&mut write_fut)
                .expect("should be ready")
                .expect("write should succeed");

            let complete = exec.run_until_stalled(&mut response_fut);

            assert_matches!(complete, Poll::Ready(Ok(())));
        }
    };
}

macro_rules! seids_command_test {
    ($test_name:ident, $signal_value:expr, $peer_function:ident) => {
        #[test]
        fn $test_name() {
            let mut exec = fasync::TestExecutor::new();
            let (peer, mut remote) = setup_peer(Transport::Socket);
            let seid1 = StreamEndpointId::try_from(1).unwrap();
            let seid2 = StreamEndpointId::try_from(16).unwrap();
            let seids = [seid1, seid2];
            let mut response_fut = Box::pin(peer.$peer_function(&seids));
            assert!(exec.run_until_stalled(&mut response_fut).is_pending());

            let received = recv_remote(&mut exec, &mut remote).unwrap();
            // Last half of header must be Single (0b00) and Command (0b00)
            assert_eq!(0x00, received[0] & 0xF);
            assert_eq!($signal_value, received[1]); // $signal_value = $request_variant
            assert_eq!(0x01 << 2, received[2]); // SEID (0x01) , RFA
            assert_eq!(0x10 << 2, received[3]); // SEID (0x10) , RFA

            let txlabel_raw = received[0] & 0xF0;

            let response: &[u8] = &[
                // txlabel (same), Single (0b00), Response Accept (0b10)
                txlabel_raw << 4 | 0x0 << 2 | 0x2,
                $signal_value, // Signal value
            ];
            let mut write_fut = remote.send(response.to_vec());
            exec.run_until_stalled(&mut write_fut)
                .expect("should be ready")
                .expect("write should succeed");

            let complete = exec.run_until_stalled(&mut response_fut);

            assert_matches!(complete, Poll::Ready(Ok(())));
        }
    };
}

macro_rules! seid_command_reject_test {
    ($test_name:ident, $signal_value: expr, $peer_function:ident) => {
        #[test]
        fn $test_name() {
            let mut exec = fasync::TestExecutor::new();
            let (peer, mut remote) = setup_peer(Transport::Socket);
            let seid = StreamEndpointId::try_from(1).unwrap();
            let mut response_fut = Box::pin(peer.$peer_function(&seid));
            assert!(exec.run_until_stalled(&mut response_fut).is_pending());

            let received = recv_remote(&mut exec, &mut remote).unwrap();
            // Last half of header must be Single (0b00) and Command (0b00)
            assert_eq!(0x00, received[0] & 0xF);
            assert_eq!($signal_value, received[1]); // 0x02 = GetAllCapabilities
            assert_eq!(0x01 << 2, received[2]); // SEID (0x01), RFA

            let txlabel_raw = received[0] & 0xF0;

            let response: &[u8] = &[
                // txlabel (same), Single (0b00), Response Reject (0b11)
                txlabel_raw | 0x0 << 2 | 0x3,
                $signal_value, // $request_variant
                0x12,          // BAD_ACP_SEID
            ];

            let mut write_fut = remote.send(response.to_vec());
            exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

            let complete = exec.run_until_stalled(&mut response_fut);

            let error = match complete {
                Poll::Ready(Err(response)) => response,
                x => panic!("Should have a ready Error response: {:?}", x),
            };

            assert_matches!(
                error, Error::RemoteRejected(e) if e ==
                RemoteReject::rejected($signal_value.try_into().unwrap(), 0x12)
            );
        }
    };
}

macro_rules! seids_command_reject_test {
    ($test_name:ident, $signal_value: expr, $peer_function:ident) => {
        #[test]
        fn $test_name() {
            let mut exec = fasync::TestExecutor::new();
            let (peer, mut remote) = setup_peer(Transport::Socket);
            let seid1 = StreamEndpointId::try_from(1).unwrap();
            let seid2 = StreamEndpointId::try_from(16).unwrap();
            let seids = [seid1, seid2];
            let mut response_fut = Box::pin(peer.$peer_function(&seids));
            assert!(exec.run_until_stalled(&mut response_fut).is_pending());

            let received = recv_remote(&mut exec, &mut remote).unwrap();
            // Last half of header must be Single (0b00) and Command (0b00)
            assert_eq!(0x00, received[0] & 0xF);
            assert_eq!($signal_value, received[1]); // 0x02 = GetAllCapabilities
            assert_eq!(0x01 << 2, received[2]); // SEID (0x01), RFA
            assert_eq!(0x10 << 2, received[3]); // SEID (0x10), RFA

            let txlabel_raw = received[0] & 0xF0;

            let response: &[u8] = &[
                // txlabel (same), Single (0b00), Response Reject (0b11)
                txlabel_raw | 0x0 << 2 | 0x3,
                $signal_value, // $request_variant
                0x10 << 2,     // SEID 16, RFA
                0x12,          // BAD_ACP_SEID
            ];

            let mut write_fut = remote.send(response.to_vec());
            exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

            let complete = exec.run_until_stalled(&mut response_fut);

            let error = match complete {
                Poll::Ready(Err(response)) => response,
                x => panic!("Should have a ready Error response: {:?}", x),
            };

            assert_matches!(
                error, Error::RemoteRejected(e) if e ==
                RemoteReject::stream($signal_value.try_into().unwrap(), 0x10, 0x12)
            );
        }
    };
}
macro_rules! seid_event_responder_send_test {
    ($test_name:ident, $variant:ident, $signal_value:expr, $command_var:ident) => {
        #[test]
        fn $test_name() {
            let (mut exec, mut stream, _, mut remote) = setup_stream_test(Transport::Socket);

            let mut write_fut = remote.send($command_var.to_vec());
            exec.run_until_stalled(&mut write_fut)
                .expect("should be ready")
                .expect("write should succeed");

            let respond_res = match next_request(&mut stream, &mut exec) {
                Request::$variant { responder, stream_id } => {
                    assert_eq!(StreamEndpointId::try_from(12).unwrap(), stream_id);
                    responder.send()
                }
                _ => panic!("received the wrong request"),
            };

            assert!(respond_res.is_ok());

            // Should receive the response.
            let ok_rsp = &[
                0x42,          // TxLabel (4) + ResponseAccept (0x02)
                $signal_value, // $variant Signal
            ];
            expect_remote_recv(&mut exec, ok_rsp, &mut remote);
        }
    };
}

macro_rules! seids_event_responder_send_test {
    ($test_name:ident, $variant:ident, $signal_value:expr, $command_var:ident) => {
        #[test]
        fn $test_name() {
            let (mut exec, mut stream, _, mut remote) = setup_stream_test(Transport::Socket);

            let mut write_fut = remote.send($command_var.to_vec());
            exec.run_until_stalled(&mut write_fut)
                .expect("should be ready")
                .expect("write should succeed");

            let respond_res = match next_request(&mut stream, &mut exec) {
                Request::$variant { responder, stream_ids } => {
                    assert_eq!(
                        &StreamEndpointId::try_from(12).unwrap(),
                        stream_ids.get(0).unwrap()
                    );
                    responder.send()
                }
                _ => panic!("received the wrong request"),
            };

            assert!(respond_res.is_ok());

            // Should receive the response.
            let ok_rsp = &[
                0x42,          // TxLabel (4) + ResponseAccept (0x02)
                $signal_value, // $variant Signal
            ];
            expect_remote_recv(&mut exec, ok_rsp, &mut remote);
        }
    };
}

macro_rules! seid_event_responder_reject_test {
    ($test_name:ident, $variant:ident, $signal_value:expr, $command_var:ident) => {
        #[test]
        fn $test_name() {
            let (mut exec, mut stream, _, mut remote) = setup_stream_test(Transport::Socket);

            let mut write_fut = remote.send($command_var.to_vec());
            exec.run_until_stalled(&mut write_fut)
                .expect("should be ready")
                .expect("write should succeed");

            let respond_res = match next_request(&mut stream, &mut exec) {
                Request::$variant { responder, stream_id } => {
                    assert_eq!(StreamEndpointId::try_from(12).unwrap(), stream_id);
                    responder.reject(ErrorCode::BadAcpSeid)
                }
                _ => panic!("received the wrong request"),
            };

            assert!(respond_res.is_ok());

            let rejected_rsp = &[
                0x43,          // TxLabel (4),  RemoteRejected (3)
                $signal_value, // $variant
                0x12,          // BAD_ACP_SEID
            ];
            expect_remote_recv(&mut exec, rejected_rsp, &mut remote);
        }
    };
}

macro_rules! stream_event_responder_reject_test {
    ($test_name:ident, $variant:ident, $signal_value:expr, $command_var:ident) => {
        #[test]
        fn $test_name() {
            let (mut exec, mut stream, _, mut remote) = setup_stream_test(Transport::Socket);

            let mut write_fut = remote.send($command_var.to_vec());
            exec.run_until_stalled(&mut write_fut)
                .expect("should be ready")
                .expect("write should succeed");

            let respond_res = match next_request(&mut stream, &mut exec) {
                Request::$variant { responder, stream_ids } => {
                    let stream_id = stream_ids.get(0).unwrap();
                    assert_eq!(&StreamEndpointId::try_from(12).unwrap(), stream_id);
                    responder.reject(stream_id, ErrorCode::BadAcpSeid)
                }
                _ => panic!("received the wrong request"),
            };

            assert!(respond_res.is_ok());

            let rejected_rsp = &[
                0x43,          // TxLabel (4),  RemoteRejected (3)
                $signal_value, // $variant
                12 << 2,       // Stream ID (12), RFA
                0x12,          // BAD_ACP_SEID
            ];
            expect_remote_recv(&mut exec, rejected_rsp, &mut remote);
        }
    };
}

// Open

const CMD_OPEN: &'static [u8] = &[
    0x40, // TxLabel (4), Single Packet (0b00), Command (0b00)
    0x06, // RFA (0b00), Open (0x06)
    0x30, // SEID 12 (0b001100)
];

const CMD_OPEN_VALUE: &'static u8 = &0x06;

incoming_cmd_length_fail_test!(open_length_too_short, *CMD_OPEN_VALUE, 0);
incoming_cmd_length_fail_test!(open_length_too_long, *CMD_OPEN_VALUE, 2);
seid_command_test!(open_command_works, *CMD_OPEN_VALUE, open);
seid_command_reject_test!(open_command_reject_works, *CMD_OPEN_VALUE, open);
seid_event_responder_send_test!(open_responder_send, Open, *CMD_OPEN_VALUE, CMD_OPEN);
seid_event_responder_reject_test!(open_responder_reject, Open, *CMD_OPEN_VALUE, CMD_OPEN);

// Start

const CMD_START: &'static [u8] = &[
    0x40, // TxLabel (4), Single Packet (0b00), Command (0b00)
    0x07, // RFA (0b00), Start (0x07)
    0x30, // SEID 12 (0b001100)
];

const CMD_START_VALUE: &'static u8 = &0x07;

incoming_cmd_length_fail_test!(start_length, *CMD_START_VALUE, 0);
seids_command_test!(start_command_works, *CMD_START_VALUE, start);
seids_command_reject_test!(start_command_reject_works, *CMD_START_VALUE, start);
seids_event_responder_send_test!(start_responder_send, Start, *CMD_START_VALUE, CMD_START);
stream_event_responder_reject_test!(start_responder_reject, Start, *CMD_START_VALUE, CMD_START);

// Close

const CMD_CLOSE: &'static [u8] = &[
    0x40, // TxLabel (4), Single Packet (0b00), Command (0b00)
    0x08, // RFA (0b00), Close (0x08)
    0x30, // SEID 12 (0b001100)
];

const CMD_CLOSE_VALUE: &'static u8 = &0x08;

incoming_cmd_length_fail_test!(close_length_too_short, *CMD_CLOSE_VALUE, 0);
incoming_cmd_length_fail_test!(close_length_too_long, *CMD_CLOSE_VALUE, 2);
seid_command_test!(close_command_works, *CMD_CLOSE_VALUE, close);
seid_command_reject_test!(close_command_reject_works, *CMD_CLOSE_VALUE, close);
seid_event_responder_send_test!(close_responder_send, Close, *CMD_CLOSE_VALUE, CMD_CLOSE);
seid_event_responder_reject_test!(close_responder_reject, Close, *CMD_CLOSE_VALUE, CMD_CLOSE);

// Suspend

const CMD_SUSPEND: &'static [u8] = &[
    0x40, // TxLabel (4), Single Packet (0b00), Command (0b00)
    0x09, // RFA (0b00), Suspend (0x09)
    0x30, // SEID 12 (0b001100)
];

const CMD_SUSPEND_VALUE: &'static u8 = &0x09;

incoming_cmd_length_fail_test!(suspend_length, *CMD_SUSPEND_VALUE, 0);
seids_command_test!(suspend_command_works, *CMD_SUSPEND_VALUE, suspend);
seids_command_reject_test!(suspend_command_reject_works, *CMD_SUSPEND_VALUE, suspend);
seids_event_responder_send_test!(suspend_responder_send, Suspend, *CMD_SUSPEND_VALUE, CMD_SUSPEND);
stream_event_responder_reject_test!(
    suspend_responder_reject,
    Suspend,
    *CMD_SUSPEND_VALUE,
    CMD_SUSPEND
);

// Abort

const CMD_ABORT: &'static [u8] = &[
    0x40, // TxLabel (4), Single Packet (0b00), Command (0b00)
    0x0A, // RFA (0b00), Suspend (0x0A)
    0x30, // SEID 12 (0b001100)
];

const CMD_ABORT_VALUE: &'static u8 = &0x0A;

incoming_cmd_length_fail_test!(abort_length_too_short, *CMD_ABORT_VALUE, 0);
incoming_cmd_length_fail_test!(abort_length_too_long, *CMD_ABORT_VALUE, 2);
seid_command_test!(abort_command_works, *CMD_ABORT_VALUE, abort);
seid_command_reject_test!(abort_command_reject_works, *CMD_ABORT_VALUE, abort);
seid_event_responder_send_test!(abort_responder_send, Abort, *CMD_ABORT_VALUE, CMD_ABORT);
seid_event_responder_reject_test!(abort_responder_reject, Abort, *CMD_ABORT_VALUE, CMD_ABORT);

/// Abort requests with an invalid SEID are allowed to be dropped.
/// We timeout after some amount of time.
#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn abort_sent_no_response(transport_mode: Transport) {
    let mut exec = fasync::TestExecutor::new();
    let (peer, mut remote) = setup_peer(transport_mode);
    let seid = StreamEndpointId::try_from(1).unwrap();
    let mut response_fut = Box::pin(peer.abort(&seid));
    assert!(exec.run_until_stalled(&mut response_fut).is_pending());

    let received = recv_remote(&mut exec, &mut remote).unwrap();
    // Last half of header must be Single (0b00) and Command (0b00)
    assert_eq!(0x00, received[0] & 0xF);
    assert_eq!(0x0A, received[1]); // $signal_value = $request_variant
    assert_eq!(0x01 << 2, received[2]); // SEID (0x01) , RFA

    let complete = exec.run_until_stalled(&mut response_fut);
    assert!(complete.is_pending());

    let _ = exec.wake_next_timer();
    assert_matches!(exec.run_until_stalled(&mut response_fut), Poll::Ready(Err(Error::Timeout)));
}

// Set Configuration

fn expect_config_recv_cap_okay(
    cmd: &[u8],
    local_seid: StreamEndpointId,
    remote_seid: StreamEndpointId,
    capability: ServiceCapability,
) {
    let (mut exec, mut stream, _, mut remote) = setup_stream_test(Transport::Socket);

    let txlabel_mask = cmd[0] & 0xF0;

    let mut write_fut = remote.send(cmd.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let respond_res = match next_request(&mut stream, &mut exec) {
        Request::SetConfiguration {
            responder,
            local_stream_id,
            remote_stream_id,
            capabilities,
        } => {
            assert_eq!(local_seid, local_stream_id);
            assert_eq!(remote_seid, remote_stream_id);
            assert_eq!(capability, capabilities[0]);
            responder.send()
        }
        _ => panic!("should have received a SetConfiguration"),
    };

    assert!(respond_res.is_ok());

    // Expected response: Same TxLabel & ResponseAccept (0x2) , Signal.
    expect_remote_recv(&mut exec, &[txlabel_mask | 0x02, 0x03], &mut remote);
}

// Set config must be at least 2 SEIDs and one configuration long.
incoming_cmd_length_fail_test!(set_config_length_too_short, 0x03, 2);

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn set_configuration_invalid_media_transport_format(transport_mode: Transport) {
    let (mut exec, mut stream, _, mut remote) = setup_stream_test(transport_mode);

    // TxLabel (4), ResponseReject, Signal, Relevant Cap (Media Transport),
    // BadMediaTransportFormat
    let rsp = &[0x43, 0x03, 0x01, 0x23];

    let cmd = &[
        0x40, 0x03, 0x04, 0x08, // TxLabel 4, Signal, ACP (1) and INT (2) SEID
        0x01, 0x01, 0x01, // Media Transport, Length 1 (too long), dummy
    ];
    stream_request_response(&mut exec, &mut stream, &mut remote, cmd, rsp);
}

// These also test that the MediaTransport is decoded and received correctly.
#[test]
fn set_config_event_responder_send_works() {
    let set_config_cmd = &[
        0x40, 0x03, 0x04, 0x08, // TxLabel 4, Signal, ACP (1) and INT (2) SEID
        0x01, 0x00, // Media Transport, Length 0
    ];

    expect_config_recv_cap_okay(
        set_config_cmd,
        StreamEndpointId(1),
        StreamEndpointId(2),
        ServiceCapability::MediaTransport,
    );
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn set_config_event_responder_reject_works(transport_mode: Transport) {
    let (mut exec, mut stream, _, mut remote) = setup_stream_test(transport_mode);

    let set_config_cmd = &[
        0x40, 0x03, 0x04, 0x08, // TxLabel 4, Signal, ACP (1) and INT (2) SEID
        0x01, 0x00, // Media Transport, Length 0
    ];

    let mut write_fut = remote.send(set_config_cmd.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let respond_res = match next_request(&mut stream, &mut exec) {
        Request::SetConfiguration {
            responder,
            local_stream_id,
            remote_stream_id,
            capabilities,
        } => {
            assert_eq!(StreamEndpointId(1), local_stream_id);
            assert_eq!(StreamEndpointId(2), remote_stream_id);
            assert_eq!(ServiceCapability::MediaTransport, capabilities[0]);
            responder.reject(capabilities[0].category(), ErrorCode::UnsupportedConfiguration)
        }
        _ => panic!("should have received a SetConfiguration"),
    };

    assert!(respond_res.is_ok());

    // Expected response: Same TxLabel (4) & ResponseReject, Signal,
    // Relevant capability, Error Code (Unsupported Configure)
    expect_remote_recv(&mut exec, &[0x43, 0x03, 0x01, 0x29], &mut remote);
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn set_config_command_works(transport_mode: Transport) {
    let mut exec = fasync::TestExecutor::new();
    let (peer, mut remote) = setup_peer(transport_mode);
    // This should cover all the implemented capabilities to confirm they are
    // encoded correctly.
    let caps = vec![
        ServiceCapability::MediaTransport,
        ServiceCapability::Reporting,
        ServiceCapability::Recovery {
            recovery_type: 0x01,
            max_recovery_window_size: 10,
            max_number_media_packets: 255,
        },
        ServiceCapability::MediaCodec {
            media_type: MediaType::Audio,
            codec_type: MediaCodecType::new(0x40),
            codec_extra: vec![0xDE, 0xAD, 0xBE, 0xEF],
        },
        ServiceCapability::ContentProtection {
            protection_type: ContentProtectionType::DigitalTransmissionContentProtection,
            extra: vec![0xF0, 0x0D],
        },
        ServiceCapability::DelayReporting,
    ];
    let mut response_fut =
        Box::pin(peer.set_configuration(&StreamEndpointId(1), &StreamEndpointId(2), &caps));
    assert!(exec.run_until_stalled(&mut response_fut).is_pending());

    let received = recv_remote(&mut exec, &mut remote).unwrap();
    #[rustfmt::skip]
    let set_capabilities_req = &[
        0x03,      // Set Configuration Signal
        0x01 << 2, // ACP SEID, RFA
        0x02 << 2, // INT SEID, RFA
        // MediaTransport (Length of Service Capability = 0)
        0x01, 0x00,
        // Reporting (LOSC = 0)
        0x02, 0x00,
        // Recovery (LOSC = 3), Type (0x01), Window size (10), Number Media Packets (255)
        0x03, 0x03, 0x01, 0x0A, 0xFF,
        // Media Codec (LOSC = 2 + 4), Media Type Audio (0x00), Codec type (0x40), Codec specific
        // as above
        0x07, 0x06, 0x00, 0x40, 0xDE, 0xAD, 0xBE, 0xEF,
        // Content Protection (LOSC = 2 + 2), DTCP (0x01, 0x00), CP Specific as above
        0x04, 0x04, 0x01, 0x00, 0xF0, 0x0D,
        // DelayReporting (LOSC = 0)
        0x08, 0x00,
    ];
    // Last half of header must be Single (0b00) and Command (0b00)
    assert_eq!(0x00, received[0] & 0xF);
    assert_eq!(set_capabilities_req, &received[1..]);

    let txlabel_raw = received[0] & 0xF0;

    let complete = exec.run_until_stalled(&mut response_fut);
    assert!(complete.is_pending());

    // Positive response
    let response: &[u8] = &[
        txlabel_raw << 4 | 0x0 << 2 | 0x2, // txlabel (same), Single (0b00), Response Accept (0b10)
        0x03,                              // Set Configuration
    ];
    let mut write_fut = remote.send(response.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let complete = exec.run_until_stalled(&mut response_fut);
    assert_matches!(complete, Poll::Ready(Ok(())));
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn set_config_error_response(transport_mode: Transport) {
    let mut exec = fasync::TestExecutor::new();
    let (peer, mut remote) = setup_peer(transport_mode);
    let caps = vec![
        ServiceCapability::MediaTransport,
        ServiceCapability::MediaCodec {
            media_type: MediaType::Audio,
            codec_type: MediaCodecType::new(0x40),
            codec_extra: vec![0xDE, 0xAD, 0xBE, 0xEF],
        },
    ];
    let mut response_fut =
        Box::pin(peer.set_configuration(&StreamEndpointId(1), &StreamEndpointId(2), &caps));
    assert!(exec.run_until_stalled(&mut response_fut).is_pending());

    let received = recv_remote(&mut exec, &mut remote).unwrap();
    #[rustfmt::skip]
    let set_capabilities_req = &[
        0x03,      // Set Configuration Signal
        0x01 << 2, // ACP SEID, RFA
        0x02 << 2, // INT SEID, RFA
        // MediaTransport (Length of Service Capability = 0)
        0x01, 0x00,
        // Media Codec (LOSC = 2 + 4), Media Type Audio (0x00), Codec type (0x40), Codec specific
        // as above
        0x07, 0x06, 0x00, 0x40, 0xDE, 0xAD, 0xBE, 0xEF,
    ];
    // Last half of header must be Single (0b00) and Command (0b00)
    assert_eq!(0x00, received[0] & 0xF);
    assert_eq!(set_capabilities_req, &received[1..]);

    let txlabel_raw = received[0] & 0xF0;

    let complete = exec.run_until_stalled(&mut response_fut);
    assert!(complete.is_pending());

    // Error response
    let response: &[u8] = &[
        txlabel_raw << 4 | 0x0 << 2 | 0x3, // txlabel (same), Single (0b00), Response Reject
        0x03,                              // Set Configuration
        0x07,                              // Relevant Capability (Media Codec)
        0x29,                              // Unsupported configuration
    ];
    let mut write_fut = remote.send(response.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let complete = exec.run_until_stalled(&mut response_fut);
    assert_matches!(
        complete, Poll::Ready(Err(Error::RemoteRejected(e))) if e ==
        RemoteReject::config(
            SignalIdentifier::SetConfiguration,
            0x07,
            0x29
        )
    );
}

// Set Config: Reporting

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn set_config_bad_reporting_format(transport_mode: Transport) {
    let (mut exec, mut stream, _, mut remote) = setup_stream_test(transport_mode);

    // TxLabel (4), ResponseReject, Signal, Relevant Cap (Reporting), BadPayloadFormat
    let rsp = &[0x43, 0x03, 0x02, 0x18];

    let cmd = &[
        0x40, 0x03, 0x04, 0x08, // TxLabel 4, Signal, ACP (1) and INT (2) SEID
        0x02, 0x01, 0x01, // Reporting, Length 1 (too long), dummy
    ];
    stream_request_response(&mut exec, &mut stream, &mut remote, cmd, rsp);
}

#[test]
fn set_config_reporting_ok() {
    let set_config_cmd = &[
        0x40, 0x03, 0x04, 0x08, // TxLabel 4, Signal, ACP (1) and INT (2) SEID
        0x02, 0x00, // Reporting, Length 0
    ];

    expect_config_recv_cap_okay(
        set_config_cmd,
        StreamEndpointId(1),
        StreamEndpointId(2),
        ServiceCapability::Reporting,
    );
}

// Set Config: Content Protection

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn set_config_bad_content_protection_format(transport_mode: Transport) {
    let (mut exec, mut stream, _, mut remote) = setup_stream_test(transport_mode);

    // TxLabel (4), ResponseReject, Signal, Relevant Cap (CP), BadCpFormat
    let rsp = &[0x43, 0x03, 0x04, 0x27];

    let cmd = &[
        0x40, 0x03, 0x04, 0x08, // TxLabel 4, Signal, ACP (1) and INT (2) SEID
        0x04, 0x01, 0x01, // Content Protection (4), Length 1 (too short), dummy
    ];
    stream_request_response(&mut exec, &mut stream, &mut remote, cmd, rsp);

    let cmd = &[
        0x40, 0x03, 0x04, 0x08, // TxLabel 4, Signal, ACP (1) and INT (2) SEID
        0x04, 0x02, // Content Protection (4), Length 2
        0xF0, 0x0F, // CP Type 0x0FF0 (invalid)
    ];
    stream_request_response(&mut exec, &mut stream, &mut remote, cmd, rsp);
}

#[test]
fn set_config_content_protection_ok() {
    let set_config_cmd = &[
        0x40, 0x03, 0x04, 0x08, // TxLabel 4, Signal, ACP (1) and INT (2) SEID
        0x04, 0x02, // Content Protection (4), Length 2
        0x01, 0x00, // LSB, MSB of DTCP
    ];

    expect_config_recv_cap_okay(
        set_config_cmd,
        StreamEndpointId(1),
        StreamEndpointId(2),
        ServiceCapability::ContentProtection {
            protection_type: ContentProtectionType::DigitalTransmissionContentProtection,
            extra: vec![],
        },
    );
}

// Set Config: Recovery

#[rustfmt::skip]
fn build_recovery_cmd(recovery_type: u8, mrws: u8, mnmp: u8) -> [u8; 9] {
    [
        0x40, 0x03, 0x04, 0x08, // TxLabel 4, Signal, ACP (1) and INT (2) SEID
        0x03, 0x03,             // Recovery (3), length 3
        recovery_type,          // Recovery Type
        mrws,                   // Maximum Recovery Window Size
        mnmp,                   // Maximum Number of Media Packets
    ]
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn set_config_bad_recovery_type(transport_mode: Transport) {
    let (mut exec, mut stream, _, mut remote) = setup_stream_test(transport_mode);

    // TxLabel (4), Signal, Relevant Cap (Recovery), BadRecoveryType
    let rsp = &[0x43, 0x03, 0x03, 0x22];

    // Forbidden Recovery Type
    let cmd = &build_recovery_cmd(0x00, 0x01, 0x01);

    stream_request_response(&mut exec, &mut stream, &mut remote, cmd, rsp);

    // RFD recovery type
    let cmd = &build_recovery_cmd(0x02, 0x01, 0x01);

    stream_request_response(&mut exec, &mut stream, &mut remote, cmd, rsp);
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn set_config_bad_recovery_format(transport_mode: Transport) {
    let (mut exec, mut stream, _, mut remote) = setup_stream_test(transport_mode);

    let cmd = &[
        0x40, 0x03, 0x04, 0x08, // TxLabel 4, Signal, ACP (1) and INT (2) SEID
        0x03, 0x01, 0x01, // Recovery (3), length 1, 0x01
    ];
    // TxLabel 4, ResponseReject, Relevant Cap (Recovery), BadRecoveryFormat
    let rsp = &[0x43, 0x03, 0x03, 0x25];

    stream_request_response(&mut exec, &mut stream, &mut remote, cmd, rsp);

    // Bad Maximum Recovery Window Size (zero is not allowed)
    let cmd = &build_recovery_cmd(0x01, 0x00, 0x01);
    stream_request_response(&mut exec, &mut stream, &mut remote, cmd, rsp);
    // Bad Maximum Recovery Window Size (too large)
    let cmd = &build_recovery_cmd(0x01, 0x20, 0x01);
    stream_request_response(&mut exec, &mut stream, &mut remote, cmd, rsp);
    // Bad Maximum Number of Media Packets (zero is not allowed)
    let cmd = &build_recovery_cmd(0x01, 0x01, 0x00);
    stream_request_response(&mut exec, &mut stream, &mut remote, cmd, rsp);
    // Bad Maximum Number of Media Packets (too large)
    let cmd = &build_recovery_cmd(0x01, 0x01, 0x21);
    stream_request_response(&mut exec, &mut stream, &mut remote, cmd, rsp);
}

#[test]
fn set_config_recovery_ok() {
    let set_config_cmd = &build_recovery_cmd(0x01, 0x01, 0x01);
    expect_config_recv_cap_okay(
        set_config_cmd,
        StreamEndpointId(1),
        StreamEndpointId(2),
        ServiceCapability::Recovery {
            recovery_type: 0x01,
            max_recovery_window_size: 0x01,
            max_number_media_packets: 0x01,
        },
    );
}

// Media Codec

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn set_config_bad_media_codec_format(transport_mode: Transport) {
    let (mut exec, mut stream, _, mut remote) = setup_stream_test(transport_mode);

    let cmd = &[
        0x40, 0x03, 0x04, 0x08, // TxLabel 4, Signal, ACP (1) and INT (2) SEID
        0x07, 0x01, 0x01, // Media Codec (7), length 1, dummy byte
    ];
    // TxLabel 4, ResponseReject, Relevant Cap (Media Codec), BadPayloadFormat
    let rsp = &[0x43, 0x03, 0x07, 0x18];

    stream_request_response(&mut exec, &mut stream, &mut remote, cmd, rsp);

    let cmd = &[
        0x40, 0x03, 0x04, 0x08, // TxLabel 4, Signal, ACP (1) and INT (2) SEID
        0x07, 0x02, // Media Codec (7), length 2
        0x40, // Unknown Media Type, RFA
        0x01, // Media Codec Type
    ];
    stream_request_response(&mut exec, &mut stream, &mut remote, cmd, rsp);
}

#[test]
fn set_config_media_type_ok() {
    // Sample set config command sent from a phone
    let set_config_cmd = &[
        0x40, 0x03, 0x18, 0x04, // TxLabel 4, Signal, ACP (6) and INT (1) SEID
        0x07, 0x06, // Media Codec (7), length 6
        0x00, // Media Type: Audio
        0x00, // Media Codec Type: SBC (0x00)
        0x21, // Sampling Freq: 44.1khz (0x20), Channel Mode Joint Stereo (0x01)
        0x15, // Block length 16 (0x10), 8 Subbands (0x04), Loudness Allocation (0x01)
        0x02, // Minimum bitpool: 2
        0x35, // Maximum bitpool: 53 (0x35)
    ];
    expect_config_recv_cap_okay(
        set_config_cmd,
        StreamEndpointId(6),
        StreamEndpointId(1),
        ServiceCapability::MediaCodec {
            media_type: MediaType::Audio,
            codec_type: MediaCodecType::new(0),
            codec_extra: vec![0x21, 0x15, 0x02, 0x35],
        },
    );
}

// Reconfigure

// Set config must be at least 1 SEID and one configuration long.
incoming_cmd_length_fail_test!(reconfigure_length_too_short, 0x05, 1);

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn reconfigure_invalid_capabilities(transport_mode: Transport) {
    let (mut exec, mut stream, _, mut remote) = setup_stream_test(transport_mode);

    let cmd = &[
        0x40, 0x05, 0x04, // TxLabel 4, Signal, ACP (1) SEID
        0x03, 0x03, // Recovery (3), length 3,
        0x01, 0x01, 0x01, // RFC2733, MRWS, MNMP
    ];
    // TxLabel (4), ResponseReject, Relevant cap (Recovery), Invalid Capabilities
    let rsp = &[0x43, 0x05, 0x03, 0x1A];

    stream_request_response(&mut exec, &mut stream, &mut remote, cmd, rsp);
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn reconfig_event_responder_send_works(transport_mode: Transport) {
    let (mut exec, mut stream, _, mut remote) = setup_stream_test(transport_mode);

    let reconfigure_cmd = &[
        0x40, 0x05, 0x04, // TxLabel 4, Signal, ACP (1) SEID
        0x07, 0x02, // Media Codec, Length 2
        0x00, // Media Type: Audio
        0x00, // Media Codec Type: SBC
    ];

    let mut write_fut = remote.send(reconfigure_cmd.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let respond_res = match next_request(&mut stream, &mut exec) {
        Request::Reconfigure { responder, local_stream_id, capabilities } => {
            assert_eq!(StreamEndpointId(1), local_stream_id);
            assert_eq!(
                ServiceCapability::MediaCodec {
                    media_type: MediaType::Audio,
                    codec_type: MediaCodecType::new(0),
                    codec_extra: vec![],
                },
                capabilities[0]
            );
            responder.send()
        }
        _ => panic!("should have received a Reconfigure"),
    };

    assert!(respond_res.is_ok());

    // Expected response: Same TxLabel (4) & ResponseAccept, Signal,
    expect_remote_recv(&mut exec, &[0x42, 0x05], &mut remote);
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn reconfig_event_responder_reject_works(transport_mode: Transport) {
    let (mut exec, mut stream, _, mut remote) = setup_stream_test(transport_mode);

    let reconfigure_cmd = &[
        0x40, 0x05, 0x04, // TxLabel 4, Signal, ACP (1) SEID
        0x07, 0x02, // Media Codec, Length 2
        0x00, // Media Type: Audio
        0x00, // Media Codec Type: SBC
    ];

    let mut write_fut = remote.send(reconfigure_cmd.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let respond_res = match next_request(&mut stream, &mut exec) {
        Request::Reconfigure { responder, local_stream_id, capabilities } => {
            assert_eq!(StreamEndpointId(1), local_stream_id);
            assert_eq!(
                ServiceCapability::MediaCodec {
                    media_type: MediaType::Audio,
                    codec_type: MediaCodecType::new(0),
                    codec_extra: vec![],
                },
                capabilities[0]
            );
            responder.reject(capabilities[0].category(), ErrorCode::UnsupportedConfiguration)
        }
        _ => panic!("should have received a Reconfigure"),
    };

    assert!(respond_res.is_ok());

    // Expected response: Same TxLabel (4) & ResponseReject, Signal,
    // Relevant capability, Error Code (Unsupported Config)
    expect_remote_recv(&mut exec, &[0x43, 0x05, 0x07, 0x29], &mut remote);
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn reconfigure_command_works(transport_mode: Transport) {
    let mut exec = fasync::TestExecutor::new();
    let (peer, mut remote) = setup_peer(transport_mode);
    // This should cover all the implemented capabilities to confirm they are
    // encoded correctly.
    let caps = vec![
        ServiceCapability::MediaCodec {
            media_type: MediaType::Audio,
            codec_type: MediaCodecType::new(0x40),
            codec_extra: vec![0xDE, 0xAD, 0xBE, 0xEF],
        },
        ServiceCapability::ContentProtection {
            protection_type: ContentProtectionType::DigitalTransmissionContentProtection,
            extra: vec![0xF0, 0x0D],
        },
    ];
    let mut response_fut = Box::pin(peer.reconfigure(&StreamEndpointId(1), &caps));
    assert!(exec.run_until_stalled(&mut response_fut).is_pending());

    let received = recv_remote(&mut exec, &mut remote).unwrap();
    #[rustfmt::skip]
    let reconfigure_req = &[
        0x05,      // Reconfigure Signal
        0x01 << 2, // ACP SEID, RFA
        // Media Codec (LOSC = 2 + 4), Media Type Audio (0x00), Codec type (0x40), Codec specific
        // as above
        0x07, 0x06, 0x00, 0x40, 0xDE, 0xAD, 0xBE, 0xEF,
        // Content Protection (LOSC = 2 + 2), DTCP (0x01, 0x00), CP Specific as above
        0x04, 0x04, 0x01, 0x00, 0xF0, 0x0D,
    ];
    // Last half of header must be Single (0b00) and Command (0b00)
    assert_eq!(0x00, received[0] & 0xF);
    assert_eq!(reconfigure_req, &received[1..]);

    let txlabel_raw = received[0] & 0xF0;

    let complete = exec.run_until_stalled(&mut response_fut);
    assert!(complete.is_pending());

    // Positive response
    let response: &[u8] = &[
        txlabel_raw << 4 | 0x0 << 2 | 0x2, // txlabel (same), Single (0b00), Response Accept (0b10)
        0x05,                              // Reconfigure
    ];
    let mut write_fut = remote.send(response.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let complete = exec.run_until_stalled(&mut response_fut);
    assert_matches!(complete, Poll::Ready(Ok(())));
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn reconfigure_error_response(transport_mode: Transport) {
    let mut exec = fasync::TestExecutor::new();
    let (peer, mut remote) = setup_peer(transport_mode);
    let caps = vec![
        ServiceCapability::MediaTransport,
        ServiceCapability::MediaCodec {
            media_type: MediaType::Audio,
            codec_type: MediaCodecType::new(0x40),
            codec_extra: vec![0xDE, 0xAD, 0xBE, 0xEF],
        },
    ];
    let mut response_fut = Box::pin(peer.reconfigure(&StreamEndpointId(1), &caps));
    // This isn't valid, you can't reconfigure Media Transport
    assert_matches!(exec.run_until_stalled(&mut response_fut), Poll::Ready(Err(Error::Encoding)));

    let caps = vec![ServiceCapability::MediaCodec {
        media_type: MediaType::Audio,
        codec_type: MediaCodecType::new(0x40),
        codec_extra: vec![0xDE, 0xAD, 0xBE, 0xEF],
    }];
    let mut response_fut = Box::pin(peer.reconfigure(&StreamEndpointId(1), &caps));
    assert!(exec.run_until_stalled(&mut response_fut).is_pending());

    let received = recv_remote(&mut exec, &mut remote).unwrap();
    #[rustfmt::skip]
    let reconfigure_req = &[
        0x05,      // Reconfigure Signal
        0x01 << 2, // ACP SEID, RFA
        // Media Codec (LOSC = 2 + 4), Media Type Audio (0x00), Codec type (0x40), Codec specific
        // as above
        0x07, 0x06, 0x00, 0x40, 0xDE, 0xAD, 0xBE, 0xEF,
    ];
    // Last half of header must be Single (0b00) and Command (0b00)
    assert_eq!(0x00, received[0] & 0xF);
    assert_eq!(reconfigure_req, &received[1..]);

    let txlabel_raw = received[0] & 0xF0;

    let complete = exec.run_until_stalled(&mut response_fut);
    assert!(complete.is_pending());

    // Error response
    let response: &[u8] = &[
        txlabel_raw << 4 | 0x0 << 2 | 0x3, // txlabel (same), Single (0b00), Response Reject
        0x05,                              // Reconfigure
        0x00,                              // Relevant Capability (none)
        0x13,                              // SEP In Use
    ];
    let mut write_fut = remote.send(response.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let complete = exec.run_until_stalled(&mut response_fut);
    assert_matches!(
        complete, Poll::Ready(Err(Error::RemoteRejected(e))) if e ==
        RemoteReject::config(
            SignalIdentifier::Reconfigure,
            0x00,
            0x13
        )
    );
}

/// This test covers the valid decoding of all valid ServiceCapabilities.
#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn get_capabilities_command_with_all_service_capabilities_works(transport_mode: Transport) {
    let mut exec = fasync::TestExecutor::new();
    let (peer, mut remote) = setup_peer(transport_mode);
    let seid = &StreamEndpointId::try_from(1).unwrap();
    let mut response_fut = Box::pin(peer.get_capabilities(&seid));
    assert!(exec.run_until_stalled(&mut response_fut).is_pending());

    let received = recv_remote(&mut exec, &mut remote).unwrap();
    // Last half of header must be Single (0b00) and Command (0b00)
    assert_eq!(0x00, received[0] & 0xF);
    assert_eq!(0x02, received[1]); // 0x02 = GetCapabilities
    assert_eq!(0x01 << 2, received[2]); // SEID (0x01) , RFA

    let txlabel_raw = received[0] & 0xF0;

    // This capabilities response should cover all the identifiers in ServiceCategory.
    #[rustfmt::skip]
    let response: &[u8] = &[
        txlabel_raw << 4 | 0x0 << 2 | 0x2, // txlabel (same), Single (0b00), Response Accept (0b10)
        0x02,                              // Get Capabilities
        // MediaTransport (Length of Service Capability = 0)
        0x01, 0x00,
        // Recovery (LOSC = 3), Type (0x01), Window size (2), Number Media Packets (5)
        0x03, 0x03, 0x01, 0x02, 0x05,
        // Media Codec (LOSC = 2 + 2), Video (0x1), Codec type (0x20), Codec specific (0xB0DE)
        0x07, 0x04, 0x10, 0x20, 0xB0, 0xDE,
        // Content Protection (LOSC = 2 + 1), SCMS (0x02, 0x00), CP Specific (0x7A)
        0x04, 0x03, 0x02, 0x00, 0x7A,
        // HeaderCompression (LOSC = 1), BackCh, Media, Recovery (0xE0)
        0x05, 0x01, 0xE0,
        // Multiplexing (LOSC = 7), Frag (0x10), TSID (), TCID () ... TCID ()
        0x06, 0x07, 0x10, 0x00, 0x00, 0x08, 0x08, 0x10, 0x10,
    ];
    let mut write_fut = remote.send(response.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let complete = exec.run_until_stalled(&mut response_fut);

    let capabilities = match complete {
        Poll::Ready(Ok(response)) => response,
        x => panic!("Should have a ready Ok response: {:?}", x),
    };

    assert_eq!(6, capabilities.len());
    let c1 = ServiceCapability::MediaTransport;
    assert_eq!(c1, capabilities[0]);
    let c2 = ServiceCapability::Recovery {
        recovery_type: 1,
        max_recovery_window_size: 2,
        max_number_media_packets: 5,
    };
    assert_eq!(c2, capabilities[1]);
    let c3 = ServiceCapability::MediaCodec {
        media_type: MediaType::Video,
        codec_type: MediaCodecType::new(0x20),
        codec_extra: vec![0xb0, 0xde],
    };
    assert_eq!(c3, capabilities[2]);
    let c4 = ServiceCapability::ContentProtection {
        protection_type: ContentProtectionType::SerialCopyManagementSystem,
        extra: vec![0x7a],
    };
    assert_eq!(c4, capabilities[3]);
    let c5 = ServiceCapability::HeaderCompression { payload_len: 0x01 };
    assert_eq!(c5, capabilities[4]);
    let c6 = ServiceCapability::Multiplexing { payload_len: 0x07 };
    assert_eq!(c6, capabilities[5]);
}

/// This test covers ServiceCapabilities that are both valid and invalid.
/// Decoding these invalid capabilities should be handled gracefully, and they should be ignored.
#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn get_capabilities_command_with_invalid_service_capabilities_works(transport_mode: Transport) {
    let mut exec = fasync::TestExecutor::new();
    let (peer, mut remote) = setup_peer(transport_mode);
    let seid = &StreamEndpointId::try_from(1).unwrap();
    let mut response_fut = Box::pin(peer.get_capabilities(&seid));
    assert!(exec.run_until_stalled(&mut response_fut).is_pending());

    let received = recv_remote(&mut exec, &mut remote).unwrap();
    // Last half of header must be Single (0b00) and Command (0b00)
    assert_eq!(0x00, received[0] & 0xF);
    assert_eq!(0x02, received[1]); // 0x02 = GetCapabilities
    assert_eq!(0x01 << 2, received[2]); // SEID (0x01) , RFA

    let txlabel_raw = received[0] & 0xF0;

    // This capabilities response includes invalid ones. The implementation should filter
    // out any invalid/not supported capabilities and only report the valid ones.
    #[rustfmt::skip]
    let response: &[u8] = &[
        txlabel_raw << 4 | 0x0 << 2 | 0x2, // txlabel (same), Single (0b00), Response Accept (0b10)
        0x02,                              // Get Capabilities
        // MediaTransport (Length of Service Capability = 0)
        0x01, 0x00,
        // Recovery (LOSC = 3), Type (0x01), Window size (2), Number Media Packets (5)
        0x03, 0x03, 0x01, 0x02, 0x05,
        // Invalid Capability that contains payload. Should skip over it.
        0xDD, 0x05, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        // Media Codec (LOSC = 2 + 2), Video (0x1), Codec type (0x20), Codec specific (0xB0DE)
        0x07, 0x04, 0x10, 0x20, 0xB0, 0xDE,
        // Invalid Capability that contains no payload. Should skip over it.
        0xFF, 0x00,
    ];
    let mut write_fut = remote.send(response.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let complete = exec.run_until_stalled(&mut response_fut);

    let capabilities = match complete {
        Poll::Ready(Ok(response)) => response,
        x => panic!("Should have a ready Ok response: {:?}", x),
    };

    assert_eq!(3, capabilities.len());
    let c1 = ServiceCapability::MediaTransport;
    assert_eq!(c1, capabilities[0]);
    let c2 = ServiceCapability::Recovery {
        recovery_type: 1,
        max_recovery_window_size: 2,
        max_number_media_packets: 5,
    };
    assert_eq!(c2, capabilities[1]);
    let c3 = ServiceCapability::MediaCodec {
        media_type: MediaType::Video,
        codec_type: MediaCodecType::new(0x20),
        codec_extra: vec![0xb0, 0xde],
    };
    assert_eq!(c3, capabilities[2]);
}

/// This test covers ServiceCapabilities that are only invalid.
#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn get_capabilities_command_with_only_invalid_service_capabilities_works(
    transport_mode: Transport,
) {
    let mut exec = fasync::TestExecutor::new();
    let (peer, mut remote) = setup_peer(transport_mode);
    let seid = &StreamEndpointId::try_from(1).unwrap();
    let mut response_fut = Box::pin(peer.get_capabilities(&seid));
    assert!(exec.run_until_stalled(&mut response_fut).is_pending());

    let received = recv_remote(&mut exec, &mut remote).unwrap();
    // Last half of header must be Single (0b00) and Command (0b00)
    assert_eq!(0x00, received[0] & 0xF);
    assert_eq!(0x02, received[1]); // 0x02 = GetCapabilities
    assert_eq!(0x01 << 2, received[2]); // SEID (0x01) , RFA

    let txlabel_raw = received[0] & 0xF0;

    // Response only includes invalid ServiceCapabilities.
    #[rustfmt::skip]
    let response: &[u8] = &[
        txlabel_raw << 4 | 0x0 << 2 | 0x2, // txlabel (same), Single (0b00), Response Accept (0b10)
        0x02,                              // Get Capabilities
        // Invalid Capability that contains payload. Should skip over it.
        0x4F, 0x05, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        // Invalid Capability that contains no payload. Should skip over it.
        0xAA, 0x00,
    ];
    let mut write_fut = remote.send(response.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let complete = exec.run_until_stalled(&mut response_fut);

    let capabilities = match complete {
        Poll::Ready(Ok(response)) => response,
        x => panic!("Should have a ready Ok response: {:?}", x),
    };

    assert_eq!(0, capabilities.len());
}

// DelayReport tests
const CMD_DELAY_REPORT_VALUE: &'static u8 = &0x0D;
const CMD_DELAY_REPORT: &'static [u8] = &[
    0x40, // TxLabel (4), Single Packet (0b00), Command (0b00)
    0x0D, // RFA (0b00), Delay Report (0x0D)
    0x30, // SEID 12 (0b001100)
    0x12, // Delay (MSB) = 0x12
    0x34, // Delay (LSB) = 0x34
];

const DELAY_VALUE: u16 = 0x1234;

incoming_cmd_length_fail_test!(delay_report_too_short, *CMD_DELAY_REPORT_VALUE, 2);
incoming_cmd_length_fail_test!(delay_report_too_long, *CMD_DELAY_REPORT_VALUE, 4);

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn delay_report_event_responder_send_works(transport_mode: Transport) {
    let (mut exec, mut stream, _, mut remote) = setup_stream_test(transport_mode);

    let mut write_fut = remote.send(CMD_DELAY_REPORT.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let respond_res = match next_request(&mut stream, &mut exec) {
        Request::DelayReport { responder, stream_id, delay } => {
            assert_eq!(StreamEndpointId::try_from(12).unwrap(), stream_id);
            assert_eq!(DELAY_VALUE, delay);
            responder.send()
        }
        _ => panic!("should have received a DelayReport"),
    };

    assert!(respond_res.is_ok());

    // Should receive the encoded version of the MediaTransport
    #[rustfmt::skip]
    let delay_report_rsp = &[
        0x42, // TxLabel (4) + ResponseAccept (0x02)
        0x0D, // DelayReport Signal
    ];
    expect_remote_recv(&mut exec, delay_report_rsp, &mut remote);
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn delay_report_responder_reject_works(transport_mode: Transport) {
    let (mut exec, mut stream, _, mut remote) = setup_stream_test(transport_mode);

    let mut write_fut = remote.send(CMD_DELAY_REPORT.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let respond_res = match next_request(&mut stream, &mut exec) {
        Request::DelayReport { responder, stream_id, delay } => {
            assert_eq!(StreamEndpointId::try_from(12).unwrap(), stream_id);
            assert_eq!(DELAY_VALUE, delay);
            responder.reject(ErrorCode::BadAcpSeid)
        }
        _ => panic!("should have received a DelayReport"),
    };

    assert!(respond_res.is_ok());

    let delay_report_rsp = &[
        0x43, // TxLabel (4),  RemoteRejected (3)
        0x0D, // Delay Report
        0x12, // BAD_ACP_SEID
    ];
    expect_remote_recv(&mut exec, delay_report_rsp, &mut remote);
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn delay_report_command_works(transport_mode: Transport) {
    let mut exec = fasync::TestExecutor::new();
    let (peer, mut remote) = setup_peer(transport_mode);
    let seid = &StreamEndpointId::try_from(1).unwrap();
    let mut response_fut = Box::pin(peer.delay_report(&seid, 0xc0de));
    assert!(exec.run_until_stalled(&mut response_fut).is_pending());

    let received = recv_remote(&mut exec, &mut remote).unwrap();
    // Last half of header must be Single (0b00) and Command (0b00)
    assert_eq!(0x00, received[0] & 0xF);
    assert_eq!(0x0D, received[1]); // 0x0D = DelayReport
    assert_eq!(0x01 << 2, received[2]); // SEID (0x01) , RFA
    assert_eq!(0xc0, received[3]);
    assert_eq!(0xde, received[4]);

    let txlabel_raw = received[0] & 0xF0;

    #[rustfmt::skip]
    let response: &[u8] = &[
        txlabel_raw << 4 | 0x0 << 2 | 0x2, // txlabel (same), Single (0b00), Response Accept (0b10)
        0x0D,                              // Delay Report
    ];
    let mut write_fut = remote.send(response.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let complete = exec.run_until_stalled(&mut response_fut);

    match complete {
        Poll::Ready(Ok(())) => {}
        x => panic!("Should have a ready Ok response: {:?}", x),
    };
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn delay_report_reject_command(transport_mode: Transport) {
    let mut exec = fasync::TestExecutor::new();
    let (peer, mut remote) = setup_peer(transport_mode);
    let seid = &StreamEndpointId::try_from(1).unwrap();
    let mut response_fut = Box::pin(peer.delay_report(&seid, 0xc0de));
    assert!(exec.run_until_stalled(&mut response_fut).is_pending());

    let received = recv_remote(&mut exec, &mut remote).unwrap();
    // Last half of header must be Single (0b00) and Command (0b00)
    assert_eq!(0x00, received[0] & 0xF);
    assert_eq!(0x0D, received[1]); // 0x0D = DelayReport
    assert_eq!(0x01 << 2, received[2]); // SEID (0x01), RFA

    let txlabel_raw = received[0] & 0xF0;

    let response: &[u8] = &[
        txlabel_raw | 0x0 << 2 | 0x3, // txlabel (same), Single (0b00), Response Reject (0b11)
        0x0D,                         // Delay Report
        0x12,                         // BAD_ACP_SEID
    ];

    let mut write_fut = remote.send(response.to_vec());
    exec.run_until_stalled(&mut write_fut).expect("should be ready").expect("write should succeed");

    let complete = exec.run_until_stalled(&mut response_fut);

    let error = match complete {
        Poll::Ready(Err(response)) => response,
        x => panic!("Should have a ready Error response: {:?}", x),
    };

    assert_matches!(error, Error::RemoteRejected(e) if e == RemoteReject::rejected(SignalIdentifier::DelayReport, 0x12));
}

#[test]
fn get_capabilities_response_decode_single_byte_error() {
    let payload = &[0x01];
    let res = GetCapabilitiesResponse::decode(payload);
    assert_matches!(res, Err(Error::Encoding));
}

#[test]
fn get_capabilities_response_decode_malformed_length_error() {
    let payload = &[
        0x01, // ServiceCategory (e.g., MediaTransport)
        0x0a, // Claimed capability length: 10 bytes (only 0 extra bytes remaining)
    ];
    let res = GetCapabilitiesResponse::decode(payload);
    assert_matches!(res, Err(Error::Encoding));
}

#[test]
fn get_capabilities_response_decode_skips_unsupported_capability() {
    #[rustfmt::skip]
    let payload = &[
        // Valid MediaTransport (Category 0x01, Length 0x00)
        0x01, 0x00,
        // Unsupported ServiceCategory (0xff, Length 0x01, Payload 0x00)
        0xff, 0x01, 0x00,
        // Valid Reporting (Category 0x02, Length 0x00)
        0x02, 0x00,
    ];
    let res = GetCapabilitiesResponse::decode(payload);
    assert_matches!(res, Ok(_));
    let response = res.unwrap();
    let caps: Vec<ServiceCapability> = response.into();
    assert_eq!(caps.len(), 2);
    assert_matches!(caps[0], ServiceCapability::MediaTransport);
    assert_matches!(caps[1], ServiceCapability::Reporting);
}

#[test]
fn get_capabilities_response_decode_skips_malformed_recognized_capability() {
    #[rustfmt::skip]
    let payload = &[
        // Recognized MediaTransport (Category 0x01) but malformed length (0x01 instead of 0x00)
        0x01, 0x01, 0x00,
        // Valid Reporting (Category 0x02, Length 0x00)
        0x02, 0x00,
    ];
    let res = GetCapabilitiesResponse::decode(payload);
    assert_matches!(res, Ok(_));
    let response = res.unwrap();
    let caps: Vec<ServiceCapability> = response.into();
    assert_eq!(caps.len(), 1);
    assert_matches!(caps[0], ServiceCapability::Reporting);
}

#[test]
fn get_capabilities_response_decode_malformed_unrecognized_capability_fails() {
    #[rustfmt::skip]
    let payload = &[
        // Valid MediaTransport (Category 0x01, Length 0x00)
        0x01, 0x00,
        // Unsupported ServiceCategory (0xff) but truncated length (claims 10 bytes, only 1 byte left)
        0xff, 0x0a, 0x00,
    ];
    let res = GetCapabilitiesResponse::decode(payload);
    assert_matches!(res, Err(Error::Encoding));
}

#[test]
fn get_capabilities_response_decode_empty_payload() {
    let payload = &[];
    let res = GetCapabilitiesResponse::decode(payload);
    assert_matches!(res, Ok(_));
    let response = res.unwrap();
    let caps: Vec<ServiceCapability> = response.into();
    assert!(caps.is_empty());
}
