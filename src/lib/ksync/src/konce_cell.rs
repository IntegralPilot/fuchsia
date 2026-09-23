// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::mem::MaybeUninit;

use crate::LockToken;

/// A cell that provides lazy/one-time initialization synchronized by a lock class `Class`.
///
/// `KOnceCell` holds data of type `T` that is uninitialized when created, and can be initialized
/// at most once by presenting proof (`LockToken`) of holding a lock of class `Class`.
///
/// Because access is token-gated by `Class`, initialization is completely free of atomic overhead.
pub struct KOnceCell<T, Class> {
    value: UnsafeCell<MaybeUninit<T>>,
    initialized: UnsafeCell<bool>,
    _marker: PhantomData<Class>,
}

unsafe impl<T: Send, Class> Sync for KOnceCell<T, Class> {}
unsafe impl<T: Send, Class> Send for KOnceCell<T, Class> {}

impl<T, Class> KOnceCell<T, Class> {
    /// Creates a new uninitialized `KOnceCell`.
    #[inline]
    pub const fn new() -> Self {
        Self {
            value: UnsafeCell::new(MaybeUninit::uninit()),
            initialized: UnsafeCell::new(false),
            _marker: PhantomData,
        }
    }

    /// Returns `true` if the cell has been initialized.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the provided `LockToken` belongs to the specific lock
    /// instance that guards this `KOnceCell` (rather than a different lock of the same lock class
    /// `Class`).
    #[inline]
    pub unsafe fn is_initialized(&self, _token: &LockToken<'_, Class>) -> bool {
        // SAFETY: The caller guarantees that the provided LockToken belongs to the specific lock
        // instance that guards this cell.
        unsafe { *self.initialized.get() }
    }

    /// Accesses the initialized value immutably using a shared lock token.
    /// Returns `None` if not yet initialized.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the provided `LockToken` belongs to the specific lock
    /// instance that guards this `KOnceCell` (rather than a different lock of the same lock class
    /// `Class`).
    #[inline]
    pub unsafe fn get<'b>(&self, _token: &'b LockToken<'_, Class>) -> Option<&'b T> {
        // SAFETY: The caller guarantees that the provided LockToken belongs to the specific lock
        // instance that guards this cell.
        if unsafe { *self.initialized.get() } {
            Some(unsafe { (*self.value.get()).assume_init_ref() })
        } else {
            None
        }
    }

    /// Accesses the initialized value mutably using a mutable lock token.
    /// Returns `None` if not yet initialized.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the provided `LockToken` belongs to the specific lock
    /// instance that guards this `KOnceCell` (rather than a different lock of the same lock class
    /// `Class`).
    #[inline]
    pub unsafe fn get_mut<'b>(&self, _token: &'b mut LockToken<'_, Class>) -> Option<&'b mut T> {
        // SAFETY: The caller guarantees that the provided LockToken belongs to the specific lock
        // instance that guards this cell, and the exclusive mutable borrow of the `LockToken`
        // ensures that no other active borrows of the same cell can co-exist.
        if unsafe { *self.initialized.get() } {
            Some(unsafe { (*self.value.get()).assume_init_mut() })
        } else {
            None
        }
    }

    /// Sets the value of the cell if uninitialized, returning `Err(value)` if already initialized.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the provided `LockToken` belongs to the specific lock
    /// instance that guards this `KOnceCell` (rather than a different lock of the same lock class
    /// `Class`).
    #[inline]
    pub unsafe fn set(&self, value: T, _token: &mut LockToken<'_, Class>) -> Result<(), T> {
        // SAFETY: The caller guarantees that the provided LockToken belongs to the specific lock
        // instance that guards this cell, and the exclusive mutable borrow of the `LockToken`
        // ensures no concurrent access.
        unsafe {
            if *self.initialized.get() {
                Err(value)
            } else {
                (*self.value.get()).write(value);
                *self.initialized.get() = true;
                Ok(())
            }
        }
    }

    /// Initializes the cell with the given closure if uninitialized, returning a mutable reference
    /// to the contained value.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the provided `LockToken` belongs to the specific lock
    /// instance that guards this `KOnceCell` (rather than a different lock of the same lock class
    /// `Class`).
    #[inline]
    pub unsafe fn get_or_init<'b>(
        &self,
        f: impl FnOnce() -> T,
        _token: &'b mut LockToken<'_, Class>,
    ) -> &'b mut T {
        // SAFETY: The caller guarantees that the provided LockToken belongs to the specific lock
        // instance that guards this cell, and the exclusive mutable borrow of the `LockToken`
        // ensures no concurrent access.
        unsafe {
            if !*self.initialized.get() {
                (*self.value.get()).write(f());
                *self.initialized.get() = true;
            }
            (*self.value.get()).assume_init_mut()
        }
    }

    /// Initializes the cell with the given fallible closure if uninitialized, returning a mutable
    /// reference to the contained value or the error.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the provided `LockToken` belongs to the specific lock
    /// instance that guards this `KOnceCell` (rather than a different lock of the same lock class
    /// `Class`).
    #[inline]
    pub unsafe fn get_or_try_init<'b, E>(
        &self,
        f: impl FnOnce() -> Result<T, E>,
        _token: &'b mut LockToken<'_, Class>,
    ) -> Result<&'b mut T, E> {
        // SAFETY: The caller guarantees that the provided LockToken belongs to the specific lock
        // instance that guards this cell, and the exclusive mutable borrow of the `LockToken`
        // ensures no concurrent access.
        unsafe {
            if !*self.initialized.get() {
                let val = f()?;
                (*self.value.get()).write(val);
                *self.initialized.get() = true;
            }
            Ok((*self.value.get()).assume_init_mut())
        }
    }

    /// Initializes the cell in-place with `PinInit` if uninitialized, returning a mutable reference
    /// to the contained value or the initialization error.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the provided `LockToken` belongs to the specific lock
    /// instance that guards this `KOnceCell` (rather than a different lock of the same lock class
    /// `Class`).
    #[inline]
    pub unsafe fn get_or_pin_init<'b, E>(
        &self,
        init: impl pin_init::PinInit<T, E>,
        _token: &'b mut LockToken<'_, Class>,
    ) -> Result<&'b mut T, E> {
        // SAFETY: The caller guarantees that the provided LockToken belongs to the specific lock
        // instance that guards this cell, and the exclusive mutable borrow of the `LockToken`
        // ensures no concurrent access.
        unsafe {
            if !*self.initialized.get() {
                init.__pinned_init((*self.value.get()).as_mut_ptr())?;
                *self.initialized.get() = true;
            }
            Ok((*self.value.get()).assume_init_mut())
        }
    }

    /// Accesses the initialized value immutably without checking for a `LockToken`.
    /// Returns `None` if not yet initialized.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the specific lock instance guarding this `KOnceCell` is held.
    #[inline]
    pub unsafe fn get_unchecked(&self) -> Option<&T> {
        // SAFETY: The caller guarantees that the specific lock instance protecting this cell
        // is held.
        if unsafe { *self.initialized.get() } {
            Some(unsafe { (*self.value.get()).assume_init_ref() })
        } else {
            None
        }
    }

    /// Accesses the initialized value mutably without checking for a `LockToken`.
    /// Returns `None` if not yet initialized.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the specific lock instance guarding this `KOnceCell` is held
    /// exclusively without concurrent access.
    #[allow(clippy::mut_from_ref)]
    #[inline]
    pub unsafe fn get_mut_unchecked(&self) -> Option<&mut T> {
        // SAFETY: The caller guarantees that the specific lock instance protecting this cell is
        // held exclusively without concurrent access.
        if unsafe { *self.initialized.get() } {
            Some(unsafe { (*self.value.get()).assume_init_mut() })
        } else {
            None
        }
    }

    /// Initializes the cell with the given closure without requiring a `LockToken`.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the specific lock instance guarding this `KOnceCell` is held
    /// exclusively without concurrent access.
    #[allow(clippy::mut_from_ref)]
    #[inline]
    pub unsafe fn get_or_init_unchecked(&self, f: impl FnOnce() -> T) -> &mut T {
        // SAFETY: The caller guarantees that the specific lock instance protecting this cell is
        // held exclusively without concurrent access.
        unsafe {
            if !*self.initialized.get() {
                (*self.value.get()).write(f());
                *self.initialized.get() = true;
            }
            (*self.value.get()).assume_init_mut()
        }
    }

    /// Initializes the cell with the given fallible closure without requiring a `LockToken`.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the specific lock instance guarding this `KOnceCell` is held
    /// exclusively without concurrent access.
    #[allow(clippy::mut_from_ref)]
    #[inline]
    pub unsafe fn get_or_try_init_unchecked<E>(
        &self,
        f: impl FnOnce() -> Result<T, E>,
    ) -> Result<&mut T, E> {
        // SAFETY: The caller guarantees that the specific lock instance protecting this cell is
        // held exclusively without concurrent access.
        unsafe {
            if !*self.initialized.get() {
                let val = f()?;
                (*self.value.get()).write(val);
                *self.initialized.get() = true;
            }
            Ok((*self.value.get()).assume_init_mut())
        }
    }

    /// Initializes the cell in-place with `PinInit` without requiring a `LockToken`.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the specific lock instance guarding this `KOnceCell` is held
    /// exclusively without concurrent access.
    #[allow(clippy::mut_from_ref)]
    #[inline]
    pub unsafe fn get_or_pin_init_unchecked<E>(
        &self,
        init: impl pin_init::PinInit<T, E>,
    ) -> Result<&mut T, E> {
        // SAFETY: The caller guarantees that the specific lock instance protecting this cell is
        // held exclusively without concurrent access.
        unsafe {
            if !*self.initialized.get() {
                init.__pinned_init((*self.value.get()).as_mut_ptr())?;
                *self.initialized.get() = true;
            }
            Ok((*self.value.get()).assume_init_mut())
        }
    }

    /// Accesses the inner value mutably by bypassing locking requirements using unique borrow
    /// ownership.
    #[inline]
    pub fn get_inner_mut(&mut self) -> Option<&mut T> {
        if *self.initialized.get_mut() {
            Some(unsafe { self.value.get_mut().assume_init_mut() })
        } else {
            None
        }
    }

    /// Unwraps the cell, returning the inner value if initialized.
    #[inline]
    pub fn into_inner(mut self) -> Option<T> {
        if *self.initialized.get_mut() {
            // Read the initialized value and reset `initialized` so `Drop` does not drop it again.
            let val = unsafe { core::ptr::read(self.value.get_mut().as_ptr()) };
            *self.initialized.get_mut() = false;
            Some(val)
        } else {
            None
        }
    }

    /// Returns a safe guard proxy for this cell using an exclusive lock token.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the provided `LockToken` belongs to the specific lock
    /// instance that guards this `KOnceCell` (rather than a different lock of the same lock class
    /// `Class`).
    #[inline]
    pub unsafe fn guard<'a>(
        &'a self,
        token: &'a mut LockToken<'_, Class>,
    ) -> KOnceCellGuard<'a, T, Class> {
        // SAFETY: The caller guarantees that the provided LockToken belongs to the specific lock
        // instance that guards this cell.
        unsafe { KOnceCellGuard::new(self, token) }
    }
}

impl<T, Class> Default for KOnceCell<T, Class> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl<T, Class> core::fmt::Debug for KOnceCell<T, Class> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("KOnceCell")
            .field("value", &"<locked>")
            .field("class", &core::any::type_name::<Class>())
            .finish()
    }
}

impl<T, Class> Drop for KOnceCell<T, Class> {
    fn drop(&mut self) {
        if *self.initialized.get_mut() {
            unsafe {
                self.value.get_mut().assume_init_drop();
            }
        }
    }
}

/// A safe RAII / proxy guard providing synchronized access to a [`KOnceCell`] protected by a lock.
///
/// Created by calling `guard.field_cell()` on a lock guard generated by `#[ksync::guarded]`,
/// or via [`KOnceCell::guard`].
pub struct KOnceCellGuard<'a, T, Class> {
    cell: &'a KOnceCell<T, Class>,
    _marker: core::marker::PhantomData<(&'a mut (), &'a Class)>,
}

impl<'a, T, Class> KOnceCellGuard<'a, T, Class> {
    /// Creates a new `KOnceCellGuard` from a cell reference and an exclusive lock token.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the provided `LockToken` belongs to the specific lock
    /// instance that guards this `KOnceCell` (rather than a different lock of the same lock class
    /// `Class`).
    #[inline]
    pub unsafe fn new(cell: &'a KOnceCell<T, Class>, _token: &'a mut LockToken<'_, Class>) -> Self {
        Self { cell, _marker: core::marker::PhantomData }
    }

    /// Returns `true` if the cell has been initialized.
    #[inline]
    pub fn is_initialized(&self) -> bool {
        unsafe { self.cell.get_unchecked().is_some() }
    }

    /// Returns a reference to the initialized value, or `None` if uninitialized.
    #[inline]
    pub fn get(&self) -> Option<&T> {
        unsafe { self.cell.get_unchecked() }
    }

    /// Returns a mutable reference to the initialized value, or `None` if uninitialized.
    #[inline]
    pub fn get_mut(&mut self) -> Option<&mut T> {
        // SAFETY: The guard guarantees exclusive access while held.
        unsafe { self.cell.get_mut_unchecked() }
    }

    /// Sets the value of the cell if uninitialized, returning `Err(value)` if already initialized.
    #[inline]
    pub fn set(&mut self, value: T) -> Result<(), T> {
        // SAFETY: The guard guarantees exclusive access while held.
        unsafe {
            if self.cell.get_unchecked().is_some() {
                Err(value)
            } else {
                (*self.cell.value.get()).write(value);
                *self.cell.initialized.get() = true;
                Ok(())
            }
        }
    }

    /// Initializes the cell with the given closure if uninitialized, returning a mutable reference
    /// to the contained value.
    #[inline]
    pub fn get_or_init(&mut self, f: impl FnOnce() -> T) -> &mut T {
        // SAFETY: The guard guarantees exclusive access while held.
        unsafe { self.cell.get_or_init_unchecked(f) }
    }

    /// Initializes the cell with the given fallible closure if uninitialized, returning a mutable
    /// reference to the contained value or the error.
    #[inline]
    pub fn get_or_try_init<E>(&mut self, f: impl FnOnce() -> Result<T, E>) -> Result<&mut T, E> {
        // SAFETY: The guard guarantees exclusive access while held.
        unsafe { self.cell.get_or_try_init_unchecked(f) }
    }

    /// Initializes the cell in-place with `PinInit` if uninitialized, returning a mutable reference
    /// to the contained value or the initialization error.
    #[inline]
    pub fn get_or_pin_init<E>(&mut self, init: impl pin_init::PinInit<T, E>) -> Result<&mut T, E> {
        // SAFETY: The guard guarantees exclusive access while held.
        unsafe { self.cell.get_or_pin_init_unchecked(init) }
    }
}

impl<T: core::fmt::Debug, Class> core::fmt::Debug for KOnceCellGuard<'_, T, Class> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("KOnceCellGuard")
            .field("value", &self.get())
            .field("class", &core::any::type_name::<Class>())
            .finish()
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
    fn test_konce_cell_init_and_get() {
        let cell: KOnceCell<u32, MyClass> = KOnceCell::new();
        unsafe {
            let mut token = LockToken::new();
            assert!(!cell.is_initialized(&token));
            assert_eq!(cell.get(&token), None);

            let val = cell.get_or_init(|| 42, &mut token);
            assert_eq!(*val, 42);

            assert!(cell.is_initialized(&token));
            assert_eq!(cell.get(&token), Some(&42));
            assert_eq!(cell.get_mut(&mut token), Some(&mut 42));

            // Subsequent get_or_init does not overwrite
            let val2 = cell.get_or_init(|| 99, &mut token);
            assert_eq!(*val2, 42);
        }
    }

    #[test]
    fn test_konce_cell_set() {
        let cell: KOnceCell<u32, MyClass> = KOnceCell::default();
        unsafe {
            let mut token = LockToken::new();
            assert_eq!(cell.set(100, &mut token), Ok(()));
            assert_eq!(cell.set(200, &mut token), Err(200));
            assert_eq!(cell.get(&token), Some(&100));
        }
    }

    #[test]
    fn test_konce_cell_try_init() {
        let cell: KOnceCell<u32, MyClass> = KOnceCell::new();
        unsafe {
            let mut token = LockToken::new();
            let err_res = cell.get_or_try_init(|| Err::<u32, _>("failed"), &mut token);
            assert_eq!(err_res, Err("failed"));
            assert!(!cell.is_initialized(&token));

            let ok_res = cell.get_or_try_init(|| Ok::<u32, &'static str>(77), &mut token);
            assert_eq!(ok_res, Ok(&mut 77));
            assert!(cell.is_initialized(&token));
        }
    }

    #[test]
    fn test_konce_cell_pin_init() {
        let cell: KOnceCell<u32, MyClass> = KOnceCell::new();
        unsafe {
            let mut token = LockToken::new();
            let init = pin_init::pin_init_from_closure(|slot: *mut u32| {
                slot.write(123);
                Ok::<(), core::convert::Infallible>(())
            });
            let res = cell.get_or_pin_init(init, &mut token);
            assert_eq!(res, Ok(&mut 123));
            assert_eq!(cell.get(&token), Some(&123));
        }
    }

    #[test]
    fn test_konce_cell_drop() {
        use core::sync::atomic::{AtomicUsize, Ordering};
        static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);

        struct DropDetector;
        impl Drop for DropDetector {
            fn drop(&mut self) {
                DROP_COUNT.fetch_add(1, Ordering::Relaxed);
            }
        }

        {
            let cell: KOnceCell<DropDetector, MyClass> = KOnceCell::new();
            unsafe {
                let mut token = LockToken::new();
                let _ = cell.get_or_init(|| DropDetector, &mut token);
            }
            assert_eq!(DROP_COUNT.load(Ordering::Relaxed), 0);
        }
        assert_eq!(DROP_COUNT.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_konce_cell_guard() {
        let cell: KOnceCell<u32, MyClass> = KOnceCell::new();
        unsafe {
            let mut token = LockToken::new();
            let mut guard = cell.guard(&mut token);
            assert!(!guard.is_initialized());
            assert_eq!(guard.get(), None);
            assert_eq!(guard.get_mut(), None);

            let val = guard.get_or_init(|| 123);
            assert_eq!(*val, 123);
            assert!(guard.is_initialized());
            assert_eq!(guard.get(), Some(&123));
            assert_eq!(guard.get_mut(), Some(&mut 123));
            assert_eq!(guard.set(456), Err(456));
        }
    }
}
