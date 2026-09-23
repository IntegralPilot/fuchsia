// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use futures::Future;
use std::cell::RefCell;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Poll, Waker};

#[derive(Clone, Debug)]
struct ConditionVariableState {
    woke: bool,
    waker: Option<Waker>,
}

/// A condition variable optimized for non-Send executors.
/// The condition variable defaults to an unsignaled state,
/// and becomes signaled when notify_one is called.
/// When wake is called, a single task which is waiting on it
/// is woken up, and the condition variable resets to the unsignaled
/// state after the wakeup happens. If multiple clones of the
/// condition variable exist, one of them will be woken up
/// when notify_one is called.
#[derive(Clone, Debug)]
pub struct LocalConditionVariable {
    state: Rc<RefCell<ConditionVariableState>>,
}
impl LocalConditionVariable {
    /// Creates a new condition variable initialized to unsignaled.
    pub fn new() -> LocalConditionVariable {
        LocalConditionVariable {
            state: Rc::new(RefCell::new(ConditionVariableState { woke: false, waker: None })),
        }
    }

    /// Wakes the condition variable
    pub fn notify_one(&self) {
        let mut state = self.state.borrow_mut();
        state.woke = true;
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
    }
}

impl Future for LocalConditionVariable {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let mut state = self.state.borrow_mut();
        if state.woke {
            state.woke = false;
            Poll::Ready(())
        } else {
            state.waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::poll_fn;

    #[fuchsia::test]
    async fn condition_variable_initialized_to_unsignalled() {
        poll_fn(|context| {
            let mut cv = LocalConditionVariable::new();
            assert_eq!(Pin::new(&mut cv).poll(context), Poll::Pending);
            Poll::Ready(())
        })
        .await;
    }

    #[fuchsia::test]
    async fn condition_variable_is_signaled_when_signal_is_called_then_reset_after_wakeup() {
        poll_fn(|context| {
            let mut cv = LocalConditionVariable::new();
            assert_eq!(Pin::new(&mut cv).poll(context), Poll::Pending);
            cv.notify_one();
            assert_eq!(Pin::new(&mut cv).poll(context), Poll::Ready(()));
            assert_eq!(Pin::new(&mut cv).poll(context), Poll::Pending);
            Poll::Ready(())
        })
        .await;
    }

    #[fuchsia::test]
    async fn condition_variable_wakes_when_wake_called_on_clone() {
        poll_fn(|context| {
            let mut cv = LocalConditionVariable::new();
            let cv_clone = cv.clone();
            assert_eq!(Pin::new(&mut cv).poll(context), Poll::Pending);
            cv_clone.notify_one();
            assert_eq!(Pin::new(&mut cv).poll(context), Poll::Ready(()));
            assert_eq!(Pin::new(&mut cv).poll(context), Poll::Pending);
            Poll::Ready(())
        })
        .await;
    }
}
