// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! State for an IP device.

use alloc::vec::Vec;
use core::fmt::Debug;
use core::hash::Hash;
use core::num::{NonZeroU8, NonZeroU16};
use core::ops::{Deref, DerefMut};
use core::time::Duration;

use derivative::Derivative;
use lock_order::lock::{OrderedLockAccess, OrderedLockRef};
use net_types::Witness as _;
use net_types::ip::{AddrSubnet, GenericOverIp, Ip, IpMarked, IpVersionMarker, Ipv4, Ipv6};
use netstack3_base::sync::{Mutex, PrimaryRc, RwLock, StrongRc, WeakRc};
use netstack3_base::{
    AssignedAddrIpExt, BroadcastIpExt, CoreTimerContext, ExistsError, Inspectable,
    InspectableValue, Inspector, Instant, InstantBindingsTypes, IpAddressId,
    NestedIntoCoreTimerCtx, NotFoundError, ReferenceNotifiers, TimerBindingsTypes, TimerContext,
    WeakDeviceIdentifier,
};
use packet_formats::icmp::ndp::NonZeroNdpLifetime;
use packet_formats::utils::NonZeroDuration;

use crate::internal::counters::{IpCounters, IpCountersIpExt};
use crate::internal::device::dad::{DadIpExt, DadState};
use crate::internal::device::route_discovery::{
    Ipv6RouteDiscoveryState, RouteDiscoveryConfiguration,
};
use crate::internal::device::router_solicitation::RsState;
use crate::internal::device::slaac::{SlaacConfiguration, SlaacState};
use crate::internal::device::{
    IpDeviceAddr, IpDeviceTimerId, Ipv4DeviceTimerId, Ipv6DeviceTimerId, WeakIpAddressId,
};
use crate::internal::gmp::igmp::{IgmpConfig, IgmpCounters, IgmpTimerId, IgmpTypeLayout};
use crate::internal::gmp::mld::{MldConfig, MldCounters, MldTimerId, MldTypeLayout};
use crate::internal::gmp::{GmpGroupState, GmpState, GmpTimerId, GmpTypeLayout, MulticastGroupSet};
use crate::internal::types::RawMetric;

/// The default value for the default hop limit to be used when sending IP
/// packets.
const DEFAULT_HOP_LIMIT: NonZeroU8 = NonZeroU8::new(64).unwrap();

/// An `Ip` extension trait adding IP device state properties.
pub trait IpDeviceStateIpExt:
    DadIpExt + BroadcastIpExt + IpCountersIpExt + AssignedAddrIpExt
{
    /// Configuration held for an IP address assigned to an interface.
    type AddressConfig<I: Instant>: Default + Debug + Inspectable + PartialEq + Send + Sync;
    /// The GMP protocol-specific configuration.
    type GmpProtoConfig: Default;
    /// The GMP type layout used by IP-version specific state.
    type GmpTypeLayout<BT: IpDeviceStateBindingsTypes>: GmpTypeLayout<Self, BT>;
    /// The timer id for GMP timers.
    type GmpTimerId<D: WeakDeviceIdentifier>: From<GmpTimerId<Self, D>>;
}

impl IpDeviceStateIpExt for Ipv4 {
    type AddressConfig<I: Instant> = Ipv4AddrConfig<I>;
    type GmpTimerId<D: WeakDeviceIdentifier> = IgmpTimerId<D>;
    type GmpProtoConfig = IgmpConfig;
    type GmpTypeLayout<BT: IpDeviceStateBindingsTypes> = IgmpTypeLayout;
}

impl IpDeviceStateIpExt for Ipv6 {
    type AddressConfig<I: Instant> = Ipv6AddrConfig<I>;
    type GmpTimerId<D: WeakDeviceIdentifier> = MldTimerId<D>;
    type GmpProtoConfig = MldConfig;
    type GmpTypeLayout<BT: IpDeviceStateBindingsTypes> = MldTypeLayout;
}

/// The primary reference to the state associated with an IP address assigned
/// to an IP device.
pub struct PrimaryAddressId<I: IpDeviceStateIpExt, BT: IpDeviceStateBindingsTypes>(
    PrimaryRc<IpAddressEntry<I, BT>>,
);

impl<I: IpDeviceStateIpExt, BT: IpDeviceStateBindingsTypes> Debug for PrimaryAddressId<I, BT> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let Self(rc) = self;
        write!(f, "PrimaryAddressId({:?} => {})", rc.debug_id(), rc.addr_sub)
    }
}

impl<I: IpDeviceStateIpExt, BT: IpDeviceStateBindingsTypes> Deref for PrimaryAddressId<I, BT> {
    type Target = IpAddressEntry<I, BT>;

    fn deref(&self) -> &Self::Target {
        let Self(inner) = self;
        inner.deref()
    }
}

impl<I: IpDeviceStateIpExt, BT: IpDeviceStateBindingsTypes> PrimaryAddressId<I, BT> {
    /// Creates a new primary reference to the provided state.
    fn new_with_strong_clone(addr: IpAddressEntry<I, BT>) -> (Self, AddressId<I, BT>) {
        let primary = PrimaryRc::new(addr);
        let strong = PrimaryRc::clone_strong(&primary);
        (Self(primary), AddressId(strong))
    }

    /// Clones a strongly-held reference.
    pub fn clone_strong(&self) -> AddressId<I, BT> {
        let Self(inner) = self;
        AddressId(PrimaryRc::clone_strong(inner))
    }

    /// Checks for equality with the provided strongly-held reference.
    pub fn ptr_eq(&self, other: &AddressId<I, BT>) -> bool {
        let Self(inner) = self;
        let AddressId(other) = other;
        PrimaryRc::ptr_eq(inner, other)
    }

    /// Consumes `self` and returns the inner [`PrimaryRc`].
    pub fn into_inner(self) -> PrimaryRc<IpAddressEntry<I, BT>> {
        self.0
    }

    /// The underlying address for this ID.
    pub fn addr(&self) -> I::AssignedWitness {
        let Self(inner) = self;
        inner.addr_sub.addr()
    }
}

/// A strongly-held reference to an IP address assigned to a device.
#[derive(Derivative)]
#[derivative(Clone(bound = ""), Eq(bound = ""), Hash(bound = ""), PartialEq(bound = ""))]
pub struct AddressId<I: IpDeviceStateIpExt, BT: IpDeviceStateBindingsTypes>(
    StrongRc<IpAddressEntry<I, BT>>,
);

impl<I: IpDeviceStateIpExt, BT: IpDeviceStateBindingsTypes> Debug for AddressId<I, BT> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let Self(rc) = self;
        write!(f, "AddressId({:?} => {})", rc.debug_id(), self.addr_sub)
    }
}

impl<I: IpDeviceStateIpExt, BT: IpDeviceStateBindingsTypes> Deref for AddressId<I, BT> {
    type Target = IpAddressEntry<I, BT>;

    fn deref(&self) -> &Self::Target {
        let Self(inner) = self;
        inner.deref()
    }
}

impl<I: IpDeviceStateIpExt, BT: IpDeviceStateBindingsTypes> IpAddressId<I::Addr>
    for AddressId<I, BT>
{
    type Weak = WeakAddressId<I, BT>;

    fn downgrade(&self) -> Self::Weak {
        let Self(inner) = self;
        WeakAddressId(StrongRc::downgrade(inner))
    }

    fn addr(&self) -> IpDeviceAddr<I::Addr> {
        let Self(inner) = self;
        inner.addr_sub().addr().into()
    }

    fn addr_sub(&self) -> AddrSubnet<I::Addr, I::AssignedWitness> {
        let Self(inner) = self;
        *inner.addr_sub()
    }
}

/// A weakly-held reference to an IP address assigned to a device.
#[derive(Derivative)]
#[derivative(Clone(bound = ""), Eq(bound = ""), Hash(bound = ""), PartialEq(bound = ""))]
pub struct WeakAddressId<I: IpDeviceStateIpExt, BT: IpDeviceStateBindingsTypes>(
    WeakRc<IpAddressEntry<I, BT>>,
);

impl<I: IpDeviceStateIpExt, BT: IpDeviceStateBindingsTypes> Debug for WeakAddressId<I, BT> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let Self(rc) = self;
        if let Some(id) = self.upgrade() {
            write!(f, "WeakAddressId({:?} => {})", rc.debug_id(), id.addr_sub)
        } else {
            write!(f, "WeakAddressId({:?} => {})", rc.debug_id(), I::NAME)
        }
    }
}

impl<I: IpDeviceStateIpExt, BT: IpDeviceStateBindingsTypes> InspectableValue
    for WeakAddressId<I, BT>
{
    fn record<II: Inspector>(&self, name: &str, inspector: &mut II) {
        inspector.record_debug(name, self);
    }
}

impl<I: IpDeviceStateIpExt, BT: IpDeviceStateBindingsTypes> WeakIpAddressId<I::Addr>
    for WeakAddressId<I, BT>
{
    type Strong = AddressId<I, BT>;

    fn upgrade(&self) -> Option<Self::Strong> {
        let Self(inner) = self;
        inner.upgrade().map(AddressId)
    }

    fn is_assigned(&self) -> bool {
        let Self(inner) = self;
        inner.strong_count() != 0
    }
}

/// The flags for an IP device.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct IpDeviceFlags {
    /// Is the device enabled?
    pub ip_enabled: bool,
}

/// The state kept for each device to handle multicast group membership.
pub struct IpDeviceMulticastGroups<I: IpDeviceStateIpExt, BT: IpDeviceStateBindingsTypes> {
    /// Multicast groups this device has joined.
    pub groups: MulticastGroupSet<I::Addr, GmpGroupState<I, BT>>,
    /// GMP state.
    pub gmp: GmpState<I, I::GmpTypeLayout<BT>, BT>,
    /// GMP protocol-specific configuration.
    pub gmp_config: I::GmpProtoConfig,
}

/// A container for the default hop limit kept by [`IpDeviceState`].
///
/// This type makes the [`OrderedLockAccess`] implementation clearer by
/// newtyping the `NonZeroU8` value and adding a version marker.
#[derive(Copy, Clone, Debug)]
pub struct DefaultHopLimit<I: Ip>(NonZeroU8, IpVersionMarker<I>);

impl<I: Ip> Deref for DefaultHopLimit<I> {
    type Target = NonZeroU8;
    fn deref(&self) -> &NonZeroU8 {
        let Self(value, IpVersionMarker { .. }) = self;
        value
    }
}

impl<I: Ip> DerefMut for DefaultHopLimit<I> {
    fn deref_mut(&mut self) -> &mut NonZeroU8 {
        let Self(value, IpVersionMarker { .. }) = self;
        value
    }
}

impl<I: Ip> Default for DefaultHopLimit<I> {
    fn default() -> Self {
        Self(DEFAULT_HOP_LIMIT, IpVersionMarker::new())
    }
}

/// The state common to all IP devices.
#[derive(GenericOverIp)]
#[generic_over_ip(I, Ip)]
pub struct IpDeviceState<I: IpDeviceStateIpExt, BT: IpDeviceStateBindingsTypes> {
    /// IP addresses assigned to this device.
    ///
    /// IPv6 addresses may be tentative (performing NDP's Duplicate Address
    /// Detection).
    ///
    /// Does not contain any duplicates.
    addrs: RwLock<IpDeviceAddresses<I, BT>>,

    /// Multicast groups and GMP handling state.
    multicast_groups: RwLock<IpDeviceMulticastGroups<I, BT>>,

    /// The default TTL (IPv4) or hop limit (IPv6) for outbound packets sent
    /// over this device.
    default_hop_limit: RwLock<DefaultHopLimit<I>>,

    /// The flags for this device.
    flags: Mutex<IpMarked<I, IpDeviceFlags>>,

    /// The IP counters for this device.
    ip_counters: IpCounters<I>,
}

impl<I: IpDeviceStateIpExt, BT: IpDeviceStateBindingsTypes>
    OrderedLockAccess<IpDeviceAddresses<I, BT>> for DualStackIpDeviceState<BT>
{
    type Lock = RwLock<IpDeviceAddresses<I, BT>>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.ip_state::<I>().addrs)
    }
}

impl<I: IpDeviceStateIpExt, BT: IpDeviceStateBindingsTypes>
    OrderedLockAccess<IpDeviceMulticastGroups<I, BT>> for DualStackIpDeviceState<BT>
{
    type Lock = RwLock<IpDeviceMulticastGroups<I, BT>>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.ip_state::<I>().multicast_groups)
    }
}

impl<I: IpDeviceStateIpExt, BT: IpDeviceStateBindingsTypes> OrderedLockAccess<DefaultHopLimit<I>>
    for DualStackIpDeviceState<BT>
{
    type Lock = RwLock<DefaultHopLimit<I>>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.ip_state::<I>().default_hop_limit)
    }
}

impl<I: IpDeviceStateIpExt, BT: IpDeviceStateBindingsTypes>
    OrderedLockAccess<IpMarked<I, IpDeviceFlags>> for DualStackIpDeviceState<BT>
{
    type Lock = Mutex<IpMarked<I, IpDeviceFlags>>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.ip_state::<I>().flags)
    }
}

impl<BT: IpDeviceStateBindingsTypes> OrderedLockAccess<SlaacState<BT>>
    for DualStackIpDeviceState<BT>
{
    type Lock = Mutex<SlaacState<BT>>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.ipv6.slaac_state)
    }
}

impl<I: IpDeviceStateIpExt, BT: IpDeviceStateBindingsTypes> IpDeviceState<I, BT> {
    /// A direct accessor to `IpDeviceAddresses` available in tests.
    #[cfg(any(test, feature = "testutils"))]
    pub fn addrs(&self) -> &RwLock<IpDeviceAddresses<I, BT>> {
        &self.addrs
    }
}

impl<I: IpDeviceStateIpExt, BC: IpDeviceStateBindingsTypes + TimerContext> IpDeviceState<I, BC> {
    fn new<D: WeakDeviceIdentifier, CC: CoreTimerContext<I::GmpTimerId<D>, BC>>(
        bindings_ctx: &mut BC,
        device_id: D,
    ) -> IpDeviceState<I, BC> {
        IpDeviceState {
            addrs: Default::default(),
            multicast_groups: RwLock::new(IpDeviceMulticastGroups {
                groups: Default::default(),
                gmp: GmpState::new::<_, NestedIntoCoreTimerCtx<CC, _>>(bindings_ctx, device_id),
                gmp_config: Default::default(),
            }),
            default_hop_limit: Default::default(),
            flags: Default::default(),
            ip_counters: Default::default(),
        }
    }
}

/// A device's IP addresses for IP version `I`.
#[derive(Derivative)]
#[derivative(Default(bound = ""))]
#[cfg_attr(test, derive(Debug))]
pub struct IpDeviceAddresses<I: Ip + IpDeviceStateIpExt, BT: IpDeviceStateBindingsTypes> {
    addrs: Vec<PrimaryAddressId<I, BT>>,
}

// TODO(https://fxbug.dev/42165707): Once we figure out what invariants we want to
// hold regarding the set of IP addresses assigned to a device, ensure that all
// of the methods on `IpDeviceAddresses` uphold those invariants.
impl<I: IpDeviceStateIpExt, BT: IpDeviceStateBindingsTypes> IpDeviceAddresses<I, BT> {
    /// Iterates over the addresses assigned to this device.
    pub fn iter(
        &self,
    ) -> impl ExactSizeIterator<Item = &PrimaryAddressId<I, BT>> + ExactSizeIterator + Clone {
        self.addrs.iter()
    }

    /// Iterates over strong clones of addresses assigned to this device.
    pub fn strong_iter(&self) -> AddressIdIter<'_, I, BT> {
        AddressIdIter(self.addrs.iter())
    }

    /// Adds an IP address to this interface.
    pub fn add(&mut self, addr: IpAddressEntry<I, BT>) -> Result<AddressId<I, BT>, ExistsError> {
        if self.iter().any(|a| a.addr() == addr.addr_sub.addr()) {
            return Err(ExistsError);
        }
        let (primary, strong) = PrimaryAddressId::new_with_strong_clone(addr);
        self.addrs.push(primary);
        Ok(strong)
    }

    /// Removes the address.
    pub fn remove(&mut self, addr: &I::Addr) -> Result<PrimaryAddressId<I, BT>, NotFoundError> {
        let (index, _entry): (_, &PrimaryAddressId<I, BT>) = self
            .addrs
            .iter()
            .enumerate()
            .find(|(_, entry)| entry.addr().get() == *addr)
            .ok_or(NotFoundError)?;
        Ok(self.addrs.remove(index))
    }
}

/// An iterator over address StrongIds. Created from `IpDeviceAddresses`.
pub struct AddressIdIter<'a, I: Ip + IpDeviceStateIpExt, BT: IpDeviceStateBindingsTypes>(
    core::slice::Iter<'a, PrimaryAddressId<I, BT>>,
);

impl<'a, I: Ip + IpDeviceStateIpExt, BT: IpDeviceStateBindingsTypes> Iterator
    for AddressIdIter<'a, I, BT>
{
    type Item = AddressId<I, BT>;

    fn next(&mut self) -> Option<Self::Item> {
        let Self(inner) = self;
        inner.next().map(|addr| addr.clone_strong())
    }
}

/// The state common to all IPv4 devices.
pub struct Ipv4DeviceState<BT: IpDeviceStateBindingsTypes> {
    ip_state: IpDeviceState<Ipv4, BT>,
    config: RwLock<Ipv4DeviceConfiguration>,
    igmp_counters: IgmpCounters,
}

impl<BT: IpDeviceStateBindingsTypes> OrderedLockAccess<Ipv4DeviceConfiguration>
    for DualStackIpDeviceState<BT>
{
    type Lock = RwLock<Ipv4DeviceConfiguration>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.ipv4.config)
    }
}

impl<BC: IpDeviceStateBindingsTypes + TimerContext> Ipv4DeviceState<BC> {
    fn new<D: WeakDeviceIdentifier, CC: CoreTimerContext<Ipv4DeviceTimerId<D, BC>, BC>>(
        bindings_ctx: &mut BC,
        device_id: D,
    ) -> Ipv4DeviceState<BC> {
        Ipv4DeviceState {
            ip_state: IpDeviceState::new::<_, NestedIntoCoreTimerCtx<CC, _>>(
                bindings_ctx,
                device_id,
            ),
            config: Default::default(),
            igmp_counters: Default::default(),
        }
    }
}

impl<BT: IpDeviceStateBindingsTypes> Ipv4DeviceState<BT> {
    fn igmp_counters(&self) -> &IgmpCounters {
        &self.igmp_counters
    }
}

impl<BT: IpDeviceStateBindingsTypes> AsRef<IpDeviceState<Ipv4, BT>> for Ipv4DeviceState<BT> {
    fn as_ref(&self) -> &IpDeviceState<Ipv4, BT> {
        &self.ip_state
    }
}

impl<BT: IpDeviceStateBindingsTypes> AsMut<IpDeviceState<Ipv4, BT>> for Ipv4DeviceState<BT> {
    fn as_mut(&mut self) -> &mut IpDeviceState<Ipv4, BT> {
        &mut self.ip_state
    }
}

/// Configurations common to all IP devices.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct IpDeviceConfiguration {
    /// Is a Group Messaging Protocol (GMP) enabled for this device?
    ///
    /// If `gmp_enabled` is false, multicast groups will still be added to
    /// `multicast_groups`, but we will not inform the network of our membership
    /// in those groups using a GMP.
    ///
    /// Default: `false`.
    pub gmp_enabled: bool,

    /// A flag indicating whether forwarding of unicast IP packets not destined
    /// for this device is enabled.
    ///
    /// This flag controls whether or not packets can be forwarded from this
    /// device. That is, when a packet arrives at a device it is not destined
    /// for, the packet can only be forwarded if the device it arrived at has
    /// forwarding enabled and there exists another device that has a path to
    /// the packet's destination, regardless of the other device's forwarding
    /// ability.
    ///
    /// Default: `false`.
    pub unicast_forwarding_enabled: bool,

    /// A flag indicating whether forwarding of multicast IP packets received on
    /// this device is enabled.
    ///
    /// This flag controls whether or not packets can be forwarded from this
    /// device. That is, when a multicast packet arrives at this device, the
    /// multicast routing table will be consulted and the packet will be
    /// forwarded out of the matching route's corresponding outbound devices
    /// (regardless of the outbound device's forwarding ability). Enabling
    /// multicast forwarding does not disrupt local delivery: the packet will
    /// both be forwarded and delivered locally.
    ///
    /// Default: `false`.
    pub multicast_forwarding_enabled: bool,
    /// The number of probes to send as part of Duplicate Address Detection.
    ///
    /// For IPv6, this corresponds to NDP's `DupAddrDetectTransmits` parameter
    /// as defined by RFC 4862 section 5.1.
    ///
    /// For IPv4, this corresponds to ACD's `PROBE_NUM` parameter, as defined by
    /// RFC 5227, section 1.1.
    ///
    /// A value of `None` means DAD will not be performed on the interface.
    pub dad_transmits: Option<NonZeroU16>,
}

/// Configuration common to all IPv4 devices.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Ipv4DeviceConfiguration {
    /// The configuration common to all IP devices.
    pub ip_config: IpDeviceConfiguration,
}

impl Ipv4DeviceConfiguration {
    /// The default `PROBE_NUM` value from RFC 5227 Section 1.1.
    pub const DEFAULT_DUPLICATE_ADDRESS_DETECTION_TRANSMITS: NonZeroU16 =
        NonZeroU16::new(3).unwrap();
}

impl AsRef<IpDeviceConfiguration> for Ipv4DeviceConfiguration {
    fn as_ref(&self) -> &IpDeviceConfiguration {
        &self.ip_config
    }
}

impl AsMut<IpDeviceConfiguration> for Ipv4DeviceConfiguration {
    fn as_mut(&mut self) -> &mut IpDeviceConfiguration {
        &mut self.ip_config
    }
}

/// Configuration common to all IPv6 devices.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Ipv6DeviceConfiguration {
    /// Value for NDP's `MAX_RTR_SOLICITATIONS` parameter to configure how many
    /// router solicitation messages to send when solicing routers.
    ///
    /// A value of `None` means router solicitation will not be performed.
    ///
    /// See [RFC 4861 section 6.3.7] for details.
    ///
    /// [RFC 4861 section 6.3.7]: https://datatracker.ietf.org/doc/html/rfc4861#section-6.3.7
    pub max_router_solicitations: Option<NonZeroU8>,

    /// The configuration for SLAAC.
    pub slaac_config: SlaacConfiguration,

    /// The configuration for route discovery.
    pub route_discovery_config: RouteDiscoveryConfiguration,

    /// The configuration common to all IP devices.
    pub ip_config: IpDeviceConfiguration,
}

impl Ipv6DeviceConfiguration {
    /// The default `MAX_RTR_SOLICITATIONS` value from [RFC 4861 section 10].
    ///
    /// [RFC 4861 section 10]: https://datatracker.ietf.org/doc/html/rfc4861#section-10
    pub const DEFAULT_MAX_RTR_SOLICITATIONS: NonZeroU8 = NonZeroU8::new(3).unwrap();

    /// The default `DupAddrDetectTransmits` value from [RFC 4862 Section 5.1]
    ///
    /// [RFC 4862 Section 5.1]: https://www.rfc-editor.org/rfc/rfc4862#section-5.1
    pub const DEFAULT_DUPLICATE_ADDRESS_DETECTION_TRANSMITS: NonZeroU16 =
        NonZeroU16::new(1).unwrap();
}

impl AsRef<IpDeviceConfiguration> for Ipv6DeviceConfiguration {
    fn as_ref(&self) -> &IpDeviceConfiguration {
        &self.ip_config
    }
}

impl AsMut<IpDeviceConfiguration> for Ipv6DeviceConfiguration {
    fn as_mut(&mut self) -> &mut IpDeviceConfiguration {
        &mut self.ip_config
    }
}

impl<BT: IpDeviceStateBindingsTypes> OrderedLockAccess<Ipv6NetworkLearnedParameters>
    for DualStackIpDeviceState<BT>
{
    type Lock = RwLock<Ipv6NetworkLearnedParameters>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.ipv6.learned_params)
    }
}

impl<BT: IpDeviceStateBindingsTypes> OrderedLockAccess<Ipv6RouteDiscoveryState<BT>>
    for DualStackIpDeviceState<BT>
{
    type Lock = Mutex<Ipv6RouteDiscoveryState<BT>>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.ipv6.route_discovery)
    }
}

impl<BT: IpDeviceStateBindingsTypes> OrderedLockAccess<RsState<BT>> for DualStackIpDeviceState<BT> {
    type Lock = Mutex<RsState<BT>>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.ipv6.router_solicitations)
    }
}

impl<BT: IpDeviceStateBindingsTypes> OrderedLockAccess<Ipv6DeviceConfiguration>
    for DualStackIpDeviceState<BT>
{
    type Lock = RwLock<Ipv6DeviceConfiguration>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.ipv6.config)
    }
}

/// IPv6 device parameters that can be learned from router advertisements.
#[derive(Default)]
pub struct Ipv6NetworkLearnedParameters {
    /// The time between retransmissions of Neighbor Solicitation messages to a
    /// neighbor when resolving the address or when probing the reachability of
    /// a neighbor.
    ///
    ///
    /// See RetransTimer in [RFC 4861 section 6.3.2] for more details.
    ///
    /// [RFC 4861 section 6.3.2]: https://tools.ietf.org/html/rfc4861#section-6.3.2
    pub retrans_timer: Option<NonZeroDuration>,
}

impl Ipv6NetworkLearnedParameters {
    /// Returns the learned retransmission timer if a network-learned one is
    /// available.
    pub fn retrans_timer(&self) -> Option<NonZeroDuration> {
        self.retrans_timer.clone()
    }

    /// Forgets any learned network parameters, resetting to the default values.
    pub fn reset(&mut self) {
        *self = Default::default()
    }
}

/// The state common to all IPv6 devices.
pub struct Ipv6DeviceState<BT: IpDeviceStateBindingsTypes> {
    learned_params: RwLock<Ipv6NetworkLearnedParameters>,
    route_discovery: Mutex<Ipv6RouteDiscoveryState<BT>>,
    router_solicitations: Mutex<RsState<BT>>,
    ip_state: IpDeviceState<Ipv6, BT>,
    config: RwLock<Ipv6DeviceConfiguration>,
    slaac_state: Mutex<SlaacState<BT>>,
    mld_counters: MldCounters,
}

impl<BC: IpDeviceStateBindingsTypes + TimerContext> Ipv6DeviceState<BC> {
    pub fn new<D: WeakDeviceIdentifier, CC: CoreTimerContext<Ipv6DeviceTimerId<D, BC>, BC>>(
        bindings_ctx: &mut BC,
        device_id: D,
    ) -> Self {
        Ipv6DeviceState {
            learned_params: Default::default(),
            route_discovery: Mutex::new(Ipv6RouteDiscoveryState::new::<
                _,
                NestedIntoCoreTimerCtx<CC, _>,
            >(bindings_ctx, device_id.clone())),
            router_solicitations: Default::default(),
            ip_state: IpDeviceState::new::<_, NestedIntoCoreTimerCtx<CC, _>>(
                bindings_ctx,
                device_id.clone(),
            ),
            config: Default::default(),
            slaac_state: Mutex::new(SlaacState::new::<_, NestedIntoCoreTimerCtx<CC, _>>(
                bindings_ctx,
                device_id,
            )),
            mld_counters: Default::default(),
        }
    }
}

impl<BT: IpDeviceStateBindingsTypes> Ipv6DeviceState<BT> {
    fn mld_counters(&self) -> &MldCounters {
        &self.mld_counters
    }
}

impl<BT: IpDeviceStateBindingsTypes> AsRef<IpDeviceState<Ipv6, BT>> for Ipv6DeviceState<BT> {
    fn as_ref(&self) -> &IpDeviceState<Ipv6, BT> {
        &self.ip_state
    }
}

impl<BT: IpDeviceStateBindingsTypes> AsMut<IpDeviceState<Ipv6, BT>> for Ipv6DeviceState<BT> {
    fn as_mut(&mut self) -> &mut IpDeviceState<Ipv6, BT> {
        &mut self.ip_state
    }
}

/// Bindings types required for IP device state.
pub trait IpDeviceStateBindingsTypes:
    InstantBindingsTypes + TimerBindingsTypes + ReferenceNotifiers + 'static
{
}
impl<BT> IpDeviceStateBindingsTypes for BT where
    BT: InstantBindingsTypes + TimerBindingsTypes + ReferenceNotifiers + 'static
{
}

/// IPv4 and IPv6 state combined.
pub struct DualStackIpDeviceState<BT: IpDeviceStateBindingsTypes> {
    /// IPv4 state.
    ipv4: Ipv4DeviceState<BT>,

    /// IPv6 state.
    ipv6: Ipv6DeviceState<BT>,

    /// The device's routing metric.
    metric: RawMetric,
}

impl<BC: IpDeviceStateBindingsTypes + TimerContext> DualStackIpDeviceState<BC> {
    /// Constructs a new `DualStackIpDeviceState` for `device_id`.
    ///
    /// `metric` is the default metric used by routes through this device.
    pub fn new<
        D: WeakDeviceIdentifier,
        CC: CoreTimerContext<IpDeviceTimerId<Ipv6, D, BC>, BC>
            + CoreTimerContext<IpDeviceTimerId<Ipv4, D, BC>, BC>,
    >(
        bindings_ctx: &mut BC,
        device_id: D,
        metric: RawMetric,
    ) -> Self {
        let ipv4 = Ipv4DeviceState::new::<
            D,
            NestedIntoCoreTimerCtx<CC, IpDeviceTimerId<Ipv4, D, BC>>,
        >(bindings_ctx, device_id.clone());
        let ipv6 = Ipv6DeviceState::new::<
            D,
            NestedIntoCoreTimerCtx<CC, IpDeviceTimerId<Ipv6, D, BC>>,
        >(bindings_ctx, device_id);
        Self { ipv4, ipv6, metric }
    }
}

impl<BT: IpDeviceStateBindingsTypes> DualStackIpDeviceState<BT> {
    /// Returns the [`RawMetric`] for this device.
    pub fn metric(&self) -> &RawMetric {
        &self.metric
    }

    /// Returns the IP device state for version `I`.
    pub fn ip_state<I: IpDeviceStateIpExt>(&self) -> &IpDeviceState<I, BT> {
        I::map_ip_out(
            self,
            |dual_stack| &dual_stack.ipv4.ip_state,
            |dual_stack| &dual_stack.ipv6.ip_state,
        )
    }

    /// Access the IGMP counters associated with this specific device state.
    pub fn igmp_counters(&self) -> &IgmpCounters {
        self.ipv4.igmp_counters()
    }

    /// Access the MLD counters associated with this specific device state.
    pub fn mld_counters(&self) -> &MldCounters {
        self.ipv6.mld_counters()
    }

    /// Access the IP counters associated with this specific device state.
    pub fn ip_counters<I: IpDeviceStateIpExt>(&self) -> &IpCounters<I> {
        &self.ip_state::<I>().ip_counters
    }
}

/// Configuration for a temporary IPv6 address assigned via SLAAC.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TemporarySlaacConfig<Instant> {
    /// The time at which the address is no longer valid.
    pub valid_until: Instant,
    /// The per-address DESYNC_FACTOR specified in RFC 8981 Section 3.4.
    pub desync_factor: Duration,
    /// The time at which the address was created.
    pub creation_time: Instant,
    /// The DAD_Counter parameter specified by RFC 8981 Section 3.3.2.1. This is
    /// used to track the number of retries that occurred prior to picking this
    /// address.
    pub dad_counter: u8,
}

/// A lifetime that may be forever/infinite.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Lifetime<I> {
    /// A finite lifetime.
    Finite(I),
    /// An infinite lifetime.
    Infinite,
}

impl<I: Instant> InspectableValue for Lifetime<I> {
    fn record<N: Inspector>(&self, name: &str, inspector: &mut N) {
        match self {
            Self::Finite(instant) => inspector.record_inspectable_value(name, instant),
            Self::Infinite => inspector.record_str(name, "infinite"),
        }
    }
}

impl<I: Instant> Lifetime<I> {
    /// Creates a new `Lifetime` `duration` from `now`, saturating to
    /// `Infinite`.
    pub fn from_ndp(now: I, duration: NonZeroNdpLifetime) -> Self {
        match duration {
            NonZeroNdpLifetime::Finite(d) => Self::after(now, d.get()),
            NonZeroNdpLifetime::Infinite => Self::Infinite,
        }
    }

    /// Creates a new `Lifetime` `duration` from `now`, saturating to
    /// `Infinite`.
    pub fn after(now: I, duration: Duration) -> Self {
        match now.checked_add(duration) {
            Some(i) => Self::Finite(i),
            None => Self::Infinite,
        }
    }
}

impl<Instant> Lifetime<Instant> {
    /// Maps the instant value in this `Lifetime` with `f`.
    pub fn map_instant<N, F: FnOnce(Instant) -> N>(self, f: F) -> Lifetime<N> {
        match self {
            Self::Infinite => Lifetime::Infinite,
            Self::Finite(i) => Lifetime::Finite(f(i)),
        }
    }
}

/// An address' preferred lifetime information.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PreferredLifetime<Instant> {
    /// Address is preferred. It can be used for new connections.
    ///
    /// `Lifetime` indicates until when the address is expected to remain in
    /// preferred state.
    Preferred(Lifetime<Instant>),
    /// Address is deprecated, it should not be used for new connections.
    Deprecated,
}

impl<Instant> PreferredLifetime<Instant> {
    /// Creates a new `PreferredLifetime` that is preferred until `instant`.
    pub const fn preferred_until(instant: Instant) -> Self {
        Self::Preferred(Lifetime::Finite(instant))
    }

    /// Creates a new `PreferredLifetime` that is preferred with an infinite
    /// lifetime.
    pub const fn preferred_forever() -> Self {
        Self::Preferred(Lifetime::Infinite)
    }

    /// Returns true if the address is deprecated.
    pub const fn is_deprecated(&self) -> bool {
        match self {
            Self::Preferred(_) => false,
            Self::Deprecated => true,
        }
    }

    /// Maps the instant value in this `PreferredLifetime` with `f`.
    pub fn map_instant<N, F: FnOnce(Instant) -> N>(self, f: F) -> PreferredLifetime<N> {
        match self {
            Self::Deprecated => PreferredLifetime::Deprecated,
            Self::Preferred(l) => PreferredLifetime::Preferred(l.map_instant(f)),
        }
    }
}

impl<I: Instant> PreferredLifetime<I> {
    /// Creates a new `PreferredLifetime` that is preferred for `duration` after
    /// `now`.
    pub fn preferred_for(now: I, duration: NonZeroNdpLifetime) -> Self {
        Self::Preferred(Lifetime::from_ndp(now, duration))
    }

    /// Creates a new `PreferredLifetime` that is preferred for `duration` after
    /// `now` if it is `Some`, `deprecated` otherwise.
    pub fn maybe_preferred_for(now: I, duration: Option<NonZeroNdpLifetime>) -> Self {
        match duration {
            Some(d) => Self::preferred_for(now, d),
            None => Self::Deprecated,
        }
    }
}

impl<Instant> Default for PreferredLifetime<Instant> {
    fn default() -> Self {
        Self::Preferred(Lifetime::Infinite)
    }
}

impl<I: Instant> InspectableValue for PreferredLifetime<I> {
    fn record<N: Inspector>(&self, name: &str, inspector: &mut N) {
        match self {
            Self::Deprecated => inspector.record_str(name, "deprecated"),
            Self::Preferred(lifetime) => inspector.record_inspectable_value(name, lifetime),
        }
    }
}

/// Address properties common to IPv4 and IPv6.
///
/// These properties may change throughout the lifetime of the address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CommonAddressProperties<Instant> {
    /// The lifetime for which the address is valid.
    pub valid_until: Lifetime<Instant>,
    /// The address' preferred lifetime.
    ///
    /// Address preferred status is used in IPv6 source address selection. IPv4
    /// addresses simply keep this information on behalf of bindings.
    ///
    /// Note that core does not deprecate manual addresses automatically. Manual
    /// addresses are fully handled outside of core.
    pub preferred_lifetime: PreferredLifetime<Instant>,
}

impl<I> Default for CommonAddressProperties<I> {
    fn default() -> Self {
        Self {
            valid_until: Lifetime::Infinite,
            preferred_lifetime: PreferredLifetime::preferred_forever(),
        }
    }
}

impl<Inst: Instant> Inspectable for CommonAddressProperties<Inst> {
    fn record<I: Inspector>(&self, inspector: &mut I) {
        let Self { valid_until, preferred_lifetime } = self;
        inspector.record_inspectable_value("ValidUntil", valid_until);
        inspector.record_inspectable_value("PreferredLifetime", preferred_lifetime);
    }
}

/// Address configuration common to IPv4 and IPv6.
///
/// Unlike [`CommonAddressProperties`], the fields here are not expected to
/// change throughout the lifetime of the address.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct CommonAddressConfig {
    /// Whether the address was installed with a specific preference towards
    /// performing Duplicate Address Detection (DAD).
    pub should_perform_dad: Option<bool>,
}

impl Inspectable for CommonAddressConfig {
    fn record<I: Inspector>(&self, inspector: &mut I) {
        let Self { should_perform_dad } = self;
        if let Some(should_perform_dad) = should_perform_dad {
            inspector.record_bool("DadOverride", *should_perform_dad);
        }
    }
}

/// The configuration for an IPv4 address.
#[derive(Clone, Copy, Debug, Derivative, PartialEq, Eq, Hash)]
#[derivative(Default(bound = ""))]
pub struct Ipv4AddrConfig<Instant> {
    /// IP version agnostic config.
    pub config: CommonAddressConfig,
    /// IP version agnostic properties.
    pub properties: CommonAddressProperties<Instant>,
}

impl<Inst: Instant> Inspectable for Ipv4AddrConfig<Inst> {
    fn record<I: Inspector>(&self, inspector: &mut I) {
        let Self { config, properties } = self;
        inspector.delegate_inspectable(config);
        inspector.delegate_inspectable(properties);
    }
}

/// Configuration for an IPv6 address assigned via SLAAC that varies based on
/// whether the address is stable or temporary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SlaacConfig<Instant> {
    /// The address is a stable address.
    Stable {
        /// The lifetime of the address.
        valid_until: Lifetime<Instant>,
        /// The time at which the address was created.
        creation_time: Instant,
        /// The number of times the address has been regenerated to avoid either an
        /// IANA-reserved IID or an address already assigned to the same interface.
        regen_counter: u8,
        /// The number of times the address has been regenerated due to DAD failure.
        dad_counter: u8,
    },
    /// The address is a temporary address, as specified by [RFC 8981].
    ///
    /// [RFC 8981]: https://tools.ietf.org/html/rfc8981
    Temporary(TemporarySlaacConfig<Instant>),
}

impl<Instant: Copy> SlaacConfig<Instant> {
    /// The lifetime for which the address is valid.
    pub fn valid_until(&self) -> Lifetime<Instant> {
        match self {
            SlaacConfig::Stable { valid_until, .. } => *valid_until,
            SlaacConfig::Temporary(TemporarySlaacConfig { valid_until, .. }) => {
                Lifetime::Finite(*valid_until)
            }
        }
    }
}

/// The configuration for an IPv6 address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ipv6AddrConfig<Instant> {
    /// Configured by stateless address autoconfiguration.
    Slaac(Ipv6AddrSlaacConfig<Instant>),

    /// Manually configured.
    Manual(Ipv6AddrManualConfig<Instant>),
}

impl<Instant> Default for Ipv6AddrConfig<Instant> {
    fn default() -> Self {
        Self::Manual(Default::default())
    }
}

impl<Inst: Instant> Inspectable for Ipv6AddrConfig<Inst> {
    fn record<I: Inspector>(&self, inspector: &mut I) {
        match self {
            // NB: Ignored fields are recorded after the match.
            Ipv6AddrConfig::Manual(Ipv6AddrManualConfig {
                config,
                properties: _,
                temporary: _,
            }) => {
                inspector.delegate_inspectable(config);
            }
            Ipv6AddrConfig::Slaac(Ipv6AddrSlaacConfig { inner, preferred_lifetime: _ }) => {
                match inner {
                    SlaacConfig::Stable {
                        valid_until: _,
                        creation_time,
                        regen_counter,
                        dad_counter,
                    } => {
                        inspector.record_inspectable_value("CreationTime", creation_time);
                        inspector.record_uint("RegenCounter", *regen_counter);
                        inspector.record_uint("DadCounter", *dad_counter);
                    }
                    SlaacConfig::Temporary(TemporarySlaacConfig {
                        valid_until: _,
                        desync_factor,
                        creation_time,
                        dad_counter,
                    }) => {
                        inspector.record_double("DesyncFactorSecs", desync_factor.as_secs_f64());
                        inspector.record_uint("DadCounter", *dad_counter);
                        inspector.record_inspectable_value("CreationTime", creation_time);
                    }
                }
            }
        };
        inspector.record_bool("IsSlaac", self.is_slaac());
        inspector.record_inspectable_value("ValidUntil", &self.valid_until());
        inspector.record_inspectable_value("PreferredLifetime", &self.preferred_lifetime());
        inspector.record_bool("Temporary", self.is_temporary());
    }
}

/// The common configuration for a SLAAC-assigned IPv6 address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Ipv6AddrSlaacConfig<Instant> {
    /// The inner slaac config, tracking stable and temporary behavior.
    pub inner: SlaacConfig<Instant>,
    /// The address' preferred lifetime.
    pub preferred_lifetime: PreferredLifetime<Instant>,
}

/// The configuration for a manually-assigned IPv6 address.
#[derive(Clone, Copy, Debug, Derivative, PartialEq, Eq, Hash)]
#[derivative(Default(bound = ""))]
pub struct Ipv6AddrManualConfig<Instant> {
    /// IP version agnostic config.
    pub config: CommonAddressConfig,
    /// IP version agnostic properties.
    pub properties: CommonAddressProperties<Instant>,
    /// True if the address is temporary.
    ///
    /// Used in source address selection.
    pub temporary: bool,
}

impl<Instant> From<Ipv6AddrManualConfig<Instant>> for Ipv6AddrConfig<Instant> {
    fn from(value: Ipv6AddrManualConfig<Instant>) -> Self {
        Self::Manual(value)
    }
}

impl<Instant: Copy + PartialEq> Ipv6AddrConfig<Instant> {
    /// The lifetime for which the address is valid.
    pub fn valid_until(&self) -> Lifetime<Instant> {
        match self {
            Ipv6AddrConfig::Slaac(Ipv6AddrSlaacConfig { inner, .. }) => inner.valid_until(),
            Ipv6AddrConfig::Manual(Ipv6AddrManualConfig { properties, .. }) => {
                properties.valid_until
            }
        }
    }

    /// Returns the preferred lifetime for this address.
    pub fn preferred_lifetime(&self) -> PreferredLifetime<Instant> {
        match self {
            Ipv6AddrConfig::Manual(Ipv6AddrManualConfig { properties, .. }) => {
                properties.preferred_lifetime
            }
            Ipv6AddrConfig::Slaac(Ipv6AddrSlaacConfig { preferred_lifetime, .. }) => {
                *preferred_lifetime
            }
        }
    }

    /// Returns true if the address is deprecated.
    pub fn is_deprecated(&self) -> bool {
        self.preferred_lifetime().is_deprecated()
    }

    /// Returns true if the address is temporary.
    pub fn is_temporary(&self) -> bool {
        match self {
            Ipv6AddrConfig::Slaac(Ipv6AddrSlaacConfig { inner, .. }) => match inner {
                SlaacConfig::Stable { .. } => false,
                SlaacConfig::Temporary(_) => true,
            },
            Ipv6AddrConfig::Manual(Ipv6AddrManualConfig { temporary, .. }) => *temporary,
        }
    }

    /// Returns true if the address was configured via SLAAC.
    pub fn is_slaac(&self) -> bool {
        match self {
            Ipv6AddrConfig::Slaac(_) => true,
            Ipv6AddrConfig::Manual(_) => false,
        }
    }
}

/// Flags associated with an IP device address.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct IpAddressFlags {
    /// True if the address is assigned.
    pub assigned: bool,
}

/// The state associated with an IP device address.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct IpAddressData<I: IpDeviceStateIpExt, Inst: Instant> {
    /// The address' flags.
    pub flags: IpAddressFlags,
    /// The address' configuration. `None` when the address is being removed.
    pub config: Option<I::AddressConfig<Inst>>,
}

impl<I: IpDeviceStateIpExt, Inst: Instant> IpAddressData<I, Inst> {
    /// Return whether DAD should be performed for this address.
    pub fn should_perform_dad(&self) -> bool {
        let Some(config) = self.config.as_ref() else {
            // When config is `None` the address is being removed. Insertion
            // must be racing with removal; don't perform DAD.
            return false;
        };

        #[derive(GenericOverIp)]
        #[generic_over_ip(I, Ip)]
        struct Wrap<'a, I: IpDeviceStateIpExt, Inst: Instant>(&'a I::AddressConfig<Inst>);
        let should_perform_dad = I::map_ip_in(
            Wrap(config),
            |Wrap(Ipv4AddrConfig { config, properties: _ })| config.should_perform_dad,
            |Wrap(config)| match config {
                Ipv6AddrConfig::Slaac(_) => None,
                Ipv6AddrConfig::Manual(c) => c.config.should_perform_dad,
            },
        );

        should_perform_dad.unwrap_or(I::DEFAULT_DAD_ENABLED)
    }
}

impl<I: IpDeviceStateIpExt, Inst: Instant> Inspectable for IpAddressData<I, Inst> {
    fn record<II: Inspector>(&self, inspector: &mut II) {
        let Self { flags: IpAddressFlags { assigned }, config } = self;
        inspector.record_bool("Assigned", *assigned);
        if let Some(config) = config {
            inspector.delegate_inspectable(config);
        }
    }
}

/// Data associated with an IPv6 address on an interface.
// TODO(https://fxbug.dev/42173351): Should this be generalized for loopback?
#[derive(Derivative)]
#[derivative(Debug(bound = ""))]
pub struct IpAddressEntry<I: IpDeviceStateIpExt, BT: IpDeviceStateBindingsTypes> {
    addr_sub: AddrSubnet<I::Addr, I::AssignedWitness>,
    dad_state: Mutex<DadState<I, BT>>,
    state: RwLock<IpAddressData<I, BT::Instant>>,
}

impl<I: IpDeviceStateIpExt, BT: IpDeviceStateBindingsTypes> IpAddressEntry<I, BT> {
    /// Constructs a new `IpAddressEntry`.
    pub fn new(
        addr_sub: AddrSubnet<I::Addr, I::AssignedWitness>,
        dad_state: DadState<I, BT>,
        config: I::AddressConfig<BT::Instant>,
    ) -> Self {
        let assigned = dad_state.is_assigned();
        Self {
            addr_sub,
            dad_state: Mutex::new(dad_state),
            state: RwLock::new(IpAddressData {
                config: Some(config),
                flags: IpAddressFlags { assigned },
            }),
        }
    }

    /// This entry's address and subnet.
    pub fn addr_sub(&self) -> &AddrSubnet<I::Addr, I::AssignedWitness> {
        &self.addr_sub
    }
}

impl<I: IpDeviceStateIpExt, BT: IpDeviceStateBindingsTypes> OrderedLockAccess<DadState<I, BT>>
    for IpAddressEntry<I, BT>
{
    type Lock = Mutex<DadState<I, BT>>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.dad_state)
    }
}

impl<I: IpDeviceStateIpExt, BT: IpDeviceStateBindingsTypes>
    OrderedLockAccess<IpAddressData<I, BT::Instant>> for IpAddressEntry<I, BT>
{
    type Lock = RwLock<IpAddressData<I, BT::Instant>>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use net_types::ip::{Ipv4Addr, Ipv6Addr};
    use netstack3_base::InstantContext as _;
    use netstack3_base::testutil::{FakeBindingsCtx, FakeInstant};
    use test_case::test_case;

    type FakeBindingsCtxImpl = FakeBindingsCtx<(), (), (), ()>;

    #[test_case(Lifetime::Infinite ; "with infinite valid_until")]
    #[test_case(Lifetime::Finite(FakeInstant::from(Duration::from_secs(1))); "with finite valid_until")]
    fn test_add_addr_ipv4(valid_until: Lifetime<FakeInstant>) {
        const ADDRESS: Ipv4Addr = Ipv4Addr::new([1, 2, 3, 4]);
        const PREFIX_LEN: u8 = 8;

        let mut ipv4 = IpDeviceAddresses::<Ipv4, FakeBindingsCtxImpl>::default();
        let config = Ipv4AddrConfig {
            properties: CommonAddressProperties { valid_until, ..Default::default() },
            ..Default::default()
        };

        let _: AddressId<_, _> = ipv4
            .add(IpAddressEntry::new(
                AddrSubnet::new(ADDRESS, PREFIX_LEN).unwrap(),
                DadState::Uninitialized,
                config,
            ))
            .unwrap();
        // Adding the same address with different prefix should fail.
        assert_eq!(
            ipv4.add(IpAddressEntry::new(
                AddrSubnet::new(ADDRESS, PREFIX_LEN + 1).unwrap(),
                DadState::Uninitialized,
                config,
            ))
            .unwrap_err(),
            ExistsError
        );
    }

    #[test_case(Lifetime::Infinite ; "with infinite valid_until")]
    #[test_case(Lifetime::Finite(FakeInstant::from(Duration::from_secs(1))); "with finite valid_until")]
    fn test_add_addr_ipv6(valid_until: Lifetime<FakeInstant>) {
        const ADDRESS: Ipv6Addr =
            Ipv6Addr::from_bytes([1, 2, 3, 4, 5, 6, 7, 8, 9, 0, 1, 2, 3, 4, 5, 6]);
        const PREFIX_LEN: u8 = 8;

        let mut ipv6 = IpDeviceAddresses::<Ipv6, FakeBindingsCtxImpl>::default();

        let mut bindings_ctx = FakeBindingsCtxImpl::default();

        let _: AddressId<_, _> = ipv6
            .add(IpAddressEntry::new(
                AddrSubnet::new(ADDRESS, PREFIX_LEN).unwrap(),
                DadState::Tentative {
                    dad_transmits_remaining: None,
                    timer: bindings_ctx.new_timer(()),
                    ip_specific_state: Default::default(),
                    probe_wait: None,
                },
                Ipv6AddrConfig::Slaac(Ipv6AddrSlaacConfig {
                    inner: SlaacConfig::Stable {
                        valid_until,
                        creation_time: bindings_ctx.now(),
                        regen_counter: 0,
                        dad_counter: 0,
                    },
                    preferred_lifetime: PreferredLifetime::Preferred(Lifetime::Infinite),
                }),
            ))
            .unwrap();
        // Adding the same address with different prefix and configuration
        // should fail.
        assert_eq!(
            ipv6.add(IpAddressEntry::new(
                AddrSubnet::new(ADDRESS, PREFIX_LEN + 1).unwrap(),
                DadState::Assigned { ran_dad: false },
                Ipv6AddrConfig::Manual(Ipv6AddrManualConfig {
                    properties: CommonAddressProperties { valid_until, ..Default::default() },
                    ..Default::default()
                }),
            ))
            .unwrap_err(),
            ExistsError,
        );
    }

    #[test_case(None => false; "default")]
    #[test_case(Some(false) => false; "disabled")]
    #[test_case(Some(true) => true; "enabled")]
    fn should_perform_dad_ipv4(setting: Option<bool>) -> bool {
        let state = IpAddressData::<Ipv4, FakeInstant> {
            flags: IpAddressFlags { assigned: false },
            config: Some(Ipv4AddrConfig::<FakeInstant> {
                config: CommonAddressConfig { should_perform_dad: setting },
                ..Default::default()
            }),
        };
        state.should_perform_dad()
    }

    #[test_case(None => true; "default")]
    #[test_case(Some(false) => false; "disabled")]
    #[test_case(Some(true) => true; "enabled")]
    fn should_perform_dad_ipv6(setting: Option<bool>) -> bool {
        let state = IpAddressData::<Ipv6, FakeInstant> {
            flags: IpAddressFlags { assigned: false },
            config: Some(Ipv6AddrConfig::Manual(Ipv6AddrManualConfig::<FakeInstant> {
                config: CommonAddressConfig { should_perform_dad: setting },
                ..Default::default()
            })),
        };
        state.should_perform_dad()
    }
}
