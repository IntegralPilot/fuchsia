// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::rcu_ptr::{RcuPtr, RcuReadGuard};
use crate::rcu_read_scope::RcuReadScope;
use crate::state_machine::rcu_drop;

use crate::rcu_droppable::RcuDroppable;

/// An RCU (Read-Copy-Update) wrapper around `Option<Box>`.
///
/// The Box can be dereferenced from multiple threads concurrently without blocking.
/// When the Box is replaced, reads may continue to see the old Box for some period of time.
#[derive(Debug)]
pub struct RcuOptionBox<T: RcuDroppable + Sync> {
    ptr: RcuPtr<T>,
}

impl<T: RcuDroppable + Sync> RcuOptionBox<T> {
    /// Create a new RCU Cell from a value.
    pub fn new(data: Option<T>) -> Self {
        Self::from(data.map(|data| Box::new(data)))
    }

    /// Read the value of the wrapped Box, if present.
    ///
    /// The object referenced by the Box will remain valid until the `RcuReadGuard` is dropped.
    /// However, another thread running concurrently might see a different object.
    pub fn read(&self) -> Option<RcuReadGuard<T>> {
        self.ptr.maybe_get()
    }

    /// Returns a reference to the value of the wrapped Box, if present.
    ///
    /// The object referenced by the Box will remain valid until the `RcuReadScope` is dropped.
    /// However, another thread running concurrently might see a different object.
    pub fn as_ref<'a>(&self, scope: &'a RcuReadScope) -> Option<&'a T> {
        self.ptr.read(scope).as_ref()
    }

    /// Returns `true` if the RCU wrapper currently contains a value.
    pub fn is_some(&self, scope: &RcuReadScope) -> bool {
        self.as_ref(scope).is_some()
    }

    /// Returns `true` if the RCU wrapper does not contain a value.
    pub fn is_none(&self, scope: &RcuReadScope) -> bool {
        self.as_ref(scope).is_none()
    }

    /// Write a new value to the RCU wrapper.
    ///
    /// Concurrent readers may continue to see the old value until the RCU state machine has
    /// made sufficient progress to ensure that no concurrent readers are holding read guards.
    pub fn update(&self, data: Option<T>) {
        let ptr = data.map(|data| Box::into_raw(Box::new(data))).unwrap_or(std::ptr::null_mut());
        // SAFETY: We can pass `Box::into_raw` to `Self::replace`.
        unsafe { self.replace(ptr) };
    }

    /// Replace the pointer in the RCU wrapper with a new pointer.
    ///
    /// # Safety
    ///
    /// The pointer must have been created by `Box::into_raw` or from `std::ptr::null_mut`.
    unsafe fn replace(&self, ptr: *mut T) {
        let old_ptr = self.ptr.replace(ptr);
        if !old_ptr.is_null() {
            // SAFETY: `old_ptr` was created by `Box::into_raw`.
            let object = unsafe { Box::from_raw(old_ptr) };
            rcu_drop(object);
        }
    }
}

impl<T: Clone + RcuDroppable + Sync> RcuOptionBox<T> {
    /// Returns a clone of the value of the wrapped Box, if present.
    ///
    /// The clone is detached from any RCU read scope.
    pub fn cloned(&self) -> Option<T> {
        self.as_ref(&RcuReadScope::new()).cloned()
    }
}

impl<T: RcuDroppable + Sync> Drop for RcuOptionBox<T> {
    fn drop(&mut self) {
        // SAFETY: We can pass `std::ptr::null_mut` to `Self::replace`.
        unsafe { self.replace(std::ptr::null_mut()) };
    }
}

impl<T: RcuDroppable + Sync> Default for RcuOptionBox<T> {
    fn default() -> Self {
        Self::new(None)
    }
}

impl<T: Clone + RcuDroppable + Sync> Clone for RcuOptionBox<T> {
    fn clone(&self) -> Self {
        let value = self.read();
        Self::new(value.map(|value| value.clone()))
    }
}

impl<T: RcuDroppable + Sync> From<Option<Box<T>>> for RcuOptionBox<T> {
    fn from(value: Option<Box<T>>) -> Self {
        let ptr = value.map(|value| Box::into_raw(value)).unwrap_or(std::ptr::null_mut());
        Self { ptr: RcuPtr::new(ptr) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state_machine::rcu_run_callbacks;
    use std::ops::Deref;

    #[test]
    fn test_rcu_option_cell() {
        let scope = RcuReadScope::new();
        let value = RcuOptionBox::new(Some(42));
        assert_eq!(value.read().unwrap().deref(), &42);
        assert!(value.is_some(&scope));
        assert!(!value.is_none(&scope));

        let value = RcuOptionBox::<i32>::new(None);
        assert!(value.read().is_none());
        assert!(value.is_none(&scope));
        assert!(!value.is_some(&scope));
    }

    #[test]
    fn test_rcu_option_cell_set_deferred() {
        let value = RcuOptionBox::new(Some(42));
        value.update(Some(43));
        assert_eq!(value.read().unwrap().deref(), &43);

        value.update(None);
        assert!(value.read().is_none());

        rcu_run_callbacks();
        assert!(value.read().is_none());
    }

    #[test]
    fn test_rcu_option_cell_drop() {
        let value = RcuOptionBox::new(Some(42));
        drop(value);
    }
}
