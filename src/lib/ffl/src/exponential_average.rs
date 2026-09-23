// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fixed::{Fixed, PromoteAddSub};
use crate::fixed_format::Value;
use crate::num_traits;

/// An exponential moving average and variance tracker using fixed-point values.
///
/// This structure tracks a value's exponential moving average and its variance
/// using separate filtering coefficients (`alpha` for positive changes and `beta`
/// for negative changes, or a single coefficient for both).
/// Note: If V_FRAC is 0 (integer values), the variance update might lose significant
/// precision or decay to zero prematurely because internal calculations (like beta * delta^2)
/// are rounded to the nearest integer. Users should be encouraged to use types with
/// enough fractional bits for the accumulator if precision in the variance is important.
///
/// Additionally, because variance calculations involve squared delta terms (\Delta^2),
/// there is a high risk of saturation. It is strongly recommended to use larger integer
/// types (e.g. `i64` or `i128`) as the underlying type if large input ranges or high variance
/// values are expected.
pub struct ExponentialAverage<Val, Alpha, Beta> {
    average: Val,
    variance: Val,
    alpha: Alpha,
    beta: Beta,
}

impl<V, const V_FRAC: usize, A, const A_FRAC: usize, B, const B_FRAC: usize>
    ExponentialAverage<Fixed<V, V_FRAC>, Fixed<A, A_FRAC>, Fixed<B, B_FRAC>>
where
    V: num_traits::Bounded + num_traits::NumCast + Copy + PromoteAddSub<V> + num_traits::Signed,
    A: num_traits::Bounded + num_traits::NumCast + Copy + PromoteAddSub<A>,
    B: num_traits::Bounded + num_traits::NumCast + Copy + PromoteAddSub<B>,
    V::Output: num_traits::Bounded + num_traits::NumCast + Copy,
    A::Output: num_traits::Bounded + num_traits::NumCast + Copy,
    B::Output: num_traits::Bounded + num_traits::NumCast + Copy,
    Fixed<V, V_FRAC>: Copy
        + Default
        + PartialOrd
        + core::ops::AddAssign
        + core::ops::Sub<Output = Fixed<V::Output, V_FRAC>>
        + core::ops::Add<Output = Fixed<V::Output, V_FRAC>>,
    Fixed<V::Output, V_FRAC>: Copy + PartialOrd,
    Fixed<A, A_FRAC>: Copy + PartialOrd + core::ops::Sub<Output = Fixed<A::Output, A_FRAC>>,
    Fixed<B, B_FRAC>: Copy + PartialOrd + core::ops::Sub<Output = Fixed<B::Output, B_FRAC>>,
    Value<V, 0>: crate::fixed_format::ConvertValue<V, V_FRAC>,
    Value<V::Output, 0>: crate::fixed_format::ConvertValue<V::Output, V_FRAC>,
    Value<A, 0>: crate::fixed_format::ConvertValue<A, A_FRAC>,
    Value<B, 0>: crate::fixed_format::ConvertValue<B, B_FRAC>,
{
    /// Creates a new `ExponentialAverage` tracker with separate positive (`alpha`)
    /// and negative (`beta`) filtering coefficients.
    ///
    /// - `alpha` is used when the new sample is greater than or equal to the current average (positive change).
    /// - `beta` is used when the new sample is less than the current average (negative change).
    ///
    /// # Panics
    ///
    /// Panics if `alpha` or `beta` is not within the range `[0, 1]`.
    pub fn new(value: Fixed<V, V_FRAC>, alpha: Fixed<A, A_FRAC>, beta: Fixed<B, B_FRAC>) -> Self {
        assert!(alpha >= Fixed::from_integer(0));
        assert!(alpha <= Fixed::from_integer(1));
        assert!(beta >= Fixed::from_integer(0));
        assert!(beta <= Fixed::from_integer(1));

        Self { average: value, variance: Fixed::from_integer(0), alpha, beta }
    }

    /// Adds a new sample to the tracker, updating both the moving average and variance.
    pub fn add_sample(&mut self, sample: Fixed<V, V_FRAC>) {
        let delta = sample - self.average;
        let one_a = Fixed::<A, A_FRAC>::from_integer(1);
        let one_b = Fixed::<B, B_FRAC>::from_integer(1);

        if delta >= Fixed::from_integer(0) {
            let term1: Fixed<V, V_FRAC> = self.alpha.mul_to(delta);
            self.average += term1;
            let term2: Fixed<V, V_FRAC> = term1.mul_to(delta);
            let sum = self.variance + term2;
            self.variance = (one_a - self.alpha).mul_to(sum);
        } else {
            let term1: Fixed<V, V_FRAC> = self.beta.mul_to(delta);
            self.average += term1;
            let term2: Fixed<V, V_FRAC> = term1.mul_to(delta);
            let sum = self.variance + term2;
            self.variance = (one_b - self.beta).mul_to(sum);
        }
    }

    /// Returns the current moving average.
    pub fn value(&self) -> Fixed<V, V_FRAC> {
        self.average
    }

    /// Returns the current variance.
    pub fn variance(&self) -> Fixed<V, V_FRAC> {
        self.variance
    }
}

impl<V, const V_FRAC: usize, A, const A_FRAC: usize>
    ExponentialAverage<Fixed<V, V_FRAC>, Fixed<A, A_FRAC>, Fixed<A, A_FRAC>>
where
    V: num_traits::Bounded + num_traits::NumCast + Copy + PromoteAddSub<V> + num_traits::Signed,
    A: num_traits::Bounded + num_traits::NumCast + Copy + PromoteAddSub<A>,
    V::Output: num_traits::Bounded + num_traits::NumCast + Copy,
    A::Output: num_traits::Bounded + num_traits::NumCast + Copy,
    Fixed<V, V_FRAC>: Copy
        + Default
        + PartialOrd
        + core::ops::AddAssign
        + core::ops::Sub<Output = Fixed<V::Output, V_FRAC>>
        + core::ops::Add<Output = Fixed<V::Output, V_FRAC>>,
    Fixed<V::Output, V_FRAC>: Copy + PartialOrd,
    Fixed<A, A_FRAC>: Copy + PartialOrd + core::ops::Sub<Output = Fixed<A::Output, A_FRAC>>,
    Value<V, 0>: crate::fixed_format::ConvertValue<V, V_FRAC>,
    Value<V::Output, 0>: crate::fixed_format::ConvertValue<V::Output, V_FRAC>,
    Value<A, 0>: crate::fixed_format::ConvertValue<A, A_FRAC>,
{
    /// Creates a new `ExponentialAverage` tracker using a single coefficient
    /// for both positive and negative changes.
    pub fn new_single_rate(value: Fixed<V, V_FRAC>, alpha: Fixed<A, A_FRAC>) -> Self {
        Self::new(value, alpha, alpha)
    }
}
