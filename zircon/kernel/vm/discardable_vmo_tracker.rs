// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use core::marker::{PhantomData, PhantomPinned};
use core::mem::MaybeUninit;
use discardable_vmo_tracker_bindings as bindings;
use zr::Opaque;

pub use bindings::DiscardableVmoTracker_DiscardablePageCounts as DiscardablePageCounts;

/// Tracks state relevant for discardable VMOs.
#[repr(C)]
pub struct DiscardableVmoTracker {
    raw: Opaque<bindings::DiscardableVmoTracker>,
    phantom: PhantomData<PhantomPinned>,
}

impl DiscardableVmoTracker {
    /// Domain-specific conversion: returns raw pointer for `DiscardableVmoTracker`.
    pub fn as_raw(&self) -> *const bindings::DiscardableVmoTracker {
        self.raw.get()
    }

    /// Domain-specific conversion: constructs a `&DiscardableVmoTracker` from a raw pointer.
    ///
    /// # Safety
    ///
    /// `ptr` must be a valid, non-null pointer to `DiscardableVmoTracker` with lifetime `'a`.
    pub unsafe fn from_raw_ref<'a>(ptr: *const bindings::DiscardableVmoTracker) -> &'a Self {
        let ptr: *const Self = ptr.cast();
        // SAFETY: bindings::DiscardableVmoTracker is layout-compatible with
        // DiscardableVmoTracker.
        unsafe { ptr.as_ref_unchecked() }
    }

    // Returns the total number of pages locked and unlocked across all discardable vmos.
    // Note that this might not be exact and we might miss some vmos, because the
    // |DiscardableVmosLock| is dropped after processing each vmo on the global discardable lists.
    // That is fine since these numbers are only used for accounting.
    pub fn debug_discardable_page_counts() -> DiscardablePageCounts {
        let mut counts = MaybeUninit::uninit();
        // SAFETY: `counts.as_mut_ptr()` is valid for writing `DiscardablePageCounts`.
        unsafe {
            bindings::cpp_discardable_vmo_tracker_debug_discardable_page_counts(
                counts.as_mut_ptr(),
            );
        }
        // SAFETY: `cpp_discardable_vmo_tracker_debug_discardable_page_counts` initialized `counts`.
        unsafe { counts.assume_init() }
    }

    /// Returns whether the VMO is in the reclaimable state.
    pub fn debug_is_reclaimable(&self) -> bool {
        // SAFETY: `self.as_raw()` returns a valid `DiscardableVmoTracker` pointer.
        unsafe { bindings::cpp_discardable_vmo_tracker_debug_is_reclaimable(self.as_raw()) }
    }

    /// Returns whether the VMO is in the unreclaimable state.
    pub fn debug_is_unreclaimable(&self) -> bool {
        // SAFETY: `self.as_raw()` returns a valid `DiscardableVmoTracker` pointer.
        unsafe { bindings::cpp_discardable_vmo_tracker_debug_is_unreclaimable(self.as_raw()) }
    }

    /// Returns whether the VMO is in the discarded state.
    pub fn debug_is_discarded(&self) -> bool {
        // SAFETY: `self.as_raw()` returns a valid `DiscardableVmoTracker` pointer.
        unsafe { bindings::cpp_discardable_vmo_tracker_debug_is_discarded(self.as_raw()) }
    }

    /// Returns the lock count of the discardable VMO.
    pub fn debug_get_lock_count(&self) -> u64 {
        // SAFETY: `self.as_raw()` returns a valid `DiscardableVmoTracker` pointer.
        unsafe { bindings::cpp_discardable_vmo_tracker_debug_get_lock_count(self.as_raw()) }
    }
}
