// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::lock_token::LockToken;
use core::marker::PhantomData;

/// A container cell that safe-guards data of type `T` using type-level lock class tracking.
///
/// `KCell` holds data that can only be safely accessed (read or written) by proving that the
/// corresponding mutual exclusion lock of class `Class` is currently held by the current thread
/// via a `LockToken`.
#[repr(transparent)]
pub struct KCell<T, Class> {
    value: core::cell::UnsafeCell<T>,
    _marker: PhantomData<Class>,
}

unsafe impl<T: Send, Class> Sync for KCell<T, Class> {}
unsafe impl<T: Send, Class> Send for KCell<T, Class> {}

impl<T, Class> KCell<T, Class> {
    /// Creates a new `KCell` containing the specified `value`.
    #[inline]
    pub const fn new(value: T) -> Self {
        Self { value: core::cell::UnsafeCell::new(value), _marker: PhantomData }
    }

    /// Access the guarded value immutably using a shared lock token.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the provided `LockToken` belongs to the specific lock instance
    /// that guards this `KCell` (rather than a different lock of the same lock class `Class`).
    #[inline]
    pub unsafe fn get<'b>(&self, _token: &'b LockToken<'_, Class>) -> &'b T {
        // SAFETY: The caller guarantees that the provided LockToken belongs to the specific lock
        // instance that guards this cell.
        unsafe { &*self.value.get() }
    }

    /// Access the guarded value mutably using a mutable lock token.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the provided `LockToken` belongs to the specific lock instance
    /// that guards this `KCell` (rather than a different lock of the same lock class `Class`).
    #[inline]
    pub unsafe fn get_mut<'b>(&self, _token: &'b mut LockToken<'_, Class>) -> &'b mut T {
        // SAFETY: The caller guarantees that the provided LockToken belongs to the specific lock
        // instance that guards this cell, and the exclusive mutable borrow of the `LockToken`
        // ensures that no other active borrows of the same cell can co-exist.
        unsafe { &mut *self.value.get() }
    }

    /// Returns a mutable raw pointer to the guarded value.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the lock instance guarding this `KCell` is held continuously
    /// while dereferencing or accessing the returned raw pointer.
    #[inline]
    pub unsafe fn as_mut_ptr(&self, _token: &mut LockToken<'_, Class>) -> *mut T {
        self.value.get()
    }

    /// Accesses the inner value mutably by bypassing the locking requirements using unique borrow
    /// ownership.
    #[inline]
    pub fn get_inner_mut(&mut self) -> &mut T {
        self.value.get_mut()
    }

    /// Projects a pinned reference to the cell to a pinned reference to the inner value.
    #[inline]
    pub fn get_pinned_mut(self: core::pin::Pin<&mut Self>) -> core::pin::Pin<&mut T> {
        // SAFETY: `KCell` is `repr(transparent)` and contains `T` at the same address.
        // Pinning is preserved because `KCell` does not move `T`.
        unsafe { self.map_unchecked_mut(|s| s.get_inner_mut()) }
    }

    /// Initializes a `KCell` with a `PinInit` initializer for its inner value.
    pub fn pin_init<E>(init: impl pin_init::PinInit<T, E>) -> impl pin_init::PinInit<Self, E> {
        // SAFETY: KCell<T, Class> has identical memory layout and alignment as T.
        unsafe {
            pin_init::pin_init_from_closure(move |slot: *mut Self| {
                init.__pinned_init(slot.cast::<T>())
            })
        }
    }

    /// Unwraps the cell, returning the inner value.
    #[inline]
    pub fn into_inner(self) -> T {
        self.value.into_inner()
    }
}

impl<T: Default, Class> Default for KCell<T, Class> {
    #[inline]
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T, Class> From<T> for KCell<T, Class> {
    #[inline]
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

impl<T, Class> AsMut<T> for KCell<T, Class> {
    #[inline]
    fn as_mut(&mut self) -> &mut T {
        self.get_inner_mut()
    }
}

impl<T, Class> core::fmt::Debug for KCell<T, Class> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("KCell")
            .field("value", &"<locked>")
            .field("class", &core::any::type_name::<Class>())
            .finish()
    }
}

/// Creates a `PinInit` wrapper for initializing a `KCell` with an inner initializer.
#[inline]
pub fn kcell_init<I, Class>(init: I) -> KCellInit<I, Class> {
    KCellInit(init, PhantomData)
}

/// Initializer for `KCell`.
pub struct KCellInit<I, Class>(I, PhantomData<Class>);

// SAFETY: KCell is repr(transparent) and contains T at the same address.
// Pinning is preserved because KCell doesn't move T.
unsafe impl<T, Class, I, E> pin_init::PinInit<KCell<T, Class>, E> for KCellInit<I, Class>
where
    I: pin_init::PinInit<T, E>,
{
    unsafe fn __pinned_init(self, slot: *mut KCell<T, Class>) -> Result<(), E> {
        // SAFETY: The caller guarantees slot is valid. KCell is repr(transparent)
        // so slot has the same address and layout as T.
        unsafe { self.0.__pinned_init(slot as *mut T) }
    }
}

#[cfg(not(feature = "kernel"))]
#[cfg(test)]
mod tests {
    use super::*;
    use lockdep::LockClass;

    struct MyClass;
    impl LockClass for MyClass {
        const ID: *mut core::ffi::c_void = core::ptr::null_mut();
    }

    #[test]
    fn test_kcell_default() {
        let cell: KCell<u32, MyClass> = KCell::default();
        unsafe {
            let token = LockToken::new();
            assert_eq!(*cell.get(&token), 0);
        }
    }

    #[test]
    fn test_kcell_exclusive_access() {
        let mut cell: KCell<u32, MyClass> = KCell::new(10);
        *cell.get_inner_mut() = 20;
        let reference: &mut u32 = cell.as_mut();
        *reference = 30;
        let value = cell.into_inner();
        assert_eq!(value, 30);
    }

    #[test]
    fn test_kcell_as_mut_ptr() {
        let cell: KCell<u32, MyClass> = KCell::new(100u32);
        unsafe {
            let mut token = LockToken::new();
            let ptr = cell.as_mut_ptr(&mut token);
            assert_eq!(*ptr, 100);
        }
    }

    #[test]
    fn test_kcell_debug() {
        extern crate std;
        let cell: KCell<u32, MyClass> = KCell::new(5);
        let debug_str = std::format!("{:?}", cell);
        assert!(debug_str.contains("KCell"));
        assert!(debug_str.contains("<locked>"));
        assert!(debug_str.contains("MyClass"));
    }

    #[test]
    fn test_kcell_init() {
        let init = unsafe {
            pin_init::pin_init_from_closure(|slot: *mut u32| {
                slot.write(42);
                Ok::<(), core::convert::Infallible>(())
            })
        };
        let cell_init = kcell_init::<_, MyClass>(init);

        pin_init::stack_pin_init!(let cell = cell_init);
        let cell: core::pin::Pin<&mut KCell<u32, MyClass>> = cell;

        unsafe {
            let token = LockToken::<MyClass>::new();
            let val = cell.as_ref().get_ref().get(&token);
            assert_eq!(*val, 42);
        }
    }
}
