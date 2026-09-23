// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::access_point::state_machine::AccessPointApi;
use crate::access_point::{state_machine as ap_fsm, types as ap_types};
use crate::client::connection_selection::ConnectionSelectionRequester;
use crate::client::connection_selection::fut_manager::{
    ConnectionSelectionFutures, ConnectionSelectionManager, ConnectionSelectionResponse,
    SelectionIdentifier,
};
use crate::client::roaming::local_roam_manager::RoamManager;
use crate::client::{state_machine as client_fsm, types as client_types};
use crate::config_management::SavedNetworksManagerApi;
use crate::mode_management::iface_manager_api::{
    ConnectAttemptRequest, SmeForApStateMachine, SmeForClientStateMachine, SmeForScan,
};
use crate::mode_management::iface_manager_types::*;
use crate::mode_management::phy_manager::{CreateClientIfacesReason, PhyManagerApi};
use crate::mode_management::{Defect, recovery};
use crate::telemetry::{TelemetryEvent, TelemetrySender};
use crate::util::state_machine::{StateMachineStatusReader, status_publisher_and_reader};
use crate::util::{future_with_metadata, listener};
use anyhow::{Error, format_err};
use async_utils::event::Event;
use fidl::endpoints::create_proxy;
use fuchsia_async as fasync;
use fuchsia_inspect_contrib::log::InspectListClosure;
use fuchsia_inspect_contrib::nodes::BoundedListNode;
use fuchsia_inspect_contrib::{inspect_insert, inspect_log};
use futures::channel::{mpsc, oneshot};
use futures::future::{FutureExt, LocalBoxFuture, ready};
use futures::lock::Mutex;
use futures::stream::FuturesUnordered;
use futures::{StreamExt, select};
use log::{debug, error, info, warn};
use std::convert::Infallible;
use std::fmt::Debug;

use std::sync::Arc;

// Maximum allowed interval between scans when attempting to reconnect client interfaces.  This
// value is taken from legacy state machine.
const MAX_AUTO_CONNECT_RETRY_SECONDS: i64 = 10;
const INSPECT_RECOVERY_INTERFACE_RECORDS: usize = 14;

#[derive(Clone, PartialEq)]
#[cfg_attr(test, derive(Debug))]
enum ClientIfaceContainerConfig {
    Unconfigured,
    Configured(ap_types::NetworkIdentifier),
}

/// Wraps around vital information associated with a WLAN client interface.  In all cases, a client
/// interface will have an ID and a ClientSmeProxy to make requests of the interface.  If a client
/// is configured to connect to a WLAN network, it will store the network configuration information
/// associated with that network as well as a communication channel to make requests of the state
/// machine that maintains client connectivity.
struct ClientIfaceContainer {
    iface_id: u16,
    sme_proxy: SmeForClientStateMachine,
    config: ClientIfaceContainerConfig,
    client_state_machine: Option<Box<dyn client_fsm::ClientApi>>,
    state_machine_task: Option<fasync::Task<()>>,
    /// The time of the last scan for roaming or new connection on this iface.
    last_roam_time: fasync::MonotonicInstant,
    status: StateMachineStatusReader<client_fsm::Status>,
}

pub(crate) struct ApIfaceContainer {
    pub iface_id: u16,
    pub config: Option<ap_fsm::ApConfig>,
    pub ap_state_machine: Box<dyn AccessPointApi>,
    // By default, Tasks are killed if their handles are dropped.  While a workaround is to detach
    // the task, fuchsia_async documentation notes that this can lead to strange behavior in tests
    // since the lifetime of the Task is now bound to the lifetime of the executor.  By storing the
    // Tasks in the IfaceContainers, their lifetimes are shared with their parent
    // IfaceManagerService instances.  Dropping an IfaceManagerService instance will result in all
    // IfaceContainers dropping, resulting in the termination of any running Tasks.
    #[expect(unused)]
    state_machine_task: fasync::Task<()>,
    enabled_time: Option<zx::MonotonicInstant>,
    status: StateMachineStatusReader<ap_fsm::Status>,
}

#[derive(Clone, Debug)]
pub struct StateMachineMetadata {
    pub iface_id: u16,
    pub role: fidl_fuchsia_wlan_common::WlanMacRole,
}

async fn create_client_state_machine(
    iface_id: u16,
    dev_monitor_proxy: &mut fidl_fuchsia_wlan_device_service::DeviceMonitorProxy,
    client_update_sender: listener::ClientListenerMessageSender,
    saved_networks: Arc<dyn SavedNetworksManagerApi>,
    connect_selection: Option<client_types::ConnectSelection>,
    telemetry_sender: TelemetrySender,
    defect_sender: mpsc::Sender<Defect>,
    roam_manager: RoamManager,
) -> Result<
    (
        Box<dyn client_fsm::ClientApi>,
        fasync::Task<()>,
        future_with_metadata::FutureWithMetadata<(), StateMachineMetadata>,
        StateMachineStatusReader<client_fsm::Status>,
    ),
    Error,
> {
    if connect_selection.is_some() {
        telemetry_sender.send(TelemetryEvent::StartEstablishConnection { reset_start_time: false });
    }

    // Create a client state machine for the newly discovered interface.
    let (sender, receiver) = mpsc::channel(1);
    let new_client = client_fsm::Client::new(sender);

    // Create a new client SME proxy.  This is required because each new client state machine will
    // take the event stream from the SME proxy.  A subsequent attempt to take the event stream
    // would cause wlancfg to panic.
    let (sme_proxy, remote) = create_proxy();
    dev_monitor_proxy.get_client_sme(iface_id, remote).await?.map_err(zx::Status::err_from_raw)?;
    let event_stream = sme_proxy.take_event_stream();
    let sme_proxy = SmeForClientStateMachine::new(sme_proxy, iface_id, defect_sender.clone());

    // State machine status information
    let (publisher, status) = status_publisher_and_reader::<client_fsm::Status>();
    let termination_event = Event::new();
    let termination_notifier = termination_event.clone();
    let fut = client_fsm::serve(
        iface_id,
        sme_proxy,
        event_stream,
        receiver,
        client_update_sender,
        saved_networks,
        connect_selection,
        telemetry_sender,
        defect_sender,
        roam_manager,
        publisher,
    )
    .then(|()| async move {
        if !termination_notifier.signal() {
            warn!("Client state machine: signaling termination_event unexpectedly returned false.")
        }
    })
    .boxed_local();
    let task = fasync::Task::local(fut);

    let metadata =
        StateMachineMetadata { iface_id, role: fidl_fuchsia_wlan_common::WlanMacRole::Client };
    let fut = future_with_metadata::FutureWithMetadata::new(
        metadata,
        async move {
            // Wait for the state machine to either exit gracefully and signal the event or for the
            // event to be dropped due to a task cancellation.
            let _ = termination_event.wait_or_dropped().await;
        }
        .boxed_local(),
    );

    Ok((Box::new(new_client), task, fut, status))
}

/// Accounts for WLAN interfaces that are present and utilizes them to service requests that are
/// made of the policy layer.
pub(crate) struct IfaceManagerService {
    phy_manager: Arc<Mutex<dyn PhyManagerApi>>,
    client_update_sender: listener::ClientListenerMessageSender,
    ap_update_sender: listener::ApListenerMessageSender,
    dev_monitor_proxy: fidl_fuchsia_wlan_device_service::DeviceMonitorProxy,
    clients: Vec<ClientIfaceContainer>,
    aps: Vec<ApIfaceContainer>,
    saved_networks: Arc<dyn SavedNetworksManagerApi>,

    roam_manager: RoamManager,
    fsm_termination_futures:
        FuturesUnordered<future_with_metadata::FutureWithMetadata<(), StateMachineMetadata>>,
    connection_selection_manager: ConnectionSelectionManager,
    telemetry_sender: TelemetrySender,
    // A sender to be cloned for state machines to report defects to the IfaceManager.
    defect_sender: mpsc::Sender<Defect>,
    _node: fuchsia_inspect::Node,
    recovery_node: BoundedListNode,
}

impl IfaceManagerService {
    pub fn new(
        phy_manager: Arc<Mutex<dyn PhyManagerApi>>,
        client_update_sender: listener::ClientListenerMessageSender,
        ap_update_sender: listener::ApListenerMessageSender,
        dev_monitor_proxy: fidl_fuchsia_wlan_device_service::DeviceMonitorProxy,
        saved_networks: Arc<dyn SavedNetworksManagerApi>,
        connection_selection_requester: ConnectionSelectionRequester,
        roam_manager: RoamManager,
        telemetry_sender: TelemetrySender,
        defect_sender: mpsc::Sender<Defect>,
        _node: fuchsia_inspect::Node,
    ) -> Self {
        let recovery_node = _node.create_child("recovery_record");
        let recovery_node = BoundedListNode::new(recovery_node, INSPECT_RECOVERY_INTERFACE_RECORDS);
        IfaceManagerService {
            phy_manager: phy_manager.clone(),
            client_update_sender,
            ap_update_sender,
            dev_monitor_proxy,
            clients: Vec::new(),
            aps: Vec::new(),
            saved_networks,
            roam_manager,
            fsm_termination_futures: FuturesUnordered::new(),
            connection_selection_manager: ConnectionSelectionManager::new(
                connection_selection_requester,
            ),
            telemetry_sender,
            defect_sender,
            _node,
            recovery_node,
        }
    }

    /// Checks for any known, unconfigured clients.  If one exists, its ClientIfaceContainer is
    /// returned to the caller.
    ///
    /// If all ifaces are configured, asks the PhyManager for an iface ID.  If the returned iface
    /// ID is an already-configured iface, that iface is removed from the configured clients list
    /// and returned to the caller.
    ///
    /// If it is not, a new ClientIfaceContainer is created from the returned iface ID and returned
    /// to the caller.
    async fn get_client(&mut self, iface_id: Option<u16>) -> Result<ClientIfaceContainer, Error> {
        let iface_id = match iface_id {
            Some(iface_id) => iface_id,
            None => {
                // If no iface_id is specified and if there there are any unconfigured client
                // ifaces, use the first available unconfigured iface.
                if let Some(removal_index) = self.clients.iter().position(|client_container| {
                    client_container.config == ClientIfaceContainerConfig::Unconfigured
                }) {
                    return Ok(self.clients.remove(removal_index));
                }

                // If all of the known client ifaces are configured, ask the PhyManager for a
                // client iface.
                match self.phy_manager.lock().await.get_client() {
                    None => return Err(format_err!("no client ifaces available")),
                    Some(id) => id,
                }
            }
        };
        self.setup_client_container(iface_id).await
    }

    // Retrieves a client iface container where the config has been set to Configured for a given
    // network id.
    fn get_configured_client(
        &mut self,
        network_id: client_types::NetworkIdentifier,
    ) -> Result<ClientIfaceContainer, Error> {
        if let Some(removal_index) = self.clients.iter().position(|client_container| {
            client_container.config == ClientIfaceContainerConfig::Configured(network_id.clone())
        }) {
            return Ok(self.clients.remove(removal_index));
        }
        Err(format_err!("No configured client iface container found for network id"))
    }

    /// Remove the client iface from the list of configured ifaces if it is there, and create a
    /// ClientIfaceContainer for it if it is needed.
    async fn setup_client_container(
        &mut self,
        iface_id: u16,
    ) -> Result<ClientIfaceContainer, Error> {
        // See if the selected iface ID is among the configured clients.
        if let Some(removal_index) =
            self.clients.iter().position(|client_container| client_container.iface_id == iface_id)
        {
            return Ok(self.clients.remove(removal_index));
        }

        // If the iface ID is not among configured clients, create a new ClientIfaceContainer for
        // the iface ID.
        let (sme_proxy, sme_server) = create_proxy();
        self.dev_monitor_proxy
            .get_client_sme(iface_id, sme_server)
            .await?
            .map_err(zx::Status::err_from_raw)?;
        let sme_proxy =
            SmeForClientStateMachine::new(sme_proxy, iface_id, self.defect_sender.clone());

        // Setup a status reader for the container.  Since this state machine is uninitialized,
        // drop the publishing end and allow the reader side to assume the default state.
        let (_publisher, status) = status_publisher_and_reader::<client_fsm::Status>();

        Ok(ClientIfaceContainer {
            iface_id,
            sme_proxy,
            config: ClientIfaceContainerConfig::Unconfigured,
            client_state_machine: None,
            state_machine_task: None,
            last_roam_time: fasync::MonotonicInstant::now(),
            status,
        })
    }

    /// Queries to PhyManager to determine if there are any interfaces that can be used as AP's.
    ///
    /// If the PhyManager indicates that there is an existing interface that should be used for the
    /// AP request, return the existing AP interface.
    ///
    /// If the indicated AP interface has not been used before, spawn a new AP state machine for
    /// the interface and return the new interface.
    async fn get_ap(&mut self, iface_id: Option<u16>) -> Result<ApIfaceContainer, Error> {
        let iface_id = match iface_id {
            Some(iface_id) => iface_id,
            None => {
                // If no iface ID is specified, ask the PhyManager for an AP iface ID.
                let mut phy_manager = self.phy_manager.lock().await;
                match phy_manager.create_or_get_ap_iface().await {
                    Ok(Some(iface_id)) => iface_id,
                    Ok(None) => return Err(format_err!("no available PHYs can support AP ifaces")),
                    phy_manager_error => {
                        return Err(format_err!("could not get AP {:?}", phy_manager_error));
                    }
                }
            }
        };

        // Check if this iface ID is already accounted for.
        if let Some(removal_index) =
            self.aps.iter().position(|ap_container| ap_container.iface_id == iface_id)
        {
            return Ok(self.aps.remove(removal_index));
        }

        // If this iface ID is not yet accounted for, create a new ApIfaceContainer.
        let (sme_proxy, sme_server) = create_proxy();
        self.dev_monitor_proxy
            .get_ap_sme(iface_id, sme_server)
            .await?
            .map_err(zx::Status::err_from_raw)?;
        let sme_proxy = SmeForApStateMachine::new(sme_proxy, iface_id, self.defect_sender.clone());

        // Spawn the AP state machine.
        let (sender, receiver) = mpsc::channel(1);
        let state_machine = ap_fsm::AccessPoint::new(sender);
        let (publisher, status) = status_publisher_and_reader::<ap_fsm::Status>();

        let event_stream = sme_proxy.take_event_stream();
        let termination_event = Event::new();
        let termination_notifier = termination_event.clone();
        let state_machine_fut = ap_fsm::serve(
            iface_id,
            sme_proxy,
            event_stream,
            receiver.fuse(),
            self.ap_update_sender.clone(),
            self.telemetry_sender.clone(),
            self.defect_sender.clone(),
            publisher,
        )
        .then(|()| async move {
            if !termination_notifier.signal() {
                warn!("AP state machine notifier was signaled prematurely.")
            }
        })
        .boxed_local();

        // Begin running and monitoring the AP state machine future.
        let metadata =
            StateMachineMetadata { iface_id, role: fidl_fuchsia_wlan_common::WlanMacRole::Ap };
        let termination_fut = future_with_metadata::FutureWithMetadata::new(
            metadata,
            async move {
                // Wait for the state machine to either exit gracefully and signal the event or for the
                // event to be dropped due to a task cancellation.
                let _ = termination_event.wait_or_dropped().await;
            }
            .boxed_local(),
        );
        self.fsm_termination_futures.push(termination_fut);
        let state_machine_task = fasync::Task::local(state_machine_fut);

        Ok(ApIfaceContainer {
            iface_id,
            config: None,
            ap_state_machine: Box::new(state_machine),
            state_machine_task,
            enabled_time: None,
            status,
        })
    }

    /// Attempts to stop the AP and then exit the AP state machine.
    async fn stop_and_exit_ap_state_machine(
        mut ap_state_machine: Box<dyn AccessPointApi>,
    ) -> Result<(), Error> {
        let (sender, receiver) = oneshot::channel();
        ap_state_machine.stop(sender)?;
        receiver.await?;

        let (sender, receiver) = oneshot::channel();
        ap_state_machine.exit(sender)?;
        receiver.await?;

        Ok(())
    }

    #[allow(clippy::needless_return, reason = "mass allow for https://fxbug.dev/381896734")]
    fn disconnect(
        &mut self,
        network_id: ap_types::NetworkIdentifier,
        reason: client_types::DisconnectReason,
    ) -> LocalBoxFuture<'static, Result<(), Error>> {
        // Cancel any ongoing connection selections for the given network id and any autoconnect
        // selections. Any targeted connection selections for different networks are unaffected.
        self.connection_selection_manager
            .cancel(&SelectionIdentifier::ConnectRequest(network_id.clone()));
        self.connection_selection_manager.cancel(&SelectionIdentifier::Automatic);

        // Find the client interface associated with the given network config and disconnect from
        // the network.
        let mut fsm_ack_receiver = None;
        let mut iface_id = None;

        // Check if the disconnect affects any clients.
        for client in self.clients.iter_mut() {
            match &client.config {
                // If a client is configured for the specified network, tell the state machine to
                // disconnect.  This will cause the state machine's future to exit so that the monitoring
                // loop discovers the completed future and attempts to reconnect the interface.
                ClientIfaceContainerConfig::Configured(id) if id == &network_id => {
                    client.config = ClientIfaceContainerConfig::Unconfigured;
                    let (responder, receiver) = oneshot::channel();
                    match client.client_state_machine.as_mut() {
                        Some(state_machine) => match state_machine.disconnect(reason, responder) {
                            Ok(()) => {}
                            Err(e) => {
                                client.client_state_machine = None;
                                return ready(Err(format_err!(
                                    "failed to send disconnect: {:?}",
                                    e
                                )))
                                .boxed();
                            }
                        },
                        None => {
                            // If there is a Configured client for this network with no client state
                            // machine, it means this disconnect came before a state machine could
                            // be set up fully (e.g. if connection selection was still ongoing). In
                            // this case, there will be a lingering `connecting` client state update
                            // for the given network that we must handle here, rather than in a
                            // client state machine. Note that automatic connection selections do
                            // _not_ send an initial 'Connecting' state update, and thus aren't
                            // handled here.
                            let networks = vec![listener::ClientNetworkState {
                                id: network_id,
                                state: client_types::ConnectionState::Disconnected,
                                status: Some(client_types::DisconnectStatus::ConnectionStopped),
                            }];
                            let update = listener::ClientStateUpdate {
                                state: client_types::ClientState::ConnectionsEnabled,
                                networks,
                            };
                            match self
                                .client_update_sender
                                .clone()
                                .unbounded_send(listener::Message::NotifyListeners(update))
                            {
                                Ok(_) => (),
                                Err(e) => error!("failed to send state update: {:?}", e),
                            };
                            return ready(Ok(())).boxed();
                        }
                    }
                    client.client_state_machine = None;
                    fsm_ack_receiver = Some(receiver);
                    iface_id = Some(client.iface_id);
                    break;
                }
                _ => {}
            }
        }

        let receiver = match fsm_ack_receiver {
            Some(receiver) => receiver,
            None => return ready(Ok(())).boxed(),
        };
        let iface_id = match iface_id {
            Some(iface_id) => iface_id,
            None => return ready(Ok(())).boxed(),
        };

        let fut = async move {
            match receiver.await {
                Ok(()) => Ok(()),
                error => {
                    Err(format_err!("failed to disconnect client iface {}: {:?}", iface_id, error))
                }
            }
        };
        return fut.boxed_local();
    }

    async fn handle_connect_request(
        &mut self,
        connect_request: ConnectAttemptRequest,
    ) -> Result<(), Error> {
        // Check to see if client connections are enabled.
        {
            let phy_manager = self.phy_manager.lock().await;
            if !phy_manager.client_connections_enabled() {
                return Err(format_err!("client connections are not enabled"));
            }
        }

        // Cancel any ongoing connection attempts for OTHER networks.
        for id in self.connection_selection_manager.active_selections() {
            if id != SelectionIdentifier::ConnectRequest(connect_request.network.clone()) {
                self.connection_selection_manager.cancel(&id);
            }
        }

        // Check if we are already connecting to THIS network.
        let selection_id = SelectionIdentifier::ConnectRequest(connect_request.network.clone());
        if self.connection_selection_manager.active_selections().contains(&selection_id) {
            info!("{} is already in progress", selection_id);
            return Ok(());
        }

        // Check if connect request has already been fulfilled.
        for client in self.clients.iter() {
            match &client.config {
                ClientIfaceContainerConfig::Configured(config)
                    if config == &connect_request.network =>
                {
                    info!("Received connect request to already connected network");
                    return Ok(());
                }
                _ => {}
            }
        }

        let mut networks = vec![];

        // Any clients that had been configured but don't have state machines are pending connect
        // requests that we just cancelled. We have to add a "Disconnected" listener update for
        // them and mark as Unconfigured.
        for client in self.clients.iter_mut() {
            if let ClientIfaceContainerConfig::Configured(id) = &client.config
                && client.client_state_machine.as_mut().is_none()
            {
                networks.push(listener::ClientNetworkState {
                    id: id.clone(),
                    state: client_types::ConnectionState::Disconnected,
                    status: Some(client_types::DisconnectStatus::ConnectionStopped),
                });
                client.config = ClientIfaceContainerConfig::Unconfigured;
            }
        }

        // Get a client, and mark its config as Configured, reserving it for this connect
        // attempt
        let mut client_iface = self.get_client(None).await?;
        client_iface.config =
            ClientIfaceContainerConfig::Configured(connect_request.network.clone());
        self.clients.push(client_iface);

        self.telemetry_sender
            .send(TelemetryEvent::StartEstablishConnection { reset_start_time: true });

        // Add listener update that connection attempt is starting.
        networks.push(listener::ClientNetworkState {
            id: connect_request.network.clone(),
            state: client_types::ConnectionState::Connecting,
            status: None,
        });
        if let Err(e) = self.client_update_sender.clone().unbounded_send(
            listener::Message::NotifyListeners(listener::ClientStateUpdate {
                state: client_types::ClientState::ConnectionsEnabled,
                networks,
            }),
        ) {
            error!("failed to send state update: {:?}", e);
        }
        self.connection_selection_manager
            .initiate_connection_selection_for_connect_request(connect_request);
        Ok(())
    }

    async fn connect(&mut self, selection: client_types::ConnectSelection) -> Result<(), Error> {
        // Get a client iface, preferring one that was reserved for this connect attempt, if
        // applicable.
        let mut client_iface = match self.get_configured_client(selection.target.network.clone()) {
            Ok(container) => container,
            Err(_) => {
                let mut client = self.get_client(None).await?;
                client.config =
                    ClientIfaceContainerConfig::Configured(selection.target.network.clone());
                client
            }
        };

        // Check if there's an existing state machine we can use
        match client_iface.client_state_machine.as_mut() {
            Some(existing_csm) => {
                existing_csm.connect(selection)?;
            }
            None => {
                // Create the state machine and controller.
                let (new_client, state_machine_task, fut, status) = create_client_state_machine(
                    client_iface.iface_id,
                    &mut self.dev_monitor_proxy,
                    self.client_update_sender.clone(),
                    self.saved_networks.clone(),
                    Some(selection),
                    self.telemetry_sender.clone(),
                    self.defect_sender.clone(),
                    self.roam_manager.clone(),
                )
                .await?;
                client_iface.status = status;
                client_iface.client_state_machine = Some(new_client);
                client_iface.state_machine_task = Some(state_machine_task);

                // Begin running and monitoring the client state machine future.
                self.fsm_termination_futures.push(fut);
            }
        }

        client_iface.last_roam_time = fasync::MonotonicInstant::now();
        self.clients.push(client_iface);
        Ok(())
    }

    fn record_idle_client(&mut self, iface_id: u16) {
        for client in self.clients.iter_mut() {
            if client.iface_id == iface_id {
                // Check if the state machine has exited.  If it has not, then another call to
                // connect has replaced the state machine already and this interface should be left
                // alone.
                if let Some(state_machine) = client.client_state_machine.as_ref()
                    && state_machine.is_alive()
                {
                    return;
                }
                client.config = ClientIfaceContainerConfig::Unconfigured;
                client.client_state_machine = None;
                client.state_machine_task = None;
                return;
            }
        }
    }

    fn idle_clients(&self) -> Vec<u16> {
        let mut idle_clients = Vec::new();

        for client in self.clients.iter() {
            if client.config == ClientIfaceContainerConfig::Unconfigured {
                idle_clients.push(client.iface_id);
                continue;
            }

            match client.client_state_machine.as_ref() {
                Some(state_machine) => {
                    if !state_machine.is_alive() {
                        idle_clients.push(client.iface_id);
                    }
                }
                None => idle_clients.push(client.iface_id),
            }
        }

        idle_clients
    }

    /// Checks the specified interface to see if there is an active state machine for it.  If there
    /// is, this indicates that a connect request has already reconnected this interface and no
    /// further action is required.  If no state machine exists for the interface, attempts to
    /// connect the interface to the specified network.
    async fn attempt_client_reconnect(
        &mut self,
        iface_id: u16,
        connect_selection: client_types::ConnectSelection,
    ) -> Result<(), Error> {
        for client in self.clients.iter_mut() {
            if client.iface_id == iface_id {
                match client.client_state_machine.as_ref() {
                    Some(state_machine) if state_machine.is_alive() => {
                        return Ok(());
                    }
                    Some(_) | None => {}
                }

                // Create the state machine and controller.
                let (new_client, state_machine_task, termination_fut, status) =
                    create_client_state_machine(
                        client.iface_id,
                        &mut self.dev_monitor_proxy,
                        self.client_update_sender.clone(),
                        self.saved_networks.clone(),
                        Some(connect_selection.clone()),
                        self.telemetry_sender.clone(),
                        self.defect_sender.clone(),
                        self.roam_manager.clone(),
                    )
                    .await?;

                self.fsm_termination_futures.push(termination_fut);
                client.config =
                    ClientIfaceContainerConfig::Configured(connect_selection.target.network);
                client.status = status;
                client.client_state_machine = Some(new_client);
                client.state_machine_task = Some(state_machine_task);
                client.last_roam_time = fasync::MonotonicInstant::now();
                break;
            }
        }

        Ok(())
    }

    async fn handle_added_iface(&mut self, iface_id: u16) -> Result<(), Error> {
        // Ensure that PhyManager is aware of the new interface.
        {
            let mut phy_manager = self.phy_manager.lock().await;
            phy_manager.on_iface_added(iface_id).await?;
        }

        self.configure_new_iface(iface_id).await
    }

    async fn configure_new_iface(&mut self, iface_id: u16) -> Result<(), Error> {
        let iface_info = self
            .dev_monitor_proxy
            .query_iface(iface_id)
            .await?
            .map_err(zx::Status::err_from_raw)?;

        match iface_info.role {
            fidl_fuchsia_wlan_common::WlanMacRole::Client => {
                let mut client_iface = self.get_client(Some(iface_id)).await?;

                // If this client has already been recorded and it has a client state machine
                // running, return success early.
                if client_iface.client_state_machine.is_some() {
                    self.clients.push(client_iface);
                    return Ok(());
                }

                // Create the state machine and controller.  The state machine is setup with no
                // initial network config.  This will cause it to quickly exit, notifying the
                // monitor loop that the interface needs attention.
                let (new_client, state_machine_task, termination_fut, status) =
                    create_client_state_machine(
                        client_iface.iface_id,
                        &mut self.dev_monitor_proxy,
                        self.client_update_sender.clone(),
                        self.saved_networks.clone(),
                        None,
                        self.telemetry_sender.clone(),
                        self.defect_sender.clone(),
                        self.roam_manager.clone(),
                    )
                    .await?;

                // Begin running and monitoring the client state machine future.
                self.fsm_termination_futures.push(termination_fut);

                client_iface.status = status;
                client_iface.client_state_machine = Some(new_client);
                client_iface.state_machine_task = Some(state_machine_task);
                self.clients.push(client_iface);
            }
            fidl_fuchsia_wlan_common::WlanMacRole::Ap => {
                let ap_iface = self.get_ap(Some(iface_id)).await?;
                self.aps.push(ap_iface);
            }
            fidl_fuchsia_wlan_common::WlanMacRole::Mesh => {
                // Mesh roles are not currently supported.
            }
            fidl_fuchsia_wlan_common::WlanMacRoleUnknown!() => {
                error!("Unknown WlanMacRole type {:?} on iface {}", iface_info.role, iface_id);
            }
        }

        Ok(())
    }

    async fn handle_removed_iface(&mut self, iface_id: u16) {
        // Delete the reference from the PhyManager.
        self.phy_manager.lock().await.on_iface_removed(iface_id);

        // If the interface was deleted, but IfaceManager still has a reference to it, then the
        // driver has likely performed some low-level recovery or the interface driver has crashed.
        // In this case, remove the old reference to the interface ID and then create a new
        // interface of the appropriate type.
        if let Some(iface_index) =
            self.clients.iter().position(|client_container| client_container.iface_id == iface_id)
        {
            warn!("client interface {} was unxpectedly removed, attempting to recover", iface_id);
            let _ = self.clients.remove(iface_index);
            let client_iface_ids = self
                .phy_manager
                .lock()
                .await
                .create_all_client_ifaces(CreateClientIfacesReason::RecoverClientIfaces)
                .await;

            let mut failures = Vec::new();
            let mut recreated_client_iface_ids = Vec::new();

            for (phy_id, result) in client_iface_ids {
                match result {
                    Ok(ids) => recreated_client_iface_ids.extend_from_slice(&ids),
                    Err(e) => failures.push((phy_id, e)),
                }
            }

            if !failures.is_empty() {
                warn!("failed to recover some client interfaces: {:?}", failures);
            }

            for iface_id in recreated_client_iface_ids {
                match self.get_client(Some(iface_id)).await {
                    Ok(iface) => self.clients.push(iface),
                    Err(e) => {
                        error!("failed to recreate client {}: {:?}", iface_id, e);
                    }
                };
            }
        }

        // Check to see if there are any remaining client interfaces.  If there are not any, send
        // listeners a notification indicating that client connections are disabled.
        if self.clients.is_empty() {
            let update = listener::ClientStateUpdate {
                state: fidl_fuchsia_wlan_policy::WlanClientState::ConnectionsDisabled,
                networks: vec![],
            };
            if let Err(e) =
                self.client_update_sender.unbounded_send(listener::Message::NotifyListeners(update))
            {
                error!("Failed to notify listeners of lack of client interfaces: {:?}", e)
            };
        }

        // While client behavior is automated based on saved network configs and available SSIDs
        // observed in scan results, the AP behavior is largely controlled by API clients.  The AP
        // state machine will send back a failure notification in the event that the interface was
        // destroyed unexpectedly.  For the AP, simply remove the reference to the interface.  If
        // the API client so desires, they may ask the policy layer to start another AP interface.
        self.aps.retain(|ap_container| ap_container.iface_id != iface_id);
    }

    async fn get_sme_proxy_for_scan(&mut self) -> Result<SmeForScan, Error> {
        let client_iface = self.get_client(None).await?;
        let proxy = client_iface.sme_proxy.sme_for_scan();
        self.clients.push(client_iface);
        Ok(proxy)
    }

    fn stop_client_connections(
        &mut self,
        reason: client_types::DisconnectReason,
    ) -> LocalBoxFuture<'static, Result<(), Error>> {
        self.telemetry_sender.send(TelemetryEvent::ClearEstablishConnectionStartTime);

        // Cancel any ongoing network selection, since a disconnect makes it invalid.
        if !self.connection_selection_manager.active_selections().is_empty() {
            info!(
                "Client connections stopping, ignoring results from ongoing connection selections."
            );
            self.connection_selection_manager.cancel_all();
        }

        let client_ifaces: Vec<ClientIfaceContainer> = std::mem::take(&mut self.clients);
        let phy_manager = self.phy_manager.clone();
        let update_sender = self.client_update_sender.clone();

        let fut = async move {
            // Disconnect and discard all of the configured client ifaces.
            let mut cancelled_connection_networks = vec![];
            for mut client_iface in client_ifaces {
                if let ClientIfaceContainerConfig::Configured(network_id) = client_iface.config {
                    client_iface.config = ClientIfaceContainerConfig::Unconfigured;
                    match client_iface.client_state_machine.as_mut() {
                        Some(state_machine) => {
                            let (responder, receiver) = oneshot::channel();
                            match state_machine.disconnect(reason, responder) {
                                Ok(()) => {}
                                Err(e) => error!("failed to issue disconnect: {:?}", e),
                            }
                            match receiver.await {
                                Ok(()) => {}
                                Err(e) => error!("failed to disconnect: {:?}", e),
                            }
                        }
                        None => {
                            // Any pending connections have been cancelled. Send a listener update
                            // to marked them as Disconnected.
                            cancelled_connection_networks.push(listener::ClientNetworkState {
                                id: network_id,
                                state: client_types::ConnectionState::Disconnected,
                                status: Some(client_types::DisconnectStatus::ConnectionStopped),
                            });
                        }
                    }
                }
            }

            // Signal to the update listener that client connections have been disabled.
            let update = listener::ClientStateUpdate {
                state: fidl_fuchsia_wlan_policy::WlanClientState::ConnectionsDisabled,
                networks: cancelled_connection_networks,
            };
            if let Err(e) = update_sender.unbounded_send(listener::Message::NotifyListeners(update))
            {
                error!("Failed to send state update: {:?}", e)
            };

            // Tell the PhyManager to stop all client connections.
            let mut phy_manager = phy_manager.lock().await;
            phy_manager.destroy_all_client_ifaces().await?;

            Ok(())
        };

        fut.boxed_local()
    }

    async fn start_client_connections(&mut self) -> Result<(), Error> {
        let mut phy_manager = self.phy_manager.lock().await;
        let client_iface_ids = phy_manager
            .create_all_client_ifaces(CreateClientIfacesReason::StartClientConnections)
            .await;
        if client_iface_ids.values().any(Result::is_err) {
            return Err(format_err!(
                "could not start client connection: {:?}",
                client_iface_ids.into_iter().collect::<Vec<_>>()
            ));
        }
        let client_iface_ids = client_iface_ids.into_values().flat_map(Result::unwrap);

        // Resume client interfaces.
        drop(phy_manager);
        for iface_id in client_iface_ids {
            if let Err(e) = self.configure_new_iface(iface_id).await {
                error!("failed to resume client {}: {:?}", iface_id, e);
            };
        }

        Ok(())
    }

    async fn start_ap(&mut self, config: ap_fsm::ApConfig) -> Result<oneshot::Receiver<()>, Error> {
        let mut ap_iface_container = self.get_ap(None).await?;

        let (sender, receiver) = oneshot::channel();
        ap_iface_container.config = Some(config.clone());
        match ap_iface_container.ap_state_machine.start(config, sender) {
            Ok(()) => {
                if ap_iface_container.enabled_time.is_none() {
                    ap_iface_container.enabled_time = Some(zx::MonotonicInstant::get());
                }

                self.aps.push(ap_iface_container)
            }
            Err(e) => {
                let mut phy_manager = self.phy_manager.lock().await;
                phy_manager.destroy_ap_iface(ap_iface_container.iface_id).await?;
                return Err(format_err!("could not start ap: {}", e));
            }
        }

        Ok(receiver)
    }

    #[allow(clippy::needless_return, reason = "mass allow for https://fxbug.dev/381896734")]
    fn stop_ap(
        &mut self,
        ssid: ap_types::Ssid,
        credential: Vec<u8>,
    ) -> LocalBoxFuture<'static, Result<(), Error>> {
        if let Some(removal_index) =
            self.aps.iter().position(|ap_container| match ap_container.config.as_ref() {
                Some(config) => config.id.ssid == ssid && config.credential == credential,
                None => false,
            })
        {
            let phy_manager = self.phy_manager.clone();
            let mut ap_container = self.aps.remove(removal_index);

            if let Some(start_time) = ap_container.enabled_time.take() {
                let enabled_duration = zx::MonotonicInstant::get() - start_time;
                self.telemetry_sender.send(TelemetryEvent::StopAp { enabled_duration });
            }

            let fut = async move {
                let _ = &ap_container;
                let stop_result =
                    Self::stop_and_exit_ap_state_machine(ap_container.ap_state_machine).await;

                let mut phy_manager = phy_manager.lock().await;
                phy_manager.destroy_ap_iface(ap_container.iface_id).await?;

                stop_result?;
                Ok(())
            };

            return fut.boxed_local();
        }

        return ready(Ok(())).boxed();
    }

    // Stop all APs, exit all of the state machines, and destroy all AP ifaces.
    fn stop_all_aps(&mut self) -> LocalBoxFuture<'static, Result<(), Error>> {
        let mut aps: Vec<ApIfaceContainer> = std::mem::take(&mut self.aps);
        let phy_manager = self.phy_manager.clone();

        for ap_container in aps.iter_mut() {
            if let Some(start_time) = ap_container.enabled_time.take() {
                let enabled_duration = zx::MonotonicInstant::get() - start_time;
                self.telemetry_sender.send(TelemetryEvent::StopAp { enabled_duration });
            }
        }

        let fut = async move {
            let mut failed_iface_deletions: u8 = 0;
            for iface in aps.drain(..) {
                match IfaceManagerService::stop_and_exit_ap_state_machine(iface.ap_state_machine)
                    .await
                {
                    Ok(()) => {}
                    Err(e) => {
                        failed_iface_deletions += 1;
                        error!("failed to stop AP: {}", e);
                    }
                }

                let mut phy_manager = phy_manager.lock().await;
                match phy_manager.destroy_ap_iface(iface.iface_id).await {
                    Ok(()) => {}
                    Err(e) => {
                        failed_iface_deletions += 1;
                        error!("failed to delete AP {}: {}", iface.iface_id, e);
                    }
                }
            }

            if failed_iface_deletions == 0 {
                Ok(())
            } else {
                Err(format_err!("failed to delete {} ifaces", failed_iface_deletions))
            }
        };

        fut.boxed_local()
    }
}

/// Queues a connection selection future for idle interface autoconnect.
async fn initiate_automatic_connection_selection(iface_manager: &mut IfaceManagerService) {
    if !iface_manager.idle_clients().is_empty()
        && iface_manager.saved_networks.known_network_count().await > 0
        && iface_manager.connection_selection_manager.active_selections().is_empty()
    {
        iface_manager
            .telemetry_sender
            .send(TelemetryEvent::StartEstablishConnection { reset_start_time: false });
        info!("Initiating network selection for idle client interface.");

        iface_manager.connection_selection_manager.initiate_automatic_connection_selection();
    }
}

/// Handles results of idle interface autoconnect connection selection, including attempting to
/// connect on the idle interface.
async fn handle_automatic_connection_selection_results(
    connection_selection_result: Option<client_types::ScannedCandidate>,
    iface_manager: &mut IfaceManagerService,
    reconnect_monitor_interval: &mut i64,
    connectivity_monitor_timer: &mut fasync::Interval,
) {
    if let Some(scanned_candidate) = connection_selection_result {
        *reconnect_monitor_interval = 1;

        let connect_selection = client_types::ConnectSelection {
            target: scanned_candidate,
            reason: client_types::ConnectReason::IdleInterfaceAutoconnect,
        };

        let mut idle_clients = iface_manager.idle_clients();
        if !idle_clients.is_empty() {
            // Any client interfaces that have recently presented as idle will be
            // reconnected.
            for iface_id in idle_clients.drain(..) {
                if let Err(e) = iface_manager
                    .attempt_client_reconnect(iface_id, connect_selection.clone())
                    .await
                {
                    warn!("Could not reconnect iface {}: {:?}", iface_id, e);
                }
            }
        }
    } else {
        *reconnect_monitor_interval =
            (2 * (*reconnect_monitor_interval)).min(MAX_AUTO_CONNECT_RETRY_SECONDS);
    }

    *connectivity_monitor_timer =
        fasync::Interval::new(zx::MonotonicDuration::from_seconds(*reconnect_monitor_interval));
}

/// Handles results of a connect request connection selection, including attempting to connect and
/// managing retry attempts.
async fn handle_connection_selection_for_connect_request_results(
    mut request: ConnectAttemptRequest,
    result: Option<client_types::ScannedCandidate>,
    iface_manager: &mut IfaceManagerService,
) {
    request.attempts += 1;
    match result {
        Some(scanned_candidate) => {
            let selection = client_types::ConnectSelection {
                target: scanned_candidate,
                reason: request.reason,
            };
            info!("Starting connection to {}", selection.target.bss.bssid,);
            let _ = iface_manager.connect(selection).await;
        }
        None => {
            if request.attempts < 3 {
                debug!("No candidates found for connect request, queueing retrying.");
                iface_manager
                    .connection_selection_manager
                    .initiate_connection_selection_for_connect_request(request);
            } else {
                info!("Failed to find a candidate for the connect request");
                // Send connection failed update.
                let networks = vec![listener::ClientNetworkState {
                    id: request.network.clone(),
                    state: client_types::ConnectionState::Failed,
                    status: Some(client_types::DisconnectStatus::ConnectionFailed),
                }];

                let update = listener::ClientStateUpdate {
                    state: client_types::ClientState::ConnectionsEnabled,
                    networks,
                };
                match iface_manager
                    .client_update_sender
                    .clone()
                    .unbounded_send(listener::Message::NotifyListeners(update))
                {
                    Ok(_) => (),
                    Err(e) => error!("failed to send connection_failed state update: {:?}", e),
                };
            }
        }
    }
}

async fn handle_terminated_state_machine(
    terminated_fsm: StateMachineMetadata,
    iface_manager: &mut IfaceManagerService,
) {
    match terminated_fsm.role {
        fidl_fuchsia_wlan_common::WlanMacRole::Ap => {
            // If the state machine exited normally, the IfaceManagerService will have already
            // destroyed the interface.  If not, then the state machine exited because it could not
            // communicate with the SME and the interface is likely unusable.
            let mut phy_manager = iface_manager.phy_manager.lock().await;
            if phy_manager.destroy_ap_iface(terminated_fsm.iface_id).await.is_err() {}
        }
        fidl_fuchsia_wlan_common::WlanMacRole::Client => {
            iface_manager.record_idle_client(terminated_fsm.iface_id);
            initiate_automatic_connection_selection(iface_manager).await;
        }
        fidl_fuchsia_wlan_common::WlanMacRole::Mesh => {
            // Not yet supported.
            panic!("Mesh support unimplemented");
        }
        fidl_fuchsia_wlan_common::WlanMacRoleUnknown!() => {
            panic!("State machine terminated with a WlanMacRoleUnknown interface");
        }
    }
}

fn initiate_set_country(
    iface_manager: &mut IfaceManagerService,
    req: SetCountryRequest,
) -> LocalBoxFuture<'static, SetCountryOperationState> {
    // Store the initial AP configs so that they can be started later.
    let initial_ap_configs =
        iface_manager.aps.iter().filter_map(|container| container.config.clone()).collect();

    // Create futures to stop all of the APs and stop client connections
    let stop_client_connections_fut = iface_manager
        .stop_client_connections(client_types::DisconnectReason::RegulatoryRegionChange);
    let stop_aps_fut = iface_manager.stop_all_aps();

    // Once the clients and APs have been stopped, set the country code.
    let phy_manager = iface_manager.phy_manager.clone();
    let regulatory_fut = async move {
        let client_connections_initially_enabled =
            phy_manager.lock().await.client_connections_enabled();

        let set_country_result = if let Err(e) = stop_client_connections_fut.await {
            Err(format_err!(
                "failed to stop client connection in preparation for setting country code: {:?}",
                e
            ))
        } else if let Err(e) = stop_aps_fut.await {
            Err(format_err!("failed to stop APs in preparation for setting country code: {:?}", e))
        } else {
            let mut phy_manager = phy_manager.lock().await;
            phy_manager
                .set_country_code(req.country_code)
                .await
                .map_err(|e| format_err!("failed to set regulatory region: {:?}", e))
        };

        SetCountryOperationState {
            client_connections_initially_enabled,
            initial_ap_configs,
            set_country_result,
            responder: req.responder,
        }
    };
    regulatory_fut.boxed_local()
}

async fn restore_state_after_setting_country_code(
    iface_manager: &mut IfaceManagerService,
    previous_state: SetCountryOperationState,
) {
    // Prior to setting the country code, it is essential that client connections and APs are all
    // stopped.  If stopping clients or APs fails or if setting the country code fails, the whole
    // process of setting the country code must be considered a failure.
    //
    // Bringing clients and APs back online following the regulatory region setting may fail and is
    // possibly recoverable.  Log failures, but do not report errors in scenarios where recreating
    // the client and AP interfaces fails.  This allows API clients to retry and attempt to create
    // the interfaces themselves by making policy API requests.
    if previous_state.client_connections_initially_enabled
        && let Err(e) = iface_manager.start_client_connections().await
    {
        error!("failed to resume client connections after setting country code: {:?}", e);
    }

    for config in previous_state.initial_ap_configs {
        if let Err(e) = iface_manager.start_ap(config).await {
            error!("failed to resume AP after setting country code: {:?}", e);
        }
    }

    if previous_state.responder.send(previous_state.set_country_result).is_err() {
        error!("could not respond to SetCountryRequest");
    }
}

// This function allows the defect recording to run in parallel with the regulatory region setting
// routine.  For full context, see https://fxbug.dev/42063961.
fn initiate_record_defect(
    phy_manager: Arc<Mutex<dyn PhyManagerApi>>,
    defect: Defect,
) -> LocalBoxFuture<'static, ()> {
    let fut = async move {
        let mut phy_manager = phy_manager.lock().await;
        phy_manager.record_defect(defect);
    };
    fut.boxed_local()
}

fn initiate_recovery(
    phy_manager: Arc<Mutex<dyn PhyManagerApi>>,
    summary: recovery::RecoverySummary,
) -> LocalBoxFuture<'static, ()> {
    let fut = async move {
        let mut phy_manager = phy_manager.lock().await;
        phy_manager.perform_recovery(summary).await;
    };
    fut.boxed_local()
}

async fn handle_iface_manager_request(
    iface_manager: &mut IfaceManagerService,
    request: IfaceManagerRequest,
) {
    match request {
        IfaceManagerRequest::Connect(ConnectRequest { request, responder }) => {
            if responder.send(iface_manager.handle_connect_request(request).await).is_err() {
                error!("could not respond to ScanForConnectionSelection");
            }
        }
        IfaceManagerRequest::RecordIdleIface(RecordIdleIfaceRequest { iface_id, responder }) => {
            iface_manager.record_idle_client(iface_id);
            if responder.send(()).is_err() {
                error!("could not respond to RecordIdleIfaceRequest");
            }
        }
        IfaceManagerRequest::HasIdleIface(HasIdleIfaceRequest { responder }) => {
            if responder.send(!iface_manager.idle_clients().is_empty()).is_err() {
                error!("could not respond to  HasIdleIfaceRequest");
            }
        }
        IfaceManagerRequest::AddIface(AddIfaceRequest { iface_id, responder }) => {
            if let Err(e) = iface_manager.handle_added_iface(iface_id).await {
                warn!("failed to add new interface {}: {:?}", iface_id, e);
            }
            if responder.send(()).is_err() {
                error!("could not respond to AddIfaceRequest");
            }
        }
        IfaceManagerRequest::RemoveIface(RemoveIfaceRequest { iface_id, responder }) => {
            iface_manager.handle_removed_iface(iface_id).await;
            if responder.send(()).is_err() {
                error!("could not respond to RemoveIfaceRequest");
            }
        }
        IfaceManagerRequest::GetScanProxy(ScanProxyRequest { responder }) => {
            if responder.send(iface_manager.get_sme_proxy_for_scan().await).is_err() {
                error!("could not respond to ScanRequest");
            }
        }
        IfaceManagerRequest::StartClientConnections(StartClientConnectionsRequest {
            responder,
        }) => {
            if responder.send(iface_manager.start_client_connections().await).is_err() {
                error!("could not respond to StartClientConnectionRequest");
            }
        }
        IfaceManagerRequest::StartAp(StartApRequest { config, responder }) => {
            if responder.send(iface_manager.start_ap(config).await).is_err() {
                error!("could not respond to StartApRequest");
            }
        }
        IfaceManagerRequest::Disconnect(DisconnectRequest { network_id, responder, reason }) => {
            if responder.send(iface_manager.disconnect(network_id, reason).await).is_err() {
                error!("could not respond to DisconnectRequest");
            }
        }
        IfaceManagerRequest::StopClientConnections(StopClientConnectionsRequest {
            reason,
            responder,
        }) => {
            if responder.send(iface_manager.stop_client_connections(reason).await).is_err() {
                error!("could not respond to StopClientConnectionsRequest");
            }
        }
        IfaceManagerRequest::StopAp(StopApRequest { ssid, password, responder }) => {
            if responder.send(iface_manager.stop_ap(ssid, password).await).is_err() {
                error!("could not respond to StopApRequest");
            }
        }
        IfaceManagerRequest::StopAllAps(StopAllApsRequest { responder }) => {
            if responder.send(iface_manager.stop_all_aps().await).is_err() {
                error!("could not respond to StopAllApsRequest");
            }
        }
        IfaceManagerRequest::SetCountry(req) => {
            let previous_state = initiate_set_country(iface_manager, req).await;
            restore_state_after_setting_country_code(iface_manager, previous_state).await;
        }
    };
}

async fn serve_iface_functionality(
    iface_manager: &mut IfaceManagerService,
    requests: &mut mpsc::Receiver<IfaceManagerRequest>,
    reconnect_monitor_interval: &mut i64,
    connectivity_monitor_timer: &mut fasync::Interval,
    defect_receiver: &mut mpsc::Receiver<Defect>,
    recovery_action_receiver: &mut recovery::RecoveryActionReceiver,
) {
    // Pollable wrapper for connection selection manager.
    let mut connection_selection_futures =
        ConnectionSelectionFutures::new(&mut iface_manager.connection_selection_manager);
    select! {
        req = requests.next() => {
            if let Some(req) = req {
                handle_iface_manager_request(
                    iface_manager,
                    req
                ).await
            }
        }
        terminated_fsm = iface_manager.fsm_termination_futures.select_next_some() => {
            info!("state machine exited: {:?}", terminated_fsm.1);
            handle_terminated_state_machine(
                terminated_fsm.1,
                iface_manager,
            ).await;
        },
        () = connectivity_monitor_timer.select_next_some() => {
            initiate_automatic_connection_selection(
                iface_manager,
            ).await;
        },
        connection_selection_result = connection_selection_futures => {
            let (selection_id, result) = connection_selection_result;
            match result {
                Ok(Some(ConnectionSelectionResponse::Autoconnect(candidate))) => {
                    info!("{} has completed.", selection_id);
                    handle_automatic_connection_selection_results(
                        candidate,
                        iface_manager,
                        reconnect_monitor_interval,
                        connectivity_monitor_timer
                    ).await;
                }
                Ok(Some(ConnectionSelectionResponse::ConnectRequest {candidate, request})) => {
                    info!("{} has completed.", selection_id);
                    handle_connection_selection_for_connect_request_results(
                        request,
                        candidate,
                        iface_manager,
                    ).await;
                }
                Ok(None) => {
                    debug!("{} cancellation has been processed by iface_manager", selection_id);
                }
                Err(e) => {
                    error!("{} failed with error: {}", selection_id, e);
                }
            }
        },
        defect = defect_receiver.select_next_some() => {
            initiate_record_defect(iface_manager.phy_manager.clone(), defect).await;
        },
        action = recovery_action_receiver.select_next_some() => {
            let client_statuses = InspectListClosure(&iface_manager.clients, |node_writer, key, client| {
                if let Ok(status) = client.status.read_status() {
                    inspect_insert!(node_writer, var key: {
                        id: client.iface_id,
                        status: status
                    });
                }
            });

            let ap_statuses = InspectListClosure(&iface_manager.aps, |node_writer, key, ap| {
                if let Ok(status) = ap.status.read_status() {
                    inspect_insert!(node_writer, var key: {
                        id: ap.iface_id,
                        status: status
                    });
                }
            });

            inspect_log!(iface_manager.recovery_node, {
                summary: action,
                clients: client_statuses,
                aps: ap_statuses
            });

            initiate_recovery(iface_manager.phy_manager.clone(), action).await;
        },
    }
}

pub(crate) async fn serve_iface_manager_requests(
    mut iface_manager: IfaceManagerService,
    mut requests: mpsc::Receiver<IfaceManagerRequest>,
    mut defect_receiver: mpsc::Receiver<Defect>,
    mut recovery_action_receiver: recovery::RecoveryActionReceiver,
) -> Result<Infallible, Error> {
    // Create a timer to periodically check to ensure that all client interfaces are connected.
    let mut reconnect_monitor_interval: i64 = 1;
    let mut connectivity_monitor_timer =
        fasync::Interval::new(zx::MonotonicDuration::from_seconds(reconnect_monitor_interval));

    loop {
        serve_iface_functionality(
            &mut iface_manager,
            &mut requests,
            &mut reconnect_monitor_interval,
            &mut connectivity_monitor_timer,
            &mut defect_receiver,
            &mut recovery_action_receiver,
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access_point::types;
    use crate::client::connection_selection::ConnectionSelectionRequest;
    use crate::client::types as client_types;
    use crate::config_management::{
        Credential, NetworkIdentifier, SavedNetworksManager, SecurityType,
    };
    use crate::mode_management::phy_manager::{self, PhyManagerError};
    use crate::mode_management::recovery::RecoverySummary;
    use crate::mode_management::{IfaceFailure, PhyFailure};
    use crate::util::testing::fakes::FakeScanRequester;
    use crate::util::testing::{
        generate_connect_selection, generate_random_scanned_candidate, poll_sme_req,
    };
    use assert_matches::assert_matches;
    use async_trait::async_trait;
    use diagnostics_assertions::assert_data_tree;
    use fuchsia_async::TestExecutor;
    use fuchsia_inspect as inspect;
    use fuchsia_inspect::reader;
    use futures::stream::StreamFuture;
    use futures::task::Poll;
    use ieee80211::MacAddr;
    use std::collections::HashMap;
    use std::pin::pin;
    use std::sync::LazyLock;
    use test_case::test_case;
    use wlan_common::RadioConfig;
    use wlan_common::channel::Bandwidth;

    // Responses that FakePhyManager will provide
    pub const TEST_CLIENT_IFACE_ID: u16 = 0;
    pub const TEST_AP_IFACE_ID: u16 = 1;

    // Fake WLAN network that tests will scan for and connect to.
    pub static TEST_SSID: LazyLock<ap_types::Ssid> =
        LazyLock::new(|| ap_types::Ssid::try_from("test_ssid").unwrap());
    pub static TEST_PASSWORD: &str = "test_password";

    /// Holds all of the boilerplate required for testing IfaceManager.
    /// * DeviceMonitorProxy and DeviceMonitorRequestStream
    ///   * Allow for the construction of Clients and ClientIfaceContainers and the ability to send
    ///     responses to their requests.
    /// * ClientListenerMessageSender and MessageStream
    ///   * Allow for the construction of ClientIfaceContainers and the absorption of
    ///     ClientStateUpdates.
    /// * ApListenerMessageSender and MessageStream
    ///   * Allow for the construction of ApIfaceContainers and the absorption of
    ///     ApStateUpdates.
    /// * KnownEssStore, SavedNetworksManager, TempDir
    ///   * Allow for the querying of network credentials and storage of connection history.
    pub struct TestValues {
        pub monitor_service_proxy: fidl_fuchsia_wlan_device_service::DeviceMonitorProxy,
        pub monitor_service_stream: fidl_fuchsia_wlan_device_service::DeviceMonitorRequestStream,
        pub client_update_sender: listener::ClientListenerMessageSender,
        pub client_update_receiver: mpsc::UnboundedReceiver<listener::ClientListenerMessage>,
        pub ap_update_sender: listener::ApListenerMessageSender,
        pub saved_networks: Arc<dyn SavedNetworksManagerApi>,
        pub scan_requester: Arc<FakeScanRequester>,
        pub inspector: inspect::Inspector,
        pub node: inspect::Node,
        pub telemetry_sender: TelemetrySender,
        pub telemetry_receiver: mpsc::Receiver<TelemetryEvent>,
        pub defect_sender: mpsc::Sender<Defect>,
        pub defect_receiver: mpsc::Receiver<Defect>,
        pub recovery_sender: recovery::RecoveryActionSender,
        pub recovery_receiver: recovery::RecoveryActionReceiver,
        pub connection_selection_requester: ConnectionSelectionRequester,
        pub connection_selection_request_receiver: mpsc::Receiver<ConnectionSelectionRequest>,
        pub roam_manager: RoamManager,
    }

    /// Create a TestValues for a unit test.
    pub fn test_setup(exec: &mut TestExecutor) -> TestValues {
        let (monitor_service_proxy, monitor_service_requests) =
            create_proxy::<fidl_fuchsia_wlan_device_service::DeviceMonitorMarker>();
        let monitor_service_stream = monitor_service_requests.into_stream();

        let (client_sender, client_receiver) = mpsc::unbounded();
        let (ap_sender, _) = mpsc::unbounded();

        let saved_networks = exec.run_singlethreaded(SavedNetworksManager::new_for_test());
        let saved_networks = Arc::new(saved_networks);
        let inspector = inspect::Inspector::default();
        let node = inspector.root().create_child("node");
        let (telemetry_sender, telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let telemetry_sender = TelemetrySender::new(telemetry_sender);
        let scan_requester = Arc::new(FakeScanRequester::new());
        let (defect_sender, defect_receiver) = mpsc::channel(100);
        let (recovery_sender, recovery_receiver) =
            mpsc::channel::<recovery::RecoverySummary>(recovery::RECOVERY_SUMMARY_CHANNEL_CAPACITY);
        let (connection_selection_request_sender, connection_selection_request_receiver) =
            mpsc::channel(5);
        let connection_selection_requester =
            ConnectionSelectionRequester::new(connection_selection_request_sender);

        let (roam_service_request_sender, _roam_service_request_receiver) = mpsc::channel(100);
        let roam_manager = RoamManager::new(roam_service_request_sender);

        TestValues {
            monitor_service_proxy,
            monitor_service_stream,
            client_update_sender: client_sender,
            client_update_receiver: client_receiver,
            ap_update_sender: ap_sender,
            saved_networks,
            scan_requester,
            inspector,
            node,
            telemetry_sender,
            telemetry_receiver,
            defect_sender,
            defect_receiver,
            recovery_sender,
            recovery_receiver,
            connection_selection_requester,
            connection_selection_request_receiver,
            roam_manager,
        }
    }

    /// Creates a new PhyManagerPtr for tests.
    fn create_empty_phy_manager(
        monitor_service: fidl_fuchsia_wlan_device_service::DeviceMonitorProxy,
        node: inspect::Node,
        telemetry_sender: TelemetrySender,
        recovery_sender: recovery::RecoveryActionSender,
    ) -> Arc<Mutex<dyn PhyManagerApi>> {
        Arc::new(Mutex::new(phy_manager::PhyManager::new(
            monitor_service,
            recovery::lookup_recovery_profile(""),
            false,
            node,
            telemetry_sender,
            recovery_sender,
        )))
    }

    #[derive(Debug)]
    struct FakePhyManager {
        create_iface_ok: bool,
        destroy_iface_ok: bool,
        set_country_ok: bool,
        country_code: Option<client_types::CountryCode>,
        client_connections_enabled: bool,
        client_ifaces: Vec<u16>,
        defects: Vec<Defect>,
        recovery_sender: Option<mpsc::Sender<(RecoverySummary, oneshot::Sender<()>)>>,
    }

    #[async_trait(?Send)]
    impl PhyManagerApi for FakePhyManager {
        async fn add_phy(&mut self, _phy_id: u16) -> Result<(), PhyManagerError> {
            unimplemented!()
        }

        fn remove_phy(&mut self, _phy_id: u16) {
            unimplemented!()
        }

        async fn on_iface_added(&mut self, iface_id: u16) -> Result<(), PhyManagerError> {
            self.client_ifaces.push(iface_id);
            Ok(())
        }

        fn on_iface_removed(&mut self, _iface_id: u16) {}

        async fn create_all_client_ifaces(
            &mut self,
            _reason: CreateClientIfacesReason,
        ) -> HashMap<u16, Result<Vec<u16>, PhyManagerError>> {
            if self.create_iface_ok {
                HashMap::from([(0, Ok(self.client_ifaces.clone()))])
            } else {
                HashMap::from([(0, Err(PhyManagerError::IfaceCreateFailure))])
            }
        }

        fn client_connections_enabled(&self) -> bool {
            self.client_connections_enabled
        }

        async fn destroy_all_client_ifaces(&mut self) -> Result<(), PhyManagerError> {
            if self.destroy_iface_ok { Ok(()) } else { Err(PhyManagerError::IfaceDestroyFailure) }
        }

        fn get_client(&mut self) -> Option<u16> {
            Some(TEST_CLIENT_IFACE_ID)
        }

        async fn create_or_get_ap_iface(&mut self) -> Result<Option<u16>, PhyManagerError> {
            if self.create_iface_ok {
                Ok(Some(TEST_AP_IFACE_ID))
            } else {
                Err(PhyManagerError::IfaceCreateFailure)
            }
        }

        async fn destroy_ap_iface(&mut self, _iface_id: u16) -> Result<(), PhyManagerError> {
            if self.destroy_iface_ok { Ok(()) } else { Err(PhyManagerError::IfaceDestroyFailure) }
        }

        async fn destroy_all_ap_ifaces(&mut self) -> Result<(), PhyManagerError> {
            if self.destroy_iface_ok { Ok(()) } else { Err(PhyManagerError::IfaceDestroyFailure) }
        }

        fn suggest_ap_mac(&mut self, _mac: MacAddr) {
            unimplemented!()
        }

        fn get_phy_ids(&self) -> Vec<u16> {
            unimplemented!()
        }

        fn log_phy_add_failure(&mut self) {
            unimplemented!();
        }

        async fn set_country_code(
            &mut self,
            country_code: Option<client_types::CountryCode>,
        ) -> Result<(), PhyManagerError> {
            if self.set_country_ok {
                self.country_code = country_code;
                Ok(())
            } else {
                Err(PhyManagerError::PhySetCountryFailure)
            }
        }

        fn record_defect(&mut self, defect: Defect) {
            self.defects.push(defect);
        }

        async fn perform_recovery(&mut self, summary: RecoverySummary) {
            if let Some(recovery_sender) = self.recovery_sender.as_mut() {
                let (sender, receiver) = oneshot::channel();
                recovery_sender
                    .try_send((summary, sender))
                    .expect("Failed to send recovery summary");
                receiver.await.expect("Failed waiting for recovery response");
            }
        }
    }

    struct FakeClient {
        disconnect_ok: bool,
        is_alive: bool,
        expected_connect_selection: Option<client_types::ConnectSelection>,
    }

    impl FakeClient {
        fn new() -> Self {
            FakeClient { disconnect_ok: true, is_alive: true, expected_connect_selection: None }
        }
    }

    #[async_trait(?Send)]
    impl client_fsm::ClientApi for FakeClient {
        fn connect(&mut self, selection: client_types::ConnectSelection) -> Result<(), Error> {
            assert_eq!(Some(selection), self.expected_connect_selection);
            Ok(())
        }
        fn disconnect(
            &mut self,
            _reason: client_types::DisconnectReason,
            responder: oneshot::Sender<()>,
        ) -> Result<(), Error> {
            if self.disconnect_ok {
                let _ = responder.send(());
                Ok(())
            } else {
                Err(format_err!("fake failed to disconnect"))
            }
        }
        fn is_alive(&self) -> bool {
            self.is_alive
        }
    }

    struct FakeAp {
        start_succeeds: bool,
        stop_succeeds: bool,
        exit_succeeds: bool,
    }

    #[async_trait(?Send)]
    impl AccessPointApi for FakeAp {
        fn start(
            &mut self,
            _request: ap_fsm::ApConfig,
            responder: oneshot::Sender<()>,
        ) -> Result<(), anyhow::Error> {
            if self.start_succeeds {
                let _ = responder.send(());
                Ok(())
            } else {
                Err(format_err!("start failed"))
            }
        }

        fn stop(&mut self, responder: oneshot::Sender<()>) -> Result<(), anyhow::Error> {
            if self.stop_succeeds {
                let _ = responder.send(());
                Ok(())
            } else {
                Err(format_err!("stop failed"))
            }
        }

        fn exit(&mut self, responder: oneshot::Sender<()>) -> Result<(), anyhow::Error> {
            if self.exit_succeeds {
                let _ = responder.send(());
                Ok(())
            } else {
                Err(format_err!("exit failed"))
            }
        }
    }

    fn create_iface_manager_with_client(
        test_values: &TestValues,
        configured: bool,
    ) -> (IfaceManagerService, StreamFuture<fidl_fuchsia_wlan_sme::ClientSmeRequestStream>) {
        let (sme_proxy, server) = create_proxy::<fidl_fuchsia_wlan_sme::ClientSmeMarker>();
        let sme_proxy = SmeForClientStateMachine::new(
            sme_proxy,
            TEST_CLIENT_IFACE_ID,
            test_values.defect_sender.clone(),
        );
        let (_publisher, status) = status_publisher_and_reader::<client_fsm::Status>();
        let mut client_container = ClientIfaceContainer {
            iface_id: TEST_CLIENT_IFACE_ID,
            sme_proxy,
            config: ClientIfaceContainerConfig::Unconfigured,
            client_state_machine: None,
            state_machine_task: None,
            last_roam_time: fasync::MonotonicInstant::now(),
            status,
        };
        let phy_manager = FakePhyManager {
            create_iface_ok: true,
            destroy_iface_ok: true,
            set_country_ok: true,
            country_code: None,
            client_connections_enabled: true,
            client_ifaces: vec![TEST_CLIENT_IFACE_ID],
            defects: vec![],
            recovery_sender: None,
        };
        let mut iface_manager = IfaceManagerService::new(
            Arc::new(Mutex::new(phy_manager)),
            test_values.client_update_sender.clone(),
            test_values.ap_update_sender.clone(),
            test_values.monitor_service_proxy.clone(),
            test_values.saved_networks.clone(),
            test_values.connection_selection_requester.clone(),
            test_values.roam_manager.clone(),
            test_values.telemetry_sender.clone(),
            test_values.defect_sender.clone(),
            test_values.node.clone_weak(),
        );

        if configured {
            client_container.config =
                ClientIfaceContainerConfig::Configured(ap_types::NetworkIdentifier {
                    ssid: TEST_SSID.clone(),
                    security_type: ap_types::SecurityType::Wpa,
                });
            client_container.client_state_machine = Some(Box::new(FakeClient::new()));
        }
        iface_manager.clients.push(client_container);

        (iface_manager, server.into_stream().into_future())
    }

    fn create_ap_config(ssid: &ap_types::Ssid, password: &str) -> ap_fsm::ApConfig {
        let radio_config = RadioConfig::new(
            fidl_fuchsia_wlan_ieee80211::WlanPhyType::Ht,
            Bandwidth::Cbw20,
            6,
            fidl_fuchsia_wlan_ieee80211::WlanBand::TwoGhz,
        );
        ap_fsm::ApConfig {
            id: ap_types::NetworkIdentifier {
                ssid: ssid.clone(),
                security_type: ap_types::SecurityType::None,
            },
            credential: password.as_bytes().to_vec(),
            radio_config,
            mode: types::ConnectivityMode::Unrestricted,
            band: types::OperatingBand::Any,
        }
    }

    fn create_iface_manager_with_ap(
        test_values: &TestValues,
        fake_ap: FakeAp,
    ) -> IfaceManagerService {
        let (_publisher, status) = status_publisher_and_reader::<ap_fsm::Status>();
        let ap_container = ApIfaceContainer {
            iface_id: TEST_AP_IFACE_ID,
            config: None,
            ap_state_machine: Box::new(fake_ap),
            state_machine_task: fasync::Task::local(async {}),
            enabled_time: None,
            status,
        };
        let phy_manager = FakePhyManager {
            create_iface_ok: true,
            destroy_iface_ok: true,
            set_country_ok: true,
            country_code: None,
            client_connections_enabled: true,
            client_ifaces: vec![],
            defects: vec![],
            recovery_sender: None,
        };
        let mut iface_manager = IfaceManagerService::new(
            Arc::new(Mutex::new(phy_manager)),
            test_values.client_update_sender.clone(),
            test_values.ap_update_sender.clone(),
            test_values.monitor_service_proxy.clone(),
            test_values.saved_networks.clone(),
            test_values.connection_selection_requester.clone(),
            test_values.roam_manager.clone(),
            test_values.telemetry_sender.clone(),
            test_values.defect_sender.clone(),
            test_values.node.clone_weak(),
        );

        iface_manager.aps.push(ap_container);
        iface_manager
    }

    #[track_caller]
    fn run_state_machine_futures(
        exec: &mut fuchsia_async::TestExecutor,
        iface_manager: &mut IfaceManagerService,
    ) {
        for mut state_machine in iface_manager.fsm_termination_futures.iter_mut() {
            assert_matches!(exec.run_until_stalled(&mut state_machine), Poll::Pending);
        }
    }

    /// Tests the case where connect is called and the only available client interface is already
    /// configured.
    #[fuchsia::test]
    fn test_connect_with_configured_iface() {
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create a configured ClientIfaceContainer.
        let test_values = test_setup(&mut exec);
        let (mut iface_manager, _) = create_iface_manager_with_client(&test_values, true);

        // Configure the mock CSM with the expected connect request
        let connect_selection = generate_connect_selection();
        iface_manager.clients[0].client_state_machine = Some(Box::new(FakeClient {
            disconnect_ok: false,
            is_alive: true,
            expected_connect_selection: Some(connect_selection.clone()),
        }));

        // Ask the IfaceManager to connect.
        {
            let connect_fut = iface_manager.connect(connect_selection.clone());
            let mut connect_fut = pin!(connect_fut);

            // Run the connect request to completion.
            match exec.run_until_stalled(&mut connect_fut) {
                Poll::Ready(connect_result) => match connect_result {
                    Ok(_) => {}
                    Err(e) => panic!("failed to connect with {e}"),
                },
                Poll::Pending => panic!("expected the connect request to finish"),
            };
        }
        // Start running the client state machine.
        run_state_machine_futures(&mut exec, &mut iface_manager);

        // Verify that the ClientIfaceContainer has the correct config.
        assert_eq!(iface_manager.clients.len(), 1);
        assert_eq!(
            iface_manager.clients[0].config,
            ClientIfaceContainerConfig::Configured(connect_selection.target.network.clone())
        );
    }

    /// Tests the case where connect is called while the only available interface is currently
    /// unconfigured.
    #[fuchsia::test]
    fn test_connect_with_unconfigured_iface() {
        let mut exec = fuchsia_async::TestExecutor::new();

        let test_values = test_setup(&mut exec);
        let (mut iface_manager, mut _sme_stream) =
            create_iface_manager_with_client(&test_values, false);

        // Add credentials for the test network to the saved networks.
        let connect_selection = generate_connect_selection();
        let save_network_fut = test_values.saved_networks.store(
            connect_selection.target.network.clone(),
            connect_selection.target.credential.clone(),
        );
        let mut save_network_fut = pin!(save_network_fut);
        assert_matches!(exec.run_until_stalled(&mut save_network_fut), Poll::Ready(_));

        {
            let connect_fut = iface_manager.connect(connect_selection.clone());
            let mut connect_fut = pin!(connect_fut);

            // Expect that we have requested a client SME proxy.
            assert_matches!(exec.run_until_stalled(&mut connect_fut), Poll::Pending);

            let mut monitor_service_fut = test_values.monitor_service_stream.into_future();
            let sme_server = assert_matches!(
                poll_service_req(&mut exec, &mut monitor_service_fut),
                Poll::Ready(fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::GetClientSme {
                    iface_id: TEST_CLIENT_IFACE_ID, sme_server, responder
                }) => {
                    // Send back a positive acknowledgement.
                    assert!(responder.send(Ok(())).is_ok());

                    sme_server
                }
            );
            _sme_stream = sme_server.into_stream().into_future();

            let mut connect_fut = pin!(connect_fut);
            match exec.run_until_stalled(&mut connect_fut) {
                Poll::Ready(connect_result) => match connect_result {
                    Ok(_) => {}
                    Err(e) => panic!("failed to connect with {e}"),
                },
                Poll::Pending => panic!("expected the connect request to finish"),
            };
        }
        // Start running the client state machine.
        run_state_machine_futures(&mut exec, &mut iface_manager);

        // Acknowledge the disconnection attempt.
        assert_matches!(
            poll_sme_req(&mut exec, &mut _sme_stream),
            Poll::Ready(fidl_fuchsia_wlan_sme::ClientSmeRequest::Disconnect{ responder, reason: fidl_fuchsia_wlan_sme::UserDisconnectReason::Startup }) => {
                responder.send().expect("could not send response")
            }
        );

        // Make sure that the connect request has been sent out.
        run_state_machine_futures(&mut exec, &mut iface_manager);
        let connect_txn_handle = assert_matches!(
            poll_sme_req(&mut exec, &mut _sme_stream),
            Poll::Ready(fidl_fuchsia_wlan_sme::ClientSmeRequest::Connect{ req, txn, control_handle: _ }) => {
                assert_eq!(req.ssid, connect_selection.target.network.ssid.clone());
                let (_stream, ctrl) = txn.expect("connect txn unused")
                    .into_stream_and_control_handle();
                ctrl
            }
        );
        connect_txn_handle
            .send_on_connect_result(&fake_successful_connect_result())
            .expect("failed to send connection completion");

        // Run the state machine future again so that it acks the oneshot.
        run_state_machine_futures(&mut exec, &mut iface_manager);

        // Verify that the ClientIfaceContainer has been moved from unconfigured to configured.
        assert_eq!(iface_manager.clients.len(), 1);
        assert_eq!(
            iface_manager.clients[0].config,
            ClientIfaceContainerConfig::Configured(connect_selection.target.network.clone())
        );
    }

    #[fuchsia::test]
    fn test_connect_request_cancels_ongoing_connection_selections_for_autoconnect() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let test_values = test_setup(&mut exec);
        let (mut iface_manager, _stream) = create_iface_manager_with_client(&test_values, false);

        // Add a saved network
        let connect_selection = generate_connect_selection();
        let save_network_fut = test_values.saved_networks.store(
            connect_selection.target.network.clone(),
            connect_selection.target.credential.clone(),
        );
        let mut save_network_fut = pin!(save_network_fut);
        assert_matches!(exec.run_until_stalled(&mut save_network_fut), Poll::Ready(_));

        // Use the public API to initiate automatic connection selection.
        {
            let mut sel_fut = pin!(initiate_automatic_connection_selection(&mut iface_manager));
            assert_matches!(exec.run_until_stalled(&mut sel_fut), Poll::Ready(()));
        }

        // Verify it is active
        assert!(
            iface_manager
                .connection_selection_manager
                .active_selections()
                .contains(&SelectionIdentifier::Automatic)
        );

        // Now initiate a connect request
        let connect_request = ConnectAttemptRequest::new(
            connect_selection.target.network.clone(),
            connect_selection.target.credential.clone(),
            connect_selection.reason,
        );

        {
            let connect_fut = iface_manager.handle_connect_request(connect_request.clone());
            let mut connect_fut = pin!(connect_fut);
            // It should return Ok(()) immediately as it spawns the future
            assert_matches!(exec.run_until_stalled(&mut connect_fut), Poll::Ready(Ok(())));
        }

        // Verify Automatic is gone and ConnectRequest is present
        let active_selections = iface_manager.connection_selection_manager.active_selections();
        assert!(!active_selections.contains(&SelectionIdentifier::Automatic));
        assert!(
            active_selections
                .contains(&SelectionIdentifier::ConnectRequest(connect_request.network))
        );
    }

    #[fuchsia::test]
    fn test_connect_request_cancels_ongoing_connection_selections_for_unrelated_connect_requests() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let test_values = test_setup(&mut exec);
        let (mut iface_manager, _stream) = create_iface_manager_with_client(&test_values, false);

        // Initiate first connect request
        let network_a = NetworkIdentifier::new(
            client_types::Ssid::try_from("network_a").unwrap(),
            SecurityType::Wpa,
        );
        let request_a = ConnectAttemptRequest::new(
            network_a.clone(),
            Credential::None,
            client_types::ConnectReason::FidlConnectRequest,
        );

        {
            let connect_fut = iface_manager.handle_connect_request(request_a.clone());
            let mut connect_fut = pin!(connect_fut);
            assert_matches!(exec.run_until_stalled(&mut connect_fut), Poll::Ready(Ok(())));
        }

        assert!(
            iface_manager
                .connection_selection_manager
                .active_selections()
                .contains(&SelectionIdentifier::ConnectRequest(network_a.clone()))
        );

        // Initiate second unrelated connect request
        let network_b = NetworkIdentifier::new(
            client_types::Ssid::try_from("network_b").unwrap(),
            SecurityType::Wpa,
        );
        let request_b = ConnectAttemptRequest::new(
            network_b.clone(),
            Credential::None,
            client_types::ConnectReason::FidlConnectRequest,
        );

        {
            let connect_fut_b = iface_manager.handle_connect_request(request_b.clone());
            let mut connect_fut_b = pin!(connect_fut_b);
            assert_matches!(exec.run_until_stalled(&mut connect_fut_b), Poll::Ready(Ok(())));
        }

        // Verify A is gone, B is present
        let active_selections = iface_manager.connection_selection_manager.active_selections();
        assert!(!active_selections.contains(&SelectionIdentifier::ConnectRequest(network_a)));
        assert!(active_selections.contains(&SelectionIdentifier::ConnectRequest(network_b)));
    }

    #[fuchsia::test]
    fn test_connect_request_dedupes_with_ongoing_connection_selections() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let mut test_values = test_setup(&mut exec);
        let (mut iface_manager, _stream) = create_iface_manager_with_client(&test_values, false);

        let network_a = NetworkIdentifier::new(
            client_types::Ssid::try_from("network_a").unwrap(),
            SecurityType::Wpa,
        );
        let request_a = ConnectAttemptRequest::new(
            network_a.clone(),
            Credential::None,
            client_types::ConnectReason::FidlConnectRequest,
        );

        // First request
        {
            let connect_fut = iface_manager.handle_connect_request(request_a.clone());
            let mut connect_fut = pin!(connect_fut);
            assert_matches!(exec.run_until_stalled(&mut connect_fut), Poll::Ready(Ok(())));
        }

        // Drive the manager to ensure the request is sent
        {
            let next_fut = iface_manager.connection_selection_manager.get_futures().next();
            let mut next_fut = pin!(next_fut);
            let _ = exec.run_until_stalled(&mut next_fut);
        }

        // Consume the first request from the channel to verify it was sent
        assert_matches!(test_values.connection_selection_request_receiver.try_next(), Ok(Some(ConnectionSelectionRequest::NewConnectionSelection { network_id: Some(id), .. })) if id == network_a);

        // Second request (same network)
        {
            let connect_fut_2 = iface_manager.handle_connect_request(request_a.clone());
            let mut connect_fut_2 = pin!(connect_fut_2);
            assert_matches!(exec.run_until_stalled(&mut connect_fut_2), Poll::Ready(Ok(())));
        }

        // Verify NO new request in channel
        assert_matches!(test_values.connection_selection_request_receiver.try_next(), Err(_));

        // Verify still valid
        assert!(
            iface_manager
                .connection_selection_manager
                .active_selections()
                .contains(&SelectionIdentifier::ConnectRequest(network_a))
        );
    }

    #[fuchsia::test]
    fn test_disconnect_cancels_ongoing_connection_selections_for_autoconnect() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let test_values = test_setup(&mut exec);
        let (mut iface_manager, _stream) = create_iface_manager_with_client(&test_values, false);

        // Add a saved network
        let connect_selection = generate_connect_selection();
        let save_network_fut = test_values.saved_networks.store(
            connect_selection.target.network.clone(),
            connect_selection.target.credential.clone(),
        );
        let mut save_network_fut = pin!(save_network_fut);
        assert_matches!(exec.run_until_stalled(&mut save_network_fut), Poll::Ready(_));

        // Use the public API to initiate automatic connection selection.
        {
            let mut sel_fut = pin!(initiate_automatic_connection_selection(&mut iface_manager));
            assert_matches!(exec.run_until_stalled(&mut sel_fut), Poll::Ready(()));
        }

        // Verify it is active
        assert!(
            iface_manager
                .connection_selection_manager
                .active_selections()
                .contains(&SelectionIdentifier::Automatic)
        );

        // Disconnect
        {
            let disconnect_fut = iface_manager.disconnect(
                connect_selection.target.network.clone(),
                client_types::DisconnectReason::NetworkUnsaved,
            );
            let mut disconnect_fut = pin!(disconnect_fut);
            assert_matches!(exec.run_until_stalled(&mut disconnect_fut), Poll::Ready(Ok(())));
        }

        // Verify cancelled
        assert!(
            !iface_manager
                .connection_selection_manager
                .active_selections()
                .contains(&SelectionIdentifier::Automatic)
        );
    }

    #[fuchsia::test]
    fn test_disconnect_cancels_ongoing_connection_selections_for_connect_request() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let test_values = test_setup(&mut exec);
        let (mut iface_manager, _stream) = create_iface_manager_with_client(&test_values, false);

        let network_a = NetworkIdentifier::new(
            client_types::Ssid::try_from("network_a").unwrap(),
            SecurityType::Wpa,
        );
        let request_a = ConnectAttemptRequest::new(
            network_a.clone(),
            Credential::None,
            client_types::ConnectReason::FidlConnectRequest,
        );

        // Start connect request
        {
            let connect_fut = iface_manager.handle_connect_request(request_a.clone());
            let mut connect_fut = pin!(connect_fut);
            assert_matches!(exec.run_until_stalled(&mut connect_fut), Poll::Ready(Ok(())));
        }

        assert!(
            iface_manager
                .connection_selection_manager
                .active_selections()
                .contains(&SelectionIdentifier::ConnectRequest(network_a.clone()))
        );

        // Disconnect
        {
            let disconnect_fut = iface_manager
                .disconnect(network_a.clone(), client_types::DisconnectReason::NetworkUnsaved);
            let mut disconnect_fut = pin!(disconnect_fut);
            assert_matches!(exec.run_until_stalled(&mut disconnect_fut), Poll::Ready(Ok(())));
        }

        // Verify cancelled
        assert!(
            !iface_manager
                .connection_selection_manager
                .active_selections()
                .contains(&SelectionIdentifier::ConnectRequest(network_a))
        );
    }

    #[fuchsia::test]
    fn test_disconnect_does_not_cancel_connection_selections_for_unrelated_connect_request() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let test_values = test_setup(&mut exec);
        let (mut iface_manager, _stream) = create_iface_manager_with_client(&test_values, false);

        let network_a = NetworkIdentifier::new(
            client_types::Ssid::try_from("network_a").unwrap(),
            SecurityType::Wpa,
        );
        let request_a = ConnectAttemptRequest::new(
            network_a.clone(),
            Credential::None,
            client_types::ConnectReason::FidlConnectRequest,
        );

        // Start connect request
        {
            let connect_fut = iface_manager.handle_connect_request(request_a.clone());
            let mut connect_fut = pin!(connect_fut);
            assert_matches!(exec.run_until_stalled(&mut connect_fut), Poll::Ready(Ok(())));
        }

        // Disconnect UNRELATED network
        let network_b = NetworkIdentifier::new(
            client_types::Ssid::try_from("network_b").unwrap(),
            SecurityType::Wpa,
        );
        {
            let disconnect_fut = iface_manager
                .disconnect(network_b.clone(), client_types::DisconnectReason::NetworkUnsaved);
            let mut disconnect_fut = pin!(disconnect_fut);
            // It might return error via ready if iface not found, or Ok if handled gracefully.
            // The disconnect implementation returns Ok generally even if not connected, unless error in FSM.
            assert_matches!(exec.run_until_stalled(&mut disconnect_fut), Poll::Ready(Ok(())));
        }

        // Verify A is STILL THERE
        assert!(
            iface_manager
                .connection_selection_manager
                .active_selections()
                .contains(&SelectionIdentifier::ConnectRequest(network_a))
        );
    }

    /// Tests the case where connect is called, but no client ifaces exist.
    #[fuchsia::test]
    fn test_connect_with_no_ifaces() {
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create a PhyManager with no knowledge of any client ifaces.
        let test_values = test_setup(&mut exec);
        let phy_manager = create_empty_phy_manager(
            test_values.monitor_service_proxy.clone(),
            test_values.node.clone_weak(),
            test_values.telemetry_sender.clone(),
            test_values.recovery_sender,
        );

        let mut iface_manager = IfaceManagerService::new(
            phy_manager,
            test_values.client_update_sender,
            test_values.ap_update_sender,
            test_values.monitor_service_proxy,
            test_values.saved_networks,
            test_values.connection_selection_requester,
            test_values.roam_manager,
            test_values.telemetry_sender,
            test_values.defect_sender,
            test_values.node,
        );

        // Call connect on the IfaceManager
        let connect_fut = iface_manager.connect(generate_connect_selection());

        // Verify that the request to connect results in an error.
        let mut connect_fut = pin!(connect_fut);
        assert_matches!(exec.run_until_stalled(&mut connect_fut), Poll::Ready(Err(_)));
    }

    /// Tests the case where connect is called, but client connections are disabled.
    #[fuchsia::test]
    fn test_connect_while_connections_disabled() {
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create a PhyManager with no knowledge of any client ifaces.
        let test_values = test_setup(&mut exec);
        let phy_manager = Arc::new(Mutex::new(FakePhyManager {
            create_iface_ok: true,
            destroy_iface_ok: true,
            set_country_ok: true,
            country_code: None,
            client_connections_enabled: false,
            client_ifaces: vec![],
            defects: vec![],
            recovery_sender: None,
        }));

        // Create the IfaceManager
        let mut iface_manager = IfaceManagerService::new(
            phy_manager,
            test_values.client_update_sender,
            test_values.ap_update_sender,
            test_values.monitor_service_proxy,
            test_values.saved_networks.clone(),
            test_values.connection_selection_requester,
            test_values.roam_manager.clone(),
            test_values.telemetry_sender,
            test_values.defect_sender,
            test_values.node,
        );

        // Construct the connect request.
        let connect_selection = generate_connect_selection();
        let request = ConnectAttemptRequest::new(
            connect_selection.target.network,
            connect_selection.target.credential,
            client_types::ConnectReason::FidlConnectRequest,
        );

        // Attempt to handle a connect request.
        let connect_fut = iface_manager.handle_connect_request(request);

        // Verify that the request to connect results in an error.
        let mut connect_fut = pin!(connect_fut);
        assert_matches!(exec.run_until_stalled(&mut connect_fut), Poll::Ready(Err(_)));
    }

    /// Tests the case where the PhyManager knows of a client iface, but the IfaceManager is not
    /// able to create an SME proxy for it.
    #[fuchsia::test]
    fn test_connect_sme_creation_fails() {
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create an IfaceManager and drop its client
        let test_values = test_setup(&mut exec);
        let (mut iface_manager, _) = create_iface_manager_with_client(&test_values, true);
        let _ = iface_manager.clients.pop();

        // Drop the serving end of our device service proxy so that the request to create an SME
        // proxy fails.
        drop(test_values.monitor_service_stream);

        // Update the saved networks with knowledge of the test SSID and credentials.
        let connect_selection = generate_connect_selection();
        assert!(
            exec.run_singlethreaded(test_values.saved_networks.store(
                connect_selection.target.network.clone(),
                connect_selection.target.credential.clone()
            ))
            .expect("failed to store a network password")
            .is_none()
        );

        // Ask the IfaceManager to connect and make sure that it fails.
        let connect_fut = iface_manager.connect(connect_selection);

        let mut connect_fut = pin!(connect_fut);
        assert_matches!(exec.run_until_stalled(&mut connect_fut), Poll::Ready(Err(_)));
    }

    /// Tests the case where disconnect is called on a configured client.
    #[fuchsia::test]
    fn test_disconnect_configured_iface() {
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create a configured ClientIfaceContainer.
        let test_values = test_setup(&mut exec);
        let (mut iface_manager, _) = create_iface_manager_with_client(&test_values, true);

        let network_id = NetworkIdentifier::new(TEST_SSID.clone(), SecurityType::Wpa);
        let credential = Credential::Password(TEST_PASSWORD.as_bytes().to_vec());
        assert!(
            exec.run_singlethreaded(test_values.saved_networks.store(network_id, credential))
                .expect("failed to store a network password")
                .is_none()
        );

        {
            // Issue a call to disconnect from the network.
            let network_id = ap_types::NetworkIdentifier {
                ssid: TEST_SSID.clone(),
                security_type: ap_types::SecurityType::Wpa,
            };
            let disconnect_fut = iface_manager
                .disconnect(network_id, client_types::DisconnectReason::NetworkUnsaved);

            // Ensure that disconnect returns a successful response.
            let mut disconnect_fut = pin!(disconnect_fut);
            assert_matches!(exec.run_until_stalled(&mut disconnect_fut), Poll::Ready(Ok(_)));
        }

        // Verify that the ClientIfaceContainer has been moved from configured to unconfigured.
        assert_eq!(iface_manager.clients.len(), 1);
        assert_eq!(iface_manager.clients[0].config, ClientIfaceContainerConfig::Unconfigured);
    }

    /// Tests the case where disconnect is called for a network for which the IfaceManager is not
    /// configured.  Verifies that the configured client is unaffected.
    #[fuchsia::test]
    fn test_disconnect_nonexistent_config() {
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create a ClientIfaceContainer with a valid client.
        let test_values = test_setup(&mut exec);
        let (mut iface_manager, _) = create_iface_manager_with_client(&test_values, true);

        // Create a PhyManager with knowledge of a single client iface.
        let network_id = NetworkIdentifier::new(TEST_SSID.clone(), SecurityType::Wpa);
        let credential = Credential::Password(TEST_PASSWORD.as_bytes().to_vec());
        assert!(
            exec.run_singlethreaded(test_values.saved_networks.store(network_id, credential))
                .expect("failed to store a network password")
                .is_none()
        );

        {
            // Issue a disconnect request for a bogus network configuration.
            let network_id = ap_types::NetworkIdentifier {
                ssid: ap_types::Ssid::try_from("nonexistent_ssid").unwrap(),
                security_type: ap_types::SecurityType::Wpa,
            };
            let disconnect_fut = iface_manager
                .disconnect(network_id, client_types::DisconnectReason::NetworkUnsaved);

            // Ensure that the request returns immediately.
            let mut disconnect_fut = pin!(disconnect_fut);
            assert!(exec.run_until_stalled(&mut disconnect_fut).is_ready());
        }

        // Verify that the configured client has not been affected.
        assert_eq!(iface_manager.clients.len(), 1);
        assert_matches!(
            &iface_manager.clients[0].config,
            ClientIfaceContainerConfig::Configured(..)
        );
    }

    /// Tests the case where disconnect is called and no client ifaces are present.
    #[fuchsia::test]
    fn test_disconnect_no_clients() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let test_values = test_setup(&mut exec);

        // Create an empty PhyManager and IfaceManager.
        let phy_manager = phy_manager::PhyManager::new(
            test_values.monitor_service_proxy.clone(),
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node.clone_weak(),
            test_values.telemetry_sender.clone(),
            test_values.recovery_sender,
        );
        let mut iface_manager = IfaceManagerService::new(
            Arc::new(Mutex::new(phy_manager)),
            test_values.client_update_sender,
            test_values.ap_update_sender,
            test_values.monitor_service_proxy,
            test_values.saved_networks,
            test_values.connection_selection_requester,
            test_values.roam_manager.clone(),
            test_values.telemetry_sender,
            test_values.defect_sender,
            test_values.node,
        );

        // Call disconnect on the IfaceManager
        let network_id = ap_types::NetworkIdentifier {
            ssid: ap_types::Ssid::try_from("nonexistent_ssid").unwrap(),
            security_type: ap_types::SecurityType::Wpa,
        };
        let disconnect_fut =
            iface_manager.disconnect(network_id, client_types::DisconnectReason::NetworkUnsaved);

        // Verify that disconnect returns immediately.
        let mut disconnect_fut = pin!(disconnect_fut);
        assert_matches!(exec.run_until_stalled(&mut disconnect_fut), Poll::Ready(Ok(_)));
    }

    /// Tests the case where the call to disconnect the client fails.
    #[fuchsia::test]
    fn test_disconnect_fails() {
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create a configured ClientIfaceContainer.
        let test_values = test_setup(&mut exec);
        let (mut iface_manager, _) = create_iface_manager_with_client(&test_values, true);

        // Make the client state machine's disconnect call fail.
        iface_manager.clients[0].client_state_machine = Some(Box::new(FakeClient {
            disconnect_ok: false,
            is_alive: true,
            expected_connect_selection: None,
        }));

        // Call disconnect on the IfaceManager
        let network_id = ap_types::NetworkIdentifier {
            ssid: TEST_SSID.clone(),
            security_type: ap_types::SecurityType::Wpa,
        };
        let disconnect_fut =
            iface_manager.disconnect(network_id, client_types::DisconnectReason::NetworkUnsaved);

        let mut disconnect_fut = pin!(disconnect_fut);
        assert_matches!(exec.run_until_stalled(&mut disconnect_fut), Poll::Ready(Err(_)));
    }

    /// Tests stop_client_connections when there is a client that is connected.
    #[fuchsia::test]
    fn test_stop_connected_client() {
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create a configured ClientIfaceContainer.
        let mut test_values = test_setup(&mut exec);
        let (mut iface_manager, _) = create_iface_manager_with_client(&test_values, true);

        // Create a PhyManager with a single, known client iface.
        let network_id = NetworkIdentifier::new(TEST_SSID.clone(), SecurityType::Wpa);
        let credential = Credential::Password(TEST_PASSWORD.as_bytes().to_vec());
        assert!(
            exec.run_singlethreaded(test_values.saved_networks.store(network_id, credential))
                .expect("failed to store a network password")
                .is_none()
        );

        {
            // Stop all client connections.
            let stop_fut = iface_manager.stop_client_connections(
                client_types::DisconnectReason::FidlStopClientConnectionsRequest,
            );
            let mut stop_fut = pin!(stop_fut);
            assert_matches!(exec.run_until_stalled(&mut stop_fut), Poll::Ready(Ok(_)));
        }

        // Ensure that no client interfaces are accounted for.
        assert!(iface_manager.clients.is_empty());

        // Ensure an update was sent
        let client_state_update = listener::ClientStateUpdate {
            state: fidl_fuchsia_wlan_policy::WlanClientState::ConnectionsDisabled,
            networks: vec![],
        };
        assert_matches!(
            test_values.client_update_receiver.try_next(),
            Ok(Some(listener::Message::NotifyListeners(updates))) => {
            assert_eq!(updates, client_state_update);
        });
    }

    /// Call stop_client_connections when the only available client is unconfigured.
    #[fuchsia::test]
    fn test_stop_unconfigured_client() {
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create a configured ClientIfaceContainer.
        let test_values = test_setup(&mut exec);
        let (mut iface_manager, _) = create_iface_manager_with_client(&test_values, true);

        // Create a PhyManager with one known client.
        let network_id = NetworkIdentifier::new(TEST_SSID.clone(), SecurityType::Wpa);
        let credential = Credential::Password(TEST_PASSWORD.as_bytes().to_vec());
        assert!(
            exec.run_singlethreaded(test_values.saved_networks.store(network_id, credential))
                .expect("failed to store a network password")
                .is_none()
        );

        {
            // Call stop_client_connections.
            let stop_fut = iface_manager.stop_client_connections(
                client_types::DisconnectReason::FidlStopClientConnectionsRequest,
            );
            let mut stop_fut = pin!(stop_fut);
            assert_matches!(exec.run_until_stalled(&mut stop_fut), Poll::Ready(Ok(_)));
        }

        // Ensure there are no remaining client ifaces.
        assert!(iface_manager.clients.is_empty());
    }

    /// Tests the case where stop_client_connections is called, but there are no client ifaces.
    #[fuchsia::test]
    fn test_stop_no_clients() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let test_values = test_setup(&mut exec);

        // Create and empty PhyManager and IfaceManager.
        let phy_manager = phy_manager::PhyManager::new(
            test_values.monitor_service_proxy.clone(),
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node.clone_weak(),
            test_values.telemetry_sender.clone(),
            test_values.recovery_sender,
        );
        let mut iface_manager = IfaceManagerService::new(
            Arc::new(Mutex::new(phy_manager)),
            test_values.client_update_sender,
            test_values.ap_update_sender,
            test_values.monitor_service_proxy,
            test_values.saved_networks,
            test_values.connection_selection_requester,
            test_values.roam_manager.clone(),
            test_values.telemetry_sender,
            test_values.defect_sender,
            test_values.node,
        );

        // Call stop_client_connections.
        {
            let stop_fut = iface_manager.stop_client_connections(
                client_types::DisconnectReason::FidlStopClientConnectionsRequest,
            );

            // Ensure stop_client_connections returns immediately and is successful.
            let mut stop_fut = pin!(stop_fut);
            assert_matches!(exec.run_until_stalled(&mut stop_fut), Poll::Ready(Ok(_)));
        }
    }

    /// Tests the case where client connections are stopped, but stopping one of the client state
    /// machines fails.
    #[fuchsia::test]
    fn test_stop_fails() {
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create a configured ClientIfaceContainer.
        let test_values = test_setup(&mut exec);
        let (mut iface_manager, _) = create_iface_manager_with_client(&test_values, true);
        iface_manager.clients[0].client_state_machine = Some(Box::new(FakeClient {
            disconnect_ok: false,
            is_alive: true,
            expected_connect_selection: None,
        }));

        // Create a PhyManager with a single, known client iface.
        let network_id = NetworkIdentifier::new(TEST_SSID.clone(), SecurityType::Wpa);
        let credential = Credential::Password(TEST_PASSWORD.as_bytes().to_vec());
        assert!(
            exec.run_singlethreaded(test_values.saved_networks.store(network_id, credential))
                .expect("failed to store a network password")
                .is_none()
        );

        {
            // Stop all client connections.
            let stop_fut = iface_manager.stop_client_connections(
                client_types::DisconnectReason::FidlStopClientConnectionsRequest,
            );
            let mut stop_fut = pin!(stop_fut);
            assert_matches!(exec.run_until_stalled(&mut stop_fut), Poll::Ready(Ok(_)));
        }

        // Ensure that no client interfaces are accounted for.
        assert!(iface_manager.clients.is_empty());
    }

    /// Tests the case where the IfaceManager fails to tear down all of the client ifaces.
    #[fuchsia::test]
    fn test_stop_iface_destruction_fails() {
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create a configured ClientIfaceContainer.
        let test_values = test_setup(&mut exec);
        let (mut iface_manager, _) = create_iface_manager_with_client(&test_values, true);
        iface_manager.phy_manager = Arc::new(Mutex::new(FakePhyManager {
            create_iface_ok: true,
            destroy_iface_ok: false,
            set_country_ok: true,
            country_code: None,
            client_connections_enabled: true,
            client_ifaces: vec![TEST_CLIENT_IFACE_ID],
            defects: vec![],
            recovery_sender: None,
        }));

        // Create a PhyManager with a single, known client iface.
        let network_id = NetworkIdentifier::new(TEST_SSID.clone(), SecurityType::Wpa);
        let credential = Credential::Password(TEST_PASSWORD.as_bytes().to_vec());
        assert!(
            exec.run_singlethreaded(test_values.saved_networks.store(network_id, credential))
                .expect("failed to store a network password")
                .is_none()
        );

        {
            // Stop all client connections.
            let stop_fut = iface_manager.stop_client_connections(
                client_types::DisconnectReason::FidlStopClientConnectionsRequest,
            );
            let mut stop_fut = pin!(stop_fut);
            assert_matches!(exec.run_until_stalled(&mut stop_fut), Poll::Ready(Err(_)));
        }

        // Ensure that no client interfaces are accounted for.
        assert!(iface_manager.clients.is_empty());
    }

    /// Tests the case where StopClientConnections is called when the client interfaces are already
    /// stopped.
    #[fuchsia::test]
    fn test_stop_client_when_already_stopped() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let mut test_values = test_setup(&mut exec);

        // Create an empty PhyManager and IfaceManager.
        let phy_manager = phy_manager::PhyManager::new(
            test_values.monitor_service_proxy.clone(),
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node.clone_weak(),
            test_values.telemetry_sender.clone(),
            test_values.recovery_sender,
        );
        let mut iface_manager = IfaceManagerService::new(
            Arc::new(Mutex::new(phy_manager)),
            test_values.client_update_sender,
            test_values.ap_update_sender,
            test_values.monitor_service_proxy,
            test_values.saved_networks,
            test_values.connection_selection_requester,
            test_values.roam_manager.clone(),
            test_values.telemetry_sender,
            test_values.defect_sender,
            test_values.node,
        );

        // Call stop_client_connections.
        {
            let stop_fut = iface_manager.stop_client_connections(
                client_types::DisconnectReason::FidlStopClientConnectionsRequest,
            );

            // Ensure stop_client_connections returns immediately and is successful.
            let mut stop_fut = pin!(stop_fut);
            assert_matches!(exec.run_until_stalled(&mut stop_fut), Poll::Ready(Ok(_)));
        }

        // Verify that telemetry event has been sent
        let event = assert_matches!(test_values.telemetry_receiver.try_next(), Ok(Some(ev)) => ev);
        assert_matches!(event, TelemetryEvent::ClearEstablishConnectionStartTime);
    }

    /// Tests the case where an existing iface is marked as idle.
    #[fuchsia::test]
    fn test_mark_iface_idle() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let test_values = test_setup(&mut exec);
        let (mut iface_manager, _) = create_iface_manager_with_client(&test_values, true);

        // Setup the client state machine so that it looks like it is no longer alive.
        iface_manager.clients[0].client_state_machine = Some(Box::new(FakeClient {
            disconnect_ok: true,
            is_alive: false,
            expected_connect_selection: None,
        }));

        assert_matches!(
            &iface_manager.clients[0].config,
            ClientIfaceContainerConfig::Configured(..)
        );
        iface_manager.record_idle_client(TEST_CLIENT_IFACE_ID);
        assert_matches!(&iface_manager.clients[0].config, ClientIfaceContainerConfig::Unconfigured);
    }

    /// Tests the case where a running and configured iface is marked as idle.
    #[fuchsia::test]
    fn test_mark_active_iface_idle() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let test_values = test_setup(&mut exec);
        let (mut iface_manager, _) = create_iface_manager_with_client(&test_values, true);

        assert_matches!(
            &iface_manager.clients[0].config,
            ClientIfaceContainerConfig::Configured(..)
        );

        // The request to mark the interface as idle should be ignored since the interface's state
        // machine is still running.
        iface_manager.record_idle_client(TEST_CLIENT_IFACE_ID);
        assert_matches!(
            &iface_manager.clients[0].config,
            ClientIfaceContainerConfig::Configured(..)
        );
    }

    /// Tests the case where a non-existent iface is marked as idle.
    #[fuchsia::test]
    fn test_mark_nonexistent_iface_idle() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let test_values = test_setup(&mut exec);
        let (mut iface_manager, _) = create_iface_manager_with_client(&test_values, true);

        assert_matches!(
            &iface_manager.clients[0].config,
            ClientIfaceContainerConfig::Configured(..)
        );
        iface_manager.record_idle_client(123);
        assert_matches!(
            &iface_manager.clients[0].config,
            ClientIfaceContainerConfig::Configured(..)
        );
    }

    /// Tests the case where an iface is not configured and has_idle_client is called.
    #[fuchsia::test]
    fn test_unconfigured_iface_idle_check() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let test_values = test_setup(&mut exec);
        let (iface_manager, _) = create_iface_manager_with_client(&test_values, false);
        assert!(!iface_manager.idle_clients().is_empty());
    }

    /// Tests the case where an iface is configured and alive and has_idle_client is called.
    #[fuchsia::test]
    fn test_configured_alive_iface_idle_check() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let test_values = test_setup(&mut exec);
        let (iface_manager, _) = create_iface_manager_with_client(&test_values, true);
        assert!(iface_manager.idle_clients().is_empty());
    }

    /// Tests the case where an iface is configured and dead and has_idle_client is called.
    #[fuchsia::test]
    fn test_configured_dead_iface_idle_check() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let test_values = test_setup(&mut exec);
        let (mut iface_manager, _) = create_iface_manager_with_client(&test_values, true);

        // Make the client state machine's liveness check fail.
        iface_manager.clients[0].client_state_machine = Some(Box::new(FakeClient {
            disconnect_ok: true,
            is_alive: false,
            expected_connect_selection: None,
        }));

        assert!(!iface_manager.idle_clients().is_empty());
    }

    /// Tests the case where not ifaces are present and has_idle_client is called.
    #[fuchsia::test]
    fn test_no_ifaces_idle_check() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let test_values = test_setup(&mut exec);
        let (mut iface_manager, _) = create_iface_manager_with_client(&test_values, true);
        let _ = iface_manager.clients.pop();
        assert!(iface_manager.idle_clients().is_empty());
    }

    /// Tests the case where starting client connections succeeds.
    #[fuchsia::test]
    fn test_start_clients_succeeds() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let mut test_values = test_setup(&mut exec);

        // Create an empty PhyManager and IfaceManager.
        let phy_manager = phy_manager::PhyManager::new(
            test_values.monitor_service_proxy.clone(),
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node.clone_weak(),
            test_values.telemetry_sender.clone(),
            test_values.recovery_sender,
        );
        let mut iface_manager = IfaceManagerService::new(
            Arc::new(Mutex::new(phy_manager)),
            test_values.client_update_sender,
            test_values.ap_update_sender,
            test_values.monitor_service_proxy,
            test_values.saved_networks,
            test_values.connection_selection_requester,
            test_values.roam_manager.clone(),
            test_values.telemetry_sender,
            test_values.defect_sender,
            test_values.node,
        );

        {
            let start_fut = iface_manager.start_client_connections();

            // Ensure start_client_connections returns immediately and is successful.
            let mut start_fut = pin!(start_fut);
            assert_matches!(exec.run_until_stalled(&mut start_fut), Poll::Ready(Ok(_)));
        }

        // Ensure no update is sent
        assert_matches!(test_values.client_update_receiver.try_next(), Err(_));
    }

    /// Tests the case where starting client connections fails.
    #[fuchsia::test]
    fn test_start_clients_fails() {
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create a configured ClientIfaceContainer.
        let test_values = test_setup(&mut exec);
        let (mut iface_manager, _) = create_iface_manager_with_client(&test_values, true);
        iface_manager.phy_manager = Arc::new(Mutex::new(FakePhyManager {
            create_iface_ok: false,
            destroy_iface_ok: true,
            set_country_ok: true,
            country_code: None,
            client_connections_enabled: true,
            client_ifaces: vec![],
            defects: vec![],
            recovery_sender: None,
        }));

        {
            let start_fut = iface_manager.start_client_connections();
            let mut start_fut = pin!(start_fut);
            assert_matches!(exec.run_until_stalled(&mut start_fut), Poll::Ready(Err(_)));
        }
    }

    /// Tests the case where there is a lingering client interface to ensure that it is resumed to
    /// a working state.
    #[fuchsia::test]
    fn test_start_clients_with_lingering_iface() {
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create an IfaceManager with no clients and client connections initially disabled.  Seed
        // it with a PhyManager that knows of a lingering client interface.
        let mut test_values = test_setup(&mut exec);
        let (mut iface_manager, _) = create_iface_manager_with_client(&test_values, false);
        iface_manager.phy_manager = Arc::new(Mutex::new(FakePhyManager {
            create_iface_ok: true,
            destroy_iface_ok: true,
            set_country_ok: true,
            country_code: None,
            client_connections_enabled: false,
            client_ifaces: vec![TEST_CLIENT_IFACE_ID],
            defects: vec![],
            recovery_sender: None,
        }));

        // Delete all client records initially.
        iface_manager.clients.clear();
        assert!(iface_manager.fsm_termination_futures.is_empty());

        {
            let start_fut = iface_manager.start_client_connections();
            let mut start_fut = pin!(start_fut);

            // The IfaceManager will first query to determine the type of interface.
            assert_matches!(exec.run_until_stalled(&mut start_fut), Poll::Pending);
            assert_matches!(
                exec.run_until_stalled(&mut test_values.monitor_service_stream.next()),
                Poll::Ready(Some(Ok(fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::QueryIface {
                    iface_id: TEST_CLIENT_IFACE_ID, responder
                }))) => {
                    let response = fidl_fuchsia_wlan_device_service::QueryIfaceResponse {
                        role: fidl_fuchsia_wlan_common::WlanMacRole::Client,
                        id: TEST_CLIENT_IFACE_ID,
                        phy_id: 0,
                        phy_assigned_id: 0,
                        sta_addr: [0; 6],
                        factory_addr: [0; 6],
                    };
                    responder
                        .send(Ok(&response))
                        .expect("Failed to send iface response");
                }
            );

            // The request should stall out while attempting to get a client interface.
            assert_matches!(exec.run_until_stalled(&mut start_fut), Poll::Pending);
            assert_matches!(
                exec.run_until_stalled(&mut test_values.monitor_service_stream.next()),
                Poll::Ready(Some(Ok(fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::GetClientSme {
                    iface_id: TEST_CLIENT_IFACE_ID, sme_server: _, responder
                }))) => {
                    // Send back a positive acknowledgement.
                    assert!(responder.send(Ok(())).is_ok());
                }
            );

            // Expect that we have requested a client SME proxy from creating the client state
            // machine.
            assert_matches!(exec.run_until_stalled(&mut start_fut), Poll::Pending);
            assert_matches!(
                exec.run_until_stalled(&mut test_values.monitor_service_stream.next()),
                Poll::Ready(Some(Ok(fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::GetClientSme {
                    iface_id: TEST_CLIENT_IFACE_ID, sme_server: _, responder
                }))) => {
                    // Send back a positive acknowledgement.
                    assert!(responder.send(Ok(())).is_ok());
                }
            );

            // The request should complete successfully.
            assert_matches!(exec.run_until_stalled(&mut start_fut), Poll::Ready(Ok(())));
        }

        assert!(!iface_manager.clients.is_empty());
        assert!(!iface_manager.fsm_termination_futures.is_empty());
    }

    /// Tests the case where the IfaceManager is able to request that the AP state machine start
    /// the access point.
    #[fuchsia::test]
    fn test_start_ap_succeeds() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let test_values = test_setup(&mut exec);
        let fake_ap = FakeAp { start_succeeds: true, stop_succeeds: true, exit_succeeds: true };
        let mut iface_manager = create_iface_manager_with_ap(&test_values, fake_ap);
        let config = create_ap_config(&TEST_SSID, TEST_PASSWORD);

        {
            let fut = iface_manager.start_ap(config);

            let mut fut = pin!(fut);
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(_)));
        }

        assert!(iface_manager.aps[0].enabled_time.is_some());
    }

    /// Tests the case where the IfaceManager is not able to request that the AP state machine start
    /// the access point.
    #[fuchsia::test]
    fn test_start_ap_fails() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let test_values = test_setup(&mut exec);
        let fake_ap = FakeAp { start_succeeds: false, stop_succeeds: true, exit_succeeds: true };
        let mut iface_manager = create_iface_manager_with_ap(&test_values, fake_ap);
        let config = create_ap_config(&TEST_SSID, TEST_PASSWORD);

        {
            let fut = iface_manager.start_ap(config);

            let mut fut = pin!(fut);
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));
        }
    }

    /// Tests the case where start is called on the IfaceManager, but there are no AP ifaces.
    #[fuchsia::test]
    fn test_start_ap_no_ifaces() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let test_values = test_setup(&mut exec);

        // Create an empty PhyManager and IfaceManager.
        let phy_manager = phy_manager::PhyManager::new(
            test_values.monitor_service_proxy.clone(),
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node.clone_weak(),
            test_values.telemetry_sender.clone(),
            test_values.recovery_sender,
        );
        let mut iface_manager = IfaceManagerService::new(
            Arc::new(Mutex::new(phy_manager)),
            test_values.client_update_sender,
            test_values.ap_update_sender,
            test_values.monitor_service_proxy,
            test_values.saved_networks,
            test_values.connection_selection_requester,
            test_values.roam_manager.clone(),
            test_values.telemetry_sender,
            test_values.defect_sender,
            test_values.node,
        );

        // Call start_ap.
        let config = create_ap_config(&TEST_SSID, TEST_PASSWORD);
        let fut = iface_manager.start_ap(config);

        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));
    }

    /// Tests the case where stop_ap is called for a config that is accounted for by the
    /// IfaceManager.
    #[fuchsia::test]
    fn test_stop_ap_succeeds() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let mut test_values = test_setup(&mut exec);
        let fake_ap = FakeAp { start_succeeds: true, stop_succeeds: true, exit_succeeds: true };
        let mut iface_manager = create_iface_manager_with_ap(&test_values, fake_ap);
        let config = create_ap_config(&TEST_SSID, TEST_PASSWORD);
        iface_manager.aps[0].config = Some(config);
        iface_manager.aps[0].enabled_time = Some(zx::MonotonicInstant::get());

        {
            let fut = iface_manager.stop_ap(TEST_SSID.clone(), TEST_PASSWORD.as_bytes().to_vec());
            let mut fut = pin!(fut);
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));
        }
        assert!(iface_manager.aps.is_empty());

        // Ensure a metric was logged.
        assert_matches!(
            test_values.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::StopAp { .. }))
        );
    }

    /// Tests the case where IfaceManager is requested to stop a config that is not accounted for.
    #[fuchsia::test]
    fn test_stop_ap_invalid_config() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let mut test_values = test_setup(&mut exec);
        let fake_ap = FakeAp { start_succeeds: true, stop_succeeds: true, exit_succeeds: true };
        let mut iface_manager = create_iface_manager_with_ap(&test_values, fake_ap);
        iface_manager.aps[0].enabled_time = Some(zx::MonotonicInstant::get());

        {
            let fut = iface_manager.stop_ap(TEST_SSID.clone(), TEST_PASSWORD.as_bytes().to_vec());
            let mut fut = pin!(fut);
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));
        }
        assert!(!iface_manager.aps.is_empty());

        // Ensure no metric was logged.
        assert_matches!(test_values.telemetry_receiver.try_next(), Err(_));

        // Ensure the AP start time has not been cleared.
        assert!(iface_manager.aps[0].enabled_time.is_some());
    }

    /// Tests the case where IfaceManager attempts to stop the AP state machine, but the request
    /// fails.
    #[fuchsia::test]
    fn test_stop_ap_stop_fails() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let mut test_values = test_setup(&mut exec);
        let fake_ap = FakeAp { start_succeeds: true, stop_succeeds: false, exit_succeeds: true };
        let mut iface_manager = create_iface_manager_with_ap(&test_values, fake_ap);
        let config = create_ap_config(&TEST_SSID, TEST_PASSWORD);
        iface_manager.aps[0].config = Some(config);
        iface_manager.aps[0].enabled_time = Some(zx::MonotonicInstant::get());

        {
            let fut = iface_manager.stop_ap(TEST_SSID.clone(), TEST_PASSWORD.as_bytes().to_vec());
            let mut fut = pin!(fut);
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));
        }

        assert!(iface_manager.aps.is_empty());

        // Ensure metric was logged.
        assert_matches!(
            test_values.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::StopAp { .. }))
        );
    }

    /// Tests the case where IfaceManager stops the AP state machine, but the request to exit
    /// fails.
    #[fuchsia::test]
    fn test_stop_ap_exit_fails() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let mut test_values = test_setup(&mut exec);
        let fake_ap = FakeAp { start_succeeds: true, stop_succeeds: true, exit_succeeds: false };
        let mut iface_manager = create_iface_manager_with_ap(&test_values, fake_ap);
        let config = create_ap_config(&TEST_SSID, TEST_PASSWORD);
        iface_manager.aps[0].config = Some(config);
        iface_manager.aps[0].enabled_time = Some(zx::MonotonicInstant::get());

        {
            let fut = iface_manager.stop_ap(TEST_SSID.clone(), TEST_PASSWORD.as_bytes().to_vec());
            let mut fut = pin!(fut);
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));
        }

        assert!(iface_manager.aps.is_empty());

        // Ensure metric was logged.
        assert_matches!(
            test_values.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::StopAp { .. }))
        );
    }

    /// Tests the case where stop is called on the IfaceManager, but there are no AP ifaces.
    #[fuchsia::test]
    fn test_stop_ap_no_ifaces() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let test_values = test_setup(&mut exec);

        // Create an empty PhyManager and IfaceManager.
        let phy_manager = phy_manager::PhyManager::new(
            test_values.monitor_service_proxy.clone(),
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node.clone_weak(),
            test_values.telemetry_sender.clone(),
            test_values.recovery_sender,
        );
        let mut iface_manager = IfaceManagerService::new(
            Arc::new(Mutex::new(phy_manager)),
            test_values.client_update_sender,
            test_values.ap_update_sender,
            test_values.monitor_service_proxy,
            test_values.saved_networks,
            test_values.connection_selection_requester,
            test_values.roam_manager.clone(),
            test_values.telemetry_sender,
            test_values.defect_sender,
            test_values.node,
        );
        let fut = iface_manager.stop_ap(TEST_SSID.clone(), TEST_PASSWORD.as_bytes().to_vec());
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));
    }

    /// Tests the case where stop_all_aps is called and it succeeds.
    #[fuchsia::test]
    fn test_stop_all_aps_succeeds() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let mut test_values = test_setup(&mut exec);
        let fake_ap = FakeAp { start_succeeds: true, stop_succeeds: true, exit_succeeds: true };
        let mut iface_manager = create_iface_manager_with_ap(&test_values, fake_ap);
        iface_manager.aps[0].enabled_time = Some(zx::MonotonicInstant::get());

        // Insert a second iface and add it to the list of APs.
        let (_publisher, status) = status_publisher_and_reader::<ap_fsm::Status>();
        let second_iface = ApIfaceContainer {
            iface_id: 2,
            config: None,
            ap_state_machine: Box::new(FakeAp {
                start_succeeds: true,
                stop_succeeds: true,
                exit_succeeds: true,
            }),
            state_machine_task: fasync::Task::local(async {}),
            enabled_time: Some(zx::MonotonicInstant::get()),
            status,
        };
        iface_manager.aps.push(second_iface);

        {
            let fut = iface_manager.stop_all_aps();
            let mut fut = pin!(fut);
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));
        }
        assert!(iface_manager.aps.is_empty());

        // Ensure metrics are logged for both AP interfaces.
        assert_matches!(
            test_values.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::StopAp { .. }))
        );
        assert_matches!(
            test_values.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::StopAp { .. }))
        );
    }

    /// Tests the case where stop_all_aps is called and the request to stop fails for an iface.
    #[fuchsia::test]
    fn test_stop_all_aps_stop_fails() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let mut test_values = test_setup(&mut exec);
        let fake_ap = FakeAp { start_succeeds: true, stop_succeeds: false, exit_succeeds: true };
        let mut iface_manager = create_iface_manager_with_ap(&test_values, fake_ap);
        iface_manager.aps[0].enabled_time = Some(zx::MonotonicInstant::get());

        // Insert a second iface and add it to the list of APs.
        let (_publisher, status) = status_publisher_and_reader::<ap_fsm::Status>();
        let second_iface = ApIfaceContainer {
            iface_id: 2,
            config: None,
            ap_state_machine: Box::new(FakeAp {
                start_succeeds: true,
                stop_succeeds: true,
                exit_succeeds: true,
            }),
            state_machine_task: fasync::Task::local(async {}),
            enabled_time: Some(zx::MonotonicInstant::get()),
            status,
        };
        iface_manager.aps.push(second_iface);

        {
            let fut = iface_manager.stop_all_aps();
            let mut fut = pin!(fut);
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));
        }
        assert!(iface_manager.aps.is_empty());

        // Ensure metrics are logged for both AP interfaces.
        assert_matches!(
            test_values.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::StopAp { .. }))
        );
        assert_matches!(
            test_values.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::StopAp { .. }))
        );
    }

    /// Tests the case where stop_all_aps is called and the request to stop fails for an iface.
    #[fuchsia::test]
    fn test_stop_all_aps_exit_fails() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let mut test_values = test_setup(&mut exec);
        let fake_ap = FakeAp { start_succeeds: true, stop_succeeds: true, exit_succeeds: false };
        let mut iface_manager = create_iface_manager_with_ap(&test_values, fake_ap);
        iface_manager.aps[0].enabled_time = Some(zx::MonotonicInstant::get());

        // Insert a second iface and add it to the list of APs.
        let (_publisher, status) = status_publisher_and_reader::<ap_fsm::Status>();
        let second_iface = ApIfaceContainer {
            iface_id: 2,
            config: None,
            ap_state_machine: Box::new(FakeAp {
                start_succeeds: true,
                stop_succeeds: true,
                exit_succeeds: true,
            }),
            state_machine_task: fasync::Task::local(async {}),
            enabled_time: Some(zx::MonotonicInstant::get()),
            status,
        };
        iface_manager.aps.push(second_iface);

        {
            let fut = iface_manager.stop_all_aps();
            let mut fut = pin!(fut);
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));
        }
        assert!(iface_manager.aps.is_empty());

        // Ensure metrics are logged for both AP interfaces.
        assert_matches!(
            test_values.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::StopAp { .. }))
        );
        assert_matches!(
            test_values.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::StopAp { .. }))
        );
    }

    /// Tests the case where stop_all_aps is called on the IfaceManager, but there are no AP
    /// ifaces.
    #[fuchsia::test]
    fn test_stop_all_aps_no_ifaces() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let mut test_values = test_setup(&mut exec);

        // Create an empty PhyManager and IfaceManager.
        let phy_manager = phy_manager::PhyManager::new(
            test_values.monitor_service_proxy.clone(),
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node.clone_weak(),
            test_values.telemetry_sender.clone(),
            test_values.recovery_sender,
        );
        let mut iface_manager = IfaceManagerService::new(
            Arc::new(Mutex::new(phy_manager)),
            test_values.client_update_sender,
            test_values.ap_update_sender,
            test_values.monitor_service_proxy,
            test_values.saved_networks,
            test_values.connection_selection_requester,
            test_values.roam_manager.clone(),
            test_values.telemetry_sender,
            test_values.defect_sender,
            test_values.node,
        );

        let fut = iface_manager.stop_all_aps();
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));

        // Ensure no metrics are logged.
        assert_matches!(test_values.telemetry_receiver.try_next(), Err(_));
    }

    /// Tests the case where there is a single AP interface and it is asked to start twice and then
    /// asked to stop.
    #[fuchsia::test]
    fn test_start_ap_twice_then_stop() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let mut test_values = test_setup(&mut exec);
        let fake_ap = FakeAp { start_succeeds: true, stop_succeeds: true, exit_succeeds: true };
        let mut iface_manager = create_iface_manager_with_ap(&test_values, fake_ap);
        let config = create_ap_config(&TEST_SSID, TEST_PASSWORD);

        // Issue an initial start command.
        {
            let fut = iface_manager.start_ap(config);

            let mut fut = pin!(fut);
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(_)));
        }

        // Record the initial start time.
        let initial_start_time = iface_manager.aps[0].enabled_time;

        // Now issue a second start command.
        let alternate_ssid = ap_types::Ssid::try_from("some_other_ssid").unwrap();
        let alternate_password = "some_other_password";
        let config = create_ap_config(&alternate_ssid, alternate_password);
        {
            let fut = iface_manager.start_ap(config);

            let mut fut = pin!(fut);
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(_)));
        }

        // Verify that the start time has not been updated.
        assert_eq!(initial_start_time, iface_manager.aps[0].enabled_time);

        // Verify that no metric has been recorded.
        assert_matches!(test_values.telemetry_receiver.try_next(), Err(_));

        // Now issue a stop command.
        {
            let fut = iface_manager.stop_all_aps();
            let mut fut = pin!(fut);
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));
        }
        assert!(iface_manager.aps.is_empty());

        // Make sure the metric has been sent.
        assert_matches!(
            test_values.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::StopAp { .. }))
        );
    }

    #[fuchsia::test]
    fn test_recover_removed_client_iface() {
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create an IfaceManager with a client and an AP.
        let mut test_values = test_setup(&mut exec);
        let (mut iface_manager, _next_sme_req) =
            create_iface_manager_with_client(&test_values, true);
        let removed_iface_id = 123;
        iface_manager.clients[0].iface_id = removed_iface_id;
        let (_publisher, status) = status_publisher_and_reader::<ap_fsm::Status>();

        let ap_iface = ApIfaceContainer {
            iface_id: TEST_AP_IFACE_ID,
            config: None,
            ap_state_machine: Box::new(FakeAp {
                start_succeeds: true,
                stop_succeeds: true,
                exit_succeeds: true,
            }),
            state_machine_task: fasync::Task::local(async {}),
            enabled_time: None,
            status,
        };
        iface_manager.aps.push(ap_iface);

        // Notify the IfaceManager that the client interface has been removed.
        {
            let fut = iface_manager.handle_removed_iface(removed_iface_id);
            let mut fut = pin!(fut);

            // Expect a DeviceMonitor request an SME proxy.
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
            assert_matches!(
                exec.run_until_stalled(&mut test_values.monitor_service_stream.next()),
                Poll::Ready(Some(Ok(fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::GetClientSme {
                    iface_id: TEST_CLIENT_IFACE_ID, sme_server: _, responder
                }))) => {
                    responder
                        .send(Ok(()))
                        .expect("failed to send AP SME response.");
                }
            );

            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));
        }

        assert!(!iface_manager.aps.is_empty());

        // Verify that a new client interface was created.
        assert_eq!(iface_manager.clients.len(), 1);
        assert_eq!(iface_manager.clients[0].iface_id, TEST_CLIENT_IFACE_ID);
    }

    #[fuchsia::test]
    fn test_client_iface_recovery_fails() {
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create an IfaceManager with a client and an AP.
        let mut test_values = test_setup(&mut exec);
        let (mut iface_manager, _next_sme_req) =
            create_iface_manager_with_client(&test_values, true);
        let removed_iface_id = 123;
        iface_manager.clients[0].iface_id = removed_iface_id;
        let (_publisher, status) = status_publisher_and_reader::<ap_fsm::Status>();

        let ap_iface = ApIfaceContainer {
            iface_id: TEST_AP_IFACE_ID,
            config: None,
            ap_state_machine: Box::new(FakeAp {
                start_succeeds: true,
                stop_succeeds: true,
                exit_succeeds: true,
            }),
            state_machine_task: fasync::Task::local(async {}),
            enabled_time: None,
            status,
        };
        iface_manager.aps.push(ap_iface);

        // Notify the IfaceManager that the client interface has been removed.
        {
            let fut = iface_manager.handle_removed_iface(removed_iface_id);
            let mut fut = pin!(fut);

            // Expect a DeviceMonitor request an SME proxy.
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
            assert_matches!(
                exec.run_until_stalled(&mut test_values.monitor_service_stream.next()),
                Poll::Ready(Some(Ok(fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::GetClientSme {
                    iface_id: TEST_CLIENT_IFACE_ID, sme_server: _, responder
                }))) => {
                    responder
                        .send(Err(zx::sys::ZX_ERR_NOT_FOUND))
                        .expect("failed to send client SME response.");
                }
            );

            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));
        }

        assert!(!iface_manager.aps.is_empty());

        // Verify that no new client interface was created.
        assert!(iface_manager.clients.is_empty());

        // Verify that a ConnectionsDisabled notification was sent.
        // Ensure an update was sent
        let expected_update = listener::ClientStateUpdate {
            state: fidl_fuchsia_wlan_policy::WlanClientState::ConnectionsDisabled,
            networks: vec![],
        };
        assert_matches!(
            test_values.client_update_receiver.try_next(),
            Ok(Some(listener::Message::NotifyListeners(updates))) => {
                assert_eq!(updates, expected_update);
            }
        );
    }

    #[fuchsia::test]
    fn test_client_iface_recreation_fails() {
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create an IfaceManager with a client and an AP.
        let test_values = test_setup(&mut exec);
        let (mut iface_manager, _next_sme_req) =
            create_iface_manager_with_client(&test_values, true);
        iface_manager.phy_manager = Arc::new(Mutex::new(FakePhyManager {
            create_iface_ok: false,
            destroy_iface_ok: true,
            set_country_ok: true,
            country_code: None,
            client_connections_enabled: true,
            client_ifaces: vec![TEST_CLIENT_IFACE_ID],
            defects: vec![],
            recovery_sender: None,
        }));

        let removed_iface_id = 123;
        iface_manager.clients[0].iface_id = removed_iface_id;

        // Notify the IfaceManager that the client interface has been removed.
        {
            let fut = iface_manager.handle_removed_iface(removed_iface_id);
            let mut fut = pin!(fut);

            // Verify that the future completes immediately since the interface recreation failed.
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));
        }

        // Verify that no new client interface was created.
        assert!(iface_manager.clients.is_empty());
    }

    #[fuchsia::test]
    fn test_do_not_recover_removed_ap_iface() {
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create an IfaceManager with a client and an AP.
        let test_values = test_setup(&mut exec);
        let (mut iface_manager, _next_sme_req) =
            create_iface_manager_with_client(&test_values, true);

        let removed_iface_id = 123;
        let (_publisher, status) = status_publisher_and_reader::<ap_fsm::Status>();
        let ap_iface = ApIfaceContainer {
            iface_id: removed_iface_id,
            config: None,
            ap_state_machine: Box::new(FakeAp {
                start_succeeds: true,
                stop_succeeds: true,
                exit_succeeds: true,
            }),
            state_machine_task: fasync::Task::local(async {}),
            enabled_time: None,
            status,
        };
        iface_manager.aps.push(ap_iface);

        // Notify the IfaceManager that the AP interface has been removed.
        {
            let fut = iface_manager.handle_removed_iface(removed_iface_id);
            let mut fut = pin!(fut);

            // The future should now run to completion.
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));
        }

        assert!(!iface_manager.clients.is_empty());

        // Verify that a new AP interface was not created.
        assert!(iface_manager.aps.is_empty());
    }

    #[fuchsia::test]
    fn test_remove_nonexistent_iface() {
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create an IfaceManager with a client and an AP.
        let test_values = test_setup(&mut exec);
        let (mut iface_manager, _next_sme_req) =
            create_iface_manager_with_client(&test_values, true);

        let (_publisher, status) = status_publisher_and_reader::<ap_fsm::Status>();
        let ap_iface = ApIfaceContainer {
            iface_id: TEST_AP_IFACE_ID,
            config: None,
            ap_state_machine: Box::new(FakeAp {
                start_succeeds: true,
                stop_succeeds: true,
                exit_succeeds: true,
            }),
            state_machine_task: fasync::Task::local(async {}),
            enabled_time: None,
            status,
        };
        iface_manager.aps.push(ap_iface);

        // Notify the IfaceManager that an interface that has not been accounted for has been
        // removed.
        {
            let fut = iface_manager.handle_removed_iface(1234);
            let mut fut = pin!(fut);
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));
        }

        assert!(!iface_manager.clients.is_empty());
        assert!(!iface_manager.aps.is_empty());
    }

    fn poll_service_req<
        T,
        E: std::fmt::Debug,
        R: fidl::endpoints::RequestStream
            + futures::Stream<Item = Result<T, E>>
            + futures::TryStream<Ok = T>,
    >(
        exec: &mut fuchsia_async::TestExecutor,
        next_req: &mut StreamFuture<R>,
    ) -> Poll<fidl::endpoints::Request<R::Protocol>> {
        exec.run_until_stalled(next_req).map(|(req, stream)| {
            *next_req = stream.into_future();
            req.expect("did not expect the SME request stream to end")
                .expect("error polling SME request stream")
        })
    }

    #[fuchsia::test]
    fn test_add_client_iface() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let test_values = test_setup(&mut exec);

        // Create an empty PhyManager and IfaceManager.
        let phy_manager = phy_manager::PhyManager::new(
            test_values.monitor_service_proxy.clone(),
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node.clone_weak(),
            test_values.telemetry_sender.clone(),
            test_values.recovery_sender,
        );
        let mut iface_manager = IfaceManagerService::new(
            Arc::new(Mutex::new(phy_manager)),
            test_values.client_update_sender,
            test_values.ap_update_sender,
            test_values.monitor_service_proxy,
            test_values.saved_networks,
            test_values.connection_selection_requester,
            test_values.roam_manager.clone(),
            test_values.telemetry_sender,
            test_values.defect_sender,
            test_values.node,
        );

        {
            // Notify the IfaceManager of a new interface.
            let fut = iface_manager.configure_new_iface(TEST_CLIENT_IFACE_ID);
            let mut fut = pin!(fut);
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

            // Expect and interface query and notify that this is a client interface.
            let mut monitor_service_fut = test_values.monitor_service_stream.into_future();
            assert_matches!(
                poll_service_req(&mut exec, &mut monitor_service_fut),
                Poll::Ready(fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::QueryIface {
                    iface_id: TEST_CLIENT_IFACE_ID, responder
                }) => {
                    let response = fidl_fuchsia_wlan_device_service::QueryIfaceResponse {
                        role: fidl_fuchsia_wlan_common::WlanMacRole::Client,
                        id: TEST_CLIENT_IFACE_ID,
                        phy_id: 0,
                        phy_assigned_id: 0,
                        sta_addr: [0; 6],
                        factory_addr: [0; 6],
                    };
                    responder
                        .send(Ok(&response))
                        .expect("Sending iface response");
                }
            );

            // Expect that we have requested a client SME proxy from get_client.
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
            assert_matches!(
                poll_service_req(&mut exec, &mut monitor_service_fut),
                Poll::Ready(fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::GetClientSme {
                    iface_id: TEST_CLIENT_IFACE_ID, sme_server: _, responder
                }) => {
                    // Send back a positive acknowledgement.
                    assert!(responder.send(Ok(())).is_ok());
                }
            );

            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

            // Expect that we have requested a client SME proxy from creating the client state
            // machine.
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
            assert_matches!(
                poll_service_req(&mut exec, &mut monitor_service_fut),
                Poll::Ready(fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::GetClientSme {
                    iface_id: TEST_CLIENT_IFACE_ID, sme_server: _, responder
                }) => {
                    // Send back a positive acknowledgement.
                    assert!(responder.send(Ok(())).is_ok());
                }
            );

            // Run the future to completion.
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));
        }

        // Ensure that the client interface has been added.
        assert!(iface_manager.aps.is_empty());
        assert_eq!(iface_manager.clients[0].iface_id, TEST_CLIENT_IFACE_ID);
    }

    #[fuchsia::test]
    fn test_add_ap_iface() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let test_values = test_setup(&mut exec);

        // Create an empty PhyManager and IfaceManager.
        let phy_manager = phy_manager::PhyManager::new(
            test_values.monitor_service_proxy.clone(),
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node.clone_weak(),
            test_values.telemetry_sender.clone(),
            test_values.recovery_sender,
        );
        let mut iface_manager = IfaceManagerService::new(
            Arc::new(Mutex::new(phy_manager)),
            test_values.client_update_sender,
            test_values.ap_update_sender,
            test_values.monitor_service_proxy,
            test_values.saved_networks,
            test_values.connection_selection_requester,
            test_values.roam_manager.clone(),
            test_values.telemetry_sender,
            test_values.defect_sender,
            test_values.node,
        );

        {
            // Notify the IfaceManager of a new interface.
            let fut = iface_manager.configure_new_iface(TEST_AP_IFACE_ID);
            let mut fut = pin!(fut);
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

            // Expect that the interface properties are queried and notify that it is an AP iface.
            let mut monitor_service_fut = test_values.monitor_service_stream.into_future();
            assert_matches!(
                poll_service_req(&mut exec, &mut monitor_service_fut),
                Poll::Ready(fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::QueryIface {
                    iface_id: TEST_AP_IFACE_ID, responder
                }) => {
                    let response = fidl_fuchsia_wlan_device_service::QueryIfaceResponse {
                        role: fidl_fuchsia_wlan_common::WlanMacRole::Ap,
                        id: TEST_AP_IFACE_ID,
                        phy_id: 0,
                        phy_assigned_id: 0,
                        sta_addr: [0; 6],
                        factory_addr: [0; 6],
                    };
                    responder
                        .send(Ok(&response))
                        .expect("Sending iface response");
                }
            );

            // Run the future so that an AP SME proxy is requested.
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

            let responder = assert_matches!(
                poll_service_req(&mut exec, &mut monitor_service_fut),
                Poll::Ready(fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::GetApSme {
                    iface_id: TEST_AP_IFACE_ID, sme_server: _, responder
                }) => responder
            );

            // Send back a positive acknowledgement.
            assert!(responder.send(Ok(())).is_ok());

            // Run the future to completion.
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));
        }

        // Ensure that the AP interface has been added.
        assert!(iface_manager.clients.is_empty());
        assert_eq!(iface_manager.aps[0].iface_id, TEST_AP_IFACE_ID);
    }

    #[fuchsia::test]
    fn test_add_nonexistent_iface() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let test_values = test_setup(&mut exec);

        // Create an empty PhyManager and IfaceManager.
        let phy_manager = phy_manager::PhyManager::new(
            test_values.monitor_service_proxy.clone(),
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node.clone_weak(),
            test_values.telemetry_sender.clone(),
            test_values.recovery_sender,
        );
        let mut iface_manager = IfaceManagerService::new(
            Arc::new(Mutex::new(phy_manager)),
            test_values.client_update_sender,
            test_values.ap_update_sender,
            test_values.monitor_service_proxy,
            test_values.saved_networks,
            test_values.connection_selection_requester,
            test_values.roam_manager.clone(),
            test_values.telemetry_sender,
            test_values.defect_sender,
            test_values.node,
        );

        {
            // Notify the IfaceManager of a new interface.
            let fut = iface_manager.configure_new_iface(TEST_AP_IFACE_ID);
            let mut fut = pin!(fut);
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

            // Expect an iface query and send back an error
            let mut monitor_service_fut = test_values.monitor_service_stream.into_future();
            assert_matches!(
                poll_service_req(&mut exec, &mut monitor_service_fut),
                Poll::Ready(fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::QueryIface {
                    iface_id: TEST_AP_IFACE_ID, responder
                }) => {
                    responder
                        .send(Err(zx::sys::ZX_ERR_NOT_FOUND))
                        .expect("Sending iface response");
                }
            );

            // Run the future to completion.
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));
        }

        // Ensure that no interfaces have been added.
        assert!(iface_manager.clients.is_empty());
        assert!(iface_manager.aps.is_empty());
    }

    #[fuchsia::test]
    fn test_add_existing_client_iface() {
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create a configured ClientIfaceContainer.
        let test_values = test_setup(&mut exec);
        let (mut iface_manager, _next_sme_req) =
            create_iface_manager_with_client(&test_values, true);

        // Notify the IfaceManager of a new interface.
        {
            let fut = iface_manager.configure_new_iface(TEST_CLIENT_IFACE_ID);
            let mut fut = pin!(fut);
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

            // Expect an interface query and notify that it is a client.
            let mut monitor_service_fut = test_values.monitor_service_stream.into_future();
            assert_matches!(
                poll_service_req(&mut exec, &mut monitor_service_fut),
                Poll::Ready(fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::QueryIface {
                    iface_id: TEST_CLIENT_IFACE_ID, responder
                }) => {
                    let response = fidl_fuchsia_wlan_device_service::QueryIfaceResponse {
                        role: fidl_fuchsia_wlan_common::WlanMacRole::Client,
                        id: TEST_CLIENT_IFACE_ID,
                        phy_id: 0,
                        phy_assigned_id: 0,
                        sta_addr: [0; 6],
                        factory_addr: [0; 6],
                    };
                    responder
                        .send(Ok(&response))
                        .expect("Sending iface response");
                }
            );

            // The future should then run to completion as it finds the existing interface.
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));
        }

        // Verify that nothing new has been appended to the clients vector or the aps vector.
        assert_eq!(iface_manager.clients.len(), 1);
        assert_eq!(iface_manager.aps.len(), 0);
    }

    #[fuchsia::test]
    fn test_add_existing_ap_iface() {
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create a configured ClientIfaceContainer.
        let test_values = test_setup(&mut exec);
        let fake_ap = FakeAp { start_succeeds: true, stop_succeeds: true, exit_succeeds: true };
        let mut iface_manager = create_iface_manager_with_ap(&test_values, fake_ap);

        // Notify the IfaceManager of a new interface.
        {
            let fut = iface_manager.configure_new_iface(TEST_AP_IFACE_ID);
            let mut fut = pin!(fut);
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

            // Expect an interface query and notify that it is a client.
            let mut monitor_service_fut = test_values.monitor_service_stream.into_future();
            assert_matches!(
                poll_service_req(&mut exec, &mut monitor_service_fut),
                Poll::Ready(fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::QueryIface {
                    iface_id: TEST_AP_IFACE_ID, responder
                }) => {
                    let response = fidl_fuchsia_wlan_device_service::QueryIfaceResponse {
                        role: fidl_fuchsia_wlan_common::WlanMacRole::Ap,
                        id: TEST_AP_IFACE_ID,
                        phy_id: 0,
                        phy_assigned_id: 0,
                        sta_addr: [0; 6],
                        factory_addr: [0; 6],
                    };
                    responder
                        .send(Ok(&response))
                        .expect("Sending iface response");
                }
            );

            // The future should then run to completion as it finds the existing interface.
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));
        }

        // Verify that nothing new has been appended to the clients vector or the aps vector.
        assert_eq!(iface_manager.clients.len(), 0);
        assert_eq!(iface_manager.aps.len(), 1);
    }

    enum TestType {
        Pass,
        Fail,
        ClientError,
    }

    fn run_service_test<T: std::fmt::Debug>(
        exec: &mut fuchsia_async::TestExecutor,
        iface_manager: IfaceManagerService,
        req: IfaceManagerRequest,
        mut req_receiver: oneshot::Receiver<Result<T, Error>>,
        monitor_service_stream: fidl_fuchsia_wlan_device_service::DeviceMonitorRequestStream,
        test_type: TestType,
    ) {
        // Start the service loop
        let (mut sender, receiver) = mpsc::channel(1);
        let (_defect_sender, defect_receiver) = mpsc::channel(100);
        let (_recovery_sender, recovery_receiver) =
            mpsc::channel::<recovery::RecoverySummary>(recovery::RECOVERY_SUMMARY_CHANNEL_CAPACITY);
        let serve_fut = serve_iface_manager_requests(
            iface_manager,
            receiver,
            defect_receiver,
            recovery_receiver,
        );
        let mut serve_fut = pin!(serve_fut);

        // Send the client's request
        sender.try_send(req).expect("failed to send request");

        // Service any device service requests in the event that a new client SME proxy is required
        // for the operation under test.
        let mut monitor_service_fut = monitor_service_stream.into_future();
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        match poll_service_req(exec, &mut monitor_service_fut) {
            Poll::Ready(fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::GetClientSme {
                iface_id: TEST_CLIENT_IFACE_ID,
                sme_server: _,
                responder,
            }) => {
                // Send back a positive acknowledgement.
                assert!(responder.send(Ok(())).is_ok());
            }
            Poll::Ready(fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::GetApSme {
                iface_id: TEST_AP_IFACE_ID,
                sme_server: _,
                responder,
            }) => {
                // Send back a positive acknowledgement.
                assert!(responder.send(Ok(())).is_ok());
            }
            _ => {}
        }

        match test_type {
            TestType::Pass => {
                // Process the request.
                assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

                // Assert that the receiving end gets a successful result.
                assert_matches!(exec.run_until_stalled(&mut req_receiver), Poll::Ready(Ok(Ok(_))));
            }
            TestType::Fail => {
                // Process the request.
                assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

                // Assert that the receiving end gets a successful result.
                assert_matches!(exec.run_until_stalled(&mut req_receiver), Poll::Ready(Ok(Err(_))));
            }
            TestType::ClientError => {
                // Simulate a client failure.
                drop(req_receiver);
            }
        }

        // Make sure the service keeps running.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
    }

    fn run_service_test_with_unit_return(
        exec: &mut fuchsia_async::TestExecutor,
        iface_manager: IfaceManagerService,
        req: IfaceManagerRequest,
        mut req_receiver: oneshot::Receiver<()>,
        test_type: TestType,
    ) {
        // Start the service loop
        let (mut sender, receiver) = mpsc::channel(1);
        let (_defect_sender, defect_receiver) = mpsc::channel(100);
        let (_recovery_sender, recovery_receiver) =
            mpsc::channel::<recovery::RecoverySummary>(recovery::RECOVERY_SUMMARY_CHANNEL_CAPACITY);
        let serve_fut = serve_iface_manager_requests(
            iface_manager,
            receiver,
            defect_receiver,
            recovery_receiver,
        );
        let mut serve_fut = pin!(serve_fut);

        // Send the client's request
        sender.try_send(req).expect("failed to send request");

        match test_type {
            TestType::Pass => {
                // Process the request.
                assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

                // Assert that the receiving end gets a successful result.
                assert_matches!(exec.run_until_stalled(&mut req_receiver), Poll::Ready(Ok(())));
            }
            TestType::Fail => {
                // Process the request.
                assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

                // Assert that the receiving end gets a successful result.
                assert_matches!(exec.run_until_stalled(&mut req_receiver), Poll::Ready(Ok(())));
            }
            TestType::ClientError => {
                // Simulate a client failure.
                drop(req_receiver);
            }
        }

        // Make sure the service keeps running.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
    }

    #[test_case(FakeClient {disconnect_ok: true, is_alive:true, expected_connect_selection: None},
        TestType::Pass; "successfully disconnects configured client")]
    #[test_case(FakeClient {disconnect_ok: false, is_alive:true, expected_connect_selection: None},
        TestType::Fail; "fails to disconnect configured client")]
    #[test_case(FakeClient {disconnect_ok: true, is_alive:true, expected_connect_selection: None},
        TestType::ClientError; "client drops receiver")]
    #[fuchsia::test(add_test_attr = false)]
    fn service_disconnect_test(fake_client: FakeClient, test_type: TestType) {
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create a configured ClientIfaceContainer.
        let test_values = test_setup(&mut exec);
        let (mut iface_manager, _stream) = create_iface_manager_with_client(&test_values, true);
        iface_manager.clients[0].client_state_machine = Some(Box::new(fake_client));

        // Send a disconnect request.
        let (ack_sender, ack_receiver) = oneshot::channel();
        let req = DisconnectRequest {
            network_id: client_types::NetworkIdentifier {
                ssid: TEST_SSID.clone(),
                security_type: client_types::SecurityType::Wpa,
            },
            reason: client_types::DisconnectReason::NetworkUnsaved,
            responder: ack_sender,
        };
        let req = IfaceManagerRequest::Disconnect(req);

        run_service_test(
            &mut exec,
            iface_manager,
            req,
            ack_receiver,
            test_values.monitor_service_stream,
            test_type,
        );
    }

    #[test_case(FakeClient {disconnect_ok: true, is_alive:true, expected_connect_selection: Some(generate_connect_selection())},
        TestType::Pass; "successfully connected a client")]
    #[test_case(FakeClient {disconnect_ok: true, is_alive:true, expected_connect_selection: Some(generate_connect_selection())},
        TestType::ClientError; "client drops receiver")]
    #[fuchsia::test(add_test_attr = false)]
    fn service_connect_test(fake_client: FakeClient, test_type: TestType) {
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create a configured ClientIfaceContainer.
        let test_values = test_setup(&mut exec);
        let (mut iface_manager, _stream) = create_iface_manager_with_client(&test_values, false);
        let connect_selection = fake_client.expected_connect_selection.clone().unwrap();
        iface_manager.clients[0].client_state_machine = Some(Box::new(fake_client));

        // Send a connect request.
        let (ack_sender, ack_receiver) = oneshot::channel();
        let req = ConnectRequest {
            request: ConnectAttemptRequest::new(
                connect_selection.target.network,
                connect_selection.target.credential,
                client_types::ConnectReason::FidlConnectRequest,
            ),
            responder: ack_sender,
        };
        let req = IfaceManagerRequest::Connect(req);

        // Currently the FakeScanRequester will panic if a scan is requested and no results are
        // queued, so add something. This test needs a few:
        // initial idle interface selection
        // for the connect request
        // idle interface selection after connect request fails
        exec.run_singlethreaded(test_values.scan_requester.add_scan_result(Ok(vec![])));
        exec.run_singlethreaded(test_values.scan_requester.add_scan_result(Ok(vec![])));
        exec.run_singlethreaded(test_values.scan_requester.add_scan_result(Ok(vec![])));

        run_service_test(
            &mut exec,
            iface_manager,
            req,
            ack_receiver,
            test_values.monitor_service_stream,
            test_type,
        );
    }

    // This test is a bit of a twofer as it covers both the mechanism for recording idle interfaces
    // as well as the mechanism for querying idle interfaces.
    #[fuchsia::test]
    fn test_service_record_idle_client() {
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create a configured ClientIfaceContainer.
        let test_values = test_setup(&mut exec);
        let (mut iface_manager, _stream) = create_iface_manager_with_client(&test_values, true);

        // Make the client state machine's liveness check fail.
        iface_manager.clients[0].client_state_machine = Some(Box::new(FakeClient {
            disconnect_ok: true,
            is_alive: false,
            expected_connect_selection: None,
        }));

        // Create mpsc channel to handle requests.
        let (mut sender, receiver) = mpsc::channel(1);
        let serve_fut = serve_iface_manager_requests(
            iface_manager,
            receiver,
            test_values.defect_receiver,
            test_values.recovery_receiver,
        );
        let mut serve_fut = pin!(serve_fut);

        // Send an idle interface request
        let (ack_sender, mut ack_receiver) = oneshot::channel();
        let req = RecordIdleIfaceRequest { iface_id: TEST_CLIENT_IFACE_ID, responder: ack_sender };
        let req = IfaceManagerRequest::RecordIdleIface(req);

        sender.try_send(req).expect("failed to send request");

        // Run the service loop and expect the request to be serviced and for the loop to not exit.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Wait for the response.
        assert_matches!(exec.run_until_stalled(&mut ack_receiver), Poll::Ready(Ok(())));

        // Check if an idle interface is present.
        let (idle_iface_sender, mut idle_iface_receiver) = oneshot::channel();
        let req = HasIdleIfaceRequest { responder: idle_iface_sender };
        let req = IfaceManagerRequest::HasIdleIface(req);
        sender.try_send(req).expect("failed to send request");

        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Make sure that the interface has been marked idle.
        assert_matches!(exec.run_until_stalled(&mut idle_iface_receiver), Poll::Ready(Ok(true)));
    }

    #[fuchsia::test]
    fn test_service_record_idle_client_response_failure() {
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create a configured ClientIfaceContainer.
        let test_values = test_setup(&mut exec);
        let (iface_manager, _stream) = create_iface_manager_with_client(&test_values, true);

        // Create mpsc channel to handle requests.
        let (mut sender, receiver) = mpsc::channel(1);
        let serve_fut = serve_iface_manager_requests(
            iface_manager,
            receiver,
            test_values.defect_receiver,
            test_values.recovery_receiver,
        );
        let mut serve_fut = pin!(serve_fut);

        // Send an idle interface request
        let (ack_sender, ack_receiver) = oneshot::channel();
        let req = RecordIdleIfaceRequest { iface_id: TEST_CLIENT_IFACE_ID, responder: ack_sender };
        let req = IfaceManagerRequest::RecordIdleIface(req);

        sender.try_send(req).expect("failed to send request");

        // Drop the receiving end and make sure the service continues running.
        drop(ack_receiver);

        // Run the service loop and expect the request to be serviced and for the loop to not exit.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
    }

    #[fuchsia::test]
    fn test_service_query_idle_client_response_failure() {
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create a configured ClientIfaceContainer.
        let test_values = test_setup(&mut exec);
        let (iface_manager, _stream) = create_iface_manager_with_client(&test_values, true);

        // Create mpsc channel to handle requests.
        let (mut sender, receiver) = mpsc::channel(1);
        let serve_fut = serve_iface_manager_requests(
            iface_manager,
            receiver,
            test_values.defect_receiver,
            test_values.recovery_receiver,
        );
        let mut serve_fut = pin!(serve_fut);

        // Check if an idle interface is present.
        let (idle_iface_sender, idle_iface_receiver) = oneshot::channel();
        let req = HasIdleIfaceRequest { responder: idle_iface_sender };
        let req = IfaceManagerRequest::HasIdleIface(req);
        sender.try_send(req).expect("failed to send request");

        // Drop the receiving end and make sure the service continues running.
        drop(idle_iface_receiver);

        // Run the service loop and expect the request to be serviced and for the loop to not exit.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
    }

    #[fuchsia::test]
    fn test_service_add_iface_succeeds() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let test_values = test_setup(&mut exec);

        // Create an empty PhyManager and IfaceManager.
        let phy_manager = Arc::new(Mutex::new(FakePhyManager {
            create_iface_ok: false,
            destroy_iface_ok: false,
            set_country_ok: false,
            country_code: None,
            client_connections_enabled: false,
            client_ifaces: vec![],
            defects: vec![],
            recovery_sender: None,
        }));
        let iface_manager = IfaceManagerService::new(
            phy_manager.clone(),
            test_values.client_update_sender,
            test_values.ap_update_sender,
            test_values.monitor_service_proxy,
            test_values.saved_networks.clone(),
            test_values.connection_selection_requester,
            test_values.roam_manager.clone(),
            test_values.telemetry_sender,
            test_values.defect_sender,
            test_values.node,
        );

        // Create mpsc channel to handle requests.
        let (mut sender, receiver) = mpsc::channel(1);
        let serve_fut = serve_iface_manager_requests(
            iface_manager,
            receiver,
            test_values.defect_receiver,
            test_values.recovery_receiver,
        );
        let mut serve_fut = pin!(serve_fut);

        // Report a new interface.
        let (new_iface_sender, mut new_iface_receiver) = oneshot::channel();
        let req = AddIfaceRequest { iface_id: TEST_CLIENT_IFACE_ID, responder: new_iface_sender };
        let req = IfaceManagerRequest::AddIface(req);
        sender.try_send(req).expect("failed to send request");

        // Run the service loop to begin processing the request.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Expect that the PhyManager has been notified of the interface.
        {
            let phy_manager_fut = phy_manager.lock();
            let mut phy_manager_fut = pin!(phy_manager_fut);
            assert_matches!(
                exec.run_until_stalled(&mut phy_manager_fut),
                Poll::Ready(phy_manager) => {
                    assert!(phy_manager.client_ifaces.contains(&TEST_CLIENT_IFACE_ID));
                }
            );
        }

        // Expect an interface query and notify that this is a client interface.
        let mut monitor_service_fut = test_values.monitor_service_stream.into_future();
        assert_matches!(
            poll_service_req(&mut exec, &mut monitor_service_fut),
            Poll::Ready(fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::QueryIface {
                iface_id: TEST_CLIENT_IFACE_ID, responder
            }) => {
                let response = fidl_fuchsia_wlan_device_service::QueryIfaceResponse {
                    role: fidl_fuchsia_wlan_common::WlanMacRole::Client,
                    id: TEST_CLIENT_IFACE_ID,
                    phy_id: 0,
                    phy_assigned_id: 0,
                    sta_addr: [0; 6],
                    factory_addr: [0; 6],
                };
                responder
                    .send(Ok(&response))
                    .expect("Sending iface response");
            }
        );

        // Expect that we have requested a client SME proxy from get_client.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert_matches!(
            poll_service_req(&mut exec, &mut monitor_service_fut),
            Poll::Ready(fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::GetClientSme {
                iface_id: TEST_CLIENT_IFACE_ID, sme_server: _, responder
            }) => {
                // Send back a positive acknowledgement.
                assert!(responder.send(Ok(())).is_ok());
            }
        );

        // Expect that we have requested a client SME proxy from creating the client state machine.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert_matches!(
            poll_service_req(&mut exec, &mut monitor_service_fut),
            Poll::Ready(fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::GetClientSme {
                iface_id: TEST_CLIENT_IFACE_ID, sme_server: _, responder
            }) => {
                // Send back a positive acknowledgement.
                assert!(responder.send(Ok(())).is_ok());
            }
        );

        // Run the service again to ensure the response is sent.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Check that the response was received.
        assert_matches!(exec.run_until_stalled(&mut new_iface_receiver), Poll::Ready(Ok(())));
    }

    #[test_case(TestType::Fail; "failed to add interface")]
    #[test_case(TestType::ClientError; "client dropped receiver")]
    #[fuchsia::test(add_test_attr = false)]
    fn service_add_iface_negative_tests(test_type: TestType) {
        let mut exec = fuchsia_async::TestExecutor::new();
        let test_values = test_setup(&mut exec);

        // Create an empty PhyManager and IfaceManager.
        let phy_manager = phy_manager::PhyManager::new(
            test_values.monitor_service_proxy.clone(),
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node.clone_weak(),
            test_values.telemetry_sender.clone(),
            test_values.recovery_sender,
        );
        let iface_manager = IfaceManagerService::new(
            Arc::new(Mutex::new(phy_manager)),
            test_values.client_update_sender,
            test_values.ap_update_sender,
            test_values.monitor_service_proxy,
            test_values.saved_networks.clone(),
            test_values.connection_selection_requester,
            test_values.roam_manager.clone(),
            test_values.telemetry_sender,
            test_values.defect_sender,
            test_values.node,
        );

        // Report a new interface.
        let (new_iface_sender, new_iface_receiver) = oneshot::channel();
        let req = AddIfaceRequest { iface_id: TEST_CLIENT_IFACE_ID, responder: new_iface_sender };
        let req = IfaceManagerRequest::AddIface(req);

        // Drop the device monitor stream so that querying the interface properties will fail.
        drop(test_values.monitor_service_stream);

        run_service_test_with_unit_return(
            &mut exec,
            iface_manager,
            req,
            new_iface_receiver,
            test_type,
        );
    }

    #[test_case(TestType::Pass; "successfully removed iface")]
    #[test_case(TestType::ClientError; "client dropped receiving end")]
    #[fuchsia::test(add_test_attr = false)]
    fn service_remove_iface_test(test_type: TestType) {
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create a configured ClientIfaceContainer.
        let test_values = test_setup(&mut exec);
        let (iface_manager, _stream) = create_iface_manager_with_client(&test_values, true);

        // Notify of interface removal.
        let (remove_iface_sender, remove_iface_receiver) = oneshot::channel();
        let req = RemoveIfaceRequest { iface_id: 123, responder: remove_iface_sender };
        let req = IfaceManagerRequest::RemoveIface(req);

        run_service_test_with_unit_return(
            &mut exec,
            iface_manager,
            req,
            remove_iface_receiver,
            test_type,
        );
    }

    #[test_case(
        FakePhyManager {
            create_iface_ok: true,
            destroy_iface_ok: true,
            set_country_ok: true,
            country_code: None,
            client_connections_enabled: true,
            client_ifaces: vec![],
            defects: vec![],
            recovery_sender: None,
        },
        TestType::Pass;
        "successfully started client connections"
    )]
    #[test_case(
        FakePhyManager {
            create_iface_ok: false,
            destroy_iface_ok: true,
            set_country_ok: true,
            country_code: None,
            client_connections_enabled: true,
            client_ifaces: vec![TEST_CLIENT_IFACE_ID],
            defects: vec![],
            recovery_sender: None,
        },
        TestType::Fail;
        "failed to start client connections"
    )]
    #[test_case(
        FakePhyManager {
            create_iface_ok: true,
            destroy_iface_ok: true,
            set_country_ok: true,
            country_code: None,
            client_connections_enabled: true,
            client_ifaces: vec![TEST_CLIENT_IFACE_ID],
            defects: vec![],
            recovery_sender: None,
        },
        TestType::ClientError;
        "client dropped receiver"
    )]
    #[fuchsia::test(add_test_attr = false)]
    fn service_start_client_connections_test(phy_manager: FakePhyManager, test_type: TestType) {
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create a configured ClientIfaceContainer.
        let test_values = test_setup(&mut exec);

        // Create an empty PhyManager and IfaceManager.
        let iface_manager = IfaceManagerService::new(
            Arc::new(Mutex::new(phy_manager)),
            test_values.client_update_sender,
            test_values.ap_update_sender,
            test_values.monitor_service_proxy,
            test_values.saved_networks.clone(),
            test_values.connection_selection_requester,
            test_values.roam_manager.clone(),
            test_values.telemetry_sender,
            test_values.defect_sender,
            test_values.node,
        );

        // Make start client connections request
        let (start_sender, start_receiver) = oneshot::channel();
        let req = StartClientConnectionsRequest { responder: start_sender };
        let req = IfaceManagerRequest::StartClientConnections(req);

        run_service_test(
            &mut exec,
            iface_manager,
            req,
            start_receiver,
            test_values.monitor_service_stream,
            test_type,
        );
    }

    #[test_case(
        FakePhyManager {
            create_iface_ok: true,
            destroy_iface_ok: true,
            set_country_ok: true,
            country_code: None,
            client_connections_enabled: true,
            client_ifaces: vec![TEST_CLIENT_IFACE_ID],
            defects: vec![],
            recovery_sender: None,
        },
        TestType::Pass;
        "successfully stopped client connections"
    )]
    #[test_case(
        FakePhyManager {
            create_iface_ok: true,
            destroy_iface_ok: false,
            set_country_ok: true,
            country_code: None,
            client_connections_enabled: true,
            client_ifaces: vec![TEST_CLIENT_IFACE_ID],
            defects: vec![],
            recovery_sender: None,
        },
        TestType::Fail;
        "failed to stop client connections"
    )]
    #[test_case(
        FakePhyManager {
            create_iface_ok: true,
            destroy_iface_ok: true,
            set_country_ok: true,
            country_code: None,
            client_connections_enabled: true,
            client_ifaces: vec![TEST_CLIENT_IFACE_ID],
            defects: vec![],
            recovery_sender: None,
        },
        TestType::ClientError;
        "client dropped receiver"
    )]
    #[fuchsia::test(add_test_attr = false)]
    fn service_stop_client_connections_test(phy_manager: FakePhyManager, test_type: TestType) {
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create a configured ClientIfaceContainer.
        let test_values = test_setup(&mut exec);

        // Create an empty PhyManager and IfaceManager.
        let iface_manager = IfaceManagerService::new(
            Arc::new(Mutex::new(phy_manager)),
            test_values.client_update_sender,
            test_values.ap_update_sender,
            test_values.monitor_service_proxy,
            test_values.saved_networks.clone(),
            test_values.connection_selection_requester,
            test_values.roam_manager.clone(),
            test_values.telemetry_sender,
            test_values.defect_sender,
            test_values.node,
        );

        // Make stop client connections request
        let (stop_sender, stop_receiver) = oneshot::channel();
        let req = StopClientConnectionsRequest {
            responder: stop_sender,
            reason: client_types::DisconnectReason::FidlStopClientConnectionsRequest,
        };
        let req = IfaceManagerRequest::StopClientConnections(req);

        run_service_test(
            &mut exec,
            iface_manager,
            req,
            stop_receiver,
            test_values.monitor_service_stream,
            test_type,
        );
    }

    #[test_case(FakeAp { start_succeeds: true, stop_succeeds: true, exit_succeeds: true }, TestType::Pass; "successfully starts AP")]
    #[test_case(FakeAp { start_succeeds: false, stop_succeeds: true, exit_succeeds: true }, TestType::Fail; "fails to start AP")]
    #[test_case(FakeAp { start_succeeds: true, stop_succeeds: true, exit_succeeds: true }, TestType::ClientError; "client drops receiver")]
    #[fuchsia::test(add_test_attr = false)]
    fn service_start_ap_test(fake_ap: FakeAp, test_type: TestType) {
        let mut exec = fuchsia_async::TestExecutor::new();
        let test_values = test_setup(&mut exec);

        // Create an IfaceManager with a fake AP interface.
        let iface_manager = create_iface_manager_with_ap(&test_values, fake_ap);

        // Request that an AP be started.
        let (start_sender, start_receiver) = oneshot::channel();
        let req = StartApRequest {
            config: create_ap_config(&TEST_SSID, TEST_PASSWORD),
            responder: start_sender,
        };
        let req = IfaceManagerRequest::StartAp(req);

        run_service_test(
            &mut exec,
            iface_manager,
            req,
            start_receiver,
            test_values.monitor_service_stream,
            test_type,
        );
    }

    #[test_case(FakeAp { start_succeeds: true, stop_succeeds: true, exit_succeeds: true }, TestType::Pass; "successfully stops AP")]
    #[test_case(FakeAp { start_succeeds: true, stop_succeeds: false, exit_succeeds: true }, TestType::Fail; "fails to stop AP")]
    #[test_case(FakeAp { start_succeeds: true, stop_succeeds: true, exit_succeeds: true }, TestType::ClientError; "client drops receiver")]
    #[fuchsia::test(add_test_attr = false)]
    fn service_stop_ap_test(fake_ap: FakeAp, test_type: TestType) {
        let mut exec = fuchsia_async::TestExecutor::new();
        let test_values = test_setup(&mut exec);

        // Create an IfaceManager with a configured fake AP interface.
        let mut iface_manager = create_iface_manager_with_ap(&test_values, fake_ap);
        let config = create_ap_config(&TEST_SSID, TEST_PASSWORD);
        iface_manager.aps[0].config = Some(config);

        // Request that an AP be stopped.
        let (stop_sender, stop_receiver) = oneshot::channel();
        let req = StopApRequest {
            ssid: TEST_SSID.clone(),
            password: TEST_PASSWORD.as_bytes().to_vec(),
            responder: stop_sender,
        };
        let req = IfaceManagerRequest::StopAp(req);

        run_service_test(
            &mut exec,
            iface_manager,
            req,
            stop_receiver,
            test_values.monitor_service_stream,
            test_type,
        );
    }

    #[test_case(FakeAp { start_succeeds: true, stop_succeeds: true, exit_succeeds: true }, TestType::Pass; "successfully stops AP")]
    #[test_case(FakeAp { start_succeeds: true, stop_succeeds: false, exit_succeeds: true }, TestType::Fail; "fails to stop AP")]
    #[test_case(FakeAp { start_succeeds: true, stop_succeeds: true, exit_succeeds: true }, TestType::ClientError; "client drops receiver")]
    #[fuchsia::test(add_test_attr = false)]
    fn service_stop_all_aps_test(fake_ap: FakeAp, test_type: TestType) {
        let mut exec = fuchsia_async::TestExecutor::new();
        let test_values = test_setup(&mut exec);

        // Create an IfaceManager with a fake AP interface.
        let iface_manager = create_iface_manager_with_ap(&test_values, fake_ap);

        // Request that an AP be started.
        let (stop_sender, stop_receiver) = oneshot::channel();
        let req = StopAllApsRequest { responder: stop_sender };
        let req = IfaceManagerRequest::StopAllAps(req);

        run_service_test(
            &mut exec,
            iface_manager,
            req,
            stop_receiver,
            test_values.monitor_service_stream,
            test_type,
        );
    }

    /// Tests the case where the IfaceManager attempts to reconnect a client interface that has
    /// disconnected.
    #[fuchsia::test]
    fn test_reconnect_disconnected_iface() {
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create a configured ClientIfaceContainer.
        let mut test_values = test_setup(&mut exec);
        let (mut iface_manager, _) = create_iface_manager_with_client(&test_values, true);

        // Make the client state machine report that it is dead.
        iface_manager.clients[0].client_state_machine = Some(Box::new(FakeClient {
            disconnect_ok: false,
            is_alive: false,
            expected_connect_selection: None,
        }));

        // Update the saved networks with knowledge of the test SSID and credentials.
        let connect_selection = generate_connect_selection();
        let save_network_fut = test_values.saved_networks.store(
            connect_selection.target.network.clone(),
            connect_selection.target.credential.clone(),
        );
        let mut save_network_fut = pin!(save_network_fut);
        assert_matches!(exec.run_until_stalled(&mut save_network_fut), Poll::Ready(_));

        // Ask the IfaceManager to reconnect.
        let mut sme_stream = {
            let reconnect_fut = iface_manager
                .attempt_client_reconnect(TEST_CLIENT_IFACE_ID, connect_selection.clone());
            let mut reconnect_fut = pin!(reconnect_fut);
            assert_matches!(exec.run_until_stalled(&mut reconnect_fut), Poll::Pending);

            // There should be a request for a client SME proxy.
            let mut monitor_service_fut = test_values.monitor_service_stream.into_future();
            let sme_server = assert_matches!(
                poll_service_req(&mut exec, &mut monitor_service_fut),
                Poll::Ready(fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::GetClientSme {
                    iface_id: TEST_CLIENT_IFACE_ID, sme_server, responder
                }) => {
                    assert!(responder.send(Ok(())).is_ok());
                    sme_server
                }
            );

            // The reconnect future should finish up.
            assert_matches!(exec.run_until_stalled(&mut reconnect_fut), Poll::Ready(Ok(())));

            sme_server.into_stream().into_future()
        };

        // Start running the new state machine.
        run_state_machine_futures(&mut exec, &mut iface_manager);

        // Verify telemetry event has been sent.
        let event = assert_matches!(test_values.telemetry_receiver.try_next(), Ok(Some(ev)) => ev);
        assert_matches!(
            event,
            TelemetryEvent::StartEstablishConnection { reset_start_time: false }
        );

        // Acknowledge the disconnection attempt.
        assert_matches!(
            poll_sme_req(&mut exec, &mut sme_stream),
            Poll::Ready(fidl_fuchsia_wlan_sme::ClientSmeRequest::Disconnect{ responder, reason: fidl_fuchsia_wlan_sme::UserDisconnectReason::Startup }) => {
                responder.send().expect("could not send response")
            }
        );

        // Make sure that the connect request has been sent out.
        run_state_machine_futures(&mut exec, &mut iface_manager);
        let connect_txn_handle = assert_matches!(
            poll_sme_req(&mut exec, &mut sme_stream),
            Poll::Ready(fidl_fuchsia_wlan_sme::ClientSmeRequest::Connect{ req, txn, control_handle: _ }) => {
                assert_eq!(req.ssid, connect_selection.target.network.ssid.clone());
                let (_stream, ctrl) = txn.expect("connect txn unused")
                    .into_stream_and_control_handle();
                ctrl
            }
        );
        connect_txn_handle
            .send_on_connect_result(&fake_successful_connect_result())
            .expect("failed to send connection completion");

        // Verify that the state machine future is still alive.
        run_state_machine_futures(&mut exec, &mut iface_manager);
        assert!(!iface_manager.fsm_termination_futures.is_empty());
    }

    /// Tests the case where the IfaceManager attempts to reconnect a client interface that does
    /// not exist.  This simulates the case where the client state machine exits because client
    /// connections have been stopped.
    #[fuchsia::test]
    fn test_reconnect_nonexistent_iface() {
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create an empty IfaceManager
        let test_values = test_setup(&mut exec);
        let phy_manager = create_empty_phy_manager(
            test_values.monitor_service_proxy.clone(),
            test_values.node.clone_weak(),
            test_values.telemetry_sender.clone(),
            test_values.recovery_sender,
        );
        let mut iface_manager = IfaceManagerService::new(
            phy_manager,
            test_values.client_update_sender,
            test_values.ap_update_sender,
            test_values.monitor_service_proxy,
            test_values.saved_networks.clone(),
            test_values.connection_selection_requester,
            test_values.roam_manager.clone(),
            test_values.telemetry_sender,
            test_values.defect_sender,
            test_values.node,
        );

        // Update the saved networks with knowledge of the test SSID and credentials.
        let connect_selection = generate_connect_selection();
        assert!(
            exec.run_singlethreaded(test_values.saved_networks.store(
                connect_selection.target.network.clone(),
                connect_selection.target.credential.clone()
            ))
            .expect("failed to store a network password")
            .is_none()
        );

        // Ask the IfaceManager to reconnect.
        {
            let reconnect_fut =
                iface_manager.attempt_client_reconnect(TEST_CLIENT_IFACE_ID, connect_selection);
            let mut reconnect_fut = pin!(reconnect_fut);
            assert_matches!(exec.run_until_stalled(&mut reconnect_fut), Poll::Ready(Ok(())));
        }

        // Ensure that there are no new state machines.
        assert!(iface_manager.fsm_termination_futures.is_empty());
    }

    /// Tests the case where the IfaceManager attempts to reconnect a client interface that is
    /// already connected to another networks.  This simulates the case where a client state
    /// machine exits because a new connect request has come in.
    #[fuchsia::test]
    fn test_reconnect_connected_iface() {
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create a configured ClientIfaceContainer.
        let test_values = test_setup(&mut exec);
        let (mut iface_manager, _sme_stream) = create_iface_manager_with_client(&test_values, true);

        // Make the client state machine report that it is alive.
        iface_manager.clients[0].client_state_machine = Some(Box::new(FakeClient {
            disconnect_ok: false,
            is_alive: true,
            expected_connect_selection: None,
        }));

        // Update the saved networks with knowledge of the test SSID and credentials.
        let connect_selection = generate_connect_selection();
        let save_network_fut = test_values.saved_networks.store(
            connect_selection.target.network.clone(),
            connect_selection.target.credential.clone(),
        );
        let mut save_network_fut = pin!(save_network_fut);
        assert_matches!(exec.run_until_stalled(&mut save_network_fut), Poll::Ready(_));

        // Ask the IfaceManager to reconnect.
        {
            let reconnect_fut =
                iface_manager.attempt_client_reconnect(TEST_CLIENT_IFACE_ID, connect_selection);
            let mut reconnect_fut = pin!(reconnect_fut);
            assert_matches!(exec.run_until_stalled(&mut reconnect_fut), Poll::Ready(Ok(())));
        }

        // Ensure that there are no new state machines.
        assert!(iface_manager.fsm_termination_futures.is_empty());
    }

    enum NetworkSelectionMissingAttribute {
        AllAttributesPresent,
        IdleClient,
        SavedNetwork,
        NetworkSelectionInProgress,
    }

    #[test_case(NetworkSelectionMissingAttribute::AllAttributesPresent; "scan is requested")]
    #[test_case(NetworkSelectionMissingAttribute::IdleClient; "no idle clients")]
    #[test_case(NetworkSelectionMissingAttribute::SavedNetwork; "no saved networks")]
    #[test_case(NetworkSelectionMissingAttribute::NetworkSelectionInProgress; "selection already in progress")]
    #[fuchsia::test(add_test_attr = false)]
    fn test_initiate_connection_selection(test_type: NetworkSelectionMissingAttribute) {
        // Start out by setting the test up such that we would expect a scan to be requested.
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create a configured ClientIfaceContainer.
        let mut test_values = test_setup(&mut exec);

        // Insert a saved network.
        let network_id = NetworkIdentifier::new(TEST_SSID.clone(), SecurityType::Wpa);
        let credential = Credential::Password(TEST_PASSWORD.as_bytes().to_vec());
        let save_network_fut =
            test_values.saved_networks.store(network_id.clone(), credential.clone());
        let mut save_network_fut = pin!(save_network_fut);
        assert_matches!(exec.run_until_stalled(&mut save_network_fut), Poll::Ready(_));

        // Make the client state machine report that it is not alive.
        let (mut iface_manager, _sme_stream) = create_iface_manager_with_client(&test_values, true);
        iface_manager.clients[0].client_state_machine = Some(Box::new(FakeClient {
            disconnect_ok: true,
            is_alive: false,
            expected_connect_selection: None,
        }));

        // Setup the test to prevent a network selection from happening for whatever reason was specified.
        match test_type {
            NetworkSelectionMissingAttribute::AllAttributesPresent => {
                // Currently the FakeScanRequester will panic if a scan is requested and no results
                // are queued, so add something. There may be two scans from network selection.
                exec.run_singlethreaded(test_values.scan_requester.add_scan_result(Ok(vec![])));
                exec.run_singlethreaded(test_values.scan_requester.add_scan_result(Ok(vec![])));
            }
            NetworkSelectionMissingAttribute::IdleClient => {
                // Make the client state machine report that it is alive.
                iface_manager.clients[0].client_state_machine = Some(Box::new(FakeClient {
                    disconnect_ok: true,
                    is_alive: true,
                    expected_connect_selection: None,
                }));
            }
            NetworkSelectionMissingAttribute::SavedNetwork => {
                // Remove the saved network so that there are no known networks to connect to.
                let remove_network_fut = test_values.saved_networks.remove(network_id.clone());
                let mut remove_network_fut = pin!(remove_network_fut);
                assert_matches!(exec.run_until_stalled(&mut remove_network_fut), Poll::Ready(_));
            }
            NetworkSelectionMissingAttribute::NetworkSelectionInProgress => {
                // Insert a future so that it looks like a scan is in progress.
                iface_manager.connection_selection_manager.spawn(
                    SelectionIdentifier::Automatic,
                    ready(Ok::<ConnectionSelectionResponse, anyhow::Error>(
                        ConnectionSelectionResponse::Autoconnect(None),
                    ))
                    .boxed(),
                );
            }
        }

        {
            // Run the future to completion.
            let mut fut = pin!(initiate_automatic_connection_selection(&mut iface_manager));
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));
        }

        // Verify telemetry event if the condition is right
        match test_type {
            NetworkSelectionMissingAttribute::AllAttributesPresent => {
                let event =
                    assert_matches!(test_values.telemetry_receiver.try_next(), Ok(Some(ev)) => ev);
                assert_matches!(
                    event,
                    TelemetryEvent::StartEstablishConnection { reset_start_time: false }
                );
            }
            _ => {
                assert_matches!(test_values.telemetry_receiver.try_next(), Err(_));
            }
        }

        match test_type {
            NetworkSelectionMissingAttribute::AllAttributesPresent => {
                // Run forward connection selection futures to kick them off.
                iface_manager.connection_selection_manager.get_futures().iter_mut().for_each(
                    |fut| {
                        assert_matches!(exec.run_until_stalled(fut), Poll::Pending);
                    },
                );
                // Connection selector should receive request if all attributes are present.
                assert_matches!(test_values.connection_selection_request_receiver.try_next(), Ok(Some(request)) => {
                    assert_matches!(request, ConnectionSelectionRequest::NewConnectionSelection {network_id, reason, responder} => {
                        assert_eq!(network_id, None);
                        assert_eq!(reason, client_types::ConnectReason::IdleInterfaceAutoconnect);
                        responder.send(Some(generate_random_scanned_candidate())).expect("failed to send selection");
                    });
                });
            }
            _ => {
                // No connection selection request should be sent.
                assert_matches!(
                    test_values.connection_selection_request_receiver.try_next(),
                    Err(_)
                );
            }
        }

        // Run all connection_selection futures to completion.
        for mut connection_selection_future in
            iface_manager.connection_selection_manager.get_futures().iter_mut()
        {
            assert_matches!(
                exec.run_until_stalled(&mut connection_selection_future),
                Poll::Ready(_)
            );
        }
    }

    #[fuchsia::test]
    fn test_scan_result_backoff() {
        let mut exec = fuchsia_async::TestExecutor::new();

        // Create an IfaceManagerService
        let test_values = test_setup(&mut exec);
        let (mut iface_manager, _stream) = create_iface_manager_with_client(&test_values, true);

        // Create a timer to periodically check to ensure that all client interfaces are connected.
        let mut reconnect_monitor_interval: i64 = 1;
        let mut connectivity_monitor_timer =
            fasync::Interval::new(zx::MonotonicDuration::from_seconds(reconnect_monitor_interval));

        // Simulate multiple failed scan attempts and ensure that the timer interval backs off as
        // expected.
        let expected_wait_times =
            [2, 4, 8, MAX_AUTO_CONNECT_RETRY_SECONDS, MAX_AUTO_CONNECT_RETRY_SECONDS];

        #[allow(clippy::needless_range_loop, reason = "mass allow for https://fxbug.dev/381896734")]
        for i in 0..5 {
            {
                let fut = handle_automatic_connection_selection_results(
                    None,
                    &mut iface_manager,
                    &mut reconnect_monitor_interval,
                    &mut connectivity_monitor_timer,
                );
                let mut fut = pin!(fut);
                assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));
            }
            assert_eq!(reconnect_monitor_interval, expected_wait_times[i]);
        }
    }

    #[fuchsia::test]
    fn test_reconnect_on_connection_selection_results() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let test_values = test_setup(&mut exec);

        // Create an interface manager with an unconfigured client interface.
        let (mut iface_manager, _sme_stream) =
            create_iface_manager_with_client(&test_values, false);
        iface_manager.clients[0].client_state_machine = Some(Box::new(FakeClient {
            disconnect_ok: true,
            is_alive: false,
            expected_connect_selection: None,
        }));

        // Setup for a reconnection attempt.
        let mut reconnect_monitor_interval = 1;
        let mut connectivity_monitor_timer =
            fasync::Interval::new(zx::MonotonicDuration::from_seconds(reconnect_monitor_interval));

        // Create a candidate network.
        let ssid = TEST_SSID.clone();
        let network_id = NetworkIdentifier::new(ssid.clone(), SecurityType::Wpa);
        let network = Some(client_types::ScannedCandidate {
            network: network_id,
            ..generate_random_scanned_candidate()
        });

        {
            // Run reconnection attempt
            let fut = handle_automatic_connection_selection_results(
                network,
                &mut iface_manager,
                &mut reconnect_monitor_interval,
                &mut connectivity_monitor_timer,
            );

            let mut fut = pin!(fut);

            // Expect a client SME proxy request
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
            let mut monitor_service_fut = test_values.monitor_service_stream.into_future();
            assert_matches!(
                poll_service_req(&mut exec, &mut monitor_service_fut),
                Poll::Ready(fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::GetClientSme {
                    iface_id: TEST_CLIENT_IFACE_ID, sme_server, responder
                }) => {
                    // Send back a positive acknowledgement.
                    assert!(responder.send(Ok(())).is_ok());

                    sme_server
                }
            );

            // The future should then complete.
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));
        }

        // The reconnect attempt should have seen an idle client interface and created a new client
        // state machine future for it.
        assert!(!iface_manager.fsm_termination_futures.is_empty());

        // There should not be any idle clients.
        assert!(iface_manager.idle_clients().is_empty());
    }

    #[fuchsia::test]
    fn test_idle_client_remains_after_failed_reconnection() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let test_values = test_setup(&mut exec);

        // Create an interface manager with an unconfigured client interface.
        let (mut iface_manager, _sme_stream) =
            create_iface_manager_with_client(&test_values, false);
        iface_manager.clients[0].client_state_machine = Some(Box::new(FakeClient {
            disconnect_ok: true,
            is_alive: false,
            expected_connect_selection: None,
        }));

        // Setup for a reconnection attempt.
        let mut reconnect_monitor_interval = 1;
        let mut connectivity_monitor_timer =
            fasync::Interval::new(zx::MonotonicDuration::from_seconds(reconnect_monitor_interval));

        // Create a candidate network.
        let ssid = TEST_SSID.clone();
        let network_id = NetworkIdentifier::new(ssid.clone(), SecurityType::Wpa);
        let network = Some(client_types::ScannedCandidate {
            network: network_id,
            ..generate_random_scanned_candidate()
        });

        {
            // Run reconnection attempt
            let fut = handle_automatic_connection_selection_results(
                network,
                &mut iface_manager,
                &mut reconnect_monitor_interval,
                &mut connectivity_monitor_timer,
            );

            let mut fut = pin!(fut);

            // Drop the device service stream so that the SME request fails.
            drop(test_values.monitor_service_stream);

            // The future should then complete.
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));
        }

        // There should still be an idle client
        assert!(!iface_manager.idle_clients().is_empty());
    }

    #[fuchsia::test]
    fn test_handle_bss_selection_for_connect_request_results() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let test_values = test_setup(&mut exec);

        // Create an interface manager with an unconfigured client interface.
        let (mut iface_manager, _sme_stream) =
            create_iface_manager_with_client(&test_values, false);

        // Create a request
        let mut connect_selection = generate_connect_selection();
        connect_selection.reason = client_types::ConnectReason::FidlConnectRequest;
        let request = ConnectAttemptRequest::new(
            connect_selection.target.network.clone(),
            connect_selection.target.credential.clone(),
            client_types::ConnectReason::FidlConnectRequest,
        );

        iface_manager.clients[0].client_state_machine = Some(Box::new(FakeClient {
            disconnect_ok: true,
            is_alive: true,
            expected_connect_selection: Some(connect_selection.clone()),
        }));

        assert!(!iface_manager.idle_clients().is_empty());

        {
            let fut = handle_connection_selection_for_connect_request_results(
                request,
                Some(connect_selection.target.clone()),
                &mut iface_manager,
            );
            let mut fut = pin!(fut);

            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));
        }

        // There should not be any idle clients.
        assert!(iface_manager.idle_clients().is_empty());
        assert_eq!(
            iface_manager.clients[0].config,
            ClientIfaceContainerConfig::Configured(connect_selection.target.network.clone())
        );
    }

    #[fuchsia::test]
    fn test_terminated_client() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let test_values = test_setup(&mut exec);

        // Create a fake network entry so that a reconnect will be attempted.
        let network_id = NetworkIdentifier::new(TEST_SSID.clone(), SecurityType::Wpa);
        let credential = Credential::Password(TEST_PASSWORD.as_bytes().to_vec());

        // Update the saved networks with knowledge of the test SSID and credentials.
        {
            let save_network_fut =
                test_values.saved_networks.store(network_id.clone(), credential.clone());
            let mut save_network_fut = pin!(save_network_fut);
            assert_matches!(exec.run_until_stalled(&mut save_network_fut), Poll::Ready(_));
        }

        // Create an interface manager with an unconfigured client interface.
        let (mut iface_manager, _sme_stream) =
            create_iface_manager_with_client(&test_values, false);
        iface_manager.clients[0].client_state_machine = Some(Box::new(FakeClient {
            disconnect_ok: true,
            is_alive: false,
            expected_connect_selection: None,
        }));

        // Create remaining boilerplate to call handle_terminated_state_machine.
        let metadata = StateMachineMetadata {
            role: fidl_fuchsia_wlan_common::WlanMacRole::Client,
            iface_id: TEST_CLIENT_IFACE_ID,
        };

        {
            let mut fut = pin!(handle_terminated_state_machine(metadata, &mut iface_manager));
            assert_eq!(exec.run_until_stalled(&mut fut), Poll::Ready(()));
        }

        // Verify that there is a disconnected client.
        assert!(iface_manager.idle_clients().contains(&TEST_CLIENT_IFACE_ID));

        // Verify that a scan has been kicked off.
        assert!(!iface_manager.connection_selection_manager.active_selections().is_empty());
    }

    #[fuchsia::test]
    fn test_terminated_ap() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let test_values = test_setup(&mut exec);

        // Create an interface manager with an unconfigured client interface.
        let (mut iface_manager, _next_sme_req) =
            create_iface_manager_with_client(&test_values, true);

        // Create remaining boilerplate to call handle_terminated_state_machine.
        let metadata = StateMachineMetadata {
            role: fidl_fuchsia_wlan_common::WlanMacRole::Ap,
            iface_id: TEST_AP_IFACE_ID,
        };

        {
            let mut fut = pin!(handle_terminated_state_machine(metadata, &mut iface_manager));
            assert_eq!(exec.run_until_stalled(&mut fut), Poll::Ready(()));
        }

        // Verify that the IfaceManagerService does not have an AP interface.
        assert!(iface_manager.aps.is_empty());
    }

    /// Tests the operation of setting the country code.  The test cases of interest are:
    /// 1. Client connections and APs are initially enabled and all operations succeed.
    /// 2. Client connections are initially enabled and all operations succeed except restarting
    ///    client connections.
    ///    * In this scenario, the country code has been properly applied and the device should be
    ///      allowed to continue running.  Higher layers can optionally re-enable client
    ///      connections to recover.
    /// 3. APs are initially enabled and all operations succeed except restarting the AP.
    ///    * As in the above scenario, the device can be allowed to continue running and the API
    ///      client can attempt to restart the AP manually.
    /// 4. Stop client connections fails.
    /// 5. Stop all APs fails.
    /// 6. Set country fails.
    #[test_case(
        true, true, true, true, true, TestType::Pass;
        "client and AP enabled and all operations succeed"
    )]
    #[test_case(
        true, false, true, true, false, TestType::Pass;
        "client enabled and restarting client fails"
    )]
    #[test_case(
        false, true, true, true, false, TestType::Pass;
        "AP enabled and start AP fails after setting country"
    )]
    #[test_case(
        true, false, false, true, true, TestType::Fail;
        "stop client connections fails"
    )]
    #[test_case(
        false, true, false, true, true, TestType::Fail;
        "stop APs fails"
    )]
    #[test_case(
        false, true, true, false, true, TestType::Fail;
        "set country fails"
    )]
    #[fuchsia::test(add_test_attr = false)]
    fn set_country_service_test(
        client_connections_enabled: bool,
        ap_enabled: bool,
        destroy_iface_ok: bool,
        set_country_ok: bool,
        create_iface_ok: bool,
        test_type: TestType,
    ) {
        let mut exec = fuchsia_async::TestExecutor::new();
        let test_values = test_setup(&mut exec);

        // Seed a FakePhyManager with the test configuration information and provide the PhyManager
        // to a new IfaceManagerService to test the behavior given the configuration.
        let phy_manager = Arc::new(Mutex::new(FakePhyManager {
            create_iface_ok,
            destroy_iface_ok,
            set_country_ok,
            country_code: None,
            client_connections_enabled,
            client_ifaces: vec![],
            defects: vec![],
            recovery_sender: None,
        }));

        let mut iface_manager = IfaceManagerService::new(
            phy_manager.clone(),
            test_values.client_update_sender.clone(),
            test_values.ap_update_sender.clone(),
            test_values.monitor_service_proxy.clone(),
            test_values.saved_networks.clone(),
            test_values.connection_selection_requester.clone(),
            test_values.roam_manager.clone(),
            test_values.telemetry_sender.clone(),
            test_values.defect_sender,
            test_values.node,
        );

        // If the test calls for it, create an AP interface to test that the IfaceManager preserves
        // the configuration and restores it after setting the country code.
        if ap_enabled {
            let fake_ap = FakeAp { start_succeeds: true, stop_succeeds: true, exit_succeeds: true };
            let (_publisher, status) = status_publisher_and_reader::<ap_fsm::Status>();
            let ap_container = ApIfaceContainer {
                iface_id: TEST_AP_IFACE_ID,
                config: Some(create_ap_config(&TEST_SSID, TEST_PASSWORD)),
                ap_state_machine: Box::new(fake_ap),
                state_machine_task: fasync::Task::local(async {}),
                enabled_time: None,
                status,
            };
            iface_manager.aps.push(ap_container);
        }

        // Call set_country and drive the operation to completion.
        let (set_country_sender, set_country_receiver) = oneshot::channel();
        let req = SetCountryRequest {
            country_code: Some("WW".parse().unwrap()),
            responder: set_country_sender,
        };
        let req = IfaceManagerRequest::SetCountry(req);

        run_service_test(
            &mut exec,
            iface_manager,
            req,
            set_country_receiver,
            test_values.monitor_service_stream,
            test_type,
        );

        if destroy_iface_ok && set_country_ok {
            let phy_manager_fut = phy_manager.lock();
            let mut phy_manager_fut = pin!(phy_manager_fut);
            assert_matches!(
                exec.run_until_stalled(&mut phy_manager_fut),
                Poll::Ready(phy_manager) => {
                    assert_eq!(phy_manager.country_code, Some("WW".parse().unwrap()))
                }
            );
        }
    }

    fn fake_successful_connect_result() -> fidl_fuchsia_wlan_sme::ConnectResult {
        fidl_fuchsia_wlan_sme::ConnectResult {
            code: fidl_fuchsia_wlan_ieee80211::StatusCode::Success,
            is_credential_rejected: false,
            is_reconnect: false,
        }
    }

    /// Ensure that defect reports are passed through to the PhyManager.
    #[fuchsia::test]
    fn test_record_defect() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let phy_manager = Arc::new(Mutex::new(FakePhyManager {
            create_iface_ok: true,
            destroy_iface_ok: true,
            set_country_ok: true,
            country_code: None,
            client_connections_enabled: true,
            client_ifaces: vec![],
            defects: vec![],
            recovery_sender: None,
        }));

        {
            let mut defect_fut = initiate_record_defect(
                phy_manager.clone(),
                Defect::Phy(PhyFailure::IfaceCreationFailure { phy_id: 2 }),
            );

            // The future should complete immediately.
            assert_matches!(exec.run_until_stalled(&mut defect_fut), Poll::Ready(()));
        }

        // Verify that the defect has been recorded.
        let phy_manager_fut = phy_manager.lock();
        let mut phy_manager_fut = pin!(phy_manager_fut);
        assert_matches!(
            exec.run_until_stalled(&mut phy_manager_fut),
            Poll::Ready(phy_manager) => {
                assert_eq!(phy_manager.defects, vec![Defect::Phy(PhyFailure::IfaceCreationFailure {phy_id: 2})])
            }
        );
    }

    /// Ensure that state machine defects are passed through to the PhyManager.
    #[fuchsia::test]
    fn test_record_state_machine_defect() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let mut test_values = test_setup(&mut exec);
        let phy_manager = Arc::new(Mutex::new(FakePhyManager {
            create_iface_ok: true,
            destroy_iface_ok: true,
            set_country_ok: true,
            country_code: None,
            client_connections_enabled: true,
            client_ifaces: vec![],
            defects: vec![],
            recovery_sender: None,
        }));
        let iface_manager = IfaceManagerService::new(
            phy_manager.clone(),
            test_values.client_update_sender.clone(),
            test_values.ap_update_sender.clone(),
            test_values.monitor_service_proxy.clone(),
            test_values.saved_networks.clone(),
            test_values.connection_selection_requester.clone(),
            test_values.roam_manager.clone(),
            test_values.telemetry_sender.clone(),
            test_values.defect_sender.clone(),
            test_values.node,
        );

        // Send a defect to the IfaceManager service loop.
        test_values
            .defect_sender
            .try_send(Defect::Iface(IfaceFailure::ApStartFailure { iface_id: 0 }))
            .expect("failed to send defect notification");

        // Run the IfaceManager service so that it can process the defect.
        let (_, receiver) = mpsc::channel(0);
        let serve_fut = serve_iface_manager_requests(
            iface_manager,
            receiver,
            test_values.defect_receiver,
            test_values.recovery_receiver,
        );
        let mut serve_fut = pin!(serve_fut);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Verify that the defect has been recorded.
        let phy_manager_fut = phy_manager.lock();
        let mut phy_manager_fut = pin!(phy_manager_fut);
        assert_matches!(
            exec.run_until_stalled(&mut phy_manager_fut),
            Poll::Ready(phy_manager) => {
                assert_eq!(phy_manager.defects, vec![Defect::Iface(IfaceFailure::ApStartFailure {iface_id: 0})])
            }
        );
    }

    /// Verify that recovery actions are acknowledged by the IfaceManager service loop.
    #[fuchsia::test]
    fn test_receive_recovery_summaries() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let mut test_values = test_setup(&mut exec);
        let (recovery_sender, mut recovery_receiver) = mpsc::channel(100);
        let phy_manager = Arc::new(Mutex::new(FakePhyManager {
            create_iface_ok: true,
            destroy_iface_ok: true,
            set_country_ok: true,
            country_code: None,
            client_connections_enabled: true,
            client_ifaces: vec![],
            defects: vec![],
            recovery_sender: Some(recovery_sender),
        }));
        let iface_manager = IfaceManagerService::new(
            phy_manager.clone(),
            test_values.client_update_sender.clone(),
            test_values.ap_update_sender.clone(),
            test_values.monitor_service_proxy.clone(),
            test_values.saved_networks.clone(),
            test_values.connection_selection_requester.clone(),
            test_values.roam_manager.clone(),
            test_values.telemetry_sender.clone(),
            test_values.defect_sender.clone(),
            test_values.node,
        );

        // Send a recovery summary to the IfaceManager service loop.
        let defect = Defect::Iface(IfaceFailure::ApStartFailure { iface_id: 0 });
        let action =
            recovery::RecoveryAction::PhyRecovery(recovery::PhyRecoveryOperation::ResetPhy {
                phy_id: 0,
            });
        test_values
            .recovery_sender
            .try_send(recovery::RecoverySummary { defect, action })
            .expect("failed to send recovery summary");

        // Run the IfaceManager service so that it can process the recovery summary.
        let (_, receiver) = mpsc::channel(0);
        let serve_fut = serve_iface_manager_requests(
            iface_manager,
            receiver,
            test_values.defect_receiver,
            test_values.recovery_receiver,
        );
        let mut serve_fut = pin!(serve_fut);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Verify that the recovery summary has been recorded.
        let next_message = recovery_receiver.next();
        let mut next_message = pin!(next_message);
        assert_matches!(
            exec.run_until_stalled(&mut next_message),
            Poll::Ready(Some((summary, responder))) => {
                assert_eq!(summary, recovery::RecoverySummary { defect, action});
                responder.send(()).expect("failed to send recovery response");
            }
        );
    }

    #[fuchsia::test]
    fn test_iface_manager_requests_not_processed_while_recovering() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let mut test_values = test_setup(&mut exec);
        let (recovery_sender, mut recovery_receiver) = mpsc::channel(100);
        let phy_manager = Arc::new(Mutex::new(FakePhyManager {
            create_iface_ok: true,
            destroy_iface_ok: true,
            set_country_ok: true,
            country_code: None,
            client_connections_enabled: true,
            client_ifaces: vec![],
            defects: vec![],
            recovery_sender: Some(recovery_sender),
        }));
        let iface_manager = IfaceManagerService::new(
            phy_manager.clone(),
            test_values.client_update_sender.clone(),
            test_values.ap_update_sender.clone(),
            test_values.monitor_service_proxy.clone(),
            test_values.saved_networks.clone(),
            test_values.connection_selection_requester.clone(),
            test_values.roam_manager.clone(),
            test_values.telemetry_sender.clone(),
            test_values.defect_sender.clone(),
            test_values.node,
        );

        // Send an AP start failure + reset PHY recovery summary to the IfaceManager service loop.
        let defect = Defect::Iface(IfaceFailure::ApStartFailure { iface_id: 0 });
        let action =
            recovery::RecoveryAction::PhyRecovery(recovery::PhyRecoveryOperation::ResetPhy {
                phy_id: 0,
            });
        test_values
            .recovery_sender
            .try_send(recovery::RecoverySummary { defect, action })
            .expect("failed to send recovery summary");

        // Run the IfaceManager service so that it can process the recovery summary.
        let (mut iface_manager_sender, iface_manager_receiver) = mpsc::channel(0);
        let serve_fut = serve_iface_manager_requests(
            iface_manager,
            iface_manager_receiver,
            test_values.defect_receiver,
            test_values.recovery_receiver,
        );
        let mut serve_fut = pin!(serve_fut);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Verify that the recovery summary has been observed.  Hold onto the responder so that the
        // IfaceManager service loop stalls waiting for a reply.  This is used to demonstrate that
        // new IfaceManager requests are not processed.
        let next_message = recovery_receiver.next();
        let mut next_message = pin!(next_message);
        let responder = assert_matches!(
            exec.run_until_stalled(&mut next_message),
            Poll::Ready(Some((summary, responder))) => {
                assert_eq!(summary, recovery::RecoverySummary { defect, action});
                responder
            }
        );

        // Request that an AP be stopped.  There aren't any fake APs setup for this test, so this
        // request would ordinarily finish immediately.  In this case, the processing of this
        // request should be delayed since recovery is in progress.
        let (stop_sender, mut stop_receiver) = oneshot::channel();
        let req = StopAllApsRequest { responder: stop_sender };
        let req = IfaceManagerRequest::StopAllAps(req);
        iface_manager_sender.try_send(req).expect("failed to make stop all APs request");

        // Run the service loop and verify that the stop all APs request has not been acknowledged
        // yet.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert_matches!(stop_receiver.try_recv(), Ok(None));

        // Complete the recovery process and observe that the stop all APs request completes.
        responder.send(()).expect("failed to ack recovery");
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert_matches!(stop_receiver.try_recv(), Ok(Some(Ok(()))));
    }

    #[fuchsia::test]
    fn test_one_recovery_action_processed_at_a_time() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let mut test_values = test_setup(&mut exec);
        let (recovery_sender, mut recovery_receiver) = mpsc::channel(100);
        let phy_manager = Arc::new(Mutex::new(FakePhyManager {
            create_iface_ok: true,
            destroy_iface_ok: true,
            set_country_ok: true,
            country_code: None,
            client_connections_enabled: true,
            client_ifaces: vec![],
            defects: vec![],
            recovery_sender: Some(recovery_sender),
        }));
        let iface_manager = IfaceManagerService::new(
            phy_manager.clone(),
            test_values.client_update_sender.clone(),
            test_values.ap_update_sender.clone(),
            test_values.monitor_service_proxy.clone(),
            test_values.saved_networks.clone(),
            test_values.connection_selection_requester.clone(),
            test_values.roam_manager.clone(),
            test_values.telemetry_sender.clone(),
            test_values.defect_sender.clone(),
            test_values.node,
        );

        // Send an AP start failure + reset PHY recovery summary to the IfaceManager service loop.
        let defect = Defect::Iface(IfaceFailure::ApStartFailure { iface_id: 0 });
        let action =
            recovery::RecoveryAction::PhyRecovery(recovery::PhyRecoveryOperation::ResetPhy {
                phy_id: 0,
            });
        test_values
            .recovery_sender
            .try_send(recovery::RecoverySummary { defect, action })
            .expect("failed to send recovery summary");

        // Run the IfaceManager service so that it can process the recovery summary.
        let (_, receiver) = mpsc::channel(0);
        let serve_fut = serve_iface_manager_requests(
            iface_manager,
            receiver,
            test_values.defect_receiver,
            test_values.recovery_receiver,
        );
        let mut serve_fut = pin!(serve_fut);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Verify that the recovery summary has been observed.  Hold onto the responder so that the
        // IfaceManager service loop stalls waiting for a reply.  This is used to demonstrate that
        // more recovery requests are not processed while the current recovery attempt is in
        // progress.
        let next_message = recovery_receiver.next();
        let mut next_message = pin!(next_message);
        let responder = assert_matches!(
            exec.run_until_stalled(&mut next_message),
            Poll::Ready(Some((summary, responder))) => {
                assert_eq!(summary, recovery::RecoverySummary { defect, action});
                responder
            }
        );

        // Now send a connect failure + destroy iface recovery summary
        let defect = Defect::Iface(IfaceFailure::ConnectionFailure { iface_id: 0 });
        let action =
            recovery::RecoveryAction::PhyRecovery(recovery::PhyRecoveryOperation::DestroyIface {
                iface_id: 0,
            });
        test_values
            .recovery_sender
            .try_send(recovery::RecoverySummary { defect, action })
            .expect("failed to send recovery summary");

        // Progress the service loop future and verify that the new summary is not received.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        let next_message = recovery_receiver.next();
        let mut next_message = pin!(next_message);
        assert_matches!(exec.run_until_stalled(&mut next_message), Poll::Pending);

        // Complete the initial recovery event and verify that the next recovery summary is sent.
        responder.send(()).expect("Failed to send completion event from oneshot");
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut next_message),
            Poll::Ready(Some((summary, _))) => {
                assert_eq!(summary, recovery::RecoverySummary { defect, action});
            }
        );
    }

    #[fuchsia::test]
    fn test_recovery_inspect_logging() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let mut test_values = test_setup(&mut exec);
        let phy_manager = Arc::new(Mutex::new(FakePhyManager {
            create_iface_ok: true,
            destroy_iface_ok: true,
            set_country_ok: true,
            country_code: None,
            client_connections_enabled: true,
            client_ifaces: vec![],
            defects: vec![],
            recovery_sender: None,
        }));
        let mut iface_manager = IfaceManagerService::new(
            phy_manager.clone(),
            test_values.client_update_sender.clone(),
            test_values.ap_update_sender.clone(),
            test_values.monitor_service_proxy.clone(),
            test_values.saved_networks.clone(),
            test_values.connection_selection_requester.clone(),
            test_values.roam_manager.clone(),
            test_values.telemetry_sender.clone(),
            test_values.defect_sender.clone(),
            test_values.node.clone_weak(),
        );

        // Set up a fake client and fake AP and write some fake statuses for them.
        let (sme_proxy, _server) = create_proxy::<fidl_fuchsia_wlan_sme::ClientSmeMarker>();
        let sme_proxy = SmeForClientStateMachine::new(
            sme_proxy,
            TEST_CLIENT_IFACE_ID,
            test_values.defect_sender.clone(),
        );
        let (client_publisher, status) = status_publisher_and_reader::<client_fsm::Status>();
        let client_status = client_fsm::Status::Connected { channel: 1, rssi: 2, snr: 3 };
        client_publisher.publish_status(client_status.clone());
        let client_container = ClientIfaceContainer {
            iface_id: TEST_CLIENT_IFACE_ID,
            sme_proxy,
            config: ClientIfaceContainerConfig::Unconfigured,
            client_state_machine: None,
            state_machine_task: None,
            last_roam_time: fasync::MonotonicInstant::now(),
            status,
        };

        iface_manager.clients.push(client_container);

        let fake_ap = FakeAp { start_succeeds: true, stop_succeeds: true, exit_succeeds: true };
        let (ap_publisher, status) = status_publisher_and_reader::<ap_fsm::Status>();
        let ap_status = ap_fsm::Status::Starting;
        ap_publisher.publish_status(ap_status.clone());
        let ap_container = ApIfaceContainer {
            iface_id: TEST_AP_IFACE_ID,
            config: None,
            ap_state_machine: Box::new(fake_ap),
            state_machine_task: fasync::Task::local(async {}),
            enabled_time: None,
            status,
        };

        iface_manager.aps.push(ap_container);

        // Send an AP start failure + reset PHY recovery summary to the IfaceManager service loop.
        let defect = Defect::Iface(IfaceFailure::ApStartFailure { iface_id: 0 });
        let action =
            recovery::RecoveryAction::PhyRecovery(recovery::PhyRecoveryOperation::ResetPhy {
                phy_id: 0,
            });
        test_values
            .recovery_sender
            .try_send(recovery::RecoverySummary { defect, action })
            .expect("failed to send recovery summary");

        // Run the IfaceManager service so that it can process the recovery summary.
        let (_, receiver) = mpsc::channel(0);
        let serve_fut = serve_iface_manager_requests(
            iface_manager,
            receiver,
            test_values.defect_receiver,
            test_values.recovery_receiver,
        );
        let mut serve_fut = pin!(serve_fut);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Verify that the inspect data was written as a part of the recovery process.
        let read_fut = reader::read(&test_values.inspector);
        let mut read_fut = pin!(read_fut);
        assert_matches!(exec.run_until_stalled(&mut read_fut), Poll::Ready(Ok(hierarchy)) => {
            assert_data_tree!(@executor exec, hierarchy, root: contains {
                node: contains {
                    recovery_record: contains {
                        "0": contains {
                            summary: contains {
                                defect: contains {
                                    ApStartFailure: { iface_id: 0_u64 }
                                },
                                action: contains {
                                    ResetPhy: { phy_id: 0_u64 }
                                }
                            },
                            clients: contains {
                                "0": contains {
                                    id: 0_u64,
                                    status: contains {
                                        Connected: { channel: 1_u64, rssi: 2_i64, snr: 3_i64 }
                                    }
                                }
                            },
                            aps: contains {
                                "0": contains {
                                    id: 1_u64,
                                    status: "Starting"
                                }
                            },
                        }
                    }
                }
            });
        });
    }

    #[fuchsia::test]
    fn test_disconnect_update_on_stopped_connection() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let mut test_values = test_setup(&mut exec);

        // Set up iface manager with an unconfigured client.
        let (mut iface_manager, _stream) = create_iface_manager_with_client(&test_values, false);

        // Send connect request to iface manager.
        let connect_selection = generate_connect_selection();
        let connect_req = ConnectAttemptRequest::new(
            connect_selection.target.network.clone(),
            connect_selection.target.credential,
            client_types::ConnectReason::FidlConnectRequest,
        );
        {
            let connect_fut = iface_manager.handle_connect_request(connect_req);
            let mut connect_fut = pin!(connect_fut);
            assert_matches!(exec.run_until_stalled(&mut connect_fut), Poll::Ready(Ok(())));
        }

        // Expect a "Connecting" update to be sent.
        let connecting_state_update = listener::ClientStateUpdate {
            state: fidl_fuchsia_wlan_policy::WlanClientState::ConnectionsEnabled,
            networks: vec![listener::ClientNetworkState {
                id: connect_selection.target.network.clone(),
                state: client_types::ConnectionState::Connecting,
                status: None,
            }],
        };
        assert_matches!(
            test_values.client_update_receiver.try_next(),
            Ok(Some(listener::Message::NotifyListeners(updates))) => {
            assert_eq!(updates, connecting_state_update);
        });

        // Send a disconnect request to iface manager.
        {
            let disconnect_fut = iface_manager.disconnect(
                connect_selection.target.network.clone(),
                client_types::DisconnectReason::NetworkUnsaved,
            );
            let mut disconnect_fut = pin!(disconnect_fut);
            assert_matches!(exec.run_until_stalled(&mut disconnect_fut), Poll::Ready(Ok(())));
        }

        // Expect a "Disconnected" update to be sent, with status "ConnectionStopped".
        let disconnected_state_update = listener::ClientStateUpdate {
            state: fidl_fuchsia_wlan_policy::WlanClientState::ConnectionsEnabled,
            networks: vec![listener::ClientNetworkState {
                id: connect_selection.target.network.clone(),
                state: client_types::ConnectionState::Disconnected,
                status: Some(client_types::DisconnectStatus::ConnectionStopped),
            }],
        };
        assert_matches!(
            test_values.client_update_receiver.try_next(),
            Ok(Some(listener::Message::NotifyListeners(updates))) => {
            assert_eq!(updates, disconnected_state_update);
        });
    }
}
