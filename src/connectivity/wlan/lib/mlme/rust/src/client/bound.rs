// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::akm_algorithm;
use crate::block_ack::BlockAckTx;
use crate::device::DeviceOps;
use crate::disconnect::LocallyInitiated;
use crate::error::Error;
use anyhow::format_err;
use fdf::{Arena, ArenaBox, ArenaStaticBox};
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_mlme as fidl_mlme;
use fidl_fuchsia_wlan_softmac as fidl_softmac;
use fuchsia_trace as trace;
use ieee80211::{Bssid, MacAddr, MacAddrBytes};
use log::error;
use std::mem;
use std::ptr::NonNull;
use wlan_common::append::Append;
use wlan_common::buffer_writer::BufferWriter;
use wlan_common::ie::rsn::rsne;
use wlan_common::mac::{self, Aid};
use wlan_common::sequence::SequenceManager;
use wlan_common::{data_writer, ie, mgmt_writer, wmm};
use wlan_frame_writer::{append_frame_to, write_frame, write_frame_with_fixed_slice};
use wlan_trace as wtrace;
use zerocopy::SplitByteSlice;

use super::{ChannelState, Client, Context, Scanner};

pub struct BoundClient<'a, D> {
    pub sta: &'a mut Client,
    pub ctx: &'a mut Context<D>,
    pub scanner: &'a mut Scanner,
    pub channel_state: &'a mut ChannelState,
}

impl<'a, D: DeviceOps> akm_algorithm::AkmAction for BoundClient<'a, D> {
    fn send_auth_frame(
        &mut self,
        auth_type: mac::AuthAlgorithmNumber,
        seq_num: u16,
        result_code: mac::StatusCode,
        auth_content: &[u8],
    ) -> Result<(), anyhow::Error> {
        self.send_auth_frame(auth_type, seq_num, result_code, auth_content).map_err(|e| e.into())
    }

    fn forward_sme_sae_rx(
        &mut self,
        seq_num: u16,
        status_code: fidl_ieee80211::StatusCode,
        sae_fields: Vec<u8>,
    ) {
        self.forward_sae_frame_rx(seq_num, status_code, sae_fields)
    }

    fn forward_sae_handshake_ind(&mut self) {
        self.forward_sae_handshake_ind()
    }
}

impl<'a, D: DeviceOps> BoundClient<'a, D> {
    /// Delivers a single MSDU to the STA's underlying device. The MSDU is delivered as an
    /// Ethernet II frame.
    /// Returns Err(_) if writing or delivering the Ethernet II frame failed.
    pub fn deliver_msdu<B: SplitByteSlice>(&mut self, msdu: mac::Msdu<B>) -> Result<(), Error> {
        let mac::Msdu { dst_addr, src_addr, llc_frame } = msdu;

        let mut packet = [0u8; mac::MAX_ETH_FRAME_LEN];
        let (frame_start, frame_end) = write_frame_with_fixed_slice!(&mut packet[..], {
            headers: {
                mac::EthernetIIHdr: &mac::EthernetIIHdr {
                    da: dst_addr,
                    sa: src_addr,
                    ether_type: llc_frame.hdr.protocol_id,
                },
            },
            payload: &llc_frame.body,
        })?;
        self.ctx
            .device
            .deliver_eth_frame(&packet[frame_start..frame_end])
            .map_err(|s| Error::Status(format!("could not deliver Ethernet II frame"), s))
    }

    pub fn send_auth_frame(
        &mut self,
        auth_type: mac::AuthAlgorithmNumber,
        seq_num: u16,
        result_code: mac::StatusCode,
        auth_content: &[u8],
    ) -> Result<(), Error> {
        let buffer = write_frame!({
            headers: {
                mac::MgmtHdr: &mgmt_writer::mgmt_hdr_to_ap(
                    mac::FrameControl(0)
                        .with_frame_type(mac::FrameType::MGMT)
                        .with_mgmt_subtype(mac::MgmtSubtype::AUTH),
                    self.sta.bssid(),
                    self.sta.iface_mac,
                    mac::SequenceControl(0)
                        .with_seq_num(self.ctx.seq_mgr.next_sns1(&self.sta.bssid().into()) as u16)
                ),
                mac::AuthHdr: &mac::AuthHdr {
                    auth_alg_num: auth_type,
                    auth_txn_seq_num: seq_num,
                    status_code: result_code,
                },
            },
            body: auth_content,
        })?;
        self.send_mgmt_or_ctrl_frame(buffer)
            .map_err(|s| Error::Status(format!("error sending open auth frame"), s))
    }

    /// Sends an authentication frame using Open System authentication.
    #[cfg(test)]
    pub fn send_open_auth_frame(&mut self) -> Result<(), Error> {
        self.send_auth_frame(
            mac::AuthAlgorithmNumber::OPEN,
            1,
            fidl_ieee80211::StatusCode::Success.into(),
            &[],
        )
    }

    /// Sends an association request frame based on device capability.
    pub fn send_assoc_req_frame(&mut self) -> Result<(), Error> {
        let ssid = self.sta.ssid().clone();
        let cap = &self.sta.client_capabilities.0;
        let capability_info = cap.capability_info.0;
        let rates: Vec<u8> = cap.rates.iter().map(|r| r.rate()).collect();
        let ht_cap = cap.ht_cap;
        let vht_cap = cap.vht_cap;
        let security_ie = self.sta.connect_req.security_ie.clone();

        let rsne = (!security_ie.is_empty() && security_ie[0] == ie::Id::RSNE.0)
            .then(|| match rsne::from_bytes(&security_ie[..]) {
                Ok((_, x)) => Ok(x),
                Err(e) => Err(format_err!("error parsing rsne {:?} : {:?}", security_ie, e)),
            })
            .transpose()?;
        let buffer = write_frame!({
            headers: {
                mac::MgmtHdr: &mgmt_writer::mgmt_hdr_to_ap(
                    mac::FrameControl(0)
                        .with_frame_type(mac::FrameType::MGMT)
                        .with_mgmt_subtype(mac::MgmtSubtype::ASSOC_REQ),
                    self.sta.bssid(),
                    self.sta.iface_mac,
                    mac::SequenceControl(0)
                        .with_seq_num(self.ctx.seq_mgr.next_sns1(&self.sta.bssid().into()) as u16)
                ),
                mac::AssocReqHdr: &mac::AssocReqHdr {
                    capabilities: mac::CapabilityInfo(capability_info),
                    listen_interval: 0,
                },
            },
            ies: {
                ssid: ssid,
                supported_rates: rates,
                extended_supported_rates: {/* continue rates */},
                rsne?: rsne,
                ht_cap?: ht_cap,
                vht_cap?: vht_cap,
            },
        })?;
        self.send_mgmt_or_ctrl_frame(buffer)
            .map_err(|s| Error::Status(format!("error sending assoc req frame"), s))
    }

    /// Sends a "keep alive" response to the BSS. A keep alive response is a NULL data frame sent as
    /// a response to the AP transmitting NULL data frames to the client.
    // Note: This function was introduced to meet C++ MLME feature parity. However, there needs to
    // be some investigation, whether these "keep alive" frames are the right way of keeping a
    // client associated to legacy APs.
    pub fn send_keep_alive_resp_frame(&mut self) -> Result<(), Error> {
        let buffer = write_frame!({
            headers: {
                mac::FixedDataHdrFields: &data_writer::data_hdr_client_to_ap(
                    mac::FrameControl(0)
                        .with_frame_type(mac::FrameType::DATA)
                        .with_data_subtype(mac::DataSubtype(0).with_null(true)),
                    self.sta.bssid(),
                    self.sta.iface_mac,
                    mac::SequenceControl(0)
                        .with_seq_num(self.ctx.seq_mgr.next_sns1(&self.sta.bssid().into()) as u16)
                ),
            },
        })?;
        self.ctx
            .device
            .send_wlan_frame(buffer, fidl_softmac::WlanTxInfoFlags::empty(), None)
            .map_err(|s| Error::Status(format!("error sending keep alive frame"), s))
    }

    pub fn send_deauth_frame(&mut self, reason_code: mac::ReasonCode) -> Result<(), Error> {
        let buffer = write_frame!({
            headers: {
                mac::MgmtHdr: &mgmt_writer::mgmt_hdr_to_ap(
                    mac::FrameControl(0)
                        .with_frame_type(mac::FrameType::MGMT)
                        .with_mgmt_subtype(mac::MgmtSubtype::DEAUTH),
                    self.sta.bssid(),
                    self.sta.iface_mac,
                    mac::SequenceControl(0)
                        .with_seq_num(self.ctx.seq_mgr.next_sns1(&self.sta.bssid().into()) as u16)
                ),
                mac::DeauthHdr: &mac::DeauthHdr {
                    reason_code,
                },
            },
        })?;
        let result = self
            .send_mgmt_or_ctrl_frame(buffer)
            .map_err(|s| Error::Status(format!("error sending deauthenticate frame"), s));
        // Clear main_channel since there is no "main channel" after deauthenticating
        self.channel_state.bind(&mut self.ctx, &mut self.scanner).clear_main_channel();

        result
    }

    /// Sends the given |payload| as a data frame over the air. If the caller does not pass an |async_id| to
    /// this function, then this function will generate its own |async_id| and end the trace if an error
    /// occurs.
    pub fn send_data_frame(
        &mut self,
        src: MacAddr,
        dst: MacAddr,
        is_protected: bool,
        qos_ctrl: bool,
        ether_type: u16,
        payload: &[u8],
        async_id: Option<trace::Id>,
    ) -> Result<(), Error> {
        let async_id_provided = async_id.is_some();
        let async_id = async_id.unwrap_or_else(|| {
            let async_id = trace::Id::new();
            wtrace::async_begin_wlansoftmac_tx(async_id, "mlme");
            async_id
        });
        wtrace::duration!("BoundClient::send_data_frame");

        let qos_ctrl = if qos_ctrl {
            Some(
                wmm::derive_tid(ether_type, payload)
                    .map_or(mac::QosControl(0), |tid| mac::QosControl(0).with_tid(tid as u16)),
            )
        } else {
            None
        };

        // IEEE Std 802.11-2016, Table 9-26 specifies address field contents and their relation
        // to the addr fields.
        // TODO(https://fxbug.dev/42128470): Support A-MSDU address field contents.

        // We do not currently support RA other than the BSS.
        // TODO(https://fxbug.dev/42122401): Support to_ds = false and alternative RA for TDLS.
        let to_ds = true;
        let from_ds = src != self.sta.iface_mac;
        // Detect when SA != TA, in which case we use addr4.
        let addr1 = self.sta.bssid().into();
        let addr2 = self.sta.iface_mac;
        let addr3 = match (to_ds, from_ds) {
            (false, false) => self.sta.bssid().into(),
            (false, true) => src,
            (true, _) => dst,
        };
        let addr4 = if from_ds && to_ds { Some(src) } else { None };

        let tx_flags = match ether_type {
            mac::ETHER_TYPE_EAPOL => fidl_softmac::WlanTxInfoFlags::FAVOR_RELIABILITY,
            _ => fidl_softmac::WlanTxInfoFlags::empty(),
        };

        // TODO(https://fxbug.dev/353987692): Replace `header_room` with actual amount of space
        // for the header in a network device buffer. The MAX_HEADER_SIZE is arbitrarily extended
        // to emulate the extra room the network device buffer will likely always provide.
        const MAX_HEADER_SIZE: usize = mem::size_of::<mac::FixedDataHdrFields>()
            + mem::size_of::<MacAddr>()
            + mem::size_of::<mac::QosControl>()
            + mem::size_of::<mac::LlcHdr>();
        let header_room = MAX_HEADER_SIZE + 100;
        let arena = Arena::new();
        let mut buffer = arena.insert_default_slice(header_room + payload.len());

        // TODO(https://fxbug.dev/353987692): Remove this clone once we migrate to network device where
        // the buffer can be reused.
        let payload_start = buffer.len() - payload.len();
        buffer[payload_start..].clone_from_slice(&payload[..]);

        let (frame_start, _frame_end) =
            write_frame_with_fixed_slice!(&mut buffer[..payload_start], {
                fill_zeroes: (),
                headers: {
                    mac::FixedDataHdrFields: &mac::FixedDataHdrFields {
                        frame_ctrl: mac::FrameControl(0)
                            .with_frame_type(mac::FrameType::DATA)
                            .with_data_subtype(mac::DataSubtype(0).with_qos(qos_ctrl.is_some()))
                            .with_protected(is_protected)
                            .with_to_ds(to_ds)
                            .with_from_ds(from_ds),
                        duration: 0,
                        addr1,
                        addr2,
                        addr3,
                        seq_ctrl: mac::SequenceControl(0).with_seq_num(
                            match qos_ctrl.as_ref() {
                                None => self.ctx.seq_mgr.next_sns1(&dst),
                                Some(qos_ctrl) => self.ctx.seq_mgr.next_sns2(&dst, qos_ctrl.tid()),
                            } as u16
                        )
                    },
                    mac::Addr4?: addr4,
                    mac::QosControl?: qos_ctrl,
                    mac::LlcHdr: &data_writer::make_snap_llc_hdr(ether_type),
                },
            })
            .map_err(|e| {
                if !async_id_provided {
                    wtrace::async_end_wlansoftmac_tx(async_id, zx::Status::INTERNAL);
                }
                e
            })?;

        // Adjust the start of the slice stored in the ArenaBox.
        //
        // Safety: buffer is a valid pointer to a slice allocated in arena.
        let buffer = unsafe {
            arena.assume_unchecked(NonNull::new_unchecked(
                &mut ArenaBox::into_ptr(buffer).as_mut()[frame_start..],
            ))
        };
        let buffer = arena.make_static(buffer);
        self.ctx.device.send_wlan_frame(buffer, tx_flags, Some(async_id)).map_err(|s| {
            if !async_id_provided {
                wtrace::async_end_wlansoftmac_tx(async_id, s);
            }
            Error::Status(format!("error sending data frame"), s)
        })
    }

    /// Sends an MLME-EAPOL.indication to MLME's SME peer.
    /// Note: MLME-EAPOL.indication is a custom for Fuchsia
    /// and not defined in IEEE 802.11.
    pub fn send_eapol_indication(
        &mut self,
        src_addr: MacAddr,
        dst_addr: MacAddr,
        eapol_frame: &[u8],
    ) -> Result<(), Error> {
        self.ctx
            .device
            .send_mlme_event(fidl_mlme::MlmeEvent::EapolInd {
                ind: fidl_mlme::EapolIndication {
                    src_addr: src_addr.to_array(),
                    dst_addr: dst_addr.to_array(),
                    data: eapol_frame.to_vec(),
                },
            })
            .map_err(|e| e.into())
    }

    /// Sends an EAPoL frame over the air and reports transmission status to SME via an
    /// MLME-EAPOL.confirm message.
    pub fn send_eapol_frame(
        &mut self,
        src: MacAddr,
        dst: MacAddr,
        is_protected: bool,
        eapol_frame: &[u8],
    ) {
        // TODO(https://fxbug.dev/42110270): EAPoL frames can be send in QoS data frames. However, Fuchsia's old C++
        // MLME never sent EAPoL frames in QoS data frames. For feature parity do the same.
        let result = self.send_data_frame(
            src,
            dst,
            is_protected,
            false, /* don't use QoS */
            mac::ETHER_TYPE_EAPOL,
            eapol_frame,
            None,
        );
        let result_code = match result {
            Ok(()) => fidl_mlme::EapolResultCode::Success,
            Err(e) => {
                error!("error sending EAPoL frame: {}", e);
                fidl_mlme::EapolResultCode::TransmissionFailure
            }
        };

        // Report transmission result to SME.
        self.ctx
            .device
            .send_mlme_event(fidl_mlme::MlmeEvent::EapolConf {
                resp: fidl_mlme::EapolConfirm { result_code, dst_addr: dst.to_array() },
            })
            .unwrap_or_else(|e| error!("error sending MLME-EAPOL.confirm message: {}", e));
    }

    pub fn send_ps_poll_frame(&mut self, aid: Aid) -> Result<(), Error> {
        const PS_POLL_ID_MASK: u16 = 0b11000000_00000000;

        let buffer = write_frame!({
            headers: {
                mac::FrameControl: &mac::FrameControl(0)
                    .with_frame_type(mac::FrameType::CTRL)
                    .with_ctrl_subtype(mac::CtrlSubtype::PS_POLL),
                mac::PsPoll: &mac::PsPoll {
                    // IEEE 802.11-2016 9.3.1.5 states the ID in the PS-Poll frame is the
                    // association ID with the 2 MSBs set to 1.
                    masked_aid: aid | PS_POLL_ID_MASK,
                    bssid: self.sta.bssid(),
                    ta: self.sta.iface_mac,
                },
            },
        })?;
        self.send_mgmt_or_ctrl_frame(buffer)
            .map_err(|s| Error::Status(format!("error sending PS-Poll frame"), s))
    }

    pub async fn start_connecting(&mut self) {
        // Safe: |state| is never None and always replaced with Some(..).
        let next_state = self.sta.state.take().unwrap().start_connecting(self).await;
        self.sta.state.replace(next_state);
    }

    pub async fn handle_mlme_request(&mut self, msg: wlan_sme::MlmeRequest) {
        // Safe: |state| is never None and always replaced with Some(..).
        let next_state = self.sta.state.take().unwrap().handle_mlme_request(self, msg).await;
        self.sta.state.replace(next_state);
    }

    pub fn send_connect_conf_failure(&mut self, result_code: fidl_ieee80211::StatusCode) {
        self.sta.connect_timeout.take();
        let bssid = self.sta.connect_req.selected_bss.bssid;
        self.send_connect_conf_failure_with_bssid(bssid, result_code);
    }

    /// Send ConnectConf failure with BSSID specified.
    /// The connect timeout is not cleared as this method may be called with a foreign BSSID.
    pub fn send_connect_conf_failure_with_bssid(
        &mut self,
        bssid: Bssid,
        result_code: fidl_ieee80211::StatusCode,
    ) {
        let connect_conf = fidl_mlme::ConnectConfirm {
            peer_sta_address: bssid.to_array(),
            result_code,
            association_id: 0,
            association_ies: vec![],
        };
        self.ctx
            .device
            .send_mlme_event(fidl_mlme::MlmeEvent::ConnectConf { resp: connect_conf })
            .unwrap_or_else(|e| error!("error sending MLME-CONNECT.confirm: {}", e));
    }

    pub fn send_connect_conf_success<B: SplitByteSlice>(
        &mut self,
        association_id: mac::Aid,
        association_ies: B,
    ) {
        self.sta.connect_timeout.take();
        let connect_conf = fidl_mlme::ConnectConfirm {
            peer_sta_address: self.sta.connect_req.selected_bss.bssid.to_array(),
            result_code: fidl_ieee80211::StatusCode::Success,
            association_id,
            association_ies: association_ies.to_vec(),
        };
        self.ctx
            .device
            .send_mlme_event(fidl_mlme::MlmeEvent::ConnectConf { resp: connect_conf })
            .unwrap_or_else(|e| error!("error sending MLME-CONNECT.confirm: {}", e));
    }

    /// Sends an MLME-DEAUTHENTICATE.indication message to the joined BSS.
    pub fn send_deauthenticate_ind(
        &mut self,
        reason_code: fidl_ieee80211::ReasonCode,
        locally_initiated: LocallyInitiated,
    ) {
        // Clear main_channel since there is no "main channel" after deauthenticating
        self.channel_state.bind(&mut self.ctx, &mut self.scanner).clear_main_channel();

        self.ctx
            .device
            .send_mlme_event(fidl_mlme::MlmeEvent::DeauthenticateInd {
                ind: fidl_mlme::DeauthenticateIndication {
                    peer_sta_address: self.sta.bssid().to_array(),
                    reason_code,
                    locally_initiated: locally_initiated.0,
                },
            })
            .unwrap_or_else(|e| error!("error sending MLME-DEAUTHENTICATE.indication: {}", e));
    }

    /// Sends an MLME-DISASSOCIATE.indication message to the joined BSS.
    pub fn send_disassoc_ind(
        &mut self,
        reason_code: fidl_ieee80211::ReasonCode,
        locally_initiated: LocallyInitiated,
    ) {
        self.ctx
            .device
            .send_mlme_event(fidl_mlme::MlmeEvent::DisassociateInd {
                ind: fidl_mlme::DisassociateIndication {
                    peer_sta_address: self.sta.bssid().to_array(),
                    reason_code,
                    locally_initiated: locally_initiated.0,
                },
            })
            .unwrap_or_else(|e| error!("error sending MLME-DISASSOCIATE.indication: {}", e));
    }

    pub async fn clear_association(&mut self) -> Result<(), zx::Status> {
        self.ctx
            .device
            .clear_association(&fidl_softmac::WlanSoftmacBaseClearAssociationRequest {
                peer_addr: Some(self.sta.bssid().to_array()),
                ..Default::default()
            })
            .await
    }

    /// Sends an sae frame rx message to the SME.
    pub fn forward_sae_frame_rx(
        &mut self,
        seq_num: u16,
        status_code: fidl_ieee80211::StatusCode,
        sae_fields: Vec<u8>,
    ) {
        self.ctx
            .device
            .send_mlme_event(fidl_mlme::MlmeEvent::OnSaeFrameRx {
                frame: fidl_mlme::SaeFrame {
                    peer_sta_address: self.sta.bssid().to_array(),
                    seq_num,
                    status_code,
                    sae_fields,
                },
            })
            .unwrap_or_else(|e| error!("error sending OnSaeFrameRx: {}", e));
    }

    pub fn forward_sae_handshake_ind(&mut self) {
        self.ctx
            .device
            .send_mlme_event(fidl_mlme::MlmeEvent::OnSaeHandshakeInd {
                ind: fidl_mlme::SaeHandshakeIndication {
                    peer_sta_address: self.sta.bssid().to_array(),
                },
            })
            .unwrap_or_else(|e| error!("error sending OnSaeHandshakeInd: {}", e));
    }

    pub async fn handle_mac_frame_rx(
        &mut self,
        bytes: &[u8],
        rx_info: fidl_softmac::WlanRxInfo,
        async_id: trace::Id,
    ) {
        wtrace::duration!("BoundClient::handle_mac_frame_rx");
        // Safe: |state| is never None and always replaced with Some(..).
        self.sta.state =
            Some(self.sta.state.take().unwrap().on_mac_frame(self, bytes, rx_info, async_id).await);
    }

    pub fn handle_eth_frame_tx(&mut self, frame: &[u8], async_id: trace::Id) -> Result<(), Error> {
        wtrace::duration!("BoundClient::handle_eth_frame_tx");
        // Safe: |state| is never None and always replaced with Some(..).
        let state = self.sta.state.take().unwrap();
        let result = state.on_eth_frame(self, frame, async_id);
        self.sta.state.replace(state);
        result
    }

    pub fn send_mgmt_or_ctrl_frame(
        &mut self,
        buffer: ArenaStaticBox<[u8]>,
    ) -> Result<(), zx::Status> {
        self.ctx.device.send_wlan_frame(buffer, fidl_softmac::WlanTxInfoFlags::empty(), None)
    }
}

impl<'a, D: DeviceOps> BlockAckTx for BoundClient<'a, D> {
    /// Sends a BlockAck frame to the associated AP.
    ///
    /// BlockAck frames are described by 802.11-2016, section 9.6.5.2, 9.6.5.3, and 9.6.5.4.
    fn send_block_ack_frame(&mut self, n: usize, body: &[u8]) -> Result<(), Error> {
        let arena = Arena::new();
        let buffer = arena.insert_default_slice::<u8>(n);
        let mut buffer = arena.make_static(buffer);
        let mut writer = BufferWriter::new(&mut buffer[..]);
        write_block_ack_hdr(
            &mut writer,
            self.sta.bssid(),
            self.sta.iface_mac,
            &mut self.ctx.seq_mgr,
        )
        .and_then(|_| writer.append_bytes(body).map_err(Into::into))?;
        self.send_mgmt_or_ctrl_frame(buffer)
            .map_err(|status| Error::Status(format!("error sending BlockAck frame"), status))
    }
}

/// Writes the header of the management frame for BlockAck frames to the given buffer.
///
/// The address may be that of the originator or recipient. The frame formats are described by IEEE
/// Std 802.11-2016, 9.6.5.
fn write_block_ack_hdr<B: Append>(
    buffer: &mut B,
    bssid: Bssid,
    addr: MacAddr,
    seq_mgr: &mut SequenceManager,
) -> Result<(), Error> {
    // The management header differs for APs and clients. The frame control and management header
    // are constructed here, but AP and client STAs share the code that constructs the body. See
    // the `block_ack` module.
    Ok(append_frame_to!(
        buffer,
        {
            headers: {
                mac::MgmtHdr: &mgmt_writer::mgmt_hdr_to_ap(
                    mac::FrameControl(0)
                        .with_frame_type(mac::FrameType::MGMT)
                        .with_mgmt_subtype(mac::MgmtSubtype::ACTION),
                    bssid,
                    addr,
                    mac::SequenceControl(0)
                        .with_seq_num(seq_mgr.next_sns1(&bssid.into()) as u16),
                ),
            },
        }
    )
    .map(|_buffer| {})?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_ack::{self, ADDBA_REQ_FRAME_LEN, ADDBA_RESP_FRAME_LEN, BlockAckTx};
    use crate::client::ParsedConnectRequest;
    use crate::client::test_utils::*;
    use ieee80211::Ssid;
    use wlan_common::buffer_writer::BufferWriter;
    use wlan_common::capabilities::{ClientCapabilities, StaCapabilities};
    use wlan_common::fake_bss_description;
    use wlan_common::test_utils::fake_frames::*;

    #[fuchsia::test(allow_stalls = false)]
    async fn client_send_open_auth_frame() {
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;
        me.make_client_station();
        let mut client = me.get_bound_client().expect("client should be present");
        client.send_open_auth_frame().expect("error delivering WLAN frame");
        assert_eq!(m.fake_device_state.lock().wlan_queue.len(), 1);
        #[rustfmt::skip]
        assert_eq!(&m.fake_device_state.lock().wlan_queue[0].0[..], &[
            // Mgmt header:
            0b1011_00_00, 0b00000000, // FC
            0, 0, // Duration
            6, 6, 6, 6, 6, 6, // addr1
            7, 7, 7, 7, 7, 7, // addr2
            6, 6, 6, 6, 6, 6, // addr3
            0x10, 0, // Sequence Control
            // Auth header:
            0, 0, // auth algorithm
            1, 0, // auth txn seq num
            0, 0, // status code
        ][..]);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn client_send_assoc_req_frame() {
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;
        let connect_req = ParsedConnectRequest {
            selected_bss: fake_bss_description!(Wpa2,
                ssid: Ssid::try_from([11, 22, 33, 44]).unwrap(),
                bssid: BSSID.to_array(),
            ),
            connect_failure_timeout: 100,
            auth_type: fidl_mlme::AuthenticationTypes::OpenSystem,
            security_ie: RSNE.to_vec(),
        };
        let client_capabilities = ClientCapabilities(StaCapabilities {
            capability_info: mac::CapabilityInfo(0x1234),
            rates: vec![8u8, 7, 6, 5, 4, 3, 2, 1, 0].into_iter().map(ie::SupportedRate).collect(),
            ht_cap: ie::parse_ht_capabilities(&(0..26).collect::<Vec<u8>>()[..]).map(|h| *h).ok(),
            vht_cap: ie::parse_vht_capabilities(&(100..112).collect::<Vec<u8>>()[..])
                .map(|v| *v)
                .ok(),
        });
        me.sta.replace(Client::new(connect_req, *IFACE_MAC, client_capabilities));
        let mut client = me.get_bound_client().expect("client should be present");
        client.send_assoc_req_frame().expect("error delivering WLAN frame");
        assert_eq!(m.fake_device_state.lock().wlan_queue.len(), 1);
        assert_eq!(
            &m.fake_device_state.lock().wlan_queue[0].0[..],
            &[
                // Mgmt header:
                0, 0, // FC
                0, 0, // Duration
                6, 6, 6, 6, 6, 6, // addr1
                7, 7, 7, 7, 7, 7, // addr2
                6, 6, 6, 6, 6, 6, // addr3
                0x10, 0, // Sequence Control
                // Association Request header:
                0x34, 0x12, // capability info
                0, 0, // listen interval
                // IEs
                0, 4, // SSID id and length
                11, 22, 33, 44, // SSID
                1, 8, // supp rates id and length
                8, 7, 6, 5, 4, 3, 2, 1, // supp rates
                50, 1, // ext supp rates and length
                0, // ext supp rates
                0x30, 0x14, // RSNE ID and len
                1, 0, // RSNE version
                0x00, 0x0f, 0xac, 0x04, // RSNE group data cipher suite
                0x01, 0x00, // RSNE pairwise cipher suite count
                0x00, 0x0f, 0xac, 0x04, // RSNE pairwise cipher suite list
                0x01, 0x00, // RSNE akm suite count
                0x00, 0x0f, 0xac, 0x02, // RSNE akm suite list
                0xa8, 0x04, // RSNE rsn capabilities
                45, 26, // HT Cap id and length
                0, 1, 2, 3, 4, 5, 6, 7, // HT Cap \
                8, 9, 10, 11, 12, 13, 14, 15, // HT Cap \
                16, 17, 18, 19, 20, 21, 22, 23, // HT Cap \
                24, 25, // HT Cap (26 bytes)
                191, 12, // VHT Cap id and length
                100, 101, 102, 103, 104, 105, 106, 107, // VHT Cap \
                108, 109, 110, 111, // VHT Cap (12 bytes)
            ][..]
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn client_send_keep_alive_resp_frame() {
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;
        me.make_client_station();
        let mut client = me.get_bound_client().expect("client should be present");
        client.send_keep_alive_resp_frame().expect("error delivering WLAN frame");
        assert_eq!(m.fake_device_state.lock().wlan_queue.len(), 1);
        #[rustfmt::skip]
        assert_eq!(&m.fake_device_state.lock().wlan_queue[0].0[..], &[
            // Data header:
            0b0100_10_00, 0b0000000_1, // FC
            0, 0, // Duration
            6, 6, 6, 6, 6, 6, // addr1
            7, 7, 7, 7, 7, 7, // addr2
            6, 6, 6, 6, 6, 6, // addr3
            0x10, 0, // Sequence Control
        ][..]);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn client_send_data_frame() {
        let payload = vec![5; 8];
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;
        me.make_client_station();
        let mut client = me.get_bound_client().expect("client should be present");
        client
            .send_data_frame(*IFACE_MAC, [4; 6].into(), false, false, 0x1234, &payload[..], None)
            .expect("error delivering WLAN frame");
        assert_eq!(m.fake_device_state.lock().wlan_queue.len(), 1);
        #[rustfmt::skip]
        assert_eq!(&m.fake_device_state.lock().wlan_queue[0].0[..], &[
            // Data header:
            0b0000_10_00, 0b0000000_1, // FC
            0, 0, // Duration
            6, 6, 6, 6, 6, 6, // addr1
            7, 7, 7, 7, 7, 7, // addr2
            4, 4, 4, 4, 4, 4, // addr3
            0x10, 0, // Sequence Control
            // LLC header:
            0xAA, 0xAA, 0x03, // DSAP, SSAP, Control
            0, 0, 0, // OUI
            0x12, 0x34, // Protocol ID
            // Payload
            5, 5, 5, 5, 5, 5, 5, 5,
        ][..]);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn client_send_data_frame_ipv4_qos() {
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;
        let mut client = make_client_station();
        client
            .bind(&mut me.ctx, &mut me.scanner, &mut me.channel_state)
            .send_data_frame(
                *IFACE_MAC,
                [4; 6].into(),
                false,
                true,
                0x0800,              // IPv4
                &[1, 0xB0, 3, 4, 5], // DSCP = 0b101100 (i.e. VOICE-ADMIT)
                None,
            )
            .expect("error delivering WLAN frame");
        assert_eq!(m.fake_device_state.lock().wlan_queue.len(), 1);
        #[rustfmt::skip]
        assert_eq!(&m.fake_device_state.lock().wlan_queue[0].0[..], &[
            // Data header:
            0b1000_10_00, 0b0000000_1, // FC
            0, 0, // Duration
            6, 6, 6, 6, 6, 6, // addr1
            7, 7, 7, 7, 7, 7, // addr2
            4, 4, 4, 4, 4, 4, // addr3
            0x10, 0, // Sequence Control
            0x06, 0, // QoS Control - TID = 6
            // LLC header:
            0xAA, 0xAA, 0x03, // DSAP, SSAP, Control
            0, 0, 0, // OUI
            0x08, 0x00, // Protocol ID
            // Payload
            1, 0xB0, 3, 4, 5,
        ][..]);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn client_send_data_frame_ipv6_qos() {
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;
        let mut client = make_client_station();
        client
            .bind(&mut me.ctx, &mut me.scanner, &mut me.channel_state)
            .send_data_frame(
                *IFACE_MAC,
                [4; 6].into(),
                false,
                true,
                0x86DD,                         // IPv6
                &[0b0101, 0b10000000, 3, 4, 5], // DSCP = 0b010110 (i.e. AF23)
                None,
            )
            .expect("error delivering WLAN frame");
        assert_eq!(m.fake_device_state.lock().wlan_queue.len(), 1);
        #[rustfmt::skip]
        assert_eq!(&m.fake_device_state.lock().wlan_queue[0].0[..], &[
            // Data header:
            0b1000_10_00, 0b0000000_1, // FC
            0, 0, // Duration
            6, 6, 6, 6, 6, 6, // addr1
            7, 7, 7, 7, 7, 7, // addr2
            4, 4, 4, 4, 4, 4, // addr3
            0x10, 0, // Sequence Control
            0x03, 0, // QoS Control - TID = 3
            // LLC header:
            0xAA, 0xAA, 0x03, // DSAP, SSAP, Control
            0, 0, 0, // OUI
            0x86, 0xDD, // Protocol ID
            // Payload
            0b0101, 0b10000000, 3, 4, 5,
        ][..]);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn client_send_data_frame_from_ds() {
        let payload = vec![5; 8];
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;
        me.make_client_station();
        let mut client = me.get_bound_client().expect("client should be present");
        client
            .send_data_frame([3; 6].into(), [4; 6].into(), false, false, 0x1234, &payload[..], None)
            .expect("error delivering WLAN frame");
        assert_eq!(m.fake_device_state.lock().wlan_queue.len(), 1);
        #[rustfmt::skip]
        assert_eq!(&m.fake_device_state.lock().wlan_queue[0].0[..], &[
            // Data header:
            0b0000_10_00, 0b000000_11, // FC (ToDS=1, FromDS=1)
            0, 0, // Duration
            6, 6, 6, 6, 6, 6, // addr1
            7, 7, 7, 7, 7, 7, // addr2 = IFACE_MAC
            4, 4, 4, 4, 4, 4, // addr3
            0x10, 0, // Sequence Control
            3, 3, 3, 3, 3, 3, // addr4
            // LLC header:
            0xAA, 0xAA, 0x03, // DSAP, SSAP, Control
            0, 0, 0, // OUI
            0x12, 0x34, // Protocol ID
            // Payload
            5, 5, 5, 5, 5, 5, 5, 5,
        ][..]);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn client_send_deauthentication_notification() {
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;
        me.make_client_station();
        let mut client = me.get_bound_client().expect("client should be present");

        client
            .send_deauth_frame(fidl_ieee80211::ReasonCode::ApInitiated.into())
            .expect("error delivering WLAN frame");
        assert_eq!(m.fake_device_state.lock().wlan_queue.len(), 1);
        #[rustfmt::skip]
        assert_eq!(&m.fake_device_state.lock().wlan_queue[0].0[..], &[
            // Mgmt header:
            0b1100_00_00, 0b00000000, // FC
            0, 0, // Duration
            6, 6, 6, 6, 6, 6, // addr1
            7, 7, 7, 7, 7, 7, // addr2
            6, 6, 6, 6, 6, 6, // addr3
            0x10, 0, // Sequence Control
            47, 0, // reason code
        ][..]);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn respond_to_keep_alive_request() {
        #[rustfmt::skip]
        let data_frame = vec![
            // Data header:
            0b0100_10_00, 0b000000_1_0, // FC
            0, 0, // Duration
            7, 7, 7, 7, 7, 7, // addr1
            6, 6, 6, 6, 6, 6, // addr2
            42, 42, 42, 42, 42, 42, // addr3
            0x10, 0, // Sequence Control
        ];
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;
        me.make_client_station();
        let mut client = me.get_bound_client().expect("client should be present");
        client.move_to_associated_state();

        client.handle_mac_frame_rx(&data_frame[..], mock_rx_info(&client), 0.into()).await;

        assert_eq!(m.fake_device_state.lock().wlan_queue.len(), 1);
        #[rustfmt::skip]
        assert_eq!(&m.fake_device_state.lock().wlan_queue[0].0[..], &[
            // Data header:
            0b0100_10_00, 0b0000000_1, // FC
            0, 0, // Duration
            6, 6, 6, 6, 6, 6, // addr1
            7, 7, 7, 7, 7, 7, // addr2
            6, 6, 6, 6, 6, 6, // addr3
            0x10, 0, // Sequence Control
        ][..]);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn data_frame_to_ethernet_single_llc() {
        let mut data_frame = make_data_frame_single_llc(None, None);
        data_frame[1] = 0b00000010; // from_ds = 1, to_ds = 0 when AP sends to client (us)
        data_frame[4..10].copy_from_slice(IFACE_MAC.as_array()); // addr1 - receiver - client (us)
        data_frame[10..16].copy_from_slice(BSSID.as_array()); // addr2 - bssid

        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;
        me.make_client_station();
        let mut client = me.get_bound_client().expect("client should be present");
        client.move_to_associated_state();

        client.handle_mac_frame_rx(&data_frame[..], mock_rx_info(&client), 0.into()).await;

        assert_eq!(m.fake_device_state.lock().eth_queue.len(), 1);
        #[rustfmt::skip]
        assert_eq!(m.fake_device_state.lock().eth_queue[0], [
            7, 7, 7, 7, 7, 7, // dst_addr
            5, 5, 5, 5, 5, 5, // src_addr
            9, 10, // ether_type
            11, 11, 11, // payload
        ]);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn data_frame_to_ethernet_amsdu() {
        let mut data_frame = make_data_frame_amsdu();
        data_frame[1] = 0b00000010; // from_ds = 1, to_ds = 0 when AP sends to client (us)
        data_frame[4..10].copy_from_slice(IFACE_MAC.as_array()); // addr1 - receiver - client (us)
        data_frame[10..16].copy_from_slice(BSSID.as_array()); // addr2 - bssid

        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;
        me.make_client_station();
        let mut client = me.get_bound_client().expect("client should be present");
        client.move_to_associated_state();

        client.handle_mac_frame_rx(&data_frame[..], mock_rx_info(&client), 0.into()).await;

        let queue = &m.fake_device_state.lock().eth_queue;
        assert_eq!(queue.len(), 2);
        #[rustfmt::skip]
        let mut expected_first_eth_frame = vec![
            0x78, 0x8a, 0x20, 0x0d, 0x67, 0x03, // dst_addr
            0xb4, 0xf7, 0xa1, 0xbe, 0xb9, 0xab, // src_addr
            0x08, 0x00, // ether_type
        ];
        expected_first_eth_frame.extend_from_slice(MSDU_1_PAYLOAD);
        assert_eq!(queue[0], &expected_first_eth_frame[..]);
        #[rustfmt::skip]
        let mut expected_second_eth_frame = vec![
            0x78, 0x8a, 0x20, 0x0d, 0x67, 0x04, // dst_addr
            0xb4, 0xf7, 0xa1, 0xbe, 0xb9, 0xac, // src_addr
            0x08, 0x01, // ether_type
        ];
        expected_second_eth_frame.extend_from_slice(MSDU_2_PAYLOAD);
        assert_eq!(queue[1], &expected_second_eth_frame[..]);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn data_frame_to_ethernet_amsdu_padding_too_short() {
        let mut data_frame = make_data_frame_amsdu_padding_too_short();
        data_frame[1] = 0b00000010; // from_ds = 1, to_ds = 0 when AP sends to client (us)
        data_frame[4..10].copy_from_slice(IFACE_MAC.as_array()); // addr1 - receiver - client (us)
        data_frame[10..16].copy_from_slice(BSSID.as_array()); // addr2 - bssid

        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;
        me.make_client_station();
        let mut client = me.get_bound_client().expect("client should be present");
        client.move_to_associated_state();

        client.handle_mac_frame_rx(&data_frame[..], mock_rx_info(&client), 0.into()).await;

        let queue = &m.fake_device_state.lock().eth_queue;
        assert_eq!(queue.len(), 1);
        #[rustfmt::skip]
            let mut expected_first_eth_frame = vec![
            0x78, 0x8a, 0x20, 0x0d, 0x67, 0x03, // dst_addr
            0xb4, 0xf7, 0xa1, 0xbe, 0xb9, 0xab, // src_addr
            0x08, 0x00, // ether_type
        ];
        expected_first_eth_frame.extend_from_slice(MSDU_1_PAYLOAD);
        assert_eq!(queue[0], &expected_first_eth_frame[..]);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn data_frame_controlled_port_closed() {
        let mut data_frame = make_data_frame_single_llc(None, None);
        data_frame[1] = 0b00000010; // from_ds = 1, to_ds = 0 when AP sends to client (us)
        data_frame[4..10].copy_from_slice(IFACE_MAC.as_array()); // addr1 - receiver - client (us)
        data_frame[10..16].copy_from_slice(BSSID.as_array()); // addr2 - bssid

        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;
        me.make_client_station_protected();
        let mut client = me.get_bound_client().expect("client should be present");
        client.move_to_associated_state();
        client.close_controlled_port().await;

        client.handle_mac_frame_rx(&data_frame[..], mock_rx_info(&client), 0.into()).await;

        // Verify frame was not sent to netstack.
        assert_eq!(m.fake_device_state.lock().eth_queue.len(), 0);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn eapol_frame_controlled_port_closed() {
        let (src_addr, dst_addr, mut eapol_frame) = make_eapol_frame(*IFACE_MAC);
        eapol_frame[1] = 0b00000010; // from_ds = 1, to_ds = 0 when AP sends to client (us)
        eapol_frame[4..10].copy_from_slice(IFACE_MAC.as_array()); // addr1 - receiver - client (us)
        eapol_frame[10..16].copy_from_slice(BSSID.as_array()); // addr2 - bssid

        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;
        me.make_client_station_protected();
        let mut client = me.get_bound_client().expect("client should be present");
        client.move_to_associated_state();
        client.close_controlled_port().await;

        client.handle_mac_frame_rx(&eapol_frame[..], mock_rx_info(&client), 0.into()).await;

        // Verify EAPoL frame was not sent to netstack.
        assert_eq!(m.fake_device_state.lock().eth_queue.len(), 0);

        // Verify EAPoL frame was sent to SME.
        let eapol_ind = m
            .fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::EapolIndication>()
            .expect("error reading EAPOL.indication");
        assert_eq!(
            eapol_ind,
            fidl_mlme::EapolIndication {
                src_addr: src_addr.to_array(),
                dst_addr: dst_addr.to_array(),
                data: EAPOL_PDU.to_vec()
            }
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn eapol_frame_is_controlled_port_open() {
        let (src_addr, dst_addr, mut eapol_frame) = make_eapol_frame(*IFACE_MAC);
        eapol_frame[1] = 0b00000010; // from_ds = 1, to_ds = 0 when AP sends to client (us)
        eapol_frame[4..10].copy_from_slice(IFACE_MAC.as_array()); // addr1 - receiver - client (us)
        eapol_frame[10..16].copy_from_slice(BSSID.as_array()); // addr2 - bssid

        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;
        me.make_client_station();
        let mut client = me.get_bound_client().expect("client should be present");
        client.move_to_associated_state();

        client.handle_mac_frame_rx(&eapol_frame[..], mock_rx_info(&client), 0.into()).await;

        // Verify EAPoL frame was not sent to netstack.
        assert_eq!(m.fake_device_state.lock().eth_queue.len(), 0);

        // Verify EAPoL frame was sent to SME.
        let eapol_ind = m
            .fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::EapolIndication>()
            .expect("error reading EAPOL.indication");
        assert_eq!(
            eapol_ind,
            fidl_mlme::EapolIndication {
                src_addr: src_addr.to_array(),
                dst_addr: dst_addr.to_array(),
                data: EAPOL_PDU.to_vec()
            }
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn send_eapol_ind_success() {
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;
        me.make_client_station();
        let mut client = me.get_bound_client().expect("client should be present");
        client
            .send_eapol_indication([1; 6].into(), [2; 6].into(), &[5; 200])
            .expect("expected EAPOL.indication to be sent");
        let eapol_ind = m
            .fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::EapolIndication>()
            .expect("error reading EAPOL.indication");
        assert_eq!(
            eapol_ind,
            fidl_mlme::EapolIndication {
                src_addr: [1; 6].into(),
                dst_addr: [2; 6].into(),
                data: vec![5; 200]
            }
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn send_eapol_frame_success() {
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;
        me.make_client_station();
        let mut client = me.get_bound_client().expect("client should be present");
        client.send_eapol_frame(*IFACE_MAC, (*BSSID).into(), false, &[5; 8]);

        // Verify EAPOL.confirm message was sent to SME.
        let eapol_confirm = m
            .fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::EapolConfirm>()
            .expect("error reading EAPOL.confirm");
        assert_eq!(
            eapol_confirm,
            fidl_mlme::EapolConfirm {
                result_code: fidl_mlme::EapolResultCode::Success,
                dst_addr: BSSID.to_array(),
            }
        );

        // Verify EAPoL frame was sent over the air.
        #[rustfmt::skip]
        assert_eq!(&m.fake_device_state.lock().wlan_queue[0].0[..], &[
            // Data header:
            0b0000_10_00, 0b0000000_1, // FC
            0, 0, // Duration
            6, 6, 6, 6, 6, 6, // addr1
            7, 7, 7, 7, 7, 7, // addr2
            6, 6, 6, 6, 6, 6, // addr3
            0x10, 0, // Sequence Control
            // LLC header:
            0xaa, 0xaa, 0x03, // dsap ssap ctrl
            0x00, 0x00, 0x00, // oui
            0x88, 0x8E, // protocol id (EAPOL)
            // EAPoL PDU:
            5, 5, 5, 5, 5, 5, 5, 5,
        ][..]);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn send_eapol_frame_failure() {
        let mut m = MockObjects::new().await;
        m.fake_device_state.lock().config.send_wlan_frame_fails = true;
        let mut me = m.make_mlme().await;
        me.make_client_station();
        let mut client = me.get_bound_client().expect("client should be present");
        client.send_eapol_frame([1; 6].into(), [2; 6].into(), false, &[5; 200]);

        // Verify EAPOL.confirm message was sent to SME.
        let eapol_confirm = m
            .fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::EapolConfirm>()
            .expect("error reading EAPOL.confirm");
        assert_eq!(
            eapol_confirm,
            fidl_mlme::EapolConfirm {
                result_code: fidl_mlme::EapolResultCode::TransmissionFailure,
                dst_addr: [2; 6].into(),
            }
        );

        // Verify EAPoL frame was not sent over the air.
        assert!(m.fake_device_state.lock().wlan_queue.is_empty());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn send_keys() {
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;
        me.make_client_station_protected();
        let mut client = me.get_bound_client().expect("client should be present");
        client.move_to_associated_state();

        assert!(m.fake_device_state.lock().keys.is_empty());
        client.handle_mlme_request(crate::test_utils::fake_set_keys_req((*BSSID).into())).await;
        assert_eq!(m.fake_device_state.lock().keys.len(), 1);

        let sent_key = crate::test_utils::fake_key((*BSSID).into());
        let received_key = &m.fake_device_state.lock().keys[0];
        assert_eq!(received_key.key, Some(sent_key.key));
        assert_eq!(received_key.key_idx, Some(sent_key.key_id as u8));
        assert_eq!(received_key.key_type, Some(fidl_ieee80211::KeyType::Pairwise));
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn send_addba_req_frame() {
        let mut mock = MockObjects::new().await;
        let mut mlme = mock.make_mlme().await;
        mlme.make_client_station();
        let mut client = mlme.get_bound_client().expect("client should be present");

        let mut body = [0u8; 16];
        let mut writer = BufferWriter::new(&mut body[..]);
        block_ack::write_addba_req_body(&mut writer, 1).expect("failed writing addba frame");
        client
            .send_block_ack_frame(ADDBA_REQ_FRAME_LEN, writer.into_written())
            .expect("failed sending addba frame");
        assert_eq!(
            &mock.fake_device_state.lock().wlan_queue[0].0[..],
            &[
                // Mgmt header 1101 for action frame
                0b11010000, 0b00000000, // frame control
                0, 0, // duration
                6, 6, 6, 6, 6, 6, // addr1
                7, 7, 7, 7, 7, 7, // addr2
                6, 6, 6, 6, 6, 6, // addr3
                0x10, 0, // sequence control
                // Action frame header (Also part of ADDBA request frame)
                0x03, // Action Category: block ack (0x03)
                0x00, // block ack action: ADDBA request (0x00)
                1,    // block ack dialog token
                0b00000011, 0b00010000, // block ack parameters (u16)
                0, 0, // block ack timeout (u16) (0: disabled)
                0b00010000, 0, // block ack starting sequence number: fragment 0, sequence 1
            ][..]
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn send_addba_resp_frame() {
        let mut mock = MockObjects::new().await;
        let mut mlme = mock.make_mlme().await;
        mlme.make_client_station();
        let mut client = mlme.get_bound_client().expect("client should be present");

        let mut body = [0u8; 16];
        let mut writer = BufferWriter::new(&mut body[..]);
        block_ack::write_addba_resp_body(&mut writer, 1).expect("failed writing addba frame");
        client
            .send_block_ack_frame(ADDBA_RESP_FRAME_LEN, writer.into_written())
            .expect("failed sending addba frame");
        assert_eq!(
            &mock.fake_device_state.lock().wlan_queue[0].0[..],
            &[
                // Mgmt header 1101 for action frame
                0b11010000, 0b00000000, // frame control
                0, 0, // duration
                6, 6, 6, 6, 6, 6, // addr1
                7, 7, 7, 7, 7, 7, // addr2
                6, 6, 6, 6, 6, 6, // addr3
                0x10, 0, // sequence control
                // Action frame header (Also part of ADDBA response frame)
                0x03, // Action Category: block ack (0x03)
                0x01, // block ack action: ADDBA response (0x01)
                1,    // block ack dialog token
                0, 0, // status
                0b00000011, 0b00010000, // block ack parameters (u16)
                0, 0, // block ack timeout (u16) (0: disabled)
            ][..]
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn client_send_successful_connect_conf() {
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;
        me.make_client_station();
        let mut client = me.get_bound_client().expect("client should be present");

        client.send_connect_conf_success(42, &[0, 5, 3, 4, 5, 6, 7][..]);
        let connect_conf = m
            .fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::ConnectConfirm>()
            .expect("error reading Connect.confirm");
        assert_eq!(
            connect_conf,
            fidl_mlme::ConnectConfirm {
                peer_sta_address: BSSID.to_array(),
                result_code: fidl_ieee80211::StatusCode::Success,
                association_id: 42,
                association_ies: vec![0, 5, 3, 4, 5, 6, 7],
            }
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn client_send_failed_connect_conf() {
        let mut m = MockObjects::new().await;
        let mut me = m.make_mlme().await;
        me.make_client_station();
        let mut client = me.get_bound_client().expect("client should be present");
        client.send_connect_conf_failure(fidl_ieee80211::StatusCode::DeniedNoMoreStas);
        let connect_conf = m
            .fake_device_state
            .lock()
            .next_mlme_msg::<fidl_mlme::ConnectConfirm>()
            .expect("error reading Connect.confirm");
        assert_eq!(
            connect_conf,
            fidl_mlme::ConnectConfirm {
                peer_sta_address: BSSID.to_array(),
                result_code: fidl_ieee80211::StatusCode::DeniedNoMoreStas,
                association_id: 0,
                association_ies: vec![],
            }
        );
    }
}
