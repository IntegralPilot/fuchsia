// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::rcu_ptr::{RcuPtr, RcuReadGuard};
use crate::rcu_read_scope::RcuReadScope;
use crate::state_machine::rcu_drop;
use std::sync::Arc;

use crate::rcu_droppable::RcuDroppable;

/// An RCU (Read-Copy-Update) wrapper around `Option<Arc<T>>`.
///
/// The Arc can be dereferenced from multiple threads concurrently without blocking.
/// When the Arc is replaced, reads may continue to see the old Arc pointer for some period of time.
#[derive(Debug)]
pub struct RcuOptionArc<T: RcuDroppable + Sync> {
    ptr: RcuPtr<T>,
}

impl<T: RcuDroppable + Sync> RcuOptionArc<T> {
    /// Create a new RCU wrapper around an `Option<Arc<T>>`.
    pub fn new(data: Option<Arc<T>>) -> Self {
        Self { ptr: RcuPtr::new(Self::into_ptr(data)) }
    }

    /// Read the value of the wrapped Arc, if present.
    ///
    /// The object referenced by the RCU Arc will remain valid until the `RcuReadGuard` is dropped.
    /// However, another thread running concurrently might see a different value for the object.
    pub fn read(&self) -> Option<RcuReadGuard<T>> {
        self.ptr.maybe_get()
    }

    /// Returns a reference to the value of the wrapped Arc, if present.
    ///
    /// The object referenced by the RCU Arc will remain valid until the `RcuReadScope` is dropped.
    /// However, another thread running concurrently might see a different value for the object.
    pub fn as_ref<'a>(&self, scope: &'a RcuReadScope) -> Option<&'a T> {
        self.ptr.read(scope).as_ref()
    }

    /// Write a new `Option<Arc<T>>` to the RCU wrapper.
    ///
    /// Concurrent readers may continue to see the old Arc pointer until the RCU state machine has
    /// made sufficient progress to ensure that no concurrent readers are holding read guards.
    pub fn update(&self, data: Option<Arc<T>>) {
        let ptr = Self::into_ptr(data);
        // SAFETY: We can pass `Self::into_ptr` to `Self::replace`.
        unsafe { self.replace(ptr) };
    }

    /// Create a new `Arc<T>` to the object referenced by the wrapped Arc, if present.
    ///
    /// This function returns a new `Option<Arc<T>>` to the object referenced by the wrapped Arc,
    /// potentially increasing the reference count of the object by one.
    pub fn to_option_arc(&self) -> Option<Arc<T>> {
        let guard = self.read()?;
        let ptr = guard.as_ptr();
        // SAFETY: We can make a new Arc to the object by incrementing the strong count and then
        // converting the pointer to an Arc.
        unsafe {
            Arc::increment_strong_count(ptr);
            Some(Arc::from_raw(ptr))
        }
    }

    /// Extract the raw pointer from an `Option<Arc<T>>`.
    ///
    /// The caller is responsible for ensuring that the pointer returned by this function is
    /// eventually converted back into an `Option<Arc<T>>` to balance its reference count.
    fn into_ptr(data: Option<Arc<T>>) -> *mut T {
        match data {
            Some(arc) => Arc::into_raw(arc) as *mut T,
            None => std::ptr::null_mut(),
        }
    }

    /// Replace the pointer in the `RcuOptionArc` with a new pointer.
    ///
    /// # Safety
    ///
    /// The caller must have obtained the pointer from `Self::into_ptr` or from `std::ptr::null_mut`.
    unsafe fn replace(&self, ptr: *mut T) {
        let old_ptr = self.ptr.replace(ptr);
        if !old_ptr.is_null() {
            let arc = unsafe { Arc::from_raw(old_ptr) };
            rcu_drop(arc);
        }
    }
}

impl<T: RcuDroppable + Sync> Drop for RcuOptionArc<T> {
    fn drop(&mut self) {
        // SAFETY: We can pass `std::ptr::null_mut`.
        unsafe { self.replace(std::ptr::null_mut()) };
    }
}

impl<T: RcuDroppable + Sync> Clone for RcuOptionArc<T> {
    fn clone(&self) -> Self {
        Self::new(self.to_option_arc())
    }
}

impl<T: RcuDroppable + Sync> From<Option<Arc<T>>> for RcuOptionArc<T> {
    fn from(data: Option<Arc<T>>) -> Self {
        Self::new(data)
    }
}

impl<T: RcuDroppable + Sync> Default for RcuOptionArc<T> {
    fn default() -> Self {
        Self::new(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state_machine::rcu_run_callbacks;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct DropCounter {
        value: usize,
        drops: Arc<AtomicUsize>,
    }

    // SAFETY: DropCounter only increments an atomic counter on drop.
    unsafe impl RcuDroppable for DropCounter {}

    impl DropCounter {
        pub fn new(value: usize) -> Arc<Self> {
            Arc::new(Self { value, drops: Arc::new(AtomicUsize::new(0)) })
        }
    }

    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn test_rcu_option_arc() {
        let object = DropCounter::new(42);
        let drops = object.drops.clone();

        let arc = RcuOptionArc::from(Some(object));
        assert_eq!(arc.read().unwrap().value, 42);
        assert_eq!(drops.load(Ordering::Relaxed), 0);
        arc.update(Some(DropCounter::new(43)));
        assert_eq!(arc.read().unwrap().value, 43);
        assert_eq!(drops.load(Ordering::Relaxed), 0);

        rcu_run_callbacks();
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_rcu_option_arc_none() {
        let object = DropCounter::new(42);
        let drops = object.drops.clone();

        let arc = RcuOptionArc::from(Some(object));
        assert_eq!(arc.read().unwrap().value, 42);
        assert_eq!(drops.load(Ordering::Relaxed), 0);

        arc.update(None);
        assert!(arc.read().is_none());
        assert_eq!(drops.load(Ordering::Relaxed), 0);

        rcu_run_callbacks();
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_rcu_option_arc_default() {
        let arc: RcuOptionArc<DropCounter> = RcuOptionArc::default();
        assert!(arc.read().is_none());
    }

    #[test]
    fn test_rcu_option_arc_to_option_arc_none() {
        let arc: RcuOptionArc<DropCounter> = RcuOptionArc::default();
        assert!(arc.to_option_arc().is_none());
    }
}
