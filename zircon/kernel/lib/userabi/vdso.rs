// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::vm::vm_object::VmObject;

pub struct VDso;

impl VDso {
    /// Returns true if the given VMO is a vDSO VMO.
    pub fn vmo_is_vdso(vmo: &VmObject) -> bool {
        // SAFETY: `vmo.as_raw()` returns a valid `VmObject` pointer.
        unsafe { cpp_vmo_is_vdso(vmo.as_raw().cast()) }
    }
}

unsafe extern "C" {
    /// # Safety
    ///
    /// `vmo` must point to a valid `VmObject`.
    pub(crate) fn cpp_vmo_is_vdso(vmo: *const VmObject) -> bool;
}
