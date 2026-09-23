// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Type-safe bindings for Zircon timer objects.

use crate::{NullableHandle, ObjectQuery, Status, Topic, ok, sys};
use std::cmp::{Eq, Ord, PartialEq, PartialOrd};
use std::hash::Hash;
use std::{ops, time as stdtime};
use zerocopy::{FromBytes, Immutable, IntoBytes};

/// A timestamp from the monontonic clock. Does not advance while the system is suspended.
pub type MonotonicInstant = Instant<MonotonicTimeline, NsUnit>;

/// A timestamp from a user-defined clock with arbitrary behavior.
pub type SyntheticInstant = Instant<SyntheticTimeline, NsUnit>;

/// A timestamp from the boot clock. Advances while the system is suspended.
pub type BootInstant = Instant<BootTimeline>;

/// A timestamp from system ticks. Has an arbitrary unit that can be measured with
/// `Ticks::per_second()`.
pub type Ticks<T> = Instant<T, TicksUnit>;

/// A timestamp from system ticks on the monotonic timeline. Does not advance while the system is
/// suspended.
pub type MonotonicTicks = Instant<MonotonicTimeline, TicksUnit>;

/// A timestamp from system ticks on the boot timeline. Advances while the system is suspended.
pub type BootTicks = Instant<BootTimeline, TicksUnit>;

/// A duration on the monotonic timeline.
pub type MonotonicDuration = Duration<MonotonicTimeline>;

/// A duration on the boot timeline.
pub type BootDuration = Duration<BootTimeline>;

/// A duration from a user-defined clock with arbitrary behavior.
pub type SyntheticDuration = Duration<SyntheticTimeline, NsUnit>;

/// A duration between two system ticks monotonic timestamps.
pub type MonotonicDurationTicks = Duration<MonotonicTimeline, TicksUnit>;

/// A duration between two system ticks boot timestamps.
pub type BootDurationTicks = Duration<BootTimeline, TicksUnit>;

/// A timestamp from the kernel. Generic over both the timeline and the units it is measured in.
#[derive(
    Clone, Copy, Default, Hash, PartialEq, Eq, PartialOrd, Ord, FromBytes, IntoBytes, Immutable,
)]
#[repr(transparent)]
pub struct Instant<T, U = NsUnit>(sys::zx_time_t, std::marker::PhantomData<(T, U)>);

impl<T, U> std::fmt::Debug for Instant<T, U> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Avoid line noise from the marker type but do include the timeline in the output.
        let timeline_name = std::any::type_name::<T>();
        let short_timeline_name =
            timeline_name.rsplit_once("::").map(|(_, n)| n).unwrap_or(timeline_name);
        let units_name = std::any::type_name::<U>();
        let short_units_name = units_name.rsplit_once("::").map(|(_, n)| n).unwrap_or(units_name);
        f.debug_tuple(&format!("Instant<{short_timeline_name}, {short_units_name}>"))
            .field(&self.0)
            .finish()
    }
}

impl MonotonicInstant {
    /// Get the current monotonic time which does not advance during system suspend.
    ///
    /// Wraps the
    /// [zx_clock_get_monotonic](https://fuchsia.dev/fuchsia-src/reference/syscalls/clock_get_monotonic.md)
    /// syscall.
    pub fn get() -> Self {
        unsafe { Self::from_nanos(sys::zx_clock_get_monotonic()) }
    }

    /// Compute a deadline for the time in the future that is the given `Duration` away.
    ///
    /// Wraps the
    /// [zx_deadline_after](https://fuchsia.dev/fuchsia-src/reference/syscalls/deadline_after.md)
    /// syscall.
    pub fn after(duration: MonotonicDuration) -> Self {
        unsafe { Self::from_nanos(sys::zx_deadline_after(duration.0)) }
    }

    /// Sleep until the given time.
    ///
    /// Wraps the
    /// [zx_nanosleep](https://fuchsia.dev/fuchsia-src/reference/syscalls/nanosleep.md)
    /// syscall.
    pub fn sleep(self) {
        unsafe {
            sys::zx_nanosleep(self.0);
        }
    }
}

impl BootInstant {
    /// Get the current boot time which advances during system suspend.
    pub fn get() -> Self {
        // SAFETY: FFI call that is always sound to call.
        unsafe { Self::from_nanos(sys::zx_clock_get_boot()) }
    }

    /// Compute a deadline for the time in the future that is the given `Duration` away.
    pub fn after(duration: BootDuration) -> Self {
        Self::from_nanos(Self::get().into_nanos().saturating_add(duration.0))
    }
}

impl<T: Timeline, U: TimeUnit> Instant<T, U> {
    pub const ZERO: Instant<T, U> = Instant(0, std::marker::PhantomData);
}

impl<T: Timeline> Instant<T> {
    pub const INFINITE: Instant<T, NsUnit> =
        Instant(sys::ZX_TIME_INFINITE, std::marker::PhantomData);
    pub const INFINITE_PAST: Instant<T, NsUnit> =
        Instant(sys::ZX_TIME_INFINITE_PAST, std::marker::PhantomData);

    /// Returns the number of nanoseconds since the epoch contained by this `Time`.
    pub const fn into_nanos(self) -> i64 {
        self.0
    }

    /// Return a strongly-typed `Time` from a raw number of nanoseconds.
    pub const fn from_nanos(nanos: i64) -> Self {
        Instant(nanos, std::marker::PhantomData)
    }
}

impl MonotonicTicks {
    /// Read the number of high-precision timer ticks on the monotonic timeline. These ticks may be
    /// processor cycles, high speed timer, profiling timer, etc. They do not advance while the
    /// system is suspended.
    ///
    /// Wraps the
    /// [zx_ticks_get](https://fuchsia.dev/fuchsia-src/reference/syscalls/ticks_get.md)
    /// syscall.
    pub fn get() -> Self {
        // SAFETY: FFI call that is always sound to call.
        Self(unsafe { sys::zx_ticks_get() }, std::marker::PhantomData)
    }
}

impl BootTicks {
    /// Read the number of high-precision timer ticks on the boot timeline. These ticks may be
    /// processor cycles, high speed timer, profiling timer, etc. They advance while the
    /// system is suspended.
    pub fn get() -> Self {
        // SAFETY: FFI call that is always sound to call.
        Self(unsafe { sys::zx_ticks_get_boot() }, std::marker::PhantomData)
    }
}

impl<T: Timeline> Ticks<T> {
    /// Return the number of ticks contained by this `Ticks`.
    pub const fn into_raw(self) -> i64 {
        self.0
    }

    /// Return a strongly-typed `Ticks` from a raw number of system ticks.
    pub const fn from_raw(raw: i64) -> Self {
        Self(raw, std::marker::PhantomData)
    }

    /// Return the number of high-precision timer ticks in a second.
    ///
    /// Wraps the
    /// [zx_ticks_per_second](https://fuchsia.dev/fuchsia-src/reference/syscalls/ticks_per_second.md)
    /// syscall.
    pub fn per_second() -> i64 {
        // SAFETY: FFI call that is always sound to call.
        unsafe { sys::zx_ticks_per_second() }
    }
}

impl<T: Timeline, U: TimeUnit> ops::Add<Duration<T, U>> for Instant<T, U> {
    type Output = Instant<T, U>;
    fn add(self, dur: Duration<T, U>) -> Self::Output {
        Self(self.0.saturating_add(dur.0), std::marker::PhantomData)
    }
}

impl<T: Timeline, U: TimeUnit> ops::Sub<Duration<T, U>> for Instant<T, U> {
    type Output = Instant<T, U>;
    fn sub(self, dur: Duration<T, U>) -> Self::Output {
        Self(self.0.saturating_sub(dur.0), std::marker::PhantomData)
    }
}

impl<T: Timeline, U: TimeUnit> ops::Sub<Instant<T, U>> for Instant<T, U> {
    type Output = Duration<T, U>;
    fn sub(self, rhs: Instant<T, U>) -> Self::Output {
        Duration(self.0.saturating_sub(rhs.0), std::marker::PhantomData)
    }
}

impl<T: Timeline, U: TimeUnit> ops::AddAssign<Duration<T, U>> for Instant<T, U> {
    fn add_assign(&mut self, dur: Duration<T, U>) {
        self.0 = self.0.saturating_add(dur.0);
    }
}

impl<T: Timeline, U: TimeUnit> ops::SubAssign<Duration<T, U>> for Instant<T, U> {
    fn sub_assign(&mut self, dur: Duration<T, U>) {
        self.0 = self.0.saturating_sub(dur.0);
    }
}

/// A marker trait for times to prevent accidental comparison between different timelines.
pub trait Timeline: Default + Copy + Clone + PartialEq + Eq {}

/// A marker type for the system's monotonic timeline which pauses during suspend.
#[derive(Debug, Default, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct MonotonicTimeline;
impl Timeline for MonotonicTimeline {}

/// A marker type for the system's boot timeline which continues running during suspend.
#[derive(Debug, Default, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct BootTimeline;
impl Timeline for BootTimeline {}

/// A marker type representing a synthetic timeline defined by a kernel clock object.
#[derive(Debug, Default, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SyntheticTimeline;
impl Timeline for SyntheticTimeline {}

/// A marker trait for times and durations to prevent accidental comparison between different units.
pub trait TimeUnit {}

/// A marker type representing nanoseconds.
#[derive(Debug, Default, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct NsUnit;
impl TimeUnit for NsUnit {}

/// A marker type representing system ticks.
#[derive(Debug, Default, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct TicksUnit;
impl TimeUnit for TicksUnit {}

#[derive(
    Debug,
    Default,
    Copy,
    Clone,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Hash,
    FromBytes,
    IntoBytes,
    Immutable,
)]
#[repr(transparent)]
pub struct Duration<T, U = NsUnit>(sys::zx_duration_t, std::marker::PhantomData<(T, U)>);

impl<T: Timeline> From<stdtime::Duration> for Duration<T, NsUnit> {
    fn from(dur: stdtime::Duration) -> Self {
        Duration::from_seconds(dur.as_secs() as i64)
            + Duration::from_nanos(dur.subsec_nanos() as i64)
    }
}

impl<T: Timeline, U: TimeUnit> ops::Add<Instant<T, U>> for Duration<T, U> {
    type Output = Instant<T, U>;
    fn add(self, time: Instant<T, U>) -> Self::Output {
        Instant(self.0.saturating_add(time.0), std::marker::PhantomData)
    }
}

impl<T: Timeline, U: TimeUnit> ops::Add for Duration<T, U> {
    type Output = Duration<T, U>;
    fn add(self, rhs: Duration<T, U>) -> Self::Output {
        Self(self.0.saturating_add(rhs.0), std::marker::PhantomData)
    }
}

impl<T: Timeline, U: TimeUnit> ops::Sub for Duration<T, U> {
    type Output = Duration<T, U>;
    fn sub(self, rhs: Duration<T, U>) -> Duration<T, U> {
        Self(self.0.saturating_sub(rhs.0), std::marker::PhantomData)
    }
}

impl<T: Timeline, U: TimeUnit> ops::AddAssign for Duration<T, U> {
    fn add_assign(&mut self, rhs: Duration<T, U>) {
        self.0 = self.0.saturating_add(rhs.0);
    }
}

impl<T: Timeline, U: TimeUnit> ops::SubAssign for Duration<T, U> {
    fn sub_assign(&mut self, rhs: Duration<T, U>) {
        self.0 = self.0.saturating_sub(rhs.0);
    }
}

impl<T: Timeline, S: Into<i64>, U: TimeUnit> ops::Mul<S> for Duration<T, U> {
    type Output = Self;
    fn mul(self, mul: S) -> Self {
        Self(self.0.saturating_mul(mul.into()), std::marker::PhantomData)
    }
}

impl<S: Into<i64>, T: Timeline, U: TimeUnit> ops::Div<S> for Duration<T, U> {
    type Output = Self;
    fn div(self, div: S) -> Self {
        Self(self.0.saturating_div(div.into()), std::marker::PhantomData)
    }
}

impl<T: Timeline, U: TimeUnit> ops::Neg for Duration<T, U> {
    type Output = Self;

    fn neg(self) -> Self::Output {
        Self(self.0.saturating_neg(), std::marker::PhantomData)
    }
}

impl<T: Timeline> Duration<T, NsUnit> {
    pub const INFINITE: Duration<T> = Duration(sys::zx_duration_t::MAX, std::marker::PhantomData);
    pub const INFINITE_PAST: Duration<T> =
        Duration(sys::zx_duration_t::MIN, std::marker::PhantomData);
    pub const ZERO: Duration<T> = Duration(0, std::marker::PhantomData);

    /// Returns the number of nanoseconds contained by this `Duration`.
    pub const fn into_nanos(self) -> i64 {
        self.0
    }

    /// Returns the total number of whole microseconds contained by this `Duration`.
    pub const fn into_micros(self) -> i64 {
        self.0 / 1_000
    }

    /// Returns the total number of whole milliseconds contained by this `Duration`.
    pub const fn into_millis(self) -> i64 {
        self.into_micros() / 1_000
    }

    /// Returns the total number of whole seconds contained by this `Duration`.
    pub const fn into_seconds(self) -> i64 {
        self.into_millis() / 1_000
    }

    /// Returns the duration as a floating-point value in seconds.
    pub fn into_seconds_f64(self) -> f64 {
        self.into_nanos() as f64 / 1_000_000_000f64
    }

    /// Returns the total number of whole minutes contained by this `Duration`.
    pub const fn into_minutes(self) -> i64 {
        self.into_seconds() / 60
    }

    /// Returns the total number of whole hours contained by this `Duration`.
    pub const fn into_hours(self) -> i64 {
        self.into_minutes() / 60
    }

    pub const fn from_nanos(nanos: i64) -> Self {
        Duration(nanos, std::marker::PhantomData)
    }

    pub const fn from_micros(micros: i64) -> Self {
        Duration(micros.saturating_mul(1_000), std::marker::PhantomData)
    }

    pub const fn from_millis(millis: i64) -> Self {
        Duration::from_micros(millis.saturating_mul(1_000))
    }

    pub const fn from_seconds(secs: i64) -> Self {
        Duration::from_millis(secs.saturating_mul(1_000))
    }

    pub const fn from_minutes(min: i64) -> Self {
        Duration::from_seconds(min.saturating_mul(60))
    }

    pub const fn from_hours(hours: i64) -> Self {
        Duration::from_minutes(hours.saturating_mul(60))
    }
}

impl<T: Timeline> Duration<T, TicksUnit> {
    /// Return the raw number of ticks represented by this `Duration`.
    pub const fn into_raw(self) -> i64 {
        self.0
    }

    /// Return a typed wrapper around the provided number of ticks.
    pub const fn from_raw(raw: i64) -> Self {
        Self(raw, std::marker::PhantomData)
    }
}

impl MonotonicDuration {
    /// Sleep for the given amount of time.
    pub fn sleep(self) {
        MonotonicInstant::after(self).sleep()
    }
}

/// An object representing a Zircon timer, such as the one returned by
/// [zx_timer_create](https://fuchsia.dev/fuchsia-src/reference/syscalls/timer_create.md).
///
/// As essentially a subtype of `NullableHandle`, it can be freely interconverted.
#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
// TODO(https://fxbug.dev/361661898) remove default type when FIDL understands mono vs. boot timers
pub struct Timer<T = MonotonicTimeline>(NullableHandle, std::marker::PhantomData<T>);

impl_handle_based!(Timer, [T: Timeline], [T]);

#[repr(C)]
#[derive(Debug, Copy, Clone, Eq, PartialEq, FromBytes, Immutable)]
pub struct TimerInfo<T: Timeline> {
    pub options: u32,
    // NB: Currently, this field is irrelevant, because the clock ID will always match the timeline
    // of the Timer which is represented by this TimerInfo.  For example, with a MonotonicTimer,
    // clock_id is always 0 (ZX_CLOCK_MONOTONIC).  If the timer syscalls eventually allow arbitrary
    // clock IDs, we can make this field public.
    clock_id: u32,
    pub deadline: Instant<T>,
    pub slack: Duration<T>,
}

pub type MonotonicTimerInfo = TimerInfo<MonotonicTimeline>;
pub type BootTimerInfo = TimerInfo<BootTimeline>;
pub type SyntheticTimerInfo = TimerInfo<SyntheticTimeline>;

static_assertions::assert_eq_size!(MonotonicTimerInfo, sys::zx_info_timer_t);
static_assertions::assert_eq_size!(BootTimerInfo, sys::zx_info_timer_t);
static_assertions::assert_eq_size!(SyntheticTimerInfo, sys::zx_info_timer_t);

impl<T: Timeline> Default for TimerInfo<T> {
    fn default() -> Self {
        Self::from_raw(sys::zx_info_timer_t::default())
    }
}

impl<T: Timeline> TimerInfo<T> {
    fn from_raw(info: sys::zx_info_timer_t) -> Self {
        zerocopy::transmute!(info)
    }
}

struct TimerInfoQuery;
unsafe impl ObjectQuery for TimerInfoQuery {
    const TOPIC: Topic = Topic::TIMER;
    type InfoTy = sys::zx_info_timer_t;
}

/// A timer that measures its deadlines against the monotonic clock.
pub type MonotonicTimer = Timer<MonotonicTimeline>;

/// A timer that measures its deadlines against the boot clock.
pub type BootTimer = Timer<BootTimeline>;

impl<T: Timeline> Timer<T> {
    /// Wraps the
    /// [zx_object_get_info](https://fuchsia.dev/fuchsia-src/reference/syscalls/object_get_info.md)
    /// syscall for the ZX_INFO_TIMER_T topic.
    pub fn info(&self) -> Result<TimerInfo<T>, Status> {
        Ok(TimerInfo::from_raw(self.0.get_info_single::<TimerInfoQuery>()?))
    }
}

impl Timer<MonotonicTimeline> {
    /// Create a timer, an object that can signal when a specified point on the monotonic clock has
    /// been reached. Wraps the
    /// [zx_timer_create](https://fuchsia.dev/fuchsia-src/reference/syscalls/timer_create.md)
    /// syscall.
    ///
    /// # Panics
    ///
    /// If the kernel reports no memory available to create a timer or the process' job policy
    /// denies timer creation.
    pub fn create() -> Self {
        let mut out = 0;
        let opts = 0;
        let status = unsafe { sys::zx_timer_create(opts, sys::ZX_CLOCK_MONOTONIC, &mut out) };
        ok(status)
            .expect("timer creation always succeeds except with OOM or when job policy denies it");
        unsafe { Self::from(NullableHandle::from_raw(out)) }
    }
}

impl Timer<BootTimeline> {
    /// Create a timer, an object that can signal when a specified point on the boot clock has been
    /// reached. Wraps the
    /// [zx_timer_create](https://fuchsia.dev/fuchsia-src/reference/syscalls/timer_create.md)
    /// syscall.
    ///
    /// If the timer elapses while the system is suspended it will not wake the system.
    ///
    /// # Panics
    ///
    /// If the kernel reports no memory available to create a timer or the process' job policy
    /// denies timer creation.
    pub fn create() -> Self {
        let mut out = 0;
        let opts = 0;
        let status = unsafe { sys::zx_timer_create(opts, sys::ZX_CLOCK_BOOT, &mut out) };
        ok(status)
            .expect("timer creation always succeeds except with OOM or when job policy denies it");
        unsafe { Self::from(NullableHandle::from_raw(out)) }
    }
}

impl<T: Timeline> Timer<T> {
    /// Start a one-shot timer that will fire when `deadline` passes. Wraps the
    /// [zx_timer_set](https://fuchsia.dev/fuchsia-src/reference/syscalls/timer_set.md)
    /// syscall.
    pub fn set(&self, deadline: Instant<T>, slack: Duration<T, NsUnit>) -> Result<(), Status> {
        let status = unsafe {
            sys::zx_timer_set(self.raw_handle(), deadline.into_nanos(), slack.into_nanos())
        };
        ok(status)
    }

    /// Cancels a pending timer that was started with set(). Wraps the
    /// [zx_timer_cancel](https://fuchsia.dev/fuchsia-src/reference/syscalls/timer_cancel.md)
    /// syscall.
    pub fn cancel(&self) -> Result<(), Status> {
        let status = unsafe { sys::zx_timer_cancel(self.raw_handle()) };
        ok(status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Signals, WaitResult};

    #[test]
    fn time_debug_repr_is_short() {
        assert_eq!(
            format!("{:?}", MonotonicInstant::from_nanos(0)),
            "Instant<MonotonicTimeline, NsUnit>(0)"
        );
        assert_eq!(
            format!("{:?}", SyntheticInstant::from_nanos(0)),
            "Instant<SyntheticTimeline, NsUnit>(0)"
        );
    }

    #[test]
    fn monotonic_time_increases() {
        let time1 = MonotonicInstant::get();
        Duration::from_nanos(1_000).sleep();
        let time2 = MonotonicInstant::get();
        assert!(time2 > time1);
    }

    #[test]
    fn ticks_increases() {
        let ticks1 = MonotonicTicks::get();
        Duration::from_nanos(1_000).sleep();
        let ticks2 = MonotonicTicks::get();
        assert!(ticks2 > ticks1);
    }

    #[test]
    fn boot_time_increases() {
        let time1 = BootInstant::get();
        Duration::from_nanos(1_000).sleep();
        let time2 = BootInstant::get();
        assert!(time2 > time1);
    }

    #[test]
    fn boot_ticks_increases() {
        let ticks1 = BootTicks::get();
        Duration::from_nanos(1_000).sleep();
        let ticks2 = BootTicks::get();
        assert!(ticks2 > ticks1);
    }

    #[test]
    fn tick_length() {
        let sleep_time = Duration::from_millis(1);
        let ticks1 = MonotonicTicks::get();
        sleep_time.sleep();
        let ticks2 = MonotonicTicks::get();

        // The number of ticks should have increased by at least 1 ms worth
        let sleep_ticks = MonotonicDurationTicks::from_raw(
            sleep_time.into_millis() * (MonotonicTicks::per_second() / 1000),
        );
        assert!(ticks2 >= (ticks1 + sleep_ticks));
    }

    #[test]
    fn sleep() {
        let sleep_ns = Duration::from_millis(1);
        let time1 = MonotonicInstant::get();
        sleep_ns.sleep();
        let time2 = MonotonicInstant::get();
        assert!(time2 > time1 + sleep_ns);
    }

    #[test]
    fn from_std() {
        let std_dur = stdtime::Duration::new(25, 25);
        let dur = MonotonicDuration::from(std_dur);
        let std_dur_nanos = (1_000_000_000 * std_dur.as_secs()) + std_dur.subsec_nanos() as u64;
        assert_eq!(std_dur_nanos as i64, dur.into_nanos());
    }

    #[test]
    fn i64_conversions() {
        let nanos_in_one_hour = 3_600_000_000_000;
        let dur_from_nanos = MonotonicDuration::from_nanos(nanos_in_one_hour);
        let dur_from_hours = MonotonicDuration::from_hours(1);
        assert_eq!(dur_from_nanos, dur_from_hours);
        assert_eq!(dur_from_nanos.into_nanos(), dur_from_hours.into_nanos());
        assert_eq!(dur_from_nanos.into_nanos(), nanos_in_one_hour);
        assert_eq!(dur_from_nanos.into_hours(), 1);
    }

    #[test]
    fn monotonic_timer_basic() {
        let slack = Duration::from_millis(0);
        let ten_ms = Duration::from_millis(10);
        let five_secs = Duration::from_seconds(5);

        // Create a timer
        let timer = MonotonicTimer::create();

        let info = timer.info().expect("info() failed");
        assert_eq!(info.clock_id, sys::ZX_CLOCK_MONOTONIC);
        assert_eq!(info.deadline, Instant::ZERO);
        // NB: We don't check slack, because the kernel can change it based on its own policies.

        // Should not signal yet.
        assert_eq!(
            timer.wait_one(Signals::TIMER_SIGNALED, MonotonicInstant::after(ten_ms)),
            WaitResult::TimedOut(Signals::empty()),
        );

        // Set it, and soon it should signal.
        let instant = MonotonicInstant::after(five_secs);
        assert_eq!(timer.set(instant, slack), Ok(()));
        assert_eq!(
            timer.wait_one(Signals::TIMER_SIGNALED, Instant::INFINITE),
            WaitResult::Ok(Signals::TIMER_SIGNALED)
        );
        // Once the timer has fired, its deadline is reset to zero.
        assert_eq!(timer.info().expect("info() failed").deadline, Instant::ZERO);

        // Cancel it, and it should stop signalling.
        assert_eq!(timer.cancel(), Ok(()));
        assert_eq!(
            timer.wait_one(Signals::TIMER_SIGNALED, MonotonicInstant::after(ten_ms)),
            WaitResult::TimedOut(Signals::empty()),
        );
        assert_eq!(timer.info().expect("info() failed").deadline, Instant::ZERO);

        assert_eq!(timer.set(Instant::INFINITE, slack), Ok(()));
        assert_eq!(timer.info().expect("info() failed").deadline, Instant::INFINITE);
    }

    #[test]
    fn boot_timer_basic() {
        let slack = Duration::from_millis(0);
        let ten_ms = Duration::from_millis(10);
        let five_secs = Duration::from_seconds(5);

        // Create a timer
        let timer = BootTimer::create();

        let info = timer.info().expect("info() failed");
        assert_eq!(info.clock_id, sys::ZX_CLOCK_BOOT);
        assert_eq!(info.deadline, Instant::ZERO);
        // NB: We don't check slack, because the kernel can change it based on its own policies.

        // Should not signal yet.
        assert_eq!(
            timer.wait_one(Signals::TIMER_SIGNALED, MonotonicInstant::after(ten_ms)),
            WaitResult::TimedOut(Signals::empty())
        );

        // Set it, and soon it should signal.
        let instant = BootInstant::after(five_secs);
        assert_eq!(timer.set(instant, slack), Ok(()));
        assert_eq!(
            timer.wait_one(Signals::TIMER_SIGNALED, Instant::INFINITE),
            WaitResult::Ok(Signals::TIMER_SIGNALED)
        );
        // Once the timer has fired, its deadline is reset to zero.
        assert_eq!(timer.info().expect("info() failed").deadline, Instant::ZERO);

        // Cancel it, and it should stop signalling.
        assert_eq!(timer.cancel(), Ok(()));
        assert_eq!(
            timer.wait_one(Signals::TIMER_SIGNALED, MonotonicInstant::after(ten_ms)),
            WaitResult::TimedOut(Signals::empty())
        );
        assert_eq!(timer.info().expect("info() failed").deadline, Instant::ZERO);

        assert_eq!(timer.set(Instant::INFINITE, slack), Ok(()));
        assert_eq!(timer.info().expect("info() failed").deadline, Instant::INFINITE);
    }

    #[test]
    fn time_minus_time() {
        let lhs = MonotonicInstant::from_nanos(10);
        let rhs = MonotonicInstant::from_nanos(30);
        assert_eq!(lhs - rhs, Duration::from_nanos(-20));
    }

    #[test]
    fn time_saturation() {
        // Addition
        assert_eq!(
            MonotonicInstant::from_nanos(10) + Duration::from_nanos(30),
            MonotonicInstant::from_nanos(40)
        );
        assert_eq!(
            MonotonicInstant::from_nanos(10) + Duration::INFINITE,
            MonotonicInstant::INFINITE
        );
        assert_eq!(
            MonotonicInstant::from_nanos(-10) + Duration::INFINITE_PAST,
            MonotonicInstant::INFINITE_PAST
        );

        // Subtraction
        assert_eq!(
            MonotonicInstant::from_nanos(10) - Duration::from_nanos(30),
            MonotonicInstant::from_nanos(-20)
        );
        assert_eq!(
            MonotonicInstant::from_nanos(-10) - Duration::INFINITE,
            MonotonicInstant::INFINITE_PAST
        );
        assert_eq!(
            MonotonicInstant::from_nanos(10) - Duration::INFINITE_PAST,
            MonotonicInstant::INFINITE
        );

        // Assigning addition
        {
            let mut t = MonotonicInstant::from_nanos(10);
            t += Duration::from_nanos(30);
            assert_eq!(t, MonotonicInstant::from_nanos(40));
        }
        {
            let mut t = MonotonicInstant::from_nanos(10);
            t += Duration::INFINITE;
            assert_eq!(t, MonotonicInstant::INFINITE);
        }
        {
            let mut t = MonotonicInstant::from_nanos(-10);
            t += Duration::INFINITE_PAST;
            assert_eq!(t, MonotonicInstant::INFINITE_PAST);
        }

        // Assigning subtraction
        {
            let mut t = MonotonicInstant::from_nanos(10);
            t -= Duration::from_nanos(30);
            assert_eq!(t, MonotonicInstant::from_nanos(-20));
        }
        {
            let mut t = MonotonicInstant::from_nanos(-10);
            t -= Duration::INFINITE;
            assert_eq!(t, MonotonicInstant::INFINITE_PAST);
        }
        {
            let mut t = MonotonicInstant::from_nanos(10);
            t -= Duration::INFINITE_PAST;
            assert_eq!(t, MonotonicInstant::INFINITE);
        }
    }

    #[test]
    fn duration_saturation() {
        // Addition
        assert_eq!(
            MonotonicDuration::from_nanos(10) + Duration::from_nanos(30),
            Duration::from_nanos(40)
        );
        assert_eq!(MonotonicDuration::from_nanos(10) + Duration::INFINITE, Duration::INFINITE);
        assert_eq!(
            MonotonicDuration::from_nanos(-10) + Duration::INFINITE_PAST,
            Duration::INFINITE_PAST
        );

        // Subtraction
        assert_eq!(
            MonotonicDuration::from_nanos(10) - Duration::from_nanos(30),
            Duration::from_nanos(-20)
        );
        assert_eq!(
            MonotonicDuration::from_nanos(-10) - Duration::INFINITE,
            Duration::INFINITE_PAST
        );
        assert_eq!(MonotonicDuration::from_nanos(10) - Duration::INFINITE_PAST, Duration::INFINITE);

        // Multiplication
        assert_eq!(MonotonicDuration::from_nanos(10) * 3, Duration::from_nanos(30));
        assert_eq!(MonotonicDuration::from_nanos(10) * i64::MAX, Duration::INFINITE);
        assert_eq!(MonotonicDuration::from_nanos(10) * i64::MIN, Duration::INFINITE_PAST);

        // Division
        assert_eq!(MonotonicDuration::from_nanos(30) / 3, Duration::from_nanos(10));
        assert_eq!(MonotonicDuration::INFINITE_PAST / -1, Duration::INFINITE);

        // Negation
        assert_eq!(-MonotonicDuration::from_nanos(30), Duration::from_nanos(-30));
        assert_eq!(-MonotonicDuration::INFINITE_PAST, Duration::INFINITE);

        // Assigning addition
        {
            let mut t = MonotonicDuration::from_nanos(10);
            t += Duration::from_nanos(30);
            assert_eq!(t, Duration::from_nanos(40));
        }
        {
            let mut t = MonotonicDuration::from_nanos(10);
            t += Duration::INFINITE;
            assert_eq!(t, Duration::INFINITE);
        }
        {
            let mut t = MonotonicDuration::from_nanos(-10);
            t += Duration::INFINITE_PAST;
            assert_eq!(t, Duration::INFINITE_PAST);
        }

        // Assigning subtraction
        {
            let mut t = MonotonicDuration::from_nanos(10);
            t -= Duration::from_nanos(30);
            assert_eq!(t, Duration::from_nanos(-20));
        }
        {
            let mut t = MonotonicDuration::from_nanos(-10);
            t -= Duration::INFINITE;
            assert_eq!(t, Duration::INFINITE_PAST);
        }
        {
            let mut t = MonotonicDuration::from_nanos(10);
            t -= Duration::INFINITE_PAST;
            assert_eq!(t, Duration::INFINITE);
        }
    }

    #[test]
    fn time_minus_time_saturates() {
        assert_eq!(
            MonotonicInstant::INFINITE - MonotonicInstant::INFINITE_PAST,
            Duration::INFINITE
        );
    }

    #[test]
    fn time_and_duration_defaults() {
        assert_eq!(MonotonicInstant::default(), MonotonicInstant::from_nanos(0));
        assert_eq!(Duration::default(), MonotonicDuration::from_nanos(0));
    }
}
