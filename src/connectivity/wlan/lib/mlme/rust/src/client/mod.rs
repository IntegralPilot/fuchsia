// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod bound;
mod channel_switch;
mod convert_beacon;
mod lost_bss;
mod scanner;
mod state;
mod station;

use bound::BoundClient;
use station::{Client, ParsedConnectRequest};
#[cfg(test)]
mod test_utils;

use crate::ddk_converter;
use crate::device::{self, DeviceOps};
use crate::error::Error;
use channel_switch::ChannelState;
use fidl_fuchsia_wlan_common as fidl_common;
use fidl_fuchsia_wlan_driver as fidl_driver_common;
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_minstrel as fidl_minstrel;
use fidl_fuchsia_wlan_mlme as fidl_mlme;
use fidl_fuchsia_wlan_softmac as fidl_softmac;
use fidl_fuchsia_wlan_stats as fidl_stats;
use fuchsia_trace as trace;
use ieee80211::{Bssid, MacAddr, MacAddrBytes};
use log::{error, warn};
use scanner::Scanner;
use wlan_common::bss::BssDescription;
use wlan_common::capabilities::{ClientCapabilities, derive_join_capabilities};
use wlan_common::channel::Channel;
use wlan_common::ie::{self, Id};
use wlan_common::mac::{self, CapabilityInfo};
use wlan_common::sequence::SequenceManager;
use wlan_common::timer::Timer;
use wlan_trace as wtrace;
use zerocopy::SplitByteSlice;

pub use scanner::ScanError;

#[derive(Debug, Clone, PartialEq)]
pub enum TimedEvent {
    /// Connecting to AP timed out.
    Connecting,
    /// Timeout for reassociating after a disassociation.
    Reassociating,
    /// Association status update includes checking for auto deauthentication due to beacon loss
    /// and report signal strength
    AssociationStatusCheck,
    /// The delay for a scheduled channel switch has elapsed.
    ChannelSwitch,
}

#[cfg(test)]
impl TimedEvent {
    fn class(&self) -> TimedEventClass {
        match self {
            Self::Connecting => TimedEventClass::Connecting,
            Self::Reassociating => TimedEventClass::Reassociating,
            Self::AssociationStatusCheck => TimedEventClass::AssociationStatusCheck,
            Self::ChannelSwitch => TimedEventClass::ChannelSwitch,
        }
    }
}

#[cfg(test)]
#[derive(Debug, PartialEq, Eq, Hash)]
pub enum TimedEventClass {
    Connecting,
    Reassociating,
    AssociationStatusCheck,
    ChannelSwitch,
}

/// ClientConfig affects time duration used for different timeouts.
/// Originally added to more easily control behavior in tests.
#[repr(C)]
#[derive(Debug, Clone, Default)]
pub struct ClientConfig {
    pub ensure_on_channel_time: zx::sys::zx_duration_t,
}

pub struct Context<D> {
    _config: ClientConfig,
    device: D,
    timer: Timer<TimedEvent>,
    seq_mgr: SequenceManager,
}

pub struct ClientMlme<D> {
    sta: Option<Client>,
    ctx: Context<D>,
    scanner: Scanner,
    channel_state: ChannelState,
}
impl<D: DeviceOps> crate::MlmeImpl for ClientMlme<D> {
    type Config = ClientConfig;
    type Device = D;
    type TimerEvent = TimedEvent;
    async fn new(
        config: Self::Config,
        mut device: Self::Device,
        timer: Timer<TimedEvent>,
    ) -> Result<Self, anyhow::Error> {
        let iface_mac = device::try_query_iface_mac(&mut device).await?;
        Ok(Self {
            sta: None,
            ctx: Context { _config: config, device, timer, seq_mgr: SequenceManager::new() },
            scanner: Scanner::new(iface_mac.into()),
            channel_state: Default::default(),
        })
    }
    async fn handle_mlme_request(
        &mut self,
        req: wlan_sme::MlmeRequest,
    ) -> Result<(), anyhow::Error> {
        match req {
            wlan_sme::MlmeRequest::Scan(req) => {
                self.on_sme_scan(req).await;
                Ok(())
            }
            wlan_sme::MlmeRequest::Connect(req) => {
                self.on_sme_connect(req).await?;
                Ok(())
            }
            wlan_sme::MlmeRequest::GetIfaceStats(responder) => {
                self.on_sme_get_iface_stats(responder)?;
                Ok(())
            }
            wlan_sme::MlmeRequest::GetIfaceHistogramStats(responder) => {
                self.on_sme_get_iface_histogram_stats(responder)?;
                Ok(())
            }
            wlan_sme::MlmeRequest::QueryDeviceInfo(responder) => {
                self.on_sme_query_device_info(responder).await?;
                Ok(())
            }
            wlan_sme::MlmeRequest::QueryMacSublayerSupport(responder) => {
                self.on_sme_query_mac_sublayer_support(responder).await?;
                Ok(())
            }
            wlan_sme::MlmeRequest::QuerySecuritySupport(responder) => {
                self.on_sme_query_security_support(responder).await?;
                Ok(())
            }
            wlan_sme::MlmeRequest::QuerySpectrumManagementSupport(responder) => {
                self.on_sme_query_spectrum_management_support(responder).await?;
                Ok(())
            }
            wlan_sme::MlmeRequest::ListMinstrelPeers(responder) => {
                self.on_sme_list_minstrel_peers(responder)?;
                Ok(())
            }
            wlan_sme::MlmeRequest::GetMinstrelStats(req, responder) => {
                self.on_sme_get_minstrel_stats(responder, &req.peer_addr.into())?;
                Ok(())
            }
            wlan_sme::MlmeRequest::GetSignalReport(responder) if self.sta.is_none() => {
                responder.respond(Ok(fidl_stats::SignalReport::default()));
                Ok(())
            }
            req if self.sta.is_some() => {
                let sta = self.sta.as_mut().unwrap();
                sta.bind(&mut self.ctx, &mut self.scanner, &mut self.channel_state)
                    .handle_mlme_request(req)
                    .await;
                Ok(())
            }
            unhandled_request => {
                if let wlan_sme::MlmeRequest::Reconnect(req) = &unhandled_request {
                    self.ctx.device.send_mlme_event(fidl_mlme::MlmeEvent::ConnectConf {
                        resp: fidl_mlme::ConnectConfirm {
                            peer_sta_address: req.peer_sta_address,
                            result_code: fidl_ieee80211::StatusCode::DeniedNoAssociationExists,
                            association_id: 0,
                            association_ies: vec![],
                        },
                    })?;
                }

                Err(Error::Status(
                    format!(
                        "Failed to handle {} MLME request: request is unhandled in the current state. \
                         Connection context exists: {}, Main channel: {:?}, Scanning: {}.",
                        unhandled_request.name(),
                        self.sta.is_some(),
                        self.channel_state.get_primary(),
                        self.scanner.is_scanning(),
                    ),
                    zx::Status::BAD_STATE,
                ).into())
            }
        }
    }
    async fn handle_mac_frame_rx(
        &mut self,
        bytes: &[u8],
        rx_info: fidl_softmac::WlanRxInfo,
        async_id: trace::Id,
    ) {
        wtrace::duration!("ClientMlme::handle_mac_frame_rx");
        // TODO(https://fxbug.dev/42120906): Send the entire frame to scanner.
        if let Some(mgmt_frame) = mac::MgmtFrame::parse(bytes, false) {
            let bssid = Bssid::from(mgmt_frame.mgmt_hdr.addr3);
            match mgmt_frame.try_into_mgmt_body().1 {
                Some(mac::MgmtBody::Beacon { bcn_hdr, elements }) => {
                    wtrace::duration!("MgmtBody::Beacon");
                    self.scanner.bind(&mut self.ctx).handle_ap_advertisement(
                        bssid,
                        bcn_hdr.beacon_interval,
                        bcn_hdr.capabilities,
                        elements,
                        rx_info.clone(),
                    );
                }
                Some(mac::MgmtBody::ProbeResp { probe_resp_hdr, elements }) => {
                    wtrace::duration!("MgmtBody::ProbeResp");
                    self.scanner.bind(&mut self.ctx).handle_ap_advertisement(
                        bssid,
                        probe_resp_hdr.beacon_interval,
                        probe_resp_hdr.capabilities,
                        elements,
                        rx_info.clone(),
                    )
                }
                _ => (),
            }
        }

        if let Some(sta) = self.sta.as_mut() {
            // Only pass the frame to a BoundClient under the following conditions:
            //   - ChannelState currently has a main channel.
            //   - ClientMlme received the frame on the main channel.
            match self.channel_state.get_primary() {
                Some(main_channel) if main_channel == rx_info.primary => {
                    sta.bind(&mut self.ctx, &mut self.scanner, &mut self.channel_state)
                        .handle_mac_frame_rx(bytes, rx_info, async_id)
                        .await;
                }
                Some(_) => {
                    wtrace::async_end_wlansoftmac_rx(async_id, "off main channel");
                }
                // TODO(https://fxbug.dev/42075118): This is only reachable because the Client state machine
                // returns to the Joined state and clears the main channel upon deauthentication.
                None => {
                    error!(
                        "Received MAC frame on channel {:?} while main channel is not set.",
                        rx_info.primary
                    );
                    wtrace::async_end_wlansoftmac_rx(async_id, "main channel not set");
                }
            }
        } else {
            wtrace::async_end_wlansoftmac_rx(async_id, "no bound client");
        }
    }
    fn handle_eth_frame_tx(
        &mut self,
        bytes: &[u8],
        async_id: trace::Id,
    ) -> Result<(), anyhow::Error> {
        wtrace::duration!("ClientMlme::handle_eth_frame_tx");
        match self.sta.as_mut() {
            None => Err(Error::Status(
                "Ethernet frame dropped (Client does not exist).".to_string(),
                zx::Status::BAD_STATE,
            )
            .into()),
            Some(sta) => sta
                .bind(&mut self.ctx, &mut self.scanner, &mut self.channel_state)
                .handle_eth_frame_tx(bytes, async_id)
                .map_err(From::from),
        }
    }
    async fn handle_scan_complete(&mut self, status: Result<(), zx::Status>, scan_id: u64) {
        self.scanner.bind(&mut self.ctx).handle_scan_complete(status, scan_id).await;
    }
    async fn handle_timeout(&mut self, event: TimedEvent) {
        if let Some(sta) = self.sta.as_mut() {
            let mut bound = sta.bind(&mut self.ctx, &mut self.scanner, &mut self.channel_state);
            bound.sta.state =
                Some(bound.sta.state.take().unwrap().on_timed_event(&mut bound, event).await);
        }
    }
}

impl<D> ClientMlme<D> {
    pub fn seq_mgr(&mut self) -> &mut SequenceManager {
        &mut self.ctx.seq_mgr
    }

    fn on_sme_get_iface_stats(
        &self,
        responder: wlan_sme::responder::Responder<fidl_mlme::GetIfaceStatsResponse>,
    ) -> Result<(), Error> {
        // TODO(https://fxbug.dev/42119762): Implement stats
        let resp = fidl_mlme::GetIfaceStatsResponse::ErrorStatus(zx::sys::ZX_ERR_NOT_SUPPORTED);
        responder.respond(resp);
        Ok(())
    }

    fn on_sme_get_iface_histogram_stats(
        &self,
        responder: wlan_sme::responder::Responder<fidl_mlme::GetIfaceHistogramStatsResponse>,
    ) -> Result<(), Error> {
        // TODO(https://fxbug.dev/42119762): Implement stats
        let resp =
            fidl_mlme::GetIfaceHistogramStatsResponse::ErrorStatus(zx::sys::ZX_ERR_NOT_SUPPORTED);
        responder.respond(resp);
        Ok(())
    }

    fn on_sme_list_minstrel_peers(
        &self,
        responder: wlan_sme::responder::Responder<fidl_mlme::MinstrelListResponse>,
    ) -> Result<(), Error> {
        // TODO(https://fxbug.dev/42159791): Implement once Minstrel is in Rust.
        error!("ListMinstrelPeers is not supported.");
        let peers = fidl_minstrel::Peers { addrs: vec![] };
        let resp = fidl_mlme::MinstrelListResponse { peers };
        responder.respond(resp);
        Ok(())
    }

    fn on_sme_get_minstrel_stats(
        &self,
        responder: wlan_sme::responder::Responder<fidl_mlme::MinstrelStatsResponse>,
        _addr: &MacAddr,
    ) -> Result<(), Error> {
        // TODO(https://fxbug.dev/42159791): Implement once Minstrel is in Rust.
        error!("GetMinstrelStats is not supported.");
        let resp = fidl_mlme::MinstrelStatsResponse { peer: None };
        responder.respond(resp);
        Ok(())
    }
}

impl<D: DeviceOps> ClientMlme<D> {
    pub async fn set_main_channel(
        &mut self,
        primary: fidl_ieee80211::ChannelNumber,
        bandwidth: fidl_ieee80211::ChannelBandwidth,
        vht_secondary_80_channel: fidl_ieee80211::ChannelNumber,
    ) -> Result<(), zx::Status> {
        self.channel_state
            .bind(&mut self.ctx, &mut self.scanner)
            .set_main_channel(primary, bandwidth, vht_secondary_80_channel)
            .await
    }

    async fn on_sme_scan(&mut self, req: fidl_mlme::ScanRequest) {
        let txn_id = req.txn_id;
        let _ = self.scanner.bind(&mut self.ctx).on_sme_scan(req).await.map_err(|e| {
            error!("Scan failed in MLME: {:?}", e);
            let code = match e {
                Error::ScanError(scan_error) => scan_error.into(),
                _ => fidl_mlme::ScanResultCode::InternalError,
            };
            self.ctx
                .device
                .send_mlme_event(fidl_mlme::MlmeEvent::OnScanEnd {
                    end: fidl_mlme::ScanEnd { txn_id, code },
                })
                .unwrap_or_else(|e| error!("error sending MLME ScanEnd: {}", e));
        });
    }

    async fn on_sme_connect(&mut self, req: fidl_mlme::ConnectRequest) -> Result<(), Error> {
        // Cancel any ongoing scan so that it doesn't conflict with the connect request
        // TODO(b/254290448): Use enable/disable scanning for better guarantees.
        if let Err(e) = self.scanner.bind(&mut self.ctx).cancel_ongoing_scan().await {
            warn!("Failed to cancel ongoing scan before connect: {}.", e);
        }

        let bssid = req.selected_bss.bssid;
        let result = match req.selected_bss.try_into() {
            Ok(bss) => {
                let req = ParsedConnectRequest {
                    selected_bss: bss,
                    connect_failure_timeout: req.connect_failure_timeout,
                    auth_type: req.auth_type,
                    security_ie: req.security_ie,
                };
                self.join_device(&req.selected_bss).await.map(|cap| (req, cap))
            }
            Err(e) => Err(Error::Status(
                format!("Error parsing BssDescription: {:?}", e),
                zx::Status::IO_INVALID,
            )),
        };

        match result {
            Ok((req, client_capabilities)) => {
                self.sta.replace(Client::new(
                    req,
                    device::try_query_iface_mac(&mut self.ctx.device).await?,
                    client_capabilities,
                ));
                if let Some(sta) = &mut self.sta {
                    sta.bind(&mut self.ctx, &mut self.scanner, &mut self.channel_state)
                        .start_connecting()
                        .await;
                }
                Ok(())
            }
            Err(e) => {
                error!("Error setting up device for join: {}", e);
                // TODO(https://fxbug.dev/42120718): Only one failure code defined in IEEE 802.11-2016 6.3.4.3
                // Can we do better?
                self.ctx.device.send_mlme_event(fidl_mlme::MlmeEvent::ConnectConf {
                    resp: fidl_mlme::ConnectConfirm {
                        peer_sta_address: bssid,
                        result_code: fidl_ieee80211::StatusCode::JoinFailure,
                        association_id: 0,
                        association_ies: vec![],
                    },
                })?;
                Err(e)
            }
        }
    }

    async fn join_device(&mut self, bss: &BssDescription) -> Result<ClientCapabilities, Error> {
        let info = ddk_converter::mlme_device_info_from_softmac(
            device::try_query(&mut self.ctx.device).await?,
        )?;
        let join_caps = derive_join_capabilities(Channel::from(bss.channel), bss.rates(), &info)
            .map_err(|e| {
                Error::Status(
                    format!("Failed to derive join capabilities: {:?}", e),
                    zx::Status::NOT_SUPPORTED,
                )
            })?;

        let (bandwidth, secondary80_num) = bss.channel.bandwidth.to_fidl();
        let vht_secondary_80_channel =
            fidl_ieee80211::ChannelNumber { band: bss.channel.band, number: secondary80_num };
        self.set_main_channel(bss.channel.into(), bandwidth, vht_secondary_80_channel)
            .await
            .map_err(|status| Error::Status(format!("Error setting device channel"), status))?;

        let join_bss_request = fidl_driver_common::JoinBssRequest {
            bssid: Some(bss.bssid.to_array()),
            bss_type: Some(fidl_ieee80211::BssType::Infrastructure),
            remote: Some(true),
            beacon_period: Some(bss.beacon_period),
            ..Default::default()
        };

        // Configure driver to pass frames from this BSS to MLME. Otherwise they will be dropped.
        self.ctx
            .device
            .join_bss(&join_bss_request)
            .await
            .map(|()| join_caps)
            .map_err(|status| Error::Status(format!("Error setting BSS in driver"), status))
    }

    async fn on_sme_query_device_info(
        &mut self,
        responder: wlan_sme::responder::Responder<fidl_mlme::DeviceInfo>,
    ) -> Result<(), Error> {
        let info = ddk_converter::mlme_device_info_from_softmac(
            device::try_query(&mut self.ctx.device).await?,
        )?;
        responder.respond(info);
        Ok(())
    }

    async fn on_sme_query_mac_sublayer_support(
        &mut self,
        responder: wlan_sme::responder::Responder<fidl_common::MacSublayerSupport>,
    ) -> Result<(), Error> {
        let support = device::try_query_mac_sublayer_support(&mut self.ctx.device).await?;
        responder.respond(support);
        Ok(())
    }

    async fn on_sme_query_security_support(
        &mut self,
        responder: wlan_sme::responder::Responder<fidl_common::SecuritySupport>,
    ) -> Result<(), Error> {
        let support = device::try_query_security_support(&mut self.ctx.device).await?;
        responder.respond(support);
        Ok(())
    }

    async fn on_sme_query_spectrum_management_support(
        &mut self,
        responder: wlan_sme::responder::Responder<fidl_common::SpectrumManagementSupport>,
    ) -> Result<(), Error> {
        let support = device::try_query_spectrum_management_support(&mut self.ctx.device).await?;
        responder.respond(support);
        Ok(())
    }
}

pub struct ParsedAssociateResp {
    pub association_id: u16,
    pub capabilities: CapabilityInfo,
    pub rates: Vec<ie::SupportedRate>,
    pub ht_cap: Option<ie::HtCapabilities>,
    pub vht_cap: Option<ie::VhtCapabilities>,
}

impl ParsedAssociateResp {
    pub fn parse<B: SplitByteSlice>(assoc_resp_frame: &mac::AssocRespFrame<B>) -> Self {
        let mut parsed = ParsedAssociateResp {
            association_id: assoc_resp_frame.assoc_resp_hdr.aid,
            capabilities: assoc_resp_frame.assoc_resp_hdr.capabilities,
            rates: vec![],
            ht_cap: None,
            vht_cap: None,
        };
        for (id, body) in assoc_resp_frame.ies() {
            match id {
                Id::SUPPORTED_RATES => match ie::parse_supported_rates(body) {
                    Err(e) => warn!("invalid Supported Rates: {}", e),
                    Ok(supported_rates) => {
                        // safe to unwrap because supported rate is 1-byte long thus always aligned
                        parsed.rates.extend(supported_rates.iter());
                    }
                },
                Id::EXTENDED_SUPPORTED_RATES => match ie::parse_extended_supported_rates(body) {
                    Err(e) => warn!("invalid Extended Supported Rates: {}", e),
                    Ok(supported_rates) => {
                        // safe to unwrap because supported rate is 1-byte long thus always aligned
                        parsed.rates.extend(supported_rates.iter());
                    }
                },
                Id::HT_CAPABILITIES => match ie::parse_ht_capabilities(body) {
                    Err(e) => warn!("invalid HT Capabilities: {}", e),
                    Ok(ht_cap) => {
                        parsed.ht_cap = Some(*ht_cap);
                    }
                },
                Id::VHT_CAPABILITIES => match ie::parse_vht_capabilities(body) {
                    Err(e) => warn!("invalid VHT Capabilities: {}", e),
                    Ok(vht_cap) => {
                        parsed.vht_cap = Some(*vht_cap);
                    }
                },
                // TODO(https://fxbug.dev/42120297): parse vendor ID and include WMM param if exists
                _ => {}
            }
        }
        parsed
    }
}

#[cfg(test)]
mod tests {
    use super::state::DEFAULT_AUTO_DEAUTH_TIMEOUT_BEACON_COUNT;
    use super::*;
    use crate::MlmeImpl;
    use crate::client::test_utils::*;
    use crate::device::{FakeDevice, LinkStatus, test_utils};
    use crate::test_utils::MockWlanRxInfo;
    use assert_matches::assert_matches;
    use fidl_fuchsia_wlan_common as fidl_common;
    use fidl_fuchsia_wlan_internal as fidl_internal;
    use fidl_fuchsia_wlan_mlme as fidl_mlme;
    use ieee80211::Ssid;
    use wlan_common::channel::Bandwidth;
    use wlan_common::fake_fidl_bss_description;
    use wlan_sme::responder::Responder;

    #[fuchsia::test(allow_stalls = false)]
    async fn spawns_new_sta_on_connect_request_from_sme() {
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;
        assert!(me.get_bound_client().is_none(), "MLME should not contain client, yet");
        me.on_sme_connect(fidl_mlme::ConnectRequest {
            selected_bss: fake_fidl_bss_description!(Open, ssid: Ssid::try_from("foo").unwrap()),
            connect_failure_timeout: 100,
            auth_type: fidl_mlme::AuthenticationTypes::OpenSystem,
            sae_password: vec![],
            wep_key: None,
            security_ie: vec![],
            owe_public_key: None,
        })
        .await
        .expect("valid ConnectRequest should be handled successfully");
        me.get_bound_client().expect("client sta should have been created by now.");
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn fails_to_connect_if_channel_unknown() {
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;
        assert!(me.get_bound_client().is_none(), "MLME should not contain client, yet");
        let mut req = fidl_mlme::ConnectRequest {
            selected_bss: fake_fidl_bss_description!(Open, ssid: Ssid::try_from("foo").unwrap()),
            connect_failure_timeout: 100,
            auth_type: fidl_mlme::AuthenticationTypes::OpenSystem,
            sae_password: vec![],
            wep_key: None,
            security_ie: vec![],
            owe_public_key: None,
        };

        req.selected_bss.bandwidth = fidl_fuchsia_wlan_ieee80211::ChannelBandwidth::unknown();
        me.on_sme_connect(req)
            .await
            .expect_err("ConnectRequest with unknown channel should be rejected");
        assert!(me.get_bound_client().is_none());
    }

    /// Consumes `TimedEvent` values from the `timer::EventStream` held by `mock_objects` and
    /// handles each `TimedEvent` value with `mlme`. This function makes the following assertions:
    ///
    ///   - The `timer::EventStream` held by `mock_objects` starts with one `StatusCheckTimeout`
    ///     pending.
    ///   - For the `beacon_count` specified, `mlme` will consume the current `StatusCheckTimeout`
    ///     and schedule the next.
    ///   - `mlme` produces a `fidl_mlme::SignalReportIndication` for each StatusCheckTimeout
    ///     consumed.
    async fn handle_association_status_checks_and_signal_reports(
        mock_objects: &mut MockObjects,
        mlme: &mut ClientMlme<FakeDevice>,
        beacon_count: u32,
    ) {
        for _ in 0..beacon_count / super::state::ASSOCIATION_STATUS_TIMEOUT_BEACON_COUNT {
            let (_, timed_event, _) = mock_objects
                .time_stream
                .try_next()
                .unwrap()
                .expect("Should have scheduled a timed event");
            mlme.handle_timeout(timed_event.event).await;
            assert_eq!(mock_objects.fake_device_state.lock().wlan_queue.len(), 0);
            mock_objects
                .fake_device_state
                .lock()
                .next_mlme_msg::<fidl_internal::SignalReportIndication>()
                .expect("error reading SignalReport.indication");
        }
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_auto_deauth_uninterrupted_interval() {
        let mut mock_objects = MockObjects::new().await;
        let mut mlme = mock_objects.make_mlme().await;
        mlme.make_client_station();
        let mut client = mlme.get_bound_client().expect("client should be present");

        client.move_to_associated_state();

        // Verify timer is scheduled and move the time to immediately before auto deauth is triggered.
        handle_association_status_checks_and_signal_reports(
            &mut mock_objects,
            &mut mlme,
            DEFAULT_AUTO_DEAUTH_TIMEOUT_BEACON_COUNT,
        )
        .await;

        // One more timeout to trigger the auto deauth
        let (_, timed_event, _) = mock_objects
            .time_stream
            .try_next()
            .unwrap()
            .expect("Should have scheduled a timed event");

        // Verify that triggering event at deadline causes deauth
        mlme.handle_timeout(timed_event.event).await;
        mock_objects
            .fake_device_state
            .lock()
            .next_mlme_msg::<fidl_internal::SignalReportIndication>()
            .expect("error reading SignalReport.indication");
        assert_eq!(mock_objects.fake_device_state.lock().wlan_queue.len(), 1);
        #[rustfmt::skip]
        assert_eq!(&mock_objects.fake_device_state.lock().wlan_queue[0].0[..], &[
            // Mgmt header:
            0b1100_00_00, 0b00000000, // FC
            0, 0, // Duration
            6, 6, 6, 6, 6, 6, // addr1
            7, 7, 7, 7, 7, 7, // addr2
            6, 6, 6, 6, 6, 6, // addr3
            0x10, 0, // Sequence Control
            3, 0, // reason code
        ][..]);
        let deauth_ind = mock_objects
            .fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::DeauthenticateIndication>()
            .expect("error reading DEAUTHENTICATE.indication");
        assert_eq!(
            deauth_ind,
            fidl_mlme::DeauthenticateIndication {
                peer_sta_address: BSSID.to_array(),
                reason_code: fidl_ieee80211::ReasonCode::LeavingNetworkDeauth,
                locally_initiated: true,
            }
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_auto_deauth_received_beacon() {
        let mut mock_objects = MockObjects::new().await;
        let mut mlme = mock_objects.make_mlme().await;
        mlme.make_client_station();
        let mut client = mlme.get_bound_client().expect("client should be present");

        client.move_to_associated_state();

        // Move the countdown to just about to cause auto deauth.
        handle_association_status_checks_and_signal_reports(
            &mut mock_objects,
            &mut mlme,
            DEFAULT_AUTO_DEAUTH_TIMEOUT_BEACON_COUNT,
        )
        .await;

        // Receive beacon midway, so lost bss countdown is reset.
        // If this beacon is not received, the next timeout will trigger auto deauth.
        let main_channel = mlme.channel_state.get_primary().unwrap();
        mlme.handle_mac_frame_rx(
            BEACON_FRAME,
            fidl_softmac::WlanRxInfo {
                rx_flags: fidl_softmac::WlanRxInfoFlags::empty(),
                valid_fields: fidl_softmac::WlanRxInfoValid::empty(),
                phy: fidl_ieee80211::WlanPhyType::Dsss,
                data_rate: 0,
                primary: main_channel,
                mcs: 0,
                rssi_dbm: 0,
                snr_dbh: 0,
                bandwidth: fidl_ieee80211::ChannelBandwidth::Cbw20,
                vht_secondary_80_channel: fidl_ieee80211::ChannelNumber {
                    band: main_channel.band,
                    number: 0,
                },
            },
            0.into(),
        )
        .await;

        // Verify auto deauth is not triggered for the entire duration.
        handle_association_status_checks_and_signal_reports(
            &mut mock_objects,
            &mut mlme,
            DEFAULT_AUTO_DEAUTH_TIMEOUT_BEACON_COUNT,
        )
        .await;

        // Verify more timer is scheduled
        let (_, timed_event2, _) = mock_objects
            .time_stream
            .try_next()
            .unwrap()
            .expect("Should have scheduled a timed event");

        // Verify that triggering event at new deadline causes deauth
        mlme.handle_timeout(timed_event2.event).await;
        mock_objects
            .fake_device_state
            .lock()
            .next_mlme_msg::<fidl_internal::SignalReportIndication>()
            .expect("error reading SignalReport.indication");
        assert_eq!(mock_objects.fake_device_state.lock().wlan_queue.len(), 1);
        #[rustfmt::skip]
        assert_eq!(&mock_objects.fake_device_state.lock().wlan_queue[0].0[..], &[
            // Mgmt header:
            0b1100_00_00, 0b00000000, // FC
            0, 0, // Duration
            6, 6, 6, 6, 6, 6, // addr1
            7, 7, 7, 7, 7, 7, // addr2
            6, 6, 6, 6, 6, 6, // addr3
            0x10, 0, // Sequence Control
            3, 0, // reason code
        ][..]);
        let deauth_ind = mock_objects
            .fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::DeauthenticateIndication>()
            .expect("error reading DEAUTHENTICATE.indication");
        assert_eq!(
            deauth_ind,
            fidl_mlme::DeauthenticateIndication {
                peer_sta_address: BSSID.to_array(),
                reason_code: fidl_ieee80211::ReasonCode::LeavingNetworkDeauth,
                locally_initiated: true,
            }
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn client_send_scan_end_on_mlme_scan_busy() {
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;
        me.make_client_station();

        // Issue a second scan before the first finishes
        me.on_sme_scan(scan_req()).await;
        me.on_sme_scan(fidl_mlme::ScanRequest { txn_id: 1338, ..scan_req() }).await;

        let scan_end = m
            .fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::ScanEnd>()
            .expect("error reading MLME ScanEnd");
        assert_eq!(
            scan_end,
            fidl_mlme::ScanEnd { txn_id: 1338, code: fidl_mlme::ScanResultCode::NotSupported }
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn client_send_scan_end_on_scan_busy() {
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;
        me.make_client_station();

        // Issue a second scan before the first finishes
        me.on_sme_scan(scan_req()).await;
        me.on_sme_scan(fidl_mlme::ScanRequest { txn_id: 1338, ..scan_req() }).await;

        let scan_end = m
            .fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::ScanEnd>()
            .expect("error reading MLME ScanEnd");
        assert_eq!(
            scan_end,
            fidl_mlme::ScanEnd { txn_id: 1338, code: fidl_mlme::ScanResultCode::NotSupported }
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn client_send_scan_end_on_mlme_scan_invalid_args() {
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;

        me.make_client_station();
        me.on_sme_scan(fidl_mlme::ScanRequest {
            txn_id: 1337,
            scan_type: fidl_mlme::ScanTypes::Passive,
            channel_list: vec![], // empty channel list
            ssid_list: vec![Ssid::try_from("ssid").unwrap().into()],
            probe_delay: 0,
            min_channel_time: 100,
            max_channel_time: 300,
        })
        .await;
        let scan_end = m
            .fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::ScanEnd>()
            .expect("error reading MLME ScanEnd");
        assert_eq!(
            scan_end,
            fidl_mlme::ScanEnd { txn_id: 1337, code: fidl_mlme::ScanResultCode::InvalidArgs }
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn client_send_scan_end_on_scan_invalid_args() {
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;

        me.make_client_station();
        me.on_sme_scan(fidl_mlme::ScanRequest {
            txn_id: 1337,
            scan_type: fidl_mlme::ScanTypes::Passive,
            channel_list: vec![fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: 6,
            }],
            ssid_list: vec![Ssid::try_from("ssid").unwrap().into()],
            probe_delay: 0,
            min_channel_time: 300, // min > max
            max_channel_time: 100,
        })
        .await;
        let scan_end = m
            .fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::ScanEnd>()
            .expect("error reading MLME ScanEnd");
        assert_eq!(
            scan_end,
            fidl_mlme::ScanEnd { txn_id: 1337, code: fidl_mlme::ScanResultCode::InvalidArgs }
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn client_send_scan_end_on_passive_scan_fails() {
        let mut m = MockObjects::new().await;
        m.fake_device_state.lock().config.start_passive_scan_fails = true;
        let mut me = m.make_mlme().await;

        me.make_client_station();
        me.on_sme_scan(scan_req()).await;
        let scan_end = m
            .fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::ScanEnd>()
            .expect("error reading MLME ScanEnd");
        assert_eq!(
            scan_end,
            fidl_mlme::ScanEnd { txn_id: 1337, code: fidl_mlme::ScanResultCode::NotSupported }
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn mlme_respond_to_query_device_info() {
        let mut mock_objects = MockObjects::new().await;
        let mut mlme = mock_objects.make_mlme().await;

        let (responder, receiver) = Responder::new();
        mlme.handle_mlme_request(wlan_sme::MlmeRequest::QueryDeviceInfo(responder))
            .await
            .expect("Failed to send MlmeRequest::Connect");
        assert_eq!(
            receiver.await.unwrap(),
            fidl_mlme::DeviceInfo {
                sta_addr: IFACE_MAC.to_array(),
                factory_addr: IFACE_MAC.to_array(),
                role: fidl_common::WlanMacRole::Client,
                bands: test_utils::fake_mlme_band_caps(),
                softmac_hardware_capability: 0,
                qos_capable: false,
            }
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn mlme_respond_to_query_mac_sublayer_support() {
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;

        let (responder, receiver) = Responder::new();
        me.handle_mlme_request(wlan_sme::MlmeRequest::QueryMacSublayerSupport(responder))
            .await
            .expect("Failed to send MlmeRequest::Connect");
        let resp = receiver.await.unwrap();
        assert_eq!(resp.rate_selection_offload.unwrap().supported, Some(false));
        assert_eq!(
            resp.data_plane.unwrap().data_plane_type,
            Some(fidl_common::DataPlaneType::EthernetDevice)
        );
        assert_eq!(resp.device.as_ref().unwrap().is_synthetic, Some(true));
        assert_eq!(
            resp.device.as_ref().unwrap().mac_implementation_type,
            Some(fidl_common::MacImplementationType::Softmac)
        );
        assert_eq!(resp.device.unwrap().tx_status_report_supported, Some(true));
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn mlme_respond_to_query_security_support() {
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;

        let (responder, receiver) = Responder::new();
        assert_matches!(
            me.handle_mlme_request(wlan_sme::MlmeRequest::QuerySecuritySupport(responder)).await,
            Ok(())
        );
        let resp = receiver.await.unwrap();
        assert_eq!(resp.mfp.unwrap().supported, Some(false));
        assert_eq!(resp.sae.as_ref().unwrap().driver_handler_supported, Some(false));
        assert_eq!(resp.sae.unwrap().sme_handler_supported, Some(false));
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn mlme_respond_to_query_spectrum_management_support() {
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;

        let (responder, receiver) = Responder::new();
        me.handle_mlme_request(wlan_sme::MlmeRequest::QuerySpectrumManagementSupport(responder))
            .await
            .expect("Failed to send MlmeRequest::QuerySpectrumManagementSupport");
        assert_eq!(receiver.await.unwrap().dfs.unwrap().supported, Some(true));
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn mlme_connect_unprotected_happy_path() {
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;
        let channel = Channel::new(6, Bandwidth::Cbw40, fidl_ieee80211::WlanBand::TwoGhz);
        let connect_req = fidl_mlme::ConnectRequest {
            selected_bss: fake_fidl_bss_description!(Open,
                ssid: Ssid::try_from("ssid").unwrap().into(),
                bssid: BSSID.to_array(),
                channel: channel.clone(),
            ),
            connect_failure_timeout: 100,
            auth_type: fidl_mlme::AuthenticationTypes::OpenSystem,
            sae_password: vec![],
            wep_key: None,
            security_ie: vec![],
            owe_public_key: None,
        };
        me.handle_mlme_request(wlan_sme::MlmeRequest::Connect(connect_req))
            .await
            .expect("Failed to send MlmeRequest::Connect");

        // Verify an event was queued up in the timer.
        assert_matches!(drain_timeouts(&mut m.time_stream).get(&TimedEventClass::Connecting), Some(ids) => {
            assert_eq!(ids.len(), 1);
        });

        // Verify authentication frame was sent to AP.
        assert_eq!(m.fake_device_state.lock().wlan_queue.len(), 1);
        let (frame, _txflags) = m.fake_device_state.lock().wlan_queue.remove(0);
        #[rustfmt::skip]
        let expected = vec![
            // Mgmt Header:
            0b1011_00_00, 0b00000000, // Frame Control
            0, 0, // Duration
            6, 6, 6, 6, 6, 6, // Addr1
            7, 7, 7, 7, 7, 7, // Addr2
            6, 6, 6, 6, 6, 6, // Addr3
            0x10, 0, // Sequence Control
            // Auth Header:
            0, 0, // Algorithm Number (Open)
            1, 0, // Txn Sequence Number
            0, 0, // Status Code
        ];
        assert_eq!(&frame[..], &expected[..]);

        // Mock auth frame response from the AP
        #[rustfmt::skip]
        let auth_resp_success = vec![
            // Mgmt Header:
            0b1011_00_00, 0b00000000, // Frame Control
            0, 0, // Duration
            7, 7, 7, 7, 7, 7, // Addr1
            7, 7, 7, 7, 7, 7, // Addr2
            6, 6, 6, 6, 6, 6, // Addr3
            0x10, 0, // Sequence Control
            // Auth Header:
            0, 0, // Algorithm Number (Open)
            2, 0, // Txn Sequence Number
            0, 0, // Status Code
        ];
        me.handle_mac_frame_rx(
            &auth_resp_success[..],
            MockWlanRxInfo::with_channel(channel.into()).into(),
            0.into(),
        )
        .await;

        // Verify association request frame was went to AP
        assert_eq!(m.fake_device_state.lock().wlan_queue.len(), 1);
        let (frame, _txflags) = m.fake_device_state.lock().wlan_queue.remove(0);
        #[rustfmt::skip]
        let expected = vec![
            // Mgmt header:
            0, 0, // FC
            0, 0, // Duration
            6, 6, 6, 6, 6, 6, // addr1
            7, 7, 7, 7, 7, 7, // addr2
            6, 6, 6, 6, 6, 6, // addr3
            0x20, 0, // Sequence Control
            // Association Request header:
            0x01, 0x00, // capability info
            0, 0, // listen interval
            // IEs
            0, 4, // SSID id and length
            0x73, 0x73, 0x69, 0x64, // SSID
            1, 8, // supp rates id and length
            2, 4, 11, 22, 12, 18, 24, 36, // supp rates
            50, 4, // ext supp rates and length
            48, 72, 96, 108, // ext supp rates
            45, 26, // HT Cap id and length
            0x63, 0, 0x17, 0xff, 0, 0, 0, // HT Cap \
            0, 0, 0, 0, 0, 0, 0, 0, 1, // HT Cap \
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // HT Cap
        ];
        assert_eq!(&frame[..], &expected[..]);

        // Mock assoc resp frame from the AP
        #[rustfmt::skip]
        let assoc_resp_success = vec![
            // Mgmt Header:
            0b0001_00_00, 0b00000000, // Frame Control
            0, 0, // Duration
            7, 7, 7, 7, 7, 7, // Addr1 == IFACE_MAC
            7, 7, 7, 7, 7, 7, // Addr2
            6, 6, 6, 6, 6, 6, // Addr3
            0x20, 0, // Sequence Control
            // Assoc Resp Header:
            0, 0, // Capabilities
            0, 0, // Status Code
            42, 0, // AID
            // IEs
            // Basic Rates
            0x01, 0x08, 0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24,
            // HT Capabilities
            0x2d, 0x1a, 0xef, 0x09, // HT capabilities info
            0x17, // A-MPDU parameters
            0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            // VHT Capabilities
            0xbf, 0x0c, 0x91, 0x59, 0x82, 0x0f, // VHT capabilities info
            0xea, 0xff, 0x00, 0x00, 0xea, 0xff, 0x00, 0x00, // VHT supported MCS set
        ];
        me.handle_mac_frame_rx(
            &assoc_resp_success[..],
            MockWlanRxInfo::with_channel(channel.into()).into(),
            0.into(),
        )
        .await;

        // Verify a successful connect conf is sent
        let msg = m
            .fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::ConnectConfirm>()
            .expect("expect ConnectConf");
        assert_eq!(
            msg,
            fidl_mlme::ConnectConfirm {
                peer_sta_address: BSSID.to_array(),
                result_code: fidl_ieee80211::StatusCode::Success,
                association_id: 42,
                association_ies: vec![
                    // IEs
                    // Basic Rates
                    0x01, 0x08, 0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24,
                    // HT Capabilities
                    0x2d, 0x1a, 0xef, 0x09, // HT capabilities info
                    0x17, // A-MPDU parameters
                    0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                    0x00, 0x00, // VHT Capabilities
                    0xbf, 0x0c, 0x91, 0x59, 0x82, 0x0f, // VHT capabilities info
                    0xea, 0xff, 0x00, 0x00, 0xea, 0xff, 0x00, 0x00, // VHT supported MCS set
                ],
            }
        );

        // Verify eth link is up
        assert_eq!(m.fake_device_state.lock().link_status, LinkStatus::UP);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn mlme_connect_protected_happy_path() {
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;
        let channel = Channel::new(6, Bandwidth::Cbw40, fidl_ieee80211::WlanBand::TwoGhz);
        let connect_req = fidl_mlme::ConnectRequest {
            selected_bss: fake_fidl_bss_description!(Wpa2,
                ssid: Ssid::try_from("ssid").unwrap().into(),
                bssid: BSSID.to_array(),
                channel: channel.clone(),
            ),
            connect_failure_timeout: 100,
            auth_type: fidl_mlme::AuthenticationTypes::OpenSystem,
            sae_password: vec![],
            wep_key: None,
            security_ie: vec![
                48, 18, // RSNE header
                1, 0, // Version
                0x00, 0x0F, 0xAC, 4, // Group Cipher: CCMP-128
                1, 0, 0x00, 0x0F, 0xAC, 4, // 1 Pairwise Cipher: CCMP-128
                1, 0, 0x00, 0x0F, 0xAC, 2, // 1 AKM: PSK
            ],
            owe_public_key: None,
        };
        me.handle_mlme_request(wlan_sme::MlmeRequest::Connect(connect_req))
            .await
            .expect("Failed to send MlmeRequest::Connect");

        // Verify an event was queued up in the timer.
        assert_matches!(drain_timeouts(&mut m.time_stream).get(&TimedEventClass::Connecting), Some(ids) => {
            assert_eq!(ids.len(), 1);
        });

        // Verify authentication frame was sent to AP.
        assert_eq!(m.fake_device_state.lock().wlan_queue.len(), 1);
        let (frame, _txflags) = m.fake_device_state.lock().wlan_queue.remove(0);
        #[rustfmt::skip]
        let expected = vec![
            // Mgmt Header:
            0b1011_00_00, 0b00000000, // Frame Control
            0, 0, // Duration
            6, 6, 6, 6, 6, 6, // Addr1
            7, 7, 7, 7, 7, 7, // Addr2
            6, 6, 6, 6, 6, 6, // Addr3
            0x10, 0, // Sequence Control
            // Auth Header:
            0, 0, // Algorithm Number (Open)
            1, 0, // Txn Sequence Number
            0, 0, // Status Code
        ];
        assert_eq!(&frame[..], &expected[..]);

        // Mock auth frame response from the AP
        #[rustfmt::skip]
        let auth_resp_success = vec![
            // Mgmt Header:
            0b1011_00_00, 0b00000000, // Frame Control
            0, 0, // Duration
            7, 7, 7, 7, 7, 7, // Addr1
            7, 7, 7, 7, 7, 7, // Addr2
            6, 6, 6, 6, 6, 6, // Addr3
            0x10, 0, // Sequence Control
            // Auth Header:
            0, 0, // Algorithm Number (Open)
            2, 0, // Txn Sequence Number
            0, 0, // Status Code
        ];
        me.handle_mac_frame_rx(
            &auth_resp_success[..],
            MockWlanRxInfo::with_channel(channel.into()).into(),
            0.into(),
        )
        .await;

        // Verify association request frame was went to AP
        assert_eq!(m.fake_device_state.lock().wlan_queue.len(), 1);
        let (frame, _txflags) = m.fake_device_state.lock().wlan_queue.remove(0);
        #[rustfmt::skip]
        let expected = vec![
            // Mgmt header:
            0, 0, // FC
            0, 0, // Duration
            6, 6, 6, 6, 6, 6, // addr1
            7, 7, 7, 7, 7, 7, // addr2
            6, 6, 6, 6, 6, 6, // addr3
            0x20, 0, // Sequence Control
            // Association Request header:
            0x01, 0x00, // capability info
            0, 0, // listen interval
            // IEs
            0, 4, // SSID id and length
            0x73, 0x73, 0x69, 0x64, // SSID
            1, 8, // supp rates id and length
            2, 4, 11, 22, 12, 18, 24, 36, // supp rates
            50, 4, // ext supp rates and length
            48, 72, 96, 108, // ext supp rates
            48, 18, // RSNE id and length
            1, 0, // RSN \
            0x00, 0x0F, 0xAC, 4, // RSN \
            1, 0, 0x00, 0x0F, 0xAC, 4, // RSN \
            1, 0, 0x00, 0x0F, 0xAC, 2, // RSN
            45, 26, // HT Cap id and length
            0x63, 0, 0x17, 0xff, 0, 0, 0, // HT Cap \
            0, 0, 0, 0, 0, 0, 0, 0, 1, // HT Cap \
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // HT Cap
        ];
        assert_eq!(&frame[..], &expected[..]);

        // Mock assoc resp frame from the AP
        #[rustfmt::skip]
        let assoc_resp_success = vec![
            // Mgmt Header:
            0b0001_00_00, 0b00000000, // Frame Control
            0, 0, // Duration
            7, 7, 7, 7, 7, 7, // Addr1 == IFACE_MAC
            7, 7, 7, 7, 7, 7, // Addr2
            6, 6, 6, 6, 6, 6, // Addr3
            0x20, 0, // Sequence Control
            // Assoc Resp Header:
            0, 0, // Capabilities
            0, 0, // Status Code
            42, 0, // AID
            // IEs
            // Basic Rates
            0x01, 0x08, 0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24,
            // RSN
            0x30, 18, 1, 0, // RSN header and version
            0x00, 0x0F, 0xAC, 4, // Group Cipher: CCMP-128
            1, 0, 0x00, 0x0F, 0xAC, 4, // 1 Pairwise Cipher: CCMP-128
            1, 0, 0x00, 0x0F, 0xAC, 2, // 1 AKM: PSK
            // HT Capabilities
            0x2d, 0x1a, 0xef, 0x09, // HT capabilities info
            0x17, // A-MPDU parameters
            0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // Other HT Cap fields
            // VHT Capabilities
            0xbf, 0x0c, 0x91, 0x59, 0x82, 0x0f, // VHT capabilities info
            0xea, 0xff, 0x00, 0x00, 0xea, 0xff, 0x00, 0x00, // VHT supported MCS set
        ];
        me.handle_mac_frame_rx(
            &assoc_resp_success[..],
            MockWlanRxInfo::with_channel(channel.into()).into(),
            0.into(),
        )
        .await;

        // Verify a successful connect conf is sent
        let msg = m
            .fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::ConnectConfirm>()
            .expect("expect ConnectConf");
        assert_eq!(
            msg,
            fidl_mlme::ConnectConfirm {
                peer_sta_address: BSSID.to_array(),
                result_code: fidl_ieee80211::StatusCode::Success,
                association_id: 42,
                association_ies: vec![
                    // IEs
                    // Basic Rates
                    0x01, 0x08, 0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24, // RSN
                    0x30, 18, 1, 0, // RSN header and version
                    0x00, 0x0F, 0xAC, 4, // Group Cipher: CCMP-128
                    1, 0, 0x00, 0x0F, 0xAC, 4, // 1 Pairwise Cipher: CCMP-128
                    1, 0, 0x00, 0x0F, 0xAC, 2, // 1 AKM: PSK
                    // HT Capabilities
                    0x2d, 0x1a, 0xef, 0x09, // HT capabilities info
                    0x17, // A-MPDU parameters
                    0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                    0x00, 0x00, // Other HT Cap fields
                    // VHT Capabilities
                    0xbf, 0x0c, 0x91, 0x59, 0x82, 0x0f, // VHT capabilities info
                    0xea, 0xff, 0x00, 0x00, 0xea, 0xff, 0x00, 0x00, // VHT supported MCS set
                ],
            }
        );

        // Verify that link is still down
        assert_eq!(m.fake_device_state.lock().link_status, LinkStatus::DOWN);

        // Send a request to open controlled port
        me.handle_mlme_request(wlan_sme::MlmeRequest::SetCtrlPort(
            fidl_mlme::SetControlledPortRequest {
                peer_sta_address: BSSID.to_array(),
                state: fidl_mlme::ControlledPortState::Open,
            },
        ))
        .await
        .expect("expect sending msg to succeed");

        // Verify that link is now up
        assert_eq!(m.fake_device_state.lock().link_status, LinkStatus::UP);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn mlme_connect_vht() {
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;
        let channel = Channel::new(36, Bandwidth::Cbw40, fidl_ieee80211::WlanBand::FiveGhz);
        let connect_req = fidl_mlme::ConnectRequest {
            selected_bss: fake_fidl_bss_description!(Open,
                ssid: Ssid::try_from("ssid").unwrap().into(),
                bssid: BSSID.to_array(),
                channel: channel.clone(),
            ),
            connect_failure_timeout: 100,
            auth_type: fidl_mlme::AuthenticationTypes::OpenSystem,
            sae_password: vec![],
            wep_key: None,
            security_ie: vec![],
            owe_public_key: None,
        };
        me.handle_mlme_request(wlan_sme::MlmeRequest::Connect(connect_req))
            .await
            .expect("Failed to send MlmeRequest::Connect.");

        // Verify an event was queued up in the timer.
        assert_matches!(drain_timeouts(&mut m.time_stream).get(&TimedEventClass::Connecting), Some(ids) => {
            assert_eq!(ids.len(), 1);
        });

        // Auth frame
        assert_eq!(m.fake_device_state.lock().wlan_queue.len(), 1);
        let (_frame, _txflags) = m.fake_device_state.lock().wlan_queue.remove(0);

        // Mock auth frame response from the AP
        #[rustfmt::skip]
        let auth_resp_success = vec![
            // Mgmt Header:
            0b1011_00_00, 0b00000000, // Frame Control
            0, 0, // Duration
            7, 7, 7, 7, 7, 7, // Addr1
            7, 7, 7, 7, 7, 7, // Addr2
            6, 6, 6, 6, 6, 6, // Addr3
            0x10, 0, // Sequence Control
            // Auth Header:
            0, 0, // Algorithm Number (Open)
            2, 0, // Txn Sequence Number
            0, 0, // Status Code
        ];
        me.handle_mac_frame_rx(
            &auth_resp_success[..],
            MockWlanRxInfo::with_channel(channel.into()).into(),
            0.into(),
        )
        .await;

        // Verify association request frame was went to AP
        assert_eq!(m.fake_device_state.lock().wlan_queue.len(), 1);
        let (frame, _txflags) = m.fake_device_state.lock().wlan_queue.remove(0);
        #[rustfmt::skip]
        let expected = vec![
            // Mgmt header:
            0, 0, // FC
            0, 0, // Duration
            6, 6, 6, 6, 6, 6, // addr1
            7, 7, 7, 7, 7, 7, // addr2
            6, 6, 6, 6, 6, 6, // addr3
            0x20, 0, // Sequence Control
            // Association Request header:
            0x01, 0x00, // capability info
            0, 0, // listen interval
            // IEs
            0, 4, // SSID id and length
            0x73, 0x73, 0x69, 0x64, // SSID
            1, 6, // supp rates id and length
            2, 4, 11, 22, 48, 96, // supp rates
            45, 26, // HT Cap id and length
            0x63, 0, 0x17, 0xff, 0, 0, 0, // HT Cap \
            0, 0, 0, 0, 0, 0, 0, 0, 1, // HT Cap \
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // HT Cap
            191, 12, // VHT Cap id and length
            50, 80, 128, 15, 254, 255, 0, 0, 254, 255, 0, 0, // VHT Cap
        ];
        assert_eq!(&frame[..], &expected[..]);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn mlme_connect_timeout() {
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;
        let connect_req = fidl_mlme::ConnectRequest {
            selected_bss: fake_fidl_bss_description!(Open, bssid: BSSID.to_array()),
            connect_failure_timeout: 100,
            auth_type: fidl_mlme::AuthenticationTypes::OpenSystem,
            sae_password: vec![],
            wep_key: None,
            security_ie: vec![],
            owe_public_key: None,
        };
        me.handle_mlme_request(wlan_sme::MlmeRequest::Connect(connect_req))
            .await
            .expect("Failed to send MlmeRequest::Connect.");

        // Verify an event was queued up in the timer.
        let (event, _id) = assert_matches!(drain_timeouts(&mut m.time_stream).get(&TimedEventClass::Connecting), Some(events) => {
            assert_eq!(events.len(), 1);
            events[0].clone()
        });

        // Quick check that a frame was sent (this is authentication frame).
        assert_eq!(m.fake_device_state.lock().wlan_queue.len(), 1);
        let (_frame, _txflags) = m.fake_device_state.lock().wlan_queue.remove(0);

        // Send connect timeout
        me.handle_timeout(event).await;

        // Verify a connect confirm message was sent
        let msg = m
            .fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::ConnectConfirm>()
            .expect("expect msg");
        assert_eq!(
            msg,
            fidl_mlme::ConnectConfirm {
                peer_sta_address: BSSID.to_array(),
                result_code: fidl_ieee80211::StatusCode::RejectedSequenceTimeout,
                association_id: 0,
                association_ies: vec![],
            },
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn mlme_reconnect_no_sta() {
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;

        let reconnect_req = fidl_mlme::ReconnectRequest { peer_sta_address: [1, 2, 3, 4, 5, 6] };
        let result = me.handle_mlme_request(wlan_sme::MlmeRequest::Reconnect(reconnect_req)).await;
        let err = result.unwrap_err();
        let mlme_err = err.downcast_ref::<Error>().expect("expected Mlme Error");
        assert_matches!(mlme_err, Error::Status(_, zx::Status::BAD_STATE));

        // Verify a connect confirm message was sent
        let msg = m
            .fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::ConnectConfirm>()
            .expect("expect msg");
        assert_eq!(
            msg,
            fidl_mlme::ConnectConfirm {
                peer_sta_address: [1, 2, 3, 4, 5, 6],
                result_code: fidl_ieee80211::StatusCode::DeniedNoAssociationExists,
                association_id: 0,
                association_ies: vec![],
            },
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn mlme_respond_to_get_iface_stats_with_error_status() {
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;

        let (responder, receiver) = Responder::new();
        me.handle_mlme_request(wlan_sme::MlmeRequest::GetIfaceStats(responder))
            .await
            .expect("Failed to send MlmeRequest::GetIfaceStats.");
        assert_eq!(
            receiver.await,
            Ok(fidl_mlme::GetIfaceStatsResponse::ErrorStatus(zx::sys::ZX_ERR_NOT_SUPPORTED))
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn mlme_respond_to_get_iface_histogram_stats_with_error_status() {
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;

        let (responder, receiver) = Responder::new();
        me.handle_mlme_request(wlan_sme::MlmeRequest::GetIfaceHistogramStats(responder))
            .await
            .expect("Failed to send MlmeRequest::GetIfaceHistogramStats");
        assert_eq!(
            receiver.await,
            Ok(fidl_mlme::GetIfaceHistogramStatsResponse::ErrorStatus(
                zx::sys::ZX_ERR_NOT_SUPPORTED
            ))
        );
    }

    #[test]
    fn drop_mgmt_frame_wrong_bssid() {
        let frame = [
            // Mgmt header 1101 for action frame
            0b11010000, 0b00000000, // frame control
            0, 0, // duration
            7, 7, 7, 7, 7, 7, // addr1
            6, 6, 6, 6, 6, 6, // addr2
            0, 0, 0, 0, 0, 0, // addr3 (bssid should have been [6; 6])
            0x10, 0, // sequence control
        ];
        let frame = mac::MacFrame::parse(&frame[..], false).unwrap();
        assert_eq!(false, make_client_station().should_handle_frame(&frame));
    }

    #[test]
    fn drop_mgmt_frame_wrong_dst_addr() {
        let frame = [
            // Mgmt header 1101 for action frame
            0b11010000, 0b00000000, // frame control
            0, 0, // duration
            0, 0, 0, 0, 0, 0, // addr1 (dst_addr should have been [7; 6])
            6, 6, 6, 6, 6, 6, // addr2
            6, 6, 6, 6, 6, 6, // addr3
            0x10, 0, // sequence control
        ];
        let frame = mac::MacFrame::parse(&frame[..], false).unwrap();
        assert_eq!(false, make_client_station().should_handle_frame(&frame));
    }

    #[test]
    fn mgmt_frame_ok_broadcast() {
        let frame = [
            // Mgmt header 1101 for action frame
            0b11010000, 0b00000000, // frame control
            0, 0, // duration
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, // addr1 (dst_addr is broadcast)
            6, 6, 6, 6, 6, 6, // addr2
            6, 6, 6, 6, 6, 6, // addr3
            0x10, 0, // sequence control
        ];
        let frame = mac::MacFrame::parse(&frame[..], false).unwrap();
        assert_eq!(true, make_client_station().should_handle_frame(&frame));
    }

    #[test]
    fn mgmt_frame_ok_client_addr() {
        let frame = [
            // Mgmt header 1101 for action frame
            0b11010000, 0b00000000, // frame control
            0, 0, // duration
            7, 7, 7, 7, 7, 7, // addr1 (dst_addr should have been [7; 6])
            6, 6, 6, 6, 6, 6, // addr2
            6, 6, 6, 6, 6, 6, // addr3
            0x10, 0, // sequence control
        ];
        let frame = mac::MacFrame::parse(&frame[..], false).unwrap();
        assert_eq!(true, make_client_station().should_handle_frame(&frame));
    }

    #[test]
    fn drop_data_frame_wrong_bssid() {
        let frame = [
            // Data header 0100
            0b01001000,
            0b00000010, // frame control. right 2 bits of octet 2: from_ds(1), to_ds(0)
            0, 0, // duration
            7, 7, 7, 7, 7, 7, // addr1 (dst_addr)
            0, 0, 0, 0, 0, 0, // addr2 (bssid should have been [6; 6])
            6, 6, 6, 6, 6, 6, // addr3
            0x10, 0, // sequence control
        ];
        let frame = mac::MacFrame::parse(&frame[..], false).unwrap();
        assert_eq!(false, make_client_station().should_handle_frame(&frame));
    }

    #[test]
    fn drop_data_frame_wrong_dst_addr() {
        let frame = [
            // Data header 0100
            0b01001000,
            0b00000010, // frame control. right 2 bits of octet 2: from_ds(1), to_ds(0)
            0, 0, // duration
            0, 0, 0, 0, 0, 0, // addr1 (dst_addr should have been [7; 6])
            6, 6, 6, 6, 6, 6, // addr2 (bssid)
            6, 6, 6, 6, 6, 6, // addr3
            0x10, 0, // sequence control
        ];
        let frame = mac::MacFrame::parse(&frame[..], false).unwrap();
        assert_eq!(false, make_client_station().should_handle_frame(&frame));
    }

    #[test]
    fn data_frame_ok_broadcast() {
        let frame = [
            // Data header 0100
            0b01001000,
            0b00000010, // frame control. right 2 bits of octet 2: from_ds(1), to_ds(0)
            0, 0, // duration
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, // addr1 (dst_addr is broadcast)
            6, 6, 6, 6, 6, 6, // addr2 (bssid)
            6, 6, 6, 6, 6, 6, // addr3
            0x10, 0, // sequence control
        ];
        let frame = mac::MacFrame::parse(&frame[..], false).unwrap();
        assert_eq!(true, make_client_station().should_handle_frame(&frame));
    }

    #[test]
    fn data_frame_ok_client_addr() {
        let frame = [
            // Data header 0100
            0b01001000,
            0b00000010, // frame control. right 2 bits of octet 2: from_ds(1), to_ds(0)
            0, 0, // duration
            7, 7, 7, 7, 7, 7, // addr1 (dst_addr)
            6, 6, 6, 6, 6, 6, // addr2 (bssid)
            6, 6, 6, 6, 6, 6, // addr3
            0x10, 0, // sequence control
        ];
        let frame = mac::MacFrame::parse(&frame[..], false).unwrap();
        assert_eq!(true, make_client_station().should_handle_frame(&frame));
    }
}
