// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// Tracks the number of populated slots in the VmCowPages' local page list. If the VmCowPages
/// changes a slot to being populated, or vice versa, that should be reported to
/// `ContinuousAttributionTracker`.
///
/// This struct is not thread safe and should only be accessed under the lock of the associated
/// `VmCowPages`.
#[repr(C)]
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ContinuousAttributionTracker {
    /// The number of populated slots in the local page list.
    current_slots: u32,

    /// The greatest number of `current_slots` since the high-water mark value was last reset.
    ///
    /// Always greater than or equal to `current_slots`.
    hwm_slots: u32,
}

// ContinuousAttributionTracker uses a 32-bit count to represent the number of populated slots in
// the local page list. Since a ContinuousAttributionTracker is intended to be stored inline in a
// VmCowPages, reducing its size (by not using a 64-bit count) is a substantial memory saving for
// the system.
zr::static_assert!(core::mem::size_of::<ContinuousAttributionTracker>() == 8);
zr::static_assert!(core::mem::align_of::<ContinuousAttributionTracker>() == 4);

impl ContinuousAttributionTracker {
    /// Constructs a new `ContinuousAttributionTracker` initialized to zero.
    pub const fn new() -> Self {
        Self { current_slots: 0, hwm_slots: 0 }
    }

    /// Move and move assignment zero the `source`.
    pub fn take_from(&mut self, source: &mut Self) {
        *self = core::mem::take(source);
    }

    /// Returns the tracked count of populated slots.
    pub fn fetch_current(&self) -> u32 {
        self.current_slots
    }

    /// Get the greatest number of populated slots since the statistic was last reset.
    ///
    /// Resets the high-water mark.
    pub fn fetch_hwm_and_reset(&mut self) -> u32 {
        debug_assert!(self.hwm_slots >= self.current_slots);
        let ret = self.hwm_slots;
        self.hwm_slots = self.current_slots; // reset
        ret
    }

    /// Increments the count by `by`. This quantity must be strictly positive.
    pub fn increment(&mut self, by: u32) {
        debug_assert!(by > 0);
        let (new_slots, did_overflow) = self.current_slots.overflowing_add(by);
        debug_assert!(!did_overflow);
        self.current_slots = new_slots;
        self.hwm_slots = core::cmp::max(self.hwm_slots, self.current_slots);
    }

    /// Decrements the count by `by`. This quantity must be strictly positive.
    pub fn decrement(&mut self, by: u32) {
        debug_assert!(by > 0);
        let (new_slots, did_overflow) = self.current_slots.overflowing_sub(by);
        // This overflows when there is untracked addition of content: addition of pages,
        // references, or parent content markers to the page list of a VmCowPages that is not
        // paired with updates to this continuous attribution tracker.
        debug_assert!(!did_overflow);
        self.current_slots = new_slots;
        debug_assert!(self.hwm_slots >= self.current_slots);
    }
}

/// The stub continuous attribution tracker. This object stores no data. Intended to be used in
/// place of the regular continuous attribution tracker, unless users opt-in to its existence.
#[repr(C)]
#[derive(Default, Debug, PartialEq, Eq)]
pub struct StubContinuousAttributionTracker;

// The continuous attribution tracker supports an empty "stubbed out" state.
zr::static_assert!(core::mem::size_of::<StubContinuousAttributionTracker>() == 0);
zr::static_assert!(core::mem::align_of::<StubContinuousAttributionTracker>() == 1);

impl StubContinuousAttributionTracker {
    /// Constructs a new `StubContinuousAttributionTracker`.
    pub const fn new() -> Self {
        Self
    }

    /// Unconditionally panics.
    pub fn fetch_current(&self) -> u32 {
        panic!("stub");
    }

    /// Unconditionally panics.
    pub fn fetch_hwm_and_reset(&mut self) -> u32 {
        panic!("stub");
    }

    /// Increments the count by `by`.
    pub fn increment(&mut self, _by: u32) {}

    /// Decrements the count by `by`.
    pub fn decrement(&mut self, _by: u32) {}
}

// C FFI exports

/// # Safety
///
/// `tracker` must be a valid, writable pointer to `ContinuousAttributionTracker` storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_continuous_attribution_tracker_move_init(
    tracker: *mut ContinuousAttributionTracker,
    source: &mut ContinuousAttributionTracker,
) {
    // SAFETY: `tracker` is valid for writes.
    unsafe {
        tracker.write(core::mem::take(source));
    }
}

/// Assigns data from `source` to `tracker`, zeroing `source`.
#[unsafe(no_mangle)]
pub extern "C" fn rust_continuous_attribution_tracker_move_assign(
    tracker: &mut ContinuousAttributionTracker,
    source: &mut ContinuousAttributionTracker,
) {
    tracker.take_from(source);
}

/// Returns the tracked count of populated slots.
#[unsafe(no_mangle)]
pub extern "C" fn rust_continuous_attribution_tracker_fetch_current(
    tracker: &ContinuousAttributionTracker,
) -> u32 {
    tracker.fetch_current()
}

/// Resets the high-water mark and returns the previous value.
#[unsafe(no_mangle)]
pub extern "C" fn rust_continuous_attribution_tracker_fetch_hwm_and_reset(
    tracker: &mut ContinuousAttributionTracker,
) -> u32 {
    tracker.fetch_hwm_and_reset()
}

/// Increments the populated slot count by `by`.
#[unsafe(no_mangle)]
pub extern "C" fn rust_continuous_attribution_tracker_increment(
    tracker: &mut ContinuousAttributionTracker,
    by: u32,
) {
    tracker.increment(by);
}

/// Decrements the populated slot count by `by`.
#[unsafe(no_mangle)]
pub extern "C" fn rust_continuous_attribution_tracker_decrement(
    tracker: &mut ContinuousAttributionTracker,
    by: u32,
) {
    tracker.decrement(by);
}
