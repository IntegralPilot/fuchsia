// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Declares types and functionality related to multicast routes.

use alloc::fmt::Debug;
use alloc::sync::Arc;
use core::hash::Hash;
use core::num::NonZeroU8;
use core::sync::atomic::Ordering;
use derivative::Derivative;
use net_types::ip::{GenericOverIp, Ip, Ipv4, Ipv4Addr, Ipv6, Ipv6Addr, Ipv6Scope};
use net_types::{
    MulticastAddr, NonMappedAddr, NonMulticastAddr, ScopeableAddress as _, SpecifiedAddr,
    UnicastAddr,
};
use netstack3_base::{
    AtomicInstant, Inspectable, InspectableValue, Inspector, InspectorDeviceExt,
    InstantBindingsTypes, IpExt, StrongDeviceIdentifier,
};

/// A witness type wrapping [`Ipv4Addr`], proving the following properties:
/// * the inner address is specified, and
/// * the inner address is not a multicast address.
/// * the inner address's scope is greater than link local.
///
/// Note, unlike for [`Ipv6SourceAddr`], the `UnicastAddr` witness type cannot
/// be used. This is because "unicastness" is not an absolute property of an
/// IPv4 address: it requires knowing the subnet in which the address is being
/// used.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ipv4SourceAddr {
    addr: NonMulticastAddr<SpecifiedAddr<Ipv4Addr>>,
}

impl Ipv4SourceAddr {
    /// Construct a new [`Ipv4SourceAddr`].
    ///
    /// `None` if the provided address does not have the required properties.
    fn new(addr: Ipv4Addr) -> Option<Self> {
        if Ipv4::LINK_LOCAL_UNICAST_SUBNET.contains(&addr) {
            return None;
        }

        Some(Ipv4SourceAddr { addr: NonMulticastAddr::new(SpecifiedAddr::new(addr)?)? })
    }
}

impl From<Ipv4SourceAddr> for net_types::ip::Ipv4SourceAddr {
    fn from(addr: Ipv4SourceAddr) -> Self {
        net_types::ip::Ipv4SourceAddr::Specified(addr.addr)
    }
}

/// A witness type wrapping [`Ipv4Addr`], proving the following properties:
/// * the inner address is multicast, and
/// * the inner address's scope is greater than link local.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ipv4DestinationAddr {
    addr: MulticastAddr<Ipv4Addr>,
}

impl Ipv4DestinationAddr {
    /// Construct a new [`Ipv4DestinationAddr`].
    ///
    /// `None` if the provided address does not have the required properties.
    fn new(addr: Ipv4Addr) -> Option<Self> {
        // As per RFC 5771 Section 4:
        //   Addresses in the Local Network Control Block are used for protocol
        //   control traffic that is not forwarded off link.
        if Ipv4::LINK_LOCAL_MULTICAST_SUBNET.contains(&addr) {
            None
        } else {
            Some(Ipv4DestinationAddr { addr: MulticastAddr::new(addr)? })
        }
    }
}

impl From<Ipv4DestinationAddr> for SpecifiedAddr<Ipv4Addr> {
    fn from(addr: Ipv4DestinationAddr) -> Self {
        addr.addr.into_specified()
    }
}

/// A witness type wrapping [`Ipv6Addr`], proving the following properties:
/// * the inner address is unicast, and
/// * the inner address's scope is greater than link local.
/// * the inner address is non-mapped.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ipv6SourceAddr {
    addr: NonMappedAddr<UnicastAddr<Ipv6Addr>>,
}

impl Ipv6SourceAddr {
    /// Construct a new [`Ipv6SourceAddr`].
    ///
    /// `None` if the provided address does not have the required properties.
    fn new(addr: Ipv6Addr) -> Option<Self> {
        let addr = NonMappedAddr::new(UnicastAddr::new(addr)?)?;
        match addr.scope() {
            Ipv6Scope::InterfaceLocal | Ipv6Scope::LinkLocal => None,
            Ipv6Scope::Reserved(_) | Ipv6Scope::Unassigned(_) => None,
            Ipv6Scope::RealmLocal | Ipv6Scope::AdminLocal | Ipv6Scope::OrganizationLocal => {
                unreachable!("Observed a multicast scope ID on a known unicast address");
            }
            Ipv6Scope::SiteLocal | Ipv6Scope::Global => Some(Ipv6SourceAddr { addr }),
        }
    }
}

impl From<Ipv6SourceAddr> for net_types::ip::Ipv6SourceAddr {
    fn from(addr: Ipv6SourceAddr) -> Self {
        net_types::ip::Ipv6SourceAddr::Unicast(addr.addr)
    }
}

/// A witness type wrapping [`Ipv6Addr`], proving the following properties:
/// * the inner address is multicast, and
/// * the inner address's scope is greater than link local.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ipv6DestinationAddr {
    addr: MulticastAddr<Ipv6Addr>,
}

impl Ipv6DestinationAddr {
    /// Construct a new [`Ipv6DestinationAddr`].
    ///
    /// `None` if the provided address does not have the required properties.
    fn new(addr: Ipv6Addr) -> Option<Self> {
        // As per RFC 4291 Section 2.7:
        //   Routers must not forward any multicast packets beyond of the scope
        //   indicated by the scop field in the destination multicast address.
        if addr.scope().multicast_scope_id() <= Ipv6Scope::MULTICAST_SCOPE_ID_REALM_LOCAL {
            None
        } else {
            Some(Ipv6DestinationAddr { addr: MulticastAddr::new(addr)? })
        }
    }
}

impl From<Ipv6DestinationAddr> for SpecifiedAddr<Ipv6Addr> {
    fn from(addr: Ipv6DestinationAddr) -> Self {
        addr.addr.into_specified()
    }
}

/// IP extension trait for multicast routes.
pub trait MulticastRouteIpExt: IpExt {
    /// The type of source address used in [`MulticastRouteKey`].
    type SourceAddress: Clone
        + Debug
        + Eq
        + Hash
        + Ord
        + PartialEq
        + PartialOrd
        + Into<Self::RecvSrcAddr>;
    /// The type of destination address used in [`MulticastRouteKey`].
    type DestinationAddress: Clone
        + Debug
        + Eq
        + Hash
        + Ord
        + PartialEq
        + PartialOrd
        + Into<SpecifiedAddr<Self::Addr>>;
}

impl MulticastRouteIpExt for Ipv4 {
    type SourceAddress = Ipv4SourceAddr;
    type DestinationAddress = Ipv4DestinationAddr;
}

impl MulticastRouteIpExt for Ipv6 {
    type SourceAddress = Ipv6SourceAddr;
    type DestinationAddress = Ipv6DestinationAddr;
}

/// The attributes of a multicast route that uniquely identify it.
#[derive(Clone, Debug, Eq, GenericOverIp, Hash, Ord, PartialEq, PartialOrd)]
#[generic_over_ip(I, Ip)]
pub struct MulticastRouteKey<I: MulticastRouteIpExt> {
    /// The source address packets must have in order to use this route.
    pub(crate) src_addr: I::SourceAddress,
    /// The destination address packets must have in order to use this route.
    pub(crate) dst_addr: I::DestinationAddress,
}

impl<I: MulticastRouteIpExt> MulticastRouteKey<I> {
    /// Construct a new [`MulticastRouteKey`].
    ///
    /// `None` if the provided addresses do not have the required properties.
    pub fn new(src_addr: I::Addr, dst_addr: I::Addr) -> Option<MulticastRouteKey<I>> {
        I::map_ip(
            (src_addr, dst_addr),
            |(src_addr, dst_addr)| {
                Some(MulticastRouteKey {
                    src_addr: Ipv4SourceAddr::new(src_addr)?,
                    dst_addr: Ipv4DestinationAddr::new(dst_addr)?,
                })
            },
            |(src_addr, dst_addr)| {
                Some(MulticastRouteKey {
                    src_addr: Ipv6SourceAddr::new(src_addr)?,
                    dst_addr: Ipv6DestinationAddr::new(dst_addr)?,
                })
            },
        )
    }

    /// Returns the source address, stripped of all its witnesses.
    pub fn src_addr(&self) -> I::Addr {
        I::map_ip(self, |key| **key.src_addr.addr, |key| **key.src_addr.addr)
    }

    /// Returns the destination address, stripped of all its witnesses.
    pub fn dst_addr(&self) -> I::Addr {
        I::map_ip(self, |key| *key.dst_addr.addr, |key| *key.dst_addr.addr)
    }
}

impl<I: MulticastRouteIpExt> Inspectable for MulticastRouteKey<I> {
    fn record<II: Inspector>(&self, inspector: &mut II) {
        inspector.record_ip_addr("SourceAddress", self.src_addr());
        inspector.record_ip_addr("DestinationAddress", self.dst_addr());
    }
}

/// An entry in the multicast route table.
#[derive(Derivative)]
#[derivative(Debug(bound = ""))]
pub struct MulticastRouteEntry<D: StrongDeviceIdentifier, BT: InstantBindingsTypes> {
    pub(crate) route: MulticastRoute<D>,
    // NB: Hold the statistics as an AtomicInstant so that they can be updated
    // without write-locking the multicast route table.
    pub(crate) stats: MulticastRouteStats<BT::AtomicInstant>,
}

impl<D: StrongDeviceIdentifier, BT: InstantBindingsTypes> MulticastRouteEntry<D, BT> {
    /// Writes this [`MulticastRouteEntry`] to the inspector.
    // NB: This exists as a method rather than an implementation of
    // `Inspectable` because we need to restrict the type of `I::DeviceId`.
    pub(crate) fn inspect<I: Inspector, II: InspectorDeviceExt<D>>(&self, inspector: &mut I) {
        let MulticastRouteEntry {
            route: MulticastRoute { input_interface, action },
            stats: MulticastRouteStats { last_used },
        } = self;
        II::record_device(inspector, "InputInterface", input_interface);
        let Action::Forward(targets) = action;
        inspector.record_child("ForwardingTargets", |inspector| {
            for MulticastRouteTarget { output_interface, min_ttl } in targets.iter() {
                inspector.record_unnamed_child(|inspector| {
                    II::record_device(inspector, "OutputInterface", output_interface);
                    inspector.record_uint("MinTTL", min_ttl.get());
                });
            }
        });
        inspector.record_child("Statistics", |inspector| {
            last_used.load(Ordering::Relaxed).record("LastUsed", inspector);
        });
    }
}

/// All attributes of a multicast route, excluding the [`MulticastRouteKey`].
///
/// This type acts as a witness that the route is valid.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MulticastRoute<D: StrongDeviceIdentifier> {
    /// The interface on which packets must arrive in order to use this route.
    pub(crate) input_interface: D,
    /// The route's action.
    pub(crate) action: Action<D>,
}

/// The action to be taken for a packet that matches a route.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Action<D: StrongDeviceIdentifier> {
    /// Forward the packet out of each provided [`Target`].
    Forward(MulticastRouteTargets<D>),
}

/// The collection of targets out of which to forward a multicast packet.
///
/// Note, storing the targets behind an `Arc` allows us to return a reference
/// to the targets, to contexts that are not protected by the multicast route
/// table lock, without cloning the underlying data. Here, an `Arc<Mutex<...>>`
/// is unnecessary, because the underlying targets list is never modified. This
/// is not to say that a route's targets are never modified (e.g. device removal
/// prunes the list of targets); in such cases the route's target list is
/// *replaced* with a new allocation. This strategy allows us to avoid
/// additional locking on the hot path, at the cost of extra allocations for
/// certain control operations.
pub type MulticastRouteTargets<D> = Arc<[MulticastRouteTarget<D>]>;

/// The target out of which to forward a multicast packet.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct MulticastRouteTarget<D: StrongDeviceIdentifier> {
    /// An interface the packet should be forwarded out of.
    pub output_interface: D,
    /// The minimum TTL of packets that will be forwarded out this interface.
    /// Evaluated on egress (e.g. after the netstack has decremented the
    /// packet's TTL).
    pub min_ttl: NonZeroU8,
}

/// Errors returned by [`MulticastRoute::new_forward`].
#[derive(Debug, Eq, PartialEq)]
pub enum ForwardMulticastRouteError {
    /// The route's list of targets is empty.
    EmptyTargetList,
    /// The route's `input_interface` is also listed as a target. This would
    /// create a routing loop.
    InputInterfaceIsTarget,
    /// The route lists the same [`Target`] output_interface multiple times.
    DuplicateTarget,
}

impl<D: StrongDeviceIdentifier> MulticastRoute<D> {
    /// Construct a new [`MulticastRoute`] with [`Action::Forward`].
    pub fn new_forward(
        input_interface: D,
        targets: MulticastRouteTargets<D>,
    ) -> Result<Self, ForwardMulticastRouteError> {
        if targets.is_empty() {
            return Err(ForwardMulticastRouteError::EmptyTargetList);
        }
        if targets.iter().any(|MulticastRouteTarget { output_interface, min_ttl: _ }| {
            output_interface == &input_interface
        }) {
            return Err(ForwardMulticastRouteError::InputInterfaceIsTarget);
        }

        // NB: Search for duplicates by doing a naive n^2 comparison. This is
        // expected to be more performant than other approaches (e.g.
        // sort + dedup, or collecting into a hash map) given how small the vec
        // is expected to be.
        for (index, target_a) in targets.iter().enumerate() {
            // NB: Only check the targets that occur in the vec *after* this
            // one. The targets before this one were checked in previous
            // iterations.
            if targets[index + 1..]
                .iter()
                .any(|target_b| target_a.output_interface == target_b.output_interface)
            {
                return Err(ForwardMulticastRouteError::DuplicateTarget);
            }
        }

        Ok(MulticastRoute { input_interface, action: Action::Forward(targets) })
    }
}

/// Statistics about a [`MulticastRoute`].
#[derive(Debug, Eq, PartialEq)]
pub struct MulticastRouteStats<Instant> {
    /// The last time the route was used to route a packet.
    ///
    /// This value is initialized to the current time when a route is installed
    /// in the route table, and updated every time the route is selected during
    /// multicast route lookup. Notably, it is updated regardless of whether the
    /// packet is actually forwarded; it might be dropped after the routing
    /// decision for a number reasons (e.g. dropped by the filtering engine,
    /// dropped at the device layer, etc).
    pub last_used: Instant,
}

#[cfg(test)]
mod tests {
    use super::*;

    use alloc::vec;
    use alloc::vec::Vec;
    use net_declare::{net_ip_v4, net_ip_v6};
    use netstack3_base::testutil::MultipleDevicesId;
    use test_case::test_case;

    const UNICAST_V4: Ipv4Addr = net_ip_v4!("192.0.2.1");
    const MULTICAST_V4: Ipv4Addr = net_ip_v4!("224.0.1.1");
    const LL_UNICAST_V4: Ipv4Addr = net_ip_v4!("169.254.0.1");
    const LL_MULTICAST_V4: Ipv4Addr = net_ip_v4!("224.0.0.1");
    const UNICAST_V6: Ipv6Addr = net_ip_v6!("2001:0DB8::1");
    const MULTICAST_V6: Ipv6Addr = net_ip_v6!("ff0e::1");
    const LL_UNICAST_V6: Ipv6Addr = net_ip_v6!("fe80::1");
    const LL_MULTICAST_V6: Ipv6Addr = net_ip_v6!("ff02::1");
    const V4_MAPPED_V6: Ipv6Addr = net_ip_v6!("::FFFF:192.0.2.1");
    const RL_MULTICAST_V6: Ipv6Addr = net_ip_v6!("ff03::1");

    #[test_case(UNICAST_V4, MULTICAST_V4 => true; "success")]
    #[test_case(UNICAST_V4, UNICAST_V4 => false; "unicast_dst")]
    #[test_case(UNICAST_V4, Ipv4::UNSPECIFIED_ADDRESS => false; "unspecified_dst")]
    #[test_case(MULTICAST_V4, MULTICAST_V4 => false; "multicast_src")]
    #[test_case(Ipv4::UNSPECIFIED_ADDRESS, MULTICAST_V4 => false; "unspecified_src")]
    #[test_case(LL_UNICAST_V4, MULTICAST_V4 => false; "ll_unicast_src")]
    #[test_case(UNICAST_V4, LL_MULTICAST_V4 => false; "ll_multicast_dst")]
    fn new_ipv4_route_key(src_addr: Ipv4Addr, dst_addr: Ipv4Addr) -> bool {
        MulticastRouteKey::<Ipv4>::new(src_addr, dst_addr).is_some()
    }

    #[test_case(UNICAST_V6, MULTICAST_V6 => true; "success")]
    #[test_case(UNICAST_V6, UNICAST_V6 => false; "unicast_dst")]
    #[test_case(UNICAST_V6, Ipv6::UNSPECIFIED_ADDRESS => false; "unspecified_dst")]
    #[test_case(MULTICAST_V6, MULTICAST_V6 => false; "multicast_src")]
    #[test_case(Ipv6::UNSPECIFIED_ADDRESS, MULTICAST_V6 => false; "unspecified_src")]
    #[test_case(LL_UNICAST_V6, MULTICAST_V6 => false; "ll_unicast_src")]
    #[test_case(UNICAST_V6, LL_MULTICAST_V6 => false; "ll_multicast_dst")]
    #[test_case(V4_MAPPED_V6, LL_MULTICAST_V6 => false; "mapped_src")]
    #[test_case(UNICAST_V6, RL_MULTICAST_V6 => false; "rl_multicast_dst")]
    fn new_ipv6_route_key(src_addr: Ipv6Addr, dst_addr: Ipv6Addr) -> bool {
        MulticastRouteKey::<Ipv6>::new(src_addr, dst_addr).is_some()
    }

    #[test_case(MultipleDevicesId::A, vec![] =>
        Some(ForwardMulticastRouteError::EmptyTargetList); "empty_target_list")]
    #[test_case(MultipleDevicesId::A, vec![MultipleDevicesId::A] =>
        Some(ForwardMulticastRouteError::InputInterfaceIsTarget); "input_interface_is_target")]
    #[test_case(MultipleDevicesId::A, vec![MultipleDevicesId::B, MultipleDevicesId::B] =>
        Some(ForwardMulticastRouteError::DuplicateTarget); "duplicate_target")]
    #[test_case(MultipleDevicesId::A, vec![MultipleDevicesId::B, MultipleDevicesId::C] =>
        None; "valid_route")]
    fn new_forward(
        input_interface: MultipleDevicesId,
        output_interfaces: Vec<MultipleDevicesId>,
    ) -> Option<ForwardMulticastRouteError> {
        let targets = output_interfaces
            .into_iter()
            .map(|output_interface| MulticastRouteTarget {
                output_interface,
                min_ttl: NonZeroU8::new(1).unwrap(),
            })
            .collect();
        MulticastRoute::new_forward(input_interface, targets).err()
    }
}
