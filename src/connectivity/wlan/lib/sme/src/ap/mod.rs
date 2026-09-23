// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod aid;
mod authenticator;
mod event;
mod remote_client;
#[cfg(test)]
pub mod test_utils;

use event::*;
use remote_client::*;

use crate::responder::Responder;
use crate::{MlmeRequest, MlmeSink, mlme_event_name};
use fidl_fuchsia_wlan_common as fidl_common;
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_internal as fidl_internal;
use fidl_fuchsia_wlan_mlme::{self as fidl_mlme, DeviceInfo, MlmeEvent};
use fidl_fuchsia_wlan_sme as fidl_sme;
use futures::channel::{mpsc, oneshot};
use ieee80211::{MacAddr, MacAddrBytes, Ssid};
use log::{debug, error, info, warn};
use std::collections::HashMap;
use wlan_common::capabilities::get_band_cap_for_channel;
use wlan_common::channel::{Bandwidth, Channel};
use wlan_common::ie::rsn::rsne::{RsnCapabilities, Rsne};
use wlan_common::ie::{ChanWidthSet, SupportedRate, parse_ht_capabilities};
use wlan_common::timer::{self, EventHandle, Timer};
use wlan_common::{RadioConfig, mac};
use wlan_rsn::psk;

const DEFAULT_BEACON_PERIOD: u16 = 100;
const DEFAULT_DTIM_PERIOD: u8 = 2;

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub ssid: Ssid,
    pub password: Vec<u8>,
    pub radio_cfg: RadioConfig,
}

// OpRadioConfig keeps admitted configuration and operation state
#[derive(Clone, Debug, PartialEq)]
pub struct OpRadioConfig {
    phy: fidl_ieee80211::WlanPhyType,
    channel: Channel,
    basic_rates: Vec<u8>,
}

struct StartingState {
    ctx: Context,
    ssid: Ssid,
    rsn_cfg: Option<RsnCfg>,
    _capabilities: mac::CapabilityInfo,
    _rates: Vec<SupportedRate>,
    start_responder: Responder<StartResult>,
    stop_responders: Vec<Responder<fidl_sme::StopApResultCode>>,
    _start_timeout: EventHandle,
    op_radio_cfg: OpRadioConfig,
}

enum State {
    Idle(Box<IdleState>),
    Started(Box<StartedState>),
    Starting(Box<StartingState>),
    Stopping(Box<StoppingState>),
}

struct StoppingState {
    ctx: Context,
    stop_req: fidl_mlme::StopRequest,
    responders: Vec<Responder<fidl_sme::StopApResultCode>>,
    stop_timeout: Option<EventHandle>,
}

#[derive(Clone)]
pub struct RsnCfg {
    psk: psk::Psk,
    rsne: Rsne,
}

struct StartedState {
    ssid: Ssid,
    rsn_cfg: Option<RsnCfg>,
    clients: HashMap<MacAddr, RemoteClient>,
    aid_map: aid::Map,
    op_radio_cfg: OpRadioConfig,
    ctx: Context,
}

pub struct Context {
    device_info: DeviceInfo,
    spectrum_management_support: fidl_common::SpectrumManagementSupport,
    mlme_sink: MlmeSink,
    timer: Timer<Event>,
}

pub struct ApSme {
    state: Option<State>,
}

struct IdleState {
    ctx: Context,
}

#[derive(Debug, PartialEq)]
pub enum StartResult {
    Success,
    Canceled,
    TimedOut,
    InvalidArguments(String),
    PreviousStartInProgress,
    AlreadyStarted,
    InternalError,
}

impl ApSme {
    pub fn new(
        device_info: DeviceInfo,
        spectrum_management_support: fidl_common::SpectrumManagementSupport,
    ) -> (Self, crate::MlmeSink, crate::MlmeStream, timer::EventStream<Event>) {
        let (mlme_sink, mlme_stream) = mpsc::unbounded();
        let (timer, time_stream) = timer::create_timer();
        let sme = ApSme {
            state: Some(State::Idle(Box::new(IdleState {
                ctx: Context {
                    device_info,
                    spectrum_management_support,
                    mlme_sink: MlmeSink::new(mlme_sink.clone()),
                    timer,
                },
            }))),
        };
        (sme, MlmeSink::new(mlme_sink), mlme_stream, time_stream)
    }

    pub fn on_start_command(&mut self, config: Config) -> oneshot::Receiver<StartResult> {
        let (responder, receiver) = Responder::new();
        self.state = self.state.take().map(|state| match state {
            State::Idle(idle_state) => {
                let mut ctx = idle_state.ctx;
                let op_radio_cfg = match validate_radio_cfg(
                    &ctx.device_info.bands[..],
                    &config.radio_cfg,
                    ctx.spectrum_management_support.clone(),
                ) {
                    Err(result) => {
                        responder.respond(result);
                        return State::Idle(Box::new(IdleState { ctx }));
                    }
                    Ok(op_radio_cfg) => op_radio_cfg,
                };

                let rsn_cfg_result = create_rsn_cfg(&config.ssid, &config.password[..]);
                let rsn_cfg = match rsn_cfg_result {
                    Err(e) => {
                        responder.respond(e);
                        return State::Idle(Box::new(IdleState { ctx }));
                    }
                    Ok(rsn_cfg) => rsn_cfg,
                };

                let capabilities =
                    mac::CapabilityInfo(ctx.device_info.softmac_hardware_capability as u16)
                        // IEEE Std 802.11-2016, 9.4.1.4: An AP sets the ESS subfield to 1 and the IBSS
                        // subfield to 0 within transmitted Beacon or Probe Response frames.
                        .with_ess(true)
                        .with_ibss(false)
                        // IEEE Std 802.11-2016, 9.4.1.4: An AP sets the Privacy subfield to 1 within
                        // transmitted Beacon, Probe Response, (Re)Association Response frames if data
                        // confidentiality is required for all Data frames exchanged within the BSS.
                        .with_privacy(rsn_cfg.is_some());

                let req = match create_start_request(
                    &op_radio_cfg,
                    &config.ssid,
                    rsn_cfg.as_ref(),
                    capabilities,
                ) {
                    Ok(req) => req,
                    Err(result) => {
                        responder.respond(result);
                        return State::Idle(Box::new(IdleState { ctx }));
                    }
                };

                // TODO(https://fxbug.dev/42103581): Select which rates are mandatory here.
                let rates = op_radio_cfg.basic_rates.iter().map(|r| SupportedRate(*r)).collect();

                ctx.mlme_sink.send(MlmeRequest::Start(req));
                let event = Event::Sme { event: SmeEvent::StartTimeout };
                let start_timeout = ctx.timer.schedule(event);

                State::Starting(Box::new(StartingState {
                    ctx,
                    ssid: config.ssid,
                    rsn_cfg,
                    _capabilities: capabilities,
                    _rates: rates,
                    start_responder: responder,
                    stop_responders: vec![],
                    _start_timeout: start_timeout,
                    op_radio_cfg,
                }))
            }
            s @ State::Starting(_) => {
                responder.respond(StartResult::PreviousStartInProgress);
                s
            }
            s @ State::Stopping(_) => {
                responder.respond(StartResult::Canceled);
                s
            }
            s @ State::Started(_) => {
                responder.respond(StartResult::AlreadyStarted);
                s
            }
        });
        receiver
    }

    pub fn on_stop_command(&mut self) -> oneshot::Receiver<fidl_sme::StopApResultCode> {
        let (responder, receiver) = Responder::new();
        self.state = self.state.take().map(|mut state| match state {
            State::Idle(idle_state) => {
                let mut ctx = idle_state.ctx;
                // We don't have an SSID, so just do a best-effort StopAP request with no SSID
                // filled in
                let stop_req = fidl_mlme::StopRequest { ssid: Ssid::empty().into() };
                let timeout = send_stop_req(&mut ctx, stop_req.clone());
                State::Stopping(Box::new(StoppingState {
                    ctx,
                    stop_req,
                    responders: vec![responder],
                    stop_timeout: Some(timeout),
                }))
            }
            State::Starting(ref mut starting_state) => {
                starting_state.stop_responders.push(responder);
                state
            }
            State::Stopping(mut state) => {
                state.responders.push(responder);
                // No stop request is ongoing, so forward this stop request.
                // The previous stop request may have timed out or failed and we are in an
                // unclean state where we don't know whether the AP has stopped or not.
                state.stop_timeout = state
                    .stop_timeout
                    .or_else(|| Some(send_stop_req(&mut state.ctx, state.stop_req.clone())));
                State::Stopping(state)
            }
            State::Started(mut bss) => {
                // IEEE Std 802.11-2016, 6.3.12.2.3: The SME should notify associated non-AP STAs of
                // imminent infrastructure BSS termination before issuing the MLME-STOP.request
                // primitive.
                for client_addr in bss.clients.keys() {
                    bss.ctx.mlme_sink.send(MlmeRequest::Deauthenticate(
                        fidl_mlme::DeauthenticateRequest {
                            peer_sta_address: client_addr.to_array(),
                            // This seems to be the most appropriate reason code (IEEE Std
                            // 802.11-2016, Table 9-45): Requesting STA is leaving the BSS (or
                            // resetting). The spec doesn't seem to mandate a choice of reason code
                            // here, so Fuchsia picks STA_LEAVING.
                            reason_code: fidl_ieee80211::ReasonCode::StaLeaving,
                        },
                    ));
                }

                let stop_req = fidl_mlme::StopRequest { ssid: bss.ssid.to_vec() };
                let timeout = send_stop_req(&mut bss.ctx, stop_req.clone());
                State::Stopping(Box::new(StoppingState {
                    ctx: bss.ctx,
                    stop_req,
                    responders: vec![responder],
                    stop_timeout: Some(timeout),
                }))
            }
        });
        receiver
    }

    pub fn get_running_ap(&self) -> Option<fidl_sme::Ap> {
        match self.state.as_ref() {
            Some(State::Started(bss)) => Some(fidl_sme::Ap {
                ssid: bss.ssid.to_vec(),
                channel: bss.op_radio_cfg.channel.primary,
                num_clients: bss.clients.len() as u16,
            }),
            _ => None,
        }
    }
}

fn send_stop_req(ctx: &mut Context, stop_req: fidl_mlme::StopRequest) -> EventHandle {
    let event = Event::Sme { event: SmeEvent::StopTimeout };
    let stop_timeout = ctx.timer.schedule(event);
    ctx.mlme_sink.send(MlmeRequest::Stop(stop_req));
    stop_timeout
}

impl super::Station for ApSme {
    type Event = Event;

    fn on_mlme_event(&mut self, event: MlmeEvent) {
        debug!("received MLME event: {:?}", event);
        self.state = self.state.take().map(|state| match state {
            State::Idle(_) => {
                warn!("received MlmeEvent while ApSme is idle {:?}", mlme_event_name(&event));
                state
            }
            State::Starting(state) => match event {
                MlmeEvent::StartConf { resp } => handle_start_conf(resp, *state),
                _ => {
                    warn!(
                        "received MlmeEvent while ApSme is starting {:?}",
                        mlme_event_name(&event)
                    );
                    State::Starting(state)
                }
            },
            State::Stopping(mut state) => match event {
                MlmeEvent::StopConf { resp } => match resp.result_code {
                    fidl_mlme::StopResultCode::Success
                    | fidl_mlme::StopResultCode::BssAlreadyStopped => {
                        for responder in state.responders.drain(..) {
                            responder.respond(fidl_sme::StopApResultCode::Success);
                        }
                        State::Idle(Box::new(IdleState { ctx: state.ctx }))
                    }
                    fidl_mlme::StopResultCode::InternalError => {
                        for responder in state.responders.drain(..) {
                            responder.respond(fidl_sme::StopApResultCode::InternalError);
                        }
                        state.stop_timeout = None;
                        State::Stopping(state)
                    }
                },
                _ => {
                    warn!(
                        "received MlmeEvent while ApSme is stopping {:?}",
                        mlme_event_name(&event)
                    );
                    State::Stopping(state)
                }
            },
            State::Started(mut bss) => {
                match event {
                    MlmeEvent::OnChannelSwitched { info } => bss.handle_channel_switch(info),
                    MlmeEvent::AuthenticateInd { ind } => bss.handle_auth_ind(ind),
                    MlmeEvent::DeauthenticateInd { ind } => {
                        bss.handle_deauth(&ind.peer_sta_address.into())
                    }
                    // TODO(https://fxbug.dev/42113580): This path should never be taken, as the MLME will never send
                    // this. Make sure this is the case.
                    MlmeEvent::DeauthenticateConf { resp } => {
                        bss.handle_deauth(&resp.peer_sta_address.into())
                    }
                    MlmeEvent::AssociateInd { ind } => bss.handle_assoc_ind(ind),
                    MlmeEvent::DisassociateInd { ind } => bss.handle_disassoc_ind(ind),
                    MlmeEvent::EapolInd { ind } => bss.handle_eapol_ind(ind),
                    MlmeEvent::EapolConf { resp } => bss.handle_eapol_conf(resp),
                    _ => {
                        warn!("unsupported MlmeEvent type {:?}; ignoring", mlme_event_name(&event))
                    }
                }
                State::Started(bss)
            }
        });
    }

    fn on_timeout(&mut self, timed_event: timer::Event<Event>) {
        self.state = self.state.take().map(|state| match state {
            State::Idle(_) => state,
            State::Starting(state) => match timed_event.event {
                Event::Sme { event: SmeEvent::StartTimeout } => {
                    let StartingState { mut ctx, start_responder, stop_responders, ssid, .. } =
                        *state;
                    warn!("Timed out waiting for MLME to start");
                    start_responder.respond(StartResult::TimedOut);
                    if stop_responders.is_empty() {
                        State::Idle(Box::new(IdleState { ctx }))
                    } else {
                        let stop_req = fidl_mlme::StopRequest { ssid: ssid.to_vec() };
                        let timeout = send_stop_req(&mut ctx, stop_req.clone());
                        State::Stopping(Box::new(StoppingState {
                            ctx,
                            stop_req,
                            responders: stop_responders,
                            stop_timeout: Some(timeout),
                        }))
                    }
                }
                _ => State::Starting(state),
            },
            State::Stopping(mut state) => {
                if let Event::Sme { event: SmeEvent::StopTimeout } = timed_event.event {
                    for responder in state.responders.drain(..) {
                        responder.respond(fidl_sme::StopApResultCode::TimedOut);
                    }
                    state.stop_timeout = None;
                }
                // If timeout triggered, then the responders and the timeout are cleared, and
                // we are left in an unclean stopping state
                State::Stopping(state)
            }
            State::Started(mut bss) => {
                bss.handle_timeout(timed_event);
                State::Started(bss)
            }
        });
    }
}

/// Validate the channel, PHY type, bandwidth, and band capabilities, in that order.
fn validate_radio_cfg(
    bands: &[fidl_mlme::BandCapability],
    radio_cfg: &RadioConfig,
    spectrum_management_support: fidl_common::SpectrumManagementSupport,
) -> Result<OpRadioConfig, StartResult> {
    let band_cap = get_band_cap_for_channel(bands, radio_cfg.channel).map_err(|e| {
        let e = e.context(format!(
            "No band capabilities for channel {}: {bands:?}",
            radio_cfg.channel.primary
        ));
        StartResult::InvalidArguments(format!("{e:?}"))
    })?;
    let channel = radio_cfg.channel;

    // Avoid hosting an AP on a 5 GHz channel on a non-DFS devices. There is no 5 GHz
    // channel that is valid in all regulatory domains.
    if channel.band == fidl_ieee80211::WlanBand::FiveGhz
        && !spectrum_management_support
            .dfs
            .as_ref()
            .is_some_and(|dfs| dfs.supported.unwrap_or(false))
    {
        return Err(StartResult::InvalidArguments(format!(
            "5 GHz channels not supported: {channel}"
        )));
    }

    let phy = radio_cfg.phy;
    match phy {
        fidl_ieee80211::WlanPhyType::Dsss
        | fidl_ieee80211::WlanPhyType::Hr
        | fidl_ieee80211::WlanPhyType::Ofdm
        | fidl_ieee80211::WlanPhyType::Erp => match channel.bandwidth {
            Bandwidth::Cbw20 => (),
            _ => {
                return Err(StartResult::InvalidArguments(format!(
                    "PHY type {phy:?} not supported on channel {channel}"
                )));
            }
        },
        fidl_ieee80211::WlanPhyType::Ht => {
            match channel.bandwidth {
                Bandwidth::Cbw20 | Bandwidth::Cbw40 | Bandwidth::Cbw40Below => (),
                _ => {
                    return Err(StartResult::InvalidArguments(format!(
                        "HT-mode not supported for channel {channel}"
                    )));
                }
            }

            match band_cap.ht_cap.as_ref() {
                None => {
                    return Err(StartResult::InvalidArguments(format!(
                        "No HT capabilities: {channel}"
                    )));
                }
                Some(ht_cap) => {
                    let ht_cap = parse_ht_capabilities(&ht_cap.bytes[..]).map_err(|e| {
                        error!("failed to parse HT capability bytes: {:?}", e);
                        StartResult::InternalError
                    })?;
                    let ht_cap_info = ht_cap.ht_cap_info;
                    if ht_cap_info.chan_width_set() == ChanWidthSet::TWENTY_ONLY
                        && channel.bandwidth != Bandwidth::Cbw20
                    {
                        return Err(StartResult::InvalidArguments(format!(
                            "20 MHz band capabilities does not support channel {channel}"
                        )));
                    }
                }
            }
        }
        fidl_ieee80211::WlanPhyType::Vht => {
            match channel.bandwidth {
                Bandwidth::Cbw160 | Bandwidth::Cbw80P80 { .. } => {
                    return Err(StartResult::InvalidArguments(format!(
                        "Supported for channel {channel} in VHT mode not available"
                    )));
                }
                _ => (),
            }

            if channel.band != fidl_ieee80211::WlanBand::FiveGhz {
                return Err(StartResult::InvalidArguments(format!(
                    "VHT only supported on 5 GHz channels: {channel}"
                )));
            }

            if band_cap.vht_cap.is_none() {
                return Err(StartResult::InvalidArguments(format!(
                    "No VHT capabilities: {channel}"
                )));
            }
        }
        fidl_ieee80211::WlanPhyType::Dmg
        | fidl_ieee80211::WlanPhyType::Tvht
        | fidl_ieee80211::WlanPhyType::S1G
        | fidl_ieee80211::WlanPhyType::Cdmg
        | fidl_ieee80211::WlanPhyType::Cmmg
        | fidl_ieee80211::WlanPhyType::He => {
            return Err(StartResult::InvalidArguments(format!("Unsupported PHY type: {phy:?}")));
        }
        fidl_common::WlanPhyTypeUnknown!() => {
            return Err(StartResult::InvalidArguments(format!("Unknown PHY type: {phy:?}")));
        }
    }

    Ok(OpRadioConfig { phy, channel, basic_rates: band_cap.basic_rates.clone() })
}

#[allow(clippy::too_many_arguments, reason = "mass allow for https://fxbug.dev/381896734")]
fn handle_start_conf(conf: fidl_mlme::StartConfirm, mut state: StartingState) -> State {
    if state.stop_responders.is_empty() {
        match conf.result_code {
            fidl_mlme::StartResultCode::Success => {
                state.start_responder.respond(StartResult::Success);
                State::Started(Box::new(StartedState {
                    ssid: state.ssid,
                    rsn_cfg: state.rsn_cfg,
                    clients: HashMap::new(),
                    aid_map: aid::Map::default(),
                    op_radio_cfg: state.op_radio_cfg,
                    ctx: state.ctx,
                }))
            }
            result_code => {
                error!("failed to start BSS: {:?}", result_code);
                state.start_responder.respond(StartResult::InternalError);
                State::Idle(Box::new(IdleState { ctx: state.ctx }))
            }
        }
    } else {
        state.start_responder.respond(StartResult::Canceled);
        let stop_req = fidl_mlme::StopRequest { ssid: state.ssid.to_vec() };
        let timeout = send_stop_req(&mut state.ctx, stop_req.clone());
        State::Stopping(Box::new(StoppingState {
            ctx: state.ctx,
            stop_req,
            responders: state.stop_responders,
            stop_timeout: Some(timeout),
        }))
    }
}

impl StartedState {
    /// Removes a client from the map.
    ///
    /// A client may only be removed via |remove_client| if:
    ///
    /// - MLME-DEAUTHENTICATE.request has been issued for the client, or,
    /// - MLME-DEAUTHENTICATE.indication or MLME-DEAUTHENTICATE.confirm has been received for the
    ///   client, or,
    /// - MLME-AUTHENTICATE.indication is being handled (see comment in |handle_auth_ind| for
    ///   details).
    ///
    /// If the client has an AID, its AID will be released from the AID map.
    ///
    /// Returns true if a client was removed, otherwise false.
    fn remove_client(&mut self, addr: &MacAddr) -> bool {
        if let Some(client) = self.clients.remove(addr) {
            if let Some(aid) = client.aid() {
                self.aid_map.release_aid(aid);
            }
            true
        } else {
            false
        }
    }

    fn handle_channel_switch(&mut self, info: fidl_internal::ChannelSwitchInfo) {
        info!("Channel switch for AP {:?}", info);
        self.op_radio_cfg.channel.primary = info.new_primary_channel.number;
        self.op_radio_cfg.channel.band = info.new_primary_channel.band;

        match Bandwidth::from_fidl(info.bandwidth, info.vht_secondary_80_channel.number) {
            Ok(cbw) => self.op_radio_cfg.channel.bandwidth = cbw,
            Err(e) => warn!("Invalid CBW: {}", e),
        }
    }

    fn handle_auth_ind(&mut self, ind: fidl_mlme::AuthenticateIndication) {
        let peer_addr: MacAddr = ind.peer_sta_address.into();
        if self.remove_client(&peer_addr) {
            // This may occur if an already authenticated client on the SME receives a fresh
            // MLME-AUTHENTICATE.indication from the MLME.
            //
            // This is safe, as we will make a fresh the client state and return an appropriate
            // MLME-AUTHENTICATE.response to the MLME, indicating whether it should deauthenticate
            // the client or not.
            warn!(
                "client {} is trying to reauthenticate; removing client and starting again",
                peer_addr
            );
        }
        let mut client = RemoteClient::new(peer_addr);
        client.handle_auth_ind(&mut self.ctx, ind.auth_type);
        if !client.authenticated() {
            info!("client {} was not authenticated", peer_addr);
            return;
        }

        info!("client {} authenticated", peer_addr);
        let _ = self.clients.insert(peer_addr, client);
    }

    fn handle_deauth(&mut self, peer_addr: &MacAddr) {
        if !self.remove_client(peer_addr) {
            warn!("client {} never authenticated, ignoring deauthentication request", peer_addr);
            return;
        }

        info!("client {} deauthenticated", peer_addr);
    }

    fn handle_assoc_ind(&mut self, ind: fidl_mlme::AssociateIndication) {
        let peer_addr: MacAddr = ind.peer_sta_address.into();

        let client = match self.clients.get_mut(&peer_addr) {
            None => {
                warn!("client {} never authenticated, ignoring association indication", peer_addr);
                return;
            }
            Some(client) => client,
        };

        client.handle_assoc_ind(
            &mut self.ctx,
            &mut self.aid_map,
            ind.capability_info,
            ind.rates.into_iter().map(SupportedRate).collect::<Vec<_>>(),
            &self.rsn_cfg,
            ind.rsne,
        );
        if !client.authenticated() {
            warn!("client {} failed to associate and was deauthenticated", peer_addr);
            let _ = self.remove_client(&peer_addr);
        } else if !client.associated() {
            warn!("client {} failed to associate but did not deauthenticate", peer_addr);
        } else {
            info!("client {} associated", peer_addr);
        }
    }

    fn handle_disassoc_ind(&mut self, ind: fidl_mlme::DisassociateIndication) {
        let peer_addr: MacAddr = ind.peer_sta_address.into();

        let client = match self.clients.get_mut(&peer_addr) {
            None => {
                warn!(
                    "client {} never authenticated, ignoring disassociation indication",
                    peer_addr
                );
                return;
            }
            Some(client) => client,
        };

        client.handle_disassoc_ind(&mut self.ctx, &mut self.aid_map);
        if client.associated() {
            panic!("client {peer_addr} didn't disassociate? this should never happen!")
        } else {
            info!("client {} disassociated", peer_addr);
        }
    }

    fn handle_timeout(&mut self, timed_event: timer::Event<Event>) {
        match timed_event.event {
            Event::Sme { .. } => (),
            Event::Client { addr, event } => {
                let client = match self.clients.get_mut(&addr) {
                    None => {
                        return;
                    }
                    Some(client) => client,
                };

                client.handle_timeout(&mut self.ctx, event);
                if !client.authenticated() {
                    if !self.remove_client(&addr) {
                        error!("failed to remove client {} from AID map", addr);
                    }
                    info!("client {} lost authentication", addr);
                }
            }
        }
    }

    fn handle_eapol_ind(&mut self, ind: fidl_mlme::EapolIndication) {
        let peer_addr: MacAddr = ind.src_addr.into();
        let client = match self.clients.get_mut(&peer_addr) {
            None => {
                warn!("client {} never authenticated, ignoring EAPoL indication", peer_addr);
                return;
            }
            Some(client) => client,
        };

        client.handle_eapol_ind(&mut self.ctx, &ind.data[..]);
    }

    fn handle_eapol_conf(&mut self, resp: fidl_mlme::EapolConfirm) {
        let dst_addr: MacAddr = resp.dst_addr.into();
        let client = match self.clients.get_mut(&dst_addr) {
            None => {
                warn!("never sent EAPOL frame to client {}, ignoring confirm", dst_addr);
                return;
            }
            Some(client) => client,
        };

        client.handle_eapol_conf(&mut self.ctx, resp.result_code);
    }
}

fn create_rsn_cfg(ssid: &Ssid, password: &[u8]) -> Result<Option<RsnCfg>, StartResult> {
    if password.is_empty() {
        Ok(None)
    } else {
        let psk_result = psk::compute(password, ssid);
        let psk = match psk_result {
            Err(e) => {
                return Err(StartResult::InvalidArguments(e.to_string()));
            }
            Ok(o) => o,
        };

        // Note: TKIP is legacy and considered insecure. Only allow CCMP usage
        // for group and pairwise ciphers.
        Ok(Some(RsnCfg { psk, rsne: Rsne::wpa2_rsne_with_caps(RsnCapabilities(0)) }))
    }
}

fn create_start_request(
    op_radio_cfg: &OpRadioConfig,
    ssid: &Ssid,
    ap_rsn: Option<&RsnCfg>,
    capabilities: mac::CapabilityInfo,
) -> Result<fidl_mlme::StartRequest, StartResult> {
    let rsne_bytes = ap_rsn.as_ref().map(|RsnCfg { rsne, .. }| {
        let mut buf = Vec::with_capacity(rsne.len());
        if let Err(e) = rsne.write_into(&mut buf) {
            error!("error writing RSNE into MLME-START.request: {}", e);
        }
        buf
    });

    let (channel_bandwidth, _vht_secondary_80_channel) = op_radio_cfg.channel.bandwidth.to_fidl();

    if op_radio_cfg.basic_rates.len() > fidl_internal::MAX_ASSOC_BASIC_RATES as usize {
        error!(
            "Too many basic rates ({}). Max is {}.",
            op_radio_cfg.basic_rates.len(),
            fidl_internal::MAX_ASSOC_BASIC_RATES
        );
        return Err(StartResult::InternalError);
    }

    Ok(fidl_mlme::StartRequest {
        ssid: ssid.to_vec(),
        bss_type: fidl_ieee80211::BssType::Infrastructure,
        beacon_period: DEFAULT_BEACON_PERIOD,
        dtim_period: DEFAULT_DTIM_PERIOD,
        primary: op_radio_cfg.channel.into(),
        capability_info: capabilities.raw(),
        rates: op_radio_cfg.basic_rates.clone(),
        country: fidl_mlme::Country {
            // TODO(https://fxbug.dev/42104247): Get config from wlancfg
            alpha2: *b"US",
            suffix: fidl_mlme::COUNTRY_ENVIRON_ALL,
        },
        rsne: rsne_bytes,
        mesh_id: vec![],
        phy: op_radio_cfg.phy,
        bandwidth: channel_bandwidth,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use crate::{MlmeStream, Station};
    use assert_matches::assert_matches;
    use fidl_fuchsia_wlan_mlme as fidl_mlme;
    use fidl_ieee80211::WlanBand::{FiveGhz, TwoGhz};
    use std::sync::LazyLock;
    use test_case::test_case;
    use wlan_common::channel::Bandwidth;
    use wlan_common::mac::Aid;
    use wlan_common::test_utils::fake_capabilities::{
        fake_2ghz_band_capability_ht, fake_5ghz_band_capability, fake_5ghz_band_capability_ht,
        fake_5ghz_band_capability_vht,
    };
    use wlan_common::test_utils::fake_features::{
        fake_dfs_supported, fake_spectrum_management_support_empty,
    };

    static AP_ADDR: LazyLock<MacAddr> =
        LazyLock::new(|| [0x11, 0x22, 0x33, 0x44, 0x55, 0x66].into());
    static CLIENT_ADDR: LazyLock<MacAddr> =
        LazyLock::new(|| [0x7A, 0xE7, 0x76, 0xD9, 0xF2, 0x67].into());
    static CLIENT_ADDR2: LazyLock<MacAddr> =
        LazyLock::new(|| [0x22, 0x22, 0x22, 0x22, 0x22, 0x22].into());
    static SSID: LazyLock<Ssid> =
        LazyLock::new(|| Ssid::try_from([0x46, 0x55, 0x43, 0x48, 0x53, 0x49, 0x41]).unwrap());

    const RSNE: &[u8] = &[
        0x30, // element id
        0x2A, // length
        0x01, 0x00, // version
        0x00, 0x0f, 0xac, 0x04, // group data cipher suite -- CCMP-128
        0x01, 0x00, // pairwise cipher suite count
        0x00, 0x0f, 0xac, 0x04, // pairwise cipher suite list -- CCMP-128
        0x01, 0x00, // akm suite count
        0x00, 0x0f, 0xac, 0x02, // akm suite list -- PSK
        0xa8, 0x04, // rsn capabilities
        0x01, 0x00, // pmk id count
        // pmk id list
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10,
        0x11, 0x00, 0x0f, 0xac, 0x04, // group management cipher suite -- CCMP-128
    ];

    fn unprotected_config() -> Config {
        Config {
            ssid: SSID.clone(),
            password: vec![],
            radio_cfg: RadioConfig::new(
                fidl_ieee80211::WlanPhyType::Ht,
                Bandwidth::Cbw20,
                11,
                TwoGhz,
            ),
        }
    }

    fn protected_config() -> Config {
        Config {
            ssid: SSID.clone(),
            password: vec![0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68],
            radio_cfg: RadioConfig::new(
                fidl_ieee80211::WlanPhyType::Ht,
                Bandwidth::Cbw20,
                11,
                TwoGhz,
            ),
        }
    }

    fn create_channel_switch_ind(channel: u8, band: fidl_ieee80211::WlanBand) -> MlmeEvent {
        MlmeEvent::OnChannelSwitched {
            info: fidl_internal::ChannelSwitchInfo {
                new_primary_channel: fidl_ieee80211::ChannelNumber { band, number: channel },
                bandwidth: fidl_ieee80211::ChannelBandwidth::Cbw20,
                vht_secondary_80_channel: fidl_ieee80211::ChannelNumber { band, number: 0 },
            },
        }
    }

    #[derive(Clone, Debug)]
    struct ValidateRadioConfigArgs {
        bands: Vec<fidl_mlme::BandCapability>,
        radio_cfg: RadioConfig,
        spectrum_management_support: fidl_common::SpectrumManagementSupport,
    }

    #[test_case(false, ValidateRadioConfigArgs {
        bands: vec![fake_2ghz_band_capability_ht()],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Ht,
            channel: Channel::new(15, Bandwidth::Cbw20, FiveGhz),
        },
        spectrum_management_support: fake_spectrum_management_support_empty(),
    }; "invalid US channel")]
    #[test_case(false, ValidateRadioConfigArgs {
        bands: vec![fake_5ghz_band_capability()],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Ht,
            channel: Channel::new(36, Bandwidth::Cbw20, FiveGhz),
        },
        spectrum_management_support: fake_spectrum_management_support_empty(),
    }; "5 GHz channel and no DFS support")]
    #[test_case(false, ValidateRadioConfigArgs {
        bands: vec![fake_2ghz_band_capability_ht()],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Dmg,
            channel: Channel::new(1, Bandwidth::Cbw20, TwoGhz),
        },
        spectrum_management_support: fake_spectrum_management_support_empty(),
    }; "DMG not supported")]
    #[test_case(false, ValidateRadioConfigArgs {
        bands: vec![fake_2ghz_band_capability_ht()],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Tvht,
            channel: Channel::new(1, Bandwidth::Cbw20, TwoGhz),
        },
        spectrum_management_support: fake_spectrum_management_support_empty(),
    }; "TVHT not supported")]
    #[test_case(false, ValidateRadioConfigArgs {
        bands: vec![fake_2ghz_band_capability_ht()],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::S1G,
            channel: Channel::new(1, Bandwidth::Cbw20, TwoGhz),
        },
        spectrum_management_support: fake_spectrum_management_support_empty(),
    }; "S1G not supported")]
    #[test_case(false, ValidateRadioConfigArgs {
        bands: vec![fake_2ghz_band_capability_ht()],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Cdmg,
            channel: Channel::new(1, Bandwidth::Cbw20, TwoGhz),
        },
        spectrum_management_support: fake_spectrum_management_support_empty(),
    }; "CDMG not supported")]
    #[test_case(false, ValidateRadioConfigArgs {
        bands: vec![fake_2ghz_band_capability_ht()],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Cmmg,
            channel: Channel::new(1, Bandwidth::Cbw20, TwoGhz),
        },
        spectrum_management_support: fake_spectrum_management_support_empty(),
    }; "CMMG not supported")]
    #[test_case(false, ValidateRadioConfigArgs {
        bands: vec![fake_2ghz_band_capability_ht()],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::He,
            channel: Channel::new(1, Bandwidth::Cbw20, TwoGhz),
        },
        spectrum_management_support: fake_spectrum_management_support_empty(),
    }; "HE not supported")]
    #[test_case(false, ValidateRadioConfigArgs {
        bands: vec![fake_2ghz_band_capability_ht()],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Ht,
            channel: Channel::new(36, Bandwidth::Cbw80, FiveGhz),
        },
        spectrum_management_support: fake_dfs_supported(),
    }; "invalid HT width")]
    #[test_case(false, ValidateRadioConfigArgs {
        bands: vec![fake_2ghz_band_capability_ht()],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Erp,
            channel: Channel::new(1, Bandwidth::Cbw40, TwoGhz),
        },
        spectrum_management_support: fake_spectrum_management_support_empty(),
    }; "non-HT greater than 20 MHz")]
    #[test_case(false, ValidateRadioConfigArgs {
        bands: vec![fake_5ghz_band_capability_ht(ChanWidthSet::TWENTY_FORTY)],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Ht,
            channel: Channel::new(36, Bandwidth::Cbw80, FiveGhz),
        },
        spectrum_management_support: fake_dfs_supported(),
    }; "HT greater than 40 MHz")]
    #[test_case(false, ValidateRadioConfigArgs {
        bands: vec![fake_5ghz_band_capability_ht(ChanWidthSet::TWENTY_FORTY)],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::unknown(),
            channel: Channel::new(36, Bandwidth::Cbw40, FiveGhz),
        },
        spectrum_management_support: fake_dfs_supported(),
    }; "Unknown PHY type")]
    #[test_case(false, ValidateRadioConfigArgs {
        bands: vec![fake_5ghz_band_capability_ht(ChanWidthSet::TWENTY_ONLY)],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Ht,
            channel: Channel::new(44, Bandwidth::Cbw40, FiveGhz),
        },
        spectrum_management_support: fake_dfs_supported(),
    }; "HT 20 MHz only")]
    #[test_case(false, ValidateRadioConfigArgs {
        bands: vec![fake_5ghz_band_capability()],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Ht,
            channel: Channel::new(48, Bandwidth::Cbw40, FiveGhz),
        },
        spectrum_management_support: fake_dfs_supported(),
    }; "No HT capabilities")]
    #[test_case(false, ValidateRadioConfigArgs {
        bands: vec![fake_5ghz_band_capability_vht()],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Vht,
            channel: Channel::new(36, Bandwidth::Cbw160, FiveGhz),
        },
        spectrum_management_support: fake_dfs_supported(),
    }; "160 MHz not supported")]
    #[test_case(false, ValidateRadioConfigArgs {
        bands: vec![fake_5ghz_band_capability_vht()],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Vht,
            channel: Channel::new(36, Bandwidth::Cbw80P80 { vht_secondary_80_channel: 106 }, FiveGhz),
        },
        spectrum_management_support: fake_dfs_supported(),
    }; "80+80 MHz not supported")]
    #[test_case(false, ValidateRadioConfigArgs {
        bands: vec![fake_2ghz_band_capability_ht()],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Vht,
            channel: Channel::new(1, Bandwidth::Cbw20, TwoGhz),
        },
        spectrum_management_support: fake_spectrum_management_support_empty(),
    }; "VHT 2.4 GHz not supported")]
    #[test_case(false, ValidateRadioConfigArgs {
        bands: vec![fake_5ghz_band_capability()],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Vht,
            channel: Channel::new(149, Bandwidth::Cbw80, FiveGhz),
        },
        spectrum_management_support: fake_dfs_supported(),
    }; "no VHT capabilities")]
    #[test_case(false, ValidateRadioConfigArgs {
        bands: vec![fake_2ghz_band_capability_ht(), fake_5ghz_band_capability_vht()],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Vht,
            channel: Channel::new(1, Bandwidth::Cbw40, TwoGhz),
        },
        spectrum_management_support: fake_spectrum_management_support_empty(),
    }; "no VHT capabilities on 2.4 GHz event when 5 GHz band capabilities provided")]
    #[test_case(false, ValidateRadioConfigArgs {
        bands: vec![fidl_mlme::BandCapability {
            primary_channels: vec![fidl_ieee80211::ChannelNumber {
                number: 2,
                band: fidl_ieee80211::WlanBand::TwoGhz,
            }],
            ..fake_2ghz_band_capability_ht()
        }],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Hr,
            channel: Channel::new(1, Bandwidth::Cbw40, TwoGhz),
        },
        spectrum_management_support: fake_spectrum_management_support_empty(),
    }; "disallow non-operating 2.4 GHz channel")]
    #[test_case(false, ValidateRadioConfigArgs {
        bands: vec![fidl_mlme::BandCapability {
            primary_channels: vec![fidl_ieee80211::ChannelNumber {
                number: 40,
                band: fidl_ieee80211::WlanBand::FiveGhz,
            }],
            ..fake_5ghz_band_capability_vht()
        }],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Vht,
            channel: Channel::new(36, Bandwidth::Cbw80, FiveGhz),
        },
        spectrum_management_support: fake_spectrum_management_support_empty(),
    }; "disallow non-operating 5 GHz channel")]
    #[test_case(true, ValidateRadioConfigArgs {
        bands: vec![fake_2ghz_band_capability_ht()],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Hr,
            channel: Channel::new(1, Bandwidth::Cbw20, TwoGhz),
        },
        spectrum_management_support: fake_spectrum_management_support_empty(),
    })]
    #[test_case(true, ValidateRadioConfigArgs {
        bands: vec![fake_2ghz_band_capability_ht()],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Erp,
            channel: Channel::new(1, Bandwidth::Cbw20, TwoGhz),
        },
        spectrum_management_support: fake_spectrum_management_support_empty(),
    })]
    #[test_case(true, ValidateRadioConfigArgs {
        bands: vec![fake_2ghz_band_capability_ht()],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Ht,
            channel: Channel::new(1, Bandwidth::Cbw20, TwoGhz),
        },
        spectrum_management_support: fake_spectrum_management_support_empty(),
    })]
    #[test_case(true, ValidateRadioConfigArgs {
        bands: vec![fake_2ghz_band_capability_ht()],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Ht,
            channel: Channel::new(1, Bandwidth::Cbw40, TwoGhz),
        },
        spectrum_management_support: fake_spectrum_management_support_empty(),
    })]
    #[test_case(true, ValidateRadioConfigArgs {
        bands: vec![fake_2ghz_band_capability_ht()],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Ht,
            channel: Channel::new(11, Bandwidth::Cbw40Below, TwoGhz),
        },
        spectrum_management_support: fake_spectrum_management_support_empty(),
    })]
    #[test_case(true, ValidateRadioConfigArgs {
        bands: vec![fake_5ghz_band_capability_ht(ChanWidthSet::TWENTY_ONLY)],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Ht,
            channel: Channel::new(36, Bandwidth::Cbw20, FiveGhz),
        },
        spectrum_management_support: fake_dfs_supported(),
    })]
    #[test_case(true, ValidateRadioConfigArgs {
        bands: vec![fake_5ghz_band_capability_ht(ChanWidthSet::TWENTY_FORTY)],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Ht,
            channel: Channel::new(36, Bandwidth::Cbw40, FiveGhz),
        },
        spectrum_management_support: fake_dfs_supported(),
    })]
    #[test_case(true, ValidateRadioConfigArgs {
        bands: vec![fake_5ghz_band_capability_ht(ChanWidthSet::TWENTY_FORTY)],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Ht,
            channel: Channel::new(40, Bandwidth::Cbw40Below, FiveGhz),
        },
        spectrum_management_support: fake_dfs_supported(),
    })]
    #[test_case(true, ValidateRadioConfigArgs {
        bands: vec![fake_5ghz_band_capability_ht(ChanWidthSet::TWENTY_FORTY)],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Ht,
            channel: Channel::new(36, Bandwidth::Cbw20, FiveGhz),
        },
        spectrum_management_support: fake_dfs_supported(),
    })]
    #[test_case(true, ValidateRadioConfigArgs {
        bands: vec![fake_5ghz_band_capability_vht()],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Ht,
            channel: Channel::new(36, Bandwidth::Cbw40, FiveGhz),
        },
        spectrum_management_support: fake_dfs_supported(),
    })]
    #[test_case(true, ValidateRadioConfigArgs {
        bands: vec![fake_5ghz_band_capability_vht()],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Ht,
            channel: Channel::new(40, Bandwidth::Cbw40Below, FiveGhz),
        },
        spectrum_management_support: fake_dfs_supported(),
    })]
    #[test_case(true, ValidateRadioConfigArgs {
        bands: vec![fake_5ghz_band_capability_vht()],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Vht,
            channel: Channel::new(36, Bandwidth::Cbw80, FiveGhz),
        },
        spectrum_management_support: fake_dfs_supported(),
    })]
    #[test_case(true, ValidateRadioConfigArgs {
        bands: vec![fake_2ghz_band_capability_ht(), fake_5ghz_band_capability_vht()],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Ht,
            channel: Channel::new(1, Bandwidth::Cbw40, TwoGhz),
        },
        spectrum_management_support: fake_spectrum_management_support_empty(),
    })]
    #[test_case(true, ValidateRadioConfigArgs {
        bands: vec![fake_2ghz_band_capability_ht(), fake_5ghz_band_capability_vht()],
        radio_cfg: RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Vht,
            channel: Channel::new(36, Bandwidth::Cbw80, FiveGhz),
        },
        spectrum_management_support: fake_dfs_supported(),
    })]
    fn test_validate_radio_cfg(expect_ok: bool, fn_args: ValidateRadioConfigArgs) {
        match validate_radio_cfg(
            &fn_args.bands[..],
            &fn_args.radio_cfg,
            fn_args.spectrum_management_support.clone(),
        ) {
            Ok(op_radio_cfg) => {
                if !expect_ok {
                    panic!("Unexpected successful validation: {0:?}, {op_radio_cfg:?}", fn_args);
                }
                assert_matches!(
                    op_radio_cfg,
                    OpRadioConfig {
                        phy,
                        channel,
                        basic_rates: _,
                    } => {
                        assert_eq!(phy, fn_args.radio_cfg.phy);
                        assert_eq!(channel, fn_args.radio_cfg.channel);
                    }
                )
            }
            Err(e @ StartResult::InvalidArguments { .. }) => {
                if expect_ok {
                    panic!("Unexpected failure to validate: {0:?}, {e:?}", fn_args)
                }
            }
            Err(e) => panic!("Unexpected StartResult value: {0:?}, {e:?}", fn_args),
        }
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn authenticate_while_sme_is_idle() {
        let (mut sme, mut mlme_stream, _) = create_sme().await;
        let client = Client::default();
        sme.on_mlme_event(client.create_auth_ind(fidl_mlme::AuthenticationTypes::OpenSystem));

        assert_matches!(mlme_stream.try_next(), Err(e) => {
            assert_eq!(e.to_string(), "receiver channel is empty");
        });
    }

    // Check status when sme is idle
    #[fuchsia::test(allow_stalls = false)]
    async fn status_when_sme_is_idle() {
        let (sme, _, _) = create_sme().await;
        assert_eq!(None, sme.get_running_ap());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn ap_starts_success() {
        let (mut sme, mut mlme_stream, _) = create_sme().await;
        let mut receiver = sme.on_start_command(unprotected_config());

        assert_matches!(mlme_stream.try_next(), Ok(Some(MlmeRequest::Start(start_req))) => {
            assert_eq!(start_req.ssid, SSID.to_vec());
            assert_eq!(
                start_req.capability_info,
                mac::CapabilityInfo(0).with_short_preamble(true).with_ess(true).raw(),
            );
            assert_eq!(start_req.bss_type, fidl_ieee80211::BssType::Infrastructure);
            assert_ne!(start_req.beacon_period, 0);
            assert_eq!(start_req.dtim_period, DEFAULT_DTIM_PERIOD);
            assert_eq!(
                start_req.primary,
                unprotected_config().radio_cfg.channel.into(),
            );
            assert!(start_req.rsne.is_none());
        });

        assert_eq!(Ok(None), receiver.try_recv());
        sme.on_mlme_event(create_start_conf(fidl_mlme::StartResultCode::Success));
        assert_eq!(Ok(Some(StartResult::Success)), receiver.try_recv());
    }

    // Check status when Ap starting and started
    #[fuchsia::test(allow_stalls = false)]
    async fn ap_starts_success_get_running_ap() {
        let (mut sme, mut mlme_stream, _) = create_sme().await;
        let mut receiver = sme.on_start_command(unprotected_config());
        assert_matches!(mlme_stream.try_next(), Ok(Some(MlmeRequest::Start(_start_req))) => {});
        // status should be Starting
        assert_eq!(None, sme.get_running_ap());
        assert_eq!(Ok(None), receiver.try_recv());
        sme.on_mlme_event(create_start_conf(fidl_mlme::StartResultCode::Success));
        assert_eq!(Ok(Some(StartResult::Success)), receiver.try_recv());
        assert_eq!(
            Some(fidl_sme::Ap {
                ssid: SSID.to_vec(),
                channel: unprotected_config().radio_cfg.channel.primary,
                num_clients: 0,
            }),
            sme.get_running_ap()
        );
    }

    // Check status after channel change
    #[fuchsia::test(allow_stalls = false)]
    async fn ap_check_status_after_channel_change() {
        let (mut sme, _, _) = start_unprotected_ap().await;
        // Check status
        assert_eq!(
            Some(fidl_sme::Ap {
                ssid: SSID.to_vec(),
                channel: unprotected_config().radio_cfg.channel.primary,
                num_clients: 0,
            }),
            sme.get_running_ap()
        );
        sme.on_mlme_event(create_channel_switch_ind(6, TwoGhz));
        // Check status
        assert_eq!(
            Some(fidl_sme::Ap { ssid: SSID.to_vec(), channel: 6, num_clients: 0 }),
            sme.get_running_ap()
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn ap_starts_timeout() {
        let (mut sme, _, mut time_stream) = create_sme().await;
        let mut receiver = sme.on_start_command(unprotected_config());

        let (_, event, _) = time_stream.try_next().unwrap().expect("expect timer message");
        sme.on_timeout(event);

        assert_eq!(Ok(Some(StartResult::TimedOut)), receiver.try_recv());
        // Check status
        assert_eq!(None, sme.get_running_ap());
    }

    // Disable logging to prevent failure from emitted error logs.
    #[fuchsia::test(allow_stalls = false, logging = false)]
    async fn ap_starts_fails() {
        let (mut sme, _, _) = create_sme().await;
        let mut receiver = sme.on_start_command(unprotected_config());

        sme.on_mlme_event(create_start_conf(fidl_mlme::StartResultCode::NotSupported));
        assert_eq!(Ok(Some(StartResult::InternalError)), receiver.try_recv());
        // Check status
        assert_eq!(None, sme.get_running_ap());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn start_req_while_ap_is_starting() {
        let (mut sme, _, _) = create_sme().await;
        let mut receiver_one = sme.on_start_command(unprotected_config());

        // While SME is starting, any start request receives an error immediately
        let mut receiver_two = sme.on_start_command(unprotected_config());
        assert_eq!(Ok(Some(StartResult::PreviousStartInProgress)), receiver_two.try_recv());

        // Start confirmation for first request should still have an affect
        sme.on_mlme_event(create_start_conf(fidl_mlme::StartResultCode::Success));
        assert_eq!(Ok(Some(StartResult::Success)), receiver_one.try_recv());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn start_req_while_ap_is_stopping() {
        let (mut sme, _, _) = start_unprotected_ap().await;
        let mut stop_receiver = sme.on_stop_command();
        let mut start_receiver = sme.on_start_command(unprotected_config());
        assert_eq!(Ok(None), stop_receiver.try_recv());
        assert_eq!(Ok(Some(StartResult::Canceled)), start_receiver.try_recv());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn ap_stops_while_idle() {
        let (mut sme, mut mlme_stream, _) = create_sme().await;
        let mut receiver = sme.on_stop_command();
        assert_matches!(mlme_stream.try_next(), Ok(Some(MlmeRequest::Stop(stop_req))) => {
            assert!(stop_req.ssid.is_empty());
        });

        // Respond with a successful stop result code
        sme.on_mlme_event(create_stop_conf(fidl_mlme::StopResultCode::Success));
        assert_eq!(Ok(Some(fidl_sme::StopApResultCode::Success)), receiver.try_recv());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn stop_req_while_ap_is_starting_then_succeeds() {
        let (mut sme, mut mlme_stream, _) = create_sme().await;
        let mut start_receiver = sme.on_start_command(unprotected_config());
        let mut stop_receiver = sme.on_stop_command();
        assert_eq!(Ok(None), start_receiver.try_recv());
        assert_eq!(Ok(None), stop_receiver.try_recv());

        // Verify start request is sent to MLME but not stop request yet
        assert_matches!(mlme_stream.try_next(), Ok(Some(MlmeRequest::Start(_))));
        assert_matches!(mlme_stream.try_next(), Err(e) => {
            assert_eq!(e.to_string(), "receiver channel is empty");
        });

        // Once start confirmation is finished, then stop request is sent out
        sme.on_mlme_event(create_start_conf(fidl_mlme::StartResultCode::Success));
        assert_eq!(Ok(Some(StartResult::Canceled)), start_receiver.try_recv());
        assert_eq!(Ok(None), stop_receiver.try_recv());
        assert_matches!(mlme_stream.try_next(), Ok(Some(MlmeRequest::Stop(stop_req))) => {
            assert_eq!(stop_req.ssid, SSID.to_vec());
        });

        // Respond with a successful stop result code
        sme.on_mlme_event(create_stop_conf(fidl_mlme::StopResultCode::Success));
        assert_eq!(Ok(Some(fidl_sme::StopApResultCode::Success)), stop_receiver.try_recv());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn stop_req_while_ap_is_starting_then_times_out() {
        let (mut sme, mut mlme_stream, mut time_stream) = create_sme().await;
        let mut start_receiver = sme.on_start_command(unprotected_config());
        let mut stop_receiver = sme.on_stop_command();
        assert_eq!(Ok(None), start_receiver.try_recv());
        assert_eq!(Ok(None), stop_receiver.try_recv());

        // Verify start request is sent to MLME but not stop request yet
        assert_matches!(mlme_stream.try_next(), Ok(Some(MlmeRequest::Start(_))));
        assert_matches!(mlme_stream.try_next(), Err(e) => {
            assert_eq!(e.to_string(), "receiver channel is empty");
        });

        // Time out the start request. Then stop request is sent out
        let (_, event, _) = time_stream.try_next().unwrap().expect("expect timer message");
        sme.on_timeout(event);
        assert_eq!(Ok(Some(StartResult::TimedOut)), start_receiver.try_recv());
        assert_eq!(Ok(None), stop_receiver.try_recv());
        assert_matches!(mlme_stream.try_next(), Ok(Some(MlmeRequest::Stop(stop_req))) => {
            assert_eq!(stop_req.ssid, SSID.to_vec());
        });

        // Respond with a successful stop result code
        sme.on_mlme_event(create_stop_conf(fidl_mlme::StopResultCode::Success));
        assert_eq!(Ok(Some(fidl_sme::StopApResultCode::Success)), stop_receiver.try_recv());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn ap_stops_after_started() {
        let (mut sme, mut mlme_stream, _) = start_unprotected_ap().await;
        let mut receiver = sme.on_stop_command();

        assert_matches!(mlme_stream.try_next(), Ok(Some(MlmeRequest::Stop(stop_req))) => {
            assert_eq!(stop_req.ssid, SSID.to_vec());
        });
        assert_eq!(Ok(None), receiver.try_recv());
        sme.on_mlme_event(create_stop_conf(fidl_mlme::StopResultCode::BssAlreadyStopped));
        assert_eq!(Ok(Some(fidl_sme::StopApResultCode::Success)), receiver.try_recv());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn ap_stops_after_started_and_deauths_all_clients() {
        let (mut sme, mut mlme_stream, _) = start_unprotected_ap().await;
        let client = Client::default();
        sme.on_mlme_event(client.create_auth_ind(fidl_mlme::AuthenticationTypes::OpenSystem));
        client.verify_auth_resp(&mut mlme_stream, fidl_mlme::AuthenticateResultCode::Success);

        // Check status
        assert_eq!(
            Some(fidl_sme::Ap {
                ssid: SSID.to_vec(),
                channel: unprotected_config().radio_cfg.channel.primary,
                num_clients: 1,
            }),
            sme.get_running_ap()
        );
        let mut receiver = sme.on_stop_command();
        assert_matches!(
        mlme_stream.try_next(),
        Ok(Some(MlmeRequest::Deauthenticate(deauth_req))) => {
            assert_eq!(&deauth_req.peer_sta_address, client.addr.as_array());
            assert_eq!(deauth_req.reason_code, fidl_ieee80211::ReasonCode::StaLeaving);
        });

        assert_matches!(mlme_stream.try_next(), Ok(Some(MlmeRequest::Stop(stop_req))) => {
            assert_eq!(stop_req.ssid, SSID.to_vec());
        });
        assert_eq!(Ok(None), receiver.try_recv());
        sme.on_mlme_event(create_stop_conf(fidl_mlme::StopResultCode::Success));
        assert_eq!(Ok(Some(fidl_sme::StopApResultCode::Success)), receiver.try_recv());

        // Check status
        assert_eq!(None, sme.get_running_ap());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn ap_queues_concurrent_stop_requests() {
        let (mut sme, _, _) = start_unprotected_ap().await;
        let mut receiver1 = sme.on_stop_command();
        let mut receiver2 = sme.on_stop_command();

        assert_eq!(Ok(None), receiver1.try_recv());
        assert_eq!(Ok(None), receiver2.try_recv());

        sme.on_mlme_event(create_stop_conf(fidl_mlme::StopResultCode::Success));
        assert_eq!(Ok(Some(fidl_sme::StopApResultCode::Success)), receiver1.try_recv());
        assert_eq!(Ok(Some(fidl_sme::StopApResultCode::Success)), receiver2.try_recv());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn uncleaned_stopping_state() {
        let (mut sme, mut mlme_stream, _) = start_unprotected_ap().await;
        let mut stop_receiver1 = sme.on_stop_command();
        // Clear out the stop request
        assert_matches!(mlme_stream.try_next(), Ok(Some(MlmeRequest::Stop(stop_req))) => {
            assert_eq!(stop_req.ssid, SSID.to_vec());
        });

        assert_eq!(Ok(None), stop_receiver1.try_recv());
        sme.on_mlme_event(create_stop_conf(fidl_mlme::StopResultCode::InternalError));
        assert_eq!(Ok(Some(fidl_sme::StopApResultCode::InternalError)), stop_receiver1.try_recv());

        // While in unclean stopping state, no start request can be made
        let mut start_receiver = sme.on_start_command(unprotected_config());
        assert_eq!(Ok(Some(StartResult::Canceled)), start_receiver.try_recv());
        assert_matches!(mlme_stream.try_next(), Err(e) => {
            assert_eq!(e.to_string(), "receiver channel is empty");
        });

        // SME will forward another stop request to lower layer
        let mut stop_receiver2 = sme.on_stop_command();
        assert_matches!(mlme_stream.try_next(), Ok(Some(MlmeRequest::Stop(stop_req))) => {
            assert_eq!(stop_req.ssid, SSID.to_vec());
        });

        // Respond successful this time
        assert_eq!(Ok(None), stop_receiver2.try_recv());
        sme.on_mlme_event(create_stop_conf(fidl_mlme::StopResultCode::Success));
        assert_eq!(Ok(Some(fidl_sme::StopApResultCode::Success)), stop_receiver2.try_recv());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn client_authenticates_supported_authentication_type() {
        let (mut sme, mut mlme_stream, _) = start_unprotected_ap().await;
        let client = Client::default();
        sme.on_mlme_event(client.create_auth_ind(fidl_mlme::AuthenticationTypes::OpenSystem));
        client.verify_auth_resp(&mut mlme_stream, fidl_mlme::AuthenticateResultCode::Success);
    }

    // Disable logging to prevent failure from emitted error logs.
    #[fuchsia::test(allow_stalls = false, logging = false)]
    async fn client_authenticates_unsupported_authentication_type() {
        let (mut sme, mut mlme_stream, _) = start_unprotected_ap().await;
        let client = Client::default();
        let auth_ind = client.create_auth_ind(fidl_mlme::AuthenticationTypes::FastBssTransition);
        sme.on_mlme_event(auth_ind);
        client.verify_auth_resp(&mut mlme_stream, fidl_mlme::AuthenticateResultCode::Refused);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn client_associates_unprotected_network() {
        let (mut sme, mut mlme_stream, _) = start_unprotected_ap().await;
        let client = Client::default();
        sme.on_mlme_event(client.create_auth_ind(fidl_mlme::AuthenticationTypes::OpenSystem));
        client.verify_auth_resp(&mut mlme_stream, fidl_mlme::AuthenticateResultCode::Success);

        sme.on_mlme_event(client.create_assoc_ind(None));
        client.verify_assoc_resp(
            &mut mlme_stream,
            1,
            fidl_mlme::AssociateResultCode::Success,
            false,
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn client_associates_valid_rsne() {
        let (mut sme, mut mlme_stream, _) = start_protected_ap().await;
        let client = Client::default();
        client.authenticate_and_drain_mlme(&mut sme, &mut mlme_stream);

        sme.on_mlme_event(client.create_assoc_ind(Some(RSNE.to_vec())));
        client.verify_assoc_resp(
            &mut mlme_stream,
            1,
            fidl_mlme::AssociateResultCode::Success,
            true,
        );
        client.verify_eapol_req(&mut mlme_stream);
    }

    // Disable logging to prevent failure from emitted error logs.
    #[fuchsia::test(allow_stalls = false, logging = false)]
    async fn client_associates_invalid_rsne() {
        let (mut sme, mut mlme_stream, _) = start_protected_ap().await;
        let client = Client::default();
        client.authenticate_and_drain_mlme(&mut sme, &mut mlme_stream);

        sme.on_mlme_event(client.create_assoc_ind(None));
        client.verify_refused_assoc_resp(
            &mut mlme_stream,
            fidl_mlme::AssociateResultCode::RefusedCapabilitiesMismatch,
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn rsn_handshake_timeout() {
        let (mut sme, mut mlme_stream, mut time_stream) = start_protected_ap().await;
        let client = Client::default();
        client.authenticate_and_drain_mlme(&mut sme, &mut mlme_stream);

        // Drain the association timeout message.
        assert_matches!(time_stream.try_next(), Ok(Some(_)));

        sme.on_mlme_event(client.create_assoc_ind(Some(RSNE.to_vec())));
        client.verify_assoc_resp(
            &mut mlme_stream,
            1,
            fidl_mlme::AssociateResultCode::Success,
            true,
        );

        // Drain the RSNA negotiation timeout message.
        assert_matches!(time_stream.try_next(), Ok(Some(_)));

        for _i in 0..4 {
            client.verify_eapol_req(&mut mlme_stream);
            let (_, event, _) = time_stream.try_next().unwrap().expect("expect timer message");
            sme.on_timeout(event);
        }

        client.verify_deauth_req(
            &mut mlme_stream,
            fidl_ieee80211::ReasonCode::FourwayHandshakeTimeout,
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn client_restarts_authentication_flow() {
        let (mut sme, mut mlme_stream, _) = start_unprotected_ap().await;
        let client = Client::default();
        client.authenticate_and_drain_mlme(&mut sme, &mut mlme_stream);
        client.associate_and_drain_mlme(&mut sme, &mut mlme_stream, None);

        sme.on_mlme_event(client.create_auth_ind(fidl_mlme::AuthenticationTypes::OpenSystem));
        client.verify_auth_resp(&mut mlme_stream, fidl_mlme::AuthenticateResultCode::Success);

        sme.on_mlme_event(client.create_assoc_ind(None));
        client.verify_assoc_resp(
            &mut mlme_stream,
            1,
            fidl_mlme::AssociateResultCode::Success,
            false,
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn multiple_clients_associate() {
        let (mut sme, mut mlme_stream, _) = start_protected_ap().await;
        let client1 = Client::default();
        let client2 = Client { addr: *CLIENT_ADDR2 };

        sme.on_mlme_event(client1.create_auth_ind(fidl_mlme::AuthenticationTypes::OpenSystem));
        client1.verify_auth_resp(&mut mlme_stream, fidl_mlme::AuthenticateResultCode::Success);

        sme.on_mlme_event(client2.create_auth_ind(fidl_mlme::AuthenticationTypes::OpenSystem));
        client2.verify_auth_resp(&mut mlme_stream, fidl_mlme::AuthenticateResultCode::Success);

        sme.on_mlme_event(client1.create_assoc_ind(Some(RSNE.to_vec())));
        client1.verify_assoc_resp(
            &mut mlme_stream,
            1,
            fidl_mlme::AssociateResultCode::Success,
            true,
        );
        client1.verify_eapol_req(&mut mlme_stream);

        sme.on_mlme_event(client2.create_assoc_ind(Some(RSNE.to_vec())));
        client2.verify_assoc_resp(
            &mut mlme_stream,
            2,
            fidl_mlme::AssociateResultCode::Success,
            true,
        );
        client2.verify_eapol_req(&mut mlme_stream);
    }

    fn create_start_conf(result_code: fidl_mlme::StartResultCode) -> MlmeEvent {
        MlmeEvent::StartConf { resp: fidl_mlme::StartConfirm { result_code } }
    }

    fn create_stop_conf(result_code: fidl_mlme::StopResultCode) -> MlmeEvent {
        MlmeEvent::StopConf { resp: fidl_mlme::StopConfirm { result_code } }
    }

    struct Client {
        addr: MacAddr,
    }

    impl Client {
        fn default() -> Self {
            Client { addr: *CLIENT_ADDR }
        }

        fn authenticate_and_drain_mlme(
            &self,
            sme: &mut ApSme,
            mlme_stream: &mut crate::MlmeStream,
        ) {
            sme.on_mlme_event(self.create_auth_ind(fidl_mlme::AuthenticationTypes::OpenSystem));
            assert_matches!(mlme_stream.try_next(), Ok(Some(MlmeRequest::AuthResponse(..))));
        }

        fn associate_and_drain_mlme(
            &self,
            sme: &mut ApSme,
            mlme_stream: &mut crate::MlmeStream,
            rsne: Option<Vec<u8>>,
        ) {
            sme.on_mlme_event(self.create_assoc_ind(rsne));
            assert_matches!(mlme_stream.try_next(), Ok(Some(MlmeRequest::AssocResponse(..))));
        }

        fn create_auth_ind(&self, auth_type: fidl_mlme::AuthenticationTypes) -> MlmeEvent {
            MlmeEvent::AuthenticateInd {
                ind: fidl_mlme::AuthenticateIndication {
                    peer_sta_address: self.addr.to_array(),
                    auth_type,
                },
            }
        }

        fn create_assoc_ind(&self, rsne: Option<Vec<u8>>) -> MlmeEvent {
            MlmeEvent::AssociateInd {
                ind: fidl_mlme::AssociateIndication {
                    peer_sta_address: self.addr.to_array(),
                    listen_interval: 100,
                    ssid: Some(SSID.to_vec()),
                    rsne,
                    capability_info: mac::CapabilityInfo(0).with_short_preamble(true).raw(),
                    rates: vec![
                        0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24, 0x30, 0x48, 0x60, 0x6c,
                    ],
                },
            }
        }

        fn verify_auth_resp(
            &self,
            mlme_stream: &mut MlmeStream,
            result_code: fidl_mlme::AuthenticateResultCode,
        ) {
            let msg = mlme_stream.try_next();
            assert_matches!(msg, Ok(Some(MlmeRequest::AuthResponse(auth_resp))) => {
                assert_eq!(&auth_resp.peer_sta_address, self.addr.as_array());
                assert_eq!(auth_resp.result_code, result_code);
            });
        }

        fn verify_assoc_resp(
            &self,
            mlme_stream: &mut MlmeStream,
            aid: Aid,
            result_code: fidl_mlme::AssociateResultCode,
            privacy: bool,
        ) {
            let msg = mlme_stream.try_next();
            assert_matches!(msg, Ok(Some(MlmeRequest::AssocResponse(assoc_resp))) => {
                assert_eq!(&assoc_resp.peer_sta_address, self.addr.as_array());
                assert_eq!(assoc_resp.association_id, aid);
                assert_eq!(assoc_resp.result_code, result_code);
                assert_eq!(
                    assoc_resp.capability_info,
                    mac::CapabilityInfo(0).with_short_preamble(true).with_privacy(privacy).raw(),
                );
            });
        }

        fn verify_refused_assoc_resp(
            &self,
            mlme_stream: &mut MlmeStream,
            result_code: fidl_mlme::AssociateResultCode,
        ) {
            let msg = mlme_stream.try_next();
            assert_matches!(msg, Ok(Some(MlmeRequest::AssocResponse(assoc_resp))) => {
                assert_eq!(&assoc_resp.peer_sta_address, self.addr.as_array());
                assert_eq!(assoc_resp.association_id, 0);
                assert_eq!(assoc_resp.result_code, result_code);
                assert_eq!(assoc_resp.capability_info, 0);
            });
        }

        fn verify_eapol_req(&self, mlme_stream: &mut MlmeStream) {
            assert_matches!(mlme_stream.try_next(), Ok(Some(MlmeRequest::Eapol(eapol_req))) => {
                assert_eq!(&eapol_req.src_addr, AP_ADDR.as_array());
                assert_eq!(&eapol_req.dst_addr, self.addr.as_array());
                assert!(!eapol_req.data.is_empty());
            });
        }

        fn verify_deauth_req(
            &self,
            mlme_stream: &mut MlmeStream,
            reason_code: fidl_ieee80211::ReasonCode,
        ) {
            let msg = mlme_stream.try_next();
            assert_matches!(msg, Ok(Some(MlmeRequest::Deauthenticate(deauth_req))) => {
                assert_eq!(&deauth_req.peer_sta_address, self.addr.as_array());
                assert_eq!(deauth_req.reason_code, reason_code);
            });
        }
    }

    // TODO(https://fxbug.dev/327499461): This function is async to ensure SME functions will
    // run in an async context and not call `wlan_common::timer::Timer::now` without an
    // executor.
    async fn start_protected_ap() -> (ApSme, crate::MlmeStream, timer::EventStream<Event>) {
        start_ap(true).await
    }

    // TODO(https://fxbug.dev/327499461): This function is async to ensure SME functions will
    // run in an async context and not call `wlan_common::timer::Timer::now` without an
    // executor.
    async fn start_unprotected_ap() -> (ApSme, crate::MlmeStream, timer::EventStream<Event>) {
        start_ap(false).await
    }

    // TODO(https://fxbug.dev/327499461): This function is async to ensure SME functions will
    // run in an async context and not call `wlan_common::timer::Timer::now` without an
    // executor.
    async fn start_ap(protected: bool) -> (ApSme, crate::MlmeStream, timer::EventStream<Event>) {
        let (mut sme, mut mlme_stream, mut time_stream) = create_sme().await;
        let config = if protected { protected_config() } else { unprotected_config() };
        let mut receiver = sme.on_start_command(config);
        assert_eq!(Ok(None), receiver.try_recv());
        assert_matches!(mlme_stream.try_next(), Ok(Some(MlmeRequest::Start(..))));
        // drain time stream
        while time_stream.try_next().is_ok() {}
        sme.on_mlme_event(create_start_conf(fidl_mlme::StartResultCode::Success));

        assert_eq!(Ok(Some(StartResult::Success)), receiver.try_recv());
        (sme, mlme_stream, time_stream)
    }

    // TODO(https://fxbug.dev/327499461): This function is async to ensure SME functions will
    // run in an async context and not call `wlan_common::timer::Timer::now` without an
    // executor.
    async fn create_sme() -> (ApSme, MlmeStream, timer::EventStream<Event>) {
        let (ap_sme, _mlme_sink, mlme_stream, time_stream) =
            ApSme::new(fake_device_info(*AP_ADDR), fake_spectrum_management_support_empty());
        (ap_sme, mlme_stream, time_stream)
    }
}
