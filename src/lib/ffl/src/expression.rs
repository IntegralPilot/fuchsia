// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fixed::Fixed;

/// An expression representing deferred division.
///
/// Evaluations are deferred to preserve precision, scaling only when converted
/// to a target resolution or assigned to a `Fixed` variable.
pub struct DivExpression<L, R> {
    /// The numerator of the division expression.
    pub numerator: L,
    /// The denominator of the division expression.
    pub denominator: R,
}

// Division operator for Fixed / Fixed
impl<IL, const FL: usize, IR, const FR: usize> core::ops::Div<Fixed<IR, FR>> for Fixed<IL, FL> {
    type Output = DivExpression<Fixed<IL, FL>, Fixed<IR, FR>>;

    fn div(self, rhs: Fixed<IR, FR>) -> Self::Output {
        DivExpression { numerator: self, denominator: rhs }
    }
}

// Division operator for Fixed / Integer (promoted to Fixed<Integer, 0>)
macro_rules! impl_div_by_int {
    ($I_R:ty) => {
        impl<IL, const FL: usize> core::ops::Div<$I_R> for Fixed<IL, FL> {
            type Output = DivExpression<Fixed<IL, FL>, Fixed<$I_R, 0>>;

            fn div(self, rhs: $I_R) -> Self::Output {
                DivExpression { numerator: self, denominator: Fixed::from_raw(rhs) }
            }
        }
    };
}

impl_div_by_int!(i8);
impl_div_by_int!(i16);
impl_div_by_int!(i32);
impl_div_by_int!(i64);
impl_div_by_int!(u8);
impl_div_by_int!(u16);
impl_div_by_int!(u32);
impl_div_by_int!(u64);

// Conversion from DivExpression to Fixed (performs the actual scaled division in raw i128 space)
impl<I, const TARGET_FRAC: usize, IL, const FL: usize, IR, const FR: usize>
    From<DivExpression<Fixed<IL, FL>, Fixed<IR, FR>>> for Fixed<I, TARGET_FRAC>
where
    I: crate::num_traits::Bounded + TryFrom<i128> + Copy,
    IL: Into<i128> + Copy,
    IR: Into<i128> + Copy,
{
    fn from(expr: DivExpression<Fixed<IL, FL>, Fixed<IR, FR>>) -> Self {
        let num_raw: i128 = expr.numerator.raw_value().into();
        let denom_raw: i128 = expr.denominator.raw_value().into();

        let src_frac = FL;
        let dst_frac = TARGET_FRAC + FR;

        let num_scaled = if src_frac > dst_frac {
            let delta = src_frac - dst_frac;
            let rounded = crate::utility::round_half_to_even::<i128>(num_raw, delta);
            if let Some(power) = 1i128.checked_shl(delta as u32) { rounded / power } else { 0 }
        } else if src_frac < dst_frac {
            let delta = dst_frac - src_frac;
            // Note on saturation: Even if saturating_shl_i128 saturates to i128::MAX/MIN,
            // dividing by denom_raw (at most 64-bit i64) leaves a quotient >= 2^64, which
            // clamp_cast correctly clamps to I::MAX/MIN (since I is at most 64 bits).
            crate::utility::saturating_shl_i128(num_raw, delta)
        } else {
            num_raw
        };

        if denom_raw == 0 {
            panic!("Division by zero in DivExpression");
        }

        let quotient = num_scaled.checked_div(denom_raw).unwrap_or(i128::MAX);

        let val: I = crate::utility::clamp_cast(quotient);
        Self::from_raw(val)
    }
}

/// Helper to create a Division Expression from a numerator and denominator.
pub fn from_ratio<IL: Copy, IR: Copy>(
    numerator: IL,
    denominator: IR,
) -> DivExpression<Fixed<IL, 0>, Fixed<IR, 0>> {
    DivExpression {
        numerator: Fixed::from_raw(numerator),
        denominator: Fixed::from_raw(denominator),
    }
}

/// Helper to coerce a division expression to a specific target resolution.
pub fn to_resolution<const TARGET_FRAC: usize, I, L, R>(
    expr: DivExpression<L, R>,
) -> Fixed<I, TARGET_FRAC>
where
    Fixed<I, TARGET_FRAC>: From<DivExpression<L, R>>,
{
    Fixed::from(expr)
}
