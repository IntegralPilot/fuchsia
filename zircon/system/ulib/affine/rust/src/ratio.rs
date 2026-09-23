// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use zr::static_assert;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exact {
    No,
    Yes,
}

/// Rounding Behaviors used when scaling.
///
/// | val  | N   | D   | Down | Up  | TowardsZero | AwayFromZero |
/// | :--- | :-- | :-- | :--- | :-- | :---------- | :----------- |
/// | 7    | 1   | 2   | 3    | 4   | 3           | 4            |
/// | -7   | 1   | 2   | -4   | -3  | -3          | -4           |
pub struct Round;
impl Round {
    pub const DOWN: u8 = 0;
    pub const UP: u8 = 1;
    pub const TOWARDS_ZERO: u8 = 2;
    pub const AWAY_FROM_ZERO: u8 = 3;
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ratio {
    numerator: u32,
    denominator: u32,
}

static_assert!(core::mem::size_of::<Ratio>() == 8);
static_assert!(core::mem::align_of::<Ratio>() == 4);

impl Default for Ratio {
    fn default() -> Self {
        Ratio { numerator: 1, denominator: 1 }
    }
}

impl Ratio {
    pub const OVERFLOW: i64 = i64::MAX;
    pub const UNDERFLOW: i64 = i64::MIN;

    pub fn new(numerator: u32, denominator: u32) -> Self {
        debug_assert!(denominator != 0);
        Ratio { numerator, denominator }
    }

    pub fn numerator(&self) -> u32 {
        self.numerator
    }

    pub fn denominator(&self) -> u32 {
        self.denominator
    }

    pub fn invertible(&self) -> bool {
        self.numerator != 0
    }

    pub fn inverse(&self) -> Self {
        debug_assert!(self.invertible());
        Ratio { numerator: self.denominator, denominator: self.numerator }
    }

    /// Reduces the ratio of numerator/denominator in-place (32-bit).
    pub fn reduce_u32(numerator: &mut u32, denominator: &mut u32) {
        assert!(*denominator != 0);
        if *numerator == 0 {
            *denominator = 1;
            return;
        }
        let gcd = binary_gcd(*numerator as u64, *denominator as u64) as u32;
        *numerator /= gcd;
        *denominator /= gcd;
    }

    /// Reduces the ratio of numerator/denominator in-place (64-bit).
    pub fn reduce_u64(numerator: &mut u64, denominator: &mut u64) {
        assert!(*denominator != 0);
        if *numerator == 0 {
            *denominator = 1;
            return;
        }
        let gcd = binary_gcd(*numerator, *denominator);
        *numerator /= gcd;
        *denominator /= gcd;
    }

    /// Reduces the ratio instance in-place.
    pub fn reduce(&mut self) {
        Self::reduce_u32(&mut self.numerator, &mut self.denominator);
    }

    /// Produces the product of two ratios.
    ///
    /// If `exact` is `Exact::Yes`, this panics on loss of precision.
    /// If `exact` is `Exact::No`, it attempts to find the best 32-bit approximation.
    pub fn product_raw(
        a_numerator: u32,
        a_denominator: u32,
        b_numerator: u32,
        b_denominator: u32,
        exact: Exact,
    ) -> (u32, u32) {
        let mut numerator = a_numerator as u64 * b_numerator as u64;
        let mut denominator = a_denominator as u64 * b_denominator as u64;

        Self::reduce_u64(&mut numerator, &mut denominator);

        if numerator > u32::MAX as u64 || denominator > u32::MAX as u64 {
            assert!(exact == Exact::No, "Precision loss in exact Ratio::product");

            // Try to find the best approximation of the ratio that we can. Our
            // approach is as follows. Figure out the number of bits to the right
            // we need to shift the numerator and denominator, rounding up or down
            // in the process, such that the result can be reduced to fit into 32
            // bits.
            //
            // This approach tends to beat out a just-shift-until-it-fits approach,
            // as well as an always-shift-then-reduce approach, but _none_ of these
            // approaches always finds the best solution.
            //
            // TODO(johngro): figure out if it is reasonable to actually compute
            // the best solution. Alternatively, consider implementing a "just
            // shift until it fits" solution if the approximate results are good
            // enough.
            for i in 1..=32 {
                // Produce a version of the numerator and denominator which have
                // each been divided by 2^i, rounding up/down as appropriate
                // (instead of truncating).
                let rounded_numerator = (numerator + (1u64 << (i - 1))) >> i;
                let rounded_denominator = (denominator + (1u64 << (i - 1))) >> i;

                if rounded_denominator == 0 {
                    // Product is larger than we can represent. Return the largest value we
                    // can represent.
                    return (u32::MAX, 1);
                }

                if rounded_numerator == 0 {
                    // Product is smaller than we can represent. Return 0.
                    return (0, 1);
                }

                let mut rn = rounded_numerator;
                let mut rd = rounded_denominator;
                Self::reduce_u64(&mut rn, &mut rd);
                if rn <= u32::MAX as u64 && rd <= u32::MAX as u64 {
                    return (rn as u32, rd as u32);
                }
            }
            // Fallback (should be unreachable)
            return (numerator as u32, denominator as u32);
        }

        (numerator as u32, denominator as u32)
    }

    pub fn product(a: Ratio, b: Ratio, exact: Exact) -> Ratio {
        let (n, d) =
            Self::product_raw(a.numerator, a.denominator, b.numerator, b.denominator, exact);
        Ratio { numerator: n, denominator: d }
    }

    /// Scales an `i64` value by the ratio of numerator/denominator.
    ///
    /// Returns a saturated value (`OVERFLOW` or `UNDERFLOW`) on overflow/underflow.
    /// The rounding behavior is determined by the `ROUND` const generic.
    pub fn scale_with_round<const ROUND: u8>(value: i64, numerator: u32, denominator: u32) -> i64 {
        assert!(denominator != 0);

        if value >= 0 {
            const LIMIT: u64 = i64::MAX as u64; // 0x7FFFFFFFFFFFFFFF
            let value = value as u64;
            let scaled = match ROUND {
                Round::UP | Round::AWAY_FROM_ZERO => {
                    scale_unsigned::<true, LIMIT>(value, numerator, denominator)
                }
                _ => scale_unsigned::<false, LIMIT>(value, numerator, denominator),
            };
            scaled as i64
        } else {
            // LIMIT == 0x8000000000000000
            //
            // Note:  We are attempting to pass the unsigned distance from zero into
            // our ScaleUInt64 function.  In the case of negative numbers, we pass
            // the twos compliment into the scale function, and then flip the sign
            // again on the way out.
            //
            // We are taking the advantage of the fact that the twos compliment of
            // MIN is itself for any signed integer type, and that casting this
            // value to an unsigned integer of the same size properly produces the
            // original value's distance from zero.  Clamping the limit to the
            // distance of MIN from zero means that saturated results will likewise
            // get properly flipped back to MIN during the return.
            //
            const LIMIT: u64 = 0x8000000000000000; // i64::MIN.unsigned_abs()
            let value = value.unsigned_abs();
            let scaled = match ROUND {
                Round::DOWN | Round::AWAY_FROM_ZERO => {
                    scale_unsigned::<true, LIMIT>(value, numerator, denominator)
                }
                _ => scale_unsigned::<false, LIMIT>(value, numerator, denominator),
            };
            if scaled == LIMIT { i64::MIN } else { -(scaled as i64) }
        }
    }

    /// Scales an `i64` value by this ratio.
    ///
    /// Returns a saturated value (`OVERFLOW` or `UNDERFLOW`) on overflow/underflow.
    /// The rounding behavior is determined by the `ROUND` const generic.
    pub fn scale<const ROUND: u8>(&self, value: i64) -> i64 {
        Self::scale_with_round::<ROUND>(value, self.numerator, self.denominator)
    }
}

// Calculates the greatest common denominator (factor) of two values.
fn binary_gcd(mut a: u64, mut b: u64) -> u64 {
    debug_assert!(a != 0 && b != 0);

    // Remove and count the common factors of 2.
    let mut twos = 0;
    while ((a | b) & 1) == 0 {
        a >>= 1;
        b >>= 1;
        twos += 1;
    }

    // Get rid of the non-common factors of 2 in a. a is non-zero, so this
    // terminates.
    while (a & 1) == 0 {
        a >>= 1;
    }

    loop {
        // Get rid of the non-common factors of 2 in b. b is non-zero, so this
        // terminates.
        while (b & 1) == 0 {
            b >>= 1;
        }

        // Apply the Euclid subtraction method.
        if a > b {
            core::mem::swap(&mut a, &mut b);
        }

        b -= a;
        if b == 0 {
            break;
        }
    }

    // Multiply in the common factors of two.
    a << twos
}

// Scales a u64 value by the ratio of two u32 values. If ROUND_UP is true, the
// result is rounded up rather than down. Saturates at LIMIT on overflow.
fn scale_unsigned<const ROUND_UP: bool, const LIMIT: u64>(
    value: u64,
    numerator: u32,
    denominator: u32,
) -> u64 {
    let numerator = numerator as u64;
    let denominator = denominator as u64;
    const LOW_32_BITS: u64 = u32::MAX as u64;

    // high and low are the product of the numerator and the high and low halves
    // (respectively) of value, with the high end of low moved into the low end
    // of high.
    let low_product = numerator * (value & LOW_32_BITS);
    let high = numerator * (value >> 32) + (low_product >> 32);
    let mut low = low_product & LOW_32_BITS;

    // Ignoring overflow and remainder, the result we want is:
    // ((high << 32) + low) / denominator.

    // Compute the divmod of high/D
    let high_q = high / denominator;
    let high_r = high % denominator;

    // If high_q is larger than the overflow limit, then we can just get out now.
    // The overflow limit will be different depending on whether we are scaling
    // a non-negative number (0x7FFFFFFF) or a negative number (0x80000000)
    if high_q > LIMIT >> 32 {
        return LIMIT;
    }

    // The remainder of high/D are the high bits of low. Or them in, and do the
    // divmod for the low portion
    low |= high_r << 32;

    let low_q = low / denominator;
    let low_r = low % denominator;
    let result = (high_q << 32) | low_q;
    if result >= LIMIT {
        return LIMIT;
    }

    // `result` is strictly less than LIMIT, so `result + 1` neither overflows
    // nor exceeds LIMIT; if it is exactly LIMIT, that is the saturated value we
    // would have returned anyway.
    if ROUND_UP && low_r != 0 {
        return result + 1;
    }

    result
}

// Operators

impl core::ops::Mul for Ratio {
    type Output = Self;
    fn mul(self, rhs: Self) -> Self::Output {
        Ratio::product(self, rhs, Exact::Yes)
    }
}

impl core::ops::Div for Ratio {
    type Output = Self;
    fn div(self, rhs: Self) -> Self::Output {
        self * rhs.inverse()
    }
}

impl core::ops::Mul<i64> for Ratio {
    type Output = i64;
    fn mul(self, rhs: i64) -> Self::Output {
        self.scale::<{ Round::DOWN }>(rhs)
    }
}

impl core::ops::Mul<Ratio> for i64 {
    type Output = i64;
    fn mul(self, rhs: Ratio) -> Self::Output {
        rhs.scale::<{ Round::DOWN }>(self)
    }
}

impl core::ops::Div<Ratio> for i64 {
    type Output = i64;
    fn div(self, rhs: Ratio) -> Self::Output {
        rhs.inverse().scale::<{ Round::DOWN }>(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_construction() {
        let valid_vectors = [(0, 1), (1, 1), (23, 41)];
        for &(n, d) in &valid_vectors {
            let r = Ratio::new(n, d);
            assert_eq!(r.numerator(), n);
            assert_eq!(r.denominator(), d);
        }

        // Ratio::default() produces 1/1
        let r = Ratio::default();
        assert_eq!(r.numerator(), 1);
        assert_eq!(r.denominator(), 1);

        // Reduction is NOT automatically performed
        let r = Ratio::new(9, 21);
        assert_eq!(r.numerator(), 9);
        assert_eq!(r.denominator(), 21);
    }

    #[test]
    fn test_reduction_32() {
        let mut vectors = [
            (1, 1, 1, 1),
            (10, 10, 1, 1),
            (10, 2, 5, 1),
            (0, 1, 0, 1),
            (0, 500, 0, 1),
            (48000, 44100, 160, 147),
            (44100, 48000, 147, 160),
            (1000007, 1000000, 1000007, 1000000),
        ];

        for v in &mut vectors {
            let mut n = v.0;
            let mut d = v.1;
            Ratio::reduce_u32(&mut n, &mut d);
            assert_eq!((n, d), (v.2, v.3));

            let mut r = Ratio::new(v.0, v.1);
            r.reduce();
            assert_eq!((r.numerator(), r.denominator()), (v.2, v.3));
        }
    }

    #[test]
    fn test_reduction_64() {
        let mut vectors = [
            (1, 1, 1, 1),
            (10, 10, 1, 1),
            (10, 2, 5, 1),
            (0, 1, 0, 1),
            (0, 500, 0, 1),
            (48000, 44100, 160, 147),
            (44100, 48000, 147, 160),
            (1000007, 1000000, 1000007, 1000000),
            (48000336000, 44100000000, 1000007, 918750),
        ];

        for v in &mut vectors {
            let mut n = v.0;
            let mut d = v.1;
            Ratio::reduce_u64(&mut n, &mut d);
            assert_eq!((n, d), (v.2, v.3));
        }
    }

    #[test]
    fn test_product() {
        struct TestVector {
            a_n: u32,
            a_d: u32,
            b_n: u32,
            b_d: u32,
            expected_n: u32,
            expected_d: u32,
            exact: Exact,
        }

        let test_vectors = [
            TestVector {
                a_n: 1,
                a_d: 1,
                b_n: 1,
                b_d: 1,
                expected_n: 1,
                expected_d: 1,
                exact: Exact::Yes,
            },
            TestVector {
                a_n: 0,
                a_d: 1,
                b_n: 1,
                b_d: 1,
                expected_n: 0,
                expected_d: 1,
                exact: Exact::Yes,
            },
            TestVector {
                a_n: 0,
                a_d: 500,
                b_n: 1,
                b_d: 1,
                expected_n: 0,
                expected_d: 1,
                exact: Exact::Yes,
            },
            TestVector {
                a_n: 3,
                a_d: 4,
                b_n: 5,
                b_d: 9,
                expected_n: 5,
                expected_d: 12,
                exact: Exact::Yes,
            },
            TestVector {
                a_n: 48000,
                a_d: 44100,
                b_n: 1000007,
                b_d: 1000000,
                expected_n: 1000007,
                expected_d: 918750,
                exact: Exact::Yes,
            },
            TestVector {
                a_n: 3465653567,
                a_d: 2327655023,
                b_n: 1291540343,
                b_d: 3698423317,
                expected_n: 317609835,
                expected_d: 610852072,
                exact: Exact::No,
            },
            TestVector {
                a_n: 0xFFFFFFFF,
                a_d: 1,
                b_n: 0xFFFFFFFF,
                b_d: 1,
                expected_n: 0xFFFFFFFF,
                expected_d: 1,
                exact: Exact::No,
            },
            TestVector {
                a_n: 1,
                a_d: 0xFFFFFFFF,
                b_n: 1,
                b_d: 0xFFFFFFFF,
                expected_n: 0,
                expected_d: 1,
                exact: Exact::No,
            },
        ];

        for v in &test_vectors {
            let a = Ratio::new(v.a_n, v.a_d);
            let b = Ratio::new(v.b_n, v.b_d);

            let res = Ratio::product(a, b, v.exact);
            assert_eq!(
                (res.numerator(), res.denominator()),
                (v.expected_n, v.expected_d),
                "Expected {}/{} * {}/{} to produce {}/{}; got {}/{} instead (static)",
                v.a_n,
                v.a_d,
                v.b_n,
                v.b_d,
                v.expected_n,
                v.expected_d,
                res.numerator(),
                res.denominator()
            );

            let res = Ratio::product(b, a, v.exact);
            assert_eq!(
                (res.numerator(), res.denominator()),
                (v.expected_n, v.expected_d),
                "Expected {}/{} * {}/{} to produce {}/{}; got {}/{} instead (commutative static)",
                v.b_n,
                v.b_d,
                v.a_n,
                v.a_d,
                v.expected_n,
                v.expected_d,
                res.numerator(),
                res.denominator()
            );

            if v.exact == Exact::Yes {
                let res = a * b;
                assert_eq!((res.numerator(), res.denominator()), (v.expected_n, v.expected_d));

                let res = b * a;
                assert_eq!((res.numerator(), res.denominator()), (v.expected_n, v.expected_d));

                if b.invertible() {
                    let res = a / b.inverse();
                    assert_eq!((res.numerator(), res.denominator()), (v.expected_n, v.expected_d));
                }

                if a.invertible() {
                    let res = b / a.inverse();
                    assert_eq!((res.numerator(), res.denominator()), (v.expected_n, v.expected_d));
                }
            }
        }
    }

    #[test]
    fn test_product_raw() {
        struct TestVector {
            a_n: u32,
            a_d: u32,
            b_n: u32,
            b_d: u32,
            expected_n: u32,
            expected_d: u32,
            exact: Exact,
        }

        let test_vectors = [
            TestVector {
                a_n: 1,
                a_d: 1,
                b_n: 1,
                b_d: 1,
                expected_n: 1,
                expected_d: 1,
                exact: Exact::Yes,
            },
            TestVector {
                a_n: 0,
                a_d: 1,
                b_n: 1,
                b_d: 1,
                expected_n: 0,
                expected_d: 1,
                exact: Exact::Yes,
            },
            TestVector {
                a_n: 0,
                a_d: 500,
                b_n: 1,
                b_d: 1,
                expected_n: 0,
                expected_d: 1,
                exact: Exact::Yes,
            },
            TestVector {
                a_n: 3,
                a_d: 4,
                b_n: 5,
                b_d: 9,
                expected_n: 5,
                expected_d: 12,
                exact: Exact::Yes,
            },
            TestVector {
                a_n: 48000,
                a_d: 44100,
                b_n: 1000007,
                b_d: 1000000,
                expected_n: 1000007,
                expected_d: 918750,
                exact: Exact::Yes,
            },
            TestVector {
                a_n: 3465653567,
                a_d: 2327655023,
                b_n: 1291540343,
                b_d: 3698423317,
                expected_n: 317609835,
                expected_d: 610852072,
                exact: Exact::No,
            },
            TestVector {
                a_n: 0xFFFFFFFF,
                a_d: 1,
                b_n: 0xFFFFFFFF,
                b_d: 1,
                expected_n: 0xFFFFFFFF,
                expected_d: 1,
                exact: Exact::No,
            },
            TestVector {
                a_n: 1,
                a_d: 0xFFFFFFFF,
                b_n: 1,
                b_d: 0xFFFFFFFF,
                expected_n: 0,
                expected_d: 1,
                exact: Exact::No,
            },
        ];

        for v in &test_vectors {
            let res = Ratio::product_raw(v.a_n, v.a_d, v.b_n, v.b_d, v.exact);
            assert_eq!(
                res,
                (v.expected_n, v.expected_d),
                "Expected {}/{} * {}/{} to produce {}/{}; got {}/{} instead",
                v.a_n,
                v.a_d,
                v.b_n,
                v.b_d,
                v.expected_n,
                v.expected_d,
                res.0,
                res.1
            );
        }
    }

    fn test_scale_helper<const ROUND: u8>() {
        struct TestVector {
            val: i64,
            n: u32,
            d: u32,
            expected: i64,
            fractional_result: bool,
        }

        let test_vectors = [
            TestVector { val: 0, n: 0, d: 1, expected: 0, fractional_result: false },
            TestVector { val: 1234567890, n: 0, d: 1, expected: 0, fractional_result: false },
            TestVector { val: 0, n: 1, d: 1, expected: 0, fractional_result: false },
            TestVector {
                val: 1234567890,
                n: 1,
                d: 1,
                expected: 1234567890,
                fractional_result: false,
            },
            TestVector { val: 198, n: 48000, d: 44100, expected: 215, fractional_result: true },
            TestVector { val: -198, n: 48000, d: 44100, expected: -216, fractional_result: true },
            TestVector {
                val: 49 * 198,
                n: 48000,
                d: 44100,
                expected: 10560,
                fractional_result: false,
            },
            TestVector {
                val: -(49 * 198),
                n: 48000,
                d: 44100,
                expected: -10560,
                fractional_result: false,
            },
            TestVector {
                val: (49 * 198) + 1,
                n: 48000,
                d: 44100,
                expected: 10561,
                fractional_result: true,
            },
            TestVector {
                val: -((49 * 198) + 1),
                n: 48000,
                d: 44100,
                expected: -10562,
                fractional_result: true,
            },
            TestVector {
                val: 0x1517ffffeae80,
                n: 0xbebc200,
                d: 0x33333333,
                expected: 0x4e94914f0000,
                fractional_result: false,
            },
            TestVector {
                val: -0x1517ffffeae80,
                n: 0xbebc200,
                d: 0x33333333,
                expected: -0x4e94914f0000,
                fractional_result: false,
            },
            TestVector {
                val: i64::MAX,
                n: 1000001,
                d: 1000000,
                expected: Ratio::OVERFLOW,
                fractional_result: false,
            },
            TestVector {
                val: i64::MIN,
                n: 1000001,
                d: 1000000,
                expected: Ratio::UNDERFLOW,
                fractional_result: false,
            },
            TestVector {
                val: -0x2000000000000001,
                n: 4,
                d: 1,
                expected: Ratio::UNDERFLOW,
                fractional_result: false,
            },
        ];

        for v in &test_vectors {
            let res_static = Ratio::scale_with_round::<ROUND>(v.val, v.n, v.d);
            let r = Ratio::new(v.n, v.d);
            let res_inst = r.scale::<ROUND>(v.val);

            let adjusted_expected = if !v.fractional_result || ROUND == Round::DOWN {
                v.expected
            } else if v.val >= 0 {
                if ROUND == Round::TOWARDS_ZERO { v.expected } else { v.expected + 1 }
            } else {
                if ROUND == Round::AWAY_FROM_ZERO { v.expected } else { v.expected + 1 }
            };

            assert_eq!(
                res_static, adjusted_expected,
                "Static: Expected {} * {}/{} to produce {}; got {}",
                v.val, v.n, v.d, adjusted_expected, res_static
            );
            assert_eq!(
                res_inst, adjusted_expected,
                "Instanced: Expected {} * {}/{} to produce {}; got {}",
                v.val, v.n, v.d, adjusted_expected, res_inst
            );

            if ROUND == Round::DOWN {
                let res_op1 = r * v.val;
                let res_op2 = v.val * r;
                assert_eq!(res_op1, adjusted_expected);
                assert_eq!(res_op2, adjusted_expected);

                if r.invertible() {
                    let res_op3 = v.val / r.inverse();
                    assert_eq!(res_op3, adjusted_expected);
                }
            }
        }
    }

    #[test]
    fn test_scale_round_down() {
        test_scale_helper::<{ Round::DOWN }>();
    }
    #[test]
    fn test_scale_round_up() {
        test_scale_helper::<{ Round::UP }>();
    }
    #[test]
    fn test_scale_round_towards_zero() {
        test_scale_helper::<{ Round::TOWARDS_ZERO }>();
    }
    #[test]
    fn test_scale_round_away_from_zero() {
        test_scale_helper::<{ Round::AWAY_FROM_ZERO }>();
    }

    #[test]
    fn test_inverse() {
        let test_vectors = [(1, 1), (123456, 987654)];
        for &(n, d) in &test_vectors {
            let r = Ratio::new(n, d);
            let inv = r.inverse();
            assert_eq!(inv.numerator(), d);
            assert_eq!(inv.denominator(), n);
        }

        let r = Ratio::new(0, 1);
        assert!(!r.invertible());
    }

    // The two limits used by `Ratio::scale_with_round`; the limit for
    // non-negative values, and the limit for the distance from zero of
    // negative values.
    const POSITIVE_LIMIT: u64 = i64::MAX as u64; // 0x7fffffffffffffff
    const NEGATIVE_LIMIT: u64 = 0x8000000000000000; // i64::MIN.unsigned_abs()

    // Invokes `scale_unsigned` and checks its result.  The rounding direction
    // and the limit are const generic parameters, so they are spelled as
    // literals here rather than being gathered into a table of test vectors.
    macro_rules! check_scale_unsigned {
        ($value:expr, $numerator:expr, $denominator:expr, $round_up:expr, $limit:expr,
         $expected:expr $(,)?) => {{
            let value: u64 = $value;
            let numerator: u32 = $numerator;
            let denominator: u32 = $denominator;
            let expected: u64 = $expected;
            let res = scale_unsigned::<{ $round_up }, { $limit }>(value, numerator, denominator);
            assert_eq!(
                res, expected,
                "Expected scale_unsigned::<{}, {:#x}>({:#x}, {}, {}) to produce {:#x}; got {:#x}",
                $round_up, $limit, value, numerator, denominator, expected, res
            );
        }};
    }

    // Exercises the `high_q > (LIMIT >> 32)` early-out, which both saturates
    // and keeps the subsequent `high_q << 32` from overflowing.
    #[test]
    fn test_scale_unsigned_early_out() {
        // high_q == 0x80000000, which is one more than POSITIVE_LIMIT >> 32;
        // take the early out.
        check_scale_unsigned!(0x8000000000000000, 1, 1, false, POSITIVE_LIMIT, POSITIVE_LIMIT);

        // The same value against the negative limit; high_q is now exactly
        // NEGATIVE_LIMIT >> 32, so there is no early out and the value is
        // scaled exactly.
        check_scale_unsigned!(0x8000000000000000, 1, 1, false, NEGATIVE_LIMIT, 0x8000000000000000);

        // high_q == POSITIVE_LIMIT >> 32 exactly; no early out, and the result
        // is well under the limit.
        check_scale_unsigned!(0x7fffffff00000000, 1, 1, false, POSITIVE_LIMIT, 0x7fffffff00000000);

        // The largest possible intermediates: high and low are each just shy of
        // 2^64.  The first takes the early out; the second has
        // high_q == (u64::MAX >> 32) and so does not.
        check_scale_unsigned!(u64::MAX, u32::MAX, 1, true, POSITIVE_LIMIT, POSITIVE_LIMIT);
        check_scale_unsigned!(u64::MAX, u32::MAX, u32::MAX, false, u64::MAX, u64::MAX);
    }

    // Exercises the `result >= LIMIT` saturation, which is only reachable when
    // high_q is exactly `LIMIT >> 32` and the low half pushes the result past
    // the limit.
    #[test]
    fn test_scale_unsigned_result_saturation() {
        // result == NEGATIVE_LIMIT + 1, reached without the early out.
        check_scale_unsigned!(0x8000000000000001, 1, 1, false, NEGATIVE_LIMIT, NEGATIVE_LIMIT);

        // result == NEGATIVE_LIMIT exactly, which also saturates (to the same
        // value it would have produced).
        check_scale_unsigned!(0x4000000000000000, 2, 1, false, NEGATIVE_LIMIT, NEGATIVE_LIMIT);

        // result == POSITIVE_LIMIT exactly.
        check_scale_unsigned!(POSITIVE_LIMIT, 1, 1, true, POSITIVE_LIMIT, POSITIVE_LIMIT);

        // Saturation of a small limit, where high_q is 0 and low_q alone
        // exceeds the limit.
        check_scale_unsigned!(1000, 3, 1, false, 100, 100);
    }

    // Exercises the rounding-up path, in particular the case where rounding up
    // would produce exactly the limit.
    #[test]
    fn test_scale_unsigned_round_up_at_limit() {
        // result == NEGATIVE_LIMIT - 1 with a non-zero remainder; rounding up
        // produces exactly the limit.
        check_scale_unsigned!(u64::MAX, 1, 2, true, NEGATIVE_LIMIT, NEGATIVE_LIMIT);

        // The same inputs without rounding up produce the truncated value.
        check_scale_unsigned!(u64::MAX, 1, 2, false, NEGATIVE_LIMIT, NEGATIVE_LIMIT - 1);

        // result == POSITIVE_LIMIT - 1 with a non-zero remainder; rounding up
        // produces exactly the limit.
        check_scale_unsigned!(0xfffffffffffffffd, 1, 2, true, POSITIVE_LIMIT, POSITIVE_LIMIT);

        // result == POSITIVE_LIMIT - 2 with a non-zero remainder; rounding up
        // stays below the limit.
        check_scale_unsigned!(0xfffffffffffffffb, 1, 2, true, POSITIVE_LIMIT, POSITIVE_LIMIT - 1);

        // An exact result is never rounded up.
        check_scale_unsigned!(0xfffffffffffffffc, 1, 2, true, POSITIVE_LIMIT, POSITIVE_LIMIT - 1);

        // Rounding up a small result.
        check_scale_unsigned!(198, 48000, 44100, true, POSITIVE_LIMIT, 216);
        check_scale_unsigned!(198, 48000, 44100, false, POSITIVE_LIMIT, 215);
    }

    // A straightforward 128-bit reference implementation of the scaling
    // operation, used to validate the 64-bit implementation.
    fn scale_unsigned_reference(
        value: u64,
        numerator: u32,
        denominator: u32,
        round_up: bool,
        limit: u64,
    ) -> u64 {
        let prod = (value as u128) * (numerator as u128);
        let q = prod / (denominator as u128);
        let r = prod % (denominator as u128);

        if q >= (limit as u128) {
            return limit;
        }

        let mut result = q as u64;
        if round_up && r != 0 {
            result += 1;
            if result >= limit {
                return limit;
            }
        }

        result
    }

    // Compares `scale_unsigned` against the reference implementation across a
    // range of values, numerators, and denominators, for one particular
    // rounding direction and limit.
    fn check_against_reference<const ROUND_UP: bool, const LIMIT: u64>() {
        let values = [
            0,
            1,
            2,
            0xffff_ffff,
            0x1_0000_0000,
            0x1_0000_0001,
            0x7fff_ffff_ffff_fffe,
            POSITIVE_LIMIT,
            NEGATIVE_LIMIT,
            0x8000_0000_0000_0001,
            0xffff_ffff_ffff_fffd,
            u64::MAX,
            0x1517ffffeae80,
            1234567890,
        ];
        let numerators = [0, 1, 2, 3, 48000, 44100, 1000001, 0x7fff_ffff, u32::MAX];
        let denominators = [1, 2, 3, 7, 44100, 1000000, 0x7fff_ffff, u32::MAX];

        for &value in &values {
            for &numerator in &numerators {
                for &denominator in &denominators {
                    let res = scale_unsigned::<ROUND_UP, LIMIT>(value, numerator, denominator);
                    let expected =
                        scale_unsigned_reference(value, numerator, denominator, ROUND_UP, LIMIT);
                    assert_eq!(
                        res, expected,
                        "Expected scale_unsigned::<{}, {:#x}>({:#x}, {}, {}) to produce \
                         {:#x}; got {:#x}",
                        ROUND_UP, LIMIT, value, numerator, denominator, expected, res
                    );
                }
            }
        }
    }

    #[test]
    fn test_scale_unsigned_matches_reference() {
        macro_rules! check_against_reference_for_limits {
            ($($limit:expr),* $(,)?) => {
                $(
                    check_against_reference::<false, { $limit }>();
                    check_against_reference::<true, { $limit }>();
                )*
            };
        }

        check_against_reference_for_limits!(0, 1, 2, 100, POSITIVE_LIMIT, NEGATIVE_LIMIT, u64::MAX,);
    }
}
