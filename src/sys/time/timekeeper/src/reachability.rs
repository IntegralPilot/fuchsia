// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{anyhow, Result};
use async_utils::hanging_get;
use fidl_fuchsia_net_reachability as ffnr;
use futures::channel::mpsc;
use futures::{SinkExt, StreamExt};
use time_adjust::Command;

/// Continuously polls the reachability FIDL API endpoint and forwards the HTTP
/// state.
pub struct Monitor {
    // Reachability state is propagated here.
    cmd: mpsc::Sender<Command>,
}

impl Monitor {
    /// Creates a new reachability monitor.
    ///
    /// Args:
    /// - `cmd`: The channel that reachability info is propagated to.
    pub fn new(cmd: mpsc::Sender<Command>) -> Self {
        Self { cmd }
    }

    /// Monitor the network reachability activity until an error occurs.
    ///
    /// Args:
    /// - `proxy`: The `fuchsia.net.reachability/Monitor` proxy to monitor.
    pub async fn serve(&mut self, proxy: ffnr::MonitorProxy) -> Result<()> {
        let mut stream =
            hanging_get::client::HangingGetStream::new(proxy, ffnr::MonitorProxy::watch);
        while let Some(maybe_request) = stream.next().await {
            match maybe_request {
                Ok(snapshot) => {
                    let http_available = snapshot.http_active.unwrap_or(false);
                    if let Err(e) = self.cmd.send(Command::Connectivity { http_available }).await {
                        return Err(anyhow!("could not send reachability info: {:?}", e));
                    }
                }
                Err(e) => {
                    return Err(anyhow!(
                        "could not process result from fuchsia.net.reachability/Monitor: {:?}",
                        e
                    ));
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fuchsia_async as fasync;

    #[fuchsia::test]
    async fn test_reachability() {
        let (send, mut rcv) = mpsc::channel(1);
        let (proxy, mut stream) = fidl::endpoints::create_proxy_and_stream::<ffnr::MonitorMarker>();

        let _server = fasync::Task::local(async move {
            while let Some(maybe_request) = stream.next().await {
                if let Ok(request) = maybe_request {
                    match request {
                        fidl_fuchsia_net_reachability::MonitorRequest::SetOptions { .. } => {
                            unreachable!()
                        }
                        fidl_fuchsia_net_reachability::MonitorRequest::Watch { responder } => {
                            responder
                                .send(&ffnr::Snapshot {
                                    http_active: Some(true),
                                    ..Default::default()
                                })
                                .unwrap();
                        }
                    }
                }
            }
        });
        let mut monitor = Monitor::new(send);
        let _task = fasync::Task::local(async move {
            monitor.serve(proxy).await.unwrap();
        });
        assert_matches!(rcv.next().await.unwrap(), Command::Connectivity { http_available: true });
    }
}
