// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bt_avctp::{AvcCommandResponse, AvcCommandType, AvcPeer, AvcResponseType, AvctpPeer};
use derivative::Derivative;
use fidl_fuchsia_bluetooth as fidl_bt;
use fidl_fuchsia_bluetooth_bredr as bredr;
use fuchsia_async as fasync;
use fuchsia_bluetooth::profile::Psm;
use fuchsia_bluetooth::types::{Channel, PeerId};
use fuchsia_inspect::Property;
use fuchsia_inspect_derive::{AttachError, Inspect};
use fuchsia_sync::RwLock;
use futures::Future;
use futures::channel::mpsc;
use futures::stream::StreamExt;
use log::{debug, info, trace, warn};
use packet_encoding::{Decodable, Encodable};
use std::collections::{HashMap, HashSet};
use std::mem::{Discriminant, discriminant};
use std::num::NonZeroU16;
use std::sync::Arc;

mod controller;
mod handlers;
mod inspect;
mod tasks;

use crate::metrics::MetricsNode;
use crate::packets::*;
use crate::peer_manager::TargetDelegate;
use crate::types::{PeerError as Error, StateChangeListener};
use fuchsia_bluetooth::profile::avrcp::AvrcpService;

pub use controller::{Controller, ControllerEvent};
pub use handlers::ControlChannelHandler;
pub use handlers::browse_channel::BrowseChannelHandler;
use inspect::RemotePeerInspect;

/// The minimum amount of time to wait before establishing an AVCTP connection.
/// This is used during connection establishment when both devices attempt to establish
/// a connection at the same time.
/// See AVRCP 1.6.2, Section 4.1.1 for more details.
const MIN_CONNECTION_EST_TIME: zx::MonotonicDuration = zx::MonotonicDuration::from_millis(100);

/// The maximum amount of time to wait before establishing an AVCTP connection.
/// This is used during connection establishment when both devices attempt to establish
/// a connection at the same time.
/// See AVRCP 1.6.2, Section 4.1.1 for more details.
const MAX_CONNECTION_EST_TIME: zx::MonotonicDuration = zx::MonotonicDuration::from_millis(1000);

/// Arbitrary threshold amount of time used to determine if two connections were
/// established at "the same time".
/// This was chosen in between the `MIN_CONNECTION_EST_TIME` and `MAX_CONNECTION_EST_TIME`
/// to account for radio delay and other factors that impede connection establishment.
const CONNECTION_THRESHOLD: zx::MonotonicDuration = zx::MonotonicDuration::from_millis(750);

#[derive(PartialEq)]
pub enum PeerChannelState<T> {
    Connected(Arc<T>),
    Connecting,
    Disconnected,
}

impl<T> std::fmt::Debug for PeerChannelState<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connected(_) => write!(f, "Connected"),
            Self::Connecting => write!(f, "Connecting"),
            Self::Disconnected => write!(f, "Disconnected"),
        }
    }
}

pub struct PeerChannel<T> {
    /// The state of this channel.
    state: PeerChannelState<T>,
    inspect: fuchsia_inspect::StringProperty,
}

impl<T> std::fmt::Debug for PeerChannel<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerChannel").field("state", &self.state).finish()
    }
}

impl<T> PeerChannel<T> {
    pub fn connected(&mut self, channel: Arc<T>) {
        self.state = PeerChannelState::Connected(channel);
        self.inspect.set("Connected");
    }

    pub fn connecting(&mut self) {
        self.state = PeerChannelState::Connecting;
        self.inspect.set("Connecting");
    }

    pub fn is_connecting(&self) -> bool {
        matches!(self.state, PeerChannelState::Connecting)
    }

    pub fn disconnect(&mut self) {
        self.state = PeerChannelState::Disconnected;
        self.inspect.set("Disconnected");
    }

    pub fn state(&self) -> &PeerChannelState<T> {
        &self.state
    }

    pub fn connection(&self) -> Option<&Arc<T>> {
        match &self.state {
            PeerChannelState::Connected(t) => Some(t),
            _ => None,
        }
    }
}

impl<T> Default for PeerChannel<T> {
    fn default() -> Self {
        Self {
            state: PeerChannelState::Disconnected,
            inspect: fuchsia_inspect::StringProperty::default(),
        }
    }
}

impl<T> Inspect for &mut PeerChannel<T> {
    fn iattach(
        self,
        parent: &fuchsia_inspect::Node,
        name: impl AsRef<str>,
    ) -> Result<(), AttachError> {
        self.inspect = parent.create_string(name.as_ref(), "Disconnected");
        Ok(())
    }
}

#[derive(Copy, Clone, Debug)]
pub enum AVCTPConnectionType {
    Control,
    Browse,
}

impl AVCTPConnectionType {
    pub fn psm(
        &self,
        controller_desc: &Option<AvrcpService>,
        target_desc: &Option<AvrcpService>,
    ) -> Psm {
        match self {
            AVCTPConnectionType::Control => match (target_desc, controller_desc) {
                (Some(AvrcpService::Target { psm, .. }), None) => *psm,
                (None, Some(AvrcpService::Controller { psm, .. })) => *psm,
                _ => {
                    info!("PSM for undiscovered peer, defaulting to PSM_AVCTP");
                    Psm::AVCTP
                }
            },
            AVCTPConnectionType::Browse => Psm::AVCTP_BROWSE,
        }
    }

    /// Get the the correct L2cap ChannelParameters for this connection type.
    /// (basic for control, enhanced retransmission for browsing)
    pub fn parameters(&self) -> fidl_bt::ChannelParameters {
        // TODO(https://fxbug.dev/42052066): set minimum MTU to 335.
        match self {
            AVCTPConnectionType::Control => fidl_bt::ChannelParameters::default(),
            AVCTPConnectionType::Browse => fidl_bt::ChannelParameters {
                channel_mode: Some(fidl_bt::ChannelMode::EnhancedRetransmission),
                ..Default::default()
            },
        }
    }

    // Return the L2CAP channel parameters appropriate for this connection, given the discovered
    // descriptors.
    pub fn connect_parameters(
        &self,
        controller_desc: &Option<AvrcpService>,
        target_desc: &Option<AvrcpService>,
    ) -> bredr::ConnectParameters {
        bredr::ConnectParameters::L2cap(bredr::L2capParameters {
            psm: Some(self.psm(controller_desc, target_desc).into()),
            parameters: Some(self.parameters()),
            ..Default::default()
        })
    }
}

/// Represents a browsable player where browse channel commands are routed to.
#[allow(unused)]
#[derive(Clone, Debug)]
pub struct BrowsablePlayer {
    player_id: u16,
    // If the browsable player is not database aware, uid_counter will be none.
    uid_counter: Option<NonZeroU16>,
    // Number of items in the current folder.
    num_items: u32,
    sub_folders: Vec<String>,
}

impl BrowsablePlayer {
    fn new(player_id: u16, params: SetBrowsedPlayerResponseParams) -> Self {
        BrowsablePlayer {
            player_id,
            uid_counter: params.uid_counter().try_into().ok(),
            num_items: params.num_items(),
            sub_folders: params.folder_names(),
        }
    }

    fn uid_counter(&self) -> u16 {
        self.uid_counter.map_or(0, Into::into)
    }
}

/// Internal object to manage a remote peer
#[derive(Derivative)]
#[derivative(Debug)]
struct RemotePeer {
    peer_id: PeerId,

    /// Contains the remote peer's target profile.
    target_descriptor: Option<AvrcpService>,

    /// Contains the remote peer's controller profile.
    controller_descriptor: Option<AvrcpService>,

    /// Control channel to the remote device.
    control_channel: PeerChannel<AvcPeer>,

    /// Browse channel to the remote device.
    browse_channel: PeerChannel<AvctpPeer>,

    /// Profile service. Used by RemotePeer to make outgoing L2CAP connections.
    profile_proxy: bredr::ProfileProxy,

    /// All stream listeners obtained by any `Controller`s around this peer that are listening for
    /// connection events from this peer.
    connection_listeners: Vec<mpsc::UnboundedSender<(AVCTPConnectionType, bool)>>,

    /// All stream listeners obtained by any `Controller`s around this peer that are listening for
    /// events from this peer.
    controller_listeners: Vec<mpsc::Sender<ControllerEvent>>,

    /// Processes commands received as AVRCP target and holds state for continuations and requested
    /// notifications for the control channel.
    control_command_handler: ControlChannelHandler,

    /// Processes commands received as AVRCP target over the browse channel.
    browse_command_handler: BrowseChannelHandler,

    /// Used to signal state changes and to notify and wake the state change observer currently
    /// processing this peer.
    state_change_listener: StateChangeListener,

    /// Set true to let the state watcher know that it should attempt to make
    /// outgoing l2cap connection to the peer for control channel. Set to false
    /// after a failed connection attempt so that we don't attempt to connect
    /// again immediately.
    attempt_control_connection: bool,

    /// Set true to let the state watcher know that it should attempt to make
    /// outgoing l2cap browse connection to the peer for browse channel. Set
    /// to false after a failed connection attempt so that we don't attempt to
    /// connect again immediately.
    attempt_browse_connection: bool,

    /// Set true to let the state watcher know that any outstanding `control_channel` processing
    /// tasks should be canceled and state cleaned up. Set to false after successfully canceling
    /// the tasks.
    cancel_control_task: bool,

    /// Set true to let the state watcher know that any outstanding `browse_channel` processing
    /// tasks should be canceled and state cleaned up. Set to false after successfully canceling
    /// the tasks.
    cancel_browse_task: bool,

    /// The timestamp of the last known control connection. Used to resolve simultaneous control
    /// channel connections.
    last_control_connected_time: Option<fasync::MonotonicInstant>,

    /// The timestamp of the last known browse connection. Used to resolve simultaneous browse
    /// channel connections.
    last_browse_connected_time: Option<fasync::MonotonicInstant>,

    /// Most recent notification values from the peer. Used to notify new controller listeners to
    /// the current state of the peer.
    notification_cache: HashMap<Discriminant<ControllerEvent>, ControllerEvent>,

    // Information about the browsable player that is currently set, if any.
    browsable_player: Option<BrowsablePlayer>,

    // Available players.
    available_players: HashMap<u16, MediaPlayerItem>,

    /// The inspect node for this peer.
    #[derivative(Debug = "ignore")]
    inspect: RemotePeerInspect,
}

impl Inspect for &mut RemotePeer {
    fn iattach(
        self,
        parent: &fuchsia_inspect::Node,
        name: impl AsRef<str>,
    ) -> Result<(), AttachError> {
        self.inspect.iattach(parent, name.as_ref())?;
        self.control_channel.iattach(&self.inspect.node(), "control")?;
        self.browse_channel.iattach(&self.inspect.node(), "browse")?;
        Ok(())
    }
}

impl RemotePeer {
    fn new(
        peer_id: PeerId,
        target_delegate: Arc<TargetDelegate>,
        profile_proxy: bredr::ProfileProxy,
    ) -> RemotePeer {
        Self {
            peer_id: peer_id.clone(),
            target_descriptor: None,
            controller_descriptor: None,
            control_channel: PeerChannel::default(),
            browse_channel: PeerChannel::default(),
            connection_listeners: Vec::new(),
            controller_listeners: Vec::new(),
            profile_proxy,
            control_command_handler: ControlChannelHandler::new(target_delegate.clone()),
            browse_command_handler: BrowseChannelHandler::new(target_delegate),
            state_change_listener: StateChangeListener::new(),
            attempt_control_connection: false,
            attempt_browse_connection: false,
            cancel_control_task: false,
            cancel_browse_task: false,
            last_control_connected_time: None,
            last_browse_connected_time: None,
            notification_cache: HashMap::new(),
            browsable_player: None,
            available_players: HashMap::new(),
            inspect: RemotePeerInspect::new(peer_id),
        }
    }

    fn id(&self) -> PeerId {
        self.peer_id
    }

    fn inspect(&self) -> &RemotePeerInspect {
        &self.inspect
    }

    /// Returns true if this peer is considered discovered, i.e. has service descriptors for
    /// either controller or target.
    fn discovered(&self) -> bool {
        self.target_descriptor.is_some() || self.controller_descriptor.is_some()
    }

    /// Caches the current value of this controller notification event for future controller event
    /// listeners and forwards the event to current controller listeners queues.
    fn handle_new_controller_notification_event(&mut self, event: ControllerEvent) {
        let _ = self.notification_cache.insert(discriminant(&event), event.clone());

        // remove all the dead listeners from the list.
        self.controller_listeners.retain(|i| !i.is_closed());
        for sender in self.controller_listeners.iter_mut() {
            if let Err(send_error) = sender.try_send(event.clone()) {
                warn!(
                    "Error sending event to listener for peer {}: {:?}",
                    self.peer_id, send_error
                );
            }
        }
    }

    /// Forwards the connection event to current listeners queues.
    fn handle_new_connection_event(&mut self, type_: AVCTPConnectionType, connected: bool) {
        // Remove all the dead listeners from the list.
        self.connection_listeners.retain(|i| !i.is_closed());
        for sender in self.connection_listeners.iter_mut() {
            if let Err(send_error) = sender.unbounded_send((type_, connected)) {
                warn!(
                    "Error sending event to listener for peer {}: {:?}",
                    self.peer_id, send_error
                );
            }
        }
    }

    fn control_connected(&self) -> bool {
        self.control_channel.connection().is_some()
    }

    fn browse_connected(&self) -> bool {
        self.browse_channel.connection().is_some()
    }

    /// Reset control channel.
    fn reset_control_connection(&mut self, attempt_reconnection: bool) {
        info!(
            "Resetting control connection peer {}, will {}attempt to reconnect",
            self.peer_id,
            if attempt_reconnection { "" } else { "not " }
        );
        self.notification_cache.clear();
        self.control_command_handler.reset();
        self.cancel_control_task = true;
        self.control_channel.disconnect();
        self.last_control_connected_time = None;
        self.handle_new_connection_event(AVCTPConnectionType::Control, false);

        self.attempt_control_connection = attempt_reconnection;
        self.wake_state_watcher();
    }

    /// Reset browse channel.
    fn reset_browse_connection(&mut self, attempt_reconnection: bool) {
        info!(
            "Resetting browse connection peer {}, will {}attempt to reconnect",
            self.peer_id,
            if attempt_reconnection { "" } else { "not " }
        );
        self.browse_command_handler.reset();
        self.cancel_browse_task = true;
        self.browse_channel.disconnect();
        self.last_browse_connected_time = None;
        self.handle_new_connection_event(AVCTPConnectionType::Browse, false);

        self.attempt_browse_connection = attempt_reconnection;
        self.wake_state_watcher();
    }

    /// Reset both browse and control channels.
    /// `attempt_reconnection` will cause state_watcher to attempt to make an outgoing connection when
    /// woken.
    fn reset_connections(&mut self, attempt_reconnection: bool) {
        self.reset_browse_connection(attempt_reconnection);
        self.reset_control_connection(attempt_reconnection);
    }

    fn is_connecting(&self, conn_type: AVCTPConnectionType) -> bool {
        match conn_type {
            AVCTPConnectionType::Control => self.control_channel.is_connecting(),
            AVCTPConnectionType::Browse => self.browse_channel.is_connecting(),
        }
    }

    /// Method for initiating outbound connection request.
    /// Immediately resolves to Ok(Err(ErrorCode::BadState)) if the connection type is not ready to
    /// connect.
    pub fn connect(
        &mut self,
        conn_type: AVCTPConnectionType,
    ) -> impl Future<
        Output = Result<Result<bredr::Channel, fidl_fuchsia_bluetooth::ErrorCode>, fidl::Error>,
    > + use<> {
        let connect_parameters = self.is_connecting(conn_type).then(|| {
            conn_type.connect_parameters(&self.controller_descriptor, &self.target_descriptor)
        });

        let peer_id = self.peer_id.into();
        let proxy = self.profile_proxy.clone();
        async move {
            let Some(params) = connect_parameters else {
                return Ok(Err(fidl_fuchsia_bluetooth::ErrorCode::BadState));
            };
            proxy.connect(&peer_id, &params).await
        }
    }

    /// Called when outgoing L2CAP connection was successfully established.
    pub fn connected(&mut self, conn_type: AVCTPConnectionType, channel: Channel) {
        // Set up the appropriate connections. If an incoming l2cap connection
        // was made while we were making an outgoing one, reset the channel(s).
        match conn_type {
            AVCTPConnectionType::Control => {
                if self.control_channel.is_connecting() {
                    let peer = AvcPeer::new(channel);
                    return self.set_control_connection(peer);
                }
                self.reset_connections(true)
            }
            AVCTPConnectionType::Browse => {
                if self.browse_channel.is_connecting() {
                    let peer = AvctpPeer::new(channel);
                    return self.set_browse_connection(peer);
                }
                self.reset_browse_connection(true)
            }
        }
    }

    /// Called when outgoing L2CAP connection was not established successfully.
    /// When always_reset is true, connections are reset unconditionally.
    /// Otherwise, they are only reset if the channel was in `is_connecting`
    /// state.
    pub fn connect_failed(&mut self, conn_type: AVCTPConnectionType, always_reset: bool) {
        self.inspect().metrics().connection_error();
        match conn_type {
            AVCTPConnectionType::Control => {
                if always_reset || self.control_channel.is_connecting() {
                    self.reset_connections(false)
                }
            }
            AVCTPConnectionType::Browse => {
                if always_reset || self.browse_channel.is_connecting() {
                    self.reset_browse_connection(false)
                }
            }
        }
    }

    fn control_connection(&mut self) -> Result<Arc<AvcPeer>, Error> {
        // if we are not connected, try to reconnect the next time we want to send a command.
        if !self.control_connected() {
            self.attempt_control_connection = true;
            self.wake_state_watcher();
        }

        match self.control_channel.connection() {
            Some(peer) => Ok(peer.clone()),
            None => Err(Error::RemoteNotFound),
        }
    }

    fn set_control_connection(&mut self, peer: AvcPeer) {
        let current_time = fasync::MonotonicInstant::now();
        debug!("Set control connection for {} at: {}", self.peer_id, current_time.into_nanos());

        // If the current connection establishment is within a threshold amount of time from the
        // most recent connection establishment, both connections should be dropped, and should
        // wait a random amount of time before re-establishment.
        if let Some(previous_time) = self.last_control_connected_time.take() {
            let diff = (current_time - previous_time).into_nanos().abs();
            if diff < CONNECTION_THRESHOLD.into_nanos() {
                debug!(
                    "Collision in control connection establishment for {}. Time diff: {}",
                    self.peer_id, diff
                );
                self.inspect.metrics().control_collision();
                self.reset_connections(true);
                return;
            }
        }

        if self.browse_connected() {
            // Browse channel was already established.
            // This indicates that this is a new control channel connection.
            // Reset pre-existing channels before setting a new control channel.
            self.reset_connections(false);
        } else if self.control_connected() {
            // Just reset existing control connection.
            self.reset_control_connection(false);
        }

        info!("{} connected new control channel", self.peer_id);
        self.last_control_connected_time = Some(current_time);
        self.control_channel.connected(Arc::new(peer));
        self.handle_new_connection_event(AVCTPConnectionType::Control, true);

        // Since control connection was newly established, allow browse
        // connection to be attempted.
        self.attempt_browse_connection = true;
        self.inspect.record_connected(current_time);
        self.wake_state_watcher();
    }

    fn browse_connection(&mut self) -> Result<Arc<AvctpPeer>, Error> {
        // If we are not connected, try to reconnect the next time we want to send a command.
        if !self.browse_connected() {
            if !self.control_connected() {
                self.attempt_control_connection = true;
                self.attempt_browse_connection = true;
            }
            self.wake_state_watcher();
        }

        match self.browse_channel.connection() {
            Some(peer) => Ok(peer.clone()),
            None => Err(Error::RemoteNotFound),
        }
    }

    fn set_browse_connection(&mut self, peer: AvctpPeer) {
        let current_time = fasync::MonotonicInstant::now();
        debug!("Set browse connection for {} at: {}", self.peer_id, current_time.into_nanos());

        // If the current connection establishment is within a threshold amount of time from the
        // most recent connection establishment, both connections should be dropped, and should
        // wait a random amount of time before re-establishment.
        if let Some(previous_time) = self.last_browse_connected_time.take() {
            let diff = (current_time - previous_time).into_nanos().abs();
            if diff < CONNECTION_THRESHOLD.into_nanos() {
                debug!(
                    "Collision in browse connection establishment for {}. Time diff: {}",
                    self.peer_id, diff
                );
                self.inspect.metrics().browse_collision();
                self.reset_browse_connection(true);
                return;
            }
        }

        // Browse connection should only be accepted if control connection
        // was already established.
        if !self.control_connected() {
            self.reset_connections(true);
            return;
        }

        if self.browse_connected() {
            // Just reset existing browse connection.
            self.reset_browse_connection(false);
        }

        info!("{} connected new browse channel", self.peer_id);
        self.last_browse_connected_time = Some(current_time);
        self.browse_channel.connected(Arc::new(peer));
        self.handle_new_connection_event(AVCTPConnectionType::Browse, true);

        self.inspect.metrics().browse_connection();
        self.wake_state_watcher();
    }

    fn set_target_descriptor(&mut self, service: AvrcpService) {
        debug!("Set target descriptor for {}", self.peer_id);
        self.target_descriptor = Some(service);
        // Record inspect target features.
        self.inspect.record_target_features(service);
        self.wake_state_watcher();
    }

    fn set_controller_descriptor(&mut self, service: AvrcpService) {
        debug!("Set controller descriptor for {}", self.peer_id);
        self.controller_descriptor = Some(service);
        self.control_command_handler.set_controller_descriptor(service);
        // Record inspect controller features.
        self.inspect.record_controller_features(service);
        self.wake_state_watcher();
    }

    fn supports_target_browsing(&self) -> bool {
        self.target_descriptor.is_some_and(|desc| desc.supports_browsing())
    }

    fn supports_controller_browsing(&self) -> bool {
        self.controller_descriptor.is_some_and(|desc| desc.supports_browsing())
    }

    fn set_metrics_node(&mut self, node: MetricsNode) {
        self.inspect.set_metrics_node(node);
    }

    fn wake_state_watcher(&self) {
        debug!("Waking state watcher for {}", self.peer_id);
        self.state_change_listener.state_changed();
    }

    fn update_available_players(&mut self, players: &[MediaPlayerItem]) {
        self.available_players.clear();
        debug!(peer_id:% = self.peer_id; "Available players updated: {players:?}");
        players.iter().for_each(|p| {
            let _ = self.available_players.insert(p.player_id(), p.clone());
        });
    }

    /// Finds the player ID of the addressed player or the player ID of one of the available players
    /// with highest browse level. The player ID is only returned if browsing is supported.
    fn get_candidate_browse_player(&self) -> Option<u16> {
        // let mut players: Vec<&MediaPlayerItem> = Vec::new();
        self.notification_cache
            .get(&discriminant(&ControllerEvent::AddressedPlayerChanged(0)))
            .map_or_else(
                || {
                    let mut players: Vec<&MediaPlayerItem> =
                        self.available_players.values().collect();
                    players.sort_unstable_by(|a, b| b.browse_level().cmp(&a.browse_level()));
                    players.first().map(|p| *p)
                },
                |e| match e {
                    ControllerEvent::AddressedPlayerChanged(player_id) => {
                        self.available_players.get(player_id)
                    }
                    _ => panic!("Shouldn't reach here"),
                },
            )
            .and_then(|p| {
                if p.browse_level() > BrowseLevel::NotBrowsable {
                    Some(p.player_id())
                } else {
                    None
                }
            })
    }
}

impl Drop for RemotePeer {
    fn drop(&mut self) {
        // Stop any stream processors that are currently running on this remote peer.
        self.state_change_listener.terminate();
    }
}

async fn send_vendor_dependent_command_internal(
    peer: Arc<RwLock<RemotePeer>>,
    command: &(impl VendorDependentPdu + PacketEncodable + VendorCommand),
) -> Result<Vec<u8>, Error> {
    let avc_peer = peer.write().control_connection()?;
    let mut buf = vec![];
    let packet = command.encode_packet().expect("unable to encode packet");
    let mut stream = avc_peer.send_vendor_dependent_command(command.command_type(), &packet[..])?;

    loop {
        let response = loop {
            let response = stream.next().await.ok_or(Error::CommandFailed)??;
            trace!("vendor response {:?}", response);
            match (response.response_type(), command.command_type()) {
                (AvcResponseType::Interim, _) => continue,
                (AvcResponseType::NotImplemented, _) => return Err(Error::CommandNotSupported),
                (AvcResponseType::Rejected, _) => return Err(Error::CommandFailed),
                (AvcResponseType::InTransition, _) => return Err(Error::UnexpectedResponse),
                (AvcResponseType::Changed, _) => return Err(Error::UnexpectedResponse),
                (AvcResponseType::Accepted, AvcCommandType::Control) => break response.1,
                (AvcResponseType::ImplementedStable, AvcCommandType::Status) => break response.1,
                _ => return Err(Error::UnexpectedResponse),
            }
        };

        match VendorDependentPreamble::decode(&response[..]) {
            Ok(preamble) => {
                buf.extend_from_slice(&response[preamble.encoded_len()..]);
                match preamble.packet_type() {
                    PacketType::Single | PacketType::Stop => {
                        break;
                    }
                    // Still more to decode. Queue up a continuation call.
                    _ => {}
                }
            }
            Err(e) => {
                warn!("Unable to parse vendor dependent preamble: {:?}", e);
                return Err(Error::PacketError(e));
            }
        };

        let command = RequestContinuingResponseCommand::new(&command.pdu_id());
        let packet = command.encode_packet().expect("unable to encode packet");

        stream = avc_peer.send_vendor_dependent_command(command.command_type(), &packet[..])?;
    }
    Ok(buf)
}

async fn send_browse_command_internal(
    peer: Arc<RwLock<RemotePeer>>,
    pdu_id: u8,
    payload: &[u8],
) -> Result<Vec<u8>, Error> {
    let preamble = BrowsePreamble::new(pdu_id, payload.to_vec());
    let avctp_peer = peer.write().browse_connection()?;

    let mut buf = vec![0; preamble.encoded_len()];
    let _ = preamble.encode(&mut buf[..])?;
    let mut stream = avctp_peer.send_command(&buf[..])?;

    // Wait for result.
    let result = stream.next().await.ok_or(Error::CommandFailed)?;
    let response = result.map_err(|e| Error::AvctpError(e))?;
    trace!("AVRCP response {:?}", response);

    match BrowsePreamble::decode(response.body()) {
        Ok(preamble) => {
            if preamble.pdu_id != pdu_id {
                return Err(Error::UnexpectedResponse);
            }
            Ok(preamble.body)
        }
        Err(e) => {
            warn!("Unable to parse browse preamble: {:?}", e);
            Err(Error::PacketError(e))
        }
    }
}

/// Retrieve the events supported by the peer by issuing a GetCapabilities command.
async fn get_supported_events_internal(
    peer: Arc<RwLock<RemotePeer>>,
) -> Result<HashSet<NotificationEventId>, Error> {
    let cmd = GetCapabilitiesCommand::new(GetCapabilitiesCapabilityId::EventsId);
    debug!("Getting supported events: {:?}", cmd);
    let buf = send_vendor_dependent_command_internal(peer.clone(), &cmd).await?;
    let capabilities = GetCapabilitiesResponse::decode(&buf[..])?;
    let mut event_ids = HashSet::new();
    for event_id in capabilities.event_ids() {
        let _ = event_ids.insert(NotificationEventId::try_from(event_id)?);
    }
    Ok(event_ids)
}

#[derive(Debug, Clone)]
pub struct RemotePeerHandle {
    peer: Arc<RwLock<RemotePeer>>,
}

impl Inspect for &mut RemotePeerHandle {
    fn iattach(
        self,
        parent: &fuchsia_inspect::Node,
        name: impl AsRef<str>,
    ) -> Result<(), AttachError> {
        self.peer.write().iattach(parent, name.as_ref())
    }
}

impl RemotePeerHandle {
    /// Create a remote peer and spawns the state watcher tasks around it.
    /// Should only be called by peer manager.
    pub fn spawn_peer(
        peer_id: PeerId,
        target_delegate: Arc<TargetDelegate>,
        profile_proxy: bredr::ProfileProxy,
    ) -> RemotePeerHandle {
        let remote_peer =
            Arc::new(RwLock::new(RemotePeer::new(peer_id, target_delegate, profile_proxy)));

        fasync::Task::spawn(tasks::state_watcher(remote_peer.clone())).detach();

        RemotePeerHandle { peer: remote_peer }
    }

    pub fn set_control_connection(&self, peer: AvcPeer) {
        self.peer.write().set_control_connection(peer);
    }

    pub fn set_browse_connection(&self, peer: AvctpPeer) {
        self.peer.write().set_browse_connection(peer);
    }

    pub fn set_target_descriptor(&self, service: AvrcpService) {
        self.peer.write().set_target_descriptor(service);
    }

    pub fn set_controller_descriptor(&self, service: AvrcpService) {
        self.peer.write().set_controller_descriptor(service);
    }

    pub fn set_metrics_node(&self, node: MetricsNode) {
        self.peer.write().set_metrics_node(node);
    }

    pub fn is_control_connected(&self) -> bool {
        self.peer.read().control_connected()
    }

    pub fn is_browse_connected(&self) -> bool {
        self.peer.read().browse_connected()
    }

    pub fn get_browsable_player(&self) -> Option<BrowsablePlayer> {
        self.peer.read().browsable_player.clone()
    }

    pub fn set_browsable_player(&self, player: Option<BrowsablePlayer>) {
        let mut lock = self.peer.write();
        let peer_id = lock.peer_id;
        info!(peer_id:%; "Changing browsable player to {:?}", player.as_ref().map(|p| p.player_id));
        lock.browsable_player = player;
    }

    /// Sends a single passthrough keycode over the control channel.
    pub fn send_avc_passthrough<'a>(
        &self,
        payload: &'a [u8; 2],
    ) -> impl Future<Output = Result<(), Error>> + 'a {
        let peer_id = self.peer.read().peer_id.clone();
        let avc_peer_result = self.peer.write().control_connection();
        async move {
            let avc_peer = avc_peer_result?;
            let response = avc_peer.send_avc_passthrough_command(payload).await;
            match response {
                Ok(AvcCommandResponse(AvcResponseType::Accepted, _)) => Ok(()),
                Ok(AvcCommandResponse(AvcResponseType::Rejected, _)) => {
                    info!("Command rejected for {:?}: {:?}", peer_id, response);
                    Err(Error::CommandNotSupported)
                }
                Err(e) => {
                    warn!("Error sending avc command to {:?}: {:?}", peer_id, e);
                    Err(Error::CommandFailed)
                }
                _ => {
                    warn!("Unhandled response for {:?}: {:?}", peer_id, response);
                    Err(Error::CommandFailed)
                }
            }
        }
    }

    /// Send a generic vendor dependent command and returns the result as a future.
    /// This method encodes the `command` packet, awaits and decodes all responses, will issue
    /// continuation commands for incomplete responses (eg "get_element_attributes" command), and
    /// will return a result of the decoded packet or an error for any non stable response received
    pub fn send_vendor_dependent_command<'a>(
        &self,
        command: &'a (impl PacketEncodable + VendorCommand),
    ) -> impl Future<Output = Result<Vec<u8>, Error>> + 'a {
        send_vendor_dependent_command_internal(self.peer.clone(), command)
    }

    /// Send AVRCP specific browsing commands as a AVCTP message. This method
    /// first encodes the specific AVRCP command message as a browse preamble
    /// message, which then gets encoded as part of a non-fragmented AVCTP
    /// message. Once it receives a AVCTP response message, it will decode it
    /// into a browse preamble and will return its parameters, so that the
    /// upstream can further decode the message into a specific AVRCP response
    /// message.
    pub fn send_browse_command<'a>(
        &'a self,
        pdu_id: u8,
        payload: &'a [u8],
    ) -> impl Future<Output = Result<Vec<u8>, Error>> + 'a {
        send_browse_command_internal(self.peer.clone(), pdu_id, payload)
    }

    /// Retrieve the events supported by the peer by issuing a GetCapabilities command.
    pub fn get_supported_events(
        &self,
    ) -> impl Future<Output = Result<HashSet<NotificationEventId>, Error>> + '_ {
        get_supported_events_internal(self.peer.clone())
    }

    /// Adds new controller listener to this remote peer. The controller listener is immediately
    /// sent the current state of all notification values.
    pub fn add_control_listener(&self, mut sender: mpsc::Sender<ControllerEvent>) {
        let mut peer_guard = self.peer.write();
        for (_, event) in &peer_guard.notification_cache {
            if let Err(send_error) = sender.try_send(event.clone()) {
                warn!(
                    "Error sending cached event to listener for {}: {:?}",
                    peer_guard.peer_id, send_error
                );
            }
        }
        peer_guard.controller_listeners.push(sender)
    }

    /// Adds new connection listener to this remote peer.
    pub fn add_connection_listener(
        &self,
        sender: mpsc::UnboundedSender<(AVCTPConnectionType, bool)>,
    ) {
        self.peer.write().connection_listeners.push(sender)
    }

    /// Used by peer manager to get
    pub fn get_controller(&self) -> Controller {
        Controller::new(self.clone())
    }

    /// Sets the `attempt_control_connection` flag to true and wakes up the state watcher to
    /// initiate a connection request if not already connected.
    pub fn request_control_connection(&self) {
        let mut peer = self.peer.write();
        peer.attempt_control_connection = true;
        peer.wake_state_watcher();
    }

    /// Sets the `attempt_browse_connection` flag to true and wakes up the state watcher to
    /// initiate a browse connection request if not already connected.
    pub fn request_browse_connection(&self) {
        if !self.is_control_connected() {
            self.peer.write().attempt_control_connection = true;
        }
        let mut lock = self.peer.write();
        lock.attempt_browse_connection = true;
        lock.wake_state_watcher();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::packets::Error as PacketError;
    use assert_matches::assert_matches;
    use async_utils::PollExt;
    use bt_avctp::{AvcCommand, AvcCommandStream, AvctpCommand, AvctpCommandStream};
    use bt_channel_test_support::{Transport, create_test_channels};
    use fuchsia_bluetooth::profile::avrcp::{
        AvrcpControllerFeatures, AvrcpProtocolVersion, AvrcpTargetFeatures,
    };
    use futures::task::Poll;
    use futures::{SinkExt, TryStreamExt};
    use std::pin::pin;
    use test_case::test_case;

    use diagnostics_assertions::assert_data_tree;
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_bluetooth::ErrorCode;
    use fidl_fuchsia_bluetooth_bredr::{
        ConnectParameters, L2capParameters, ProfileMarker, ProfileRequest, ProfileRequestStream,
    };
    use fuchsia_async::{self as fasync, DurationExt};
    use fuchsia_inspect_derive::WithInspect;

    fn setup_remote_peer(
        id: PeerId,
    ) -> (RemotePeerHandle, Arc<TargetDelegate>, ProfileRequestStream) {
        let (profile_proxy, profile_requests) = create_proxy_and_stream::<ProfileMarker>();
        let target_delegate = Arc::new(TargetDelegate::new());
        let peer_handle = RemotePeerHandle::spawn_peer(id, target_delegate.clone(), profile_proxy);

        (peer_handle, target_delegate, profile_requests)
    }

    #[track_caller]
    fn assert_write_initiated(res: std::task::Poll<Result<(), zx::Status>>) {
        match res {
            std::task::Poll::Ready(Err(e)) => panic!("Write failed: {:?}", e),
            _ => {}
        }
    }

    #[track_caller]
    fn expect_channel_writable(exec: &mut fasync::TestExecutor, channel: &mut Channel) {
        // Should be able to send data over the channel.
        let mut write_fut = channel.send(vec![0; 1]);
        assert_write_initiated(exec.run_until_stalled(&mut write_fut));
    }

    #[track_caller]
    fn expect_channel_closed(exec: &mut fasync::TestExecutor, channel: &mut Channel) {
        let mut write_fut = channel.send(vec![0; 1]);
        let result = exec.run_until_stalled(&mut write_fut).expect("should be ready");
        assert!(result.is_err());
    }

    // Helper function to set incoming control connection.
    fn set_incoming_control_connection(
        peer_handle: &RemotePeerHandle,
        transport: Transport,
    ) -> Channel {
        let (local, remote) = create_test_channels(transport);
        let control_channel = AvcPeer::new(local);
        peer_handle.set_control_connection(control_channel);

        remote
    }

    // Helper function to set incoming browse connection.
    fn set_incoming_browse_connection(
        peer_handle: &RemotePeerHandle,
        transport: Transport,
    ) -> Channel {
        let (local, remote) = create_test_channels(transport);
        let browse_channel = AvctpPeer::new(local);
        peer_handle.set_browse_connection(browse_channel);

        remote
    }

    // Check that the remote will attempt to connect to a peer if we have a profile.
    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn trigger_connection_test(transport: Transport) {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(5_000000000));

        let id = PeerId(1);
        let (peer_handle, _target_delegate, mut profile_requests) = setup_remote_peer(id);

        // Set the descriptor to simulate service found for peer - while unusual, this peer
        // advertises a non-standard PSM.
        let peer_psm = Psm::new(37); // OTS PSM
        peer_handle.set_target_descriptor(AvrcpService::Target {
            features: AvrcpTargetFeatures::CATEGORY1,
            psm: peer_psm,
            protocol_version: AvrcpProtocolVersion(1, 6),
        });
        peer_handle.request_control_connection();

        assert!(!peer_handle.is_control_connected());

        let next_request_fut = profile_requests.next();
        let mut next_request_fut = pin!(next_request_fut);
        assert!(exec.run_until_stalled(&mut next_request_fut).is_pending());

        // Advance time by the maximum amount of time it would take to establish
        // a connection.
        exec.set_fake_time(MAX_CONNECTION_EST_TIME.after_now());
        let _ = exec.wake_expired_timers();

        // We should have requested a connection for control.
        let (channel, _remote) = create_test_channels(transport);
        match exec.run_until_stalled(&mut next_request_fut) {
            Poll::Ready(Some(Ok(ProfileRequest::Connect { responder, connection, .. }))) => {
                let channel = channel.try_into().unwrap();
                responder.send(Ok(channel)).expect("FIDL response should work");
                // The connect request should be for the PSM advertised by the remote peer.
                match connection {
                    ConnectParameters::L2cap(L2capParameters { psm: Some(v), .. }) => {
                        assert_eq!(v, u16::from(peer_psm));
                    }
                    x => panic!("Expected L2CAP parameters but got: {:?}", x),
                }
            }
            x => panic!("Expected Profile connection request to be ready, got {:?} instead.", x),
        };

        // run until stalled, the connection should be put in place.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        assert!(peer_handle.is_control_connected());

        // Advance time by the maximum amount of time it would take to establish
        // a connection.
        exec.set_fake_time(MAX_CONNECTION_EST_TIME.after_now());
        let _ = exec.wake_expired_timers();

        // Since peer does not support browsing, verify that outgoing
        // browsing connection was not initiated.
        let next_request_fut = profile_requests.next();
        let mut next_request_fut = pin!(next_request_fut);
        assert!(exec.run_until_stalled(&mut next_request_fut).is_pending());
        assert!(!peer_handle.is_browse_connected());
    }

    // Check that the remote will attempt to connect to a peer for both control
    // and browsing.
    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn trigger_connections_test(transport: Transport) {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(5_000000000));

        let id = PeerId(1);
        let (peer_handle, _target_delegate, mut profile_requests) = setup_remote_peer(id);

        // Set the descriptor to simulate service found for peer - while unusual, this peer
        // advertises a non-standard PSM.
        let peer_psm = Psm::new(23); // AVCTP PSM
        peer_handle.set_target_descriptor(AvrcpService::Target {
            features: AvrcpTargetFeatures::CATEGORY1 | AvrcpTargetFeatures::SUPPORTSBROWSING,
            psm: peer_psm,
            protocol_version: AvrcpProtocolVersion(1, 6),
        });
        peer_handle.request_control_connection();

        assert!(!peer_handle.is_control_connected());

        let next_request_fut = profile_requests.next();
        let mut next_request_fut = pin!(next_request_fut);
        assert!(exec.run_until_stalled(&mut next_request_fut).is_pending());

        // Advance time by the maximum amount of time it would take to establish
        // a connection.
        exec.set_fake_time(MAX_CONNECTION_EST_TIME.after_now());
        let _ = exec.wake_expired_timers();

        // We should have requested a connection for control.
        let (channel, _remote) = create_test_channels(transport);
        match exec.run_until_stalled(&mut next_request_fut) {
            Poll::Ready(Some(Ok(ProfileRequest::Connect { responder, .. }))) => {
                let channel = channel.try_into().unwrap();
                responder.send(Ok(channel)).expect("FIDL response should work");
            }
            x => panic!("Expected Profile connection request to be ready, got {:?} instead.", x),
        };

        // run until stalled, the connection should be put in place.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        assert!(peer_handle.is_control_connected());

        // Advance time by the maximum amount of time it would take to establish
        // a connection.
        exec.set_fake_time(MAX_CONNECTION_EST_TIME.after_now());
        let _ = exec.wake_expired_timers();

        // We should have requested a connection for browse.
        let (channel2, _remote2) = create_test_channels(transport);
        let next_request_fut = profile_requests.next();
        let mut next_request_fut = pin!(next_request_fut);
        match exec.run_until_stalled(&mut next_request_fut) {
            Poll::Ready(Some(Ok(ProfileRequest::Connect { responder, connection, .. }))) => {
                let channel = channel2.try_into().unwrap();
                responder.send(Ok(channel)).expect("FIDL response should work");
                // The connect request should be for the PSM advertised by the remote peer.
                match connection {
                    ConnectParameters::L2cap(L2capParameters {
                        psm: Some(v),
                        parameters: Some(params),
                        ..
                    }) => {
                        assert_eq!(v, u16::from(Psm::new(27))); // AVCTP_BROWSE
                        assert_eq!(
                            params.channel_mode.expect("channel mode should not be None"),
                            fidl_bt::ChannelMode::EnhancedRetransmission
                        );
                    }
                    x => panic!("Expected L2CAP parameters but got: {:?}", x),
                }
            }
            x => panic!("Expected Profile connection request to be ready, got {:?} instead.", x),
        };

        // run until stalled, the connection should be put in place.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        assert!(peer_handle.is_browse_connected());
    }

    /// Tests initial connection establishment to a peer.
    /// Tests peer reconnection correctly terminates the old processing task, including the
    /// underlying channel, and spawns a new task to handle incoming requests.
    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn test_peer_reconnection(transport: Transport) {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(5_000000000));

        let id = PeerId(123);
        let (peer_handle, _target_delegate, mut profile_requests) = setup_remote_peer(id);

        // Set the descriptor to simulate service found for peer.
        let peer_psm = Psm::new(bredr::PSM_AVCTP);
        peer_handle.set_target_descriptor(AvrcpService::Target {
            features: AvrcpTargetFeatures::CATEGORY1,
            psm: peer_psm,
            protocol_version: AvrcpProtocolVersion(1, 6),
        });
        peer_handle.request_control_connection();

        assert!(!peer_handle.is_control_connected());

        let next_request_fut = profile_requests.next();
        let mut next_request_fut = pin!(next_request_fut);
        assert!(exec.run_until_stalled(&mut next_request_fut).is_pending());

        // Advance time by the maximum amount of time it would take to establish
        // a connection.
        exec.set_fake_time(MAX_CONNECTION_EST_TIME.after_now());
        let _ = exec.wake_expired_timers();

        // Peer should have requested a connection.
        let (channel, mut remote) = create_test_channels(transport);
        match exec.run_until_stalled(&mut next_request_fut) {
            Poll::Ready(Some(Ok(ProfileRequest::Connect { responder, connection, .. }))) => {
                let channel = channel.try_into().unwrap();
                responder.send(Ok(channel)).expect("FIDL response should work");
                match connection {
                    ConnectParameters::L2cap(L2capParameters { psm: Some(v), .. }) => {
                        assert_eq!(v, u16::from(peer_psm));
                    }
                    x => panic!("Expected L2CAP parameters but got: {:?}", x),
                }
            }
            x => panic!("Expected Profile connection request to be ready, got {:?} instead.", x),
        };

        // Peer should be connected.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        assert!(peer_handle.is_control_connected());

        // Should be able to send data over the channel.
        expect_channel_writable(&mut exec, &mut remote);

        // Advance time by some arbitrary amount before peer decides to reconnect.
        exec.set_fake_time(zx::MonotonicDuration::from_seconds(5).after_now());
        let _ = exec.wake_expired_timers();

        // Peer reconnects with a new l2cap connection. Keep the old one alive to validate that it's
        // closed.
        let mut remote2 = set_incoming_control_connection(&peer_handle, transport);
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        assert!(peer_handle.is_control_connected());

        // Shouldn't be able to send data over the old channel.
        expect_channel_closed(&mut exec, &mut remote);

        // Should be able to send data over the new channel.
        expect_channel_writable(&mut exec, &mut remote2);
    }

    /// Tests that when inbound and outbound control connections are
    /// established at the same time, AVRCP drops both, and attempts to
    /// reconnect.
    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn test_simultaneous_control_connections(transport: Transport) {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(5_000000000));

        let id = PeerId(123);
        let (peer_handle, _target_delegate, mut profile_requests) = setup_remote_peer(id);

        // Set the descriptor to simulate service found for peer.
        peer_handle.set_target_descriptor(AvrcpService::Target {
            features: AvrcpTargetFeatures::CATEGORY1,
            psm: Psm::AVCTP,
            protocol_version: AvrcpProtocolVersion(1, 6),
        });
        peer_handle.request_control_connection();

        assert!(!peer_handle.is_control_connected());

        let next_request_fut = profile_requests.next();
        let mut next_request_fut = pin!(next_request_fut);
        assert!(exec.run_until_stalled(&mut next_request_fut).is_pending());

        // Advance time by the maximum amount of time it would take to establish a connection.
        exec.set_fake_time(MAX_CONNECTION_EST_TIME.after_now());
        let _ = exec.wake_expired_timers();

        // We expect to initiate an outbound connection through the profile server.
        let (channel, mut remote) = create_test_channels(transport);
        match exec.run_until_stalled(&mut next_request_fut) {
            Poll::Ready(Some(Ok(ProfileRequest::Connect { responder, .. }))) => {
                let channel = channel.try_into().unwrap();
                responder.send(Ok(channel)).expect("FIDL response should work");
            }
            x => panic!("Expected Profile connection request to be ready, got {:?} instead.", x),
        };

        // The inbound connection is accepted (since there were no previous connections).
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        assert!(peer_handle.is_control_connected());

        // Should be able to send data over the channel.
        expect_channel_writable(&mut exec, &mut remote);

        // Advance time by LESS than the CONNECTION_THRESHOLD amount.
        let advance_time = CONNECTION_THRESHOLD.into_nanos() - 100;
        exec.set_fake_time(zx::MonotonicDuration::from_nanos(advance_time).after_now());
        let _ = exec.wake_expired_timers();

        // Simulate inbound connection.
        let mut remote2 = set_incoming_control_connection(&peer_handle, transport);
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        assert!(!peer_handle.is_control_connected());

        // Both the inbound and outbound-initiated control channels should be
        // dropped. Sending data should not work.
        expect_channel_closed(&mut exec, &mut remote);
        expect_channel_closed(&mut exec, &mut remote2);

        // We expect to attempt to reconnect.
        // Advance time by the maximum amount of time it would take to establish
        // a connection.
        exec.set_fake_time(MAX_CONNECTION_EST_TIME.after_now());
        let _ = exec.wake_expired_timers();

        let (channel3, mut _remote3) = create_test_channels(transport);
        let next_request_fut = profile_requests.next();
        let mut next_request_fut = pin!(next_request_fut);
        match exec.run_until_stalled(&mut next_request_fut) {
            Poll::Ready(Some(Ok(ProfileRequest::Connect { responder, .. }))) => {
                let channel = channel3.try_into().unwrap();
                responder.send(Ok(channel)).expect("FIDL response should work");
            }
            x => panic!("Expected Profile connection request to be ready, got {:?} instead.", x),
        };

        // Run to update watcher state.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        assert!(peer_handle.is_control_connected());

        // New channel should be good to go.
        expect_channel_writable(&mut exec, &mut _remote3);
    }

    /// Tests that when connection fails, we don't infinitely retry.
    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn test_connection_no_retries(transport: Transport) {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(5_000000000));

        let id = PeerId(123);
        let (peer_handle, _target_delegate, mut profile_requests) = setup_remote_peer(id);

        // Set the descriptor to simulate service found for peer.
        peer_handle.set_target_descriptor(AvrcpService::Target {
            features: AvrcpTargetFeatures::CATEGORY1 | AvrcpTargetFeatures::SUPPORTSBROWSING,
            psm: Psm::AVCTP,
            protocol_version: AvrcpProtocolVersion(1, 6),
        });
        peer_handle.request_control_connection();

        assert!(!peer_handle.is_control_connected());

        let next_request_fut = profile_requests.next();
        let mut next_request_fut = pin!(next_request_fut);
        assert!(exec.run_until_stalled(&mut next_request_fut).is_pending());

        // Advance time by the maximum amount of time it would take to establish
        // a connection.
        exec.set_fake_time(MAX_CONNECTION_EST_TIME.after_now());
        let _ = exec.wake_expired_timers();

        // We should have requested a connection for control.
        match exec.run_until_stalled(&mut next_request_fut) {
            Poll::Ready(Some(Ok(ProfileRequest::Connect { responder, .. }))) => {
                // Trigger connection failure.
                responder
                    .send(Err(fidl_fuchsia_bluetooth::ErrorCode::Failed))
                    .expect("FIDL response should work");
            }
            x => panic!("Expected Profile connection request to be ready, got {:?} instead.", x),
        };

        // run until stalled, the connection should have failed.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        assert!(!peer_handle.is_control_connected());

        // Advance time by the maximum amount of time it would take to establish
        // a connection.
        exec.set_fake_time(MAX_CONNECTION_EST_TIME.after_now());
        let _ = exec.wake_expired_timers();

        // We shouldn't have requested retry.
        let next_request_fut = profile_requests.next();
        let mut next_request_fut = pin!(next_request_fut);
        assert!(exec.run_until_stalled(&mut next_request_fut).is_pending());

        // Set control channel manually to test browse channel connection retry
        let _remote = set_incoming_control_connection(&peer_handle, transport);

        // Browse is still not connected,
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        assert!(peer_handle.is_control_connected());
        assert!(!peer_handle.is_browse_connected());

        // Advance time by the maximum amount of time it would take to establish
        // a connection.
        exec.set_fake_time(MAX_CONNECTION_EST_TIME.after_now());
        let _ = exec.wake_expired_timers();

        // We should have requested a connection for browse.
        match exec.run_until_stalled(&mut next_request_fut) {
            Poll::Ready(Some(Ok(ProfileRequest::Connect { connection, responder, .. }))) => {
                // Trigger failure.
                responder
                    .send(Err(fidl_fuchsia_bluetooth::ErrorCode::Failed))
                    .expect("FIDL response should work");

                // Verify that request is for browse.
                match connection {
                    ConnectParameters::L2cap(L2capParameters {
                        psm: Some(v),
                        parameters: Some(params),
                        ..
                    }) => {
                        assert_eq!(v, u16::from(Psm::new(27))); // AVCTP_BROWSE
                        assert_eq!(
                            params.channel_mode.expect("channel mode should not be None"),
                            fidl_bt::ChannelMode::EnhancedRetransmission
                        );
                    }
                    x => panic!("Expected L2CAP parameters but got: {:?}", x),
                }
            }
            x => panic!("Expected Profile connection request to be ready, got {:?} instead.", x),
        };

        // run until stalled, the connection should have failed.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        assert!(!peer_handle.is_browse_connected());

        // Advance time by the maximum amount of time it would take to establish
        // a connection.
        exec.set_fake_time(MAX_CONNECTION_EST_TIME.after_now());
        let _ = exec.wake_expired_timers();

        let next_request_fut = profile_requests.next();
        let mut next_request_fut = pin!(next_request_fut);
        assert!(exec.run_until_stalled(&mut next_request_fut).is_pending());
    }

    /// Tests that when inbound and outbound browse connections are
    /// established at the same time, AVRCP drops both, and attempts to
    /// reconnect.
    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn test_simultaneous_browse_connections(transport: Transport) {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(5_000000000));

        let id = PeerId(123);
        let (peer_handle, _target_delegate, mut profile_requests) = setup_remote_peer(id);

        // Set the descriptor to simulate service found for peer.
        peer_handle.set_target_descriptor(AvrcpService::Target {
            features: AvrcpTargetFeatures::CATEGORY1 | AvrcpTargetFeatures::SUPPORTSBROWSING,
            psm: Psm::AVCTP,
            protocol_version: AvrcpProtocolVersion(1, 6),
        });
        peer_handle.request_control_connection();

        assert!(!peer_handle.is_control_connected());

        let next_request_fut = profile_requests.next();
        let mut next_request_fut = pin!(next_request_fut);
        assert!(exec.run_until_stalled(&mut next_request_fut).is_pending());

        // Advance time by the maximum amount of time it would take to establish
        // a connection.
        exec.set_fake_time(MAX_CONNECTION_EST_TIME.after_now());
        let _ = exec.wake_expired_timers();

        // We should have requested a connection for control.
        let (channel, _remote) = create_test_channels(transport);
        match exec.run_until_stalled(&mut next_request_fut) {
            Poll::Ready(Some(Ok(ProfileRequest::Connect { responder, .. }))) => {
                let channel = channel.try_into().unwrap();
                responder.send(Ok(channel)).expect("FIDL response should work");
            }
            x => panic!("Expected Profile connection request to be ready, got {:?} instead.", x),
        };

        // Run until stalled, the connection should be put in place.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        assert!(peer_handle.is_control_connected());

        // Advance time by the maximum amount of time it would take to establish
        // a connection.
        exec.set_fake_time(MAX_CONNECTION_EST_TIME.after_now());
        let _ = exec.wake_expired_timers();

        // We should have requested a connection for browse.
        let (channel2, mut remote2) = create_test_channels(transport);
        let mut next_request_fut = profile_requests.next();
        match exec.run_until_stalled(&mut next_request_fut) {
            Poll::Ready(Some(Ok(ProfileRequest::Connect { responder, .. }))) => {
                let channel = channel2.try_into().unwrap();
                responder.send(Ok(channel)).expect("FIDL response should work");
            }
            x => panic!("Expected Profile connection request to be ready, got {:?} instead.", x),
        };

        // Run until stalled, the connection should be put in place.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        assert!(peer_handle.is_browse_connected());

        // Should be able to send data over the channel.
        expect_channel_writable(&mut exec, &mut remote2);

        // Advance time by LESS than the CONNECTION_THRESHOLD amount.
        let advance_time = CONNECTION_THRESHOLD.into_nanos() - 200;
        exec.set_fake_time(zx::MonotonicDuration::from_nanos(advance_time).after_now());
        let _ = exec.wake_expired_timers();

        // Simulate inbound browse connection.
        let mut remote3 = set_incoming_browse_connection(&peer_handle, transport);

        // Browse channel should be disconnected, but control channel should remain connected.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        assert!(peer_handle.is_control_connected());
        assert!(!peer_handle.is_browse_connected());

        // Both the inbound and outbound-initiated browse channels should be
        // dropped. Sending data should not work.
        expect_channel_closed(&mut exec, &mut remote2);
        expect_channel_closed(&mut exec, &mut remote3);

        // We expect to attempt to reconnect.
        // Advance time by the maximum amount of time it would take to establish
        // a connection.
        exec.set_fake_time(MAX_CONNECTION_EST_TIME.after_now());
        let _ = exec.wake_expired_timers();

        let (channel4, mut remote4) = create_test_channels(transport);
        let mut next_request_fut = profile_requests.next();
        match exec.run_until_stalled(&mut next_request_fut) {
            Poll::Ready(Some(Ok(ProfileRequest::Connect { responder, .. }))) => {
                let channel = channel4.try_into().unwrap();
                responder.send(Ok(channel)).expect("FIDL response should work");
            }
            x => panic!("Expected Profile connection request to be ready, got {:?} instead.", x),
        };

        // Run to update watcher state.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        assert!(peer_handle.is_browse_connected());

        // New channel should be good to go.
        expect_channel_writable(&mut exec, &mut remote4);
    }

    /// Tests that when new inbound control connection comes in, previous
    /// control and browse connections are dropped.
    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn incoming_channel_resets_connections(transport: Transport) {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(5_000000000));

        let id = PeerId(1);
        let (peer_handle, _target_delegate, mut profile_requests) = setup_remote_peer(id);

        // Set the descriptor to simulate service found for peer - while unusual, this peer
        // advertises a non-standard PSM.
        peer_handle.set_target_descriptor(AvrcpService::Target {
            features: AvrcpTargetFeatures::CATEGORY1 | AvrcpTargetFeatures::SUPPORTSBROWSING,
            psm: Psm::AVCTP,
            protocol_version: AvrcpProtocolVersion(1, 6),
        });
        peer_handle.request_control_connection();

        assert!(!peer_handle.is_control_connected());

        let next_request_fut = profile_requests.next();
        let mut next_request_fut = pin!(next_request_fut);
        assert!(exec.run_until_stalled(&mut next_request_fut).is_pending());

        // Advance time by the maximum amount of time it would take to establish
        // a connection.
        exec.set_fake_time(MAX_CONNECTION_EST_TIME.after_now());
        let _ = exec.wake_expired_timers();

        // We should have requested a connection for control.
        let (channel, mut remote) = create_test_channels(transport);
        match exec.run_until_stalled(&mut next_request_fut) {
            Poll::Ready(Some(Ok(ProfileRequest::Connect { responder, .. }))) => {
                let channel = channel.try_into().unwrap();
                responder.send(Ok(channel)).expect("FIDL response should work");
            }
            x => panic!("Expected Profile connection request to be ready, got {:?} instead.", x),
        };

        // run until stalled, the connection should be put in place.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        assert!(peer_handle.is_control_connected());

        // Advance time by the maximum amount of time it would take to establish
        // a connection.
        exec.set_fake_time(MAX_CONNECTION_EST_TIME.after_now());
        let _ = exec.wake_expired_timers();

        // We should have requested a connection for browse.
        let (channel2, mut remote2) = create_test_channels(transport);
        let mut next_request_fut = profile_requests.next();
        match exec.run_until_stalled(&mut next_request_fut) {
            Poll::Ready(Some(Ok(ProfileRequest::Connect { responder, .. }))) => {
                let channel = channel2.try_into().unwrap();
                responder.send(Ok(channel)).expect("FIDL response should work");
            }
            x => panic!("Expected Profile connection request to be ready, got {:?} instead.", x),
        };

        // run until stalled, the connection should be put in place.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        assert!(peer_handle.is_browse_connected());

        // Should be able to send data over the channels.
        expect_channel_writable(&mut exec, &mut remote);
        expect_channel_writable(&mut exec, &mut remote2);

        // Advance time by some arbitrary amount before peer decides to reconnect.
        exec.set_fake_time(zx::MonotonicDuration::from_seconds(5).after_now());
        let _ = exec.wake_expired_timers();

        // After some time, remote peer sends incoming a new l2cap connection
        // for control channel. Keep the old one alive to validate that it's closed.
        let mut remote3 = set_incoming_control_connection(&peer_handle, transport);

        // Run to update watcher state. Control channel should be connected,
        // but browse channel that was previously set should have closed.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        assert!(peer_handle.is_control_connected());
        assert!(!peer_handle.is_browse_connected());

        // Shouldn't be able to send data over the old channels.
        expect_channel_closed(&mut exec, &mut remote);
        expect_channel_closed(&mut exec, &mut remote2);
        expect_channel_writable(&mut exec, &mut remote3);
    }

    /// Tests that when new inbound control connection comes in, previous
    /// control and browse connections are dropped.
    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn incoming_browse_channel_dropped(transport: Transport) {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(5_000000000));

        let id = PeerId(1);
        let (peer_handle, _target_delegate, mut _profile_requests) = setup_remote_peer(id);

        // Set the descriptor to simulate service found for peer - while unusual, this peer
        // advertises a non-standard PSM.
        peer_handle.set_target_descriptor(AvrcpService::Target {
            features: AvrcpTargetFeatures::CATEGORY1,
            psm: Psm::AVCTP,
            protocol_version: AvrcpProtocolVersion(1, 6),
        });

        assert!(!peer_handle.is_control_connected());
        assert!(!peer_handle.is_browse_connected());

        // Peer connects with a new l2cap connection for browse channel.
        // Since control channel was not already connected, verify that
        // browse channel was dropped.
        let _remote = set_incoming_browse_connection(&peer_handle, transport);
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        assert!(!peer_handle.is_browse_connected());
    }

    /// Tests that when control/browse channels are established by incoming connections,
    /// we handle the future state changes appropriately such as ensuring that the stream tasks
    /// are not started when they already exist.
    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn test_incoming_connections(transport: Transport) {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(5_000000000));

        let id = PeerId(123);
        let (peer_handle, _target_delegate, _profile_requests) = setup_remote_peer(id);
        assert!(!peer_handle.is_control_connected());

        // Simulate inbound control connection.
        let mut remote1 = set_incoming_control_connection(&peer_handle, transport);

        // Advance time by the maximum amount of time it would take to establish
        // a connection.
        exec.set_fake_time(MAX_CONNECTION_EST_TIME.after_now());
        let _ = exec.wake_expired_timers();
        assert!(peer_handle.is_control_connected());
        assert!(!peer_handle.is_browse_connected());

        // Should be able to send data over the channel.
        expect_channel_writable(&mut exec, &mut remote1);

        // Simulate inbound browse connection.
        let mut remote2 = set_incoming_browse_connection(&peer_handle, transport);

        // Advance time by the maximum amount of time it would take to establish
        // a connection.
        exec.set_fake_time(MAX_CONNECTION_EST_TIME.after_now());
        let _ = exec.wake_expired_timers();
        assert!(peer_handle.is_control_connected());
        assert!(peer_handle.is_browse_connected());

        // Should be able to send data over the channel.
        expect_channel_writable(&mut exec, &mut remote2);

        // Set the descriptors to simulate service found for peer. Setting the
        // descriptor should wake up the state watcher. At this point, both control and
        // browse connections were already established.
        peer_handle.set_controller_descriptor(AvrcpService::Controller {
            features: AvrcpControllerFeatures::CATEGORY1
                | AvrcpControllerFeatures::CATEGORY2
                | AvrcpControllerFeatures::SUPPORTSBROWSING
                | AvrcpControllerFeatures::SUPPORTSCOVERARTGETIMAGEPROPERTIES,
            psm: Psm::AVCTP,
            protocol_version: AvrcpProtocolVersion(1, 5),
        });
        assert!(peer_handle.is_control_connected());
        assert!(peer_handle.is_browse_connected());

        // State watcher task should not have panicked since control/browse command stream would
        // not have been taken for the second time.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

        // Set the descriptors to simulate service found for peer. This should
        // trigger state change.
        peer_handle.set_target_descriptor(AvrcpService::Target {
            features: AvrcpTargetFeatures::CATEGORY1
                | AvrcpTargetFeatures::CATEGORY2
                | AvrcpTargetFeatures::PLAYERSETTINGS
                | AvrcpTargetFeatures::GROUPNAVIGATION
                | AvrcpTargetFeatures::SUPPORTSBROWSING
                | AvrcpTargetFeatures::SUPPORTSMULTIPLEMEDIAPLAYERS,
            psm: Psm::AVCTP,
            protocol_version: AvrcpProtocolVersion(1, 5),
        });
        assert!(peer_handle.is_control_connected());
        assert!(peer_handle.is_browse_connected());

        // State watcher task should not have panicked since control/browse command stream would
        // not have been taken for the second time.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
    }

    fn attach_inspect_with_metrics(
        peer: &mut RemotePeerHandle,
    ) -> (fuchsia_inspect::Inspector, MetricsNode) {
        let inspect = fuchsia_inspect::Inspector::default();
        let metrics_node = MetricsNode::default().with_inspect(inspect.root(), "metrics").unwrap();
        peer.iattach(inspect.root(), "peer").unwrap();
        peer.set_metrics_node(metrics_node.clone());
        (inspect, metrics_node)
    }

    #[fuchsia::test]
    fn outgoing_connection_error_updates_inspect() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(5_000000000));

        let id = PeerId(30789);
        let (mut peer_handle, _target_delegate, mut profile_requests) = setup_remote_peer(id);
        let (inspect, _metrics_node) = attach_inspect_with_metrics(&mut peer_handle);

        // Set the descriptor to simulate service found for peer.
        peer_handle.set_target_descriptor(AvrcpService::Target {
            features: AvrcpTargetFeatures::CATEGORY1,
            psm: Psm::AVCTP,
            protocol_version: AvrcpProtocolVersion(1, 6),
        });
        peer_handle.request_control_connection();

        let mut next_request_fut = Box::pin(profile_requests.next());
        assert!(exec.run_until_stalled(&mut next_request_fut).is_pending());

        // Advance time by the maximum amount of time it would take to establish a connection.
        exec.set_fake_time(MAX_CONNECTION_EST_TIME.after_now());
        let _ = exec.wake_expired_timers();

        // We expect to initiate an outbound connection through the profile server. Simulate error.
        match exec.run_until_stalled(&mut next_request_fut) {
            Poll::Ready(Some(Ok(ProfileRequest::Connect { responder, .. }))) => {
                responder.send(Err(ErrorCode::Failed)).expect("Fidl response should be ok");
            }
            x => panic!("Expected ready profile connection, but got: {:?}", x),
        };

        // Run to update watcher state.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

        // Inspect tree should be updated with the connection error count.
        assert_data_tree!(@executor exec, inspect, root: {
            peer: contains {},
            metrics: contains {
                connection_errors: 1u64,
                control_connections: 0u64,
            }
        });
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn successful_inbound_connection_updates_inspect_metrics(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let id = PeerId(842);
        let (mut peer_handle, _target_delegate, _profile_requests) = setup_remote_peer(id);
        let (inspect, _metrics_node) = attach_inspect_with_metrics(&mut peer_handle);

        // Set the descriptor to simulate service found for peer.
        peer_handle.set_target_descriptor(AvrcpService::Target {
            features: AvrcpTargetFeatures::CATEGORY1,
            psm: Psm::AVCTP,
            protocol_version: AvrcpProtocolVersion(1, 6),
        });
        peer_handle.request_control_connection();
        // Peer initiates connection to us.
        let remote1 = set_incoming_control_connection(&peer_handle, transport);

        // Run to update watcher state.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

        // Inspect tree should be updated with the connection.
        assert_data_tree!(@executor exec, inspect, root: {
            peer: contains {
                control: "Connected",
            },
            metrics: contains {
                connection_errors: 0u64,
                control_connections: 1u64,
                browse_connections: 0u64,
            }
        });

        // Peer initiates a browse connection.
        let _remote2 = set_incoming_browse_connection(&peer_handle, transport);

        // Run to update watcher state.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

        // Inspect tree should be updated with the browse connection.
        assert_data_tree!(@executor exec, inspect, root: {
            peer: contains {
                control: "Connected",
            },
            metrics: contains {
                connection_errors: 0u64,
                control_connections: 1u64,
                browse_connections: 1u64,
            }
        });

        // Peer disconnects.
        drop(remote1);
        // Run to update watcher state.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        // Inspect tree should be updated with the disconnection.
        assert_data_tree!(@executor exec, inspect, root: {
            peer: contains {
                control: "Disconnected",
            },
            metrics: contains {
                connection_errors: 0u64,
                control_connections: 1u64,
                browse_connections: 1u64,
            }
        });
    }

    #[track_caller]
    pub(crate) fn get_next_avctp_command(
        exec: &mut fasync::TestExecutor,
        command_stream: &mut AvctpCommandStream,
    ) -> AvctpCommand {
        exec.run_until_stalled(&mut command_stream.try_next())
            .expect("should be ready")
            .unwrap()
            .expect("has valid command")
    }

    #[track_caller]
    pub(crate) fn get_next_avc_command(
        exec: &mut fasync::TestExecutor,
        command_stream: &mut AvcCommandStream,
    ) -> AvcCommand {
        exec.run_until_stalled(&mut command_stream.try_next())
            .expect("should be ready")
            .unwrap()
            .expect("has valid command")
    }

    // There are some browse commands that are sent out post peer-setup.
    // Verify that those browse commands were successfully sent out and reply
    // with a mock response.
    #[track_caller]
    pub(crate) fn expect_outgoing_commands(
        exec: &mut fasync::TestExecutor,
        avc_stream: &mut AvcCommandStream,
        avctp_stream: &mut AvctpCommandStream,
    ) {
        // Expect get folder items command with media player scope.
        let command = get_next_avctp_command(exec, avctp_stream);
        let params = decode_avctp_command(&command, PduId::GetFolderItems);
        let cmd =
            GetFolderItemsCommand::decode(&params).expect("should have received valid command");
        assert_matches!(cmd.scope(), Scope::MediaPlayerList);
        let mock_resp = GetFolderItemsResponse::new_success(1, vec![]);
        send_avctp_response(PduId::GetFolderItems, &mock_resp, &command);

        // Expect get capabilities command for setting up notification streams.
        let command = get_next_avc_command(exec, avc_stream);
        let (pdu_id, _) = decode_avc_vendor_command(&command).expect("should have succeeded");
        assert_eq!(pdu_id, PduId::GetCapabilities);
        let packets =
            GetCapabilitiesResponse::new_events(&[]).encode_packet().expect("should not fail");
        let _ = command
            .send_response(AvcResponseType::ImplementedStable, &packets[..])
            .expect("should succeed");
    }

    /// Helper function for decoding AVCTP command. It checks that the PduId of the message is
    /// equal to the expected PduId.
    #[track_caller]
    pub(crate) fn decode_avctp_command(command: &AvctpCommand, expected_pdu_id: PduId) -> Vec<u8> {
        // Decode the provided `command` into a PduId and command parameters.
        match BrowseChannelHandler::decode_command(command) {
            Ok((id, packet)) if id == expected_pdu_id => packet,
            result => panic!("[Browse Channel] Received unexpected result: {:?}", result),
        }
    }

    /// Helper function to reply to the command with an AVCTP response.
    #[track_caller]
    pub(crate) fn send_avctp_response(
        pdu_id: PduId,
        response: &impl Encodable<Error = PacketError>,
        command: &AvctpCommand,
    ) {
        let mut buf = vec![0; response.encoded_len()];
        response.encode(&mut buf[..]).expect("should have succeeded");

        send_avctp_response_raw(pdu_id, buf, command)
    }

    /// Helper function to reply to the command with an AVCTP response buffer.
    #[track_caller]
    pub(crate) fn send_avctp_response_raw(pdu_id: PduId, body: Vec<u8>, command: &AvctpCommand) {
        // Send the response back to the remote peer.
        let response_packet = BrowsePreamble::new(u8::from(&pdu_id), body);
        let mut response_buf = vec![0; response_packet.encoded_len()];
        response_packet.encode(&mut response_buf[..]).expect("Encoding should work");

        let _ = command.send_response(&response_buf[..]).expect("should have succeeded");
    }

    #[fuchsia::test]
    fn test_get_candidate_browse_player() {
        let _exec = fasync::TestExecutor::new();

        // Set up peer for testing with available players information.
        let id = PeerId(1);
        let (peer_handle, _target_delegate, _profile_requests) = setup_remote_peer(id);
        peer_handle.peer.write().update_available_players(&[
            // Browsable, OnlyBrowsableWhenAddressed.
            MediaPlayerItem::new(
                1,
                1,
                1,
                PlaybackStatus::Playing,
                [0x0000000000B701EF, 0],
                "player 1".to_string(),
            ),
            // Not browsable
            MediaPlayerItem::new(
                2,
                1,
                1,
                PlaybackStatus::Playing,
                [0x0000000000B70167, 0],
                "player 2".to_string(),
            ),
            // Browsable.
            MediaPlayerItem::new(
                3,
                1,
                1,
                PlaybackStatus::Playing,
                [0x0000000000B7016F, 0],
                "player 3".to_string(),
            ),
        ]);

        // Without addressed player set, player ID with highest browse support level is returned if it exists.
        assert_eq!(
            peer_handle.peer.read().get_candidate_browse_player().expect("should have returned ID"),
            3
        );

        // With addressed player set, addressed player ID is returned if it supports browsing.
        peer_handle
            .peer
            .write()
            .handle_new_controller_notification_event(ControllerEvent::AddressedPlayerChanged(1));
        assert_eq!(
            peer_handle.peer.read().get_candidate_browse_player().expect("should have returned ID"),
            1
        );

        // With addressed player set to non-browsing supporting player, none is returned.
        peer_handle
            .peer
            .write()
            .handle_new_controller_notification_event(ControllerEvent::AddressedPlayerChanged(2));
        assert!(peer_handle.peer.read().get_candidate_browse_player().is_none());
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn successful_post_browse_connection_setup(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let id = PeerId(842);
        let (peer_handle, _target_delegate, _profile_requests) = setup_remote_peer(id);

        // Set the descriptor to simulate service found for peer.
        peer_handle.set_target_descriptor(AvrcpService::Target {
            features: AvrcpTargetFeatures::CATEGORY1,
            psm: Psm::AVCTP,
            protocol_version: AvrcpProtocolVersion(1, 6),
        });
        // Peer initiates control connection to us.
        let _remote1 = set_incoming_control_connection(&peer_handle, transport);
        // Run to update watcher state.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

        // Peer initiates a browse connection.
        let remote2 = set_incoming_browse_connection(&peer_handle, transport);
        // Run to update watcher state.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

        let remote_peer = AvctpPeer::new(remote2);
        let mut remote_command_stream = remote_peer.take_command_stream();

        // Should have sent a request to get all players.
        // Expect get folder items command with media player scope.
        let command = get_next_avctp_command(&mut exec, &mut remote_command_stream);
        let params = decode_avctp_command(&command, PduId::GetFolderItems);
        let cmd =
            GetFolderItemsCommand::decode(&params).expect("should have received valid command");
        assert_matches!(cmd.scope(), Scope::MediaPlayerList);

        const PLAYER_1_ID: u16 = 1003; // player with higher browseability.
        const PLAYER_2_ID: u16 = 1004;
        let mock_players = vec![
            BrowseableItem::MediaPlayer(MediaPlayerItem::new(
                PLAYER_1_ID,
                1,
                1,
                PlaybackStatus::Playing,
                // Browsable.
                [0x0000000000B7016F as u64, 0x02000000000000 as u64],
                "player 1".to_string(),
            )),
            BrowseableItem::MediaPlayer(MediaPlayerItem::new(
                PLAYER_2_ID,
                1,
                1,
                PlaybackStatus::Playing,
                // Browsable, OnlyBrowsableWhenAddressed.
                [0x0000000000B701EF as u64, 0x02000000000000 as u64],
                "player 2".to_string(),
            )),
        ];
        let mock_resp = GetFolderItemsResponse::new_success(1, mock_players);
        send_avctp_response(PduId::GetFolderItems, &mock_resp, &command);

        // Should have sent a request to set browsed player.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        let command = get_next_avctp_command(&mut exec, &mut remote_command_stream);
        // Ensure command params are correct.
        let params = decode_avctp_command(&command, PduId::SetBrowsedPlayer);
        let cmd =
            SetBrowsedPlayerCommand::decode(&params).expect("should have received valid command");
        assert_eq!(cmd.player_id(), 1003); // Player with "higher" browsing capability is set as browsed player.
        // Create mock response.
        let resp =
            SetBrowsedPlayerResponse::new_success(0, 0, vec![]).expect("should not return error");
        send_avctp_response(PduId::SetBrowsedPlayer, &resp, &command);
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

        // Should have set browsed player.
        let player = peer_handle.get_browsable_player().expect("should have been set");
        assert_eq!(player.player_id, PLAYER_1_ID);
        assert_eq!(player.uid_counter, None);
        assert_eq!(player.num_items, 0);
        assert_eq!(player.sub_folders.len(), 0);
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn incoming_control_command_loop_exits_gracefully(transport: Transport) {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(5_000000000));

        let id = PeerId(842);
        let (peer_handle, _target_delegate, _profile_requests) = setup_remote_peer(id);

        // Simulate inbound control connection.
        let mut remote_control = set_incoming_control_connection(&peer_handle, transport);
        exec.set_fake_time(MAX_CONNECTION_EST_TIME.after_now());
        let _ = exec.wake_expired_timers();

        // Simulate inbound browse connection.
        let _remote_browse = set_incoming_browse_connection(&peer_handle, transport);
        exec.set_fake_time(MAX_CONNECTION_EST_TIME.after_now());
        let _ = exec.wake_expired_timers();

        assert!(peer_handle.is_control_connected());
        assert!(peer_handle.is_browse_connected());

        // Set the descriptors to simulate service found for peer.
        peer_handle.set_controller_descriptor(AvrcpService::Controller {
            features: AvrcpControllerFeatures::CATEGORY1
                | AvrcpControllerFeatures::CATEGORY2
                | AvrcpControllerFeatures::SUPPORTSBROWSING
                | AvrcpControllerFeatures::SUPPORTSCOVERARTGETIMAGEPROPERTIES,
            psm: Psm::AVCTP,
            protocol_version: AvrcpProtocolVersion(1, 5),
        });
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

        // Send erroneous data over the control channel.
        let mut write_fut = remote_control.send(vec![0, 17, 14, 0, 72]);
        exec.run_until_stalled(&mut write_fut)
            .expect("should be ready")
            .expect("write should succeed");

        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

        // Should have gracefully disconnected all connections.
        assert!(!peer_handle.is_control_connected());
        assert!(!peer_handle.is_browse_connected());
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn incoming_browse_command_loop_exits_gracefully(transport: Transport) {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(5_000000000));

        let id = PeerId(842);
        let (peer_handle, _target_delegate, _profile_requests) = setup_remote_peer(id);

        // Simulate inbound control connection.
        let _remote_control = set_incoming_control_connection(&peer_handle, transport);
        exec.set_fake_time(MAX_CONNECTION_EST_TIME.after_now());
        let _ = exec.wake_expired_timers();

        // Simulate inbound browse connection.
        let (local_browse, mut remote_browse) = match transport {
            Transport::Socket => {
                let (remote, local) = zx::Socket::create_datagram();
                assert!(local.half_close().is_ok());
                (
                    Channel::from_socket_infallible(local, Channel::DEFAULT_MAX_TX),
                    Channel::from_socket_infallible(remote, Channel::DEFAULT_MAX_TX),
                )
            }
            Transport::Fidl => create_test_channels(transport),
        };

        let browse_channel = AvctpPeer::new(local_browse);
        peer_handle.set_browse_connection(browse_channel);
        exec.set_fake_time(MAX_CONNECTION_EST_TIME.after_now());
        let _ = exec.wake_expired_timers();

        assert!(peer_handle.is_control_connected());
        assert!(peer_handle.is_browse_connected());

        // Set the descriptors to simulate service found for peer.
        peer_handle.set_controller_descriptor(AvrcpService::Controller {
            features: AvrcpControllerFeatures::CATEGORY1
                | AvrcpControllerFeatures::CATEGORY2
                | AvrcpControllerFeatures::SUPPORTSBROWSING
                | AvrcpControllerFeatures::SUPPORTSCOVERARTGETIMAGEPROPERTIES,
            psm: Psm::AVCTP,
            protocol_version: AvrcpProtocolVersion(1, 5),
        });
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

        match transport {
            Transport::Socket => {
                // Send data over the browse channel with socket that'll cause error on write.
                let mut write_fut = remote_browse.send(vec![1, 1]);
                let _ = exec
                    .run_until_stalled(&mut write_fut)
                    .expect("should be ready")
                    .expect_err("should have failed");
            }
            Transport::Fidl => {
                // For FIDL, we just drop the remote side to trigger EOF.
                drop(remote_browse);
            }
        }

        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

        // Should have gracefully disconnected only the browse connection.
        assert!(peer_handle.is_control_connected());
        assert!(!peer_handle.is_browse_connected());
    }
}
