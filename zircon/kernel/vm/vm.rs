// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::kernel::types::PAddr;

unsafe extern "C" {
    fn cpp_vaddr_to_paddr(va: *const core::ffi::c_void) -> PAddr;
}

// While this method is implemented using FFI clippy cannot observer that the pointer is not
// de-referenced and so for now squash this lint.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
/// Converts a kernel virtual address to a physical address.
pub fn vaddr_to_paddr(va: *const core::ffi::c_void) -> PAddr {
    unsafe { cpp_vaddr_to_paddr(va) }
}
