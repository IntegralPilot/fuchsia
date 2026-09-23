// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fidl_test_components::{TriggerMarker, TriggerRequestStream};
use fuchsia_async as fasync;
use fuchsia_component::server::{ServiceFs, ServiceFsDir};
use futures::{StreamExt, TryStreamExt};
use std::pin::pin;

/// See the `stop_with_delivery_on_readable_request` test case.
#[fuchsia::main]
pub async fn main() {
    struct Trigger(TriggerRequestStream);

    // Handle exactly one connection request, which is what the test sends.
    let mut fs = ServiceFs::new();
    let _: &mut ServiceFsDir<'_, _> = fs.dir("svc").add_fidl_service(Trigger);
    let _: &mut ServiceFs<_> = fs.take_and_serve_directory_handle().unwrap();
    let request = fs.next().await.unwrap();
    handle_trigger(request.0).await;
}

async fn handle_trigger(stream: TriggerRequestStream) {
    let (stream, stalled) =
        detect_stall::until_stalled(stream, fasync::MonotonicDuration::from_micros(1));
    let mut stream = pin!(stream);
    while let Ok(Some(request)) = stream.try_next().await {
        match request {
            fidl_fidl_test_components::TriggerRequest::Run { responder } => {
                responder.send("hello").unwrap()
            }
        }
    }
    if let Ok(Some(server_end)) = stalled.await {
        // Send the server endpoint back to the framework.
        fuchsia_component::client::connect_channel_to_protocol_at::<TriggerMarker>(
            server_end.into(),
            "/escrow",
        )
        .unwrap();
    }
}
