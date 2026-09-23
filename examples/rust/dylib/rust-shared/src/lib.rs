// Copyright 2022 The Fuchsia Authors.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::hint::black_box;
use std::time::SystemTime;

pub fn rust_get_int(from: u64) -> u64 {
    // Show calling into libstd. Use black_box to prevent optimizations from
    // removing this code.
    let _ = black_box(SystemTime::now());

    black_box(from)
}
