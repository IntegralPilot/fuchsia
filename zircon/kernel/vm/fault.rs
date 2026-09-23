// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

/// page fault flags
pub mod flag {
    use vm_fault_bindings as bindings;

    pub const WRITE: u32 = bindings::VMM_PF_FLAG_WRITE;
    /// Indicates the fault occurred while the CPU was executing in user mode.
    pub const USER: u32 = bindings::VMM_PF_FLAG_USER;
    pub const GUEST: u32 = bindings::VMM_PF_FLAG_GUEST;
    pub const INSTRUCTION: u32 = bindings::VMM_PF_FLAG_INSTRUCTION;
    pub const NOT_PRESENT: u32 = bindings::VMM_PF_FLAG_NOT_PRESENT;
    /// hardware is requesting a fault
    pub const HW_FAULT: u32 = bindings::VMM_PF_FLAG_HW_FAULT;
    /// software fault
    pub const SW_FAULT: u32 = bindings::VMM_PF_FLAG_SW_FAULT;
    pub const ACCESS: u32 = bindings::VMM_PF_FLAG_ACCESS;
    pub const FAULT_MASK: u32 = bindings::VMM_PF_FLAG_FAULT_MASK;
}
