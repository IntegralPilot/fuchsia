// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// Helper compile-time constants and utilities for a fixed-point format.
pub struct FixedFormat<I, const FRAC: usize>(core::marker::PhantomData<I>);

/// A wrapper type representing an un-scaled raw value in a fixed-point format,
/// used for intermediate conversion step calculations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Value<I, const FRAC: usize> {
    /// The underlying raw value.
    pub value: I,
}

macro_rules! impl_fixed_format {
    ($I:ty, $is_signed:expr, $bits:expr) => {
        impl<const FRAC: usize> FixedFormat<$I, FRAC> {
            pub const BITS: usize = $bits;
            pub const FRACTIONAL_BITS: usize = FRAC;
            pub const IS_SIGNED: bool = $is_signed;
            pub const IS_UNSIGNED: bool = !$is_signed;
            const _ASSERT_STRICT_FORMAT: () = {
                let bits = core::mem::size_of::<$I>() * 8;
                if $is_signed {
                    assert!(
                        FRAC < bits,
                        "The number of fractional bits must be less than the total bit-width for signed types."
                    );
                } else {
                    assert!(
                        FRAC <= bits,
                        "The number of fractional bits must be less than or equal to the total bit-width for unsigned types."
                    );
                }
            };
            pub const INTEGRAL_BITS: usize = {
                let _ = Self::_ASSERT_STRICT_FORMAT;
                $bits - FRAC - (if $is_signed { 1 } else { 0 })
            };
            pub const POWER: usize =
                if FRAC >= $bits { 0 } else { 1usize.wrapping_shl(FRAC as u32) };
            pub const FRACTIONAL_MASK: $I = if FRAC >= $bits {
                !0 as $I
            } else {
                (1 as $I).wrapping_shl(FRAC as u32).wrapping_sub(1)
            };
            pub const INTEGRAL_MASK: $I = !Self::FRACTIONAL_MASK;
            pub const SIGN_BIT: $I =
                if $is_signed { (1 as $I).wrapping_shl(($bits - 1) as u32) } else { 0 };
            pub const BINARY_POINT: $I =
                if FRAC > 0 { (1 as $I).wrapping_shl((FRAC - 1) as u32) } else { 0 };
            pub const ONES_PLACE: $I = (1 as $I).wrapping_shl(FRAC as u32);
            pub const APPROXIMATE_UNIT: bool = ($is_signed && FRAC == $bits - 1) || FRAC == $bits;
            pub const ADJUSTED_FRACTIONAL_BITS: usize =
                FRAC - (if Self::APPROXIMATE_UNIT { 1 } else { 0 });
            pub const ADJUSTED_POWER: $I =
                (1 as $I).wrapping_shl(Self::ADJUSTED_FRACTIONAL_BITS as u32);
            pub const ADJUSTMENT_FACTOR: $I =
                if Self::APPROXIMATE_UNIT { 2 as $I } else { 1 as $I };
            pub const ADJUSTED_FRACTIONAL_MASK: $I =
                (1 as $I).wrapping_shl(Self::ADJUSTED_FRACTIONAL_BITS as u32).wrapping_sub(1);
            pub const ADJUSTED_INTEGRAL_MASK: $I = !Self::ADJUSTED_FRACTIONAL_MASK;
            pub const MIN: $I = <$I>::MIN;
            pub const MAX: $I = <$I>::MAX;
            pub const INTEGRAL_MIN: $I =
                if FRAC >= $bits { 0 } else { ((<$I>::MIN as i128) / (Self::POWER as i128)) as $I };
            pub const INTEGRAL_MAX: $I =
                if FRAC >= $bits { 0 } else { ((<$I>::MAX as i128) / (Self::POWER as i128)) as $I };



            pub fn round(value: $I) -> $I {
                crate::utility::round_half_to_even::<$I>(value, FRAC)
            }

            pub fn round_to_place(value: $I, place: usize) -> $I {
                crate::utility::round_half_to_even::<$I>(value, place)
            }
        }
    };
}

impl_fixed_format!(i8, true, 8);
impl_fixed_format!(i16, true, 16);
impl_fixed_format!(i32, true, 32);
impl_fixed_format!(i64, true, 64);
impl_fixed_format!(u8, false, 8);
impl_fixed_format!(u16, false, 16);
impl_fixed_format!(u32, false, 32);
impl_fixed_format!(u64, false, 64);

/// A trait for performing precision-preserving scaling and conversion between
/// raw values of different fixed-point formats.
pub trait ConvertValue<IDst, const FRAC_DST: usize> {
    /// Converts `self` to a `Value` of another fixed-point format.
    fn convert_value(self) -> Value<IDst, FRAC_DST>;
}

macro_rules! impl_convert_value {
    (unsigned: $I_SRC:ty, $I_DST:ty) => {
        impl<const FRAC_SRC: usize, const FRAC_DST: usize> ConvertValue<$I_DST, FRAC_DST>
            for Value<$I_SRC, FRAC_SRC>
        {
            fn convert_value(self) -> Value<$I_DST, FRAC_DST> {
                let val = self.value;
                let promoted_value = val as i128;

                let src_adj_frac = FixedFormat::<$I_SRC, FRAC_SRC>::ADJUSTED_FRACTIONAL_BITS;
                let _dst_adj_frac = FixedFormat::<$I_DST, FRAC_DST>::ADJUSTED_FRACTIONAL_BITS;

                let src_adj_factor = FixedFormat::<$I_SRC, FRAC_SRC>::ADJUSTMENT_FACTOR as i128;
                let _dst_adj_factor = FixedFormat::<$I_DST, FRAC_DST>::ADJUSTMENT_FACTOR as i128;

                if FRAC_SRC > FRAC_DST {
                    let shifted_value = promoted_value / src_adj_factor;
                    let delta = src_adj_frac - FRAC_DST;
                    let rounded = crate::utility::round_half_to_even::<i128>(shifted_value, delta);
                    let power = 1i128 << delta;
                    let converted_value = rounded / power;
                    Value { value: crate::utility::clamp_cast(converted_value) }
                } else if FRAC_SRC < FRAC_DST {
                    let delta = FRAC_DST - FRAC_SRC;
                    let power = 1i128 << delta;
                    let converted_value: $I_DST =
                        crate::saturating_arithmetic::saturate_multiply(promoted_value, power);
                    Value { value: converted_value }
                } else {
                    Value { value: crate::utility::clamp_cast(promoted_value) }
                }
            }
        }
    };
    (mixed: $I_SRC:ty, $I_DST:ty) => {
        impl<const FRAC_SRC: usize, const FRAC_DST: usize> ConvertValue<$I_DST, FRAC_DST>
            for Value<$I_SRC, FRAC_SRC>
        {
            fn convert_value(self) -> Value<$I_DST, FRAC_DST> {
                let val = self.value;
                let promoted_value: i128 = crate::utility::clamp_cast(val);

                let src_adj_frac = FixedFormat::<$I_SRC, FRAC_SRC>::ADJUSTED_FRACTIONAL_BITS;
                let _dst_adj_frac = FixedFormat::<$I_DST, FRAC_DST>::ADJUSTED_FRACTIONAL_BITS;

                let src_adj_factor = FixedFormat::<$I_SRC, FRAC_SRC>::ADJUSTMENT_FACTOR as i128;
                let _dst_adj_factor = FixedFormat::<$I_DST, FRAC_DST>::ADJUSTMENT_FACTOR as i128;

                if FRAC_SRC > FRAC_DST {
                    let shifted_value = promoted_value / src_adj_factor;
                    let delta = src_adj_frac - FRAC_DST;
                    let rounded = crate::utility::round_half_to_even::<i128>(shifted_value, delta);
                    let power = 1i128 << delta;
                    let converted_value = rounded / power;
                    Value { value: crate::utility::clamp_cast(converted_value) }
                } else if FRAC_SRC < FRAC_DST {
                    let delta = FRAC_DST - FRAC_SRC;
                    let power = 1i128 << delta;
                    let converted_value: $I_DST =
                        crate::saturating_arithmetic::saturate_multiply(promoted_value, power);
                    Value { value: converted_value }
                } else {
                    Value { value: crate::utility::clamp_cast(promoted_value) }
                }
            }
        }
    };
}

macro_rules! gen_convert_values {
    (unsigned: [$($T:ty),*], $U_list:tt) => {
        $(
            gen_convert_values!(@unsigned_one $T, $U_list);
        )*
    };
    (@unsigned_one $T:ty, [$($U:ty),*]) => {
        $(
            impl_convert_value!(unsigned: $T, $U);
        )*
    };

    (mixed: [$($T:ty),*], $U_list:tt) => {
        $(
            gen_convert_values!(@mixed_one $T, $U_list);
        )*
    };
    (@mixed_one $T:ty, [$($U:ty),*]) => {
        $(
            impl_convert_value!(mixed: $T, $U);
        )*
    };
}

gen_convert_values!(unsigned: [u8, u16, u32, u64], [u8, u16, u32, u64]);
gen_convert_values!(mixed: [i8, i16, i32, i64], [i8, i16, i32, i64]);
gen_convert_values!(mixed: [i8, i16, i32, i64], [u8, u16, u32, u64]);
gen_convert_values!(mixed: [u8, u16, u32, u64], [i8, i16, i32, i64]);

macro_rules! impl_convert_value_i128 {
    ($I_SRC:ty) => {
        impl<const FRAC_SRC: usize, const FRAC_DST: usize> ConvertValue<i128, FRAC_DST>
            for Value<$I_SRC, FRAC_SRC>
        {
            fn convert_value(self) -> Value<i128, FRAC_DST> {
                let val = self.value;
                let promoted_value: i128 = crate::utility::clamp_cast(val);

                let src_adj_frac = FixedFormat::<$I_SRC, FRAC_SRC>::ADJUSTED_FRACTIONAL_BITS;
                let src_adj_factor = FixedFormat::<$I_SRC, FRAC_SRC>::ADJUSTMENT_FACTOR as i128;

                if FRAC_SRC > FRAC_DST {
                    let shifted_value = promoted_value / src_adj_factor;
                    let delta = src_adj_frac - FRAC_DST;
                    let rounded = crate::utility::round_half_to_even::<i128>(shifted_value, delta);
                    let power = 1i128 << delta;
                    let converted_value = rounded / power;
                    Value { value: converted_value }
                } else if FRAC_SRC < FRAC_DST {
                    let delta = FRAC_DST - FRAC_SRC;
                    let power = 1i128 << delta;
                    let converted_value: i128 =
                        crate::saturating_arithmetic::saturate_multiply(promoted_value, power);
                    Value { value: converted_value }
                } else {
                    Value { value: promoted_value }
                }
            }
        }
    };
}

impl_convert_value_i128!(i8);
impl_convert_value_i128!(i16);
impl_convert_value_i128!(i32);
impl_convert_value_i128!(i64);
impl_convert_value_i128!(u8);
impl_convert_value_i128!(u16);
impl_convert_value_i128!(u32);
impl_convert_value_i128!(u64);
