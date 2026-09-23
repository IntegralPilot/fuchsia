// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::{Protection, StateChangeContext, StateChangeContextExt, now};
use crate::client::EstablishRsnaFailureReason;
use crate::client::event::{self, Event, RsnaCompletionTimeout, RsnaResponseTimeout};
use crate::client::internal::Context;
use crate::client::rsn::Rsna;
use crate::{MlmeRequest, MlmeSink};
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_mlme as fidl_mlme;
use fuchsia_inspect_contrib::inspect_log;
use fuchsia_inspect_contrib::log::InspectBytes;
use ieee80211::{Bssid, MacAddr, MacAddrBytes, WILDCARD_BSSID};
use log::{error, warn};
use wlan_common::bss::BssDescription;
use wlan_common::timer::EventHandle;
use wlan_rsn::key::Tk;
use wlan_rsn::key::exchange::Key;
use wlan_rsn::rsna::{self, SecAssocStatus, SecAssocUpdate};
use wlan_statemachine::*;

#[derive(Debug)]
pub struct Init;

#[derive(Debug)]
pub struct EstablishingRsna {
    pub rsna: Rsna,
    // Timeout for the total duration RSNA may take to complete. This timeout is
    // never rescheduled.
    pub rsna_completion_timeout: Option<EventHandle>,
    // Timeout for the duration RSNA will await a response after transmitting a frame.
    // This timeout will be rescheduled upon receiving each valid frame.
    pub rsna_response_timeout: Option<EventHandle>,
    // Timeout for the duration RSNA will await a response after transmitting a frame
    // before possibly retransmitting the same frame. This timeout will be rescheduled
    // upon transmitting each frame.
    pub rsna_retransmission_timeout: Option<EventHandle>,

    // The following conditions must all be satisfied to consider an RSNA established. They
    // may be satisfied in multiple orders, so we represent them as a threshold rather than
    // as additional link states.
    // Indicates that all handshake frames have been sent or received.
    pub handshake_complete: bool,
    // If empty, indicates that we have received confirms for all keys we've installed.
    pub pending_key_ids: std::collections::HashSet<u16>,
}

#[derive(Debug)]
pub struct LinkUp {
    pub protection: Protection,
    pub since: zx::MonotonicInstant,
}

statemachine!(
    #[derive(Debug)]
    pub enum LinkState,
    () => Init,
    // If the association does not use an Rsna, we move directly from Init to LinkUp.
    Init => [EstablishingRsna, LinkUp],
    EstablishingRsna => LinkUp,
);

#[derive(Debug)]
enum RsnaStatus {
    Failed(EstablishRsnaFailureReason),
    Unchanged,
    Progressed {
        ap_responsive: Option<EventHandle>,
        new_retransmission_timeout: Option<EventHandle>,
        handshake_complete: bool,
        sent_keys: Vec<u16>,
    },
}

#[derive(Debug)]
enum RsnaProgressed {
    Complete(LinkUp),
    InProgress(EstablishingRsna),
}

impl EstablishingRsna {
    pub fn into_protection(self) -> Protection {
        match self.rsna.negotiated_protection.protection_type {
            rsna::ProtectionType::LegacyWpa1 => Protection::LegacyWpa(self.rsna),
            rsna::ProtectionType::Rsne => Protection::Rsna(self.rsna),
        }
    }

    fn on_rsna_progressed(
        mut self,
        ap_responsive: Option<EventHandle>,
        rsna_retransmission_timeout: Option<EventHandle>,
        handshake_complete: bool,
        sent_keys: Vec<u16>,
    ) -> Self {
        sent_keys.into_iter().for_each(|key| {
            let _ = self.pending_key_ids.insert(key);
        });
        self.handshake_complete |= handshake_complete;

        // Always cancel the retransmission timeout if RSNA progresssed since,
        // once transmitting frames, all meaningful progression implies the last
        // tranmsmitted frame resulted in progress.
        self.rsna_retransmission_timeout = rsna_retransmission_timeout;
        // If the AP is responsive, then reset the response timeout.
        self.rsna_response_timeout = ap_responsive.or(self.rsna_response_timeout);
        self
    }

    /// Establish the RSNA if all conditions are met.
    fn try_establish(self, bss: &BssDescription, context: &mut Context) -> RsnaProgressed {
        if self.handshake_complete && self.pending_key_ids.is_empty() {
            context.mlme_sink.send(MlmeRequest::SetCtrlPort(fidl_mlme::SetControlledPortRequest {
                peer_sta_address: bss.bssid.to_array(),
                state: fidl_mlme::ControlledPortState::Open,
            }));

            let now = now();
            RsnaProgressed::Complete(LinkUp { protection: self.into_protection(), since: now })
        } else {
            RsnaProgressed::InProgress(self)
        }
    }

    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255153)
    fn handle_rsna_response_timeout(mut self) -> Result<Self, EstablishRsnaFailureReason> {
        warn!("RSNA response timeout expired: {}ms", event::RSNA_RESPONSE_TIMEOUT_MILLIS);
        self.rsna_retransmission_timeout = None;
        self.rsna_response_timeout = None;
        self.rsna_completion_timeout = None;
        Err(self.rsna.supplicant.on_rsna_response_timeout())
    }

    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255153)
    fn handle_rsna_completion_timeout(mut self) -> Result<Self, EstablishRsnaFailureReason> {
        warn!("RSNA completion timeout expired: {}ms", event::RSNA_COMPLETION_TIMEOUT_MILLIS);
        self.rsna_retransmission_timeout = None;
        self.rsna_response_timeout = None;
        self.rsna_completion_timeout = None;
        Err(self.rsna.supplicant.on_rsna_completion_timeout())
    }
}

impl From<EstablishingRsna> for Protection {
    fn from(state: EstablishingRsna) -> Self {
        state.into_protection()
    }
}

impl LinkState {
    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255153)
    pub fn new(
        protection: Protection,
        context: &mut Context,
    ) -> Result<Self, EstablishRsnaFailureReason> {
        let mut rsna = match protection {
            Protection::Open | Protection::Wep(_) => {
                let now = now();
                return Ok(State::new(Init)
                    .transition_to(LinkUp { protection, since: now })
                    .into());
            }
            Protection::Rsna(rsna) | Protection::LegacyWpa(rsna) => rsna,
        };
        let mut update_sink = rsna::UpdateSink::default();
        rsna.supplicant.start(&mut update_sink).map_err(|e| {
            error!("could not start Supplicant: {}", e);
            EstablishRsnaFailureReason::StartSupplicantFailed
        })?;
        let state = State::new(Init).transition_to(EstablishingRsna {
            rsna,
            rsna_completion_timeout: Some(context.timer.schedule(event::RsnaCompletionTimeout)),
            rsna_response_timeout: Some(context.timer.schedule(event::RsnaResponseTimeout)),
            rsna_retransmission_timeout: None,
            handshake_complete: false,
            pending_key_ids: Default::default(),
        });
        let (transition, state) = state.release_data();
        match process_rsna_updates(context, None, update_sink, None) {
            RsnaStatus::Unchanged => Ok(transition.to(state).into()),
            // RSNA progress during start() should only be trivial.
            RsnaStatus::Progressed {
                ap_responsive: None,
                new_retransmission_timeout: None,
                handshake_complete: false,
                sent_keys,
            } if sent_keys.is_empty() => {
                // Normally, we call both on_rsna_progressed() and
                // try_establish(). Here, we omit try_establish() since it is not
                // possible to establish an RSNA without any EAPOL frames exchanged.
                let state = state.on_rsna_progressed(None, None, false, sent_keys);
                Ok(transition.to(state).into())
            }
            RsnaStatus::Failed(reason) => Err(reason),
            rsna_status => {
                error!("Unexpected RsnaStatus upon Supplicant::start(): {:?}", rsna_status);
                Err(EstablishRsnaFailureReason::StartSupplicantFailed)
            }
        }
    }

    pub fn disconnect(self) -> (Protection, Option<zx::MonotonicDuration>) {
        match self {
            Self::EstablishingRsna(state) => {
                let (_, state) = state.release_data();
                (state.into_protection(), None)
            }
            Self::LinkUp(state) => {
                let (_, state) = state.release_data();
                let connected_duration = now() - state.since;
                (state.protection, Some(connected_duration))
            }
            // We always transition to EstablishingRsna or LinkUp on initialization
            // and never transition back
            #[expect(clippy::unreachable)]
            _ => unreachable!(),
        }
    }

    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255153)
    fn on_eapol_event<T, H>(
        self,
        eapol_event: T,
        process_eapol_event: H,
        bss: &BssDescription,
        state_change_msg: &mut Option<StateChangeContext>,
        context: &mut Context,
    ) -> Result<Self, EstablishRsnaFailureReason>
    where
        H: Fn(&mut Context, &mut Rsna, &T) -> RsnaStatus,
    {
        match self {
            Self::EstablishingRsna(state) => {
                let (transition, mut state) = state.release_data();
                match process_eapol_event(context, &mut state.rsna, &eapol_event) {
                    RsnaStatus::Failed(failure_reason) => {
                        state_change_msg.set_msg(format!("{failure_reason:?}"));
                        Err(failure_reason)
                    }
                    RsnaStatus::Progressed {
                        ap_responsive,
                        new_retransmission_timeout,
                        sent_keys,
                        handshake_complete,
                    } => {
                        match state
                            .on_rsna_progressed(
                                ap_responsive,
                                new_retransmission_timeout,
                                handshake_complete,
                                sent_keys,
                            )
                            .try_establish(bss, context)
                        {
                            RsnaProgressed::Complete(link_up) => {
                                state_change_msg.set_msg("RSNA established".to_string());
                                Ok(transition.to(link_up).into())
                            }
                            RsnaProgressed::InProgress(still_establishing_rsna) => {
                                Ok(transition.to(still_establishing_rsna).into())
                            }
                        }
                    }
                    RsnaStatus::Unchanged => Ok(transition.to(state).into()),
                }
            }
            Self::LinkUp(state) => {
                let (transition, mut state) = state.release_data();
                // Drop EAPOL frames if the BSS is not an RSN.
                if let Protection::Rsna(rsna) | Protection::LegacyWpa(rsna) = &mut state.protection
                {
                    match process_eapol_event(context, rsna, &eapol_event) {
                        RsnaStatus::Unchanged => {}
                        // This can happen when there's a GTK rotation.
                        // Timeout is ignored because only one RX frame is
                        // needed in the exchange, so we are not waiting for
                        // another one.
                        // sent_keys is ignored because we're not waiting for key installations
                        // to complete the RSNA.
                        RsnaStatus::Progressed {
                            ap_responsive: _,
                            new_retransmission_timeout: _,
                            handshake_complete: _,
                            sent_keys: _,
                        } => {}
                        // Once re-keying is supported, the RSNA can fail in
                        // LinkUp as well and cause deauthentication.
                        s => error!("unexpected RsnaStatus in LinkUp state: {:?}", s),
                    };
                }
                Ok(transition.to(state).into())
            }
            // We always transition to EstablishingRsna or LinkUp on initialization
            // and never transition back
            #[expect(clippy::unreachable)]
            _ => unreachable!(),
        }
    }

    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255153)
    pub fn on_eapol_ind(
        self,
        eapol_ind: fidl_mlme::EapolIndication,
        bss: &BssDescription,
        state_change_msg: &mut Option<StateChangeContext>,
        context: &mut Context,
    ) -> Result<Self, EstablishRsnaFailureReason> {
        self.on_eapol_event(eapol_ind, process_eapol_ind, bss, state_change_msg, context)
    }

    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255153)
    pub fn on_eapol_conf(
        self,
        eapol_conf: fidl_mlme::EapolConfirm,
        bss: &BssDescription,
        state_change_msg: &mut Option<StateChangeContext>,
        context: &mut Context,
    ) -> Result<Self, EstablishRsnaFailureReason> {
        self.on_eapol_event(eapol_conf, process_eapol_conf, bss, state_change_msg, context)
    }

    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255153)
    pub fn on_set_keys_conf(
        self,
        set_keys_conf: fidl_mlme::SetKeysConfirm,
        bss: &BssDescription,
        state_change_msg: &mut Option<StateChangeContext>,
        context: &mut Context,
    ) -> Result<Self, EstablishRsnaFailureReason> {
        for key_result in &set_keys_conf.results {
            if zx::Status::ok(key_result.status).is_err() {
                state_change_msg.set_msg("Failed to set key in driver".to_string());
                return Err(EstablishRsnaFailureReason::InternalError);
            }
        }

        match self {
            Self::EstablishingRsna(state) => {
                let (transition, mut state) = state.release_data();
                for key_result in set_keys_conf.results {
                    let _ = state.pending_key_ids.remove(&key_result.key_id);
                }
                match state.try_establish(bss, context) {
                    RsnaProgressed::Complete(link_up) => {
                        state_change_msg.set_msg("RSNA established".to_string());
                        Ok(transition.to(link_up).into())
                    }
                    RsnaProgressed::InProgress(still_establishing_rsna) => {
                        Ok(transition.to(still_establishing_rsna).into())
                    }
                }
            }
            _ => Ok(self),
        }
    }

    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255153)
    pub fn handle_timeout(
        self,
        event: Event,
        state_change_msg: &mut Option<StateChangeContext>,
        context: &mut Context,
    ) -> Result<Self, EstablishRsnaFailureReason> {
        match self {
            Self::EstablishingRsna(state) => match event {
                Event::RsnaResponseTimeout(RsnaResponseTimeout {}) => {
                    let (transition, state) = state.release_data();
                    match state.handle_rsna_response_timeout() {
                        Ok(still_establishing_rsna) => {
                            Ok(transition.to(still_establishing_rsna).into())
                        }
                        Err(failure) => {
                            state_change_msg.set_msg("RSNA response timeout".to_string());
                            Err(failure)
                        }
                    }
                }
                Event::RsnaCompletionTimeout(RsnaCompletionTimeout {}) => {
                    let (transition, state) = state.release_data();
                    match state.handle_rsna_completion_timeout() {
                        Ok(still_establishing_rsna) => {
                            Ok(transition.to(still_establishing_rsna).into())
                        }
                        Err(failure) => {
                            state_change_msg.set_msg("RSNA completion timeout".to_string());
                            Err(failure)
                        }
                    }
                }
                Event::RsnaRetransmissionTimeout(timeout) => {
                    let (transition, mut state) = state.release_data();
                    match process_rsna_retransmission_timeout(context, timeout, &mut state.rsna) {
                        RsnaStatus::Failed(failure_reason) => Err(failure_reason),
                        RsnaStatus::Unchanged => Ok(transition.to(state).into()),
                        RsnaStatus::Progressed {
                            ap_responsive,
                            new_retransmission_timeout,
                            sent_keys,
                            handshake_complete,
                        } => {
                            let still_establishing_rsna = state.on_rsna_progressed(
                                ap_responsive,
                                new_retransmission_timeout,
                                handshake_complete,
                                sent_keys,
                            );
                            Ok(transition.to(still_establishing_rsna).into())
                        }
                    }
                }
                _ => Ok(state.into()),
            },
            Self::LinkUp(state) => Ok(state.into()),
            // We always transition to EstablishingRsna or LinkUp on initialization
            // and never transition back
            #[expect(clippy::unreachable)]
            _ => unreachable!(),
        }
    }
}

fn inspect_log_key(context: &mut Context, key: &Key) {
    let (cipher, key_index) = match key {
        Key::Ptk(ptk) => (Some(&ptk.cipher), None),
        Key::Gtk(gtk) => (Some(gtk.cipher()), Some(gtk.key_id())),
        _ => (None, None),
    };
    inspect_log!(context.inspect.rsn_events.lock(), {
        derived_key: key.name(),
        cipher?: cipher.map(|c| format!("{c:?}")),
        key_index?: key_index,
    });
}

fn send_keys(mlme_sink: &MlmeSink, bssid: Bssid, key: Key) -> Option<u16> {
    let key_descriptor = match key {
        Key::Ptk(ptk) => fidl_mlme::SetKeyDescriptor {
            key_type: fidl_mlme::KeyType::Pairwise,
            key: ptk.tk().to_vec(),
            key_id: 0,
            address: bssid.to_array(),
            cipher_suite_oui: eapol::to_array(&ptk.cipher.oui[..]),
            cipher_suite_type: fidl_ieee80211::CipherSuiteType::from_primitive_allow_unknown(
                ptk.cipher.suite_type.into(),
            ),
            rsc: 0,
        },
        Key::Gtk(gtk) => fidl_mlme::SetKeyDescriptor {
            key_type: fidl_mlme::KeyType::Group,
            key: gtk.tk().to_vec(),
            key_id: gtk.key_id() as u16,
            address: WILDCARD_BSSID.to_array(),
            cipher_suite_oui: eapol::to_array(&gtk.cipher().oui[..]),
            cipher_suite_type: fidl_ieee80211::CipherSuiteType::from_primitive_allow_unknown(
                gtk.cipher().suite_type.into(),
            ),
            rsc: gtk.key_rsc(),
        },
        Key::Igtk(igtk) => {
            let mut rsc = [0u8; 8];
            rsc[2..].copy_from_slice(&igtk.ipn[..]);
            fidl_mlme::SetKeyDescriptor {
                key_type: fidl_mlme::KeyType::Igtk,
                key: igtk.igtk,
                key_id: igtk.key_id,
                address: [0xFFu8; 6],
                cipher_suite_oui: eapol::to_array(&igtk.cipher.oui[..]),
                cipher_suite_type: fidl_ieee80211::CipherSuiteType::from_primitive_allow_unknown(
                    igtk.cipher.suite_type.into(),
                ),
                rsc: u64::from_be_bytes(rsc),
            }
        }
        _ => {
            error!("derived unexpected key");
            return None;
        }
    };
    let key_id = key_descriptor.key_id;
    mlme_sink
        .send(MlmeRequest::SetKeys(fidl_mlme::SetKeysRequest { keylist: vec![key_descriptor] }));
    Some(key_id)
}

/// Sends an eapol frame, and optionally schedules a timeout for the response.
/// If schedule_timeout is true, we should expect our peer to send us an eapol
/// frame in response to this one, and schedule a timeout as well.
fn send_eapol_frame(
    context: &mut Context,
    bssid: Bssid,
    sta_addr: MacAddr,
    frame: eapol::KeyFrameBuf,
    schedule_timeout: bool,
) -> Option<EventHandle> {
    let resp_timeout = if schedule_timeout {
        Some(context.timer.schedule(event::RsnaRetransmissionTimeout { bssid, sta_addr }))
    } else {
        None
    };
    inspect_log!(context.inspect.rsn_events.lock(), tx_eapol_frame: InspectBytes(&frame[..]));
    context.mlme_sink.send(MlmeRequest::Eapol(fidl_mlme::EapolRequest {
        src_addr: sta_addr.to_array(),
        dst_addr: bssid.to_array(),
        data: frame.into(),
    }));
    resp_timeout
}

fn process_eapol_conf(
    context: &mut Context,
    rsna: &mut Rsna,
    eapol_conf: &fidl_mlme::EapolConfirm,
) -> RsnaStatus {
    let mut update_sink = rsna::UpdateSink::default();
    match rsna.supplicant.on_eapol_conf(&mut update_sink, eapol_conf.result_code) {
        Err(e) => {
            error!("error handling EAPOL confirm: {}", e);
            RsnaStatus::Unchanged
        }
        Ok(()) => {
            process_rsna_updates(context, Some(Bssid::from(eapol_conf.dst_addr)), update_sink, None)
        }
    }
}

fn process_rsna_retransmission_timeout(
    context: &mut Context,
    timeout: event::RsnaRetransmissionTimeout,
    rsna: &mut Rsna,
) -> RsnaStatus {
    let mut update_sink = rsna::UpdateSink::default();
    match rsna.supplicant.on_rsna_retransmission_timeout(&mut update_sink) {
        Err(e) => {
            error!("{:?}", e);
            RsnaStatus::Failed(EstablishRsnaFailureReason::InternalError)
        }
        Ok(()) => process_rsna_updates(context, Some(timeout.bssid), update_sink, None),
    }
}

fn process_eapol_ind(
    context: &mut Context,
    rsna: &mut Rsna,
    ind: &fidl_mlme::EapolIndication,
) -> RsnaStatus {
    let mic_size = rsna.negotiated_protection.mic_size;
    let eapol_pdu = &ind.data[..];
    let eapol_frame = match eapol::KeyFrameRx::parse(mic_size as usize, eapol_pdu) {
        Ok(key_frame) => eapol::Frame::Key(key_frame),
        Err(e) => {
            error!("received invalid EAPOL Key frame: {:?}", e);
            inspect_log!(context.inspect.rsn_events.lock(), {
                rx_eapol_frame: InspectBytes(&eapol_pdu),
                status: format!("rejected (parse error): {:?}", e)
            });
            return RsnaStatus::Unchanged;
        }
    };

    let mut update_sink = rsna::UpdateSink::default();
    if let Err(e) = rsna.supplicant.on_eapol_frame(&mut update_sink, eapol_frame) {
        error!("error processing EAPOL key frame: {}", e);
        inspect_log!(context.inspect.rsn_events.lock(), {
            rx_eapol_frame: InspectBytes(&eapol_pdu),
            status: format!("rejected (processing error): {}", e)
        });
        return RsnaStatus::Unchanged;
    }

    inspect_log!(context.inspect.rsn_events.lock(), {
        rx_eapol_frame: InspectBytes(&eapol_pdu),
        status: "processed"
    });
    let ap_responsive =
        (!update_sink.is_empty()).then(|| context.timer.schedule(event::RsnaResponseTimeout {}));
    process_rsna_updates(context, Some(Bssid::from(ind.src_addr)), update_sink, ap_responsive)
}

fn process_rsna_updates(
    context: &mut Context,
    bssid: Option<Bssid>,
    updates: rsna::UpdateSink,
    ap_responsive: Option<EventHandle>,
) -> RsnaStatus {
    if updates.is_empty() {
        return RsnaStatus::Unchanged;
    }

    let sta_addr = MacAddr::from(context.device_info.sta_addr);
    let mut new_retransmission_timeout = None;
    let mut handshake_complete = false;
    let mut sent_keys = vec![];
    for update in updates {
        match update {
            // ESS Security Association requests to send an EAPOL frame.
            // Forward EAPOL frame to MLME.
            SecAssocUpdate::TxEapolKeyFrame { frame, expect_response } => {
                new_retransmission_timeout = match bssid {
                    None => {
                        error!("No BSSID set to handle SecAssocUpdate::TxEapolKeyFrame");
                        return RsnaStatus::Failed(EstablishRsnaFailureReason::InternalError);
                    }
                    Some(bssid) => {
                        send_eapol_frame(context, bssid, sta_addr, frame, expect_response)
                    }
                }
            }
            // ESS Security Association derived a new key.
            // Configure key in MLME.
            SecAssocUpdate::Key(key) => match bssid {
                None => {
                    error!("No BSSID set to handle SecAssocUpdate::Key");
                    return RsnaStatus::Failed(EstablishRsnaFailureReason::InternalError);
                }
                Some(bssid) => {
                    inspect_log_key(context, &key);
                    if let Some(key_id) = send_keys(&context.mlme_sink, bssid, key) {
                        sent_keys.push(key_id);
                    }
                }
            },
            // Received a status update.
            SecAssocUpdate::Status(status) => {
                inspect_log!(
                    context.inspect.rsn_events.lock(),
                    rsna_status: format!("{:?}", status)
                );
                match status {
                    // ESS Security Association was successfully established. Link is now up.
                    SecAssocStatus::EssSaEstablished => {
                        handshake_complete = true;
                    }
                    SecAssocStatus::WrongPassword => {
                        return RsnaStatus::Failed(EstablishRsnaFailureReason::InternalError);
                    }
                    SecAssocStatus::PmkSaEstablished => (),
                }
            }
            // TODO(https://fxbug.dev/42103820): We must handle SAE here for FullMAC devices.
            update => warn!("Unhandled association update: {:?}", update),
        }
    }

    RsnaStatus::Progressed {
        ap_responsive,
        new_retransmission_timeout,
        handshake_complete,
        sent_keys,
    }
}
