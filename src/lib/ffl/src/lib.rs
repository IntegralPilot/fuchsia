// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Fuchsia Fixed-Point Library (FFL)
//!
//! FFL is a fixed-point arithmetic library ported from C++. It provides:
//! - [`Fixed`]: The primary fixed-point type, parameterized by the underlying
//!   integer type and fractional bit count.
//! - [`FastFixed`]: A wrapper type designed for high-performance arithmetic
//!   operations on 64-bit platforms, by saturating to 64-bit intermediates
//!   instead of standard 128-bit intermediates.
//! - [`ExponentialAverage`]: A utility for tracking exponential moving averages
//!   and variance using fixed-point values.
//! - Expression templates: Deferred-evaluation expression structures like
//!   [`expression::DivExpression`] to maintain resolution and precision for
//!   division operations.

#![cfg_attr(not(test), no_std)]

pub mod exponential_average;
pub mod expression;
pub mod fast;
pub mod fixed;
pub mod fixed_format;
pub mod num_traits;
pub(crate) mod saturating_arithmetic;
pub mod string;
pub(crate) mod utility;

// Re-export public API
pub use exponential_average::ExponentialAverage;
pub use expression::{from_ratio, to_resolution};
pub use fast::FastFixed;
pub use fixed::{Fixed, compare_fixed};
pub use string::{Mode, String};

/// Rounds the given fixed-point value to the nearest integer using
/// round-half-to-even (convergent rounding).
pub fn round<I, const FRAC: usize>(val: Fixed<I, FRAC>) -> I
where
    I: num_traits::Bounded + num_traits::NumCast + Copy + Into<i128> + num_traits::Zero,
{
    val.round()
}

/// Creates a `Fixed` value from a raw underlying integer value.
pub const fn from_raw<I: Copy, const FRAC: usize>(value: I) -> Fixed<I, FRAC> {
    Fixed::from_raw(value)
}

#[cfg(test)]
mod tests;
