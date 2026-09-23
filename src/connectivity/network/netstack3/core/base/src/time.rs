// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Common time abstractions.

pub(crate) mod local_timer_heap;
#[cfg(any(test, feature = "testutils"))]
pub(crate) mod testutil;

use core::convert::Infallible as Never;
use core::fmt::Debug;
use core::marker::PhantomData;
use core::sync::atomic::Ordering;
use core::time::Duration;

use crate::inspect::InspectableValue;

/// A type representing an instant in time.
///
/// `Instant` can be implemented by any type which represents an instant in
/// time. This can include any sort of real-world clock time (e.g.,
/// [`std::time::Instant`]) or fake time such as in testing.
pub trait Instant:
    Sized + Ord + Copy + Clone + Debug + Send + Sync + InspectableValue + 'static
{
    /// Returns the amount of time elapsed from another instant to this one.
    ///
    /// Returns `None` if `earlier` is not before `self`.
    fn checked_duration_since(&self, earlier: Self) -> Option<Duration>;

    /// Returns the amount of time elapsed from another instant to this one,
    /// saturating at zero.
    fn saturating_duration_since(&self, earlier: Self) -> Duration {
        self.checked_duration_since(earlier).unwrap_or_default()
    }

    /// Returns `Some(t)` where `t` is the time `self + duration` if `t` can be
    /// represented as `Instant` (which means it's inside the bounds of the
    /// underlying data structure), `None` otherwise.
    fn checked_add(&self, duration: Duration) -> Option<Self>;

    /// Returns the instant at `self + duration` saturating to the maximum
    /// representable instant value.
    fn saturating_add(&self, duration: Duration) -> Self;

    /// Unwraps the result from `checked_add`.
    ///
    /// # Panics
    ///
    /// This function will panic if the addition makes the clock wrap around.
    fn panicking_add(&self, duration: Duration) -> Self {
        self.checked_add(duration).unwrap_or_else(|| {
            panic!("clock wraps around when adding {:?} to {:?}", duration, *self);
        })
    }

    /// Returns `Some(t)` where `t` is the time `self - duration` if `t` can be
    /// represented as `Instant` (which means it's inside the bounds of the
    /// underlying data structure), `None` otherwise.
    fn checked_sub(&self, duration: Duration) -> Option<Self>;
}

/// A type representing an instant in time that can be atomically updated.
pub trait AtomicInstant<I: Instant>: Debug {
    /// Instantiates [`Self`] from the given instant.
    fn new(instant: I) -> Self;

    /// Loads an [`Instant`], atomically.
    fn load(&self, ordering: Ordering) -> I;

    /// Stores an [`Instant`], atomically,
    fn store(&self, instant: I, ordering: Ordering);

    /// Store the maximum of the current value and the provided value.
    fn store_max(&self, instant: I, ordering: Ordering);
}

/// Trait defining the `Instant` type provided by bindings' [`InstantContext`]
/// implementation.
///
/// It is a separate trait from `InstantContext` so the type stands by itself to
/// be stored at rest in core structures.
pub trait InstantBindingsTypes {
    /// The type of an instant in time.
    ///
    /// All time is measured using `Instant`s, including scheduling timers
    /// through [`TimerContext`]. This type may represent some sort of
    /// real-world time (e.g., [`std::time::Instant`]), or may be faked in
    /// testing using a fake clock.
    type Instant: Instant + 'static;

    /// An atomic representation of [`Self::Instant`].
    type AtomicInstant: AtomicInstant<Self::Instant>;
}

/// A context that provides access to a monotonic clock.
pub trait InstantContext: InstantBindingsTypes {
    /// Returns the current instant.
    ///
    /// `now` guarantees that two subsequent calls to `now` will return
    /// monotonically non-decreasing values.
    fn now(&self) -> Self::Instant;

    /// Returns the current instant, as an [`Self::AtomicInstant`].
    fn now_atomic(&self) -> Self::AtomicInstant {
        Self::AtomicInstant::new(self.now())
    }
}

/// Opaque types provided by bindings used by [`TimerContext`].
pub trait TimerBindingsTypes {
    /// State for a timer created through [`TimerContext`].
    type Timer: Debug + Send + Sync;
    /// The type used to dispatch fired timers from bindings to core.
    type DispatchId: Clone;
    /// A value that uniquely identifiers a `Timer`. It is given along with the
    /// `DispatchId` whenever a timer is fired.
    ///
    /// See [`TimerContext::unique_timer_id`] for details.
    type UniqueTimerId: PartialEq + Eq;
}

/// A context providing time scheduling to core.
pub trait TimerContext: InstantContext + TimerBindingsTypes {
    /// Creates a new timer that dispatches `id` back to core when fired.
    ///
    /// Creating a new timer is an expensive operation and should be used
    /// sparingly. Modules should prefer to create a timer on creation and then
    /// schedule/reschedule it as needed. For modules with very dynamic timers,
    /// a [`LocalTimerHeap`] tied to a larger `Timer` might be a better
    /// alternative than creating many timers.
    fn new_timer(&mut self, id: Self::DispatchId) -> Self::Timer;

    /// Schedule a timer to fire at some point in the future.
    /// Returns the previously scheduled instant, if this timer was scheduled.
    fn schedule_timer_instant(
        &mut self,
        time: Self::Instant,
        timer: &mut Self::Timer,
    ) -> Option<Self::Instant>;

    /// Like [`schedule_timer_instant`] but schedules a time for `duration` in
    /// the future.
    fn schedule_timer(
        &mut self,
        duration: Duration,
        timer: &mut Self::Timer,
    ) -> Option<Self::Instant> {
        self.schedule_timer_instant(self.now().checked_add(duration).unwrap(), timer)
    }

    /// Cancel a timer.
    ///
    /// Cancels `timer`, returning the instant it was scheduled for if it was
    /// scheduled.
    ///
    /// Note that there's no guarantee that observing `None` means that the
    /// dispatch procedure for a previously fired timer has already concluded.
    /// It is possible to observe `None` here while the `DispatchId` `timer`
    /// was created with is still making its way to the module that originally
    /// scheduled this timer. If `Some` is observed, however, then the
    /// `TimerContext` guarantees this `timer` will *not* fire until
    ///[`schedule_timer_instant`] is called to reschedule it.
    fn cancel_timer(&mut self, timer: &mut Self::Timer) -> Option<Self::Instant>;

    /// Get the instant a timer will fire, if one is scheduled.
    fn scheduled_instant(&self, timer: &mut Self::Timer) -> Option<Self::Instant>;

    /// Retrieves the timer id for `timer`.
    ///
    /// This can be used with [`TimerHandler::handle_timer`] to match a
    /// [`Self::Timer`] instance with a firing event.
    fn unique_timer_id(&self, timer: &Self::Timer) -> Self::UniqueTimerId;
}

/// A handler for timer firing events.
///
/// A `TimerHandler` is a type capable of handling the event of a timer firing.
///
/// `TimerHandler` is offered as a blanket implementation for all timers that
/// implement [`HandleableTimer`]. `TimerHandler` is meant to be used as bounds
/// on core context types. whereas `HandleableTimer` allows split-crate
/// implementations sidestepping coherence issues.
pub trait TimerHandler<BC: TimerBindingsTypes, Id> {
    /// Handle a timer firing.
    ///
    /// `dispatch` is the firing timer's dispatch identifier, i.e., a
    /// [`HandleableTimer`].
    ///
    /// `timer` is the unique timer identifier for the
    /// [`TimerBindingsTypes::Timer`] that scheduled this operation.
    fn handle_timer(&mut self, bindings_ctx: &mut BC, dispatch: Id, timer: BC::UniqueTimerId);
}

impl<Id, CC, BC> TimerHandler<BC, Id> for CC
where
    BC: TimerBindingsTypes,
    Id: HandleableTimer<CC, BC>,
{
    fn handle_timer(&mut self, bindings_ctx: &mut BC, dispatch: Id, timer: BC::UniqueTimerId) {
        dispatch.handle(self, bindings_ctx, timer)
    }
}

/// A timer that can be handled by a pair of core context `CC` and bindings
/// context `BC`.
///
/// This trait exists to sidestep coherence issues when dealing with timer
/// layers, see [`TimerHandler`] for more.
pub trait HandleableTimer<CC, BC: TimerBindingsTypes> {
    /// Handles this timer firing.
    ///
    /// `timer` is the unique timer identifier for the
    /// [`TimerBindingsTypes::Timer`] that scheduled this operation.
    fn handle(self, core_ctx: &mut CC, bindings_ctx: &mut BC, timer: BC::UniqueTimerId);
}

/// A core context providing timer type conversion.
///
/// This trait is used to convert from a core-internal timer type `T` to the
/// timer dispatch ID supported by bindings in `BT::DispatchId`.
pub trait CoreTimerContext<T, BT: TimerBindingsTypes> {
    /// Converts an inner timer to the bindings timer type.
    fn convert_timer(dispatch_id: T) -> BT::DispatchId;

    /// A helper function to create a new timer with the provided dispatch id.
    fn new_timer(bindings_ctx: &mut BT, dispatch_id: T) -> BT::Timer
    where
        BT: TimerContext,
    {
        bindings_ctx.new_timer(Self::convert_timer(dispatch_id))
    }
}

/// An uninstantiable type that performs conversions based on `Into`
/// implementations.
pub enum IntoCoreTimerCtx {}

impl<T, BT> CoreTimerContext<T, BT> for IntoCoreTimerCtx
where
    BT: TimerBindingsTypes,
    T: Into<BT::DispatchId>,
{
    fn convert_timer(dispatch_id: T) -> BT::DispatchId {
        dispatch_id.into()
    }
}

/// An uninstantiable type that performs conversions based on `Into`
/// implementations and an available outer [`CoreTimerContext`] `CC`.
pub struct NestedIntoCoreTimerCtx<CC, N>(Never, PhantomData<(CC, N)>);

impl<CC, N, T, BT> CoreTimerContext<T, BT> for NestedIntoCoreTimerCtx<CC, N>
where
    BT: TimerBindingsTypes,
    CC: CoreTimerContext<N, BT>,
    T: Into<N>,
{
    fn convert_timer(dispatch_id: T) -> BT::DispatchId {
        CC::convert_timer(dispatch_id.into())
    }
}
