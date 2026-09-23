// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::format_err;
use bt_map::Error;
use fidl::endpoints::{ClientEnd, DiscoverableProtocolMarker, RequestStream};
use fidl_fuchsia_bluetooth_bredr as bredr;
use fidl_fuchsia_bluetooth_map::{
    AccessorMarker, AccessorRequest, AccessorRequestStream, MessagingClientMarker,
    MessagingClientRequest, MessagingClientRequestStream, MessagingClientWatchAccessorResponder,
    MessagingClientWatchAccessorResponse,
};
use fuchsia_bluetooth::profile::ProtocolDescriptor;
use fuchsia_bluetooth::types::{Channel, PeerId};
use fuchsia_sync::Mutex;
use futures::StreamExt;
use futures::channel::mpsc;
use futures::stream::{FusedStream, Stream};
use log::{info, warn};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Poll, Waker};

use crate::message_access_service::MasInstance;
use crate::message_notification_service::{Session, SessionSignal};
use crate::profile::MasConfig;

/// Manages connected peers and their availability to serve the Accessor FIDL service.
#[derive(Default)]
struct AccessorManager {
    connected_peers: Mutex<HashMap<PeerId, Arc<Accessor>>>,
}

impl AccessorManager {
    fn remove_disconnected(&self) {
        let mut lock = self.connected_peers.lock();
        lock.retain(|_, accessor| accessor.purge_disconnected_mas());
    }

    /// Returns a clone of [Arc<Accessor>] that can be used to serve the Accessor FIDL service.
    fn get_available_accessor(&self) -> Option<Arc<Accessor>> {
        self.remove_disconnected();

        for (_peer_id, accessor) in self.connected_peers.lock().iter() {
            if !accessor.is_fidl_running() {
                return Some(accessor.clone());
            }
        }
        None
    }
}

pub struct MessagingClient {
    profile_proxy: bredr::ProfileProxy,
    accessor_manager: AccessorManager,
    fidl_stream: Option<MessagingClientRequestStream>,
    accessor_request: Option<MessagingClientWatchAccessorResponder>,
    // Waker to notify the client of MessagingClient to poll on the stream.
    waker: Option<Waker>,
}

pub struct AccessorTasks {
    // Task for running the Accessor FIDL server.
    pub(crate) fidl_task: fuchsia_async::Task<PeerId>,
    // Task for processing the signals from the established Message
    // Notification Server for this Accessor.
    pub(crate) mns_signal_process_task: fuchsia_async::Task<()>,
}

/// Returns the Accessor FIDL service futures that were started from client's `WatchAccessor` requests.
/// Returns pending if there weren't any `WatchAccessor` requests or if the Accessor FIDL service
/// was not started because none of the peers were available.
impl Stream for MessagingClient {
    /// Future for the Accessor FIDL service that was started.
    type Item = AccessorTasks;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        if let Some(fidl_stream) = self.fidl_stream.as_mut() {
            match fidl_stream.poll_next_unpin(cx) {
                Poll::Pending => {}
                Poll::Ready(Some(Ok(MessagingClientRequest::WatchAccessor { responder }))) => {
                    if self.accessor_request.is_some() {
                        let _ = responder.send(Err(fidl_fuchsia_bluetooth_map::Error::BadRequest));
                    } else {
                        self.accessor_request = Some(responder);
                    }
                }
                Poll::Ready(Some(Ok(unknown))) => {
                    warn!("Unknown method received: {:?}", unknown);
                }
                error_or_terminated => {
                    let _ = self.fidl_stream.take();
                    let _ = self.accessor_request.take();
                    warn!(
                        "{:?} FIDL stream terminated{}",
                        MessagingClientMarker::PROTOCOL_NAME,
                        if let Poll::Ready(Some(Err(e))) = error_or_terminated {
                            format!(": {e:?}")
                        } else {
                            "".to_string()
                        }
                    );
                }
            }
        }

        // If no WatchAccessor request, we don't need to look for an available `Accessor`
        if self.accessor_request.is_none() {
            self.waker = Some(cx.waker().clone());
            return Poll::Pending;
        }

        let Some(accessor) = self.accessor_manager.get_available_accessor() else {
            self.waker = Some(cx.waker().clone());
            return Poll::Pending;
        };

        let (accessor_client, accessor_request_stream): (ClientEnd<AccessorMarker>, _) =
            fidl::endpoints::create_request_stream();
        let watch_responder = self.accessor_request.take().unwrap();
        let _ = watch_responder.send(Ok(MessagingClientWatchAccessorResponse {
            peer_id: Some(accessor.peer_id.into()),
            accessor: Some(accessor_client),
            ..Default::default()
        }));

        let (mns_signal_sender, mns_signal_receiver) = mpsc::channel(1);
        let fidl_task = fuchsia_async::Task::local(run_accessor_fidl_server(
            accessor.clone(),
            accessor_request_stream,
            mns_signal_sender,
        ));
        let mns_signal_process_task =
            fuchsia_async::Task::local(process_mns_signals(accessor, mns_signal_receiver));
        Poll::Ready(Some(AccessorTasks { fidl_task, mns_signal_process_task }))
    }
}

impl FusedStream for MessagingClient {
    fn is_terminated(&self) -> bool {
        false
    }
}

impl MessagingClient {
    pub fn new(profile_proxy: bredr::ProfileProxy) -> Self {
        MessagingClient {
            profile_proxy,
            accessor_manager: Default::default(),
            fidl_stream: None,
            accessor_request: None,
            waker: None,
        }
    }

    pub fn set_fidl_stream(&mut self, stream: MessagingClientRequestStream) -> Result<(), Error> {
        if self.fidl_stream.is_some() {
            // Only one MessagingClient FIDL client can be active at a time.
            // Close the new request stream.
            stream.control_handle().shutdown_with_epitaph(zx::Status::ALREADY_BOUND);
            return Err(Error::other(format!(
                "{} client connection rejected. At most one client can be active at a time",
                MessagingClientMarker::PROTOCOL_NAME
            )));
        }
        let _ = self.fidl_stream.replace(stream);

        // If new FIDL stream was set, notify the waker to try polling on MessagingClient stream.
        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
        Ok(())
    }

    /// Makes a L2CAP or RFCOMM connection to the new Message Access
    /// Service server from the specified peer.
    pub async fn connect_new_mas(
        &mut self,
        peer_id: PeerId,
        mas_config: MasConfig,
    ) -> Result<(), Error> {
        let channel = match self
            .profile_proxy
            .connect(&peer_id.into(), mas_config.connection_params())
            .await
            .map_err(|e| Error::Other(e.into()))?
        {
            Ok(chan) => chan,
            Err(e) => {
                return Err(Error::Other(format_err!(
                    "Failed to connect to peer {peer_id:}: {e:?}"
                )));
            }
        };
        let chan = channel.try_into().map_err(|e: zx::Status| Error::Other(e.into()))?;

        self.accessor_manager.remove_disconnected();

        let mut is_new_peer = false;
        let accessor = {
            let mut lock = self.accessor_manager.connected_peers.lock();
            let accessor = lock.entry(peer_id).or_insert_with(|| {
                is_new_peer = true;
                Arc::new(Accessor::new(peer_id))
            });
            accessor.clone()
        };

        accessor.set_mas_connection(chan, mas_config).await;

        // If new peer, wake the waker to notify that a new peer is available.
        if let Some(waker) = self.waker.take_if(|_| is_new_peer) {
            waker.wake();
        }
        Ok(())
    }

    /// Accepts an incoming L2CAP or RFCOMM connection from the Message Notification Service
    /// client at the remote peer.
    /// If the MNS connection was successfully established, returns the
    /// future for the [NotificationRegistration] FIDL server for the currently
    /// active MNS session.
    pub async fn new_mns_connection(
        &mut self,
        peer_id: PeerId,
        protocol: Vec<ProtocolDescriptor>,
        channel: Channel,
    ) -> Result<(), Error> {
        self.accessor_manager.remove_disconnected();

        let accessor = self
            .accessor_manager
            .connected_peers
            .lock()
            .get(&peer_id)
            .ok_or(Error::MasUnavailable)?
            .clone();

        if let Err(e) = accessor.set_mns_connection(protocol, channel).await {
            // If MNS connection was not set successfully, we unregister all notifications.
            let mas_instances = accessor.mns_session.mas_instances().keys().copied().collect();
            if let Err(e) = accessor.register_notifications(mas_instances, false).await {
                warn!(e:?, peer_id:%; "Failed to unregister notifications");
            }
            return Err(e);
        }

        Ok(())
    }
}

/// Runs the Accessor FIDL server for the [Accessor] with the specified [PeerId].
async fn run_accessor_fidl_server(
    accessor: Arc<Accessor>,
    mut accessor_request_stream: AccessorRequestStream,
    mns_signal_sender: mpsc::Sender<SessionSignal>,
) -> PeerId {
    let peer_id = accessor.peer_id;
    accessor.is_fidl_running.store(true, Ordering::SeqCst);

    while let Some(item) = accessor_request_stream.next().await {
        match item {
            Ok(request) => {
                accessor.handle_fidl_request(request, mns_signal_sender.clone()).await;
            }
            Err(e) => {
                warn!(e:%, peer_id:%; "Accessor FIDL server for peer will terminate.");
                break;
            }
        }
    }
    accessor.is_fidl_running.store(false, Ordering::SeqCst);
    peer_id
}

/// Processes the incoming MNS signals.
async fn process_mns_signals(
    accessor: Arc<Accessor>,
    mut mns_signal_receiver: mpsc::Receiver<SessionSignal>,
) {
    while let Some(signal) = mns_signal_receiver.next().await {
        match signal {
            SessionSignal::UnregisterNotifications => {
                if let Err(instances) = accessor
                    .register_notifications(
                        accessor.mns_session.mas_instances().keys().copied().collect(),
                        false,
                    )
                    .await
                {
                    warn!(instances:?; "Failed to unregister notifications");
                }
            }
            SessionSignal::Terminate => {
                accessor.mns_session.clean_up();
            }
        }
    }
}

/// Represents the remote MSE peer device. Manages all of the Message Access Service
/// and Message Notification Service connections to the peer.
/// There may be multiple MAS OBEX connections (one per MAS instance), but
/// there should only be 1 MNS OBEX connection.
pub struct Accessor {
    peer_id: PeerId,
    // Key is the MAS instance ID.
    mas_instances: Mutex<HashMap<u8, Arc<MasInstance>>>,
    mns_session: Session,
    is_fidl_running: AtomicBool,
}

impl Accessor {
    pub fn new(peer_id: PeerId) -> Self {
        Accessor {
            peer_id: peer_id,
            mas_instances: Default::default(),
            mns_session: Default::default(),
            is_fidl_running: AtomicBool::new(false),
        }
    }

    #[allow(dead_code)]
    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    fn is_fidl_running(&self) -> bool {
        self.is_fidl_running.load(Ordering::SeqCst)
    }

    async fn set_mas_connection(&self, channel: Channel, mas_config: MasConfig) {
        let mas_instance = MasInstance::create(channel, mas_config);

        // Some devices expect OBEX CONNECT request right away after the transport is connected.
        // Even if the initial OBEX CONNECT fails, do not deem the entire connection as having failed
        // since some platforms require additional user permission before OBEX connection.
        match mas_instance.connect().await {
            Ok(()) => info!(peer_id:% = self.peer_id; "automatic OBEX CONNECT accepted"),
            Err(e) => info!(peer_id:% = self.peer_id, e:?; "initial OBEX CONNECT failed"),
        }

        let _ = self.mas_instances.lock().insert(mas_instance.id(), Arc::new(mas_instance));
    }

    async fn set_mns_connection(
        &self,
        protocol: Vec<ProtocolDescriptor>,
        channel: Channel,
    ) -> Result<(), Error> {
        // Clear out any disconnected MAS instances.
        if self.mas_instances.lock().is_empty() {
            // The establishment of a MNS connection requires the
            // previous establishment of a MAS connection as per
            // MAP v1.4.2 section 6.4.3.
            return Err(Error::MasUnavailable);
        }

        // Accept the connection to finish MNS session establishment.
        self.mns_session.run(protocol, channel).await?;

        // Now that the MNS session is running, ensure that all
        // the configured MAS instances have their notifications turned on.
        // See MAP v1.4.2 Section 6.4.3 for details.
        let mas_instance_ids = self.mns_session.mas_instances().keys().copied().collect();
        if let Err(e) = self.register_notifications(mas_instance_ids, true).await {
            warn!(e:?, peer_id:% = self.peer_id; "Failed to register notifications for all requested MAS instances");
        }

        Ok(())
    }

    async fn register_notifications(
        &self,
        mas_instance_ids: Vec<u8>,
        notification_on: bool,
    ) -> Result<(), Vec<u8>> {
        let mut need_registration = vec![];
        {
            let mas_instances = self.mas_instances.lock();
            for instance_id in mas_instance_ids {
                if let Some(mas_instance) = mas_instances.get(&instance_id) {
                    if mas_instance.notification_registered() != notification_on {
                        need_registration.push(mas_instance.clone());
                    }
                }
            }
        }

        let mut failed = vec![];
        for instance in need_registration {
            if let Err(_e) = instance.set_register_notification(notification_on).await {
                failed.push(instance.id());
            }
        }

        if failed.is_empty() { Ok(()) } else { Err(failed) }
    }

    /// Remove the MAS instances where the underlying transport is still connected.
    /// Returns true if at least one MAS instances is still connected after purging.
    fn purge_disconnected_mas(&self) -> bool {
        let mut lock = self.mas_instances.lock();
        lock.retain(|_uid, instance| instance.is_transport_connected());
        !lock.is_empty()
    }

    async fn handle_fidl_request(
        &self,
        request: AccessorRequest,
        mns_session_signal_sender: mpsc::Sender<SessionSignal>,
    ) {
        // Before every request, clear out any MAS instances where the underlying
        // transport is closed.
        let _ = self.purge_disconnected_mas();

        let peer_id = self.peer_id;
        match request {
            AccessorRequest::ListAllMasInstances { responder } => {
                let discovered_mas_instances = self.mas_instances.lock();
                // If there are no connected MAS instances, that means
                // we lost connections with the peer.
                if discovered_mas_instances.is_empty() {
                    let _ = responder
                        .send(Err(fidl_fuchsia_bluetooth_map::Error::Unavailable))
                        .inspect_err(|e| warn!(e:%; "Failed to send FIDL response"));
                    return;
                }
                let instances: Vec<fidl_fuchsia_bluetooth_map::MasInstance> =
                    discovered_mas_instances
                        .iter()
                        .map(|(_, instance)| instance.as_ref().into())
                        .collect();
                let _ = responder
                    .send(Ok(instances.as_slice()))
                    .inspect_err(|e| warn!(e:%; "Failed to send FIDL response"));
            }
            AccessorRequest::SetNotificationRegistration { payload, responder } => {
                let Some(relayer) = payload.server else {
                    let _ = responder
                        .send(Err(fidl_fuchsia_bluetooth_map::Error::BadRequest))
                        .inspect_err(|e| warn!(e:%; "Failed to send FIDL response"));
                    return;
                };

                // If there aren't any MAS instances connected, return an error since we cannot
                // make MNS connection.
                if self.mas_instances.lock().is_empty() {
                    let _ = responder
                        .send(Err(fidl_fuchsia_bluetooth_map::Error::PeerDisconnected))
                        .inspect_err(|e| warn!(e:%; "Failed to send FIDL response"));
                    return;
                }

                let mut instance_ids = payload.mas_instance_ids.unwrap_or_default();
                let mut instance_info = HashMap::new();
                {
                    let lock = self.mas_instances.lock();
                    if instance_ids.is_empty() {
                        // If instance IDs argument is empty, we assume it's attempting
                        // to register notifications for all connected MAS instances.
                        instance_ids = lock.iter().map(|item| *item.0).collect();
                    }

                    for id in &instance_ids {
                        if let Some(instance) = lock.get(id) {
                            let _ = instance_info.insert(*id, instance.features());
                        }
                    }
                }

                if instance_info.len() != instance_ids.len() {
                    let _ = responder
                        .send(Err(fidl_fuchsia_bluetooth_map::Error::NotFound))
                        .inspect_err(|e| warn!(e:%; "Failed to send FIDL response"));
                    return;
                }

                // At this point, we're starting a process to establish a new MNS connection.
                // Turn on notifications for the first MAS instance in the list to initiate
                // the connection procedure as outlined in MAP v1.4.2 Section 4.5.
                let first_instance_id = instance_ids[0];
                if let Err(e) = self.register_notifications(vec![first_instance_id], true).await {
                    warn!(e:?, peer_id:%; "Failed to register notifications for MAS {first_instance_id}");
                    let _ = responder
                        .send(Err(fidl_fuchsia_bluetooth_map::Error::Unknown))
                        .inspect_err(|e| warn!(e:%; "Failed to send FIDL response"));
                    return;
                }

                if let Err(e) = self.mns_session.initialize(
                    instance_info,
                    relayer,
                    responder,
                    mns_session_signal_sender,
                ) {
                    warn!(e:%, peer_id:%; "Failed to initialize MNS session");
                }
            }
            unimplemented => {
                warn!("Request unimplemented {:?}", unimplemented);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_test_helpers::{expect_stream_item, run_while};
    use async_utils::PollExt;
    use bt_channel_test_support::{Transport, create_test_channels};
    use bt_map::{MapSupportedFeatures, MessageType};
    use bt_obex::header::{Header, HeaderSet};
    use bt_obex::operation::{OpCode, RequestPacket, ResponseCode, ResponsePacket};
    use fidl::endpoints::{create_proxy_and_stream, create_request_stream};
    use fidl_fuchsia_bluetooth_bredr::{ProfileMarker, ProfileRequest, ProfileRequestStream};
    use fidl_fuchsia_bluetooth_map::{
        AccessorSetNotificationRegistrationRequest, MasInstance, MessageType as FidlMessageType,
        MessagingClientProxy, NotificationRegistrationMarker,
    };
    use fuchsia_async as fasync;
    use fuchsia_bluetooth::profile::DataElement;
    use futures::SinkExt;
    use packet_encoding::{Decodable, Encodable};
    use std::pin::pin;
    use test_case::test_case;

    use crate::message_access_service::tests::send_ok_response;
    use crate::message_notification_service::tests::send_packet;

    const TEST_PEER_ID: u64 = 1;
    const TEST_MAS_INSTANCE_ID: u8 = 1;

    /// Checks that we have successfully sent out an OBEX CONNECT request
    /// and mock a response from the remote OBEX server.
    #[track_caller]
    fn verify_and_respond_to_obex_connect_request(
        exec: &mut fasync::TestExecutor,
        remote_mas_channel: &mut Channel,
        remote_response_ok: bool,
    ) {
        let request_raw =
            expect_stream_item(exec, remote_mas_channel).expect("should have request");
        let request = RequestPacket::decode(&request_raw[..]).expect("can decode request");
        assert_eq!(request.code(), &OpCode::Connect);

        if remote_response_ok {
            let mut response_fut = pin!(send_ok_response(
                remote_mas_channel,
                vec![Header::ConnectionId(1.try_into().unwrap())],
                vec![0x10, 0x00, 0xff, 0xff], // data for connection
            ));
            exec.run_until_stalled(&mut response_fut).expect("response should complete");
        } else {
            let response = ResponsePacket::new(
                ResponseCode::Forbidden,
                vec![],
                HeaderSet::from_headers(vec![]).unwrap(),
            );
            let mut response_buf = vec![0; response.encoded_len()];
            response.encode(&mut response_buf[..]).unwrap();
            let mut write_fut = pin!(remote_mas_channel.send(response_buf.to_vec()));
            exec.run_until_stalled(&mut write_fut)
                .expect("write should complete")
                .expect("no error");
        }
    }

    // Creates a `MessagingClient` for test purposes. Returned messaging
    // client is connected to a single MAS instance from a remote peer.
    #[track_caller]
    fn test_messaging_client(
        exec: &mut fasync::TestExecutor,
        initial_obex_connect_success: bool,
        transport: Transport,
    ) -> (ProfileRequestStream, MessagingClient, Channel, MessagingClientProxy) {
        let (profile_proxy, mut profile_requests) = create_proxy_and_stream::<ProfileMarker>();

        let mut messaging_client = MessagingClient::new(profile_proxy);

        // Can connect to MAS instance.
        let (local, mut remote) = create_test_channels(transport);
        {
            let connect_fut = messaging_client.connect_new_mas(
                PeerId(TEST_PEER_ID),
                MasConfig::new(
                    TEST_MAS_INSTANCE_ID,
                    "test 1",
                    vec![MessageType::SmsGsm],
                    bredr::ConnectParameters::L2cap(bredr::L2capParameters {
                        psm: Some(0x1),
                        ..Default::default()
                    }),
                    MapSupportedFeatures::NOTIFICATION
                        | MapSupportedFeatures::NOTIFICATION_REGISTRATION,
                ),
            );
            let mut connect_fut = pin!(connect_fut);
            let _ = exec.run_until_stalled(&mut connect_fut).expect_pending("should be pending");

            // We should have sent a connect request.
            let profile_req =
                expect_stream_item(exec, &mut profile_requests).expect("should have sent request");
            match profile_req {
                ProfileRequest::Connect { peer_id, connection: _, responder } => {
                    assert_eq!(peer_id, PeerId(TEST_PEER_ID).into());
                    responder.send(Ok(local.try_into().unwrap())).unwrap();
                }
                _ => panic!("should be connect request"),
            }

            let _ = exec.run_until_stalled(&mut connect_fut).expect_pending("should be pending");

            verify_and_respond_to_obex_connect_request(
                exec,
                &mut remote,
                initial_obex_connect_success,
            );

            let _ = exec
                .run_until_stalled(&mut connect_fut)
                .expect("should be ready")
                .expect("should succeed");
        }

        let (msg_client_proxy, msg_client_request_stream) =
            create_proxy_and_stream::<MessagingClientMarker>();

        messaging_client.set_fidl_stream(msg_client_request_stream).expect("should succeed");
        (profile_requests, messaging_client, remote, msg_client_proxy)
    }

    // Fakes an incoming MNS connection from remote peer.
    // Returns the remote peer end of the MNS connection and the future for the running MNS session.
    #[track_caller]
    fn fake_remote_msn_connection(
        exec: &mut fasync::TestExecutor,
        messaging_client: &mut MessagingClient,
        transport: Transport,
    ) -> Channel {
        let (local, mut remote_mns) = create_test_channels(transport);
        let connect_fut = messaging_client.new_mns_connection(
            PeerId(TEST_PEER_ID),
            vec![ProtocolDescriptor {
                protocol: bredr::ProtocolIdentifier::L2Cap,
                params: vec![DataElement::Uint16(1234)],
            }],
            local,
        );

        let mut connect_fut = pin!(connect_fut);
        exec.run_until_stalled(&mut connect_fut).expect_pending("accessor server still running");

        // We should expect an OBEX CONNECT request from remote peer.
        let connect_packet = RequestPacket::new(
            OpCode::Connect,
            vec![0x10, 0x00, 0xff, 0xff],
            HeaderSet::from_headers(vec![Header::Target(vec![
                0xbb, 0x58, 0x2b, 0x41, 0x42, 0x0c, 0x11, 0xdb, 0xb0, 0xde, 0x08, 0x00, 0x20, 0x0c,
                0x9a, 0x66,
            ])])
            .unwrap(),
        );
        send_packet(exec, &mut remote_mns, connect_packet);

        exec.run_until_stalled(&mut connect_fut).expect("should not be pending").expect("no error");

        remote_mns
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn accessor_stream(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (_profile_requests, mut messaging_client, _remote, messaging_client_proxy) =
            test_messaging_client(&mut exec, true, transport);

        let watch_req_fut = messaging_client_proxy.watch_accessor();
        let mut watch_req_fut = pin!(watch_req_fut);

        let _accessor_fut = expect_stream_item(&mut exec, &mut messaging_client);
        let response = exec
            .run_until_stalled(&mut watch_req_fut)
            .expect("should have received response")
            .expect("should be ok")
            .expect("should have response");
        assert_eq!(response.peer_id, Some(PeerId(TEST_PEER_ID).into()));

        let watch_req_fut2 = messaging_client_proxy.watch_accessor();
        let mut watch_req_fut2 = pin!(watch_req_fut2);
        exec.run_until_stalled(&mut messaging_client.next())
            .expect_pending("no accessor available");
        exec.run_until_stalled(&mut watch_req_fut2)
            .expect_pending("should be pending since no accessor");

        let watch_req_fut3 = messaging_client_proxy.watch_accessor();
        let mut watch_req_fut3 = pin!(watch_req_fut3);
        exec.run_until_stalled(&mut messaging_client.next())
            .expect_pending("no accessor available");
        let _ = exec
            .run_until_stalled(&mut watch_req_fut3)
            .expect("should be ready")
            .expect("should contain result")
            .expect_err("should have returned error");
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn accessor_terminatation(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (_profile_requests, mut messaging_client, _remote, messaging_client_proxy) =
            test_messaging_client(&mut exec, true, transport);

        let watch_req_fut = messaging_client_proxy.watch_accessor();
        let mut watch_req_fut = pin!(watch_req_fut);

        let AccessorTasks { fidl_task, .. } = expect_stream_item(&mut exec, &mut messaging_client);
        let mut accessor_fut = pin!(fidl_task);
        let response = exec
            .run_until_stalled(&mut watch_req_fut)
            .expect("should have received response")
            .expect("should be ok")
            .expect("should have response");

        exec.run_until_stalled(&mut accessor_fut).expect_pending("should be running");

        let watch_req_fut2 = messaging_client_proxy.watch_accessor();
        let mut watch_req_fut2 = pin!(watch_req_fut2);
        exec.run_until_stalled(&mut messaging_client.next())
            .expect_pending("no accessor available");
        exec.run_until_stalled(&mut watch_req_fut2)
            .expect_pending("should be pending since no accessor");

        // If the client end closes, the Accessor FIDL should terminate.
        drop(response.accessor);
        let _ = exec.run_until_stalled(&mut accessor_fut).expect("should have terminated");

        let AccessorTasks { fidl_task, .. } = expect_stream_item(&mut exec, &mut messaging_client);
        let response = exec
            .run_until_stalled(&mut watch_req_fut2)
            .expect("should have received response")
            .expect("should be ok")
            .expect("should have response");
        assert_eq!(response.peer_id, Some(PeerId(TEST_PEER_ID).into()));

        let mut accessor_fut = pin!(fidl_task);
        exec.run_until_stalled(&mut accessor_fut).expect_pending("should be running");
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn list_all_mas_instances(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        // Set up MessagingClient and run the Accessor FIDL.
        let (_profile_requests, mut messaging_client, _remote, messaging_client_proxy) =
            test_messaging_client(&mut exec, true, transport);
        let watch_req_fut = messaging_client_proxy.watch_accessor();
        let mut watch_req_fut = pin!(watch_req_fut);

        let AccessorTasks { fidl_task, .. } = expect_stream_item(&mut exec, &mut messaging_client);
        let response = exec
            .run_until_stalled(&mut watch_req_fut)
            .expect("should have received response")
            .expect("should be ok")
            .expect("should have response");

        let accessor_proxy = response.accessor.expect("should exist").into_proxy();
        let accessor_fut = pin!(fidl_task);

        // Incoming FIDL request for listing all MAS instances.
        let request_fut = accessor_proxy.list_all_mas_instances();
        let (request_res, _accessor_fut) = run_while(&mut exec, accessor_fut, request_fut);
        let mas_instanes = request_res.expect("fidl ok").expect("result ok");
        assert_eq!(
            mas_instanes,
            vec![MasInstance {
                id: Some(TEST_MAS_INSTANCE_ID),
                supported_message_types: Some(FidlMessageType::SMS_GSM),
                supports_notification: Some(true),
                ..Default::default()
            }]
        )
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn list_all_mas_instances_fail(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        // Set up MessagingClient and run the Accessor FIDL.
        let (_profile_requests, mut messaging_client, remote, messaging_client_proxy) =
            test_messaging_client(&mut exec, true, transport);
        let watch_req_fut = messaging_client_proxy.watch_accessor();
        let mut watch_req_fut = pin!(watch_req_fut);

        let AccessorTasks { fidl_task, .. } = expect_stream_item(&mut exec, &mut messaging_client);
        let response = exec
            .run_until_stalled(&mut watch_req_fut)
            .expect("should have received response")
            .expect("should be ok")
            .expect("should have response");

        let accessor_proxy = response.accessor.expect("should exist").into_proxy();
        let accessor_fut = pin!(fidl_task);

        // Drop the connection to the MAS instance.
        drop(remote);

        // Incoming FIDL request for listing all MAS instances.
        let request_fut = accessor_proxy.list_all_mas_instances();
        let (request_res, _accessor_fut) = run_while(&mut exec, accessor_fut, request_fut);
        let _ = request_res.expect("fidl ok").expect_err("should error");
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn set_notification_registration(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        // Set up MessagingClient and run the Accessor FIDL.
        let (_profile_requests, mut messaging_client, mut remote_mas, messaging_client_proxy) =
            test_messaging_client(&mut exec, false, transport);
        let watch_req_fut = messaging_client_proxy.watch_accessor();
        let mut watch_req_fut = pin!(watch_req_fut);

        let AccessorTasks { fidl_task, mut mns_signal_process_task } =
            expect_stream_item(&mut exec, &mut messaging_client);
        let response = exec
            .run_until_stalled(&mut watch_req_fut)
            .expect("should have received response")
            .expect("should be ok")
            .expect("should have response");

        let accessor_proxy = response.accessor.expect("should exist").into_proxy();
        let mut accessor_fut = pin!(fidl_task);

        // Case 1: SetNotificationRegistration with mas_instance_ids.
        let (relayer_client, relayer_request_stream) =
            create_request_stream::<NotificationRegistrationMarker>();

        let request_fut = accessor_proxy.set_notification_registration(
            AccessorSetNotificationRegistrationRequest {
                mas_instance_ids: Some(vec![TEST_MAS_INSTANCE_ID]),
                server: Some(relayer_client),
                ..Default::default()
            },
        );
        let mut request_fut = pin!(request_fut);

        {
            exec.run_until_stalled(&mut request_fut).expect_pending("should not be ready");
            exec.run_until_stalled(&mut accessor_fut)
                .expect_pending("accessor server still running");

            // Should have sent OBEX CONNET request to peer.
            verify_and_respond_to_obex_connect_request(&mut exec, &mut remote_mas, true);

            exec.run_until_stalled(&mut accessor_fut)
                .expect_pending("accessor server still running");
            exec.run_until_stalled(&mut request_fut).expect_pending("should not be ready");

            // Should have sent OBEX PUT request for registering notification.
            let request_raw = expect_stream_item(&mut exec, &mut remote_mas).expect("request");
            let request = RequestPacket::decode(&request_raw[..]).expect("can decode request");
            assert_eq!(request.code(), &OpCode::PutFinal);
            let mut response_fut = pin!(send_ok_response(&mut remote_mas, vec![], vec![]));
            exec.run_until_stalled(&mut response_fut).expect("response should complete");
        };

        // Still should be pending for response since MNS connection wasn't made.
        exec.run_until_stalled(&mut accessor_fut).expect_pending("accessor server still running");
        exec.run_until_stalled(&mut request_fut).expect_pending("should not be ready");

        // Mimic an incoming MNS connection which should successfully set up a MNS server.
        let remote_mns = fake_remote_msn_connection(&mut exec, &mut messaging_client, transport);

        exec.run_until_stalled(&mut accessor_fut).expect_pending("accessor server still running");

        exec.run_until_stalled(&mut request_fut)
            .expect("ready")
            .expect("fidl success")
            .expect("result success");

        // Reset the notification registration for second test case.
        {
            // Mimic the client terminating the FIDL for NotificationRegistration.
            drop(relayer_request_stream);

            exec.run_until_stalled(&mut mns_signal_process_task)
                .expect_pending("should be processing signal");

            // Should have sent OBEX PUT request for un-registering notification.
            let request_raw = expect_stream_item(&mut exec, &mut remote_mas).expect("request");
            let request = RequestPacket::decode(&request_raw[..]).expect("can decode request");
            assert_eq!(request.code(), &OpCode::PutFinal);
            let mut response_fut = pin!(send_ok_response(&mut remote_mas, vec![], vec![]));
            exec.run_until_stalled(&mut response_fut).expect("response should complete");

            exec.run_until_stalled(&mut accessor_fut)
                .expect_pending("accessor server still running");

            // Mimic the remote MNS client disconnecting from the MNS server.
            drop(remote_mns);

            exec.run_until_stalled(&mut accessor_fut)
                .expect_pending("accessor server still running");
        };

        // Case 2: SetNotificationRegistration without mas_instance_ids.
        let (relayer_client, _relayer_request_stream) =
            create_request_stream::<NotificationRegistrationMarker>();

        let request_fut = accessor_proxy.set_notification_registration(
            AccessorSetNotificationRegistrationRequest {
                mas_instance_ids: Some(vec![]),
                server: Some(relayer_client),
                ..Default::default()
            },
        );
        let mut request_fut = pin!(request_fut);

        {
            exec.run_until_stalled(&mut request_fut).expect_pending("should not be ready");
            exec.run_until_stalled(&mut accessor_fut)
                .expect_pending("accessor server still running");

            // Should have sent OBEX PUT request for registering notification.
            let request_raw = expect_stream_item(&mut exec, &mut remote_mas).expect("request");
            let request = RequestPacket::decode(&request_raw[..]).expect("can decode request");
            assert_eq!(request.code(), &OpCode::PutFinal);
            let mut response_fut = pin!(send_ok_response(&mut remote_mas, vec![], vec![]));
            exec.run_until_stalled(&mut response_fut).expect("response should complete");
        };

        // Still should be pending for response since MNS connection wasn't made.
        exec.run_until_stalled(&mut accessor_fut).expect_pending("accessor server still running");
        exec.run_until_stalled(&mut request_fut).expect_pending("should not be ready");

        // Mimic an incoming MNS connection which should successfully set up a MNS server.
        let _remote_mns = fake_remote_msn_connection(&mut exec, &mut messaging_client, transport);

        exec.run_until_stalled(&mut accessor_fut).expect_pending("accessor server still running");

        // Second Notification Registration should be successful.
        exec.run_until_stalled(&mut request_fut)
            .expect("ready")
            .expect("fidl success")
            .expect("result success");
    }
}
