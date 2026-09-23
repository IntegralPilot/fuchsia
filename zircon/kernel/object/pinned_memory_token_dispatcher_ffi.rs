// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::bus_transaction_initiator_dispatcher::BusTransactionInitiatorDispatcher;
use super::handle::KernelHandle;
use super::pinned_memory_token_dispatcher::{
    PinnedMemoryTokenDispatcher, PinnedMemoryTokenDispatcherState,
};
use super::pmt::QueryAddressResult;
use core::mem::MaybeUninit;
use fbl::RefPtr;
use zx_types::zx_status_t;

// C++ FFI declarations for PMT and PinnedMemoryTokenDispatcher
unsafe extern "C" {
    pub(crate) fn cpp_pmt_recycle(pmt: *mut super::pmt::Pmt);
    pub(crate) fn cpp_pmt_get_ref_counted(pmt: *mut super::pmt::Pmt) -> *mut ();
    pub(crate) fn cpp_pmt_release_pinned_memory(pmt: *mut super::pmt::Pmt);
    pub(crate) fn cpp_pmt_on_dispatcher_zero_handles(pmt: *mut super::pmt::Pmt);
    pub(crate) fn cpp_pmt_size(pmt: *const super::pmt::Pmt) -> u64;
    pub(crate) fn cpp_pmt_query_address(
        pmt: *mut super::pmt::Pmt,
        query_offset: u64,
        query_size: usize,
        out_result: *mut MaybeUninit<QueryAddressResult>,
    ) -> zx_status_t;

    pub(crate) fn cpp_pinned_memory_token_dispatcher_create(
        bti: *mut BusTransactionInitiatorDispatcher,
        handle_out: *mut MaybeUninit<KernelHandle<PinnedMemoryTokenDispatcher>>,
    ) -> zx_status_t;
}

// Rust FFI trampolines for C++ calling into Rust PinnedMemoryTokenDispatcher

/// Initializes a `PinnedMemoryTokenDispatcherState` in-place.
///
/// # Safety
///
/// `state` must point to valid uninitialized memory for `PinnedMemoryTokenDispatcherState`.
/// `bti_raw` must be a valid raw pointer exported from an
/// `fbl::RefPtr<BusTransactionInitiatorDispatcher>`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_pinned_memory_token_dispatcher_state_init(
    state: *mut PinnedMemoryTokenDispatcherState,
    dispatcher: *const PinnedMemoryTokenDispatcher,
    bti_raw: *mut BusTransactionInitiatorDispatcher,
) {
    // SAFETY: `bti_raw` is a valid raw pointer exported from
    // `fbl::RefPtr<BusTransactionInitiatorDispatcher>`.
    let bti = unsafe { RefPtr::from_raw(bti_raw) };
    let init = PinnedMemoryTokenDispatcherState::init(dispatcher, bti);
    // SAFETY: `state` points to uninitialized memory allocated for
    // `PinnedMemoryTokenDispatcherState`.
    unsafe {
        let _ = pin_init::PinInit::__pinned_init(init, state);
    }
}

/// Called when all handles to the dispatcher are closed.
#[unsafe(no_mangle)]
pub extern "C" fn rust_pinned_memory_token_dispatcher_on_zero_handles(
    disp: &PinnedMemoryTokenDispatcher,
) {
    disp.on_zero_handles();
}
