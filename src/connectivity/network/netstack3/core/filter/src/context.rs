// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::fmt::Debug;

use net_types::SpecifiedAddr;
use net_types::ip::{IpVersion, Ipv4, Ipv6};

pub use netstack3_base::Marks;
pub use netstack3_base::socket::{EitherIpProto, SocketInfo};
use netstack3_base::{
    InstantBindingsTypes, InterfaceProperties, IpDeviceAddr, IpDeviceAddressIdContext,
    MatcherBindingsTypes, RngContext, TimerBindingsTypes, TimerContext, TxMetadataBindingsTypes,
};
use packet::FragmentedByteSlice;
use packet_formats::ip::IpExt;

use crate::FilterIpExt;
use crate::matchers::BindingsPacketMatcher;
use crate::packets::FilterIpPacket;
use crate::state::State;

/// Trait defining required types for filtering provided by bindings.
pub trait FilterBindingsTypes:
    InstantBindingsTypes + MatcherBindingsTypes + TimerBindingsTypes + 'static
{
}

impl<BT: InstantBindingsTypes + MatcherBindingsTypes + TimerBindingsTypes + 'static>
    FilterBindingsTypes for BT
{
}

/// Trait aggregating functionality required from bindings.
pub trait FilterBindingsContext<D>:
    TimerContext + RngContext + FilterBindingsTypes<BindingsPacketMatcher: BindingsPacketMatcher<D>>
{
}
impl<D, BC> FilterBindingsContext<D> for BC
where
    BC: TimerContext + RngContext + FilterBindingsTypes,
    BC::BindingsPacketMatcher: BindingsPacketMatcher<D>,
{
}

/// The IP version-specific execution context for packet filtering.
///
/// This trait exists to abstract over access to the filtering state. It is
/// useful to implement filtering logic in terms of this trait, as opposed to,
/// for example, [`crate::logic::FilterHandler`] methods taking the state
/// directly as an argument, because it allows Netstack3 Core to use lock
/// ordering types to enforce that filtering state is only acquired at or before
/// a given lock level, while keeping test code free of locking concerns.
pub trait FilterIpContext<I: FilterIpExt, BT: FilterBindingsTypes>:
    IpDeviceAddressIdContext<I, DeviceId: InterfaceProperties<BT::DeviceClass>>
{
    /// The execution context that allows the filtering engine to perform
    /// Network Address Translation (NAT).
    type NatCtx<'a>: NatContext<I, BT, DeviceId = Self::DeviceId, WeakAddressId = Self::WeakAddressId>;

    /// Calls the function with a reference to filtering state.
    fn with_filter_state<O, F: FnOnce(&State<I, Self::WeakAddressId, BT>) -> O>(
        &mut self,
        cb: F,
    ) -> O {
        self.with_filter_state_and_nat_ctx(|state, _ctx| cb(state))
    }

    /// Calls the function with a reference to filtering state and the NAT
    /// context.
    fn with_filter_state_and_nat_ctx<
        O,
        F: FnOnce(&State<I, Self::WeakAddressId, BT>, &mut Self::NatCtx<'_>) -> O,
    >(
        &mut self,
        cb: F,
    ) -> O;
}

/// The execution context for Network Address Translation (NAT).
pub trait NatContext<I: IpExt, BT: FilterBindingsTypes>:
    IpDeviceAddressIdContext<I, DeviceId: InterfaceProperties<BT::DeviceClass>>
{
    /// Returns the best local address for communicating with the remote.
    fn get_local_addr_for_remote(
        &mut self,
        device_id: &Self::DeviceId,
        remote: Option<SpecifiedAddr<I::Addr>>,
    ) -> Option<Self::AddressId>;

    /// Returns a strongly-held reference to the provided address, if it is assigned
    /// to the specified device.
    fn get_address_id(
        &mut self,
        device_id: &Self::DeviceId,
        addr: IpDeviceAddr<I::Addr>,
    ) -> Option<Self::AddressId>;
}

/// A context for mutably accessing all filtering state at once, to allow IPv4
/// and IPv6 filtering state to be modified atomically.
pub trait FilterContext<BT: FilterBindingsTypes>:
    IpDeviceAddressIdContext<Ipv4, DeviceId: InterfaceProperties<BT::DeviceClass>>
    + IpDeviceAddressIdContext<Ipv6, DeviceId: InterfaceProperties<BT::DeviceClass>>
{
    /// Calls the function with a mutable reference to all filtering state.
    fn with_all_filter_state_mut<
        O,
        F: FnOnce(
            &mut State<Ipv4, <Self as IpDeviceAddressIdContext<Ipv4>>::WeakAddressId, BT>,
            &mut State<Ipv6, <Self as IpDeviceAddressIdContext<Ipv6>>::WeakAddressId, BT>,
        ) -> O,
    >(
        &mut self,
        cb: F,
    ) -> O;
}

/// Result returned from [`SocketOpsFilter::on_egress`].
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SocketEgressFilterResult {
    /// Send the packet normally.
    Pass {
        /// Indicates that congestion should be signaled to the higher level protocol.
        congestion: bool,
    },

    /// Drop the packet.
    Drop {
        /// Indicates that congestion should be signaled to the higher level protocol.
        congestion: bool,
    },
}

/// Result returned from [`SocketOpsFilter::on_ingress`].
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SocketIngressFilterResult {
    /// Accept the packet.
    Accept,

    /// Drop the packet.
    Drop,
}

/// Trait for a socket operations filter.
pub trait SocketOpsFilter<D> {
    /// Called on every outgoing packet originated from a local socket.
    fn on_egress<I: FilterIpExt, P: FilterIpPacket<I>>(
        &self,
        packet: &P,
        device: &D,
        socket_info: SocketInfo,
        marks: &Marks,
    ) -> SocketEgressFilterResult;

    /// Called on every incoming packet handled by a local socket.
    fn on_ingress(
        &self,
        ip_version: IpVersion,
        packet: FragmentedByteSlice<'_, &[u8]>,
        header_len: usize,
        device: &D,
        socket_info: SocketInfo,
        marks: &Marks,
    ) -> SocketIngressFilterResult;
}

/// Implemented by bindings to provide socket operations filtering.
pub trait SocketOpsFilterBindingContext<D>: TxMetadataBindingsTypes {
    /// Returns the filter that should be called for socket ops.
    fn socket_ops_filter(&self) -> impl SocketOpsFilter<D>;
}

#[cfg(any(test, feature = "testutils"))]
impl<
    TimerId: Debug + PartialEq + Clone + Send + Sync + 'static,
    Event: Debug + 'static,
    State: 'static,
    FrameMeta: 'static,
    D,
> SocketOpsFilterBindingContext<D>
    for netstack3_base::testutil::FakeBindingsCtx<TimerId, Event, State, FrameMeta>
{
    fn socket_ops_filter(&self) -> impl SocketOpsFilter<D> {
        crate::testutil::NoOpSocketOpsFilter
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use alloc::sync::{Arc, Weak};
    use alloc::vec::Vec;
    use core::hash::{Hash, Hasher};
    use core::ops::Deref;
    use core::sync::atomic::AtomicUsize;
    use core::time::Duration;

    use derivative::Derivative;
    use net_types::ip::{AddrSubnet, GenericOverIp, Ip};
    use netstack3_base::testutil::{
        FakeAtomicInstant, FakeCryptoRng, FakeDeviceClass, FakeInstant, FakeMatcherDeviceId,
        FakeTimerCtx, FakeWeakDeviceId, WithFakeTimerContext,
    };
    use netstack3_base::{
        AnyDevice, AssignedAddrIpExt, DeviceIdContext, InspectableValue, Inspector, InstantContext,
        IntoCoreTimerCtx, IpAddressId, WeakIpAddressId,
    };
    use netstack3_hashmap::HashMap;

    use super::*;
    use crate::logic::FilterTimerId;
    use crate::logic::nat::NatConfig;
    use crate::state::validation::ValidRoutines;
    use crate::state::{FilterPacketMetadata, IpRoutines, NatRoutines, OneWayBoolean, Routines};
    use crate::{Interfaces, conntrack};

    pub trait TestIpExt: FilterIpExt + AssignedAddrIpExt {}

    impl<I: FilterIpExt + AssignedAddrIpExt> TestIpExt for I {}

    #[derive(Debug)]
    pub struct FakePrimaryAddressId<I: AssignedAddrIpExt>(
        pub Arc<AddrSubnet<I::Addr, I::AssignedWitness>>,
    );

    #[derive(Clone, Debug, Hash, Eq, PartialEq)]
    pub struct FakeAddressId<I: AssignedAddrIpExt>(Arc<AddrSubnet<I::Addr, I::AssignedWitness>>);

    #[derive(Clone, Debug)]
    pub struct FakeWeakAddressId<I: AssignedAddrIpExt>(
        pub Weak<AddrSubnet<I::Addr, I::AssignedWitness>>,
    );

    impl<I: AssignedAddrIpExt> PartialEq for FakeWeakAddressId<I> {
        fn eq(&self, other: &Self) -> bool {
            let Self(lhs) = self;
            let Self(rhs) = other;
            Weak::ptr_eq(lhs, rhs)
        }
    }

    impl<I: AssignedAddrIpExt> Eq for FakeWeakAddressId<I> {}

    impl<I: AssignedAddrIpExt> Hash for FakeWeakAddressId<I> {
        fn hash<H: Hasher>(&self, state: &mut H) {
            let Self(this) = self;
            this.as_ptr().hash(state)
        }
    }

    impl<I: AssignedAddrIpExt> WeakIpAddressId<I::Addr> for FakeWeakAddressId<I> {
        type Strong = FakeAddressId<I>;

        fn upgrade(&self) -> Option<Self::Strong> {
            let Self(inner) = self;
            inner.upgrade().map(FakeAddressId)
        }

        fn is_assigned(&self) -> bool {
            let Self(inner) = self;
            inner.strong_count() != 0
        }
    }

    impl<I: AssignedAddrIpExt> InspectableValue for FakeWeakAddressId<I> {
        fn record<Inspector: netstack3_base::Inspector>(
            &self,
            _name: &str,
            _inspector: &mut Inspector,
        ) {
            unimplemented!()
        }
    }

    impl<I: AssignedAddrIpExt> Deref for FakeAddressId<I> {
        type Target = AddrSubnet<I::Addr, I::AssignedWitness>;

        fn deref(&self) -> &Self::Target {
            let Self(inner) = self;
            inner.deref()
        }
    }

    impl<I: AssignedAddrIpExt> IpAddressId<I::Addr> for FakeAddressId<I> {
        type Weak = FakeWeakAddressId<I>;

        fn downgrade(&self) -> Self::Weak {
            let Self(inner) = self;
            FakeWeakAddressId(Arc::downgrade(inner))
        }

        fn addr(&self) -> IpDeviceAddr<I::Addr> {
            let Self(inner) = self;

            #[derive(GenericOverIp)]
            #[generic_over_ip(I, Ip)]
            struct WrapIn<I: AssignedAddrIpExt>(I::AssignedWitness);
            I::map_ip(
                WrapIn(inner.addr()),
                |WrapIn(v4_addr)| IpDeviceAddr::new_from_witness(v4_addr),
                |WrapIn(v6_addr)| IpDeviceAddr::new_from_ipv6_device_addr(v6_addr),
            )
        }

        fn addr_sub(&self) -> AddrSubnet<I::Addr, I::AssignedWitness> {
            let Self(inner) = self;
            **inner
        }
    }

    pub struct FakeCtx<I: TestIpExt> {
        state: State<I, FakeWeakAddressId<I>, FakeBindingsCtx<I>>,
        nat: FakeNatCtx<I>,
    }

    #[derive(Derivative)]
    #[derivative(Default(bound = ""))]
    pub struct FakeNatCtx<I: TestIpExt> {
        pub(crate) device_addrs: HashMap<FakeMatcherDeviceId, FakePrimaryAddressId<I>>,
    }

    impl<I: TestIpExt> FakeCtx<I> {
        pub fn new(bindings_ctx: &mut FakeBindingsCtx<I>) -> Self {
            Self {
                state: State {
                    installed_routines: ValidRoutines::default(),
                    uninstalled_routines: Vec::default(),
                    conntrack: conntrack::Table::new::<IntoCoreTimerCtx>(bindings_ctx),
                    nat_installed: OneWayBoolean::default(),
                },
                nat: FakeNatCtx::default(),
            }
        }

        pub fn with_ip_routines(
            bindings_ctx: &mut FakeBindingsCtx<I>,
            routines: IpRoutines<I, FakeBindingsCtx<I>, ()>,
        ) -> Self {
            let (installed_routines, uninstalled_routines) =
                ValidRoutines::new(Routines { ip: routines, ..Default::default() })
                    .expect("invalid state");
            Self {
                state: State {
                    installed_routines,
                    uninstalled_routines,
                    conntrack: conntrack::Table::new::<IntoCoreTimerCtx>(bindings_ctx),
                    nat_installed: OneWayBoolean::default(),
                },
                nat: FakeNatCtx::default(),
            }
        }

        pub fn with_nat_routines_and_device_addrs(
            bindings_ctx: &mut FakeBindingsCtx<I>,
            routines: NatRoutines<I, FakeBindingsCtx<I>, ()>,
            device_addrs: impl IntoIterator<
                Item = (FakeMatcherDeviceId, AddrSubnet<I::Addr, I::AssignedWitness>),
            >,
        ) -> Self {
            let (installed_routines, uninstalled_routines) =
                ValidRoutines::new(Routines { nat: routines, ..Default::default() })
                    .expect("invalid state");
            Self {
                state: State {
                    installed_routines,
                    uninstalled_routines,
                    conntrack: conntrack::Table::new::<IntoCoreTimerCtx>(bindings_ctx),
                    nat_installed: OneWayBoolean::TRUE,
                },
                nat: FakeNatCtx {
                    device_addrs: device_addrs
                        .into_iter()
                        .map(|(device, addr)| (device, FakePrimaryAddressId(Arc::new(addr))))
                        .collect(),
                },
            }
        }

        pub fn conntrack(
            &mut self,
        ) -> &conntrack::Table<I, NatConfig<I, FakeWeakAddressId<I>>, FakeBindingsCtx<I>> {
            &self.state.conntrack
        }
    }

    impl<I: TestIpExt> DeviceIdContext<AnyDevice> for FakeCtx<I> {
        type DeviceId = FakeMatcherDeviceId;
        type WeakDeviceId = FakeWeakDeviceId<FakeMatcherDeviceId>;
    }

    impl<I: TestIpExt> IpDeviceAddressIdContext<I> for FakeCtx<I> {
        type AddressId = FakeAddressId<I>;
        type WeakAddressId = FakeWeakAddressId<I>;
    }

    impl<I: TestIpExt> FilterIpContext<I, FakeBindingsCtx<I>> for FakeCtx<I> {
        type NatCtx<'a> = FakeNatCtx<I>;

        fn with_filter_state_and_nat_ctx<
            O,
            F: FnOnce(&State<I, FakeWeakAddressId<I>, FakeBindingsCtx<I>>, &mut Self::NatCtx<'_>) -> O,
        >(
            &mut self,
            cb: F,
        ) -> O {
            let Self { state, nat } = self;
            cb(state, nat)
        }
    }

    impl<I: TestIpExt> FakeNatCtx<I> {
        pub fn new(
            device_addrs: impl IntoIterator<
                Item = (FakeMatcherDeviceId, AddrSubnet<I::Addr, I::AssignedWitness>),
            >,
        ) -> Self {
            Self {
                device_addrs: device_addrs
                    .into_iter()
                    .map(|(device, addr)| (device, FakePrimaryAddressId(Arc::new(addr))))
                    .collect(),
            }
        }
    }

    impl<I: TestIpExt> DeviceIdContext<AnyDevice> for FakeNatCtx<I> {
        type DeviceId = FakeMatcherDeviceId;
        type WeakDeviceId = FakeWeakDeviceId<FakeMatcherDeviceId>;
    }

    impl<I: TestIpExt> IpDeviceAddressIdContext<I> for FakeNatCtx<I> {
        type AddressId = FakeAddressId<I>;
        type WeakAddressId = FakeWeakAddressId<I>;
    }

    impl<I: TestIpExt> NatContext<I, FakeBindingsCtx<I>> for FakeNatCtx<I> {
        fn get_local_addr_for_remote(
            &mut self,
            device_id: &Self::DeviceId,
            _remote: Option<SpecifiedAddr<I::Addr>>,
        ) -> Option<Self::AddressId> {
            let FakePrimaryAddressId(primary) = self.device_addrs.get(device_id)?;
            Some(FakeAddressId(primary.clone()))
        }

        fn get_address_id(
            &mut self,
            device_id: &Self::DeviceId,
            addr: IpDeviceAddr<I::Addr>,
        ) -> Option<Self::AddressId> {
            let FakePrimaryAddressId(id) = self.device_addrs.get(device_id)?;
            let id = FakeAddressId(id.clone());
            if id.addr() == addr { Some(id) } else { None }
        }
    }

    pub struct FakeBindingsCtx<I: Ip> {
        pub timer_ctx: FakeTimerCtx<FilterTimerId<I>>,
        pub rng: FakeCryptoRng,
    }

    impl<I: Ip> FakeBindingsCtx<I> {
        pub(crate) fn new() -> Self {
            Self { timer_ctx: FakeTimerCtx::default(), rng: FakeCryptoRng::default() }
        }

        pub(crate) fn sleep(&mut self, time_elapsed: Duration) {
            self.timer_ctx.instant.sleep(time_elapsed)
        }
    }

    impl<I: Ip> InstantBindingsTypes for FakeBindingsCtx<I> {
        type Instant = FakeInstant;
        type AtomicInstant = FakeAtomicInstant;
    }

    #[derive(Debug)]
    pub struct FakeBindingsPacketMatcher {
        num_calls: AtomicUsize,
        result: bool,
    }

    impl FakeBindingsPacketMatcher {
        pub fn new(result: bool) -> Arc<Self> {
            Arc::new(Self { num_calls: AtomicUsize::new(0), result })
        }

        pub fn num_calls(&self) -> usize {
            self.num_calls.load(core::sync::atomic::Ordering::SeqCst)
        }
    }

    impl BindingsPacketMatcher<FakeMatcherDeviceId> for FakeBindingsPacketMatcher {
        fn matches<I: FilterIpExt, P: FilterIpPacket<I>>(
            &self,
            _packet: &P,
            _interfaces: Interfaces<'_, FakeMatcherDeviceId>,
            _socket_info: &impl FilterPacketMetadata,
        ) -> bool {
            let _: usize = self.num_calls.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
            self.result
        }
    }

    impl InspectableValue for FakeBindingsPacketMatcher {
        fn record<I: Inspector>(&self, _name: &str, _inspector: &mut I) {
            unimplemented!()
        }
    }

    impl<I: Ip> MatcherBindingsTypes for FakeBindingsCtx<I> {
        type DeviceClass = FakeDeviceClass;
        type BindingsPacketMatcher = Arc<FakeBindingsPacketMatcher>;
    }

    impl<I: Ip> InstantContext for FakeBindingsCtx<I> {
        fn now(&self) -> Self::Instant {
            self.timer_ctx.now()
        }
    }

    impl<I: Ip> TimerBindingsTypes for FakeBindingsCtx<I> {
        type Timer = <FakeTimerCtx<FilterTimerId<I>> as TimerBindingsTypes>::Timer;
        type DispatchId = <FakeTimerCtx<FilterTimerId<I>> as TimerBindingsTypes>::DispatchId;
        type UniqueTimerId = <FakeTimerCtx<FilterTimerId<I>> as TimerBindingsTypes>::UniqueTimerId;
    }

    impl<I: Ip> TimerContext for FakeBindingsCtx<I> {
        fn new_timer(&mut self, id: Self::DispatchId) -> Self::Timer {
            self.timer_ctx.new_timer(id)
        }

        fn schedule_timer_instant(
            &mut self,
            time: Self::Instant,
            timer: &mut Self::Timer,
        ) -> Option<Self::Instant> {
            self.timer_ctx.schedule_timer_instant(time, timer)
        }

        fn cancel_timer(&mut self, timer: &mut Self::Timer) -> Option<Self::Instant> {
            self.timer_ctx.cancel_timer(timer)
        }

        fn scheduled_instant(&self, timer: &mut Self::Timer) -> Option<Self::Instant> {
            self.timer_ctx.scheduled_instant(timer)
        }

        fn unique_timer_id(&self, timer: &Self::Timer) -> Self::UniqueTimerId {
            self.timer_ctx.unique_timer_id(timer)
        }
    }

    impl<I: Ip> WithFakeTimerContext<FilterTimerId<I>> for FakeBindingsCtx<I> {
        fn with_fake_timer_ctx<O, F: FnOnce(&FakeTimerCtx<FilterTimerId<I>>) -> O>(
            &self,
            f: F,
        ) -> O {
            f(&self.timer_ctx)
        }

        fn with_fake_timer_ctx_mut<O, F: FnOnce(&mut FakeTimerCtx<FilterTimerId<I>>) -> O>(
            &mut self,
            f: F,
        ) -> O {
            f(&mut self.timer_ctx)
        }
    }

    impl<I: Ip> RngContext for FakeBindingsCtx<I> {
        type Rng<'a>
            = FakeCryptoRng
        where
            Self: 'a;

        fn rng(&mut self) -> Self::Rng<'_> {
            self.rng.clone()
        }
    }
}
