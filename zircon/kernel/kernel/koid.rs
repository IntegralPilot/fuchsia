// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use zx_types::zx_koid_t;

unsafe extern "C" {
    fn cpp_koid_generate() -> zx_koid_t;
}

/// Generates a unique 64-bit kernel object ID (KOID).
pub fn generate() -> zx_koid_t {
    unsafe { cpp_koid_generate() }
}
