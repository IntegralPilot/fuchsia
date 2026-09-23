// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::rcu_ptr::RcuPtr;
use crate::rcu_read_scope::RcuReadScope;
use crate::state_machine::rcu_drop;
use crate::subtle::rcu_ptr_upgrade;
use std::sync::Arc;

/// A version of [crate::RcuOptionArc] which does not require `T: RcuDroppable` in exchange for
/// some extra atomic checking on reads.
///
/// RcuArc allows arbitrary types to be used with RCU by separating `Drop`ing the type,
/// which may not be safe to do on the RCU advancer thread, and freeing the allocation for the
/// type, which is safe to do, once an RCU grace period has elapsed.
///
/// We do the former by `Drop`ing the old `Arc<T>` synchronously on the calling thread when we
/// do a replace. We do the latter by first placing a `Weak<T>` in the RCU callback queue to hold
/// the memory alive (but with a potential strong count of zero) until an RCU grace period has
/// elapsed.
///
/// Since the strong count of the memory may drop to zero even while a reader holds an
/// RcuReadScope, readers must verify the memory is safe to read. Readers safely do this by using a
/// Weak::upgrade, which serializes with a compare_exchange loop on the strong count.
#[derive(Debug)]
pub struct RcuArc<T: Send + Sync + 'static> {
    ptr: RcuPtr<T>,
}

impl<T: Send + Sync + 'static> RcuArc<T> {
    /// Create a new RcuArc from an `Option<Arc<T>>`.
    pub fn new(data: impl Into<Option<Arc<T>>>) -> Self {
        Self { ptr: RcuPtr::new(Self::into_ptr(data.into())) }
    }

    /// Read the contents of the RcuArc.
    ///
    /// Returns `None` if the wrapped Arc is `None` or is no longer valid.
    pub fn upgrade(&self) -> Option<Arc<T>> {
        let scope = RcuReadScope::new();
        loop {
            let ptr = self.ptr.read(&scope);
            // If the pointer is null, we're done.
            if ptr.is_null() {
                return None;
            }
            // We could be racing with a call to [Self::update] here. If we get a non null pointer,
            // but fail to upgrade the weak pointer, we simply retry. Because the writer updates
            // `self.ptr` before dropping the old Arc, re-reading `self.ptr` will observe the new
            // pointer (or null).

            // SAFETY: `ptr` was created from an Arc::into_raw() or is null.
            if let Some(arc) = unsafe { rcu_ptr_upgrade(ptr) } {
                return Some(arc);
            }
        }
    }

    /// Write a new `Option<Arc<T>>` to the RcuArc.
    ///
    /// The old `Arc<T>` (if any) is dropped on the caller's thread, running `T::drop()`
    /// synchronously if it was the last strong reference.
    pub fn update(&self, data: impl Into<Option<Arc<T>>>) {
        let ptr = Self::into_ptr(data.into());
        // SAFETY: We pass a pointer obtained from `Self::into_ptr`.
        unsafe { self.replace(ptr) };
    }

    /// Returns `true` if the RCU wrapper currently contains a value.
    pub fn is_some(&self) -> bool {
        self.upgrade().is_some()
    }

    /// Returns `true` if the RCU wrapper does not contain a value.
    pub fn is_none(&self) -> bool {
        self.upgrade().is_none()
    }

    /// Extract the raw pointer from an `Option<Arc<T>>`.
    fn into_ptr(data: Option<Arc<T>>) -> *mut T {
        match data {
            Some(arc) => Arc::into_raw(arc) as *mut T,
            None => std::ptr::null_mut(),
        }
    }

    /// Replace the pointer in the `RcuArc` with a new pointer.
    ///
    /// # Safety
    ///
    /// The caller must have obtained the pointer from `Self::into_ptr` or from `std::ptr::null_mut`.
    unsafe fn replace(&self, ptr: *mut T) {
        let old_ptr = self.ptr.replace(ptr);
        if !old_ptr.is_null() {
            // SAFETY: The caller ensures the pointer is obtained from Self::into_ptr or
            // std::ptr::null_mut.
            let old_arc = unsafe { Arc::from_raw(old_ptr) };
            let weak = Arc::downgrade(&old_arc);
            drop(old_arc);
            rcu_drop(weak);
        }
    }
}

impl<T: Send + Sync + 'static> Drop for RcuArc<T> {
    fn drop(&mut self) {
        // SAFETY: We can pass `std::ptr::null_mut`.
        unsafe { self.replace(std::ptr::null_mut()) };
    }
}

impl<T: Send + Sync + 'static> Clone for RcuArc<T> {
    fn clone(&self) -> Self {
        Self::new(self.upgrade())
    }
}

impl<T: Send + Sync + 'static> From<Option<Arc<T>>> for RcuArc<T> {
    fn from(data: Option<Arc<T>>) -> Self {
        Self::new(data)
    }
}

impl<T: Send + Sync + 'static> From<Arc<T>> for RcuArc<T> {
    fn from(data: Arc<T>) -> Self {
        Self::new(Some(data))
    }
}

impl<T: Send + Sync + 'static> Default for RcuArc<T> {
    fn default() -> Self {
        Self::new(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state_machine::rcu_run_callbacks;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // A struct that intentionally does NOT implement RcuDroppable
    struct DropCounter {
        value: usize,
        drops: Arc<AtomicUsize>,
    }

    impl DropCounter {
        pub fn new(value: usize, drops: Arc<AtomicUsize>) -> Arc<Self> {
            Arc::new(Self { value, drops })
        }
    }

    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn test_arc_update_and_synchronous_drop() {
        // We should see the Drop handler trigger before rcu_synchronize.
        let drops = Arc::new(AtomicUsize::new(0));
        let arc = RcuArc::new(Some(DropCounter::new(42, drops.clone())));

        assert!(arc.is_some());
        assert_eq!(arc.upgrade().unwrap().value, 42);
        assert_eq!(drops.load(Ordering::Relaxed), 0);

        arc.update(Some(DropCounter::new(43, drops.clone())));
        assert_eq!(arc.upgrade().unwrap().value, 43);
        assert_eq!(drops.load(Ordering::Relaxed), 1, "Drop must execute synchronously on update");

        arc.update(None);
        assert!(arc.is_none());
        assert_eq!(drops.load(Ordering::Relaxed), 2, "Drop must execute synchronously on reset");

        rcu_run_callbacks();
        assert_eq!(drops.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn test_arc_default() {
        let arc = RcuArc::<DropCounter>::default();
        assert!(arc.is_none());
        assert!(arc.upgrade().is_none());
    }

    #[test]
    fn test_arc_clone() {
        let drops = Arc::new(AtomicUsize::new(0));
        let arc1 = RcuArc::new(Some(DropCounter::new(100, drops.clone())));
        let arc2 = arc1.clone();

        assert_eq!(arc1.upgrade().unwrap().value, 100);
        assert_eq!(arc2.upgrade().unwrap().value, 100);

        drop(arc1);
        // arc2 still holds a strong reference to the value.
        assert_eq!(drops.load(Ordering::Relaxed), 0);

        drop(arc2);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }
}
