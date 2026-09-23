// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use affine::{Exact, Ratio, Saturate, Transform};
use arbitrary::{Arbitrary, Unstructured};
use fuzz::fuzz;

#[derive(Debug)]
struct FuzzInput {
    ratio1_num: u32,
    ratio1_den: u32,
    ratio2_num: u32,
    ratio2_den: u32,
    scale_val: i64,
    trans1_a: i64,
    trans1_b: i64,
    trans2_a: i64,
    trans2_b: i64,
}

impl<'a> Arbitrary<'a> for FuzzInput {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        Ok(FuzzInput {
            ratio1_num: u.arbitrary()?,
            ratio1_den: u.int_in_range(1..=u32::MAX)?,
            ratio2_num: u.arbitrary()?,
            ratio2_den: u.int_in_range(1..=u32::MAX)?,
            scale_val: u.arbitrary()?,
            trans1_a: u.arbitrary()?,
            trans1_b: u.arbitrary()?,
            trans2_a: u.arbitrary()?,
            trans2_b: u.arbitrary()?,
        })
    }
}

#[fuzz]
fn affine_fuzzer(input: FuzzInput) {
    // Construct Ratios
    let r1 = Ratio::new(input.ratio1_num, input.ratio1_den);
    let r2 = Ratio::new(input.ratio2_num, input.ratio2_den);

    // Call Reduce on copies
    let mut r1_copy = r1;
    let mut r2_copy = r2;
    r1_copy.reduce();
    r2_copy.reduce();

    // Product (with Exact::No to avoid panics on precision loss)
    let _p1 = Ratio::product(r1, r2, Exact::No);
    let _p2 = Ratio::product(r2, r1, Exact::No);

    // Scale
    let _s1 = r1 * input.scale_val;
    let _s2 = r2 * input.scale_val;

    // Construct Transforms
    let t1 = Transform::new(input.trans1_a, input.trans1_b, r1);
    let t2 = Transform::new(input.trans2_a, input.trans2_b, r2);

    // Apply (only Apply, not ApplyInverse, to avoid potential non-invertible errors)
    let _a1 = t1.apply::<{ Saturate::Yes }>(input.scale_val);
    let _a2 = t2.apply::<{ Saturate::Yes }>(input.scale_val);

    // Compose
    let composed1 = Transform::compose(&t1, &t2, Exact::No);
    let _ = composed1.apply::<{ Saturate::Yes }>(input.scale_val);

    let composed2 = Transform::compose(&t2, &t1, Exact::No);
    let _ = composed2.apply::<{ Saturate::Yes }>(input.scale_val);
}
