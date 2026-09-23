// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use sapphire_collections::storage::StorageFamily;
use sapphire_sync::mutex::raw::RawMutex;

pub mod filtered;
pub mod unfiltered;

pub use filtered::*;
pub use unfiltered::*;

/// An opaque unique identifier for an active channel subscriber.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SubId(pub(crate) usize);

impl SubId {
    /// Creates a new `SubId` with the given raw identifier value.
    const fn new(id: usize) -> Self {
        Self(id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Payload<T> {
    pub payload: T,
    pub remaining_subs: usize,
}

/// Configuration trait for configuring the types inside an [`UnfilteredBroadcastChannel`] or [`FilteredBroadcastChannel`].
pub trait BroadcastCfg {
    /// The storage container family used for the internal message queue buffer.
    type Buffer: StorageFamily;
    /// The storage container family used for the active subscriber map.
    type SubscriptionStore: StorageFamily;
    /// The raw mutex fundamental used to synchronize internal channel state.
    type Mtx: RawMutex;
}

/// Error indicating that a slow subscriber missed broadcasted messages.
///
/// Occurs when the buffer's capacity was exceeded and oldest elements were evicted
/// via [`UnfilteredBroadcastChannel::force_publish`] before this subscriber could read them.
#[derive(Debug, PartialEq, Eq)]
pub struct MissedMessages {
    /// The exact number of missed messages.
    pub count: usize,
}
