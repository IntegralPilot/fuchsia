// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use component_events::events::EventStream;
use fidl_fidl_test_components as ftest;

use fidl::endpoints::ServerEnd;
use fidl_fuchsia_component::EventPayload;
use futures_util::{future, StreamExt};

/// This component sends CapabilityRequested events, and verifies
/// they get routed to the correct protocol.
#[fuchsia::main]
async fn main() {
    let mut event_stream =
        EventStream::open_at_path("/events/capability_requested_0").await.unwrap();
    let mut event_stream_2 =
        EventStream::open_at_path("/events/capability_requested_1").await.unwrap();
    let protocol0 = async move {
        let event = event_stream.next().await.unwrap();
        if let Some(EventPayload::CapabilityRequested(payload)) = event.payload {
            let mut trigger_server =
                ServerEnd::<ftest::TriggerMarker>::new(payload.capability.unwrap()).into_stream();
            let request = trigger_server.next().await.unwrap().unwrap();
            match request {
                ftest::TriggerRequest::Run { responder } => {
                    responder.send("0").unwrap();
                }
            }
        } else {
            panic!("Invalid request");
        }
    };

    let protocol1 = async move {
        let event = event_stream_2.next().await.unwrap();
        if let Some(EventPayload::CapabilityRequested(payload)) = event.payload {
            let mut trigger_server =
                ServerEnd::<ftest::TriggerMarker>::new(payload.capability.unwrap()).into_stream();
            let request = trigger_server.next().await.unwrap().unwrap();
            match request {
                ftest::TriggerRequest::Run { responder } => {
                    responder.send("1").unwrap();
                }
            }
        } else {
            panic!("Invalid request");
        }
    };

    future::join(protocol0, protocol1).await;
}
