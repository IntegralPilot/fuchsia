// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A shareable fake bindings context.

use alloc::vec::Vec;
use core::convert::Infallible as Never;
use core::fmt::Debug;

use crate::sync::DynDebugReferences;
use crate::testutil::{
    AlwaysDefaultsSettingsContext, FakeAtomicInstant, FakeCryptoRng, FakeEventCtx, FakeFrameCtx,
    FakeInstant, FakeTimerCtx, WithFakeTimerContext,
};
use crate::{
    ContextProvider, DeferredResourceRemovalContext, EventContext, InstantBindingsTypes,
    InstantContext, MatcherBindingsTypes, ReferenceNotifiers, RngContext, TimerBindingsTypes,
    TimerContext,
};

/// A test helper used to provide an implementation of a bindings context.
pub struct FakeBindingsCtx<TimerId, Event: Debug, State, FrameMeta> {
    /// Provides [`RngContext`].
    pub rng: FakeCryptoRng,
    /// Provides [`TimerContext`].
    pub timers: FakeTimerCtx<TimerId>,
    /// Provides [`EventContext`].
    pub events: FakeEventCtx<Event>,
    /// Provides [`SendFrameContext`].
    pub frames: FakeFrameCtx<FrameMeta>,
    /// Generic state used by specific tests.
    pub state: State,
}

impl<TimerId, Event: Debug, State, FrameMeta> ContextProvider
    for FakeBindingsCtx<TimerId, Event, State, FrameMeta>
{
    type Context = Self;
    fn context(&mut self) -> &mut Self::Context {
        self
    }
}

impl<TimerId, Event: Debug, State: Default, FrameMeta> Default
    for FakeBindingsCtx<TimerId, Event, State, FrameMeta>
{
    fn default() -> Self {
        Self {
            rng: FakeCryptoRng::new_xorshift(0),
            timers: FakeTimerCtx::default(),
            events: FakeEventCtx::default(),
            frames: FakeFrameCtx::default(),
            state: Default::default(),
        }
    }
}

impl<TimerId, Event: Debug, State, FrameMeta> FakeBindingsCtx<TimerId, Event, State, FrameMeta> {
    /// Seed the testing RNG with a specific value.
    pub fn seed_rng(&mut self, seed: u128) {
        self.rng = FakeCryptoRng::new_xorshift(seed);
    }

    /// Takes all the accumulated events from the [`FakeEventCtx`].
    pub fn take_events(&mut self) -> Vec<Event> {
        self.events.take()
    }
}

impl<TimerId, Event: Debug, State, FrameMeta> RngContext
    for FakeBindingsCtx<TimerId, Event, State, FrameMeta>
{
    type Rng<'a>
        = FakeCryptoRng
    where
        Self: 'a;

    fn rng(&mut self) -> Self::Rng<'_> {
        self.rng.clone()
    }
}

impl<TimerId, Event: Debug, State, FrameMeta> InstantBindingsTypes
    for FakeBindingsCtx<TimerId, Event, State, FrameMeta>
{
    type Instant = FakeInstant;
    type AtomicInstant = FakeAtomicInstant;
}

impl<TimerId, Event: Debug, State, FrameMeta> InstantContext
    for FakeBindingsCtx<TimerId, Event, State, FrameMeta>
{
    fn now(&self) -> Self::Instant {
        self.timers.now()
    }
}

impl<Id: Debug + PartialEq + Clone + Send + Sync, Event: Debug, State, FrameMeta> TimerBindingsTypes
    for FakeBindingsCtx<Id, Event, State, FrameMeta>
{
    type Timer = <FakeTimerCtx<Id> as TimerBindingsTypes>::Timer;
    type DispatchId = <FakeTimerCtx<Id> as TimerBindingsTypes>::DispatchId;
    type UniqueTimerId = <FakeTimerCtx<Id> as TimerBindingsTypes>::UniqueTimerId;
}

impl<Id: Debug + PartialEq + Clone + Send + Sync, Event: Debug, State, FrameMeta> TimerContext
    for FakeBindingsCtx<Id, Event, State, FrameMeta>
{
    fn new_timer(&mut self, id: Self::DispatchId) -> Self::Timer {
        self.timers.new_timer(id)
    }

    fn schedule_timer_instant(
        &mut self,
        time: Self::Instant,
        timer: &mut Self::Timer,
    ) -> Option<Self::Instant> {
        self.timers.schedule_timer_instant(time, timer)
    }

    fn cancel_timer(&mut self, timer: &mut Self::Timer) -> Option<Self::Instant> {
        self.timers.cancel_timer(timer)
    }

    fn scheduled_instant(&self, timer: &mut Self::Timer) -> Option<Self::Instant> {
        self.timers.scheduled_instant(timer)
    }

    fn unique_timer_id(&self, timer: &Self::Timer) -> Self::UniqueTimerId {
        self.timers.unique_timer_id(timer)
    }
}

impl<Id, Event: Debug, State, FrameMeta> EventContext<Event>
    for FakeBindingsCtx<Id, Event, State, FrameMeta>
{
    fn on_event(&mut self, event: Event) {
        self.events.on_event(event)
    }
}

impl<Id, Event: Debug, State, FrameMeta> ReferenceNotifiers
    for FakeBindingsCtx<Id, Event, State, FrameMeta>
{
    type ReferenceReceiver<T: 'static> = Never;

    type ReferenceNotifier<T: Send + 'static> = Never;

    fn new_reference_notifier<T: Send + 'static>(
        debug_references: DynDebugReferences,
    ) -> (Self::ReferenceNotifier<T>, Self::ReferenceReceiver<T>) {
        // NB: We don't want deferred destruction in core tests. These are
        // always single-threaded and single-task, and we want to encourage
        // explicit cleanup.
        panic!(
            "FakeBindingsCtx can't create deferred reference notifiers for type {}: \
            debug_references={debug_references:?}",
            core::any::type_name::<T>()
        );
    }
}

impl<Id, Event: Debug, State, FrameMeta> DeferredResourceRemovalContext
    for FakeBindingsCtx<Id, Event, State, FrameMeta>
{
    fn defer_removal<T: Send + 'static>(&mut self, receiver: Self::ReferenceReceiver<T>) {
        match receiver {}
    }
}

impl<TimerId, Event: Debug, State, FrameMeta> WithFakeTimerContext<TimerId>
    for FakeBindingsCtx<TimerId, Event, State, FrameMeta>
{
    fn with_fake_timer_ctx<O, F: FnOnce(&FakeTimerCtx<TimerId>) -> O>(&self, f: F) -> O {
        f(&self.timers)
    }

    fn with_fake_timer_ctx_mut<O, F: FnOnce(&mut FakeTimerCtx<TimerId>) -> O>(
        &mut self,
        f: F,
    ) -> O {
        f(&mut self.timers)
    }
}

impl<TimerId, Event: Debug, State, FrameMeta> AlwaysDefaultsSettingsContext
    for FakeBindingsCtx<TimerId, Event, State, FrameMeta>
{
}

impl<TimerId: Debug, Event: Debug, State, FrameMeta> MatcherBindingsTypes
    for FakeBindingsCtx<TimerId, Event, State, FrameMeta>
{
    type DeviceClass = ();
    type BindingsPacketMatcher = Never;
}
