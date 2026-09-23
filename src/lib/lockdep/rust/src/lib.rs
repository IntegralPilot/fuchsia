// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![no_std]

/// Flags to specify which rules to apply to a lock class during validation.
pub type LockFlags = u16;

/// Apply only common rules that apply to all locks.
pub const LOCK_FLAGS_NONE: LockFlags = 0;
/// Apply the irq-safety rules in addition to the common rules for all locks.
pub const LOCK_FLAGS_IRQ_SAFE: LockFlags = 1 << 0;
/// Apply the nestable rules in addition to the common rules for all locks.
pub const LOCK_FLAGS_NESTABLE: LockFlags = 1 << 1;
/// Apply the multi-acquire rules in additioon to the common rules for all
/// locks.
pub const LOCK_FLAGS_MULTI_ACQUIRE: LockFlags = 1 << 2;
/// Apply the leaf lock rules in addition to the common rules for all locks.
/// NOTE: Use this flag with caution. See https://fxbug.dev/459856993.
pub const LOCK_FLAGS_LEAF: LockFlags = 1 << 3;
/// Do not report validation errors. This flag prevents recursive validation
/// of locks that are acquired by reporting routines.
pub const LOCK_FLAGS_REPORTING_DISABLED: LockFlags = 1 << 4;
/// There is only one member of this locks class.
pub const LOCK_FLAGS_SINGLETON_LOCK: LockFlags = 1 << 5;
/// Abort the program with an error if a lock is improperly acquired more
/// than once in the same context.
pub const LOCK_FLAGS_RE_ACQUIRE_FATAL: LockFlags = 1 << 6;
/// Do not add this acquisition to the active list. This may be required for
/// locks that are used to protect context switching logic.
pub const LOCK_FLAGS_ACTIVE_LIST_DISABLED: LockFlags = 1 << 7;
/// Do not track this lock.
pub const LOCK_FLAGS_TRACKING_DISABLED: LockFlags = 1 << 8;

// Keep dependency alive even when lockdep is disabled to satisfy unused dependency lints.
use zr as _;

/// Trait implemented by lock class types to expose their dynamic LockClassId pointer.
pub trait LockClass {
    const ID: *mut core::ffi::c_void;
}

#[cfg(any(feature = "lock_dep", feature = "lock_metadata_only"))]
mod enabled {
    core::cfg_select! {
        feature = "lock_dep" => {
            const LOCK_CLASS_STATE_SIZE: usize = 1608;
            const LOCK_CLASS_REGISTRATION_SIZE: usize = 1624;
        }
        feature = "lock_metadata_only" => {
            const LOCK_CLASS_STATE_SIZE: usize = 8;
            const LOCK_CLASS_REGISTRATION_SIZE: usize = 24;
        }
    }

    #[repr(C, align(8))]
    #[derive(Default)]
    struct LockClassStateStorage(zr::OpaqueBytes<LOCK_CLASS_STATE_SIZE>);

    impl LockClassStateStorage {
        const fn uninit() -> Self {
            Self(zr::OpaqueBytes::uninit())
        }
    }

    /// A registration entry for a Rust lock class.
    ///
    /// This struct is registered with the C++ lockdep implementation via a linker section. The
    /// layout of this struct is known to C++.
    #[repr(C)]
    pub struct LockClassRegistration {
        name: *const kstring::interned_string::InternedString,
        flags: u16,
        state_storage: LockClassStateStorage,
    }

    unsafe impl Sync for LockClassRegistration {}
    unsafe impl Send for LockClassRegistration {}

    impl LockClassRegistration {
        pub const fn new(name: &'static ::kstring::interned_string::InternedString) -> Self {
            Self { name, flags: 0, state_storage: LockClassStateStorage::uninit() }
        }

        pub const fn with_flags(
            name: &'static ::kstring::interned_string::InternedString,
            flags: u16,
        ) -> Self {
            Self { name, flags, state_storage: LockClassStateStorage::uninit() }
        }

        #[inline]
        pub const fn get(&self) -> *mut core::ffi::c_void {
            self.state_storage.0.get() as *mut _
        }
    }

    zr::static_assert!(
        core::mem::size_of::<LockClassRegistration>() == LOCK_CLASS_REGISTRATION_SIZE
    );
    zr::static_assert!(core::mem::align_of::<LockClassRegistration>() == 8);
}

#[cfg(any(feature = "lock_dep", feature = "lock_metadata_only"))]
pub use enabled::LockClassRegistration;

#[cfg(not(any(feature = "lock_dep", feature = "lock_metadata_only")))]
mod disabled {
    /// A registration entry for a Rust lock class (stub for disabled lockdep).
    pub struct LockClassRegistration;

    impl LockClassRegistration {
        pub const fn new(_name: &'static ::kstring::interned_string::InternedString) -> Self {
            Self
        }

        pub const fn with_flags(
            _name: &'static ::kstring::interned_string::InternedString,
            _flags: u16,
        ) -> Self {
            Self
        }

        #[inline]
        pub const fn get(&self) -> *mut core::ffi::c_void {
            core::ptr::null_mut()
        }
    }
}

#[cfg(not(any(feature = "lock_dep", feature = "lock_metadata_only")))]
pub use disabled::LockClassRegistration;

/// Asserts that the current thread holds no tracked locks.
///
/// When lock validation is enabled, this checks the current thread's lock state
/// and panics or generates a kernel oops if any tracked locks are held.
/// When lock validation is disabled, this is a zero-cost no-op.
#[inline(always)]
pub fn assert_no_locks_held() {
    #[cfg(feature = "lock_dep")]
    {
        unsafe extern "C" {
            fn lockdep_assert_no_locks_held();
        }
        // SAFETY: `lockdep_assert_no_locks_held` only inspects thread-local lock state
        // and does not violate Rust memory safety or aliasing invariants.
        unsafe { lockdep_assert_no_locks_held() }
    }
}
