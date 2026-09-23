// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mm::memory::MemoryObject;
use crate::mm::{CompareExchangeResult, ProtectionFlags};
use crate::task::{CurrentTask, EventHandler, SignalHandler, SignalHandlerInner, Task, Waiter};
use futures::channel::oneshot;
use starnix_sync::{FutexTableStateLock, InterruptibleEvent, LockDepMutex};
use starnix_types::futex_address::FutexAddress;
use starnix_uapi::errors::Errno;
use starnix_uapi::user_address::UserAddress;
use starnix_uapi::{FUTEX_BITSET_MATCH_ANY, FUTEX_TID_MASK, FUTEX_WAITERS, errno, error};
use std::collections::hash_map::Entry;
use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::sync::{Arc, Weak};

/// A table of futexes.
///
/// Each 32-bit aligned address in an address space can potentially have an associated futex that
/// userspace can wait upon. This table is a sparse representation that has an actual WaitQueue
/// only for those addresses that have ever actually had a futex operation performed on them.
pub struct FutexTable<Key: FutexKey> {
    /// The futexes associated with each address in each VMO.
    ///
    /// This HashMap is populated on-demand when futexes are used.
    state: LockDepMutex<FutexTableState<Key>, FutexTableStateLock>,
}

impl<Key: FutexKey> Default for FutexTable<Key> {
    fn default() -> Self {
        Self { state: LockDepMutex::new(FutexTableState::default()) }
    }
}

impl<Key: FutexKey> FutexTable<Key> {
    /// Resolves the outcome of a blocking wait on this table.
    ///
    /// On success, the waker has already removed the waiter from the queue and there is nothing
    /// left to do. On error (e.g. ETIMEDOUT, EINTR), the waiter must remove itself from the queue
    /// to prevent a memory leak.
    ///
    /// Finding nothing to remove means a concurrent wake dequeued the waiter while it was waking
    /// up for another reason. That wake counted this waiter as woken, so report success rather
    /// than dropping the wake on the floor.
    fn resolve_wait(
        &self,
        result: Result<(), Errno>,
        key: Key,
        matcher: WaiterMatcher<'_>,
    ) -> Result<(), Errno> {
        result.or_else(|e| {
            if self.state.lock().remove_waiter_from_queue(key, &matcher) { Err(e) } else { Ok(()) }
        })
    }

    /// Wait on the futex at the given address given a boot deadline.
    ///
    /// See FUTEX_WAIT when passed a deadline in CLOCK_REALTIME.
    pub fn wait_boot(
        &self,
        current_task: &CurrentTask,
        addr: UserAddress,
        value: u32,
        mask: u32,
        deadline: zx::BootInstant,
        timer_slack: zx::BootDuration,
    ) -> Result<(), Errno> {
        let addr = FutexAddress::try_from(addr)?;
        // Pre-fault the address before acquiring the FutexTable lock to make page faults or
        // memory reclamation under the lock highly unlikely.
        // TODO(https://fxbug.dev/548025031): Implement a non-blocking try_load and retry loop once
        // supported by Zircon.
        let _ = current_task.mm()?.atomic_load_u32_acquire(addr)?;
        let mut state = self.state.lock();
        // As the state is locked, no wake can happen before the waiter is registered.
        // If the addr is remapped, we will read stale data, but we will not miss a futex wake.
        // Acquire ordering to synchronize with userspace modifications to the value on other
        // threads.
        let loaded_value = current_task.mm()?.atomic_load_u32_acquire(addr)?;
        if value != loaded_value {
            return error!(EAGAIN);
        }

        let key = Key::get(current_task, addr)?;
        let waiter = Arc::new(Waiter::new());
        let timer = zx::BootTimer::create();
        let signal_handler = SignalHandler {
            inner: SignalHandlerInner::None,
            event_handler: EventHandler::None,
            err_code: Some(errno!(ETIMEDOUT)),
        };
        waiter
            .wake_on_zircon_signals(&timer, zx::Signals::TIMER_SIGNALED, signal_handler)
            .expect("wait can only fail in OOM conditions");
        timer
            .set(deadline, timer_slack)
            .expect("timer set cannot fail with valid handles and slack");
        state.get_waiters_or_default(key.clone()).add(FutexWaiter {
            mask,
            notifiable: FutexNotifiable::new_internal_boot(Arc::downgrade(&waiter)),
        });
        std::mem::drop(state);

        let result = waiter.wait(current_task);
        self.resolve_wait(result, key, WaiterMatcher::BootWaiter(&waiter))
    }

    /// Wait on the futex at the given address.
    ///
    /// See FUTEX_WAIT.
    pub fn wait(
        &self,
        current_task: &CurrentTask,
        addr: UserAddress,
        value: u32,
        mask: u32,
        deadline: zx::MonotonicInstant,
    ) -> Result<(), Errno> {
        let addr = FutexAddress::try_from(addr)?;
        // Pre-fault the address before acquiring the FutexTable lock to make page faults or
        // memory reclamation under the lock highly unlikely.
        // TODO(https://fxbug.dev/548025031): Implement a non-blocking try_load and retry loop once
        // supported by Zircon.
        let _ = current_task.mm()?.atomic_load_u32_acquire(addr)?;
        let mut state = self.state.lock();
        // As the state is locked, no wake can happen before the waiter is registered.
        // If the addr is remapped, we will read stale data, but we will not miss a futex wake.
        // Acquire ordering to synchronize with userspace modifications to the value on other
        // threads.
        let loaded_value = current_task.mm()?.atomic_load_u32_acquire(addr)?;
        if value != loaded_value {
            return error!(EAGAIN);
        }

        let key = Key::get(current_task, addr)?;
        let event = InterruptibleEvent::new();
        let guard = event.begin_wait();
        state.get_waiters_or_default(key.clone()).add(FutexWaiter {
            mask,
            notifiable: FutexNotifiable::new_internal(Arc::downgrade(&event)),
        });
        std::mem::drop(state);

        let result = current_task.block_until(guard, deadline);
        self.resolve_wait(result, key, WaiterMatcher::Event(&event))
    }

    /// Wake the given number of waiters on futex at the given address. Returns the number of
    /// waiters actually woken.
    ///
    /// See FUTEX_WAKE.
    pub fn wake(
        &self,
        task: &Task,
        addr: UserAddress,
        count: usize,
        mask: u32,
    ) -> Result<usize, Errno> {
        let addr = FutexAddress::try_from(addr)?;
        let key = Key::get(task, addr)?;
        Ok(self.state.lock().wake(key, count, mask))
    }

    /// Requeue the waiters to another address.
    ///
    /// See FUTEX_CMP_REQUEUE
    pub fn requeue(
        &self,
        current_task: &CurrentTask,
        addr: UserAddress,
        wake_count: usize,
        requeue_count: usize,
        new_addr: UserAddress,
        expected_value: Option<u32>,
    ) -> Result<usize, Errno> {
        let addr = FutexAddress::try_from(addr)?;
        let new_addr = FutexAddress::try_from(new_addr)?;
        if expected_value.is_some() {
            // Pre-fault the address before acquiring the FutexTable lock to make page faults or
            // memory reclamation under the lock highly unlikely.
            // TODO(https://fxbug.dev/548025031): Implement a non-blocking try_load and retry loop once
            // supported by Zircon.
            let _ = current_task.mm()?.atomic_load_u32_acquire(addr)?;
        }
        let key = Key::get(current_task, addr)?;
        let new_key = Key::get(current_task, new_addr)?;
        let mut state = self.state.lock();
        if let Some(expected) = expected_value {
            // Use acquire ordering here to synchronize with mutex impls that store w/ release
            // ordering.
            let value = current_task.mm()?.atomic_load_u32_acquire(addr)?;
            if value != expected {
                return error!(EAGAIN);
            }
        }

        Ok(state.requeue(key, new_key, wake_count, requeue_count))
    }

    /// Lock the futex at the given address.
    ///
    /// See FUTEX_LOCK_PI.
    pub fn lock_pi(
        &self,
        current_task: &CurrentTask,
        addr: UserAddress,
        deadline: zx::MonotonicInstant,
    ) -> Result<(), Errno> {
        let addr = FutexAddress::try_from(addr)?;
        let mm = current_task.mm()?;
        // Perform a dummy CAS to pre-fault the page with write permissions (e.g. for COW pages)
        // before acquiring the FutexTable lock to make page faults or memory reclamation under
        // the lock highly unlikely.
        // TODO(https://fxbug.dev/548025031): Implement a non-blocking try_load and retry loop once
        // supported by Zircon.
        if let Ok(current) = mm.atomic_load_u32_relaxed(addr) {
            let _ = mm.atomic_compare_exchange_u32_acq_rel(addr, current, current);
        }
        let mut state = self.state.lock();
        // As the state is locked, no unlock can happen before the waiter is registered.
        // If the addr is remapped, we will read stale data, but we will not miss a futex unlock.
        let key = Key::get(current_task, addr)?;

        let tid = current_task.get_tid() as u32;

        // Use a relaxed ordering because the compare/exchange below creates a synchronization
        // point with userspace threads in the success case. No synchronization is required in
        // failure cases.
        let mut current_value = mm.atomic_load_u32_relaxed(addr)?;
        let new_owner_tid = loop {
            let new_owner_tid = current_value & FUTEX_TID_MASK;
            if new_owner_tid == tid {
                // From <https://man7.org/linux/man-pages/man2/futex.2.html>:
                //
                //   EDEADLK
                //          (FUTEX_LOCK_PI, FUTEX_LOCK_PI2, FUTEX_TRYLOCK_PI,
                //          FUTEX_CMP_REQUEUE_PI) The futex word at uaddr is
                //          already locked by the caller.
                return error!(EDEADLOCK);
            }

            if current_value == 0 {
                // Use acq/rel ordering to synchronize with acquire ordering on userspace lock ops
                // and with the release ordering on userspace unlock ops.
                match mm.atomic_compare_exchange_weak_u32_acq_rel(addr, current_value, tid) {
                    CompareExchangeResult::Success => return Ok(()),
                    CompareExchangeResult::Stale { observed } => {
                        current_value = observed;
                        continue;
                    }
                    CompareExchangeResult::Error(e) => return Err(e),
                }
            }

            // Use acq/rel ordering to synchronize with acquire ordering on userspace lock ops and
            // with the release ordering on userspace unlock ops.
            let target_value = current_value | FUTEX_WAITERS;
            match mm.atomic_compare_exchange_u32_acq_rel(addr, current_value, target_value) {
                CompareExchangeResult::Success => (),
                CompareExchangeResult::Stale { observed } => {
                    current_value = observed;
                    continue;
                }
                CompareExchangeResult::Error(e) => return Err(e),
            }
            break new_owner_tid;
        };

        let event = InterruptibleEvent::new();
        let guard = event.begin_wait();
        let notifiable = FutexNotifiable::new_internal(Arc::downgrade(&event));
        state
            .get_rt_mutex_waiters_or_default(key.clone())
            .push_back(RtMutexWaiter { tid, notifiable });
        std::mem::drop(state);

        // ESRCH  (FUTEX_LOCK_PI, FUTEX_LOCK_PI2, FUTEX_TRYLOCK_PI,
        //        FUTEX_CMP_REQUEUE_PI) The thread ID in the futex word at
        //        uaddr does not exist.
        current_task
            .get_task(new_owner_tid as i32)
            .ok()
            .and_then(|o| o.running_state().unwrap().thread.get().map(|t| Arc::clone(&t.thread)))
            .map_or_else(
                || error!(ESRCH),
                |owner| current_task.block_with_owner_until(guard, &owner, deadline),
            )
            .inspect_err(|_| {
                // If block_with_owner_until returned an error (e.g., ETIMEDOUT), or if we
                // failed to find the new owner (ESRCH), we must explicitly remove our waiter
                // from the PI-mutex queue to prevent a memory leak.
                self.state
                    .lock()
                    .remove_rt_mutex_waiter_from_queue(key, &WaiterMatcher::Event(&event));
            })
    }

    /// Unlock the futex at the given address.
    ///
    /// See FUTEX_UNLOCK_PI.
    pub fn unlock_pi(&self, current_task: &CurrentTask, addr: UserAddress) -> Result<(), Errno> {
        let addr = FutexAddress::try_from(addr)?;
        let mm = current_task.mm()?;
        // Perform a placeholder CAS to pre-fault the page with write permissions (e.g. for COW pages)
        // before acquiring the FutexTable lock to make page faults or memory reclamation under
        // the lock highly unlikely.
        // TODO(https://fxbug.dev/548025031): Implement a non-blocking try_load and retry loop once
        // supported by Zircon.
        if let Ok(current) = mm.atomic_load_u32_relaxed(addr) {
            let _ = mm.atomic_compare_exchange_u32_acq_rel(addr, current, current);
        }
        let mut state = self.state.lock();
        let tid = current_task.get_tid() as u32;

        let key = Key::get(current_task, addr)?;

        // Use a relaxed ordering because the compare/exchange below creates a synchronization
        // point with userspace threads in the success case. No synchronization is required in
        // failure cases.
        let current_value = mm.atomic_load_u32_relaxed(addr)?;
        if current_value & FUTEX_TID_MASK != tid {
            // From <https://man7.org/linux/man-pages/man2/futex.2.html>:
            //
            //   EPERM  (FUTEX_UNLOCK_PI) The caller does not own the lock
            //          represented by the futex word.
            return error!(EPERM);
        }

        loop {
            let maybe_waiter = state.pop_rt_mutex_waiter(key.clone());
            let target_value = if let Some(waiter) = &maybe_waiter { waiter.tid } else { 0 };

            // Use acq/rel ordering to synchronize with acquire ordering on userspace lock ops and
            // with the release ordering on userspace unlock ops.
            match mm.atomic_compare_exchange_u32_acq_rel(addr, current_value, target_value) {
                CompareExchangeResult::Success => (),
                // From <https://man7.org/linux/man-pages/man2/futex.2.html>:
                //
                //   EINVAL (FUTEX_LOCK_PI, FUTEX_LOCK_PI2, FUTEX_TRYLOCK_PI,
                //       FUTEX_UNLOCK_PI) The kernel detected an inconsistency
                //       between the user-space state at uaddr and the kernel
                //       state.  This indicates either state corruption or that the
                //       kernel found a waiter on uaddr which is waiting via
                //       FUTEX_WAIT or FUTEX_WAIT_BITSET.
                CompareExchangeResult::Stale { .. } => return error!(EINVAL),
                // From <https://man7.org/linux/man-pages/man2/futex.2.html>:
                //
                //   EACCES No read access to the memory of a futex word.
                CompareExchangeResult::Error(_) => return error!(EACCES),
            }

            let Some(mut waiter) = maybe_waiter else {
                // We can stop trying to notify a thread if there are no more waiters.
                break;
            };

            if waiter.notifiable.notify() {
                break;
            }

            // If we couldn't notify the waiter, then we need to pull the next thread off the
            // waiter list.
        }

        Ok(())
    }
}

impl FutexTable<SharedFutexKey> {
    /// Wait on the futex at the given offset in the memory.
    ///
    /// Returns a receiver that will be signaled when the futex is woken, and an
    /// `Arc<()>` token that must be kept alive by the caller for the duration of the
    /// wait. If the caller drops the token (e.g., if the external client
    /// disconnects), the waiter is marked as stale and will be garbage-collected by the
    /// next futex operation on this table.
    ///
    /// See FUTEX_WAIT.
    pub fn external_wait(
        &self,
        memory: MemoryObject,
        offset: u64,
        value: u32,
        mask: u32,
    ) -> Result<(Arc<()>, oneshot::Receiver<()>), Errno> {
        let key = SharedFutexKey::new(&memory, offset);
        let mut state = self.state.lock();
        // As the state is locked, no wake can happen before the waiter is registered.
        Self::external_check_futex_value(&memory, offset, value)?;

        let token = Arc::new(());
        let (sender, receiver) = oneshot::channel::<()>();
        state.get_waiters_or_default(key).add(FutexWaiter {
            mask,
            notifiable: FutexNotifiable::new_external(Arc::downgrade(&token), sender),
        });
        Ok((token, receiver))
    }

    /// Wake the given number of waiters on futex at the given offset in the memory. Returns the
    /// number of waiters actually woken.
    ///
    /// See FUTEX_WAKE.
    pub fn external_wake(
        &self,
        memory: MemoryObject,
        offset: u64,
        count: usize,
        mask: u32,
    ) -> Result<usize, Errno> {
        Ok(self.state.lock().wake(SharedFutexKey::new(&memory, offset), count, mask))
    }

    pub fn external_requeue(
        &self,
        first_memory: MemoryObject,
        first_offset: u64,
        second_memory: Option<MemoryObject>,
        second_offset: u64,
        wake_count: usize,
        requeue_count: usize,
        expected_value: Option<u32>,
    ) -> Result<usize, Errno> {
        let first_key = SharedFutexKey::new(&first_memory, first_offset);
        let second_key = match second_memory.as_ref() {
            Some(second_memory) => SharedFutexKey::new(second_memory, second_offset),
            None => SharedFutexKey::new(&first_memory, second_offset),
        };
        // If/when we move from a single table mutex to a mutex per futex, we'll likely want to
        // define a consistent SharedFutexKey sort order independent of which is "first" and which
        // is "second" in this call. Then we can acquire each of the two mutexes corresponding to
        // each of the two futexes per that sort order. This way, we can be holding both mutexes to
        // make the requeue atomic despite each futex having its own mutex, while avoiding
        // deadlocks. But for now we lock the whole FutexTable.
        let mut state = self.state.lock();
        if let Some(expected) = expected_value {
            // The state being locked is how this is included in the set of atomic changes.
            Self::external_check_futex_value(&first_memory, first_offset, expected)?;
        }
        Ok(state.requeue(first_key, second_key, wake_count, requeue_count))
    }

    fn external_check_futex_value(
        memory: &MemoryObject,
        offset: u64,
        value: u32,
    ) -> Result<(), Errno> {
        let loaded_value = {
            // TODO: This read should be atomic.
            let mut buf = [0u8; 4];
            memory.read(&mut buf, offset).map_err(|_| errno!(EINVAL))?;
            u32::from_ne_bytes(buf)
        };
        if loaded_value != value {
            return error!(EAGAIN);
        }
        Ok(())
    }
}

pub trait FutexKey: Sized + Ord + Hash + Clone {
    fn get(task: &Task, addr: FutexAddress) -> Result<Self, Errno>;
    fn get_table_from_task(task: &Task) -> Result<Arc<FutexTable<Self>>, Errno>;
}

#[derive(Debug, Clone, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct PrivateFutexKey {
    addr: FutexAddress,
}

impl FutexKey for PrivateFutexKey {
    fn get(_task: &Task, addr: FutexAddress) -> Result<Self, Errno> {
        Ok(PrivateFutexKey { addr })
    }

    fn get_table_from_task(task: &Task) -> Result<Arc<FutexTable<Self>>, Errno> {
        Ok(task.mm()?.futex.clone())
    }
}

#[derive(Debug, Clone, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct SharedFutexKey {
    // No chance of collisions since koids are never reused:
    // https://fuchsia.dev/fuchsia-src/concepts/kernel/concepts#kernel_object_ids
    koid: zx::Koid,
    offset: u64,
}

impl FutexKey for SharedFutexKey {
    fn get(task: &Task, addr: FutexAddress) -> Result<Self, Errno> {
        let (memory, offset) = task.mm()?.get_mapping_memory(addr.into(), ProtectionFlags::READ)?;
        Ok(SharedFutexKey::new(&memory, offset))
    }

    fn get_table_from_task(task: &Task) -> Result<Arc<FutexTable<Self>>, Errno> {
        Ok(task.kernel().shared_futexes.clone())
    }
}

impl SharedFutexKey {
    fn new(memory: &MemoryObject, offset: u64) -> Self {
        Self { koid: memory.get_koid(), offset }
    }
}

struct FutexTableState<Key: FutexKey> {
    waiters: HashMap<Key, FutexWaiters>,
    rt_mutex_waiters: HashMap<Key, VecDeque<RtMutexWaiter>>,
}

impl<Key: FutexKey> Default for FutexTableState<Key> {
    fn default() -> Self {
        Self { waiters: Default::default(), rt_mutex_waiters: Default::default() }
    }
}

impl<Key: FutexKey> FutexTableState<Key> {
    /// Returns the FutexWaiters for a given address, creating an empty one if none is registered.
    fn get_waiters_or_default(&mut self, key: Key) -> &mut FutexWaiters {
        self.waiters.entry(key).or_default()
    }

    fn wake(&mut self, key: Key, count: usize, mask: u32) -> usize {
        let entry = self.waiters.entry(key);
        match entry {
            Entry::Vacant(_) => 0,
            Entry::Occupied(mut entry) => {
                let count = entry.get_mut().notify(mask, count);
                if entry.get().is_empty() {
                    entry.remove();
                }
                count
            }
        }
    }

    fn requeue(
        &mut self,
        key: Key,
        new_key: Key,
        wake_count: usize,
        requeue_count: usize,
    ) -> usize {
        let woken;
        let to_requeue;
        match self.waiters.entry(key) {
            Entry::Vacant(_) => return 0,
            Entry::Occupied(mut entry) => {
                // Wake up at most `wake_count` waiters.
                woken = entry.get_mut().notify(FUTEX_BITSET_MATCH_ANY, wake_count);

                // Dequeue up to `requeue_count` waiters to requeue below.
                to_requeue = entry.get_mut().split_for_requeue(requeue_count);

                if entry.get().is_empty() {
                    entry.remove();
                }
            }
        }

        let requeued = to_requeue.0.len();
        if !to_requeue.is_empty() {
            self.get_waiters_or_default(new_key).transfer(to_requeue);
        }

        woken + requeued
    }

    /// Returns the RT-Mutex waiters queue for a given address, creating an empty queue if none is
    /// registered.
    fn get_rt_mutex_waiters_or_default(&mut self, key: Key) -> &mut VecDeque<RtMutexWaiter> {
        self.rt_mutex_waiters.entry(key).or_default()
    }

    /// Pop the next RT-Mutex for the given address.
    fn pop_rt_mutex_waiter(&mut self, key: Key) -> Option<RtMutexWaiter> {
        let entry = self.rt_mutex_waiters.entry(key);
        match entry {
            Entry::Vacant(_) => None,
            Entry::Occupied(mut entry) => {
                let mut waiter = entry.get_mut().pop_front();
                // Clean up the hash map entry if the queue is empty. We do this
                // regardless of whether `pop_front` returned a waiter or `None`,
                // effectively garbage collecting erroneously empty map entries.
                if entry.get().is_empty() {
                    entry.remove();
                } else if let Some(waiter) = &mut waiter {
                    waiter.tid |= FUTEX_WAITERS;
                }
                waiter
            }
        }
    }

    /// Removes the waiter designated by `matcher` from the queue it is waiting on.
    ///
    /// This uses a two-step approach:
    /// 1. O(1) Fast path: Check the `key` where the waiter originally went to sleep.
    /// 2. O(N) Fallback: If not found (e.g. moved via `FUTEX_REQUEUE`), scan all futexes.
    ///
    /// Returns whether the waiter was still queued. A `false` return means the waiter was
    /// already dequeued by a concurrent wake.
    fn remove_waiter_from_queue(&mut self, key: Key, matcher: &WaiterMatcher<'_>) -> bool {
        if let Entry::Occupied(mut entry) = self.waiters.entry(key) {
            let found = entry.get_mut().remove_waiter(matcher);
            if entry.get().is_empty() {
                entry.remove();
            }
            if found {
                return true;
            }
        }

        let mut found = false;
        let mut key_to_remove = None;
        for (key, waiters) in self.waiters.iter_mut() {
            if waiters.remove_waiter(matcher) {
                found = true;
                if waiters.is_empty() {
                    key_to_remove = Some(key.clone());
                }
                break;
            }
        }
        if let Some(key) = key_to_remove {
            self.waiters.remove(&key);
        }
        found
    }

    /// Removes a PI-mutex (`FUTEX_LOCK_PI`) waiter.
    ///
    /// Operates on the separate `rt_mutex_waiters` map using the same two-step
    /// O(1)/O(N) algorithm as the other removal methods to handle edge cases where
    /// PI-mutexes might be requeued (e.g. if `FUTEX_CMP_REQUEUE_PI` is used).
    fn remove_rt_mutex_waiter_from_queue(&mut self, key: Key, matcher: &WaiterMatcher<'_>) {
        let predicate =
            |w: &RtMutexWaiter| !matcher.matches(&w.notifiable) && !w.notifiable.is_stale();

        if let Entry::Occupied(mut entry) = self.rt_mutex_waiters.entry(key) {
            let len_before = entry.get().len();
            entry.get_mut().retain(predicate);
            if entry.get().len() < len_before {
                if entry.get().is_empty() {
                    entry.remove();
                }
                return;
            }
        }

        let mut key_to_remove = None;
        for (key, waiters) in self.rt_mutex_waiters.iter_mut() {
            let len_before = waiters.len();
            waiters.retain(predicate);
            if waiters.len() < len_before {
                if waiters.is_empty() {
                    key_to_remove = Some(key.clone());
                }
                break;
            }
        }
        if let Some(key) = key_to_remove {
            self.rt_mutex_waiters.remove(&key);
        }
    }
}

/// Designates the waiter a removal operation is looking for.
///
/// The queues mix the waiters of every flavor of `FUTEX_WAIT`, so a removal needs to describe
/// which one of them belongs to the caller.
enum WaiterMatcher<'a> {
    /// The waiter blocked on the given `InterruptibleEvent`. See `FutexNotifiable::Internal`.
    Event(&'a Arc<InterruptibleEvent>),

    /// The waiter blocked on the given `Waiter`. See `FutexNotifiable::InternalBoot`.
    BootWaiter(&'a Arc<Waiter>),
}

impl WaiterMatcher<'_> {
    /// Returns whether `notifiable` designates the waiter this matcher is looking for.
    fn matches(&self, notifiable: &FutexNotifiable) -> bool {
        match (self, notifiable) {
            (Self::Event(event), FutexNotifiable::Internal(weak)) => {
                weak.upgrade().is_some_and(|strong| Arc::ptr_eq(&strong, event))
            }
            (Self::BootWaiter(waiter), FutexNotifiable::InternalBoot(weak)) => {
                weak.upgrade().is_some_and(|strong| Arc::ptr_eq(&strong, waiter))
            }
            _ => false,
        }
    }
}

/// Abstraction over a process waiting on a Futex that can be notified.
enum FutexNotifiable {
    /// An internal process waiting on a Futex.
    Internal(Weak<InterruptibleEvent>),
    // An internal process waiting on a Futex with a boot deadline.
    InternalBoot(Weak<Waiter>),
    /// An external process waiting on a Futex.
    // The sender needs to be an option so that one can send the notification while only holding a
    // mut reference on the ExternalWaiter.
    External(Weak<()>, Option<oneshot::Sender<()>>),
}

impl FutexNotifiable {
    fn new_internal(event: Weak<InterruptibleEvent>) -> Self {
        Self::Internal(event)
    }

    fn new_internal_boot(waiter: Weak<Waiter>) -> Self {
        Self::InternalBoot(waiter)
    }

    fn new_external(token: Weak<()>, sender: oneshot::Sender<()>) -> Self {
        Self::External(token, Some(sender))
    }

    /// Tries to notify the process. Returns `true` is the process have been notified. Returns
    /// `false` otherwise. This means the process is stale and will never be available again.
    fn notify(&mut self) -> bool {
        match self {
            Self::Internal(event) => {
                if let Some(event) = event.upgrade() {
                    event.notify();
                    true
                } else {
                    false
                }
            }
            Self::InternalBoot(waiter) => {
                if let Some(waiter) = waiter.upgrade() {
                    waiter.notify();
                    true
                } else {
                    false
                }
            }
            Self::External(_, sender) => {
                if let Some(sender) = sender.take() {
                    sender.send(()).is_ok()
                } else {
                    false
                }
            }
        }
    }

    fn is_stale(&self) -> bool {
        match self {
            Self::Internal(weak) => weak.strong_count() == 0,
            Self::External(weak, _) => weak.strong_count() == 0,
            Self::InternalBoot(weak) => weak.strong_count() == 0,
        }
    }
}

struct FutexWaiter {
    mask: u32,
    notifiable: FutexNotifiable,
}

#[derive(Default)]
struct FutexWaiters(VecDeque<FutexWaiter>);

impl FutexWaiters {
    fn add(&mut self, waiter: FutexWaiter) {
        self.0.push_back(waiter);
    }

    fn notify(&mut self, mask: u32, count: usize) -> usize {
        let mut woken = 0;
        self.0.retain_mut(|waiter| {
            if woken == count || waiter.mask & mask == 0 {
                return true;
            }
            // The send will fail if the receiver is gone, which means nothing was actualling
            // waiting on the futex.
            if waiter.notifiable.notify() {
                woken += 1;
            }
            false
        });
        woken
    }

    fn transfer(&mut self, mut other: Self) {
        self.0.append(&mut other.0);
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Removes the waiter designated by `matcher` from the queue, also garbage collecting stale
    /// waiters.
    ///
    /// Returns whether that waiter was in the queue.
    fn remove_waiter(&mut self, matcher: &WaiterMatcher<'_>) -> bool {
        let mut found = false;
        self.0.retain(|w| {
            if matcher.matches(&w.notifiable) {
                found = true;
                return false;
            }
            !w.notifiable.is_stale()
        });
        found
    }

    fn split_for_requeue(&mut self, count: usize) -> Self {
        let count = std::cmp::min(count, self.0.len());
        let tail = self.0.split_off(count);
        let head = std::mem::replace(&mut self.0, tail);
        FutexWaiters(head)
    }
}

struct RtMutexWaiter {
    /// The tid, possibly with the FUTEX_WAITERS bit set.
    tid: u32,

    notifiable: FutexNotifiable,
}

#[cfg(test)]
mod tests {
    use super::*;
    use starnix_sync::InterruptibleEvent;
    use starnix_uapi::restricted_aspace::RESTRICTED_ASPACE_BASE;
    use starnix_uapi::user_address::UserAddress;

    #[fuchsia::test]
    fn test_remove_waiter_simple() {
        let mut state = FutexTableState::<PrivateFutexKey>::default();
        let key = PrivateFutexKey {
            addr: FutexAddress::try_from(UserAddress::from(
                (RESTRICTED_ASPACE_BASE + 0x1000) as u64,
            ))
            .unwrap(),
        };
        let event = Arc::new(InterruptibleEvent::new());

        state.get_waiters_or_default(key.clone()).add(FutexWaiter {
            mask: u32::MAX,
            notifiable: FutexNotifiable::new_internal(Arc::downgrade(&event)),
        });

        assert_eq!(state.waiters.len(), 1);
        assert!(state.remove_waiter_from_queue(key, &WaiterMatcher::Event(&event)));
        assert_eq!(state.waiters.len(), 0);
    }

    #[fuchsia::test]
    fn test_remove_waiter_requeued() {
        let mut state = FutexTableState::<PrivateFutexKey>::default();
        let key1 = PrivateFutexKey {
            addr: FutexAddress::try_from(UserAddress::from(
                (RESTRICTED_ASPACE_BASE + 0x1000) as u64,
            ))
            .unwrap(),
        };
        let key2 = PrivateFutexKey {
            addr: FutexAddress::try_from(UserAddress::from(
                (RESTRICTED_ASPACE_BASE + 0x2000) as u64,
            ))
            .unwrap(),
        };
        let event = Arc::new(InterruptibleEvent::new());

        state.get_waiters_or_default(key2.clone()).add(FutexWaiter {
            mask: u32::MAX,
            notifiable: FutexNotifiable::new_internal(Arc::downgrade(&event)),
        });

        assert_eq!(state.waiters.len(), 1);
        assert!(state.remove_waiter_from_queue(key1, &WaiterMatcher::Event(&event)));
        assert_eq!(state.waiters.len(), 0);
    }

    /// A wake that races with an interruption must not be lost.
    ///
    /// When a signal interrupts a waiter, the waiter can still be sitting in the queue when
    /// another thread issues a wake. That wake counts the waiter as woken, so the waiter must
    /// notice that it has been dequeued and report the wake instead of the interruption.
    #[fuchsia::test]
    fn test_wake_racing_with_interruption_is_not_lost() {
        let mut state = FutexTableState::<PrivateFutexKey>::default();
        let key = PrivateFutexKey {
            addr: FutexAddress::try_from(UserAddress::from(
                (RESTRICTED_ASPACE_BASE + 0x1000) as u64,
            ))
            .unwrap(),
        };
        let event = InterruptibleEvent::new();
        let _guard = event.begin_wait();

        state.get_waiters_or_default(key.clone()).add(FutexWaiter {
            mask: u32::MAX,
            notifiable: FutexNotifiable::new_internal(Arc::downgrade(&event)),
        });

        // A signal interrupts the waiter before it is dequeued.
        event.interrupt();

        // The waker still sees the waiter in the queue and counts it as woken.
        assert_eq!(state.wake(key.clone(), 1, FUTEX_BITSET_MATCH_ANY), 1);

        // The interrupted waiter must therefore observe that it is no longer queued, which tells
        // it to report the wake rather than the interruption.
        assert!(!state.remove_waiter_from_queue(key, &WaiterMatcher::Event(&event)));
    }

    /// The boot-deadline flavor of `FUTEX_WAIT` must resolve the same race the same way.
    #[fuchsia::test]
    fn test_wake_racing_with_boot_waiter_removal_is_not_lost() {
        let mut state = FutexTableState::<PrivateFutexKey>::default();
        let key = PrivateFutexKey {
            addr: FutexAddress::try_from(UserAddress::from(
                (RESTRICTED_ASPACE_BASE + 0x1000) as u64,
            ))
            .unwrap(),
        };
        let waiter = Arc::new(Waiter::new());

        state.get_waiters_or_default(key.clone()).add(FutexWaiter {
            mask: u32::MAX,
            notifiable: FutexNotifiable::new_internal_boot(Arc::downgrade(&waiter)),
        });

        // The waker dequeues the waiter and counts it as woken, whether or not the waiter was
        // about to give up on its deadline.
        assert_eq!(state.wake(key.clone(), 1, FUTEX_BITSET_MATCH_ANY), 1);

        assert!(!state.remove_waiter_from_queue(key, &WaiterMatcher::BootWaiter(&waiter)));
    }

    /// A waiter must only be removed by a matcher of its own kind.
    ///
    /// The two kinds share the same queues, so a mismatched removal must neither report a match
    /// nor dequeue the waiter, which would turn the next wake into a lost one.
    #[fuchsia::test]
    fn test_remove_waiter_ignores_other_waiter_kinds() {
        let mut state = FutexTableState::<PrivateFutexKey>::default();
        let key = PrivateFutexKey {
            addr: FutexAddress::try_from(UserAddress::from(
                (RESTRICTED_ASPACE_BASE + 0x1000) as u64,
            ))
            .unwrap(),
        };
        let event = InterruptibleEvent::new();
        let waiter = Arc::new(Waiter::new());

        state.get_waiters_or_default(key.clone()).add(FutexWaiter {
            mask: u32::MAX,
            notifiable: FutexNotifiable::new_internal_boot(Arc::downgrade(&waiter)),
        });

        assert!(!state.remove_waiter_from_queue(key.clone(), &WaiterMatcher::Event(&event)));
        assert_eq!(state.waiters.len(), 1);

        assert!(state.remove_waiter_from_queue(key, &WaiterMatcher::BootWaiter(&waiter)));
        assert_eq!(state.waiters.len(), 0);
    }

    #[fuchsia::test]
    fn test_remove_rt_mutex_waiter() {
        let mut state = FutexTableState::<PrivateFutexKey>::default();
        let key = PrivateFutexKey {
            addr: FutexAddress::try_from(UserAddress::from(
                (RESTRICTED_ASPACE_BASE + 0x1000) as u64,
            ))
            .unwrap(),
        };
        let event = Arc::new(InterruptibleEvent::new());

        state.get_rt_mutex_waiters_or_default(key.clone()).push_back(RtMutexWaiter {
            tid: 1,
            notifiable: FutexNotifiable::new_internal(Arc::downgrade(&event)),
        });

        assert_eq!(state.rt_mutex_waiters.len(), 1);
        state.remove_rt_mutex_waiter_from_queue(key, &WaiterMatcher::Event(&event));
        assert_eq!(state.rt_mutex_waiters.len(), 0);
    }

    #[fuchsia::test]
    fn test_split_for_requeue_fairness() {
        let mut waiters = FutexWaiters::default();
        let e1 = Arc::new(InterruptibleEvent::new());
        let e2 = Arc::new(InterruptibleEvent::new());
        let e3 = Arc::new(InterruptibleEvent::new());

        waiters.add(FutexWaiter {
            mask: 1,
            notifiable: FutexNotifiable::new_internal(Arc::downgrade(&e1)),
        });
        waiters.add(FutexWaiter {
            mask: 2,
            notifiable: FutexNotifiable::new_internal(Arc::downgrade(&e2)),
        });
        waiters.add(FutexWaiter {
            mask: 3,
            notifiable: FutexNotifiable::new_internal(Arc::downgrade(&e3)),
        });

        let split = waiters.split_for_requeue(2);

        assert_eq!(split.0.len(), 2);
        assert_eq!(split.0[0].mask, 1);
        assert_eq!(split.0[1].mask, 2);

        assert_eq!(waiters.0.len(), 1);
        assert_eq!(waiters.0[0].mask, 3);
    }

    #[fuchsia::test]
    fn test_stale_external_waiter_cleanup() {
        let mut state = FutexTableState::<PrivateFutexKey>::default();
        let key = PrivateFutexKey {
            addr: FutexAddress::try_from(UserAddress::from(
                (RESTRICTED_ASPACE_BASE + 0x1000) as u64,
            ))
            .unwrap(),
        };

        {
            let token = Arc::new(());
            let (sender, _receiver) = oneshot::channel::<()>();
            state.get_waiters_or_default(key.clone()).add(FutexWaiter {
                mask: u32::MAX,
                notifiable: FutexNotifiable::new_external(Arc::downgrade(&token), sender),
            });
        } // token is dropped here, so it becomes stale

        assert_eq!(state.waiters.len(), 1);

        // Trigger a cleanup with a placeholder event
        let dummy_event = InterruptibleEvent::new();
        state.remove_waiter_from_queue(key, &WaiterMatcher::Event(&dummy_event));

        assert_eq!(state.waiters.len(), 0, "Stale external waiter should be removed");
    }

    #[::fuchsia::test]
    async fn test_futex_deadlock_with_pager() {
        use crate::mm::memory::MemoryObject;
        use crate::mm::{DesiredAddress, MappingName, MappingOptions, PAGE_SIZE, ProtectionFlags};
        use crate::testing::spawn_kernel_and_run;
        use std::sync::atomic::{AtomicBool, Ordering};
        use zx::sys::zx_page_request_command_t::ZX_PAGER_VMO_READ;

        spawn_kernel_and_run(async move |current_task| {
            let mm = current_task.mm().unwrap();

            let port = Arc::new(zx::Port::create());
            let port_clone = port.clone();
            let pager =
                Arc::new(zx::Pager::create(zx::PagerOptions::empty()).expect("create failed"));
            let pager_clone = pager.clone();

            let vmo = Arc::new(
                pager
                    .create_vmo(zx::VmoOptions::RESIZABLE, &port, 1, *PAGE_SIZE)
                    .expect("create_vmo failed"),
            );
            let vmo_clone = vmo.clone();

            let mapped_addr = mm
                .map_memory(
                    DesiredAddress::Any,
                    Arc::new(MemoryObject::from(
                        (*vmo).duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                    )),
                    0,
                    *PAGE_SIZE as usize,
                    ProtectionFlags::READ | ProtectionFlags::WRITE,
                    MappingOptions::empty(),
                    MappingName::None,
                )
                .expect("map failed");

            let futex_table = Arc::new(FutexTable::<PrivateFutexKey>::default());
            let futex_table_clone = futex_table.clone();
            let task_clone = current_task.task.clone();
            let dummy_addr = UserAddress::from((RESTRICTED_ASPACE_BASE + 0x2000) as u64);

            let page_requested = Arc::new(AtomicBool::new(false));
            let wake_completed = Arc::new(AtomicBool::new(false));

            let page_req_clone = page_requested.clone();
            let wake_completed_clone = wake_completed.clone();

            let pager_thread = std::thread::spawn(move || {
                let packet = port_clone.wait(zx::MonotonicInstant::INFINITE).expect("wait failed");
                if let zx::PacketContents::Pager(contents) = packet.contents() {
                    if contents.command() == ZX_PAGER_VMO_READ {
                        let range = contents.range();
                        page_req_clone.store(true, Ordering::SeqCst);

                        // Spawn waker thread while main thread is page-faulting before acquiring the lock
                        let waker = std::thread::spawn(move || {
                            let _ = futex_table_clone.wake(&task_clone, dummy_addr, 1, u32::MAX);
                            wake_completed_clone.store(true, Ordering::SeqCst);
                        });

                        // Verify waker completes immediately without being blocked by the page fault
                        waker.join().unwrap();
                        assert!(
                            wake_completed.load(Ordering::SeqCst),
                            "Waker thread must NOT be blocked while page fault is in progress!"
                        );

                        // Supply pages to unblock the main thread's page fault
                        let source_vmo =
                            zx::Vmo::create(range.end - range.start).expect("create failed");
                        pager_clone
                            .supply_pages(&vmo_clone, range, &source_vmo, 0)
                            .expect("supply_pages failed");
                    }
                }
            });

            // This will lock the FutexTable, then page fault on mapped_addr when reading value
            let _ = futex_table.wait(
                current_task,
                mapped_addr,
                0,
                u32::MAX,
                zx::MonotonicInstant::from_nanos(1),
            );

            pager_thread.join().unwrap();
        })
        .await;
    }
}
