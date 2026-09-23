// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use fidl::prelude::*;
use fidl_fuchsia_bluetooth_avrcp::*;
use fidl_fuchsia_bluetooth_avrcp_test::*;
use fuchsia_async as fasync;
use fuchsia_async::TimeoutExt;
use fuchsia_bluetooth::types::PeerId;
use fuchsia_component::server::{ServiceFs, ServiceObj};
use futures::channel::mpsc;
use futures::future::{FutureExt, TryFutureExt};
use futures::stream::{StreamExt, TryStreamExt};
use log::{info, warn};
use zx::MonotonicDuration;

use crate::peer::Controller;
use crate::peer_manager::ServiceRequest;
use crate::types::PeerError;
use crate::{browse_controller_service, controller_service};

async fn get_controller_for_peer(
    peer_id: PeerId,
    sender: &mut mpsc::Sender<ServiceRequest>,
) -> Result<Controller, Error> {
    let (response, pcr) = ServiceRequest::new_controller_request(peer_id);
    sender.try_send(pcr)?;
    match response.await {
        Ok(controller) => Ok(controller),
        Err(e) => Err(e.into()),
    }
}

/// Polls the stream for the PeerManager FIDL interface to set target handlers and respond with
/// new controller clients.
pub async fn handle_peer_manager_requests(
    mut stream: PeerManagerRequestStream,
    mut sender: mpsc::Sender<ServiceRequest>,
) -> Result<(), anyhow::Error>
where
{
    let connect_tasks = fasync::Scope::new();
    while let Some(req) = stream.try_next().await? {
        match req {
            PeerManagerRequest::GetBrowseControllerForTarget { peer_id, client, responder } => {
                let peer_id = peer_id.into();
                info!("Received client request for browse controller for peer {}", peer_id);

                let controller = get_controller_for_peer(peer_id, &mut sender).await?;

                let _ = connect_tasks.spawn(async move {
                    let client_stream = client.into_stream();

                    // Wait for browse connection.
                    if let Err(e) = controller
                        .wait_for_browse_connection()
                        .on_timeout(MonotonicDuration::from_seconds(5), || {
                            Err(PeerError::ConnectionFailure(format_err!(
                                "browse connection timeout"
                            )))
                        })
                        .await
                    {
                        warn!(peer_id:%, e:?; "Browse connection timeout");
                        let _ = responder.send(Err(zx::Status::TIMED_OUT.into_raw()));
                        return;
                    }

                    // BrowseController can remain connected after PeerManager disconnects.
                    browse_controller_service::spawn_service(controller, client_stream).detach();
                    let _ = responder.send(Ok(()));
                });
            }
            PeerManagerRequest::GetControllerForTarget { peer_id, client, responder } => {
                let peer_id = peer_id.into();
                info!("Received client request for controller for peer {}", peer_id);

                let controller = get_controller_for_peer(peer_id, &mut sender).await?;

                let _ = connect_tasks.spawn(async move {
                    let client_stream = client.into_stream();

                    // Wait for control connection.
                    if let Err(e) = controller
                        .wait_for_control_connection()
                        .on_timeout(MonotonicDuration::from_seconds(5), || {
                            Err(PeerError::ConnectionFailure(format_err!(
                                "control connection timeout"
                            )))
                        })
                        .await
                    {
                        warn!(peer_id:%, e:?; "Control connection timeout");
                        let _ = responder.send(Err(zx::Status::TIMED_OUT.into_raw()));
                        return;
                    }
                    // Controller can remain connected after PeerManager disconnects.
                    controller_service::spawn_service(controller, client_stream).detach();
                    let _ = responder.send(Ok(()));
                });
            }
            PeerManagerRequest::SetAbsoluteVolumeHandler { handler, responder } => {
                info!("Received client request to set absolute volume handler");
                let absolute_volume_handler = handler.into_proxy();
                let (response, register_absolute_volume_handler_request) =
                    ServiceRequest::new_register_absolute_volume_handler_request(
                        absolute_volume_handler,
                    );
                sender.try_send(register_absolute_volume_handler_request)?;
                match response.await? {
                    Ok(_) => responder.send(Ok(()))?,
                    Err(_) => responder.send(Err(zx::Status::ALREADY_BOUND.into_raw()))?,
                }
            }
            PeerManagerRequest::RegisterTargetHandler { handler, responder } => {
                info!("Received client request for registering Target Handler");
                let target_handler = handler.into_proxy();
                let (response, register_target_handler_request) =
                    ServiceRequest::new_register_target_handler_request(target_handler);
                sender.try_send(register_target_handler_request)?;
                match response.await? {
                    Ok(_) => responder.send(Ok(()))?,
                    Err(_) => responder.send(Err(zx::Status::ALREADY_BOUND.into_raw()))?,
                }
            }
        }
    }
    Ok(())
}

/// Polls the stream for the PeerManagerExt FIDL interface and responds with new test controller clients.
pub async fn handle_peer_manager_ext_requests(
    mut stream: PeerManagerExtRequestStream,
    mut sender: mpsc::Sender<ServiceRequest>,
) -> Result<(), anyhow::Error> {
    while let Some(req) = stream.try_next().await? {
        match req {
            PeerManagerExtRequest::GetBrowseControllerForTarget { peer_id, client, responder } => {
                let peer_id: PeerId = peer_id.into();
                info!("New test connection request for {}", peer_id);

                let client_stream = client.into_stream();
                let (response, pcr) = ServiceRequest::new_controller_request(peer_id);
                sender.try_send(pcr)?;
                let controller = response.await?;
                // BrowseControllerExt can remain connected after PeerManager disconnects.
                browse_controller_service::spawn_ext_service(controller, client_stream).detach();
                responder.send(Ok(()))?;
            }
            PeerManagerExtRequest::GetControllerForTarget { peer_id, client, responder } => {
                let peer_id: PeerId = peer_id.into();
                info!("New test connection request for {}", peer_id);

                let client_stream = client.into_stream();
                let (response, pcr) = ServiceRequest::new_controller_request(peer_id);
                sender.try_send(pcr)?;
                let controller = response.await?;
                // ControllerExt can remain connected after PeerManager disconnects.
                controller_service::spawn_ext_service(controller, client_stream).detach();
                responder.send(Ok(()))?;
            }
        }
    }
    Ok(())
}

/// Sets up public FIDL services and client handlers.
pub fn run_services(
    mut fs: ServiceFs<ServiceObj<'_, ()>>,
    sender: mpsc::Sender<ServiceRequest>,
) -> Result<impl Future<Output = Result<(), Error>> + '_, Error> {
    let sender_avrcp = sender.clone();
    let sender_test = sender;
    let _ = fs
        .dir("svc")
        .add_fidl_service_at(PeerManagerExtMarker::PROTOCOL_NAME, move |stream| {
            fasync::Task::spawn(
                handle_peer_manager_ext_requests(stream, sender_test.clone()).unwrap_or_else(
                    |e: anyhow::Error| warn!("Test AVRCP client handler finished: {:?}", e),
                ),
            )
            .detach();
        })
        .add_fidl_service_at(PeerManagerMarker::PROTOCOL_NAME, move |stream| {
            fasync::Task::spawn(
                handle_peer_manager_requests(stream, sender_avrcp.clone()).unwrap_or_else(
                    |e: anyhow::Error| warn!("AVRCP client handler finished: {:?}", e),
                ),
            )
            .detach();
        });
    let _ = fs.take_and_serve_directory_handle()?;
    info!("Running fidl service");
    Ok(fs.collect::<()>().map(|_| Err(format_err!("FIDL service listener returned"))))
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_matches::assert_matches;
    use async_utils::PollExt;
    use bt_avctp::{AvcCommand, AvcPeer, AvcResponseType, AvctpPeer};
    use bt_channel_test_support::{Transport, create_test_channels};
    use fidl::endpoints::{create_endpoints, create_proxy, create_proxy_and_stream};
    use fidl_fuchsia_bluetooth_bredr::{ProfileMarker, ProfileProxy, ProfileRequestStream};
    use fuchsia_bluetooth::profile::Psm;
    use fuchsia_bluetooth::types::Channel;
    use packet_encoding::Decodable;
    use std::collections::HashSet;
    use std::pin::pin;
    use std::sync::Arc;
    use test_case::test_case;

    use crate::packets::{PlaybackStatus as PacketPlaybackStatus, *};
    use crate::peer::{Controller, RemotePeerHandle};
    use crate::peer_manager::{PeerManager, TargetDelegate};
    use fuchsia_bluetooth::profile::avrcp::{
        AvrcpProtocolVersion, AvrcpService, AvrcpTargetFeatures,
    };

    fn handle_get_controller(
        exec: &mut fasync::TestExecutor,
        profile_proxy: ProfileProxy,
        request: ServiceRequest,
        transport: Transport,
    ) -> (Channel, Channel) {
        match request {
            ServiceRequest::GetController { peer_id, reply } => {
                // TODO(jamuraa): Make Controller a trait so we can mock it here.
                let peer = RemotePeerHandle::spawn_peer(
                    peer_id,
                    Arc::new(TargetDelegate::new()),
                    profile_proxy,
                );
                // Set a valid control connection and browse connection for the peer.
                let (local, remote_control) = create_test_channels(transport);
                let control_channel = AvcPeer::new(local);
                peer.set_control_connection(control_channel);

                let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

                let (local_browse, remote_browse) = create_test_channels(transport);
                let browse_channel = AvctpPeer::new(local_browse);
                peer.set_browse_connection(browse_channel);

                let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

                reply.send(Controller::new(peer)).expect("reply should succeed");
                return (remote_control, remote_browse);
            }
            x => panic!("Unexpected request from client stream: {:?}", x),
        }
    }

    /// Tests that a request to register a target handler responds correctly.
    #[fuchsia::test]
    fn spawn_avrcp_target() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let (peer_manager_proxy, peer_manager_requests) =
            create_proxy_and_stream::<PeerManagerMarker>();

        let (client_sender, mut service_request_receiver) = mpsc::channel(512);

        let (target_client, _target_server) = create_endpoints::<TargetHandlerMarker>();

        let request_fut = peer_manager_proxy.register_target_handler(target_client);
        let mut request_fut = pin!(request_fut);

        let handler_fut = handle_peer_manager_requests(peer_manager_requests, client_sender);
        let mut handler_fut = pin!(handler_fut);

        // Make the request.
        exec.run_until_stalled(&mut request_fut).expect_pending("should not be ready");

        // Running the stream handler should produce a request to register a target
        exec.run_until_stalled(&mut handler_fut).expect_pending("should not be ready");

        let request =
            service_request_receiver.try_next().unwrap().expect("a request should be made");

        match request {
            ServiceRequest::RegisterTargetHandler { target_handler: _, reply } => {
                reply.send(Ok(())).expect("Reply should succeed");
            }
            x => panic!("Unexpected request from client stream: {:?}", x),
        };

        // The request should be answered.
        exec.run_until_stalled(&mut handler_fut).expect_pending("should not be ready");
        assert_matches!(
            exec.run_until_stalled(&mut request_fut).expect("should be ready"),
            Ok(Ok(()))
        );

        drop(peer_manager_proxy);

        // The handler should end when the client is closed.
        assert_matches!(exec.run_until_stalled(&mut handler_fut).expect("should be ready"), Ok(()));
    }

    /// Tests that the client stream handler will spawn a controller when a controller request
    /// successfully sets up a controller.
    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn spawn_avrcp_controllers(transport: Transport) {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let (peer_manager_proxy, peer_manager_requests) =
            create_proxy_and_stream::<PeerManagerMarker>();

        let (client_sender, mut service_request_receiver) = mpsc::channel(512);

        let (profile_proxy, _profile_requests) = create_proxy_and_stream::<ProfileMarker>();

        let (_c_proxy, controller_server) = create_proxy();
        let (_bc_proxy, bcontroller_server) = create_proxy();

        let handler_fut = handle_peer_manager_requests(peer_manager_requests, client_sender);
        let mut handler_fut = pin!(handler_fut);

        let request_fut =
            peer_manager_proxy.get_controller_for_target(&PeerId(123).into(), controller_server);
        let mut request_fut = pin!(request_fut);

        // Make the request.
        assert!(exec.run_until_stalled(&mut request_fut).is_pending());

        // Running the stream handler should produce a request for a controller.
        assert!(exec.run_until_stalled(&mut handler_fut).is_pending());

        let request =
            service_request_receiver.try_next().unwrap().expect("a request should be made");
        let _connections =
            handle_get_controller(&mut exec, profile_proxy.clone(), request, transport);

        // The handler should spawn the request after the reply.
        assert!(exec.run_until_stalled(&mut handler_fut).is_pending());

        assert_matches!(
            exec.run_until_stalled(&mut request_fut).expect("should be ready"),
            Ok(Ok(()))
        );

        let request_fut = peer_manager_proxy
            .get_browse_controller_for_target(&PeerId(123).into(), bcontroller_server);
        let mut request_fut = pin!(request_fut);

        // Make the request.
        exec.run_until_stalled(&mut request_fut).expect_pending("should be pending");

        // Running the stream handler should produce a request for a controller.
        exec.run_until_stalled(&mut handler_fut).expect_pending("should be pending");

        let request =
            service_request_receiver.try_next().unwrap().expect("a request should be made");
        let _connections =
            handle_get_controller(&mut exec, profile_proxy.clone(), request, transport);

        // The handler should spawn the request after the reply.
        exec.run_until_stalled(&mut handler_fut).expect_pending("should be pending");

        assert_matches!(
            exec.run_until_stalled(&mut request_fut).expect("should be ready"),
            Ok(Ok(()))
        );

        drop(peer_manager_proxy);

        // The handler should end when the client is closed.
        assert_matches!(exec.run_until_stalled(&mut handler_fut).expect("should be ready"), Ok(()));
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    /// Test that getting a controller from the test server (PeerManagerExt) works.
    fn spawn_avrcp_extension_controllers(transport: Transport) {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let (peer_manager_ext_proxy, peer_manager_ext_requests) =
            create_proxy_and_stream::<PeerManagerExtMarker>();

        let (client_sender, mut service_request_receiver) = mpsc::channel(512);

        let (profile_proxy, _profile_requests) = create_proxy_and_stream::<ProfileMarker>();

        let (_c_proxy, controller_server) = create_proxy();
        let (_bc_proxy, bcontroller_server) = create_proxy();

        let handler_fut =
            handle_peer_manager_ext_requests(peer_manager_ext_requests, client_sender);
        let mut handler_fut = pin!(handler_fut);

        let request_fut = peer_manager_ext_proxy
            .get_controller_for_target(&PeerId(123).into(), controller_server);
        let mut request_fut = pin!(request_fut);

        // Make the request.
        exec.run_until_stalled(&mut request_fut).expect_pending("should be pending");

        // Running the stream handler should produce a request for a controller.
        exec.run_until_stalled(&mut handler_fut).expect_pending("should be pending");

        let request =
            service_request_receiver.try_next().unwrap().expect("a request should be made");
        let _connections =
            handle_get_controller(&mut exec, profile_proxy.clone(), request, transport);

        // The handler should spawn the request after the reply.
        exec.run_until_stalled(&mut handler_fut).expect_pending("should be pending");

        assert_matches!(
            exec.run_until_stalled(&mut request_fut).expect("should be ready"),
            Ok(Ok(()))
        );

        let request_fut = peer_manager_ext_proxy
            .get_browse_controller_for_target(&PeerId(123).into(), bcontroller_server);
        let mut request_fut = pin!(request_fut);

        // Make the request.
        exec.run_until_stalled(&mut request_fut).expect_pending("should be pending");

        // Running the stream handler should produce a request for a controller.
        exec.run_until_stalled(&mut handler_fut).expect_pending("should be pending");

        let request =
            service_request_receiver.try_next().unwrap().expect("a request should be made");
        let _connections =
            handle_get_controller(&mut exec, profile_proxy.clone(), request, transport);

        // The handler should spawn the request after the reply.
        exec.run_until_stalled(&mut handler_fut).expect_pending("should be pending");

        assert_matches!(
            exec.run_until_stalled(&mut request_fut).expect("should be ready"),
            Ok(Ok(()))
        );

        drop(peer_manager_ext_proxy);

        // The handler should end when the client is closed.
        assert_matches!(exec.run_until_stalled(&mut handler_fut).expect("shuld be ready"), Ok(()));
    }

    fn spawn_peer_manager()
    -> (PeerManager, ProfileRequestStream, PeerManagerProxy, mpsc::Receiver<ServiceRequest>) {
        let (client_sender, service_request_receiver) = mpsc::channel(2);

        let (profile_proxy, profile_requests) = create_proxy_and_stream::<ProfileMarker>();

        let peer_manager = PeerManager::new(profile_proxy);

        let (peer_manager_proxy, peer_manager_requests) =
            create_proxy_and_stream::<PeerManagerMarker>();

        fasync::Task::spawn(async move {
            let _ = handle_peer_manager_requests(peer_manager_requests, client_sender).await;
        })
        .detach();

        (peer_manager, profile_requests, peer_manager_proxy, service_request_receiver)
    }

    #[fuchsia::test]
    fn target_delegate_target_handler_already_bound_test() {
        let mut exec = fasync::TestExecutor::new();

        let (mut peer_manager, _profile_requests, peer_manager_proxy, mut service_request_receiver) =
            spawn_peer_manager();

        // create an target handler.
        let (target_client, target_server) = create_endpoints::<TargetHandlerMarker>();

        // Make a request and start it.  It should be pending.
        let register_fut = peer_manager_proxy.register_target_handler(target_client);
        let mut register_fut = pin!(register_fut);

        exec.run_until_stalled(&mut register_fut).expect_pending("should be pending");

        // We should have a service request.
        let request = service_request_receiver
            .try_next()
            .unwrap()
            .expect("Service request should be handled");

        // Handing it to Peer Manager should resolve the request
        peer_manager.handle_service_request(request);

        assert_matches!(
            exec.run_until_stalled(&mut register_fut).expect("should be ready"),
            Ok(Ok(()))
        );

        // Dropping the server end of this should drop the handler.
        drop(target_server);

        // create an new target handler.
        let (target_client_2, _target_server_2) = create_endpoints::<TargetHandlerMarker>();

        // should succeed if the previous handler was dropped.
        let register_fut = peer_manager_proxy.register_target_handler(target_client_2);
        let mut register_fut = pin!(register_fut);
        exec.run_until_stalled(&mut register_fut).expect_pending("should be pending");
        let request = service_request_receiver
            .try_next()
            .unwrap()
            .expect("Service request should be handled");
        peer_manager.handle_service_request(request);
        assert_matches!(
            exec.run_until_stalled(&mut register_fut).expect("should be ready"),
            Ok(Ok(()))
        );

        // create an new target handler.
        let (target_client_3, _target_server_3) = create_endpoints::<TargetHandlerMarker>();

        // should fail since the target handler is already set.
        let register_fut = peer_manager_proxy.register_target_handler(target_client_3);
        let mut register_fut = pin!(register_fut);
        exec.run_until_stalled(&mut register_fut).expect_pending("should be pending");
        let request = service_request_receiver
            .try_next()
            .unwrap()
            .expect("Service request should be handled");
        peer_manager.handle_service_request(request);
        let expected_status = zx::Status::ALREADY_BOUND.into_raw();
        assert_matches!(exec.run_until_stalled(&mut register_fut).expect("should be ready"), Ok(Err(status)) if status == expected_status);
    }

    #[fuchsia::test]
    fn target_delegate_volume_handler_already_bound_test() {
        let mut exec = fasync::TestExecutor::new();

        let (mut peer_manager, _profile_requests, peer_manager_proxy, mut service_request_receiver) =
            spawn_peer_manager();

        // create a volume handler.
        let (volume_client, volume_server) = create_endpoints::<AbsoluteVolumeHandlerMarker>();

        // Make a request and start it.  It should be pending.
        let register_fut = peer_manager_proxy.set_absolute_volume_handler(volume_client);
        let mut register_fut = pin!(register_fut);

        exec.run_until_stalled(&mut register_fut).expect_pending("should be pending");

        // We should have a service request.
        let request = service_request_receiver
            .try_next()
            .unwrap()
            .expect("Service request should be handled");

        // Handing it to Peer Manager should resolve the request
        peer_manager.handle_service_request(request);

        assert_matches!(
            exec.run_until_stalled(&mut register_fut).expect("should be ready"),
            Ok(Ok(()))
        );

        // Dropping the server end of this should drop the handler.
        drop(volume_server);

        // create an new target handler.
        let (volume_client_2, _volume_server_2) = create_endpoints::<AbsoluteVolumeHandlerMarker>();

        // should succeed if the previous handler was dropped.
        let register_fut = peer_manager_proxy.set_absolute_volume_handler(volume_client_2);
        let mut register_fut = pin!(register_fut);
        exec.run_until_stalled(&mut register_fut).expect_pending("should be pending");
        let request = service_request_receiver
            .try_next()
            .unwrap()
            .expect("Service request should be handled");
        peer_manager.handle_service_request(request);
        assert_matches!(
            exec.run_until_stalled(&mut register_fut).expect("should be ready"),
            Ok(Ok(()))
        );

        // create an new target handler.
        let (volume_client_3, _volume_server_3) = create_endpoints::<AbsoluteVolumeHandlerMarker>();

        // should fail since the target handler is already set.
        let register_fut = peer_manager_proxy.set_absolute_volume_handler(volume_client_3);
        let mut register_fut = pin!(register_fut);
        exec.run_until_stalled(&mut register_fut).expect_pending("should be pending");
        let request = service_request_receiver
            .try_next()
            .unwrap()
            .expect("Service request should be handled");
        peer_manager.handle_service_request(request);
        let expected_status = zx::Status::ALREADY_BOUND.into_raw();
        assert_matches!(exec.run_until_stalled(&mut register_fut).expect("should be ready"), Ok(Err(status)) if status == expected_status);
    }

    // Integration test of the peer manager and the FIDL front end with a mock BDEDR backend an
    // emulated remote peer. Validates we can get a controller to a device we discovered, we can send
    // commands on that controller, and that we can send responses and have them be dispatched back as
    // responses to the FIDL frontend in AVRCP. Exercises a majority of the peer manager
    // controller logic.
    // 1. Creates a front end FIDL endpoints for the test controller interface, a peer manager, and mock
    //    backend.
    // 2. It then creates a channel and injects a fake services discovered and incoming connection
    //    event into the mock profile service.
    // 3. Obtains both a regular and test controller from the FIDL service.
    // 4. Issues a Key0 passthrough keypress and validates we got both a key up and key down event
    // 4. Issues a GetCapabilities command using get_events_supported on the test controller FIDL
    // 5. Issues a SetAbsoluteVolume command on the controller FIDL
    // 6. Issues a GetElementAttributes command, encodes multiple packets, and handles continuations
    // 7. Issues a GetPlayStatus command on the controller FIDL.
    // 8. Issues a GetPlayerApplicationSettings command on the controller FIDL.
    // 9. Issues a SetPlayerApplicationSettings command on the controller FIDL.
    // 10. Register event notification for position change callbacks and mock responses.
    // 11. Waits until we have a response to all commands from our mock remote service return expected
    //    values and we have received enough position change events.
    #[fuchsia::test]
    async fn test_peer_manager_with_fidl_client_and_mock_profile() {
        for transport in [Transport::Socket, Transport::Fidl] {
            test_peer_manager_with_fidl_client_and_mock_profile_impl(transport).await;
        }
    }

    async fn test_peer_manager_with_fidl_client_and_mock_profile_impl(transport: Transport) {
        const REQUESTED_VOLUME: u8 = 0x40;
        const SET_VOLUME: u8 = 0x24;
        let fake_peer_id = PeerId(0);
        const LOREM_IPSUM: &str = "Lorem ipsum dolor sit amet,\
         consectetur adipiscing elit. Nunc eget elit cursus ipsum \
         fermentum viverra id vitae lorem. Cras luctus elementum \
         metus vel volutpat. Vestibulum ante ipsum primis in \
         faucibus orci luctus et ultrices posuere cubilia \
         Curae; Praesent efficitur velit sed metus luctus \
         Mauris in ante ultrices, vehicula lorem non, sagittis metus. \
         Nam facilisis volutpat quam. Suspendisse sem ipsum, blandit ut faucibus vitae,\
         facilisis quis massa. Aliquam sagittis, orci sed dignissim vulputate, odio neque \
         tempor dui, vel feugiat metus massa id urna. Nam at risus sem.\
         Duis commodo suscipit metus, at placerat elit suscipit eget. Suspendisse interdum \
         id metus vitae porta. Ut cursus viverra imperdiet. Aliquam erat volutpat. \
         Curabitur vehicula mauris nec ex sollicitudin rhoncus. Integer ipsum libero, \
         porta id velit et, egestas facilisis tellus.\
         Mauris in ante ultrices, vehicula lorem non, sagittis metus.\
         Nam facilisis volutpat quam. Suspendisse sem ipsum, blandit ut faucibus vitae,\
         facilisis quis massa. Aliquam sagittis, orci sed dignissim vulputate, odio neque \
         tempor dui, vel feugiat metus massa id urna. Nam at risus sem.\
         Duis commodo suscipit metus, at placerat elit suscipit eget. Suspendisse interdum \
         id metus vitae porta. Ut cursus viverra imperdiet. Aliquam erat volutpat. \
         Curabitur vehicula mauris nec ex sollicitudin rhoncus. Integer ipsum libero, \
         porta id velit et, egestas facilisis tellus.";

        // when zero, we exit the test.
        let mut expected_commands: i64 = 0;

        let (peer_manager_proxy, peer_manager_requests) =
            create_proxy_and_stream::<PeerManagerMarker>();
        let (ext_proxy, ext_requests) = create_proxy_and_stream::<PeerManagerExtMarker>();

        let (client_sender, mut peer_controller_request_receiver) = mpsc::channel(512);

        let (local, remote) = create_test_channels(transport);
        let remote_peer = AvcPeer::new(remote);
        let (profile_proxy, _requests) = create_proxy::<ProfileMarker>();

        let mut peer_manager = PeerManager::new(profile_proxy);

        peer_manager.services_found(
            &fake_peer_id,
            vec![AvrcpService::Target {
                features: AvrcpTargetFeatures::CATEGORY1,
                psm: Psm::AVCTP,
                protocol_version: AvrcpProtocolVersion(1, 6),
            }],
        );

        peer_manager.new_control_connection(&fake_peer_id, local);

        let handler_fut =
            handle_peer_manager_requests(peer_manager_requests, client_sender.clone()).fuse();
        let mut handler_fut = pin!(handler_fut);

        let test_handler_fut =
            handle_peer_manager_ext_requests(ext_requests, client_sender.clone()).fuse();
        let mut test_handler_fut = pin!(test_handler_fut);

        let (controller_proxy, controller_server) = create_proxy();
        let get_controller_fut = peer_manager_proxy
            .get_controller_for_target(&fake_peer_id.into(), controller_server)
            .fuse();
        let mut get_controller_fut = pin!(get_controller_fut);

        let (controller_ext_proxy, controller_ext_server) = create_proxy();
        let get_test_controller_fut =
            ext_proxy.get_controller_for_target(&fake_peer_id.into(), controller_ext_server).fuse();
        let mut get_test_controller_fut = pin!(get_test_controller_fut);

        let is_connected_fut = controller_ext_proxy.is_connected().fuse();
        let mut is_connected_fut = pin!(is_connected_fut);

        let passthrough_fut = controller_proxy.send_command(AvcPanelCommand::Key0).fuse();
        let mut passthrough_fut = pin!(passthrough_fut);
        expected_commands += 1;
        let mut keydown_pressed = false;
        let mut keyup_pressed = false;

        let volume_fut = controller_proxy.set_absolute_volume(REQUESTED_VOLUME).fuse();
        let mut volume_fut = pin!(volume_fut);
        expected_commands += 1;

        let events_fut = controller_ext_proxy.get_events_supported().fuse();
        let mut events_fut = pin!(events_fut);
        expected_commands += 1;

        let get_media_attributes_fut = controller_proxy.get_media_attributes().fuse();
        let mut get_media_attributes_fut = pin!(get_media_attributes_fut);
        expected_commands += 1;

        let get_play_status_fut = controller_proxy.get_play_status().fuse();
        let mut get_play_status_fut = pin!(get_play_status_fut);
        expected_commands += 1;

        let attribute_ids =
            &[fidl_fuchsia_bluetooth_avrcp::PlayerApplicationSettingAttributeId::Equalizer];
        let get_player_application_settings_fut =
            controller_proxy.get_player_application_settings(attribute_ids).fuse();
        let mut get_player_application_settings_fut = pin!(get_player_application_settings_fut);
        expected_commands += 1;

        let get_all_player_application_settings_fut =
            controller_proxy.get_player_application_settings(&[]).fuse();
        let mut get_all_player_application_settings_fut =
            pin!(get_all_player_application_settings_fut);
        expected_commands += 1;

        let mut settings = fidl_fuchsia_bluetooth_avrcp::PlayerApplicationSettings::default();
        settings.scan_mode = Some(fidl_fuchsia_bluetooth_avrcp::ScanMode::GroupScan);
        settings.shuffle_mode = Some(fidl_fuchsia_bluetooth_avrcp::ShuffleMode::Off);
        let set_player_application_settings_fut =
            controller_proxy.set_player_application_settings(&settings).fuse();
        let mut set_player_application_settings_fut = pin!(set_player_application_settings_fut);
        expected_commands += 1;

        let current_battery_status = BatteryStatus::Warning;
        let inform_battery_status_fut =
            controller_proxy.inform_battery_status(current_battery_status).fuse();
        let mut inform_battery_status_fut = pin!(inform_battery_status_fut);
        expected_commands += 1;

        let mut additional_packets: Vec<Vec<u8>> = vec![];

        // set controller event filter to ones we support.
        let _ = controller_proxy
            .set_notification_filter(Notifications::TRACK_POS | Notifications::VOLUME, 1)
            .unwrap();

        let mut volume_value_received = false;

        let event_stream = controller_proxy.take_event_stream();
        let mut event_stream = pin!(event_stream);

        let mut position_changed_events = 0;

        let remote_command_stream = remote_peer.take_command_stream().fuse();
        let mut remote_command_stream = pin!(remote_command_stream);

        let mut handle_remote_command = |avc_command: AvcCommand| {
            if avc_command.is_vendor_dependent() {
                let (pdu_id, body) =
                    decode_avc_vendor_command(&avc_command).expect("should succeed");

                match pdu_id {
                    PduId::GetElementAttributes => {
                        let _get_element_attributes_command =
                            GetElementAttributesCommand::decode(body)
                                .expect("unable to decode get element attributes");

                        // We are making a massive response to test packet continuations work properly.
                        let element_attributes_response =
                            GetElementAttributesResponse(MediaAttributeEntries {
                                title: Some(String::from(&LOREM_IPSUM[0..100])),
                                artist_name: Some(String::from(&LOREM_IPSUM[500..])),
                                album_name: Some(String::from(&LOREM_IPSUM[250..500])),
                                genre: Some(String::from(&LOREM_IPSUM[100..250])),
                                ..MediaAttributeEntries::default()
                            });
                        let mut packets = element_attributes_response
                            .encode_packets()
                            .expect("unable to encode packets for event");
                        let _ = avc_command
                            .send_response(AvcResponseType::ImplementedStable, &packets[0][..]);
                        drop(packets.remove(0));
                        additional_packets = packets;
                    }
                    PduId::RequestContinuingResponse => {
                        let request_cont_response = RequestContinuingResponseCommand::decode(body)
                            .expect("unable to decode continuting response");
                        assert_eq!(request_cont_response.pdu_id_response(), 0x20); // GetElementAttributes

                        let _ = avc_command.send_response(
                            AvcResponseType::ImplementedStable,
                            &additional_packets[0][..],
                        );
                        drop(additional_packets.remove(0));
                    }
                    PduId::RegisterNotification => {
                        let register_notification_command =
                            RegisterNotificationCommand::decode(body)
                                .expect("unable to decode packet body");

                        assert!(
                            register_notification_command.event_id()
                                == &NotificationEventId::EventPlaybackPosChanged
                                || register_notification_command.event_id()
                                    == &NotificationEventId::EventVolumeChanged
                        );

                        match register_notification_command.event_id() {
                            NotificationEventId::EventPlaybackPosChanged => {
                                position_changed_events += 1;

                                let intirm_response = PlaybackPosChangedNotificationResponse::new(
                                    1000 * position_changed_events,
                                )
                                .encode_packet()
                                .expect("unable to encode pos response packet");
                                let _ = avc_command
                                    .send_response(AvcResponseType::Interim, &intirm_response[..]);

                                // we are going to hang the response and not return an changed response after the 50th call.
                                // the last interim response should be
                                if position_changed_events < 50 {
                                    let change_response =
                                        PlaybackPosChangedNotificationResponse::new(
                                            1000 * (position_changed_events + 1),
                                        )
                                        .encode_packet()
                                        .expect("unable to encode pos response packet");
                                    let _ = avc_command.send_response(
                                        AvcResponseType::Changed,
                                        &change_response[..],
                                    );
                                }
                            }
                            NotificationEventId::EventVolumeChanged => {
                                let intirm_response = VolumeChangedNotificationResponse::new(0x22)
                                    .encode_packet()
                                    .expect("unable to encode volume response packet");
                                let _ = avc_command
                                    .send_response(AvcResponseType::Interim, &intirm_response[..]);

                                // we do not send an change response. We just hang it as if the volume never changes.
                            }
                            _ => assert!(false),
                        }
                    }
                    PduId::SetAbsoluteVolume => {
                        let set_absolute_volume_command =
                            SetAbsoluteVolumeCommand::decode(body).expect("unable to packet body");
                        assert_eq!(set_absolute_volume_command.volume(), REQUESTED_VOLUME);
                        let response = SetAbsoluteVolumeResponse::new(SET_VOLUME)
                            .expect("volume error")
                            .encode_packet()
                            .expect("unable to encode volume response packet");
                        let _ = avc_command.send_response(AvcResponseType::Accepted, &response[..]);
                    }
                    PduId::GetCapabilities => {
                        let get_capabilities_command =
                            GetCapabilitiesCommand::decode(body).expect("unable to packet body");
                        assert_eq!(
                            get_capabilities_command.capability_id(),
                            GetCapabilitiesCapabilityId::EventsId
                        );
                        // only notification types we claim to have are battery status, track pos, and volume
                        let response = GetCapabilitiesResponse::new_events(&[0x05, 0x06, 0x0d])
                            .encode_packet()
                            .expect("unable to encode response");
                        let _ = avc_command
                            .send_response(AvcResponseType::ImplementedStable, &response[..]);
                    }
                    PduId::GetPlayStatus => {
                        let _get_play_status_comand = GetPlayStatusCommand::decode(body)
                            .expect("GetPlayStatus: unable to packet body");
                        // Reply back with arbitrary status response. Song pos not supported.
                        let response = GetPlayStatusResponse {
                            song_length: 100,
                            song_position: 0xFFFFFFFF,
                            playback_status: PacketPlaybackStatus::Stopped,
                        }
                        .encode_packet()
                        .expect("unable to encode response");
                        let _ = avc_command
                            .send_response(AvcResponseType::ImplementedStable, &response[..]);
                    }
                    PduId::GetCurrentPlayerApplicationSettingValue => {
                        let _get_player_application_settings_command =
                            GetCurrentPlayerApplicationSettingValueCommand::decode(body)
                                .expect("GetPlayerApplicationSettings: unable to packet body");
                        let response = GetCurrentPlayerApplicationSettingValueResponse::new(vec![(
                        player_application_settings::PlayerApplicationSettingAttributeId::Equalizer,
                        0x01,
                    )])
                        .encode_packet()
                        .expect("Unable to encode response");
                        let _ = avc_command
                            .send_response(AvcResponseType::ImplementedStable, &response[..]);
                    }
                    PduId::SetPlayerApplicationSettingValue => {
                        let _set_player_application_settings_command =
                            SetPlayerApplicationSettingValueCommand::decode(body)
                                .expect("SetPlayerApplicationSettings: unable to packet body");
                        let response = SetPlayerApplicationSettingValueResponse::new()
                            .encode_packet()
                            .expect("Unable to encode response");
                        let _ = avc_command.send_response(AvcResponseType::Accepted, &response[..]);
                    }
                    PduId::ListPlayerApplicationSettingAttributes => {
                        let _list_attributes_command =
                            ListPlayerApplicationSettingAttributesCommand::decode(body).expect(
                                "ListPlayerApplicationSettingAttributes: unable to packet body",
                            );
                        let response = ListPlayerApplicationSettingAttributesResponse::new(
                            1,
                            vec![player_application_settings::PlayerApplicationSettingAttributeId::Equalizer],
                        )
                        .encode_packet()
                        .expect("Unable to encode response");
                        let _ = avc_command
                            .send_response(AvcResponseType::ImplementedStable, &response[..]);
                    }
                    PduId::GetPlayerApplicationSettingAttributeText => {
                        let _get_attribute_text_command =
                            GetPlayerApplicationSettingAttributeTextCommand::decode(body).expect(
                                "GetPlayerApplicationSettingAttributeText: unable to packet body",
                            );
                        let response = GetPlayerApplicationSettingAttributeTextResponse::new(vec![
                            AttributeInfo::new(
                                player_application_settings::PlayerApplicationSettingAttributeId::Equalizer,
                                CharsetId::Utf8,
                                1,
                                vec![0x62],
                            ),
                        ])
                        .encode_packet()
                        .expect("Unable to encode response");
                        let _ = avc_command
                            .send_response(AvcResponseType::ImplementedStable, &response[..]);
                    }
                    PduId::ListPlayerApplicationSettingValues => {
                        let _list_value_command =
                            ListPlayerApplicationSettingValuesCommand::decode(body).expect(
                                "ListPlayerApplicationSettingValues: unable to packet body",
                            );
                        let response =
                            ListPlayerApplicationSettingValuesResponse::new(2, vec![0x01, 0x02])
                                .encode_packet()
                                .expect("Unable to encode response");
                        let _ = avc_command
                            .send_response(AvcResponseType::ImplementedStable, &response[..]);
                    }
                    PduId::GetPlayerApplicationSettingValueText => {
                        let _get_value_text_command =
                            GetPlayerApplicationSettingValueTextCommand::decode(body).expect(
                                "GetPlayerApplicationSettingValueText: unable to packet body",
                            );
                        let response = GetPlayerApplicationSettingValueTextResponse::new(vec![
                            ValueInfo::new(1, CharsetId::Utf8, 1, vec![0x63]),
                        ])
                        .encode_packet()
                        .expect("Unable to encode response");
                        let _ = avc_command
                            .send_response(AvcResponseType::ImplementedStable, &response[..]);
                    }
                    PduId::InformBatteryStatusOfCT => {
                        let received_command = InformBatteryStatusOfCtCommand::decode(body)
                            .expect("InformBatteryStatusOfCt: unable to decode packet body");
                        assert_eq!(received_command.battery_status(), current_battery_status);
                        let response = InformBatteryStatusOfCtResponse::new()
                            .encode_packet()
                            .expect("Unable to encode response");
                        let _ = avc_command.send_response(AvcResponseType::Accepted, &response[..]);
                    }
                    _ => {
                        // not entirely correct but just get it off our back for now.
                        let _ = avc_command.send_response(AvcResponseType::NotImplemented, &[]);
                    }
                }
            } else {
                // Passthrough
                let body = avc_command.body();
                if body[0] & 0x80 == 0 {
                    keydown_pressed = true;
                } else {
                    keyup_pressed = true;
                }
                let key_code = body[0] & !0x80;
                let command = AvcPanelCommand::from_primitive(key_code);
                assert_eq!(command, Some(AvcPanelCommand::Key0));
                let _ = avc_command.send_response(AvcResponseType::Accepted, &[]);
            }
        };

        let mut last_receieved_pos = 0;

        loop {
            futures::select! {
                command = remote_command_stream.select_next_some() => {
                    handle_remote_command(command.unwrap());
                }
                request = peer_controller_request_receiver.select_next_some()  => {
                    peer_manager.handle_service_request(request);
                }
                res = handler_fut => {
                    let _ = res.unwrap();
                    assert!(false, "handler returned");
                    drop(peer_manager_proxy);
                    return
                }
                res = test_handler_fut => {
                    let _ = res.unwrap();
                    assert!(false, "handler returned");
                    drop(ext_proxy);
                    return
                }
                res = get_controller_fut => {
                    let _ = res.unwrap();
                }
                res = get_test_controller_fut => {
                    let _ = res.unwrap();
                }
                res = is_connected_fut => {
                    assert!(res.unwrap());
                }
                res = passthrough_fut => {
                    expected_commands -= 1;
                    assert_eq!(res.unwrap(), Ok(()));
                }
                res = volume_fut => {
                    expected_commands -= 1;
                    assert_eq!(res.unwrap(), Ok(SET_VOLUME));
                }
                res = events_fut => {
                    expected_commands -= 1;
                    let mut expected_set: HashSet<NotificationEvent> = [NotificationEvent::TrackPosChanged, NotificationEvent::BattStatusChanged, NotificationEvent::VolumeChanged].iter().cloned().collect();
                    for item in res.unwrap().expect("supported events should be Ok") {
                        assert!(expected_set.remove(&item), "Missing {:?} in Events Supported.", item);
                    }
                }
                res = get_media_attributes_fut => {
                    expected_commands -= 1;
                    let media_attributes = res.unwrap().expect("unable to parse media attributes");
                    let expected = Some(String::from(&LOREM_IPSUM[100..250]));
                    assert_eq!(media_attributes.genre, expected);
                }
                res = get_play_status_fut => {
                    expected_commands -= 1;
                    let play_status = res.unwrap().expect("unable to parse play status");
                    assert_eq!(play_status.playback_status, Some(fidl_fuchsia_bluetooth_avrcp::PlaybackStatus::Stopped).into());
                    assert_eq!(play_status.song_length, Some(100));
                    assert_eq!(play_status.song_position, None);
                }
                res = get_player_application_settings_fut => {
                    expected_commands -= 1;
                    let settings = res.unwrap().expect("unable to parse get player application settings");
                    assert!(settings.equalizer.is_some());
                    assert!(settings.custom_settings.is_none());
                    assert!(settings.scan_mode.is_none());
                    assert!(settings.repeat_status_mode.is_none());
                    assert!(settings.shuffle_mode.is_none());
                    let eq = settings.equalizer.unwrap();
                    assert_eq!(eq, fidl_fuchsia_bluetooth_avrcp::Equalizer::Off);
                }
                res = get_all_player_application_settings_fut => {
                    expected_commands -= 1;
                    let settings = res.unwrap().expect("unable to parse get player application settings");
                    assert!(settings.equalizer.is_some());
                    assert_eq!(settings.equalizer.unwrap(), fidl_fuchsia_bluetooth_avrcp::Equalizer::Off);
                }
                res = set_player_application_settings_fut => {
                    expected_commands -= 1;
                    let set_settings = res.unwrap().expect("unable to parse set player application settings");
                    assert_eq!(set_settings.scan_mode, Some(fidl_fuchsia_bluetooth_avrcp::ScanMode::GroupScan));
                    assert_eq!(set_settings.shuffle_mode, Some(fidl_fuchsia_bluetooth_avrcp::ShuffleMode::Off));
                }
                res = inform_battery_status_fut => {
                    expected_commands -= 1;
                    assert_eq!(res.unwrap(), Ok(()));
                }
                event = event_stream.select_next_some() => {
                    let event = event.unwrap();
                    match event {
                        ControllerEvent::OnNotification { timestamp: _, notification } => {
                            if let Some(value) = notification.pos {
                                if value > last_receieved_pos {
                                    last_receieved_pos = value;
                                }
                            }

                            if let Some(value) = notification.volume {
                                volume_value_received = true;
                                assert_eq!(value, 0x22);
                            }

                            controller_proxy.notify_notification_handled().unwrap();
                        }
                    }
                }
            }
            if expected_commands <= 0 && last_receieved_pos >= 50 * 1000 && volume_value_received {
                assert!(keydown_pressed && keyup_pressed);
                return;
            }
        }
    }
}
