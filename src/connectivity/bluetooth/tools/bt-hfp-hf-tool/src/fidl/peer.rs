// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, format_err};
use async_utils::hanging_get::client::HangingGetStream;
use fidl_fuchsia_bluetooth_hfp as hfp;
use fuchsia_async::Task;
use fuchsia_bluetooth::types::PeerId;
use fuchsia_sync::Mutex;
use futures::stream::FuturesUnordered;
use futures::{StreamExt, select};
use std::collections::HashMap;
use std::fmt;
use std::ops::RangeFrom;
use std::sync::Arc;

use crate::fidl::call::{Call, LocalCallId};

pub type LocalPeerId = u64;

#[derive(Clone)]
pub struct Peer {
    pub local_id: LocalPeerId,
    canonical_id: PeerId,
    pub proxy: hfp::PeerHandlerProxy,
}

struct PeerHandlerProxyTask {
    local_id: LocalPeerId,
    canonical_id: PeerId,

    next_local_call_id: Arc<Mutex<RangeFrom<LocalCallId>>>,
    calls: Arc<Mutex<HashMap<LocalCallId, Call>>>,
    call_tasks: FuturesUnordered<Task<LocalCallId>>,

    proxy: hfp::PeerHandlerProxy,
}

impl fmt::Debug for Peer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "peer {}: [ peer id: {} ]", self.local_id, self.canonical_id)
    }
}

impl fmt::Debug for PeerHandlerProxyTask {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "peer {}: [ peer id: {} ]", self.local_id, self.canonical_id)
    }
}

impl Peer {
    pub fn new_peer_and_task(
        local_id: LocalPeerId,
        canonical_id: PeerId,
        next_local_call_id: Arc<Mutex<RangeFrom<LocalCallId>>>,
        calls: Arc<Mutex<HashMap<LocalCallId, Call>>>,
        proxy: hfp::PeerHandlerProxy,
    ) -> (Peer, Task<LocalPeerId>) {
        let call_tasks = FuturesUnordered::new();

        let peer_task = PeerHandlerProxyTask {
            local_id,
            canonical_id,
            next_local_call_id,
            calls,
            call_tasks,
            proxy: proxy.clone(),
        };
        let peer_fut = peer_task.run();
        let task = Task::local(peer_fut);

        let peer = Peer { local_id, canonical_id, proxy };
        (peer, task)
    }
}

impl PeerHandlerProxyTask {
    async fn run(mut self) -> LocalPeerId {
        let result = self.run_inner().await;
        if let Err(err) = result {
            println!("Error running peer task for peer {self:?}: {err:?}")
        }

        self.local_id
    }

    async fn run_inner(&mut self) -> Result<()> {
        let mut next_call_stream =
            HangingGetStream::new(self.proxy.clone(), hfp::PeerHandlerProxy::watch_next_call);

        loop {
            // If the collection is empty, `poll_next` may return `Ready(None)`.  However, we
            // don't want to exit in that case as it may have more calls in the future.
            let mut finished_call_fut = self.call_tasks.select_next_some();
            let mut next_call_fut = next_call_stream.next();

            select! {
                finished_call = finished_call_fut => {
                    self.handle_finished_call(finished_call);
                }
                next_call = next_call_fut => {
                    let next_call = match next_call {
                        Some(Ok(call)) => call,
                        Some(Err(fidl::Error::ClientChannelClosed { .. })) | None => return Ok(()),
                        Some(Err(err)) => Err(format_err!("FIDL error: {err}"))?,
                    };
                    self.handle_next_call(next_call)?;
                }
            }
        }
    }

    fn handle_finished_call(&mut self, local_call_id: LocalCallId) {
        let mut calls = self.calls.lock();
        if let Some(removed_call) = calls.remove(&local_call_id) {
            println!("Call {local_call_id} ended: {removed_call:?}")
        } else {
            println!("BUG: Unknown call {} removed.", local_call_id)
        }
    }

    fn handle_next_call(&mut self, next_call: hfp::NextCall) -> Result<()> {
        let next_call_debug = format!("{:?}", next_call);

        let client_end = next_call.call.ok_or_else(|| {
            format_err!("Missing Call client end on received call {}", next_call_debug)
        })?;
        let proxy = client_end.into_proxy();

        let number = next_call
            .remote
            .ok_or_else(|| format_err!("Missing number on received call {}", next_call_debug))?;
        let direction = next_call
            .direction
            .ok_or_else(|| format_err!("Missing direction on received call {}", next_call_debug))?;
        let state = next_call
            .state
            .ok_or_else(|| format_err!("Missing state on received call {}", next_call_debug))?;

        let local_id =
            self.next_local_call_id.lock().next().expect("Couldn't get next local call id.");

        let (call, task) =
            Call::new_call_with_task(local_id, number, direction, state, self.calls.clone(), proxy);
        println!("New call: {call:?}");

        let mut calls = self.calls.lock();
        let no_previous_call = calls.insert(local_id, call);
        // This should be impossible as we increment the ca;; id every time.
        assert!(no_previous_call.is_none(), "Reused local call ID.");

        self.call_tasks.push(task);

        Ok(())
    }
}
