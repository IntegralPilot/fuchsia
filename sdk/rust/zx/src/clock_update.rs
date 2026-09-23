// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Type-safe bindings for Zircon clock update objects.
//!
//! Example usage:
//! ```rust
//! clock.update(ClockUpdate::builder().approximate_value(updated_time)).expect("update failed");
//!
//! let update = ClockUpdate::builder().rate_adjust(42).error_bounds(1_000_000).build();
//! clock.update(update).expect("update failed");
//! ```

use crate::{sys, Instant, Timeline};
use std::fmt::Debug;

/// A trait implemented by all components of a ClockUpdateBuilder's state.
pub trait State {
    /// Records the validity in the supplied bitfield.
    fn add_options(&self, options: &mut u64);
}

/// A trait implemented by states that describe how to set a clock value.
pub trait ValueState: State {
    type ReferenceTimeline: Timeline;
    type OutputTimeline: Timeline;
    fn reference_value(&self) -> Option<Instant<Self::ReferenceTimeline>>;
    fn synthetic_value(&self) -> Option<Instant<Self::OutputTimeline>>;
}

/// A trait implemented by states that describe how to set a clock rate.
pub trait RateState: State {
    fn rate_adjustment(&self) -> Option<i32>;
}

/// A trait implemented by states that describe how to set a clock error.
pub trait ErrorState: State {
    fn error_bound(&self) -> Option<u64>;
}

/// A `ClockUpdateBuilder` state indicating no change.
pub struct Null<R, O>(std::marker::PhantomData<(R, O)>);

impl<R: Timeline, O: Timeline> State for Null<R, O> {
    fn add_options(&self, _: &mut u64) {}
}

impl<R: Timeline, O: Timeline> ValueState for Null<R, O> {
    type ReferenceTimeline = R;
    type OutputTimeline = O;
    fn reference_value(&self) -> Option<Instant<R>> {
        None
    }
    fn synthetic_value(&self) -> Option<Instant<O>> {
        None
    }
}
impl<R: Timeline, O: Timeline> RateState for Null<R, O> {
    fn rate_adjustment(&self) -> Option<i32> {
        None
    }
}
impl<R: Timeline, O: Timeline> ErrorState for Null<R, O> {
    fn error_bound(&self) -> Option<u64> {
        None
    }
}

/// A `ClockUpdateBuilder` state indicating value should be set using a
/// (reference time, synthetic time) tuple.
pub struct AbsoluteValue<R, O> {
    reference_value: Instant<R>,
    synthetic_value: Instant<O>,
}

impl<R: Timeline + Copy, O: Timeline + Copy> State for AbsoluteValue<R, O> {
    #[inline]
    fn add_options(&self, opts: &mut u64) {
        *opts |= sys::ZX_CLOCK_UPDATE_OPTION_REFERENCE_VALUE_VALID
            | sys::ZX_CLOCK_UPDATE_OPTION_SYNTHETIC_VALUE_VALID;
    }
}

impl<R: Timeline + Copy, O: Timeline + Copy> ValueState for AbsoluteValue<R, O> {
    type ReferenceTimeline = R;
    type OutputTimeline = O;
    fn reference_value(&self) -> Option<Instant<R>> {
        Some(self.reference_value)
    }
    fn synthetic_value(&self) -> Option<Instant<O>> {
        Some(self.synthetic_value)
    }
}

/// A `ClockUpdateBuilder` state indicating value should be set using only a synthetic time.
pub struct ApproximateValue<R, O>(Instant<O>, std::marker::PhantomData<R>);

impl<R: Timeline + Copy, O: Timeline + Copy> State for ApproximateValue<R, O> {
    #[inline]
    fn add_options(&self, opts: &mut u64) {
        *opts |= sys::ZX_CLOCK_UPDATE_OPTION_SYNTHETIC_VALUE_VALID;
    }
}

impl<R: Timeline + Copy, O: Timeline + Copy> ValueState for ApproximateValue<R, O> {
    type ReferenceTimeline = R;
    type OutputTimeline = O;
    fn reference_value(&self) -> Option<Instant<R>> {
        None
    }
    fn synthetic_value(&self) -> Option<Instant<O>> {
        Some(self.0)
    }
}

/// A clock update state indicating the rate should be set using the contained ppm offset.
pub struct Rate(i32);

impl State for Rate {
    #[inline]
    fn add_options(&self, opts: &mut u64) {
        *opts |= sys::ZX_CLOCK_UPDATE_OPTION_RATE_ADJUST_VALID;
    }
}

impl RateState for Rate {
    fn rate_adjustment(&self) -> Option<i32> {
        Some(self.0)
    }
}

/// A clock update state indicating the clock error should be set using the contained bound in
/// nanoseconds.
pub struct Error(u64);

impl State for Error {
    #[inline]
    fn add_options(&self, opts: &mut u64) {
        *opts |= sys::ZX_CLOCK_UPDATE_OPTION_ERROR_BOUND_VALID;
    }
}

impl ErrorState for Error {
    fn error_bound(&self) -> Option<u64> {
        Some(self.0)
    }
}

/// Builder to specify how zero or more properties of a clock should be updated.
/// See [`Clock::update`].
///
/// A `ClockUpdateBuilder` may be created using `ClockUpdate::builder()`.
#[derive(Debug, Eq, PartialEq)]
pub struct ClockUpdateBuilder<Val, Rate, Err, Ref, Out> {
    value_state: Val,
    rate_state: Rate,
    error_state: Err,
    _output_marker: std::marker::PhantomData<(Ref, Out)>,
}

impl<
        Val: ValueState<ReferenceTimeline = Ref, OutputTimeline = Out>,
        Rate: RateState,
        Err: ErrorState,
        Ref: Timeline,
        Out: Timeline,
    > ClockUpdateBuilder<Val, Rate, Err, Ref, Out>
{
    /// Converts this `ClockUpdateBuilder` to a `ClockUpdate`.
    #[inline]
    pub fn build(self) -> ClockUpdate<Ref, Out> {
        ClockUpdate::from(self)
    }
}

impl<R: Timeline, O: Timeline> ClockUpdateBuilder<Null<R, O>, Null<R, O>, Null<R, O>, R, O> {
    /// Returns an empty `ClockUpdateBuilder`.
    #[inline]
    fn new() -> Self {
        Self {
            value_state: Null(std::marker::PhantomData),
            rate_state: Null(std::marker::PhantomData),
            error_state: Null(std::marker::PhantomData),
            _output_marker: std::marker::PhantomData,
        }
    }
}

impl<Rate: RateState, Err: ErrorState, Ref: Timeline, Out: Timeline>
    ClockUpdateBuilder<Null<Ref, Out>, Rate, Err, Ref, Out>
{
    /// Sets an absolute value for this `ClockUpdate` using a (reference time, synthetic time) pair.
    ///
    /// Reference time is typically monotonic and synthetic time is the time tracked by the clock.
    /// Adding an absolute value is only possible when no other value has been set.
    #[inline]
    pub fn absolute_value(
        self,
        reference_value: Instant<Ref>,
        synthetic_value: Instant<Out>,
    ) -> ClockUpdateBuilder<AbsoluteValue<Ref, Out>, Rate, Err, Ref, Out> {
        ClockUpdateBuilder {
            value_state: AbsoluteValue { reference_value, synthetic_value },
            rate_state: self.rate_state,
            error_state: self.error_state,
            _output_marker: std::marker::PhantomData,
        }
    }
}

impl<Err: ErrorState, Ref: Timeline, Out: Timeline>
    ClockUpdateBuilder<Null<Ref, Out>, Null<Ref, Out>, Err, Ref, Out>
{
    /// Sets an approximate value for this `ClockUpdateBuilder` using a synthetic time only.
    ///
    /// Synthetic time is the time tracked by the clock. The reference time will be set to current
    /// monotonic time when the kernel applies this clock update, meaning any delay between
    /// calculating synthetic time and applying the update will result in a clock error. Adding an
    /// approximate value is only possible when no other value has been set and when no rate has
    /// been set.
    #[inline]
    pub fn approximate_value(
        self,
        synthetic_value: Instant<Out>,
    ) -> ClockUpdateBuilder<ApproximateValue<Ref, Out>, Null<Ref, Out>, Err, Ref, Out> {
        ClockUpdateBuilder {
            value_state: ApproximateValue(synthetic_value, std::marker::PhantomData),
            rate_state: self.rate_state,
            error_state: self.error_state,
            _output_marker: std::marker::PhantomData,
        }
    }
}

impl<Err: ErrorState, Ref: Timeline, Out: Timeline>
    ClockUpdateBuilder<Null<Ref, Out>, Null<Ref, Out>, Err, Ref, Out>
{
    /// Adds a rate change in parts per million to this `ClockUpdateBuilder`.
    ///
    /// Adding a rate is only possible when the value is either not set or set to an absolute value
    /// and when no rate has been set previously.
    #[inline]
    pub fn rate_adjust(
        self,
        rate_adjust_ppm: i32,
    ) -> ClockUpdateBuilder<Null<Ref, Out>, Rate, Err, Ref, Out> {
        ClockUpdateBuilder {
            value_state: self.value_state,
            rate_state: Rate(rate_adjust_ppm),
            error_state: self.error_state,
            _output_marker: std::marker::PhantomData,
        }
    }
}

impl<Err: ErrorState, Ref: Timeline, Out: Timeline>
    ClockUpdateBuilder<AbsoluteValue<Ref, Out>, Null<Ref, Out>, Err, Ref, Out>
{
    /// Adds a rate change in parts per million to this `ClockUpdateBuilder`.
    ///
    /// Adding a rate is only possible when the value is either not set or set to an absolute value
    /// and when no rate has been set previously.
    #[inline]
    pub fn rate_adjust(
        self,
        rate_adjust_ppm: i32,
    ) -> ClockUpdateBuilder<AbsoluteValue<Ref, Out>, Rate, Err, Ref, Out> {
        ClockUpdateBuilder {
            value_state: self.value_state,
            rate_state: Rate(rate_adjust_ppm),
            error_state: self.error_state,
            _output_marker: std::marker::PhantomData,
        }
    }
}

impl<Val: ValueState, Rate: RateState, Ref: Timeline, Out: Timeline>
    ClockUpdateBuilder<Val, Rate, Null<Ref, Out>, Ref, Out>
{
    /// Adds an error bound in nanoseconds to this `ClockUpdateBuilder`.
    #[inline]
    pub fn error_bounds(
        self,
        error_bound_ns: u64,
    ) -> ClockUpdateBuilder<Val, Rate, Error, Ref, Out> {
        ClockUpdateBuilder {
            value_state: self.value_state,
            rate_state: self.rate_state,
            error_state: Error(error_bound_ns),
            _output_marker: std::marker::PhantomData,
        }
    }
}

/// Specifies an update to zero or more properties of a clock. See [`Clock::update`]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClockUpdate<Reference, Output> {
    options: u64,
    rate_adjust: i32,
    synthetic_value: Instant<Output>,
    reference_value: Instant<Reference>,
    error_bound: u64,
}

impl<R: Timeline, O: Timeline> ClockUpdate<R, O> {
    /// Returns a new, empty, `ClockUpdateBuilder`.
    #[inline]
    pub fn builder() -> ClockUpdateBuilder<Null<R, O>, Null<R, O>, Null<R, O>, R, O> {
        ClockUpdateBuilder::new()
    }

    /// Returns a bitfield of options to pass to [`sys::zx_clock_update`] in conjunction with a
    /// `zx_clock_update_args_v2_t` generated from this `ClockUpdate`.
    #[inline]
    pub fn options(&self) -> u64 {
        self.options
    }

    pub(crate) fn args(self) -> sys::zx_clock_update_args_v2_t {
        let mut ret = sys::zx_clock_update_args_v2_t::default();
        ret.rate_adjust = self.rate_adjust;
        ret.synthetic_value = self.synthetic_value.into_nanos();
        ret.reference_value = self.reference_value.into_nanos();
        ret.error_bound = self.error_bound;
        ret
    }
}

impl<
        Val: ValueState<ReferenceTimeline = Ref, OutputTimeline = Out>,
        Rate: RateState,
        Err: ErrorState,
        Ref: Timeline,
        Out: Timeline,
    > From<ClockUpdateBuilder<Val, Rate, Err, Ref, Out>> for ClockUpdate<Ref, Out>
{
    fn from(builder: ClockUpdateBuilder<Val, Rate, Err, Ref, Out>) -> Self {
        let mut options = sys::ZX_CLOCK_ARGS_VERSION_2;
        builder.value_state.add_options(&mut options);
        builder.rate_state.add_options(&mut options);
        builder.error_state.add_options(&mut options);

        Self {
            options,
            rate_adjust: builder.rate_state.rate_adjustment().unwrap_or_default(),
            synthetic_value: builder.value_state.synthetic_value().unwrap_or_default(),
            reference_value: builder.value_state.reference_value().unwrap_or_default(),
            error_bound: builder.error_state.error_bound().unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MonotonicInstant, MonotonicTimeline, SyntheticInstant, SyntheticTimeline};

    #[test]
    fn empty_update() {
        let update =
            ClockUpdateBuilder::<_, _, _, MonotonicTimeline, SyntheticTimeline>::new().build();
        assert_eq!(update.options(), sys::ZX_CLOCK_ARGS_VERSION_2);
        assert_eq!(update.args(), sys::zx_clock_update_args_v2_t::default(),);
    }

    #[test]
    fn rate_only() {
        let update = ClockUpdate::<MonotonicTimeline, SyntheticTimeline>::from(
            ClockUpdateBuilder::new().rate_adjust(52),
        );
        assert_eq!(
            update.options(),
            sys::ZX_CLOCK_ARGS_VERSION_2 | sys::ZX_CLOCK_UPDATE_OPTION_RATE_ADJUST_VALID
        );
        let args = update.args();
        assert_eq!(args.rate_adjust, 52);
        assert_eq!(args.reference_value, 0);
        assert_eq!(args.synthetic_value, 0);
        assert_eq!(args.error_bound, 0);
    }

    #[test]
    fn approximate_value() {
        let update = ClockUpdateBuilder::<_, _, _, MonotonicTimeline, SyntheticTimeline>::new()
            .approximate_value(SyntheticInstant::from_nanos(42))
            .error_bounds(62)
            .build();
        assert_eq!(
            update.options(),
            sys::ZX_CLOCK_ARGS_VERSION_2
                | sys::ZX_CLOCK_UPDATE_OPTION_SYNTHETIC_VALUE_VALID
                | sys::ZX_CLOCK_UPDATE_OPTION_ERROR_BOUND_VALID
        );
        let args = update.args();
        assert_eq!(args.rate_adjust, 0);
        assert_eq!(args.reference_value, 0);
        assert_eq!(args.synthetic_value, 42);
        assert_eq!(args.error_bound, 62);
    }

    #[test]
    fn absolute_value() {
        let update = ClockUpdateBuilder::<_, _, _, MonotonicTimeline, SyntheticTimeline>::new()
            .absolute_value(MonotonicInstant::from_nanos(1000), SyntheticInstant::from_nanos(42))
            .rate_adjust(52)
            .error_bounds(62)
            .build();
        assert_eq!(
            update.options(),
            sys::ZX_CLOCK_ARGS_VERSION_2
                | sys::ZX_CLOCK_UPDATE_OPTION_REFERENCE_VALUE_VALID
                | sys::ZX_CLOCK_UPDATE_OPTION_SYNTHETIC_VALUE_VALID
                | sys::ZX_CLOCK_UPDATE_OPTION_RATE_ADJUST_VALID
                | sys::ZX_CLOCK_UPDATE_OPTION_ERROR_BOUND_VALID
        );

        let args = update.args();
        assert_eq!(args.rate_adjust, 52);
        assert_eq!(args.reference_value, 1000);
        assert_eq!(args.synthetic_value, 42);
        assert_eq!(args.error_bound, 62);
    }
}
