// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Event pairs supporting readable/writable signals for sockets.

use std::sync::Arc;

use log::error;
use netstack3_core::socket::SocketWritableListener;
use zx::Peered as _;

use crate::bindings::socket::queue::{QueueErrorListener, QueueReadableListener};
use crate::bindings::socket::{ZXSIO_SIGNAL_ERROR, ZXSIO_SIGNAL_INCOMING, ZXSIO_SIGNAL_OUTGOING};

/// A shared instance of [`zx::EventPair`] that can be signaled as
/// readable/writable.
#[derive(Debug, Clone)]
pub(crate) struct SocketEventPair(Arc<zx::EventPair>);

impl SocketEventPair {
    /// Creates a new shared eventpair.
    ///
    /// Returns the shared eventpair + the peer handle for it.
    pub(crate) fn create() -> (Self, zx::EventPair) {
        let (local, peer) = zx::EventPair::create();
        // Sockets are always writable on creation.
        signal_zx_event(&local, ZXSIO_SIGNAL_OUTGOING, true);
        (Self(Arc::new(local)), peer)
    }
}

impl SocketWritableListener for SocketEventPair {
    fn on_writable_changed(&mut self, writable: bool) {
        let Self(event) = self;
        signal_zx_event(&*event, ZXSIO_SIGNAL_OUTGOING, writable);
    }
}

impl QueueReadableListener for SocketEventPair {
    fn on_readable_changed(&mut self, readable: bool) {
        let Self(event) = self;
        signal_zx_event(event, ZXSIO_SIGNAL_INCOMING, readable);
    }
}

impl QueueErrorListener for SocketEventPair {
    fn on_error_changed(&mut self, error: bool) {
        let Self(event) = self;
        signal_zx_event(event, ZXSIO_SIGNAL_ERROR, error);
    }
}

impl QueueReadableListener for zx::EventPair {
    fn on_readable_changed(&mut self, readable: bool) {
        signal_zx_event(self, ZXSIO_SIGNAL_INCOMING, readable);
    }
}

fn signal_zx_event(event: &zx::EventPair, signal: zx::Signals, on: bool) {
    let (clear, set) = if on { (zx::Signals::NONE, signal) } else { (signal, zx::Signals::NONE) };
    event
        .signal_peer(clear, set)
        .unwrap_or_else(|e| error!("failed to signal socket event {signal:?} = {on:?}: {e:?}"));
}
