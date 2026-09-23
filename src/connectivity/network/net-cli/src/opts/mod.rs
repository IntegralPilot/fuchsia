// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use fidl_fuchsia_net_ext as fnet_ext;
#[cfg(not(feature = "fdomain"))]
use fidl_fuchsia_net_interfaces_ext as finterfaces_ext;
#[cfg(feature = "fdomain")]
use fidl_fuchsia_net_interfaces_ext_fdomain as finterfaces_ext;
use flex_fuchsia_net as fnet;
use flex_fuchsia_net_interfaces as finterfaces;
use flex_fuchsia_net_interfaces_admin as finterfaces_admin;
use flex_fuchsia_net_stackmigrationdeprecated as fnet_migration;
use std::collections::HashMap;
use std::convert::TryInto as _;
use std::num::NonZeroU64;

pub(crate) mod capture;
pub(crate) mod dhcpd;
pub(crate) mod dns;
pub(crate) mod filter;

#[derive(thiserror::Error, Clone, Debug)]
#[error("{msg}")]
pub struct UserFacingError {
    pub msg: String,
}

pub fn user_facing_error(cause: impl Into<String>) -> anyhow::Error {
    UserFacingError { msg: cause.into() }.into()
}

pub fn underlying_user_facing_error(error: &anyhow::Error) -> Option<UserFacingError> {
    error.root_cause().downcast_ref::<UserFacingError>().cloned()
}

fn parse_ip_version_str(value: &str) -> Result<fnet::IpVersion, String> {
    match &value.to_lowercase()[..] {
        "ipv4" => Ok(fnet::IpVersion::V4),
        "ipv6" => Ok(fnet::IpVersion::V6),
        _ => Err("invalid IP version".to_string()),
    }
}

#[derive(ArgsInfo, FromArgs, Debug)]
/// commands for net-cli
pub struct Command {
    #[argh(subcommand)]
    pub cmd: CommandEnum,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand)]
pub enum CommandEnum {
    Capture(capture::Capture),
    FilterDeprecated(FilterDeprecated),
    Filter(filter::Filter),
    If(If),
    Log(Log),
    Neigh(Neigh),
    Route(Route),
    Rule(Rule),
    Dhcp(Dhcp),
    Dhcpd(dhcpd::Dhcpd),
    Dns(dns::Dns),
    NetstackMigration(NetstackMigration),
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "filter-deprecated")]
/// commands for configuring packet filtering with the deprecated API
pub struct FilterDeprecated {
    #[argh(subcommand)]
    pub filter_cmd: FilterDeprecatedEnum,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand)]
pub enum FilterDeprecatedEnum {
    GetNatRules(FilterGetNatRules),
    GetRdrRules(FilterGetRdrRules),
    GetRules(FilterGetRules),
    SetNatRules(FilterSetNatRules),
    SetRdrRules(FilterSetRdrRules),
    SetRules(FilterSetRules),
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "get-nat-rules")]
/// gets nat rules
pub struct FilterGetNatRules {}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "get-rdr-rules")]
/// gets rdr rules
pub struct FilterGetRdrRules {}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "get-rules")]
/// gets filter rules
pub struct FilterGetRules {}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "set-nat-rules")]
/// sets nat rules (see the netfilter::parser library for the NAT rules format)
pub struct FilterSetNatRules {
    #[argh(positional)]
    pub rules: String,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "set-rdr-rules")]
/// sets rdr rules (see the netfilter::parser library for the RDR rules format)
pub struct FilterSetRdrRules {
    #[argh(positional)]
    pub rules: String,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "set-rules")]
/// sets filter rules (see the netfilter::parser library for the rules format)
pub struct FilterSetRules {
    #[argh(positional)]
    pub rules: String,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "if")]
/// commands for network interfaces
pub struct If {
    #[argh(subcommand)]
    pub if_cmd: IfEnum,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand)]
pub enum IfEnum {
    Addr(IfAddr),
    Bridge(IfBridge),
    Config(IfConfig),
    Disable(IfDisable),
    Enable(IfEnable),
    Get(IfGet),
    Igmp(IfIgmp),
    IpForward(IfIpForward),
    List(IfList),
    Mld(IfMld),
    Add(IfAdd),
    Remove(IfRemove),
}

#[derive(Clone, Debug, PartialEq)]
pub enum InterfaceIdentifier {
    Id(NonZeroU64),
    Name(String),
}

impl InterfaceIdentifier {
    pub async fn find_nicid<C>(&self, connector: &C) -> Result<NonZeroU64, anyhow::Error>
    where
        C: crate::ServiceConnector<finterfaces::StateMarker>,
    {
        match self {
            Self::Id(id) => Ok(*id),
            Self::Name(name) => {
                let interfaces_state = crate::connect_with_context(connector).await?;
                let stream = finterfaces_ext::event_stream_from_state::<
                    finterfaces_ext::AllInterest,
                >(&interfaces_state, Default::default())?;
                let response = finterfaces_ext::existing(
                    stream,
                    HashMap::<NonZeroU64, finterfaces_ext::PropertiesAndState<(), _>>::new(),
                )
                .await?;
                response
                    .values()
                    .find_map(|properties_and_state| {
                        (&properties_and_state.properties.name == name)
                            .then_some(properties_and_state.properties.id)
                    })
                    .ok_or_else(|| user_facing_error(format!("No interface with name {}", name)))
            }
        }
    }
}

impl core::str::FromStr for InterfaceIdentifier {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let nicid_parse_result = s.parse::<NonZeroU64>();
        nicid_parse_result.map_or_else(
            |nicid_parse_error| {
                if !s.starts_with("name:") {
                    Err(user_facing_error(format!(
                        "Failed to parse as NICID (error: {}) or as interface name \
                        (error: interface names must be specified as `name:ifname`, where \
                        ifname is the actual interface name in this example)",
                        nicid_parse_error
                    )))
                } else {
                    Ok(Self::Name(s["name:".len()..].to_string()))
                }
            },
            |nicid| Ok(Self::Id(nicid)),
        )
    }
}

impl From<NonZeroU64> for InterfaceIdentifier {
    fn from(nicid: NonZeroU64) -> InterfaceIdentifier {
        InterfaceIdentifier::Id(nicid)
    }
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "addr")]
/// commands for updating network interface addresses
pub struct IfAddr {
    #[argh(subcommand)]
    pub addr_cmd: IfAddrEnum,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand)]
pub enum IfAddrEnum {
    Add(IfAddrAdd),
    Del(IfAddrDel),
    Wait(IfAddrWait),
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "add")]
/// adds an address to the network interface
pub struct IfAddrAdd {
    #[argh(positional, arg_name = "nicid or name:ifname")]
    pub interface: InterfaceIdentifier,
    #[argh(positional)]
    pub addr: String,
    #[argh(positional, from_str_fn(parse_netmask_or_prefix_length))]
    pub prefix: u8,
    #[argh(switch)]
    /// skip adding a local subnet route for this interface and address
    pub no_subnet_route: bool,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "del")]
/// deletes an address from the network interface
pub struct IfAddrDel {
    #[argh(positional, arg_name = "nicid or name:ifname")]
    pub interface: InterfaceIdentifier,
    #[argh(positional)]
    pub addr: String,
    #[argh(positional, from_str_fn(parse_netmask_or_prefix_length))]
    pub prefix: Option<u8>,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "wait")]
/// waits for an address to be assigned on the network interface.
///
/// by default waits for any address; if --ipv6 is specified, waits for an IPv6
/// address.
pub struct IfAddrWait {
    #[argh(positional, arg_name = "nicid or name:ifname")]
    pub interface: InterfaceIdentifier,
    /// wait for an IPv6 address
    #[argh(switch)]
    pub ipv6: bool,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "bridge")]
/// creates a bridge between network interfaces
pub struct IfBridge {
    #[argh(positional, arg_name = "nicid or name:ifname")]
    pub interfaces: Vec<InterfaceIdentifier>,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "disable")]
/// disables a network interface
pub struct IfDisable {
    #[argh(positional, arg_name = "nicid or name:ifname")]
    pub interface: InterfaceIdentifier,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "enable")]
/// enables a network interface
pub struct IfEnable {
    #[argh(positional, arg_name = "nicid or name:ifname")]
    pub interface: InterfaceIdentifier,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "get")]
/// queries a network interface
pub struct IfGet {
    #[argh(positional, arg_name = "nicid or name:ifname")]
    pub interface: InterfaceIdentifier,
}

// TODO(https://fxbug.dev/371584272): Replace this with if config.
#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "igmp")]
/// get or set IGMP configuration
pub struct IfIgmp {
    #[argh(subcommand)]
    pub cmd: IfIgmpEnum,
}

// TODO(https://fxbug.dev/371584272): Replace this with if config.
#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand)]
pub enum IfIgmpEnum {
    Get(IfIgmpGet),
    Set(IfIgmpSet),
}

// TODO(https://fxbug.dev/371584272): Replace this with if config get.
#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "get")]
/// get IGMP configuration for an interface
pub struct IfIgmpGet {
    #[argh(positional, arg_name = "nicid or name:ifname")]
    pub interface: InterfaceIdentifier,
}

// TODO(https://fxbug.dev/371584272): Replace this with if config set.
#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "set")]
/// set IGMP configuration for an interface
pub struct IfIgmpSet {
    #[argh(positional, arg_name = "nicid or name:ifname")]
    pub interface: InterfaceIdentifier,

    /// the version of IGMP to perform.
    #[argh(option, from_str_fn(parse_igmp_version))]
    pub version: Option<finterfaces_admin::IgmpVersion>,
}

fn parse_igmp_version(s: &str) -> Result<finterfaces_admin::IgmpVersion, String> {
    match s.parse::<u8>() {
        Err(err) => Err(format!("Failed to parse IGMP version (error: {})", err)),
        Ok(v) => match v {
            1 => Ok(finterfaces_admin::IgmpVersion::V1),
            2 => Ok(finterfaces_admin::IgmpVersion::V2),
            3 => Ok(finterfaces_admin::IgmpVersion::V3),
            v => Err(format!("Invalid IGMP version ({}). Valid values: 1, 2, 3", v)),
        },
    }
}

// TODO(https://fxbug.dev/371584272): Replace this with if config.
#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "ip-forward")]
/// get or set IP forwarding for an interface
pub struct IfIpForward {
    #[argh(subcommand)]
    pub cmd: IfIpForwardEnum,
}

// TODO(https://fxbug.dev/371584272): Replace this with if config.
#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand)]
pub enum IfIpForwardEnum {
    Get(IfIpForwardGet),
    Set(IfIpForwardSet),
}

// TODO(https://fxbug.dev/371584272): Replace this with if config get.
#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "get")]
/// get IP forwarding for an interface
pub struct IfIpForwardGet {
    #[argh(positional, arg_name = "nicid or name:ifname")]
    pub interface: InterfaceIdentifier,

    #[argh(positional, from_str_fn(parse_ip_version_str))]
    pub ip_version: fnet::IpVersion,
}

// TODO(https://fxbug.dev/371584272): Replace this with if config set.
#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "set")]
/// set IP forwarding for an interface
pub struct IfIpForwardSet {
    #[argh(positional, arg_name = "nicid or name:ifname")]
    pub interface: InterfaceIdentifier,

    #[argh(positional, from_str_fn(parse_ip_version_str))]
    pub ip_version: fnet::IpVersion,

    #[argh(positional)]
    pub enable: bool,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "list")]
/// lists network interfaces (supports ffx machine output)
pub struct IfList {
    #[argh(positional)]
    pub name_pattern: Option<String>,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "add")]
/// add interfaces
pub struct IfAdd {
    #[argh(subcommand)]
    pub cmd: IfAddEnum,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand)]
pub enum IfAddEnum {
    Blackhole(IfBlackholeAdd),
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "blackhole")]
/// add a blackhole interface
pub struct IfBlackholeAdd {
    #[argh(positional)]
    pub name: String,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "remove")]
/// remove interfaces
pub struct IfRemove {
    #[argh(positional, arg_name = "nicid or name:ifname")]
    pub interface: InterfaceIdentifier,
}

// TODO(https://fxbug.dev/371584272): Replace this with if config.
#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "mld")]
/// get or set MLD configuration
pub struct IfMld {
    #[argh(subcommand)]
    pub cmd: IfMldEnum,
}

// TODO(https://fxbug.dev/371584272): Replace this with if config.
#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand)]
pub enum IfMldEnum {
    Get(IfMldGet),
    Set(IfMldSet),
}

// TODO(https://fxbug.dev/371584272): Replace this with if config get.
#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "get")]
/// get MLD configuration for an interface
pub struct IfMldGet {
    #[argh(positional, arg_name = "nicid or name:ifname")]
    pub interface: InterfaceIdentifier,
}

// TODO(https://fxbug.dev/371584272): Replace this with if config set.
#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "set")]
/// set MLD configuration for an interface
pub struct IfMldSet {
    #[argh(positional, arg_name = "nicid or name:ifname")]
    pub interface: InterfaceIdentifier,

    /// the version of MLD to perform.
    #[argh(option, from_str_fn(parse_mld_version))]
    pub version: Option<finterfaces_admin::MldVersion>,
}

fn parse_mld_version(s: &str) -> Result<finterfaces_admin::MldVersion, String> {
    match s.parse::<u8>() {
        Err(err) => Err(format!("Failed to parse MLD version (error: {})", err)),
        Ok(v) => match v {
            1 => Ok(finterfaces_admin::MldVersion::V1),
            2 => Ok(finterfaces_admin::MldVersion::V2),
            v => Err(format!("Invalid MLD version ({}). Valid values: 1, 2", v)),
        },
    }
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "config")]
/// get or set interface configuration
pub struct IfConfig {
    #[argh(positional, arg_name = "nicid or name:ifname")]
    pub interface: InterfaceIdentifier,

    #[argh(subcommand)]
    pub cmd: IfConfigEnum,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand)]
/// get configuration for an interface
pub enum IfConfigEnum {
    Set(IfConfigSet),
    Get(IfConfigGet),
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "set")]
// TODO(https://fxbug.dev/370850762): Generate the help string programmatically
// so that it can live next to the parsing logic.
/** set interface configuration

Configuration parameters and the values to be set to should be passed
in pairs. The names of the configuration parameters are taken from
the structure of fuchsia.net.interfaces.admin/Configuration.

The list of supported parameters are:
  ipv6.ndp.slaac.temporary_address_enabled
    bool
    Whether temporary addresses should be generated.
  ipv6.ndp.dad.transmits
    u16
    Number of DAD transmissions before an address is considered available for use.
  ipv6.ndp.router_solicitations.max
    u8
    Maximum number of Router Solicitations to send.
*/
pub struct IfConfigSet {
    /// the config parameter names and the
    #[argh(positional)]
    pub options: Vec<String>,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "get")]
/// get interface configuration
pub struct IfConfigGet {}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "log")]
/// commands for logging
pub struct Log {
    #[argh(subcommand)]
    pub log_cmd: LogEnum,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand)]
pub enum LogEnum {
    SetPackets(LogSetPackets),
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "set-packets")]
/// log packets to stdout
pub struct LogSetPackets {
    #[argh(positional)]
    pub enabled: bool,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "neigh")]
/// commands for neighbor tables
pub struct Neigh {
    #[argh(subcommand)]
    pub neigh_cmd: NeighEnum,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand)]
pub enum NeighEnum {
    Add(NeighAdd),
    Clear(NeighClear),
    Del(NeighDel),
    List(NeighList),
    Watch(NeighWatch),
    Config(NeighConfig),
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "add")]
/// adds an entry to the neighbor table
pub struct NeighAdd {
    #[argh(positional, arg_name = "nicid or name:ifname")]
    pub interface: InterfaceIdentifier,
    #[argh(positional)]
    pub ip: fnet_ext::IpAddress,
    #[argh(positional)]
    pub mac: fnet_ext::MacAddress,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "clear")]
/// removes all entries associated with a network interface from the neighbor table
pub struct NeighClear {
    #[argh(positional, arg_name = "nicid or name:ifname")]
    pub interface: InterfaceIdentifier,

    #[argh(positional, from_str_fn(parse_ip_version_str))]
    pub ip_version: fnet::IpVersion,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "list")]
/// lists neighbor table entries (supports ffx machine output)
pub struct NeighList {}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "del")]
/// removes an entry from the neighbor table
pub struct NeighDel {
    #[argh(positional, arg_name = "nicid or name:ifname")]
    pub interface: InterfaceIdentifier,
    #[argh(positional)]
    pub ip: fnet_ext::IpAddress,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "watch")]
/// watches neighbor table entries for state changes (supports ffx machine output)
pub struct NeighWatch {}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "config")]
/// commands for the Neighbor Unreachability Detection configuration
pub struct NeighConfig {
    #[argh(subcommand)]
    pub neigh_config_cmd: NeighConfigEnum,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand)]
pub enum NeighConfigEnum {
    Get(NeighGetConfig),
    Update(NeighUpdateConfig),
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "get")]
/// returns the current NUD configuration options for the provided interface
pub struct NeighGetConfig {
    #[argh(positional, arg_name = "nicid or name:ifname")]
    pub interface: InterfaceIdentifier,

    #[argh(positional, from_str_fn(parse_ip_version_str))]
    pub ip_version: fnet::IpVersion,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "update")]
/// updates the current NUD configuration options for the provided interface
pub struct NeighUpdateConfig {
    #[argh(positional, arg_name = "nicid or name:ifname")]
    pub interface: InterfaceIdentifier,

    #[argh(positional, from_str_fn(parse_ip_version_str))]
    pub ip_version: fnet::IpVersion,

    /// a base duration, in nanoseconds, for computing the random reachable
    /// time
    #[argh(option)]
    pub base_reachable_time: Option<i64>,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "route")]
/// commands for routing tables
pub struct Route {
    #[argh(subcommand)]
    pub route_cmd: RouteEnum,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand)]
pub enum RouteEnum {
    List(RouteList),
    Add(RouteAdd),
    Del(RouteDel),
}

fn parse_netmask_or_prefix_length(s: &str) -> Result<u8, String> {
    let netmask_parse_result = s.parse::<std::net::IpAddr>();
    let prefix_len_parse_result = s.parse::<u8>();
    match (netmask_parse_result, prefix_len_parse_result) {
        (Err(netmask_parse_error), Err(prefix_len_parse_error)) => Err(format!(
            "Failed to parse as netmask (error: {}) or prefix length (error: {})",
            netmask_parse_error, prefix_len_parse_error,
        )),
        (Ok(_), Ok(_)) => Err(format!(
            "Input parses both as netmask and as prefix length. This should never happen."
        )),
        (Ok(netmask), Err(_)) => Ok(subnet_mask_to_prefix_length(netmask)),
        (Err(_), Ok(prefix_len)) => Ok(prefix_len),
    }
}

fn subnet_mask_to_prefix_length(addr: std::net::IpAddr) -> u8 {
    match addr {
        std::net::IpAddr::V4(addr) => (!u32::from_be_bytes(addr.octets())).leading_zeros(),
        std::net::IpAddr::V6(addr) => (!u128::from_be_bytes(addr.octets())).leading_zeros(),
    }
    .try_into()
    .unwrap()
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "list")]
/// lists routes (supports ffx machine output)
pub struct RouteList {}

macro_rules! route_struct {
    ($ty_name:ident, $name:literal, $comment:expr) => {
        #[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
        #[argh(subcommand, name = $name)]
        #[doc = $comment]
        pub struct $ty_name {
            #[argh(option)]
            /// the network id of the destination network
            pub destination: std::net::IpAddr,
            #[argh(
                option,
                arg_name = "netmask or prefix length",
                from_str_fn(parse_netmask_or_prefix_length),
                long = "netmask"
            )]
            /// the netmask or prefix length corresponding to destination
            pub prefix_len: u8,
            #[argh(option)]
            /// the ip address of the first hop router
            pub gateway: Option<std::net::IpAddr>,
            #[argh(option, arg_name = "nicid or name:ifname", long = "nicid")]
            /// the outgoing network interface of the route
            pub interface: InterfaceIdentifier,
            #[argh(option)]
            /// the metric for the route
            pub metric: Option<u32>,
        }
    };
}

// TODO(https://github.com/google/argh/issues/48): do this more sanely.
route_struct!(RouteAdd, "add", "adds a route to the route table");
route_struct!(RouteDel, "del", "deletes a route from the route table");

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "rule")]
/// commands for policy-based-routing rules
pub struct Rule {
    #[argh(subcommand)]
    pub rule_cmd: RuleEnum,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand)]
pub enum RuleEnum {
    List(RuleList),
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "list")]
/// lists rules (supports ffx machine output)
pub struct RuleList {}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "dhcp")]
/// commands for an interfaces dhcp client
pub struct Dhcp {
    #[argh(subcommand)]
    pub dhcp_cmd: DhcpEnum,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand)]
pub enum DhcpEnum {
    Start(DhcpStart),
    Stop(DhcpStop),
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "start")]
/// starts a dhcp client on the interface
pub struct DhcpStart {
    #[argh(positional, arg_name = "nicid or name:ifname")]
    pub interface: InterfaceIdentifier,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "stop")]
/// stops the dhcp client on the interface
pub struct DhcpStop {
    #[argh(positional, arg_name = "nicid or name:ifname")]
    pub interface: InterfaceIdentifier,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "migration")]
/// controls netstack selection for migration from netstack2 to netstack3
pub struct NetstackMigration {
    #[argh(subcommand)]
    pub cmd: NetstackMigrationEnum,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand)]
pub enum NetstackMigrationEnum {
    Set(NetstackMigrationSet),
    Get(NetstackMigrationGet),
    Clear(NetstackMigrationClear),
}

fn parse_netstack_version(s: &str) -> Result<fnet_migration::NetstackVersion, String> {
    match s {
        "ns2" => Ok(fnet_migration::NetstackVersion::Netstack2),
        "ns3" => Ok(fnet_migration::NetstackVersion::Netstack3),
        // NB: argh already prints the bad value to the user, no need to repeat
        // it.
        _ => Err(format!("valid values are ns2 or ns3")),
    }
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "set")]
/// sets the netstack version at next boot to |ns2| or |ns3|.
pub struct NetstackMigrationSet {
    #[argh(positional, from_str_fn(parse_netstack_version))]
    /// ns2 or ns3
    pub version: fnet_migration::NetstackVersion,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "get")]
/// prints the currently configured netstack version for migration.
pub struct NetstackMigrationGet {}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "clear")]
/// clears netstack version for migration configuration.
pub struct NetstackMigrationClear {}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use test_case::test_case;

    #[test_case("24", 24 ; "from prefix length")]
    #[test_case("255.255.254.0", 23 ; "from ipv4 netmask")]
    #[test_case("ffff:fff0::", 28 ; "from ipv6 netmask")]
    fn parse_prefix_len(to_parse: &str, want: u8) {
        let got = parse_netmask_or_prefix_length(to_parse).unwrap();
        assert_eq!(got, want)
    }

    proptest::proptest! {
        #[test]
        fn cant_parse_as_both_netmask_and_prefix_len(s: String) {
            let netmask_parse_result = s.parse::<std::net::IpAddr>();
            let prefix_len_parse_result = s.parse::<u8>();
            assert_matches!((netmask_parse_result, prefix_len_parse_result), (Ok(_), Err(_)) | (Err(_), Ok(_)) | (Err(_), Err(_)));
        }
    }

    #[test_case("1", InterfaceIdentifier::Id(NonZeroU64::new(1).unwrap()) ; "as nicid")]
    #[test_case("name:lo", InterfaceIdentifier::Name("lo".to_string()) ; "as ifname")]
    #[test_case("name:name:lo", InterfaceIdentifier::Name("name:lo".to_string()) ; "as ifname with 'name:' as part of name")]
    #[test_case("name:1", InterfaceIdentifier::Name("1".to_string()) ; "as numerical ifname")]
    fn parse_interface(to_parse: &str, want: InterfaceIdentifier) {
        let got = to_parse.parse::<InterfaceIdentifier>().unwrap();
        assert_eq!(got, want)
    }

    #[test]
    fn parse_interface_without_prefix_fails() {
        assert_matches!("lo".parse::<InterfaceIdentifier>(), Err(_))
    }

    #[test]
    fn parse_interface_zero_fails() {
        assert_matches!("0".parse::<InterfaceIdentifier>(), Err(_))
    }

    #[test_case("0", Err("Invalid IGMP version (0). Valid values: 1, 2, 3".to_string()); "input_0")]
    #[test_case("1", Ok(finterfaces_admin::IgmpVersion::V1); "input_1")]
    #[test_case("2", Ok(finterfaces_admin::IgmpVersion::V2); "input_2")]
    #[test_case("3", Ok(finterfaces_admin::IgmpVersion::V3); "input_3")]
    #[test_case("4", Err("Invalid IGMP version (4). Valid values: 1, 2, 3".to_string()); "input_4")]
    #[test_case("a", Err("Failed to parse IGMP version (error: invalid digit found in string)".to_string()); "input_a")]
    fn igmp_version(s: &str, expected: Result<finterfaces_admin::IgmpVersion, String>) {
        assert_eq!(parse_igmp_version(s), expected)
    }

    #[test_case("0", Err("Invalid MLD version (0). Valid values: 1, 2".to_string()); "input_0")]
    #[test_case("1", Ok(finterfaces_admin::MldVersion::V1); "input_1")]
    #[test_case("2", Ok(finterfaces_admin::MldVersion::V2); "input_2")]
    #[test_case("3", Err("Invalid MLD version (3). Valid values: 1, 2".to_string()); "input_3")]
    #[test_case("a", Err("Failed to parse MLD version (error: invalid digit found in string)".to_string()); "input_a")]
    fn mld_version(s: &str, expected: Result<finterfaces_admin::MldVersion, String>) {
        assert_eq!(parse_mld_version(s), expected)
    }
}
