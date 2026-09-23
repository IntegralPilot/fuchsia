// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implementations for multicast forwarding that integrate with traits/types
//! from foreign modules.

use lock_order::lock::LockLevelFor;
use lock_order::relation::LockBefore;
use lock_order::wrap::{LockedWrapperApi, LockedWrapperUnlockedApi};
use netstack3_base::{CoreTimerContext, CounterContext};
use netstack3_device::{DeviceId, WeakDeviceId};
use netstack3_ip::IpLayerTimerId;
use netstack3_ip::multicast_forwarding::{
    MulticastForwardingCounters, MulticastForwardingEnabledState,
    MulticastForwardingPendingPackets, MulticastForwardingPendingPacketsContext,
    MulticastForwardingState, MulticastForwardingStateContext, MulticastForwardingTimerId,
    MulticastRouteTable, MulticastRouteTableContext,
};

use crate::{BindingsContext, BindingsTypes, CoreCtx, IpExt, lock_ordering};

#[netstack3_macros::instantiate_ip_impl_block(I)]
impl<I: IpExt, BC: BindingsContext, L: LockBefore<lock_ordering::IpMulticastForwardingState<I>>>
    MulticastForwardingStateContext<I, BC> for CoreCtx<'_, BC, L>
{
    type Ctx<'a> = CoreCtx<'a, BC, lock_ordering::IpMulticastForwardingState<I>>;

    fn with_state<
        O,
        F: FnOnce(&MulticastForwardingState<I, Self::DeviceId, BC>, &mut Self::Ctx<'_>) -> O,
    >(
        &mut self,
        cb: F,
    ) -> O {
        let (state, mut core_ctx) =
            self.read_lock_and::<lock_ordering::IpMulticastForwardingState<I>>();
        cb(&state, &mut core_ctx)
    }

    fn with_state_mut<
        O,
        F: FnOnce(&mut MulticastForwardingState<I, Self::DeviceId, BC>, &mut Self::Ctx<'_>) -> O,
    >(
        &mut self,
        cb: F,
    ) -> O {
        let (mut state, mut core_ctx) =
            self.write_lock_and::<lock_ordering::IpMulticastForwardingState<I>>();
        cb(&mut state, &mut core_ctx)
    }
}

#[netstack3_macros::instantiate_ip_impl_block(I)]
impl<I: IpExt, BC: BindingsContext, L: LockBefore<lock_ordering::IpMulticastRouteTable<I>>>
    MulticastRouteTableContext<I, BC> for CoreCtx<'_, BC, L>
{
    type Ctx<'a> = CoreCtx<'a, BC, lock_ordering::IpMulticastRouteTable<I>>;

    fn with_route_table<
        O,
        F: FnOnce(&MulticastRouteTable<I, Self::DeviceId, BC>, &mut Self::Ctx<'_>) -> O,
    >(
        &mut self,
        state: &MulticastForwardingEnabledState<I, Self::DeviceId, BC>,
        cb: F,
    ) -> O {
        let mut locked = self.adopt(state);
        let (route_table, mut core_ctx) =
            locked.read_lock_with_and::<lock_ordering::IpMulticastRouteTable<I>, _>(|c| c.right());
        let mut core_ctx = core_ctx.cast_core_ctx();
        cb(&route_table, &mut core_ctx)
    }

    fn with_route_table_mut<
        O,
        F: FnOnce(&mut MulticastRouteTable<I, Self::DeviceId, BC>, &mut Self::Ctx<'_>) -> O,
    >(
        &mut self,
        state: &MulticastForwardingEnabledState<I, Self::DeviceId, BC>,
        cb: F,
    ) -> O {
        let mut locked = self.adopt(state);
        let (mut route_table, mut core_ctx) =
            locked.write_lock_with_and::<lock_ordering::IpMulticastRouteTable<I>, _>(|c| c.right());
        let mut core_ctx = core_ctx.cast_core_ctx();
        cb(&mut route_table, &mut core_ctx)
    }
}

#[netstack3_macros::instantiate_ip_impl_block(I)]
impl<
    I: IpExt,
    BC: BindingsContext,
    L: LockBefore<lock_ordering::IpMulticastForwardingPendingPackets<I>>,
> MulticastForwardingPendingPacketsContext<I, BC> for CoreCtx<'_, BC, L>
{
    fn with_pending_table_mut<
        O,
        F: FnOnce(&mut MulticastForwardingPendingPackets<I, Self::WeakDeviceId, BC>) -> O,
    >(
        &mut self,
        state: &MulticastForwardingEnabledState<I, Self::DeviceId, BC>,
        cb: F,
    ) -> O {
        let mut locked = self.adopt(state);
        let mut pending_table = locked
            .lock_with::<lock_ordering::IpMulticastForwardingPendingPackets<I>, _>(|c| c.right());
        cb(&mut pending_table)
    }
}

impl<I: IpExt, BT: BindingsTypes> LockLevelFor<MulticastForwardingEnabledState<I, DeviceId<BT>, BT>>
    for lock_ordering::IpMulticastRouteTable<I>
{
    type Data = MulticastRouteTable<I, DeviceId<BT>, BT>;
}

impl<I: IpExt, BT: BindingsTypes> LockLevelFor<MulticastForwardingEnabledState<I, DeviceId<BT>, BT>>
    for lock_ordering::IpMulticastForwardingPendingPackets<I>
{
    type Data = MulticastForwardingPendingPackets<I, WeakDeviceId<BT>, BT>;
}

impl<I, BC, L> CoreTimerContext<MulticastForwardingTimerId<I>, BC> for CoreCtx<'_, BC, L>
where
    I: IpExt,
    BC: BindingsContext,
{
    fn convert_timer(dispatch_id: MulticastForwardingTimerId<I>) -> BC::DispatchId {
        IpLayerTimerId::from(dispatch_id).into()
    }
}

impl<I, BC, L> CounterContext<MulticastForwardingCounters<I>> for CoreCtx<'_, BC, L>
where
    I: IpExt,
    BC: BindingsContext,
{
    fn counters(&self) -> &MulticastForwardingCounters<I> {
        self.unlocked_access::<crate::lock_ordering::UnlockedState>()
            .inner_ip_state()
            .multicast_forwarding_counters()
    }
}
