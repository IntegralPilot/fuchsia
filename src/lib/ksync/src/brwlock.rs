// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::marker::PhantomData;
use core::pin::Pin;
use pin_init::{PinInit, pin_data, pin_init, pinned_drop};

use crate::{LockEntryStorage, RawBrwLockPi};
use lockdep::LockClass;

/// A priority-inheriting reader-writer lock.
#[repr(transparent)]
#[pin_data]
pub struct BrwLockPi<Class: LockClass> {
    #[pin]
    lock: RawBrwLockPi,
    _marker: PhantomData<Class>,
}

// SAFETY: BrwLockPi is safe to share across threads because the underlying RawBrwLockPi is Sync.
unsafe impl<Class: LockClass> Sync for BrwLockPi<Class> {}
unsafe impl<Class: LockClass> Send for BrwLockPi<Class> {}

impl<Class: LockClass> BrwLockPi<Class> {
    /// Safe dynamic initialization inside pin context.
    pub fn init() -> impl PinInit<Self, core::convert::Infallible> {
        pin_init!(Self {
            // SAFETY: `Class::ID` is either null or a valid, static `LockClassId`.
            lock <- unsafe { RawBrwLockPi::init(Class::ID) },
            _marker: PhantomData,
        })
    }

    /// Acquire the read lock and return a stack-pinned validation guard.
    #[inline]
    pub fn read_lock(
        &self,
    ) -> impl PinInit<BrwLockPiReadGuard<'_, Class>, core::convert::Infallible> {
        BrwLockPiReadGuard::new(self)
    }

    /// Acquire the write lock and return a stack-pinned validation guard.
    #[inline]
    pub fn write_lock(
        &self,
    ) -> impl PinInit<BrwLockPiWriteGuard<'_, Class>, core::convert::Infallible> {
        BrwLockPiWriteGuard::new(self)
    }
}

/// A validation guard representing reader lock ownership and active list participation.
#[repr(C)]
#[pin_data(PinnedDrop)]
pub struct BrwLockPiReadGuard<'a, Class: LockClass> {
    lock: &'a BrwLockPi<Class>,
    #[pin]
    lock_entry: LockEntryStorage,
    token: crate::LockToken<'a, Class>,
}

impl<'a, Class: LockClass> BrwLockPiReadGuard<'a, Class> {
    /// Creates a new stack-pinned validation guard initialization block.
    pub fn new(lock: &'a BrwLockPi<Class>) -> impl PinInit<Self, core::convert::Infallible> {
        // SAFETY: The closure correctly initializes all fields of the allocated
        // `BrwLockPiReadGuard` and satisfies all safety requirements of `pin_init_from_closure`.
        unsafe {
            pin_init::pin_init_from_closure(
                move |this: *mut Self| -> Result<(), core::convert::Infallible> {
                    let lock_addr = core::ptr::addr_of_mut!((*this).lock);
                    core::ptr::write(lock_addr, lock);

                    let entry_addr = core::ptr::addr_of_mut!((*this).lock_entry);
                    core::ptr::write(entry_addr, LockEntryStorage::default());

                    lock.lock.acquire_read(entry_addr as *mut core::ffi::c_void);

                    let token_addr = core::ptr::addr_of_mut!((*this).token);
                    core::ptr::write(token_addr, crate::LockToken::new());

                    Ok(())
                },
            )
        }
    }

    /// Returns a shared reference to the lock proof `LockToken`.
    #[inline]
    pub fn token(&self) -> &crate::LockToken<'a, Class> {
        &self.token
    }

    /// Returns a mutable reference to the lock proof `LockToken` inside this pinned projection.
    #[inline]
    pub fn token_mut(self: Pin<&mut Self>) -> &mut crate::LockToken<'a, Class> {
        // SAFETY: We are accessing `token` mutably but `LockToken` is a ZST and does not require
        // pinning invariants to be maintained.
        let me = unsafe { self.get_unchecked_mut() };
        &mut me.token
    }

    /// Temporarily releases the read lock before executing the given callable `f` and then
    /// re-acquires the read lock.
    #[inline]
    pub fn call_unlocked<R, F: FnOnce() -> R>(self: Pin<&mut Self>, f: F) -> R {
        // SAFETY: `lock_entry` is pinned on the stack and valid.
        unsafe {
            let me = self.get_unchecked_mut();
            let entry_addr = &mut me.lock_entry as *mut _;
            me.lock.lock.release_read(entry_addr as *mut core::ffi::c_void);
            let result = f();
            me.lock.lock.acquire_read(entry_addr as *mut core::ffi::c_void);
            result
        }
    }
}

#[pinned_drop]
impl<'a, Class: LockClass> PinnedDrop for BrwLockPiReadGuard<'a, Class> {
    fn drop(self: Pin<&mut Self>) {
        // SAFETY: `get_unchecked_mut` is safe because we do not move the fields out of Pin.
        // `release_read` is safe because the read lock was acquired when creating this guard,
        // and we are releasing it with the same entry storage.
        unsafe {
            let me = self.get_unchecked_mut();
            let entry_addr = &mut me.lock_entry as *mut _;
            me.lock.lock.release_read(entry_addr as *mut core::ffi::c_void);
        }
    }
}

/// A validation guard representing writer lock ownership and active list participation.
#[repr(C)]
#[pin_data(PinnedDrop)]
pub struct BrwLockPiWriteGuard<'a, Class: LockClass> {
    lock: &'a BrwLockPi<Class>,
    #[pin]
    lock_entry: LockEntryStorage,
    token: crate::LockToken<'a, Class>,
}

impl<'a, Class: LockClass> BrwLockPiWriteGuard<'a, Class> {
    /// Creates a new stack-pinned validation guard initialization block.
    pub fn new(lock: &'a BrwLockPi<Class>) -> impl PinInit<Self, core::convert::Infallible> {
        // SAFETY: The closure correctly initializes all fields of the allocated
        // `BrwLockPiWriteGuard` and satisfies all safety requirements of `pin_init_from_closure`.
        unsafe {
            pin_init::pin_init_from_closure(
                move |this: *mut Self| -> Result<(), core::convert::Infallible> {
                    let lock_addr = core::ptr::addr_of_mut!((*this).lock);
                    core::ptr::write(lock_addr, lock);

                    let entry_addr = core::ptr::addr_of_mut!((*this).lock_entry);
                    core::ptr::write(entry_addr, LockEntryStorage::default());

                    lock.lock.acquire_write(entry_addr as *mut core::ffi::c_void);

                    let token_addr = core::ptr::addr_of_mut!((*this).token);
                    core::ptr::write(token_addr, crate::LockToken::new());

                    Ok(())
                },
            )
        }
    }

    /// Returns a shared reference to the lock proof `LockToken`.
    #[inline]
    pub fn token(&self) -> &crate::LockToken<'a, Class> {
        &self.token
    }

    /// Returns a mutable reference to the lock proof `LockToken` inside this pinned projection.
    #[inline]
    pub fn token_mut(self: Pin<&mut Self>) -> &mut crate::LockToken<'a, Class> {
        // SAFETY: We are accessing `token` mutably but `LockToken` is a ZST and does not require
        // pinning invariants to be maintained.
        let me = unsafe { self.get_unchecked_mut() };
        &mut me.token
    }

    /// Temporarily releases the write lock before executing the given callable `f` and then
    /// re-acquires the write lock.
    #[inline]
    pub fn call_unlocked<R, F: FnOnce() -> R>(self: Pin<&mut Self>, f: F) -> R {
        // SAFETY: `lock_entry` is pinned on the stack and valid.
        unsafe {
            let me = self.get_unchecked_mut();
            let entry_addr = &mut me.lock_entry as *mut _;
            me.lock.lock.release_write(entry_addr as *mut core::ffi::c_void);
            let result = f();
            me.lock.lock.acquire_write(entry_addr as *mut core::ffi::c_void);
            result
        }
    }
}

#[pinned_drop]
impl<'a, Class: LockClass> PinnedDrop for BrwLockPiWriteGuard<'a, Class> {
    fn drop(self: Pin<&mut Self>) {
        // SAFETY: `get_unchecked_mut` is safe because we do not move the fields out of Pin.
        // `release_write` is safe because the write lock was acquired when creating this guard,
        // and we are releasing it with the same entry storage.
        unsafe {
            let me = self.get_unchecked_mut();
            let entry_addr = &mut me.lock_entry as *mut _;
            me.lock.lock.release_write(entry_addr as *mut core::ffi::c_void);
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use crate::{BrwLockPi, RawBrwLockPi, guarded};
    use pin_init::{pin_init, stack_pin_init};
    use std::string::String;
    use std::vec::Vec;

    #[guarded]
    struct Database {
        #[brwlock]
        lock: BrwLockPi,

        #[guarded_by(lock)]
        data: Vec<String>,

        #[guarded_by(lock)]
        query_count: u64,
    }

    fn read_data(db: &Database) -> usize {
        lock!(let guard = db.read_lock());
        let len = guard.data().len();

        let fields = guard.fields();
        let _ = fields.data.len();
        let _ = *fields.query_count;

        len
    }

    fn append_data(db: &Database, value: String) {
        lock!(let mut guard = db.write_lock());
        let fields = guard.as_mut().fields_mut();
        fields.data.push(value);
        *fields.query_count += 1;
    }

    #[test]
    fn test_brwlock_projections() {
        stack_pin_init!(let db = pin_init!(Database {
            lock <- BrwLockPi::init(),
            data: Vec::new().into(),
            query_count: 0.into(),
        }));

        assert_eq!(read_data(&db), 0);
        append_data(&db, String::from("hello"));
        assert_eq!(read_data(&db), 1);

        lock!(let guard = db.read_lock());
        assert_eq!(guard.data()[0], "hello");
        assert_eq!(*guard.query_count(), 1);
    }

    #[pin_init::pin_data]
    struct BrwLockTest {
        #[pin]
        lock: RawBrwLockPi,
        state: std::sync::atomic::AtomicU32,
        kill: std::sync::atomic::AtomicBool,
    }

    fn run_test(readers: usize, writers: usize) {
        use std::sync::atomic::Ordering;

        stack_pin_init!(let test = pin_init!(BrwLockTest {
            lock <- unsafe { RawBrwLockPi::init(core::ptr::null()) },
            state: std::sync::atomic::AtomicU32::new(0).into(),
            kill: std::sync::atomic::AtomicBool::new(false).into(),
        }));

        std::thread::scope(|s| {
            let mut threads = std::vec::Vec::new();

            for _ in 0..readers {
                threads.push(s.spawn(|| {
                    while !test.kill.load(Ordering::Relaxed) {
                        // SAFETY: lock is initialized and pinned.
                        unsafe {
                            test.lock.acquire_read(core::ptr::null_mut());
                        }
                        test.state.fetch_add(1, Ordering::Relaxed);
                        std::thread::yield_now();
                        test.state.fetch_sub(1, Ordering::Relaxed);
                        // SAFETY: lock is held in read mode.
                        unsafe {
                            test.lock.release_read(core::ptr::null_mut());
                        }
                        std::thread::yield_now();
                    }
                }));
            }

            for _ in 0..writers {
                threads.push(s.spawn(|| {
                    while !test.kill.load(Ordering::Relaxed) {
                        // SAFETY: lock is initialized and pinned.
                        unsafe {
                            test.lock.acquire_write(core::ptr::null_mut());
                        }
                        test.state.fetch_add(0x10000, Ordering::Relaxed);
                        std::thread::yield_now();
                        test.state.fetch_sub(0x10000, Ordering::Relaxed);
                        // SAFETY: lock is held in write mode.
                        unsafe {
                            test.lock.release_write(core::ptr::null_mut());
                        }
                        std::thread::yield_now();
                    }
                }));
            }

            let start = std::time::Instant::now();
            while start.elapsed() < std::time::Duration::from_millis(300) {
                let local_state = test.state.load(Ordering::Relaxed);
                let num_readers = (local_state & 0xffff) as usize;
                let num_writers = (local_state >> 16) as usize;

                assert!(num_readers <= readers, "Too many readers: {}", num_readers);
                assert!(num_writers <= 1, "Too many writers: {}", num_writers);
                assert!(
                    num_readers == 0 || num_writers == 0,
                    "Both readers ({}) and writers ({}) active!",
                    num_readers,
                    num_writers
                );

                std::thread::yield_now();
            }

            test.kill.store(true, Ordering::SeqCst);
        });
    }

    #[test]
    fn test_parallel_readers() {
        run_test(8, 0);
    }

    #[test]
    fn test_single_writer() {
        run_test(0, 4);
    }

    #[test]
    fn test_readers_and_writers() {
        run_test(4, 2);
    }

    #[allow(dead_code)]
    struct CustomBrwLockClass;
    impl lockdep::LockClass for CustomBrwLockClass {
        const ID: *mut core::ffi::c_void = core::ptr::null_mut();
    }

    #[guarded]
    struct CustomBrwLockStruct {
        #[brwlock(CustomBrwLockClass)]
        lock: BrwLockPi<CustomBrwLockClass>,
        #[guarded_by(lock)]
        value: u32,
    }

    #[test]
    fn test_custom_brwlock_class() {
        stack_pin_init!(let s = pin_init!(CustomBrwLockStruct {
            lock <- BrwLockPi::init(),
            value: 42.into(),
        }));

        lock!(let guard = s.read_lock());
        assert_eq!(*guard.value(), 42);
    }
}
