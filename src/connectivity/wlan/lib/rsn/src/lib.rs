// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg_attr(feature = "benchmarks", feature(test))]

use thiserror::Error;

// TODO(hahnr): Limit exports and rearrange modules.

mod aes;
pub mod auth;
mod integrity;
pub mod key;
mod key_data;
mod keywrap;
pub mod nonce;
mod prf;
pub mod rsna;

use crate::aes::AesError;
use crate::key::exchange::handshake::{HandshakeMessageNumber, fourway, group_key};
use crate::key::exchange::{self};
use crate::rsna::esssa::EssSa;
use crate::rsna::{Role, UpdateSink};
use fidl_fuchsia_wlan_mlme::{EapolResultCode, SaeFrame};
use fuchsia_sync::Mutex;
use ieee80211::{MacAddr, Ssid};
use log::warn;
use std::sync::Arc;
use wlan_common::ie::rsn::cipher::Cipher;
use wlan_common::ie::rsn::rsne::{self, Rsne};
use wlan_common::ie::wpa::WpaIe;
use zerocopy::SplitByteSlice;

pub use crate::auth::psk;
pub use crate::key::Pmk;
pub use crate::key::gtk::{self, GtkProvider};
pub use crate::key::igtk::{self, IgtkProvider};
pub use crate::rsna::NegotiatedProtection;
pub use wlan_fcg_crypto::sae::PweMethod;

#[derive(Debug)]
pub struct Supplicant {
    auth_method: auth::Method,
    esssa: EssSa,
    pub auth_cfg: auth::Config,
}

/// Any information (i.e. info elements) used to negotiate protection on an RSN.
#[derive(Debug, Clone, PartialEq)]
pub enum ProtectionInfo {
    Rsne(Rsne),
    LegacyWpa(WpaIe),
}

fn extract_pmk_helper(update_sink: &UpdateSink) -> Option<Pmk> {
    for update in &update_sink[..] {
        if let rsna::SecAssocUpdate::Key(key::exchange::Key::Pmk(pmk)) = update {
            return Some(pmk.clone());
        }
    }
    None
}

impl Supplicant {
    /// WPA personal supplicant which supports 4-Way- and Group-Key Handshakes.
    pub fn new_wpa_personal(
        nonce_rdr: Arc<nonce::NonceReader>,
        auth_cfg: auth::Config,
        s_addr: MacAddr,
        s_protection: ProtectionInfo,
        a_addr: MacAddr,
        a_protection: ProtectionInfo,
        pmksa_caching_supported: bool,
    ) -> Result<Supplicant, anyhow::Error> {
        let negotiated_protection = NegotiatedProtection::from_protection(&s_protection)?;
        let gtk_exch_cfg = Some(exchange::Config::GroupKeyHandshake(group_key::Config {
            role: Role::Supplicant,
            protection: negotiated_protection.clone(),
        }));

        let auth_method = auth::Method::from_config(auth_cfg.clone())?;
        let pmk = match auth_cfg.clone() {
            auth::Config::ComputedPsk(psk) => Some(Pmk::from_pmk(psk.to_vec())),
            _ => None,
        };
        let esssa = EssSa::new(
            Role::Supplicant,
            pmk,
            negotiated_protection,
            exchange::Config::FourWayHandshake(fourway::Config::new(
                Role::Supplicant,
                s_addr,
                s_protection,
                a_addr,
                a_protection,
                nonce_rdr,
                None,
                None,
                pmksa_caching_supported,
            )?),
            gtk_exch_cfg,
        )?;

        Ok(Supplicant { auth_method, esssa, auth_cfg })
    }

    #[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
    pub fn start(&mut self, update_sink: &mut UpdateSink) -> Result<(), Error> {
        self.esssa.initiate(update_sink)
    }

    pub fn reset(&mut self) {
        // The replay counter must be reset so subsequent associations are not ignored.
        self.esssa.reset_replay_counter();
        self.esssa.reset_security_associations();
    }

    #[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
    pub fn on_eapol_frame<B: SplitByteSlice>(
        &mut self,
        update_sink: &mut UpdateSink,
        frame: eapol::Frame<B>,
    ) -> Result<(), Error> {
        self.esssa.on_eapol_frame(update_sink, frame)
    }

    #[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
    pub fn on_eapol_conf(
        &mut self,
        update_sink: &mut UpdateSink,
        result: EapolResultCode,
    ) -> Result<(), Error> {
        self.esssa.on_eapol_conf(update_sink, result)
    }

    #[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
    pub fn on_rsna_retransmission_timeout(
        &mut self,
        update_sink: &mut UpdateSink,
    ) -> Result<(), Error> {
        self.esssa.on_rsna_retransmission_timeout(update_sink)
    }

    /// Can be called at anytime to determine the reason why the RSNA
    /// is not complete. This is normally called when
    /// the higher layer, usually SME, determines establishing the
    /// RSNA failed, likely because of an expired timeout.
    pub fn incomplete_reason(&self) -> Error {
        self.esssa.incomplete_reason()
    }

    #[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
    fn extract_sae_key(&mut self, update_sink: &mut UpdateSink) -> Result<(), Error> {
        if let Some(pmk) = extract_pmk_helper(&update_sink) {
            self.esssa.on_pmk_available(update_sink, pmk)?;
        }
        Ok(())
    }

    #[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
    pub fn on_pmk_available(
        &mut self,
        update_sink: &mut UpdateSink,
        pmk: &[u8],
        pmkid: &[u8],
    ) -> Result<(), Error> {
        self.auth_method.on_pmk_available(pmk, pmkid, update_sink).map_err(Error::AuthError)?;
        self.extract_sae_key(update_sink)
    }

    #[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
    pub fn on_sae_handshake_ind(&mut self, update_sink: &mut UpdateSink) -> Result<(), Error> {
        self.auth_method.on_sae_handshake_ind(update_sink).map_err(Error::AuthError)
    }

    #[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
    pub fn on_sae_frame_rx(
        &mut self,
        update_sink: &mut UpdateSink,
        frame: SaeFrame,
    ) -> Result<(), Error> {
        self.auth_method.on_sae_frame_rx(update_sink, frame).map_err(Error::AuthError)?;
        self.extract_sae_key(update_sink)
    }

    #[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
    pub fn on_sae_timeout(
        &mut self,
        update_sink: &mut UpdateSink,
        event_id: u64,
    ) -> Result<(), Error> {
        self.auth_method.on_sae_timeout(update_sink, event_id).map_err(Error::AuthError)
    }

    #[allow(clippy::result_large_err, reason = "using existing Error type")]
    pub fn initiate_owe(&mut self, update_sink: &mut UpdateSink) -> Result<(), Error> {
        self.auth_method.initiate_owe(update_sink).map_err(Error::AuthError)
    }

    #[allow(clippy::result_large_err, reason = "using existing Error type")]
    pub fn on_owe_public_key_rx(
        &mut self,
        update_sink: &mut UpdateSink,
        group: u16,
        public_key: Vec<u8>,
    ) -> Result<(), Error> {
        self.auth_method
            .on_owe_public_key_rx(update_sink, group, public_key)
            .map_err(Error::AuthError)?;
        if let Some(pmk) = extract_pmk_helper(&update_sink) {
            self.esssa.on_pmk_available(update_sink, pmk)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct Authenticator {
    auth_method: auth::Method,
    esssa: EssSa,
    pub auth_cfg: auth::Config,
}

impl Authenticator {
    /// WPA2-PSK CCMP-128 Authenticator which supports 4-Way Handshake.
    /// The Authenticator does not support GTK rotations.
    pub fn new_wpa2psk_ccmp128(
        nonce_rdr: Arc<nonce::NonceReader>,
        gtk_provider: Arc<Mutex<gtk::GtkProvider>>,
        psk: psk::Psk,
        s_addr: MacAddr,
        s_protection: ProtectionInfo,
        a_addr: MacAddr,
        a_protection: ProtectionInfo,
    ) -> Result<Authenticator, anyhow::Error> {
        let negotiated_protection = NegotiatedProtection::from_protection(&s_protection)?;
        let auth_cfg = auth::Config::ComputedPsk(psk.clone());
        let auth_method = auth::Method::from_config(auth_cfg.clone())?;
        let esssa = EssSa::new(
            Role::Authenticator,
            Some(Pmk::from_pmk(psk.to_vec())),
            negotiated_protection,
            exchange::Config::FourWayHandshake(fourway::Config::new(
                Role::Authenticator,
                s_addr,
                s_protection,
                a_addr,
                a_protection,
                nonce_rdr,
                Some(gtk_provider),
                None,
                false,
            )?),
            // Group-Key Handshake does not support Authenticator role yet.
            None,
        )?;

        Ok(Authenticator { auth_method, esssa, auth_cfg })
    }

    /// WPA3 Authenticator which supports 4-Way Handshake.
    /// The Authenticator does not support GTK rotations.
    pub fn new_wpa3(
        nonce_rdr: Arc<nonce::NonceReader>,
        gtk_provider: Arc<Mutex<gtk::GtkProvider>>,
        igtk_provider: Arc<Mutex<igtk::IgtkProvider>>,
        ssid: Ssid,
        password: Vec<u8>,
        s_addr: MacAddr,
        s_protection: ProtectionInfo,
        a_addr: MacAddr,
        a_protection: ProtectionInfo,
    ) -> Result<Authenticator, anyhow::Error> {
        let negotiated_protection = NegotiatedProtection::from_protection(&s_protection)?;
        let auth_cfg = auth::Config::Sae {
            ssid,
            password,
            mac: a_addr.clone(),
            peer_mac: s_addr.clone(),
            pwe_method: PweMethod::Loop,
        };
        let auth_method = auth::Method::from_config(auth_cfg.clone())?;

        let esssa = EssSa::new(
            Role::Authenticator,
            None,
            negotiated_protection,
            exchange::Config::FourWayHandshake(fourway::Config::new(
                Role::Authenticator,
                s_addr,
                s_protection,
                a_addr,
                a_protection,
                nonce_rdr,
                Some(gtk_provider),
                Some(igtk_provider),
                false,
            )?),
            // Group-Key Handshake does not support Authenticator role yet.
            None,
        )?;

        Ok(Authenticator { auth_cfg, esssa, auth_method })
    }

    pub fn get_negotiated_protection(&self) -> &NegotiatedProtection {
        &self.esssa.negotiated_protection
    }

    /// Resets all established Security Associations and invalidates all derived keys in this ESSSA.
    /// The Authenticator must be reset or destroyed when the underlying 802.11 association
    /// terminates. The replay counter is also reset.
    pub fn reset(&mut self) {
        self.esssa.reset_replay_counter();
        self.esssa.reset_security_associations();

        // Recreate auth_method to reset its state
        match auth::Method::from_config(self.auth_cfg.clone()) {
            Ok(auth_method) => self.auth_method = auth_method,
            Err(e) => warn!("Unable to recreate auth::Method: {}", e),
        }
    }

    #[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
    /// `initiate(...)` must be called when the Authenticator should start establishing a
    /// security association with a client.
    /// The Authenticator must always initiate the security association in the current system as
    /// EAPOL request frames from clients are not yet supported.
    /// This method can be called multiple times to re-initiate the security association, however,
    /// calling this method will invalidate all established security associations and their derived
    /// keys.
    pub fn initiate(&mut self, update_sink: &mut UpdateSink) -> Result<(), Error> {
        self.esssa.initiate(update_sink)
    }

    #[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
    /// Entry point for all incoming EAPOL frames. Incoming frames can be corrupted, invalid or of
    /// unsupported types; the Authenticator will filter and drop all unexpected frames.
    /// Outbound EAPOL frames, status and key updates will be pushed into the `update_sink`.
    /// The method will return an `Error` if the frame was invalid.
    pub fn on_eapol_frame<B: SplitByteSlice>(
        &mut self,
        update_sink: &mut UpdateSink,
        frame: eapol::Frame<B>,
    ) -> Result<(), Error> {
        self.esssa.on_eapol_frame(update_sink, frame)
    }

    #[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
    pub fn on_eapol_conf(
        &mut self,
        update_sink: &mut UpdateSink,
        result: EapolResultCode,
    ) -> Result<(), Error> {
        self.esssa.on_eapol_conf(update_sink, result)
    }

    #[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
    fn extract_sae_key(&mut self, update_sink: &mut UpdateSink) -> Result<(), Error> {
        if let Some(pmk) = extract_pmk_helper(&update_sink) {
            self.esssa.on_pmk_available(update_sink, pmk)?;
        }
        Ok(())
    }

    #[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
    pub fn on_sae_handshake_ind(&mut self, update_sink: &mut UpdateSink) -> Result<(), Error> {
        self.auth_method.on_sae_handshake_ind(update_sink).map_err(Error::AuthError)
    }

    #[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
    pub fn on_sae_frame_rx(
        &mut self,
        update_sink: &mut UpdateSink,
        frame: SaeFrame,
    ) -> Result<(), Error> {
        self.auth_method.on_sae_frame_rx(update_sink, frame).map_err(Error::AuthError)?;
        self.extract_sae_key(update_sink)
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid OUI length; expected 3 bytes but received {}", _0)]
    InvalidOuiLength(usize),
    #[error("invalid PMKID length; expected 16 bytes but received {}", _0)]
    InvalidPmkidLength(usize),
    #[error("invalid passphrase length: {}", _0)]
    InvalidPassphraseLen(usize),
    #[error("passphrase is not valid UTF-8; failed to parse after byte at index: {:x}", _0)]
    InvalidPassphraseEncoding(usize),
    #[error("the config `{:?}` is incompatible with the auth method `{:?}`", _0, _1)]
    IncompatibleConfig(auth::Config, String),
    #[error("invalid bit size; must be a multiple of 8 but was {}", _0)]
    InvalidBitSize(usize),
    #[error("nonce could not be generated")]
    NonceError,
    #[error("error deriving PTK; invalid PMK")]
    PtkHierarchyInvalidPmkError,
    #[error("error deriving PTK; unsupported AKM suite")]
    PtkHierarchyUnsupportedAkmError,
    #[error("error deriving PTK; unsupported cipher suite")]
    PtkHierarchyUnsupportedCipherError,
    #[error("error deriving GTK; unsupported cipher suite")]
    GtkHierarchyUnsupportedCipherError,
    #[error("error deriving IGTK; unsupported cipher suite")]
    IgtkHierarchyUnsupportedCipherError,
    #[error("no GtkProvider for Authenticator")]
    MissingGtkProvider,
    #[error("no IgtkProvider for Authenticator with Management Frame Protection support")]
    MissingIgtkProvider,
    #[error("invalid supplicant protection: {}", _0)]
    InvalidSupplicantProtection(String),
    #[error("required group mgmt cipher does not match IgtkProvider cipher: {:?} != {:?}", _0, _1)]
    WrongIgtkProviderCipher(Cipher, Cipher),
    #[error("error determining group mgmt cipher: {:?} != {:?}", _0, _1)]
    GroupMgmtCipherMismatch(Cipher, Cipher),
    #[error("client requires management frame protection and ap is not capable")]
    MgmtFrameProtectionRequiredByClient,
    #[error("ap requires management frame protection and client is not capable")]
    MgmtFrameProtectionRequiredByAp,
    #[error("client set MFP required bit without setting MFP capability bit")]
    InvalidClientMgmtFrameProtectionCapabilityBit,
    #[error("ap set MFP required bit without setting MFP capability bit")]
    InvalidApMgmtFrameProtectionCapabilityBit,
    #[error("AES operation failed: {}", _0)]
    Aes(AesError),
    #[error("invalid key data length; must be at least 16 bytes and a multiple of 8: {}", _0)]
    InvaidKeyDataLength(usize),
    #[error("invalid key data; error code: {:?}", _0)]
    InvalidKeyData(nom::error::ErrorKind),
    #[error("unknown authentication method")]
    UnknownAuthenticationMethod,
    #[error("no AKM negotiated")]
    InvalidNegotiatedAkm,
    #[error("unknown key exchange method")]
    UnknownKeyExchange,
    #[error("cannot initiate Fourway Handshake as Supplicant")]
    UnexpectedInitiationRequest,
    #[error("cannot initiate Supplicant in current EssSa state")]
    UnexpectedEsssaInitiation,
    #[error("key frame transmission failed")]
    KeyFrameTransmissionFailed,
    #[error("no key frame transmission confirm received; dropped {} pending updates", _0)]
    NoKeyFrameTransmissionConfirm(usize),
    #[error("eapol handshake not started")]
    EapolHandshakeNotStarted,
    #[error("likely wrong credential")]
    LikelyWrongCredential,
    #[error("eapol handshake incomplete: {}", _0)]
    EapolHandshakeIncomplete(String),
    #[error("unsupported Key Descriptor Type: {:?}", _0)]
    UnsupportedKeyDescriptor(eapol::KeyDescriptor),
    #[error("unexpected Key Descriptor Type {:?}; expected {:?}", _0, _1)]
    InvalidKeyDescriptor(eapol::KeyDescriptor, eapol::KeyDescriptor),
    #[error("unsupported Key Descriptor Version: {:?}", _0)]
    UnsupportedKeyDescriptorVersion(u16),
    #[error("only PTK and GTK derivation is supported")]
    UnsupportedKeyDerivation,
    #[error("unexpected message: {:?}", _0)]
    UnexpectedHandshakeMessage(HandshakeMessageNumber),
    #[error("invalid install bit value; message: {:?}", _0)]
    InvalidInstallBitValue(HandshakeMessageNumber),
    #[error("error, install bit set for Group-/SMK-Handshake")]
    InvalidInstallBitGroupSmkHandshake,
    #[error("invalid key_ack bit value; message: {:?}", _0)]
    InvalidKeyAckBitValue(HandshakeMessageNumber),
    #[error("invalid key_mic bit value; message: {:?}", _0)]
    InvalidKeyMicBitValue(HandshakeMessageNumber),
    #[error("invalid secure bit value; message: {:?}", _0)]
    InvalidSecureBitValue(HandshakeMessageNumber),
    #[error("error, secure bit set by Authenticator before PTK is known")]
    SecureBitWithUnknownPtk,
    #[error("error, secure bit set must be set by Supplicant once PTK and GTK are known")]
    SecureBitNotSetWithKnownPtkGtk,
    #[error("invalid error bit value; message: {:?}", _0)]
    InvalidErrorBitValue(HandshakeMessageNumber),
    #[error("invalid request bit value; message: {:?}", _0)]
    InvalidRequestBitValue(HandshakeMessageNumber),
    #[error("error, Authenticator set request bit")]
    InvalidRequestBitAuthenticator,
    #[error("error, Authenticator set error bit")]
    InvalidErrorBitAuthenticator,
    #[error("error, Supplicant set key_ack bit")]
    InvalidKeyAckBitSupplicant,
    #[error("invalid encrypted_key_data bit value")]
    InvalidEncryptedKeyDataBitValue(HandshakeMessageNumber),
    #[error("encrypted_key_data bit requires MIC bit to be set")]
    InvalidMicBitForEncryptedKeyData,
    #[error("invalid key length {:?}; expected {:?}", _0, _1)]
    InvalidKeyLength(usize, usize),
    #[error("unsupported cipher suite")]
    UnsupportedCipherSuite,
    #[error("unsupported AKM suite")]
    UnsupportedAkmSuite,
    #[error("cannot compute MIC for key frames which haven't set their MIC bit")]
    ComputingMicForUnprotectedFrame,
    #[error("cannot compute MIC; error while encrypting")]
    ComputingMicEncryptionError,
    #[error("the key frame's MIC size ({}) differes from the expected size: {}", _0, _1)]
    MicSizesDiffer(usize, usize),
    #[error("invalid MIC size")]
    InvalidMicSize,
    #[error("invalid Nonce; expected to be non-zero")]
    InvalidNonce(HandshakeMessageNumber),
    #[error("invalid RSC; expected to be zero")]
    InvalidRsc(HandshakeMessageNumber),
    #[error("invalid key data; must not be zero")]
    EmptyKeyData(HandshakeMessageNumber),
    #[error("invalid key data")]
    InvalidKeyDataContent,
    #[error("invalid key data length; doesn't match with key data")]
    InvalidKeyDataLength,
    #[error("cannot validate MIC; PTK not yet derived")]
    UnexpectedMic,
    #[error("invalid MIC")]
    InvalidMic,
    #[error("cannot decrypt key data; PTK not yet derived")]
    UnexpectedEncryptedKeyData,
    #[error("invalid key replay counter {:?}; expected counter to be > {:?}", _0, _1)]
    InvalidKeyReplayCounter(u64, u64),
    #[error("invalid nonce; nonce must match nonce from 1st message")]
    ErrorNonceDoesntMatch,
    #[error("invalid IV; EAPOL protocol version: {:?}; message: {:?}", _0, _1)]
    InvalidIv(eapol::ProtocolVersion, HandshakeMessageNumber),
    #[error("PMKSA was not yet established")]
    PmksaNotEstablished,
    #[error("invalid nonce size; expected 32 bytes, found: {:?}", _0)]
    InvalidNonceSize(usize),
    #[error("invalid key data; expected negotiated protection")]
    InvalidKeyDataProtection,
    #[error("buffer too small; required: {}, available: {}", _0, _1)]
    BufferTooSmall(usize, usize),
    #[error("error, SMK-Handshake is not supported")]
    SmkHandshakeNotSupported,
    #[error("error, negotiated protection is invalid")]
    InvalidNegotiatedProtection,
    #[error("unknown integrity algorithm for negotiated protection")]
    UnknownIntegrityAlgorithm,
    #[error("unknown keywrap algorithm for negotiated protection")]
    UnknownKeywrapAlgorithm,
    #[error("eapol error, {}", _0)]
    EapolError(eapol::Error),
    #[error("auth error, {}", _0)]
    AuthError(auth::AuthError),
    #[error("rsne error, {}", _0)]
    RsneError(rsne::Error),
    #[error("rsne invalid subset, supplicant: {:?}, authenticator: {:?}", _0, _1)]
    RsneInvalidSubset(rsne::Rsne, rsne::Rsne),
    #[error("error, {}", _0)]
    GenericError(String),
}

impl PartialEq for Error {
    fn eq(&self, other: &Self) -> bool {
        format!("{:?}", self) == format!("{:?}", other)
    }
}
impl Eq for Error {}

impl From<AesError> for Error {
    fn from(error: AesError) -> Self {
        Error::Aes(error)
    }
}

#[macro_export]
macro_rules! rsn_ensure {
    ($cond:expr, $err:literal) => {
        if !$cond {
            return std::result::Result::Err(Error::GenericError($err.to_string()));
        }
    };
    ($cond:expr, $err:expr $(,)?) => {
        if !$cond {
            return std::result::Result::Err($err);
        }
    };
}

#[macro_export]
macro_rules! format_rsn_err {
    ($msg:literal $(,)?) => {
        // Handle $:literal as a special case to make cargo-expanded code more
        // concise in the common case.
        Error::GenericError($msg.to_string())
    };
    ($err:expr $(,)?) => ({
        Error::GenericError($err)
    });
    ($fmt:expr, $($arg:tt)*) => {
        Error::GenericError(format!($fmt, $($arg)*))
    };
}

impl From<eapol::Error> for Error {
    fn from(e: eapol::Error) -> Self {
        Error::EapolError(e)
    }
}

impl From<auth::AuthError> for Error {
    fn from(e: auth::AuthError) -> Self {
        Error::AuthError(e)
    }
}

impl From<rsne::Error> for Error {
    fn from(e: rsne::Error) -> Self {
        Error::RsneError(e)
    }
}

#[cfg(test)]
mod tests {
    use crate::key::exchange::Key;
    use crate::rsna::{SecAssocStatus, SecAssocUpdate, test_util};
    use crate::{Pmk, key_data};
    use assert_matches::assert_matches;
    use test_case::test_case;

    #[test]
    fn supplicant_extract_sae_key() {
        let mut supplicant = test_util::get_wpa3_supplicant();
        let mut dummy_update_sink = vec![
            SecAssocUpdate::ScheduleSaeTimeout(123),
            SecAssocUpdate::Key(Key::Pmk(vec![1, 2, 3, 4, 5, 6, 7, 8].into())),
        ];
        supplicant.extract_sae_key(&mut dummy_update_sink).expect("Failed to extract key");
        // ESSSA should register the new PMK and report this.
        assert_eq!(
            dummy_update_sink,
            vec![
                SecAssocUpdate::ScheduleSaeTimeout(123),
                SecAssocUpdate::Key(Key::Pmk(vec![1, 2, 3, 4, 5, 6, 7, 8].into())),
                SecAssocUpdate::Status(SecAssocStatus::PmkSaEstablished),
            ]
        );
    }

    #[test]
    fn supplicant_extract_sae_key_no_key() {
        let mut supplicant = test_util::get_wpa3_supplicant();
        let mut dummy_update_sink = vec![SecAssocUpdate::ScheduleSaeTimeout(123)];
        supplicant.extract_sae_key(&mut dummy_update_sink).expect("Failed to extract key");
        // No PMK means no new update.
        assert_eq!(dummy_update_sink, vec![SecAssocUpdate::ScheduleSaeTimeout(123)]);
    }

    #[test]
    fn authenticator_extract_sae_key() {
        let mut authenticator = test_util::get_wpa3_authenticator();
        let mut dummy_update_sink = vec![
            SecAssocUpdate::ScheduleSaeTimeout(123),
            SecAssocUpdate::Key(Key::Pmk(vec![1, 2, 3, 4, 5, 6, 7, 8].into())),
        ];
        authenticator.extract_sae_key(&mut dummy_update_sink).expect("Failed to extract key");
        // ESSSA should register the new PMK and report this.
        assert_eq!(
            &dummy_update_sink[0..3],
            vec![
                SecAssocUpdate::ScheduleSaeTimeout(123),
                SecAssocUpdate::Key(Key::Pmk(vec![1, 2, 3, 4, 5, 6, 7, 8].into())),
                SecAssocUpdate::Status(SecAssocStatus::PmkSaEstablished),
            ]
            .as_slice(),
        );

        // ESSSA should also transmit an EAPOL frame since this is the Authenticator.
        assert_matches!(&dummy_update_sink[3], &SecAssocUpdate::TxEapolKeyFrame { .. });
    }

    #[test]
    fn authenticator_extract_sae_key_no_key() {
        let mut authenticator = test_util::get_wpa3_authenticator();
        let mut dummy_update_sink = vec![SecAssocUpdate::ScheduleSaeTimeout(123)];
        authenticator.extract_sae_key(&mut dummy_update_sink).expect("Failed to extract key");
        // No PMK means no new update.
        assert_eq!(dummy_update_sink, vec![SecAssocUpdate::ScheduleSaeTimeout(123)]);
    }

    #[test]
    fn supplicant_initiate_owe_and_handle_public_key() {
        let mut supplicant = test_util::get_owe_supplicant();
        let mut update_sink = vec![];
        supplicant.initiate_owe(&mut update_sink).expect("Failed to initiate OWE");
        assert_eq!(update_sink.len(), 1);
        let (group_id, key) = assert_matches!(update_sink.remove(0), SecAssocUpdate::TxOwePublicKey { group_id, key } => (group_id, key));
        assert_eq!(group_id, 19);
        assert!(!key.is_empty());

        const AP_PUBLIC_KEY: [u8; 32] = [
            0xa9, 0x8c, 0x47, 0xc5, 0xbd, 0xcf, 0x1d, 0x5e, 0x2c, 0x3c, 0x95, 0x8e, 0x10, 0xf3,
            0x71, 0x61, 0xc4, 0x61, 0x02, 0x13, 0x22, 0xb2, 0x95, 0xf6, 0xc7, 0x81, 0x1e, 0xf8,
            0x14, 0xc6, 0x03, 0x17,
        ];
        supplicant
            .on_owe_public_key_rx(&mut update_sink, group_id, AP_PUBLIC_KEY.to_vec())
            .expect("Failed to handle OWE public key");
        // After handling the AP's public key, the supplicant should derive the PMK and
        // notify the ESSSA.
        assert_eq!(update_sink.len(), 2);
        let pmk = assert_matches!(update_sink.remove(0), SecAssocUpdate::Key(Key::Pmk(pmk)) => pmk);
        assert!(!pmk.pmk.is_empty());
        assert_eq!(update_sink.remove(0), SecAssocUpdate::Status(SecAssocStatus::PmkSaEstablished));
    }

    #[test_case(
        vec![
            0xa9, 0x8c, 0x47, 0xc5, 0xbd, 0xcf, 0x1d, 0x5e, 0x2c, 0x3c, 0x95, 0x8e, 0x10, 0xf3,
            0x71, 0x61, 0xc4, 0x61, 0x02, 0x13, 0x22, 0xb2, 0x95, 0xf6, 0xc7, 0x81, 0x1e, 0xf8,
            0x14, 0xc6, 0x03,
        ];
        "invalid key size"
    )]
    #[test_case(
        vec![
            0xa9, 0x8c, 0x47, 0xc5, 0xbd, 0xcf, 0x1d, 0x5e, 0x2c, 0x3c, 0x95, 0x8e, 0x10, 0xf3,
            0x71, 0x61, 0xc4, 0x61, 0x02, 0x13, 0x22, 0xb2, 0x95, 0xf6, 0xc7, 0x81, 0x1e, 0xf8,
            0x14, 0xc6, 0x03, 0x16,
        ];
        "not a valid point on curve"
    )]
    #[fuchsia::test(add_test_attr = false)]
    fn supplicant_handle_public_key_wrong_key_size(public_key: Vec<u8>) {
        let mut supplicant = test_util::get_owe_supplicant();
        let mut update_sink = vec![];
        supplicant.initiate_owe(&mut update_sink).expect("Failed to initiate OWE");
        let (group_id, _key) = assert_matches!(update_sink.remove(0), SecAssocUpdate::TxOwePublicKey { group_id, key } => (group_id, key));

        supplicant
            .on_owe_public_key_rx(&mut update_sink, group_id, public_key)
            .expect_err("Should fail due to invalid public key");
    }

    #[test_case(true; "pmksa caching supported")]
    #[test_case(false; "pmksa caching not supported")]
    #[fuchsia::test(add_test_attr = false)]
    fn supplicant_driver_sae_on_pmk_available(pmksa_caching_supported: bool) {
        let mut supplicant =
            test_util::get_driver_sae_supplicant_with_pmksa_caching(pmksa_caching_supported);
        let mut update_sink = vec![];
        let pmk = vec![0x11; 32];
        let pmkid = vec![0x22; 16];
        supplicant
            .on_pmk_available(&mut update_sink, &pmk, &pmkid)
            .expect("Failed to process OnPmkAvailable");
        assert_eq!(update_sink.len(), 2);
        assert_eq!(
            update_sink.remove(0),
            SecAssocUpdate::Key(Key::Pmk(Pmk::new(pmk, Some(pmkid.clone()))))
        );
        assert_eq!(update_sink.remove(0), SecAssocUpdate::Status(SecAssocStatus::PmkSaEstablished));

        let anonce = [0xaa; 32];
        let msg1 = test_util::get_wpa3_4whs_msg1(&anonce[..]);
        let msg1_frame = eapol::Frame::Key(msg1.keyframe());
        supplicant
            .on_eapol_frame(&mut update_sink, msg1_frame)
            .expect("Failed to process EAPOL Msg 1");
        let msg2 = test_util::expect_eapol_resp(&update_sink[..]);
        let raw_key_data = &msg2.keyframe().key_data[..];
        let elements =
            key_data::extract_elements(raw_key_data).expect("Failed to extract key data");
        let rsne = elements
            .into_iter()
            .find_map(|e| match e {
                key_data::Element::Rsne(rsne) => Some(rsne),
                _ => None,
            })
            .expect("RSNE missing in Msg 2");
        let expected_pmkids = if pmksa_caching_supported {
            vec![bytes::Bytes::copy_from_slice(&pmkid)]
        } else {
            vec![]
        };
        assert_eq!(rsne.pmkids, expected_pmkids);
    }
}
