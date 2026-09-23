// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_utils::PollExt;
use bt_channel_test_support::{Transport, create_test_channels};
use core::task::Poll;
use fuchsia_async as fasync;
use futures::{SinkExt, StreamExt};
use test_case::test_case;
use zx::{self as zx};

use super::*;
use crate::avctp::MessageType as AvctpMessageType;

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn closes_channel_when_dropped(transport_mode: Transport) {
    let mut exec = fasync::TestExecutor::new();
    let (peer_chan, mut control) = create_test_channels(transport_mode);

    {
        let peer = Peer::new(peer_chan);
        let mut _stream = peer.take_command_stream();
    }

    let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

    let mut send_fut = control.send(vec![0; 1]);
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

pub(crate) fn setup_peer(transport_mode: Transport) -> (Peer, Channel) {
    let (control, remote) = create_test_channels(transport_mode);

    let peer = Peer::new(remote);
    (peer, control)
}

fn setup_stream_test(
    transport_mode: Transport,
) -> (fasync::TestExecutor, CommandStream, Peer, Channel) {
    let exec = fasync::TestExecutor::new();
    let (peer, remote) = setup_peer(transport_mode);
    let stream = peer.take_command_stream();
    (exec, stream, peer, remote)
}

pub(crate) fn expect_remote_recv(
    exec: &mut fasync::TestExecutor,
    expected: &[u8],
    remote: &mut Channel,
) {
    let mut fut = remote.next();
    let r = match exec.run_until_stalled(&mut fut) {
        Poll::Ready(Some(res)) => res,
        Poll::Ready(None) => Err(zx::Status::PEER_CLOSED),
        Poll::Pending => Err(zx::Status::SHOULD_WAIT),
    };
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
fn closed_peer_ends_request_stream(transport_mode: Transport) {
    let (mut exec, mut stream, _peer, remote) = setup_stream_test(transport_mode);
    drop(remote);
    assert!(exec.run_until_stalled(&mut stream.next()).expect("ready").is_none());
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn send_stop_avc_passthrough_command_timeout(transport_mode: Transport) {
    let (mut exec, _stream, peer, mut channel) = setup_stream_test(transport_mode);
    let mut cmd_fut = Box::pin(peer.send_avc_passthrough_command(&[69, 0]));
    let poll_ret: Poll<Result<CommandResponse>> = exec.run_until_stalled(&mut cmd_fut);
    assert!(poll_ret.is_pending());

    expect_remote_recv(
        &mut exec,
        &[
            0x00, // TxLabel 0, Single 0, Command 0, Ipid 0,
            0x11, // AV PROFILE
            0x0e, // AV PROFILE
            0x00, // command: Control
            0x48, // panel subunit_type 9 (<< 3), subunit_id 0
            0x7c, // op code: passthrough
            0x45, // random keypress
            0x00, // passthrough payload
        ],
        &mut channel,
    );

    let _ = exec.wake_next_timer();
    assert_eq!(Poll::Ready(Err(Error::Timeout)), exec.run_until_stalled(&mut cmd_fut));
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn send_stop_avc_passthrough_command(transport_mode: Transport) {
    let (mut exec, _stream, peer, mut channel) = setup_stream_test(transport_mode);
    let mut cmd_fut = Box::pin(peer.send_avc_passthrough_command(&[69, 0]));
    let poll_ret: Poll<Result<CommandResponse>> = exec.run_until_stalled(&mut cmd_fut);
    assert!(poll_ret.is_pending());

    expect_remote_recv(
        &mut exec,
        &[
            0x00, // TxLabel 0, Single 0, Command 0, Ipid 0,
            0x11, // AV PROFILE
            0x0e, // AV PROFILE
            0x00, // command: Control
            0x48, // panel subunit_type 9 (<< 3), subunit_id 0
            0x7c, // op code: passthrough
            0x45, // random keypress
            0x00, // passthrough payload
        ],
        &mut channel,
    );

    let write_buf = &[
        0x02, // TxLabel 0, Single 0, Response 1, Ipid 0,
        0x11, // AV PROFILE
        0x0e, // AV PROFILE
        0x09, // response: Accepted
        0x48, // panel subunit_type 9 (<< 3), subunit_id 0
        0x7c, // op code: passthrough
        0x45, // random keypress
        0x00, // passthrough payload
    ];

    exec.run_until_stalled(&mut channel.send(write_buf.to_vec()))
        .expect("signaling write")
        .expect("write successful");
    let poll_ret = exec.run_until_stalled(&mut cmd_fut);
    let command_response = match poll_ret {
        Poll::Ready(Ok(response)) => response,
        x => panic!("Should have had an Ready OK response and got {:?}", x),
    };
    assert_eq!(ResponseType::Accepted, command_response.0);
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn receive_register_notification_command(transport_mode: Transport) {
    let (mut exec, mut stream, _peer, mut channel) = setup_stream_test(transport_mode);
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
    exec.run_until_stalled(&mut channel.send(notif_command_packet.to_vec()))
        .expect("signaling write")
        .expect("write successful");
    let command = next_request(&mut stream, &mut exec);
    assert!(command.avctp_header().is_type(&AvctpMessageType::Command));
    assert!(command.avctp_header().is_single());
    assert_eq!(PacketType::Command(CommandType::Notify), command.avc_header().packet_type()); // NOTIFY
    assert_eq!(&OpCode::VendorDependent, command.avc_header().op_code());
    assert_eq!(Some(SubunitType::Panel), command.avc_header().subunit_type());
    assert_eq!(
        &[
            // vendor specific payload (register notification for volume change)
            0x31, // register notification Pdu_ID
            0x00, // reserved/packet type
            0x00, 0x05, // parameter len
            0x0D, // Event ID
            0x00, 0x00, 0x00, 0x00, // Playback interval
        ],
        command.body(),
    );
    assert!(command.send_response(ResponseType::NotImplemented, &[]).is_ok());
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
        &mut channel,
    );
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn receive_unit_info(transport_mode: Transport) {
    let (mut exec, mut stream, _peer, mut channel) = setup_stream_test(transport_mode);
    let command_packet = &[
        0x00, // TxLabel 0, Single 0, Command 0, Ipid 0,
        0x11, // AV PROFILE
        0x0e, // AV PROFILE
        0x01, // command: Status
        0xff, // unit subunit_type 0x1F (<< 3), subunit_id 7
        0x30, // opcode: unit info
        0xff, 0xff, 0xff, 0xff, 0xff, // pad
    ];
    exec.run_until_stalled(&mut channel.send(command_packet.to_vec()))
        .expect("write to channel success")
        .expect("write successful");

    let mut fut = stream.next();
    let complete = exec.run_until_stalled(&mut fut); // wake and pump.
    assert!(complete.is_pending());

    expect_remote_recv(
        &mut exec,
        &[
            0x02, // TxLabel 0, Single 0, response 1, Ipid 0,
            0x11, // AV PROFILE
            0x0e, // AV PROFILE
            0x0c, // Response: stable
            0xff, // unit subunit_type 0x1F (<< 3), subunit_id 7
            0x30, // opcode: unit info
            0x07, // constant
            0x48, // SubunitType::Panel
            0xff, 0xff, 0xff, // generic company ID.
        ],
        &mut channel,
    );
}

#[test_case(Transport::Socket ; "socket")]
#[test_case(Transport::Fidl ; "fidl")]
#[fuchsia::test]
fn receive_subunit_info(transport_mode: Transport) {
    let (mut exec, mut stream, _peer, mut channel) = setup_stream_test(transport_mode);
    let command_packet = &[
        0x00, // TxLabel 0, Single 0, Command 0, Ipid 0,
        0x11, // AV PROFILE
        0x0e, // AV PROFILE
        0x01, // command: Status
        0xff, // unit subunit_type 0x1F (<< 3), subunit_id 7
        0x31, // opcode: sub_unit info
        0x07, // extension code
        0xff, 0xff, 0xff, 0xff, // pad
    ];
    exec.run_until_stalled(&mut channel.send(command_packet.to_vec()))
        .expect("write to channel success")
        .expect("write successful");

    let mut fut = stream.next();
    let complete = exec.run_until_stalled(&mut fut); // wake and pump.
    assert!(complete.is_pending());

    expect_remote_recv(
        &mut exec,
        &[
            0x02, // TxLabel 0, Single 0, response 1, Ipid 0,
            0x11, // AV PROFILE
            0x0e, // AV PROFILE
            0x0c, // Response: stable
            0xff, // unit subunit_type 0x1F (<< 3), subunit_id 7
            0x31, // opcode: sub unit info
            0x07, // extension code
            0x48, // SubunitType::Panel
            0xff, 0xff, 0xff, // padding
        ],
        &mut channel,
    );
}
