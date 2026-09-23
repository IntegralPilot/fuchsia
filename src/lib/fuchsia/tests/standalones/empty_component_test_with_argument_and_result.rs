// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[fuchsia::main]
fn main(opt: src_lib_fuchsia_testing::Options) -> Result<(), anyhow::Error> {
    src_lib_fuchsia_testing::assert_logger_registered!();
    assert_eq!(opt.should_be_false, false);
    Ok(())
}
