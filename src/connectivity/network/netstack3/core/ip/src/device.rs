// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! An IP device.

pub(crate) mod api;
pub(crate) mod config;
pub(crate) mod dad;
pub(crate) mod nud;
pub(crate) mod opaque_iid;
pub(crate) mod route_discovery;
pub(crate) mod router_solicitation;
pub(crate) mod slaac;
pub(crate) mod state;

use alloc::vec::Vec;
use core::fmt::{Debug, Display};
use core::hash::Hash;
use core::num::NonZeroU8;

use derivative::Derivative;
use log::{debug, info};
use net_types::ip::{
    AddrSubnet, GenericOverIp, Ip, IpAddress, Ipv4, Ipv4Addr, Ipv4SourceAddr, Ipv6, Ipv6Addr,
    Ipv6SourceAddr, Mtu, Subnet,
};
use net_types::{LinkLocalAddress as _, MulticastAddr, SpecifiedAddr, Witness};
use netstack3_base::{
    AnyDevice, AssignedAddrIpExt, Counter, CounterCollectionSpec, DeferredResourceRemovalContext,
    DeviceIdContext, EventContext, ExistsError, HandleableTimer, Instant, InstantBindingsTypes,
    InstantContext, IpAddressId, IpDeviceAddr, IpDeviceAddressIdContext, IpExt, Ipv4DeviceAddr,
    Ipv6DeviceAddr, NetworkSerializer, NotFoundError, RemoveResourceResultWithContext,
    ResourceCounterContext, RngContext, SendFrameError, StrongDeviceIdentifier, TimerContext,
    TimerHandler, TxMetadataBindingsTypes, WeakDeviceIdentifier, WeakIpAddressId,
};
use netstack3_filter::ProofOfEgressCheck;
use packet::BufferMut;
use packet_formats::icmp::mld::MldPacket;
use packet_formats::icmp::ndp::NonZeroNdpLifetime;
use packet_formats::utils::NonZeroDuration;
use zerocopy::SplitByteSlice;

use crate::device::CommonAddressProperties;
use crate::internal::base::{DeviceIpLayerMetadata, IpDeviceMtuContext, IpPacketDestination};
use crate::internal::counters::IpCounters;
use crate::internal::device::config::{
    IpDeviceConfigurationUpdate, Ipv4DeviceConfigurationUpdate, Ipv6DeviceConfigurationUpdate,
};
use crate::internal::device::dad::{
    DadHandler, DadIncomingPacketResult, DadIpExt, DadTimerId, Ipv4DadAddressInfo,
    Ipv6PacketResultMetadata, NeedsDad,
};
use crate::internal::device::nud::NudIpHandler;
use crate::internal::device::route_discovery::{
    Ipv6DiscoveredRoute, Ipv6DiscoveredRouteProperties, Ipv6DiscoveredRouteTimerId,
    RouteDiscoveryHandler,
};
use crate::internal::device::router_solicitation::{RsHandler, RsTimerId};
use crate::internal::device::slaac::{SlaacHandler, SlaacTimerId};
use crate::internal::device::state::{
    IpAddressData, IpAddressFlags, IpDeviceConfiguration, IpDeviceFlags, IpDeviceState,
    IpDeviceStateBindingsTypes, IpDeviceStateIpExt, Ipv4AddrConfig, Ipv4DeviceConfiguration,
    Ipv4DeviceState, Ipv6AddrConfig, Ipv6AddrManualConfig, Ipv6DeviceConfiguration,
    Ipv6DeviceState, Ipv6NetworkLearnedParameters, Lifetime, PreferredLifetime, WeakAddressId,
};
use crate::internal::gmp::igmp::{IgmpPacketHandler, IgmpTimerId};
use crate::internal::gmp::mld::{MldPacketHandler, MldTimerId};
use crate::internal::gmp::{self, GmpHandler, GroupJoinResult, GroupLeaveResult};
use crate::internal::local_delivery::{IpHeaderInfo, LocalDeliveryPacketInfo};

/// An IP device timer.
///
/// This timer is an indirection to the real types defined by the
/// [`IpDeviceIpExt`] trait. Having a concrete type parameterized over IP allows
/// us to provide implementations generic on I for outer timer contexts that
/// handle `IpDeviceTimerId` timers.
#[derive(Derivative, GenericOverIp)]
#[derivative(
    Clone(bound = ""),
    Eq(bound = ""),
    PartialEq(bound = ""),
    Hash(bound = ""),
    Debug(bound = "")
)]
#[generic_over_ip(I, Ip)]
pub struct IpDeviceTimerId<
    I: IpDeviceIpExt,
    D: WeakDeviceIdentifier,
    BT: IpDeviceStateBindingsTypes,
>(I::Timer<D, BT>);

/// A timer ID for IPv4 devices.
#[derive(Derivative)]
#[derivative(
    Clone(bound = ""),
    Debug(bound = ""),
    Eq(bound = ""),
    Hash(bound = ""),
    PartialEq(bound = "")
)]
pub enum Ipv4DeviceTimerId<D: WeakDeviceIdentifier, BT: IpDeviceStateBindingsTypes> {
    /// The timer ID is specific to the IGMP protocol suite.
    Igmp(IgmpTimerId<D>),
    /// The timer ID is specific to duplicate address detection.
    Dad(DadTimerId<Ipv4, D, WeakAddressId<Ipv4, BT>>),
}

impl<D: WeakDeviceIdentifier, BT: IpDeviceStateBindingsTypes> Ipv4DeviceTimerId<D, BT> {
    /// Gets the device ID from this timer IFF the device hasn't been destroyed.
    fn device_id(&self) -> Option<D::Strong> {
        match self {
            Ipv4DeviceTimerId::Igmp(igmp) => igmp.device_id().upgrade(),
            Ipv4DeviceTimerId::Dad(dad) => dad.device_id().upgrade(),
        }
    }

    /// Transforms this timer ID into the common [`IpDeviceTimerId`] version.
    pub fn into_common(self) -> IpDeviceTimerId<Ipv4, D, BT> {
        self.into()
    }
}

impl<D: WeakDeviceIdentifier, BT: IpDeviceStateBindingsTypes> From<IpDeviceTimerId<Ipv4, D, BT>>
    for Ipv4DeviceTimerId<D, BT>
{
    fn from(IpDeviceTimerId(inner): IpDeviceTimerId<Ipv4, D, BT>) -> Self {
        inner
    }
}

impl<D: WeakDeviceIdentifier, BT: IpDeviceStateBindingsTypes> From<Ipv4DeviceTimerId<D, BT>>
    for IpDeviceTimerId<Ipv4, D, BT>
{
    fn from(value: Ipv4DeviceTimerId<D, BT>) -> Self {
        Self(value)
    }
}

impl<D: WeakDeviceIdentifier, BT: IpDeviceStateBindingsTypes> From<IgmpTimerId<D>>
    for Ipv4DeviceTimerId<D, BT>
{
    fn from(id: IgmpTimerId<D>) -> Ipv4DeviceTimerId<D, BT> {
        Ipv4DeviceTimerId::Igmp(id)
    }
}

impl<D: WeakDeviceIdentifier, BT: IpDeviceStateBindingsTypes>
    From<DadTimerId<Ipv4, D, WeakAddressId<Ipv4, BT>>> for Ipv4DeviceTimerId<D, BT>
{
    fn from(id: DadTimerId<Ipv4, D, WeakAddressId<Ipv4, BT>>) -> Ipv4DeviceTimerId<D, BT> {
        Ipv4DeviceTimerId::Dad(id)
    }
}

impl<
    D: WeakDeviceIdentifier,
    BC: IpDeviceStateBindingsTypes,
    CC: TimerHandler<BC, IgmpTimerId<D>>
        + TimerHandler<BC, DadTimerId<Ipv4, D, WeakAddressId<Ipv4, BC>>>,
> HandleableTimer<CC, BC> for Ipv4DeviceTimerId<D, BC>
{
    fn handle(self, core_ctx: &mut CC, bindings_ctx: &mut BC, timer: BC::UniqueTimerId) {
        match self {
            Ipv4DeviceTimerId::Igmp(id) => core_ctx.handle_timer(bindings_ctx, id, timer),
            Ipv4DeviceTimerId::Dad(id) => core_ctx.handle_timer(bindings_ctx, id, timer),
        }
    }
}

impl<I, CC, BC> HandleableTimer<CC, BC> for IpDeviceTimerId<I, CC::WeakDeviceId, BC>
where
    I: IpDeviceIpExt,
    BC: IpDeviceBindingsContext<I, CC::DeviceId>,
    CC: IpDeviceConfigurationContext<I, BC>,
    for<'a> CC::WithIpDeviceConfigurationInnerCtx<'a>:
        TimerHandler<BC, I::Timer<CC::WeakDeviceId, BC>>,
{
    fn handle(self, core_ctx: &mut CC, bindings_ctx: &mut BC, timer: BC::UniqueTimerId) {
        let Self(id) = self;
        let Some(device_id) = I::timer_device_id(&id) else {
            return;
        };
        core_ctx.with_ip_device_configuration(&device_id, |_state, mut core_ctx| {
            TimerHandler::handle_timer(&mut core_ctx, bindings_ctx, id, timer)
        })
    }
}

/// A timer ID for IPv6 devices.
#[derive(Derivative)]
#[derivative(
    Clone(bound = ""),
    Debug(bound = ""),
    Eq(bound = ""),
    Hash(bound = ""),
    PartialEq(bound = "")
)]
#[allow(missing_docs)]
pub enum Ipv6DeviceTimerId<D: WeakDeviceIdentifier, BT: IpDeviceStateBindingsTypes> {
    Mld(MldTimerId<D>),
    Dad(DadTimerId<Ipv6, D, WeakAddressId<Ipv6, BT>>),
    Rs(RsTimerId<D>),
    RouteDiscovery(Ipv6DiscoveredRouteTimerId<D>),
    Slaac(SlaacTimerId<D>),
}

impl<D: WeakDeviceIdentifier, BT: IpDeviceStateBindingsTypes> From<IpDeviceTimerId<Ipv6, D, BT>>
    for Ipv6DeviceTimerId<D, BT>
{
    fn from(IpDeviceTimerId(inner): IpDeviceTimerId<Ipv6, D, BT>) -> Self {
        inner
    }
}

impl<D: WeakDeviceIdentifier, BT: IpDeviceStateBindingsTypes> From<Ipv6DeviceTimerId<D, BT>>
    for IpDeviceTimerId<Ipv6, D, BT>
{
    fn from(value: Ipv6DeviceTimerId<D, BT>) -> Self {
        Self(value)
    }
}

impl<D: WeakDeviceIdentifier, BT: IpDeviceStateBindingsTypes> Ipv6DeviceTimerId<D, BT> {
    /// Gets the device ID from this timer IFF the device hasn't been destroyed.
    fn device_id(&self) -> Option<D::Strong> {
        match self {
            Self::Mld(id) => id.device_id(),
            Self::Dad(id) => id.device_id(),
            Self::Rs(id) => id.device_id(),
            Self::RouteDiscovery(id) => id.device_id(),
            Self::Slaac(id) => id.device_id(),
        }
        .upgrade()
    }

    /// Transforms this timer ID into the common [`IpDeviceTimerId`] version.
    pub fn into_common(self) -> IpDeviceTimerId<Ipv6, D, BT> {
        self.into()
    }
}

impl<D: WeakDeviceIdentifier, BT: IpDeviceStateBindingsTypes> From<MldTimerId<D>>
    for Ipv6DeviceTimerId<D, BT>
{
    fn from(id: MldTimerId<D>) -> Ipv6DeviceTimerId<D, BT> {
        Ipv6DeviceTimerId::Mld(id)
    }
}

impl<D: WeakDeviceIdentifier, BT: IpDeviceStateBindingsTypes>
    From<DadTimerId<Ipv6, D, WeakAddressId<Ipv6, BT>>> for Ipv6DeviceTimerId<D, BT>
{
    fn from(id: DadTimerId<Ipv6, D, WeakAddressId<Ipv6, BT>>) -> Ipv6DeviceTimerId<D, BT> {
        Ipv6DeviceTimerId::Dad(id)
    }
}

impl<D: WeakDeviceIdentifier, BT: IpDeviceStateBindingsTypes> From<RsTimerId<D>>
    for Ipv6DeviceTimerId<D, BT>
{
    fn from(id: RsTimerId<D>) -> Ipv6DeviceTimerId<D, BT> {
        Ipv6DeviceTimerId::Rs(id)
    }
}

impl<D: WeakDeviceIdentifier, BT: IpDeviceStateBindingsTypes> From<Ipv6DiscoveredRouteTimerId<D>>
    for Ipv6DeviceTimerId<D, BT>
{
    fn from(id: Ipv6DiscoveredRouteTimerId<D>) -> Ipv6DeviceTimerId<D, BT> {
        Ipv6DeviceTimerId::RouteDiscovery(id)
    }
}

impl<D: WeakDeviceIdentifier, BT: IpDeviceStateBindingsTypes> From<SlaacTimerId<D>>
    for Ipv6DeviceTimerId<D, BT>
{
    fn from(id: SlaacTimerId<D>) -> Ipv6DeviceTimerId<D, BT> {
        Ipv6DeviceTimerId::Slaac(id)
    }
}

impl<
    D: WeakDeviceIdentifier,
    BC: IpDeviceStateBindingsTypes,
    CC: TimerHandler<BC, RsTimerId<D>>
        + TimerHandler<BC, Ipv6DiscoveredRouteTimerId<D>>
        + TimerHandler<BC, MldTimerId<D>>
        + TimerHandler<BC, SlaacTimerId<D>>
        + TimerHandler<BC, DadTimerId<Ipv6, D, WeakAddressId<Ipv6, BC>>>,
> HandleableTimer<CC, BC> for Ipv6DeviceTimerId<D, BC>
{
    fn handle(self, core_ctx: &mut CC, bindings_ctx: &mut BC, timer: BC::UniqueTimerId) {
        match self {
            Ipv6DeviceTimerId::Mld(id) => core_ctx.handle_timer(bindings_ctx, id, timer),
            Ipv6DeviceTimerId::Dad(id) => core_ctx.handle_timer(bindings_ctx, id, timer),
            Ipv6DeviceTimerId::Rs(id) => core_ctx.handle_timer(bindings_ctx, id, timer),
            Ipv6DeviceTimerId::RouteDiscovery(id) => core_ctx.handle_timer(bindings_ctx, id, timer),
            Ipv6DeviceTimerId::Slaac(id) => core_ctx.handle_timer(bindings_ctx, id, timer),
        }
    }
}

/// An extension trait adding IP device properties.
pub trait IpDeviceIpExt: IpDeviceStateIpExt + AssignedAddrIpExt + gmp::IpExt + DadIpExt {
    /// IP layer state kept by the device.
    type State<BT: IpDeviceStateBindingsTypes>: AsRef<IpDeviceState<Self, BT>>
        + AsMut<IpDeviceState<Self, BT>>;
    /// IP layer configuration kept by the device.
    type Configuration: AsRef<IpDeviceConfiguration>
        + AsMut<IpDeviceConfiguration>
        + Clone
        + Debug
        + Eq
        + PartialEq;
    /// High level IP device timer.
    type Timer<D: WeakDeviceIdentifier, BT: IpDeviceStateBindingsTypes>: Into<IpDeviceTimerId<Self, D, BT>>
        + From<IpDeviceTimerId<Self, D, BT>>
        + Clone
        + Eq
        + PartialEq
        + Debug
        + Hash;
    /// Manual device address configuration (user-initiated).
    type ManualAddressConfig<I: Instant>: Default + Debug + Into<Self::AddressConfig<I>>;
    /// Device configuration update request.
    type ConfigurationUpdate: From<IpDeviceConfigurationUpdate>
        + AsRef<IpDeviceConfigurationUpdate>
        + Debug;

    /// Gets the common properties of an address from its configuration.
    fn get_common_props<I: Instant>(config: &Self::AddressConfig<I>) -> CommonAddressProperties<I>;

    /// Extracts the device ID from a device timer.
    fn timer_device_id<D: WeakDeviceIdentifier, BT: IpDeviceStateBindingsTypes>(
        timer: &Self::Timer<D, BT>,
    ) -> Option<D::Strong>;
}

impl IpDeviceIpExt for Ipv4 {
    type State<BT: IpDeviceStateBindingsTypes> = Ipv4DeviceState<BT>;
    type Configuration = Ipv4DeviceConfiguration;
    type Timer<D: WeakDeviceIdentifier, BT: IpDeviceStateBindingsTypes> = Ipv4DeviceTimerId<D, BT>;
    type ManualAddressConfig<I: Instant> = Ipv4AddrConfig<I>;
    type ConfigurationUpdate = Ipv4DeviceConfigurationUpdate;

    fn get_common_props<I: Instant>(config: &Self::AddressConfig<I>) -> CommonAddressProperties<I> {
        config.properties
    }

    fn timer_device_id<D: WeakDeviceIdentifier, BT: IpDeviceStateBindingsTypes>(
        timer: &Self::Timer<D, BT>,
    ) -> Option<D::Strong> {
        timer.device_id()
    }
}

impl IpDeviceIpExt for Ipv6 {
    type State<BT: IpDeviceStateBindingsTypes> = Ipv6DeviceState<BT>;
    type Configuration = Ipv6DeviceConfiguration;
    type Timer<D: WeakDeviceIdentifier, BT: IpDeviceStateBindingsTypes> = Ipv6DeviceTimerId<D, BT>;
    type ManualAddressConfig<I: Instant> = Ipv6AddrManualConfig<I>;
    type ConfigurationUpdate = Ipv6DeviceConfigurationUpdate;

    fn get_common_props<I: Instant>(config: &Self::AddressConfig<I>) -> CommonAddressProperties<I> {
        CommonAddressProperties {
            valid_until: config.valid_until(),
            preferred_lifetime: config.preferred_lifetime(),
        }
    }

    fn timer_device_id<D: WeakDeviceIdentifier, BT: IpDeviceStateBindingsTypes>(
        timer: &Self::Timer<D, BT>,
    ) -> Option<D::Strong> {
        timer.device_id()
    }
}
/// IP address assignment states.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum IpAddressState {
    /// The address is unavailable because it's interface is not IP enabled.
    Unavailable,
    /// The address is assigned to an interface and can be considered bound to
    /// it (all packets destined to the address will be accepted).
    Assigned,
    /// The address is considered unassigned to an interface for normal
    /// operations, but has the intention of being assigned in the future (e.g.
    /// once Duplicate Address Detection is completed).
    Tentative,
}

/// The reason an address was removed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AddressRemovedReason {
    /// The address was removed in response to external action.
    Manual,
    /// The address was removed because it was detected as a duplicate via DAD.
    DadFailed,
    /// The address was voluntarily forfeited because a conflict was detected
    /// during ongoing address conflict detection.
    Forfeited,
}

#[derive(Debug, Eq, Hash, PartialEq, GenericOverIp)]
#[generic_over_ip(I, Ip)]
/// Events emitted from IP devices.
pub enum IpDeviceEvent<DeviceId, I: Ip, Instant> {
    /// Address was assigned.
    AddressAdded {
        /// The device.
        device: DeviceId,
        /// The new address.
        addr: AddrSubnet<I::Addr>,
        /// Initial address state.
        state: IpAddressState,
        /// The lifetime for which the address is valid.
        valid_until: Lifetime<Instant>,
        /// The  preferred lifetime information for the address.
        preferred_lifetime: PreferredLifetime<Instant>,
    },
    /// Address was unassigned.
    AddressRemoved {
        /// The device.
        device: DeviceId,
        /// The removed address.
        addr: SpecifiedAddr<I::Addr>,
        /// The reason the address was removed.
        reason: AddressRemovedReason,
    },
    /// Address state changed.
    AddressStateChanged {
        /// The device.
        device: DeviceId,
        /// The address whose state was changed.
        addr: SpecifiedAddr<I::Addr>,
        /// The new address state.
        state: IpAddressState,
    },
    /// Address properties changed.
    AddressPropertiesChanged {
        /// The device.
        device: DeviceId,
        /// The address whose properties were changed.
        addr: SpecifiedAddr<I::Addr>,
        /// The new `valid_until` lifetime.
        valid_until: Lifetime<Instant>,
        /// The new preferred lifetime information.
        preferred_lifetime: PreferredLifetime<Instant>,
    },
    /// IP was enabled/disabled on the device
    EnabledChanged {
        /// The device.
        device: DeviceId,
        /// `true` if IP was enabled on the device; `false` if IP was disabled.
        ip_enabled: bool,
    },
}

impl<DeviceId, I: Ip, Instant> IpDeviceEvent<DeviceId, I, Instant> {
    /// Changes the device id type with `map`.
    pub fn map_device<N, F: FnOnce(DeviceId) -> N>(self, map: F) -> IpDeviceEvent<N, I, Instant> {
        match self {
            IpDeviceEvent::AddressAdded {
                device,
                addr,
                state,
                valid_until,
                preferred_lifetime,
            } => IpDeviceEvent::AddressAdded {
                device: map(device),
                addr,
                state,
                valid_until,
                preferred_lifetime,
            },
            IpDeviceEvent::AddressRemoved { device, addr, reason } => {
                IpDeviceEvent::AddressRemoved { device: map(device), addr, reason }
            }
            IpDeviceEvent::AddressStateChanged { device, addr, state } => {
                IpDeviceEvent::AddressStateChanged { device: map(device), addr, state }
            }
            IpDeviceEvent::EnabledChanged { device, ip_enabled } => {
                IpDeviceEvent::EnabledChanged { device: map(device), ip_enabled }
            }
            IpDeviceEvent::AddressPropertiesChanged {
                device,
                addr,
                valid_until,
                preferred_lifetime,
            } => IpDeviceEvent::AddressPropertiesChanged {
                device: map(device),
                addr,
                valid_until,
                preferred_lifetime,
            },
        }
    }
}

/// The bindings execution context for IP devices.
pub trait IpDeviceBindingsContext<I: IpDeviceIpExt, D: StrongDeviceIdentifier>:
    IpDeviceStateBindingsTypes
    + DeferredResourceRemovalContext
    + TimerContext
    + RngContext
    + EventContext<IpDeviceEvent<D, I, <Self as InstantBindingsTypes>::Instant>>
{
}
impl<
    D: StrongDeviceIdentifier,
    I: IpDeviceIpExt,
    BC: IpDeviceStateBindingsTypes
        + DeferredResourceRemovalContext
        + TimerContext
        + RngContext
        + EventContext<IpDeviceEvent<D, I, <Self as InstantBindingsTypes>::Instant>>,
> IpDeviceBindingsContext<I, D> for BC
{
}

/// The core context providing access to device IP address state.
pub trait IpDeviceAddressContext<I: IpDeviceIpExt, BT: InstantBindingsTypes>:
    IpDeviceAddressIdContext<I>
{
    /// Calls the callback with a reference to the address data `addr_id` on
    /// `device_id`.
    fn with_ip_address_data<O, F: FnOnce(&IpAddressData<I, BT::Instant>) -> O>(
        &mut self,
        device_id: &Self::DeviceId,
        addr_id: &Self::AddressId,
        cb: F,
    ) -> O;

    /// Calls the callback with a mutable reference to the address data
    /// `addr_id` on `device_id`.
    fn with_ip_address_data_mut<O, F: FnOnce(&mut IpAddressData<I, BT::Instant>) -> O>(
        &mut self,
        device_id: &Self::DeviceId,
        addr_id: &Self::AddressId,
        cb: F,
    ) -> O;
}

/// Accessor for IP device state.
pub trait IpDeviceStateContext<I: IpDeviceIpExt, BT: IpDeviceStateBindingsTypes>:
    IpDeviceAddressContext<I, BT>
{
    /// Inner accessor context.
    type IpDeviceAddressCtx<'a>: IpDeviceAddressContext<I, BT, DeviceId = Self::DeviceId, AddressId = Self::AddressId>;

    /// Calls the function with immutable access to the device's flags.
    ///
    /// Note that this trait should only provide immutable access to the flags.
    /// Changes to the IP device flags must only be performed while synchronizing
    /// with the IP device configuration, so mutable access to the flags is through
    /// `WithIpDeviceConfigurationMutInner::with_configuration_and_flags_mut`.
    fn with_ip_device_flags<O, F: FnOnce(&IpDeviceFlags) -> O>(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O;

    /// Removes an address from the device identified by the ID.
    fn remove_ip_address(
        &mut self,
        device_id: &Self::DeviceId,
        addr: Self::AddressId,
    ) -> RemoveResourceResultWithContext<AddrSubnet<I::Addr>, BT>;

    /// Returns the address ID for the given address value.
    fn get_address_id(
        &mut self,
        device_id: &Self::DeviceId,
        addr: SpecifiedAddr<I::Addr>,
    ) -> Result<Self::AddressId, NotFoundError>;

    /// The iterator given to `with_address_ids`.
    type AddressIdsIter<'a>: Iterator<Item = Self::AddressId> + 'a;

    /// Calls the function with an iterator over all the address IDs associated
    /// with the device.
    fn with_address_ids<
        O,
        F: FnOnce(Self::AddressIdsIter<'_>, &mut Self::IpDeviceAddressCtx<'_>) -> O,
    >(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O;

    /// Calls the function with an immutable reference to the device's default
    /// hop limit for this IP version.
    fn with_default_hop_limit<O, F: FnOnce(&NonZeroU8) -> O>(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O;

    /// Calls the function with a mutable reference to the device's default
    /// hop limit for this IP version.
    fn with_default_hop_limit_mut<O, F: FnOnce(&mut NonZeroU8) -> O>(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O;

    /// Joins the link-layer multicast group associated with the given IP
    /// multicast group.
    fn join_link_multicast_group(
        &mut self,
        bindings_ctx: &mut BT,
        device_id: &Self::DeviceId,
        multicast_addr: MulticastAddr<I::Addr>,
    );

    /// Leaves the link-layer multicast group associated with the given IP
    /// multicast group.
    fn leave_link_multicast_group(
        &mut self,
        bindings_ctx: &mut BT,
        device_id: &Self::DeviceId,
        multicast_addr: MulticastAddr<I::Addr>,
    );
}

/// A trait abstracting the ability to add an IP address to a device instance.
///
/// This trait is _only_ accessible to callers in this crate via
/// [`WithIpDeviceConfigurationMutInner`], which guarantees that adding an
/// address can _only_ happen under exclusive device configuration access.
pub trait IpDeviceAddAddressContext<I: IpDeviceIpExt, BT: IpDeviceStateBindingsTypes>:
    IpDeviceAddressContext<I, BT>
{
    /// Adds an IP address for the device.
    fn add_ip_address(
        &mut self,
        device_id: &Self::DeviceId,
        addr: AddrSubnet<I::Addr, I::AssignedWitness>,
        config: I::AddressConfig<BT::Instant>,
    ) -> Result<Self::AddressId, ExistsError>;
}

/// The context provided to the callback passed to
/// [`IpDeviceConfigurationContext::with_ip_device_configuration_mut`].
pub trait WithIpDeviceConfigurationMutInner<I: IpDeviceIpExt, BT: IpDeviceStateBindingsTypes>:
    DeviceIdContext<AnyDevice>
{
    /// The inner device state context.
    type IpDeviceStateCtx<'s>: IpDeviceStateContext<I, BT, DeviceId = Self::DeviceId>
        + IpDeviceAddAddressContext<I, BT>
        + GmpHandler<I, BT>
        + NudIpHandler<I, BT>
        + DadHandler<I, BT>
        + 's
    where
        Self: 's;

    /// Returns an immutable reference to a device's IP configuration and an
    /// `IpDeviceStateCtx`.
    fn ip_device_configuration_and_ctx(
        &mut self,
    ) -> (&I::Configuration, Self::IpDeviceStateCtx<'_>);

    /// Calls the function with a mutable reference to a device's IP
    /// configuration and flags.
    fn with_configuration_and_flags_mut<
        O,
        F: FnOnce(&mut I::Configuration, &mut IpDeviceFlags) -> O,
    >(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O;
}

/// The execution context for IP devices.
pub trait IpDeviceConfigurationContext<
    I: IpDeviceIpExt,
    BC: IpDeviceBindingsContext<I, Self::DeviceId>,
>: IpDeviceStateContext<I, BC> + IpDeviceMtuContext<I> + DeviceIdContext<AnyDevice>
{
    /// The iterator provided by this context.
    type DevicesIter<'s>: Iterator<Item = Self::DeviceId> + 's;
    /// The inner configuration context.
    type WithIpDeviceConfigurationInnerCtx<'s>: IpDeviceStateContext<I, BC, DeviceId = Self::DeviceId, AddressId = Self::AddressId>
        + GmpHandler<I, BC>
        + NudIpHandler<I, BC>
        + DadHandler<I, BC>
        + IpAddressRemovalHandler<I, BC>
        + IpDeviceMtuContext<I>
        + 's;
    /// The inner mutable configuration context.
    type WithIpDeviceConfigurationMutInner<'s>: WithIpDeviceConfigurationMutInner<I, BC, DeviceId = Self::DeviceId>
        + 's;
    /// Provides access to device state.
    type DeviceAddressAndGroupsAccessor<'s>: IpDeviceStateContext<I, BC, DeviceId = Self::DeviceId>
        + 's;

    /// Calls the function with an immutable reference to the IP device
    /// configuration and a `WithIpDeviceConfigurationInnerCtx`.
    fn with_ip_device_configuration<
        O,
        F: FnOnce(&I::Configuration, Self::WithIpDeviceConfigurationInnerCtx<'_>) -> O,
    >(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O;

    /// Calls the function with a `WithIpDeviceConfigurationMutInner`.
    fn with_ip_device_configuration_mut<
        O,
        F: FnOnce(Self::WithIpDeviceConfigurationMutInner<'_>) -> O,
    >(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O;

    /// Calls the function with an [`Iterator`] of IDs for all initialized
    /// devices and an accessor for device state.
    fn with_devices_and_state<
        O,
        F: FnOnce(Self::DevicesIter<'_>, Self::DeviceAddressAndGroupsAccessor<'_>) -> O,
    >(
        &mut self,
        cb: F,
    ) -> O;

    /// Returns the ID of the loopback interface, if one exists on the system
    /// and is initialized.
    fn loopback_id(&mut self) -> Option<Self::DeviceId>;
}

/// The context provided to the callback passed to
/// [`Ipv6DeviceConfigurationContext::with_ipv6_device_configuration_mut`].
pub trait WithIpv6DeviceConfigurationMutInner<BC: IpDeviceBindingsContext<Ipv6, Self::DeviceId>>:
    WithIpDeviceConfigurationMutInner<Ipv6, BC>
{
    /// The inner IPv6 device state context.
    type Ipv6DeviceStateCtx<'s>: Ipv6DeviceContext<BC, DeviceId = Self::DeviceId>
        + GmpHandler<Ipv6, BC>
        + NudIpHandler<Ipv6, BC>
        + DadHandler<Ipv6, BC>
        + RsHandler<BC>
        + SlaacHandler<BC>
        + RouteDiscoveryHandler<BC>
        + 's
    where
        Self: 's;

    /// Returns an immutable reference to a device's IPv6 configuration and an
    /// `Ipv6DeviceStateCtx`.
    fn ipv6_device_configuration_and_ctx(
        &mut self,
    ) -> (&Ipv6DeviceConfiguration, Self::Ipv6DeviceStateCtx<'_>);
}

/// The core context for IPv6 device configuration.
pub trait Ipv6DeviceConfigurationContext<BC: IpDeviceBindingsContext<Ipv6, Self::DeviceId>>:
    IpDeviceConfigurationContext<Ipv6, BC>
{
    /// The context available while holding device configuration.
    type Ipv6DeviceStateCtx<'s>: Ipv6DeviceContext<BC, DeviceId = Self::DeviceId, AddressId = Self::AddressId>
        + GmpHandler<Ipv6, BC>
        + MldPacketHandler<BC, Self::DeviceId>
        + NudIpHandler<Ipv6, BC>
        + DadHandler<Ipv6, BC>
        + RsHandler<BC>
        + SlaacHandler<BC>
        + RouteDiscoveryHandler<BC>
        + 's;
    /// The context available while holding mutable device configuration.
    type WithIpv6DeviceConfigurationMutInner<'s>: WithIpv6DeviceConfigurationMutInner<BC, DeviceId = Self::DeviceId>
        + 's;

    /// Calls the function with an immutable reference to the IPv6 device
    /// configuration and an `Ipv6DeviceStateCtx`.
    fn with_ipv6_device_configuration<
        O,
        F: FnOnce(&Ipv6DeviceConfiguration, Self::Ipv6DeviceStateCtx<'_>) -> O,
    >(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O;

    /// Calls the function with a `WithIpv6DeviceConfigurationMutInner`.
    fn with_ipv6_device_configuration_mut<
        O,
        F: FnOnce(Self::WithIpv6DeviceConfigurationMutInner<'_>) -> O,
    >(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O;
}

/// A link-layer address that can be used to generate IPv6 addresses.
pub trait Ipv6LinkLayerAddr {
    /// Gets the address as a byte slice.
    fn as_bytes(&self) -> &[u8];

    /// Gets the device's EUI-64 based interface identifier.
    fn eui64_iid(&self) -> [u8; 8];
}

/// The execution context for an IPv6 device.
pub trait Ipv6DeviceContext<BC: IpDeviceBindingsContext<Ipv6, Self::DeviceId>>:
    IpDeviceStateContext<Ipv6, BC>
{
    /// A link-layer address.
    type LinkLayerAddr: Ipv6LinkLayerAddr;

    /// Gets the device's link-layer address, if the device supports link-layer
    /// addressing.
    fn get_link_layer_addr(&mut self, device_id: &Self::DeviceId) -> Option<Self::LinkLayerAddr>;

    /// Sets the link MTU for the device.
    fn set_link_mtu(&mut self, device_id: &Self::DeviceId, mtu: Mtu);

    /// Calls the function with an immutable reference to the retransmit timer.
    fn with_network_learned_parameters<O, F: FnOnce(&Ipv6NetworkLearnedParameters) -> O>(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O;

    /// Calls the function with a mutable reference to the retransmit timer.
    fn with_network_learned_parameters_mut<O, F: FnOnce(&mut Ipv6NetworkLearnedParameters) -> O>(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O;
}

/// An implementation of an IP device.
pub trait IpDeviceHandler<I: IpDeviceIpExt, BC>: DeviceIdContext<AnyDevice> {
    /// Returns whether the device is a router.
    fn is_router_device(&mut self, device_id: &Self::DeviceId) -> bool;

    /// Sets the device's default hop limit.
    fn set_default_hop_limit(&mut self, device_id: &Self::DeviceId, hop_limit: NonZeroU8);

    /// Handles a received Duplicate Address Detection Packet.
    ///
    /// Takes action in response to a received DAD packet for the given address.
    /// Returns the assignment state of the address on the given interface, if
    /// there was one before any action was taken. That is, this method returns
    /// `IpAddressState::Tentative` when the address was tentatively assigned
    /// (and now removed), `IpAddressState::Assigned` if the address was
    /// assigned (and possibly removed), `IpAddressState::Unassigned` if the
    /// address was uninitialized, and `None` if the address wasn't associated
    /// with the interface
    ///
    /// For IPv4, a DAD packet is either an ARP request or response. For IPv6 a
    /// DAD packet is either a Neighbor Solicitation or a Neighbor
    /// Advertisement.
    fn handle_received_dad_packet(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        addr: SpecifiedAddr<I::Addr>,
        packet_data: I::ReceivedPacketData<'_>,
    ) -> Option<IpAddressState>;
}

impl<
    I: IpDeviceIpExt,
    BC: IpDeviceBindingsContext<I, CC::DeviceId>,
    CC: IpDeviceConfigurationContext<I, BC> + ResourceCounterContext<CC::DeviceId, IpCounters<I>>,
> IpDeviceHandler<I, BC> for CC
{
    fn is_router_device(&mut self, device_id: &Self::DeviceId) -> bool {
        is_ip_unicast_forwarding_enabled(self, device_id)
    }

    fn set_default_hop_limit(&mut self, device_id: &Self::DeviceId, hop_limit: NonZeroU8) {
        self.with_default_hop_limit_mut(device_id, |default_hop_limit| {
            *default_hop_limit = hop_limit
        })
    }

    fn handle_received_dad_packet(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        addr: SpecifiedAddr<I::Addr>,
        packet_data: I::ReceivedPacketData<'_>,
    ) -> Option<IpAddressState> {
        let addr_id = match self.get_address_id(device_id, addr) {
            Ok(o) => o,
            Err(NotFoundError) => return None,
        };

        let orig_state =
            match self.with_ip_device_configuration(device_id, |_config, mut core_ctx| {
                core_ctx.handle_incoming_packet(bindings_ctx, device_id, &addr_id, packet_data)
            }) {
                DadIncomingPacketResult::Assigned { should_remove } => {
                    if !should_remove {
                        // Return `Assigned` without removing the address.
                        return Some(IpAddressState::Assigned);
                    } else {
                        // Otherwise fall through to address removal.

                        // Note: RFC 5227 section 2.4 states that:
                        //   Before abandoning an address due to a conflict,
                        //   hosts SHOULD actively attempt to reset any existing
                        //   connections using that address.  This mitigates
                        //   some security threats posed by address reconfiguration
                        //
                        // For now we don't reset any TCP connections, but in
                        // the future may decide it's appropriate to do so.
                        IpAddressState::Assigned
                    }
                }
                DadIncomingPacketResult::Tentative { meta } => {
                    #[derive(GenericOverIp)]
                    #[generic_over_ip(I, Ip)]
                    struct Wrapped<I: IpDeviceIpExt>(I::IncomingPacketResultMeta);
                    let is_looped_back = I::map_ip_in(
                        Wrapped(meta),
                        // Note: Looped back ARP probes are handled directly in the
                        // ARP engine.
                        |Wrapped(())| false,
                        // Per RFC 7527 section 4.2:
                        //   If the node has been configured to use the Enhanced DAD algorithm and
                        //   an interface on the node receives any NS(DAD) message where the
                        //   Target Address matches the interface address (in tentative or
                        //   optimistic state), the receiver compares the nonce included in the
                        //   message, with any stored nonce on the receiving interface.  If a
                        //   match is found, the node SHOULD log a system management message,
                        //   SHOULD update any statistics counter, and MUST drop the received
                        //   message.  If the received NS(DAD) message includes a nonce and no
                        //   match is found with any stored nonce, the node SHOULD log a system
                        //   management message for a DAD-failed state and SHOULD update any
                        //   statistics counter.
                        |Wrapped(Ipv6PacketResultMetadata { matched_nonce })| matched_nonce,
                    );

                    if is_looped_back {
                        // Increment a counter (IPv6 only).
                        self.increment_both(device_id, |c| {
                            #[derive(GenericOverIp)]
                            #[generic_over_ip(I, Ip)]
                            struct InCounters<'a, I: IpDeviceIpExt>(
                                &'a <I::RxCounters as CounterCollectionSpec>::CounterCollection<
                                    Counter,
                                >,
                            );
                            I::map_ip_in::<_, _>(
                                InCounters(&c.version_rx),
                                |_counters| {
                                    unreachable!("Looped back ARP probes are dropped in ARP")
                                },
                                |InCounters(counters)| &counters.drop_looped_back_dad_probe,
                            )
                        });

                        // Return `Tentative` without removing the address if the
                        // probe is looped back.
                        return Some(IpAddressState::Tentative);
                    } else {
                        // Otherwise fall through to address removal.
                        IpAddressState::Tentative
                    }
                }
                DadIncomingPacketResult::Uninitialized => IpAddressState::Unavailable,
            };

        // If we're here, we've had a conflicting packet and we should remove the
        // address.
        let removal_reason = match orig_state {
            // Conflicts while assigned are part of ongoing conflict detection
            // and are considered forfeits.
            IpAddressState::Assigned => AddressRemovedReason::Forfeited,
            // Conflicts while tentative or unavailable are considered DAD
            // failures.
            IpAddressState::Tentative | IpAddressState::Unavailable => {
                AddressRemovedReason::DadFailed
            }
        };
        match del_ip_addr(
            self,
            bindings_ctx,
            device_id,
            DelIpAddr::AddressId(addr_id),
            removal_reason,
        ) {
            Ok(result) => {
                bindings_ctx.defer_removal_result(result);
                Some(orig_state)
            }
            Err(NotFoundError) => {
                // We may have raced with user removal of this address.
                None
            }
        }
    }
}

/// Handles receipt of an IGMP packet on `device`.
pub fn receive_igmp_packet<CC, BC, B, H>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device: &CC::DeviceId,
    src_ip: Ipv4SourceAddr,
    dst_ip: SpecifiedAddr<Ipv4Addr>,
    buffer: B,
    info: &LocalDeliveryPacketInfo<Ipv4, H>,
) where
    CC: IpDeviceConfigurationContext<Ipv4, BC>,
    BC: IpDeviceBindingsContext<Ipv4, CC::DeviceId>,
    for<'a> CC::WithIpDeviceConfigurationInnerCtx<'a>: IpDeviceStateContext<Ipv4, BC, DeviceId = CC::DeviceId>
        + IgmpPacketHandler<BC, CC::DeviceId>,
    B: BufferMut,
    H: IpHeaderInfo<Ipv4>,
{
    core_ctx.with_ip_device_configuration(device, |_config, mut core_ctx| {
        IgmpPacketHandler::receive_igmp_packet(
            &mut core_ctx,
            bindings_ctx,
            device,
            src_ip,
            dst_ip,
            buffer,
            info,
        )
    })
}

/// An implementation of an IPv6 device.
pub trait Ipv6DeviceHandler<BC>: IpDeviceHandler<Ipv6, BC> {
    /// A link-layer address.
    type LinkLayerAddr: Ipv6LinkLayerAddr;

    /// Gets the device's link-layer address, if the device supports link-layer
    /// addressing.
    fn get_link_layer_addr(&mut self, device_id: &Self::DeviceId) -> Option<Self::LinkLayerAddr>;

    /// Sets the discovered retransmit timer for the device.
    fn set_discovered_retrans_timer(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        retrans_timer: NonZeroDuration,
    );

    /// Sets the link MTU for the device.
    fn set_link_mtu(&mut self, device_id: &Self::DeviceId, mtu: Mtu);

    /// Updates a discovered IPv6 route.
    fn update_discovered_ipv6_route(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        route: Ipv6DiscoveredRoute,
        properties: Ipv6DiscoveredRouteProperties,
        lifetime: Option<NonZeroNdpLifetime>,
    );

    /// Applies a SLAAC update.
    fn apply_slaac_update(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        prefix: Subnet<Ipv6Addr>,
        preferred_lifetime: Option<NonZeroNdpLifetime>,
        valid_lifetime: Option<NonZeroNdpLifetime>,
    );

    /// Receives an MLD packet for processing.
    fn receive_mld_packet<B: SplitByteSlice, H: IpHeaderInfo<Ipv6>>(
        &mut self,
        bindings_ctx: &mut BC,
        device: &Self::DeviceId,
        src_ip: Ipv6SourceAddr,
        dst_ip: SpecifiedAddr<Ipv6Addr>,
        packet: MldPacket<B>,
        header_info: &H,
    );
}

impl<
    BC: IpDeviceBindingsContext<Ipv6, CC::DeviceId>,
    CC: Ipv6DeviceContext<BC>
        + Ipv6DeviceConfigurationContext<BC>
        + ResourceCounterContext<CC::DeviceId, IpCounters<Ipv6>>,
> Ipv6DeviceHandler<BC> for CC
{
    type LinkLayerAddr = CC::LinkLayerAddr;

    fn get_link_layer_addr(&mut self, device_id: &Self::DeviceId) -> Option<CC::LinkLayerAddr> {
        Ipv6DeviceContext::get_link_layer_addr(self, device_id)
    }

    fn set_discovered_retrans_timer(
        &mut self,
        _bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        retrans_timer: NonZeroDuration,
    ) {
        self.with_network_learned_parameters_mut(device_id, |state| {
            state.retrans_timer = Some(retrans_timer)
        })
    }

    fn set_link_mtu(&mut self, device_id: &Self::DeviceId, mtu: Mtu) {
        Ipv6DeviceContext::set_link_mtu(self, device_id, mtu)
    }

    fn update_discovered_ipv6_route(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        route: Ipv6DiscoveredRoute,
        properties: Ipv6DiscoveredRouteProperties,
        lifetime: Option<NonZeroNdpLifetime>,
    ) {
        self.with_ipv6_device_configuration(device_id, |config, mut core_ctx| {
            RouteDiscoveryHandler::update_route(
                &mut core_ctx,
                bindings_ctx,
                device_id,
                route,
                properties,
                lifetime,
                &config.route_discovery_config,
            )
        })
    }

    fn apply_slaac_update(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        prefix: Subnet<Ipv6Addr>,
        preferred_lifetime: Option<NonZeroNdpLifetime>,
        valid_lifetime: Option<NonZeroNdpLifetime>,
    ) {
        self.with_ipv6_device_configuration(device_id, |_config, mut core_ctx| {
            SlaacHandler::apply_slaac_update(
                &mut core_ctx,
                bindings_ctx,
                device_id,
                prefix,
                preferred_lifetime,
                valid_lifetime,
            )
        })
    }

    fn receive_mld_packet<B: SplitByteSlice, H: IpHeaderInfo<Ipv6>>(
        &mut self,
        bindings_ctx: &mut BC,
        device: &Self::DeviceId,
        src_ip: Ipv6SourceAddr,
        dst_ip: SpecifiedAddr<Ipv6Addr>,
        packet: MldPacket<B>,
        header_info: &H,
    ) {
        self.with_ipv6_device_configuration(device, |_config, mut core_ctx| {
            MldPacketHandler::receive_mld_packet(
                &mut core_ctx,
                bindings_ctx,
                device,
                src_ip,
                dst_ip,
                packet,
                header_info,
            )
        })
    }
}

/// The execution context for an IP device with a buffer.
pub trait IpDeviceSendContext<I: IpExt, BC: TxMetadataBindingsTypes>:
    DeviceIdContext<AnyDevice>
{
    /// Sends an IP packet through the device.
    fn send_ip_frame<S>(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        destination: IpPacketDestination<I, &Self::DeviceId>,
        ip_layer_metadata: DeviceIpLayerMetadata<BC>,
        body: S,
        egress_proof: ProofOfEgressCheck,
    ) -> Result<(), SendFrameError<S>>
    where
        S: NetworkSerializer,
        S::Buffer: BufferMut;
}

fn enable_ipv6_device_with_config<
    BC: IpDeviceBindingsContext<Ipv6, CC::DeviceId>,
    CC: Ipv6DeviceContext<BC>
        + GmpHandler<Ipv6, BC>
        + RsHandler<BC>
        + DadHandler<Ipv6, BC>
        + SlaacHandler<BC>,
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &CC::DeviceId,
    config: &Ipv6DeviceConfiguration,
) {
    // All nodes should join the all-nodes multicast group.
    join_ip_multicast_with_config(
        core_ctx,
        bindings_ctx,
        device_id,
        Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS,
        config,
    );
    GmpHandler::gmp_handle_maybe_enabled(core_ctx, bindings_ctx, device_id);

    // Perform DAD for all addresses when enabling a device.
    //
    // We have to do this for all addresses (including ones that had DAD
    // performed) as while the device was disabled, another node could have
    // assigned the address and we wouldn't have responded to its DAD
    // solicitations.
    core_ctx
        .with_address_ids(device_id, |addrs, _core_ctx| addrs.collect::<Vec<_>>())
        .into_iter()
        .for_each(|addr_id| {
            let needs_dad = DadHandler::initialize_duplicate_address_detection(
                core_ctx,
                bindings_ctx,
                device_id,
                &addr_id,
                |state| IpDeviceEvent::AddressStateChanged {
                    device: device_id.clone(),
                    addr: addr_id.addr().into(),
                    state,
                },
            );
            match needs_dad {
                NeedsDad::Yes(token) => {
                    core_ctx.start_duplicate_address_detection(bindings_ctx, token);
                }
                NeedsDad::No => {}
            }
        });

    // Only generate a link-local address if the device supports link-layer
    // addressing.
    if core_ctx.get_link_layer_addr(device_id).is_some() {
        SlaacHandler::generate_link_local_address(core_ctx, bindings_ctx, device_id);
    }

    RsHandler::start_router_solicitation(core_ctx, bindings_ctx, device_id);
}

fn disable_ipv6_device_with_config<
    BC: IpDeviceBindingsContext<Ipv6, CC::DeviceId>,
    CC: Ipv6DeviceContext<BC>
        + GmpHandler<Ipv6, BC>
        + RsHandler<BC>
        + DadHandler<Ipv6, BC>
        + RouteDiscoveryHandler<BC>
        + SlaacHandler<BC>
        + NudIpHandler<Ipv6, BC>,
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &CC::DeviceId,
    device_config: &Ipv6DeviceConfiguration,
) {
    NudIpHandler::flush_neighbor_table(core_ctx, bindings_ctx, device_id);

    SlaacHandler::remove_all_slaac_addresses(core_ctx, bindings_ctx, device_id);

    RouteDiscoveryHandler::invalidate_routes(core_ctx, bindings_ctx, device_id);

    RsHandler::stop_router_solicitation(core_ctx, bindings_ctx, device_id);

    // Reset the learned network parameters. If the device is re-enabled in the
    // future, there's no guarantee that it's on the same network.
    core_ctx.with_network_learned_parameters_mut(device_id, |params| params.reset());

    // Delete the link-local address generated when enabling the device and stop
    // DAD on the other addresses.
    core_ctx
        .with_address_ids(device_id, |addrs, core_ctx| {
            addrs
                .map(|addr_id| {
                    core_ctx.with_ip_address_data(
                        device_id,
                        &addr_id,
                        |IpAddressData { flags: _, config }| (addr_id.clone(), *config),
                    )
                })
                .collect::<Vec<_>>()
        })
        .into_iter()
        .for_each(|(addr_id, config)| {
            if config
                .is_some_and(|config| config.is_slaac() && addr_id.addr().addr().is_link_local())
            {
                del_ip_addr_inner_and_notify_handler(
                    core_ctx,
                    bindings_ctx,
                    device_id,
                    DelIpAddr::AddressId(addr_id),
                    AddressRemovedReason::Manual,
                    device_config,
                )
                .map(|remove_result| {
                    bindings_ctx.defer_removal_result(remove_result);
                })
                .unwrap_or_else(|NotFoundError| {
                    // We're not holding locks on the addresses anymore we must
                    // allow a NotFoundError since the address can be removed as
                    // we release the lock.
                })
            } else {
                DadHandler::stop_duplicate_address_detection(
                    core_ctx,
                    bindings_ctx,
                    device_id,
                    &addr_id,
                );
                bindings_ctx.on_event(IpDeviceEvent::AddressStateChanged {
                    device: device_id.clone(),
                    addr: addr_id.addr().into(),
                    state: IpAddressState::Unavailable,
                });
            }
        });

    GmpHandler::gmp_handle_disabled(core_ctx, bindings_ctx, device_id);
    leave_ip_multicast_with_config(
        core_ctx,
        bindings_ctx,
        device_id,
        Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS,
        device_config,
    );
}

fn enable_ipv4_device_with_config<
    BC: IpDeviceBindingsContext<Ipv4, CC::DeviceId>,
    CC: IpDeviceStateContext<Ipv4, BC> + GmpHandler<Ipv4, BC> + DadHandler<Ipv4, BC>,
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &CC::DeviceId,
    config: &Ipv4DeviceConfiguration,
) {
    // All systems should join the all-systems multicast group.
    join_ip_multicast_with_config(
        core_ctx,
        bindings_ctx,
        device_id,
        Ipv4::ALL_SYSTEMS_MULTICAST_ADDRESS,
        config,
    );
    GmpHandler::gmp_handle_maybe_enabled(core_ctx, bindings_ctx, device_id);
    core_ctx
        .with_address_ids(device_id, |addrs, _core_ctx| addrs.collect::<Vec<_>>())
        .into_iter()
        .for_each(|addr_id| {
            let needs_dad = DadHandler::initialize_duplicate_address_detection(
                core_ctx,
                bindings_ctx,
                device_id,
                &addr_id,
                |state| IpDeviceEvent::AddressStateChanged {
                    device: device_id.clone(),
                    addr: addr_id.addr().into(),
                    state,
                },
            );
            match needs_dad {
                NeedsDad::Yes(token) => {
                    core_ctx.start_duplicate_address_detection(bindings_ctx, token);
                }
                NeedsDad::No => {}
            }
        })
}

fn disable_ipv4_device_with_config<
    BC: IpDeviceBindingsContext<Ipv4, CC::DeviceId>,
    CC: IpDeviceStateContext<Ipv4, BC>
        + GmpHandler<Ipv4, BC>
        + DadHandler<Ipv4, BC>
        + NudIpHandler<Ipv4, BC>,
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &CC::DeviceId,
    config: &Ipv4DeviceConfiguration,
) {
    NudIpHandler::flush_neighbor_table(core_ctx, bindings_ctx, device_id);
    GmpHandler::gmp_handle_disabled(core_ctx, bindings_ctx, device_id);
    leave_ip_multicast_with_config(
        core_ctx,
        bindings_ctx,
        device_id,
        Ipv4::ALL_SYSTEMS_MULTICAST_ADDRESS,
        config,
    );
    core_ctx
        .with_address_ids(device_id, |addrs, _core_ctx| addrs.collect::<Vec<_>>())
        .into_iter()
        .for_each(|addr_id| {
            DadHandler::stop_duplicate_address_detection(
                core_ctx,
                bindings_ctx,
                device_id,
                &addr_id,
            );
            bindings_ctx.on_event(IpDeviceEvent::AddressStateChanged {
                device: device_id.clone(),
                addr: addr_id.addr().into(),
                state: IpAddressState::Unavailable,
            });
        });
}

/// Gets a single IPv4 address and subnet for a device.
pub fn get_ipv4_addr_subnet<BT: IpDeviceStateBindingsTypes, CC: IpDeviceStateContext<Ipv4, BT>>(
    core_ctx: &mut CC,
    device_id: &CC::DeviceId,
) -> Option<AddrSubnet<Ipv4Addr, Ipv4DeviceAddr>> {
    core_ctx.with_address_ids(device_id, |mut addrs, _core_ctx| addrs.next().map(|a| a.addr_sub()))
}

/// Gets the hop limit for new IPv6 packets that will be sent out from `device`.
pub fn get_ipv6_hop_limit<BT: IpDeviceStateBindingsTypes, CC: IpDeviceStateContext<Ipv6, BT>>(
    core_ctx: &mut CC,
    device: &CC::DeviceId,
) -> NonZeroU8 {
    core_ctx.with_default_hop_limit(device, Clone::clone)
}

/// Is IP packet unicast forwarding enabled?
pub fn is_ip_unicast_forwarding_enabled<
    I: IpDeviceIpExt,
    BC: IpDeviceBindingsContext<I, CC::DeviceId>,
    CC: IpDeviceConfigurationContext<I, BC>,
>(
    core_ctx: &mut CC,
    device_id: &CC::DeviceId,
) -> bool {
    core_ctx.with_ip_device_configuration(device_id, |state, _ctx| {
        AsRef::<IpDeviceConfiguration>::as_ref(state).unicast_forwarding_enabled
    })
}

/// Is IP packet multicast forwarding enabled?
pub fn is_ip_multicast_forwarding_enabled<
    I: IpDeviceIpExt,
    BC: IpDeviceBindingsContext<I, CC::DeviceId>,
    CC: IpDeviceConfigurationContext<I, BC>,
>(
    core_ctx: &mut CC,
    device_id: &CC::DeviceId,
) -> bool {
    core_ctx.with_ip_device_configuration(device_id, |state, _ctx| {
        AsRef::<IpDeviceConfiguration>::as_ref(state).multicast_forwarding_enabled
    })
}

/// Joins the multicast group `multicast_addr` on `device_id`.
///
/// `_config` is not used but required to make sure that the caller is currently
/// holding a a reference to the IP device's IP configuration as a way to prove
/// that caller has synchronized this operation with other accesses to the IP
/// device configuration.
pub fn join_ip_multicast_with_config<
    I: IpDeviceIpExt,
    BC: IpDeviceBindingsContext<I, CC::DeviceId>,
    CC: IpDeviceStateContext<I, BC> + GmpHandler<I, BC>,
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &CC::DeviceId,
    multicast_addr: MulticastAddr<I::Addr>,
    _config: &I::Configuration,
) {
    match core_ctx.gmp_join_group(bindings_ctx, device_id, multicast_addr) {
        GroupJoinResult::Joined(()) => {
            debug!("joined IP Multicast Group {multicast_addr} on device {device_id:?}");
            core_ctx.join_link_multicast_group(bindings_ctx, device_id, multicast_addr)
        }
        GroupJoinResult::AlreadyMember => {}
    }
}

/// Adds `device_id` to a multicast group `multicast_addr`.
///
/// Calling `join_ip_multicast` multiple times is completely safe. A counter
/// will be kept for the number of times `join_ip_multicast` has been called
/// with the same `device_id` and `multicast_addr` pair. To completely leave a
/// multicast group, [`leave_ip_multicast`] must be called the same number of
/// times `join_ip_multicast` has been called for the same `device_id` and
/// `multicast_addr` pair. The first time `join_ip_multicast` is called for a
/// new `device` and `multicast_addr` pair, the device will actually join the
/// multicast group.
pub fn join_ip_multicast<
    I: IpDeviceIpExt,
    BC: IpDeviceBindingsContext<I, CC::DeviceId>,
    CC: IpDeviceConfigurationContext<I, BC>,
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &CC::DeviceId,
    multicast_addr: MulticastAddr<I::Addr>,
) {
    core_ctx.with_ip_device_configuration(device_id, |config, mut core_ctx| {
        join_ip_multicast_with_config(
            &mut core_ctx,
            bindings_ctx,
            device_id,
            multicast_addr,
            config,
        )
    })
}

/// Leaves the multicast group `multicast_addr` on `device_id`.
///
/// `_config` is not used but required to make sure that the caller is currently
/// holding a a reference to the IP device's IP configuration as a way to prove
/// that caller has synchronized this operation with other accesses to the IP
/// device configuration.
pub fn leave_ip_multicast_with_config<
    I: IpDeviceIpExt,
    BC: IpDeviceBindingsContext<I, CC::DeviceId>,
    CC: IpDeviceStateContext<I, BC> + GmpHandler<I, BC>,
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &CC::DeviceId,
    multicast_addr: MulticastAddr<I::Addr>,
    _config: &I::Configuration,
) {
    match core_ctx.gmp_leave_group(bindings_ctx, device_id, multicast_addr) {
        GroupLeaveResult::Left(()) => {
            core_ctx.leave_link_multicast_group(bindings_ctx, device_id, multicast_addr)
        }
        GroupLeaveResult::StillMember => {}
        GroupLeaveResult::NotMember => panic!(
            "attempted to leave IP multicast group we were not a member of: {}",
            multicast_addr,
        ),
    }
}

/// Removes `device_id` from a multicast group `multicast_addr`.
///
/// `leave_ip_multicast` will attempt to remove `device_id` from a multicast
/// group `multicast_addr`. `device_id` may have "joined" the same multicast
/// address multiple times, so `device_id` will only leave the multicast group
/// once `leave_ip_multicast` has been called for each corresponding
/// [`join_ip_multicast`]. That is, if `join_ip_multicast` gets called 3
/// times and `leave_ip_multicast` gets called two times (after all 3
/// `join_ip_multicast` calls), `device_id` will still be in the multicast
/// group until the next (final) call to `leave_ip_multicast`.
///
/// # Panics
///
/// If `device_id` is not currently in the multicast group `multicast_addr`.
pub fn leave_ip_multicast<
    I: IpDeviceIpExt,
    BC: IpDeviceBindingsContext<I, CC::DeviceId>,
    CC: IpDeviceConfigurationContext<I, BC>,
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &CC::DeviceId,
    multicast_addr: MulticastAddr<I::Addr>,
) {
    core_ctx.with_ip_device_configuration(device_id, |config, mut core_ctx| {
        leave_ip_multicast_with_config(
            &mut core_ctx,
            bindings_ctx,
            device_id,
            multicast_addr,
            config,
        )
    })
}

/// Adds `addr_sub` to `device_id` with configuration `addr_config`.
///
/// `_device_config` is not used but required to make sure that the caller is
/// currently holding a a reference to the IP device's IP configuration as a way
/// to prove that caller has synchronized this operation with other accesses to
/// the IP device configuration.
///
/// See [`IpDeviceAddAddressContext`] for further synchronization guarantees.
pub fn add_ip_addr_subnet_with_config<
    I: IpDeviceIpExt,
    BC: IpDeviceBindingsContext<I, CC::DeviceId>,
    CC: IpDeviceStateContext<I, BC>
        + IpDeviceAddAddressContext<I, BC>
        + GmpHandler<I, BC>
        + DadHandler<I, BC>,
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &CC::DeviceId,
    addr_sub: AddrSubnet<I::Addr, I::AssignedWitness>,
    addr_config: I::AddressConfig<BC::Instant>,
    _device_config: &I::Configuration,
) -> Result<CC::AddressId, ExistsError> {
    // LINT.IfChange(netstack_ip_added_tefmo)
    info!("adding addr {addr_sub:?} config {addr_config:?} to device {device_id:?}");
    // LINT.ThenChange(//tools/testing/tefmocheck/cdc_ethernet_state_check.go:netstack_ip_added_tefmo)
    let CommonAddressProperties { valid_until, preferred_lifetime } =
        I::get_common_props(&addr_config);
    let addr_id = core_ctx.add_ip_address(device_id, addr_sub, addr_config)?;
    assert_eq!(addr_id.addr().addr(), addr_sub.addr().get());

    let ip_enabled =
        core_ctx.with_ip_device_flags(device_id, |IpDeviceFlags { ip_enabled }| *ip_enabled);

    let needs_dad = if ip_enabled {
        DadHandler::initialize_duplicate_address_detection(
            core_ctx,
            bindings_ctx,
            device_id,
            &addr_id,
            |state| IpDeviceEvent::AddressAdded {
                device: device_id.clone(),
                addr: addr_sub.to_witness(),
                state,
                valid_until,
                preferred_lifetime,
            },
        )
    } else {
        // NB: We don't start DAD if the device is disabled. DAD will be
        // performed when the device is enabled for all addresses.
        //
        // Typically, the `AddressAdded` event would be dispatched by the DAD
        // engine, but because we're not calling into the DAD engine we need
        // to dispatch the event here.
        bindings_ctx.on_event(IpDeviceEvent::AddressAdded {
            device: device_id.clone(),
            addr: addr_sub.to_witness(),
            state: IpAddressState::Unavailable,
            valid_until,
            preferred_lifetime,
        });
        NeedsDad::No
    };

    match needs_dad {
        NeedsDad::Yes(token) => {
            core_ctx.start_duplicate_address_detection(bindings_ctx, token);
        }
        NeedsDad::No => {}
    }

    Ok(addr_id)
}

/// A handler to abstract side-effects of removing IP device addresses.
pub trait IpAddressRemovalHandler<I: IpDeviceIpExt, BC: InstantBindingsTypes>:
    DeviceIdContext<AnyDevice>
{
    /// Notifies the handler that the addr `addr` with `config` has been removed
    /// from `device_id` with `reason`.
    fn on_address_removed(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        addr_sub: AddrSubnet<I::Addr, I::AssignedWitness>,
        config: I::AddressConfig<BC::Instant>,
        reason: AddressRemovedReason,
    );
}

/// There's no special action to be taken for removed IPv4 addresses.
impl<CC: DeviceIdContext<AnyDevice>, BC: InstantBindingsTypes> IpAddressRemovalHandler<Ipv4, BC>
    for CC
{
    fn on_address_removed(
        &mut self,
        _bindings_ctx: &mut BC,
        _device_id: &Self::DeviceId,
        _addr_sub: AddrSubnet<Ipv4Addr, Ipv4DeviceAddr>,
        _config: Ipv4AddrConfig<BC::Instant>,
        _reason: AddressRemovedReason,
    ) {
        // Nothing to do.
    }
}

/// Provide the IPv6 implementation for all [`SlaacHandler`] implementations.
impl<CC: SlaacHandler<BC>, BC: InstantContext> IpAddressRemovalHandler<Ipv6, BC> for CC {
    fn on_address_removed(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        addr_sub: AddrSubnet<Ipv6Addr, Ipv6DeviceAddr>,
        config: Ipv6AddrConfig<BC::Instant>,
        reason: AddressRemovedReason,
    ) {
        match config {
            Ipv6AddrConfig::Slaac(config) => SlaacHandler::on_address_removed(
                self,
                bindings_ctx,
                device_id,
                addr_sub,
                config,
                reason,
            ),
            Ipv6AddrConfig::Manual(_manual_config) => (),
        }
    }
}

/// Possible representations of an IP address that is valid for deletion.
#[allow(missing_docs)]
pub enum DelIpAddr<Id, A> {
    SpecifiedAddr(SpecifiedAddr<A>),
    AddressId(Id),
}

impl<Id: IpAddressId<A>, A: IpAddress<Version: AssignedAddrIpExt>> Display for DelIpAddr<Id, A> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DelIpAddr::SpecifiedAddr(addr) => write!(f, "{}", *addr),
            DelIpAddr::AddressId(id) => write!(f, "{}", id.addr()),
        }
    }
}

/// Deletes an IP address from a device, returning the address and its
/// configuration if it was removed.
pub fn del_ip_addr_inner<
    I: IpDeviceIpExt,
    BC: IpDeviceBindingsContext<I, CC::DeviceId>,
    CC: IpDeviceStateContext<I, BC> + GmpHandler<I, BC> + DadHandler<I, BC>,
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &CC::DeviceId,
    addr: DelIpAddr<CC::AddressId, I::Addr>,
    reason: AddressRemovedReason,
    // Require configuration lock to do this.
    //
    // This ensures a fixed device configuration, but also synchronization with
    // address addition and removal.
    //
    // See `IpDeviceAddAddressContext` for further synchronization guarantees.
    _config: &I::Configuration,
) -> Result<
    (
        AddrSubnet<I::Addr, I::AssignedWitness>,
        I::AddressConfig<BC::Instant>,
        RemoveResourceResultWithContext<AddrSubnet<I::Addr>, BC>,
    ),
    NotFoundError,
> {
    let addr_id = match addr {
        DelIpAddr::SpecifiedAddr(addr) => core_ctx.get_address_id(device_id, addr)?,
        DelIpAddr::AddressId(id) => id,
    };
    DadHandler::stop_duplicate_address_detection(core_ctx, bindings_ctx, device_id, &addr_id);
    // Extract the configuration out of the address to properly mark it as ready
    // for deletion. If the configuration has already been taken, consider as if
    // the address is already removed.
    let addr_config = core_ctx
        .with_ip_address_data_mut(device_id, &addr_id, |addr_data| addr_data.config.take())
        .ok_or(NotFoundError)?;

    let addr_sub = addr_id.addr_sub();
    let result = core_ctx.remove_ip_address(device_id, addr_id);

    bindings_ctx.on_event(IpDeviceEvent::AddressRemoved {
        device: device_id.clone(),
        addr: addr_sub.addr().into(),
        reason,
    });

    Ok((addr_sub, addr_config, result))
}

/// Removes an IP address and associated subnet from this device.
fn del_ip_addr<
    I: IpDeviceIpExt,
    BC: IpDeviceBindingsContext<I, CC::DeviceId>,
    CC: IpDeviceConfigurationContext<I, BC>,
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &CC::DeviceId,
    addr: DelIpAddr<CC::AddressId, I::Addr>,
    reason: AddressRemovedReason,
) -> Result<RemoveResourceResultWithContext<AddrSubnet<I::Addr>, BC>, NotFoundError> {
    info!("removing addr {addr} from device {device_id:?}");
    core_ctx.with_ip_device_configuration(device_id, |config, mut core_ctx| {
        del_ip_addr_inner_and_notify_handler(
            &mut core_ctx,
            bindings_ctx,
            device_id,
            addr,
            reason,
            config,
        )
    })
}

/// Removes an IP address and associated subnet from this device and notifies
/// the address removal handler.
fn del_ip_addr_inner_and_notify_handler<
    I: IpDeviceIpExt,
    BC: IpDeviceBindingsContext<I, CC::DeviceId>,
    CC: IpDeviceStateContext<I, BC>
        + GmpHandler<I, BC>
        + DadHandler<I, BC>
        + IpAddressRemovalHandler<I, BC>,
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &CC::DeviceId,
    addr: DelIpAddr<CC::AddressId, I::Addr>,
    reason: AddressRemovedReason,
    config: &I::Configuration,
) -> Result<RemoveResourceResultWithContext<AddrSubnet<I::Addr>, BC>, NotFoundError> {
    del_ip_addr_inner(core_ctx, bindings_ctx, device_id, addr, reason, config).map(
        |(addr_sub, config, result)| {
            core_ctx.on_address_removed(bindings_ctx, device_id, addr_sub, config, reason);
            result
        },
    )
}

/// Returns whether `device_id` is enabled for IP version `I`.
pub fn is_ip_device_enabled<
    I: IpDeviceIpExt,
    BC: IpDeviceBindingsContext<I, CC::DeviceId>,
    CC: IpDeviceStateContext<I, BC>,
>(
    core_ctx: &mut CC,
    device_id: &CC::DeviceId,
) -> bool {
    core_ctx.with_ip_device_flags(device_id, |flags| flags.ip_enabled)
}

/// Removes IPv4 state for the device without emitting events.
pub fn clear_ipv4_device_state<
    BC: IpDeviceBindingsContext<Ipv4, CC::DeviceId>,
    CC: IpDeviceConfigurationContext<Ipv4, BC>,
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &CC::DeviceId,
) {
    core_ctx.with_ip_device_configuration_mut(device_id, |mut core_ctx| {
        let ip_enabled = core_ctx.with_configuration_and_flags_mut(device_id, |_config, flags| {
            // Start by force-disabling IPv4 so we're sure we won't handle
            // any more packets.
            let IpDeviceFlags { ip_enabled } = flags;
            core::mem::replace(ip_enabled, false)
        });

        let (config, mut core_ctx) = core_ctx.ip_device_configuration_and_ctx();
        let core_ctx = &mut core_ctx;
        if ip_enabled {
            disable_ipv4_device_with_config(core_ctx, bindings_ctx, device_id, config);
        }
    })
}

/// Removes IPv6 state for the device without emitting events.
pub fn clear_ipv6_device_state<
    BC: IpDeviceBindingsContext<Ipv6, CC::DeviceId>,
    CC: Ipv6DeviceConfigurationContext<BC>,
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &CC::DeviceId,
) {
    core_ctx.with_ipv6_device_configuration_mut(device_id, |mut core_ctx| {
        let ip_enabled = core_ctx.with_configuration_and_flags_mut(device_id, |_config, flags| {
            // Start by force-disabling IPv6 so we're sure we won't handle
            // any more packets.
            let IpDeviceFlags { ip_enabled } = flags;
            core::mem::replace(ip_enabled, false)
        });

        let (config, mut core_ctx) = core_ctx.ipv6_device_configuration_and_ctx();
        let core_ctx = &mut core_ctx;
        if ip_enabled {
            disable_ipv6_device_with_config(core_ctx, bindings_ctx, device_id, config);
        }
    })
}

/// Dispatches a received ARP packet (Request or Reply) to the IP layer.
///
/// Returns whether the `target_addr` is assigned on the device.
pub fn on_arp_packet<CC, BC>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &CC::DeviceId,
    frame_src: impl Debug,
    sender_hwaddr: impl Debug,
    sender_addr: Ipv4Addr,
    target_addr: Ipv4Addr,
    is_arp_probe: bool,
) -> bool
where
    CC: IpDeviceHandler<Ipv4, BC> + IpDeviceStateContext<Ipv4, BC>,
    BC: IpDeviceStateBindingsTypes,
{
    // As Per RFC 5227, section 2.1.1
    //   If [...] the host receives any ARP packet (Request *or* Reply) on the
    //   interface where the probe is being performed, where the packet's
    //   'sender IP address' is the address being probed for, then the host MUST
    //   treat this address as being in use by some other host.
    if let Some(sender_addr) = SpecifiedAddr::new(sender_addr) {
        let sender_addr_state = IpDeviceHandler::<Ipv4, _>::handle_received_dad_packet(
            core_ctx,
            bindings_ctx,
            &device_id,
            sender_addr,
            Ipv4DadAddressInfo::SourceAddr,
        );
        match sender_addr_state {
            None => {}
            // As Per RFC 5227 section 2.4:
            //   At any time, if a host receives an ARP packet (Request *or*
            //   Reply) where the 'sender IP address' is (one of) the host's own
            //   IP address(es) configured on that interface, but the 'sender
            //   hardware address' does not match any of the host's own
            //   interface addresses, then this is a conflicting ARP packet,
            //   indicating some other host also thinks it is validly using this
            //   address.
            Some(state) => {
                info!(
                    "DAD received conflicting ARP packet (sender) for {sender_addr} \
                    (state={state:?}). arp_sha={sender_hwaddr:?}, frame_src={frame_src:?}"
                );
            }
        }
    }

    let Some(target_addr) = SpecifiedAddr::new(target_addr) else {
        return false;
    };

    // As Per RFC 5227, section 2.1.1
    //  In addition, if during this period the host receives any ARP Probe
    //  where the packet's 'target IP address' is the address being probed
    //  for, [... ] then the host SHOULD similarly treat this as an address
    //  conflict.
    if is_arp_probe {
        let target_addr_state = IpDeviceHandler::<Ipv4, _>::handle_received_dad_packet(
            core_ctx,
            bindings_ctx,
            &device_id,
            target_addr,
            Ipv4DadAddressInfo::TargetAddr,
        );
        let assigned = match target_addr_state {
            None => false,
            // Unlike the sender_addr, it's not concerning to receive an
            // ARP packet whose target_addr is assigned to us.
            Some(IpAddressState::Assigned) => true,
            Some(state @ (IpAddressState::Tentative | IpAddressState::Unavailable)) => {
                info!(
                    "DAD received conflicting ARP packet (target) for {target_addr} \
                    (state={state:?}). arp_sha={sender_hwaddr:?}, frame_src={frame_src:?}"
                );
                false
            }
        };
        assigned
    } else {
        // Otherwise, don't dispatch the probe to DAD. Instead just return the
        // target address' assignment state.
        let addr_id = match core_ctx.get_address_id(device_id, target_addr) {
            Ok(o) => o,
            Err(NotFoundError) => return false,
        };

        core_ctx.with_ip_address_data(
            device_id,
            &addr_id,
            |IpAddressData { flags: IpAddressFlags { assigned }, config: _ }| *assigned,
        )
    }
}

#[cfg(any(test, feature = "testutils"))]
pub(crate) mod testutil {
    use alloc::boxed::Box;

    use crate::device::IpAddressFlags;

    use super::*;

    /// Calls the callback with an iterator of the IPv4 addresses assigned to
    /// `device_id`.
    pub fn with_assigned_ipv4_addr_subnets<
        BT: IpDeviceStateBindingsTypes,
        CC: IpDeviceStateContext<Ipv4, BT>,
        O,
        F: FnOnce(Box<dyn Iterator<Item = AddrSubnet<Ipv4Addr, Ipv4DeviceAddr>> + '_>) -> O,
    >(
        core_ctx: &mut CC,
        device_id: &CC::DeviceId,
        cb: F,
    ) -> O {
        core_ctx.with_address_ids(device_id, |addrs, _core_ctx| {
            cb(Box::new(addrs.map(|a| a.addr_sub())))
        })
    }

    /// Gets the IPv6 address and subnet pairs associated with this device which are
    /// in the assigned state.
    ///
    /// Tentative IP addresses (addresses which are not yet fully bound to a device)
    /// and deprecated IP addresses (addresses which have been assigned but should
    /// no longer be used for new connections) will not be returned by
    /// `get_assigned_ipv6_addr_subnets`.
    ///
    /// Returns an [`Iterator`] of `AddrSubnet`.
    ///
    /// See [`Tentative`] and [`AddrSubnet`] for more information.
    pub fn with_assigned_ipv6_addr_subnets<
        BC: IpDeviceBindingsContext<Ipv6, CC::DeviceId>,
        CC: Ipv6DeviceContext<BC>,
        O,
        F: FnOnce(Box<dyn Iterator<Item = AddrSubnet<Ipv6Addr, Ipv6DeviceAddr>> + '_>) -> O,
    >(
        core_ctx: &mut CC,
        device_id: &CC::DeviceId,
        cb: F,
    ) -> O {
        core_ctx.with_address_ids(device_id, |addrs, core_ctx| {
            cb(Box::new(addrs.filter_map(|addr_id| {
                core_ctx
                    .with_ip_address_data(
                        device_id,
                        &addr_id,
                        |IpAddressData { flags: IpAddressFlags { assigned }, config: _ }| *assigned,
                    )
                    .then(|| addr_id.addr_sub())
            })))
        })
    }
}
