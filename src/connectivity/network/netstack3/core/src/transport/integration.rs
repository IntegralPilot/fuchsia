// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use lock_order::lock::{DelegatedOrderedLockAccess, LockLevelFor};
use lock_order::relation::LockBefore;
use net_types::ip::{Ip, Ipv4, Ipv6};
use netstack3_base::socket::MaybeDualStack;
use netstack3_base::{
    CoreTimerContext, CounterContext, ResourceCounterContext, Uninstantiable,
    UninstantiableWrapper, WeakDeviceIdentifier,
};
use netstack3_datagram as datagram;
use netstack3_device::WeakDeviceId;
use netstack3_tcp::{
    self as tcp, IsnGenerator, TcpContext, TcpCountersWithSocket, TcpCountersWithoutSocket,
    TcpDemuxContext, TcpDualStackContext, TcpSocketId, TcpSocketSet, TcpSocketState, TcpState,
    TimestampOffsetGenerator, WeakTcpSocketId,
};
use netstack3_udp::{
    self as udp, UdpCountersWithSocket, UdpCountersWithoutSocket, UdpSocketId, UdpSocketSet,
    UdpSocketState,
};

use crate::context::WrapLockLevel;
use crate::context::prelude::*;
use crate::transport::TransportLayerTimerId;
use crate::{BindingsContext, BindingsTypes, CoreCtx, StackState};

impl<I, BC, L> CoreTimerContext<WeakTcpSocketId<I, WeakDeviceId<BC>, BC>, BC> for CoreCtx<'_, BC, L>
where
    I: tcp::DualStackIpExt,
    BC: BindingsContext,
{
    fn convert_timer(dispatch_id: WeakTcpSocketId<I, WeakDeviceId<BC>, BC>) -> BC::DispatchId {
        TransportLayerTimerId::Tcp(dispatch_id.into()).into()
    }
}

#[netstack3_macros::instantiate_ip_impl_block(I)]
impl<I, L, BC> TcpDemuxContext<I, WeakDeviceId<BC>, BC> for CoreCtx<'_, BC, L>
where
    I: Ip,
    BC: BindingsContext,
    L: LockBefore<crate::lock_ordering::TcpDemux<I>>,
{
    type IpTransportCtx<'a> = CoreCtx<'a, BC, WrapLockLevel<crate::lock_ordering::TcpDemux<I>>>;
    fn with_demux<O, F: FnOnce(&tcp::DemuxState<I, WeakDeviceId<BC>, BC>) -> O>(
        &mut self,
        cb: F,
    ) -> O {
        cb(&self.read_lock::<crate::lock_ordering::TcpDemux<I>>())
    }

    fn with_demux_mut<O, F: FnOnce(&mut tcp::DemuxState<I, WeakDeviceId<BC>, BC>) -> O>(
        &mut self,
        cb: F,
    ) -> O {
        cb(&mut self.write_lock::<crate::lock_ordering::TcpDemux<I>>())
    }
}

impl<L, BC> TcpContext<Ipv4, BC> for CoreCtx<'_, BC, L>
where
    BC: BindingsContext,
    L: LockBefore<crate::lock_ordering::TcpAllSocketsSet<Ipv4>>,
{
    type ThisStackIpTransportAndDemuxCtx<'a> =
        CoreCtx<'a, BC, WrapLockLevel<crate::lock_ordering::TcpSocketState<Ipv4>>>;
    type SingleStackIpTransportAndDemuxCtx<'a> =
        CoreCtx<'a, BC, WrapLockLevel<crate::lock_ordering::TcpSocketState<Ipv4>>>;

    type DualStackIpTransportAndDemuxCtx<'a> = UninstantiableWrapper<
        CoreCtx<'a, BC, WrapLockLevel<crate::lock_ordering::TcpSocketState<Ipv4>>>,
    >;

    type SingleStackConverter = ();
    type DualStackConverter = Uninstantiable;

    fn with_all_sockets_mut<O, F: FnOnce(&mut TcpSocketSet<Ipv4, Self::WeakDeviceId, BC>) -> O>(
        &mut self,
        cb: F,
    ) -> O {
        let mut all_sockets = self.write_lock::<crate::lock_ordering::TcpAllSocketsSet<Ipv4>>();
        cb(&mut *all_sockets)
    }

    fn for_each_socket<
        F: FnMut(
            &TcpSocketId<Ipv4, Self::WeakDeviceId, BC>,
            &TcpSocketState<Ipv4, Self::WeakDeviceId, BC>,
        ),
    >(
        &mut self,
        mut cb: F,
    ) {
        let (all_sockets, mut locked) =
            self.read_lock_and::<crate::lock_ordering::TcpAllSocketsSet<Ipv4>>();
        all_sockets.keys().for_each(|id| {
            let mut locked = locked.adopt(id);
            let guard = locked
                .read_lock_with::<crate::lock_ordering::TcpSocketState<Ipv4>, _>(|c| c.right());
            cb(id, &*guard);
        });
    }

    fn with_socket_mut_generators_transport_demux<
        O,
        F: for<'a> FnOnce(
            MaybeDualStack<
                (&'a mut Self::DualStackIpTransportAndDemuxCtx<'a>, Self::DualStackConverter),
                (&'a mut Self::SingleStackIpTransportAndDemuxCtx<'a>, Self::SingleStackConverter),
            >,
            &mut TcpSocketState<Ipv4, Self::WeakDeviceId, BC>,
            &IsnGenerator<BC::Instant>,
            &TimestampOffsetGenerator<BC::Instant>,
        ) -> O,
    >(
        &mut self,
        id: &TcpSocketId<Ipv4, Self::WeakDeviceId, BC>,
        cb: F,
    ) -> O {
        let TcpState { isn_generator, timestamp_offset_generator, .. } = &self
            .unlocked_access::<crate::lock_ordering::UnlockedState>()
            .transport
            .tcp_state::<Ipv4>();
        let mut locked = self.adopt(id);
        let (mut socket_state, mut restricted) = locked
            .write_lock_with_and::<crate::lock_ordering::TcpSocketState<Ipv4>, _>(|c| c.right());
        let mut restricted = restricted.cast_core_ctx();
        let maybe_dual_stack = MaybeDualStack::NotDualStack((&mut restricted, ()));
        cb(maybe_dual_stack, &mut socket_state, isn_generator, timestamp_offset_generator)
    }

    fn with_socket_and_converter<
        O,
        F: FnOnce(
            &TcpSocketState<Ipv4, Self::WeakDeviceId, BC>,
            MaybeDualStack<Self::DualStackConverter, Self::SingleStackConverter>,
        ) -> O,
    >(
        &mut self,
        id: &TcpSocketId<Ipv4, Self::WeakDeviceId, BC>,
        cb: F,
    ) -> O {
        // Acquire socket lock at the current level.
        let mut locked = self.adopt(id);
        let socket_state =
            locked.read_lock_with::<crate::lock_ordering::TcpSocketState<Ipv4>, _>(|c| c.right());
        cb(&socket_state, MaybeDualStack::NotDualStack(()))
    }
}

impl<L, BC> TcpContext<Ipv6, BC> for CoreCtx<'_, BC, L>
where
    BC: BindingsContext,
    L: LockBefore<crate::lock_ordering::TcpAllSocketsSet<Ipv6>>,
{
    type ThisStackIpTransportAndDemuxCtx<'a> =
        CoreCtx<'a, BC, WrapLockLevel<crate::lock_ordering::TcpSocketState<Ipv6>>>;
    type SingleStackIpTransportAndDemuxCtx<'a> = UninstantiableWrapper<
        CoreCtx<'a, BC, WrapLockLevel<crate::lock_ordering::TcpSocketState<Ipv6>>>,
    >;

    type DualStackIpTransportAndDemuxCtx<'a> =
        CoreCtx<'a, BC, WrapLockLevel<crate::lock_ordering::TcpSocketState<Ipv6>>>;

    type SingleStackConverter = Uninstantiable;
    type DualStackConverter = ();

    fn with_all_sockets_mut<O, F: FnOnce(&mut TcpSocketSet<Ipv6, Self::WeakDeviceId, BC>) -> O>(
        &mut self,
        cb: F,
    ) -> O {
        let mut all_sockets = self.write_lock::<crate::lock_ordering::TcpAllSocketsSet<Ipv6>>();
        cb(&mut *all_sockets)
    }

    fn for_each_socket<
        F: FnMut(
            &TcpSocketId<Ipv6, Self::WeakDeviceId, BC>,
            &TcpSocketState<Ipv6, Self::WeakDeviceId, BC>,
        ),
    >(
        &mut self,
        mut cb: F,
    ) {
        let (all_sockets, mut locked) =
            self.read_lock_and::<crate::lock_ordering::TcpAllSocketsSet<Ipv6>>();
        all_sockets.keys().for_each(|id| {
            let mut locked = locked.adopt(id);
            let guard = locked
                .read_lock_with::<crate::lock_ordering::TcpSocketState<Ipv6>, _>(|c| c.right());
            cb(id, &*guard);
        });
    }

    fn with_socket_mut_generators_transport_demux<
        O,
        F: for<'a> FnOnce(
            MaybeDualStack<
                (&'a mut Self::DualStackIpTransportAndDemuxCtx<'a>, Self::DualStackConverter),
                (&'a mut Self::SingleStackIpTransportAndDemuxCtx<'a>, Self::SingleStackConverter),
            >,
            &mut TcpSocketState<Ipv6, Self::WeakDeviceId, BC>,
            &IsnGenerator<BC::Instant>,
            &TimestampOffsetGenerator<BC::Instant>,
        ) -> O,
    >(
        &mut self,
        id: &TcpSocketId<Ipv6, Self::WeakDeviceId, BC>,
        cb: F,
    ) -> O {
        let TcpState { isn_generator, timestamp_offset_generator, .. } = &self
            .unlocked_access::<crate::lock_ordering::UnlockedState>()
            .transport
            .tcp_state::<Ipv6>();
        let mut locked = self.adopt(id);
        let (mut socket_state, mut restricted) = locked
            .write_lock_with_and::<crate::lock_ordering::TcpSocketState<Ipv6>, _>(|c| c.right());
        let mut restricted = restricted.cast_core_ctx();
        let maybe_dual_stack = MaybeDualStack::DualStack((&mut restricted, ()));
        cb(maybe_dual_stack, &mut socket_state, isn_generator, timestamp_offset_generator)
    }

    fn with_socket_and_converter<
        O,
        F: FnOnce(
            &TcpSocketState<Ipv6, Self::WeakDeviceId, BC>,
            MaybeDualStack<Self::DualStackConverter, Self::SingleStackConverter>,
        ) -> O,
    >(
        &mut self,
        id: &TcpSocketId<Ipv6, Self::WeakDeviceId, BC>,
        cb: F,
    ) -> O {
        // Acquire socket lock at the current level.
        let mut locked = self.adopt(id);
        let socket_state =
            locked.read_lock_with::<crate::lock_ordering::TcpSocketState<Ipv6>, _>(|c| c.right());
        cb(&socket_state, MaybeDualStack::DualStack(()))
    }
}

impl<L: LockBefore<crate::lock_ordering::TcpDemux<Ipv4>>, BC: BindingsContext>
    TcpDualStackContext<Ipv6, WeakDeviceId<BC>, BC> for CoreCtx<'_, BC, L>
{
    type DualStackIpTransportCtx<'a> =
        CoreCtx<'a, BC, WrapLockLevel<crate::lock_ordering::TcpDemux<Ipv6>>>;
    fn other_demux_id_converter(&self) -> impl tcp::DualStackDemuxIdConverter<Ipv6> {
        tcp::Ipv6SocketIdToIpv4DemuxIdConverter
    }

    fn dual_stack_enabled(&self, ip_options: &tcp::Ipv6Options) -> bool {
        ip_options.dual_stack_enabled
    }

    fn set_dual_stack_enabled(&self, ip_options: &mut tcp::Ipv6Options, value: bool) {
        ip_options.dual_stack_enabled = value;
    }

    fn with_both_demux_mut<
        O,
        F: FnOnce(
            &mut tcp::DemuxState<Ipv6, WeakDeviceId<BC>, BC>,
            &mut tcp::DemuxState<Ipv4, WeakDeviceId<BC>, BC>,
        ) -> O,
    >(
        &mut self,
        cb: F,
    ) -> O {
        let (mut demux_v4, mut locked) =
            self.write_lock_and::<crate::lock_ordering::TcpDemux<Ipv4>>();
        let mut demux_v6 = locked.write_lock::<crate::lock_ordering::TcpDemux<Ipv6>>();
        cb(&mut demux_v6, &mut demux_v4)
    }
}

#[netstack3_macros::instantiate_ip_impl_block(I)]
impl<I: Ip, BC: BindingsContext, L: LockBefore<crate::lock_ordering::UdpAllSocketsSet<I>>>
    udp::StateContext<I, BC> for CoreCtx<'_, BC, L>
{
    type SocketStateCtx<'a> =
        CoreCtx<'a, BC, WrapLockLevel<crate::lock_ordering::UdpSocketState<I>>>;

    fn with_bound_state_context<O, F: FnOnce(&mut Self::SocketStateCtx<'_>) -> O>(
        &mut self,
        cb: F,
    ) -> O {
        cb(&mut self.cast_locked::<crate::lock_ordering::UdpSocketState<I>>())
    }

    fn with_all_sockets_mut<O, F: FnOnce(&mut UdpSocketSet<I, Self::WeakDeviceId, BC>) -> O>(
        &mut self,
        cb: F,
    ) -> O {
        cb(&mut self.write_lock::<crate::lock_ordering::UdpAllSocketsSet<I>>())
    }

    fn with_all_sockets<O, F: FnOnce(&UdpSocketSet<I, Self::WeakDeviceId, BC>) -> O>(
        &mut self,
        cb: F,
    ) -> O {
        cb(&self.read_lock::<crate::lock_ordering::UdpAllSocketsSet<I>>())
    }

    fn with_socket_state<
        O,
        F: FnOnce(&mut Self::SocketStateCtx<'_>, &UdpSocketState<I, Self::WeakDeviceId, BC>) -> O,
    >(
        &mut self,
        id: &UdpSocketId<I, Self::WeakDeviceId, BC>,
        cb: F,
    ) -> O {
        let mut locked = self.adopt(id);
        let (socket_state, mut restricted) =
            locked.read_lock_with_and::<crate::lock_ordering::UdpSocketState<I>, _>(|c| c.right());
        let mut restricted = restricted.cast_core_ctx();
        cb(&mut restricted, &socket_state)
    }

    fn with_socket_state_mut<
        O,
        F: FnOnce(&mut Self::SocketStateCtx<'_>, &mut UdpSocketState<I, Self::WeakDeviceId, BC>) -> O,
    >(
        &mut self,
        id: &UdpSocketId<I, Self::WeakDeviceId, BC>,
        cb: F,
    ) -> O {
        let mut locked = self.adopt(id);
        let (mut socket_state, mut restricted) =
            locked.write_lock_with_and::<crate::lock_ordering::UdpSocketState<I>, _>(|c| c.right());
        let mut restricted = restricted.cast_core_ctx();
        cb(&mut restricted, &mut socket_state)
    }

    fn for_each_socket<
        F: FnMut(
            &mut Self::SocketStateCtx<'_>,
            &UdpSocketId<I, Self::WeakDeviceId, BC>,
            &UdpSocketState<I, Self::WeakDeviceId, BC>,
        ),
    >(
        &mut self,
        mut cb: F,
    ) {
        let (all_sockets, mut locked) =
            self.read_lock_and::<crate::lock_ordering::UdpAllSocketsSet<I>>();
        all_sockets.keys().for_each(|id| {
            let id = UdpSocketId::from(id.clone());
            let mut locked = locked.adopt(&id);
            let (socket_state, mut restricted) = locked
                .read_lock_with_and::<crate::lock_ordering::UdpSocketState<I>, _>(|c| c.right());
            let mut restricted = restricted.cast_core_ctx();
            cb(&mut restricted, &id, &socket_state);
        });
    }
}

impl<BC: BindingsContext, L: LockBefore<crate::lock_ordering::UdpBoundMap<Ipv4>>>
    udp::BoundStateContext<Ipv4, BC> for CoreCtx<'_, BC, L>
{
    type IpSocketsCtx<'a> = CoreCtx<'a, BC, WrapLockLevel<crate::lock_ordering::UdpBoundMap<Ipv4>>>;
    type DualStackContext = UninstantiableWrapper<Self>;
    type NonDualStackContext = Self;

    fn with_bound_sockets<
        O,
        F: FnOnce(&mut Self::IpSocketsCtx<'_>, &udp::BoundSockets<Ipv4, Self::WeakDeviceId, BC>) -> O,
    >(
        &mut self,
        cb: F,
    ) -> O {
        let (bound_sockets, mut locked) =
            self.read_lock_and::<crate::lock_ordering::UdpBoundMap<Ipv4>>();
        cb(&mut locked, &bound_sockets)
    }

    fn with_bound_sockets_mut<
        O,
        F: FnOnce(
            &mut Self::IpSocketsCtx<'_>,
            &mut udp::BoundSockets<Ipv4, Self::WeakDeviceId, BC>,
        ) -> O,
    >(
        &mut self,
        cb: F,
    ) -> O {
        let (mut bound_sockets, mut locked) =
            self.write_lock_and::<crate::lock_ordering::UdpBoundMap<Ipv4>>();
        cb(&mut locked, &mut bound_sockets)
    }

    fn dual_stack_context(
        &self,
    ) -> MaybeDualStack<&Self::DualStackContext, &Self::NonDualStackContext> {
        MaybeDualStack::NotDualStack(self)
    }

    fn dual_stack_context_mut(
        &mut self,
    ) -> MaybeDualStack<&mut Self::DualStackContext, &mut Self::NonDualStackContext> {
        MaybeDualStack::NotDualStack(self)
    }

    fn with_transport_context<O, F: FnOnce(&mut Self::IpSocketsCtx<'_>) -> O>(
        &mut self,
        cb: F,
    ) -> O {
        cb(&mut self.cast_locked())
    }
}

impl<BC: BindingsContext, L: LockBefore<crate::lock_ordering::UdpBoundMap<Ipv4>>>
    udp::BoundStateContext<Ipv6, BC> for CoreCtx<'_, BC, L>
{
    type IpSocketsCtx<'a> = CoreCtx<'a, BC, WrapLockLevel<crate::lock_ordering::UdpBoundMap<Ipv6>>>;
    type DualStackContext = Self;
    type NonDualStackContext = UninstantiableWrapper<Self>;

    fn with_bound_sockets<
        O,
        F: FnOnce(&mut Self::IpSocketsCtx<'_>, &udp::BoundSockets<Ipv6, Self::WeakDeviceId, BC>) -> O,
    >(
        &mut self,
        cb: F,
    ) -> O {
        let (bound_sockets, mut locked) =
            self.read_lock_and::<crate::lock_ordering::UdpBoundMap<Ipv6>>();
        cb(&mut locked, &bound_sockets)
    }

    fn with_bound_sockets_mut<
        O,
        F: FnOnce(
            &mut Self::IpSocketsCtx<'_>,
            &mut udp::BoundSockets<Ipv6, Self::WeakDeviceId, BC>,
        ) -> O,
    >(
        &mut self,
        cb: F,
    ) -> O {
        let (mut bound_sockets, mut locked) =
            self.write_lock_and::<crate::lock_ordering::UdpBoundMap<Ipv6>>();
        cb(&mut locked, &mut bound_sockets)
    }

    fn dual_stack_context(
        &self,
    ) -> MaybeDualStack<&Self::DualStackContext, &Self::NonDualStackContext> {
        MaybeDualStack::DualStack(self)
    }

    fn dual_stack_context_mut(
        &mut self,
    ) -> MaybeDualStack<&mut Self::DualStackContext, &mut Self::NonDualStackContext> {
        MaybeDualStack::DualStack(self)
    }

    fn with_transport_context<O, F: FnOnce(&mut Self::IpSocketsCtx<'_>) -> O>(
        &mut self,
        cb: F,
    ) -> O {
        cb(&mut self.cast_locked::<crate::lock_ordering::UdpBoundMap<Ipv6>>())
    }
}

impl<L, BC: BindingsContext> udp::UdpStateContext for CoreCtx<'_, BC, L> {}

impl<BC: BindingsContext, L: LockBefore<crate::lock_ordering::UdpBoundMap<Ipv4>>>
    udp::DualStackBoundStateContext<Ipv6, BC> for CoreCtx<'_, BC, L>
{
    type IpSocketsCtx<'a> = CoreCtx<'a, BC, WrapLockLevel<crate::lock_ordering::UdpBoundMap<Ipv6>>>;

    fn with_both_bound_sockets_mut<
        O,
        F: FnOnce(
            &mut Self::IpSocketsCtx<'_>,
            &mut udp::BoundSockets<Ipv6, Self::WeakDeviceId, BC>,
            &mut udp::BoundSockets<Ipv4, Self::WeakDeviceId, BC>,
        ) -> O,
    >(
        &mut self,
        cb: F,
    ) -> O {
        let (mut bound_v4, mut locked) =
            self.write_lock_and::<crate::lock_ordering::UdpBoundMap<Ipv4>>();
        let (mut bound_v6, mut locked) =
            locked.write_lock_and::<crate::lock_ordering::UdpBoundMap<Ipv6>>();
        cb(&mut locked, &mut bound_v6, &mut bound_v4)
    }

    fn with_other_bound_sockets_mut<
        O,
        F: FnOnce(
            &mut Self::IpSocketsCtx<'_>,
            &mut udp::BoundSockets<Ipv4, Self::WeakDeviceId, BC>,
        ) -> O,
    >(
        &mut self,
        cb: F,
    ) -> O {
        let (mut bound_v4, mut locked) =
            self.write_lock_and::<crate::lock_ordering::UdpBoundMap<Ipv4>>();
        cb(&mut locked.cast_locked::<crate::lock_ordering::UdpBoundMap<Ipv6>>(), &mut bound_v4)
    }

    fn with_transport_context<O, F: FnOnce(&mut Self::IpSocketsCtx<'_>) -> O>(
        &mut self,
        cb: F,
    ) -> O {
        cb(&mut self.cast_locked::<crate::lock_ordering::UdpBoundMap<Ipv6>>())
    }
}

impl<BC: BindingsContext, L: LockBefore<crate::lock_ordering::UdpBoundMap<Ipv4>>>
    udp::NonDualStackBoundStateContext<Ipv4, BC> for CoreCtx<'_, BC, L>
{
}

impl<I: tcp::DualStackIpExt, BT: BindingsTypes> LockLevelFor<StackState<BT>>
    for crate::lock_ordering::TcpAllSocketsSet<I>
{
    type Data = TcpSocketSet<I, WeakDeviceId<BT>, BT>;
}

impl<I: tcp::DualStackIpExt, BT: BindingsTypes>
    DelegatedOrderedLockAccess<TcpSocketSet<I, WeakDeviceId<BT>, BT>> for StackState<BT>
{
    type Inner = tcp::Sockets<I, WeakDeviceId<BT>, BT>;
    fn delegate_ordered_lock_access(&self) -> &Self::Inner {
        &self.transport.tcp_state::<I>().sockets
    }
}

impl<I: tcp::DualStackIpExt, BT: BindingsTypes> LockLevelFor<StackState<BT>>
    for crate::lock_ordering::TcpDemux<I>
{
    type Data = tcp::DemuxState<I, WeakDeviceId<BT>, BT>;
}

impl<I: tcp::DualStackIpExt, BT: BindingsTypes>
    DelegatedOrderedLockAccess<tcp::DemuxState<I, WeakDeviceId<BT>, BT>> for StackState<BT>
{
    type Inner = tcp::Sockets<I, WeakDeviceId<BT>, BT>;
    fn delegate_ordered_lock_access(&self) -> &Self::Inner {
        &self.transport.tcp_state::<I>().sockets
    }
}

impl<I: crate::transport::tcp::DualStackIpExt, D: WeakDeviceIdentifier, BT: BindingsTypes>
    LockLevelFor<TcpSocketId<I, D, BT>> for crate::lock_ordering::TcpSocketState<I>
{
    type Data = TcpSocketState<I, D, BT>;
}

impl<I: datagram::IpExt, D: WeakDeviceIdentifier, BT: BindingsTypes>
    LockLevelFor<UdpSocketId<I, D, BT>> for crate::lock_ordering::UdpSocketState<I>
{
    type Data = UdpSocketState<I, D, BT>;
}

impl<I: datagram::IpExt, BT: BindingsTypes> LockLevelFor<StackState<BT>>
    for crate::lock_ordering::UdpBoundMap<I>
{
    type Data = udp::BoundSockets<I, WeakDeviceId<BT>, BT>;
}

impl<I: datagram::IpExt, BT: BindingsTypes>
    DelegatedOrderedLockAccess<udp::BoundSockets<I, WeakDeviceId<BT>, BT>> for StackState<BT>
{
    type Inner = udp::Sockets<I, WeakDeviceId<BT>, BT>;
    fn delegate_ordered_lock_access(&self) -> &Self::Inner {
        &self.transport.udp_state::<I>().sockets
    }
}

impl<I: datagram::IpExt, BT: BindingsTypes> LockLevelFor<StackState<BT>>
    for crate::lock_ordering::UdpAllSocketsSet<I>
{
    type Data = UdpSocketSet<I, WeakDeviceId<BT>, BT>;
}

impl<I: datagram::IpExt, BT: BindingsTypes>
    DelegatedOrderedLockAccess<UdpSocketSet<I, WeakDeviceId<BT>, BT>> for StackState<BT>
{
    type Inner = udp::Sockets<I, WeakDeviceId<BT>, BT>;
    fn delegate_ordered_lock_access(&self) -> &Self::Inner {
        &self.transport.udp_state::<I>().sockets
    }
}

impl<BC: BindingsContext, I: Ip, L> CounterContext<TcpCountersWithSocket<I>>
    for CoreCtx<'_, BC, L>
{
    fn counters(&self) -> &TcpCountersWithSocket<I> {
        self.unlocked_access::<crate::lock_ordering::UnlockedState>()
            .transport
            .tcp_counters_with_socket::<I>()
    }
}

impl<BC: BindingsContext, I: Ip, L> CounterContext<TcpCountersWithoutSocket<I>>
    for CoreCtx<'_, BC, L>
{
    fn counters(&self) -> &TcpCountersWithoutSocket<I> {
        self.unlocked_access::<crate::lock_ordering::UnlockedState>()
            .transport
            .tcp_counters_without_socket::<I>()
    }
}

impl<BC: BindingsContext, I: netstack3_tcp::DualStackIpExt, L>
    ResourceCounterContext<TcpSocketId<I, WeakDeviceId<BC>, BC>, TcpCountersWithSocket<I>>
    for CoreCtx<'_, BC, L>
{
    fn per_resource_counters<'a>(
        &'a self,
        resource: &'a TcpSocketId<I, WeakDeviceId<BC>, BC>,
    ) -> &'a TcpCountersWithSocket<I> {
        resource.counters()
    }
}

impl<BC: BindingsContext, I: Ip, L> CounterContext<UdpCountersWithSocket<I>>
    for CoreCtx<'_, BC, L>
{
    fn counters(&self) -> &UdpCountersWithSocket<I> {
        self.unlocked_access::<crate::lock_ordering::UnlockedState>()
            .transport
            .udp_counters_with_socket::<I>()
    }
}

impl<BC: BindingsContext, I: Ip, L> CounterContext<UdpCountersWithoutSocket<I>>
    for CoreCtx<'_, BC, L>
{
    fn counters(&self) -> &UdpCountersWithoutSocket<I> {
        self.unlocked_access::<crate::lock_ordering::UnlockedState>()
            .transport
            .udp_counters_without_socket::<I>()
    }
}

impl<BC: BindingsContext, I: netstack3_datagram::IpExt, L>
    ResourceCounterContext<UdpSocketId<I, WeakDeviceId<BC>, BC>, UdpCountersWithSocket<I>>
    for CoreCtx<'_, BC, L>
{
    fn per_resource_counters<'a>(
        &'a self,
        resource: &'a UdpSocketId<I, WeakDeviceId<BC>, BC>,
    ) -> &'a UdpCountersWithSocket<I> {
        resource.counters()
    }
}
