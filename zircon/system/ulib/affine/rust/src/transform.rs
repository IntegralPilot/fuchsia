// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::ratio::{Exact, Ratio, Round};
use zr::static_assert;

pub struct Saturate;
#[allow(non_upper_case_globals)]
impl Saturate {
    pub const No: bool = false;
    pub const Yes: bool = true;
}

/// A small helper struct which represents a 1 dimensional affine transformation
/// from a signed 64 bit space A, to a signed 64 bit space B.  Conceptually, this
/// is the function...
///
/// f(a) = b = (a * scale) + offset
///
/// Internally, however, the exact function used is
///
/// f(a) = b = (((a - A_offset) * B_scale) / A_scale) + B_offset
///
/// Where the offsets involved are 64 bit signed integers, and the scale factors
/// are 32 bit unsigned integers.
///
/// Overflow/Underflow saturation behavior is as follows.
/// The transformation operation is divided into three stages.
///
/// 1) Offset by A_offset
/// 2) Scale by (B_scale / A_scale)
/// 3) Offset by B_offset
///
/// Each stage is saturated independently.  That is to say, if the result of
/// stage #1 is clamped at int64::min, this is the input value which will be fed
/// into stage #2.  The calculations are *not* done with infinite precision and
/// then clamped at the end.
///
/// TODO(johngro): Reconsider this.  Clamping at intermediate stages can make it
/// more difficult to understand that saturation happened at all, and might be
/// important to a client.  It may be better to either signal explicitly that
/// this happened, or to extend the precision of the operation in the rare slow
/// path so that saturation behavior occurs only at the end of the op, and
/// produces a correct result if the transform would have saturated at an
/// intermediate step, but got brought back into range by a subsequent operation.
///
/// Saturation is enabled by default, but may be disabled by choosing the
/// Saturate::No form of Apply/ApplyInverse.  When saturation behavior is
/// disabled, the results of a transformation where over/underflow occurs at any
/// stage is undefined.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transform {
    a_offset: i64,
    b_offset: i64,
    ratio: Ratio,
}

static_assert!(core::mem::size_of::<Transform>() == 24);
static_assert!(core::mem::align_of::<Transform>() == 8);

// Transform::default() produces the identity transform
impl Default for Transform {
    fn default() -> Self {
        Transform { a_offset: 0, b_offset: 0, ratio: Ratio::default() }
    }
}

// TODO(https://fxbug.dev/42082948)
impl Transform {
    /// Constructs a new Transform.
    pub fn new(a_offset: i64, b_offset: i64, ratio: Ratio) -> Self {
        Transform { a_offset, b_offset, ratio }
    }

    // Construct a linear transformation (zero offsets) from a ratio
    pub fn new_linear(ratio: Ratio) -> Self {
        Transform { a_offset: 0, b_offset: 0, ratio }
    }

    pub fn invertible(&self) -> bool {
        self.ratio.invertible()
    }

    pub fn a_offset(&self) -> i64 {
        self.a_offset
    }

    pub fn b_offset(&self) -> i64 {
        self.b_offset
    }

    pub fn ratio(&self) -> Ratio {
        self.ratio
    }

    pub fn numerator(&self) -> u32 {
        self.ratio.numerator()
    }

    pub fn denominator(&self) -> u32 {
        self.ratio.denominator()
    }

    // Construct and return a transform which is the inverse of this transform.
    pub fn inverse(&self) -> Self {
        Transform { a_offset: self.b_offset, b_offset: self.a_offset, ratio: self.ratio.inverse() }
    }

    // Applies a transformation from A -> B
    pub fn apply_static<const SATURATE: bool>(
        a_offset: i64,
        b_offset: i64,
        ratio: Ratio,
        val: i64,
    ) -> i64 {
        if SATURATE {
            let sub = val.saturating_sub(a_offset);
            let scaled = ratio.scale::<{ Round::DOWN }>(sub);
            scaled.saturating_add(b_offset)
        } else {
            // TODO(johngro): the multiplication by the ratio operation here
            // actually implements saturation behavior.  If we want this
            // operation to actually perform no saturation checks at all, we
            // need to make a Saturate::No version of Ratio::Scale.
            let sub = val.wrapping_sub(a_offset);
            let scaled = ratio.scale::<{ Round::DOWN }>(sub);
            scaled.wrapping_add(b_offset)
        }
    }

    // Applies the inverse transformation B -> A
    pub fn apply_inverse_static<const SATURATE: bool>(
        a_offset: i64,
        b_offset: i64,
        ratio: Ratio,
        val: i64,
    ) -> i64 {
        Self::apply_static::<SATURATE>(b_offset, a_offset, ratio.inverse(), val)
    }

    // Applies the transformation
    pub fn apply<const SATURATE: bool>(&self, val: i64) -> i64 {
        Self::apply_static::<SATURATE>(self.a_offset, self.b_offset, self.ratio, val)
    }

    // Applies the inverse transformation
    pub fn apply_inverse<const SATURATE: bool>(&self, val: i64) -> i64 {
        debug_assert!(self.ratio.denominator() != 0);
        Self::apply_inverse_static::<SATURATE>(self.a_offset, self.b_offset, self.ratio, val)
    }

    // Composes two timeline functions B->C and A->B producing A->C. If exact is
    // Exact::Yes, asserts on loss of precision.
    //
    // During composition, the saturation behavior is as follows
    //
    // 1) The intermediate offset (bc.a_offset - ab.b_offset) will be saturated
    //    before distribution to the offsets ac.
    // 2) Both offsets of ac will be saturated as ab.a_offset and bc.b_offset
    //    are combined with the distributed intermediate offset.
    pub fn compose(bc: &Transform, ab: &Transform, exact: Exact) -> Transform {
        Transform {
            a_offset: ab.a_offset,
            b_offset: bc.apply::<{ Saturate::Yes }>(ab.b_offset),
            ratio: Ratio::product(ab.ratio, bc.ratio, exact),
        }
    }
}

// Operators

/// Composes two timeline functions B->C and A->B producing A->C.
///
/// Panics on loss of precision.
impl core::ops::Mul for Transform {
    type Output = Self;
    fn mul(self, rhs: Self) -> Self::Output {
        Transform::compose(&self, &rhs, Exact::Yes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_construction() {
        let t = Transform::default();
        assert_eq!(t.a_offset(), 0);
        assert_eq!(t.b_offset(), 0);
        assert_eq!(t.numerator(), 1);
        assert_eq!(t.denominator(), 1);

        struct TestVector {
            a_offset: i64,
            b_offset: i64,
            n: u32,
            d: u32,
        }

        let valid_vectors = [
            TestVector { a_offset: 12345, b_offset: 98764, n: 3, d: 2 },
            TestVector { a_offset: -12345, b_offset: 98764, n: 247, d: 931 },
            TestVector { a_offset: -12345, b_offset: -98764, n: 48000, d: 44100 },
            TestVector { a_offset: 12345, b_offset: -98764, n: 1000007, d: 1000000 },
            TestVector { a_offset: 12345, b_offset: 98764, n: 0, d: 1000000 },
        ];

        for v in &valid_vectors {
            let ratio = Ratio::new(v.n, v.d);

            let t_linear = Transform::new_linear(ratio);
            assert_eq!(t_linear.a_offset(), 0);
            assert_eq!(t_linear.b_offset(), 0);
            assert_eq!(t_linear.numerator(), ratio.numerator());
            assert_eq!(t_linear.denominator(), ratio.denominator());

            let t_affine = Transform::new(v.a_offset, v.b_offset, ratio);
            assert_eq!(t_affine.a_offset(), v.a_offset);
            assert_eq!(t_affine.b_offset(), v.b_offset);
            assert_eq!(t_affine.numerator(), ratio.numerator());
            assert_eq!(t_affine.denominator(), ratio.denominator());
        }
    }

    #[test]
    fn test_inverse() {
        struct TestVector {
            a_offset: i64,
            b_offset: i64,
            n: u32,
            d: u32,
        }

        let test_vectors = [
            TestVector { a_offset: 12345, b_offset: 98764, n: 3, d: 2 },
            TestVector { a_offset: -12345, b_offset: 98764, n: 247, d: 931 },
            TestVector { a_offset: -12345, b_offset: -98764, n: 48000, d: 44100 },
            TestVector { a_offset: 12345, b_offset: -98764, n: 1000007, d: 1000000 },
        ];

        for v in &test_vectors {
            let ratio = Ratio::new(v.n, v.d);
            let t = Transform::new(v.a_offset, v.b_offset, ratio);

            if t.invertible() {
                let res = t.inverse();
                assert_eq!(t.a_offset(), res.b_offset());
                assert_eq!(t.b_offset(), res.a_offset());
                assert_eq!(t.numerator(), res.denominator());
                assert_eq!(t.denominator(), res.numerator());
                assert_eq!(t.ratio().inverse().numerator(), res.ratio().numerator());
                assert_eq!(t.ratio().inverse().denominator(), res.ratio().denominator());
            }
        }

        let t_non_inv = Transform::new(12345, 98764, Ratio::new(0, 1000000));
        assert!(!t_non_inv.invertible());
    }

    #[test]
    fn test_apply() {
        struct TestVector {
            a_offset: i64,
            b_offset: i64,
            n: u32,
            d: u32,
            val: i64,
            expected: i64,
            expect_ovfl: bool,
        }

        let test_vectors = [
            TestVector {
                a_offset: 0,
                b_offset: 0,
                n: 1,
                d: 1,
                val: 12345,
                expected: 12345,
                expect_ovfl: false,
            },
            TestVector {
                a_offset: 50,
                b_offset: 0,
                n: 1,
                d: 1,
                val: 12345,
                expected: 12295,
                expect_ovfl: false,
            },
            TestVector {
                a_offset: 0,
                b_offset: -50,
                n: 1,
                d: 1,
                val: 12345,
                expected: 12295,
                expect_ovfl: false,
            },
            TestVector {
                a_offset: 50,
                b_offset: -50,
                n: 1,
                d: 1,
                val: 12345,
                expected: 12245,
                expect_ovfl: false,
            },
            TestVector {
                a_offset: 50,
                b_offset: 50,
                n: 1,
                d: 1,
                val: 12345,
                expected: 12345,
                expect_ovfl: false,
            },
            TestVector {
                a_offset: 0,
                b_offset: 0,
                n: 48000,
                d: 44100,
                val: 12345,
                expected: 13436,
                expect_ovfl: false,
            },
            TestVector {
                a_offset: 50,
                b_offset: 0,
                n: 48000,
                d: 44100,
                val: 12345,
                expected: 13382,
                expect_ovfl: false,
            },
            TestVector {
                a_offset: 0,
                b_offset: -54,
                n: 48000,
                d: 44100,
                val: 12345,
                expected: 13382,
                expect_ovfl: false,
            },
            TestVector {
                a_offset: 50,
                b_offset: -54,
                n: 48000,
                d: 44100,
                val: 12345,
                expected: 13328,
                expect_ovfl: false,
            },
            TestVector {
                a_offset: 50,
                b_offset: 54,
                n: 48000,
                d: 44100,
                val: 12345,
                expected: 13436,
                expect_ovfl: false,
            },
            TestVector {
                a_offset: -100,
                b_offset: -17,
                n: 1,
                d: 1,
                val: i64::MAX - 1,
                expected: i64::MAX - 17,
                expect_ovfl: true,
            },
            TestVector {
                a_offset: 100,
                b_offset: 17,
                n: 1,
                d: 1,
                val: i64::MIN + 1,
                expected: i64::MIN + 17,
                expect_ovfl: true,
            },
            TestVector {
                a_offset: 0,
                b_offset: -17,
                n: 3,
                d: 1,
                val: i64::MAX / 2,
                expected: i64::MAX - 17,
                expect_ovfl: true,
            },
            TestVector {
                a_offset: 0,
                b_offset: 17,
                n: 3,
                d: 1,
                val: i64::MIN / 2,
                expected: i64::MIN + 17,
                expect_ovfl: true,
            },
            TestVector {
                a_offset: 0,
                b_offset: 17,
                n: 1,
                d: 1,
                val: i64::MAX - 10,
                expected: i64::MAX,
                expect_ovfl: true,
            },
            TestVector {
                a_offset: 0,
                b_offset: -17,
                n: 1,
                d: 1,
                val: i64::MIN + 10,
                expected: i64::MIN,
                expect_ovfl: true,
            },
        ];

        for v in &test_vectors {
            let t = Transform::new(v.a_offset, v.b_offset, Ratio::new(v.n, v.d));

            let res_sat_static = Transform::apply_static::<{ Saturate::Yes }>(
                t.a_offset(),
                t.b_offset(),
                t.ratio(),
                v.val,
            );
            assert_eq!(res_sat_static, v.expected);

            if !v.expect_ovfl {
                let res_nosat_static = Transform::apply_static::<{ Saturate::No }>(
                    t.a_offset(),
                    t.b_offset(),
                    t.ratio(),
                    v.val,
                );
                assert_eq!(res_nosat_static, v.expected);
            }

            let res_sat_obj = t.apply::<{ Saturate::Yes }>(v.val);
            assert_eq!(res_sat_obj, v.expected);

            if !v.expect_ovfl {
                let res_nosat_obj = t.apply::<{ Saturate::No }>(v.val);
                assert_eq!(res_nosat_obj, v.expected);
            }

            if t.invertible() {
                let t_inv = t.inverse();

                let res_sat_inv_static = Transform::apply_inverse_static::<{ Saturate::Yes }>(
                    t_inv.a_offset(),
                    t_inv.b_offset(),
                    t_inv.ratio(),
                    v.val,
                );
                assert_eq!(res_sat_inv_static, v.expected);

                if !v.expect_ovfl {
                    let res_nosat_inv_static = Transform::apply_inverse_static::<{ Saturate::No }>(
                        t_inv.a_offset(),
                        t_inv.b_offset(),
                        t_inv.ratio(),
                        v.val,
                    );
                    assert_eq!(res_nosat_inv_static, v.expected);
                }

                let res_sat_inv_obj = t_inv.apply_inverse::<{ Saturate::Yes }>(v.val);
                assert_eq!(res_sat_inv_obj, v.expected);

                if !v.expect_ovfl {
                    let res_nosat_inv_obj = t_inv.apply_inverse::<{ Saturate::No }>(v.val);
                    assert_eq!(res_nosat_inv_obj, v.expected);
                }
            }
        }
    }

    #[test]
    fn test_compose() {
        struct TestVector {
            ab: Transform,
            bc: Transform,
            ac: Transform,
            is_exact: Exact,
        }

        let test_vectors = [
            TestVector {
                ab: Transform::new(0, 0, Ratio::new(1, 1)),
                bc: Transform::new(0, 0, Ratio::new(1, 1)),
                ac: Transform::new(0, 0, Ratio::new(1, 1)),
                is_exact: Exact::Yes,
            },
            TestVector {
                ab: Transform::new(0, 0, Ratio::new(1, 1)),
                bc: Transform::new(12345, 98765, Ratio::new(17, 7)),
                ac: Transform::new(0, 68784, Ratio::new(17, 7)),
                is_exact: Exact::Yes,
            },
            TestVector {
                ab: Transform::new(12345, 98765, Ratio::new(17, 7)),
                bc: Transform::new(0, 0, Ratio::new(1, 1)),
                ac: Transform::new(12345, 98765, Ratio::new(17, 7)),
                is_exact: Exact::Yes,
            },
            TestVector {
                ab: Transform::new(34327, 86539, Ratio::new(1000007, 1000000)),
                bc: Transform::new(728376, -34265, Ratio::new(48000, 44100)),
                ac: Transform::new(34327, -732864, Ratio::new(1000007, 918750)),
                is_exact: Exact::Yes,
            },
            TestVector {
                ab: Transform::new(0, i64::MAX - 5, Ratio::new(1, 1)),
                bc: Transform::new(-100, 0, Ratio::new(1, 1)),
                ac: Transform::new(0, i64::MAX, Ratio::new(1, 1)),
                is_exact: Exact::Yes,
            },
            TestVector {
                ab: Transform::new(0, i64::MIN + 5, Ratio::new(1, 1)),
                bc: Transform::new(100, 0, Ratio::new(1, 1)),
                ac: Transform::new(0, i64::MIN, Ratio::new(1, 1)),
                is_exact: Exact::Yes,
            },
            TestVector {
                ab: Transform::new(0, 100, Ratio::new(1, 1)),
                bc: Transform::new(0, i64::MAX - 5, Ratio::new(1, 1)),
                ac: Transform::new(0, i64::MAX, Ratio::new(1, 1)),
                is_exact: Exact::Yes,
            },
            TestVector {
                ab: Transform::new(0, -100, Ratio::new(1, 1)),
                bc: Transform::new(0, i64::MIN + 5, Ratio::new(1, 1)),
                ac: Transform::new(0, i64::MIN, Ratio::new(1, 1)),
                is_exact: Exact::Yes,
            },
            TestVector {
                ab: Transform::new(0, 0, Ratio::new(3465653567, 2327655023)),
                bc: Transform::new(0, 0, Ratio::new(1291540343, 3698423317)),
                ac: Transform::new(0, 0, Ratio::new(317609835, 610852072)),
                is_exact: Exact::No,
            },
            TestVector {
                ab: Transform::new(0, 20, Ratio::new(3465653567, 2327655023)),
                bc: Transform::new(-3698423317 + 20, 5, Ratio::new(1291540343, 3698423317)),
                ac: Transform::new(0, 1291540343 + 5, Ratio::new(317609835, 610852072)),
                is_exact: Exact::No,
            },
        ];

        for v in &test_vectors {
            if v.is_exact == Exact::Yes {
                let res_static = Transform::compose(&v.bc, &v.ab, Exact::Yes);
                assert_eq!(res_static, v.ac);

                let res_op = v.bc * v.ab;
                assert_eq!(res_op, v.ac);
            }

            let res_inexact = Transform::compose(&v.bc, &v.ab, Exact::No);
            assert_eq!(res_inexact, v.ac);
        }
    }
}
