// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::kernel::types::VAddr;

unsafe extern "C" {
    fn arch_zero_page(va: *mut core::ffi::c_void);
}

/// Arch optimized version of a page zero routine against a page aligned buffer.
/// Usually implemented in or called from assembly.
///
/// # Safety
///
/// Caller knows it is safe to zero the given virtual address.
pub unsafe fn zero_page(va: VAddr) {
    // SAFETY: it is safe to zero the given address.
    unsafe {
        arch_zero_page(core::ptr::with_exposed_provenance_mut(va.0));
    }
}
