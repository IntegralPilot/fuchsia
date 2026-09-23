// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![no_std]

#[cfg(test)]
extern crate self as ksync;

pub use kstring::declare_interned_string;
pub use ksync_macro::{declare_singleton_lock, guarded};
pub use pin_init;

/// Locks a mutex.
///
/// Usage:
///   `ksync::lock!(let mut guard = self.lock_mu());`
///   Locks the mutex and binds a mutable pin to `guard`. Useful when you need to mutate
///   guarded fields via `guard.as_mut().fields_mut()`.
///
///   `ksync::lock!(let guard = self.lock_mu());`
///   Locks the mutex and binds an immutable pin to `guard`. Useful for read-only access
///   to guarded fields via `guard.fields()`.
///
///   `ksync::lock!(self.lock_mu());`
///   Locks the mutex and keeps it locked until the end of the scope, without binding the guard.
#[macro_export]
macro_rules! lock {
    (let mut $guard:ident = $lock_init:expr) => {
        $crate::pin_init::stack_pin_init!(let $guard = $lock_init);
        let mut $guard = $guard;
    };
    (let $guard:ident = $lock_init:expr) => {
        $crate::pin_init::stack_pin_init!(let $guard = $lock_init);
    };
    ($lock_init:expr) => {
        $crate::pin_init::stack_pin_init!(let _guard = $lock_init);
    };
}

/// A static C-string tag identifying the source location (`"file:line"`) where a lock is acquired.
///
/// Used with `RawMonitoredSpinlock` to pass critical section names to the kernel lockup detector.
/// Construct via the [`source_tag!`] macro, which is the Rust equivalent of C++ `SOURCE_TAG`.
#[repr(transparent)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SourceTag(&'static core::ffi::CStr);

impl SourceTag {
    /// Creates a `SourceTag` from a static null-terminated byte slice without checking for
    /// interior null bytes.
    ///
    /// # Safety
    ///
    /// `bytes` must be a valid null-terminated C string (ending in `\0` with no interior `\0`
    /// bytes).
    #[inline]
    pub const unsafe fn from_bytes_with_nul_unchecked(bytes: &'static [u8]) -> Self {
        // SAFETY: The caller guarantees `bytes` is a valid null-terminated byte slice.
        Self(unsafe { core::ffi::CStr::from_bytes_with_nul_unchecked(bytes) })
    }

    /// Creates a `SourceTag` from a static `CStr`.
    #[inline]
    pub const fn from_cstr(cstr: &'static core::ffi::CStr) -> Self {
        Self(cstr)
    }

    /// Returns a pointer to the underlying null-terminated C string.
    #[inline]
    pub const fn as_ptr(&self) -> *const core::ffi::c_char {
        self.0.as_ptr()
    }

    /// Returns the underlying `CStr`.
    #[inline]
    pub const fn as_cstr(&self) -> &'static core::ffi::CStr {
        self.0
    }
}

impl Default for SourceTag {
    #[inline]
    fn default() -> Self {
        Self(c"<unknown>")
    }
}

/// Expands to a [`SourceTag`] containing the file path and line number (`"file:line"`) of the
/// call site, equivalent to C++ `SOURCE_TAG`.
#[macro_export]
macro_rules! source_tag {
    () => {{
        // SAFETY: `concat!(file!(), ":", line!(), "\0")` produces a static string literal ending
        // in a single NUL byte and containing no interior NUL bytes.
        const TAG: $crate::SourceTag = unsafe {
            $crate::SourceTag::from_bytes_with_nul_unchecked(
                concat!(file!(), ":", line!(), "\0").as_bytes(),
            )
        };
        TAG
    }};
}

mod kcell;
mod kmutex;
mod konce_cell;
mod lock_token;
mod phantom_mutex;
mod raw_lock;
mod singleton;

#[cfg(not(feature = "kernel"))]
mod raw_userspace_mutex;

#[cfg(feature = "kernel")]
mod raw_kernel_mutex;
#[cfg(feature = "kernel")]
mod raw_spin_lock;

pub use kcell::{KCell, KCellInit, kcell_init};
pub use konce_cell::{KOnceCell, KOnceCellGuard};
#[cfg(any(feature = "kernel", test))]
mod brwlock;

#[cfg(feature = "kernel")]
mod raw_kernel_brwlock;

#[cfg(all(not(feature = "kernel"), test))]
mod raw_userspace_brwlock;

pub use kmutex::{
    AliasedLock, KMutex, KMutexAliasedGuard, KMutexGuard, aliased_lock, aliased_lock_policy,
    aliased_lock_policy_with, aliased_lock_with,
};
pub use lock_token::LockToken;
pub use lockdep::{LOCK_FLAGS_SINGLETON_LOCK, LockClass, LockClassRegistration, LockFlags};
pub use phantom_mutex::PhantomMutex;
pub use raw_lock::{LockPolicy, RawLock};
pub use singleton::SingletonMutex;

#[cfg(not(feature = "kernel"))]
pub use raw_userspace_mutex::RawMutex;

#[cfg(not(feature = "kernel"))]
pub type LockEntryStorage = ();

#[cfg(feature = "kernel")]
pub use raw_spin_lock::{
    InterruptSavedState, IrqSavePolicy, MonitoredSpinlockGuardState, NoIrqSavePolicy,
    RawMonitoredSpinlock, RawSpinlock,
};
#[cfg(feature = "kernel")]
pub type KSpinlock<Class> = KMutex<Class, RawSpinlock>;
#[cfg(feature = "kernel")]
pub type KMonitoredSpinlock<Class> = KMutex<Class, RawMonitoredSpinlock>;
#[cfg(any(feature = "kernel", test))]
pub use brwlock::{BrwLockPi, BrwLockPiReadGuard, BrwLockPiWriteGuard};
#[cfg(feature = "kernel")]
pub use raw_kernel_brwlock::RawBrwLockPi;
#[cfg(feature = "kernel")]
pub use raw_kernel_mutex::{LockEntryStorage, RawCriticalMutex, RawMutex};
#[cfg(all(not(feature = "kernel"), test))]
pub use raw_userspace_brwlock::RawBrwLockPi;
