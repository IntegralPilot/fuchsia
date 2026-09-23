// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Provides a synchronous thread-safe oneshot channel.

use std::ops::DerefMut as _;

#[cfg(loom)]
pub(crate) use loom::sync::{Arc, Condvar, Mutex};
#[cfg(not(loom))]
pub(crate) use std::sync::{Arc, Condvar, Mutex};

#[derive(PartialEq, Eq, Debug, Clone, Copy)]
enum CompletionState<T> {
    /// The oneshot has not been sent.
    Pending,
    /// The oneshot sender was dropped.
    Dropped,
    /// The oneshot has been sent, and the receiver has not taken the sent
    /// value.
    Done(T),
    /// The receiver has taken the sent value.
    Finished,
}

impl<T> CompletionState<T> {
    fn assert_pending_and_replace(&mut self, next_completion_state: CompletionState<T>) {
        // We'd prefer to use `assert_matches` here, but `T` doesn't necessarily
        // impl Debug.
        match std::mem::replace(self, next_completion_state) {
            CompletionState::Pending => (),
            CompletionState::Done(_) => panic!("CompletionState should be Pending, not Done"),
            CompletionState::Dropped => panic!("CompletionState should be Pending, not Dropped"),
            CompletionState::Finished => panic!("CompletionState should be Pending, not Finished"),
        }
    }
}

#[derive(Debug)]
struct CompletionSignal<T> {
    done: Mutex<CompletionState<T>>,
    condvar: Condvar,
}

/// Creates a pair of sender and corresponding receiver.
pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let inner = Arc::new(CompletionSignal {
        done: Mutex::new(CompletionState::Pending),
        condvar: Condvar::new(),
    });
    (Sender { inner: Some(inner.clone()) }, Receiver { inner })
}

/// The sender side of a pair of sender and receiver.
#[derive(Debug)]
pub struct Sender<T> {
    inner: Option<Arc<CompletionSignal<T>>>,
}

impl<T> Sender<T> {
    /// Sends the `done_value` to the peer, waking it up if it is blocked on
    /// receiving.
    pub fn send(mut self, done_value: T) {
        // `inner` is present unless we have already sent or been dropped, which
        // is impossible.
        let inner = self.inner.take().expect("should be present");
        std::mem::forget(self);

        let CompletionSignal { done, condvar } = inner.as_ref();
        let mut done = done.lock().unwrap();
        done.assert_pending_and_replace(CompletionState::Done(done_value));
        condvar.notify_one();
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let Self { inner } = self;
        // `inner` is present unless we have already sent or been dropped, which
        // is impossible.
        let inner = inner.take().expect("should be present");
        let CompletionSignal { done, condvar } = inner.as_ref();
        let mut done = done.lock().unwrap();
        done.assert_pending_and_replace(CompletionState::Dropped);
        condvar.notify_one();
    }
}

/// Used to block on a completion signal being observed.
#[derive(Debug)]
pub struct Receiver<T> {
    inner: Arc<CompletionSignal<T>>,
}

/// Error indicating the sender was dropped without ever signaling the receiver.
#[derive(Debug, PartialEq, Eq)]
pub struct SenderDroppedError;

impl<T> Receiver<T> {
    /// Blocks until the sender sends or is dropped.
    pub fn receive(self) -> Result<T, SenderDroppedError> {
        let Self { inner } = self;
        let CompletionSignal { done, condvar } = inner.as_ref();

        // Loop until we're no longer pending.
        let mut done = done.lock().expect("should not be poisoned");
        loop {
            done = match done.deref_mut() {
                CompletionState::Pending => condvar.wait(done).expect("should not be poisoned"),
                CompletionState::Dropped => return Err(SenderDroppedError),
                CompletionState::Finished => panic!("should not have finished during receive"),
                CompletionState::Done(_) => break,
            }
        }

        let done = std::mem::replace(done.deref_mut(), CompletionState::Finished);
        match done {
            CompletionState::Done(done) => Ok(done),
            _ => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loom::sync::atomic::AtomicU8;
    use loom::thread;
    use std::sync::atomic::Ordering;

    #[test]
    fn waits_for_completion() {
        // Spawn a sender and a receiver thread that both want to write to the
        // same atomic, where the receiver thread should wait until the completion
        // signal before writing to it.

        loom::model(|| {
            let (sender, receiver) = channel();
            let data_to_race_on = Arc::new(AtomicU8::new(0));

            let sender_thread = thread::spawn({
                let data_to_race_on = data_to_race_on.clone();
                move || {
                    let previous_value = data_to_race_on.swap(1, Ordering::Relaxed);
                    assert_eq!(previous_value, 0);
                    sender.send(());
                }
            });

            let receiver_thread = thread::spawn({
                move || {
                    receiver.receive().expect("sender should not be dropped");
                    let previous_value = data_to_race_on.swap(2, Ordering::Relaxed);
                    assert_eq!(previous_value, 1);
                }
            });

            sender_thread.join().expect("should succeed");
            receiver_thread.join().expect("should succeed");
        });
    }

    #[test]
    fn observes_drop() {
        loom::model(|| {
            let (sender, receiver) = channel::<()>();
            let data_to_race_on = Arc::new(AtomicU8::new(0));

            let sender_thread = thread::spawn({
                let data_to_race_on = data_to_race_on.clone();
                move || {
                    let previous_value = data_to_race_on.swap(1, Ordering::Relaxed);
                    assert_eq!(previous_value, 0);
                    drop(sender);
                }
            });

            let receiver_thread = thread::spawn({
                move || {
                    let SenderDroppedError =
                        receiver.receive().expect_err("sender should be dropped");
                    let previous_value = data_to_race_on.swap(2, Ordering::Relaxed);
                    assert_eq!(previous_value, 1);
                }
            });

            sender_thread.join().expect("should succeed");
            receiver_thread.join().expect("should succeed");
        });
    }
}
