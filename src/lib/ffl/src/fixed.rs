// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fixed_format::{ConvertValue, Value};
use crate::num_traits;

/// A fixed-point number represented by an underlying integer type and a
/// fractional bit depth.
///
/// `Fixed` provides precise, saturating arithmetic operations with convergent
/// rounding.
///
/// # Generics
///
/// - `I`: The underlying integer type (e.g. `i32`, `i64`).
/// - `FRAC`: The number of bits reserved for the fractional component.
#[repr(transparent)]
#[derive(Default, Clone, Copy, Hash)]
pub struct Fixed<I, const FRAC: usize> {
    value: I,
}

impl<I: Copy, const FRAC: usize> Fixed<I, FRAC> {
    const _ASSERT_VALID_FORMAT: () = {
        let bits = core::mem::size_of::<I>() * 8;
        assert!(bits <= 64, "The Integer template parameter must have at most 64 bits!");
        assert!(FRAC <= bits, "The number of fractional bits cannot exceed the integer bit-width!");
    };

    /// Returns the given raw integer as a fixed-point value in this format.
    pub const fn from_raw(value: I) -> Self {
        let () = Self::_ASSERT_VALID_FORMAT;
        Self { value }
    }

    /// Returns the raw fixed-point value as the underlying integer type.
    pub const fn raw_value(&self) -> I {
        self.value
    }

    /// Returns the fixed-point value as an intermediate value type.
    pub const fn value(&self) -> Value<I, FRAC> {
        Value { value: self.value }
    }

    /// Returns a Fixed value representing the ratio between numerator and denominator.
    pub fn from_ratio(numerator: I, denominator: I) -> Self
    where
        I: num_traits::Bounded + num_traits::NumCast + Copy,
        Value<I, 0>: crate::fixed_format::ConvertValue<i128, FRAC>,
    {
        let expr = crate::expression::from_ratio(numerator, denominator);
        Fixed::from(expr)
    }

    /// Multiplies self with another Fixed value and scales directly to target resolution.
    pub fn mul_to<U, const FRAC_R: usize, R, const FRAC_OUT: usize>(
        self,
        other: Fixed<U, FRAC_R>,
    ) -> Fixed<R, FRAC_OUT>
    where
        I: Into<i128> + Copy,
        U: Into<i128> + Copy,
        R: num_traits::Bounded + num_traits::NumCast + Copy,
    {
        let val_l: i128 = self.value.into();
        let val_r: i128 = other.value.into();

        let neg = (val_l < 0) ^ (val_r < 0);
        let a_abs = val_l.unsigned_abs();
        let b_abs = val_r.unsigned_abs();
        let prod_abs = a_abs.saturating_mul(b_abs);

        let src_frac = FRAC + FRAC_R;
        let dst_frac = FRAC_OUT;

        let scaled_abs = if src_frac > dst_frac {
            let delta = src_frac - dst_frac;
            let rounded = crate::utility::round_half_to_even::<u128>(prod_abs, delta);
            if let Some(power) = 1u128.checked_shl(delta as u32) { rounded / power } else { 0 }
        } else if src_frac < dst_frac {
            let delta = dst_frac - src_frac;
            if prod_abs <= (u128::MAX >> delta) { prod_abs << delta } else { u128::MAX }
        } else {
            prod_abs
        };

        // Convert the scaled absolute value back to signed (if necessary) and then to R
        let converted: R = if neg {
            let scaled_i128 = if scaled_abs > i128::MIN.unsigned_abs() {
                i128::MIN
            } else {
                (scaled_abs as i128).wrapping_neg()
            };
            crate::utility::clamp_cast(scaled_i128)
        } else {
            crate::utility::clamp_cast_u128(scaled_abs)
        };

        Fixed::from_raw(converted)
    }
}

impl<I: num_traits::Bounded + num_traits::NumCast + Copy, const FRAC: usize> Fixed<I, FRAC> {
    /// Returns the minimum value of this fixed point format.
    pub fn min() -> Self {
        Self::from_raw(I::min_value())
    }

    /// Returns the maximum value of this fixed point format.
    pub fn max() -> Self {
        Self::from_raw(I::max_value())
    }

    /// Returns the smallest positive value of this fixed point format.
    pub fn epsilon() -> Self
    where
        I: num_traits::One,
    {
        Self::from_raw(I::one())
    }

    /// Explicit conversion from an integer value.
    pub fn from_integer(val: i128) -> Self
    where
        I: num_traits::Bounded + num_traits::NumCast + Copy,
        Value<I, 0>: crate::fixed_format::ConvertValue<I, FRAC>,
    {
        let val_i: I = crate::utility::clamp_cast(val);
        Self::from(Value::<I, 0> { value: val_i }.convert_value())
    }

    /// Explicit conversion from another fixed-point format.
    pub fn from_fixed<OtherI: Copy, const OTHER_FRAC: usize>(
        other: Fixed<OtherI, OTHER_FRAC>,
    ) -> Self
    where
        Value<OtherI, OTHER_FRAC>: crate::fixed_format::ConvertValue<I, FRAC>,
    {
        Self::from(other.value().convert_value())
    }
}

impl<I: Copy, const FRAC: usize> From<Value<I, FRAC>> for Fixed<I, FRAC> {
    fn from(val: Value<I, FRAC>) -> Self {
        Self::from_raw(val.value)
    }
}

#[inline]
fn safe_shl_u128(val: u128, shift: usize) -> Option<u128> {
    if val == 0 {
        return Some(0);
    }
    if shift >= 128 {
        None
    } else {
        if val <= (u128::MAX >> shift) { Some(val << shift) } else { None }
    }
}

#[inline]
fn safe_shl_i128(val: i128, shift: usize) -> Option<i128> {
    if val == 0 {
        return Some(0);
    }
    if val > 0 {
        if shift >= 127 {
            None
        } else {
            if (val as u128) <= ((i128::MAX as u128) >> shift) { Some(val << shift) } else { None }
        }
    } else {
        let abs = val.unsigned_abs();
        let safe = if shift >= 128 { false } else { abs <= (1u128 << (127 - shift)) };
        if safe { Some(val << shift) } else { None }
    }
}

pub(crate) fn compare_raw(
    lhs_raw: i128,
    f1: usize,
    rhs_raw: i128,
    f2: usize,
) -> core::cmp::Ordering {
    if lhs_raw < 0 && rhs_raw >= 0 {
        return core::cmp::Ordering::Less;
    }
    if lhs_raw >= 0 && rhs_raw < 0 {
        return core::cmp::Ordering::Greater;
    }
    if f1 == f2 {
        return lhs_raw.cmp(&rhs_raw);
    }
    if lhs_raw >= 0 {
        let l = lhs_raw as u128;
        let r = rhs_raw as u128;
        if f1 > f2 {
            if let Some(val) = safe_shl_u128(r, f1 - f2) {
                l.cmp(&val)
            } else {
                core::cmp::Ordering::Less
            }
        } else {
            if let Some(val) = safe_shl_u128(l, f2 - f1) {
                val.cmp(&r)
            } else {
                core::cmp::Ordering::Greater
            }
        }
    } else {
        if f1 > f2 {
            if let Some(val) = safe_shl_i128(rhs_raw, f1 - f2) {
                lhs_raw.cmp(&val)
            } else {
                core::cmp::Ordering::Greater
            }
        } else {
            if let Some(val) = safe_shl_i128(lhs_raw, f2 - f1) {
                val.cmp(&rhs_raw)
            } else {
                core::cmp::Ordering::Less
            }
        }
    }
}

// Relational operators between different Fixed types (fully generic, using
// shift-based math to avoid generic const bounds)
impl<I, const F1: usize, OtherI, const F2: usize> PartialEq<Fixed<OtherI, F2>> for Fixed<I, F1>
where
    I: Into<i128> + Copy,
    OtherI: Into<i128> + Copy,
{
    fn eq(&self, other: &Fixed<OtherI, F2>) -> bool {
        let lhs_i: i128 = self.raw_value().into();
        let rhs_i: i128 = other.raw_value().into();
        compare_raw(lhs_i, F1, rhs_i, F2) == core::cmp::Ordering::Equal
    }
}

impl<I, const F1: usize, OtherI, const F2: usize> PartialOrd<Fixed<OtherI, F2>> for Fixed<I, F1>
where
    I: Into<i128> + Copy,
    OtherI: Into<i128> + Copy,
{
    fn partial_cmp(&self, other: &Fixed<OtherI, F2>) -> Option<core::cmp::Ordering> {
        let lhs_i: i128 = self.raw_value().into();
        let rhs_i: i128 = other.raw_value().into();
        Some(compare_raw(lhs_i, F1, rhs_i, F2))
    }
}

/// Compares two fixed-point numbers with potentially different underlying types
/// and fractional resolutions.
pub fn compare_fixed<I1, const F1: usize, I2, const F2: usize>(
    lhs: Fixed<I1, F1>,
    rhs: Fixed<I2, F2>,
) -> core::cmp::Ordering
where
    I1: Into<i128> + Copy,
    I2: Into<i128> + Copy,
{
    let lhs_i: i128 = lhs.raw_value().into();
    let rhs_i: i128 = rhs.raw_value().into();
    compare_raw(lhs_i, F1, rhs_i, F2)
}

// Relational operators between Fixed and integers
macro_rules! impl_fixed_integer_comparison {
    ($T:ty, $Int:ty) => {
        impl<const FRAC: usize> PartialEq<$Int> for Fixed<$T, FRAC>
        where
            Value<$T, FRAC>: crate::fixed_format::ConvertValue<i128, FRAC>,
            Value<$Int, 0>: crate::fixed_format::ConvertValue<i128, FRAC>,
        {
            fn eq(&self, other: &$Int) -> bool {
                let lhs: Value<i128, FRAC> = self.value().convert_value();
                let rhs: Value<i128, FRAC> = Value::<$Int, 0> { value: *other }.convert_value();
                lhs.value == rhs.value
            }
        }

        impl<const FRAC: usize> PartialEq<Fixed<$T, FRAC>> for $Int
        where
            Value<$T, FRAC>: crate::fixed_format::ConvertValue<i128, FRAC>,
            Value<$Int, 0>: crate::fixed_format::ConvertValue<i128, FRAC>,
        {
            fn eq(&self, other: &Fixed<$T, FRAC>) -> bool {
                other.eq(self)
            }
        }

        impl<const FRAC: usize> PartialOrd<$Int> for Fixed<$T, FRAC>
        where
            Value<$T, FRAC>: crate::fixed_format::ConvertValue<i128, FRAC>,
            Value<$Int, 0>: crate::fixed_format::ConvertValue<i128, FRAC>,
        {
            fn partial_cmp(&self, other: &$Int) -> Option<core::cmp::Ordering> {
                let lhs: Value<i128, FRAC> = self.value().convert_value();
                let rhs: Value<i128, FRAC> = Value::<$Int, 0> { value: *other }.convert_value();
                lhs.value.partial_cmp(&rhs.value)
            }
        }

        impl<const FRAC: usize> PartialOrd<Fixed<$T, FRAC>> for $Int
        where
            Value<$T, FRAC>: crate::fixed_format::ConvertValue<i128, FRAC>,
            Value<$Int, 0>: crate::fixed_format::ConvertValue<i128, FRAC>,
        {
            fn partial_cmp(&self, other: &Fixed<$T, FRAC>) -> Option<core::cmp::Ordering> {
                other.partial_cmp(self).map(|o| o.reverse())
            }
        }
    };
}

macro_rules! gen_fixed_integer_comparisons {
    (signed: [$($T:ty),*], $Int_list:tt) => {
        $(
            gen_fixed_integer_comparisons!(@signed_one $T, $Int_list);
        )*
    };
    (@signed_one $T:ty, [$($Int:ty),*]) => {
        $(
            impl_fixed_integer_comparison!($T, $Int);
        )*
    };

    (unsigned: [$($T:ty),*], $Int_list:tt) => {
        $(
            gen_fixed_integer_comparisons!(@unsigned_one $T, $Int_list);
        )*
    };
    (@unsigned_one $T:ty, [$($Int:ty),*]) => {
        $(
            impl_fixed_integer_comparison!($T, $Int);
        )*
    };

    (mixed: [$($T:ty),*], $Int_list:tt) => {
        $(
            gen_fixed_integer_comparisons!(@mixed_one $T, $Int_list);
        )*
    };
    (@mixed_one $T:ty, [$($Int:ty),*]) => {
        $(
            impl_fixed_integer_comparison!($T, $Int);
        )*
    };
}

gen_fixed_integer_comparisons!(signed: [i8, i16, i32, i64], [i8, i16, i32, i64]);
gen_fixed_integer_comparisons!(unsigned: [u8, u16, u32, u64], [u8, u16, u32, u64]);
gen_fixed_integer_comparisons!(mixed: [i8, i16, i32, i64], [u8, u16, u32, u64]);
gen_fixed_integer_comparisons!(mixed: [u8, u16, u32, u64], [i8, i16, i32, i64]);

impl<I, const FRAC: usize> Eq for Fixed<I, FRAC> where I: Into<i128> + Copy {}

/// Note: In Rust, `Ord` only permits comparing `Self` with `Self` (same integer type and fractional bit count `FRAC`).
/// Cross-scale and cross-type comparisons are provided via `PartialOrd`.
impl<I, const FRAC: usize> Ord for Fixed<I, FRAC>
where
    I: Into<i128> + Copy,
{
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        compare_fixed(*self, *other)
    }
}

// Logical methods on Fixed
impl<I, const FRAC: usize> Fixed<I, FRAC> {
    /// Returns the closest integer value greater-than or equal-to this fixed-point value.
    pub fn ceiling(&self) -> I
    where
        I: num_traits::Bounded
            + num_traits::NumCast
            + Copy
            + Into<i128>
            + num_traits::One
            + num_traits::Zero,
    {
        let raw_i: i128 = self.value.into();
        let power = if let Some(p) = 1i128.checked_shl(FRAC as u32) {
            p
        } else {
            return if raw_i > 0 { I::one() } else { I::zero() };
        };
        let div = raw_i / power;
        let rem = raw_i % power;

        let res = if rem != 0 && raw_i > 0 { div + 1 } else { div };
        crate::utility::clamp_cast(res)
    }

    /// Returns the closest integer value less-than or equal-to this fixed-point value.
    pub fn floor(&self) -> I
    where
        I: num_traits::Bounded + num_traits::NumCast + Copy + Into<i128> + num_traits::Zero,
    {
        let raw_i: i128 = self.value.into();
        let power = if let Some(p) = 1i128.checked_shl(FRAC as u32) {
            p
        } else {
            return if raw_i < 0 { crate::utility::clamp_cast(-1i128) } else { I::zero() };
        };
        let div = raw_i / power;
        let rem = raw_i % power;

        let res = if rem != 0 && raw_i < 0 { div - 1 } else { div };
        crate::utility::clamp_cast(res)
    }

    /// Returns the rounded value of this fixed-point value as an integer.
    pub fn round(&self) -> I
    where
        I: num_traits::Bounded + num_traits::NumCast + Copy + Into<i128> + num_traits::Zero,
    {
        let raw_i: i128 = self.value.into();
        let rounded = crate::utility::round_half_to_even::<i128>(raw_i, FRAC);
        let power = if let Some(p) = 1i128.checked_shl(FRAC as u32) {
            p
        } else {
            return I::zero();
        };
        let rounded_int = rounded / power;
        crate::utility::clamp_cast(rounded_int)
    }

    /// Returns the integral component of this fixed-point value.
    pub fn integral(&self) -> Self
    where
        I: num_traits::Bounded + num_traits::NumCast + PartialOrd + Copy,
    {
        if FRAC == 0 {
            return *self;
        }
        let raw_i: i128 = self.value.into();
        let mask = (!0u128).checked_shl(FRAC as u32).unwrap_or(0);
        let abs_integral = raw_i.unsigned_abs() & mask;
        let raw_integral = if raw_i < 0 { -(abs_integral as i128) } else { abs_integral as i128 };
        Self::from_raw(crate::utility::clamp_cast(raw_integral))
    }

    /// Returns the fractional component of this fixed-point value.
    pub fn fraction(&self) -> Self
    where
        I: num_traits::Bounded
            + num_traits::NumCast
            + PartialOrd
            + Copy
            + num_traits::Zero
            + core::ops::Sub<Output = I>,
    {
        if FRAC == 0 {
            return Self::from_raw(I::zero());
        }
        Self::from_raw(self.value - self.integral().value)
    }

    /// Returns the absolute value of this fixed-point value.
    ///
    /// Note: If the underlying value is the minimum value representable by a signed type
    /// (e.g., `I::MIN`), its mathematical absolute value cannot be represented and will
    /// saturate to `I::MAX`.
    pub fn absolute(&self) -> Self
    where
        I: num_traits::Bounded + num_traits::NumCast + Copy + Into<i128>,
    {
        let raw_i: i128 = self.value.into();
        if raw_i >= 0 {
            return *self;
        }
        let abs_raw = raw_i.unsigned_abs();
        Self::from_raw(crate::utility::clamp_cast_u128(abs_raw))
    }
}

// Promotion traits for operators
pub trait PromoteAddSub<U> {
    type Output;
}
pub trait PromoteMul<U> {
    type Output;
}

macro_rules! impl_promotes {
    ($T:ty, $U:ty => $AddSub:ty, $Mul:ty) => {
        impl PromoteAddSub<$U> for $T {
            type Output = $AddSub;
        }
        impl PromoteAddSub<$T> for $U {
            type Output = $AddSub;
        }
        impl PromoteMul<$U> for $T {
            type Output = $Mul;
        }
        impl PromoteMul<$T> for $U {
            type Output = $Mul;
        }
    };
}

macro_rules! impl_promotes_same {
    ($T:ty => $AddSub:ty, $Mul:ty) => {
        impl PromoteAddSub<$T> for $T {
            type Output = $AddSub;
        }
        impl PromoteMul<$T> for $T {
            type Output = $Mul;
        }
    };
}

impl_promotes_same!(u8 => u16, u16);
impl_promotes_same!(u16 => u32, u32);
impl_promotes_same!(u32 => u64, u64);
impl_promotes_same!(u64 => u64, u64);

impl_promotes_same!(i8 => i16, i16);
impl_promotes_same!(i16 => i32, i32);
impl_promotes_same!(i32 => i64, i64);
impl_promotes_same!(i64 => i64, i64);

impl_promotes!(u8, u16 => u32, u32);
impl_promotes!(u8, u32 => u64, u64);
impl_promotes!(u8, u64 => u64, u64);
impl_promotes!(u16, u32 => u64, u64);
impl_promotes!(u16, u64 => u64, u64);
impl_promotes!(u32, u64 => u64, u64);

impl_promotes!(i8, i16 => i32, i32);
impl_promotes!(i8, i32 => i64, i64);
impl_promotes!(i8, i64 => i64, i64);
impl_promotes!(i16, i32 => i64, i64);
impl_promotes!(i16, i64 => i64, i64);
impl_promotes!(i32, i64 => i64, i64);

impl_promotes!(i8, u8 => i16, i32);
impl_promotes!(i8, u16 => i32, i32);
impl_promotes!(i8, u32 => i64, i64);
impl_promotes!(i8, u64 => i64, i64);

impl_promotes!(i16, u8 => i32, i32);
impl_promotes!(i16, u16 => i32, i32);
impl_promotes!(i16, u32 => i64, i64);
impl_promotes!(i16, u64 => i64, i64);

impl_promotes!(i32, u8 => i64, i64);
impl_promotes!(i32, u16 => i64, i64);
impl_promotes!(i32, u32 => i64, i64);
impl_promotes!(i32, u64 => i64, i64);

impl_promotes!(i64, u8 => i64, i64);
impl_promotes!(i64, u16 => i64, i64);
impl_promotes!(i64, u32 => i64, i64);
impl_promotes!(i64, u64 => i64, i64);

// Operators supporting mixed-resolution addition and subtraction (returning LHS resolution)
impl<T, const FRAC: usize, U, const FRAC_R: usize> core::ops::Add<Fixed<U, FRAC_R>>
    for Fixed<T, FRAC>
where
    T: PromoteAddSub<U> + Copy,
    U: Copy,
    T::Output: num_traits::Bounded + num_traits::NumCast + Copy,
    Value<T, FRAC>: crate::fixed_format::ConvertValue<i128, FRAC>,
    Value<U, FRAC_R>: crate::fixed_format::ConvertValue<i128, FRAC>,
{
    type Output = Fixed<T::Output, FRAC>;

    fn add(self, rhs: Fixed<U, FRAC_R>) -> Self::Output {
        let lhs_promoted: Value<i128, FRAC> = self.value().convert_value();
        let rhs_promoted: Value<i128, FRAC> = rhs.value().convert_value();
        let sum: i128 =
            crate::saturating_arithmetic::saturate_add(lhs_promoted.value, rhs_promoted.value);
        Fixed::from_raw(crate::utility::clamp_cast(sum))
    }
}

impl<T, const FRAC: usize, U, const FRAC_R: usize> core::ops::Sub<Fixed<U, FRAC_R>>
    for Fixed<T, FRAC>
where
    T: PromoteAddSub<U> + Copy,
    U: Copy,
    T::Output: num_traits::Bounded + num_traits::NumCast + Copy,
    Value<T, FRAC>: crate::fixed_format::ConvertValue<i128, FRAC>,
    Value<U, FRAC_R>: crate::fixed_format::ConvertValue<i128, FRAC>,
{
    type Output = Fixed<T::Output, FRAC>;

    fn sub(self, rhs: Fixed<U, FRAC_R>) -> Self::Output {
        let lhs_promoted: Value<i128, FRAC> = self.value().convert_value();
        let rhs_promoted: Value<i128, FRAC> = rhs.value().convert_value();
        let diff: i128 =
            crate::saturating_arithmetic::saturate_subtract(lhs_promoted.value, rhs_promoted.value);
        Fixed::from_raw(crate::utility::clamp_cast(diff))
    }
}

impl<T, const FRAC: usize, U, const FRAC_R: usize> core::ops::Mul<Fixed<U, FRAC_R>>
    for Fixed<T, FRAC>
where
    T: PromoteMul<U> + Into<i128> + Copy,
    U: Into<i128> + Copy,
    T::Output: num_traits::Bounded + num_traits::NumCast + Copy,
{
    type Output = Fixed<T::Output, FRAC>;

    fn mul(self, rhs: Fixed<U, FRAC_R>) -> Self::Output {
        self.mul_to::<U, FRAC_R, T::Output, FRAC>(rhs)
    }
}

impl<I, const FRAC: usize> core::ops::Neg for Fixed<I, FRAC>
where
    I: num_traits::Bounded + num_traits::NumCast + Copy + num_traits::Signed + Into<i128>,
{
    type Output = Self;

    fn neg(self) -> Self::Output {
        let zero = 0i128;
        let negated = crate::saturating_arithmetic::saturate_subtract(zero, self.value);
        Self::from_raw(negated)
    }
}

// Commutative operators between Fixed and raw integers
macro_rules! impl_fixed_integer_arithmetic {
    ($T:ty, $Int:ty) => {
        impl<const FRAC: usize> core::ops::Add<$Int> for Fixed<$T, FRAC>
        where
            $T: PromoteAddSub<$Int> + Copy,
            $Int: Copy,
            <$T as PromoteAddSub<$Int>>::Output: num_traits::Bounded + num_traits::NumCast + Copy,
            Value<$T, FRAC>: crate::fixed_format::ConvertValue<i128, FRAC>,
            Value<$Int, 0>: crate::fixed_format::ConvertValue<i128, FRAC>,
        {
            type Output = Fixed<<$T as PromoteAddSub<$Int>>::Output, FRAC>;

            fn add(self, rhs: $Int) -> Self::Output {
                let lhs_promoted: Value<i128, FRAC> = self.value().convert_value();
                let rhs_promoted: Value<i128, FRAC> =
                    Value::<$Int, 0> { value: rhs }.convert_value();
                let sum: i128 = crate::saturating_arithmetic::saturate_add(
                    lhs_promoted.value,
                    rhs_promoted.value,
                );
                Fixed::from_raw(crate::utility::clamp_cast(sum))
            }
        }

        impl<const FRAC: usize> core::ops::Add<Fixed<$T, FRAC>> for $Int
        where
            $T: PromoteAddSub<$Int> + Copy,
            $Int: Copy,
            <$T as PromoteAddSub<$Int>>::Output: num_traits::Bounded + num_traits::NumCast + Copy,
            Value<$T, FRAC>: crate::fixed_format::ConvertValue<i128, FRAC>,
            Value<$Int, 0>: crate::fixed_format::ConvertValue<i128, FRAC>,
        {
            type Output = Fixed<<$T as PromoteAddSub<$Int>>::Output, FRAC>;

            fn add(self, rhs: Fixed<$T, FRAC>) -> Self::Output {
                let lhs_promoted: Value<i128, FRAC> =
                    Value::<$Int, 0> { value: self }.convert_value();
                let rhs_promoted: Value<i128, FRAC> = rhs.value().convert_value();
                let sum: i128 = crate::saturating_arithmetic::saturate_add(
                    lhs_promoted.value,
                    rhs_promoted.value,
                );
                Fixed::from_raw(crate::utility::clamp_cast(sum))
            }
        }

        impl<const FRAC: usize> core::ops::Sub<$Int> for Fixed<$T, FRAC>
        where
            $T: PromoteAddSub<$Int> + Copy,
            $Int: Copy,
            <$T as PromoteAddSub<$Int>>::Output: num_traits::Bounded + num_traits::NumCast + Copy,
            Value<$T, FRAC>: crate::fixed_format::ConvertValue<i128, FRAC>,
            Value<$Int, 0>: crate::fixed_format::ConvertValue<i128, FRAC>,
        {
            type Output = Fixed<<$T as PromoteAddSub<$Int>>::Output, FRAC>;

            fn sub(self, rhs: $Int) -> Self::Output {
                let lhs_promoted: Value<i128, FRAC> = self.value().convert_value();
                let rhs_promoted: Value<i128, FRAC> =
                    Value::<$Int, 0> { value: rhs }.convert_value();
                let diff: i128 = crate::saturating_arithmetic::saturate_subtract(
                    lhs_promoted.value,
                    rhs_promoted.value,
                );
                Fixed::from_raw(crate::utility::clamp_cast(diff))
            }
        }

        impl<const FRAC: usize> core::ops::Sub<Fixed<$T, FRAC>> for $Int
        where
            $T: PromoteAddSub<$Int> + Copy,
            $Int: Copy,
            <$T as PromoteAddSub<$Int>>::Output: num_traits::Bounded + num_traits::NumCast + Copy,
            Value<$T, FRAC>: crate::fixed_format::ConvertValue<i128, FRAC>,
            Value<$Int, 0>: crate::fixed_format::ConvertValue<i128, FRAC>,
        {
            type Output = Fixed<<$T as PromoteAddSub<$Int>>::Output, FRAC>;

            fn sub(self, rhs: Fixed<$T, FRAC>) -> Self::Output {
                let lhs_promoted: Value<i128, FRAC> =
                    Value::<$Int, 0> { value: self }.convert_value();
                let rhs_promoted: Value<i128, FRAC> = rhs.value().convert_value();
                let diff: i128 = crate::saturating_arithmetic::saturate_subtract(
                    lhs_promoted.value,
                    rhs_promoted.value,
                );
                Fixed::from_raw(crate::utility::clamp_cast(diff))
            }
        }

        impl<const FRAC: usize> core::ops::Mul<$Int> for Fixed<$T, FRAC>
        where
            $T: PromoteMul<$Int> + Copy,
            $Int: Copy,
            <$T as PromoteMul<$Int>>::Output: num_traits::Bounded + num_traits::NumCast + Copy,
            Value<$T, FRAC>: crate::fixed_format::ConvertValue<i128, FRAC>,
        {
            type Output = Fixed<<$T as PromoteMul<$Int>>::Output, FRAC>;

            fn mul(self, rhs: $Int) -> Self::Output {
                let lhs_promoted: Value<i128, FRAC> = self.value().convert_value();
                let rhs_i128: i128 = rhs.into();
                let prod: i128 =
                    crate::saturating_arithmetic::saturate_multiply(lhs_promoted.value, rhs_i128);
                Fixed::from_raw(crate::utility::clamp_cast(prod))
            }
        }

        impl<const FRAC: usize> core::ops::Mul<Fixed<$T, FRAC>> for $Int
        where
            $T: PromoteMul<$Int> + Copy,
            $Int: Copy,
            <$T as PromoteMul<$Int>>::Output: num_traits::Bounded + num_traits::NumCast + Copy,
            Value<$T, FRAC>: crate::fixed_format::ConvertValue<i128, FRAC>,
        {
            type Output = Fixed<<$T as PromoteMul<$Int>>::Output, FRAC>;

            fn mul(self, rhs: Fixed<$T, FRAC>) -> Self::Output {
                rhs.mul(self)
            }
        }

        impl<const FRAC: usize> core::ops::AddAssign<$Int> for Fixed<$T, FRAC>
        where
            $T: PromoteAddSub<$Int> + num_traits::Bounded + num_traits::NumCast + Copy,
            $Int: Copy,
            <$T as PromoteAddSub<$Int>>::Output: num_traits::Bounded + num_traits::NumCast + Copy,
            Value<$T, FRAC>: crate::fixed_format::ConvertValue<i128, FRAC>,
            Value<$Int, 0>: crate::fixed_format::ConvertValue<i128, FRAC>,
        {
            fn add_assign(&mut self, rhs: $Int) {
                let sum = core::ops::Add::add(*self, rhs);
                self.value = crate::utility::clamp_cast(sum.raw_value());
            }
        }

        impl<const FRAC: usize> core::ops::SubAssign<$Int> for Fixed<$T, FRAC>
        where
            $T: PromoteAddSub<$Int> + num_traits::Bounded + num_traits::NumCast + Copy,
            $Int: Copy,
            <$T as PromoteAddSub<$Int>>::Output: num_traits::Bounded + num_traits::NumCast + Copy,
            Value<$T, FRAC>: crate::fixed_format::ConvertValue<i128, FRAC>,
            Value<$Int, 0>: crate::fixed_format::ConvertValue<i128, FRAC>,
        {
            fn sub_assign(&mut self, rhs: $Int) {
                let diff = core::ops::Sub::sub(*self, rhs);
                self.value = crate::utility::clamp_cast(diff.raw_value());
            }
        }

        impl<const FRAC: usize> core::ops::MulAssign<$Int> for Fixed<$T, FRAC>
        where
            $T: PromoteMul<$Int> + num_traits::Bounded + num_traits::NumCast + Copy,
            $Int: Copy,
            <$T as PromoteMul<$Int>>::Output: num_traits::Bounded + num_traits::NumCast + Copy,
            Value<$T, FRAC>: crate::fixed_format::ConvertValue<i128, FRAC>,
        {
            fn mul_assign(&mut self, rhs: $Int) {
                let prod = core::ops::Mul::mul(*self, rhs);
                self.value = crate::utility::clamp_cast(prod.raw_value());
            }
        }
    };
}

macro_rules! gen_fixed_integer_arithmetic {
    (signed: [$($T:ty),*], $Int_list:tt) => {
        $(
            gen_fixed_integer_arithmetic!(@one $T, $Int_list);
        )*
    };
    (unsigned: [$($T:ty),*], $Int_list:tt) => {
        $(
            gen_fixed_integer_arithmetic!(@one $T, $Int_list);
        )*
    };
    (mixed: [$($T:ty),*], $Int_list:tt) => {
        $(
            gen_fixed_integer_arithmetic!(@one $T, $Int_list);
        )*
    };
    (@one $T:ty, [$($Int:ty),*]) => {
        $(
            impl_fixed_integer_arithmetic!($T, $Int);
        )*
    };
}

gen_fixed_integer_arithmetic!(signed: [i8, i16, i32, i64], [i8, i16, i32, i64]);
gen_fixed_integer_arithmetic!(unsigned: [u8, u16, u32, u64], [u8, u16, u32, u64]);
gen_fixed_integer_arithmetic!(mixed: [i8, i16, i32, i64], [u8, u16, u32, u64]);
gen_fixed_integer_arithmetic!(mixed: [u8, u16, u32, u64], [i8, i16, i32, i64]);
// Compound assignments (supporting mixed types and resolutions)
impl<I, const FRAC: usize, U, const FRAC_R: usize> core::ops::AddAssign<Fixed<U, FRAC_R>>
    for Fixed<I, FRAC>
where
    I: PromoteAddSub<U> + Copy + num_traits::Bounded + num_traits::NumCast + TryFrom<I::Output>,
    U: Copy,
    I::Output: num_traits::Bounded + num_traits::NumCast + Copy + PartialOrd + num_traits::Zero,
    Value<I, FRAC>: crate::fixed_format::ConvertValue<i128, FRAC>,
    Value<U, FRAC_R>: crate::fixed_format::ConvertValue<i128, FRAC>,
{
    fn add_assign(&mut self, rhs: Fixed<U, FRAC_R>) {
        let sum = *self + rhs;
        self.value = crate::utility::clamp_cast(sum.raw_value());
    }
}

impl<I, const FRAC: usize, U, const FRAC_R: usize> core::ops::SubAssign<Fixed<U, FRAC_R>>
    for Fixed<I, FRAC>
where
    I: PromoteAddSub<U> + Copy + num_traits::Bounded + num_traits::NumCast + TryFrom<I::Output>,
    U: Copy,
    I::Output: num_traits::Bounded + num_traits::NumCast + Copy + PartialOrd + num_traits::Zero,
    Value<I, FRAC>: crate::fixed_format::ConvertValue<i128, FRAC>,
    Value<U, FRAC_R>: crate::fixed_format::ConvertValue<i128, FRAC>,
{
    fn sub_assign(&mut self, rhs: Fixed<U, FRAC_R>) {
        let diff = *self - rhs;
        self.value = crate::utility::clamp_cast(diff.raw_value());
    }
}

impl<I, const FRAC: usize> core::ops::MulAssign<Fixed<I, FRAC>> for Fixed<I, FRAC>
where
    I: num_traits::Bounded + num_traits::NumCast + Into<i128> + Copy,
{
    fn mul_assign(&mut self, rhs: Self) {
        *self = self.mul_to(rhs);
    }
}

impl<I, const FRAC: usize> core::ops::DivAssign<Fixed<I, FRAC>> for Fixed<I, FRAC>
where
    Self: Copy,
    Fixed<I, FRAC>: From<crate::expression::DivExpression<Self, Self>>,
{
    fn div_assign(&mut self, rhs: Self) {
        *self = Self::from(crate::expression::DivExpression { numerator: *self, denominator: rhs });
    }
}

// Display, Debug, LowerHex implementations
impl<I, const FRAC: usize> core::fmt::Display for Fixed<I, FRAC>
where
    I: Into<i128> + Copy,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let s = crate::string::String::new(*self, crate::string::Mode::Dec, f.precision());
        f.write_str(s.as_str())
    }
}

impl<I, const FRAC: usize> core::fmt::Debug for Fixed<I, FRAC>
where
    I: Into<i128> + Copy + core::fmt::Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let s = crate::string::String::new(*self, crate::string::Mode::Dec, None);
        write!(f, "Fixed(raw: {:?}, value: {})", self.raw_value(), s.as_str())
    }
}

impl<I, const FRAC: usize> core::fmt::LowerHex for Fixed<I, FRAC>
where
    I: Into<i128> + Copy,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let s = crate::string::String::new(*self, crate::string::Mode::Hex, Some(0));
        f.write_str(s.as_str())
    }
}

/// A helper struct that implements `Display` to format a fixed-point number
/// as a rational value (e.g., `1+1/2`).
pub struct RationalFormatter<'a, I, const FRAC: usize>(&'a Fixed<I, FRAC>);

impl<I, const FRAC: usize> core::fmt::Display for RationalFormatter<'_, I, FRAC>
where
    I: Into<i128> + Copy,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let s = crate::string::String::new(*self.0, crate::string::Mode::DecRational, Some(0));
        f.write_str(s.as_str())
    }
}

impl<I, const FRAC: usize> Fixed<I, FRAC> {
    /// Returns a helper that displays the fixed-point number formatted as
    /// a rational fraction (e.g., `1+1/2`).
    pub fn rational(&self) -> RationalFormatter<'_, I, FRAC> {
        RationalFormatter(self)
    }
}
