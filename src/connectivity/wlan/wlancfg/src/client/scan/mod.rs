// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Manages Scan requests for the Client Policy API.
use crate::client::types;
use crate::config_management::SavedNetworksManagerApi;
use crate::mode_management::iface_manager_api::{IfaceManagerApi, SmeForScan};
use crate::telemetry::{ScanEventInspectData, ScanIssue, TelemetryEvent, TelemetrySender};
use anyhow::{Error, format_err};
use async_trait::async_trait;
use fidl_fuchsia_location_sensor as fidl_location_sensor;
use fidl_fuchsia_wlan_policy as fidl_policy;
use fidl_fuchsia_wlan_sme as fidl_sme;
use fuchsia_async::{self as fasync, DurationExt, TimeoutExt};
use fuchsia_component::client::connect_to_protocol;
use futures::channel::{mpsc, oneshot};
use futures::future::{Fuse, FusedFuture, FutureExt};
use futures::lock::Mutex;
use futures::select;
use futures::stream::{FuturesUnordered, StreamExt};
use itertools::Itertools;
use log::{debug, error, info, trace, warn};
use std::collections::HashMap;
use std::pin::pin;
use std::sync::Arc;

mod fidl_conversion;
mod queue;

pub use fidl_conversion::{
    scan_result_to_policy_scan_result, send_scan_error_over_fidl, send_scan_results_over_fidl,
};

// Delay between scanning retries when the firmware returns "ShouldWait" error code
const SCAN_RETRY_DELAY_MS: i64 = 100;
// Max time allowed for consumers of scan results to retrieve results
const SCAN_CONSUMER_MAX_SECONDS_ALLOWED: i64 = 5;
/// Capacity of "first come, first serve" slots available to scan requesters
pub const SCAN_REQUEST_BUFFER_SIZE: usize = 100;

// Inidication of the scan caller, for use in logging caller specific metrics
#[derive(Debug, PartialEq)]
pub enum ScanReason {
    ClientRequest,
    NetworkSelection,
    BssSelection,
    BssSelectionAugmentation,
    RoamSearch,
}

#[async_trait(?Send)]
pub trait ScanRequestApi {
    async fn perform_scan(
        &self,
        scan_reason: ScanReason,
        ssids: Vec<types::Ssid>,
        channels: Vec<types::WlanChan>,
    ) -> Result<Vec<types::ScanResult>, types::ScanError>;
}

pub struct ScanRequester {
    pub sender: mpsc::Sender<ApiScanRequest>,
}

pub enum ApiScanRequest {
    Scan(
        ScanReason,
        Vec<types::Ssid>,
        Vec<types::WlanChan>,
        oneshot::Sender<Result<Vec<types::ScanResult>, types::ScanError>>,
    ),
}

#[async_trait(?Send)]
impl ScanRequestApi for ScanRequester {
    async fn perform_scan(
        &self,
        scan_reason: ScanReason,
        ssids: Vec<types::Ssid>,
        channels: Vec<types::WlanChan>,
    ) -> Result<Vec<types::ScanResult>, types::ScanError> {
        let (responder, receiver) = oneshot::channel();
        self.sender
            .clone()
            .try_send(ApiScanRequest::Scan(scan_reason, ssids, channels, responder))
            .map_err(|e| {
                error!("Failed to send ScanRequest: {:?}", e);
                types::ScanError::GeneralError
            })?;
        receiver.await.map_err(|e| {
            error!("Failed to receive ScanRequest response: {:?}", e);
            types::ScanError::GeneralError
        })?
    }
}

// Wrapper struct to track it the scan request is being retried.
#[derive(Debug, Clone)]
struct ScanRequest {
    pub sme_req: fidl_sme::ScanRequest,
    pub is_retry: bool,
}
impl From<fidl_sme::ScanRequest> for ScanRequest {
    fn from(sme_req: fidl_sme::ScanRequest) -> Self {
        Self { sme_req, is_retry: false }
    }
}

/// Create a future representing the scan manager loop.
pub async fn serve_scanning_loop(
    iface_manager: Arc<Mutex<dyn IfaceManagerApi>>,
    saved_networks_manager: Arc<dyn SavedNetworksManagerApi>,
    telemetry_sender: TelemetrySender,
    location_sensor_updater: impl ScanResultUpdate,
    mut scan_request_channel: mpsc::Receiver<ApiScanRequest>,
) -> Result<(), Error> {
    let mut queue = queue::RequestQueue::new(telemetry_sender.clone());
    let mut location_sensor_updates = FuturesUnordered::new();
    // Use `Fuse::terminated()` to create an already-terminated future
    // which may be instantiated later.
    let ongoing_scan = Fuse::terminated();
    let mut ongoing_scan = pin!(ongoing_scan);

    let transform_next_sme_req = |next_sme_req: Option<ScanRequest>| match next_sme_req {
        None => Fuse::terminated(),
        Some(next_sme_req) => perform_scan(
            next_sme_req,
            iface_manager.clone(),
            saved_networks_manager.clone(),
            telemetry_sender.clone(),
        )
        .fuse(),
    };

    loop {
        select! {
            request = scan_request_channel.next() => {
                match request {
                    Some(ApiScanRequest::Scan(reason, ssids, channels, responder)) => {
                        queue.add_request(reason, ssids, channels, responder, zx::MonotonicInstant::get());
                        // Check if there's an ongoing scan, otherwise take one from the queue
                        if ongoing_scan.is_terminated() {
                            ongoing_scan.set(transform_next_sme_req(queue.get_next_sme_request().map(|req| req.into())));
                        }
                    },
                    None => {
                        error!("Unexpected 'None' on scan_request_channel");
                    }
                }
            },
            (completed_sme_request, scan_results) = ongoing_scan => {
                if scan_results == Err(types::ScanError::Cancelled) && !completed_sme_request.is_retry {
                    // Retry after delay on first cancellation attempt.
                    info!("Driver requested a delay before retrying the cancelled scan request.");
                    fasync::Timer::new(zx::MonotonicDuration::from_millis(SCAN_RETRY_DELAY_MS).after_now()).await;
                    ongoing_scan.set(transform_next_sme_req(Some(ScanRequest {is_retry: true, ..completed_sme_request})));
                    continue;
                }
                match scan_results {
                    Ok(ref results) => {
                        // Return results to requester.
                        queue.handle_completed_sme_scan(
                            completed_sme_request.sme_req.clone(),
                            scan_results.clone(),
                            zx::MonotonicInstant::get()
                        );
                        // Send scan results to Location
                        if !results.is_empty() {
                            location_sensor_updates.push(location_sensor_updater
                                .update_scan_results(results.clone())
                                .on_timeout(zx::MonotonicDuration::from_seconds(SCAN_CONSUMER_MAX_SECONDS_ALLOWED), || {
                                    error!("Timed out waiting for location sensor to get results");
                                })
                            );
                        }
                    },
                    Err(error) => {
                        // Handle all other cases (including cancellation after retry). Return
                        // results immediately.
                        queue.handle_completed_sme_scan(
                            completed_sme_request.sme_req.clone(),
                            scan_results.clone(),
                            zx::MonotonicInstant::get()
                        );
                        // If there was a cancellation after retrying, add a back off.
                        if error == types::ScanError::Cancelled {
                            info!("After multiple cancelled attempts, driver requested a delay before serving new scan requests.");
                            fasync::Timer::new(zx::MonotonicDuration::from_millis(SCAN_RETRY_DELAY_MS).after_now()).await;
                        }
                    }
                }
                // Fetch the next request
                ongoing_scan.set(transform_next_sme_req(
                    queue.get_next_sme_request().map(|req| req.into())
                ));
            },
            () = location_sensor_updates.select_next_some() => {},
            complete => {
                // all futures are terminated
                warn!("Unexpectedly reached end of scanning loop");
                break
            },
        }
    }

    Err(format_err!("Unexpectedly reached end of scanning loop"))
}

/// Allows for consumption of updated scan results.
#[async_trait(?Send)]
pub trait ScanResultUpdate {
    async fn update_scan_results(&self, scan_results: Vec<types::ScanResult>);
}

/// Requests a new SME scan and returns the results.
async fn sme_scan(
    sme_proxy: &SmeForScan,
    scan_request: &fidl_sme::ScanRequest,
    scan_defects: &mut Vec<ScanIssue>,
) -> Result<Vec<wlan_common::scan::ScanResult>, types::ScanError> {
    debug!("Sending scan request to SME");
    let scan_result = sme_proxy.scan(scan_request).await.map_err(|error| {
        error!("Failed to send scan to SME: {:?}", error);
        types::ScanError::GeneralError
    })?;
    debug!("Finished getting scan results from SME");
    match scan_result {
        Ok(vmo) => {
            let scan_result_list = wlan_common::scan::read_vmo(vmo).map_err(|error| {
                error!("Failed to read scan results from VMO: {:?}", error);
                types::ScanError::GeneralError
            })?;
            Ok(scan_result_list
                .into_iter()
                .filter_map(|scan_result| {
                    wlan_common::scan::ScanResult::try_from(scan_result).map(Some).unwrap_or_else(
                        |e| {
                            // TODO(https://fxbug.dev/42164415): Report details about which
                            // scan result failed to convert if possible.
                            error!("ScanResult conversion failed: {:?}", e);
                            None
                        },
                    )
                })
                .inspect(|scan_result| {
                    // This trace-level logging is only enabled when a user manually sets the log
                    // level to TRACE with `fx log` or `fx test`.
                    trace!(
                        "Scan result SSID: {}, BSSID: {}, channel: {}",
                        scan_result.bss_description.ssid,
                        scan_result.bss_description.bssid,
                        scan_result.bss_description.channel
                    )
                })
                .collect::<Vec<_>>())
        }
        Err(scan_error_code) => {
            log_metric_for_scan_error(&scan_error_code, scan_defects);
            match scan_error_code {
                fidl_sme::ScanErrorCode::ShouldWait
                | fidl_sme::ScanErrorCode::CanceledByDriverOrFirmware => {
                    info!("Scan cancelled by SME, retry indicated: {:?}", scan_error_code);
                    Err(types::ScanError::Cancelled)
                }
                _ => {
                    error!("Scan error from SME: {:?}", scan_error_code);
                    Err(types::ScanError::GeneralError)
                }
            }
        }
    }
}

/// Handles incoming scan requests by creating a new SME scan request.
async fn perform_scan(
    scan_request: ScanRequest,
    iface_manager: Arc<Mutex<dyn IfaceManagerApi>>,
    saved_networks_manager: Arc<dyn SavedNetworksManagerApi>,
    telemetry_sender: TelemetrySender,
) -> (ScanRequest, Result<Vec<types::ScanResult>, types::ScanError>) {
    let bss_by_network: HashMap<types::NetworkIdentifierDetailed, Vec<types::Bss>>;
    let mut scan_event_inspect_data = ScanEventInspectData::new();
    let mut scan_defects: Vec<ScanIssue> = vec![];

    let sme_proxy = match iface_manager.lock().await.get_sme_proxy_for_scan().await {
        Ok(proxy) => proxy,
        Err(_) => {
            warn!("Failed to get sme proxy for passive scan");
            return (scan_request, Err(types::ScanError::GeneralError));
        }
    };
    telemetry_sender.send(TelemetryEvent::SmeScanStart);
    let scan_results = sme_scan(&sme_proxy, &scan_request.sme_req, &mut scan_defects).await;
    let sme_scan_result = match &scan_results {
        Ok(results) => wlan_telemetry::ScanResult::Complete { num_results: results.len() },
        Err(types::ScanError::Cancelled) => wlan_telemetry::ScanResult::Cancelled,
        Err(_) => wlan_telemetry::ScanResult::Failed,
    };
    telemetry_sender.send(TelemetryEvent::SmeScanResult { result: sme_scan_result });
    report_scan_defects_to_sme(&sme_proxy, &scan_results, &scan_request.sme_req).await;

    match scan_results {
        Ok(results) => {
            // Record observed BSSs to saved networks manager.
            let target_ssids = match scan_request.sme_req {
                fidl_sme::ScanRequest::Passive(_) => vec![],
                fidl_sme::ScanRequest::Active(ref req) => req
                    .ssids
                    .iter()
                    .map(|s| types::Ssid::from_bytes_unchecked(s.to_vec()))
                    .collect(),
            };
            bss_by_network =
                bss_to_network_map(results, &target_ssids, &mut scan_event_inspect_data);
            saved_networks_manager.record_scan_result(target_ssids, &bss_by_network).await;
        }
        Err(scan_err) => {
            return (scan_request, Err(scan_err));
        }
    }

    // If the passive scan results are empty, report an empty scan results metric.
    if let fidl_sme::ScanRequest::Passive(_) = scan_request.sme_req
        && bss_by_network.is_empty()
    {
        scan_defects.push(ScanIssue::EmptyScanResults);
    }

    telemetry_sender
        .send(TelemetryEvent::ScanEvent { inspect_data: scan_event_inspect_data, scan_defects });

    let scan_results = network_map_to_scan_result(bss_by_network);
    (scan_request, Ok(scan_results))
}

/// The location sensor module uses scan results to help determine the
/// device's location, for use by the Emergency Location Provider.
pub struct LocationSensorUpdater {}
#[async_trait(?Send)]
impl ScanResultUpdate for LocationSensorUpdater {
    async fn update_scan_results(&self, scan_results: Vec<types::ScanResult>) {
        async fn send_results(scan_results: Vec<fidl_policy::ScanResult>) -> Result<(), Error> {
            // Get an output iterator
            let (iter, server) =
                fidl::endpoints::create_endpoints::<fidl_policy::ScanResultIteratorMarker>();
            let location_watcher_proxy =
                connect_to_protocol::<fidl_location_sensor::WlanBaseStationWatcherMarker>()
                    .map_err(|err| {
                        format_err!("failed to connect to location sensor service: {:?}", err)
                    })?;
            location_watcher_proxy
                .report_current_stations(iter)
                .map_err(|err| format_err!("failed to call location sensor service: {:?}", err))?;

            // Send results to the iterator
            fidl_conversion::send_scan_results_over_fidl(server, &scan_results).await
        }

        let scan_results = fidl_conversion::scan_result_to_policy_scan_result(&scan_results);
        // Filter out any errors and just log a message.
        // No error recovery, we'll just try again next time a scan result comes in.
        if let Err(e) = send_results(scan_results).await {
            info!("Failed to send scan results to location sensor: {:?}", e)
        } else {
            debug!("Updated location sensor")
        };
    }
}

/// Converts sme::ScanResult to our internal BSS type, then adds it to a map.
/// Only keeps the first unique instance of a BSSID
fn bss_to_network_map(
    scan_result_list: Vec<wlan_common::scan::ScanResult>,
    target_ssids: &[types::Ssid],
    scan_event_inspect_data: &mut ScanEventInspectData,
) -> HashMap<types::NetworkIdentifierDetailed, Vec<types::Bss>> {
    let mut bss_by_network: HashMap<types::NetworkIdentifierDetailed, Vec<types::Bss>> =
        HashMap::new();
    for scan_result in scan_result_list.into_iter() {
        let security_type: types::SecurityTypeDetailed =
            scan_result.bss_description.protection().into();
        if security_type == types::SecurityTypeDetailed::Unknown {
            // Log a space-efficient version of the IEs.
            let readable_ie =
                scan_result.bss_description.ies().iter().map(|n| n.to_string()).join(",");
            debug!("Encountered unknown protection, ies: [{:?}]", readable_ie);
            scan_event_inspect_data.unknown_protection_ies.push(readable_ie);
        };
        let entry = bss_by_network
            .entry(types::NetworkIdentifierDetailed {
                ssid: scan_result.bss_description.ssid.clone(),
                security_type,
            })
            .or_default();

        // Check if this BSSID is already in the hashmap
        if !entry.iter().any(|existing_bss| existing_bss.bssid == scan_result.bss_description.bssid)
        {
            entry.push(types::Bss {
                bssid: scan_result.bss_description.bssid,
                signal: types::Signal {
                    rssi_dbm: scan_result.bss_description.rssi_dbm,
                    snr_db: scan_result.bss_description.snr_db,
                },
                channel: scan_result.bss_description.channel,
                timestamp: scan_result.timestamp,
                // TODO(123709): if target_ssids contains the wildcard, this need to be "Unknown"
                observation: if target_ssids.contains(&scan_result.bss_description.ssid) {
                    types::ScanObservation::Active
                } else {
                    types::ScanObservation::Passive
                },
                compatibility: scan_result.compatibility,
                bss_description: wlan_common::sequestered::Sequestered::from(
                    fidl_fuchsia_wlan_ieee80211::BssDescription::from(scan_result.bss_description),
                ),
            });
        };
    }
    bss_by_network
}

fn network_map_to_scan_result(
    mut bss_by_network: HashMap<types::NetworkIdentifierDetailed, Vec<types::Bss>>,
) -> Vec<types::ScanResult> {
    let mut scan_results: Vec<types::ScanResult> = bss_by_network
        .drain()
        .map(|(types::NetworkIdentifierDetailed { ssid, security_type }, bss_entries)| {
            let compatibility = if bss_entries.iter().any(|bss| bss.is_compatible()) {
                fidl_policy::Compatibility::Supported
            } else {
                fidl_policy::Compatibility::DisallowedNotSupported
            };
            types::ScanResult {
                ssid,
                security_type_detailed: security_type,
                entries: bss_entries,
                compatibility,
            }
        })
        .collect();

    scan_results.sort_by(|a, b| a.ssid.cmp(&b.ssid));
    scan_results
}

fn log_metric_for_scan_error(reason: &fidl_sme::ScanErrorCode, scan_defects: &mut Vec<ScanIssue>) {
    let metric_type = match *reason {
        fidl_sme::ScanErrorCode::NotSupported
        | fidl_sme::ScanErrorCode::InternalError
        | fidl_sme::ScanErrorCode::InternalMlmeError => ScanIssue::ScanFailure,
        fidl_sme::ScanErrorCode::ShouldWait
        | fidl_sme::ScanErrorCode::CanceledByDriverOrFirmware => ScanIssue::AbortedScan,
    };

    scan_defects.push(metric_type);
}

async fn report_scan_defects_to_sme(
    sme_proxy: &SmeForScan,
    scan_result: &Result<Vec<wlan_common::scan::ScanResult>, types::ScanError>,
    scan_request: &fidl_sme::ScanRequest,
) {
    match scan_result {
        Ok(results) => {
            // If passive scan results are empty, report an empty scan results metric and defect.
            if results.is_empty()
                && let fidl_sme::ScanRequest::Passive(_) = scan_request
            {
                sme_proxy.log_empty_scan_defect();
            }
        }
        Err(types::ScanError::GeneralError) => sme_proxy.log_failed_scan_defect(),
        Err(types::ScanError::Cancelled) => sme_proxy.log_aborted_scan_defect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access_point::state_machine as ap_fsm;
    use crate::mode_management::iface_manager_api::ConnectAttemptRequest;
    use crate::mode_management::{Defect, IfaceFailure};
    use crate::util::testing::fakes::FakeSavedNetworksManager;
    use crate::util::testing::{
        generate_channel, generate_random_sme_scan_result, run_until_completion,
    };
    use assert_matches::assert_matches;
    use fidl::endpoints::{ControlHandle, Responder, create_proxy};
    use fidl_fuchsia_wlan_ieee80211::WlanBand::{FiveGhz, TwoGhz};
    use fidl_fuchsia_wlan_internal as fidl_internal;
    use fuchsia_async as fasync;
    use futures::future;
    use futures::task::Poll;
    use std::pin::pin;
    use test_case::test_case;
    use wlan_common::ie::IeType;
    use wlan_common::scan::{Compatible, Incompatible, write_vmo};
    use wlan_common::security::SecurityDescriptor;
    use wlan_common::test_utils::fake_frames::fake_unknown_rsne;
    use wlan_common::test_utils::fake_stas::IesOverrides;
    use wlan_common::{fake_bss_description, random_fidl_bss_description};

    fn active_sme_req(ssids: Vec<&str>, channels: Vec<u8>) -> fidl_sme::ScanRequest {
        fidl_sme::ScanRequest::Active(fidl_sme::ActiveScanRequest {
            ssids: ssids.iter().map(|s| s.as_bytes().to_vec()).collect(),
            channels,
        })
    }

    fn passive_sme_req() -> fidl_sme::ScanRequest {
        fidl_sme::ScanRequest::Passive(fidl_sme::PassiveScanRequest { channels: vec![] })
    }

    struct FakeIfaceManager {
        pub sme_proxy: fidl_fuchsia_wlan_sme::ClientSmeProxy,
        pub defect_sender: mpsc::Sender<Defect>,
        pub defect_receiver: mpsc::Receiver<Defect>,
    }

    impl FakeIfaceManager {
        pub fn new(proxy: fidl_fuchsia_wlan_sme::ClientSmeProxy) -> Self {
            let (defect_sender, defect_receiver) = mpsc::channel(100);
            FakeIfaceManager { sme_proxy: proxy, defect_sender, defect_receiver }
        }
    }

    #[async_trait(?Send)]
    impl IfaceManagerApi for FakeIfaceManager {
        async fn disconnect(
            &mut self,
            _network_id: types::NetworkIdentifier,
            _reason: types::DisconnectReason,
        ) -> Result<(), Error> {
            unimplemented!()
        }

        async fn connect(&mut self, _connect_req: ConnectAttemptRequest) -> Result<(), Error> {
            unimplemented!()
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
            Ok(SmeForScan::new(self.sme_proxy.clone(), 0, self.defect_sender.clone()))
        }

        async fn stop_client_connections(
            &mut self,
            _reason: types::DisconnectReason,
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

    /// Creates a Client wrapper.
    async fn create_iface_manager()
    -> (Arc<Mutex<FakeIfaceManager>>, fidl_sme::ClientSmeRequestStream) {
        let (client_sme, remote) = create_proxy::<fidl_sme::ClientSmeMarker>();
        let iface_manager = FakeIfaceManager::new(client_sme);
        let iface_manager = Arc::new(Mutex::new(iface_manager));
        (iface_manager, remote.into_stream())
    }

    /// Creates an SME proxy for tests.
    async fn create_sme_proxy() -> (fidl_sme::ClientSmeProxy, fidl_sme::ClientSmeRequestStream) {
        let (client_sme, remote) = create_proxy::<fidl_sme::ClientSmeMarker>();
        (client_sme, remote.into_stream())
    }

    struct MockScanResultConsumer {
        scan_results: Arc<Mutex<Option<Vec<types::ScanResult>>>>,
        stalled: Arc<Mutex<bool>>,
    }
    impl MockScanResultConsumer {
        #[allow(clippy::type_complexity)]
        fn new() -> (Self, Arc<Mutex<Option<Vec<types::ScanResult>>>>, Arc<Mutex<bool>>) {
            let scan_results = Arc::new(Mutex::new(None));
            let stalled = Arc::new(Mutex::new(false));
            (
                Self { scan_results: scan_results.clone(), stalled: stalled.clone() },
                scan_results,
                stalled,
            )
        }
    }
    #[async_trait(?Send)]
    impl ScanResultUpdate for MockScanResultConsumer {
        async fn update_scan_results(&self, scan_results: Vec<types::ScanResult>) {
            if *self.stalled.lock().await {
                let () = future::pending().await;
                unreachable!();
            }
            let mut guard = self.scan_results.lock().await;
            *guard = Some(scan_results);
        }
    }

    // Creates test data for the scan functions.
    struct MockScanData {
        sme_results: Vec<fidl_sme::ScanResult>,
        internal_results: Vec<types::ScanResult>,
    }
    fn create_scan_ap_data(observation: types::ScanObservation) -> MockScanData {
        let sme_result_1 = fidl_sme::ScanResult {
            compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                mutual_security_protocols: vec![fidl_internal::Protocol::Wpa3Personal],
            }),
            timestamp_nanos: zx::MonotonicInstant::get().into_nanos(),
            bss_description: random_fidl_bss_description!(
                Wpa3,
                bssid: [0, 0, 0, 0, 0, 0],
                ssid: types::Ssid::try_from("duplicated ssid").unwrap(),
                rssi_dbm: 0,
                snr_db: 1,
                channel: types::WlanChan::new(1, types::Bandwidth::Cbw20, TwoGhz),
            ),
        };
        let sme_result_2 = fidl_sme::ScanResult {
            compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                mutual_security_protocols: vec![fidl_internal::Protocol::Wpa2Personal],
            }),
            timestamp_nanos: zx::MonotonicInstant::get().into_nanos(),
            bss_description: random_fidl_bss_description!(
                Wpa2,
                bssid: [1, 2, 3, 4, 5, 6],
                ssid: types::Ssid::try_from("unique ssid").unwrap(),
                rssi_dbm: 7,
                snr_db: 2,
                channel: types::WlanChan::new(8, types::Bandwidth::Cbw20, TwoGhz),
            ),
        };
        let sme_result_3 = fidl_sme::ScanResult {
            compatibility: fidl_sme::Compatibility::Incompatible(fidl_sme::Incompatible {
                description: String::from("unknown"),
                disjoint_security_protocols: None,
            }),
            timestamp_nanos: zx::MonotonicInstant::get().into_nanos(),
            bss_description: random_fidl_bss_description!(
                Wpa3,
                bssid: [7, 8, 9, 10, 11, 12],
                ssid: types::Ssid::try_from("duplicated ssid").unwrap(),
                rssi_dbm: 13,
                snr_db: 3,
                channel: types::WlanChan::new(11, types::Bandwidth::Cbw20, TwoGhz),
            ),
        };

        let sme_results = vec![sme_result_1.clone(), sme_result_2.clone(), sme_result_3.clone()];
        // input_aps contains some duplicate SSIDs, which should be
        // grouped in the output.
        let internal_results = vec![
            types::ScanResult {
                ssid: types::Ssid::try_from("duplicated ssid").unwrap(),
                security_type_detailed: types::SecurityTypeDetailed::Wpa3Personal,
                entries: vec![
                    types::Bss {
                        bssid: types::Bssid::from([0, 0, 0, 0, 0, 0]),
                        signal: types::Signal { rssi_dbm: 0, snr_db: 1 },
                        timestamp: zx::MonotonicInstant::from_nanos(sme_result_1.timestamp_nanos),
                        channel: types::WlanChan::new(1, types::Bandwidth::Cbw20, TwoGhz),
                        observation,
                        compatibility: Compatible::expect_ok([SecurityDescriptor::WPA3_PERSONAL]),
                        bss_description: sme_result_1.bss_description.clone().into(),
                    },
                    types::Bss {
                        bssid: types::Bssid::from([7, 8, 9, 10, 11, 12]),
                        signal: types::Signal { rssi_dbm: 13, snr_db: 3 },
                        timestamp: zx::MonotonicInstant::from_nanos(sme_result_3.timestamp_nanos),
                        channel: types::WlanChan::new(11, types::Bandwidth::Cbw20, TwoGhz),
                        observation,
                        compatibility: Incompatible::unknown(),
                        bss_description: sme_result_3.bss_description.clone().into(),
                    },
                ],
                compatibility: types::Compatibility::Supported,
            },
            types::ScanResult {
                ssid: types::Ssid::try_from("unique ssid").unwrap(),
                security_type_detailed: types::SecurityTypeDetailed::Wpa2Personal,
                entries: vec![types::Bss {
                    bssid: types::Bssid::from([1, 2, 3, 4, 5, 6]),
                    signal: types::Signal { rssi_dbm: 7, snr_db: 2 },
                    timestamp: zx::MonotonicInstant::from_nanos(sme_result_2.timestamp_nanos),
                    channel: types::WlanChan::new(8, types::Bandwidth::Cbw20, TwoGhz),
                    observation,
                    compatibility: Compatible::expect_ok([SecurityDescriptor::WPA2_PERSONAL]),
                    bss_description: sme_result_2.bss_description.clone().into(),
                }],
                compatibility: types::Compatibility::Supported,
            },
        ];

        MockScanData { sme_results, internal_results }
    }

    fn create_telemetry_sender_and_receiver() -> (TelemetrySender, mpsc::Receiver<TelemetryEvent>) {
        let (sender, receiver) = mpsc::channel::<TelemetryEvent>(100);
        let sender = TelemetrySender::new(sender);
        (sender, receiver)
    }

    fn get_fake_defects(
        exec: &mut fasync::TestExecutor,
        iface_manager: Arc<Mutex<FakeIfaceManager>>,
    ) -> Vec<Defect> {
        let defects_fut = async move {
            let mut iface_manager = iface_manager.lock().await;
            let mut defects = Vec::<Defect>::new();
            while let Ok(Some(defect)) = iface_manager.defect_receiver.try_next() {
                defects.push(defect)
            }

            defects
        };
        let mut defects_fut = pin!(defects_fut);
        assert_matches!(exec.run_until_stalled(&mut defects_fut), Poll::Ready(defects) => defects)
    }

    #[fuchsia::test]
    fn sme_scan_with_passive_request() {
        let mut exec = fasync::TestExecutor::new();
        let (sme_proxy, mut sme_stream) = exec.run_singlethreaded(create_sme_proxy());
        let (defect_sender, _) = mpsc::channel(100);
        let sme_proxy = SmeForScan::new(sme_proxy, 0, defect_sender);

        // Issue request to scan.
        let scan_request =
            fidl_sme::ScanRequest::Passive(fidl_sme::PassiveScanRequest { channels: vec![] });
        let mut scan_defects = vec![];
        let scan_fut = sme_scan(&sme_proxy, &scan_request, &mut scan_defects);
        let mut scan_fut = pin!(scan_fut);

        // Request scan data from SME
        assert_matches!(exec.run_until_stalled(&mut scan_fut), Poll::Pending);

        // Create mock scan data
        let MockScanData { sme_results: input_aps, internal_results: _ } =
            create_scan_ap_data(types::ScanObservation::Passive);
        // Validate the SME received the scan_request and send back mock data
        assert_matches!(
            exec.run_until_stalled(&mut sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Scan {
                req, responder,
            }))) => {
                assert_eq!(req, scan_request);
                let vmo = write_vmo(input_aps.clone()).expect("failed to write VMO");
                responder.send(Ok(vmo)).expect("failed to send scan data");
            }
        );

        // Check for results
        assert_matches!(exec.run_until_stalled(&mut scan_fut), Poll::Ready(result) => {
            let scan_results: Vec<fidl_sme::ScanResult> = result.expect("failed to get scan results")
                .iter().map(|r| r.clone().into()).collect();
            assert_eq!(scan_results, input_aps);
        });

        // No further requests to the sme
        assert_matches!(exec.run_until_stalled(&mut sme_stream.next()), Poll::Pending);
    }

    #[fuchsia::test]
    fn sme_scan_with_active_request() {
        let mut exec = fasync::TestExecutor::new();
        let (sme_proxy, mut sme_stream) = exec.run_singlethreaded(create_sme_proxy());
        let (defect_sender, _) = mpsc::channel(100);
        let sme_proxy = SmeForScan::new(sme_proxy, 0, defect_sender);

        // Issue request to scan.
        let scan_request = fidl_sme::ScanRequest::Active(fidl_sme::ActiveScanRequest {
            ssids: vec![
                types::Ssid::try_from("foo_ssid").unwrap().into(),
                types::Ssid::try_from("bar_ssid").unwrap().into(),
            ],
            channels: vec![1, 20],
        });
        let mut scan_defects = vec![];
        let scan_fut = sme_scan(&sme_proxy, &scan_request, &mut scan_defects);
        let mut scan_fut = pin!(scan_fut);

        // Request scan data from SME
        assert_matches!(exec.run_until_stalled(&mut scan_fut), Poll::Pending);

        // Create mock scan data
        let MockScanData { sme_results: input_aps, internal_results: _ } =
            create_scan_ap_data(types::ScanObservation::Active);
        // Validate the SME received the scan_request and send back mock data
        assert_matches!(
            exec.run_until_stalled(&mut sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Scan {
                req, responder,
            }))) => {
                assert_eq!(req, scan_request);
                let vmo = write_vmo(input_aps.clone()).expect("failed to write VMO");
                responder.send(Ok(vmo)).expect("failed to send scan data");
            }
        );

        // Check for results
        assert_matches!(exec.run_until_stalled(&mut scan_fut), Poll::Ready(result) => {
            let scan_results: Vec<fidl_sme::ScanResult> = result.expect("failed to get scan results")
                .iter().map(|r| r.clone().into()).collect();
            assert_eq!(scan_results, input_aps);
        });

        // No further requests to the sme
        assert_matches!(exec.run_until_stalled(&mut sme_stream.next()), Poll::Pending);
    }

    #[fuchsia::test]
    fn sme_channel_closed_while_awaiting_scan_results() {
        let mut exec = fasync::TestExecutor::new();
        let (sme_proxy, mut sme_stream) = exec.run_singlethreaded(create_sme_proxy());
        let (defect_sender, _) = mpsc::channel(100);
        let sme_proxy = SmeForScan::new(sme_proxy, 0, defect_sender);

        // Issue request to scan.
        let scan_request =
            fidl_sme::ScanRequest::Passive(fidl_sme::PassiveScanRequest { channels: vec![] });
        let mut scan_defects = vec![];
        let scan_fut = sme_scan(&sme_proxy, &scan_request, &mut scan_defects);
        let mut scan_fut = pin!(scan_fut);

        // Request scan data from SME
        assert_matches!(exec.run_until_stalled(&mut scan_fut), Poll::Pending);

        // Check that a scan request was sent to the sme and close the channel
        assert_matches!(
            exec.run_until_stalled(&mut sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Scan {
                req: _, responder,
            }))) => {
                // Shutdown SME request stream.
                responder.control_handle().shutdown();
                // TODO(https://fxbug.dev/42161447): Drop the stream to shutdown the channel.
                drop(sme_stream);
            }
        );

        // Check for results
        assert_matches!(exec.run_until_stalled(&mut scan_fut), Poll::Ready(result) => {
            let error = result.expect_err("did not expect scan results");
            assert_eq!(error, types::ScanError::GeneralError);
        });
    }

    #[fuchsia::test]
    fn basic_scan() {
        let mut exec = fasync::TestExecutor::new();
        let (client, mut sme_stream) = exec.run_singlethreaded(create_iface_manager());
        let saved_networks_manager = Arc::new(FakeSavedNetworksManager::new());
        let (telemetry_sender, mut telemetry_receiver) = create_telemetry_sender_and_receiver();
        // Issue request to scan.
        let sme_scan = passive_sme_req();
        let scan_fut =
            perform_scan(sme_scan.clone().into(), client, saved_networks_manager, telemetry_sender);
        let mut scan_fut = pin!(scan_fut);

        // Progress scan handler forward so that it will respond to the iterator get next request.
        assert_matches!(exec.run_until_stalled(&mut scan_fut), Poll::Pending);

        // Create mock scan data and send it via the SME
        let MockScanData { sme_results: input_aps, internal_results: internal_aps } =
            create_scan_ap_data(types::ScanObservation::Passive);
        assert_matches!(
            exec.run_until_stalled(&mut sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Scan {
                req, responder,
            }))) => {
                assert_eq!(req, sme_scan.clone());
                let vmo = write_vmo(input_aps).expect("failed to write VMO");
                responder.send(Ok(vmo)).expect("failed to send scan data");
            }
        );

        // Process scan handler
        assert_matches!(exec.run_until_stalled(&mut scan_fut), Poll::Ready((request, results)) => {
            assert_eq!(request.sme_req, sme_scan);
            assert_eq!(results.unwrap(), internal_aps);
        });

        // Since the scanning process went off without a hitch, there should not be any defect
        // metrics logged.
        assert_matches!(telemetry_receiver.try_next(), Ok(Some(TelemetryEvent::SmeScanStart)));
        assert_matches!(
            telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::SmeScanResult {
                result: wlan_telemetry::ScanResult::Complete { num_results: 3 }
            }))
        );
        assert_matches!(
            telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::ScanEvent {inspect_data, scan_defects})) => {
                assert_eq!(inspect_data, ScanEventInspectData::new());
                assert_eq!(scan_defects, vec![]);
        });
    }

    #[fuchsia::test]
    fn empty_passive_scan_results() {
        let mut exec = fasync::TestExecutor::new();
        let (client, mut sme_stream) = exec.run_singlethreaded(create_iface_manager());
        let saved_networks_manager = Arc::new(FakeSavedNetworksManager::new());
        let (telemetry_sender, mut telemetry_receiver) = create_telemetry_sender_and_receiver();

        // Issue request to scan.
        let sme_scan = passive_sme_req();
        let scan_fut = perform_scan(
            sme_scan.clone().into(),
            client.clone(),
            saved_networks_manager,
            telemetry_sender,
        );
        let mut scan_fut = pin!(scan_fut);

        // Progress scan handler
        assert_matches!(exec.run_until_stalled(&mut scan_fut), Poll::Pending);

        // Send back empty scan results via the SME
        assert_matches!(
            exec.run_until_stalled(&mut sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Scan {
                req, responder,
            }))) => {
                assert_eq!(req, sme_scan.clone());
                let vmo = write_vmo(vec![]).expect("failed to write VMO");
                responder.send(Ok(vmo)).expect("failed to send scan data");
            }
        );

        // Process response from SME (which is empty) and expect the future to complete.
        assert_matches!(exec.run_until_stalled(&mut scan_fut),  Poll::Ready((request, results)) => {
            assert_eq!(request.sme_req, sme_scan);
            assert!(results.unwrap().is_empty());
        });

        // Verify that an empty scan result has been logged
        assert_matches!(telemetry_receiver.try_next(), Ok(Some(TelemetryEvent::SmeScanStart)));
        assert_matches!(
            telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::SmeScanResult {
                result: wlan_telemetry::ScanResult::Complete { num_results: 0 }
            }))
        );
        assert_matches!(
            telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::ScanEvent {inspect_data, scan_defects})) => {
                assert_eq!(inspect_data, ScanEventInspectData::new());
                assert_eq!(scan_defects, vec![ScanIssue::EmptyScanResults]);
        });

        // Verify that a defect was logged.
        let logged_defects = get_fake_defects(&mut exec, client);
        let expected_defects = vec![Defect::Iface(IfaceFailure::EmptyScanResults { iface_id: 0 })];
        assert_eq!(logged_defects, expected_defects);
    }

    #[fuchsia::test]
    fn empty_active_scan_results() {
        let mut exec = fasync::TestExecutor::new();
        let (client, mut sme_stream) = exec.run_singlethreaded(create_iface_manager());
        let saved_networks_manager = Arc::new(FakeSavedNetworksManager::new());
        let (telemetry_sender, mut telemetry_receiver) = create_telemetry_sender_and_receiver();

        // Issue request to scan.
        let sme_scan = active_sme_req(vec!["foo"], vec![]);
        let scan_fut = perform_scan(
            sme_scan.clone().into(),
            client.clone(),
            saved_networks_manager,
            telemetry_sender,
        );
        let mut scan_fut = pin!(scan_fut);

        // Progress scan handler
        assert_matches!(exec.run_until_stalled(&mut scan_fut), Poll::Pending);

        // Send back empty scan results via the SME
        assert_matches!(
            exec.run_until_stalled(&mut sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Scan {
                req, responder,
            }))) => {
                assert_eq!(req, sme_scan.clone());
                let vmo = write_vmo(vec![]).expect("failed to write VMO");
                responder.send(Ok(vmo)).expect("failed to send scan data");
            }
        );

        // Process response from SME (which is empty) and expect the future to complete.
        assert_matches!(exec.run_until_stalled(&mut scan_fut),  Poll::Ready((request, results)) => {
            assert_eq!(request.sme_req, sme_scan);
            assert!(results.unwrap().is_empty());
        });

        // Verify that a scan defect has not been logged; this should only be logged for
        // passive scans because it is common for active scans.
        assert_matches!(telemetry_receiver.try_next(), Ok(Some(TelemetryEvent::SmeScanStart)));
        assert_matches!(
            telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::SmeScanResult {
                result: wlan_telemetry::ScanResult::Complete { num_results: 0 }
            }))
        );
        assert_matches!(telemetry_receiver.try_next(), Ok(Some(TelemetryEvent::ScanEvent { scan_defects, .. })) => {
            assert!(scan_defects.is_empty());
        });

        // Verify that no defect was logged.
        let logged_defects = get_fake_defects(&mut exec, client);
        let expected_defects = vec![];
        assert_eq!(logged_defects, expected_defects);
    }

    /// Verify that saved networks have their hidden network probabilities updated.
    #[test_case(active_sme_req(vec![], vec![])          ; "active_sme_req, no ssid")]
    #[test_case(active_sme_req(vec![""], vec![])        ; "active_sme_req, wildcard ssid")]
    #[test_case(active_sme_req(vec!["", "foo"], vec![]) ; "active_sme_req, wildcard and foo ssid")]
    #[test_case(active_sme_req(vec!["foo"], vec![])     ; "active_sme_req, foo ssid")]
    #[test_case(passive_sme_req())]
    #[fuchsia::test(add_test_attr = false)]
    fn scan_updates_hidden_network_probabilities(sme_scan_request: fidl_sme::ScanRequest) {
        let mut exec = fasync::TestExecutor::new();
        let (client, mut sme_stream) = exec.run_singlethreaded(create_iface_manager());
        let saved_networks_manager = Arc::new(FakeSavedNetworksManager::new());
        let (telemetry_sender, _telemetry_receiver) = create_telemetry_sender_and_receiver();

        // Create the scan info
        let MockScanData { sme_results: input_aps, internal_results: scan_results } =
            create_scan_ap_data(types::ScanObservation::Unknown);

        // Issue request to scan.
        let scan_fut = perform_scan(
            sme_scan_request.clone().into(),
            client,
            saved_networks_manager.clone(),
            telemetry_sender,
        );
        let mut scan_fut = pin!(scan_fut);

        // Progress scan handler
        assert_matches!(exec.run_until_stalled(&mut scan_fut), Poll::Pending);

        // Create mock scan data and send it via the SME
        assert_matches!(
            exec.run_until_stalled(&mut sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Scan {
                req, responder,
            }))) => {
                assert_eq!(req, sme_scan_request);
                let vmo = write_vmo(input_aps).expect("failed to write VMO");
                responder.send(Ok(vmo)).expect("failed to send scan data");
            }
        );

        // Process response from SME. If an active scan is requested for the unseen network this
        // will be pending, otherwise it will be ready.
        let _ = exec.run_until_stalled(&mut scan_fut);

        // Verify that the scan results were recorded.
        let target_ssids = match sme_scan_request {
            fidl_sme::ScanRequest::Passive(_) => vec![],
            fidl_sme::ScanRequest::Active(ref req) => {
                req.ssids.iter().map(|s| types::Ssid::from_bytes_unchecked(s.to_vec())).collect()
            }
        };
        let mut scan_results_ids: Vec<types::NetworkIdentifierDetailed> = scan_results
            .iter()
            .map(|scan_result| types::NetworkIdentifierDetailed {
                ssid: scan_result.ssid.clone(),
                security_type: scan_result.security_type_detailed,
            })
            .collect();

        let scan_result_record_guard =
            exec.run_singlethreaded(saved_networks_manager.scan_result_records.lock());
        assert_eq!(scan_result_record_guard.len(), 1);
        assert_eq!(scan_result_record_guard[0].0, target_ssids);

        // Get recorded scan result network ids
        let mut recorded_ids = scan_result_record_guard[0]
            .1
            .keys()
            .cloned()
            .collect::<Vec<types::NetworkIdentifierDetailed>>();

        recorded_ids.sort();
        scan_results_ids.sort();
        assert_eq!(scan_results_ids, recorded_ids);

        // Note: the decision to active scan is non-deterministic (using the hidden network probabilities),
        // no need to continue and verify the results in this test case.
    }

    #[fuchsia::test]
    fn bss_to_network_map_duplicated_bss() {
        // Create some input data with duplicated BSSID and Network Identifiers
        let first_result = fidl_sme::ScanResult {
            compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                mutual_security_protocols: vec![fidl_internal::Protocol::Wpa3Personal],
            }),
            timestamp_nanos: zx::MonotonicInstant::get().into_nanos(),
            bss_description: random_fidl_bss_description!(
                Wpa3,
                bssid: [0, 0, 0, 0, 0, 0],
                ssid: types::Ssid::try_from("duplicated ssid").unwrap(),
                rssi_dbm: 0,
                snr_db: 1,
                channel: types::WlanChan::new(1, types::Bandwidth::Cbw20, TwoGhz),
            ),
        };
        let second_result = fidl_sme::ScanResult {
            compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                mutual_security_protocols: vec![fidl_internal::Protocol::Wpa3Personal],
            }),
            timestamp_nanos: zx::MonotonicInstant::get().into_nanos(),
            bss_description: random_fidl_bss_description!(
                Wpa3,
                ssid: types::Ssid::try_from("duplicated ssid").unwrap(),
                bssid: [1, 2, 3, 4, 5, 6],
                rssi_dbm: 101,
                snr_db: 101,
                channel: types::WlanChan::new(101, types::Bandwidth::Cbw40, FiveGhz),
            ),
        };

        let sme_results = [
            first_result.clone(),
            second_result.clone(),
            // same bssid as first_result
            fidl_sme::ScanResult {
                compatibility: fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                    mutual_security_protocols: vec![fidl_internal::Protocol::Wpa3Personal],
                }),
                timestamp_nanos: zx::MonotonicInstant::get().into_nanos(),
                bss_description: random_fidl_bss_description!(
                    Wpa3,
                    bssid: [0, 0, 0, 0, 0, 0],
                    ssid: types::Ssid::try_from("duplicated ssid").unwrap(),
                        rssi_dbm: 13,
                        snr_db: 3,
                        channel: types::WlanChan::new(14, types::Bandwidth::Cbw20, TwoGhz),
                ),
            },
        ];

        let expected_id = types::NetworkIdentifierDetailed {
            ssid: types::Ssid::try_from("duplicated ssid").unwrap(),
            security_type: types::SecurityTypeDetailed::Wpa3Personal,
        };

        // We should only see one entry for the duplicated BSSs in the scan results, and a second
        // entry for the unique bss
        let expected_bss = vec![
            types::Bss {
                bssid: types::Bssid::from([0, 0, 0, 0, 0, 0]),
                signal: types::Signal { rssi_dbm: 0, snr_db: 1 },
                timestamp: zx::MonotonicInstant::from_nanos(first_result.timestamp_nanos),
                channel: types::WlanChan::new(1, types::Bandwidth::Cbw20, TwoGhz),
                observation: types::ScanObservation::Passive,
                compatibility: Compatible::expect_ok([SecurityDescriptor::WPA3_PERSONAL]),
                bss_description: first_result.bss_description.clone().into(),
            },
            types::Bss {
                bssid: types::Bssid::from([1, 2, 3, 4, 5, 6]),
                signal: types::Signal { rssi_dbm: 101, snr_db: 101 },
                timestamp: zx::MonotonicInstant::from_nanos(second_result.timestamp_nanos),
                channel: types::WlanChan::new(101, types::Bandwidth::Cbw40, FiveGhz),
                observation: types::ScanObservation::Passive,
                compatibility: Compatible::expect_ok([SecurityDescriptor::WPA3_PERSONAL]),
                bss_description: second_result.bss_description.clone().into(),
            },
        ];

        let bss_by_network = bss_to_network_map(
            sme_results
                .iter()
                .map(|scan_result| {
                    scan_result.clone().try_into().expect("Failed to convert ScanResult")
                })
                .collect::<Vec<wlan_common::scan::ScanResult>>(),
            &[],
            &mut ScanEventInspectData::new(),
        );
        assert_eq!(bss_by_network.len(), 1);
        assert_eq!(bss_by_network[&expected_id], expected_bss);
    }

    #[test_case(
        fidl_sme::ScanErrorCode::InternalError,
        types::ScanError::GeneralError,
        Defect::Iface(IfaceFailure::FailedScan {iface_id: 0})
    )]
    #[test_case(
        fidl_sme::ScanErrorCode::InternalMlmeError,
        types::ScanError::GeneralError,
        Defect::Iface(IfaceFailure::FailedScan {iface_id: 0})
    )]
    #[test_case(
        fidl_sme::ScanErrorCode::NotSupported,
        types::ScanError::GeneralError,
        Defect::Iface(IfaceFailure::FailedScan {iface_id: 0})
    )]
    #[fuchsia::test(add_test_attr = false)]
    fn scan_error_no_retries(
        sme_failure_mode: fidl_sme::ScanErrorCode,
        policy_failure_mode: types::ScanError,
        expected_defect: Defect,
    ) {
        let mut exec = fasync::TestExecutor::new();
        let (client, mut sme_stream) = exec.run_singlethreaded(create_iface_manager());
        let saved_networks_manager = Arc::new(FakeSavedNetworksManager::new());
        let (telemetry_sender, _telemetry_receiver) = create_telemetry_sender_and_receiver();

        // Issue request to scan.
        let sme_scan = active_sme_req(vec![], vec![1]);
        let scan_fut = perform_scan(
            sme_scan.clone().into(),
            client.clone(),
            saved_networks_manager,
            telemetry_sender,
        );
        let mut scan_fut = pin!(scan_fut);
        assert_matches!(exec.run_until_stalled(&mut scan_fut), Poll::Pending);

        // Send back a failure to the scan request that was generated.
        assert_matches!(
            exec.run_until_stalled(&mut sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Scan { req: _, responder }))) => {
                // Send failed scan response.
                responder.send(Err(sme_failure_mode)).expect("failed to send scan error");
            }
        );

        // The scan future should complete with an error.
        assert_matches!(exec.run_until_stalled(&mut scan_fut), Poll::Ready((request, results)) => {
            assert_eq!(request.sme_req, sme_scan);
            assert_eq!(results, Err(policy_failure_mode));
        });

        // A defect should have been logged on the IfaceManager.
        let logged_defects = get_fake_defects(&mut exec, client);
        let expected_defects = vec![expected_defect];
        assert_eq!(logged_defects, expected_defects);
    }

    #[fuchsia::test]
    fn scan_returns_error_on_timeout() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let (client, _sme_stream) = run_until_completion(&mut exec, create_iface_manager());
        let saved_networks_manager = Arc::new(FakeSavedNetworksManager::new());
        let (telemetry_sender, _telemetry_receiver) = create_telemetry_sender_and_receiver();

        // Issue request to scan.
        let sme_scan = passive_sme_req();
        let scan_fut =
            perform_scan(sme_scan.clone().into(), client, saved_networks_manager, telemetry_sender);
        let mut scan_fut = pin!(scan_fut);

        // Progress scan handler forward so that it will respond to the iterator get next request.
        assert_matches!(exec.run_until_stalled(&mut scan_fut), Poll::Pending);

        // Wake up the next timer, which should be the timeour on the scan request.
        assert!(exec.wake_next_timer().is_some());

        // Check that an error is returned for the scan and there are no location sensor results.
        assert_matches!(exec.run_until_stalled(&mut scan_fut), Poll::Ready((request, results)) => {
            assert_eq!(request.sme_req, sme_scan);
            assert_eq!(results, Err(types::ScanError::GeneralError));
        });
    }

    #[test_case(fidl_sme::ScanErrorCode::NotSupported, ScanIssue::ScanFailure)]
    #[test_case(fidl_sme::ScanErrorCode::InternalError, ScanIssue::ScanFailure)]
    #[test_case(fidl_sme::ScanErrorCode::InternalMlmeError, ScanIssue::ScanFailure)]
    #[test_case(fidl_sme::ScanErrorCode::ShouldWait, ScanIssue::AbortedScan)]
    #[test_case(fidl_sme::ScanErrorCode::CanceledByDriverOrFirmware, ScanIssue::AbortedScan)]
    #[fuchsia::test(add_test_attr = false)]
    fn test_scan_error_metric_conversion(
        scan_error: fidl_sme::ScanErrorCode,
        expected_issue: ScanIssue,
    ) {
        let mut scan_defects = vec![];
        log_metric_for_scan_error(&scan_error, &mut scan_defects);
        assert_eq!(scan_defects, vec![expected_issue]);
    }

    #[test_case(Err(types::ScanError::GeneralError), Some(Defect::Iface(IfaceFailure::FailedScan { iface_id: 0 })))]
    #[test_case(Err(types::ScanError::Cancelled), Some(Defect::Iface(IfaceFailure::CanceledScan { iface_id: 0 })))]
    #[test_case(Ok(vec![]), Some(Defect::Iface(IfaceFailure::EmptyScanResults { iface_id: 0 })))]
    #[test_case(Ok(vec![wlan_common::scan::ScanResult::try_from(
            fidl_sme::ScanResult {
                bss_description: random_fidl_bss_description!(Wpa2, ssid: types::Ssid::try_from("other ssid").unwrap()),
                ..generate_random_sme_scan_result()
            },
        ).expect("failed scan result conversion")]),
        None
    )]
    #[fuchsia::test(add_test_attr = false)]
    fn test_scan_defect_reporting(
        scan_result: Result<Vec<wlan_common::scan::ScanResult>, types::ScanError>,
        expected_defect: Option<Defect>,
    ) {
        let mut exec = fasync::TestExecutor::new();
        let (iface_manager, _) = exec.run_singlethreaded(create_iface_manager());
        let scan_request = passive_sme_req();

        // Get the SME out of the IfaceManager.
        let sme = {
            let cloned_iface_manager = iface_manager.clone();
            let fut = async move {
                let mut iface_manager = cloned_iface_manager.lock().await;
                iface_manager.get_sme_proxy_for_scan().await
            };
            let mut fut = pin!(fut);
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(sme)) => sme)
        };

        // Report the desired scan error or success.
        let fut = report_scan_defects_to_sme(&sme, &scan_result, &scan_request);
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));

        // Based on the expected defect (or lack thereof), ensure that the correct value is obsered
        // on the receiver.
        // Verify that a defect was logged.
        let logged_defects = get_fake_defects(&mut exec, iface_manager);
        match expected_defect {
            Some(defect) => {
                assert_eq!(logged_defects, vec![defect])
            }
            None => assert!(logged_defects.is_empty()),
        }
    }

    #[fuchsia::test]
    fn scanning_loop_handles_sequential_requests() {
        let mut exec = fasync::TestExecutor::new();
        let (iface_mgr, mut sme_stream) = exec.run_singlethreaded(create_iface_manager());
        let saved_networks_manager = Arc::new(FakeSavedNetworksManager::new());
        let (telemetry_sender, _telemetry_receiver) = create_telemetry_sender_and_receiver();
        let (location_sensor, _, _) = MockScanResultConsumer::new();
        let (scan_request_sender, scan_request_receiver) = mpsc::channel(100);
        let scan_requester = Arc::new(ScanRequester { sender: scan_request_sender });
        let scanning_loop = serve_scanning_loop(
            iface_mgr.clone(),
            saved_networks_manager.clone(),
            telemetry_sender,
            location_sensor,
            scan_request_receiver,
        );
        let mut scanning_loop = pin!(scanning_loop);

        // Issue request to scan.
        let first_req_channels = vec![13];
        let scan_req_fut1 = scan_requester.perform_scan(
            ScanReason::BssSelection,
            vec!["foo".try_into().unwrap()],
            vec![generate_channel(13, fidl_fuchsia_wlan_ieee80211::WlanBand::TwoGhz)],
        );
        let mut scan_req_fut1 = pin!(scan_req_fut1);
        assert_matches!(exec.run_until_stalled(&mut scan_req_fut1), Poll::Pending);
        assert_matches!(exec.run_until_stalled(&mut scanning_loop), Poll::Pending);

        // Send back a failure to the scan request that was generated.
        assert_matches!(
            exec.run_until_stalled(&mut sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Scan { req, responder }))) => {
                // Make sure it's the right scan
                assert_matches!(req, fidl_sme::ScanRequest::Active(req) => {
                    assert_eq!(req.channels, first_req_channels)
                });
                // Send failed scan response.
                responder.send(Err(fidl_sme::ScanErrorCode::InternalError)).expect("failed to send scan error");
            }
        );
        assert_matches!(exec.run_until_stalled(&mut scanning_loop), Poll::Pending);

        // The scan request future should complete with an error.
        assert_matches!(exec.run_until_stalled(&mut scan_req_fut1), Poll::Ready(results) => {
            assert_eq!(results, Err(types::ScanError::GeneralError));
        });

        // There should be no other SME requests in the queue
        assert_matches!(exec.run_until_stalled(&mut sme_stream.next()), Poll::Pending);

        // Issue another request to scan.
        let second_req_channels = vec![55];
        let scan_req_fut2 = scan_requester.perform_scan(
            ScanReason::BssSelection,
            vec!["foo".try_into().unwrap()],
            vec![generate_channel(55, fidl_fuchsia_wlan_ieee80211::WlanBand::FiveGhz)],
        );
        let mut scan_req_fut2 = pin!(scan_req_fut2);
        assert_matches!(exec.run_until_stalled(&mut scan_req_fut2), Poll::Pending);
        assert_matches!(exec.run_until_stalled(&mut scanning_loop), Poll::Pending);

        // Send back a failure to the scan request that was generated.
        assert_matches!(
            exec.run_until_stalled(&mut sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Scan { req, responder }))) => {
                // Make sure it's the right scan
                assert_matches!(req, fidl_sme::ScanRequest::Active(req) => {
                    assert_eq!(req.channels, second_req_channels)
                });
                // Send failed scan response.
                responder.send(Err(fidl_sme::ScanErrorCode::InternalError)).expect("failed to send scan error");
            }
        );
        assert_matches!(exec.run_until_stalled(&mut scanning_loop), Poll::Pending);

        // The scan request future should complete with an error.
        assert_matches!(exec.run_until_stalled(&mut scan_req_fut2), Poll::Ready(results) => {
            assert_eq!(results, Err(types::ScanError::GeneralError));
        });

        // There should be no other SME requests in the queue
        assert_matches!(exec.run_until_stalled(&mut sme_stream.next()), Poll::Pending);
    }

    #[fuchsia::test]
    fn scanning_loop_handles_overlapping_requests() {
        let mut exec = fasync::TestExecutor::new();
        let (iface_mgr, mut sme_stream) = exec.run_singlethreaded(create_iface_manager());
        let saved_networks_manager = Arc::new(FakeSavedNetworksManager::new());
        let (telemetry_sender, _telemetry_receiver) = create_telemetry_sender_and_receiver();
        let (location_sensor, _, _) = MockScanResultConsumer::new();
        let (scan_request_sender, scan_request_receiver) = mpsc::channel(100);
        let scan_requester = Arc::new(ScanRequester { sender: scan_request_sender });
        let scanning_loop = serve_scanning_loop(
            iface_mgr.clone(),
            saved_networks_manager.clone(),
            telemetry_sender,
            location_sensor,
            scan_request_receiver,
        );
        let mut scanning_loop = pin!(scanning_loop);

        // Issue request to scan.
        let first_req_channels = vec![13];
        let scan_req_fut1 = scan_requester.perform_scan(
            ScanReason::BssSelection,
            vec!["foo".try_into().unwrap()],
            vec![generate_channel(13, fidl_fuchsia_wlan_ieee80211::WlanBand::TwoGhz)],
        );
        let mut scan_req_fut1 = pin!(scan_req_fut1);
        assert_matches!(exec.run_until_stalled(&mut scan_req_fut1), Poll::Pending);
        assert_matches!(exec.run_until_stalled(&mut scanning_loop), Poll::Pending);

        // Check the scan request was sent to the SME.
        let responder1 = assert_matches!(
            exec.run_until_stalled(&mut sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Scan { req, responder }))) => {
                // Make sure it's the right scan
                assert_matches!(req, fidl_sme::ScanRequest::Active(req) => {
                    assert_eq!(req.channels, first_req_channels)
                });
                responder
            }
        );
        assert_matches!(exec.run_until_stalled(&mut scanning_loop), Poll::Pending);

        // Issue another request to scan.
        let second_req_channels = vec![55];
        let scan_req_fut2 = scan_requester.perform_scan(
            ScanReason::BssSelection,
            vec!["foo".try_into().unwrap()],
            vec![generate_channel(55, fidl_fuchsia_wlan_ieee80211::WlanBand::FiveGhz)],
        );
        let mut scan_req_fut2 = pin!(scan_req_fut2);
        assert_matches!(exec.run_until_stalled(&mut scan_req_fut2), Poll::Pending);
        assert_matches!(exec.run_until_stalled(&mut scanning_loop), Poll::Pending);

        // Both requests are pending
        assert_matches!(exec.run_until_stalled(&mut scan_req_fut1), Poll::Pending);
        assert_matches!(exec.run_until_stalled(&mut scan_req_fut2), Poll::Pending);
        // There should be no other SME requests in the queue
        assert_matches!(exec.run_until_stalled(&mut sme_stream.next()), Poll::Pending);

        // Send back a failed scan response.
        responder1
            .send(Err(fidl_sme::ScanErrorCode::InternalError))
            .expect("failed to send scan error");
        assert_matches!(exec.run_until_stalled(&mut scanning_loop), Poll::Pending);

        // There should immediately be a new SME scan for the second request
        assert_matches!(
            exec.run_until_stalled(&mut sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Scan { req, responder }))) => {
                // Make sure it's the right scan
                assert_matches!(req, fidl_sme::ScanRequest::Active(req) => {
                    assert_eq!(req.channels, second_req_channels)
                });
                // Send failed scan response.
                responder.send(Err(fidl_sme::ScanErrorCode::InternalError)).expect("failed to send scan error");
            }
        );
        assert_matches!(exec.run_until_stalled(&mut scanning_loop), Poll::Pending);

        // Both scan request futures should complete with an error.
        assert_matches!(exec.run_until_stalled(&mut scan_req_fut1), Poll::Ready(results) => {
            assert_eq!(results, Err(types::ScanError::GeneralError));
        });
        assert_matches!(exec.run_until_stalled(&mut scan_req_fut2), Poll::Ready(results) => {
            assert_eq!(results, Err(types::ScanError::GeneralError));
        });

        // There should be no other SME requests in the queue
        assert_matches!(exec.run_until_stalled(&mut sme_stream.next()), Poll::Pending);
    }

    #[fuchsia::test]
    fn scanning_loop_sends_results_to_requester_and_location_sensor() {
        let mut exec = fasync::TestExecutor::new();
        let (iface_mgr, mut sme_stream) = exec.run_singlethreaded(create_iface_manager());
        let saved_networks_manager = Arc::new(FakeSavedNetworksManager::new());
        let (telemetry_sender, _telemetry_receiver) = create_telemetry_sender_and_receiver();
        let (location_sensor, location_sensor_results, _) = MockScanResultConsumer::new();
        let (scan_request_sender, scan_request_receiver) = mpsc::channel(100);
        let scan_requester = Arc::new(ScanRequester { sender: scan_request_sender });
        let scanning_loop = serve_scanning_loop(
            iface_mgr.clone(),
            saved_networks_manager.clone(),
            telemetry_sender,
            location_sensor,
            scan_request_receiver,
        );
        let mut scanning_loop = pin!(scanning_loop);

        // Issue request to scan.
        let scan_req_fut = scan_requester.perform_scan(ScanReason::BssSelection, vec![], vec![]);
        let mut scan_req_fut = pin!(scan_req_fut);
        assert_matches!(exec.run_until_stalled(&mut scan_req_fut), Poll::Pending);
        assert_matches!(exec.run_until_stalled(&mut scanning_loop), Poll::Pending);

        // Send back scan results
        assert_matches!(
            exec.run_until_stalled(&mut sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Scan { req: _, responder }))) => {
                let results = vec![generate_random_sme_scan_result(), generate_random_sme_scan_result()];
                let vmo = write_vmo(results).expect("failed to write VMO");
                responder.send(Ok(vmo)).expect("failed to send scan error");
            }
        );
        assert_matches!(exec.run_until_stalled(&mut scanning_loop), Poll::Pending);

        // The scan request future should complete.
        assert_matches!(exec.run_until_stalled(&mut scan_req_fut), Poll::Ready(Ok(results)) => {
            assert_eq!(results.len(), 2);
        });

        // Check location sensor got results
        assert_matches!(exec.run_until_stalled(&mut scanning_loop), Poll::Pending);
        assert_matches!(
            &*exec.run_singlethreaded(location_sensor_results.lock()),
            Some(results) => {
                assert_eq!(results.len(), 2);
            }
        );
    }

    #[fuchsia::test]
    fn scanning_loop_location_sensor_timeout_works() {
        let mut exec = fasync::TestExecutor::new();
        let (iface_mgr, mut sme_stream) = exec.run_singlethreaded(create_iface_manager());
        let saved_networks_manager = Arc::new(FakeSavedNetworksManager::new());
        let (telemetry_sender, _telemetry_receiver) = create_telemetry_sender_and_receiver();
        let (location_sensor, location_sensor_results, location_sensor_stalled) =
            MockScanResultConsumer::new();
        let (scan_request_sender, scan_request_receiver) = mpsc::channel(100);
        let scan_requester = Arc::new(ScanRequester { sender: scan_request_sender });
        let scanning_loop = serve_scanning_loop(
            iface_mgr.clone(),
            saved_networks_manager.clone(),
            telemetry_sender,
            location_sensor,
            scan_request_receiver,
        );
        let mut scanning_loop = pin!(scanning_loop);

        // Make location sensor stalled
        *(exec.run_singlethreaded(location_sensor_stalled.lock())) = true;

        // Issue request to scan.
        let scan_req_fut = scan_requester.perform_scan(ScanReason::BssSelection, vec![], vec![]);
        let mut scan_req_fut = pin!(scan_req_fut);
        assert_matches!(exec.run_until_stalled(&mut scan_req_fut), Poll::Pending);
        assert_matches!(exec.run_until_stalled(&mut scanning_loop), Poll::Pending);

        // Send back scan results
        assert_matches!(
            exec.run_until_stalled(&mut sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Scan { req: _, responder }))) => {
                let results = vec![generate_random_sme_scan_result(), generate_random_sme_scan_result()];
                let vmo = write_vmo(results).expect("failed to write VMO");
                responder.send(Ok(vmo)).expect("failed to send scan error");
            }
        );
        assert_matches!(exec.run_until_stalled(&mut scanning_loop), Poll::Pending);

        // Check location sensor didn't get any results
        assert_matches!(exec.run_until_stalled(&mut scanning_loop), Poll::Pending);
        assert_matches!(&*exec.run_singlethreaded(location_sensor_results.lock()), None);

        // Make location sensor *not* stalled
        *(exec.run_singlethreaded(location_sensor_stalled.lock())) = false;

        // Issue another request to scan.
        let scan_req_fut = scan_requester.perform_scan(ScanReason::BssSelection, vec![], vec![]);
        let mut scan_req_fut = pin!(scan_req_fut);
        assert_matches!(exec.run_until_stalled(&mut scan_req_fut), Poll::Pending);
        assert_matches!(exec.run_until_stalled(&mut scanning_loop), Poll::Pending);

        // Send back scan results
        assert_matches!(
            exec.run_until_stalled(&mut sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Scan { req: _, responder }))) => {
                let results = vec![generate_random_sme_scan_result(), generate_random_sme_scan_result()];
                let vmo = write_vmo(results).expect("failed to write VMO");
                responder.send(Ok(vmo)).expect("failed to send scan error");
            }
        );
        assert_matches!(exec.run_until_stalled(&mut scanning_loop), Poll::Pending);

        // Check location sensor got results
        assert_matches!(exec.run_until_stalled(&mut scanning_loop), Poll::Pending);
        assert_matches!(
            &*exec.run_singlethreaded(location_sensor_results.lock()),
            Some(results) => {
                assert_eq!(results.len(), 2);
            }
        );
    }

    #[fuchsia::test]
    fn scanning_loops_sends_inspect_data_to_telemetry() {
        let mut exec = fasync::TestExecutor::new();
        let (iface_mgr, mut sme_stream) = exec.run_singlethreaded(create_iface_manager());
        let saved_networks_manager = Arc::new(FakeSavedNetworksManager::new());
        let (telemetry_sender, mut telemetry_receiver) = create_telemetry_sender_and_receiver();
        let (location_sensor, _, _) = MockScanResultConsumer::new();
        let (scan_request_sender, scan_request_receiver) = mpsc::channel(100);
        let scan_requester = Arc::new(ScanRequester { sender: scan_request_sender });
        let scanning_loop = serve_scanning_loop(
            iface_mgr.clone(),
            saved_networks_manager.clone(),
            telemetry_sender,
            location_sensor,
            scan_request_receiver,
        );
        let mut scanning_loop = pin!(scanning_loop);

        // Issue request to scan
        let scan_req_fut = scan_requester.perform_scan(ScanReason::BssSelection, vec![], vec![]);
        let mut scan_req_fut = pin!(scan_req_fut);
        assert_matches!(exec.run_until_stalled(&mut scan_req_fut), Poll::Pending);
        assert_matches!(exec.run_until_stalled(&mut scanning_loop), Poll::Pending);

        // Prepare scan results with unknown protection IEs
        let bss_description = fake_bss_description!(Wpa2, ies_overrides: IesOverrides::new().set(IeType::RSNE, fake_unknown_rsne()[2..].to_vec()));
        let scan_result = fidl_sme::ScanResult {
            bss_description: bss_description.into(),
            ..generate_random_sme_scan_result()
        };

        // Send back scan results
        assert_matches!(
            exec.run_until_stalled(&mut sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Scan {
                req, responder,
            }))) => {
                assert_eq!(req, passive_sme_req());
                let vmo = write_vmo(vec![scan_result.clone()]).expect("failed to write VMO");
                responder.send(Ok(vmo)).expect("failed to send scan data");
            }
        );

        // Process scan handler
        assert_matches!(exec.run_until_stalled(&mut scanning_loop), Poll::Pending);

        // The scan request future should complete.
        assert_matches!(exec.run_until_stalled(&mut scan_req_fut), Poll::Ready(Ok(results)) => {
            assert_eq!(results.len(), 1);
        });

        // Verify inspect data was sent to telemetry module.
        let readable_ie: String =
            scan_result.bss_description.ies.iter().map(|n| n.to_string()).join(",");
        assert_matches!(telemetry_receiver.try_next(), Ok(Some(TelemetryEvent::SmeScanStart)));
        assert_matches!(
            telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::SmeScanResult {
                result: wlan_telemetry::ScanResult::Complete { num_results: 1 }
            }))
        );
        assert_matches!(
            telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::ScanEvent {inspect_data, scan_defects})) => {
                assert_eq!(scan_defects, vec![]);
                assert_eq!(inspect_data.unknown_protection_ies, vec![readable_ie]);
        });
    }

    #[test_case(fidl_sme::ScanErrorCode::ShouldWait, false; "Scan error ShouldWait with failed retry")]
    #[test_case(fidl_sme::ScanErrorCode::ShouldWait, true; "Scan error ShouldWait with successful retry")]
    #[test_case(fidl_sme::ScanErrorCode::CanceledByDriverOrFirmware, true; "Scan error CanceledByDriverOrFirmware with successful retry")]
    #[fuchsia::test]
    fn scanning_loop_retries_cancelled_request_once(
        error_code: fidl_sme::ScanErrorCode,
        retry_succeeds: bool,
    ) {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let (iface_mgr, mut sme_stream) = run_until_completion(&mut exec, create_iface_manager());
        let saved_networks_manager = Arc::new(FakeSavedNetworksManager::new());
        let (telemetry_sender, _telemetry_receiver) = create_telemetry_sender_and_receiver();
        let (location_sensor, _, _) = MockScanResultConsumer::new();
        let (scan_request_sender, scan_request_receiver) = mpsc::channel(100);
        let scan_requester = Arc::new(ScanRequester { sender: scan_request_sender });
        let scanning_loop = serve_scanning_loop(
            iface_mgr.clone(),
            saved_networks_manager.clone(),
            telemetry_sender,
            location_sensor,
            scan_request_receiver,
        );
        let mut scanning_loop = pin!(scanning_loop);

        // Issue request to scan.
        let req_channels = vec![13];
        let scan_req_fut = scan_requester.perform_scan(
            ScanReason::BssSelection,
            vec!["foo".try_into().unwrap()],
            vec![generate_channel(13, fidl_fuchsia_wlan_ieee80211::WlanBand::TwoGhz)],
        );
        let mut scan_req_fut = pin!(scan_req_fut);
        assert_matches!(exec.run_until_stalled(&mut scan_req_fut), Poll::Pending);
        assert_matches!(exec.run_until_stalled(&mut scanning_loop), Poll::Pending);

        // Check that the scan request was sent to the SME.
        let responder = assert_matches!(
            exec.run_until_stalled(&mut sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Scan { req, responder }))) => {
                // Make sure it's the right scan
                assert_matches!(req, fidl_sme::ScanRequest::Active(req) => {
                    assert_eq!(req.channels, req_channels)
                });
                responder
            }
        );
        assert_matches!(exec.run_until_stalled(&mut scanning_loop), Poll::Pending);

        // Send back a error from SME for the request.
        responder.send(Err(error_code)).expect("failed to send scan error");
        assert_matches!(exec.run_until_stalled(&mut scanning_loop), Poll::Pending);

        // Request should return still be pending, awaiting retry.
        assert_matches!(exec.run_until_stalled(&mut scan_req_fut), Poll::Pending);

        // There should be no new SME requests yet.
        assert_matches!(exec.run_until_stalled(&mut sme_stream.next()), Poll::Pending);

        // Wake up the back off timer and advance the scan request future.
        assert!(exec.wake_next_timer().is_some());
        assert_matches!(exec.run_until_stalled(&mut scanning_loop), Poll::Pending);

        // Verify the retry scan request was sent to SME.
        let responder = assert_matches!(
            exec.run_until_stalled(&mut sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Scan { req, responder }))) => {
                assert_matches!(req, fidl_sme::ScanRequest::Active(req) => {
                    assert_eq!(req.channels, req_channels)
                });
                responder
            }
        );

        if retry_succeeds {
            // Create mock scan data and send it via the SME. Although it's an active scan, the
            // scan doesn't target any of these SSIDs, so results should be ScanObservation::Passive
            let MockScanData { sme_results: input_aps, internal_results: _internal_aps } =
                create_scan_ap_data(types::ScanObservation::Passive);
            let vmo = write_vmo(input_aps).expect("failed to write VMO");
            responder.send(Ok(vmo)).expect("failed to send scan data");
            assert_matches!(exec.run_until_stalled(&mut scanning_loop), Poll::Pending);

            // Verify one defect was logged.
            let logged_defects = get_fake_defects(&mut exec, iface_mgr);
            let expected_defects = vec![Defect::Iface(IfaceFailure::CanceledScan { iface_id: 0 })];
            assert_eq!(logged_defects, expected_defects);
        } else {
            // Send back a error from SME.
            responder.send(Err(error_code)).expect("failed to send scan error");
            assert_matches!(exec.run_until_stalled(&mut scanning_loop), Poll::Pending);

            // Verify that both defects were logged.
            let logged_defects = get_fake_defects(&mut exec, iface_mgr);
            let expected_defects = vec![
                Defect::Iface(IfaceFailure::CanceledScan { iface_id: 0 }),
                Defect::Iface(IfaceFailure::CanceledScan { iface_id: 0 }),
            ];
            assert_eq!(logged_defects, expected_defects);
        }
        // Request should get a response now.
        assert_matches!(exec.run_until_stalled(&mut scan_req_fut), Poll::Ready(_));

        // There should be no new SME requests.
        assert_matches!(exec.run_until_stalled(&mut sme_stream.next()), Poll::Pending);
    }

    #[fuchsia::test]
    fn scanning_loop_backs_off_after_cancelled_request() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let (iface_mgr, mut sme_stream) = run_until_completion(&mut exec, create_iface_manager());
        let saved_networks_manager = Arc::new(FakeSavedNetworksManager::new());
        let (telemetry_sender, _telemetry_receiver) = create_telemetry_sender_and_receiver();
        let (location_sensor, _, _) = MockScanResultConsumer::new();
        let (scan_request_sender, scan_request_receiver) = mpsc::channel(100);
        let scan_requester = Arc::new(ScanRequester { sender: scan_request_sender });
        let scanning_loop = serve_scanning_loop(
            iface_mgr.clone(),
            saved_networks_manager.clone(),
            telemetry_sender,
            location_sensor,
            scan_request_receiver,
        );
        let mut scanning_loop = pin!(scanning_loop);

        // Issue first request to scan.
        let first_req_channels = vec![13];
        let scan_req_fut1 = scan_requester.perform_scan(
            ScanReason::BssSelection,
            vec!["foo".try_into().unwrap()],
            vec![generate_channel(13, fidl_fuchsia_wlan_ieee80211::WlanBand::TwoGhz)],
        );
        let mut scan_req_fut1 = pin!(scan_req_fut1);
        assert_matches!(exec.run_until_stalled(&mut scan_req_fut1), Poll::Pending);
        assert_matches!(exec.run_until_stalled(&mut scanning_loop), Poll::Pending);

        // Check the first scan request was sent to the SME.
        let responder = assert_matches!(
            exec.run_until_stalled(&mut sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Scan { req, responder }))) => {
                // Make sure it's the right scan
                assert_matches!(req, fidl_sme::ScanRequest::Active(req) => {
                    assert_eq!(req.channels, first_req_channels)
                });
                responder
            }
        );

        // Issue a second request to scan.
        let second_req_channels = vec![55];
        let scan_req_fut2 = scan_requester.perform_scan(
            ScanReason::BssSelection,
            vec!["foo".try_into().unwrap()],
            vec![generate_channel(55, fidl_fuchsia_wlan_ieee80211::WlanBand::FiveGhz)],
        );
        let mut scan_req_fut2 = pin!(scan_req_fut2);
        assert_matches!(exec.run_until_stalled(&mut scan_req_fut2), Poll::Pending);
        assert_matches!(exec.run_until_stalled(&mut scanning_loop), Poll::Pending);

        // There should be no new SME requests in the queue.
        assert_matches!(exec.run_until_stalled(&mut sme_stream.next()), Poll::Pending);

        // Send back a ShouldWait error from SME for the first request.
        responder
            .send(Err(fidl_sme::ScanErrorCode::ShouldWait))
            .expect("failed to send scan error");
        assert_matches!(exec.run_until_stalled(&mut scanning_loop), Poll::Pending);

        // First request still be pending, as it awaits a retry.
        assert_matches!(exec.run_until_stalled(&mut scan_req_fut1), Poll::Pending);

        // There should be no new SME requests, since the first request should be backing off
        // before issuing a retry.
        assert_matches!(exec.run_until_stalled(&mut sme_stream.next()), Poll::Pending);

        // Wake up the back off timer and advance the scan request future.
        assert!(exec.wake_next_timer().is_some());
        assert_matches!(exec.run_until_stalled(&mut scanning_loop), Poll::Pending);

        // Verify the retry scan request was sent to SME.
        let responder = assert_matches!(
            exec.run_until_stalled(&mut sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Scan { req, responder }))) => {
                // Make sure it's the right scan
                assert_matches!(req, fidl_sme::ScanRequest::Active(req) => {
                    assert_eq!(req.channels, first_req_channels)
                });
                responder
            }
        );

        // Send back another ShouldWait error from SME for the first request retry.
        responder
            .send(Err(fidl_sme::ScanErrorCode::ShouldWait))
            .expect("failed to send scan error");
        assert_matches!(exec.run_until_stalled(&mut scanning_loop), Poll::Pending);

        // The first scan req future should now be Ready, returning a Cancelled error.
        assert_matches!(exec.run_until_stalled(&mut scan_req_fut1), Poll::Ready(results) => {
            assert_eq!(results, Err(types::ScanError::Cancelled));
        });

        // There should be no SME requests in the queue, because the scan loop should be backing
        // off before serviving the next scan request.
        assert_matches!(exec.run_until_stalled(&mut sme_stream.next()), Poll::Pending);

        // Wake up the back off timer.
        assert!(exec.wake_next_timer().is_some());
        assert_matches!(exec.run_until_stalled(&mut scanning_loop), Poll::Pending);

        // There should now be an SME scan request for the second scan req.
        assert_matches!(
            exec.run_until_stalled(&mut sme_stream.next()),
            Poll::Ready(Some(Ok(fidl_sme::ClientSmeRequest::Scan { req, .. }))) => {
                // Make sure it's the right scan
                assert_matches!(req, fidl_sme::ScanRequest::Active(req) => {
                    assert_eq!(req.channels, second_req_channels)
                });
            }
        );
    }
}
