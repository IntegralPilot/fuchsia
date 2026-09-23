// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::num_traits::Bounded;

/// Returns the saturated result of addition.
pub fn saturate_add<T, U, R>(a: T, b: U) -> R
where
    T: Into<i128>,
    U: Into<i128>,
    R: Bounded + TryFrom<i128>,
{
    let a_i: i128 = a.into();
    let b_i: i128 = b.into();
    crate::utility::clamp_cast(a_i.saturating_add(b_i))
}

/// Returns the saturated result of subtraction.
pub fn saturate_subtract<T, U, R>(a: T, b: U) -> R
where
    T: Into<i128>,
    U: Into<i128>,
    R: Bounded + TryFrom<i128>,
{
    let a_i: i128 = a.into();
    let b_i: i128 = b.into();
    crate::utility::clamp_cast(a_i.saturating_sub(b_i))
}

/// Returns the saturated result of multiplication.
pub fn saturate_multiply<T, U, R>(a: T, b: U) -> R
where
    T: Into<i128>,
    U: Into<i128>,
    R: Bounded + TryFrom<i128>,
{
    let a_i: i128 = a.into();
    let b_i: i128 = b.into();
    crate::utility::clamp_cast(a_i.saturating_mul(b_i))
}
