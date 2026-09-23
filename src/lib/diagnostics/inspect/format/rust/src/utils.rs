// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::constants;

/// Returns the smallest order such that (MIN_ORDER_SHIFT << order) >= size.
/// Size must be non zero.
pub fn fit_order(size: usize) -> usize {
    if size == 0 {
        return 0;
    }
    // Safety: `leading_zeros` returns a u32, so this is safe promotion
    (std::mem::size_of::<usize>() * 8 - (size - 1).leading_zeros() as usize)
        .saturating_sub(constants::MIN_ORDER_SHIFT)
}

/// Get size in bytes of a given |order|.
pub fn order_to_size(order: u8) -> usize {
    constants::MIN_ORDER_SIZE << order
}

/// Get the necessary |block size| to fit the given |payload_size| in range
/// MIN_ORDER_SIZE <= block size <= MAX_ORDER_SIZE
pub fn block_size_for_payload(payload_size: usize) -> usize {
    (payload_size + constants::HEADER_SIZE_BYTES)
        .clamp(constants::MIN_ORDER_SIZE, constants::MAX_ORDER_SIZE)
}

/// Get the size in bytes for the payload section of a block of the given |order|.
pub fn payload_size_for_order(order: u8) -> usize {
    order_to_size(order) - constants::HEADER_SIZE_BYTES
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn fit_order_test() {
        assert_eq!(0, fit_order(1));
        assert_eq!(0, fit_order(16));
        assert_eq!(1, fit_order(17));
        assert_eq!(2, fit_order(33));
        assert_eq!(7, fit_order(2048));
    }

    #[test]
    fn order_to_size_test() {
        assert_eq!(16, order_to_size(0));
        assert_eq!(32, order_to_size(1));
        assert_eq!(64, order_to_size(2));
        assert_eq!(128, order_to_size(3));
        assert_eq!(256, order_to_size(4));
        assert_eq!(512, order_to_size(5));
        assert_eq!(1024, order_to_size(6));
        assert_eq!(2048, order_to_size(7));
    }

    #[test]
    fn fit_payload_test() {
        for payload_size in 0..500 {
            let block_size = block_size_for_payload(payload_size);
            let order = fit_order(block_size) as u8;
            let payload_max = payload_size_for_order(order);
            assert!(
                payload_size <= payload_max,
                "Needed {payload_size} bytes for a payload, but only got {payload_max}; block size {block_size}, order {order}"
            );
        }
    }
}
