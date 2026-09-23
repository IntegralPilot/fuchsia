// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub const VDSO_SIGRETURN_NAME: Option<&'static [u8]> = Some(b"__vdso_rt_sigreturn\0");

pub fn raw_ticks() -> u64 {
    // Returns 0 since the VDSO is not fully implemented for riscv64 yet.
    // This isn't a problem as vvar_data is currently unused in this architecture
    // TODO(https://fxbug.dev/42079789): Implement gettimeofday() in riscv64.
    0
}
