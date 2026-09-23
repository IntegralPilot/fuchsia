// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, format_err};
use bt_avctp::{AvcPeer, AvcResponseType};
use fidl::endpoints::{create_endpoints, create_proxy, create_request_stream};
use fidl_fuchsia_bluetooth as fidl_bt;
use fidl_fuchsia_bluetooth_avrcp::{
    AbsoluteVolumeHandlerMarker, AbsoluteVolumeHandlerRequest, AbsoluteVolumeHandlerRequestStream,
    ControllerMarker, PeerManagerMarker, TargetHandlerMarker,
};
use fidl_fuchsia_bluetooth_bredr as bredr;
use fidl_fuchsia_bluetooth_bredr_test as bredr_test;
use fixture::fixture;
use fuchsia_async as fasync;
use fuchsia_bluetooth::profile::{Psm, l2cap_connect_parameters};
use fuchsia_bluetooth::types::{Channel, Uuid};
use fuchsia_component_test::Capability;
use futures::stream::StreamExt;
use futures::{SinkExt, TryFutureExt, join};
use mock_piconet_client::{BtProfileComponent, PiconetHarness, PiconetMember};

/// AVRCP component URL.
const AVRCP_URL: &str = "fuchsia-pkg://fuchsia.com/bt-avrcp-integration-tests#meta/bt-avrcp.cm";
/// Attribute ID for SDP Support Features.
const SDP_SUPPORTED_FEATURES: u16 = 0x0311;
/// Name prefix for mock peers
const MOCK_PEER_NAME: &str = "mock-peer";

struct AvrcpIntegrationTest {
    avrcp_observer: BtProfileComponent,
    mock_peer: PiconetMember,
    test_realm: fuchsia_component_test::RealmInstance,
}

impl AvrcpIntegrationTest {
    async fn new() -> Result<Self, anyhow::Error> {
        let mut piconet = PiconetHarness::new().await;
        let spec = piconet.add_mock_piconet_member(MOCK_PEER_NAME.to_string(), None).await.unwrap();
        let observer = piconet
            .add_profile_with_capabilities(
                "bt-avrcp-profile".to_string(),
                AVRCP_URL.to_string(),
                None,
                vec![],
                vec![Capability::protocol::<PeerManagerMarker>().into()],
            )
            .await
            .unwrap();
        let test_realm = piconet.build().await.unwrap();
        let mock_peer = PiconetMember::new_from_spec(spec, &test_realm).unwrap();
        Ok(Self { avrcp_observer: observer, mock_peer: mock_peer, test_realm: test_realm })
    }
}

async fn setup_piconet<F, Fut>(_name: &str, test: F)
where
    F: FnOnce(AvrcpIntegrationTest) -> Fut,
    Fut: futures::Future<Output = ()>,
{
    let test_fixture = AvrcpIntegrationTest::new().await.unwrap();
    test(test_fixture).await
}

/// Make the SDP definition for an AVRCP Controller service.
fn avrcp_controller_service_definition() -> bredr::ServiceDefinition {
    bredr::ServiceDefinition {
        service_class_uuids: Some(vec![Uuid::new16(0x110E).into(), Uuid::new16(0x110F).into()]),
        protocol_descriptor_list: Some(vec![
            bredr::ProtocolDescriptor {
                protocol: Some(bredr::ProtocolIdentifier::L2Cap),
                params: Some(vec![bredr::DataElement::Uint16(bredr::PSM_AVCTP as u16)]),
                ..Default::default()
            },
            bredr::ProtocolDescriptor {
                protocol: Some(bredr::ProtocolIdentifier::Avctp),
                params: Some(vec![bredr::DataElement::Uint16(0x0103)]), // Indicate v1.3
                ..Default::default()
            },
        ]),
        profile_descriptors: Some(vec![bredr::ProfileDescriptor {
            profile_id: Some(bredr::ServiceClassProfileIdentifier::AvRemoteControl),
            major_version: Some(1),
            minor_version: Some(6),
            ..Default::default()
        }]),
        additional_attributes: Some(vec![bredr::Attribute {
            id: Some(SDP_SUPPORTED_FEATURES), // SDP Attribute "SUPPORTED FEATURES"
            element: Some(bredr::DataElement::Uint16(0x0003)), // CATEGORY 1 and 2.
            ..Default::default()
        }]),
        ..Default::default()
    }
}

/// Stub absolute volume handler which accepts any requested volume
async fn run_absolute_volume_handler(stream: AbsoluteVolumeHandlerRequestStream) {
    stream
        .for_each(|request| async move {
            match request {
                Ok(AbsoluteVolumeHandlerRequest::SetVolume { requested_volume, responder }) => {
                    responder
                        .send(requested_volume)
                        .expect("Error forwarding absolute volume request");
                }
                _ => {}
            }
        })
        .await;
}

/// Tests that AVRCP correctly advertises it's TG-related services and can be
/// discovered by another peer in the mock piconet.
#[fixture(setup_piconet)]
#[fuchsia::test]
async fn avrcp_target_service_advertisement(tf: AvrcpIntegrationTest) {
    let _ = &tf;
    let mut results_requests = tf
        .mock_peer
        .register_service_search(
            bredr::ServiceClassProfileIdentifier::AvRemoteControlTarget,
            vec![],
        )
        .unwrap();
    let service_found_fut = results_requests.select_next_some().map_err(|e| format_err!("{:?}", e));
    let bredr::SearchResultsRequest::ServiceFound { peer_id, responder, .. } =
        service_found_fut.await.unwrap()
    else {
        panic!("unknown method")
    };
    assert_eq!(tf.avrcp_observer.peer_id(), peer_id.into());
    responder.send().unwrap();
}

/// Tests that AVRCP correctly advertises it's CT-related services and can be
/// discovered by another peer in the mock piconet.
#[fixture(setup_piconet)]
#[fuchsia::test]
async fn avrcp_controller_service_advertisement(tf: AvrcpIntegrationTest) {
    let _ = &tf;
    let mut results_requests = tf
        .mock_peer
        .register_service_search(bredr::ServiceClassProfileIdentifier::AvRemoteControl, vec![])
        .unwrap();
    let service_found_fut = results_requests.select_next_some().map_err(|e| format_err!("{:?}", e));
    let bredr::SearchResultsRequest::ServiceFound { peer_id, responder, .. } =
        service_found_fut.await.unwrap()
    else {
        panic!("unknown method")
    };
    assert_eq!(tf.avrcp_observer.peer_id(), peer_id.into());
    responder.send().unwrap();
}

/// Tests that AVRCP correctly searches for AvRemoteControls, discovers a mock peer
/// providing it, and attempts to connect to the mock peer.
#[fixture(setup_piconet)]
#[fuchsia::test]
async fn avrcp_search_and_connect(mut tf: AvrcpIntegrationTest) {
    let _ = &tf;
    // Mock peer advertises an AVRCP CT service.
    let service_defs = vec![avrcp_controller_service_definition()];
    let mut connect_requests = tf
        .mock_peer
        .register_service_advertisement(service_defs)
        .expect("Mock peer failed to register service advertisement");

    // We then expect AVRCP to discover the mock peer and attempt to connect to it
    tf.avrcp_observer.expect_observer_service_found_request(tf.mock_peer.peer_id()).await.unwrap();

    // Connect to bt-avrcp.cm's controller service to request a connection.
    let avrcp_svc = tf
        .avrcp_observer
        .connect_to_protocol::<PeerManagerMarker>(&tf.test_realm)
        .context("Failed to connect to Bluetooth AVRCP interface")
        .unwrap();
    // Get controller for mock peer to initiate a connection.
    let (_c_client, c_server) = create_endpoints::<ControllerMarker>();
    let _ = avrcp_svc
        .get_controller_for_target(&tf.mock_peer.peer_id().into(), c_server)
        .await
        .unwrap();

    let _channel = match connect_requests.select_next_some().await.unwrap() {
        bredr::ConnectionReceiverRequest::Connected { peer_id, channel, .. } => {
            assert_eq!(tf.avrcp_observer.peer_id(), peer_id.into());
            channel
        }
        bredr::ConnectionReceiverRequest::_UnknownMethod { .. } => {
            panic!("unknown method")
        }
    };
}

/// Tests the case of a remote peer initiating the AVCTP connection to AVRCP.
/// AVRCP should accept the connection (which should be relayed on the observer), and
/// sending/receiving data should be OK.
#[fixture(setup_piconet)]
#[fuchsia::test]
async fn remote_initiates_connection_to_avrcp(mut tf: AvrcpIntegrationTest) {
    let _ = &tf;
    let avrcp_profile_id = tf.avrcp_observer.peer_id();

    // Mock peer advertises an AVRCP CT service.
    let service_defs = vec![avrcp_controller_service_definition()];
    let _connect_requests = tf
        .mock_peer
        .register_service_advertisement(service_defs)
        .expect("Mock peer failed to register service advertisement");

    // We then expect AVRCP to discover the mock peer.
    tf.avrcp_observer.expect_observer_service_found_request(tf.mock_peer.peer_id()).await.unwrap();

    // Mock peer attempts to connect to AVRCP.
    let params = l2cap_connect_parameters(Psm::AVCTP, fidl_bt::ChannelMode::Basic);
    let channel = tf.mock_peer.make_connection(avrcp_profile_id, params).await.unwrap();
    let mut channel: Channel = channel.try_into().unwrap();

    // The observer of bt-avrcp.cm should be notified of the connection attempt.
    match tf.avrcp_observer.expect_observer_request().await.unwrap() {
        bredr_test::PeerObserverRequest::PeerConnected { peer_id, responder, .. } => {
            assert_eq!(tf.mock_peer.peer_id(), peer_id.into());
            responder.send().unwrap();
        }
        x => panic!("Expected PeerConnected but got: {:?}", x),
    }

    // Connect to bt-avrcp.cm's controller service to request a connection to the mock peer.
    let avrcp_svc = tf
        .avrcp_observer
        .connect_to_protocol::<PeerManagerMarker>(&tf.test_realm)
        .context("Failed to connect to Bluetooth AVRCP interface")
        .unwrap();
    let (_c_client, c_server) = create_endpoints::<ControllerMarker>();
    let _ = avrcp_svc
        .get_controller_for_target(&tf.mock_peer.peer_id().into(), c_server)
        .await
        .unwrap();

    // Sending a random non-AVRCP data packet should be OK.
    let write_result = channel.send(vec![0x00, 0x00, 0x00]).await;
    assert!(write_result.is_ok());

    // We expect a general reject response, since the sent packet is malformed.
    let expected_read_result_packet = &[
        0x03, // Bit 0 indicates invalid profile id, Bit 1 indicates CommandType::Response.
        0x00, // Profile ID
        0x00, // Profile ID
    ];
    let vec = channel.next().await.expect("still open").expect("packet");
    assert_eq!(vec.len(), 3);
    assert_eq!(vec.as_slice(), expected_read_result_packet);
}

/// Tests the case of a remote device attempting to connect over PSM_BROWSE
/// before PSM_AVCTP. AVRCP 1.6 states that the Browse channel may only be established
/// after the control channel. We expect AVRCP to drop the incoming channel.
#[fixture(setup_piconet)]
#[fuchsia::test]
async fn remote_initiates_browse_channel_before_control(mut tf: AvrcpIntegrationTest) {
    let _ = &tf;
    let avrcp_profile_id = tf.avrcp_observer.peer_id();

    // Mock peer advertises an AVRCP CT service.
    let service_defs = vec![avrcp_controller_service_definition()];
    let _connect_requests = tf
        .mock_peer
        .register_service_advertisement(service_defs)
        .expect("Mock peer failed to register service advertisement");

    // We then expect AVRCP to discover the mock peer.
    tf.avrcp_observer.expect_observer_service_found_request(tf.mock_peer.peer_id()).await.unwrap();

    // Mock peer tries to initiate a browse channel connection.
    let params =
        l2cap_connect_parameters(Psm::AVCTP_BROWSE, fidl_bt::ChannelMode::EnhancedRetransmission);
    let channel = tf.mock_peer.make_connection(avrcp_profile_id, params).await.unwrap();
    let channel: Channel = channel.try_into().unwrap();

    // The observer of bt-avrcp.cm should be notified of the connection attempt.
    match tf.avrcp_observer.expect_observer_request().await.unwrap() {
        bredr_test::PeerObserverRequest::PeerConnected { peer_id, responder, .. } => {
            assert_eq!(tf.mock_peer.peer_id(), peer_id.into());
            responder.send().unwrap();
        }
        x => panic!("Expected PeerConnected but got: {:?}", x),
    }

    // However, AVRCP should immediately close the `channel` because the Browse channel
    // cannot be established before the Control channel.
    let closed = channel.closed().await;
    assert_eq!(closed, Ok(()));
}

/// Tests that AVRCP's absolute volume handler and target handler can each be set once and only once
#[fixture(setup_piconet)]
#[fuchsia::test]
async fn avrcp_disallows_handler_double_sets(mut tf: AvrcpIntegrationTest) {
    // Mock peer advertises service & bt-avrcp discovers it
    let service_defs = vec![avrcp_controller_service_definition()];
    let mut connect_requests = tf
        .mock_peer
        .register_service_advertisement(service_defs)
        .expect("Mock peer failed to register service advertisement");

    // We then expect AVRCP to discover the mock peer and attempt to connect to it
    tf.avrcp_observer.expect_observer_service_found_request(tf.mock_peer.peer_id()).await.unwrap();

    // Connect to bt-avrcp.cm's controller service to request a connection.
    let avrcp_svc = tf
        .avrcp_observer
        .connect_to_protocol::<PeerManagerMarker>(&tf.test_realm)
        .context("Failed to connect to Bluetooth AVRCP interface")
        .unwrap();
    // Get controller for mock peer to initiate a connection.
    let (_c_client, c_server) = create_endpoints::<ControllerMarker>();
    let _ = avrcp_svc
        .get_controller_for_target(&tf.mock_peer.peer_id().into(), c_server)
        .await
        .unwrap();

    let _channel = match connect_requests.select_next_some().await.unwrap() {
        bredr::ConnectionReceiverRequest::Connected { peer_id, channel, .. } => {
            assert_eq!(tf.avrcp_observer.peer_id(), peer_id.into());
            channel
        }
        bredr::ConnectionReceiverRequest::_UnknownMethod { .. } => {
            panic!("unknown method");
        }
    };

    // Connect to bt-avrcp.cm's controller service
    let avrcp_svc = tf
        .avrcp_observer
        .connect_to_protocol::<PeerManagerMarker>(&tf.test_realm)
        .context("Failed to connect to Bluetooth AVRCP interface")
        .unwrap();

    // Get controller for mock peer
    let (_c_client, c_server) = create_endpoints::<ControllerMarker>();
    let _ = avrcp_svc
        .get_controller_for_target(&tf.mock_peer.peer_id().into(), c_server)
        .await
        .unwrap();
    // Create absolute volume handler, register with AVRCP
    let (avh_client, avh_stream) = create_request_stream::<AbsoluteVolumeHandlerMarker>();
    let avh_fut = run_absolute_volume_handler(avh_stream);
    fasync::Task::spawn(avh_fut).detach();
    avrcp_svc
        .set_absolute_volume_handler(avh_client)
        .await
        .unwrap()
        .expect("AVRCP should accept first absolute volume handler");

    // Register with AVRCP again
    // Request should be rejected, so no handler needs to be instantiated
    let (avh_client, _avh_server) = create_endpoints::<AbsoluteVolumeHandlerMarker>();
    let _ = avrcp_svc
        .set_absolute_volume_handler(avh_client)
        .await
        .unwrap()
        .expect_err("AVRCP should reject absolute volume handler double sets");

    // Create target handler, register with AVRCP
    let (t_client, t_stream) = create_request_stream::<TargetHandlerMarker>();
    let t_fut = t_stream.for_each(|_request| async move {});
    fasync::Task::spawn(t_fut).detach();
    avrcp_svc
        .register_target_handler(t_client)
        .await
        .unwrap()
        .expect("AVRCP should accept first target handler");

    // Register with AVRCP again
    // Request should be rejected, so no handler needs to be instantiated
    let (t_client, _t_server) = create_endpoints::<TargetHandlerMarker>();
    let _ = avrcp_svc
        .register_target_handler(t_client)
        .await
        .unwrap()
        .expect_err("AVRCP should reject target handler double sets");
}

/// Confirm that peers can have their absolute volume set
/// through the registered absolute volume handler
#[fixture(setup_piconet)]
#[fuchsia::test]
async fn avrcp_remote_receives_set_absolute_volume_request(mut tf: AvrcpIntegrationTest) {
    // Mock peer advertises an AVRCP CT service.
    let service_defs = vec![avrcp_controller_service_definition()];
    let mut connect_requests = tf
        .mock_peer
        .register_service_advertisement(service_defs)
        .expect("Mock peer failed to register service advertisement");

    // We then expect AVRCP to discover the mock peer and attempt to connect to it
    tf.avrcp_observer.expect_observer_service_found_request(tf.mock_peer.peer_id()).await.unwrap();

    // Connect to avrcp controller service.
    let avrcp_svc = tf
        .avrcp_observer
        .connect_to_protocol::<PeerManagerMarker>(&tf.test_realm)
        .context("Failed to connect to Bluetooth AVRCP interface")
        .unwrap();

    // Get controller for mock peer to initiate a connection.
    let (_c_proxy, c_server) = create_proxy::<ControllerMarker>();
    avrcp_svc
        .get_controller_for_target(&tf.mock_peer.peer_id().into(), c_server)
        .await
        .unwrap()
        .expect("Failed to get controller for mock peer");

    let channel: Channel = match connect_requests.select_next_some().await.unwrap() {
        bredr::ConnectionReceiverRequest::Connected { peer_id, channel, .. } => {
            assert_eq!(tf.avrcp_observer.peer_id(), peer_id.into());
            channel.try_into().unwrap()
        }
        bredr::ConnectionReceiverRequest::_UnknownMethod { .. } => {
            panic!("unknown method")
        }
    };

    // Create absolute volume handler server which simply forwards the desired volume unmodified
    let (avh_client, avh_server) = create_request_stream::<AbsoluteVolumeHandlerMarker>();
    let avh_fut = run_absolute_volume_handler(avh_server);
    avrcp_svc
        .set_absolute_volume_handler(avh_client)
        .await
        .unwrap()
        .expect("Error registering AVH");
    let _avh_task = fasync::Task::spawn(avh_fut);

    // Get controller for mock peer
    let (c_proxy, c_server) = create_proxy::<ControllerMarker>();
    avrcp_svc
        .get_controller_for_target(&tf.mock_peer.peer_id().into(), c_server)
        .await
        .unwrap()
        .expect("Failed to get controller for mock peer");

    // Mock peer receives SetAbsoluteVolume request and responds with the actual volume which was set
    let desired_vol: u8 = 0x7F; // Valid volume range is 0x00 - 0x7F
    let actual_vol: u8 = 0x7E; // Volume actually set on the target
    const SET_ABSOLUTE_VOLUME_PDU: u8 = 0x050;
    let mut avc_command_stream = AvcPeer::new(channel).take_command_stream();
    async {
        let set_av_fut = c_proxy.set_absolute_volume(desired_vol);
        let receive_av_fut = async {
            match avc_command_stream.next().await.expect("Mock peer did not receive a command") {
                Ok(command) => {
                    // Command and response structure described in AVRCP spec 25.16
                    let body = command.body();
                    assert_eq!(body.len(), 5);
                    assert_eq!(body[0], SET_ABSOLUTE_VOLUME_PDU); // Confirm PDU is SetAbsoluteVolume
                    assert_eq!(body[4], desired_vol); // Confirm the requested volume was received unmodified

                    // Respond with the actual volume set on the target
                    command
                        .send_response(
                            AvcResponseType::Accepted,
                            &[0x50, 0x00, 0x00, 0x01, actual_vol],
                        )
                        .unwrap();
                }
                Err(e) => panic!("{}", e),
            }
        };
        match join!(set_av_fut, receive_av_fut) {
            (Ok(Ok(vol)), _) => assert_eq!(vol, actual_vol),
            (Ok(Err(e)), _) => {
                panic!("Received ControllerError from AVRCP: {}", e.into_primitive())
            }
            (Err(e), _) => panic!("SetAbsoluteVolume FIDL call failed: {}", e),
        }
    }
    .await;
}
