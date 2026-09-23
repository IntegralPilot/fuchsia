// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;

pub fn say_hello() -> Result<&'static str> {
    Ok("Hello, Rust!")
}

fn main() {
    println!("{}", say_hello().unwrap())
}

#[cfg(test)]
mod test {
    use super::*;
    use assert_matches::assert_matches;

    #[test]
    fn test_hello() {
        assert_matches!(say_hello(), Ok("Hello, Rust!"));
    }
}
