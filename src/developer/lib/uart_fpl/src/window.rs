// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Sliding-window flow control math, sequence validation, and cumulative ACK tracking.

use futures::future::poll_fn;
use futures::task::AtomicWaker;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};

const HAS_ACK_FLAG: u64 = 1 << 63;

/// Evaluates whether an 8-bit sequence number `seq` lies within the active sliding window `[base, next_seq)`.
///
/// Handles 8-bit modular wrapping (e.g. `base = 250`, `next_seq = 5`).
/// When `base == next_seq`, the window is empty and this function always returns `false`.
pub(crate) fn is_seq_in_window(base: u8, next_seq: u8, seq: u8) -> bool {
    seq.wrapping_sub(base) < next_seq.wrapping_sub(base)
}

#[derive(Debug, Default)]
struct AckTrackerInner {
    state: AtomicU64,
    waker: AtomicWaker,
}

/// Thread-safe cumulative ACK tracker with waker notifications.
///
/// In Go-Back-N (and ResendSP), ACKs are cumulative. When receiving multiple
/// in-order frames in rapid succession, `AckTracker` maintains the latest
/// session ID and highest acknowledged sequence number. This prevents reverse-path
/// flooding (e.g. queueing hundreds of redundant individual ACK frames) and
/// ensures the writer task is immediately woken up to emit the cumulative ACK.
///
/// Both `session_id` and `seq` (along with the pending ACK flag) are packed
/// into a single atomic `u64` to prevent mixed-state torn reads between concurrent
/// `set_ack` and `take_ack` calls.
#[derive(Debug, Clone, Default)]
pub struct AckTracker {
    inner: Arc<AckTrackerInner>,
}

impl AckTracker {
    /// Creates a new, empty ACK tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a new cumulative ACK for `session_id` up to `seq` and notifies any waiting writer.
    pub fn set_ack(&self, session_id: u32, seq: u8) {
        let packed = HAS_ACK_FLAG | ((seq as u64) << 32) | (session_id as u64);
        self.inner.state.store(packed, Ordering::Release);
        self.inner.waker.wake();
    }

    /// Retrieves and clears the latest cumulative ACK if one is pending.
    pub fn take_ack(&self) -> Option<(u32, u8)> {
        let val = self.inner.state.swap(0, Ordering::AcqRel);
        if (val & HAS_ACK_FLAG) != 0 {
            let sid = val as u32;
            let seq = ((val >> 32) & 0xFF) as u8;
            Some((sid, seq))
        } else {
            None
        }
    }

    /// Registers the current task waker to be notified when a new ACK is recorded.
    pub fn register_waker(&self, cx: &mut Context<'_>) {
        self.inner.waker.register(cx.waker());
    }

    /// Asynchronously waits until a cumulative ACK is recorded and returns `(session_id, seq)`.
    ///
    /// Note: Designed for a single waiting task. If multiple tasks await `wait_ack()`,
    /// only the most recently registered waker is notified when an ACK arrives.
    pub async fn wait_ack(&self) -> (u32, u8) {
        poll_fn(|cx| {
            if let Some(ack) = self.take_ack() {
                Poll::Ready(ack)
            } else {
                self.register_waker(cx);
                if let Some(ack) = self.take_ack() { Poll::Ready(ack) } else { Poll::Pending }
            }
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}

    #[fuchsia::test]
    fn test_tracker_thread_safe() {
        assert_send::<AckTracker>();
        assert_sync::<AckTracker>();
    }

    #[fuchsia::test]
    async fn test_ack_tracker_wait_ack() {
        let tracker = AckTracker::new();
        tracker.set_ack(0x12345678, 42);
        let (sid, seq) = tracker.wait_ack().await;
        assert_eq!(sid, 0x12345678);
        assert_eq!(seq, 42);
    }

    #[fuchsia::test]
    async fn test_ack_tracker_wait_ack_async_wake() {
        let tracker = AckTracker::new();
        let tracker_clone = tracker.clone();
        let join_handle = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(10));
            tracker_clone.set_ack(0x12345678, 42);
        });
        let (sid, seq) = tracker.wait_ack().await;
        assert_eq!(sid, 0x12345678);
        assert_eq!(seq, 42);
        join_handle.join().unwrap();
    }

    #[fuchsia::test]
    fn test_ack_tracker() {
        let tracker = AckTracker::new();
        assert_eq!(tracker.take_ack(), None);

        tracker.set_ack(0x12345678, 42);
        assert_eq!(tracker.take_ack(), Some((0x12345678, 42)));
        assert_eq!(tracker.take_ack(), None);

        // Coalescing updates
        tracker.set_ack(0x11111111, 1);
        tracker.set_ack(0x11111111, 2);
        assert_eq!(tracker.take_ack(), Some((0x11111111, 2)));
    }

    #[fuchsia::test]
    fn test_is_seq_in_window() {
        // Normal case without wrapping: [10, 20)
        assert!(!is_seq_in_window(10, 20, 9));
        assert!(is_seq_in_window(10, 20, 10));
        assert!(is_seq_in_window(10, 20, 15));
        assert!(is_seq_in_window(10, 20, 19));
        assert!(!is_seq_in_window(10, 20, 20));

        // Empty window: base == next_seq
        assert!(!is_seq_in_window(10, 10, 10));
        assert!(!is_seq_in_window(10, 10, 9));

        // Wrapping case: [250, 10)
        assert!(is_seq_in_window(250, 10, 250));
        assert!(is_seq_in_window(250, 10, 255));
        assert!(is_seq_in_window(250, 10, 0));
        assert!(is_seq_in_window(250, 10, 9));
        assert!(!is_seq_in_window(250, 10, 10));
        assert!(!is_seq_in_window(250, 10, 249));
    }
}
