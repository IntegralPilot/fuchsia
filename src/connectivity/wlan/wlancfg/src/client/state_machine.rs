// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::client::roaming::lib::{PolicyRoamRequest, ROAMING_CHANNEL_BUFFER_SIZE};
use crate::client::roaming::local_roam_manager::RoamManager;
use crate::client::roaming::roam_monitor::RoamDataSender;
use crate::client::types;
use crate::config_management::{Credential, PastConnectionData, SavedNetworksManagerApi};
use crate::mode_management::iface_manager_api::SmeForClientStateMachine;
use crate::mode_management::{Defect, IfaceFailure};
use crate::telemetry::{
    AVERAGE_SCORE_DELTA_MINIMUM_DURATION, DisconnectInfo, METRICS_SHORT_CONNECT_DURATION,
    TelemetryEvent, TelemetrySender,
};
use crate::util::listener::Message::NotifyListeners;
use crate::util::listener::{ClientListenerMessageSender, ClientNetworkState, ClientStateUpdate};
use crate::util::state_machine::{self, ExitReason, IntoStateExt, StateMachineStatusPublisher};
use anyhow::format_err;
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_internal as fidl_internal;
use fidl_fuchsia_wlan_policy as fidl_policy;
use fidl_fuchsia_wlan_sme as fidl_sme;
use fuchsia_async::{self as fasync, DurationExt};
use fuchsia_inspect::Node as InspectNode;
use fuchsia_inspect_contrib::inspect_insert;
use fuchsia_inspect_contrib::log::WriteInspect;
use futures::channel::{mpsc, oneshot};
use futures::future::{Fuse, FutureExt};
use futures::select;
use futures::stream::{self, StreamExt, TryStreamExt};
use log::{debug, error, info, warn};
use std::borrow::Cow;
use std::pin::Pin;
use std::sync::Arc;
use wlan_common::bss::BssDescription;
use wlan_common::channel::{Bandwidth, Channel};
use wlan_common::historical_list::HistoricalList;
use wlan_common::sequestered::Sequestered;

const MAX_CONNECTION_ATTEMPTS: u8 = 4; // arbitrarily chosen until we have some data
const NUM_PAST_SCORES: usize = 91; // number of past periodic connection scores to store for metrics
const PENDING_ROAM_TIMEOUT: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(3);

type State = state_machine::State<ExitReason>;
type ReqStream = stream::Fuse<mpsc::Receiver<ManualRequest>>;
pub type TrackedSignals = HistoricalList<types::TimestampedSignal, NUM_PAST_SCORES>;

#[derive(Clone)]
struct PendingRoam {
    pub request: PolicyRoamRequest,
    pub timestamp: fasync::MonotonicInstant,
}
impl From<PolicyRoamRequest> for PendingRoam {
    fn from(request: PolicyRoamRequest) -> Self {
        Self { request, timestamp: fasync::MonotonicInstant::now() }
    }
}

pub trait ClientApi {
    fn connect(&mut self, selection: types::ConnectSelection) -> Result<(), anyhow::Error>;
    fn disconnect(
        &mut self,
        reason: types::DisconnectReason,
        responder: oneshot::Sender<()>,
    ) -> Result<(), anyhow::Error>;

    /// Queries the liveness of the channel used to control the client state machine.  If the
    /// channel is not alive, this indicates that the client state machine has exited.
    fn is_alive(&self) -> bool;
}

pub struct Client {
    req_sender: mpsc::Sender<ManualRequest>,
}

impl Client {
    pub fn new(req_sender: mpsc::Sender<ManualRequest>) -> Self {
        Self { req_sender }
    }
}

impl ClientApi for Client {
    fn connect(&mut self, selection: types::ConnectSelection) -> Result<(), anyhow::Error> {
        self.req_sender
            .try_send(ManualRequest::Connect(Box::new(selection)))
            .map_err(|e| format_err!("failed to send connect selection: {:?}", e))
    }

    fn disconnect(
        &mut self,
        reason: types::DisconnectReason,
        responder: oneshot::Sender<()>,
    ) -> Result<(), anyhow::Error> {
        self.req_sender
            .try_send(ManualRequest::Disconnect((reason, responder)))
            .map_err(|e| format_err!("failed to send disconnect request: {:?}", e))
    }

    fn is_alive(&self) -> bool {
        !self.req_sender.is_closed()
    }
}

// TODO(https://fxbug.dev/324167674): fix.
pub enum ManualRequest {
    Connect(Box<types::ConnectSelection>),
    Disconnect((types::DisconnectReason, oneshot::Sender<()>)),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    Disconnecting,
    #[default]
    Disconnected,
    Connecting,
    Connected {
        channel: u8,
        rssi: i8,
        snr: i8,
    },
}

impl Status {
    fn from_ap_state(ap_state: &types::ApState) -> Self {
        Status::Connected {
            channel: ap_state.tracked.channel.primary,
            rssi: ap_state.tracked.signal.rssi_dbm,
            snr: ap_state.tracked.signal.snr_db,
        }
    }
}

impl WriteInspect for Status {
    fn write_inspect<'a>(&self, writer: &InspectNode, key: impl Into<Cow<'a, str>>) {
        match self {
            Status::Connected { channel, rssi, snr } => {
                inspect_insert!(writer, var key: {
                    Connected: {
                        channel: channel,
                        rssi: rssi,
                        snr: snr
                    }
                })
            }
            other => inspect_insert!(writer, var key: format!("{:?}", other)),
        }
    }
}

fn send_listener_state_update(
    sender: &ClientListenerMessageSender,
    network_update: Option<ClientNetworkState>,
) {
    let mut networks = vec![];
    if let Some(network) = network_update {
        networks.push(network)
    }

    let updates =
        ClientStateUpdate { state: fidl_policy::WlanClientState::ConnectionsEnabled, networks };
    match sender.clone().unbounded_send(NotifyListeners(updates)) {
        Ok(_) => (),
        Err(e) => error!("failed to send state update: {:?}", e),
    };
}

pub async fn serve(
    iface_id: u16,
    proxy: SmeForClientStateMachine,
    sme_event_stream: fidl_sme::ClientSmeEventStream,
    req_stream: mpsc::Receiver<ManualRequest>,
    update_sender: ClientListenerMessageSender,
    saved_networks_manager: Arc<dyn SavedNetworksManagerApi>,
    connect_selection: Option<types::ConnectSelection>,
    telemetry_sender: TelemetrySender,
    defect_sender: mpsc::Sender<Defect>,
    roam_manager: RoamManager,
    status_publisher: StateMachineStatusPublisher<Status>,
) {
    let next_network = connect_selection
        .map(|selection| ConnectingOptions { connect_selection: selection, attempt_counter: 0 });
    let disconnect_options = DisconnectingOptions {
        disconnect_responder: None,
        previous_network: None,
        next_network,
        reason: types::DisconnectReason::Startup,
    };
    let common_options = CommonStateOptions {
        proxy,
        req_stream: req_stream.fuse(),
        update_sender,
        saved_networks_manager,
        telemetry_sender: telemetry_sender.clone(),
        iface_id,
        defect_sender,
        roam_manager,
        status_publisher: status_publisher.clone(),
    };
    let state_machine =
        disconnecting_state(common_options, disconnect_options).into_state_machine();
    let removal_watcher = sme_event_stream.map_ok(|_| ()).try_collect::<()>();
    select! {
        state_machine = state_machine.fuse() => {
            match state_machine {
                Err(ExitReason(Err(e))) => error!("Client state machine for iface #{} terminated with an error: {:?}",
                    iface_id, e),
                Err(ExitReason(Ok(_))) => info!("Client state machine for iface #{} exited gracefully",
                    iface_id,),
            }
        }
        removal_watcher = removal_watcher.fuse() => {
            match removal_watcher {
                Ok(()) => {
                    info!("Device was unexpectedly removed.");
                }
                Err(e) => {
                    info!("Error reading from Client SME channel of iface #{}: {:?}",
                        iface_id, e);
                }
            }

            telemetry_sender
                .send(TelemetryEvent::Disconnected { track_subsequent_downtime: false, info: None });
        },
    }

    status_publisher.publish_status(Status::Disconnected);
}

/// Common parameters passed to all states
struct CommonStateOptions {
    proxy: SmeForClientStateMachine,
    req_stream: ReqStream,
    update_sender: ClientListenerMessageSender,
    saved_networks_manager: Arc<dyn SavedNetworksManagerApi>,
    telemetry_sender: TelemetrySender,
    iface_id: u16,
    defect_sender: mpsc::Sender<Defect>,
    roam_manager: RoamManager,
    status_publisher: StateMachineStatusPublisher<Status>,
}

impl CommonStateOptions {
    async fn network_is_likely_hidden(&self, options: &ConnectingOptions) -> bool {
        match self
            .saved_networks_manager
            .lookup(&options.connect_selection.target.network)
            .await
            .filter(|config| config.credential == options.connect_selection.target.credential)
        {
            Some(config) => config.is_hidden(),
            None => {
                error!("Could not lookup if connected network is hidden.");
                false
            }
        }
    }
}

pub type ConnectionStatsSender = mpsc::UnboundedSender<fidl_internal::SignalReportIndication>;
pub type ConnectionStatsReceiver = mpsc::UnboundedReceiver<fidl_internal::SignalReportIndication>;

fn handle_none_request() -> Result<State, ExitReason> {
    Err(ExitReason(Err(format_err!("The stream of requests ended unexpectedly"))))
}

// These functions were introduced to resolve the following error:
// ```
// error[E0391]: cycle detected when evaluating trait selection obligation
// `impl core::future::future::Future: std::marker::Send`
// ```
// which occurs when two functions that return an `impl Trait` call each other
// in a cycle. (e.g. this case `connecting_state` calling `disconnecting_state`,
// which calls `connecting_state`)
fn to_disconnecting_state(
    common_options: CommonStateOptions,
    disconnecting_options: DisconnectingOptions,
) -> State {
    disconnecting_state(common_options, disconnecting_options).into_state()
}
fn to_connecting_state(
    common_options: CommonStateOptions,
    connecting_options: ConnectingOptions,
) -> State {
    connecting_state(common_options, connecting_options).into_state()
}

struct DisconnectingOptions {
    disconnect_responder: Option<oneshot::Sender<()>>,
    /// Information about the previously connected network, if there was one. Used to send out
    /// listener updates.
    previous_network: Option<(types::NetworkIdentifier, types::DisconnectStatus)>,
    /// Configuration for the next network to connect to, after the disconnect is complete. If not
    /// present, the state machine will proceed to IDLE.
    next_network: Option<ConnectingOptions>,
    reason: types::DisconnectReason,
}
/// The DISCONNECTING state requests an SME disconnect, then transitions to either:
/// - the CONNECTING state if options.next_network is present
/// - exit otherwise
async fn disconnecting_state(
    common_options: CommonStateOptions,
    mut options: DisconnectingOptions,
) -> Result<State, ExitReason> {
    // Log a message with the disconnect reason
    match options.reason {
        types::DisconnectReason::FailedToConnect
        | types::DisconnectReason::Startup
        | types::DisconnectReason::DisconnectDetectedFromSme => {
            // These are either just noise or have separate logging, so keep the level at debug.
            debug!("Disconnected due to {:?}", options.reason);
        }
        reason => {
            info!("Disconnected due to {:?}", reason);
        }
    }

    notify_on_disconnect_attempt(&common_options);

    // TODO(https://fxbug.dev/42130926): either make this fire-and-forget in the SME, or spawn a thread for this,
    // so we don't block on it
    common_options
        .proxy
        .disconnect(types::convert_to_sme_disconnect_reason(options.reason))
        .await
        .map_err(|e| ExitReason(Err(e)))?;

    notify_once_disconnected(&common_options, &mut options);

    // Transition to next state
    match options.next_network {
        Some(next_network) => Ok(to_connecting_state(common_options, next_network)),
        None => Err(ExitReason(Ok(()))),
    }
}

fn notify_on_disconnect_attempt(common_options: &CommonStateOptions) {
    common_options.status_publisher.publish_status(Status::Disconnecting);
}

fn notify_once_disconnected(
    common_options: &CommonStateOptions,
    options: &mut DisconnectingOptions,
) {
    common_options.status_publisher.publish_status(Status::Disconnected);

    // Notify listeners if a disconnect request was sent, or ensure that listeners know client
    // connections are enabled.
    let networks =
        options.previous_network.clone().map(|(network_identifier, status)| ClientNetworkState {
            id: network_identifier,
            state: types::ConnectionState::Disconnected,
            status: Some(status),
        });
    send_listener_state_update(&common_options.update_sender, networks);

    // Notify the caller that disconnect was sent to the SME once the final disconnected update has
    // been sent.  This ensures that there will not be a race when the IfaceManager sends out a
    // ConnectionsDisabled update.
    #[allow(clippy::single_match, reason = "mass allow for https://fxbug.dev/381896734")]
    match options.disconnect_responder.take() {
        Some(responder) => responder.send(()).unwrap_or(()),
        None => (),
    }
}

struct ConnectingOptions {
    connect_selection: types::ConnectSelection,
    /// Count of previous consecutive failed connection attempts to this same network.
    attempt_counter: u8,
}

async fn handle_connecting_error_and_retry(
    common_options: CommonStateOptions,
    options: ConnectingOptions,
) -> Result<State, ExitReason> {
    // Check if the limit for connection attempts to this network has been
    // exceeded.
    let new_attempt_count = options.attempt_counter + 1;
    if new_attempt_count >= MAX_CONNECTION_ATTEMPTS {
        info!("Exceeded maximum connection attempts, will not retry");
        send_listener_state_update(
            &common_options.update_sender,
            Some(ClientNetworkState {
                id: options.connect_selection.target.network,
                state: types::ConnectionState::Failed,
                status: Some(types::DisconnectStatus::ConnectionFailed),
            }),
        );
        Err(ExitReason(Ok(())))
    } else {
        // Limit not exceeded, retry after backing off.
        let backoff_time = 400_i64 * i64::from(new_attempt_count);
        info!("Will attempt to reconnect after {}ms backoff", backoff_time);
        fasync::Timer::new(zx::MonotonicDuration::from_millis(backoff_time).after_now()).await;

        let next_connecting_options = ConnectingOptions {
            connect_selection: types::ConnectSelection {
                reason: types::ConnectReason::RetryAfterFailedConnectAttempt,
                ..options.connect_selection
            },
            attempt_counter: new_attempt_count,
        };
        let disconnecting_options = DisconnectingOptions {
            disconnect_responder: None,
            previous_network: None,
            next_network: Some(next_connecting_options),
            reason: types::DisconnectReason::FailedToConnect,
        };
        Ok(to_disconnecting_state(common_options, disconnecting_options))
    }
}

#[allow(clippy::needless_return, reason = "mass allow for https://fxbug.dev/381896734")]
/// The CONNECTING state requests an SME connect. It handles the SME connect response:
/// - for a successful connection, transition to CONNECTED state
/// - for a failed connection, retry connection by passing a next_network to the
///   DISCONNECTING state, as long as there haven't been too many connection attempts
#[allow(clippy::doc_lazy_continuation, reason = "mass allow for https://fxbug.dev/381896734")]
/// During this time, incoming ManualRequests are also monitored for:
/// - duplicate connect requests are deduped
/// - different connect requests are serviced by passing a next_network to the DISCONNECTING state
/// - disconnect requests cause a transition to DISCONNECTING state
async fn connecting_state(
    mut common_options: CommonStateOptions,
    options: ConnectingOptions,
) -> Result<State, ExitReason> {
    debug!("Entering connecting state");
    notify_on_connection_attempt(&common_options, &options);

    // Release the sequestered BSS description. While considered a "black box" elsewhere, the state
    // machine uses this by design to construct its AP state and to report telemetry.
    let bss_description =
        Sequestered::release(options.connect_selection.target.bss.bss_description.clone());
    let ap_state = types::ApState::from(
        BssDescription::try_from(bss_description.clone()).map_err(|error| {
            // This only occurs if an invalid `BssDescription` is received from SME, which should
            // never happen.
            ExitReason(Err(
                format_err!("Failed to convert BSS description from FIDL: {:?}", error,),
            ))
        })?,
    );

    let sme_connect_request = fidl_sme::ConnectRequest {
        ssid: options.connect_selection.target.network.ssid.to_vec(),
        bss_description,
        multiple_bss_candidates: options.connect_selection.target.network_has_multiple_bss,
        authentication: options.connect_selection.target.authenticator.clone().into(),
        deprecated_scan_type: fidl_fuchsia_wlan_common::ScanType::Active,
    };
    let (sme_result, connect_txn_stream) = common_options
        .proxy
        .connect(&sme_connect_request)
        .await
        .map_err(|e| ExitReason(Err(format_err!("{:?}", e))))?;

    notify_on_connection_result(&mut common_options, &options, ap_state.clone(), sme_result).await;

    match (sme_result.code, sme_result.is_credential_rejected) {
        (fidl_ieee80211::StatusCode::Success, _) => {
            info!("Successfully connected to network");
            let network_is_likely_hidden = common_options.network_is_likely_hidden(&options).await;
            let connected_options = ConnectedOptions::new(
                &mut common_options,
                Box::new(ap_state.clone()),
                options.connect_selection.target.network_has_multiple_bss,
                options.connect_selection.target.network.clone(),
                options.connect_selection.target.credential.clone(),
                options.connect_selection.reason,
                connect_txn_stream,
                network_is_likely_hidden,
            );
            Ok(connected_state(common_options, connected_options).into_state())
        }
        (code, true) => {
            info!("Failed to connect: {:?}. Will not retry because of credential error.", code);
            return Err(ExitReason(Ok(())));
        }
        (code, _) => {
            info!("Failed to connect: {:?}", code);
            handle_connecting_error_and_retry(common_options, options).await
        }
    }
}

fn notify_on_connection_attempt(common_options: &CommonStateOptions, options: &ConnectingOptions) {
    common_options.status_publisher.publish_status(Status::Connecting);

    if options.attempt_counter > 0 {
        info!(
            "Retrying connection, {} attempts remaining",
            MAX_CONNECTION_ATTEMPTS - options.attempt_counter
        );
    }

    // Send a "Connecting" update to listeners, unless this is a retry
    if options.attempt_counter == 0 {
        send_listener_state_update(
            &common_options.update_sender,
            Some(ClientNetworkState {
                id: options.connect_selection.target.network.clone(),
                state: types::ConnectionState::Connecting,
                status: None,
            }),
        );
    };
}

async fn notify_on_connection_result(
    common_options: &mut CommonStateOptions,
    options: &ConnectingOptions,
    ap_state: types::ApState,
    sme_result: fidl_sme::ConnectResult,
) {
    common_options.status_publisher.publish_status(Status::from_ap_state(&ap_state));

    // Report the connect result to the saved networks manager.
    common_options
        .saved_networks_manager
        .record_connect_result(
            options.connect_selection.target.network.clone(),
            &options.connect_selection.target.credential,
            ap_state.clone().original().bssid,
            sme_result,
            options.connect_selection.target.bss.observation,
        )
        .await;

    let network_is_likely_hidden = common_options.network_is_likely_hidden(options).await;
    common_options.telemetry_sender.send(TelemetryEvent::ConnectResult {
        ap_state,
        result: sme_result,
        policy_connect_reason: Some(options.connect_selection.reason),
        multiple_bss_candidates: options.connect_selection.target.network_has_multiple_bss,
        iface_id: common_options.iface_id,
        network_is_likely_hidden,
    });

    if sme_result.code == fidl_ieee80211::StatusCode::Success {
        send_listener_state_update(
            &common_options.update_sender,
            Some(ClientNetworkState {
                id: options.connect_selection.target.network.clone(),
                state: types::ConnectionState::Connected,
                status: None,
            }),
        );
    } else if sme_result.is_credential_rejected {
        send_listener_state_update(
            &common_options.update_sender,
            Some(ClientNetworkState {
                id: options.connect_selection.target.network.clone(),
                state: types::ConnectionState::Failed,
                status: Some(types::DisconnectStatus::CredentialsFailed),
            }),
        );
    } else {
        // Defects should be logged for connection failures that are not due to bad credentials.
        if let Err(e) =
            common_options.defect_sender.try_send(Defect::Iface(IfaceFailure::ConnectionFailure {
                iface_id: common_options.iface_id,
            }))
        {
            warn!("Failed to log connection failure: {}", e);
        }
    }
}

struct ConnectedOptions {
    // Keep track of the BSSID we are connected in order to record connection information for
    // future network selection.
    ap_state: Box<types::ApState>,
    multiple_bss_candidates: bool,
    network_identifier: types::NetworkIdentifier,
    credential: Credential,
    ess_connect_reason: types::ConnectReason,
    connect_txn_stream: fidl_sme::ConnectTransactionEventStream,
    network_is_likely_hidden: bool,
    ess_connect_start_time: fasync::MonotonicInstant,
    bss_connect_start_time: fasync::MonotonicInstant,
    initial_signal: types::Signal,
    tracked_signals: TrackedSignals,
    roam_monitor_sender: RoamDataSender,
    roam_request_receiver: mpsc::Receiver<PolicyRoamRequest>,
    post_connect_metric_timer: Pin<Box<fasync::Timer>>,
    bss_connect_duration_metric_timer: Pin<Box<fasync::Timer>>,
    pending_roam: Option<PendingRoam>,
    pending_roam_timer: Pin<Box<Fuse<fasync::Timer>>>,
}
impl ConnectedOptions {
    pub fn new(
        common_options: &mut CommonStateOptions,
        ap_state: Box<types::ApState>,
        multiple_bss_candidates: bool,
        network_identifier: types::NetworkIdentifier,
        credential: Credential,
        ess_connect_reason: types::ConnectReason,
        connect_txn_stream: fidl_sme::ConnectTransactionEventStream,
        network_is_likely_hidden: bool,
    ) -> Self {
        // Tracked signals
        let mut past_signals = TrackedSignals::new();
        let initial_signal = ap_state.tracked.signal;
        past_signals.add(types::TimestampedSignal {
            time: fasync::MonotonicInstant::now(),
            signal: initial_signal,
        });

        // Initialize roam monitor with roam manager service.
        let (roam_request_sender, roam_request_receiver) =
            mpsc::channel(ROAMING_CHANNEL_BUFFER_SIZE);
        let roam_monitor_sender = common_options.roam_manager.initialize_roam_monitor(
            (*ap_state).clone(),
            network_identifier.clone(),
            credential.clone(),
            roam_request_sender,
        );
        Self {
            ap_state,
            multiple_bss_candidates,
            network_identifier,
            credential,
            ess_connect_reason,
            connect_txn_stream,
            network_is_likely_hidden,
            ess_connect_start_time: fasync::MonotonicInstant::now(),
            bss_connect_start_time: fasync::MonotonicInstant::now(),
            initial_signal,
            tracked_signals: TrackedSignals::new(),
            roam_monitor_sender,
            roam_request_receiver,
            post_connect_metric_timer: Box::pin(fasync::Timer::new(
                AVERAGE_SCORE_DELTA_MINIMUM_DURATION.after_now(),
            )),
            bss_connect_duration_metric_timer: Box::pin(fasync::Timer::new(
                METRICS_SHORT_CONNECT_DURATION.after_now(),
            )),
            pending_roam: None,
            pending_roam_timer: Box::pin(Fuse::terminated()),
        }
    }
}
/// The CONNECTED state monitors the SME status. It handles the SME status response:
/// - if still connected to the correct network, no action
/// - if disconnected, retry connection by passing a next_network to the
///   DISCONNECTING state
#[allow(clippy::doc_lazy_continuation, reason = "mass allow for https://fxbug.dev/381896734")]
/// During this time, incoming ManualRequests are also monitored for:
/// - duplicate connect requests are deduped
/// - different connect requests are serviced by passing a next_network to the DISCONNECTING state
/// - disconnect requests cause a transition to DISCONNECTING state
async fn connected_state(
    mut common_options: CommonStateOptions,
    mut options: ConnectedOptions,
) -> Result<State, ExitReason> {
    debug!("Entering connected state");
    loop {
        select! {
            event = options.connect_txn_stream.next() => match event {
                Some(Ok(event)) => {
                    let is_sme_idle = match event {
                        fidl_sme::ConnectTransactionEvent::OnDisconnect { info: fidl_info } => {
                            notify_when_disconnect_detected(
                                &common_options,
                                &options,
                                fidl_info,
                            ).await;

                            !fidl_info.is_sme_reconnecting
                        }
                        fidl_sme::ConnectTransactionEvent::OnConnectResult { result } => {
                            let connected = result.code == fidl_ieee80211::StatusCode::Success;
                            if connected {
                                // This OnConnectResult should be for SME reconnecting to the same
                                // AP, so keep the same SignalData but reset the connect start time
                                // to track as a new connection.
                                options.ess_connect_start_time = fasync::MonotonicInstant::now();
                                options.bss_connect_start_time = fasync::MonotonicInstant::now();
                            }
                            notify_when_reconnect_detected(&common_options, &options, result);
                            !connected
                        }
                        fidl_sme::ConnectTransactionEvent::OnRoamResult { result } => {
                            if let Err(error) = handle_roam_result(&mut common_options, &mut options, &result).await {
                                error!("Error handling roam result: {:?}. Cannot proceed with connection.", error);
                                true
                            } else {
                                let connected = result.status_code == fidl_ieee80211::StatusCode::Success || result.original_association_maintained;
                                !connected
                            }
                        }
                        fidl_sme::ConnectTransactionEvent::OnSignalReport { ind } => {
                            // Update connection data
                            options.ap_state.tracked.signal = ind.into();

                            // Update list of signals
                            options.tracked_signals.add(types::TimestampedSignal {
                                time: fasync::MonotonicInstant::now(),
                                signal: ind.into(),
                            });

                            notify_on_signal_report(
                                &common_options,
                                &mut options,
                                ind
                            );
                            false
                        }
                        fidl_sme::ConnectTransactionEvent::OnChannelSwitched { info } => {
                            info!(
                                "OnChannelSwitch received. Previous channel: {:?}, new channel: {:?}.",
                                options.ap_state.tracked.channel.primary, info.new_primary_channel
                            );

                            let cbw = match Bandwidth::from_fidl(info.bandwidth, info.vht_secondary_80_channel.number) {
                                Ok(cbw) => cbw,
                                Err(e) => {
                                    // In the event that the CBW is invalid, reuse the previous CBW
                                    // in determining the client's channel to preserve any legacy
                                    // behavior.
                                    error!("Invalid CBW in ChannelSwitchInfo: {}", e);
                                    options.ap_state.tracked.channel.bandwidth
                                }
                            };
                            options.ap_state.tracked.channel = Channel::new(
                                info.new_primary_channel.number,
                                cbw,
                                info.new_primary_channel.band
                            );

                            // Re-initialize roam monitor for new channel
                            let (sender, roam_request_receiver) = mpsc::channel(ROAMING_CHANNEL_BUFFER_SIZE);
                            options.roam_request_receiver = roam_request_receiver;
                            options.roam_monitor_sender =
                                common_options.roam_manager.initialize_roam_monitor(
                                    (*options.ap_state).clone(),
                                    options.network_identifier.clone(),
                                    options.credential.clone(),
                                    sender
                                );
                            notify_on_channel_switch(&common_options, &options, info);
                            false
                        }
                    };

                    if is_sme_idle {
                        info!("Idle sme detected.");
                        let options = DisconnectingOptions {
                            disconnect_responder: None,
                            previous_network: Some((
                                options.network_identifier.clone(),
                                types::DisconnectStatus::ConnectionFailed
                            )),
                            next_network: None,
                            reason: types::DisconnectReason::DisconnectDetectedFromSme,
                        };
                        return Ok(disconnecting_state(common_options, options).into_state());
                    }
                }
                _ => {
                    info!("SME dropped ConnectTransaction channel. Exiting state machine");
                    return Err(ExitReason(Err(format_err!("Failed to receive ConnectTransactionEvent for SME status"))));
                }
            },
            req = common_options.req_stream.next() => {
                match req {
                    Some(ManualRequest::Disconnect((reason, responder))) => {
                        debug!("Disconnect requested");
                        notify_on_manual_disconnect_request_received(
                            &common_options,
                            &options,
                            reason,
                        ).await;

                        let options = DisconnectingOptions {
                            disconnect_responder: Some(responder),
                            previous_network: Some((
                                options.network_identifier.clone(),
                                types::DisconnectStatus::ConnectionStopped
                            )),
                            next_network: None,
                            reason,
                        };
                        return Ok(disconnecting_state(common_options, options).into_state());
                    }
                    Some(ManualRequest::Connect(box_connect_selection)) => {
                        let new_connect_selection = *box_connect_selection;
                        // Check if it's the same network as we're currently connected to. If yes, reply immediately
                        if new_connect_selection.target.network
                            == options.network_identifier {
                            info!("Received connection request for current network, deduping");
                            continue
                        }

                        let reason = convert_manual_connect_to_disconnect_reason(
                            &new_connect_selection.reason
                        ).unwrap_or_else(|_| {
                            error!("Unexpected connection reason: {:?}", new_connect_selection.reason);
                            types::DisconnectReason::Unknown
                        });
                        notify_on_manual_connect_request_received(
                            &common_options,
                            &options,
                            reason,
                        ).await;

                        let options = DisconnectingOptions {
                            disconnect_responder: None,
                            previous_network: Some((
                                options.network_identifier,
                                types::DisconnectStatus::ConnectionStopped
                            )),
                            next_network: Some(ConnectingOptions {
                                connect_selection: new_connect_selection.clone(),
                                attempt_counter: 0,
                            }),
                            reason
                        };
                        info!("Connection to new network requested, disconnecting from current network");
                        return Ok(disconnecting_state(common_options, options).into_state())
                    }
                    None => return handle_none_request(),
                };
            },
            () = &mut options.post_connect_metric_timer => {
                common_options.telemetry_sender.send(TelemetryEvent::PostConnectionSignals {
                        connect_time: options.bss_connect_start_time,
                        signal_at_connect: options.initial_signal,
                        signals: options.tracked_signals.clone()
                });
            },
            () = &mut options.bss_connect_duration_metric_timer => {
                // Log the average connection score metric for a long duration BSS connection.
                common_options.telemetry_sender.send(TelemetryEvent::LongDurationSignals{
                    signals: options.tracked_signals.get_before(fasync::MonotonicInstant::now())
                });
            },
            () = &mut options.pending_roam_timer => {
                error!("Pending roam request has timed out without a response from SME, cannot proceed with connection");
                notify_on_roam_error_and_exit(&common_options, &options).await;
                let options = DisconnectingOptions {
                    disconnect_responder: None,
                    previous_network: Some((
                        options.network_identifier.clone(),
                        types::DisconnectStatus::ConnectionFailed
                    )),
                    next_network: None,
                    // TODO(b/405151253): Add a disconnect reason for errors attempting to roam.
                    reason: types::DisconnectReason::DisconnectDetectedFromSme,
                };
                return Ok(disconnecting_state(common_options, options).into_state());
            }
            roam_request = options.roam_request_receiver.select_next_some() => {
                if let Some(pending_roam) = &options.pending_roam {
                    info!("Already pending a roam result for a requested roam to BSSID: {:?}", pending_roam.request.candidate.bss.bssid);
                } else {
                    let _ = common_options
                    .proxy
                    .roam(&roam_request.clone().into())
                    .inspect_err(|e| {
                        error!("Error sending sme roam request: {}", e);
                    });
                    options.pending_roam = Some(roam_request.clone().into());
                    options.pending_roam_timer.set(fasync::Timer::new(PENDING_ROAM_TIMEOUT.after_now()).fuse());
                    common_options.telemetry_sender.send(TelemetryEvent::PolicyRoamAttempt {
                        request: roam_request,
                        connected_duration: fasync::MonotonicInstant::now() - options.bss_connect_start_time,
                    });
                }
            }
        }
    }
}

async fn notify_when_disconnect_detected(
    common_options: &CommonStateOptions,
    options: &ConnectedOptions,
    fidl_info: fidl_sme::DisconnectInfo,
) {
    log_disconnect_to_telemetry(common_options, options, fidl_info, true).await;
    log_disconnect_to_config_manager(
        common_options,
        options,
        types::DisconnectReason::DisconnectDetectedFromSme,
    )
    .await
}

fn notify_when_reconnect_detected(
    common_options: &CommonStateOptions,
    options: &ConnectedOptions,
    result: fidl_sme::ConnectResult,
) {
    common_options.telemetry_sender.send(TelemetryEvent::ConnectResult {
        iface_id: common_options.iface_id,
        result,
        policy_connect_reason: None,
        // It's not necessarily true that there are still multiple BSS
        // candidates in the network at this point in time, but we use the
        // heuristic that if previously there were multiple BSS's, then
        // it likely remains the same.
        multiple_bss_candidates: options.multiple_bss_candidates,
        ap_state: (*options.ap_state).clone(),
        network_is_likely_hidden: options.network_is_likely_hidden,
    });
}

fn notify_on_signal_report(
    common_options: &CommonStateOptions,
    options: &mut ConnectedOptions,
    ind: fidl_internal::SignalReportIndication,
) {
    // Update reported state.
    common_options.status_publisher.publish_status(Status::from_ap_state(&options.ap_state));

    // Send signal report metrics
    common_options.telemetry_sender.send(TelemetryEvent::OnSignalReport { ind });

    // Forward signal report data to roam monitor.
    let _ = options
        .roam_monitor_sender
        .send_signal_report_ind(ind)
        .inspect_err(|e| error!("Error handling signal report: {}", e));
}

fn notify_on_channel_switch(
    common_options: &CommonStateOptions,
    options: &ConnectedOptions,
    info: fidl_internal::ChannelSwitchInfo,
) {
    common_options.telemetry_sender.send(TelemetryEvent::OnChannelSwitched { info });
    // Update reported state.
    common_options.status_publisher.publish_status(Status::from_ap_state(&options.ap_state));
}

async fn notify_on_manual_disconnect_request_received(
    common_options: &CommonStateOptions,
    options: &ConnectedOptions,
    reason: types::DisconnectReason,
) {
    let fidl_info = fidl_sme::DisconnectInfo {
        is_sme_reconnecting: false,
        disconnect_source: fidl_sme::DisconnectSource::User(
            types::convert_to_sme_disconnect_reason(reason),
        ),
    };
    log_disconnect_to_telemetry(common_options, options, fidl_info, false).await;
    log_disconnect_to_config_manager(common_options, options, reason).await
}

async fn notify_on_manual_connect_request_received(
    common_options: &CommonStateOptions,
    options: &ConnectedOptions,
    reason: types::DisconnectReason,
) {
    let fidl_info = fidl_sme::DisconnectInfo {
        is_sme_reconnecting: false,
        disconnect_source: fidl_sme::DisconnectSource::User(
            types::convert_to_sme_disconnect_reason(reason),
        ),
    };
    log_disconnect_to_telemetry(common_options, options, fidl_info, false).await;
    log_disconnect_to_config_manager(common_options, options, reason).await
}

/// On a roam success:
///   - logs a disconnect from the original BSS to saved networks manager
///   - updates the state machine internal state (see update_internal_state_on_roam_success)
///
/// On a roam failure and original association not maintained:
///   - logs a disconnect from the original BSS to saved networks manager
///   - logs a disconnect to telemetry
///
/// Then the connect attempt, roam result, and any defects are logged (see notify_on_roam_result)
async fn handle_roam_result(
    common_options: &mut CommonStateOptions,
    options: &mut ConnectedOptions,
    result: &fidl_sme::RoamResult,
) -> Result<(), anyhow::Error> {
    // TODO(b/399649753): Distinguish between policy-initaited and firmware-initiated RoamResults
    // here, when possible. Only correlate policy-initiated RoamResults with pending roams.

    // Verify that the received roam result matches the pending roam request. If there is not a
    // pending roam request, or the pending request is for another BSS, the connection cannot
    // proceed.
    let pending_roam = match options.pending_roam.take() {
        None => {
            notify_on_roam_error_and_exit(common_options, options).await;
            return Err(format_err!(
                "Roam result unexpectedly received without a pending roam request."
            ));
        }
        Some(pending_roam) => {
            if pending_roam.request.candidate.bss.bssid != result.bssid.into() {
                notify_on_roam_error_and_exit(common_options, options).await;
                return Err(format_err!(
                    "Roam result received for bssid {:?} while awaiting result for bssid {:?}.",
                    result.bssid,
                    pending_roam.request.candidate.bss.bssid
                ));
            }
            pending_roam
        }
    };
    options.pending_roam_timer.set(Fuse::terminated());

    let roam_succeeded = result.status_code == fidl_ieee80211::StatusCode::Success;
    let original_ap_state = options.ap_state.clone();
    if roam_succeeded {
        // Record BSS disconnect to config manager. We do not report a disconnect to
        // telemetry, since we are still connected to the same ESS.
        log_disconnect_to_config_manager(common_options, options, types::DisconnectReason::Unknown)
            .await;
        // Update internals of connected state to proceed with connection.
        update_internal_state_on_roam_success(common_options, options, result)?;
        info!("Roam succeeded!");
    } else if !result.original_association_maintained {
        // RoamResult should always include disconnect_info on failure, but this check prevents
        // issues if it's missing.
        let sme_disconnect_info = match result.disconnect_info {
            Some(ref info) => *info.clone(),
            None => {
                warn!("RoamResult failure does not contain SME disconnect info.");
                fidl_sme::DisconnectInfo {
                    is_sme_reconnecting: false,
                    disconnect_source: fidl_sme::DisconnectSource::Mlme(
                        fidl_sme::DisconnectCause {
                            reason_code: fidl_ieee80211::ReasonCode::UnspecifiedReason,
                            mlme_event_name:
                                fidl_sme::DisconnectMlmeEventName::RoamResultIndication,
                        },
                    ),
                }
            }
        };
        // Record a disconnect to both config manager and telemetry.
        log_disconnect_to_telemetry(common_options, options, sme_disconnect_info, true).await;
        log_disconnect_to_config_manager(common_options, options, types::DisconnectReason::Unknown)
            .await;
        info!("Roam attempt failed, original association not maintained, disconnecting");
    }
    notify_on_roam_result(common_options, options, result, *original_ap_state, pending_roam).await;
    Ok(())
}

/// Called for each roam result, regardless of success/failure, after any required
/// internal state updates have been made. This function:
///   - Records the connect attempt to the target BSS with saved networks manager
///   - Logs the roam result to telemetry
///   - Publishes the current ap state (which may have been updates)
///   - Logs a connect failure defect, if applicable.
async fn notify_on_roam_result(
    common_options: &mut CommonStateOptions,
    options: &mut ConnectedOptions,
    result: &fidl_sme::RoamResult,
    original_ap_state: types::ApState,
    pending_roam: PendingRoam,
) {
    // Record connect result to config manager, regardless of status.
    common_options
        .saved_networks_manager
        .record_connect_result(
            options.network_identifier.clone(),
            &options.credential,
            result.bssid.into(),
            fidl_sme::ConnectResult {
                code: result.status_code,
                is_credential_rejected: result.is_credential_rejected,
                is_reconnect: match result.disconnect_info {
                    Some(ref info) => info.is_sme_reconnecting,
                    None => false,
                },
            },
            types::ScanObservation::Unknown,
        )
        .await;

    // Log policy-initiated roam result to telemetry once state is up to date.
    common_options.telemetry_sender.send(TelemetryEvent::PolicyInitiatedRoamResult {
        iface_id: common_options.iface_id,
        result: result.clone(),
        updated_ap_state: (*options.ap_state).clone(),
        original_ap_state: Box::new(original_ap_state),
        request: Box::new(pending_roam.request.clone()),
        request_time: pending_roam.timestamp,
        result_time: fasync::MonotonicInstant::now(),
    });

    // Publish state.
    common_options.status_publisher.publish_status(Status::from_ap_state(&options.ap_state));

    // Log defect on connect failure.
    if result.status_code != fidl_ieee80211::StatusCode::Success
        && !result.is_credential_rejected
        && let Err(e) =
            common_options.defect_sender.try_send(Defect::Iface(IfaceFailure::ConnectionFailure {
                iface_id: common_options.iface_id,
            }))
    {
        warn!("Failed to log connection failure: {}", e);
    }
}

async fn notify_on_roam_error_and_exit(
    common_options: &CommonStateOptions,
    options: &ConnectedOptions,
) {
    // TODO(b/405151253): Add a disconnect reason for errors attempting to roam.
    let fidl_info = fidl_sme::DisconnectInfo {
        is_sme_reconnecting: false,
        disconnect_source: fidl_sme::DisconnectSource::User(
            fidl_sme::UserDisconnectReason::Unknown,
        ),
    };
    log_disconnect_to_telemetry(common_options, options, fidl_info, false).await;
    log_disconnect_to_config_manager(common_options, options, types::DisconnectReason::Unknown)
        .await;

    // Clear the network from the updates stream, since this state machine will exit.
    let networks = Some(ClientNetworkState {
        id: options.network_identifier.clone(),
        state: types::ConnectionState::Disconnected,
        status: Some(types::DisconnectStatus::ConnectionFailed),
    });
    send_listener_state_update(&common_options.update_sender, networks);
}

async fn log_disconnect_to_telemetry(
    common_options: &CommonStateOptions,
    options: &ConnectedOptions,
    fidl_info: fidl_sme::DisconnectInfo,
    track_subsequent_downtime: bool,
) {
    let now = fasync::MonotonicInstant::now();
    let info = DisconnectInfo {
        iface_id: common_options.iface_id,
        connected_duration: now - options.ess_connect_start_time,
        is_sme_reconnecting: fidl_info.is_sme_reconnecting,
        disconnect_source: fidl_info.disconnect_source,
        previous_connect_reason: options.ess_connect_reason,
        ap_state: (*options.ap_state).clone(),
        signals: options.tracked_signals.clone(),
    };
    common_options
        .telemetry_sender
        .send(TelemetryEvent::Disconnected { track_subsequent_downtime, info: Some(info) });
}

async fn log_disconnect_to_config_manager(
    common_options: &CommonStateOptions,
    options: &ConnectedOptions,
    reason: types::DisconnectReason,
) {
    let curr_time = fasync::MonotonicInstant::now();
    let uptime = curr_time - options.bss_connect_start_time;
    let data = PastConnectionData::new(
        options.ap_state.original().bssid,
        curr_time,
        uptime,
        reason,
        options.ap_state.tracked.signal,
        // TODO: record average phy rate over connection once available
        0,
    );
    common_options
        .saved_networks_manager
        .record_disconnect(&options.network_identifier.clone(), &options.credential, data)
        .await;
}

/// Updates all internal state following a roam to a new BSS. This includes updating the ap state,
/// restarting metrics timers, clearing signal tracking, and re-initializing a new roam monitor.
fn update_internal_state_on_roam_success(
    common_options: &mut CommonStateOptions,
    options: &mut ConnectedOptions,
    result: &fidl_sme::RoamResult,
) -> Result<(), anyhow::Error> {
    // Update internal state, to proceed with connection.
    let bss_description = match result.bss_description {
        Some(ref bss_description) => bss_description,
        None => {
            return Err(format_err!("RoamResult is missing BSS description from FIDL"));
        }
    };
    let ap_state = types::ApState::from(
        BssDescription::try_from(*bss_description.clone()).map_err(|error| {
            // This only occurs if an invalid `BssDescription` is received from SME, which should
            // never happen.
            format_err!("Failed to convert BSS description from FIDL: {:?}", error,)
        })?,
    );
    *options.ap_state = ap_state;
    options.bss_connect_start_time = fasync::MonotonicInstant::now();
    options.tracked_signals = TrackedSignals::new();
    options.initial_signal = options.ap_state.tracked.signal;
    options.tracked_signals.add(types::TimestampedSignal {
        time: fasync::MonotonicInstant::now(),
        signal: options.initial_signal,
    });
    options.post_connect_metric_timer =
        Box::pin(fasync::Timer::new(AVERAGE_SCORE_DELTA_MINIMUM_DURATION.after_now()));
    options.bss_connect_duration_metric_timer =
        Box::pin(fasync::Timer::new(METRICS_SHORT_CONNECT_DURATION.after_now()));
    // Re-initialize roam monitor for new BSS
    let (sender, roam_receiver) = mpsc::channel(ROAMING_CHANNEL_BUFFER_SIZE);
    options.roam_request_receiver = roam_receiver;
    options.roam_monitor_sender = common_options.roam_manager.initialize_roam_monitor(
        (*options.ap_state).clone(),
        options.network_identifier.clone(),
        options.credential.clone(),
        sender,
    );
    Ok(())
}

#[allow(clippy::result_unit_err, reason = "mass allow for https://fxbug.dev/381896734")]
/// Get the disconnect reason corresponding to the connect reason. Return an error if the connect
/// reason does not correspond to a manual connect.
pub fn convert_manual_connect_to_disconnect_reason(
    reason: &types::ConnectReason,
) -> Result<types::DisconnectReason, ()> {
    match reason {
        types::ConnectReason::FidlConnectRequest => Ok(types::DisconnectReason::FidlConnectRequest),
        types::ConnectReason::ProactiveNetworkSwitch => {
            Ok(types::DisconnectReason::ProactiveNetworkSwitch)
        }
        types::ConnectReason::RetryAfterDisconnectDetected
        | types::ConnectReason::RetryAfterFailedConnectAttempt
        | types::ConnectReason::RegulatoryChangeReconnect
        | types::ConnectReason::IdleInterfaceAutoconnect
        | types::ConnectReason::NewSavedNetworkAutoconnect => Err(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::roaming::lib::{PolicyRoamRequest, RoamTriggerData};
    use crate::client::roaming::local_roam_manager::RoamServiceRequest;
    use crate::config_management::{network_config, new_past_connection_list};
    use crate::util::listener;
    use crate::util::state_machine::{StateMachineStatusReader, status_publisher_and_reader};
    use crate::util::testing::{
        ConnectResultRecord, ConnectionRecord, FakeSavedNetworksManager,
        generate_connect_selection, generate_disconnect_info, generate_policy_roam_request,
        generate_random_scanned_candidate, poll_sme_req, random_connection_data,
    };
    use assert_matches::assert_matches;
    use fidl::endpoints::{create_proxy, create_proxy_and_stream};
    use fidl::prelude::*;
    use fidl_fuchsia_wlan_policy as fidl_policy;
    use futures::Future;
    use futures::task::Poll;
    use ieee80211::MacAddrBytes;
    use std::pin::pin;
    use wlan_common::random_fidl_bss_description;
    use wlan_metrics_registry::PolicyDisconnectionMigratedMetricDimensionReason;

    struct TestValues {
        common_options: CommonStateOptions,
        sme_req_stream: fidl_sme::ClientSmeRequestStream,
        saved_networks_manager: Arc<FakeSavedNetworksManager>,
        client_req_sender: mpsc::Sender<ManualRequest>,
        update_receiver: mpsc::UnboundedReceiver<listener::ClientListenerMessage>,
        telemetry_receiver: mpsc::Receiver<TelemetryEvent>,
        defect_receiver: mpsc::Receiver<Defect>,
        roam_service_request_receiver: mpsc::Receiver<RoamServiceRequest>,
        status_reader: StateMachineStatusReader<Status>,
    }

    fn test_setup() -> TestValues {
        let (client_req_sender, client_req_stream) = mpsc::channel(1);
        let (update_sender, update_receiver) = mpsc::unbounded();
        let (sme_proxy, sme_server) = create_proxy::<fidl_sme::ClientSmeMarker>();
        let sme_req_stream = sme_server.into_stream();
        let saved_networks = FakeSavedNetworksManager::new();
        let saved_networks_manager = Arc::new(saved_networks);
        let (telemetry_sender, telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let telemetry_sender = TelemetrySender::new(telemetry_sender);
        let (defect_sender, defect_receiver) = mpsc::channel(100);
        let (roam_service_request_sender, roam_service_request_receiver) = mpsc::channel(100);
        let roam_manager = RoamManager::new(roam_service_request_sender);
        let (status_publisher, status_reader) = status_publisher_and_reader::<Status>();

        TestValues {
            common_options: CommonStateOptions {
                proxy: SmeForClientStateMachine::new(sme_proxy, 0, defect_sender.clone()),
                req_stream: client_req_stream.fuse(),
                update_sender,
                saved_networks_manager: saved_networks_manager.clone(),
                telemetry_sender,
                iface_id: 1,
                defect_sender,
                roam_manager,
                status_publisher,
            },
            sme_req_stream,
            saved_networks_manager,
            client_req_sender,
            update_receiver,
            telemetry_receiver,
            defect_receiver,
            roam_service_request_receiver,
            status_reader,
        }
    }

    #[allow(clippy::needless_return, reason = "mass allow for https://fxbug.dev/381896734")]
    async fn run_state_machine(fut: impl Future<Output = Result<State, ExitReason>> + 'static) {
        let state_machine = fut.into_state_machine();
        select! {
            _state_machine = state_machine.fuse() => return,
        }
    }

    #[fuchsia::test]
    fn connecting_state_successfully_connects() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = test_setup();

        let connect_selection = generate_connect_selection();
        let bss_description =
            Sequestered::release(connect_selection.target.bss.bss_description.clone());

        // Store the network in the saved_networks_manager, so we can record connection success
        let save_fut = test_values.saved_networks_manager.store(
            connect_selection.target.network.clone(),
            connect_selection.target.credential.clone(),
        );
        let mut save_fut = pin!(save_fut);
        assert_matches!(exec.run_until_stalled(&mut save_fut), Poll::Ready(Ok(None)));

        // Check that the saved networks manager has the expected initial data
        let saved_network = exec
            .run_singlethreaded(
                test_values
                    .saved_networks_manager
                    .lookup(&connect_selection.target.network.clone()),
            )
            .expect("failed to lookup network");
        assert!(!saved_network.has_ever_connected);
        assert!(saved_network.hidden_probability > 0.0);

        let connecting_options =
            ConnectingOptions { connect_selection: connect_selection.clone(), attempt_counter: 0 };
        let initial_state = connecting_state(test_values.common_options, connecting_options);
        let fut = run_state_machine(initial_state);
        let mut fut = pin!(fut);
        let sme_fut = test_values.sme_req_stream.into_future();
        let mut sme_fut = pin!(sme_fut);

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Ensure a connect request is sent to the SME
        let connect_txn_handle = assert_matches!(
            poll_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ClientSmeRequest::Connect{ req, txn, control_handle: _ }) => {
                assert_eq!(req.ssid, connect_selection.target.network.ssid.clone().to_vec());
                assert_eq!(req.bss_description, bss_description);
                assert_eq!(req.deprecated_scan_type, fidl_fuchsia_wlan_common::ScanType::Active);
                assert_eq!(req.multiple_bss_candidates, connect_selection.target.network_has_multiple_bss);
                // Send connection response.
                let (_stream, ctrl) = txn.expect("connect txn unused")
                    .into_stream_and_control_handle();
                ctrl
            }
        );
        connect_txn_handle
            .send_on_connect_result(&fake_successful_connect_result())
            .expect("failed to send connection completion");

        // Check for a connecting update
        let client_state_update = ClientStateUpdate {
            state: fidl_policy::WlanClientState::ConnectionsEnabled,
            networks: vec![ClientNetworkState {
                id: connect_selection.target.network.clone(),
                state: fidl_policy::ConnectionState::Connecting,
                status: None,
            }],
        };
        assert_matches!(
            test_values.update_receiver.try_next(),
            Ok(Some(listener::Message::NotifyListeners(updates))) => {
            assert_eq!(updates, client_state_update);
        });

        // Progress the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Check for a connect update
        let client_state_update = ClientStateUpdate {
            state: fidl_policy::WlanClientState::ConnectionsEnabled,
            networks: vec![ClientNetworkState {
                id: connect_selection.target.network.clone(),
                state: fidl_policy::ConnectionState::Connected,
                status: None,
            }],
        };
        assert_matches!(
            test_values.update_receiver.try_next(),
            Ok(Some(listener::Message::NotifyListeners(updates))) => {
            assert_eq!(updates, client_state_update);
        });

        // Check that the connection was recorded to SavedNetworksManager
        assert_matches!(test_values.saved_networks_manager.get_recorded_connect_reslts().as_slice(), [data] => {
            let expected_connect_result = ConnectResultRecord {
                 id: connect_selection.target.network.clone(),
                 credential: connect_selection.target.credential.clone(),
                 bssid: types::Bssid::from(bss_description.bssid),
                 connect_result: fake_successful_connect_result(),
                 scan_type: connect_selection.target.bss.observation,
            };
            assert_eq!(data, &expected_connect_result);
        });

        // Progress the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Ensure no further updates were sent to listeners
        assert_matches!(
            exec.run_until_stalled(&mut test_values.update_receiver.into_future()),
            Poll::Pending
        );
    }

    #[fuchsia::test]
    fn connecting_state_times_out() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = test_setup();

        let connect_selection = generate_connect_selection();
        let bss_description =
            Sequestered::release(connect_selection.target.bss.bss_description.clone());

        // Store the network in the saved_networks_manager
        let save_fut = test_values.saved_networks_manager.store(
            connect_selection.target.network.clone(),
            connect_selection.target.credential.clone(),
        );
        let mut save_fut = pin!(save_fut);
        assert_matches!(exec.run_until_stalled(&mut save_fut), Poll::Ready(Ok(None)));

        // Prepare state machine
        let connecting_options =
            ConnectingOptions { connect_selection: connect_selection.clone(), attempt_counter: 0 };
        let initial_state = connecting_state(test_values.common_options, connecting_options);
        let fut = run_state_machine(initial_state);
        let mut fut = pin!(fut);
        let sme_fut = test_values.sme_req_stream.into_future();
        let mut sme_fut = pin!(sme_fut);

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Ensure a connect request is sent to the SME
        let connect_txn_handle = assert_matches!(
            poll_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ClientSmeRequest::Connect{ req, txn, control_handle: _ }) => {
                assert_eq!(req.ssid, connect_selection.target.network.ssid.clone().to_vec());
                assert_eq!(req.bss_description, bss_description);
                assert_eq!(req.deprecated_scan_type, fidl_fuchsia_wlan_common::ScanType::Active);
                assert_eq!(req.multiple_bss_candidates, connect_selection.target.network_has_multiple_bss);
                let (_stream, ctrl) = txn.expect("connect txn unused")
                    .into_stream_and_control_handle();
                ctrl
            }
        );

        // Check for a connecting update
        let client_state_update = ClientStateUpdate {
            state: fidl_policy::WlanClientState::ConnectionsEnabled,
            networks: vec![ClientNetworkState {
                id: connect_selection.target.network.clone(),
                state: fidl_policy::ConnectionState::Connecting,
                status: None,
            }],
        };
        assert_matches!(
            test_values.update_receiver.try_next(),
            Ok(Some(listener::Message::NotifyListeners(updates))) => {
            assert_eq!(updates, client_state_update);
        });

        // Respond with a SignalReport, which should not unblock connecting_state
        connect_txn_handle
            .send_on_signal_report(&fidl_internal::SignalReportIndication {
                rssi_dbm: -25,
                snr_db: 30,
                tx_rate_500kbps: 0,
            })
            .expect("failed to send singal report");

        // Run the state machine. Should still be pending
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Wake up the next timer, which is the timeout for the connect request.
        assert!(exec.wake_next_timer().is_some());

        // State machine should exit.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));
    }

    #[fuchsia::test]
    fn connecting_state_successfully_scans_and_connects() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(123));
        let mut test_values = test_setup();

        let connect_selection = generate_connect_selection();
        let bss_description =
            Sequestered::release(connect_selection.target.bss.bss_description.clone());

        // Set how the SavedNetworksManager should respond to lookup_compatible for the scan.
        let expected_config = network_config::NetworkConfig::new(
            connect_selection.target.network.clone(),
            connect_selection.target.credential.clone(),
            connect_selection.target.saved_network_info.has_ever_connected,
            None,
        )
        .expect("failed to create network config");
        test_values.saved_networks_manager.set_lookup_compatible_response(vec![expected_config]);

        let connecting_options =
            ConnectingOptions { connect_selection: connect_selection.clone(), attempt_counter: 0 };
        let initial_state = connecting_state(test_values.common_options, connecting_options);
        let fut = run_state_machine(initial_state);
        let mut fut = pin!(fut);

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Ensure a connect request is sent to the SME
        let sme_fut = test_values.sme_req_stream.into_future();
        let mut sme_fut = pin!(sme_fut);
        let time_to_connect = zx::MonotonicDuration::from_seconds(30);
        let connect_txn_handle = assert_matches!(
            poll_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ClientSmeRequest::Connect{ req, txn, control_handle: _ }) => {
                assert_eq!(req.ssid, connect_selection.target.network.ssid.clone().to_vec());
                assert_eq!(req.bss_description, bss_description.clone());
                assert_eq!(req.deprecated_scan_type, fidl_fuchsia_wlan_common::ScanType::Active);
                assert_eq!(req.multiple_bss_candidates, connect_selection.target.network_has_multiple_bss);
                // Send connection response.
                exec.set_fake_time(fasync::MonotonicInstant::after(time_to_connect));
                let (_stream, ctrl) = txn.expect("connect txn unused")
                    .into_stream_and_control_handle();
                ctrl
            }
        );
        connect_txn_handle
            .send_on_connect_result(&fake_successful_connect_result())
            .expect("failed to send connection completion");

        // Check for a connecting update
        let client_state_update = ClientStateUpdate {
            state: fidl_policy::WlanClientState::ConnectionsEnabled,
            networks: vec![ClientNetworkState {
                id: connect_selection.target.network.clone(),
                state: fidl_policy::ConnectionState::Connecting,
                status: None,
            }],
        };
        assert_matches!(
            test_values.update_receiver.try_next(),
            Ok(Some(listener::Message::NotifyListeners(updates))) => {
            assert_eq!(updates, client_state_update);
        });

        // Progress the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Check for a connect update
        let client_state_update = ClientStateUpdate {
            state: fidl_policy::WlanClientState::ConnectionsEnabled,
            networks: vec![ClientNetworkState {
                id: connect_selection.target.network.clone(),
                state: fidl_policy::ConnectionState::Connected,
                status: None,
            }],
        };
        assert_matches!(
            test_values.update_receiver.try_next(),
            Ok(Some(listener::Message::NotifyListeners(updates))) => {
            assert_eq!(updates, client_state_update);
        });

        // Check that the saved networks manager has the connection result recorded
        assert_matches!(test_values.saved_networks_manager.get_recorded_connect_reslts().as_slice(), [data] => {
            let expected_connect_result = ConnectResultRecord {
                 id: connect_selection.target.network.clone(),
                 credential: connect_selection.target.credential.clone(),
                 bssid: types::Bssid::from(bss_description.bssid),
                 connect_result: fake_successful_connect_result(),
                 scan_type: connect_selection.target.bss.observation,
            };
            assert_eq!(data, &expected_connect_result);
        });

        // Check that connected telemetry event is sent
        assert_matches!(
            test_values.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::ConnectResult { iface_id: 1, policy_connect_reason, result, multiple_bss_candidates, ap_state, network_is_likely_hidden: _ })) => {
                assert_eq!(bss_description, ap_state.original().clone().into());
                assert_eq!(multiple_bss_candidates, connect_selection.target.network_has_multiple_bss);
                assert_eq!(policy_connect_reason, Some(connect_selection.reason));
                assert_eq!(result, fake_successful_connect_result());
            }
        );

        // Progress the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Ensure no further updates were sent to listeners
        assert_matches!(
            exec.run_until_stalled(&mut test_values.update_receiver.into_future()),
            Poll::Pending
        );

        // Verify the Connected status was set.
        let status = test_values.status_reader.read_status().expect("failed to read status");
        assert_matches!(status, Status::Connected { .. });

        // Send a disconnect and check that the connection data is correctly recorded
        let is_sme_reconnecting = false;
        let fidl_disconnect_info = generate_disconnect_info(is_sme_reconnecting);
        connect_txn_handle
            .send_on_disconnect(&fidl_disconnect_info)
            .expect("failed to send disconnection event");
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify roam monitor request was sent.
        assert_matches!(test_values.roam_service_request_receiver.try_next(), Ok(Some(request)) => {
            assert_matches!(request, RoamServiceRequest::InitializeRoamMonitor { .. });
        });

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        let expected_recorded_connection = ConnectionRecord {
            id: connect_selection.target.network.clone(),
            credential: connect_selection.target.credential.clone(),
            data: PastConnectionData {
                bssid: types::Bssid::from(bss_description.bssid),
                disconnect_time: fasync::MonotonicInstant::now(),
                connection_uptime: zx::MonotonicDuration::from_minutes(0),
                disconnect_reason: types::DisconnectReason::DisconnectDetectedFromSme,
                signal_at_disconnect: types::Signal {
                    rssi_dbm: bss_description.rssi_dbm,
                    snr_db: bss_description.snr_db,
                },
                // TODO: record average phy rate over connection once available
                average_tx_rate: 0,
            },
        };
        assert_matches!(test_values.saved_networks_manager.get_recorded_past_connections().as_slice(), [data] => {
            assert_eq!(data, &expected_recorded_connection);
        });
    }

    #[fuchsia::test]
    fn connecting_state_fails_to_connect_and_retries() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = test_setup();

        let connect_selection = generate_connect_selection();
        let bss_description =
            Sequestered::release(connect_selection.target.bss.bss_description.clone());

        let connecting_options =
            ConnectingOptions { connect_selection: connect_selection.clone(), attempt_counter: 0 };
        let initial_state = connecting_state(test_values.common_options, connecting_options);
        let fut = run_state_machine(initial_state);
        let mut fut = pin!(fut);
        let sme_fut = test_values.sme_req_stream.into_future();
        let mut sme_fut = pin!(sme_fut);

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Ensure a connect request is sent to the SME
        let mut connect_txn_handle = assert_matches!(
            poll_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ClientSmeRequest::Connect{ req, txn, control_handle: _ }) => {
                assert_eq!(req.ssid, connect_selection.target.network.ssid.to_vec());
                 // Send connection response.
                let (_stream, ctrl) = txn.expect("connect txn unused")
                    .into_stream_and_control_handle();
                ctrl
            }
        );
        let connect_result = fidl_sme::ConnectResult {
            code: fidl_ieee80211::StatusCode::RefusedReasonUnspecified,
            ..fake_successful_connect_result()
        };
        connect_txn_handle
            .send_on_connect_result(&connect_result)
            .expect("failed to send connection completion");

        // Check for a connecting update
        let client_state_update = ClientStateUpdate {
            state: fidl_policy::WlanClientState::ConnectionsEnabled,
            networks: vec![ClientNetworkState {
                id: types::NetworkIdentifier {
                    ssid: connect_selection.target.network.ssid.clone(),
                    security_type: types::SecurityType::Wpa2,
                },
                state: fidl_policy::ConnectionState::Connecting,
                status: None,
            }],
        };
        assert_matches!(
            test_values.update_receiver.try_next(),
            Ok(Some(listener::Message::NotifyListeners(updates))) => {
            assert_eq!(updates, client_state_update);
        });

        // Progress the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        assert!(exec.wake_next_timer().is_some());
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Check that connect result telemetry event is sent
        assert_matches!(
            test_values.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::ConnectResult { iface_id: 1, policy_connect_reason, result, multiple_bss_candidates, ap_state, network_is_likely_hidden: _ })) => {
                assert_eq!(bss_description, ap_state.original().clone().into());
                assert_eq!(multiple_bss_candidates, connect_selection.target.network_has_multiple_bss);
                assert_eq!(policy_connect_reason, Some(connect_selection.reason));
                assert_eq!(result, connect_result);
            }
        );

        // Ensure a disconnect request is sent to the SME
        assert_matches!(
            poll_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ClientSmeRequest::Disconnect{ responder, reason: fidl_sme::UserDisconnectReason::FailedToConnect }) => {
                responder.send().expect("could not send sme response");
            }
        );

        // Progress the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Ensure a connect request is sent to the SME
        connect_txn_handle = assert_matches!(
            poll_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ClientSmeRequest::Connect{ req, txn, control_handle: _ }) => {
                assert_eq!(req.ssid, connect_selection.target.network.ssid.to_vec());
                assert_eq!(req.bss_description, Sequestered::release(connect_selection.target.bss.bss_description));
                assert_eq!(req.multiple_bss_candidates, connect_selection.target.network_has_multiple_bss);
                 // Send connection response.
                let (_stream, ctrl) = txn.expect("connect txn unused")
                    .into_stream_and_control_handle();
                ctrl
            }
        );
        let connect_result = fake_successful_connect_result();
        connect_txn_handle
            .send_on_connect_result(&connect_result)
            .expect("failed to send connection completion");

        // Progress the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Empty update sent to NotifyListeners (which in this case, will not actually be sent.)
        assert_matches!(
            test_values.update_receiver.try_next(),
            Ok(Some(listener::Message::NotifyListeners(ClientStateUpdate {
                state: fidl_policy::WlanClientState::ConnectionsEnabled,
                networks
            }))) => {
                assert!(networks.is_empty());
            }
        );

        // A defect should be logged.
        assert_matches!(
            test_values.defect_receiver.try_next(),
            Ok(Some(Defect::Iface(IfaceFailure::ConnectionFailure { iface_id: 1 })))
        );

        // Check for a connected update
        let client_state_update = ClientStateUpdate {
            state: fidl_policy::WlanClientState::ConnectionsEnabled,
            networks: vec![ClientNetworkState {
                id: types::NetworkIdentifier {
                    ssid: connect_selection.target.network.ssid.clone(),
                    security_type: types::SecurityType::Wpa2,
                },
                state: fidl_policy::ConnectionState::Connected,
                status: None,
            }],
        };
        assert_matches!(
            test_values.update_receiver.try_next(),
            Ok(Some(listener::Message::NotifyListeners(updates))) => {
            assert_eq!(updates, client_state_update);
        });

        // Progress the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Ensure no further updates were sent to listeners
        assert_matches!(
            exec.run_until_stalled(&mut test_values.update_receiver.into_future()),
            Poll::Pending
        );
    }

    #[fuchsia::test]
    fn connecting_state_fails_to_connect_at_max_retries() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = test_setup();

        let connect_selection = generate_connect_selection();
        let bss_description =
            Sequestered::release(connect_selection.target.bss.bss_description.clone());

        // save network to check that failed connect is recorded
        assert!(
            exec.run_singlethreaded(test_values.saved_networks_manager.store(
                connect_selection.target.network.clone(),
                connect_selection.target.credential.clone()
            ),)
                .expect("Failed to save network")
                .is_none()
        );

        let connecting_options = ConnectingOptions {
            connect_selection: connect_selection.clone(),
            attempt_counter: MAX_CONNECTION_ATTEMPTS - 1,
        };
        let initial_state = connecting_state(test_values.common_options, connecting_options);
        let fut = run_state_machine(initial_state);
        let mut fut = pin!(fut);
        let sme_fut = test_values.sme_req_stream.into_future();
        let mut sme_fut = pin!(sme_fut);

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Ensure a connect request is sent to the SME
        assert_matches!(
            poll_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ClientSmeRequest::Connect{ req, txn, control_handle: _ }) => {
                assert_eq!(req.ssid, connect_selection.target.network.ssid.clone().to_vec());
                assert_eq!(req.bss_description, bss_description.clone());
                assert_eq!(req.deprecated_scan_type, fidl_fuchsia_wlan_common::ScanType::Active);
                assert_eq!(req.multiple_bss_candidates, connect_selection.target.network_has_multiple_bss);
                 // Send connection response.
                let (_stream, ctrl) = txn.expect("connect txn unused")
                    .into_stream_and_control_handle();
                let connect_result = fidl_sme::ConnectResult {
                    code: fidl_ieee80211::StatusCode::RefusedReasonUnspecified,
                    ..fake_successful_connect_result()
                };
                ctrl
                    .send_on_connect_result(&connect_result)
                    .expect("failed to send connection completion");
            }
        );

        // After failing to reconnect, the state machine should exit so that the state machine
        // monitor can attempt to reconnect the interface.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));

        // Check for a connect update
        let client_state_update = ClientStateUpdate {
            state: fidl_policy::WlanClientState::ConnectionsEnabled,
            networks: vec![ClientNetworkState {
                id: connect_selection.target.network.clone(),
                state: fidl_policy::ConnectionState::Failed,
                status: Some(fidl_policy::DisconnectStatus::ConnectionFailed),
            }],
        };
        assert_matches!(
            test_values.update_receiver.try_next(),
            Ok(Some(listener::Message::NotifyListeners(updates))) => {
            assert_eq!(updates, client_state_update);
        });

        // Check that failure was recorded in SavedNetworksManager
        assert_matches!(test_values.saved_networks_manager.get_recorded_connect_reslts().as_slice(), [data] => {
            let connect_result = fidl_sme::ConnectResult {
                code: fidl_ieee80211::StatusCode::RefusedReasonUnspecified,
                is_credential_rejected: false,
                is_reconnect: false,
            };
            let expected_connect_result = ConnectResultRecord {
                 id: connect_selection.target.network.clone(),
                 credential: connect_selection.target.credential.clone(),
                 bssid: types::Bssid::from(bss_description.bssid),
                 connect_result,
                 scan_type: connect_selection.target.bss.observation,
            };
            assert_eq!(data, &expected_connect_result);
        });

        // A defect should be logged.
        assert_matches!(
            test_values.defect_receiver.try_next(),
            Ok(Some(Defect::Iface(IfaceFailure::ConnectionFailure { iface_id: 1 })))
        );
    }

    #[fuchsia::test]
    fn connecting_state_fails_to_connect_with_bad_credentials() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = test_setup();

        let connect_selection = generate_connect_selection();
        let bss_description =
            Sequestered::release(connect_selection.target.bss.bss_description.clone());

        assert!(
            exec.run_singlethreaded(test_values.saved_networks_manager.store(
                connect_selection.target.network.clone(),
                connect_selection.target.credential.clone()
            ),)
                .expect("Failed to save network")
                .is_none()
        );

        let connecting_options = ConnectingOptions {
            connect_selection: connect_selection.clone(),
            attempt_counter: MAX_CONNECTION_ATTEMPTS - 1,
        };
        let initial_state = connecting_state(test_values.common_options, connecting_options);
        let fut = run_state_machine(initial_state);
        let mut fut = pin!(fut);
        let sme_fut = test_values.sme_req_stream.into_future();
        let mut sme_fut = pin!(sme_fut);

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Ensure a connect request is sent to the SME
        assert_matches!(
            poll_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ClientSmeRequest::Connect{ req, txn, control_handle: _ }) => {
                assert_eq!(req.ssid, connect_selection.target.network.ssid.clone().to_vec());
                assert_eq!(req.bss_description, bss_description.clone());
                assert_eq!(req.deprecated_scan_type, fidl_fuchsia_wlan_common::ScanType::Active);
                assert_eq!(req.multiple_bss_candidates, connect_selection.target.network_has_multiple_bss);
                 // Send connection response.
                let (_stream, ctrl) = txn.expect("connect txn unused")
                    .into_stream_and_control_handle();
                let connect_result = fidl_sme::ConnectResult {
                    code: fidl_ieee80211::StatusCode::RefusedReasonUnspecified,
                    is_credential_rejected: true,
                    ..fake_successful_connect_result()
                };
                ctrl
                    .send_on_connect_result(&connect_result)
                    .expect("failed to send connection completion");
            }
        );

        // The state machine should exit when bad credentials are detected so that the state
        // machine monitor can try to connect to another network.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));

        // Check for a connect update
        let client_state_update = ClientStateUpdate {
            state: fidl_policy::WlanClientState::ConnectionsEnabled,
            networks: vec![ClientNetworkState {
                id: connect_selection.target.network.clone(),
                state: fidl_policy::ConnectionState::Failed,
                status: Some(fidl_policy::DisconnectStatus::CredentialsFailed),
            }],
        };
        assert_matches!(
            test_values.update_receiver.try_next(),
            Ok(Some(listener::Message::NotifyListeners(updates))) => {
            assert_eq!(updates, client_state_update);
        });

        // Check that failure was recorded to SavedNetworksManager
        assert_matches!(test_values.saved_networks_manager.get_recorded_connect_reslts().as_slice(), [data] => {
            let connect_result = fidl_sme::ConnectResult {
                code: fidl_ieee80211::StatusCode::RefusedReasonUnspecified,
                is_credential_rejected: true,
                is_reconnect: false,
            };
            let expected_connect_result = ConnectResultRecord {
                 id: connect_selection.target.network.clone(),
                 credential: connect_selection.target.credential.clone(),
                 bssid: types::Bssid::from(bss_description.bssid),
                 connect_result,
                 scan_type: connect_selection.target.bss.observation,
            };
            assert_eq!(data, &expected_connect_result);
        });

        // No defect should have been observed.
        assert_matches!(test_values.defect_receiver.try_next(), Ok(None));
    }

    #[fuchsia::test]
    fn connecting_state_gets_duplicate_connect_selection() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = test_setup();

        let connect_selection = generate_connect_selection();
        let bss_description =
            Sequestered::release(connect_selection.target.bss.bss_description.clone());

        let connecting_options =
            ConnectingOptions { connect_selection: connect_selection.clone(), attempt_counter: 0 };
        let initial_state = connecting_state(test_values.common_options, connecting_options);
        let fut = run_state_machine(initial_state);
        let mut fut = pin!(fut);
        let sme_fut = test_values.sme_req_stream.into_future();
        let mut sme_fut = pin!(sme_fut);

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Check for a connecting update
        let client_state_update = ClientStateUpdate {
            state: fidl_policy::WlanClientState::ConnectionsEnabled,
            networks: vec![ClientNetworkState {
                id: types::NetworkIdentifier {
                    ssid: connect_selection.target.network.ssid.clone(),
                    security_type: types::SecurityType::Wpa2,
                },
                state: fidl_policy::ConnectionState::Connecting,
                status: None,
            }],
        };
        assert_matches!(
            test_values.update_receiver.try_next(),
            Ok(Some(listener::Message::NotifyListeners(updates))) => {
            assert_eq!(updates, client_state_update);
        });

        // Send a duplicate connect request
        let mut client = Client::new(test_values.client_req_sender);
        let duplicate_request = types::ConnectSelection {
            // this incoming request should be deduped regardless of the reason
            reason: types::ConnectReason::ProactiveNetworkSwitch,
            ..connect_selection.clone()
        };
        client.connect(duplicate_request).expect("failed to make request");

        // Progress the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Ensure a connect request is sent to the SME
        let connect_txn_handle = assert_matches!(
            poll_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ClientSmeRequest::Connect{ req, txn, control_handle: _ }) => {
                assert_eq!(req.ssid, connect_selection.target.network.ssid.clone().to_vec());
                assert_eq!(req.deprecated_scan_type, fidl_fuchsia_wlan_common::ScanType::Active);
                assert_eq!(req.bss_description, bss_description);
                assert_eq!(req.multiple_bss_candidates, connect_selection.target.network_has_multiple_bss);
                 // Send connection response.
                let (_stream, ctrl) = txn.expect("connect txn unused")
                    .into_stream_and_control_handle();
                ctrl
            }
        );
        connect_txn_handle
            .send_on_connect_result(&fake_successful_connect_result())
            .expect("failed to send connection completion");

        // Progress the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Check for a connect update
        let client_state_update = ClientStateUpdate {
            state: fidl_policy::WlanClientState::ConnectionsEnabled,
            networks: vec![ClientNetworkState {
                id: connect_selection.target.network.clone(),
                state: fidl_policy::ConnectionState::Connected,
                status: None,
            }],
        };
        assert_matches!(
            test_values.update_receiver.try_next(),
            Ok(Some(listener::Message::NotifyListeners(updates))) => {
            assert_eq!(updates, client_state_update);
        });

        // Progress the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Ensure no further updates were sent to listeners
        assert_matches!(
            exec.run_until_stalled(&mut test_values.update_receiver.into_future()),
            Poll::Pending
        );
    }

    #[fuchsia::test]
    fn connecting_state_has_broken_sme() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();

        let connect_selection = generate_connect_selection();

        let connecting_options =
            ConnectingOptions { connect_selection: connect_selection.clone(), attempt_counter: 0 };
        let initial_state = connecting_state(test_values.common_options, connecting_options);
        let fut = run_state_machine(initial_state);
        let mut fut = pin!(fut);

        // Break the SME by dropping the server end of the SME stream, so it causes an error
        drop(test_values.sme_req_stream);

        // Ensure the state machine exits
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));
    }

    #[fuchsia::test]
    fn connected_state_gets_disconnect_request() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));

        let mut test_values = test_setup();
        let mut telemetry_receiver = test_values.telemetry_receiver;
        let connect_selection = generate_connect_selection();
        let bss_description =
            Sequestered::release(connect_selection.target.bss.bss_description.clone());
        let init_ap_state =
            types::ApState::from(BssDescription::try_from(bss_description.clone()).unwrap());

        let (connect_txn_proxy, _connect_txn_stream) =
            create_proxy_and_stream::<fidl_sme::ConnectTransactionMarker>();
        let options = ConnectedOptions::new(
            &mut test_values.common_options,
            Box::new(init_ap_state.clone()),
            connect_selection.target.network_has_multiple_bss,
            connect_selection.target.network.clone(),
            connect_selection.target.credential.clone(),
            connect_selection.reason,
            connect_txn_proxy.take_event_stream(),
            false,
        );
        let initial_state = connected_state(test_values.common_options, options);
        let fut = run_state_machine(initial_state);
        let mut fut = pin!(fut);
        let sme_fut = test_values.sme_req_stream.into_future();
        let mut sme_fut = pin!(sme_fut);

        let disconnect_time =
            fasync::MonotonicInstant::after(zx::MonotonicDuration::from_hours(12));

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify roam monitor request was sent.
        assert_matches!(test_values.roam_service_request_receiver.try_next(), Ok(Some(request)) => {
            assert_matches!(request, RoamServiceRequest::InitializeRoamMonitor { .. });
        });

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Run forward to get post connection signals metrics
        exec.set_fake_time(fasync::MonotonicInstant::after(
            AVERAGE_SCORE_DELTA_MINIMUM_DURATION + zx::MonotonicDuration::from_seconds(1),
        ));
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        assert_matches!(telemetry_receiver.try_next(), Ok(Some(event)) => {
            assert_matches!(event, TelemetryEvent::PostConnectionSignals { .. });
        });

        // Run forward to get long duration signals metrics
        exec.set_fake_time(fasync::MonotonicInstant::after(
            METRICS_SHORT_CONNECT_DURATION + zx::MonotonicDuration::from_seconds(1),
        ));
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        assert_matches!(telemetry_receiver.try_next(), Ok(Some(event)) => {
            assert_matches!(event, TelemetryEvent::LongDurationSignals { .. });
        });

        // Run forward to disconnect time
        exec.set_fake_time(disconnect_time);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Send a disconnect request
        let mut client = Client::new(test_values.client_req_sender);
        let (sender, mut receiver) = oneshot::channel();
        client
            .disconnect(types::DisconnectReason::FidlStopClientConnectionsRequest, sender)
            .expect("failed to make request");

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Respond to the SME disconnect
        assert_matches!(
            poll_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ClientSmeRequest::Disconnect{ responder, reason: fidl_sme::UserDisconnectReason::FidlStopClientConnectionsRequest }) => {
                responder.send().expect("could not send sme response");
            }
        );

        // Once the disconnect is processed, the state machine should exit.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));

        // Check for a disconnect update and the responder
        let client_state_update = ClientStateUpdate {
            state: fidl_policy::WlanClientState::ConnectionsEnabled,
            networks: vec![ClientNetworkState {
                id: connect_selection.target.network.clone(),
                state: fidl_policy::ConnectionState::Disconnected,
                status: Some(fidl_policy::DisconnectStatus::ConnectionStopped),
            }],
        };
        assert_matches!(
            test_values.update_receiver.try_next(),
            Ok(Some(listener::Message::NotifyListeners(updates))) => {
            assert_eq!(updates, client_state_update);
        });
        assert_matches!(exec.run_until_stalled(&mut receiver), Poll::Ready(Ok(())));

        // Disconnect telemetry event sent
        assert_matches!(telemetry_receiver.try_next(), Ok(Some(event)) => {
            assert_matches!(event, TelemetryEvent::Disconnected { track_subsequent_downtime, info: Some(info) } => {
                assert!(!track_subsequent_downtime);
                assert_matches!(info, DisconnectInfo {connected_duration, is_sme_reconnecting, disconnect_source, previous_connect_reason, ap_state, ..} => {
                    assert_eq!(connected_duration, zx::MonotonicDuration::from_hours(12));
                    assert!(!is_sme_reconnecting);
                    assert_eq!(disconnect_source, fidl_sme::DisconnectSource::User(fidl_sme::UserDisconnectReason::FidlStopClientConnectionsRequest));
                    assert_eq!(previous_connect_reason, connect_selection.reason);
                    assert_eq!(ap_state, init_ap_state.clone());
                });
            });
        });

        // The disconnect should have been recorded for the saved network config.
        let expected_recorded_connection = ConnectionRecord {
            id: connect_selection.target.network.clone(),
            credential: connect_selection.target.credential.clone(),
            data: PastConnectionData {
                bssid: init_ap_state.original().bssid,
                disconnect_time,
                connection_uptime: zx::MonotonicDuration::from_hours(12),
                disconnect_reason: types::DisconnectReason::FidlStopClientConnectionsRequest,
                signal_at_disconnect: types::Signal {
                    rssi_dbm: bss_description.rssi_dbm,
                    snr_db: bss_description.snr_db,
                },
                // TODO: record average phy rate over connection once available
                average_tx_rate: 0,
            },
        };
        assert_matches!(test_values.saved_networks_manager.get_recorded_past_connections().as_slice(), [connection_data] => {
            assert_eq!(connection_data, &expected_recorded_connection);
        });
    }

    #[fuchsia::test]
    fn connected_state_records_unexpected_disconnect() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));

        let mut test_values = test_setup();
        let mut telemetry_receiver = test_values.telemetry_receiver;

        let connect_selection = generate_connect_selection();
        let bss_description =
            Sequestered::release(connect_selection.target.bss.bss_description.clone());
        let init_ap_state =
            types::ApState::from(BssDescription::try_from(bss_description.clone()).unwrap());

        // Save the network in order to later record the disconnect to it.
        let save_fut = test_values.saved_networks_manager.store(
            connect_selection.target.network.clone(),
            connect_selection.target.credential.clone(),
        );
        let mut save_fut = pin!(save_fut);
        assert_matches!(exec.run_until_stalled(&mut save_fut), Poll::Ready(Ok(None)));

        let (connect_txn_proxy, connect_txn_stream) =
            create_proxy_and_stream::<fidl_sme::ConnectTransactionMarker>();
        let connect_txn_handle = connect_txn_stream.control_handle();
        let options = ConnectedOptions::new(
            &mut test_values.common_options,
            Box::new(init_ap_state.clone()),
            connect_selection.target.network_has_multiple_bss,
            connect_selection.target.network.clone(),
            connect_selection.target.credential.clone(),
            connect_selection.reason,
            connect_txn_proxy.take_event_stream(),
            false,
        );

        // Start the state machine in the connected state.
        let initial_state = connected_state(test_values.common_options, options);
        let fut = run_state_machine(initial_state);
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify roam monitor request was sent.
        assert_matches!(test_values.roam_service_request_receiver.try_next(), Ok(Some(request)) => {
            assert_matches!(request, RoamServiceRequest::InitializeRoamMonitor { .. });
        });

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        let disconnect_time =
            fasync::MonotonicInstant::after(zx::MonotonicDuration::from_hours(12));
        exec.set_fake_time(disconnect_time);

        // SME notifies Policy of disconnection
        let fidl_disconnect_info = generate_disconnect_info(false);
        connect_txn_handle
            .send_on_disconnect(&fidl_disconnect_info)
            .expect("failed to send disconnection event");
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // The disconnect should have been recorded for the saved network config.
        let expected_recorded_connection = ConnectionRecord {
            id: connect_selection.target.network.clone(),
            credential: connect_selection.target.credential.clone(),
            data: PastConnectionData {
                bssid: init_ap_state.original().bssid,
                disconnect_time,
                connection_uptime: zx::MonotonicDuration::from_hours(12),
                disconnect_reason: types::DisconnectReason::DisconnectDetectedFromSme,
                signal_at_disconnect: types::Signal {
                    rssi_dbm: bss_description.rssi_dbm,
                    snr_db: bss_description.snr_db,
                },
                // TODO: record average phy rate over connection once available
                average_tx_rate: 0,
            },
        };
        assert_matches!(test_values.saved_networks_manager.get_recorded_past_connections().as_slice(), [connection_data] => {
            assert_eq!(connection_data, &expected_recorded_connection);
        });

        // Disconnect telemetry event sent
        assert_matches!(telemetry_receiver.try_next(), Ok(Some(event)) => {
            assert_matches!(event, TelemetryEvent::Disconnected { track_subsequent_downtime, info: Some(info) } => {
                assert!(track_subsequent_downtime);
                assert_matches!(info, DisconnectInfo {connected_duration, is_sme_reconnecting, disconnect_source, previous_connect_reason, ap_state, ..} => {
                    assert_eq!(connected_duration, zx::MonotonicDuration::from_hours(12));
                    assert!(!is_sme_reconnecting);
                    assert_eq!(disconnect_source, fidl_disconnect_info.disconnect_source);
                    assert_eq!(previous_connect_reason, connect_selection.reason);
                    assert_eq!(ap_state, init_ap_state);
                });
            });
        });
    }

    #[fuchsia::test]
    fn connected_state_reconnect_resets_connected_duration() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));

        let mut test_values = test_setup();
        let mut telemetry_receiver = test_values.telemetry_receiver;

        let connect_selection = generate_connect_selection();
        let bss_description =
            Sequestered::release(connect_selection.target.bss.bss_description.clone());
        let ap_state =
            types::ApState::from(BssDescription::try_from(bss_description.clone()).unwrap());

        let (connect_txn_proxy, connect_txn_stream) =
            create_proxy_and_stream::<fidl_sme::ConnectTransactionMarker>();
        let connect_txn_handle = connect_txn_stream.control_handle();
        let options = ConnectedOptions::new(
            &mut test_values.common_options,
            Box::new(ap_state.clone()),
            connect_selection.target.network_has_multiple_bss,
            connect_selection.target.network.clone(),
            connect_selection.target.credential.clone(),
            connect_selection.reason,
            connect_txn_proxy.take_event_stream(),
            false,
        );
        let initial_state = connected_state(test_values.common_options, options);
        let fut = run_state_machine(initial_state);
        let mut fut = pin!(fut);

        let disconnect_time =
            fasync::MonotonicInstant::after(zx::MonotonicDuration::from_hours(12));

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify roam monitor request was sent.
        assert_matches!(test_values.roam_service_request_receiver.try_next(), Ok(Some(request)) => {
            assert_matches!(request, RoamServiceRequest::InitializeRoamMonitor { .. });
        });

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Run forward to get post connection score metrics
        exec.set_fake_time(fasync::MonotonicInstant::after(
            AVERAGE_SCORE_DELTA_MINIMUM_DURATION + zx::MonotonicDuration::from_seconds(1),
        ));
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        assert_matches!(telemetry_receiver.try_next(), Ok(Some(event)) => {
            assert_matches!(event, TelemetryEvent::PostConnectionSignals { .. });
        });

        // Run forward to get long duration signals metrics
        exec.set_fake_time(fasync::MonotonicInstant::after(
            METRICS_SHORT_CONNECT_DURATION + zx::MonotonicDuration::from_seconds(1),
        ));
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        assert_matches!(telemetry_receiver.try_next(), Ok(Some(event)) => {
            assert_matches!(event, TelemetryEvent::LongDurationSignals { .. });
        });

        // Run forward to disconnect time
        exec.set_fake_time(disconnect_time);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // SME notifies Policy of disconnection with SME-initiated reconnect
        let is_sme_reconnecting = true;
        let fidl_disconnect_info = generate_disconnect_info(is_sme_reconnecting);
        connect_txn_handle
            .send_on_disconnect(&fidl_disconnect_info)
            .expect("failed to send disconnection event");
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Disconnect telemetry event sent
        assert_matches!(telemetry_receiver.try_next(), Ok(Some(event)) => {
            assert_matches!(event, TelemetryEvent::Disconnected { info: Some(info), .. } => {
                assert_eq!(info.connected_duration, zx::MonotonicDuration::from_hours(12));
            });
        });

        // SME notifies Policy of reconnection successful
        exec.set_fake_time(fasync::MonotonicInstant::after(zx::MonotonicDuration::from_seconds(1)));
        let connect_result =
            fidl_sme::ConnectResult { is_reconnect: true, ..fake_successful_connect_result() };
        connect_txn_handle
            .send_on_connect_result(&connect_result)
            .expect("failed to send connect result event");

        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        assert_matches!(
            telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::ConnectResult { .. }))
        );

        // SME notifies Policy of another disconnection
        exec.set_fake_time(fasync::MonotonicInstant::after(zx::MonotonicDuration::from_hours(2)));
        let is_sme_reconnecting = false;
        let fidl_disconnect_info = generate_disconnect_info(is_sme_reconnecting);
        connect_txn_handle
            .send_on_disconnect(&fidl_disconnect_info)
            .expect("failed to send disconnection event");
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Another disconnect telemetry event sent
        assert_matches!(telemetry_receiver.try_next(), Ok(Some(event)) => {
            assert_matches!(event, TelemetryEvent::Disconnected { info, .. } => {
                assert_eq!(info.unwrap().connected_duration, zx::MonotonicDuration::from_hours(2));
            });
        });
    }

    #[fuchsia::test]
    fn connected_state_records_unexpected_disconnect_unspecified_bss() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let connection_attempt_time = fasync::MonotonicInstant::from_nanos(0);
        exec.set_fake_time(connection_attempt_time);
        let mut test_values = test_setup();

        let connect_selection = generate_connect_selection();
        let bss_description =
            Sequestered::release(connect_selection.target.bss.bss_description.clone());

        // Setup for network selection in the connecting state to select the intended network.
        let expected_config = network_config::NetworkConfig::new(
            connect_selection.target.network.clone(),
            connect_selection.target.credential.clone(),
            false,
            None,
        )
        .expect("failed to create network config");
        test_values.saved_networks_manager.set_lookup_compatible_response(vec![expected_config]);

        let connecting_options =
            ConnectingOptions { connect_selection: connect_selection.clone(), attempt_counter: 0 };
        let initial_state = connecting_state(test_values.common_options, connecting_options);
        let state_fut = run_state_machine(initial_state);
        let mut state_fut = pin!(state_fut);
        let sme_fut = test_values.sme_req_stream.into_future();
        let mut sme_fut = pin!(sme_fut);

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut state_fut), Poll::Pending);

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut state_fut), Poll::Pending);

        let time_to_connect = zx::MonotonicDuration::from_seconds(10);
        exec.set_fake_time(fasync::MonotonicInstant::after(time_to_connect));

        // Process connect request sent to SME
        let connect_txn_handle = assert_matches!(
            poll_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ClientSmeRequest::Connect{ req: _, txn, control_handle: _ }) => {
                 // Send connection response.
                let (_stream, ctrl) = txn.expect("connect txn unused")
                    .into_stream_and_control_handle();
                ctrl
            }
        );
        connect_txn_handle
            .send_on_connect_result(&fake_successful_connect_result())
            .expect("failed to send connection completion");
        assert_matches!(exec.run_until_stalled(&mut state_fut), Poll::Pending);

        // SME notifies Policy of disconnection.
        let disconnect_time = fasync::MonotonicInstant::after(zx::MonotonicDuration::from_hours(5));
        exec.set_fake_time(disconnect_time);
        let is_sme_reconnecting = false;
        connect_txn_handle
            .send_on_disconnect(&generate_disconnect_info(is_sme_reconnecting))
            .expect("failed to send disconnection event");
        assert_matches!(exec.run_until_stalled(&mut state_fut), Poll::Pending);

        // Verify roam monitor request was sent.
        assert_matches!(test_values.roam_service_request_receiver.try_next(), Ok(Some(request)) => {
            assert_matches!(request, RoamServiceRequest::InitializeRoamMonitor { .. });
        });

        assert_matches!(exec.run_until_stalled(&mut state_fut), Poll::Pending);
        // The connection data should have been recorded at disconnect.
        let expected_recorded_connection = ConnectionRecord {
            id: connect_selection.target.network.clone(),
            credential: connect_selection.target.credential.clone(),
            data: PastConnectionData {
                bssid: types::Bssid::from(bss_description.bssid),
                disconnect_time,
                connection_uptime: zx::MonotonicDuration::from_hours(5),
                disconnect_reason: types::DisconnectReason::DisconnectDetectedFromSme,
                signal_at_disconnect: types::Signal {
                    rssi_dbm: bss_description.rssi_dbm,
                    snr_db: bss_description.snr_db,
                },
                average_tx_rate: 0,
            },
        };
        assert_matches!(test_values.saved_networks_manager.get_recorded_past_connections().as_slice(), [connection_data] => {
            assert_eq!(connection_data, &expected_recorded_connection);
        });
    }

    #[fuchsia::test]
    fn connected_state_gets_duplicate_connect_selection() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));
        let mut test_values = test_setup();
        let mut telemetry_receiver = test_values.telemetry_receiver;

        let connect_selection = generate_connect_selection();
        let bss_description =
            Sequestered::release(connect_selection.target.bss.bss_description.clone());
        let ap_state =
            types::ApState::from(BssDescription::try_from(bss_description.clone()).unwrap());

        let (connect_txn_proxy, _connect_txn_stream) =
            create_proxy_and_stream::<fidl_sme::ConnectTransactionMarker>();
        let options = ConnectedOptions::new(
            &mut test_values.common_options,
            Box::new(ap_state.clone()),
            connect_selection.target.network_has_multiple_bss,
            connect_selection.target.network.clone(),
            connect_selection.target.credential.clone(),
            connect_selection.reason,
            connect_txn_proxy.take_event_stream(),
            false,
        );
        let initial_state = connected_state(test_values.common_options, options);
        let fut = run_state_machine(initial_state);
        let mut fut = pin!(fut);
        let sme_fut = test_values.sme_req_stream.into_future();
        let mut sme_fut = pin!(sme_fut);

        // Send another duplicate request
        let mut client = Client::new(test_values.client_req_sender);
        client.connect(connect_selection.clone()).expect("failed to make request");

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Ensure nothing was sent to the SME
        assert_matches!(poll_sme_req(&mut exec, &mut sme_fut), Poll::Pending);

        // No telemetry event is sent
        assert_matches!(telemetry_receiver.try_next(), Err(_));
    }

    #[fuchsia::test]
    fn connected_state_gets_different_connect_selection() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));

        let mut test_values = test_setup();
        let mut telemetry_receiver = test_values.telemetry_receiver;

        let first_connect_selection = generate_connect_selection();
        let first_bss_desc =
            Sequestered::release(first_connect_selection.target.bss.bss_description.clone());
        let first_ap_state =
            types::ApState::from(BssDescription::try_from(first_bss_desc.clone()).unwrap());
        let second_connect_selection = types::ConnectSelection {
            reason: types::ConnectReason::ProactiveNetworkSwitch,
            ..generate_connect_selection()
        };

        let (connect_txn_proxy, _connect_txn_stream) =
            create_proxy_and_stream::<fidl_sme::ConnectTransactionMarker>();
        let options = ConnectedOptions::new(
            &mut test_values.common_options,
            Box::new(first_ap_state.clone()),
            first_connect_selection.target.network_has_multiple_bss,
            first_connect_selection.target.network.clone(),
            first_connect_selection.target.credential.clone(),
            first_connect_selection.reason,
            connect_txn_proxy.take_event_stream(),
            false,
        );
        let initial_state = connected_state(test_values.common_options, options);
        let fut = run_state_machine(initial_state);
        let mut fut = pin!(fut);
        let sme_fut = test_values.sme_req_stream.into_future();
        let mut sme_fut = pin!(sme_fut);

        let disconnect_time =
            fasync::MonotonicInstant::after(zx::MonotonicDuration::from_hours(12));

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify roam monitor request was sent.
        assert_matches!(test_values.roam_service_request_receiver.try_next(), Ok(Some(request)) => {
            assert_matches!(request, RoamServiceRequest::InitializeRoamMonitor { .. });
        });

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Run forward to get post connection signals metrics
        exec.set_fake_time(fasync::MonotonicInstant::after(
            AVERAGE_SCORE_DELTA_MINIMUM_DURATION + zx::MonotonicDuration::from_seconds(1),
        ));
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        assert_matches!(telemetry_receiver.try_next(), Ok(Some(event)) => {
            assert_matches!(event, TelemetryEvent::PostConnectionSignals { .. });
        });

        // Run forward to get long duration signals metrics
        exec.set_fake_time(fasync::MonotonicInstant::after(
            METRICS_SHORT_CONNECT_DURATION + zx::MonotonicDuration::from_seconds(1),
        ));
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        assert_matches!(telemetry_receiver.try_next(), Ok(Some(event)) => {
            assert_matches!(event, TelemetryEvent::LongDurationSignals { .. });
        });

        // Run forward to disconnect time
        exec.set_fake_time(disconnect_time);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Send a different connect request
        let mut client = Client::new(test_values.client_req_sender);
        client.connect(second_connect_selection.clone()).expect("failed to make request");

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // There should be 2 requests to the SME stacked up
        // First SME request: disconnect
        assert_matches!(
            poll_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ClientSmeRequest::Disconnect{ responder, reason: fidl_sme::UserDisconnectReason::ProactiveNetworkSwitch }) => {
                responder.send().expect("could not send sme response");
            }
        );
        // Progress the state machine
        // TODO(https://fxbug.dev/42130926): remove this once the disconnect request is fire-and-forget
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        // Second SME request: connect to the second network
        let connect_txn_handle = assert_matches!(
            poll_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ClientSmeRequest::Connect{ req, txn, control_handle: _ }) => {
                assert_eq!(req.ssid, second_connect_selection.target.network.ssid.clone().to_vec());
                 // Send connection response.
                let (_stream, ctrl) = txn.expect("connect txn unused")
                    .into_stream_and_control_handle();
                ctrl
            }
        );
        connect_txn_handle
            .send_on_connect_result(&fake_successful_connect_result())
            .expect("failed to send connection completion");
        // Progress the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Check for a disconnect update
        let client_state_update = ClientStateUpdate {
            state: fidl_policy::WlanClientState::ConnectionsEnabled,
            networks: vec![ClientNetworkState {
                id: first_connect_selection.target.network.clone(),
                state: fidl_policy::ConnectionState::Disconnected,
                status: Some(fidl_policy::DisconnectStatus::ConnectionStopped),
            }],
        };
        assert_matches!(
            test_values.update_receiver.try_next(),
            Ok(Some(listener::Message::NotifyListeners(updates))) => {
            assert_eq!(updates, client_state_update);
        });

        // Disconnect telemetry event sent
        assert_matches!(telemetry_receiver.try_next(), Ok(Some(event)) => {
            assert_matches!(event, TelemetryEvent::Disconnected { track_subsequent_downtime, info: Some(info) } => {
                assert!(!track_subsequent_downtime);
                assert_matches!(info, DisconnectInfo {connected_duration, is_sme_reconnecting, disconnect_source, previous_connect_reason, ap_state, ..} => {
                    assert_eq!(connected_duration, zx::MonotonicDuration::from_hours(12));
                    assert!(!is_sme_reconnecting);
                    assert_eq!(disconnect_source, fidl_sme::DisconnectSource::User(fidl_sme::UserDisconnectReason::ProactiveNetworkSwitch));
                    assert_eq!(previous_connect_reason, first_connect_selection.reason);
                    assert_eq!(ap_state, first_ap_state.clone());
                });
            });
        });

        // Check for a connecting update
        let client_state_update = ClientStateUpdate {
            state: fidl_policy::WlanClientState::ConnectionsEnabled,
            networks: vec![ClientNetworkState {
                id: types::NetworkIdentifier {
                    ssid: second_connect_selection.target.network.ssid.clone(),
                    security_type: types::SecurityType::Wpa2,
                },
                state: fidl_policy::ConnectionState::Connecting,
                status: None,
            }],
        };
        assert_matches!(
            test_values.update_receiver.try_next(),
            Ok(Some(listener::Message::NotifyListeners(updates))) => {
            assert_eq!(updates, client_state_update);
        });
        // Check for a connected update
        let client_state_update = ClientStateUpdate {
            state: fidl_policy::WlanClientState::ConnectionsEnabled,
            networks: vec![ClientNetworkState {
                id: types::NetworkIdentifier {
                    ssid: second_connect_selection.target.network.ssid.clone(),
                    security_type: types::SecurityType::Wpa2,
                },
                state: fidl_policy::ConnectionState::Connected,
                status: None,
            }],
        };
        assert_matches!(
            test_values.update_receiver.try_next(),
            Ok(Some(listener::Message::NotifyListeners(updates))) => {
            assert_eq!(updates, client_state_update);
        });

        // Progress the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Ensure no further updates were sent to listeners
        assert_matches!(
            exec.run_until_stalled(&mut test_values.update_receiver.into_future()),
            Poll::Pending
        );

        // Check that the first connection was recorded
        let expected_recorded_connection = ConnectionRecord {
            id: first_connect_selection.target.network.clone(),
            credential: first_connect_selection.target.credential.clone(),
            data: PastConnectionData {
                bssid: types::Bssid::from(first_bss_desc.bssid),
                disconnect_time,
                connection_uptime: zx::MonotonicDuration::from_hours(12),
                disconnect_reason: types::DisconnectReason::ProactiveNetworkSwitch,
                signal_at_disconnect: types::Signal {
                    rssi_dbm: first_bss_desc.rssi_dbm,
                    snr_db: first_bss_desc.snr_db,
                },
                // TODO: record average phy rate over connection once available
                average_tx_rate: 0,
            },
        };
        assert_matches!(test_values.saved_networks_manager.get_recorded_past_connections().as_slice(), [connection_data] => {
            assert_eq!(connection_data, &expected_recorded_connection);
        });
    }

    #[fuchsia::test]
    fn connected_state_notified_of_network_disconnect_no_sme_reconnect_short_uptime_no_retry() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let mut test_values = test_setup();

        let connect_selection = generate_connect_selection();
        let bss_description =
            Sequestered::release(connect_selection.target.bss.bss_description.clone());
        let ap_state =
            types::ApState::from(BssDescription::try_from(bss_description.clone()).unwrap());

        let (connect_txn_proxy, connect_txn_stream) =
            create_proxy_and_stream::<fidl_sme::ConnectTransactionMarker>();
        let connect_txn_handle = connect_txn_stream.control_handle();
        let options = ConnectedOptions::new(
            &mut test_values.common_options,
            Box::new(ap_state.clone()),
            connect_selection.target.network_has_multiple_bss,
            connect_selection.target.network.clone(),
            connect_selection.target.credential.clone(),
            connect_selection.reason,
            connect_txn_proxy.take_event_stream(),
            false,
        );
        let initial_state = connected_state(test_values.common_options, options);
        let fut = run_state_machine(initial_state);
        let mut fut = pin!(fut);
        let sme_fut = test_values.sme_req_stream.into_future();
        let mut sme_fut = pin!(sme_fut);

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify roam monitor request was sent.
        assert_matches!(test_values.roam_service_request_receiver.try_next(), Ok(Some(request)) => {
            assert_matches!(request, RoamServiceRequest::InitializeRoamMonitor { .. });
        });

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // SME notifies Policy of disconnection.
        let is_sme_reconnecting = false;
        connect_txn_handle
            .send_on_disconnect(&generate_disconnect_info(is_sme_reconnecting))
            .expect("failed to send disconnection event");

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Check for a disconnect request to SME
        assert_matches!(
            poll_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ClientSmeRequest::Disconnect{ responder, reason: fidl_sme::UserDisconnectReason::DisconnectDetectedFromSme }) => {
                responder.send().expect("could not send sme response");
            }
        );

        // The state machine should exit since there is no attempt to reconnect.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));
    }

    #[fuchsia::test]
    fn connected_state_notified_of_network_disconnect_sme_reconnect_successfully() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = test_setup();

        let connect_selection = generate_connect_selection();
        let bss_description =
            Sequestered::release(connect_selection.target.bss.bss_description.clone());
        let ap_state =
            types::ApState::from(BssDescription::try_from(bss_description.clone()).unwrap());

        let (connect_txn_proxy, connect_txn_stream) =
            create_proxy_and_stream::<fidl_sme::ConnectTransactionMarker>();
        let connect_txn_handle = connect_txn_stream.control_handle();
        let options = ConnectedOptions::new(
            &mut test_values.common_options,
            Box::new(ap_state.clone()),
            connect_selection.target.network_has_multiple_bss,
            connect_selection.target.network.clone(),
            connect_selection.target.credential.clone(),
            connect_selection.reason,
            connect_txn_proxy.take_event_stream(),
            false,
        );
        let initial_state = connected_state(test_values.common_options, options);
        let fut = run_state_machine(initial_state);
        let mut fut = pin!(fut);

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // SME notifies Policy of disconnection
        let is_sme_reconnecting = true;
        connect_txn_handle
            .send_on_disconnect(&generate_disconnect_info(is_sme_reconnecting))
            .expect("failed to send disconnection event");

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // SME notifies Policy that reconnects succeeds
        let connect_result =
            fidl_sme::ConnectResult { is_reconnect: true, ..fake_successful_connect_result() };
        connect_txn_handle
            .send_on_connect_result(&connect_result)
            .expect("failed to send reconnection result");

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Check there were no state updates
        assert_matches!(test_values.update_receiver.try_next(), Err(_));
    }

    #[fuchsia::test]
    fn connected_state_notified_of_network_disconnect_sme_reconnect_unsuccessfully() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let mut test_values = test_setup();
        let connect_selection = generate_connect_selection();
        let bss_description =
            Sequestered::release(connect_selection.target.bss.bss_description.clone());
        let ap_state =
            types::ApState::from(BssDescription::try_from(bss_description.clone()).unwrap());

        // Set the start time of the connection
        let start_time = fasync::MonotonicInstant::now();
        exec.set_fake_time(start_time);

        let (connect_txn_proxy, connect_txn_stream) =
            create_proxy_and_stream::<fidl_sme::ConnectTransactionMarker>();
        let connect_txn_handle = connect_txn_stream.control_handle();
        let options = ConnectedOptions::new(
            &mut test_values.common_options,
            Box::new(ap_state.clone()),
            connect_selection.target.network_has_multiple_bss,
            connect_selection.target.network.clone(),
            connect_selection.target.credential.clone(),
            connect_selection.reason,
            connect_txn_proxy.take_event_stream(),
            false,
        );
        let initial_state = connected_state(test_values.common_options, options);
        let fut = run_state_machine(initial_state);
        let mut fut = pin!(fut);
        let sme_fut = test_values.sme_req_stream.into_future();
        let mut sme_fut = pin!(sme_fut);

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify roam monitor request was sent.
        assert_matches!(test_values.roam_service_request_receiver.try_next(), Ok(Some(request)) => {
            assert_matches!(request, RoamServiceRequest::InitializeRoamMonitor { .. });
        });

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Set time to indicate a decent uptime before the disconnect so the AP is retried
        exec.set_fake_time(start_time + fasync::MonotonicDuration::from_hours(24));

        // SME notifies Policy of disconnection
        let is_sme_reconnecting = true;
        connect_txn_handle
            .send_on_disconnect(&generate_disconnect_info(is_sme_reconnecting))
            .expect("failed to send disconnection event");

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // SME notifies Policy that reconnects fails
        let connect_result = fidl_sme::ConnectResult {
            code: fidl_ieee80211::StatusCode::RefusedReasonUnspecified,
            is_reconnect: true,
            ..fake_successful_connect_result()
        };
        connect_txn_handle
            .send_on_connect_result(&connect_result)
            .expect("failed to send reconnection result");

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Check for an SME disconnect request
        assert_matches!(
            poll_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ClientSmeRequest::Disconnect{ responder, reason: fidl_sme::UserDisconnectReason::DisconnectDetectedFromSme }) => {
                responder.send().expect("could not send sme response");
            }
        );

        // The state machine should exit since there is no policy attempt to reconnect.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));

        // Check for a disconnect update
        let client_state_update = ClientStateUpdate {
            state: fidl_policy::WlanClientState::ConnectionsEnabled,
            networks: vec![ClientNetworkState {
                id: connect_selection.target.network.clone(),
                state: fidl_policy::ConnectionState::Disconnected,
                status: Some(fidl_policy::DisconnectStatus::ConnectionFailed),
            }],
        };
        assert_matches!(
            test_values.update_receiver.try_next(),
            Ok(Some(listener::Message::NotifyListeners(updates))) => {
            assert_eq!(updates, client_state_update);
        });
    }

    #[fuchsia::test]
    fn connected_state_on_signal_report() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));

        let mut test_values = test_setup();

        // Verify the status is initialized to default.
        let status = test_values.status_reader.read_status().expect("failed to read status");
        assert_matches!(status, Status::Disconnected);

        // Set initial RSSI and SNR values
        let mut connect_selection = generate_connect_selection();
        let init_rssi = -40;
        let init_snr = 30;
        connect_selection.target.bss.signal =
            types::Signal { rssi_dbm: init_rssi, snr_db: init_snr };

        let mut bss_description =
            Sequestered::release(connect_selection.target.bss.bss_description.clone());
        bss_description.rssi_dbm = init_rssi;
        bss_description.snr_db = init_snr;
        connect_selection.target.bss.bss_description = bss_description.clone().into();

        let ap_state =
            types::ApState::from(BssDescription::try_from(bss_description.clone()).unwrap());

        // Add a PastConnectionData for the connected network to be send in BSS quality data.
        let mut past_connections = new_past_connection_list();
        let mut past_connection_data = random_connection_data();
        past_connection_data.bssid = ieee80211::Bssid::from(bss_description.bssid);
        past_connections.add(past_connection_data);
        let mut saved_networks_manager = FakeSavedNetworksManager::new();
        saved_networks_manager.past_connections_response = past_connections.clone();
        test_values.common_options.saved_networks_manager = Arc::new(saved_networks_manager);

        // Set up the state machine, starting at the connected state.
        let (connect_txn_proxy, connect_txn_stream) =
            create_proxy_and_stream::<fidl_sme::ConnectTransactionMarker>();
        let options = ConnectedOptions::new(
            &mut test_values.common_options,
            Box::new(ap_state.clone()),
            connect_selection.target.network_has_multiple_bss,
            connect_selection.target.network.clone(),
            connect_selection.target.credential.clone(),
            connect_selection.reason,
            connect_txn_proxy.take_event_stream(),
            false,
        );
        let initial_state = connected_state(test_values.common_options, options);

        let connect_txn_handle = connect_txn_stream.control_handle();
        let fut = run_state_machine(initial_state);
        let mut fut = pin!(fut);
        let sme_fut = test_values.sme_req_stream.into_future();
        let mut sme_fut = pin!(sme_fut);

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        let request = test_values
            .roam_service_request_receiver
            .try_next()
            .expect("error receiving roam service request")
            .expect("received None roam service request");
        assert_matches!(request, RoamServiceRequest::InitializeRoamMonitor{ mut roam_trigger_data_receiver, .. } => {
            // Run the state machine
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

            // Send the first signal report from SME
            let rssi_1 = -50;
            let snr_1 = 25;
            let fidl_signal_report =
                fidl_internal::SignalReportIndication { rssi_dbm: rssi_1, snr_db: snr_1 , tx_rate_500kbps: 0};
            connect_txn_handle
                .send_on_signal_report(&fidl_signal_report)
                .expect("failed to send signal report");
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

            // Do a quick check that state machine does not exist and there's no disconnect to SME
            assert_matches!(poll_sme_req(&mut exec, &mut sme_fut), Poll::Pending);

            // Verify telemetry event
            assert_matches!(test_values.telemetry_receiver.try_next(), Ok(Some(event)) => {
                assert_matches!(event, TelemetryEvent::OnSignalReport { .. });
            });

            // Verify that signal report is sent to the roam monitor
            assert_matches!(roam_trigger_data_receiver.try_next(), Ok(Some(RoamTriggerData::SignalReportInd(_))));

            // Verify that the status is updated.
            let status = test_values.status_reader.read_status().expect("failed to read status");
            assert_eq!(
                status,
                Status::Connected {
                    rssi: rssi_1,
                    snr: snr_1,
                    channel: ap_state.tracked.channel.primary
                }
            );

            // Send a second signal report with higher RSSI and SNR than the previous reports.
            let rssi_2 = -30;
            let snr_2 = 35;
            let fidl_signal_report =
                fidl_internal::SignalReportIndication { rssi_dbm: rssi_2, snr_db: snr_2 , tx_rate_500kbps: 0};
            connect_txn_handle
                .send_on_signal_report(&fidl_signal_report)
                .expect("failed to send signal report");
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

            // Verify telemetry events;
            assert_matches!(test_values.telemetry_receiver.try_next(), Ok(Some(event)) => {
                assert_matches!(event, TelemetryEvent::OnSignalReport { .. });
            });

            // Verify that signal report is sent to the roam monitor
            assert_matches!(roam_trigger_data_receiver.try_next(), Ok(Some(RoamTriggerData::SignalReportInd(_))));
        });
    }

    #[fuchsia::test]
    fn connected_state_on_channel_switched() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));

        let mut test_values = test_setup();
        let mut telemetry_receiver = test_values.telemetry_receiver;

        let connect_selection = generate_connect_selection();
        let bss_description =
            Sequestered::release(connect_selection.target.bss.bss_description.clone());
        let ap_state =
            types::ApState::from(BssDescription::try_from(bss_description.clone()).unwrap());

        // Set up the state machine, starting at the connected state.
        let (connect_txn_proxy, connect_txn_stream) =
            create_proxy_and_stream::<fidl_sme::ConnectTransactionMarker>();
        let options = ConnectedOptions::new(
            &mut test_values.common_options,
            Box::new(ap_state.clone()),
            connect_selection.target.network_has_multiple_bss,
            connect_selection.target.network.clone(),
            connect_selection.target.credential.clone(),
            connect_selection.reason,
            connect_txn_proxy.take_event_stream(),
            false,
        );
        let initial_state = connected_state(test_values.common_options, options);

        let connect_txn_handle = connect_txn_stream.control_handle();
        let fut = run_state_machine(initial_state);
        let mut fut = pin!(fut);

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify roam monitor request was sent.
        assert_matches!(test_values.roam_service_request_receiver.try_next(), Ok(Some(request)) => {
            assert_matches!(request, RoamServiceRequest::InitializeRoamMonitor { ap_state, .. } => {
                assert_eq!(ap_state.tracked.channel.primary, bss_description.primary.number)
            });
        });

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        let channel_switch_info = fidl_internal::ChannelSwitchInfo {
            new_primary_channel: fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: 10,
            },
            bandwidth: fidl_ieee80211::ChannelBandwidth::Cbw20,
            vht_secondary_80_channel: fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: 0,
            },
        };
        connect_txn_handle
            .send_on_channel_switched(&channel_switch_info)
            .expect("failed to send signal report");
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify telemetry event
        assert_matches!(telemetry_receiver.try_next(), Ok(Some(event)) => {
            assert_matches!(event, TelemetryEvent::OnChannelSwitched { info } => {
                assert_eq!(info, channel_switch_info);
            });
        });

        // Verify the roam monitor was re-initialized with the new channel
        assert_matches!(test_values.roam_service_request_receiver.try_next(), Ok(Some(request)) => {
            assert_matches!(request, RoamServiceRequest::InitializeRoamMonitor { ap_state, .. } => {
                assert_eq!(ap_state.tracked.channel.primary, 10)
            });
        });

        // Have SME notify Policy of disconnection so we can see whether the channel in the
        // BssDescription has changed.
        let is_sme_reconnecting = false;
        let fidl_disconnect_info = generate_disconnect_info(is_sme_reconnecting);
        connect_txn_handle
            .send_on_disconnect(&fidl_disconnect_info)
            .expect("failed to send disconnection event");
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify telemetry event
        assert_matches!(telemetry_receiver.try_next(), Ok(Some(event)) => {
            assert_matches!(event, TelemetryEvent::Disconnected { info, .. } => {
                assert_eq!(info.unwrap().ap_state.tracked.channel.primary, 10);
            });
        });
    }

    #[fuchsia::test]
    fn connected_state_on_roam_selection() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));

        let mut test_values = test_setup();
        let sme_fut = test_values.sme_req_stream.into_future();
        let mut sme_fut = pin!(sme_fut);

        // Set up the state machine, starting at the connected state.
        let connect_selection = generate_connect_selection();
        let bss_description =
            Sequestered::release(connect_selection.target.bss.bss_description.clone());
        let ap_state =
            types::ApState::from(BssDescription::try_from(bss_description.clone()).unwrap());
        let (connect_txn_proxy, _connect_txn_stream) =
            create_proxy_and_stream::<fidl_sme::ConnectTransactionMarker>();
        let options = ConnectedOptions::new(
            &mut test_values.common_options,
            Box::new(ap_state.clone()),
            connect_selection.target.network_has_multiple_bss,
            connect_selection.target.network.clone(),
            connect_selection.target.credential.clone(),
            connect_selection.reason,
            connect_txn_proxy.take_event_stream(),
            false,
        );
        let initial_state = connected_state(test_values.common_options, options);
        let fut = run_state_machine(initial_state);
        let mut fut = pin!(fut);

        // Run the state machine.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify roam monitor selection was sent.
        let mut roam_sender;
        assert_matches!(test_values.roam_service_request_receiver.try_next(), Ok(Some(request)) => {
            assert_matches!(request, RoamServiceRequest::InitializeRoamMonitor { roam_request_sender, .. } => {
                roam_sender = roam_request_sender;
            });
        });

        // Send a roam request to state machine.
        let roam_candidate = generate_random_scanned_candidate();
        roam_sender
            .try_send(PolicyRoamRequest { candidate: roam_candidate.clone(), reasons: vec![] })
            .unwrap();

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify state machine issues a roam to SME.
        assert_matches!(
            poll_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ClientSmeRequest::Roam{ req, ..}) => {
                assert_eq!(req.bss_description, Sequestered::release(roam_candidate.clone().bss.bss_description));
            }
        );

        // Verify roam attempt telemetry event.
        assert_matches!(test_values.telemetry_receiver.try_next(), Ok(Some(event)) => {
            assert_matches!(event, TelemetryEvent::PolicyRoamAttempt { request, connected_duration } => {
                assert_eq!(request.candidate, roam_candidate);
                assert_eq!(request.reasons, vec![]);
                assert_eq!(connected_duration, zx::Duration::from_minutes(0));

            });
        });
    }

    #[fuchsia::test]
    fn connected_state_on_roam_result_success() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));

        let mut test_values = test_setup();
        let mut telemetry_receiver = test_values.telemetry_receiver;

        let connect_selection = generate_connect_selection();
        let bss_description =
            Sequestered::release(connect_selection.target.bss.bss_description.clone());
        let ap_state =
            types::ApState::from(BssDescription::try_from(bss_description.clone()).unwrap());

        // Set up the state machine, starting at the connected state.
        let (connect_txn_proxy, connect_txn_stream) =
            create_proxy_and_stream::<fidl_sme::ConnectTransactionMarker>();
        let mut options = ConnectedOptions::new(
            &mut test_values.common_options,
            Box::new(ap_state.clone()),
            connect_selection.target.network_has_multiple_bss,
            connect_selection.target.network.clone(),
            connect_selection.target.credential.clone(),
            connect_selection.reason,
            connect_txn_proxy.take_event_stream(),
            false,
        );
        // Set the pending roam request, so we don't immediately exit when a result comes in.
        let policy_request = generate_policy_roam_request([1, 1, 1, 1, 1, 1].into());
        options.pending_roam = Some(policy_request.clone().into());
        let initial_state = connected_state(test_values.common_options, options);

        let connect_txn_handle = connect_txn_stream.control_handle();
        let fut = run_state_machine(initial_state);
        let mut fut = pin!(fut);

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify roam monitor request was sent.
        assert_matches!(test_values.roam_service_request_receiver.try_next(), Ok(Some(request)) => {
            assert_matches!(request, RoamServiceRequest::InitializeRoamMonitor { .. });
        });

        // Send a successful roam result
        let bss_desc = random_fidl_bss_description!();
        let roam_result = fidl_sme::RoamResult {
            bssid: [1, 1, 1, 1, 1, 1],
            status_code: fidl_ieee80211::StatusCode::Success,
            original_association_maintained: false,
            bss_description: Some(Box::new(bss_desc.clone())),
            disconnect_info: None,
            is_credential_rejected: false,
        };
        connect_txn_handle.send_on_roam_result(&roam_result).expect("failed to send roam result");
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify the roam monitor was re-initialized with the new BSS
        assert_matches!(test_values.roam_service_request_receiver.try_next(), Ok(Some(request)) => {
            assert_matches!(request, RoamServiceRequest::InitializeRoamMonitor { ap_state, .. } => {
                assert_eq!(ap_state.original().bssid.to_array(), bss_desc.bssid);
            });
        });

        // Verify a disconnect was logged to saved networks manager
        assert_matches!(test_values.saved_networks_manager.get_recorded_past_connections().as_slice(), [ConnectionRecord {id, credential, data}] => {
            assert_eq!(id, &connect_selection.target.network.clone());
            assert_eq!(credential, &connect_selection.target.credential.clone());
            assert_matches!(data, PastConnectionData {bssid, disconnect_reason, ..} => {
                assert_eq!(bssid, &connect_selection.target.bss.bssid);
                assert_eq!(disconnect_reason, &types::DisconnectReason::Unknown);
            })
        });

        // Verify the successful connect result was logged to saved networks manager
        assert_matches!(test_values.saved_networks_manager.get_recorded_connect_reslts().as_slice(), [data] => {
            let expected_connect_result = ConnectResultRecord {
                 id: connect_selection.target.network.clone(),
                 credential: connect_selection.target.credential.clone(),
                 bssid: types::Bssid::from(roam_result.bssid),
                 connect_result: fidl_sme::ConnectResult {
                    code: roam_result.status_code,
                    is_credential_rejected: roam_result.is_credential_rejected,
                    is_reconnect: false,
                 },
                 scan_type: types::ScanObservation::Unknown,
            };
            assert_eq!(data, &expected_connect_result);
        });

        // Verify telemetry event for roam result
        assert_matches!(telemetry_receiver.try_next(), Ok(Some(event)) => {
            assert_matches!(event, TelemetryEvent::PolicyInitiatedRoamResult { result, .. } => {
                assert_eq!(result, roam_result);
            });
        });

        // Explicitly verify there is _not_ a disconnect metric logged, since we have not exited the
        // ESS.
        assert_matches!(telemetry_receiver.try_next(), Err(_));

        // Run time forward past the timeout for the pending roam request.
        exec.set_fake_time(fasync::MonotonicInstant::after(
            PENDING_ROAM_TIMEOUT + zx::MonotonicDuration::from_millis(1),
        ));

        // Run the state machine and verify that it does NOT fire the pending timer and exit.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
    }

    #[fuchsia::test]
    fn connected_state_on_roam_result_failed_original_association_maintained() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));

        let mut test_values = test_setup();
        let mut telemetry_receiver = test_values.telemetry_receiver;

        let connect_selection = generate_connect_selection();
        let bss_description =
            Sequestered::release(connect_selection.target.bss.bss_description.clone());
        let ap_state =
            types::ApState::from(BssDescription::try_from(bss_description.clone()).unwrap());

        // Set up the state machine, starting at the connected state.
        let (connect_txn_proxy, connect_txn_stream) =
            create_proxy_and_stream::<fidl_sme::ConnectTransactionMarker>();
        let mut options = ConnectedOptions::new(
            &mut test_values.common_options,
            Box::new(ap_state.clone()),
            connect_selection.target.network_has_multiple_bss,
            connect_selection.target.network.clone(),
            connect_selection.target.credential.clone(),
            connect_selection.reason,
            connect_txn_proxy.take_event_stream(),
            false,
        );
        // Set the pending roam request, so we don't immediately exit when a result comes in.
        let policy_request = generate_policy_roam_request([1, 1, 1, 1, 1, 1].into());
        options.pending_roam = Some(policy_request.clone().into());
        let initial_state = connected_state(test_values.common_options, options);

        let connect_txn_handle = connect_txn_stream.control_handle();
        let fut = run_state_machine(initial_state);
        let mut fut = pin!(fut);

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify roam monitor request was sent.
        assert_matches!(test_values.roam_service_request_receiver.try_next(), Ok(Some(request)) => {
            assert_matches!(request, RoamServiceRequest::InitializeRoamMonitor { .. });
        });

        // Send a failed roam result, where the original association was maintained.
        let roam_result = fidl_sme::RoamResult {
            bssid: [1, 1, 1, 1, 1, 1],
            status_code: fidl_ieee80211::StatusCode::JoinFailure,
            original_association_maintained: true,
            bss_description: Some(Box::new(bss_description.clone())),
            disconnect_info: None,
            is_credential_rejected: false,
        };
        connect_txn_handle.send_on_roam_result(&roam_result).expect("failed to send roam result");
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify the failed connect result was logged to saved networks manager
        assert_matches!(test_values.saved_networks_manager.get_recorded_connect_reslts().as_slice(), [data] => {
            let expected_connect_result = ConnectResultRecord {
                 id: connect_selection.target.network.clone(),
                 credential: connect_selection.target.credential.clone(),
                 bssid: types::Bssid::from(roam_result.bssid),
                 connect_result: fidl_sme::ConnectResult {
                    code: roam_result.status_code,
                    is_credential_rejected: roam_result.is_credential_rejected,
                    is_reconnect: false,
                 },
                 scan_type: types::ScanObservation::Unknown,
            };
            assert_eq!(data, &expected_connect_result);
        });

        // A defect should be logged.
        assert_matches!(
            test_values.defect_receiver.try_next(),
            Ok(Some(Defect::Iface(IfaceFailure::ConnectionFailure { iface_id: 1 })))
        );

        // Verify telemetry event for roam result
        assert_matches!(telemetry_receiver.try_next(), Ok(Some(event)) => {
            assert_matches!(event, TelemetryEvent::PolicyInitiatedRoamResult { result, .. } => {
                assert_eq!(result, roam_result);
            });
        });

        // Explicitly verify there is _not_ a disconnect metric logged, since we have not exited the
        // ESS.
        assert_matches!(telemetry_receiver.try_next(), Err(_));

        // Verify the roam monitor was _not_ re-initialized.
        assert_matches!(test_values.roam_service_request_receiver.try_next(), Err(_));

        // Run time forward past the timeout for the pending roam request.
        exec.set_fake_time(fasync::MonotonicInstant::after(
            PENDING_ROAM_TIMEOUT + zx::MonotonicDuration::from_millis(1),
        ));

        // Run the state machine and verify that it does NOT fire the pending timer and exit.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
    }

    #[fuchsia::test]
    fn connected_state_on_roam_result_failed_and_disconnected() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));

        let mut test_values = test_setup();
        let mut telemetry_receiver = test_values.telemetry_receiver;
        let sme_fut = test_values.sme_req_stream.into_future();
        let mut sme_fut = pin!(sme_fut);

        let connect_selection = generate_connect_selection();
        let bss_description =
            Sequestered::release(connect_selection.target.bss.bss_description.clone());
        let ap_state =
            types::ApState::from(BssDescription::try_from(bss_description.clone()).unwrap());

        // Set up the state machine, starting at the connected state.
        let (connect_txn_proxy, connect_txn_stream) =
            create_proxy_and_stream::<fidl_sme::ConnectTransactionMarker>();
        let mut options = ConnectedOptions::new(
            &mut test_values.common_options,
            Box::new(ap_state.clone()),
            connect_selection.target.network_has_multiple_bss,
            connect_selection.target.network.clone(),
            connect_selection.target.credential.clone(),
            connect_selection.reason,
            connect_txn_proxy.take_event_stream(),
            false,
        );
        // Set the pending roam request, so we don't immediately exit when a result comes in.
        let policy_request = generate_policy_roam_request([1, 1, 1, 1, 1, 1].into());
        options.pending_roam = Some(policy_request.clone().into());
        let initial_state = connected_state(test_values.common_options, options);

        let connect_txn_handle = connect_txn_stream.control_handle();
        let fut = run_state_machine(initial_state);
        let mut fut = pin!(fut);

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify roam monitor request was sent.
        assert_matches!(test_values.roam_service_request_receiver.try_next(), Ok(Some(request)) => {
            assert_matches!(request, RoamServiceRequest::InitializeRoamMonitor { .. });
        });

        // Send a failed roam result, where the original association was *NOT* maintained.
        let disconnect_info = fidl_sme::DisconnectInfo {
            is_sme_reconnecting: false,
            disconnect_source: fidl_sme::DisconnectSource::Ap(fidl_sme::DisconnectCause {
                mlme_event_name: fidl_sme::DisconnectMlmeEventName::DisassociateIndication,
                reason_code: fidl_ieee80211::ReasonCode::UnspecifiedReason,
            }),
        };
        let roam_result = fidl_sme::RoamResult {
            bssid: [1, 1, 1, 1, 1, 1],
            status_code: fidl_ieee80211::StatusCode::JoinFailure,
            original_association_maintained: false,
            bss_description: None,
            disconnect_info: Some(Box::new(disconnect_info)),
            is_credential_rejected: false,
        };
        connect_txn_handle.send_on_roam_result(&roam_result).expect("failed to send roam result");
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify the failed connect result was logged to saved networks manager
        assert_matches!(test_values.saved_networks_manager.get_recorded_connect_reslts().as_slice(), [data] => {
            let expected_connect_result = ConnectResultRecord {
                 id: connect_selection.target.network.clone(),
                 credential: connect_selection.target.credential.clone(),
                 bssid: types::Bssid::from(roam_result.bssid),
                 connect_result: fidl_sme::ConnectResult {
                    code: roam_result.status_code,
                    is_credential_rejected: roam_result.is_credential_rejected,
                    is_reconnect: false,
                 },
                 scan_type: types::ScanObservation::Unknown,
            };
            assert_eq!(data, &expected_connect_result);
        });

        // Verify a disconnect was logged to saved networks manager
        assert_matches!(test_values.saved_networks_manager.get_recorded_past_connections().as_slice(), [ConnectionRecord {id, credential, data}] => {
            assert_eq!(id, &connect_selection.target.network.clone());
            assert_eq!(credential, &connect_selection.target.credential.clone());
            assert_matches!(data, PastConnectionData {bssid, disconnect_reason, ..} => {
                assert_eq!(bssid, &connect_selection.target.bss.bssid);
                assert_eq!(disconnect_reason, &types::DisconnectReason::Unknown);
            })
        });

        // Verify telemetry event for disconnect
        assert_matches!(telemetry_receiver.try_next(), Ok(Some(event)) => {
            assert_matches!(event, TelemetryEvent::Disconnected { info, .. } => {
                assert_eq!(info.unwrap().disconnect_source, disconnect_info.disconnect_source);
            });
        });

        // Verify telemetry event for roam result
        assert_matches!(telemetry_receiver.try_next(), Ok(Some(event)) => {
            assert_matches!(event, TelemetryEvent::PolicyInitiatedRoamResult { result, .. } => {
                assert_eq!(result, roam_result);
            });
        });

        // A defect should be logged.
        assert_matches!(
            test_values.defect_receiver.try_next(),
            Ok(Some(Defect::Iface(IfaceFailure::ConnectionFailure { iface_id: 1 })))
        );

        // Check for an SME disconnect request
        assert_matches!(
            poll_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ClientSmeRequest::Disconnect { .. })
        );

        // Run time forward past the timeout for the pending roam request.
        exec.set_fake_time(fasync::MonotonicInstant::after(
            PENDING_ROAM_TIMEOUT + zx::MonotonicDuration::from_millis(1),
        ));

        // Run the state machine and verify that it does NOT fire the pending timer and exit.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
    }
    #[fuchsia::test]
    fn connected_state_on_unexpected_policy_roam_result_disconnects() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));

        let mut test_values = test_setup();
        let mut telemetry_receiver = test_values.telemetry_receiver;
        let sme_fut = test_values.sme_req_stream.into_future();
        let mut sme_fut = pin!(sme_fut);

        let connect_selection = generate_connect_selection();
        let bss_description =
            Sequestered::release(connect_selection.target.bss.bss_description.clone());
        let ap_state =
            types::ApState::from(BssDescription::try_from(bss_description.clone()).unwrap());

        // Set up the state machine, starting at the connected state.
        let (connect_txn_proxy, connect_txn_stream) =
            create_proxy_and_stream::<fidl_sme::ConnectTransactionMarker>();
        let mut options = ConnectedOptions::new(
            &mut test_values.common_options,
            Box::new(ap_state.clone()),
            connect_selection.target.network_has_multiple_bss,
            connect_selection.target.network.clone(),
            connect_selection.target.credential.clone(),
            connect_selection.reason,
            connect_txn_proxy.take_event_stream(),
            false,
        );
        // Set the pending roam request for some BSSID.
        let policy_request = generate_policy_roam_request([1, 1, 1, 1, 1, 1].into());
        options.pending_roam = Some(policy_request.into());
        let initial_state = connected_state(test_values.common_options, options);

        let connect_txn_handle = connect_txn_stream.control_handle();
        let fut = run_state_machine(initial_state);
        let mut fut = pin!(fut);

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify roam monitor request was sent.
        assert_matches!(test_values.roam_service_request_receiver.try_next(), Ok(Some(request)) => {
            assert_matches!(request, RoamServiceRequest::InitializeRoamMonitor { .. });
        });

        // Send a roam result with a BSSID that does NOT match the roam request.
        let bss_desc = random_fidl_bss_description!();
        let roam_result = fidl_sme::RoamResult {
            bssid: [2, 2, 2, 2, 2, 2],
            status_code: fidl_ieee80211::StatusCode::Success,
            original_association_maintained: false,
            bss_description: Some(Box::new(bss_desc.clone())),
            disconnect_info: None,
            is_credential_rejected: false,
        };
        connect_txn_handle.send_on_roam_result(&roam_result).expect("failed to send roam result");

        // Run forward the state machine.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify a disconnect request is sent to SME.
        assert_matches!(
            poll_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ClientSmeRequest::Disconnect{ responder, reason: fidl_sme::UserDisconnectReason::DisconnectDetectedFromSme }) => {
                responder.send().expect("could not send sme response");
            }
        );

        // Ensure the state machine exits once the disconnect is processed.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));

        // Verify a disconnect was logged to saved networks manager
        assert_matches!(test_values.saved_networks_manager.get_recorded_past_connections().as_slice(), [ConnectionRecord {id, credential, data}] => {
            assert_eq!(id, &connect_selection.target.network.clone());
            assert_eq!(credential, &connect_selection.target.credential.clone());
            assert_matches!(data, PastConnectionData {bssid, disconnect_reason, ..} => {
                assert_eq!(bssid, &connect_selection.target.bss.bssid);
                assert_eq!(disconnect_reason, &types::DisconnectReason::Unknown);
            })
        });

        // Verify a disconnect event was logged to telemetry
        assert_matches!(telemetry_receiver.try_next(), Ok(Some(event)) => {
            assert_matches!(event, TelemetryEvent::Disconnected { info, .. } => {
                assert_eq!(info.unwrap().disconnect_source, fidl_sme::DisconnectSource::User(fidl_sme::UserDisconnectReason::Unknown));
            });
        });

        // Verify a disconnected listener update is sent.
        assert_matches!(
            test_values.update_receiver.try_next(),
            Ok(Some(listener::Message::NotifyListeners(ClientStateUpdate {
                state: fidl_policy::WlanClientState::ConnectionsEnabled,
                networks
            }))) => {
                assert_eq!(networks.len(), 1);
                assert_eq!(networks[0].id, connect_selection.target.network);
                assert_eq!(networks[0].state, fidl_policy::ConnectionState::Disconnected);
                assert_eq!(networks[0].status, Some(fidl_policy::DisconnectStatus::ConnectionFailed));
            }
        );
    }

    #[fuchsia::test]
    fn connected_state_on_unexpected_policy_roam_result_exits_on_broken_sme() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));

        let mut test_values = test_setup();
        let mut telemetry_receiver = test_values.telemetry_receiver;

        let connect_selection = generate_connect_selection();
        let bss_description =
            Sequestered::release(connect_selection.target.bss.bss_description.clone());
        let ap_state =
            types::ApState::from(BssDescription::try_from(bss_description.clone()).unwrap());

        // Set up the state machine, starting at the connected state.
        let (connect_txn_proxy, connect_txn_stream) =
            create_proxy_and_stream::<fidl_sme::ConnectTransactionMarker>();
        let mut options = ConnectedOptions::new(
            &mut test_values.common_options,
            Box::new(ap_state.clone()),
            connect_selection.target.network_has_multiple_bss,
            connect_selection.target.network.clone(),
            connect_selection.target.credential.clone(),
            connect_selection.reason,
            connect_txn_proxy.take_event_stream(),
            false,
        );
        // Set the pending roam request for some BSSID.
        let policy_request = generate_policy_roam_request([1, 1, 1, 1, 1, 1].into());
        options.pending_roam = Some(policy_request.into());
        let initial_state = connected_state(test_values.common_options, options);

        let connect_txn_handle = connect_txn_stream.control_handle();
        let fut = run_state_machine(initial_state);
        let mut fut = pin!(fut);

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify roam monitor request was sent.
        assert_matches!(test_values.roam_service_request_receiver.try_next(), Ok(Some(request)) => {
            assert_matches!(request, RoamServiceRequest::InitializeRoamMonitor { .. });
        });

        // Break the SME by dropping the server end of the SME stream, so it causes an error
        drop(test_values.sme_req_stream);

        // Send a roam result with a BSSID that does NOT match the roam request.
        let bss_desc = random_fidl_bss_description!();
        let roam_result = fidl_sme::RoamResult {
            bssid: [2, 2, 2, 2, 2, 2],
            status_code: fidl_ieee80211::StatusCode::Success,
            original_association_maintained: false,
            bss_description: Some(Box::new(bss_desc.clone())),
            disconnect_info: None,
            is_credential_rejected: false,
        };
        connect_txn_handle.send_on_roam_result(&roam_result).expect("failed to send roam result");

        // Ensure the state machine exits.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));

        // Verify a disconnect was logged to saved networks manager
        assert_matches!(test_values.saved_networks_manager.get_recorded_past_connections().as_slice(), [ConnectionRecord {id, credential, data}] => {
            assert_eq!(id, &connect_selection.target.network.clone());
            assert_eq!(credential, &connect_selection.target.credential.clone());
            assert_matches!(data, PastConnectionData {bssid, disconnect_reason, ..} => {
                assert_eq!(bssid, &connect_selection.target.bss.bssid);
                assert_eq!(disconnect_reason, &types::DisconnectReason::Unknown);
            })
        });

        // Verify a disconnect event was logged to telemetry
        assert_matches!(telemetry_receiver.try_next(), Ok(Some(event)) => {
            assert_matches!(event, TelemetryEvent::Disconnected { info, .. } => {
                assert_eq!(info.unwrap().disconnect_source, fidl_sme::DisconnectSource::User(fidl_sme::UserDisconnectReason::Unknown));
            });
        });

        // Verify a disconnected listener update is sent.
        assert_matches!(
            test_values.update_receiver.try_next(),
            Ok(Some(listener::Message::NotifyListeners(ClientStateUpdate {
                state: fidl_policy::WlanClientState::ConnectionsEnabled,
                networks
            }))) => {
                assert_eq!(networks.len(), 1);
                assert_eq!(networks[0].id, connect_selection.target.network);
                assert_eq!(networks[0].state, fidl_policy::ConnectionState::Disconnected);
                assert_eq!(networks[0].status, Some(fidl_policy::DisconnectStatus::ConnectionFailed));
            }
        );
    }

    #[fuchsia::test]
    fn connected_state_pending_roam_expires() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();

        let mut test_values = test_setup();
        let sme_fut = test_values.sme_req_stream.into_future();
        let mut sme_fut = pin!(sme_fut);

        let connect_selection = generate_connect_selection();
        let bss_description =
            Sequestered::release(connect_selection.target.bss.bss_description.clone());
        let ap_state =
            types::ApState::from(BssDescription::try_from(bss_description.clone()).unwrap());

        let (connect_txn_proxy, _connect_txn_stream) =
            create_proxy_and_stream::<fidl_sme::ConnectTransactionMarker>();
        let options = ConnectedOptions::new(
            &mut test_values.common_options,
            Box::new(ap_state.clone()),
            connect_selection.target.network_has_multiple_bss,
            connect_selection.target.network.clone(),
            connect_selection.target.credential.clone(),
            connect_selection.reason,
            connect_txn_proxy.take_event_stream(),
            false,
        );
        // Set the pending roam request for some BSSID.
        let initial_state = connected_state(test_values.common_options, options);
        let fut = run_state_machine(initial_state);
        let mut fut = pin!(fut);

        // Verify roam monitor init was sent.
        let mut roam_sender;
        assert_matches!(test_values.roam_service_request_receiver.try_next(), Ok(Some(request)) => {
            assert_matches!(request, RoamServiceRequest::InitializeRoamMonitor { roam_request_sender, .. } => {
                roam_sender = roam_request_sender;
            });
        });

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Send a roam request to state machine.
        let roam_candidate = generate_random_scanned_candidate();
        roam_sender
            .try_send(PolicyRoamRequest { candidate: roam_candidate.clone(), reasons: vec![] })
            .unwrap();

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify state machine issues a roam to SME.
        assert_matches!(
            poll_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ClientSmeRequest::Roam{ req, ..}) => {
                assert_eq!(req.bss_description, Sequestered::release(roam_candidate.clone().bss.bss_description));
            }
        );

        // Verify roam attempt telemetry event.
        assert_matches!(test_values.telemetry_receiver.try_next(), Ok(Some(event)) => {
            assert_matches!(event, TelemetryEvent::PolicyRoamAttempt { request, .. } => {
                assert_eq!(request.candidate,
                    roam_candidate);
                assert_eq!(request.reasons, vec![]);
            });
        });

        // Run time forward past the timeout for the pending roam request.
        exec.set_fake_time(fasync::MonotonicInstant::after(
            PENDING_ROAM_TIMEOUT + zx::MonotonicDuration::from_millis(1),
        ));

        // Run forward the state machine.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify a disconnect request is sent to SME.
        assert_matches!(
            poll_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ClientSmeRequest::Disconnect{ responder, reason: fidl_sme::UserDisconnectReason::DisconnectDetectedFromSme }) => {
                responder.send().expect("could not send sme response");
            }
        );

        // Ensure the state machine exits once the disconnect is processed.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));

        // Verify a disconnect was logged to saved networks manager
        assert_matches!(test_values.saved_networks_manager.get_recorded_past_connections().as_slice(), [ConnectionRecord {id, credential, data}] => {
            assert_eq!(id, &connect_selection.target.network.clone());
            assert_eq!(credential, &connect_selection.target.credential.clone());
            assert_matches!(data, PastConnectionData {bssid, disconnect_reason, ..} => {
                assert_eq!(bssid, &connect_selection.target.bss.bssid);
                assert_eq!(disconnect_reason, &types::DisconnectReason::Unknown);
            })
        });

        // Verify a disconnect event was logged to telemetry
        assert_matches!(test_values.telemetry_receiver.try_next(), Ok(Some(event)) => {
            assert_matches!(event, TelemetryEvent::Disconnected { info, .. } => {
                assert_eq!(info.unwrap().disconnect_source, fidl_sme::DisconnectSource::User(fidl_sme::UserDisconnectReason::Unknown));
            });
        });

        // Verify a disconnected listener update is sent.
        assert_matches!(
            test_values.update_receiver.try_next(),
            Ok(Some(listener::Message::NotifyListeners(ClientStateUpdate {
                state: fidl_policy::WlanClientState::ConnectionsEnabled,
                networks
            }))) => {
                assert_eq!(networks.len(), 1);
                assert_eq!(networks[0].id, connect_selection.target.network);
                assert_eq!(networks[0].state, fidl_policy::ConnectionState::Disconnected);
                assert_eq!(networks[0].status, Some(fidl_policy::DisconnectStatus::ConnectionFailed));
            }
        );
    }

    #[fuchsia::test]
    fn disconnecting_state_completes_and_exits() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = test_setup();

        let (sender, _) = oneshot::channel();
        let disconnecting_options = DisconnectingOptions {
            disconnect_responder: Some(sender),
            previous_network: None,
            next_network: None,
            reason: types::DisconnectReason::RegulatoryRegionChange,
        };
        let initial_state = disconnecting_state(test_values.common_options, disconnecting_options);
        let fut = run_state_machine(initial_state);
        let mut fut = pin!(fut);
        let sme_fut = test_values.sme_req_stream.into_future();
        let mut sme_fut = pin!(sme_fut);

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Ensure a disconnect request is sent to the SME
        assert_matches!(
            poll_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ClientSmeRequest::Disconnect{ responder, reason: fidl_sme::UserDisconnectReason::RegulatoryRegionChange }) => {
                responder.send().expect("could not send sme response");
            }
        );

        // Ensure the state machine exits once the disconnect is processed.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));

        // The state machine should have sent a listener update
        assert_matches!(
            test_values.update_receiver.try_next(),
            Ok(Some(listener::Message::NotifyListeners(ClientStateUpdate {
                state: fidl_policy::WlanClientState::ConnectionsEnabled,
                networks
            }))) => {
                assert!(networks.is_empty());
            }
        );
    }

    #[fuchsia::test]
    fn disconnecting_state_completes_disconnect_to_connecting() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = test_setup();

        let previous_connect_selection = generate_connect_selection();
        let next_connect_selection = generate_connect_selection();

        let bss_description =
            Sequestered::release(next_connect_selection.target.bss.bss_description.clone());

        let (disconnect_sender, mut disconnect_receiver) = oneshot::channel();
        let connecting_options = ConnectingOptions {
            connect_selection: next_connect_selection.clone(),
            attempt_counter: 0,
        };
        // Include both a "previous" and "next" network
        let disconnecting_options = DisconnectingOptions {
            disconnect_responder: Some(disconnect_sender),
            previous_network: Some((
                previous_connect_selection.target.network.clone(),
                fidl_policy::DisconnectStatus::ConnectionStopped,
            )),
            next_network: Some(connecting_options),
            reason: types::DisconnectReason::ProactiveNetworkSwitch,
        };
        let initial_state = disconnecting_state(test_values.common_options, disconnecting_options);
        let fut = run_state_machine(initial_state);
        let mut fut = pin!(fut);
        let sme_fut = test_values.sme_req_stream.into_future();
        let mut sme_fut = pin!(sme_fut);

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Ensure a disconnect request is sent to the SME
        assert_matches!(
            poll_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ClientSmeRequest::Disconnect{ responder, reason: fidl_sme::UserDisconnectReason::ProactiveNetworkSwitch }) => {
                responder.send().expect("could not send sme response");
            }
        );

        // Progress the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Check for a disconnect update and the disconnect responder
        let client_state_update = ClientStateUpdate {
            state: fidl_policy::WlanClientState::ConnectionsEnabled,
            networks: vec![ClientNetworkState {
                id: previous_connect_selection.target.network.clone(),
                state: fidl_policy::ConnectionState::Disconnected,
                status: Some(fidl_policy::DisconnectStatus::ConnectionStopped),
            }],
        };
        assert_matches!(
            test_values.update_receiver.try_next(),
            Ok(Some(listener::Message::NotifyListeners(updates))) => {
            assert_eq!(updates, client_state_update);
        });

        assert_matches!(exec.run_until_stalled(&mut disconnect_receiver), Poll::Ready(Ok(())));

        // Ensure a connect request is sent to the SME
        assert_matches!(
            poll_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ClientSmeRequest::Connect{ req, txn, control_handle: _ }) => {
                assert_eq!(req.ssid, next_connect_selection.target.network.ssid.clone().to_vec());
                assert_eq!(req.deprecated_scan_type, fidl_fuchsia_wlan_common::ScanType::Active);
                assert_eq!(req.bss_description, bss_description.clone());
                assert_eq!(req.multiple_bss_candidates, next_connect_selection.target.network_has_multiple_bss);
                 // Send connection response.
                let (_stream, ctrl) = txn.expect("connect txn unused")
                    .into_stream_and_control_handle();
                ctrl
                    .send_on_connect_result(&fake_successful_connect_result())
                    .expect("failed to send connection completion");
            }
        );
    }

    #[fuchsia::test]
    fn disconnecting_state_has_broken_sme() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();

        let (sender, mut receiver) = oneshot::channel();
        let disconnecting_options = DisconnectingOptions {
            disconnect_responder: Some(sender),
            previous_network: None,
            next_network: None,
            reason: types::DisconnectReason::NetworkConfigUpdated,
        };
        let initial_state = disconnecting_state(test_values.common_options, disconnecting_options);
        let fut = run_state_machine(initial_state);
        let mut fut = pin!(fut);

        // Break the SME by dropping the server end of the SME stream, so it causes an error
        drop(test_values.sme_req_stream);

        // Ensure the state machine exits
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));

        // Expect the responder to have an error
        assert_matches!(exec.run_until_stalled(&mut receiver), Poll::Ready(Err(_)));
    }

    #[fuchsia::test]
    fn serve_loop_handles_startup() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let sme_proxy = test_values.common_options.proxy;
        let sme_event_stream = sme_proxy.take_event_stream();
        let (_client_req_sender, client_req_stream) = mpsc::channel(1);
        let sme_fut = test_values.sme_req_stream.into_future();
        let mut sme_fut = pin!(sme_fut);

        // Create a connect request so that the state machine does not immediately exit.
        let connect_selection = generate_connect_selection();

        let fut = serve(
            0,
            sme_proxy,
            sme_event_stream,
            client_req_stream,
            test_values.common_options.update_sender,
            test_values.common_options.saved_networks_manager,
            Some(connect_selection),
            test_values.common_options.telemetry_sender,
            test_values.common_options.defect_sender,
            test_values.common_options.roam_manager,
            test_values.common_options.status_publisher,
        );
        let mut fut = pin!(fut);

        // Run the state machine so it sends the initial SME disconnect request.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        assert_matches!(
            poll_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ClientSmeRequest::Disconnect{ responder, reason: fidl_sme::UserDisconnectReason::Startup }) => {
                responder.send().expect("could not send sme response");
            }
        );

        // Run the future again and ensure that it has not exited after receiving the response.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
    }

    #[fuchsia::test]
    fn serve_loop_handles_sme_disappearance() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = test_setup();
        let (_client_req_sender, client_req_stream) = mpsc::channel(1);

        // Make our own SME proxy for this test
        let (sme_proxy, sme_server) = create_proxy::<fidl_sme::ClientSmeMarker>();
        let (sme_req_stream, sme_control_handle) = sme_server.into_stream_and_control_handle();

        let sme_fut = sme_req_stream.into_future();
        let mut sme_fut = pin!(sme_fut);

        let sme_event_stream = sme_proxy.take_event_stream();

        // Create a connect request so that the state machine does not immediately exit.
        let connect_selection = generate_connect_selection();

        let fut = serve(
            0,
            SmeForClientStateMachine::new(
                sme_proxy,
                0,
                test_values.common_options.defect_sender.clone(),
            ),
            sme_event_stream,
            client_req_stream,
            test_values.common_options.update_sender,
            test_values.common_options.saved_networks_manager,
            Some(connect_selection),
            test_values.common_options.telemetry_sender,
            test_values.common_options.defect_sender,
            test_values.common_options.roam_manager,
            test_values.common_options.status_publisher,
        );
        let mut fut = pin!(fut);

        // Run the state machine so it sends the initial SME disconnect request.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        assert_matches!(
            poll_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ClientSmeRequest::Disconnect{ responder, reason: fidl_sme::UserDisconnectReason::Startup }) => {
                responder.send().expect("could not send sme response");
            }
        );

        // Run the future again and ensure that it has not exited after receiving the response.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        sme_control_handle.shutdown_with_epitaph(zx::Status::UNAVAILABLE);

        // Ensure the state machine has no further actions and is exited
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));

        // Verify that a disconnect event was logged on exit.
        let mut telemetry_events = Vec::new();
        while let Ok(Some(event)) = test_values.telemetry_receiver.try_next() {
            telemetry_events.push(event)
        }

        assert_matches!(
            telemetry_events.last(),
            Some(&TelemetryEvent::Disconnected { track_subsequent_downtime: false, info: None })
        );
    }

    #[fuchsia::test]
    fn serve_loop_handles_disconnect() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = test_setup();
        let sme_proxy = test_values.common_options.proxy;
        let sme_event_stream = sme_proxy.take_event_stream();
        let (client_req_sender, client_req_stream) = mpsc::channel(1);
        let sme_fut = test_values.sme_req_stream.into_future();
        let mut sme_fut = pin!(sme_fut);

        // Create a connect request so that the state machine does not immediately exit.
        let connect_selection = generate_connect_selection();
        let fut = serve(
            0,
            sme_proxy,
            sme_event_stream,
            client_req_stream,
            test_values.common_options.update_sender,
            test_values.common_options.saved_networks_manager,
            Some(connect_selection),
            test_values.common_options.telemetry_sender,
            test_values.common_options.defect_sender,
            test_values.common_options.roam_manager,
            test_values.common_options.status_publisher,
        );
        let mut fut = pin!(fut);

        // Run the state machine so it sends the initial SME disconnect request.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        assert_matches!(
            poll_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ClientSmeRequest::Disconnect{ responder, reason: fidl_sme::UserDisconnectReason::Startup }) => {
                responder.send().expect("could not send sme response");
            }
        );

        // Run the future again and ensure that it has not exited after receiving the response.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Absorb the connect request.
        let connect_txn_handle = assert_matches!(
            poll_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ClientSmeRequest::Connect{ req: _, txn, control_handle: _ }) => {
                // Send connection response.
                let (_stream, ctrl) = txn.expect("connect txn unused")
                    .into_stream_and_control_handle();
                ctrl
            }
        );
        connect_txn_handle
            .send_on_connect_result(&fake_successful_connect_result())
            .expect("failed to send connection completion");
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify roam monitor request was sent.
        assert_matches!(test_values.roam_service_request_receiver.try_next(), Ok(Some(request)) => {
            assert_matches!(request, RoamServiceRequest::InitializeRoamMonitor { .. });
        });

        // Run the state machine
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Send a disconnect request
        let mut client = Client::new(client_req_sender);
        let (sender, mut receiver) = oneshot::channel();
        client
            .disconnect(
                PolicyDisconnectionMigratedMetricDimensionReason::NetworkConfigUpdated,
                sender,
            )
            .expect("failed to make request");

        // Run the state machine so that it handles the disconnect message.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        assert_matches!(
            poll_sme_req(&mut exec, &mut sme_fut),
            Poll::Ready(fidl_sme::ClientSmeRequest::Disconnect{ responder, reason: fidl_sme::UserDisconnectReason::NetworkConfigUpdated }) => {
                responder.send().expect("could not send sme response");
            }
        );

        // The state machine should exit following the disconnect request.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));

        // Expect the responder to be acknowledged
        assert_matches!(exec.run_until_stalled(&mut receiver), Poll::Ready(Ok(())));
    }

    #[fuchsia::test]
    fn serve_loop_handles_state_machine_error() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let sme_proxy = test_values.common_options.proxy;
        let sme_event_stream = sme_proxy.take_event_stream();
        let (_client_req_sender, client_req_stream) = mpsc::channel(1);

        // Set the status to something non-disconnected so that we can verify that the state is set
        // when the state machine exits.
        test_values.common_options.status_publisher.publish_status(Status::Connecting);

        // Create a connect request so that the state machine does not immediately exit.
        let connect_selection = generate_connect_selection();

        let fut = serve(
            0,
            sme_proxy,
            sme_event_stream,
            client_req_stream,
            test_values.common_options.update_sender,
            test_values.common_options.saved_networks_manager,
            Some(connect_selection),
            test_values.common_options.telemetry_sender,
            test_values.common_options.defect_sender,
            test_values.common_options.roam_manager,
            test_values.common_options.status_publisher.clone(),
        );
        let mut fut = pin!(fut);

        // Drop the server end of the SME stream, so it causes an error
        drop(test_values.sme_req_stream);

        // Ensure the state machine exits
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));

        // Verify that the state has been set to disconnected.
        let status = test_values.status_reader.read_status().expect("could not get reader");
        assert_eq!(status, Status::Disconnected);
    }

    fn fake_successful_connect_result() -> fidl_sme::ConnectResult {
        fidl_sme::ConnectResult {
            code: fidl_ieee80211::StatusCode::Success,
            is_credential_rejected: false,
            is_reconnect: false,
        }
    }

    #[fuchsia::test]
    fn disconnecting_sets_status() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();

        let (sender, _) = oneshot::channel();
        let disconnecting_options = DisconnectingOptions {
            disconnect_responder: Some(sender),
            previous_network: None,
            next_network: None,
            reason: types::DisconnectReason::RegulatoryRegionChange,
        };

        // Run the disconnecting state machine.
        let initial_state = disconnecting_state(test_values.common_options, disconnecting_options);
        let fut = run_state_machine(initial_state);
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify the disconnecting state has been reported.
        let status = test_values.status_reader.read_status().expect("could not get reader");
        assert_eq!(status, Status::Disconnecting);
    }

    #[fuchsia::test]
    fn connecting_sets_status() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let connection_attempt_time = fasync::MonotonicInstant::from_nanos(0);
        exec.set_fake_time(connection_attempt_time);
        let test_values = test_setup();

        let connect_selection = generate_connect_selection();
        let connecting_options =
            ConnectingOptions { connect_selection: connect_selection.clone(), attempt_counter: 0 };

        // Run the connecting state
        let initial_state = connecting_state(test_values.common_options, connecting_options);
        let fut = run_state_machine(initial_state);
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify the status was set.
        let status = test_values.status_reader.read_status().expect("failed to read status");
        assert_matches!(status, Status::Connecting);
    }

    struct InspectTestValues {
        exec: fasync::TestExecutor,
        inspector: fuchsia_inspect::Inspector,
        _node: fuchsia_inspect::Node,
        status_node: fuchsia_inspect_contrib::nodes::BoundedListNode,
    }

    impl InspectTestValues {
        fn new(exec: fasync::TestExecutor) -> Self {
            let inspector = fuchsia_inspect::Inspector::default();
            let _node = inspector.root().create_child("node");
            let status_node =
                fuchsia_inspect_contrib::nodes::BoundedListNode::new(_node.clone_weak(), 1);

            Self { exec, inspector, _node, status_node }
        }

        fn log_status(&mut self, status: Status) -> fuchsia_inspect::reader::DiagnosticsHierarchy {
            fuchsia_inspect_contrib::inspect_log!(self.status_node, "status" => status);
            let read_fut = fuchsia_inspect::reader::read(&self.inspector);
            let mut read_fut = pin!(read_fut);
            assert_matches!(
                self.exec.run_until_stalled(&mut read_fut),
                Poll::Ready(Ok(hierarchy)) => hierarchy
            )
        }
    }

    #[fuchsia::test]
    fn test_disconnecting_status_inspect_log() {
        let exec = fasync::TestExecutor::new_with_fake_time();
        let mut test_values = InspectTestValues::new(exec);
        let hierarchy = test_values.log_status(Status::Disconnecting);
        diagnostics_assertions::assert_data_tree!(
            @executor test_values.exec,
            hierarchy,
            root: contains {
                node: contains {
                    "0": contains {
                        status: "Disconnecting"
                    }
                }
        });
    }

    #[fuchsia::test]
    fn test_disconnected_status_inspect_log() {
        let exec = fasync::TestExecutor::new_with_fake_time();
        let mut test_values = InspectTestValues::new(exec);
        let hierarchy = test_values.log_status(Status::Disconnected);
        diagnostics_assertions::assert_data_tree!(
            @executor test_values.exec,
            hierarchy,
            root: contains {
                node: contains {
                    "0": contains {
                        status: "Disconnected"
                    }
                }
        });
    }

    #[fuchsia::test]
    fn test_connecting_status_inspect_log() {
        let exec = fasync::TestExecutor::new_with_fake_time();
        let mut test_values = InspectTestValues::new(exec);
        let hierarchy = test_values.log_status(Status::Connecting);
        diagnostics_assertions::assert_data_tree!(
            @executor test_values.exec,
            hierarchy,
            root: contains {
                node: contains {
                    "0": contains {
                        status: "Connecting"
                    }
                }
        });
    }

    #[fuchsia::test]
    fn test_connected_status_inspect_log() {
        let exec = fasync::TestExecutor::new_with_fake_time();
        let mut test_values = InspectTestValues::new(exec);
        let hierarchy = test_values.log_status(Status::Connected { channel: 1, rssi: 2, snr: 3 });
        diagnostics_assertions::assert_data_tree!(
            @executor test_values.exec,
            hierarchy,
            root: contains {
                node: contains {
                    "0": contains {
                        status: contains {
                            Connected: { channel: 1_u64, rssi: 2_i64, snr: 3_i64 }
                        }
                    }
                }
        });
    }
}
