// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Group Management Protocols (GMPs).
//!
//! This module provides implementations of the Internet Group Management Protocol
//! (IGMP) and the Multicast Listener Discovery (MLD) protocol. These allow
//! hosts to join IPv4 and IPv6 multicast groups respectively.
//!
//! The term "Group Management Protocol" is defined in [RFC 4606]:
//!
//! > Due to the commonality of function, the term "Group Management Protocol",
//! > or "GMP", will be used to refer to both IGMP and MLD.
//!
//! [RFC 4606]: https://tools.ietf.org/html/rfc4604

// This macro is used by tests in both the `igmp` and `mld` modules.

/// Assert that the GMP state machine for `$group` is in the given state.
///
/// `$ctx` is a `context::testutil::FakeCtx` whose state contains a `groups:
/// MulticastGroupSet` field.
#[cfg(test)]
macro_rules! assert_gmp_state {
    ($ctx:expr, $group:expr, NonMember) => {
        assert_gmp_state!(@inner $ctx, $group, crate::internal::gmp::v1::MemberState::NonMember(_));
    };
    ($ctx:expr, $group:expr, Delaying) => {
        assert_gmp_state!(@inner $ctx, $group, crate::internal::gmp::v1::MemberState::Delaying(_));
    };
    (@inner $ctx:expr, $group:expr, $pattern:pat) => {
        assert!(matches!($ctx.state.groups().get($group).unwrap().v1().inner.as_ref().unwrap(), $pattern))
    };
}

pub(crate) mod igmp;
pub(crate) mod mld;
#[cfg(test)]
mod testutil;
mod v1;
mod v2;

use core::fmt::Debug;
use core::num::NonZeroU64;
use core::time::Duration;

use assert_matches::assert_matches;
use log::info;
use net_types::MulticastAddr;
use net_types::ip::{Ip, IpAddress, IpVersionMarker};
use netstack3_base::ref_counted_hash_map::{InsertResult, RefCountedHashMap, RemoveResult};
use netstack3_base::{
    AnyDevice, CoreTimerContext, DeviceIdContext, InspectableValue, Inspector,
    InstantBindingsTypes, LocalTimerHeap, RngContext, TimerBindingsTypes, TimerContext,
    WeakDeviceIdentifier,
};
use rand::{Rng, RngExt as _};

/// The result of joining a multicast group.
///
/// `GroupJoinResult` is the result of joining a multicast group in a
/// [`MulticastGroupSet`].
#[cfg_attr(test, derive(Debug, Eq, PartialEq))]
pub enum GroupJoinResult<O = ()> {
    /// We were not previously a member of the group, so we joined the
    /// group.
    Joined(O),
    /// We were already a member of the group, so we incremented the group's
    /// reference count.
    AlreadyMember,
}

impl<O> GroupJoinResult<O> {
    /// Maps a [`GroupJoinResult::Joined`] variant to another type.
    ///
    /// If `self` is [`GroupJoinResult::AlreadyMember`], it is left as-is.
    pub(crate) fn map<P, F: FnOnce(O) -> P>(self, f: F) -> GroupJoinResult<P> {
        match self {
            GroupJoinResult::Joined(output) => GroupJoinResult::Joined(f(output)),
            GroupJoinResult::AlreadyMember => GroupJoinResult::AlreadyMember,
        }
    }
}

impl<O> From<InsertResult<O>> for GroupJoinResult<O> {
    fn from(result: InsertResult<O>) -> Self {
        match result {
            InsertResult::Inserted(output) => GroupJoinResult::Joined(output),
            InsertResult::AlreadyPresent => GroupJoinResult::AlreadyMember,
        }
    }
}

/// The result of leaving a multicast group.
///
/// `GroupLeaveResult` is the result of leaving a multicast group in
/// [`MulticastGroupSet`].
#[cfg_attr(test, derive(Debug, Eq, PartialEq))]
pub enum GroupLeaveResult<T = ()> {
    /// The reference count reached 0, so we left the group.
    Left(T),
    /// The reference count did not reach 0, so we are still a member of the
    /// group.
    StillMember,
    /// We were not a member of the group.
    NotMember,
}

impl<T> GroupLeaveResult<T> {
    /// Maps a [`GroupLeaveResult::Left`] variant to another type.
    ///
    /// If `self` is [`GroupLeaveResult::StillMember`] or
    /// [`GroupLeaveResult::NotMember`], it is left as-is.
    pub(crate) fn map<U, F: FnOnce(T) -> U>(self, f: F) -> GroupLeaveResult<U> {
        match self {
            GroupLeaveResult::Left(value) => GroupLeaveResult::Left(f(value)),
            GroupLeaveResult::StillMember => GroupLeaveResult::StillMember,
            GroupLeaveResult::NotMember => GroupLeaveResult::NotMember,
        }
    }
}

impl<T> From<RemoveResult<T>> for GroupLeaveResult<T> {
    fn from(result: RemoveResult<T>) -> Self {
        match result {
            RemoveResult::Removed(value) => GroupLeaveResult::Left(value),
            RemoveResult::StillPresent => GroupLeaveResult::StillMember,
            RemoveResult::NotPresent => GroupLeaveResult::NotMember,
        }
    }
}

/// A set of reference-counted multicast groups and associated data.
///
/// `MulticastGroupSet` is a set of multicast groups, each with associated data
/// `T`. Each group is reference-counted, only being removed once its reference
/// count reaches zero.
#[cfg_attr(test, derive(Debug))]
pub struct MulticastGroupSet<A: IpAddress, T> {
    inner: RefCountedHashMap<MulticastAddr<A>, T>,
}

impl<A: IpAddress, T> Default for MulticastGroupSet<A, T> {
    fn default() -> MulticastGroupSet<A, T> {
        MulticastGroupSet { inner: RefCountedHashMap::default() }
    }
}

impl<A: IpAddress, T> MulticastGroupSet<A, T> {
    fn groups_mut(&mut self) -> impl Iterator<Item = (&MulticastAddr<A>, &mut T)> + '_ {
        self.inner.iter_mut()
    }

    fn join_group_with<O, F: FnOnce() -> (T, O)>(
        &mut self,
        group: MulticastAddr<A>,
        f: F,
    ) -> GroupJoinResult<O> {
        self.inner.insert_with(group, f).into()
    }

    fn leave_group(&mut self, group: MulticastAddr<A>) -> GroupLeaveResult<T> {
        self.inner.remove(group).into()
    }

    /// Does the set contain the given group?
    pub(crate) fn contains(&self, group: &MulticastAddr<A>) -> bool {
        self.inner.contains_key(group)
    }

    #[cfg(test)]
    fn get(&self, group: &MulticastAddr<A>) -> Option<&T> {
        self.inner.get(group)
    }

    fn get_mut(&mut self, group: &MulticastAddr<A>) -> Option<&mut T> {
        self.inner.get_mut(group)
    }

    fn iter_mut<'a>(&'a mut self) -> impl 'a + Iterator<Item = (&'a MulticastAddr<A>, &'a mut T)> {
        self.inner.iter_mut()
    }

    fn iter<'a>(&'a self) -> impl 'a + Iterator<Item = (&'a MulticastAddr<A>, &'a T)> + Clone {
        self.inner.iter()
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl<A: IpAddress, T> InspectableValue for MulticastGroupSet<A, T> {
    fn record<I: Inspector>(&self, name: &str, inspector: &mut I) {
        inspector.record_child(name, |inspector| {
            for (addr, ref_count) in self.inner.iter_ref_counts() {
                inspector.record_display_child(addr, |inspector| {
                    inspector.record_usize("Refs", ref_count.get())
                });
            }
        });
    }
}

/// An implementation of query operations on a Group Management Protocol (GMP).
pub trait GmpQueryHandler<I: Ip, BC>: DeviceIdContext<AnyDevice> {
    /// Returns true if the device is a member of the group.
    fn gmp_is_in_group(
        &mut self,
        device: &Self::DeviceId,
        group_addr: MulticastAddr<I::Addr>,
    ) -> bool;
}

/// An implementation of a Group Management Protocol (GMP) such as the Internet
/// Group Management Protocol, Version 2 (IGMPv2) for IPv4 or the Multicast
/// Listener Discovery (MLD) protocol for IPv6.
pub trait GmpHandler<I: IpExt, BC>: DeviceIdContext<AnyDevice> {
    /// Handles GMP potentially being enabled.
    ///
    /// Attempts to transition memberships in the non-member state to a member
    /// state. Should be called anytime a configuration change occurs which
    /// results in GMP potentially being enabled. E.g. when IP or GMP
    /// transitions to being enabled.
    ///
    /// This method is idempotent, once into the enabled state future calls are
    /// no-ops.
    fn gmp_handle_maybe_enabled(&mut self, bindings_ctx: &mut BC, device: &Self::DeviceId);

    /// Handles GMP being disabled.
    ///
    /// All joined groups will transition to the non-member state but still
    /// remain locally joined.
    ///
    /// This method is idempotent, once into the disabled state future calls are
    /// no-ops.
    fn gmp_handle_disabled(&mut self, bindings_ctx: &mut BC, device: &Self::DeviceId);

    /// Joins the given multicast group.
    fn gmp_join_group(
        &mut self,
        bindings_ctx: &mut BC,
        device: &Self::DeviceId,
        group_addr: MulticastAddr<I::Addr>,
    ) -> GroupJoinResult;

    /// Leaves the given multicast group.
    fn gmp_leave_group(
        &mut self,
        bindings_ctx: &mut BC,
        device: &Self::DeviceId,
        group_addr: MulticastAddr<I::Addr>,
    ) -> GroupLeaveResult;

    /// Returns the current protocol mode.
    fn gmp_get_mode(&mut self, device: &Self::DeviceId) -> I::GmpProtoConfigMode;

    /// Sets the new user-configured protocol mode.
    ///
    /// Returns the previous mode. No packets are sent in response to switching
    /// modes.
    fn gmp_set_mode(
        &mut self,
        bindings_ctx: &mut BC,
        device: &Self::DeviceId,
        new_mode: I::GmpProtoConfigMode,
    ) -> I::GmpProtoConfigMode;
}

impl<I: IpExt, BT: GmpBindingsTypes, CC: GmpStateContext<I, BT>> GmpQueryHandler<I, BT> for CC {
    fn gmp_is_in_group(
        &mut self,
        device: &Self::DeviceId,
        group_addr: MulticastAddr<I::Addr>,
    ) -> bool {
        self.with_multicast_groups(device, |groups| groups.contains(&group_addr))
    }
}

impl<I: IpExt, BC: GmpBindingsContext, CC: GmpContext<I, BC>> GmpHandler<I, BC> for CC {
    fn gmp_handle_maybe_enabled(&mut self, bindings_ctx: &mut BC, device: &Self::DeviceId) {
        self.with_gmp_state_mut_and_ctx(device, |mut core_ctx, state| {
            if !state.enabled {
                return;
            }
            // Update enablement state tracking.
            match core::mem::replace(
                &mut state.gmp.enablement_idempotency_guard,
                LastState::Enabled,
            ) {
                LastState::Disabled => {}
                LastState::Enabled => {
                    // Do nothing if we were already enabled.
                    return;
                }
            }

            match state.gmp.gmp_mode() {
                GmpMode::V1 { compat: _ } => {
                    v1::handle_enabled(&mut core_ctx, bindings_ctx, device, state);
                }
                GmpMode::V2 => {
                    v2::handle_enabled(bindings_ctx, state);
                }
            }
        })
    }

    fn gmp_handle_disabled(&mut self, bindings_ctx: &mut BC, device: &Self::DeviceId) {
        self.with_gmp_state_mut_and_ctx(device, |mut core_ctx, mut state| {
            assert!(!state.enabled, "handle_disabled called with enabled GMP state");
            // Update enablement state tracking.
            match core::mem::replace(
                &mut state.gmp.enablement_idempotency_guard,
                LastState::Disabled,
            ) {
                LastState::Enabled => {}
                LastState::Disabled => {
                    // Do nothing if we were already disabled.
                    return;
                }
            }

            match state.gmp.gmp_mode() {
                GmpMode::V1 { .. } => {
                    v1::handle_disabled(&mut core_ctx, bindings_ctx, device, state.as_mut());
                }
                GmpMode::V2 => {
                    v2::handle_disabled(&mut core_ctx, bindings_ctx, device, state.as_mut());
                }
            }
            // Ask protocol which mode to enter on disable.
            let next_mode =
                <CC::Inner<'_> as GmpContextInner<I, BC>>::mode_on_disable(&state.gmp.mode);
            enter_mode(bindings_ctx, state.as_mut(), next_mode);
            // Always reset v2 protocol state when disabled, regardless of which
            // mode we're in.
            state.gmp.v2_proto = Default::default();
            // Always clear all timers on disable.
            state.gmp.timers.clear(bindings_ctx);
        })
    }

    fn gmp_join_group(
        &mut self,
        bindings_ctx: &mut BC,
        device: &CC::DeviceId,
        group_addr: MulticastAddr<I::Addr>,
    ) -> GroupJoinResult {
        self.with_gmp_state_mut_and_ctx(device, |mut core_ctx, state| match state.gmp.gmp_mode() {
            GmpMode::V1 { compat: _ } => {
                v1::join_group(&mut core_ctx, bindings_ctx, device, group_addr, state)
            }
            GmpMode::V2 => v2::join_group(bindings_ctx, group_addr, state),
        })
    }

    fn gmp_leave_group(
        &mut self,
        bindings_ctx: &mut BC,
        device: &CC::DeviceId,
        group_addr: MulticastAddr<I::Addr>,
    ) -> GroupLeaveResult {
        self.with_gmp_state_mut_and_ctx(device, |mut core_ctx, state| match state.gmp.gmp_mode() {
            GmpMode::V1 { compat: _ } => {
                v1::leave_group(&mut core_ctx, bindings_ctx, device, group_addr, state)
            }
            GmpMode::V2 => v2::leave_group(bindings_ctx, group_addr, state),
        })
    }

    fn gmp_get_mode(&mut self, device: &CC::DeviceId) -> I::GmpProtoConfigMode {
        self.with_gmp_state_mut(device, |state| {
            <CC::Inner<'_> as GmpContextInner<I, BC>>::mode_to_config(&state.gmp.mode)
        })
    }

    fn gmp_set_mode(
        &mut self,
        bindings_ctx: &mut BC,
        device: &CC::DeviceId,
        new_mode: I::GmpProtoConfigMode,
    ) -> I::GmpProtoConfigMode {
        self.with_gmp_state_mut(device, |state| {
            let old_mode =
                <CC::Inner<'_> as GmpContextInner<I, BC>>::mode_to_config(&state.gmp.mode);
            info!("GMP({}) mode change by user from {:?} to {:?}", I::NAME, old_mode, new_mode);
            let new_mode = <CC::Inner<'_> as GmpContextInner<I, BC>>::config_to_mode(
                &state.gmp.mode,
                new_mode,
            );
            enter_mode(bindings_ctx, state, new_mode);
            old_mode
        })
    }
}

/// Randomly generates a timeout in (0, period].
fn random_report_timeout<R: Rng>(rng: &mut R, period: Duration) -> Duration {
    let micros = if let Some(micros) =
        NonZeroU64::new(u64::try_from(period.as_micros()).unwrap_or(u64::MAX))
    {
        // NB: gen_range panics if the range is empty, this must be inclusive
        // end.
        rng.random_range(1..=micros.get())
    } else {
        1
    };
    // u64 will be enough here because the only input of the function is from
    // the `MaxRespTime` field of the GMP query packets. The representable
    // number of microseconds is bounded by 2^33.
    Duration::from_micros(micros)
}

/// A timer ID for GMP to send a report.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
pub struct GmpTimerId<I: Ip, D: WeakDeviceIdentifier> {
    pub(crate) device: D,
    pub(crate) _marker: IpVersionMarker<I>,
}

impl<I: Ip, D: WeakDeviceIdentifier> GmpTimerId<I, D> {
    fn device_id(&self) -> &D {
        let Self { device, _marker: IpVersionMarker { .. } } = self;
        device
    }

    const fn new(device: D) -> Self {
        Self { device, _marker: IpVersionMarker::new() }
    }
}

/// The bindings types for GMP.
pub trait GmpBindingsTypes: InstantBindingsTypes + TimerBindingsTypes {}
impl<BT> GmpBindingsTypes for BT where BT: InstantBindingsTypes + TimerBindingsTypes {}

/// The bindings execution context for GMP.
pub trait GmpBindingsContext: RngContext + TimerContext + GmpBindingsTypes {}
impl<BC> GmpBindingsContext for BC where BC: RngContext + TimerContext + GmpBindingsTypes {}

/// An extension trait to [`Ip`].
pub trait IpExt: Ip {
    /// The user-controllable mode configuration.
    type GmpProtoConfigMode: Debug + Copy + Clone + Eq + PartialEq;

    /// Returns true iff GMP should be performed for the multicast group.
    fn should_perform_gmp(addr: MulticastAddr<Self::Addr>) -> bool;
}

/// The timer id kept in [`GmpState`]'s local timer heap.
#[derive(Debug, Eq, PartialEq, Hash, Clone)]
enum TimerIdInner<I: Ip> {
    /// Timers scheduled by the v1 state machine.
    V1(v1::DelayedReportTimerId<I>),
    /// V1 compatibility mode exit timer.
    V1Compat,
    V2(v2::TimerId<I>),
}

impl<I: Ip> From<v1::DelayedReportTimerId<I>> for TimerIdInner<I> {
    fn from(value: v1::DelayedReportTimerId<I>) -> Self {
        Self::V1(value)
    }
}

impl<I: Ip> From<v2::TimerId<I>> for TimerIdInner<I> {
    fn from(value: v2::TimerId<I>) -> Self {
        Self::V2(value)
    }
}

/// Generic group management state.
#[cfg_attr(test, derive(Debug))]
pub struct GmpState<I: Ip, CC: GmpTypeLayout<I, BT>, BT: GmpBindingsTypes> {
    timers: LocalTimerHeap<TimerIdInner<I>, (), BT>,
    mode: CC::ProtoMode,
    v2_proto: v2::ProtocolState<I>,
    /// Keeps track of interface-wide enablement state.
    ///
    /// In [`v1`] each group keeps track of whether it's tracked by GMP or not,
    /// but in [`v2`] we need to keep track of an interface-wide enabled state
    /// to be able to handle `disable` only once. This is necessary because the
    /// IP layer may call [`GmpHandler::gmp_handle_disabled`] multiple times
    /// (since the interface-wide enabled state is an `and` of IP enabled and
    /// GMP enabled) and we should avoid sending leave messages on
    /// [`v2::handle_disabled`] multiple times.
    ///
    /// Note that no assumptions can be made between this flag and
    /// [`GmpStateRef::enabled`], since part of that state is held in a
    /// different lock. This exists _only_ to allow idempotency in
    /// [`GmpHandler::gmp_handle_maybe_enabled`] and
    /// [`GmpHandler::gmp_handle_disabled`].
    enablement_idempotency_guard: LastState,
}

/// Supports [`GmpState::enablement_idempotency_guard`].
#[cfg_attr(test, derive(Debug))]
enum LastState {
    Disabled,
    Enabled,
}

impl LastState {
    fn from_enabled(enabled: bool) -> Self {
        if enabled { Self::Enabled } else { Self::Disabled }
    }
}

// NB: This block is not bound on GmpBindingsContext because we don't need
// RngContext to construct GmpState.
impl<I: Ip, T: GmpTypeLayout<I, BC>, BC: GmpBindingsTypes + TimerContext> GmpState<I, T, BC> {
    /// Constructs a new `GmpState` for `device`.
    pub fn new<D: WeakDeviceIdentifier, CC: CoreTimerContext<GmpTimerId<I, D>, BC>>(
        bindings_ctx: &mut BC,
        device: D,
    ) -> Self {
        Self::new_with_enabled_and_mode::<D, CC>(bindings_ctx, device, false, Default::default())
    }

    /// Constructs a new `GmpState` for `device` assuming initial enabled state
    /// `enabled` and `mode`.
    ///
    /// This is meant to be called directly only in test scenarios (besides
    /// helping implement [`GmpState::new`] that is) where to decrease test
    /// verbosity `GmpState` can be created in a state that assumes
    /// [`GmpHandler::gmp_handle_maybe_enabled`] was called.
    fn new_with_enabled_and_mode<
        D: WeakDeviceIdentifier,
        CC: CoreTimerContext<GmpTimerId<I, D>, BC>,
    >(
        bindings_ctx: &mut BC,
        device: D,
        enabled: bool,
        mode: T::ProtoMode,
    ) -> Self {
        Self {
            timers: LocalTimerHeap::new_with_context::<_, CC>(
                bindings_ctx,
                GmpTimerId::new(device),
            ),
            mode,
            v2_proto: Default::default(),
            enablement_idempotency_guard: LastState::from_enabled(enabled),
        }
    }
}

impl<I: IpExt, T: GmpTypeLayout<I, BT>, BT: GmpBindingsTypes> GmpState<I, T, BT> {
    fn gmp_mode(&self) -> GmpMode {
        self.mode.into()
    }

    pub(crate) fn mode(&self) -> &T::ProtoMode {
        &self.mode
    }
}

/// A reference to a device's GMP state.
pub struct GmpStateRef<'a, I: IpExt, CC: GmpTypeLayout<I, BT>, BT: GmpBindingsTypes> {
    /// True if GMP is enabled for the device.
    pub enabled: bool,
    /// Mutable reference to the multicast groups on a device.
    pub groups: &'a mut MulticastGroupSet<I::Addr, GmpGroupState<I, BT>>,
    /// Mutable reference to the device's GMP state.
    pub gmp: &'a mut GmpState<I, CC, BT>,
    /// Protocol specific configuration.
    pub config: &'a CC::Config,
}

impl<'a, I: IpExt, CC: GmpTypeLayout<I, BT>, BT: GmpBindingsTypes> GmpStateRef<'a, I, CC, BT> {
    fn as_mut(&mut self) -> GmpStateRef<'_, I, CC, BT> {
        let Self { enabled, groups, gmp, config } = self;
        GmpStateRef { enabled: *enabled, groups, gmp, config }
    }
}

/// Provides IP-specific associated types for GMP.
pub trait GmpTypeLayout<I: Ip, BT: GmpBindingsTypes>: Sized {
    /// The type for protocol-specific configs.
    type Config: Debug + v1::ProtocolConfig + v2::ProtocolConfig;
    /// Protocol-specific mode.
    type ProtoMode: Debug
        + Copy
        + Clone
        + Eq
        + PartialEq
        + Into<GmpMode>
        + Default
        + InspectableValue;
}

/// The state kept by each muitlcast group the host is a member of.
pub struct GmpGroupState<I: Ip, BT: GmpBindingsTypes> {
    version_specific: GmpGroupStateByVersion<I, BT>,
    // TODO(https://fxbug.dev/381241191): When we support SSM, each group should
    // keep track of the source interest and filter modes.
}

impl<I: Ip, BT: GmpBindingsTypes> GmpGroupState<I, BT> {
    /// Retrieves a mutable borrow to the v1 state machine value.
    ///
    /// # Panics
    ///
    /// Panics if the state machine is not in the v1 state. When switching
    /// modes, GMP is responsible for updating all group states to the
    /// appropriate version.
    fn v1_mut(&mut self) -> &mut v1::GmpStateMachine<BT::Instant> {
        match &mut self.version_specific {
            GmpGroupStateByVersion::V1(v1) => return v1,
            GmpGroupStateByVersion::V2(_) => {
                panic!("expected GMP v1")
            }
        }
    }

    /// Retrieves a mutable borrow to the v2 state machine value.
    ///
    /// # Panics
    ///
    /// Panics if the state machine is not in the v2 state. When switching
    /// modes, GMP is responsible for updating all group states to the
    /// appropriate version.
    fn v2_mut(&mut self) -> &mut v2::GroupState<I> {
        match &mut self.version_specific {
            GmpGroupStateByVersion::V2(v2) => return v2,
            GmpGroupStateByVersion::V1(_) => {
                panic!("expected GMP v2")
            }
        }
    }

    /// Like [`GmpGroupState::v1_mut`] but returns a non mutable borrow.
    #[cfg(test)]
    fn v1(&self) -> &v1::GmpStateMachine<BT::Instant> {
        match &self.version_specific {
            GmpGroupStateByVersion::V1(v1) => v1,
            GmpGroupStateByVersion::V2(_) => panic!("group not in v1 mode"),
        }
    }

    /// Like [`GmpGroupState::v2_mut`] but returns a non mutable borrow.
    fn v2(&self) -> &v2::GroupState<I> {
        match &self.version_specific {
            GmpGroupStateByVersion::V2(v2) => v2,
            GmpGroupStateByVersion::V1 { .. } => panic!("group not in v2 mode"),
        }
    }

    /// Equivalent to [`GmpGroupState::v1_mut`] but drops all remaining state.
    ///
    /// # Panics
    ///
    /// See [`GmpGroupState::v1`].
    fn into_v1(self) -> v1::GmpStateMachine<BT::Instant> {
        let Self { version_specific } = self;
        match version_specific {
            GmpGroupStateByVersion::V1(v1) => v1,
            GmpGroupStateByVersion::V2(_) => panic!("expected GMP v1"),
        }
    }

    /// Equivalent to [`GmpGroupState::v2_mut`] but drops all remaining state.
    ///
    /// # Panics
    ///
    /// See [`GmpGroupState::v2`].
    fn into_v2(self) -> v2::GroupState<I> {
        let Self { version_specific } = self;
        match version_specific {
            GmpGroupStateByVersion::V2(v2) => v2,
            GmpGroupStateByVersion::V1(_) => panic!("expected GMP v2"),
        }
    }

    /// Creates a new `GmpGroupState` with associated v1 state machine.
    fn new_v1(v1: v1::GmpStateMachine<BT::Instant>) -> Self {
        Self { version_specific: GmpGroupStateByVersion::V1(v1) }
    }

    /// Creates a new `GmpGroupState` with associated v2 state.
    fn new_v2(v2: v2::GroupState<I>) -> Self {
        Self { version_specific: GmpGroupStateByVersion::V2(v2) }
    }
}

/// GMP Compatibility mode.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Default)]
pub enum GmpMode {
    /// GMP operating in v1 mode. This is the mode supporting MLDv1 and IGMPv2.
    V1 {
        /// Compat indicates v1 mode behavior.
        ///
        /// It is `true` if v1 is entered due to receiving a query from a v1
        /// router. It is `false` if v1 is entered due to administrative action
        /// (i.e. "forcing" v1 mode).
        ///
        /// It effectively controls whether we can revert back to v2 on
        /// interface disable <-> re-enable.
        compat: bool,
    },
    /// GMP operating in v2 mode. This is the mode supporting MLDv2 and IGMPv3.
    #[default]
    V2,
}

impl GmpMode {
    fn is_v1(&self) -> bool {
        match self {
            Self::V1 { .. } => true,
            Self::V2 => false,
        }
    }

    fn is_v2(&self) -> bool {
        match self {
            Self::V2 => true,
            Self::V1 { .. } => false,
        }
    }

    fn maybe_enter_v1_compat(&self) -> Self {
        match self {
            // Enter v1 compat if in v2.
            Self::V2 => Self::V1 { compat: true },
            // Maintain compat value.
            m @ Self::V1 { .. } => *m,
        }
    }

    fn maybe_exit_v1_compat(&self) -> Self {
        match self {
            // Maintain mode if not in compat.
            m @ Self::V2 | m @ Self::V1 { compat: false } => *m,
            // Exit compat mode.
            Self::V1 { compat: true } => Self::V2,
        }
    }
}

#[cfg_attr(test, derive(derivative::Derivative))]
#[cfg_attr(test, derivative(Debug(bound = "")))]
enum GmpGroupStateByVersion<I: Ip, BT: GmpBindingsTypes> {
    V1(v1::GmpStateMachine<BT::Instant>),
    V2(v2::GroupState<I>),
}

/// Provides immutable access to GMP state.
pub trait GmpStateContext<I: IpExt, BT: GmpBindingsTypes>: DeviceIdContext<AnyDevice> {
    /// The types used by this context.
    type TypeLayout: GmpTypeLayout<I, BT>;

    /// Calls the function with immutable access to the [`MulticastGroupSet`].
    fn with_multicast_groups<
        O,
        F: FnOnce(&MulticastGroupSet<I::Addr, GmpGroupState<I, BT>>) -> O,
    >(
        &mut self,
        device: &Self::DeviceId,
        cb: F,
    ) -> O {
        self.with_gmp_state(device, |groups, _gmp_state| cb(groups))
    }

    /// Calls the function with immutable access to the [`MulticastGroupSet`]
    /// and current GMP state.
    fn with_gmp_state<
        O,
        F: FnOnce(
            &MulticastGroupSet<I::Addr, GmpGroupState<I, BT>>,
            &GmpState<I, Self::TypeLayout, BT>,
        ) -> O,
    >(
        &mut self,
        device: &Self::DeviceId,
        cb: F,
    ) -> O;
}

/// Provides common functionality for GMP context implementations.
///
/// This trait implements portions of a group management protocol.
trait GmpContext<I: IpExt, BC: GmpBindingsContext>: DeviceIdContext<AnyDevice> {
    /// The types used by this context.
    type TypeLayout: GmpTypeLayout<I, BC>;

    /// The inner context given to `with_gmp_state_mut_and_ctx`.
    type Inner<'a>: GmpContextInner<I, BC, TypeLayout = Self::TypeLayout, DeviceId = Self::DeviceId>
        + 'a;

    /// Calls the function with mutable access to GMP state in [`GmpStateRef`]
    /// and access to a [`GmpContextInner`] context.
    fn with_gmp_state_mut_and_ctx<
        O,
        F: FnOnce(Self::Inner<'_>, GmpStateRef<'_, I, Self::TypeLayout, BC>) -> O,
    >(
        &mut self,
        device: &Self::DeviceId,
        cb: F,
    ) -> O;

    /// Calls the function with mutable access to GMP state in [`GmpStateRef`].
    fn with_gmp_state_mut<O, F: FnOnce(GmpStateRef<'_, I, Self::TypeLayout, BC>) -> O>(
        &mut self,
        device: &Self::DeviceId,
        cb: F,
    ) -> O {
        self.with_gmp_state_mut_and_ctx(device, |_core_ctx, state| cb(state))
    }
}

/// The inner GMP context.
///
/// Provides access to external actions while holding the GMP state lock.
trait GmpContextInner<I: IpExt, BC: GmpBindingsContext>: DeviceIdContext<AnyDevice> {
    /// The types used by this context,
    type TypeLayout: GmpTypeLayout<I, BC>;

    /// Sends a GMPv1 message.
    fn send_message_v1(
        &mut self,
        bindings_ctx: &mut BC,
        device: &Self::DeviceId,
        cur_mode: &<Self::TypeLayout as GmpTypeLayout<I, BC>>::ProtoMode,
        group_addr: GmpEnabledGroup<I::Addr>,
        msg_type: v1::GmpMessageType,
    );

    /// Sends a GMPv2 report message.
    fn send_report_v2(
        &mut self,
        bindings_ctx: &mut BC,
        device: &Self::DeviceId,
        groups: impl Iterator<Item: v2::VerifiedReportGroupRecord<I::Addr> + Clone> + Clone,
    );

    /// Returns the compatibility mode to enter upon observing `query` with
    /// `config`.
    fn mode_update_from_v1_query<Q: v1::QueryMessage<I>>(
        &mut self,
        bindings_ctx: &mut BC,
        query: &Q,
        gmp_state: &GmpState<I, Self::TypeLayout, BC>,
        config: &<Self::TypeLayout as GmpTypeLayout<I, BC>>::Config,
    ) -> <Self::TypeLayout as GmpTypeLayout<I, BC>>::ProtoMode;

    /// Returns the current operating mode as the user configuration value from
    /// the current generic GMP mode + protocol state.
    fn mode_to_config(
        mode: &<Self::TypeLayout as GmpTypeLayout<I, BC>>::ProtoMode,
    ) -> I::GmpProtoConfigMode;

    /// Returns the new mode to enter based on the current mode `cur_mode` and
    /// user-provided configuration `config`.
    fn config_to_mode(
        cur_mode: &<Self::TypeLayout as GmpTypeLayout<I, BC>>::ProtoMode,
        config: I::GmpProtoConfigMode,
    ) -> <Self::TypeLayout as GmpTypeLayout<I, BC>>::ProtoMode;

    /// Returns the new mode to use as a result of disabling GMP on an
    /// interface.
    fn mode_on_disable(
        cur_mode: &<Self::TypeLayout as GmpTypeLayout<I, BC>>::ProtoMode,
    ) -> <Self::TypeLayout as GmpTypeLayout<I, BC>>::ProtoMode;

    /// The mode to enter when GMP's compat timer fires.
    ///
    /// The returned mode *MUST* be [`GmpMode::V2`] when converted.
    fn mode_on_exit_compat() -> <Self::TypeLayout as GmpTypeLayout<I, BC>>::ProtoMode;
}

fn handle_timer<I, BC, CC>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    timer: GmpTimerId<I, CC::WeakDeviceId>,
) where
    BC: GmpBindingsContext,
    CC: GmpContext<I, BC>,
    I: IpExt,
{
    let GmpTimerId { device, _marker: IpVersionMarker { .. } } = timer;
    let Some(device) = device.upgrade() else {
        return;
    };
    core_ctx.with_gmp_state_mut_and_ctx(&device, |mut core_ctx, state| {
        let Some((timer_id, ())) = state.gmp.timers.pop(bindings_ctx) else {
            return;
        };
        // No timers should be firing if the state is disabled.
        assert!(state.enabled, "{timer_id:?} fired in GMP disabled state");

        match (timer_id, state.gmp.gmp_mode()) {
            (TimerIdInner::V1(v1), GmpMode::V1 { .. }) => {
                v1::handle_timer(&mut core_ctx, bindings_ctx, &device, state, v1);
            }
            (TimerIdInner::V1Compat, GmpMode::V1 { compat: true }) => {
                let mode = <CC::Inner<'_> as GmpContextInner<I, BC>>::mode_on_exit_compat();
                debug_assert_eq!(mode.into(), GmpMode::V2);
                enter_mode(bindings_ctx, state, mode);
            }
            (TimerIdInner::V2(timer), GmpMode::V2) => {
                v2::handle_timer(&mut core_ctx, bindings_ctx, &device, timer, state);
            }
            (TimerIdInner::V1Compat, bad) => {
                panic!("v1 compat timer fired in non v1 compat mode: {bad:?}")
            }
            bad @ (TimerIdInner::V1(_), GmpMode::V2)
            | bad @ (TimerIdInner::V2(_), GmpMode::V1 { .. }) => {
                panic!("incompatible timer fired {bad:?}")
            }
        }
    });
}

/// Enters `mode` in the state referenced by `state`.
///
/// Mode changes cause all timers to be canceled, and the group state to be
/// updated to the appropriate GMP version.
///
/// No-op if `new_mode` is current.
fn enter_mode<I: IpExt, CC: GmpTypeLayout<I, BC>, BC: GmpBindingsContext>(
    bindings_ctx: &mut BC,
    state: GmpStateRef<'_, I, CC, BC>,
    new_mode: CC::ProtoMode,
) {
    let GmpStateRef { enabled: _, gmp, groups, config: _ } = state;
    let old_mode = core::mem::replace(&mut gmp.mode, new_mode);
    match (old_mode.into(), gmp.gmp_mode()) {
        (GmpMode::V1 { compat }, GmpMode::V1 { compat: new_compat }) => {
            if new_compat != compat {
                // While in v1 mode, we only allow exiting compat mode, not entering
                // it again.
                assert_eq!(new_compat, false, "attempted to enter compatibility mode from forced");
                // Deschedule the compatibility mode exit timer.
                assert_matches!(
                    gmp.timers.cancel(bindings_ctx, &TimerIdInner::V1Compat),
                    Some((_, ()))
                );
                info!("GMP({}) enter mode {:?}", I::NAME, gmp.mode);
            }
            return;
        }
        (GmpMode::V2, GmpMode::V2) => {
            // Same mode.
            return;
        }
        (GmpMode::V1 { compat: _ }, GmpMode::V2) => {
            // Transition to v2.
            //
            // Update the group state in each group to a default value which
            // will not trigger any unsolicited reports and is ready to respond
            // to incoming queries.
            for (_, GmpGroupState { version_specific }) in groups.iter_mut() {
                *version_specific =
                    GmpGroupStateByVersion::V2(v2::GroupState::new_for_mode_transition())
            }
        }
        (GmpMode::V2, GmpMode::V1 { compat: _ }) => {
            // Transition to v1.
            //
            // Update the state machine in each group to the appropriate idle
            // definition in GMPv1. This is a state with no timers that just
            // waits to respond to queries.
            for (_, GmpGroupState { version_specific }) in groups.iter_mut() {
                *version_specific =
                    GmpGroupStateByVersion::V1(v1::GmpStateMachine::new_for_mode_transition())
            }
            gmp.v2_proto.on_enter_v1();
        }
    };
    info!("GMP({}) enter mode {:?}", I::NAME, new_mode);
    gmp.timers.clear(bindings_ctx);
    gmp.mode = new_mode;
}

fn schedule_v1_compat<I: IpExt, CC: GmpTypeLayout<I, BC>, BC: GmpBindingsContext>(
    bindings_ctx: &mut BC,
    state: GmpStateRef<'_, I, CC, BC>,
) {
    let GmpStateRef { gmp, config, enabled, .. } = state;
    assert!(enabled, "should not schedule v1 compat timer when disabled");
    let timeout = gmp.v2_proto.older_version_querier_present_timeout(config);
    let _: Option<_> =
        gmp.timers.schedule_after(bindings_ctx, TimerIdInner::V1Compat, (), timeout.into());
}

/// Error returned when operating queries but the host is not a member.
#[cfg_attr(test, derive(Debug, Eq, PartialEq))]
struct NotAMemberErr<I: Ip>(I::Addr);

/// The group targeted in a query message.
enum QueryTarget<A> {
    Unspecified,
    Specified(MulticastAddr<A>),
}

impl<A: IpAddress> QueryTarget<A> {
    fn new(addr: A) -> Option<Self> {
        if addr == <A::Version as Ip>::UNSPECIFIED_ADDRESS {
            Some(Self::Unspecified)
        } else {
            MulticastAddr::new(addr).map(Self::Specified)
        }
    }
}

mod witness {
    use super::*;

    /// A witness type for an IP multicast address that passes
    /// [`IpExt::should_perform_gmp`].
    #[derive(Debug, Eq, PartialEq, Copy, Clone, Hash)]
    pub(super) struct GmpEnabledGroup<A>(MulticastAddr<A>);

    impl<A: IpAddress<Version: IpExt>> GmpEnabledGroup<A> {
        /// Creates a new `GmpEnabledGroup` if `addr` should have GMP performed
        /// on it.
        pub fn new(addr: MulticastAddr<A>) -> Option<Self> {
            <A::Version as IpExt>::should_perform_gmp(addr).then_some(Self(addr))
        }

        /// Like [`GmpEnabledGroup::new`] but returns a `Result` with `addr` on
        /// `Err`.
        pub fn try_new(addr: MulticastAddr<A>) -> Result<Self, MulticastAddr<A>> {
            Self::new(addr).ok_or(addr)
        }

        /// Returns a copy of the multicast address witness.
        pub fn multicast_addr(&self) -> MulticastAddr<A> {
            let Self(addr) = self;
            *addr
        }

        /// Consumes the witness returning a multicast address.
        pub fn into_multicast_addr(self) -> MulticastAddr<A> {
            let Self(addr) = self;
            addr
        }
    }

    impl<A> AsRef<MulticastAddr<A>> for GmpEnabledGroup<A> {
        fn as_ref(&self) -> &MulticastAddr<A> {
            let Self(addr) = self;
            addr
        }
    }
}
use witness::GmpEnabledGroup;

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;
    use core::num::NonZeroU8;

    use assert_matches::assert_matches;
    use ip_test_macro::ip_test;
    use net_types::Witness as _;
    use netstack3_base::InstantContext as _;
    use netstack3_base::testutil::{FakeDeviceId, FakeTimerCtxExt, FakeWeakDeviceId};

    use testutil::{FakeCtx, FakeGmpContextInner, FakeV1Query, TestIpExt};

    use super::*;

    #[ip_test(I)]
    fn mode_change_state_clearing<I: TestIpExt>() {
        let FakeCtx { mut core_ctx, mut bindings_ctx } =
            testutil::new_context_with_mode::<I>(GmpMode::V1 { compat: false });

        assert_eq!(
            core_ctx.gmp_join_group(&mut bindings_ctx, &FakeDeviceId, I::GROUP_ADDR1),
            GroupJoinResult::Joined(())
        );
        // Drop the group join message so we can assert no more messages are
        // sent after this.
        core_ctx.inner.v1_messages.clear();

        // We should now have timers installed and v1 state in groups.
        assert!(core_ctx.gmp.timers.iter().next().is_some());
        assert_matches!(
            core_ctx.groups.get(&I::GROUP_ADDR1).unwrap().version_specific,
            GmpGroupStateByVersion::V1(_)
        );

        core_ctx.with_gmp_state_mut(&FakeDeviceId, |mut state| {
            enter_mode(&mut bindings_ctx, state.as_mut(), GmpMode::V2);
            assert_eq!(state.gmp.mode, GmpMode::V2);
        });
        // Timers were removed and state is now v2.
        core_ctx.gmp.timers.assert_timers([]);
        assert_matches!(
            core_ctx.groups.get(&I::GROUP_ADDR1).unwrap().version_specific,
            GmpGroupStateByVersion::V2(_)
        );

        // Moving back moves the state back to v1.
        core_ctx.with_gmp_state_mut(&FakeDeviceId, |mut state| {
            enter_mode(&mut bindings_ctx, state.as_mut(), GmpMode::V1 { compat: false });
            assert_eq!(state.gmp.mode, GmpMode::V1 { compat: false });
        });
        assert_matches!(
            core_ctx.groups.get(&I::GROUP_ADDR1).unwrap().version_specific,
            GmpGroupStateByVersion::V1(_)
        );

        // Throughout we should've generated no traffic.
        let FakeGmpContextInner { v1_messages, v2_messages } = &core_ctx.inner;
        assert_eq!(v1_messages, &Vec::new());
        assert_eq!(v2_messages, &Vec::<Vec<_>>::new());
    }

    #[ip_test(I)]
    #[should_panic(expected = "attempted to enter compatibility mode from forced")]
    fn cant_enter_v1_compat<I: TestIpExt>() {
        let FakeCtx { mut core_ctx, mut bindings_ctx } =
            testutil::new_context_with_mode::<I>(GmpMode::V1 { compat: false });
        core_ctx.with_gmp_state_mut(&FakeDeviceId, |mut state| {
            enter_mode(&mut bindings_ctx, state.as_mut(), GmpMode::V1 { compat: true });
        });
    }

    #[ip_test(I)]
    fn disable_exits_compat<I: TestIpExt>() {
        // Disabling in compat mode returns to v2.
        let FakeCtx { mut core_ctx, mut bindings_ctx } =
            testutil::new_context_with_mode::<I>(GmpMode::V1 { compat: true });
        core_ctx.enabled = false;
        core_ctx.gmp_handle_disabled(&mut bindings_ctx, &FakeDeviceId);
        assert_eq!(core_ctx.gmp.mode, GmpMode::V2);

        // Same is not true for not compat v1.
        let FakeCtx { mut core_ctx, mut bindings_ctx } =
            testutil::new_context_with_mode::<I>(GmpMode::V1 { compat: false });
        core_ctx.enabled = false;
        core_ctx.gmp_handle_disabled(&mut bindings_ctx, &FakeDeviceId);
        assert_eq!(core_ctx.gmp.mode, GmpMode::V1 { compat: false });
    }

    #[ip_test(I)]
    fn disable_clears_v2_state<I: TestIpExt>() {
        let FakeCtx { mut core_ctx, mut bindings_ctx } =
            testutil::new_context_with_mode::<I>(GmpMode::V1 { compat: false });
        let v2::ProtocolState { robustness_variable, query_interval, left_groups } =
            &mut core_ctx.gmp.v2_proto;
        *robustness_variable = robustness_variable.checked_add(1).unwrap();
        *query_interval = *query_interval + Duration::from_secs(20);
        *left_groups =
            [(GmpEnabledGroup::new(I::GROUP_ADDR1).unwrap(), NonZeroU8::new(1).unwrap())]
                .into_iter()
                .collect();
        core_ctx.enabled = false;
        core_ctx.gmp_handle_disabled(&mut bindings_ctx, &FakeDeviceId);
        assert_eq!(core_ctx.gmp.v2_proto, v2::ProtocolState::default());
    }

    #[ip_test(I)]
    fn v1_compat_mode_on_timeout<I: TestIpExt>() {
        let FakeCtx { mut core_ctx, mut bindings_ctx } =
            testutil::new_context_with_mode::<I>(GmpMode::V2);
        assert_eq!(
            v1::handle_query_message(
                &mut core_ctx,
                &mut bindings_ctx,
                &FakeDeviceId,
                &FakeV1Query {
                    group_addr: I::GROUP_ADDR1.get(),
                    max_response_time: Duration::from_secs(1)
                }
            ),
            Err(NotAMemberErr(I::GROUP_ADDR1.get()))
        );
        // Now in v1 mode and a compat timer is scheduled.
        assert_eq!(core_ctx.gmp.mode, GmpMode::V1 { compat: true });

        let timeout =
            core_ctx.gmp.v2_proto.older_version_querier_present_timeout(&core_ctx.config).into();
        core_ctx.gmp.timers.assert_timers([(
            TimerIdInner::V1Compat,
            (),
            bindings_ctx.now() + timeout,
        )]);

        // Increment the time and see that the timer updates.
        bindings_ctx.timers.instant.sleep(timeout / 2);
        assert_eq!(
            v1::handle_query_message(
                &mut core_ctx,
                &mut bindings_ctx,
                &FakeDeviceId,
                &FakeV1Query {
                    group_addr: I::GROUP_ADDR1.get(),
                    max_response_time: Duration::from_secs(1)
                }
            ),
            Err(NotAMemberErr(I::GROUP_ADDR1.get()))
        );
        assert_eq!(core_ctx.gmp.mode, GmpMode::V1 { compat: true });
        core_ctx.gmp.timers.assert_timers([(
            TimerIdInner::V1Compat,
            (),
            bindings_ctx.now() + timeout,
        )]);

        // Trigger the timer and observe a fallback to v2.
        let timer = bindings_ctx.trigger_next_timer(&mut core_ctx);
        assert_eq!(timer, Some(GmpTimerId::new(FakeWeakDeviceId(FakeDeviceId))));
        assert_eq!(core_ctx.gmp.mode, GmpMode::V2);
        // No more timers should exist, no frames are sent out.
        core_ctx.gmp.timers.assert_timers([]);
        let testutil::FakeGmpContextInner { v1_messages, v2_messages } = &core_ctx.inner;
        assert_eq!(v1_messages, &Vec::new());
        assert_eq!(v2_messages, &Vec::<Vec<_>>::new());
    }

    #[ip_test(I)]
    fn v1_no_compat_timer_when_disabled<I: TestIpExt>() {
        let FakeCtx { mut core_ctx, mut bindings_ctx } =
            testutil::new_context_with_mode::<I>(GmpMode::V2);
        core_ctx.enabled = false;
        core_ctx.gmp_handle_disabled(&mut bindings_ctx, &FakeDeviceId);
        assert_eq!(
            v1::handle_query_message(
                &mut core_ctx,
                &mut bindings_ctx,
                &FakeDeviceId,
                &FakeV1Query {
                    group_addr: I::GROUP_ADDR1.get(),
                    max_response_time: Duration::from_secs(1)
                }
            ),
            Err(NotAMemberErr(I::GROUP_ADDR1.get()))
        );
        // Should stay in V2 mode.
        assert_eq!(core_ctx.gmp.mode, GmpMode::V2);
        // No timers should exist.
        core_ctx.gmp.timers.assert_timers([]);
    }
}
