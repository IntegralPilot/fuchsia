// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod psk;

use crate::Error;
use crate::key::Pmk;
use crate::key::exchange::Key;
use crate::rsna::{
    AuthRejectedReason, AuthStatus, Dot11VerifiedKeyFrame, SecAssocUpdate, UpdateSink,
};
use fidl_fuchsia_wlan_mlme::SaeFrame;
use ieee80211::{MacAddr, MacAddrBytes, Ssid};
use log::warn;
use wlan_common::ie::rsn::akm::{AKM_OWE, AKM_SAE};
use wlan_fcg_crypto::{owe, sae};
use zerocopy::SplitByteSlice;

/// IEEE Std 802.11-2016, 12.4.4.1
/// Elliptic curve group 19 is the default supported group -- all SAE peers must support it, and in
/// practice it is generally used.
const DEFAULT_GROUP_ID: u16 = 19;

#[derive(Error, Debug)]
pub enum AuthError {
    #[error("Failed to construct auth method from the given configuration: {:?}", _0)]
    FailedConstruction(anyhow::Error),
    #[error("Non-SAE auth method received an SAE event")]
    UnexpectedSaeEvent,
    #[error("Non-OWE auth method received an OWE event")]
    UnexpectedOweEvent,
    #[error("Failed to initiate OWE: {:?}", _0)]
    FailedInitiateOwe(anyhow::Error),
    #[error("Failed to handle OWE public key: {:?}", _0)]
    FailedHandleOwePublicKey(anyhow::Error),
}

pub struct SaeData {
    peer: MacAddr,
    pub pmk: Option<sae::Key>,
    handshake: Box<dyn sae::SaeHandshake>,
    // Our timer interface does not support cancellation, so we instead use a counter to skip
    // outdated timouts.
    retransmit_timeout_id: u64,
}

pub struct OweData {
    pub pmk: Option<Vec<u8>>,
    handshake: Box<dyn owe::ClientOweHandshake>,
}

#[derive(Debug, PartialEq, Clone)]
pub enum Config {
    ComputedPsk(psk::Psk),
    Sae {
        ssid: Ssid,
        password: Vec<u8>,
        mac: MacAddr,
        peer_mac: MacAddr,
        pwe_method: sae::PweMethod,
    },
    DriverSae {
        password: Vec<u8>,
    },
    Owe,
}

impl Config {
    pub fn method_name(&self) -> MethodName {
        match self {
            Config::ComputedPsk(_) => MethodName::Psk,
            Config::Sae { .. } | Config::DriverSae { .. } => MethodName::Sae,
            Config::Owe => MethodName::Owe,
        }
    }
}

pub enum Method {
    Psk(psk::Psk),
    Sae(SaeData),
    /// SAE handled in the driver/firmware, so the PMK will just eventually arrive.
    DriverSae(Option<sae::Key>),
    Owe(OweData),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MethodName {
    Psk,
    Sae,
    Owe,
}

impl std::fmt::Debug for Method {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::result::Result<(), std::fmt::Error> {
        match self {
            Self::Psk(psk) => write!(f, "Method::Psk({:?})", psk),
            Self::Sae(sae_data) => write!(
                f,
                "Method::Sae {{ peer: {:?}, pmk: {}, .. }}",
                sae_data.peer,
                match sae_data.pmk {
                    Some(_) => "Some(_)",
                    None => "None",
                }
            ),
            Self::DriverSae(key) => write!(f, "Method::DriverSae({:?})", key),
            Self::Owe(owe_data) => write!(
                f,
                "Method::Owe {{ pmk: {}, .. }}",
                match owe_data.pmk {
                    Some(_) => "Some(_)",
                    None => "None",
                }
            ),
        }
    }
}

impl Method {
    pub fn from_config(cfg: Config) -> Result<Method, AuthError> {
        match cfg {
            Config::ComputedPsk(psk) => Ok(Method::Psk(psk)),
            Config::Sae { ssid, password, mac, peer_mac, pwe_method } => {
                // TODO(https://fxbug.dev/42173568): Use PweMethod::Direct here for SAE Hash-to-Element.
                let handshake = sae::new_sae_handshake(
                    DEFAULT_GROUP_ID,
                    AKM_SAE,
                    pwe_method,
                    ssid,
                    password,
                    None, // Not required for PweMethod::Loop
                    mac,
                    peer_mac.clone(),
                )
                .map_err(AuthError::FailedConstruction)?;
                Ok(Method::Sae(SaeData {
                    peer: peer_mac,
                    pmk: None,
                    handshake,
                    retransmit_timeout_id: 0,
                }))
            }
            Config::DriverSae { .. } => Ok(Method::DriverSae(None)),
            Config::Owe => {
                let handshake = owe::new_client_owe_handshake(DEFAULT_GROUP_ID, AKM_OWE)
                    .map_err(AuthError::FailedConstruction)?;
                Ok(Method::Owe(OweData { pmk: None, handshake }))
            }
        }
    }

    // Unused as only PSK is supported so far.
    pub fn on_eapol_key_frame<B: SplitByteSlice>(
        &self,
        _update_sink: &mut UpdateSink,
        _frame: Dot11VerifiedKeyFrame<B>,
    ) -> Result<(), AuthError> {
        Ok(())
    }

    /// Currently only used so that an SAE handshake managed in firmware can send
    /// the PMK upward.
    pub fn on_pmk_available(
        &mut self,
        pmk: &[u8],
        pmkid: &[u8],
        assoc_update_sink: &mut UpdateSink,
    ) -> Result<(), AuthError> {
        match self {
            Method::DriverSae(key) => {
                key.replace(sae::Key { pmk: pmk.to_vec(), pmkid: pmkid.to_vec() });
                assoc_update_sink.push(SecAssocUpdate::Key(Key::Pmk(Pmk::new(
                    pmk.to_vec(),
                    Some(pmkid.to_vec()),
                ))));
                Ok(())
            }
            _ => Err(AuthError::UnexpectedSaeEvent),
        }
    }

    pub fn on_sae_handshake_ind(
        &mut self,
        assoc_update_sink: &mut UpdateSink,
    ) -> Result<(), AuthError> {
        match self {
            Method::Sae(sae_data) => {
                let mut sae_update_sink = sae::SaeUpdateSink::default();
                sae_data.handshake.initiate_sae(&mut sae_update_sink);
                process_sae_updates(sae_data, assoc_update_sink, sae_update_sink);
                Ok(())
            }
            _ => Err(AuthError::UnexpectedSaeEvent),
        }
    }

    pub fn on_sae_frame_rx(
        &mut self,
        assoc_update_sink: &mut UpdateSink,
        frame: SaeFrame,
    ) -> Result<(), AuthError> {
        match self {
            Method::Sae(sae_data) => {
                let mut sae_update_sink = sae::SaeUpdateSink::default();
                let frame_rx = sae::AuthFrameRx {
                    seq: frame.seq_num,
                    status_code: frame.status_code,
                    body: &frame.sae_fields[..],
                };
                sae_data.handshake.handle_frame(&mut sae_update_sink, &frame_rx);
                process_sae_updates(sae_data, assoc_update_sink, sae_update_sink);
                Ok(())
            }
            _ => Err(AuthError::UnexpectedSaeEvent),
        }
    }

    pub fn on_sae_timeout(
        &mut self,
        assoc_update_sink: &mut UpdateSink,
        event_id: u64,
    ) -> Result<(), AuthError> {
        match self {
            Method::Sae(sae_data) => {
                if sae_data.retransmit_timeout_id == event_id {
                    sae_data.retransmit_timeout_id += 1;
                    let mut sae_update_sink = sae::SaeUpdateSink::default();
                    sae_data
                        .handshake
                        .handle_timeout(&mut sae_update_sink, sae::Timeout::Retransmission);
                    process_sae_updates(sae_data, assoc_update_sink, sae_update_sink);
                }
                Ok(())
            }
            _ => Err(AuthError::UnexpectedSaeEvent),
        }
    }

    pub fn initiate_owe(&mut self, assoc_update_sink: &mut UpdateSink) -> Result<(), AuthError> {
        match self {
            Method::Owe(owe_data) => {
                let mut owe_update_sink = owe::OweUpdateSink::default();
                owe_data
                    .handshake
                    .initiate_owe(&mut owe_update_sink)
                    .map_err(AuthError::FailedInitiateOwe)?;
                process_owe_updates(owe_data, assoc_update_sink, owe_update_sink);
                Ok(())
            }
            _ => Err(AuthError::UnexpectedOweEvent),
        }
    }

    pub fn on_owe_public_key_rx(
        &mut self,
        assoc_update_sink: &mut UpdateSink,
        group: u16,
        public_key: Vec<u8>,
    ) -> Result<(), AuthError> {
        match self {
            Method::Owe(owe_data) => {
                let mut owe_update_sink = owe::OweUpdateSink::default();
                owe_data
                    .handshake
                    .handle_public_key(&mut owe_update_sink, group, public_key)
                    .map_err(AuthError::FailedHandleOwePublicKey)?;
                process_owe_updates(owe_data, assoc_update_sink, owe_update_sink);
                Ok(())
            }
            _ => Err(AuthError::UnexpectedOweEvent),
        }
    }
}

fn process_sae_updates(
    sae_data: &mut SaeData,
    assoc_update_sink: &mut UpdateSink,
    sae_update_sink: sae::SaeUpdateSink,
) {
    for sae_update in sae_update_sink {
        match sae_update {
            sae::SaeUpdate::SendFrame(frame) => {
                let sae_frame = SaeFrame {
                    peer_sta_address: sae_data.peer.clone().to_array(),
                    status_code: frame.status_code,
                    seq_num: frame.seq,
                    sae_fields: frame.body,
                };
                assoc_update_sink.push(SecAssocUpdate::TxSaeFrame(sae_frame));
            }
            sae::SaeUpdate::Success(key) => {
                sae_data.pmk.replace(key.clone());
                assoc_update_sink
                    .push(SecAssocUpdate::Key(Key::Pmk(Pmk::new(key.pmk, Some(key.pmkid)))));
                assoc_update_sink.push(SecAssocUpdate::SaeAuthStatus(AuthStatus::Success));
            }
            sae::SaeUpdate::Reject(reason) => {
                warn!("SAE handshake rejected: {:?}", reason);
                let status = match reason {
                    sae::RejectReason::AuthFailed => {
                        AuthStatus::Rejected(AuthRejectedReason::AuthFailed)
                    }
                    sae::RejectReason::KeyExpiration => {
                        AuthStatus::Rejected(AuthRejectedReason::PmksaExpired)
                    }
                    sae::RejectReason::TooManyRetries => {
                        AuthStatus::Rejected(AuthRejectedReason::TooManyRetries)
                    }
                    sae::RejectReason::InternalError(_) => AuthStatus::InternalError,
                };
                assoc_update_sink.push(SecAssocUpdate::SaeAuthStatus(status));
            }
            sae::SaeUpdate::ResetTimeout(timer) => {
                match timer {
                    sae::Timeout::KeyExpiration => (), // We don't use this event.
                    sae::Timeout::Retransmission => {
                        sae_data.retransmit_timeout_id += 1;
                        assoc_update_sink.push(SecAssocUpdate::ScheduleSaeTimeout(
                            sae_data.retransmit_timeout_id,
                        ));
                    }
                };
            }
            sae::SaeUpdate::CancelTimeout(timer) => {
                match timer {
                    sae::Timeout::KeyExpiration => (),
                    sae::Timeout::Retransmission => {
                        sae_data.retransmit_timeout_id += 1;
                    }
                };
            }
        }
    }
}

fn process_owe_updates(
    owe_data: &mut OweData,
    assoc_update_sink: &mut UpdateSink,
    owe_update_sink: owe::OweUpdateSink,
) {
    for owe_update in owe_update_sink {
        match owe_update {
            owe::OweUpdate::TxPublicKey { group_id, key } => {
                assoc_update_sink.push(SecAssocUpdate::TxOwePublicKey { group_id, key });
            }
            owe::OweUpdate::Success { key } => {
                owe_data.pmk.replace(key.clone());
                assoc_update_sink.push(SecAssocUpdate::Key(Key::Pmk(Pmk::from_pmk(key))));
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use assert_matches::assert_matches;
    use fuchsia_sync::Mutex;
    use std::sync::Arc;

    #[test]
    fn psk_rejects_sae() {
        let mut auth = Method::from_config(Config::ComputedPsk(Box::new([0x8; 16])))
            .expect("Failed to construct PSK auth method");
        let mut sink = UpdateSink::default();
        auth.on_sae_handshake_ind(&mut sink).expect_err("PSK auth method accepted SAE ind");
        let frame = SaeFrame {
            peer_sta_address: [0xaa; 6],
            status_code: fidl_fuchsia_wlan_ieee80211::StatusCode::Success,
            seq_num: 1,
            sae_fields: vec![0u8; 10],
        };
        auth.on_sae_frame_rx(&mut sink, frame).expect_err("PSK auth method accepted SAE frame");
        // No updates should be queued for these invalid ops.
        assert!(sink.is_empty());
    }

    #[derive(Default)]
    struct SaeCounter {
        initiated: bool,
        handled_commits: u32,
        handled_confirms: u32,
        handled_timeouts: u32,
    }

    struct DummySae(Arc<Mutex<SaeCounter>>);

    // This sends dummy frames as though it is the SAE initiator.
    impl sae::SaeHandshake for DummySae {
        fn initiate_sae(&mut self, sink: &mut sae::SaeUpdateSink) {
            self.0.lock().initiated = true;
            sink.push(sae::SaeUpdate::SendFrame(sae::AuthFrameTx {
                seq: 1,
                status_code: fidl_fuchsia_wlan_ieee80211::StatusCode::Success,
                body: vec![],
            }));
        }
        fn handle_commit(
            &mut self,
            _sink: &mut sae::SaeUpdateSink,
            _commit_msg: &sae::CommitMsg<'_>,
        ) {
            assert!(self.0.lock().initiated);
            self.0.lock().handled_commits += 1;
        }
        fn handle_confirm(
            &mut self,
            sink: &mut sae::SaeUpdateSink,
            _confirm_msg: &sae::ConfirmMsg<'_>,
        ) {
            assert!(self.0.lock().initiated);
            self.0.lock().handled_confirms += 1;
            sink.push(sae::SaeUpdate::SendFrame(sae::AuthFrameTx {
                seq: 2,
                status_code: fidl_fuchsia_wlan_ieee80211::StatusCode::Success,
                body: vec![],
            }));
            sink.push(sae::SaeUpdate::Success(sae::Key { pmk: vec![0xaa], pmkid: vec![0xbb] }))
        }
        fn handle_anti_clogging_token(
            &mut self,
            _sink: &mut sae::SaeUpdateSink,
            _msg: &sae::AntiCloggingTokenMsg<'_>,
        ) {
            panic!("The SAE initiator should never receive an anti-clogging token.");
        }
        fn handle_timeout(&mut self, _sink: &mut sae::SaeUpdateSink, _timeout: sae::Timeout) {
            self.0.lock().handled_timeouts += 1;
        }
    }

    // These are not valid commit and confirm bodies, but are appropriately sized so they will parse.

    const COMMIT: [u8; 98] = [
        0x13, 0x00, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa,
        0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa,
        0xaa, 0xaa, 0xaa, 0xaa, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb,
        0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb,
        0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb,
        0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb,
        0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb,
    ];
    const CONFIRM: [u8; 34] = [
        0xaa, 0xaa, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb,
        0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb,
        0xbb, 0xbb, 0xbb, 0xbb,
    ];

    #[test]
    fn sae_executes_handshake() {
        let sae_counter = Arc::new(Mutex::new(SaeCounter::default()));
        let mut auth = Method::Sae(SaeData {
            peer: MacAddr::from([0xaa; 6]),
            pmk: None,
            handshake: Box::new(DummySae(sae_counter.clone())),
            retransmit_timeout_id: 0,
        });
        let mut sink = UpdateSink::default();

        auth.on_sae_handshake_ind(&mut sink).expect("SAE handshake should accept SAE ind");
        assert!(sae_counter.lock().initiated);
        assert_matches!(sink.pop(), Some(SecAssocUpdate::TxSaeFrame(_)));

        let commit_frame = SaeFrame {
            peer_sta_address: [0xaa; 6],
            status_code: fidl_fuchsia_wlan_ieee80211::StatusCode::Success,
            seq_num: 1,
            sae_fields: COMMIT.to_vec(),
        };
        auth.on_sae_frame_rx(&mut sink, commit_frame).expect("SAE handshake should accept commit");
        assert_eq!(sae_counter.lock().handled_commits, 1);
        assert!(sink.is_empty());

        let confirm_frame = SaeFrame {
            peer_sta_address: [0xaa; 6],
            status_code: fidl_fuchsia_wlan_ieee80211::StatusCode::Success,
            seq_num: 2,
            sae_fields: CONFIRM.to_vec(),
        };
        auth.on_sae_frame_rx(&mut sink, confirm_frame)
            .expect("SAE handshake should accept confirm");
        assert_eq!(sae_counter.lock().handled_confirms, 1);
        assert_eq!(sink.len(), 3);
        assert_matches!(sink.remove(0), SecAssocUpdate::TxSaeFrame(_));
        assert_matches!(sink.remove(0), SecAssocUpdate::Key(_));
        assert_matches!(sink.remove(0), SecAssocUpdate::SaeAuthStatus(AuthStatus::Success));
        match auth {
            Method::Sae(sae_data) => assert!(sae_data.pmk.is_some()),
            _ => unreachable!(),
        };
    }

    #[test]
    fn sae_handles_current_timeouts() {
        let sae_counter = Arc::new(Mutex::new(SaeCounter::default()));
        let mut sae = Method::Sae(SaeData {
            peer: MacAddr::from([0xaa; 6]),
            pmk: None,
            handshake: Box::new(DummySae(sae_counter.clone())),
            retransmit_timeout_id: 0,
        });
        let mut sink = UpdateSink::default();

        if let Method::Sae(data) = &mut sae {
            process_sae_updates(
                data,
                &mut sink,
                vec![sae::SaeUpdate::ResetTimeout(sae::Timeout::Retransmission)],
            );
        };
        let event_id = assert_matches!(sink.pop(),
            Some(SecAssocUpdate::ScheduleSaeTimeout(id)) => id
        );
        sae.on_sae_timeout(&mut sink, event_id).expect("SAE handshake should accept timeout");
        assert_eq!(sae_counter.lock().handled_timeouts, 1);
        // Don't handle the same timeout twice.
        sae.on_sae_timeout(&mut sink, event_id).expect("SAE handshake should accept timeout");
        assert_eq!(sae_counter.lock().handled_timeouts, 1); // No timeout handled.

        // Don't handle a cancelled timeout.
        if let Method::Sae(data) = &mut sae {
            process_sae_updates(
                data,
                &mut sink,
                vec![
                    sae::SaeUpdate::ResetTimeout(sae::Timeout::Retransmission),
                    sae::SaeUpdate::CancelTimeout(sae::Timeout::Retransmission),
                ],
            );
        };
        let event_id = assert_matches!(sink.pop(),
                Some(SecAssocUpdate::ScheduleSaeTimeout(id)) => id
        );
        sae.on_sae_timeout(&mut sink, event_id).expect("SAE handshake should accept timeout");
        assert_eq!(sae_counter.lock().handled_timeouts, 1); // No timeout handled.
    }

    #[test]
    fn sae_key_expiration_no_op() {
        let sae_counter = Arc::new(Mutex::new(SaeCounter::default()));
        let mut data = SaeData {
            peer: MacAddr::from([0xaa; 6]),
            pmk: None,
            handshake: Box::new(DummySae(sae_counter.clone())),
            retransmit_timeout_id: 0,
        };
        let mut sink = UpdateSink::new();
        process_sae_updates(
            &mut data,
            &mut sink,
            vec![
                sae::SaeUpdate::ResetTimeout(sae::Timeout::KeyExpiration),
                sae::SaeUpdate::CancelTimeout(sae::Timeout::KeyExpiration),
            ],
        );
        assert!(sink.is_empty(), "KeyExpiration should not produce updates.");
    }

    #[test]
    fn driver_sae_handles_pmk() {
        let mut auth = Method::from_config(Config::DriverSae { password: vec![0xbb; 8] })
            .expect("Failed to construct PSK auth method");
        let mut sink = UpdateSink::default();
        auth.on_pmk_available(&[0xcc; 8][..], &[0xdd; 8][..], &mut sink)
            .expect("Driver SAE should handle on_pmk_available");
        assert_eq!(sink.len(), 1);
        let pmk = assert_matches!(sink.get(0), Some(SecAssocUpdate::Key(Key::Pmk(pmk))) => pmk);
        assert_eq!(pmk.pmk, vec![0xcc; 8]);
        assert_eq!(pmk.pmkid, Some(vec![0xdd; 8]));
    }

    #[test]
    fn driver_sae_rejects_sme_sae_calls() {
        let mut auth = Method::from_config(Config::DriverSae { password: vec![0xbb; 8] })
            .expect("Failed to construct PSK auth method");
        let mut sink = UpdateSink::default();
        auth.on_sae_handshake_ind(&mut sink).expect_err("Driver SAE shouldn't handle SAE ind");
        let frame = SaeFrame {
            peer_sta_address: [0xaa; 6],
            status_code: fidl_fuchsia_wlan_ieee80211::StatusCode::Success,
            seq_num: 1,
            sae_fields: COMMIT.to_vec(),
        };
        auth.on_sae_frame_rx(&mut sink, frame).expect_err("Driver SAE shouldn't handle frames");
        auth.on_sae_timeout(&mut sink, 0).expect_err("Driver SAE shouldn't handle SAE timeouts");
        assert!(sink.is_empty());
    }

    #[test]
    fn owe_initiates_and_handles_public_key() {
        let mut auth =
            Method::from_config(Config::Owe).expect("Failed to construct OWE auth method");
        let mut sink = UpdateSink::default();

        auth.initiate_owe(&mut sink).expect("OWE handshake should initiate");
        assert_eq!(sink.len(), 1);
        let (group_id, key) = assert_matches!(sink.remove(0),
            SecAssocUpdate::TxOwePublicKey { group_id, key } => (group_id, key)
        );
        assert_eq!(group_id, 19);
        assert!(!key.is_empty());

        const AP_PUBLIC_KEY: [u8; 32] = [
            0xa9, 0x8c, 0x47, 0xc5, 0xbd, 0xcf, 0x1d, 0x5e, 0x2c, 0x3c, 0x95, 0x8e, 0x10, 0xf3,
            0x71, 0x61, 0xc4, 0x61, 0x02, 0x13, 0x22, 0xb2, 0x95, 0xf6, 0xc7, 0x81, 0x1e, 0xf8,
            0x14, 0xc6, 0x03, 0x17,
        ];
        auth.on_owe_public_key_rx(&mut sink, group_id, AP_PUBLIC_KEY.to_vec())
            .expect("OWE handshake should handle public key");
        assert_eq!(sink.len(), 1);
        let pmk = assert_matches!(sink.remove(0), SecAssocUpdate::Key(Key::Pmk(pmk)) => pmk);
        assert!(!pmk.pmk.is_empty());
        assert_eq!(pmk.pmkid, None);
    }
}
