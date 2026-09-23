// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::rcu_ptr::{RcuPtr, RcuReadGuard};
use crate::rcu_read_scope::RcuReadScope;
use crate::state_machine::rcu_drop;
use crate::subtle::RcuPtrRef;
use std::sync::Arc;

use crate::rcu_droppable::RcuDroppable;

/// An RCU (Read-Copy-Update) wrapper around an `Arc` for types implementing [`RcuDroppable`].
///
/// The Arc can be dereferenced from multiple threads concurrently without blocking.
/// When the Arc is replaced, reads may continue to see the old Arc pointer for some period of time.
#[derive(Debug)]
pub struct RcuDroppableArc<T: RcuDroppable + Sync> {
    ptr: RcuPtr<T>,
}

impl<T: RcuDroppable + Sync> RcuDroppableArc<T> {
    /// Create a new RCU wrapper around an `Arc`.
    pub fn new(data: Arc<T>) -> Self {
        Self { ptr: RcuPtr::new(Self::into_ptr(data)) }
    }

    /// Read the value of the wrapped Arc.
    ///
    /// The object referenced by the RCU Arc will remain valid until the `RcuReadGuard` is dropped.
    /// However, another thread running concurrently might see a different value for the object.
    pub fn read(&self) -> RcuReadGuard<T> {
        self.ptr.get()
    }

    /// Returns a reference to the value of the wrapped Arc.
    ///
    /// The object referenced by the RCU Arc will remain valid until the `RcuReadScope` is dropped.
    /// However, another thread running concurrently might see a different value for the object.
    pub fn as_ref<'a>(&self, scope: &'a RcuReadScope) -> &'a T {
        self.ptr.read(scope).as_ref().unwrap()
    }

    /// Write a new Arc to the RCU wrapper.
    ///
    /// Concurrent readers may continue to see the old Arc pointer until the RCU state machine has
    /// made sufficient progress to ensure that no concurrent readers are holding read guards.
    pub fn update(&self, data: Arc<T>) {
        let ptr = Self::into_ptr(data);
        // SAFETY: We can pass `Self::into_ptr` to `Self::replace`.
        unsafe { self.replace(ptr) };
    }

    /// Write a new Arc to the RCU wrapper and return a reference to the old value.
    ///
    /// Concurrent readers may continue to see the old Arc pointer until the RCU state machine has
    /// made sufficient progress to ensure that no concurrent readers are holding read guards.
    pub fn update_swap<'a>(&self, scope: &'a RcuReadScope, data: Arc<T>) -> Arc<T> {
        let ptr = Self::into_ptr(data);
        // SAFETY: We can pass `Self::into_ptr` to `Self::replace_swap`.
        unsafe { self.replace_swap(scope, ptr) }
    }

    /// Create a new `Arc` to the object referenced by the wrapped Arc.
    ///
    /// This function returns a new `Arc` to the object referenced by the wrapped Arc,
    /// increasing the reference count of the object by one.
    pub fn to_arc(&self) -> Arc<T> {
        let scope = RcuReadScope::new();
        let ptr = self.ptr.read(&scope);
        // SAFETY: We can pass `self.ptr` to `rcu_ptr_to_arc` because it was obtained from
        // `Arc::into_raw`.
        unsafe { rcu_ptr_to_arc(ptr) }
    }

    /// Extract the raw pointer from an `Arc`.
    ///
    /// The caller is responsible for ensuring that the pointer returned by this function is
    /// eventually converted back into an `Arc` to balance its reference count.
    fn into_ptr(data: Arc<T>) -> *mut T {
        Arc::into_raw(data) as *mut T
    }

    /// Replace the Arc pointer in the RCU wrapper with a new pointer.
    ///
    /// # Safety
    ///
    /// The caller must have obtained the pointer from `Self::into_ptr` or from `std::ptr::null_mut`.
    unsafe fn replace(&self, ptr: *mut T) {
        let old_ptr = self.ptr.replace(ptr);
        let arc = unsafe { Arc::from_raw(old_ptr) };
        rcu_drop(arc);
    }

    /// Replace the Arc pointer in the RCU wrapper with a new pointer and return a reference to the
    /// old value.
    ///
    /// # Safety
    ///
    /// The caller must have obtained the pointer from `Self::into_ptr` or from `std::ptr::null_mut`.
    unsafe fn replace_swap<'a>(&self, scope: &'a RcuReadScope, ptr: *mut T) -> Arc<T> {
        let old_ptr_ref = self.ptr.swap(scope, ptr);
        // SAFETY: `old_ptr_ref` points to an existing `Arc<T>` with a strong reference.
        let rcu_arc = unsafe { Arc::from_raw(old_ptr_ref.as_ptr()) };
        let old_arc = rcu_arc.clone();
        rcu_drop(rcu_arc);
        old_arc
    }
}

impl<T: RcuDroppable + Sync> Drop for RcuDroppableArc<T> {
    fn drop(&mut self) {
        // SAFETY: We can pass `std::ptr::null_mut`.
        unsafe { self.replace(std::ptr::null_mut()) };
    }
}

impl<T: RcuDroppable + Sync> Clone for RcuDroppableArc<T> {
    fn clone(&self) -> Self {
        Self::new(self.to_arc())
    }
}

impl<T: RcuDroppable + Sync> From<Arc<T>> for RcuDroppableArc<T> {
    fn from(data: Arc<T>) -> Self {
        Self::new(data)
    }
}

impl<T: Default + RcuDroppable + Sync> Default for RcuDroppableArc<T> {
    fn default() -> Self {
        Self::new(Arc::new(T::default()))
    }
}

/// Reconstruct an `Arc` from an `RcuPtrRef` by incrementing its strong count.
///
/// # Safety
///
/// The caller must guarantee that the pointer was obtained from `Arc::into_raw()` and that the
/// Arc's strong count is non zero.
///
/// If the underlying Arc<T> strong count may drop to zero, such as by having outstanding Weak
/// pointers, use [rcu_ptr_upgrade] to first check that the pointer is valid to reconstruct.
pub unsafe fn rcu_ptr_to_arc<'a, T>(ptr: RcuPtrRef<'a, T>) -> Arc<T> {
    let raw_ptr = ptr.as_ptr();
    unsafe {
        Arc::increment_strong_count(raw_ptr);
        Arc::from_raw(raw_ptr)
    }
}

/// Reconstruct an `Arc` from an `RcuPtrRef` by upgrading its strong count if it's safe to do so.
///
/// Returns `None` if the strong count of the underlying `Arc` has dropped to zero or if the
/// pointer is null.
///
/// # Safety
///
/// The caller must guarantee that the pointer was obtained from `Arc::into_raw()` or
/// `Weak::into_raw()`.
pub unsafe fn rcu_ptr_upgrade<'a, T>(ptr: RcuPtrRef<'a, T>) -> Option<Arc<T>> {
    let raw_ptr = ptr.as_ptr();
    if raw_ptr.is_null() {
        return None;
    }
    // SAFETY: The caller guarantees `raw_ptr` comes from `Arc::into_raw` or `Weak::into_raw`
    //
    // The allocation is valid for the duration of the RcuReadScope. We pass the pointer through a
    // std::sync::Weak to safely increase the Strong count only if it's valid to do so.
    // ManuallyDrop ensures we don't actually inc/dec the weak count on the Arc.
    unsafe { std::mem::ManuallyDrop::new(std::sync::Weak::from_raw(raw_ptr)).upgrade() }
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
    fn test_rcu_droppable_arc_update() {
        let object = DropCounter::new(42);
        let drops = object.drops.clone();

        let arc = RcuDroppableArc::from(object);
        assert_eq!(arc.read().value, 42);
        assert_eq!(drops.load(Ordering::Relaxed), 0);
        arc.update(DropCounter::new(43));
        assert_eq!(arc.read().value, 43);
        assert_eq!(drops.load(Ordering::Relaxed), 0);

        rcu_run_callbacks();
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_rcu_droppable_arc_update_swap() {
        let object = DropCounter::new(42);
        let drops = object.drops.clone();

        let arc = RcuDroppableArc::from(object);
        {
            let scope = RcuReadScope::new();
            let old_object = arc.update_swap(&scope, DropCounter::new(43));
            assert_eq!(old_object.value, 42);
            assert_eq!(arc.read().value, 43);
            assert_eq!(drops.load(Ordering::Relaxed), 0);
        }

        rcu_run_callbacks();
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_rcu_ptr_upgrade() {
        let scope = RcuReadScope::new();
        let null_ptr: RcuPtrRef<'_, DropCounter> = RcuPtrRef::null();
        assert!(unsafe { rcu_ptr_upgrade(null_ptr) }.is_none());

        let object = DropCounter::new(42);
        let _weak = Arc::downgrade(&object);
        let raw = Arc::into_raw(object);
        let ptr_ref = unsafe { RcuPtrRef::new(&scope, raw) };

        {
            let upgraded = unsafe { rcu_ptr_upgrade(ptr_ref) };
            assert!(upgraded.is_some());
            assert_eq!(upgraded.unwrap().value, 42);
        }

        // Drop the strong Arc while holding a Weak.
        let arc = unsafe { Arc::from_raw(raw) };
        drop(arc);

        // The strong count is 0.
        assert!(unsafe { rcu_ptr_upgrade(ptr_ref) }.is_none());
    }
}
