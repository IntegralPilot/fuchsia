// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Lock ordering for Netstack3 core.
//!
//! This module contains the "lock ordering" for Netstack3 core: it describes
//! the order in which additional locks can be acquired while other locks are
//! held. In general, code that is written to avoid deadlocks must respect the
//! same lock ordering.
//!
//! Netstack3 core relies on the [`lock_order`] crate to ensure that all
//! possible code paths respect the lock ordering defined here. By leveraging
//! the types and traits from `lock_order`, we can guarantee at compile time
//! that there are no opportunities for deadlock. The way this works is that
//! each lock in [`CoreCtx`] and its associated per-device state gets assigned a
//! type in this module. Then, for a pair of locks `A` and `B`, where `B` is
//! allowed to be acquired while `A` is locked, there is a corresponding
//! declaration of the [`lock_order::relation::LockAfter`] trait (done via the
//! [`impl_lock_after`] macro).
//!
//! Notionally, the lock ordering forms a [directed acyclic graph (DAG)], where
//! the nodes in the graph are locks that can be acquired, and each edge
//! represents a `LockAfter` implementation. For a lock `B` to be acquired while
//! some lock `A` is held, the node for `B` in the graph must be reachable from
//! `A`, either via direct edge or via an indirect path that traverses
//! other nodes. This can also be thought of as a [strict partial order] between
//! locks, where `A < B` means `B` can be acquired with `A` held.
//!
//! Ideally we'd represent the lock ordering here that way too, but it doesn't
//! work well with the blanket impls of `LockAfter` emitted by the `impl_lock_after` macro.
//! We want to use `impl_lock_after` since it helps prevent deadlocks (see
//! documentation for [`lock_order::relation`]). The problem can be illustrated
//! by this reduced example. Suppose we have a lock ordering DAG like this:
//!
//! ```text
//! ┌────────────────────┐
//! │LoopbackRx          │
//! └┬──────────────────┬┘
//! ┌▽─────────────┐  ┌─▽────────────┐
//! │IpState<Ipv4> │  │IpState<Ipv6> │
//! └┬─────────────┘  └┬─────────────┘
//! ┌▽─────────────────▽┐
//! │DevicesState       │
//! └───────────────────┘
//!```
//!
//! With the blanket `LockAfter` impls, we'd get this:
//! ```no_compile
//! impl<X> LockAfter<X> for DevicesState where IpState<Ipv4>: LockAfter<X> {}
//! // and
//! impl<X> LockAfter<X> for DevicesState where IpState<Ipv6>: LockAfter<X> {}
//! ```
//!
//! Since `X` could be `LoopbackRx`, we'd have duplicate impls of
//! `LockAfter<LoopbackRx> for DevicesState`.
//!
//! To work around this, we pick a different graph for the lock ordering that
//! won't produce duplicate blanket impls. The property we need is that every
//! path in the original graph is also present in our alternate graph. Luckily,
//! graph theory proves that this is always possible: a [topological sort] has
//! precisely this property, and every DAG has at least one topological sort.
//! For the graph above, that could look something like this:
//!
//! ```text
//! ┌────────────────────┐
//! │LoopbackRx          │
//! └┬───────────────────┘
//! ┌▽─────────────┐
//! │IpState<Ipv4> │
//! └┬─────────────┘
//! ┌▽─────────────┐
//! │IpState<Ipv6> │
//! └┬─────────────┘
//! ┌▽──────────────────┐
//! │DevicesState       │
//! └───────────────────┘
//! ```
//!
//! Note that every possible lock ordering path in the original graph is present
//! (directly or transitively) in the second graph. There are additional paths,
//! like `IpState<Ipv4> -> IpState<Ipv6>`, but those don't matter since
//!   a) if they become load-bearing, they need to be in the original DAG, and
//!   b) load-bearing or not, the result is still a valid lock ordering graph.
//!
//! The lock ordering described in this module is likewise a modification of
//! the ideal graph. Instead of specifying the actual DAG, we make nice to the
//! Rust compiler so we can use the transitive blanket impls instead. Since the
//! graph doesn't have multiple paths between any two nodes (that's what causes
//! the duplicate blanket impls), it ends up being a [multitree]. While we could
//! turn it into a (linear) total ordering, we keep the tree structure so that
//! we can more faithfully represent that some locks can't be held at the same
//! time by putting them on different branches.
//!
//! If, in the future, someone comes up with a better way to declare the lock
//! ordering graph and preserve cycle rejection, we should be able to migrate
//! the definitions here without affecting the usages.
//!
//! [`CoreCtx`]: crate::CoreCtx
//! [directed acyclic graph (DAG)]: https://en.wikipedia.org/wiki/Directed_acyclic_graph
//! [strict partial order]: https://en.wikipedia.org/wiki/Partially_ordered_set#Strict_partial_orders
//! [topological sort]: https://en.wikipedia.org/wiki/Topological_sorting
//! [multitree]: https://en.wikipedia.org/wiki/Multitree

pub(crate) use lock_order::Unlocked;

use core::convert::Infallible as Never;
use core::marker::PhantomData;

use lock_order::impl_lock_after;
use lock_order::relation::LockAfter;
use net_types::ip::{Ipv4, Ipv6};

// This lock level is used to provide unlocked access to stack state that does
// not require any locking.
//
// Implementations of UnlockedAccess must ensure that they do not provide access
// to any state that is held by a lock, in order not to sidestep the global lock
// order declared below.
pub enum UnlockedState {}

pub struct IcmpAllSocketsSet<I>(PhantomData<I>, Never);
pub struct IcmpSocketState<I>(PhantomData<I>, Never);
pub struct IcmpBoundMap<I>(PhantomData<I>, Never);

pub struct IcmpTokenBucket<I>(PhantomData<I>, Never);

pub struct TcpAllSocketsSet<I>(PhantomData<I>, Never);
pub struct TcpSocketState<I>(PhantomData<I>, Never);
pub struct TcpDemux<I>(PhantomData<I>, Never);
pub struct UdpAllSocketsSet<I>(PhantomData<I>, Never);
pub struct UdpSocketState<I>(PhantomData<I>, Never);
pub struct UdpBoundMap<I>(PhantomData<I>, Never);

pub struct IpDeviceConfiguration<I>(PhantomData<I>, Never);
pub struct IpDeviceGmp<I>(PhantomData<I>, Never);
pub struct IpDeviceAddresses<I>(PhantomData<I>, Never);
pub struct IpDeviceFlags<I>(PhantomData<I>, Never);
pub struct IpDeviceDefaultHopLimit<I>(PhantomData<I>, Never);
pub struct IpDeviceAddressDad<I>(PhantomData<I>, Never);
pub struct IpDeviceAddressData<I>(PhantomData<I>, Never);

pub enum Ipv6DeviceRouterSolicitations {}
pub enum Ipv6DeviceRouteDiscovery {}
pub enum Ipv6DeviceLearnedParams {}
pub enum Ipv6DeviceSlaac {}
pub struct NudConfig<I>(PhantomData<I>, Never);

// This is not a real lock level, but it is useful for writing bounds that
// require "before IPv4" or "before IPv6".
pub struct IpState<I>(PhantomData<I>, Never);
pub struct IpStatePmtuCache<I>(PhantomData<I>, Never);
pub struct IpStateFragmentCache<I>(PhantomData<I>, Never);
pub struct IpStateRulesTable<I>(PhantomData<I>, Never);
pub struct IpStateRoutingTables<I>(PhantomData<I>, Never);
pub struct IpStateRoutingTable<I>(PhantomData<I>, Never);
// Lock level attributed to the state of an individual raw IP sockets.
pub struct RawIpSocketState<I>(PhantomData<I>, Never);
// Lock level attributed to the collection of all raw IP sockets.
pub struct AllRawIpSockets<I>(PhantomData<I>, Never);

// Lock level attributed to all multicast forwarding state.
pub struct IpMulticastForwardingState<I>(PhantomData<I>, Never);
// Lock level attributed to the multicast routing table.
pub struct IpMulticastRouteTable<I>(PhantomData<I>, Never);
// Lock level attributed to the table of pending multicast packets.
pub struct IpMulticastForwardingPendingPackets<I>(PhantomData<I>, Never);

pub enum DeviceLayerState {}
pub enum AllDeviceSockets {}
pub enum AnyDeviceSockets {}
pub enum DeviceSocketState {}
pub enum DeviceSockets {}
pub struct EthernetDeviceIpState<I>(PhantomData<I>, Never);
pub enum EthernetDeviceDynamicState {}
pub enum PureIpDeviceDynamicState {}

pub enum EthernetIpv4Arp {}
pub enum EthernetIpv6Nud {}
pub enum EthernetTxQueue {}
pub enum EthernetTxDequeue {}
// We do not actually have a dedicated RX queue for ethernet, but we want to have a
// clear separation between the ethernet layer and above (IP/ARP) without specifying
// any specific protocol. To do this, we introduce this lock-level to show the
// "boundary" between ethernet-level RX path work and upper level RX path work.
//
// Note that if/when an RX queue is implemented for ethernet, this lock-level may be
// trivially used.
pub enum EthernetRxDequeue {}

pub enum LoopbackRxQueue {}
pub enum LoopbackRxDequeue {}
pub enum LoopbackTxQueue {}
pub enum LoopbackTxDequeue {}

pub enum PureIpDeviceTxQueue {}
pub enum PureIpDeviceTxDequeue {}
// Note: Pure IP devices do not have an RX queue. This lock marker exists to
// provide separation between the "device" layer, and the IP layer. If an RX
// queue is introduced in the future, this lock-level may be trivially used.
pub enum PureIpDeviceRxDequeue {}

pub struct FilterState<I>(PhantomData<I>, Never);

impl LockAfter<Unlocked> for LoopbackTxDequeue {}
impl_lock_after!(LoopbackTxDequeue => EthernetTxDequeue);
impl_lock_after!(EthernetTxDequeue => PureIpDeviceTxDequeue);
impl_lock_after!(PureIpDeviceTxDequeue => LoopbackRxDequeue);
impl_lock_after!(LoopbackRxDequeue => EthernetRxDequeue);
impl_lock_after!(EthernetRxDequeue => PureIpDeviceRxDequeue);
impl_lock_after!(PureIpDeviceRxDequeue => IcmpAllSocketsSet<Ipv4>);
impl_lock_after!(IcmpAllSocketsSet<Ipv4> => IcmpAllSocketsSet<Ipv6>);
impl_lock_after!(IcmpAllSocketsSet<Ipv6> => IcmpSocketState<Ipv4>);
impl_lock_after!(IcmpSocketState<Ipv4> => IcmpBoundMap<Ipv4>);
impl_lock_after!(IcmpBoundMap<Ipv4> => IcmpSocketState<Ipv6>);
impl_lock_after!(IcmpSocketState<Ipv6> => IcmpBoundMap<Ipv6>);
impl_lock_after!(IcmpBoundMap<Ipv6> => AllRawIpSockets<Ipv4>);
impl_lock_after!(AllRawIpSockets<Ipv4> => AllRawIpSockets<Ipv6>);
impl_lock_after!(AllRawIpSockets<Ipv6> => RawIpSocketState<Ipv4>);
impl_lock_after!(RawIpSocketState<Ipv4> => RawIpSocketState<Ipv6>);
impl_lock_after!(RawIpSocketState<Ipv6> => TcpAllSocketsSet<Ipv4>);

// Ideally we'd have separate impls `LoopbackRxDequeue =>
// TcpAllSocketsSet<Ipv4>` and for `Ipv6`, but that doesn't play well with the
// blanket impls. Linearize IPv4 and IPv6, and TCP and UDP, like for `IpState`
// below.
impl_lock_after!(TcpAllSocketsSet<Ipv4> => TcpAllSocketsSet<Ipv6>);
impl_lock_after!(TcpAllSocketsSet<Ipv6> => TcpSocketState<Ipv4>);
impl_lock_after!(TcpSocketState<Ipv4> => TcpSocketState<Ipv6>);
impl_lock_after!(TcpSocketState<Ipv6> => TcpDemux<Ipv4>);
impl_lock_after!(TcpDemux<Ipv4> => TcpDemux<Ipv6>);
impl_lock_after!(TcpDemux<Ipv6> => UdpAllSocketsSet<Ipv4>);
impl_lock_after!(UdpAllSocketsSet<Ipv4> => UdpAllSocketsSet<Ipv6>);
impl_lock_after!(UdpAllSocketsSet<Ipv6> => UdpSocketState<Ipv4>);
impl_lock_after!(UdpSocketState<Ipv4> => UdpSocketState<Ipv6>);
impl_lock_after!(UdpSocketState<Ipv6> => UdpBoundMap<Ipv4>);
impl_lock_after!(UdpBoundMap<Ipv4> => UdpBoundMap<Ipv6>);
impl_lock_after!(UdpBoundMap<Ipv6> => IcmpTokenBucket<Ipv4>);
impl_lock_after!(IcmpTokenBucket<Ipv4> => IcmpTokenBucket<Ipv6>);
impl_lock_after!(IcmpTokenBucket<Ipv6> => IpMulticastForwardingState<Ipv4>);
impl_lock_after!(IpMulticastForwardingState<Ipv4> => IpMulticastRouteTable<Ipv4>);
impl_lock_after!(IpMulticastRouteTable<Ipv4> => IpMulticastForwardingPendingPackets<Ipv4>);
impl_lock_after!(IpMulticastForwardingPendingPackets<Ipv4> => IpMulticastForwardingState<Ipv6>);
impl_lock_after!(IpMulticastForwardingState<Ipv6> => IpMulticastRouteTable<Ipv6>);
impl_lock_after!(IpMulticastRouteTable<Ipv6> => IpMulticastForwardingPendingPackets<Ipv6>);
impl_lock_after!(IpMulticastForwardingPendingPackets<Ipv6> => IpStateRulesTable<Ipv4>);
impl_lock_after!(IpStateRulesTable<Ipv4> => IpStateRulesTable<Ipv6>);
impl_lock_after!(IpStateRulesTable<Ipv6> => IpStateRoutingTables<Ipv4>);
impl_lock_after!(IpStateRoutingTables<Ipv4> => IpStateRoutingTables<Ipv6>);
impl_lock_after!(IpStateRoutingTables<Ipv6> => IpStateRoutingTable<Ipv4>);
impl_lock_after!(IpStateRoutingTable<Ipv4> => IpStateRoutingTable<Ipv6>);
impl_lock_after!(IpStateRoutingTable<Ipv6> => IpDeviceConfiguration<Ipv4>);
impl_lock_after!(IpDeviceConfiguration<Ipv4> => IpDeviceConfiguration<Ipv6>);
impl_lock_after!(IpDeviceConfiguration<Ipv6> => DeviceLayerState);
impl_lock_after!(DeviceLayerState => Ipv6DeviceRouteDiscovery);
impl_lock_after!(Ipv6DeviceRouteDiscovery => Ipv6DeviceSlaac);
impl_lock_after!(Ipv6DeviceSlaac => IpDeviceAddressDad<Ipv4>);
impl_lock_after!(IpDeviceAddressDad<Ipv4> => IpDeviceAddressDad<Ipv6>);
impl_lock_after!(IpDeviceAddressDad<Ipv6> => IpDeviceGmp<Ipv4>);

impl_lock_after!(IpDeviceGmp<Ipv4> => IpDeviceGmp<Ipv6>);
impl_lock_after!(IpDeviceGmp<Ipv6> => FilterState<Ipv4>);

impl_lock_after!(FilterState<Ipv4> => FilterState<Ipv6>);
impl_lock_after!(FilterState<Ipv6> => IpState<Ipv4>);
impl_lock_after!(IpState<Ipv4> => IpState<Ipv6>);

impl_lock_after!(IpState<Ipv4> => IpStatePmtuCache<Ipv4>);
impl_lock_after!(IpState<Ipv6> => IpStatePmtuCache<Ipv6>);
impl_lock_after!(IpState<Ipv4> => IpStateFragmentCache<Ipv4>);
impl_lock_after!(IpState<Ipv6> => IpStateFragmentCache<Ipv6>);

impl_lock_after!(IpState<Ipv6> => LoopbackTxQueue);
impl_lock_after!(LoopbackTxQueue => LoopbackRxQueue);
impl_lock_after!(LoopbackTxQueue => EthernetIpv4Arp);
impl_lock_after!(EthernetIpv4Arp => EthernetIpv6Nud);
impl_lock_after!(EthernetIpv6Nud => AllDeviceSockets);

impl_lock_after!(AllDeviceSockets => AnyDeviceSockets);
impl_lock_after!(AnyDeviceSockets => EthernetDeviceIpState<Ipv4>);
impl_lock_after!(EthernetDeviceIpState<Ipv4> => IpDeviceAddresses<Ipv4>);
impl_lock_after!(IpDeviceAddresses<Ipv4> => IpDeviceAddresses<Ipv6>);
impl_lock_after!(IpDeviceAddresses<Ipv6> => IpDeviceFlags<Ipv4>);
impl_lock_after!(IpDeviceFlags<Ipv4> => IpDeviceFlags<Ipv6>);
impl_lock_after!(IpDeviceFlags<Ipv6> => IpDeviceAddressData<Ipv4>);
impl_lock_after!(IpDeviceAddressData<Ipv4> => IpDeviceAddressData<Ipv6>);
impl_lock_after!(IpDeviceAddressData<Ipv6> => IpDeviceDefaultHopLimit<Ipv4>);
impl_lock_after!(IpDeviceDefaultHopLimit<Ipv4> => EthernetDeviceIpState<Ipv6>);
impl_lock_after!(EthernetDeviceIpState<Ipv6> => IpDeviceDefaultHopLimit<Ipv6>);
impl_lock_after!(IpDeviceDefaultHopLimit<Ipv6> => Ipv6DeviceRouterSolicitations);
impl_lock_after!(Ipv6DeviceRouterSolicitations => Ipv6DeviceLearnedParams);
impl_lock_after!(Ipv6DeviceLearnedParams => NudConfig<Ipv4>);
impl_lock_after!(NudConfig<Ipv4> => NudConfig<Ipv6>);
impl_lock_after!(NudConfig<Ipv6> => EthernetDeviceDynamicState);
impl_lock_after!(EthernetDeviceDynamicState => EthernetTxQueue);
impl_lock_after!(EthernetTxQueue => PureIpDeviceDynamicState);
impl_lock_after!(PureIpDeviceDynamicState => PureIpDeviceTxQueue);

impl_lock_after!(AnyDeviceSockets => DeviceSockets);
impl_lock_after!(DeviceSockets => DeviceSocketState);
