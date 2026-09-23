// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module implements a lockup detector for Starnix kernel threads.
//! It tracks the start time of operations and reports threads that run for too long
//! without pausing or stopping the operation.
//!
//! It uses a global registry to track active operations across all threads.

use pin_project::pin_project;
use starnix_sync::{LockDepRwLock, ThreadLockupDetectorRegistryLock};
use std::borrow::Borrow;
use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};

#[derive(Default)]
pub struct ThreadLockupDetector;

/// Thread-local state that registers the thread in the global registry on creation
/// and removes it on drop.
struct ThreadState {
    /// Pointer to the atomic u64 used to store the start time of the current operation.
    /// This is boxed to ensure its address remains stable while registered.
    atomic: Box<AtomicU64>,
    /// The KOID of the thread, used as the key for removal in `Drop`.
    koid: zx::Koid,
}

impl ThreadState {
    /// Creates a new `ThreadState`, registering the current thread in the global `REGISTRY`.
    fn new() -> Self {
        let handle = fuchsia_runtime::with_thread_self(|thread| thread.raw_handle());
        let koid = fuchsia_runtime::with_thread_self(|thread| thread.koid()).unwrap();
        let atomic = Box::new(AtomicU64::new(0));
        let ptr = &*atomic as *const AtomicU64;

        let mut rcu_nesting_level = std::ptr::null();
        let mut rcu_counter_index = std::ptr::null();
        fuchsia_rcu::with_thread_block_counters(|nesting_ptr, counter_ptr| {
            rcu_nesting_level = nesting_ptr;
            rcu_counter_index = counter_ptr;
        });

        let registered = RegisteredThread {
            // SAFETY: The handle is valid as long as the thread is registered.
            thread: unsafe { zx::Unowned::from_raw_handle(handle) },
            koid,
            atomic: ptr,
            rcu_nesting_level,
            rcu_counter_index,
        };
        REGISTRY.write().insert(registered);
        Self { atomic, koid }
    }
}

impl Drop for ThreadState {
    /// Removes the thread from the global `REGISTRY` when the thread exits.
    fn drop(&mut self) {
        REGISTRY.write().remove(&self.koid);
    }
}

thread_local! {
    static THREAD_STATE: RefCell<Option<ThreadState>> = const { RefCell::new(None) };
}

/// The information stored in the global registry for each tracked thread.
#[derive(Clone)]
struct RegisteredThread {
    /// An unowned handle to the thread, used for inspection.
    thread: zx::Unowned<'static, zx::Thread>,
    /// The KOID of the thread.
    koid: zx::Koid,
    /// Pointer to the atomic u64 in the thread's `ThreadState`.
    atomic: *const AtomicU64,
    /// Pointer to the RCU nesting level.
    rcu_nesting_level: *const AtomicUsize,
    /// Pointer to the RCU counter index.
    rcu_counter_index: *const AtomicU8,
}

// We only hash and compare by `koid` to allow lookup and removal by `koid`
// in the `HashSet`.
impl std::hash::Hash for RegisteredThread {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.koid.hash(state);
    }
}

impl PartialEq for RegisteredThread {
    fn eq(&self, other: &Self) -> bool {
        self.koid == other.koid
    }
}

impl Eq for RegisteredThread {}

impl Borrow<zx::Koid> for RegisteredThread {
    fn borrow(&self) -> &zx::Koid {
        &self.koid
    }
}

// SAFETY: Access to the pointers in the global REGISTRY is protected by a LockDepRwLock,
// ensuring that a thread cannot free its data while another thread is reading it.
unsafe impl Send for RegisteredThread {}
// SAFETY: Same as above.
unsafe impl Sync for RegisteredThread {}

#[derive(Clone, Debug)]
pub struct ThreadLockupInfo {
    pub thread: zx::Unowned<'static, zx::Thread>,
    pub koid: zx::Koid,
    pub start_time: zx::MonotonicInstant,
}

/// Global registry of all tracked threads.
static REGISTRY: LazyLock<
    LockDepRwLock<HashSet<RegisteredThread>, ThreadLockupDetectorRegistryLock>,
> = LazyLock::new(|| Default::default());

impl ThreadLockupDetector {
    /// Starts an operation by storing the current time in the thread-local atomic.
    fn start_operation() {
        THREAD_STATE.with(|state| {
            let mut state = state.borrow_mut();
            let state = state.get_or_insert_with(|| ThreadState::new());
            state.atomic.store(zx::MonotonicInstant::get().into_nanos() as u64, Ordering::Relaxed);
        });
    }

    /// Stops an operation by storing 0 in the thread-local atomic.
    fn stop_operation() {
        THREAD_STATE.with(|state| {
            if let Some(state) = state.borrow().as_ref() {
                state.atomic.store(0, Ordering::Relaxed);
            }
        });
    }

    /// Iterates over the registry, finds threads that have been running longer than the threshold,
    /// and returns their `ThreadLockupInfo`.
    pub fn get_long_running_threads(threshold: zx::MonotonicDuration) -> Vec<ThreadLockupInfo> {
        let now = zx::MonotonicInstant::get();
        let registry = REGISTRY.read();
        registry
            .iter()
            .filter_map(|registered| {
                // SAFETY: We hold the read lock on REGISTRY. Any thread exiting must
                // acquire the write lock to remove its pointer before freeing the memory.
                // So the pointer is valid as long as we hold the read lock.
                let atomic = unsafe { &*registered.atomic };
                let start_nanos = atomic.load(Ordering::Relaxed);
                if start_nanos == 0 {
                    return None;
                }
                let start_time = zx::MonotonicInstant::from_nanos(start_nanos as i64);
                if now - start_time > threshold {
                    Some(ThreadLockupInfo {
                        thread: registered.thread.clone(),
                        koid: registered.koid,
                        start_time,
                    })
                } else {
                    None
                }
            })
            .collect()
    }

    /// Starts tracking the current operation on the current thread.
    /// Returns a guard that stops tracking when dropped.
    pub fn track() -> LockupDetectorGuard {
        LockupDetectorGuard::new()
    }

    /// Pauses tracking for the current operation on the current thread.
    /// Returns a guard that resumes tracking when dropped.
    pub fn pause_tracking() -> LockupDetectorWaitingGuard {
        LockupDetectorWaitingGuard::new()
    }

    /// Wraps a future to track its execution when polled.
    pub fn track_future<F>(inner: F) -> LockupDetectorFuture<F> {
        LockupDetectorFuture::new(inner)
    }

    /// Creates a channel where the receiver pauses tracking while waiting for messages.
    pub fn tracked_channel<T>() -> (std::sync::mpsc::Sender<T>, LockupDetectorReceiver<T>) {
        let (sender, receiver) = std::sync::mpsc::channel();
        (sender, LockupDetectorReceiver::new(receiver))
    }

    pub fn active_rcu_read_locks<F>(mut check: F)
    where
        F: FnMut(&zx::Thread, zx::Koid, u8),
    {
        let registry = REGISTRY.read();
        for registered in registry.iter() {
            if registered.rcu_nesting_level.is_null() || registered.rcu_counter_index.is_null() {
                continue;
            }
            // SAFETY: The pointers point to thread-local storage of the registered thread.
            // Before the thread exits, its `ThreadState` is dropped, which acquires a write
            // lock on `REGISTRY` before the thread-local storage is destroyed. Since we hold the
            // read lock on `REGISTRY` here, the thread cannot complete its cleanup and destroy the
            // TLS until we release the lock, ensuring the pointers remain valid.
            let (nesting_level, counter_index) = unsafe {
                (
                    (*registered.rcu_nesting_level).load(Ordering::Relaxed),
                    (*registered.rcu_counter_index).load(Ordering::Relaxed),
                )
            };
            if nesting_level > 0 {
                check(&registered.thread, registered.koid, counter_index);
            }
        }
    }
}

pub struct LockupDetectorGuard;

impl LockupDetectorGuard {
    fn new() -> Self {
        ThreadLockupDetector::start_operation();
        Self
    }
}

impl Drop for LockupDetectorGuard {
    fn drop(&mut self) {
        ThreadLockupDetector::stop_operation();
    }
}

pub struct LockupDetectorWaitingGuard;

impl LockupDetectorWaitingGuard {
    fn new() -> Self {
        ThreadLockupDetector::stop_operation();
        Self
    }
}

impl Drop for LockupDetectorWaitingGuard {
    fn drop(&mut self) {
        ThreadLockupDetector::start_operation();
    }
}

#[pin_project]
pub struct LockupDetectorFuture<F> {
    #[pin]
    inner: F,
}

impl<F> LockupDetectorFuture<F> {
    fn new(inner: F) -> Self {
        Self { inner }
    }
}

impl<F: std::future::Future> std::future::Future for LockupDetectorFuture<F> {
    type Output = F::Output;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let _guard = LockupDetectorGuard::new();
        let this = self.project();
        this.inner.poll(cx)
    }
}

pub struct LockupDetectorReceiver<T> {
    inner: std::sync::mpsc::Receiver<T>,
}

impl<T> LockupDetectorReceiver<T> {
    fn new(inner: std::sync::mpsc::Receiver<T>) -> Self {
        Self { inner }
    }

    pub fn recv(&self) -> Result<T, std::sync::mpsc::RecvError> {
        let _guard = LockupDetectorWaitingGuard::new();
        self.inner.recv()
    }

    pub fn try_iter(&self) -> std::sync::mpsc::TryIter<'_, T> {
        self.inner.try_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get_long_running_koids() -> Vec<zx::Koid> {
        ThreadLockupDetector::get_long_running_threads(zx::MonotonicDuration::from_nanos(0))
            .iter()
            .map(|r| r.koid)
            .collect()
    }

    #[test]
    fn test_lockup_detector() {
        let koid = fuchsia_runtime::with_thread_self(|thread| thread.koid()).unwrap();

        {
            let _guard = ThreadLockupDetector::track();

            // Exceed threshold immediately with zero duration.
            assert!(get_long_running_koids().contains(&koid));

            // After triggering, it still contains it (we don't reset).
            assert!(get_long_running_koids().contains(&koid));
        }

        // Guard dropped.
        assert!(get_long_running_koids().is_empty());
    }

    #[test]
    fn test_guard() {
        let koid = fuchsia_runtime::with_thread_self(|thread| thread.koid()).unwrap();

        {
            let _guard = ThreadLockupDetector::track();
            assert!(get_long_running_koids().contains(&koid));
        }

        // Guard dropped.
        assert!(get_long_running_koids().is_empty());
    }

    #[test]
    fn test_waiting_guard() {
        let koid = fuchsia_runtime::with_thread_self(|thread| thread.koid()).unwrap();

        let _guard = ThreadLockupDetector::track();

        {
            let _waiting_guard = ThreadLockupDetector::pause_tracking();
            // Operation stopped during wait.
            assert!(get_long_running_koids().is_empty());
        }

        // Guard dropped, operation restarted.
        assert!(get_long_running_koids().contains(&koid));
    }

    #[test]
    fn test_track_future() {
        let (koid_tx, koid_rx) = std::sync::mpsc::channel();
        let (signal_tx, signal_rx) = futures::channel::oneshot::channel::<()>();

        let t = std::thread::spawn(move || {
            let koid = fuchsia_runtime::with_thread_self(|thread| thread.koid()).unwrap();
            koid_tx.send(koid).unwrap();

            let fut = ThreadLockupDetector::track_future(async move {
                signal_rx.await.unwrap();
            });

            fuchsia_async::LocalExecutor::default().run_singlethreaded(fut);

            koid
        });

        let spawned_koid = koid_rx.recv().unwrap();

        // Wait a bit to ensure it entered the future and is waiting.
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Check that spawned_koid is NOT in long running koids.
        assert!(!get_long_running_koids().contains(&spawned_koid));

        // Now signal to unblock it.
        signal_tx.send(()).unwrap();

        t.join().unwrap();
    }

    #[test]
    fn test_track_future_polling() {
        let koid = fuchsia_runtime::with_thread_self(|thread| thread.koid()).unwrap();

        // Before polling, should not be found.
        assert!(!get_long_running_koids().contains(&koid));

        let fut = ThreadLockupDetector::track_future(async {
            assert!(get_long_running_koids().contains(&koid));
        });

        fuchsia_async::LocalExecutor::default().run_singlethreaded(fut);

        // After polling, should not be found.
        assert!(!get_long_running_koids().contains(&koid));
    }

    #[test]
    fn test_track_channel() {
        let (koid_tx, koid_rx) = std::sync::mpsc::channel();
        let (tx, rx) = ThreadLockupDetector::tracked_channel();

        let t = std::thread::spawn(move || {
            let koid = fuchsia_runtime::with_thread_self(|thread| thread.koid()).unwrap();
            koid_tx.send(koid).unwrap();

            let _guard = ThreadLockupDetector::track();

            // This will block.
            rx.recv().unwrap();

            koid
        });

        let spawned_koid = koid_rx.recv().unwrap();

        // Wait a bit to ensure it entered rx.recv()
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Check that spawned_koid is NOT in long running koids.
        assert!(!get_long_running_koids().contains(&spawned_koid));

        // Now send data to unblock it.
        tx.send(()).unwrap();

        t.join().unwrap();
    }
}
