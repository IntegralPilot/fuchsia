// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Serves Client policy services.
use crate::client::types as client_types;
use crate::config_management::{
    self, Credential, NetworkConfigError, NetworkIdentifier, SaveError, SavedNetworksManagerApi,
};
use crate::mode_management::iface_manager_api::{ConnectAttemptRequest, IfaceManagerApi};
use crate::telemetry::{TelemetryEvent, TelemetrySender};
use crate::util::listener;
use fidl::epitaph::ChannelEpitaphExt;
use fidl_fuchsia_wlan_policy as fidl_policy;
use futures::lock::{Mutex, MutexGuard};
use futures::prelude::*;
use futures::select;
use futures::stream::FuturesUnordered;
use log::{error, info, warn};
use std::sync::Arc;

pub mod connection_selection;
pub mod roaming;
pub mod scan;
pub mod state_machine;
pub mod types;

/// Max number of network configs that we will send at once through the network config iterator
/// in get_saved_networks. This depends on the maximum size of a FIDL NetworkConfig, so it may
/// need to change if a FIDL NetworkConfig or FIDL Credential changes.
const MAX_CONFIGS_PER_RESPONSE: usize = 100;

// This number was chosen arbitrarily.
const MAX_CONCURRENT_LISTENERS: usize = 1000;

type ClientRequests = fidl::endpoints::ServerEnd<fidl_policy::ClientControllerMarker>;
type SavedNetworksPtr = Arc<dyn SavedNetworksManagerApi>;

/// Serves the ClientProvider protocol.
/// Only one ClientController can be active. Additional requests to register ClientControllers
/// will result in their channel being immediately closed.
pub async fn serve_provider_requests(
    iface_manager: Arc<Mutex<dyn IfaceManagerApi>>,
    update_sender: listener::ClientListenerMessageSender,
    saved_networks: SavedNetworksPtr,
    scan_requester: Arc<dyn scan::ScanRequestApi>,
    client_provider_lock: Arc<Mutex<()>>,
    mut requests: fidl_policy::ClientProviderRequestStream,
    telemetry_sender: TelemetrySender,
) {
    let mut controller_reqs = FuturesUnordered::new();

    loop {
        select! {
            // Progress controller requests.
            _ = controller_reqs.select_next_some() => (),
            // Process provider requests.
            req = requests.select_next_some() => if let Ok(req) = req {
                // If another component already has a client provider, rejest the request.
                if let Some(client_provider_guard) = client_provider_lock.try_lock() {
                    let fut = handle_provider_request(
                        Arc::clone(&iface_manager),
                        update_sender.clone(),
                        saved_networks.clone(),
                        scan_requester.clone(),
                        client_provider_guard,
                        req,
                        telemetry_sender.clone(),
                    );
                    controller_reqs.push(fut);
                } else if let Err(e) = reject_provider_request(req) {
                    error!("error sending rejection epitaph: {:?}", e);
                }
            },
            complete => break,
        }
    }
}

/// Serves the ClientListener protocol.
pub async fn serve_listener_requests(
    update_sender: listener::ClientListenerMessageSender,
    requests: fidl_policy::ClientListenerRequestStream,
) {
    let serve_fut = requests
        .try_for_each_concurrent(MAX_CONCURRENT_LISTENERS, |req| {
            handle_listener_request(update_sender.clone(), req)
        })
        .unwrap_or_else(|e| error!("error serving Client Listener API: {}", e));
    serve_fut.await;
}

/// Handle inbound requests to acquire a new ClientController.
async fn handle_provider_request(
    iface_manager: Arc<Mutex<dyn IfaceManagerApi>>,
    update_sender: listener::ClientListenerMessageSender,
    saved_networks: SavedNetworksPtr,
    scan_requester: Arc<dyn scan::ScanRequestApi>,
    client_provider_guard: MutexGuard<'_, ()>,
    req: fidl_policy::ClientProviderRequest,
    telemetry_sender: TelemetrySender,
) -> Result<(), fidl::Error> {
    match req {
        fidl_policy::ClientProviderRequest::GetController { requests, updates, .. } => {
            register_listener(update_sender, updates.into_proxy());
            handle_client_requests(
                iface_manager,
                scan_requester,
                saved_networks,
                client_provider_guard,
                requests,
                telemetry_sender,
            )
            .await?;
            Ok(())
        }
    }
}

/// Logs a message for an incoming ClientControllerRequest
fn log_client_request(request: &fidl_policy::ClientControllerRequest) {
    info!(
        "Received policy client request {}",
        match request {
            fidl_policy::ClientControllerRequest::Connect { .. } => "Connect",
            fidl_policy::ClientControllerRequest::StartClientConnections { .. } =>
                "StartClientConnections",
            fidl_policy::ClientControllerRequest::StopClientConnections { .. } =>
                "StopClientConnections",
            fidl_policy::ClientControllerRequest::ScanForNetworks { .. } => "ScanForNetworks",
            fidl_policy::ClientControllerRequest::SaveNetwork { .. } => "SaveNetwork",
            fidl_policy::ClientControllerRequest::RemoveNetwork { .. } => "RemoveNetwork",
            fidl_policy::ClientControllerRequest::ForgetNetwork { .. } => "ForgetNetwork",
            fidl_policy::ClientControllerRequest::GetSavedNetworks { .. } => "GetSavedNetworks",
        }
    );
}

/// Handles all incoming requests from a ClientController.
async fn handle_client_requests(
    iface_manager: Arc<Mutex<dyn IfaceManagerApi>>,
    scan_requester: Arc<dyn scan::ScanRequestApi>,
    saved_networks: SavedNetworksPtr,
    client_provider_guard: MutexGuard<'_, ()>,
    requests: ClientRequests,
    telemetry_sender: TelemetrySender,
) -> Result<(), fidl::Error> {
    let mut request_stream = requests.into_stream();
    while let Some(request) = request_stream.try_next().await? {
        log_client_request(&request);
        match request {
            fidl_policy::ClientControllerRequest::Connect { id, responder, .. } => {
                let response = handle_client_request_connect(
                    Arc::clone(&iface_manager),
                    saved_networks.clone(),
                    &id,
                )
                .await;
                responder.send(response)?
            }
            fidl_policy::ClientControllerRequest::StartClientConnections { responder } => {
                telemetry_sender.send(TelemetryEvent::StartClientConnectionsRequest);
                let response =
                    handle_client_request_start_client_connections(iface_manager.clone()).await;
                responder.send(response)?
            }
            fidl_policy::ClientControllerRequest::StopClientConnections { responder } => {
                telemetry_sender.send(TelemetryEvent::StopClientConnectionsRequest);
                let response =
                    handle_client_request_stop_client_connections(iface_manager.clone()).await;
                responder.send(response)?
            }
            fidl_policy::ClientControllerRequest::ScanForNetworks { iterator, .. } => {
                let fut = handle_client_request_scan(
                    scan_requester.clone(),
                    saved_networks.clone(),
                    telemetry_sender.clone(),
                    iterator,
                );
                // The scan handler is infallible and should not block handling of further request.
                // Detach the future here so we continue responding to other requests.
                fuchsia_async::Task::local(fut).detach();
            }
            fidl_policy::ClientControllerRequest::SaveNetwork { config, responder } => {
                // If there is an error saving the network, log it and convert to a FIDL value.
                let response = handle_client_request_save_network(
                    saved_networks.clone(),
                    config,
                    Arc::clone(&iface_manager),
                )
                .await
                .map_err(|e| {
                    error!("Failed to save network: {:?}", e);
                    fidl_policy::NetworkConfigChangeError::from(e)
                });
                responder.send(response)?;
            }
            fidl_policy::ClientControllerRequest::RemoveNetwork { config, responder } => {
                let id_opt = config.id;
                let err = async {
                    let id = id_opt.ok_or(NetworkConfigError::ConfigMissingId)?;
                    handle_client_request_remove_network(
                        saved_networks.clone(),
                        id,
                        iface_manager.clone(),
                    )
                    .await
                }
                .await
                .map_err(|_| SaveError::GeneralError);

                responder.send(err)?;
            }
            fidl_policy::ClientControllerRequest::ForgetNetwork { id, responder } => {
                let err = handle_client_request_remove_network(
                    saved_networks.clone(),
                    id,
                    iface_manager.clone(),
                )
                .map_err(|_| SaveError::GeneralError)
                .await;
                responder.send(err)?;
            }
            fidl_policy::ClientControllerRequest::GetSavedNetworks { iterator, .. } => {
                handle_client_request_get_networks(saved_networks.clone(), iterator).await?;
            }
        }
    }
    drop(client_provider_guard);
    Ok(())
}

/// Attempts to issue a new connect request to the currently active Client.
/// The network's configuration must have been stored before issuing a connect request.
async fn handle_client_request_connect(
    iface_manager: Arc<Mutex<dyn IfaceManagerApi>>,
    saved_networks: SavedNetworksPtr,
    network: &fidl_policy::NetworkIdentifier,
) -> fidl_policy::RequestStatus {
    let network_config = match saved_networks
        .lookup(&NetworkIdentifier::new(
            client_types::Ssid::from_bytes_unchecked(network.ssid.clone()),
            network.type_.into(),
        ))
        .await
    {
        Some(config) => config,
        None => {
            error!("Requested network not found in saved networks");
            return fidl_policy::RequestStatus::RejectedNotSupported;
        }
    };

    let network_id = fidl_policy::NetworkIdentifier {
        ssid: network_config.ssid.into(),
        type_: fidl_policy::SecurityType::from(network_config.security_type),
    };
    let connect_req = ConnectAttemptRequest::new(
        network_id.into(),
        network_config.credential,
        client_types::ConnectReason::FidlConnectRequest,
    );

    let mut iface_manager = iface_manager.lock().await;
    match iface_manager.connect(connect_req).await {
        Ok(_) => fidl_policy::RequestStatus::Acknowledged,
        Err(e) => {
            error!("failed to connect: {:?}", e);
            fidl_policy::RequestStatus::RejectedIncompatibleMode
        }
    }
}

async fn handle_client_request_scan(
    scan_requester: Arc<dyn scan::ScanRequestApi>,
    saved_networks: SavedNetworksPtr,
    telemetry_sender: TelemetrySender,
    output_iterator: fidl::endpoints::ServerEnd<fidl_fuchsia_wlan_policy::ScanResultIteratorMarker>,
) {
    let get_scan_results = async {
        let passive_scan_results =
            scan_requester.perform_scan(scan::ScanReason::ClientRequest, vec![], vec![]).await?;

        let requested_active_scan_ssids: Vec<types::Ssid> =
            config_management::select_high_probability_hidden_networks(
                saved_networks.get_networks().await,
            )
            .drain(..)
            .map(|id| id.ssid)
            .collect();

        info!(
            "Completed passive scan for API request, {} likely-hidden networks to scan for.",
            requested_active_scan_ssids.len()
        );
        telemetry_sender.send(TelemetryEvent::ActiveScanRequestedViaApi {
            num_ssids_requested: requested_active_scan_ssids.len(),
        });

        if requested_active_scan_ssids.is_empty() {
            Ok(passive_scan_results)
        } else {
            scan_requester
                .perform_scan(scan::ScanReason::ClientRequest, requested_active_scan_ssids, vec![])
                .await
                .map(|mut scan_results| {
                    scan_results.extend(passive_scan_results);
                    scan_results
                })
        }
    };

    match get_scan_results.await {
        Ok(results) => {
            // Note: for now, we must always present WPA2/3 networks as WPA2 over our external
            // interfaces (i.e. to FIDL consumers of scan results). See b/182209070 for more
            // information.
            let fidl_results = scan::scan_result_to_policy_scan_result(&results);
            if let Err(e) = scan::send_scan_results_over_fidl(output_iterator, &fidl_results).await
            {
                warn!("Failed to send scan results to requester: {:?}", e);
            }
        }
        Err(e) => {
            if let Err(e) = scan::send_scan_error_over_fidl(output_iterator, e).await {
                warn!("Failed to send scan error to requester: {:?}", e);
            }
        }
    }
}

/// This function handles requests to save a network by saving the network and sending back to the
/// controller whether or not we successfully saved the network. There could be a FIDL error in
/// sending the response.
async fn handle_client_request_save_network(
    saved_networks: SavedNetworksPtr,
    network_config: fidl_policy::NetworkConfig,
    iface_manager: Arc<Mutex<dyn IfaceManagerApi>>,
) -> Result<(), NetworkConfigError> {
    // The FIDL network config fields are defined as Options, and we consider it an error if either
    // field is missing (ie None) here.
    let net_id: client_types::NetworkIdentifier =
        network_config.id.ok_or(NetworkConfigError::ConfigMissingId)?.into();
    let credential = Credential::try_from(
        network_config.credential.ok_or(NetworkConfigError::ConfigMissingCredential)?,
    )?;
    let evicted_config = saved_networks.store(net_id.clone(), credential.clone()).await?;

    // If a config was removed, disconnect from that network.
    let mut iface_manager = iface_manager.lock().await;
    if let Some(config) = evicted_config {
        let net_id = client_types::NetworkIdentifier {
            ssid: config.ssid,
            security_type: config.security_type,
        };
        match iface_manager
            .disconnect(net_id, client_types::DisconnectReason::NetworkConfigUpdated)
            .await
        {
            Ok(()) => {}
            Err(e) => error!("failed to disconnect from network: {}", e),
        }
    }

    // Attempt to connect to the new network if there is an idle client interface.
    let connect_req = ConnectAttemptRequest::new(
        net_id,
        credential,
        client_types::ConnectReason::NewSavedNetworkAutoconnect,
    );
    match iface_manager.has_idle_client().await {
        Ok(true) => {
            info!("Idle interface available, will attempt connection to new saved network");
            let _ = iface_manager.connect(connect_req).await;
        }
        Ok(false) => {}
        Err(e) => {
            error!("Unable to query idle client state while saving network: {:?}", e);
        }
    }

    Ok(())
}

/// Will remove the specified network and respond to the remove network request with a network
/// config change error if an error occurs while trying to remove the network.
async fn handle_client_request_remove_network(
    saved_networks: SavedNetworksPtr,
    network_id: fidl_policy::NetworkIdentifier,
    iface_manager: Arc<Mutex<dyn IfaceManagerApi>>,
) -> Result<(), NetworkConfigError> {
    let net_id = NetworkIdentifier::from(network_id);
    if saved_networks.remove(net_id.clone()).await? {
        match iface_manager
            .lock()
            .await
            .disconnect(net_id, client_types::DisconnectReason::NetworkUnsaved)
            .await
        {
            Ok(()) => {}
            Err(e) => error!("failed to disconnect from network: {}", e),
        }
    }
    Ok(())
}

async fn handle_client_request_get_networks(
    saved_networks: SavedNetworksPtr,
    iterator: fidl::endpoints::ServerEnd<fidl_policy::NetworkConfigIteratorMarker>,
) -> Result<(), fidl::Error> {
    // make sufficiently small batches of networks to send and convert configs to FIDL values
    let network_configs = saved_networks.get_networks().await;
    let chunks = network_configs.chunks(MAX_CONFIGS_PER_RESPONSE);
    let fidl_chunks = chunks.into_iter().map(|chunk| {
        chunk
            .iter()
            .map(fidl_policy::NetworkConfig::from)
            .collect::<Vec<fidl_policy::NetworkConfig>>()
    });
    let mut stream = iterator.into_stream();
    for chunk in fidl_chunks {
        send_next_chunk(&mut stream, chunk).await?;
    }
    send_next_chunk(&mut stream, vec![]).await
}

/// Send a chunk of saved networks to the specified FIDL iterator
async fn send_next_chunk(
    stream: &mut fidl_policy::NetworkConfigIteratorRequestStream,
    chunk: Vec<fidl_policy::NetworkConfig>,
) -> Result<(), fidl::Error> {
    if let Some(req) = stream.try_next().await? {
        let fidl_policy::NetworkConfigIteratorRequest::GetNext { responder } = req;
        responder.send(&chunk)
    } else {
        // This will happen if the iterator request stream was closed and we expected to send
        // another response.
        // TODO(https://fxbug.dev/42121602) Test this error path
        info!("Info: peer closed channel for network config results unexpectedly");
        Ok(())
    }
}

/// Handle inbound requests to register an additional ClientStateUpdates listener.
async fn handle_listener_request(
    update_sender: listener::ClientListenerMessageSender,
    req: fidl_policy::ClientListenerRequest,
) -> Result<(), fidl::Error> {
    match req {
        fidl_policy::ClientListenerRequest::GetListener { updates, .. } => {
            register_listener(update_sender, updates.into_proxy());
            Ok(())
        }
    }
}

/// Registers a new update listener.
/// The client's current state will be send to the newly added listener immediately.
fn register_listener(
    update_sender: listener::ClientListenerMessageSender,
    listener: fidl_policy::ClientStateUpdatesProxy,
) {
    let _ignored = update_sender.unbounded_send(listener::Message::NewListener(listener));
}

/// Rejects a ClientProvider request by sending a corresponding Epitaph via the |requests| and
/// |updates| channels.
fn reject_provider_request(req: fidl_policy::ClientProviderRequest) -> Result<(), fidl::Error> {
    match req {
        fidl_policy::ClientProviderRequest::GetController { requests, updates, .. } => {
            info!("Rejecting new client controller request because a controller is in use");
            requests.into_channel().close_with_epitaph(zx::Status::ALREADY_BOUND)?;
            updates.into_channel().close_with_epitaph(zx::Status::ALREADY_BOUND)?;
            Ok(())
        }
    }
}

/// Allows client operations to be performed.
async fn handle_client_request_start_client_connections(
    iface_manager: Arc<Mutex<dyn IfaceManagerApi>>,
) -> fidl_policy::RequestStatus {
    let mut iface_manager = iface_manager.lock().await;
    if let Err(e) = iface_manager.start_client_connections().await {
        warn!("encountered an error while starting client connections: {:?}", e);
    }
    fidl_policy::RequestStatus::Acknowledged
}

/// Stops all active client connections and disallows future client operations.
async fn handle_client_request_stop_client_connections(
    iface_manager: Arc<Mutex<dyn IfaceManagerApi>>,
) -> fidl_policy::RequestStatus {
    let mut iface_manager = iface_manager.lock().await;
    if let Err(e) = iface_manager
        .stop_client_connections(client_types::DisconnectReason::FidlStopClientConnectionsRequest)
        .await
    {
        warn!("encountered an error while stopping client connections: {:?}", e);
    }
    fidl_policy::RequestStatus::Acknowledged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access_point::state_machine as ap_fsm;
    use crate::config_management::{NetworkConfig, SecurityType, WPA_PSK_BYTE_LEN};
    use crate::mode_management::iface_manager_api::SmeForScan;
    use crate::util::testing::fakes::{FakeSavedNetworksManager, FakeScanRequester};
    use crate::util::testing::{
        generate_random_fidl_network_config, generate_random_fidl_network_config_with_ssid,
    };
    use anyhow::{Error, format_err};
    use assert_matches::assert_matches;
    use async_trait::async_trait;
    use fidl::endpoints::{Proxy, create_proxy, create_request_stream};
    use fuchsia_async as fasync;
    use futures::channel::{mpsc, oneshot};
    use futures::task::Poll;
    use std::pin::pin;
    use test_case::test_case;

    /// Only used to tell us what disconnect request was given to the IfaceManager so that we
    /// don't need to worry about the implementation logic in the FakeIfaceManager.
    #[derive(Debug)]
    enum IfaceManagerRequest {
        Disconnect(client_types::NetworkIdentifier, client_types::DisconnectReason),
    }

    struct FakeIfaceManager {
        pub sme_proxy: fidl_fuchsia_wlan_sme::ClientSmeProxy,
        pub disconnected_ifaces: Vec<u16>,
        command_sender: mpsc::Sender<IfaceManagerRequest>,
        start_client_connections_succeeds: bool,
        stop_client_connections_succeeds: bool,
    }

    impl FakeIfaceManager {
        pub fn new(
            proxy: fidl_fuchsia_wlan_sme::ClientSmeProxy,
            command_sender: mpsc::Sender<IfaceManagerRequest>,
        ) -> Self {
            FakeIfaceManager {
                sme_proxy: proxy,
                disconnected_ifaces: Vec::new(),
                command_sender,
                start_client_connections_succeeds: true,
                stop_client_connections_succeeds: true,
            }
        }
    }

    #[async_trait(?Send)]
    impl IfaceManagerApi for FakeIfaceManager {
        async fn disconnect(
            &mut self,
            network_id: client_types::NetworkIdentifier,
            reason: client_types::DisconnectReason,
        ) -> Result<(), Error> {
            self.command_sender
                .try_send(IfaceManagerRequest::Disconnect(network_id, reason))
                .map_err(|e| {
                    error!(
                        "Failed to send disconnect: commands_sender's receiver may have
                        been dropped. FakeIfaceManager should be created manually with a sender
                        assigned: {:?}",
                        e
                    );
                    format_err!("failed to send disconnect: {:?}", e)
                })
        }

        async fn connect(&mut self, _connect_req: ConnectAttemptRequest) -> Result<(), Error> {
            let _ = self.disconnected_ifaces.pop();
            Ok(())
        }

        async fn record_idle_client(&mut self, iface_id: u16) -> Result<(), Error> {
            self.disconnected_ifaces.push(iface_id);
            Ok(())
        }

        async fn has_idle_client(&mut self) -> Result<bool, Error> {
            Ok(!self.disconnected_ifaces.is_empty())
        }

        async fn handle_added_iface(&mut self, _iface_id: u16) -> Result<(), Error> {
            unimplemented!()
        }

        async fn handle_removed_iface(&mut self, _iface_id: u16) -> Result<(), Error> {
            unimplemented!()
        }

        async fn get_sme_proxy_for_scan(&mut self) -> Result<SmeForScan, Error> {
            let (defect_sender, _) = mpsc::channel(100);
            Ok(SmeForScan::new(self.sme_proxy.clone(), 0, defect_sender))
        }

        async fn stop_client_connections(
            &mut self,
            _reason: client_types::DisconnectReason,
        ) -> Result<(), Error> {
            if self.stop_client_connections_succeeds {
                Ok(())
            } else {
                Err(format_err!("stop client connections failed"))
            }
        }

        async fn start_client_connections(&mut self) -> Result<(), Error> {
            if self.start_client_connections_succeeds {
                Ok(())
            } else {
                Err(format_err!("start client connections failed"))
            }
        }

        async fn start_ap(
            &mut self,
            _config: ap_fsm::ApConfig,
        ) -> Result<oneshot::Receiver<()>, Error> {
            unimplemented!()
        }

        async fn stop_ap(&mut self, _ssid: types::Ssid, _password: Vec<u8>) -> Result<(), Error> {
            unimplemented!()
        }

        async fn stop_all_aps(&mut self) -> Result<(), Error> {
            unimplemented!()
        }

        async fn set_country(
            &mut self,
            _country_code: Option<types::CountryCode>,
        ) -> Result<(), Error> {
            unimplemented!()
        }
    }

    /// Requests a new ClientController from the given ClientProvider.
    fn request_controller(
        provider: &fidl_policy::ClientProviderProxy,
    ) -> (fidl_policy::ClientControllerProxy, fidl_policy::ClientStateUpdatesRequestStream) {
        let (controller, requests) = create_proxy::<fidl_policy::ClientControllerMarker>();
        let (update_sink, update_stream) =
            create_request_stream::<fidl_policy::ClientStateUpdatesMarker>();
        provider.get_controller(requests, update_sink).expect("error getting controller");
        (controller, update_stream)
    }

    struct TestValues {
        saved_networks: SavedNetworksPtr,
        net_id_open: fidl_policy::NetworkIdentifier,
        net_id_wpa2_w_password: fidl_policy::NetworkIdentifier,
        net_id_wpa2_w_psk: fidl_policy::NetworkIdentifier,
        provider: fidl_policy::ClientProviderProxy,
        requests: fidl_policy::ClientProviderRequestStream,
        iface_manager: Arc<Mutex<dyn IfaceManagerApi>>,
        iface_mgr_req_recvr: mpsc::Receiver<IfaceManagerRequest>,
        scan_requester: Arc<FakeScanRequester>,
        update_sender: mpsc::UnboundedSender<listener::ClientListenerMessage>,
        listener_updates: mpsc::UnboundedReceiver<listener::ClientListenerMessage>,
        client_provider_lock: Arc<Mutex<()>>,
        telemetry_sender: TelemetrySender,
        telemetry_receiver: mpsc::Receiver<TelemetryEvent>,
    }

    // setup channels and proxies needed for the tests to use use the Client Provider and
    // Client Controller APIs in tests. The stash id should be the test name so that each
    // test will have a unique persistent store behind it.
    fn test_setup() -> TestValues {
        let net_id_open = fidl_policy::NetworkIdentifier {
            ssid: "foobar".to_string().into_bytes(),
            type_: fidl_policy::SecurityType::None,
        };
        let net_id_wpa2_w_password = fidl_policy::NetworkIdentifier {
            ssid: b"foobar-wpa2".to_vec(),
            type_: fidl_policy::SecurityType::Wpa2,
        };
        let net_id_wpa2_w_psk = fidl_policy::NetworkIdentifier {
            ssid: b"foobar-psk".to_vec(),
            type_: fidl_policy::SecurityType::Wpa2,
        };

        let presaved_default_configs = vec![
            fidl_policy::NetworkConfig {
                id: Some(net_id_open.clone()),
                credential: Some(fidl_policy::Credential::None(fidl_policy::Empty)),
                ..Default::default()
            },
            fidl_policy::NetworkConfig {
                id: Some(net_id_wpa2_w_password.clone()),
                credential: Some(fidl_policy::Credential::Password(b"foobar-password".to_vec())),
                ..Default::default()
            },
            fidl_policy::NetworkConfig {
                id: Some(net_id_wpa2_w_psk.clone()),
                credential: Some(fidl_policy::Credential::Psk([64; WPA_PSK_BYTE_LEN].to_vec())),
                ..Default::default()
            },
        ];
        let saved_networks =
            Arc::new(FakeSavedNetworksManager::new_with_saved_networks(presaved_default_configs));
        let (telemetry_sender, telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let (provider, requests) = create_proxy::<fidl_policy::ClientProviderMarker>();
        let requests = requests.into_stream();

        let (proxy, _server) = create_proxy::<fidl_fuchsia_wlan_sme::ClientSmeMarker>();
        let (req_sender, iface_mgr_req_recvr) = mpsc::channel(1);
        let iface_manager = FakeIfaceManager::new(proxy.clone(), req_sender);
        let iface_manager = Arc::new(Mutex::new(iface_manager));
        let scan_requester = Arc::new(FakeScanRequester::new());

        let (update_sender, listener_updates) = mpsc::unbounded();

        TestValues {
            saved_networks,
            net_id_open,
            net_id_wpa2_w_password,
            net_id_wpa2_w_psk,
            provider,
            requests,
            iface_manager,
            iface_mgr_req_recvr,
            scan_requester,
            update_sender,
            listener_updates,
            client_provider_lock: Arc::new(Mutex::new(())),
            telemetry_sender: TelemetrySender::new(telemetry_sender),
            telemetry_receiver,
        }
    }

    #[fuchsia::test]
    fn connect_request_unknown_network() {
        let ssid = client_types::Ssid::try_from("foobar-unknown").unwrap();
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let serve_fut = serve_provider_requests(
            test_values.iface_manager,
            test_values.update_sender,
            Arc::clone(&test_values.saved_networks),
            test_values.scan_requester,
            test_values.client_provider_lock,
            test_values.requests,
            test_values.telemetry_sender,
        );
        let mut serve_fut = pin!(serve_fut);

        // No request has been sent yet. Future should be idle.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Request a new controller.
        let (controller, _) = request_controller(&test_values.provider);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Issue connect request.
        let connect_fut = controller.connect(&fidl_policy::NetworkIdentifier {
            ssid: ssid.to_vec(),
            type_: fidl_policy::SecurityType::None,
        });
        let mut connect_fut = pin!(connect_fut);

        // Process connect request and verify connect response.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        assert_matches!(
            exec.run_until_stalled(&mut connect_fut),
            Poll::Ready(Ok(fidl_policy::RequestStatus::RejectedNotSupported))
        );

        // Unknown network should not have been saved by saved networks manager
        // since we did not successfully connect.
        let id = NetworkIdentifier::new(ssid, SecurityType::None);
        let lookup_fut = test_values.saved_networks.lookup(&id);
        assert!(exec.run_singlethreaded(lookup_fut).is_none());
    }

    #[fuchsia::test]
    fn connect_request_open_network() {
        let mut exec = fasync::TestExecutor::new();

        let test_values = test_setup();
        let serve_fut = serve_provider_requests(
            test_values.iface_manager,
            test_values.update_sender,
            Arc::clone(&test_values.saved_networks),
            test_values.scan_requester,
            test_values.client_provider_lock,
            test_values.requests,
            test_values.telemetry_sender,
        );
        let mut serve_fut = pin!(serve_fut);

        // No request has been sent yet. Future should be idle.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Request a new controller.
        let (controller, _update_stream) = request_controller(&test_values.provider);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Issue connect request.
        let connect_fut = controller.connect(&test_values.net_id_open);
        let mut connect_fut = pin!(connect_fut);

        // Process connect request and verify connect response.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Verify that the connect call is acknowledged.
        assert_matches!(
            exec.run_until_stalled(&mut connect_fut),
            Poll::Ready(Ok(fidl_policy::RequestStatus::Acknowledged))
        );
    }

    #[fuchsia::test]
    fn connect_request_protected_network() {
        let mut exec = fasync::TestExecutor::new();

        let test_values = test_setup();
        let serve_fut = serve_provider_requests(
            test_values.iface_manager,
            test_values.update_sender,
            Arc::clone(&test_values.saved_networks),
            test_values.scan_requester,
            test_values.client_provider_lock,
            test_values.requests,
            test_values.telemetry_sender,
        );
        let mut serve_fut = pin!(serve_fut);

        // No request has been sent yet. Future should be idle.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Request a new controller.
        let (controller, _update_stream) = request_controller(&test_values.provider);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Issue connect request.
        let connect_fut = controller.connect(&test_values.net_id_wpa2_w_password);
        let mut connect_fut = pin!(connect_fut);

        // Process connect request and verify connect response.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Verify that the connect call is acknowledged.
        assert_matches!(
            exec.run_until_stalled(&mut connect_fut),
            Poll::Ready(Ok(fidl_policy::RequestStatus::Acknowledged))
        );
    }

    #[fuchsia::test]
    fn connect_request_protected_psk_network() {
        let mut exec = fasync::TestExecutor::new();

        let test_values = test_setup();
        let serve_fut = serve_provider_requests(
            test_values.iface_manager,
            test_values.update_sender,
            Arc::clone(&test_values.saved_networks),
            test_values.scan_requester,
            test_values.client_provider_lock,
            test_values.requests,
            test_values.telemetry_sender,
        );
        let mut serve_fut = pin!(serve_fut);

        // No request has been sent yet. Future should be idle.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Request a new controller.
        let (controller, _update_stream) = request_controller(&test_values.provider);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Issue connect request.
        let connect_fut = controller.connect(&test_values.net_id_wpa2_w_psk);
        let mut connect_fut = pin!(connect_fut);

        // Process connect request and verify connect response.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Verify that the connect call is acknowledged.
        assert_matches!(
            exec.run_until_stalled(&mut connect_fut),
            Poll::Ready(Ok(fidl_policy::RequestStatus::Acknowledged))
        );
    }

    #[fuchsia::test]
    fn start_and_stop_client_connections() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = test_setup();
        let serve_fut = serve_provider_requests(
            test_values.iface_manager,
            test_values.update_sender,
            test_values.saved_networks,
            test_values.scan_requester,
            test_values.client_provider_lock,
            test_values.requests,
            test_values.telemetry_sender,
        );
        let mut serve_fut = pin!(serve_fut);

        // No request has been sent yet. Future should now be waiting for request.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Request a new controller.
        let (controller, _update_stream) = request_controller(&test_values.provider);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Issue request to start client connections.
        let start_fut = controller.start_client_connections();
        let mut start_fut = pin!(start_fut);

        // Request should be acknowledged.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut start_fut),
            Poll::Ready(Ok(fidl_policy::RequestStatus::Acknowledged))
        );

        // A message should be sent to telemetry.
        assert_matches!(test_values.telemetry_receiver.try_next(), Ok(Some(event)) => {
            assert_matches!(event, TelemetryEvent::StartClientConnectionsRequest);
        });

        // Perform a connect operation.
        let connect_fut = controller.connect(&test_values.net_id_wpa2_w_password);
        let mut connect_fut = pin!(connect_fut);

        // Process connect request and verify connect response.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Verify that the connect call is acknowledged.
        assert_matches!(
            exec.run_until_stalled(&mut connect_fut),
            Poll::Ready(Ok(fidl_policy::RequestStatus::Acknowledged))
        );

        // The client state machine will immediately query for status.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Issue request to stop client connections.
        let stop_fut = controller.stop_client_connections();
        let mut stop_fut = pin!(stop_fut);

        // Run the serve future until it stalls and expect the client to disconnect
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // A message should be sent to telemetry.
        assert_matches!(test_values.telemetry_receiver.try_next(), Ok(Some(event)) => {
            assert_matches!(event, TelemetryEvent::StopClientConnectionsRequest);
        });

        // Request should be acknowledged.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut stop_fut),
            Poll::Ready(Ok(fidl_policy::RequestStatus::Acknowledged))
        );
    }

    /// End-to-end test of the scan function, verifying that an incoming
    /// FIDL scan request results in a scan in the SME, and that the results
    /// make it back to the requester.
    #[fuchsia::test]
    fn scan_end_to_end() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let serve_fut = serve_provider_requests(
            test_values.iface_manager,
            test_values.update_sender,
            test_values.saved_networks,
            test_values.scan_requester.clone(),
            test_values.client_provider_lock,
            test_values.requests,
            test_values.telemetry_sender,
        );
        let mut serve_fut = pin!(serve_fut);

        // No request has been sent yet. Future should be idle.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Request a new controller.
        let (controller, _update_stream) = request_controller(&test_values.provider);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Issue a scan request.
        let (_iter, server) = fidl::endpoints::create_proxy();
        controller.scan_for_networks(server).expect("Failed to call scan for networks");

        // Currently the FakeScanRequester will panic if a scan is requested and no results
        // are queued, so add something.
        exec.run_singlethreaded(test_values.scan_requester.add_scan_result(Ok(vec![])));
        // A second scan request for all the likely-hidden networks
        exec.run_singlethreaded(test_values.scan_requester.add_scan_result(Ok(vec![])));

        // Process scan request and verify scan response.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Check that a scan request was sent to the scan module, including an active scan for the
        // potentially-hidden networks.
        let expected_active_ssids = [
            test_values.net_id_wpa2_w_password,
            test_values.net_id_wpa2_w_psk,
            test_values.net_id_open,
        ]
        .iter()
        .map(|id| types::Ssid::from_bytes_unchecked(id.ssid.clone()))
        .collect();
        exec.run_singlethreaded(test_values.scan_requester.verify_scan_request((
            scan::ScanReason::ClientRequest,
            vec![],
            vec![],
        )));
        exec.run_singlethreaded(test_values.scan_requester.verify_scan_request((
            scan::ScanReason::ClientRequest,
            expected_active_ssids,
            vec![],
        )));
    }

    #[fuchsia::test]
    fn save_network() {
        let mut exec = fasync::TestExecutor::new();
        let saved_networks = Arc::new(FakeSavedNetworksManager::new());

        let mut test_values = test_setup();
        let serve_fut = serve_provider_requests(
            test_values.iface_manager,
            test_values.update_sender,
            saved_networks.clone(),
            test_values.scan_requester,
            test_values.client_provider_lock,
            test_values.requests,
            test_values.telemetry_sender,
        );
        let mut serve_fut = pin!(serve_fut);

        // No request has been sent yet. Future should be idle.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Request a new controller.
        let (controller, _update_stream) = request_controller(&test_values.provider);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut test_values.listener_updates.next()),
            Poll::Ready(_)
        );

        // Save some network
        let network_id = fidl_policy::NetworkIdentifier {
            ssid: b"foo".to_vec(),
            type_: fidl_policy::SecurityType::None,
        };
        let network_config = fidl_policy::NetworkConfig {
            id: Some(network_id.clone()),
            credential: Some(fidl_policy::Credential::None(fidl_policy::Empty)),
            ..Default::default()
        };
        let mut save_fut = controller.save_network(&network_config);

        // Run server_provider forward so that it will process the save network request
        assert_matches!(exec.run_until_stalled(&mut save_fut), Poll::Pending);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Check that the the response says we succeeded.
        assert_matches!(exec.run_until_stalled(&mut save_fut), Poll::Ready(Ok(Ok(()))));

        // Check that the value was actually saved in the saved networks manager.
        let target_id = NetworkIdentifier::from(network_id);
        let target_config = NetworkConfig::new(target_id.clone(), Credential::None, false, None)
            .expect("Failed to create network config");
        assert_eq!(exec.run_singlethreaded(saved_networks.lookup(&target_id)), Some(target_config));
    }

    #[fuchsia::test]
    fn save_network_with_disconnected_iface() {
        let mut exec = fasync::TestExecutor::new();
        let saved_networks = Arc::new(FakeSavedNetworksManager::new());

        let mut test_values = test_setup();
        let serve_fut = serve_provider_requests(
            test_values.iface_manager.clone(),
            test_values.update_sender,
            saved_networks.clone(),
            test_values.scan_requester,
            test_values.client_provider_lock,
            test_values.requests,
            test_values.telemetry_sender,
        );
        let mut serve_fut = pin!(serve_fut);

        // Setup the IfaceManager so that it has a disconnected iface.
        {
            let iface_manager = test_values.iface_manager.clone();
            let iface_manager_fut = iface_manager.lock();
            let mut iface_manager_fut = pin!(iface_manager_fut);
            let mut iface_manager = match exec.run_until_stalled(&mut iface_manager_fut) {
                Poll::Ready(iface_manager) => iface_manager,
                Poll::Pending => panic!("expected to acquire iface_manager lock"),
            };
            let record_idle_fut = iface_manager.record_idle_client(0);
            let mut record_idle_fut = pin!(record_idle_fut);
            assert_matches!(exec.run_until_stalled(&mut record_idle_fut), Poll::Ready(Ok(())));
        }

        // No request has been sent yet. Future should be idle.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Request a new controller.
        let (controller, _update_stream) = request_controller(&test_values.provider);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut test_values.listener_updates.next()),
            Poll::Ready(_)
        );

        // Save some network
        let network_id = fidl_policy::NetworkIdentifier {
            ssid: b"foo".to_vec(),
            type_: fidl_policy::SecurityType::None,
        };
        let network_config = fidl_policy::NetworkConfig {
            id: Some(network_id.clone()),
            credential: Some(fidl_policy::Credential::None(fidl_policy::Empty)),
            ..Default::default()
        };
        let mut save_fut = controller.save_network(&network_config);

        assert_matches!(exec.run_until_stalled(&mut save_fut), Poll::Pending);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Check that the the response says we succeeded.
        assert_matches!(exec.run_until_stalled(&mut save_fut), Poll::Ready(result) => {
            let save_result = result.expect("Failed to get save network response");
            assert_eq!(save_result, Ok(()));
        });

        // Check that the value was actually saved in the saved networks manager.
        let target_id = NetworkIdentifier::from(network_id);
        let target_config = NetworkConfig::new(target_id.clone(), Credential::None, false, None)
            .expect("Failed to create network config");
        assert_eq!(exec.run_singlethreaded(saved_networks.lookup(&target_id)), Some(target_config));
    }

    #[fuchsia::test]
    fn save_network_overwrite_disconnects() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = test_setup();
        let saved_networks = Arc::new(FakeSavedNetworksManager::new());

        let serve_fut = serve_provider_requests(
            test_values.iface_manager,
            test_values.update_sender,
            saved_networks.clone(),
            test_values.scan_requester,
            test_values.client_provider_lock,
            test_values.requests,
            test_values.telemetry_sender,
        );
        let mut serve_fut = pin!(serve_fut);

        // No request has been sent yet. Future should be idle.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Request a new controller.
        let (controller, _update_stream) = request_controller(&test_values.provider);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut test_values.listener_updates.next()),
            Poll::Ready(_)
        );

        // Save the network directly.
        let network_id = NetworkIdentifier::try_from("foo", SecurityType::Wpa2).unwrap();
        let credential = Credential::Password(b"password".to_vec());
        let save_fut = saved_networks.store(network_id.clone(), credential.clone());
        let mut save_fut = pin!(save_fut);
        assert_matches!(exec.run_until_stalled(&mut save_fut), Poll::Ready(Ok(None)));

        // Save a network with the same identifier but a different password
        let network_config = fidl_policy::NetworkConfig {
            id: Some(fidl_policy::NetworkIdentifier::from(network_id.clone())),
            credential: Some(fidl_policy::Credential::Password(b"other-password".to_vec())),
            ..Default::default()
        };
        let mut save_fut = controller.save_network(&network_config);

        // Process the remove request on the server side and handle requests to stash on the way.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Check that the iface manager was asked to disconnect from some network
        assert_matches!(exec.run_until_stalled(&mut test_values.iface_mgr_req_recvr.next()), Poll::Ready(Some(IfaceManagerRequest::Disconnect(net_id, reason))) => {
            assert_eq!(net_id, network_id.clone());
            assert_eq!(reason, client_types::DisconnectReason::NetworkConfigUpdated);
        });
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert_matches!(exec.run_until_stalled(&mut save_fut), Poll::Ready(Ok(Ok(()))));
    }

    #[fuchsia::test]
    fn save_bad_network_should_fail() {
        let mut exec = fasync::TestExecutor::new();
        let mut saved_networks = FakeSavedNetworksManager::new();
        saved_networks.fail_all_stores = true;
        let saved_networks = Arc::new(saved_networks);

        let mut test_values = test_setup();
        let serve_fut = serve_provider_requests(
            test_values.iface_manager,
            test_values.update_sender,
            saved_networks.clone(),
            test_values.scan_requester,
            test_values.client_provider_lock,
            test_values.requests,
            test_values.telemetry_sender,
        );
        let mut serve_fut = pin!(serve_fut);

        // No request has been sent yet. Future should be idle.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Request a new controller.
        let (controller, _update_stream) = request_controller(&test_values.provider);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut test_values.listener_updates.next()),
            Poll::Ready(_)
        );

        // Create a network config whose password is too short. FIDL network config does not
        // require valid fields unlike our crate define config. We should not be able to
        // successfully save this network through the API.
        let bad_network_id = fidl_policy::NetworkIdentifier {
            ssid: b"foo".to_vec(),
            type_: fidl_policy::SecurityType::Wpa2,
        };
        let network_config = fidl_policy::NetworkConfig {
            id: Some(bad_network_id.clone()),
            credential: Some(fidl_policy::Credential::Password(b"bar".to_vec())),
            ..Default::default()
        };
        // Attempt to save the config
        let mut save_fut = controller.save_network(&network_config);

        // Run server_provider forward so that it will process the save network request
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Check that the the response says we failed to save the network.
        assert_matches!(exec.run_until_stalled(&mut save_fut), Poll::Ready(result) => {
            let error = result.expect("Failed to get save network response");
            assert_eq!(error, Err(fidl_policy::NetworkConfigChangeError::NetworkConfigWriteError));
        });

        // Check that the value was was not saved in saved networks manager.
        let target_id = NetworkIdentifier::from(bad_network_id);
        assert_eq!(exec.run_singlethreaded(saved_networks.lookup(&target_id)), None);
    }

    #[fuchsia::test]
    fn test_remove_a_network() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = test_setup();

        let serve_fut = serve_provider_requests(
            test_values.iface_manager,
            test_values.update_sender,
            test_values.saved_networks.clone(),
            test_values.scan_requester,
            test_values.client_provider_lock,
            test_values.requests,
            test_values.telemetry_sender,
        );
        let mut serve_fut = pin!(serve_fut);

        // No request has been sent yet. Future should be idle.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Request a new controller.
        let (controller, _update_stream) = request_controller(&test_values.provider);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut test_values.listener_updates.next()),
            Poll::Ready(_)
        );

        // Request to a network that is saved
        let mut remove_fut = controller.forget_network(&test_values.net_id_open);

        // Process the remove request on the server side and handle requests to stash on the way.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Successfully removing a network should always request a disconnect from IfaceManager,
        // which will know whether we are connected to the network to disconnect from. This checks
        // that the IfaceManager is told to disconnect (if connected).
        assert_matches!(exec.run_until_stalled(&mut test_values.iface_mgr_req_recvr.next()), Poll::Ready(Some(IfaceManagerRequest::Disconnect(net_id, reason))) => {
            assert_eq!(net_id, test_values.net_id_open.clone().into());
            assert_eq!(reason, client_types::DisconnectReason::NetworkUnsaved);
        });
        assert_matches!(exec.run_until_stalled(&mut remove_fut), Poll::Ready(Ok(Ok(()))));
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert!(
            exec.run_singlethreaded(
                test_values.saved_networks.lookup(&test_values.net_id_open.clone().into())
            )
            .is_none()
        );

        // Removing a network that is not saved should not trigger a disconnect.
        let mut remove_fut = controller.forget_network(&test_values.net_id_open);
        // Process the remove request on the server side and handle requests to stash on the way.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut test_values.iface_mgr_req_recvr.next()),
            Poll::Pending
        );
        assert_matches!(exec.run_until_stalled(&mut remove_fut), Poll::Ready(Ok(Ok(()))));
    }

    #[fuchsia::test]
    fn test_get_saved_network() {
        let expected_configs = vec![generate_random_fidl_network_config()];
        let expected_num_sends = 1;
        run_get_saved_networks_test(expected_configs, expected_num_sends);
    }

    #[fuchsia::test]
    fn test_get_saved_networks_multiple_chunks() {
        // Save MAX_CONFIGS_PER_RESPONSE + 1 configs so that get_saved_networks should respond with
        // 2 chunks of responses plus one response with an empty vector.
        let mut configs = vec![];
        for index in 0..MAX_CONFIGS_PER_RESPONSE + 1 {
            // Create unique network config to be saved.
            let ssid = format!("some_config{index}");
            let network_config = generate_random_fidl_network_config_with_ssid(&ssid);
            configs.push(network_config);
        }

        let expected_num_sends = 2;
        run_get_saved_networks_test(configs, expected_num_sends);
    }

    /// Test that get saved networks with the given saved networks
    /// test_id: the name of the test to create a unique persistent store for each test
    /// configs: list of FIDL NetworkConfigs that we expect to get from get_saved_networks and
    ///          and which are returned by SavedNetworksManager
    /// expected_num_sends: number of chunks of results we expect to get from get_saved_networks.
    ///     This is not counting the empty vector that signifies no more results.
    fn run_get_saved_networks_test(
        configs: Vec<fidl_policy::NetworkConfig>,
        expected_num_sends: usize,
    ) {
        let mut exec = fasync::TestExecutor::new();
        let saved_networks =
            Arc::new(FakeSavedNetworksManager::new_with_saved_networks(configs.clone()));

        let test_values = test_setup();
        let serve_fut = serve_provider_requests(
            test_values.iface_manager,
            test_values.update_sender,
            saved_networks.clone(),
            test_values.scan_requester,
            test_values.client_provider_lock,
            test_values.requests,
            test_values.telemetry_sender,
        );
        let mut serve_fut = pin!(serve_fut);

        // No request has been sent yet. Future should be idle.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Request a new controller.
        let (controller, _update_stream) = request_controller(&test_values.provider);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Issue request to get the list of saved networks.
        let (iter, server) =
            fidl::endpoints::create_proxy::<fidl_policy::NetworkConfigIteratorMarker>();
        controller.get_saved_networks(server).expect("Failed to call get saved networks");

        // Get responses from iterator. Expect to see the specified number of responses with
        // results plus one response of an empty vector indicating the end of results.
        let mut saved_networks_results = vec![];
        for i in 0..expected_num_sends {
            let get_saved_fut = iter.get_next();
            assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
            let results = exec
                .run_singlethreaded(get_saved_fut)
                .expect("Failed to get next chunk of saved networks results");
            // the size of received chunk should either be max chunk size or whatever is left
            // to receive in the last chunk
            if i < expected_num_sends - 1 {
                assert_eq!(results.len(), MAX_CONFIGS_PER_RESPONSE);
            } else {
                assert_eq!(results.len(), configs.len() % MAX_CONFIGS_PER_RESPONSE);
            }
            saved_networks_results.extend(results);
        }
        let get_saved_end_fut = iter.get_next();
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        let results = exec
            .run_singlethreaded(get_saved_end_fut)
            .expect("Failed to get next chunk of saved networks results");
        assert!(results.is_empty());

        // check whether each network we saved is in the results and that nothing else is there.
        for network_config in &configs {
            assert!(saved_networks_results.contains(network_config));
        }
        assert_eq!(configs.len(), saved_networks_results.len());
    }

    #[fuchsia::test]
    fn register_update_listener() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = test_setup();
        let serve_fut = serve_provider_requests(
            test_values.iface_manager,
            test_values.update_sender,
            test_values.saved_networks,
            test_values.scan_requester,
            test_values.client_provider_lock,
            test_values.requests,
            test_values.telemetry_sender,
        );
        let mut serve_fut = pin!(serve_fut);

        // No request has been sent yet. Future should be idle.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Request a new controller.
        let (_controller, _update_stream) = request_controller(&test_values.provider);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut test_values.listener_updates.next()),
            Poll::Ready(Some(listener::Message::NewListener(_)))
        );
    }

    #[fuchsia::test]
    fn get_listener() {
        let mut exec = fasync::TestExecutor::new();
        let (listener, requests) = create_proxy::<fidl_policy::ClientListenerMarker>();
        let requests = requests.into_stream();

        let (update_sender, mut listener_updates) = mpsc::unbounded();
        let serve_fut = serve_listener_requests(update_sender, requests);
        let mut serve_fut = pin!(serve_fut);

        // No request has been sent yet. Future should be idle.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Register listener.
        let (update_sink, _update_stream) =
            create_request_stream::<fidl_policy::ClientStateUpdatesMarker>();
        listener.get_listener(update_sink).expect("error getting listener");

        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut listener_updates.next()),
            Poll::Ready(Some(listener::Message::NewListener(_)))
        );
    }

    #[fuchsia::test]
    fn multiple_controllers_write_attempt() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let serve_fut = serve_provider_requests(
            test_values.iface_manager,
            test_values.update_sender,
            test_values.saved_networks,
            test_values.scan_requester,
            test_values.client_provider_lock,
            test_values.requests,
            test_values.telemetry_sender,
        );
        let mut serve_fut = pin!(serve_fut);

        // No request has been sent yet. Future should be idle.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Request a controller.
        let (controller1, _) = request_controller(&test_values.provider);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Request another controller.
        let (controller2, _) = request_controller(&test_values.provider);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Ensure first controller is operable. Issue connect request.
        let connect_fut = controller1.connect(&test_values.net_id_open);
        let mut connect_fut = pin!(connect_fut);

        // Process connect request from first controller. Verify success.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        assert_matches!(
            exec.run_until_stalled(&mut connect_fut),
            Poll::Ready(Ok(fidl_policy::RequestStatus::Acknowledged))
        );

        // Ensure second controller is not operable. Issue connect request.
        let connect_fut = controller2.connect(&test_values.net_id_open);
        let mut connect_fut = pin!(connect_fut);

        // Process connect request from second controller. Verify failure.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut connect_fut),
            Poll::Ready(Err(fidl::Error::ClientChannelClosed { epitaph, .. }))
                if epitaph == zx::Status::ALREADY_BOUND
        );

        // Drop first controller. A new controller can now take control.
        drop(controller1);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Request another controller.
        let (controller3, _) = request_controller(&test_values.provider);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Ensure third controller is operable. Issue connect request.
        let connect_fut = controller3.connect(&test_values.net_id_open);
        let mut connect_fut = pin!(connect_fut);

        // Process connect request from third controller. Verify success.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        assert_matches!(
            exec.run_until_stalled(&mut connect_fut),
            Poll::Ready(Ok(fidl_policy::RequestStatus::Acknowledged))
        );
    }

    #[fuchsia::test]
    fn multiple_controllers_epitaph() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let serve_fut = serve_provider_requests(
            test_values.iface_manager,
            test_values.update_sender,
            test_values.saved_networks.clone(),
            test_values.scan_requester,
            test_values.client_provider_lock,
            test_values.requests,
            test_values.telemetry_sender,
        );
        let mut serve_fut = pin!(serve_fut);

        // No request has been sent yet. Future should be idle.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Request a controller.
        let (_controller1, _) = request_controller(&test_values.provider);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Request another controller.
        let (controller2, _) = request_controller(&test_values.provider);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Verify Epitaph was received.
        let mut controller2_event_stream = controller2.take_event_stream();
        let controller2_event_fut = controller2_event_stream.next();
        let mut controller2_event_fut = pin!(controller2_event_fut);
        assert_matches!(
            exec.run_until_stalled(&mut controller2_event_fut),
            Poll::Ready(Some(Err(fidl::Error::ClientChannelClosed { epitaph, .. })))
                if epitaph == zx::Status::ALREADY_BOUND
        );

        assert!(controller2.is_closed());
    }

    struct FakeIfaceManagerNoIfaces {}

    #[async_trait(?Send)]
    impl IfaceManagerApi for FakeIfaceManagerNoIfaces {
        async fn disconnect(
            &mut self,
            _network_id: client_types::NetworkIdentifier,
            _reason: client_types::DisconnectReason,
        ) -> Result<(), Error> {
            Err(format_err!("No ifaces"))
        }

        async fn connect(&mut self, _connect_req: ConnectAttemptRequest) -> Result<(), Error> {
            Err(format_err!("No ifaces"))
        }

        async fn record_idle_client(&mut self, _iface_id: u16) -> Result<(), Error> {
            unimplemented!()
        }

        async fn has_idle_client(&mut self) -> Result<bool, Error> {
            unimplemented!()
        }

        async fn handle_added_iface(&mut self, _iface_id: u16) -> Result<(), Error> {
            unimplemented!()
        }

        async fn handle_removed_iface(&mut self, _iface_id: u16) -> Result<(), Error> {
            unimplemented!()
        }

        async fn get_sme_proxy_for_scan(&mut self) -> Result<SmeForScan, Error> {
            Err(format_err!("No ifaces"))
        }

        async fn stop_client_connections(
            &mut self,
            _reason: client_types::DisconnectReason,
        ) -> Result<(), Error> {
            unimplemented!()
        }

        async fn start_client_connections(&mut self) -> Result<(), Error> {
            unimplemented!()
        }

        async fn start_ap(
            &mut self,
            _config: ap_fsm::ApConfig,
        ) -> Result<oneshot::Receiver<()>, Error> {
            unimplemented!()
        }

        async fn stop_ap(&mut self, _ssid: types::Ssid, _password: Vec<u8>) -> Result<(), Error> {
            unimplemented!()
        }

        async fn stop_all_aps(&mut self) -> Result<(), Error> {
            unimplemented!()
        }

        async fn set_country(
            &mut self,
            _country_code: Option<types::CountryCode>,
        ) -> Result<(), Error> {
            unimplemented!()
        }
    }

    #[fuchsia::test]
    fn no_client_interface() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();

        let iface_manager = Arc::new(Mutex::new(FakeIfaceManagerNoIfaces {}));

        let serve_fut = serve_provider_requests(
            iface_manager,
            test_values.update_sender,
            test_values.saved_networks,
            test_values.scan_requester,
            test_values.client_provider_lock,
            test_values.requests,
            test_values.telemetry_sender,
        );
        let mut serve_fut = pin!(serve_fut);

        // No request has been sent yet. Future should be idle.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Request a controller.
        let (controller, _) = request_controller(&test_values.provider);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Issue connect request.
        let connect_fut = controller.connect(&test_values.net_id_open);
        let mut connect_fut = pin!(connect_fut);

        // Process connect request from first controller. Verify success.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut connect_fut),
            Poll::Ready(Ok(fidl_policy::RequestStatus::RejectedIncompatibleMode))
        );
    }

    // Gets a saved network config with a particular SSID, security type, and credential.
    // If there are more than one configs saved for the same SSID, security type, and credential,
    // the function will panic.
    async fn get_config(
        saved_networks: Arc<dyn SavedNetworksManagerApi>,
        id: NetworkIdentifier,
        cred: Credential,
    ) -> Option<NetworkConfig> {
        saved_networks.lookup(&id).await.filter(|cfg| cfg.credential == cred)
    }

    #[fuchsia::test]
    async fn get_correct_config() {
        let saved_networks = Arc::new(FakeSavedNetworksManager::new());
        let network_id = NetworkIdentifier::try_from("foo", SecurityType::Wpa2).unwrap();
        let cfg = NetworkConfig::new(
            network_id.clone(),
            Credential::Password(b"password".to_vec()),
            false,
            None,
        )
        .expect("Failed to create network config");

        assert!(
            saved_networks
                .store(network_id.clone(), Credential::Password(b"password".to_vec()))
                .await
                .expect("Failed to store network config")
                .is_none()
        );

        assert_eq!(
            Some(cfg),
            get_config(
                saved_networks.clone(),
                network_id,
                Credential::Password(b"password".to_vec())
            )
            .await
        );
        assert_eq!(
            None,
            get_config(
                saved_networks.clone(),
                NetworkIdentifier::try_from("foo", SecurityType::Wpa2).unwrap(),
                Credential::Password(b"not-saved".to_vec())
            )
            .await
        );
    }

    #[fuchsia::test]
    fn multiple_api_clients() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup();
        let serve_fut = serve_provider_requests(
            test_values.iface_manager.clone(),
            test_values.update_sender.clone(),
            test_values.saved_networks.clone(),
            test_values.scan_requester.clone(),
            test_values.client_provider_lock.clone(),
            test_values.requests,
            test_values.telemetry_sender.clone(),
        );
        let mut serve_fut = pin!(serve_fut);

        // No request has been sent yet. Future should be idle.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Request a controller.
        let (controller1, _) = request_controller(&test_values.provider);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Create another client provider request stream and start serving it.  This is equivalent
        // to the behavior that occurs when a second client connects to the ClientProvider service.
        let (provider, requests) = create_proxy::<fidl_policy::ClientProviderMarker>();
        let requests = requests.into_stream();

        let second_serve_fut = serve_provider_requests(
            test_values.iface_manager.clone(),
            test_values.update_sender.clone(),
            test_values.saved_networks.clone(),
            test_values.scan_requester,
            test_values.client_provider_lock.clone(),
            requests,
            test_values.telemetry_sender,
        );
        let mut second_serve_fut = pin!(second_serve_fut);

        // Request a client controller from the second instance of the service.
        let (controller2, _) = request_controller(&provider);
        assert_matches!(exec.run_until_stalled(&mut second_serve_fut), Poll::Pending);

        // Verify Epitaph was received.
        let mut controller2_event_stream = controller2.take_event_stream();
        let controller2_event_fut = controller2_event_stream.next();
        let mut controller2_event_fut = pin!(controller2_event_fut);
        assert_matches!(
            exec.run_until_stalled(&mut controller2_event_fut),
            Poll::Ready(Some(Err(fidl::Error::ClientChannelClosed { epitaph, .. })))
                if epitaph == zx::Status::ALREADY_BOUND
        );

        assert!(controller2.is_closed());

        // Drop the first controller and verify that the second provider client can get a
        // functional client controller.
        drop(controller1);
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        let (controller2, _) = request_controller(&provider);
        assert_matches!(exec.run_until_stalled(&mut second_serve_fut), Poll::Pending);

        // Ensure the new controller works by issuing a connect request.
        let connect_fut = controller2.connect(&test_values.net_id_open);
        let mut connect_fut = pin!(connect_fut);

        // Process connect request from first controller. Verify success.
        assert_matches!(exec.run_until_stalled(&mut second_serve_fut), Poll::Pending);

        assert_matches!(
            exec.run_until_stalled(&mut connect_fut),
            Poll::Ready(Ok(fidl_policy::RequestStatus::Acknowledged))
        );
    }

    #[test_case(true)]
    #[test_case(false)]
    #[fuchsia::test(add_test_attr = false)]
    fn test_start_client_connections(start_client_connections_succeeds: bool) {
        let mut exec = fasync::TestExecutor::new();
        let (proxy, _server) = create_proxy::<fidl_fuchsia_wlan_sme::ClientSmeMarker>();
        let (req_sender, _req_recvr) = mpsc::channel(1);
        let mut iface_manager = FakeIfaceManager::new(proxy.clone(), req_sender);

        // Setup the IfaceManager to succeed or fail depending on the test configuration.
        iface_manager.start_client_connections_succeeds = start_client_connections_succeeds;

        let iface_manager: Arc<Mutex<dyn IfaceManagerApi>> = Arc::new(Mutex::new(iface_manager));

        // Regardless of what the IfaceManager is actually able to do, we expect the response to
        // simply ack the caller.
        let fut = handle_client_request_start_client_connections(iface_manager);
        let mut fut = pin!(fut);
        assert_matches!(
            exec.run_until_stalled(&mut fut),
            Poll::Ready(fidl_policy::RequestStatus::Acknowledged)
        );
    }

    #[test_case(true)]
    #[test_case(false)]
    #[fuchsia::test(add_test_attr = false)]
    fn test_stop_client_connections(stop_client_connections_succeeds: bool) {
        let mut exec = fasync::TestExecutor::new();
        let (proxy, _server) = create_proxy::<fidl_fuchsia_wlan_sme::ClientSmeMarker>();
        let (req_sender, _req_recvr) = mpsc::channel(1);
        let mut iface_manager = FakeIfaceManager::new(proxy.clone(), req_sender);

        // Setup the IfaceManager to succeed or fail depending on the test configuration.
        iface_manager.stop_client_connections_succeeds = stop_client_connections_succeeds;

        let iface_manager: Arc<Mutex<dyn IfaceManagerApi>> = Arc::new(Mutex::new(iface_manager));

        // Regardless of what the IfaceManager is actually able to do, we expect the response to
        // simply ack the caller.
        let fut = handle_client_request_stop_client_connections(iface_manager);
        let mut fut = pin!(fut);
        assert_matches!(
            exec.run_until_stalled(&mut fut),
            Poll::Ready(fidl_policy::RequestStatus::Acknowledged)
        );
    }
}
