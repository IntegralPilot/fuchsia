// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{LockPolicy, LockToken, RawLock, RawMutex};
use core::marker::PhantomData;
use core::pin::Pin;
use lockdep::LockClass;
use pin_init::{PinInit, pin_data, pin_init, pin_init_from_closure, pinned_drop};

#[cfg(feature = "kernel")]
unsafe extern "C" {
    fn cpp_lock_validate_release(entry_storage: *mut core::ffi::c_void);
    fn cpp_lock_validate_acquire(entry_storage: *mut core::ffi::c_void);
}

/// A safe, Zircon-compatible mutual exclusion lock supporting compile-time order validation.
///
/// `KMutex` wraps a platform-specific `RawLock` abstraction. It is pinned in memory to support FFI
/// loop-detector active list registrations safely under the lock class `Class`.
#[repr(transparent)] // Ensure KMutex has the same layout as the underlying RawLock M.
#[pin_data]
pub struct KMutex<Class: LockClass, M: RawLock = RawMutex> {
    #[pin]
    mutex: M,
    _marker: PhantomData<Class>,
}

impl<Class: LockClass, M: RawLock> KMutex<Class, M> {
    /// Create a new KMutex with a pre-initialized raw lock.
    pub const fn new(mutex: M) -> Self {
        Self { mutex, _marker: PhantomData }
    }

    /// Safe dynamic initialization of the validation lock inside pin context.
    pub fn init() -> impl PinInit<Self, core::convert::Infallible> {
        pin_init!(Self {
            mutex <- unsafe { M::init(Self::class_id()) },
            _marker: PhantomData,
        })
    }

    /// Acquires the lock, using the default policy, and registers the active loop node.
    #[inline]
    pub fn lock(&self) -> impl PinInit<KMutexGuard<'_, Class, M>, core::convert::Infallible>
    where
        <M as RawLock>::DefaultPolicy: LockPolicy<M, AcquireArgs = ()>,
    {
        KMutexGuard::new(self)
    }

    /// Acquires the lock with arguments, using the default policy, and registers the active loop
    /// node.
    #[inline]
    pub fn lock_with(
        &self,
        args: <<M as RawLock>::DefaultPolicy as LockPolicy<M>>::AcquireArgs,
    ) -> impl PinInit<KMutexGuard<'_, Class, M>, core::convert::Infallible> {
        KMutexGuard::new_with_args(self, args)
    }

    /// Acquires the lock, using the specified policy, and registers the active loop node.
    #[inline]
    pub fn lock_policy<P: LockPolicy<M, AcquireArgs = ()>>(
        &self,
    ) -> impl PinInit<KMutexGuard<'_, Class, M, P>, core::convert::Infallible> {
        KMutexGuard::new(self)
    }

    /// Acquires the lock with arguments, using the specified policy, and registers the active loop
    /// node.
    #[inline]
    pub fn lock_policy_with<P: LockPolicy<M>>(
        &self,
        args: P::AcquireArgs,
    ) -> impl PinInit<KMutexGuard<'_, Class, M, P>, core::convert::Infallible> {
        KMutexGuard::new_with_args(self, args)
    }

    /// Acquires this lock and aliases it with `alias`, returning a guard that proves ownership
    /// of both `Class` and `AliasClass`.
    #[inline]
    pub fn aliased_lock<'a, AliasClass: LockClass, M2: RawLock>(
        &'a self,
        alias: &'a KMutex<AliasClass, M2>,
    ) -> impl PinInit<KMutexAliasedGuard<'a, Class, AliasClass, M, M2>, core::convert::Infallible>
    where
        <M as RawLock>::DefaultPolicy: LockPolicy<M, AcquireArgs = ()>,
    {
        KMutexAliasedGuard::new(self, alias)
    }

    /// Acquires this lock with arguments and aliases it with `alias`, returning a guard that proves
    /// ownership of both `Class` and `AliasClass`.
    #[inline]
    pub fn aliased_lock_with<'a, AliasClass: LockClass, M2: RawLock>(
        &'a self,
        alias: &'a KMutex<AliasClass, M2>,
        args: <<M as RawLock>::DefaultPolicy as LockPolicy<M>>::AcquireArgs,
    ) -> impl PinInit<KMutexAliasedGuard<'a, Class, AliasClass, M, M2>, core::convert::Infallible>
    {
        KMutexAliasedGuard::new_with_args(self, alias, args)
    }

    /// Acquires this lock with policy `P` and aliases it with `alias`.
    #[inline]
    pub fn aliased_lock_policy<
        'a,
        AliasClass: LockClass,
        M2: RawLock,
        P: LockPolicy<M, AcquireArgs = ()>,
    >(
        &'a self,
        alias: &'a KMutex<AliasClass, M2>,
    ) -> impl PinInit<KMutexAliasedGuard<'a, Class, AliasClass, M, M2, P>, core::convert::Infallible>
    {
        KMutexAliasedGuard::new(self, alias)
    }

    /// Acquires this lock with policy `P` and arguments and aliases it with `alias`.
    #[inline]
    pub fn aliased_lock_policy_with<'a, AliasClass: LockClass, M2: RawLock, P: LockPolicy<M>>(
        &'a self,
        alias: &'a KMutex<AliasClass, M2>,
        args: P::AcquireArgs,
    ) -> impl PinInit<KMutexAliasedGuard<'a, Class, AliasClass, M, M2, P>, core::convert::Infallible>
    {
        KMutexAliasedGuard::new_with_args(self, alias, args)
    }

    /// Returns a reference to the underlying raw lock.
    #[cfg(any(test, ktest))]
    #[inline]
    pub fn raw_mutex(&self) -> &M {
        &self.mutex
    }

    const fn class_id() -> *const core::ffi::c_void {
        if cfg!(feature = "lock_dep") { Class::ID } else { core::ptr::null() }
    }
}

impl<Class: LockClass, M: RawLock> core::fmt::Debug for KMutex<Class, M> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("KMutex").field("class", &core::any::type_name::<Class>()).finish()
    }
}

/// A validation guard representing exclusive lock ownership and active list participation.
///
/// The guard is pinned in memory to ensure that its `lock_entry` pointer remains safe and valid
/// inside the C++ loop detector active thread list.
#[repr(C)]
#[pin_data(PinnedDrop)]
pub struct KMutexGuard<
    'a,
    Class: LockClass,
    M: RawLock = RawMutex,
    P: LockPolicy<M> = <M as RawLock>::DefaultPolicy,
> {
    mutex: &'a KMutex<Class, M>,

    #[pin]
    lock_entry: M::LockEntry,

    state: P::GuardState,

    token: LockToken<'a, Class>,
}

impl<'a, Class: LockClass, M: RawLock, P: LockPolicy<M>> KMutexGuard<'a, Class, M, P> {
    /// Creates a new stack-pinned validation guard initialization block.
    pub fn new(mutex: &'a KMutex<Class, M>) -> impl PinInit<Self, core::convert::Infallible>
    where
        P: LockPolicy<M, AcquireArgs = ()>,
    {
        Self::new_with_args(mutex, ())
    }

    /// Creates a new stack-pinned validation guard initialization block with policy acquire
    /// arguments.
    pub fn new_with_args(
        mutex: &'a KMutex<Class, M>,
        args: P::AcquireArgs,
    ) -> impl PinInit<Self, core::convert::Infallible> {
        // SAFETY: The closure correctly initializes all fields of the allocated `KMutexGuard`
        // and satisfies all safety requirements of `pin_init_from_closure`.
        unsafe {
            pin_init_from_closure(move |this: *mut Self| -> Result<(), core::convert::Infallible> {
                // SAFETY: `this` is a valid pointer to uninitialized memory allocated for
                // `KMutexGuard`.

                let mutex_addr = core::ptr::addr_of_mut!((*this).mutex);
                core::ptr::write(mutex_addr, mutex);

                let entry_addr = core::ptr::addr_of_mut!((*this).lock_entry);
                core::ptr::write(entry_addr, M::LockEntry::default());

                let state = P::acquire(&mutex.mutex, entry_addr, args);

                let state_addr = core::ptr::addr_of_mut!((*this).state);
                core::ptr::write(state_addr, state);

                let token_addr = core::ptr::addr_of_mut!((*this).token);
                core::ptr::write(token_addr, LockToken::new());

                Ok(())
            })
        }
    }

    /// Returns a shared reference to the lock proof `LockToken`.
    #[inline]
    pub fn token(&self) -> &LockToken<'a, Class> {
        &self.token
    }

    /// Returns a mutable reference to the lock proof `LockToken` inside this pinned projection.
    #[inline]
    pub fn token_mut(self: Pin<&mut Self>) -> &mut LockToken<'a, Class> {
        // SAFETY: Modifying the non-pinned raw `token` field does not violate pinning invariants
        // since the token has no drop logic or pointer-location sensitivity.
        let me = unsafe { self.get_unchecked_mut() };
        &mut me.token
    }

    /// Temporarily releases the lock before executing the given callable `f` and then
    /// re-acquires the lock.
    #[inline]
    pub fn call_unlocked<R, F: FnOnce() -> R>(self: Pin<&mut Self>, f: F) -> R {
        // SAFETY: `lock_entry` is pinned on the stack and valid.
        unsafe {
            let me = self.get_unchecked_mut();
            let entry_addr = &mut me.lock_entry as *mut _;
            P::release(&me.mutex.mutex, entry_addr, me.state);
            let result = f();
            P::reacquire(&me.mutex.mutex, entry_addr, &mut me.state);
            result
        }
    }

    /// Calls a closure while temporarily disabling lockdep tracking for the lock held by this
    /// guard.
    #[inline]
    pub fn call_untracked<R, F: FnOnce(&mut LockToken<'a, Class>) -> R>(
        self: Pin<&mut Self>,
        f: F,
    ) -> R {
        #[cfg(feature = "kernel")]
        // SAFETY: `lock_entry` is pinned on the stack and valid.
        unsafe {
            let me = self.get_unchecked_mut();
            let entry_addr = &mut me.lock_entry as *mut _ as *mut core::ffi::c_void;
            cpp_lock_validate_release(entry_addr);
            let result = f(&mut me.token);
            cpp_lock_validate_acquire(entry_addr);
            result
        }
        #[cfg(not(feature = "kernel"))]
        {
            let me = unsafe { self.get_unchecked_mut() };
            f(&mut me.token)
        }
    }
}

#[pinned_drop]
impl<'a, Class: LockClass, M: RawLock, P: LockPolicy<M>> PinnedDrop
    for KMutexGuard<'a, Class, M, P>
{
    // SAFETY: The stack slot `lock_entry` remains valid and pinned on the stack until this drop
    // block completes. Accessing the fields directly to release the raw lock and remove the
    // active list node is safe and correct under the current thread context.
    fn drop(self: Pin<&mut Self>) {
        unsafe {
            let me = self.get_unchecked_mut();
            let entry_addr = &mut me.lock_entry as *mut _;
            P::release(&me.mutex.mutex, entry_addr, me.state);
        }
    }
}

/// Type tag to indicate aliased lock acquisition.
pub struct AliasedLock;

/// Acquires an aliased lock on two `KMutex` instances that reference the same underlying lock.
///
/// Only `lock1` is physically acquired, but the resulting guard holds proof tokens for both
/// `Class1` and `Class2`.
#[inline]
pub fn aliased_lock<'a, Class1: LockClass, Class2: LockClass, M1: RawLock, M2: RawLock>(
    lock1: &'a KMutex<Class1, M1>,
    lock2: &'a KMutex<Class2, M2>,
) -> impl PinInit<KMutexAliasedGuard<'a, Class1, Class2, M1, M2>, core::convert::Infallible>
where
    <M1 as RawLock>::DefaultPolicy: LockPolicy<M1, AcquireArgs = ()>,
{
    KMutexAliasedGuard::new(lock1, lock2)
}

/// Acquires an aliased lock with arguments on two `KMutex` instances that reference the same
/// underlying lock.
#[inline]
pub fn aliased_lock_with<'a, Class1: LockClass, Class2: LockClass, M1: RawLock, M2: RawLock>(
    lock1: &'a KMutex<Class1, M1>,
    lock2: &'a KMutex<Class2, M2>,
    args: <<M1 as RawLock>::DefaultPolicy as LockPolicy<M1>>::AcquireArgs,
) -> impl PinInit<KMutexAliasedGuard<'a, Class1, Class2, M1, M2>, core::convert::Infallible> {
    KMutexAliasedGuard::new_with_args(lock1, lock2, args)
}

/// Acquires an aliased lock with a specific policy on two `KMutex` instances that reference the
/// same underlying lock.
#[inline]
pub fn aliased_lock_policy<
    'a,
    Class1: LockClass,
    Class2: LockClass,
    M1: RawLock,
    M2: RawLock,
    P: LockPolicy<M1, AcquireArgs = ()>,
>(
    lock1: &'a KMutex<Class1, M1>,
    lock2: &'a KMutex<Class2, M2>,
) -> impl PinInit<KMutexAliasedGuard<'a, Class1, Class2, M1, M2, P>, core::convert::Infallible> {
    KMutexAliasedGuard::new(lock1, lock2)
}

/// Acquires an aliased lock with a specific policy and arguments on two `KMutex` instances that
/// reference the same underlying lock.
#[inline]
pub fn aliased_lock_policy_with<
    'a,
    Class1: LockClass,
    Class2: LockClass,
    M1: RawLock,
    M2: RawLock,
    P: LockPolicy<M1>,
>(
    lock1: &'a KMutex<Class1, M1>,
    lock2: &'a KMutex<Class2, M2>,
    args: P::AcquireArgs,
) -> impl PinInit<KMutexAliasedGuard<'a, Class1, Class2, M1, M2, P>, core::convert::Infallible> {
    KMutexAliasedGuard::new_with_args(lock1, lock2, args)
}

/// A validation guard representing ownership of two aliased locks simultaneously.
///
/// Only the first lock is physically acquired, but proof tokens for both lock classes (`Class1` and
/// `Class2`) are provided.
#[pin_data]
pub struct KMutexAliasedGuard<
    'a,
    Class1: LockClass,
    Class2: LockClass,
    M1: RawLock = RawMutex,
    M2: RawLock = M1,
    P: LockPolicy<M1> = <M1 as RawLock>::DefaultPolicy,
> {
    #[pin]
    inner: KMutexGuard<'a, Class1, M1, P>,

    token2: LockToken<'a, Class2>,

    _phantom: PhantomData<&'a KMutex<Class2, M2>>,
}

impl<'a, Class1: LockClass, Class2: LockClass, M1: RawLock, M2: RawLock, P: LockPolicy<M1>>
    KMutexAliasedGuard<'a, Class1, Class2, M1, M2, P>
{
    /// Creates a new stack-pinned aliased validation guard initialization block.
    pub fn new(
        lock1: &'a KMutex<Class1, M1>,
        lock2: &'a KMutex<Class2, M2>,
    ) -> impl PinInit<Self, core::convert::Infallible>
    where
        P: LockPolicy<M1, AcquireArgs = ()>,
    {
        Self::new_with_args(lock1, lock2, ())
    }

    /// Creates a new stack-pinned aliased validation guard initialization block with policy acquire
    /// arguments.
    pub fn new_with_args(
        lock1: &'a KMutex<Class1, M1>,
        _lock2: &'a KMutex<Class2, M2>,
        args: P::AcquireArgs,
    ) -> impl PinInit<Self, core::convert::Infallible> {
        if core::mem::size_of::<M2>() > 0 {
            debug_assert_eq!(
                lock1.mutex.as_mut_ptr(),
                _lock2.mutex.as_mut_ptr(),
                "AliasedLock requires lock1 and lock2 to point to the same physical lock"
            );
        }
        pin_init!(Self {
            inner <- KMutexGuard::new_with_args(lock1, args),
            // SAFETY: `inner` holds the underlying mutex, which is aliased to represent `Class2`.
            token2: unsafe { LockToken::new() },
            _phantom: PhantomData,
        })
    }

    /// Returns shared references to both lock proof tokens `(Class1, Class2)`.
    #[inline]
    pub fn tokens(&self) -> (&LockToken<'a, Class1>, &LockToken<'a, Class2>) {
        (self.inner.token(), &self.token2)
    }

    /// Returns simultaneous mutable references to both proof tokens `(Class1, Class2)`.
    #[inline]
    pub fn tokens_mut(
        self: Pin<&mut Self>,
    ) -> (&mut LockToken<'a, Class1>, &mut LockToken<'a, Class2>) {
        let me = unsafe { self.get_unchecked_mut() };
        let inner_pin = unsafe { Pin::new_unchecked(&mut me.inner) };
        (inner_pin.token_mut(), &mut me.token2)
    }

    /// Returns a shared reference to the primary lock proof token (`Class1`).
    #[inline]
    pub fn token(&self) -> &LockToken<'a, Class1> {
        self.inner.token()
    }

    /// Returns a mutable reference to the primary lock proof token (`Class1`).
    #[inline]
    pub fn token_mut(self: Pin<&mut Self>) -> &mut LockToken<'a, Class1> {
        let me = unsafe { self.get_unchecked_mut() };
        let inner_pin = unsafe { Pin::new_unchecked(&mut me.inner) };
        inner_pin.token_mut()
    }

    /// Returns a pinned mutable reference to the primary inner `KMutexGuard`.
    #[inline]
    pub fn inner_guard(self: Pin<&mut Self>) -> Pin<&mut KMutexGuard<'a, Class1, M1, P>> {
        let me = unsafe { self.get_unchecked_mut() };
        unsafe { Pin::new_unchecked(&mut me.inner) }
    }

    /// Temporarily releases the lock before executing the given callable `f` and then
    /// re-acquires the lock.
    #[inline]
    pub fn call_unlocked<R, F: FnOnce() -> R>(self: Pin<&mut Self>, f: F) -> R {
        let me = unsafe { self.get_unchecked_mut() };
        let inner_pin = unsafe { Pin::new_unchecked(&mut me.inner) };
        inner_pin.call_unlocked(f)
    }

    /// Calls a closure while temporarily disabling lockdep tracking for the lock held by this
    /// guard.
    #[inline]
    pub fn call_untracked<
        R,
        F: FnOnce(&mut LockToken<'a, Class1>, &mut LockToken<'a, Class2>) -> R,
    >(
        self: Pin<&mut Self>,
        f: F,
    ) -> R {
        let me = unsafe { self.get_unchecked_mut() };
        let token2 = &mut me.token2;
        let inner_pin = unsafe { Pin::new_unchecked(&mut me.inner) };
        inner_pin.call_untracked(|token1| f(token1, token2))
    }
}

#[cfg(not(feature = "kernel"))]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{KCell, guarded};
    use lockdep::LockClass;
    use pin_init::{pin_init, stack_pin_init};

    struct MyClass;
    impl LockClass for MyClass {
        const ID: *mut core::ffi::c_void = core::ptr::null_mut();
    }

    #[pin_init::pin_data]
    struct MyStruct {
        #[pin]
        mu: KMutex<MyClass>,
        data1: KCell<u32, MyClass>,
        data2: KCell<i32, MyClass>,
    }

    #[test]
    fn test_basic_token_access() {
        stack_pin_init!(let s = pin_init!(MyStruct {
            mu <- KMutex::init(),
            data1: KCell::new(10),
            data2: KCell::new(-5),
        }));

        lock!(let mut guard = s.mu.lock());

        unsafe {
            assert_eq!(*s.data1.get(guard.token()), 10);
            assert_eq!(*s.data2.get(guard.token()), -5);
        }
        unsafe {
            let token_mut = guard.as_mut().token_mut();
            *s.data1.get_mut(token_mut) = 20;
            assert_eq!(*s.data1.get(guard.token()), 20);
        }
    }

    #[guarded]
    struct MyGuardedStruct {
        #[mutex]
        mu: KMutex,
        #[guarded_by(mu)]
        data1: u32,
        #[guarded_by(mu)]
        data2: i32,
    }

    #[test]
    fn test_macro_guarded() {
        stack_pin_init!(let s = pin_init!(MyGuardedStruct {
            mu <- KMutex::init(),
            data1: 100.into(),
            data2: (-50).into(),
        }));

        {
            lock!(let mut guard = s.lock_mu());

            // Safe individual field access
            assert_eq!(*guard.data1(), 100);
            assert_eq!(*guard.data2(), -50);

            *guard.as_mut().data1_mut() = 200;
            assert_eq!(*guard.data1(), 200);

            // Safe disjoint/split access
            let fields = guard.as_mut().fields_mut();
            *fields.data1 += 50;
            *fields.data2 += 50;
        }

        // Verify fields
        lock!(let guard = s.lock_mu());
        assert_eq!(*guard.data1(), 250);
        assert_eq!(*guard.data2(), 0);
    }

    #[test]
    fn test_kmutex_init() {
        stack_pin_init!(let mu = KMutex::<MyClass>::init());
        lock!(mu.lock());
    }

    #[test]
    fn test_kmutex_debug() {
        extern crate std;
        stack_pin_init!(let mu = KMutex::<MyClass>::init());
        let debug_str = std::format!("{:?}", mu);
        assert!(debug_str.contains("KMutex"));
    }

    #[guarded]
    struct MyMultiGuardedStruct {
        #[mutex]
        mu1: KMutex,
        #[mutex]
        mu2: KMutex,
        #[guarded_by(mu1)]
        data1: u32,
        #[guarded_by(mu2)]
        data2: i32,
    }

    #[test]
    fn test_macro_multi_guarded() {
        stack_pin_init!(let s = pin_init!(MyMultiGuardedStruct {
            mu1 <- KMutex::init(),
            mu2 <- KMutex::init(),
            data1: 10.into(),
            data2: 20.into(),
        }));

        lock!(let mut guard1 = s.lock_mu1());
        lock!(let mut guard2 = s.lock_mu2());

        assert_eq!(*guard1.data1(), 10);
        assert_eq!(*guard2.data2(), 20);
        *guard1.as_mut().data1_mut() = 15;
        *guard2.as_mut().data2_mut() = 25;
        assert_eq!(*guard1.data1(), 15);
        assert_eq!(*guard2.data2(), 25);
    }

    #[guarded]
    struct MyDefaultGuardedStruct {
        #[mutex]
        mu: KMutex,
        #[guarded_by(mu)]
        data: u32,
    }

    #[test]
    fn test_derive_default_guarded() {
        stack_pin_init!(let s = pin_init!(MyDefaultGuardedStruct {
            mu <- KMutex::init(),
            data: 0.into(),
        }));
        lock!(let guard = s.lock_mu());
        assert_eq!(*guard.data(), 0);
    }

    #[guarded]
    struct MyGenericGuardedStruct<T> {
        #[mutex]
        mu: KMutex,
        #[guarded_by(mu)]
        data: T,
    }

    #[test]
    fn test_macro_generic_guarded() {
        stack_pin_init!(let s = pin_init!(MyGenericGuardedStruct::<u32> {
            mu <- KMutex::init(),
            data: 0.into(),
        }));
        lock!(let mut guard = s.lock_mu());
        assert_eq!(*guard.data(), 0);

        *guard.as_mut().data_mut() = 42;
        assert_eq!(*guard.data(), 42);

        let fields = guard.as_mut().fields_mut();
        *fields.data = 100;

        let fields_shared = guard.fields();
        assert_eq!(*fields_shared.data, 100);
    }

    #[guarded]
    struct MyExplicitParentGuardedStruct {
        #[mutex]
        mu: KMutex,
        #[guarded_by(mu)]
        data: u32,
        pub label: &'static str,
    }

    impl MyExplicitParentGuardedStruct {
        pub fn has_label(&self) -> bool {
            !self.label.is_empty()
        }
    }

    impl<'a> MyExplicitParentGuardedStructMuGuard<'a> {
        pub fn process_with_context(self: Pin<&mut Self>) {
            let me = unsafe { self.get_unchecked_mut() };
            let has_label = me.parent.has_label();
            let label = me.parent.label;
            if has_label && label == "apply_update" {
                unsafe {
                    let mut_self = Pin::new_unchecked(me);
                    let fields = mut_self.fields_mut();
                    *fields.data = 100;
                }
            }
        }
    }

    #[test]
    fn test_macro_guard_explicit_parent_access() {
        stack_pin_init!(let s = pin_init!(MyExplicitParentGuardedStruct {
            mu <- KMutex::init(),
            data: 0.into(),
            label: "apply_update",
        }));

        {
            lock!(let mut guard = s.lock_mu());
            guard.as_mut().process_with_context();
        }

        lock!(let guard = s.lock_mu());
        assert_eq!(*guard.data(), 100);
    }

    #[test]
    fn test_call_unlocked() {
        stack_pin_init!(let s = pin_init!(MyGuardedStruct {
            mu <- KMutex::init(),
            data1: 10.into(),
            data2: 20.into(),
        }));

        lock!(let mut guard = s.lock_mu());
        assert_eq!(*guard.data1(), 10);

        let unlocked_result = guard.as_mut().call_unlocked(|| {
            // During call_unlocked, the lock is temporarily released.
            42
        });
        assert_eq!(unlocked_result, 42);

        // After call_unlocked returns, the lock is held again and fields can be accessed/modified.
        *guard.as_mut().data1_mut() = 99;
        assert_eq!(*guard.data1(), 99);
    }

    #[guarded]
    struct AliasedTargetStruct {
        #[mutex(MyGuardedStructMuClass)]
        mu: KMutex<crate::PhantomMutex>,
        #[guarded_by(mu)]
        value: u32,
    }

    #[test]
    fn test_aliased_lock_basic() {
        stack_pin_init!(let s = pin_init!(MyGuardedStruct {
            mu <- KMutex::init(),
            data1: 100.into(),
            data2: 200.into(),
        }));

        stack_pin_init!(let target = pin_init!(AliasedTargetStruct {
            mu: KMutex::new(crate::PhantomMutex),
            value: 300.into(),
        }));

        {
            lock!(let mut guard = aliased_lock(&s.mu, &target.mu));

            // Access fields of s and target using tokens() pair
            let (t1, t2) = guard.tokens();
            assert_eq!(*s.guard_mu(t1).data1(), 100);
            assert_eq!(*s.guard_mu(t1).data2(), 200);
            assert_eq!(*target.guard_mu(t2).value(), 300);

            // Disjoint simultaneous mutable access to both structs
            let (t1_mut, t2_mut) = guard.as_mut().tokens_mut();
            *s.guard_mu_mut(t1_mut).data1_mut() = 101;
            *target.guard_mu_mut(t2_mut).value_mut() = 301;

            // Call unlocked on aliased guard
            let res = guard.as_mut().call_unlocked(|| 1234);
            assert_eq!(res, 1234);

            // Verify mutations persist
            let (t1, t2) = guard.tokens();
            assert_eq!(*s.guard_mu(t1).data1(), 101);
            assert_eq!(*target.guard_mu(t2).value(), 301);
        }

        // Verify state with regular lock
        lock!(let guard = s.lock_mu());
        assert_eq!(*guard.data1(), 101);
    }

    #[test]
    fn test_aliased_lock_macro_method() {
        stack_pin_init!(let s = pin_init!(MyGuardedStruct {
            mu <- KMutex::init(),
            data1: 10.into(),
            data2: 20.into(),
        }));

        stack_pin_init!(let target = pin_init!(AliasedTargetStruct {
            mu: KMutex::new(crate::PhantomMutex),
            value: 30.into(),
        }));

        {
            lock!(let mut guard = s.lock_mu_aliased(&target.mu));
            let (_t1, t2) = guard.tokens();
            assert_eq!(*target.guard_mu(t2).value(), 30);
            let (_t1_mut, t2_mut) = guard.as_mut().tokens_mut();
            *target.guard_mu_mut(t2_mut).value_mut() = 35;
        }

        lock!(let guard = s.lock_mu());
        assert_eq!(*target.guard_mu(guard.token()).value(), 35);
    }

    #[guarded]
    struct FlaggedMutexStruct {
        #[mutex(flags = lockdep::LOCK_FLAGS_ACTIVE_LIST_DISABLED)]
        seek_lock: KMutex,
        #[guarded_by(seek_lock)]
        seek: u64,
    }

    #[test]
    fn test_flagged_mutex_struct() {
        stack_pin_init!(let s = pin_init!(FlaggedMutexStruct {
            seek_lock <- KMutex::init(),
            seek: 0.into(),
        }));

        lock!(let mut guard = s.lock_seek_lock());
        assert_eq!(*guard.seek(), 0);
        *guard.as_mut().seek_mut() = 1024;
        assert_eq!(*guard.seek(), 1024);
    }
}
