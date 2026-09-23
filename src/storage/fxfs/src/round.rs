// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// Round `offset` up to next multiple of `block_size`.
/// This function will fail if rounding up leads to an integer overflow.
///
/// (Note that unstable rust is currently adding the same function
/// `{integer}::checked_next_multiple_of()` behind the "int_roundings" feature.)
pub fn round_up<T: Into<U>, U: Copy + num_traits::PrimInt>(offset: U, block_size: T) -> Option<U> {
    let block_size = block_size.into();
    #[allow(clippy::eq_op)]
    let one = block_size / block_size;
    Some(round_down(offset.checked_add(&(block_size - one))?, block_size))
}

/// Round `offset` down to the previous multiple of `block_size`.
pub fn round_down<
    T: Into<U>,
    U: Copy + std::ops::Rem<U, Output = U> + std::ops::Sub<U, Output = U>,
>(
    offset: U,
    block_size: T,
) -> U {
    let block_size = block_size.into();
    offset - offset % block_size
}

/// Computes the rounded division of `numerator` / `denominator`.
///
/// Returns None for an arithmetic error (overflow, or divide-by-zero).
pub fn round_div(numerator: u64, denominator: u64) -> Option<u64> {
    numerator.checked_add(denominator / 2)?.checked_div(denominator)
}

#[cfg(test)]
mod tests {
    use crate::round::round_div;

    #[test]
    fn test_round_div() {
        assert_eq!(round_div(9, 4), Some(2));
        assert_eq!(round_div(10, 4), Some(3));
        assert_eq!(round_div(11, 4), Some(3));
        assert_eq!(round_div(12, 4), Some(3));
        assert_eq!(round_div(13, 4), Some(3));
        assert_eq!(round_div(14, 4), Some(4));
        assert_eq!(round_div(0, 1), Some(0));
        assert_eq!(round_div(100, 0), None);
        assert_eq!(round_div(u64::MAX, 2), None);
    }
}
