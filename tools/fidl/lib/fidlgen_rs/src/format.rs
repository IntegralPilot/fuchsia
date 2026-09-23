// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::fmt::Write as _;

pub fn trim_trailing_whitespace(input: &str) -> String {
    let mut out = String::new();
    for line in input.lines() {
        writeln!(out, "{}", line.trim_end()).unwrap();
    }
    out
}
