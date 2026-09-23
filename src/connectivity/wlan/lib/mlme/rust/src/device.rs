// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::common::mac::WlanGi;
use crate::error::Error;
use anyhow::format_err;
use fdf::ArenaStaticBox;
use fidl_fuchsia_wlan_common as fidl_common;
use fidl_fuchsia_wlan_driver as fidl_driver_common;
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_mlme as fidl_mlme;
use fidl_fuchsia_wlan_softmac as fidl_softmac;
use fuchsia_trace as trace;
use futures::Future;
use futures::channel::mpsc;
use ieee80211::MacAddr;
use log::error;
use std::fmt::Display;
use std::mem;
use std::sync::Arc;
use trace::Id as TraceId;
use wlan_common::mac::FrameControl;
use wlan_common::{TimeUnit, tx_vector};
use wlan_ffi_transport::{EthernetRx, EthernetTx, FfiEthernetTx, FfiWlanRx, WlanRx, WlanTx};
use wlan_trace as wtrace;

pub use test_utils::*;

#[derive(Debug, PartialEq)]
pub struct LinkStatus(u32);
impl LinkStatus {
    pub const DOWN: Self = Self(0);
    pub const UP: Self = Self(1);
}

impl From<fidl_mlme::ControlledPortState> for LinkStatus {
    fn from(state: fidl_mlme::ControlledPortState) -> Self {
        match state {
            fidl_mlme::ControlledPortState::Open => Self::UP,
            fidl_mlme::ControlledPortState::Closed => Self::DOWN,
        }
    }
}

pub struct Device {
    ethernet_rx: EthernetRx,
    ethernet_tx: Option<EthernetTx>,
    wlan_rx: Option<WlanRx>,
    wlan_tx: WlanTx,
    wlan_softmac_bridge_proxy: fidl_softmac::WlanSoftmacBridgeProxy,
    minstrel: Option<crate::MinstrelWrapper>,
    event_receiver: Option<mpsc::UnboundedReceiver<fidl_mlme::MlmeEvent>>,
    event_sink: mpsc::UnboundedSender<fidl_mlme::MlmeEvent>,
}

impl Device {
    pub fn new(
        wlan_softmac_bridge_proxy: fidl_softmac::WlanSoftmacBridgeProxy,
        ethernet_rx: EthernetRx,
        wlan_tx: WlanTx,
    ) -> Device {
        let (event_sink, event_receiver) = mpsc::unbounded();
        Device {
            ethernet_rx,
            ethernet_tx: None,
            wlan_rx: None,
            wlan_tx,
            wlan_softmac_bridge_proxy,
            minstrel: None,
            event_receiver: Some(event_receiver),
            event_sink,
        }
    }

    // TODO(https://fxbug.dev/356119431): Share this with fullmac.
    fn flatten_and_log_error<T>(
        method_name: impl Display,
        result: Result<Result<T, zx::sys::zx_status_t>, fidl::Error>,
    ) -> Result<T, zx::Status> {
        result
            .map_err(|fidl_error| {
                error!("FIDL error during {}: {:?}", method_name, fidl_error);
                zx::Status::INTERNAL
            })?
            .map_err(|status| {
                error!("{} failed: {:?}", method_name, status);
                zx::Status::err_from_raw(status)
            })
    }
}

const REQUIRED_WLAN_HEADER_LEN: usize = 10;
const PEER_ADDR_OFFSET: usize = 4;

/// This trait abstracts operations performed by the vendor driver and ethernet device.
pub trait DeviceOps {
    fn wlan_softmac_query_response(
        &mut self,
    ) -> impl Future<Output = Result<fidl_softmac::WlanSoftmacQueryResponse, zx::Status>>;
    fn discovery_support(
        &mut self,
    ) -> impl Future<Output = Result<fidl_softmac::DiscoverySupport, zx::Status>>;
    fn mac_sublayer_support(
        &mut self,
    ) -> impl Future<Output = Result<fidl_common::MacSublayerSupport, zx::Status>>;
    fn security_support(
        &mut self,
    ) -> impl Future<Output = Result<fidl_common::SecuritySupport, zx::Status>>;
    fn spectrum_management_support(
        &mut self,
    ) -> impl Future<Output = Result<fidl_common::SpectrumManagementSupport, zx::Status>>;
    fn start(
        &mut self,
        ifc_bridge: fidl::endpoints::ClientEnd<fidl_softmac::WlanSoftmacIfcBridgeMarker>,
        ethernet_tx: EthernetTx,
        wlan_rx: WlanRx,
    ) -> impl Future<Output = Result<fidl::Channel, zx::Status>>;
    fn deliver_eth_frame(&mut self, packet: &[u8]) -> Result<(), zx::Status>;
    /// Sends the slice corresponding to |buffer| as a frame over the air. If the
    /// caller does not pass an |async_id| to this function, then this function will
    /// generate its own |async_id| and end the trace if an error occurs.
    fn send_wlan_frame(
        &mut self,
        buffer: ArenaStaticBox<[u8]>,
        tx_flags: fidl_softmac::WlanTxInfoFlags,
        async_id: Option<TraceId>,
    ) -> Result<(), zx::Status>;

    fn set_ethernet_status(
        &mut self,
        status: LinkStatus,
    ) -> impl Future<Output = Result<(), zx::Status>>;
    fn set_ethernet_up(&mut self) -> impl Future<Output = Result<(), zx::Status>> {
        self.set_ethernet_status(LinkStatus::UP)
    }
    fn set_ethernet_down(&mut self) -> impl Future<Output = Result<(), zx::Status>> {
        self.set_ethernet_status(LinkStatus::DOWN)
    }
    fn set_channel(
        &mut self,
        primary: fidl_ieee80211::ChannelNumber,
        bandwidth: fidl_ieee80211::ChannelBandwidth,
        vht_secondary_80_channel: fidl_ieee80211::ChannelNumber,
    ) -> impl Future<Output = Result<(), zx::Status>>;
    fn set_mac_address(
        &mut self,
        mac_addr: fidl_ieee80211::MacAddr,
    ) -> impl Future<Output = Result<(), zx::Status>>;
    fn start_passive_scan(
        &mut self,
        request: &fidl_softmac::WlanSoftmacBaseStartPassiveScanRequest,
    ) -> impl Future<Output = Result<fidl_softmac::WlanSoftmacBaseStartPassiveScanResponse, zx::Status>>;
    fn start_active_scan(
        &mut self,
        request: &fidl_softmac::WlanSoftmacStartActiveScanRequest,
    ) -> impl Future<Output = Result<fidl_softmac::WlanSoftmacBaseStartActiveScanResponse, zx::Status>>;
    fn cancel_scan(
        &mut self,
        request: &fidl_softmac::WlanSoftmacBaseCancelScanRequest,
    ) -> impl Future<Output = Result<(), zx::Status>>;
    fn join_bss(
        &mut self,
        request: &fidl_driver_common::JoinBssRequest,
    ) -> impl Future<Output = Result<(), zx::Status>>;
    fn enable_beaconing(
        &mut self,
        request: fidl_softmac::WlanSoftmacBaseEnableBeaconingRequest,
    ) -> impl Future<Output = Result<(), zx::Status>>;
    fn disable_beaconing(&mut self) -> impl Future<Output = Result<(), zx::Status>>;
    fn install_key(
        &mut self,
        key_configuration: &fidl_softmac::WlanKeyConfiguration,
    ) -> impl Future<Output = Result<(), zx::Status>>;
    fn notify_association_complete(
        &mut self,
        assoc_cfg: fidl_softmac::WlanAssociationConfig,
    ) -> impl Future<Output = Result<(), zx::Status>>;
    fn clear_association(
        &mut self,
        request: &fidl_softmac::WlanSoftmacBaseClearAssociationRequest,
    ) -> impl Future<Output = Result<(), zx::Status>>;
    fn update_wmm_parameters(
        &mut self,
        request: &fidl_softmac::WlanSoftmacBaseUpdateWmmParametersRequest,
    ) -> impl Future<Output = Result<(), zx::Status>>;
    fn take_mlme_event_stream(&mut self) -> Option<mpsc::UnboundedReceiver<fidl_mlme::MlmeEvent>>;
    fn send_mlme_event(&mut self, event: fidl_mlme::MlmeEvent) -> Result<(), anyhow::Error>;
    fn set_minstrel(&mut self, minstrel: crate::MinstrelWrapper);
    fn minstrel(&mut self) -> Option<crate::MinstrelWrapper>;
    fn tx_vector_idx(
        &mut self,
        frame_control: &FrameControl,
        peer_addr: &MacAddr,
        flags: fidl_softmac::WlanTxInfoFlags,
    ) -> tx_vector::TxVecIdx {
        self.minstrel()
            .as_ref()
            .and_then(|minstrel| {
                minstrel.lock().get_tx_vector_idx(frame_control, &peer_addr, flags)
            })
            .unwrap_or_else(|| {
                // We either don't have minstrel, or minstrel failed to generate a tx vector.
                // Use a reasonable default value instead.
                // Note: This is only effective if the underlying device meets both criteria below:
                // 1. Does not support tx status report.
                // 2. Honors our instruction on tx_vector to use.
                // TODO(https://fxbug.dev/42103583): Choose an optimal MCS for management frames
                // TODO(https://fxbug.dev/42119762): Log stats about minstrel usage vs default tx vector.
                let mcs_idx = if frame_control.is_data() { 7 } else { 3 };
                tx_vector::TxVector::new(
                    fidl_ieee80211::WlanPhyType::Erp,
                    WlanGi::G_800NS,
                    fidl_ieee80211::ChannelBandwidth::Cbw20,
                    mcs_idx,
                )
                .unwrap()
                .to_idx()
            })
    }
}

pub async fn try_query(
    device: &mut impl DeviceOps,
) -> Result<fidl_softmac::WlanSoftmacQueryResponse, Error> {
    device
        .wlan_softmac_query_response()
        .await
        .map_err(|status| Error::Status(String::from("Failed to query device."), status))
}

pub async fn try_query_iface_mac(device: &mut impl DeviceOps) -> Result<MacAddr, Error> {
    try_query(device).await.and_then(|query_response| {
        query_response.sta_addr.map(From::from).ok_or_else(|| {
            Error::Internal(format_err!(
                "Required field not set in device query response: iface MAC"
            ))
        })
    })
}

pub async fn try_query_discovery_support(
    device: &mut impl DeviceOps,
) -> Result<fidl_softmac::DiscoverySupport, Error> {
    device.discovery_support().await.map_err(|status| {
        Error::Status(String::from("Failed to query discovery support for device."), status)
    })
}

pub async fn try_query_mac_sublayer_support(
    device: &mut impl DeviceOps,
) -> Result<fidl_common::MacSublayerSupport, Error> {
    device.mac_sublayer_support().await.map_err(|status| {
        Error::Status(String::from("Failed to query MAC sublayer support for device."), status)
    })
}

pub async fn try_query_security_support(
    device: &mut impl DeviceOps,
) -> Result<fidl_common::SecuritySupport, Error> {
    device.security_support().await.map_err(|status| {
        Error::Status(String::from("Failed to query security support for device."), status)
    })
}

pub async fn try_query_spectrum_management_support(
    device: &mut impl DeviceOps,
) -> Result<fidl_common::SpectrumManagementSupport, Error> {
    device.spectrum_management_support().await.map_err(|status| {
        Error::Status(
            String::from("Failed to query spectrum management support for device."),
            status,
        )
    })
}

impl DeviceOps for Device {
    async fn wlan_softmac_query_response(
        &mut self,
    ) -> Result<fidl_softmac::WlanSoftmacQueryResponse, zx::Status> {
        Self::flatten_and_log_error("Query", self.wlan_softmac_bridge_proxy.query().await)
    }

    async fn discovery_support(&mut self) -> Result<fidl_softmac::DiscoverySupport, zx::Status> {
        Self::flatten_and_log_error(
            "QueryDiscoverySupport",
            self.wlan_softmac_bridge_proxy.query_discovery_support().await,
        )
    }

    async fn mac_sublayer_support(
        &mut self,
    ) -> Result<fidl_common::MacSublayerSupport, zx::Status> {
        Self::flatten_and_log_error(
            "QueryMacSublayerSupport",
            self.wlan_softmac_bridge_proxy.query_mac_sublayer_support().await,
        )
    }

    async fn security_support(&mut self) -> Result<fidl_common::SecuritySupport, zx::Status> {
        Self::flatten_and_log_error(
            "QuerySecuritySupport",
            self.wlan_softmac_bridge_proxy.query_security_support().await,
        )
    }

    async fn spectrum_management_support(
        &mut self,
    ) -> Result<fidl_common::SpectrumManagementSupport, zx::Status> {
        Self::flatten_and_log_error(
            "QuerySpectrumManagementSupport",
            self.wlan_softmac_bridge_proxy.query_spectrum_management_support().await,
        )
    }

    async fn start(
        &mut self,
        ifc_bridge: fidl::endpoints::ClientEnd<fidl_softmac::WlanSoftmacIfcBridgeMarker>,
        ethernet_tx: EthernetTx,
        wlan_rx: WlanRx,
    ) -> Result<fidl::Channel, zx::Status> {
        // Safety: These calls are safe because `ethernet_tx` and
        // `wlan_rx` will outlive all uses of the constructed
        // `FfiEthernetTx` and `FfiWlanRx` across the FFI boundary. This includes
        // during unbind when the C++ portion of wlansoftmac will
        // ensure no additional calls will be made through
        // `FfiEthernetTx` and `FfiWlanRx` after unbind begins.
        let mut ffi_ethernet_tx = unsafe { ethernet_tx.to_ffi() };
        let mut ffi_wlan_rx = unsafe { wlan_rx.to_ffi() };

        // Re-bind `ffi_ethernet_tx` and `ffi_wlan_rx` to exclusive references that stay in scope across the
        // await. The exclusive references guarantees the consumer of the references across the FIDL
        // hop is the only accessor and that the references are valid during the await.
        let ffi_ethernet_tx = &mut ffi_ethernet_tx;
        let ffi_wlan_rx = &mut ffi_wlan_rx;

        self.ethernet_tx = Some(ethernet_tx);
        self.wlan_rx = Some(wlan_rx);

        self.wlan_softmac_bridge_proxy
            .start(
                ifc_bridge,
                ffi_ethernet_tx as *mut FfiEthernetTx as u64,
                ffi_wlan_rx as *mut FfiWlanRx as u64,
            )
            .await
            .map_err(|error| {
                error!("Start failed with FIDL error: {:?}", error);
                zx::Status::INTERNAL
            })?
            .map_err(zx::Status::err_from_raw)
    }

    fn deliver_eth_frame(&mut self, packet: &[u8]) -> Result<(), zx::Status> {
        wtrace::duration!("Device::deliver_eth_frame");
        self.ethernet_rx.transfer(&fidl_softmac::EthernetRxTransferRequest {
            packet_address: Some(packet.as_ptr() as u64),
            packet_size: Some(packet.len() as u64),
            ..Default::default()
        })
    }

    fn send_wlan_frame(
        &mut self,
        buffer: ArenaStaticBox<[u8]>,
        mut tx_flags: fidl_softmac::WlanTxInfoFlags,
        async_id: Option<TraceId>,
    ) -> Result<(), zx::Status> {
        let async_id_provided = async_id.is_some();
        let async_id = async_id.unwrap_or_else(|| {
            let async_id = TraceId::new();
            wtrace::async_begin_wlansoftmac_tx(async_id, "mlme");
            async_id
        });
        wtrace::duration!("Device::send_data_frame");

        let (arena, mut buffer) = ArenaStaticBox::into_raw(buffer);

        // Safety: buffer points to a valid allocation of a slice, and arena remains
        // is always in scope while buffer is in use.
        let buffer = unsafe { buffer.as_mut() };
        if buffer.len() < REQUIRED_WLAN_HEADER_LEN {
            let status = zx::Status::BUFFER_TOO_SMALL;
            if !async_id_provided {
                wtrace::async_end_wlansoftmac_tx(async_id, status);
            }
            return Err(status);
        }
        // Unwrap is safe because FrameControl is the correct size.
        const _: () =
            assert!(mem::size_of::<FrameControl>() == 2, "Size of FrameControl is not 2 bytes");
        let frame_control = zerocopy::Ref::into_ref(
            zerocopy::Ref::<&[u8], FrameControl>::from_bytes(&buffer[0..=1]).unwrap(),
        );
        if frame_control.protected() {
            tx_flags |= fidl_softmac::WlanTxInfoFlags::PROTECTED;
        }
        let peer_addr: MacAddr = {
            let mut peer_addr = [0u8; 6];
            // Safety: buffer is points to a slice of sufficient length
            peer_addr.copy_from_slice(&buffer[PEER_ADDR_OFFSET..PEER_ADDR_OFFSET + 6]);
            peer_addr.into()
        };
        let tx_vector_idx = self.tx_vector_idx(frame_control, &peer_addr, tx_flags);

        let tx_info = wlan_common::tx_vector::TxVector::from_idx(tx_vector_idx)
            .to_fidl_tx_info(tx_flags, self.minstrel.is_some());
        let packet_address = Some(buffer.as_ptr() as *mut u8 as u64);
        let packet_size = Some(buffer.len() as u64);

        self.wlan_tx
            .transfer(&fidl_softmac::WlanTxTransferRequest {
                arena: Some(arena.as_ptr() as u64),
                packet_size,
                packet_address,
                packet_info: Some(tx_info),
                async_id: Some(async_id.into()),
                ..Default::default()
            })
            .map_err(|s| {
                if !async_id_provided {
                    wtrace::async_end_wlansoftmac_tx(async_id, s);
                }
                s
            })
    }

    async fn set_ethernet_status(&mut self, status: LinkStatus) -> Result<(), zx::Status> {
        self.wlan_softmac_bridge_proxy.set_ethernet_status(status.0).await.map_err(|error| {
            error!("SetEthernetStatus failed with FIDL error: {:?}", error);
            zx::Status::INTERNAL
        })
    }

    async fn set_channel(
        &mut self,
        primary: fidl_ieee80211::ChannelNumber,
        bandwidth: fidl_ieee80211::ChannelBandwidth,
        vht_secondary_80_channel: fidl_ieee80211::ChannelNumber,
    ) -> Result<(), zx::Status> {
        self.wlan_softmac_bridge_proxy
            .set_channel(&fidl_softmac::WlanSoftmacBaseSetChannelRequest {
                primary: Some(primary),
                bandwidth: Some(bandwidth),
                vht_secondary_80_channel: Some(vht_secondary_80_channel),
                ..Default::default()
            })
            .await
            .map_err(|error| {
                error!("SetChannel failed with FIDL error: {:?}", error);
                zx::Status::INTERNAL
            })?
            .map_err(zx::Status::err_from_raw)
    }

    /// Setting the MAC address is currently unavailable in the softmac.fidl, due to stable
    /// versioning limitations. It may be added in the future to support MAC address changes on
    /// softmac devices.
    async fn set_mac_address(
        &mut self,
        _mac_addr: fidl_fuchsia_wlan_ieee80211::MacAddr,
    ) -> Result<(), zx::Status> {
        Err(zx::Status::NOT_SUPPORTED)
    }

    async fn start_passive_scan(
        &mut self,
        request: &fidl_softmac::WlanSoftmacBaseStartPassiveScanRequest,
    ) -> Result<fidl_softmac::WlanSoftmacBaseStartPassiveScanResponse, zx::Status> {
        Self::flatten_and_log_error(
            "StartPassiveScan",
            self.wlan_softmac_bridge_proxy.start_passive_scan(request).await,
        )
    }

    async fn start_active_scan(
        &mut self,
        request: &fidl_softmac::WlanSoftmacStartActiveScanRequest,
    ) -> Result<fidl_softmac::WlanSoftmacBaseStartActiveScanResponse, zx::Status> {
        Self::flatten_and_log_error(
            "StartActiveScan",
            self.wlan_softmac_bridge_proxy.start_active_scan(request).await,
        )
    }

    async fn cancel_scan(
        &mut self,
        request: &fidl_softmac::WlanSoftmacBaseCancelScanRequest,
    ) -> Result<(), zx::Status> {
        Self::flatten_and_log_error(
            "CancelScan",
            self.wlan_softmac_bridge_proxy.cancel_scan(request).await,
        )
    }

    async fn join_bss(
        &mut self,
        request: &fidl_driver_common::JoinBssRequest,
    ) -> Result<(), zx::Status> {
        Self::flatten_and_log_error(
            "JoinBss",
            self.wlan_softmac_bridge_proxy.join_bss(request).await,
        )
    }

    async fn enable_beaconing(
        &mut self,
        request: fidl_softmac::WlanSoftmacBaseEnableBeaconingRequest,
    ) -> Result<(), zx::Status> {
        self.wlan_softmac_bridge_proxy
            .enable_beaconing(&request)
            .await
            .map_err(|error| {
                error!("FIDL error during EnableBeaconing: {:?}", error);
                zx::Status::INTERNAL
            })?
            .map_err(zx::Status::err_from_raw)
    }

    async fn disable_beaconing(&mut self) -> Result<(), zx::Status> {
        self.wlan_softmac_bridge_proxy
            .disable_beaconing()
            .await
            .map_err(|error| {
                error!("DisableBeaconing failed with FIDL error: {:?}", error);
                zx::Status::INTERNAL
            })?
            .map_err(zx::Status::err_from_raw)
    }

    async fn install_key(
        &mut self,
        key_configuration: &fidl_softmac::WlanKeyConfiguration,
    ) -> Result<(), zx::Status> {
        self.wlan_softmac_bridge_proxy
            .install_key(&key_configuration)
            .await
            .map_err(|error| {
                error!("FIDL error during InstallKey: {:?}", error);
                zx::Status::INTERNAL
            })?
            .map_err(zx::Status::err_from_raw)
    }

    async fn notify_association_complete(
        &mut self,
        assoc_cfg: fidl_softmac::WlanAssociationConfig,
    ) -> Result<(), zx::Status> {
        if let Some(minstrel) = &self.minstrel {
            minstrel.lock().add_peer(&assoc_cfg)?;
        }
        Self::flatten_and_log_error(
            "NotifyAssociationComplete",
            self.wlan_softmac_bridge_proxy.notify_association_complete(&assoc_cfg).await,
        )
    }

    async fn clear_association(
        &mut self,
        request: &fidl_softmac::WlanSoftmacBaseClearAssociationRequest,
    ) -> Result<(), zx::Status> {
        let addr: MacAddr = request
            .peer_addr
            .ok_or_else(|| {
                error!("ClearAssociation called with no peer_addr field.");
                zx::Status::INVALID_ARGS
            })?
            .into();
        if let Some(minstrel) = &self.minstrel {
            minstrel.lock().remove_peer(&addr);
        }
        Self::flatten_and_log_error(
            "ClearAssociation",
            self.wlan_softmac_bridge_proxy.clear_association(request).await,
        )
    }

    async fn update_wmm_parameters(
        &mut self,
        request: &fidl_softmac::WlanSoftmacBaseUpdateWmmParametersRequest,
    ) -> Result<(), zx::Status> {
        Self::flatten_and_log_error(
            "UpdateWmmParameters",
            self.wlan_softmac_bridge_proxy.update_wmm_parameters(request).await,
        )
    }

    fn take_mlme_event_stream(&mut self) -> Option<mpsc::UnboundedReceiver<fidl_mlme::MlmeEvent>> {
        self.event_receiver.take()
    }

    fn send_mlme_event(&mut self, event: fidl_mlme::MlmeEvent) -> Result<(), anyhow::Error> {
        self.event_sink.unbounded_send(event).map_err(|e| e.into())
    }

    fn set_minstrel(&mut self, minstrel: crate::MinstrelWrapper) {
        self.minstrel.replace(minstrel);
    }

    fn minstrel(&mut self) -> Option<crate::MinstrelWrapper> {
        self.minstrel.as_ref().map(Arc::clone)
    }
}

pub mod test_utils {
    use super::*;
    use crate::ddk_converter;
    use fidl_fuchsia_wlan_common as fidl_common;
    use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
    use fidl_fuchsia_wlan_internal as fidl_internal;
    use fidl_fuchsia_wlan_sme as fidl_sme;
    use fuchsia_sync::Mutex;
    use paste::paste;
    use std::collections::VecDeque;

    pub trait FromMlmeEvent {
        fn from_event(event: fidl_mlme::MlmeEvent) -> Option<Self>
        where
            Self: std::marker::Sized;
    }

    impl FromMlmeEvent for fidl_mlme::AuthenticateIndication {
        fn from_event(event: fidl_mlme::MlmeEvent) -> Option<Self> {
            event.into_authenticate_ind()
        }
    }

    impl FromMlmeEvent for fidl_mlme::AssociateIndication {
        fn from_event(event: fidl_mlme::MlmeEvent) -> Option<Self> {
            event.into_associate_ind()
        }
    }

    impl FromMlmeEvent for fidl_mlme::ConnectConfirm {
        fn from_event(event: fidl_mlme::MlmeEvent) -> Option<Self> {
            event.into_connect_conf()
        }
    }

    impl FromMlmeEvent for fidl_mlme::StartConfirm {
        fn from_event(event: fidl_mlme::MlmeEvent) -> Option<Self> {
            event.into_start_conf()
        }
    }

    impl FromMlmeEvent for fidl_mlme::StopConfirm {
        fn from_event(event: fidl_mlme::MlmeEvent) -> Option<Self> {
            event.into_stop_conf()
        }
    }

    impl FromMlmeEvent for fidl_mlme::ScanResult {
        fn from_event(event: fidl_mlme::MlmeEvent) -> Option<Self> {
            event.into_on_scan_result()
        }
    }

    impl FromMlmeEvent for fidl_mlme::ScanEnd {
        fn from_event(event: fidl_mlme::MlmeEvent) -> Option<Self> {
            event.into_on_scan_end()
        }
    }

    impl FromMlmeEvent for fidl_mlme::EapolConfirm {
        fn from_event(event: fidl_mlme::MlmeEvent) -> Option<Self> {
            event.into_eapol_conf()
        }
    }

    impl FromMlmeEvent for fidl_mlme::EapolIndication {
        fn from_event(event: fidl_mlme::MlmeEvent) -> Option<Self> {
            event.into_eapol_ind()
        }
    }

    impl FromMlmeEvent for fidl_mlme::DeauthenticateConfirm {
        fn from_event(event: fidl_mlme::MlmeEvent) -> Option<Self> {
            event.into_deauthenticate_conf()
        }
    }

    impl FromMlmeEvent for fidl_mlme::DeauthenticateIndication {
        fn from_event(event: fidl_mlme::MlmeEvent) -> Option<Self> {
            event.into_deauthenticate_ind()
        }
    }

    impl FromMlmeEvent for fidl_mlme::DisassociateIndication {
        fn from_event(event: fidl_mlme::MlmeEvent) -> Option<Self> {
            event.into_disassociate_ind()
        }
    }

    impl FromMlmeEvent for fidl_mlme::SetKeysConfirm {
        fn from_event(event: fidl_mlme::MlmeEvent) -> Option<Self> {
            event.into_set_keys_conf()
        }
    }

    impl FromMlmeEvent for fidl_internal::SignalReportIndication {
        fn from_event(event: fidl_mlme::MlmeEvent) -> Option<Self> {
            event.into_signal_report()
        }
    }

    pub struct FakeDeviceConfig {
        mock_query_response: Option<Result<fidl_softmac::WlanSoftmacQueryResponse, zx::Status>>,
        mock_discovery_support: Option<Result<fidl_softmac::DiscoverySupport, zx::Status>>,
        mock_mac_sublayer_support: Option<Result<fidl_common::MacSublayerSupport, zx::Status>>,
        mock_security_support: Option<Result<fidl_common::SecuritySupport, zx::Status>>,
        mock_spectrum_management_support:
            Option<Result<fidl_common::SpectrumManagementSupport, zx::Status>>,
        mock_start_result: Option<Result<fidl::Channel, zx::Status>>,
        pub start_passive_scan_fails: bool,
        pub start_active_scan_fails: bool,
        pub send_wlan_frame_fails: bool,
    }

    impl Default for FakeDeviceConfig {
        fn default() -> Self {
            Self {
                mock_start_result: None,
                mock_query_response: None,
                mock_discovery_support: None,
                mock_mac_sublayer_support: None,
                mock_security_support: None,
                mock_spectrum_management_support: None,
                start_passive_scan_fails: false,
                start_active_scan_fails: false,
                send_wlan_frame_fails: false,
            }
        }
    }

    /// Generates a public [<with_mock_ $mock_name>]() function to specify a mock value for corresponding
    /// DeviceOps method. When called, the generated function will overwrite whatever mocked value already
    /// exists, if any, including mocked fields.
    macro_rules! with_mock_func {
        ( $mock_name: ident, $mock_type: path ) => {
            paste! {
                pub fn [<with_mock_ $mock_name>](
                    mut self,
                    mock_value: Result<$mock_type, zx::Status>
                ) -> Self {
                    self.[<mock_ $mock_name>] = Some(mock_value);
                    self
                }
            }
        };
    }

    impl FakeDeviceConfig {
        with_mock_func!(query_response, fidl_softmac::WlanSoftmacQueryResponse);
        with_mock_func!(discovery_support, fidl_softmac::DiscoverySupport);
        with_mock_func!(mac_sublayer_support, fidl_common::MacSublayerSupport);
        with_mock_func!(security_support, fidl_common::SecuritySupport);
        with_mock_func!(spectrum_management_support, fidl_common::SpectrumManagementSupport);
        with_mock_func!(start_result, fidl::Channel);

        pub fn with_mock_sta_addr(mut self, mock_field: [u8; 6]) -> Self {
            if let None = self.mock_query_response {
                let mut mock_value = Self::default_mock_query_response();
                mock_value.as_mut().unwrap().sta_addr = Some(mock_field);
                return self.with_mock_query_response(mock_value);
            }
            let mock_value = self
                .mock_query_response
                .as_mut()
                .unwrap()
                .as_mut()
                .expect("Cannot overwrite an Err value mock");
            mock_value.sta_addr = Some(mock_field);
            self
        }

        pub fn with_mock_mac_role(mut self, mock_field: fidl_common::WlanMacRole) -> Self {
            if let None = self.mock_query_response {
                let mut mock_value = Self::default_mock_query_response();
                mock_value.as_mut().unwrap().mac_role = Some(mock_field);
                return self.with_mock_query_response(mock_value);
            }
            let mock_value = self
                .mock_query_response
                .as_mut()
                .unwrap()
                .as_mut()
                .expect("Cannot overwrite an Err value mock");
            mock_value.mac_role = Some(mock_field);
            self
        }

        fn default_mock_query_response()
        -> Result<fidl_softmac::WlanSoftmacQueryResponse, zx::Status> {
            Ok(fidl_softmac::WlanSoftmacQueryResponse {
                sta_addr: Some([7u8; 6]),
                mac_role: Some(fidl_common::WlanMacRole::Client),
                supported_phys: Some(vec![
                    fidl_ieee80211::WlanPhyType::Dsss,
                    fidl_ieee80211::WlanPhyType::Hr,
                    fidl_ieee80211::WlanPhyType::Ofdm,
                    fidl_ieee80211::WlanPhyType::Erp,
                    fidl_ieee80211::WlanPhyType::Ht,
                    fidl_ieee80211::WlanPhyType::Vht,
                ]),
                hardware_capability: Some(0),
                band_caps: Some(fake_band_caps()),
                factory_addr: Some([7u8; 6]),
                ..Default::default()
            })
        }

        pub fn with_mock_probe_response_offload(
            mut self,
            mock_field: fidl_softmac::ProbeResponseOffloadExtension,
        ) -> Self {
            if let None = self.mock_discovery_support {
                let mut mock_value = Self::default_mock_discovery_support();
                mock_value.as_mut().unwrap().probe_response_offload = Some(mock_field);
                return self.with_mock_discovery_support(mock_value);
            }
            let mock_value = self
                .mock_discovery_support
                .as_mut()
                .unwrap()
                .as_mut()
                .expect("Cannot overwrite an Err value mock");
            mock_value.probe_response_offload = Some(mock_field);
            self
        }

        fn default_mock_discovery_support() -> Result<fidl_softmac::DiscoverySupport, zx::Status> {
            Ok(fidl_softmac::DiscoverySupport {
                scan_offload: Some(fidl_softmac::ScanOffloadExtension {
                    supported: Some(true),
                    scan_cancel_supported: Some(false),
                    ..Default::default()
                }),
                probe_response_offload: Some(fidl_softmac::ProbeResponseOffloadExtension {
                    supported: Some(false),
                    ..Default::default()
                }),
                ..Default::default()
            })
        }

        pub fn with_mock_mac_implementation_type(
            mut self,
            mock_field: fidl_common::MacImplementationType,
        ) -> Self {
            if let None = self.mock_mac_sublayer_support {
                let mut mock_value = Self::default_mock_mac_sublayer_support();
                mock_value.as_mut().unwrap().device.as_mut().unwrap().mac_implementation_type =
                    Some(mock_field);
                return self.with_mock_mac_sublayer_support(mock_value);
            }
            let mock_value = self
                .mock_mac_sublayer_support
                .as_mut()
                .unwrap()
                .as_mut()
                .expect("Cannot overwrite an Err value mock");
            mock_value.device.as_mut().unwrap().mac_implementation_type = Some(mock_field);
            self
        }

        fn default_mock_mac_sublayer_support() -> Result<fidl_common::MacSublayerSupport, zx::Status>
        {
            Ok(fidl_common::MacSublayerSupport {
                rate_selection_offload: Some(fidl_common::RateSelectionOffloadExtension {
                    supported: Some(false),
                    ..Default::default()
                }),
                data_plane: Some(fidl_common::DataPlaneExtension {
                    data_plane_type: Some(fidl_common::DataPlaneType::EthernetDevice),
                    ..Default::default()
                }),
                device: Some(fidl_common::DeviceExtension {
                    is_synthetic: Some(true),
                    mac_implementation_type: Some(fidl_common::MacImplementationType::Softmac),
                    tx_status_report_supported: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            })
        }
    }

    /// Wrapper struct that can share mutable access to the internal
    /// FakeDeviceState.
    #[derive(Clone)]
    pub struct FakeDevice {
        state: Arc<Mutex<FakeDeviceState>>,
        mlme_event_sink: mpsc::UnboundedSender<fidl_mlme::MlmeEvent>,
    }

    pub struct FakeDeviceState {
        pub config: FakeDeviceConfig,
        pub minstrel: Option<crate::MinstrelWrapper>,
        pub eth_queue: Vec<Vec<u8>>,
        pub wlan_queue: Vec<(Vec<u8>, usize)>,
        pub wlan_softmac_ifc_bridge_proxy: Option<fidl_softmac::WlanSoftmacIfcBridgeProxy>,
        pub mlme_event_stream: Option<mpsc::UnboundedReceiver<fidl_mlme::MlmeEvent>>,
        pub mlme_request_sink: mpsc::UnboundedSender<wlan_sme::MlmeRequest>,
        pub mlme_request_stream: Option<mpsc::UnboundedReceiver<wlan_sme::MlmeRequest>>,
        pub usme_bootstrap_client_end:
            Option<fidl::endpoints::ClientEnd<fidl_sme::UsmeBootstrapMarker>>,
        pub usme_bootstrap_server_end:
            Option<fidl::endpoints::ServerEnd<fidl_sme::UsmeBootstrapMarker>>,
        pub primary_channel: fidl_ieee80211::ChannelNumber,
        pub bandwidth: fidl_ieee80211::ChannelBandwidth,
        pub vht_secondary_80_channel: fidl_ieee80211::ChannelNumber,
        pub keys: Vec<fidl_softmac::WlanKeyConfiguration>,
        pub next_scan_id: u64,
        pub captured_passive_scan_request:
            Option<fidl_softmac::WlanSoftmacBaseStartPassiveScanRequest>,
        pub captured_active_scan_request: Option<fidl_softmac::WlanSoftmacStartActiveScanRequest>,

        pub join_bss_request: Option<fidl_driver_common::JoinBssRequest>,
        pub beacon_config: Option<(Vec<u8>, usize, TimeUnit)>,
        pub link_status: LinkStatus,
        pub assocs: std::collections::HashMap<MacAddr, fidl_softmac::WlanAssociationConfig>,
        pub install_key_results: VecDeque<Result<(), zx::Status>>,
        pub captured_update_wmm_parameters_request:
            Option<fidl_softmac::WlanSoftmacBaseUpdateWmmParametersRequest>,
    }

    impl FakeDevice {
        // TODO(https://fxbug.dev/327499461): This function is async to ensure MLME functions will
        // run in an async context and not call `wlan_common::timer::Timer::now` without an
        // executor.
        pub async fn new() -> (FakeDevice, Arc<Mutex<FakeDeviceState>>) {
            Self::new_with_config(FakeDeviceConfig::default()).await
        }

        // TODO(https://fxbug.dev/327499461): This function is async to ensure MLME functions will
        // run in an async context and not call `wlan_common::timer::Timer::now` without an
        // executor.
        pub async fn new_with_config(
            config: FakeDeviceConfig,
        ) -> (FakeDevice, Arc<Mutex<FakeDeviceState>>) {
            // Create a channel for SME requests, to be surfaced by start().
            let (usme_bootstrap_client_end, usme_bootstrap_server_end) =
                fidl::endpoints::create_endpoints::<fidl_sme::UsmeBootstrapMarker>();
            let (mlme_event_sink, mlme_event_stream) = mpsc::unbounded();
            let (mlme_request_sink, mlme_request_stream) = mpsc::unbounded();
            let state = Arc::new(Mutex::new(FakeDeviceState {
                config,
                minstrel: None,
                eth_queue: vec![],
                wlan_queue: vec![],
                wlan_softmac_ifc_bridge_proxy: None,
                mlme_event_stream: Some(mlme_event_stream),
                mlme_request_sink,
                mlme_request_stream: Some(mlme_request_stream),
                usme_bootstrap_client_end: Some(usme_bootstrap_client_end),
                usme_bootstrap_server_end: Some(usme_bootstrap_server_end),
                primary_channel: fidl_ieee80211::ChannelNumber {
                    band: fidl_ieee80211::WlanBand::TwoGhz,
                    number: 0,
                },
                bandwidth: fidl_ieee80211::ChannelBandwidth::Cbw20,
                vht_secondary_80_channel: fidl_ieee80211::ChannelNumber {
                    band: fidl_ieee80211::WlanBand::TwoGhz,
                    number: 0,
                },
                next_scan_id: 0,
                captured_passive_scan_request: None,
                captured_active_scan_request: None,
                keys: vec![],
                join_bss_request: None,
                beacon_config: None,
                link_status: LinkStatus::DOWN,
                assocs: std::collections::HashMap::new(),
                install_key_results: VecDeque::new(),
                captured_update_wmm_parameters_request: None,
            }));
            (FakeDevice { state: state.clone(), mlme_event_sink }, state)
        }

        pub fn state(&self) -> Arc<Mutex<FakeDeviceState>> {
            self.state.clone()
        }
    }

    impl FakeDeviceState {
        #[track_caller]
        pub fn next_mlme_msg<T: FromMlmeEvent>(&mut self) -> Result<T, Error> {
            self.mlme_event_stream
                .as_mut()
                .expect("no mlme event stream available")
                .try_next()
                .map_err(|e| anyhow::format_err!("Failed to read mlme event stream: {}", e))
                .and_then(|opt_next| {
                    opt_next.ok_or_else(|| anyhow::format_err!("No message available"))
                })
                .and_then(|evt| {
                    T::from_event(evt).ok_or_else(|| anyhow::format_err!("Unexpected mlme event"))
                })
                .map_err(|e| e.into())
        }

        pub fn reset(&mut self) {
            self.eth_queue.clear();
        }
    }

    impl DeviceOps for FakeDevice {
        async fn wlan_softmac_query_response(
            &mut self,
        ) -> Result<fidl_softmac::WlanSoftmacQueryResponse, zx::Status> {
            let state = self.state.lock();
            match state.config.mock_query_response.as_ref() {
                Some(query_response) => query_response.clone(),
                None => FakeDeviceConfig::default_mock_query_response(),
            }
        }

        async fn discovery_support(
            &mut self,
        ) -> Result<fidl_softmac::DiscoverySupport, zx::Status> {
            let state = self.state.lock();
            match state.config.mock_discovery_support.as_ref() {
                Some(discovery_support) => discovery_support.clone(),
                None => FakeDeviceConfig::default_mock_discovery_support(),
            }
        }

        async fn mac_sublayer_support(
            &mut self,
        ) -> Result<fidl_common::MacSublayerSupport, zx::Status> {
            let state = self.state.lock();
            match state.config.mock_mac_sublayer_support.as_ref() {
                Some(mac_sublayer_support) => mac_sublayer_support.clone(),
                None => FakeDeviceConfig::default_mock_mac_sublayer_support(),
            }
        }

        async fn security_support(&mut self) -> Result<fidl_common::SecuritySupport, zx::Status> {
            let state = self.state.lock();
            match state.config.mock_security_support.as_ref() {
                Some(security_support) => security_support.clone(),
                None => Ok(fidl_common::SecuritySupport {
                    mfp: Some(fidl_common::MfpFeature {
                        supported: Some(false),
                        ..Default::default()
                    }),
                    sae: Some(fidl_common::SaeFeature {
                        driver_handler_supported: Some(false),
                        sme_handler_supported: Some(false),
                        hash_to_element_supported: Some(false),
                        ..Default::default()
                    }),
                    owe: Some(fidl_common::OweFeature {
                        supported: Some(false),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            }
        }

        async fn spectrum_management_support(
            &mut self,
        ) -> Result<fidl_common::SpectrumManagementSupport, zx::Status> {
            let state = self.state.lock();
            match state.config.mock_spectrum_management_support.as_ref() {
                Some(spectrum_management_support) => spectrum_management_support.clone(),
                None => Ok(fidl_common::SpectrumManagementSupport {
                    dfs: Some(fidl_common::DfsFeature {
                        supported: Some(true),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            }
        }

        async fn start(
            &mut self,
            ifc_bridge: fidl::endpoints::ClientEnd<fidl_softmac::WlanSoftmacIfcBridgeMarker>,
            _ethernet_tx: EthernetTx,
            _wlan_rx: WlanRx,
        ) -> Result<fidl::Channel, zx::Status> {
            let mut state = self.state.lock();

            if let Some(mock_start_result) = state.config.mock_start_result.take() {
                return mock_start_result;
            }

            state.wlan_softmac_ifc_bridge_proxy = Some(ifc_bridge.into_proxy());
            Ok(state.usme_bootstrap_server_end.take().unwrap().into_channel())
        }

        fn deliver_eth_frame(&mut self, packet: &[u8]) -> Result<(), zx::Status> {
            self.state.lock().eth_queue.push(packet.to_vec());
            Ok(())
        }

        fn send_wlan_frame(
            &mut self,
            buffer: ArenaStaticBox<[u8]>,
            _tx_flags: fidl_softmac::WlanTxInfoFlags,
            _async_id: Option<TraceId>,
        ) -> Result<(), zx::Status> {
            let mut state = self.state.lock();
            if state.config.send_wlan_frame_fails {
                return Err(zx::Status::IO);
            }
            state.wlan_queue.push((buffer.to_vec(), 0));
            Ok(())
        }

        async fn set_ethernet_status(&mut self, status: LinkStatus) -> Result<(), zx::Status> {
            self.state.lock().link_status = status;
            Ok(())
        }

        async fn set_channel(
            &mut self,
            primary: fidl_ieee80211::ChannelNumber,
            bandwidth: fidl_ieee80211::ChannelBandwidth,
            vht_secondary_80_channel: fidl_ieee80211::ChannelNumber,
        ) -> Result<(), zx::Status> {
            let mut state = self.state.lock();
            state.primary_channel = primary;
            state.bandwidth = bandwidth;
            state.vht_secondary_80_channel = vht_secondary_80_channel;
            Ok(())
        }

        async fn set_mac_address(
            &mut self,
            _mac_addr: fidl_fuchsia_wlan_ieee80211::MacAddr,
        ) -> Result<(), zx::Status> {
            Err(zx::Status::NOT_SUPPORTED)
        }

        async fn start_passive_scan(
            &mut self,
            request: &fidl_softmac::WlanSoftmacBaseStartPassiveScanRequest,
        ) -> Result<fidl_softmac::WlanSoftmacBaseStartPassiveScanResponse, zx::Status> {
            let mut state = self.state.lock();
            if state.config.start_passive_scan_fails {
                return Err(zx::Status::NOT_SUPPORTED);
            }
            let scan_id = state.next_scan_id;
            state.next_scan_id += 1;
            state.captured_passive_scan_request.replace(request.clone());
            Ok(fidl_softmac::WlanSoftmacBaseStartPassiveScanResponse {
                scan_id: Some(scan_id),
                ..Default::default()
            })
        }

        async fn start_active_scan(
            &mut self,
            request: &fidl_softmac::WlanSoftmacStartActiveScanRequest,
        ) -> Result<fidl_softmac::WlanSoftmacBaseStartActiveScanResponse, zx::Status> {
            let mut state = self.state.lock();
            if state.config.start_active_scan_fails {
                return Err(zx::Status::NOT_SUPPORTED);
            }
            let scan_id = state.next_scan_id;
            state.next_scan_id += 1;
            state.captured_active_scan_request.replace(request.clone());
            Ok(fidl_softmac::WlanSoftmacBaseStartActiveScanResponse {
                scan_id: Some(scan_id),
                ..Default::default()
            })
        }

        async fn cancel_scan(
            &mut self,
            _request: &fidl_softmac::WlanSoftmacBaseCancelScanRequest,
        ) -> Result<(), zx::Status> {
            Err(zx::Status::NOT_SUPPORTED)
        }

        async fn join_bss(
            &mut self,
            request: &fidl_driver_common::JoinBssRequest,
        ) -> Result<(), zx::Status> {
            self.state.lock().join_bss_request.replace(request.clone());
            Ok(())
        }

        async fn enable_beaconing(
            &mut self,
            request: fidl_softmac::WlanSoftmacBaseEnableBeaconingRequest,
        ) -> Result<(), zx::Status> {
            match (request.packet_template, request.tim_ele_offset, request.beacon_interval) {
                (Some(packet_template), Some(tim_ele_offset), Some(beacon_interval)) => Ok({
                    self.state.lock().beacon_config = Some((
                        packet_template.mac_frame,
                        usize::try_from(tim_ele_offset).map_err(|_| zx::Status::INTERNAL)?,
                        TimeUnit(beacon_interval),
                    ));
                }),
                _ => Err(zx::Status::INVALID_ARGS),
            }
        }

        async fn disable_beaconing(&mut self) -> Result<(), zx::Status> {
            self.state.lock().beacon_config = None;
            Ok(())
        }

        async fn install_key(
            &mut self,
            key_configuration: &fidl_softmac::WlanKeyConfiguration,
        ) -> Result<(), zx::Status> {
            let mut state = self.state.lock();
            state.keys.push(key_configuration.clone());
            state.install_key_results.pop_front().unwrap_or(Ok(()))
        }

        async fn notify_association_complete(
            &mut self,
            cfg: fidl_softmac::WlanAssociationConfig,
        ) -> Result<(), zx::Status> {
            let mut state = self.state.lock();
            if let Some(minstrel) = &state.minstrel {
                minstrel.lock().add_peer(&cfg)?
            }
            state.assocs.insert(cfg.bssid.unwrap().into(), cfg);
            Ok(())
        }

        async fn clear_association(
            &mut self,
            request: &fidl_softmac::WlanSoftmacBaseClearAssociationRequest,
        ) -> Result<(), zx::Status> {
            let addr: MacAddr = request.peer_addr.unwrap().into();
            let mut state = self.state.lock();
            if let Some(minstrel) = &state.minstrel {
                minstrel.lock().remove_peer(&addr);
            }
            state.assocs.remove(&addr);
            state.join_bss_request = None;
            Ok(())
        }

        async fn update_wmm_parameters(
            &mut self,
            request: &fidl_softmac::WlanSoftmacBaseUpdateWmmParametersRequest,
        ) -> Result<(), zx::Status> {
            let mut state = self.state.lock();
            state.captured_update_wmm_parameters_request.replace(request.clone());
            Ok(())
        }

        fn take_mlme_event_stream(
            &mut self,
        ) -> Option<mpsc::UnboundedReceiver<fidl_mlme::MlmeEvent>> {
            self.state.lock().mlme_event_stream.take()
        }

        fn send_mlme_event(&mut self, event: fidl_mlme::MlmeEvent) -> Result<(), anyhow::Error> {
            self.mlme_event_sink.unbounded_send(event).map_err(|e| e.into())
        }

        fn set_minstrel(&mut self, minstrel: crate::MinstrelWrapper) {
            self.state.lock().minstrel.replace(minstrel);
        }

        fn minstrel(&mut self) -> Option<crate::MinstrelWrapper> {
            self.state.lock().minstrel.as_ref().map(Arc::clone)
        }
    }

    pub fn fake_band_caps() -> Vec<fidl_softmac::WlanSoftmacBandCapability> {
        vec![
            fidl_softmac::WlanSoftmacBandCapability {
                band: Some(fidl_ieee80211::WlanBand::TwoGhz),
                basic_rates: Some(vec![
                    0x02, 0x04, 0x0b, 0x16, 0x0c, 0x12, 0x18, 0x24, 0x30, 0x48, 0x60, 0x6c,
                ]),
                primary_channels: Some(
                    (1..=14)
                        .map(|c| fidl_ieee80211::ChannelNumber {
                            band: fidl_ieee80211::WlanBand::TwoGhz,
                            number: c,
                        })
                        .collect(),
                ),
                ht_caps: Some(fidl_ieee80211::HtCapabilities {
                    bytes: [
                        0x63, 0x00, // HT capability info
                        0x17, // AMPDU params
                        0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                        0x00, // Rx MCS bitmask, Supported MCS values: 0-7
                        0x01, 0x00, 0x00, 0x00, // Tx parameters
                        0x00, 0x00, // HT extended capabilities
                        0x00, 0x00, 0x00, 0x00, // TX beamforming capabilities
                        0x00, // ASEL capabilities
                    ],
                }),
                vht_caps: None,
                ..Default::default()
            },
            fidl_softmac::WlanSoftmacBandCapability {
                band: Some(fidl_ieee80211::WlanBand::FiveGhz),
                basic_rates: Some(vec![0x02, 0x04, 0x0b, 0x16, 0x30, 0x60, 0x7e, 0x7f]),
                primary_channels: Some(
                    [36, 40, 44, 48, 149, 153, 157, 161]
                        .into_iter()
                        .map(|c| fidl_ieee80211::ChannelNumber {
                            band: fidl_ieee80211::WlanBand::FiveGhz,
                            number: c,
                        })
                        .collect(),
                ),
                ht_caps: Some(fidl_ieee80211::HtCapabilities {
                    bytes: [
                        0x63, 0x00, // HT capability info
                        0x17, // AMPDU params
                        0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                        0x00, // Rx MCS bitmask, Supported MCS values: 0-7
                        0x01, 0x00, 0x00, 0x00, // Tx parameters
                        0x00, 0x00, // HT extended capabilities
                        0x00, 0x00, 0x00, 0x00, // TX beamforming capabilities
                        0x00, // ASEL capabilities
                    ],
                }),
                vht_caps: Some(fidl_ieee80211::VhtCapabilities {
                    bytes: [0x32, 0x50, 0x80, 0x0f, 0xfe, 0xff, 0x00, 0x00, 0xfe, 0xff, 0x00, 0x00],
                }),
                ..Default::default()
            },
        ]
    }

    pub fn fake_mlme_band_caps() -> Vec<fidl_mlme::BandCapability> {
        fake_band_caps()
            .into_iter()
            .map(ddk_converter::mlme_band_cap_from_softmac)
            .collect::<Result<_, _>>()
            .expect("Failed to convert softmac driver band capabilities.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{WlanTxPacketExt as _, ddk_converter};
    use assert_matches::assert_matches;
    use fdf::Arena;
    use fidl_fuchsia_wlan_common as fidl_common;
    use fidl_fuchsia_wlan_driver as fidl_driver_common;
    use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
    use ieee80211::Ssid;

    fn make_deauth_confirm_msg() -> fidl_mlme::DeauthenticateConfirm {
        fidl_mlme::DeauthenticateConfirm { peer_sta_address: [1; 6] }
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn state_method_returns_correct_pointer() {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        assert_eq!(Arc::as_ptr(&fake_device.state()), Arc::as_ptr(&fake_device_state));
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn fake_device_returns_expected_wlan_softmac_query_response() {
        let (mut fake_device, _) = FakeDevice::new().await;
        let query_response = fake_device.wlan_softmac_query_response().await.unwrap();
        assert_eq!(query_response.sta_addr, [7u8; 6].into());
        assert_eq!(query_response.factory_addr, [7u8; 6].into());
        assert_eq!(query_response.mac_role, Some(fidl_common::WlanMacRole::Client));
        assert_eq!(
            query_response.supported_phys,
            Some(vec![
                fidl_ieee80211::WlanPhyType::Dsss,
                fidl_ieee80211::WlanPhyType::Hr,
                fidl_ieee80211::WlanPhyType::Ofdm,
                fidl_ieee80211::WlanPhyType::Erp,
                fidl_ieee80211::WlanPhyType::Ht,
                fidl_ieee80211::WlanPhyType::Vht,
            ]),
        );
        assert_eq!(query_response.hardware_capability, Some(0));

        let expected_band_caps = [
            fidl_softmac::WlanSoftmacBandCapability {
                band: Some(fidl_ieee80211::WlanBand::TwoGhz),
                basic_rates: Some(vec![
                    0x02, 0x04, 0x0b, 0x16, 0x0c, 0x12, 0x18, 0x24, 0x30, 0x48, 0x60, 0x6c,
                ]),
                primary_channels: Some(
                    (1..=14)
                        .map(|c| fidl_ieee80211::ChannelNumber {
                            band: fidl_ieee80211::WlanBand::TwoGhz,
                            number: c,
                        })
                        .collect(),
                ),
                ht_caps: Some(fidl_ieee80211::HtCapabilities {
                    bytes: [
                        0x63, 0x00, // HT capability info
                        0x17, // AMPDU params
                        0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                        0x00, // Rx MCS bitmask, Supported MCS values: 0-7
                        0x01, 0x00, 0x00, 0x00, // Tx parameters
                        0x00, 0x00, // HT extended capabilities
                        0x00, 0x00, 0x00, 0x00, // TX beamforming capabilities
                        0x00, // ASEL capabilities
                    ],
                }),
                vht_caps: None,
                ..Default::default()
            },
            fidl_softmac::WlanSoftmacBandCapability {
                band: Some(fidl_ieee80211::WlanBand::FiveGhz),
                basic_rates: Some(vec![0x02, 0x04, 0x0b, 0x16, 0x30, 0x60, 0x7e, 0x7f]),
                primary_channels: Some(
                    [36, 40, 44, 48, 149, 153, 157, 161]
                        .into_iter()
                        .map(|c| fidl_ieee80211::ChannelNumber {
                            band: fidl_ieee80211::WlanBand::FiveGhz,
                            number: c,
                        })
                        .collect(),
                ),
                ht_caps: Some(fidl_ieee80211::HtCapabilities {
                    bytes: [
                        0x63, 0x00, // HT capability info
                        0x17, // AMPDU params
                        0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                        0x00, // Rx MCS bitmask, Supported MCS values: 0-7
                        0x01, 0x00, 0x00, 0x00, // Tx parameters
                        0x00, 0x00, // HT extended capabilities
                        0x00, 0x00, 0x00, 0x00, // TX beamforming capabilities
                        0x00, // ASEL capabilities
                    ],
                }),
                vht_caps: Some(fidl_ieee80211::VhtCapabilities {
                    bytes: [0x32, 0x50, 0x80, 0x0f, 0xfe, 0xff, 0x00, 0x00, 0xfe, 0xff, 0x00, 0x00],
                }),
                ..Default::default()
            },
        ];
        let actual_band_caps = query_response.band_caps.as_ref().unwrap();
        for (actual_band_cap, expected_band_cap) in actual_band_caps.iter().zip(&expected_band_caps)
        {
            assert_eq!(actual_band_cap, expected_band_cap);
        }
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn fake_device_returns_expected_discovery_support() {
        let (mut fake_device, _) = FakeDevice::new().await;
        let discovery_support = fake_device.discovery_support().await.unwrap();
        assert_eq!(
            discovery_support,
            fidl_softmac::DiscoverySupport {
                scan_offload: Some(fidl_softmac::ScanOffloadExtension {
                    supported: Some(true),
                    scan_cancel_supported: Some(false),
                    ..Default::default()
                }),
                probe_response_offload: Some(fidl_softmac::ProbeResponseOffloadExtension {
                    supported: Some(false),
                    ..Default::default()
                }),
                ..Default::default()
            }
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn fake_device_returns_expected_mac_sublayer_support() {
        let (mut fake_device, _) = FakeDevice::new().await;
        let mac_sublayer_support = fake_device.mac_sublayer_support().await.unwrap();
        assert_eq!(
            mac_sublayer_support,
            fidl_common::MacSublayerSupport {
                rate_selection_offload: Some(fidl_common::RateSelectionOffloadExtension {
                    supported: Some(false),
                    ..Default::default()
                }),
                data_plane: Some(fidl_common::DataPlaneExtension {
                    data_plane_type: Some(fidl_common::DataPlaneType::EthernetDevice),
                    ..Default::default()
                }),
                device: Some(fidl_common::DeviceExtension {
                    is_synthetic: Some(true),
                    mac_implementation_type: Some(fidl_common::MacImplementationType::Softmac),
                    tx_status_report_supported: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            }
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn fake_device_returns_expected_security_support() {
        let (mut fake_device, _) = FakeDevice::new().await;
        let security_support = fake_device.security_support().await.unwrap();
        assert_eq!(
            security_support,
            fidl_common::SecuritySupport {
                mfp: Some(fidl_common::MfpFeature { supported: Some(false), ..Default::default() }),
                sae: Some(fidl_common::SaeFeature {
                    driver_handler_supported: Some(false),
                    sme_handler_supported: Some(false),
                    hash_to_element_supported: Some(false),
                    ..Default::default()
                }),
                owe: Some(fidl_common::OweFeature { supported: Some(false), ..Default::default() }),
                ..Default::default()
            }
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn fake_device_returns_expected_spectrum_management_support() {
        let (mut fake_device, _) = FakeDevice::new().await;
        let spectrum_management_support = fake_device.spectrum_management_support().await.unwrap();
        assert_eq!(
            spectrum_management_support,
            fidl_common::SpectrumManagementSupport {
                dfs: Some(fidl_common::DfsFeature { supported: Some(true), ..Default::default() }),
                ..Default::default()
            }
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_can_dynamically_change_fake_device_state() {
        let (mut fake_device, fake_device_state) = FakeDevice::new_with_config(
            FakeDeviceConfig::default().with_mock_mac_role(fidl_common::WlanMacRole::Client),
        )
        .await;
        let query_response = fake_device.wlan_softmac_query_response().await.unwrap();
        assert_eq!(query_response.mac_role, Some(fidl_common::WlanMacRole::Client));

        fake_device_state.lock().config =
            FakeDeviceConfig::default().with_mock_mac_role(fidl_common::WlanMacRole::Ap);

        let query_response = fake_device.wlan_softmac_query_response().await.unwrap();
        assert_eq!(query_response.mac_role, Some(fidl_common::WlanMacRole::Ap));
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn send_mlme_message() {
        let (mut fake_device, fake_device_state) = FakeDevice::new().await;
        fake_device
            .send_mlme_event(fidl_mlme::MlmeEvent::DeauthenticateConf {
                resp: make_deauth_confirm_msg(),
            })
            .expect("error sending MLME message");

        // Read message from channel.
        let msg = fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::DeauthenticateConfirm>()
            .expect("error reading message from channel");
        assert_eq!(msg, make_deauth_confirm_msg());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn send_mlme_message_peer_already_closed() {
        let (mut fake_device, fake_device_state) = FakeDevice::new().await;
        fake_device_state.lock().mlme_event_stream.take();

        fake_device
            .send_mlme_event(fidl_mlme::MlmeEvent::DeauthenticateConf {
                resp: make_deauth_confirm_msg(),
            })
            .expect_err("Mlme event should fail");
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn fake_device_deliver_eth_frame() {
        let (mut fake_device, fake_device_state) = FakeDevice::new().await;
        assert_eq!(fake_device_state.lock().eth_queue.len(), 0);
        let first_frame = [5; 32];
        let second_frame = [6; 32];
        assert_eq!(fake_device.deliver_eth_frame(&first_frame[..]), Ok(()));
        assert_eq!(fake_device.deliver_eth_frame(&second_frame[..]), Ok(()));
        assert_eq!(fake_device_state.lock().eth_queue.len(), 2);
        assert_eq!(&fake_device_state.lock().eth_queue[0], &first_frame);
        assert_eq!(&fake_device_state.lock().eth_queue[1], &second_frame);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn set_channel() {
        let (mut fake_device, fake_device_state) = FakeDevice::new().await;
        let expected_primary =
            fidl_ieee80211::ChannelNumber { band: fidl_ieee80211::WlanBand::TwoGhz, number: 2 };
        let expected_bandwidth = fidl_ieee80211::ChannelBandwidth::Cbw80P80;
        let expected_vht_secondary_80_channel =
            fidl_ieee80211::ChannelNumber { band: fidl_ieee80211::WlanBand::TwoGhz, number: 4 };
        fake_device
            .set_channel(expected_primary, expected_bandwidth, expected_vht_secondary_80_channel)
            .await
            .expect("set_channel failed?");
        // Check the internal state.
        assert_eq!(fake_device_state.lock().primary_channel, expected_primary);
        assert_eq!(fake_device_state.lock().bandwidth, expected_bandwidth);
        assert_eq!(
            fake_device_state.lock().vht_secondary_80_channel,
            expected_vht_secondary_80_channel
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn install_key() {
        let (mut fake_device, fake_device_state) = FakeDevice::new().await;
        fake_device
            .install_key(&fidl_softmac::WlanKeyConfiguration {
                protection: Some(fidl_softmac::WlanProtection::None),
                cipher_oui: Some([3, 4, 5]),
                cipher_type: Some(6),
                key_type: Some(fidl_ieee80211::KeyType::Pairwise),
                peer_addr: Some([8; 6]),
                key_idx: Some(9),
                key: Some(vec![11; 32]),
                rsc: Some(12),
                ..Default::default()
            })
            .await
            .expect("error setting key");
        assert_eq!(fake_device_state.lock().keys.len(), 1);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn start_passive_scan() {
        let (mut fake_device, fake_device_state) = FakeDevice::new().await;

        let result = fake_device
            .start_passive_scan(&fidl_softmac::WlanSoftmacBaseStartPassiveScanRequest {
                channels: Some(
                    [1, 2, 3]
                        .into_iter()
                        .map(|c| fidl_ieee80211::ChannelNumber {
                            band: fidl_ieee80211::WlanBand::TwoGhz,
                            number: c,
                        })
                        .collect(),
                ),
                min_channel_time: Some(zx::MonotonicDuration::from_millis(0).into_nanos()),
                max_channel_time: Some(zx::MonotonicDuration::from_millis(200).into_nanos()),
                min_home_time: Some(0),
                ..Default::default()
            })
            .await;
        assert!(result.is_ok());

        assert_eq!(
            fake_device_state.lock().captured_passive_scan_request,
            Some(fidl_softmac::WlanSoftmacBaseStartPassiveScanRequest {
                channels: Some(
                    [1, 2, 3]
                        .into_iter()
                        .map(|c| fidl_ieee80211::ChannelNumber {
                            band: fidl_ieee80211::WlanBand::TwoGhz,
                            number: c,
                        })
                        .collect(),
                ),
                min_channel_time: Some(0),
                max_channel_time: Some(200_000_000),
                min_home_time: Some(0),
                ..Default::default()
            }),
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn start_active_scan() {
        let (mut fake_device, fake_device_state) = FakeDevice::new().await;

        let result = fake_device
            .start_active_scan(&fidl_softmac::WlanSoftmacStartActiveScanRequest {
                channels: Some(
                    [1, 2, 3]
                        .into_iter()
                        .map(|c| fidl_ieee80211::ChannelNumber {
                            band: fidl_ieee80211::WlanBand::TwoGhz,
                            number: c,
                        })
                        .collect(),
                ),
                ssids: Some(vec![
                    ddk_converter::cssid_from_ssid_unchecked(
                        &Ssid::try_from("foo").unwrap().into(),
                    ),
                    ddk_converter::cssid_from_ssid_unchecked(
                        &Ssid::try_from("bar").unwrap().into(),
                    ),
                ]),
                mac_header: Some(vec![
                    0x40u8, 0x00, // Frame Control
                    0x00, 0x00, // Duration
                    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, // Address 1
                    0x66, 0x66, 0x66, 0x66, 0x66, 0x66, // Address 2
                    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, // Address 3
                    0x70, 0xdc, // Sequence Control
                ]),
                ies: Some(vec![
                    0x01u8, // Element ID for Supported Rates
                    0x08,   // Length
                    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, // Supported Rates
                ]),
                min_channel_time: Some(zx::MonotonicDuration::from_millis(0).into_nanos()),
                max_channel_time: Some(zx::MonotonicDuration::from_millis(200).into_nanos()),
                min_home_time: Some(0),
                min_probes_per_channel: Some(1),
                max_probes_per_channel: Some(3),
                ..Default::default()
            })
            .await;
        assert!(result.is_ok());
        assert_eq!(
            fake_device_state.lock().captured_active_scan_request,
            Some(fidl_softmac::WlanSoftmacStartActiveScanRequest {
                channels: Some(
                    [1, 2, 3]
                        .into_iter()
                        .map(|c| fidl_ieee80211::ChannelNumber {
                            band: fidl_ieee80211::WlanBand::TwoGhz,
                            number: c,
                        })
                        .collect(),
                ),
                ssids: Some(vec![
                    ddk_converter::cssid_from_ssid_unchecked(
                        &Ssid::try_from("foo").unwrap().into()
                    ),
                    ddk_converter::cssid_from_ssid_unchecked(
                        &Ssid::try_from("bar").unwrap().into()
                    ),
                ]),
                mac_header: Some(vec![
                    0x40, 0x00, // Frame Control
                    0x00, 0x00, // Duration
                    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, // Address 1
                    0x66, 0x66, 0x66, 0x66, 0x66, 0x66, // Address 2
                    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, // Address 3
                    0x70, 0xdc, // Sequence Control
                ]),
                ies: Some(vec![
                    0x01, // Element ID for Supported Rates
                    0x08, // Length
                    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08 // Supported Rates
                ]),
                min_channel_time: Some(0),
                max_channel_time: Some(200_000_000),
                min_home_time: Some(0),
                min_probes_per_channel: Some(1),
                max_probes_per_channel: Some(3),
                ..Default::default()
            }),
            "No active scan argument available."
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn join_bss() {
        let (mut fake_device, fake_device_state) = FakeDevice::new().await;
        fake_device
            .join_bss(&fidl_driver_common::JoinBssRequest {
                bssid: Some([1, 2, 3, 4, 5, 6]),
                bss_type: Some(fidl_ieee80211::BssType::Personal),
                remote: Some(true),
                beacon_period: Some(100),
                ..Default::default()
            })
            .await
            .expect("error configuring bss");
        assert!(fake_device_state.lock().join_bss_request.is_some());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn enable_disable_beaconing() {
        let (mut fake_device, fake_device_state) = FakeDevice::new().await;
        let arena = Arena::new();
        let mut buffer = arena.insert_default_slice::<u8>(4);
        buffer.copy_from_slice(&[1, 2, 3, 4][..]);
        let mac_frame = buffer.to_vec();

        fake_device
            .enable_beaconing(fidl_softmac::WlanSoftmacBaseEnableBeaconingRequest {
                packet_template: Some(fidl_softmac::WlanTxPacket::template(mac_frame)),
                tim_ele_offset: Some(1),
                beacon_interval: Some(2),
                ..Default::default()
            })
            .await
            .expect("error enabling beaconing");
        assert_matches!(
        fake_device_state.lock().beacon_config.as_ref(),
        Some((buffer, tim_ele_offset, beacon_interval)) => {
            assert_eq!(&buffer[..], &[1, 2, 3, 4][..]);
            assert_eq!(*tim_ele_offset, 1);
            assert_eq!(*beacon_interval, TimeUnit(2));
        });
        fake_device.disable_beaconing().await.expect("error disabling beaconing");
        assert_matches!(fake_device_state.lock().beacon_config.as_ref(), None);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn set_ethernet_status() {
        let (mut fake_device, fake_device_state) = FakeDevice::new().await;
        fake_device.set_ethernet_up().await.expect("failed setting status");
        assert_eq!(fake_device_state.lock().link_status, LinkStatus::UP);

        fake_device.set_ethernet_down().await.expect("failed setting status");
        assert_eq!(fake_device_state.lock().link_status, LinkStatus::DOWN);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn notify_association_complete() {
        let (mut fake_device, fake_device_state) = FakeDevice::new().await;
        fake_device
            .notify_association_complete(fidl_softmac::WlanAssociationConfig {
                bssid: Some([1, 2, 3, 4, 5, 6]),
                aid: Some(1),
                listen_interval: Some(2),
                primary: Some(fidl_ieee80211::ChannelNumber {
                    band: fidl_ieee80211::WlanBand::TwoGhz,
                    number: 3,
                }),
                bandwidth: Some(fidl_ieee80211::ChannelBandwidth::Cbw20),
                qos: Some(false),
                wmm_params: None,
                rates: None,
                capability_info: Some(0x0102),
                ht_cap: None,
                ht_op: None,
                vht_cap: None,
                vht_op: None,
                ..Default::default()
            })
            .await
            .expect("error configuring assoc");
        assert!(fake_device_state.lock().assocs.contains_key(&[1, 2, 3, 4, 5, 6].into()));
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn clear_association() {
        let (mut fake_device, fake_device_state) = FakeDevice::new().await;
        fake_device
            .join_bss(&fidl_driver_common::JoinBssRequest {
                bssid: Some([1, 2, 3, 4, 5, 6]),
                bss_type: Some(fidl_ieee80211::BssType::Personal),
                remote: Some(true),
                beacon_period: Some(100),
                ..Default::default()
            })
            .await
            .expect("error configuring bss");

        let assoc_cfg = fidl_softmac::WlanAssociationConfig {
            bssid: Some([1, 2, 3, 4, 5, 6]),
            aid: Some(1),
            primary: Some(fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::FiveGhz,
                number: 149,
            }),
            bandwidth: Some(fidl_ieee80211::ChannelBandwidth::Cbw20),
            ..Default::default()
        };

        assert!(fake_device_state.lock().join_bss_request.is_some());
        fake_device.notify_association_complete(assoc_cfg).await.expect("error configuring assoc");
        assert_eq!(fake_device_state.lock().assocs.len(), 1);
        fake_device
            .clear_association(&fidl_softmac::WlanSoftmacBaseClearAssociationRequest {
                peer_addr: Some([1, 2, 3, 4, 5, 6]),
                ..Default::default()
            })
            .await
            .expect("error clearing assoc");
        assert_eq!(fake_device_state.lock().assocs.len(), 0);
        assert!(fake_device_state.lock().join_bss_request.is_none());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn fake_device_captures_update_wmm_parameters_request() {
        let (mut fake_device, fake_device_state) = FakeDevice::new().await;

        let request = fidl_softmac::WlanSoftmacBaseUpdateWmmParametersRequest {
            ac: Some(fidl_ieee80211::WlanAccessCategory::Background),
            params: Some(fidl_driver_common::WlanWmmParameters {
                apsd: true,
                ac_be_params: fidl_driver_common::WlanWmmAccessCategoryParameters {
                    ecw_min: 10,
                    ecw_max: 100,
                    aifsn: 1,
                    txop_limit: 5,
                    acm: true,
                },
                ac_bk_params: fidl_driver_common::WlanWmmAccessCategoryParameters {
                    ecw_min: 11,
                    ecw_max: 100,
                    aifsn: 1,
                    txop_limit: 5,
                    acm: true,
                },
                ac_vi_params: fidl_driver_common::WlanWmmAccessCategoryParameters {
                    ecw_min: 12,
                    ecw_max: 100,
                    aifsn: 1,
                    txop_limit: 5,
                    acm: true,
                },
                ac_vo_params: fidl_driver_common::WlanWmmAccessCategoryParameters {
                    ecw_min: 13,
                    ecw_max: 100,
                    aifsn: 1,
                    txop_limit: 5,
                    acm: true,
                },
            }),
            ..Default::default()
        };
        let result = fake_device.update_wmm_parameters(&request).await;
        assert!(result.is_ok());

        assert_eq!(fake_device_state.lock().captured_update_wmm_parameters_request, Some(request),);
    }
}
