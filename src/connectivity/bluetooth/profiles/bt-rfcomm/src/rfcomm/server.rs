// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use bt_rfcomm::ServerChannel;
use bt_rfcomm::profile::build_rfcomm_protocol;
use fidl::prelude::*;
use fidl_fuchsia_bluetooth::ErrorCode;
use fidl_fuchsia_bluetooth_bredr as bredr;
use fidl_fuchsia_bluetooth_rfcomm_test::RfcommTestRequest;
use fuchsia_async as fasync;
use fuchsia_bluetooth::detachable_map::DetachableMap;
use fuchsia_bluetooth::types::{Channel, PeerId};
use fuchsia_inspect as inspect;
use fuchsia_inspect_derive::{AttachError, Inspect};
use futures::FutureExt;
use futures::lock::Mutex;
use log::{debug, info, trace, warn};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::rfcomm::inspect::RfcommServerInspect;
use crate::rfcomm::session::Session;
use crate::rfcomm::types::{SignaledTask, status_to_rls_error};

/// An RFCOMM client that is registered with the RFCOMM server.
struct RegisteredClient {
    /// The proxy used to relay RFCOMM channels to the client.
    connection_receiver: bredr::ConnectionReceiverProxy,
    /// The channel number associated with the registration.
    _channel_number: inspect::UintProperty,
}

/// Manages the current clients of the RFCOMM server. Provides an API for registering,
/// unregistering, and relaying RFCOMM channels to clients.
#[derive(Clone, Inspect)]
pub struct Clients {
    #[inspect(forward)]
    inner: Arc<Mutex<ClientsInner>>,
}

#[derive(Inspect)]
pub struct ClientsInner {
    /// The currently registered clients. Each registered client is identified by a unique
    /// ServerChannel and can be inspected.
    #[inspect(skip)]
    channel_receivers: HashMap<ServerChannel, RegisteredClient>,
    /// The inspect node for the set of clients.
    inspect_node: inspect::Node,
}

impl Clients {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ClientsInner {
                channel_receivers: HashMap::new(),
                inspect_node: inspect::Node::default(),
            })),
        }
    }

    /// Returns the number of available spaces for clients that can be registered.
    async fn available_space(&self) -> usize {
        let inner = self.inner.lock().await;
        ServerChannel::all().filter(|sc| !inner.channel_receivers.contains_key(&sc)).count()
    }

    /// Removes the client that has registered `server_channel`.
    async fn remove(&self, server_channel: &ServerChannel) {
        let _ = self.inner.lock().await.channel_receivers.remove(server_channel);
    }

    /// Clears all the registered clients.
    async fn clear(&self) {
        self.inner.lock().await.channel_receivers.clear();
    }

    /// Reserves the next available ServerChannel for a client represented by a `proxy`.
    ///
    /// If allocated, returns the ServerChannel assigned to the client, None otherwise.
    pub async fn new_client(&self, proxy: bredr::ConnectionReceiverProxy) -> Option<ServerChannel> {
        let mut inner = self.inner.lock().await;
        let new_channel =
            ServerChannel::all().find(|sc| !inner.channel_receivers.contains_key(&sc));
        new_channel.map(|channel| {
            trace!(server_channel:% = channel; "Reserving RFCOMM channel");
            let tagged_client = RegisteredClient {
                connection_receiver: proxy,
                _channel_number: inner
                    .inspect_node
                    .create_uint(inspect::unique_name("channel_number"), u8::from(channel) as u64),
            };
            let _ = inner.channel_receivers.insert(channel, tagged_client);
            channel
        })
    }

    /// Delivers the `channel` to the client that has registered the `server_channel`.
    /// Returns an error if delivery fails or if there is no such client.
    pub async fn deliver_channel(
        &self,
        peer_id: PeerId,
        server_channel: ServerChannel,
        channel: Channel,
    ) -> Result<(), Error> {
        trace!(peer_id:%, server_channel:%; "Delivering RFCOMM channel to client");
        let inner = self.inner.lock().await;
        let client = inner
            .channel_receivers
            .get(&server_channel)
            .ok_or_else(|| format_err!("{server_channel:?} not registered"))?;
        // Build the RFCOMM protocol descriptor and relay the channel.
        let protocol: Vec<bredr::ProtocolDescriptor> =
            build_rfcomm_protocol(server_channel).iter().map(Into::into).collect();
        if client.connection_receiver.is_closed() {
            return Err(format_err!("connection receiver peer closed"));
        }
        client
            .connection_receiver
            .connected(&peer_id.into(), bredr::Channel::try_from(channel).unwrap(), &protocol)
            .map_err(|e| format_err!("{e:?}"))
    }
}

/// The RfcommServer handles connection requests from profiles clients and remote peers.
pub struct RfcommServer {
    /// The currently registered profile clients of the RFCOMM server.
    clients: Clients,

    /// Active sessions between us and a remote peer. Each Session will multiplex
    /// RFCOMM connections over a single L2CAP channel.
    /// There can only be one session per remote peer. See RFCOMM Section 5.2.
    sessions: DetachableMap<PeerId, Session>,

    /// Inspect node for Sessions to attach to.
    inspect: RfcommServerInspect,
}

impl Inspect for &mut RfcommServer {
    fn iattach(self, parent: &inspect::Node, name: impl AsRef<str>) -> Result<(), AttachError> {
        self.inspect.iattach(parent, name)?;
        self.clients.iattach(self.inspect.node(), "advertised_channels")?;
        Ok(())
    }
}

impl RfcommServer {
    pub fn new() -> Self {
        Self {
            clients: Clients::new(),
            sessions: DetachableMap::new(),
            inspect: RfcommServerInspect::default(),
        }
    }

    /// Returns true if a session identified by `id` exists and is currently
    /// active.
    /// An RFCOMM Session is active if there is a currently running processing task.
    pub fn is_active_session(&mut self, id: &PeerId) -> bool {
        self.sessions.get(id).is_some_and(|session| session.upgrade().is_some())
    }

    /// Returns the number of available server channels in this server.
    pub async fn available_server_channels(&self) -> usize {
        self.clients.available_space().await
    }

    /// De-allocates the server `channels` provided.
    pub async fn free_server_channels(&mut self, channels: &HashSet<ServerChannel>) {
        for sc in channels {
            self.clients.remove(sc).await;
        }
    }

    /// De-allocates all the server channels in this server.
    pub async fn free_all_server_channels(&mut self) {
        self.clients.clear().await;
    }

    /// Reserves the next available ServerChannel for a client's `proxy`.
    ///
    /// Returns the allocated ServerChannel.
    pub async fn allocate_server_channel(
        &mut self,
        proxy: bredr::ConnectionReceiverProxy,
    ) -> Option<ServerChannel> {
        self.clients.new_client(proxy).await
    }

    /// Opens an RFCOMM channel specified by `server_channel` with the remote peer.
    ///
    /// Returns an error if there is no session established with the peer.
    pub async fn open_rfcomm_channel(
        &mut self,
        peer_id: PeerId,
        server_channel: ServerChannel,
        responder: bredr::ProfileConnectResponder,
    ) -> Result<(), Error> {
        trace!(peer_id:%, server_channel:%; "Opening RFCOMM channel");

        match self.sessions.get(&peer_id).and_then(|s| s.upgrade()) {
            None => {
                // Peer either disconnected or doesn't exist.
                let _ = responder.send(Err(ErrorCode::Failed));
                Err(format_err!("Invalid peer ID {peer_id}"))
            }
            Some(session) => {
                let channel_opened_callback =
                    Box::new(move |channel: Result<Channel, ErrorCode>| {
                        let channel = channel.map(|c| bredr::Channel::try_from(c).unwrap());
                        responder.send(channel).map_err(|e| format_err!("{e:?}"))
                    });
                session.open_rfcomm_channel(server_channel, channel_opened_callback).await;
                Ok(())
            }
        }
    }

    /// Handles the establishment of a new L2CAP connection.
    ///
    /// Creates and stores a new RFCOMM Session for the provided `l2cap` channel.
    /// Returns Error if an active session already exists for the peer `id`.
    pub fn new_l2cap_connection(&mut self, peer_id: PeerId, l2cap: Channel) -> Result<(), Error> {
        if self.is_active_session(&peer_id) {
            return Err(format_err!("RFCOMM Session already exists with peer {peer_id}"));
        }
        info!(peer_id:%, max_tx:% = l2cap.max_tx_size(); "Handling new L2CAP connection for the RFCOMM PSM");

        // Create a new RFCOMM Session with the provided `channel_opened_callback` which will be
        // called anytime an RFCOMM channel is created. Opened RFCOMM channels will be delivered
        // to the `clients` of the `RfcommServer`.
        let clients = self.clients.clone();
        let channel_opened_callback = Box::new(move |server_channel, channel| {
            let peer_id = peer_id;
            let clients = clients.clone();
            async move { clients.deliver_channel(peer_id, server_channel, channel).await }.boxed()
        });
        let mut session = Session::create(peer_id, l2cap, channel_opened_callback);
        let _ = session.iattach(self.inspect.peers(), peer_id.to_string());
        let closed_fut = session.finished();
        if self.sessions.insert(peer_id, session).is_some() {
            debug!(peer_id:%; "Overwriting existing RFCOMM session");
        }

        // Task eagerly removes the Session from the set of active sessions upon termination.
        let detached_session = self.sessions.get(&peer_id).expect("just inserted");
        fasync::Task::spawn(async move {
            let _ = closed_fut.await;
            detached_session.detach();
        })
        .detach();

        Ok(())
    }

    /// Handles a `RfcommTest` FIDL request.
    pub async fn handle_test_request(&mut self, request: RfcommTestRequest) {
        info!("Received RFCOMM Test request: {:?}", request);
        // Note: The test request is a no-op if there is no connected session with the peer.
        match request {
            RfcommTestRequest::Disconnect { id, .. } => {
                let id = id.into();
                if let Some(session) = self.sessions.get(&id).and_then(|s| s.upgrade()) {
                    session.close().await;
                }
            }
            RfcommTestRequest::RemoteLineStatus { id, channel_number, status, .. } => {
                let id = id.into();
                let server_channel_number = match ServerChannel::try_from(channel_number) {
                    Ok(sc) => sc,
                    Err(e) => {
                        warn!(
                            "RemoteLineStatus FIDL request with invalid ServerChannel number: {e:?}"
                        );
                        return;
                    }
                };
                if let Some(session) = self.sessions.get(&id).and_then(|s| s.upgrade()) {
                    session
                        .send_remote_line_status(server_channel_number, status_to_rls_error(status))
                        .await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_matches::assert_matches;
    use async_utils::PollExt;
    use bt_channel_test_support::{
        Transport, create_test_channels, create_test_channels_with_max_tx,
    };
    use bt_rfcomm::frame::mux_commands::*;
    use bt_rfcomm::frame::*;
    use bt_rfcomm::{DLCI, Role};
    use diagnostics_assertions::assert_data_tree;
    use fidl::endpoints::{create_proxy, create_proxy_and_stream};
    use fidl_fuchsia_bluetooth_bredr::ConnectionReceiverMarker;
    use fuchsia_async as fasync;
    use fuchsia_inspect_derive::WithInspect;
    use futures::task::Poll;
    use futures::{SinkExt, StreamExt};
    use std::pin::pin;
    use test_case::test_case;

    use crate::rfcomm::test_util::{expect_frame_received_by_peer, send_peer_frame};

    fn setup_rfcomm_manager() -> (fasync::TestExecutor, RfcommServer) {
        let exec = fasync::TestExecutor::new();
        let rfcomm = RfcommServer::new();
        (exec, rfcomm)
    }

    #[fuchsia::test]
    async fn test_allocate_server_channel() {
        let mut rfcomm = RfcommServer::new();

        let expected_free_channels = ServerChannel::all().count();
        assert_eq!(rfcomm.available_server_channels().await, expected_free_channels);

        // Allocating a server channel should be OK.
        let (c, _s) = create_proxy::<ConnectionReceiverMarker>();
        let first_channel =
            rfcomm.allocate_server_channel(c.clone()).await.expect("should allocate");

        // Allocate the remaining n-1 channels.
        let mut n = expected_free_channels - 1;
        while n > 0 {
            assert!(rfcomm.allocate_server_channel(c.clone()).await.is_some());
            n -= 1;
        }

        // Allocating another should fail.
        assert_eq!(rfcomm.available_server_channels().await, 0);
        assert!(rfcomm.allocate_server_channel(c.clone()).await.is_none());

        // De-allocating should work.
        let single_channel = vec![first_channel].into_iter().collect();
        rfcomm.free_server_channels(&single_channel).await;

        // We should be able to allocate another now that space has freed.
        let (c, _s) = create_proxy::<ConnectionReceiverMarker>();
        assert!(rfcomm.allocate_server_channel(c).await.is_some());
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn test_new_l2cap_connection(transport: Transport) {
        let (mut exec, mut rfcomm) = setup_rfcomm_manager();

        let id = PeerId(123);
        let (mut remote, channel) = create_test_channels(transport);
        assert!(rfcomm.new_l2cap_connection(id, channel).is_ok());

        // The Session should still be active.
        assert!(rfcomm.is_active_session(&id));

        // Simulate peer sending RFCOMM data to the session - should be OK.
        let buf = [0x00, 0x00, 0x00];
        let mut write_fut = remote.send(buf.to_vec());
        exec.run_until_stalled(&mut write_fut)
            .expect("should be ready")
            .expect("write should succeed");

        // Remote peer disconnects - drive the background processing task to detect disconnection.
        drop(remote);
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

        // The session should be inactive now.
        assert!(!rfcomm.is_active_session(&id));
        // Checking again is OK.
        assert!(!rfcomm.is_active_session(&id));
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn test_new_rfcomm_channel_is_relayed_to_client(transport: Transport) {
        let (mut exec, mut rfcomm) = setup_rfcomm_manager();

        // Profile-client reserves a server channel.
        let (c, mut s) = create_proxy_and_stream::<ConnectionReceiverMarker>();
        let first_channel = {
            let fut = rfcomm.allocate_server_channel(c.clone());
            let mut fut = pin!(fut);
            match exec.run_until_stalled(&mut fut) {
                Poll::Ready(Some(sc)) => sc,
                x => panic!("Expected server channel but got {:?}", x),
            }
        };

        let profile_client_fut = s.next();
        let mut profile_client_fut = pin!(profile_client_fut);
        exec.run_until_stalled(&mut profile_client_fut)
            .expect_pending("waiting for connection request");

        // Start up a session with remote peer.
        let id = PeerId(1);
        let (local, mut remote) = create_test_channels(transport);
        assert!(rfcomm.new_l2cap_connection(id, local).is_ok());
        assert!(rfcomm.is_active_session(&id));

        // Remote peer requests to start up session multiplexer.
        let sabm = Frame::make_sabm_command(Role::Unassigned, DLCI::MUX_CONTROL_DLCI);
        send_peer_frame(&mut exec, &mut remote, sabm);
        // Expect to send a positive response to the peer.
        expect_frame_received_by_peer(&mut exec, &mut remote);

        // Remote peer requests to open an RFCOMM channel.
        let user_dlci = first_channel.to_dlci(Role::Responder).unwrap();
        let user_sabm = Frame::make_sabm_command(Role::Initiator, user_dlci);
        send_peer_frame(&mut exec, &mut remote, user_sabm);
        // Expect to send a positive response to the peer.
        expect_frame_received_by_peer(&mut exec, &mut remote);

        // The Session should open a new RFCOMM channel for the provided `user_dlci`, and
        // the Channel should be relayed to the profile client.
        let () = match exec.run_until_stalled(&mut profile_client_fut) {
            Poll::Ready(Some(Ok(bredr::ConnectionReceiverRequest::Connected { .. }))) => {}
            x => panic!("Expected connection but got {:?}", x),
        };
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn test_register_and_deliver_inbound_channel_to_clients(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        exec.run_singlethreaded(async move {
            let clients = Clients::new();

            // Initial capacity is the range of all valid Server Channels (1..30).
            let mut expected_space = 30;
            assert_eq!(clients.available_space().await, expected_space);

            // Attempting to deliver an inbound channel for an unregistered ServerChannel should be
            // an error.
            let random_server_channel = ServerChannel::try_from(10).unwrap();
            let (local, _remote) = create_test_channels(transport);
            assert!(
                clients.deliver_channel(PeerId(1), random_server_channel, local).await.is_err()
            );

            // Registering a new client should be OK.
            let (c, s) = create_proxy_and_stream::<ConnectionReceiverMarker>();
            let server_channel = clients.new_client(c).await.unwrap();
            expected_space -= 1;
            assert_eq!(clients.available_space().await, expected_space);

            // Delivering channel to registered client should be OK.
            let (local, _remote) = create_test_channels(transport);
            assert!(clients.deliver_channel(PeerId(1), server_channel, local).await.is_ok());

            // Client disconnects - delivering a new channel should fail.
            drop(s);
            let (local, _remote) = create_test_channels(transport);
            assert!(clients.deliver_channel(PeerId(1), server_channel, local).await.is_err());
        })
    }

    #[fuchsia::test]
    async fn clients_inspect_tree() {
        let inspect = inspect::Inspector::default();
        let clients = Clients::new()
            .with_inspect(inspect.root(), "advertised_channels")
            .expect("valid inspect tree");

        // Default inspect tree.
        assert_data_tree!(inspect, root: {
            advertised_channels: {
            }
        });

        // New client is ok.
        let (c1, _s1) = create_proxy_and_stream::<ConnectionReceiverMarker>();
        let ch_number1 = clients.new_client(c1).await.expect("valid client");
        let ch_number_raw1 = u8::from(ch_number1) as u64;
        assert_data_tree!(inspect, root: {
            advertised_channels: {
                channel_number0: ch_number_raw1,
            }
        });

        // Multiple clients is ok.
        let (c2, _s2) = create_proxy_and_stream::<ConnectionReceiverMarker>();
        let ch_number2 = clients.new_client(c2).await.expect("valid client");
        let ch_number_raw2 = u8::from(ch_number2) as u64;
        assert_data_tree!(inspect, root: {
            advertised_channels: {
                channel_number0: ch_number_raw1,
                channel_number1: ch_number_raw2,
            }
        });

        // Removing a client should result in an updated inspect tree.
        clients.remove(&ch_number1).await;
        assert_data_tree!(inspect, root: {
            advertised_channels: {
                channel_number1: ch_number_raw2,
            }
        });
    }

    /// Makes a client Profile::Connect() request and returns the responder for the request
    /// and a Future associated with the request.
    #[track_caller]
    fn make_client_connect_request(
        exec: &mut fasync::TestExecutor,
        id: PeerId,
    ) -> (
        bredr::ProfileConnectResponder,
        fidl::client::QueryResponseFut<Result<bredr::Channel, ErrorCode>>,
    ) {
        let (profile, mut profile_server) = create_proxy_and_stream::<bredr::ProfileMarker>();
        let mut profile_stream = Box::pin(profile_server.next());
        let connect_request = profile.connect(
            &id.into(),
            &bredr::ConnectParameters::L2cap(bredr::L2capParameters::default()),
        );
        let responder = match exec.run_until_stalled(&mut profile_stream) {
            Poll::Ready(Some(Ok(bredr::ProfileRequest::Connect { responder, .. }))) => responder,
            x => panic!("Expected ready connect request but got: {:?}", x),
        };
        (responder, connect_request)
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn test_request_outbound_connection_succeeds(transport: Transport) {
        let (mut exec, mut rfcomm) = setup_rfcomm_manager();

        // Start up a session with remote peer.
        let id = PeerId(1);
        let local_max_packet_size = 700;
        let (local, mut remote) =
            create_test_channels_with_max_tx(transport, local_max_packet_size as usize);
        assert!(rfcomm.new_l2cap_connection(id, local).is_ok());

        // Simulate a client connect request.
        let (responder, connect_request_fut) = make_client_connect_request(&mut exec, id);
        let mut connect_request_fut = pin!(connect_request_fut);
        exec.run_until_stalled(&mut connect_request_fut).expect_pending("waiting for channel");
        // We expect the open channel request to be OK - still awaiting the channel.
        let server_channel = ServerChannel::try_from(9).unwrap();
        let expected_dlci = server_channel.to_dlci(Role::Responder).unwrap();
        let mut outbound_fut = Box::pin(rfcomm.open_rfcomm_channel(id, server_channel, responder));
        assert_matches!(exec.run_until_stalled(&mut outbound_fut), Poll::Ready(Ok(_)));
        exec.run_until_stalled(&mut connect_request_fut).expect_pending("waiting for channel");

        // Expect to send a frame to the peer - SABM for mux startup.
        expect_frame_received_by_peer(&mut exec, &mut remote);
        // Simulate peer responding positively.
        let ua = Frame::make_ua_response(Role::Unassigned, DLCI::MUX_CONTROL_DLCI);
        send_peer_frame(&mut exec, &mut remote, ua);

        // Expect to send a frame to peer - parameter negotiation.
        expect_frame_received_by_peer(&mut exec, &mut remote);
        // Simulate peer responding positively with a larger max packet size.
        let data = MuxCommand {
            params: MuxCommandParams::ParameterNegotiation(ParameterNegotiationParams {
                dlci: expected_dlci,
                credit_based_flow_handshake: CreditBasedFlowHandshake::SupportedResponse,
                priority: 12,
                max_frame_size: 900,
                initial_credits: 1,
            }),
            command_response: CommandResponse::Response,
        };
        let pn_response = Frame::make_mux_command(Role::Responder, data);
        send_peer_frame(&mut exec, &mut remote, pn_response);

        // Expect to send a frame to peer - SABM for channel opening.
        expect_frame_received_by_peer(&mut exec, &mut remote);
        // Simulate peer responding positively.
        let ua = Frame::make_ua_response(Role::Responder, expected_dlci);
        send_peer_frame(&mut exec, &mut remote, ua);

        // The channel should be established and relayed to the client that requested it.
        let channel_held_by_client = match exec.run_until_stalled(&mut connect_request_fut) {
            Poll::Ready(Ok(Ok(c))) => c,
            x => panic!("Expected future ready with channel, got: {x:?}"),
        };
        // The reported max TX should be our preferred max TX since ours is smaller. Less 6 bytes
        // for the RFCOMM header.
        let expected_rfcomm_max_tx = local_max_packet_size - 6;
        assert_eq!(channel_held_by_client.max_tx_sdu_size, Some(expected_rfcomm_max_tx));
    }

    #[fuchsia::test]
    fn test_request_outbound_connection_invalid_peer() {
        let (mut exec, mut rfcomm) = setup_rfcomm_manager();

        // Simulate a client connect request.
        let random_id = PeerId(41);
        let (responder, connect_request_fut) = make_client_connect_request(&mut exec, random_id);
        let mut connect_request_fut = pin!(connect_request_fut);
        exec.run_until_stalled(&mut connect_request_fut).expect_pending("waiting for channel");

        // We expect the open channel request to fail.
        let server_channel = ServerChannel::try_from(8).unwrap();
        let mut outbound_fut =
            Box::pin(rfcomm.open_rfcomm_channel(random_id, server_channel, responder));
        assert_matches!(exec.run_until_stalled(&mut outbound_fut), Poll::Ready(Err(_)));
        // Responder should be notified of failure.
        assert_matches!(exec.run_until_stalled(&mut connect_request_fut), Poll::Ready(Ok(Err(_))));
    }
    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn test_server_inspect_hierarchy(transport: Transport) {
        let (mut exec, mut rfcomm) = setup_rfcomm_manager();
        let inspect = inspect::Inspector::default();
        rfcomm.iattach(inspect.root(), "rfcomm_server").expect("should attach");

        // Start up a session with remote peer.
        let id = PeerId(1);
        let (local, _remote) = create_test_channels(transport);
        assert!(rfcomm.new_l2cap_connection(id, local).is_ok());

        assert_data_tree!(@executor exec, inspect, root: {
            rfcomm_server: {
                peers: {
                    "0000000000000001": contains {
                        connected: "Connected",
                    },
                },
                advertised_channels: contains {},
            }
        });
    }
}
