// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Rust implementation of sizes formatting and parsing, matching zircon/system/ulib/pretty.

use core::fmt::Write;

/// Units for formatting byte sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SizeUnit {
    Auto = 0, // Automatically select an appropriate unit.
    Bytes = b'B',
    KiB = b'K',
    MiB = b'M',
    GiB = b'G',
    TiB = b'T',
    PiB = b'P',
    EiB = b'E',
}

impl TryFrom<u8> for SizeUnit {
    type Error = ();
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value.to_ascii_uppercase() {
            b'B' => Ok(SizeUnit::Bytes),
            b'K' => Ok(SizeUnit::KiB),
            b'M' => Ok(SizeUnit::MiB),
            b'G' => Ok(SizeUnit::GiB),
            b'T' => Ok(SizeUnit::TiB),
            b'P' => Ok(SizeUnit::PiB),
            b'E' => Ok(SizeUnit::EiB),
            0 => Ok(SizeUnit::Auto),
            _ => Err(()),
        }
    }
}

impl TryFrom<char> for SizeUnit {
    type Error = ();
    fn try_from(value: char) -> Result<Self, Self::Error> {
        if value.is_ascii() { Self::try_from(value as u8) } else { Err(()) }
    }
}

impl SizeUnit {
    pub fn to_str(self) -> &'static str {
        match self {
            SizeUnit::Auto => "",
            SizeUnit::Bytes => "B",
            SizeUnit::KiB => "K",
            SizeUnit::MiB => "M",
            SizeUnit::GiB => "G",
            SizeUnit::TiB => "T",
            SizeUnit::PiB => "P",
            SizeUnit::EiB => "E",
        }
    }
}

// A buffer size (including trailing NUL in C, but here just capacity)
// that's large enough for any value formatted by format_size_fixed().
// In C++: sizeof("18446744073709551616B") = 22.
pub const MAX_FORMAT_SIZE_LEN: usize = 22;

struct SliceWriter<'a> {
    slice: &'a mut [u8],
    cursor: usize,
}

impl<'a> SliceWriter<'a> {
    fn new(slice: &'a mut [u8]) -> Self {
        Self { slice, cursor: 0 }
    }

    fn into_str(self) -> &'a str {
        core::str::from_utf8(&self.slice[..self.cursor]).unwrap_or("")
    }
}

impl<'a> core::fmt::Write for SliceWriter<'a> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let bytes = s.as_bytes();
        let keep = core::cmp::min(bytes.len(), self.slice.len() - self.cursor);
        self.slice[self.cursor..self.cursor + keep].copy_from_slice(&bytes[..keep]);
        self.cursor += keep;
        Ok(())
    }
}

// Calculate "n / d" as an integer, rounding any fractional part.
//
// The often-used expression "(n + (d / 2)) / d" can't be used due to
// potential overflow.
fn rounding_divide(n: usize, d: usize) -> usize {
    // If `n` is half way to the next multiple of `d`, we want to round up.
    // Otherwise, we truncate.
    let round_up = (n % d) >= (d / 2);
    n / d + if round_up { 1 } else { 0 }
}

/// Formats |bytes| as a human-readable string like "123.4k".
/// Units are in powers of 1024, so "K" is technically "kiB", etc.
/// Values smaller than "K" have the suffix "B".
///
/// Exact multiples of a unit are displayed without a decimal;
/// e.g., "17K" means the value is exactly 17 * 1024.
///
/// Otherwise, a decimal is present; e.g., "17.0K" means the value
/// is (17 * 1024) +/- epsilon.
///
/// |unit| is the unit to use, as a u8 character (e.g. b'B', b'K', etc.).
/// If zero, picks a natural unit for the size, ensuring at most four whole decimal places.
/// If |unit| is unknown, the output will have a '?' prefix but otherwise
/// behave the same as |unit==0|.
pub fn format_size_fixed_rs(buf: &mut [u8], bytes: usize, mut unit: u8) -> &str {
    if buf.is_empty() {
        return "";
    }

    let mut writer = SliceWriter::new(buf);
    let units = b"BKMGTPE";
    let num_units = units.len();

    let orig_bytes = bytes;
    let mut prepended_question = false;

    let mut bytes = bytes;

    loop {
        let mut ui = 0;
        let mut divisor = 1;

        // If we have a fixed (non-zero) unit, divide until we hit it.
        //
        // Otherwise, divide until we reach a unit that can express the value
        // with 4 or fewer whole digits.
        // - If we can express the value without a fraction (it's a whole
        //   kibi/mebi/gibibyte), use the largest possible unit (e.g., favor
        //   "1M" over "1024K").
        // - Otherwise, favor more whole digits to retain precision (e.g.,
        //   favor "1025K" or "1025.0K" over "1.0M").
        while if unit != 0 {
            ui < num_units && units[ui] != unit
        } else {
            bytes >= 10000 || (bytes != 0 && (bytes & 1023) == 0)
        } {
            ui += 1;
            if ui >= num_units {
                // We probably got an unknown unit. Fall back to a natural unit,
                // but leave a hint that something's wrong.
                if !prepended_question {
                    let _ = writer.write_char('?');
                    prepended_question = true;
                }
                unit = 0;
                bytes = orig_bytes;
                break;
            }
            bytes /= 1024;
            divisor *= 1024;
        }

        if ui < num_units {
            // If the chosen divisor divides the input value evenly, don't print out a
            // fractional part.
            if orig_bytes % divisor == 0 {
                let _ = core::write!(&mut writer, "{}{}", bytes, units[ui] as char);
            } else {
                // We don't have an exact number, so print one unit of precision.
                //
                // Ideally we could just calculate:
                //
                //   sprintf("%0.1f\n", (double)orig_bytes / divisor)
                //
                // but want to avoid floating point. Instead, we separately calculate the
                // two parts using integer arithmetic.
                let mut int_part = orig_bytes / divisor;
                let mut fractional_part = rounding_divide((orig_bytes % divisor) * 10, divisor);
                if fractional_part >= 10 {
                    // the fractional part rounded up to 10: carry it over to the integer part.
                    fractional_part = 0;
                    int_part += 1;
                }
                let _ = core::write!(
                    &mut writer,
                    "{}.{}{}",
                    int_part,
                    fractional_part,
                    units[ui] as char
                );
            }
            break;
        }
    }

    let s = writer.into_str();
    assert!(s.len() <= MAX_FORMAT_SIZE_LEN);
    s
}

/// Calls format_size_fixed_rs() with unit=0, picking a natural unit for the size.
pub fn format_size_rs(buf: &mut [u8], bytes: usize) -> &str {
    format_size_fixed_rs(buf, bytes, 0)
}

/// FormattedBytes is an inline buffer suitable for containing formatted byte sizes.
///
/// Matches C++ `pretty::FormattedBytes`.
#[derive(Clone, Copy)]
pub struct FormattedBytes {
    buf: [u8; MAX_FORMAT_SIZE_LEN],
    len: usize,
}

impl Default for FormattedBytes {
    fn default() -> Self {
        Self::empty()
    }
}

impl FormattedBytes {
    /// Construct an empty formatted byte size buffer.
    pub const fn empty() -> Self {
        Self { buf: [0u8; MAX_FORMAT_SIZE_LEN], len: 0 }
    }

    /// Construct a string representing the given size, choosing an appropriate unit automatically.
    pub fn new(bytes: usize) -> Self {
        let mut res = Self::empty();
        res.set_size(bytes);
        res
    }

    /// Construct a string representing the given size, using the given unit.
    pub fn with_unit(bytes: usize, unit: SizeUnit) -> Self {
        let mut res = Self::empty();
        res.set_size_with_unit(bytes, unit);
        res
    }

    /// Update the string to the given size, choosing an appropriate unit automatically.
    pub fn set_size(&mut self, bytes: usize) -> &mut Self {
        let s = format_size_rs(&mut self.buf, bytes);
        self.len = s.len();
        self
    }

    /// Update the string to the given size, using the given unit.
    pub fn set_size_with_unit(&mut self, bytes: usize, unit: SizeUnit) -> &mut Self {
        let s = format_size_fixed_rs(&mut self.buf, bytes, unit as u8);
        self.len = s.len();
        self
    }

    /// Return the formatted string slice.
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }

    /// Returns the formatted magnitude as a string slice (without unit suffix).
    pub fn magnitude(&self) -> &str {
        let s = self.as_str();
        if s.is_empty() { s } else { &s[..s.len() - 1] }
    }

    /// Returns the associated `SizeUnit`. In the case of an empty string, `SizeUnit::Auto` is returned.
    pub fn unit(&self) -> SizeUnit {
        let s = self.as_str().as_bytes();
        match s.last() {
            Some(&last) => SizeUnit::try_from(last).unwrap_or(SizeUnit::Auto),
            None => SizeUnit::Auto,
        }
    }
}

impl core::ops::Deref for FormattedBytes {
    type Target = str;

    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for FormattedBytes {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl core::fmt::Display for FormattedBytes {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl core::fmt::Debug for FormattedBytes {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self.as_str(), f)
    }
}

impl kprint::backend::AsKPrintStr for FormattedBytes {
    #[inline(always)]
    fn kprint_len(&self) -> core::ffi::c_int {
        self.len as core::ffi::c_int
    }

    #[inline(always)]
    fn kprint_ptr(&self) -> *const core::ffi::c_char {
        self.buf.as_ptr() as *const core::ffi::c_char
    }
}

impl kprint::backend::AsKPrintStr for &FormattedBytes {
    #[inline(always)]
    fn kprint_len(&self) -> core::ffi::c_int {
        self.len as core::ffi::c_int
    }

    #[inline(always)]
    fn kprint_ptr(&self) -> *const core::ffi::c_char {
        self.buf.as_ptr() as *const core::ffi::c_char
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EncodedSize<'a> {
    // All numbers before the first '.'.
    integral: &'a str,

    // All numbers after the first '.'.
    fractional: Option<&'a str>,

    unit: SizeUnit,

    scale: u64,
}

fn process_formatted_string(mut formatted: &str) -> Option<EncodedSize<'_>> {
    if formatted.is_empty() {
        return None;
    }

    let mut unit = SizeUnit::Bytes;
    let mut scale = 1u64;

    if let Some(last_char) = formatted.chars().next_back() {
        if !last_char.is_ascii_digit() {
            unit = SizeUnit::try_from(last_char).ok()?;
            formatted = &formatted[..formatted.len() - last_char.len_utf8()];

            // Look for the unit.
            match unit {
                SizeUnit::Bytes => scale = 1,
                SizeUnit::KiB => scale = 1 << 10,
                SizeUnit::MiB => scale = 1 << 20,
                SizeUnit::GiB => scale = 1 << 30,
                SizeUnit::TiB => scale = 1 << 40,
                SizeUnit::PiB => scale = 1 << 50,
                SizeUnit::EiB => scale = 1 << 60,
                _ => return None,
            }
        }
    }

    let split_at = formatted.find('.');
    let (integral, fractional) = if let Some(split_at) = split_at {
        let integral = &formatted[..split_at];
        let fractional = &formatted[split_at + 1..];
        // "A.[Unit]" with A being digit is still invalid.
        if fractional.is_empty() {
            return None;
        }
        (integral, Some(fractional))
    } else {
        (formatted, None)
    };

    if integral.is_empty() {
        return None;
    }

    Some(EncodedSize { integral, fractional, unit, scale })
}

/// Returns the number of bytes represented by a human readable string
/// like "123.4k", 123.4 * 1024 bytes encoded in |formatted_bytes|.
///
/// If |formatted_bytes| is not correctly formatted then |None| is returned.
///
/// This is a reverse function of |format_size| input |bytes|. Except that it considers
/// absence of unit (e.g. "123") to be in bytes(implicit B).
pub fn parse_size_bytes(formatted_bytes: &str) -> Option<u64> {
    let encoded_size = process_formatted_string(formatted_bytes)?;

    let mut integral: u64 = 0;
    let mut base_10: u64 = 1;

    for c in encoded_size.integral.chars().rev() {
        if !c.is_ascii_digit() {
            return None;
        }
        let val = c.to_digit(10)? as u64;
        let scaled_val = val.checked_mul(base_10)?.checked_mul(encoded_size.scale)?;

        integral = integral.checked_add(scaled_val)?;
        base_10 = base_10.checked_mul(10)?;
    }

    let mut fractional: u64 = 0;
    if let Some(frac_str) = encoded_size.fractional {
        let mut frac_base_10: u64 = 1;
        let mut carry: u64 = 0;

        // This loop provides software division, because for the larger
        // units its is quite possible to overflow when doing the scaling
        // of the mantissa.
        // If one were to use the naive approach:
        //  * let m be the mantissa as an integer.
        //  * k the length of the mantissa.
        //  * u the scaling factor of the provided unit.
        //
        // The number of bytes encoded in the mantissa, can be calculated as:
        //     |m * u / 10^(k)|
        // The problem arises when |m| * |u| exceeds the capacity of 64 bits.
        for c in frac_str.chars() {
            if !c.is_ascii_digit() {
                return None;
            }
            let val = c.to_digit(10)? as u64;
            frac_base_10 = frac_base_10.checked_mul(10)?;
            let scaled_value = val.checked_mul(encoded_size.scale)?;

            // Calculate how many bytes does this digit of the mantissa contributes.
            let contrib = scaled_value / frac_base_10;
            fractional = fractional.checked_add(contrib)?;

            // Bring the carry from 10^-(i - 1) bytes to 10^-(i) bytes.
            carry = carry.checked_mul(10)?.checked_add(scaled_value % frac_base_10)?;

            // Try to consume any part of the accumulated carry.
            let consumed_carry = carry / frac_base_10;
            fractional = fractional.checked_add(consumed_carry)?;

            // Adjust the units back again.
            carry %= frac_base_10;
        }

        // At this point there should be no carry left, unless we were given
        // a value that is not byte aligned (Y.X bytes) where X is non zero,
        // after applying the proper scaling.
        if carry != 0 {
            return None;
        }
    }

    integral.checked_add(fractional)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KILO: usize = 1024;
    const MEGA: usize = KILO * 1024;
    const GIGA: usize = MEGA * 1024;
    const TERA: usize = GIGA * 1024;
    const PETA: usize = TERA * 1024;
    const EXA: usize = PETA * 1024;

    struct FormatSizeTestCase {
        input: usize,
        unit: u8,
        expected_output: &'static str,
    }

    const FORMAT_SIZE_TEST_CASES: &[FormatSizeTestCase] = &[
        // Whole multiples don't print decimals,
        // and always round up to their largest unit.
        FormatSizeTestCase { input: 0, unit: 0, expected_output: "0B" },
        FormatSizeTestCase { input: 1, unit: 0, expected_output: "1B" },
        // Favor the largest unit when it loses no precision
        // (e.g., "1K" not "1024B").
        // Larger values may still use a smaller unit
        // (e.g., "1K" + 1 == "1025B") to preserve precision.
        FormatSizeTestCase { input: KILO - 1, unit: 0, expected_output: "1023B" },
        FormatSizeTestCase { input: KILO, unit: 0, expected_output: "1K" },
        FormatSizeTestCase { input: KILO + 1, unit: 0, expected_output: "1025B" },
        FormatSizeTestCase { input: KILO * 9, unit: 0, expected_output: "9K" },
        FormatSizeTestCase { input: KILO * 9 + 1, unit: 0, expected_output: "9217B" },
        FormatSizeTestCase { input: KILO * 10, unit: 0, expected_output: "10K" },
        // Same demonstration for the next unit.
        FormatSizeTestCase { input: MEGA - KILO, unit: 0, expected_output: "1023K" },
        FormatSizeTestCase { input: MEGA, unit: 0, expected_output: "1M" },
        FormatSizeTestCase { input: MEGA + KILO, unit: 0, expected_output: "1025K" },
        FormatSizeTestCase { input: MEGA * 9, unit: 0, expected_output: "9M" },
        FormatSizeTestCase { input: MEGA * 9 + KILO, unit: 0, expected_output: "9217K" },
        FormatSizeTestCase { input: MEGA * 10, unit: 0, expected_output: "10M" },
        // Sanity checks for remaining units.
        FormatSizeTestCase { input: MEGA, unit: 0, expected_output: "1M" },
        FormatSizeTestCase { input: GIGA, unit: 0, expected_output: "1G" },
        FormatSizeTestCase { input: TERA, unit: 0, expected_output: "1T" },
        FormatSizeTestCase { input: PETA, unit: 0, expected_output: "1P" },
        FormatSizeTestCase { input: EXA, unit: 0, expected_output: "1E" },
        // Non-whole multiples print decimals, and favor more whole digits
        // (e.g., "1024.0K" not "1.0M") to retain precision.
        FormatSizeTestCase { input: MEGA - 1, unit: 0, expected_output: "1024.0K" },
        FormatSizeTestCase { input: MEGA + MEGA / 3, unit: 0, expected_output: "1365.3K" },
        FormatSizeTestCase { input: GIGA - 1, unit: 0, expected_output: "1024.0M" },
        FormatSizeTestCase { input: TERA - 1, unit: 0, expected_output: "1024.0G" },
        FormatSizeTestCase { input: PETA - 1, unit: 0, expected_output: "1024.0T" },
        FormatSizeTestCase { input: EXA - 1, unit: 0, expected_output: "1024.0P" },
        FormatSizeTestCase { input: usize::MAX, unit: 0, expected_output: "16.0E" },
        // Never show more than four whole digits,
        // to make the values easier to eyeball.
        FormatSizeTestCase { input: 9999, unit: 0, expected_output: "9999B" },
        FormatSizeTestCase { input: 10000, unit: 0, expected_output: "9.8K" },
        FormatSizeTestCase { input: KILO * 9999, unit: 0, expected_output: "9999K" },
        FormatSizeTestCase { input: KILO * 9999 + 1, unit: 0, expected_output: "9999.0K" },
        FormatSizeTestCase { input: KILO * 10000, unit: 0, expected_output: "9.8M" },
        // Ensure values are correctly rounded.
        FormatSizeTestCase { input: 10700, unit: 0, expected_output: "10.4K" },
        FormatSizeTestCase { input: 10701, unit: 0, expected_output: "10.5K" },
        FormatSizeTestCase { input: 69887590, unit: 0, expected_output: "66.6M" },
        FormatSizeTestCase { input: 69887591, unit: 0, expected_output: "66.7M" },
        FormatSizeTestCase { input: 18389097998479209267, unit: 0, expected_output: "15.9E" },
        FormatSizeTestCase { input: 18389097998479209268, unit: 0, expected_output: "16.0E" },
        // When fixed, we can see a lot more digits.
        FormatSizeTestCase {
            input: usize::MAX,
            unit: b'B',
            expected_output: "18446744073709551615B",
        },
        FormatSizeTestCase {
            input: usize::MAX,
            unit: b'K',
            expected_output: "18014398509481984.0K",
        },
        FormatSizeTestCase { input: usize::MAX, unit: b'M', expected_output: "17592186044416.0M" },
        FormatSizeTestCase { input: usize::MAX, unit: b'G', expected_output: "17179869184.0G" },
        FormatSizeTestCase { input: usize::MAX, unit: b'T', expected_output: "16777216.0T" },
        FormatSizeTestCase { input: usize::MAX, unit: b'P', expected_output: "16384.0P" },
        FormatSizeTestCase { input: usize::MAX, unit: b'E', expected_output: "16.0E" },
        // Smaller than natural fixed unit.
        FormatSizeTestCase { input: GIGA, unit: b'K', expected_output: "1048576K" },
        // Larger than natural fixed unit.
        FormatSizeTestCase { input: MEGA / 10, unit: b'M', expected_output: "0.1M" },
        // Unknown units fall back to natural, but add a '?' prefix.
        FormatSizeTestCase { input: GIGA, unit: b'q', expected_output: "?1G" },
        FormatSizeTestCase { input: KILO, unit: b'q', expected_output: "?1K" },
        FormatSizeTestCase { input: GIGA + 1, unit: b'#', expected_output: "?1.0G" },
        FormatSizeTestCase { input: KILO + 1, unit: b'#', expected_output: "?1025B" },
    ];

    #[test]
    fn test_format_size_fixed() {
        let mut buf = [0u8; MAX_FORMAT_SIZE_LEN];
        for (i, tc) in FORMAT_SIZE_TEST_CASES.iter().enumerate() {
            buf.fill(0);
            let res = format_size_fixed_rs(&mut buf, tc.input, tc.unit);
            assert_eq!(
                res, tc.expected_output,
                "case {}, input={}, unit={}",
                i, tc.input, tc.unit as char
            );
        }
    }

    #[test]
    fn test_format_size_short_buf_truncates() {
        let input = 1023 * KILO + 1;
        let expected_output = "1023.0K";

        let mut buf = [0x55u8; MAX_FORMAT_SIZE_LEN * 2];
        for str_size in 0..=expected_output.len() {
            buf.fill(0x55);
            let res = format_size_rs(&mut buf[..str_size], input);
            assert_eq!(res, &expected_output[..str_size]);
            assert_eq!(buf[str_size], 0x55);
        }
    }

    #[test]
    fn test_format_size_bad_unit_short_buf_truncates() {
        let mut buf = [0x55u8; MAX_FORMAT_SIZE_LEN];

        // Size zero should not touch the buffer.
        buf.fill(0x55);
        let res = format_size_fixed_rs(&mut buf[..0], GIGA, b'q');
        assert_eq!(res, "");
        assert_eq!(buf[0], 0x55);

        // Size 1 should just be the warning '?'.
        buf.fill(0x55);
        let res = format_size_fixed_rs(&mut buf[..1], GIGA, b'q');
        assert_eq!(res, "?");
        assert_eq!(buf[1], 0x55);

        // Then just the number without units.
        buf.fill(0x55);
        let res = format_size_fixed_rs(&mut buf[..2], GIGA, b'q');
        assert_eq!(res, "?1");
        assert_eq!(buf[2], 0x55);

        // Then the whole thing.
        buf.fill(0x55);
        let res = format_size_fixed_rs(&mut buf[..3], GIGA, b'q');
        assert_eq!(res, "?1G");
        assert_eq!(buf[3], 0x55);
    }

    #[test]
    fn test_cpp_to_string() {
        assert_eq!(SizeUnit::Auto.to_str(), "");
        assert_eq!(SizeUnit::Bytes.to_str(), "B");
        assert_eq!(SizeUnit::KiB.to_str(), "K");
        assert_eq!(SizeUnit::MiB.to_str(), "M");
        assert_eq!(SizeUnit::GiB.to_str(), "G");
        assert_eq!(SizeUnit::TiB.to_str(), "T");
        assert_eq!(SizeUnit::PiB.to_str(), "P");
        assert_eq!(SizeUnit::EiB.to_str(), "E");
    }

    struct ParseTestCase {
        expected_bytes: u64,
        input: &'static str,
    }

    const PARSE_TEST_CASES: &[ParseTestCase] = &[
        // Integral
        ParseTestCase { expected_bytes: 1234, input: "1234" },
        ParseTestCase { expected_bytes: 1234, input: "1234b" },
        ParseTestCase { expected_bytes: 1234, input: "1234B" },
        ParseTestCase { expected_bytes: 1234 * 1024, input: "1234k" },
        ParseTestCase { expected_bytes: 1234 * 1024, input: "1234K" },
        ParseTestCase { expected_bytes: 1234 * 1024 * 1024, input: "1234m" },
        ParseTestCase { expected_bytes: 1234 * 1024 * 1024, input: "1234M" },
        ParseTestCase { expected_bytes: 1234 * 1024 * 1024 * 1024, input: "1234g" },
        ParseTestCase { expected_bytes: 1234 * 1024 * 1024 * 1024, input: "1234G" },
        ParseTestCase { expected_bytes: 1234 * 1024 * 1024 * 1024 * 1024, input: "1234t" },
        ParseTestCase { expected_bytes: 1234 * 1024 * 1024 * 1024 * 1024, input: "1234T" },
        ParseTestCase { expected_bytes: 5 * 1024 * 1024 * 1024 * 1024 * 1024, input: "5p" },
        ParseTestCase { expected_bytes: 5 * 1024 * 1024 * 1024 * 1024 * 1024, input: "5P" },
        ParseTestCase { expected_bytes: 2 * 1024 * 1024 * 1024 * 1024 * 1024 * 1024, input: "2e" },
        ParseTestCase { expected_bytes: 2 * 1024 * 1024 * 1024 * 1024 * 1024 * 1024, input: "2E" },
        // Fractional
        ParseTestCase { expected_bytes: 10700, input: "10.4492187500k" },
        ParseTestCase { expected_bytes: 10700, input: "10.4492187500K" },
        ParseTestCase { expected_bytes: 10700 * 1024, input: "10.4492187500m" },
        ParseTestCase { expected_bytes: 10700 * 1024, input: "10.4492187500M" },
        ParseTestCase { expected_bytes: 10700 * 1024 * 1024, input: "10.4492187500g" },
        ParseTestCase { expected_bytes: 10700 * 1024 * 1024, input: "10.4492187500G" },
        ParseTestCase { expected_bytes: 10700 * 1024 * 1024 * 1024, input: "10.4492187500t" },
        ParseTestCase { expected_bytes: 10700 * 1024 * 1024 * 1024, input: "10.4492187500T" },
        ParseTestCase {
            expected_bytes: 10700 * 1024 * 1024 * 1024 * 1024,
            input: "10.4492187500p",
        },
        ParseTestCase {
            expected_bytes: 10700 * 1024 * 1024 * 1024 * 1024,
            input: "10.4492187500P",
        },
        ParseTestCase { expected_bytes: 1441151880758558720, input: "1.25e" },
        ParseTestCase { expected_bytes: 1441151880758558720, input: "1.25E" },
    ];

    #[test]
    fn test_parse_size_bytes() {
        for tc in PARSE_TEST_CASES {
            let res = parse_size_bytes(tc.input);
            assert_eq!(res, Some(tc.expected_bytes), "input: {}", tc.input);
        }
    }

    const INVALID_INPUTS: &[&str] = &["", "1..1", "1w", "b", "AM", "1.AM", "A.1M"];

    #[test]
    fn test_parse_size_bytes_invalid() {
        for input in INVALID_INPUTS {
            let res = parse_size_bytes(input);
            assert_eq!(res, None, "input: {}", input);
        }
    }

    #[test]
    fn test_formatted_bytes() {
        let empty = FormattedBytes::empty();
        assert_eq!(empty.as_str(), "");
        assert_eq!(empty.magnitude(), "");
        assert_eq!(empty.unit(), SizeUnit::Auto);

        let fb = FormattedBytes::new(1024 * 1024);
        assert_eq!(fb.as_str(), "1M");
        assert_eq!(fb.magnitude(), "1");
        assert_eq!(fb.unit(), SizeUnit::MiB);

        let fb_unit = FormattedBytes::with_unit(1024 * 1024, SizeUnit::KiB);
        assert_eq!(fb_unit.as_str(), "1024K");
        assert_eq!(fb_unit.magnitude(), "1024");
        assert_eq!(fb_unit.unit(), SizeUnit::KiB);
    }
}
