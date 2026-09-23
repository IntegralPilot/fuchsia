// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::client::config_management::Credential;
use crate::client::connection_selection::ConnectionSelectionRequester;
use crate::client::roaming::lib::*;
use crate::client::roaming::roam_monitor;
use crate::client::types;
use crate::config_management::SavedNetworksManagerApi;
use crate::telemetry::TelemetrySender;
use anyhow::Error;
use futures::channel::mpsc;
use futures::future::LocalBoxFuture;
use futures::lock::Mutex;
use futures::stream::FuturesUnordered;
use futures::{StreamExt, select};
use log::{debug, error};
use std::convert::Infallible;
use std::sync::Arc;

// Create a roam monitor implementation based on the roaming profile.
fn create_roam_monitor(
    roaming_policy: RoamingPolicy,
    ap_state: types::ApState,
    network_identifier: types::NetworkIdentifier,
    credential: Credential,
    telemetry_sender: TelemetrySender,
    saved_networks: Arc<dyn SavedNetworksManagerApi>,
    past_roams: Arc<Mutex<PastRoamList>>,
) -> Box<dyn roam_monitor::RoamMonitorApi> {
    match roaming_policy {
        RoamingPolicy::Enabled { profile: RoamingProfile::Stationary, .. } => {
            Box::new(roam_monitor::stationary_monitor::StationaryMonitor::new(
                ap_state,
                network_identifier,
                credential,
                telemetry_sender,
                saved_networks,
                past_roams,
            ))
        }
        RoamingPolicy::Disabled => {
            Box::new(roam_monitor::default_monitor::DefaultRoamMonitor::new())
        }
    }
}

// Requests that can be made to roam manager service loop.
#[cfg_attr(test, derive(Debug))]
pub enum RoamServiceRequest {
    InitializeRoamMonitor {
        ap_state: types::ApState,
        network_identifier: types::NetworkIdentifier,
        credential: Credential,
        roam_request_sender: mpsc::Sender<PolicyRoamRequest>,
        roam_trigger_data_receiver: mpsc::Receiver<RoamTriggerData>,
    },
}

// Allows callers to make roam service requests.
#[derive(Clone)]
pub struct RoamManager {
    sender: mpsc::Sender<RoamServiceRequest>,
}

impl RoamManager {
    pub fn new(sender: mpsc::Sender<RoamServiceRequest>) -> Self {
        Self { sender }
    }
    // Create a roam monitor and return handles for passing trigger data and roam requests .
    pub fn initialize_roam_monitor(
        &mut self,
        ap_state: types::ApState,
        network_identifier: types::NetworkIdentifier,
        credential: Credential,
        roam_request_sender: mpsc::Sender<PolicyRoamRequest>,
    ) -> roam_monitor::RoamDataSender {
        let (roam_trigger_data_sender, roam_trigger_data_receiver) =
            mpsc::channel(ROAMING_CHANNEL_BUFFER_SIZE);
        let _ = self
            .sender
            .try_send(RoamServiceRequest::InitializeRoamMonitor {
                ap_state,
                network_identifier,
                credential,
                roam_request_sender,
                roam_trigger_data_receiver,
            })
            .inspect_err(|e| {
                error!("Failed to request roam monitoring: {}. Proceeding without roaming.", e)
            });
        roam_monitor::RoamDataSender::new(roam_trigger_data_sender)
    }
}

/// Service loop for handling roam manager requests.
pub async fn serve_local_roam_manager_requests(
    roaming_policy: RoamingPolicy,
    mut roam_service_request_receiver: mpsc::Receiver<RoamServiceRequest>,
    connection_selection_requester: ConnectionSelectionRequester,
    telemetry_sender: TelemetrySender,
    saved_networks: Arc<dyn SavedNetworksManagerApi>,
) -> Result<Infallible, Error> {
    // Queue of created monitor futures.
    let mut monitor_futs: FuturesUnordered<LocalBoxFuture<'static, Result<(), anyhow::Error>>> =
        FuturesUnordered::new();
    let past_roams = Arc::new(Mutex::new(PastRoamList::new()));

    loop {
        select! {
            // Handle requests sent to roam manager service loop.
            req = roam_service_request_receiver.select_next_some() => {
                match req {
                    // Create and start a roam monitor future, passing in handles from caller. This
                    // ensures that new data is initialized for every new caller (e.g. connected_state).
                    RoamServiceRequest::InitializeRoamMonitor { ap_state, network_identifier, credential, roam_request_sender, roam_trigger_data_receiver }=> {
                        let monitor = create_roam_monitor(roaming_policy, ap_state, network_identifier, credential, telemetry_sender.clone(), saved_networks.clone(), past_roams.clone());
                        let monitor_fut = roam_monitor::serve_roam_monitor(monitor, roaming_policy, roam_trigger_data_receiver, connection_selection_requester.clone(), roam_request_sender, telemetry_sender.clone(), past_roams.clone());
                        monitor_futs.push(Box::pin(monitor_fut));
                    }
                }
            }
            // Serves roam monitors. Futures return when when caller drops handle.
            _terminated_monitor = monitor_futs.select_next_some() => {
                debug!("Roam monitor future closed.");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::TelemetryEvent;
    use crate::util::testing::{
        FakeSavedNetworksManager, generate_random_ap_state, generate_random_network_identifier,
        generate_random_password, generate_random_roaming_connection_data,
    };
    use assert_matches::assert_matches;
    use fidl_fuchsia_wlan_internal::SignalReportIndication;
    use fuchsia_async::TestExecutor;
    use futures::task::Poll;
    use std::pin::pin;
    use test_case::test_case;

    struct RoamManagerServiceTestValues {
        telemetry_sender: TelemetrySender,
        connection_selection_requester: ConnectionSelectionRequester,
        roam_manager: RoamManager,
        roam_service_request_receiver: mpsc::Receiver<RoamServiceRequest>,
        saved_networks: Arc<FakeSavedNetworksManager>,
    }

    fn setup_test() -> RoamManagerServiceTestValues {
        let (telemetry_sender, _) = mpsc::channel::<TelemetryEvent>(100);
        let telemetry_sender = TelemetrySender::new(telemetry_sender);
        let (connection_selection_request_sender, _) = mpsc::channel(5);
        let connection_selection_requester =
            ConnectionSelectionRequester::new(connection_selection_request_sender);
        let (roam_service_request_sender, roam_service_request_receiver) = mpsc::channel(100);
        let roam_manager = RoamManager::new(roam_service_request_sender);
        let saved_networks = Arc::new(FakeSavedNetworksManager::new());
        RoamManagerServiceTestValues {
            telemetry_sender,
            connection_selection_requester,
            roam_manager,
            roam_service_request_receiver,
            saved_networks,
        }
    }

    #[test_case(RoamingPolicy::Disabled; "disabled")]
    #[test_case(RoamingPolicy::Enabled { profile: RoamingProfile::Stationary, mode: RoamingMode::CanRoam }; "enabled_stationary_can_roam")]
    #[fuchsia::test(add_test_attr = false)]
    fn test_create_roam_monitor(roaming_policy: RoamingPolicy) {
        let _exec = TestExecutor::new();
        let (telemetry_sender, _) = mpsc::channel::<TelemetryEvent>(100);
        let telemetry_sender = TelemetrySender::new(telemetry_sender);
        let saved_networks = Arc::new(FakeSavedNetworksManager::new());
        let past_roams = Arc::new(Mutex::new(PastRoamList::new()));
        let monitor = create_roam_monitor(
            roaming_policy,
            generate_random_ap_state(),
            generate_random_network_identifier(),
            generate_random_password(),
            telemetry_sender.clone(),
            saved_networks.clone(),
            past_roams.clone(),
        );
        match roaming_policy {
            RoamingPolicy::Disabled => {
                let default_monitor: Box<dyn roam_monitor::RoamMonitorApi> =
                    Box::new(roam_monitor::default_monitor::DefaultRoamMonitor::new());
                assert_eq!(default_monitor.type_id(), monitor.type_id());
            }
            RoamingPolicy::Enabled { profile: RoamingProfile::Stationary, .. } => {
                let stationary_monitor: Box<dyn roam_monitor::RoamMonitorApi> =
                    Box::new(roam_monitor::stationary_monitor::StationaryMonitor::new(
                        generate_random_ap_state(),
                        generate_random_network_identifier(),
                        generate_random_password(),
                        telemetry_sender.clone(),
                        saved_networks,
                        past_roams.clone(),
                    ));
                assert_eq!(stationary_monitor.type_id(), monitor.type_id());
            }
        }
    }

    #[test_case(RoamingPolicy::Disabled; "disabled")]
    #[test_case(RoamingPolicy::Enabled { profile: RoamingProfile::Stationary, mode: RoamingMode::CanRoam }; "enabled_stationary_can_roam")]
    #[fuchsia::test(add_test_attr = false)]
    fn test_roam_manager_handles_get_roam_monitor_request(roaming_policy: RoamingPolicy) {
        let mut exec = TestExecutor::new();
        let mut test_values = setup_test();
        let mut serve_fut = pin!(serve_local_roam_manager_requests(
            roaming_policy,
            test_values.roam_service_request_receiver,
            test_values.connection_selection_requester.clone(),
            test_values.telemetry_sender.clone(),
            test_values.saved_networks,
        ));
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Send a request to initialize a roam monitor
        let connection_data = generate_random_roaming_connection_data();
        let (sender, _) = mpsc::channel(ROAMING_CHANNEL_BUFFER_SIZE);
        let mut roam_monitor_sender = test_values.roam_manager.initialize_roam_monitor(
            connection_data.ap_state.clone(),
            generate_random_network_identifier(),
            generate_random_password(),
            sender,
        );

        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Send some data via one of the roam monitor. Ensure that it doesn't error out, which means
        // a created roam monitor is holding the receiver end.
        roam_monitor_sender
            .send_signal_report_ind(SignalReportIndication {
                rssi_dbm: -60,
                snr_db: 30,
                tx_rate_500kbps: 0,
            })
            .expect("error sending data via roam monitor sender");
    }

    #[test_case(RoamingPolicy::Disabled; "disabled")]
    #[test_case(RoamingPolicy::Enabled { profile: RoamingProfile::Stationary, mode: RoamingMode::CanRoam }; "enabled_stationary_can_roam")]
    #[fuchsia::test]
    fn test_roam_manager_handles_terminated_roam_monitors(roaming_policy: RoamingPolicy) {
        let mut exec = TestExecutor::new();
        let mut test_values = setup_test();
        let mut serve_fut = pin!(serve_local_roam_manager_requests(
            roaming_policy,
            test_values.roam_service_request_receiver,
            test_values.connection_selection_requester.clone(),
            test_values.telemetry_sender.clone(),
            test_values.saved_networks,
        ));
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Send a request to initialize a roam monitor
        let connection_data = generate_random_roaming_connection_data();
        let (sender, _) = mpsc::channel(ROAMING_CHANNEL_BUFFER_SIZE);
        let mut roam_monitor_sender = test_values.roam_manager.initialize_roam_monitor(
            connection_data.ap_state.clone(),
            generate_random_network_identifier(),
            generate_random_password(),
            sender,
        );

        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Send some trigger data over a channel. Ensure that it doesn't error out, which means
        // a created roam monitor is holding the receiver end.
        roam_monitor_sender
            .send_signal_report_ind(SignalReportIndication {
                rssi_dbm: -60,
                snr_db: 30,
                tx_rate_500kbps: 0,
            })
            .expect("error sending data via roam monitor sender");

        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Drop the trigger data sender. This should cause the monitor to terminate?
        std::mem::drop(roam_monitor_sender);

        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);
    }
}
