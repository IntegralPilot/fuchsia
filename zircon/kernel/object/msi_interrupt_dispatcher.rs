// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use fbl::RefPtr;
use zx_status::Status;
use zx_types::{ZX_OBJ_TYPE_INTERRUPT, zx_rights_t};

use super::KernelHandle;
use super::msi_allocation::MsiAllocation;
use super::msi_interrupt_dispatcher_ffi::cpp_msi_interrupt_dispatcher_create;
use crate::vm::vm_object::VmObject;

crate::object::dispatcher::impl_dispatcher_facade!(
    pub struct MsiInterruptDispatcher,
    ZX_OBJ_TYPE_INTERRUPT
);

impl MsiInterruptDispatcher {
    /// Creates a new `MsiInterruptDispatcher`.
    pub fn create(
        alloc: &RefPtr<MsiAllocation>,
        msi_id: u32,
        vmo: &RefPtr<VmObject>,
        cap_offset: usize,
        options: u32,
    ) -> Result<(KernelHandle<Self>, zx_rights_t), Status> {
        // SAFETY: `alloc` and `vmo` are valid references to `RefPtr`, and
        // `cpp_msi_interrupt_dispatcher_create` initializes `handle_out` and `rights_out`.
        unsafe {
            KernelHandle::create_with_rights(|handle_out, rights_out| {
                cpp_msi_interrupt_dispatcher_create(
                    alloc, msi_id, vmo, cap_offset, options, handle_out, rights_out,
                )
            })
        }
    }
}
