// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;
use fidl_fuchsia_bluetooth_sys::TechnologyType;
use std::str::FromStr;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "peer",
    description = "Show details for a known peer.",
    example = "ffx bluetooth peer"
)]
pub struct PeerCommand {
    #[argh(subcommand)]
    pub subcommand: PeerSubCommand,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(subcommand)]
pub enum PeerSubCommand {
    List(ListCommand),
    Show(ShowCommand),
    Connect(ConnectCommand),
    Disconnect(DisconnectCommand),
    Forget(ForgetCommand),
    Pair(PairCommand),
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(
    subcommand,
    name = "list",
    description = "Show all known peers in a summarized view (optionally filtered).",
    example = "ffx bluetooth peer list <filter>"
)]
pub struct ListCommand {
    /// filter all known peers by id, address, or name (case-insensitive)
    #[argh(positional)]
    pub filter: Option<String>,

    /// show details for all known peers
    #[argh(switch)]
    pub details: bool,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(
    subcommand,
    name = "show",
    description = "Show details for a known peer.",
    example = "ffx bluetooth peer show <id|addr>"
)]
pub struct ShowCommand {
    #[argh(positional)]
    pub id_or_addr: ffx_bluetooth_common::PeerIdOrAddr,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(
    subcommand,
    name = "connect",
    description = "Send a connection request to a peer. To start a pairing delegate first, use \
`--with-pairing`.",
    example = "ffx bluetooth peer connect <id|addr>"
)]
pub struct ConnectCommand {
    #[argh(positional)]
    pub id_or_addr: ffx_bluetooth_common::PeerIdOrAddr,

    /// start a pairing delegate before connecting. Use this when connecting to unbonded devices to
    /// ensure the connection isn't rejected when pairing is required
    #[argh(switch, long = "with-pairing")]
    pub with_pairing: bool,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(
    subcommand,
    name = "disconnect",
    description = "Disconnect from a peer.",
    example = "ffx bluetooth peer disconnect <id|addr>"
)]
pub struct DisconnectCommand {
    #[argh(positional)]
    pub id_or_addr: ffx_bluetooth_common::PeerIdOrAddr,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(
    subcommand,
    name = "forget",
    description = "Delete and disconnect a peer.",
    example = "ffx bluetooth peer forget <id|addr>"
)]
pub struct ForgetCommand {
    #[argh(positional)]
    pub id_or_addr: ffx_bluetooth_common::PeerIdOrAddr,
}

/// ffx bluetooth peer pair
#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(
    subcommand,
    name = "pair",
    description = "Pair to a peer.",
    example = "Basic usage:

    $ ffx bluetooth peer pair <id|addr>

To set LE security level, specify `--le-security-level`. To disable bonding, use `--non-bondable`. \
To set the transport technology to use, specify `--transport` or `-t`.

    $ ffx bluetooth peer pair <id|addr> --le-security-level auth --non-bondable -t dm"
)]
pub struct PairCommand {
    /// specify peer by id or address
    #[argh(positional)]
    pub id_or_addr: ffx_bluetooth_common::PeerIdOrAddr,

    /// only relevant for LE. Setting this option when transport is "classic" will throw an error.
    /// Specify the Security Manager security level to pair with. Allowed values are
    /// "encrypted"/"enc" and "authenticated"/"auth". Default value is "authenticated"
    #[argh(option, long = "le-security-level")]
    pub le_security_level: Option<LeSecurityLevel>,

    /// prevent the device from forming a bond during pairing. Bonding is enabled by default. If
    /// transport is "classic", this option must be absent or an error will be thrown (non bondable
    /// mode is not currently supported for the "classic" transport)
    // TODO(https://fxbug.dev/42118593): Support NON_BONDABLE for the CLASSIC transport.
    #[argh(switch, long = "non-bondable")]
    pub non_bondable: bool,

    /// specify the technology type to pair over. Allowed values are "lowenergy"/"le",
    /// "classic"/"bredr"/"c", and "dualmode"/"both"/"dual"/"dm". Default value is "dualmode"
    #[argh(option, long = "transport", short = 't', default = "Transport::DualMode")]
    pub transport: Transport,
}

/// only relevant for LE. Determines the Security Manager security level to pair with
#[derive(Debug, PartialEq, Clone)]
pub enum LeSecurityLevel {
    Encrypted,
    Authenticated,
}

impl FromStr for LeSecurityLevel {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "enc" | "encrypted" => Ok(LeSecurityLevel::Encrypted),
            "auth" | "authenticated" => Ok(LeSecurityLevel::Authenticated),
            _ => Err("security level should be 'encrypted' or 'authenticated'"),
        }
    }
}

/// determines the technology type to pair over
#[derive(Debug, PartialEq, Clone)]
pub enum Transport {
    LowEnergy,
    Classic,
    DualMode,
}

impl FromStr for Transport {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().replace("_", "").replace("-", "").as_str() {
            "le" | "lowenergy" => Ok(Transport::LowEnergy),
            "c" | "classic" | "bredr" => Ok(Transport::Classic),
            "dm" | "dualmode" | "both" | "dual" => Ok(Transport::DualMode),
            _ => Err("transport should be 'lowenergy', 'classic', or 'dualmode'"),
        }
    }
}

impl From<Transport> for TechnologyType {
    fn from(transport: Transport) -> Self {
        match transport {
            Transport::LowEnergy => TechnologyType::LowEnergy,
            Transport::Classic => TechnologyType::Classic,
            Transport::DualMode => TechnologyType::DualMode,
        }
    }
}
