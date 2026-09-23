// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The integrations for protocols built on top of an IP device.

use core::borrow::Borrow;
use core::marker::PhantomData;
use core::num::NonZeroU8;
use core::ops::{Deref as _, DerefMut as _};
use core::sync::atomic::AtomicU16;

use lock_order::lock::{LockLevelFor, UnlockedAccess};
use lock_order::relation::LockBefore;
use log::debug;
use net_types::ip::{AddrSubnet, Ip, IpMarked, Ipv4, Ipv4Addr, Ipv6, Ipv6Addr, Mtu};
use net_types::{LinkLocalUnicastAddr, MulticastAddr, SpecifiedAddr, UnicastAddr, Witness as _};
use netstack3_base::{
    AnyDevice, CoreTimerContext, CounterContext, DeviceIdContext, ExistsError, IpAddressId as _,
    IpDeviceAddr, IpDeviceAddressIdContext, Ipv4DeviceAddr, Ipv6DeviceAddr,
    NetworkPartialSerializer, NetworkSerializer, NotFoundError, RemoveResourceResultWithContext,
    ResourceCounterContext,
};
use netstack3_device::ethernet::EthernetDeviceId;
use netstack3_device::{DeviceId, WeakDeviceId};
use netstack3_filter::FilterImpl;
use netstack3_ip::device::{
    self, AddressRemovedReason, DadAddressContext, DadAddressStateRef, DadContext, DadState,
    DadStateRef, DadTimerId, DefaultHopLimit, DelIpAddr, DualStackIpDeviceState, IpAddressData,
    IpAddressEntry, IpAddressFlags, IpDeviceAddresses, IpDeviceConfiguration, IpDeviceFlags,
    IpDeviceIpExt, IpDeviceMulticastGroups, IpDeviceStateBindingsTypes, IpDeviceStateContext,
    IpDeviceStateIpExt, IpDeviceTimerId, Ipv4DadSendData, Ipv4DeviceConfiguration,
    Ipv4DeviceTimerId, Ipv6AddrConfig, Ipv6AddrSlaacConfig, Ipv6DadAddressContext, Ipv6DadSendData,
    Ipv6DeviceConfiguration, Ipv6DeviceTimerId, Ipv6DiscoveredRoute, Ipv6DiscoveredRouteProperties,
    Ipv6DiscoveredRoutesContext, Ipv6NetworkLearnedParameters, Ipv6RouteDiscoveryContext,
    Ipv6RouteDiscoveryState, RsContext, RsState, RsTimerId, SlaacAddressEntry,
    SlaacAddressEntryMut, SlaacAddresses, SlaacConfigAndState, SlaacContext, SlaacCounters,
    SlaacState, WeakAddressId, add_ip_addr_subnet_with_config, del_ip_addr_inner,
    get_ipv6_hop_limit, is_ip_device_enabled, is_ip_multicast_forwarding_enabled,
    is_ip_unicast_forwarding_enabled, join_ip_multicast_with_config,
    leave_ip_multicast_with_config,
};
use netstack3_ip::gmp::{
    GmpGroupState, GmpState, GmpStateRef, IgmpContext, IgmpContextMarker, IgmpSendContext,
    IgmpStateContext, IgmpTypeLayout, MldContext, MldContextMarker, MldSendContext,
    MldStateContext, MldTypeLayout, MulticastGroupSet,
};
use netstack3_ip::nud::{self, ConfirmationFlags, NudCounters, NudIpHandler};
use netstack3_ip::{
    self as ip, AddableMetric, AddressStatus, DEFAULT_TTL, FilterHandlerProvider, IpDeviceContext,
    IpDeviceEgressStateContext, IpDeviceIngressStateContext, IpLayerIpExt, IpSasHandler,
    IpSendFrameError, Ipv4PresentAddressStatus,
};
use packet::{EmptyBuf, InnerPacketBuilder};
use packet_formats::icmp::IcmpZeroCode;
use packet_formats::icmp::ndp::options::{NdpNonce, NdpOptionBuilder};
use packet_formats::icmp::ndp::{OptionSequenceBuilder, RouterSolicitation};
use packet_formats::utils::NonZeroDuration;

use crate::context::WrapLockLevel;
use crate::context::prelude::*;
use crate::{BindingsContext, BindingsTypes, CoreCtx, IpExt};

pub struct SlaacAddrs<'a, BC: BindingsContext> {
    pub(crate) core_ctx: CoreCtxWithIpDeviceConfiguration<
        'a,
        &'a Ipv6DeviceConfiguration,
        WrapLockLevel<crate::lock_ordering::Ipv6DeviceSlaac>,
        BC,
    >,
    pub(crate) device_id: DeviceId<BC>,
    pub(crate) config: &'a Ipv6DeviceConfiguration,
}

/// Provides an Iterator for `SlaacAddrs` to implement `SlaacAddresses`.
///
/// Note that we use concrete types here instead of going through traits because
/// it's the only way to satisfy the GAT bounds on `SlaacAddresses`' associated
/// type.
pub struct SlaacAddrsIter<'x, BC: BindingsContext> {
    core_ctx: CoreCtx<'x, BC, WrapLockLevel<crate::lock_ordering::IpDeviceAddresses<Ipv6>>>,
    addrs: ip::device::AddressIdIter<'x, Ipv6, BC>,
    device_id: &'x DeviceId<BC>,
}

impl<'x, BC> Iterator for SlaacAddrsIter<'x, BC>
where
    BC: BindingsContext,
{
    type Item = SlaacAddressEntry<BC::Instant>;
    fn next(&mut self) -> Option<Self::Item> {
        let Self { core_ctx, addrs, device_id } = self;
        // NB: This form is equivalent to using the `filter_map` combinator but
        // keeps the type signature simple.
        addrs.by_ref().find_map(|addr_id| {
            device::IpDeviceAddressContext::<Ipv6, _>::with_ip_address_data(
                core_ctx,
                device_id,
                &addr_id,
                |IpAddressData { flags: IpAddressFlags { assigned: _ }, config }| {
                    let addr_sub = addr_id.addr_sub();
                    match config {
                        Some(Ipv6AddrConfig::Slaac(config)) => {
                            Some(SlaacAddressEntry { addr_sub: addr_sub, config: *config })
                        }
                        None | Some(Ipv6AddrConfig::Manual(_)) => None,
                    }
                },
            )
        })
    }
}

impl<'a, BC: BindingsContext> CounterContext<SlaacCounters> for SlaacAddrs<'a, BC> {
    fn counters(&self) -> &SlaacCounters {
        &self
            .core_ctx
            .core_ctx
            .unlocked_access::<crate::lock_ordering::UnlockedState>()
            .ipv6
            .slaac_counters
    }
}

impl<'a, BC: BindingsContext> SlaacAddresses<BC> for SlaacAddrs<'a, BC> {
    fn for_each_addr_mut<F: FnMut(SlaacAddressEntryMut<'_, BC::Instant>)>(&mut self, mut cb: F) {
        let SlaacAddrs { core_ctx, device_id, config: _ } = self;
        let CoreCtxWithIpDeviceConfiguration { config: _, core_ctx } = core_ctx;
        let mut state = crate::device::integration::ip_device_state(core_ctx, device_id);
        let (addrs, mut locked) =
            state.read_lock_and::<crate::lock_ordering::IpDeviceAddresses<Ipv6>>();
        addrs.iter().for_each(|entry| {
            let addr_sub = *entry.addr_sub();
            let mut locked = locked.adopt(&**entry);
            let mut state = locked
                .write_lock_with::<crate::lock_ordering::IpDeviceAddressData<Ipv6>, _>(|c| {
                    c.right()
                });
            let IpAddressData { config, flags: IpAddressFlags { assigned: _ } } = &mut *state;

            match config {
                Some(Ipv6AddrConfig::Slaac(config)) => {
                    cb(SlaacAddressEntryMut { addr_sub, config })
                }
                None | Some(Ipv6AddrConfig::Manual(_)) => {}
            }
        })
    }

    type AddrsIter<'x> = SlaacAddrsIter<'x, BC>;

    fn with_addrs<O, F: FnOnce(Self::AddrsIter<'_>) -> O>(&mut self, cb: F) -> O {
        let SlaacAddrs { core_ctx, device_id, config: _ } = self;
        device::IpDeviceStateContext::<Ipv6, BC>::with_address_ids(
            core_ctx,
            device_id,
            |addrs, core_ctx| {
                cb(SlaacAddrsIter { core_ctx: core_ctx.as_owned(), addrs, device_id })
            },
        )
    }

    fn add_addr_sub_and_then<O, F: FnOnce(SlaacAddressEntryMut<'_, BC::Instant>, &mut BC) -> O>(
        &mut self,
        bindings_ctx: &mut BC,
        add_addr_sub: AddrSubnet<Ipv6Addr, Ipv6DeviceAddr>,
        slaac_config: Ipv6AddrSlaacConfig<BC::Instant>,
        and_then: F,
    ) -> Result<O, ExistsError> {
        let SlaacAddrs { core_ctx, device_id, config } = self;

        add_ip_addr_subnet_with_config::<Ipv6, _, _>(
            core_ctx,
            bindings_ctx,
            device_id,
            add_addr_sub.to_witness(),
            Ipv6AddrConfig::Slaac(slaac_config),
            config,
        )
        .map(|entry| {
            let addr_sub = entry.addr_sub();
            let mut locked = core_ctx.core_ctx.adopt(entry.deref());
            let mut state = locked
                .write_lock_with::<crate::lock_ordering::IpDeviceAddressData<Ipv6>, _>(|c| {
                    c.right()
                });
            let IpAddressData { config, flags: _ } = &mut *state;
            let config = assert_matches::assert_matches!(
                config,
                Some(Ipv6AddrConfig::Slaac(c)) => c
            );
            and_then(SlaacAddressEntryMut { addr_sub: addr_sub, config }, bindings_ctx)
        })
    }

    fn remove_addr(
        &mut self,
        bindings_ctx: &mut BC,
        addr: &Ipv6DeviceAddr,
    ) -> Result<
        (AddrSubnet<Ipv6Addr, Ipv6DeviceAddr>, Ipv6AddrSlaacConfig<BC::Instant>),
        NotFoundError,
    > {
        let SlaacAddrs { core_ctx, device_id, config } = self;
        del_ip_addr_inner::<Ipv6, _, _>(
            core_ctx,
            bindings_ctx,
            device_id,
            DelIpAddr::SpecifiedAddr(addr.into_specified()),
            AddressRemovedReason::Manual,
            config,
        )
        .map(|(addr_sub, config, result)| {
            assert_eq!(&addr_sub.addr(), addr);
            bindings_ctx.defer_removal_result(result);
            match config {
                Ipv6AddrConfig::Slaac(config) => (addr_sub, config),
                Ipv6AddrConfig::Manual(_manual_config) => {
                    unreachable!(
                        "address {addr_sub} on device {device_id:?} should have been a SLAAC \
                        address; config = {config:?}",
                    );
                }
            }
        })
    }
}

impl<BT: BindingsTypes, L> IgmpContextMarker for CoreCtx<'_, BT, L> {}

impl<BC: BindingsContext, L: LockBefore<crate::lock_ordering::IpDeviceGmp<Ipv4>>>
    IgmpStateContext<BC> for CoreCtx<'_, BC, L>
{
    fn with_igmp_state<
        O,
        F: FnOnce(
            &MulticastGroupSet<Ipv4Addr, GmpGroupState<Ipv4, BC>>,
            &GmpState<Ipv4, IgmpTypeLayout, BC>,
        ) -> O,
    >(
        &mut self,
        device: &Self::DeviceId,
        cb: F,
    ) -> O {
        let mut state = crate::device::integration::ip_device_state(self, device);
        let state = state.read_lock::<crate::lock_ordering::IpDeviceGmp<Ipv4>>();
        let IpDeviceMulticastGroups { groups, gmp, .. } = &*state;
        cb(groups, gmp)
    }
}

impl<BT: BindingsTypes, L> MldContextMarker for CoreCtx<'_, BT, L> {}

impl<BC: BindingsContext, L: LockBefore<crate::lock_ordering::IpDeviceGmp<Ipv6>>>
    MldStateContext<BC> for CoreCtx<'_, BC, L>
{
    fn with_mld_state<
        O,
        F: FnOnce(
            &MulticastGroupSet<Ipv6Addr, GmpGroupState<Ipv6, BC>>,
            &GmpState<Ipv6, MldTypeLayout, BC>,
        ) -> O,
    >(
        &mut self,
        device: &Self::DeviceId,
        cb: F,
    ) -> O {
        let mut state = crate::device::integration::ip_device_state(self, device);
        let state = state.read_lock::<crate::lock_ordering::IpDeviceGmp<Ipv6>>();
        let IpDeviceMulticastGroups { groups, gmp, .. } = &*state;
        cb(groups, gmp)
    }
}

/// Iterator over a device and its status for an address.
///
/// This is functionally identical to using `Iterator::filter_map` on the
/// provided devices and yielding devices with the address assigned (and the
/// status), but is named so that it can be used as an associated type.
pub struct FilterPresentWithDevices<
    I: IpLayerIpExt,
    Devices: Iterator<Item = Accessor::DeviceId>,
    Accessor: DeviceIdContext<AnyDevice>,
    BT,
> {
    devices: Devices,
    addr: SpecifiedAddr<I::Addr>,
    state_accessor: Accessor,
    assignment_state: fn(
        &mut Accessor,
        &Accessor::DeviceId,
        SpecifiedAddr<I::Addr>,
    ) -> AddressStatus<I::AddressStatus>,
    _marker: PhantomData<BT>,
}

impl<
    I: IpLayerIpExt,
    Devices: Iterator<Item = Accessor::DeviceId>,
    Accessor: DeviceIdContext<AnyDevice>,
    BT,
> FilterPresentWithDevices<I, Devices, Accessor, BT>
{
    fn new(
        devices: Devices,
        state_accessor: Accessor,
        assignment_state: fn(
            &mut Accessor,
            &Accessor::DeviceId,
            SpecifiedAddr<I::Addr>,
        ) -> AddressStatus<I::AddressStatus>,
        addr: SpecifiedAddr<I::Addr>,
    ) -> Self {
        Self { devices, addr, state_accessor, assignment_state, _marker: PhantomData }
    }
}

impl<
    's,
    BT: IpDeviceStateBindingsTypes,
    I: Ip + IpLayerIpExt + IpDeviceIpExt,
    Devices: Iterator<Item = Accessor::DeviceId>,
    Accessor: IpDeviceStateContext<I, BT>,
> Iterator for FilterPresentWithDevices<I, Devices, Accessor, BT>
where
    <I as IpDeviceIpExt>::State<BT>: 's,
{
    type Item = (Accessor::DeviceId, I::AddressStatus);
    fn next(&mut self) -> Option<Self::Item> {
        let Self { devices, addr, state_accessor, assignment_state, _marker } = self;
        devices
            .filter_map(|d| match assignment_state(state_accessor, &d, *addr) {
                AddressStatus::Present(status) => Some((d, status)),
                AddressStatus::Unassigned => None,
            })
            .next()
    }
}

impl<BC: BindingsContext, L: LockBefore<crate::lock_ordering::IpState<Ipv4>>>
    IpDeviceEgressStateContext<Ipv4> for CoreCtx<'_, BC, L>
{
    fn with_next_packet_id<O, F: FnOnce(&AtomicU16) -> O>(&self, cb: F) -> O {
        cb(&self.unlocked_access::<crate::lock_ordering::UnlockedState>().ipv4.next_packet_id)
    }

    fn get_local_addr_for_remote(
        &mut self,
        device_id: &Self::DeviceId,
        remote: Option<SpecifiedAddr<Ipv4Addr>>,
    ) -> Option<IpDeviceAddr<Ipv4Addr>> {
        IpSasHandler::<Ipv4, _>::get_local_addr_for_remote(self, device_id, remote)
    }

    fn get_hop_limit(&mut self, _device_id: &Self::DeviceId) -> NonZeroU8 {
        DEFAULT_TTL
    }
}

impl<BC: BindingsContext, L: LockBefore<crate::lock_ordering::IpDeviceGmp<Ipv4>>>
    IpDeviceIngressStateContext<Ipv4> for CoreCtx<'_, BC, L>
{
    fn address_status_for_device(
        &mut self,
        dst_ip: SpecifiedAddr<Ipv4Addr>,
        device_id: &Self::DeviceId,
    ) -> AddressStatus<Ipv4PresentAddressStatus> {
        AddressStatus::from_context_addr_v4(self, device_id, dst_ip)
    }
}

impl<BC: BindingsContext, L: LockBefore<crate::lock_ordering::IpDeviceConfiguration<Ipv4>>>
    IpDeviceContext<Ipv4> for CoreCtx<'_, BC, L>
{
    fn is_ip_device_enabled(&mut self, device_id: &Self::DeviceId) -> bool {
        is_ip_device_enabled::<Ipv4, _, _>(self, device_id)
    }

    type DeviceAndAddressStatusIter<'a> = FilterPresentWithDevices<
        Ipv4,
        <Self as device::IpDeviceConfigurationContext<Ipv4, BC>>::DevicesIter<'a>,
        <Self as device::IpDeviceConfigurationContext<Ipv4, BC>>::DeviceAddressAndGroupsAccessor<
            'a,
        >,
        BC,
    >;

    fn with_address_statuses<F: FnOnce(Self::DeviceAndAddressStatusIter<'_>) -> R, R>(
        &mut self,
        addr: SpecifiedAddr<Ipv4Addr>,
        cb: F,
    ) -> R {
        device::IpDeviceConfigurationContext::<Ipv4, _>::with_devices_and_state(
            self,
            |devices, state| {
                cb(FilterPresentWithDevices::new(
                    devices,
                    state,
                    AddressStatus::from_context_addr_v4,
                    addr,
                ))
            },
        )
    }

    fn is_device_unicast_forwarding_enabled(&mut self, device_id: &Self::DeviceId) -> bool {
        is_ip_unicast_forwarding_enabled::<Ipv4, _, _>(self, device_id)
    }
}

impl<BC: BindingsContext, L: LockBefore<crate::lock_ordering::IpState<Ipv6>>>
    IpDeviceEgressStateContext<Ipv6> for CoreCtx<'_, BC, L>
{
    fn with_next_packet_id<O, F: FnOnce(&()) -> O>(&self, cb: F) -> O {
        cb(&())
    }

    fn get_local_addr_for_remote(
        &mut self,
        device_id: &Self::DeviceId,
        remote: Option<SpecifiedAddr<Ipv6Addr>>,
    ) -> Option<IpDeviceAddr<Ipv6Addr>> {
        ip::IpSasHandler::<Ipv6, _>::get_local_addr_for_remote(self, device_id, remote)
    }

    fn get_hop_limit(&mut self, device_id: &Self::DeviceId) -> NonZeroU8 {
        get_ipv6_hop_limit(self, device_id)
    }
}

impl<BC: BindingsContext, L: LockBefore<crate::lock_ordering::IpDeviceGmp<Ipv6>>>
    IpDeviceIngressStateContext<Ipv6> for CoreCtx<'_, BC, L>
{
    fn address_status_for_device(
        &mut self,
        addr: SpecifiedAddr<Ipv6Addr>,
        device_id: &Self::DeviceId,
    ) -> AddressStatus<<Ipv6 as IpLayerIpExt>::AddressStatus> {
        AddressStatus::from_context_addr_v6(self, device_id, addr)
    }
}

impl<BC: BindingsContext, L: LockBefore<crate::lock_ordering::IpDeviceConfiguration<Ipv6>>>
    ip::IpDeviceContext<Ipv6> for CoreCtx<'_, BC, L>
{
    fn is_ip_device_enabled(&mut self, device_id: &Self::DeviceId) -> bool {
        is_ip_device_enabled::<Ipv6, _, _>(self, device_id)
    }

    type DeviceAndAddressStatusIter<'a> = FilterPresentWithDevices<
        Ipv6,
        <Self as device::IpDeviceConfigurationContext<Ipv6, BC>>::DevicesIter<'a>,
        <Self as device::IpDeviceConfigurationContext<Ipv6, BC>>::DeviceAddressAndGroupsAccessor<
            'a,
        >,
        BC,
    >;

    fn with_address_statuses<F: FnOnce(Self::DeviceAndAddressStatusIter<'_>) -> R, R>(
        &mut self,
        addr: SpecifiedAddr<Ipv6Addr>,
        cb: F,
    ) -> R {
        device::IpDeviceConfigurationContext::<Ipv6, _>::with_devices_and_state(
            self,
            |devices, state| {
                cb(FilterPresentWithDevices::new(
                    devices,
                    state,
                    AddressStatus::from_context_addr_v6,
                    addr,
                ))
            },
        )
    }

    fn is_device_unicast_forwarding_enabled(&mut self, device_id: &Self::DeviceId) -> bool {
        is_ip_unicast_forwarding_enabled::<Ipv6, _, _>(self, device_id)
    }
}

#[netstack3_macros::instantiate_ip_impl_block(I)]
impl<I: IpExt, BC: BindingsContext, L: LockBefore<crate::lock_ordering::IpDeviceConfiguration<I>>>
    ip::IpDeviceConfirmReachableContext<I, BC> for CoreCtx<'_, BC, L>
{
    fn confirm_reachable(
        &mut self,
        bindings_ctx: &mut BC,
        device: &Self::DeviceId,
        neighbor: SpecifiedAddr<<I as Ip>::Addr>,
    ) {
        match device {
            DeviceId::Ethernet(id) => {
                nud::confirm_reachable::<I, _, _, _>(self, bindings_ctx, id, neighbor)
            }
            // NUD is not supported on Loopback, pure IP, or blackhole devices.
            DeviceId::Loopback(_) | DeviceId::PureIp(_) | DeviceId::Blackhole(_) => {}
        }
    }
}

impl<I: IpExt, BC: BindingsContext, L: LockBefore<crate::lock_ordering::EthernetDeviceDynamicState>>
    ip::IpDeviceMtuContext<I> for CoreCtx<'_, BC, L>
{
    fn get_mtu(&mut self, device_id: &Self::DeviceId) -> Mtu {
        crate::device::integration::get_mtu(self, device_id)
    }
}

#[netstack3_macros::instantiate_ip_impl_block(I)]
impl<I: IpExt, BC: BindingsContext, L: LockBefore<crate::lock_ordering::IpDeviceConfiguration<I>>>
    ip::multicast_forwarding::MulticastForwardingDeviceContext<I> for CoreCtx<'_, BC, L>
{
    fn is_device_multicast_forwarding_enabled(&mut self, device_id: &Self::DeviceId) -> bool {
        is_ip_multicast_forwarding_enabled::<I, _, _>(self, device_id)
    }
}

pub struct CoreCtxWithIpDeviceConfiguration<'a, Config, L, BC: BindingsContext> {
    pub config: Config,
    pub core_ctx: CoreCtx<'a, BC, L>,
}

impl<'a, Config, L, BC: BindingsContext, T> CounterContext<T>
    for CoreCtxWithIpDeviceConfiguration<'a, Config, L, BC>
where
    CoreCtx<'a, BC, L>: CounterContext<T>,
{
    fn counters(&self) -> &T {
        self.core_ctx.counters()
    }
}

impl<'a, Config, L, BC: BindingsContext, R, T> ResourceCounterContext<R, T>
    for CoreCtxWithIpDeviceConfiguration<'a, Config, L, BC>
where
    CoreCtx<'a, BC, L>: ResourceCounterContext<R, T>,
{
    fn per_resource_counters<'b>(&'b self, resource: &'b R) -> &'b T {
        self.core_ctx.per_resource_counters(resource)
    }
}

impl<'a, Config, L, BC: BindingsContext, T> CoreTimerContext<T, BC>
    for CoreCtxWithIpDeviceConfiguration<'a, Config, L, BC>
where
    CoreCtx<'a, BC, L>: CoreTimerContext<T, BC>,
{
    fn convert_timer(dispatch_id: T) -> BC::DispatchId {
        <CoreCtx<'a, BC, L> as CoreTimerContext<T, BC>>::convert_timer(dispatch_id)
    }
}

#[netstack3_macros::instantiate_ip_impl_block(I)]
impl<'a, I: gmp::IpExt + IpDeviceIpExt, BC: BindingsContext>
    device::WithIpDeviceConfigurationMutInner<I, BC>
    for CoreCtxWithIpDeviceConfiguration<
        'a,
        &mut <I as IpDeviceIpExt>::Configuration,
        WrapLockLevel<crate::lock_ordering::IpDeviceConfiguration<I>>,
        BC,
    >
{
    type IpDeviceStateCtx<'s>
        = CoreCtxWithIpDeviceConfiguration<
        's,
        &'s <I as IpDeviceIpExt>::Configuration,
        WrapLockLevel<crate::lock_ordering::IpDeviceConfiguration<I>>,
        BC,
    >
    where
        Self: 's;

    fn ip_device_configuration_and_ctx(
        &mut self,
    ) -> (&<I as IpDeviceIpExt>::Configuration, Self::IpDeviceStateCtx<'_>) {
        let Self { config, core_ctx } = self;
        let config = &**config;
        (config, CoreCtxWithIpDeviceConfiguration { config, core_ctx: core_ctx.as_owned() })
    }

    fn with_configuration_and_flags_mut<
        O,
        F: FnOnce(&mut <I as IpDeviceIpExt>::Configuration, &mut IpDeviceFlags) -> O,
    >(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O {
        let Self { config, core_ctx } = self;
        let mut state = crate::device::integration::ip_device_state(core_ctx, device_id);
        let mut flags = state.lock::<crate::lock_ordering::IpDeviceFlags<I>>();
        cb(*config, &mut *flags)
    }
}

impl<'a, BC: BindingsContext> device::WithIpv6DeviceConfigurationMutInner<BC>
    for CoreCtxWithIpDeviceConfiguration<
        'a,
        &mut Ipv6DeviceConfiguration,
        WrapLockLevel<crate::lock_ordering::IpDeviceConfiguration<Ipv6>>,
        BC,
    >
{
    type Ipv6DeviceStateCtx<'s>
        = CoreCtxWithIpDeviceConfiguration<
        's,
        &'s Ipv6DeviceConfiguration,
        WrapLockLevel<crate::lock_ordering::IpDeviceConfiguration<Ipv6>>,
        BC,
    >
    where
        Self: 's;

    fn ipv6_device_configuration_and_ctx(
        &mut self,
    ) -> (&Ipv6DeviceConfiguration, Self::Ipv6DeviceStateCtx<'_>) {
        let Self { config, core_ctx } = self;
        let config = &**config;
        (config, CoreCtxWithIpDeviceConfiguration { config, core_ctx: core_ctx.as_owned() })
    }
}

impl<'a, Config, BC: BindingsContext, L> DeviceIdContext<AnyDevice>
    for CoreCtxWithIpDeviceConfiguration<'a, Config, L, BC>
{
    type DeviceId = <CoreCtx<'a, BC, L> as DeviceIdContext<AnyDevice>>::DeviceId;
    type WeakDeviceId = <CoreCtx<'a, BC, L> as DeviceIdContext<AnyDevice>>::WeakDeviceId;
}

impl<'a, Config: Borrow<Ipv6DeviceConfiguration>, BC: BindingsContext> SlaacContext<BC>
    for CoreCtxWithIpDeviceConfiguration<
        'a,
        Config,
        WrapLockLevel<crate::lock_ordering::IpDeviceConfiguration<Ipv6>>,
        BC,
    >
{
    type LinkLayerAddr = <Self as device::Ipv6DeviceContext<BC>>::LinkLayerAddr;

    type SlaacAddrs<'s> = SlaacAddrs<'s, BC>;

    fn with_slaac_addrs_mut_and_configs<
        O,
        F: FnOnce(
            &mut Self::SlaacAddrs<'_>,
            SlaacConfigAndState<Self::LinkLayerAddr, BC>,
            &mut SlaacState<BC>,
        ) -> O,
    >(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O {
        let Self { config, core_ctx } = self;
        let retrans_timer = get_retrans_timer(core_ctx, device_id).get();
        // We use the link-layer address to derive opaque IIDs for the interface, rather
        // than the interface ID or name, because it is more stable. This does imply
        // that we do not generate SLAAC addresses for interfaces without a link-layer
        // address (e.g. loopback and pure IP devices); we could revisit this in the
        // future if desired.
        let link_layer_addr = device::Ipv6DeviceContext::get_link_layer_addr(core_ctx, device_id);

        let config = Borrow::borrow(config);
        let Ipv6DeviceConfiguration {
            max_router_solicitations: _,
            slaac_config,
            route_discovery_config: _,
            ip_config:
                IpDeviceConfiguration {
                    unicast_forwarding_enabled: _,
                    multicast_forwarding_enabled: _,
                    gmp_enabled: _,
                    dad_transmits,
                },
        } = *config;

        let ipv6_state = &core_ctx.unlocked_access::<crate::lock_ordering::UnlockedState>().ipv6;
        let stable_secret_key = ipv6_state.slaac_stable_secret_key;
        let temp_secret_key = ipv6_state.slaac_temp_secret_key;
        let mut core_ctx_and_resource =
            crate::device::integration::ip_device_state_and_core_ctx(core_ctx, device_id);
        let (mut state, mut locked) = core_ctx_and_resource
            .lock_with_and::<crate::lock_ordering::Ipv6DeviceSlaac, _>(|x| x.right());
        let core_ctx =
            CoreCtxWithIpDeviceConfiguration { config, core_ctx: locked.cast_core_ctx() };

        let mut addrs = SlaacAddrs { core_ctx, device_id: device_id.clone(), config };

        cb(
            &mut addrs,
            SlaacConfigAndState {
                config: slaac_config,
                dad_transmits,
                retrans_timer,
                link_layer_addr,
                temp_secret_key,
                stable_secret_key,
                _marker: PhantomData,
            },
            &mut state,
        )
    }
}

/// Returns `Some` if the provided device supports the ARP protocol.
fn into_arp_compatible_device<BT: BindingsTypes>(
    device_id: &DeviceId<BT>,
) -> Option<&EthernetDeviceId<BT>> {
    match device_id {
        DeviceId::Loopback(_) | DeviceId::PureIp(_) | DeviceId::Blackhole(_) => None,
        // At the moment, Ethernet is the only device type that supports ARP.
        // However, in the future that may change as we introduce new device
        // types (e.g. Token Ring devices as specified in IEEE 802.5).
        DeviceId::Ethernet(id) => Some(id),
    }
}

impl<BC: BindingsContext, L: LockBefore<crate::lock_ordering::IpDeviceAddressData<Ipv4>>>
    DadAddressContext<Ipv4, BC>
    for CoreCtxWithIpDeviceConfiguration<'_, &'_ Ipv4DeviceConfiguration, L, BC>
{
    fn with_address_assigned<O, F: FnOnce(&mut bool) -> O>(
        &mut self,
        _: &Self::DeviceId,
        addr: &Self::AddressId,
        cb: F,
    ) -> O {
        let mut locked = self.core_ctx.adopt(addr.deref());
        let mut state = locked
            .write_lock_with::<crate::lock_ordering::IpDeviceAddressData<Ipv4>, _>(|c| c.right());
        let IpAddressData { flags: IpAddressFlags { assigned }, config: _ } = &mut *state;

        cb(assigned)
    }

    fn should_perform_dad(&mut self, device: &Self::DeviceId, addr: &Self::AddressId) -> bool {
        // NB: DAD can only be performed for IPv4 addresses on devices that
        // support ARP. Short circuit for devices that are unsupported.
        if into_arp_compatible_device(device).is_none() {
            return false;
        }

        let mut locked = self.core_ctx.adopt(addr.deref());
        let state = locked
            .write_lock_with::<crate::lock_ordering::IpDeviceAddressData<Ipv4>, _>(|c| c.right());
        state.should_perform_dad()
    }
}

impl<BC: BindingsContext, L: LockBefore<crate::lock_ordering::IpDeviceAddressData<Ipv6>>>
    DadAddressContext<Ipv6, BC>
    for CoreCtxWithIpDeviceConfiguration<'_, &'_ Ipv6DeviceConfiguration, L, BC>
{
    fn with_address_assigned<O, F: FnOnce(&mut bool) -> O>(
        &mut self,
        _: &Self::DeviceId,
        addr: &Self::AddressId,
        cb: F,
    ) -> O {
        let mut locked = self.core_ctx.adopt(addr.deref());
        let mut state = locked
            .write_lock_with::<crate::lock_ordering::IpDeviceAddressData<Ipv6>, _>(|c| c.right());
        let IpAddressData { flags: IpAddressFlags { assigned }, config: _ } = &mut *state;

        cb(assigned)
    }

    fn should_perform_dad(&mut self, _: &Self::DeviceId, addr: &Self::AddressId) -> bool {
        let mut locked = self.core_ctx.adopt(addr.deref());
        let state = locked
            .write_lock_with::<crate::lock_ordering::IpDeviceAddressData<Ipv6>, _>(|c| c.right());
        state.should_perform_dad()
    }
}

impl<BC: BindingsContext, L: LockBefore<crate::lock_ordering::IpDeviceGmp<Ipv6>>>
    Ipv6DadAddressContext<BC>
    for CoreCtxWithIpDeviceConfiguration<'_, &'_ Ipv6DeviceConfiguration, L, BC>
{
    fn join_multicast_group(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        multicast_addr: MulticastAddr<Ipv6Addr>,
    ) {
        let Self { config, core_ctx } = self;
        let config = Borrow::borrow(&*config);
        join_ip_multicast_with_config(
            &mut CoreCtxWithIpDeviceConfiguration { config, core_ctx: core_ctx.as_owned() },
            bindings_ctx,
            device_id,
            multicast_addr,
            config,
        )
    }

    fn leave_multicast_group(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        multicast_addr: MulticastAddr<Ipv6Addr>,
    ) {
        let Self { config, core_ctx } = self;
        let config = Borrow::borrow(&*config);
        leave_ip_multicast_with_config(
            &mut CoreCtxWithIpDeviceConfiguration { config, core_ctx: core_ctx.as_owned() },
            bindings_ctx,
            device_id,
            multicast_addr,
            config,
        )
    }
}

impl<
    'a,
    Config: Borrow<Ipv4DeviceConfiguration>,
    BC: BindingsContext,
    L: LockBefore<crate::lock_ordering::IpDeviceAddressDad<Ipv4>>,
> DadContext<Ipv4, BC> for CoreCtxWithIpDeviceConfiguration<'a, Config, L, BC>
{
    type DadAddressCtx<'b> = CoreCtxWithIpDeviceConfiguration<
        'b,
        &'b Ipv4DeviceConfiguration,
        WrapLockLevel<crate::lock_ordering::IpDeviceAddressDad<Ipv4>>,
        BC,
    >;

    fn with_dad_state<O, F: FnOnce(DadStateRef<'_, Ipv4, Self::DadAddressCtx<'_>, BC>) -> O>(
        &mut self,
        _device_id: &Self::DeviceId,
        addr: &Self::AddressId,
        cb: F,
    ) -> O {
        let Self { config, core_ctx } = self;
        let mut core_ctx = core_ctx.adopt(addr.deref());
        let config = Borrow::borrow(&*config);

        let (mut dad_state, mut locked) = core_ctx
            .lock_with_and::<crate::lock_ordering::IpDeviceAddressDad<Ipv4>, _>(|c| c.right());
        let mut core_ctx =
            CoreCtxWithIpDeviceConfiguration { config, core_ctx: locked.cast_core_ctx() };

        cb(DadStateRef {
            state: DadAddressStateRef { dad_state: dad_state.deref_mut(), core_ctx: &mut core_ctx },
            // We don't honor the user-provided retrans-timer value for IPv4 DAD,
            // as per https://datatracker.ietf.org/doc/html/rfc5227#section-1.1:
            //
            //  Note that the values listed here are fixed constants; they
            //  are not intended to be modifiable by implementers, operators, or end
            //  users.
            retrans_timer_data: &(),
            max_dad_transmits: &config.ip_config.dad_transmits,
        })
    }

    fn send_dad_probe(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        data: Ipv4DadSendData,
    ) {
        // NB: Safe to `unwrap` here because DAD is disabled for IPv4 addresses
        // on devices that don't support ARP. See the implementation of
        // [`DadAddressContext::should_perform_dad`] for IPv4.
        let device_id = into_arp_compatible_device(device_id).unwrap_or_else(|| {
            panic!("shouldn't run IPv4 DAD on devices that don't support ARP. dev={device_id:?}")
        });

        // As per RFC 5227 Section 2.1.1:
        //   A host probes to see if an address is already in use by broadcasting
        //   an ARP Request for the desired address.  The client MUST fill in the
        //  'sender hardware address' field of the ARP Request with the hardware
        //   address of the interface through which it is sending the packet.
        //   [...]
        //   The 'target hardware address' field is ignored and SHOULD be set to
        //   all zeroes.
        //
        // Setting the `target_link_addr` to `None` causes `send_arp_request` to
        // 1) broadcast the request, and 2) set the target hardware address to
        // all 0s.
        let target_link_addr = None;

        let (sender_ip, target_ip) = data.into_sender_and_target_addr();

        let Self { config: _, core_ctx } = self;
        netstack3_device::send_arp_request(
            core_ctx,
            bindings_ctx,
            device_id,
            sender_ip,
            target_ip,
            target_link_addr,
        )
    }
}

fn get_retrans_timer<
    'a,
    BC: BindingsContext,
    L: LockBefore<crate::lock_ordering::IpDeviceAddresses<Ipv6>>,
>(
    core_ctx: &mut CoreCtx<'a, BC, L>,
    device_id: &DeviceId<BC>,
) -> NonZeroDuration {
    device::Ipv6DeviceContext::<BC>::with_network_learned_parameters(core_ctx, device_id, |p| {
        p.retrans_timer()
    })
    .unwrap_or_else(|| match device_id {
        DeviceId::Ethernet(base_device_id) => {
            crate::device::integration::device_state(core_ctx, base_device_id)
                .read_lock::<crate::lock_ordering::NudConfig<Ipv6>>()
                .retrans_timer
        }
        DeviceId::Loopback(_) | DeviceId::PureIp(_) | DeviceId::Blackhole(_) => {
            nud::DEFAULT_RETRANS_TIMER
        }
    })
}

impl<
    'a,
    Config: Borrow<Ipv6DeviceConfiguration>,
    BC: BindingsContext,
    L: LockBefore<crate::lock_ordering::IpDeviceAddressDad<Ipv6>>,
> DadContext<Ipv6, BC> for CoreCtxWithIpDeviceConfiguration<'a, Config, L, BC>
{
    type DadAddressCtx<'b> = CoreCtxWithIpDeviceConfiguration<
        'b,
        &'b Ipv6DeviceConfiguration,
        WrapLockLevel<crate::lock_ordering::IpDeviceAddressDad<Ipv6>>,
        BC,
    >;

    fn with_dad_state<O, F: FnOnce(DadStateRef<'_, Ipv6, Self::DadAddressCtx<'_>, BC>) -> O>(
        &mut self,
        device_id: &Self::DeviceId,
        addr: &Self::AddressId,
        cb: F,
    ) -> O {
        let Self { config, core_ctx } = self;
        let retrans_timer = get_retrans_timer(core_ctx, device_id);

        let mut core_ctx = core_ctx.adopt(addr.deref());
        let config = Borrow::borrow(&*config);

        let (mut dad_state, mut locked) = core_ctx
            .lock_with_and::<crate::lock_ordering::IpDeviceAddressDad<Ipv6>, _>(|c| c.right());
        let mut core_ctx =
            CoreCtxWithIpDeviceConfiguration { config, core_ctx: locked.cast_core_ctx() };

        cb(DadStateRef {
            state: DadAddressStateRef { dad_state: dad_state.deref_mut(), core_ctx: &mut core_ctx },
            retrans_timer_data: &retrans_timer,
            max_dad_transmits: &config.ip_config.dad_transmits,
        })
    }
    /// Sends an NDP Neighbor Solicitation message for DAD to the local-link.
    ///
    /// The message will be sent with the unspecified (all-zeroes) source
    /// address.
    fn send_dad_probe(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        Ipv6DadSendData { dst_ip, message, nonce }: Ipv6DadSendData,
    ) {
        // Do not include the source link-layer option when the NS
        // message as DAD messages are sent with the unspecified source
        // address which must not hold a source link-layer option.
        //
        // As per RFC 4861 section 4.3,
        //
        //   Possible options:
        //
        //      Source link-layer address
        //           The link-layer address for the sender. MUST NOT be
        //           included when the source IP address is the
        //           unspecified address. Otherwise, on link layers
        //           that have addresses this option MUST be included in
        //           multicast solicitations and SHOULD be included in
        //           unicast solicitations.
        let src_ip = None;
        let options = [NdpOptionBuilder::Nonce(NdpNonce::from(&nonce))];

        let result = ip::icmp::send_ndp_packet(
            self,
            bindings_ctx,
            device_id,
            src_ip,
            dst_ip.into_specified(),
            OptionSequenceBuilder::new(options.iter()).into_serializer(),
            ip::icmp::NdpMessage::NeighborSolicitation { message, code: IcmpZeroCode },
        );
        match result {
            Ok(()) => {}
            Err(IpSendFrameError { serializer: _, error }) => {
                // TODO(https://fxbug.dev/42165912): Either panic or guarantee
                // that this error can't happen statically.
                debug!("error sending DAD packet: {error:?}")
            }
        }
    }
}

impl<'a, Config: Borrow<Ipv6DeviceConfiguration>, BC: BindingsContext> RsContext<BC>
    for CoreCtxWithIpDeviceConfiguration<
        'a,
        Config,
        WrapLockLevel<crate::lock_ordering::IpDeviceConfiguration<Ipv6>>,
        BC,
    >
{
    type LinkLayerAddr = <CoreCtx<
        'a,
        BC,
        WrapLockLevel<crate::lock_ordering::IpDeviceConfiguration<Ipv6>>,
    > as device::Ipv6DeviceContext<BC>>::LinkLayerAddr;

    fn with_rs_state_mut_and_max<O, F: FnOnce(&mut RsState<BC>, Option<NonZeroU8>) -> O>(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O {
        let Self { config, core_ctx } = self;
        let mut state = crate::device::integration::ip_device_state(core_ctx, device_id);
        let mut state = state.lock::<crate::lock_ordering::Ipv6DeviceRouterSolicitations>();
        cb(&mut state, Borrow::borrow(&*config).max_router_solicitations)
    }

    fn get_link_layer_addr(&mut self, device_id: &Self::DeviceId) -> Option<Self::LinkLayerAddr> {
        let Self { config: _, core_ctx } = self;
        device::Ipv6DeviceContext::get_link_layer_addr(core_ctx, device_id)
    }

    fn send_rs_packet<
        S: NetworkSerializer<Buffer = EmptyBuf> + NetworkPartialSerializer,
        F: FnOnce(Option<UnicastAddr<Ipv6Addr>>) -> S,
    >(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        message: RouterSolicitation,
        body: F,
    ) -> Result<(), IpSendFrameError<S>> {
        let Self { config: _, core_ctx } = self;

        let dst_ip = Ipv6::ALL_ROUTERS_LINK_LOCAL_MULTICAST_ADDRESS.into_specified();
        let src_ip = ip::IpSasHandler::<Ipv6, _>::get_local_addr_for_remote(
            core_ctx,
            device_id,
            Some(dst_ip),
        )
        .and_then(|addr| UnicastAddr::new(addr.addr()));
        ip::icmp::send_ndp_packet(
            core_ctx,
            bindings_ctx,
            device_id,
            src_ip.map(UnicastAddr::into_specified),
            dst_ip,
            body(src_ip),
            ip::icmp::NdpMessage::RouterSolicitation { message, code: IcmpZeroCode },
        )
    }
}

impl<
    I: IpExt,
    Config,
    BC: BindingsContext,
    L: LockBefore<crate::lock_ordering::EthernetDeviceDynamicState>,
> ip::IpDeviceMtuContext<I> for CoreCtxWithIpDeviceConfiguration<'_, Config, L, BC>
{
    fn get_mtu(&mut self, device_id: &Self::DeviceId) -> Mtu {
        ip::IpDeviceMtuContext::<I>::get_mtu(&mut self.core_ctx, device_id)
    }
}

impl<L, BT: BindingsTypes> CoreTimerContext<RsTimerId<WeakDeviceId<BT>>, BT>
    for CoreCtx<'_, BT, L>
{
    fn convert_timer(dispatch_id: RsTimerId<WeakDeviceId<BT>>) -> BT::DispatchId {
        IpDeviceTimerId::<Ipv6, _, _>::from(Ipv6DeviceTimerId::from(dispatch_id)).into()
    }
}

impl<BC: BindingsContext> Ipv6DiscoveredRoutesContext<BC>
    for CoreCtx<'_, BC, WrapLockLevel<crate::lock_ordering::Ipv6DeviceRouteDiscovery>>
{
    fn add_discovered_ipv6_route(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        Ipv6DiscoveredRoute { subnet, gateway }: Ipv6DiscoveredRoute,
        Ipv6DiscoveredRouteProperties { route_preference }: Ipv6DiscoveredRouteProperties,
    ) {
        let device_id = device_id.clone();
        let entry = ip::AddableEntry {
            subnet,
            device: device_id,
            gateway: gateway.map(|g| (*g).into_specified()),
            metric: AddableMetric::MetricTracksInterface,
            route_preference,
        };

        ip::request_context_add_route::<Ipv6, _, _>(bindings_ctx, entry);
    }

    fn del_discovered_ipv6_route(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        Ipv6DiscoveredRoute { subnet, gateway }: Ipv6DiscoveredRoute,
    ) {
        ip::request_context_del_routes::<Ipv6, _, _>(
            bindings_ctx,
            subnet,
            device_id.clone(),
            gateway.map(|g| (*g).into_specified()),
        );
    }
}

impl<'a, Config, BC: BindingsContext> Ipv6RouteDiscoveryContext<BC>
    for CoreCtxWithIpDeviceConfiguration<
        'a,
        Config,
        WrapLockLevel<crate::lock_ordering::IpDeviceConfiguration<Ipv6>>,
        BC,
    >
{
    type WithDiscoveredRoutesMutCtx<'b> =
        CoreCtx<'b, BC, WrapLockLevel<crate::lock_ordering::Ipv6DeviceRouteDiscovery>>;

    fn with_discovered_routes_mut<
        O,
        F: FnOnce(&mut Ipv6RouteDiscoveryState<BC>, &mut Self::WithDiscoveredRoutesMutCtx<'_>) -> O,
    >(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O {
        let Self { config: _, core_ctx } = self;
        let mut core_ctx_and_resource =
            crate::device::integration::ip_device_state_and_core_ctx(core_ctx, device_id);

        let (mut state, mut locked) =
            core_ctx_and_resource
                .lock_with_and::<crate::lock_ordering::Ipv6DeviceRouteDiscovery, _>(|x| x.right());
        cb(&mut state, &mut locked.cast_core_ctx())
    }
}

impl<'a, Config, BC: BindingsContext> device::Ipv6DeviceContext<BC>
    for CoreCtxWithIpDeviceConfiguration<
        'a,
        Config,
        WrapLockLevel<crate::lock_ordering::IpDeviceConfiguration<Ipv6>>,
        BC,
    >
{
    type LinkLayerAddr = <CoreCtx<
        'a,
        BC,
        WrapLockLevel<crate::lock_ordering::IpDeviceConfiguration<Ipv6>>,
    > as device::Ipv6DeviceContext<BC>>::LinkLayerAddr;

    fn get_link_layer_addr(&mut self, device_id: &Self::DeviceId) -> Option<Self::LinkLayerAddr> {
        let Self { config: _, core_ctx } = self;
        device::Ipv6DeviceContext::get_link_layer_addr(core_ctx, device_id)
    }

    fn set_link_mtu(&mut self, device_id: &Self::DeviceId, mtu: Mtu) {
        let Self { config: _, core_ctx } = self;
        device::Ipv6DeviceContext::set_link_mtu(core_ctx, device_id, mtu)
    }

    fn with_network_learned_parameters<O, F: FnOnce(&Ipv6NetworkLearnedParameters) -> O>(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O {
        let Self { config: _, core_ctx } = self;
        device::Ipv6DeviceContext::with_network_learned_parameters(core_ctx, device_id, cb)
    }

    fn with_network_learned_parameters_mut<O, F: FnOnce(&mut Ipv6NetworkLearnedParameters) -> O>(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O {
        let Self { config: _, core_ctx } = self;
        device::Ipv6DeviceContext::with_network_learned_parameters_mut(core_ctx, device_id, cb)
    }
}

impl<'a, Config, I: IpDeviceIpExt, L, BC: BindingsContext> IpDeviceAddressIdContext<I>
    for CoreCtxWithIpDeviceConfiguration<'a, Config, L, BC>
where
    CoreCtx<'a, BC, L>: IpDeviceAddressIdContext<I>,
{
    type AddressId = <CoreCtx<'a, BC, L> as IpDeviceAddressIdContext<I>>::AddressId;
    type WeakAddressId = <CoreCtx<'a, BC, L> as IpDeviceAddressIdContext<I>>::WeakAddressId;
}

impl<'a, Config, I: IpDeviceIpExt, BC: BindingsContext, L> device::IpDeviceAddressContext<I, BC>
    for CoreCtxWithIpDeviceConfiguration<'a, Config, L, BC>
where
    CoreCtx<'a, BC, L>: device::IpDeviceAddressContext<I, BC>,
{
    fn with_ip_address_data<O, F: FnOnce(&IpAddressData<I, BC::Instant>) -> O>(
        &mut self,
        device_id: &Self::DeviceId,
        addr_id: &Self::AddressId,
        cb: F,
    ) -> O {
        let Self { config: _, core_ctx } = self;
        device::IpDeviceAddressContext::<I, BC>::with_ip_address_data(
            core_ctx, device_id, addr_id, cb,
        )
    }

    fn with_ip_address_data_mut<O, F: FnOnce(&mut IpAddressData<I, BC::Instant>) -> O>(
        &mut self,
        device_id: &Self::DeviceId,
        addr_id: &Self::AddressId,
        cb: F,
    ) -> O {
        let Self { config: _, core_ctx } = self;
        device::IpDeviceAddressContext::<I, BC>::with_ip_address_data_mut(
            core_ctx, device_id, addr_id, cb,
        )
    }
}

impl<'a, Config, I: IpDeviceIpExt, BC: BindingsContext, L> device::IpDeviceAddAddressContext<I, BC>
    for CoreCtxWithIpDeviceConfiguration<'a, Config, L, BC>
where
    CoreCtx<'a, BC, L>: device::IpDeviceAddAddressContext<I, BC>,
{
    fn add_ip_address(
        &mut self,
        device_id: &Self::DeviceId,
        addr: AddrSubnet<I::Addr, I::AssignedWitness>,
        config: I::AddressConfig<BC::Instant>,
    ) -> Result<Self::AddressId, ExistsError> {
        let Self { config: _, core_ctx } = self;
        device::IpDeviceAddAddressContext::<I, BC>::add_ip_address(
            core_ctx, device_id, addr, config,
        )
    }
}

impl<'a, Config, I: IpDeviceIpExt, BC: BindingsContext, L> device::IpDeviceStateContext<I, BC>
    for CoreCtxWithIpDeviceConfiguration<'a, Config, L, BC>
where
    CoreCtx<'a, BC, L>: device::IpDeviceStateContext<I, BC>,
{
    type IpDeviceAddressCtx<'b> =
        <CoreCtx<'a, BC, L> as device::IpDeviceStateContext<I, BC>>::IpDeviceAddressCtx<'b>;

    fn with_ip_device_flags<O, F: FnOnce(&IpDeviceFlags) -> O>(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O {
        let Self { config: _, core_ctx } = self;
        device::IpDeviceStateContext::<I, BC>::with_ip_device_flags(core_ctx, device_id, cb)
    }

    fn remove_ip_address(
        &mut self,
        device_id: &Self::DeviceId,
        addr: Self::AddressId,
    ) -> RemoveResourceResultWithContext<AddrSubnet<I::Addr>, BC> {
        let Self { config: _, core_ctx } = self;
        device::IpDeviceStateContext::<I, BC>::remove_ip_address(core_ctx, device_id, addr)
    }

    fn get_address_id(
        &mut self,
        device_id: &Self::DeviceId,
        addr: SpecifiedAddr<I::Addr>,
    ) -> Result<Self::AddressId, NotFoundError> {
        let Self { config: _, core_ctx } = self;
        device::IpDeviceStateContext::<I, BC>::get_address_id(core_ctx, device_id, addr)
    }

    type AddressIdsIter<'b> =
        <CoreCtx<'a, BC, L> as device::IpDeviceStateContext<I, BC>>::AddressIdsIter<'b>;
    fn with_address_ids<
        O,
        F: FnOnce(Self::AddressIdsIter<'_>, &mut Self::IpDeviceAddressCtx<'_>) -> O,
    >(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O {
        let Self { config: _, core_ctx } = self;
        device::IpDeviceStateContext::<I, BC>::with_address_ids(core_ctx, device_id, cb)
    }

    fn with_default_hop_limit<O, F: FnOnce(&NonZeroU8) -> O>(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O {
        let Self { config: _, core_ctx } = self;
        device::IpDeviceStateContext::<I, BC>::with_default_hop_limit(core_ctx, device_id, cb)
    }

    fn with_default_hop_limit_mut<O, F: FnOnce(&mut NonZeroU8) -> O>(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O {
        let Self { config: _, core_ctx } = self;
        device::IpDeviceStateContext::<I, BC>::with_default_hop_limit_mut(core_ctx, device_id, cb)
    }

    fn join_link_multicast_group(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        multicast_addr: MulticastAddr<I::Addr>,
    ) {
        let Self { config: _, core_ctx } = self;
        device::IpDeviceStateContext::<I, BC>::join_link_multicast_group(
            core_ctx,
            bindings_ctx,
            device_id,
            multicast_addr,
        )
    }

    fn leave_link_multicast_group(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        multicast_addr: MulticastAddr<I::Addr>,
    ) {
        let Self { config: _, core_ctx } = self;
        device::IpDeviceStateContext::<I, BC>::leave_link_multicast_group(
            core_ctx,
            bindings_ctx,
            device_id,
            multicast_addr,
        )
    }
}

impl<BC: BindingsContext, Config, L> IgmpContextMarker
    for CoreCtxWithIpDeviceConfiguration<'_, Config, L, BC>
{
}

impl<'a, Config: Borrow<Ipv4DeviceConfiguration>, BC: BindingsContext> IgmpContext<BC>
    for CoreCtxWithIpDeviceConfiguration<
        'a,
        Config,
        WrapLockLevel<crate::lock_ordering::IpDeviceConfiguration<Ipv4>>,
        BC,
    >
{
    type SendContext<'b> = CoreCtx<'b, BC, WrapLockLevel<crate::lock_ordering::IpDeviceGmp<Ipv4>>>;

    /// Calls the function with a mutable reference to the device's IGMP state
    /// and whether or not IGMP is enabled for the `device`.
    fn with_igmp_state_mut<
        O,
        F: for<'b> FnOnce(Self::SendContext<'b>, GmpStateRef<'b, Ipv4, IgmpTypeLayout, BC>) -> O,
    >(
        &mut self,
        device: &Self::DeviceId,
        cb: F,
    ) -> O {
        let Self { config, core_ctx } = self;
        let Ipv4DeviceConfiguration { ip_config: IpDeviceConfiguration { gmp_enabled, .. } } =
            Borrow::borrow(&*config);

        let mut state = crate::device::integration::ip_device_state_and_core_ctx(core_ctx, device);
        // Note that changes to `ip_enabled` is not possible in this context
        // since IP enabled changes are only performed while the IP device
        // configuration lock is held exclusively. Since we have access to
        // the IP device configuration here (`config`), we know changes to
        // IP enabled are not possible.
        let ip_enabled = state
            .lock_with::<crate::lock_ordering::IpDeviceFlags<Ipv4>, _>(|x| x.right())
            .ip_enabled;
        let (mut state, mut locked) =
            state.write_lock_with_and::<crate::lock_ordering::IpDeviceGmp<Ipv4>, _>(|x| x.right());
        let IpDeviceMulticastGroups { groups, gmp, gmp_config } = &mut *state;
        let enabled = ip_enabled && *gmp_enabled;
        cb(locked.cast_core_ctx(), GmpStateRef { enabled, groups, gmp, config: gmp_config })
    }
}

impl<'a, BC: BindingsContext> IgmpSendContext<BC>
    for CoreCtx<'a, BC, WrapLockLevel<crate::lock_ordering::IpDeviceGmp<Ipv4>>>
{
    fn get_ip_addr_subnet(
        &mut self,
        device: &Self::DeviceId,
    ) -> Option<AddrSubnet<Ipv4Addr, Ipv4DeviceAddr>> {
        ip::device::get_ipv4_addr_subnet(self, device)
    }
}

impl<BC: BindingsContext, Config, L> MldContextMarker
    for CoreCtxWithIpDeviceConfiguration<'_, Config, L, BC>
{
}

impl<
    'a,
    Config: Borrow<Ipv6DeviceConfiguration>,
    BC: BindingsContext,
    L: LockBefore<crate::lock_ordering::IpDeviceGmp<Ipv6>>,
> MldContext<BC> for CoreCtxWithIpDeviceConfiguration<'a, Config, L, BC>
{
    type SendContext<'b> = CoreCtx<'b, BC, WrapLockLevel<crate::lock_ordering::IpDeviceGmp<Ipv6>>>;

    fn with_mld_state_mut<
        O,
        F: FnOnce(Self::SendContext<'_>, GmpStateRef<'_, Ipv6, MldTypeLayout, BC>) -> O,
    >(
        &mut self,
        device: &Self::DeviceId,
        cb: F,
    ) -> O {
        let Self { config, core_ctx } = self;
        let Ipv6DeviceConfiguration {
            max_router_solicitations: _,
            slaac_config: _,
            route_discovery_config: _,
            ip_config: IpDeviceConfiguration { gmp_enabled, .. },
        } = Borrow::borrow(&*config);

        let mut state = crate::device::integration::ip_device_state_and_core_ctx(core_ctx, device);
        let ip_enabled = state
            .lock_with::<crate::lock_ordering::IpDeviceFlags<Ipv6>, _>(|x| x.right())
            .ip_enabled;
        let (mut state, mut locked) =
            state.write_lock_with_and::<crate::lock_ordering::IpDeviceGmp<Ipv6>, _>(|x| x.right());
        let IpDeviceMulticastGroups { groups, gmp, gmp_config } = &mut *state;
        let enabled = ip_enabled && *gmp_enabled;
        cb(locked.cast_core_ctx(), GmpStateRef { enabled, groups, gmp, config: gmp_config })
    }
}

impl<'a, BC: BindingsContext> MldSendContext<BC>
    for CoreCtx<'a, BC, WrapLockLevel<crate::lock_ordering::IpDeviceGmp<Ipv6>>>
{
    fn get_ipv6_link_local_addr(
        &mut self,
        device: &Self::DeviceId,
    ) -> Option<LinkLocalUnicastAddr<Ipv6Addr>> {
        device::IpDeviceStateContext::<Ipv6, BC>::with_address_ids(
            self,
            device,
            |mut addrs, core_ctx| {
                addrs.find_map(|addr_id| {
                    device::IpDeviceAddressContext::<Ipv6, _>::with_ip_address_data(
                        core_ctx,
                        device,
                        &addr_id,
                        |IpAddressData { flags: IpAddressFlags { assigned }, config: _ }| {
                            if *assigned {
                                LinkLocalUnicastAddr::new(addr_id.addr_sub().addr().get())
                            } else {
                                None
                            }
                        },
                    )
                })
            },
        )
    }
}

impl<'a, Config, I: IpDeviceIpExt, BC: BindingsContext, L> NudIpHandler<I, BC>
    for CoreCtxWithIpDeviceConfiguration<'a, Config, L, BC>
where
    CoreCtx<'a, BC, L>: NudIpHandler<I, BC>,
{
    fn handle_neighbor_probe(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        neighbor: SpecifiedAddr<I::Addr>,
        link_addr: &[u8],
    ) {
        let Self { config: _, core_ctx } = self;
        NudIpHandler::<I, BC>::handle_neighbor_probe(
            core_ctx,
            bindings_ctx,
            device_id,
            neighbor,
            link_addr,
        )
    }

    fn handle_neighbor_confirmation(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        neighbor: SpecifiedAddr<I::Addr>,
        link_addr: Option<&[u8]>,
        flags: ConfirmationFlags,
    ) {
        let Self { config: _, core_ctx } = self;
        NudIpHandler::<I, BC>::handle_neighbor_confirmation(
            core_ctx,
            bindings_ctx,
            device_id,
            neighbor,
            link_addr,
            flags,
        )
    }

    fn flush_neighbor_table(&mut self, bindings_ctx: &mut BC, device_id: &Self::DeviceId) {
        let Self { config: _, core_ctx } = self;
        NudIpHandler::<I, BC>::flush_neighbor_table(core_ctx, bindings_ctx, device_id)
    }
}

#[netstack3_macros::instantiate_ip_impl_block(I)]
impl<'a, I: IpExt, Config, BC: BindingsContext, L: LockBefore<crate::lock_ordering::FilterState<I>>>
    FilterHandlerProvider<I, BC> for CoreCtxWithIpDeviceConfiguration<'a, Config, L, BC>
{
    type Handler<'b>
        = FilterImpl<'b, CoreCtx<'a, BC, L>>
    where
        Self: 'b;

    fn filter_handler(&mut self) -> Self::Handler<'_> {
        let Self { config: _, core_ctx } = self;
        FilterHandlerProvider::<I, BC>::filter_handler(core_ctx)
    }
}

#[netstack3_macros::instantiate_ip_impl_block(I)]
impl<
    'a,
    I: IpLayerIpExt,
    Config,
    BC: BindingsContext,
    L: LockBefore<crate::lock_ordering::IpDeviceGmp<I>>,
> IpDeviceEgressStateContext<I> for CoreCtxWithIpDeviceConfiguration<'a, Config, L, BC>
{
    fn with_next_packet_id<O, F: FnOnce(&<I as IpLayerIpExt>::PacketIdState) -> O>(
        &self,
        cb: F,
    ) -> O {
        let Self { config: _, core_ctx } = self;
        IpDeviceEgressStateContext::<I>::with_next_packet_id(core_ctx, cb)
    }

    fn get_local_addr_for_remote(
        &mut self,
        device_id: &Self::DeviceId,
        remote: Option<SpecifiedAddr<<I as Ip>::Addr>>,
    ) -> Option<IpDeviceAddr<<I as Ip>::Addr>> {
        let Self { config: _, core_ctx } = self;
        IpDeviceEgressStateContext::<I>::get_local_addr_for_remote(core_ctx, device_id, remote)
    }

    fn get_hop_limit(&mut self, device_id: &Self::DeviceId) -> NonZeroU8 {
        let Self { config: _, core_ctx } = self;
        IpDeviceEgressStateContext::<I>::get_hop_limit(core_ctx, device_id)
    }
}

#[netstack3_macros::instantiate_ip_impl_block(I)]
impl<
    'a,
    I: IpLayerIpExt,
    Config,
    BC: BindingsContext,
    L: LockBefore<crate::lock_ordering::IpDeviceGmp<I>>,
> IpDeviceIngressStateContext<I> for CoreCtxWithIpDeviceConfiguration<'a, Config, L, BC>
{
    fn address_status_for_device(
        &mut self,
        dst_ip: SpecifiedAddr<<I as Ip>::Addr>,
        device_id: &Self::DeviceId,
    ) -> AddressStatus<<I as IpLayerIpExt>::AddressStatus> {
        let Self { config: _, core_ctx } = self;
        IpDeviceIngressStateContext::<I>::address_status_for_device(core_ctx, dst_ip, device_id)
    }
}

impl<BC: BindingsContext, I: Ip, L> CounterContext<NudCounters<I>> for CoreCtx<'_, BC, L> {
    fn counters(&self) -> &NudCounters<I> {
        self.unlocked_access::<crate::lock_ordering::UnlockedState>().device.nud_counters::<I>()
    }
}

impl<L, BT: BindingsTypes>
    CoreTimerContext<DadTimerId<Ipv4, WeakDeviceId<BT>, WeakAddressId<Ipv4, BT>>, BT>
    for CoreCtx<'_, BT, L>
{
    fn convert_timer(
        dispatch_id: DadTimerId<Ipv4, WeakDeviceId<BT>, WeakAddressId<Ipv4, BT>>,
    ) -> BT::DispatchId {
        IpDeviceTimerId::<Ipv4, _, _>::from(Ipv4DeviceTimerId::from(dispatch_id)).into()
    }
}

impl<L, BT: BindingsTypes>
    CoreTimerContext<DadTimerId<Ipv6, WeakDeviceId<BT>, WeakAddressId<Ipv6, BT>>, BT>
    for CoreCtx<'_, BT, L>
{
    fn convert_timer(
        dispatch_id: DadTimerId<Ipv6, WeakDeviceId<BT>, WeakAddressId<Ipv6, BT>>,
    ) -> BT::DispatchId {
        IpDeviceTimerId::<Ipv6, _, _>::from(Ipv6DeviceTimerId::from(dispatch_id)).into()
    }
}

impl<I: IpDeviceIpExt, BT: BindingsTypes, L>
    CoreTimerContext<IpDeviceTimerId<I, WeakDeviceId<BT>, BT>, BT> for CoreCtx<'_, BT, L>
{
    fn convert_timer(dispatch_id: IpDeviceTimerId<I, WeakDeviceId<BT>, BT>) -> BT::DispatchId {
        dispatch_id.into()
    }
}

impl<I: IpDeviceStateIpExt, BT: IpDeviceStateBindingsTypes> LockLevelFor<DualStackIpDeviceState<BT>>
    for crate::lock_ordering::IpDeviceAddresses<I>
{
    type Data = IpDeviceAddresses<I, BT>;
}

impl<I: IpDeviceStateIpExt, BT: IpDeviceStateBindingsTypes> LockLevelFor<DualStackIpDeviceState<BT>>
    for crate::lock_ordering::IpDeviceGmp<I>
{
    type Data = IpDeviceMulticastGroups<I, BT>;
}

impl<I: IpDeviceStateIpExt, BT: IpDeviceStateBindingsTypes> LockLevelFor<DualStackIpDeviceState<BT>>
    for crate::lock_ordering::IpDeviceDefaultHopLimit<I>
{
    type Data = DefaultHopLimit<I>;
}

impl<I: IpDeviceStateIpExt, BT: IpDeviceStateBindingsTypes> LockLevelFor<DualStackIpDeviceState<BT>>
    for crate::lock_ordering::IpDeviceFlags<I>
{
    type Data = IpMarked<I, IpDeviceFlags>;
}

impl<BT: IpDeviceStateBindingsTypes> LockLevelFor<DualStackIpDeviceState<BT>>
    for crate::lock_ordering::Ipv6DeviceSlaac
{
    type Data = SlaacState<BT>;
}

/// It is safe to provide unlocked access to [`DualStackIpDeviceState`] itself
/// here because care has been taken to avoid exposing publicly to the core
/// integration crate any state that is held by a lock, as opposed to read-only
/// state that can be accessed safely at any lock level, e.g. state with no
/// interior mutability or atomics.
///
/// Access to state held by locks *must* be mediated using the global lock
/// ordering declared in [`crate::lock_ordering`].
impl<BT: IpDeviceStateBindingsTypes> UnlockedAccess<crate::lock_ordering::UnlockedState>
    for DualStackIpDeviceState<BT>
{
    type Data = DualStackIpDeviceState<BT>;
    type Guard<'l>
        = &'l DualStackIpDeviceState<BT>
    where
        Self: 'l;

    fn access(&self) -> Self::Guard<'_> {
        &self
    }
}

impl<BT: IpDeviceStateBindingsTypes> LockLevelFor<DualStackIpDeviceState<BT>>
    for crate::lock_ordering::IpDeviceConfiguration<Ipv4>
{
    type Data = Ipv4DeviceConfiguration;
}

impl<BT: IpDeviceStateBindingsTypes> LockLevelFor<DualStackIpDeviceState<BT>>
    for crate::lock_ordering::Ipv6DeviceLearnedParams
{
    type Data = Ipv6NetworkLearnedParameters;
}

impl<BT: IpDeviceStateBindingsTypes> LockLevelFor<DualStackIpDeviceState<BT>>
    for crate::lock_ordering::Ipv6DeviceRouteDiscovery
{
    type Data = Ipv6RouteDiscoveryState<BT>;
}

impl<BT: IpDeviceStateBindingsTypes> LockLevelFor<DualStackIpDeviceState<BT>>
    for crate::lock_ordering::Ipv6DeviceRouterSolicitations
{
    type Data = RsState<BT>;
}

impl<BT: IpDeviceStateBindingsTypes> LockLevelFor<DualStackIpDeviceState<BT>>
    for crate::lock_ordering::IpDeviceConfiguration<Ipv6>
{
    type Data = Ipv6DeviceConfiguration;
}

impl<BT: IpDeviceStateBindingsTypes> LockLevelFor<IpAddressEntry<Ipv4, BT>>
    for crate::lock_ordering::IpDeviceAddressDad<Ipv4>
{
    type Data = DadState<Ipv4, BT>;
}

impl<BT: IpDeviceStateBindingsTypes> LockLevelFor<IpAddressEntry<Ipv4, BT>>
    for crate::lock_ordering::IpDeviceAddressData<Ipv4>
{
    type Data = IpAddressData<Ipv4, BT::Instant>;
}

impl<BT: IpDeviceStateBindingsTypes> LockLevelFor<IpAddressEntry<Ipv6, BT>>
    for crate::lock_ordering::IpDeviceAddressDad<Ipv6>
{
    type Data = DadState<Ipv6, BT>;
}

impl<BT: IpDeviceStateBindingsTypes> LockLevelFor<IpAddressEntry<Ipv6, BT>>
    for crate::lock_ordering::IpDeviceAddressData<Ipv6>
{
    type Data = IpAddressData<Ipv6, BT::Instant>;
}

impl<BT: BindingsTypes, L> CounterContext<SlaacCounters> for CoreCtx<'_, BT, L> {
    fn counters(&self) -> &SlaacCounters {
        &self.unlocked_access::<crate::lock_ordering::UnlockedState>().ipv6.slaac_counters
    }
}
