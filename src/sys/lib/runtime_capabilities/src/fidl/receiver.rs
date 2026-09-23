// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{DirReceiver, Receiver};
use fidl::endpoints::Proxy;
use fidl_fuchsia_component_sandbox as fsandbox;
use futures::future::{self, Either};
use std::pin::pin;

impl Receiver {
    pub(crate) async fn handle_receiver(self, receiver_proxy: fsandbox::ReceiverProxy) {
        let mut on_closed = pin!(receiver_proxy.on_closed());
        loop {
            match future::select(pin!(self.receive()), on_closed).await {
                Either::Left((channel, fut)) => {
                    on_closed = fut;
                    let Some(channel) = channel else {
                        return;
                    };
                    if let Err(_) = receiver_proxy.receive(channel) {
                        return;
                    }
                }
                Either::Right((_, _)) => {
                    return;
                }
            }
        }
    }
}

impl DirReceiver {
    pub(crate) async fn handle_receiver(self, receiver_proxy: fsandbox::DirReceiverProxy) {
        let mut on_closed = pin!(receiver_proxy.on_closed());
        loop {
            match future::select(pin!(self.receive()), on_closed).await {
                Either::Left((payload, fut)) => {
                    on_closed = fut;
                    let Some(payload) = payload else {
                        return;
                    };
                    if let Err(_) = receiver_proxy.receive(fsandbox::DirReceiverReceiveRequest {
                        channel: Some(payload.dir.into()),
                        flags: payload.flags,
                        subdir: Some(payload.subdir.to_string()),
                        ..Default::default()
                    }) {
                        return;
                    }
                }
                Either::Right((_, _)) => {
                    return;
                }
            }
        }
    }
}
