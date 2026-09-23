// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::task::Poll;

use sapphire_collections::deque::Deque;
use sapphire_collections::map::HashMap;
use sapphire_sync::mutex::Mutex;

use crate::global_index::GlobalIndex;
use crate::notification::Notification;

use super::{BroadcastCfg, MissedMessages, Payload, SubId};

/// An asynchronous multi-subscriber Unfiltered Broadcast Channel.
///
/// `UnfilteredBroadcastChannel` allows a publisher to broadcast messages to multiple subscribers
/// simultaneously. It supports both growable heap-allocated message buffers and zero-heap
/// stack-allocated arrays.
///
/// The buffer reclaims space based on the slowest reader (reclaiming only elements that
/// all active subscribers have read). To prevent blocking the publisher when one subscriber
/// is slow, `force_publish` can be used to evict the oldest elements and catch up slow readers.
///
/// # Examples
///
/// Basic concurrent publishing and subscribing:
///
/// ```
/// use sapphire_async::broadcast::{UnfilteredBroadcastChannel, BroadcastCfg};
/// use sapphire_async::testing::TestExecutor;
/// use sapphire_async::executor::BoundedExecutor;
/// use sapphire_collections::storage::ArrayStorage;
/// use sapphire_sync::mutex::raw::SingleThreadMutex;
///
/// struct MyBroadcastCfg;
/// impl BroadcastCfg for MyBroadcastCfg {
///     type Buffer = ArrayStorage<3>;
///     type SubscriptionStore = ArrayStorage<2>;
///     type Mtx = SingleThreadMutex;
/// }
///
/// let channel = UnfilteredBroadcastChannel::<i32, MyBroadcastCfg>::new();
/// let mut sub1 = channel.subscribe().unwrap();
/// let mut sub2 = channel.subscribe().unwrap();
///
/// # BoundedExecutor::new(TestExecutor::new(), |s| {
/// #     s.block_on(async {
/// channel.publish(42).await;
/// assert_eq!(sub1.next().await, Ok(42));
/// assert_eq!(sub2.next().await, Ok(42));
/// #     });
/// # });
/// ```
///
/// An `UnfilteredBroadcastChannel` configured with `SingleThreadMutex` cannot be shared across threads:
///
/// ```compile_fail
/// use sapphire_async::broadcast::{UnfilteredBroadcastChannel, BroadcastCfg};
/// use sapphire_collections::storage::ArrayStorage;
/// use sapphire_sync::mutex::raw::SingleThreadMutex;
///
/// struct SingleThreadedCfg;
///
/// impl BroadcastCfg for SingleThreadedCfg {
///     type Buffer = ArrayStorage<3>;
///     type SubscriptionStore = ArrayStorage<2>;
///     type Mtx = SingleThreadMutex;
/// }
///
/// type NonSyncBroadcast = UnfilteredBroadcastChannel<i32, SingleThreadedCfg>;
///
/// fn assert_sync<T: Sync>() {}
/// assert_sync::<NonSyncBroadcast>(); // Correctly fails to compile because SingleThreadMutex is !Sync
/// ```
pub struct UnfilteredBroadcastChannel<T, Cfg: BroadcastCfg> {
    state: Mutex<Cfg::Mtx, UnfilteredBroadcastChannelState<T, Cfg>>,
    not_full: Notification<Cfg::Mtx>,
    not_empty: Notification<Cfg::Mtx>,
}

/// The synchronized internal state of an [`UnfilteredBroadcastChannel`].
struct UnfilteredBroadcastChannelState<T, Cfg: BroadcastCfg> {
    queue: Deque<Payload<T>, Cfg::Buffer>,
    head_global_idx: GlobalIndex,
    next_global_idx: GlobalIndex, // Where the next message will be written

    subscribers: HashMap<SubId, SubscriberState, Cfg::SubscriptionStore>,
    next_sub_id: usize,
}

/// The trackable state of an active subscriber enqueued in the channel state.
struct SubscriberState {
    next_global_idx: GlobalIndex,
}

/// An active subscriber endpoint to an [`UnfilteredBroadcastChannel`].
///
/// Receives cloned broadcasted messages. Can be polled asynchronously via [`Subscriber::next`].
pub struct Subscriber<'a, T, Cfg: BroadcastCfg> {
    channel: &'a UnfilteredBroadcastChannel<T, Cfg>,
    id: SubId,
}

impl<T, Cfg: BroadcastCfg> UnfilteredBroadcastChannelState<T, Cfg> {
    fn force_push_back(&mut self, payload: Payload<T>) -> Option<Payload<T>> {
        let prev = self.queue.force_push_back(payload);
        if prev.is_some() {
            self.head_global_idx += 1;
        }
        self.next_global_idx += 1;
        prev
    }

    /// Attempts to push a payload to the back of the queue, incrementing `next_global_idx` on success.
    ///
    /// Returns `Err(payload)` if the queue is full and cannot grow.
    fn push_back(&mut self, payload: Payload<T>) -> Result<(), Payload<T>> {
        self.queue.push_back(payload)?;
        self.next_global_idx += 1;
        Ok(())
    }

    fn reclaim_space(&mut self, not_full: &Notification<Cfg::Mtx>) {
        let mut reclaimed = 0;
        while self.queue.pop_front_if(|payload| payload.remaining_subs == 0).is_some() {
            self.head_global_idx += 1;
            reclaimed += 1;
        }

        if reclaimed > 0 {
            not_full.notify_many(reclaimed);
        }
    }

    pub fn used(&self) -> usize {
        (self.next_global_idx - self.head_global_idx)
            .try_into()
            .expect("Next index should be ahead of head")
    }
}

impl<T, Cfg: BroadcastCfg> Default for UnfilteredBroadcastChannel<T, Cfg>
where
    HashMap<SubId, SubscriberState, Cfg::SubscriptionStore>: Default,
    Deque<Payload<T>, Cfg::Buffer>: Default,
{
    fn default() -> Self {
        Self {
            state: Mutex::new(UnfilteredBroadcastChannelState {
                queue: Deque::default(),
                head_global_idx: GlobalIndex::new(0),
                next_global_idx: GlobalIndex::new(0),
                subscribers: HashMap::default(),
                next_sub_id: 0,
            }),
            not_full: Notification::new(),
            not_empty: Notification::new(),
        }
    }
}

impl<T: Clone, Cfg: BroadcastCfg> UnfilteredBroadcastChannel<T, Cfg> {
    /// Creates a new, empty `UnfilteredBroadcastChannel` with the configured mutex and notification fundamentals.
    pub fn new() -> Self
    where
        Self: Default,
    {
        Self::default()
    }

    /// Subscribes to the channel, returning a [`Subscriber`] endpoint if there is slot capacity.
    ///
    /// Returns `None` if the maximum number of subscribers (defined by `SubscriptionStore` capacity)
    /// has been reached.
    pub fn subscribe(&self) -> Option<Subscriber<'_, T, Cfg>> {
        let mut state = self.state.lock();
        let id = SubId::new(state.next_sub_id);
        state.next_sub_id += 1;

        let next_global_idx = state.next_global_idx;
        state.subscribers.try_insert(id, SubscriberState { next_global_idx }).ok()?;

        Some(Subscriber { channel: self, id })
    }

    /// Publishes a message to the channel asynchronously.
    ///
    /// If the channel's buffer is at capacity, this method blocks until the slowest reader
    /// reads enough elements to reclaim space.
    pub async fn publish(&self, payload: T) {
        let mut payload = Some(payload);
        let guard = self.state.lock();

        self.not_full
            .when(guard, |state| {
                let remaining_subs = state.subscribers.len();
                match state.push_back(Payload {
                    payload: payload.take().expect("Payload not refreshed"),
                    remaining_subs,
                }) {
                    Ok(()) => {
                        state.reclaim_space(&self.not_full);
                        Poll::Ready(())
                    }
                    Err(item) => {
                        payload.replace(item.payload);
                        Poll::Pending
                    }
                }
            })
            .await;
        // Notify all consumers that a message is enqueued
        self.not_empty.notify_all();
    }

    /// Publishes a message to the channel, evicting the oldest message if at capacity.
    ///
    /// This method never blocks. If the channel is at capacity, the oldest message is evicted
    /// and slow readers will miss it, returning `Err(MissedMessages)` on their next poll.
    pub fn force_publish(&self, payload: T) {
        let mut state = self.state.lock();
        let remaining_subs = state.subscribers.len();

        state.force_push_back(Payload { payload, remaining_subs });
        state.reclaim_space(&self.not_full);
        // Notify all consumers that a message is enqueued
        self.not_empty.notify_all();
    }
}

impl<'a, T: Clone, Cfg: BroadcastCfg> Subscriber<'a, T, Cfg> {
    /// Asynchronously polls and retrieves the next broadcasted message.
    pub async fn next(&mut self) -> Result<T, MissedMessages> {
        let guard = self.channel.state.lock();

        let res = self
            .channel
            .not_empty
            .when(guard, |state| {
                let sub = state.subscribers.get_mut(&self.id).expect("Subscriber not found");

                if sub.next_global_idx < state.head_global_idx {
                    let missed = (state.head_global_idx - sub.next_global_idx) as usize;
                    sub.next_global_idx = state.head_global_idx;
                    return Poll::Ready(Err(MissedMessages { count: missed }));
                }

                if sub.next_global_idx < state.next_global_idx {
                    let logical_idx = (sub.next_global_idx - state.head_global_idx) as usize;
                    // If the subscriber has fallen so far behind that modular index arithmetic
                    // wrapped around (or if logical_idx is out of bounds), queue.get will return None.
                    // Instead of panicking, reset the subscriber to head_global_idx and report
                    // usize::MAX missed messages since the exact count is unbounded.
                    let item = match state.queue.get_mut(logical_idx) {
                        Some(item) => item,
                        None => {
                            sub.next_global_idx = state.head_global_idx;
                            return Poll::Ready(Err(MissedMessages { count: usize::MAX }));
                        }
                    };
                    let payload = item.payload.clone();
                    item.remaining_subs =
                        item.remaining_subs.checked_sub(1).expect("remaining_subs underflow");
                    sub.next_global_idx += 1;

                    state.reclaim_space(&self.channel.not_full);
                    Poll::Ready(Ok(payload))
                } else {
                    Poll::Pending
                }
            })
            .await;

        res
    }
}

impl<'a, T, Cfg: BroadcastCfg> Drop for Subscriber<'a, T, Cfg> {
    fn drop(&mut self) {
        let mut state = self.channel.state.lock();
        if let Some(mut sub) = state.subscribers.remove(&self.id) {
            if sub.next_global_idx < state.head_global_idx {
                sub.next_global_idx = state.head_global_idx;
            }
            while sub.next_global_idx < state.next_global_idx {
                let logical_idx = (sub.next_global_idx - state.head_global_idx) as usize;

                match state.queue.get_mut(logical_idx) {
                    Some(item) => {
                        item.remaining_subs =
                            item.remaining_subs.checked_sub(1).expect("remaining_subs underflow");
                        sub.next_global_idx += 1;
                    }
                    None => {
                        break;
                    }
                }
            }
            state.reclaim_space(&self.channel.not_full);
        }
    }
}

#[cfg(all(test, feature = "testing"))]
mod tests {
    use super::*;
    use crate::executor::BoundedExecutor;
    use crate::testing::TestExecutor;
    use sapphire_collections::storage::ArrayStorage;
    use sapphire_sync::mutex::raw::SingleThreadMutex;

    struct StackCfg<const B: usize, const S: usize>;
    impl<const B: usize, const S: usize> BroadcastCfg for StackCfg<B, S> {
        type Buffer = ArrayStorage<B>;
        type SubscriptionStore = ArrayStorage<S>;
        type Mtx = SingleThreadMutex;
    }

    type StackBroadcast<T, const B: usize, const S: usize> =
        UnfilteredBroadcastChannel<T, StackCfg<B, S>>;

    #[test]
    fn test_broadcast_basic() {
        let channel = StackBroadcast::<i32, 10, 2>::new();

        let mut sub1 = channel.subscribe().unwrap();
        let mut sub2 = channel.subscribe().unwrap();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            s.block_on(async {
                channel.publish(42).await;

                let v1 = sub1.next().await.unwrap();
                let v2 = sub2.next().await.unwrap();

                assert_eq!(v1, 42);
                assert_eq!(v2, 42);
            });
        });
    }

    #[test]
    fn test_broadcast_publish_zero_subscribers() {
        // Channel with buffer capacity 1 and max 2 subscribers
        let channel = StackBroadcast::<i32, 1, 2>::new();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            assert!(channel.state.lock().used() == 0);
            let t = s.spawn(async {
                for _ in 0..10 {
                    channel.publish(0).await;
                }
            });
            s.run_until_stalled();
            assert!(t.is_finished());

            // No subscribers so the slots should be reclaimed
            assert!(channel.state.lock().used() == 0);

            // Now subscribe and verify the channel functions normally with capacity 1.

            let t = s.spawn(async {
                let mut sub = channel.subscribe().unwrap();
                channel.publish(1).await;
                // Actually added to the queue since we have subscribers
                assert!(channel.state.lock().used() == 1);
                assert_eq!(sub.next().await.unwrap(), 1);
            });
            s.run_until_stalled();
            assert!(t.is_finished());

            let t = s.spawn(async {
                for _ in 0..10 {
                    channel.publish(2).await;
                }
            });
            s.run_until_stalled();
            assert!(t.is_finished());
            // subscriber is now gone, so publishign should not use up a slot
            assert!(channel.state.lock().used() == 0);
        });
    }

    #[test]
    fn test_broadcast_blocking_publisher() {
        // Capacity 1, max 2 subscribers
        let channel = StackBroadcast::<i32, 1, 2>::new();
        let mut sub = channel.subscribe().unwrap();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            s.block_on(async {
                channel.publish(1).await; // Should succeed immediately
            });

            let handle = s.spawn(async {
                channel.publish(2).await; // Should block because capacity is 1 and `sub` hasn't read `1`
            });

            s.run_until_stalled();
            assert!(!handle.is_finished(), "Publisher should be blocked");

            s.block_on(async {
                let val = sub.next().await.unwrap();
                assert_eq!(val, 1);
            });

            s.run_until_stalled();
            assert!(handle.is_finished(), "Publisher should be unblocked after read");
        });
    }

    #[test]
    fn test_broadcast_force_publish() {
        // Capacity 1, max 2 subscribers
        let channel = StackBroadcast::<i32, 1, 2>::new();
        let mut sub1 = channel.subscribe().unwrap();
        let mut sub2 = channel.subscribe().unwrap();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            s.block_on(async {
                channel.publish(10).await; // Bounded queue is now full
                let r1 = sub1.next().await;
                assert_eq!(r1, Ok(10));
            });

            // Force publish 20, which should discard 10
            channel.force_publish(20);

            s.block_on(async {
                // Both subscribers should report missed messages
                let r1 = sub1.next().await;
                let r2 = sub2.next().await;

                assert_eq!(r1, Ok(20));
                assert_eq!(r2, Err(MissedMessages { count: 1 }));

                // Next read should get the new message
                let r2 = sub2.next().await;

                assert_eq!(r2, Ok(20));
            });
        });
    }

    #[cfg(feature = "std")]
    #[test]
    fn test_broadcast_growable() {
        use sapphire_collections::storage::Global;

        struct StdCfg;
        impl BroadcastCfg for StdCfg {
            type Buffer = Global;
            type SubscriptionStore = Global;
            type Mtx = SingleThreadMutex;
        }
        type StdBroadcast<T> = UnfilteredBroadcastChannel<T, StdCfg>;

        let channel = StdBroadcast::<i32>::new();
        let mut sub = channel.subscribe().unwrap();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            s.block_on(async {
                channel.publish(1).await;
                channel.publish(2).await;
                channel.publish(3).await;

                assert_eq!(sub.next().await.unwrap(), 1);
                assert_eq!(sub.next().await.unwrap(), 2);
                assert_eq!(sub.next().await.unwrap(), 3);
            });
        });
    }

    mod proptests {
        use super::*;
        use crate::executor::BoundedExecutor;
        use crate::testing::TestExecutor;
        use proptest::prelude::*;

        use std::collections::VecDeque;

        #[derive(Debug, Clone)]
        enum BroadcastOp {
            Publish(i32),
            ForcePublish(i32),
            RecvSub1,
            RecvSub2,
        }

        type TestBroadcast = StackBroadcast<i32, 2, 2>;

        proptest! {
            #[test]
            fn test_broadcast_proptest(
                ops in prop::collection::vec(
                    prop_oneof![
                        any::<i32>().prop_map(BroadcastOp::Publish),
                        any::<i32>().prop_map(BroadcastOp::ForcePublish),
                        Just(BroadcastOp::RecvSub1),
                        Just(BroadcastOp::RecvSub2),
                    ],
                    0..50
                )
            ) {
                let channel = TestBroadcast::new();
                let mut sub1 = channel.subscribe().unwrap();
                let mut sub2 = channel.subscribe().unwrap();

                let mut expected_vals = VecDeque::new();
                let mut next_global_idx = 0;
                let mut head_global_idx = 0;
                let mut sub1_next = 0;
                let mut sub2_next = 0;

                BoundedExecutor::new(TestExecutor::new(), |s| {
                    for op in ops {
                        let cur_len = next_global_idx - head_global_idx;

                        match op {
                            BroadcastOp::Publish(val) => {
                                if cur_len < 2 {
                                    s.block_on(channel.publish(val));
                                    expected_vals.push_back((next_global_idx, val));
                                    next_global_idx += 1;
                                }
                            }
                            BroadcastOp::ForcePublish(val) => {
                                channel.force_publish(val);
                                if cur_len == 2 {
                                    expected_vals.pop_front();
                                    head_global_idx += 1;
                                }
                                expected_vals.push_back((next_global_idx, val));
                                next_global_idx += 1;
                            }
                            BroadcastOp::RecvSub1 => {
                                if sub1_next < next_global_idx {
                                    let res = s.block_on(sub1.next());
                                    if sub1_next < head_global_idx {
                                        let missed = head_global_idx - sub1_next;
                                        assert_eq!(res, Err(MissedMessages { count: missed }));
                                        sub1_next = head_global_idx;
                                    } else {
                                        let expected_val = expected_vals.iter()
                                            .find(|(idx, _)| *idx == sub1_next)
                                            .map(|(_, val)| *val)
                                            .unwrap();
                                        assert_eq!(res, Ok(expected_val));
                                        sub1_next += 1;
                                    }
                                    head_global_idx = std::cmp::max(head_global_idx, std::cmp::min(sub1_next, sub2_next));
                                    while expected_vals.front().map(|(idx, _)| *idx < head_global_idx).unwrap_or(false) {
                                        expected_vals.pop_front();
                                    }
                                }
                            }
                            BroadcastOp::RecvSub2 => {
                                if sub2_next < next_global_idx {
                                    let res = s.block_on(sub2.next());
                                    if sub2_next < head_global_idx {
                                        let missed = head_global_idx - sub2_next;
                                        assert_eq!(res, Err(MissedMessages { count: missed }));
                                        sub2_next = head_global_idx;
                                    } else {
                                        let expected_val = expected_vals.iter()
                                            .find(|(idx, _)| *idx == sub2_next)
                                            .map(|(_, val)| *val)
                                            .unwrap();
                                        assert_eq!(res, Ok(expected_val));
                                        sub2_next += 1;
                                    }
                                    head_global_idx = std::cmp::max(head_global_idx, std::cmp::min(sub1_next, sub2_next));
                                    while expected_vals.front().map(|(idx, _)| *idx < head_global_idx).unwrap_or(false) {
                                        expected_vals.pop_front();
                                    }
                                }
                            }
                        }
                    }
                });
            }
        }

        #[cfg(feature = "std")]
        use sapphire_collections::storage::Global;

        #[cfg(feature = "std")]
        struct GrowableCfg;
        #[cfg(feature = "std")]
        impl BroadcastCfg for GrowableCfg {
            type Buffer = Global;
            type SubscriptionStore = Global;
            type Mtx = SingleThreadMutex;
        }

        #[cfg(feature = "std")]
        type GrowableBroadcast = UnfilteredBroadcastChannel<i32, GrowableCfg>;

        #[cfg(feature = "std")]
        proptest! {
            #[test]
            fn test_broadcast_growable_proptest(
                ops in prop::collection::vec(
                    prop_oneof![
                        any::<i32>().prop_map(BroadcastOp::Publish),
                        any::<i32>().prop_map(BroadcastOp::ForcePublish),
                        Just(BroadcastOp::RecvSub1),
                        Just(BroadcastOp::RecvSub2),
                    ],
                    0..50
                )
            ) {
                let channel = GrowableBroadcast::new();
                let mut sub1 = channel.subscribe().unwrap();
                let mut sub2 = channel.subscribe().unwrap();

                let mut expected_vals = VecDeque::new();
                let mut next_global_idx = 0;
                let mut head_global_idx = 0;
                let mut sub1_next = 0;
                let mut sub2_next = 0;

                BoundedExecutor::new(TestExecutor::new(), |s| {
                    for op in ops {
                        match op {
                            BroadcastOp::Publish(val) => {
                                s.block_on(channel.publish(val));
                                expected_vals.push_back((next_global_idx, val));
                                next_global_idx += 1;
                            }
                            BroadcastOp::ForcePublish(val) => {
                                channel.force_publish(val);
                                expected_vals.push_back((next_global_idx, val));
                                next_global_idx += 1;
                            }
                            BroadcastOp::RecvSub1 => {
                                if sub1_next < next_global_idx {
                                    let res = s.block_on(sub1.next());
                                    assert!(sub1_next >= head_global_idx);
                                    let expected_val = expected_vals.iter()
                                        .find(|(idx, _)| *idx == sub1_next)
                                        .map(|(_, val)| *val)
                                        .unwrap();
                                    assert_eq!(res, Ok(expected_val));
                                    sub1_next += 1;

                                    head_global_idx = std::cmp::max(head_global_idx, std::cmp::min(sub1_next, sub2_next));
                                    while expected_vals.front().map(|(idx, _)| *idx < head_global_idx).unwrap_or(false) {
                                        expected_vals.pop_front();
                                    }
                                }
                            }
                            BroadcastOp::RecvSub2 => {
                                if sub2_next < next_global_idx {
                                    let res = s.block_on(sub2.next());
                                    assert!(sub2_next >= head_global_idx);
                                    let expected_val = expected_vals.iter()
                                        .find(|(idx, _)| *idx == sub2_next)
                                        .map(|(_, val)| *val)
                                        .unwrap();
                                    assert_eq!(res, Ok(expected_val));
                                    sub2_next += 1;

                                    head_global_idx = std::cmp::max(head_global_idx, std::cmp::min(sub1_next, sub2_next));
                                    while expected_vals.front().map(|(idx, _)| *idx < head_global_idx).unwrap_or(false) {
                                        expected_vals.pop_front();
                                    }
                                }
                            }
                        }
                    }
                });
            }
        }
    }
}
