// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use clap::{Parser, ValueEnum};
use fidl_fuchsia_wlan_common as wlan_common;
use fidl_fuchsia_wlan_common::PowerSaveType;
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;

#[derive(ValueEnum, PartialEq, Copy, Clone, Debug)]
pub enum RoleArg {
    Client,
    Ap,
}

#[derive(ValueEnum, PartialEq, Copy, Clone, Debug)]
pub enum PhyArg {
    Erp,
    Ht,
    Vht,
}

#[derive(ValueEnum, PartialEq, Copy, Clone, Debug)]
pub enum CbwArg {
    Cbw20,
    Cbw40,
    Cbw80,
}

#[derive(ValueEnum, PartialEq, Copy, Clone, Debug)]
pub enum ScanTypeArg {
    Active,
    Passive,
}

#[derive(ValueEnum, PartialEq, Copy, Clone, Debug)]
pub enum PsModeArg {
    PsModeUltraLowPower,
    PsModeLowPower,
    PsModeBalanced,
    PsModePerformance,
}

#[derive(ValueEnum, PartialEq, Copy, Clone, Debug)]
pub enum OnOffArg {
    On,
    Off,
}

impl ::std::convert::From<RoleArg> for wlan_common::WlanMacRole {
    fn from(arg: RoleArg) -> Self {
        match arg {
            RoleArg::Client => wlan_common::WlanMacRole::Client,
            RoleArg::Ap => wlan_common::WlanMacRole::Ap,
        }
    }
}

impl ::std::convert::From<PhyArg> for fidl_ieee80211::WlanPhyType {
    fn from(arg: PhyArg) -> Self {
        match arg {
            PhyArg::Erp => fidl_ieee80211::WlanPhyType::Erp,
            PhyArg::Ht => fidl_ieee80211::WlanPhyType::Ht,
            PhyArg::Vht => fidl_ieee80211::WlanPhyType::Vht,
        }
    }
}

impl ::std::convert::From<CbwArg> for fidl_ieee80211::ChannelBandwidth {
    fn from(arg: CbwArg) -> Self {
        match arg {
            CbwArg::Cbw20 => fidl_ieee80211::ChannelBandwidth::Cbw20,
            CbwArg::Cbw40 => fidl_ieee80211::ChannelBandwidth::Cbw40,
            CbwArg::Cbw80 => fidl_ieee80211::ChannelBandwidth::Cbw80,
        }
    }
}

impl ::std::convert::From<ScanTypeArg> for wlan_common::ScanType {
    fn from(arg: ScanTypeArg) -> Self {
        match arg {
            ScanTypeArg::Active => wlan_common::ScanType::Active,
            ScanTypeArg::Passive => wlan_common::ScanType::Passive,
        }
    }
}

impl ::std::convert::From<PsModeArg> for PowerSaveType {
    fn from(arg: PsModeArg) -> Self {
        match arg {
            PsModeArg::PsModePerformance => PowerSaveType::PsModePerformance,
            PsModeArg::PsModeBalanced => PowerSaveType::PsModeBalanced,
            PsModeArg::PsModeLowPower => PowerSaveType::PsModeLowPower,
            PsModeArg::PsModeUltraLowPower => PowerSaveType::PsModeUltraLowPower,
        }
    }
}

impl ::std::convert::From<OnOffArg> for bool {
    fn from(arg: OnOffArg) -> Self {
        match arg {
            OnOffArg::On => true,
            OnOffArg::Off => false,
        }
    }
}

#[derive(ValueEnum, PartialEq, Copy, Clone, Debug)]
pub enum TxPowerScenarioArg {
    Default,
    VoiceCall,
    HeadCellOff,
    HeadCellOn,
    BodyCellOff,
    BodyCellOn,
    BodyBtActive,
}

impl ::std::convert::From<TxPowerScenarioArg> for fidl_fuchsia_wlan_internal::TxPowerScenario {
    fn from(arg: TxPowerScenarioArg) -> Self {
        match arg {
            TxPowerScenarioArg::Default => fidl_fuchsia_wlan_internal::TxPowerScenario::Default,
            TxPowerScenarioArg::VoiceCall => fidl_fuchsia_wlan_internal::TxPowerScenario::VoiceCall,
            TxPowerScenarioArg::HeadCellOff => {
                fidl_fuchsia_wlan_internal::TxPowerScenario::HeadCellOff
            }
            TxPowerScenarioArg::HeadCellOn => {
                fidl_fuchsia_wlan_internal::TxPowerScenario::HeadCellOn
            }
            TxPowerScenarioArg::BodyCellOff => {
                fidl_fuchsia_wlan_internal::TxPowerScenario::BodyCellOff
            }
            TxPowerScenarioArg::BodyCellOn => {
                fidl_fuchsia_wlan_internal::TxPowerScenario::BodyCellOn
            }
            TxPowerScenarioArg::BodyBtActive => {
                fidl_fuchsia_wlan_internal::TxPowerScenario::BodyBtActive
            }
        }
    }
}

#[derive(ValueEnum, PartialEq, Copy, Clone, Debug)]
pub enum BandArg {
    #[value(name = "2g")]
    TwoGhz,
    #[value(name = "5g")]
    FiveGhz,
}

impl ::std::convert::From<BandArg> for fidl_ieee80211::WlanBand {
    fn from(arg: BandArg) -> Self {
        match arg {
            BandArg::TwoGhz => fidl_ieee80211::WlanBand::TwoGhz,
            BandArg::FiveGhz => fidl_ieee80211::WlanBand::FiveGhz,
        }
    }
}

#[derive(Parser, Debug, PartialEq)]
pub enum Opt {
    #[command(subcommand, name = "phy")]
    /// commands for wlan phy devices
    Phy(PhyCmd),

    #[command(subcommand, name = "iface")]
    /// commands for wlan iface devices
    Iface(IfaceCmd),

    /// commands for client stations
    #[command(subcommand, name = "client")]
    Client(ClientCmd),
    #[command(name = "connect")]
    Connect(ClientConnectCmd),
    #[command(name = "disconnect")]
    Disconnect(ClientDisconnectCmd),
    #[command(name = "scan")]
    Scan(ClientScanCmd),
    #[command(name = "status")]
    Status(IfaceStatusCmd),
    #[command(name = "wmm_status")]
    WmmStatus(ClientWmmStatusCmd),

    #[command(subcommand, name = "ap")]
    /// commands for AP stations
    Ap(ApCmd),

    #[command(subcommand, name = "rsn")]
    #[cfg(target_os = "fuchsia")]
    /// commands for verifying RSN behavior
    Rsn(RsnCmd),
}

#[derive(Parser, Clone, Debug, PartialEq)]
pub enum PhyCmd {
    #[command(name = "list")]
    /// lists phy devices
    List,
    #[command(name = "query")]
    /// queries a phy device
    Query {
        /// id of the phy to query
        phy_id: u16,
    },
    #[command(name = "get-country")]
    /// gets the phy's country used for WLAN regulatory purposes
    GetCountry {
        /// id of the phy to query
        phy_id: u16,
    },
    #[command(name = "set-country")]
    /// sets the phy's country for WLAN regulatory purpose
    SetCountry {
        /// id of the phy to query
        phy_id: u16,
        country: String,
    },
    #[command(name = "clear-country")]
    /// sets the phy's country code to world-safe value
    ClearCountry {
        /// id of the phy to query
        phy_id: u16,
    },
    #[command(name = "reset")]
    Reset {
        /// id of the phy to reset
        phy_id: u16,
    },
    #[command(name = "get-power-state")]
    /// gets the on/off state of the phy
    GetPowerState {
        /// id of the phy to get its power state
        phy_id: u16,
    },
    #[command(name = "set-power-state")]
    /// sets the on/off state of the phy
    SetPowerState {
        /// id of the phy to get its power state
        phy_id: u16,
        /// desired state of the phy
        state: OnOffArg,
    },
    #[command(name = "get-powersave-mode")]
    /// gets the power save mode of the phy
    GetPowerSaveMode {
        /// id of the phy to get its power save mode
        phy_id: u16,
    },
    #[command(name = "set-powersave-mode")]
    /// sets the power save mode of the phy
    SetPowerSaveMode {
        /// id of the phy to set its power save mode
        phy_id: u16,
        #[arg(value_enum, ignore_case = true)]
        /// desired power save mode of the phy
        mode: PsModeArg,
    },
    #[command(name = "set-tx-power-scenario")]
    /// sets the tx power scenario of the phy
    SetTxPowerScenario {
        /// id of the phy
        phy_id: u16,
        #[arg(value_enum, ignore_case = true)]
        /// desired tx power scenario of the phy
        mode: TxPowerScenarioArg,
    },
    #[command(name = "get-tx-power-scenario")]
    /// gets the tx power scenario of the phy
    GetTxPowerScenario {
        /// id of the phy to query
        phy_id: u16,
    },

    #[command(name = "reset-tx-power-scenario")]
    ResetTxPowerScenario {
        // id of phy to reset tx power scenario
        phy_id: u16,
    },
}

#[derive(Parser, Clone, Debug, PartialEq)]
pub enum IfaceCmd {
    #[command(name = "new")]
    /// creates a new iface device
    New {
        #[arg(short = 'p', long = "phy")]
        /// id of the phy that will host the iface
        phy_id: u16,

        #[arg(
            short = 'r',
            long = "role",
            value_enum,
            default_value = "Client",
            ignore_case = true
        )]
        /// role of the new iface
        role: RoleArg,

        #[arg(short = 'm', long = "sta_addr", help = "Optional sta addr when we create an iface")]
        /// initial sta address for this iface
        sta_addr: Option<String>,
    },

    #[command(name = "del")]
    /// destroys an iface device
    Delete {
        /// iface id to destroy
        iface_id: u16,
    },

    #[command(name = "list")]
    List,
    #[command(name = "query")]
    Query { iface_id: u16 },
    #[command(subcommand, name = "minstrel")]
    Minstrel(MinstrelCmd),
    #[command(name = "status")]
    Status(IfaceStatusCmd),
}

#[derive(Parser, Clone, Debug, PartialEq)]
pub enum MinstrelCmd {
    #[command(name = "list")]
    List { iface_id: Option<u16> },
    #[command(name = "show")]
    Show { iface_id: Option<u16>, peer_addr: Option<String> },
}

#[derive(Parser, Clone, Debug, PartialEq)]
pub struct ClientConnectCmd {
    #[arg(short = 'i', long = "iface", default_value = "0")]
    pub iface_id: u16,
    #[arg(short = 'p', long = "password", help = "Password")]
    pub password: Option<String>,
    #[arg(long = "hash", help = "WPA2 PSK as hex string")]
    pub psk: Option<String>,
    #[arg(
        short = 's',
        long = "scan-type",
        default_value = "passive",
        value_enum,
        ignore_case = true,
        help = "Determines the type of scan performed on non-DFS channels when connecting."
    )]
    pub scan_type: ScanTypeArg,
    #[arg(short = 'b', long = "bssid", help = "Specific BSSID to connect to")]
    pub bssid: Option<String>,
    #[arg(
        help = "SSID of the target network. Connecting via only an SSID is deprecated and will be \
                removed; use the `donut` tool instead."
    )]
    pub ssid: String,
}

#[derive(Parser, Clone, Debug, PartialEq)]
pub struct ClientDisconnectCmd {
    #[arg(short = 'i', long = "iface", default_value = "0")]
    pub iface_id: u16,
}

#[derive(Parser, Clone, Debug, PartialEq)]
pub struct ClientScanCmd {
    #[arg(short = 'i', long = "iface", default_value = "0")]
    pub iface_id: u16,
    #[arg(
        short = 's',
        long = "scan-type",
        default_value = "passive",
        value_enum,
        ignore_case = true,
        help = "Experimental. Default scan type on each channel. \
                Behavior may differ on DFS channel"
    )]
    pub scan_type: ScanTypeArg,
}

#[derive(Parser, Clone, Debug, PartialEq)]
pub struct ClientWmmStatusCmd {
    #[arg(short = 'i', long = "iface", default_value = "0")]
    pub iface_id: u16,
}

#[derive(Parser, Clone, Debug, PartialEq)]
pub struct IfaceStatusCmd {
    #[arg(short = 'i', long = "iface")]
    pub iface_id: Option<u16>,
}

#[derive(Parser, Clone, Debug, PartialEq)]
pub enum ClientCmd {
    #[command(name = "scan")]
    Scan(ClientScanCmd),
    #[command(name = "connect")]
    Connect(ClientConnectCmd),
    #[command(name = "disconnect")]
    Disconnect(ClientDisconnectCmd),
    #[command(name = "wmm_status")]
    WmmStatus(ClientWmmStatusCmd),
}

#[derive(Parser, Clone, Debug, PartialEq)]
pub enum ApCmd {
    #[command(name = "start")]
    Start {
        #[arg(short = 'i', long = "iface", default_value = "0")]
        iface_id: u16,
        #[arg(short = 's', long = "ssid")]
        ssid: String,
        #[arg(short = 'p', long = "password")]
        password: Option<String>,
        #[arg(short = 'c', long = "channel")]
        // TODO(porce): Expand to support PHY and CBW
        channel: u8,
        #[arg(short = 'b', long = "band", value_enum, ignore_case = true)]
        band: BandArg,
    },
    #[command(name = "stop")]
    Stop {
        #[arg(short = 'i', long = "iface", default_value = "0")]
        iface_id: u16,
    },
}

#[derive(Parser, Clone, Debug, PartialEq)]
pub enum RsnCmd {
    #[command(name = "generate-psk")]
    GeneratePsk {
        #[arg(short = 'p', long = "passphrase")]
        passphrase: String,
        #[arg(short = 's', long = "ssid")]
        ssid: String,
    },
}
