// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_utils::stream::{StreamMap, Tagged, WithTag};
use fuchsia_bluetooth::types::{HostId, PeerId};
use futures::future::BoxFuture;
use futures::stream::{FusedStream, FuturesUnordered, Stream};
use futures::StreamExt;
use std::collections::HashMap;
use std::pin::Pin;
use std::task::{Context, Poll};

/// A Stream of outstanding Pairing Requests, indexed by Host. When polled, the Stream impl will
/// yield the next available completed request, when ready. `T` is the response type returned on
/// completion of a request.
pub struct PairingRequests<T> {
    inner: StreamMap<HostId, FuturesUnordered<Tagged<PeerId, BoxFuture<'static, T>>>>,
}

impl<T> PairingRequests<T> {
    /// Create a new empty PairingRequests<T>
    pub fn empty() -> PairingRequests<T> {
        PairingRequests { inner: StreamMap::empty() }
    }
    /// Insert a new pending request future, identified by HostId and PeerId
    pub fn insert(&mut self, host: HostId, peer: PeerId, request: BoxFuture<'static, T>) {
        self.inner
            .inner_mut()
            .entry(host)
            .or_insert_with(|| Box::pin(FuturesUnordered::new()))
            .push(request.tagged(peer))
    }
    /// Remove all pending requests for a given host, returning the PeerIds of those requests
    pub fn remove_host_requests(&mut self, host: HostId) -> Option<Vec<PeerId>> {
        self.inner.remove(&host).map(|mut futs| futs.iter_mut().map(|f| f.tag()).collect())
    }
    /// Remove all pending requests, returning the PeerIds of those requests
    pub fn take_all_requests(&mut self) -> HashMap<HostId, Vec<PeerId>> {
        self.inner
            .inner_mut()
            .drain()
            .map(|(host, mut futs)| (host, futs.iter_mut().map(|f| f.tag()).collect()))
            .collect()
    }
}

impl<T> FusedStream for PairingRequests<T> {
    // PairingRequests is never terminated - a new pending request be added at any time
    fn is_terminated(&self) -> bool {
        false
    }
}

impl<T> Stream for PairingRequests<T> {
    type Item = (PeerId, T);

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.poll_next_unpin(cx)
    }
}
