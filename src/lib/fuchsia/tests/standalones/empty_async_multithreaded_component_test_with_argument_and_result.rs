// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(target_os = "fuchsia")]
#[fuchsia::main(threads = 2)]
async fn main(opt: src_lib_fuchsia_testing::Options) -> Result<(), anyhow::Error> {
    src_lib_fuchsia_testing::assert_logger_registered!();
    assert_eq!(opt.should_be_false, false);
    Ok(())
}

#[cfg(not(target_os = "fuchsia"))]
fn main() {
    #[allow(unused)]
    use anyhow as _anyhow;
    #[allow(unused)]
    use fuchsia as _fuchsia;
    #[allow(unused)]
    use src_lib_fuchsia_testing as _src_lib_fuchsia_testing;
}
