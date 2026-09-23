// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::handle::KernelHandle;
use super::msi_allocation::MsiAllocation;
use super::msi_interrupt_dispatcher::MsiInterruptDispatcher;
use crate::vm::vm_object::VmObject;
use core::mem::MaybeUninit;
use fbl::RefPtr;
use zx_types::{zx_rights_t, zx_status_t};

unsafe extern "C" {
    pub(crate) fn cpp_msi_interrupt_dispatcher_create(
        alloc: &RefPtr<MsiAllocation>,
        msi_id: u32,
        vmo: &RefPtr<VmObject>,
        cap_offset: usize,
        options: u32,
        handle_out: *mut MaybeUninit<KernelHandle<MsiInterruptDispatcher>>,
        rights_out: *mut MaybeUninit<zx_rights_t>,
    ) -> zx_status_t;
}
