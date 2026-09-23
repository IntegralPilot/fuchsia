// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::common::EHandle;
use crate::runtime::DurationExt;

use std::ops;

/// A time relative to the executor's clock.
///
/// # Panics
///
/// If there is no executor set up. For example if used in a context that is not
/// bound to an executor, calling `now()` would panic.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct MonotonicInstant(zx::MonotonicInstant);

pub use zx::MonotonicDuration;

impl MonotonicInstant {
    /// Return the current time according to the global executor.
    ///
    /// This function requires that an executor has been set up.
    pub fn now() -> Self {
        EHandle::local().inner().now()
    }

    /// Compute a deadline for the time in the future that is the
    /// given `Duration` away. Similarly to `zx::MonotonicInstant::after`,
    /// saturates on overflow instead of wrapping around.
    ///
    /// This function requires that an executor has been set up.
    pub fn after(duration: zx::MonotonicDuration) -> Self {
        Self::now() + duration
    }

    /// Convert from `zx::MonotonicInstant`.
    pub const fn from_zx(t: zx::MonotonicInstant) -> Self {
        MonotonicInstant(t)
    }

    /// Convert into `zx::MonotonicInstant`.
    pub const fn into_zx(self) -> zx::MonotonicInstant {
        self.0
    }

    /// Convert from nanoseconds.
    pub const fn from_nanos(nanos: i64) -> Self {
        Self::from_zx(zx::MonotonicInstant::from_nanos(nanos))
    }

    /// Convert to nanoseconds.
    pub const fn into_nanos(self) -> i64 {
        self.0.into_nanos()
    }

    /// The maximum time.
    pub const INFINITE: MonotonicInstant = MonotonicInstant(zx::MonotonicInstant::INFINITE);

    /// The minimum time.
    pub const INFINITE_PAST: MonotonicInstant =
        MonotonicInstant(zx::MonotonicInstant::INFINITE_PAST);

    /// The epoch.
    pub const ZERO: MonotonicInstant = MonotonicInstant(zx::MonotonicInstant::ZERO);
}

impl From<zx::MonotonicInstant> for MonotonicInstant {
    fn from(t: zx::MonotonicInstant) -> MonotonicInstant {
        MonotonicInstant(t)
    }
}

impl From<MonotonicInstant> for zx::MonotonicInstant {
    fn from(t: MonotonicInstant) -> zx::MonotonicInstant {
        t.0
    }
}

impl ops::Add<zx::MonotonicDuration> for MonotonicInstant {
    type Output = MonotonicInstant;
    fn add(self, d: zx::MonotonicDuration) -> MonotonicInstant {
        MonotonicInstant(self.0 + d)
    }
}

impl ops::Add<MonotonicInstant> for zx::MonotonicDuration {
    type Output = MonotonicInstant;
    fn add(self, t: MonotonicInstant) -> MonotonicInstant {
        MonotonicInstant(self + t.0)
    }
}

impl ops::Sub<zx::MonotonicDuration> for MonotonicInstant {
    type Output = MonotonicInstant;
    fn sub(self, d: zx::MonotonicDuration) -> MonotonicInstant {
        MonotonicInstant(self.0 - d)
    }
}

impl ops::Sub<MonotonicInstant> for MonotonicInstant {
    type Output = zx::MonotonicDuration;
    fn sub(self, t: MonotonicInstant) -> zx::MonotonicDuration {
        self.0 - t.0
    }
}

impl ops::AddAssign<zx::MonotonicDuration> for MonotonicInstant {
    fn add_assign(&mut self, d: zx::MonotonicDuration) {
        self.0.add_assign(d)
    }
}

impl ops::SubAssign<zx::MonotonicDuration> for MonotonicInstant {
    fn sub_assign(&mut self, d: zx::MonotonicDuration) {
        self.0.sub_assign(d)
    }
}

impl DurationExt for zx::MonotonicDuration {
    fn after_now(self) -> MonotonicInstant {
        MonotonicInstant::after(self)
    }
}

/// A time relative to the executor's clock on the boot timeline.
///
/// # Panics
///
/// If there is no executor set up. For example if used in a context that is not
/// bound to an executor, calling `now()` would panic.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct BootInstant(zx::BootInstant);

impl BootInstant {
    /// Return the current time according to the system boot clock. Advances while the system
    /// is suspended.
    pub fn now() -> Self {
        EHandle::local().inner().boot_now()
    }

    /// Compute a deadline for the time in the future that is the
    /// given `Duration` away. Similar to `zx::BootInstant::after`,
    /// saturates on overflow instead of wrapping around.
    pub fn after(duration: zx::BootDuration) -> Self {
        Self::now() + duration
    }

    /// Convert from `zx::BootInstant`.
    pub const fn from_zx(t: zx::BootInstant) -> Self {
        BootInstant(t)
    }

    /// Convert into `zx::BootInstant`.
    pub const fn into_zx(self) -> zx::BootInstant {
        self.0
    }

    /// Convert from nanoseconds.
    pub const fn from_nanos(nanos: i64) -> Self {
        Self::from_zx(zx::BootInstant::from_nanos(nanos))
    }

    /// Convert to nanoseconds.
    pub const fn into_nanos(self) -> i64 {
        self.0.into_nanos()
    }

    /// The maximum time.
    pub const INFINITE: BootInstant = BootInstant(zx::BootInstant::INFINITE);

    /// The minimum time.
    pub const INFINITE_PAST: BootInstant = BootInstant(zx::BootInstant::INFINITE_PAST);
}

impl From<zx::BootInstant> for BootInstant {
    fn from(t: zx::BootInstant) -> BootInstant {
        BootInstant(t)
    }
}

impl From<BootInstant> for zx::BootInstant {
    fn from(t: BootInstant) -> zx::BootInstant {
        t.0
    }
}

impl ops::Add<zx::BootDuration> for BootInstant {
    type Output = BootInstant;
    fn add(self, d: zx::BootDuration) -> BootInstant {
        BootInstant(self.0 + d)
    }
}

impl ops::Add<BootInstant> for zx::BootDuration {
    type Output = BootInstant;
    fn add(self, t: BootInstant) -> BootInstant {
        BootInstant(self + t.0)
    }
}

impl ops::Sub<zx::BootDuration> for BootInstant {
    type Output = BootInstant;
    fn sub(self, d: zx::BootDuration) -> BootInstant {
        BootInstant(self.0 - d)
    }
}

impl ops::Sub<BootInstant> for BootInstant {
    type Output = zx::BootDuration;
    fn sub(self, t: BootInstant) -> zx::BootDuration {
        self.0 - t.0
    }
}

impl ops::AddAssign<zx::BootDuration> for BootInstant {
    fn add_assign(&mut self, d: zx::BootDuration) {
        self.0.add_assign(d)
    }
}

impl ops::SubAssign<zx::BootDuration> for BootInstant {
    fn sub_assign(&mut self, d: zx::BootDuration) {
        self.0.sub_assign(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn time_operations_param(
        zxt1: zx::MonotonicInstant,
        zxt2: zx::MonotonicInstant,
        d: zx::MonotonicDuration,
    ) {
        let t1 = MonotonicInstant::from_zx(zxt1);
        let t2 = MonotonicInstant::from_zx(zxt2);
        assert_eq!(t1.into_zx(), zxt1);

        assert_eq!(
            MonotonicInstant::from_zx(zx::MonotonicInstant::INFINITE),
            MonotonicInstant::INFINITE
        );
        assert_eq!(
            MonotonicInstant::from_zx(zx::MonotonicInstant::INFINITE_PAST),
            MonotonicInstant::INFINITE_PAST
        );
        assert_eq!(zxt1 - zxt2, t1 - t2);
        assert_eq!(zxt1 + d, (t1 + d).into_zx());
        assert_eq!(d + zxt1, (d + t1).into_zx());
        assert_eq!(zxt1 - d, (t1 - d).into_zx());

        let mut zxt = zxt1;
        let mut t = t1;
        t += d;
        zxt += d;
        assert_eq!(zxt, t.into_zx());
        t -= d;
        zxt -= d;
        assert_eq!(zxt, t.into_zx());
    }

    #[test]
    fn time_operations() {
        time_operations_param(
            zx::MonotonicInstant::from_nanos(0),
            zx::MonotonicInstant::from_nanos(1000),
            zx::MonotonicDuration::from_seconds(12),
        );
        time_operations_param(
            zx::MonotonicInstant::from_nanos(-100000),
            zx::MonotonicInstant::from_nanos(65324),
            zx::MonotonicDuration::from_hours(-785),
        );
    }

    #[test]
    fn time_saturating_add() {
        assert_eq!(
            MonotonicInstant::from_nanos(10) + zx::MonotonicDuration::from_nanos(30),
            MonotonicInstant::from_nanos(40)
        );
        assert_eq!(
            MonotonicInstant::from_nanos(10)
                + zx::MonotonicDuration::from_nanos(MonotonicInstant::INFINITE.into_nanos()),
            MonotonicInstant::INFINITE
        );
        assert_eq!(
            MonotonicInstant::from_nanos(-10)
                + zx::MonotonicDuration::from_nanos(MonotonicInstant::INFINITE_PAST.into_nanos()),
            MonotonicInstant::INFINITE_PAST
        );
    }
}
