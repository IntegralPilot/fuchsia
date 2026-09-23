// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::fmt::{Error, Write as _};
use zx_libc::sanitizer;

fn main() -> Result<(), Error> {
    sanitizer::log("Hello Rust sanitizer logging!");
    write!(&mut sanitizer::Log::new(), "Rust {} works too!", "formatting")
}
