// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::kernel::relaxed_atomic::RelaxedAtomicI64;
pub use page_bindings as bindings;

/// Defines the state of a VM page (`vm_page_t`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct VmPageState(pub bindings::vm_page_state);

impl VmPageState {
    pub const COUNT: usize = bindings::vm_page_state::COUNT_ as usize;
    /// Returns the index of `self` as a `usize`.
    #[inline]
    pub const fn index(self) -> usize {
        self.0 as usize
    }

    #[inline]
    pub const fn as_raw(self) -> u8 {
        self.0 as u8
    }
}

/// Counts of VM pages by state.
#[repr(C)]
#[derive(Debug, Default)]
pub struct VmPageCounts {
    /// See comment in `percpu::vm_page_counts` for why we used a `RelaxedAtomic`.
    pub by_state: [RelaxedAtomicI64; VmPageState::COUNT],
}

/// Returns a string description of `state`.
#[inline]
pub const fn page_state_to_string(state: VmPageState) -> &'static str {
    match state.0 {
        bindings::vm_page_state::FREE => "free",
        bindings::vm_page_state::ALLOC => "alloc",
        bindings::vm_page_state::OBJECT => "object",
        bindings::vm_page_state::WIRED => "wired",
        bindings::vm_page_state::HEAP => "heap",
        bindings::vm_page_state::MMU => "mmu",
        bindings::vm_page_state::IPC => "ipc",
        bindings::vm_page_state::CACHE => "cache",
        bindings::vm_page_state::SLAB => "slab",
        bindings::vm_page_state::ZRAM => "zram",
        _ => "unknown",
    }
}

impl core::fmt::Display for VmPageState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(page_state_to_string(*self))
    }
}

const _: () = assert!(core::mem::size_of::<VmPageState>() == 1);
const _: () = assert!(core::mem::align_of::<VmPageState>() == 1);
const _: () = assert!(core::mem::size_of::<VmPageCounts>() == 12 * 8);
const _: () = assert!(core::mem::align_of::<VmPageCounts>() == 8);
const _: () = assert!(core::mem::offset_of!(VmPageCounts, by_state) == 0);
