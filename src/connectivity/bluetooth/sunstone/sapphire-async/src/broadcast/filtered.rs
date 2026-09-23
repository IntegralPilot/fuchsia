// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};

use sapphire_collections::deque::Deque;
use sapphire_collections::map::HashMap;
use sapphire_sync::mutex::Mutex;

use crate::global_index::GlobalIndex;
use crate::notification::Notification;

use super::{BroadcastCfg, MissedMessages, Payload, SubId};

/// The outcome of evaluating a subscriber's interest in a broadcast payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interest {
    /// The subscriber is interested in receiving the payload.
    Interested,
    /// The subscriber is not interested and will skip the payload without being woken.
    Uninterested,
}

impl Interest {
    /// Returns `true` if the filter variant is [`Interest::Interested`].
    pub fn is_interested(self) -> bool {
        matches!(self, Interest::Interested)
    }
}

/// A synchronous filter trait for determining a subscriber's interest in a broadcasted item.
///
/// # Purity
///
/// The filtering logic is meant to be pure as both the publisher and subscriber
/// will call the filter function on the same payload. Failure to implement the filter
/// with a pure function will result in unexpected behavior and/or panics
pub trait Filter {
    /// The item type that this filter inspects.
    type Item;

    /// Evaluates the subscriber's interest in the given payload.
    fn interest(&self, payload: &Self::Item) -> Interest;
}

impl<T> Filter for fn(&T) -> Interest {
    type Item = T;

    fn interest(&self, payload: &T) -> Interest {
        (self)(payload)
    }
}

/// An asynchronous multi-subscriber Broadcast Channel with per-subscriber synchronous message filtering.
///
/// `FilteredBroadcastChannel` allows a publisher to broadcast messages to subscribers while invoking a
/// synchronous [`Filter`] for each subscriber. If a subscriber filter returns `Interest::Uninterested`,
/// the subscriber's global index is incremented to skip the message without waking the subscriber task.
///
/// # Examples
///
/// Basic concurrent publishing with per-subscriber filtering:
///
/// ```
/// use sapphire_async::broadcast::{FilteredBroadcastChannel, BroadcastCfg, Interest};
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
/// let channel = FilteredBroadcastChannel::<i32, MyBroadcastCfg>::new();
/// let mut sub_evens = channel
///     .subscribe(|x: &i32| if *x % 2 == 0 { Interest::Interested } else { Interest::Uninterested })
///     .unwrap();
/// let mut sub_all = channel.subscribe(|_| Interest::Interested).unwrap();
///
/// # BoundedExecutor::new(TestExecutor::new(), |s| {
/// #     s.block_on(async {
/// channel.publish(41).await;
/// channel.publish(42).await;
/// assert_eq!(sub_evens.next().await, Ok(42));
/// assert_eq!(sub_all.next().await, Ok(41));
/// assert_eq!(sub_all.next().await, Ok(42));
/// #     });
/// # });
/// ```
///
/// A `FilteredBroadcastChannel` configured with `SingleThreadMutex` cannot be shared across threads:
///
/// ```compile_fail
/// use sapphire_async::broadcast::{FilteredBroadcastChannel, BroadcastCfg};
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
/// type NonSyncBroadcast = FilteredBroadcastChannel<i32, SingleThreadedCfg>;
///
/// fn assert_sync<T: Sync>() {}
/// assert_sync::<NonSyncBroadcast>(); // Correctly fails to compile because SingleThreadMutex is !Sync
/// ```
pub struct FilteredBroadcastChannel<T, Cfg: BroadcastCfg, F = fn(&T) -> Interest> {
    state: Mutex<Cfg::Mtx, FilteredBroadcastChannelState<T, Cfg, F>>,
    not_full: Notification<Cfg::Mtx>,
}

/// The synchronized internal state of a [`FilteredBroadcastChannel`].
struct FilteredBroadcastChannelState<T, Cfg: BroadcastCfg, F> {
    queue: Deque<Payload<T>, Cfg::Buffer>,
    head_global_idx: GlobalIndex,
    next_global_idx: GlobalIndex,
    subscribers: HashMap<SubId, FilteredSubscriberState<F>, Cfg::SubscriptionStore>,
    next_sub_id: usize,
}

/// The trackable state of an active subscriber in a [`FilteredBroadcastChannel`].
struct FilteredSubscriberState<F> {
    next_interesting_message: Option<GlobalIndex>,
    filter: F,
    waker: Option<Waker>,
}

/// An active subscriber endpoint to a [`FilteredBroadcastChannel`].
pub struct FilteredSubscriber<'a, T, Cfg: BroadcastCfg, F: Filter<Item = T> = fn(&T) -> Interest> {
    channel: &'a FilteredBroadcastChannel<T, Cfg, F>,
    id: SubId,
}

/// A custom future returned by [`FilteredSubscriber::next`].
pub struct NextFuture<'a, 's, T, Cfg: BroadcastCfg, F: Filter<Item = T>> {
    subscriber: &'s mut FilteredSubscriber<'a, T, Cfg, F>,
}

impl<T, Cfg: BroadcastCfg, F: Filter<Item = T>> FilteredBroadcastChannelState<T, Cfg, F> {
    fn force_publish(&mut self, payload: T, not_full: &Notification<Cfg::Mtx>) -> Option<T> {
        let interested = self
            .subscribers
            .values_mut()
            .filter(|sub| sub.filter.interest(&payload).is_interested())
            .fold(0, |count, sub| {
                if let Some(waker) = sub.waker.take() {
                    waker.wake();
                }

                sub.next_interesting_message.get_or_insert(self.next_global_idx);
                count + 1
            });
        if interested != 0 {
            let prev = self.queue.force_push_back(Payload { payload, remaining_subs: interested });
            if prev.is_some() {
                self.head_global_idx += 1;
            }
            self.next_global_idx += 1;
            self.reclaim_space(not_full);
            prev.map(|payload| payload.payload)
        } else {
            None
        }
    }

    /// Attempts to push a payload to the back of the queue, incrementing `next_global_idx` on success.
    ///
    /// Returns `Err(payload)` if the queue is full and cannot grow.
    fn try_publish(&mut self, payload: T, not_full: &Notification<Cfg::Mtx>) -> Result<(), T> {
        if self.queue.try_reserve(1).is_err() {
            return Err(payload);
        }
        assert!(
            self.force_publish(payload, not_full).is_none(),
            "Must not evict since reserve succeeded"
        );
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
}

impl<T, Cfg: BroadcastCfg, F: Filter<Item = T>> Default for FilteredBroadcastChannel<T, Cfg, F>
where
    HashMap<SubId, FilteredSubscriberState<F>, Cfg::SubscriptionStore>: Default,
    Deque<Payload<T>, Cfg::Buffer>: Default,
{
    fn default() -> Self {
        Self {
            state: Mutex::new(FilteredBroadcastChannelState {
                queue: Deque::default(),
                head_global_idx: GlobalIndex::new(0),
                next_global_idx: GlobalIndex::new(0),
                subscribers: HashMap::default(),
                next_sub_id: 0,
            }),
            not_full: Notification::new(),
        }
    }
}

impl<T: Clone, Cfg: BroadcastCfg, F: Filter<Item = T>> FilteredBroadcastChannel<T, Cfg, F> {
    /// Creates a new, empty `FilteredBroadcastChannel`.
    pub fn new() -> Self
    where
        Self: Default,
    {
        Self::default()
    }

    /// Subscribes to the channel with a synchronous filter.
    pub fn subscribe(&self, filter: F) -> Option<FilteredSubscriber<'_, T, Cfg, F>> {
        let mut state = self.state.lock();
        let id = SubId::new(state.next_sub_id);
        state.next_sub_id += 1;

        state
            .subscribers
            .try_insert(
                id,
                FilteredSubscriberState { filter, waker: None, next_interesting_message: None },
            )
            .ok()?;

        Some(FilteredSubscriber { channel: self, id })
    }

    /// Publishes a payload to the filtered channel.
    pub async fn publish(&self, payload: T) {
        let mut payload = Some(payload);
        let guard = self.state.lock();

        self.not_full
            .when(guard, |state| {
                let item = payload.take().expect("Payload not refreshed");
                match state.try_publish(item, &self.not_full) {
                    Ok(()) => Poll::Ready(()),
                    Err(item) => {
                        payload.replace(item);
                        Poll::Pending
                    }
                }
            })
            .await;
    }

    /// Publishes a payload, evicting the oldest message if full.
    ///
    /// Returns the evicted message
    pub fn force_publish(&self, payload: T) -> Option<T> {
        let state = &mut *self.state.lock();
        state.force_publish(payload, &self.not_full)
    }
}

impl<'a, T: Clone, Cfg: BroadcastCfg, F: Filter<Item = T>> FilteredSubscriber<'a, T, Cfg, F> {
    /// Asynchronously polls and retrieves the next broadcasted message that matched the filter.
    pub fn next<'s>(&'s mut self) -> NextFuture<'a, 's, T, Cfg, F> {
        NextFuture { subscriber: self }
    }
}

impl<'a, 's, T: Clone, Cfg: BroadcastCfg, F: Filter<Item = T>> Future
    for NextFuture<'a, 's, T, Cfg, F>
{
    type Output = Result<T, MissedMessages>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let state = &mut *self.subscriber.channel.state.lock();

        let qhead = state.head_global_idx;
        let sub = state.subscribers.get_mut(&self.subscriber.id).expect("Subscriber not found");

        let Some(mut idx) = sub.next_interesting_message.take() else {
            sub.waker = Some(cx.waker().clone());
            return Poll::Pending;
        };

        let logical_idx = (idx - qhead) as usize;
        let out = match state.queue.get_mut(logical_idx) {
            Some(item) => {
                debug_assert!(sub.filter.interest(&item.payload).is_interested());
                item.remaining_subs =
                    item.remaining_subs.checked_sub(1).expect("remaining_subs underflow");
                idx += 1;
                Ok(item.payload.clone())
            }
            None => {
                let missed = (qhead - idx) as usize;
                idx = qhead;
                Err(MissedMessages { count: missed })
            }
        };

        // Fast-forward to the next interesting message
        while let Some(item) = state.queue.get((idx - qhead) as usize) {
            if sub.filter.interest(&item.payload).is_interested() {
                sub.next_interesting_message = Some(idx);
                break;
            }
            idx += 1;
        }

        state.reclaim_space(&self.subscriber.channel.not_full);
        Poll::Ready(out)
    }
}

impl<'a, 's, T, Cfg: BroadcastCfg, F: Filter<Item = T>> Drop for NextFuture<'a, 's, T, Cfg, F> {
    fn drop(&mut self) {
        let mut state = self.subscriber.channel.state.lock();
        if let Some(sub) = state.subscribers.get_mut(&self.subscriber.id) {
            sub.waker = None;
        }
    }
}

impl<'a, T, Cfg: BroadcastCfg, F: Filter<Item = T>> Drop for FilteredSubscriber<'a, T, Cfg, F> {
    fn drop(&mut self) {
        let mut state = self.channel.state.lock();
        if let Some(sub) = state.subscribers.remove(&self.id)
            && let Some(mut idx) = sub.next_interesting_message
        {
            let head = state.head_global_idx;
            if idx < head {
                idx = head;
            }
            while let Some(item) = state.queue.get_mut((idx - head) as usize) {
                if sub.filter.interest(&item.payload).is_interested() {
                    item.remaining_subs =
                        item.remaining_subs.checked_sub(1).expect("remaining_subs underflow");
                }
                idx += 1;
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

    #[test]
    fn test_filtered_broadcast_basic() {
        type TestFilteredChannel = FilteredBroadcastChannel<i32, StackCfg<10, 2>>;
        let channel = TestFilteredChannel::new();

        let mut sub1 = channel
            .subscribe(|x| if x % 2 == 0 { Interest::Interested } else { Interest::Uninterested })
            .unwrap();

        let mut sub2 = channel
            .subscribe(|x| if x % 2 != 0 { Interest::Interested } else { Interest::Uninterested })
            .unwrap();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            s.block_on(async {
                channel.publish(1).await;
                channel.publish(2).await;
                channel.publish(3).await;
                channel.publish(4).await;

                assert_eq!(sub1.next().await, Ok(2));
                assert_eq!(sub1.next().await, Ok(4));

                assert_eq!(sub2.next().await, Ok(1));
                assert_eq!(sub2.next().await, Ok(3));
            });
        });
    }

    #[test]
    fn test_filtered_broadcast_selective_wake() {
        type TestFilteredChannel = FilteredBroadcastChannel<i32, StackCfg<10, 2>>;
        let channel = TestFilteredChannel::new();

        let mut sub1 = channel
            .subscribe(|x| if *x == 42 { Interest::Interested } else { Interest::Uninterested })
            .unwrap();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            let h1 = s.spawn(async move { sub1.next().await });

            s.run_until_stalled();
            assert!(!h1.is_finished());

            s.block_on(async {
                channel.publish(10).await;
            });
            s.run_until_stalled();
            assert!(!h1.is_finished());

            s.block_on(async {
                channel.publish(42).await;
            });
            s.run_until_stalled();
            assert!(h1.is_finished());
        });
    }

    #[test]
    fn test_filtered_broadcast_force_publish() {
        type TestFilteredChannel = FilteredBroadcastChannel<i32, StackCfg<1, 2>>;
        let channel = TestFilteredChannel::new();

        let mut sub1 = channel
            .subscribe(|x| if x % 2 == 0 { Interest::Interested } else { Interest::Uninterested })
            .unwrap();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            s.block_on(async {
                channel.publish(2).await;
            });

            channel.force_publish(4);

            s.block_on(async {
                assert_eq!(sub1.next().await, Err(MissedMessages { count: 1 }));
                assert_eq!(sub1.next().await, Ok(4));
            });
        });
    }

    #[test]
    fn test_filtered_broadcast_publish_uninteresting_does_not_consume_capacity() {
        // Buffer capacity 1, max 1 subscriber
        type TestFilteredChannel = FilteredBroadcastChannel<i32, StackCfg<1, 1>>;
        let channel = TestFilteredChannel::new();

        let mut sub = channel
            .subscribe(|x| if *x % 2 == 0 { Interest::Interested } else { Interest::Uninterested })
            .unwrap();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            s.block_on(async {
                // Publishing uninteresting (odd) messages on a capacity-1 queue
                // does not consume queue capacity and does not block.
                for odd in [1, 3, 5, 7, 9] {
                    channel.publish(odd).await;
                    channel.force_publish(odd);
                }

                // publishing an interested (even) message should succeed and be received by the subscriber.
                channel.publish(42).await;
                assert_eq!(sub.next().await, Ok(42));
            });
        });
    }

    #[test]
    fn test_filtered_broadcast_publish_uninteresting_does_not_block() {
        // Buffer capacity 1, max 1 subscriber
        type TestFilteredChannel = FilteredBroadcastChannel<i32, StackCfg<2, 1>>;
        let channel = TestFilteredChannel::new();

        let mut sub = channel
            .subscribe(|x| if *x % 2 == 0 { Interest::Interested } else { Interest::Uninterested })
            .unwrap();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            s.block_on(async {
                // publishing an interested (even) message should succeed and be received by the subscriber.
                channel.publish(42).await;

                // Publishing uninteresting (odd) messages on a capacity-1 queue
                // does not consume queue capacity and does not block.
                for odd in [1, 3, 5, 7, 9] {
                    channel.publish(odd).await;
                    channel.force_publish(odd);
                }

                assert_eq!(sub.next().await, Ok(42));
            });
        });
    }

    #[test]
    fn test_filtered_broadcast_force_publish_uninteresting_does_not_evict() {
        // Buffer capacity 1, max 1 subscriber
        type TestFilteredChannel = FilteredBroadcastChannel<i32, StackCfg<1, 1>>;
        let channel = TestFilteredChannel::new();

        let mut sub = channel
            .subscribe(|x| if *x % 2 == 0 { Interest::Interested } else { Interest::Uninterested })
            .unwrap();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            s.block_on(async {
                // 1. Fill the capacity-1 queue with an interested message.
                channel.publish(10).await;
            });

            // 2. Force publish an uninteresting (odd) message when queue is full.
            channel.force_publish(11);

            s.block_on(async {
                // 3. The subscriber should receive 10 intact without MissedMessages because 10 was not evicted.
                assert_eq!(sub.next().await, Ok(10));
            });
        });
    }

    #[test]
    fn test_filtered_broadcast_blocking_publisher_unblock_on_read() {
        // Capacity 1, max 2 subscribers
        type TestFilteredChannel = FilteredBroadcastChannel<i32, StackCfg<1, 2>>;
        let channel = TestFilteredChannel::new();

        let mut sub1 = channel
            .subscribe(|x| if *x % 2 == 0 { Interest::Interested } else { Interest::Uninterested })
            .unwrap();
        let mut sub2 = channel.subscribe(|_| Interest::Interested).unwrap();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            s.block_on(async {
                channel.publish(1).await; // sub1 skips (uninterested), sub2 keeps (interested)
            });

            // Channel buffer has msg 1 (read by sub1, but unread by sub2). Buffer is full (capacity 1).
            let handle = s.spawn(async {
                channel.publish(2).await; // Should block because sub2 hasn't read 1
            });

            s.run_until_stalled();
            assert!(!handle.is_finished(), "Publisher should be blocked waiting for sub2");

            s.block_on(async {
                let val = sub2.next().await.unwrap();
                assert_eq!(val, 1);
            });

            s.run_until_stalled();
            assert!(handle.is_finished(), "Publisher should be unblocked after sub2 reads");

            s.block_on(async {
                assert_eq!(sub1.next().await, Ok(2));
                assert_eq!(sub2.next().await, Ok(2));
            });
        });
    }

    #[test]
    fn test_filtered_broadcast_blocking_publisher_unblock_on_skip() {
        // Capacity 2, max 2 subscribers
        type TestFilteredChannel = FilteredBroadcastChannel<i32, StackCfg<2, 2>>;
        let channel = TestFilteredChannel::new();

        let mut sub1 = channel
            .subscribe(|x| if *x % 2 == 0 { Interest::Interested } else { Interest::Uninterested })
            .unwrap();
        let mut sub2 = channel.subscribe(|_| Interest::Interested).unwrap();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            s.block_on(async {
                channel.publish(2).await; // Both interested, sub1 and sub2 stay at idx 0
                channel.publish(3).await; // sub1 uninterested, sub2 interested, so 3 is enqueued
            });

            // Queue now has [2, 3] and is full (capacity 2).
            let handle1 = s.spawn(async {
                channel.publish(4).await;
            });
            s.run_until_stalled();
            assert!(!handle1.is_finished(), "Publisher should block when queue is full");

            // sub1 reads 2 and skips 3. sub2 reads 2, freeing slot 2.
            s.block_on(async {
                assert_eq!(sub1.next().await, Ok(2));
                assert_eq!(sub2.next().await, Ok(2));
            });

            // Space for 2 is reclaimed; publisher unblocks and publishes 4.
            // Queue now has [3, 4] (len 2, capacity 2).
            s.run_until_stalled();
            assert!(
                handle1.is_finished(),
                "Publisher should unblock after reading 2 and freeing a slot"
            );

            // sub2 reads 3, freeing slot 3.
            // Queue now has [4] (len 1, capacity 2).
            s.block_on(async {
                assert_eq!(sub2.next().await, Ok(3));
            });

            // Since capacity is 2 and queue only contains [4], publishing 6 succeeds immediately.
            s.block_on(async {
                channel.publish(6).await;
            });

            // Queue now has [4, 6] and is full (capacity 2).
            let handle2 = s.spawn(async {
                channel.publish(8).await;
            });
            s.run_until_stalled();
            assert!(!handle2.is_finished(), "Publisher should block when queue is full again");

            // sub1 and sub2 read 4, which reclaims slot 4 and unblocks the publisher for 8.
            s.block_on(async {
                assert_eq!(sub1.next().await, Ok(4));
                assert_eq!(sub2.next().await, Ok(4));
            });

            s.run_until_stalled();
            assert!(handle2.is_finished(), "Publisher should unblock after reading 4");

            s.block_on(async {
                assert_eq!(sub1.next().await, Ok(6));
                assert_eq!(sub2.next().await, Ok(6));
                assert_eq!(sub1.next().await, Ok(8));
                assert_eq!(sub2.next().await, Ok(8));
            });
        });
    }

    #[test]
    fn test_filtered_broadcast_drop_subscriber_unblocks_publisher() {
        // Capacity 1, max 2 subscribers
        type TestFilteredChannel = FilteredBroadcastChannel<i32, StackCfg<1, 2>>;
        let channel = TestFilteredChannel::new();

        let mut sub1 = channel.subscribe(|_| Interest::Interested).unwrap();
        let sub2 = channel.subscribe(|_| Interest::Interested).unwrap();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            s.block_on(async {
                channel.publish(1).await;
                assert_eq!(sub1.next().await, Ok(1));
            });

            // sub1 read 1 (sub1 at idx 1), sub2 did not read 1 (sub2 at idx 0).
            // Buffer contains [1] and is full (capacity 1).
            let handle = s.spawn(async {
                channel.publish(2).await;
            });
            s.run_until_stalled();
            assert!(!handle.is_finished(), "Publisher should be blocked waiting for sub2");

            // Dropping sub2 reclaims space held by sub2
            drop(sub2);

            s.run_until_stalled();
            assert!(handle.is_finished(), "Publisher should be unblocked after sub2 is dropped");

            s.block_on(async {
                assert_eq!(sub1.next().await, Ok(2));
            });
        });
    }

    #[test]
    fn test_filtered_broadcast_drop_all_subscribers_unblocks_publisher() {
        // Capacity 1, max 1 subscriber
        type TestFilteredChannel = FilteredBroadcastChannel<i32, StackCfg<1, 1>>;
        let channel = TestFilteredChannel::new();

        let sub = channel.subscribe(|_| Interest::Interested).unwrap();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            s.block_on(async {
                channel.publish(10).await;
            });

            let handle = s.spawn(async {
                channel.publish(20).await;
            });
            s.run_until_stalled();
            assert!(!handle.is_finished(), "Publisher should be blocked");

            drop(sub);

            s.run_until_stalled();
            assert!(
                handle.is_finished(),
                "Publisher should be unblocked after all subscribers dropped"
            );
        });
    }

    #[test]
    fn test_filtered_broadcast_drop_next_future_pending_and_poll_again() {
        type TestFilteredChannel = FilteredBroadcastChannel<i32, StackCfg<5, 2>>;
        let channel = TestFilteredChannel::new();

        let mut sub = channel
            .subscribe(|x| if *x % 2 == 0 { Interest::Interested } else { Interest::Uninterested })
            .unwrap();

        // 1. Spawn a task that polls sub.next() while channel is empty.
        BoundedExecutor::new(TestExecutor::new(), |s| {
            let handle = s.spawn(async { sub.next().await });
            s.run_until_stalled();
            assert!(!handle.is_finished(), "NextFuture should be pending");

            // 2. Cancel/drop the task while NextFuture is pending.
            handle.cancel();
        });

        // 3. Verify sub.waker was cleared by checking internal state.
        {
            let state = channel.state.lock();
            let sub_state = state.subscribers.get(&sub.id).unwrap();
            assert!(sub_state.waker.is_none(), "Waker should be cleared on NextFuture drop");
        }

        BoundedExecutor::new(TestExecutor::new(), |s| {
            // 4. Spawn a new task polling sub.next().
            let handle2 = s.spawn(async { sub.next().await });
            s.run_until_stalled();
            assert!(!handle2.is_finished(), "New NextFuture should be pending");

            // 5. Publish uninterested message (1).
            s.block_on(async {
                channel.publish(1).await;
            });
            s.run_until_stalled();
            assert!(!handle2.is_finished(), "Should not wake on uninterested message");

            // 6. Publish interested message (2).
            s.block_on(async {
                channel.publish(2).await;
            });
            s.run_until_stalled();
            assert!(handle2.is_finished(), "Should wake and finish on interested message");

            s.block_on(async {
                assert_eq!(handle2.join().await, Ok(2));
            });
        });
    }

    #[test]
    fn test_filtered_broadcast_drop_next_future_with_unfiltered_backlog() {
        type TestFilteredChannel = FilteredBroadcastChannel<i32, StackCfg<5, 2>>;
        let channel = TestFilteredChannel::new();

        let mut sub = channel
            .subscribe(|x| if *x % 2 == 0 { Interest::Interested } else { Interest::Uninterested })
            .unwrap();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            s.block_on(async {
                channel.publish(1).await;
                channel.publish(3).await;
            });

            // Start next() future: should be pending because no even numbers.
            let handle = s.spawn(async { sub.next().await });
            s.run_until_stalled();
            assert!(!handle.is_finished());

            // Cancel the future
            handle.cancel();
        });

        BoundedExecutor::new(TestExecutor::new(), |s| {
            // Publish another odd and an even
            s.block_on(async {
                channel.publish(5).await;
                channel.publish(6).await;
            });

            // Next read should find 6
            s.block_on(async {
                assert_eq!(sub.next().await, Ok(6));
            });
        });
    }

    #[cfg(feature = "std")]
    #[test]
    fn test_filtered_broadcast_growable() {
        use sapphire_collections::storage::Global;

        struct StdCfg;
        impl BroadcastCfg for StdCfg {
            type Buffer = Global;
            type SubscriptionStore = Global;
            type Mtx = SingleThreadMutex;
        }
        type StdBroadcast<T> = FilteredBroadcastChannel<T, StdCfg>;

        let channel = StdBroadcast::<i32>::new();
        let mut sub1 = channel
            .subscribe(|x| if *x % 2 == 0 { Interest::Interested } else { Interest::Uninterested })
            .unwrap();
        let mut sub2 = channel
            .subscribe(|x| if *x % 2 != 0 { Interest::Interested } else { Interest::Uninterested })
            .unwrap();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            s.block_on(async {
                channel.publish(1).await;
                channel.publish(2).await;
                channel.publish(3).await;
                channel.publish(4).await;

                assert_eq!(sub1.next().await.unwrap(), 2);
                assert_eq!(sub1.next().await.unwrap(), 4);

                assert_eq!(sub2.next().await.unwrap(), 1);
                assert_eq!(sub2.next().await.unwrap(), 3);
            });
        });
    }

    #[test]
    fn test_filtered_broadcast_custom_filter() {
        struct ThresholdFilter {
            min: i32,
        }

        impl Filter for ThresholdFilter {
            type Item = i32;

            fn interest(&self, payload: &i32) -> Interest {
                if *payload >= self.min { Interest::Interested } else { Interest::Uninterested }
            }
        }

        type TestChannel = FilteredBroadcastChannel<i32, StackCfg<10, 2>, ThresholdFilter>;
        let channel = TestChannel::new();

        let mut sub = channel.subscribe(ThresholdFilter { min: 25 }).unwrap();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            s.block_on(async {
                channel.publish(10).await;
                channel.publish(20).await;
                channel.publish(30).await;
                channel.publish(40).await;

                assert_eq!(sub.next().await, Ok(30));
                assert_eq!(sub.next().await, Ok(40));
            });
        });
    }

    mod proptests {
        use super::*;
        use crate::executor::BoundedExecutor;
        use crate::semaphore::Semaphore;
        use crate::testing::TestExecutor;
        use proptest::prelude::*;
        use sapphire_sync::mutex::raw::SingleThreadMutex;

        #[derive(Debug, Clone)]
        enum BroadcastOp {
            Publish(i32),
            ForcePublish(i32),
            RecvSub1,
            RecvSub2,
        }

        fn filter1(x: &i32) -> Interest {
            if *x % 2 == 0 { Interest::Interested } else { Interest::Uninterested }
        }

        fn filter2(x: &i32) -> Interest {
            if *x % 3 == 0 { Interest::Interested } else { Interest::Uninterested }
        }

        proptest! {
            #[test]
            fn test_filtered_broadcast_only_receives_interested(
                ops in prop::collection::vec(
                    prop_oneof![
                        any::<i32>().prop_map(BroadcastOp::Publish),
                        any::<i32>().prop_map(BroadcastOp::ForcePublish),
                        Just(BroadcastOp::RecvSub1),
                        Just(BroadcastOp::RecvSub2),
                    ],
                    0..100
                )
            ) {
                type TestChannel = FilteredBroadcastChannel<i32, StackCfg<4, 2>>;
                let channel = TestChannel::new();

                let mut sub1 = channel.subscribe(filter1).unwrap();
                let mut sub2 = channel.subscribe(filter2).unwrap();

                let sem1 = Semaphore::<SingleThreadMutex>::new(0);
                let sem2 = Semaphore::<SingleThreadMutex>::new(0);

                let chan = &channel;
                let s1 = &sem1;
                let s2 = &sem2;
                BoundedExecutor::new(TestExecutor::new(), |s| {
                    let h1 = s.spawn(async move {
                        loop {
                            s1.down().await;
                            match sub1.next().await {
                                Ok(val) => {
                                    assert_eq!(filter1(&val), Interest::Interested, "sub1 received uninterested message: {val}");
                                }
                                Err(MissedMessages { .. }) => {}
                            }
                        }
                    });
                    let h2 = s.spawn(async move {
                        loop {
                            s2.down().await;
                            match sub2.next().await {
                                Ok(val) => {
                                    assert_eq!(filter2(&val), Interest::Interested, "sub2 received uninterested message: {val}");
                                }
                                Err(MissedMessages { .. }) => {}
                            }
                        }
                    });

                    for op in ops {
                        match op {
                            BroadcastOp::Publish(val) => {
                                s.spawn(async move {
                                    chan.publish(val).await;
                                });
                                s.run_until_stalled();
                            }
                            BroadcastOp::ForcePublish(val) => {
                                chan.force_publish(val);
                                s.run_until_stalled();
                            }
                            BroadcastOp::RecvSub1 => {
                                sem1.up();
                                s.run_until_stalled();
                            }
                            BroadcastOp::RecvSub2 => {
                                sem2.up();
                                s.run_until_stalled();
                            }
                        }
                    }

                    h1.cancel();
                    h2.cancel();
                });
            }

            #[test]
            fn test_filtered_broadcast_up_to_date_subscribers_never_miss(
                ops in prop::collection::vec(
                    prop_oneof![
                        any::<i32>().prop_map(|v| (false, v)),
                        any::<i32>().prop_map(|v| (true, v)),
                    ],
                    0..100
                )
            ) {
                // Use minimum capacity 1 to strictly test boundary eviction/backpressure conditions
                type TestChannel = FilteredBroadcastChannel<i32, StackCfg<1, 2>>;
                let channel = TestChannel::new();

                let mut sub1 = channel.subscribe(filter1).unwrap();
                let mut sub2 = channel.subscribe(filter2).unwrap();

                let sem1 = Semaphore::<SingleThreadMutex>::new(0);
                let sem2 = Semaphore::<SingleThreadMutex>::new(0);

                let chan = &channel;
                let s1 = &sem1;
                let s2 = &sem2;
                BoundedExecutor::new(TestExecutor::new(), |s| {
                    let h1 = s.spawn(async move {
                        loop {
                            s1.down().await;
                            match sub1.next().await {
                                Ok(_) => {}
                                Err(MissedMessages { count }) => {
                                    panic!("Up-to-date sub1 should never receive MissedMessages, missed {count}");
                                }
                            }
                        }
                    });
                    let h2 = s.spawn(async move {
                        loop {
                            s2.down().await;
                            match sub2.next().await {
                                Ok(_) => {}
                                Err(MissedMessages { count }) => {
                                    panic!("Up-to-date sub2 should never receive MissedMessages, missed {count}");
                                }
                            }
                        }
                    });

                    for (is_force, val) in ops {
                        if is_force {
                            chan.force_publish(val);
                        } else {
                            s.spawn(async move {
                                chan.publish(val).await;
                            });
                        }
                        sem1.up();
                        sem2.up();
                        s.run_until_stalled();
                    }

                    h1.cancel();
                    h2.cancel();
                });
            }

            #[test]
            fn test_filtered_broadcast_no_force_publish_never_misses(
                ops in prop::collection::vec(
                    prop_oneof![
                        any::<i32>().prop_map(BroadcastOp::Publish),
                        Just(BroadcastOp::RecvSub1),
                        Just(BroadcastOp::RecvSub2),
                    ],
                    0..100
                )
            ) {
                type TestChannel = FilteredBroadcastChannel<i32, StackCfg<2, 2>>;
                let channel = TestChannel::new();

                let mut sub1 = channel.subscribe(filter1).unwrap();
                let mut sub2 = channel.subscribe(filter2).unwrap();

                let sem1 = Semaphore::<SingleThreadMutex>::new(0);
                let sem2 = Semaphore::<SingleThreadMutex>::new(0);

                let chan = &channel;
                let s1 = &sem1;
                let s2 = &sem2;
                BoundedExecutor::new(TestExecutor::new(), |s| {
                    let h1 = s.spawn(async move {
                        loop {
                            s1.down().await;
                            match sub1.next().await {
                                Ok(_) => {}
                                Err(MissedMessages { count }) => {
                                    panic!("Without force_publish, sub1 must never receive MissedMessages, missed {count}");
                                }
                            }
                        }
                    });
                    let h2 = s.spawn(async move {
                        loop {
                            s2.down().await;
                            match sub2.next().await {
                                Ok(_) => {}
                                Err(MissedMessages { count }) => {
                                    panic!("Without force_publish, sub2 must never receive MissedMessages, missed {count}");
                                }
                            }
                        }
                    });

                    for op in ops {
                        match op {
                            BroadcastOp::Publish(val) => {
                                s.spawn(async move {
                                    chan.publish(val).await;
                                });
                                s.run_until_stalled();
                            }
                            BroadcastOp::RecvSub1 => {
                                sem1.up();
                                s.run_until_stalled();
                            }
                            BroadcastOp::RecvSub2 => {
                                sem2.up();
                                s.run_until_stalled();
                            }
                            BroadcastOp::ForcePublish(_) => {}
                        }
                    }

                    h1.cancel();
                    h2.cancel();
                });
            }
        }

        proptest! {
            #[test]
            fn test_filtered_broadcast_used_capacity_dictated_by_slowest_reader(
                ops in prop::collection::vec(
                    prop_oneof![
                        any::<i32>().prop_map(BroadcastOp::Publish),
                        any::<i32>().prop_map(BroadcastOp::ForcePublish),
                        Just(BroadcastOp::RecvSub1),
                        Just(BroadcastOp::RecvSub2),
                    ],
                    0..100
                )
            ) {
                type TestChannel = FilteredBroadcastChannel<i32, StackCfg<4, 2>>;
                let channel = TestChannel::new();

                let mut sub1 = channel.subscribe(filter1).unwrap();
                let mut sub2 = channel.subscribe(filter2).unwrap();

                let sub1_id = sub1.id;
                let sub2_id = sub2.id;

                let sem1 = Semaphore::<SingleThreadMutex>::new(0);
                let sem2 = Semaphore::<SingleThreadMutex>::new(0);

                let chan = &channel;
                let s1 = &sem1;
                let s2 = &sem2;
                BoundedExecutor::new(TestExecutor::new(), |s| {
                    let h1 = s.spawn(async move {
                        loop {
                            s1.down().await;
                            let _ = sub1.next().await;
                        }
                    });
                    let h2 = s.spawn(async move {
                        loop {
                            s2.down().await;
                            let _ = sub2.next().await;
                        }
                    });

                    for op in ops {
                        match op {
                            BroadcastOp::Publish(val) => {
                                s.spawn(async move {
                                    chan.publish(val).await;
                                });
                                s.run_until_stalled();
                            }
                            BroadcastOp::RecvSub1 => {
                                sem1.up();
                                s.run_until_stalled();
                            }
                            BroadcastOp::RecvSub2 => {
                                sem2.up();
                                s.run_until_stalled();
                            }
                            BroadcastOp::ForcePublish(val) => {
                                chan.force_publish(val);
                                s.run_until_stalled();
                            }
                        }

                        let state = chan.state.lock();
                        let total_buffered = (state.next_global_idx - state.head_global_idx) as usize;
                        let sub1_lag = state
                            .subscribers
                            .get(&sub1_id)
                            .and_then(|s| s.next_interesting_message)
                            .map(|s| (state.next_global_idx - s) as usize)
                            .unwrap_or(0);
                        let sub2_lag = state
                            .subscribers
                            .get(&sub2_id)
                            .and_then(|s| s.next_interesting_message)
                            .map(|s| (state.next_global_idx - s) as usize)
                            .unwrap_or(0);

                        let max_sub_lag = std::cmp::max(sub1_lag, sub2_lag);
                        let expected_used_capacity = std::cmp::min(total_buffered, max_sub_lag);

                        assert_eq!(state.queue.len(), expected_used_capacity);
                    }

                    h1.cancel();
                    h2.cancel();
                });
            }
        }
    }
}
