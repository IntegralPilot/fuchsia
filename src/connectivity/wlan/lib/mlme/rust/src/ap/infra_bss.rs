// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::WlanTxPacketExt as _;
use crate::ap::remote_client::{ClientRejection, RemoteClient};
use crate::ap::{BeaconOffloadParams, BufferedFrame, Context, Rejection, TimedEvent, frame_writer};
use crate::ddk_converter::softmac_key_configuration_from_mlme;
use crate::device::{self, DeviceOps};
use crate::error::Error;
use anyhow::format_err;
use fdf::ArenaStaticBox;
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_mlme as fidl_mlme;
use fidl_fuchsia_wlan_softmac as fidl_softmac;
use fuchsia_trace as trace;
use ieee80211::{MacAddr, MacAddrBytes, Ssid};
use log::error;
use std::collections::{HashMap, VecDeque};
use std::fmt::Display;
use wlan_common::mac::{self, CapabilityInfo, EthernetIIHdr};
use wlan_common::{TimeUnit, ie, tim};
use zerocopy::SplitByteSlice;

pub struct InfraBss {
    pub ssid: Ssid,
    pub rsne: Option<Vec<u8>>,
    pub beacon_interval: TimeUnit,
    pub dtim_period: u8,
    pub capabilities: CapabilityInfo,
    pub rates: Vec<u8>,
    pub channel: u8,
    pub band: fidl_ieee80211::WlanBand,
    pub clients: HashMap<MacAddr, RemoteClient>,

    group_buffered: VecDeque<BufferedFrame>,
    dtim_count: u8,
}

fn get_client_mut(
    clients: &mut HashMap<MacAddr, RemoteClient>,
    addr: MacAddr,
) -> Result<&mut RemoteClient, Error> {
    clients
        .get_mut(&addr)
        .ok_or(Error::Status(format!("client {:02X?} not found", addr), zx::Status::NOT_FOUND))
}

/// Prepends the client's MAC address to an error::Error.
///
/// This will discard any more specific error information (e.g. if it was a FIDL error or a
/// anyhow::Error error), but will still preserve the underlying zx::Status code.
fn make_client_error(addr: MacAddr, e: Error) -> Error {
    Error::Status(format!("client {}: {}", addr, e), e.into())
}

impl InfraBss {
    pub async fn new<D: DeviceOps>(
        ctx: &mut Context<D>,
        ssid: Ssid,
        beacon_interval: TimeUnit,
        dtim_period: u8,
        capabilities: CapabilityInfo,
        rates: Vec<u8>,
        channel: u8,
        band: fidl_ieee80211::WlanBand,
        rsne: Option<Vec<u8>>,
    ) -> Result<Self, Error> {
        let bss = Self {
            ssid,
            rsne,
            beacon_interval,
            dtim_period,
            rates,
            capabilities,
            channel,
            band,
            clients: HashMap::new(),

            group_buffered: VecDeque::new(),
            dtim_count: 0,
        };

        ctx.device
            .set_channel(
                fidl_ieee80211::ChannelNumber { band, number: channel },
                fidl_ieee80211::ChannelBandwidth::Cbw20,
                fidl_ieee80211::ChannelNumber { band, number: 0 },
            )
            .await
            .map_err(|s| Error::Status(format!("failed to set channel"), s))?;

        // TODO(https://fxbug.dev/42113580): Support DTIM.

        let (in_buffer, beacon_offload_params) = bss.make_beacon_frame(ctx)?;
        let mac_frame = in_buffer.to_vec();
        let tim_ele_offset = u64::try_from(beacon_offload_params.tim_ele_offset).map_err(|_| {
            Error::Internal(format_err!(
                "failed to convert TIM offset for beacon frame packet template"
            ))
        })?;
        ctx.device
            .enable_beaconing(fidl_softmac::WlanSoftmacBaseEnableBeaconingRequest {
                packet_template: Some(fidl_softmac::WlanTxPacket::template(mac_frame)),
                tim_ele_offset: Some(tim_ele_offset),
                beacon_interval: Some(beacon_interval.0),
                ..Default::default()
            })
            .await
            .map_err(|s| Error::Status(format!("failed to enable beaconing"), s))?;
        ctx.device
            .set_ethernet_up()
            .await
            .map_err(|s| Error::Status(format!("Failed to set ethernet status to UP"), s))?;

        Ok(bss)
    }

    pub async fn stop<D: DeviceOps>(&self, ctx: &mut Context<D>) -> Result<(), Error> {
        ctx.device
            .set_ethernet_down()
            .await
            .map_err(|s| Error::Status(format!("Failed to set ethernet status to DOWN"), s))?;
        ctx.device
            .disable_beaconing()
            .await
            .map_err(|s| Error::Status(format!("failed to disable beaconing"), s))
    }

    fn make_tim(&self) -> tim::TrafficIndicationMap {
        let mut tim = tim::TrafficIndicationMap::new();
        for client in self.clients.values() {
            let aid = match client.aid() {
                Some(aid) => aid,
                None => {
                    continue;
                }
            };
            tim.set_traffic_buffered(aid, client.has_buffered_frames());
        }
        tim
    }

    pub async fn handle_mlme_setkeys_req<D: DeviceOps>(
        &mut self,
        ctx: &mut Context<D>,
        keylist: Vec<fidl_mlme::SetKeyDescriptor>,
    ) -> Result<(), Error> {
        fn key_type_name(key_type: fidl_mlme::KeyType) -> impl Display {
            match key_type {
                fidl_mlme::KeyType::Group => "GTK",
                fidl_mlme::KeyType::Pairwise => "PTK",
                fidl_mlme::KeyType::PeerKey => "peer key",
                fidl_mlme::KeyType::Igtk => "IGTK",
            }
        }

        if self.rsne.is_none() {
            return Err(Error::Status(
                format!("cannot set keys for an unprotected BSS"),
                zx::Status::BAD_STATE,
            ));
        }

        for key_descriptor in keylist.into_iter() {
            let key_type = key_descriptor.key_type;
            ctx.device
                .install_key(&softmac_key_configuration_from_mlme(key_descriptor))
                .await
                .map_err(|status| {
                    Error::Status(
                        format!("failed to set {} on PHY", key_type_name(key_type)),
                        status,
                    )
                })?;
        }
        Ok(())
    }

    pub async fn handle_mlme_auth_resp<D: DeviceOps>(
        &mut self,
        ctx: &mut Context<D>,
        resp: fidl_mlme::AuthenticateResponse,
    ) -> Result<(), Error> {
        let client = get_client_mut(&mut self.clients, resp.peer_sta_address.into())?;
        client
            .handle_mlme_auth_resp(ctx, resp.result_code)
            .await
            .map_err(|e| make_client_error(client.addr, e))
    }

    pub async fn handle_mlme_deauth_req<D: DeviceOps>(
        &mut self,
        ctx: &mut Context<D>,
        req: fidl_mlme::DeauthenticateRequest,
    ) -> Result<(), Error> {
        let client = get_client_mut(&mut self.clients, req.peer_sta_address.into())?;
        client
            .handle_mlme_deauth_req(ctx, req.reason_code)
            .await
            .map_err(|e| make_client_error(client.addr, e))?;
        if client.deauthenticated() {
            self.clients.remove(&req.peer_sta_address.into());
        }
        Ok(())
    }

    pub async fn handle_mlme_assoc_resp<D: DeviceOps>(
        &mut self,
        ctx: &mut Context<D>,
        resp: fidl_mlme::AssociateResponse,
    ) -> Result<(), Error> {
        let client = get_client_mut(&mut self.clients, resp.peer_sta_address.into())?;

        client
            .handle_mlme_assoc_resp(
                ctx,
                self.rsne.is_some(),
                fidl_ieee80211::ChannelNumber { number: self.channel, band: self.band },
                CapabilityInfo(resp.capability_info),
                resp.result_code,
                resp.association_id,
                &resp.rates,
            )
            .await
            .map_err(|e| make_client_error(client.addr, e))
    }

    pub async fn handle_mlme_disassoc_req<D: DeviceOps>(
        &mut self,
        ctx: &mut Context<D>,
        req: fidl_mlme::DisassociateRequest,
    ) -> Result<(), Error> {
        let client = get_client_mut(&mut self.clients, req.peer_sta_address.into())?;
        client
            .handle_mlme_disassoc_req(ctx, req.reason_code.into_primitive())
            .await
            .map_err(|e| make_client_error(client.addr, e))
    }

    pub fn handle_mlme_set_controlled_port_req(
        &mut self,
        req: fidl_mlme::SetControlledPortRequest,
    ) -> Result<(), Error> {
        let client = get_client_mut(&mut self.clients, req.peer_sta_address.into())?;
        client
            .handle_mlme_set_controlled_port_req(req.state)
            .map_err(|e| make_client_error(client.addr, e))
    }

    pub fn handle_mlme_eapol_req<D: DeviceOps>(
        &mut self,
        ctx: &mut Context<D>,
        req: fidl_mlme::EapolRequest,
    ) -> Result<(), Error> {
        let client = get_client_mut(&mut self.clients, req.dst_addr.into())?;
        match client
            .handle_mlme_eapol_req(ctx, req.src_addr.into(), &req.data)
            .map_err(|e| make_client_error(client.addr, e))
        {
            Ok(()) => {
                ctx.send_mlme_eapol_conf(fidl_mlme::EapolResultCode::Success, req.dst_addr.into())
            }
            Err(e) => {
                if let Err(e) = ctx.send_mlme_eapol_conf(
                    fidl_mlme::EapolResultCode::TransmissionFailure,
                    req.dst_addr.into(),
                ) {
                    error!("Failed to send eapol transmission failure: {:?}", e);
                }
                Err(e)
            }
        }
    }

    fn make_beacon_frame<D>(
        &self,
        ctx: &Context<D>,
    ) -> Result<(ArenaStaticBox<[u8]>, BeaconOffloadParams), Error> {
        let tim = self.make_tim();
        let (pvb_offset, pvb_bitmap) = tim.make_partial_virtual_bitmap();

        ctx.make_beacon_frame(
            self.beacon_interval,
            self.capabilities,
            &self.ssid,
            &self.rates,
            self.channel,
            ie::TimHeader {
                dtim_count: self.dtim_count,
                dtim_period: self.dtim_period,
                bmp_ctrl: ie::BitmapControl(0)
                    .with_group_traffic(!self.group_buffered.is_empty())
                    .with_offset(pvb_offset),
            },
            pvb_bitmap,
            self.rsne.as_ref().map_or(&[], |rsne| &rsne),
        )
    }

    fn handle_probe_req<D: DeviceOps>(
        &mut self,
        ctx: &mut Context<D>,
        client_addr: MacAddr,
    ) -> Result<(), Rejection> {
        // According to IEEE Std 802.11-2016, 11.1.4.1, we should intersect our IEs with the probe
        // request IEs. However, the client is able to do this anyway so we just send the same IEs
        // as we would with a beacon frame.
        let buffer = ctx
            .make_probe_resp_frame(
                client_addr,
                self.beacon_interval,
                self.capabilities,
                &self.ssid,
                &self.rates,
                self.channel,
                self.rsne.as_ref().map_or(&[], |rsne| &rsne),
            )
            .map_err(|e| Rejection::Client(client_addr, ClientRejection::WlanSendError(e)))?;
        ctx.device.send_wlan_frame(buffer, fidl_softmac::WlanTxInfoFlags::empty(), None).map_err(
            |s| {
                Rejection::Client(
                    client_addr,
                    ClientRejection::WlanSendError(Error::Status(
                        format!("failed to send probe resp"),
                        s,
                    )),
                )
            },
        )
    }

    pub async fn handle_mgmt_frame<B: SplitByteSlice, D: DeviceOps>(
        &mut self,
        ctx: &mut Context<D>,
        mgmt_frame: mac::MgmtFrame<B>,
    ) -> Result<(), Rejection> {
        let frame_ctrl = mgmt_frame.frame_ctrl();
        if frame_ctrl.to_ds() || frame_ctrl.from_ds() {
            // IEEE Std 802.11-2016, 9.2.4.1.4 and Table 9-4: The To DS bit is only set for QMF
            // (QoS Management frame) management frames, and the From DS bit is reserved.
            return Err(Rejection::BadDsBits);
        }

        let to_bss = mgmt_frame.mgmt_hdr.addr1.as_array() == ctx.bssid.as_array()
            && mgmt_frame.mgmt_hdr.addr3.as_array() == ctx.bssid.as_array();
        let client_addr = mgmt_frame.mgmt_hdr.addr2;

        if mgmt_frame.mgmt_subtype() == mac::MgmtSubtype::PROBE_REQ {
            if device::try_query_discovery_support(&mut ctx.device)
                .await
                .map_err(anyhow::Error::from)?
                .probe_response_offload
                .and_then(|offload| offload.supported)
                .unwrap_or(false)
            {
                // We expected the probe response to be handled by hardware.
                return Err(Rejection::Error(format_err!(
                    "driver indicates probe response offload but MLME received a probe response!"
                )));
            }

            if to_bss
                || (mgmt_frame.mgmt_hdr.addr1 == ieee80211::BROADCAST_ADDR
                    && mgmt_frame.mgmt_hdr.addr3 == ieee80211::BROADCAST_ADDR)
            {
                // Allow either probe request sent directly to the AP, or ones that are broadcast.
                for (id, ie_body) in mgmt_frame.into_ies().1 {
                    match id {
                        ie::Id::SSID => {
                            if !ie_body.is_empty() && *ie_body != self.ssid[..] {
                                // Frame is not for this BSS.
                                return Err(Rejection::OtherBss);
                            }
                        }
                        _ => {}
                    }
                }

                // Technically, the probe request must contain an SSID IE (IEEE Std 802.11-2016,
                // 11.1.4.1), but we just treat it here as the same as it being an empty SSID.
                return self.handle_probe_req(ctx, client_addr);
            } else {
                // Frame is not for this BSS.
                return Err(Rejection::OtherBss);
            }
        } else if !to_bss {
            // Frame is not for this BSS.
            return Err(Rejection::OtherBss);
        }

        // We might allocate a client into the Option if there is none present in the map. We do not
        // allocate directly into the map as we do not know yet if the client will even be added
        // (e.g. if the frame being handled is bogus, or the client did not even authenticate).
        let mut new_client = None;
        let client = match self.clients.get_mut(&client_addr) {
            Some(client) => client,
            None => new_client.get_or_insert_with(|| RemoteClient::new(client_addr)),
        };

        if let Err(e) = client
            .handle_mgmt_frame(ctx, self.capabilities, Some(self.ssid.clone()), mgmt_frame)
            .await
        {
            return Err(Rejection::Client(client_addr, e));
        }

        // IEEE Std 802.11-2016, 9.2.4.1.7: The value [of the Power Management subfield] indicates
        // the mode of the STA after the successful completion of the frame exchange sequence.
        match client.set_power_state(ctx, frame_ctrl.power_mgmt()) {
            Err(ClientRejection::NotAssociated) => {
                error!("client {:02X?} tried to doze but is not associated", client_addr);
            }
            Err(e) => {
                return Err(Rejection::Client(client.addr, e));
            }
            Ok(()) => {}
        }

        if client.deauthenticated() {
            if new_client.is_none() {
                // The client needs to be removed from the map, as it was not freshly allocated from
                // handling this frame.
                self.clients.remove(&client_addr);
            }
        } else {
            // The client was successfully authenticated! Remember it here.
            if let Some(client) = new_client.take() {
                self.clients.insert(client_addr, client);
            }
        }

        Ok(())
    }

    /// Handles an incoming data frame.
    ///
    ///
    pub fn handle_data_frame<B: SplitByteSlice, D: DeviceOps>(
        &mut self,
        ctx: &mut Context<D>,
        data_frame: mac::DataFrame<B>,
    ) -> Result<(), Rejection> {
        if mac::data_receiver_addr(&data_frame.fixed_fields).as_array() != ctx.bssid.as_array() {
            // Frame is not for this BSSID.
            return Err(Rejection::OtherBss);
        }

        let frame_ctrl = data_frame.frame_ctrl();
        if !*&frame_ctrl.to_ds() || *&frame_ctrl.from_ds() {
            // IEEE Std 802.11-2016, 9.2.4.1.4 and Table 9-3: Frame was not sent to a distribution
            // system (e.g. an AP), or was received from another distribution system.
            return Err(Rejection::BadDsBits);
        }

        let src_addr =
            mac::data_src_addr(&data_frame.fixed_fields, data_frame.addr4.as_deref().copied())
                .ok_or(Rejection::NoSrcAddr)?;

        // Handle the frame, pretending that the client is an unauthenticated client if we don't
        // know about it.
        let mut maybe_client = None;
        let client = self
            .clients
            .get_mut(&src_addr)
            .unwrap_or_else(|| maybe_client.get_or_insert_with(|| RemoteClient::new(src_addr)));

        client.handle_data_frame(ctx, data_frame).map_err(|e| Rejection::Client(client.addr, e))?;

        // IEEE Std 802.11-2016, 9.2.4.1.7: The value [of the Power Management subfield] indicates
        // the mode of the STA after the successful completion of the frame exchange sequence.
        match client.set_power_state(ctx, frame_ctrl.power_mgmt()) {
            Err(ClientRejection::NotAssociated) => {
                error!("client {:02X?} tried to doze but is not associated", client.addr);
            }
            Err(e) => {
                return Err(Rejection::Client(client.addr, e));
            }
            Ok(()) => {}
        }

        Ok(())
    }

    pub fn handle_ctrl_frame<B: SplitByteSlice, D: DeviceOps>(
        &mut self,
        ctx: &mut Context<D>,
        ctrl_frame: mac::CtrlFrame<B>,
    ) -> Result<(), Rejection> {
        match ctrl_frame.try_into_ctrl_body().ok_or(Rejection::FrameMalformed)? {
            mac::CtrlBody::PsPoll { ps_poll } => {
                let client = match self.clients.get_mut(&ps_poll.ta) {
                    Some(client) => client,
                    _ => {
                        return Err(Rejection::Client(
                            ps_poll.ta,
                            ClientRejection::NotAuthenticated,
                        ));
                    }
                };

                // IEEE 802.11-2016 9.3.1.5 states the ID in the PS-Poll frame is the association ID
                // with the 2 MSBs set to 1.
                const PS_POLL_MASK: u16 = 0b11000000_00000000;
                client
                    .handle_ps_poll(ctx, ps_poll.masked_aid & !PS_POLL_MASK)
                    .map_err(|e| Rejection::Client(client.addr, e))
            }
            _ => Err(Rejection::FrameMalformed),
        }
    }

    pub fn handle_multicast_eth_frame<D: DeviceOps>(
        &mut self,
        ctx: &mut Context<D>,
        hdr: EthernetIIHdr,
        body: &[u8],
        async_id: trace::Id,
    ) -> Result<(), Rejection> {
        let buffer = ctx
            .make_data_frame(
                hdr.da,
                hdr.sa,
                self.rsne.is_some(),
                false, // TODO(https://fxbug.dev/42113580): Support QoS.
                hdr.ether_type.get(),
                body,
            )
            .map_err(|e| Rejection::Client(hdr.da, ClientRejection::WlanSendError(e)))?;
        let tx_flags = fidl_softmac::WlanTxInfoFlags::empty();

        if !self.clients.values().any(|client| client.dozing()) {
            ctx.device.send_wlan_frame(buffer, tx_flags, Some(async_id)).map_err(move |s| {
                Rejection::Client(
                    hdr.da,
                    ClientRejection::WlanSendError(Error::Status(
                        format!("error sending multicast data frame"),
                        s,
                    )),
                )
            })?;
        } else {
            self.group_buffered.push_back(BufferedFrame { buffer, tx_flags, async_id });
        }

        Ok(())
    }

    pub fn handle_eth_frame<D: DeviceOps>(
        &mut self,
        ctx: &mut Context<D>,
        hdr: EthernetIIHdr,
        body: &[u8],
        async_id: trace::Id,
    ) -> Result<(), Rejection> {
        if hdr.da.is_multicast() {
            return self.handle_multicast_eth_frame(ctx, hdr, body, async_id);
        }

        // Handle the frame, pretending that the client is an unauthenticated client if we don't
        // know about it.
        let mut maybe_client = None;
        let client = self
            .clients
            .get_mut(&hdr.da)
            .unwrap_or_else(|| maybe_client.get_or_insert_with(|| RemoteClient::new(hdr.da)));
        client
            .handle_eth_frame(ctx, hdr.da, hdr.sa, hdr.ether_type.get(), body, async_id)
            .map_err(|e| Rejection::Client(client.addr, e))
    }

    // TODO(https://fxbug.dev/335283785): Remove or explain unused code.
    #[allow(dead_code)]
    pub fn handle_bcn_tx_complete_indication<D: DeviceOps>(
        &mut self,
        ctx: &mut Context<D>,
    ) -> Result<(), Error> {
        if self.dtim_count > 0 {
            self.dtim_count -= 1;
            return Ok(());
        }

        self.dtim_count = self.dtim_period;

        let mut buffered = self.group_buffered.drain(..).peekable();
        while let Some(BufferedFrame { mut buffer, tx_flags, async_id }) = buffered.next() {
            if buffered.peek().is_some() {
                frame_writer::set_more_data(&mut buffer[..])?;
            }
            ctx.device
                .send_wlan_frame(buffer, tx_flags, Some(async_id))
                .map_err(|s| Error::Status(format!("error sending buffered frame"), s))?;
        }

        Ok(())
    }

    // Timed event functions

    /// Handles timed events.
    pub async fn handle_timed_event<D: DeviceOps>(
        &mut self,
        ctx: &mut Context<D>,
        event: TimedEvent,
    ) -> Result<(), Rejection> {
        match event {
            TimedEvent::ClientEvent(addr, event) => {
                let client = self.clients.get_mut(&addr).ok_or(Rejection::NoSuchClient(addr))?;
                client.handle_event(ctx, event).await.map_err(|e| Rejection::Client(client.addr, e))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ap::remote_client::ClientEvent;
    use crate::device::{FakeDevice, FakeDeviceConfig, FakeDeviceState};
    use assert_matches::assert_matches;
    use fidl_fuchsia_wlan_ieee80211::WlanBand::TwoGhz;
    use fidl_fuchsia_wlan_ieee80211::{self as fidl_ieee80211};
    use fuchsia_sync::Mutex;
    use ieee80211::Bssid;
    use std::sync::{Arc, LazyLock};
    use test_case::test_case;
    use wlan_common::mac::IntoBytesExt as _;
    use wlan_common::test_utils::fake_frames::fake_wpa2_rsne;
    use wlan_common::timer::{self, create_timer};
    use zerocopy::byteorder::big_endian::U16 as BigEndianU16;

    static CLIENT_ADDR: LazyLock<MacAddr> = LazyLock::new(|| [4u8; 6].into());
    static BSSID: LazyLock<Bssid> = LazyLock::new(|| [2u8; 6].into());
    static CLIENT_ADDR2: LazyLock<MacAddr> = LazyLock::new(|| [6u8; 6].into());
    static REMOTE_ADDR: LazyLock<MacAddr> = LazyLock::new(|| [123u8; 6].into());

    fn make_context(
        fake_device: FakeDevice,
    ) -> (Context<FakeDevice>, timer::EventStream<TimedEvent>) {
        let (timer, time_stream) = create_timer();
        (Context::new(fake_device, timer, *BSSID), time_stream)
    }

    async fn make_infra_bss(ctx: &mut Context<FakeDevice>) -> InfraBss {
        InfraBss::new(
            ctx,
            Ssid::try_from("coolnet").unwrap(),
            TimeUnit::DEFAULT_BEACON_INTERVAL,
            2,
            CapabilityInfo(0),
            vec![0b11111000],
            1,
            TwoGhz,
            None,
        )
        .await
        .expect("expected InfraBss::new ok")
    }

    async fn make_protected_infra_bss(ctx: &mut Context<FakeDevice>) -> InfraBss {
        InfraBss::new(
            ctx,
            Ssid::try_from("coolnet").unwrap(),
            TimeUnit::DEFAULT_BEACON_INTERVAL,
            2,
            CapabilityInfo(0),
            vec![0b11111000],
            1,
            TwoGhz,
            Some(fake_wpa2_rsne()),
        )
        .await
        .expect("expected InfraBss::new ok")
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn new() {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        InfraBss::new(
            &mut ctx,
            Ssid::try_from([1, 2, 3, 4, 5]).unwrap(),
            TimeUnit::DEFAULT_BEACON_INTERVAL,
            2,
            CapabilityInfo(0).with_ess(true),
            vec![0b11111000],
            1,
            TwoGhz,
            None,
        )
        .await
        .expect("expected InfraBss::new ok");

        {
            let state = fake_device_state.lock();
            assert_eq!(
                state.primary_channel,
                fidl_ieee80211::ChannelNumber { band: fidl_ieee80211::WlanBand::TwoGhz, number: 1 }
            );
            assert_eq!(state.bandwidth, fidl_ieee80211::ChannelBandwidth::Cbw20);
            assert_eq!(
                state.vht_secondary_80_channel,
                fidl_ieee80211::ChannelNumber { band: fidl_ieee80211::WlanBand::TwoGhz, number: 0 }
            );
        }

        let beacon_tmpl = vec![
            // Mgmt header
            0b10000000, 0, // Frame Control
            0, 0, // Duration
            255, 255, 255, 255, 255, 255, // addr1
            2, 2, 2, 2, 2, 2, // addr2
            2, 2, 2, 2, 2, 2, // addr3
            0, 0, // Sequence Control
            // Beacon header:
            0, 0, 0, 0, 0, 0, 0, 0, // Timestamp
            100, 0, // Beacon interval
            1, 0, // Capabilities
            // IEs:
            0, 5, 1, 2, 3, 4, 5, // SSID
            1, 1, 0b11111000, // Basic rates
            3, 1, 1, // DSSS parameter set
            5, 4, 0, 2, 0, 0, // TIM
        ];

        assert_eq!(
            fake_device_state.lock().beacon_config.as_ref().expect("expected beacon_config"),
            &(beacon_tmpl, 49, TimeUnit::DEFAULT_BEACON_INTERVAL)
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn stop() {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let bss = make_infra_bss(&mut ctx).await;
        bss.stop(&mut ctx).await.expect("expected InfraBss::stop ok");
        assert!(fake_device_state.lock().beacon_config.is_none());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handle_mlme_auth_resp() {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let mut bss = make_infra_bss(&mut ctx).await;

        bss.clients.insert(*CLIENT_ADDR, RemoteClient::new(*CLIENT_ADDR));

        bss.handle_mlme_auth_resp(
            &mut ctx,
            fidl_mlme::AuthenticateResponse {
                peer_sta_address: CLIENT_ADDR.to_array(),
                result_code: fidl_mlme::AuthenticateResultCode::AntiCloggingTokenRequired,
            },
        )
        .await
        .expect("expected InfraBss::handle_mlme_auth_resp ok");
        assert_eq!(fake_device_state.lock().wlan_queue.len(), 1);
        assert_eq!(
            &fake_device_state.lock().wlan_queue[0].0[..],
            &[
                // Mgmt header
                0b10110000, 0, // Frame Control
                0, 0, // Duration
                4, 4, 4, 4, 4, 4, // addr1
                2, 2, 2, 2, 2, 2, // addr2
                2, 2, 2, 2, 2, 2, // addr3
                0x10, 0, // Sequence Control
                // Auth header:
                0, 0, // auth algorithm
                2, 0, // auth txn seq num
                76, 0, // status code
            ][..]
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handle_mlme_auth_resp_no_such_client() {
        let (fake_device, _) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let mut bss = make_infra_bss(&mut ctx).await;

        assert_eq!(
            zx::Status::from(
                bss.handle_mlme_auth_resp(
                    &mut ctx,
                    fidl_mlme::AuthenticateResponse {
                        peer_sta_address: CLIENT_ADDR.to_array(),
                        result_code: fidl_mlme::AuthenticateResultCode::AntiCloggingTokenRequired,
                    },
                )
                .await
                .expect_err("expected InfraBss::handle_mlme_auth_resp error")
            ),
            zx::Status::NOT_FOUND
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handle_mlme_deauth_req() {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let mut bss = make_infra_bss(&mut ctx).await;

        bss.clients.insert(*CLIENT_ADDR, RemoteClient::new(*CLIENT_ADDR));

        bss.handle_mlme_deauth_req(
            &mut ctx,
            fidl_mlme::DeauthenticateRequest {
                peer_sta_address: CLIENT_ADDR.to_array(),
                reason_code: fidl_ieee80211::ReasonCode::LeavingNetworkDeauth,
            },
        )
        .await
        .expect("expected InfraBss::handle_mlme_deauth_req ok");
        assert_eq!(fake_device_state.lock().wlan_queue.len(), 1);
        assert_eq!(
            &fake_device_state.lock().wlan_queue[0].0[..],
            &[
                // Mgmt header
                0b11000000, 0, // Frame Control
                0, 0, // Duration
                4, 4, 4, 4, 4, 4, // addr1
                2, 2, 2, 2, 2, 2, // addr2
                2, 2, 2, 2, 2, 2, // addr3
                0x10, 0, // Sequence Control
                // Deauth header:
                3, 0, // reason code
            ][..]
        );

        assert!(!bss.clients.contains_key(&CLIENT_ADDR));
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handle_mlme_assoc_resp() {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let mut bss = make_infra_bss(&mut ctx).await;

        bss.clients.insert(*CLIENT_ADDR, RemoteClient::new(*CLIENT_ADDR));
        bss.handle_mlme_assoc_resp(
            &mut ctx,
            fidl_mlme::AssociateResponse {
                peer_sta_address: CLIENT_ADDR.to_array(),
                result_code: fidl_mlme::AssociateResultCode::Success,
                association_id: 1,
                capability_info: 0,
                rates: vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
            },
        )
        .await
        .expect("expected InfraBss::handle_mlme_assoc_resp ok");
        assert_eq!(fake_device_state.lock().wlan_queue.len(), 1);
        assert_eq!(
            &fake_device_state.lock().wlan_queue[0].0[..],
            &[
                // Mgmt header
                0b00010000, 0, // Frame Control
                0, 0, // Duration
                4, 4, 4, 4, 4, 4, // addr1
                2, 2, 2, 2, 2, 2, // addr2
                2, 2, 2, 2, 2, 2, // addr3
                0x10, 0, // Sequence Control
                // Association response header:
                0, 0, // Capabilities
                0, 0, // status code
                1, 0, // AID
                // IEs
                1, 8, 1, 2, 3, 4, 5, 6, 7, 8, // Rates
                50, 2, 9, 10, // Extended rates
                90, 3, 90, 0, 0, // BSS max idle period
            ][..]
        );
        assert!(fake_device_state.lock().assocs.contains_key(&CLIENT_ADDR));
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handle_mlme_assoc_resp_with_caps() {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let mut bss = InfraBss::new(
            &mut ctx,
            Ssid::try_from("coolnet").unwrap(),
            TimeUnit::DEFAULT_BEACON_INTERVAL,
            2,
            CapabilityInfo(0).with_short_preamble(true).with_ess(true),
            vec![0b11111000],
            1,
            TwoGhz,
            None,
        )
        .await
        .expect("expected InfraBss::new ok");

        bss.clients.insert(*CLIENT_ADDR, RemoteClient::new(*CLIENT_ADDR));

        bss.handle_mlme_assoc_resp(
            &mut ctx,
            fidl_mlme::AssociateResponse {
                peer_sta_address: CLIENT_ADDR.to_array(),
                result_code: fidl_mlme::AssociateResultCode::Success,
                association_id: 1,
                capability_info: CapabilityInfo(0).with_short_preamble(true).raw(),
                rates: vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
            },
        )
        .await
        .expect("expected InfraBss::handle_mlme_assoc_resp ok");
        assert_eq!(fake_device_state.lock().wlan_queue.len(), 1);
        assert_eq!(
            &fake_device_state.lock().wlan_queue[0].0[..],
            &[
                // Mgmt header
                0b00010000, 0, // Frame Control
                0, 0, // Duration
                4, 4, 4, 4, 4, 4, // addr1
                2, 2, 2, 2, 2, 2, // addr2
                2, 2, 2, 2, 2, 2, // addr3
                0x10, 0, // Sequence Control
                // Association response header:
                0b00100000, 0b00000000, // Capabilities
                0, 0, // status code
                1, 0, // AID
                // IEs
                1, 8, 1, 2, 3, 4, 5, 6, 7, 8, // Rates
                50, 2, 9, 10, // Extended rates
                90, 3, 90, 0, 0, // BSS max idle period
            ][..]
        );
        assert!(fake_device_state.lock().assocs.contains_key(&CLIENT_ADDR));
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handle_mlme_disassoc_req() {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let mut bss = make_infra_bss(&mut ctx).await;

        bss.clients.insert(*CLIENT_ADDR, RemoteClient::new(*CLIENT_ADDR));

        bss.handle_mlme_disassoc_req(
            &mut ctx,
            fidl_mlme::DisassociateRequest {
                peer_sta_address: CLIENT_ADDR.to_array(),
                reason_code: fidl_ieee80211::ReasonCode::LeavingNetworkDisassoc,
            },
        )
        .await
        .expect("expected InfraBss::handle_mlme_disassoc_req ok");
        assert_eq!(fake_device_state.lock().wlan_queue.len(), 1);
        assert_eq!(
            &fake_device_state.lock().wlan_queue[0].0[..],
            &[
                // Mgmt header
                0b10100000, 0, // Frame Control
                0, 0, // Duration
                4, 4, 4, 4, 4, 4, // addr1
                2, 2, 2, 2, 2, 2, // addr2
                2, 2, 2, 2, 2, 2, // addr3
                0x10, 0, // Sequence Control
                // Disassoc header:
                8, 0, // reason code
            ][..]
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handle_mlme_set_controlled_port_req() {
        let (fake_device, _) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let mut bss = make_protected_infra_bss(&mut ctx).await;

        bss.clients.insert(*CLIENT_ADDR, RemoteClient::new(*CLIENT_ADDR));

        bss.handle_mlme_assoc_resp(
            &mut ctx,
            fidl_mlme::AssociateResponse {
                peer_sta_address: CLIENT_ADDR.to_array(),
                result_code: fidl_mlme::AssociateResultCode::Success,
                association_id: 1,
                capability_info: 0,
                rates: vec![1, 2, 3],
            },
        )
        .await
        .expect("expected InfraBss::handle_mlme_assoc_resp ok");

        bss.handle_mlme_set_controlled_port_req(fidl_mlme::SetControlledPortRequest {
            peer_sta_address: CLIENT_ADDR.to_array(),
            state: fidl_mlme::ControlledPortState::Open,
        })
        .expect("expected InfraBss::handle_mlme_set_controlled_port_req ok");
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handle_mlme_eapol_req() {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let mut bss = make_infra_bss(&mut ctx).await;

        bss.clients.insert(*CLIENT_ADDR, RemoteClient::new(*CLIENT_ADDR));

        bss.handle_mlme_eapol_req(
            &mut ctx,
            fidl_mlme::EapolRequest {
                dst_addr: CLIENT_ADDR.to_array(),
                src_addr: BSSID.to_array(),
                data: vec![1, 2, 3],
            },
        )
        .expect("expected InfraBss::handle_mlme_eapol_req ok");
        assert_eq!(fake_device_state.lock().wlan_queue.len(), 1);
        assert_eq!(
            &fake_device_state.lock().wlan_queue[0].0[..],
            &[
                // Header
                0b00001000, 0b00000010, // Frame Control
                0, 0, // Duration
                4, 4, 4, 4, 4, 4, // addr1
                2, 2, 2, 2, 2, 2, // addr2
                2, 2, 2, 2, 2, 2, // addr3
                0x10, 0, // Sequence Control
                0xAA, 0xAA, 0x03, // DSAP, SSAP, Control, OUI
                0, 0, 0, // OUI
                0x88, 0x8E, // EAPOL protocol ID
                // Data
                1, 2, 3,
            ][..]
        );

        let confirm = fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::EapolConfirm>()
            .expect("Did not receive valid Eapol Confirm msg");
        assert_eq!(confirm.result_code, fidl_mlme::EapolResultCode::Success);
        assert_eq!(&confirm.dst_addr, CLIENT_ADDR.as_array());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handle_mgmt_frame_auth() {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let mut bss = make_infra_bss(&mut ctx).await;

        bss.handle_mgmt_frame(
            &mut ctx,
            mac::MgmtFrame {
                mgmt_hdr: mac::MgmtHdr {
                    frame_ctrl: mac::FrameControl(0)
                        .with_frame_type(mac::FrameType::MGMT)
                        .with_mgmt_subtype(mac::MgmtSubtype::AUTH),
                    duration: 0,
                    addr1: (*BSSID).into(),
                    addr2: *CLIENT_ADDR,
                    addr3: (*BSSID).into(),
                    seq_ctrl: mac::SequenceControl(10),
                }
                .as_bytes_ref(),
                ht_ctrl: None,
                body: &[
                    // Auth body
                    0, 0, // Auth Algorithm Number
                    1, 0, // Auth Txn Seq Number
                    0, 0, // Status code
                ][..],
            },
        )
        .await
        .expect("expected OK");

        assert_eq!(bss.clients.contains_key(&CLIENT_ADDR), true);

        let msg = fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::AuthenticateIndication>()
            .expect("expected MLME message");
        assert_eq!(
            msg,
            fidl_mlme::AuthenticateIndication {
                peer_sta_address: CLIENT_ADDR.to_array(),
                auth_type: fidl_mlme::AuthenticationTypes::OpenSystem,
            },
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handle_mgmt_frame_assoc_req() {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let mut bss = make_infra_bss(&mut ctx).await;

        bss.clients.insert(*CLIENT_ADDR, RemoteClient::new(*CLIENT_ADDR));
        let client = bss.clients.get_mut(&CLIENT_ADDR).unwrap();
        client
            .handle_mlme_auth_resp(&mut ctx, fidl_mlme::AuthenticateResultCode::Success)
            .await
            .expect("expected OK");

        bss.handle_mgmt_frame(
            &mut ctx,
            mac::MgmtFrame {
                mgmt_hdr: mac::MgmtHdr {
                    frame_ctrl: mac::FrameControl(0)
                        .with_frame_type(mac::FrameType::MGMT)
                        .with_mgmt_subtype(mac::MgmtSubtype::ASSOC_REQ),
                    duration: 0,
                    addr1: (*BSSID).into(),
                    addr2: *CLIENT_ADDR,
                    addr3: (*BSSID).into(),
                    seq_ctrl: mac::SequenceControl(10),
                }
                .as_bytes_ref(),
                ht_ctrl: None,
                body: &[
                    // Assoc req body
                    0, 0, // Capability info
                    10, 0, // Listen interval
                    // IEs
                    1, 8, 1, 2, 3, 4, 5, 6, 7, 8, // Rates
                    50, 2, 9, 10, // Extended rates
                    48, 2, 77, 88, // RSNE
                ][..],
            },
        )
        .await
        .expect("expected OK");

        assert_eq!(bss.clients.contains_key(&CLIENT_ADDR), true);

        let msg = fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::AssociateIndication>()
            .expect("expected MLME message");
        assert_eq!(
            msg,
            fidl_mlme::AssociateIndication {
                peer_sta_address: CLIENT_ADDR.to_array(),
                listen_interval: 10,
                ssid: Some(Ssid::try_from("coolnet").unwrap().into()),
                capability_info: 0,
                rates: vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
                rsne: Some(vec![48, 2, 77, 88]),
            },
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handle_mgmt_frame_bad_ds_bits_to_ds() {
        let (fake_device, _) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let mut bss = make_infra_bss(&mut ctx).await;

        assert_matches!(
            bss.handle_mgmt_frame(
                &mut ctx,
                mac::MgmtFrame {
                    mgmt_hdr: mac::MgmtHdr {
                        frame_ctrl: mac::FrameControl(0)
                            .with_frame_type(mac::FrameType::MGMT)
                            .with_mgmt_subtype(mac::MgmtSubtype::AUTH)
                            .with_to_ds(true),
                        duration: 0,
                        addr1: (*BSSID).into(),
                        addr2: *CLIENT_ADDR,
                        addr3: (*BSSID).into(),
                        seq_ctrl: mac::SequenceControl(10),
                    }
                    .as_bytes_ref(),
                    ht_ctrl: None,
                    body: &[
                        // Auth body
                        0, 0, // Auth Algorithm Number
                        1, 0, // Auth Txn Seq Number
                        0, 0, // Status code
                    ][..],
                },
            )
            .await
            .expect_err("expected error"),
            Rejection::BadDsBits
        );

        assert_eq!(bss.clients.contains_key(&CLIENT_ADDR), false);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handle_mgmt_frame_bad_ds_bits_from_ds() {
        let (fake_device, _) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let mut bss = make_infra_bss(&mut ctx).await;

        assert_matches!(
            bss.handle_mgmt_frame(
                &mut ctx,
                mac::MgmtFrame {
                    mgmt_hdr: mac::MgmtHdr {
                        frame_ctrl: mac::FrameControl(0)
                            .with_frame_type(mac::FrameType::MGMT)
                            .with_mgmt_subtype(mac::MgmtSubtype::AUTH)
                            .with_from_ds(true),
                        duration: 0,
                        addr1: (*BSSID).into(),
                        addr2: *CLIENT_ADDR,
                        addr3: (*BSSID).into(),
                        seq_ctrl: mac::SequenceControl(10),
                    }
                    .as_bytes_ref(),
                    ht_ctrl: None,
                    body: &[
                        // Auth body
                        0, 0, // Auth Algorithm Number
                        1, 0, // Auth Txn Seq Number
                        0, 0, // Status code
                    ][..],
                },
            )
            .await
            .expect_err("expected error"),
            Rejection::BadDsBits
        );

        assert_eq!(bss.clients.contains_key(&CLIENT_ADDR), false);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handle_mgmt_frame_no_such_client() {
        let (fake_device, _) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let mut bss = make_infra_bss(&mut ctx).await;

        assert_matches!(
            bss.handle_mgmt_frame(
                &mut ctx,
                mac::MgmtFrame {
                    mgmt_hdr: mac::MgmtHdr {
                        frame_ctrl: mac::FrameControl(0)
                            .with_frame_type(mac::FrameType::MGMT)
                            .with_mgmt_subtype(mac::MgmtSubtype::DISASSOC),
                        duration: 0,
                        addr1: (*BSSID).into(),
                        addr2: *CLIENT_ADDR,
                        addr3: (*BSSID).into(),
                        seq_ctrl: mac::SequenceControl(10),
                    }
                    .as_bytes_ref(),
                    ht_ctrl: None,
                    body: &[
                        // Disassoc header:
                        8, 0, // reason code
                    ][..],
                },
            )
            .await
            .expect_err("expected error"),
            Rejection::Client(_, ClientRejection::NotPermitted)
        );

        assert_eq!(bss.clients.contains_key(&CLIENT_ADDR), false);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handle_mgmt_frame_bogus() {
        let (fake_device, _) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let mut bss = make_infra_bss(&mut ctx).await;

        assert_matches!(
            bss.handle_mgmt_frame(
                &mut ctx,
                mac::MgmtFrame {
                    mgmt_hdr: mac::MgmtHdr {
                        frame_ctrl: mac::FrameControl(0)
                            .with_frame_type(mac::FrameType::MGMT)
                            .with_mgmt_subtype(mac::MgmtSubtype::AUTH),
                        duration: 0,
                        addr1: (*BSSID).into(),
                        addr2: *CLIENT_ADDR,
                        addr3: (*BSSID).into(),
                        seq_ctrl: mac::SequenceControl(10),
                    }
                    .as_bytes_ref(),
                    ht_ctrl: None,
                    // Auth frame should have a header; doesn't.
                    body: &[][..],
                },
            )
            .await
            .expect_err("expected error"),
            Rejection::Client(_, ClientRejection::ParseFailed)
        );

        assert_eq!(bss.clients.contains_key(&CLIENT_ADDR), false);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handle_data_frame() {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let mut bss = make_infra_bss(&mut ctx).await;

        bss.clients.insert(*CLIENT_ADDR, RemoteClient::new(*CLIENT_ADDR));
        let client = bss.clients.get_mut(&CLIENT_ADDR).unwrap();

        // Move the client to associated so it can handle data frames.
        client
            .handle_mlme_auth_resp(&mut ctx, fidl_mlme::AuthenticateResultCode::Success)
            .await
            .expect("expected OK");
        client
            .handle_mlme_assoc_resp(
                &mut ctx,
                false,
                fidl_ieee80211::ChannelNumber { number: 1, band: TwoGhz },
                mac::CapabilityInfo(0),
                fidl_mlme::AssociateResultCode::Success,
                1,
                &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10][..],
            )
            .await
            .expect("expected OK");

        bss.handle_data_frame(
            &mut ctx,
            mac::DataFrame {
                fixed_fields: mac::FixedDataHdrFields {
                    frame_ctrl: mac::FrameControl(0)
                        .with_frame_type(mac::FrameType::DATA)
                        .with_to_ds(true),
                    duration: 0,
                    addr1: (*BSSID).into(),
                    addr2: *CLIENT_ADDR,
                    addr3: *CLIENT_ADDR2,
                    seq_ctrl: mac::SequenceControl(10),
                }
                .as_bytes_ref(),
                addr4: None,
                qos_ctrl: None,
                ht_ctrl: None,
                body: &[
                    7, 7, 7, // DSAP, SSAP & control
                    8, 8, 8, // OUI
                    0x12, 0x34, // eth type
                    // Trailing bytes
                    1, 2, 3, 4, 5,
                ][..],
            },
        )
        .expect("expected OK");

        assert_eq!(fake_device_state.lock().eth_queue.len(), 1);
        assert_eq!(
            &fake_device_state.lock().eth_queue[0][..],
            &[
                6, 6, 6, 6, 6, 6, // dest
                4, 4, 4, 4, 4, 4, // src
                0x12, 0x34, // ether_type
                // Data
                1, 2, 3, 4, 5,
            ][..]
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handle_data_frame_bad_ds_bits() {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let mut bss = make_infra_bss(&mut ctx).await;

        assert_matches!(
            bss.handle_data_frame(
                &mut ctx,
                mac::DataFrame {
                    fixed_fields: mac::FixedDataHdrFields {
                        frame_ctrl: mac::FrameControl(0)
                            .with_frame_type(mac::FrameType::DATA)
                            .with_to_ds(false),
                        duration: 0,
                        addr1: (*BSSID).into(),
                        addr2: *CLIENT_ADDR,
                        addr3: *CLIENT_ADDR2,
                        seq_ctrl: mac::SequenceControl(10),
                    }
                    .as_bytes_ref(),
                    addr4: None,
                    qos_ctrl: None,
                    ht_ctrl: None,
                    body: &[
                        7, 7, 7, // DSAP, SSAP & control
                        8, 8, 8, // OUI
                        0x12, 0x34, // eth type
                        // Trailing bytes
                        1, 2, 3, 4, 5,
                    ][..],
                },
            )
            .expect_err("expected error"),
            Rejection::BadDsBits
        );

        assert_eq!(fake_device_state.lock().eth_queue.len(), 0);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handle_client_event() {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        let (mut ctx, mut time_stream) = make_context(fake_device);
        let mut bss = make_infra_bss(&mut ctx).await;

        bss.clients.insert(*CLIENT_ADDR, RemoteClient::new(*CLIENT_ADDR));
        let client = bss.clients.get_mut(&CLIENT_ADDR).unwrap();

        // Move the client to associated so it can handle data frames.
        client
            .handle_mlme_auth_resp(&mut ctx, fidl_mlme::AuthenticateResultCode::Success)
            .await
            .expect("expected OK");
        client
            .handle_mlme_assoc_resp(
                &mut ctx,
                false,
                fidl_ieee80211::ChannelNumber { number: 1, band: TwoGhz },
                mac::CapabilityInfo(0),
                fidl_mlme::AssociateResultCode::Success,
                1,
                &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10][..],
            )
            .await
            .expect("expected OK");

        fake_device_state.lock().wlan_queue.clear();

        let _ = time_stream.try_next().unwrap().expect("Should have scheduled a timeout");
        bss.handle_timed_event(
            &mut ctx,
            TimedEvent::ClientEvent(*CLIENT_ADDR, ClientEvent::BssIdleTimeout),
        )
        .await
        .expect("expected OK");

        // Check that we received a disassociation frame.
        assert_eq!(fake_device_state.lock().wlan_queue.len(), 1);
        #[rustfmt::skip]
        assert_eq!(&fake_device_state.lock().wlan_queue[0].0[..], &[
            // Mgmt header
            0b10100000, 0, // Frame Control
            0, 0, // Duration
            4, 4, 4, 4, 4, 4, // addr1
            2, 2, 2, 2, 2, 2, // addr2
            2, 2, 2, 2, 2, 2, // addr3
            0x30, 0, // Sequence Control
            // Disassoc header:
            4, 0, // reason code
        ][..]);

        let msg = fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::DisassociateIndication>()
            .expect("expected MLME message");
        assert_eq!(
            msg,
            fidl_mlme::DisassociateIndication {
                peer_sta_address: CLIENT_ADDR.to_array(),
                reason_code: fidl_ieee80211::ReasonCode::ReasonInactivity,
                locally_initiated: true,
            },
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handle_data_frame_no_such_client() {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let mut bss = make_infra_bss(&mut ctx).await;

        assert_matches!(
            bss.handle_data_frame(
                &mut ctx,
                mac::DataFrame {
                    fixed_fields: mac::FixedDataHdrFields {
                        frame_ctrl: mac::FrameControl(0)
                            .with_frame_type(mac::FrameType::DATA)
                            .with_to_ds(true),
                        duration: 0,
                        addr1: (*BSSID).into(),
                        addr2: *CLIENT_ADDR,
                        addr3: *CLIENT_ADDR2,
                        seq_ctrl: mac::SequenceControl(10),
                    }
                    .as_bytes_ref(),
                    addr4: None,
                    qos_ctrl: None,
                    ht_ctrl: None,
                    body: &[
                        7, 7, 7, // DSAP, SSAP & control
                        8, 8, 8, // OUI
                        0x12, 0x34, // eth type
                        // Trailing bytes
                        1, 2, 3, 4, 5,
                    ][..],
                },
            )
            .expect_err("expected error"),
            Rejection::Client(_, ClientRejection::NotPermitted)
        );

        assert_eq!(fake_device_state.lock().eth_queue.len(), 0);

        assert_eq!(fake_device_state.lock().wlan_queue.len(), 1);
        assert_eq!(
            fake_device_state.lock().wlan_queue[0].0,
            &[
                // Mgmt header
                0b11000000, 0b00000000, // Frame Control
                0, 0, // Duration
                4, 4, 4, 4, 4, 4, // addr1
                2, 2, 2, 2, 2, 2, // addr2
                2, 2, 2, 2, 2, 2, // addr3
                0x10, 0, // Sequence Control
                // Disassoc header:
                7, 0, // reason code
            ][..]
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handle_data_frame_client_not_associated() {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let mut bss = make_infra_bss(&mut ctx).await;

        bss.clients.insert(*CLIENT_ADDR, RemoteClient::new(*CLIENT_ADDR));
        let client = bss.clients.get_mut(&CLIENT_ADDR).unwrap();

        // Move the client to authenticated, but not associated: data frames are still not
        // permitted.
        client
            .handle_mlme_auth_resp(&mut ctx, fidl_mlme::AuthenticateResultCode::Success)
            .await
            .expect("expected OK");

        fake_device_state.lock().wlan_queue.clear();

        assert_matches!(
            bss.handle_data_frame(
                &mut ctx,
                mac::DataFrame {
                    fixed_fields: mac::FixedDataHdrFields {
                        frame_ctrl: mac::FrameControl(0)
                            .with_frame_type(mac::FrameType::DATA)
                            .with_to_ds(true),
                        duration: 0,
                        addr1: (*BSSID).into(),
                        addr2: *CLIENT_ADDR,
                        addr3: *CLIENT_ADDR2,
                        seq_ctrl: mac::SequenceControl(10),
                    }
                    .as_bytes_ref(),
                    addr4: None,
                    qos_ctrl: None,
                    ht_ctrl: None,
                    body: &[
                        7, 7, 7, // DSAP, SSAP & control
                        8, 8, 8, // OUI
                        0x12, 0x34, // eth type
                        // Trailing bytes
                        1, 2, 3, 4, 5,
                    ][..],
                },
            )
            .expect_err("expected error"),
            Rejection::Client(_, ClientRejection::NotPermitted)
        );

        assert_eq!(fake_device_state.lock().eth_queue.len(), 0);

        assert_eq!(fake_device_state.lock().wlan_queue.len(), 1);
        assert_eq!(
            fake_device_state.lock().wlan_queue[0].0,
            &[
                // Mgmt header
                0b10100000, 0b00000000, // Frame Control
                0, 0, // Duration
                4, 4, 4, 4, 4, 4, // addr1
                2, 2, 2, 2, 2, 2, // addr2
                2, 2, 2, 2, 2, 2, // addr3
                0x20, 0, // Sequence Control
                // Disassoc header:
                7, 0, // reason code
            ][..]
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handle_eth_frame_no_rsn() {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let mut bss = make_infra_bss(&mut ctx).await;
        bss.clients.insert(*CLIENT_ADDR, RemoteClient::new(*CLIENT_ADDR));

        let client = bss.clients.get_mut(&CLIENT_ADDR).unwrap();
        client
            .handle_mlme_auth_resp(&mut ctx, fidl_mlme::AuthenticateResultCode::Success)
            .await
            .expect("expected OK");
        client
            .handle_mlme_assoc_resp(
                &mut ctx,
                false,
                fidl_ieee80211::ChannelNumber { number: 1, band: TwoGhz },
                mac::CapabilityInfo(0),
                fidl_mlme::AssociateResultCode::Success,
                1,
                &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10][..],
            )
            .await
            .expect("expected OK");
        fake_device_state.lock().wlan_queue.clear();

        bss.handle_eth_frame(
            &mut ctx,
            EthernetIIHdr {
                da: *CLIENT_ADDR,
                sa: *CLIENT_ADDR2,
                ether_type: BigEndianU16::new(0x1234),
            },
            &[1, 2, 3, 4, 5][..],
            0.into(),
        )
        .expect("expected OK");

        assert_eq!(fake_device_state.lock().wlan_queue.len(), 1);
        assert_eq!(
            &fake_device_state.lock().wlan_queue[0].0[..],
            &[
                // Mgmt header
                0b00001000, 0b00000010, // Frame Control
                0, 0, // Duration
                4, 4, 4, 4, 4, 4, // addr1
                2, 2, 2, 2, 2, 2, // addr2
                6, 6, 6, 6, 6, 6, // addr3
                0x30, 0, // Sequence Control
                0xAA, 0xAA, 0x03, // DSAP, SSAP, Control, OUI
                0, 0, 0, // OUI
                0x12, 0x34, // Protocol ID
                // Data
                1, 2, 3, 4, 5,
            ][..]
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handle_eth_frame_no_client() {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let mut bss = make_infra_bss(&mut ctx).await;

        assert_matches!(
            bss.handle_eth_frame(
                &mut ctx,
                EthernetIIHdr {
                    da: *CLIENT_ADDR,
                    sa: *CLIENT_ADDR2,
                    ether_type: BigEndianU16::new(0x1234)
                },
                &[1, 2, 3, 4, 5][..],
                0.into(),
            )
            .expect_err("expected error"),
            Rejection::Client(_, ClientRejection::NotAssociated)
        );

        assert_eq!(fake_device_state.lock().wlan_queue.len(), 0);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handle_eth_frame_is_rsn_eapol_controlled_port_closed() {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let mut bss = make_protected_infra_bss(&mut ctx).await;
        bss.clients.insert(*CLIENT_ADDR, RemoteClient::new(*CLIENT_ADDR));

        let client = bss.clients.get_mut(&CLIENT_ADDR).unwrap();
        client
            .handle_mlme_auth_resp(&mut ctx, fidl_mlme::AuthenticateResultCode::Success)
            .await
            .expect("expected OK");
        client
            .handle_mlme_assoc_resp(
                &mut ctx,
                true,
                fidl_ieee80211::ChannelNumber { number: 1, band: TwoGhz },
                mac::CapabilityInfo(0),
                fidl_mlme::AssociateResultCode::Success,
                1,
                &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10][..],
            )
            .await
            .expect("expected OK");
        fake_device_state.lock().wlan_queue.clear();

        assert_matches!(
            bss.handle_eth_frame(
                &mut ctx,
                EthernetIIHdr {
                    da: *CLIENT_ADDR,
                    sa: *CLIENT_ADDR2,
                    ether_type: BigEndianU16::new(0x1234)
                },
                &[1, 2, 3, 4, 5][..],
                0.into(),
            )
            .expect_err("expected error"),
            Rejection::Client(_, ClientRejection::ControlledPortClosed)
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handle_eth_frame_is_rsn_eapol_controlled_port_open() {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let mut bss = make_protected_infra_bss(&mut ctx).await;
        bss.clients.insert(*CLIENT_ADDR, RemoteClient::new(*CLIENT_ADDR));

        let client = bss.clients.get_mut(&CLIENT_ADDR).unwrap();
        client
            .handle_mlme_auth_resp(&mut ctx, fidl_mlme::AuthenticateResultCode::Success)
            .await
            .expect("expected OK");
        client
            .handle_mlme_assoc_resp(
                &mut ctx,
                true,
                fidl_ieee80211::ChannelNumber { number: 1, band: TwoGhz },
                mac::CapabilityInfo(0),
                fidl_mlme::AssociateResultCode::Success,
                1,
                &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10][..],
            )
            .await
            .expect("expected OK");
        fake_device_state.lock().wlan_queue.clear();

        client
            .handle_mlme_set_controlled_port_req(fidl_mlme::ControlledPortState::Open)
            .expect("expected OK");

        bss.handle_eth_frame(
            &mut ctx,
            EthernetIIHdr {
                da: *CLIENT_ADDR,
                sa: *CLIENT_ADDR2,
                ether_type: BigEndianU16::new(0x1234),
            },
            &[1, 2, 3, 4, 5][..],
            0.into(),
        )
        .expect("expected OK");

        assert_eq!(fake_device_state.lock().wlan_queue.len(), 1);
        assert_eq!(
            &fake_device_state.lock().wlan_queue[0].0[..],
            &[
                // Mgmt header
                0b00001000, 0b01000010, // Frame Control
                0, 0, // Duration
                4, 4, 4, 4, 4, 4, // addr1
                2, 2, 2, 2, 2, 2, // addr2
                6, 6, 6, 6, 6, 6, // addr3
                0x30, 0, // Sequence Control
                0xAA, 0xAA, 0x03, // DSAP, SSAP, Control, OUI
                0, 0, 0, // OUI
                0x12, 0x34, // Protocol ID
                // Data
                1, 2, 3, 4, 5,
            ][..]
        );
    }

    #[test_case(false; "Controlled port closed")]
    #[test_case(true; "Controlled port open")]
    #[fuchsia::test(allow_stalls = false)]
    async fn handle_data_frame_is_rsn_eapol(controlled_port_open: bool) {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let mut bss = make_protected_infra_bss(&mut ctx).await;
        bss.clients.insert(*CLIENT_ADDR, RemoteClient::new(*CLIENT_ADDR));

        let client = bss.clients.get_mut(&CLIENT_ADDR).unwrap();
        client
            .handle_mlme_auth_resp(&mut ctx, fidl_mlme::AuthenticateResultCode::Success)
            .await
            .expect("expected OK");
        client
            .handle_mlme_assoc_resp(
                &mut ctx,
                true,
                fidl_ieee80211::ChannelNumber { number: 1, band: TwoGhz },
                mac::CapabilityInfo(0),
                fidl_mlme::AssociateResultCode::Success,
                1,
                &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10][..],
            )
            .await
            .expect("expected OK");
        fake_device_state.lock().wlan_queue.clear();

        if controlled_port_open {
            client
                .handle_mlme_set_controlled_port_req(fidl_mlme::ControlledPortState::Open)
                .expect("expected OK");
        }

        bss.handle_data_frame(
            &mut ctx,
            mac::DataFrame {
                fixed_fields: mac::FixedDataHdrFields {
                    frame_ctrl: mac::FrameControl(0)
                        .with_frame_type(mac::FrameType::DATA)
                        .with_to_ds(true),
                    duration: 0,
                    addr1: (*BSSID).into(),
                    addr2: *CLIENT_ADDR,
                    addr3: *CLIENT_ADDR2,
                    seq_ctrl: mac::SequenceControl(10),
                }
                .as_bytes_ref(),
                addr4: None,
                qos_ctrl: None,
                ht_ctrl: None,
                body: &[
                    7, 7, 7, // DSAP, SSAP & control
                    8, 8, 8, // OUI
                    0x12, 0x34, // eth type
                    // Trailing bytes
                    1, 2, 3, 4, 5,
                ][..],
            },
        )
        .expect("expected OK");

        if controlled_port_open {
            assert_eq!(fake_device_state.lock().eth_queue.len(), 1);
        } else {
            assert!(fake_device_state.lock().eth_queue.is_empty());
        }
    }

    async fn authenticate_client(
        fake_device_state: Arc<Mutex<FakeDeviceState>>,
        ctx: &mut Context<FakeDevice>,
        bss: &mut InfraBss,
        client_addr: MacAddr,
    ) {
        bss.handle_mgmt_frame(
            ctx,
            mac::MgmtFrame {
                mgmt_hdr: mac::MgmtHdr {
                    frame_ctrl: mac::FrameControl(0)
                        .with_frame_type(mac::FrameType::MGMT)
                        .with_mgmt_subtype(mac::MgmtSubtype::AUTH),
                    duration: 0,
                    addr1: (*BSSID).into(),
                    addr2: client_addr,
                    addr3: (*BSSID).into(),
                    seq_ctrl: mac::SequenceControl(10),
                }
                .as_bytes_ref(),
                ht_ctrl: None,
                body: &[
                    // Auth body
                    0, 0, // Auth Algorithm Number
                    1, 0, // Auth Txn Seq Number
                    0, 0, // Status code
                ][..],
            },
        )
        .await
        .expect("failed to handle auth req frame");

        fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::AuthenticateIndication>()
            .expect("expected auth indication");
        bss.handle_mlme_auth_resp(
            ctx,
            fidl_mlme::AuthenticateResponse {
                peer_sta_address: client_addr.to_array(),
                result_code: fidl_mlme::AuthenticateResultCode::Success,
            },
        )
        .await
        .expect("failed to handle auth resp");
        assert_eq!(fake_device_state.lock().wlan_queue.len(), 1);
        fake_device_state.lock().wlan_queue.clear();
    }

    async fn associate_client(
        fake_device_state: Arc<Mutex<FakeDeviceState>>,
        ctx: &mut Context<FakeDevice>,
        bss: &mut InfraBss,
        client_addr: MacAddr,
        association_id: u16,
    ) {
        bss.handle_mgmt_frame(
            ctx,
            mac::MgmtFrame {
                mgmt_hdr: mac::MgmtHdr {
                    frame_ctrl: mac::FrameControl(0)
                        .with_frame_type(mac::FrameType::MGMT)
                        .with_mgmt_subtype(mac::MgmtSubtype::ASSOC_REQ),
                    duration: 0,
                    addr1: (*BSSID).into(),
                    addr2: client_addr,
                    addr3: (*BSSID).into(),
                    seq_ctrl: mac::SequenceControl(10),
                }
                .as_bytes_ref(),
                ht_ctrl: None,
                body: &[
                    // Assoc req body
                    0, 0, // Capability info
                    10, 0, // Listen interval
                    // IEs
                    1, 8, 1, 2, 3, 4, 5, 6, 7, 8, // Rates
                    50, 2, 9, 10, // Extended rates
                    48, 2, 77, 88, // RSNE
                ][..],
            },
        )
        .await
        .expect("expected OK");
        let msg = fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::AssociateIndication>()
            .expect("expected assoc indication");
        bss.handle_mlme_assoc_resp(
            ctx,
            fidl_mlme::AssociateResponse {
                peer_sta_address: client_addr.to_array(),
                result_code: fidl_mlme::AssociateResultCode::Success,
                association_id,
                capability_info: msg.capability_info,
                rates: msg.rates,
            },
        )
        .await
        .expect("failed to handle assoc resp");
        assert_eq!(fake_device_state.lock().wlan_queue.len(), 1);
        fake_device_state.lock().wlan_queue.clear();
    }

    fn send_eth_frame_from_ds_to_client(
        ctx: &mut Context<FakeDevice>,
        bss: &mut InfraBss,
        client_addr: MacAddr,
    ) {
        bss.handle_eth_frame(
            ctx,
            EthernetIIHdr {
                da: client_addr,
                sa: *REMOTE_ADDR,
                ether_type: BigEndianU16::new(0x1234),
            },
            &[1, 2, 3, 4, 5][..],
            0.into(),
        )
        .expect("expected OK");
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handle_multiple_complete_associations() {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let mut bss = make_infra_bss(&mut ctx).await;

        authenticate_client(fake_device_state.clone(), &mut ctx, &mut bss, *CLIENT_ADDR).await;
        authenticate_client(fake_device_state.clone(), &mut ctx, &mut bss, *CLIENT_ADDR2).await;

        associate_client(fake_device_state.clone(), &mut ctx, &mut bss, *CLIENT_ADDR, 1).await;
        associate_client(fake_device_state.clone(), &mut ctx, &mut bss, *CLIENT_ADDR2, 2).await;

        assert!(bss.clients.contains_key(&CLIENT_ADDR));
        assert!(bss.clients.contains_key(&CLIENT_ADDR2));

        send_eth_frame_from_ds_to_client(&mut ctx, &mut bss, *CLIENT_ADDR);
        send_eth_frame_from_ds_to_client(&mut ctx, &mut bss, *CLIENT_ADDR2);

        assert_eq!(fake_device_state.lock().wlan_queue.len(), 2);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handle_ps_poll() {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let mut bss = make_infra_bss(&mut ctx).await;
        bss.clients.insert(*CLIENT_ADDR, RemoteClient::new(*CLIENT_ADDR));

        let client = bss.clients.get_mut(&CLIENT_ADDR).unwrap();
        client
            .handle_mlme_auth_resp(&mut ctx, fidl_mlme::AuthenticateResultCode::Success)
            .await
            .expect("expected OK");
        client
            .handle_mlme_assoc_resp(
                &mut ctx,
                false,
                fidl_ieee80211::ChannelNumber { number: 1, band: TwoGhz },
                mac::CapabilityInfo(0),
                fidl_mlme::AssociateResultCode::Success,
                1,
                &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10][..],
            )
            .await
            .expect("expected OK");
        client.set_power_state(&mut ctx, mac::PowerState::DOZE).expect("expected doze ok");
        fake_device_state.lock().wlan_queue.clear();

        bss.handle_eth_frame(
            &mut ctx,
            EthernetIIHdr {
                da: *CLIENT_ADDR,
                sa: *CLIENT_ADDR2,
                ether_type: BigEndianU16::new(0x1234),
            },
            &[1, 2, 3, 4, 5][..],
            0.into(),
        )
        .expect("expected OK");
        assert_eq!(fake_device_state.lock().wlan_queue.len(), 0);

        bss.handle_ctrl_frame(
            &mut ctx,
            mac::CtrlFrame {
                frame_ctrl: mac::FrameControl(0)
                    .with_frame_type(mac::FrameType::CTRL)
                    .with_ctrl_subtype(mac::CtrlSubtype::PS_POLL),
                body: &[
                    0b00000001, 0b11000000, // Masked AID
                    2, 2, 2, 2, 2, 2, // RA
                    4, 4, 4, 4, 4, 4, // TA
                ][..],
            },
        )
        .expect("expected OK");
        assert_eq!(fake_device_state.lock().wlan_queue.len(), 1);
        assert_eq!(
            &fake_device_state.lock().wlan_queue[0].0[..],
            &[
                // Mgmt header
                0b00001000, 0b00000010, // Frame Control
                0, 0, // Duration
                4, 4, 4, 4, 4, 4, // addr1
                2, 2, 2, 2, 2, 2, // addr2
                6, 6, 6, 6, 6, 6, // addr3
                0x30, 0, // Sequence Control
                0xAA, 0xAA, 0x03, // DSAP, SSAP, Control, OUI
                0, 0, 0, // OUI
                0x12, 0x34, // Protocol ID
                // Data
                1, 2, 3, 4, 5,
            ][..]
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handle_mlme_setkeys_req() {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let mut bss = make_protected_infra_bss(&mut ctx).await;
        bss.handle_mlme_setkeys_req(
            &mut ctx,
            vec![fidl_mlme::SetKeyDescriptor {
                cipher_suite_oui: [1, 2, 3],
                cipher_suite_type: fidl_ieee80211::CipherSuiteType::from_primitive_allow_unknown(4),
                key_type: fidl_mlme::KeyType::Pairwise,
                address: [5; 6].into(),
                key_id: 6,
                key: vec![1, 2, 3, 4, 5, 6, 7],
                rsc: 8,
            }],
        )
        .await
        .expect("expected InfraBss::handle_mlme_setkeys_req OK");
        assert_eq!(
            fake_device_state.lock().keys,
            vec![fidl_softmac::WlanKeyConfiguration {
                protection: Some(fidl_softmac::WlanProtection::RxTx),
                cipher_oui: Some([1, 2, 3]),
                cipher_type: Some(4),
                key_type: Some(fidl_ieee80211::KeyType::Pairwise),
                peer_addr: Some([5; 6]),
                key_idx: Some(6),
                key: Some(vec![1, 2, 3, 4, 5, 6, 7]),
                rsc: Some(8),
                ..Default::default()
            }]
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handle_mlme_setkeys_req_no_rsne() {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let mut bss = make_infra_bss(&mut ctx).await;
        assert_matches!(
            bss.handle_mlme_setkeys_req(
                &mut ctx,
                vec![fidl_mlme::SetKeyDescriptor {
                    cipher_suite_oui: [1, 2, 3],
                    cipher_suite_type:
                        fidl_ieee80211::CipherSuiteType::from_primitive_allow_unknown(4),
                    key_type: fidl_mlme::KeyType::Pairwise,
                    address: [5; 6],
                    key_id: 6,
                    key: vec![1, 2, 3, 4, 5, 6, 7],
                    rsc: 8,
                }]
            )
            .await
            .expect_err("expected InfraBss::handle_mlme_setkeys_req error"),
            Error::Status(_, zx::Status::BAD_STATE)
        );
        assert!(fake_device_state.lock().keys.is_empty());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handle_probe_req() {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let mut bss = InfraBss::new(
            &mut ctx,
            Ssid::try_from([1, 2, 3, 4, 5]).unwrap(),
            TimeUnit::DEFAULT_BEACON_INTERVAL,
            2,
            mac::CapabilityInfo(33),
            vec![248],
            1,
            TwoGhz,
            Some(vec![48, 2, 77, 88]),
        )
        .await
        .expect("expected InfraBss::new ok");

        bss.handle_probe_req(&mut ctx, *CLIENT_ADDR)
            .expect("expected InfraBss::handle_probe_req ok");
        assert_eq!(fake_device_state.lock().wlan_queue.len(), 1);
        assert_eq!(
            &fake_device_state.lock().wlan_queue[0].0[..],
            &[
                // Mgmt header
                0b01010000, 0, // Frame Control
                0, 0, // Duration
                4, 4, 4, 4, 4, 4, // addr1
                2, 2, 2, 2, 2, 2, // addr2
                2, 2, 2, 2, 2, 2, // addr3
                0x10, 0, // Sequence Control
                // Beacon header:
                0, 0, 0, 0, 0, 0, 0, 0, // Timestamp
                100, 0, // Beacon interval
                33, 0, // Capabilities
                // IEs:
                0, 5, 1, 2, 3, 4, 5, // SSID
                1, 1, 248, // Supported rates
                3, 1, 1, // DSSS parameter set
                48, 2, 77, 88, // RSNE
            ][..]
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handle_probe_req_has_offload() {
        let (fake_device, _fake_device_state) = FakeDevice::new_with_config(
            FakeDeviceConfig::default().with_mock_probe_response_offload(
                fidl_softmac::ProbeResponseOffloadExtension {
                    supported: Some(true),
                    ..Default::default()
                },
            ),
        )
        .await;

        let (mut ctx, _) = make_context(fake_device);
        let mut bss = InfraBss::new(
            &mut ctx,
            Ssid::try_from([1, 2, 3, 4, 5]).unwrap(),
            TimeUnit::DEFAULT_BEACON_INTERVAL,
            2,
            CapabilityInfo(33),
            vec![0b11111000],
            1,
            TwoGhz,
            Some(vec![48, 2, 77, 88]),
        )
        .await
        .expect("expected InfraBss::new ok");

        bss.handle_mgmt_frame(
            &mut ctx,
            mac::MgmtFrame {
                mgmt_hdr: mac::MgmtHdr {
                    frame_ctrl: mac::FrameControl(0)
                        .with_frame_type(mac::FrameType::MGMT)
                        .with_mgmt_subtype(mac::MgmtSubtype::PROBE_REQ),
                    duration: 0,
                    addr1: (*BSSID).into(),
                    addr2: *CLIENT_ADDR,
                    addr3: (*BSSID).into(),
                    seq_ctrl: mac::SequenceControl(10),
                }
                .as_bytes_ref(),
                ht_ctrl: None,
                body: &[][..],
            },
        )
        .await
        .expect_err("expected InfraBss::handle_mgmt_frame error");
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handle_probe_req_wildcard_ssid() {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let mut bss = InfraBss::new(
            &mut ctx,
            Ssid::try_from([1, 2, 3, 4, 5]).unwrap(),
            TimeUnit::DEFAULT_BEACON_INTERVAL,
            2,
            CapabilityInfo(33),
            vec![0b11111000],
            1,
            TwoGhz,
            Some(vec![48, 2, 77, 88]),
        )
        .await
        .expect("expected InfraBss::new ok");

        bss.handle_mgmt_frame(
            &mut ctx,
            mac::MgmtFrame {
                mgmt_hdr: mac::MgmtHdr {
                    frame_ctrl: mac::FrameControl(0)
                        .with_frame_type(mac::FrameType::MGMT)
                        .with_mgmt_subtype(mac::MgmtSubtype::PROBE_REQ),
                    duration: 0,
                    addr1: (*BSSID).into(),
                    addr2: *CLIENT_ADDR,
                    addr3: (*BSSID).into(),
                    seq_ctrl: mac::SequenceControl(10),
                }
                .as_bytes_ref(),
                ht_ctrl: None,
                body: &[
                    0, 0, // Wildcard SSID
                ][..],
            },
        )
        .await
        .expect("expected InfraBss::handle_mgmt_frame ok");

        assert_eq!(fake_device_state.lock().wlan_queue.len(), 1);
        assert_eq!(
            &fake_device_state.lock().wlan_queue[0].0[..],
            &[
                // Mgmt header
                0b01010000, 0, // Frame Control
                0, 0, // Duration
                4, 4, 4, 4, 4, 4, // addr1
                2, 2, 2, 2, 2, 2, // addr2
                2, 2, 2, 2, 2, 2, // addr3
                0x10, 0, // Sequence Control
                // Beacon header:
                0, 0, 0, 0, 0, 0, 0, 0, // Timestamp
                100, 0, // Beacon interval
                33, 0, // Capabilities
                // IEs:
                0, 5, 1, 2, 3, 4, 5, // SSID
                1, 1, 248, // Supported rates
                3, 1, 1, // DSSS parameter set
                48, 2, 77, 88, // RSNE
            ][..]
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handle_probe_req_matching_ssid() {
        let (fake_device, fake_device_state) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let mut bss = InfraBss::new(
            &mut ctx,
            Ssid::try_from([1, 2, 3, 4, 5]).unwrap(),
            TimeUnit::DEFAULT_BEACON_INTERVAL,
            2,
            CapabilityInfo(33),
            vec![0b11111000],
            1,
            TwoGhz,
            Some(vec![48, 2, 77, 88]),
        )
        .await
        .expect("expected InfraBss::new ok");

        bss.handle_mgmt_frame(
            &mut ctx,
            mac::MgmtFrame {
                mgmt_hdr: mac::MgmtHdr {
                    frame_ctrl: mac::FrameControl(0)
                        .with_frame_type(mac::FrameType::MGMT)
                        .with_mgmt_subtype(mac::MgmtSubtype::PROBE_REQ),
                    duration: 0,
                    addr1: (*BSSID).into(),
                    addr2: *CLIENT_ADDR,
                    addr3: (*BSSID).into(),
                    seq_ctrl: mac::SequenceControl(10),
                }
                .as_bytes_ref(),
                ht_ctrl: None,
                body: &[0, 5, 1, 2, 3, 4, 5][..],
            },
        )
        .await
        .expect("expected InfraBss::handle_mgmt_frame ok");

        assert_eq!(fake_device_state.lock().wlan_queue.len(), 1);
        assert_eq!(
            &fake_device_state.lock().wlan_queue[0].0[..],
            &[
                // Mgmt header
                0b01010000, 0, // Frame Control
                0, 0, // Duration
                4, 4, 4, 4, 4, 4, // addr1
                2, 2, 2, 2, 2, 2, // addr2
                2, 2, 2, 2, 2, 2, // addr3
                0x10, 0, // Sequence Control
                // Beacon header:
                0, 0, 0, 0, 0, 0, 0, 0, // Timestamp
                100, 0, // Beacon interval
                33, 0, // Capabilities
                // IEs:
                0, 5, 1, 2, 3, 4, 5, // SSID
                1, 1, 248, // Supported rates
                3, 1, 1, // DSSS parameter set
                48, 2, 77, 88, // RSNE
            ][..]
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handle_probe_req_mismatching_ssid() {
        let (fake_device, _) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let mut bss = InfraBss::new(
            &mut ctx,
            Ssid::try_from([1, 2, 3, 4, 5]).unwrap(),
            TimeUnit::DEFAULT_BEACON_INTERVAL,
            2,
            CapabilityInfo(33),
            vec![0b11111000],
            1,
            TwoGhz,
            Some(vec![48, 2, 77, 88]),
        )
        .await
        .expect("expected InfraBss::new ok");

        assert_matches!(
            bss.handle_mgmt_frame(
                &mut ctx,
                mac::MgmtFrame {
                    mgmt_hdr: mac::MgmtHdr {
                        frame_ctrl: mac::FrameControl(0)
                            .with_frame_type(mac::FrameType::MGMT)
                            .with_mgmt_subtype(mac::MgmtSubtype::PROBE_REQ),
                        duration: 0,
                        addr1: (*BSSID).into(),
                        addr2: *CLIENT_ADDR,
                        addr3: (*BSSID).into(),
                        seq_ctrl: mac::SequenceControl(10),
                    }
                    .as_bytes_ref(),
                    ht_ctrl: None,
                    body: &[0, 5, 1, 2, 3, 4, 6][..],
                },
            )
            .await
            .expect_err("expected InfraBss::handle_mgmt_frame error"),
            Rejection::OtherBss
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn make_tim() {
        let (fake_device, _) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let mut bss = make_infra_bss(&mut ctx).await;
        bss.clients.insert(*CLIENT_ADDR, RemoteClient::new(*CLIENT_ADDR));

        let client = bss.clients.get_mut(&CLIENT_ADDR).unwrap();
        client
            .handle_mlme_auth_resp(&mut ctx, fidl_mlme::AuthenticateResultCode::Success)
            .await
            .expect("expected OK");
        client
            .handle_mlme_assoc_resp(
                &mut ctx,
                false,
                fidl_ieee80211::ChannelNumber { number: 1, band: TwoGhz },
                mac::CapabilityInfo(0),
                fidl_mlme::AssociateResultCode::Success,
                1,
                &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10][..],
            )
            .await
            .expect("expected OK");
        client.set_power_state(&mut ctx, mac::PowerState::DOZE).expect("expected doze OK");

        bss.handle_eth_frame(
            &mut ctx,
            EthernetIIHdr {
                da: *CLIENT_ADDR,
                sa: *CLIENT_ADDR2,
                ether_type: BigEndianU16::new(0x1234),
            },
            &[1, 2, 3, 4, 5][..],
            0.into(),
        )
        .expect("expected OK");

        let tim = bss.make_tim();
        let (pvb_offset, pvb_bitmap) = tim.make_partial_virtual_bitmap();
        assert_eq!(pvb_offset, 0);
        assert_eq!(pvb_bitmap, &[0b00000010][..]);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn make_tim_empty() {
        let (fake_device, _) = FakeDevice::new().await;
        let (mut ctx, _) = make_context(fake_device);
        let bss = make_infra_bss(&mut ctx).await;

        let tim = bss.make_tim();
        let (pvb_offset, pvb_bitmap) = tim.make_partial_virtual_bitmap();
        assert_eq!(pvb_offset, 0);
        assert_eq!(pvb_bitmap, &[0b00000000][..]);
    }
}
