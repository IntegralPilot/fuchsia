// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{LockPolicy, RawLock};
use pin_init::{PinInit, pin_data, pin_init};

/// A zero-sized raw mutex implementation that does not perform physical locking.
///
/// `PhantomMutex` can be used as the raw lock type in a `KMutex<Class, PhantomMutex>` field
/// to serve as a zero-sized "phantom" lock in structs where physical synchronization is managed
/// by an external shared lock (such as a ref-counted `PeerHolder` for peered dispatchers).
///
/// Using `KMutex<Class, PhantomMutex>` allows structs to integrate with `#[guarded]` and
/// `#[guarded_by(...)]` without allocating memory for a physical mutex in every instance.
///
/// # Example
///
/// ```
/// use ksync::{KCell, KMutex, PhantomMutex, guarded};
///
/// #[guarded]
/// struct RealObject {
///     #[mutex]
///     mu: KMutex,
/// }
///
/// #[guarded]
/// struct PeeredObject {
///     #[mutex(RealObjectMuClass)]
///     mu: KMutex<PhantomMutex>,
///     #[guarded_by(mu)]
///     value: KCell<i32>,
/// }
///
/// // When holding the shared physical mutex (`real_obj.mu`), call `lock_mu` to access fields
/// // protected by the zero-sized `PhantomMutex`:
///
///     ksync::lock!(let guard = phantom_obj.lock_mu(&real_obj.mu));
///     let value = guard.value();
///
/// // If already holding a lock token from another guard of the same class, call `guard_mu`
/// // to obtain an accessor object without re-locking:
///
///     let other_value = other_phantom_obj.guard_mu(guard.token()).value();
/// ```
#[pin_data]
#[derive(Default, Debug, Clone, Copy)]
pub struct PhantomMutex;

pub struct PhantomMutexPolicy;

impl LockPolicy<PhantomMutex> for PhantomMutexPolicy {
    type AcquireArgs = ();
    type GuardState = ();

    #[inline]
    unsafe fn acquire(_lock: &PhantomMutex, _entry: *mut (), _args: ()) -> Self::GuardState {}

    #[inline]
    unsafe fn reacquire(_lock: &PhantomMutex, _entry: *mut (), _state: &mut Self::GuardState) {}

    #[inline]
    unsafe fn release(_lock: &PhantomMutex, _entry: *mut (), _state: Self::GuardState) {}
}

impl RawLock for PhantomMutex {
    type LockEntry = ();
    type DefaultPolicy = PhantomMutexPolicy;

    #[inline]
    unsafe fn init(
        _class_id: *const core::ffi::c_void,
    ) -> impl PinInit<Self, core::convert::Infallible>
    where
        Self: Sized,
    {
        pin_init!(Self {})
    }

    #[inline]
    fn as_mut_ptr(&self) -> *mut core::ffi::c_void {
        core::ptr::NonNull::<Self>::dangling().as_ptr() as *mut _
    }
}
