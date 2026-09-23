// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::client::{ClientSmeStatus, ServingApInfo};
use fidl_fuchsia_wlan_common as fidl_common;
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_mlme as fidl_mlme;
use fuchsia_inspect::{
    BoolProperty, BytesProperty, Inspector, IntProperty, Node, Property, StringProperty,
    UintProperty,
};
use fuchsia_inspect_contrib::inspect_insert;
use fuchsia_inspect_contrib::log::{InspectListClosure, InspectUintArray};
use fuchsia_inspect_contrib::nodes::{BoundedListNode, MonotonicTimeProperty, NodeTimeExt};
use fuchsia_sync::Mutex;
use ieee80211::Ssid;
use wlan_common::ie::{self, wsc};

/// These limits are set to capture roughly 5 to 10 recent connection attempts. An average
/// successful connection attempt would generate about 5 state events and 7 supplicant events (this
/// number may be different in error cases).
const STATE_EVENTS_LIMIT: usize = 50;
const RSN_EVENTS_LIMIT: usize = 50;

/// Limit set to capture roughly join scans for 10 recent connection attempts.
const JOIN_SCAN_EVENTS_LIMIT: usize = 10;

/// Display idle status str
const IDLE_STR: &str = "idle";

/// Wrapper struct SME inspection nodes
pub struct SmeTree {
    /// Top level inspector that contains the SmeTree root node.
    pub inspector: Inspector,
    /// Base SME inspection node that holds all other nodes in the SmeTree.
    pub _root_node: Node,
    /// Inspection node to log recent state transitions, or cases where an event would that would
    /// normally cause a state transition doesn't due to an error.
    pub state_events: Mutex<BoundedListNode>,
    /// Inspection node to log EAPOL frames processed by supplicant and its output.
    pub rsn_events: Mutex<BoundedListNode>,
    /// Inspection node to log recent join scan results.
    // We expect to only write this node
    #[allow(dead_code)]
    pub join_scan_events: Mutex<BoundedListNode>,
    /// Inspect node to log periodic pulse check. For the most part, information logged in this
    /// node can be derived from (and is therefore redundant with) `state_events` node. This
    /// is logged  for two reasons:
    /// 1. To show a quick summary of latest status.
    /// 2. To show how up-to-date the latest status is (although pulse is logged within SME, it can
    ///    be thought similarly to an external entity periodically checking SME's status).
    pub last_pulse: Mutex<PulseNode>,

    /// Number of FIDL BSS we discard in scan because we fail to convert them.
    pub scan_discard_fidl_bss: UintProperty,

    /// Number of time we decide to merge an IE during scan but it fails.
    /// This should never occur, but we log the count in case the assumption is violated.
    pub scan_merge_ie_failures: UintProperty,
}

impl SmeTree {
    pub fn new(
        inspector: Inspector,
        node: Node,
        device_info: &fidl_mlme::DeviceInfo,
        spectrum_management_support: &fidl_common::SpectrumManagementSupport,
    ) -> Self {
        let state_events =
            BoundedListNode::new(node.create_child("state_events"), STATE_EVENTS_LIMIT);
        let rsn_events = BoundedListNode::new(node.create_child("rsn_events"), RSN_EVENTS_LIMIT);
        let join_scan_events =
            BoundedListNode::new(node.create_child("join_scan_events"), JOIN_SCAN_EVENTS_LIMIT);
        let pulse = PulseNode::new(node.create_child("last_pulse"));
        let scan_discard_fidl_bss = node.create_uint("scan_discard_fidl_bss", 0);
        let scan_merge_ie_failures = node.create_uint("scan_merge_ie_failures", 0);
        inspect_insert!(node, device_support: {
            device_info: {
                bands: InspectListClosure(&device_info.bands, |node, key, band| {
                    let channels: Vec<u8> =
                        band.primary_channels.iter().map(|c| c.number).collect();
                    inspect_insert!(node, var key: {
                        band: match band.band {
                            fidl_ieee80211::WlanBand::TwoGhz => "2.4Ghz",
                            fidl_ieee80211::WlanBand::FiveGhz => "5Ghz",
                            _ => "Unknown",
                        },
                        operating_channels: InspectUintArray::new(&channels),
                    });
                }),
            },
            spectrum_management_support: {
                dfs: {
                    supported: spectrum_management_support
                        .dfs
                        .as_ref()
                        .is_some_and(|dfs| dfs.supported.unwrap_or(false)),
                }
            }
        });
        Self {
            inspector,
            _root_node: node,
            state_events: Mutex::new(state_events),
            rsn_events: Mutex::new(rsn_events),
            join_scan_events: Mutex::new(join_scan_events),
            last_pulse: Mutex::new(pulse),
            scan_discard_fidl_bss,
            scan_merge_ie_failures,
        }
    }

    pub fn update_pulse(&self, new_status: ClientSmeStatus) {
        self.last_pulse.lock().update(new_status)
    }

    pub fn clone_vmo_data(&self) -> Option<fidl::Vmo> {
        self.inspector.copy_vmo()
    }
}

pub struct PulseNode {
    node: Node,
    _started: MonotonicTimeProperty,
    last_updated: MonotonicTimeProperty,
    last_link_up: Option<MonotonicTimeProperty>,
    status_node: Option<ClientSmeStatusNode>,

    // Not part of Inspect node. We use it to compare new status against existing status
    status: Option<ClientSmeStatus>,
}

impl PulseNode {
    fn new(node: Node) -> Self {
        let now = zx::MonotonicInstant::get();
        let started = node.create_time_at("started", now);
        let last_updated = node.create_time_at("last_updated", now);
        Self {
            node,
            _started: started,
            last_updated,
            last_link_up: None,
            status_node: None,
            status: None,
        }
    }

    pub fn update(&mut self, new_status: ClientSmeStatus) {
        let now = zx::MonotonicInstant::get();
        self.last_updated.set_at(now);

        // This method is always called when there's a state transition, so even if the client is
        // no longer connected now, if the client was previously connected, we can conclude
        // that they were connected until now.
        if new_status.is_connected()
            || self.status.as_ref().map(|s| s.is_connected()).unwrap_or(false)
        {
            match &self.last_link_up {
                Some(last_link_up) => last_link_up.set_at(now),
                None => self.last_link_up = Some(self.node.create_time_at("last_link_up", now)),
            }
        }

        // Do not update status_node if status is the same.
        if let Some(status) = &self.status
            && *status == new_status
        {
            return;
        }

        match self.status_node.as_mut() {
            Some(status_node) => status_node.update(self.status.as_ref(), &new_status),
            None => {
                self.status_node =
                    Some(ClientSmeStatusNode::new(self.node.create_child("status"), &new_status))
            }
        }
        self.status = Some(new_status);
    }
}

pub struct ClientSmeStatusNode {
    node: Node,
    status_str: StringProperty,
    prev_connected_to: Option<ServingApInfoNode>,
    connected_to: Option<ServingApInfoNode>,
    connecting_to: Option<ConnectingToNode>,
}

impl ClientSmeStatusNode {
    fn new(node: Node, status: &ClientSmeStatus) -> Self {
        let status_str = node.create_string("status_str", IDLE_STR);
        let mut status_node = Self {
            node,
            status_str,
            prev_connected_to: None,
            connected_to: None,
            connecting_to: None,
        };
        status_node.update(None, status);
        status_node
    }

    pub fn update(&mut self, old_status: Option<&ClientSmeStatus>, new_status: &ClientSmeStatus) {
        let status_str = match new_status {
            ClientSmeStatus::Connected(_) => "connected",
            ClientSmeStatus::Connecting(_) => "connecting",
            ClientSmeStatus::Roaming(_) => "roaming",
            ClientSmeStatus::Idle => IDLE_STR,
        };
        self.status_str.set(status_str);

        if status_str == IDLE_STR
            && let Some(ClientSmeStatus::Connected(serving_ap_info)) = old_status
        {
            match self.prev_connected_to.as_mut() {
                Some(prev_connected_to) => prev_connected_to.update(serving_ap_info),
                None => {
                    self.prev_connected_to = Some(ServingApInfoNode::new(
                        self.node.create_child("prev_connected_to"),
                        serving_ap_info,
                    ));
                }
            }
        }

        match &new_status {
            ClientSmeStatus::Connected(serving_ap_info) => match self.connected_to.as_mut() {
                Some(connected_to) => connected_to.update(serving_ap_info),
                None => {
                    self.connected_to = Some(ServingApInfoNode::new(
                        self.node.create_child("connected_to"),
                        serving_ap_info,
                    ));
                }
            },
            ClientSmeStatus::Connecting(_)
            | ClientSmeStatus::Roaming(_)
            | ClientSmeStatus::Idle => {
                self.connected_to = None;
            }
        }

        match &new_status {
            ClientSmeStatus::Connecting(ssid) => match self.connecting_to.as_mut() {
                Some(connecting_to) => connecting_to.update(ssid),
                None => {
                    self.connecting_to =
                        Some(ConnectingToNode::new(self.node.create_child("connecting_to"), ssid));
                }
            },
            ClientSmeStatus::Connected(_) | ClientSmeStatus::Roaming(_) | ClientSmeStatus::Idle => {
                self.connecting_to = None;
            }
        }
    }
}

pub struct ServingApInfoNode {
    node: Node,
    bssid: StringProperty,
    ssid: StringProperty,

    rssi_dbm: IntProperty,
    snr_db: IntProperty,
    signal_report_time: MonotonicTimeProperty,
    channel: ChannelNode,
    protection: StringProperty,
    is_wmm_assoc: BoolProperty,
    wmm_param: Option<BssWmmParamNode>,
    ht_cap: Option<BytesProperty>,
    vht_cap: Option<BytesProperty>,
    wsc: Option<BssWscNode>,
}

impl ServingApInfoNode {
    fn new(node: Node, ap: &ServingApInfo) -> Self {
        let (cbw, secondary80_num) = ap.channel.bandwidth.to_fidl();
        let secondary80 =
            fidl_ieee80211::ChannelNumber { band: ap.channel.band, number: secondary80_num };
        let mut serving_ap_info_node = Self {
            bssid: node.create_string("bssid", ap.bssid.to_string()),
            ssid: node.create_string("ssid", ap.ssid.to_string()),
            rssi_dbm: node.create_int("rssi_dbm", ap.rssi_dbm as i64),
            snr_db: node.create_int("snr_db", ap.snr_db as i64),
            signal_report_time: node.create_time_at("signal_report_time", ap.signal_report_time),
            channel: ChannelNode::new(
                node.create_child("channel"),
                ap.channel.into(),
                cbw,
                secondary80,
            ),
            protection: node.create_string("protection", format!("{}", ap.protection)),

            // Reuse update_* helper functions to fill this fields.
            ht_cap: None,
            vht_cap: None,
            is_wmm_assoc: node.create_bool("is_wmm_assoc", false),
            wmm_param: None,
            wsc: None,

            node,
        };
        serving_ap_info_node.update_ht_cap_node(ap);
        serving_ap_info_node.update_vht_cap_node(ap);
        serving_ap_info_node.update_wmm_node(ap);
        serving_ap_info_node.update_wsc_node(ap);

        serving_ap_info_node
    }

    fn update(&mut self, ap: &ServingApInfo) {
        let (cbw, secondary80_num) = ap.channel.bandwidth.to_fidl();
        let secondary80 =
            fidl_ieee80211::ChannelNumber { band: ap.channel.band, number: secondary80_num };
        self.bssid.set(&ap.bssid.to_string());
        self.ssid.set(&ap.ssid.to_string());
        self.rssi_dbm.set(ap.rssi_dbm as i64);
        self.snr_db.set(ap.snr_db as i64);
        self.signal_report_time.set_at(ap.signal_report_time);
        self.channel.update(ap.channel.into(), cbw, secondary80);
        self.protection.set(&format!("{}", ap.protection));

        self.update_ht_cap_node(ap);
        self.update_vht_cap_node(ap);
        self.update_wmm_node(ap);
        self.update_wsc_node(ap);
    }

    fn update_ht_cap_node(&mut self, ap: &ServingApInfo) {
        match &ap.ht_cap {
            Some(ht_cap) => match self.ht_cap.as_mut() {
                Some(ht_cap_prop) => ht_cap_prop.set(&ht_cap.bytes),
                None => self.ht_cap = Some(self.node.create_bytes("ht_cap", ht_cap.bytes)),
            },
            None => {
                self.ht_cap = None;
            }
        }
    }

    fn update_vht_cap_node(&mut self, ap: &ServingApInfo) {
        match &ap.vht_cap {
            Some(vht_cap) => match self.vht_cap.as_mut() {
                Some(vht_cap_prop) => vht_cap_prop.set(&vht_cap.bytes),
                None => self.vht_cap = Some(self.node.create_bytes("vht_cap", vht_cap.bytes)),
            },
            None => {
                self.vht_cap = None;
            }
        }
    }

    fn update_wmm_node(&mut self, ap: &ServingApInfo) {
        match &ap.wmm_param {
            Some(wmm_param) => {
                self.is_wmm_assoc.set(true);
                match self.wmm_param.as_mut() {
                    Some(wmm_param_node) => wmm_param_node.update(wmm_param),
                    None => {
                        self.wmm_param = Some(BssWmmParamNode::new(
                            self.node.create_child("wmm_param"),
                            wmm_param,
                        ))
                    }
                }
            }
            None => {
                self.is_wmm_assoc.set(false);
                self.wmm_param = None;
            }
        }
    }

    fn update_wsc_node(&mut self, ap: &ServingApInfo) {
        match &ap.probe_resp_wsc {
            Some(wsc) => match self.wsc.as_mut() {
                Some(wsc_node) => wsc_node.update(wsc),
                None => self.wsc = Some(BssWscNode::new(self.node.create_child("wsc"), wsc)),
            },
            None => {
                self.wsc = None;
            }
        }
    }
}

pub struct ChannelNode {
    _node: Node,
    primary_channel: UintProperty,
    primary_band: StringProperty,
    cbw: StringProperty,
    secondary80_channel: UintProperty,
    secondary80_band: StringProperty,
}

impl ChannelNode {
    pub fn new(
        node: Node,
        channel: fidl_ieee80211::ChannelNumber,
        cbw: fidl_ieee80211::ChannelBandwidth,
        secondary80: fidl_ieee80211::ChannelNumber,
    ) -> Self {
        let primary_channel = node.create_uint("primary_channel", channel.number as u64);
        let primary_band =
            node.create_string("primary_band", Self::band_from_wlan_band(channel.band));
        let cbw = node.create_string("cbw", format!("{:?}", cbw));
        let secondary80_channel =
            node.create_uint("secondary80_channel", secondary80.number as u64);
        let secondary80_band =
            node.create_string("secondary80_band", Self::band_from_wlan_band(secondary80.band));
        Self {
            _node: node,
            primary_channel,
            primary_band,
            cbw,
            secondary80_channel,
            secondary80_band,
        }
    }

    pub fn update(
        &mut self,
        channel: fidl_ieee80211::ChannelNumber,
        cbw: fidl_ieee80211::ChannelBandwidth,
        secondary80: fidl_ieee80211::ChannelNumber,
    ) {
        self.primary_channel.set(channel.number as u64);
        self.primary_band.set(Self::band_from_wlan_band(channel.band));
        self.cbw.set(&format!("{:?}", cbw));
        self.secondary80_channel.set(secondary80.number as u64);
        self.secondary80_band.set(Self::band_from_wlan_band(secondary80.band));
    }

    fn band_from_wlan_band(band: fidl_ieee80211::WlanBand) -> &'static str {
        match band {
            fidl_ieee80211::WlanBand::TwoGhz => "2.4Ghz",
            fidl_ieee80211::WlanBand::FiveGhz => "5Ghz",
            _ => "Unknown",
        }
    }
}

pub struct BssWmmParamNode {
    _node: Node,
    wmm_info: BssWmmInfoNode,
    ac_be: BssWmmAcParamsNode,
    ac_bk: BssWmmAcParamsNode,
    ac_vi: BssWmmAcParamsNode,
    ac_vo: BssWmmAcParamsNode,
}

impl BssWmmParamNode {
    fn new(node: Node, wmm_param: &ie::WmmParam) -> Self {
        let wmm_info =
            BssWmmInfoNode::new(node.create_child("wmm_info"), wmm_param.wmm_info.ap_wmm_info());
        let ac_be = BssWmmAcParamsNode::new(node.create_child("ac_be"), wmm_param.ac_be_params);
        let ac_bk = BssWmmAcParamsNode::new(node.create_child("ac_bk"), wmm_param.ac_bk_params);
        let ac_vi = BssWmmAcParamsNode::new(node.create_child("ac_vi"), wmm_param.ac_vi_params);
        let ac_vo = BssWmmAcParamsNode::new(node.create_child("ac_vo"), wmm_param.ac_vo_params);
        Self { _node: node, wmm_info, ac_be, ac_bk, ac_vi, ac_vo }
    }

    fn update(&mut self, wmm_param: &ie::WmmParam) {
        self.wmm_info.update(&wmm_param.wmm_info.ap_wmm_info());
        self.ac_be.update(&wmm_param.ac_be_params);
        self.ac_bk.update(&wmm_param.ac_bk_params);
        self.ac_vi.update(&wmm_param.ac_vi_params);
        self.ac_vo.update(&wmm_param.ac_vo_params);
    }
}

pub struct BssWmmInfoNode {
    _node: Node,
    param_set_count: UintProperty,
    uapsd: BoolProperty,
}

impl BssWmmInfoNode {
    fn new(node: Node, info: ie::ApWmmInfo) -> Self {
        let param_set_count =
            node.create_uint("param_set_count", info.parameter_set_count() as u64);
        let uapsd = node.create_bool("uapsd", info.uapsd());
        Self { _node: node, param_set_count, uapsd }
    }

    fn update(&mut self, info: &ie::ApWmmInfo) {
        self.param_set_count.set(info.parameter_set_count() as u64);
        self.uapsd.set(info.uapsd());
    }
}

pub struct BssWmmAcParamsNode {
    _node: Node,
    aifsn: UintProperty,
    acm: BoolProperty,
    ecw_min: UintProperty,
    ecw_max: UintProperty,
    txop_limit: UintProperty,
}

impl BssWmmAcParamsNode {
    fn new(node: Node, ac_params: ie::WmmAcParams) -> Self {
        let aifsn = node.create_uint("aifsn", ac_params.aci_aifsn.aifsn() as u64);
        let acm = node.create_bool("acm", ac_params.aci_aifsn.acm());
        let ecw_min = node.create_uint("ecw_min", ac_params.ecw_min_max.ecw_min() as u64);
        let ecw_max = node.create_uint("ecw_max", ac_params.ecw_min_max.ecw_max() as u64);
        let txop_limit = node.create_uint("txop_limit", ac_params.txop_limit as u64);
        Self { _node: node, aifsn, acm, ecw_min, ecw_max, txop_limit }
    }

    fn update(&self, ac_params: &ie::WmmAcParams) {
        self.aifsn.set(ac_params.aci_aifsn.aifsn() as u64);
        self.acm.set(ac_params.aci_aifsn.acm());
        self.ecw_min.set(ac_params.ecw_min_max.ecw_min() as u64);
        self.ecw_max.set(ac_params.ecw_min_max.ecw_max() as u64);
        self.txop_limit.set(ac_params.txop_limit as u64);
    }
}

pub struct BssWscNode {
    _node: Node,
    manufacturer: StringProperty,
    model_name: StringProperty,
    model_number: StringProperty,
}

impl BssWscNode {
    fn new(node: Node, wsc: &wsc::ProbeRespWsc) -> Self {
        let manufacturer =
            node.create_string("manufacturer", String::from_utf8_lossy(&wsc.manufacturer[..]));
        let model_name =
            node.create_string("model_name", String::from_utf8_lossy(&wsc.model_name[..]));
        let model_number =
            node.create_string("model_number", String::from_utf8_lossy(&wsc.model_number[..]));

        Self { _node: node, manufacturer, model_name, model_number }
    }

    fn update(&mut self, wsc: &wsc::ProbeRespWsc) {
        self.manufacturer.set(&String::from_utf8_lossy(&wsc.manufacturer[..]));
        self.model_name.set(&String::from_utf8_lossy(&wsc.model_name[..]));
        self.model_number.set(&String::from_utf8_lossy(&wsc.model_number[..]));
    }
}

pub struct ConnectingToNode {
    _node: Node,
    ssid: StringProperty,
}

impl ConnectingToNode {
    fn new(node: Node, ssid: &Ssid) -> Self {
        let ssid = node.create_string("ssid", ssid.to_string());
        Self { _node: node, ssid }
    }

    fn update(&mut self, ssid: &Ssid) {
        self.ssid.set(&ssid.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::test_utils;
    use diagnostics_assertions::{AnyProperty, assert_data_tree};
    use fuchsia_inspect::Inspector;

    #[fuchsia::test]
    async fn test_inspect_update_pulse_connect_disconnect() {
        let inspector = Inspector::default();
        let root = inspector.root();
        let mut pulse = PulseNode::new(root.create_child("last_pulse"));

        // SME is idle. Pulse node should not have any field except "last_updated" and "status"
        let status = ClientSmeStatus::Idle;
        pulse.update(status);
        assert_data_tree!(inspector, root: {
            last_pulse: {
                started: AnyProperty,
                last_updated: AnyProperty,
                status: { status_str: "idle" }
            }
        });

        // SME is connecting. Check that "connecting_to" field now appears, and that existing
        // fields are still kept.
        let status = ClientSmeStatus::Connecting(Ssid::try_from("foo").unwrap());
        pulse.update(status);
        assert_data_tree!(inspector, root: {
            last_pulse: {
                started: AnyProperty,
                last_updated: AnyProperty,
                status: {
                    status_str: "connecting",
                    connecting_to: { ssid: "<ssid-666f6f>" }
                },
            }
        });

        // SME is connected. Aside from verifying that existing fields are kept, key things we
        // want to check are that "last_link_up" and "connected_to" are populated, and
        // "connecting_to" is cleared out.
        let status = ClientSmeStatus::Connected(Box::new(test_utils::fake_serving_ap_info()));
        pulse.update(status);
        assert_data_tree!(inspector, root: {
            last_pulse: {
                started: AnyProperty,
                last_updated: AnyProperty,
                last_link_up: AnyProperty,
                status: {
                    status_str: "connected",
                    connected_to: contains {
                        ssid: "<ssid-666f6f>",
                        bssid: "37:0a:16:03:09:46",
                    },
                },
            }
        });

        // SME is idle. The "connected_to" field is cleared out.
        // The "prev_connected_to" field is logged.
        let status = ClientSmeStatus::Idle;
        pulse.update(status);
        assert_data_tree!(inspector, root: {
            last_pulse: {
                started: AnyProperty,
                last_updated: AnyProperty,
                last_link_up: AnyProperty,
                status: {
                    status_str: "idle",
                    prev_connected_to: contains {
                        ssid: "<ssid-666f6f>",
                        bssid: "37:0a:16:03:09:46",
                    },
                },
            }
        });
    }

    #[fuchsia::test]
    async fn test_inspect_update_pulse_wmm_status_changed() {
        let inspector = Inspector::default();
        let root = inspector.root();
        let mut pulse = PulseNode::new(root.create_child("last_pulse"));

        let mut serving_ap_info = test_utils::fake_serving_ap_info();
        serving_ap_info.wmm_param = None;
        let status = ClientSmeStatus::Connected(Box::new(serving_ap_info.clone()));
        pulse.update(status);
        assert_data_tree!(inspector, root: {
            last_pulse: contains {
                status: contains {
                    connected_to: contains {
                        is_wmm_assoc: false,
                    },
                },
            }
        });

        let mut wmm_param =
            *ie::parse_wmm_param(&test_utils::fake_wmm_param().bytes[..]).expect("parse wmm");
        serving_ap_info.wmm_param = Some(wmm_param);
        let status = ClientSmeStatus::Connected(Box::new(serving_ap_info.clone()));
        pulse.update(status);
        assert_data_tree!(inspector, root: {
            last_pulse: contains {
                status: contains {
                    connected_to: contains {
                        is_wmm_assoc: true,
                        wmm_param: contains {
                            ac_be: {
                                aifsn: 3u64,
                                acm: false,
                                ecw_min: 4u64,
                                ecw_max: 10u64,
                                txop_limit: 0u64,
                            },
                            ac_bk: {
                                aifsn: 7u64,
                                acm: false,
                                ecw_min: 4u64,
                                ecw_max: 10u64,
                                txop_limit: 0u64,
                            },
                            ac_vi: {
                                aifsn: 2u64,
                                acm: false,
                                ecw_min: 3u64,
                                ecw_max: 4u64,
                                txop_limit: 0x5eu64,
                            },
                            ac_vo: {
                                aifsn: 2u64,
                                acm: false,
                                ecw_min: 2u64,
                                ecw_max: 3u64,
                                txop_limit: 0x2fu64,
                            },
                            wmm_info: contains {
                                uapsd: true,
                            },
                        }
                    },
                },
            }
        });

        let mut wmm_info = wmm_param.wmm_info.ap_wmm_info();
        wmm_info.set_uapsd(false);
        wmm_param.wmm_info.0 = wmm_info.0;
        wmm_param.ac_be_params.aci_aifsn.set_aifsn(9);
        wmm_param.ac_bk_params.aci_aifsn.set_acm(true);
        wmm_param.ac_vi_params.ecw_min_max.set_ecw_min(11);
        wmm_param.ac_vi_params.ecw_min_max.set_ecw_max(14);
        wmm_param.ac_vo_params.txop_limit = 0xaa;
        serving_ap_info.wmm_param = Some(wmm_param);
        let status = ClientSmeStatus::Connected(Box::new(serving_ap_info.clone()));
        pulse.update(status);
        assert_data_tree!(inspector, root: {
            last_pulse: contains {
                status: contains {
                    connected_to: contains {
                        is_wmm_assoc: true,
                        wmm_param: contains {
                            ac_be: {
                                aifsn: 9u64,
                                acm: false,
                                ecw_min: 4u64,
                                ecw_max: 10u64,
                                txop_limit: 0u64,
                            },
                            ac_bk: {
                                aifsn: 7u64,
                                acm: true,
                                ecw_min: 4u64,
                                ecw_max: 10u64,
                                txop_limit: 0u64,
                            },
                            ac_vi: {
                                aifsn: 2u64,
                                acm: false,
                                ecw_min: 11u64,
                                ecw_max: 14u64,
                                txop_limit: 0x5eu64,
                            },
                            ac_vo: {
                                aifsn: 2u64,
                                acm: false,
                                ecw_min: 2u64,
                                ecw_max: 3u64,
                                txop_limit: 0xaau64,
                            },
                            wmm_info: contains {
                                uapsd: false,
                            },
                        }
                    },
                },
            }
        });

        serving_ap_info.wmm_param = None;
        let status = ClientSmeStatus::Connected(Box::new(serving_ap_info.clone()));
        pulse.update(status);
        assert_data_tree!(inspector, root: {
            last_pulse: contains {
                status: contains {
                    connected_to: contains {
                        is_wmm_assoc: false,
                    },
                },
            }
        });
    }
}
