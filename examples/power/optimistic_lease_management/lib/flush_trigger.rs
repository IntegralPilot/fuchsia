// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use futures::stream::StreamExt;
use log::warn;
use {fidl_fuchsia_power_system as fsag, fuchsia_trace as ftrace};

/// Uses |fuchsia.power.system/SuspendBlocker|s to determine when to
/// trigger a flush.
pub struct FlushTrigger {
    sag: fsag::ActivityGovernorProxy,
}

impl FlushTrigger {
    pub fn new(sag: fsag::ActivityGovernorProxy) -> Self {
        Self { sag }
    }

    /// Calls |flusher| when it receives
    /// |fuchsia.power.system/SuspendBlocker.BeforeSuspend| and
    /// replies to the request *AFTER* |flusher.flush| returns.
    pub async fn run<'a>(&self, flusher: &dyn FlushListener) -> Result<(), fidl::Error> {
        let (client, server) = fidl::endpoints::create_endpoints::<fsag::SuspendBlockerMarker>();

        let registration_lease = self
            .sag
            .register_suspend_blocker(fsag::ActivityGovernorRegisterSuspendBlockerRequest {
                suspend_blocker: Some(client),
                name: Some("flush_trigger".into()),
                ..Default::default()
            })
            .await?
            .expect("error registering suspend blocker");
        drop(registration_lease);

        let mut request_stream = server.into_stream();

        while let Some(req) = request_stream.next().await {
            match req {
                Ok(fsag::SuspendBlockerRequest::BeforeSuspend { responder }) => {
                    ftrace::duration!(crate::TRACE_CATEGORY, c"flush-triggered");
                    flusher.flush().await;
                    let _ = responder.send();
                }
                Ok(fsag::SuspendBlockerRequest::AfterResume { responder }) => {
                    let _ = responder.send();
                }
                Ok(fsag::SuspendBlockerRequest::_UnknownMethod { .. }) => {
                    warn!("unrecognized SuspendBlocker method, ignoring");
                }
                Err(_) => continue,
            }
        }
        Ok(())
    }
}

#[async_trait]
pub trait FlushListener {
    async fn flush(&self);
}
