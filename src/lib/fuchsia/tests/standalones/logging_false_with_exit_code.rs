// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use src_lib_fuchsia_testing as _;
use std::process::ExitCode;

#[fuchsia::main(logging = false)]
fn main() -> ExitCode {
    ExitCode::SUCCESS
}
