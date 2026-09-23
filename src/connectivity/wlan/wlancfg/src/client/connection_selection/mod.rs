// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::client::scan::{self, ScanReason};
use crate::client::types::{
    self, Bss, InternalSavedNetworkData, SecurityType, SecurityTypeDetailed,
};
use crate::config_management::{
    self, Credential, SavedNetworksManagerApi, network_config, select_authentication_method,
    select_subset_potentially_hidden_networks,
};
use crate::telemetry::{self, TelemetryEvent, TelemetrySender};
use anyhow::format_err;
use async_trait::async_trait;
use fidl_fuchsia_wlan_common as fidl_common;
use fuchsia_async as fasync;
use fuchsia_inspect::Node as InspectNode;
use fuchsia_inspect_contrib::inspect_insert;
use fuchsia_inspect_contrib::log::WriteInspect;
use fuchsia_inspect_contrib::nodes::BoundedListNode as InspectBoundedListNode;
use futures::channel::{mpsc, oneshot};
use futures::lock::Mutex;
use futures::select;
use futures::stream::StreamExt;
use log::{debug, error, info, warn};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use wlan_common::security::SecurityAuthenticator;
use wlan_common::sequestered::Sequestered;

pub mod bss_selection;
pub mod fut_manager;
pub mod network_selection;
pub mod scoring_functions;

pub const CONNECTION_SELECTION_REQUEST_BUFFER_SIZE: usize = 100;
const INSPECT_EVENT_LIMIT_FOR_CONNECTION_SELECTIONS: usize = 10;

const RECENT_DISCONNECT_WINDOW: zx::MonotonicDuration =
    zx::MonotonicDuration::from_seconds(60 * 15);
const RECENT_FAILURE_WINDOW: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(60 * 5);
const SHORT_CONNECT_DURATION: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(7 * 60);

#[derive(Clone)]
pub struct ConnectionSelectionRequester {
    sender: mpsc::Sender<ConnectionSelectionRequest>,
}

impl ConnectionSelectionRequester {
    pub fn new(sender: mpsc::Sender<ConnectionSelectionRequest>) -> Self {
        Self { sender }
    }

    pub async fn do_connection_selection(
        &mut self,
        // None if no specific SSID is requested.
        network_id: Option<types::NetworkIdentifier>,
        reason: types::ConnectReason,
    ) -> Result<Option<types::ScannedCandidate>, anyhow::Error> {
        let (sender, receiver) = oneshot::channel();
        self.sender
            .try_send(ConnectionSelectionRequest::NewConnectionSelection {
                network_id,
                reason,
                responder: sender,
            })
            .map_err(|e| format_err!("Failed to send connection selection request: {}", e))?;
        receiver.await.map_err(|e| format_err!("Error during connection selection: {:?}", e))
    }
    pub async fn do_roam_selection(
        &mut self,
        scan_type: fidl_common::ScanType,
        network_id: types::NetworkIdentifier,
        credential: network_config::Credential,
        current_security: types::SecurityTypeDetailed,
    ) -> Result<Option<types::ScannedCandidate>, anyhow::Error> {
        let (sender, receiver) = oneshot::channel();
        self.sender
            .try_send(ConnectionSelectionRequest::RoamSelection {
                scan_type,
                network_id,
                credential,
                current_security,
                responder: sender,
            })
            .map_err(|e| format_err!("Failed to queue connection selection: {}", e))?;
        receiver.await.map_err(|e| format_err!("Error during roam selection: {:?}", e))
    }
}

#[cfg_attr(test, derive(Debug))]
pub enum ConnectionSelectionRequest {
    NewConnectionSelection {
        network_id: Option<types::NetworkIdentifier>,
        reason: types::ConnectReason,
        responder: oneshot::Sender<Option<types::ScannedCandidate>>,
    },
    RoamSelection {
        scan_type: fidl_common::ScanType,
        network_id: types::NetworkIdentifier,
        credential: network_config::Credential,
        current_security: types::SecurityTypeDetailed,
        responder: oneshot::Sender<Option<types::ScannedCandidate>>,
    },
}

// Primary functionality in a trait, so it can be stubbed for tests.
#[async_trait(?Send)]
pub trait ConnectionSelectorApi {
    // Find best connection candidate to initialize a connection.
    async fn find_and_select_connection_candidate(
        &self,
        network: Option<types::NetworkIdentifier>,
        reason: types::ConnectReason,
    ) -> Option<types::ScannedCandidate>;
    // Find best roam candidate.
    async fn find_and_select_roam_candidate(
        &self,
        scan_type: fidl_common::ScanType,
        network: types::NetworkIdentifier,
        credential: &network_config::Credential,
        current_security: types::SecurityTypeDetailed,
    ) -> Option<types::ScannedCandidate>;
}

// Main loop to handle incoming requests.
pub async fn serve_connection_selection_request_loop(
    connection_selector: Rc<dyn ConnectionSelectorApi>,
    mut request_channel: mpsc::Receiver<ConnectionSelectionRequest>,
) {
    loop {
        select! {
            request = request_channel.select_next_some() => {
                match request {
                    ConnectionSelectionRequest::NewConnectionSelection { network_id, reason, responder} => {
                        let selected = connection_selector.find_and_select_connection_candidate(network_id, reason).await;
                        // It's acceptable for the receiver to close the channel, preventing this
                        // sender from responding.
                        let _ = responder.send(selected);
                    }
                    ConnectionSelectionRequest::RoamSelection { scan_type, network_id, credential, current_security, responder } => {
                        let selected = connection_selector.find_and_select_roam_candidate(scan_type, network_id, &credential, current_security).await;
                        // It's acceptable for the receiver to close the channel, preventing this
                        // sender from responding.
                        let _ = responder.send(selected);
                    }
                }
            }
        }
    }
}

pub struct ConnectionSelector {
    saved_network_manager: Arc<dyn SavedNetworksManagerApi>,
    scan_requester: Arc<dyn scan::ScanRequestApi>,
    last_scan_result_time: Arc<Mutex<zx::MonotonicInstant>>,
    _inspect_node_root: Arc<Mutex<InspectNode>>,
    inspect_node_for_connection_selection: Arc<Mutex<InspectBoundedListNode>>,
    telemetry_sender: TelemetrySender,
}

impl ConnectionSelector {
    pub fn new(
        saved_network_manager: Arc<dyn SavedNetworksManagerApi>,
        scan_requester: Arc<dyn scan::ScanRequestApi>,
        inspect_node: InspectNode,
        telemetry_sender: TelemetrySender,
    ) -> Self {
        let inspect_node_for_connection_selection = InspectBoundedListNode::new(
            inspect_node.create_child("connection_selection"),
            INSPECT_EVENT_LIMIT_FOR_CONNECTION_SELECTIONS,
        );
        Self {
            saved_network_manager,
            scan_requester,
            last_scan_result_time: Arc::new(Mutex::new(zx::MonotonicInstant::ZERO)),
            _inspect_node_root: Arc::new(Mutex::new(inspect_node)),
            inspect_node_for_connection_selection: Arc::new(Mutex::new(
                inspect_node_for_connection_selection,
            )),
            telemetry_sender,
        }
    }

    /// Requests scans and compiles list of BSSs that appear in scan results and belong to currently
    /// saved networks.
    async fn find_available_bss_candidate_list(
        &self,
        network: Option<types::NetworkIdentifier>,
    ) -> Vec<types::ScannedCandidate> {
        let scan_for_candidates = || async {
            if let Some(network) = &network {
                self.scan_requester
                    .perform_scan(ScanReason::BssSelection, vec![network.ssid.clone()], vec![])
                    .await
            } else {
                let last_scan_result_time = *self.last_scan_result_time.lock().await;
                let scan_age = zx::MonotonicInstant::get() - last_scan_result_time;
                if last_scan_result_time != zx::MonotonicInstant::ZERO {
                    info!("Scan results are {}s old, triggering a scan", scan_age.into_seconds());
                    self.telemetry_sender.send(TelemetryEvent::NetworkSelectionScanInterval {
                        time_since_last_scan: scan_age,
                    });
                }
                let passive_scan_results = match self
                    .scan_requester
                    .perform_scan(ScanReason::NetworkSelection, vec![], vec![])
                    .await
                {
                    Ok(scan_results) => scan_results,
                    Err(e) => return Err(e),
                };
                let passive_scan_ssids: HashSet<types::Ssid> = HashSet::from_iter(
                    passive_scan_results.iter().map(|result| result.ssid.clone()),
                );
                let requested_active_scan_ssids: Vec<types::Ssid> =
                    select_subset_potentially_hidden_networks(
                        self.saved_network_manager.get_networks().await,
                    )
                    .drain(..)
                    .map(|id| id.ssid)
                    .filter(|ssid| !passive_scan_ssids.contains(ssid))
                    .collect();

                self.telemetry_sender.send(TelemetryEvent::ActiveScanRequested {
                    num_ssids_requested: requested_active_scan_ssids.len(),
                });

                if requested_active_scan_ssids.is_empty() {
                    Ok(passive_scan_results)
                } else {
                    self.scan_requester
                        .perform_scan(
                            ScanReason::NetworkSelection,
                            requested_active_scan_ssids,
                            vec![],
                        )
                        .await
                        .map(|mut scan_results| {
                            scan_results.extend(passive_scan_results);
                            scan_results
                        })
                }
            }
        };

        let scan_results = scan_for_candidates().await;

        match scan_results {
            Err(e) => {
                warn!("Failed to get available BSSs, {:?}", e);
                vec![]
            }
            Ok(scan_results) => {
                let candidates =
                    merge_saved_networks_and_scan_data(&self.saved_network_manager, scan_results)
                        .await;
                if network.is_none() {
                    *self.last_scan_result_time.lock().await = zx::MonotonicInstant::get();
                    record_metrics_on_scan(candidates.clone(), &self.telemetry_sender);
                }
                candidates
            }
        }
    }

    /// If a BSS was discovered via a passive scan, we need to perform an active scan on it to
    /// discover all the information potentially needed by the SME layer.
    async fn augment_bss_candidate_with_active_scan(
        &self,
        scanned_candidate: types::ScannedCandidate,
    ) -> types::ScannedCandidate {
        // This internal function encapsulates all the logic and has a Result<> return type,
        // allowing us to use the `?` operator inside it to reduce nesting.
        async fn get_enhanced_bss_description(
            scanned_candidate: &types::ScannedCandidate,
            scan_requester: Arc<dyn scan::ScanRequestApi>,
        ) -> Result<Sequestered<fidl_fuchsia_wlan_ieee80211::BssDescription>, ()> {
            match scanned_candidate.bss.observation {
                types::ScanObservation::Passive => {
                    info!("Performing directed active scan on selected network")
                }
                types::ScanObservation::Active => {
                    debug!("Network already discovered via active scan.");
                    return Err(());
                }
                types::ScanObservation::Unknown => {
                    error!("Unexpected `Unknown` variant of network `observation`.");
                    return Err(());
                }
            }

            // Perform the scan
            let mut directed_scan_result = scan_requester
                .perform_scan(
                    ScanReason::BssSelectionAugmentation,
                    vec![scanned_candidate.network.ssid.clone()],
                    vec![scanned_candidate.bss.channel],
                )
                .await
                .map_err(|_| {
                    info!("Failed to perform active scan to augment BSS info.");
                })?;

            // Find the bss in the results
            let bss_description = directed_scan_result
                .drain(..)
                .find_map(|mut network| {
                    if network.ssid == scanned_candidate.network.ssid {
                        for bss in network.entries.drain(..) {
                            if bss.bssid == scanned_candidate.bss.bssid {
                                return Some(bss.bss_description);
                            }
                        }
                    }
                    None
                })
                .ok_or_else(|| {
                    info!("BSS info will lack active scan augmentation, proceeding anyway.");
                })?;

            Ok(bss_description)
        }

        match get_enhanced_bss_description(&scanned_candidate, self.scan_requester.clone()).await {
            Ok(new_bss_description) => {
                let updated_scanned_bss =
                    Bss { bss_description: new_bss_description, ..scanned_candidate.bss.clone() };
                types::ScannedCandidate { bss: updated_scanned_bss, ..scanned_candidate }
            }
            Err(()) => scanned_candidate,
        }
    }

    // Find scan results in provided network
    async fn roam_scan(
        &self,
        scan_type: fidl_common::ScanType,
        network: types::NetworkIdentifier,
        current_security: types::SecurityTypeDetailed,
    ) -> Vec<types::ScanResult> {
        let ssids = match scan_type {
            fidl_common::ScanType::Passive => vec![],
            fidl_common::ScanType::Active => vec![network.ssid.clone()],
        };
        self.scan_requester
            .perform_scan(ScanReason::RoamSearch, ssids, vec![])
            .await
            .unwrap_or_else(|e| {
                error!("{}", format_err!("Error scanning: {:?}", e));
                vec![]
            })
            .into_iter()
            .filter(|s| {
                // Roaming is only allowed between APs with identical protection
                s.ssid == network.ssid && current_security == s.security_type_detailed
            })
            .collect::<Vec<_>>()
    }
}

#[async_trait(?Send)]
impl ConnectionSelectorApi for ConnectionSelector {
    /// Full connection selection. Scans to find available candidates, uses network selection (or
    /// optional provided network) to filter out networks, and then bss selection to select the best
    /// of the remaining candidates. If the candidate was discovered via a passive scan, augments the
    /// bss info with an active scan.
    async fn find_and_select_connection_candidate(
        &self,
        network: Option<types::NetworkIdentifier>,
        reason: types::ConnectReason,
    ) -> Option<types::ScannedCandidate> {
        // Scan for BSSs belonging to saved networks.
        let available_candidate_list =
            self.find_available_bss_candidate_list(network.clone()).await;

        // Network selection.
        let available_networks: HashSet<types::NetworkIdentifier> =
            available_candidate_list.iter().map(|candidate| candidate.network.clone()).collect();
        let selected_networks = network_selection::select_networks(available_networks, &network);

        // Send network selection metrics
        self.telemetry_sender.send(TelemetryEvent::NetworkSelectionDecision {
            network_selection_type: match network {
                Some(_) => telemetry::NetworkSelectionType::Directed,
                None => telemetry::NetworkSelectionType::Undirected,
            },
            num_candidates: (!available_candidate_list.is_empty())
                .then_some(available_candidate_list.len())
                .ok_or(()),
            selected_count: selected_networks.len(),
        });

        // Filter down to only BSSs in the selected networks.
        let allowed_candidate_list = available_candidate_list
            .iter()
            .filter(|candidate| selected_networks.contains(&candidate.network))
            .cloned()
            .collect();

        // BSS selection.

        match bss_selection::select_bss(
            allowed_candidate_list,
            reason,
            self.inspect_node_for_connection_selection.clone(),
            self.telemetry_sender.clone(),
        )
        .await
        {
            Some(mut candidate) => {
                if network.is_some() {
                    // Strip scan observation type, because the candidate was discovered via a
                    // directed active scan, so we cannot know if it is discoverable via a passive
                    // scan.
                    candidate.bss.observation = types::ScanObservation::Unknown;
                }
                // If it was observed passively, augment with active scan.
                match candidate.bss.observation {
                    types::ScanObservation::Passive => {
                        Some(self.augment_bss_candidate_with_active_scan(candidate.clone()).await)
                    }
                    _ => Some(candidate),
                }
            }
            None => None,
        }
    }

    /// Return the "best" AP to connect to from the current network. It may be the same AP that is
    /// currently connected. Returning None means that no APs were seen. The credential is required
    /// to ensure the network config matches.
    async fn find_and_select_roam_candidate(
        &self,
        scan_type: fidl_common::ScanType,
        network: types::NetworkIdentifier,
        credential: &network_config::Credential,
        current_security: types::SecurityTypeDetailed,
    ) -> Option<types::ScannedCandidate> {
        // Scan for APs in the provided network.
        let mut matching_scan_results =
            self.roam_scan(scan_type, network.clone(), current_security).await;
        if matching_scan_results.is_empty() && scan_type == fidl_common::ScanType::Passive {
            info!("No scan results seen in passive roam scan. Active scanning.");
            matching_scan_results = self
                .roam_scan(fidl_common::ScanType::Active, network.clone(), current_security)
                .await;
        }

        let mut candidates = Vec::new();
        // All APs should be grouped in the same scan result based on SSID and security,
        // but combine all if there are multiple scan results.
        for mut s in matching_scan_results {
            if let Some(config) = self
                .saved_network_manager
                .lookup(&network)
                .await
                .filter(|c| &c.credential == credential)
            {
                candidates.append(&mut merge_config_and_scan_data(config, &mut s));
            } else {
                // This should only happen if the network config was just removed as the scan
                // happened.
                warn!("Failed to find config for network to roam from");
            }
        }
        // Choose the best AP
        bss_selection::select_bss(
            candidates,
            types::ConnectReason::ProactiveNetworkSwitch,
            self.inspect_node_for_connection_selection.clone(),
            self.telemetry_sender.clone(),
        )
        .await
    }
}

impl types::ScannedCandidate {
    pub fn recent_failure_count(&self) -> u64 {
        self.saved_network_info
            .recent_failures
            .iter()
            .filter(|failure| failure.bssid == self.bss.bssid)
            .count()
            .try_into()
            .unwrap_or_else(|e| {
                error!("{}", e);
                u64::MAX
            })
    }
    pub fn recent_short_connections(&self) -> usize {
        self.saved_network_info
            .past_connections
            .get(&self.bss.bssid)
            .cloned()
            .unwrap_or_default()
            .get_recent(fasync::MonotonicInstant::now() - RECENT_DISCONNECT_WINDOW)
            .iter()
            .filter(|d| d.connection_uptime < SHORT_CONNECT_DURATION)
            .count()
    }

    pub fn saved_security_type_to_string(&self) -> String {
        match self.network.security_type {
            SecurityType::None => "open",
            SecurityType::Wep => "WEP",
            SecurityType::Wpa => "WPA1",
            SecurityType::Wpa2 => "WPA2",
            SecurityType::Wpa3 => "WPA3",
        }
        .to_string()
    }

    pub fn scanned_security_type_to_string(&self) -> String {
        match self.security_type_detailed {
            SecurityTypeDetailed::Unknown => "unknown",
            SecurityTypeDetailed::Open => "open",
            SecurityTypeDetailed::OpenOweTransition => "Open OWE Transition",
            SecurityTypeDetailed::Owe => "OWE",
            SecurityTypeDetailed::Wep => "WEP",
            SecurityTypeDetailed::Wpa1 => "WPA1",
            SecurityTypeDetailed::Wpa1Wpa2PersonalTkipOnly => "WPA1/2Tk",
            SecurityTypeDetailed::Wpa2PersonalTkipOnly => "WPA2Tk",
            SecurityTypeDetailed::Wpa1Wpa2Personal => "WPA1/2",
            SecurityTypeDetailed::Wpa2Personal => "WPA2",
            SecurityTypeDetailed::Wpa2Wpa3Personal => "WPA2/3",
            SecurityTypeDetailed::Wpa3Personal => "WPA3",
            SecurityTypeDetailed::Wpa2Enterprise => "WPA2Ent",
            SecurityTypeDetailed::Wpa3Enterprise => "WPA3Ent",
        }
        .to_string()
    }

    pub fn to_string_without_pii(&self) -> String {
        let channel = self.bss.channel;
        let rssi = self.bss.signal.rssi_dbm;
        let recent_failure_count = self.recent_failure_count();
        let recent_short_connection_count = self.recent_short_connections();

        format!(
            "{}({:4}), {}({:6}), {:>4}dBm, channel {:8}, score {:4}{}{}{}{}",
            self.network.ssid,
            self.saved_security_type_to_string(),
            self.bss.bssid,
            self.scanned_security_type_to_string(),
            rssi,
            channel,
            scoring_functions::score_bss_scanned_candidate(self.clone()),
            if !self.bss.is_compatible() { ", NOT compatible" } else { "" },
            if recent_failure_count > 0 {
                format!(", {recent_failure_count} recent failures")
            } else {
                "".to_string()
            },
            if recent_short_connection_count > 0 {
                format!(", {recent_short_connection_count} recent short disconnects")
            } else {
                "".to_string()
            },
            if !self.saved_network_info.has_ever_connected { ", never used yet" } else { "" },
        )
    }
}

impl WriteInspect for types::ScannedCandidate {
    fn write_inspect<'a>(&self, writer: &InspectNode, key: impl Into<Cow<'a, str>>) {
        inspect_insert!(writer, var key: {
            ssid: self.network.ssid.to_string(),
            bssid: self.bss.bssid.to_string(),
            rssi: self.bss.signal.rssi_dbm,
            score: scoring_functions::score_bss_scanned_candidate(self.clone()),
            security_type_saved: self.saved_security_type_to_string(),
            security_type_scanned: format!(
                "{}",
                wlan_common::bss::Protection::from(self.security_type_detailed),
            ),
            channel: format!("{}", self.bss.channel),
            compatible: self.bss.is_compatible(),
            incompatibility: self.bss
                .compatibility
                .as_ref()
                .err()
                .map(ToString::to_string)
                .unwrap_or_else(|| String::from("none")),
            recent_failure_count: self.recent_failure_count(),
            saved_network_has_ever_connected: self.saved_network_info.has_ever_connected,
        });
    }
}

fn get_authenticator(bss: &Bss, credential: &Credential) -> Option<SecurityAuthenticator> {
    let mutual_security_protocols = match bss.compatibility.as_ref() {
        Ok(compatible) => compatible.mutual_security_protocols().clone(),
        Err(incompatible) => {
            error!("BSS ({:?}) is incompatible: {}", bss.bssid, incompatible);
            return None;
        }
    };

    match select_authentication_method(mutual_security_protocols.clone(), credential) {
        Some(authenticator) => Some(authenticator),
        None => {
            error!(
                "Failed to negotiate authentication for BSS ({:?}) with mutually supported \
                 security protocols: {:?}, and credential type: {:?}.",
                bss.bssid,
                mutual_security_protocols,
                credential.type_str()
            );
            None
        }
    }
}

fn merge_config_and_scan_data(
    network_config: config_management::NetworkConfig,
    scan_result: &mut types::ScanResult,
) -> Vec<types::ScannedCandidate> {
    if network_config.ssid != scan_result.ssid
        || !network_config
            .security_type
            .is_compatible_with_scanned_type(&scan_result.security_type_detailed)
    {
        return Vec::new();
    }

    let mut merged_networks = Vec::new();
    let multiple_bss_candidates = scan_result.entries.len() > 1;

    for bss in scan_result.entries.iter() {
        let authenticator = match get_authenticator(bss, &network_config.credential) {
            Some(authenticator) => authenticator,
            None => {
                error!(
                    "Failed to create authenticator for bss candidate {:?} (SSID: {:?}). Removing from candidates.",
                    bss.bssid, network_config.ssid
                );
                continue;
            }
        };
        let scanned_candidate = types::ScannedCandidate {
            network: types::NetworkIdentifier {
                ssid: network_config.ssid.clone(),
                security_type: network_config.security_type,
            },
            security_type_detailed: scan_result.security_type_detailed,
            credential: network_config.credential.clone(),
            network_has_multiple_bss: multiple_bss_candidates,
            saved_network_info: InternalSavedNetworkData {
                has_ever_connected: network_config.has_ever_connected,
                recent_failures: network_config.get_recent_connection_failures(
                    fasync::MonotonicInstant::now() - RECENT_FAILURE_WINDOW,
                ),
                past_connections: network_config.perf_stats.past_connections.clone(),
            },
            bss: bss.clone(),
            authenticator,
        };
        merged_networks.push(scanned_candidate)
    }
    merged_networks
}

/// Merge the saved networks and scan results into a vector of BSS candidates that correspond to a
/// saved network.
async fn merge_saved_networks_and_scan_data(
    saved_network_manager: &Arc<dyn SavedNetworksManagerApi>,
    mut scan_results: Vec<types::ScanResult>,
) -> Vec<types::ScannedCandidate> {
    let mut merged_networks = vec![];
    for mut scan_result in scan_results.drain(..) {
        for saved_config in saved_network_manager
            .lookup_compatible(&scan_result.ssid, scan_result.security_type_detailed)
            .await
        {
            let multiple_bss_candidates = scan_result.entries.len() > 1;
            for bss in scan_result.entries.drain(..) {
                let authenticator = match get_authenticator(&bss, &saved_config.credential) {
                    Some(authenticator) => authenticator,
                    None => {
                        error!(
                            "Failed to create authenticator for bss candidate {} (SSID: {}). Removing from candidates.",
                            bss.bssid, saved_config.ssid
                        );
                        continue;
                    }
                };
                let scanned_candidate = types::ScannedCandidate {
                    network: types::NetworkIdentifier {
                        ssid: saved_config.ssid.clone(),
                        security_type: saved_config.security_type,
                    },
                    security_type_detailed: scan_result.security_type_detailed,
                    credential: saved_config.credential.clone(),
                    network_has_multiple_bss: multiple_bss_candidates,
                    saved_network_info: InternalSavedNetworkData {
                        has_ever_connected: saved_config.has_ever_connected,
                        recent_failures: saved_config.get_recent_connection_failures(
                            fasync::MonotonicInstant::now() - RECENT_FAILURE_WINDOW,
                        ),
                        past_connections: saved_config.perf_stats.past_connections.clone(),
                    },
                    bss,
                    authenticator,
                };
                merged_networks.push(scanned_candidate)
            }
        }
    }
    merged_networks
}

fn record_metrics_on_scan(
    mut merged_networks: Vec<types::ScannedCandidate>,
    telemetry_sender: &TelemetrySender,
) {
    let mut merged_network_map: HashMap<types::NetworkIdentifier, Vec<types::ScannedCandidate>> =
        HashMap::new();
    for bss in merged_networks.drain(..) {
        merged_network_map.entry(bss.network.clone()).or_default().push(bss);
    }

    let num_saved_networks_observed = merged_network_map.len();
    let mut num_actively_scanned_networks = 0;
    let bss_count_per_saved_network = merged_network_map
        .values()
        .map(|bsss| {
            // Check if the network was found via active scan.
            if bsss.iter().any(|bss| matches!(bss.bss.observation, types::ScanObservation::Active))
            {
                num_actively_scanned_networks += 1;
            };
            // Count how many BSSs are visible in the scan results for this saved network.
            bsss.len()
        })
        .collect();

    telemetry_sender.send(TelemetryEvent::ConnectionSelectionScanResults {
        saved_network_count: num_saved_networks_observed,
        bss_count_per_saved_network,
        saved_network_count_found_by_active_scan: num_actively_scanned_networks,
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_management::{ConnectFailure, FailureReason, SavedNetworksManager};
    use crate::util::testing::fakes::{FakeSavedNetworksManager, FakeScanRequester};
    use crate::util::testing::{
        generate_channel, generate_random_bss, generate_random_connect_reason,
        generate_random_network_identifier, generate_random_password, generate_random_scan_result,
        generate_random_scanned_candidate,
    };
    use assert_matches::assert_matches;
    use diagnostics_assertions::{AnyNumericProperty, assert_data_tree};
    use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
    use fidl_fuchsia_wlan_sme as fidl_sme;
    use fuchsia_async as fasync;
    use fuchsia_inspect as inspect;
    use futures::task::Poll;
    use ieee80211_testutils::BSSID_REGEX;
    use std::pin::pin;
    use std::rc::Rc;
    use std::sync::LazyLock;
    use test_case::test_case;
    use wlan_common::bss::BssDescription;
    use wlan_common::random_fidl_bss_description;
    use wlan_common::scan::Compatible;
    use wlan_common::security::SecurityDescriptor;

    pub static TEST_PASSWORD: LazyLock<Credential> =
        LazyLock::new(|| Credential::Password(b"password".to_vec()));

    struct TestValues {
        connection_selector: Rc<ConnectionSelector>,
        real_saved_network_manager: Arc<dyn SavedNetworksManagerApi>,
        saved_network_manager: Arc<FakeSavedNetworksManager>,
        scan_requester: Arc<FakeScanRequester>,
        inspector: inspect::Inspector,
        telemetry_receiver: mpsc::Receiver<TelemetryEvent>,
    }

    async fn test_setup(use_real_save_network_manager: bool) -> TestValues {
        let real_saved_network_manager = Arc::new(SavedNetworksManager::new_for_test().await);
        let saved_network_manager = Arc::new(FakeSavedNetworksManager::new());
        let scan_requester = Arc::new(FakeScanRequester::new());
        let inspector = inspect::Inspector::default();
        let inspect_node = inspector.root().create_child("connection_selection_test");
        let (telemetry_sender, telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);

        let connection_selector = Rc::new(ConnectionSelector::new(
            if use_real_save_network_manager {
                real_saved_network_manager.clone()
            } else {
                saved_network_manager.clone()
            },
            scan_requester.clone(),
            inspect_node,
            TelemetrySender::new(telemetry_sender),
        ));

        TestValues {
            connection_selector,
            real_saved_network_manager,
            saved_network_manager,
            scan_requester,
            inspector,
            telemetry_receiver,
        }
    }

    fn fake_successful_connect_result() -> fidl_sme::ConnectResult {
        fidl_sme::ConnectResult {
            code: fidl_ieee80211::StatusCode::Success,
            is_credential_rejected: false,
            is_reconnect: false,
        }
    }

    #[fuchsia::test]
    async fn scan_results_merged_with_saved_networks() {
        let test_values = test_setup(true).await;

        // create some identifiers
        let test_ssid_1 = types::Ssid::try_from("foo").unwrap();
        let test_security_1 = types::SecurityTypeDetailed::Wpa3Personal;
        let test_id_1 = types::NetworkIdentifier {
            ssid: test_ssid_1.clone(),
            security_type: types::SecurityType::Wpa3,
        };
        let credential_1 = Credential::Password("foo_pass".as_bytes().to_vec());
        let test_ssid_2 = types::Ssid::try_from("bar").unwrap();
        let test_security_2 = types::SecurityTypeDetailed::Wpa1;
        let test_id_2 = types::NetworkIdentifier {
            ssid: test_ssid_2.clone(),
            security_type: types::SecurityType::Wpa,
        };
        let credential_2 = Credential::Password("bar_pass".as_bytes().to_vec());

        // insert the saved networks
        assert!(
            test_values
                .real_saved_network_manager
                .store(test_id_1.clone(), credential_1.clone())
                .await
                .unwrap()
                .is_none()
        );

        assert!(
            test_values
                .real_saved_network_manager
                .store(test_id_2.clone(), credential_2.clone())
                .await
                .unwrap()
                .is_none()
        );

        // build some scan results
        let mock_scan_results = vec![
            types::ScanResult {
                ssid: test_ssid_1.clone(),
                security_type_detailed: test_security_1,
                entries: vec![
                    types::Bss {
                        compatibility: Compatible::expect_ok([SecurityDescriptor::WPA3_PERSONAL]),
                        ..generate_random_bss()
                    },
                    types::Bss {
                        compatibility: Compatible::expect_ok([SecurityDescriptor::WPA3_PERSONAL]),
                        ..generate_random_bss()
                    },
                    types::Bss {
                        compatibility: Compatible::expect_ok([SecurityDescriptor::WPA3_PERSONAL]),
                        ..generate_random_bss()
                    },
                ],
                compatibility: types::Compatibility::Supported,
            },
            types::ScanResult {
                ssid: test_ssid_2.clone(),
                security_type_detailed: test_security_2,
                entries: vec![types::Bss {
                    compatibility: Compatible::expect_ok([SecurityDescriptor::WPA1]),
                    ..generate_random_bss()
                }],
                compatibility: types::Compatibility::DisallowedNotSupported,
            },
        ];

        let bssid_1 = mock_scan_results[0].entries[0].bssid;
        let bssid_2 = mock_scan_results[0].entries[1].bssid;

        // mark the first one as having connected
        test_values
            .real_saved_network_manager
            .record_connect_result(
                test_id_1.clone(),
                &credential_1.clone(),
                bssid_1,
                fake_successful_connect_result(),
                types::ScanObservation::Unknown,
            )
            .await;

        // mark the second one as having a failure
        test_values
            .real_saved_network_manager
            .record_connect_result(
                test_id_1.clone(),
                &credential_1.clone(),
                bssid_2,
                fidl_sme::ConnectResult {
                    code: fidl_ieee80211::StatusCode::RefusedReasonUnspecified,
                    is_credential_rejected: true,
                    ..fake_successful_connect_result()
                },
                types::ScanObservation::Unknown,
            )
            .await;

        // build our expected result
        let saved_config = test_values
            .real_saved_network_manager
            .lookup(&test_id_1.clone())
            .await
            .expect("failed to get config");
        let failure_time = saved_config
            .get_recent_connection_failures(fasync::MonotonicInstant::now() - RECENT_FAILURE_WINDOW)
            .first()
            .expect("failed to get recent failure")
            .time;
        let recent_failures = vec![ConnectFailure {
            bssid: bssid_2,
            time: failure_time,
            reason: FailureReason::CredentialRejected,
        }];
        let expected_internal_data_1 = InternalSavedNetworkData {
            has_ever_connected: true,
            recent_failures: recent_failures.clone(),
            past_connections: HashMap::new(),
        };
        let wpa3_authenticator = select_authentication_method(
            HashSet::from([SecurityDescriptor::WPA3_PERSONAL]),
            &credential_1,
        )
        .unwrap();
        let open_authenticator =
            select_authentication_method(HashSet::from([SecurityDescriptor::WPA1]), &credential_2)
                .unwrap();
        let expected_results = vec![
            types::ScannedCandidate {
                network: test_id_1.clone(),
                credential: credential_1.clone(),
                network_has_multiple_bss: true,
                security_type_detailed: test_security_1,
                saved_network_info: expected_internal_data_1.clone(),
                bss: mock_scan_results[0].entries[0].clone(),
                authenticator: wpa3_authenticator.clone(),
            },
            types::ScannedCandidate {
                network: test_id_1.clone(),
                credential: credential_1.clone(),
                network_has_multiple_bss: true,
                security_type_detailed: test_security_1,
                saved_network_info: expected_internal_data_1.clone(),
                bss: mock_scan_results[0].entries[1].clone(),
                authenticator: wpa3_authenticator.clone(),
            },
            types::ScannedCandidate {
                network: test_id_1.clone(),
                credential: credential_1.clone(),
                network_has_multiple_bss: true,
                security_type_detailed: test_security_1,
                saved_network_info: expected_internal_data_1.clone(),
                bss: mock_scan_results[0].entries[2].clone(),
                authenticator: wpa3_authenticator.clone(),
            },
            types::ScannedCandidate {
                network: test_id_2.clone(),
                credential: credential_2.clone(),
                network_has_multiple_bss: false,
                security_type_detailed: test_security_2,
                saved_network_info: InternalSavedNetworkData {
                    has_ever_connected: false,
                    recent_failures: Vec::new(),
                    past_connections: HashMap::new(),
                },
                bss: mock_scan_results[1].entries[0].clone(),
                authenticator: open_authenticator.clone(),
            },
        ];

        // validate the function works
        let results = merge_saved_networks_and_scan_data(
            // We're not yet ready to change all our Arc to Rc
            #[allow(clippy::arc_with_non_send_sync)]
            &Arc::new(test_values.real_saved_network_manager),
            mock_scan_results,
        )
        .await;

        assert_eq!(results, expected_results);
    }

    #[fuchsia::test]
    fn augment_bss_candidate_with_active_scan_doesnt_run_on_actively_found_networks() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = exec.run_singlethreaded(test_setup(true));
        let mut candidate = generate_random_scanned_candidate();
        candidate.bss.observation = types::ScanObservation::Active;

        let fut = test_values
            .connection_selector
            .augment_bss_candidate_with_active_scan(candidate.clone());
        let mut fut = pin!(fut);

        // The connect_req comes out the other side with no change
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(res) => {
            assert_eq!(&res, &candidate);
        });
    }

    #[fuchsia::test]
    fn augment_bss_candidate_with_active_scan_runs_on_passively_found_networks() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = exec.run_singlethreaded(test_setup(true));

        let mut passively_scanned_candidate = generate_random_scanned_candidate();
        passively_scanned_candidate.bss.observation = types::ScanObservation::Passive;

        let fut = test_values
            .connection_selector
            .augment_bss_candidate_with_active_scan(passively_scanned_candidate.clone());
        let fut = pin!(fut);

        // Set the scan results
        let new_bss_desc = random_fidl_bss_description!();
        exec.run_singlethreaded(test_values.scan_requester.add_scan_result(Ok(vec![
            types::ScanResult {
                ssid: passively_scanned_candidate.network.ssid.clone(),
                security_type_detailed: passively_scanned_candidate.security_type_detailed,
                compatibility: types::Compatibility::Supported,
                entries: vec![types::Bss {
                    bssid: passively_scanned_candidate.bss.bssid,
                    compatibility: wlan_common::scan::Compatible::expect_ok([
                        wlan_common::security::SecurityDescriptor::WPA1,
                    ]),
                    bss_description: new_bss_desc.clone().into(),
                    ..generate_random_bss()
                }],
            },
        ])));

        let candidate = exec.run_singlethreaded(fut);
        // The connect_req comes out the other side with the new bss_description
        assert_eq!(
            &candidate,
            &types::ScannedCandidate {
                bss: types::Bss {
                    bss_description: new_bss_desc.into(),
                    ..passively_scanned_candidate.bss.clone()
                },
                ..passively_scanned_candidate.clone()
            },
        );

        // Check the right scan request was sent
        assert_eq!(
            *exec.run_singlethreaded(test_values.scan_requester.scan_requests.lock()),
            vec![(
                ScanReason::BssSelectionAugmentation,
                vec![passively_scanned_candidate.network.ssid.clone()],
                vec![passively_scanned_candidate.bss.channel]
            )]
        );
    }

    #[fuchsia::test]
    fn find_available_bss_list_with_network_specified() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = exec.run_singlethreaded(test_setup(false));
        let connection_selector = test_values.connection_selector;

        // create identifiers
        let test_id_1 = types::NetworkIdentifier {
            ssid: types::Ssid::try_from("foo").unwrap(),
            security_type: types::SecurityType::Wpa3,
        };
        let credential_1 = Credential::Password("foo_pass".as_bytes().to_vec());

        // insert saved networks
        assert!(
            exec.run_singlethreaded(
                test_values.saved_network_manager.store(test_id_1.clone(), credential_1.clone()),
            )
            .unwrap()
            .is_none()
        );

        // Prep the scan results
        let mutual_security_protocols_1 = [SecurityDescriptor::WPA3_PERSONAL];
        let bss_desc_1 = random_fidl_bss_description!();
        exec.run_singlethreaded(test_values.scan_requester.add_scan_result(Ok(vec![
            types::ScanResult {
                ssid: test_id_1.ssid.clone(),
                security_type_detailed: types::SecurityTypeDetailed::Wpa3Personal,
                compatibility: types::Compatibility::Supported,
                entries: vec![types::Bss {
                    compatibility: wlan_common::scan::Compatible::expect_ok(
                        mutual_security_protocols_1,
                    ),
                    bss_description: bss_desc_1.clone().into(),
                    ..generate_random_bss()
                }],
            },
            generate_random_scan_result(),
            generate_random_scan_result(),
        ])));

        // Run the scan, specifying the desired network
        let fut = connection_selector.find_available_bss_candidate_list(Some(test_id_1.clone()));
        let mut fut = pin!(fut);
        let results = exec.run_singlethreaded(&mut fut);
        assert_eq!(results.len(), 1);

        // Check that the right scan request was sent
        assert_eq!(
            *exec.run_singlethreaded(test_values.scan_requester.scan_requests.lock()),
            vec![(ScanReason::BssSelection, vec![test_id_1.ssid.clone()], vec![])]
        );
    }

    #[test_case(true)]
    #[test_case(false)]
    #[fuchsia::test(add_test_attr = false)]
    fn find_available_bss_list_without_network_specified(hidden: bool) {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = exec.run_singlethreaded(test_setup(false));
        let connection_selector = test_values.connection_selector;

        // create identifiers
        let test_id_not_hidden = types::NetworkIdentifier {
            ssid: types::Ssid::try_from("foo").unwrap(),
            security_type: types::SecurityType::Wpa3,
        };
        let test_id_maybe_hidden = types::NetworkIdentifier {
            ssid: types::Ssid::try_from("bar").unwrap(),
            security_type: types::SecurityType::Wpa3,
        };
        let test_id_hidden_but_seen = types::NetworkIdentifier {
            ssid: types::Ssid::try_from("baz").unwrap(),
            security_type: types::SecurityType::Wpa3,
        };
        let credential = Credential::Password("some_pass".as_bytes().to_vec());

        // insert saved networks
        assert!(
            exec.run_singlethreaded(
                test_values
                    .saved_network_manager
                    .store(test_id_not_hidden.clone(), credential.clone()),
            )
            .unwrap()
            .is_none()
        );
        assert!(
            exec.run_singlethreaded(
                test_values
                    .saved_network_manager
                    .store(test_id_maybe_hidden.clone(), credential.clone()),
            )
            .unwrap()
            .is_none()
        );
        assert!(
            exec.run_singlethreaded(
                test_values
                    .saved_network_manager
                    .store(test_id_hidden_but_seen.clone(), credential.clone()),
            )
            .unwrap()
            .is_none()
        );

        // Set the hidden probability for test_id_not_hidden and test_id_hidden_but_seen
        exec.run_singlethreaded(
            test_values.saved_network_manager.update_hidden_prob(test_id_not_hidden.clone(), 0.0),
        );
        exec.run_singlethreaded(
            test_values
                .saved_network_manager
                .update_hidden_prob(test_id_hidden_but_seen.clone(), 1.0),
        );
        // Set the hidden probability for test_id_maybe_hidden based on test variant
        exec.run_singlethreaded(
            test_values
                .saved_network_manager
                .update_hidden_prob(test_id_maybe_hidden.clone(), if hidden { 1.0 } else { 0.0 }),
        );

        // Prep the scan results
        let mutual_security_protocols = [SecurityDescriptor::WPA3_PERSONAL];
        let bss_desc = random_fidl_bss_description!();
        if hidden {
            // Initial passive scan has non-hidden result and hidden-but-seen result
            exec.run_singlethreaded(test_values.scan_requester.add_scan_result(Ok(vec![
                types::ScanResult {
                    ssid: test_id_not_hidden.ssid.clone(),
                    security_type_detailed: types::SecurityTypeDetailed::Wpa3Personal,
                    compatibility: types::Compatibility::Supported,
                    entries: vec![types::Bss {
                        compatibility: wlan_common::scan::Compatible::expect_ok(
                            mutual_security_protocols,
                        ),
                        bss_description: bss_desc.clone().into(),
                        ..generate_random_bss()
                    }],
                },
                types::ScanResult {
                    ssid: test_id_hidden_but_seen.ssid.clone(),
                    security_type_detailed: types::SecurityTypeDetailed::Wpa3Personal,
                    compatibility: types::Compatibility::Supported,
                    entries: vec![types::Bss {
                        compatibility: wlan_common::scan::Compatible::expect_ok(
                            mutual_security_protocols,
                        ),
                        bss_description: bss_desc.clone().into(),
                        ..generate_random_bss()
                    }],
                },
                generate_random_scan_result(),
                generate_random_scan_result(),
            ])));
            // Next active scan has hidden result
            exec.run_singlethreaded(test_values.scan_requester.add_scan_result(Ok(vec![
                types::ScanResult {
                    ssid: test_id_maybe_hidden.ssid.clone(),
                    security_type_detailed: types::SecurityTypeDetailed::Wpa3Personal,
                    compatibility: types::Compatibility::Supported,
                    entries: vec![types::Bss {
                        compatibility: wlan_common::scan::Compatible::expect_ok(
                            mutual_security_protocols,
                        ),
                        bss_description: bss_desc.clone().into(),
                        ..generate_random_bss()
                    }],
                },
                generate_random_scan_result(),
                generate_random_scan_result(),
            ])));
        } else {
            // Non-hidden test case, only one scan, return all results
            exec.run_singlethreaded(test_values.scan_requester.add_scan_result(Ok(vec![
                types::ScanResult {
                    ssid: test_id_not_hidden.ssid.clone(),
                    security_type_detailed: types::SecurityTypeDetailed::Wpa3Personal,
                    compatibility: types::Compatibility::Supported,
                    entries: vec![types::Bss {
                        compatibility: wlan_common::scan::Compatible::expect_ok(
                            mutual_security_protocols,
                        ),
                        bss_description: bss_desc.clone().into(),
                        ..generate_random_bss()
                    }],
                },
                types::ScanResult {
                    ssid: test_id_maybe_hidden.ssid.clone(),
                    security_type_detailed: types::SecurityTypeDetailed::Wpa3Personal,
                    compatibility: types::Compatibility::Supported,
                    entries: vec![types::Bss {
                        compatibility: wlan_common::scan::Compatible::expect_ok(
                            mutual_security_protocols,
                        ),
                        bss_description: bss_desc.clone().into(),
                        ..generate_random_bss()
                    }],
                },
                types::ScanResult {
                    ssid: test_id_hidden_but_seen.ssid.clone(),
                    security_type_detailed: types::SecurityTypeDetailed::Wpa3Personal,
                    compatibility: types::Compatibility::Supported,
                    entries: vec![types::Bss {
                        compatibility: wlan_common::scan::Compatible::expect_ok(
                            mutual_security_protocols,
                        ),
                        bss_description: bss_desc.clone().into(),
                        ..generate_random_bss()
                    }],
                },
                generate_random_scan_result(),
                generate_random_scan_result(),
            ])));
        }

        // Run the scan(s)
        let connection_selection_fut = connection_selector.find_available_bss_candidate_list(None);
        let mut connection_selection_fut = pin!(connection_selection_fut);
        let results = exec.run_singlethreaded(&mut connection_selection_fut);
        assert_eq!(results.len(), 3);

        // Check that the right scan request(s) were sent. The hidden-but-seen SSID should never
        // be requested for the active scan.
        if hidden {
            assert_eq!(
                *exec.run_singlethreaded(test_values.scan_requester.scan_requests.lock()),
                vec![
                    (ScanReason::NetworkSelection, vec![], vec![]),
                    (ScanReason::NetworkSelection, vec![test_id_maybe_hidden.ssid.clone()], vec![])
                ]
            )
        } else {
            assert_eq!(
                *exec.run_singlethreaded(test_values.scan_requester.scan_requests.lock()),
                vec![(ScanReason::NetworkSelection, vec![], vec![])]
            )
        }

        // Check that the metrics were logged
        assert_matches!(
            test_values.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::ActiveScanRequested{num_ssids_requested})) => {
                if hidden {
                        assert_eq!(num_ssids_requested, 1);
                } else {
                        assert_eq!(num_ssids_requested, 0);
                }
        });
    }

    #[fuchsia::test]
    fn find_and_select_connection_candidate_scan_error() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = exec.run_singlethreaded(test_setup(true));
        let connection_selector = test_values.connection_selector;
        let mut telemetry_receiver = test_values.telemetry_receiver;

        // Return an error on the scan
        exec.run_singlethreaded(
            test_values.scan_requester.add_scan_result(Err(types::ScanError::GeneralError)),
        );

        // Kick off network selection
        let mut connection_selection_fut = pin!(
            connection_selector
                .find_and_select_connection_candidate(None, generate_random_connect_reason())
        );
        // Check that nothing is returned
        assert_matches!(exec.run_until_stalled(&mut connection_selection_fut), Poll::Ready(None));

        // Check that the right scan request was sent
        assert_eq!(
            *exec.run_singlethreaded(test_values.scan_requester.scan_requests.lock()),
            vec![(scan::ScanReason::NetworkSelection, vec![], vec![])]
        );

        // Check the network selections were logged
        assert_data_tree!(@executor exec, test_values.inspector, root: {
            connection_selection_test: {
                connection_selection: {
                    "0": {
                        "@time": AnyNumericProperty,
                        "candidates": {},
                    },
                }
            },
        });

        // Verify TelemetryEvent for network selection was sent
        assert_matches!(telemetry_receiver.try_next(), Ok(Some(event)) => {
            assert_matches!(event, TelemetryEvent::NetworkSelectionDecision {
                network_selection_type: telemetry::NetworkSelectionType::Undirected,
                num_candidates: Err(()),
                selected_count: 0,
            });
        });
        assert_matches!(
            telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::BssSelectionResult { selected_candidate: None, .. }))
        );
    }

    #[fuchsia::test]
    fn find_and_select_connection_candidate_end_to_end() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = exec.run_singlethreaded(test_setup(true));
        let connection_selector = test_values.connection_selector;
        let mut telemetry_receiver = test_values.telemetry_receiver;

        // create some identifiers
        let test_id_1 = types::NetworkIdentifier {
            ssid: types::Ssid::try_from("foo").unwrap(),
            security_type: types::SecurityType::Wpa3,
        };
        let credential_1 = Credential::Password("foo_pass".as_bytes().to_vec());
        let bssid_1 = types::Bssid::from([1, 1, 1, 1, 1, 1]);

        let test_id_2 = types::NetworkIdentifier {
            ssid: types::Ssid::try_from("bar").unwrap(),
            security_type: types::SecurityType::Wpa,
        };
        let credential_2 = Credential::Password("bar_pass".as_bytes().to_vec());
        let bssid_2 = types::Bssid::from([2, 2, 2, 2, 2, 2]);

        // insert some new saved networks
        assert!(
            exec.run_singlethreaded(
                test_values
                    .real_saved_network_manager
                    .store(test_id_1.clone(), credential_1.clone()),
            )
            .unwrap()
            .is_none()
        );
        assert!(
            exec.run_singlethreaded(
                test_values
                    .real_saved_network_manager
                    .store(test_id_2.clone(), credential_2.clone()),
            )
            .unwrap()
            .is_none()
        );

        // Mark them as having connected. Make connection passive so that no active scans are made.
        exec.run_singlethreaded(test_values.real_saved_network_manager.record_connect_result(
            test_id_1.clone(),
            &credential_1.clone(),
            bssid_1,
            fake_successful_connect_result(),
            types::ScanObservation::Passive,
        ));
        exec.run_singlethreaded(test_values.real_saved_network_manager.record_connect_result(
            test_id_2.clone(),
            &credential_2.clone(),
            bssid_2,
            fake_successful_connect_result(),
            types::ScanObservation::Passive,
        ));

        // Prep the scan results
        let mutual_security_protocols_1 = [SecurityDescriptor::WPA3_PERSONAL];
        let channel_1 = generate_channel(3, fidl_fuchsia_wlan_ieee80211::WlanBand::TwoGhz);
        let mutual_security_protocols_2 = [SecurityDescriptor::WPA1];
        let channel_2 = generate_channel(50, fidl_fuchsia_wlan_ieee80211::WlanBand::FiveGhz);
        let mock_passive_scan_results = vec![
            types::ScanResult {
                ssid: test_id_1.ssid.clone(),
                security_type_detailed: types::SecurityTypeDetailed::Wpa3Personal,
                compatibility: types::Compatibility::Supported,
                entries: vec![types::Bss {
                    compatibility: wlan_common::scan::Compatible::expect_ok(
                        mutual_security_protocols_1,
                    ),
                    bssid: bssid_1,
                    channel: channel_1,
                    signal: types::Signal {
                        rssi_dbm: -30, // much higher than other result
                        snr_db: 0,
                    },
                    observation: types::ScanObservation::Passive,
                    ..generate_random_bss()
                }],
            },
            types::ScanResult {
                ssid: test_id_2.ssid.clone(),
                security_type_detailed: types::SecurityTypeDetailed::Wpa1,
                compatibility: types::Compatibility::Supported,
                entries: vec![types::Bss {
                    compatibility: wlan_common::scan::Compatible::expect_ok(
                        mutual_security_protocols_2,
                    ),
                    bssid: bssid_2,
                    channel: channel_2,
                    signal: types::Signal {
                        rssi_dbm: -100, // much lower than other result
                        snr_db: 0,
                    },
                    observation: types::ScanObservation::Passive,
                    ..generate_random_bss()
                }],
            },
            generate_random_scan_result(),
            generate_random_scan_result(),
        ];

        // Initial passive scan
        exec.run_singlethreaded(
            test_values.scan_requester.add_scan_result(Ok(mock_passive_scan_results.clone())),
        );

        // An additional directed active scan should be made for the selected network
        let bss_desc1_active = random_fidl_bss_description!();
        let new_bss = types::Bss {
            compatibility: wlan_common::scan::Compatible::expect_ok(mutual_security_protocols_1),
            bssid: bssid_1,
            bss_description: bss_desc1_active.clone().into(),
            ..generate_random_bss()
        };
        exec.run_singlethreaded(test_values.scan_requester.add_scan_result(Ok(vec![
            types::ScanResult {
                ssid: test_id_1.ssid.clone(),
                security_type_detailed: types::SecurityTypeDetailed::Wpa3Personal,
                compatibility: types::Compatibility::Supported,
                entries: vec![new_bss.clone()],
            },
            generate_random_scan_result(),
        ])));

        // Check that we pick a network
        let mut connection_selection_fut = pin!(
            connection_selector
                .find_and_select_connection_candidate(None, generate_random_connect_reason())
        );
        let results =
            exec.run_singlethreaded(&mut connection_selection_fut).expect("no selected candidate");
        assert_eq!(&results.network, &test_id_1.clone());
        assert_eq!(
            &results.bss,
            &types::Bss {
                bss_description: bss_desc1_active.clone().into(),
                ..mock_passive_scan_results[0].entries[0].clone()
            }
        );

        // Check that the right scan requests were sent
        assert_eq!(
            *exec.run_singlethreaded(test_values.scan_requester.scan_requests.lock()),
            vec![
                // Initial passive scan
                (ScanReason::NetworkSelection, vec![], vec![]),
                // Directed active scan should be made for the selected network
                (
                    ScanReason::BssSelectionAugmentation,
                    vec![test_id_1.ssid.clone()],
                    vec![channel_1]
                )
            ]
        );

        // Check the network selections were logged
        assert_data_tree!(@executor exec, test_values.inspector, root: {
            connection_selection_test: {
                connection_selection: {
                    "0": {
                        "@time": AnyNumericProperty,
                        "candidates": {
                            "0": contains {
                                bssid: &*BSSID_REGEX,
                                score: AnyNumericProperty,
                            },
                            "1": contains {
                                bssid: &*BSSID_REGEX,
                                score: AnyNumericProperty,
                            },
                        },
                        "selected": contains {
                            bssid: &*BSSID_REGEX,
                            score: AnyNumericProperty,
                        },
                    },
                }
            },
        });

        // Verify TelemetryEvents for network selection were sent
        assert_matches!(
            telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::ActiveScanRequested { num_ssids_requested: 0 }))
        );
        assert_matches!(
            telemetry_receiver.try_next(), Ok(Some(TelemetryEvent::ConnectionSelectionScanResults {
                saved_network_count, bss_count_per_saved_network, saved_network_count_found_by_active_scan
            })) => {
                assert_eq!(saved_network_count, 2);
                assert_eq!(bss_count_per_saved_network, vec![1, 1]);
                assert_eq!(saved_network_count_found_by_active_scan, 0);
            }
        );
        assert_matches!(telemetry_receiver.try_next(), Ok(Some(event)) => {
            assert_matches!(event, TelemetryEvent::NetworkSelectionDecision {
                network_selection_type: telemetry::NetworkSelectionType::Undirected,
                num_candidates: Ok(2),
                selected_count: 2,
            });
        });
        assert_matches!(
            telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::BssSelectionResult { .. }))
        );
    }

    #[fuchsia::test]
    fn find_and_select_connection_candidate_with_network_end_to_end() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = exec.run_singlethreaded(test_setup(true));
        let connection_selector = test_values.connection_selector;
        let mut telemetry_receiver = test_values.telemetry_receiver;

        // create identifiers
        let test_id_1 = types::NetworkIdentifier {
            ssid: types::Ssid::try_from("foo").unwrap(),
            security_type: types::SecurityType::Wpa3,
        };

        // insert saved networks
        assert!(
            exec.run_singlethreaded(
                test_values
                    .real_saved_network_manager
                    .store(test_id_1.clone(), TEST_PASSWORD.clone()),
            )
            .unwrap()
            .is_none()
        );

        // Prep the scan results
        let mutual_security_protocols_1 = [SecurityDescriptor::WPA3_PERSONAL];
        let bss_desc_1 = random_fidl_bss_description!();
        let scanned_bss = types::Bss {
            // This network is WPA3, but should still match against the desired WPA2 network
            compatibility: wlan_common::scan::Compatible::expect_ok(mutual_security_protocols_1),
            bss_description: bss_desc_1.clone().into(),
            // Observation should be unknown, since we didn't try a passive scan.
            observation: types::ScanObservation::Unknown,
            ..generate_random_bss()
        };
        exec.run_singlethreaded(test_values.scan_requester.add_scan_result(Ok(vec![
            types::ScanResult {
                ssid: test_id_1.ssid.clone(),
                // This network is WPA3, but should still match against the desired WPA2 network
                security_type_detailed: types::SecurityTypeDetailed::Wpa3Personal,
                compatibility: types::Compatibility::Supported,
                entries: vec![scanned_bss.clone()],
            },
            generate_random_scan_result(),
            generate_random_scan_result(),
        ])));

        // Run network selection
        let connection_selection_fut = connection_selector.find_and_select_connection_candidate(
            Some(test_id_1.clone()),
            generate_random_connect_reason(),
        );
        let mut connection_selection_fut = pin!(connection_selection_fut);
        let results =
            exec.run_singlethreaded(&mut connection_selection_fut).expect("no selected candidate");
        assert_eq!(&results.network, &test_id_1);
        assert_eq!(&results.security_type_detailed, &types::SecurityTypeDetailed::Wpa3Personal);
        assert_eq!(&results.credential, &TEST_PASSWORD.clone());
        assert_eq!(&results.bss, &scanned_bss);

        // Check that the right scan request was sent
        assert_eq!(
            *exec.run_singlethreaded(test_values.scan_requester.scan_requests.lock()),
            vec![(ScanReason::BssSelection, vec![test_id_1.ssid.clone()], vec![])]
        );
        // Verify that NetworkSelectionDecision telemetry event is sent
        assert_matches!(telemetry_receiver.try_next(), Ok(Some(event)) => {
            assert_matches!(event, TelemetryEvent::NetworkSelectionDecision {
                network_selection_type: telemetry::NetworkSelectionType::Directed,
                num_candidates: Ok(1),
                selected_count: 1,
            });
        });
        assert_matches!(
            telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::BssSelectionResult { .. }))
        );
    }

    #[fuchsia::test]
    fn find_and_select_connection_candidate_with_network_end_to_end_with_failure() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = exec.run_singlethreaded(test_setup(true));
        let connection_selector = test_values.connection_selector;
        let mut telemetry_receiver = test_values.telemetry_receiver;

        // create identifiers
        let test_id_1 = types::NetworkIdentifier {
            ssid: types::Ssid::try_from("foo").unwrap(),
            security_type: types::SecurityType::Wpa3,
        };

        // Return an error on the scan
        exec.run_singlethreaded(
            test_values.scan_requester.add_scan_result(Err(types::ScanError::GeneralError)),
        );

        // Kick off network selection
        let connection_selection_fut = connection_selector.find_and_select_connection_candidate(
            Some(test_id_1.clone()),
            generate_random_connect_reason(),
        );
        let mut connection_selection_fut = pin!(connection_selection_fut);

        // Check that nothing is returned
        let results = exec.run_singlethreaded(&mut connection_selection_fut);
        assert_eq!(results, None);

        // Check that the right scan request was sent
        assert_eq!(
            *exec.run_singlethreaded(test_values.scan_requester.scan_requests.lock()),
            vec![(ScanReason::BssSelection, vec![test_id_1.ssid.clone()], vec![])]
        );

        // Verify that NetworkSelectionDecision telemetry event is sent
        assert_matches!(telemetry_receiver.try_next(), Ok(Some(event)) => {
            assert_matches!(event, TelemetryEvent::NetworkSelectionDecision {
                network_selection_type: telemetry::NetworkSelectionType::Directed,
                num_candidates: Err(()),
                selected_count: 1,
            });
        });

        assert_matches!(
            telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::BssSelectionResult { .. }))
        );
    }

    #[test_case(fidl_common::ScanType::Active)]
    #[test_case(fidl_common::ScanType::Passive)]
    #[fuchsia::test(add_test_attr = false)]
    fn find_and_select_roam_candidate_requests_typed_scan(scan_type: fidl_common::ScanType) {
        let mut exec = fasync::TestExecutor::new();
        let test_values = exec.run_singlethreaded(test_setup(true));
        let connection_selector = test_values.connection_selector;

        let test_id = types::NetworkIdentifier {
            ssid: types::Ssid::try_from("foo").unwrap(),
            security_type: types::SecurityType::Wpa3,
        };
        let credential = generate_random_password();

        // Set a scan result to be returned
        exec.run_singlethreaded(test_values.scan_requester.add_scan_result(Ok(vec![
            types::ScanResult {
                ssid: test_id.ssid.clone(),
                security_type_detailed: types::SecurityTypeDetailed::Wpa3Personal,
                compatibility: types::Compatibility::Supported,
                entries: vec![],
            },
        ])));

        // Kick off roam selection
        let roam_selection_fut = connection_selector.find_and_select_roam_candidate(
            scan_type,
            test_id.clone(),
            &credential,
            types::SecurityTypeDetailed::Wpa3Personal,
        );
        let mut roam_selection_fut = pin!(roam_selection_fut);
        let _ = exec.run_singlethreaded(&mut roam_selection_fut);

        // Check that the right scan request was sent
        if scan_type == fidl_common::ScanType::Active {
            assert_eq!(
                *exec.run_singlethreaded(test_values.scan_requester.scan_requests.lock()),
                vec![(ScanReason::RoamSearch, vec![test_id.ssid.clone()], vec![])]
            );
        } else {
            assert_eq!(
                *exec.run_singlethreaded(test_values.scan_requester.scan_requests.lock()),
                vec![(ScanReason::RoamSearch, vec![], vec![])]
            );
        }
    }

    #[fuchsia::test]
    fn find_and_select_roam_candidate_active_scans_if_no_results_found() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = exec.run_singlethreaded(test_setup(true));
        let connection_selector = test_values.connection_selector;

        let test_id = generate_random_network_identifier();
        let credential = generate_random_password();

        // Set empty scan results to be returned.
        exec.run_singlethreaded(test_values.scan_requester.add_scan_result(Ok(vec![])));
        exec.run_singlethreaded(test_values.scan_requester.add_scan_result(Ok(vec![])));

        // Kick off roam selection with a passive scan.
        let roam_selection_fut = connection_selector.find_and_select_roam_candidate(
            fidl_common::ScanType::Passive,
            test_id.clone(),
            &credential,
            types::SecurityTypeDetailed::Open,
        );

        let mut roam_selection_fut = pin!(roam_selection_fut);
        let _ = exec.run_singlethreaded(&mut roam_selection_fut);

        // Check that two scan requests were issued, one passive scan and one active scan when the
        // passive scan yielded no results.
        assert_eq!(
            *exec.run_singlethreaded(test_values.scan_requester.scan_requests.lock()),
            vec![
                (ScanReason::RoamSearch, vec![], vec![]),
                (ScanReason::RoamSearch, vec![test_id.ssid.clone()], vec![])
            ]
        );
    }

    #[allow(clippy::vec_init_then_push, reason = "mass allow for https://fxbug.dev/381896734")]
    #[fuchsia::test]
    async fn recorded_metrics_on_scan() {
        let (telemetry_sender, mut telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let telemetry_sender = TelemetrySender::new(telemetry_sender);

        // create some identifiers
        let test_id_1 = types::NetworkIdentifier {
            ssid: types::Ssid::try_from("foo").unwrap().clone(),
            security_type: types::SecurityType::Wpa3,
        };

        let test_id_2 = types::NetworkIdentifier {
            ssid: types::Ssid::try_from("bar").unwrap().clone(),
            security_type: types::SecurityType::Wpa,
        };

        let mut mock_scan_results = vec![];

        mock_scan_results.push(types::ScannedCandidate {
            network: test_id_1.clone(),
            bss: types::Bss {
                observation: types::ScanObservation::Passive,
                ..generate_random_bss()
            },
            ..generate_random_scanned_candidate()
        });
        mock_scan_results.push(types::ScannedCandidate {
            network: test_id_1.clone(),
            bss: types::Bss {
                observation: types::ScanObservation::Passive,
                ..generate_random_bss()
            },
            ..generate_random_scanned_candidate()
        });
        mock_scan_results.push(types::ScannedCandidate {
            network: test_id_1.clone(),
            bss: types::Bss {
                observation: types::ScanObservation::Active,
                ..generate_random_bss()
            },
            ..generate_random_scanned_candidate()
        });
        mock_scan_results.push(types::ScannedCandidate {
            network: test_id_2.clone(),
            bss: types::Bss {
                observation: types::ScanObservation::Passive,
                ..generate_random_bss()
            },
            ..generate_random_scanned_candidate()
        });

        record_metrics_on_scan(mock_scan_results, &telemetry_sender);

        assert_matches!(
            telemetry_receiver.try_next(), Ok(Some(TelemetryEvent::ConnectionSelectionScanResults {
                saved_network_count, mut bss_count_per_saved_network, saved_network_count_found_by_active_scan
            })) => {
                assert_eq!(saved_network_count, 2);
                bss_count_per_saved_network.sort();
                assert_eq!(bss_count_per_saved_network, vec![1, 3]);
                assert_eq!(saved_network_count_found_by_active_scan, 1);
            }
        );
    }

    #[fuchsia::test]
    async fn recorded_metrics_on_scan_no_saved_networks() {
        let (telemetry_sender, mut telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let telemetry_sender = TelemetrySender::new(telemetry_sender);

        let mock_scan_results = vec![];

        record_metrics_on_scan(mock_scan_results, &telemetry_sender);

        assert_matches!(
            telemetry_receiver.try_next(), Ok(Some(TelemetryEvent::ConnectionSelectionScanResults {
                saved_network_count, bss_count_per_saved_network, saved_network_count_found_by_active_scan
            })) => {
                assert_eq!(saved_network_count, 0);
                assert_eq!(bss_count_per_saved_network, Vec::<usize>::new());
                assert_eq!(saved_network_count_found_by_active_scan, 0);
            }
        );
    }

    // Fake selector to test that ConnectionSelectionRequester and the service loop plumb requests
    // to the correct methods.
    struct FakeConnectionSelector {
        pub response_to_find_and_select_connection_candidate: Option<types::ScannedCandidate>,
        pub response_to_find_and_select_roam_candidate: Option<types::ScannedCandidate>,
    }
    #[async_trait(?Send)]
    impl ConnectionSelectorApi for FakeConnectionSelector {
        async fn find_and_select_connection_candidate(
            &self,
            _network: Option<types::NetworkIdentifier>,
            _reason: types::ConnectReason,
        ) -> Option<types::ScannedCandidate> {
            self.response_to_find_and_select_connection_candidate.clone()
        }
        async fn find_and_select_roam_candidate(
            &self,
            _scan_type: fidl_common::ScanType,
            _network: types::NetworkIdentifier,
            _credential: &network_config::Credential,
            _scanned_securioty_type: types::SecurityTypeDetailed,
        ) -> Option<types::ScannedCandidate> {
            self.response_to_find_and_select_roam_candidate.clone()
        }
    }
    #[fuchsia::test]
    fn test_service_loop_passes_connection_selection_request() {
        let mut exec = fasync::TestExecutor::new();

        // Create a fake selector, since we're just testing the service loop.
        let candidate = generate_random_scanned_candidate();
        let connection_selector = Rc::new(FakeConnectionSelector {
            response_to_find_and_select_connection_candidate: Some(candidate.clone()),
            response_to_find_and_select_roam_candidate: None,
        });

        // Start the service loop
        let (request_sender, request_receiver) = mpsc::channel(5);
        let mut serve_fut =
            pin!(serve_connection_selection_request_loop(connection_selector, request_receiver));
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Create a requester struct
        let mut requester = ConnectionSelectionRequester { sender: request_sender };

        // Call request method
        let mut connection_selection_fut = pin!(
            requester.do_connection_selection(None, types::ConnectReason::IdleInterfaceAutoconnect)
        );
        assert_matches!(exec.run_until_stalled(&mut connection_selection_fut), Poll::Pending);

        // Run the service loop forward
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Check that receiver gets expected result, confirming the request was plumbed correctly.
        assert_matches!(exec.run_until_stalled(&mut connection_selection_fut), Poll::Ready(Ok(Some(selected_candidate))) => {
            assert_eq!(selected_candidate, candidate);
        });
    }

    #[fuchsia::test]
    fn test_service_loop_passes_roam_selection_request() {
        let mut exec = fasync::TestExecutor::new();

        // Create a fake selector, since we're just testing the service loop.
        let candidate = generate_random_scanned_candidate();
        let connection_selector = Rc::new(FakeConnectionSelector {
            response_to_find_and_select_connection_candidate: None,
            response_to_find_and_select_roam_candidate: Some(candidate.clone()),
        });

        // Start the service loop
        let (request_sender, request_receiver) = mpsc::channel(5);
        let mut serve_fut =
            pin!(serve_connection_selection_request_loop(connection_selector, request_receiver));
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Create a requester struct
        let mut requester = ConnectionSelectionRequester { sender: request_sender };

        // Call request method
        let bss_desc =
            BssDescription::try_from(Sequestered::release(candidate.bss.bss_description.clone()))
                .unwrap();
        let mut roam_selection_fut = pin!(requester.do_roam_selection(
            fidl_common::ScanType::Passive,
            candidate.network.clone(),
            candidate.credential.clone(),
            bss_desc.protection().into()
        ));
        assert_matches!(exec.run_until_stalled(&mut roam_selection_fut), Poll::Pending);

        // Run the service loop forward
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Check that receiver gets expected result, confirming the request was plumbed correctly.
        assert_matches!(exec.run_until_stalled(&mut roam_selection_fut), Poll::Ready(Ok(Some(selected_candidate))) => {
            assert_eq!(selected_candidate, candidate);
        });
    }
}
