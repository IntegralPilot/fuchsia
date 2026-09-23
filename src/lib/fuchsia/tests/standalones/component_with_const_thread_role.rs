// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(target_os = "fuchsia")]
#[fuchsia::main(thread_role = ROLE_NAME_FOR_TEST)]
async fn main() {
    src_lib_fuchsia_testing::assert_logger_registered!();
}
#[cfg(target_os = "fuchsia")]
const ROLE_NAME_FOR_TEST: &str = "role.for.test";

#[cfg(not(target_os = "fuchsia"))]
fn main() {
    #[allow(unused)]
    use fuchsia as _fuchsia;
    #[allow(unused)]
    use src_lib_fuchsia_testing as _src_lib_fuchsia_testing;
}
