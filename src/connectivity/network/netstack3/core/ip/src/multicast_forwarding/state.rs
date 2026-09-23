// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Declares types and functionality related to multicast-forwarding state.

use alloc::collections::BTreeMap;
use derivative::Derivative;
use lock_order::lock::{OrderedLockAccess, OrderedLockRef};
use netstack3_base::sync::{Mutex, RwLock};
use netstack3_base::{
    AnyDevice, CoreTimerContext, CounterContext, DeviceIdContext, StrongDeviceIdentifier,
};

use crate::IpLayerIpExt;
use crate::internal::multicast_forwarding::counters::MulticastForwardingCounters;
use crate::internal::multicast_forwarding::packet_queue::MulticastForwardingPendingPackets;
use crate::internal::multicast_forwarding::route::{MulticastRouteEntry, MulticastRouteKey};
use crate::internal::multicast_forwarding::{
    MulticastForwardingBindingsContext, MulticastForwardingBindingsTypes,
    MulticastForwardingTimerId,
};

/// Multicast forwarding state for an IP version `I`.
///
/// Multicast forwarding can be enabled/disabled for `I` globally. When disabled
/// no state is held.
#[derive(Derivative)]
#[derivative(Debug(bound = ""), Default(bound = ""))]
pub enum MulticastForwardingState<
    I: IpLayerIpExt,
    D: StrongDeviceIdentifier,
    BT: MulticastForwardingBindingsTypes,
> {
    /// Multicast forwarding is disabled.
    #[derivative(Default)]
    Disabled,
    /// Multicast forwarding is enabled.
    Enabled(MulticastForwardingEnabledState<I, D, BT>),
}

impl<I: IpLayerIpExt, D: StrongDeviceIdentifier, BT: MulticastForwardingBindingsTypes>
    MulticastForwardingState<I, D, BT>
{
    pub(crate) fn enabled(&self) -> Option<&MulticastForwardingEnabledState<I, D, BT>> {
        match self {
            MulticastForwardingState::Disabled => None,
            MulticastForwardingState::Enabled(state) => Some(state),
        }
    }
}

/// State held by the netstack when multicast forwarding is enabled for `I`.
#[derive(Derivative)]
#[derivative(Debug(bound = ""))]
pub struct MulticastForwardingEnabledState<
    I: IpLayerIpExt,
    D: StrongDeviceIdentifier,
    BT: MulticastForwardingBindingsTypes,
> {
    /// The stack's multicast route table.
    ///
    /// Keys here must not be present in `pending_table`.
    route_table: RwLock<MulticastRouteTable<I, D, BT>>,
    /// The stack's table of pending multicast packets.
    ///
    /// Keys here must not be present in `route_table`.
    pending_table: Mutex<MulticastForwardingPendingPackets<I, D::Weak, BT>>,
}

impl<I: IpLayerIpExt, D: StrongDeviceIdentifier, BC: MulticastForwardingBindingsContext<I, D>>
    MulticastForwardingEnabledState<I, D, BC>
{
    pub(super) fn new<CC>(bindings_ctx: &mut BC) -> Self
    where
        CC: CoreTimerContext<MulticastForwardingTimerId<I>, BC>,
    {
        Self {
            route_table: Default::default(),
            pending_table: Mutex::new(MulticastForwardingPendingPackets::new::<CC>(bindings_ctx)),
        }
    }

    // Helper function to circumvent lock ordering, for tests.
    #[cfg(test)]
    pub(super) fn route_table(&self) -> &RwLock<MulticastRouteTable<I, D, BC>> {
        &self.route_table
    }
    // Helper function to circumvent lock ordering, for tests.
    #[cfg(test)]
    pub(super) fn pending_table(
        &self,
    ) -> &Mutex<MulticastForwardingPendingPackets<I, D::Weak, BC>> {
        &self.pending_table
    }
}

/// A table of multicast routes specifying how to forward multicast packets.
pub type MulticastRouteTable<I, D, BT> = BTreeMap<MulticastRouteKey<I>, MulticastRouteEntry<D, BT>>;

impl<I: IpLayerIpExt, D: StrongDeviceIdentifier, BT: MulticastForwardingBindingsTypes>
    OrderedLockAccess<MulticastRouteTable<I, D, BT>> for MulticastForwardingEnabledState<I, D, BT>
{
    type Lock = RwLock<MulticastRouteTable<I, D, BT>>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.route_table)
    }
}

impl<I: IpLayerIpExt, D: StrongDeviceIdentifier, BT: MulticastForwardingBindingsTypes>
    OrderedLockAccess<MulticastForwardingPendingPackets<I, D::Weak, BT>>
    for MulticastForwardingEnabledState<I, D, BT>
{
    type Lock = Mutex<MulticastForwardingPendingPackets<I, D::Weak, BT>>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.pending_table)
    }
}

/// A trait providing access to [`MulticastForwardingState`].
pub trait MulticastForwardingStateContext<I: IpLayerIpExt, BT: MulticastForwardingBindingsTypes>:
    DeviceIdContext<AnyDevice>
{
    /// The context available after locking the multicast forwarding state.
    type Ctx<'a>: MulticastRouteTableContext<
            I,
            BT,
            DeviceId = Self::DeviceId,
            WeakDeviceId = Self::WeakDeviceId,
        > + MulticastForwardingPendingPacketsContext<
            I,
            BT,
            DeviceId = Self::DeviceId,
            WeakDeviceId = Self::WeakDeviceId,
        > + CounterContext<MulticastForwardingCounters<I>>;
    /// Provides immutable access to the state.
    fn with_state<
        O,
        F: FnOnce(&MulticastForwardingState<I, Self::DeviceId, BT>, &mut Self::Ctx<'_>) -> O,
    >(
        &mut self,
        cb: F,
    ) -> O;
    /// Provides mutable access to the state.
    fn with_state_mut<
        O,
        F: FnOnce(&mut MulticastForwardingState<I, Self::DeviceId, BT>, &mut Self::Ctx<'_>) -> O,
    >(
        &mut self,
        cb: F,
    ) -> O;
}

/// A trait providing access to [`MulticastRouteTable`].
pub trait MulticastRouteTableContext<I: IpLayerIpExt, BT: MulticastForwardingBindingsTypes>:
    DeviceIdContext<AnyDevice>
{
    /// The context available after locking the multicast route table.
    type Ctx<'a>: MulticastForwardingPendingPacketsContext<
            I,
            BT,
            DeviceId = Self::DeviceId,
            WeakDeviceId = Self::WeakDeviceId,
        > + CounterContext<MulticastForwardingCounters<I>>;
    /// Provides immutable access to the route table.
    fn with_route_table<
        O,
        F: FnOnce(&MulticastRouteTable<I, Self::DeviceId, BT>, &mut Self::Ctx<'_>) -> O,
    >(
        &mut self,
        state: &MulticastForwardingEnabledState<I, Self::DeviceId, BT>,
        cb: F,
    ) -> O;
    /// Provides mutable access to the route table.
    fn with_route_table_mut<
        O,
        F: FnOnce(&mut MulticastRouteTable<I, Self::DeviceId, BT>, &mut Self::Ctx<'_>) -> O,
    >(
        &mut self,
        state: &MulticastForwardingEnabledState<I, Self::DeviceId, BT>,
        cb: F,
    ) -> O;
}

/// A trait providing access to [`MulticastForwardingPendingPackets`].
pub trait MulticastForwardingPendingPacketsContext<
    I: IpLayerIpExt,
    BT: MulticastForwardingBindingsTypes,
>: DeviceIdContext<AnyDevice>
{
    /// Provides mutable access to the table of pending packets.
    fn with_pending_table_mut<
        O,
        F: FnOnce(&mut MulticastForwardingPendingPackets<I, Self::WeakDeviceId, BT>) -> O,
    >(
        &mut self,
        state: &MulticastForwardingEnabledState<I, Self::DeviceId, BT>,
        cb: F,
    ) -> O;
}
