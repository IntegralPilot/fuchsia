// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::client::config_management::Credential;
use crate::client::roaming::lib::*;
use crate::client::roaming::roam_monitor::RoamMonitorApi;
use crate::client::types;
use crate::config_management::SavedNetworksManagerApi;
use crate::telemetry::{TelemetryEvent, TelemetrySender};
use crate::util::pseudo_energy::EwmaSignalData;
use async_trait::async_trait;
use fidl_fuchsia_wlan_common as fidl_common;
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_internal as fidl_internal;
use fuchsia_async as fasync;
use futures::lock::Mutex;
use log::{error, info};
use std::sync::Arc;
use wlan_common::historical_list::Timestamped;

pub const MIN_BACKOFF_BETWEEN_ROAM_SCANS: zx::MonotonicDuration =
    zx::MonotonicDuration::from_minutes(1);
pub const MAX_BACKOFF_BETWEEN_ROAM_SCANS: zx::MonotonicDuration =
    zx::MonotonicDuration::from_minutes(32);

const LOCAL_ROAM_THRESHOLD_RSSI_2G: f64 = -72.0;
const LOCAL_ROAM_THRESHOLD_RSSI_5G: f64 = -75.0;

const MIN_RSSI_IMPROVEMENT_TO_ROAM: f64 = 3.0;
const MIN_RSSI_DROP_TO_RESET_BACKOFF: f64 = 3.0;

/// Number of previous RSSI measurements to exponentially weigh into average.
/// TODO(https://fxbug.dev/42165706): Tune smoothing factor.
pub const STATIONARY_ROAMING_EWMA_SMOOTHING_FACTOR: usize = 10;

// Roams will not be considered if more than this many roams have been attempted in the last day.
pub const NUM_MAX_ROAMS_PER_DAY: usize = 5;

pub struct StationaryMonitor {
    pub connection_data: RoamingConnectionData,
    pub telemetry_sender: TelemetrySender,
    saved_networks: Arc<dyn SavedNetworksManagerApi>,
    scan_backoff: zx::MonotonicDuration,
    /// To be used to limit how often roams can happen to avoid thrashing between APs.
    past_roams: Arc<Mutex<PastRoamList>>,
    next_roaming_enabled_time: fasync::MonotonicInstant,
}

impl StationaryMonitor {
    pub fn new(
        ap_state: types::ApState,
        network_identifier: types::NetworkIdentifier,
        credential: Credential,
        telemetry_sender: TelemetrySender,
        saved_networks: Arc<dyn SavedNetworksManagerApi>,
        past_roams: Arc<Mutex<PastRoamList>>,
    ) -> Self {
        let connection_data = RoamingConnectionData::new(
            ap_state.clone(),
            network_identifier,
            credential,
            EwmaSignalData::new(
                ap_state.tracked.signal.rssi_dbm,
                ap_state.tracked.signal.snr_db,
                STATIONARY_ROAMING_EWMA_SMOOTHING_FACTOR,
            ),
        );
        Self {
            connection_data,
            telemetry_sender,
            saved_networks,
            scan_backoff: MIN_BACKOFF_BETWEEN_ROAM_SCANS,
            past_roams,
            next_roaming_enabled_time: fasync::MonotonicInstant::now(),
        }
    }

    // Handle signal report indiciations. Update internal connection data, if necessary. Returns
    // true if a roam search should be initiated.
    #[allow(clippy::needless_return, reason = "mass allow for https://fxbug.dev/381896734")]
    async fn handle_signal_report(
        &mut self,
        stats: fidl_internal::SignalReportIndication,
    ) -> Result<RoamTriggerDataOutcome, anyhow::Error> {
        self.connection_data.signal_data.update_with_new_measurement(stats.rssi_dbm, stats.snr_db);

        // Update velocity with EWMA signal, to smooth out noise.
        self.connection_data.rssi_velocity.update(self.connection_data.signal_data.ewma_rssi.get());

        self.telemetry_sender.send(TelemetryEvent::OnSignalVelocityUpdate {
            rssi_velocity: self.connection_data.rssi_velocity.get(),
        });

        // If the network likely has 1 BSS, don't scan for another BSS to roam to.
        match self
            .saved_networks
            .is_network_single_bss(
                &self.connection_data.network_identifier,
                &self.connection_data.credential,
            )
            .await
        {
            Ok(true) => return Ok(RoamTriggerDataOutcome::Noop),
            _ => {
                // There could be an error if the config is not found. If there was an error, treat
                // that as the network could be multi BSS and consider a roam scan.
                return Ok(self.should_roam_scan_after_signal_report());
            }
        }
    }

    fn should_roam_scan_after_signal_report(&mut self) -> RoamTriggerDataOutcome {
        // Exit early if roaming has been disabled.
        if fasync::MonotonicInstant::now() <= self.next_roaming_enabled_time {
            return RoamTriggerDataOutcome::Noop;
        }
        // If we have exceeded the maximum number of roam attempts per day, do not attempt roaming
        // and record the next time at which roaming will be enabled.
        if let Some(past_roams) = self.past_roams.try_lock() {
            let recent_roams =
                past_roams.get_recent(fasync::MonotonicInstant::now() - TIMESPAN_TO_LIMIT_SCANS);
            if recent_roams.len() >= NUM_MAX_ROAMS_PER_DAY {
                if let Some(oldest_roam_attempt) = recent_roams.first() {
                    self.next_roaming_enabled_time =
                        oldest_roam_attempt.time() + TIMESPAN_TO_LIMIT_SCANS;
                    info!(
                        "Maximum number of roam attempts per day ({}) has been reached. Roam scanning is disabled until a reboot, or until fewer than {} roam attempts have occured in the past 24 hours.",
                        NUM_MAX_ROAMS_PER_DAY, NUM_MAX_ROAMS_PER_DAY
                    );
                } else {
                    error!("Unexpectedly failed to get the oldest roam attempt.");
                }
                return RoamTriggerDataOutcome::Noop;
            }
        } else {
            error!("Unexpectedly failed to get lock on recent roam attempts data");
        }
        let mut roam_reasons: Vec<RoamReason> = vec![];

        // Check RSSI threshold
        let rssi_threshold = if self.connection_data.ap_state.tracked.channel.band
            == fidl_ieee80211::WlanBand::FiveGhz
        {
            LOCAL_ROAM_THRESHOLD_RSSI_5G
        } else {
            LOCAL_ROAM_THRESHOLD_RSSI_2G
        };
        let rssi = self.connection_data.signal_data.ewma_rssi.get();
        if rssi <= rssi_threshold {
            roam_reasons.push(RoamReason::RssiBelowThreshold);

            // Reset scan backoff as RSSI has dropped notably since the last roam scan.
            if rssi
                <= self.connection_data.previous_roam_scan_data.rssi
                    - MIN_RSSI_DROP_TO_RESET_BACKOFF
            {
                self.scan_backoff = MIN_BACKOFF_BETWEEN_ROAM_SCANS;
            }
        }

        // Do not scan if there are no roam reasons, or if the scan backoff has not yet passed.f
        let now = fasync::MonotonicInstant::now();
        if roam_reasons.is_empty()
            || now < self.connection_data.previous_roam_scan_data.time + self.scan_backoff
        {
            return RoamTriggerDataOutcome::Noop;
        }

        // Exponentially extend backoff between roam scans
        self.scan_backoff =
            std::cmp::min(self.scan_backoff * 2_i64, MAX_BACKOFF_BETWEEN_ROAM_SCANS);

        // Updated fields for tracking roam scan decisions and initiated roam search.
        self.connection_data.previous_roam_scan_data.time = fasync::MonotonicInstant::now();
        self.connection_data.previous_roam_scan_data.rssi = rssi;
        info!("Initiating roam search for roam reasons: {:?}", roam_reasons);

        RoamTriggerDataOutcome::RoamSearch {
            // Stationary monitor uses active roam scans to prioritize shorter scan times over power
            // consumption.
            scan_type: fidl_common::ScanType::Active,
            network_identifier: self.connection_data.network_identifier.clone(),
            credential: self.connection_data.credential.clone(),
            current_security: self.connection_data.ap_state.original().protection().into(),
            reasons: roam_reasons,
        }
    }
}

#[async_trait(?Send)]
impl RoamMonitorApi for StationaryMonitor {
    async fn handle_roam_trigger_data(
        &mut self,
        data: RoamTriggerData,
    ) -> Result<RoamTriggerDataOutcome, anyhow::Error> {
        match data {
            RoamTriggerData::SignalReportInd(stats) => self.handle_signal_report(stats).await,
        }
    }

    fn should_send_roam_request(&self, request: PolicyRoamRequest) -> Result<bool, anyhow::Error> {
        if request.candidate.bss.bssid == self.connection_data.ap_state.original().bssid {
            info!("Selected roam candidate is the currently connected candidate, ignoring");
            return Ok(false);
        }
        // Only send roam scan if the selected candidate shows a significant signal improvement,
        // compared to the most up-to-date roaming connection data
        let latest_rssi = self.connection_data.signal_data.ewma_rssi.get();
        if (request.candidate.bss.signal.rssi_dbm as f64)
            < latest_rssi + MIN_RSSI_IMPROVEMENT_TO_ROAM
        {
            info!(
                "Selected roam candidate ({:?}) is not enough of an improvement. Ignoring.",
                request.candidate.to_string_without_pii()
            );
            return Ok(false);
        }
        Ok(true)
    }

    fn notify_of_roam_attempt(&mut self) {
        // Reset scan backoff when a roam is attempted, because we will now be on a new BSS (or
        // will get disconnected, and this will be irrelevant).
        self.scan_backoff = MIN_BACKOFF_BETWEEN_ROAM_SCANS;
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::util::testing::{
        FakeSavedNetworksManager, generate_random_bss, generate_random_password,
        generate_random_roaming_connection_data, generate_random_scanned_candidate,
    };
    use assert_matches::assert_matches;
    use fidl_fuchsia_wlan_internal as fidl_internal;
    use futures::channel::mpsc;
    use futures::task::Poll;
    use test_case::test_case;

    struct TestValues {
        monitor: StationaryMonitor,
        telemetry_receiver: mpsc::Receiver<TelemetryEvent>,
        saved_networks: Arc<FakeSavedNetworksManager>,
        past_roams: Arc<Mutex<PastRoamList>>,
    }

    const TEST_OK_SNR: f64 = 40.0;

    fn setup_test() -> TestValues {
        let connection_data = generate_random_roaming_connection_data();
        let (telemetry_sender, telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let telemetry_sender = TelemetrySender::new(telemetry_sender);
        let saved_networks = Arc::new(FakeSavedNetworksManager::new());
        let past_roams = Arc::new(Mutex::new(PastRoamList::new()));
        // Set the fake saved networks manager to respond that the network is not single BSS by
        // default since most tests are for cases where roaming should be considered.
        saved_networks.set_is_single_bss_response(false);
        let monitor = StationaryMonitor {
            connection_data,
            telemetry_sender,
            saved_networks: saved_networks.clone(),
            scan_backoff: MIN_BACKOFF_BETWEEN_ROAM_SCANS,
            past_roams: past_roams.clone(),
            next_roaming_enabled_time: fasync::MonotonicInstant::now(),
        };
        TestValues { monitor, telemetry_receiver, saved_networks, past_roams }
    }

    fn setup_test_with_data(connection_data: RoamingConnectionData) -> TestValues {
        let (telemetry_sender, telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let telemetry_sender = TelemetrySender::new(telemetry_sender);
        let saved_networks = Arc::new(FakeSavedNetworksManager::new());
        let past_roams = Arc::new(Mutex::new(PastRoamList::new()));
        let monitor = StationaryMonitor {
            connection_data,
            telemetry_sender,
            saved_networks: saved_networks.clone(),
            scan_backoff: MIN_BACKOFF_BETWEEN_ROAM_SCANS,
            past_roams: past_roams.clone(),
            next_roaming_enabled_time: fasync::MonotonicInstant::now(),
        };
        TestValues { monitor, telemetry_receiver, saved_networks, past_roams }
    }

    /// This runs handle_roam_trigger_data with run_until_stalled and expects it to finish.
    /// run_single_threaded cannot be used with fake time.
    fn run_handle_roam_trigger_data(
        exec: &mut fasync::TestExecutor,
        monitor: &mut StationaryMonitor,
        trigger_data: RoamTriggerData,
    ) -> RoamTriggerDataOutcome {
        return assert_matches!(exec.run_until_stalled(&mut monitor.handle_roam_trigger_data(trigger_data)), Poll::Ready(Ok(should_roam)) => {should_roam});
    }

    #[test_case(-80, true; "bad rssi")]
    #[test_case(-40, false; "good rssi")]
    #[fuchsia::test(add_test_attr = false)]
    fn test_handle_signal_report_trigger_data(rssi: i8, should_roam_search: bool) {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::now());

        // Generate initial connection data based on test case.
        let connection_data = RoamingConnectionData {
            signal_data: EwmaSignalData::new(rssi, TEST_OK_SNR, 10),
            ..generate_random_roaming_connection_data()
        };
        let mut test_values = setup_test_with_data(connection_data);

        // Advance the time so that we allow roam scanning,
        exec.set_fake_time(fasync::MonotonicInstant::after(fasync::MonotonicDuration::from_hours(
            1,
        )));

        // Generate trigger data that won't change the above values, and send to handle_roam_trigger_data
        // method.
        let trigger_data =
            RoamTriggerData::SignalReportInd(fidl_internal::SignalReportIndication {
                rssi_dbm: rssi,
                snr_db: TEST_OK_SNR as i8,
                tx_rate_500kbps: 0,
            });
        let result =
            run_handle_roam_trigger_data(&mut exec, &mut test_values.monitor, trigger_data.clone());

        if should_roam_search {
            assert_matches!(result, RoamTriggerDataOutcome::RoamSearch { .. });
        } else {
            assert_matches!(result, RoamTriggerDataOutcome::Noop);
        }
    }

    #[fuchsia::test]
    fn test_stationary_monitor_uses_active_scans() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::now());

        // Setup monitor with connection data that would trigger a roam scan due to RSSI below
        // threshold. Set the EWMA weights to 1 so the values can be easily changed later in tests.
        let rssi = LOCAL_ROAM_THRESHOLD_RSSI_5G - 1.0;
        let connection_data = RoamingConnectionData {
            signal_data: EwmaSignalData::new(rssi, TEST_OK_SNR, 1),
            ..generate_random_roaming_connection_data()
        };
        let mut test_values = setup_test_with_data(connection_data);

        // Generate trigger data with same signal values as initial, which would trigger a roam
        // search due to the below threshold RSSI.
        let trigger_data =
            RoamTriggerData::SignalReportInd(fidl_internal::SignalReportIndication {
                rssi_dbm: rssi as i8,
                snr_db: TEST_OK_SNR as i8,
                tx_rate_500kbps: 0,
            });

        // Advance the time so that we allow roam scanning
        exec.set_fake_time(fasync::MonotonicInstant::after(fasync::MonotonicDuration::from_hours(
            1,
        )));

        // Send trigger data, and verify that the roam scan type is Active.
        assert_matches!(
            run_handle_roam_trigger_data(&mut exec, &mut test_values.monitor, trigger_data.clone()),
            RoamTriggerDataOutcome::RoamSearch { scan_type: fidl_common::ScanType::Active, .. }
        );
    }

    #[fuchsia::test]
    fn test_minimum_time_between_roam_scans() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::now());

        // Setup monitor with connection data that would trigger a roam scan due to RSSI below
        // threshold. Set the EWMA weights to 1 so the values can be easily changed later in tests.
        let rssi = LOCAL_ROAM_THRESHOLD_RSSI_5G - 1.0;
        let connection_data = RoamingConnectionData {
            signal_data: EwmaSignalData::new(rssi, TEST_OK_SNR, 1),
            ..generate_random_roaming_connection_data()
        };
        let mut test_values = setup_test_with_data(connection_data);
        let trigger_data =
            RoamTriggerData::SignalReportInd(fidl_internal::SignalReportIndication {
                rssi_dbm: rssi as i8,
                snr_db: TEST_OK_SNR as i8,
                tx_rate_500kbps: 0,
            });

        // Advance the time less than the minimum scan backoff time.
        exec.set_fake_time(fasync::MonotonicInstant::after(
            MIN_BACKOFF_BETWEEN_ROAM_SCANS - fasync::MonotonicDuration::from_seconds(1),
        ));

        // Send trigger data, and verify that we aren't told to roam search because the minimum wait
        // time has not passed.
        assert_matches!(
            run_handle_roam_trigger_data(&mut exec, &mut test_values.monitor, trigger_data.clone()),
            RoamTriggerDataOutcome::Noop
        );

        // Now advance past the minimum wait time.
        exec.set_fake_time(fasync::MonotonicInstant::after(
            fasync::MonotonicDuration::from_seconds(2),
        ));

        // Send trigger data, and verify that we are told to roam search.
        assert_matches!(
            run_handle_roam_trigger_data(&mut exec, &mut test_values.monitor, trigger_data.clone()),
            RoamTriggerDataOutcome::RoamSearch { .. }
        );
    }

    #[fuchsia::test]
    fn test_roam_scans_backoff_exponentially() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::now());

        // Setup monitor with connection data that would trigger a roam scan due to RSSI below
        // threshold. Set the EWMA weights to 1 so the values can be easily changed later in tests.
        let rssi = LOCAL_ROAM_THRESHOLD_RSSI_5G - 1.0;
        let connection_data = RoamingConnectionData {
            signal_data: EwmaSignalData::new(rssi, TEST_OK_SNR, 1),
            ..generate_random_roaming_connection_data()
        };
        let trigger_data =
            RoamTriggerData::SignalReportInd(fidl_internal::SignalReportIndication {
                rssi_dbm: rssi as i8,
                snr_db: TEST_OK_SNR as i8,
                tx_rate_500kbps: 0,
            });
        let mut test_values = setup_test_with_data(connection_data);

        // Expected backoffs should start at the minimum value, and double up to the maximum value.
        let mut expected_backoff = MIN_BACKOFF_BETWEEN_ROAM_SCANS;
        while expected_backoff <= MAX_BACKOFF_BETWEEN_ROAM_SCANS {
            // Advance time by less than the expected backoff.
            exec.set_fake_time(fasync::MonotonicInstant::after(
                expected_backoff - fasync::MonotonicDuration::from_seconds(1),
            ));

            // Send trigger data, and verify that we aren't told to roam search because the minimum wait
            // time has not passed.
            assert_matches!(
                run_handle_roam_trigger_data(
                    &mut exec,
                    &mut test_values.monitor,
                    trigger_data.clone()
                ),
                RoamTriggerDataOutcome::Noop
            );

            // Advance time past the expected backoff time.
            exec.set_fake_time(fasync::MonotonicInstant::after(
                fasync::MonotonicDuration::from_seconds(2),
            ));

            // Send trigger data, and verify that we are told to roam search.
            assert_matches!(
                run_handle_roam_trigger_data(
                    &mut exec,
                    &mut test_values.monitor,
                    trigger_data.clone()
                ),
                RoamTriggerDataOutcome::RoamSearch { .. }
            );

            // Backoff exponentially
            expected_backoff = expected_backoff * 2;
        }
        // Ensure the backoff has not extended past the maximum value by advancing past the maximum
        // and verifying we can roam search.
        exec.set_fake_time(fasync::MonotonicInstant::after(
            MAX_BACKOFF_BETWEEN_ROAM_SCANS + fasync::MonotonicDuration::from_seconds(1),
        ));
        assert_matches!(
            run_handle_roam_trigger_data(&mut exec, &mut test_values.monitor, trigger_data.clone()),
            RoamTriggerDataOutcome::RoamSearch { .. }
        );
    }

    #[fuchsia::test]
    fn test_roam_attempt_resets_backoff() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::now());

        // Setup monitor with connection data that would trigger a roam scan due to RSSI below
        // threshold. Set the EWMA weights to 1 so the values can be easily changed later in tests.
        let rssi = LOCAL_ROAM_THRESHOLD_RSSI_5G - 1.0;
        let connection_data = RoamingConnectionData {
            signal_data: EwmaSignalData::new(rssi, TEST_OK_SNR, 1),
            ..generate_random_roaming_connection_data()
        };
        let trigger_data =
            RoamTriggerData::SignalReportInd(fidl_internal::SignalReportIndication {
                rssi_dbm: rssi as i8,
                snr_db: TEST_OK_SNR as i8,
                tx_rate_500kbps: 0,
            });
        let mut test_values = setup_test_with_data(connection_data);

        // Run time forward past the minimum backoff time.
        exec.set_fake_time(
            fasync::MonotonicInstant::after(MIN_BACKOFF_BETWEEN_ROAM_SCANS)
                + zx::MonotonicDuration::from_seconds(1),
        );

        // Send trigger data, and verify that we are told to roam search.
        assert_matches!(
            run_handle_roam_trigger_data(&mut exec, &mut test_values.monitor, trigger_data.clone()),
            RoamTriggerDataOutcome::RoamSearch { .. }
        );

        // Run time forward past the minimum backoff time.
        exec.set_fake_time(
            fasync::MonotonicInstant::after(MIN_BACKOFF_BETWEEN_ROAM_SCANS)
                + zx::MonotonicDuration::from_seconds(1),
        );

        // Send trigger data, and verify that we do not roam search, because the backoff has grown.
        assert_matches!(
            run_handle_roam_trigger_data(&mut exec, &mut test_values.monitor, trigger_data.clone()),
            RoamTriggerDataOutcome::Noop
        );

        // Receive notification of a roam attempt.
        test_values.monitor.notify_of_roam_attempt();

        // Send trigger data, and verify that we may now roam search, because the backoff has been
        // reset.
        assert_matches!(
            run_handle_roam_trigger_data(&mut exec, &mut test_values.monitor, trigger_data.clone()),
            RoamTriggerDataOutcome::RoamSearch { .. }
        );
    }

    #[fuchsia::test]
    fn test_rssi_drop_resets_backoff() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::now());

        // Setup monitor with connection data that would trigger a roam scan due to RSSI below
        // threshold. Set the EWMA weights to 1 so the values can be easily changed later in tests.
        let rssi = LOCAL_ROAM_THRESHOLD_RSSI_5G - 1.0;
        let connection_data = RoamingConnectionData {
            signal_data: EwmaSignalData::new(rssi, TEST_OK_SNR, 1),
            ..generate_random_roaming_connection_data()
        };
        let trigger_data =
            RoamTriggerData::SignalReportInd(fidl_internal::SignalReportIndication {
                rssi_dbm: rssi as i8,
                snr_db: TEST_OK_SNR as i8,
                tx_rate_500kbps: 0,
            });
        let mut test_values = setup_test_with_data(connection_data);

        // Run time forward past the minimum backoff time.
        exec.set_fake_time(
            fasync::MonotonicInstant::after(MIN_BACKOFF_BETWEEN_ROAM_SCANS)
                + zx::MonotonicDuration::from_seconds(1),
        );

        // Send trigger data, and verify that we are told to roam search.
        assert_matches!(
            run_handle_roam_trigger_data(&mut exec, &mut test_values.monitor, trigger_data.clone()),
            RoamTriggerDataOutcome::RoamSearch { .. }
        );

        // Run time forward past the minimum backoff time again.
        exec.set_fake_time(
            fasync::MonotonicInstant::after(MIN_BACKOFF_BETWEEN_ROAM_SCANS)
                + zx::MonotonicDuration::from_seconds(1),
        );

        // Send trigger data, and verify that we do not roam scan, because the backoff has extended.
        assert_matches!(
            run_handle_roam_trigger_data(&mut exec, &mut test_values.monitor, trigger_data.clone()),
            RoamTriggerDataOutcome::Noop
        );

        // Now send trigger data showing the RSSI has dropped significantly, and verify that we
        // do now roam search because the backoff was reset to the minimum time.
        let trigger_data =
            RoamTriggerData::SignalReportInd(fidl_internal::SignalReportIndication {
                rssi_dbm: (rssi - MIN_RSSI_DROP_TO_RESET_BACKOFF) as i8,
                snr_db: TEST_OK_SNR as i8,
                tx_rate_500kbps: 0,
            });
        assert_matches!(
            run_handle_roam_trigger_data(&mut exec, &mut test_values.monitor, trigger_data.clone()),
            RoamTriggerDataOutcome::RoamSearch { .. }
        );

        // Run time forward time, but not past the absolute minimum backoff time .
        exec.set_fake_time(
            fasync::MonotonicInstant::after(MIN_BACKOFF_BETWEEN_ROAM_SCANS)
                - zx::MonotonicDuration::from_seconds(1),
        );
        // Now send trigger data showing an _additional_ drop in RSSI, but verify that we do not
        // not scan as the minimum backoff time has not passed.
        assert_matches!(
            run_handle_roam_trigger_data(&mut exec, &mut test_values.monitor, trigger_data.clone()),
            RoamTriggerDataOutcome::Noop
        );
    }

    #[fuchsia::test]
    fn test_should_send_roam_request() {
        let _exec = fasync::TestExecutor::new();
        let test_values = setup_test();

        // Get the randomized RSSI value.
        let current_rssi = test_values.monitor.connection_data.signal_data.ewma_rssi.get();

        // Verify that roam recommendations are blocked if RSSI is an insufficient improvement.
        let candidate = types::ScannedCandidate {
            bss: types::Bss {
                signal: types::Signal {
                    rssi_dbm: (current_rssi + MIN_RSSI_IMPROVEMENT_TO_ROAM - 1.0) as i8,
                    snr_db: TEST_OK_SNR as i8,
                },
                ..generate_random_bss()
            },
            ..generate_random_scanned_candidate()
        };
        assert!(
            !test_values
                .monitor
                .should_send_roam_request(PolicyRoamRequest { candidate, reasons: vec![] })
                .expect("failed to check roam request")
        );

        // Verify that a roam recommendation is made if RSSI improvement exceeds threshold.
        let candidate = types::ScannedCandidate {
            bss: types::Bss {
                signal: types::Signal {
                    rssi_dbm: (current_rssi + MIN_RSSI_IMPROVEMENT_TO_ROAM) as i8,
                    snr_db: TEST_OK_SNR as i8,
                },
                ..generate_random_bss()
            },
            ..generate_random_scanned_candidate()
        };
        assert!(
            test_values
                .monitor
                .should_send_roam_request(PolicyRoamRequest { candidate, reasons: vec![] })
                .expect("failed to check roam request")
        );

        // Verify that roam recommendations are blocked if the selected candidate is the currently
        // connected BSS. Set signal values high enough to isolate the dedupe function.
        let candidate = types::ScannedCandidate {
            bss: types::Bss {
                signal: types::Signal {
                    rssi_dbm: (current_rssi + MIN_RSSI_IMPROVEMENT_TO_ROAM + 1.0) as i8,
                    snr_db: TEST_OK_SNR as i8,
                },
                bssid: test_values.monitor.connection_data.ap_state.original().bssid,
                ..generate_random_bss()
            },
            credential: generate_random_password(),
            ..generate_random_scanned_candidate()
        };
        assert!(
            !test_values
                .monitor
                .should_send_roam_request(PolicyRoamRequest { candidate, reasons: vec![] })
                .expect("failed to check roam reqeust")
        );
    }

    #[fuchsia::test]
    fn test_send_signal_velocity_metric_event() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::now());

        let connection_data = RoamingConnectionData {
            signal_data: EwmaSignalData::new(-40, 50, 1),
            ..generate_random_roaming_connection_data()
        };
        let mut test_values = setup_test_with_data(connection_data);
        test_values.saved_networks.set_is_single_bss_response(true);

        let trigger_data =
            RoamTriggerData::SignalReportInd(fidl_internal::SignalReportIndication {
                rssi_dbm: -80,
                snr_db: TEST_OK_SNR as i8,
                tx_rate_500kbps: 0,
            });
        let _ =
            run_handle_roam_trigger_data(&mut exec, &mut test_values.monitor, trigger_data.clone());

        assert_matches!(
            test_values.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::OnSignalVelocityUpdate { .. }))
        );
    }

    #[fuchsia::test]
    fn test_should_not_roam_scan_single_bss() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fasync::MonotonicInstant::now());

        let rssi = -80;
        let connection_data = RoamingConnectionData {
            signal_data: EwmaSignalData::new(rssi, TEST_OK_SNR, 10),
            ..generate_random_roaming_connection_data()
        };
        let mut test_values = setup_test_with_data(connection_data);

        // Set the FakeSavedNetworks manager to report the network as single BSS
        test_values.saved_networks.set_is_single_bss_response(true);

        // Advance the time so that we allow roam scanning,
        exec.set_fake_time(fasync::MonotonicInstant::after(fasync::MonotonicDuration::from_hours(
            1,
        )));

        let trigger_data =
            RoamTriggerData::SignalReportInd(fidl_internal::SignalReportIndication {
                rssi_dbm: rssi,
                snr_db: TEST_OK_SNR as i8,
                tx_rate_500kbps: 0,
            });
        let trigger_result =
            run_handle_roam_trigger_data(&mut exec, &mut test_values.monitor, trigger_data.clone());

        assert_eq!(trigger_result, RoamTriggerDataOutcome::Noop);
    }

    #[fuchsia::test]
    fn test_roam_not_considered_if_attempted_too_many_times_today() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let first_roam_time = fasync::MonotonicInstant::now();
        exec.set_fake_time(first_roam_time);

        // Send a signal report that would trigger a roam scan if the limit were not hit.
        let rssi = LOCAL_ROAM_THRESHOLD_RSSI_5G - 5.0;
        let connection_data = RoamingConnectionData {
            signal_data: EwmaSignalData::new(rssi, TEST_OK_SNR, 10),
            ..generate_random_roaming_connection_data()
        };
        let mut test_values = setup_test_with_data(connection_data);

        // Record enough roam attempts to prevent roaming for a while.
        for _ in 0..NUM_MAX_ROAMS_PER_DAY {
            test_values.past_roams.try_lock().unwrap().add(RoamEvent::new_roam_now());
            exec.set_fake_time(fasync::MonotonicInstant::after(MAX_BACKOFF_BETWEEN_ROAM_SCANS));
        }

        let trigger_data =
            RoamTriggerData::SignalReportInd(fidl_internal::SignalReportIndication {
                rssi_dbm: rssi as i8,
                snr_db: TEST_OK_SNR as i8,
                tx_rate_500kbps: 0,
            });

        exec.set_fake_time(fasync::MonotonicInstant::after(
            MAX_BACKOFF_BETWEEN_ROAM_SCANS + zx::MonotonicDuration::from_seconds(1),
        ));

        // The limit on roams per day has been hit, no roam scan should be recommended.
        let should_roam_scan_result =
            run_handle_roam_trigger_data(&mut exec, &mut test_values.monitor, trigger_data.clone());
        assert_eq!(should_roam_scan_result, RoamTriggerDataOutcome::Noop);

        // Advance time to 24 hours past the first roam time. Roam scanning should now be allowed
        // again.
        exec.set_fake_time(
            first_roam_time
                + zx::MonotonicDuration::from_hours(24)
                + zx::MonotonicDuration::from_seconds(1),
        );
        let should_roam_scan_result =
            run_handle_roam_trigger_data(&mut exec, &mut test_values.monitor, trigger_data.clone());
        assert_matches!(should_roam_scan_result, RoamTriggerDataOutcome::RoamSearch { .. });
    }

    #[fuchsia::test]
    fn test_roaming_disabled_timer_is_set_and_respected() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let first_roam_time = fasync::MonotonicInstant::now();
        exec.set_fake_time(first_roam_time);

        // Setup with RSSIthat would trigger a roam scan if the limit were not hit.
        let rssi = LOCAL_ROAM_THRESHOLD_RSSI_5G - 5.0;
        let connection_data = RoamingConnectionData {
            signal_data: EwmaSignalData::new(rssi, TEST_OK_SNR, 10),
            ..generate_random_roaming_connection_data()
        };
        let mut test_values = setup_test_with_data(connection_data);

        // Record enough roam attempts to prevent roaming for a while. The first roam attempt is
        // at time 0, the second at 1 min, etc.
        for i in 0..NUM_MAX_ROAMS_PER_DAY {
            let time_of_roam = first_roam_time + zx::MonotonicDuration::from_minutes(i as i64);
            exec.set_fake_time(time_of_roam);
            test_values.past_roams.try_lock().unwrap().add(RoamEvent::new(time_of_roam));
        }

        let trigger_data =
            RoamTriggerData::SignalReportInd(fidl_internal::SignalReportIndication {
                rssi_dbm: rssi as i8,
                snr_db: TEST_OK_SNR as i8,
                tx_rate_500kbps: 0,
            });

        // Advance time just enough to be after the last roam event.
        exec.set_fake_time(fasync::MonotonicInstant::after(zx::MonotonicDuration::from_minutes(
            NUM_MAX_ROAMS_PER_DAY as i64,
        )));

        // The limit on roams per day has been hit, no roam scan should be recommended.
        let should_roam_scan_result =
            run_handle_roam_trigger_data(&mut exec, &mut test_values.monitor, trigger_data.clone());
        assert_eq!(should_roam_scan_result, RoamTriggerDataOutcome::Noop);

        // Check that the `next_roaming_enabled_time` was set correctly. It should be 24 hours
        // after the *first* roam attempt.
        let expected_reenabled_time = first_roam_time + TIMESPAN_TO_LIMIT_SCANS;
        assert_eq!(test_values.monitor.next_roaming_enabled_time, expected_reenabled_time);

        // Advance time to just BEFORE the re-enable time. Roaming should still be disabled.
        exec.set_fake_time(expected_reenabled_time - zx::MonotonicDuration::from_seconds(1));
        let should_roam_scan_result =
            run_handle_roam_trigger_data(&mut exec, &mut test_values.monitor, trigger_data.clone());
        assert_eq!(should_roam_scan_result, RoamTriggerDataOutcome::Noop);

        // Advance time to just AFTER the re-enable time. Roaming should now be allowed.
        exec.set_fake_time(expected_reenabled_time + zx::MonotonicDuration::from_seconds(1));
        let should_roam_scan_result =
            run_handle_roam_trigger_data(&mut exec, &mut test_values.monitor, trigger_data.clone());
        assert_matches!(should_roam_scan_result, RoamTriggerDataOutcome::RoamSearch { .. });
    }
}
