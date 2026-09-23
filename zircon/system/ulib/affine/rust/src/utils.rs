// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// Performs saturating addition on i64.
///
/// This implements a clamping policy in the case of overflow, returning
/// `i64::MIN` or `i64::MAX`.
pub fn clamp_add(a: i64, b: i64) -> i64 {
    a.saturating_add(b)
}

/// Performs saturating subtraction on i64.
///
/// This implements a clamping policy in the case of overflow, returning
/// `i64::MIN` or `i64::MAX`.
pub fn clamp_sub(a: i64, b: i64) -> i64 {
    a.saturating_sub(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX: i64 = i64::MAX;
    const MIN: i64 = i64::MIN;

    struct TestVector {
        a: i64,
        b: i64,
        expected: i64,
    }

    #[test]
    fn test_clamp_add() {
        let test_vectors = [
            TestVector { a: 15, b: 25, expected: 40 },
            TestVector { a: 15, b: -25, expected: -10 },
            TestVector { a: 15, b: MAX - 16, expected: MAX - 1 },
            TestVector { a: 15, b: MAX - 15, expected: MAX - 0 },
            TestVector { a: 15, b: MAX - 14, expected: MAX - 0 },
            TestVector { a: MAX - 16, b: 15, expected: MAX - 1 },
            TestVector { a: MAX - 15, b: 15, expected: MAX - 0 },
            TestVector { a: MAX - 14, b: 15, expected: MAX - 0 },
            TestVector { a: -15, b: MIN + 16, expected: MIN + 1 },
            TestVector { a: -15, b: MIN + 15, expected: MIN + 0 },
            TestVector { a: -15, b: MIN + 14, expected: MIN + 0 },
            TestVector { a: MIN + 16, b: -15, expected: MIN + 1 },
            TestVector { a: MIN + 15, b: -15, expected: MIN + 0 },
            TestVector { a: MIN + 14, b: -15, expected: MIN + 0 },
            TestVector { a: MAX, b: MAX - 1, expected: MAX },
            TestVector { a: MAX - 1, b: MAX, expected: MAX },
            TestVector { a: MAX, b: MAX, expected: MAX },
        ];

        for v in &test_vectors {
            let result = clamp_add(v.a, v.b);
            assert_eq!(result, v.expected, "test case: {} + {}", v.a, v.b);
        }
    }

    #[test]
    fn test_clamp_sub() {
        let test_vectors = [
            TestVector { a: 15, b: 25, expected: -10 },
            TestVector { a: 15, b: -25, expected: 40 },
            TestVector { a: -15, b: MAX - 16, expected: MIN + 2 },
            TestVector { a: -15, b: MAX - 15, expected: MIN + 1 },
            TestVector { a: -15, b: MAX - 14, expected: MIN + 0 },
            TestVector { a: -15, b: MAX - 13, expected: MIN + 0 },
            TestVector { a: MIN + 16, b: 15, expected: MIN + 1 },
            TestVector { a: MIN + 15, b: 15, expected: MIN + 0 },
            TestVector { a: MIN + 14, b: 15, expected: MIN + 0 },
            TestVector { a: 15, b: MIN + 15, expected: MAX - 0 },
            TestVector { a: 15, b: MIN + 16, expected: MAX - 0 },
            TestVector { a: 15, b: MIN + 17, expected: MAX - 1 },
            TestVector { a: MAX - 16, b: -15, expected: MAX - 1 },
            TestVector { a: MAX - 15, b: -15, expected: MAX - 0 },
            TestVector { a: MAX - 14, b: -15, expected: MAX - 0 },
            TestVector { a: 0, b: MIN + 0, expected: MAX - 0 },
            TestVector { a: 0, b: MIN + 1, expected: MAX - 0 },
            TestVector { a: 0, b: MIN + 2, expected: MAX - 1 },
            TestVector { a: MIN, b: MIN + 1, expected: -1 },
            TestVector { a: MIN, b: MIN, expected: 0 },
        ];

        for v in &test_vectors {
            let result = clamp_sub(v.a, v.b);
            assert_eq!(result, v.expected, "test case: {} - {}", v.a, v.b);
        }
    }
}
