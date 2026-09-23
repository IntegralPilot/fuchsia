// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bt_common::{PeerId, Uuid};
use bt_gatt::test_utils::{FakeServer, FakeServerEvent, FakeTypes};
use bt_gatt::types::Handle;

use futures::channel::mpsc::UnboundedReceiver;
use futures::{Stream, StreamExt};
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::server::AudioStreamControlServiceServer;
use crate::server::ServiceEvent;
use crate::server::{ASCS_SERVICE_ID, ASCS_UUID};
use crate::types::CodecConfiguration;
use crate::*;

#[track_caller]
fn expect_service_event(events: &mut UnboundedReceiver<FakeServerEvent>) -> FakeServerEvent {
    match events.poll_next_unpin(&mut futures_test::task::noop_context()) {
        Poll::Ready(Some(event)) => event,
        x => panic!("Expected fake server event, got {x:?}"),
    }
}

#[test]
fn publishes() {
    let mut ascs_server = std::pin::pin!(AudioStreamControlServiceServer::<FakeTypes>::new(1, 1));

    let (count_waker, woken_count) = futures_test::task::new_count_waker();

    // Polling the server before it's published should result in Poll::Pending
    let poll_result = ascs_server.as_mut().poll_next(&mut Context::from_waker(&count_waker));

    assert!(poll_result.is_pending());
    assert_eq!(woken_count.get(), 0);

    let (fake_server, mut events) = FakeServer::new();

    let result = ascs_server.publish(&fake_server);
    assert!(result.is_ok());

    let already_published = ascs_server.publish(&fake_server);

    assert!(already_published.is_err());

    assert_eq!(woken_count.get(), 1);

    let poll_result = ascs_server.poll_next(&mut Context::from_waker(&count_waker));

    match expect_service_event(&mut events) {
        FakeServerEvent::Published { id: _, definition } => {
            assert_eq!(definition.uuid(), ASCS_UUID);
            assert_eq!(
                definition.characteristics().filter(|c| c.uuid == Uuid::from_u16(0x2BC4)).count(),
                1
            );
            assert_eq!(
                definition.characteristics().filter(|c| c.uuid == Uuid::from_u16(0x2BC5)).count(),
                1
            );
        }
        x => panic!("Expected published event, got {x:?}"),
    };

    // Should still be pending, even though we had some work to do.
    assert!(poll_result.is_pending());
}

fn register_for_notification(fake_server: &mut FakeServer, peer_id: PeerId, handle: Handle) {
    fake_server.incoming_client_configuration(
        peer_id,
        ASCS_SERVICE_ID,
        handle,
        bt_gatt::server::NotificationType::Notify,
    );
}

// A published server with one sink and one source.
fn published_server() -> (
    Pin<Box<AudioStreamControlServiceServer<FakeTypes>>>,
    FakeServer,
    UnboundedReceiver<FakeServerEvent>,
) {
    let mut ascs_server = Box::pin(AudioStreamControlServiceServer::<FakeTypes>::new(1, 1));
    let (mut fake_server, mut events) = FakeServer::new();

    let result = ascs_server.publish(&fake_server);
    assert!(result.is_ok());

    assert!(ascs_server.poll_next_unpin(&mut futures_test::task::noop_context()).is_pending());

    assert!(matches!(expect_service_event(&mut events), FakeServerEvent::Published { .. }));

    for peer_id in [PeerId(1), PeerId(2)] {
        for handle in [Handle(1), Handle(2), Handle(3)] {
            register_for_notification(&mut fake_server, peer_id, handle);
        }
    }

    (ascs_server, fake_server, events)
}

fn poll_server(
    server: &mut Pin<Box<AudioStreamControlServiceServer<FakeTypes>>>,
) -> Poll<Option<core::result::Result<ServiceEvent, Error>>> {
    server.poll_next_unpin(&mut futures_test::task::noop_context())
}

#[test]
fn peers_are_separated() {
    let (mut ascs_server, fake_server, mut server_events) = published_server();

    // Read the sink uuid
    fake_server.incoming_read(PeerId(1), ASCS_SERVICE_ID, Handle(3), 0);
    // Poll the ascs server, should not result in an ASCS event
    assert!(poll_server(&mut ascs_server).is_pending());

    // Should have the response
    let mut peer_one_value;
    match server_events.poll_next_unpin(&mut futures_test::task::noop_context()) {
        Poll::Ready(Some(FakeServerEvent::ReadResponded { service_id, handle: _, value })) => {
            assert_eq!(service_id, ASCS_SERVICE_ID);
            peer_one_value = value.unwrap();
        }
        x => panic!("Expected the read to be responded to got {x:?}"),
    };

    let ase_id = peer_one_value[0];

    // Codec Configure the first peer ase_id
    fake_server.incoming_write(
        PeerId(1),
        ASCS_SERVICE_ID,
        Handle(1),
        0,
        vec![0x01, 0x01, ase_id, 0x01, 0x01, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00],
    );

    use crate::server::ServiceEvent;
    use crate::types::*;
    // Should have a codec configure event to respond to
    match poll_server(&mut ascs_server) {
        Poll::Ready(Some(Ok(ServiceEvent::CodecConfigure { responder, .. }))) => {
            responder.accept(
                Framing::Unframed,
                vec![Phy::Le1MPhy],
                5,
                std::time::Duration::from_millis(20).try_into().unwrap(),
                PresentationDelayRange::build(0, 500).unwrap(),
            );
        }
        x => panic!("Expected a CodecConfigure, got {x:?}"),
    };

    // Expect the write to be responded to / acknowledged and a notification from
    // the CP handle and the Source ASE
    match expect_service_event(&mut server_events) {
        FakeServerEvent::WriteResponded { value, .. } => assert!(value.is_ok()),
        x => panic!("Expected acknowledge of write, got {x:?}"),
    };
    assert!(poll_server(&mut ascs_server).is_pending());
    match expect_service_event(&mut server_events) {
        FakeServerEvent::Notified { handle, .. } => assert_eq!(Handle(1), handle),
        x => panic!("Expected acknowledge of write, got {x:?}"),
    };
    match expect_service_event(&mut server_events) {
        FakeServerEvent::Notified { peers, handle, value, .. } => {
            assert!(peers.contains(&PeerId(1)));
            assert_eq!(Handle(3), handle);
            // Should be the same ASE_ID
            assert_eq!(value[0], ase_id);
            assert_eq!(value[1], 0x01); // State is CodecConfigured
            peer_one_value = value;
        }
        x => panic!("Expected acknowledge of write, got {x:?}"),
    };

    // Read the sink id from another peer
    fake_server.incoming_read(PeerId(2), ASCS_SERVICE_ID, Handle(3), 0);

    assert!(poll_server(&mut ascs_server).is_pending());
    assert!(poll_server(&mut ascs_server).is_pending());

    let peer_two_value;
    match expect_service_event(&mut server_events) {
        FakeServerEvent::ReadResponded { service_id, handle: _, value } => {
            assert_eq!(service_id, ASCS_SERVICE_ID);
            peer_two_value = value.unwrap();
        }
        x => panic!("Expected the read to be responded to got {x:?}"),
    };

    assert!(peer_one_value[1] != peer_two_value[1]);
}

#[track_caller]
fn expect_control_point_notified(
    ascs_server: &mut Pin<Box<AudioStreamControlServiceServer<FakeTypes>>>,
    server_events: &mut UnboundedReceiver<FakeServerEvent>,
) -> Vec<u8> {
    // Expect the write to be responded to / acknowledged and a notification from
    // the CP handle and the Source ASE
    match expect_service_event(server_events) {
        FakeServerEvent::WriteResponded { value, .. } => assert!(value.is_ok()),
        x => panic!("Expected acknowledge of write, got {x:?}"),
    };
    assert!(poll_server(ascs_server).is_pending());
    match expect_service_event(server_events) {
        FakeServerEvent::Notified { handle, value, peers, .. } => {
            assert!(peers.contains(&PeerId(1)));
            assert_eq!(Handle(1), handle);
            value
        }
        x => panic!("Expected acknowledge of write, got {x:?}"),
    }
}

#[track_caller]
fn find_ase_id(
    ascs_server: &mut Pin<Box<AudioStreamControlServiceServer<FakeTypes>>>,
    fake_server: &FakeServer,
    server_events: &mut UnboundedReceiver<FakeServerEvent>,
    handle: Handle,
) -> u8 {
    // Read the sink uuid
    fake_server.incoming_read(PeerId(1), ASCS_SERVICE_ID, handle, 0);
    // Poll the ascs server, should not result in an ASCS event
    assert!(poll_server(ascs_server).is_pending());

    // Find the AseId
    let sink_value;
    match server_events.poll_next_unpin(&mut futures_test::task::noop_context()) {
        Poll::Ready(Some(FakeServerEvent::ReadResponded { service_id, handle: _, value })) => {
            assert_eq!(service_id, ASCS_SERVICE_ID);
            sink_value = value.unwrap();
        }
        x => panic!("Expected the read to be responded to got {x:?}"),
    };

    let ase_id = sink_value[0];
    // Should start in idle.
    assert_eq!(sink_value[1], 0x00);
    assert_eq!(sink_value.len(), 2);

    ase_id
}

#[track_caller]
fn find_first_ase_id(
    ascs_server: &mut Pin<Box<AudioStreamControlServiceServer<FakeTypes>>>,
    fake_server: &FakeServer,
    server_events: &mut UnboundedReceiver<FakeServerEvent>,
) -> u8 {
    find_ase_id(ascs_server, fake_server, server_events, Handle(3))
}

#[test]
fn invalid_operation() {
    let (mut ascs_server, fake_server, mut server_events) = published_server();

    let ase_id = find_first_ase_id(&mut ascs_server, &fake_server, &mut server_events);

    // Write an operation that is unknown.
    fake_server.incoming_write(
        PeerId(1),
        ASCS_SERVICE_ID,
        Handle(1),
        0,
        vec![0x1f, 0x01, ase_id, 0xC0, 0xDE],
    );

    // Should have nothing to respond to
    match poll_server(&mut ascs_server) {
        Poll::Pending => {}
        x => panic!("Expected to still be pending, got {x:?}"),
    };

    let cp_value = expect_control_point_notified(&mut ascs_server, &mut server_events);
    // Opcode should match
    // Number_of_ASEs should be 0xFF (Table 4.7)
    // ASE_ID is 0x00, and Reason should be 0x00
    assert_eq!(cp_value, &[0x1f, 0xFF, 0x00, 0x01, 0x00]);
}

#[test]
fn invalid_length() {
    let (mut ascs_server, fake_server, mut server_events) = published_server();

    let ase_id = find_first_ase_id(&mut ascs_server, &fake_server, &mut server_events);

    // Write an operation that is the wrong length (too short)
    fake_server.incoming_write(
        PeerId(1),
        ASCS_SERVICE_ID,
        Handle(1),
        0,
        vec![0x01, 0x01, ase_id, 0xC0, 0xDE],
    );

    // Should have nothing to respond to
    match poll_server(&mut ascs_server) {
        Poll::Pending => {}
        x => panic!("Expected to still be pending, got {x:?}"),
    };

    let cp_value = expect_control_point_notified(&mut ascs_server, &mut server_events);
    // Opcode should match
    // Number_of_ASEs should be 0xFF (Table 4.7)
    // ASE_ID is 0x00, and Reason should be 0x00
    assert_eq!(cp_value, &[0x01, 0xFF, 0x00, 0x02, 0x00]);
}

#[track_caller]
fn assert_handle_value_eq(
    ascs_server: &mut Pin<Box<AudioStreamControlServiceServer<FakeTypes>>>,
    fake_server: &FakeServer,
    events: &mut UnboundedReceiver<FakeServerEvent>,
    handle: Handle,
    expected: &[u8],
) {
    // Read the sink uuid
    fake_server.incoming_read(PeerId(1), ASCS_SERVICE_ID, handle, 0);
    // Poll the ascs server, should not result in an ASCS event
    assert!(poll_server(ascs_server).is_pending());
    let sink_value;
    match events.poll_next_unpin(&mut futures_test::task::noop_context()) {
        Poll::Ready(Some(FakeServerEvent::ReadResponded { service_id, handle: _, value })) => {
            assert_eq!(service_id, ASCS_SERVICE_ID);
            sink_value = value.unwrap();
        }
        x => panic!("Expected the read to be responded to got {x:?}"),
    };

    assert_eq!(sink_value, expected);
}

#[rustfmt::skip]
fn build_codec_configure(ase_id: u8) -> Vec<u8> {
    // Balanced Latency, 1m phy, LC3 codec (no company / company codecid): ltv (xx
    // bytes), 48khz, frame_duration 10ms, stereo audio channels,
    vec![
        0x01, 0x01, ase_id, 0x02, 0x01, 0x06, 0x00, 0x00, 0x00, 0x00,
        0x10, // 16 bytes
        0x02, 0x01, 0x08, // 48khz
        0x02, 0x02, 0x01, // 10ms duration
        0x05, 0x03, 0x00, 0x00, 0x00, 0x03, // FrontLeft + FrontRight
        0x03, 0x04, 0x64, 0x00, // 100 bytes per codec frame
    ]
}

#[test]
fn invalid_ase_ids() {
    let (mut ascs_server, fake_server, mut server_events) = published_server();

    let ase_id = find_first_ase_id(&mut ascs_server, &fake_server, &mut server_events);

    // Try with an invalid ase_id
    let codec_configure_bad_ase_id = build_codec_configure(0x00);

    fake_server.incoming_write(
        PeerId(1),
        ASCS_SERVICE_ID,
        Handle(1),
        0,
        codec_configure_bad_ase_id.clone(),
    );

    // Should have nothing to respond to
    match poll_server(&mut ascs_server) {
        Poll::Pending => {}
        x => panic!("Expected to still be pending, got {x:?}"),
    };

    let cp_value = expect_control_point_notified(&mut ascs_server, &mut server_events);
    // Opcode should match (0x01)
    // Number_of_ASEs should be 1
    // ASE_ID is 0x00, and Reason should be 0x03 (invalid ASE ID)
    assert_eq!(cp_value, &[0x01, 0x01, 0x00, 0x03, 0x00]);

    // Try with one valid and one invalid
    let mut codec_configure_two_ase_ids_one_bad = build_codec_configure(ase_id);
    // Adjust the number of ASEs
    codec_configure_two_ase_ids_one_bad[1] = 0x02;
    // This one has a bad ase_id in it
    // TODO(b/518022833): adjust to find all ASE_IDs and randomly pick a non-used
    // one)
    let bad_ase_id_configure = build_codec_configure(ase_id | 0xF0);
    // Skip the opcode and num of ase_ids in this one
    codec_configure_two_ase_ids_one_bad.extend(&bad_ase_id_configure[2..]);

    fake_server.incoming_write(
        PeerId(1),
        ASCS_SERVICE_ID,
        Handle(1),
        0,
        codec_configure_two_ase_ids_one_bad.clone(),
    );

    // Should have the one valid CodecConfigure to repond to
    match poll_server(&mut ascs_server) {
        Poll::Ready(Some(Ok(ServiceEvent::CodecConfigure {
            configuration:
                CodecConfiguration {
                    target_latency,
                    target_phy,
                    codec_id,
                    codec_specific_configuration,
                    ase_id: configured_ase_id,
                },
            responder,
        }))) => {
            use crate::types::*;
            use bt_common::core::{CodecId, CodingFormat};
            assert_eq!(AseId(ase_id), configured_ase_id);
            assert_eq!(target_latency, TargetLatency::TargetBalanced);
            assert_eq!(target_phy, TargetPhy::Le1MPhy);
            assert_eq!(codec_id, CodecId::Assigned(CodingFormat::Lc3));
            assert_eq!(codec_specific_configuration.len(), 16);
            responder.accept(
                Framing::Unframed,
                vec![Phy::Le1MPhy],
                0x10,
                MaxTransportLatency::try_from(std::time::Duration::from_millis(40)).unwrap(),
                PresentationDelayRange::build(20000, 100000).unwrap(),
            );
        }
        x => panic!("Expected CodecConfigure, got {x:?}"),
    }
    // Then we should be pending.
    match poll_server(&mut ascs_server) {
        Poll::Pending => {}
        x => panic!("Expected to still be pending, got {x:?}"),
    };

    let cp_value = expect_control_point_notified(&mut ascs_server, &mut server_events);
    // Opcode should match
    // Number_of_ASEs should be 2
    // ASE_ID should match and succeed, and the other ase_id should have failed.
    assert_eq!(cp_value, &[0x01, 0x02, ase_id | 0xF0, 0x03, 0x00, ase_id, 0x00, 0x00]);
}

#[test]
fn application_rejection() {
    let (mut ascs_server, fake_server, mut server_events) = published_server();

    let ase_id = find_first_ase_id(&mut ascs_server, &fake_server, &mut server_events);

    // Codec configure
    let codec_configure = build_codec_configure(ase_id);
    fake_server.incoming_write(PeerId(1), ASCS_SERVICE_ID, Handle(1), 0, codec_configure.clone());

    // Should have a CodecConfigure to respond to. Let's reject.
    match poll_server(&mut ascs_server) {
        Poll::Ready(Some(Ok(ServiceEvent::CodecConfigure {
            configuration:
                CodecConfiguration {
                    target_latency,
                    target_phy,
                    codec_id,
                    codec_specific_configuration,
                    ase_id,
                },
            responder,
        }))) => {
            use crate::types::*;
            use bt_common::core::{CodecId, CodingFormat};
            assert_eq!(target_latency, TargetLatency::TargetBalanced);
            assert_eq!(target_phy, TargetPhy::Le1MPhy);
            assert_eq!(codec_id, CodecId::Assigned(CodingFormat::Lc3));
            assert_eq!(codec_specific_configuration.len(), 16);
            responder.reject(ResponseCode::InsufficientResources { ase_id });
        }

        x => panic!("Expected CodecConfigure, got {x:?}"),
    };

    let cp_value = expect_control_point_notified(&mut ascs_server, &mut server_events);
    // Opcode should match
    // Number_of_ASEs should be 1 should match ASE_ID and response should match.
    assert_eq!(cp_value, &[0x01, 0x01, ase_id, 0x0D, 0x00]);
}

#[test]
fn endpoint_lifecycle_source() {
    let (mut ascs_server, fake_server, mut server_events) = published_server();

    let ase_id = find_first_ase_id(&mut ascs_server, &fake_server, &mut server_events);

    // QosConfigure the ase_id, with CIG 0x01, CIS 0x01, SDU Interval 0x1FF,
    // Unframed, 1m PHY, Max SDU 0x0FFF, RetransmissionNumber 0xFF,
    // Max_Transport_latency max (0x0FA0), and 30ms (0x007530)
    let qos_configure = vec![
        0x02, 0x01, ase_id, 0x01, 0x01, 0x00, 0xFF, 0x01, 0x00, 0x01, 0xFF, 0x0F, 0xFF, 0xA0, 0x0F,
        0x30, 0x75, 0x00,
    ];
    // Can't transition to QoS without CodecConfigure first
    fake_server.incoming_write(PeerId(1), ASCS_SERVICE_ID, Handle(1), 0, qos_configure.clone());

    // Should have nothing to respond to
    match poll_server(&mut ascs_server) {
        Poll::Pending => {}
        x => panic!("Expected to still be pending, got {x:?}"),
    };

    let cp_value = expect_control_point_notified(&mut ascs_server, &mut server_events);
    // Opcode should match
    // Number_of_ASEs should be 1
    // ASE_ID is 0x00, and Reason should be 0x04 (invalid state machine transition)
    assert_eq!(cp_value, &[0x02, 0x01, ase_id, 0x04, 0x00]);

    // And the ASE should still be in idle.
    assert_handle_value_eq(
        &mut ascs_server,
        &fake_server,
        &mut server_events,
        Handle(3),
        &[ase_id, 0x00],
    );

    // Go to CodecConfigure
    let codec_configure = build_codec_configure(ase_id);

    fake_server.incoming_write(PeerId(1), ASCS_SERVICE_ID, Handle(1), 0, codec_configure.clone());

    // Should have a CodecConfigure to respond to.
    match poll_server(&mut ascs_server) {
        Poll::Ready(Some(Ok(ServiceEvent::CodecConfigure {
            configuration:
                CodecConfiguration {
                    target_latency,
                    target_phy,
                    codec_id,
                    codec_specific_configuration,
                    ..
                },
            responder,
        }))) => {
            use crate::types::*;
            use bt_common::core::{CodecId, CodingFormat};
            assert_eq!(target_latency, TargetLatency::TargetBalanced);
            assert_eq!(target_phy, TargetPhy::Le1MPhy);
            assert_eq!(codec_id, CodecId::Assigned(CodingFormat::Lc3));
            assert_eq!(codec_specific_configuration.len(), 16);
            responder.accept(
                Framing::Unframed,
                vec![Phy::Le1MPhy],
                0x10,
                MaxTransportLatency::try_from(std::time::Duration::from_millis(40)).unwrap(),
                PresentationDelayRange::build(20000, 100000).unwrap(),
            );
        }

        x => panic!("Expected CodecConfigure, got {x:?}"),
    };

    let cp_value = expect_control_point_notified(&mut ascs_server, &mut server_events);
    // Opcode should match
    // Number_of_ASEs should be 1 should match ASE_ID and success 0x00
    assert_eq!(cp_value, &[0x01, 0x01, ase_id, 0x00, 0x00]);

    // And the ASE should be configured and notified
    match expect_service_event(&mut server_events) {
        FakeServerEvent::Notified { handle, value, peers, .. } => {
            assert!(peers.contains(&PeerId(1)));
            assert_eq!(Handle(3), handle);
            assert_eq!(value[0], ase_id);
            assert_eq!(value[1], 0x01); // Configured state
            assert_eq!(value.len(), 2 + 23 + usize::from(value[24]))
            // state length should be 2 (static) + 24
            // (configured state) + len of codec
            // configuration
        }
        x => panic!("Expected Endpoint Notification, got {x:?}"),
    }

    // QoSConfigure
    fake_server.incoming_write(PeerId(1), ASCS_SERVICE_ID, Handle(1), 0, qos_configure.clone());

    // Should have a QosConfigure to respond to.
    match poll_server(&mut ascs_server) {
        Poll::Ready(Some(Ok(ServiceEvent::QosConfigure {
            peer_id,
            target_configuration: _,
            responder,
        }))) => {
            assert_eq!(peer_id, PeerId(1));
            responder.accept();
        }

        x => panic!("Expected QoSConfigure, got {x:?}"),
    };

    let cp_value = expect_control_point_notified(&mut ascs_server, &mut server_events);
    // Opcode should match
    // Number_of_ASEs should be 1 should match ASE_ID and success 0x00
    assert_eq!(cp_value, &[0x02, 0x01, ase_id, 0x00, 0x00]);

    // And the ASE should be configured and notified
    match expect_service_event(&mut server_events) {
        FakeServerEvent::Notified { handle, value, peers, .. } => {
            assert!(peers.contains(&PeerId(1)));
            assert_eq!(Handle(3), handle);
            assert_eq!(value[0], ase_id);
            assert_eq!(value[1], 0x02); // QoS Configured
        }
        x => panic!("Expected Endpoint Notification, got {x:?}"),
    }

    // Enable
    let enable = vec![0x03, 0x01, ase_id, 0x00];
    fake_server.incoming_write(PeerId(1), ASCS_SERVICE_ID, Handle(1), 0, enable.clone());

    // Should have an event o respond to.
    match poll_server(&mut ascs_server) {
        Poll::Ready(Some(Ok(ServiceEvent::Enable {
            ase_id: enabled_ase_id, responder, ..
        }))) => {
            assert_eq!(ase_id, enabled_ase_id.0);
            responder.accept();
        }

        x => panic!("Expected Enable, got {x:?}"),
    };

    let cp_value = expect_control_point_notified(&mut ascs_server, &mut server_events);
    // Opcode should match
    // Number_of_ASEs should be 1 should match ASE_ID and success 0x00
    assert_eq!(cp_value, &[0x03, 0x01, ase_id, 0x00, 0x00]);

    // And the ASE should be configured and notified
    match expect_service_event(&mut server_events) {
        FakeServerEvent::Notified { handle, value, peers, .. } => {
            assert!(peers.contains(&PeerId(1)));
            assert_eq!(Handle(3), handle);
            assert_eq!(value[0], ase_id);
            assert_eq!(value[1], 0x03); // Enabling
        }
        x => panic!("Expected Endpoint Notification, got {x:?}"),
    }

    // Receiver Start Ready -> Streaming
    let receiver_start_ready = vec![0x04, 0x01, ase_id];
    fake_server.incoming_write(
        PeerId(1),
        ASCS_SERVICE_ID,
        Handle(1),
        0,
        receiver_start_ready.clone(),
    );

    // We auto-accept this, so we should not have a response, but do generate an
    // event.
    match poll_server(&mut ascs_server) {
        Poll::Ready(Some(Ok(ServiceEvent::Start {
            peer_id: _,
            ase_id: started_ase_id,
            cis: _,
        }))) => {
            assert_eq!(ase_id, started_ase_id.0);
        }
        x => panic!("Expected Start, got {x:?}"),
    };

    let cp_value = expect_control_point_notified(&mut ascs_server, &mut server_events);
    // Opcode should match
    // Number_of_ASEs should be 1 should match ASE_ID and success 0x00
    assert_eq!(cp_value, &[0x04, 0x01, ase_id, 0x00, 0x00]);

    // And the ASE should be configured and notified
    match expect_service_event(&mut server_events) {
        FakeServerEvent::Notified { handle, value, peers, .. } => {
            assert!(peers.contains(&PeerId(1)));
            assert_eq!(Handle(3), handle);
            assert_eq!(value[0], ase_id);
            assert_eq!(value[1], 0x04); // Streaming
        }
        x => panic!("Expected Endpoint Notification, got {x:?}"),
    }

    // Disable -> Disabling
    let disable = vec![0x05, 0x01, ase_id];
    fake_server.incoming_write(PeerId(1), ASCS_SERVICE_ID, Handle(1), 0, disable.clone());

    // We auto-accept this, so we should not have a event.
    // We also don't generate an event until Receiver Stop Ready
    match poll_server(&mut ascs_server) {
        Poll::Pending => {}
        x => panic!("Expected no event on Disable, got {x:?}"),
    };

    let cp_value = expect_control_point_notified(&mut ascs_server, &mut server_events);
    // Opcode should match
    // Number_of_ASEs should be 1 should match ASE_ID and success 0x00
    assert_eq!(cp_value, &[0x05, 0x01, ase_id, 0x00, 0x00]);

    // And the ASE should be configured and notified
    match expect_service_event(&mut server_events) {
        FakeServerEvent::Notified { handle, value, peers, .. } => {
            assert!(peers.contains(&PeerId(1)));
            assert_eq!(Handle(3), handle);
            assert_eq!(value[0], ase_id);
            assert_eq!(value[1], 0x05); // Disabling
        }
        x => panic!("Expected Endpoint Notification, got {x:?}"),
    }

    // Receiver Stop Ready -> QoSConfigured
    let receiver_stop_ready = vec![0x06, 0x01, ase_id];
    fake_server.incoming_write(
        PeerId(1),
        ASCS_SERVICE_ID,
        Handle(1),
        0,
        receiver_stop_ready.clone(),
    );

    match poll_server(&mut ascs_server) {
        Poll::Ready(Some(Ok(ServiceEvent::Disable {
            peer_id: _,
            ase_id: disabled_ase_id,
            cis: _,
            responder,
        }))) => {
            assert_eq!(ase_id, disabled_ase_id.0);
            responder.accept();
        }
        x => panic!("Expected Disable, got {x:?}"),
    };

    let cp_value = expect_control_point_notified(&mut ascs_server, &mut server_events);
    // Opcode should match
    // Number_of_ASEs should be 1 should match ASE_ID and success 0x00
    assert_eq!(cp_value, &[0x06, 0x01, ase_id, 0x00, 0x00]);

    // And the ASE should be configured and notified
    match expect_service_event(&mut server_events) {
        FakeServerEvent::Notified { handle, value, peers, .. } => {
            assert!(peers.contains(&PeerId(1)));
            assert_eq!(Handle(3), handle);
            assert_eq!(value[0], ase_id);
            assert_eq!(value[1], 0x02); // QosConfigured
        }
        x => panic!("Expected Endpoint Notification, got {x:?}"),
    }

    // Release -> Releasing
    let release = vec![0x08, 0x01, ase_id];
    fake_server.incoming_write(PeerId(1), ASCS_SERVICE_ID, Handle(1), 0, release.clone());

    // We don't expect an event for this (disable has already happened
    match poll_server(&mut ascs_server) {
        Poll::Pending => {}
        x => panic!("Expected no event from release, got {x:?}"),
    };

    let cp_value = expect_control_point_notified(&mut ascs_server, &mut server_events);
    // Opcode should match
    // Number_of_ASEs should be 1 should match ASE_ID and success 0x00
    assert_eq!(cp_value, &[0x08, 0x01, ase_id, 0x00, 0x00]);

    // And the ASE should be configured and notified
    match expect_service_event(&mut server_events) {
        FakeServerEvent::Notified { handle, value, peers, .. } => {
            assert!(peers.contains(&PeerId(1)));
            assert_eq!(Handle(3), handle);
            assert_eq!(value[0], ase_id);
            assert_eq!(value[1], 0x06); // Releasing
        }
        x => panic!("Expected Endpoint Notification, got {x:?}"),
    }

    // TODO(b/518022833): Automated behaviors are not done yet
    // TODO(b/518022833): Test Update Metadata
}

#[test]
fn endpoint_lifecycle_sink() {
    let (mut ascs_server, fake_server, mut server_events) = published_server();

    // Handle 3 is the Sink ASE
    let ase_id = find_ase_id(&mut ascs_server, &fake_server, &mut server_events, Handle(4));

    // QosConfigure the ase_id, with CIG 0x01, CIS 0x01, SDU Interval 0x1FF,
    // Unframed, 1m PHY, Max SDU 0x0FFF, RetransmissionNumber 0xFF,
    // Max_Transport_latency max (0x0FA0), and 30ms (0x007530)
    let qos_configure = vec![
        0x02, 0x01, ase_id, 0x01, 0x01, 0x00, 0xFF, 0x01, 0x00, 0x01, 0xFF, 0x0F, 0xFF, 0xA0, 0x0F,
        0x30, 0x75, 0x00,
    ];
    // Can't transition to QoS without CodecConfigure first
    fake_server.incoming_write(PeerId(1), ASCS_SERVICE_ID, Handle(1), 0, qos_configure.clone());

    // Should have nothing to respond to
    match poll_server(&mut ascs_server) {
        Poll::Pending => {}
        x => panic!("Expected to still be pending, got {x:?}"),
    };

    let cp_value = expect_control_point_notified(&mut ascs_server, &mut server_events);
    // Opcode should match
    // Number_of_ASEs should be 1
    // ASE_ID is 0x00, and Reason should be 0x04 (invalid state machine transition)
    assert_eq!(cp_value, &[0x02, 0x01, ase_id, 0x04, 0x00]);

    // And the ASE should still be in idle.
    assert_handle_value_eq(
        &mut ascs_server,
        &fake_server,
        &mut server_events,
        Handle(4),
        &[ase_id, 0x00],
    );

    // Go to CodecConfigure
    let codec_configure = build_codec_configure(ase_id);

    fake_server.incoming_write(PeerId(1), ASCS_SERVICE_ID, Handle(1), 0, codec_configure.clone());

    // Should have a CodecConfigure to respond to.
    match poll_server(&mut ascs_server) {
        Poll::Ready(Some(Ok(ServiceEvent::CodecConfigure {
            configuration:
                CodecConfiguration {
                    target_latency,
                    target_phy,
                    codec_id,
                    codec_specific_configuration,
                    ..
                },
            responder,
        }))) => {
            use crate::types::*;
            use bt_common::core::{CodecId, CodingFormat};
            assert_eq!(target_latency, TargetLatency::TargetBalanced);
            assert_eq!(target_phy, TargetPhy::Le1MPhy);
            assert_eq!(codec_id, CodecId::Assigned(CodingFormat::Lc3));
            assert_eq!(codec_specific_configuration.len(), 16);
            responder.accept(
                Framing::Unframed,
                vec![Phy::Le1MPhy],
                0x10,
                MaxTransportLatency::try_from(std::time::Duration::from_millis(40)).unwrap(),
                PresentationDelayRange::build(20000, 100000).unwrap(),
            );
        }

        x => panic!("Expected CodecConfigure, got {x:?}"),
    };

    let cp_value = expect_control_point_notified(&mut ascs_server, &mut server_events);
    // Opcode should match
    // Number_of_ASEs should be 1 should match ASE_ID and success 0x00
    assert_eq!(cp_value, &[0x01, 0x01, ase_id, 0x00, 0x00]);

    // And the ASE should be configured and notified
    match expect_service_event(&mut server_events) {
        FakeServerEvent::Notified { handle, value, peers, .. } => {
            assert!(peers.contains(&PeerId(1)));
            assert_eq!(Handle(4), handle);
            assert_eq!(value[0], ase_id);
            assert_eq!(value[1], 0x01); // Configured state
            assert_eq!(value.len(), 2 + 23 + usize::from(value[24]))
            // state length should be 2 (static) + 24
            // (configured state) + len of codec
            // configuration
        }
        x => panic!("Expected Endpoint Notification, got {x:?}"),
    }

    // QoSConfigure
    fake_server.incoming_write(PeerId(1), ASCS_SERVICE_ID, Handle(1), 0, qos_configure.clone());

    // Should have a QosConfigure to respond to.
    match poll_server(&mut ascs_server) {
        Poll::Ready(Some(Ok(ServiceEvent::QosConfigure {
            peer_id,
            target_configuration: _,
            responder,
        }))) => {
            assert_eq!(peer_id, PeerId(1));
            responder.accept();
        }

        x => panic!("Expected QoSConfigure, got {x:?}"),
    };

    let cp_value = expect_control_point_notified(&mut ascs_server, &mut server_events);
    // Opcode should match
    // Number_of_ASEs should be 1 should match ASE_ID and success 0x00
    assert_eq!(cp_value, &[0x02, 0x01, ase_id, 0x00, 0x00]);

    // And the ASE should be configured and notified
    match expect_service_event(&mut server_events) {
        FakeServerEvent::Notified { handle, value, peers, .. } => {
            assert!(peers.contains(&PeerId(1)));
            assert_eq!(Handle(4), handle);
            assert_eq!(value[0], ase_id);
            assert_eq!(value[1], 0x02); // QoS Configured
        }
        x => panic!("Expected Endpoint Notification, got {x:?}"),
    }

    // Enable
    let enable = vec![0x03, 0x01, ase_id, 0x00];
    fake_server.incoming_write(PeerId(1), ASCS_SERVICE_ID, Handle(1), 0, enable.clone());

    // Should have an event to respond to.
    match poll_server(&mut ascs_server) {
        Poll::Ready(Some(Ok(ServiceEvent::Enable {
            ase_id: enabled_ase_id, responder, ..
        }))) => {
            assert_eq!(ase_id, enabled_ase_id.0);
            responder.accept();
        }

        x => panic!("Expected Enable, got {x:?}"),
    };

    let cp_value = expect_control_point_notified(&mut ascs_server, &mut server_events);
    // Opcode should match
    // Number_of_ASEs should be 1 should match ASE_ID and success 0x00
    assert_eq!(cp_value, &[0x03, 0x01, ase_id, 0x00, 0x00]);

    // And the ASE should be configured and notified
    match expect_service_event(&mut server_events) {
        FakeServerEvent::Notified { handle, value, peers, .. } => {
            assert!(peers.contains(&PeerId(1)));
            assert_eq!(Handle(4), handle);
            assert_eq!(value[0], ase_id);
            assert_eq!(value[1], 0x03); // Enabling
        }
        x => panic!("Expected Endpoint Notification, got {x:?}"),
    }

    // Receiver Start Ready should not be allowed from the client for a Sink ASE
    let receiver_start_ready = vec![0x04, 0x01, ase_id];
    fake_server.incoming_write(
        PeerId(1),
        ASCS_SERVICE_ID,
        Handle(1),
        0,
        receiver_start_ready.clone(),
    );

    // Should not have an event for the error.
    match poll_server(&mut ascs_server) {
        Poll::Pending => {}
        x => panic!("Expected no event for invalid ReceiverStartReady, got {x:?}"),
    };

    let cp_value = expect_control_point_notified(&mut ascs_server, &mut server_events);
    // Opcode should match
    // Number_of_ASEs should be 1 should match ASE_ID and success 0x00
    assert_eq!(cp_value, &[0x04, 0x01, ase_id, 0x05, 0x00]);

    // TODO: We don't do server-initiated ReceiverStart yet, so we can't proceed to
    // streaming.

    // Disable -> QosConfigured
    let disable = vec![0x05, 0x01, ase_id];
    fake_server.incoming_write(PeerId(1), ASCS_SERVICE_ID, Handle(1), 0, disable.clone());

    // On Sink ASE, we generate the Disable event when Disabling.
    match poll_server(&mut ascs_server) {
        Poll::Ready(Some(Ok(ServiceEvent::Disable {
            peer_id: _,
            ase_id: disabled_ase_id,
            cis: _,
            responder,
        }))) => {
            assert_eq!(ase_id, disabled_ase_id.0);
            responder.accept();
        }
        x => panic!("Expected Disable, got {x:?}"),
    };

    let cp_value = expect_control_point_notified(&mut ascs_server, &mut server_events);
    // Opcode should match
    // Number_of_ASEs should be 1 should match ASE_ID and success 0x00
    assert_eq!(cp_value, &[0x05, 0x01, ase_id, 0x00, 0x00]);

    // And the ASE should be configured and notified
    match expect_service_event(&mut server_events) {
        FakeServerEvent::Notified { handle, value, peers, .. } => {
            assert!(peers.contains(&PeerId(1)));
            assert_eq!(Handle(4), handle);
            assert_eq!(value[0], ase_id);
            assert_eq!(value[1], 0x02); // Automatically went to QoSConfigure
        }
        x => panic!("Expected Endpoint Notification, got {x:?}"),
    }

    // Receiver Stop Ready is not allowed for a Sink ASE
    let receiver_stop_ready = vec![0x06, 0x01, ase_id];
    fake_server.incoming_write(
        PeerId(1),
        ASCS_SERVICE_ID,
        Handle(1),
        0,
        receiver_stop_ready.clone(),
    );

    assert!(poll_server(&mut ascs_server).is_pending());
    let cp_value = expect_control_point_notified(&mut ascs_server, &mut server_events);
    // Opcode should match
    // Number_of_ASEs should be 1 should match ASE_ID and Failure for Invalid ASE
    // State Transition (since we are already in QosCondfiguredj
    assert_eq!(cp_value, &[0x06, 0x01, ase_id, 0x04, 0x00]);

    // Release -> Releasing
    let release = vec![0x08, 0x01, ase_id];
    fake_server.incoming_write(PeerId(1), ASCS_SERVICE_ID, Handle(1), 0, release.clone());

    // We don't expect an event for this (disable has already happened
    match poll_server(&mut ascs_server) {
        Poll::Pending => {}
        x => panic!("Expected no event from release, got {x:?}"),
    };

    let cp_value = expect_control_point_notified(&mut ascs_server, &mut server_events);
    // Opcode should match
    // Number_of_ASEs should be 1 should match ASE_ID and success 0x00
    assert_eq!(cp_value, &[0x08, 0x01, ase_id, 0x00, 0x00]);

    // And the ASE should be configured and notified
    match expect_service_event(&mut server_events) {
        FakeServerEvent::Notified { handle, value, peers, .. } => {
            assert!(peers.contains(&PeerId(1)));
            assert_eq!(Handle(4), handle);
            assert_eq!(value[0], ase_id);
            assert_eq!(value[1], 0x06); // Releasing
        }
        x => panic!("Expected Endpoint Notification, got {x:?}"),
    }

    // TODO: Automated behaviors are not done yet
    // TODO: Test Update Metadata
}
