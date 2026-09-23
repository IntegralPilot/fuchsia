// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use core::ffi::c_char;

unsafe extern "C" {
    fn cpp_persistent_dlog_write(ptr: *const c_char, len: usize);
}

/// Writes `bytes` to the persistent debuglog, if enabled.
pub fn persistent_dlog_write(bytes: &[u8]) {
    // SAFETY: `bytes.as_ptr()` points to `bytes.len()` initialized bytes in memory.
    unsafe { cpp_persistent_dlog_write(bytes.as_ptr().cast::<c_char>(), bytes.len()) }
}
