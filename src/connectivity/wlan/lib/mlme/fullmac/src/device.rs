// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, format_err};
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_wlan_common as fidl_common;
use fidl_fuchsia_wlan_fullmac as fidl_fullmac;
use fidl_fuchsia_wlan_mlme as fidl_mlme;
use fidl_fuchsia_wlan_stats as fidl_stats;

/// This trait abstracts how Device accomplish operations. Test code
/// can then implement trait methods instead of mocking an underlying DeviceInterface
/// and FIDL proxy.
pub trait DeviceOps {
    fn init(
        &mut self,
        fullmac_ifc_client_end: ClientEnd<fidl_fullmac::WlanFullmacImplIfcMarker>,
    ) -> Result<fidl::Channel, zx::Status>;
    fn query_device_info(&self) -> anyhow::Result<fidl_fullmac::WlanFullmacImplQueryResponse>;
    fn query_security_support(&self) -> anyhow::Result<fidl_common::SecuritySupport>;
    fn query_spectrum_management_support(
        &self,
    ) -> anyhow::Result<fidl_common::SpectrumManagementSupport>;
    fn query_telemetry_support(&self) -> anyhow::Result<Result<fidl_stats::TelemetrySupport, i32>>;
    fn query_apf_packet_filter_support(
        &self,
    ) -> anyhow::Result<Result<fidl_common::ApfPacketFilterSupport, i32>>;
    fn start_scan(&self, req: fidl_fullmac::WlanFullmacImplStartScanRequest) -> anyhow::Result<()>;
    fn connect(&self, req: fidl_fullmac::WlanFullmacImplConnectRequest) -> anyhow::Result<()>;
    fn reconnect(&self, req: fidl_fullmac::WlanFullmacImplReconnectRequest) -> anyhow::Result<()>;
    fn roam(&self, req: fidl_fullmac::WlanFullmacImplRoamRequest) -> anyhow::Result<()>;
    fn auth_resp(&self, resp: fidl_fullmac::WlanFullmacImplAuthRespRequest) -> anyhow::Result<()>;
    fn deauth(&self, req: fidl_fullmac::WlanFullmacImplDeauthRequest) -> anyhow::Result<()>;
    fn assoc_resp(&self, resp: fidl_fullmac::WlanFullmacImplAssocRespRequest)
    -> anyhow::Result<()>;
    fn disassoc(&self, req: fidl_fullmac::WlanFullmacImplDisassocRequest) -> anyhow::Result<()>;
    fn start_bss(&self, req: fidl_fullmac::WlanFullmacImplStartBssRequest) -> anyhow::Result<()>;
    fn stop_bss(&self, req: fidl_fullmac::WlanFullmacImplStopBssRequest) -> anyhow::Result<()>;
    fn set_keys(
        &self,
        req: fidl_fullmac::WlanFullmacImplSetKeysRequest,
    ) -> anyhow::Result<fidl_fullmac::WlanFullmacSetKeysResp>;
    fn eapol_tx(&self, req: fidl_fullmac::WlanFullmacImplEapolTxRequest) -> anyhow::Result<()>;
    fn get_iface_stats(&self) -> anyhow::Result<fidl_mlme::GetIfaceStatsResponse>;
    fn get_iface_histogram_stats(
        &self,
    ) -> anyhow::Result<fidl_mlme::GetIfaceHistogramStatsResponse>;
    fn get_signal_report(&self) -> anyhow::Result<Result<fidl_stats::SignalReport, i32>>;
    fn sae_handshake_resp(
        &self,
        resp: fidl_fullmac::WlanFullmacImplSaeHandshakeRespRequest,
    ) -> anyhow::Result<()>;
    fn sae_frame_tx(&self, frame: fidl_fullmac::SaeFrame) -> anyhow::Result<()>;
    fn wmm_status_req(&self) -> anyhow::Result<()>;
    fn on_link_state_changed(
        &self,
        req: fidl_fullmac::WlanFullmacImplOnLinkStateChangedRequest,
    ) -> anyhow::Result<()>;
    fn set_mac_address(
        &self,
        req: fidl_fullmac::WlanFullmacImplSetMacAddressRequest,
    ) -> anyhow::Result<Result<(), i32>>;
    fn install_apf_packet_filter(
        &self,
        req: fidl_fullmac::WlanFullmacImplInstallApfPacketFilterRequest,
    ) -> anyhow::Result<Result<(), i32>>;
    fn read_apf_packet_filter_data(
        &self,
    ) -> anyhow::Result<Result<fidl_fullmac::WlanFullmacImplReadApfPacketFilterDataResponse, i32>>;
    fn set_apf_packet_filter_enabled(
        &self,
        req: fidl_fullmac::WlanFullmacImplSetApfPacketFilterEnabledRequest,
    ) -> anyhow::Result<Result<(), i32>>;
    fn get_apf_packet_filter_enabled(
        &self,
    ) -> anyhow::Result<Result<fidl_fullmac::WlanFullmacImplGetApfPacketFilterEnabledResponse, i32>>;
    fn start_scheduled_scan(
        &self,
        req: fidl_fullmac::WlanFullmacImplStartScheduledScanRequest,
    ) -> anyhow::Result<Result<(), i32>>;
    fn stop_scheduled_scan(
        &self,
        req: fidl_fullmac::WlanFullmacImplStopScheduledScanRequest,
    ) -> anyhow::Result<Result<(), i32>>;
    fn get_scheduled_scan_enabled(
        &self,
    ) -> anyhow::Result<Result<fidl_fullmac::WlanFullmacImplGetScheduledScanEnabledResponse, i32>>;
}

pub struct FullmacDevice {
    fullmac_impl_sync_proxy: fidl_fullmac::WlanFullmacImpl_SynchronousProxy,
}

/// TODO(https://fxbug.dev/368323681): Users should be notified when the WlanFullmacImpl channel
/// closes.
impl FullmacDevice {
    pub fn new(
        fullmac_impl_sync_proxy: fidl_fullmac::WlanFullmacImpl_SynchronousProxy,
    ) -> FullmacDevice {
        FullmacDevice { fullmac_impl_sync_proxy }
    }
}

impl DeviceOps for FullmacDevice {
    fn init(
        &mut self,
        fullmac_ifc_client_end: ClientEnd<fidl_fullmac::WlanFullmacImplIfcMarker>,
    ) -> Result<fidl::Channel, zx::Status> {
        let req = fidl_fullmac::WlanFullmacImplInitRequest {
            ifc: Some(fullmac_ifc_client_end),
            ..Default::default()
        };
        let resp = self
            .fullmac_impl_sync_proxy
            .init(req, zx::MonotonicInstant::INFINITE)
            .map_err(|e| {
                log::error!("FIDL error on Start: {}", e);
                zx::Status::INTERNAL
            })?
            .map_err(zx::Status::err_from_raw)?;

        resp.sme_channel.ok_or(zx::Status::INVALID_ARGS)
    }

    fn query_device_info(&self) -> anyhow::Result<fidl_fullmac::WlanFullmacImplQueryResponse> {
        self.fullmac_impl_sync_proxy
            .query(zx::MonotonicInstant::INFINITE)
            .context("FIDL error on QueryDeviceInfo")?
            .map_err(|e| format_err!("Driver returned error on QueryDeviceInfo: {}", e))
    }

    fn query_security_support(&self) -> anyhow::Result<fidl_common::SecuritySupport> {
        self.fullmac_impl_sync_proxy
            .query_security_support(zx::MonotonicInstant::INFINITE)
            .context("FIDL error on QuerySecuritySupport")?
            .map_err(|e| format_err!("Driver returned error on QuerySecuritySupport: {}", e))
            .and_then(|support| {
                support.resp.ok_or_else(|| {
                    format_err!("Driver returned empty QuerySecuritySupport response")
                })
            })
    }

    fn query_spectrum_management_support(
        &self,
    ) -> anyhow::Result<fidl_common::SpectrumManagementSupport> {
        self.fullmac_impl_sync_proxy
            .query_spectrum_management_support(zx::MonotonicInstant::INFINITE)
            .context("FIDL error on QuerySpectrumManagementSupport")?
            .map_err(|e| {
                format_err!("Driver returned error on QuerySpectrumManagementSupport: {}", e)
            })
            .and_then(|support| {
                support.resp.ok_or_else(|| {
                    format_err!("Driver returned empty QuerySpectrumManagementSupport response")
                })
            })
    }

    fn query_telemetry_support(&self) -> anyhow::Result<Result<fidl_stats::TelemetrySupport, i32>> {
        self.fullmac_impl_sync_proxy
            .query_telemetry_support(zx::MonotonicInstant::INFINITE)
            .context("FIDL error on QueryTelemetrySupport")
            .and_then(|support| match support {
                Ok(response) => response
                    .resp
                    .ok_or_else(|| {
                        format_err!("Driver returned empty QueryTelemetrySupport response")
                    })
                    .map(Ok),
                Err(e) => Ok(Err(e)),
            })
    }

    fn query_apf_packet_filter_support(
        &self,
    ) -> anyhow::Result<Result<fidl_common::ApfPacketFilterSupport, i32>> {
        self.fullmac_impl_sync_proxy
            .query_apf_packet_filter_support(zx::MonotonicInstant::INFINITE)
            .context("FIDL error on QueryApfPacketFilterSupport")
            .and_then(|support| match support {
                Ok(response) => response
                    .resp
                    .ok_or_else(|| {
                        format_err!("Driver returned empty QueryApfPacketFilterSupport response")
                    })
                    .map(Ok),
                Err(e) => Ok(Err(e)),
            })
    }

    fn start_scan(&self, req: fidl_fullmac::WlanFullmacImplStartScanRequest) -> anyhow::Result<()> {
        self.fullmac_impl_sync_proxy
            .start_scan(&req, zx::MonotonicInstant::INFINITE)
            .context("FIDL error on StartScan")
    }
    fn connect(&self, req: fidl_fullmac::WlanFullmacImplConnectRequest) -> anyhow::Result<()> {
        self.fullmac_impl_sync_proxy
            .connect(&req, zx::MonotonicInstant::INFINITE)
            .context("FIDL error on Connect")
    }
    fn reconnect(&self, req: fidl_fullmac::WlanFullmacImplReconnectRequest) -> anyhow::Result<()> {
        self.fullmac_impl_sync_proxy
            .reconnect(&req, zx::MonotonicInstant::INFINITE)
            .context("FIDL error on Reconnect")
    }
    fn roam(&self, req: fidl_fullmac::WlanFullmacImplRoamRequest) -> anyhow::Result<()> {
        self.fullmac_impl_sync_proxy
            .roam(&req, zx::MonotonicInstant::INFINITE)
            .context("FIDL error on Roam")
    }
    fn auth_resp(&self, resp: fidl_fullmac::WlanFullmacImplAuthRespRequest) -> anyhow::Result<()> {
        self.fullmac_impl_sync_proxy
            .auth_resp(&resp, zx::MonotonicInstant::INFINITE)
            .context("FIDL error on AuthResp")
    }
    fn deauth(&self, req: fidl_fullmac::WlanFullmacImplDeauthRequest) -> anyhow::Result<()> {
        self.fullmac_impl_sync_proxy
            .deauth(&req, zx::MonotonicInstant::INFINITE)
            .context("FIDL error on Deauth")
    }
    fn assoc_resp(
        &self,
        resp: fidl_fullmac::WlanFullmacImplAssocRespRequest,
    ) -> anyhow::Result<()> {
        self.fullmac_impl_sync_proxy
            .assoc_resp(&resp, zx::MonotonicInstant::INFINITE)
            .context("FIDL error on AssocResp")
    }
    fn disassoc(&self, req: fidl_fullmac::WlanFullmacImplDisassocRequest) -> anyhow::Result<()> {
        self.fullmac_impl_sync_proxy
            .disassoc(&req, zx::MonotonicInstant::INFINITE)
            .context("FIDL error on Disassoc")
    }
    fn start_bss(&self, req: fidl_fullmac::WlanFullmacImplStartBssRequest) -> anyhow::Result<()> {
        self.fullmac_impl_sync_proxy
            .start_bss(&req, zx::MonotonicInstant::INFINITE)
            .context("FIDL error on StartBss")
    }
    fn stop_bss(&self, req: fidl_fullmac::WlanFullmacImplStopBssRequest) -> anyhow::Result<()> {
        self.fullmac_impl_sync_proxy
            .stop_bss(&req, zx::MonotonicInstant::INFINITE)
            .context("FIDL error on StopBss")
    }
    fn set_keys(
        &self,
        req: fidl_fullmac::WlanFullmacImplSetKeysRequest,
    ) -> anyhow::Result<fidl_fullmac::WlanFullmacSetKeysResp> {
        self.fullmac_impl_sync_proxy
            .set_keys(&req, zx::MonotonicInstant::INFINITE)
            .context("FIDL error on SetKeysReq")
    }
    fn eapol_tx(&self, req: fidl_fullmac::WlanFullmacImplEapolTxRequest) -> anyhow::Result<()> {
        self.fullmac_impl_sync_proxy
            .eapol_tx(&req, zx::MonotonicInstant::INFINITE)
            .context("FIDL error on EapolTx")
    }
    fn get_iface_stats(&self) -> anyhow::Result<fidl_mlme::GetIfaceStatsResponse> {
        match self
            .fullmac_impl_sync_proxy
            .get_iface_stats(zx::MonotonicInstant::INFINITE)
            .context("FIDL error on GetIfaceStats")?
        {
            Ok(stats) => Ok(fidl_mlme::GetIfaceStatsResponse::Stats(stats)),
            Err(e) => Ok(fidl_mlme::GetIfaceStatsResponse::ErrorStatus(e)),
        }
    }
    fn get_iface_histogram_stats(
        &self,
    ) -> anyhow::Result<fidl_mlme::GetIfaceHistogramStatsResponse> {
        match self
            .fullmac_impl_sync_proxy
            .get_iface_histogram_stats(zx::MonotonicInstant::INFINITE)
            .context("FIDL error on GetIfaceHistogramStats")?
        {
            Ok(stats) => Ok(fidl_mlme::GetIfaceHistogramStatsResponse::Stats(stats)),
            Err(e) => Ok(fidl_mlme::GetIfaceHistogramStatsResponse::ErrorStatus(e)),
        }
    }
    fn get_signal_report(&self) -> anyhow::Result<Result<fidl_stats::SignalReport, i32>> {
        self.fullmac_impl_sync_proxy
            .get_signal_report(zx::MonotonicInstant::INFINITE)
            .context("FIDL error on GetSignalReport")
    }

    fn sae_handshake_resp(
        &self,
        resp: fidl_fullmac::WlanFullmacImplSaeHandshakeRespRequest,
    ) -> anyhow::Result<()> {
        self.fullmac_impl_sync_proxy
            .sae_handshake_resp(&resp, zx::MonotonicInstant::INFINITE)
            .context("FIDL error on SaeHandshakeResp")
    }
    fn sae_frame_tx(&self, frame: fidl_fullmac::SaeFrame) -> anyhow::Result<()> {
        self.fullmac_impl_sync_proxy
            .sae_frame_tx(&frame, zx::MonotonicInstant::INFINITE)
            .context("FIDL error on SaeFrameTx")
    }
    fn wmm_status_req(&self) -> anyhow::Result<()> {
        self.fullmac_impl_sync_proxy
            .wmm_status_req(zx::MonotonicInstant::INFINITE)
            .context("FIDL error on WmmStatusReq")
    }
    fn on_link_state_changed(
        &self,
        req: fidl_fullmac::WlanFullmacImplOnLinkStateChangedRequest,
    ) -> anyhow::Result<()> {
        self.fullmac_impl_sync_proxy
            .on_link_state_changed(&req, zx::MonotonicInstant::INFINITE)
            .context("FIDL error on OnLinkStateChanged")
    }
    fn set_mac_address(
        &self,
        req: fidl_fuchsia_wlan_fullmac::WlanFullmacImplSetMacAddressRequest,
    ) -> anyhow::Result<Result<(), i32>> {
        self.fullmac_impl_sync_proxy
            .set_mac_address(&req.mac_addr, zx::MonotonicInstant::INFINITE)
            .context("FIDL error on SetMacAddress")
    }

    fn install_apf_packet_filter(
        &self,
        req: fidl_fullmac::WlanFullmacImplInstallApfPacketFilterRequest,
    ) -> anyhow::Result<Result<(), i32>> {
        self.fullmac_impl_sync_proxy
            .install_apf_packet_filter(&req, zx::MonotonicInstant::INFINITE)
            .context("FIDL error on InstallApfPacketFilter")
    }

    fn read_apf_packet_filter_data(
        &self,
    ) -> anyhow::Result<Result<fidl_fullmac::WlanFullmacImplReadApfPacketFilterDataResponse, i32>>
    {
        self.fullmac_impl_sync_proxy
            .read_apf_packet_filter_data(zx::MonotonicInstant::INFINITE)
            .context("FIDL error on ReadApfPacketFilterData")
    }

    fn set_apf_packet_filter_enabled(
        &self,
        req: fidl_fullmac::WlanFullmacImplSetApfPacketFilterEnabledRequest,
    ) -> anyhow::Result<Result<(), i32>> {
        self.fullmac_impl_sync_proxy
            .set_apf_packet_filter_enabled(&req, zx::MonotonicInstant::INFINITE)
            .context("FIDL error on SetApfPacketFilterEnabled")
    }

    fn get_apf_packet_filter_enabled(
        &self,
    ) -> anyhow::Result<Result<fidl_fullmac::WlanFullmacImplGetApfPacketFilterEnabledResponse, i32>>
    {
        self.fullmac_impl_sync_proxy
            .get_apf_packet_filter_enabled(zx::MonotonicInstant::INFINITE)
            .context("FIDL error on GetApfPacketFilterEnabled")
    }

    fn start_scheduled_scan(
        &self,
        req: fidl_fullmac::WlanFullmacImplStartScheduledScanRequest,
    ) -> anyhow::Result<Result<(), i32>> {
        self.fullmac_impl_sync_proxy
            .start_scheduled_scan(&req, zx::MonotonicInstant::INFINITE)
            .context("FIDL error on StartScheduledScan")
    }

    fn stop_scheduled_scan(
        &self,
        req: fidl_fullmac::WlanFullmacImplStopScheduledScanRequest,
    ) -> anyhow::Result<Result<(), i32>> {
        self.fullmac_impl_sync_proxy
            .stop_scheduled_scan(&req, zx::MonotonicInstant::INFINITE)
            .context("FIDL error on StopScheduledScan")
    }

    fn get_scheduled_scan_enabled(
        &self,
    ) -> anyhow::Result<Result<fidl_fullmac::WlanFullmacImplGetScheduledScanEnabledResponse, i32>>
    {
        self.fullmac_impl_sync_proxy
            .get_scheduled_scan_enabled(zx::MonotonicInstant::INFINITE)
            .context("FIDL error on GetScheduledScanEnabled")
    }
}

#[cfg(test)]
pub mod test_utils {
    use super::*;
    use fidl_fuchsia_wlan_sme as fidl_sme;
    use fuchsia_sync::Mutex;
    use futures::channel::mpsc;
    use std::sync::Arc;
    use wlan_common::sink::UnboundedSink;

    #[derive(Debug)]
    pub enum DriverCall {
        StartScan {
            req: fidl_fullmac::WlanFullmacImplStartScanRequest,
        },
        StartScheduledScan {
            req: fidl_fullmac::WlanFullmacImplStartScheduledScanRequest,
        },
        StopScheduledScan {
            req: fidl_fullmac::WlanFullmacImplStopScheduledScanRequest,
        },
        GetScheduledScanEnabled,
        ConnectReq {
            req: fidl_fullmac::WlanFullmacImplConnectRequest,
        },
        ReconnectReq {
            req: fidl_fullmac::WlanFullmacImplReconnectRequest,
        },
        RoamReq {
            req: fidl_fullmac::WlanFullmacImplRoamRequest,
        },
        AuthResp {
            resp: fidl_fullmac::WlanFullmacImplAuthRespRequest,
        },
        DeauthReq {
            req: fidl_fullmac::WlanFullmacImplDeauthRequest,
        },
        AssocResp {
            resp: fidl_fullmac::WlanFullmacImplAssocRespRequest,
        },
        Disassoc {
            req: fidl_fullmac::WlanFullmacImplDisassocRequest,
        },
        StartBss {
            req: fidl_fullmac::WlanFullmacImplStartBssRequest,
        },
        StopBss {
            req: fidl_fullmac::WlanFullmacImplStopBssRequest,
        },
        SetKeys {
            req: fidl_fullmac::WlanFullmacImplSetKeysRequest,
        },
        EapolTx {
            req: fidl_fullmac::WlanFullmacImplEapolTxRequest,
        },
        QueryTelemetrySupport,
        QueryApfPacketFilterSupport,
        GetIfaceStats,
        GetIfaceHistogramStats,
        GetSignalReport,
        SaeHandshakeResp {
            resp: fidl_fullmac::WlanFullmacImplSaeHandshakeRespRequest,
        },
        SaeFrameTx {
            frame: fidl_fullmac::SaeFrame,
        },
        WmmStatusReq,
        OnLinkStateChanged {
            req: fidl_fullmac::WlanFullmacImplOnLinkStateChangedRequest,
        },
        SetMacAddress {
            req: fidl_fullmac::WlanFullmacImplSetMacAddressRequest,
        },
        InstallApfPacketFilter {
            req: fidl_fullmac::WlanFullmacImplInstallApfPacketFilterRequest,
        },
        ReadApfPacketFilterData,
        SetApfPacketFilterEnabled {
            req: fidl_fullmac::WlanFullmacImplSetApfPacketFilterEnabledRequest,
        },
        GetApfPacketFilterEnabled,
    }

    pub struct FakeFullmacDeviceMocks {
        pub start_fn_status_mock: Option<zx::sys::zx_status_t>,

        // Note: anyhow::Error isn't cloneable, so the query mocks are all optionals to make this
        // easier to work with.
        //
        // If any of the query mocks are None, then an Err is returned from DeviceOps with an empty
        // error message.
        pub query_device_info_mock: Option<fidl_fullmac::WlanFullmacImplQueryResponse>,
        pub query_security_support_mock: Option<fidl_common::SecuritySupport>,
        pub query_spectrum_management_support_mock: Option<fidl_common::SpectrumManagementSupport>,
        pub query_telemetry_support_mock: Option<Result<fidl_stats::TelemetrySupport, i32>>,
        pub query_apf_packet_filter_support_mock:
            Option<Result<fidl_common::ApfPacketFilterSupport, i32>>,

        pub set_keys_resp_mock: Option<fidl_fullmac::WlanFullmacSetKeysResp>,
        pub get_iface_stats_mock: Option<fidl_mlme::GetIfaceStatsResponse>,
        pub get_iface_histogram_stats_mock: Option<fidl_mlme::GetIfaceHistogramStatsResponse>,
        pub get_signal_report_mock: Option<Result<fidl_stats::SignalReport, i32>>,
        pub install_apf_packet_filter_mock: Option<Result<(), i32>>,
        pub read_apf_packet_filter_data_mock:
            Option<Result<fidl_fullmac::WlanFullmacImplReadApfPacketFilterDataResponse, i32>>,
        pub set_apf_packet_filter_enabled_mock: Option<Result<(), i32>>,
        pub get_apf_packet_filter_enabled_mock:
            Option<Result<fidl_fullmac::WlanFullmacImplGetApfPacketFilterEnabledResponse, i32>>,
        pub get_scheduled_scan_enabled_mock:
            Option<Result<fidl_fullmac::WlanFullmacImplGetScheduledScanEnabledResponse, i32>>,
        pub fullmac_ifc_client_end: Option<ClientEnd<fidl_fullmac::WlanFullmacImplIfcMarker>>,
    }

    unsafe impl Send for FakeFullmacDevice {}
    pub struct FakeFullmacDevice {
        pub usme_bootstrap_client_end:
            Option<fidl::endpoints::ClientEnd<fidl_sme::UsmeBootstrapMarker>>,
        pub usme_bootstrap_server_end:
            Option<fidl::endpoints::ServerEnd<fidl_sme::UsmeBootstrapMarker>>,
        driver_call_sender: UnboundedSink<DriverCall>,

        // This is boxed because tests want a reference to this to check captured calls, but in
        // production we pass ownership of the DeviceOps to FullmacMlme. This avoids changing
        // ownership semantics for tests.
        pub mocks: Arc<Mutex<FakeFullmacDeviceMocks>>,
    }

    impl FakeFullmacDevice {
        pub fn new() -> (Self, mpsc::UnboundedReceiver<DriverCall>) {
            // Create a channel for SME requests, to be surfaced by init().
            let (usme_bootstrap_client_end, usme_bootstrap_server_end) =
                fidl::endpoints::create_endpoints::<fidl_sme::UsmeBootstrapMarker>();

            let (driver_call_sender, driver_call_receiver) = mpsc::unbounded();

            let device = Self {
                usme_bootstrap_client_end: Some(usme_bootstrap_client_end),
                usme_bootstrap_server_end: Some(usme_bootstrap_server_end),
                driver_call_sender: UnboundedSink::new(driver_call_sender),
                mocks: Arc::new(Mutex::new(FakeFullmacDeviceMocks {
                    fullmac_ifc_client_end: None,
                    start_fn_status_mock: None,
                    query_device_info_mock: Some(fidl_fullmac::WlanFullmacImplQueryResponse {
                        sta_addr: Some([0u8; 6]),
                        factory_addr: Some([0u8; 6]),
                        role: Some(fidl_common::WlanMacRole::Client),
                        band_caps: Some(vec![]),
                        ..Default::default()
                    }),
                    query_security_support_mock: Some(fidl_common::SecuritySupport {
                        sae: Some(fidl_common::SaeFeature {
                            driver_handler_supported: Some(false),
                            sme_handler_supported: Some(true),
                            hash_to_element_supported: Some(false),
                            ..Default::default()
                        }),
                        mfp: Some(fidl_common::MfpFeature {
                            supported: Some(false),
                            ..Default::default()
                        }),
                        owe: Some(fidl_common::OweFeature {
                            supported: Some(false),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    query_spectrum_management_support_mock: Some(
                        fidl_common::SpectrumManagementSupport {
                            dfs: Some(fidl_common::DfsFeature {
                                supported: Some(false),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    ),
                    query_telemetry_support_mock: Some(Ok(fidl_stats::TelemetrySupport {
                        ..Default::default()
                    })),
                    query_apf_packet_filter_support_mock: Some(Ok(
                        fidl_common::ApfPacketFilterSupport {
                            supported: Some(false),
                            ..Default::default()
                        },
                    )),
                    get_signal_report_mock: Some(Ok(fidl_stats::SignalReport {
                        ..Default::default()
                    })),
                    set_keys_resp_mock: None,
                    get_iface_stats_mock: None,
                    get_iface_histogram_stats_mock: None,
                    install_apf_packet_filter_mock: Some(Ok(())),
                    read_apf_packet_filter_data_mock: Some(Ok(
                        fidl_fullmac::WlanFullmacImplReadApfPacketFilterDataResponse {
                            memory: Some(vec![]),
                            ..Default::default()
                        },
                    )),
                    set_apf_packet_filter_enabled_mock: Some(Ok(())),
                    get_apf_packet_filter_enabled_mock: Some(Ok(
                        fidl_fullmac::WlanFullmacImplGetApfPacketFilterEnabledResponse {
                            enabled: Some(false),
                            ..Default::default()
                        },
                    )),
                    get_scheduled_scan_enabled_mock: Some(Ok(
                        fidl_fullmac::WlanFullmacImplGetScheduledScanEnabledResponse {
                            active_txn_ids: Some(vec![]),
                            ..Default::default()
                        },
                    )),
                })),
            };

            (device, driver_call_receiver)
        }
    }

    impl DeviceOps for FakeFullmacDevice {
        fn init(
            &mut self,
            fullmac_ifc_client_end: ClientEnd<fidl_fullmac::WlanFullmacImplIfcMarker>,
        ) -> Result<fidl::Channel, zx::Status> {
            let mut mocks = self.mocks.lock();

            mocks.fullmac_ifc_client_end = Some(fullmac_ifc_client_end);
            match mocks.start_fn_status_mock {
                Some(status) => Err(zx::Status::err_from_raw(status)),

                // Start can only be called once since this moves usme_bootstrap_server_end.
                None => Ok(self.usme_bootstrap_server_end.take().unwrap().into_channel()),
            }
        }

        fn query_device_info(&self) -> anyhow::Result<fidl_fullmac::WlanFullmacImplQueryResponse> {
            self.mocks.lock().query_device_info_mock.clone().ok_or_else(|| format_err!(""))
        }

        fn query_security_support(&self) -> anyhow::Result<fidl_common::SecuritySupport> {
            self.mocks.lock().query_security_support_mock.clone().ok_or_else(|| format_err!(""))
        }

        fn query_spectrum_management_support(
            &self,
        ) -> anyhow::Result<fidl_common::SpectrumManagementSupport> {
            self.mocks
                .lock()
                .query_spectrum_management_support_mock
                .clone()
                .ok_or_else(|| format_err!(""))
        }

        fn query_telemetry_support(
            &self,
        ) -> anyhow::Result<Result<fidl_stats::TelemetrySupport, i32>> {
            self.driver_call_sender.send(DriverCall::QueryTelemetrySupport);
            self.mocks.lock().query_telemetry_support_mock.clone().ok_or_else(|| format_err!(""))
        }

        fn query_apf_packet_filter_support(
            &self,
        ) -> anyhow::Result<Result<fidl_common::ApfPacketFilterSupport, i32>> {
            self.driver_call_sender.send(DriverCall::QueryApfPacketFilterSupport);
            self.mocks.lock().query_apf_packet_filter_support_mock.clone().ok_or_else(|| {
                format_err!("query_apf_packet_filter_support_mock is None in FakeFullmacDevice")
            })
        }

        // Cannot mark fn unsafe because it has to match fn signature in FullDeviceInterface
        fn start_scan(
            &self,
            req: fidl_fullmac::WlanFullmacImplStartScanRequest,
        ) -> anyhow::Result<()> {
            self.driver_call_sender.send(DriverCall::StartScan { req });
            Ok(())
        }

        fn connect(&self, req: fidl_fullmac::WlanFullmacImplConnectRequest) -> anyhow::Result<()> {
            self.driver_call_sender.send(DriverCall::ConnectReq { req });
            Ok(())
        }
        fn reconnect(
            &self,
            req: fidl_fullmac::WlanFullmacImplReconnectRequest,
        ) -> anyhow::Result<()> {
            self.driver_call_sender.send(DriverCall::ReconnectReq { req });
            Ok(())
        }
        fn roam(&self, req: fidl_fullmac::WlanFullmacImplRoamRequest) -> anyhow::Result<()> {
            self.driver_call_sender.send(DriverCall::RoamReq { req });
            Ok(())
        }
        fn auth_resp(
            &self,
            resp: fidl_fullmac::WlanFullmacImplAuthRespRequest,
        ) -> anyhow::Result<()> {
            self.driver_call_sender.send(DriverCall::AuthResp { resp });
            Ok(())
        }
        fn deauth(&self, req: fidl_fullmac::WlanFullmacImplDeauthRequest) -> anyhow::Result<()> {
            self.driver_call_sender.send(DriverCall::DeauthReq { req });
            Ok(())
        }
        fn assoc_resp(
            &self,
            resp: fidl_fullmac::WlanFullmacImplAssocRespRequest,
        ) -> anyhow::Result<()> {
            self.driver_call_sender.send(DriverCall::AssocResp { resp });
            Ok(())
        }
        fn disassoc(
            &self,
            req: fidl_fullmac::WlanFullmacImplDisassocRequest,
        ) -> anyhow::Result<()> {
            self.driver_call_sender.send(DriverCall::Disassoc { req });
            Ok(())
        }
        fn start_bss(
            &self,
            req: fidl_fullmac::WlanFullmacImplStartBssRequest,
        ) -> anyhow::Result<()> {
            self.driver_call_sender.send(DriverCall::StartBss { req });
            Ok(())
        }
        fn stop_bss(&self, req: fidl_fullmac::WlanFullmacImplStopBssRequest) -> anyhow::Result<()> {
            self.driver_call_sender.send(DriverCall::StopBss { req });
            Ok(())
        }
        fn set_keys(
            &self,
            req: fidl_fullmac::WlanFullmacImplSetKeysRequest,
        ) -> anyhow::Result<fidl_fullmac::WlanFullmacSetKeysResp> {
            let num_keys = req.keylist.as_ref().unwrap().len();
            self.driver_call_sender.send(DriverCall::SetKeys { req });
            match &self.mocks.lock().set_keys_resp_mock {
                Some(resp) => Ok(resp.clone()),
                None => {
                    Ok(fidl_fullmac::WlanFullmacSetKeysResp { statuslist: vec![0i32; num_keys] })
                }
            }
        }
        fn eapol_tx(&self, req: fidl_fullmac::WlanFullmacImplEapolTxRequest) -> anyhow::Result<()> {
            self.driver_call_sender.send(DriverCall::EapolTx { req });
            Ok(())
        }
        fn get_iface_stats(&self) -> anyhow::Result<fidl_mlme::GetIfaceStatsResponse> {
            self.driver_call_sender.send(DriverCall::GetIfaceStats);
            Ok(self.mocks.lock().get_iface_stats_mock.clone().unwrap_or(
                fidl_mlme::GetIfaceStatsResponse::ErrorStatus(zx::sys::ZX_ERR_NOT_SUPPORTED),
            ))
        }
        fn get_iface_histogram_stats(
            &self,
        ) -> anyhow::Result<fidl_mlme::GetIfaceHistogramStatsResponse> {
            self.driver_call_sender.send(DriverCall::GetIfaceHistogramStats);
            Ok(self.mocks.lock().get_iface_histogram_stats_mock.clone().unwrap_or(
                fidl_mlme::GetIfaceHistogramStatsResponse::ErrorStatus(
                    zx::sys::ZX_ERR_NOT_SUPPORTED,
                ),
            ))
        }
        fn get_signal_report(&self) -> anyhow::Result<Result<fidl_stats::SignalReport, i32>> {
            self.driver_call_sender.send(DriverCall::GetSignalReport);
            self.mocks.lock().get_signal_report_mock.clone().ok_or_else(|| format_err!(""))
        }
        fn sae_handshake_resp(
            &self,
            resp: fidl_fullmac::WlanFullmacImplSaeHandshakeRespRequest,
        ) -> anyhow::Result<()> {
            self.driver_call_sender.send(DriverCall::SaeHandshakeResp { resp });
            Ok(())
        }
        fn sae_frame_tx(&self, frame: fidl_fullmac::SaeFrame) -> anyhow::Result<()> {
            self.driver_call_sender.send(DriverCall::SaeFrameTx { frame });
            Ok(())
        }
        fn wmm_status_req(&self) -> anyhow::Result<()> {
            self.driver_call_sender.send(DriverCall::WmmStatusReq);
            Ok(())
        }
        fn on_link_state_changed(
            &self,
            req: fidl_fullmac::WlanFullmacImplOnLinkStateChangedRequest,
        ) -> anyhow::Result<()> {
            self.driver_call_sender.send(DriverCall::OnLinkStateChanged { req });
            Ok(())
        }
        fn set_mac_address(
            &self,
            req: fidl_fuchsia_wlan_fullmac::WlanFullmacImplSetMacAddressRequest,
        ) -> anyhow::Result<Result<(), i32>> {
            self.driver_call_sender.send(DriverCall::SetMacAddress { req });
            Ok(Ok(()))
        }

        fn install_apf_packet_filter(
            &self,
            req: fidl_fullmac::WlanFullmacImplInstallApfPacketFilterRequest,
        ) -> anyhow::Result<Result<(), i32>> {
            self.driver_call_sender.send(DriverCall::InstallApfPacketFilter { req });
            self.mocks.lock().install_apf_packet_filter_mock.clone().ok_or_else(|| {
                format_err!("install_apf_packet_filter_mock is None in FakeFullmacDevice")
            })
        }

        fn read_apf_packet_filter_data(
            &self,
        ) -> anyhow::Result<Result<fidl_fullmac::WlanFullmacImplReadApfPacketFilterDataResponse, i32>>
        {
            self.driver_call_sender.send(DriverCall::ReadApfPacketFilterData);
            self.mocks.lock().read_apf_packet_filter_data_mock.clone().ok_or_else(|| {
                format_err!("read_apf_packet_filter_data_mock is None in FakeFullmacDevice")
            })
        }

        fn set_apf_packet_filter_enabled(
            &self,
            req: fidl_fullmac::WlanFullmacImplSetApfPacketFilterEnabledRequest,
        ) -> anyhow::Result<Result<(), i32>> {
            self.driver_call_sender.send(DriverCall::SetApfPacketFilterEnabled { req });
            self.mocks.lock().set_apf_packet_filter_enabled_mock.clone().ok_or_else(|| {
                format_err!("set_apf_packet_filter_enabled_mock is None in FakeFullmacDevice")
            })
        }

        fn get_apf_packet_filter_enabled(
            &self,
        ) -> anyhow::Result<
            Result<fidl_fullmac::WlanFullmacImplGetApfPacketFilterEnabledResponse, i32>,
        > {
            self.driver_call_sender.send(DriverCall::GetApfPacketFilterEnabled);
            self.mocks.lock().get_apf_packet_filter_enabled_mock.clone().ok_or_else(|| {
                format_err!("get_apf_packet_filter_enabled_mock is None in FakeFullmacDevice")
            })
        }

        fn get_scheduled_scan_enabled(
            &self,
        ) -> anyhow::Result<Result<fidl_fullmac::WlanFullmacImplGetScheduledScanEnabledResponse, i32>>
        {
            self.driver_call_sender.send(DriverCall::GetScheduledScanEnabled);
            self.mocks.lock().get_scheduled_scan_enabled_mock.clone().ok_or_else(|| {
                format_err!("get_scheduled_scan_enabled_mock is None in FakeFullmacDevice")
            })
        }

        fn start_scheduled_scan(
            &self,
            req: fidl_fullmac::WlanFullmacImplStartScheduledScanRequest,
        ) -> anyhow::Result<Result<(), i32>> {
            self.driver_call_sender.send(DriverCall::StartScheduledScan { req });
            Ok(Ok(()))
        }

        fn stop_scheduled_scan(
            &self,
            req: fidl_fullmac::WlanFullmacImplStopScheduledScanRequest,
        ) -> anyhow::Result<Result<(), i32>> {
            self.driver_call_sender.send(DriverCall::StopScheduledScan { req });
            Ok(Ok(()))
        }
    }
}
