// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fuchsia_diagnostics_system as ftarget;
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures::channel::oneshot;
use log::warn;

#[derive(Clone)]
pub struct LogFreezeServer {
    freezer: UnboundedSender<oneshot::Sender<zx::EventPair>>,
}

impl LogFreezeServer {
    pub fn new() -> (Self, UnboundedReceiver<oneshot::Sender<zx::EventPair>>) {
        let (freezer, rx) = unbounded();
        (Self { freezer }, rx)
    }

    /// Actually handle the FIDL request. This handles only a single request, then exits.
    pub async fn handle_requests(
        &self,
        mut stream: ftarget::SerialLogControlRequestStream,
    ) -> Result<(), Error> {
        while let Some(request) = stream.next().await {
            match request? {
                fidl_fuchsia_diagnostics_system::SerialLogControlRequest::FreezeSerialForwarding { responder } => {
                    let (tx, rx) = oneshot::channel();
                    self.freezer.unbounded_send(tx)?;
                    // Ignore errors.
                    let _ = responder.send(rx.await?);
                },
                ftarget::SerialLogControlRequest::_UnknownMethod {
                                            ordinal,
                                            method_type,
                                            control_handle,
                                            ..
                                        } => {
                                            warn!(ordinal, method_type:?; "Unknown request. Closing connection");
                                            control_handle.shutdown_with_epitaph(zx::Status::UNAVAILABLE);
                                        }
            }
        }
        Ok(())
    }
}
