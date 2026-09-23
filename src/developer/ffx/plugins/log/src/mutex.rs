// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::condition_variable::LocalConditionVariable;
use derivative::Derivative;
use std::cell::{Cell, RefCell, RefMut};
use std::fmt::Debug;
use std::ops::{Deref, DerefMut};

/// Mutex designed for local executors which guarantees
/// wakeup ordering.
#[derive(Derivative)]
#[derivative(Debug)]
pub struct LocalOrderedMutex<T> {
    #[derivative(Debug = "ignore")]
    pending_tasks: Cell<Option<LocalConditionVariable>>,
    value: RefCell<T>,
}

impl<T> LocalOrderedMutex<T> {
    pub fn new(value: T) -> Self {
        Self { value: RefCell::new(value), pending_tasks: Cell::new(None) }
    }

    /// Locks the mutex. Tasks will be unblocked in the order in which
    /// they locked the mutex.
    pub async fn lock(&self) -> LocalOrderedLockGuard<'_, T> {
        let cv = LocalConditionVariable::new();
        if let Some(prev) = self.pending_tasks.take() {
            self.pending_tasks.set(Some(cv.clone()));
            prev.await;
        } else {
            self.pending_tasks.set(Some(cv.clone()));
        }
        return LocalOrderedLockGuard { inner: self.value.borrow_mut(), cv };
    }
}

#[derive(Debug)]
pub struct LocalOrderedLockGuard<'a, T: ?Sized> {
    inner: RefMut<'a, T>,
    cv: LocalConditionVariable,
}

impl<'a, T> Drop for LocalOrderedLockGuard<'a, T>
where
    T: ?Sized + 'a,
{
    fn drop(&mut self) {
        self.cv.notify_one();
    }
}

impl<'a, T> Deref for LocalOrderedLockGuard<'a, T>
where
    T: ?Sized + 'a,
{
    fn deref(&self) -> &Self::Target {
        &*self.inner
    }

    type Target = T;
}

impl<'a, T> DerefMut for LocalOrderedLockGuard<'a, T>
where
    T: ?Sized + 'a,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut *self.inner
    }
}

#[cfg(test)]
mod tests {
    use std::future::poll_fn;
    use std::pin::pin;
    use std::task::{Context, Poll};

    use super::*;
    use assert_matches::assert_matches;
    use futures::stream::FuturesUnordered;
    use futures::{Future, StreamExt};

    #[fuchsia::test]
    async fn mutex_signals_completions_in_order() {
        let mutex = LocalOrderedMutex::new(());
        let mutex_ref = &mutex;
        let unordered = FuturesUnordered::new();
        for i in 0..100 {
            unordered.push(async move {
                mutex_ref.lock().await;
                i
            });
        }
        let mut last = 0;
        let mut unordered = unordered.enumerate();
        while let Some((value, expected)) = unordered.next().await {
            assert_eq!(value, expected);
            last = expected;
        }
        assert_eq!(last, 99);
    }

    #[fuchsia::test]
    async fn mutex_does_not_allow_multiple_mutable_borrows() {
        let mutex = LocalOrderedMutex::new(());
        let waker = poll_fn(|context| Poll::Ready(context.waker().clone())).await;
        let mut context = Context::from_waker(&waker);
        let mut lock_task = pin!(mutex.lock());
        // Mutex is in locked state
        let guard =
            assert_matches!(lock_task.as_mut().poll(&mut context), Poll::Ready(guard)=>guard);
        let mut lock_task = pin!(mutex.lock());
        // We shouldn't be able to lock the same mutex twice.
        assert_matches!(lock_task.as_mut().poll(&mut context), Poll::Pending);
        // The mutex should unblock when the previous one completes.
        drop(guard);
        assert_matches!(lock_task.poll(&mut context), Poll::Ready(_));
    }
}
