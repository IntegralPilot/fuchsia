// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::event::action::{self, AuthenticationControl, AuthenticationTap};
use crate::event::{Handler, branch};
use fidl::endpoints::{create_endpoints, create_proxy};
use fidl_fuchsia_sys2 as _;
use fidl_fuchsia_wlan_common::WlanMacRole;
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_mlme as fidl_mlme;
use fidl_fuchsia_wlan_policy as fidl_policy;
use fidl_fuchsia_wlan_softmac as fidl_wlan_softmac;
use fidl_fuchsia_wlan_tap::{WlanRxInfo, WlantapPhyConfig, WlantapPhyProxy};
use fuchsia_component::client::connect_to_protocol_at;
use ieee80211::{Bssid, MacAddr, Ssid};
use std::future::Future;
use std::pin::pin;
use std::sync::LazyLock;
use wlan_common::bss::Protection;
use wlan_common::channel::{Bandwidth, Channel};
use wlan_common::ie::rsn::cipher::{CIPHER_CCMP_128, CIPHER_TKIP, Cipher};
use wlan_common::ie::rsn::rsne;
use wlan_common::ie::rsn::suite_filter::DEFAULT_GROUP_MGMT_CIPHER;
use wlan_common::ie::wpa;
use wlan_common::{TimeUnit, data_writer, mac, mgmt_writer};
use wlan_frame_writer::write_frame_to_vec;
use wlan_rsn::rsna::UpdateSink;

pub mod event;
pub mod netdevice_helper;
pub mod test_utils;

pub use wlancfg_helper::*;

mod config;
mod wlancfg_helper;

pub const PSK_STR_LEN: usize = 64;

pub static CLIENT_MAC_ADDR: LazyLock<MacAddr> =
    LazyLock::new(|| [0x67, 0x62, 0x6f, 0x6e, 0x69, 0x6b].into());
pub static AP_MAC_ADDR: LazyLock<Bssid> =
    LazyLock::new(|| [0x70, 0xf1, 0x1c, 0x05, 0x2d, 0x7f].into());
pub static AP_SSID: LazyLock<Ssid> = LazyLock::new(|| Ssid::try_from("ap_ssid").unwrap());
pub static ETH_DST_MAC: LazyLock<MacAddr> =
    LazyLock::new(|| [0x65, 0x74, 0x68, 0x64, 0x73, 0x74].into());

pub const WLANCFG_DEFAULT_AP_CHANNEL: Channel =
    Channel::new(11, Bandwidth::Cbw20, fidl_ieee80211::WlanBand::TwoGhz);

// TODO(https://fxbug.dev/42060050): This sleep was introduced to preserve the old timing behavior
// of scanning when hw-sim depending on the SoftMAC driver iterating through all of the
// channels.
pub static ARTIFICIAL_SCAN_SLEEP: LazyLock<zx::MonotonicDuration> =
    LazyLock::new(|| zx::MonotonicDuration::from_seconds(2));

// Once a client interface is available for scanning, it takes up to around 30s for a scan
// to complete (see https://fxbug.dev/42061276). Allow at least double that amount of time to reduce
// flakiness and longer than the timeout WLAN policy should have.
pub static SCAN_RESPONSE_TEST_TIMEOUT: LazyLock<zx::MonotonicDuration> =
    LazyLock::new(|| zx::MonotonicDuration::from_seconds(70));

/// A client supplicant.
///
/// Provides the client and security components necessary to attempt a connection via Policy.
pub struct Supplicant<'a> {
    pub controller: &'a fidl_policy::ClientControllerProxy,
    pub state_update_stream: &'a mut fidl_policy::ClientStateUpdatesRequestStream,
    pub security_type: fidl_policy::SecurityType,
    pub password: Option<&'a str>,
}

impl<'a> Supplicant<'a> {
    /// Clones the supplicant through reborrowing of mutable references.
    ///
    /// # Examples
    ///
    /// This function can be used for templating.
    ///
    /// ```rust,ignore
    /// let mut supplicant = Supplicant { /* ... */ }; // Template.
    /// // ...
    /// // Connect via the template supplicant but with a particular password.
    /// let _ = connect(Supplicant { password: "********", ..supplicant.reborrow() });
    /// ```
    pub fn reborrow(&mut self) -> Supplicant<'_> {
        Supplicant {
            controller: self.controller,
            state_update_stream: &mut *self.state_update_stream,
            security_type: self.security_type,
            password: self.password,
        }
    }
}

pub fn default_wlantap_config_client() -> WlantapPhyConfig {
    wlantap_config_client(format!("wlantap-client"), *CLIENT_MAC_ADDR)
}

pub fn wlantap_config_client(name: String, mac_addr: MacAddr) -> WlantapPhyConfig {
    config::create_wlantap_config(name, mac_addr, WlanMacRole::Client)
}

pub fn default_wlantap_config_ap() -> WlantapPhyConfig {
    wlantap_config_ap(format!("wlantap-ap"), (*AP_MAC_ADDR).into())
}

pub fn wlantap_config_ap(name: String, mac_addr: MacAddr) -> WlantapPhyConfig {
    config::create_wlantap_config(name, mac_addr, WlanMacRole::Ap)
}

pub fn rx_info_with_default_ap() -> WlanRxInfo {
    rx_info_with_valid_rssi(&WLANCFG_DEFAULT_AP_CHANNEL, 0)
}

fn rx_info_with_valid_rssi(channel: &Channel, rssi_dbm: i8) -> WlanRxInfo {
    let (cbw, secondary80_num) = channel.bandwidth.to_fidl();
    WlanRxInfo {
        rx_flags: 0,
        valid_fields: if rssi_dbm == 0 {
            0
        } else {
            fidl_wlan_softmac::WlanRxInfoValid::RSSI.bits()
        },
        phy: fidl_ieee80211::WlanPhyType::Dsss,
        data_rate: 0,
        primary: (*channel).into(),
        mcs: 0,
        rssi_dbm,
        snr_dbh: 0,
        bandwidth: cbw,
        vht_secondary_80_channel: fidl_ieee80211::ChannelNumber {
            band: channel.band,
            number: secondary80_num,
        },
    }
}

pub fn send_sae_authentication_frame(
    sae_frame: &fidl_mlme::SaeFrame,
    channel: &Channel,
    bssid: &Bssid,
    proxy: &WlantapPhyProxy,
) -> Result<(), anyhow::Error> {
    let buffer = write_frame_to_vec!({
        headers: {
            mac::MgmtHdr: &mgmt_writer::mgmt_hdr_from_ap(
                mac::FrameControl(0)
                    .with_frame_type(mac::FrameType::MGMT)
                    .with_mgmt_subtype(mac::MgmtSubtype::AUTH),
                *CLIENT_MAC_ADDR,
                bssid.clone().into(),
                mac::SequenceControl(0).with_seq_num(123),
            ),
            mac::AuthHdr: &mac::AuthHdr {
                auth_alg_num: mac::AuthAlgorithmNumber::SAE,
                auth_txn_seq_num: sae_frame.seq_num,
                status_code: sae_frame.status_code.into(),
            },
        },
        body: &sae_frame.sae_fields[..],
    })?;
    proxy.rx(&buffer, &rx_info_with_valid_rssi(channel, 0))?;
    Ok(())
}

pub fn send_open_authentication(
    channel: &Channel,
    bssid: &Bssid,
    status_code: impl Into<mac::StatusCode>,
    proxy: &WlantapPhyProxy,
) -> Result<(), anyhow::Error> {
    let buffer = write_frame_to_vec!({
        headers: {
            mac::MgmtHdr: &mgmt_writer::mgmt_hdr_from_ap(
                mac::FrameControl(0)
                    .with_frame_type(mac::FrameType::MGMT)
                    .with_mgmt_subtype(mac::MgmtSubtype::AUTH),
                *CLIENT_MAC_ADDR,
                bssid.clone().into(),
                mac::SequenceControl(0).with_seq_num(123),
            ),
            mac::AuthHdr: &mac::AuthHdr {
                auth_alg_num: mac::AuthAlgorithmNumber::OPEN,
                auth_txn_seq_num: 2,
                status_code: status_code.into(),
            },
        },
    })?;
    proxy.rx(&buffer, &rx_info_with_valid_rssi(channel, 0))?;
    Ok(())
}

pub fn send_association_response(
    channel: &Channel,
    bssid: &Bssid,
    status_code: impl Into<mac::StatusCode>,
    proxy: &WlantapPhyProxy,
) -> Result<(), anyhow::Error> {
    let buffer = write_frame_to_vec!({
        headers: {
            mac::MgmtHdr: &mgmt_writer::mgmt_hdr_from_ap(
                mac::FrameControl(0)
                    .with_frame_type(mac::FrameType::MGMT)
                    .with_mgmt_subtype(mac::MgmtSubtype::ASSOC_RESP),
                *CLIENT_MAC_ADDR,
                bssid.clone().into(),
                mac::SequenceControl(0).with_seq_num(123),
            ),
            mac::AssocRespHdr: &mac::AssocRespHdr {
                capabilities: mac::CapabilityInfo(0).with_ess(true).with_short_preamble(true),
                status_code: status_code.into(),
                aid: 2, // does not matter
            },
        },
        ies: {
            // These rates will be captured in assoc_cfg to initialize Minstrel. 11b rates are
            // ignored.
            // tx_vec_idx:        _     _     _   129   130     _   131   132
            supported_rates: &[0x82, 0x84, 0x8b, 0x0c, 0x12, 0x96, 0x18, 0x24],
            // tx_vec_idx:              133 134 basic_135  136
            extended_supported_rates:  &[48, 72, 128 + 96, 108],
        },
    })?;
    proxy.rx(&buffer, &rx_info_with_valid_rssi(channel, 0))?;
    Ok(())
}

pub fn send_disassociate(
    channel: &Channel,
    bssid: &Bssid,
    reason_code: impl Into<mac::ReasonCode>,
    proxy: &WlantapPhyProxy,
) -> Result<(), anyhow::Error> {
    let buffer = write_frame_to_vec!({
        headers: {
            mac::MgmtHdr: &mgmt_writer::mgmt_hdr_from_ap(
                mac::FrameControl(0)
                    .with_frame_type(mac::FrameType::MGMT)
                    .with_mgmt_subtype(mac::MgmtSubtype::DISASSOC),
                *CLIENT_MAC_ADDR,
                bssid.clone().into(),
                mac::SequenceControl(0).with_seq_num(123),
            ),
            mac::DisassocHdr: &mac::DisassocHdr {
                reason_code: reason_code.into(),
            },
        },
    })?;
    proxy.rx(&buffer, &rx_info_with_valid_rssi(channel, 0))?;
    Ok(())
}

pub fn password_or_psk_to_policy_credential<S: ToString>(
    password_or_psk: Option<S>,
) -> fidl_policy::Credential {
    return match password_or_psk {
        None => fidl_policy::Credential::None(fidl_policy::Empty),
        Some(p) => {
            let p = p.to_string().as_bytes().to_vec();
            if p.len() == PSK_STR_LEN {
                // The PSK is given in a 64 character hexadecimal string.
                let psk = hex::decode(p).expect("Failed to decode psk");
                fidl_policy::Credential::Psk(psk)
            } else {
                fidl_policy::Credential::Password(p)
            }
        }
    };
}

pub fn create_authenticator(
    bssid: &Bssid,
    ssid: &Ssid,
    password_or_psk: &str,
    // The group key cipher
    gtk_cipher: Cipher,
    // The advertised protection in the IEs during the 4-way handshake
    advertised_protection: Protection,
    // The protection used for the actual handshake
    supplicant_protection: Protection,
) -> wlan_rsn::Authenticator {
    let nonce_rdr =
        wlan_rsn::nonce::NonceReader::new(&bssid.clone().into()).expect("creating nonce reader");
    let gtk_provider = wlan_rsn::GtkProvider::new(gtk_cipher, 1, 0).expect("creating gtk provider");

    let advertised_protection_info = match advertised_protection {
        Protection::Wpa3Personal => wlan_rsn::ProtectionInfo::Rsne(rsne::Rsne::wpa3_rsne()),
        Protection::Wpa2Wpa3Personal => {
            wlan_rsn::ProtectionInfo::Rsne(rsne::Rsne::wpa2_wpa3_rsne())
        }
        Protection::Wpa2Personal | Protection::Wpa1Wpa2Personal => wlan_rsn::ProtectionInfo::Rsne(
            rsne::Rsne::wpa2_rsne_with_caps(rsne::RsnCapabilities(0)),
        ),
        Protection::Wpa2PersonalTkipOnly | Protection::Wpa1Wpa2PersonalTkipOnly => {
            panic!("need tkip support")
        }
        Protection::Wpa1 => {
            wlan_rsn::ProtectionInfo::LegacyWpa(wpa::fake_wpa_ies::fake_deprecated_wpa1_vendor_ie())
        }
        _ => {
            panic!("{} not implemented", advertised_protection)
        }
    };

    match supplicant_protection {
        Protection::Wpa1 | Protection::Wpa2Personal => {
            let psk = match password_or_psk.len() {
                PSK_STR_LEN => {
                    // The PSK is given in a 64 character hexadecimal string.
                    hex::decode(password_or_psk).expect("Failed to decode psk").into_boxed_slice()
                }
                _ => {
                    wlan_rsn::psk::compute(password_or_psk.as_bytes(), ssid).expect("computing PSK")
                }
            };
            let supplicant_protection_info = match supplicant_protection {
                Protection::Wpa1 => wlan_rsn::ProtectionInfo::LegacyWpa(
                    wpa::fake_wpa_ies::fake_deprecated_wpa1_vendor_ie(),
                ),
                Protection::Wpa2Personal => wlan_rsn::ProtectionInfo::Rsne(
                    rsne::Rsne::wpa2_rsne_with_caps(rsne::RsnCapabilities(0)),
                ),
                _ => unreachable!("impossible combination in this nested match"),
            };
            wlan_rsn::Authenticator::new_wpa2psk_ccmp128(
                nonce_rdr,
                std::sync::Arc::new(fuchsia_sync::Mutex::new(gtk_provider)),
                psk,
                *CLIENT_MAC_ADDR,
                supplicant_protection_info,
                bssid.clone().into(),
                advertised_protection_info,
            )
            .expect("creating authenticator")
        }
        Protection::Wpa3Personal => {
            let igtk_provider = wlan_rsn::IgtkProvider::new(DEFAULT_GROUP_MGMT_CIPHER)
                .expect("creating igtk provider");
            let supplicant_protection_info =
                wlan_rsn::ProtectionInfo::Rsne(rsne::Rsne::wpa3_rsne());
            wlan_rsn::Authenticator::new_wpa3(
                nonce_rdr,
                std::sync::Arc::new(fuchsia_sync::Mutex::new(gtk_provider)),
                std::sync::Arc::new(fuchsia_sync::Mutex::new(igtk_provider)),
                ssid.clone(),
                password_or_psk.as_bytes().to_vec(),
                *CLIENT_MAC_ADDR,
                supplicant_protection_info,
                bssid.clone().into(),
                advertised_protection_info,
            )
            .expect("creating authenticator")
        }
        _ => {
            panic!("Cannot create an authenticator for {}", supplicant_protection)
        }
    }
}

pub enum ApAdvertisementMode {
    Beacon,
    ProbeResponse,
}

pub trait ApAdvertisement {
    fn mode(&self) -> ApAdvertisementMode;
    fn channel(&self) -> &Channel;
    fn bssid(&self) -> &Bssid;
    fn ssid(&self) -> &Ssid;
    fn protection(&self) -> &Protection;
    fn rssi_dbm(&self) -> i8;
    fn wsc_ie(&self) -> Option<&Vec<u8>>;

    fn beacon_interval(&self) -> TimeUnit {
        TimeUnit::DEFAULT_BEACON_INTERVAL * 20u16
    }

    fn capabilities(&self) -> mac::CapabilityInfo {
        mac::CapabilityInfo(0)
            // IEEE Std 802.11-2016, 9.4.1.4: An AP sets the ESS subfield to 1 and the IBSS
            // subfield to 0 within transmitted Beacon or Probe Response frames.
            .with_ess(true)
            .with_ibss(false)
            // IEEE Std 802.11-2016, 9.4.1.4: An AP sets the Privacy subfield to 1 within
            // transmitted Beacon, Probe Response, (Re)Association Response frames if data
            // confidentiality is required for all Data frames exchanged within the BSS.
            .with_privacy(*self.protection() != Protection::Open)
    }

    fn send(&self, phy: &WlantapPhyProxy) -> Result<(), anyhow::Error> {
        let buffer = self.generate_frame()?;
        phy.rx(&buffer, &rx_info_with_valid_rssi(&self.channel(), self.rssi_dbm()))?;
        Ok(())
    }

    fn generate_frame(&self) -> Result<Vec<u8>, anyhow::Error> {
        let mode = self.mode();
        let protection = self.protection();
        let beacon_header = match mode {
            ApAdvertisementMode::Beacon => {
                Some(mac::BeaconHdr::new(self.beacon_interval(), self.capabilities()))
            }
            _ => None,
        };
        let probe_response_header = match mode {
            ApAdvertisementMode::ProbeResponse => {
                Some(mac::ProbeRespHdr::new(self.beacon_interval(), self.capabilities()))
            }
            _ => None,
        };

        let buffer = write_frame_to_vec!({
            headers: {
                mac::MgmtHdr: &mgmt_writer::mgmt_hdr_from_ap(
                    mac::FrameControl(0)
                        .with_frame_type(mac::FrameType::MGMT)
                        .with_mgmt_subtype(match mode {
                            ApAdvertisementMode::Beacon => mac::MgmtSubtype::BEACON,
                            ApAdvertisementMode::ProbeResponse{..} => mac::MgmtSubtype::PROBE_RESP
                        }),
                    match mode {
                        ApAdvertisementMode::Beacon => ieee80211::BROADCAST_ADDR,
                        ApAdvertisementMode::ProbeResponse{..} => *CLIENT_MAC_ADDR
                    },
                    *self.bssid(),
                    mac::SequenceControl(0).with_seq_num(123),
                ),
                mac::BeaconHdr?: beacon_header,
                mac::ProbeRespHdr?: probe_response_header,
            },
            ies: {
                ssid: &self.ssid(),
                supported_rates: &[0x82, 0x84, 0x8b, 0x0c, 0x12, 0x96, 0x18, 0x24, 0x30, 0x48, 0xe0, 0x6c],
                extended_supported_rates: { /* continues from supported_rates */ },
                dsss_param_set: &ie::DsssParamSet { current_channel: self.channel().primary },
                rsne?: match protection {
                    Protection::Unknown => panic!("Cannot send beacon with unknown protection"),
                    Protection::Open | Protection::Wep | Protection::Wpa1 => None,
                    Protection::Wpa1Wpa2Personal | Protection::Wpa2Personal =>
                        Some(rsne::Rsne::wpa2_rsne_with_caps(rsne::RsnCapabilities(0))),
                    Protection::Wpa2Wpa3Personal => Some(rsne::Rsne::wpa2_wpa3_rsne()),
                    Protection::Wpa3Personal => Some(rsne::Rsne::wpa3_rsne()),
                    _ => panic!("unsupported fake beacon: {:?}", protection),
                },
                wpa1?: match protection {
                    Protection::Unknown => panic!("Cannot send beacon with unknown protection"),
                    Protection::Open | Protection::Wep => None,
                    Protection::Wpa1 | Protection::Wpa1Wpa2Personal => Some(wpa::fake_wpa_ies::fake_deprecated_wpa1_vendor_ie()),
                    Protection::Wpa2Personal | Protection::Wpa2Wpa3Personal | Protection::Wpa3Personal => None,
                    _ => panic!("unsupported fake beacon: {:?}", protection),
                },
                wsc?: self.wsc_ie()
            },
        })?;
        Ok(buffer.into())
    }
}

pub struct Beacon {
    pub channel: Channel,
    pub bssid: Bssid,
    pub ssid: Ssid,
    pub protection: Protection,
    pub rssi_dbm: i8,
}

impl ApAdvertisement for Beacon {
    fn mode(&self) -> ApAdvertisementMode {
        ApAdvertisementMode::Beacon
    }
    fn channel(&self) -> &Channel {
        &self.channel
    }
    fn bssid(&self) -> &Bssid {
        &self.bssid
    }
    fn ssid(&self) -> &Ssid {
        &self.ssid
    }
    fn protection(&self) -> &Protection {
        &self.protection
    }
    fn rssi_dbm(&self) -> i8 {
        self.rssi_dbm
    }
    fn wsc_ie(&self) -> Option<&Vec<u8>> {
        None
    }
}

pub struct ProbeResponse {
    pub channel: Channel,
    pub bssid: Bssid,
    pub ssid: Ssid,
    pub protection: Protection,
    pub rssi_dbm: i8,
    pub wsc_ie: Option<Vec<u8>>,
}

impl ApAdvertisement for ProbeResponse {
    fn mode(&self) -> ApAdvertisementMode {
        ApAdvertisementMode::ProbeResponse
    }
    fn channel(&self) -> &Channel {
        &self.channel
    }
    fn bssid(&self) -> &Bssid {
        &self.bssid
    }
    fn ssid(&self) -> &Ssid {
        &self.ssid
    }
    fn protection(&self) -> &Protection {
        &self.protection
    }
    fn rssi_dbm(&self) -> i8 {
        self.rssi_dbm
    }
    fn wsc_ie(&self) -> Option<&Vec<u8>> {
        self.wsc_ie.as_ref()
    }
}

pub async fn save_network_and_wait_until_connected(
    test_ns_prefix: &str,
    ssid: &Ssid,
    security_type: fidl_policy::SecurityType,
    credential: fidl_policy::Credential,
) -> (fidl_policy::ClientControllerProxy, fidl_policy::ClientStateUpdatesRequestStream) {
    // Connect to the client policy service and get a client controller.
    let (client_controller, mut client_state_update_stream) =
        wlancfg_helper::init_client_controller(test_ns_prefix).await;

    save_network(&client_controller, ssid, security_type, credential).await;

    // Wait until the policy layer indicates that the client has successfully connected.
    let id = fidl_policy::NetworkIdentifier { ssid: ssid.to_vec(), type_: security_type.clone() };
    wait_until_client_state(&mut client_state_update_stream, |update| {
        has_id_and_state(update, &id, fidl_policy::ConnectionState::Connected)
    })
    .await;

    (client_controller, client_state_update_stream)
}

/// Runs a future until completion or timeout with a client event handler that attempts to connect
/// to an AP with the given SSID, BSSID, and protection.
pub async fn connect_or_timeout_with<F>(
    helper: &mut test_utils::TestHelper,
    timeout: zx::MonotonicDuration,
    ssid: &Ssid,
    bssid: &Bssid,
    protection: &Protection,
    authenticator: Option<wlan_rsn::Authenticator>,
    future: F,
) -> F::Output
where
    F: Future + Unpin,
{
    let phy = helper.proxy();
    let channel = Channel::new(1, Bandwidth::Cbw20, fidl_ieee80211::WlanBand::TwoGhz);
    let beacons = [Beacon {
        channel,
        bssid: bssid.clone(),
        ssid: ssid.clone(),
        protection: protection.clone(),
        rssi_dbm: -30,
    }];
    let mut control = authenticator
        .map(|authenticator| AuthenticationControl { updates: UpdateSink::new(), authenticator });
    let connect = if let Some(ref mut control) = control {
        let tap = AuthenticationTap { control, handler: action::authenticate_with_control_state() };
        event::boxed(action::connect_with_authentication_tap(
            &phy, ssid, bssid, &channel, protection, tap,
        ))
    } else {
        event::boxed(action::connect_with_open_authentication(
            &phy, ssid, bssid, &channel, protection,
        ))
    };
    helper
        .run_until_complete_or_timeout(
            timeout,
            format!(
                "connecting to {} ({:02X?}) with {:?} protection",
                ssid.to_string_not_redactable(),
                bssid,
                protection,
            ),
            branch::or((
                event::on_scan(action::send_advertisements_and_scan_completion(&phy, beacons)),
                event::on_transmit(connect),
            ))
            .expect("failed to connect client"),
            future,
        )
        .await
}

/// Waits for a timeout or Policy to establish a connection to an AP with the given SSID, BSSID,
/// and protection.
pub async fn connect_or_timeout(
    helper: &mut test_utils::TestHelper,
    timeout: zx::MonotonicDuration,
    ssid: &Ssid,
    bssid: &Bssid,
    bss_protection: &Protection,
    password_or_psk: Option<&str>,
    security_type: fidl_policy::SecurityType,
) {
    let authenticator = match bss_protection {
        Protection::Wpa3Personal | Protection::Wpa2Wpa3Personal => {
            password_or_psk.map(|password_or_psk| {
                create_authenticator(
                    bssid,
                    ssid,
                    password_or_psk,
                    CIPHER_CCMP_128,
                    *bss_protection,
                    Protection::Wpa3Personal,
                )
            })
        }
        Protection::Wpa2Personal | Protection::Wpa1Wpa2Personal => {
            password_or_psk.map(|password_or_psk| {
                create_authenticator(
                    bssid,
                    ssid,
                    password_or_psk,
                    CIPHER_CCMP_128,
                    *bss_protection,
                    Protection::Wpa2Personal,
                )
            })
        }
        Protection::Wpa2PersonalTkipOnly | Protection::Wpa1Wpa2PersonalTkipOnly => {
            panic!("Hardware simulator does not support WPA2-TKIP.")
        }
        Protection::Wpa1 => password_or_psk.map(|password_or_psk| {
            create_authenticator(
                bssid,
                ssid,
                password_or_psk,
                CIPHER_TKIP,
                *bss_protection,
                Protection::Wpa1,
            )
        }),
        Protection::Open => None,
        _ => {
            panic!("Unsupported WLAN protection: {}", bss_protection)
        }
    };

    let credential = password_or_psk_to_policy_credential(password_or_psk);
    let test_ns_prefix = helper.test_ns_prefix().to_string();
    let connect = pin!(save_network_and_wait_until_connected(
        &test_ns_prefix,
        ssid,
        security_type,
        credential
    ));
    connect_or_timeout_with(helper, timeout, ssid, bssid, bss_protection, authenticator, connect)
        .await;
}

pub fn rx_wlan_data_frame(
    channel: &Channel,
    addr1: &MacAddr,
    addr2: &MacAddr,
    addr3: &MacAddr,
    payload: &[u8],
    ether_type: u16,
    phy: &WlantapPhyProxy,
) -> Result<(), anyhow::Error> {
    let buffer = write_frame_to_vec!({
        headers: {
            mac::FixedDataHdrFields: &mac::FixedDataHdrFields {
                frame_ctrl: mac::FrameControl(0)
                    .with_frame_type(mac::FrameType::DATA)
                    .with_data_subtype(mac::DataSubtype(0))
                    .with_from_ds(true),
                duration: 0,
                addr1: *addr1,
                addr2: *addr2,
                addr3: *addr3,
                seq_ctrl: mac::SequenceControl(0).with_seq_num(3),
            },
            mac::LlcHdr: &data_writer::make_snap_llc_hdr(ether_type),
        },
        payload: payload,
    })?;

    phy.rx(&buffer, &rx_info_with_valid_rssi(channel, 0))?;
    Ok(())
}

pub async fn loop_until_iface_is_found(helper: &mut test_utils::TestHelper) {
    // Connect to the client policy service and get a client controller.
    let policy_provider =
        connect_to_protocol_at::<fidl_policy::ClientProviderMarker>(helper.test_ns_prefix())
            .expect("connecting to wlan policy");
    let (client_controller, server_end) = create_proxy();
    let (update_client_end, _update_server_end) = create_endpoints();
    let () =
        policy_provider.get_controller(server_end, update_client_end).expect("getting controller");

    // Attempt to issue a scan command until the request succeeds.  Scanning will fail until a
    // client interface is available.  A successful response to a scan request indicates that the
    // client policy layer is ready to use.
    // TODO(https://fxbug.dev/42135259): Figure out a new way to signal that the client policy layer is ready to go.
    let mut retry = test_utils::RetryWithBackoff::infinite_with_max_interval(
        zx::MonotonicDuration::from_seconds(10),
    );
    loop {
        let (scan_proxy, server_end) = create_proxy();
        client_controller.scan_for_networks(server_end).expect("requesting scan");

        let fut = pin!(async move { scan_proxy.get_next().await.expect("getting scan results") });

        let phy = helper.proxy();
        match helper
            .run_until_complete_or_timeout(
                *SCAN_RESPONSE_TEST_TIMEOUT,
                "receive a scan response",
                event::on_scan(action::send_advertisements_and_scan_completion(
                    &phy,
                    [] as [Beacon; 0],
                )),
                fut,
            )
            .await
        {
            Err(_) => {
                retry.sleep_unless_after_deadline().await.unwrap_or_else(|_| {
                    panic!("Wlanstack did not recognize the interface in time")
                });
            }
            Ok(_) => return,
        }
    }
}
