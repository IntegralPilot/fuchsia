// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Explicit initialization for objects in static storage.
//!
//! [`LazyInit`] provides explicit, one-time initialization for values in
//! `static` storage that cannot be constructed in a `const` context. Accesses
//! are validated according to the specified [`Policy`] strategy.

#![no_std]

mod check;

pub use check::{AtomicCheck, BasicCheck, CheckedPolicy, Lifecycle, NoCheck, Policy};

use core::cell::UnsafeCell;
use core::convert::Infallible;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::ops;
use core::pin::Pin;

use pin_init::PinInit;

/// A cell providing explicit, one-time initialization for a static value of
/// type `T`.
///
/// `LazyInit` allows storing types in static memory that require runtime
/// initialization. Accesses are validated according to the specified `Policy`
/// strategy.
pub struct LazyInit<T, Check: Policy = BasicCheck> {
    value: UnsafeCell<MaybeUninit<T>>,
    check: Check::State,
    _check: PhantomData<Check>,
}

impl<T, Check: Policy> LazyInit<T, Check> {
    /// Creates an uninitialized instance.
    pub const fn uninit() -> Self {
        Self {
            value: UnsafeCell::new(MaybeUninit::uninit()),
            check: Check::UNINIT_STATE,
            _check: PhantomData,
        }
    }

    /// Moves the given value into the wrapped storage.
    ///
    /// # Safety
    ///
    /// The caller must ensure that initialization is serialized with respect to
    /// any other access to this instance (i.e., that `init` does not race with
    /// any concurrent reads or other initialization calls).
    ///
    /// # Panics
    ///
    /// Panics if the instance has already been initialized or is currently
    /// initializing.
    pub unsafe fn init(&'static self, value: T) {
        let _ = Check::init_with(&self.check, || -> Result<(), Infallible> {
            unsafe {
                (*self.value.get()).write(value);
            }
            Ok(())
        });
    }

    /// Explicitly constructs the wrapped value in-place using a [`PinInit`]
    /// initializer.
    ///
    /// # Safety
    ///
    /// The caller must ensure that initialization is serialized with respect to
    /// any other access to this instance (i.e., that `init_pin` does not race
    /// with any concurrent reads or other initialization calls).
    ///
    /// # Panics
    ///
    /// Panics if the instance has already been initialized or is currently
    /// initializing.
    pub unsafe fn init_pin<E>(self: Pin<&'static Self>, init: impl PinInit<T, E>) -> Result<(), E> {
        let slot = unsafe { (*self.value.get()).as_mut_ptr() };
        // Safety: `slot` is pinned since `self` is.
        Check::init_with(&self.check, || unsafe { init.__pinned_init(slot) })
    }

    /// Returns a reference to the wrapped value without checking
    /// initialization state.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the instance has indeed been initialized.
    #[inline(always)]
    pub const unsafe fn get_unchecked(&self) -> &T {
        unsafe { (*self.value.get()).assume_init_ref() }
    }
}

impl<T, Check: CheckedPolicy> LazyInit<T, Check> {
    /// Returns a reference to the wrapped value.
    ///
    /// Asserts that initialization has already occurred.
    ///
    /// # Panics
    ///
    /// Panics if the instance has not been initialized.
    #[inline(always)]
    pub fn get(&self) -> &T {
        Check::assert_initialized(&self.check);
        // Safety: state just checked as initialized.
        unsafe { self.get_unchecked() }
    }
}

impl<T, Check: CheckedPolicy> ops::Deref for LazyInit<T, Check> {
    type Target = T;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        self.get()
    }
}

// Safety: If T does not have any thread-affinity, then neither does LazyInit.
unsafe impl<T: Send, Check: Policy> Send for LazyInit<T, Check> where Check::State: Send {}

// Safety: The caller of `init` / `init_pin` guarantees that initialization is
// serialized with respect to all other accesses. After initialization, the
// instance is immutable, making concurrent reads data-race free.
unsafe impl<T: Sync, Check: Policy> Sync for LazyInit<T, Check> {}

#[cfg(test)]
mod tests {
    use pin_init::{pin_data, pin_init};

    use super::*;

    #[test]
    fn basic_check() {
        static LAZY: LazyInit<i32, BasicCheck> = LazyInit::uninit();
        unsafe {
            LAZY.init(42);
        }
        assert_eq!(*LAZY.get(), 42);
        assert_eq!(*LAZY, 42);
        assert_eq!(unsafe { *LAZY.get_unchecked() }, 42);
    }

    #[test]
    fn no_check() {
        static LAZY: LazyInit<i32, NoCheck> = LazyInit::uninit();
        unsafe {
            LAZY.init(42);
        }
        assert_eq!(unsafe { *LAZY.get_unchecked() }, 42);
    }

    #[test]
    #[should_panic(expected = "LazyInit: accessed before initialization")]
    fn basic_uninit_get_panics() {
        static LAZY: LazyInit<i32, BasicCheck> = LazyInit::uninit();
        let _ = LAZY.get();
    }

    #[test]
    #[should_panic(expected = "LazyInit: already initialized")]
    fn basic_double_init_panics() {
        static LAZY: LazyInit<i32, BasicCheck> = LazyInit::uninit();
        unsafe {
            LAZY.init(1);
            LAZY.init(2);
        }
    }

    #[test]
    fn atomic_check() {
        static LAZY: LazyInit<&'static str, AtomicCheck> = LazyInit::uninit();
        unsafe {
            LAZY.init("hello world");
        }
        assert_eq!(*LAZY.get(), "hello world");
    }

    #[test]
    #[should_panic(expected = "LazyInit: accessed before initialization")]
    fn atomic_uninit_get_panics() {
        static LAZY: LazyInit<i32, AtomicCheck> = LazyInit::uninit();
        let _ = LAZY.get();
    }

    #[test]
    #[should_panic(expected = "LazyInit: already initialized")]
    fn atomic_double_init_panics() {
        static LAZY: LazyInit<i32, AtomicCheck> = LazyInit::uninit();
        unsafe {
            LAZY.init(1);
            LAZY.init(2);
        }
    }

    #[test]
    fn init_pin() {
        #[pin_data]
        struct PinnedStruct {
            val: u32,
        }

        static LAZY: LazyInit<PinnedStruct, BasicCheck> = LazyInit::uninit();
        let pinned = Pin::static_ref(&LAZY);
        unsafe {
            pinned.init_pin(pin_init!(PinnedStruct { val: 123 })).unwrap();
        }
        assert_eq!(pinned.get().val, 123);
    }
}
