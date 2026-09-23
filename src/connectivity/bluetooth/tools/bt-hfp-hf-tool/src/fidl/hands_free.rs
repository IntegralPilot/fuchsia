// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, format_err};
use async_utils::hanging_get::client::HangingGetStream;
use fidl::endpoints::ClientEnd;
use fuchsia_async::Task;
use fuchsia_bluetooth::types::PeerId;
use fuchsia_sync::Mutex;
use futures::stream::FuturesUnordered;
use futures::{FutureExt, StreamExt, select};
use std::collections::HashMap;
use std::future::Future;
use std::ops::RangeFrom;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use {fidl_fuchsia_bluetooth as bt, fidl_fuchsia_bluetooth_hfp as hfp};

use crate::fidl::call::{Call, LocalCallId};
use crate::fidl::peer::{LocalPeerId, Peer};

pub struct HandsFree {
    pub peers: Arc<Mutex<HashMap<LocalPeerId, Peer>>>,
    pub calls: Arc<Mutex<HashMap<LocalCallId, Call>>>,

    _proxy: hfp::HandsFreeProxy,
    task: Task<Result<()>>,
}

struct HandsFreeProxyTask {
    next_local_peer_id: RangeFrom<LocalPeerId>,
    next_local_call_id: Arc<Mutex<RangeFrom<LocalCallId>>>,

    peers: Arc<Mutex<HashMap<LocalPeerId, Peer>>>,
    peer_tasks: FuturesUnordered<Task<LocalPeerId>>,

    calls: Arc<Mutex<HashMap<LocalCallId, Call>>>,

    proxy: hfp::HandsFreeProxy,
}

impl Future for HandsFree {
    type Output = Result<()>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        self.task.poll_unpin(context)
    }
}

impl HandsFree {
    pub fn new(proxy: hfp::HandsFreeProxy) -> HandsFree {
        let peers = Arc::new(Mutex::new(HashMap::new()));
        let calls = Arc::new(Mutex::new(HashMap::new()));

        let peer_tasks = FuturesUnordered::new();

        let next_local_peer_id = RangeFrom { start: 1 };
        let next_local_call_id = Arc::new(Mutex::new(RangeFrom { start: 1 }));

        let call_task = HandsFreeProxyTask {
            next_local_peer_id,
            next_local_call_id,
            peers: peers.clone(),
            peer_tasks,
            calls: calls.clone(),
            proxy: proxy.clone(),
        };
        let hands_free_fut = call_task.run();
        let task = Task::local(hands_free_fut);

        Self { peers, calls, _proxy: proxy, task }
    }
}

impl HandsFreeProxyTask {
    pub async fn run(mut self) -> Result<()> {
        let mut new_peer_stream =
            HangingGetStream::new(self.proxy.clone(), hfp::HandsFreeProxy::watch_peer_connected);

        loop {
            let mut finished_peer_fut = self.peer_tasks.next();
            let mut new_peer_fut = new_peer_stream.next();

            select! {
                finished_peer_option = finished_peer_fut => {
                    if let Some(finished_peer) = finished_peer_option {
                       self.handle_finished_peer(finished_peer);
                    }
                    // Otherwise the collection is empty, but it may have more peers in the future.
                }
                new_peer = new_peer_fut => {
                    let (new_peer_id, new_peer_client_end) =
                        new_peer
                            .ok_or_else(|| format_err!("HandsFree stream closed."))?
                            .map_err(|e| format_err!("FIDL error: {e}"))?
                            .map_err(|e| format_err!("HandsFree error: {e}"))?;
                    self.handle_new_peer(new_peer_id, new_peer_client_end)?;
                }
            }
        }
    }

    fn handle_finished_peer(&mut self, local_peer_id: LocalPeerId) {
        let mut peers = self.peers.lock();
        if let Some(removed_peer) = peers.remove(&local_peer_id) {
            println!("Peer {local_peer_id} disconnected: {removed_peer:?}")
        } else {
            println!("BUG: Unknown peer {local_peer_id} removed.")
        }
    }

    fn handle_new_peer(
        &mut self,
        canonical_id: bt::PeerId,
        client_end: ClientEnd<hfp::PeerHandlerMarker>,
    ) -> Result<()> {
        let local_id = self.next_local_peer_id.next().expect("Couldn't get next local peer id.");

        let next_local_call_id = self.next_local_call_id.clone();
        let canonical_id: PeerId = canonical_id.into();
        let proxy: hfp::PeerHandlerProxy = client_end.into_proxy();

        let (peer, task) = Peer::new_peer_and_task(
            local_id,
            canonical_id,
            next_local_call_id,
            self.calls.clone(),
            proxy,
        );
        println!("New peer connected: {peer:?}");

        let no_previous_peer = self.peers.lock().insert(local_id, peer);
        // This should be impossible as we increment the peer id every time.
        assert!(no_previous_peer.is_none(), "Reused local peer ID.");

        self.peer_tasks.push(task);

        Ok(())
    }
}
