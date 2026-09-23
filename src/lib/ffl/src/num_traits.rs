// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub trait Zero: Sized {
    fn zero() -> Self;
    fn is_zero(&self) -> bool;
}

pub trait One: Sized {
    fn one() -> Self;
}

pub trait Bounded {
    fn min_value() -> Self;
    fn max_value() -> Self;
}

pub trait Signed {}

pub trait NumCast: TryFrom<i128> + Into<i128> + TryFrom<u128> {}
impl<T: TryFrom<i128> + Into<i128> + TryFrom<u128>> NumCast for T {}

pub trait SaturatingAdd: Sized {
    fn saturating_add(&self, v: &Self) -> Self;
}

pub trait SaturatingSub: Sized {
    fn saturating_sub(&self, v: &Self) -> Self;
}

pub trait SaturatingMul: Sized {
    fn saturating_mul(&self, v: &Self) -> Self;
}

pub trait CheckedDiv: Sized {
    fn checked_div(&self, v: &Self) -> Option<Self>;
}

pub trait FixedInt:
    Copy
    + Ord
    + Zero
    + One
    + SaturatingAdd
    + SaturatingSub
    + SaturatingMul
    + CheckedDiv
    + core::ops::BitAnd<Output = Self>
    + core::ops::BitOr<Output = Self>
    + core::ops::Not<Output = Self>
    + core::ops::Shl<usize, Output = Self>
    + core::ops::Shr<usize, Output = Self>
    + core::ops::Add<Output = Self>
    + core::ops::Sub<Output = Self>
    + core::ops::Mul<Output = Self>
    + core::ops::Div<Output = Self>
{
}

pub trait PrimInt: FixedInt {}
impl<T: FixedInt> PrimInt for T {}

macro_rules! impl_numeric_traits {
    ($($t:ty),*) => {
        $(
            impl Zero for $t {
                #[inline]
                fn zero() -> Self { 0 }
                #[inline]
                fn is_zero(&self) -> bool { *self == 0 }
            }

            impl One for $t {
                #[inline]
                fn one() -> Self { 1 }
            }

            impl Bounded for $t {
                #[inline]
                fn min_value() -> Self { <$t>::MIN }
                #[inline]
                fn max_value() -> Self { <$t>::MAX }
            }

            impl SaturatingAdd for $t {
                #[inline]
                fn saturating_add(&self, v: &Self) -> Self { (*self).saturating_add(*v) }
            }

            impl SaturatingSub for $t {
                #[inline]
                fn saturating_sub(&self, v: &Self) -> Self { (*self).saturating_sub(*v) }
            }

            impl SaturatingMul for $t {
                #[inline]
                fn saturating_mul(&self, v: &Self) -> Self { (*self).saturating_mul(*v) }
            }

            impl CheckedDiv for $t {
                #[inline]
                fn checked_div(&self, v: &Self) -> Option<Self> { (*self).checked_div(*v) }
            }

            impl FixedInt for $t {}
        )*
    };
}

impl_numeric_traits!(i8, i16, i32, i64, i128, u8, u16, u32, u64, u128);

macro_rules! impl_signed {
    ($($t:ty),*) => {
        $(
            impl Signed for $t {}
        )*
    };
}

impl_signed!(i8, i16, i32, i64, i128);
