// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Rather than reuse existing _ext types, we define intermediary types for
//! JSON serialization to avoid coupling too closely to particular FIDL
//! protocols.

#[cfg(feature = "fdomain")]
use fidl_fuchsia_net_interfaces_ext_fdomain as fidl_fuchsia_net_interfaces_ext;
#[cfg(feature = "fdomain")]
use fidl_fuchsia_net_neighbor_ext_fdomain as fidl_fuchsia_net_neighbor_ext;
use fidl_fuchsia_net_routes_ext as froutes_ext;
#[cfg(feature = "fdomain")]
use fidl_fuchsia_net_routes_ext_fdomain as fidl_fuchsia_net_routes_ext;
use net_types::Witness as _;
use net_types::ip::IpAddress as _;
use thiserror::Error;

#[derive(serde::Serialize, Ord, PartialOrd, Eq, PartialEq)]
pub(crate) struct Subnet<T> {
    pub(crate) addr: T,
    pub(crate) prefix_len: u8,
}

impl From<fidl_fuchsia_net_ext::Subnet> for Subnet<std::net::IpAddr> {
    fn from(ext: fidl_fuchsia_net_ext::Subnet) -> Subnet<std::net::IpAddr> {
        let fidl_fuchsia_net_ext::Subnet {
            addr: fidl_fuchsia_net_ext::IpAddress(addr),
            prefix_len,
        } = ext;
        Subnet { addr, prefix_len }
    }
}

impl<A: net_types::ip::IpAddress> From<net_types::ip::Subnet<A>> for Subnet<std::net::IpAddr> {
    fn from(sub: net_types::ip::Subnet<A>) -> Subnet<std::net::IpAddr> {
        let addr = sub.network().to_ip_addr().into();
        let prefix_len = sub.prefix();
        Subnet { addr, prefix_len }
    }
}

#[derive(serde::Serialize, Ord, PartialOrd, Eq, PartialEq)]
pub(crate) enum AddressAssignmentState {
    Tentative,
    Assigned,
    Unavailable,
}

impl From<flex_fuchsia_net_interfaces::AddressAssignmentState> for AddressAssignmentState {
    fn from(value: flex_fuchsia_net_interfaces::AddressAssignmentState) -> Self {
        match value {
            flex_fuchsia_net_interfaces::AddressAssignmentState::Tentative => Self::Tentative,
            flex_fuchsia_net_interfaces::AddressAssignmentState::Assigned => Self::Assigned,
            flex_fuchsia_net_interfaces::AddressAssignmentState::Unavailable => Self::Unavailable,
        }
    }
}

#[derive(serde::Serialize, Ord, PartialOrd, Eq, PartialEq)]
pub(crate) struct Address<I> {
    #[serde(flatten)]
    pub(crate) subnet: Subnet<I>,
    pub(crate) valid_until: Option<i64>,
    pub(crate) assignment_state: AddressAssignmentState,
}

impl<I> Address<I> {
    fn map<I2, F: Fn(I) -> I2>(self, f: F) -> Address<I2> {
        let Self { subnet: Subnet { addr, prefix_len }, valid_until, assignment_state } = self;
        Address { subnet: Subnet { addr: f(addr), prefix_len }, valid_until, assignment_state }
    }
}

#[derive(serde::Serialize)]
pub(crate) struct Addresses {
    pub(crate) ipv4: Vec<Address<std::net::Ipv4Addr>>,
    pub(crate) ipv6: Vec<Address<std::net::Ipv6Addr>>,
}

impl Addresses {
    pub(crate) fn all_addresses(self) -> impl Iterator<Item = Address<std::net::IpAddr>> {
        let Self { ipv4, ipv6 } = self;
        ipv4.into_iter()
            .map(|a| a.map(Into::into))
            .chain(ipv6.into_iter().map(|a| a.map(Into::into)))
    }
}

impl<
    I: Iterator<
        Item = fidl_fuchsia_net_interfaces_ext::Address<
            fidl_fuchsia_net_interfaces_ext::AllInterest,
        >,
    >,
> From<I> for Addresses
{
    fn from(addresses: I) -> Addresses {
        use itertools::Itertools as _;

        let (mut ipv4, mut ipv6): (Vec<_>, Vec<_>) = addresses.into_iter().partition_map(
            |fidl_fuchsia_net_interfaces_ext::Address {
                 addr,
                 valid_until,
                 assignment_state,
                 // TODO(https://fxbug.dev/42051655): Expose address lifetimes.
                 preferred_lifetime_info: _,
             }| {
                let fidl_fuchsia_net_ext::Subnet {
                    addr: fidl_fuchsia_net_ext::IpAddress(addr),
                    prefix_len,
                } = addr.into();
                let assignment_state = assignment_state.into();

                fn new_address<I>(
                    addr: I,
                    prefix_len: u8,
                    valid_until: fidl_fuchsia_net_interfaces_ext::PositiveMonotonicInstant,
                    assignment_state: AddressAssignmentState,
                ) -> Address<I> {
                    let valid_until =
                        (!valid_until.is_infinite()).then_some(valid_until.into_nanos());
                    Address { subnet: Subnet { addr, prefix_len }, valid_until, assignment_state }
                }
                match addr {
                    std::net::IpAddr::V4(addr) => itertools::Either::Left(new_address(
                        addr,
                        prefix_len,
                        valid_until,
                        assignment_state,
                    )),
                    std::net::IpAddr::V6(addr) => itertools::Either::Right(new_address(
                        addr,
                        prefix_len,
                        valid_until,
                        assignment_state,
                    )),
                }
            },
        );
        ipv4.sort();
        ipv6.sort();
        Addresses { ipv4, ipv6 }
    }
}

#[derive(serde::Serialize)]
pub(crate) enum DeviceClass {
    Loopback,
    Blackhole,
    Virtual,
    Ethernet,
    WlanClient,
    Ppp,
    Bridge,
    WlanAp,
    Lowpan,
}

impl From<fidl_fuchsia_net_interfaces_ext::PortClass> for DeviceClass {
    fn from(port_class: fidl_fuchsia_net_interfaces_ext::PortClass) -> Self {
        match port_class {
            fidl_fuchsia_net_interfaces_ext::PortClass::Loopback => Self::Loopback,
            fidl_fuchsia_net_interfaces_ext::PortClass::Blackhole => Self::Blackhole,
            fidl_fuchsia_net_interfaces_ext::PortClass::Virtual => Self::Virtual,
            fidl_fuchsia_net_interfaces_ext::PortClass::Ethernet => Self::Ethernet,
            fidl_fuchsia_net_interfaces_ext::PortClass::WlanClient => Self::WlanClient,
            fidl_fuchsia_net_interfaces_ext::PortClass::WlanAp => Self::WlanAp,
            fidl_fuchsia_net_interfaces_ext::PortClass::Ppp => Self::Ppp,
            fidl_fuchsia_net_interfaces_ext::PortClass::Bridge => Self::Bridge,
            fidl_fuchsia_net_interfaces_ext::PortClass::Lowpan => Self::Lowpan,
        }
    }
}

#[derive(serde::Serialize)]
/// Intermediary struct for serializing interface properties into JSON.
pub(crate) struct InterfaceView {
    pub(crate) nicid: u64,
    pub(crate) name: String,
    pub(crate) device_class: DeviceClass,
    pub(crate) online: bool,
    pub(crate) addresses: Addresses,
    pub(crate) has_default_ipv4_route: bool,
    pub(crate) has_default_ipv6_route: bool,
    pub(crate) mac: Option<fidl_fuchsia_net_ext::MacAddress>,
    pub(crate) port_identity_koid: Option<zx_types::zx_koid_t>,
}

impl
    From<(
        fidl_fuchsia_net_interfaces_ext::Properties<fidl_fuchsia_net_interfaces_ext::AllInterest>,
        Option<flex_fuchsia_net::MacAddress>,
    )> for InterfaceView
{
    fn from(
        t: (
            fidl_fuchsia_net_interfaces_ext::Properties<
                fidl_fuchsia_net_interfaces_ext::AllInterest,
            >,
            Option<flex_fuchsia_net::MacAddress>,
        ),
    ) -> InterfaceView {
        let (
            fidl_fuchsia_net_interfaces_ext::Properties {
                id,
                name,
                port_class,
                online,
                addresses,
                has_default_ipv4_route,
                has_default_ipv6_route,
                port_identity_koid,
            },
            mac,
        ) = t;
        InterfaceView {
            nicid: id.get(),
            name,
            device_class: port_class.into(),
            online,
            addresses: addresses.into_iter().into(),
            has_default_ipv4_route,
            has_default_ipv6_route,
            mac: mac.map(Into::into),
            port_identity_koid: port_identity_koid.map(|e| e.raw_koid()),
        }
    }
}

#[derive(serde::Serialize, Ord, PartialOrd, Eq, PartialEq)]
/// Intermediary struct for serializing IP forwarding table entries into JSON.
pub struct ForwardingEntry {
    #[serde(rename = "destination")]
    subnet: Subnet<std::net::IpAddr>,
    #[serde(rename = "nicid")]
    device_id: u64,
    #[serde(rename = "gateway")]
    next_hop: Option<std::net::IpAddr>,
    metric: u32,
    table_id: u32,
}

/// Errors returned when converting from [`froutes_ext::InstalledRoute`]
/// to [`ForwardingEntry`].
#[derive(Debug, Error)]
pub enum ForwardingEntryConversionError {
    #[error("the route's action was unknown")]
    UnknownRouteAction,
}

impl<I: net_types::ip::Ip> TryFrom<froutes_ext::InstalledRoute<I>> for ForwardingEntry {
    type Error = ForwardingEntryConversionError;
    fn try_from(route: froutes_ext::InstalledRoute<I>) -> Result<Self, Self::Error> {
        let froutes_ext::InstalledRoute {
            route: froutes_ext::Route { destination, action, properties: _ },
            effective_properties: froutes_ext::EffectiveRouteProperties { metric },
            table_id,
        } = route;
        let (device_id, next_hop) = match action {
            froutes_ext::RouteAction::Forward(froutes_ext::RouteTarget {
                outbound_interface,
                next_hop,
            }) => (outbound_interface, next_hop),
            froutes_ext::RouteAction::Unknown => {
                return Err(ForwardingEntryConversionError::UnknownRouteAction);
            }
        };
        let subnet = destination.into();
        let next_hop = next_hop.map(|next_hop| next_hop.get().to_ip_addr().into());
        Ok(Self { subnet, device_id, next_hop, metric, table_id: table_id.get() })
    }
}

pub struct NeighborTableEntryIteratorItemVariants<T> {
    existing: T,
    added: T,
    changed: T,
    removed: T,
    idle: T,
}

impl<T> NeighborTableEntryIteratorItemVariants<T> {
    pub fn select(self, item: &flex_fuchsia_net_neighbor::EntryIteratorItem) -> T {
        use flex_fuchsia_net_neighbor::EntryIteratorItem;
        let Self { existing, added, changed, removed, idle } = self;
        match item {
            EntryIteratorItem::Existing(_) => existing,
            EntryIteratorItem::Added(_) => added,
            EntryIteratorItem::Changed(_) => changed,
            EntryIteratorItem::Removed(_) => removed,
            EntryIteratorItem::Idle(_) => idle,
        }
    }
}

impl<T> IntoIterator for NeighborTableEntryIteratorItemVariants<T> {
    type Item = T;
    type IntoIter = <[T; 5] as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        let Self { existing, added, changed, removed, idle } = self;
        [existing, added, changed, removed, idle].into_iter()
    }
}

pub const DISPLAYED_NEIGH_ENTRY_VARIANTS: NeighborTableEntryIteratorItemVariants<&'static str> =
    NeighborTableEntryIteratorItemVariants {
        existing: "EXISTING",
        added: "ADDED",
        changed: "CHANGED",
        removed: "REMOVED",
        idle: "IDLE",
    };

/// Intermediary type for serializing Entry (e.g. into JSON).
#[derive(serde::Serialize)]
pub struct NeighborTableEntry {
    interface: u64,
    neighbor: std::net::IpAddr,
    state: &'static str,
    mac: Option<fidl_fuchsia_net_ext::MacAddress>,
}

impl From<fidl_fuchsia_net_neighbor_ext::Entry> for NeighborTableEntry {
    fn from(
        fidl_fuchsia_net_neighbor_ext::Entry {
            interface,
            neighbor,
            state,
            mac,
            // Ignored since the tabular format ignores this field.
            updated_at: _,
        }: fidl_fuchsia_net_neighbor_ext::Entry,
    ) -> NeighborTableEntry {
        let fidl_fuchsia_net_ext::IpAddress(neighbor) = neighbor.into();
        NeighborTableEntry {
            interface: interface.into(),
            neighbor,
            state: fidl_fuchsia_net_neighbor_ext::display_entry_state(&state),
            mac: mac.map(|mac| mac.into()),
        }
    }
}
