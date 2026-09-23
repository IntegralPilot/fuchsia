// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Structs containing the entire stack state.

use lock_order::lock::UnlockedAccess;
use netstack3_base::{BuildableCoreContext, ContextProvider, CoreTimerContext, CtxPair};
use netstack3_device::{DeviceId, DeviceLayerState};
use netstack3_ip::icmp::IcmpState;
use netstack3_ip::{self as ip, IpLayerIpExt, IpLayerTimerId, IpStateInner, Ipv4State, Ipv6State};

use crate::api::CoreApi;
use crate::time::TimerId;
use crate::transport::{TransportLayerState, TransportStateBuilder};
use crate::{BindingsContext, BindingsTypes, CoreCtx};

/// A builder for [`StackState`].
#[derive(Clone, Default)]
pub struct StackStateBuilder {
    transport: TransportStateBuilder,
    ipv4: ip::Ipv4StateBuilder,
    ipv6: ip::Ipv6StateBuilder,
}

impl StackStateBuilder {
    /// Get the builder for the transport layer state.
    pub fn transport_builder(&mut self) -> &mut TransportStateBuilder {
        &mut self.transport
    }

    /// Get the builder for the IPv4 state.
    pub fn ipv4_builder(&mut self) -> &mut ip::Ipv4StateBuilder {
        &mut self.ipv4
    }

    /// Get the builder for the IPv6 state.
    pub fn ipv6_builder(&mut self) -> &mut ip::Ipv6StateBuilder {
        &mut self.ipv6
    }

    /// Consume this builder and produce a `StackState`.
    pub fn build_with_ctx<BC: BindingsContext>(self, bindings_ctx: &mut BC) -> StackState<BC> {
        StackState {
            transport: self.transport.build_with_ctx(bindings_ctx),
            ipv4: self.ipv4.build::<StackState<BC>, _, _>(bindings_ctx),
            ipv6: self.ipv6.build::<StackState<BC>, _, _>(bindings_ctx),
            device: Default::default(),
        }
    }
}

impl<BC: BindingsContext> BuildableCoreContext<BC> for StackState<BC> {
    type Builder = StackStateBuilder;
    fn build(bindings_ctx: &mut BC, builder: StackStateBuilder) -> Self {
        builder.build_with_ctx(bindings_ctx)
    }
}

/// The state associated with the network stack.
pub struct StackState<BT: BindingsTypes> {
    pub(crate) transport: TransportLayerState<BT>,
    pub(crate) ipv4: Ipv4State<DeviceId<BT>, BT>,
    pub(crate) ipv6: Ipv6State<DeviceId<BT>, BT>,
    pub(crate) device: DeviceLayerState<BT>,
}

impl<BT: BindingsTypes> StackState<BT> {
    /// Gets access to the API from a mutable reference to the bindings context.
    pub fn api<'a, BP: ContextProvider<Context = BT>>(
        &'a self,
        bindings_ctx: BP,
    ) -> CoreApi<'a, BP> {
        CoreApi::new(CtxPair { core_ctx: CoreCtx::new(self), bindings_ctx })
    }

    pub(crate) fn inner_ip_state<I: IpLayerIpExt>(&self) -> &IpStateInner<I, DeviceId<BT>, BT> {
        I::map_ip((), |()| &self.ipv4.inner, |()| &self.ipv6.inner)
    }

    pub(crate) fn inner_icmp_state<I: IpLayerIpExt>(&self) -> &IcmpState<I, BT> {
        I::map_ip((), |()| &self.ipv4.icmp.inner, |()| &self.ipv6.icmp.inner)
    }
}

// Stack state accessors for use in tests.
// We don't want bindings using this directly.
#[cfg(any(test, feature = "testutils"))]

impl<BT: BindingsTypes> StackState<BT> {
    /// Accessor for transport state.
    pub fn transport(&self) -> &TransportLayerState<BT> {
        &self.transport
    }
    /// Accessor for IPv4 state.
    pub fn ipv4(&self) -> &Ipv4State<DeviceId<BT>, BT> {
        &self.ipv4
    }
    /// Accessor for IPv6 state.
    pub fn ipv6(&self) -> &Ipv6State<DeviceId<BT>, BT> {
        &self.ipv6
    }
    /// Accessor for device state.
    pub fn device(&self) -> &DeviceLayerState<BT> {
        &self.device
    }
    /// Gets the core context.
    pub fn context(&self) -> crate::context::UnlockedCoreCtx<'_, BT> {
        crate::context::UnlockedCoreCtx::new(self)
    }
    /// Accessor for common IP state for `I`.
    pub fn common_ip<I: IpLayerIpExt>(&self) -> &IpStateInner<I, DeviceId<BT>, BT> {
        self.inner_ip_state::<I>()
    }
    /// Accessor for common ICMP state for `I`.
    pub fn common_icmp<I: IpLayerIpExt>(&self) -> &IcmpState<I, BT> {
        self.inner_icmp_state::<I>()
    }
}

impl<BT: BindingsTypes> CoreTimerContext<IpLayerTimerId, BT> for StackState<BT> {
    fn convert_timer(timer: IpLayerTimerId) -> TimerId<BT> {
        timer.into()
    }
}

/// It is safe to provide unlocked access to [`StackState`] itself here because
/// care has been taken to avoid exposing publicly to the core integration crate
/// any state that is held by a lock, as opposed to read-only state that can be
/// accessed safely at any lock level, e.g. state with no interior mutability or
/// atomics.
///
/// Access to state held by locks *must* be mediated using the global lock
/// ordering declared in [`crate::lock_ordering`].
impl<BT: BindingsTypes> UnlockedAccess<crate::lock_ordering::UnlockedState> for StackState<BT> {
    type Data = StackState<BT>;
    type Guard<'l>
        = &'l StackState<BT>
    where
        Self: 'l;

    fn access(&self) -> Self::Guard<'_> {
        &self
    }
}
