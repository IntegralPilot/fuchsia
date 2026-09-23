// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::client::Context;
use crate::client::convert_beacon::construct_bss_description;
use crate::ddk_converter::cssid_from_ssid_unchecked;
use crate::device::{self, DeviceOps};
use crate::error::Error;
use anyhow::format_err;
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_mlme as fidl_mlme;
use fidl_fuchsia_wlan_softmac as fidl_softmac;
use fuchsia_trace as trace;
use ieee80211::{Bssid, MacAddr};
use log::{error, warn};
use thiserror::Error;
use wlan_common::mac::{self, CapabilityInfo};
use wlan_common::mgmt_writer;
use wlan_common::time::TimeUnit;
use wlan_frame_writer::write_frame_to_vec;
use wlan_trace as wtrace;

// TODO(https://fxbug.dev/42171393): Currently hardcoded until parameters supported.
const MIN_HOME_TIME: zx::MonotonicDuration = zx::MonotonicDuration::from_millis(0);
const MIN_PROBES_PER_CHANNEL: u8 = 0;
const MAX_PROBES_PER_CHANNEL: u8 = 0;

#[derive(Error, Debug, PartialEq, Eq)]
pub enum ScanError {
    #[error("scanner is busy")]
    Busy,
    #[error("invalid arg: empty channel list")]
    EmptyChannelList,
    #[error("invalid arg: max_channel_time < min_channel_time")]
    MaxChannelTimeLtMin,
    #[error("fail starting device scan: {}", _0)]
    StartOffloadScanFails(zx::Status),
    #[error("invalid response")]
    InvalidResponse,
}

impl From<ScanError> for zx::Status {
    fn from(e: ScanError) -> Self {
        match e {
            ScanError::Busy => zx::Status::UNAVAILABLE,
            ScanError::EmptyChannelList | ScanError::MaxChannelTimeLtMin => {
                zx::Status::INVALID_ARGS
            }
            ScanError::StartOffloadScanFails(status) => status,
            ScanError::InvalidResponse => zx::Status::INVALID_ARGS,
        }
    }
}

impl From<ScanError> for fidl_mlme::ScanResultCode {
    fn from(e: ScanError) -> Self {
        match e {
            ScanError::Busy => fidl_mlme::ScanResultCode::NotSupported,
            ScanError::EmptyChannelList | ScanError::MaxChannelTimeLtMin => {
                fidl_mlme::ScanResultCode::InvalidArgs
            }
            ScanError::StartOffloadScanFails(zx::Status::NOT_SUPPORTED) => {
                fidl_mlme::ScanResultCode::NotSupported
            }
            ScanError::StartOffloadScanFails(..) => fidl_mlme::ScanResultCode::InternalError,
            ScanError::InvalidResponse => fidl_mlme::ScanResultCode::InternalError,
        }
    }
}

pub struct Scanner {
    ongoing_scan: Option<OngoingScan>,
    /// MAC address of current client interface
    iface_mac: MacAddr,
    scanning_enabled: bool,
}

impl Scanner {
    pub fn new(iface_mac: MacAddr) -> Self {
        Self { ongoing_scan: None, iface_mac, scanning_enabled: true }
    }

    pub fn bind<'a, D>(&'a mut self, ctx: &'a mut Context<D>) -> BoundScanner<'a, D> {
        BoundScanner { scanner: self, ctx }
    }

    pub fn is_scanning(&self) -> bool {
        self.ongoing_scan.is_some()
    }
}

pub struct BoundScanner<'a, D> {
    scanner: &'a mut Scanner,
    ctx: &'a mut Context<D>,
}

enum OngoingScan {
    PassiveOffloadScan {
        /// Scan txn_id that's currently being serviced.
        mlme_txn_id: u64,
        /// Unique identifier returned from the device driver when the scan began.
        in_progress_device_scan_id: u64,
    },
    ActiveOffloadScan {
        /// Scan txn_id that's currently being serviced.
        mlme_txn_id: u64,
        /// Unique identifier returned from the device driver when the scan began.
        in_progress_device_scan_id: u64,
        /// Remaining arguments to be sent to future scan requests.
        remaining_active_scan_requests: Vec<fidl_softmac::WlanSoftmacStartActiveScanRequest>,
    },
}

impl OngoingScan {
    fn scan_id(&self) -> u64 {
        match self {
            Self::PassiveOffloadScan { in_progress_device_scan_id, .. } => {
                *in_progress_device_scan_id
            }
            Self::ActiveOffloadScan { in_progress_device_scan_id, .. } => {
                *in_progress_device_scan_id
            }
        }
    }
}

impl<'a, D: DeviceOps> BoundScanner<'a, D> {
    /// Temporarily disable scanning. If scan cancellation is supported, any
    /// ongoing scan will be cancelled when scanning is disabled. If a scan
    /// is in progress but cannot be cancelled, this function returns
    /// zx::Status::NOT_SUPPORTED and makes no changes to the system.
    pub async fn disable_scanning(&mut self) -> Result<(), zx::Status> {
        if self.scanner.scanning_enabled {
            self.cancel_ongoing_scan().await?;
            self.scanner.scanning_enabled = false;
        }
        Ok(())
    }

    pub fn enable_scanning(&mut self) {
        self.scanner.scanning_enabled = true;
    }

    /// Canceling any software scan that's in progress
    /// TODO(b/254290448): Remove 'pub' when all clients use enable/disable scanning.
    pub async fn cancel_ongoing_scan(&mut self) -> Result<(), zx::Status> {
        if let Some(scan) = &self.scanner.ongoing_scan {
            let discovery_support = self.ctx.device.discovery_support().await?;
            if discovery_support
                .scan_offload
                .and_then(|offload| offload.scan_cancel_supported)
                .unwrap_or(false)
            {
                self.ctx
                    .device
                    .cancel_scan(&fidl_softmac::WlanSoftmacBaseCancelScanRequest {
                        scan_id: Some(scan.scan_id()),
                        ..Default::default()
                    })
                    .await
            } else {
                Err(zx::Status::NOT_SUPPORTED)
            }
        } else {
            Ok(())
        }
    }

    /// Handle scan request. Queue requested scan channels in channel scheduler.
    ///
    /// If a scan request is in progress, or the new request has invalid argument (empty channel
    /// list or larger min channel time than max), then the request is rejected.
    pub async fn on_sme_scan(&'a mut self, req: fidl_mlme::ScanRequest) -> Result<(), Error> {
        if self.scanner.ongoing_scan.is_some() || !self.scanner.scanning_enabled {
            return Err(Error::ScanError(ScanError::Busy));
        }
        if req.channel_list.is_empty() {
            return Err(Error::ScanError(ScanError::EmptyChannelList));
        }
        if req.max_channel_time < req.min_channel_time {
            return Err(Error::ScanError(ScanError::MaxChannelTimeLtMin));
        }

        let query_response = self
            .ctx
            .device
            .wlan_softmac_query_response()
            .await
            .map_err(|status| Error::Status(String::from("Failed to query device."), status))?;
        let discovery_support = device::try_query_discovery_support(&mut self.ctx.device).await?;

        // TODO(https://fxbug.dev/321627682): MLME only supports offloaded scanning.
        if discovery_support.scan_offload.and_then(|offload| offload.supported).unwrap_or(false) {
            match req.scan_type {
                fidl_mlme::ScanTypes::Passive => self.start_passive_scan(req).await,
                fidl_mlme::ScanTypes::Active => self.start_active_scan(req, &query_response).await,
            }
            .map(|ongoing_scan| self.scanner.ongoing_scan = Some(ongoing_scan))
            .map_err(|e| {
                self.scanner.ongoing_scan.take();
                e
            })
        } else {
            Err(Error::ScanError(ScanError::StartOffloadScanFails(zx::Status::NOT_SUPPORTED)))
        }
    }

    async fn start_passive_scan(
        &mut self,
        req: fidl_mlme::ScanRequest,
    ) -> Result<OngoingScan, Error> {
        Ok(OngoingScan::PassiveOffloadScan {
            mlme_txn_id: req.txn_id,
            in_progress_device_scan_id: self
                .ctx
                .device
                .start_passive_scan(&fidl_softmac::WlanSoftmacBaseStartPassiveScanRequest {
                    channels: Some(req.channel_list),
                    // TODO(https://fxbug.dev/42171328): A TimeUnit is generally limited to 2 octets. Conversion here
                    // is required since fuchsia.wlan.mlme/ScanRequest.min_channel_time has a width of
                    // four octets.
                    min_channel_time: Some(
                        zx::MonotonicDuration::from(TimeUnit(req.min_channel_time as u16))
                            .into_nanos(),
                    ),
                    max_channel_time: Some(
                        zx::MonotonicDuration::from(TimeUnit(req.max_channel_time as u16))
                            .into_nanos(),
                    ),
                    min_home_time: Some(MIN_HOME_TIME.into_nanos()),
                    ..Default::default()
                })
                .await
                .map_err(|status| Error::ScanError(ScanError::StartOffloadScanFails(status)))?
                .scan_id
                .ok_or(Error::ScanError(ScanError::InvalidResponse))?,
        })
    }

    async fn start_active_scan(
        &mut self,
        req: fidl_mlme::ScanRequest,
        query_response: &fidl_softmac::WlanSoftmacQueryResponse,
    ) -> Result<OngoingScan, Error> {
        let ssids_list = req.ssid_list.iter().map(cssid_from_ssid_unchecked).collect::<Vec<_>>();

        let mac_header = write_frame_to_vec!({
            headers: {
                mac::MgmtHdr: &self.probe_request_mac_header(),
            },
        })?;

        let mut remaining_active_scan_requests = active_scan_request_series(
            // TODO(https://fxbug.dev/42171328): A TimeUnit is generally limited to 2 octets. Conversion here
            // is required since fuchsia.wlan.mlme/ScanRequest.min_channel_time has a width of
            // four octets.
            query_response,
            req.channel_list,
            ssids_list,
            mac_header,
            zx::MonotonicDuration::from(TimeUnit(req.min_channel_time as u16)).into_nanos(),
            zx::MonotonicDuration::from(TimeUnit(req.max_channel_time as u16)).into_nanos(),
            MIN_HOME_TIME.into_nanos(),
            MIN_PROBES_PER_CHANNEL,
            MAX_PROBES_PER_CHANNEL,
        )?;

        match remaining_active_scan_requests.pop() {
            None => {
                error!("unexpected empty list of active scan args");
                return Err(Error::ScanError(ScanError::StartOffloadScanFails(
                    zx::Status::INVALID_ARGS,
                )));
            }
            Some(active_scan_request) => Ok(OngoingScan::ActiveOffloadScan {
                mlme_txn_id: req.txn_id,
                in_progress_device_scan_id: self
                    .start_next_active_scan(&active_scan_request)
                    .await
                    .map_err(|scan_error| Error::ScanError(scan_error))?,
                remaining_active_scan_requests,
            }),
        }
    }

    async fn start_next_active_scan(
        &mut self,
        request: &fidl_softmac::WlanSoftmacStartActiveScanRequest,
    ) -> Result<u64, ScanError> {
        match self.ctx.device.start_active_scan(request).await {
            Ok(response) => Ok(response.scan_id.ok_or_else(|| {
                error!("Active scan response missing scan id!");
                ScanError::InvalidResponse
            })?),
            Err(status) => Err(ScanError::StartOffloadScanFails(status)),
        }
    }

    /// Called when MLME receives an advertisement from the AP, e.g. a Beacon or Probe
    /// Response frame. If a scan is in progress, then the advertisement will be saved
    /// in the BSS map.
    pub fn handle_ap_advertisement(
        &mut self,
        bssid: Bssid,
        beacon_interval: TimeUnit,
        capability_info: CapabilityInfo,
        ies: &[u8],
        rx_info: fidl_softmac::WlanRxInfo,
    ) {
        wtrace::duration!("BoundScanner::handle_ap_advertisement");

        let mlme_txn_id = match self.scanner.ongoing_scan {
            Some(OngoingScan::PassiveOffloadScan { mlme_txn_id, .. }) => mlme_txn_id,
            Some(OngoingScan::ActiveOffloadScan { mlme_txn_id, .. }) => mlme_txn_id,
            None => return,
        };
        let bss_description =
            construct_bss_description(bssid, beacon_interval, capability_info, ies, rx_info);
        let bss_description = match bss_description {
            Ok(bss) => bss,
            Err(e) => {
                warn!("Failed to process beacon or probe response: {}", e);
                return;
            }
        };
        send_scan_result(mlme_txn_id, bss_description, &mut self.ctx.device);
    }

    pub async fn handle_scan_complete(&mut self, status: Result<(), zx::Status>, scan_id: u64) {
        macro_rules! send_on_scan_end {
            ($mlme_txn_id: ident, $code:expr) => {
                self.ctx
                    .device
                    .send_mlme_event(fidl_mlme::MlmeEvent::OnScanEnd {
                        end: fidl_mlme::ScanEnd { txn_id: $mlme_txn_id, code: $code },
                    })
                    .unwrap_or_else(|e| error!("error sending MLME ScanEnd: {}", e));
            };
        }

        match self.scanner.ongoing_scan.take() {
            // TODO(https://fxbug.dev/42172565): A spurious ScanComplete should not silently cancel an
            // MlmeScan by permanently taking the contents of ongoing_scan.
            None => {
                warn!("Unexpected ScanComplete when no scan in progress.");
            }
            Some(OngoingScan::PassiveOffloadScan { mlme_txn_id, in_progress_device_scan_id })
                if in_progress_device_scan_id == scan_id =>
            {
                send_on_scan_end!(
                    mlme_txn_id,
                    if let Err(error) = &status {
                        error!(error:?; "passive offload scan failed");
                        fidl_mlme::ScanResultCode::InternalError
                    } else {
                        fidl_mlme::ScanResultCode::Success
                    }
                );
            }
            Some(OngoingScan::ActiveOffloadScan {
                mlme_txn_id,
                in_progress_device_scan_id,
                mut remaining_active_scan_requests,
            }) if in_progress_device_scan_id == scan_id => {
                if let Err(error) = status {
                    error!(error:?; "active offload scan failed");
                    send_on_scan_end!(mlme_txn_id, fidl_mlme::ScanResultCode::InternalError);
                    return;
                }

                match remaining_active_scan_requests.pop() {
                    None => {
                        send_on_scan_end!(mlme_txn_id, fidl_mlme::ScanResultCode::Success);
                    }
                    Some(active_scan_request) => {
                        match self.start_next_active_scan(&active_scan_request).await {
                            Ok(in_progress_device_scan_id) => {
                                self.scanner.ongoing_scan = Some(OngoingScan::ActiveOffloadScan {
                                    mlme_txn_id,
                                    in_progress_device_scan_id,
                                    remaining_active_scan_requests,
                                });
                            }
                            Err(scan_error) => {
                                self.scanner.ongoing_scan.take();
                                send_on_scan_end!(mlme_txn_id, scan_error.into());
                            }
                        }
                    }
                }
            }
            Some(other) => {
                let in_progress_device_scan_id = match other {
                    OngoingScan::ActiveOffloadScan { in_progress_device_scan_id, .. } => {
                        in_progress_device_scan_id
                    }
                    OngoingScan::PassiveOffloadScan { in_progress_device_scan_id, .. } => {
                        in_progress_device_scan_id
                    }
                };
                warn!(
                    "Unexpected scan ID upon scan completion. expected: {}, returned: {}",
                    in_progress_device_scan_id, scan_id
                );
                self.scanner.ongoing_scan.replace(other);
            }
        }
    }

    fn probe_request_mac_header(&mut self) -> mac::MgmtHdr {
        mgmt_writer::mgmt_hdr_to_ap(
            mac::FrameControl(0)
                .with_frame_type(mac::FrameType::MGMT)
                .with_mgmt_subtype(mac::MgmtSubtype::PROBE_REQ),
            ieee80211::BROADCAST_ADDR.into(),
            self.scanner.iface_mac,
            mac::SequenceControl(0)
                .with_seq_num(self.ctx.seq_mgr.next_sns1(&ieee80211::BROADCAST_ADDR) as u16),
        )
    }
}

fn band_cap_for_band(
    query_response: &fidl_softmac::WlanSoftmacQueryResponse,
    band: fidl_ieee80211::WlanBand,
) -> Option<&fidl_softmac::WlanSoftmacBandCapability> {
    query_response
        .band_caps
        .as_ref()
        .map(|band_caps| band_caps.iter())
        .into_iter()
        .flatten()
        .filter(|band_cap| band_cap.band == Some(band))
        .next()
}

// TODO(https://fxbug.dev/42172555): Zero should not mark a null rate.
fn supported_rates_for_band(
    query_response: &fidl_softmac::WlanSoftmacQueryResponse,
    band: fidl_ieee80211::WlanBand,
) -> Result<Vec<u8>, Error> {
    let rates = band_cap_for_band(&query_response, band)
        .ok_or_else(|| format_err!("no capabilities found for band {:?}", band))?
        .basic_rates
        .clone()
        .ok_or_else(|| format_err!("no basic rates found for band capabilities"))?;
    Ok(rates)
}

fn active_scan_request_series(
    query_response: &fidl_softmac::WlanSoftmacQueryResponse,
    channels: Vec<fidl_ieee80211::ChannelNumber>,
    ssids: Vec<fidl_ieee80211::CSsid>,
    mac_header: Vec<u8>,
    min_channel_time: zx::sys::zx_duration_t,
    max_channel_time: zx::sys::zx_duration_t,
    min_home_time: zx::sys::zx_duration_t,
    min_probes_per_channel: u8,
    max_probes_per_channel: u8,
) -> Result<Vec<fidl_softmac::WlanSoftmacStartActiveScanRequest>, Error> {
    struct BandChannels {
        band: fidl_ieee80211::WlanBand,
        channels: Vec<u8>,
    }
    let band_channels_list: [BandChannels; 2] = channels.into_iter().fold(
        [
            BandChannels { band: fidl_ieee80211::WlanBand::FiveGhz, channels: vec![] },
            BandChannels { band: fidl_ieee80211::WlanBand::TwoGhz, channels: vec![] },
        ],
        |mut band_channels_list, channel| {
            for band_channels in &mut band_channels_list {
                if channel.band == band_channels.band {
                    band_channels.channels.push(channel.number);
                }
            }
            band_channels_list
        },
    );

    let mut active_scan_request_series = vec![];
    for band_channels in band_channels_list {
        let band = band_channels.band;
        let channels = band_channels.channels;
        if channels.is_empty() {
            continue;
        }
        let supported_rates = supported_rates_for_band(query_response, band)?;
        let fidl_channels =
            channels.iter().map(|&c| fidl_ieee80211::ChannelNumber { band, number: c }).collect();
        active_scan_request_series.push(fidl_softmac::WlanSoftmacStartActiveScanRequest {
            channels: Some(fidl_channels),
            ssids: Some(ssids.clone()),
            mac_header: Some(mac_header.clone()),
            // Exclude the SSID IE because the device driver will generate using ssids_list.
            ies: Some(write_frame_to_vec!({
                ies: {
                    supported_rates: supported_rates,
                    extended_supported_rates: {/* continue rates */},
                }
            })?),
            min_channel_time: Some(min_channel_time),
            max_channel_time: Some(max_channel_time),
            min_home_time: Some(min_home_time),
            min_probes_per_channel: Some(min_probes_per_channel),
            max_probes_per_channel: Some(max_probes_per_channel),
            ..Default::default()
        });
    }
    Ok(active_scan_request_series)
}

fn send_scan_result<D: DeviceOps>(
    txn_id: u64,
    bss: fidl_ieee80211::BssDescription,
    device: &mut D,
) {
    if trace::is_enabled() {
        let trace_bss = wlan_common::bss::BssDescription::try_from(bss.clone())
            .map(|bss| format!("{}", bss))
            .unwrap_or_else(|e| format!("{}", e));
        wtrace::duration!("send_scan_result", "bss" => &*trace_bss);
    }
    device
        .send_mlme_event(fidl_mlme::MlmeEvent::OnScanResult {
            result: fidl_mlme::ScanResult {
                txn_id,
                timestamp_nanos: zx::MonotonicInstant::get().into_nanos(),
                bss,
            },
        })
        .unwrap_or_else(|e| error!("error sending MLME ScanResult: {}", e));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::TimedEvent;
    use crate::device::{FakeDevice, FakeDeviceState};
    use crate::test_utils::{MockWlanRxInfo, fake_wlan_channel};
    use assert_matches::assert_matches;
    use fuchsia_sync::Mutex;
    use ieee80211::{MacAddrBytes, Ssid};
    use std::sync::{Arc, LazyLock};
    use test_case::test_case;
    use wlan_common::sequence::SequenceManager;
    use wlan_common::timer::{self, Timer, create_timer};

    use fidl_ieee80211::ChannelNumber;
    use fidl_ieee80211::WlanBand::{FiveGhz, TwoGhz};

    static BSSID_FOO: LazyLock<Bssid> = LazyLock::new(|| [6u8; 6].into());
    const CAPABILITY_INFO_FOO: CapabilityInfo = CapabilityInfo(1);
    const BEACON_INTERVAL_FOO: u16 = 100;

    #[rustfmt::skip]
    static BEACON_IES_FOO: &'static [u8] = &[
        // SSID: "ssid"
        0x00, 0x03, b'f', b'o', b'o',
        // Supported rates: 24(B), 36, 48, 54
        0x01, 0x04, 0xb0, 0x48, 0x60, 0x6c,
        // TIM - DTIM count: 0, DTIM period: 1, PVB: 2
        0x05, 0x04, 0x00, 0x01, 0x00, 0x02,
    ];

    static RX_INFO_FOO: LazyLock<fidl_softmac::WlanRxInfo> = LazyLock::new(|| {
        MockWlanRxInfo { rssi_dbm: -30, ..MockWlanRxInfo::with_channel(fake_wlan_channel().into()) }
            .into()
    });
    static BSS_DESCRIPTION_FOO: LazyLock<fidl_ieee80211::BssDescription> =
        LazyLock::new(|| fidl_ieee80211::BssDescription {
            bssid: BSSID_FOO.to_array(),
            bss_type: fidl_ieee80211::BssType::Infrastructure,
            beacon_period: BEACON_INTERVAL_FOO,
            capability_info: CAPABILITY_INFO_FOO.0,
            ies: BEACON_IES_FOO.to_vec(),
            rssi_dbm: RX_INFO_FOO.rssi_dbm,
            primary: fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: RX_INFO_FOO.primary.number,
            },
            bandwidth: fidl_ieee80211::ChannelBandwidth::Cbw20,
            vht_secondary_80_channel: fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: 0,
            },
            snr_db: 0,
        });
    static BSSID_BAR: LazyLock<Bssid> = LazyLock::new(|| [1u8; 6].into());

    const CAPABILITY_INFO_BAR: CapabilityInfo = CapabilityInfo(33);
    const BEACON_INTERVAL_BAR: u16 = 150;
    #[rustfmt::skip]
    static BEACON_IES_BAR: &'static [u8] = &[
        // SSID: "ss"
        0x00, 0x03, b'b', b'a', b'r',
        // Supported rates: 24(B), 36, 48, 54
        0x01, 0x04, 0xb0, 0x48, 0x60, 0x6c,
        // TIM - DTIM count: 0, DTIM period: 1, PVB: 2
        0x05, 0x04, 0x00, 0x01, 0x00, 0x02,
    ];
    static RX_INFO_BAR: LazyLock<fidl_softmac::WlanRxInfo> = LazyLock::new(|| {
        MockWlanRxInfo { rssi_dbm: -60, ..MockWlanRxInfo::with_channel(fake_wlan_channel().into()) }
            .into()
    });
    static BSS_DESCRIPTION_BAR: LazyLock<fidl_ieee80211::BssDescription> =
        LazyLock::new(|| fidl_ieee80211::BssDescription {
            bssid: BSSID_BAR.to_array(),
            bss_type: fidl_ieee80211::BssType::Infrastructure,
            beacon_period: BEACON_INTERVAL_BAR,
            capability_info: CAPABILITY_INFO_BAR.0,
            ies: BEACON_IES_BAR.to_vec(),
            rssi_dbm: RX_INFO_BAR.rssi_dbm,
            primary: fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: RX_INFO_BAR.primary.number,
            },
            bandwidth: fidl_ieee80211::ChannelBandwidth::Cbw20,
            vht_secondary_80_channel: fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: 0,
            },
            snr_db: 0,
        });

    static IFACE_MAC: LazyLock<MacAddr> = LazyLock::new(|| [7u8; 6].into());

    fn passive_scan_req() -> fidl_mlme::ScanRequest {
        fidl_mlme::ScanRequest {
            txn_id: 1337,
            scan_type: fidl_mlme::ScanTypes::Passive,
            channel_list: vec![ChannelNumber { number: 6, band: TwoGhz }],
            ssid_list: vec![],
            probe_delay: 0,
            min_channel_time: 100,
            max_channel_time: 300,
        }
    }

    fn active_scan_req(channel_list: &[ChannelNumber]) -> fidl_mlme::ScanRequest {
        fidl_mlme::ScanRequest {
            txn_id: 1337,
            scan_type: fidl_mlme::ScanTypes::Active,
            channel_list: Vec::from(channel_list),
            ssid_list: vec![
                Ssid::try_from("foo").unwrap().into(),
                Ssid::try_from("bar").unwrap().into(),
            ],
            probe_delay: 3,
            min_channel_time: 100,
            max_channel_time: 300,
        }
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_handle_scan_req_reject_if_busy() {
        let mut m = MockObjects::new().await;
        let mut ctx = m.make_ctx();
        let mut scanner = Scanner::new(*IFACE_MAC);

        scanner
            .bind(&mut ctx)
            .on_sme_scan(passive_scan_req())
            .await
            .expect("expect scan req accepted");
        let scan_req = fidl_mlme::ScanRequest { txn_id: 1338, ..passive_scan_req() };
        let result = scanner.bind(&mut ctx).on_sme_scan(scan_req).await;
        assert_matches!(result, Err(Error::ScanError(ScanError::Busy)));
        m.fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::ScanEnd>()
            .expect_err("unexpected MLME ScanEnd from BoundScanner");
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_handle_scan_req_reject_if_disabled() {
        let mut m = MockObjects::new().await;
        let mut ctx = m.make_ctx();
        let mut scanner = Scanner::new(*IFACE_MAC);

        scanner.bind(&mut ctx).disable_scanning().await.expect("Failed to disable scanning");
        let result = scanner.bind(&mut ctx).on_sme_scan(passive_scan_req()).await;
        assert_matches!(result, Err(Error::ScanError(ScanError::Busy)));
        m.fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::ScanEnd>()
            .expect_err("unexpected MLME ScanEnd from BoundScanner");

        // Accept after reenabled.
        scanner.bind(&mut ctx).enable_scanning();
        scanner
            .bind(&mut ctx)
            .on_sme_scan(passive_scan_req())
            .await
            .expect("expect scan req accepted");
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_handle_scan_req_empty_channel_list() {
        let mut m = MockObjects::new().await;
        let mut ctx = m.make_ctx();
        let mut scanner = Scanner::new(*IFACE_MAC);

        let scan_req = fidl_mlme::ScanRequest { channel_list: vec![], ..passive_scan_req() };
        let result = scanner.bind(&mut ctx).on_sme_scan(scan_req).await;
        assert_matches!(result, Err(Error::ScanError(ScanError::EmptyChannelList)));
        m.fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::ScanEnd>()
            .expect_err("unexpected MLME ScanEnd from BoundScanner");
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_handle_scan_req_invalid_channel_time() {
        let mut m = MockObjects::new().await;
        let mut ctx = m.make_ctx();
        let mut scanner = Scanner::new(*IFACE_MAC);

        let scan_req = fidl_mlme::ScanRequest {
            min_channel_time: 101,
            max_channel_time: 100,
            ..passive_scan_req()
        };
        let result = scanner.bind(&mut ctx).on_sme_scan(scan_req).await;
        assert_matches!(result, Err(Error::ScanError(ScanError::MaxChannelTimeLtMin)));
        m.fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::ScanEnd>()
            .expect_err("unexpected MLME ScanEnd from BoundScanner");
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_start_offload_passive_scan_success() {
        let mut m = MockObjects::new().await;
        let mut ctx = m.make_ctx();
        let mut scanner = Scanner::new(*IFACE_MAC);
        let test_start_timestamp_nanos = zx::MonotonicInstant::get().into_nanos();

        scanner
            .bind(&mut ctx)
            .on_sme_scan(passive_scan_req())
            .await
            .expect("expect scan req accepted");

        // Verify that passive offload scan is requested
        assert_eq!(
            m.fake_device_state.lock().captured_passive_scan_request,
            Some(fidl_softmac::WlanSoftmacBaseStartPassiveScanRequest {
                channels: Some(vec![fidl_ieee80211::ChannelNumber {
                    band: fidl_ieee80211::WlanBand::TwoGhz,
                    number: 6,
                }]),
                min_channel_time: Some(102_400_000),
                max_channel_time: Some(307_200_000),
                min_home_time: Some(0),
                ..Default::default()
            }),
        );
        let expected_scan_id = m.fake_device_state.lock().next_scan_id - 1;

        // Mock receiving a beacon
        handle_beacon_foo(&mut scanner, &mut ctx);
        let scan_result = m
            .fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::ScanResult>()
            .expect("error reading ScanResult");
        assert_eq!(scan_result.txn_id, 1337);
        assert!(scan_result.timestamp_nanos > test_start_timestamp_nanos);
        assert_eq!(scan_result.bss, *BSS_DESCRIPTION_FOO);

        // Verify ScanEnd sent after handle_scan_complete
        scanner.bind(&mut ctx).handle_scan_complete(Ok(()), expected_scan_id).await;
        let scan_end = m
            .fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::ScanEnd>()
            .expect("error reading MLME ScanEnd");
        assert_eq!(
            scan_end,
            fidl_mlme::ScanEnd { txn_id: 1337, code: fidl_mlme::ScanResultCode::Success }
        );
    }

    struct ExpectedDynamicActiveScanRequest {
        channels: Vec<ChannelNumber>,
        ies: Vec<u8>,
    }

    #[test_case(&[ChannelNumber { number: 6, band: TwoGhz }],
                Some(ExpectedDynamicActiveScanRequest {
                    channels: vec![ChannelNumber { number: 6, band: TwoGhz }],
                    ies: vec![ 0x01, // Element ID for Supported Rates
                               0x08, // Length
                               0x02, 0x04, 0x0b, 0x16, 0x0c, 0x12, 0x18, 0x24, // Supported Rates
                               0x32, // Element ID for Extended Supported Rates
                               0x04, // Length
                               0x30, 0x48, 0x60, 0x6c // Extended Supported Rates
                    ]}),
                None; "single channel")]
    #[test_case(&[ChannelNumber { number: 1, band: TwoGhz }, ChannelNumber { number: 2, band: TwoGhz }, ChannelNumber { number: 3, band: TwoGhz }, ChannelNumber { number: 4, band: TwoGhz }, ChannelNumber { number: 5, band: TwoGhz }],
                Some(ExpectedDynamicActiveScanRequest {
                    channels: vec![ChannelNumber { number: 1, band: TwoGhz }, ChannelNumber { number: 2, band: TwoGhz }, ChannelNumber { number: 3, band: TwoGhz }, ChannelNumber { number: 4, band: TwoGhz }, ChannelNumber { number: 5, band: TwoGhz }],
                    ies: vec![ 0x01, // Element ID for Supported Rates
                               0x08, // Length
                               0x02, 0x04, 0x0b, 0x16, 0x0c, 0x12, 0x18, 0x24, // Supported Rates
                               0x32, // Element ID for Extended Supported Rates
                               0x04, // Length
                               0x30, 0x48, 0x60, 0x6c // Extended Supported Rates
                    ]}),
                None; "multiple channels 2.4GHz band")]
    #[test_case(&[ChannelNumber { number: 36, band: FiveGhz }, ChannelNumber { number: 40, band: FiveGhz }, ChannelNumber { number: 100, band: FiveGhz }, ChannelNumber { number: 108, band: FiveGhz }],
                None,
                Some(ExpectedDynamicActiveScanRequest {
                    channels: vec![ChannelNumber { number: 36, band: FiveGhz }, ChannelNumber { number: 40, band: FiveGhz }, ChannelNumber { number: 100, band: FiveGhz }, ChannelNumber { number: 108, band: FiveGhz }],
                    ies: vec![ 0x01, // Element ID for Supported Rates
                               0x08, // Length
                               0x02, 0x04, 0x0b, 0x16, 0x30, 0x60, 0x7e, 0x7f // Supported Rates
                    ],
                }); "multiple channels 5GHz band")]
    #[test_case(&[ChannelNumber { number: 1, band: TwoGhz }, ChannelNumber { number: 2, band: TwoGhz }, ChannelNumber { number: 3, band: TwoGhz }, ChannelNumber { number: 4, band: TwoGhz }, ChannelNumber { number: 5, band: TwoGhz }, ChannelNumber { number: 36, band: FiveGhz }, ChannelNumber { number: 40, band: FiveGhz }, ChannelNumber { number: 100, band: FiveGhz }, ChannelNumber { number: 108, band: FiveGhz }],
                Some(ExpectedDynamicActiveScanRequest {
                    channels: vec![ChannelNumber { number: 1, band: TwoGhz }, ChannelNumber { number: 2, band: TwoGhz }, ChannelNumber { number: 3, band: TwoGhz }, ChannelNumber { number: 4, band: TwoGhz }, ChannelNumber { number: 5, band: TwoGhz }],
                    ies: vec![ 0x01, // Element ID for Supported Rates
                               0x08, // Length
                               0x02, 0x04, 0x0b, 0x16, 0x0c, 0x12, 0x18, 0x24, // Supported Rates
                               0x32, // Element ID for Extended Supported Rates
                               0x04, // Length
                               0x30, 0x48, 0x60, 0x6c // Extended Supported Rates
                    ]}),
                Some(ExpectedDynamicActiveScanRequest {
                    channels: vec![ChannelNumber { number: 36, band: FiveGhz }, ChannelNumber { number: 40, band: FiveGhz }, ChannelNumber { number: 100, band: FiveGhz }, ChannelNumber { number: 108, band: FiveGhz }],
                    ies: vec![ 0x01, // Element ID for Supported Rates
                               0x08, // Length
                               0x02, 0x04, 0x0b, 0x16, 0x30, 0x60, 0x7e, 0x7f, // Supported Rates
                    ],
                }); "multiple bands")]
    #[fuchsia::test(allow_stalls = false)]
    async fn test_start_active_scan_success(
        channel_list: &[ChannelNumber],
        expected_two_ghz_dynamic_args: Option<ExpectedDynamicActiveScanRequest>,
        expected_five_ghz_dynamic_args: Option<ExpectedDynamicActiveScanRequest>,
    ) {
        let mut m = MockObjects::new().await;
        let mut ctx = m.make_ctx();
        let mut scanner = Scanner::new(*IFACE_MAC);
        let test_start_timestamp_nanos = zx::MonotonicInstant::get().into_nanos();

        scanner
            .bind(&mut ctx)
            .on_sme_scan(active_scan_req(channel_list))
            .await
            .expect("expect scan req accepted");

        for probe_request_ies in &[expected_two_ghz_dynamic_args, expected_five_ghz_dynamic_args] {
            match probe_request_ies {
                None => {}
                Some(ExpectedDynamicActiveScanRequest { channels, ies, .. }) => {
                    // Verify that active offload scan is requested
                    assert_eq!(
                        m.fake_device_state.lock().captured_active_scan_request,
                        Some(fidl_softmac::WlanSoftmacStartActiveScanRequest {
                            channels: Some(channels.clone()),
                            ssids: Some(vec![
                                cssid_from_ssid_unchecked(&Ssid::try_from("foo").unwrap().into()),
                                cssid_from_ssid_unchecked(&Ssid::try_from("bar").unwrap().into()),
                            ]),
                            mac_header: Some(vec![
                                0x40, 0x00, // Frame Control
                                0x00, 0x00, // Duration
                                0xff, 0xff, 0xff, 0xff, 0xff, 0xff, // Address 1
                                0x07, 0x07, 0x07, 0x07, 0x07, 0x07, // Address 2
                                0xff, 0xff, 0xff, 0xff, 0xff, 0xff, // Address 3
                                0x10, 0x00, // Sequence Control
                            ]),
                            ies: Some(ies.clone()),
                            min_channel_time: Some(102_400_000),
                            max_channel_time: Some(307_200_000),
                            min_home_time: Some(0),
                            min_probes_per_channel: Some(0),
                            max_probes_per_channel: Some(0),
                            ..Default::default()
                        }),
                        "active offload scan not initiated"
                    );
                    let expected_scan_id = m.fake_device_state.lock().next_scan_id - 1;

                    // Mock receiving beacons
                    handle_beacon_foo(&mut scanner, &mut ctx);
                    let scan_result = m
                        .fake_device_state
                        .lock()
                        .next_mlme_msg::<fidl_mlme::ScanResult>()
                        .expect("error reading ScanResult");
                    assert_eq!(scan_result.txn_id, 1337);
                    assert!(scan_result.timestamp_nanos > test_start_timestamp_nanos);
                    assert_eq!(scan_result.bss, *BSS_DESCRIPTION_FOO);

                    handle_beacon_bar(&mut scanner, &mut ctx);
                    let scan_result = m
                        .fake_device_state
                        .lock()
                        .next_mlme_msg::<fidl_mlme::ScanResult>()
                        .expect("error reading ScanResult");
                    assert_eq!(scan_result.txn_id, 1337);
                    assert!(scan_result.timestamp_nanos > test_start_timestamp_nanos);
                    assert_eq!(scan_result.bss, *BSS_DESCRIPTION_BAR);

                    // Verify ScanEnd sent after handle_scan_complete
                    scanner.bind(&mut ctx).handle_scan_complete(Ok(()), expected_scan_id).await;
                }
            }
        }
        let scan_end = m
            .fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::ScanEnd>()
            .expect("error reading MLME ScanEnd");
        assert_eq!(
            scan_end,
            fidl_mlme::ScanEnd { txn_id: 1337, code: fidl_mlme::ScanResultCode::Success }
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_start_passive_scan_fails() {
        let mut m = MockObjects::new().await;
        m.fake_device_state.lock().config.start_passive_scan_fails = true;
        let mut ctx = m.make_ctx();
        let mut scanner = Scanner::new(*IFACE_MAC);

        let result = scanner.bind(&mut ctx).on_sme_scan(passive_scan_req()).await;
        assert_matches!(
            result,
            Err(Error::ScanError(ScanError::StartOffloadScanFails(zx::Status::NOT_SUPPORTED)))
        );
        m.fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::ScanEnd>()
            .expect_err("unexpected MLME ScanEnd from BoundScanner");
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_start_active_scan_fails() {
        let mut m = MockObjects::new().await;
        m.fake_device_state.lock().config.start_active_scan_fails = true;
        let mut ctx = m.make_ctx();
        let mut scanner = Scanner::new(*IFACE_MAC);

        let result = scanner
            .bind(&mut ctx)
            .on_sme_scan(active_scan_req(&[ChannelNumber { number: 6, band: TwoGhz }]))
            .await;
        assert_matches!(
            result,
            Err(Error::ScanError(ScanError::StartOffloadScanFails(zx::Status::NOT_SUPPORTED)))
        );
        m.fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::ScanEnd>()
            .expect_err("unexpected MLME ScanEnd from BoundScanner");
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_start_passive_scan_canceled() {
        let mut m = MockObjects::new().await;
        let mut ctx = m.make_ctx();
        let mut scanner = Scanner::new(*IFACE_MAC);
        let test_start_timestamp_nanos = zx::MonotonicInstant::get().into_nanos();

        scanner
            .bind(&mut ctx)
            .on_sme_scan(passive_scan_req())
            .await
            .expect("expect scan req accepted");

        // Verify that passive offload scan is requested
        assert_matches!(
            m.fake_device_state.lock().captured_passive_scan_request,
            Some(_),
            "passive offload scan not initiated"
        );
        let expected_scan_id = m.fake_device_state.lock().next_scan_id - 1;

        // Mock receiving a beacon
        handle_beacon_foo(&mut scanner, &mut ctx);
        let scan_result = m
            .fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::ScanResult>()
            .expect("error reading ScanResult");
        assert_eq!(scan_result.txn_id, 1337);
        assert!(scan_result.timestamp_nanos > test_start_timestamp_nanos);
        assert_eq!(scan_result.bss, *BSS_DESCRIPTION_FOO);

        // Verify ScanEnd sent after handle_scan_complete
        scanner
            .bind(&mut ctx)
            .handle_scan_complete(Err(zx::Status::CANCELED), expected_scan_id)
            .await;
        let scan_end = m
            .fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::ScanEnd>()
            .expect("error reading MLME ScanEnd");
        assert_eq!(
            scan_end,
            fidl_mlme::ScanEnd { txn_id: 1337, code: fidl_mlme::ScanResultCode::InternalError }
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_start_active_scan_canceled() {
        let mut m = MockObjects::new().await;
        let mut ctx = m.make_ctx();
        let mut scanner = Scanner::new(*IFACE_MAC);
        let test_start_timestamp_nanos = zx::MonotonicInstant::get().into_nanos();

        scanner
            .bind(&mut ctx)
            .on_sme_scan(active_scan_req(&[ChannelNumber { number: 6, band: TwoGhz }]))
            .await
            .expect("expect scan req accepted");

        // Verify that active offload scan is requested
        assert!(
            m.fake_device_state.lock().captured_active_scan_request.is_some(),
            "active offload scan not initiated"
        );
        let expected_scan_id = m.fake_device_state.lock().next_scan_id - 1;

        // Mock receiving a beacon
        handle_beacon_foo(&mut scanner, &mut ctx);
        let scan_result = m
            .fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::ScanResult>()
            .expect("error reading ScanResult");
        assert_eq!(scan_result.txn_id, 1337);
        assert!(scan_result.timestamp_nanos > test_start_timestamp_nanos);
        assert_eq!(scan_result.bss, *BSS_DESCRIPTION_FOO);

        // Verify ScanEnd sent after handle_scan_complete
        scanner
            .bind(&mut ctx)
            .handle_scan_complete(Err(zx::Status::CANCELED), expected_scan_id)
            .await;
        let scan_end = m
            .fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::ScanEnd>()
            .expect("error reading MLME ScanEnd");
        assert_eq!(
            scan_end,
            fidl_mlme::ScanEnd { txn_id: 1337, code: fidl_mlme::ScanResultCode::InternalError }
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_handle_ap_advertisement() {
        let mut m = MockObjects::new().await;
        let mut ctx = m.make_ctx();
        let mut scanner = Scanner::new(*IFACE_MAC);
        let test_start_timestamp_nanos = zx::MonotonicInstant::get().into_nanos();

        scanner
            .bind(&mut ctx)
            .on_sme_scan(passive_scan_req())
            .await
            .expect("expect scan req accepted");
        handle_beacon_foo(&mut scanner, &mut ctx);
        let ongoing_scan_id = scanner.ongoing_scan.as_ref().unwrap().scan_id();
        scanner.bind(&mut ctx).handle_scan_complete(Ok(()), ongoing_scan_id).await;

        let scan_result = m
            .fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::ScanResult>()
            .expect("error reading MLME ScanResult");
        assert_eq!(scan_result.txn_id, 1337);
        assert!(scan_result.timestamp_nanos > test_start_timestamp_nanos);
        assert_eq!(scan_result.bss, *BSS_DESCRIPTION_FOO);

        let scan_end = m
            .fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::ScanEnd>()
            .expect("error reading MLME ScanEnd");
        assert_eq!(
            scan_end,
            fidl_mlme::ScanEnd { txn_id: 1337, code: fidl_mlme::ScanResultCode::Success }
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_handle_ap_advertisement_multiple() {
        let mut m = MockObjects::new().await;
        let mut ctx = m.make_ctx();
        let mut scanner = Scanner::new(*IFACE_MAC);
        let test_start_timestamp_nanos = zx::MonotonicInstant::get().into_nanos();

        scanner
            .bind(&mut ctx)
            .on_sme_scan(passive_scan_req())
            .await
            .expect("expect scan req accepted");

        handle_beacon_foo(&mut scanner, &mut ctx);
        handle_beacon_bar(&mut scanner, &mut ctx);
        let ongoing_scan_id = scanner.ongoing_scan.as_ref().unwrap().scan_id();
        scanner.bind(&mut ctx).handle_scan_complete(Ok(()), ongoing_scan_id).await;

        // Verify that one scan result is sent for each beacon
        let foo_scan_result = m
            .fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::ScanResult>()
            .expect("error reading MLME ScanResult");
        assert_eq!(foo_scan_result.txn_id, 1337);
        assert!(foo_scan_result.timestamp_nanos > test_start_timestamp_nanos);
        assert_eq!(foo_scan_result.bss, *BSS_DESCRIPTION_FOO);

        let bar_scan_result = m
            .fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::ScanResult>()
            .expect("error reading MLME ScanResult");
        assert_eq!(bar_scan_result.txn_id, 1337);
        assert!(bar_scan_result.timestamp_nanos > foo_scan_result.timestamp_nanos);
        assert_eq!(bar_scan_result.bss, *BSS_DESCRIPTION_BAR);

        let scan_end = m
            .fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::ScanEnd>()
            .expect("error reading MLME ScanEnd");
        assert_eq!(
            scan_end,
            fidl_mlme::ScanEnd { txn_id: 1337, code: fidl_mlme::ScanResultCode::Success }
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn not_scanning_vs_scanning() {
        let mut m = MockObjects::new().await;
        let mut ctx = m.make_ctx();
        let mut scanner = Scanner::new(*IFACE_MAC);
        assert_eq!(false, scanner.is_scanning());

        scanner
            .bind(&mut ctx)
            .on_sme_scan(passive_scan_req())
            .await
            .expect("expect scan req accepted");
        assert_eq!(true, scanner.is_scanning());
    }

    fn handle_beacon_foo(scanner: &mut Scanner, ctx: &mut Context<FakeDevice>) {
        scanner.bind(ctx).handle_ap_advertisement(
            *BSSID_FOO,
            TimeUnit(BEACON_INTERVAL_FOO),
            CAPABILITY_INFO_FOO,
            BEACON_IES_FOO,
            RX_INFO_FOO.clone(),
        );
    }

    fn handle_beacon_bar(scanner: &mut Scanner, ctx: &mut Context<FakeDevice>) {
        scanner.bind(ctx).handle_ap_advertisement(
            *BSSID_BAR,
            TimeUnit(BEACON_INTERVAL_BAR),
            CAPABILITY_INFO_BAR,
            BEACON_IES_BAR,
            RX_INFO_BAR.clone(),
        );
    }

    struct MockObjects {
        fake_device: FakeDevice,
        fake_device_state: Arc<Mutex<FakeDeviceState>>,
        _time_stream: timer::EventStream<TimedEvent>,
        timer: Option<Timer<TimedEvent>>,
    }

    impl MockObjects {
        // TODO(https://fxbug.dev/327499461): This function is async to ensure MLME functions will
        // run in an async context and not call `wlan_common::timer::Timer::now` without an
        // executor.
        async fn new() -> Self {
            let (timer, _time_stream) = create_timer();
            let (fake_device, fake_device_state) = FakeDevice::new().await;
            Self { fake_device, fake_device_state, _time_stream, timer: Some(timer) }
        }

        fn make_ctx(&mut self) -> Context<FakeDevice> {
            Context {
                _config: Default::default(),
                device: self.fake_device.clone(),
                timer: self.timer.take().unwrap(),
                seq_mgr: SequenceManager::new(),
            }
        }
    }
}
