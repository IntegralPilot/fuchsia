// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use futures::channel::oneshot;
use futures::{FutureExt, ready};
use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::future::Future;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll};

/// Fields of `StrictMutex` that are protected by as synchronous mutex.
struct StrictMutexInner {
    /// Queue of waiters trying to acquire this lock. Firing one of the senders in this queue
    /// informs that waiter that it has the lock, and it will take the pointer.
    queue: VecDeque<oneshot::Sender<()>>,

    /// Whether the lock is presently held.
    held: bool,
}

/// An async mutex.
///
/// Mostly this is like the Mutexes provided in other crates, except that it guarantees queued
/// acquisition order. The Mutex in the `futures` and `async-lock` crates sacrifice fairness for
/// performance. Unfortunately we hit a very pathological edge of one of those sacrifices and the
/// repl will hang if we use one of the existing mutexes.
pub struct StrictMutex<T: ?Sized> {
    /// Our inner state is held by a regular synchronous mutex.
    inner: Mutex<StrictMutexInner>,

    /// The value protected by this lock.
    data: UnsafeCell<T>,
}

impl<T> StrictMutex<T> {
    pub fn new(data: T) -> Self {
        StrictMutex {
            inner: Mutex::new(StrictMutexInner { queue: VecDeque::new(), held: false }),
            data: UnsafeCell::new(data),
        }
    }
}

impl<T: ?Sized> StrictMutex<T> {
    pub fn lock(&self) -> StrictMutexLockFuture<'_, T> {
        let mut inner = self.inner.lock().unwrap();
        let (sender, receiver) = oneshot::channel();

        if inner.held {
            inner.queue.push_back(sender);
        } else {
            debug_assert!(inner.queue.is_empty());
            inner.held = true;
            sender
                .send(())
                .expect("Somehow managed to drop receiver before future was constructed");
        }

        StrictMutexLockFuture { lock: self, receiver }
    }
}

// SAFETY: This object implements a mutex which should make these operations safe.
unsafe impl<T: ?Sized + Send> Send for StrictMutex<T> {}
unsafe impl<T: ?Sized + Send> Sync for StrictMutex<T> {}

/// Future which polls the lock attempting to acquire it.
pub struct StrictMutexLockFuture<'l, T: ?Sized> {
    lock: &'l StrictMutex<T>,
    receiver: oneshot::Receiver<()>,
}

impl<'l, T: ?Sized + 'l> Future for StrictMutexLockFuture<'l, T> {
    type Output = StrictMutexLockGuard<'l, T>;

    fn poll(mut self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Self::Output> {
        ready!(self.receiver.poll_unpin(ctx))
            .expect("Reciever drop while we referenced the lock. Polled after acquisition?");

        Poll::Ready(StrictMutexLockGuard { lock: self.lock })
    }
}

/// Lock guard. Represents possession of the lock.
pub struct StrictMutexLockGuard<'l, T: ?Sized> {
    lock: &'l StrictMutex<T>,
}

impl<'l, T: ?Sized> Deref for StrictMutexLockGuard<'l, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: The pointer must be valid since it comes from unsafe cell. Ensuring correct
        // aliasing access is the point of the lock.
        unsafe { &*self.lock.data.get() }
    }
}

impl<'l, T: ?Sized> DerefMut for StrictMutexLockGuard<'l, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: The pointer must be valid since it comes from unsafe cell. Ensuring correct
        // aliasing access is the point of the lock.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<'l, T: ?Sized> Drop for StrictMutexLockGuard<'l, T> {
    fn drop(&mut self) {
        let mut inner = self.lock.inner.lock().unwrap();

        debug_assert!(inner.held);

        while let Some(sender) = inner.queue.pop_front() {
            if sender.send(()).is_ok() {
                return;
            }
        }

        inner.held = false;
    }
}
