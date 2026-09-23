// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(ktest)]
#[unittest::suite]
/// Tests for Rust ksync bindings
mod ksync_tests {
    use pin_init::{pin_data, pin_init, stack_pin_init};
    use unittest::{assert_true, expect_true};

    #[ksync::guarded]
    #[fbl::ref_counted]
    #[derive(fbl::Recyclable)]
    #[pin_data]
    #[repr(C)]
    struct GuardedMutexObj {
        #[mutex]
        mu: ksync::KMutex,
        #[guarded_by(mu)]
        value: u32,
    }

    #[ksync::guarded]
    struct GuardedSpinlockObj {
        #[mutex]
        mu: ksync::KMutex<ksync::RawSpinlock>,
        #[guarded_by(mu)]
        value: u32,
    }

    #[ksync::guarded]
    struct GuardedSpinlockObj2 {
        #[mutex]
        mu: ksync::KMutex<ksync::RawSpinlock>,
        #[guarded_by(mu)]
        value: u32,
    }

    #[ksync::guarded]
    struct GuardedMonitoredSpinlockObj {
        #[mutex]
        mu: ksync::KMutex<ksync::RawMonitoredSpinlock>,
        #[guarded_by(mu)]
        value: u32,
    }

    #[ksync::guarded]
    struct GuardedCriticalMutexObj {
        #[mutex]
        mu: ksync::KMutex<ksync::RawCriticalMutex>,
        #[guarded_by(mu)]
        value: u32,
    }

    #[ksync::guarded]
    struct GuardedBrwLockObj {
        #[brwlock]
        lock: ksync::BrwLockPi,
        #[guarded_by(lock)]
        value: u32,
    }

    #[ksync::guarded]
    struct GuardedOnceObj {
        #[mutex]
        mu: ksync::KMutex,
        #[guarded_by(mu)]
        once_val: ksync::KOnceCell<u32>,
    }

    #[ksync::guarded]
    struct GuardedPhantomObj {
        #[mutex(GuardedMutexObjMuClass)]
        mu: ksync::KMutex<ksync::PhantomMutex>,
        #[guarded_by(mu)]
        target: fbl::RefPtr<GuardedMutexObj>,
    }

    #[ksync::guarded]
    #[fbl::ref_counted]
    #[derive(fbl::Recyclable)]
    #[pin_data]
    #[repr(C)]
    struct GuardedGenericObj<T> {
        #[mutex]
        mu: ksync::KMutex,
        #[guarded_by(mu)]
        value: T,
    }

    #[ksync::guarded]
    struct GuardedGenericPhantomObj<T> {
        #[mutex(GuardedGenericObjMuClass<T>)]
        mu: ksync::KMutex<ksync::PhantomMutex>,
        #[guarded_by(mu)]
        target: fbl::RefPtr<GuardedGenericObj<T>>,
    }

    unsafe extern "C" {
        fn cpp_verify_mutex_id(
            lock: *const core::ffi::c_void,
            expected_id: *const core::ffi::c_void,
        ) -> bool;
        fn cpp_verify_critical_mutex_id(
            lock: *const core::ffi::c_void,
            expected_id: *const core::ffi::c_void,
        ) -> bool;
        fn cpp_verify_spinlock_id(
            lock: *const core::ffi::c_void,
            expected_id: *const core::ffi::c_void,
        ) -> bool;
        fn cpp_verify_monitored_spinlock_id(
            lock: *const core::ffi::c_void,
            expected_id: *const core::ffi::c_void,
        ) -> bool;
        fn cpp_verify_brwlock_id(
            lock: *const core::ffi::c_void,
            expected_id: *const core::ffi::c_void,
        ) -> bool;

        fn cpp_get_test_singleton_mutex_ptr() -> *const core::ffi::c_void;
        fn cpp_get_test_singleton_mutex_size() -> usize;
        fn cpp_get_test_singleton_mutex_raw_lock_ptr() -> *const core::ffi::c_void;
        fn cpp_get_test_singleton_mutex_raw_lock_size() -> usize;

        fn cpp_get_test_singleton_critical_mutex_ptr() -> *const core::ffi::c_void;
        fn cpp_get_test_singleton_critical_mutex_size() -> usize;
        fn cpp_get_test_singleton_critical_mutex_raw_lock_ptr() -> *const core::ffi::c_void;
        fn cpp_get_test_singleton_critical_mutex_raw_lock_size() -> usize;
    }

    /// test Rust KMutex ID
    #[test]
    fn mutex_id() {
        let obj = fbl::pin_make_ref_counted!(GuardedMutexObj {
            mu <- ksync::KMutex::init(),
            value: 0.into(),
        })
        .unwrap();
        unsafe {
            assert_true!(cpp_verify_mutex_id(
                &obj.mu as *const _ as *const core::ffi::c_void,
                <GuardedMutexObjMuClass as ksync::LockClass>::ID,
            ));
        }
    }

    /// test Rust KCriticalMutex ID
    #[test]
    fn critical_mutex_id() {
        stack_pin_init!(let obj = pin_init!(GuardedCriticalMutexObj {
            mu <- ksync::KMutex::init(),
            value: 0.into(),
        }));
        unsafe {
            assert_true!(cpp_verify_critical_mutex_id(
                &obj.mu as *const _ as *const core::ffi::c_void,
                <GuardedCriticalMutexObjMuClass as ksync::LockClass>::ID,
            ));
        }
    }

    /// test Rust KSpinlock ID
    #[test]
    fn spinlock_id() {
        stack_pin_init!(let obj = pin_init!(GuardedSpinlockObj {
            mu <- ksync::KMutex::init(),
            value: 0.into(),
        }));
        unsafe {
            assert_true!(cpp_verify_spinlock_id(
                &obj.mu as *const _ as *const core::ffi::c_void,
                <GuardedSpinlockObjMuClass as ksync::LockClass>::ID,
            ));
        }
    }

    /// test Rust KMonitoredSpinlock ID and acquire/release with source_tag!
    #[test]
    fn monitored_spinlock_id_and_ops() {
        stack_pin_init!(let obj = pin_init!(GuardedMonitoredSpinlockObj {
            mu <- ksync::KMutex::init(),
            value: 10.into(),
        }));
        unsafe {
            assert_true!(cpp_verify_monitored_spinlock_id(
                &obj.mu as *const _ as *const core::ffi::c_void,
                <GuardedMonitoredSpinlockObjMuClass as ksync::LockClass>::ID,
            ));
        }

        // Acquire with default IrqSavePolicy and source_tag!
        {
            ksync::lock!(let mut guard = obj.lock_mu(ksync::source_tag!()));
            expect_true!(*guard.value() == 10);
            *guard.as_mut().value_mut() = 20;
            expect_true!(*guard.value() == 20);
        }

        // Verify persistence and acquire with explicit policy
        {
            ksync::lock!(
                let guard = obj.lock_mu_policy::<ksync::IrqSavePolicy>(ksync::source_tag!())
            );
            expect_true!(*guard.value() == 20);
        }
    }

    /// test Rust BrwLockPi ID
    #[test]
    fn brwlock_id() {
        stack_pin_init!(let obj = pin_init!(GuardedBrwLockObj {
            lock <- ksync::BrwLockPi::init(),
            value: 0.into(),
        }));
        unsafe {
            assert_true!(cpp_verify_brwlock_id(
                &obj.lock as *const _ as *const core::ffi::c_void,
                <GuardedBrwLockObjLockClass as ksync::LockClass>::ID,
            ));
        }
    }

    /// test Rust KSpinlock
    #[test]
    fn spinlock() {
        stack_pin_init!(let obj = pin_init!(GuardedSpinlockObj {
            mu <- ksync::KMutex::init(),
            value: 100.into(),
        }));
        stack_pin_init!(let obj2 = pin_init!(GuardedSpinlockObj2 {
            mu <- ksync::KMutex::init(),
            value: 100.into(),
        }));

        {
            ksync::lock!(let mut guard = obj.lock_mu());
            expect_true!(*guard.value() == 100);
            *guard.as_mut().value_mut() = 101;
        }

        {
            ksync::lock!(let guard = obj.lock_mu());
            expect_true!(*guard.value() == 101);
        }
        {
            ksync::lock!(let guard = obj.mu.lock());
            ksync::lock!(let guard2 = obj2.mu.lock_policy::<ksync::NoIrqSavePolicy>());
            // Expect a no irq save policy guard to be strictly smaller than one that had to
            // remember the irq state.
            expect_true!(core::mem::size_of_val(&*guard2) < core::mem::size_of_val(&*guard));
        }
    }

    /// test Rust KMutex
    #[test]
    fn mutex() {
        let obj = fbl::pin_make_ref_counted!(GuardedMutexObj {
            mu <- ksync::KMutex::init(),
            value: 42.into(),
        })
        .unwrap();

        {
            ksync::lock!(let mut guard = obj.lock_mu());
            expect_true!(*guard.value() == 42);
            *guard.as_mut().value_mut() = 43;
        }

        {
            ksync::lock!(let guard = obj.lock_mu());
            expect_true!(*guard.value() == 43);
        }
    }

    /// test PhantomMutex and #[mutex(LockClass)]
    #[test]
    fn phantom_lock() {
        // This test demonstrates how an object (`phantom_obj`) can use a zero-sized `PhantomMutex`
        // to participate in `#[guarded]` type checking while physical mutual exclusion is
        // provided by an external physical lock (`real_obj.mu`).

        // Create an object with a real, physical `KMutex`, wrapped in an `fbl::RefPtr`.
        let real_obj = fbl::pin_make_ref_counted!(GuardedMutexObj {
            mu <- ksync::KMutex::init(),
            value: 1.into(),
        })
        .unwrap();

        // Create an object protected by a zero-sized `PhantomMutex` (no physical lock storage).
        // Its synchronization is provided externally by `real_obj.mu`.
        let target = real_obj.clone();
        stack_pin_init!(let phantom_obj = pin_init!(GuardedPhantomObj {
            mu: ksync::KMutex::new(ksync::PhantomMutex),
            target: target.into(),
        }));

        {
            // Acquire a guard for `phantom_obj` by passing the physical `real_obj.mu` to `lock_mu`.
            ksync::lock!(let mut guard = phantom_obj.lock_mu(&real_obj.mu));

            // Read the reference to `real_obj` from `phantom_obj.target`.
            let target = guard.target().clone();

            // Using the proof token from `guard`, read `target.value`.
            expect_true!(*target.guard_mu(guard.token()).value() == 1);

            // Modify `target.value` using the mutable token from `guard`.
            *target.guard_mu_mut(guard.as_mut().token_mut()).value_mut() = 200;
        }

        {
            // Lock `real_obj` directly using `GuardedMutexObj`'s own guard to verify
            // that we read back the same value (200) that was written via `phantom_obj`.
            ksync::lock!(let guard = real_obj.lock_mu());
            expect_true!(*guard.value() == 200);
        }
    }

    /// test PhantomMutex with generic LockClass
    #[test]
    fn generic_phantom_mutex() {
        let real_obj = fbl::pin_make_ref_counted!(GuardedGenericObj {
            mu <- ksync::KMutex::init(),
            value: 42.into(),
        })
        .unwrap();

        let real_obj_clone = real_obj.clone();
        stack_pin_init!(let phantom_obj = pin_init!(GuardedGenericPhantomObj {
            mu: ksync::KMutex::new(ksync::PhantomMutex),
            target: ksync::KCell::new(real_obj_clone),
        }));

        {
            ksync::lock!(let mut guard = phantom_obj.lock_mu(&real_obj.mu));
            let target = guard.target().clone();
            expect_true!(*target.guard_mu(guard.token()).value() == 42);
            *target.guard_mu_mut(guard.as_mut().token_mut()).value_mut() = 99;
        }

        {
            ksync::lock!(let guard = real_obj.lock_mu());
            expect_true!(*guard.value() == 99);
        }
    }

    /// test Rust BrwLockPi
    #[test]
    fn brwlock() {
        stack_pin_init!(let obj = pin_init!(GuardedBrwLockObj {
            lock <- ksync::BrwLockPi::init(),
            value: 10.into(),
        }));

        {
            ksync::lock!(let guard = obj.read_lock());
            expect_true!(*guard.value() == 10);
        }

        {
            ksync::lock!(let mut guard = obj.write_lock());
            expect_true!(*guard.value() == 10);
            *guard.as_mut().value_mut() = 20;
        }

        {
            ksync::lock!(let guard = obj.read_lock());
            expect_true!(*guard.value() == 20);
        }
    }

    /// test call_unlocked on KMutex and macro-generated guards
    #[test]
    fn call_unlocked() {
        let obj = fbl::pin_make_ref_counted!(GuardedMutexObj {
            mu <- ksync::KMutex::init(),
            value: 10.into(),
        })
        .unwrap();

        ksync::lock!(let mut guard = obj.lock_mu());
        expect_true!(*guard.value() == 10);

        let res = guard.as_mut().call_unlocked(|| 42);
        expect_true!(res == 42);

        *guard.as_mut().value_mut() = 25;
        expect_true!(*guard.value() == 25);
    }

    /// test AliasedLock in kernel
    #[test]
    fn aliased_lock() {
        let real_obj = fbl::pin_make_ref_counted!(GuardedMutexObj {
            mu <- ksync::KMutex::init(),
            value: 10.into(),
        })
        .unwrap();

        let target = real_obj.clone();
        stack_pin_init!(let phantom_obj = pin_init!(GuardedPhantomObj {
            mu: ksync::KMutex::new(ksync::PhantomMutex),
            target: target.into(),
        }));

        {
            ksync::lock!(let mut guard = ksync::aliased_lock(&real_obj.mu, &phantom_obj.mu));

            let (t1, _t2) = guard.tokens();
            expect_true!(*real_obj.guard_mu(t1).value() == 10);

            let (t1_mut, _t2_mut) = guard.as_mut().tokens_mut();
            *real_obj.guard_mu_mut(t1_mut).value_mut() = 55;

            let res = guard.as_mut().call_unlocked(|| 99);
            expect_true!(res == 99);

            let (t1, _t2) = guard.tokens();
            expect_true!(*real_obj.guard_mu(t1).value() == 55);
        }

        ksync::lock!(let guard = real_obj.lock_mu());
        expect_true!(*guard.value() == 55);
    }

    #[ksync::guarded]
    struct GuardedFlaggedMutexObj {
        #[mutex(flags = lockdep::LOCK_FLAGS_ACTIVE_LIST_DISABLED)]
        seek_lock: ksync::KMutex,
        #[guarded_by(seek_lock)]
        seek: u64,
    }

    /// test Mutex with custom flags (e.g. LOCK_FLAGS_ACTIVE_LIST_DISABLED)
    #[test]
    fn flagged_mutex() {
        stack_pin_init!(let obj = pin_init!(GuardedFlaggedMutexObj {
            seek_lock <- ksync::KMutex::init(),
            seek: 0.into(),
        }));

        ksync::lock!(let mut guard = obj.lock_seek_lock());
        expect_true!(*guard.seek() == 0);
        *guard.as_mut().seek_mut() = 4096;
        expect_true!(*guard.seek() == 4096);
    }

    ksync::declare_singleton_mutex!(TestKernelSingletonMutex);
    ksync::declare_singleton_critical_mutex!(TestKernelSingletonCriticalMutex);

    /// test Rust declare_singleton_mutex and declare_singleton_critical_mutex
    #[test]
    fn singleton_mutex() {
        let lock1 = TestKernelSingletonMutex::Get();
        let lock2 = TestKernelSingletonMutex::get();
        expect_true!(core::ptr::eq(lock1, lock2));

        // Verify that the C++ lockdep helper reads the lock ID from the Rust singleton correctly.
        unsafe {
            expect_true!(cpp_verify_mutex_id(
                lock1 as *const _ as *const core::ffi::c_void,
                <TestKernelSingletonMutex as ksync::LockClass>::ID,
            ));
            expect_true!(cpp_verify_critical_mutex_id(
                TestKernelSingletonCriticalMutex::Get() as *const _ as *const core::ffi::c_void,
                <TestKernelSingletonCriticalMutex as ksync::LockClass>::ID,
            ));
        }

        // Compare total sizes of Rust-declared singletons vs C++-declared singletons.
        let rust_mutex_size = core::mem::size_of_val(lock1);
        let cpp_mutex_size = unsafe { cpp_get_test_singleton_mutex_size() };
        expect_true!(rust_mutex_size == cpp_mutex_size);

        let rust_critical_size = core::mem::size_of_val(TestKernelSingletonCriticalMutex::Get());
        let cpp_critical_size = unsafe { cpp_get_test_singleton_critical_mutex_size() };
        expect_true!(rust_critical_size == cpp_critical_size);

        // Compare raw mutex storage byte-by-byte between Rust and C++.
        let cpp_raw_ptr = unsafe { cpp_get_test_singleton_mutex_raw_lock_ptr() };
        let cpp_raw_size = unsafe { cpp_get_test_singleton_mutex_raw_lock_size() };
        let cpp_raw_bytes =
            unsafe { core::slice::from_raw_parts(cpp_raw_ptr as *const u8, cpp_raw_size) };

        let rust_raw_bytes = lock1.raw_mutex().raw_storage_slice();
        expect_true!(rust_raw_bytes == cpp_raw_bytes);

        // Compare CriticalMutex raw storage byte-by-byte between Rust and C++.
        let cpp_critical_raw_ptr = unsafe { cpp_get_test_singleton_critical_mutex_raw_lock_ptr() };
        let cpp_critical_raw_size =
            unsafe { cpp_get_test_singleton_critical_mutex_raw_lock_size() };
        let cpp_critical_raw_bytes = unsafe {
            core::slice::from_raw_parts(cpp_critical_raw_ptr as *const u8, cpp_critical_raw_size)
        };

        let rust_critical_raw_bytes =
            TestKernelSingletonCriticalMutex::Get().raw_mutex().raw_storage_slice();
        expect_true!(rust_critical_raw_bytes == cpp_critical_raw_bytes);

        {
            ksync::lock!(let _guard = TestKernelSingletonMutex::Get().lock());
        }

        {
            ksync::lock!(TestKernelSingletonMutex::lock());
        }

        {
            ksync::lock!(let _guard = TestKernelSingletonCriticalMutex::Get().lock());
        }
    }

    /// test KOnceCell lazy initialization and access
    #[test]
    fn konce_cell_test() {
        static TEST_CELL: ksync::KOnceCell<u64, TestKernelSingletonMutex> = ksync::KOnceCell::new();
        ksync::lock!(let mut guard = TestKernelSingletonMutex::lock());
        unsafe {
            let token = guard.token();
            expect_true!(!TEST_CELL.is_initialized(token));
            expect_true!(TEST_CELL.get(token).is_none());

            let token_mut = guard.as_mut().token_mut();
            let val = TEST_CELL.get_or_init(|| 12345u64, token_mut);
            expect_true!(*val == 12345);

            let token = guard.token();
            expect_true!(TEST_CELL.is_initialized(token));
            expect_true!(TEST_CELL.get(token) == Some(&12345));

            let unchk = TEST_CELL.get_unchecked();
            expect_true!(unchk == Some(&12345));
        }
    }

    /// test KOnceCell integration with #[ksync::guarded]
    #[test]
    fn guarded_konce_cell_test() {
        stack_pin_init!(let obj = pin_init!(GuardedOnceObj {
            mu <- ksync::KMutex::init(),
            once_val: ksync::KOnceCell::new(),
        }));

        // Before init: read accessor returns None
        {
            ksync::lock!(let guard = obj.lock_mu());
            expect_true!(guard.once_val().is_none());
        }

        // Initialize through mutable guard
        {
            ksync::lock!(let mut guard = obj.lock_mu());
            let mut once_cell = guard.as_mut().once_val_cell();
            let val = once_cell.get_or_init(|| 42);
            expect_true!(*val == 42);

            // Read after init
            expect_true!(guard.once_val() == Some(&42));
            expect_true!(guard.as_mut().once_val_mut() == Some(&mut 42));

            // Attempting to set fails because it's already initialized
            expect_true!(guard.as_mut().once_val_cell().set(99) == Err(99));

            // Test proxy cell access
            expect_true!(guard.as_mut().once_val_cell().get() == Some(&42));

            // Disjoint fields access
            let fields = guard.fields();
            expect_true!(fields.once_val == Some(&42));

            let fields_mut = guard.as_mut().fields_mut();
            expect_true!(fields_mut.once_val == Some(&mut 42));
        }
    }
}
