// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;
use crate::key::Pmk;
use crate::key::exchange::handshake::fourway::{self, Fourway, SupplicantKeyReplayCounter};
use crate::key::exchange::{compute_mic, compute_mic_from_buf};
use crate::key::gtk::{Gtk, GtkProvider};
use crate::key::igtk::{Igtk, IgtkProvider};
use crate::key::ptk::Ptk;
use crate::key_data::kde;
use crate::nonce::NonceReader;
use crate::{Authenticator, Supplicant, auth, psk};
use eapol::KeyFrameTx;
use fuchsia_sync::Mutex;
use hex::FromHex;
use ieee80211::{MacAddr, Ssid};
use std::sync::{Arc, LazyLock};
use wlan_common::ie::rsn::cipher::{self};
use wlan_common::ie::rsn::suite_filter::DEFAULT_GROUP_MGMT_CIPHER;
use wlan_common::ie::rsn::suite_selector::OUI;
use wlan_common::ie::rsn::{
    akm, fake_wpa2_a_rsne, fake_wpa2_s_rsne, fake_wpa3_a_rsne, fake_wpa3_s_rsne,
};
use wlan_common::ie::{fake_wpa_ie, write_wpa1_ie};
use wlan_common::organization::Oui;
use wlan_fcg_crypto::sae::PweMethod;

static S_ADDR: LazyLock<MacAddr> =
    LazyLock::new(|| MacAddr::from([0x81, 0x76, 0x61, 0x14, 0xDF, 0xC9]));
static A_ADDR: LazyLock<MacAddr> =
    LazyLock::new(|| MacAddr::from([0x1D, 0xE3, 0xFD, 0xDF, 0xCB, 0xD3]));
static PMK: LazyLock<Box<[u8]>> = LazyLock::new(|| {
    Vec::from_hex("0dc0d6eb90555ed6419756b9a15ec3e3209b63df707dd508d14581f8982721af")
        .expect("error reading PMK from hex")
        .into_boxed_slice()
});

pub fn get_rsne_protection() -> NegotiatedProtection {
    NegotiatedProtection::from_rsne(&fake_wpa2_s_rsne())
        .expect("error creating RSNE NegotiatedProtection")
}

pub fn get_wpa2_supplicant() -> Supplicant {
    let nonce_rdr = NonceReader::new(&S_ADDR).expect("error creating Reader");
    let psk = psk::compute("ThisIsAPassword".as_bytes(), &Ssid::try_from("ThisIsASSID").unwrap())
        .expect("error computing PSK");
    Supplicant::new_wpa_personal(
        nonce_rdr,
        auth::Config::ComputedPsk(psk),
        *S_ADDR,
        ProtectionInfo::Rsne(fake_wpa2_s_rsne()),
        *A_ADDR,
        ProtectionInfo::Rsne(fake_wpa2_a_rsne()),
        false,
    )
    .expect("could not create Supplicant")
}

pub fn get_wpa2_authenticator() -> Authenticator {
    let gtk_provider = GtkProvider::new(Cipher { oui: OUI, suite_type: cipher::CCMP_128 }, 1, 0)
        .expect("error creating GtkProvider");
    let nonce_rdr = NonceReader::new(&S_ADDR).expect("error creating Reader");
    let psk = psk::compute("ThisIsAPassword".as_bytes(), &Ssid::try_from("ThisIsASSID").unwrap())
        .expect("error computing PSK");
    Authenticator::new_wpa2psk_ccmp128(
        nonce_rdr,
        Arc::new(Mutex::new(gtk_provider)),
        psk,
        *S_ADDR,
        ProtectionInfo::Rsne(fake_wpa2_s_rsne()),
        *A_ADDR,
        ProtectionInfo::Rsne(fake_wpa2_a_rsne()),
    )
    .expect("could not create Authenticator")
}

pub fn get_wpa3_supplicant() -> Supplicant {
    let nonce_rdr = NonceReader::new(&S_ADDR).expect("error creating Reader");
    Supplicant::new_wpa_personal(
        nonce_rdr,
        auth::Config::Sae {
            ssid: Ssid::try_from("ThisIsASSID").unwrap(),
            password: "ThisIsAPassword".as_bytes().to_vec(),
            mac: *S_ADDR,
            peer_mac: *A_ADDR,
            pwe_method: PweMethod::Loop,
        },
        *S_ADDR,
        ProtectionInfo::Rsne(fake_wpa3_s_rsne()),
        *A_ADDR,
        ProtectionInfo::Rsne(fake_wpa3_a_rsne()),
        false,
    )
    .expect("could not create Supplicant")
}

pub fn get_wpa3_authenticator() -> Authenticator {
    let gtk_provider = GtkProvider::new(Cipher { oui: OUI, suite_type: cipher::CCMP_128 }, 1, 0)
        .expect("error creating GtkProvider");
    let igtk_provider =
        IgtkProvider::new(DEFAULT_GROUP_MGMT_CIPHER).expect("error creating IgtkProvider");
    let nonce_rdr = NonceReader::new(&S_ADDR).expect("error creating Reader");
    let ssid = Ssid::try_from("ThisIsASSID").unwrap();
    let password = "ThisIsAPassword".as_bytes().to_vec();
    Authenticator::new_wpa3(
        nonce_rdr,
        Arc::new(Mutex::new(gtk_provider)),
        Arc::new(Mutex::new(igtk_provider)),
        ssid,
        password,
        *S_ADDR,
        ProtectionInfo::Rsne(fake_wpa3_s_rsne()),
        *A_ADDR,
        ProtectionInfo::Rsne(fake_wpa3_a_rsne()),
    )
    .expect("could not create Authenticator")
}

pub fn get_owe_supplicant() -> Supplicant {
    let nonce_rdr = NonceReader::new(&S_ADDR).expect("error creating Reader");
    Supplicant::new_wpa_personal(
        nonce_rdr,
        auth::Config::Owe,
        *S_ADDR,
        ProtectionInfo::Rsne(Rsne::common_owe_rsne()),
        *A_ADDR,
        ProtectionInfo::Rsne(Rsne::common_owe_rsne()),
        false,
    )
    .expect("could not create Supplicant")
}

pub fn get_driver_sae_supplicant() -> Supplicant {
    get_driver_sae_supplicant_with_pmksa_caching(true)
}

pub fn get_driver_sae_supplicant_with_pmksa_caching(pmksa_caching_supported: bool) -> Supplicant {
    let nonce_rdr = NonceReader::new(&S_ADDR).expect("error creating Reader");
    Supplicant::new_wpa_personal(
        nonce_rdr,
        auth::Config::DriverSae { password: "ThisIsAPassword".as_bytes().to_vec() },
        *S_ADDR,
        ProtectionInfo::Rsne(fake_wpa3_s_rsne()),
        *A_ADDR,
        ProtectionInfo::Rsne(fake_wpa3_a_rsne()),
        pmksa_caching_supported,
    )
    .expect("could not create Supplicant")
}

pub fn get_wpa1_protection() -> NegotiatedProtection {
    NegotiatedProtection::from_legacy_wpa(&fake_wpa_ie())
        .expect("error creating WPA1 NegotiatedProtection")
}

pub fn get_wpa3_protection() -> NegotiatedProtection {
    NegotiatedProtection::from_rsne(&fake_wpa3_s_rsne())
        .expect("error creating WPA3 NegotiatedProtection")
}

pub fn get_ptk(anonce: &[u8], snonce: &[u8]) -> Ptk {
    let akm = get_akm();
    let s_rsne = fake_wpa2_s_rsne();
    let cipher = s_rsne
        .pairwise_cipher_suites
        .get(0)
        .expect("Supplicant's RSNE holds no Pairwise Cipher suite");
    Ptk::new(&PMK[..], &A_ADDR, &S_ADDR, anonce, snonce, &akm, cipher.clone())
        .expect("error deriving PTK")
}

pub fn get_wpa3_ptk(anonce: &[u8], snonce: &[u8]) -> Ptk {
    let s_rsne = fake_wpa3_s_rsne();
    let akm = &s_rsne.akm_suites[0];
    let cipher = s_rsne
        .pairwise_cipher_suites
        .get(0)
        .expect("Supplicant's RSNE holds no Pairwise Cipher suite");
    Ptk::new(&PMK[..], &A_ADDR, &S_ADDR, anonce, snonce, &akm, cipher.clone())
        .expect("error deriving PTK")
}

pub fn get_wpa1_ptk(anonce: &[u8], snonce: &[u8]) -> Ptk {
    let wpa = fake_wpa_ie();
    let akm = wpa.akm_list.get(0).expect("WPA1 IE holds no AKM");
    let cipher = wpa.unicast_cipher_list.get(0).expect("WPA1 IE holds no unicast cipher");
    Ptk::new(&PMK[..], &A_ADDR, &S_ADDR, anonce, snonce, &akm, cipher.clone())
        .expect("error deriving PTK")
}

pub fn get_wpa1_4whs_msg1(anonce: &[u8]) -> eapol::KeyFrameBuf {
    eapol::KeyFrameTx::new(
        eapol::ProtocolVersion::IEEE802DOT1X2001,
        eapol::KeyFrameFields::new(
            eapol::KeyDescriptor::LEGACY_WPA1,
            eapol::KeyInformation::default()
                .with_key_descriptor_version(1)
                .with_key_type(eapol::KeyType::PAIRWISE)
                .with_key_ack(true),
            32,
            0,
            eapol::to_array(anonce),
            [0u8; 16],
            0,
        ),
        vec![],
        16,
    )
    .serialize()
    .finalize_without_mic()
    .expect("failed to construct wpa1 4whs msg 1")
}

pub fn get_wpa1_4whs_msg3(ptk: &Ptk, anonce: &[u8]) -> eapol::KeyFrameBuf {
    let mut wpa_ie_buf = vec![];
    write_wpa1_ie(&mut wpa_ie_buf, &fake_wpa_ie())
        .expect("failed to write wpa ie for wpa1 4whs msg 3");
    let frame = eapol::KeyFrameTx::new(
        eapol::ProtocolVersion::IEEE802DOT1X2001,
        eapol::KeyFrameFields::new(
            eapol::KeyDescriptor::LEGACY_WPA1,
            eapol::KeyInformation::default()
                .with_key_descriptor_version(1)
                .with_key_type(eapol::KeyType::PAIRWISE)
                .with_install(true)
                .with_key_ack(true)
                .with_key_mic(true),
            32,
            0,
            eapol::to_array(anonce),
            [0u8; 16],
            0,
        ),
        wpa_ie_buf.into(),
        16,
    )
    .serialize();
    let mic = get_wpa1_protection()
        .integrity_algorithm()
        .expect("no integrity algorithm found for wpa1")
        .compute(ptk.kck(), frame.unfinalized_buf())
        .expect("failed to compute mic for wpa1 4whs msg 3");
    frame.finalize_with_mic(&mic[..]).expect("failed to construct wpa1 4whs msg 3")
}

pub fn get_pmk() -> Vec<u8> {
    PMK.clone().into_vec()
}

pub fn encrypt_key_data(kek: &[u8], protection: &NegotiatedProtection, key_data: &[u8]) -> Vec<u8> {
    let keywrap_alg =
        protection.keywrap_algorithm().expect("protection has no known keywrap Algorithm");
    keywrap_alg.wrap_key(kek, &[0; 16], key_data).expect("could not encrypt key data")
}

pub fn mic_len() -> usize {
    get_akm().mic_bytes().expect("AKM has no known MIC size") as usize
}

pub fn get_akm() -> akm::Akm {
    fake_wpa2_s_rsne().akm_suites.remove(0)
}

pub fn get_cipher() -> cipher::Cipher {
    fake_wpa2_s_rsne().pairwise_cipher_suites.remove(0)
}

enum FourwayConfig {
    Wpa2,
    Wpa3,
}

fn get_4whs_msg1<'a, F>(config: FourwayConfig, anonce: &[u8], msg_modifier: F) -> eapol::KeyFrameBuf
where
    F: Fn(&mut KeyFrameTx),
{
    let (protocol_version, key_info) = match config {
        FourwayConfig::Wpa2 => {
            (eapol::ProtocolVersion::IEEE802DOT1X2001, eapol::KeyInformation(0x008a))
        }
        FourwayConfig::Wpa3 => {
            (eapol::ProtocolVersion::IEEE802DOT1X2004, eapol::KeyInformation(0x0088))
        }
    };
    let mut msg1 = KeyFrameTx::new(
        protocol_version,
        eapol::KeyFrameFields::new(
            eapol::KeyDescriptor::IEEE802DOT11,
            key_info,
            16,
            1,
            eapol::to_array(anonce),
            [0u8; 16],
            0,
        ),
        vec![],
        mic_len(),
    );
    msg_modifier(&mut msg1);
    msg1.serialize().finalize_without_mic().expect("failed to construct 4whs msg 1")
}

pub fn get_wpa2_4whs_msg1<'a, F>(anonce: &[u8], msg_modifier: F) -> eapol::KeyFrameBuf
where
    F: Fn(&mut KeyFrameTx),
{
    get_4whs_msg1(FourwayConfig::Wpa2, anonce, msg_modifier)
}

pub fn get_wpa3_4whs_msg1(anonce: &[u8]) -> eapol::KeyFrameBuf {
    get_4whs_msg1(FourwayConfig::Wpa3, anonce, |_| {})
}

fn get_4whs_msg3<'a, F1, F2>(
    config: FourwayConfig,
    ptk: &Ptk,
    anonce: &[u8],
    gtk: &[u8],
    igtk: Option<&[u8]>,
    msg_modifier: F1,
    mic_modifier: F2,
) -> eapol::KeyFrameBuf
where
    F1: Fn(&mut KeyFrameTx),
    F2: Fn(&mut [u8]),
{
    let (protocol_version, key_info, a_rsne, s_rsne) = match config {
        FourwayConfig::Wpa2 => (
            eapol::ProtocolVersion::IEEE802DOT1X2001,
            eapol::KeyInformation(0x13ca),
            fake_wpa2_a_rsne(),
            fake_wpa2_s_rsne(),
        ),
        FourwayConfig::Wpa3 => (
            eapol::ProtocolVersion::IEEE802DOT1X2004,
            eapol::KeyInformation(0x13c8),
            fake_wpa3_a_rsne(),
            fake_wpa3_s_rsne(),
        ),
    };
    let mut w = kde::Writer::new();
    w.write_gtk(&kde::Gtk::new(2, kde::GtkInfoTx::BothRxTx, gtk)).expect("error writing GTK KDE");
    if let Some(igtk) = igtk {
        w.write_igtk(&kde::Igtk::new(4, &[0u8; 6], igtk)).expect("error writing IGTK KDE");
    }
    w.write_protection(&ProtectionInfo::Rsne(a_rsne)).expect("error writing RSNE");

    let protection =
        NegotiatedProtection::from_rsne(&s_rsne).expect("error creating RSNE NegotiatedProtection");
    let key_data = w.finalize_for_encryption().expect("error finalizing key data");
    let encrypted_key_data = encrypt_key_data(ptk.kek(), &protection, &key_data[..]);

    let mut msg3 = KeyFrameTx::new(
        protocol_version,
        eapol::KeyFrameFields::new(
            eapol::KeyDescriptor::IEEE802DOT11,
            key_info,
            16,
            2, // replay counter
            eapol::to_array(anonce),
            [0u8; 16], // iv
            0,         // rsc
        ),
        encrypted_key_data,
        mic_len(),
    );
    msg_modifier(&mut msg3);
    let msg3 = msg3.serialize();
    let mut mic = compute_mic_from_buf(ptk.kck(), &protection, msg3.unfinalized_buf())
        .expect("failed to compute msg3 mic");
    mic_modifier(&mut mic);
    msg3.finalize_with_mic(&mic[..]).expect("failed to construct 4whs msg 3")
}

pub fn get_wpa2_4whs_msg3<'a, F>(
    ptk: &Ptk,
    anonce: &[u8],
    gtk: &[u8],
    msg_modifier: F,
) -> eapol::KeyFrameBuf
where
    F: Fn(&mut KeyFrameTx),
{
    get_4whs_msg3(FourwayConfig::Wpa2, ptk, anonce, gtk, None, msg_modifier, |_| {})
}

pub fn get_wpa2_4whs_msg3_with_mic_modifier<'a, F1, F2>(
    ptk: &Ptk,
    anonce: &[u8],
    gtk: &[u8],
    msg_modifier: F1,
    mic_modifier: F2,
) -> eapol::KeyFrameBuf
where
    F1: Fn(&mut KeyFrameTx),
    F2: Fn(&mut [u8]),
{
    get_4whs_msg3(FourwayConfig::Wpa2, ptk, anonce, gtk, None, msg_modifier, mic_modifier)
}

pub fn get_wpa3_4whs_msg3(
    ptk: &Ptk,
    anonce: &[u8],
    gtk: &[u8],
    igtk: Option<&[u8]>,
) -> eapol::KeyFrameBuf {
    get_4whs_msg3(FourwayConfig::Wpa3, ptk, anonce, gtk, igtk, |_| {}, |_| {})
}

pub fn get_group_key_hs_msg1(
    ptk: &Ptk,
    gtk: &[u8],
    key_id: u8,
    key_replay_counter: u64,
) -> eapol::KeyFrameBuf {
    let mut w = kde::Writer::new();
    w.write_gtk(&kde::Gtk::new(key_id, kde::GtkInfoTx::BothRxTx, gtk))
        .expect("error writing GTK KDE");
    let key_data = w.finalize_for_encryption().expect("error finalizing key data");
    let encrypted_key_data = encrypt_key_data(ptk.kek(), &get_rsne_protection(), &key_data[..]);

    let msg1 = KeyFrameTx::new(
        eapol::ProtocolVersion::IEEE802DOT1X2001,
        eapol::KeyFrameFields::new(
            eapol::KeyDescriptor::IEEE802DOT11,
            eapol::KeyInformation(0x1382),
            16,
            key_replay_counter,
            [0u8; 32], // nonce
            [0u8; 16], // iv
            0,         // rsc
        ),
        encrypted_key_data,
        mic_len(),
    );
    let msg1 = msg1.serialize();
    let mic = compute_mic_from_buf(ptk.kck(), &get_rsne_protection(), msg1.unfinalized_buf())
        .expect("failed to compute mic");
    msg1.finalize_with_mic(&mic[..]).expect("failed to construct group key hs msg 1")
}

pub fn is_zero(slice: &[u8]) -> bool {
    slice.iter().all(|&x| x == 0)
}

fn make_fourway_cfg(
    role: Role,
    cipher: Cipher,
    s_protection: ProtectionInfo,
    a_protection: ProtectionInfo,
    gtk_key_id: u8,
    gtk_key_rsc: u64,
) -> fourway::Config {
    let gtk_provider = match role {
        Role::Authenticator => Some(Arc::new(Mutex::new(
            GtkProvider::new(cipher, gtk_key_id, gtk_key_rsc).expect("error creating GtkProvider"),
        ))),
        Role::Supplicant => None,
    };
    let igtk_provider = match role {
        Role::Authenticator => {
            match fourway::get_group_mgmt_cipher(&s_protection, &a_protection)
                .expect("error getting group mgmt cipher")
            {
                Some(group_mgmt_cipher) => Some(Arc::new(Mutex::new(
                    IgtkProvider::new(group_mgmt_cipher).expect("error creating GtkProvider"),
                ))),
                None => None,
            }
        }
        Role::Supplicant => None,
    };

    let nonce_rdr = NonceReader::new(&S_ADDR).expect("error creating Reader");
    fourway::Config::new(
        role,
        *S_ADDR,
        s_protection,
        *A_ADDR,
        a_protection,
        nonce_rdr,
        gtk_provider,
        igtk_provider,
        false,
    )
    .expect("could not construct PTK exchange method")
}

pub fn make_wpa2_fourway_cfg(role: Role, gtk_key_id: u8, gtk_key_rsc: u64) -> fourway::Config {
    let cipher = Cipher { oui: OUI, suite_type: cipher::CCMP_128 };
    let s_protection = ProtectionInfo::Rsne(fake_wpa2_s_rsne());
    let a_protection = ProtectionInfo::Rsne(fake_wpa2_a_rsne());
    make_fourway_cfg(role, cipher, s_protection, a_protection, gtk_key_id, gtk_key_rsc)
}

pub fn make_wpa3_fourway_cfg(role: Role, gtk_key_id: u8, gtk_key_rsc: u64) -> fourway::Config {
    let cipher = Cipher { oui: OUI, suite_type: cipher::CCMP_128 };
    let s_protection = ProtectionInfo::Rsne(fake_wpa3_s_rsne());
    let a_protection = ProtectionInfo::Rsne(fake_wpa3_a_rsne());
    make_fourway_cfg(role, cipher, s_protection, a_protection, gtk_key_id, gtk_key_rsc)
}

pub fn make_wpa1_fourway_cfg() -> fourway::Config {
    let cipher = Cipher { oui: Oui::MSFT, suite_type: cipher::TKIP };
    let s_protection = ProtectionInfo::LegacyWpa(fake_wpa_ie());
    let a_protection = ProtectionInfo::LegacyWpa(fake_wpa_ie());
    // There is no WPA1 Authenticator implementation, so the key id and rsc arguments do not matter.
    make_fourway_cfg(Role::Supplicant, cipher, s_protection, a_protection, 1, 0)
}

#[derive(Clone, Copy)]
pub enum HandshakeKind {
    Wpa2,
    Wpa3,
}

pub fn make_handshake(
    handshake_kind: HandshakeKind,
    role: Role,
    gtk_key_id: u8,
    gtk_key_rsc: u64,
) -> Fourway {
    match handshake_kind {
        HandshakeKind::Wpa2 => Fourway::new(
            make_wpa2_fourway_cfg(role, gtk_key_id, gtk_key_rsc),
            PMK.clone().into_vec(),
        )
        .expect("error while creating WPA2 4-Way Handshake"),
        HandshakeKind::Wpa3 => Fourway::new(
            make_wpa3_fourway_cfg(role, gtk_key_id, gtk_key_rsc),
            PMK.clone().into_vec(),
        )
        .expect("error while creating WPA3 4-Way Handshake"),
    }
}

// TODO(https://fxbug.dev/42149565): The expect_* functions that follow should be refactored with a macro.

pub fn get_eapol_resp(updates: &[SecAssocUpdate]) -> Option<eapol::KeyFrameBuf> {
    updates
        .iter()
        .filter_map(|u| match u {
            SecAssocUpdate::TxEapolKeyFrame { frame, .. } => Some(frame),
            _ => None,
        })
        .next()
        .map(|x| x.clone())
}

pub fn expect_eapol_resp(updates: &[SecAssocUpdate]) -> eapol::KeyFrameBuf {
    get_eapol_resp(updates).expect("updates do not contain EAPOL frame")
}

pub fn get_schedule_sae_timeout(updates: &[SecAssocUpdate]) -> Option<u64> {
    updates
        .iter()
        .filter_map(|u| match u {
            SecAssocUpdate::ScheduleSaeTimeout(timeout) => Some(timeout),
            _ => None,
        })
        .next()
        .map(|x| x.clone())
}

pub fn expect_schedule_sae_timeout(updates: &[SecAssocUpdate]) -> u64 {
    get_schedule_sae_timeout(updates).expect("updates do not schedule SAE timeout")
}

pub fn get_sae_frame_vec(updates: &[SecAssocUpdate]) -> Vec<SaeFrame> {
    updates
        .iter()
        .filter_map(|u| match u {
            SecAssocUpdate::TxSaeFrame(sae_frame) => Some(sae_frame.clone()),
            _ => None,
        })
        .collect()
}

pub fn expect_sae_frame_vec(updates: &[SecAssocUpdate]) -> Vec<SaeFrame> {
    let sae_frame_vec = get_sae_frame_vec(updates);
    assert!(sae_frame_vec.len() > 0, "updates do not contain SAE frame: {:?}", updates);
    sae_frame_vec
}

pub fn get_reported_pmk(updates: &[SecAssocUpdate]) -> Option<Pmk> {
    updates
        .iter()
        .filter_map(|u| match u {
            SecAssocUpdate::Key(Key::Pmk(pmk)) => Some(pmk),
            _ => None,
        })
        .next()
        .cloned()
}

pub fn expect_reported_pmk(updates: &[SecAssocUpdate]) -> Pmk {
    get_reported_pmk(updates).expect("updates do not contain PMK")
}

pub fn get_reported_ptk(updates: &[SecAssocUpdate]) -> Option<Ptk> {
    updates
        .iter()
        .filter_map(|u| match u {
            SecAssocUpdate::Key(Key::Ptk(ptk)) => Some(ptk),
            _ => None,
        })
        .next()
        .map(|x| x.clone())
}

pub fn expect_reported_ptk(updates: &[SecAssocUpdate]) -> Ptk {
    get_reported_ptk(updates).expect("updates do not contain PTK")
}

pub fn get_reported_gtk(updates: &[SecAssocUpdate]) -> Option<Gtk> {
    updates
        .iter()
        .filter_map(|u| match u {
            SecAssocUpdate::Key(Key::Gtk(gtk)) => Some(gtk),
            _ => None,
        })
        .next()
        .map(|x| x.clone())
}

pub fn expect_reported_gtk(updates: &[SecAssocUpdate]) -> Gtk {
    get_reported_gtk(updates).expect("updates do not contain GTK")
}

pub fn get_reported_igtk(updates: &[SecAssocUpdate]) -> Option<Igtk> {
    updates
        .iter()
        .filter_map(|u| match u {
            SecAssocUpdate::Key(Key::Igtk(igtk)) => Some(igtk),
            _ => None,
        })
        .next()
        .map(|x| x.clone())
}

pub fn expect_reported_igtk(updates: &[SecAssocUpdate]) -> Igtk {
    get_reported_igtk(updates).expect("updates do not contain IGTK")
}

pub fn get_reported_sae_auth_status(updates: &[SecAssocUpdate]) -> Option<AuthStatus> {
    updates
        .iter()
        .filter_map(|u| match u {
            SecAssocUpdate::SaeAuthStatus(status) => Some(status),
            _ => None,
        })
        .next()
        .map(|x| x.clone())
}

pub fn expect_reported_sae_auth_status(
    updates: &[SecAssocUpdate],
    expected_status: AuthStatus,
) -> AuthStatus {
    match get_reported_sae_auth_status(updates) {
        Some(status) => {
            assert_eq!(status, expected_status, "SAE AuthStatus does not match expected");
            status
        }
        None => panic!("updates do not contain a SAE AuthStatus"),
    }
}

pub fn get_reported_status(updates: &[SecAssocUpdate]) -> Option<SecAssocStatus> {
    updates
        .iter()
        .filter_map(|u| match u {
            SecAssocUpdate::Status(status) => Some(status),
            _ => None,
        })
        .next()
        .map(|x| x.clone())
}

pub fn expect_reported_status(
    updates: &[SecAssocUpdate],
    expected_status: SecAssocStatus,
) -> SecAssocStatus {
    match get_reported_status(updates) {
        Some(status) => {
            assert_eq!(status, expected_status, "EAPOL SecAssocStatus does not match expected");
            status
        }
        None => panic!("updates do not contain a EAPOL SecAssocStatus"),
    }
}

pub struct FourwayTestEnv {
    pub supplicant: Fourway,
    pub authenticator: Fourway,
}

// TODO(b/310961096): We should prefer to use the FourwayTestEnv in tests to avoid inconsistent
// test behavior and construction.
pub fn send_msg_to_fourway<B: SplitByteSlice + std::fmt::Debug>(
    fourway: &mut Fourway,
    msg: eapol::KeyFrameRx<B>,
    s_key_replay_counter: SupplicantKeyReplayCounter,
) -> UpdateSink {
    let role = match &fourway {
        Fourway::Authenticator(_) => Role::Authenticator,
        Fourway::Supplicant(_) => Role::Supplicant,
    };
    // We always derive NegotiatedProtection from the Supplicant's ProtectionInfo
    let protection =
        NegotiatedProtection::from_protection(&fourway.get_config().s_protection).unwrap();
    let verified_msg = make_verified(msg, role, s_key_replay_counter, &protection);

    let mut update_sink = UpdateSink::default();
    let result = fourway.on_eapol_key_frame(&mut update_sink, verified_msg);
    assert!(result.is_ok(), "{:?} failed processing msg: {}", role, result.unwrap_err());
    update_sink
}

fn make_verified<B: SplitByteSlice + std::fmt::Debug>(
    frame: eapol::KeyFrameRx<B>,
    role: Role,
    s_key_replay_counter: SupplicantKeyReplayCounter,
    protection: &NegotiatedProtection,
) -> Dot11VerifiedKeyFrame<B> {
    let result =
        Dot11VerifiedKeyFrame::from_frame(frame, &role, &protection, *s_key_replay_counter);
    assert!(result.is_ok(), "failed verifying message sent to {:?}: {}", role, result.unwrap_err());
    result.unwrap()
}

impl FourwayTestEnv {
    pub fn new(handshake_kind: HandshakeKind, gtk_key_id: u8, gtk_key_rsc: u64) -> FourwayTestEnv {
        FourwayTestEnv {
            supplicant: make_handshake(handshake_kind, Role::Supplicant, gtk_key_id, gtk_key_rsc),
            authenticator: make_handshake(
                handshake_kind,
                Role::Authenticator,
                gtk_key_id,
                gtk_key_rsc,
            ),
        }
    }

    pub fn initiate<'a>(
        &mut self,
        s_key_replay_counter: SupplicantKeyReplayCounter,
    ) -> eapol::KeyFrameBuf {
        // Initiate 4-Way Handshake. The Authenticator will send message #1 of the handshake.
        let mut a_update_sink = vec![];
        match &mut self.authenticator {
            Fourway::Authenticator(state_machine) => state_machine
                .try_replace_state(|state| state.initiate(&mut a_update_sink, s_key_replay_counter))
                .map(|_state_machine| ())
                .expect("Failed to initiate() Fourway::Authenticator"),
            _ => panic!("self.authenticator is not a Fourway::Authenticator"),
        }
        assert_eq!(a_update_sink.len(), 1);

        // Verify Authenticator sent message #1.
        expect_eapol_resp(&a_update_sink[..])
    }

    fn get_negotiated_protection(&self) -> NegotiatedProtection {
        // We always derive NegotiatedProtection from the Supplicant's ProtectionInfo.
        NegotiatedProtection::from_protection(&self.supplicant.get_config().s_protection).unwrap()
    }

    fn get_ptk(&self, anonce: &[u8], snonce: &[u8]) -> Ptk {
        let config = self.supplicant.get_config();
        let protection = self.get_negotiated_protection();
        Ptk::new(
            &PMK[..],
            &config.a_addr,
            &config.s_addr,
            anonce,
            snonce,
            &protection.akm,
            protection.pairwise,
        )
        .expect("error deriving PTK")
    }

    fn make_verified<B: SplitByteSlice + std::fmt::Debug>(
        &self,
        frame: eapol::KeyFrameRx<B>,
        role: Role,
        s_key_replay_counter: SupplicantKeyReplayCounter,
    ) -> Dot11VerifiedKeyFrame<B> {
        let protection = self.get_negotiated_protection();
        let result =
            Dot11VerifiedKeyFrame::from_frame(frame, &role, &protection, *s_key_replay_counter);
        assert!(
            result.is_ok(),
            "failed verifying message sent to {:?}: {}",
            role,
            result.unwrap_err()
        );
        result.unwrap()
    }

    pub fn finalize_key_frame(&self, frame: &mut eapol::KeyFrameRx<&mut [u8]>, kck: Option<&[u8]>) {
        if let Some(kck) = kck {
            let mic = compute_mic(kck, &self.get_negotiated_protection(), &frame)
                .expect("failed to compute mic");
            frame.key_mic.copy_from_slice(&mic[..]);
        }
    }

    pub fn send_msg_to_authenticator<B: SplitByteSlice + std::fmt::Debug>(
        &mut self,
        msg: eapol::KeyFrameRx<B>,
        s_key_replay_counter: SupplicantKeyReplayCounter,
    ) -> UpdateSink {
        let verified_msg = self.make_verified(msg, Role::Authenticator, s_key_replay_counter);

        let mut update_sink = UpdateSink::default();
        let result = self.authenticator.on_eapol_key_frame(&mut update_sink, verified_msg);
        assert!(result.is_ok(), "Authenticator failed processing msg: {}", result.unwrap_err());
        update_sink
    }

    pub fn send_msg_to_supplicant<B: SplitByteSlice + std::fmt::Debug>(
        &mut self,
        msg: eapol::KeyFrameRx<B>,
        s_key_replay_counter: SupplicantKeyReplayCounter,
    ) -> UpdateSink {
        // We always derive NegotiatedProtection from the Supplicant's ProtectionInfo.
        let verified_msg = self.make_verified(msg, Role::Supplicant, s_key_replay_counter);

        let mut update_sink = UpdateSink::default();
        let result = self.supplicant.on_eapol_key_frame(&mut update_sink, verified_msg);
        assert!(result.is_ok(), "Supplicant failed processing msg: {}", result.unwrap_err());
        update_sink
    }

    pub fn send_msg1_to_supplicant<'a, B: SplitByteSlice + std::fmt::Debug>(
        &mut self,
        msg1: eapol::KeyFrameRx<B>,
        s_key_replay_counter: SupplicantKeyReplayCounter,
    ) -> (eapol::KeyFrameBuf, Ptk) {
        let anonce = msg1.key_frame_fields.key_nonce;

        // Send message #1 to Supplicant and extract responses.
        let s_update_sink = self.send_msg_to_supplicant(msg1, s_key_replay_counter);
        let msg2 = expect_eapol_resp(&s_update_sink[..]);
        let keyframe = msg2.keyframe();
        let ptk = self.get_ptk(&anonce[..], &keyframe.key_frame_fields.key_nonce[..]);

        (msg2, ptk)
    }

    pub fn send_msg1_to_supplicant_expect_err<B: SplitByteSlice + std::fmt::Debug>(
        &mut self,
        msg1: eapol::KeyFrameRx<B>,
        s_key_replay_counter: SupplicantKeyReplayCounter,
    ) {
        let verified_msg1 = self.make_verified(msg1, Role::Supplicant, s_key_replay_counter);

        // Send message #1 to Supplicant and extract responses.
        let mut s_update_sink = vec![];
        let result = self.supplicant.on_eapol_key_frame(&mut s_update_sink, verified_msg1);
        assert!(result.is_err(), "Supplicant successfully processed illegal msg #1");
    }

    pub fn send_msg2_to_authenticator<'a, B: SplitByteSlice + std::fmt::Debug>(
        &mut self,
        msg2: eapol::KeyFrameRx<B>,
        s_key_replay_counter: SupplicantKeyReplayCounter,
    ) -> eapol::KeyFrameBuf {
        let verified_msg2 = self.make_verified(msg2, Role::Authenticator, s_key_replay_counter);

        // Send message #2 to Authenticator and extract responses.
        let mut a_update_sink = vec![];
        let result = self.authenticator.on_eapol_key_frame(&mut a_update_sink, verified_msg2);
        assert!(result.is_ok(), "Authenticator failed processing msg #2: {}", result.unwrap_err());
        expect_eapol_resp(&a_update_sink[..])
    }

    pub fn send_msg3_to_supplicant<'a, B: SplitByteSlice + std::fmt::Debug>(
        &mut self,
        msg3: eapol::KeyFrameRx<B>,
        s_key_replay_counter: SupplicantKeyReplayCounter,
    ) -> (eapol::KeyFrameBuf, Ptk, Gtk) {
        // Send message #3 to Supplicant and extract responses.
        let s_update_sink = self.send_msg_to_supplicant(msg3, s_key_replay_counter);
        let msg4 = expect_eapol_resp(&s_update_sink[..]);
        let s_ptk = expect_reported_ptk(&s_update_sink[..]);
        let s_gtk = expect_reported_gtk(&s_update_sink[..]);

        (msg4, s_ptk, s_gtk)
    }

    pub fn send_msg3_to_supplicant_capture_updates<B: SplitByteSlice + std::fmt::Debug>(
        &mut self,
        msg3: eapol::KeyFrameRx<B>,
        s_key_replay_counter: SupplicantKeyReplayCounter,
        mut update_sink: &mut UpdateSink,
    ) {
        let verified_msg3 = self.make_verified(msg3, Role::Supplicant, s_key_replay_counter);

        // Send message #3 to Supplicant and extract responses.
        let result = self.supplicant.on_eapol_key_frame(&mut update_sink, verified_msg3);
        assert!(result.is_ok(), "Supplicant failed processing msg #3: {}", result.unwrap_err());
    }

    pub fn send_msg3_to_supplicant_expect_err<B: SplitByteSlice + std::fmt::Debug>(
        &mut self,
        msg3: eapol::KeyFrameRx<B>,
        s_key_replay_counter: SupplicantKeyReplayCounter,
    ) {
        let verified_msg3 = self.make_verified(msg3, Role::Supplicant, s_key_replay_counter);

        // Send message #3 to Supplicant and extract responses.
        let mut s_update_sink = vec![];
        let result = self.supplicant.on_eapol_key_frame(&mut s_update_sink, verified_msg3);
        assert!(result.is_err(), "Supplicant successfully processed illegal msg #3");
    }

    pub fn send_msg4_to_authenticator<B: SplitByteSlice + std::fmt::Debug>(
        &mut self,
        msg4: eapol::KeyFrameRx<B>,
        s_key_replay_counter: SupplicantKeyReplayCounter,
    ) -> (Ptk, Gtk) {
        // Send message #4 to Authenticator and extract responses.
        let a_update_sink = self.send_msg_to_authenticator(msg4, s_key_replay_counter);
        let a_ptk = expect_reported_ptk(&a_update_sink[..]);
        let a_gtk = expect_reported_gtk(&a_update_sink[..]);

        (a_ptk, a_gtk)
    }
}
