// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod authenticator;
mod supplicant;

use crate::key::exchange;
use crate::key::gtk::{Gtk, GtkProvider};
use crate::key::igtk::{Igtk, IgtkProvider};
use crate::key::ptk::Ptk;
use crate::nonce::NonceReader;
use crate::rsna::{Dot11VerifiedKeyFrame, NegotiatedProtection, Role, UpdateSink};
use crate::{Error, ProtectionInfo, rsn_ensure};
use fuchsia_sync::Mutex;
use ieee80211::MacAddr;
use std::ops::Deref;
use std::sync::Arc;
use wlan_common::ie::rsn::cipher::Cipher;
use wlan_common::ie::rsn::rsne::Rsne;
use wlan_common::ie::rsn::suite_filter::DEFAULT_GROUP_MGMT_CIPHER;
use wlan_statemachine::StateMachine;
use zerocopy::SplitByteSlice;

#[derive(Debug, PartialEq)]
pub enum MessageNumber {
    Message1 = 1,
    Message2 = 2,
    Message3 = 3,
    Message4 = 4,
}

/// Represents the current value of an Authenticator's Key Replay Counter
/// as defined in IEEE 802.11-2016 12.7.2 EAPOL-Key frames.
#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub struct AuthenticatorKeyReplayCounter(u64);

impl Deref for AuthenticatorKeyReplayCounter {
    type Target = u64;

    fn deref(&self) -> &u64 {
        &self.0
    }
}

impl AuthenticatorKeyReplayCounter {
    pub fn next_after(key_replay_counter: u64) -> Self {
        Self(key_replay_counter + 1)
    }
}

/// Represents the current value of a Supplicant's Key Replay Counter
/// as defined in IEEE 802.11-2016 12.7.2 EAPOL-Key frames.
#[derive(Debug, Clone, Copy)]
pub struct SupplicantKeyReplayCounter(u64);

impl Deref for SupplicantKeyReplayCounter {
    type Target = u64;

    fn deref(&self) -> &u64 {
        &self.0
    }
}

impl From<u64> for SupplicantKeyReplayCounter {
    fn from(a: u64) -> Self {
        Self(a)
    }
}

/// Struct which carries EAPOL key frames which comply with IEEE Std 802.11-2016, 12.7.2 and
/// IEEE Std 802.11-2016, 12.7.6.
pub struct FourwayHandshakeFrame<B: SplitByteSlice>(Dot11VerifiedKeyFrame<B>);

impl<B: SplitByteSlice> FourwayHandshakeFrame<B> {
    #[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
    pub fn from_verified(
        frame: Dot11VerifiedKeyFrame<B>,
        role: Role,
        nonce: Option<&[u8]>,
    ) -> Result<FourwayHandshakeFrame<B>, Error> {
        // Safe: the raw frame is never exposed outside of this function.
        let raw_frame = frame.unsafe_get_raw();
        // Drop messages which were not expected by the configured role.
        let msg_no = message_number(raw_frame);
        match role {
            // Authenticator should only receive message 2 and 4.
            Role::Authenticator => match msg_no {
                MessageNumber::Message2 | MessageNumber::Message4 => {}
                _ => return Err(Error::UnexpectedHandshakeMessage(msg_no.into()).into()),
            },
            Role::Supplicant => match msg_no {
                MessageNumber::Message1 | MessageNumber::Message3 => {}
                _ => return Err(Error::UnexpectedHandshakeMessage(msg_no.into()).into()),
            },
        };

        // Explicit validation based on the frame's message number.
        match msg_no {
            MessageNumber::Message1 => validate_message_1(raw_frame),
            MessageNumber::Message2 => validate_message_2(raw_frame),
            MessageNumber::Message3 => validate_message_3(raw_frame, nonce),
            MessageNumber::Message4 => validate_message_4(raw_frame),
        }?;

        Ok(FourwayHandshakeFrame(frame))
    }

    pub fn get(self) -> Dot11VerifiedKeyFrame<B> {
        self.0
    }

    /// Returns the 4-Way Handshake's message number.
    fn message_number(&self) -> MessageNumber {
        // Safe: At this point the frame was validated to be a valid 4-Way Handshake frame.
        message_number(self.unsafe_get_raw())
    }
}

impl<B: SplitByteSlice> std::ops::Deref for FourwayHandshakeFrame<B> {
    type Target = Dot11VerifiedKeyFrame<B>;

    fn deref(&self) -> &Dot11VerifiedKeyFrame<B> {
        &self.0
    }
}

#[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
pub fn get_group_mgmt_cipher(
    s_protection: &ProtectionInfo,
    a_protection: &ProtectionInfo,
) -> Result<Option<Cipher>, Error> {
    // IEEE 802.1-2016 12.6.3 - Management frame protection is
    // negotiated when an AP and non-AP STA set the Management
    // Frame Protection Capable field to 1 in their respective
    // RSNEs in the (re)association procedure, ...
    match (s_protection, a_protection) {
        (
            ProtectionInfo::Rsne(Rsne {
                rsn_capabilities: Some(s_rsn_capabilities),
                group_mgmt_cipher_suite: s_group_mgmt_cipher_suite,
                ..
            }),
            ProtectionInfo::Rsne(Rsne {
                rsn_capabilities: Some(a_rsn_capabilities),
                group_mgmt_cipher_suite: a_group_mgmt_cipher_suite,
                ..
            }),
        ) => {
            // Check for invalid bits
            if !s_rsn_capabilities.mgmt_frame_protection_cap()
                && s_rsn_capabilities.mgmt_frame_protection_req()
            {
                return Err(Error::InvalidClientMgmtFrameProtectionCapabilityBit);
            }
            if !a_rsn_capabilities.mgmt_frame_protection_cap()
                && a_rsn_capabilities.mgmt_frame_protection_req()
            {
                return Err(Error::InvalidApMgmtFrameProtectionCapabilityBit);
            }

            // Check for incompatible capabilities and requirements
            if s_rsn_capabilities.mgmt_frame_protection_req()
                && !a_rsn_capabilities.mgmt_frame_protection_cap()
            {
                return Err(Error::MgmtFrameProtectionRequiredByClient);
            }
            if !s_rsn_capabilities.mgmt_frame_protection_cap()
                && a_rsn_capabilities.mgmt_frame_protection_req()
            {
                return Err(Error::MgmtFrameProtectionRequiredByAp);
            }

            if s_rsn_capabilities.mgmt_frame_protection_cap()
                && a_rsn_capabilities.mgmt_frame_protection_cap()
            {
                let s_group_mgmt_cipher_suite =
                    s_group_mgmt_cipher_suite.unwrap_or(DEFAULT_GROUP_MGMT_CIPHER);
                let a_group_mgmt_cipher_suite =
                    a_group_mgmt_cipher_suite.unwrap_or(DEFAULT_GROUP_MGMT_CIPHER);

                if s_group_mgmt_cipher_suite != a_group_mgmt_cipher_suite {
                    return Err(Error::GroupMgmtCipherMismatch(
                        s_group_mgmt_cipher_suite,
                        a_group_mgmt_cipher_suite,
                    ));
                }

                return Ok(Some(s_group_mgmt_cipher_suite));
            }
        }
        (_, ProtectionInfo::Rsne(Rsne { rsn_capabilities: Some(a_rsn_capabilities), .. })) => {
            if a_rsn_capabilities.mgmt_frame_protection_req() {
                return Err(Error::MgmtFrameProtectionRequiredByAp);
            }
        }
        (ProtectionInfo::Rsne(Rsne { rsn_capabilities: Some(s_rsn_capabilities), .. }), _) => {
            if s_rsn_capabilities.mgmt_frame_protection_req() {
                return Err(Error::MgmtFrameProtectionRequiredByClient);
            }
        }

        // If management frame protection will not be used or is not required, then we can
        // safely ignore the supplicant ProtectionInfo
        (ProtectionInfo::Rsne(Rsne { rsn_capabilities: None, .. }), _)
        | (ProtectionInfo::LegacyWpa(_), _) => {}
    }

    Ok(None)
}

#[derive(Debug, Clone)]
pub struct Config {
    pub role: Role,
    pub s_addr: MacAddr,
    pub s_protection: ProtectionInfo,
    pub a_addr: MacAddr,
    pub a_protection: ProtectionInfo,
    pub nonce_rdr: Arc<NonceReader>,
    pub gtk_provider: Option<Arc<Mutex<GtkProvider>>>,
    pub igtk_provider: Option<Arc<Mutex<IgtkProvider>>>,
    pub pmksa_caching_supported: bool,

    // Private field to ensure Config can only be created by one of Config's
    // associated functions.
    _private: (),
}

impl Config {
    #[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
    pub fn new(
        role: Role,
        s_addr: MacAddr,
        s_protection: ProtectionInfo,
        a_addr: MacAddr,
        a_protection: ProtectionInfo,
        nonce_rdr: Arc<NonceReader>,
        gtk_provider: Option<Arc<Mutex<GtkProvider>>>,
        igtk_provider: Option<Arc<Mutex<IgtkProvider>>>,
        pmksa_caching_supported: bool,
    ) -> Result<Config, Error> {
        // Check that the supplicant protection is a subset of the authenticator protection.
        match (&s_protection, &a_protection) {
            (ProtectionInfo::Rsne(s_rsne), ProtectionInfo::Rsne(a_rsne)) => {
                // TODO(https://fxbug.dev/42104575): Replace with ? syntax when
                // NegotiatedProtection::from_protection no longer returns
                // anyhow::Error.
                match s_rsne.is_valid_subset_of(a_rsne) {
                    Ok(true) => {}
                    Ok(false) => {
                        return Err(Error::RsneInvalidSubset(s_rsne.clone(), a_rsne.clone()));
                    }
                    Err(e) => return Err(e.into()),
                };
            }
            // TODO(https://fxbug.dev/42149656): Check if the ProtectionInfo::LegacyWpa is a
            // subset or superset of the other ProtectionInfo
            (_, ProtectionInfo::LegacyWpa(_)) => {}
            (ProtectionInfo::LegacyWpa(_), _) => {}
        }

        // TODO(https://fxbug.dev/42104575): Replace with ? syntax when
        // NegotiatedProtection::from_protection no longer returns
        // anyhow::Error.
        // TODO(https://fxbug.dev/42149659): NegotiatedProtection should take into
        // account a_protection since the use of management frame
        // protection cannot be determined from s_protection alone.
        match NegotiatedProtection::from_protection(&s_protection) {
            Ok(negotiated_protection) => negotiated_protection,
            Err(e) => return Err(Error::InvalidSupplicantProtection(format!("{:?}", e))),
        };

        // Check that both an GtkProvider and IgtkProvider are provided if this configuration
        // is for an authenticator. An IgtkProvider is only required if management frame
        // protection is activated by this Config.
        if role == Role::Authenticator {
            rsn_ensure!(gtk_provider.is_some(), Error::MissingGtkProvider);

            // TODO(https://fxbug.dev/42149659): NegotiatedProtection should have a group_mgmt_cipher
            // associated function instead.
            match get_group_mgmt_cipher(&s_protection, &a_protection)? {
                Some(group_mgmt_cipher) => match igtk_provider.as_ref() {
                    None => return Err(Error::MissingIgtkProvider),
                    Some(igtk_provider) => {
                        let igtk_provider_cipher = igtk_provider.lock().cipher();
                        rsn_ensure!(
                            group_mgmt_cipher == igtk_provider_cipher,
                            Error::WrongIgtkProviderCipher(group_mgmt_cipher, igtk_provider_cipher),
                        );
                    }
                },
                None => {}
            }
        }

        Ok(Config {
            role,
            s_addr,
            s_protection,
            a_addr,
            a_protection,
            nonce_rdr,
            gtk_provider,
            igtk_provider,
            pmksa_caching_supported,
            _private: (),
        })
    }
}

impl PartialEq for Config {
    fn eq(&self, other: &Config) -> bool {
        self.role == other.role
            && self.s_addr == other.s_addr
            && self.s_protection == other.s_protection
            && self.a_addr == other.a_addr
            && self.a_protection == other.a_protection
            && self.pmksa_caching_supported == other.pmksa_caching_supported
    }
}

#[derive(Debug, PartialEq)]
pub enum Fourway {
    Authenticator(StateMachine<authenticator::State>),
    Supplicant(StateMachine<supplicant::State>),
}

impl Fourway {
    pub fn new(cfg: Config, pmk: Vec<u8>) -> Result<Fourway, anyhow::Error> {
        let fourway = match &cfg.role {
            Role::Supplicant => {
                let state = supplicant::new(cfg, pmk);
                Fourway::Supplicant(StateMachine::new(state))
            }
            Role::Authenticator => {
                let state = authenticator::new(cfg, pmk);
                Fourway::Authenticator(StateMachine::new(state))
            }
        };
        Ok(fourway)
    }

    #[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
    pub fn on_eapol_key_frame<B: SplitByteSlice>(
        &mut self,
        update_sink: &mut UpdateSink,
        frame: Dot11VerifiedKeyFrame<B>,
    ) -> Result<(), Error> {
        match self {
            Fourway::Authenticator(state_machine) => {
                let frame = FourwayHandshakeFrame::from_verified(frame, Role::Authenticator, None)?;
                state_machine.replace_state(|state| state.on_eapol_key_frame(update_sink, frame));
                Ok(())
            }
            Fourway::Supplicant(state_machine) => {
                let anonce = state_machine.as_ref().anonce();
                let frame = FourwayHandshakeFrame::from_verified(frame, Role::Supplicant, anonce)?;
                state_machine.replace_state(|state| state.on_eapol_key_frame(update_sink, frame));
                Ok(())
            }
        }
    }

    #[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
    pub fn on_rsna_response_timeout(&self) -> Result<(), Error> {
        match self {
            Fourway::Authenticator(state_machine) => state_machine.on_rsna_response_timeout(),
            Fourway::Supplicant(state_machine) => state_machine.on_rsna_response_timeout(),
        }
    }

    pub fn ptk(&self) -> Option<Ptk> {
        match self {
            Fourway::Authenticator(state_machine) => state_machine.as_ref().ptk(),
            Fourway::Supplicant(state_machine) => state_machine.as_ref().ptk(),
        }
    }

    pub fn gtk(&self) -> Option<Gtk> {
        match self {
            Fourway::Authenticator(state_machine) => state_machine.as_ref().gtk(),
            Fourway::Supplicant(state_machine) => state_machine.as_ref().gtk(),
        }
    }

    pub fn igtk(&self) -> Option<Igtk> {
        match self {
            Fourway::Authenticator(state_machine) => state_machine.as_ref().igtk(),
            Fourway::Supplicant(state_machine) => state_machine.as_ref().igtk(),
        }
    }

    #[cfg(test)]
    pub fn get_config(&self) -> Config {
        match self {
            Fourway::Supplicant(state_machine) => match state_machine.as_ref() {
                supplicant::State::AwaitingMsg1 { cfg, .. }
                | supplicant::State::AwaitingMsg3 { cfg, .. }
                | supplicant::State::KeysInstalled { cfg, .. } => cfg.clone(),
            },
            Fourway::Authenticator(state_machine) => match state_machine.as_ref() {
                authenticator::State::Idle { cfg, .. }
                | authenticator::State::AwaitingMsg2 { cfg, .. }
                | authenticator::State::AwaitingMsg4 { cfg, .. }
                | authenticator::State::KeysInstalled { cfg, .. } => cfg.clone(),
            },
        }
    }

    pub fn destroy(self) -> exchange::Config {
        let cfg = match self {
            Fourway::Supplicant(state_machine) => state_machine.into_state().destroy(),
            Fourway::Authenticator(state_machine) => state_machine.into_state().destroy(),
        };
        exchange::Config::FourWayHandshake(cfg)
    }
}

// Verbose and explicit verification of Message 1 to 4 against IEEE Std 802.11-2016, 12.7.6.2.

#[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
fn validate_message_1<B: SplitByteSlice>(frame: &eapol::KeyFrameRx<B>) -> Result<(), Error> {
    let key_info = frame.key_frame_fields.key_info();
    // IEEE Std 802.11-2016, 12.7.2 b.4)
    rsn_ensure!(!key_info.install(), Error::InvalidInstallBitValue(MessageNumber::Message1.into()));
    // IEEE Std 802.11-2016, 12.7.2 b.5)
    rsn_ensure!(key_info.key_ack(), Error::InvalidKeyAckBitValue(MessageNumber::Message1.into()));
    // IEEE Std 802.11-2016, 12.7.2 b.6)
    rsn_ensure!(!key_info.key_mic(), Error::InvalidKeyMicBitValue(MessageNumber::Message1.into()));
    // IEEE Std 802.11-2016, 12.7.2 b.7)
    rsn_ensure!(!key_info.secure(), Error::InvalidSecureBitValue(MessageNumber::Message1.into()));
    // IEEE Std 802.11-2016, 12.7.2 b.8)
    rsn_ensure!(!key_info.error(), Error::InvalidErrorBitValue(MessageNumber::Message1.into()));
    // IEEE Std 802.11-2016, 12.7.2 b.9)
    rsn_ensure!(!key_info.request(), Error::InvalidRequestBitValue(MessageNumber::Message1.into()));
    // IEEE Std 802.11-2016, 12.7.2 b.10)
    rsn_ensure!(
        !key_info.encrypted_key_data(),
        Error::InvalidEncryptedKeyDataBitValue(MessageNumber::Message1.into())
    );
    // IEEE Std 802.11-2016, 12.7.2 e)
    rsn_ensure!(
        !is_zero(&frame.key_frame_fields.key_nonce[..]),
        Error::InvalidNonce(MessageNumber::Message1.into())
    );
    // IEEE Std 802.11-2016, 12.7.2 f)
    // IEEE Std 802.11-2016, 12.7.6.2
    rsn_ensure!(
        is_zero(&frame.key_frame_fields.key_iv[..]),
        Error::InvalidIv(frame.eapol_fields.version, MessageNumber::Message1.into())
    );
    // IEEE Std 802.11-2016, 12.7.2 g)
    rsn_ensure!(
        frame.key_frame_fields.key_rsc.get() == 0,
        Error::InvalidRsc(MessageNumber::Message1.into())
    );

    // The first message of the Handshake is also required to carry a zeroed MIC.
    // Some routers however send messages without zeroing out the MIC beforehand.
    // To ensure compatibility with such routers, the MIC of the first message is
    // allowed to be set.
    // This assumption faces no security risk because the message's MIC is only
    // validated in the Handshake and not in the Supplicant or Authenticator
    // implementation.
    Ok(())
}

#[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
fn validate_message_2<B: SplitByteSlice>(frame: &eapol::KeyFrameRx<B>) -> Result<(), Error> {
    let key_info = frame.key_frame_fields.key_info();
    // IEEE Std 802.11-2016, 12.7.2 b.4)
    rsn_ensure!(!key_info.install(), Error::InvalidInstallBitValue(MessageNumber::Message2.into()));
    // IEEE Std 802.11-2016, 12.7.2 b.5)
    rsn_ensure!(!key_info.key_ack(), Error::InvalidKeyAckBitValue(MessageNumber::Message2.into()));
    // IEEE Std 802.11-2016, 12.7.2 b.6)
    rsn_ensure!(key_info.key_mic(), Error::InvalidKeyMicBitValue(MessageNumber::Message2.into()));
    // IEEE Std 802.11-2016, 12.7.2 b.7)
    rsn_ensure!(!key_info.secure(), Error::InvalidSecureBitValue(MessageNumber::Message2.into()));
    // IEEE Std 802.11-2016, 12.7.2 b.8)
    // Error bit only set by Supplicant in MIC failures in SMK derivation.
    // SMK derivation not yet supported.
    rsn_ensure!(!key_info.error(), Error::InvalidErrorBitValue(MessageNumber::Message2.into()));
    // IEEE Std 802.11-2016, 12.7.2 b.9)
    rsn_ensure!(!key_info.request(), Error::InvalidRequestBitValue(MessageNumber::Message2.into()));
    // IEEE Std 802.11-2016, 12.7.2 b.10)
    rsn_ensure!(
        !key_info.encrypted_key_data(),
        Error::InvalidEncryptedKeyDataBitValue(MessageNumber::Message2.into())
    );
    // IEEE Std 802.11-2016, 12.7.2 e)
    rsn_ensure!(
        !is_zero(&frame.key_frame_fields.key_nonce[..]),
        Error::InvalidNonce(MessageNumber::Message2.into())
    );
    // IEEE Std 802.11-2016, 12.7.2 f)
    // IEEE Std 802.11-2016, 12.7.6.3
    rsn_ensure!(
        is_zero(&frame.key_frame_fields.key_iv[..]),
        Error::InvalidIv(frame.eapol_fields.version, MessageNumber::Message2.into())
    );
    // IEEE Std 802.11-2016, 12.7.2 g)
    rsn_ensure!(
        frame.key_frame_fields.key_rsc.get() == 0,
        Error::InvalidRsc(MessageNumber::Message2.into())
    );

    Ok(())
}

#[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
fn validate_message_3<B: SplitByteSlice>(
    frame: &eapol::KeyFrameRx<B>,
    nonce: Option<&[u8]>,
) -> Result<(), Error> {
    let key_info = frame.key_frame_fields.key_info();
    // IEEE Std 802.11-2016, 12.7.2 b.4)
    // Install = 0 is only used in key mapping with TKIP and WEP, neither is supported by Fuchsia.
    rsn_ensure!(key_info.install(), Error::InvalidInstallBitValue(MessageNumber::Message3.into()));
    // IEEE Std 802.11-2016, 12.7.2 b.5)
    rsn_ensure!(key_info.key_ack(), Error::InvalidKeyAckBitValue(MessageNumber::Message3.into()));
    // These bit values are not used by WPA1.
    if frame.key_frame_fields.descriptor_type != eapol::KeyDescriptor::LEGACY_WPA1 {
        // IEEE Std 802.11-2016, 12.7.2 b.6)
        rsn_ensure!(
            key_info.key_mic(),
            Error::InvalidKeyMicBitValue(MessageNumber::Message3.into())
        );
        // IEEE Std 802.11-2016, 12.7.2 b.7)
        rsn_ensure!(
            key_info.secure(),
            Error::InvalidSecureBitValue(MessageNumber::Message3.into())
        );
        // IEEE Std 802.11-2016, 12.7.2 b.10)
        rsn_ensure!(
            key_info.encrypted_key_data(),
            Error::InvalidEncryptedKeyDataBitValue(MessageNumber::Message3.into())
        );
    }
    // IEEE Std 802.11-2016, 12.7.2 b.8)
    rsn_ensure!(!key_info.error(), Error::InvalidErrorBitValue(MessageNumber::Message3.into()));
    // IEEE Std 802.11-2016, 12.7.2 b.9)
    rsn_ensure!(!key_info.request(), Error::InvalidRequestBitValue(MessageNumber::Message3.into()));
    // IEEE Std 802.11-2016, 12.7.2 e)
    if let Some(nonce) = nonce {
        rsn_ensure!(
            !is_zero(&frame.key_frame_fields.key_nonce[..])
                && &frame.key_frame_fields.key_nonce[..] == nonce,
            Error::InvalidNonce(MessageNumber::Message3.into())
        );
    }
    // IEEE Std 802.11-2016, 12.7.2 f)
    // IEEE Std 802.11-2016, 12.7.6.4
    // IEEE 802.11-2016 requires a zeroed IV for 802.1X-2004+ and allows random ones for older
    // protocols. Some APs such as TP-Link violate this requirement and send non-zeroed IVs while
    // using 802.1X-2004. For compatibility, random IVs are allowed for 802.1X-2004.
    rsn_ensure!(
        frame.eapol_fields.version < eapol::ProtocolVersion::IEEE802DOT1X2010
            || is_zero(&frame.key_frame_fields.key_iv[..]),
        Error::InvalidIv(frame.eapol_fields.version, MessageNumber::Message3.into())
    );
    // IEEE Std 802.11-2016, 12.7.2 i) & j)
    // Key Data must not be empty.
    rsn_ensure!(frame.key_data.len() != 0, Error::EmptyKeyData(MessageNumber::Message3.into()));

    Ok(())
}

#[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
fn validate_message_4<B: SplitByteSlice>(frame: &eapol::KeyFrameRx<B>) -> Result<(), Error> {
    let key_info = frame.key_frame_fields.key_info();
    // IEEE Std 802.11-2016, 12.7.2 b.4)
    rsn_ensure!(!key_info.install(), Error::InvalidInstallBitValue(MessageNumber::Message4.into()));
    // IEEE Std 802.11-2016, 12.7.2 b.5)
    rsn_ensure!(!key_info.key_ack(), Error::InvalidKeyAckBitValue(MessageNumber::Message4.into()));
    // IEEE Std 802.11-2016, 12.7.2 b.6)
    rsn_ensure!(key_info.key_mic(), Error::InvalidKeyMicBitValue(MessageNumber::Message4.into()));
    // IEEE Std 802.11-2016, 12.7.2 b.7)
    rsn_ensure!(key_info.secure(), Error::InvalidSecureBitValue(MessageNumber::Message4.into()));
    // IEEE Std 802.11-2016, 12.7.2 b.8)
    // Error bit only set by Supplicant in MIC failures in SMK derivation.
    // SMK derivation not yet supported.
    rsn_ensure!(!key_info.error(), Error::InvalidErrorBitValue(MessageNumber::Message4.into()));
    // IEEE Std 802.11-2016, 12.7.2 b.9)
    rsn_ensure!(!key_info.request(), Error::InvalidRequestBitValue(MessageNumber::Message4.into()));
    // IEEE Std 802.11-2016, 12.7.2 b.10)
    rsn_ensure!(
        !key_info.encrypted_key_data(),
        Error::InvalidEncryptedKeyDataBitValue(MessageNumber::Message4.into())
    );
    // IEEE Std 802.11-2016, 12.7.2 f)
    // IEEE Std 802.11-2016, 12.7.6.5
    rsn_ensure!(
        is_zero(&frame.key_frame_fields.key_iv[..]),
        Error::InvalidIv(frame.eapol_fields.version, MessageNumber::Message4.into())
    );
    // IEEE Std 802.11-2016, 12.7.2 g)
    rsn_ensure!(
        frame.key_frame_fields.key_rsc.get() == 0,
        Error::InvalidRsc(MessageNumber::Message4.into())
    );

    Ok(())
}

fn message_number<B: SplitByteSlice>(rx_frame: &eapol::KeyFrameRx<B>) -> MessageNumber {
    // IEEE does not specify how to determine a frame's message number in the 4-Way Handshake
    // sequence. However, it's important to know a frame's message number to do further
    // validations. To derive the message number the key info field is used.
    // 4-Way Handshake specific EAPOL Key frame requirements:
    // IEEE Std 802.11-2016, 12.7.6.1

    // IEEE Std 802.11-2016, 12.7.6.2 & 12.7.6.4
    // Authenticator requires acknowledgement of all its sent frames.
    if rx_frame.key_frame_fields.key_info().key_ack() {
        // Authenticator only sends 1st and 3rd message of the handshake.
        // IEEE Std 802.11-2016, 12.7.2 b.4)
        // The third requires key installation while the first one doesn't.
        if rx_frame.key_frame_fields.key_info().install() {
            MessageNumber::Message3
        } else {
            MessageNumber::Message1
        }
    } else {
        // Supplicant only sends 2nd and 4th message of the handshake.
        // IEEE Std 802.11-2016, 12.7.2 b.7)
        // The fourth message is secured while the second one is not.
        if rx_frame.key_frame_fields.key_info().secure() {
            MessageNumber::Message4
        } else {
            MessageNumber::Message2
        }
    }
}

fn is_zero(slice: &[u8]) -> bool {
    slice.iter().all(|&x| x == 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rsna::{SecAssocUpdate, test_util};
    use crate::rsne::RsnCapabilities;
    use wlan_common::ie::rsn::cipher::{CIPHER_BIP_CMAC_128, CIPHER_BIP_CMAC_256};
    use wlan_common::ie::wpa::fake_wpa_ies::fake_deprecated_wpa1_vendor_ie;

    #[test]
    fn correct_value_returned_by_authenticator_key_replay_counter() {
        let key_replay_counter = AuthenticatorKeyReplayCounter(5);
        assert_eq!(*key_replay_counter, 5);
    }

    #[test]
    fn correct_value_returned_by_supplicant_key_replay_counter() {
        let key_replay_counter = SupplicantKeyReplayCounter(5);
        assert_eq!(*key_replay_counter, 5);
    }

    #[test]
    fn next_replay_counter_is_next_integer() {
        let s_key_replay_counter = SupplicantKeyReplayCounter(5);
        let next_key_replay_counter =
            AuthenticatorKeyReplayCounter::next_after(*s_key_replay_counter);
        assert_eq!(*next_key_replay_counter, 6);
    }

    // Create an Authenticator and Supplicant and performs the entire 4-Way Handshake.
    #[test]
    fn test_supplicant_with_authenticator() {
        let mut env = test_util::FourwayTestEnv::new(test_util::HandshakeKind::Wpa2, 1, 3);

        // Use arbitrarily chosen key_replay_counter.
        let msg1 = env.initiate(11.into());
        assert_eq!(msg1.keyframe().eapol_fields.version, eapol::ProtocolVersion::IEEE802DOT1X2004);
        let (msg2, _) = env.send_msg1_to_supplicant(msg1.keyframe(), 11.into());
        assert_eq!(msg2.keyframe().eapol_fields.version, eapol::ProtocolVersion::IEEE802DOT1X2004);
        let msg3 = env.send_msg2_to_authenticator(msg2.keyframe(), 12.into());
        assert_eq!(msg3.keyframe().eapol_fields.version, eapol::ProtocolVersion::IEEE802DOT1X2004);
        let (msg4, s_ptk, s_gtk) = env.send_msg3_to_supplicant(msg3.keyframe(), 12.into());
        assert_eq!(msg4.keyframe().eapol_fields.version, eapol::ProtocolVersion::IEEE802DOT1X2004);
        let (a_ptk, a_gtk) = env.send_msg4_to_authenticator(msg4.keyframe(), 13.into());

        // Verify Supplicant and Authenticator derived the same PTK.
        assert_eq!(s_ptk, a_ptk);

        // Verify Supplicant and Authenticator derived the same GTK and the Key Identifier and
        // RSC are correct.
        assert_eq!(s_gtk, a_gtk);
        assert_eq!(s_gtk.key_id(), 1);
        assert_eq!(s_gtk.key_rsc(), 3);
    }

    #[test]
    fn test_wpa3_handshake_generates_igtk_real_authenticator() {
        let mut env = test_util::FourwayTestEnv::new(test_util::HandshakeKind::Wpa3, 1, 754);

        // Use arbitrarily chosen key_replay_counter.
        let msg1 = env.initiate(11.into());
        assert_eq!(msg1.keyframe().eapol_fields.version, eapol::ProtocolVersion::IEEE802DOT1X2004);
        let (msg2, _) = env.send_msg1_to_supplicant(msg1.keyframe(), 11.into());
        assert_eq!(msg2.keyframe().eapol_fields.version, eapol::ProtocolVersion::IEEE802DOT1X2004);
        let msg3 = env.send_msg2_to_authenticator(msg2.keyframe(), 12.into());
        assert_eq!(msg3.keyframe().eapol_fields.version, eapol::ProtocolVersion::IEEE802DOT1X2004);
        let (msg4, s_ptk, s_gtk) = env.send_msg3_to_supplicant(msg3.keyframe(), 12.into());
        assert_eq!(msg4.keyframe().eapol_fields.version, eapol::ProtocolVersion::IEEE802DOT1X2004);
        let (a_ptk, a_gtk) = env.send_msg4_to_authenticator(msg4.keyframe(), 13.into());

        // Finally verify that Supplicant and Authenticator derived the same keys.
        assert_eq!(s_ptk, a_ptk);
        assert_eq!(s_gtk, a_gtk);
        assert_eq!(s_gtk.key_id(), 1);
        assert_eq!(s_gtk.key_rsc(), 754);
        assert!(env.supplicant.igtk().is_some());
        assert_eq!(env.supplicant.igtk(), env.authenticator.igtk());
    }

    fn run_wpa3_handshake_mock_authenticator(
        gtk: &[u8],
        igtk: Option<&[u8]>,
    ) -> (Ptk, Vec<SecAssocUpdate>) {
        let anonce = [0xab; 32];
        let mut supplicant = test_util::make_handshake(
            test_util::HandshakeKind::Wpa3,
            super::Role::Supplicant,
            1,
            3,
        );
        let msg1_buf = test_util::get_wpa3_4whs_msg1(&anonce[..]);
        let msg1 = msg1_buf.keyframe();
        let updates = test_util::send_msg_to_fourway(&mut supplicant, msg1, 0.into());
        let msg2 = test_util::expect_eapol_resp(&updates[..]);
        let a_ptk =
            test_util::get_wpa3_ptk(&anonce[..], &msg2.keyframe().key_frame_fields.key_nonce[..]);
        let msg3_buf = &test_util::get_wpa3_4whs_msg3(&a_ptk, &anonce[..], &gtk[..], igtk);
        let msg3 = msg3_buf.keyframe();
        (a_ptk, test_util::send_msg_to_fourway(&mut supplicant, msg3, 0.into()))
    }

    #[test]
    fn test_wpa3_handshake_generates_igtk_mock_authenticator() {
        let gtk = [0xbb; 32];
        let igtk = [0xcc; 32];
        let (ptk, updates) = run_wpa3_handshake_mock_authenticator(&gtk[..], Some(&igtk[..]));

        test_util::expect_eapol_resp(&updates[..]);
        let s_ptk = test_util::expect_reported_ptk(&updates[..]);
        let s_gtk = test_util::expect_reported_gtk(&updates[..]);
        let s_igtk = test_util::expect_reported_igtk(&updates[..]);
        assert_eq!(s_ptk, ptk);
        assert_eq!(&s_gtk.bytes[..], &gtk[..]);
        assert_eq!(&s_igtk.igtk[..], &igtk[..]);
    }

    #[test]
    fn test_wpa3_handshake_requires_igtk() {
        let gtk = [0xbb; 32];
        let (_ptk, updates) = run_wpa3_handshake_mock_authenticator(&gtk[..], None);
        assert!(
            test_util::get_eapol_resp(&updates[..]).is_none(),
            "WPA3 should not send EAPOL msg4 without IGTK"
        );
    }

    #[test]
    fn test_wpa1_handshake() {
        let pmk = test_util::get_pmk();
        let cfg = test_util::make_wpa1_fourway_cfg();
        let mut supplicant = Fourway::new(cfg, pmk).expect("error while creating 4-Way Handshake");

        // We don't have a WPA1 authenticator so we use fake messages.
        let anonce = [0xab; 32];
        let msg1_buf = test_util::get_wpa1_4whs_msg1(&anonce[..]);
        let msg1 = msg1_buf.keyframe();
        let updates = test_util::send_msg_to_fourway(&mut supplicant, msg1, 0.into());
        let msg2 = test_util::expect_eapol_resp(&updates[..]);
        let a_ptk =
            test_util::get_wpa1_ptk(&anonce[..], &msg2.keyframe().key_frame_fields.key_nonce[..]);
        let msg3_buf = &test_util::get_wpa1_4whs_msg3(&a_ptk, &anonce[..]);
        let msg3 = msg3_buf.keyframe();
        let updates = test_util::send_msg_to_fourway(&mut supplicant, msg3, 0.into());

        // Verify that we completed the exchange and computed the same PTK as our fake AP would.
        test_util::expect_eapol_resp(&updates[..]);
        let s_ptk = test_util::expect_reported_ptk(&updates[..]);
        assert_eq!(s_ptk, a_ptk);
    }

    #[test]
    fn test_supplicant_replay_msg3() {
        let mut env = test_util::FourwayTestEnv::new(test_util::HandshakeKind::Wpa2, 1, 3);

        // Use arbitrarily chosen key_replay_counter.
        let msg1 = env.initiate(11.into());
        let (msg2, _) = env.send_msg1_to_supplicant(msg1.keyframe(), 11.into());
        let msg3 = env.send_msg2_to_authenticator(msg2.keyframe(), 12.into());
        let (_, s_ptk, s_gtk) = env.send_msg3_to_supplicant(msg3.keyframe(), 12.into());

        // Replay third message pretending Authenticator did not receive Supplicant's response.
        let mut update_sink = UpdateSink::default();

        env.send_msg3_to_supplicant_capture_updates(msg3.keyframe(), 12.into(), &mut update_sink);
        let msg4 = test_util::expect_eapol_resp(&update_sink[..]);

        for update in update_sink {
            if let SecAssocUpdate::Key(_) = update {
                panic!("reinstalled key");
            }
        }

        // Let Authenticator process 4th message.
        let (a_ptk, a_gtk) = env.send_msg4_to_authenticator(msg4.keyframe(), 13.into());

        // Finally verify that Supplicant and Authenticator derived the same keys.
        assert_eq!(s_ptk, a_ptk);
        assert_eq!(s_gtk, a_gtk);
    }

    #[test]
    fn test_supplicant_replay_msg3_different_gtk() {
        let mut env = test_util::FourwayTestEnv::new(test_util::HandshakeKind::Wpa2, 1, 3);

        // Use arbitrarily chosen key_replay_counter.
        let msg1 = env.initiate(11.into());
        let anonce = msg1.keyframe().key_frame_fields.key_nonce.clone();
        let (msg2, _) = env.send_msg1_to_supplicant(msg1.keyframe(), 11.into());
        let msg3 = env.send_msg2_to_authenticator(msg2.keyframe(), 12.into());
        let (_, s_ptk, s_gtk) = env.send_msg3_to_supplicant(msg3.keyframe(), 12.into());

        // Replay third message pretending Authenticator did not receive Supplicant's response.
        // Modify GTK to simulate GTK rotation while 4-Way Handshake was in progress.
        let mut other_gtk = s_gtk.bytes.clone();
        other_gtk[0] ^= 0xFF;
        let msg3 = test_util::get_wpa2_4whs_msg3(&s_ptk, &anonce[..], &other_gtk[..], |msg3| {
            msg3.key_frame_fields.key_replay_counter.set(42);
        });
        let mut update_sink = UpdateSink::default();
        env.send_msg3_to_supplicant_capture_updates(msg3.keyframe(), 13.into(), &mut update_sink);

        // Ensure Supplicant rejected and dropped 3rd message without replying.
        assert_eq!(update_sink.len(), 0);
    }

    // First messages of 4-Way Handshake must carry a zeroed IV in all protocol versions.

    #[test]
    fn test_random_iv_msg1_v1() {
        let mut env = test_util::FourwayTestEnv::new(test_util::HandshakeKind::Wpa2, 1, 3);

        let msg1 = env.initiate(1.into());
        let mut buf = vec![];
        let mut msg1 = msg1.copy_keyframe_mut(&mut buf);
        msg1.eapol_fields.version = eapol::ProtocolVersion::IEEE802DOT1X2001;
        msg1.key_frame_fields.key_iv = [0xFFu8; 16];
        env.send_msg1_to_supplicant_expect_err(msg1, 1.into());
    }

    #[test]
    fn test_random_iv_msg1_v2() {
        let mut env = test_util::FourwayTestEnv::new(test_util::HandshakeKind::Wpa2, 1, 3);

        let msg1 = env.initiate(1.into());
        let mut buf = vec![];
        let mut msg1 = msg1.copy_keyframe_mut(&mut buf);
        msg1.eapol_fields.version = eapol::ProtocolVersion::IEEE802DOT1X2004;
        msg1.key_frame_fields.key_iv = [0xFFu8; 16];
        env.send_msg1_to_supplicant_expect_err(msg1, 1.into());
    }

    // EAPOL Key frames can carry a random IV in the third message of the 4-Way Handshake if
    // protocol version 1, 802.1X-2001, is used. All other protocol versions require a zeroed IV
    // for the third message of the handshake. Some vendors violate this requirement. For
    // compatibility, Fuchsia relaxes this requirement and allows random IVs with 802.1X-2004.

    #[test]
    fn test_random_iv_msg3_v2001() {
        let mut env = test_util::FourwayTestEnv::new(test_util::HandshakeKind::Wpa2, 1, 3);

        let msg1 = env.initiate(11.into());
        let (msg2, ptk) = env.send_msg1_to_supplicant(msg1.keyframe(), 11.into());
        let msg3 = env.send_msg2_to_authenticator(msg2.keyframe(), 12.into());
        let mut buf = vec![];
        let mut msg3 = msg3.copy_keyframe_mut(&mut buf);
        msg3.eapol_fields.version = eapol::ProtocolVersion::IEEE802DOT1X2001;
        msg3.key_frame_fields.key_iv = [0xFFu8; 16];
        env.finalize_key_frame(&mut msg3, Some(ptk.kck()));

        let (msg4, s_ptk, s_gtk) = env.send_msg3_to_supplicant(msg3, 12.into());
        let (a_ptk, a_gtk) = env.send_msg4_to_authenticator(msg4.keyframe(), 13.into());

        assert_eq!(s_ptk, a_ptk);
        assert_eq!(s_gtk, a_gtk);
    }

    #[test]
    fn test_random_iv_msg3_v2004() {
        let mut env = test_util::FourwayTestEnv::new(test_util::HandshakeKind::Wpa2, 1, 3);

        let msg1 = env.initiate(11.into());
        let (msg2, ptk) = env.send_msg1_to_supplicant(msg1.keyframe(), 11.into());
        let msg3 = env.send_msg2_to_authenticator(msg2.keyframe(), 12.into());
        let mut buf = vec![];
        let mut msg3 = msg3.copy_keyframe_mut(&mut buf);
        msg3.eapol_fields.version = eapol::ProtocolVersion::IEEE802DOT1X2004;
        msg3.key_frame_fields.key_iv = [0xFFu8; 16];
        env.finalize_key_frame(&mut msg3, Some(ptk.kck()));

        let (msg4, s_ptk, s_gtk) = env.send_msg3_to_supplicant(msg3, 12.into());
        let (a_ptk, a_gtk) = env.send_msg4_to_authenticator(msg4.keyframe(), 13.into());

        assert_eq!(s_ptk, a_ptk);
        assert_eq!(s_gtk, a_gtk);
    }

    #[test]
    fn test_zeroed_iv_msg3_v2004() {
        let mut env = test_util::FourwayTestEnv::new(test_util::HandshakeKind::Wpa2, 1, 3);

        let msg1 = env.initiate(11.into());
        let (msg2, ptk) = env.send_msg1_to_supplicant(msg1.keyframe(), 11.into());
        let msg3 = env.send_msg2_to_authenticator(msg2.keyframe(), 12.into());
        let mut buf = vec![];
        let mut msg3 = msg3.copy_keyframe_mut(&mut buf);
        msg3.eapol_fields.version = eapol::ProtocolVersion::IEEE802DOT1X2004;
        msg3.key_frame_fields.key_iv = [0u8; 16];
        env.finalize_key_frame(&mut msg3, Some(ptk.kck()));

        let (msg4, s_ptk, s_gtk) = env.send_msg3_to_supplicant(msg3, 12.into());
        let (a_ptk, a_gtk) = env.send_msg4_to_authenticator(msg4.keyframe(), 13.into());

        assert_eq!(s_ptk, a_ptk);
        assert_eq!(s_gtk, a_gtk);
    }

    #[test]
    fn test_random_iv_msg3_v2010() {
        let mut env = test_util::FourwayTestEnv::new(test_util::HandshakeKind::Wpa2, 1, 3);

        let msg1 = env.initiate(11.into());
        let (msg2, ptk) = env.send_msg1_to_supplicant(msg1.keyframe(), 11.into());
        let msg3 = env.send_msg2_to_authenticator(msg2.keyframe(), 12.into());
        let mut buf = vec![];
        let mut msg3 = msg3.copy_keyframe_mut(&mut buf);
        msg3.eapol_fields.version = eapol::ProtocolVersion::IEEE802DOT1X2010;
        msg3.key_frame_fields.key_iv = [0xFFu8; 16];
        env.finalize_key_frame(&mut msg3, Some(ptk.kck()));

        env.send_msg3_to_supplicant_expect_err(msg3, 12.into());
    }

    fn make_protection_info_with_mfp_parameters(
        mfp_bits: Option<(bool, bool)>, // Option<(mfpc, mfpr)>
        group_mgmt_cipher_suite: Option<Cipher>,
    ) -> ProtectionInfo {
        ProtectionInfo::Rsne(Rsne {
            rsn_capabilities: match mfp_bits {
                None => None,
                Some((mfpc, mfpr)) => Some(
                    RsnCapabilities(0)
                        .with_mgmt_frame_protection_cap(mfpc)
                        .with_mgmt_frame_protection_req(mfpr),
                ),
            },
            group_mgmt_cipher_suite,
            ..Default::default()
        })
    }

    fn check_rsne_get_group_mgmt_cipher(
        s_mfp_bits: Option<(bool, bool)>, // Option<(mfpc, mfpr)>
        s_cipher: Option<Cipher>,
        a_mfp_bits: Option<(bool, bool)>, // Option<(mfpc, mfpr)>
        a_cipher: Option<Cipher>,
        expected_result: Result<Option<Cipher>, Error>,
    ) {
        let s_protection_info = make_protection_info_with_mfp_parameters(s_mfp_bits, s_cipher);
        let a_protection_info = make_protection_info_with_mfp_parameters(a_mfp_bits, a_cipher);

        assert_eq!(get_group_mgmt_cipher(&s_protection_info, &a_protection_info), expected_result);
    }

    #[test]
    fn test_get_group_mgmt_cipher() {
        // Check that CIPHER_BIP_CMAC_256 is not DEFAULT_GROUP_MGMT_CIPHER so we can check cases when a
        // non-default cipher is specified.
        assert!(
            CIPHER_BIP_CMAC_256 != DEFAULT_GROUP_MGMT_CIPHER,
            "default group mgmt cipher is CIPHER_BIP_CMAC_256"
        );

        for (s_mfpr, a_mfpr) in vec![(false, false), (false, true), (true, false), (true, true)] {
            check_rsne_get_group_mgmt_cipher(
                Some((true, s_mfpr)),
                None,
                Some((true, a_mfpr)),
                None,
                Ok(Some(DEFAULT_GROUP_MGMT_CIPHER)),
            );
        }

        for (s_mfpr, a_mfpr) in vec![(false, false), (false, true), (true, false), (true, true)] {
            check_rsne_get_group_mgmt_cipher(
                Some((true, s_mfpr)),
                Some(CIPHER_BIP_CMAC_256),
                Some((true, a_mfpr)),
                Some(CIPHER_BIP_CMAC_256),
                Ok(Some(CIPHER_BIP_CMAC_256)),
            );
        }

        for (s_mfpc, a_mfpc) in vec![(false, false), (false, true), (true, false)] {
            check_rsne_get_group_mgmt_cipher(
                Some((s_mfpc, false)),
                Some(CIPHER_BIP_CMAC_128),
                Some((a_mfpc, false)),
                None,
                Ok(None),
            );
        }

        for (s_mfpc, a_mfpc) in vec![(false, false), (false, true), (true, false)] {
            check_rsne_get_group_mgmt_cipher(
                Some((s_mfpc, false)),
                None,
                Some((a_mfpc, false)),
                None,
                Ok(None),
            );
        }

        let s_protection_info = ProtectionInfo::LegacyWpa(fake_deprecated_wpa1_vendor_ie());
        let a_protection_info = make_protection_info_with_mfp_parameters(None, None);
        assert_eq!(get_group_mgmt_cipher(&s_protection_info, &a_protection_info), Ok(None));

        let s_protection_info = make_protection_info_with_mfp_parameters(None, None);
        let a_protection_info = ProtectionInfo::LegacyWpa(fake_deprecated_wpa1_vendor_ie());
        assert_eq!(get_group_mgmt_cipher(&s_protection_info, &a_protection_info), Ok(None));
    }

    #[test]
    fn test_get_group_mgmt_cipher_errors() {
        // Error::Invalid*MgmtFrameProtectionCapabilityBit
        check_rsne_get_group_mgmt_cipher(
            Some((false, true)),
            None,
            Some((false, false)),
            None,
            Err(Error::InvalidClientMgmtFrameProtectionCapabilityBit),
        );
        check_rsne_get_group_mgmt_cipher(
            Some((false, false)),
            None,
            Some((false, true)),
            None,
            Err(Error::InvalidApMgmtFrameProtectionCapabilityBit),
        );

        // Error::MgmtFrameProtectionRequiredByClient
        check_rsne_get_group_mgmt_cipher(
            Some((true, true)),
            None,
            Some((false, false)),
            None,
            Err(Error::MgmtFrameProtectionRequiredByClient),
        );
        check_rsne_get_group_mgmt_cipher(
            Some((true, true)),
            None,
            None,
            None,
            Err(Error::MgmtFrameProtectionRequiredByClient),
        );
        let s_protection_info = make_protection_info_with_mfp_parameters(Some((true, true)), None);
        let a_protection_info = ProtectionInfo::LegacyWpa(fake_deprecated_wpa1_vendor_ie());
        assert_eq!(
            get_group_mgmt_cipher(&s_protection_info, &a_protection_info),
            Err(Error::MgmtFrameProtectionRequiredByClient)
        );

        // Error::MgmtFrameProtectionRequiredByAp
        check_rsne_get_group_mgmt_cipher(
            Some((false, false)),
            None,
            Some((true, true)),
            None,
            Err(Error::MgmtFrameProtectionRequiredByAp),
        );
        check_rsne_get_group_mgmt_cipher(
            None,
            None,
            Some((true, true)),
            None,
            Err(Error::MgmtFrameProtectionRequiredByAp),
        );
        let s_protection_info = ProtectionInfo::LegacyWpa(fake_deprecated_wpa1_vendor_ie());
        let a_protection_info = make_protection_info_with_mfp_parameters(Some((true, true)), None);
        assert_eq!(
            get_group_mgmt_cipher(&s_protection_info, &a_protection_info),
            Err(Error::MgmtFrameProtectionRequiredByAp)
        );

        // Error::GroupMgmtCipherMismatch
        check_rsne_get_group_mgmt_cipher(
            Some((true, true)),
            Some(CIPHER_BIP_CMAC_128),
            Some((true, true)),
            Some(CIPHER_BIP_CMAC_256),
            Err(Error::GroupMgmtCipherMismatch(CIPHER_BIP_CMAC_128, CIPHER_BIP_CMAC_256)),
        );
        check_rsne_get_group_mgmt_cipher(
            Some((true, true)),
            Some(CIPHER_BIP_CMAC_256),
            Some((true, true)),
            None,
            Err(Error::GroupMgmtCipherMismatch(CIPHER_BIP_CMAC_256, DEFAULT_GROUP_MGMT_CIPHER)),
        );
    }
}
