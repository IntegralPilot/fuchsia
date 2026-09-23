// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// One million for PPM calculations
const MILLION: u64 = 1_000_000;

/// A transformation from monotonic time to synthetic time, including an error bound on this
/// synthetic time.
#[derive(Clone, Default, Debug, Eq, PartialEq)]
pub struct Transform<Reference, Output> {
    /// An offset on the monotonic timeline in nanoseconds.
    pub reference_offset: zx::Instant<Reference>,
    /// An offset on the synthetic timeline in nanoseconds.
    pub synthetic_offset: zx::Instant<Output>,
    /// An adjustment to the standard 1 monotonic tick:1 synthetic tick rate in parts per million.
    /// Positive values indicate the synthetic clock is moving faster than the monotonic clock.
    pub rate_adjust_ppm: i32,
    /// The error bound on synthetic clock at monotonic = monotonic_offset.
    pub error_bound_at_offset: u64,
    /// The growth in error bound per monotonic tick in parts per million.
    pub error_bound_growth_ppm: u32,
}

impl<Reference: zx::Timeline + Copy, Output: zx::Timeline + Copy> Transform<Reference, Output> {
    /// Returns the synthetic time at the supplied monotonic time.
    pub fn synthetic(&self, reference: zx::Instant<Reference>) -> zx::Instant<Output> {
        // Cast to i128 to avoid overflows in multiplication.
        let reference_difference = (reference - self.reference_offset).into_nanos() as i128;
        let synthetic_offset = self.synthetic_offset.into_nanos() as i128;
        let synthetic_ticks = self.rate_adjust_ppm as i128 + MILLION as i128;
        let reference_ticks = MILLION as i128;

        let time_nanos =
            (reference_difference * synthetic_ticks / reference_ticks) + synthetic_offset;
        zx::Instant::from_nanos(time_nanos as i64)
    }

    /// Returns the error bound at the supplied monotonic time.
    pub fn error_bound(&self, reference: zx::Instant<Reference>) -> u64 {
        // Cast to i128 to avoid overflows in multiplication.
        let reference_difference = (reference - self.reference_offset).into_nanos() as i128;
        if reference_difference <= 0 {
            // Assume the error bound was fixed at the supplied value before the reference time.
            self.error_bound_at_offset
        } else {
            // Error bound increases linearly after the reference time.
            let error_increase =
                (reference_difference * self.error_bound_growth_ppm as i128) / MILLION as i128;
            self.error_bound_at_offset + error_increase as u64
        }
    }

    /// Returns the synthetic time on this `Transform` minus the synthetic time on `other`,
    /// calculated at the supplied monotonic time.
    pub fn difference(
        &self,
        other: &Self,
        reference: zx::Instant<Reference>,
    ) -> zx::Duration<Output> {
        self.synthetic(reference) - other.synthetic(reference)
    }

    /// Returns a `ClockUpdate` that will set a `Clock` onto this `Transform` using data
    /// from the supplied monotonic time.
    pub fn jump_to(&self, reference: zx::Instant<Reference>) -> zx::ClockUpdate<Reference, Output> {
        zx::ClockUpdate::<Reference, Output>::builder()
            .absolute_value(reference, self.synthetic(reference))
            .rate_adjust(self.rate_adjust_ppm)
            .error_bounds(self.error_bound(reference))
            .build()
    }
}

impl<Reference: zx::Timeline, Output: zx::Timeline> From<&zx::Clock<Reference, Output>>
    for Transform<Reference, Output>
{
    fn from(clock: &zx::Clock<Reference, Output>) -> Self {
        // Clock read failures should only be caused by an invalid clock object.
        let details = clock.get_details().expect("failed to get clock details");
        // Cast to i64 to avoid overflows in multiplication.
        let reference_ticks = details.reference_to_synthetic.rate.reference_ticks as i64;
        let synthetic_ticks = details.reference_to_synthetic.rate.synthetic_ticks as i64;
        let rate_adjust_ppm =
            ((synthetic_ticks * MILLION as i64) / reference_ticks) - MILLION as i64;

        Transform {
            reference_offset: details.reference_to_synthetic.reference_offset,
            synthetic_offset: details.reference_to_synthetic.synthetic_offset,
            rate_adjust_ppm: rate_adjust_ppm as i32,
            // Zircon clocks don't document the change in error over time. Assume a fixed error.
            error_bound_at_offset: details.error_bounds,
            error_bound_growth_ppm: 0,
        }
    }
}

/// Returns the time on the clock at a given monotonic reference time. This calculates the time
/// based on the clock transform definition, which only contains the most recent segment. This
/// is only useful for calculating the time for monotonic times close to the current time.
pub fn time_at_monotonic<Reference: zx::Timeline, Output: zx::Timeline>(
    clock: &zx::Clock<Reference, Output>,
    reference: zx::Instant<Reference>,
) -> zx::Instant<Output> {
    let reference_nanos = reference.into_nanos() as i128;
    // Clock read failures should only be caused by an invalid clock object.
    let details = clock.get_details().expect("failed to get clock details");
    // Calculate using the transform definition underlying a zircon clock.
    // Cast to i128 to avoid overflows in multiplication.
    let reference_offset = details.reference_to_synthetic.reference_offset.into_nanos() as i128;
    let synthetic_offset = details.reference_to_synthetic.synthetic_offset.into_nanos() as i128;
    let reference_ticks = details.reference_to_synthetic.rate.reference_ticks as i128;
    let synthetic_ticks = details.reference_to_synthetic.rate.synthetic_ticks as i128;

    let time_nanos = ((reference_nanos - reference_offset) * synthetic_ticks / reference_ticks)
        + synthetic_offset;
    zx::Instant::from_nanos(time_nanos as i64)
}

#[cfg(test)]
mod test {
    use super::*;
    use test_util::{assert_geq, assert_leq};

    const BACKSTOP: zx::SyntheticInstant = zx::SyntheticInstant::from_nanos(1234567890);
    const TIME_DIFF: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(5);
    const SLEW_RATE_PPM: i32 = 750;
    const ONE_MILLION: i32 = 1_000_000;

    const TEST_REFERENCE: zx::MonotonicInstant = zx::MonotonicInstant::from_nanos(70_000_000_000);
    const TEST_OFFSET: zx::MonotonicDuration = zx::MonotonicDuration::from_nanos(5_000_000_000);
    const TEST_ERROR_BOUND: u64 = 1234_000;
    const TEST_ERROR_BOUND_GROWTH: u32 = 100;

    const TOLERANCE: zx::MonotonicDuration = zx::MonotonicDuration::from_nanos(500_000_000);

    #[fuchsia::test]
    fn transform_properties_zero_rate_adjust() {
        let transform = Transform {
            reference_offset: TEST_REFERENCE,
            synthetic_offset: zx::SyntheticInstant::from_nanos(
                (TEST_REFERENCE + TEST_OFFSET).into_nanos(),
            ),
            rate_adjust_ppm: 0,
            error_bound_at_offset: TEST_ERROR_BOUND,
            error_bound_growth_ppm: TEST_ERROR_BOUND_GROWTH,
        };

        assert_eq!(
            transform.synthetic(TEST_REFERENCE).into_nanos(),
            (TEST_REFERENCE + TEST_OFFSET).into_nanos()
        );
        assert_eq!(
            transform
                .synthetic(TEST_REFERENCE + zx::MonotonicDuration::from_millis(200))
                .into_nanos(),
            (TEST_REFERENCE + TEST_OFFSET + zx::MonotonicDuration::from_millis(200)).into_nanos(),
        );
        assert_eq!(
            transform
                .synthetic(TEST_REFERENCE - zx::MonotonicDuration::from_millis(100))
                .into_nanos(),
            (TEST_REFERENCE + TEST_OFFSET - zx::MonotonicDuration::from_millis(100)).into_nanos(),
        );

        assert_eq!(transform.error_bound(TEST_REFERENCE), TEST_ERROR_BOUND);
        assert_eq!(
            transform.error_bound(TEST_REFERENCE + zx::MonotonicDuration::from_millis(1)),
            TEST_ERROR_BOUND + TEST_ERROR_BOUND_GROWTH as u64
        );
        assert_eq!(
            transform.error_bound(TEST_REFERENCE - zx::MonotonicDuration::from_millis(1)),
            TEST_ERROR_BOUND as u64
        );
    }

    #[fuchsia::test]
    fn transform_properties_positive_rate_adjust() {
        let transform = Transform {
            reference_offset: TEST_REFERENCE,
            synthetic_offset: zx::SyntheticInstant::from_nanos(
                (TEST_REFERENCE + TEST_OFFSET).into_nanos(),
            ),
            rate_adjust_ppm: 25,
            error_bound_at_offset: TEST_ERROR_BOUND,
            error_bound_growth_ppm: 0,
        };

        assert_eq!(
            transform.synthetic(TEST_REFERENCE).into_nanos(),
            (TEST_REFERENCE + TEST_OFFSET).into_nanos(),
        );
        assert_eq!(
            transform
                .synthetic(TEST_REFERENCE + zx::MonotonicDuration::from_millis(200))
                .into_nanos(),
            (TEST_REFERENCE
                + TEST_OFFSET
                + zx::MonotonicDuration::from_millis(200)
                + zx::MonotonicDuration::from_nanos(25 * 200))
            .into_nanos(),
        );
        assert_eq!(
            transform
                .synthetic(TEST_REFERENCE - zx::MonotonicDuration::from_millis(100))
                .into_nanos(),
            (TEST_REFERENCE + TEST_OFFSET
                - zx::MonotonicDuration::from_millis(100)
                - zx::MonotonicDuration::from_nanos(25 * 100))
            .into_nanos(),
        );

        assert_eq!(transform.error_bound(TEST_REFERENCE), TEST_ERROR_BOUND);
        assert_eq!(
            transform.error_bound(TEST_REFERENCE + zx::MonotonicDuration::from_millis(1)),
            TEST_ERROR_BOUND as u64
        );
        assert_eq!(
            transform.error_bound(TEST_REFERENCE - zx::MonotonicDuration::from_millis(1)),
            TEST_ERROR_BOUND as u64
        );
    }

    #[fuchsia::test]
    fn transform_properties_negative_rate_adjust() {
        let transform = Transform {
            reference_offset: TEST_REFERENCE,
            synthetic_offset: zx::SyntheticInstant::from_nanos(
                (TEST_REFERENCE + TEST_OFFSET).into_nanos(),
            ),
            rate_adjust_ppm: -50,
            error_bound_at_offset: TEST_ERROR_BOUND,
            error_bound_growth_ppm: TEST_ERROR_BOUND_GROWTH,
        };

        assert_eq!(
            transform.synthetic(TEST_REFERENCE).into_nanos(),
            (TEST_REFERENCE + TEST_OFFSET).into_nanos(),
        );
        assert_eq!(
            transform
                .synthetic(TEST_REFERENCE + zx::MonotonicDuration::from_millis(200))
                .into_nanos(),
            (TEST_REFERENCE + TEST_OFFSET + zx::MonotonicDuration::from_millis(200)
                - zx::MonotonicDuration::from_nanos(50 * 200))
            .into_nanos(),
        );
        assert_eq!(
            transform
                .synthetic(TEST_REFERENCE - zx::MonotonicDuration::from_millis(100))
                .into_nanos(),
            (TEST_REFERENCE + TEST_OFFSET - zx::MonotonicDuration::from_millis(100)
                + zx::MonotonicDuration::from_nanos(50 * 100))
            .into_nanos(),
        );

        assert_eq!(transform.error_bound(TEST_REFERENCE), TEST_ERROR_BOUND);
        assert_eq!(
            transform.error_bound(TEST_REFERENCE + zx::MonotonicDuration::from_seconds(1)),
            TEST_ERROR_BOUND + (TEST_ERROR_BOUND_GROWTH * 1000) as u64
        );
        assert_eq!(
            transform.error_bound(TEST_REFERENCE - zx::MonotonicDuration::from_seconds(1)),
            TEST_ERROR_BOUND as u64
        );
    }

    #[fuchsia::test]
    fn transform_difference() {
        let transform_1 = Transform {
            reference_offset: TEST_REFERENCE,
            synthetic_offset: zx::SyntheticInstant::from_nanos(
                (TEST_REFERENCE + TEST_OFFSET).into_nanos(),
            ),
            rate_adjust_ppm: 25,
            error_bound_at_offset: TEST_ERROR_BOUND,
            error_bound_growth_ppm: TEST_ERROR_BOUND_GROWTH,
        };

        let transform_2 = Transform {
            reference_offset: TEST_REFERENCE,
            synthetic_offset: zx::SyntheticInstant::from_nanos(TEST_REFERENCE.into_nanos()),
            rate_adjust_ppm: -50,
            error_bound_at_offset: TEST_ERROR_BOUND,
            error_bound_growth_ppm: 0,
        };

        assert_eq!(
            transform_1.difference(&transform_1, TEST_REFERENCE),
            zx::SyntheticDuration::from_nanos(0)
        );
        assert_eq!(
            transform_1.difference(&transform_2, TEST_REFERENCE),
            zx::SyntheticDuration::from_nanos(TEST_OFFSET.into_nanos())
        );
        assert_eq!(
            transform_2.difference(&transform_1, TEST_REFERENCE),
            zx::SyntheticDuration::from_nanos(-TEST_OFFSET.into_nanos())
        );
        assert_eq!(
            transform_1
                .difference(&transform_2, TEST_REFERENCE + zx::MonotonicDuration::from_millis(500)),
            zx::SyntheticDuration::from_nanos(TEST_OFFSET.into_nanos() + 75 * 500)
        );
        assert_eq!(
            transform_1
                .difference(&transform_2, TEST_REFERENCE - zx::MonotonicDuration::from_millis(300)),
            zx::SyntheticDuration::from_nanos(TEST_OFFSET.into_nanos() - 75 * 300)
        );
    }

    #[fuchsia::test]
    fn transform_conversion() {
        let transform = Transform {
            reference_offset: TEST_REFERENCE,
            synthetic_offset: zx::SyntheticInstant::from_nanos(
                (TEST_REFERENCE + TEST_OFFSET).into_nanos(),
            ),
            rate_adjust_ppm: -15,
            error_bound_at_offset: TEST_ERROR_BOUND,
            error_bound_growth_ppm: 0,
        };

        let monotonic = zx::MonotonicInstant::get();
        let clock_update = transform.jump_to(monotonic);
        assert_eq!(
            clock_update,
            zx::ClockUpdate::builder()
                .absolute_value(monotonic, transform.synthetic(monotonic))
                .rate_adjust(-15)
                .error_bounds(transform.error_bound(monotonic))
                .build()
        );

        let clock = zx::SyntheticClock::create(zx::ClockOpts::empty(), None).unwrap();
        clock.update(clock_update).unwrap();

        let double_converted = Transform::from(&clock);
        assert_eq!(double_converted.rate_adjust_ppm, transform.rate_adjust_ppm);
        assert_eq!(double_converted.error_bound_at_offset, transform.error_bound_at_offset);
        assert_eq!(double_converted.error_bound_growth_ppm, 0);
        assert_eq!(double_converted.rate_adjust_ppm, transform.rate_adjust_ppm);
        // Before RFC-0077 we accumulate some error in setting a clock, perform a coarse comparison.
        let synthetic_from_double_converted = double_converted.synthetic(TEST_REFERENCE);
        assert_geq!(
            synthetic_from_double_converted.into_nanos(),
            (TEST_REFERENCE + TEST_OFFSET - TOLERANCE).into_nanos()
        );
        assert_leq!(
            synthetic_from_double_converted.into_nanos(),
            (TEST_REFERENCE + TEST_OFFSET + TOLERANCE).into_nanos()
        );
    }

    #[fuchsia::test]
    fn time_at_monotonic_clock_not_started() {
        let clock = zx::SyntheticClock::create(zx::ClockOpts::empty(), Some(BACKSTOP)).unwrap();
        assert_eq!(time_at_monotonic(&clock, zx::MonotonicInstant::get() + TIME_DIFF), BACKSTOP);
    }

    #[fuchsia::test]
    fn time_at_monotonic_clock_started() {
        let clock = zx::SyntheticClock::create(zx::ClockOpts::empty(), Some(BACKSTOP)).unwrap();

        let mono = zx::MonotonicInstant::get();
        clock.update(zx::ClockUpdate::builder().absolute_value(mono, BACKSTOP)).unwrap();

        let clock_time = time_at_monotonic(&clock, mono + TIME_DIFF);
        assert_eq!(
            clock_time,
            BACKSTOP + zx::SyntheticDuration::from_nanos(TIME_DIFF.into_nanos())
        );
    }

    #[fuchsia::test]
    fn time_at_monotonic_clock_slew_fast() {
        let clock = zx::SyntheticClock::create(zx::ClockOpts::empty(), Some(BACKSTOP)).unwrap();

        let mono = zx::MonotonicInstant::get();
        clock
            .update(
                zx::ClockUpdate::builder()
                    .absolute_value(mono, BACKSTOP)
                    .rate_adjust(SLEW_RATE_PPM),
            )
            .unwrap();

        let clock_time = time_at_monotonic(&clock, mono + TIME_DIFF);
        assert_eq!(
            clock_time,
            BACKSTOP
                + zx::SyntheticDuration::from_nanos(
                    (TIME_DIFF * (ONE_MILLION + SLEW_RATE_PPM) / ONE_MILLION).into_nanos()
                )
        );
    }

    #[fuchsia::test]
    fn time_at_monotonic_clock_slew_slow() {
        let clock = zx::SyntheticClock::create(zx::ClockOpts::empty(), Some(BACKSTOP)).unwrap();

        let mono = zx::MonotonicInstant::get();
        clock
            .update(
                zx::ClockUpdate::builder()
                    .absolute_value(mono, BACKSTOP)
                    .rate_adjust(-SLEW_RATE_PPM),
            )
            .unwrap();

        let clock_time = time_at_monotonic(&clock, mono + TIME_DIFF);
        assert_eq!(
            clock_time,
            BACKSTOP
                + zx::SyntheticDuration::from_nanos(
                    (TIME_DIFF * (ONE_MILLION - SLEW_RATE_PPM) / ONE_MILLION).into_nanos()
                )
        );
    }
}
