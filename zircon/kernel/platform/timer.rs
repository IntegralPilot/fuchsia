// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use zx_types::{ZX_TIME_INFINITE, ZX_TIME_INFINITE_PAST};

/// Monotonic timeline instant in nanoseconds.
#[repr(transparent)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstantMono(pub i64);

impl core::ops::Add<i64> for InstantMono {
    type Output = Self;

    #[inline]
    fn add(self, rhs: i64) -> Self {
        Self(self.0.saturating_add(rhs))
    }
}

impl core::ops::Add<DurationMono> for InstantMono {
    type Output = Self;

    #[inline]
    fn add(self, rhs: DurationMono) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }
}

impl core::ops::Sub<DurationMono> for InstantMono {
    type Output = Self;

    #[inline]
    fn sub(self, rhs: DurationMono) -> Self {
        Self(self.0.saturating_sub(rhs.0))
    }
}

impl core::ops::Sub<InstantMono> for InstantMono {
    type Output = DurationMono;

    #[inline]
    fn sub(self, rhs: InstantMono) -> DurationMono {
        DurationMono(self.0.saturating_sub(rhs.0))
    }
}

impl InstantMono {
    pub const ZERO: Self = Self(0);
    pub const INFINITE: Self = Self(ZX_TIME_INFINITE);
    pub const INFINITE_PAST: Self = Self(ZX_TIME_INFINITE_PAST);

    pub const fn from_nanos(nanos: i64) -> Self {
        Self(nanos)
    }

    pub const fn into_nanos(self) -> i64 {
        self.0
    }
}

/// Monotonic timeline instant in ticks.
#[repr(transparent)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstantMonoTicks(pub i64);

/// Boot timeline instant in nanoseconds.
#[repr(transparent)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstantBoot(pub i64);

/// Boot timeline instant in ticks.
#[repr(transparent)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstantBootTicks(pub i64);

/// Monotonic timeline duration in nanoseconds.
#[repr(transparent)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DurationMono(pub i64);

impl core::ops::Add<DurationMono> for DurationMono {
    type Output = Self;

    #[inline]
    fn add(self, rhs: DurationMono) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }
}

impl core::ops::Sub<DurationMono> for DurationMono {
    type Output = Self;

    #[inline]
    fn sub(self, rhs: DurationMono) -> Self {
        Self(self.0.saturating_sub(rhs.0))
    }
}

impl core::ops::Mul<i64> for DurationMono {
    type Output = Self;

    #[inline]
    fn mul(self, rhs: i64) -> Self {
        Self(self.0.saturating_mul(rhs))
    }
}

impl DurationMono {
    pub const ZERO: Self = Self(0);
    pub const INFINITE: Self = Self(ZX_TIME_INFINITE);
    pub const INFINITE_PAST: Self = Self(ZX_TIME_INFINITE_PAST);

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
        Self(nanos)
    }

    pub const fn from_micros(micros: i64) -> Self {
        Self(micros.saturating_mul(1_000))
    }

    pub const fn from_millis(millis: i64) -> Self {
        Self::from_micros(millis.saturating_mul(1_000))
    }

    pub const fn from_seconds(secs: i64) -> Self {
        Self::from_millis(secs.saturating_mul(1_000))
    }

    pub const fn from_minutes(min: i64) -> Self {
        Self::from_seconds(min.saturating_mul(60))
    }

    pub const fn from_hours(hours: i64) -> Self {
        Self::from_minutes(hours.saturating_mul(60))
    }
}

/// Monotonic timeline duration in ticks.
#[repr(transparent)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DurationMonoTicks(pub i64);

/// Boot timeline duration in nanoseconds.
#[repr(transparent)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DurationBoot(pub i64);

/// Boot timeline duration in ticks.
#[repr(transparent)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DurationBootTicks(pub i64);

// TODO(https://fxbug.dev/319935985): Not all locations are migrated to using specific timeline
// types, and some types (such as Deadline) are dangerously allowed to be instantiated on different
// timelines without capturing it in the type. For the moment we allow such behavior with these
// explicitly untyped types.

/// Instant in nanoseconds on an unknown timeline.
#[repr(transparent)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstantUnknown(pub i64);

impl From<InstantMono> for InstantUnknown {
    #[inline]
    fn from(val: InstantMono) -> Self {
        Self(val.0)
    }
}

impl From<InstantBoot> for InstantUnknown {
    #[inline]
    fn from(val: InstantBoot) -> Self {
        Self(val.0)
    }
}

/// Duration in nanoseconds on an unknown timeline.
#[repr(transparent)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DurationUnknown(pub i64);

impl From<DurationMono> for DurationUnknown {
    #[inline]
    fn from(val: DurationMono) -> Self {
        Self(val.0)
    }
}

impl From<DurationBoot> for DurationUnknown {
    #[inline]
    fn from(val: DurationBoot) -> Self {
        Self(val.0)
    }
}

unsafe extern "C" {
    fn cpp_timer_current_mono_ticks() -> InstantMonoTicks;
    fn cpp_timer_current_boot_ticks() -> InstantBootTicks;
    fn cpp_current_mono_time() -> InstantMono;
    fn cpp_current_boot_time() -> InstantBoot;
}

/// Returns the current monotonic time in ticks.
#[inline]
pub fn timer_current_mono_ticks() -> InstantMonoTicks {
    // SAFETY: Calling this FFI function has no preconditions and safely returns the platform timer
    // ticks.
    unsafe { cpp_timer_current_mono_ticks() }
}

/// Returns the current boot time in ticks.
#[inline]
pub fn timer_current_boot_ticks() -> InstantBootTicks {
    // SAFETY: Calling this FFI function has no preconditions and safely returns the platform timer
    // ticks.
    unsafe { cpp_timer_current_boot_ticks() }
}

/// Current monotonic time in nanoseconds.
#[inline]
pub fn current_mono_time() -> InstantMono {
    // SAFETY: Calling this FFI function has no preconditions and safely returns the platform
    // monotonic time.
    unsafe { cpp_current_mono_time() }
}

/// Current boot time in nanoseconds.
#[inline]
pub fn current_boot_time() -> InstantBoot {
    // SAFETY: Calling this FFI function has no preconditions and safely returns the platform boot
    // time.
    unsafe { cpp_current_boot_time() }
}

/// Platform timer tests.
#[cfg(ktest)]
#[unittest::suite(name = "platform_timer")]
mod tests {
    use super::{DurationMono, InstantMono};
    use unittest::assert_true;

    /// Test time ordering and equality.
    #[test]
    fn test_time_ordering_and_equality() {
        assert_true!(DurationMono(10) < DurationMono(20));
        assert_true!(DurationMono(15) == DurationMono(15));
        assert_true!(InstantMono(100) < InstantMono(200));
        assert_true!(InstantMono(100) + 50 == InstantMono(150));
        assert_true!(InstantMono(100) + DurationMono(50) == InstantMono(150));
    }

    /// Test saturating addition overflow on DurationMono.
    #[test]
    fn test_duration_mono_add_overflow() {
        assert_true!(DurationMono::INFINITE + DurationMono(1) == DurationMono::INFINITE);
        assert_true!(DurationMono(i64::MAX - 10) + DurationMono(20) == DurationMono::INFINITE);
        assert_true!(DurationMono(i64::MAX - 1) + DurationMono(1) == DurationMono::INFINITE);

        assert_true!(DurationMono::INFINITE_PAST + DurationMono(-1) == DurationMono::INFINITE_PAST);
        assert_true!(
            DurationMono(i64::MIN + 10) + DurationMono(-20) == DurationMono::INFINITE_PAST
        );
        assert_true!(DurationMono(i64::MIN + 1) + DurationMono(-1) == DurationMono::INFINITE_PAST);
    }

    /// Test saturating subtraction overflow on DurationMono.
    #[test]
    fn test_duration_mono_sub_overflow() {
        assert_true!(DurationMono::INFINITE - DurationMono(-1) == DurationMono::INFINITE);
        assert_true!(DurationMono(i64::MAX - 10) - DurationMono(-20) == DurationMono::INFINITE);
        assert_true!(DurationMono(i64::MAX - 1) - DurationMono(-1) == DurationMono::INFINITE);

        assert_true!(DurationMono::INFINITE_PAST - DurationMono(1) == DurationMono::INFINITE_PAST);
        assert_true!(DurationMono(i64::MIN + 10) - DurationMono(20) == DurationMono::INFINITE_PAST);
        assert_true!(DurationMono(i64::MIN + 1) - DurationMono(1) == DurationMono::INFINITE_PAST);
    }

    /// Test saturating multiplication overflow on DurationMono.
    #[test]
    fn test_duration_mono_mul_overflow() {
        // Positive * positive overflow
        assert_true!(DurationMono::INFINITE * 2 == DurationMono::INFINITE);
        assert_true!(DurationMono(i64::MAX / 2 + 1) * 2 == DurationMono::INFINITE);

        // Negative * negative overflow
        assert_true!(DurationMono::INFINITE_PAST * -1 == DurationMono::INFINITE);
        assert_true!(DurationMono::INFINITE_PAST * -2 == DurationMono::INFINITE);

        // Positive * negative underflow
        assert_true!(DurationMono::INFINITE * -2 == DurationMono::INFINITE_PAST);
        assert_true!(DurationMono(i64::MAX / 2 + 2) * -2 == DurationMono::INFINITE_PAST);

        // Negative * positive underflow
        assert_true!(DurationMono::INFINITE_PAST * 2 == DurationMono::INFINITE_PAST);
        assert_true!(DurationMono(i64::MIN / 2 - 1) * 2 == DurationMono::INFINITE_PAST);
    }

    /// Test saturating conversion from microseconds.
    #[test]
    fn test_duration_mono_from_micros_overflow() {
        let max_valid = i64::MAX / 1_000;
        assert_true!(DurationMono::from_micros(max_valid) == DurationMono(max_valid * 1_000));
        assert_true!(DurationMono::from_micros(max_valid + 1) == DurationMono::INFINITE);
        assert_true!(DurationMono::from_micros(i64::MAX) == DurationMono::INFINITE);

        let min_valid = i64::MIN / 1_000;
        assert_true!(DurationMono::from_micros(min_valid) == DurationMono(min_valid * 1_000));
        assert_true!(DurationMono::from_micros(min_valid - 1) == DurationMono::INFINITE_PAST);
        assert_true!(DurationMono::from_micros(i64::MIN) == DurationMono::INFINITE_PAST);
    }

    /// Test saturating conversion from milliseconds.
    #[test]
    fn test_duration_mono_from_millis_overflow() {
        let max_valid = i64::MAX / 1_000_000;
        assert_true!(DurationMono::from_millis(max_valid) == DurationMono(max_valid * 1_000_000));
        assert_true!(DurationMono::from_millis(max_valid + 1) == DurationMono::INFINITE);
        assert_true!(DurationMono::from_millis(i64::MAX / 1_000 + 1) == DurationMono::INFINITE);
        assert_true!(DurationMono::from_millis(i64::MAX) == DurationMono::INFINITE);

        let min_valid = i64::MIN / 1_000_000;
        assert_true!(DurationMono::from_millis(min_valid) == DurationMono(min_valid * 1_000_000));
        assert_true!(DurationMono::from_millis(min_valid - 1) == DurationMono::INFINITE_PAST);
        assert_true!(
            DurationMono::from_millis(i64::MIN / 1_000 - 1) == DurationMono::INFINITE_PAST
        );
        assert_true!(DurationMono::from_millis(i64::MIN) == DurationMono::INFINITE_PAST);
    }

    /// Test saturating conversion from seconds.
    #[test]
    fn test_duration_mono_from_seconds_overflow() {
        let max_valid = i64::MAX / 1_000_000_000;
        assert_true!(
            DurationMono::from_seconds(max_valid) == DurationMono(max_valid * 1_000_000_000)
        );
        assert_true!(DurationMono::from_seconds(max_valid + 1) == DurationMono::INFINITE);
        assert_true!(DurationMono::from_seconds(i64::MAX) == DurationMono::INFINITE);

        let min_valid = i64::MIN / 1_000_000_000;
        assert_true!(
            DurationMono::from_seconds(min_valid) == DurationMono(min_valid * 1_000_000_000)
        );
        assert_true!(DurationMono::from_seconds(min_valid - 1) == DurationMono::INFINITE_PAST);
        assert_true!(DurationMono::from_seconds(i64::MIN) == DurationMono::INFINITE_PAST);
    }

    /// Test saturating conversion from minutes.
    #[test]
    fn test_duration_mono_from_minutes_overflow() {
        const NANOS_PER_MIN: i64 = 60_000_000_000;
        let max_valid = i64::MAX / NANOS_PER_MIN;
        assert_true!(
            DurationMono::from_minutes(max_valid) == DurationMono(max_valid * NANOS_PER_MIN)
        );
        assert_true!(DurationMono::from_minutes(max_valid + 1) == DurationMono::INFINITE);
        assert_true!(DurationMono::from_minutes(i64::MAX) == DurationMono::INFINITE);

        let min_valid = i64::MIN / NANOS_PER_MIN;
        assert_true!(
            DurationMono::from_minutes(min_valid) == DurationMono(min_valid * NANOS_PER_MIN)
        );
        assert_true!(DurationMono::from_minutes(min_valid - 1) == DurationMono::INFINITE_PAST);
        assert_true!(DurationMono::from_minutes(i64::MIN) == DurationMono::INFINITE_PAST);
    }

    /// Test saturating conversion from hours.
    #[test]
    fn test_duration_mono_from_hours_overflow() {
        const NANOS_PER_HOUR: i64 = 3_600_000_000_000;
        let max_valid = i64::MAX / NANOS_PER_HOUR;
        assert_true!(
            DurationMono::from_hours(max_valid) == DurationMono(max_valid * NANOS_PER_HOUR)
        );
        assert_true!(DurationMono::from_hours(max_valid + 1) == DurationMono::INFINITE);
        assert_true!(DurationMono::from_hours(i64::MAX) == DurationMono::INFINITE);

        let min_valid = i64::MIN / NANOS_PER_HOUR;
        assert_true!(
            DurationMono::from_hours(min_valid) == DurationMono(min_valid * NANOS_PER_HOUR)
        );
        assert_true!(DurationMono::from_hours(min_valid - 1) == DurationMono::INFINITE_PAST);
        assert_true!(DurationMono::from_hours(i64::MIN) == DurationMono::INFINITE_PAST);
    }

    /// Test saturating arithmetic on InstantMono with DurationMono and InstantMono.
    #[test]
    fn test_instant_mono_overflow() {
        // InstantMono + i64
        assert_true!(InstantMono::INFINITE + 1 == InstantMono::INFINITE);
        assert_true!(InstantMono::INFINITE_PAST + (-1) == InstantMono::INFINITE_PAST);

        // InstantMono + DurationMono
        assert_true!(InstantMono::INFINITE + DurationMono(1) == InstantMono::INFINITE);
        assert_true!(InstantMono::INFINITE_PAST + DurationMono(-1) == InstantMono::INFINITE_PAST);

        // InstantMono - DurationMono
        assert_true!(InstantMono::INFINITE - DurationMono(-1) == InstantMono::INFINITE);
        assert_true!(InstantMono::INFINITE_PAST - DurationMono(1) == InstantMono::INFINITE_PAST);

        // InstantMono - InstantMono -> DurationMono
        assert_true!(InstantMono::INFINITE - InstantMono(-1) == DurationMono::INFINITE);
        assert_true!(InstantMono::INFINITE_PAST - InstantMono(1) == DurationMono::INFINITE_PAST);
    }
}
