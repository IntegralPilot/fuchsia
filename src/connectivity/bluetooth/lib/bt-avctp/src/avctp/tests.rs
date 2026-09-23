// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_utils::PollExt;
use bt_channel_test_support::{Transport, create_test_channels};
use core::task::Poll;
use fuchsia_async as fasync;
use futures::{SinkExt, StreamExt};
use std::result;
use test_case::test_case;
use zx::{self as zx};

use super::*;

pub(crate) fn setup_peer(transport_mode: Transport) -> (Peer, Channel) {
    let (signaling, remote) = create_test_channels(transport_mode);

    let peer = Peer::new(remote);
    (peer, signaling)
}

fn setup_stream_test(
    transport_mode: Transport,
) -> (fasync::TestExecutor, CommandStream, Peer, Channel) {
    let exec = fasync::TestExecutor::new();
    let (peer, remote) = setup_peer(transport_mode);
    let stream = peer.take_command_stream();
    (exec, stream, peer, remote)
}

#[track_caller]
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
    let r = recv_remote(exec, remote);
    assert!(r.is_ok());
    let response = r.unwrap();
    if expected.len() != response.len() {
        panic!("received wrong length\nexpected: {:?}\nreceived: {:?}", expected, response);
    }
    assert_eq!(expected, &response[0..expected.len()]);
}

fn next_request(stream: &mut CommandStream, exec: &mut fasync::TestExecutor) -> Command {
    let mut fut = stream.next();
    let complete = exec.run_until_stalled(&mut fut);

    match complete {
        Poll::Ready(Some(Ok(r))) => r,
        _ => panic!("should have a request"),
    }
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn closes_socket_when_dropped(transport_mode: Transport) {
    let mut exec = fasync::TestExecutor::new();
    let (control, mut peer_chan) = create_test_channels(transport_mode);

    {
        let peer = Peer::new(control);
        let mut _stream = peer.take_command_stream();
    }

    let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

    let mut send_fut = peer_chan.send(vec![0; 1]);
    let write_res = exec.run_until_stalled(&mut send_fut);
    assert!(matches!(write_res, Poll::Ready(Err(_))));
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn socket_open_when_stream_open(transport_mode: Transport) {
    let mut exec = fasync::TestExecutor::new();
    let (control, mut peer_chan) = create_test_channels(transport_mode);

    {
        let mut _stream;
        {
            let peer = Peer::new(control);
            _stream = peer.take_command_stream();
        }

        // Writing to the sock from the other end should pass.
        exec.run_until_stalled(&mut peer_chan.send(vec![0; 1]))
            .expect("signaling write")
            .expect("write successful");
    }

    let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

    // Writing to the sock from the other end should fail.
    let mut send_fut = peer_chan.send(vec![0; 1]);
    let write_res = exec.run_until_stalled(&mut send_fut);
    assert!(matches!(write_res, Poll::Ready(Err(_))));
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
#[should_panic(expected = "Command stream has already been taken")]
fn can_only_take_stream_once(transport_mode: Transport) {
    let mut _exec = fasync::TestExecutor::new();
    let (control, _) = create_test_channels(transport_mode);

    let peer = Peer::new(control);
    let mut _stream = peer.take_command_stream();
    let mut _stream2 = peer.take_command_stream();
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn closed_peer_ends_request_stream(transport_mode: Transport) {
    let (mut exec, mut stream, _peer, remote) = setup_stream_test(transport_mode);
    drop(remote);
    assert!(exec.run_until_stalled(&mut stream.next()).expect("ready").is_none());
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn send_command_receive_response(transport_mode: Transport) {
    let (mut exec, _stream, peer, mut socket) = setup_stream_test(transport_mode);

    // sending random payload.
    let mut command_stream =
        peer.send_command(&[22, 33, 44, 55]).expect("Unable to get command stream");

    // Assuming we got assigned TxLabel(0) here. This might be flaky.
    // Sending random payload.
    expect_remote_recv(
        &mut exec,
        &[
            0x00, // TxLabel 0, Single 0, Command 0, Ipid 0,
            0x11, // AV PROFILE
            0x0e, // AV PROFILE
            22, 33, 44, 55, // Random payload should match above
        ],
        &mut socket,
    );

    let mut response_fut = command_stream.next();
    let stream_ret: Poll<Option<Result<Packet>>> = exec.run_until_stalled(&mut response_fut);
    assert!(stream_ret.is_pending());
    exec.run_until_stalled(&mut socket.send(vec![
        0x02, // TxLabel 0, Single 0, Response 1, Ipid 0,
        0x11, // AV PROFILE
        0x0e, // AV PROFILE
        66, 77, 88, 99, // Random Payload
    ]))
    .expect("signaling write")
    .expect("write successful"); // Response accept packet

    let stream_ret: Poll<Option<Result<Packet>>> = exec.run_until_stalled(&mut response_fut);
    assert!(stream_ret.is_ready());
    if let Poll::Ready(Some(Ok(packet))) = stream_ret {
        // Random Payload should match what we expected
        assert_eq!(&[66, 77, 88, 99], packet.body());
        assert!(packet.header().is_single());
        assert!(packet.header().is_type(&MessageType::Response));
        assert_eq!(&TxLabel::try_from(0).unwrap(), packet.header().label());
    } else {
        panic!("Invalid stream result");
    }
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn receive_command_send_response(transport_mode: Transport) {
    let (mut exec, mut stream, _peer, mut socket) = setup_stream_test(transport_mode);
    let notif_command_packet = &[
        0x00, // TxLabel 0, Single 0, Command 0, Ipid 0,
        0x11, // AV PROFILE
        0x0e, // AV PROFILE
        0x03, // command: Notify
        0x48, // panel subunit_type 9 (<< 3), subunit_id 0
        0x00, // op code: VendorDependent
        0x00, 0x19, 0x58, // bit sig company id
        // vendor specific payload (register notification for volume change)
        0x31, // register notification Pdu_ID
        0x00, // reserved/packet type
        0x00, 0x05, // parameter len
        0x0D, // Event ID
        0x00, 0x00, 0x00, 0x00, // Playback interval
    ];
    exec.run_until_stalled(&mut socket.send(notif_command_packet.to_vec()))
        .expect("signaling write")
        .expect("write successful");
    let command = next_request(&mut stream, &mut exec);
    assert!(command.header().is_type(&MessageType::Command));
    assert!(command.header().is_single());
    assert_eq!(
        // body should match the same payload above
        &[
            0x03, // command: Notify
            0x48, // panel subunit_type 9 (<< 3), subunit_id 0
            0x00, // op code: VendorDependent
            0x00, 0x19, 0x58, // bit sig company id
            // vendor specific payload (register notification for volume change)
            0x31, // register notification Pdu_ID
            0x00, // reserved/packet type
            0x00, 0x05, // parameter len
            0x0D, // Event ID
            0x00, 0x00, 0x00, 0x00, // Playback interval
        ],
        command.body()
    );
    assert!(
        command
            .send_response(&[
                0x08, // response: NotImplemented
                0x48, // panel subunit_type 9 (<< 3), subunit_id 0
                0x00, // op code: VendorDependent
                0x00, 0x19, 0x58, // bit sig company id
            ],)
            .is_ok()
    );
    expect_remote_recv(
        &mut exec,
        &[
            0x02, // TxLabel 0, Single 0, Response 1, Ipid 0,
            0x11, // AV PROFILE
            0x0e, // AV PROFILE
            0x08, // response: NotImplemented
            0x48, // panel subunit_type 9 (<< 3), subunit_id 0
            0x00, // op code: VendorDependent
            0x00, 0x19, 0x58, // bit sig company id
        ],
        &mut socket,
    );
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn receive_command_too_short_is_dropped(transport_mode: Transport) {
    let (mut exec, mut stream, _peer, mut socket) = setup_stream_test(transport_mode);
    let notif_command_packet = &[
        // No payload. Only a command
        0x00, // TxLabel 0, Single 0, Command 0, Ipid 0,
        0x11, // AV PROFILE
        0x0e, // AV PROFILE
    ];
    exec.run_until_stalled(&mut socket.send(notif_command_packet.to_vec()))
        .expect("signaling write")
        .expect("write successful");

    let mut fut = stream.next();
    let complete = exec.run_until_stalled(&mut fut);
    assert!(complete.is_pending());
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn receive_invalid_is_dropped(transport_mode: Transport) {
    let (mut exec, mut stream, _peer, mut socket) = setup_stream_test(transport_mode);
    let notif_command_packet = &[0];
    exec.run_until_stalled(&mut socket.send(notif_command_packet.to_vec()))
        .expect("signaling write")
        .expect("write successful");

    let mut fut = stream.next();
    let complete = exec.run_until_stalled(&mut fut);
    assert!(complete.is_pending());
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn invalid_profile_id_response(transport_mode: Transport) {
    let (mut exec, mut stream, _peer, mut socket) = setup_stream_test(transport_mode);
    let notif_command_packet = &[
        // command for wrong profile id
        0x03, // TxLabel 0, Single 0, Response 1, Ipid 1,
        0x11, 0x00, // random profile ID
        3, 72, 0, // random payload
    ];
    exec.run_until_stalled(&mut socket.send(notif_command_packet.to_vec()))
        .expect("write to channel success")
        .expect("write successful");

    let mut fut = stream.next();
    let complete = exec.run_until_stalled(&mut fut); // wake and pump.
    assert!(complete.is_pending());

    expect_remote_recv(
        &mut exec,
        &[
            // Ipid bit should be set.
            0x03, // TxLabel 0, Single 0, Response 1, Ipid 1,
            0x11, 0x00, // random profile ID same as above
        ],
        &mut socket,
    ); // receive invalid profile response
}
