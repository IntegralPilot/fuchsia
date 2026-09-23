// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, format_err};
use async_utils::hanging_get::client::HangingGetStream;
use fuchsia_async::Task;
use fuchsia_sync::Mutex;
use futures::StreamExt;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use {fidl_fuchsia_bluetooth_hfp as hfp, fuchsia_async as fasync};

pub type LocalCallId = u64;

#[derive(Clone)]
pub struct Call {
    pub local_id: LocalCallId,

    number: String,
    direction: hfp::CallDirection,
    state: hfp::CallState,

    #[allow(unused)]
    pub proxy: hfp::CallProxy,
}

struct CallProxyTask {
    local_id: LocalCallId,

    calls: Arc<Mutex<HashMap<LocalCallId, Call>>>,
    proxy: hfp::CallProxy,
}

impl fmt::Debug for Call {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "call {}: [ number: {}, direction: {:?}, state: {:?} ]",
            self.local_id, self.number, self.direction, self.state,
        )
    }
}

impl Call {
    pub fn new_call_with_task(
        local_id: LocalCallId,
        number: String,
        direction: hfp::CallDirection,
        state: hfp::CallState,
        calls: Arc<Mutex<HashMap<LocalCallId, Call>>>,
        proxy: hfp::CallProxy,
    ) -> (Call, Task<LocalCallId>) {
        let call_task = CallProxyTask { local_id, calls, proxy: proxy.clone() };
        let call_fut = call_task.run();
        let task = fasync::Task::local(call_fut);

        let call = Call { local_id, number, direction, state, proxy };
        (call, task)
    }
}

impl CallProxyTask {
    async fn run(mut self) -> LocalCallId {
        let result = self.run_inner().await;
        if let Err(err) = result {
            println!("Error running Peer task for call {}: {err:?}", self.local_id)
        }

        self.local_id
    }

    async fn run_inner(&mut self) -> Result<()> {
        let mut call_state_stream =
            HangingGetStream::new(self.proxy.clone(), hfp::CallProxy::watch_state);
        loop {
            let new_call_state = call_state_stream.next().await;
            let new_call_state = match new_call_state {
                Some(Ok(state)) => state,
                Some(Err(fidl::Error::ClientChannelClosed { .. })) | None => return Ok(()),
                Some(Err(err)) => Err(format_err!("FIDL error: {err}"))?,
            };

            self.handle_new_call_state(new_call_state);
        }
    }

    fn handle_new_call_state(&mut self, new_state: hfp::CallState) {
        let mut calls = self.calls.lock();
        let call = calls.get_mut(&self.local_id);

        match call {
            None => {
                println!("BUG: got state {new_state:?} for nonexistent call {}", self.local_id)
            }
            Some(call) => {
                println!(
                    "Got state update for call {}: {:?} -> {:?})",
                    call.local_id, call.state, new_state
                );
                call.state = new_state;
            }
        }
    }
}
