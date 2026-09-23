// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use core::marker::PhantomData;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// PhysicalPageBorrowingConfig singleton.
static INSTANCE: PhysicalPageBorrowingConfig = PhysicalPageBorrowingConfig::new();

/// Dynamic configuration for physical page borrowing (for pager-backed VMOs).
#[derive(Debug)]
pub struct PhysicalPageBorrowingConfig {
    /// Enable page borrowing when a page is logically moved to the MRU queue.  If true, replace an
    /// accessed non-loaned page with loaned on access.  If false, this is disabled.
    borrowing_on_mru_enabled: AtomicBool,

    /// Enable page loaning.  If false, no page loaning will occur.  If true, decommitting pages
    /// of a contiguous VMO will loan the pages.  This can be dynamically changed, but changes
    /// will only apply to subsequent decommit of contiguous VMO pages.
    loaning_enabled: AtomicBool,

    // Enables copy of page contents, instead of eviction, when a loaned page is committed back
    // to its contiguous owner.
    replace_on_unloan: AtomicBool,

    /// Tracks the number of active unloaning operations in progress. See comment near
    /// is_borrowing_active() for how this is used.
    active_unloans: AtomicU32,
}

impl PhysicalPageBorrowingConfig {
    /// Creates a new `PhysicalPageBorrowingConfig` with default values (all disabled/zero).
    pub const fn new() -> Self {
        Self {
            borrowing_on_mru_enabled: AtomicBool::new(false),
            loaning_enabled: AtomicBool::new(false),
            replace_on_unloan: AtomicBool::new(false),
            active_unloans: AtomicU32::new(0),
        }
    }

    /// Returns a reference to the global singleton instance.
    pub fn get() -> &'static Self {
        &INSTANCE
    }

    /// Sets whether page borrowing is allowed when a page is logically moved to the MRU queue.
    pub fn set_borrowing_on_mru_enabled(&self, enabled: bool) {
        self.borrowing_on_mru_enabled.store(enabled, Ordering::Relaxed);
    }

    /// Returns true if page borrowing is allowed when a page is logically moved to the MRU queue.
    pub fn is_borrowing_on_mru_enabled(&self) -> bool {
        self.borrowing_on_mru_enabled.load(Ordering::Relaxed)
    }

    /// Sets whether page loaning is enabled.
    ///
    /// true - decommitted contiguous VMO pages will decommit+loan the pages.
    /// false - decommit of a contiguous VMO page zeroes instead of decommitting+loaning.
    pub fn set_loaning_enabled(&self, enabled: bool) {
        self.loaning_enabled.store(enabled, Ordering::Relaxed);
    }

    /// Returns true if page loaning is enabled.
    pub fn is_loaning_enabled(&self) -> bool {
        self.loaning_enabled.load(Ordering::Relaxed)
    }

    /// Sets whether loaned pages will be replaced with a new page (copied contents) on unloan.
    ///
    /// true - loaned pages will be replaced with new page with copied contents.
    /// false - loaned pages will be evicted.
    pub fn set_replace_on_unloan_enabled(&self, enabled: bool) {
        self.replace_on_unloan.store(enabled, Ordering::Relaxed);
    }

    /// Returns true if replacement on unloan is enabled.
    pub fn is_replace_on_unloan_enabled(&self) -> bool {
        self.replace_on_unloan.load(Ordering::Relaxed)
    }

    /// Increments the active unloans counter when a contiguous VMO starts unloaning.
    pub fn notify_unloan_started(&self) {
        self.active_unloans.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrements the active unloans counter when a contiguous VMO completes unloaning.
    pub fn notify_unloan_finished(&self) {
        let prev = self
            .active_unloans
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |val| val.checked_sub(1));
        debug_assert!(prev.is_ok());
    }

    /// Returns true if borrowing is permitted at this time.
    ///
    /// To avoid lock contention on the borrowing VMOs' paged_vmo_lock_ between the LRU thread
    /// (sweeping) and the unloaning thread, borrowing is temporarily disabled if any unloans are
    /// currently in progress.
    pub fn is_borrowing_active(&self) -> bool {
        self.is_borrowing_on_mru_enabled() && (self.active_unloans.load(Ordering::Relaxed) == 0)
    }
}

/// RAII guard to enable or disable page loaning for a scope, restoring the previous state on drop.
#[derive(Debug)]
pub struct ScopedLoaningEnabled {
    prev_state: bool,

    // Make ScopedLoaningEnabled !Send + !Sync, since it relies on LIFO ordering
    _marker: PhantomData<*mut ()>,
}

impl ScopedLoaningEnabled {
    /// Creates a new `ScopedLoaningEnabled` guard, enabling or disabling page loaning for its
    /// lifetime.
    pub fn new(enable: bool) -> Self {
        let prev_state = PhysicalPageBorrowingConfig::get().is_loaning_enabled();
        PhysicalPageBorrowingConfig::get().set_loaning_enabled(enable);
        Self { prev_state, _marker: PhantomData }
    }
}

impl Drop for ScopedLoaningEnabled {
    fn drop(&mut self) {
        PhysicalPageBorrowingConfig::get().set_loaning_enabled(self.prev_state);
    }
}

// C FFI exports

#[unsafe(no_mangle)]
pub extern "C" fn rust_ppb_config_set_borrowing_on_mru_enabled(enabled: bool) {
    PhysicalPageBorrowingConfig::get().set_borrowing_on_mru_enabled(enabled);
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_ppb_config_is_borrowing_on_mru_enabled() -> bool {
    PhysicalPageBorrowingConfig::get().is_borrowing_on_mru_enabled()
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_ppb_config_set_loaning_enabled(enabled: bool) {
    PhysicalPageBorrowingConfig::get().set_loaning_enabled(enabled);
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_ppb_config_is_loaning_enabled() -> bool {
    PhysicalPageBorrowingConfig::get().is_loaning_enabled()
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_ppb_config_set_replace_on_unloan_enabled(enabled: bool) {
    PhysicalPageBorrowingConfig::get().set_replace_on_unloan_enabled(enabled);
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_ppb_config_is_replace_on_unloan_enabled() -> bool {
    PhysicalPageBorrowingConfig::get().is_replace_on_unloan_enabled()
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_ppb_config_notify_unloan_started() {
    PhysicalPageBorrowingConfig::get().notify_unloan_started();
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_ppb_config_notify_unloan_finished() {
    PhysicalPageBorrowingConfig::get().notify_unloan_finished();
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_ppb_config_is_borrowing_active() -> bool {
    PhysicalPageBorrowingConfig::get().is_borrowing_active()
}

#[cfg(ktest)]
/// Unit tests for `PhysicalPageBorrowingConfig`.
#[unittest::suite(name = "physical_page_borrowing_config")]
mod tests {
    use super::PhysicalPageBorrowingConfig;
    use unittest::{expect_false, expect_true};

    /// Tests that in-flight unloan operations suppress borrowing activity.
    #[test]
    fn unloan_tracking_suppresses_borrowing() {
        let config = PhysicalPageBorrowingConfig::new();
        config.set_borrowing_on_mru_enabled(true);
        expect_true!(config.is_borrowing_active());

        config.notify_unloan_started();
        expect_false!(config.is_borrowing_active());
        expect_true!(config.is_borrowing_on_mru_enabled());

        config.notify_unloan_started();
        expect_false!(config.is_borrowing_active());

        config.notify_unloan_finished();
        expect_false!(config.is_borrowing_active());

        config.notify_unloan_finished();
        expect_true!(config.is_borrowing_active());
    }
}
