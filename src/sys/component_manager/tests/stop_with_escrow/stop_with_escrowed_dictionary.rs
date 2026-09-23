// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::{RequestStream, ServerEnd};
use fidl_fidl_test_components::{TriggerMarker, TriggerRequestStream};
use fuchsia_component::runtime;
use fuchsia_component::server::{ServiceFs, ServiceFsDir};
use fuchsia_runtime::{HandleInfo, HandleType};
use futures::{StreamExt, TryStreamExt};
use std::pin::pin;
use {fidl_fuchsia_process_lifecycle as flifecycle, fuchsia_async as fasync};

/// See the `stop_with_escrowed_dictionary` test case.
///
/// This program stores some state in the escrow request, to be read back the
/// next time it is started. In particular, it stores an increasing counter of
/// the number of `TriggerRequest.Run` calls.
#[fuchsia::main]
pub async fn main() {
    struct Trigger(TriggerRequestStream);

    // If there is no `EscrowedDictionary` processargs, initialize the counter to 0.
    let counter = match fuchsia_runtime::take_startup_handle(HandleInfo::new(
        HandleType::EscrowedDictionary,
        0,
    )) {
        Some(handle) => {
            log::warn!("found escrowed dictionary!");
            read_counter_from_dictionary(&zx::EventPair::from(handle).into()).await
        }
        None => {
            log::warn!("no escrowed dictionary, starting at 0");
            0
        }
    };

    // Handle exactly one connection request, which is what the test sends.
    let mut fs = ServiceFs::new();
    let _: &mut ServiceFsDir<'_, _> = fs.dir("svc").add_fidl_service(Trigger);
    let _: &mut ServiceFs<_> = fs.take_and_serve_directory_handle().unwrap();
    let request = fs.next().await.unwrap();
    let counter = handle_trigger(counter, request.0).await;
    escrow_counter_then_stop(counter).await;
}

async fn read_counter_from_dictionary(dictionary: &runtime::Dictionary) -> u64 {
    let data = match dictionary.get("counter").await {
        Some(runtime::Capability::Data(data)) => data,
        other_value => panic!("unexpected value in dictionary: {other_value:?}"),
    };
    match data.get_value().await {
        runtime::DataValue::Uint64(counter) => counter,
        other_value => panic!("unexpected type of data: {other_value:?}"),
    }
}

async fn handle_trigger(mut counter: u64, stream: TriggerRequestStream) -> u64 {
    let (stream, stalled) =
        detect_stall::until_stalled(stream, fasync::MonotonicDuration::from_micros(1));
    let mut stream = pin!(stream);
    while let Ok(Some(request)) = stream.try_next().await {
        match request {
            fidl_fidl_test_components::TriggerRequest::Run { responder } => {
                counter += 1;
                responder.send(&format!("{counter}")).unwrap();
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
    counter
}

async fn escrow_counter_then_stop(counter: u64) {
    // Create a new dictionary.
    let dictionary = runtime::Dictionary::new().await;

    // Add the counter into the dictionary.
    let data = runtime::Data::new(runtime::DataValue::Uint64(counter)).await;
    dictionary.insert("counter", data).await;

    // Send the dictionary away.
    let lifecycle =
        fuchsia_runtime::take_startup_handle(HandleInfo::new(HandleType::Lifecycle, 0)).unwrap();
    let lifecycle = zx::Channel::from(lifecycle);
    let lifecycle = ServerEnd::<flifecycle::LifecycleMarker>::from(lifecycle);
    lifecycle
        .into_stream()
        .control_handle()
        .send_on_escrow(flifecycle::LifecycleOnEscrowRequest {
            escrowed_dictionary_handle: Some(dictionary.handle),
            ..Default::default()
        })
        .unwrap();
}
