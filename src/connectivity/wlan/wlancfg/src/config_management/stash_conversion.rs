// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::network_config::{Credential, NetworkConfig, NetworkIdentifier, SecurityType};
use crate::client::types as client_types;
use std::collections::HashMap;
use wlan_storage::policy as storage;

impl From<storage::NetworkIdentifier> for NetworkIdentifier {
    fn from(item: storage::NetworkIdentifier) -> Self {
        Self {
            ssid: client_types::Ssid::from_bytes_unchecked(item.ssid),
            security_type: item.security_type.into(),
        }
    }
}

impl From<NetworkIdentifier> for storage::NetworkIdentifier {
    fn from(item: NetworkIdentifier) -> Self {
        Self { ssid: item.ssid.into(), security_type: item.security_type.into() }
    }
}

impl From<storage::SecurityType> for SecurityType {
    fn from(item: storage::SecurityType) -> Self {
        match item {
            storage::SecurityType::None => SecurityType::None,
            storage::SecurityType::Wep => SecurityType::Wep,
            storage::SecurityType::Wpa => SecurityType::Wpa,
            storage::SecurityType::Wpa2 => SecurityType::Wpa2,
            storage::SecurityType::Wpa3 => SecurityType::Wpa3,
        }
    }
}

impl From<SecurityType> for storage::SecurityType {
    fn from(item: SecurityType) -> Self {
        match item {
            SecurityType::None => storage::SecurityType::None,
            SecurityType::Wep => storage::SecurityType::Wep,
            SecurityType::Wpa => storage::SecurityType::Wpa,
            SecurityType::Wpa2 => storage::SecurityType::Wpa2,
            SecurityType::Wpa3 => storage::SecurityType::Wpa3,
        }
    }
}

impl From<storage::Credential> for Credential {
    fn from(item: storage::Credential) -> Self {
        match item {
            storage::Credential::None => Credential::None,
            storage::Credential::Password(pass) => Credential::Password(pass),
            storage::Credential::Psk(psk) => Credential::Psk(psk),
        }
    }
}

impl From<Credential> for storage::Credential {
    fn from(item: Credential) -> Self {
        match item {
            Credential::None => storage::Credential::None,
            Credential::Password(pass) => storage::Credential::Password(pass),
            Credential::Psk(psk) => storage::Credential::Psk(psk),
        }
    }
}

impl From<NetworkConfig> for storage::PersistentStorageData {
    fn from(item: NetworkConfig) -> Self {
        Self {
            ssid: item.ssid.to_vec(),
            security_type: item.security_type.into(),
            credential: item.credential.into(),
            has_ever_connected: item.has_ever_connected,
            hidden_probability: if item.hidden_probability
                == super::network_config::PROB_HIDDEN_DEFAULT
            {
                None
            } else {
                Some(item.hidden_probability)
            },
        }
    }
}

/// Convert configs to the persistent data for configs. It takes in a map of NetworkConfigs
/// corresponding to the way network configs are internally tracked.
pub fn persistent_data_from_config_map(
    network_config: &HashMap<NetworkIdentifier, NetworkConfig>,
) -> Vec<storage::PersistentStorageData> {
    network_config.values().map(|c| c.clone().into()).collect()
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;
    use test_case::test_case;
    use wlan_storage::policy as storage;

    #[fuchsia::test]
    fn network_identifier_to_stash_from_policy() {
        assert_eq!(
            storage::NetworkIdentifier::from(NetworkIdentifier {
                ssid: client_types::Ssid::try_from("ssid").unwrap(),
                security_type: SecurityType::Wep,
            }),
            storage::NetworkIdentifier {
                ssid: client_types::Ssid::try_from("ssid").unwrap().into(),
                security_type: storage::SecurityType::Wep,
            }
        );
    }

    #[fuchsia::test]
    fn network_identifier_to_policy_from_stash() {
        assert_eq!(
            NetworkIdentifier::from(storage::NetworkIdentifier {
                ssid: client_types::Ssid::try_from("ssid").unwrap().into(),
                security_type: storage::SecurityType::Wep,
            }),
            NetworkIdentifier {
                ssid: client_types::Ssid::try_from("ssid").unwrap(),
                security_type: SecurityType::Wep
            }
        );
    }

    #[fuchsia::test]
    fn security_type_to_stash_from_policy() {
        assert_eq!(storage::SecurityType::from(SecurityType::None), storage::SecurityType::None);
        assert_eq!(storage::SecurityType::from(SecurityType::Wep), storage::SecurityType::Wep);
        assert_eq!(storage::SecurityType::from(SecurityType::Wpa), storage::SecurityType::Wpa);
        assert_eq!(storage::SecurityType::from(SecurityType::Wpa2), storage::SecurityType::Wpa2);
        assert_eq!(storage::SecurityType::from(SecurityType::Wpa3), storage::SecurityType::Wpa3);
    }

    #[fuchsia::test]
    fn security_type_to_policy_from_stash() {
        assert_eq!(SecurityType::from(storage::SecurityType::None), SecurityType::None);
        assert_eq!(SecurityType::from(storage::SecurityType::Wep), SecurityType::Wep);
        assert_eq!(SecurityType::from(storage::SecurityType::Wpa), SecurityType::Wpa);
        assert_eq!(SecurityType::from(storage::SecurityType::Wpa2), SecurityType::Wpa2);
        assert_eq!(SecurityType::from(storage::SecurityType::Wpa3), SecurityType::Wpa3);
    }

    #[fuchsia::test]
    fn credential_to_stash_from_policy() {
        assert_eq!(storage::Credential::from(Credential::None), storage::Credential::None);
        assert_eq!(
            storage::Credential::from(Credential::Password(b"foo_pass123".to_vec())),
            storage::Credential::Password(b"foo_pass123".to_vec())
        );
        assert_eq!(
            storage::Credential::from(Credential::Psk(b"foo_psk123".to_vec())),
            storage::Credential::Psk(b"foo_psk123".to_vec())
        );
    }

    #[fuchsia::test]
    fn credential_to_policy_from_stash() {
        assert_eq!(Credential::from(storage::Credential::None), Credential::None);
        assert_eq!(
            Credential::from(storage::Credential::Password(b"foo_pass123".to_vec())),
            Credential::Password(b"foo_pass123".to_vec())
        );
        assert_eq!(
            Credential::from(storage::Credential::Password(b"foo_psk123".to_vec())),
            Credential::Password(b"foo_psk123".to_vec())
        );
    }

    #[test_case(true)]
    #[test_case(false)]
    #[fuchsia::test(add_test_attr = false)]
    fn persistent_data_from_network_config_has_connected(has_ever_connected: bool) {
        // Check that from() works when has_ever_connected is true and false.
        let ssid = "ssid";
        let security_type = SecurityType::Wpa3;
        let stash_security = storage::SecurityType::Wpa3;
        let id = NetworkIdentifier::try_from(ssid, security_type).unwrap();
        let credential = Credential::Password(b"foo_pass".to_vec());
        let network_config = NetworkConfig::new(id, credential, has_ever_connected, None)
            .expect("failed to create network config");
        assert_eq!(
            storage::PersistentStorageData::from(network_config),
            storage::PersistentStorageData {
                ssid: ssid.as_bytes().to_vec(),
                security_type: stash_security,
                credential: storage::Credential::Password(b"foo_pass".to_vec()),
                has_ever_connected,
                hidden_probability: None,
            }
        );
    }

    #[test_case(SecurityType::Wpa, storage::SecurityType::Wpa)]
    #[fuchsia::test(add_test_attr = false)]
    fn persistent_data_from_network_config_security(
        security: SecurityType,
        storage_security: storage::SecurityType,
    ) {
        // Check that from() works for different security types
        let ssid = "ssid";
        let id = NetworkIdentifier::try_from(ssid, security).unwrap();
        let has_ever_connected = true;

        // Use a password for WPA, PSK for WEP, or None for open networks.
        let (credential, storage_credential) = match security {
            SecurityType::None => (Credential::None, storage::Credential::None),
            SecurityType::Wep => {
                let psk_bytes = vec![1; network_config::WPA_PSK_BYTE_LEN];
                (Credential::Psk(psk_bytes.clone()), storage::Credential::Psk(psk_bytes))
            }
            SecurityType::Wpa | SecurityType::Wpa2 | SecurityType::Wpa3 => (
                Credential::Password(b"foo_pass".to_vec()),
                storage::Credential::Password(b"foo_pass".to_vec()),
            ),
        };

        let network_config = NetworkConfig::new(id, credential, has_ever_connected, None)
            .expect("failed to create network config");
        assert_eq!(
            storage::PersistentStorageData::from(network_config),
            storage::PersistentStorageData {
                ssid: ssid.as_bytes().to_vec(),
                security_type: storage_security,
                credential: storage_credential,
                has_ever_connected,
                hidden_probability: None,
            }
        );
    }

    #[fuchsia::test]
    fn test_persistent_data_from_network_config_non_default_hidden_probability() {
        let id = NetworkIdentifier::try_from("ssid", SecurityType::Wpa3).unwrap();
        let mut network_config =
            NetworkConfig::new(id, Credential::Password(b"foo_pass".to_vec()), true, None)
                .expect("failed to create network config");
        network_config.hidden_probability = 0.05;
        assert_eq!(
            storage::PersistentStorageData::from(network_config),
            storage::PersistentStorageData {
                ssid: b"ssid".to_vec(),
                security_type: storage::SecurityType::Wpa3,
                credential: storage::Credential::Password(b"foo_pass".to_vec()),
                has_ever_connected: true,
                hidden_probability: Some(0.05),
            }
        );
    }
}
