// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::ops::Deref;
use core::task::Poll;

use sapphire_collections::deque::Deque;
use sapphire_collections::storage::StorageFamily;
use sapphire_sync::mutex::Mutex;
use sapphire_sync::mutex::raw::RawMutex;
use thiserror::Error;

use crate::notification::Notification;

/// Configuration trait for configuring the internals of the [`Mpsc`] channel.
///
/// # Examples
///
/// An `Mpsc` channel configured with `SingleThreadMutex` cannot be shared across threads (compile_fail):
///
/// ```compile_fail
/// use sapphire_async::mpsc::{Mpsc, MpscCfg};
/// use sapphire_collections::storage::ArrayStorage;
/// use sapphire_sync::mutex::raw::SingleThreadMutex;
/// struct NonSyncCfg;
/// impl MpscCfg for NonSyncCfg {
///     type Buffer = ArrayStorage<3>;
///     type Mtx = SingleThreadMutex;
/// }
///
/// type NonSyncMpsc = Mpsc<i32, NonSyncCfg>;
///
/// fn assert_sync<T: Sync>() {}
/// assert_sync::<NonSyncMpsc>();
/// ```
pub trait MpscCfg {
    /// The storage container family used for the internal queue buffer.
    type Buffer: StorageFamily;
    /// The raw mutex fundamental used to synchronize internal channel state.
    type Mtx: RawMutex;
}

/// A Multi-Producer Single-Consumer (MPSC) asynchronous channel.
///
/// `Mpsc` is backed by a generic double-ended queue (`Deque<C, T>`), supporting both
/// growable heap-allocated buffers and bounded stack-allocated zero-heap arrays.
/// Senders (`Tx`) can be cloned to allow multiple producers, while `Rx` provides exclusive
/// consumer access.
///
/// Senders and receivers block asynchronously using fine-grained notification signals
/// (`not_full` and `not_empty`) on a shared `Mutex`, eliminating thundering herd wakeups.
///
/// # Examples
///
/// Basic concurrent sending and receiving:
///
/// ```
/// use sapphire_async::mpsc::{Mpsc, MpscCfg};
/// use sapphire_async::testing::TestExecutor;
/// use sapphire_async::executor::BoundedExecutor;
/// use sapphire_collections::storage::ArrayStorage;
/// use sapphire_sync::mutex::raw::SingleThreadMutex;
///
/// struct MyMpscCfg;
/// impl MpscCfg for MyMpscCfg {
///     type Buffer = ArrayStorage<3>;
///     type Mtx = SingleThreadMutex;
/// }
///
/// let mut channel = Mpsc::<i32, MyMpscCfg>::new();
/// let (mut tx, mut rx) = channel.split();
///
/// # BoundedExecutor::new(TestExecutor::new(), |s| {
/// #     s.block_on(async {
/// tx.send(42).await.unwrap();
/// assert_eq!(rx.recv().await.unwrap(), 42);
/// #     });
/// # });
/// ```
pub struct Mpsc<T, Cfg: MpscCfg> {
    state: Mutex<Cfg::Mtx, MpscInner<T, Cfg>>,
    not_full: Notification<Cfg::Mtx>,
    not_empty: Notification<Cfg::Mtx>,
}

struct MpscInner<T, Cfg: MpscCfg> {
    queue: Deque<T, Cfg::Buffer>,
    tx_count: usize,
    rx_count: usize,
}

#[derive(Debug)]
pub struct Tx<Chan: MpscHandles> {
    channel: Chan,
}

#[derive(Debug)]
pub struct Rx<Chan: MpscHandles> {
    channel: Chan,
}

impl<Chan: MpscHandles + Clone> Clone for Tx<Chan> {
    fn clone(&self) -> Self {
        self.channel.clone_tx();
        Self { channel: self.channel.clone() }
    }
}

impl<Chan: MpscHandles> Drop for Tx<Chan> {
    fn drop(&mut self) {
        self.channel.drop_tx();
    }
}
impl<Chan: MpscHandles> Drop for Rx<Chan> {
    fn drop(&mut self) {
        self.channel.drop_rx();
    }
}

pub trait MpscHandles {
    fn clone_tx(&self);
    fn clone_rx(&self);
    fn drop_tx(&self);
    fn drop_rx(&self);
}

impl<T, Cfg: MpscCfg, Chan: Deref<Target = Mpsc<T, Cfg>>> MpscHandles for Chan {
    fn clone_tx(&self) {
        self.state.lock().tx_count += 1;
    }

    fn clone_rx(&self) {
        self.state.lock().rx_count += 1;
    }

    fn drop_tx(&self) {
        let mut guard = self.state.lock();
        guard.tx_count -= 1;
        if guard.tx_count == 0 {
            self.not_empty.notify_all();
        }
    }

    fn drop_rx(&self) {
        let mut guard = self.state.lock();
        guard.rx_count -= 1;
        if guard.rx_count == 0 {
            self.not_full.notify_all();
        }
    }
}

impl<T, Cfg: MpscCfg> Default for Mpsc<T, Cfg>
where
    Deque<T, Cfg::Buffer>: Default,
{
    fn default() -> Self {
        Self::new_with(Deque::default())
    }
}
impl<T, Cfg: MpscCfg> Mpsc<T, Cfg> {
    /// Creates a new, empty `Mpsc` with the configured buffer type.
    pub fn new() -> Self
    where
        Self: Default,
    {
        Self::default()
    }

    /// Creates a new, empty `Mpsc` backed by the provided buffer container.
    pub fn new_with(mut buffer: Deque<T, Cfg::Buffer>) -> Self {
        buffer.clear();
        Self {
            state: Mutex::new(MpscInner { queue: buffer, tx_count: 0, rx_count: 0 }),
            not_full: Notification::new(),
            not_empty: Notification::new(),
        }
    }

    /// Splits the `Mpsc` channel into a sender (`Tx`) and a receiver (`Rx`) pair.
    pub fn split(&mut self) -> (Tx<&'_ Self>, Rx<&'_ Self>) {
        self.state.get_mut().tx_count = 1;
        self.state.get_mut().rx_count = 1;
        (Tx { channel: self }, Rx { channel: self })
    }

    #[cfg(feature = "std")]
    /// Splits the channel into atomically reference-counted sender and receiver ends
    pub fn split_to_arc(mut self) -> (Tx<std::sync::Arc<Self>>, Rx<std::sync::Arc<Self>>) {
        self.state.get_mut().tx_count = 1;
        self.state.get_mut().rx_count = 1;
        let this = std::sync::Arc::new(self);
        (Tx { channel: this.clone() }, Rx { channel: this.clone() })
    }

    #[cfg(feature = "std")]
    /// Splits the channel into reference-counted sender and receiver ends
    pub fn split_to_rc(mut self) -> (Tx<std::rc::Rc<Self>>, Rx<std::rc::Rc<Self>>) {
        self.state.get_mut().tx_count = 1;
        self.state.get_mut().rx_count = 1;
        let this = std::rc::Rc::new(self);
        (Tx { channel: this.clone() }, Rx { channel: this.clone() })
    }
}

impl<T, Cfg: MpscCfg> MpscInner<T, Cfg> {
    pub fn send(&mut self, payload: T) -> Result<(), (T, SendSyncError)> {
        if self.rx_count == 0 {
            return Err((payload, SendSyncError::SendError(SendError::Closed)));
        }
        self.queue.push_back(payload).map_err(|t| (t, SendSyncError::WouldBlock))
    }

    pub fn recv(&mut self) -> Result<Option<T>, RecvError> {
        match self.queue.pop_front() {
            Some(t) => Ok(Some(t)),
            None => {
                if self.tx_count == 0 {
                    Err(RecvError::Closed)
                } else {
                    Ok(None)
                }
            }
        }
    }
}

/// Error sending a payload to the channel.
#[derive(Debug, Clone, Error)]
pub enum SendError {
    #[error("All receiving handles have been closed")]
    Closed,
}

/// Error sending a payload to the channel synchronously (without blocking).
#[derive(Debug, Clone, Error)]
pub enum SendSyncError {
    #[error(transparent)]
    SendError(#[from] SendError),
    #[error("Channel is currently full and request requires non-blocking semantics")]
    WouldBlock,
}

impl<T, Cfg, Chan> Tx<Chan>
where
    Chan: Deref<Target = Mpsc<T, Cfg>>,
    Cfg: MpscCfg,
{
    /// Asynchronously sends a message over the channel.
    ///
    /// Blocks if the channel's buffer is full until a receiver reads a message and frees a slot.
    pub async fn send(&mut self, payload: T) -> Result<(), (T, SendError)> {
        let channel = &*self.channel;
        let mut payload = Some(payload);
        let guard = channel.state.lock();

        channel
            .not_full
            .when(guard, |inner| match inner.send(payload.take().expect("Missing payload")) {
                Ok(()) => {
                    channel.not_empty.notify_one();
                    Poll::Ready(Ok(()))
                }
                Err((t, SendSyncError::WouldBlock)) => {
                    payload.replace(t);
                    Poll::Pending
                }
                Err((t, SendSyncError::SendError(error))) => Poll::Ready(Err((t, error))),
            })
            .await
    }

    /// Attempts to immediately send a message over the channel without blocking.
    ///
    /// Returns `Err(payload)` if the channel buffer is currently full.
    pub fn try_send(&self, payload: T) -> Result<(), (T, SendSyncError)> {
        let channel = &*self.channel;
        let mut guard = channel.state.lock();
        let out = guard.send(payload);
        if out.is_ok() {
            channel.not_empty.notify_one();
        }
        out
    }
}

/// Error sending a payload to the channel.
#[derive(Debug, Clone, Error)]
pub enum RecvError {
    #[error("All sender handles have been closed and the channel is empty")]
    Closed,
}

impl<T, Cfg, Chan> Rx<Chan>
where
    Chan: Deref<Target = Mpsc<T, Cfg>>,
    Cfg: MpscCfg,
{
    /// Asynchronously receives the next message from the channel.
    ///
    /// Blocks if the channel is empty until a sender publishes a message.
    pub async fn recv(&self) -> Result<T, RecvError> {
        let channel = &*self.channel;
        let guard = channel.state.lock();
        channel
            .not_empty
            .when(guard, |inner| match inner.recv() {
                Ok(Some(payload)) => {
                    channel.not_full.notify_one();
                    Poll::Ready(Ok(payload))
                }
                Ok(None) => Poll::Pending,
                Err(e) => Poll::Ready(Err(e)),
            })
            .await
    }

    /// Attempts to immediately receive the next message from the channel without blocking.
    ///
    /// Returns `None` if there are no pending messages enqueued.
    pub fn try_recv(&self) -> Result<Option<T>, RecvError> {
        let channel = &*self.channel;
        let mut guard = channel.state.lock();
        let out = guard.recv();
        if out.as_ref().is_ok_and(Option::is_some) {
            channel.not_full.notify_one();
        }
        out
    }
}

#[cfg(all(test, feature = "testing"))]
mod tests {
    use super::*;
    use crate::executor::BoundedExecutor;
    use crate::testing::TestExecutor;
    use sapphire_sync::mutex::raw::SingleThreadMutex;
    use std::cell::RefCell;

    use sapphire_collections::storage::ArrayStorage;

    struct StackMpscCfg<const N: usize>;
    impl<const N: usize> MpscCfg for StackMpscCfg<N> {
        type Buffer = ArrayStorage<N>;
        type Mtx = SingleThreadMutex;
    }

    type TestMpsc<T, const N: usize> = Mpsc<T, StackMpscCfg<N>>;

    #[test]
    fn test_mpsc_basic() {
        let mut channel = TestMpsc::<i32, 10>::new();
        let (mut tx, rx) = channel.split();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            s.block_on(async {
                tx.send(42).await.unwrap();
                let val = rx.recv().await.unwrap();
                assert_eq!(val, 42);
            });
        });
    }

    #[test]
    fn test_mpsc_blocking_recv() {
        let mut channel = TestMpsc::<i32, 10>::new();
        let (mut tx, rx) = channel.split();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            let mut handle = s.spawn(async { rx.recv().await.unwrap() });

            s.run_until_stalled();
            assert!(!handle.is_finished()); // Should be blocked

            s.block_on(async {
                tx.send(100).await.unwrap();
            });

            s.run_until_stalled();
            assert_eq!(handle.get(), Some(100)); // Should be unblocked and received
        });
    }

    #[test]
    fn test_mpsc_blocking_send() {
        // Bounded queue of size 1
        let mut channel = TestMpsc::<i32, 1>::new();
        let (tx, rx) = channel.split();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            let mut tx_temp = tx.clone();
            s.block_on(async move {
                tx_temp.send(1).await.unwrap(); // Should succeed immediately
            });

            let mut tx_clone = tx.clone();
            let handle = s.spawn(async move {
                tx_clone.send(2).await.unwrap(); // Should block because capacity is 1
            });

            s.run_until_stalled();
            assert!(!handle.is_finished()); // Should be blocked

            s.block_on(async {
                let val = rx.recv().await.unwrap();
                assert_eq!(val, 1);
            });

            s.run_until_stalled();
            assert!(handle.is_finished()); // Should be unblocked now
        });
    }

    #[test]
    fn test_mpsc_multi_producer() {
        let mut channel = TestMpsc::<i32, 10>::new();
        let (tx1, rx) = channel.split();
        let tx2 = tx1.clone();

        let results = RefCell::new(Vec::new());

        BoundedExecutor::new(TestExecutor::new(), |s| {
            let mut tx1_clone = tx1.clone();
            s.block_on(async move {
                tx1_clone.send(10).await.unwrap();
            });

            let mut tx2_clone = tx2.clone();
            s.block_on(async move {
                tx2_clone.send(20).await.unwrap();
            });

            s.spawn(async {
                let v1 = rx.recv().await.unwrap();
                let v2 = rx.recv().await.unwrap();
                results.borrow_mut().push(v1);
                results.borrow_mut().push(v2);
            });

            s.run_until_stalled();
            let res = results.borrow();
            assert_eq!(&*res, &[10, 20]);
        });
    }

    #[test]
    fn test_mpsc_channel_closing_senders() {
        let mut channel = TestMpsc::<i32, 10>::new();
        let (mut tx1, rx) = channel.split();
        let mut tx2 = tx1.clone();

        BoundedExecutor::new(TestExecutor::new(), move |s| {
            s.block_on(async move {
                tx1.send(1).await.unwrap();
                drop(tx1); // Drop first sender
            });

            s.block_on(async {
                let val = rx.recv().await.unwrap();
                assert_eq!(val, 1);
            });

            s.block_on(async move {
                tx2.send(2).await.unwrap();
                drop(tx2); // Drop second/last sender
            });

            s.block_on(async {
                let val = rx.recv().await.unwrap();
                assert_eq!(val, 2);

                // Now rx should return RecvError::Closed
                let err = rx.recv().await;
                assert!(matches!(err, Err(RecvError::Closed)));
            });
        });
    }

    #[test]
    fn test_mpsc_channel_closing_receivers() {
        let mut channel = TestMpsc::<i32, 10>::new();
        let (mut tx, rx) = channel.split();

        BoundedExecutor::new(TestExecutor::new(), move |s| {
            s.block_on(async move {
                tx.send(1).await.unwrap();

                drop(rx); // Drop first receiver
                // Send should now fail with SendError::Closed
                let err = tx.send(2).await;
                assert!(matches!(err, Err((2, SendError::Closed))));
            });
        });
    }

    mod proptests {
        use super::*;
        use crate::executor::BoundedExecutor;
        use crate::semaphore::Semaphore;
        use crate::testing::TestExecutor;
        use proptest::prelude::*;

        use std::cell::{Cell, RefCell};
        use std::collections::VecDeque;
        use std::rc::Rc;

        #[derive(Debug, Clone)]
        enum MpscOp {
            Send(i32),
            Recv,
        }

        type StackMpsc = TestMpsc<i32, 3>;

        proptest! {
            #[test]
            fn test_mpsc_proptest(
                ops in prop::collection::vec(
                    prop_oneof![
                        any::<i32>().prop_map(MpscOp::Send),
                        Just(MpscOp::Recv),
                    ],
                    0..50
                )
            ) {
                let mut channel = StackMpsc::new();
                let (tx, rx) = channel.split();

                let sent = Rc::new(RefCell::new(VecDeque::new()));
                let received = Rc::new(RefCell::new(Vec::new()));

                let sem = Rc::new(Semaphore::<SingleThreadMutex>::new(0));
                let completed_sends = Rc::new(Cell::new(0));

                BoundedExecutor::new(TestExecutor::new(), |s| {
                    // Controlled receiver task
                    let received_clone = received.clone();
                    let sem_clone = sem.clone();
                    s.spawn(async move {
                        loop {
                            sem_clone.down().await;
                            match rx.recv().await {
                                Ok(val) => received_clone.borrow_mut().push(val),
                                Err(_) => break, // Channel closed
                            }
                        }
                    });

                    let mut total_sends_requested = 0;
                    let mut total_recvs_requested = 0;
                    let tx_clone = tx.clone();

                    for op in ops {
                        match op {
                            MpscOp::Send(val) => {
                                total_sends_requested += 1;
                                sent.borrow_mut().push_back(val);
                                let mut tx_temp = tx_clone.clone();
                                let completed_sends_clone = completed_sends.clone();
                                s.spawn(async move {
                                    let _ = tx_temp.send(val).await;
                                    completed_sends_clone.set(completed_sends_clone.get() + 1);
                                });
                            }
                            MpscOp::Recv => {
                                total_recvs_requested += 1;
                                sem.up();
                            }
                        }
                        s.run_until_stalled();

                        let expected_recvs = std::cmp::min(total_sends_requested, total_recvs_requested);
                        let expected_sends = std::cmp::min(total_sends_requested, 3 + expected_recvs);

                        assert_eq!(received.borrow().len(), expected_recvs);
                        assert_eq!(completed_sends.get(), expected_sends);

                        let recvs = received.borrow();
                        let sends = sent.borrow();
                        for i in 0..recvs.len() {
                            assert_eq!(recvs[i], sends[i]);
                        }
                    }
                });
            }
        }
    }
}
