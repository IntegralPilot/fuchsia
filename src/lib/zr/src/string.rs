// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// Parse a decimal integer from a string slice at compile time.
///
/// Returns a `Some(usize)` value on success, or `None` on failure.
pub const fn parse_usize(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    let mut val: usize = 0;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b < b'0' || b > b'9' {
            return None;
        }
        let digit = (b - b'0') as usize;
        match val.checked_mul(10) {
            Some(v) => match v.checked_add(digit) {
                Some(final_val) => val = final_val,
                None => return None,
            },
            None => return None,
        }
        i += 1;
    }
    Some(val)
}

/// Copy a string slice into a fixed-size byte array at compile time, leaving space for a null
/// terminator.
pub const fn to_array<const N: usize>(s: &str) -> [u8; N] {
    assert!(N > 0, "N must be non-zero");
    let bytes = s.as_bytes();
    let mut arr = [0; N];
    let mut i = 0;
    while i < bytes.len() && i < N - 1 {
        arr[i] = bytes[i];
        i += 1;
    }
    arr
}
