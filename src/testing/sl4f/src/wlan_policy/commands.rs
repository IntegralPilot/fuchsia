// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::server::Facade;
use crate::wlan_policy::facade::WlanPolicyFacade;
use anyhow::{Error, format_err};
use async_trait::async_trait;
use fidl_fuchsia_wlan_policy as fidl_policy;
use log::*;
use serde_json::{Value, to_value};

#[async_trait(?Send)]
impl Facade for WlanPolicyFacade {
    async fn handle_request(&self, method: String, args: Value) -> Result<Value, Error> {
        match method.as_ref() {
            "scan_for_networks" => {
                info!(tag = "WlanPolicyFacade"; "performing scan for networks");
                let result = self.scan_for_networks().await?;
                to_value(result).map_err(|e| format_err!("error handling scan result: {}", e))
            }
            "connect" => {
                let target_ssid = parse_target_ssid(&args)?;
                let security_type = parse_security_type(&args)?;

                info!(
                    tag = "WlanPolicyFacade";
                    "performing wlan connect to SSID: {:?}", target_ssid
                );
                let result = self.connect(target_ssid, security_type).await?;
                to_value(result).map_err(|e| format_err!("error parsing connection result: {}", e))
            }
            "remove_network" => {
                let target_ssid = parse_target_ssid(&args)?;
                let security_type = parse_security_type(&args)?;
                info!(tag = "WlanPolicyFacade"; "removing network with SSID: {:?}", target_ssid);
                let result = self.remove_network(target_ssid, security_type).await?;
                to_value(result)
                    .map_err(|e| format_err!("error parsing remove network result: {}", e))
            }
            "start_client_connections" => {
                info!(tag = "WlanPolicyFacade"; "attempting to start client connections");
                let result = self.start_client_connections().await?;
                to_value(result).map_err(|e| {
                    format_err!("error handling start client connections result: {}", e)
                })
            }
            "stop_client_connections" => {
                info!(tag = "WlanPolicyFacade"; "attempting to stop client connections");
                let result = self.stop_client_connections().await?;
                to_value(result).map_err(|e| {
                    format_err!("error handling stop client connections result: {}", e)
                })
            }
            "save_network" => {
                let target_ssid = parse_target_ssid(&args)?;
                let security_type = parse_security_type(&args)?;
                let target_pwd = parse_target_pwd(&args)?;

                info!(tag = "WlanPolicyFacade"; "saving network with SSID: {:?}", target_ssid);
                let result = self.save_network(target_ssid, security_type, target_pwd).await?;
                to_value(result)
                    .map_err(|e| format_err!("error parsing save network result: {}", e))
            }
            "get_saved_networks" => {
                info!(tag = "WlanPolicyFacade"; "attempting to get saved networks");
                let result = self.get_saved_networks_json().await?;
                to_value(result)
                    .map_err(|e| format_err!("error handling get saved networks result: {}", e))
            }
            "create_client_controller" => {
                info!(tag = "WlanPolicyFacade"; "initializing client controller");
                let result = self.create_client_controller().await?;
                to_value(result)
                    .map_err(|e| format_err!("error initializing client controller: {}", e))
            }
            "drop_client_controller" => {
                info!(tag = "WlanPolicyFacade"; "dropping client controller");
                let result = self.drop_client_controller();
                to_value(result).map_err(|e| format_err!("error dropping client controller: {}", e))
            }
            "remove_all_networks" => {
                info!(tag = "WlanPolicyFacade"; "Removing all saved client network configs");
                let result = self.remove_all_networks().await?;
                to_value(result)
                    .map_err(|e| format_err!("error removing all saved networks: {}", e))
            }
            "get_update" => {
                info!(tag = "WlanPolicyFacade"; "getting client update");
                let result = self.get_update().await?;
                to_value(result).map_err(|e| format_err!("error handling listener update: {}", e))
            }
            "set_new_update_listener" => {
                info!(tag = "WlanPolicyFacade"; "initializing new update listener");
                let result = self.set_new_listener()?;
                to_value(result)
                    .map_err(|e| format_err!("error initializing new update listener: {}", e))
            }
            _ => return Err(format_err!("unsupported command!")),
        }
    }
}

fn parse_target_ssid(args: &Value) -> Result<Vec<u8>, Error> {
    args.get("target_ssid")
        .and_then(|ssid| ssid.as_str().map(|ssid| ssid.as_bytes().to_vec()))
        .ok_or_else(|| format_err!("Please provide a target ssid"))
}

/// In ACTS tests we will require a security type is specified for a call that uses a security
/// type; none specified will not default to a none security type.
fn parse_security_type(args: &Value) -> Result<fidl_policy::SecurityType, Error> {
    let security_type = match args.get("security_type") {
        Some(Value::String(security)) => security.as_bytes().to_vec(),
        Some(value) => {
            info!(tag = "WlanFacade"; "Please check provided security type, must be String");
            bail!("provided security type arg is not a string, cannot parse {}", value);
        }
        None => {
            info!(tag = "WlanFacade"; "Please check provided security type, none found");
            bail!("no security type is provided");
        }
    };

    // Parse network ID to connect to. The string is made lower case upstream in the pipeline.
    match std::str::from_utf8(&security_type)? {
        "none" => Ok(fidl_policy::SecurityType::None),
        "wep" => Ok(fidl_policy::SecurityType::Wep),
        "wpa" => Ok(fidl_policy::SecurityType::Wpa),
        "wpa2" => Ok(fidl_policy::SecurityType::Wpa2),
        "wpa3" => Ok(fidl_policy::SecurityType::Wpa3),
        _ => Err(format_err!("failed to parse security type (None, WEP, WPA, WPA2, or WPA3")),
    }
}

/// Parse the credential argument. The credential argument must be a string. No credential (for an
/// open network) must be indicated by an empty string. Tests may omit the password argument, and
/// if so an empty string will be provided as a default value as an argument. Tests do not need
/// to specify the type of credential; it will be inferred by the length.
/// PSK format must be string representation of hexadecimal, not the 32 bytes representation.
/// PSK will be distinguished from password by the length of password, if 64 bytes it will be PSK.
fn parse_target_pwd(args: &Value) -> Result<fidl_policy::Credential, Error> {
    let target_pwd = match args.get("target_pwd") {
        Some(Value::String(pwd)) => pwd.as_bytes().to_vec(),
        Some(value) => {
            info!(tag = "WlanFacade"; "Please check provided credential, must be String");
            bail!("provided credential is not a string, cannot parse {}", value);
        }
        None => {
            info!(tag = "WlanFacade"; "Please check provided credential, none provided");
            bail!("no credential argument provided");
        }
    };

    const PSK_LEN: usize = 64;
    let credential = match target_pwd.len() {
        0 => fidl_policy::Credential::None(fidl_policy::Empty),
        PSK_LEN => {
            let psk = hex::decode(target_pwd).map_err(|e| {
                info!(
                    tag = "WlanFacade";
                    "Please check provided credential, PSK must be valid hexadecimal string"
                );
                format_err!("provided credential length matches PSK, failed to decode: {:?}", e)
            })?;
            fidl_policy::Credential::Psk(psk)
        }
        _ => fidl_policy::Credential::Password(target_pwd),
    };
    Ok(credential)
}
