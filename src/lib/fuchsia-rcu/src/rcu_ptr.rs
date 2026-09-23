// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::rcu_read_scope::RcuReadScope;
use crate::state_machine::{rcu_assign_pointer, rcu_read_pointer, rcu_replace_pointer};
use std::ops::Deref;
use std::sync::atomic::AtomicPtr;

/// A pointer managed by the RCU state machine.
///
/// This pointer can be read from multiple threads concurrently without blocking.
/// When the pointer is written, reads may continue to see the old value of the pointer
/// for some period of time.
///
/// Callers are responsible managing the lifetime of the object referenced by the pointer. When the
/// the pointer value is written, the caller should typically use `rcu_call` or `rcu_drop` to defer
/// cleanup of the object referenced by the old pointer value until the RCU state machine has made
/// sufficient progress to ensure that no concurrent readers are holding read guards.
#[derive(Debug)]
pub struct RcuPtr<T> {
    ptr: AtomicPtr<T>,
}

impl<T> RcuPtr<T> {
    /// Create a new RCU pointer from a raw pointer.
    pub fn new(ptr: *mut T) -> Self {
        Self { ptr: AtomicPtr::new(ptr) }
    }

    /// Create a new RCU pointer from a reference.
    pub fn from_ref(reference: &T) -> Self {
        Self::new(reference as *const T as *mut T)
    }

    /// Create a null RCU pointer.
    pub fn null() -> Self {
        Self { ptr: AtomicPtr::new(std::ptr::null_mut()) }
    }

    /// Get the value pointed to by the RCU pointer.
    ///
    /// If the RCU pointer is null, this function will panic.
    ///
    /// The object referenced by the RCU pointer will remain valid until the `RcuReadGuard` is
    /// dropped. However, another thread running concurrently might see a different value for the
    /// object.
    pub fn get(&self) -> RcuReadGuard<T> {
        let scope = RcuReadScope::new();
        let ptr = self.read(&scope).as_ptr();
        assert!(!ptr.is_null());
        RcuReadGuard { scope, ptr }
    }

    /// Get the value pointed to by the RCU pointer.
    ///
    /// If the RCU pointer is null, this function will return `None`.
    ///
    /// The object referenced by the RCU pointer will remain valid until the `RcuReadGuard` is
    /// dropped. However, another thread running concurrently might see a different value for the
    /// object.
    pub fn maybe_get(&self) -> Option<RcuReadGuard<T>> {
        let scope = RcuReadScope::new();
        let ptr = self.read(&scope).as_ptr();
        if ptr.is_null() { None } else { Some(RcuReadGuard { scope, ptr }) }
    }

    /// Read the value of the RCU pointer.
    ///
    /// The returned pointer will remain valid until the `RcuReadScope` is dropped. However, another
    /// thread running concurrently might see a different value for the object.
    pub fn read<'a>(&self, scope: &'a RcuReadScope) -> RcuPtrRef<'a, T> {
        let ptr = rcu_read_pointer(&self.ptr);
        // SAFETY: The RCU state machine ensures that the pointer is valid for reads until we drop
        // the RcuReadScope whose lifetime is described by the lifetime parameter.
        unsafe { RcuPtrRef::new(scope, ptr) }
    }

    /// Assign a new value to the RCU pointer.
    ///
    /// Concurrent readers may continue to see the old value of the pointer until the RCU state
    /// machine has made sufficient progress. To wait until all concurrent readers have dropped
    /// their read guards, call `rcu_synchronize()`.
    pub fn assign(&self, ptr: *mut T) {
        rcu_assign_pointer(&self.ptr, ptr);
    }

    /// Assign a new value to the RCU pointer.
    ///
    /// Concurrent readers may continue to see the old value of the pointer until the RCU state
    /// machine has made sufficient progress. To wait until all concurrent readers have dropped
    /// their read guards, call `rcu_synchronize()`.
    pub fn assign_ptr(&self, ptr: RcuPtrRef<'_, T>) {
        self.assign(ptr.as_mut_ptr());
    }

    /// Replace the value of the RCU pointer.
    ///
    /// Concurrent readers may continue to see the old value of the pointer until the RCU state
    /// machine has made sufficient progress. To wait until all concurrent readers have dropped
    /// their read guards, call `rcu_synchronize()`.
    pub fn replace(&self, ptr: *mut T) -> *mut T {
        rcu_replace_pointer(&self.ptr, ptr)
    }

    /// Replace the value of the RCU pointer.
    ///
    /// Concurrent readers may continue to see the old value of the pointer until the RCU state
    /// machine has made sufficient progress. To wait until all concurrent readers have dropped
    /// their read guards, call `rcu_synchronize()`.
    pub fn replace_ptr(&self, ptr: RcuPtrRef<'_, T>) -> *mut T {
        self.replace(ptr.as_mut_ptr())
    }

    /// Swap the value of the RCU pointer with another pointer.
    ///
    /// Concurrent readers may continue to see the old value of the pointer until the RCU state
    /// machine has made sufficient progress. To wait until all concurrent readers have dropped
    /// their read guards, call `rcu_synchronize()`.
    pub fn swap<'a>(&self, scope: &'a RcuReadScope, ptr: *mut T) -> RcuPtrRef<'a, T> {
        let old_ptr = self.replace(ptr);
        // SAFETY: The RCU state machine ensures that the pointer is valid for reads until we drop
        // the RcuReadScope whose lifetime is described by the lifetime parameter.
        unsafe { RcuPtrRef::new(scope, old_ptr) }
    }

    /// Poison the RCU pointer.
    ///
    /// Poisoning the RCU pointer will cause readers to see a dangling pointer. Useful when the
    /// pointer is no longer valid for reading.
    pub fn poison(&self) {
        rcu_assign_pointer(&self.ptr, std::ptr::dangling_mut());
    }
}

/// A read guard for an object managed by the RCU state machine.
///
/// This guard ensures that the object remains valid until the guard is dropped.
pub struct RcuReadGuard<T> {
    /// The scope in which the object is valid.
    scope: RcuReadScope,

    /// The pointer to the object.
    ptr: *const T,
}

impl<T> RcuReadGuard<T> {
    /// Get the scope in which the object is valid.
    pub fn scope(&self) -> &RcuReadScope {
        &self.scope
    }

    /// Get the raw pointer to the object.
    ///
    /// This function returns the raw pointer to the object. The caller is responsible for ensuring
    /// that the pointer is not referenced after the guard is dropped.
    ///
    /// To use the Rust borrow checker to enforce that the object is not accessed after the guard is
    /// dropped, use the `Deref` implementation.
    pub fn as_ptr(&self) -> *const T {
        self.ptr
    }
}

impl<T> Deref for RcuReadGuard<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        // SAFETY: The RCU state machine ensures that the pointer is valid for reads until we drop
        // the RcuReadScope.
        unsafe { &*self.ptr }
    }
}

/// A pointer to an object managed by the RCU state machine.
///
/// This pointer is valid for reading until the `RcuReadScope` is dropped.
pub struct RcuPtrRef<'a, T> {
    /// The pointer to the object.
    ptr: *const T,

    /// The scope in which the pointer is valid.
    _marker: std::marker::PhantomData<&'a T>,
}

impl<'a, T> Clone for RcuPtrRef<'a, T> {
    fn clone(&self) -> Self {
        Self { ptr: self.ptr, _marker: self._marker }
    }
}

impl<'a, T> Copy for RcuPtrRef<'a, T> {}

impl<'a, T> RcuPtrRef<'a, T> {
    /// Create a new `RcuPtrRef` from a pointer and a scope.
    ///
    /// # Safety
    ///
    /// The pointer must be valid for reading until the `RcuReadScope` is dropped.
    pub unsafe fn new(_scope: &'a RcuReadScope, ptr: *const T) -> Self {
        Self { ptr, _marker: std::marker::PhantomData }
    }

    /// Create a null `RcuPtrRef`.
    pub fn null() -> Self {
        Self { ptr: std::ptr::null(), _marker: std::marker::PhantomData }
    }

    /// Check if the pointer is null.
    pub fn is_null(&self) -> bool {
        self.ptr.is_null()
    }

    /// Create a new `RcuPtrRef` from a reference.
    pub fn from_ref(object: &'a T) -> Self {
        Self { ptr: object as *const T, _marker: std::marker::PhantomData }
    }

    /// Get a reference to the object.
    ///
    /// Returns `None` if the pointer is null.
    pub fn as_ref(&self) -> Option<&'a T> {
        if self.is_null() {
            None
        } else {
            // SAFETY: The RCU state machine ensures that the pointer is valid for reads until we
            // drop the RcuReadScope whose lifetime is described by the lifetime parameter.
            Some(unsafe { &*self.ptr })
        }
    }

    /// Get the raw pointer to the object.
    pub fn as_ptr(&self) -> *const T {
        self.ptr
    }

    /// Get the raw mutable pointer to the object.
    pub fn as_mut_ptr(&self) -> *mut T {
        self.ptr as *mut T
    }

    /// Adds a byte offset to the pointer.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the offset is within the bounds of the object and points to a
    /// valid object of type `U`.
    pub unsafe fn add_byte_offset<U>(&self, offset: usize) -> RcuPtrRef<'a, U> {
        let ptr = self.ptr as *const u8;
        // SAFETY: The caller must ensure that the offset is within the bounds of the object and
        // points to a valid object of type `U`.
        RcuPtrRef { ptr: unsafe { ptr.add(offset) } as *const U, _marker: std::marker::PhantomData }
    }

    /// Subtracts a byte offset from the pointer.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the offset is within the bounds of the object and points to a
    /// valid object of type `U`.
    pub unsafe fn sub_byte_offset<U>(&self, offset: usize) -> RcuPtrRef<'a, U> {
        let ptr = self.ptr as *const u8;
        // SAFETY: The caller must ensure that the offset is within the bounds of the object and
        // points to a valid object of type `U`.
        RcuPtrRef { ptr: unsafe { ptr.sub(offset) } as *const U, _marker: std::marker::PhantomData }
    }
}
