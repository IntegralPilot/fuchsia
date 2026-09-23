// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use lib::add;

/// Wraps the add function for demonstration purposes.
fn wrap_add(a: i32, b: i32) -> i32 {
    add(a, b)
}

fn main() {
    println!("1 + 2 = {}", wrap_add(1, 2));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add() {
        assert_eq!(add(1, 2), 3);
    }
}
