// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

unsafe extern "C" {
    fn cpp_vmm_set_active_aspace_normal();
    fn cpp_vmm_set_active_aspace_restricted();
}

/// Sets the active address space to the current process's normal mode address space.
pub fn set_active_aspace_normal() {
    // SAFETY: Foreign function call into VMM address space switcher.
    unsafe { cpp_vmm_set_active_aspace_normal() }
}

/// Sets the active address space to the current process's restricted mode address space.
pub fn set_active_aspace_restricted() {
    // SAFETY: Foreign function call into VMM address space switcher.
    unsafe { cpp_vmm_set_active_aspace_restricted() }
}
