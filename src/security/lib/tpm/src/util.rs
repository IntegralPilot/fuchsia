// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// Returns true if the flag is set.
pub fn flag_set(base: u32, flag: u32) -> bool {
    (base & flag) == flag
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flag_set() {
        assert!(flag_set(0b11, 0b10));
        assert!(flag_set(0b1011, 0b1000));
    }

    #[test]
    fn test_flag_not_set() {
        assert!(!flag_set(0b11, 0b100));
        assert!(!flag_set(0b1011, 0b0100));
    }
}
