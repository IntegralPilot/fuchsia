// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fixed::Fixed;

/// Formatting modes for displaying fixed-point values.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Decimal format (e.g. `1.23`).
    Dec,
    /// Hexadecimal format (e.g. `0x1.3b`).
    Hex,
    /// Rational format (e.g. `1+1/2`).
    DecRational,
}

/// A stack-allocated, fixed-size string buffer for formatting fixed-point values
/// without heap allocations.
pub struct String {
    buffer: [u8; 128],
    length: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ZeroMode {
    NoLeadingZeros,
    NoTrailingZeros,
}

impl String {
    /// Creates an empty string buffer.
    pub fn empty() -> Self {
        Self { buffer: [0u8; 128], length: 0 }
    }

    /// Formats the given fixed-point value in the specified mode and precision.
    pub fn new<I, const FRAC: usize>(
        value: Fixed<I, FRAC>,
        mode: Mode,
        precision: Option<usize>,
    ) -> Self
    where
        I: Into<i128> + Copy,
    {
        let mut s = Self::empty();
        match mode {
            Mode::Dec => s.write_dec(value, precision),
            Mode::Hex => s.write_hex(value),
            Mode::DecRational => s.write_dec_rational(value),
        }
        s
    }

    /// Returns a string slice referencing the formatted string.
    pub fn as_str(&self) -> &str {
        self
    }

    fn push_byte(&mut self, b: u8) {
        if self.length < self.buffer.len() {
            self.buffer[self.length] = b;
            self.length += 1;
        }
    }

    fn write_dec_integer(&mut self, value: u128) {
        let mut digits = [0u8; 39];
        let mut len = 0;
        let mut rem = value;
        loop {
            digits[len] = b'0' + (rem % 10) as u8;
            len += 1;
            rem /= 10;
            if rem == 0 {
                break;
            }
        }
        for i in (0..len).rev() {
            self.push_byte(digits[i]);
        }
    }

    // Formats the fixed-point value in decimal.
    // Note: To match the C++ ffl library behavior, the fractional part is truncated
    // rather than rounded to the requested precision.
    fn write_dec<I, const FRAC: usize>(&mut self, value: Fixed<I, FRAC>, precision: Option<usize>)
    where
        I: Into<i128> + Copy,
    {
        let raw = value.raw_value();
        let raw_i: i128 = raw.into();
        if raw_i < 0 {
            self.push_byte(b'-');
        }

        let absolute_value = raw_i.unsigned_abs();
        let integral_value = if FRAC >= 128 { 0 } else { absolute_value >> FRAC };

        self.write_dec_integer(integral_value);

        let max_digits = precision.unwrap_or(10);
        if max_digits > 0 {
            self.push_byte(b'.');
            let start_pos = self.length;
            let mut last_nonzero = start_pos;

            let requested_stop = start_pos + max_digits;
            let max_stop = self.buffer.len() - 1;
            let stop = if requested_stop < max_stop { requested_stop } else { max_stop };

            let mut remaining_value = absolute_value;
            let frac_mask = (1u128 << FRAC) - 1;

            let mut pos = start_pos;
            loop {
                remaining_value &= frac_mask;
                remaining_value *= 10;
                let digit_val = remaining_value >> FRAC;
                let digit = b'0' + digit_val as u8;
                if digit != b'0' {
                    last_nonzero = pos;
                }
                if pos < self.buffer.len() {
                    self.buffer[pos] = digit;
                }
                pos += 1;
                remaining_value &= frac_mask;
                if (remaining_value == 0 && precision.is_none()) || pos >= stop {
                    break;
                }
            }
            if precision.is_some() {
                self.length = stop;
            } else {
                self.length = last_nonzero + 1;
            }
        }
    }

    fn write_dec_rational<I, const FRAC: usize>(&mut self, value: Fixed<I, FRAC>)
    where
        I: Into<i128> + Copy,
    {
        // The library currently supports fixed-point types up to 64 bits (i8..i64).
        // Ensure that FRAC is at most 64 so that u64 arithmetic is lossless.
        assert!(FRAC <= 64, "Fixed-point types larger than 64 bits are not supported");
        let raw = value.raw_value();
        let raw_i: i128 = raw.into();
        let sign = if raw_i < 0 { b'-' } else { b'+' };
        if raw_i < 0 {
            self.push_byte(b'-');
        }

        // Since I is at most 64 bits, unsigned_abs() fits losslessly in u64.

        let absolute_value = raw_i.unsigned_abs();
        let integral_value = if FRAC >= 128 { 0 } else { absolute_value >> FRAC };

        self.write_dec_integer(integral_value);
        self.push_byte(sign);

        let frac_mask = if FRAC >= 128 { !0u128 } else { (1u128 << FRAC) - 1 };
        self.write_dec_integer(absolute_value & frac_mask);
        self.push_byte(b'/');

        self.write_dec_integer(1u128 << FRAC);
    }

    fn write_hex<I, const FRAC: usize>(&mut self, value: Fixed<I, FRAC>)
    where
        I: Into<i128> + Copy,
    {
        let bits = core::mem::size_of::<I>() * 8;
        let raw_value_mask = if bits == 64 { !0u64 } else { (1u64 << bits) - 1 };
        let raw_i: i128 = value.raw_value().into();
        let raw_value = (raw_i as u64) & raw_value_mask;

        if FRAC == bits {
            self.push_byte(b'0');
        } else {
            let integral_mask = if bits == 64 {
                !0u64
            } else {
                (!((1u64.wrapping_shl(FRAC as u32)).wrapping_sub(1))) & raw_value_mask
            };
            let integral_value = (raw_value & integral_mask) >> FRAC;
            let integral_hex_digits = (bits - FRAC).div_ceil(4);
            let integral_shifted = integral_value << ((16 - integral_hex_digits) * 4);
            self.write_hex_integer(integral_shifted, integral_hex_digits, ZeroMode::NoLeadingZeros);
        }

        self.push_byte(b'.');
        if FRAC == 0 {
            self.push_byte(b'0');
        } else {
            let frac_mask = if FRAC >= 64 { !0u64 } else { (1u64 << FRAC) - 1 };
            let fractional_value = raw_value & frac_mask;
            let fractional_shifted = fractional_value << (64 - FRAC);
            let fractional_hex_digits = FRAC.div_ceil(4);
            self.write_hex_integer(
                fractional_shifted,
                fractional_hex_digits,
                ZeroMode::NoTrailingZeros,
            );
        }
    }

    // Writes `digits` hexadecimal characters of `value` to the buffer, starting
    // from the most significant 4 bits (bits 60-63) and shifting left.
    //
    // Note: The caller must shift `value` left by `(16 - digits) * 4` bits so that the
    // value's most significant hex digit aligns with bits 60-63.
    fn write_hex_integer(&mut self, mut value: u64, digits: usize, zero_mode: ZeroMode) {
        if value == 0 {
            self.push_byte(b'0');
            return;
        }

        let mut had_nonzero = false;
        let mut last_nonzero = self.length;

        for _ in 0..digits {
            let digit = value >> 60;
            if digit == 0 {
                if zero_mode == ZeroMode::NoLeadingZeros && !had_nonzero {
                    value <<= 4;
                    continue;
                }
            } else {
                had_nonzero = true;
                last_nonzero = self.length;
            }
            if digit < 10 {
                self.push_byte(b'0' + digit as u8);
            } else {
                self.push_byte(b'a' + (digit - 10) as u8);
            }
            value <<= 4;
        }

        if zero_mode == ZeroMode::NoTrailingZeros {
            self.length = last_nonzero + 1;
        }
    }
}

impl core::ops::Deref for String {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        core::str::from_utf8(&self.buffer[..self.length]).unwrap()
    }
}

impl core::fmt::Display for String {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self)
    }
}

impl core::fmt::Debug for String {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self)
    }
}
