// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::client::inspect;
use crate::responder::Responder;
use crate::{Error, MlmeRequest, MlmeSink};
use fidl_fuchsia_wlan_common as fidl_common;
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_mlme as fidl_mlme;
use fidl_fuchsia_wlan_sme as fidl_sme;
use fuchsia_inspect::NumericProperty;
use futures::channel::mpsc;
use ieee80211::{Bssid, Ssid};
use log::warn;
use std::collections::{HashMap, HashSet, hash_map};
use std::mem;
use std::sync::{Arc, LazyLock};
use wlan_common::bss::BssDescription;
use wlan_common::channel::{Bandwidth, Channel};
use wlan_common::ie::IesMerger;

type ScanTxnId = u64;

const PASSIVE_SCAN_CHANNEL_MS: u32 = 200;
const ACTIVE_SCAN_PROBE_DELAY_MS: u32 = 5;
const ACTIVE_SCAN_CHANNEL_MS: u32 = 75;

// A "user"-initiated scan request for the purpose of discovering available networks
#[derive(Debug, PartialEq)]
pub struct DiscoveryScan<T> {
    tokens: Vec<T>,
    scan_request: fidl_sme::ScanRequest,
}

impl<T> DiscoveryScan<T> {
    pub fn new(token: T, scan_request: fidl_sme::ScanRequest) -> Self {
        Self { tokens: vec![token], scan_request }
    }

    pub fn matches(&self, scan: &DiscoveryScan<T>) -> bool {
        self.scan_request == scan.scan_request
    }

    pub fn merges(&mut self, mut scan: DiscoveryScan<T>) {
        self.tokens.append(&mut scan.tokens)
    }
}
/// Client end of a scheduled scan session.
pub struct ScheduledScanReceiver {
    scan_results_receiver: mpsc::UnboundedReceiver<fidl::Vmo>,
    pub(crate) txn_id: ScanTxnId,
    mlme_sink: MlmeSink,
    stopped_by_firmware: bool,
}
impl ScheduledScanReceiver {
    fn new(
        scan_results_receiver: mpsc::UnboundedReceiver<fidl::Vmo>,
        txn_id: ScanTxnId,
        mlme_sink: MlmeSink,
    ) -> Self {
        Self { scan_results_receiver, txn_id, mlme_sink, stopped_by_firmware: false }
    }
}
impl futures::stream::Stream for ScheduledScanReceiver {
    type Item = fidl::Vmo;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let poll_result = std::pin::Pin::new(&mut self.scan_results_receiver).poll_next(cx);
        if let std::task::Poll::Ready(None) = poll_result {
            self.stopped_by_firmware = true;
        }
        poll_result
    }
}
impl Drop for ScheduledScanReceiver {
    fn drop(&mut self) {
        // If the firmware already stopped the scheduled scan, we do not need to send a stop
        // command. Sending it anyway could result in MLME returning ZX_ERR_NOT_FOUND and
        // logging errors.
        if !self.stopped_by_firmware {
            let mlme_req = fidl_mlme::MlmeStopScheduledScanRequest { txn_id: self.txn_id };
            let (responder, _) = Responder::new();
            self.mlme_sink.send(MlmeRequest::StopScheduledScan(mlme_req, responder));
        }
    }
}

/// Represents the internal state of an active scheduled scan. Used to accumulate streamed scheduled
/// scan results and send them via VMO when ready.
pub(crate) struct ScheduledScanState {
    scan_results_sender: mpsc::UnboundedSender<fidl::Vmo>,
    bss_map: std::collections::HashMap<
        Bssid,
        (fidl_ieee80211::BssDescription, wlan_common::ie::IesMerger),
    >,
}
impl ScheduledScanState {
    fn new(scan_results_sender: mpsc::UnboundedSender<fidl::Vmo>) -> Self {
        Self { scan_results_sender, bss_map: HashMap::new() }
    }
}
pub struct ScanScheduler<T> {
    // The currently running scan. We assume that MLME can handle a single concurrent scan
    // regardless of its own state.
    current: ScanState<T>,
    // Pending discovery requests from the user
    pending_discovery: Vec<DiscoveryScan<T>>,
    device_info: Arc<fidl_mlme::DeviceInfo>,
    spectrum_management_support: fidl_common::SpectrumManagementSupport,
    // Map of active scheduled scan transaction IDs to their internal states.
    pub(crate) scheduled_scan_receivers: HashMap<ScanTxnId, ScheduledScanState>,
    last_mlme_txn_id: ScanTxnId,
}

#[derive(Debug)]
enum ScanState<T> {
    NotScanning,
    ScanningToDiscover {
        cmd: DiscoveryScan<T>,
        mlme_txn_id: ScanTxnId,
        bss_map: HashMap<Bssid, (fidl_ieee80211::BssDescription, IesMerger)>,
    },
}

#[derive(Debug)]
pub struct ScanEnd<T> {
    pub tokens: Vec<T>,
    pub result_code: fidl_mlme::ScanResultCode,
    pub bss_description_list: Vec<BssDescription>,
}

impl<T> ScanScheduler<T> {
    pub fn new(
        device_info: Arc<fidl_mlme::DeviceInfo>,
        spectrum_management_support: fidl_common::SpectrumManagementSupport,
    ) -> Self {
        ScanScheduler {
            current: ScanState::NotScanning,
            pending_discovery: Vec::new(),
            device_info,
            spectrum_management_support,
            scheduled_scan_receivers: HashMap::new(),
            last_mlme_txn_id: 0,
        }
    }

    // Initiate a "discovery" scan. The scan might or might not begin immediately.
    // The request can be merged with any pending or ongoing requests.
    // If a ScanRequest is returned, the caller is responsible for forwarding it to MLME.
    pub fn enqueue_scan_to_discover(
        &mut self,
        s: DiscoveryScan<T>,
    ) -> Option<fidl_mlme::ScanRequest> {
        if let ScanState::ScanningToDiscover { cmd, .. } = &mut self.current
            && cmd.matches(&s)
        {
            cmd.merges(s);
            return None;
        }
        if let Some(scan_cmd) = self.pending_discovery.iter_mut().find(|cmd| cmd.matches(&s)) {
            scan_cmd.merges(s);
            return None;
        }
        self.pending_discovery.push(s);
        self.start_next_scan()
    }

    // Returns a unique transaction ID for the next MLME transaction.
    fn get_next_mlme_txn_id(&mut self) -> ScanTxnId {
        self.last_mlme_txn_id += 1;
        self.last_mlme_txn_id
    }

    pub(crate) fn start_scheduled_scan(
        &mut self,
        req: fidl_common::ScheduledScanRequest,
        mlme_sink: MlmeSink,
        responder: Responder<Result<(), i32>>,
    ) -> ScheduledScanReceiver {
        // Send start request to MLME with a new transaction ID
        let txn_id = self.get_next_mlme_txn_id();
        let mlme_req = fidl_mlme::MlmeStartScheduledScanRequest { txn_id, req };
        mlme_sink.send(MlmeRequest::StartScheduledScan(mlme_req, responder));

        // Create a channel to process scan results streamed from MLME
        let (sender, receiver) = mpsc::unbounded();
        let _ = self.scheduled_scan_receivers.insert(txn_id, ScheduledScanState::new(sender));
        ScheduledScanReceiver::new(receiver, txn_id, mlme_sink)
    }

    // Should be called for every OnScanResult event received from MLME.
    pub fn on_mlme_scan_result(&mut self, msg: fidl_mlme::ScanResult) -> Result<(), Error> {
        // First check if this belongs to a scheduled scan session.
        if let Some(session) = self.scheduled_scan_receivers.get_mut(&msg.txn_id) {
            maybe_insert_bss(&mut session.bss_map, msg.bss);
            return Ok(());
        }

        match &mut self.current {
            ScanState::NotScanning => Err(Error::ScanResultNotScanning),
            ScanState::ScanningToDiscover { mlme_txn_id, .. } if *mlme_txn_id != msg.txn_id => {
                Err(Error::ScanResultWrongTxnId)
            }
            ScanState::ScanningToDiscover { bss_map, .. } => {
                maybe_insert_bss(bss_map, msg.bss);
                Ok(())
            }
        }
    }

    pub(crate) fn on_scheduled_scan_matches_available(
        &mut self,
        txn_id: ScanTxnId,
        sme_inspect: &Arc<inspect::SmeTree>,
        cfg: &crate::client::ClientConfig,
        device_info: &fidl_mlme::DeviceInfo,
        security_support: &fidl_common::SecuritySupport,
    ) {
        if let Some(session) = self.scheduled_scan_receivers.get_mut(&txn_id) {
            let bss_map = std::mem::take(&mut session.bss_map);
            let bss_description_list = convert_bss_map(bss_map, None::<Ssid>, sme_inspect);
            let results_fidl = bss_description_list
                .into_iter()
                .map(|bss_description| {
                    cfg.create_scan_result(
                        // TODO(https://fxbug.dev/42164608): ScanEnd drops the timestamp from MLME
                        zx::MonotonicInstant::from_nanos(0),
                        bss_description,
                        device_info,
                        security_support,
                    )
                })
                .map(Into::into)
                .collect::<Vec<_>>();

            match wlan_common::scan::write_vmo(results_fidl) {
                Ok(vmo) => {
                    let _ = session.scan_results_sender.unbounded_send(vmo);
                }
                Err(e) => {
                    log::error!("Failed to write VMO for sched scan results: {:?}", e);
                }
            }
        }
    }

    pub(crate) fn on_scheduled_scan_stopped_by_firmware(&mut self, txn_id: ScanTxnId) {
        let _ = self.scheduled_scan_receivers.remove(&txn_id);
    }

    // Should be called for every OnScanEnd event received from MLME.
    // If a ScanRequest is returned, the caller is responsible for forwarding it to MLME.
    pub fn on_mlme_scan_end(
        &mut self,
        msg: fidl_mlme::ScanEnd,
        sme_inspect: &Arc<inspect::SmeTree>,
    ) -> Result<(ScanEnd<T>, Option<fidl_mlme::ScanRequest>), Error> {
        match mem::replace(&mut self.current, ScanState::NotScanning) {
            ScanState::NotScanning => Err(Error::ScanEndNotScanning),
            ScanState::ScanningToDiscover { mlme_txn_id, .. } if mlme_txn_id != msg.txn_id => {
                Err(Error::ScanEndWrongTxnId)
            }
            ScanState::ScanningToDiscover { cmd, bss_map, .. } => {
                let scan_end = ScanEnd {
                    tokens: cmd.tokens,
                    result_code: msg.code,
                    bss_description_list: convert_bss_map(bss_map, None::<Ssid>, sme_inspect),
                };

                let request = self.start_next_scan();
                Ok((scan_end, request))
            }
        }
    }

    fn start_next_scan(&mut self) -> Option<fidl_mlme::ScanRequest> {
        let has_pending = !self.pending_discovery.is_empty();
        (matches!(self.current, ScanState::NotScanning) && has_pending).then(|| {
            let txn_id = self.get_next_mlme_txn_id();
            let scan_cmd = self.pending_discovery.remove(0);
            let request = new_discovery_scan_request(
                txn_id,
                &scan_cmd,
                &self.device_info,
                self.spectrum_management_support.clone(),
            );
            self.current = ScanState::ScanningToDiscover {
                cmd: scan_cmd,
                mlme_txn_id: txn_id,
                bss_map: HashMap::new(),
            };
            request
        })
    }
}

fn maybe_insert_bss(
    bss_map: &mut HashMap<Bssid, (fidl_ieee80211::BssDescription, IesMerger)>,
    mut fidl_bss: fidl_ieee80211::BssDescription,
) {
    let mut ies = vec![];
    std::mem::swap(&mut ies, &mut fidl_bss.ies);

    match bss_map.entry(Bssid::from(fidl_bss.bssid)) {
        hash_map::Entry::Occupied(mut entry) => {
            let (existing_bss, ies_merger) = entry.get_mut();

            if (fidl_bss.primary != existing_bss.primary)
                && (fidl_bss.rssi_dbm < existing_bss.rssi_dbm)
            {
                // Assume `fidl_bss` is from an "echo" Beacon frame from the same BSSID
                return;
            }

            ies_merger.merge(&ies[..]);
            if ies_merger.buffer_overflow() {
                warn!(
                    "Not merging some IEs due to running out of buffer. BSSID: {}",
                    Bssid::from(fidl_bss.bssid)
                );
            }
            *existing_bss = fidl_bss;
        }
        hash_map::Entry::Vacant(entry) => {
            let _ = entry.insert((fidl_bss, IesMerger::new(ies)));
        }
    }
}

fn convert_bss_map(
    bss_map: HashMap<Bssid, (fidl_ieee80211::BssDescription, IesMerger)>,
    ssid_selector: Option<Ssid>,
    sme_inspect: &Arc<inspect::SmeTree>,
) -> Vec<BssDescription> {
    let bss_description_list =
        bss_map.into_iter().filter_map(|(_bssid, (mut bss, mut ies_merger))| {
            let _ = sme_inspect.scan_merge_ie_failures.add(ies_merger.merge_ie_failures() as u64);

            let mut ies = ies_merger.finalize();
            std::mem::swap(&mut ies, &mut bss.ies);
            let bss: Option<BssDescription> = bss.try_into().ok();
            if bss.is_none() {
                let _ = sme_inspect.scan_discard_fidl_bss.add(1);
            }
            bss
        });

    match ssid_selector {
        None => bss_description_list.collect(),
        Some(ssid) => bss_description_list.filter(|v| v.ssid == ssid).collect(),
    }
}

fn new_scan_request(
    mlme_txn_id: ScanTxnId,
    scan_request: fidl_sme::ScanRequest,
    ssid_list: Vec<Ssid>,
    device_info: &fidl_mlme::DeviceInfo,
    spectrum_management_support: fidl_common::SpectrumManagementSupport,
) -> fidl_mlme::ScanRequest {
    let scan_req = fidl_mlme::ScanRequest {
        txn_id: mlme_txn_id,
        scan_type: fidl_mlme::ScanTypes::Passive,
        probe_delay: 0,
        // TODO(https://fxbug.dev/42169913): SME silently ignores unsupported channels
        channel_list: get_primary_channels_for_scan(
            device_info,
            spectrum_management_support,
            &scan_request,
        ),
        ssid_list: ssid_list.into_iter().map(Ssid::into).collect(),
        min_channel_time: PASSIVE_SCAN_CHANNEL_MS,
        max_channel_time: PASSIVE_SCAN_CHANNEL_MS,
    };
    match scan_request {
        fidl_sme::ScanRequest::Active(active_scan_params) => fidl_mlme::ScanRequest {
            scan_type: fidl_mlme::ScanTypes::Active,
            ssid_list: active_scan_params.ssids,
            probe_delay: ACTIVE_SCAN_PROBE_DELAY_MS,
            min_channel_time: ACTIVE_SCAN_CHANNEL_MS,
            max_channel_time: ACTIVE_SCAN_CHANNEL_MS,
            ..scan_req
        },
        fidl_sme::ScanRequest::Passive(_) => scan_req,
    }
}

fn new_discovery_scan_request<T>(
    mlme_txn_id: ScanTxnId,
    discovery_scan: &DiscoveryScan<T>,
    device_info: &fidl_mlme::DeviceInfo,
    spectrum_management_support: fidl_common::SpectrumManagementSupport,
) -> fidl_mlme::ScanRequest {
    new_scan_request(
        mlme_txn_id,
        discovery_scan.scan_request.clone(),
        vec![],
        device_info,
        spectrum_management_support,
    )
}

/// Returns channels at the intersection of
///
///   - CANDIDATE_PRIMARY_CHANNELS
///   - This device's primary channels.
///   - The requested channels (for an active scan only).
///
/// When a device does not support DFS, 5 GHz channels are excluded for active scans.
/// Every 5 GHz channel requires DFS support in at least one regulatory domain, or is otherwise
/// not allowed in some regulatory domain. This function cautiously excludes 5 GHz channels
/// for active scans on those devices to ensure accordance with each the regulatory domain's DFS
/// requirements. The wlan-sme library is the common component in every WLAN interface and
/// is therefore a sensible place for this filter.
///
/// TODO(https://fxbug.dev/42144530): Known quirks about this implementation.
fn get_primary_channels_for_scan(
    device_info: &fidl_mlme::DeviceInfo,
    spectrum_management_support: fidl_common::SpectrumManagementSupport,
    scan_request: &fidl_sme::ScanRequest,
) -> Vec<fidl_ieee80211::ChannelNumber> {
    let mut primary_channels: HashSet<u8> = HashSet::new();
    for band in &device_info.bands {
        primary_channels.extend(band.primary_channels.iter().map(|c| c.number));
    }

    let requested_channels = match scan_request {
        fidl_sme::ScanRequest::Active(options) => &options.channels[..],
        fidl_sme::ScanRequest::Passive(options) => &options.channels[..],
    };
    let channels: Vec<fidl_ieee80211::ChannelNumber> = CANDIDATE_PRIMARY_CHANNELS
        .iter()
        .filter(|channel| primary_channels.contains(&channel.primary))
        .filter(|channel| {
            // Avoid active scans on 5 GHz channels on a non-DFS device. There is no 5 GHz
            // channel that is valid in all regulatory domains.
            if let &fidl_sme::ScanRequest::Passive(_) = scan_request {
                return true;
            };
            if channel.band == fidl_ieee80211::WlanBand::FiveGhz {
                return spectrum_management_support
                    .dfs
                    .as_ref()
                    .and_then(|dfs| dfs.supported)
                    .unwrap_or(false);
            };
            true
        })
        .filter(|channel| {
            // If there are any channels specified by the caller, only include those channels.
            if !requested_channels.is_empty() {
                return requested_channels.contains(&channel.primary);
            }
            true
        })
        .copied()
        .map(|channel| channel.into())
        .collect();

    if channels.is_empty() {
        if !requested_channels.is_empty() {
            warn!("All channels are filtered out. Requested channels: {:?}", requested_channels);
        } else {
            warn!("All channels are filtered out.");
        };
    }

    channels
}

// The following constructs the Channel list at runtime once and leaks its contents
// as a static reference. Firmware will reject channels if they are not allowed by
// the current regulatory region.
static CANDIDATE_PRIMARY_CHANNELS: LazyLock<&'static [Channel]> = LazyLock::new(|| {
    let channels_two_ghz = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14];
    let channels_five_ghz = vec![
        36, 40, 44, 48, 52, 56, 60, 64, 100, 104, 108, 112, 116, 120, 124, 128, 132, 136, 140, 144,
        149, 153, 157, 161, 165,
    ];

    let mut channels_to_scan = Vec::new();
    for channel in channels_two_ghz {
        channels_to_scan.push(Channel::new(
            channel,
            Bandwidth::Cbw20,
            fidl_ieee80211::WlanBand::TwoGhz,
        ));
    }
    for channel in channels_five_ghz {
        channels_to_scan.push(Channel::new(
            channel,
            Bandwidth::Cbw20,
            fidl_ieee80211::WlanBand::FiveGhz,
        ));
    }

    channels_to_scan.leak()
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils;
    use assert_matches::assert_matches;
    use fidl_ieee80211::WlanBand::{FiveGhz, TwoGhz};
    use fuchsia_inspect::Inspector;

    use ieee80211::MacAddr;
    use regex::bytes::Regex;
    use std::fmt::Write;
    use std::sync::LazyLock;
    use test_case::test_case;
    use wlan_common::test_utils::fake_capabilities::fake_5ghz_band_capability;
    use wlan_common::test_utils::fake_features::fake_spectrum_management_support_empty;
    use wlan_common::{fake_bss_description, fake_fidl_bss_description};

    static CLIENT_ADDR: LazyLock<MacAddr> =
        LazyLock::new(|| [0x7A, 0xE7, 0x76, 0xD9, 0xF2, 0x67].into());

    impl ScheduledScanReceiver {
        pub(crate) fn try_next(&mut self) -> Result<Option<fidl::Vmo>, mpsc::TryRecvError> {
            let res = self.scan_results_receiver.try_next();
            if let Ok(None) = res {
                self.stopped_by_firmware = true;
            }
            res
        }
    }

    fn passive_discovery_scan(token: i32) -> DiscoveryScan<i32> {
        DiscoveryScan::new(
            token,
            fidl_sme::ScanRequest::Passive(fidl_sme::PassiveScanRequest { channels: vec![] }),
        )
    }

    #[test]
    fn discovery_scan() {
        let mut sched = create_sched();
        let _next_txn_id = 0;
        let (_inspector, sme_inspect) = sme_inspect();
        let req = sched
            .enqueue_scan_to_discover(passive_discovery_scan(10))
            .expect("expected a ScanRequest");
        let txn_id = req.txn_id;
        sched
            .on_mlme_scan_result(fidl_mlme::ScanResult {
                txn_id,
                timestamp_nanos: zx::MonotonicInstant::get().into_nanos(),
                bss: fidl_ieee80211::BssDescription {
                    bssid: [1; 6],
                    ..fake_fidl_bss_description!(Open, ssid: Ssid::try_from("foo").unwrap())
                },
            })
            .expect("expect scan result received");
        assert_matches!(
            sched.on_mlme_scan_result(fidl_mlme::ScanResult {
                txn_id: txn_id + 100, // mismatching transaction id
                timestamp_nanos: zx::MonotonicInstant::get().into_nanos(),
                bss: fidl_ieee80211::BssDescription {
                    bssid: [2; 6],
                    ..fake_fidl_bss_description!(Open, ssid: Ssid::try_from("bar").unwrap())
                },
            },),
            Err(Error::ScanResultWrongTxnId)
        );
        sched
            .on_mlme_scan_result(fidl_mlme::ScanResult {
                txn_id,
                timestamp_nanos: zx::MonotonicInstant::get().into_nanos(),
                bss: fidl_ieee80211::BssDescription {
                    bssid: [3; 6],
                    ..fake_fidl_bss_description!(Open, ssid: Ssid::try_from("qux").unwrap())
                },
            })
            .expect("expect scan result received");
        let (scan_end, mlme_req) = assert_matches!(
            sched.on_mlme_scan_end(
                fidl_mlme::ScanEnd { txn_id, code: fidl_mlme::ScanResultCode::Success },
                &sme_inspect),
            Ok((scan_end, mlme_req)) => (scan_end, mlme_req)
        );
        assert!(mlme_req.is_none());
        let (tokens, bss_description_list) = assert_matches!(
            scan_end,
            ScanEnd {
                tokens,
                result_code: fidl_mlme::ScanResultCode::Success,
                bss_description_list
            } => (tokens, bss_description_list),
            "expected discovery scan to be completed successfully"
        );
        assert_eq!(vec![10], tokens);
        let mut ssid_list =
            bss_description_list.into_iter().map(|bss| bss.ssid).collect::<Vec<_>>();
        ssid_list.sort();
        assert_eq!(vec![Ssid::try_from("foo").unwrap(), Ssid::try_from("qux").unwrap()], ssid_list);
    }

    #[test_case(vec![
        fake_fidl_bss_description!(Open, ssid: Ssid::try_from("bar").unwrap()),
        fake_fidl_bss_description!(Open, ssid: Ssid::try_from("baz").unwrap()),
    ], vec![fake_bss_description!(Open, ssid: Ssid::try_from("baz").unwrap())] ;
                "when latest BSS Description is new")]
    #[test_case(vec![
        fake_fidl_bss_description!(Open, rssi_dbm: -36, channel: Channel::new(149, Bandwidth::Cbw20, FiveGhz)),
        fake_fidl_bss_description!(Open, rssi_dbm: -84, channel: Channel::new(165, Bandwidth::Cbw20, FiveGhz)),
    ], vec![fake_bss_description!(Open, rssi_dbm: -36, channel: Channel::new(149, Bandwidth::Cbw20, FiveGhz))] ;
                "when strong signal is first")]
    #[test_case(vec![
        fake_fidl_bss_description!(Open, rssi_dbm: -84, channel: Channel::new(64, Bandwidth::Cbw20, FiveGhz)),
        fake_fidl_bss_description!(Open, rssi_dbm: -36, channel: Channel::new(50, Bandwidth::Cbw20, FiveGhz)),
        fake_fidl_bss_description!(Open, rssi_dbm: -80, channel: Channel::new(36, Bandwidth::Cbw20, FiveGhz)),
    ], vec![fake_bss_description!(Open, rssi_dbm: -36, channel: Channel::new(50, Bandwidth::Cbw20, FiveGhz))];
                "when strong signal is middle")]
    #[test_case(vec![
        fake_fidl_bss_description!(Open, rssi_dbm: -84, channel: Channel::new(64, Bandwidth::Cbw20, FiveGhz)),
        fake_fidl_bss_description!(Open, rssi_dbm: -80, channel: Channel::new(36, Bandwidth::Cbw20, FiveGhz)),
        fake_fidl_bss_description!(Open, rssi_dbm: -36, channel: Channel::new(50, Bandwidth::Cbw20, FiveGhz)),
    ], vec![fake_bss_description!(Open, rssi_dbm: -36, channel: Channel::new(50, Bandwidth::Cbw20, FiveGhz))];
                "when strong signal is last")]
    #[test_case(vec![
        fake_fidl_bss_description!(Open, rssi_dbm: -84, ssid: Ssid::try_from("bar").unwrap(),
                                   channel: Channel::new(149, Bandwidth::Cbw20, FiveGhz)),
        fake_fidl_bss_description!(Open, rssi_dbm: -36, ssid: Ssid::try_from("bar").unwrap(),
                                   channel: Channel::new(165, Bandwidth::Cbw20, FiveGhz)),
        fake_fidl_bss_description!(Open, rssi_dbm: -40, ssid: Ssid::try_from("baz").unwrap(),
                                   channel: Channel::new(165, Bandwidth::Cbw20, FiveGhz)),
    ], vec![fake_bss_description!(Open, rssi_dbm: -40, ssid: Ssid::try_from("baz").unwrap(),
                                  channel: Channel::new(165, Bandwidth::Cbw20, FiveGhz))];
                "overwrite latest chosen channel")]
    fn deduplicate_by_bssid(
        bss_description_list_from_mlme: Vec<fidl_ieee80211::BssDescription>,
        returned_bss_description_list: Vec<BssDescription>,
    ) {
        let mut sched = create_sched();
        let _next_txn_id = 0;
        let (_inspector, sme_inspect) = sme_inspect();
        let req = sched
            .enqueue_scan_to_discover(passive_discovery_scan(10))
            .expect("expected a ScanRequest");
        let txn_id = req.txn_id;
        for bss in bss_description_list_from_mlme {
            sched
                .on_mlme_scan_result(fidl_mlme::ScanResult {
                    txn_id,
                    timestamp_nanos: zx::MonotonicInstant::get().into_nanos(),
                    bss,
                })
                .expect("expect scan result received");
        }
        let (scan_end, mlme_req) = assert_matches!(
            sched.on_mlme_scan_end(
                fidl_mlme::ScanEnd { txn_id, code: fidl_mlme::ScanResultCode::Success },
                &sme_inspect),
            Ok((scan_end, mlme_req)) => (scan_end, mlme_req)
        );
        assert!(mlme_req.is_none());
        let (tokens, bss_description_list) = assert_matches!(
            scan_end,
            ScanEnd {
                tokens,
                result_code: fidl_mlme::ScanResultCode::Success,
                bss_description_list
            } => (tokens, bss_description_list),
            "expected discovery scan to be completed successfully"
        );
        assert_eq!(vec![10], tokens);
        assert_eq!(bss_description_list, returned_bss_description_list);
    }

    #[test]
    fn discovery_scan_merge_ies() {
        let mut sched = create_sched();
        let _next_txn_id = 0;
        let (_inspector, sme_inspect) = sme_inspect();
        let req = sched
            .enqueue_scan_to_discover(passive_discovery_scan(10))
            .expect("expected a ScanRequest");
        let txn_id = req.txn_id;

        let mut bss = fake_fidl_bss_description!(Open, ssid: Ssid::try_from("ssid").unwrap());
        // Add an extra IE so we can distinguish this result.
        let ie_marker1 = &[0xdd, 0x07, 0xee, 0xee, 0xee, 0xee, 0xee, 0xee, 0xee];
        bss.ies.extend_from_slice(ie_marker1);
        sched
            .on_mlme_scan_result(fidl_mlme::ScanResult {
                txn_id,
                timestamp_nanos: zx::MonotonicInstant::get().into_nanos(),
                bss,
            })
            .expect("expect scan result received");

        let mut bss = fake_fidl_bss_description!(Open, ssid: Ssid::try_from("ssid").unwrap());
        // Add an extra IE so we can distinguish this result.
        let ie_marker2 = &[0xdd, 0x07, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
        bss.ies.extend_from_slice(ie_marker2);
        sched
            .on_mlme_scan_result(fidl_mlme::ScanResult {
                txn_id,
                timestamp_nanos: zx::MonotonicInstant::get().into_nanos(),
                bss,
            })
            .expect("expect scan result received");
        let (scan_end, mlme_req) = assert_matches!(
            sched.on_mlme_scan_end(
                fidl_mlme::ScanEnd { txn_id, code: fidl_mlme::ScanResultCode::Success },
                &sme_inspect),
            Ok((scan_end, mlme_req)) => (scan_end, mlme_req)
        );
        assert!(mlme_req.is_none());
        let (tokens, bss_description_list) = assert_matches!(
            scan_end,
            ScanEnd {
                tokens,
                result_code: fidl_mlme::ScanResultCode::Success,
                bss_description_list
            } => (tokens, bss_description_list),
            "expected discovery scan to be completed successfully"
        );
        assert_eq!(vec![10], tokens);

        assert_eq!(bss_description_list.len(), 1);
        // Verify that both IEs are processed.
        assert!(slice_contains(bss_description_list[0].ies(), ie_marker1));
        assert!(slice_contains(bss_description_list[0].ies(), ie_marker2));
    }

    fn slice_contains(slice: &[u8], subslice: &[u8]) -> bool {
        // https://github.com/rust-lang/regex/issues/451#issuecomment-367987989
        let re = {
            let mut re_string = String::with_capacity(6 + subslice.len() * 4);
            re_string += "(?-u:";
            for b in subslice {
                write!(re_string, "\\x{b:02X}").unwrap();
            }
            re_string += ")";
            Regex::new(&re_string).unwrap()
        };
        re.is_match(slice)
    }

    #[test_case(&[1, 2, 3], &[] => true; "vacuous")]
    #[test_case(&[1, 2, 3], &[1u8] => true; "one byte")]
    #[test_case(&[1, 2, 3], &[2u8, 3] => true; "multiple bytes")]
    #[test_case(&[1, 1, 1], &[1u8, 1] => true; "multiple matches")]
    #[test_case(&[1, 2, 3], &[0u8] => false; "no match")]
    #[test_case(&[1, 2, 3], &[1u8, 2, 3, 4] => false; "too large")]
    #[test_case(&[0x87, 0x77, 0x78], &[0x77, 0x77] => false; "misaligned match")]
    fn slice_contains_test(slice: &[u8], subslice: &[u8]) -> bool {
        slice_contains(slice, subslice)
    }

    #[test]
    fn test_passive_discovery_scan_args() {
        let mut sched = create_sched();
        let _next_txn_id = 0;
        let req = sched
            .enqueue_scan_to_discover(passive_discovery_scan(10))
            .expect("expected a ScanRequest");
        assert_eq!(req.txn_id, 1);
        assert_eq!(req.scan_type, fidl_mlme::ScanTypes::Passive);
        assert_eq!(
            req.channel_list.into_iter().collect::<HashSet<_>>(),
            CANDIDATE_PRIMARY_CHANNELS.iter().copied().map(|c| c.into()).collect::<HashSet<_>>()
        );
        assert_eq!(req.ssid_list, Vec::<Vec<u8>>::new());
        assert_eq!(req.probe_delay, 0);
        assert_eq!(req.min_channel_time, 200);
        assert_eq!(req.max_channel_time, 200);
    }

    #[test_case(true, HashSet::from([
        fidl_ieee80211::ChannelNumber { number: 1, band: TwoGhz },
        fidl_ieee80211::ChannelNumber { number: 36, band: FiveGhz },
        fidl_ieee80211::ChannelNumber { number: 165, band: FiveGhz },
    ]); "dfs_enabled")]
    #[test_case(false, HashSet::from([
        fidl_ieee80211::ChannelNumber { number: 1, band: TwoGhz },
    ]); "dfs_disabled")]
    fn test_active_discovery_scan_args_empty(
        dfs_supported: bool,
        expected_channels: HashSet<fidl_ieee80211::ChannelNumber>,
    ) {
        let device_info = device_info_with_channel(vec![1, 36, 165]);
        let mut spectrum_management = fake_spectrum_management_support_empty();
        if dfs_supported {
            spectrum_management.dfs.get_or_insert_with(Default::default).supported = Some(true);
        }
        let mut sched: ScanScheduler<i32> =
            ScanScheduler::new(Arc::new(device_info), spectrum_management);
        let _next_txn_id = 0;
        let scan_cmd = DiscoveryScan::new(
            10,
            fidl_sme::ScanRequest::Active(fidl_sme::ActiveScanRequest {
                ssids: vec![],
                channels: vec![],
            }),
        );
        let req = sched.enqueue_scan_to_discover(scan_cmd).expect("expected a ScanRequest");

        assert_eq!(req.txn_id, 1);
        assert_eq!(req.scan_type, fidl_mlme::ScanTypes::Active);
        assert_eq!(req.channel_list.into_iter().collect::<HashSet<_>>(), expected_channels);
        assert_eq!(req.ssid_list, Vec::<Vec<u8>>::new());
        assert_eq!(req.probe_delay, 5);
        assert_eq!(req.min_channel_time, 75);
        assert_eq!(req.max_channel_time, 75);
    }

    #[test]
    fn test_active_discovery_scan_args_filled() {
        let device_info = device_info_with_channel(vec![1, 36, 165]);
        let mut sched: ScanScheduler<i32> =
            ScanScheduler::new(Arc::new(device_info), fake_spectrum_management_support_empty());
        let _next_txn_id = 0;
        let ssid1: Vec<u8> = Ssid::try_from("ssid1").unwrap().into();
        let ssid2: Vec<u8> = Ssid::try_from("ssid2").unwrap().into();
        let scan_cmd = DiscoveryScan::new(
            10,
            fidl_sme::ScanRequest::Active(fidl_sme::ActiveScanRequest {
                ssids: vec![ssid1.clone(), ssid2.clone()],
                // TODO(https://fxbug.dev/42169913): SME silently ignores unsupported channels
                channels: vec![1, 20, 100],
            }),
        );
        let req = sched.enqueue_scan_to_discover(scan_cmd).expect("expected a ScanRequest");

        assert_eq!(req.txn_id, 1);
        assert_eq!(req.scan_type, fidl_mlme::ScanTypes::Active);
        assert_eq!(
            req.channel_list,
            vec![fidl_ieee80211::ChannelNumber { number: 1, band: TwoGhz }]
        );
        assert_eq!(req.ssid_list, vec![ssid1, ssid2]);
        assert_eq!(req.probe_delay, 5);
        assert_eq!(req.min_channel_time, 75);
        assert_eq!(req.max_channel_time, 75);
    }

    #[test]
    fn test_passive_discovery_scan_args_filled() {
        // Set up the device that can operate on channels 1, 36, and 165.
        let device_info = device_info_with_channel(vec![1, 36, 165]);
        let mut sched: ScanScheduler<i32> =
            ScanScheduler::new(Arc::new(device_info), fake_spectrum_management_support_empty());
        // Request a scan using only some of the supported channels.
        let scan_cmd = DiscoveryScan::new(
            10,
            fidl_sme::ScanRequest::Passive(fidl_sme::PassiveScanRequest { channels: vec![1, 36] }),
        );
        let _next_txn_id = 0;
        let req = sched.enqueue_scan_to_discover(scan_cmd).expect("expected a ScanRequest");

        assert_eq!(req.txn_id, 1);
        assert_eq!(req.scan_type, fidl_mlme::ScanTypes::Passive);
        // Verify that only the requested channels are included.
        assert_eq!(
            req.channel_list.into_iter().collect::<HashSet<_>>(),
            HashSet::from([
                fidl_ieee80211::ChannelNumber { number: 1, band: TwoGhz },
                fidl_ieee80211::ChannelNumber { number: 36, band: FiveGhz },
            ])
        );
        assert_eq!(req.ssid_list, Vec::<Vec<u8>>::new());
        assert_eq!(req.probe_delay, 0);
        assert_eq!(req.min_channel_time, 200);
        assert_eq!(req.max_channel_time, 200);
    }

    #[test]
    fn test_passive_discovery_scan_args_unsupported_filtered() {
        let device_info = device_info_with_channel(vec![1, 36]);
        let mut sched: ScanScheduler<i32> =
            ScanScheduler::new(Arc::new(device_info), fake_spectrum_management_support_empty());
        let _next_txn_id = 0;
        // Request a scan that includes a channel not supported by the device.
        let scan_cmd = DiscoveryScan::new(
            10,
            fidl_sme::ScanRequest::Passive(fidl_sme::PassiveScanRequest {
                channels: vec![1, 6, 36],
            }),
        );
        let req = sched.enqueue_scan_to_discover(scan_cmd).expect("expected a ScanRequest");

        assert_eq!(req.txn_id, 1);
        assert_eq!(req.scan_type, fidl_mlme::ScanTypes::Passive);
        // Verify that the unsupported channel 6 was filtered out.
        assert_eq!(
            req.channel_list.into_iter().collect::<HashSet<_>>(),
            HashSet::from([
                fidl_ieee80211::ChannelNumber { number: 1, band: TwoGhz },
                fidl_ieee80211::ChannelNumber { number: 36, band: FiveGhz },
            ])
        );
    }

    #[test]
    fn test_passive_discovery_scan_args_invalid_filtered() {
        let device_info = device_info_with_channel(vec![1, 200]);
        let mut sched: ScanScheduler<i32> =
            ScanScheduler::new(Arc::new(device_info), fake_spectrum_management_support_empty());
        let _next_txn_id = 0;
        // Request a scan that includes an invalid channel.
        let scan_cmd = DiscoveryScan::new(
            10,
            fidl_sme::ScanRequest::Passive(fidl_sme::PassiveScanRequest { channels: vec![1, 200] }),
        );
        let req = sched.enqueue_scan_to_discover(scan_cmd).expect("expected a ScanRequest");

        assert_eq!(req.txn_id, 1);
        assert_eq!(req.scan_type, fidl_mlme::ScanTypes::Passive);
        // Verify that the invalid channel 200 was filtered out and the valid channel is included.
        assert_eq!(
            req.channel_list,
            vec![fidl_ieee80211::ChannelNumber { number: 1, band: TwoGhz }]
        );
    }

    #[test]
    fn test_passive_discovery_scan_args_empty_list() {
        let device_info = device_info_with_channel(vec![1, 36, 165]);
        let mut sched: ScanScheduler<i32> =
            ScanScheduler::new(Arc::new(device_info), fake_spectrum_management_support_empty());
        let _next_txn_id = 0;
        let scan_cmd = DiscoveryScan::new(
            10,
            fidl_sme::ScanRequest::Passive(fidl_sme::PassiveScanRequest { channels: vec![] }),
        );
        let req = sched.enqueue_scan_to_discover(scan_cmd).expect("expected a ScanRequest");

        assert_eq!(req.txn_id, 1);
        assert_eq!(req.scan_type, fidl_mlme::ScanTypes::Passive);
        assert_eq!(
            req.channel_list.into_iter().collect::<HashSet<_>>(),
            HashSet::from([
                fidl_ieee80211::ChannelNumber { number: 1, band: TwoGhz },
                fidl_ieee80211::ChannelNumber { number: 36, band: FiveGhz },
                fidl_ieee80211::ChannelNumber { number: 165, band: FiveGhz },
            ])
        );
    }

    #[test]
    fn test_discovery_scans_dedupe_single_group() {
        let mut sched = create_sched();
        let _next_txn_id = 0;
        let (_inspector, sme_inspect) = sme_inspect();

        // Post one scan command, expect a message to MLME
        let mlme_req = sched
            .enqueue_scan_to_discover(passive_discovery_scan(10))
            .expect("expected a ScanRequest");
        let txn_id = mlme_req.txn_id;

        // Report a scan result
        sched
            .on_mlme_scan_result(fidl_mlme::ScanResult {
                txn_id,
                timestamp_nanos: zx::MonotonicInstant::get().into_nanos(),
                bss: fidl_ieee80211::BssDescription {
                    bssid: [1; 6],
                    ..fake_fidl_bss_description!(Open, ssid: Ssid::try_from("foo").unwrap())
                },
            })
            .expect("expect scan result received");

        // Post another command. It should not issue another request to the MLME since
        // there is already an on-going one
        assert!(sched.enqueue_scan_to_discover(passive_discovery_scan(20)).is_none());

        // Report another scan result and the end of the scan transaction
        sched
            .on_mlme_scan_result(fidl_mlme::ScanResult {
                txn_id,
                timestamp_nanos: zx::MonotonicInstant::get().into_nanos(),
                bss: fidl_ieee80211::BssDescription {
                    bssid: [2; 6],
                    ..fake_fidl_bss_description!(Open, ssid: Ssid::try_from("bar").unwrap())
                },
            })
            .expect("expect scan result received");
        let (scan_end, mlme_req) = assert_matches!(
            sched.on_mlme_scan_end(
                fidl_mlme::ScanEnd { txn_id, code: fidl_mlme::ScanResultCode::Success },
                &sme_inspect),
            Ok((scan_end, mlme_req)) => (scan_end, mlme_req)
        );

        // We don't expect another request to the MLME
        assert!(mlme_req.is_none());

        // Expect a discovery result with both tokens and both SSIDs
        assert_discovery_scan_result(
            scan_end,
            vec![10, 20],
            vec![Ssid::try_from("bar").unwrap(), Ssid::try_from("foo").unwrap()],
        );
    }

    #[test]
    fn test_discovery_scans_dedupe_multiple_groups() {
        let mut sched = create_sched();
        let (_inspector, sme_inspect) = sme_inspect();

        // Post a passive scan command, expect a message to MLME
        let mlme_req = sched
            .enqueue_scan_to_discover(passive_discovery_scan(10))
            .expect("expected a ScanRequest");
        let txn_id = mlme_req.txn_id;

        // Post an active scan command, which should be enqueued until the previous one finishes
        let scan_cmd = DiscoveryScan::new(
            20,
            fidl_sme::ScanRequest::Active(fidl_sme::ActiveScanRequest {
                ssids: vec![],
                channels: vec![],
            }),
        );
        assert!(sched.enqueue_scan_to_discover(scan_cmd).is_none());

        // Post a passive scan command. It should be merged with the ongoing one and so should not
        // issue another request to MLME
        assert!(sched.enqueue_scan_to_discover(passive_discovery_scan(30)).is_none());

        // Post an active scan command. It should be merged with the active scan command that's
        // still enqueued, and so should not issue another request to MLME
        let scan_cmd = DiscoveryScan::new(
            40,
            fidl_sme::ScanRequest::Active(fidl_sme::ActiveScanRequest {
                ssids: vec![],
                channels: vec![],
            }),
        );
        assert!(sched.enqueue_scan_to_discover(scan_cmd).is_none());

        // Report scan result and scan end
        sched
            .on_mlme_scan_result(fidl_mlme::ScanResult {
                txn_id,
                timestamp_nanos: zx::MonotonicInstant::get().into_nanos(),
                bss: fidl_ieee80211::BssDescription {
                    bssid: [1; 6],
                    ..fake_fidl_bss_description!(Open, ssid: Ssid::try_from("foo").unwrap())
                },
            })
            .expect("expect scan result received");
        let (scan_end, mlme_req) = assert_matches!(
            sched.on_mlme_scan_end(
                fidl_mlme::ScanEnd { txn_id, code: fidl_mlme::ScanResultCode::Success },
                &sme_inspect),
            Ok((scan_end, mlme_req)) => (scan_end, mlme_req)
        );

        // Expect discovery result with 1st and 3rd tokens
        assert_discovery_scan_result(scan_end, vec![10, 30], vec![Ssid::try_from("foo").unwrap()]);

        // Next mlme_req should be an active scan request
        assert!(mlme_req.is_some());
        let mlme_req = mlme_req.unwrap();
        assert_eq!(mlme_req.scan_type, fidl_mlme::ScanTypes::Active);
        let txn_id = mlme_req.txn_id;

        // Report scan result and scan end
        sched
            .on_mlme_scan_result(fidl_mlme::ScanResult {
                txn_id,
                timestamp_nanos: zx::MonotonicInstant::get().into_nanos(),
                bss: fidl_ieee80211::BssDescription {
                    bssid: [2; 6],
                    ..fake_fidl_bss_description!(Open, ssid: Ssid::try_from("bar").unwrap())
                },
            })
            .expect("expect scan result received");
        let (scan_end, mlme_req) = assert_matches!(
            sched.on_mlme_scan_end(
                fidl_mlme::ScanEnd { txn_id, code: fidl_mlme::ScanResultCode::Success },
                &sme_inspect),
            Ok((scan_end, mlme_req)) => (scan_end, mlme_req)
        );

        // Expect discovery result with 2nd and 4th tokens
        assert_discovery_scan_result(scan_end, vec![20, 40], vec![Ssid::try_from("bar").unwrap()]);

        // We don't expect another request to the MLME
        assert!(mlme_req.is_none());
    }

    #[test]
    fn test_discovery_scan_result_wrong_txn_id() {
        let mut sched = create_sched();
        let _next_txn_id = 0;

        // Post a passive scan command, expect a message to MLME
        let mlme_req = sched
            .enqueue_scan_to_discover(passive_discovery_scan(10))
            .expect("expected a ScanRequest");
        let txn_id = mlme_req.txn_id;

        // Report scan result with wrong txn id
        assert_matches!(
            sched.on_mlme_scan_result(fidl_mlme::ScanResult {
                txn_id: txn_id + 1,
                timestamp_nanos: zx::MonotonicInstant::get().into_nanos(),
                bss: fidl_ieee80211::BssDescription {
                    bssid: [1; 6],
                    ..fake_fidl_bss_description!(Open, ssid: Ssid::try_from("foo").unwrap())
                },
            },),
            Err(Error::ScanResultWrongTxnId)
        );
    }

    #[test]
    fn test_discovery_scan_result_not_scanning() {
        let mut sched = create_sched();
        assert_matches!(
            sched.on_mlme_scan_result(fidl_mlme::ScanResult {
                txn_id: 0,
                timestamp_nanos: zx::MonotonicInstant::get().into_nanos(),
                bss: fidl_ieee80211::BssDescription {
                    bssid: [1; 6],
                    ..fake_fidl_bss_description!(Open, ssid: Ssid::try_from("foo").unwrap())
                },
            },),
            Err(Error::ScanResultNotScanning)
        );
    }

    #[test]
    fn test_discovery_scan_end_wrong_txn_id() {
        let mut sched = create_sched();
        let _next_txn_id = 0;
        let (_inspector, sme_inspect) = sme_inspect();

        // Post a passive scan command, expect a message to MLME
        let mlme_req = sched
            .enqueue_scan_to_discover(passive_discovery_scan(10))
            .expect("expected a ScanRequest");
        let txn_id = mlme_req.txn_id;

        assert_matches!(
            sched.on_mlme_scan_end(
                fidl_mlme::ScanEnd { txn_id: txn_id + 1, code: fidl_mlme::ScanResultCode::Success },
                &sme_inspect
            ),
            Err(Error::ScanEndWrongTxnId)
        );
    }

    #[test]
    fn test_discovery_scan_end_not_scanning() {
        let mut sched = create_sched();
        let _next_txn_id = 0;
        let (_inspector, sme_inspect) = sme_inspect();
        assert_matches!(
            sched.on_mlme_scan_end(
                fidl_mlme::ScanEnd { txn_id: 0, code: fidl_mlme::ScanResultCode::Success },
                &sme_inspect
            ),
            Err(Error::ScanEndNotScanning)
        );
    }

    fn assert_discovery_scan_result(
        scan_end: ScanEnd<i32>,
        expected_tokens: Vec<i32>,
        expected_ssids: Vec<Ssid>,
    ) {
        let (tokens, bss_description_list) = assert_matches!(
            scan_end,
            ScanEnd {
                tokens,
                result_code: fidl_mlme::ScanResultCode::Success,
                bss_description_list
            } => (tokens, bss_description_list),
            "expected discovery scan to be completed successfully"
        );
        assert_eq!(tokens, expected_tokens);
        let mut ssid_list =
            bss_description_list.into_iter().map(|bss| bss.ssid.clone()).collect::<Vec<_>>();
        ssid_list.sort();
        assert_eq!(ssid_list, expected_ssids);
    }

    fn create_sched() -> ScanScheduler<i32> {
        ScanScheduler::new(
            Arc::new(test_utils::fake_device_info(*CLIENT_ADDR)),
            fake_spectrum_management_support_empty(),
        )
    }

    fn device_info_with_channel(operating_channels: Vec<u8>) -> fidl_mlme::DeviceInfo {
        fidl_mlme::DeviceInfo {
            bands: vec![fidl_mlme::BandCapability {
                primary_channels: operating_channels
                    .into_iter()
                    .map(|n| fidl_ieee80211::ChannelNumber {
                        number: n,
                        band: fidl_ieee80211::WlanBand::FiveGhz,
                    })
                    .collect(),
                ..fake_5ghz_band_capability()
            }],
            ..test_utils::fake_device_info(*CLIENT_ADDR)
        }
    }

    fn sme_inspect() -> (Inspector, Arc<inspect::SmeTree>) {
        let inspector = Inspector::default();
        let sme_inspect = Arc::new(inspect::SmeTree::new(
            inspector.clone(),
            inspector.root().create_child("usme"),
            &test_utils::fake_device_info([1u8; 6].into()),
            &fake_spectrum_management_support_empty(),
        ));
        (inspector, sme_inspect)
    }

    #[test]
    fn test_scan_scheduler_routing() {
        let mut sched = create_sched();
        let (mlme_sink, mut _mlme_stream) = mpsc::unbounded();
        let mlme_sink = MlmeSink::new(mlme_sink);

        let (responder, _receiver) = Responder::new();
        let mut stream = sched.start_scheduled_scan(
            fidl_common::ScheduledScanRequest { ..Default::default() },
            mlme_sink,
            responder,
        );
        let sched_txn_id = stream.txn_id;
        assert_eq!(sched_txn_id, 1);

        // Enqueue discovery scan
        let req = sched.enqueue_scan_to_discover(passive_discovery_scan(10)).unwrap();
        let disc_txn_id = req.txn_id;
        assert_eq!(disc_txn_id, 2);

        // Send scan result for scheduled scan (txn_id 1)
        let bss1 = fake_fidl_bss_description!(Open, ssid: Ssid::try_from("scheduled").unwrap());
        sched
            .on_mlme_scan_result(fidl_mlme::ScanResult {
                txn_id: sched_txn_id,
                timestamp_nanos: 1000,
                bss: bss1.clone(),
            })
            .unwrap();

        // Send scan result for discovery scan (txn_id 2)
        let bss2 = fake_fidl_bss_description!(Open, ssid: Ssid::try_from("discovery").unwrap());
        sched
            .on_mlme_scan_result(fidl_mlme::ScanResult {
                txn_id: disc_txn_id,
                timestamp_nanos: 2000,
                bss: bss2.clone(),
            })
            .unwrap();

        // Trigger matches available for scheduled scan
        let (_inspector, sme_inspect) = sme_inspect();
        let cfg = crate::client::ClientConfig::default();
        let device_info = test_utils::fake_device_info(*CLIENT_ADDR);
        let security_support = wlan_common::test_utils::fake_features::fake_security_support();
        sched.on_scheduled_scan_matches_available(
            sched_txn_id,
            &sme_inspect,
            &cfg,
            &device_info,
            &security_support,
        );

        // Verify scheduled scan receiver got the matches
        assert_matches!(
            stream.try_next(),
            Ok(Some(scan_results)) => {
                let results = wlan_common::scan::read_vmo(scan_results).unwrap();
                assert_eq!(results.len(), 1);
                let parsed_bss = wlan_common::bss::BssDescription::try_from(results[0].bss_description.clone()).unwrap();
                assert_eq!(parsed_bss.ssid, Ssid::try_from("scheduled").unwrap());
            }
        );

        // Verify discovery scan state has the match
        if let ScanState::ScanningToDiscover { bss_map, .. } = &sched.current {
            assert!(bss_map.contains_key(&Bssid::from(bss2.bssid)));
        } else {
            panic!("Expected ScanState::ScanningToDiscover");
        }
    }
}
