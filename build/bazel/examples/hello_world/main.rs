// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub fn say_hello_world() -> &'static str {
    "Hello, host Rust!"
}

fn main() {
    println!("{}", say_hello_world())
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_hello() {
        assert_eq!("Hello, host Rust!", say_hello_world());
    }
}
