// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use core::sync::atomic::{AtomicI32, Ordering};

/// Support for intrusive atomic reference counting.
///
/// This supports intrusive atomic reference counting with adoption. This means
/// that a new object starts life at a reference count of 1 and has to be adopted
/// by a type (such as a `fbl::RefPtr`) that begins manipulation of the reference
/// count. If the reference count ever reaches zero, the object's lifetime is
/// over and it should be destroyed (`Release()` returns true if this is the case).
#[repr(C)]
pub struct RefCounted {
    ref_count: AtomicI32,
}

#[cfg(debug_assertions)]
const PRE_ADOPT_SENTINEL: i32 = 0xC0000000u32 as i32;

impl RefCounted {
    /// Create a new RefCounted.
    ///
    /// In debug builds, the initial count is set to a sentinel value to detect
    /// use before adoption. In release builds, it starts at 1.
    pub const fn new() -> Self {
        #[cfg(debug_assertions)]
        let initial_count = PRE_ADOPT_SENTINEL;
        #[cfg(not(debug_assertions))]
        let initial_count = 1;

        RefCounted { ref_count: AtomicI32::new(initial_count) }
    }

    /// Transition the object from unadopted to adopted state.
    pub(crate) fn adopt(&self) {
        #[cfg(debug_assertions)]
        {
            let expected = PRE_ADOPT_SENTINEL;
            let res =
                self.ref_count.compare_exchange(expected, 1, Ordering::AcqRel, Ordering::Acquire);
            assert!(res.is_ok(), "Double adopt or adopt on invalid object");
        }
        #[cfg(not(debug_assertions))]
        {
            self.ref_count.store(1, Ordering::Release);
        }
    }

    /// Increment the reference count.
    pub(crate) fn add_ref(&self) {
        let rc = self.ref_count.fetch_add(1, Ordering::Relaxed);
        assert!(rc >= 1, "AddRef on un-adopted or destroyed object");
    }

    /// Decrement the reference count. Returns true if the object should be destroyed.
    #[must_use = "Release must be checked to determine if the object should be deleted"]
    pub(crate) fn release(&self) -> bool {
        let rc = self.ref_count.fetch_sub(1, Ordering::Release);
        assert!(rc >= 1, "Release on un-adopted or destroyed object");
        if rc == 1 {
            core::sync::atomic::fence(Ordering::Acquire);
            return true;
        }
        false
    }

    /// Current ref count. Only to be used for debugging purposes.
    pub fn ref_count_debug(&self) -> i32 {
        self.ref_count.load(Ordering::Relaxed)
    }
}

/// Trait to be implemented by types that contain a `RefCounted` field.
///
/// Used to locate the `RefCounted` field within a type.
pub trait HasRefCount {
    /// Returns a reference to the contained `RefCounted` field.
    fn ref_count(&self) -> &RefCounted;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new() {
        let rc = RefCounted::new();
        #[cfg(debug_assertions)]
        assert_eq!(rc.ref_count_debug(), PRE_ADOPT_SENTINEL);
        #[cfg(not(debug_assertions))]
        assert_eq!(rc.ref_count_debug(), 1);
    }

    #[test]
    fn test_add_ref() {
        let rc = RefCounted::new();
        rc.adopt();
        rc.add_ref();
        assert_eq!(rc.ref_count_debug(), 2);
    }

    #[test]
    fn test_release() {
        let rc = RefCounted::new();
        rc.adopt();
        assert!(rc.release()); // returns true, count becomes 0
        assert_eq!(rc.ref_count_debug(), 0);
    }

    #[test]
    #[should_panic(expected = "AddRef on un-adopted or destroyed object")]
    fn test_add_ref_panic() {
        let rc = RefCounted::new();
        rc.adopt();
        assert!(rc.release()); // count becomes 0
        rc.add_ref(); // Should panic!
    }

    #[test]
    #[should_panic(expected = "Release on un-adopted or destroyed object")]
    fn test_release_panic() {
        let rc = RefCounted::new();
        rc.adopt();
        assert!(rc.release()); // count becomes 0
        let _ = rc.release(); // Should panic!
    }
}
