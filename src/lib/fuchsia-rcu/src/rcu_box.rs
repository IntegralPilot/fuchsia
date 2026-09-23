// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::rcu_ptr::{RcuPtr, RcuReadGuard};
use crate::rcu_read_scope::RcuReadScope;
use crate::state_machine::rcu_drop;

use crate::rcu_droppable::RcuDroppable;

/// An RCU (Read-Copy-Update) wrapper around a `Box`.
///
/// The Box can be dereferenced from multiple threads concurrently without blocking.
/// When the Box is replaced, reads may continue to see the old Box pointer for some period of time.
#[derive(Debug)]
pub struct RcuBox<T: RcuDroppable + Sync> {
    ptr: RcuPtr<T>,
}

impl<T: RcuDroppable + Sync> RcuBox<T> {
    /// Create a new RCU wrapped Box from a value.
    pub fn new(data: T) -> Self {
        Self::from(Box::new(data))
    }

    /// Read the value of the wrapped Box.
    ///
    /// The object referenced by the Box will remain valid until the `RcuReadGuard` is dropped.
    /// However, another thread running concurrently might see a different object.
    pub fn read(&self) -> RcuReadGuard<T> {
        self.ptr.get()
    }

    /// Returns a reference to the value of the wrapped Box.
    ///
    /// The object referenced by the Box will remain valid until the `RcuReadScope` is dropped.
    /// However, another thread running concurrently might see a different object.
    pub fn as_ref<'a>(&self, scope: &'a RcuReadScope) -> &'a T {
        self.ptr.read(scope).as_ref().unwrap()
    }

    /// Write a new Boxed value to the RCU wrapper.
    ///
    /// Concurrent readers may continue to see the old boxed object until the RCU state machine has
    /// made sufficient progress to ensure that no concurrent readers are holding read guards.
    pub fn update(&self, data: T) {
        let ptr = Box::into_raw(Box::new(data));
        // SAFETY: We can pass `Box::into_raw` to `Self::replace`.
        unsafe { self.replace(ptr) };
    }

    /// Replace the Box pointer in the RCU wrapper with a new pointer.
    ///
    /// # Safety
    ///
    /// The pointer must have been created by `Box::into_raw` or from `std::ptr::null_mut`.
    unsafe fn replace(&self, ptr: *mut T) {
        let old_ptr = self.ptr.replace(ptr);
        let object = unsafe { Box::from_raw(old_ptr) };
        rcu_drop(object);
    }
}

impl<T: Clone + RcuDroppable + Sync> RcuBox<T> {
    /// Returns a clone of the value of the wrapped Box.
    ///
    /// The clone is detached from any RCU read scope.
    pub fn cloned(&self) -> T {
        self.as_ref(&RcuReadScope::new()).clone()
    }
}

impl<T: RcuDroppable + Sync> Drop for RcuBox<T> {
    fn drop(&mut self) {
        // SAFETY: We can pass `std::ptr::null_mut` to `Self::replace`.
        unsafe { self.replace(std::ptr::null_mut()) };
    }
}

impl<T: Default + RcuDroppable + Sync> Default for RcuBox<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T: Clone + RcuDroppable + Sync> Clone for RcuBox<T> {
    fn clone(&self) -> Self {
        let value = self.read();
        Self::new(value.clone())
    }
}

impl<T: RcuDroppable + Sync> From<Box<T>> for RcuBox<T> {
    fn from(value: Box<T>) -> Self {
        Self { ptr: RcuPtr::new(Box::into_raw(value)) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state_machine::rcu_run_callbacks;
    use std::ops::Deref;

    #[test]
    fn test_rcu_cell() {
        let value = RcuBox::new(42);
        assert_eq!(value.read().deref(), &42);
    }

    #[test]
    fn test_rcu_cell_set_deferred() {
        let value = RcuBox::new(42);
        value.update(43);
        assert_eq!(value.read().deref(), &43);
        rcu_run_callbacks();
    }

    #[test]
    fn test_rcu_cell_drop() {
        let value = RcuBox::new(42);
        drop(value);
    }
}
