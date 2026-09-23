// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub fn round_up(a: usize, b: usize) -> usize {
    (a + b - 1) / b * b
}

pub use round_up as align;

pub fn round_down(a: usize, b: usize) -> usize {
    a - (a % b)
}

pub fn is_aligned(a: usize, b: usize) -> bool {
    (a & (b - 1)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_round_up() {
        assert_eq!(round_up(0, 4), 0);
        assert_eq!(round_up(1, 4), 4);
        assert_eq!(round_up(3, 4), 4);
        assert_eq!(round_up(4, 4), 4);
        assert_eq!(round_up(5, 4), 8);
    }

    #[test]
    fn test_align() {
        assert_eq!(align(0, 4), 0);
        assert_eq!(align(1, 4), 4);
        assert_eq!(align(3, 4), 4);
        assert_eq!(align(4, 4), 4);
        assert_eq!(align(5, 4), 8);
    }

    #[test]
    fn test_round_down() {
        assert_eq!(round_down(0, 4), 0);
        assert_eq!(round_down(1, 4), 0);
        assert_eq!(round_down(3, 4), 0);
        assert_eq!(round_down(4, 4), 4);
        assert_eq!(round_down(5, 4), 4);
    }

    #[test]
    fn test_is_aligned() {
        assert!(is_aligned(0, 4));
        assert!(!is_aligned(1, 4));
        assert!(!is_aligned(2, 4));
        assert!(!is_aligned(3, 4));
        assert!(is_aligned(4, 4));
    }
}
