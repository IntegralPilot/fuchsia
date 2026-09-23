// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::client::config_management::Credential;
use crate::client::connection_selection::ConnectionSelectionRequester;
use crate::client::roaming::lib::*;
use crate::client::types;
use crate::telemetry::{TelemetryEvent, TelemetrySender};
use anyhow::{Error, format_err};
use async_trait::async_trait;
use fidl_fuchsia_wlan_common as fidl_common;
use fidl_fuchsia_wlan_internal as fidl_internal;
use futures::channel::mpsc;
use futures::future::LocalBoxFuture;
use futures::lock::Mutex;
use futures::stream::{FuturesUnordered, StreamExt};
use futures::{FutureExt, select};
use log::{debug, error, info, warn};
use std::any::Any;
use std::sync::Arc;

pub mod default_monitor;
pub mod stationary_monitor;

// Struct to expose methods for state machine to send roam data, regardless of roam profile.
pub struct RoamDataSender {
    sender: mpsc::Sender<RoamTriggerData>,
}
impl RoamDataSender {
    pub fn new(trigger_data_sender: mpsc::Sender<RoamTriggerData>) -> Self {
        Self { sender: trigger_data_sender }
    }
    pub fn send_signal_report_ind(
        &mut self,
        ind: fidl_internal::SignalReportIndication,
    ) -> Result<(), anyhow::Error> {
        Ok(self.sender.try_send(RoamTriggerData::SignalReportInd(ind))?)
    }
}
/// Trait for creating different roam monitors based on roaming profiles.
#[async_trait(?Send)]
pub trait RoamMonitorApi: Any {
    // Handles trigger data and evaluates current state. Returns an outcome to be taken (e.g. if
    // roam search is warranted). All roam monitors MUST handle all trigger data types, even if
    // they always take no action.
    async fn handle_roam_trigger_data(
        &mut self,
        data: RoamTriggerData,
    ) -> Result<RoamTriggerDataOutcome, anyhow::Error>;
    // Determines if the selected roam candidate is still relevant and provides enough potential
    // improvement to warrant a roam. Returns true if roam request should be sent to state machine.
    fn should_send_roam_request(&self, request: PolicyRoamRequest) -> Result<bool, anyhow::Error>;
    // Method to inform roam monitor of a roam being attempted, so it may  reset any internal state
    // as necessary.
    fn notify_of_roam_attempt(&mut self);
}

// Service loop that orchestrates interaction between state machine (incoming roam data and outgoing
// roam requests), roam monitor implementation, and roam manager (roam search requests and results).
pub async fn serve_roam_monitor(
    mut roam_monitor: Box<dyn RoamMonitorApi>,
    roaming_policy: RoamingPolicy,
    mut trigger_data_receiver: mpsc::Receiver<RoamTriggerData>,
    connection_selection_requester: ConnectionSelectionRequester,
    mut roam_request_sender: mpsc::Sender<PolicyRoamRequest>,
    telemetry_sender: TelemetrySender,
    past_roams: Arc<Mutex<PastRoamList>>,
) -> Result<(), anyhow::Error> {
    // Queue of initialized roam searches.
    let mut roam_search_result_futs: FuturesUnordered<
        LocalBoxFuture<'static, Result<PolicyRoamRequest, Error>>,
    > = FuturesUnordered::new();

    loop {
        select! {
            // Handle incoming trigger data.
            trigger_data = trigger_data_receiver.next() => if let Some(data) = trigger_data {
                match roam_monitor.handle_roam_trigger_data(data).await {
                    Ok(RoamTriggerDataOutcome::RoamSearch { scan_type, network_identifier, credential, current_security, reasons}) => {
                        telemetry_sender.send(TelemetryEvent::PolicyRoamScan { reasons: reasons.clone() });
                        info!("Performing scan to find proactive local roaming candidates.");
                        let roam_search_fut = get_roaming_connection_selection_future(
                            connection_selection_requester.clone(),
                            scan_type,
                            network_identifier,
                            credential,
                            current_security,
                            reasons
                        );
                        roam_search_result_futs.push(roam_search_fut.boxed());
                    },
                    Ok(RoamTriggerDataOutcome::Noop) => {},
                    Err(e) => error!("error handling roam trigger data: {}", e),
                }
            },
            // Handle the result of a completed roam search, sending recommentation to roam if
            // necessary.
            roam_search_result = roam_search_result_futs.select_next_some() => match roam_search_result {
                Ok(request) => {
                    if roam_monitor.should_send_roam_request(request.clone()).unwrap_or_else(|e| {
                            error!("Error validating selected roam candidate: {}", e);
                            false
                        }) {
                            match roaming_policy {
                                RoamingPolicy::Enabled { mode: RoamingMode::CanRoam, ..} => {
                                    info!("Requesting roam to candidate: {:?}", request.candidate.to_string_without_pii());
                                    if roam_request_sender.try_send(request).is_err() {
                                        warn!("Failed to send roam request, exiting monitor service loop.");
                                        break
                                    }
                                }
                                _ => {
                                    debug!("Roaming policy is {:?}. Skipping roam request.", roaming_policy);
                                    telemetry_sender.send(TelemetryEvent::WouldRoamConnect)
                                }
                            }
                            // Record that a roam attempt is made.
                            if let Some(mut past_roams) = past_roams.try_lock() {
                                past_roams.add(RoamEvent::new_roam_now());
                            } else {
                                error!("Unexpectedly failed to acquire lock on past roam list; will not record roam");
                            }
                    }
                }
                Err(e) => {
                    error!("Error occured during roam search: {:?}", e);
                }
            },
            complete => {
                debug!("Roam monitor channels dropped, exiting monitor service loop.");
                break
            }
        }
    }
    Ok(())
}

// Request a roam selection from the connection selection module, bundle the receiver into a future
// to be queued and that can also return the initiating request.
async fn get_roaming_connection_selection_future(
    mut connection_selection_requester: ConnectionSelectionRequester,
    scan_type: fidl_common::ScanType,
    network_identifier: types::NetworkIdentifier,
    credential: Credential,
    current_security: types::SecurityTypeDetailed,
    reasons: Vec<RoamReason>,
) -> Result<PolicyRoamRequest, Error> {
    match connection_selection_requester
        .do_roam_selection(scan_type, network_identifier, credential, current_security)
        .await?
    {
        Some(candidate) => Ok(PolicyRoamRequest { candidate, reasons }),
        None => Err(format_err!("No roam candidates found.")),
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::client::connection_selection::ConnectionSelectionRequest;
    use crate::client::roaming::lib::RoamingProfile;
    use crate::telemetry::TelemetryEvent;
    use crate::util::testing::fakes::FakeRoamMonitor;
    use crate::util::testing::{
        generate_random_network_identifier, generate_random_password,
        generate_random_scanned_candidate,
    };
    use assert_matches::assert_matches;
    use fidl_fuchsia_wlan_common as fidl_common;
    use fidl_fuchsia_wlan_internal as fidl_internal;
    use fuchsia_async::{self as fasync, TestExecutor};
    use futures::task::Poll;
    use futures::{Future, pin_mut};
    use std::pin::Pin;
    use test_case::test_case;

    struct TestValues {
        trigger_data_sender: mpsc::Sender<RoamTriggerData>,
        trigger_data_receiver: mpsc::Receiver<RoamTriggerData>,
        roam_request_sender: mpsc::Sender<PolicyRoamRequest>,
        roam_request_receiver: mpsc::Receiver<PolicyRoamRequest>,
        connection_selection_requester: ConnectionSelectionRequester,
        connection_selection_request_receiver: mpsc::Receiver<ConnectionSelectionRequest>,
        telemetry_sender: TelemetrySender,
        telemetry_receiver: mpsc::Receiver<TelemetryEvent>,
        past_roams: Arc<Mutex<PastRoamList>>,
    }

    fn setup_test() -> TestValues {
        let (trigger_data_sender, trigger_data_receiver) = mpsc::channel(100);
        let (roam_sender, roam_receiver) = mpsc::channel(100);
        let (connection_selection_request_sender, connection_selection_request_receiver) =
            mpsc::channel(5);
        let connection_selection_requester =
            ConnectionSelectionRequester::new(connection_selection_request_sender);
        let (telemetry_sender, telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let telemetry_sender = TelemetrySender::new(telemetry_sender);
        let past_roams = Arc::new(Mutex::new(PastRoamList::new()));
        TestValues {
            trigger_data_sender,
            trigger_data_receiver,
            roam_request_sender: roam_sender,
            roam_request_receiver: roam_receiver,
            connection_selection_requester,
            connection_selection_request_receiver,
            telemetry_sender,
            telemetry_receiver,
            past_roams,
        }
    }

    #[fuchsia::test]
    fn test_roam_data_sender_send_signal_report_ind() {
        let _exec = TestExecutor::new();
        let (sender, mut receiver) = mpsc::channel(100);
        let mut roam_data_sender = RoamDataSender::new(sender);
        let ind =
            fidl_internal::SignalReportIndication { rssi_dbm: -60, snr_db: 30, tx_rate_500kbps: 0 };

        roam_data_sender.send_signal_report_ind(ind).expect("error sending signal report");

        // Verify that roam sender packages trigger data and sends to roam monitor receiver.
        assert_matches!(receiver.try_next(), Ok(Some(RoamTriggerData::SignalReportInd(data))) => {
            assert_eq!(ind, data);
        });
    }

    #[test_case(RoamTriggerDataOutcome::Noop; "should not queue roam search")]
    #[test_case(RoamTriggerDataOutcome::RoamSearch { scan_type: fidl_common::ScanType::Passive, network_identifier: generate_random_network_identifier(), credential: generate_random_password(), current_security: types::SecurityTypeDetailed::Open, reasons: vec![]}; "should queue roam search")]
    #[fuchsia::test(add_test_attr = false)]
    fn test_serve_loop_handles_trigger_data(response_to_should_roam_scan: RoamTriggerDataOutcome) {
        let mut exec = TestExecutor::new();
        let mut test_values = setup_test();

        // Create a fake roam monitor. Set the should_roam_scan response, so we can verify that the
        // serve loop forwarded the data and that the correct action was taken.
        let mut roam_monitor = FakeRoamMonitor::new();
        roam_monitor.response_to_should_roam_scan = response_to_should_roam_scan.clone();

        // Start a serve loop with the fake roam monitor
        let serve_fut = serve_roam_monitor(
            Box::new(roam_monitor),
            RoamingPolicy::Enabled {
                profile: RoamingProfile::Stationary,
                mode: RoamingMode::CanRoam,
            },
            test_values.trigger_data_receiver,
            test_values.connection_selection_requester,
            test_values.roam_request_sender,
            test_values.telemetry_sender,
            test_values.past_roams,
        );
        pin_mut!(serve_fut);

        // Run loop forward
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Send some trigger data to kick off the handling sequence. The actual values here are
        // irrelevant.
        test_values
            .trigger_data_sender
            .try_send(RoamTriggerData::SignalReportInd(fidl_internal::SignalReportIndication {
                rssi_dbm: -40,
                snr_db: 40,
                tx_rate_500kbps: 0,
            }))
            .expect("failed to send");

        // Run loop forward.
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        match response_to_should_roam_scan {
            RoamTriggerDataOutcome::RoamSearch { .. } => {
                // Verify metric was sent for upcoming roam scan
                assert_matches!(
                    test_values.telemetry_receiver.try_next(),
                    Ok(Some(TelemetryEvent::PolicyRoamScan { .. }))
                );
                // Verify that a roam search request was sent after monitor responded true.
                assert_matches!(
                    test_values.connection_selection_request_receiver.try_next(),
                    Ok(Some(_))
                );
            }
            RoamTriggerDataOutcome::Noop => {
                // Verify that no roam search was triggered after monitor responded false.
                assert_matches!(
                    test_values.connection_selection_request_receiver.try_next(),
                    Err(_)
                );
            }
        }
    }

    #[test_case(false, RoamingMode::CanRoam; "should not send roam request can roam")]
    #[test_case(false, RoamingMode::MetricsOnly; "should not send roam request metrics only")]
    #[test_case(true, RoamingMode::CanRoam; "should send roam request can roam")]
    #[test_case(true, RoamingMode::MetricsOnly; "should send roam request metrics only")]
    #[fuchsia::test(add_test_attr = false)]
    fn test_serve_loop_handles_roam_search_results(
        response_to_should_send_roam_request: bool,
        roaming_mode: RoamingMode,
    ) {
        let mut exec = TestExecutor::new();
        let mut test_values = setup_test();

        // Create a fake roam monitor. Set should_roam_scan to true to ensure roam searches get
        // queued. Conditionally set the should_send_roam_request response.
        let mut roam_monitor = FakeRoamMonitor::new();
        roam_monitor.response_to_should_roam_scan = RoamTriggerDataOutcome::RoamSearch {
            scan_type: fidl_common::ScanType::Passive,
            network_identifier: generate_random_network_identifier(),
            credential: generate_random_password(),
            current_security: types::SecurityTypeDetailed::Open,
            reasons: vec![],
        };
        roam_monitor.response_to_should_send_roam_request = response_to_should_send_roam_request;

        // Start a serve loop with the fake roam monitor
        let serve_fut = serve_roam_monitor(
            Box::new(roam_monitor),
            RoamingPolicy::Enabled { profile: RoamingProfile::Stationary, mode: roaming_mode },
            test_values.trigger_data_receiver,
            test_values.connection_selection_requester,
            test_values.roam_request_sender,
            test_values.telemetry_sender,
            test_values.past_roams,
        );
        pin_mut!(serve_fut);

        // Run loop forward
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Send some trigger data to kick off the handling sequence. The actual values here are
        // irrelevant.
        test_values
            .trigger_data_sender
            .try_send(RoamTriggerData::SignalReportInd(fidl_internal::SignalReportIndication {
                rssi_dbm: -40,
                snr_db: 40,
                tx_rate_500kbps: 0,
            }))
            .expect("failed to send");

        // Run loop forward
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Respond via the connection selection requester
        let candidate = generate_random_scanned_candidate();
        assert_matches!(test_values.connection_selection_request_receiver.try_next(), Ok(Some(ConnectionSelectionRequest::RoamSelection { responder, .. })) => {
            // Respond with a roam candidate
            responder.send(Some(candidate.clone())).expect("failed to send");
        });

        // Run loop forward
        assert_matches!(exec.run_until_stalled(&mut serve_fut), Poll::Pending);

        // Verify metric was sent for upcoming roam scan
        assert_matches!(
            test_values.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::PolicyRoamScan { .. }))
        );

        if response_to_should_send_roam_request && roaming_mode == RoamingMode::CanRoam {
            // Verify that a roam request is sent if the should_send_roam_request method returns
            // true.
            assert_matches!(test_values.roam_request_receiver.try_next(), Ok(Some(selection)) => {
                assert_eq!(selection.candidate, candidate);
            });
        } else {
            // Verify that no roam request is sent if the should_send_roam_request method returns
            // false, regardless of the roaming mode.
            assert_matches!(test_values.roam_request_receiver.try_next(), Err(_));
        }
    }

    #[fuchsia::test]
    fn test_roam_attempts_are_recorded_in_past_roams() {
        let mut exec = TestExecutor::new();
        let mut test_values = setup_test();

        // Create a fake roam monitor. Set should_roam_scan to true to ensure roam searches get
        // queued. Conditionally set the should_send_roam_request response.
        let mut roam_monitor = FakeRoamMonitor::new();
        roam_monitor.response_to_should_roam_scan = RoamTriggerDataOutcome::RoamSearch {
            scan_type: fidl_common::ScanType::Passive,
            network_identifier: generate_random_network_identifier(),
            credential: generate_random_password(),
            current_security: types::SecurityTypeDetailed::Open,
            reasons: vec![],
        };
        roam_monitor.response_to_should_send_roam_request = true;

        // Start a serve loop with the fake roam monitor
        let serve_fut = serve_roam_monitor(
            Box::new(roam_monitor),
            RoamingPolicy::Enabled {
                profile: RoamingProfile::Stationary,
                mode: RoamingMode::CanRoam,
            },
            test_values.trigger_data_receiver,
            test_values.connection_selection_requester,
            test_values.roam_request_sender,
            test_values.telemetry_sender,
            test_values.past_roams.clone(),
        );
        pin_mut!(serve_fut);

        trigger_scan_and_roam(
            &mut exec,
            &mut serve_fut,
            &mut test_values.trigger_data_sender,
            &mut test_values.connection_selection_request_receiver,
            &mut test_values.roam_request_receiver,
        );

        // A roam request should have been sent, verify that it is recorded in the past roams list.
        let past_roams = test_values
            .past_roams
            .clone()
            .try_lock()
            .unwrap()
            .get_recent(fasync::MonotonicInstant::INFINITE_PAST);
        assert_eq!(past_roams.len(), 1);

        trigger_scan_and_roam(
            &mut exec,
            &mut serve_fut,
            &mut test_values.trigger_data_sender,
            &mut test_values.connection_selection_request_receiver,
            &mut test_values.roam_request_receiver,
        );

        // A roam request should have been sent, verify that it is recorded in the past roams list.
        let past_roams = test_values
            .past_roams
            .clone()
            .try_lock()
            .unwrap()
            .get_recent(fasync::MonotonicInstant::INFINITE_PAST);
        assert_eq!(past_roams.len(), 2);
    }

    // This sends a signal report to the roam monitor, responds to roam searches with a random
    // candidate network, progresses the serve loop forward, and verifies that the roam monitor
    // sends out a roam request.
    fn trigger_scan_and_roam(
        exec: &mut TestExecutor,
        serve_fut: &mut Pin<&mut impl Future<Output = std::result::Result<(), anyhow::Error>>>,
        trigger_data_sender: &mut mpsc::Sender<RoamTriggerData>,
        connection_selection_request_receiver: &mut mpsc::Receiver<ConnectionSelectionRequest>,
        roam_request_receiver: &mut mpsc::Receiver<PolicyRoamRequest>,
    ) {
        // Run the serve loop forward
        assert_matches!(exec.run_until_stalled(serve_fut), Poll::Pending);

        // Send some trigger data to kick off the handling sequence. The actual values here are
        // irrelevant.
        trigger_data_sender
            .try_send(RoamTriggerData::SignalReportInd(fidl_internal::SignalReportIndication {
                rssi_dbm: -40,
                snr_db: 40,
                tx_rate_500kbps: 0,
            }))
            .expect("failed to send");

        // Run loop forward
        assert_matches!(exec.run_until_stalled(serve_fut), Poll::Pending);

        // Respond via the connection selection requester
        let candidate = generate_random_scanned_candidate();
        assert_matches!(connection_selection_request_receiver.try_next(), Ok(Some(ConnectionSelectionRequest::RoamSelection { responder, .. })) => {
            // Respond with a roam candidate
            responder.send(Some(candidate.clone())).expect("failed to send");
        });

        assert_matches!(exec.run_until_stalled(serve_fut), Poll::Pending);
        assert_matches!(roam_request_receiver.try_next(), Ok(Some(selection)) => {
            assert_eq!(selection.candidate, candidate);
        });
    }
}
