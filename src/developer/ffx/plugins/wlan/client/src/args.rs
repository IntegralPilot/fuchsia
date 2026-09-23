// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use fdomain_fuchsia_wlan_policy as wlan_policy;
use ffx_core::ffx_command;
use ffx_wlan_common::args::{CredentialType, SecurityType};

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "client", description = "Controls WLAN client policy API.")]
pub struct ClientCommand {
    #[argh(subcommand)]
    pub subcommand: ClientSubCommand,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand)]
pub enum ClientSubCommand {
    BatchConfig(BatchConfig),
    Connect(Connect),
    Listen(Listen),
    List(ListSavedNetworks),
    ForgetNetwork(ForgetNetwork),
    SaveNetwork(SaveNetwork),
    Scan(Scan),
    Start(StartClientConnections),
    Status(Status),
    Stop(StopClientConnections),
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "batch-config",
    description = "Allows WLAN credentials to be extracted and restored."
)]
pub struct BatchConfig {
    #[argh(subcommand)]
    pub subcommand: BatchConfigSubCommand,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand)]
pub enum BatchConfigSubCommand {
    Dump(Dump),
    Restore(Restore),
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "listen",
    description = "Listens for policy client connections updates",
    example = "To begin listening for client events

    $ ffx wlan client listen"
)]
pub struct Listen {}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "status",
    description = "Provides the first available client status update",
    example = "To query client status

    $ ffx wlan client status"
)]
pub struct Status {}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "list-saved-networks",
    description = "Lists all networks saved by the WLAN policy layer.",
    example = "To list saved networks

    $ ffx wlan client list-saved-networks",
    note = "Only one application at a time can interact with the WLAN policy
layer."
)]
pub struct ListSavedNetworks {}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "scan",
    description = "Scan for nearby WLAN networks.",
    example = "To scan

    $ ffx wlan client scan",
    note = "Only one application at a time can interact with the WLAN policy
layer."
)]
pub struct Scan {}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "start",
    description = "Allows wlancfg to automate WLAN client operation",
    example = "To start client connections

    $ ffx wlan client start",
    note = "Only one application at a time can interact with the WLAN policy
layer."
)]
pub struct StartClientConnections {}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "stop",
    description = "Stops automated WLAN policy control of client interfaces and
destroys all client interfaces.",
    example = "To stop client connections

    $ ffx wlan client stop",
    note = "Only one application at a time can interact with the WLAN policy
layer."
)]
pub struct StopClientConnections {}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "dump",
    description = "Extracts a structured representation of the device's saved WLAN credentials.",
    example = "To dump WLAN client configs

    $ ffx wlan client batch-config dump",
    note = "Only one application at a time can interact with the WLAN policy layer."
)]
pub struct Dump {}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "restore",
    description = "Injects a structure representation of WLAN credentials into a device.",
    example = "To restore WLAN client configs

    $ ffx wlan client batch-config restore <STRUCTURE_CONFIG_DATA>",
    note = "Only one application at a time can interact with the WLAN policy layer."
)]
pub struct Restore {
    #[argh(positional, description = "structured representation of WLAN credentials.")]
    pub serialized_config: String,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "forget-network",
    description = "WLAN policy network storage container",
    note = "Only one application at a time can interact with the WLAN policy layer.",
    example = "To forget a WLAN network

    $ffx wlan client forget-network\n
        --ssid TestNetwork\n
        --security-type wpa2"
)]
pub struct ForgetNetwork {
    #[argh(option, default = "String::from(\"\")", description = "WLAN network name")]
    pub ssid: String,
    #[argh(option, description = "one of None, WEP, WPA, WPA2, WPA3")]
    pub security_type: Option<SecurityType>,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "save-network",
    description = "WLAN policy network storage container",
    note = "Only one application at a time can interact with the WLAN policy layer.",
    example = "To save a WLAN network

    $ffx wlan client save-network\n
        --ssid TestNetwork\n
        --security-type wpa2\n
        --credential-type password\n
        --credential \"Your very secure password here\""
)]
pub struct SaveNetwork {
    #[argh(option, default = "String::from(\"\")", description = "WLAN network name")]
    pub ssid: String,
    #[argh(
        option,
        default = "SecurityType::None",
        description = "one of None, WEP, WPA, WPA2, WPA3"
    )]
    pub security_type: SecurityType,
    #[argh(option, default = "CredentialType::None", description = "one of None, PSK, Password")]
    pub credential_type: CredentialType,
    #[argh(option, default = "String::from(\"\")", description = "WLAN Password or PSK")]
    pub credential: String,
}

impl From<SaveNetwork> for wlan_policy::NetworkConfig {
    fn from(arg: SaveNetwork) -> Self {
        ffx_wlan_common::args::config_from_args(
            arg.ssid,
            arg.security_type,
            arg.credential_type,
            arg.credential,
        )
    }
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "connect",
    description = "Connect to the specified WLAN network",
    note = "Only one application at a time can interact with the WLAN policy layer.",
    example = "To remove a WLAN network

    $ffx wlan client connect\n
        --ssid TestNetwork\n
        --security-type wpa2"
)]
pub struct Connect {
    #[argh(option, default = "String::from(\"\")", description = "WLAN network name")]
    pub ssid: String,
    #[argh(option, description = "one of None, WEP, WPA, WPA2, WPA3")]
    pub security_type: Option<SecurityType>,
}
