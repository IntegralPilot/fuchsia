// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_starnix_psi::{PsiProviderRequest, PsiProviderRequestStream};
use fuchsia_component_test::LocalComponentHandles;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use futures::lock::Mutex;
use futures::{Future, FutureExt, SinkExt, StreamExt, select};
use std::pin::pin;
use std::sync::Arc;

pub struct FakePsiProvider {
    producer_state: Mutex<ProducerState>,
    receiver_state: Mutex<ReceiverState>,
}

struct ProducerState {
    expecting_request: bool,
    channel: UnboundedSender<PsiProviderRequest>,
}

struct ReceiverState {
    channel: UnboundedReceiver<PsiProviderRequest>,
}

impl FakePsiProvider {
    pub fn new() -> FakePsiProvider {
        let (sender, receiver) = futures::channel::mpsc::unbounded();

        FakePsiProvider {
            producer_state: Mutex::new(ProducerState { expecting_request: false, channel: sender }),
            receiver_state: Mutex::new(ReceiverState { channel: receiver }),
        }
    }

    /// Executes `body` while expecting it to make exactly one request over this fake
    /// `fuchsia.starnix.runner.PsiProvider`. The request will be forwarded to the provided
    /// `request_handler` callback, which has to send a reply, if applicable.
    ///
    /// Notes:
    ///  - Calls to this function cannot be nested.
    ///  - This function panics if the provided `body` completes without ever issuing
    ///    any request.
    ///  - This function also panics if `body` issues more than one request.
    pub async fn with_expected_request<T>(
        self: Arc<Self>,
        request_handler: impl FnOnce(PsiProviderRequest) + Send,
        body: impl Future<Output = T>,
    ) -> T {
        let mut receiver_state = self
            .receiver_state
            .try_lock()
            .expect("Cannot have multiple ongoing with_expected_request at the same time");

        // Signal serve() that we expect exactly 1 request.
        {
            let mut producer_state = self.producer_state.lock().await;
            assert!(!producer_state.expecting_request);
            producer_state.expecting_request = true;
        }

        // Execute the request handler (up to once) and the body in parallel.
        let mut body = pin!(body.fuse());
        let result = {
            select! {
                _ = body => panic!("Body returned without issuing the expected request"),
                request = receiver_state.channel.next() => {
                    request_handler(request.unwrap());
                    body.await
                }
            }
        };

        result
    }

    pub async fn serve(
        self: Arc<Self>,
        handles: LocalComponentHandles,
    ) -> Result<(), anyhow::Error> {
        let mut fs = fuchsia_component::server::ServiceFs::new();
        fs.dir("svc").add_fidl_service(|client: PsiProviderRequestStream| client);
        fs.serve_connection(handles.outgoing_dir).unwrap();

        while let Some(mut client) = fs.next().await {
            while let Some(request) = client.next().await {
                let mut producer_state = self.producer_state.lock().await;

                assert!(
                    producer_state.expecting_request,
                    "Received request outside of a with_expected_request block"
                );
                producer_state.expecting_request = false;

                producer_state.channel.send(request.unwrap()).await.unwrap();
            }
        }

        Ok(())
    }
}
