// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::sync::atomic::Ordering;

pub struct RawSyncRwLock {
    /// Holds the primary state of the RwLock.
    ///
    /// See the constants declared below for the semantics of this value.
    ///
    /// Readers will zx_futex_wait on this address.
    ///
    /// Ordering rules:
    ///
    ///  * Any store operation which may release the lock must use Ordering::Release on state to
    ///    establish a happens-before relationship with the next lock acquisition.
    ///  * Any load operation which may acquire the lock must use Ordering::Acquire on state to
    ///    establish a happens-before relationship with the previous lock release.
    state: zx::Futex,

    /// The queue of writers waiting to obtain this lock.
    ///
    /// The value of this field is just a generation counter for this queue.
    ///
    /// Writers will zx_futex_wait on this address with the current generation number.
    ///
    /// Ordering rules:
    ///
    ///  * Stores to writer_queue must be preceded by a store to state and use Ordering::Release.
    ///  * Loads from writer_queue must use Ordering::Acquire and be followed by a load of state.
    writer_queue: zx::Futex,
}

const INITIAL_STATE: i32 = 0;

/// If this bit is set in `state`, then the lock is held exclusively (i.e., as a writer) by the
/// thread that set this bit.
const WRITER_BIT: i32 = 0b0001;

/// If this bit is set in `state`, then a writer wished to acquire exclusive access to this lock
/// but observed a reader or a writer holding the lock. The writer will fetch the currentgeneration
/// number for `writer_queue`, re-check `state`, and then zx_futex_wait on the `writer_queue`.
const WRITER_BLOCKED_BIT: i32 = 0b0010;

/// If this bit is set in `state`, then a reader wished to acquire shared access to this lock
/// but could not because either (a) the lock was held exclusively by a writer or (b) a writer
/// was already blocked waiting for the lock. This second condition is necessary to avoid
/// starving writers: once a writer is blocked, readers that could otherwise have acquired
/// shared access to the lock become blocked waiting for at least one writer to acquire the lock.
const READER_BLOCKED_BIT: i32 = 0b0100;

/// The amount `state` is incremented when a reader acquires the lock. The `state` tracks the
/// number of outstanding readers so that once all the readers have released their shared access,
/// the lock can be made available for exclusive access again.
///
/// We count the readers in the high bits of the state so that we can use arithmetic overflow to
/// detect when too many readers have acquired the lock for us to keep track of.
const READER_UNIT: i32 = 0b1000;

/// A mask to select only the bits that count the number of readers holding shared access to the
/// lock.
const READER_MASK: i32 = !0b0111;

/// # STATE MACHINE
///
/// The RwLock goes through the following states:
///
/// ## Initial
///
/// In the "Initial" state, the `state` is zero. No thread has access to the lock and no threads
/// are waiting.
///
/// * If a reader tries to acquire the lock => Shared access (unblocked)
/// * If a writer tries to acquire the lock => Exclusive access (unblocked)
/// * If a previously blocked writer acquires the lock => Exclusive access (writers blocked)
///
/// ## Shared access (unblocked)
///
/// In this state, `state & READER_MASK` is non-zero and other bits are unset. A non-zero
/// number of threads have shared access to the lock and no threads are waiting.
///
/// Additional readers can acquire shared access to the lock without entering the kernel.
///
/// * If a reader tries to acquire the lock => Shared access (unblocked)
/// * If a writer tries to acquire the lock => Shared access (writers blocked)
/// * If the last reader releases the lock => Initial
///
/// ## Shared access (writers blocked)
///
/// In this state, `state & READER_MASK` is non-zero, WRITER_BLOCKED_BIT is set, and other bits are
/// unset. A non-zero number of threads have shared access to the lock and a non-zero number of
/// writers are waiting for exclusive access.
///
/// The lock is contended and requires kernel coordination to wake the blocked threads.
///
/// * If a reader tries to acquire the lock => Shared access (readers and writers blocked)
/// * If a writer tries to acquire the lock => Shared access (writers blocked)
/// * If the last reader releases the lock => Exclusive access (writers blocked)
///
/// ## Shared access (readers and writers blocked)
///
/// In this state, `state & READER_MASK` is non-zero, WRITER_BLOCKED_BIT and READER_BLOCKED_BIT are
/// set, and other bits are unset. A non-zero number of threads have shared access to the lock,
/// a non-zero number of writers are waiting for exclusive access, and a non-zero number of writers
/// are waiting for shared access.
///
/// The lock is contended and requires kernel coordination to wake the blocked threads.
///
/// * If a reader tries to acquire the lock => Shared access (readers and writers blocked)
/// * If a writer tries to acquire the lock => Shared access (readers and writers blocked)
/// * If the last reader releases the lock => Exclusive access (readers and writers blocked)
///
/// ## Exclusive access (unblocked)
///
/// In this state, WRITER_BIT is set and other bits are unset. Exactly one thread has exclusive
/// access to the lock and no threads are waiting.
///
/// The writer can release the lock without entering the kernel.
///
/// * If a reader tries to acquire the lock => Exclusive access (readers and writers blocked)
/// * If a writer tries to acquire the lock => Exclusive access (writers blocked)
/// * If a writer tries to downgrade the lock => Shared access (unblocked)
/// * If the writer releases the lock => Initial
///
/// ## Exclusive access (writers blocked)
///
/// In this state, WRITER_BIT and WRITER_BLOCKED_BIT are set and other bits are unset. Exactly one
/// thread has exclusive access to the lock and zero or more writers are waiting for exclusive
/// access.
///
/// When the writer release the lock, the state transitions to the "Initial state" and then the
/// lock wakes up one of the writers, if any exist. If this previously waiting writer succeeds in
/// acquiring the lock, the state machine returns to the "Exclusive access (writers blocked)" state
/// because we do not know how many writers are blocked waiting for exclusive access.
///
/// * If a reader tries to acquire the lock => Exclusive access (readers and writers blocked)
/// * If a writer tries to acquire the lock => Exclusive access (writers blocked)
/// * If a writer tries to downgrade the lock => Shared access (writers blocked)
/// * If the writer releases the lock => Initial
///
/// ## Exclusive access (readers blocked)
///
/// In this state, WRITER_BIT and READER_BLOCKED_BIT are set and other bits are unset. Exactly one
/// thread has exclusive access to the lock and zero or more writers are waiting for shared
/// access.
///
/// When the writer release the lock, the state transitions to the initial state and then the lock
/// wakes up any blocked readers.
///
/// * If a reader tries to acquire the lock => Exclusive access (readers blocked)
/// * If a writer tries to acquire the lock => Exclusive access (readers and writers blocked)
/// * If a writer tries to downgrade the lock => Unique reader (readers blocked)
/// * If the writer releases the lock => Initial
///
/// ## Exclusive access (readers and writers blocked)
///
/// In this state, WRITER_BIT, WRITER_BLOCKED_BIT, and READER_BLOCKED_BIT are set and other bits
/// are unset. Exactly one thread has exclusive access to the lock and zero or more writers are
/// waiting for exclusive access, and a non-zero number of readers are waiting for shared access.
///
/// The lock is contended and requires kernel coordination to wake the blocked threads.
///
/// * If a reader tries to acquire the lock => Exclusive access (readers and writers blocked)
/// * If a writer tries to acquire the lock => Exclusive access (readers and writers blocked)
/// * If a writer tries to downgrade the lock => Unique reader (readers and writers blocked)
/// * If the writer releases the lock => Unlocked (readers blocked)
///
/// ## Unlocked (readers blocked)
///
/// In this state, READER_BLOCKED_BIT is set and other bits are unset. No thread has access to the
/// lock and a non-zero number of readers are waiting for shared access.
///
/// This state is transitory and the state machine will leave this state without outside
/// intervention by returning to the "Initial" state and waking any blocked readers.
///
/// * If a reader tries to acquire the lock => Unlocked (readers blocked)
/// * If a writer tries to acquire the lock => Exclusive access (readers blocked)
/// * Otherwise => Initial
///
/// ## Unique reader (readers blocked)
///
/// In this state, there is exactly one reader, who is running on the current thread, the
/// READER_BLOCKED_BIT is set and and other bits are unset. A non-zero number of readers are
/// waiting for shared access.
///
/// This state is transitory and the state machine will leave this state without outside
/// intervention by moving to the "Shared access (unblocked)" state and waking any blocked
/// readers.
///
/// * If a reader tries to acquire the lock => Unique reader (readers blocked)
/// * If a writer tries to acquire the lock => Unique reader (readers and writers blocked)
/// * Otherwise => Shared access (unblocked)
///
/// ## Unique reader (readers and writers blocked)
///
/// In this state, there is exactly one reader, who is running on the current thread, and the
/// READER_BLOCKED_BIT and the WRITER_BLOCKED_BIT are set. Zero or more writers are waiting for
/// exclusive access, and a non-zero number of readers are waiting for shared access.
///
/// This state is transitory and the state machine will leave this state without outside
/// intervention by moving to the "Shared access (writers blocked)" state and waking any blocked
/// readers.
///
///
/// * If a reader tries to acquire the lock => Unique reader (readers and writers blocked)
/// * If a writer tries to acquire the lock => Unique reader (readers and writers blocked)
/// * Otherwise => Shared access (writers blocked)

fn is_locked_exclusive(state: i32) -> bool {
    state & WRITER_BIT != 0
}

fn has_blocked_writer(state: i32) -> bool {
    state & WRITER_BLOCKED_BIT != 0
}

fn has_blocked_reader(state: i32) -> bool {
    state & READER_BLOCKED_BIT != 0
}

fn can_lock_shared(state: i32) -> bool {
    !is_locked_exclusive(state) && !has_blocked_writer(state) && !has_blocked_reader(state)
}

fn is_unlocked(state: i32) -> bool {
    state & (WRITER_BIT | READER_MASK) == 0
}

impl RawSyncRwLock {
    #[inline]
    fn try_lock_shared_fast(&self) -> bool {
        let state = self.state.load(Ordering::Relaxed);
        if can_lock_shared(state) {
            if let Some(new_state) = state.checked_add(READER_UNIT) {
                return self
                    .state
                    .compare_exchange(state, new_state, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok();
            }
        }
        false
    }

    #[cold]
    fn lock_shared_slow(&self) {
        let mut state = self.state.load(Ordering::Relaxed);
        loop {
            if can_lock_shared(state) {
                let new_state =
                    state.checked_add(READER_UNIT).expect("overflowed reader count in rwlock");
                match self.state.compare_exchange_weak(
                    state,
                    new_state,
                    Ordering::Acquire,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => return, // Acquired shared lock.
                    Err(observed_state) => {
                        state = observed_state;
                        continue;
                    }
                }
            }

            let desired_sleep_state = state | READER_BLOCKED_BIT;

            if !has_blocked_reader(state) {
                if let Err(observed_state) = self.state.compare_exchange(
                    state,
                    desired_sleep_state,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    state = observed_state;
                    continue;
                }
            }

            // Ignore spurious wakeups, the loop will retry.
            self.state
                .wait(
                    desired_sleep_state,
                    None, // We don't integrate with priority inheritance yet.
                    zx::MonotonicInstant::INFINITE,
                )
                .ok();
            state = self.state.load(Ordering::Relaxed);
        }
    }

    #[cold]
    fn lock_exclusive_slow(&self) {
        let mut state = self.state.load(Ordering::Relaxed);
        let mut other_writers_bit = 0;

        loop {
            if is_unlocked(state) {
                match self.state.compare_exchange_weak(
                    state,
                    state | WRITER_BIT | other_writers_bit,
                    Ordering::Acquire,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => return, // Acquired exclusive lock.
                    Err(observed_state) => {
                        state = observed_state;
                        continue;
                    }
                }
            }

            if !has_blocked_writer(state) {
                if let Err(observed_state) = self.state.compare_exchange(
                    state,
                    state | WRITER_BLOCKED_BIT,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    state = observed_state;
                    continue;
                }
            }

            other_writers_bit = WRITER_BLOCKED_BIT;

            let generation_number = self.writer_queue.load(Ordering::Acquire);

            // Before we go to sleep on the writer_queue at the fetched generation number, we need
            // to make sure that some other thread is going to wake that generation of sleeping
            // writers. If we didn't fetch the state again, it's possible that another thread could
            // have cleared the WRITER_BLOCKED_BIT in the state and incremented the generation
            // number between the last time we observed state and the time we observed the
            // generation number.
            //
            // By observing the WRITER_BLOCKED_BIT *after* fetching the generation number, we
            // ensure that either (a) this generation has already been awoken or (b) whoever clears
            // the WRITER_BLOCKED_BIT bit will wake this generation in the future.
            state = self.state.load(Ordering::Relaxed);

            // If the lock is available or the WRITER_BLOCKED_BIT is missing, try again. No one has
            // promised to wake the observed generation number.
            if is_unlocked(state) || !has_blocked_writer(state) {
                continue;
            }

            // Ignore spurious wakeups here, the loop will retry.
            self.writer_queue
                .wait(
                    generation_number,
                    None, // We don't integrate with priority inheritance yet.
                    zx::MonotonicInstant::INFINITE,
                )
                .ok();

            state = self.state.load(Ordering::Relaxed);
        }
    }

    #[cold]
    fn unlock_slow(&self, mut state: i32) {
        debug_assert!(is_unlocked(state));

        // There are only writers waiting.
        if state == WRITER_BLOCKED_BIT {
            match self.state.compare_exchange(
                state,
                INITIAL_STATE,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    self.wake_writer();
                    // We either made progress by waking a waiter or no one is waiting for this
                    // lock anymore.
                    return;
                }
                Err(observed_state) => {
                    state = observed_state;
                }
            }
        }

        // There are both readers and writers waiting.
        if state == READER_BLOCKED_BIT | WRITER_BLOCKED_BIT {
            // Attempt to clear the WRITER_BLOCKED_BIT.
            if self
                .state
                .compare_exchange(state, READER_BLOCKED_BIT, Ordering::Relaxed, Ordering::Relaxed)
                .is_err()
            {
                // The state changed, which means another thread made progress. We're done.
                return;
            }
            self.wake_writer();
            // We cannot be sure that we actually work up a writer, which means we also need to
            // wake up the readers to avoid the situation where a stack of readers are waiting for
            // a non-existent writer to be done.
            state = READER_BLOCKED_BIT;
        }

        // There are only readers waiting.
        if state == READER_BLOCKED_BIT {
            if self
                .state
                .compare_exchange(state, INITIAL_STATE, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                // Wake up all the readers.
                self.wake_readers();
            }
        }
    }

    #[cold]
    fn downgrade_slow(&self, mut state: i32) {
        debug_assert!(has_blocked_reader(state));
        loop {
            if !has_blocked_reader(state) {
                // Someone else must have woken up the readers.
                return;
            }

            match self.state.compare_exchange(
                state,
                state - READER_BLOCKED_BIT,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    // We cleared the READER_BLOCKED_BIT, so we need to wake the readers.
                    self.wake_readers();
                    return;
                }
                Err(observed_state) => {
                    state = observed_state;
                    continue;
                }
            }
        }
    }

    fn wake_writer(&self) {
        self.writer_queue.fetch_add(1, Ordering::Release);
        // TODO: Track which thread owns this futex for priority inheritance.
        self.writer_queue.wake(1);
    }

    fn wake_readers(&self) {
        self.state.wake_all();
    }
}

unsafe impl lock_api::RawRwLock for RawSyncRwLock {
    const INIT: RawSyncRwLock =
        RawSyncRwLock { state: zx::Futex::new(0), writer_queue: zx::Futex::new(0) };

    // These operations do not need to happen on the same thread.
    // However, we set this to no send to catch mistakes where folks accidentally hold a lock across
    // an async await, which is often not intentional behavior and can lead to a deadlock. If
    // sufficient need is required, this may be changed back to `lock_api::GuardSend`.
    type GuardMarker = lock_api::GuardNoSend;

    #[inline]
    fn lock_shared(&self) {
        if !self.try_lock_shared_fast() {
            self.lock_shared_slow();
        }
    }

    #[inline]
    fn try_lock_shared(&self) -> bool {
        self.try_lock_shared_fast()
    }

    #[inline]
    unsafe fn unlock_shared(&self) {
        let state = self.state.fetch_sub(READER_UNIT, Ordering::Release) - READER_UNIT;

        // If we just released a reader, then we cannot have blocked readers unless we also have
        // blocked writers because, otherwise, the reader would just have acquired the lock.
        debug_assert!(!has_blocked_reader(state) || has_blocked_writer(state));

        // If we were the last reader and there are writers blocked, we need to wake up the blocked
        // writer.
        if is_unlocked(state) && has_blocked_writer(state) {
            self.unlock_slow(state);
        }
    }

    #[inline]
    fn lock_exclusive(&self) {
        if self
            .state
            .compare_exchange_weak(INITIAL_STATE, WRITER_BIT, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            self.lock_exclusive_slow();
        }
    }

    #[inline]
    fn try_lock_exclusive(&self) -> bool {
        self.state
            .compare_exchange(INITIAL_STATE, WRITER_BIT, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    #[inline]
    unsafe fn unlock_exclusive(&self) {
        let state = self.state.fetch_sub(WRITER_BIT, Ordering::Release) - WRITER_BIT;

        // If we just released a writer, then there must not be any readers or writers.
        debug_assert!(is_unlocked(state));

        if has_blocked_reader(state) || has_blocked_writer(state) {
            self.unlock_slow(state);
        }
    }
}

unsafe impl lock_api::RawRwLockDowngrade for RawSyncRwLock {
    #[inline]
    unsafe fn downgrade(&self) {
        let state = self.state.fetch_add(READER_UNIT - WRITER_BIT, Ordering::Release);

        if has_blocked_reader(state) {
            self.downgrade_slow(state);
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::RwLock;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn test_write_and_read() {
        let value = RwLock::<u32>::new(5);
        let mut guard = value.write();
        assert_eq!(*guard, 5);
        *guard = 6;
        assert_eq!(*guard, 6);
        std::mem::drop(guard);

        let guard = value.read();
        assert_eq!(*guard, 6);
    }

    #[test]
    fn test_try_during_read() {
        // Bypass lock cycle detection to ensure try_write & try_read fail as expected.
        let value = lock_api::RwLock::<RawSyncRwLock, u32>::new(5);
        let _read_guard = value.read();
        assert!(value.try_write().is_none());
        assert!(value.try_read().is_some());
    }

    #[test]
    fn test_try_during_write() {
        // Bypass lock cycle detection to ensure try_write & try_read fail as expected.
        let value = lock_api::RwLock::<RawSyncRwLock, u32>::new(5);
        let _write_guard = value.write();
        assert!(value.try_write().is_none());
        assert!(value.try_read().is_none());
    }

    #[test]
    fn test_downgrade() {
        // Bypass lock cycle detection to ensure try_write & try_read fail as expected.
        let value = lock_api::RwLock::<RawSyncRwLock, u32>::new(5);
        let mut guard = value.write();
        assert_eq!(*guard, 5);
        *guard = 6;
        assert_eq!(*guard, 6);
        assert!(value.try_write().is_none());
        assert!(value.try_read().is_none());
        let guard1 = lock_api::RwLockWriteGuard::downgrade(guard);
        assert_eq!(*guard1, 6);
        assert!(value.try_write().is_none());
        let guard2 = value.read();
        assert_eq!(*guard2, 6);
    }

    struct State {
        value: RwLock<u32>,
        gate: zx::Futex,
        writer_count: AtomicUsize,
        reader_count: AtomicUsize,
    }

    impl Default for State {
        fn default() -> Self {
            Self {
                value: Default::default(),
                gate: zx::Futex::new(0),
                writer_count: Default::default(),
                reader_count: Default::default(),
            }
        }
    }

    impl State {
        fn wait_for_gate(&self) {
            while self.gate.load(Ordering::Acquire) == 0 {
                // Ignore failures, we'll retry anyways.
                self.gate.wait(0, None, zx::MonotonicInstant::INFINITE).ok();
            }
        }

        fn open_gate(&self) {
            self.gate.fetch_add(1, Ordering::Release);
            self.gate.wake_all();
        }

        fn spawn_writer(state: Arc<Self>, count: usize) -> std::thread::JoinHandle<()> {
            std::thread::spawn(move || {
                state.wait_for_gate();
                for _ in 0..count {
                    let mut guard = state.value.write();
                    *guard = *guard + 1;
                    let writer_count = state.writer_count.fetch_add(1, Ordering::Acquire) + 1;
                    let reader_count = state.reader_count.load(Ordering::Acquire);
                    state.writer_count.fetch_sub(1, Ordering::Release);
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
                state.wait_for_gate();
                for _ in 0..count {
                    let guard = state.value.read();
                    let observed_value = *guard;
                    let reader_count = state.reader_count.fetch_add(1, Ordering::Acquire) + 1;
                    let writer_count = state.writer_count.load(Ordering::Acquire);
                    state.reader_count.fetch_sub(1, Ordering::Release);
                    std::mem::drop(guard);
                    assert!(observed_value < u32::MAX, "The value inside the RwLock underflowed.");
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
    fn test_thundering_writes() {
        let state = Arc::new(State::default());
        let mut threads = vec![];
        for _ in 0..10 {
            threads.push(State::spawn_writer(Arc::clone(&state), 100));
        }

        // Try to align the thundering herd to stress the RwLock as much as possible.
        std::thread::sleep(std::time::Duration::from_millis(100));
        state.open_gate();

        while let Some(thread) = threads.pop() {
            thread.join().expect("failed to join thread");
        }
        let guard = state.value.read();
        assert_eq!(1000, *guard, "The RwLock held the wrong value at the end.");
    }

    #[test]
    fn test_thundering_reads_and_writes() {
        let state = Arc::new(State::default());
        let mut threads = vec![];
        for _ in 0..10 {
            let state = Arc::clone(&state);
            threads.push(State::spawn_writer(Arc::clone(&state), 100));
            threads.push(State::spawn_reader(Arc::clone(&state), 100));
        }

        // Try to align the thundering herd to stress the RwLock as much as possible.
        std::thread::sleep(std::time::Duration::from_millis(100));
        state.open_gate();

        while let Some(thread) = threads.pop() {
            thread.join().expect("failed to join thread");
        }
        let guard = state.value.read();
        assert_eq!(1000, *guard, "The RwLock held the wrong value at the end.");
    }

    #[test]
    #[cfg(detect_lock_cycles)]
    #[should_panic(expected = "Found cycle in mutex dependency graph")]
    fn test_rwlock_write_write_reentrancy() {
        let value = RwLock::<u32>::new(5);
        let _guard = value.write();
        let _guard2 = value.write();
    }

    #[test]
    #[cfg(detect_lock_cycles)]
    #[should_panic(expected = "Found cycle in mutex dependency graph")]
    fn test_rwlock_read_write_reentrancy() {
        let value = RwLock::<u32>::new(5);
        let _guard = value.read();
        let _guard2 = value.write();
    }

    #[test]
    #[cfg(detect_lock_cycles)]
    #[should_panic(expected = "Found cycle in mutex dependency graph")]
    fn test_rwlock_write_read_reentrancy() {
        let value = RwLock::<u32>::new(5);
        let _guard = value.write();
        let _guard2 = value.read();
    }
}
