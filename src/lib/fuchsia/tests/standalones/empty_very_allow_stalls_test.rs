// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(target_os = "fuchsia")]
#[fuchsia::test(allow_stalls = true)]
async fn empty_very_allow_stalls_test() {
    src_lib_fuchsia_testing::assert_logger_registered!();
}

#[cfg(not(target_os = "fuchsia"))]
#[allow(unused)]
use fuchsia as _fuchsia;

#[cfg(not(target_os = "fuchsia"))]
#[allow(unused)]
use src_lib_fuchsia_testing as _src_lib_fuchsia_testing;
