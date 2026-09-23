// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Types and traits defining events emitted from core to bindings.

/// A context for emitting events.
///
/// `EventContext` encodes the common pattern for emitting atomic events of type
/// `T` from core. An implementation of `EventContext` must guarantee that
/// events are processed in the order they are emitted.
pub trait EventContext<T> {
    /// Handles `event`.
    fn on_event(&mut self, event: T);
}

#[cfg(any(test, feature = "testutils"))]
pub(crate) mod testutil {
    use super::*;

    use alloc::vec::Vec;
    use core::fmt::Debug;

    /// A fake [`EventContext`].
    pub struct FakeEventCtx<E: Debug> {
        events: Vec<E>,
        must_watch_all_events: bool,
    }

    impl<E: Debug> EventContext<E> for FakeEventCtx<E> {
        fn on_event(&mut self, event: E) {
            self.events.push(event)
        }
    }

    impl<E: Debug> Drop for FakeEventCtx<E> {
        fn drop(&mut self) {
            if self.must_watch_all_events {
                assert!(
                    self.events.is_empty(),
                    "dropped context with unacknowledged events: {:?}",
                    self.events
                );
            }
        }
    }

    impl<E: Debug> Default for FakeEventCtx<E> {
        fn default() -> Self {
            Self { events: Default::default(), must_watch_all_events: false }
        }
    }

    impl<E: Debug> FakeEventCtx<E> {
        /// Takes all events from the context.
        ///
        /// After calling `take`, the caller opts into event watching and must
        /// acknowledge all events before fropping the `FakeEventCtx`.
        pub fn take(&mut self) -> Vec<E> {
            // Any client that calls `take()` is opting into watching events
            // and must watch them all.
            self.must_watch_all_events = true;
            core::mem::take(&mut self.events)
        }
    }
}
