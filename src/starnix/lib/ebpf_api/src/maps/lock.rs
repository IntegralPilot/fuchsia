// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::cell::UnsafeCell;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicU32, Ordering};

/// A read-write lock used to synchronize access to control data structures of
/// shared eBPF maps that are stored in shared VMOs. The lock stores its state
/// inside the VMO as a 32-bit atomic value. It also uses signals on the VMO
/// handle in order to wait when the acquire operation has to be blocked.
/// The first 3 arguments passed to `RwMapLock::new()` define the lock. They
/// should be the same for all instances of the lock in all processes that
/// share the eBPF map.
///
/// Unlike regular locks, an instance of `RwMapLock` is created only when the
/// lock is about to be acquired. `read()` and `write()` consume `self`, i.e.
/// each instance of the lock can be used only once.
///
/// The implementation provides the following properties:
///
/// * System calls are for the most common case when the lock is free. The
/// VMO handle is signalled only when when there are threads waiting for the
/// lock.
/// * Readers may be blocked to yield to writers. This allows to avoid
/// starvation of the writing threads.
/// * Spurious wake-ups of the waiting threads are possible when the
/// handle/signal pair is shared between multiple locks. This is acceptable
/// because normally each eBPF map is shared only with a small number of
/// processes and eBPF programs. Also neither readers nor writers are expected
/// to hold the lock for a long amount of time.
pub(super) struct RwMapLock<'a, T> {
    state: RwMapLockState<'a, T>,
}

impl<'a, T> RwMapLock<'a, T> {
    /// Creates a new instance of the lock. `state_cell` refers to an atomic
    /// cell inside of the shared VMO used to store the state of the lock.
    /// `handle` and `signal` are used by the lock when the thread needs to be
    /// blocked. All threads that depend on the lock must use the same set of
    /// these 3 parameters. `handle` and `signal` may be shared between
    /// multiple locks. Normally `handle` should be the handle of the VMO
    /// where `state_cell` is stored, but that's not required.
    ///
    /// `value` contains an object that provides access to the data being
    /// controlled by this lock. Normally it should read and write data from
    /// the same VMO that stores the `state_cell`.
    pub unsafe fn new(
        state_cell: &'a AtomicU32,
        handle: zx::HandleRef<'a>,
        signal: zx::Signals,
        value: T,
    ) -> Self {
        Self { state: RwMapLockState { state_cell, handle, signal, value: UnsafeCell::new(value) } }
    }

    /// Acquires the lock for exclusive access.
    pub fn write(self) -> RwMapLockWriteGuard<'a, T> {
        let Self { state } = self;
        state.lock_write();
        RwMapLockWriteGuard { state }
    }

    /// Acquires the lock for shared access.
    pub fn read(self) -> RwMapLockReadGuard<'a, T> {
        let Self { state } = self;
        state.lock_read();
        RwMapLockReadGuard { state }
    }
}
pub(super) struct RwMapLockReadGuard<'a, T> {
    state: RwMapLockState<'a, T>,
}

impl<'a, T> Drop for RwMapLockReadGuard<'a, T> {
    #[inline]
    fn drop(&mut self) {
        self.state.unlock_read();
    }
}

impl<'a, T> Deref for RwMapLockReadGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        unsafe {
            &*self.state.value.get()
        }
    }
}

pub(super) struct RwMapLockWriteGuard<'a, T> {
    state: RwMapLockState<'a, T>,
}

impl<'a, T> Drop for RwMapLockWriteGuard<'a, T> {
    #[inline]
    fn drop(&mut self) {
        self.state.unlock_write();
    }
}

impl<'a, T> Deref for RwMapLockWriteGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &T {
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        unsafe {
            &*self.state.value.get()
        }
    }
}

impl<'a, T> DerefMut for RwMapLockWriteGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        unsafe {
            &mut *self.state.value.get()
        }
    }
}

/// Stores the internal state of an `RwMapLock`.
struct RwMapLockState<'a, T> {
    /// Reference to a 32-bit value inside the VMO that stores the current
    /// state of the lock. The value contains 4 fields as follows:
    ///
    /// +-----------------+-----------------+------------+-------------+
    /// | waiting_readers | waiting_writers | write_lock | num_readers |
    /// |      10 bit     |      10 bit     |   1 bit    |   11 bit    |
    /// +-----------------+-----------------+------------+-------------+
    ///
    ///  - `waiting_readers`: number of threads waiting to acquire the lock
    /// for shared access (read).
    ///  - `waiting_writers`: number of threads waiting to acquire the lock
    /// for exclusive access (write).
    ///  - `write_lock`: indicates that the lock is currently acquired for
    /// exclusive access.
    ///  - `num_readers`: number of threads that currently hold the lock for
    /// shared access.
    ///
    /// `write_lock` and `num_readers` fields are mutually exclusive, i.e.
    /// `write_lock` must be 0 when `num_readers` is greater than 0 and
    /// `num_readers` must be 0 when `write_lock` is set to 1.
    ///
    /// `waiting_readers` and `waiting_writers` indicate number of threads
    /// that are currently blocked trying to acquire the lock either for
    /// read of write. These threads are blocked waiting for the `signal` from
    /// the `handle`.
    state_cell: &'a AtomicU32,

    /// Handle used to wait and signal blocked threads.
    handle: zx::HandleRef<'a>,

    /// The signal used to synchronize threads waiting for the lock with the
    /// thread holding the lock.
    ///
    /// When a thread needs to acquire the lock it first tries to do so by
    /// updating the `state_cell`. If that attempt fails, then the thread
    /// increments a waiter counter (i.e. either `waiting_readers` or
    /// `waiting_writers` in `state_cell`) and then sleeps until it's woken
    /// up by the thread currently holding the lock. When a thread releases
    /// the lock, it checks the counters and then wakes up waiting threads
    /// using the signal, but only if either of the counters is greater than 0.
    ///
    /// Momentary state of the `signal` is meaningless. Waiting threads wait
    /// for the signal to transition from inactive to active using the
    /// `ZX_WAIT_ASYNC_EDGE` flag. Threads that need to wake up waiting
    /// threads do so by resetting the `signal` and then signaling it again.
    signal: zx::Signals,

    /// The value access to which is being controlled by this lock.
    value: UnsafeCell<T>,
}

thread_local! {
    static LOCK_WAIT_PORT: zx::Port = zx::Port::create();
}

impl<'a, T> RwMapLockState<'a, T> {
    const NUM_WAITING_READERS_MASK: u32 = 0xffc0_0000;
    const WAITING_READER_INCREMENT: u32 = 0x0040_0000;

    const NUM_WAITING_WRITERS_MASK: u32 = 0x003f_f000;
    const WAITING_WRITER_INCREMENT: u32 = 0x0000_1000;

    const WRITER_LOCKED_BIT: u32 = 0x0000_0800;
    const NUM_READERS_MASK: u32 = 0x0000_07ff;

    /// Tries to acquire the lock for shared access. Fails and returns false
    /// if the lock is locked for exclusive access or there are threads trying
    /// to acquire it for exclusive access.
    fn try_lock_read_fast(&self) -> bool {
        let mut state = 0;
        loop {
            match self.state_cell.compare_exchange_weak(
                state,
                state + 1,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(actual) => state = actual,
            }
            if state & (Self::WRITER_LOCKED_BIT | Self::NUM_WAITING_WRITERS_MASK) != 0 {
                return false;
            }
            assert!(state & Self::NUM_READERS_MASK < Self::NUM_READERS_MASK, "Too many readers");
        }
    }

    /// Repeatedly waits for `self.signal` from `self.handle` and tries to
    /// acquire the lock by updating the state using `update_state` callback.
    fn lock_slow_loop<F>(&self, update_state: F)
    where
        F: Fn(u32, usize) -> Option<u32>,
    {
        let mut num_waits = 0;

        let try_lock = |num_waits| {
            self.state_cell
                .try_update(Ordering::Acquire, Ordering::Relaxed, |state| {
                    update_state(state, num_waits)
                })
                .is_ok()
        };

        loop {
            const WAIT_KEY: u64 = 0;

            LOCK_WAIT_PORT.with(|port| {
                self.handle
                    .wait_async(port, WAIT_KEY, self.signal, zx::WaitAsyncOpts::EDGE_TRIGGERED)
                    .unwrap()
            });

            // Try to lock again here in case the state was updated before the
            // `wait_async()` call above. If we fail to lock here then the
            // `wait()` is guaranteed to unblock once the lock is released.
            if try_lock(num_waits) {
                LOCK_WAIT_PORT.with(|port| port.cancel(WAIT_KEY).unwrap());
                break;
            }

            LOCK_WAIT_PORT.with(|port| port.wait(zx::MonotonicInstant::INFINITE).unwrap());

            num_waits += 1;

            if try_lock(num_waits) {
                break;
            }
        }
    }

    /// Wakes up all threads waiting to acquire the lock in `lock_slow_loop()`.
    fn wake_up_waiters(&self) {
        self.handle.signal(self.signal, zx::Signals::NONE).unwrap();
        self.handle.signal(zx::Signals::NONE, self.signal).unwrap();
    }

    /// Acquires the lock for shared access in case `try_lock_read_fast()`
    /// has failed.
    fn lock_read_slow(&self) {
        // Increment counter waiting readers counter. Relaxed ordering is
        // appropriate because the counter is stored together with the state
        // so it doesn't need to be ordered with any other memory access
        // operations.
        let state = self.state_cell.fetch_add(Self::WAITING_READER_INCREMENT, Ordering::Relaxed);
        assert!(
            state & Self::NUM_WAITING_READERS_MASK < Self::NUM_WAITING_READERS_MASK,
            "waiting_readers overflow"
        );

        // Yield to writers when we have any waiting for the lock. This avoids
        // starvation for writers - they are guaranteed to acquire the lock
        // eventually, even it's being contested by readers.
        //
        // Note that the loop below yields to writers only once. This allows
        // to avoid reader starvation as well.
        let yield_to_writers = (state & Self::NUM_WAITING_WRITERS_MASK) > 0;

        self.lock_slow_loop(|state, num_waits| {
            assert!(state & Self::NUM_READERS_MASK < Self::NUM_READERS_MASK, "Too many readers");
            assert!(state & Self::NUM_WAITING_READERS_MASK > 0, "invalid lock state");

            let yield_to_writers = yield_to_writers && num_waits <= 1;
            let write_locked = state & Self::WRITER_LOCKED_BIT > 0;
            let writers_waiting = state & Self::NUM_WAITING_WRITERS_MASK > 0;
            if write_locked || (writers_waiting && yield_to_writers) {
                None
            } else {
                // Increment `num_readers` and decrement `waiting_readers`.
                Some(state - Self::WAITING_READER_INCREMENT + 1)
            }
        });
    }

    /// Acquires the lock for shared access.
    fn lock_read(&self) {
        if !self.try_lock_read_fast() {
            self.lock_read_slow();
        }
    }

    /// Releases the lock that was held by the current thread for shared
    /// access. Signals waiting threads (if any) when there are no other
    /// threads holding shared access lock.
    fn unlock_read(&self) {
        let state = self.state_cell.fetch_sub(1, Ordering::Release);
        let num_readers = state & Self::NUM_READERS_MASK;
        assert!(num_readers > 0, "not locked for read");
        let writers_waiting = state & Self::NUM_WAITING_WRITERS_MASK != 0;
        if num_readers == 1 && writers_waiting {
            self.wake_up_waiters();
        }
    }

    /// Tries to acquire the lock for exclusive access. May fail returning
    /// false if the lock is currently acquired for either shared or
    /// exclusive access.
    fn try_lock_write_fast(&self) -> bool {
        let mut state = 0;
        loop {
            match self.state_cell.compare_exchange_weak(
                state,
                state | Self::WRITER_LOCKED_BIT,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(actual) => state = actual,
            };

            if (state & (Self::NUM_READERS_MASK | Self::WRITER_LOCKED_BIT)) != 0 {
                // Already locked for read or write.
                return false;
            }
        }
    }

    /// Acquires the lock for exclusive access in case
    /// `try_lock_write_fast()` has failed.
    fn lock_write_slow(&self) {
        // Increment counter waiting readers counter. Relaxed ordering is
        // appropriate because the counter is stored together with the state
        // so it doesn't need to be ordered with any other memory access
        // operations.
        let state = self.state_cell.fetch_add(Self::WAITING_WRITER_INCREMENT, Ordering::Relaxed);
        assert!(
            state & Self::NUM_WAITING_WRITERS_MASK < Self::NUM_WAITING_WRITERS_MASK,
            "waiting_readers overflow"
        );

        self.lock_slow_loop(|state, _| {
            assert!(state & Self::NUM_WAITING_WRITERS_MASK > 0, "invalid state");
            let read_or_write_locked =
                state & (Self::NUM_READERS_MASK | Self::WRITER_LOCKED_BIT) > 0;
            if read_or_write_locked {
                None
            } else {
                // Set the `write_lock` bit and decrement `waiting_writers`.
                Some((state - Self::WAITING_WRITER_INCREMENT) | Self::WRITER_LOCKED_BIT)
            }
        });
    }

    /// Acquires the lock for exclusive access.
    fn lock_write(&self) {
        if !self.try_lock_write_fast() {
            self.lock_write_slow();
        }
    }

    /// Releases the lock held by the current thread for exclusive access.
    /// Signals to wake up other threads waiting for the lock, if any.
    fn unlock_write(&self) {
        let state = self.state_cell.fetch_and(!Self::WRITER_LOCKED_BIT, Ordering::Release);
        assert!(state & Self::WRITER_LOCKED_BIT != 0, "not locked for write");

        let threads_waiting =
            state & (Self::NUM_WAITING_READERS_MASK | Self::NUM_WAITING_WRITERS_MASK) != 0;
        if threads_waiting {
            self.wake_up_waiters();
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::maps::buffer::MapBuffer;
    use std::sync::atomic::AtomicU32;
    use std::sync::{Arc, Barrier};
    use std::thread;

    const NUM_THREADS: usize = 10;
    const NUM_ITERATIONS: usize = 100;
    const LOCK_SIGNAL: zx::Signals = zx::Signals::USER_1;

    /// Verifies that all blocked readers are all unblocked once the writer
    /// releases the lock.
    #[fuchsia::test]
    fn test_multiple_reader_waiters() {
        let buf = MapBuffer::new(32, "name").unwrap();
        let state = AtomicU32::new(0);

        // Returns a new lock that stores the state in `state` and uses VMO
        // handle for signaling.
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let lock = || unsafe { RwMapLock::new(&state, buf.vmo().as_handle_ref(), LOCK_SIGNAL, ()) };

        let readers = AtomicU32::new(0);
        let barrier = Barrier::new(NUM_THREADS + 1);

        thread::scope(|scope| {
            for _ in 0..NUM_THREADS {
                scope.spawn(|| {
                    barrier.wait();
                    let guard = lock().read();
                    readers.fetch_add(1, Ordering::Relaxed);
                    barrier.wait();
                    // main thread verifiers of readers
                    barrier.wait();
                    std::mem::drop(guard);
                });
            }

            let write_guard = lock().write();
            barrier.wait();
            assert!(readers.load(Ordering::Relaxed) as usize == 0);
            std::mem::drop(write_guard);
            barrier.wait();
            assert!(readers.load(Ordering::Relaxed) as usize == NUM_THREADS);
            barrier.wait();
            lock().write();
        });
    }

    // The tests below were adapted from the test for the RwLock in
    // `src/lib/fuchsia-sync/src/rwlock.rs`.
    struct LockedState {
        addr: usize,
    }

    impl LockedState {
        fn value(&self) -> &usize {
            #[allow(
                clippy::undocumented_unsafe_blocks,
                reason = "Force documented unsafe blocks in Starnix"
            )]
            unsafe {
                &*(self.addr as *const usize)
            }
        }
        fn value_mut(&mut self) -> &mut usize {
            #[allow(
                clippy::undocumented_unsafe_blocks,
                reason = "Force documented unsafe blocks in Starnix"
            )]
            unsafe {
                &mut *(self.addr as *mut usize)
            }
        }
    }

    struct State {
        buf: MapBuffer,
        start_barrier: Barrier,
        writer_count: AtomicU32,
        reader_count: AtomicU32,
    }

    impl State {
        fn new(num_threads: usize) -> Self {
            Self {
                buf: MapBuffer::new(32, "name").unwrap(),
                start_barrier: Barrier::new(num_threads),
                writer_count: Default::default(),
                reader_count: Default::default(),
            }
        }

        fn lock<'a>(&'a self) -> RwMapLock<'a, LockedState> {
            #[allow(
                clippy::undocumented_unsafe_blocks,
                reason = "Force documented unsafe blocks in Starnix"
            )]
            unsafe {
                let base_ptr = self.buf.ptr().raw_ptr();
                let lock_cell = &*(base_ptr as *const AtomicU32);
                RwMapLock::new(
                    lock_cell,
                    self.buf.vmo().as_handle_ref(),
                    LOCK_SIGNAL,
                    LockedState { addr: base_ptr.byte_offset(8) as usize },
                )
            }
        }

        fn spawn_writer(state: Arc<Self>, count: usize) -> std::thread::JoinHandle<()> {
            std::thread::spawn(move || {
                state.start_barrier.wait();
                for i in 0..count {
                    let mut guard = state.lock().write();
                    *guard.value_mut() += 1;
                    let writer_count = state.writer_count.fetch_add(1, Ordering::Acquire) + 1;
                    let reader_count = state.reader_count.load(Ordering::Acquire);
                    state.writer_count.fetch_sub(1, Ordering::Release);

                    // Yield on every other iteration. This gives other threads
                    // some time to get blocked waiting for the signal.
                    if i % 2 == 1 {
                        std::thread::yield_now();
                    }

                    std::mem::drop(guard);
                    assert_eq!(writer_count, 1, "More than one writer held the RwLock at once.");
                    assert_eq!(
                        reader_count, 0,
                        "A reader and writer held the RwLock at the same time."
                    );
                }
            })
        }

        fn spawn_reader(state: Arc<Self>, count: usize) -> std::thread::JoinHandle<()> {
            std::thread::spawn(move || {
                state.start_barrier.wait();
                for i in 0..count {
                    let guard = state.lock().read();
                    let observed_value = *guard.value();
                    let reader_count = state.reader_count.fetch_add(1, Ordering::Acquire) + 1;
                    let writer_count = state.writer_count.load(Ordering::Acquire);
                    state.reader_count.fetch_sub(1, Ordering::Release);

                    // Yield on every other iteration. This gives other threads
                    // some time to get blocked waiting for the signal.
                    if i % 2 == 1 {
                        std::thread::yield_now();
                    }

                    std::mem::drop(guard);
                    assert!(
                        observed_value < u32::MAX as usize,
                        "The value inside the RwLock underflowed."
                    );
                    assert_eq!(
                        writer_count, 0,
                        "A reader and writer held the RwLock at the same time."
                    );
                    assert!(reader_count > 0, "A reader held the RwLock without being counted.");
                }
            })
        }
    }

    #[test]
    fn test_single_reader_single_writer() {
        let state = Arc::new(State::new(2));
        let reader = State::spawn_reader(Arc::clone(&state), NUM_ITERATIONS);
        let writer = State::spawn_writer(Arc::clone(&state), NUM_ITERATIONS);

        reader.join().expect("Failed to join thread");
        writer.join().expect("Failed to join thread");

        let guard = state.lock().read();
        assert_eq!(NUM_ITERATIONS, *guard.value(), "The RwLock held the wrong value at the end.");
    }

    #[test]
    fn test_thundering_writes() {
        let state = Arc::new(State::new(NUM_THREADS));
        let mut threads = vec![];
        for _ in 0..NUM_THREADS {
            threads.push(State::spawn_writer(Arc::clone(&state), NUM_ITERATIONS));
        }

        while let Some(thread) = threads.pop() {
            thread.join().expect("failed to join thread");
        }
        let guard = state.lock().read();
        assert_eq!(
            NUM_THREADS * NUM_ITERATIONS,
            *guard.value(),
            "The RwLock held the wrong value at the end."
        );
    }

    #[test]
    fn test_thundering_reads_and_writes() {
        let state = Arc::new(State::new(NUM_THREADS * 2));
        let mut threads = vec![];
        for _ in 0..NUM_THREADS {
            let state = Arc::clone(&state);
            threads.push(State::spawn_writer(Arc::clone(&state), NUM_ITERATIONS));
            threads.push(State::spawn_reader(Arc::clone(&state), NUM_ITERATIONS));
        }

        while let Some(thread) = threads.pop() {
            thread.join().expect("failed to join thread");
        }
        let guard = state.lock().read();
        assert_eq!(
            NUM_THREADS * NUM_ITERATIONS,
            *guard.value(),
            "The RwLock held the wrong value at the end."
        );
    }
}
