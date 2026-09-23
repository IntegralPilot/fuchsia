// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! An implementation of multicast forwarding.
//!
//! Multicast forwarding is the ability for netstack to forward multicast
//! packets that arrive on an interface out multiple interfaces (while also
//! optionally delivering the packet to the host itself if the arrival host has
//! an interest in the packet).
//!
//! Note that multicast forwarding decisions are made by consulting the
//! multicast routing table, a routing table entirely separate from the unicast
//! routing table(s).

pub(crate) mod api;
pub(crate) mod counters;
pub(crate) mod packet_queue;
pub(crate) mod route;
pub(crate) mod state;

use core::sync::atomic::Ordering;

use net_types::ip::{GenericOverIp, Ip, IpVersionMarker};
use netstack3_base::{
    AnyDevice, AtomicInstant, CounterContext, DeviceIdContext, EventContext, HandleableTimer,
    InstantBindingsTypes, InstantContext, LocalFrameDestination, TimerBindingsTypes, TimerContext,
    WeakDeviceIdentifier,
};
use packet_formats::ip::IpPacket;
use zerocopy::SplitByteSlice;

use crate::internal::multicast_forwarding::counters::MulticastForwardingCounters;
use crate::internal::multicast_forwarding::packet_queue::QueuePacketOutcome;
use crate::internal::multicast_forwarding::route::{
    Action, MulticastRouteEntry, MulticastRouteTargets,
};
use crate::multicast_forwarding::{
    MulticastForwardingPendingPacketsContext, MulticastForwardingState,
    MulticastForwardingStateContext, MulticastRoute, MulticastRouteKey,
    MulticastRouteTableContext as _,
};
use crate::{IpLayerEvent, IpLayerIpExt};

/// Required types for multicast forwarding provided by Bindings.
pub trait MulticastForwardingBindingsTypes: InstantBindingsTypes + TimerBindingsTypes {}
impl<BT: InstantBindingsTypes + TimerBindingsTypes> MulticastForwardingBindingsTypes for BT {}

/// Required functionality for multicast forwarding provided by Bindings.
pub trait MulticastForwardingBindingsContext<I: IpLayerIpExt, D>:
    MulticastForwardingBindingsTypes + InstantContext + TimerContext + EventContext<IpLayerEvent<D, I>>
{
}
impl<
    I: IpLayerIpExt,
    D,
    BC: MulticastForwardingBindingsTypes
        + InstantContext
        + TimerContext
        + EventContext<IpLayerEvent<D, I>>,
> MulticastForwardingBindingsContext<I, D> for BC
{
}

/// Device related functionality required by multicast forwarding.
pub trait MulticastForwardingDeviceContext<I: IpLayerIpExt>: DeviceIdContext<AnyDevice> {
    /// True if the given device has multicast forwarding enabled.
    fn is_device_multicast_forwarding_enabled(&mut self, dev: &Self::DeviceId) -> bool;
}

/// A timer event for multicast forwarding.
#[derive(Clone, Debug, Eq, GenericOverIp, Hash, PartialEq)]
#[generic_over_ip(I, Ip)]
pub enum MulticastForwardingTimerId<I: Ip> {
    /// A trigger to perform garbage collection on the pending packets table.
    PendingPacketsGc(IpVersionMarker<I>),
}

impl<
    I: IpLayerIpExt,
    BC: MulticastForwardingBindingsContext<I, CC::DeviceId>,
    CC: MulticastForwardingStateContext<I, BC> + CounterContext<MulticastForwardingCounters<I>>,
> HandleableTimer<CC, BC> for MulticastForwardingTimerId<I>
{
    fn handle(self, core_ctx: &mut CC, bindings_ctx: &mut BC, _: BC::UniqueTimerId) {
        match self {
            MulticastForwardingTimerId::PendingPacketsGc(_) => {
                core_ctx.with_state(|state, ctx| match state {
                    // Multicast forwarding was disabled after GC was scheduled;
                    // there are no resources to GC now.
                    MulticastForwardingState::Disabled => {}
                    MulticastForwardingState::Enabled(state) => {
                        CounterContext::<MulticastForwardingCounters<I>>::counters(ctx)
                            .pending_table_gc
                            .increment();
                        let removed_count = ctx.with_pending_table_mut(state, |pending_table| {
                            pending_table.run_garbage_collection(bindings_ctx)
                        });
                        CounterContext::<MulticastForwardingCounters<I>>::counters(ctx)
                            .pending_packet_drops_gc
                            .add(removed_count);
                    }
                })
            }
        }
    }
}

/// Events that may be published by the multicast forwarding engine.
#[derive(Debug, Eq, Hash, PartialEq, GenericOverIp)]
#[generic_over_ip(I, Ip)]
pub enum MulticastForwardingEvent<I: IpLayerIpExt, D> {
    /// A multicast packet was received for which there was no applicable route.
    MissingRoute {
        /// The key of the route that's missing.
        key: MulticastRouteKey<I>,
        /// The interface on which the packet was received.
        input_interface: D,
    },
    /// A multicast packet was received on an unexpected input interface.
    WrongInputInterface {
        /// The key of the route with the unexpected input interface.
        key: MulticastRouteKey<I>,
        /// The interface on which the packet was received.
        actual_input_interface: D,
        /// The interface on which the packet was expected (as specified in the
        /// multicast route).
        expected_input_interface: D,
    },
}

impl<I: IpLayerIpExt, D> MulticastForwardingEvent<I, D> {
    pub(crate) fn map_device<O, F: Fn(D) -> O>(self, map: F) -> MulticastForwardingEvent<I, O> {
        match self {
            MulticastForwardingEvent::MissingRoute { key, input_interface } => {
                MulticastForwardingEvent::MissingRoute {
                    key,
                    input_interface: map(input_interface),
                }
            }
            MulticastForwardingEvent::WrongInputInterface {
                key,
                actual_input_interface,
                expected_input_interface,
            } => MulticastForwardingEvent::WrongInputInterface {
                key,
                actual_input_interface: map(actual_input_interface),
                expected_input_interface: map(expected_input_interface),
            },
        }
    }
}

impl<I: IpLayerIpExt, D: WeakDeviceIdentifier> MulticastForwardingEvent<I, D> {
    /// Upgrades the device IDs held by this event.
    pub fn upgrade_device_id(self) -> Option<MulticastForwardingEvent<I, D::Strong>> {
        match self {
            MulticastForwardingEvent::MissingRoute { key, input_interface } => {
                Some(MulticastForwardingEvent::MissingRoute {
                    key,
                    input_interface: input_interface.upgrade()?,
                })
            }
            MulticastForwardingEvent::WrongInputInterface {
                key,
                actual_input_interface,
                expected_input_interface,
            } => Some(MulticastForwardingEvent::WrongInputInterface {
                key,
                actual_input_interface: actual_input_interface.upgrade()?,
                expected_input_interface: expected_input_interface.upgrade()?,
            }),
        }
    }
}

/// Query the multicast route table and return the forwarding targets.
///
/// `None` may be returned in several situations:
///   * if multicast forwarding is disabled (either stack-wide or for the
///     provided `dev`),
///   * if the packets src/dst addrs are not viable for multicast forwarding
///     (see the requirements on [`MulticastRouteKey`]), or
///   * if the route table does not have an entry suitable for this packet.
///
/// In the latter case, the packet is stashed in the
/// [`MulticastForwardingPendingPackets`] table, and a relevant event is
/// dispatched to bindings.
///
/// Note that the returned targets are not synchronized with the multicast route
/// table and may grow stale if the table is updated.
pub(crate) fn lookup_multicast_route_or_stash_packet<I, B, CC, BC>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    packet: &I::Packet<B>,
    dev: &CC::DeviceId,
    frame_dst: Option<LocalFrameDestination>,
    max_fragment_len: Option<usize>,
) -> Option<MulticastRouteTargets<CC::DeviceId>>
where
    I: IpLayerIpExt,
    B: SplitByteSlice,
    CC: MulticastForwardingStateContext<I, BC>
        + MulticastForwardingDeviceContext<I>
        + CounterContext<MulticastForwardingCounters<I>>,
    BC: MulticastForwardingBindingsContext<I, CC::DeviceId>,
{
    CounterContext::<MulticastForwardingCounters<I>>::counters(core_ctx).rx.increment();
    // Short circuit if the packet's addresses don't constitute a valid
    // multicast route key (e.g. src is not unicast, or dst is not multicast).
    let Some(key) = MulticastRouteKey::new(packet.src_ip(), packet.dst_ip()) else {
        CounterContext::<MulticastForwardingCounters<I>>::counters(core_ctx)
            .no_tx_invalid_key
            .increment();
        return None;
    };

    // Short circuit if the device has forwarding disabled.
    if !core_ctx.is_device_multicast_forwarding_enabled(dev) {
        CounterContext::<MulticastForwardingCounters<I>>::counters(core_ctx)
            .no_tx_disabled_dev
            .increment();
        return None;
    }

    core_ctx.with_state(|state, ctx| {
        // Short circuit if forwarding is disabled stack-wide.
        let Some(state) = state.enabled() else {
            CounterContext::<MulticastForwardingCounters<I>>::counters(ctx)
                .no_tx_disabled_stack_wide
                .increment();
            return None;
        };
        ctx.with_route_table(state, |route_table, ctx| {
            if let Some(MulticastRouteEntry {
                route: MulticastRoute { input_interface, action },
                stats,
            }) = route_table.get(&key)
            {
                if dev != input_interface {
                    CounterContext::<MulticastForwardingCounters<I>>::counters(ctx)
                        .no_tx_wrong_dev
                        .increment();
                    bindings_ctx.on_event(
                        MulticastForwardingEvent::WrongInputInterface {
                            key,
                            actual_input_interface: dev.clone(),
                            expected_input_interface: input_interface.clone(),
                        }
                        .into(),
                    );
                    return None;
                }

                stats.last_used.store_max(bindings_ctx.now(), Ordering::Relaxed);

                match action {
                    Action::Forward(targets) => {
                        CounterContext::<MulticastForwardingCounters<I>>::counters(ctx)
                            .tx
                            .increment();
                        return Some(targets.clone());
                    }
                }
            }
            CounterContext::<MulticastForwardingCounters<I>>::counters(ctx)
                .pending_packets
                .increment();
            match ctx.with_pending_table_mut(state, |pending_table| {
                pending_table.try_queue_packet(
                    bindings_ctx,
                    key.clone(),
                    packet,
                    dev,
                    frame_dst,
                    max_fragment_len,
                )
            }) {
                QueuePacketOutcome::QueuedInNewQueue => {
                    bindings_ctx.on_event(
                        MulticastForwardingEvent::MissingRoute {
                            key,
                            input_interface: dev.clone(),
                        }
                        .into(),
                    );
                }
                QueuePacketOutcome::QueuedInExistingQueue => {}
                QueuePacketOutcome::ExistingQueueFull => {
                    CounterContext::<MulticastForwardingCounters<I>>::counters(ctx)
                        .pending_packet_drops_queue_full
                        .increment();
                }
            }
            return None;
        })
    })
}

#[cfg(test)]
mod testutil {
    use super::*;

    use alloc::rc::Rc;
    use alloc::vec::Vec;
    use core::cell::RefCell;
    use derivative::Derivative;
    use net_declare::{net_ip_v4, net_ip_v6};
    use net_types::MulticastAddr;
    use net_types::ip::{Ipv4, Ipv4Addr, Ipv6, Ipv6Addr, Mtu};
    use netstack3_base::socket::SocketIpAddr;
    use netstack3_base::testutil::{FakeStrongDeviceId, MultipleDevicesId};
    use netstack3_base::{
        CoreTimerContext, CounterContext, CtxPair, Marks, NetworkSerializationContext,
        NetworkSerializer, ResourceCounterContext,
    };
    use netstack3_filter::ProofOfEgressCheck;
    use netstack3_hashmap::HashSet;
    use packet::{BufferMut, InnerPacketBuilder, NestablePacketBuilder as _, Serializer};
    use packet_formats::ip::{IpPacketBuilder, IpProto};

    use crate::device::IpDeviceSendContext;
    use crate::internal::base::DeviceIpLayerMetadata;
    use crate::internal::icmp::IcmpErrorHandler;
    use crate::multicast_forwarding::{
        MulticastForwardingApi, MulticastForwardingEnabledState, MulticastForwardingPendingPackets,
        MulticastForwardingPendingPacketsContext, MulticastForwardingState, MulticastRouteTable,
        MulticastRouteTableContext,
    };
    use crate::{IpCounters, IpDeviceMtuContext, IpLayerEvent, IpPacketDestination};

    /// An IP extension trait providing constants for various IP addresses.
    pub(crate) trait TestIpExt: IpLayerIpExt {
        const SRC1: Self::Addr;
        const SRC2: Self::Addr;
        const DST1: Self::Addr;
        const DST2: Self::Addr;
    }

    impl TestIpExt for Ipv4 {
        const SRC1: Ipv4Addr = net_ip_v4!("192.0.2.1");
        const SRC2: Ipv4Addr = net_ip_v4!("192.0.2.2");
        const DST1: Ipv4Addr = net_ip_v4!("224.0.1.1");
        const DST2: Ipv4Addr = net_ip_v4!("224.0.1.2");
    }

    impl TestIpExt for Ipv6 {
        const SRC1: Ipv6Addr = net_ip_v6!("2001:0DB8::1");
        const SRC2: Ipv6Addr = net_ip_v6!("2001:0DB8::2");
        const DST1: Ipv6Addr = net_ip_v6!("ff0e::1");
        const DST2: Ipv6Addr = net_ip_v6!("ff0e::2");
    }

    /// Constructs a buffer containing an IP packet with sensible defaults.
    pub(crate) fn new_ip_packet_buf<I: IpLayerIpExt>(
        src_addr: I::Addr,
        dst_addr: I::Addr,
    ) -> impl AsRef<[u8]> {
        const TTL: u8 = 255;
        /// Arbitrary data to put inside of an IP packet.
        const IP_BODY: [u8; 10] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
        I::PacketBuilder::new(src_addr, dst_addr, TTL, IpProto::Udp.into())
            .wrap_body(IP_BODY.into_serializer())
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
    }

    #[derive(Debug, PartialEq)]
    pub(crate) struct SentPacket<I: IpLayerIpExt, D> {
        pub(crate) dst: MulticastAddr<I::Addr>,
        pub(crate) device: D,
    }

    #[derive(Derivative)]
    #[derivative(Default(bound = ""))]
    pub(crate) struct FakeCoreCtxState<I: IpLayerIpExt, D: FakeStrongDeviceId> {
        // NB: Hold in an `Rc<RefCell<...>>` to switch to runtime borrow
        // checking. This allows us to borrow the multicast forwarding state at
        // the same time as the outer `FakeCoreCtx` is mutably borrowed.
        pub(crate) multicast_forwarding:
            Rc<RefCell<MulticastForwardingState<I, D, FakeBindingsCtx<I, D>>>>,
        // The list of devices that have multicast forwarding enabled.
        pub(crate) forwarding_enabled_devices: HashSet<D>,
        // The list of packets sent by the netstack.
        pub(crate) sent_packets: Vec<SentPacket<I, D>>,
        stack_wide_counters: IpCounters<I>,
        per_device_counters: IpCounters<I>,
        multicast_forwarding_counters: MulticastForwardingCounters<I>,
    }

    impl<I: IpLayerIpExt, D: FakeStrongDeviceId> FakeCoreCtxState<I, D> {
        pub(crate) fn set_multicast_forwarding_enabled_for_dev(&mut self, dev: D, enabled: bool) {
            if enabled {
                let _: bool = self.forwarding_enabled_devices.insert(dev);
            } else {
                let _: bool = self.forwarding_enabled_devices.remove(&dev);
            }
        }

        pub(crate) fn take_sent_packets(&mut self) -> Vec<SentPacket<I, D>> {
            core::mem::take(&mut self.sent_packets)
        }
    }

    impl<I: IpLayerIpExt, D: FakeStrongDeviceId> CounterContext<IpCounters<I>>
        for FakeCoreCtxState<I, D>
    {
        fn counters(&self) -> &IpCounters<I> {
            &self.stack_wide_counters
        }
    }

    impl<I: IpLayerIpExt, D: FakeStrongDeviceId> ResourceCounterContext<D, IpCounters<I>>
        for FakeCoreCtxState<I, D>
    {
        fn per_resource_counters(&self, _resource: &D) -> &IpCounters<I> {
            &self.per_device_counters
        }
    }

    impl<I: IpLayerIpExt, D: FakeStrongDeviceId> CounterContext<MulticastForwardingCounters<I>>
        for FakeCoreCtxState<I, D>
    {
        fn counters(&self) -> &MulticastForwardingCounters<I> {
            &self.multicast_forwarding_counters
        }
    }

    pub(crate) type FakeBindingsCtx<I, D> = netstack3_base::testutil::FakeBindingsCtx<
        MulticastForwardingTimerId<I>,
        IpLayerEvent<D, I>,
        (),
        (),
    >;
    pub(crate) type FakeCoreCtx<I, D> =
        netstack3_base::testutil::FakeCoreCtx<FakeCoreCtxState<I, D>, (), D>;

    impl<I: IpLayerIpExt, D: FakeStrongDeviceId>
        MulticastForwardingStateContext<I, FakeBindingsCtx<I, D>> for FakeCoreCtx<I, D>
    {
        type Ctx<'a> = FakeCoreCtx<I, D>;
        fn with_state<
            O,
            F: FnOnce(
                &MulticastForwardingState<I, Self::DeviceId, FakeBindingsCtx<I, D>>,
                &mut Self::Ctx<'_>,
            ) -> O,
        >(
            &mut self,
            cb: F,
        ) -> O {
            let state = self.state.multicast_forwarding.clone();
            let borrow = state.borrow();
            cb(&borrow, self)
        }
        fn with_state_mut<
            O,
            F: FnOnce(
                &mut MulticastForwardingState<I, Self::DeviceId, FakeBindingsCtx<I, D>>,
                &mut Self::Ctx<'_>,
            ) -> O,
        >(
            &mut self,
            cb: F,
        ) -> O {
            let state = self.state.multicast_forwarding.clone();
            let mut borrow = state.borrow_mut();
            cb(&mut borrow, self)
        }
    }

    impl<I: IpLayerIpExt, D: FakeStrongDeviceId>
        MulticastRouteTableContext<I, FakeBindingsCtx<I, D>> for FakeCoreCtx<I, D>
    {
        type Ctx<'a> = FakeCoreCtx<I, D>;
        fn with_route_table<
            O,
            F: FnOnce(
                &MulticastRouteTable<I, Self::DeviceId, FakeBindingsCtx<I, D>>,
                &mut Self::Ctx<'_>,
            ) -> O,
        >(
            &mut self,
            state: &MulticastForwardingEnabledState<I, Self::DeviceId, FakeBindingsCtx<I, D>>,
            cb: F,
        ) -> O {
            let route_table = state.route_table().read();
            cb(&route_table, self)
        }
        fn with_route_table_mut<
            O,
            F: FnOnce(
                &mut MulticastRouteTable<I, Self::DeviceId, FakeBindingsCtx<I, D>>,
                &mut Self::Ctx<'_>,
            ) -> O,
        >(
            &mut self,
            state: &MulticastForwardingEnabledState<I, Self::DeviceId, FakeBindingsCtx<I, D>>,
            cb: F,
        ) -> O {
            let mut route_table = state.route_table().write();
            cb(&mut route_table, self)
        }
    }

    impl<I: IpLayerIpExt, D: FakeStrongDeviceId>
        MulticastForwardingPendingPacketsContext<I, FakeBindingsCtx<I, D>> for FakeCoreCtx<I, D>
    {
        fn with_pending_table_mut<
            O,
            F: FnOnce(
                &mut MulticastForwardingPendingPackets<I, Self::WeakDeviceId, FakeBindingsCtx<I, D>>,
            ) -> O,
        >(
            &mut self,
            state: &MulticastForwardingEnabledState<I, Self::DeviceId, FakeBindingsCtx<I, D>>,
            cb: F,
        ) -> O {
            let mut pending_table = state.pending_table().lock();
            cb(&mut pending_table)
        }
    }

    impl<I: IpLayerIpExt, D: FakeStrongDeviceId> MulticastForwardingDeviceContext<I>
        for FakeCoreCtx<I, D>
    {
        fn is_device_multicast_forwarding_enabled(&mut self, device_id: &Self::DeviceId) -> bool {
            self.state.forwarding_enabled_devices.contains(device_id)
        }
    }

    impl<I: IpLayerIpExt, D: FakeStrongDeviceId>
        CoreTimerContext<MulticastForwardingTimerId<I>, FakeBindingsCtx<I, D>>
        for FakeCoreCtx<I, D>
    {
        fn convert_timer(
            dispatch_id: MulticastForwardingTimerId<I>,
        ) -> MulticastForwardingTimerId<I> {
            dispatch_id
        }
    }

    impl<I: IpLayerIpExt, D: FakeStrongDeviceId> IpDeviceSendContext<I, FakeBindingsCtx<I, D>>
        for FakeCoreCtx<I, D>
    {
        fn send_ip_frame<S>(
            &mut self,
            _bindings_ctx: &mut FakeBindingsCtx<I, D>,
            device_id: &D,
            destination: IpPacketDestination<I, &D>,
            _ip_layer_metadata: DeviceIpLayerMetadata<FakeBindingsCtx<I, D>>,
            _body: S,
            _egress_proof: ProofOfEgressCheck,
        ) -> Result<(), netstack3_base::SendFrameError<S>>
        where
            S: NetworkSerializer,
            S::Buffer: BufferMut,
        {
            let dst = match destination {
                IpPacketDestination::Multicast(dst) => dst,
                dst => panic!("unexpected sent packet: destination={dst:?}"),
            };
            self.state.sent_packets.push(SentPacket { dst, device: device_id.clone() });
            Ok(())
        }
    }

    impl<I: IpLayerIpExt, D: FakeStrongDeviceId> IpDeviceMtuContext<I> for FakeCoreCtx<I, D> {
        fn get_mtu(&mut self, _device_id: &Self::DeviceId) -> Mtu {
            Mtu::max()
        }
    }

    impl<I: IpLayerIpExt, D: FakeStrongDeviceId> IcmpErrorHandler<I, FakeBindingsCtx<I, D>>
        for FakeCoreCtx<I, D>
    {
        fn send_icmp_error_message<B: BufferMut>(
            &mut self,
            _bindings_ctx: &mut FakeBindingsCtx<I, D>,
            _device: Option<&D>,
            _frame_dst: Option<LocalFrameDestination>,
            _src_ip: SocketIpAddr<I::Addr>,
            _dst_ip: SocketIpAddr<I::Addr>,
            _original_packet: B,
            _error: I::IcmpError,
            _header_len: usize,
            _proto: I::Proto,
            _marks: &Marks,
        ) {
            unimplemented!()
        }
    }

    pub(crate) fn new_api<I: IpLayerIpExt>() -> MulticastForwardingApi<
        I,
        CtxPair<FakeCoreCtx<I, MultipleDevicesId>, FakeBindingsCtx<I, MultipleDevicesId>>,
    > {
        MulticastForwardingApi::new(CtxPair::with_core_ctx(FakeCoreCtx::with_state(
            Default::default(),
        )))
    }

    /// A test helper to access the [`MulticastForwardingPendingPackets`] table.
    ///
    /// # Panics
    ///
    /// Panics if multicast forwarding is disabled.
    pub(crate) fn with_pending_table<I, O, F, CC, BT>(core_ctx: &mut CC, cb: F) -> O
    where
        I: IpLayerIpExt,
        CC: MulticastForwardingStateContext<I, BT>,
        BT: MulticastForwardingBindingsTypes,
        F: FnOnce(&mut MulticastForwardingPendingPackets<I, CC::WeakDeviceId, BT>) -> O,
    {
        core_ctx.with_state(|state, ctx| {
            let state = state.enabled().unwrap();
            ctx.with_route_table(state, |_routing_table, ctx| {
                ctx.with_pending_table_mut(state, |pending_table| cb(pending_table))
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use alloc::vec;
    use core::num::NonZeroU8;
    use core::time::Duration;

    use ip_test_macro::ip_test;
    use netstack3_base::testutil::MultipleDevicesId;
    use packet::ParseBuffer;
    use test_case::test_case;
    use testutil::TestIpExt;

    use crate::internal::multicast_forwarding::route::MulticastRouteStats;
    use crate::multicast_forwarding::MulticastRouteTarget;

    struct LookupTestCase {
        // Whether multicast forwarding is enabled for the netstack.
        enabled: bool,
        // Whether multicast forwarding is enabled for the device.
        dev_enabled: bool,
        // Whether the packet has the correct src/dst addrs.
        right_key: bool,
        // Whether the packet arrived on the correct device.
        right_dev: bool,
    }
    const LOOKUP_SUCCESS_CASE: LookupTestCase =
        LookupTestCase { enabled: true, dev_enabled: true, right_key: true, right_dev: true };

    #[ip_test(I)]
    #[test_case(LOOKUP_SUCCESS_CASE => true; "success")]
    #[test_case(LookupTestCase{enabled: false, ..LOOKUP_SUCCESS_CASE} => false; "disabled")]
    #[test_case(LookupTestCase{dev_enabled: false, ..LOOKUP_SUCCESS_CASE} => false; "dev_disabled")]
    #[test_case(LookupTestCase{right_key: false, ..LOOKUP_SUCCESS_CASE} => false; "wrong_key")]
    #[test_case(LookupTestCase{right_dev: false, ..LOOKUP_SUCCESS_CASE} => false; "wrong_dev")]
    fn lookup_route<I: TestIpExt>(test_case: LookupTestCase) -> bool {
        let LookupTestCase { enabled, dev_enabled, right_key, right_dev } = test_case;
        const FRAME_DST: Option<LocalFrameDestination> = None;
        const MAX_FRAGMENT_LEN: Option<usize> = None;
        let mut api = testutil::new_api::<I>();

        let expected_key = MulticastRouteKey::new(I::SRC1, I::DST1).unwrap();
        let actual_key = if right_key {
            expected_key.clone()
        } else {
            MulticastRouteKey::new(I::SRC2, I::DST2).unwrap()
        };

        let expected_dev = MultipleDevicesId::A;
        let actual_dev = if right_dev { expected_dev } else { MultipleDevicesId::B };

        if enabled {
            assert!(api.enable());
            // NB: Only attempt to install the route when enabled; Otherwise
            // installation fails.
            assert_eq!(
                api.add_multicast_route(
                    expected_key.clone(),
                    MulticastRoute::new_forward(
                        expected_dev,
                        [MulticastRouteTarget {
                            output_interface: MultipleDevicesId::C,
                            min_ttl: NonZeroU8::new(1).unwrap(),
                        }]
                        .into()
                    )
                    .unwrap()
                ),
                Ok(None)
            );
        }

        api.core_ctx().state.set_multicast_forwarding_enabled_for_dev(actual_dev, dev_enabled);

        let (core_ctx, bindings_ctx) = api.contexts();
        let creation_time = bindings_ctx.now();
        bindings_ctx.timers.instant.sleep(Duration::from_secs(5));
        let lookup_time = bindings_ctx.now();
        assert!(lookup_time > creation_time);

        let buf = testutil::new_ip_packet_buf::<I>(actual_key.src_addr(), actual_key.dst_addr());
        let mut buf_ref = buf.as_ref();
        let packet = buf_ref.parse::<I::Packet<_>>().expect("parse should succeed");

        let route = lookup_multicast_route_or_stash_packet(
            core_ctx,
            bindings_ctx,
            &packet,
            &actual_dev,
            FRAME_DST,
            MAX_FRAGMENT_LEN,
        );

        // Verify that multicast routing events are generated.
        let mut expected_events = vec![];
        if !right_key {
            expected_events.push(IpLayerEvent::MulticastForwarding(
                MulticastForwardingEvent::MissingRoute {
                    key: actual_key.clone(),
                    input_interface: actual_dev,
                },
            ));
        }
        if !right_dev {
            expected_events.push(IpLayerEvent::MulticastForwarding(
                MulticastForwardingEvent::WrongInputInterface {
                    key: actual_key,
                    actual_input_interface: actual_dev,
                    expected_input_interface: expected_dev,
                },
            ));
        }
        assert_eq!(bindings_ctx.take_events(), expected_events);

        let lookup_succeeded = route.is_some();

        if enabled {
            // Verify that on success, the last_used field in stats is updated.
            let expected_stats = if lookup_succeeded {
                MulticastRouteStats { last_used: lookup_time }
            } else {
                MulticastRouteStats { last_used: creation_time }
            };
            assert_eq!(api.get_route_stats(&expected_key), Ok(Some(expected_stats)));
        }

        // Verify that counters are updated.
        let counters: &MulticastForwardingCounters<I> = api.core_ctx().counters();
        assert_eq!(counters.rx.get(), 1);
        assert_eq!(counters.tx.get(), if lookup_succeeded { 1 } else { 0 });
        assert_eq!(counters.no_tx_disabled_dev.get(), if dev_enabled { 0 } else { 1 });
        assert_eq!(counters.no_tx_disabled_stack_wide.get(), if enabled { 0 } else { 1 });
        assert_eq!(counters.no_tx_wrong_dev.get(), if right_dev { 0 } else { 1 });
        assert_eq!(counters.pending_packets.get(), if right_key { 0 } else { 1 });

        lookup_succeeded
    }
}
