// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::rcu_ptr::RcuPtr;
use crate::rcu_read_scope::RcuReadScope;
use crate::state_machine::rcu_drop;
use std::mem::ManuallyDrop;
use std::sync::{Arc, Weak};

/// An RCU (Read-Copy-Update) wrapper around a [`Weak`] pointer.
///
/// The weak pointer can be read and upgraded from multiple threads concurrently without blocking.
/// When the weak pointer is replaced, reads may continue to see the old weak pointer for some
/// period of time.
#[derive(Debug)]
pub struct RcuWeak<T: Send + Sync + 'static> {
    ptr: RcuPtr<T>,
}

impl<T: Send + Sync + 'static> RcuWeak<T> {
    /// Create a new RCU wrapper around a [`Weak`] pointer.
    pub fn new(data: Weak<T>) -> Self {
        Self { ptr: RcuPtr::new(Self::into_ptr(data)) }
    }

    /// Try to upgrade the wrapped [`Weak`] pointer to an [`Arc`].
    ///
    /// Returns [`None`] if the weak pointer has expired and the referenced object was destroyed or
    /// if the [`RcuWeak`] is in a transient state during drop.
    pub fn upgrade(&self) -> Option<Arc<T>> {
        let scope = RcuReadScope::new();
        let ptr = self.ptr.read(&scope);
        // SAFETY: We can pass `ptr` to `rcu_ptr_upgrade` because it was obtained from
        // `Weak::into_raw`.
        unsafe { crate::subtle::rcu_ptr_upgrade(ptr) }
    }

    /// Create a new [`Weak`] pointer to the object referenced by the wrapped Weak pointer.
    pub fn to_weak(&self) -> Weak<T> {
        self.with_weak(&RcuReadScope::new(), |w| w.clone())
    }

    /// Check if the wrapped weak pointer points to the same allocation as `other`.
    pub fn ptr_eq(&self, other: &Weak<T>) -> bool {
        let scope = RcuReadScope::new();
        let ptr = self.ptr.read(&scope).as_ptr();
        ptr as *const T == other.as_ptr()
    }

    /// Write a new [`Weak`] pointer to the RCU wrapper.
    ///
    /// Concurrent readers may continue to see the old weak pointer until the RCU state machine has
    /// made sufficient progress to ensure that no concurrent readers are holding read guards.
    pub fn update(&self, data: Weak<T>) {
        let ptr = Self::into_ptr(data);
        // SAFETY: We can pass `Self::into_ptr` to `Self::replace`.
        unsafe { self.replace(ptr) };
    }

    /// Gets the number of strong (Arc) pointers pointing to the allocation.
    pub fn strong_count(&self, scope: &RcuReadScope) -> usize {
        self.with_weak(scope, |w| w.strong_count())
    }

    /// Extract the raw pointer from a `Weak` pointer.
    ///
    /// The caller is responsible for ensuring that the pointer returned by this function is
    /// eventually converted back into a `Weak` pointer to balance its weak reference count.
    fn into_ptr(weak: Weak<T>) -> *mut T {
        Weak::into_raw(weak) as *mut T
    }

    /// Replace the Weak pointer in the RCU wrapper with a new pointer.
    ///
    /// # Safety
    ///
    /// The caller must have obtained the pointer from `Self::into_ptr` or from
    /// `std::ptr::null_mut`.
    unsafe fn replace(&self, ptr: *mut T) {
        let old_ptr = self.ptr.replace(ptr);
        if !old_ptr.is_null() {
            let weak = unsafe { Weak::from_raw(old_ptr) };
            rcu_drop(weak);
        }
    }

    /// Executes a closure with a reference to the wrapped [`Weak`] pointer.
    ///
    /// The weak pointer is temporarily reconstructed within an RCU read scope
    /// using [`ManuallyDrop`] to avoid taking ownership of the pointer. If the
    /// internal pointer is null, a reference to an empty [`Weak`] is provided.
    fn with_weak<A>(&self, scope: &RcuReadScope, f: impl FnOnce(&Weak<T>) -> A) -> A {
        let ptr = self.ptr.read(scope).as_ptr();
        if ptr.is_null() {
            f(&Weak::new())
        } else {
            // SAFETY: The RCU state machine ensures that the pointer is valid for reads until the
            // `RcuReadScope` is dropped. Temporarily reconstructs the `Weak` pointer using
            // `ManuallyDrop` and runs `f` on it. This prevents the `Weak` from taking ownership of
            // the wrapped pointer.
            unsafe {
                let weak = ManuallyDrop::new(Weak::from_raw(ptr));
                f(&*weak)
            }
        }
    }
}

impl<T: Send + Sync + 'static> Drop for RcuWeak<T> {
    fn drop(&mut self) {
        // SAFETY: We can pass `std::ptr::null_mut`.
        unsafe { self.replace(std::ptr::null_mut()) };
    }
}

impl<T: Send + Sync + 'static> Clone for RcuWeak<T> {
    fn clone(&self) -> Self {
        Self::new(self.to_weak())
    }
}

impl<T: Send + Sync + 'static> From<Weak<T>> for RcuWeak<T> {
    fn from(weak: Weak<T>) -> Self {
        Self::new(weak)
    }
}

impl<T: Send + Sync + 'static> Default for RcuWeak<T> {
    fn default() -> Self {
        Self::new(Weak::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state_machine::rcu_run_callbacks;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct DropCounter {
        drops: Arc<AtomicUsize>,
    }

    impl DropCounter {
        pub fn new() -> Arc<Self> {
            Arc::new(Self { drops: Arc::new(AtomicUsize::new(0)) })
        }
    }

    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn test_rcu_weak_upgrade() {
        let object = DropCounter::new();
        let drops = object.drops.clone();

        let weak = Arc::downgrade(&object);
        let rcu_weak = RcuWeak::from(weak);

        assert!(rcu_weak.upgrade().is_some());
        assert_eq!(drops.load(Ordering::Relaxed), 0);

        drop(object);
        assert!(rcu_weak.upgrade().is_none());
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_rcu_weak_update() {
        let object1 = DropCounter::new();
        let drops1 = object1.drops.clone();
        let rcu_weak = RcuWeak::from(Arc::downgrade(&object1));

        let object2 = DropCounter::new();
        let drops2 = object2.drops.clone();

        rcu_weak.update(Arc::downgrade(&object2));
        assert!(rcu_weak.upgrade().map_or(false, |obj| Arc::ptr_eq(&obj, &object2)));

        // Old weak should be dropped eventually.
        // This doesn't affect the object1 lifetime, but decrements weak count.
        rcu_run_callbacks();

        drop(object1);
        assert_eq!(drops1.load(Ordering::Relaxed), 1);

        drop(object2);
        assert_eq!(drops2.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_rcu_weak_ptr_eq() {
        let object1 = DropCounter::new();
        let weak1 = Arc::downgrade(&object1);
        let rcu_weak = RcuWeak::from(weak1.clone());

        let object2 = DropCounter::new();
        let weak2 = Arc::downgrade(&object2);

        assert!(rcu_weak.ptr_eq(&weak1));
        assert!(!rcu_weak.ptr_eq(&weak2));

        rcu_weak.update(weak2.clone());
        assert!(!rcu_weak.ptr_eq(&weak1));
        assert!(rcu_weak.ptr_eq(&weak2));
    }

    #[test]
    fn test_rcu_weak_strong_count() {
        let scope = RcuReadScope::new();
        let rcu_weak = RcuWeak::<DropCounter>::default();
        assert_eq!(rcu_weak.strong_count(&scope), 0);

        let object = DropCounter::new();
        rcu_weak.update(Arc::downgrade(&object));
        assert_eq!(rcu_weak.strong_count(&scope), 1);

        drop(object);
        assert_eq!(rcu_weak.strong_count(&scope), 0);
    }
}
