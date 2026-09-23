// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::num::{NonZeroU16, NonZeroU64};
use std::ops::RangeInclusive;
use std::str::FromStr;

use anyhow::{Context as _, anyhow};
use argh::{ArgsInfo, FromArgs};
use fidl_fuchsia_net_ext as fnet_ext;
#[cfg(not(feature = "fdomain"))]
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
#[cfg(feature = "fdomain")]
use fidl_fuchsia_net_interfaces_ext_fdomain as fnet_interfaces_ext;
#[cfg(not(feature = "fdomain"))]
use fidl_fuchsia_net_matchers_ext as fnet_matchers_ext;
#[cfg(feature = "fdomain")]
use fidl_fuchsia_net_matchers_ext_fdomain as fnet_matchers_ext;
use flex_fuchsia_net as fnet;
use flex_fuchsia_net_matchers as fnet_matchers;

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "filter")]
/// commands for configuring packet filtering
pub struct Filter {
    #[argh(subcommand)]
    pub filter_cmd: FilterEnum,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand)]
pub enum FilterEnum {
    List(List),
    Create(Create),
    Remove(Remove),
}

/// A command to list filtering configuration.
#[derive(Clone, Debug, ArgsInfo, FromArgs, PartialEq)]
#[argh(subcommand, name = "list")]
pub struct List {}

/// A command to create new filtering resources
#[derive(Clone, Debug, ArgsInfo, FromArgs, PartialEq)]
#[argh(subcommand, name = "create")]
pub struct Create {
    /// the name of the controller to create (or connect to, if existing)
    #[argh(option)]
    pub controller: String,
    /// whether the resource creation should be idempotent, i.e. succeed even
    /// if the resource already exists
    #[argh(switch)]
    pub idempotent: Option<bool>,
    /// the resource to create
    #[argh(subcommand)]
    pub resource: Resource,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand)]
pub enum Resource {
    Namespace(Namespace),
    Routine(Routine),
    Rule(Rule),
}

/// A command to specify a filtering namespace
#[derive(Clone, Debug, ArgsInfo, FromArgs, PartialEq)]
#[argh(subcommand, name = "namespace")]
pub struct Namespace {
    /// the name of the namespace
    #[argh(option)]
    pub name: String,
    /// the IP domain of the namespace
    #[argh(option, default = "Domain::All")]
    pub domain: Domain,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Domain {
    All,
    Ipv4,
    Ipv6,
}

impl std::str::FromStr for Domain {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "all" => Ok(Self::All),
            "ipv4" => Ok(Self::Ipv4),
            "ipv6" => Ok(Self::Ipv6),
            s => Err(anyhow!("unknown IP domain {s}")),
        }
    }
}

/// A command to specify a filtering routine
#[derive(Clone, Debug, ArgsInfo, FromArgs, PartialEq)]
#[argh(subcommand, name = "routine")]
pub struct Routine {
    /// the namespace that contains the routine
    #[argh(option)]
    pub namespace: String,
    /// the name of the routine
    #[argh(option)]
    pub name: String,
    /// the type of the routine (IP or NAT)
    #[argh(option, arg_name = "type")]
    pub type_: RoutineType,
    /// the hook on which the routine is installed (optional)
    #[argh(option)]
    pub hook: Option<Hook>,
    /// the priority of the routine on its hook (optional)
    #[argh(option)]
    pub priority: Option<i32>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RoutineType {
    Ip,
    Nat,
}

impl std::str::FromStr for RoutineType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "ip" => Ok(Self::Ip),
            "nat" => Ok(Self::Nat),
            s => Err(anyhow!("unknown routine type {s}")),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Hook {
    Ingress,
    LocalIngress,
    Forwarding,
    LocalEgress,
    Egress,
}

impl std::str::FromStr for Hook {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "ingress" => Ok(Self::Ingress),
            "local_ingress" | "local-ingress" => Ok(Self::LocalIngress),
            "forwarding" => Ok(Self::Forwarding),
            "local_egress" | "local-egress" => Ok(Self::LocalEgress),
            "egress" => Ok(Self::Egress),
            s => Err(anyhow!("unknown hook {s}")),
        }
    }
}

// TODO(https://fxbug.dev/364951693): eventually we should specify a grammar
// for fuchsia.net.filter rules, which we currently only have for
// fuchsia.net.filter.deprecated. Then `net filter create rule` can take a raw
// string that must be specified in this grammar.
//
/// A command to specify a filtering rule
#[derive(Clone, Debug, ArgsInfo, FromArgs, PartialEq)]
#[argh(subcommand, name = "rule")]
pub struct Rule {
    /// the namespace that contains the rule
    #[argh(option)]
    pub namespace: String,
    /// the routine that contains the rule
    #[argh(option)]
    pub routine: String,
    /// the index of the rule
    #[argh(option)]
    pub index: u32,
    /// a matcher for the ingress interface of the packet (optional)
    ///
    /// Accepted formats are any of "id:<numeric>" | "name:<string>" | "class:<string>".
    #[argh(option)]
    pub in_interface: Option<InterfaceMatcher>,
    /// a matcher for the egress interface of the packet (optional)
    ///
    /// Accepted formats are any of "id:<numeric>" | "name:<string>" | "class:<string>".
    #[argh(option)]
    pub out_interface: Option<InterfaceMatcher>,
    /// a matcher for the source address of the packet (optional)
    ///
    /// Accepted formats are any of "subnet:<subnet>" | "range:<min-addr>..=<max-addr>".
    /// Can be prefixed with a `!` for inverse match.
    #[argh(option)]
    pub src_addr: Option<AddressMatcher>,
    /// a matcher for the destination address of the packet (optional)
    ///
    /// Accepted formats are any of "subnet:<subnet>" | "range:<min-addr>..=<max-addr>".
    /// Can be prefixed with a `!` for inverse match.
    #[argh(option)]
    pub dst_addr: Option<AddressMatcher>,
    /// a matcher for the transport protocol of the packet (optional)
    ///
    /// Accepted protocols are "tcp" | "udp" | "icmp" | "icmpv6".
    #[argh(option)]
    pub transport_protocol: Option<TransportProtocolMatcher>,
    /// a matcher for the source port of the packet (optional)
    ///
    /// Must be accompanied by a transport protocol matcher for either TCP or UDP.
    ///
    /// Accepted format is "<min>..=<max>". Can be prefixed with a `!` for inverse match.
    #[argh(option)]
    pub src_port: Option<PortMatcher>,
    /// a matcher for the destination port of the packet (optional)
    ///
    /// Must be accompanied by a transport protocol matcher for either TCP or UDP.
    ///
    /// Accepted format is "<min>..=<max>". Can be prefixed with a `!` for inverse match.
    #[argh(option)]
    pub dst_port: Option<PortMatcher>,
    /// the action to take if the rule matches a given packet
    #[argh(subcommand)]
    pub action: Action,
}

/// An interface matcher
#[derive(Clone, Debug, PartialEq)]
pub enum InterfaceMatcher {
    Id(NonZeroU64),
    Name(String),
    PortClass(fnet_interfaces_ext::PortClass),
}

impl FromStr for InterfaceMatcher {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (property, value) = s.split_once(":").ok_or_else(|| {
            anyhow!("expected $property:$value where property is one of id, name, class")
        })?;
        match property {
            "id" => {
                let id = value.parse::<NonZeroU64>()?;
                Ok(Self::Id(id))
            }
            "name" => Ok(Self::Name(value.to_owned())),
            "class" => {
                let class = match value.to_lowercase().as_str() {
                    "loopback" => fnet_interfaces_ext::PortClass::Loopback,
                    "virtual" => fnet_interfaces_ext::PortClass::Virtual,
                    "ethernet" => fnet_interfaces_ext::PortClass::Ethernet,
                    "wlan_client" | "wlan-client" | "wlanclient" => {
                        fnet_interfaces_ext::PortClass::WlanClient
                    }
                    "wlan_ap" | "wlan-ap" | "wlanap" => fnet_interfaces_ext::PortClass::WlanAp,
                    "ppp" => fnet_interfaces_ext::PortClass::Ppp,
                    "bridge" => fnet_interfaces_ext::PortClass::Bridge,
                    "lowpan" => fnet_interfaces_ext::PortClass::Lowpan,
                    other => return Err(anyhow!("unrecognized port class {other}")),
                };
                Ok(Self::PortClass(class))
            }
            other => Err(anyhow!("unrecognized interface property {other}")),
        }
    }
}

impl From<InterfaceMatcher> for fnet_matchers_ext::Interface {
    fn from(matcher: InterfaceMatcher) -> Self {
        match matcher {
            InterfaceMatcher::Id(id) => Self::Id(id),
            InterfaceMatcher::Name(name) => Self::Name(name),
            InterfaceMatcher::PortClass(class) => Self::PortClass(class),
        }
    }
}

/// An invertible address matcher
#[derive(Clone, Debug, PartialEq)]
pub struct AddressMatcher {
    pub matcher: AddressMatcherType,
    pub invert: bool,
}

impl FromStr for AddressMatcher {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (invert, s) = s.strip_prefix("!").map(|s| (true, s)).unwrap_or((false, s));
        let matcher = s.parse::<AddressMatcherType>()?;
        Ok(Self { matcher, invert })
    }
}

impl From<AddressMatcher> for fnet_matchers_ext::Address {
    fn from(matcher: AddressMatcher) -> Self {
        let AddressMatcher { matcher, invert } = matcher;
        Self { matcher: matcher.into(), invert }
    }
}

/// An address matcher
#[derive(Clone, Debug, PartialEq)]
pub enum AddressMatcherType {
    Subnet(fnet_matchers_ext::Subnet),
    Range(fnet_matchers_ext::AddressRange),
}

impl FromStr for AddressMatcherType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (property, value) = s.split_once(":").ok_or_else(|| {
            anyhow!("expected $property:$value where property is one of subnet, range")
        })?;
        match property {
            "subnet" => {
                let subnet: fnet::Subnet = value.parse::<fnet_ext::Subnet>()?.into();
                let subnet = subnet.try_into()?;
                Ok(Self::Subnet(subnet))
            }
            "range" => {
                let (start, end) = value.split_once("..=").ok_or_else(|| {
                    anyhow!(
                        "expected inclusive address range to be specified in the form $start..=$end"
                    )
                })?;
                let start: fnet::IpAddress = start.parse::<fnet_ext::IpAddress>()?.into();
                let end: fnet::IpAddress = end.parse::<fnet_ext::IpAddress>()?.into();
                let range = fnet_matchers::AddressRange { start, end }.try_into()?;
                Ok(Self::Range(range))
            }
            other => Err(anyhow!("unrecognized address property {other}")),
        }
    }
}

impl From<AddressMatcherType> for fnet_matchers_ext::AddressMatcherType {
    fn from(matcher: AddressMatcherType) -> Self {
        match matcher {
            AddressMatcherType::Subnet(subnet) => Self::Subnet(subnet),
            AddressMatcherType::Range(range) => Self::Range(range),
        }
    }
}

/// A transport protocol matcher
#[derive(Clone, Debug, PartialEq)]
pub enum TransportProtocolMatcher {
    Tcp,
    Udp,
    Icmp,
    Icmpv6,
}

impl FromStr for TransportProtocolMatcher {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let matcher = match s.to_lowercase().as_str() {
            "tcp" => Self::Tcp,
            "udp" => Self::Udp,
            "icmp" | "icmpv4" => Self::Icmp,
            "icmpv6" => Self::Icmpv6,
            other => return Err(anyhow!("unrecognized transport protocol {other}")),
        };
        Ok(matcher)
    }
}

/// An invertible address matcher
#[derive(Clone, Debug, PartialEq)]
pub struct PortMatcher(fnet_matchers_ext::Port);

impl FromStr for PortMatcher {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (invert, s) = s.strip_prefix("!").map(|s| (true, s)).unwrap_or((false, s));
        let (start, end) = s.split_once("..=").ok_or_else(|| {
            anyhow!("expected inclusive port range to be specified in the form $start..=$end")
        })?;
        let start = start.parse::<u16>()?;
        let end = end.parse::<u16>()?;
        let matcher = fnet_matchers_ext::Port::new(start, end, invert)?;
        Ok(Self(matcher))
    }
}

impl From<PortMatcher> for fnet_matchers_ext::Port {
    fn from(matcher: PortMatcher) -> Self {
        matcher.0
    }
}

/// A filtering action
#[derive(Clone, Debug, ArgsInfo, FromArgs, PartialEq)]
#[argh(subcommand)]
pub enum Action {
    Accept(Accept),
    Drop(Drop),
    Jump(Jump),
    Return(Return),
    TransparentProxy(TransparentProxy),
    Redirect(Redirect),
    Masquerade(Masquerade),
}

/// The `fuchsia.net.filter/Action.Accept` action.
#[derive(Clone, Debug, ArgsInfo, FromArgs, PartialEq)]
#[argh(subcommand, name = "accept")]
pub struct Accept {}

/// The `fuchsia.net.filter/Action.Drop` action.
#[derive(Clone, Debug, ArgsInfo, FromArgs, PartialEq)]
#[argh(subcommand, name = "drop")]
pub struct Drop {}

/// The `fuchsia.net.filter/Action.Jump` action.
#[derive(Clone, Debug, ArgsInfo, FromArgs, PartialEq)]
#[argh(subcommand, name = "jump")]
pub struct Jump {
    /// the routine to jump to
    #[argh(positional)]
    pub target: String,
}

/// The `fuchsia.net.filter/Action.Return` action.
#[derive(Clone, Debug, ArgsInfo, FromArgs, PartialEq)]
#[argh(subcommand, name = "return")]
pub struct Return {}

/// The `fuchsia.net.filter/Action.TransparentProxy` action.
///
/// Both --addr and --port are optional, but at least one must be specified.
#[derive(Clone, Debug, ArgsInfo, FromArgs, PartialEq)]
#[argh(subcommand, name = "tproxy")]
pub struct TransparentProxy {
    /// the bound address of the local socket to redirect the packet to (optional)
    #[argh(option)]
    pub addr: Option<String>,
    /// the bound port of the local socket to redirect the packet to (optional, must be nonzero)
    #[argh(option)]
    pub port: Option<NonZeroU16>,
}

/// An inclusive range of nonzero ports
#[derive(Clone, Debug, PartialEq)]
pub struct PortRange(pub RangeInclusive<NonZeroU16>);

impl FromStr for PortRange {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (start, end) = s.split_once("..=").ok_or_else(|| {
            anyhow!("expected inclusive port range to be specified in the form $start..=$end")
        })?;
        let start = NonZeroU16::new(start.parse::<u16>()?).context("port must be nonzero")?;
        let end = NonZeroU16::new(end.parse::<u16>()?).context("port must be nonzero")?;
        Ok(Self(start..=end))
    }
}

/// The `fuchsia.net.filter/Action.Redirect` action.
///
/// The destination port range to which to redirect the packet is optional, but
/// --min-dst-port and --max-dst-port must either both be specified, or
/// neither.
#[derive(Clone, Debug, ArgsInfo, FromArgs, PartialEq)]
#[argh(subcommand, name = "redirect")]
pub struct Redirect {
    /// the destination port range used to rewrite the packet (optional)
    #[argh(option)]
    pub dst_port: Option<PortRange>,
}

/// The `fuchsia.net.filter/Action.Masquerade` action.
///
/// The source port range to use to rewrite the packet is optional, but
/// --min-src-port and --max-src-port must either both be specified, or
/// neither.
#[derive(Clone, Debug, ArgsInfo, FromArgs, PartialEq)]
#[argh(subcommand, name = "masquerade")]
pub struct Masquerade {
    /// the source port range used to rewrite the packet (optional)
    #[argh(option)]
    pub src_port: Option<PortRange>,
}

/// A command to remove existing filtering resources
#[derive(Clone, Debug, ArgsInfo, FromArgs, PartialEq)]
#[argh(subcommand, name = "remove")]
pub struct Remove {
    /// the name of the controller that owns the resource
    #[argh(option)]
    pub controller: String,
    /// the resource to be removed
    #[argh(subcommand)]
    pub resource: ResourceId,
    /// whether the resource removal should be idempotent, i.e. succeed even if
    /// the resource does not exist
    #[argh(switch)]
    pub idempotent: Option<bool>,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand)]
pub enum ResourceId {
    Namespace(NamespaceId),
    Routine(RoutineId),
    Rule(RuleId),
}

/// A command to identify a filtering namespace
#[derive(Clone, Debug, ArgsInfo, FromArgs, PartialEq)]
#[argh(subcommand, name = "namespace")]
pub struct NamespaceId {
    /// the name of the namespace
    #[argh(option)]
    pub name: String,
}

/// A command to identify a filtering routine
#[derive(Clone, Debug, ArgsInfo, FromArgs, PartialEq)]
#[argh(subcommand, name = "routine")]
pub struct RoutineId {
    /// the namespace that contains the routine
    #[argh(option)]
    pub namespace: String,
    /// the name of the routine
    #[argh(option)]
    pub name: String,
}

/// A command to identify a filtering rule
#[derive(Clone, Debug, ArgsInfo, FromArgs, PartialEq)]
#[argh(subcommand, name = "rule")]
pub struct RuleId {
    /// the namespace that contains the rule
    #[argh(option)]
    pub namespace: String,
    /// the routine that contains the rule
    #[argh(option)]
    pub routine: String,
    /// the index of the rule
    #[argh(option)]
    pub index: u32,
}

#[cfg(test)]
mod tests {
    use net_declare::{fidl_ip, fidl_subnet};
    use test_case::test_case;

    use super::*;

    const THREE: NonZeroU64 = NonZeroU64::new(3).unwrap();

    #[test_case("id:0" => Err(()); "id must be nonzero")]
    #[test_case("id:-1" => Err(()); "id must be nonnegative")]
    #[test_case("id:a" => Err(()); "id must be numeric")]
    #[test_case("wlan" => Err(()); "must have both property and value")]
    #[test_case("class:not-a-class" => Err(()); "class must be valid")]
    #[test_case("unknown-property:value" => Err(()); "property must be id, name, or class")]
    #[test_case("id:3" => Ok(InterfaceMatcher::Id(THREE)); "valid ID matcher")]
    #[test_case(
        "name:wlan" =>
        Ok(InterfaceMatcher::Name(String::from("wlan")));
        "valid name matcher"
    )]
    #[test_case(
        "class:ethernet" =>
        Ok(InterfaceMatcher::PortClass(fnet_interfaces_ext::PortClass::Ethernet));
        "valid ethernet matcher"
    )]
    #[test_case(
        "class:wlan-client" =>
        Ok(InterfaceMatcher::PortClass(fnet_interfaces_ext::PortClass::WlanClient));
        "valid wlan-client matcher"
    )]
    fn interface_matcher(s: &str) -> Result<InterfaceMatcher, ()> {
        s.parse::<InterfaceMatcher>().map_err(|_| ())
    }

    #[test_case("192.168.0.1" => Err(()); "must have both property and value")]
    #[test_case("unknown-property:value" => Err(()); "property must be range or subnet")]
    #[test_case("subnet:192.0.2.1/" => Err(()); "not a subnet")]
    #[test_case("subnet:192.0.2.1/24" => Err(()); "host bits are set")]
    #[test_case("range:192.0.2.1" => Err(()); "not a range")]
    #[test_case("range:192.0.2.1..192.0.2.255" => Err(()); "range must be specified with ..=")]
    #[test_case("range:192.0.2.255..=192.0.2.1" => Err(()); "invalid range")]
    #[test_case("range:A..=B" => Err(()); "not an IP address")]
    #[test_case("!!subnet:192.0.2.0/24" => Err(()); "double invert")]
    #[test_case(
        "subnet:192.0.2.0/24" =>
        Ok(AddressMatcher {
            matcher: AddressMatcherType::Subnet(fidl_subnet!("192.0.2.0/24").try_into().unwrap()),
            invert: false,
        });
        "valid IPv4 subnet matcher"
    )]
    #[test_case(
        "subnet:2001:db8::/32" =>
        Ok(AddressMatcher {
            matcher: AddressMatcherType::Subnet(fidl_subnet!("2001:db8::/32").try_into().unwrap()),
            invert: false,
        });
        "valid IPv6 subnet matcher"
    )]
    #[test_case(
        "!subnet:192.0.2.0/24" =>
        Ok(AddressMatcher {
            matcher: AddressMatcherType::Subnet(fidl_subnet!("192.0.2.0/24").try_into().unwrap()),
            invert: true,
        });
        "valid inverse IPv4 subnet matcher"
    )]
    #[test_case(
        "range:192.0.2.1..=192.0.2.255" =>
        Ok(AddressMatcher {
            matcher: AddressMatcherType::Range(
                fnet_matchers::AddressRange {
                    start: fidl_ip!("192.0.2.1"),
                    end: fidl_ip!("192.0.2.255"),
                }.try_into().unwrap()
            ),
            invert: false,
        });
        "valid IPv4 range matcher"
    )]
    #[test_case(
        "range:2001:db8::1..=2001:db8::2" =>
        Ok(AddressMatcher {
            matcher: AddressMatcherType::Range(
                fnet_matchers::AddressRange {
                    start: fidl_ip!("2001:db8::1"),
                    end: fidl_ip!("2001:db8::2"),
                }.try_into().unwrap()
            ),
            invert: false,
        });
        "valid IPv6 range matcher"
    )]
    #[test_case(
        "!range:192.0.2.1..=192.0.2.255" =>
        Ok(AddressMatcher {
            matcher: AddressMatcherType::Range(
                fnet_matchers::AddressRange {
                    start: fidl_ip!("192.0.2.1"),
                    end: fidl_ip!("192.0.2.255"),
                }.try_into().unwrap()
            ),
            invert: true,
        });
        "valid inverse IPv6 range matcher"
    )]
    fn address_matcher(s: &str) -> Result<AddressMatcher, ()> {
        s.parse::<AddressMatcher>().map_err(|_| ())
    }

    #[test_case("22" => Err(()); "must be specified as range")]
    #[test_case("22..22" => Err(()); "must be written as ..=")]
    #[test_case("65536..=65536" => Err(()); "invalid u16")]
    #[test_case("33333..=22222" => Err(()); "invalid range")]
    #[test_case("!!33333..=22222" => Err(()); "double invert")]
    #[test_case(
        "22..=22" =>
        Ok(PortMatcher(fnet_matchers_ext::Port::new(22, 22, /* invert */ false).unwrap()));
        "valid single port matcher"
    )]
    #[test_case(
        "0..=65535" =>
        Ok(PortMatcher(fnet_matchers_ext::Port::new(0, 65535, /* invert */ false).unwrap()));
        "valid port range matcher"
    )]
    #[test_case(
        "!443..=443" =>
        Ok(PortMatcher(fnet_matchers_ext::Port::new(443, 443, /* invert */ true).unwrap()));
        "valid inverse port matcher"
    )]
    fn port_matcher(s: &str) -> Result<PortMatcher, ()> {
        s.parse::<PortMatcher>().map_err(|_| ())
    }

    #[test_case("22" => Err(()); "must be specified as range")]
    #[test_case("22..22" => Err(()); "must be written as ..=")]
    #[test_case("0..=1" => Err(()); "must be nonzero")]
    #[test_case("65536..=65536" => Err(()); "invalid u16")]
    #[test_case(
        "22..=22" => Ok(PortRange(NonZeroU16::new(22).unwrap()..=NonZeroU16::new(22).unwrap()));
        "valid single port matcher"
    )]
    #[test_case(
        "1..=65535" => Ok(PortRange(NonZeroU16::new(1).unwrap()..=NonZeroU16::MAX));
        "valid port range matcher"
    )]
    fn port_range(s: &str) -> Result<PortRange, ()> {
        s.parse::<PortRange>().map_err(|_| ())
    }
}
