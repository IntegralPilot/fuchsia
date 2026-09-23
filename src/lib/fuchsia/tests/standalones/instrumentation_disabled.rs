// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(target_os = "fuchsia")]
use anyhow::Error;

#[cfg(target_os = "fuchsia")]
#[fuchsia::main(instrumentation = false)]
async fn main() -> Result<(), Error> {
    Ok(())
}

#[cfg(not(target_os = "fuchsia"))]
fn main() {
    #[allow(unused)]
    use fuchsia as __fuchsia;
}

#[allow(unused)]
use src_lib_fuchsia_testing as __src_lib_fuchsia_testing;
