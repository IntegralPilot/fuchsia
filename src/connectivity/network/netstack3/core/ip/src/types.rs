// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Common types for dealing with ip table entries.

use core::fmt::{Debug, Display, Formatter};
use core::hash::Hash;

use net_types::SpecifiedAddr;
use net_types::ip::{GenericOverIp, Ip, IpAddress, Ipv4Addr, Ipv6Addr, Subnet, SubnetEither};
use netstack3_base::socket::SocketIpAddr;
use netstack3_base::{BroadcastIpExt, IpDeviceAddr};

/// Route preferences learned from network, default Medium.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum RoutePreference {
    /// Low preference.
    Low,
    /// Medium preference.
    #[default]
    Medium,
    /// High preference.
    High,
}

impl From<packet_formats::icmp::ndp::RoutePreference> for RoutePreference {
    fn from(p: packet_formats::icmp::ndp::RoutePreference) -> Self {
        match p {
            packet_formats::icmp::ndp::RoutePreference::Low => RoutePreference::Low,
            packet_formats::icmp::ndp::RoutePreference::Medium => RoutePreference::Medium,
            packet_formats::icmp::ndp::RoutePreference::High => RoutePreference::High,
        }
    }
}

impl From<RoutePreference> for packet_formats::icmp::ndp::RoutePreference {
    fn from(p: RoutePreference) -> Self {
        match p {
            RoutePreference::Low => packet_formats::icmp::ndp::RoutePreference::Low,
            RoutePreference::Medium => packet_formats::icmp::ndp::RoutePreference::Medium,
            RoutePreference::High => packet_formats::icmp::ndp::RoutePreference::High,
        }
    }
}

/// The priority of a forwarding entry. Lower metrics are preferred.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
pub struct RawMetric(pub u32);

impl RawMetric {
    /// The highest possible preference for a metric value, i.e., the lowest
    /// numerical value.
    pub const HIGHEST_PREFERENCE: Self = Self(u32::MIN);
    /// The lowest possible preference for a metric value, i.e., the highest
    /// numerical value.
    pub const LOWEST_PREFERENCE: Self = Self(u32::MAX);
}

impl Display for RawMetric {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), core::fmt::Error> {
        let RawMetric(metric) = self;
        write!(f, "{}", metric)
    }
}

impl From<RawMetric> for u32 {
    fn from(RawMetric(metric): RawMetric) -> u32 {
        metric
    }
}

impl From<RawMetric> for u64 {
    fn from(RawMetric(metric): RawMetric) -> u64 {
        u64::from(metric)
    }
}

/// The metric for an [`AddableEntry`].
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
pub enum AddableMetric {
    /// The entry's metric is unspecified, and shall defer to the routing metric
    /// of its interface.
    MetricTracksInterface,
    /// The entry's metric shall be the following explicit value.
    ExplicitMetric(RawMetric),
}

impl From<Metric> for AddableMetric {
    fn from(metric: Metric) -> AddableMetric {
        match metric {
            Metric::MetricTracksInterface(_) => AddableMetric::MetricTracksInterface,
            Metric::ExplicitMetric(metric) => AddableMetric::ExplicitMetric(metric),
        }
    }
}

/// `AddableEntry` is a routing entry that may be used to add a new entry to the
/// forwarding table.
///
/// See [`Entry`] for the type used to represent a route in the forwarding
/// table.
#[derive(Debug, Copy, Clone, Eq, GenericOverIp, PartialEq, Hash)]
#[generic_over_ip(A, IpAddress)]
pub struct AddableEntry<A: IpAddress, D> {
    /// The destination subnet.
    pub subnet: Subnet<A>,
    /// The outgoing interface.
    pub device: D,
    /// Next hop.
    pub gateway: Option<SpecifiedAddr<A>>,
    /// Route metric.
    pub metric: AddableMetric,
    /// The preference of the route learned from network. Unlike the metric, it
    /// is not specified by the user.
    pub route_preference: RoutePreference,
}

impl<D, A: IpAddress> AddableEntry<A, D> {
    /// Creates a new [`AddableEntry`] with a specified gateway.
    pub fn with_gateway(
        subnet: Subnet<A>,
        device: D,
        gateway: SpecifiedAddr<A>,
        metric: AddableMetric,
    ) -> Self {
        Self {
            subnet,
            device,
            gateway: Some(gateway),
            metric,
            route_preference: Default::default(),
        }
    }

    /// Creates a new [`AddableEntry`] with a specified device.
    pub fn without_gateway(subnet: Subnet<A>, device: D, metric: AddableMetric) -> Self {
        Self { subnet, device, gateway: None, metric, route_preference: Default::default() }
    }

    /// Converts the `AddableEntry` to an `Entry`.
    pub fn resolve_metric(self, device_metric: RawMetric) -> Entry<A, D> {
        let Self { subnet, device, gateway, metric, route_preference } = self;
        let metric = match metric {
            AddableMetric::MetricTracksInterface => Metric::MetricTracksInterface(device_metric),
            AddableMetric::ExplicitMetric(metric) => Metric::ExplicitMetric(metric),
        };
        Entry { subnet, device, gateway, metric, route_preference }
    }

    /// Maps the device ID held by this `AddableEntry`.
    pub fn map_device_id<D2>(self, f: impl FnOnce(D) -> D2) -> AddableEntry<A, D2> {
        let Self { subnet, device, gateway, metric, route_preference } = self;
        AddableEntry { subnet, device: f(device), gateway, metric, route_preference }
    }

    /// Fallibly maps the device ID held by this `AddableEntry`.
    pub fn try_map_device_id<D2, E>(
        self,
        f: impl FnOnce(D) -> Result<D2, E>,
    ) -> Result<AddableEntry<A, D2>, E> {
        let Self { subnet, device, gateway, metric, route_preference } = self;
        Ok(AddableEntry { subnet, device: f(device)?, gateway, metric, route_preference })
    }

    /// Sets the generation on an entry.
    pub fn with_generation(self, generation: Generation) -> AddableEntryAndGeneration<A, D> {
        AddableEntryAndGeneration { entry: self, generation }
    }
}

/// An IPv4 forwarding entry or an IPv6 forwarding entry.
#[allow(missing_docs)]
#[derive(Debug, Copy, Clone, Eq, GenericOverIp, PartialEq, Hash)]
#[generic_over_ip()]
pub enum AddableEntryEither<D> {
    V4(AddableEntry<Ipv4Addr, D>),
    V6(AddableEntry<Ipv6Addr, D>),
}

impl<D> AddableEntryEither<D> {
    /// Creates a new [`AddableEntryEither`] with the specified device as the
    /// next hop.
    pub fn without_gateway(subnet: SubnetEither, device: D, metric: AddableMetric) -> Self {
        match subnet {
            SubnetEither::V4(subnet) => {
                AddableEntry::without_gateway(subnet, device, metric).into()
            }
            SubnetEither::V6(subnet) => {
                AddableEntry::without_gateway(subnet, device, metric).into()
            }
        }
    }
}

impl<A: IpAddress, D> From<AddableEntry<A, D>> for AddableEntryEither<D> {
    fn from(entry: AddableEntry<A, D>) -> AddableEntryEither<D> {
        A::Version::map_ip(entry, AddableEntryEither::V4, AddableEntryEither::V6)
    }
}

/// A routing table entry together with the generation it was created in.
#[derive(Debug, Copy, Clone, GenericOverIp)]
#[generic_over_ip(A, IpAddress)]
pub struct AddableEntryAndGeneration<A: IpAddress, D> {
    /// The entry.
    pub entry: AddableEntry<A, D>,
    /// The generation in which it was created.
    pub generation: Generation,
}

impl<A: IpAddress, D> From<Entry<A, D>> for AddableEntry<A, D> {
    fn from(Entry { subnet, device, gateway, metric, route_preference }: Entry<A, D>) -> Self {
        Self {
            subnet: subnet,
            device: device,
            gateway: gateway,
            metric: metric.into(),
            route_preference,
        }
    }
}

/// The metric for an [`Entry`].
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
pub enum Metric {
    /// The entry's metric tracks its interface's routing metric and has the
    /// included value.
    MetricTracksInterface(RawMetric),
    /// The entry's metric was explicitly set to the included value.
    ExplicitMetric(RawMetric),
}

impl Metric {
    /// Returns the underlying value of the `Metric`.
    pub fn value(&self) -> RawMetric {
        match self {
            Self::MetricTracksInterface(value) => *value,
            Self::ExplicitMetric(value) => *value,
        }
    }
}

/// A forwarding entry.
///
/// `Entry` is a `Subnet` with an egress device and optional gateway.
#[derive(Debug, Copy, Clone, Eq, GenericOverIp, PartialEq, Hash)]
#[generic_over_ip(A, IpAddress)]
pub struct Entry<A: IpAddress, D> {
    /// The matching subnet.
    pub subnet: Subnet<A>,
    /// The destination device.
    pub device: D,
    /// An optional gateway if the subnet is not on link.
    // TODO(https://fxbug.dev/42074188): Restrict `gateway` to `UnicastAddr`.
    pub gateway: Option<SpecifiedAddr<A>>,
    /// The metric of the entry.
    pub metric: Metric,
    /// The preference of the route.
    pub route_preference: RoutePreference,
}

/// A forwarding entry with the generation it was created in.
#[derive(Debug, Copy, Clone, GenericOverIp, PartialEq, Eq)]
#[generic_over_ip(A, IpAddress)]
pub struct EntryAndGeneration<A: IpAddress, D> {
    /// The entry.
    pub entry: Entry<A, D>,
    /// The generation.
    pub generation: Generation,
}

impl<A: IpAddress, D: Debug> Display for EntryAndGeneration<A, D> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), core::fmt::Error> {
        let EntryAndGeneration { entry, generation: Generation(generation) } = self;
        write!(f, "{} (generation = {})", entry, generation)
    }
}

/// Used to compare routes for how early they were added to the table.
///
/// If two routes have the same prefix length and metric, and are both on-link
/// or are both-off-link, then the route with the earlier generation will be
/// preferred.
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Generation(u64);

impl Generation {
    /// Returns the initial generation.
    pub fn initial() -> Self {
        Self(0)
    }

    /// Returns the next generation.
    pub fn next(&self) -> Generation {
        let Self(n) = self;
        Generation(n + 1)
    }
}

impl<A: IpAddress, D> Entry<A, D> {
    /// Maps the device ID held by this `Entry`.
    pub fn map_device_id<D2>(self, f: impl FnOnce(D) -> D2) -> Entry<A, D2> {
        let Self { subnet, device, gateway, metric, route_preference } = self;
        Entry { subnet, device: f(device), gateway, metric, route_preference }
    }

    /// Sets the generation on an entry.
    pub fn with_generation(self, generation: Generation) -> EntryAndGeneration<A, D> {
        EntryAndGeneration { entry: self, generation }
    }
}

impl<A: IpAddress, D: Debug> Display for Entry<A, D> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), core::fmt::Error> {
        let Entry { subnet, device, gateway, metric, route_preference } = self;
        match gateway {
            Some(gateway) => {
                write!(
                    f,
                    "{:?} (via {}) -> {} metric {} pref {:?}",
                    device,
                    gateway,
                    subnet,
                    metric.value(),
                    route_preference
                )
            }
            None => write!(
                f,
                "{:?} -> {} metric {} pref {:?}",
                device,
                subnet,
                metric.value(),
                route_preference
            ),
        }
    }
}

/// An IPv4 forwarding entry or an IPv6 forwarding entry.
#[allow(missing_docs)]
#[derive(Debug, Copy, Clone, Eq, GenericOverIp, PartialEq)]
#[generic_over_ip()]
pub enum EntryEither<D> {
    V4(Entry<Ipv4Addr, D>),
    V6(Entry<Ipv6Addr, D>),
}

impl<A: IpAddress, D> From<Entry<A, D>> for EntryEither<D> {
    fn from(entry: Entry<A, D>) -> EntryEither<D> {
        A::Version::map_ip(entry, EntryEither::V4, EntryEither::V6)
    }
}

/// `OrderedLocality` provides an implementation of `core::cmp::PartialOrd` for
/// a route's "locality".
// Define an enum, so that `OnLink` routes are sorted before `OffLink` routes.
// See https://doc.rust-lang.org/core/cmp/trait.PartialOrd.html#derivable for
// more details.
#[derive(PartialEq, PartialOrd, Eq, Ord)]
pub(crate) enum OrderedLocality {
    // The route does not have a gateway.
    OnLink,
    // The route does have a gateway.
    OffLink,
}

// `OrderedRoute` provides an implementation of `core::cmp::PartialOrd`
// for routes. Note that the fields are consulted in the order they are
// declared. For more details, see
// https://doc.rust-lang.org/core/cmp/trait.PartialOrd.html#derivable.
#[derive(PartialEq, PartialOrd, Eq, Ord)]
pub(crate) struct OrderedEntry<'a, A: IpAddress, D> {
    // Order longer prefixes before shorter prefixes.
    prefix_len: core::cmp::Reverse<u8>,
    // Order `OnLink` routes before `OffLink` routes.
    //
    // Adhere to industry norms by considering the route locality before the
    // route metric.
    locality: OrderedLocality,
    // Order lower metrics before larger metrics.
    metric: u32,
    // Order higher preference routes before lower preference ones.
    route_preference: core::cmp::Reverse<RoutePreference>,
    // Earlier-added routes should come before later ones.
    generation: Generation,
    // To provide a consistent ordering, tiebreak using the remaining fields
    // of the entry.
    subnet_addr: A,
    device: &'a D,
    // Note that while this appears to duplicate the ordering provided by
    // `locality`, it's important that we sort above on presence of the gateway
    // and not on the actual address of the gateway. The latter is only used
    // for tiebreaking at the end to provide a total order. Duplicating it this
    // way allows us to avoid writing a manual `PartialOrd` impl.
    gateway: Option<SpecifiedAddr<A>>,
}

impl<'a, A: IpAddress, D> From<&'a EntryAndGeneration<A, D>> for OrderedEntry<'a, A, D> {
    fn from(entry: &'a EntryAndGeneration<A, D>) -> OrderedEntry<'a, A, D> {
        let EntryAndGeneration {
            entry: Entry { subnet, device, gateway, metric, route_preference },
            generation,
        } = entry;
        OrderedEntry {
            prefix_len: core::cmp::Reverse(subnet.prefix()),
            locality: gateway.map_or(OrderedLocality::OnLink, |_gateway| OrderedLocality::OffLink),
            route_preference: core::cmp::Reverse(*route_preference),
            metric: metric.value().into(),
            generation: *generation,
            subnet_addr: subnet.network(),
            device: &device,
            gateway: *gateway,
        }
    }
}

/// The next hop for a [`Destination`].
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum NextHop<A: IpAddress>
where
    A::Version: BroadcastIpExt,
{
    /// Indicates that the next-hop for a the packet is the remote since it is a
    /// neighboring node (on-link).
    RemoteAsNeighbor,
    /// Indicates that the next-hop is a gateway/router since the remote is not
    /// a neighboring node (off-link).
    Gateway(SpecifiedAddr<A>),
    /// Indicates that the packet should be broadcast rather than sent to a
    /// specific neighbor.
    Broadcast(<A::Version as BroadcastIpExt>::BroadcastMarker),
}

impl<A: IpAddress> NextHop<A>
where
    A::Version: BroadcastIpExt,
{
    /// True if the `NextHop` is `Broadcast`.
    pub fn is_broadcast(self) -> bool {
        match self {
            Self::Broadcast(_) => true,
            Self::RemoteAsNeighbor | Self::Gateway(_) => false,
        }
    }
}

impl<A: IpAddress, NewIp: BroadcastIpExt> GenericOverIp<NewIp> for NextHop<A>
where
    A::Version: BroadcastIpExt,
{
    type Type = NextHop<NewIp::Addr>;
}

/// An IP Address that witnesses properties needed to be routed.
pub type RoutableIpAddr<A> = SocketIpAddr<A>;

/// The resolved route to a destination IP address.
#[derive(Debug, Copy, Clone, PartialEq, Eq, GenericOverIp)]
#[generic_over_ip(I, Ip)]
pub struct ResolvedRoute<I: BroadcastIpExt, D> {
    /// The source address to use when forwarding packets towards the
    /// destination.
    pub src_addr: IpDeviceAddr<I::Addr>,
    /// The device over which this destination can be reached.
    pub device: D,
    /// Present when `device` is loopback with the device that the destination
    /// address is assigned to.
    ///
    /// NB: it's possible that `local_delivery_device` is itself loopback.
    pub local_delivery_device: Option<D>,
    /// The next hop via which this destination can be reached.
    pub next_hop: NextHop<I::Addr>,
    /// The route's internal forwarding semantics.
    pub internal_forwarding: InternalForwarding<D>,
}

/// Internal forwarding semantics.
///
/// Internal forwarding allows the netstack to behave as a Weak Host when
/// forwarding is enabled on a device.
///
/// In a sending context, internal forwarding allows sending a packet out of a
/// device using a source address not assigned to that device, provided that the
/// source address is assigned to another device, and that other device has
/// forwarding enabled.
///
/// In a receiving context, internal forwarding allows receiving a packet that
/// was destined to an address not assigned to the device that is arrived on,
/// provided that destination address is assigned to another device and the
/// device the packet arrived on has forwarding enabled.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum InternalForwarding<D> {
    /// Internal forwarding is being used. The relevant address (src addr when
    /// sending, dst addr when receiving) is assigned to the provided device.
    Used(D),
    /// Internal forwarding is not being used.
    NotUsed,
}

impl<D> InternalForwarding<D> {
    /// Applies the given callback to the held device identifier.
    pub fn map_device<F: FnOnce(D) -> O, O>(self, cb: F) -> InternalForwarding<O> {
        match self {
            InternalForwarding::NotUsed => InternalForwarding::NotUsed,
            InternalForwarding::Used(d) => InternalForwarding::Used(cb(d)),
        }
    }
}

/// The destination of an outbound IP packet.
///
/// Outbound IP packets are sent to a particular device (specified by the
/// `device` field).
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Destination<A: IpAddress, D>
where
    A::Version: BroadcastIpExt,
{
    /// Indicates the next hop via which this destination can be reached.
    pub next_hop: NextHop<A>,
    /// Indicates the device over which this destination can be reached.
    pub device: D,
}
