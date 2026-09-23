// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_wlan_common as fidl_common;
use fidl_fuchsia_wlan_sme as fidl_sme;
use fuchsia_async as fasync;
use fuchsia_sync::Mutex;
use futures::StreamExt;
use log::{error, info, warn};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use wlan_common::channel::primary_channel_from_freq;

use crate::ifaces::{self, ClientIface};
use anyhow::{Error, format_err};
use futures::channel::mpsc;
use wlan_common::bss::BssDescription;
use wlan_telemetry::{TelemetryEvent, TelemetrySender};

type IfaceId = u32;

/// Events to be reported to the wlanix main loop, for netlink serialization.
pub(crate) enum ScheduledScanEvent {
    ResultsAvailable { iface_id: IfaceId },
    Stopped { iface_id: IfaceId },
}

use std::collections::HashMap;

pub(crate) enum ScheduledScanState {
    Starting,
    #[expect(dead_code)]
    FirmwareScansActive {
        transaction_proxy: fidl_sme::ScheduledScanTransactionProxy,
        task: fasync::Task<()>,
    },
    SoftwareScansActive {
        request: fidl_common::ScheduledScanRequest,
        iface: Arc<dyn ClientIface>,
        task: fasync::Task<()>,
    },
    SoftwareScansPaused {
        request: fidl_common::ScheduledScanRequest,
        iface: Arc<dyn ClientIface>,
    },
}

/// Controls the lifecycle and state of scheduled scans across interfaces.
///
/// Coordinates inputs from power state (charging vs. discharging),
/// connection state, and incoming requests to update scheduled scanning
/// state.
pub(crate) struct ScheduledScanController {
    pub(crate) states: Mutex<HashMap<IfaceId, ScheduledScanState>>,
    event_sender: mpsc::UnboundedSender<ScheduledScanEvent>,
    telemetry_sender: TelemetrySender,
    is_charging: AtomicBool,
}

impl ScheduledScanController {
    pub(crate) fn new(
        telemetry_sender: TelemetrySender,
        event_sender: mpsc::UnboundedSender<ScheduledScanEvent>,
    ) -> Self {
        Self {
            states: Mutex::new(HashMap::new()),
            event_sender,
            telemetry_sender,
            is_charging: AtomicBool::new(false),
        }
    }

    /// Helper methods for atomic booleans
    pub(crate) fn is_charging(&self) -> bool {
        self.is_charging.load(Ordering::SeqCst)
    }

    pub(crate) fn set_charging_state(self: &Arc<Self>, is_charging: bool) {
        let prev_charging_state = self.is_charging.swap(is_charging, Ordering::SeqCst);
        if prev_charging_state != is_charging {
            self.process_charging_state_change();
        }
    }

    /// Updates state when a new start request is received.
    pub(crate) async fn on_start_sched_scan(
        self: &Arc<Self>,
        request: fidl_common::ScheduledScanRequest,
        iface_id: u32,
        iface: Arc<dyn ClientIface>,
    ) -> Result<(), Error> {
        // Clean up existing scheduled scan for this interface, if applicable.
        {
            let mut states = self.states.lock();
            if let Some(_old_state) = states.remove(&iface_id) {
                warn!(
                    "Received scheduled scan request for iface {} while one is already enabled. Stopping previous.",
                    iface_id
                );
                self.telemetry_sender.send(TelemetryEvent::PnoScanDisabled {
                    reason: wlan_telemetry::PnoScanDisabledReason::Internal,
                });
                let _ = self.event_sender.unbounded_send(ScheduledScanEvent::Stopped { iface_id });
            }
            states.insert(iface_id, ScheduledScanState::Starting);
        }

        // Attempt to start firmware scheduled scan.
        self.telemetry_sender.send(TelemetryEvent::PnoScanEnabled);
        match iface.start_sched_scan(request.clone()).await {
            Ok(tx_proxy) => {
                info!("Firmware scheduled scan started successfully for iface {}", iface_id);
                self.telemetry_sender.send(TelemetryEvent::ScanStart);

                let mut states = self.states.lock();
                // Check if we were cancelled by a StopSchedScan request while awaiting
                if let Some(ScheduledScanState::Starting) = states.get(&iface_id) {
                    // Start task to manage transactions
                    let txn_proxy = tx_proxy.into_proxy();
                    let event_stream = txn_proxy.take_event_stream();
                    let controller_weak = Arc::downgrade(self);
                    let telemetry_sender = self.telemetry_sender.clone();
                    let event_sender = self.event_sender.clone();
                    let iface_clone = Arc::clone(&iface);
                    let task = fasync::Task::spawn(async move {
                        let res = handle_scheduled_scan_transactions(
                            telemetry_sender,
                            event_sender,
                            event_stream,
                            iface_id,
                            iface_clone,
                        )
                        .await;
                        if let Some(controller) = controller_weak.upgrade() {
                            controller.on_firmware_scan_ended(iface_id, res).await;
                        }
                    });

                    // Update state to active
                    states.insert(
                        iface_id,
                        ScheduledScanState::FirmwareScansActive {
                            transaction_proxy: txn_proxy,
                            task,
                        },
                    );
                } else {
                    // Scheduled scanning was cancelled while processing the start request.
                    info!(
                        "Firmware scheduled scan start for iface {} was cancelled while starting. Aborting.",
                        iface_id
                    );
                }
            }
            Err(e) => {
                info!(
                    "Failed to start firmware scheduled scan for iface {}: {:?}, falling back to software.",
                    iface_id, e
                );
                let mut states = self.states.lock();
                if let Some(ScheduledScanState::Starting) = states.get(&iface_id) {
                    if self.is_charging() {
                        let iface_clone = Arc::clone(&iface);
                        let scan_primary_channels = get_scan_primary_channels(&request);
                        let match_sets = request.match_sets.clone();
                        // Start task to run software scan loop
                        let controller_weak = Arc::downgrade(self);
                        let telemetry_sender = self.telemetry_sender.clone();
                        let event_sender = self.event_sender.clone();
                        let task = fasync::Task::spawn(async move {
                            software_scheduled_scan_loop(
                                telemetry_sender,
                                event_sender,
                                iface_id,
                                iface_clone,
                                scan_primary_channels,
                                match_sets,
                            )
                            .await;
                            if let Some(controller) = controller_weak.upgrade() {
                                controller.on_software_scan_ended(iface_id).await;
                            }
                        });
                        // Update state to active
                        states.insert(
                            iface_id,
                            ScheduledScanState::SoftwareScansActive { request, iface, task },
                        );
                    } else {
                        info!(
                            "Software scheduled scan pending for iface {} (not charging).",
                            iface_id
                        );
                        // Update state to paused
                        states.insert(
                            iface_id,
                            ScheduledScanState::SoftwareScansPaused { request, iface },
                        );
                    }
                } else {
                    info!(
                        "Software scheduled scan fallback for iface {} was cancelled while starting. Aborting.",
                        iface_id
                    );
                }
            }
        }
        Ok(())
    }

    /// Updates state when scheduled scanning is stopped via the API request
    pub(crate) async fn on_stop_sched_scan(&self, iface_id: u32) {
        self.telemetry_sender.send(TelemetryEvent::PnoScanDisabled {
            reason: wlan_telemetry::PnoScanDisabledReason::ApiRequest,
        });
        let mut states = self.states.lock();
        if states.remove(&iface_id).is_some() {
            let _ = self.event_sender.unbounded_send(ScheduledScanEvent::Stopped { iface_id });
        }
    }

    /// Updates state when firmware scheduled scanning is stopped by the driver or firmware
    async fn on_firmware_scan_ended(&self, iface_id: u32, res: Result<(), Error>) {
        let mut states = self.states.lock();
        if let Some(ScheduledScanState::FirmwareScansActive { .. }) = states.get(&iface_id) {
            match res {
                Ok(()) => {
                    info!("Firmware scheduled scan stopped cleanly for iface {}.", iface_id);
                    self.telemetry_sender.send(TelemetryEvent::PnoScanDisabled {
                        reason: wlan_telemetry::PnoScanDisabledReason::Firmware,
                    });
                }
                Err(e) => {
                    error!(
                        "Firmware scheduled scan failed for iface {} with error: {}.",
                        iface_id, e
                    );
                    self.telemetry_sender.send(TelemetryEvent::PnoScanFailure);
                }
            }
            let _ = self.event_sender.unbounded_send(ScheduledScanEvent::Stopped { iface_id });
            states.remove(&iface_id);
        }
    }

    /// Updates state when software scheduled scanning terminates.
    async fn on_software_scan_ended(&self, iface_id: u32) {
        let mut states = self.states.lock();
        if let Some(ScheduledScanState::SoftwareScansActive { .. }) = states.remove(&iface_id) {
            let _ = self.event_sender.unbounded_send(ScheduledScanEvent::Stopped { iface_id });
        }
    }

    /// Idempotently update state based on the current state and charging status.
    fn process_charging_state_change(self: &Arc<Self>) {
        let mut states = self.states.lock();
        let is_charging = self.is_charging();

        let prev_charging_states = std::mem::take(&mut *states);
        for (iface_id, current_state) in prev_charging_states {
            let new_state = match current_state {
                current @ ScheduledScanState::FirmwareScansActive { .. } => current,
                current @ ScheduledScanState::Starting => current,
                ScheduledScanState::SoftwareScansActive { request, iface, task } => {
                    if !is_charging {
                        info!(
                            "Pausing software scheduled scan for iface {} because device is not charging.",
                            iface_id
                        );
                        ScheduledScanState::SoftwareScansPaused { request, iface }
                    } else {
                        ScheduledScanState::SoftwareScansActive { request, iface, task }
                    }
                }
                ScheduledScanState::SoftwareScansPaused { request, iface } => {
                    if is_charging {
                        info!(
                            "Resuming software scheduled scan for iface {} because device is charging.",
                            iface_id
                        );
                        let iface_clone = Arc::clone(&iface);
                        let scan_primary_channels = get_scan_primary_channels(&request);
                        let match_sets = request.match_sets.clone();
                        let controller_weak = Arc::downgrade(self);
                        let telemetry_sender = self.telemetry_sender.clone();
                        let event_sender = self.event_sender.clone();
                        let new_task = fasync::Task::spawn(async move {
                            software_scheduled_scan_loop(
                                telemetry_sender,
                                event_sender,
                                iface_id,
                                iface_clone,
                                scan_primary_channels,
                                match_sets,
                            )
                            .await;
                            if let Some(controller) = controller_weak.upgrade() {
                                controller.on_software_scan_ended(iface_id).await;
                            }
                        });
                        ScheduledScanState::SoftwareScansActive { request, iface, task: new_task }
                    } else {
                        ScheduledScanState::SoftwareScansPaused { request, iface }
                    }
                }
            };
            states.insert(iface_id, new_state);
        }
    }
}

/// Runs a periodic active scan loop to emulate scheduled scanning in software.
///
/// Used as a fallback when the driver or firmware does not support hardware-offloaded scheduled
/// scans. To save battery, it only runs when the device is charging.
///
/// TODO(498247761): Remove once firmware scheduled scans are supported.
async fn software_scheduled_scan_loop(
    telemetry_sender: TelemetrySender,
    event_sender: mpsc::UnboundedSender<ScheduledScanEvent>,
    iface_id: u32,
    iface: Arc<dyn ClientIface>,
    scan_primary_channels: Vec<u8>,
    match_sets: Option<Vec<fidl_common::ScheduledScanMatchSet>>,
) {
    info!("Starting software scheduled scan loop for iface {}", iface_id);

    loop {
        // Trigger scan
        info!("Triggering software scheduled scan for iface {}", iface_id);
        match iface.trigger_scan(None, scan_primary_channels.clone()).await {
            Ok(ifaces::ScanEnd::Complete) => {
                let results = iface.get_last_scan_results();
                telemetry_sender.send(TelemetryEvent::ScanResult {
                    result: wlan_telemetry::ScanResult::Complete { num_results: results.len() },
                });
                let matching_results = get_matching_scan_results(&match_sets, results);
                if !matching_results.is_empty() {
                    info!(
                        "Matching scheduled scan results found for iface {}, notifying caller",
                        iface_id
                    );
                    telemetry_sender.send(TelemetryEvent::PnoScanResultsReceived);
                    let _ = event_sender
                        .unbounded_send(ScheduledScanEvent::ResultsAvailable { iface_id });
                }
            }
            Ok(ifaces::ScanEnd::Cancelled) => {
                info!("Software scheduled scan cancelled for iface {}", iface_id);
                telemetry_sender.send(TelemetryEvent::ScanResult {
                    result: wlan_telemetry::ScanResult::Cancelled,
                });
            }
            Err(e) => {
                warn!("Software scheduled scan failed for iface {}: {:?}", iface_id, e);
                telemetry_sender.send(TelemetryEvent::PnoScanFailure);
                telemetry_sender.send(TelemetryEvent::ScanResult {
                    result: wlan_telemetry::ScanResult::Failed,
                });
                return;
            }
        }

        fasync::Timer::new(fasync::MonotonicDuration::from_minutes(5)).await;
    }
}

/// Handles matching scan results, and stoppages initiated by dropping the transaction stream.
async fn handle_scheduled_scan_transactions(
    telemetry_sender: TelemetrySender,
    event_sender: mpsc::UnboundedSender<ScheduledScanEvent>,
    mut stream: fidl_sme::ScheduledScanTransactionEventStream,
    iface_id: u32,
    iface: Arc<dyn ClientIface>,
) -> Result<(), Error> {
    while let Some(event) = stream.next().await {
        match event {
            Ok(fidl_sme::ScheduledScanTransactionEvent::OnScheduledScanMatchesAvailable {
                scan_results,
            }) => {
                info!("Received scheduled scan results from SME for iface {}", iface_id);
                let results = match wlan_common::scan::read_vmo(scan_results) {
                    Ok(res) => res,
                    Err(e) => {
                        warn!("Failed to read scan VMO for iface {}: {}", iface_id, e);
                        continue;
                    }
                };

                // Update the scan results cache. The results are retrieved using the standard
                // nl80211 GetScan message, so we can't distinguish which set of results the caller
                // is requesting.
                iface.update_last_scan_results(results);

                telemetry_sender.send(TelemetryEvent::PnoScanResultsReceived);
                let _ =
                    event_sender.unbounded_send(ScheduledScanEvent::ResultsAvailable { iface_id });
            }
            Err(fidl::Error::ClientChannelClosed { .. }) => {
                info!("Scheduled scan transaction channel closed by SME for iface {}", iface_id);
                return Ok(());
            }
            Err(e) => {
                return Err(format_err!(
                    "Error on scheduled scan transaction event stream for iface {}: {}",
                    iface_id,
                    e
                ));
            }
        }
    }
    info!("Scheduled scan transaction channel closed by SME for iface {}", iface_id);
    Ok(())
}

fn get_scan_primary_channels(req: &fidl_common::ScheduledScanRequest) -> Vec<u8> {
    let mut scan_primary_channels = vec![];
    let mut invalid_frequencies = vec![];
    if let Some(freqs) = &req.frequencies {
        for &f in freqs {
            match primary_channel_from_freq(f) {
                Some(chan) => scan_primary_channels.push(chan),
                None => invalid_frequencies.push(f),
            }
        }
    }
    if !invalid_frequencies.is_empty() {
        warn!("Failed to convert some frequencies to channels: {:?}", invalid_frequencies);
    }
    scan_primary_channels
}

// TODO(498247761): Remove once firmware scheduled scans are supported.
fn get_matching_scan_results(
    match_sets: &Option<Vec<fidl_common::ScheduledScanMatchSet>>,
    results: Vec<fidl_sme::ScanResult>,
) -> Vec<fidl_sme::ScanResult> {
    if let Some(match_sets) = match_sets {
        // If no target networks are specified, return scan results, which will be sent up if non-empty.
        if match_sets.is_empty() {
            results
        } else {
            results
                .into_iter()
                .filter(|r| {
                    BssDescription::try_from(r.bss_description.clone())
                        .map(|bss| {
                            let bss_band = bss.channel.band;
                            match_sets.iter().any(|match_set| {
                                let is_ssid_match = match_set
                                    .ssid
                                    .as_ref()
                                    .is_some_and(|match_ssid| *match_ssid == bss.ssid);

                                let adjusted_rssi = match_set
                                    .band_rssi_adjustments
                                    .as_ref()
                                    .and_then(|adjustments| {
                                        adjustments.iter().find(|a| a.band == bss_band)
                                    })
                                    .map_or(r.bss_description.rssi_dbm, |adj| {
                                        r.bss_description
                                            .rssi_dbm
                                            .saturating_add(adj.rssi_adjustment)
                                    });

                                let is_rssi_match = match_set
                                    .min_rssi_threshold
                                    .is_none_or(|threshold| adjusted_rssi > threshold);
                                is_ssid_match && is_rssi_match
                            })
                        })
                        .unwrap_or(false)
                })
                .collect()
        }
    } else {
        // If match_sets weren't provided, return scan results, which will be sent up if non-empty.
        results
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use assert_matches::assert_matches;
    use futures::SinkExt;
    use futures::channel::mpsc;
    use futures::task::Poll;
    use std::pin::pin;

    use crate::ifaces::test_utils::{ClientIfaceCall, TestClientIface};

    impl std::fmt::Debug for ScheduledScanState {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                ScheduledScanState::Starting => write!(f, "Starting"),
                ScheduledScanState::FirmwareScansActive { .. } => write!(f, "FirmwareScansActive"),
                ScheduledScanState::SoftwareScansActive { .. } => write!(f, "SoftwareScansActive"),
                ScheduledScanState::SoftwareScansPaused { .. } => write!(f, "SoftwareScansPaused"),
            }
        }
    }

    #[fuchsia::test]
    fn test_get_matching_scan_results() {
        use ieee80211::Ssid;
        use wlan_common::bss::BssDescription;
        use wlan_common::fake_fidl_bss_description;
        use wlan_common::test_utils::fake_stas::FakeProtectionCfg;

        let scan_results = vec![
            fidl_sme::ScanResult {
                bss_description: fake_fidl_bss_description!(protection => FakeProtectionCfg::Open,
                    ssid: Ssid::try_from(b"SSID1".to_vec()).unwrap(),
                    bssid: [1, 2, 3, 4, 5, 6],
                    rssi_dbm: -30,
                ),
                compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                    mutual_security_protocols: vec![],
                }),
                timestamp_nanos: 0,
            },
            fidl_sme::ScanResult {
                bss_description: fake_fidl_bss_description!(protection => FakeProtectionCfg::Open,
                    ssid: Ssid::try_from(b"SSID2".to_vec()).unwrap(),
                    bssid: [7, 8, 9, 10, 11, 12],
                    rssi_dbm: -30,
                ),
                compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                    mutual_security_protocols: vec![],
                }),
                timestamp_nanos: 0,
            },
        ];

        let match_sets = Some(vec![fidl_common::ScheduledScanMatchSet {
            ssid: Some(b"SSID1".to_vec()),
            ..Default::default()
        }]);

        let matching = get_matching_scan_results(&match_sets, scan_results.clone());
        assert_eq!(matching.len(), 1);
        let bss = BssDescription::try_from(matching[0].bss_description.clone()).unwrap();
        assert_eq!(bss.ssid, Ssid::try_from(b"SSID1".to_vec()).unwrap());

        let match_sets_none = None;
        let matching_none = get_matching_scan_results(&match_sets_none, scan_results.clone());
        assert_eq!(matching_none.len(), 2);

        let match_sets_no_match = Some(vec![fidl_common::ScheduledScanMatchSet {
            ssid: Some(b"SSID3".to_vec()),
            ..Default::default()
        }]);
        let matching_no_match = get_matching_scan_results(&match_sets_no_match, scan_results);
        assert_eq!(matching_no_match.len(), 0);
    }

    fn setup_controller_test() -> (
        fasync::TestExecutor,
        Arc<ScheduledScanController>,
        Arc<TestClientIface>,
        TelemetrySender,
        mpsc::Receiver<TelemetryEvent>,
    ) {
        let exec = fasync::TestExecutor::new();
        let (telemetry_sender, telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let telemetry_sender = TelemetrySender::new(telemetry_sender);
        let (event_sender, _event_receiver) = mpsc::unbounded();
        let controller =
            Arc::new(ScheduledScanController::new(telemetry_sender.clone(), event_sender));
        let iface = Arc::new(TestClientIface::new());
        (exec, controller, iface, telemetry_sender, telemetry_receiver)
    }

    #[fuchsia::test]
    fn test_controller_start_firmware_scan() {
        let (mut exec, controller, iface, _telemetry_sender, _telemetry_receiver) =
            setup_controller_test();

        let request = fidl_common::ScheduledScanRequest {
            ssids: Some(vec![b"TestSSID".to_vec()]),
            scan_plans: Some(vec![fidl_common::ScheduledScanPlan { interval: 40, iterations: 0 }]),
            ..Default::default()
        };

        let mut fut = pin!(controller.on_start_sched_scan(request.clone(), 1, iface.clone(),));

        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));

        // Verify firmware scan started
        let calls = iface.calls.lock();
        assert_eq!(calls.len(), 1);
        let recorded_request =
            assert_matches!(&calls[0], ClientIfaceCall::StartSchedScan { _request } => _request);
        assert_eq!(recorded_request.ssids, request.ssids);

        assert_matches!(
            controller.states.lock().get(&1),
            Some(ScheduledScanState::FirmwareScansActive { .. })
        );
    }

    #[fuchsia::test]
    fn test_controller_start_software_scan_charging() {
        let (mut exec, controller, iface, _telemetry_sender, _telemetry_receiver) =
            setup_controller_test();

        // Force firmware scan failure
        *iface.fail_start_sched_scan.lock() = true;
        controller.set_charging_state(true);

        let request = fidl_common::ScheduledScanRequest {
            ssids: Some(vec![b"TestSSID".to_vec()]),
            scan_plans: Some(vec![fidl_common::ScheduledScanPlan { interval: 40, iterations: 0 }]),
            ..Default::default()
        };

        let mut fut = pin!(controller.on_start_sched_scan(request.clone(), 1, iface.clone(),));

        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));

        // Verify software fallback active and task spawned because charging
        assert_matches!(
            controller.states.lock().get(&1),
            Some(ScheduledScanState::SoftwareScansActive { .. })
        );
    }

    #[fuchsia::test]
    fn test_controller_start_software_scan_not_charging() {
        let (mut exec, controller, iface, _telemetry_sender, _telemetry_receiver) =
            setup_controller_test();

        // Force firmware scan failure
        *iface.fail_start_sched_scan.lock() = true;
        controller.set_charging_state(false);

        let request = fidl_common::ScheduledScanRequest {
            ssids: Some(vec![b"TestSSID".to_vec()]),
            scan_plans: Some(vec![fidl_common::ScheduledScanPlan { interval: 40, iterations: 0 }]),
            ..Default::default()
        };

        let mut fut = pin!(controller.on_start_sched_scan(request.clone(), 1, iface.clone(),));

        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));

        // Verify software fallback active but task NOT spawned because not charging
        assert_matches!(
            controller.states.lock().get(&1),
            Some(ScheduledScanState::SoftwareScansPaused { .. })
        );
    }

    #[fuchsia::test]
    fn test_controller_charging_state_change() {
        let (_exec, controller, iface, _telemetry_sender, _telemetry_receiver) =
            setup_controller_test();

        // Setup software fallback active but not charging
        controller.set_charging_state(false);
        {
            let mut states = controller.states.lock();
            states.insert(
                1,
                ScheduledScanState::SoftwareScansPaused {
                    request: fidl_common::ScheduledScanRequest::default(),
                    iface: Arc::clone(&iface) as Arc<dyn ClientIface>,
                },
            );
        }

        // Transition to charging -> should spawn task
        controller.set_charging_state(true);
        assert_matches!(
            controller.states.lock().get(&1),
            Some(ScheduledScanState::SoftwareScansActive { .. })
        );

        // Transition to discharging -> should pause task
        controller.set_charging_state(false);
        assert_matches!(
            controller.states.lock().get(&1),
            Some(ScheduledScanState::SoftwareScansPaused { .. })
        );
    }

    #[fuchsia::test]
    fn test_controller_stop_scan() {
        let (mut exec, controller, _iface, _telemetry_sender, _telemetry_receiver) =
            setup_controller_test();

        // Setup active firmware scan
        {
            let mut states = controller.states.lock();
            let (proxy, _server) =
                fidl::endpoints::create_proxy::<fidl_sme::ScheduledScanTransactionMarker>();
            states.insert(
                1,
                ScheduledScanState::FirmwareScansActive {
                    transaction_proxy: proxy,
                    task: fasync::Task::spawn(async {}),
                },
            );
        }

        let mut fut = pin!(controller.on_stop_sched_scan(1));
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));

        // Verify stopped and cleaned up
        assert!(controller.states.lock().is_empty());
    }

    #[fuchsia::test]
    fn test_controller_multiple_interfaces_independent_scans() {
        let (mut exec, controller, iface_1, _telemetry_sender, _telemetry_receiver) =
            setup_controller_test();
        let iface_2 = Arc::new(TestClientIface::new());

        // Force software scheduled scan fallback (PNO not supported)
        *iface_1.fail_start_sched_scan.lock() = true;
        *iface_2.fail_start_sched_scan.lock() = true;

        // Start not charging
        controller.set_charging_state(false);

        let request = fidl_common::ScheduledScanRequest {
            ssids: Some(vec![b"TestSSID".to_vec()]),
            scan_plans: Some(vec![fidl_common::ScheduledScanPlan { interval: 40, iterations: 0 }]),
            ..Default::default()
        };

        // 1. Start scan on interface 1
        let mut fut_1 = pin!(controller.on_start_sched_scan(request.clone(), 1, iface_1.clone()));
        assert_matches!(exec.run_until_stalled(&mut fut_1), Poll::Ready(Ok(())));

        // Verify only interface 1 is active (paused because not charging)
        {
            let states = controller.states.lock();
            assert_matches!(states.get(&1), Some(ScheduledScanState::SoftwareScansPaused { .. }));
            assert_matches!(states.get(&2), None);
        }

        // 2. Start scan on interface 2
        let mut fut_2 = pin!(controller.on_start_sched_scan(request.clone(), 2, iface_2.clone()));
        assert_matches!(exec.run_until_stalled(&mut fut_2), Poll::Ready(Ok(())));

        // Verify both interfaces are now active (paused because not charging)
        {
            let states = controller.states.lock();
            assert_matches!(states.get(&1), Some(ScheduledScanState::SoftwareScansPaused { .. }));
            assert_matches!(states.get(&2), Some(ScheduledScanState::SoftwareScansPaused { .. }));
        }

        // 3. Transition to charging -> both should spawn tasks and become Active independently
        controller.set_charging_state(true);
        {
            let states = controller.states.lock();
            assert_matches!(states.get(&1), Some(ScheduledScanState::SoftwareScansActive { .. }));
            assert_matches!(states.get(&2), Some(ScheduledScanState::SoftwareScansActive { .. }));
        }

        // 4. Stop scan on interface 1
        let mut stop_fut_1 = pin!(controller.on_stop_sched_scan(1));
        assert_matches!(exec.run_until_stalled(&mut stop_fut_1), Poll::Ready(()));

        // Verify interface 1 is cleaned up, but interface 2 remains untouched!
        {
            let states = controller.states.lock();
            assert_matches!(states.get(&1), None);
            assert_matches!(states.get(&2), Some(ScheduledScanState::SoftwareScansActive { .. }));
        }

        // 5. Stop scan on interface 2
        let mut stop_fut_2 = pin!(controller.on_stop_sched_scan(2));
        assert_matches!(exec.run_until_stalled(&mut stop_fut_2), Poll::Ready(()));

        // Verify both are now stopped and map is empty
        assert!(controller.states.lock().is_empty());
    }

    #[fuchsia::test]
    fn test_controller_start_scan_cancelled_while_starting() {
        let (mut exec, controller, iface, _telemetry_sender, _telemetry_receiver) =
            setup_controller_test();

        // Setup yielding start_sched_scan
        let (mut yield_sender, yielding_rx) = mpsc::channel(1);
        *iface.start_sched_scan_yielding_rx.lock() = Some(yielding_rx);

        let request = fidl_common::ScheduledScanRequest {
            ssids: Some(vec![b"TestSSID".to_vec()]),
            scan_plans: Some(vec![fidl_common::ScheduledScanPlan { interval: 40, iterations: 0 }]),
            ..Default::default()
        };

        // 1. Initiate StartScan
        let mut start_fut = pin!(controller.on_start_sched_scan(request, 1, iface.clone()));

        // Poll it -> should yield inside start_sched_scan, leaving placeholder Starting state
        assert_matches!(exec.run_until_stalled(&mut start_fut), Poll::Pending);
        assert_matches!(controller.states.lock().get(&1), Some(ScheduledScanState::Starting));

        // 2. Intercept with StopScan request while StartScan is still pending
        let mut stop_fut = pin!(controller.on_stop_sched_scan(1));
        assert_matches!(exec.run_until_stalled(&mut stop_fut), Poll::Ready(()));

        // StopScan should have cleared the Starting placeholder
        assert_matches!(controller.states.lock().get(&1), None);

        // 3. Resume/unblock the StartScan call
        assert_matches!(exec.run_until_stalled(&mut yield_sender.send(())), Poll::Ready(Ok(())));

        // Poll StartScan again to finish execution
        assert_matches!(exec.run_until_stalled(&mut start_fut), Poll::Ready(Ok(())));

        // Verify that the state remained cleaned up (we did NOT insert FirmwareScansActive)
        assert_matches!(controller.states.lock().get(&1), None);
    }
}
