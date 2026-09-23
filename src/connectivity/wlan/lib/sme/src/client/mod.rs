// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod event;
mod inspect;
mod protection;
mod rsn;
mod scan;
mod state;

mod wpa;

#[cfg(test)]
pub mod test_utils;

use self::event::Event;
use self::protection::{Protection, SecurityContext};
pub use self::scan::ScheduledScanReceiver;
use self::scan::{DiscoveryScan, ScanScheduler};
use self::state::{ClientState, ConnectCommand};
use crate::responder::Responder;
use crate::{Config, MlmeRequest, MlmeSink, MlmeStream};
use fidl_fuchsia_wlan_common as fidl_common;
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_internal as fidl_internal;
use fidl_fuchsia_wlan_mlme as fidl_mlme;
use fidl_fuchsia_wlan_sme as fidl_sme;
use fidl_fuchsia_wlan_stats as fidl_stats;
use futures::channel::{mpsc, oneshot};
use ieee80211::{Bssid, MacAddrBytes, Ssid};
use log::{error, info, warn};
use std::sync::Arc;
use wlan_common::bss::{BssDescription, Protection as BssProtection};
use wlan_common::capabilities::derive_join_capabilities;
use wlan_common::ie::rsn::rsne;
use wlan_common::ie::{self, wsc};
use wlan_common::mac::MacRole;
use wlan_common::scan::{Compatibility, Compatible, Incompatible, ScanResult};
use wlan_common::security::{SecurityAuthenticator, SecurityDescriptor};
use wlan_common::sink::UnboundedSink;
use wlan_common::timer;
use wlan_rsn::auth;

// This is necessary to trick the private-in-public checker.
// A private module is not allowed to include private types in its interface,
// even though the module itself is private and will never be exported.
// As a workaround, we add another private module with public types.
mod internal {
    use crate::MlmeSink;
    use crate::client::event::Event;
    use crate::client::{ConnectionAttemptId, inspect};
    use fidl_fuchsia_wlan_common as fidl_common;
    use fidl_fuchsia_wlan_mlme as fidl_mlme;
    use std::sync::Arc;
    use wlan_common::timer::Timer;

    pub struct Context {
        pub device_info: Arc<fidl_mlme::DeviceInfo>,
        pub mlme_sink: MlmeSink,
        pub(crate) timer: Timer<Event>,
        pub att_id: ConnectionAttemptId,
        pub(crate) inspect: Arc<inspect::SmeTree>,
        pub security_support: fidl_common::SecuritySupport,
    }
}

use self::internal::*;

// An automatically increasing sequence number that uniquely identifies a logical
// connection attempt. For example, a new connection attempt can be triggered
// by a DisassociateInd message from the MLME.
pub type ConnectionAttemptId = u64;

#[derive(Default, Debug, Copy, Clone, PartialEq, Eq)]
pub struct ClientConfig {
    cfg: Config,
    pub wpa3_supported: bool,
    pub owe_supported: bool,
}

impl ClientConfig {
    pub fn from_config(cfg: Config, wpa3_supported: bool, owe_supported: bool) -> Self {
        Self { cfg, wpa3_supported, owe_supported }
    }

    /// Converts a given BssDescription into a ScanResult.
    pub fn create_scan_result(
        &self,
        timestamp: zx::MonotonicInstant,
        bss_description: BssDescription,
        device_info: &fidl_mlme::DeviceInfo,
        security_support: &fidl_common::SecuritySupport,
    ) -> ScanResult {
        ScanResult {
            compatibility: self.bss_compatibility(&bss_description, device_info, security_support),
            timestamp,
            bss_description,
        }
    }

    /// Gets the compatible modes of operation of the BSS with respect to driver and hardware
    /// support.
    ///
    /// Returns `None` if the BSS is not supported by the client.
    pub fn bss_compatibility(
        &self,
        bss: &BssDescription,
        device_info: &fidl_mlme::DeviceInfo,
        security_support: &fidl_common::SecuritySupport,
    ) -> Compatibility {
        // TODO(https://fxbug.dev/384797729): Include information about disjoint channels and data
        //                                    rates in `Incompatible`.
        self.has_compatible_channel_and_data_rates(bss, device_info)
            .then(|| {
                Compatible::try_from_features(
                    self.security_protocol_intersection(bss, security_support),
                )
            })
            .flatten()
            .ok_or_else(|| {
                Incompatible::try_from_features(
                    "incompatible channel, PHY data rates, or security protocols",
                    Some(self.security_protocols_by_mac_role(bss)),
                )
                .unwrap_or_else(|| {
                    Incompatible::from_description("incompatible channel or PHY data rates")
                })
            })
    }

    /// Gets the intersection of security protocols supported by the BSS and local interface.
    ///
    /// Security protocol support of the local interface is determined by the given
    /// `SecuritySupport`. The set of mutually supported protocols may be empty.
    fn security_protocol_intersection(
        &self,
        bss: &BssDescription,
        security_support: &fidl_common::SecuritySupport,
    ) -> Vec<SecurityDescriptor> {
        // Construct queries for security protocol support based on hardware, driver, and BSS
        // compatibility.
        let has_privacy = wlan_common::mac::CapabilityInfo(bss.capability_info).privacy();
        let has_owe_support = || {
            self.owe_supported
                && has_privacy
                && bss.rsne().is_some_and(|rsne| {
                    rsne::from_bytes(rsne)
                        .is_ok_and(|(_, a_rsne)| a_rsne.is_owe_rsn_compatible(security_support))
                })
        };
        let has_wep_support = || self.cfg.wep_supported;
        let has_wpa1_support = || self.cfg.wpa1_supported;
        let has_wpa2_support = || {
            // TODO(https://fxbug.dev/42059694): Unlike other protocols, hardware and driver
            //                                   support for WPA2 is assumed here. Query and track
            //                                   this as with other security protocols.
            has_privacy
                && bss.rsne().is_some_and(|rsne| {
                    rsne::from_bytes(rsne)
                        .is_ok_and(|(_, a_rsne)| a_rsne.is_wpa2_rsn_compatible(security_support))
                })
        };
        let has_wpa3_support = || {
            self.wpa3_supported
                && has_privacy
                && bss.rsne().is_some_and(|rsne| {
                    rsne::from_bytes(rsne)
                        .is_ok_and(|(_, a_rsne)| a_rsne.is_wpa3_rsn_compatible(security_support))
                })
        };

        // Determine security protocol compatibility. This `match` expression does not use guard
        // expressions to avoid implicit patterns like `_`, which may introduce bugs if
        // `BssProtection` changes.
        match bss.protection() {
            BssProtection::Open => vec![SecurityDescriptor::OPEN],
            // Add OWE support for OWE transition without checking if the BSS supports the protocol
            // as this BSS would not actually support it, it points to one that does support it.
            BssProtection::OpenOweTransition if self.owe_supported => {
                vec![SecurityDescriptor::OWE, SecurityDescriptor::OPEN]
            }
            BssProtection::OpenOweTransition => vec![SecurityDescriptor::OPEN],
            BssProtection::Owe if has_owe_support() => vec![SecurityDescriptor::OWE],
            BssProtection::Owe => vec![],
            BssProtection::Wep if has_wep_support() => vec![SecurityDescriptor::WEP],
            BssProtection::Wep => vec![],
            BssProtection::Wpa1 if has_wpa1_support() => vec![SecurityDescriptor::WPA1],
            BssProtection::Wpa1 => vec![],
            BssProtection::Wpa1Wpa2PersonalTkipOnly | BssProtection::Wpa1Wpa2Personal => {
                has_wpa2_support()
                    .then_some(SecurityDescriptor::WPA2_PERSONAL)
                    .into_iter()
                    .chain(has_wpa1_support().then_some(SecurityDescriptor::WPA1))
                    .collect()
            }
            BssProtection::Wpa2PersonalTkipOnly | BssProtection::Wpa2Personal
                if has_wpa2_support() =>
            {
                vec![SecurityDescriptor::WPA2_PERSONAL]
            }
            BssProtection::Wpa2PersonalTkipOnly | BssProtection::Wpa2Personal => vec![],
            BssProtection::Wpa2Wpa3Personal => match (has_wpa3_support(), has_wpa2_support()) {
                (true, true) => {
                    vec![SecurityDescriptor::WPA3_PERSONAL, SecurityDescriptor::WPA2_PERSONAL]
                }
                (true, false) => vec![SecurityDescriptor::WPA3_PERSONAL],
                (false, true) => vec![SecurityDescriptor::WPA2_PERSONAL],
                (false, false) => vec![],
            },
            BssProtection::Wpa3Personal if has_wpa3_support() => {
                vec![SecurityDescriptor::WPA3_PERSONAL]
            }
            BssProtection::Wpa3Personal => vec![],
            // TODO(https://fxbug.dev/42174395): Implement conversions for WPA Enterprise protocols.
            BssProtection::Wpa2Enterprise | BssProtection::Wpa3Enterprise => vec![],
            BssProtection::Unknown => vec![],
        }
    }

    fn security_protocols_by_mac_role(
        &self,
        bss: &BssDescription,
    ) -> impl Iterator<Item = (SecurityDescriptor, MacRole)> {
        let has_privacy = wlan_common::mac::CapabilityInfo(bss.capability_info).privacy();
        let has_wep_support = || self.cfg.wep_supported;
        let has_wpa1_support = || self.cfg.wpa1_supported;
        let has_wpa2_support = || {
            // TODO(https://fxbug.dev/42059694): Unlike other protocols, hardware and driver
            //                                   support for WPA2 is assumed here. Query and track
            //                                   this as with other security protocols.
            has_privacy
        };
        let has_wpa3_support = || self.wpa3_supported && has_privacy;
        let client_security_protocols = Some(SecurityDescriptor::OPEN)
            .into_iter()
            .chain(has_wep_support().then_some(SecurityDescriptor::WEP))
            .chain(has_wpa1_support().then_some(SecurityDescriptor::WPA1))
            .chain(has_wpa2_support().then_some(SecurityDescriptor::WPA2_PERSONAL))
            .chain(has_wpa3_support().then_some(SecurityDescriptor::WPA3_PERSONAL))
            .map(|descriptor| (descriptor, MacRole::Client));

        let bss_security_protocols = match bss.protection() {
            BssProtection::Open => &[SecurityDescriptor::OPEN][..],
            BssProtection::OpenOweTransition => &[SecurityDescriptor::OPEN][..],
            BssProtection::Owe => &[SecurityDescriptor::OWE][..],
            BssProtection::Wep => &[SecurityDescriptor::WEP][..],
            BssProtection::Wpa1 => &[SecurityDescriptor::WPA1][..],
            BssProtection::Wpa1Wpa2PersonalTkipOnly | BssProtection::Wpa1Wpa2Personal => {
                &[SecurityDescriptor::WPA1, SecurityDescriptor::WPA2_PERSONAL][..]
            }
            BssProtection::Wpa2PersonalTkipOnly | BssProtection::Wpa2Personal => {
                &[SecurityDescriptor::WPA2_PERSONAL][..]
            }
            BssProtection::Wpa2Wpa3Personal => {
                &[SecurityDescriptor::WPA3_PERSONAL, SecurityDescriptor::WPA2_PERSONAL][..]
            }
            BssProtection::Wpa3Personal => &[SecurityDescriptor::WPA3_PERSONAL][..],
            // TODO(https://fxbug.dev/42174395): Implement conversions for WPA Enterprise protocols.
            BssProtection::Wpa2Enterprise | BssProtection::Wpa3Enterprise => &[],
            BssProtection::Unknown => &[],
        }
        .iter()
        .cloned()
        .map(|descriptor| (descriptor, MacRole::Ap));

        client_security_protocols.chain(bss_security_protocols)
    }

    fn has_compatible_channel_and_data_rates(
        &self,
        bss: &BssDescription,
        device_info: &fidl_mlme::DeviceInfo,
    ) -> bool {
        derive_join_capabilities(bss.channel, bss.rates(), device_info).is_ok()
    }
}

pub struct ClientSme {
    cfg: ClientConfig,
    state: Option<ClientState>,
    scan_sched: ScanScheduler<Responder<Result<Vec<ScanResult>, fidl_mlme::ScanResultCode>>>,
    wmm_status_responders: Vec<Responder<fidl_sme::ClientSmeWmmStatusResult>>,
    context: Context,
}

#[derive(Debug, PartialEq)]
pub enum ConnectResult {
    Success,
    Canceled,
    Failed(ConnectFailure),
}

impl<T: Into<ConnectFailure>> From<T> for ConnectResult {
    fn from(failure: T) -> Self {
        ConnectResult::Failed(failure.into())
    }
}

#[derive(Debug, PartialEq)]
pub enum RoamResult {
    Success(Box<BssDescription>),
    Failed(Box<RoamFailure>),
}

impl<T: Into<RoamFailure>> From<T> for RoamResult {
    fn from(failure: T) -> Self {
        RoamResult::Failed(Box::new(failure.into()))
    }
}

#[derive(Debug)]
pub struct ConnectTransactionSink {
    sink: UnboundedSink<ConnectTransactionEvent>,
    is_reconnecting: bool,
}

impl ConnectTransactionSink {
    pub fn new_unbounded() -> (Self, ConnectTransactionStream) {
        let (sender, receiver) = mpsc::unbounded();
        let sink =
            ConnectTransactionSink { sink: UnboundedSink::new(sender), is_reconnecting: false };
        (sink, receiver)
    }

    pub fn is_reconnecting(&self) -> bool {
        self.is_reconnecting
    }

    pub fn send_connect_result(&mut self, result: ConnectResult) {
        let event =
            ConnectTransactionEvent::OnConnectResult { result, is_reconnect: self.is_reconnecting };
        self.send(event);
    }

    pub fn send_roam_result(&mut self, result: RoamResult) {
        let event = ConnectTransactionEvent::OnRoamResult { result };
        self.send(event);
    }

    pub fn send(&mut self, event: ConnectTransactionEvent) {
        if let ConnectTransactionEvent::OnDisconnect { info } = &event {
            self.is_reconnecting = info.is_sme_reconnecting;
        };
        self.sink.send(event);
    }
}

pub type ConnectTransactionStream = mpsc::UnboundedReceiver<ConnectTransactionEvent>;

#[derive(Debug, PartialEq)]
pub enum ConnectTransactionEvent {
    OnConnectResult { result: ConnectResult, is_reconnect: bool },
    OnRoamResult { result: RoamResult },
    OnDisconnect { info: fidl_sme::DisconnectInfo },
    OnSignalReport { ind: fidl_internal::SignalReportIndication },
    OnChannelSwitched { info: fidl_internal::ChannelSwitchInfo },
}

#[derive(Debug, PartialEq)]
pub enum ConnectFailure {
    SelectNetworkFailure(SelectNetworkFailure),
    // TODO(https://fxbug.dev/42147565): SME no longer performs scans when connecting. Remove the
    //                        `ScanFailure` variant.
    ScanFailure(fidl_mlme::ScanResultCode),
    // TODO(https://fxbug.dev/42178810): `JoinFailure` and `AuthenticationFailure` no longer needed when
    //                        state machine is fully transitioned to USME.
    JoinFailure(fidl_ieee80211::StatusCode),
    AuthenticationFailure(fidl_ieee80211::StatusCode),
    AssociationFailure(AssociationFailure),
    EstablishRsnaFailure(EstablishRsnaFailure),
}

impl ConnectFailure {
    // TODO(https://fxbug.dev/42163244): ConnectFailure::is_timeout is not useful, remove it
    #[allow(clippy::collapsible_match, reason = "mass allow for https://fxbug.dev/381896734")]
    #[allow(
        clippy::match_like_matches_macro,
        reason = "mass allow for https://fxbug.dev/381896734"
    )]
    pub fn is_timeout(&self) -> bool {
        // Note: For association, we don't have a failure type for timeout, so cannot deduce
        //       whether an association failure is due to timeout.
        match self {
            ConnectFailure::AuthenticationFailure(failure) => match failure {
                fidl_ieee80211::StatusCode::RejectedSequenceTimeout => true,
                _ => false,
            },
            ConnectFailure::EstablishRsnaFailure(failure) => match failure {
                EstablishRsnaFailure {
                    reason: EstablishRsnaFailureReason::RsnaResponseTimeout(_),
                    ..
                }
                | EstablishRsnaFailure {
                    reason: EstablishRsnaFailureReason::RsnaCompletionTimeout(_),
                    ..
                } => true,
                _ => false,
            },
            _ => false,
        }
    }

    /// Returns true if failure was likely caused by rejected
    /// credentials. In some cases, we cannot be 100% certain that
    /// credentials were rejected, but it's worth noting when we
    /// observe a failure event that was more than likely caused by
    /// rejected credentials.
    pub fn likely_due_to_credential_rejected(&self) -> bool {
        match self {
            // Assuming the correct type of credentials are given, a
            // bad password will cause a variety of errors depending
            // on the security type. All of the following cases assume
            // no frames were dropped unintentionally. For example,
            // it's possible to conflate a WPA2 bad password error
            // with a dropped frame at just the right moment since the
            // error itself is *caused by* a dropped frame.

            // For WPA1 and WPA2, the error will be
            // RsnaResponseTimeout or RsnaCompletionTimeout.  When
            // the authenticator receives a bad MIC (derived from the
            // password), it will silently drop the EAPOL handshake
            // frame it received.
            //
            // NOTE: The alternative possibilities for seeing these
            // errors are an error in our crypto parameter parsing and
            // crypto implementation, or a lost connection with the AP.
            ConnectFailure::EstablishRsnaFailure(EstablishRsnaFailure {
                auth_method: Some(auth::MethodName::Psk),
                reason:
                    EstablishRsnaFailureReason::RsnaResponseTimeout(
                        wlan_rsn::Error::LikelyWrongCredential,
                    ),
            })
            | ConnectFailure::EstablishRsnaFailure(EstablishRsnaFailure {
                auth_method: Some(auth::MethodName::Psk),
                reason:
                    EstablishRsnaFailureReason::RsnaCompletionTimeout(
                        wlan_rsn::Error::LikelyWrongCredential,
                    ),
            }) => true,

            // For WEP, the entire association is always handled by
            // fullmac, so the best we can do is use
            // fidl_mlme::AssociateResultCode. The code that arises
            // when WEP fails with rejected credentials is
            // RefusedReasonUnspecified. This is a catch-all error for
            // a WEP authentication failure, but it is being
            // considered good enough for catching rejected
            // credentials for a deprecated WEP association.
            ConnectFailure::AssociationFailure(AssociationFailure {
                bss_protection: BssProtection::Wep,
                code: fidl_ieee80211::StatusCode::RefusedUnauthenticatedAccessNotSupported,
            }) => true,

            // For WPA3, the AP will not respond to SAE authentication frames
            // if it detects an invalid credential, so we expect the connection
            // attempt to time out.
            ConnectFailure::AssociationFailure(AssociationFailure {
                bss_protection: BssProtection::Wpa3Personal,
                code: fidl_ieee80211::StatusCode::RejectedSequenceTimeout,
            })
            | ConnectFailure::AssociationFailure(AssociationFailure {
                bss_protection: BssProtection::Wpa2Wpa3Personal,
                code: fidl_ieee80211::StatusCode::RejectedSequenceTimeout,
            }) => true,
            _ => false,
        }
    }

    pub fn status_code(&self) -> fidl_ieee80211::StatusCode {
        match self {
            ConnectFailure::JoinFailure(code)
            | ConnectFailure::AuthenticationFailure(code)
            | ConnectFailure::AssociationFailure(AssociationFailure { code, .. }) => *code,
            ConnectFailure::EstablishRsnaFailure(..) => {
                fidl_ieee80211::StatusCode::EstablishRsnaFailure
            }
            // SME no longer does join scan, so these two failures should no longer happen
            ConnectFailure::ScanFailure(fidl_mlme::ScanResultCode::ShouldWait) => {
                fidl_ieee80211::StatusCode::Canceled
            }
            ConnectFailure::SelectNetworkFailure(..) | ConnectFailure::ScanFailure(..) => {
                fidl_ieee80211::StatusCode::RefusedReasonUnspecified
            }
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum RoamFailureType {
    SelectNetworkFailure,
    RoamStartMalformedFailure,
    RoamResultMalformedFailure,
    RoamRequestMalformedFailure,
    RoamConfirmationMalformedFailure,
    ReassociationFailure,
    EstablishRsnaFailure,
}

#[derive(Debug, PartialEq)]
pub struct RoamFailure {
    failure_type: RoamFailureType,
    pub selected_bssid: Bssid,
    pub status_code: fidl_ieee80211::StatusCode,
    pub disconnect_info: fidl_sme::DisconnectInfo,
    auth_method: Option<auth::MethodName>,
    pub selected_bss: Option<BssDescription>,
    establish_rsna_failure_reason: Option<EstablishRsnaFailureReason>,
}

impl RoamFailure {
    /// Returns true if failure was likely caused by rejected credentials.
    /// Very similar to `ConnectFailure::likely_due_to_credential_rejected`.
    #[allow(
        clippy::match_like_matches_macro,
        reason = "mass allow for https://fxbug.dev/381896734"
    )]
    pub fn likely_due_to_credential_rejected(&self) -> bool {
        match self.failure_type {
            // WPA1 and WPA2
            RoamFailureType::EstablishRsnaFailure => match self.auth_method {
                Some(auth::MethodName::Psk) => match self.establish_rsna_failure_reason {
                    Some(EstablishRsnaFailureReason::RsnaResponseTimeout(
                        wlan_rsn::Error::LikelyWrongCredential,
                    ))
                    | Some(EstablishRsnaFailureReason::RsnaCompletionTimeout(
                        wlan_rsn::Error::LikelyWrongCredential,
                    )) => true,
                    _ => false,
                },
                _ => false,
            },
            RoamFailureType::ReassociationFailure => {
                match &self.selected_bss {
                    Some(selected_bss) => match selected_bss.protection() {
                        // WEP
                        BssProtection::Wep => match self.status_code {
                            fidl_ieee80211::StatusCode::RefusedUnauthenticatedAccessNotSupported => true,
                            _ => false,
                        },
                        // WPA3
                        BssProtection::Wpa3Personal
                        | BssProtection::Wpa2Wpa3Personal => match self.status_code {
                            fidl_ieee80211::StatusCode::RejectedSequenceTimeout => true,
                            _ => false,
                        },
                        _ => false,
                    },
                    // If selected_bss is unavailable, there's a bigger problem with the roam
                    // attempt than just a rejected credential.
                    None => false,
                }
            }
            _ => false,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum SelectNetworkFailure {
    NoScanResultWithSsid,
    IncompatibleConnectRequest,
    InternalProtectionError,
}

impl From<SelectNetworkFailure> for ConnectFailure {
    fn from(failure: SelectNetworkFailure) -> Self {
        ConnectFailure::SelectNetworkFailure(failure)
    }
}

#[derive(Debug, PartialEq)]
pub struct AssociationFailure {
    pub bss_protection: BssProtection,
    pub code: fidl_ieee80211::StatusCode,
}

impl From<AssociationFailure> for ConnectFailure {
    fn from(failure: AssociationFailure) -> Self {
        ConnectFailure::AssociationFailure(failure)
    }
}

#[derive(Debug, PartialEq)]
pub struct EstablishRsnaFailure {
    pub auth_method: Option<auth::MethodName>,
    pub reason: EstablishRsnaFailureReason,
}

#[derive(Debug, PartialEq)]
pub enum EstablishRsnaFailureReason {
    StartSupplicantFailed,
    RsnaResponseTimeout(wlan_rsn::Error),
    RsnaCompletionTimeout(wlan_rsn::Error),
    InternalError,
}

impl From<EstablishRsnaFailure> for ConnectFailure {
    fn from(failure: EstablishRsnaFailure) -> Self {
        ConnectFailure::EstablishRsnaFailure(failure)
    }
}

// Almost mirrors fidl_sme::ServingApInfo except that ServingApInfo
// contains more info here than it does in fidl_sme.
#[derive(Clone, Debug, PartialEq)]
pub struct ServingApInfo {
    pub bssid: Bssid,
    pub ssid: Ssid,
    pub rssi_dbm: i8,
    pub snr_db: i8,
    pub signal_report_time: zx::MonotonicInstant,
    pub channel: wlan_common::channel::Channel,
    pub protection: BssProtection,
    pub ht_cap: Option<fidl_ieee80211::HtCapabilities>,
    pub vht_cap: Option<fidl_ieee80211::VhtCapabilities>,
    pub probe_resp_wsc: Option<wsc::ProbeRespWsc>,
    pub wmm_param: Option<ie::WmmParam>,
}

impl From<ServingApInfo> for fidl_sme::ServingApInfo {
    fn from(ap: ServingApInfo) -> fidl_sme::ServingApInfo {
        let (cbw, secondary80_num) = ap.channel.bandwidth.to_fidl();
        fidl_sme::ServingApInfo {
            bssid: ap.bssid.to_array(),
            ssid: ap.ssid.to_vec(),
            rssi_dbm: ap.rssi_dbm,
            snr_db: ap.snr_db,
            primary: ap.channel.into(),
            protection: ap.protection.into(),
            bandwidth: cbw,
            vht_secondary_80_channel: fidl_ieee80211::ChannelNumber {
                band: ap.channel.band,
                number: secondary80_num,
            },
        }
    }
}

// TODO(https://fxbug.dev/324167674): fix.
#[derive(Clone, Debug, PartialEq)]
pub enum ClientSmeStatus {
    Connected(Box<ServingApInfo>),
    Connecting(Ssid),
    Roaming(Bssid),
    Idle,
}

impl ClientSmeStatus {
    pub fn is_connecting(&self) -> bool {
        matches!(self, ClientSmeStatus::Connecting(_))
    }

    pub fn is_connected(&self) -> bool {
        matches!(self, ClientSmeStatus::Connected(_))
    }
}

impl From<ClientSmeStatus> for fidl_sme::ClientStatusResponse {
    fn from(client_sme_status: ClientSmeStatus) -> fidl_sme::ClientStatusResponse {
        match client_sme_status {
            ClientSmeStatus::Connected(serving_ap_info) => {
                fidl_sme::ClientStatusResponse::Connected((*serving_ap_info).into())
            }
            ClientSmeStatus::Connecting(ssid) => {
                fidl_sme::ClientStatusResponse::Connecting(ssid.to_vec())
            }
            ClientSmeStatus::Roaming(bssid) => {
                fidl_sme::ClientStatusResponse::Roaming(bssid.to_array())
            }
            ClientSmeStatus::Idle => fidl_sme::ClientStatusResponse::Idle(fidl_sme::Empty {}),
        }
    }
}

impl ClientSme {
    #[allow(clippy::too_many_arguments, reason = "mass allow for https://fxbug.dev/381896734")]
    pub fn new(
        cfg: ClientConfig,
        info: fidl_mlme::DeviceInfo,
        inspector: fuchsia_inspect::Inspector,
        inspect_node: fuchsia_inspect::Node,
        security_support: fidl_common::SecuritySupport,
        spectrum_management_support: fidl_common::SpectrumManagementSupport,
    ) -> (Self, MlmeSink, MlmeStream, timer::EventStream<Event>) {
        let device_info = Arc::new(info);
        let (mlme_sink, mlme_stream) = mpsc::unbounded();
        let (mut timer, time_stream) = timer::create_timer();
        let inspect = Arc::new(inspect::SmeTree::new(
            inspector,
            inspect_node,
            &device_info,
            &spectrum_management_support,
        ));
        let _ = timer.schedule(event::InspectPulseCheck);

        (
            ClientSme {
                cfg,
                state: Some(ClientState::new(cfg)),
                scan_sched: <ScanScheduler<
                    Responder<Result<Vec<ScanResult>, fidl_mlme::ScanResultCode>>,
                >>::new(
                    Arc::clone(&device_info), spectrum_management_support
                ),
                wmm_status_responders: vec![],
                context: Context {
                    mlme_sink: MlmeSink::new(mlme_sink.clone()),
                    device_info,
                    timer,
                    att_id: 0,
                    inspect,
                    security_support,
                },
            },
            MlmeSink::new(mlme_sink),
            mlme_stream,
            time_stream,
        )
    }

    pub fn on_connect_command(
        &mut self,
        req: fidl_sme::ConnectRequest,
    ) -> ConnectTransactionStream {
        let (mut connect_txn_sink, connect_txn_stream) = ConnectTransactionSink::new_unbounded();

        // Cancel any ongoing connect attempt
        self.state = self.state.take().map(|state| state.cancel_ongoing_connect(&mut self.context));

        let bss_description: BssDescription = match req.bss_description.try_into() {
            Ok(bss_description) => bss_description,
            Err(e) => {
                error!("Failed converting FIDL BssDescription in ConnectRequest: {:?}", e);
                connect_txn_sink
                    .send_connect_result(SelectNetworkFailure::IncompatibleConnectRequest.into());
                return connect_txn_stream;
            }
        };

        info!("Received ConnectRequest for {}", bss_description);

        if self
            .cfg
            .bss_compatibility(
                &bss_description,
                &self.context.device_info,
                &self.context.security_support,
            )
            .is_err()
        {
            warn!("BSS is incompatible");
            connect_txn_sink
                .send_connect_result(SelectNetworkFailure::IncompatibleConnectRequest.into());
            return connect_txn_stream;
        }

        let authentication = req.authentication.clone();
        let protection = match SecurityAuthenticator::try_from(req.authentication)
            .map_err(From::from)
            .and_then(|authenticator| {
                Protection::try_from(SecurityContext {
                    security: &authenticator,
                    device: &self.context.device_info,
                    security_support: &self.context.security_support,
                    config: &self.cfg,
                    bss: &bss_description,
                })
            }) {
            Ok(protection) => protection,
            Err(error) => {
                warn!(
                    "{:?}",
                    format!(
                        "Failed to configure protection for network {} ({}): {:?}",
                        bss_description.ssid, bss_description.bssid, error
                    )
                );
                connect_txn_sink
                    .send_connect_result(SelectNetworkFailure::IncompatibleConnectRequest.into());
                return connect_txn_stream;
            }
        };
        let cmd = ConnectCommand {
            bss: Box::new(bss_description),
            connect_txn_sink,
            protection,
            authentication,
        };

        self.state = self.state.take().map(|state| state.connect(cmd, &mut self.context));
        connect_txn_stream
    }

    pub fn on_roam_command(&mut self, req: fidl_sme::RoamRequest) {
        if !self.status().is_connected() {
            error!("SME ignoring roam request because client is not connected");
        } else {
            self.state =
                self.state.take().map(|state| state.roam(&mut self.context, req.bss_description));
        }
    }

    pub fn on_disconnect_command(
        &mut self,
        policy_disconnect_reason: fidl_sme::UserDisconnectReason,
        responder: fidl_sme::ClientSmeDisconnectResponder,
    ) {
        self.state = self
            .state
            .take()
            .map(|state| state.disconnect(&mut self.context, policy_disconnect_reason, responder));
        self.context.inspect.update_pulse(self.status());
    }

    pub fn on_scan_command(
        &mut self,
        scan_request: fidl_sme::ScanRequest,
    ) -> oneshot::Receiver<Result<Vec<wlan_common::scan::ScanResult>, fidl_mlme::ScanResultCode>>
    {
        let (responder, receiver) = Responder::new();
        if self.status().is_connecting() {
            info!("SME ignoring scan request because a connect is in progress");
            responder.respond(Err(fidl_mlme::ScanResultCode::ShouldWait));
        } else {
            info!(
                "SME received a scan command, initiating a{} discovery scan",
                match scan_request {
                    fidl_sme::ScanRequest::Active(_) => "n active",
                    fidl_sme::ScanRequest::Passive(_) => " passive",
                }
            );
            let scan = DiscoveryScan::new(responder, scan_request);
            let req = self.scan_sched.enqueue_scan_to_discover(scan);
            self.send_scan_request(req);
        }
        receiver
    }

    pub fn on_start_scheduled_scan_command(
        &mut self,
        req: fidl_common::ScheduledScanRequest,
    ) -> (oneshot::Receiver<Result<(), i32>>, ScheduledScanReceiver) {
        let (responder, receiver) = Responder::new();
        let session =
            self.scan_sched.start_scheduled_scan(req, self.context.mlme_sink.clone(), responder);
        (receiver, session)
    }

    pub fn on_get_scheduled_scan_enabled_command(
        &mut self,
    ) -> oneshot::Receiver<Result<fidl_mlme::MlmeGetScheduledScanEnabledResponse, i32>> {
        let (responder, receiver) = Responder::new();
        self.context.mlme_sink.send(MlmeRequest::GetScheduledScanEnabled(responder));
        receiver
    }

    pub fn on_clone_inspect_vmo(&self) -> Option<fidl::Vmo> {
        self.context.inspect.clone_vmo_data()
    }

    pub fn status(&self) -> ClientSmeStatus {
        // `self.state` is always set to another state on transition and thus always present
        #[expect(clippy::expect_used)]
        self.state.as_ref().expect("expected state to be always present").status()
    }

    pub fn wmm_status(&mut self) -> oneshot::Receiver<fidl_sme::ClientSmeWmmStatusResult> {
        let (responder, receiver) = Responder::new();
        self.wmm_status_responders.push(responder);
        self.context.mlme_sink.send(MlmeRequest::WmmStatusReq);
        receiver
    }

    fn send_scan_request(&mut self, req: Option<fidl_mlme::ScanRequest>) {
        if let Some(req) = req {
            self.context.mlme_sink.send(MlmeRequest::Scan(req));
        }
    }

    pub fn query_telemetry_support(
        &mut self,
    ) -> oneshot::Receiver<Result<fidl_stats::TelemetrySupport, i32>> {
        let (responder, receiver) = Responder::new();
        self.context.mlme_sink.send(MlmeRequest::QueryTelemetrySupport(responder));
        receiver
    }

    pub fn iface_stats(&mut self) -> oneshot::Receiver<fidl_mlme::GetIfaceStatsResponse> {
        let (responder, receiver) = Responder::new();
        self.context.mlme_sink.send(MlmeRequest::GetIfaceStats(responder));
        receiver
    }

    pub fn histogram_stats(
        &mut self,
    ) -> oneshot::Receiver<fidl_mlme::GetIfaceHistogramStatsResponse> {
        let (responder, receiver) = Responder::new();
        self.context.mlme_sink.send(MlmeRequest::GetIfaceHistogramStats(responder));
        receiver
    }

    pub fn signal_report(&mut self) -> oneshot::Receiver<Result<fidl_stats::SignalReport, i32>> {
        let (responder, receiver) = Responder::new();
        self.context.mlme_sink.send(MlmeRequest::GetSignalReport(responder));
        receiver
    }

    pub fn set_mac_address(&mut self, mac_addr: [u8; 6]) -> oneshot::Receiver<Result<(), i32>> {
        let (responder, receiver) = Responder::new();
        self.context.mlme_sink.send(MlmeRequest::SetMacAddress(mac_addr, responder));
        receiver
    }

    pub fn update_mac_address(&mut self, mac_addr: [u8; 6]) {
        let mut device_info = (*self.context.device_info).clone();
        device_info.sta_addr = mac_addr;
        self.context.device_info = Arc::new(device_info);
    }

    pub fn device_info(&self) -> Arc<fidl_mlme::DeviceInfo> {
        Arc::clone(&self.context.device_info)
    }

    pub fn query_apf_packet_filter_support(
        &mut self,
    ) -> oneshot::Receiver<Result<fidl_common::ApfPacketFilterSupport, i32>> {
        let (responder, receiver) = Responder::new();
        self.context.mlme_sink.send(MlmeRequest::QueryApfPacketFilterSupport(responder));
        receiver
    }

    pub fn install_apf_packet_filter(
        &mut self,
        program: Vec<u8>,
    ) -> oneshot::Receiver<Result<(), i32>> {
        let (responder, receiver) = Responder::new();
        self.context.mlme_sink.send(MlmeRequest::InstallApfPacketFilter(
            fidl_mlme::MlmeInstallApfPacketFilterRequest { program },
            responder,
        ));
        receiver
    }

    pub fn read_apf_packet_filter_data(
        &mut self,
    ) -> oneshot::Receiver<Result<fidl_mlme::MlmeReadApfPacketFilterDataResponse, i32>> {
        let (responder, receiver) = Responder::new();
        self.context.mlme_sink.send(MlmeRequest::ReadApfPacketFilterData(responder));
        receiver
    }

    pub fn set_apf_packet_filter_enabled(
        &mut self,
        enabled: bool,
    ) -> oneshot::Receiver<Result<(), i32>> {
        let (responder, receiver) = Responder::new();
        self.context.mlme_sink.send(MlmeRequest::SetApfPacketFilterEnabled(
            fidl_mlme::MlmeSetApfPacketFilterEnabledRequest { enabled },
            responder,
        ));
        receiver
    }

    pub fn get_apf_packet_filter_enabled(
        &mut self,
    ) -> oneshot::Receiver<Result<fidl_mlme::MlmeGetApfPacketFilterEnabledResponse, i32>> {
        let (responder, receiver) = Responder::new();
        self.context.mlme_sink.send(MlmeRequest::GetApfPacketFilterEnabled(responder));
        receiver
    }
}

impl super::Station for ClientSme {
    type Event = Event;

    fn on_mlme_event(&mut self, event: fidl_mlme::MlmeEvent) {
        match event {
            fidl_mlme::MlmeEvent::OnScanResult { result } => {
                self.scan_sched
                    .on_mlme_scan_result(result)
                    .unwrap_or_else(|e| error!("scan result error: {:?}", e));
            }
            fidl_mlme::MlmeEvent::OnScanEnd { end } => {
                match self.scan_sched.on_mlme_scan_end(end, &self.context.inspect) {
                    Err(e) => error!("scan end error: {:?}", e),
                    Ok((scan_end, next_request)) => {
                        // Finalize stats for previous scan before sending scan request for
                        // the next one, which start stats collection for new scan.
                        self.send_scan_request(next_request);

                        match scan_end.result_code {
                            fidl_mlme::ScanResultCode::Success => {
                                let scan_result_list: Vec<ScanResult> = scan_end
                                    .bss_description_list
                                    .into_iter()
                                    .map(|bss_description| {
                                        self.cfg.create_scan_result(
                                            // TODO(https://fxbug.dev/42164608): ScanEnd drops the timestamp from MLME
                                            zx::MonotonicInstant::from_nanos(0),
                                            bss_description,
                                            &self.context.device_info,
                                            &self.context.security_support,
                                        )
                                    })
                                    .collect();
                                for responder in scan_end.tokens {
                                    responder.respond(Ok(scan_result_list.clone()));
                                }
                            }
                            result_code => {
                                let count = scan_end.bss_description_list.len();
                                if count > 0 {
                                    warn!("Incomplete scan with {} pending results.", count);
                                }
                                for responder in scan_end.tokens {
                                    responder.respond(Err(result_code));
                                }
                            }
                        }
                    }
                }
            }
            fidl_mlme::MlmeEvent::OnScheduledScanMatchesAvailable { txn_id } => {
                self.scan_sched.on_scheduled_scan_matches_available(
                    txn_id,
                    &self.context.inspect,
                    &self.cfg,
                    &self.context.device_info,
                    &self.context.security_support,
                );
            }
            fidl_mlme::MlmeEvent::OnScheduledScanStoppedByFirmware { txn_id } => {
                self.scan_sched.on_scheduled_scan_stopped_by_firmware(txn_id);
            }
            fidl_mlme::MlmeEvent::OnWmmStatusResp { status, resp } => {
                for responder in self.wmm_status_responders.drain(..) {
                    let result = if status == zx::sys::ZX_OK { Ok(resp) } else { Err(status) };
                    responder.respond(result);
                }
                let event = fidl_mlme::MlmeEvent::OnWmmStatusResp { status, resp };
                self.state =
                    self.state.take().map(|state| state.on_mlme_event(event, &mut self.context));
            }
            other => {
                self.state =
                    self.state.take().map(|state| state.on_mlme_event(other, &mut self.context));
            }
        };

        self.context.inspect.update_pulse(self.status());
    }

    fn on_timeout(&mut self, timed_event: timer::Event<Event>) {
        self.state = self.state.take().map(|state| match timed_event.event {
            event @ Event::RsnaCompletionTimeout(..)
            | event @ Event::RsnaResponseTimeout(..)
            | event @ Event::RsnaRetransmissionTimeout(..)
            | event @ Event::SaeTimeout(..)
            | event @ Event::DeauthenticateTimeout(..) => {
                state.handle_timeout(event, &mut self.context)
            }
            Event::InspectPulseCheck(..) => {
                self.context.mlme_sink.send(MlmeRequest::WmmStatusReq);
                let _ = self.context.timer.schedule(event::InspectPulseCheck);
                state
            }
        });

        // Because `self.status()` relies on the value of `self.state` to be present, we cannot
        // retrieve it and update pulse node inside the closure above.
        self.context.inspect.update_pulse(self.status());
    }
}

fn report_connect_finished(connect_txn_sink: &mut ConnectTransactionSink, result: ConnectResult) {
    connect_txn_sink.send_connect_result(result);
}

fn report_roam_finished(connect_txn_sink: &mut ConnectTransactionSink, result: RoamResult) {
    connect_txn_sink.send_roam_result(result);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config as SmeConfig;
    use assert_matches::assert_matches;
    use fidl_fuchsia_wlan_common as fidl_common;
    use fidl_fuchsia_wlan_mlme as fidl_mlme;
    use fuchsia_inspect as finspect;
    use ieee80211::MacAddr;
    use std::collections::HashSet;
    use std::sync::LazyLock;
    use test_case::test_case;
    use wlan_common::{
        channel::{Bandwidth, Channel},
        fake_bss_description, fake_fidl_bss_description,
        ie::{/*rsn::akm,*/ IeType, fake_ht_cap_bytes, fake_vht_cap_bytes},
        security::{wep::WEP40_KEY_BYTES, wpa::credential::PSK_SIZE_BYTES},
        test_utils::{
            fake_features::{
                fake_security_support, fake_security_support_empty,
                fake_spectrum_management_support_empty,
            },
            fake_stas::{FakeProtectionCfg, IesOverrides},
        },
    };

    use super::test_utils::{create_on_wmm_status_resp, fake_wmm_param, fake_wmm_status_resp};

    use crate::{Station, test_utils};

    static CLIENT_ADDR: LazyLock<MacAddr> =
        LazyLock::new(|| [0x7A, 0xE7, 0x76, 0xD9, 0xF2, 0x67].into());

    fn authentication_open() -> fidl_internal::Authentication {
        fidl_internal::Authentication { protocol: fidl_internal::Protocol::Open, credentials: None }
    }

    fn authentication_wep40() -> fidl_internal::Authentication {
        fidl_internal::Authentication {
            protocol: fidl_internal::Protocol::Wep,
            credentials: Some(Box::new(fidl_internal::Credentials::Wep(
                fidl_internal::WepCredentials { key: [1; WEP40_KEY_BYTES].into() },
            ))),
        }
    }

    fn authentication_wpa1_passphrase() -> fidl_internal::Authentication {
        fidl_internal::Authentication {
            protocol: fidl_internal::Protocol::Wpa1,
            credentials: Some(Box::new(fidl_internal::Credentials::Wpa(
                fidl_internal::WpaCredentials::Passphrase(b"password".as_slice().into()),
            ))),
        }
    }

    fn authentication_wpa2_personal_psk() -> fidl_internal::Authentication {
        fidl_internal::Authentication {
            protocol: fidl_internal::Protocol::Wpa2Personal,
            credentials: Some(Box::new(fidl_internal::Credentials::Wpa(
                fidl_internal::WpaCredentials::Psk([1; PSK_SIZE_BYTES]),
            ))),
        }
    }

    fn authentication_wpa2_personal_passphrase() -> fidl_internal::Authentication {
        fidl_internal::Authentication {
            protocol: fidl_internal::Protocol::Wpa2Personal,
            credentials: Some(Box::new(fidl_internal::Credentials::Wpa(
                fidl_internal::WpaCredentials::Passphrase(b"password".as_slice().into()),
            ))),
        }
    }

    fn authentication_wpa3_personal_passphrase() -> fidl_internal::Authentication {
        fidl_internal::Authentication {
            protocol: fidl_internal::Protocol::Wpa3Personal,
            credentials: Some(Box::new(fidl_internal::Credentials::Wpa(
                fidl_internal::WpaCredentials::Passphrase(b"password".as_slice().into()),
            ))),
        }
    }

    fn report_fake_scan_result(
        sme: &mut ClientSme,
        timestamp_nanos: i64,
        bss: fidl_ieee80211::BssDescription,
    ) {
        sme.on_mlme_event(fidl_mlme::MlmeEvent::OnScanResult {
            result: fidl_mlme::ScanResult { txn_id: 1, timestamp_nanos, bss },
        });
        sme.on_mlme_event(fidl_mlme::MlmeEvent::OnScanEnd {
            end: fidl_mlme::ScanEnd { txn_id: 1, code: fidl_mlme::ScanResultCode::Success },
        });
    }

    #[test_case(FakeProtectionCfg::Open)]
    #[test_case(FakeProtectionCfg::Wpa1Wpa2TkipOnly)]
    #[test_case(FakeProtectionCfg::Wpa2TkipOnly)]
    #[test_case(FakeProtectionCfg::Wpa2)]
    #[test_case(FakeProtectionCfg::Wpa2Wpa3)]
    fn default_client_protection_is_bss_compatible(protection: FakeProtectionCfg) {
        let cfg = ClientConfig::default();
        let fake_device_info = test_utils::fake_device_info([1u8; 6].into());
        assert!(
            cfg.bss_compatibility(
                &fake_bss_description!(protection => protection),
                &fake_device_info,
                &fake_security_support_empty()
            )
            .is_ok(),
        );
    }

    #[test_case(FakeProtectionCfg::Wpa1)]
    #[test_case(FakeProtectionCfg::Wpa3)]
    #[test_case(FakeProtectionCfg::Wpa3Transition)]
    #[test_case(FakeProtectionCfg::Eap)]
    fn default_client_protection_is_bss_incompatible(protection: FakeProtectionCfg) {
        let cfg = ClientConfig::default();
        let fake_device_info = test_utils::fake_device_info([1u8; 6].into());
        assert!(
            cfg.bss_compatibility(
                &fake_bss_description!(protection => protection),
                &fake_device_info,
                &fake_security_support_empty()
            )
            .is_err(),
        );
    }

    #[test_case(FakeProtectionCfg::Open)]
    #[test_case(FakeProtectionCfg::OpenOweTransition)]
    #[test_case(FakeProtectionCfg::Wpa1Wpa2TkipOnly)]
    #[test_case(FakeProtectionCfg::Wpa2TkipOnly)]
    #[test_case(FakeProtectionCfg::Wpa2)]
    #[test_case(FakeProtectionCfg::Wpa2Wpa3)]
    fn compatible_default_client_protection_security_protocol_intersection_is_non_empty(
        protection: FakeProtectionCfg,
    ) {
        let cfg = ClientConfig::default();
        assert!(
            !cfg.security_protocol_intersection(
                &fake_bss_description!(protection => protection),
                &fake_security_support_empty()
            )
            .is_empty()
        );
    }

    #[test_case(FakeProtectionCfg::Owe)]
    #[test_case(FakeProtectionCfg::Wpa1)]
    #[test_case(FakeProtectionCfg::Wpa3)]
    #[test_case(FakeProtectionCfg::Wpa3Transition)]
    #[test_case(FakeProtectionCfg::Eap)]
    fn incompatible_default_client_protection_security_protocol_intersection_is_empty(
        protection: FakeProtectionCfg,
    ) {
        let cfg = ClientConfig::default();
        assert!(
            cfg.security_protocol_intersection(
                &fake_bss_description!(protection => protection),
                &fake_security_support_empty()
            )
            .is_empty(),
        );
    }

    #[test_case(FakeProtectionCfg::Wpa1, [SecurityDescriptor::WPA1])]
    #[test_case(FakeProtectionCfg::Wpa3, [SecurityDescriptor::WPA3_PERSONAL])]
    #[test_case(FakeProtectionCfg::Wpa3Transition, [SecurityDescriptor::WPA3_PERSONAL])]
    // This BSS configuration is not specific enough to detect security protocols.
    #[test_case(FakeProtectionCfg::Eap, [])]
    fn default_client_protection_security_protocols_by_mac_role_eq(
        protection: FakeProtectionCfg,
        expected: impl IntoIterator<Item = SecurityDescriptor>,
    ) {
        let cfg = ClientConfig::default();
        let security_protocols: HashSet<_> = cfg
            .security_protocols_by_mac_role(&fake_bss_description!(protection => protection))
            .collect();
        // The protocols here are not necessarily disjoint between client and AP. Note that
        // security descriptors are less specific than BSS fixtures.
        assert_eq!(
            security_protocols,
            HashSet::from_iter(
                [
                    (SecurityDescriptor::OPEN, MacRole::Client),
                    (SecurityDescriptor::WPA2_PERSONAL, MacRole::Client),
                ]
                .into_iter()
                .chain(expected.into_iter().map(|protocol| (protocol, MacRole::Ap)))
            ),
        );
    }

    #[test]
    fn configured_client_bss_owe_compatible() {
        // OWE support is configurable.
        let cfg = ClientConfig::from_config(Config::default(), false, true);
        let mut security_support = fake_security_support_empty();
        security_support.mfp.as_mut().unwrap().supported = Some(true);
        security_support.owe.as_mut().unwrap().supported = Some(true);
        assert!(
            !cfg.security_protocol_intersection(&fake_bss_description!(Owe), &security_support)
                .is_empty()
        );
    }

    #[test]
    fn configured_client_bss_wep_compatible() {
        // WEP support is configurable.
        let cfg = ClientConfig::from_config(Config::default().with_wep(), false, false);
        assert!(
            !cfg.security_protocol_intersection(
                &fake_bss_description!(Wep),
                &fake_security_support_empty()
            )
            .is_empty()
        );
    }

    #[test]
    fn configured_client_bss_wpa1_compatible() {
        // WPA1 support is configurable.
        let cfg = ClientConfig::from_config(Config::default().with_wpa1(), false, false);
        assert!(
            !cfg.security_protocol_intersection(
                &fake_bss_description!(Wpa1),
                &fake_security_support_empty()
            )
            .is_empty()
        );
    }

    #[test]
    fn configured_client_bss_wpa3_compatible() {
        // WPA3 support is configurable.
        let cfg = ClientConfig::from_config(Config::default(), true, false);
        let mut security_support = fake_security_support_empty();
        security_support.mfp.as_mut().unwrap().supported = Some(true);
        assert!(
            !cfg.security_protocol_intersection(&fake_bss_description!(Wpa3), &security_support)
                .is_empty()
        );
        assert!(
            !cfg.security_protocol_intersection(
                &fake_bss_description!(Wpa3Transition),
                &security_support,
            )
            .is_empty()
        );
    }

    #[test]
    fn verify_rates_compatibility() {
        // Compatible rates.
        let cfg = ClientConfig::default();
        let device_info = test_utils::fake_device_info([1u8; 6].into());
        assert!(
            cfg.has_compatible_channel_and_data_rates(&fake_bss_description!(Open), &device_info)
        );

        // Compatible rates with HT BSS membership selector (`0xFF`).
        let bss = fake_bss_description!(Open, rates: vec![0x8C, 0xFF]);
        assert!(cfg.has_compatible_channel_and_data_rates(&bss, &device_info));

        // Incompatible rates.
        let bss = fake_bss_description!(Open, rates: vec![0x81]);
        assert!(!cfg.has_compatible_channel_and_data_rates(&bss, &device_info));
    }

    #[test]
    fn convert_scan_result() {
        let cfg = ClientConfig::default();
        let bss_description = fake_bss_description!(Wpa2,
            ssid: Ssid::empty(),
            bssid: [0u8; 6],
            rssi_dbm: -30,
            snr_db: 0,
            channel: Channel::new(1, Bandwidth::Cbw20, fidl_ieee80211::WlanBand::TwoGhz),
            ies_overrides: IesOverrides::new()
                .set(IeType::HT_CAPABILITIES, fake_ht_cap_bytes().to_vec())
                .set(IeType::VHT_CAPABILITIES, fake_vht_cap_bytes().to_vec()),
        );
        let device_info = test_utils::fake_device_info([1u8; 6].into());
        let timestamp = zx::MonotonicInstant::get();
        let scan_result = cfg.create_scan_result(
            timestamp,
            bss_description.clone(),
            &device_info,
            &fake_security_support(),
        );

        assert_eq!(
            scan_result,
            ScanResult {
                compatibility: Compatible::expect_ok([SecurityDescriptor::WPA2_PERSONAL]),
                timestamp,
                bss_description,
            }
        );

        let wmm_param = *ie::parse_wmm_param(&fake_wmm_param().bytes[..])
            .expect("expect WMM param to be parseable");
        let bss_description = fake_bss_description!(Wpa2,
            ssid: Ssid::empty(),
            bssid: [0u8; 6],
            rssi_dbm: -30,
            snr_db: 0,
            channel: Channel::new(1, Bandwidth::Cbw20, fidl_ieee80211::WlanBand::TwoGhz),
            wmm_param: Some(wmm_param),
            ies_overrides: IesOverrides::new()
                .set(IeType::HT_CAPABILITIES, fake_ht_cap_bytes().to_vec())
                .set(IeType::VHT_CAPABILITIES, fake_vht_cap_bytes().to_vec()),
        );
        let timestamp = zx::MonotonicInstant::get();
        let scan_result = cfg.create_scan_result(
            timestamp,
            bss_description.clone(),
            &device_info,
            &fake_security_support(),
        );

        assert_eq!(
            scan_result,
            ScanResult {
                compatibility: Compatible::expect_ok([SecurityDescriptor::WPA2_PERSONAL]),
                timestamp,
                bss_description,
            }
        );

        let bss_description = fake_bss_description!(Wep,
            ssid: Ssid::empty(),
            bssid: [0u8; 6],
            rssi_dbm: -30,
            snr_db: 0,
            channel: Channel::new(1, Bandwidth::Cbw20, fidl_ieee80211::WlanBand::TwoGhz),
            ies_overrides: IesOverrides::new()
                .set(IeType::HT_CAPABILITIES, fake_ht_cap_bytes().to_vec())
                .set(IeType::VHT_CAPABILITIES, fake_vht_cap_bytes().to_vec()),
        );
        let timestamp = zx::MonotonicInstant::get();
        let scan_result = cfg.create_scan_result(
            timestamp,
            bss_description.clone(),
            &device_info,
            &fake_security_support(),
        );
        assert_eq!(
            scan_result,
            ScanResult {
                compatibility: Incompatible::expect_err(
                    "incompatible channel, PHY data rates, or security protocols",
                    Some([
                        (SecurityDescriptor::WEP, MacRole::Ap),
                        (SecurityDescriptor::OPEN, MacRole::Client),
                        (SecurityDescriptor::WPA2_PERSONAL, MacRole::Client),
                    ])
                ),
                timestamp,
                bss_description,
            },
        );

        let cfg = ClientConfig::from_config(Config::default().with_wep(), false, false);
        let bss_description = fake_bss_description!(Wep,
            ssid: Ssid::empty(),
            bssid: [0u8; 6],
            rssi_dbm: -30,
            snr_db: 0,
            channel: Channel::new(1, Bandwidth::Cbw20, fidl_ieee80211::WlanBand::TwoGhz),
            ies_overrides: IesOverrides::new()
                .set(IeType::HT_CAPABILITIES, fake_ht_cap_bytes().to_vec())
                .set(IeType::VHT_CAPABILITIES, fake_vht_cap_bytes().to_vec()),
        );
        let timestamp = zx::MonotonicInstant::get();
        let scan_result = cfg.create_scan_result(
            timestamp,
            bss_description.clone(),
            &device_info,
            &fake_security_support(),
        );
        assert_eq!(
            scan_result,
            ScanResult {
                compatibility: Compatible::expect_ok([SecurityDescriptor::WEP]),
                timestamp,
                bss_description,
            }
        );
    }

    #[test_case(EstablishRsnaFailureReason::RsnaResponseTimeout(
        wlan_rsn::Error::LikelyWrongCredential
    ))]
    #[test_case(EstablishRsnaFailureReason::RsnaCompletionTimeout(
        wlan_rsn::Error::LikelyWrongCredential
    ))]
    fn test_connect_detection_of_rejected_wpa1_or_wpa2_credentials(
        reason: EstablishRsnaFailureReason,
    ) {
        let failure = ConnectFailure::EstablishRsnaFailure(EstablishRsnaFailure {
            auth_method: Some(auth::MethodName::Psk),
            reason,
        });
        assert!(failure.likely_due_to_credential_rejected());
    }

    #[test_case(fake_bss_description!(Wpa1), EstablishRsnaFailureReason::RsnaResponseTimeout(wlan_rsn::Error::LikelyWrongCredential))]
    #[test_case(fake_bss_description!(Wpa1), EstablishRsnaFailureReason::RsnaCompletionTimeout(wlan_rsn::Error::LikelyWrongCredential))]
    #[test_case(fake_bss_description!(Wpa1Wpa2TkipOnly), EstablishRsnaFailureReason::RsnaResponseTimeout(wlan_rsn::Error::LikelyWrongCredential))]
    #[test_case(fake_bss_description!(Wpa1Wpa2TkipOnly), EstablishRsnaFailureReason::RsnaCompletionTimeout(wlan_rsn::Error::LikelyWrongCredential))]
    #[test_case(fake_bss_description!(Wpa2), EstablishRsnaFailureReason::RsnaResponseTimeout(wlan_rsn::Error::LikelyWrongCredential))]
    #[test_case(fake_bss_description!(Wpa2), EstablishRsnaFailureReason::RsnaCompletionTimeout(wlan_rsn::Error::LikelyWrongCredential))]
    fn test_roam_detection_of_rejected_wpa1_or_wpa2_credentials(
        selected_bss: BssDescription,
        failure_reason: EstablishRsnaFailureReason,
    ) {
        let disconnect_info = fidl_sme::DisconnectInfo {
            is_sme_reconnecting: false,
            disconnect_source: fidl_sme::DisconnectSource::Mlme(fidl_sme::DisconnectCause {
                mlme_event_name: fidl_sme::DisconnectMlmeEventName::RoamResultIndication,
                reason_code: fidl_ieee80211::ReasonCode::UnspecifiedReason,
            }),
        };
        let failure = RoamFailure {
            status_code: fidl_ieee80211::StatusCode::RefusedUnauthenticatedAccessNotSupported,
            failure_type: RoamFailureType::EstablishRsnaFailure,
            selected_bssid: selected_bss.bssid,
            disconnect_info,
            auth_method: Some(auth::MethodName::Psk),
            establish_rsna_failure_reason: Some(failure_reason),
            selected_bss: Some(selected_bss),
        };
        assert!(failure.likely_due_to_credential_rejected());
    }

    #[test]
    fn test_connect_detection_of_rejected_wpa3_credentials() {
        let bss = fake_bss_description!(Wpa3);
        let failure = ConnectFailure::AssociationFailure(AssociationFailure {
            bss_protection: bss.protection(),
            code: fidl_ieee80211::StatusCode::RejectedSequenceTimeout,
        });

        assert!(failure.likely_due_to_credential_rejected());
    }

    #[test]
    fn test_roam_detection_of_rejected_wpa3_credentials() {
        let selected_bss = fake_bss_description!(Wpa3);
        let disconnect_info = fidl_sme::DisconnectInfo {
            is_sme_reconnecting: false,
            disconnect_source: fidl_sme::DisconnectSource::Mlme(fidl_sme::DisconnectCause {
                mlme_event_name: fidl_sme::DisconnectMlmeEventName::RoamResultIndication,
                reason_code: fidl_ieee80211::ReasonCode::UnspecifiedReason,
            }),
        };
        let failure = RoamFailure {
            status_code: fidl_ieee80211::StatusCode::RejectedSequenceTimeout,
            failure_type: RoamFailureType::ReassociationFailure,
            selected_bssid: selected_bss.bssid,
            disconnect_info,
            auth_method: Some(auth::MethodName::Sae),
            establish_rsna_failure_reason: None,
            selected_bss: Some(selected_bss),
        };
        assert!(failure.likely_due_to_credential_rejected());
    }

    #[test]
    fn test_connect_detection_of_rejected_wep_credentials() {
        let failure = ConnectFailure::AssociationFailure(AssociationFailure {
            bss_protection: BssProtection::Wep,
            code: fidl_ieee80211::StatusCode::RefusedUnauthenticatedAccessNotSupported,
        });
        assert!(failure.likely_due_to_credential_rejected());
    }

    #[test]
    fn test_roam_detection_of_rejected_wep_credentials() {
        let selected_bss = fake_bss_description!(Wep);
        let disconnect_info = fidl_sme::DisconnectInfo {
            is_sme_reconnecting: false,
            disconnect_source: fidl_sme::DisconnectSource::Mlme(fidl_sme::DisconnectCause {
                mlme_event_name: fidl_sme::DisconnectMlmeEventName::RoamResultIndication,
                reason_code: fidl_ieee80211::ReasonCode::UnspecifiedReason,
            }),
        };
        let failure = RoamFailure {
            status_code: fidl_ieee80211::StatusCode::RefusedUnauthenticatedAccessNotSupported,
            failure_type: RoamFailureType::ReassociationFailure,
            selected_bssid: selected_bss.bssid,
            disconnect_info,
            auth_method: Some(auth::MethodName::Psk),
            establish_rsna_failure_reason: None,
            selected_bss: Some(selected_bss),
        };
        assert!(failure.likely_due_to_credential_rejected());
    }

    #[test]
    fn test_connect_no_detection_of_rejected_wpa1_or_wpa2_credentials() {
        let failure = ConnectFailure::ScanFailure(fidl_mlme::ScanResultCode::InternalError);
        assert!(!failure.likely_due_to_credential_rejected());

        let failure = ConnectFailure::AssociationFailure(AssociationFailure {
            bss_protection: BssProtection::Wpa2Personal,
            code: fidl_ieee80211::StatusCode::RefusedUnauthenticatedAccessNotSupported,
        });
        assert!(!failure.likely_due_to_credential_rejected());
    }

    #[test_case(fake_bss_description!(Wpa1))]
    #[test_case(fake_bss_description!(Wpa1Wpa2TkipOnly))]
    #[test_case(fake_bss_description!(Wpa2))]
    fn test_roam_no_detection_of_rejected_wpa1_or_wpa2_credentials(selected_bss: BssDescription) {
        let disconnect_info = fidl_sme::DisconnectInfo {
            is_sme_reconnecting: false,
            disconnect_source: fidl_sme::DisconnectSource::Mlme(fidl_sme::DisconnectCause {
                mlme_event_name: fidl_sme::DisconnectMlmeEventName::RoamResultIndication,
                reason_code: fidl_ieee80211::ReasonCode::UnspecifiedReason,
            }),
        };
        let failure = RoamFailure {
            status_code: fidl_ieee80211::StatusCode::RefusedUnauthenticatedAccessNotSupported,
            failure_type: RoamFailureType::EstablishRsnaFailure,
            selected_bssid: selected_bss.bssid,
            disconnect_info,
            auth_method: Some(auth::MethodName::Psk),
            establish_rsna_failure_reason: Some(EstablishRsnaFailureReason::StartSupplicantFailed),
            selected_bss: Some(selected_bss),
        };
        assert!(!failure.likely_due_to_credential_rejected());
    }

    #[test]
    fn test_connect_no_detection_of_rejected_wpa3_credentials() {
        let bss = fake_bss_description!(Wpa3);
        let failure = ConnectFailure::AssociationFailure(AssociationFailure {
            bss_protection: bss.protection(),
            code: fidl_ieee80211::StatusCode::RefusedUnauthenticatedAccessNotSupported,
        });

        assert!(!failure.likely_due_to_credential_rejected());
    }

    #[test]
    fn test_roam_no_detection_of_rejected_wpa3_credentials() {
        let selected_bss = fake_bss_description!(Wpa3);
        let disconnect_info = fidl_sme::DisconnectInfo {
            is_sme_reconnecting: false,
            disconnect_source: fidl_sme::DisconnectSource::Mlme(fidl_sme::DisconnectCause {
                mlme_event_name: fidl_sme::DisconnectMlmeEventName::RoamResultIndication,
                reason_code: fidl_ieee80211::ReasonCode::UnspecifiedReason,
            }),
        };
        let failure = RoamFailure {
            status_code: fidl_ieee80211::StatusCode::RefusedUnauthenticatedAccessNotSupported,
            failure_type: RoamFailureType::ReassociationFailure,
            selected_bssid: selected_bss.bssid,
            disconnect_info,
            auth_method: Some(auth::MethodName::Sae),
            establish_rsna_failure_reason: None,
            selected_bss: Some(selected_bss),
        };
        assert!(!failure.likely_due_to_credential_rejected());
    }

    #[test]
    fn test_connect_no_detection_of_rejected_wep_credentials() {
        let failure = ConnectFailure::AssociationFailure(AssociationFailure {
            bss_protection: BssProtection::Wep,
            code: fidl_ieee80211::StatusCode::InvalidParameters,
        });
        assert!(!failure.likely_due_to_credential_rejected());
    }

    #[test]
    fn test_roam_no_detection_of_rejected_wep_credentials() {
        let selected_bss = fake_bss_description!(Wep);
        let disconnect_info = fidl_sme::DisconnectInfo {
            is_sme_reconnecting: false,
            disconnect_source: fidl_sme::DisconnectSource::Mlme(fidl_sme::DisconnectCause {
                mlme_event_name: fidl_sme::DisconnectMlmeEventName::RoamResultIndication,
                reason_code: fidl_ieee80211::ReasonCode::UnspecifiedReason,
            }),
        };
        let failure = RoamFailure {
            status_code: fidl_ieee80211::StatusCode::StatusInvalidElement,
            failure_type: RoamFailureType::ReassociationFailure,
            selected_bssid: selected_bss.bssid,
            disconnect_info,
            auth_method: Some(auth::MethodName::Psk),
            establish_rsna_failure_reason: None,
            selected_bss: Some(selected_bss),
        };
        assert!(!failure.likely_due_to_credential_rejected());
    }

    #[test_case(fake_bss_description!(Open), authentication_open() => matches Ok(Protection::Open))]
    #[test_case(fake_bss_description!(Open), authentication_wpa2_personal_passphrase() => matches Err(_))]
    #[test_case(fake_bss_description!(Wpa2), authentication_wpa2_personal_passphrase() => matches Ok(Protection::Rsna(_)))]
    #[test_case(fake_bss_description!(Wpa2), authentication_wpa2_personal_psk() => matches Ok(Protection::Rsna(_)))]
    #[test_case(fake_bss_description!(Wpa2), authentication_open() => matches Err(_))]
    fn test_protection_from_authentication(
        bss: BssDescription,
        authentication: fidl_internal::Authentication,
    ) -> Result<Protection, anyhow::Error> {
        let device = test_utils::fake_device_info(*CLIENT_ADDR);
        let security_support = fake_security_support();
        let config = Default::default();

        // Open BSS with open authentication:
        let authenticator = SecurityAuthenticator::try_from(authentication).unwrap();
        Protection::try_from(SecurityContext {
            security: &authenticator,
            device: &device,
            security_support: &security_support,
            config: &config,
            bss: &bss,
        })
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn status_connecting() {
        let (mut sme, _mlme_stream, _time_stream) = create_sme().await;
        assert_eq!(ClientSmeStatus::Idle, sme.status());

        // Issue a connect command and expect the status to change appropriately.
        let bss_description =
            fake_fidl_bss_description!(Open, ssid: Ssid::try_from("foo").unwrap());
        let _recv = sme.on_connect_command(connect_req(
            Ssid::try_from("foo").unwrap(),
            bss_description,
            authentication_open(),
        ));
        assert_eq!(ClientSmeStatus::Connecting(Ssid::try_from("foo").unwrap()), sme.status());

        // We should still be connecting to "foo", but the status should now come from the state
        // machine and not from the scanner.
        let ssid = assert_matches!(sme.state.as_ref().unwrap().status(), ClientSmeStatus::Connecting(ssid) => ssid);
        assert_eq!(Ssid::try_from("foo").unwrap(), ssid);
        assert_eq!(ClientSmeStatus::Connecting(Ssid::try_from("foo").unwrap()), sme.status());

        // As soon as connect command is issued for "bar", the status changes immediately
        let bss_description =
            fake_fidl_bss_description!(Open, ssid: Ssid::try_from("bar").unwrap());
        let _recv2 = sme.on_connect_command(connect_req(
            Ssid::try_from("bar").unwrap(),
            bss_description,
            authentication_open(),
        ));
        assert_eq!(ClientSmeStatus::Connecting(Ssid::try_from("bar").unwrap()), sme.status());
    }

    #[test]
    fn connecting_to_wep_network_supported() {
        let _executor = fuchsia_async::TestExecutor::new();
        let inspector = finspect::Inspector::default();
        let sme_root_node = inspector.root().create_child("sme");
        let (mut sme, _mlme_sink, mut mlme_stream, _time_stream) = ClientSme::new(
            ClientConfig::from_config(SmeConfig::default().with_wep(), false, false),
            test_utils::fake_device_info(*CLIENT_ADDR),
            inspector,
            sme_root_node,
            fake_security_support(),
            fake_spectrum_management_support_empty(),
        );
        assert_eq!(ClientSmeStatus::Idle, sme.status());

        // Issue a connect command and expect the status to change appropriately.
        let bss_description = fake_fidl_bss_description!(Wep, ssid: Ssid::try_from("foo").unwrap());
        let req =
            connect_req(Ssid::try_from("foo").unwrap(), bss_description, authentication_wep40());
        let _recv = sme.on_connect_command(req);
        assert_eq!(ClientSmeStatus::Connecting(Ssid::try_from("foo").unwrap()), sme.status());

        assert_matches!(mlme_stream.try_next(), Ok(Some(MlmeRequest::Connect(..))));
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_scheduled_scan_session_events() {
        let (mut sme, mut mlme_stream, _time_stream) = create_sme().await;

        let req = fidl_common::ScheduledScanRequest { ..Default::default() };

        let (receiver, mut session_event_stream) = sme.on_start_scheduled_scan_command(req.clone());

        assert_matches!(
            mlme_stream.try_next(),
            Ok(Some(MlmeRequest::StartScheduledScan(fidl_mlme::MlmeStartScheduledScanRequest { txn_id: id, req: _ }, responder))) => {
                assert_eq!(id, 1);
                responder.respond(Ok(()));
            }
        );

        let result = receiver.await.expect("receiver failed");
        assert!(result.is_ok());

        let bss = fake_bss_description!(Open, ssid: Ssid::try_from("foo").unwrap());
        sme.on_mlme_event(fidl_mlme::MlmeEvent::OnScanResult {
            result: fidl_mlme::ScanResult { txn_id: 1, timestamp_nanos: 1000, bss: bss.into() },
        });

        sme.on_mlme_event(fidl_mlme::MlmeEvent::OnScheduledScanMatchesAvailable { txn_id: 1 });

        assert_matches!(
            session_event_stream.try_next(),
            Ok(Some(scan_results)) => {
                let results = wlan_common::scan::read_vmo(scan_results).unwrap();
                assert_eq!(results.len(), 1);
                let parsed_bss = wlan_common::bss::BssDescription::try_from(results[0].bss_description.clone()).unwrap();
                assert_eq!(parsed_bss.ssid, Ssid::try_from("foo").unwrap());
            }
        );

        sme.on_mlme_event(fidl_mlme::MlmeEvent::OnScheduledScanStoppedByFirmware { txn_id: 1 });

        assert_matches!(session_event_stream.try_next(), Ok(None));
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_concurrent_scheduled_scan_sessions() {
        let (mut sme, mut mlme_stream, _time_stream) = create_sme().await;
        let req = fidl_common::ScheduledScanRequest { ..Default::default() };

        // Start session 1
        let (receiver1, mut session_event_stream1) =
            sme.on_start_scheduled_scan_command(req.clone());

        assert_matches!(
            mlme_stream.try_next(),
            Ok(Some(MlmeRequest::StartScheduledScan(fidl_mlme::MlmeStartScheduledScanRequest { txn_id: id, req: _ }, responder))) => {
                assert_eq!(id, 1);
                assert_eq!(session_event_stream1.txn_id, 1);
                responder.respond(Ok(()));
            }
        );
        let _ = receiver1.await.unwrap();

        // Start start session 2
        let (receiver2, mut session_event_stream2) =
            sme.on_start_scheduled_scan_command(req.clone());

        assert_matches!(
            mlme_stream.try_next(),
            Ok(Some(MlmeRequest::StartScheduledScan(fidl_mlme::MlmeStartScheduledScanRequest { txn_id: id, req: _ }, responder))) => {
                assert_eq!(id, 2);
                assert_eq!(session_event_stream2.txn_id, 2);
                responder.respond(Ok(()));
            }
        );
        let _ = receiver2.await.unwrap();

        // Send results for session 1
        let bss1 = fake_bss_description!(Open, ssid: Ssid::try_from("session1").unwrap());
        sme.on_mlme_event(fidl_mlme::MlmeEvent::OnScanResult {
            result: fidl_mlme::ScanResult { txn_id: 1, timestamp_nanos: 1000, bss: bss1.into() },
        });
        sme.on_mlme_event(fidl_mlme::MlmeEvent::OnScheduledScanMatchesAvailable { txn_id: 1 });

        // Verify session 1 receives results
        assert_matches!(
            session_event_stream1.try_next(),
            Ok(Some(scan_results)) => {
                let results = wlan_common::scan::read_vmo(scan_results).unwrap();
                assert_eq!(results.len(), 1);
                let parsed_bss = wlan_common::bss::BssDescription::try_from(results[0].bss_description.clone()).unwrap();
                assert_eq!(parsed_bss.ssid, Ssid::try_from("session1").unwrap());
            }
        );

        // Verify session 2 has not received results
        assert_matches!(session_event_stream2.try_next(), Err(_));

        // Stop stop session 2
        sme.on_mlme_event(fidl_mlme::MlmeEvent::OnScheduledScanStoppedByFirmware { txn_id: 2 });

        assert_matches!(session_event_stream2.try_next(), Ok(None));

        // Verify session 1 is still alive
        assert!(sme.scan_sched.scheduled_scan_receivers.contains_key(&1));
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn connecting_to_wep_network_unsupported() {
        let (mut sme, mut _mlme_stream, _time_stream) = create_sme().await;
        assert_eq!(ClientSmeStatus::Idle, sme.status());

        // Issue a connect command and expect the status to change appropriately.
        let bss_description = fake_fidl_bss_description!(Wep, ssid: Ssid::try_from("foo").unwrap());
        let req =
            connect_req(Ssid::try_from("foo").unwrap(), bss_description, authentication_wep40());
        let mut _connect_fut = sme.on_connect_command(req);
        assert_eq!(ClientSmeStatus::Idle, sme.state.as_ref().unwrap().status());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn connecting_password_supplied_for_protected_network() {
        let (mut sme, mut mlme_stream, _time_stream) = create_sme().await;
        assert_eq!(ClientSmeStatus::Idle, sme.status());

        // Issue a connect command and expect the status to change appropriately.
        let bss_description =
            fake_fidl_bss_description!(Wpa2, ssid: Ssid::try_from("foo").unwrap());
        let req = connect_req(
            Ssid::try_from("foo").unwrap(),
            bss_description,
            authentication_wpa2_personal_passphrase(),
        );
        let _recv = sme.on_connect_command(req);
        assert_eq!(ClientSmeStatus::Connecting(Ssid::try_from("foo").unwrap()), sme.status());

        assert_matches!(mlme_stream.try_next(), Ok(Some(MlmeRequest::Connect(..))));
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn connecting_psk_supplied_for_protected_network() {
        let (mut sme, mut mlme_stream, _time_stream) = create_sme().await;
        assert_eq!(ClientSmeStatus::Idle, sme.status());

        // Issue a connect command and expect the status to change appropriately.
        let bss_description =
            fake_fidl_bss_description!(Wpa2, ssid: Ssid::try_from("IEEE").unwrap());
        let req = connect_req(
            Ssid::try_from("IEEE").unwrap(),
            bss_description,
            authentication_wpa2_personal_psk(),
        );
        let _recv = sme.on_connect_command(req);
        assert_eq!(ClientSmeStatus::Connecting(Ssid::try_from("IEEE").unwrap()), sme.status());

        assert_matches!(mlme_stream.try_next(), Ok(Some(MlmeRequest::Connect(..))));
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn connecting_password_supplied_for_unprotected_network() {
        let (mut sme, mut _mlme_stream, _time_stream) = create_sme().await;
        assert_eq!(ClientSmeStatus::Idle, sme.status());

        let bss_description =
            fake_fidl_bss_description!(Open, ssid: Ssid::try_from("foo").unwrap());
        let req = connect_req(
            Ssid::try_from("foo").unwrap(),
            bss_description,
            authentication_wpa2_personal_passphrase(),
        );
        let mut connect_txn_stream = sme.on_connect_command(req);
        assert_eq!(ClientSmeStatus::Idle, sme.status());

        // User should get a message that connection failed
        assert_matches!(
            connect_txn_stream.try_next(),
            Ok(Some(ConnectTransactionEvent::OnConnectResult { result, is_reconnect: false })) => {
                assert_eq!(result, SelectNetworkFailure::IncompatibleConnectRequest.into());
            }
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn connecting_psk_supplied_for_unprotected_network() {
        let (mut sme, mut _mlme_stream, _time_stream) = create_sme().await;
        assert_eq!(ClientSmeStatus::Idle, sme.status());

        let bss_description =
            fake_fidl_bss_description!(Open, ssid: Ssid::try_from("foo").unwrap());
        let req = connect_req(
            Ssid::try_from("foo").unwrap(),
            bss_description,
            authentication_wpa2_personal_psk(),
        );
        let mut connect_txn_stream = sme.on_connect_command(req);
        assert_eq!(ClientSmeStatus::Idle, sme.state.as_ref().unwrap().status());

        // User should get a message that connection failed
        assert_matches!(
            connect_txn_stream.try_next(),
            Ok(Some(ConnectTransactionEvent::OnConnectResult { result, is_reconnect: false })) => {
                assert_eq!(result, SelectNetworkFailure::IncompatibleConnectRequest.into());
            }
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn connecting_no_password_supplied_for_protected_network() {
        let (mut sme, mut mlme_stream, _time_stream) = create_sme().await;
        assert_eq!(ClientSmeStatus::Idle, sme.status());

        let bss_description =
            fake_fidl_bss_description!(Wpa2, ssid: Ssid::try_from("foo").unwrap());
        let req =
            connect_req(Ssid::try_from("foo").unwrap(), bss_description, authentication_open());
        let mut connect_txn_stream = sme.on_connect_command(req);
        assert_eq!(ClientSmeStatus::Idle, sme.state.as_ref().unwrap().status());

        // No join request should be sent to MLME
        assert_no_connect(&mut mlme_stream);

        // User should get a message that connection failed
        assert_matches!(
            connect_txn_stream.try_next(),
            Ok(Some(ConnectTransactionEvent::OnConnectResult { result, is_reconnect: false })) => {
                assert_eq!(result, SelectNetworkFailure::IncompatibleConnectRequest.into());
            }
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn connecting_bypass_join_scan_open() {
        let (mut sme, mut mlme_stream, _time_stream) = create_sme().await;
        assert_eq!(ClientSmeStatus::Idle, sme.status());

        let bss_description =
            fake_fidl_bss_description!(Open, ssid: Ssid::try_from("bssname").unwrap());
        let req =
            connect_req(Ssid::try_from("bssname").unwrap(), bss_description, authentication_open());
        let mut connect_txn_stream = sme.on_connect_command(req);

        assert_eq!(ClientSmeStatus::Connecting(Ssid::try_from("bssname").unwrap()), sme.status());
        assert_matches!(mlme_stream.try_next(), Ok(Some(MlmeRequest::Connect(..))));
        // There should be no message in the connect_txn_stream
        assert_matches!(connect_txn_stream.try_next(), Err(_));
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn connecting_bypass_join_scan_protected() {
        let (mut sme, mut mlme_stream, _time_stream) = create_sme().await;
        assert_eq!(ClientSmeStatus::Idle, sme.status());

        let bss_description =
            fake_fidl_bss_description!(Wpa2, ssid: Ssid::try_from("bssname").unwrap());
        let req = connect_req(
            Ssid::try_from("bssname").unwrap(),
            bss_description,
            authentication_wpa2_personal_passphrase(),
        );
        let mut connect_txn_stream = sme.on_connect_command(req);

        assert_eq!(ClientSmeStatus::Connecting(Ssid::try_from("bssname").unwrap()), sme.status());
        assert_matches!(mlme_stream.try_next(), Ok(Some(MlmeRequest::Connect(..))));
        // There should be no message in the connect_txn_stream
        assert_matches!(connect_txn_stream.try_next(), Err(_));
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn connecting_bypass_join_scan_mismatched_credential() {
        let (mut sme, mut mlme_stream, _time_stream) = create_sme().await;
        assert_eq!(ClientSmeStatus::Idle, sme.status());

        let bss_description =
            fake_fidl_bss_description!(Wpa2, ssid: Ssid::try_from("bssname").unwrap());
        let req =
            connect_req(Ssid::try_from("bssname").unwrap(), bss_description, authentication_open());
        let mut connect_txn_stream = sme.on_connect_command(req);

        assert_eq!(ClientSmeStatus::Idle, sme.status());
        assert_no_connect(&mut mlme_stream);

        // User should get a message that connection failed
        assert_matches!(
            connect_txn_stream.try_next(),
            Ok(Some(ConnectTransactionEvent::OnConnectResult { result, is_reconnect: false })) => {
                assert_eq!(result, SelectNetworkFailure::IncompatibleConnectRequest.into());
            }
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn connecting_bypass_join_scan_unsupported_bss() {
        let (mut sme, mut mlme_stream, _time_stream) = create_sme().await;
        assert_eq!(ClientSmeStatus::Idle, sme.status());

        let bss_description =
            fake_fidl_bss_description!(Wpa3Enterprise, ssid: Ssid::try_from("bssname").unwrap());
        let req = connect_req(
            Ssid::try_from("bssname").unwrap(),
            bss_description,
            authentication_wpa3_personal_passphrase(),
        );
        let mut connect_txn_stream = sme.on_connect_command(req);

        assert_eq!(ClientSmeStatus::Idle, sme.status());
        assert_no_connect(&mut mlme_stream);

        // User should get a message that connection failed
        assert_matches!(
            connect_txn_stream.try_next(),
            Ok(Some(ConnectTransactionEvent::OnConnectResult { result, is_reconnect: false })) => {
                assert_eq!(result, SelectNetworkFailure::IncompatibleConnectRequest.into());
            }
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn connecting_right_credential_type_no_privacy() {
        let (mut sme, _mlme_stream, _time_stream) = create_sme().await;

        let bss_description = fake_fidl_bss_description!(
            Wpa2,
            ssid: Ssid::try_from("foo").unwrap(),
        );
        // Manually override the privacy bit since fake_fidl_bss_description!()
        // does not allow setting it directly.
        let bss_description = fidl_ieee80211::BssDescription {
            capability_info: wlan_common::mac::CapabilityInfo(bss_description.capability_info)
                .with_privacy(false)
                .0,
            ..bss_description
        };
        let mut connect_txn_stream = sme.on_connect_command(connect_req(
            Ssid::try_from("foo").unwrap(),
            bss_description,
            authentication_wpa2_personal_passphrase(),
        ));

        assert_matches!(
            connect_txn_stream.try_next(),
            Ok(Some(ConnectTransactionEvent::OnConnectResult { result, is_reconnect: false })) => {
                assert_eq!(result, SelectNetworkFailure::IncompatibleConnectRequest.into());
            }
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn connecting_mismatched_security_protocol() {
        let (mut sme, _mlme_stream, _time_stream) = create_sme().await;

        let bss_description =
            fake_fidl_bss_description!(Wpa2, ssid: Ssid::try_from("wpa2").unwrap());
        let mut connect_txn_stream = sme.on_connect_command(connect_req(
            Ssid::try_from("wpa2").unwrap(),
            bss_description,
            authentication_wep40(),
        ));
        assert_matches!(
            connect_txn_stream.try_next(),
            Ok(Some(ConnectTransactionEvent::OnConnectResult { result, is_reconnect: false })) => {
                assert_eq!(result, SelectNetworkFailure::IncompatibleConnectRequest.into());
            }
        );

        let bss_description =
            fake_fidl_bss_description!(Wpa2, ssid: Ssid::try_from("wpa2").unwrap());
        let mut connect_txn_stream = sme.on_connect_command(connect_req(
            Ssid::try_from("wpa2").unwrap(),
            bss_description,
            authentication_wpa1_passphrase(),
        ));
        assert_matches!(
            connect_txn_stream.try_next(),
            Ok(Some(ConnectTransactionEvent::OnConnectResult { result, is_reconnect: false })) => {
                assert_eq!(result, SelectNetworkFailure::IncompatibleConnectRequest.into());
            }
        );

        let bss_description =
            fake_fidl_bss_description!(Wpa3, ssid: Ssid::try_from("wpa3").unwrap());
        let mut connect_txn_stream = sme.on_connect_command(connect_req(
            Ssid::try_from("wpa3").unwrap(),
            bss_description,
            authentication_wpa2_personal_passphrase(),
        ));
        assert_matches!(
            connect_txn_stream.try_next(),
            Ok(Some(ConnectTransactionEvent::OnConnectResult { result, is_reconnect: false })) => {
                assert_eq!(result, SelectNetworkFailure::IncompatibleConnectRequest.into());
            }
        );
    }

    // Disable logging to prevent failure from emitted error logs.
    #[fuchsia::test(allow_stalls = false, logging = false)]
    async fn connecting_right_credential_type_but_short_password() {
        let (mut sme, _mlme_stream, _time_stream) = create_sme().await;

        let bss_description =
            fake_fidl_bss_description!(Wpa2, ssid: Ssid::try_from("foo").unwrap());
        let mut connect_txn_stream = sme.on_connect_command(connect_req(
            Ssid::try_from("foo").unwrap(),
            bss_description.clone(),
            fidl_internal::Authentication {
                protocol: fidl_internal::Protocol::Wpa2Personal,
                credentials: Some(Box::new(fidl_internal::Credentials::Wpa(
                    fidl_internal::WpaCredentials::Passphrase(b"nope".as_slice().into()),
                ))),
            },
        ));
        report_fake_scan_result(
            &mut sme,
            zx::MonotonicInstant::get().into_nanos(),
            bss_description,
        );

        assert_matches!(
            connect_txn_stream.try_next(),
            Ok(Some(ConnectTransactionEvent::OnConnectResult { result, is_reconnect: false })) => {
                assert_eq!(result, SelectNetworkFailure::IncompatibleConnectRequest.into());
            }
        );
    }

    // Disable logging to prevent failure from emitted error logs.
    #[fuchsia::test(allow_stalls = false, logging = false)]
    async fn new_connect_attempt_cancels_pending_connect() {
        let (mut sme, _mlme_stream, _time_stream) = create_sme().await;

        let bss_description =
            fake_fidl_bss_description!(Open, ssid: Ssid::try_from("foo").unwrap());
        let req = connect_req(
            Ssid::try_from("foo").unwrap(),
            bss_description.clone(),
            authentication_open(),
        );
        let mut connect_txn_stream1 = sme.on_connect_command(req);

        let req2 = connect_req(
            Ssid::try_from("foo").unwrap(),
            bss_description.clone(),
            authentication_open(),
        );
        let mut connect_txn_stream2 = sme.on_connect_command(req2);

        // User should get a message that first connection attempt is canceled
        assert_matches!(
            connect_txn_stream1.try_next(),
            Ok(Some(ConnectTransactionEvent::OnConnectResult {
                result: ConnectResult::Canceled,
                is_reconnect: false
            }))
        );

        // Report scan result to transition second connection attempt past scan. This is to verify
        // that connection attempt will be canceled even in the middle of joining the network
        report_fake_scan_result(
            &mut sme,
            zx::MonotonicInstant::get().into_nanos(),
            fake_fidl_bss_description!(Open, ssid: Ssid::try_from("foo").unwrap()),
        );

        let req3 = connect_req(
            Ssid::try_from("foo").unwrap(),
            bss_description.clone(),
            authentication_open(),
        );
        let mut _connect_fut3 = sme.on_connect_command(req3);

        // Verify that second connection attempt is canceled as new connect request comes in
        assert_matches!(
            connect_txn_stream2.try_next(),
            Ok(Some(ConnectTransactionEvent::OnConnectResult {
                result: ConnectResult::Canceled,
                is_reconnect: false
            }))
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_simple_scan_error() {
        let (mut sme, _mlme_strem, _time_stream) = create_sme().await;
        let mut recv =
            sme.on_scan_command(fidl_sme::ScanRequest::Passive(fidl_sme::PassiveScanRequest {
                channels: vec![],
            }));

        sme.on_mlme_event(fidl_mlme::MlmeEvent::OnScanEnd {
            end: fidl_mlme::ScanEnd {
                txn_id: 1,
                code: fidl_mlme::ScanResultCode::CanceledByDriverOrFirmware,
            },
        });

        assert_eq!(
            recv.try_recv(),
            Ok(Some(Err(fidl_mlme::ScanResultCode::CanceledByDriverOrFirmware)))
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_scan_error_after_some_results_returned() {
        let (mut sme, _mlme_strem, _time_stream) = create_sme().await;
        let mut recv =
            sme.on_scan_command(fidl_sme::ScanRequest::Passive(fidl_sme::PassiveScanRequest {
                channels: vec![],
            }));

        let mut bss = fake_fidl_bss_description!(Open, ssid: Ssid::try_from("foo").unwrap());
        bss.bssid = [3; 6];
        sme.on_mlme_event(fidl_mlme::MlmeEvent::OnScanResult {
            result: fidl_mlme::ScanResult {
                txn_id: 1,
                timestamp_nanos: zx::MonotonicInstant::get().into_nanos(),
                bss,
            },
        });
        let mut bss = fake_fidl_bss_description!(Open, ssid: Ssid::try_from("foo").unwrap());
        bss.bssid = [4; 6];
        sme.on_mlme_event(fidl_mlme::MlmeEvent::OnScanResult {
            result: fidl_mlme::ScanResult {
                txn_id: 1,
                timestamp_nanos: zx::MonotonicInstant::get().into_nanos(),
                bss,
            },
        });

        sme.on_mlme_event(fidl_mlme::MlmeEvent::OnScanEnd {
            end: fidl_mlme::ScanEnd {
                txn_id: 1,
                code: fidl_mlme::ScanResultCode::CanceledByDriverOrFirmware,
            },
        });

        // Scan results are lost when an error occurs.
        assert_eq!(
            recv.try_recv(),
            Ok(Some(Err(fidl_mlme::ScanResultCode::CanceledByDriverOrFirmware)))
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_scan_is_rejected_while_connecting() {
        let (mut sme, _mlme_strem, _time_stream) = create_sme().await;

        // Send a connect command to move SME into Connecting state
        let bss_description =
            fake_fidl_bss_description!(Open, ssid: Ssid::try_from("foo").unwrap());
        let _recv = sme.on_connect_command(connect_req(
            Ssid::try_from("foo").unwrap(),
            bss_description,
            authentication_open(),
        ));
        assert_matches!(sme.status(), ClientSmeStatus::Connecting(_));

        // Send a scan command and verify a ShouldWait response is returned
        let mut recv =
            sme.on_scan_command(fidl_sme::ScanRequest::Passive(fidl_sme::PassiveScanRequest {
                channels: vec![],
            }));
        assert_eq!(recv.try_recv(), Ok(Some(Err(fidl_mlme::ScanResultCode::ShouldWait))));
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_wmm_status_success() {
        let (mut sme, mut mlme_stream, _time_stream) = create_sme().await;
        let mut receiver = sme.wmm_status();

        assert_matches!(mlme_stream.try_next(), Ok(Some(MlmeRequest::WmmStatusReq)));

        let resp = fake_wmm_status_resp();
        #[allow(
            clippy::redundant_field_names,
            reason = "mass allow for https://fxbug.dev/381896734"
        )]
        sme.on_mlme_event(fidl_mlme::MlmeEvent::OnWmmStatusResp {
            status: zx::sys::ZX_OK,
            resp: resp,
        });

        assert_eq!(receiver.try_recv(), Ok(Some(Ok(resp))));
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_wmm_status_failed() {
        let (mut sme, mut mlme_stream, _time_stream) = create_sme().await;
        let mut receiver = sme.wmm_status();

        assert_matches!(mlme_stream.try_next(), Ok(Some(MlmeRequest::WmmStatusReq)));
        sme.on_mlme_event(create_on_wmm_status_resp(zx::sys::ZX_ERR_IO));
        assert_eq!(receiver.try_recv(), Ok(Some(Err(zx::sys::ZX_ERR_IO))));
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_query_apf_packet_filter_support() {
        let (mut sme, mut mlme_stream, _time_stream) = create_sme().await;
        let mut _receiver = sme.query_apf_packet_filter_support();
        assert_matches!(
            mlme_stream.try_next(),
            Ok(Some(MlmeRequest::QueryApfPacketFilterSupport(..)))
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_install_apf_packet_filter() {
        let (mut sme, mut mlme_stream, _time_stream) = create_sme().await;
        let program = vec![1, 2, 3];
        let mut _receiver = sme.install_apf_packet_filter(program.clone());
        let req = assert_matches!(mlme_stream.try_next(), Ok(Some(MlmeRequest::InstallApfPacketFilter(req, ..))) => req);
        assert_eq!(req.program, program);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_read_apf_packet_filter_data() {
        let (mut sme, mut mlme_stream, _time_stream) = create_sme().await;
        let mut _receiver = sme.read_apf_packet_filter_data();
        assert_matches!(mlme_stream.try_next(), Ok(Some(MlmeRequest::ReadApfPacketFilterData(..))));
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_set_apf_packet_filter_enabled() {
        let (mut sme, mut mlme_stream, _time_stream) = create_sme().await;
        let mut _receiver = sme.set_apf_packet_filter_enabled(true);
        let req = assert_matches!(mlme_stream.try_next(), Ok(Some(MlmeRequest::SetApfPacketFilterEnabled(req, ..))) => req);
        assert!(req.enabled);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_get_apf_packet_filter_enabled() {
        let (mut sme, mut mlme_stream, _time_stream) = create_sme().await;
        let mut _receiver = sme.get_apf_packet_filter_enabled();
        assert_matches!(
            mlme_stream.try_next(),
            Ok(Some(MlmeRequest::GetApfPacketFilterEnabled(..)))
        );
    }

    fn assert_no_connect(mlme_stream: &mut mpsc::UnboundedReceiver<MlmeRequest>) {
        loop {
            match mlme_stream.try_next() {
                Ok(event) => match event {
                    Some(MlmeRequest::Connect(..)) => {
                        panic!("unexpected connect request sent to MLME")
                    }
                    None => break,
                    _ => (),
                },
                Err(e) => {
                    assert_eq!(e.to_string(), "receiver channel is empty");
                    break;
                }
            }
        }
    }

    fn connect_req(
        ssid: Ssid,
        bss_description: fidl_ieee80211::BssDescription,
        authentication: fidl_internal::Authentication,
    ) -> fidl_sme::ConnectRequest {
        fidl_sme::ConnectRequest {
            ssid: ssid.to_vec(),
            bss_description,
            multiple_bss_candidates: true,
            authentication,
            deprecated_scan_type: fidl_common::ScanType::Passive,
        }
    }

    // The unused _exec parameter ensures that an executor exists for the lifetime of the SME.
    // Our internal timer implementation relies on the existence of a local executor.
    //
    // TODO(https://fxbug.dev/327499461): This function is async to ensure SME functions will
    // run in an async context and not call `wlan_common::timer::Timer::now` without an
    // executor.
    async fn create_sme() -> (ClientSme, MlmeStream, timer::EventStream<Event>) {
        let inspector = finspect::Inspector::default();
        let sme_root_node = inspector.root().create_child("sme");
        let (client_sme, _mlme_sink, mlme_stream, time_stream) = ClientSme::new(
            ClientConfig::default(),
            test_utils::fake_device_info(*CLIENT_ADDR),
            inspector,
            sme_root_node,
            fake_security_support(),
            fake_spectrum_management_support_empty(),
        );
        (client_sme, mlme_stream, time_stream)
    }
}
