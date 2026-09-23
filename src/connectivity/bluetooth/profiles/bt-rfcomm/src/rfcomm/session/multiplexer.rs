// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bt_rfcomm::frame::Frame;
use bt_rfcomm::frame::mux_commands::DEFAULT_INITIAL_CREDITS;
use bt_rfcomm::{DLCI, RfcommError, Role};
use fidl_fuchsia_bluetooth as fidl_bt;
use fuchsia_bluetooth::types::{Channel, ConnectionBackendType};
use fuchsia_inspect as inspect;
use fuchsia_inspect_derive::{AttachError, Inspect};
use futures::channel::mpsc;
use log::{info, trace, warn};
use std::collections::HashMap;

use crate::rfcomm::inspect::SessionMultiplexerInspect;
use crate::rfcomm::session::channel::{
    Credits, FlowControlMode, FlowControlledData, SessionChannel,
};
use crate::rfcomm::types::Error;

/// The negotiation parameters associated with a specific DLC.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DlcNegotiationParameters {
    /// The maximum frame size for the DLC.
    pub max_frame_size: u16,
    /// The initial number of credits for the DLC. Only applicable when flow control is enabled.
    ///
    /// If the parameters are associated with a request from a peer, then these credits are the
    /// initial credits that are assigned to the local device (i.e. `initial_local_credits`).
    /// If the parameters are associated with a response to a peer, then these credits are the
    /// initial credits that are assigned to the remote device (i.e. `initial_remote_credits`).
    pub initial_credits: u8,
}

/// The parameters associated with this Session.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SessionParameters {
    /// Whether credit-based flow control is being used for this session.
    pub credit_based_flow: bool,
}

/// By default, we prefer credit-based flow control.
impl Default for SessionParameters {
    fn default() -> Self {
        Self { credit_based_flow: true }
    }
}

/// The current state of the session parameters.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ParameterNegotiationState {
    /// Parameters have not been negotiated.
    #[default]
    NotNegotiated,
    /// Parameters have been negotiated.
    Negotiated(SessionParameters),
}

impl ParameterNegotiationState {
    /// Negotiates the `new` parameters with the (potential) current parameters. Returns
    /// the parameters that were set.
    fn negotiate(&mut self, new_parameters: SessionParameters) -> SessionParameters {
        // Our implementation is OK with either flow control mode. Choose whatever is requested.
        *self = Self::Negotiated(new_parameters);
        new_parameters
    }
}

/// The `SessionMultiplexer` manages channels over the range of valid User-DLCIs. It is responsible
/// for maintaining the current state of the RFCOMM Session, and provides an API to create,
/// establish, and relay user data over the multiplexed channels.
///
/// The `SessionMultiplexer` is considered "started" when its Role has been assigned.
/// The parameters for the multiplexer must be negotiated before the first DLCI has
/// been established. RFCOMM 5.5.3 states that renegotiation of parameters is optional - this
/// multiplexer will simply echo the current parameters in the event a negotiation request is
/// received after the first DLC is opened and established.
pub struct SessionMultiplexer {
    /// The role for the multiplexer.
    role: Role,
    /// The maximum RFCOMM packet size that can be sent over the underlying transport. This is the
    /// stack's preferred maximum packet size, and can be negotiated for each DLC.
    max_rfcomm_packet_size: u16,
    /// The parameters for the multiplexer.
    parameters: ParameterNegotiationState,
    /// Local opened RFCOMM channels for this session.
    channels: HashMap<DLCI, SessionChannel>,
    /// The inspect node for this object.
    inspect: SessionMultiplexerInspect,
    /// The backend type of the underlying L2CAP channel.
    backend_type: ConnectionBackendType,
}

impl Inspect for &mut SessionMultiplexer {
    fn iattach(self, parent: &inspect::Node, name: impl AsRef<str>) -> Result<(), AttachError> {
        self.inspect.iattach(parent, name)?;
        self.inspect.set_default_max_packet_size(self.max_rfcomm_packet_size);
        Ok(())
    }
}

impl SessionMultiplexer {
    pub fn create(max_rfcomm_packet_size: u16, backend_type: ConnectionBackendType) -> Self {
        if backend_type == ConnectionBackendType::FidlServer {
            warn!("RFCOMM SessionMultiplexer initialized with unexpected FidlServer backend type");
        }
        Self {
            role: Role::Unassigned,
            max_rfcomm_packet_size,
            parameters: ParameterNegotiationState::default(),
            channels: HashMap::new(),
            inspect: SessionMultiplexerInspect::default(),
            backend_type,
        }
    }

    /// Resets the multiplexer back to its initial state with no opened channels.
    pub fn reset(&mut self) {
        *self = Self::create(self.max_rfcomm_packet_size, self.backend_type);
    }

    pub fn role(&self) -> Role {
        self.role
    }

    pub fn set_role(&mut self, role: Role) {
        self.role = role;
        self.inspect.set_role(role);
    }

    /// Returns true if credit-based flow control is enabled.
    pub fn credit_based_flow(&self) -> bool {
        self.parameters().credit_based_flow
    }

    /// Returns the default max frame size for the session.
    pub fn default_max_packet_size(&self) -> u16 {
        self.max_rfcomm_packet_size
    }

    #[cfg(test)]
    pub fn parameter_negotiation_state(&self) -> ParameterNegotiationState {
        self.parameters
    }

    /// Returns true if the session parameters have been negotiated.
    pub fn parameters_negotiated(&self) -> bool {
        std::matches!(&self.parameters, ParameterNegotiationState::Negotiated { .. })
    }

    /// Returns the parameters associated with this session.
    pub fn parameters(&self) -> SessionParameters {
        match &self.parameters {
            ParameterNegotiationState::Negotiated(p) => *p,
            ParameterNegotiationState::NotNegotiated => SessionParameters::default(),
        }
    }

    /// Negotiates the parameters for this session - returns the session parameters that were set.
    pub fn negotiate_session_parameters(
        &mut self,
        new_session_parameters: SessionParameters,
    ) -> SessionParameters {
        // This implementation does not support changing the flow control setting after a DLC has
        // been established. See RFCOMM Section 5.5.3 for details.
        if self.any_dlc_established() {
            let current = self.parameters();
            if current.credit_based_flow != new_session_parameters.credit_based_flow {
                warn!(
                    "Negotiation request for conflicting flow control setting. Using current credit-flow setting = {}",
                    current.credit_based_flow
                );
            }
            return current;
        }

        // No DLCs have been established yet. Negotiate the session-wide flow control mode.
        let updated = self.parameters.negotiate(new_session_parameters);
        trace!("Updated Session parameters: {:?}", updated);
        self.inspect.set_session_parameters(updated);
        updated
    }

    /// Negotiates the peer's `channel_parameters` with our parameters for the specific `dlci`.
    /// Returns the negotiated max packet size and initial remote credits (if applicable) that are
    /// set for the DLC.
    pub fn negotiate_channel_parameters(
        &mut self,
        dlci: DLCI,
        channel_parameters: DlcNegotiationParameters,
    ) -> DlcNegotiationParameters {
        // The negotiated maximum frame size is always the smaller value requested between the peer
        // and our preferred default.
        let negotiated_max_packet_size =
            std::cmp::min(self.max_rfcomm_packet_size, channel_parameters.max_frame_size);

        // Initialize the flow control mode based on the session-wide flow control mode.
        let flow_control = if self.credit_based_flow() {
            // The credits provided by the peer in `initial_credits` is our (local) credit count.
            // In return, we always assign `DEFAULT_INITIAL_CREDITS` as the peer's (remote) credit
            // count.
            let credits = Credits::new(
                usize::from(channel_parameters.initial_credits),
                usize::from(DEFAULT_INITIAL_CREDITS),
            );
            FlowControlMode::CreditBased(credits)
        } else {
            FlowControlMode::None
        };
        // Will be 0 if flow control is disabled.
        let initial_credits = flow_control.initial_remote_credits().unwrap_or_default();

        let channel = self.find_or_create_session_channel(dlci);
        if let Err(e) = channel.set_parameters(negotiated_max_packet_size, flow_control) {
            warn!("Failed to set parameters for {dlci:?}: {e:?}");
        }
        DlcNegotiationParameters { max_frame_size: negotiated_max_packet_size, initial_credits }
    }

    /// Returns true if the multiplexer has started.
    pub fn started(&self) -> bool {
        self.role.is_multiplexer_started()
    }

    /// Starts the session multiplexer and assumes the provided `role`. Returns Ok(()) if mux
    /// startup is successful.
    pub fn start(&mut self, role: Role) -> Result<(), Error> {
        // Re-starting the multiplexer is not valid, as this would invalidate any opened
        // RFCOMM channels.
        if self.started() {
            return Err(Error::MultiplexerAlreadyStarted);
        }

        // Role must be a valid started role.
        if !role.is_multiplexer_started() {
            return Err(RfcommError::InvalidRole(role).into());
        }

        self.set_role(role);
        info!(role:?; "RFCOMM Session multiplexer started");
        Ok(())
    }

    /// Returns true if the provided `dlci` has been initialized and established in
    /// the multiplexer.
    pub fn dlci_established(&self, dlci: &DLCI) -> bool {
        self.channels.get(dlci).map(|c| c.is_established()).unwrap_or_default()
    }

    /// Returns true if at least one DLC has been established.
    pub fn any_dlc_established(&self) -> bool {
        self.channels
            .iter()
            .fold(false, |acc, (_, session_channel)| acc | session_channel.is_established())
    }

    /// Returns true if the parameters have been negotiated for the provided `dlci`.
    pub fn dlc_parameters_negotiated(&self, dlci: &DLCI) -> bool {
        self.channels.get(dlci).is_some_and(|c| c.parameters_negotiated())
    }

    #[cfg(test)]
    fn get_session_channel(&self, dlci: DLCI) -> Option<&SessionChannel> {
        self.channels.get(&dlci)
    }

    /// Finds or initializes a new SessionChannel for the provided `dlci`. Returns a mutable
    /// reference to the channel.
    pub fn find_or_create_session_channel(&mut self, dlci: DLCI) -> &mut SessionChannel {
        let channel = self.channels.entry(dlci).or_insert_with(|| {
            let mut channel = SessionChannel::new(dlci, self.role);
            let _ = channel.iattach(self.inspect.node(), inspect::unique_name("channel_"));
            channel
        });
        channel
    }

    /// Attempts to establish a SessionChannel for the provided `dlci`.
    /// `user_data_sender` is used by the SessionChannel to relay any received UserData
    /// frames from the client associated with the channel.
    ///
    /// Returns the remote end of the channel on success.
    pub fn establish_session_channel(
        &mut self,
        dlci: DLCI,
        user_data_sender: mpsc::Sender<Frame>,
    ) -> Result<Channel, Error> {
        let backend_type = self.backend_type;
        trace!("Establishing session channel for DLCI {} using backend {:?}", dlci, backend_type);
        // If the session parameters have not been negotiated, set them to our preferred default.
        if !self.parameters_negotiated() {
            let _ = self.negotiate_session_parameters(SessionParameters::default());
        }
        let session_max_packet_size = self.default_max_packet_size();

        // Potentially reserve a new `SessionChannel` for the provided DLCI.
        let channel = self.find_or_create_session_channel(dlci);
        if channel.is_established() {
            return Err(Error::ChannelAlreadyEstablished(dlci));
        }

        // If the channel parameters have not been negotiated, set them to our preferred default.
        if !channel.parameters_negotiated() {
            channel.set_parameters(
                session_max_packet_size,
                FlowControlMode::CreditBased(Credits::default()),
            )?;
        }

        // Create endpoints for the session channel. The local end is held by this component
        // and the remote end is returned to be held by a RFCOMM profile.
        let max_tx_size = channel.max_packet_size().expect("set in `set_parameters`");
        let (local, remote) = match backend_type {
            ConnectionBackendType::Socket => {
                Channel::create_socket_pair_with_max_tx(max_tx_size.into())
            }
            ConnectionBackendType::FidlClient | ConnectionBackendType::FidlServer => {
                let (proxy, stream) =
                    fidl::endpoints::create_proxy_and_stream::<fidl_bt::ChannelMarker>();
                let remote = Channel::from_fidl_client(proxy, max_tx_size.into());
                let local = Channel::from_fidl_server(stream, max_tx_size.into());
                (local, remote)
            }
        };
        channel.establish(local, user_data_sender)?;
        Ok(remote)
    }

    /// Closes the `SessionChannel` for the provided `dlci`. Returns true if the `SessionChannel`
    /// was closed.
    pub fn close_session_channel(&mut self, dlci: &DLCI) -> bool {
        self.channels.remove(dlci).is_some()
    }

    /// Forwards `user_data` received from the peer to the `SessionChannel` associated with the
    /// `dlci`.
    /// Returns Error if there is no such channel or if it is closed.
    pub async fn receive_user_data(
        &mut self,
        dlci: DLCI,
        user_data: FlowControlledData,
    ) -> Result<(), Error> {
        let Some(session_channel) = self.channels.get_mut(&dlci) else {
            return Err(RfcommError::InvalidDLCI(dlci).into());
        };
        session_channel.receive_user_data(user_data).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use test_case::test_case;

    use assert_matches::assert_matches;
    use async_utils::PollExt;
    use diagnostics_assertions::assert_data_tree;
    use futures::StreamExt;

    use crate::rfcomm::inspect::CREDIT_FLOW_CONTROL;
    use crate::rfcomm::session::channel::Credits;
    use crate::rfcomm::types::SignaledTask;

    #[test_case(ConnectionBackendType::Socket; "socket")]
    #[test_case(ConnectionBackendType::FidlClient; "fidl")]
    #[fuchsia::test]
    fn negotiate_session_parameters(backend_type: ConnectionBackendType) {
        const DEFAULT_MAX_TX: u16 = 900;
        let mut multiplexer = SessionMultiplexer::create(DEFAULT_MAX_TX, backend_type);
        assert!(!multiplexer.parameters_negotiated());

        let new_parameters = SessionParameters { credit_based_flow: true };
        let negotiated = multiplexer.negotiate_session_parameters(new_parameters);
        assert_eq!(negotiated, SessionParameters { credit_based_flow: true });
        assert!(multiplexer.parameters_negotiated());
    }

    #[test_case(ConnectionBackendType::Socket; "socket")]
    #[test_case(ConnectionBackendType::FidlClient; "fidl")]
    #[fuchsia::test]
    fn renegotiate_parameters_after_channel_closed(backend_type: ConnectionBackendType) {
        let mut exec = fuchsia_async::TestExecutor::new();
        let mut multiplexer = SessionMultiplexer::create(900, backend_type);
        let dlci = DLCI::try_from(8).unwrap();

        // Negotiate initial parameters.
        let negotiated_params1 = multiplexer.negotiate_channel_parameters(
            dlci,
            DlcNegotiationParameters { max_frame_size: 500, initial_credits: 10 },
        );
        assert_eq!(negotiated_params1.max_frame_size, 500);

        // Establish the channel.
        let (sender, mut frame_receiver) = mpsc::channel(0);
        let channel_remote =
            multiplexer.establish_session_channel(dlci, sender).expect("can establish");
        assert!(multiplexer.dlci_established(&dlci));
        let channel = multiplexer.get_session_channel(dlci).expect("channel exists");
        let mut closed_fut = channel.finished();
        exec.run_until_stalled(&mut closed_fut).expect_pending("channel still active");

        // Simulate the RFCOMM application closing the channel.
        drop(channel_remote);

        // Expect the outgoing disconnect frame and the channel should be considered closed.
        let _frame = exec.run_until_stalled(&mut frame_receiver.next()).expect("frame received");
        exec.run_until_stalled(&mut closed_fut).expect("channel closed");
        assert!(!multiplexer.dlci_established(&dlci));

        // Re-negotiate parameters with new values.
        let negotiated_params2 = multiplexer.negotiate_channel_parameters(
            dlci,
            DlcNegotiationParameters { max_frame_size: 750, initial_credits: 0 },
        );
        // Should accept new parameters.
        assert_eq!(negotiated_params2.max_frame_size, 750);

        // Channel can be re-established and should have the new parameters.
        let (sender2, _receiver2) = mpsc::channel(0);
        let _ = multiplexer.establish_session_channel(dlci, sender2).expect("can establish again");
        assert!(multiplexer.dlci_established(&dlci));

        let channel2 = multiplexer.get_session_channel(dlci).expect("channel exists");
        assert_eq!(channel2.max_packet_size(), Some(750));
        let mut closed_fut2 = channel2.finished();
        exec.run_until_stalled(&mut closed_fut2).expect_pending("channel still active");
    }

    #[test_case(ConnectionBackendType::Socket; "socket")]
    #[test_case(ConnectionBackendType::FidlClient; "fidl")]
    #[fuchsia::test]
    fn negotiate_multiple_channels_and_renegotiation(backend_type: ConnectionBackendType) {
        let mut multiplexer = SessionMultiplexer::create(900, backend_type);
        let dlci1 = DLCI::try_from(8).unwrap();
        let dlci2 = DLCI::try_from(9).unwrap();

        // Negotiate parameters for DLCI 1.
        let parameters1 = multiplexer.negotiate_channel_parameters(
            dlci1,
            DlcNegotiationParameters { max_frame_size: 500, initial_credits: 10 },
        );
        assert_eq!(parameters1.max_frame_size, 500);
        assert_eq!(parameters1.initial_credits, DEFAULT_INITIAL_CREDITS);
        let channel1 = multiplexer.get_session_channel(dlci1).expect("channel should exist");
        assert_eq!(channel1.max_packet_size(), Some(500));

        // Negotiate parameters for DLCI 2.
        let parameters2 = multiplexer.negotiate_channel_parameters(
            dlci2,
            DlcNegotiationParameters { max_frame_size: 600, initial_credits: 20 },
        );
        assert_eq!(parameters2.max_frame_size, 600);
        assert_eq!(parameters2.initial_credits, DEFAULT_INITIAL_CREDITS);
        let channel2 = multiplexer.get_session_channel(dlci2).expect("channel should exist");
        assert_eq!(channel2.max_packet_size(), Some(600));

        // Renegotiate parameters for DLCI 1.
        let parameters1_updated = multiplexer.negotiate_channel_parameters(
            dlci1,
            DlcNegotiationParameters { max_frame_size: 550, initial_credits: 15 },
        );
        assert_eq!(parameters1_updated.max_frame_size, 550);
        assert_eq!(parameters1_updated.initial_credits, DEFAULT_INITIAL_CREDITS);
        let channel1_updated =
            multiplexer.get_session_channel(dlci1).expect("channel should exist");
        assert_eq!(channel1_updated.max_packet_size(), Some(550));
    }

    #[test_case(ConnectionBackendType::Socket; "socket")]
    #[test_case(ConnectionBackendType::FidlClient; "fidl")]
    #[fuchsia::test]
    fn negotiate_session_parameters_after_dlc_established_is_ignored(
        backend_type: ConnectionBackendType,
    ) {
        let _exec = fuchsia_async::TestExecutor::new();
        let mut multiplexer = SessionMultiplexer::create(900, backend_type);
        let dlci = DLCI::try_from(8).unwrap();

        // Establish a channel.
        let (sender, _receiver) = mpsc::channel(0);
        let _ = multiplexer.establish_session_channel(dlci, sender).expect("can establish");
        assert!(multiplexer.any_dlc_established());

        // Default is credit-based flow control.
        assert!(multiplexer.credit_based_flow());

        // Attempt to negotiate no credit-based flow control.
        let new_params = SessionParameters { credit_based_flow: false };
        let negotiated = multiplexer.negotiate_session_parameters(new_params);

        // Should be ignored, and return the current parameters (credit based).
        assert_eq!(negotiated.credit_based_flow, true);
        assert!(multiplexer.credit_based_flow());
    }

    #[test_case(ConnectionBackendType::Socket; "socket")]
    #[test_case(ConnectionBackendType::FidlClient; "fidl")]
    #[fuchsia::test]
    fn negotiate_channel_parameters_ignores_credits_if_flow_control_disabled(
        backend_type: ConnectionBackendType,
    ) {
        let mut multiplexer = SessionMultiplexer::create(900, backend_type);
        let dlci = DLCI::try_from(8).unwrap();

        // Negotiate NO credit-based flow control.
        let _ = multiplexer
            .negotiate_session_parameters(SessionParameters { credit_based_flow: false });
        assert!(!multiplexer.credit_based_flow());

        // Negotiate channel parameters with initial credits.
        let negotiated = multiplexer.negotiate_channel_parameters(
            dlci,
            DlcNegotiationParameters { max_frame_size: 500, initial_credits: 10 },
        );

        // Initial credits should be 0 because flow control is disabled.
        assert_eq!(negotiated.initial_credits, 0);
        // Max frame size should still be negotiated.
        assert_eq!(negotiated.max_frame_size, 500);
    }

    #[test_case(ConnectionBackendType::Socket; "socket")]
    #[test_case(ConnectionBackendType::FidlClient; "fidl")]
    #[fuchsia::test]
    fn negotiate_channel_parameters_respects_max_frame_size(backend_type: ConnectionBackendType) {
        let mut multiplexer = SessionMultiplexer::create(900, backend_type);
        let dlci = DLCI::try_from(8).unwrap();

        // Request larger than preferred
        let negotiated_parameters = multiplexer.negotiate_channel_parameters(
            dlci,
            DlcNegotiationParameters { max_frame_size: 1000, initial_credits: 10 },
        );
        // Should be capped at 900.
        assert_eq!(negotiated_parameters.max_frame_size, 900);
        let channel = multiplexer.get_session_channel(dlci).expect("channel should exist");
        assert_eq!(channel.max_packet_size(), Some(900));

        // Request smaller than preferred
        let negotiated_parameters2 = multiplexer.negotiate_channel_parameters(
            dlci,
            DlcNegotiationParameters { max_frame_size: 500, initial_credits: 10 },
        );
        // Should be 500.
        assert_eq!(negotiated_parameters2.max_frame_size, 500);
        let channel = multiplexer.get_session_channel(dlci).expect("channel should exist");
        assert_eq!(channel.max_packet_size(), Some(500));
    }

    #[test_case(ConnectionBackendType::Socket; "socket")]
    #[test_case(ConnectionBackendType::FidlClient; "fidl")]
    #[fuchsia::test]
    fn negotiate_channel_parameters_caps_large_peer_mtu(backend_type: ConnectionBackendType) {
        let mut multiplexer = SessionMultiplexer::create(32767, backend_type);
        let dlci = DLCI::try_from(8).unwrap();

        // Peer attempts to negotiate a super large max frame size.
        let negotiated_parameters = multiplexer.negotiate_channel_parameters(
            dlci,
            DlcNegotiationParameters { max_frame_size: 65535, initial_credits: 10 },
        );
        assert_eq!(negotiated_parameters.max_frame_size, 32767);
        let channel = multiplexer.get_session_channel(dlci).expect("channel should exist");
        assert_eq!(channel.max_packet_size(), Some(32767));
    }

    #[test_case(ConnectionBackendType::Socket; "socket")]
    #[test_case(ConnectionBackendType::FidlClient; "fidl")]
    #[fuchsia::test]
    fn start_multiplexer_multiple_times_is_error(backend_type: ConnectionBackendType) {
        const DEFAULT_MAX_TX: u16 = 900;
        let mut multiplexer = SessionMultiplexer::create(DEFAULT_MAX_TX, backend_type);
        multiplexer.start(Role::Initiator).expect("can start the multiplexer");
        assert!(multiplexer.started());
        let err_result = multiplexer.start(Role::Responder);
        assert_matches!(err_result, Err(Error::MultiplexerAlreadyStarted));
    }

    #[test_case(ConnectionBackendType::Socket; "socket")]
    #[test_case(ConnectionBackendType::FidlClient; "fidl")]
    #[fuchsia::test]
    async fn start_multiplexer_and_establish_dlci(backend_type: ConnectionBackendType) {
        const DEFAULT_MAX_TX: u16 = 900;
        let mut multiplexer = SessionMultiplexer::create(DEFAULT_MAX_TX, backend_type);
        multiplexer.start(Role::Initiator).expect("can start the multiplexer");
        assert!(multiplexer.started());
        assert!(!multiplexer.any_dlc_established());

        let dlci = DLCI::try_from(9).unwrap();
        let (sender, _receiver) = mpsc::channel(0);
        // Establish a channel with credit flow control.
        multiplexer
            .find_or_create_session_channel(dlci)
            .set_parameters(DEFAULT_MAX_TX, FlowControlMode::CreditBased(Credits::new(10, 10)))
            .expect("can set parameters");
        let mut user_rfcomm_channel =
            multiplexer.establish_session_channel(dlci, sender).expect("can register");
        assert_eq!(user_rfcomm_channel.connection_type(), backend_type);
        assert!(multiplexer.any_dlc_established());
        assert!(multiplexer.dlci_established(&dlci));

        // Can't set the flow control for a DLCI that has already been established.
        let result = multiplexer
            .find_or_create_session_channel(dlci)
            .set_parameters(DEFAULT_MAX_TX, FlowControlMode::None);
        assert_matches!(result, Err(Error::ChannelAlreadyEstablished(_)));

        // Data received from the peer should be forwarded to the `user_rfcomm_channel`.
        let data = FlowControlledData::new_no_credits(vec![4, 5, 6]);
        let result = multiplexer.receive_user_data(dlci, data).await;
        assert_matches!(result, Ok(_));
        let user_data_received = user_rfcomm_channel.next().await.expect("data received");
        assert_eq!(user_data_received, Ok(vec![4, 5, 6]));

        assert!(multiplexer.close_session_channel(&dlci));
        assert!(!multiplexer.dlci_established(&dlci));
    }

    #[test_case(ConnectionBackendType::Socket; "socket")]
    #[test_case(ConnectionBackendType::FidlClient; "fidl")]
    #[fuchsia::test]
    fn multiplexer_inspect_hierarchy(backend_type: ConnectionBackendType) {
        let mut exec = fuchsia_async::TestExecutor::new();
        let inspect = inspect::Inspector::default();

        // Setup multiplexer with inspect.
        let mut multiplexer = SessionMultiplexer::create(600, backend_type);
        multiplexer.iattach(inspect.root(), "multiplexer").expect("should attach to inspect tree");
        // Default inspect tree.
        assert_data_tree!(@executor exec, inspect, root: {
            multiplexer: {
                role: "Unassigned",
                default_max_packet_size: 600u64,
            },
        });

        // Reserving a channel should add to the inspect tree.
        let dlci = DLCI::try_from(9).unwrap();
        let _ = multiplexer.find_or_create_session_channel(dlci);
        assert_data_tree!(@executor exec, inspect, root: {
            multiplexer: {
                role: "Unassigned",
                default_max_packet_size: 600u64,
                channel_0: contains {
                    dlci: 9u64,
                }
            },
        });

        // Establishing a channel should add to the inspect tree. Multiplexer parameters are
        // negotiated to a default and updated in the inspect tree.
        let dlci2 = DLCI::try_from(20).unwrap();
        let (sender2, _receiver2) = mpsc::channel(0);
        let _channel2 = multiplexer.establish_session_channel(dlci2, sender2);
        assert_data_tree!(@executor exec, inspect, root: {
            multiplexer: {
                role: "Unassigned",
                flow_control: CREDIT_FLOW_CONTROL,
                default_max_packet_size: 600u64,
                channel_0: contains {
                    dlci: 9u64,
                },
                channel_1: contains {
                    dlci: 20u64,
                }
            },
        });

        // Removing a channel is OK. The lifetime of the `channel_*` node is tied to the
        // SessionChannel. This makes cleanup easy.
        assert!(multiplexer.close_session_channel(&dlci2));
        // The multiplexer closing the SessionChannel results in dropping the fasync::Task<()>
        // for the channel. In doing so, the RemoteHandle for the Task is dropped. The
        // associated future will only then be _woken up_ to be dropped by the executor.
        // This line of code runs the executor to complete the drop of the future. Only then
        // will the `channel_1` inspect node be removed from the tree.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        // Read hierarchy again to verify channel_1 is gone.
        assert_data_tree!(@executor exec, inspect, root: {
            multiplexer: {
                role: "Unassigned",
                flow_control: CREDIT_FLOW_CONTROL,
                default_max_packet_size: 600u64,
                channel_0: contains {
                    dlci: 9u64,
                },
            },
        });
    }
}
