// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_async as fasync;

/// DropNotifier allows a client to be notified when is is dropped.
///
/// An object that has client that needs to be notified when it is dropped can keep a
/// `DropNotifier` in its member. Client can then request a waiter from it. When the `DropNotifier`
/// is dropped, the `waiter` will be signaled with a PEER_CLOSED event.
#[derive(Debug)]
pub struct DropNotifier {
    _local_event: zx::EventPair,
    notified_event: zx::EventPair,
}

/// A waiter on a `DropNotifier`.
pub type DropWaiter = fasync::RWHandle<zx::EventPair>;

impl DropNotifier {
    /// Get a new waiter on this notifier. It will be notified when this object is dropped.
    pub fn waiter(&self) -> DropWaiter {
        fasync::RWHandle::new(self.event())
    }

    /// Get an event pair that will receive a PEER_CLOSED signal when this object is dropped.
    pub fn event(&self) -> zx::EventPair {
        self.notified_event.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("duplicate event")
    }
}

impl Default for DropNotifier {
    fn default() -> Self {
        let (_local_event, notified_event) = zx::EventPair::create();
        Self { _local_event, notified_event }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::pin;

    #[fuchsia::test]
    async fn check_notifier() {
        let notifier = DropNotifier::default();
        let waiter = notifier.waiter();
        let mut on_closed = pin!(waiter.on_closed());
        assert!(futures::poll!(&mut on_closed).is_pending());
        assert!(!waiter.is_closed());
        std::mem::drop(notifier);
        let on_closed2 = waiter.on_closed();
        assert!(waiter.is_closed());
        assert_eq!(on_closed.await.expect("await"), zx::Signals::EVENTPAIR_PEER_CLOSED);
        assert_eq!(on_closed2.await.expect("await"), zx::Signals::EVENTPAIR_PEER_CLOSED);
    }
}
