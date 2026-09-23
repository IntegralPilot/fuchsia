// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use clap::{Parser, Subcommand};
use eui48::MacAddress;
use flex_fuchsia_wlan_common as wlan_common;
use flex_fuchsia_wlan_policy as wlan_policy;

#[derive(PartialEq, Copy, Clone, Debug, clap::ValueEnum)]
pub enum RoleArg {
    Client,
    Ap,
}

#[derive(PartialEq, Copy, Clone, Debug, clap::ValueEnum)]
pub enum ScanTypeArg {
    Active,
    Passive,
}

#[derive(PartialEq, Copy, Clone, Debug, clap::ValueEnum)]
pub enum SecurityTypeArg {
    None,
    Wep,
    Wpa,
    Wpa2,
    Wpa3,
}

#[derive(PartialEq, Copy, Clone, Debug, clap::ValueEnum)]
pub enum CredentialTypeArg {
    None,
    Psk,
    Password,
}

impl From<RoleArg> for wlan_common::WlanMacRole {
    fn from(arg: RoleArg) -> Self {
        match arg {
            RoleArg::Client => wlan_common::WlanMacRole::Client,
            RoleArg::Ap => wlan_common::WlanMacRole::Ap,
        }
    }
}

impl From<ScanTypeArg> for wlan_common::ScanType {
    fn from(arg: ScanTypeArg) -> Self {
        match arg {
            ScanTypeArg::Active => wlan_common::ScanType::Active,
            ScanTypeArg::Passive => wlan_common::ScanType::Passive,
        }
    }
}

impl From<SecurityTypeArg> for wlan_policy::SecurityType {
    fn from(arg: SecurityTypeArg) -> Self {
        match arg {
            SecurityTypeArg::r#None => wlan_policy::SecurityType::None,
            SecurityTypeArg::Wep => wlan_policy::SecurityType::Wep,
            SecurityTypeArg::Wpa => wlan_policy::SecurityType::Wpa,
            SecurityTypeArg::Wpa2 => wlan_policy::SecurityType::Wpa2,
            SecurityTypeArg::Wpa3 => wlan_policy::SecurityType::Wpa3,
        }
    }
}

impl From<PolicyNetworkConfig> for wlan_policy::NetworkConfig {
    fn from(arg: PolicyNetworkConfig) -> Self {
        let credential = match arg.credential_type {
            Some(CredentialTypeArg::r#None) => wlan_policy::Credential::None(wlan_policy::Empty),
            Some(CredentialTypeArg::Psk) => {
                wlan_policy::Credential::Psk(parse_psk_string(arg.credential.unwrap()))
            }
            Some(CredentialTypeArg::Password) => {
                wlan_policy::Credential::Password(arg.credential.unwrap().as_bytes().to_vec())
            }
            None => {
                // If credential type is not provided, infer it from the credential value.
                credential_from_string(arg.credential.unwrap_or_else(|| "".to_string()))
            }
        };

        let security_type = security_type_from_args(arg.security_type, &credential);

        let network_id = wlan_policy::NetworkIdentifier {
            ssid: arg.ssid.as_bytes().to_vec(),
            type_: security_type,
        };
        wlan_policy::NetworkConfig {
            id: Some(network_id),
            credential: Some(credential),
            ..Default::default()
        }
    }
}

/// Parse the hexadecimal characters to bytes if a valid PSK is provided, or panic with an error
/// message if the format is invalid.
fn parse_psk_string(credential: String) -> Vec<u8> {
    let psk_arg = credential.as_bytes().to_vec();
    hex::decode(psk_arg).expect(
        "Error: PSK must be 64 hexadecimal characters.\
        Example: \"123456789ABCDEF123456789ABCDEF123456789ABCDEF123456789ABCDEF1234\"",
    )
}

/// Build a WLAN policy FIDL type credential from a string. PSK will be given in hexadecimal.
/// This panics if the string does not represent a valid credential.
fn credential_from_string(credential: String) -> wlan_policy::Credential {
    match credential.len() {
        0 => wlan_policy::Credential::None(wlan_policy::Empty),
        0..=63 => wlan_policy::Credential::Password(credential.into_bytes()),
        64 => wlan_policy::Credential::Psk(parse_psk_string(credential)),
        65..=usize::MAX => {
            panic!(
                "Provided credential is too long. A password must be between 0 and 63 \
                characters and a PSK must be 64 hexadecimal characters. Provided \
                credential is {} characters.",
                credential.len()
            );
        }
        _ => {
            // This shouldn't happen; all possible lengths should be handled above.
            panic!("Invalid credential of length {}", credential.len())
        }
    }
}

/// Convert the security type provided as an argument, or use a default type that matches the
/// provided credential.
fn security_type_from_args(
    security_arg: Option<SecurityTypeArg>,
    credential: &wlan_policy::Credential,
) -> wlan_policy::SecurityType {
    if let Some(arg) = security_arg {
        match arg {
            SecurityTypeArg::Wep => wlan_policy::SecurityType::Wep,
            SecurityTypeArg::Wpa => wlan_policy::SecurityType::Wpa,
            SecurityTypeArg::Wpa2 => wlan_policy::SecurityType::Wpa2,
            SecurityTypeArg::Wpa3 => wlan_policy::SecurityType::Wpa3,
            SecurityTypeArg::r#None => wlan_policy::SecurityType::None,
        }
    } else {
        match credential {
            wlan_policy::Credential::None(_) => wlan_policy::SecurityType::None,
            _ => wlan_policy::SecurityType::Wpa2,
        }
    }
}

#[derive(clap::Args, Clone, Debug)]
pub struct PolicyNetworkConfig {
    #[arg(long)]
    pub ssid: String,
    #[arg(long = "security-type", value_enum, ignore_case = true)]
    pub security_type: Option<SecurityTypeArg>,
    #[arg(long = "credential-type", value_enum, ignore_case = true)]
    pub credential_type: Option<CredentialTypeArg>,
    #[arg(long)]
    pub credential: Option<String>,
}

#[derive(clap::Args, Clone, Debug)]
pub struct ForgetArgs {
    #[arg(long)]
    pub ssid: String,
    #[arg(long = "security-type", value_enum, ignore_case = true)]
    pub security_type: Option<SecurityTypeArg>,
}

impl ForgetArgs {
    pub fn parse_security(&self) -> Option<wlan_policy::SecurityType> {
        self.security_type.map(|s| s.into())
    }
}

#[derive(clap::Args, Clone, Debug)]
pub struct SaveNetworkArgs {
    #[arg(long)]
    pub ssid: String,
    #[arg(long = "security-type", value_enum, ignore_case = true)]
    pub security_type: SecurityTypeArg,
    #[arg(long = "credential-type", value_enum, ignore_case = true)]
    pub credential_type: Option<CredentialTypeArg>,
    #[arg(long)]
    pub credential: String,
}

#[derive(clap::Args, Clone, Debug)]
pub struct ConnectArgs {
    #[arg(long)]
    pub ssid: String,
    #[arg(long = "security-type", value_enum, ignore_case = true)]
    pub security_type: Option<SecurityTypeArg>,
}

#[derive(Subcommand, Clone, Debug)]
pub enum PolicyClientCmd {
    #[command(name = "connect")]
    Connect(ConnectArgs),
    #[command(name = "list-saved-networks")]
    GetSavedNetworks,
    #[command(name = "listen")]
    Listen,
    #[command(name = "forget-network")]
    ForgetNetwork(ForgetArgs),
    #[command(name = "save-network")]
    SaveNetwork(PolicyNetworkConfig),
    #[command(name = "scan")]
    ScanForNetworks,
    #[command(name = "start-client-connections")]
    StartClientConnections,
    #[command(name = "stop-client-connections")]
    StopClientConnections,
    #[command(name = "dump-config")]
    DumpConfig,
    #[command(name = "restore-config")]
    RestoreConfig { serialized_config: String },
    #[command(name = "status")]
    Status,
}

#[derive(Subcommand, Clone, Debug)]
pub enum PolicyAccessPointCmd {
    // TODO(sakuma): Allow users to specify connectivity mode and operating band.
    #[command(name = "start")]
    Start(PolicyNetworkConfig),
    #[command(name = "stop")]
    Stop(PolicyNetworkConfig),
    #[command(name = "stop-all")]
    StopAllAccessPoints,
    #[command(name = "listen")]
    Listen,
    #[command(name = "status")]
    Status,
}

#[derive(Subcommand, Clone, Debug)]
pub enum DeprecatedConfiguratorCmd {
    #[command(name = "suggest-mac")]
    SuggestAccessPointMacAddress {
        #[arg(required = true)]
        mac: MacAddress,
    },
}

#[derive(Parser, Clone, Debug)]
pub enum Opt {
    #[command(subcommand, name = "client")]
    Client(PolicyClientCmd),
    #[command(subcommand, name = "ap")]
    AccessPoint(PolicyAccessPointCmd),
    #[command(subcommand, name = "deprecated")]
    Deprecated(DeprecatedConfiguratorCmd),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests that a WEP network config will be correctly translated for save and remove network.
    #[fuchsia::test]
    fn test_construct_config_wep() {
        test_construct_config_security(wlan_policy::SecurityType::Wep, SecurityTypeArg::Wep);
    }

    /// Tests that a WPA network config will be correctly translated for save and remove network.
    #[fuchsia::test]
    fn test_construct_config_wpa() {
        test_construct_config_security(wlan_policy::SecurityType::Wpa, SecurityTypeArg::Wpa);
    }

    /// Tests that a WPA2 network config will be correctly translated for save and remove network.
    #[fuchsia::test]
    fn test_construct_config_wpa2() {
        test_construct_config_security(wlan_policy::SecurityType::Wpa2, SecurityTypeArg::Wpa2);
    }

    /// Tests that a WPA3 network config will be correctly translated for save and remove network.
    #[fuchsia::test]
    fn test_construct_config_wpa3() {
        test_construct_config_security(wlan_policy::SecurityType::Wpa3, SecurityTypeArg::Wpa3);
    }

    /// Tests that a config for an open network will be correctly translated to FIDL values for
    /// save and remove network when no security type and credential type are omitted.
    #[fuchsia::test]
    fn test_construct_config_open() {
        let open_config = PolicyNetworkConfig {
            ssid: "some_ssid".to_string(),
            security_type: None,
            credential_type: None,
            credential: Some("".to_string()),
        };
        let expected_cfg = wlan_policy::NetworkConfig {
            id: Some(wlan_policy::NetworkIdentifier {
                ssid: "some_ssid".as_bytes().to_vec(),
                type_: wlan_policy::SecurityType::None,
            }),
            credential: Some(wlan_policy::Credential::None(wlan_policy::Empty {})),
            ..Default::default()
        };
        let result_cfg = wlan_policy::NetworkConfig::from(open_config);
        assert_eq!(expected_cfg, result_cfg);
    }

    /// Tests that a config for an open network will be correctly translated to FIDL values for
    /// save and remove network when credential type and security type are specified.
    #[fuchsia::test]
    fn test_construct_config_open_with_omitted_args() {
        let open_config = PolicyNetworkConfig {
            ssid: "some_ssid".to_string(),
            security_type: Some(SecurityTypeArg::None),
            credential_type: Some(CredentialTypeArg::None),
            credential: Some("".to_string()),
        };
        let expected_cfg = wlan_policy::NetworkConfig {
            id: Some(wlan_policy::NetworkIdentifier {
                ssid: "some_ssid".as_bytes().to_vec(),
                type_: wlan_policy::SecurityType::None,
            }),
            credential: Some(wlan_policy::Credential::None(wlan_policy::Empty {})),
            ..Default::default()
        };
        let result_cfg = wlan_policy::NetworkConfig::from(open_config);
        assert_eq!(expected_cfg, result_cfg);
    }

    /// Test the case where a config is saved with SSID and password, but no security type or
    /// credential type provided. This is a common usage of the tool.
    #[fuchsia::test]
    fn test_construct_config_password_provided_no_security() {
        let password = "mypassword";
        let ssid = "some_ssid";
        let arg_config = PolicyNetworkConfig {
            ssid: ssid.to_string(),
            security_type: None,
            credential_type: None,
            credential: Some(password.to_string()),
        };
        let expected_cfg = wlan_policy::NetworkConfig {
            id: Some(wlan_policy::NetworkIdentifier {
                ssid: ssid.as_bytes().to_vec(),
                type_: wlan_policy::SecurityType::Wpa2,
            }),
            credential: Some(wlan_policy::Credential::Password(password.as_bytes().to_vec())),
            ..Default::default()
        };
        let result_cfg = wlan_policy::NetworkConfig::from(arg_config);
        assert_eq!(expected_cfg, result_cfg);
    }

    /// Test the case where a config is saved with SSID and psk, but no security type or
    /// credential type provided.
    #[fuchsia::test]
    fn test_construct_config_psk_provided_no_security() {
        let psk = "123456789ABCDEF123456789ABCDEF123456789ABCDEF123456789ABCDEF1234".to_string();
        let psk_bytes = hex::decode(psk.as_bytes().to_vec()).unwrap();
        let ssid = "some_ssid";
        let arg_config = PolicyNetworkConfig {
            ssid: ssid.to_string(),
            security_type: None,
            credential_type: None,
            credential: Some(psk),
        };
        let expected_cfg = wlan_policy::NetworkConfig {
            id: Some(wlan_policy::NetworkIdentifier {
                ssid: ssid.as_bytes().to_vec(),
                type_: wlan_policy::SecurityType::Wpa2,
            }),
            credential: Some(wlan_policy::Credential::Psk(psk_bytes)),
            ..Default::default()
        };
        let result_cfg = wlan_policy::NetworkConfig::from(arg_config);
        assert_eq!(expected_cfg, result_cfg);
    }

    /// Test that a config with a PSK will be translated correctly, including a transfer from a
    /// hex string to bytes.
    #[fuchsia::test]
    fn test_construct_config_psk() {
        // Test PSK separately since it has a unique credential
        const ASCII_ZERO: u8 = 49;
        let psk =
            String::from_utf8([ASCII_ZERO; 64].to_vec()).expect("Failed to create PSK test value");
        let wpa_config = PolicyNetworkConfig {
            ssid: "some_ssid".to_string(),
            security_type: Some(SecurityTypeArg::Wpa2),
            credential_type: Some(CredentialTypeArg::Psk),
            credential: Some(psk),
        };
        let expected_cfg = wlan_policy::NetworkConfig {
            id: Some(wlan_policy::NetworkIdentifier {
                ssid: "some_ssid".as_bytes().to_vec(),
                type_: wlan_policy::SecurityType::Wpa2,
            }),
            credential: Some(wlan_policy::Credential::Psk([17; 32].to_vec())),
            ..Default::default()
        };
        let result_cfg = wlan_policy::NetworkConfig::from(wpa_config);
        assert_eq!(expected_cfg, result_cfg);
    }

    /// Test that the given variant of security type with a password works when constructing
    /// network configs as used by save and remove network.
    fn test_construct_config_security(
        fidl_type: wlan_policy::SecurityType,
        tool_type: SecurityTypeArg,
    ) {
        let args_config = PolicyNetworkConfig {
            ssid: "some_ssid".to_string(),
            security_type: Some(tool_type),
            credential_type: Some(CredentialTypeArg::Password),
            credential: Some("some_password_here".to_string()),
        };
        let expected_cfg = wlan_policy::NetworkConfig {
            id: Some(wlan_policy::NetworkIdentifier {
                ssid: "some_ssid".as_bytes().to_vec(),
                type_: fidl_type,
            }),
            credential: Some(wlan_policy::Credential::Password(
                "some_password_here".as_bytes().to_vec(),
            )),
            ..Default::default()
        };
        let result_cfg = wlan_policy::NetworkConfig::from(args_config);
        assert_eq!(expected_cfg, result_cfg);
    }
}
