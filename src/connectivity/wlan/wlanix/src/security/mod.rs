// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! TODO(https://fxbug.dev/42084621): Types and functions in this file are taken from wlancfg. Dedupe later.

pub mod wep;

use crate::security::wep::WepKeys;
use fidl_fuchsia_wlan_wlanix as fidl_wlanix;
use ieee80211::Bssid;
use log::warn;
use std::cmp::Reverse;
use std::collections::HashSet;
use std::convert::TryFrom;
use wlan_common::scan::Compatible;
use wlan_common::security::wpa::credential::Passphrase;
use wlan_common::security::wpa::{Authentication, WpaDescriptor};
use wlan_common::security::{SecurityAuthenticator, SecurityDescriptor};

pub fn security_matches_key_mgmt(
    security: &SecurityDescriptor,
    mask: fidl_wlanix::KeyMgmtMask,
) -> bool {
    match security {
        SecurityDescriptor::Open => mask.contains(fidl_wlanix::KeyMgmtMask::NONE),
        SecurityDescriptor::Owe => mask.contains(fidl_wlanix::KeyMgmtMask::OWE),
        SecurityDescriptor::Wep => mask.contains(fidl_wlanix::KeyMgmtMask::NONE),
        SecurityDescriptor::Wpa(wpa) => match wpa {
            WpaDescriptor::Wpa1 { .. } => {
                mask.contains(fidl_wlanix::KeyMgmtMask::WPA_PSK)
                    || mask.contains(fidl_wlanix::KeyMgmtMask::WPA_PSK_SHA256)
                    || mask.contains(fidl_wlanix::KeyMgmtMask::FT_PSK)
            }
            WpaDescriptor::Wpa2 { authentication, .. } => match authentication {
                Authentication::Personal(_) => {
                    mask.contains(fidl_wlanix::KeyMgmtMask::WPA_PSK)
                        || mask.contains(fidl_wlanix::KeyMgmtMask::WPA_PSK_SHA256)
                        || mask.contains(fidl_wlanix::KeyMgmtMask::FT_PSK)
                }
                Authentication::Enterprise(_) => {
                    mask.contains(fidl_wlanix::KeyMgmtMask::WPA_EAP)
                        || mask.contains(fidl_wlanix::KeyMgmtMask::WPA_EAP_SHA256)
                        || mask.contains(fidl_wlanix::KeyMgmtMask::FT_EAP)
                        || mask.contains(fidl_wlanix::KeyMgmtMask::IEEE8021_X)
                }
            },
            WpaDescriptor::Wpa3 { authentication, .. } => match authentication {
                Authentication::Personal(_) => mask.contains(fidl_wlanix::KeyMgmtMask::SAE),
                Authentication::Enterprise(_) => {
                    mask.contains(fidl_wlanix::KeyMgmtMask::WPA_EAP)
                        || mask.contains(fidl_wlanix::KeyMgmtMask::WPA_EAP_SHA256)
                        || mask.contains(fidl_wlanix::KeyMgmtMask::FT_EAP)
                        || mask.contains(fidl_wlanix::KeyMgmtMask::IEEE8021_X)
                }
            },
        },
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Credential {
    /// The Credential type None is not set through the starnix, rather the None variant will be
    /// used if a credential is not set.
    None,
    Password(Vec<u8>),
    SaePassword(Vec<u8>),
    WepKey(WepKeys),
}

impl Credential {
    pub fn type_str(&self) -> &str {
        match self {
            Credential::None => "None",
            Credential::Password(_) => "Password",
            Credential::SaePassword(_) => "SAE password",
            Credential::WepKey(_) => "WEP keys",
        }
    }
}

pub fn get_authenticator(
    bssid: Bssid,
    compatible: Compatible,
    credential: &Credential,
    key_mgmt: Option<fidl_wlanix::KeyMgmtMask>,
) -> Option<SecurityAuthenticator> {
    let mutual_security_protocols = compatible.mutual_security_protocols().clone();
    match select_authentication_method(mutual_security_protocols.clone(), credential, key_mgmt) {
        Some(authenticator) => Some(authenticator),
        None => {
            warn!(
                "Failed to negotiate authentication for BSS ({:?}) with mutually supported
                security protocols: {:?}, and credential type: {:?}.",
                bssid,
                mutual_security_protocols,
                credential.type_str()
            );
            None
        }
    }
}

/// Binds a credential to a security protocol.
///
/// Binding constructs a `SecurityAuthenticator` that can be used to construct an SME
/// `ConnectRequest`. This function is similar to `SecurityDescriptor::bind`, but operates on the
/// Policy `Credential` type, which requires some additional logic to determine how the credential
/// data is interpreted.
///
/// Returns `None` if the given protocol is incompatible with the given credential.
fn bind_credential_to_protocol(
    protocol: SecurityDescriptor,
    credential: &Credential,
) -> Option<SecurityAuthenticator> {
    match protocol {
        SecurityDescriptor::Open => match credential {
            Credential::None => protocol.bind(None).ok(),
            _ => None,
        },
        SecurityDescriptor::Owe => match credential {
            Credential::None => protocol.bind(None).ok(),
            _ => None,
        },
        SecurityDescriptor::Wep => match credential {
            Credential::WepKey(wep_keys) => {
                let key = wep_keys.get_key();
                protocol
                    .bind(key.map(|k| k.into()))
                    .inspect_err(|&e| {
                        warn!("Error binding WEP key to get a security authenticator: {}", e);
                    })
                    .ok()
            }
            _ => None,
        },
        SecurityDescriptor::Wpa(wpa) => match wpa {
            WpaDescriptor::Wpa1 { .. } | WpaDescriptor::Wpa2 { .. } => match credential {
                Credential::Password(passphrase) => Passphrase::try_from(passphrase.as_slice())
                    .ok()
                    .and_then(|passphrase| protocol.bind(Some(passphrase.into())).ok()),
                _ => None,
            },
            WpaDescriptor::Wpa3 { .. } => match credential {
                Credential::SaePassword(passphrase) => Passphrase::try_from(passphrase.as_slice())
                    .ok()
                    .and_then(|passphrase| protocol.bind(Some(passphrase.into())).ok()),
                _ => None,
            },
        },
    }
}

/// Creates a security authenticator based on supported security protocols and credentials.
///
/// The authentication method is chosen based on the general strength of each mutually supported
/// security protocol (the protocols supported by both the local and remote stations) and the
/// compatibility of those protocols with the given credentials.
///
/// Returns `None` if no appropriate authentication method can be selected for the given protocols
/// and credentials.
pub fn select_authentication_method(
    mutual_security_protocols: HashSet<SecurityDescriptor>,
    credential: &Credential,
    key_mgmt: Option<fidl_wlanix::KeyMgmtMask>,
) -> Option<SecurityAuthenticator> {
    let mut protocols: Vec<_> = mutual_security_protocols.into_iter().collect();
    protocols.sort_by_key(|protocol| {
        Reverse(match protocol {
            SecurityDescriptor::Open => 0,
            SecurityDescriptor::Owe => 3,
            SecurityDescriptor::Wep => 1,
            SecurityDescriptor::Wpa(wpa) => match wpa {
                WpaDescriptor::Wpa1 { .. } => 2,
                WpaDescriptor::Wpa2 { .. } => 4,
                WpaDescriptor::Wpa3 { .. } => 5,
            },
        })
    });
    protocols
        .into_iter()
        .filter(|protocol| match key_mgmt {
            Some(mask) => security_matches_key_mgmt(protocol, mask),
            None => true,
        })
        .flat_map(|protocol| bind_credential_to_protocol(protocol, credential))
        .next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wlan_common::security::wpa::{Authentication, WpaAuthenticator};

    #[test]
    fn test_select_authentication_method_sae_key_mgmt() {
        let wpa2 = SecurityDescriptor::Wpa(WpaDescriptor::Wpa2 {
            cipher: None,
            authentication: Authentication::Personal(()),
        });
        let wpa3 = SecurityDescriptor::Wpa(WpaDescriptor::Wpa3 {
            cipher: None,
            authentication: Authentication::Personal(()),
        });

        let mutual_protocols: HashSet<SecurityDescriptor> = [wpa2, wpa3].into_iter().collect();

        // Client allows only WPA3 (SAE) -> should select WPA3
        let method = select_authentication_method(
            mutual_protocols,
            &Credential::SaePassword(b"password123".to_vec()),
            Some(fidl_wlanix::KeyMgmtMask::SAE),
        );
        assert!(method.is_some());
        assert!(matches!(
            method.unwrap(),
            SecurityAuthenticator::Wpa(WpaAuthenticator::Wpa3 { .. })
        ));
    }

    #[test]
    fn test_select_authentication_method_wpa2_key_mgmt() {
        let wpa2 = SecurityDescriptor::Wpa(WpaDescriptor::Wpa2 {
            cipher: None,
            authentication: Authentication::Personal(()),
        });
        let wpa3 = SecurityDescriptor::Wpa(WpaDescriptor::Wpa3 {
            cipher: None,
            authentication: Authentication::Personal(()),
        });

        let mutual_protocols: HashSet<SecurityDescriptor> = [wpa2, wpa3].into_iter().collect();

        // Client allows only WPA2 (WPA_PSK) -> should select WPA2
        let method = select_authentication_method(
            mutual_protocols,
            &Credential::Password(b"password123".to_vec()),
            Some(fidl_wlanix::KeyMgmtMask::WPA_PSK),
        );
        assert!(method.is_some());
        assert!(matches!(
            method.unwrap(),
            SecurityAuthenticator::Wpa(WpaAuthenticator::Wpa2 { .. })
        ));
    }
}
