// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::num_traits::{Bounded, FixedInt};

/// Clamps the given value to the range of the Target type by converting to i128.
pub fn clamp_cast<Source, Target>(value: Source) -> Target
where
    Source: Into<i128> + Copy,
    Target: Bounded + TryFrom<i128>,
{
    let val_i128: i128 = value.into();
    if let Ok(cast_val) = val_i128.try_into() {
        cast_val
    } else if val_i128 < 0 {
        Target::min_value()
    } else {
        Target::max_value()
    }
}

/// Clamps the given u128 value to the range of the Target type.
pub fn clamp_cast_u128<Target>(value: u128) -> Target
where
    Target: Bounded + TryFrom<u128>,
{
    if let Ok(cast_val) = value.try_into() { cast_val } else { Target::max_value() }
}

/// Rounds value to the given place using the convergent, or round-half-to-even, method.
///
/// Note on saturation near integer limits (e.g., `round_half_to_even(126i8, 6)`):
/// To match C++ FFL (`Format::Round`), when adding the round bit would overflow `I::MAX`,
/// the addition saturates to `I::MAX` before masking off the fractional bits. For example,
/// rounding `126i8` to bit 6 adds `32` (saturating to `127`) and masks by `!63`, returning `64`
/// (the numerically closest representable multiple of 64 without overflow).
pub fn round_half_to_even<I>(value: I, place: usize) -> I
where
    I: FixedInt + Bounded,
{
    if place == 0 {
        return value;
    }
    let bits = core::mem::size_of::<I>() * 8;
    if place >= bits {
        let zero = I::zero();
        let one = I::one();
        let two = one + one;
        if I::min_value() < zero {
            // Signed
            if value > I::max_value() / two {
                return I::max_value();
            } else if value < I::min_value() / two {
                return I::min_value();
            } else {
                return zero;
            }
        } else {
            // Unsigned: half threshold is 1 << (bits - 1)
            let half_threshold = one << (bits - 1);
            if value >= half_threshold {
                return I::max_value();
            } else {
                return zero;
            }
        }
    }
    if place == bits - 1 {
        let zero = I::zero();
        let one = I::one();
        let two = one + one;
        if I::min_value() < zero {
            // Signed
            let limit = I::min_value() / two;
            if value < limit {
                return I::min_value();
            }
        } else {
            // Unsigned
            let limit = one << (bits - 2);
            let grid = one << (bits - 1);
            if value > limit {
                return grid;
            }
        }
        return I::zero();
    }
    let one = I::one();
    let place_bit = one << place;
    let place_mask = !(place_bit - one);
    let half_bit = one << (place - 1);
    let below_half_mask = !place_mask >> 1;

    let round_bit =
        if (value & (place_bit | below_half_mask)) != I::zero() { half_bit } else { I::zero() };

    let max_val = I::max_value();
    let rounded: I = if max_val - round_bit < value { max_val } else { value + round_bit };
    rounded & place_mask
}

/// Performs a saturating left shift on an i128 value.
#[inline]
pub fn saturating_shl_i128(val: i128, bits: usize) -> i128 {
    if bits == 0 || val == 0 {
        return val;
    }
    if val > 0 {
        let safe = if bits >= 127 { false } else { (val as u128) <= ((i128::MAX as u128) >> bits) };
        if safe { val << bits } else { i128::MAX }
    } else {
        let abs = val.unsigned_abs();
        let safe = if bits >= 128 { false } else { abs <= (1u128 << (127 - bits)) };
        if safe { val << bits } else { i128::MIN }
    }
}
