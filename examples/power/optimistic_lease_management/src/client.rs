// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_component::client::connect_to_protocol;
use fuchsia_component::server::{ServiceFs, ServiceObj};
use futures::stream::StreamExt;
use lease_management::SequenceClient;
use log::{error, info, warn};
use std::sync::Arc;
use {fidl_fuchsia_example_power as fexample, fuchsia_async as fasync};

async fn receive_messages(
    msg_source: fexample::MessageSourceProxy,
    baton_receiver: Arc<SequenceClient>,
) {
    // Create a socket and ask the server to receive messages on it.
    let (rcv, tx) = zx::Socket::create_datagram();
    msg_source.receive_messages(tx).await.unwrap();
    let mut buffer: [u8; 256] = [0; 256];
    loop {
        let r = fasync::OnSignals::new(
            rcv.as_handle_ref(),
            zx::Signals::SOCKET_READABLE | zx::Signals::SOCKET_PEER_CLOSED,
        )
        .await;

        if let Err(e) = r {
            error!("Got error waiting for signals: {:?}", e);
            break;
        }

        // Maybe use SuspendBlocker to only do work while we're resumed to simulate actual
        // suspension.
        let signal = r.unwrap();
        if signal.contains(zx::Signals::SOCKET_READABLE) {
            // Socket is readable, so read should succeed
            let bytes_read = rcv.read(&mut buffer).unwrap();

            let _secret_message = std::string::String::from_utf8_lossy(&buffer[..bytes_read]);
            info!("received message");

            // simulate how long it takes to process the message
            fasync::Timer::new(zx::MonotonicDuration::from_millis(100)).await;

            // Notify the baton manager that we processed a message
            baton_receiver.process_message().await;
            continue;
        }

        if signal.contains(zx::Signals::SOCKET_PEER_CLOSED) {
            info!("socket closed");
            break;
        }
    }
}

enum ExposedCapabilities {
    CountCheck(fexample::CounterRequestStream),
}

#[fuchsia::main]
async fn main() {
    fuchsia_trace_provider::trace_provider_create_with_fdio();
    let mut svc_fs: ServiceFs<ServiceObj<'_, ExposedCapabilities>> = ServiceFs::new();
    svc_fs.add_fidl_service(ExposedCapabilities::CountCheck);
    svc_fs.take_and_serve_directory_handle().expect("failed to serve outgoing");

    let task_scope = fasync::Scope::new();

    // Connect the the message source
    let msg_src = connect_to_protocol::<fexample::MessageSourceMarker>().expect("connect failed!");
    let baton_receiver = Arc::new(SequenceClient::new(msg_src.clone()));

    let baton_receiver_clone = baton_receiver.clone();
    task_scope.spawn(async move {
        let _ = baton_receiver_clone.run().await;
    });

    // Run the count check protocol so the test cod can ask us how many
    // messages we've received.
    let count_check_clone = baton_receiver.clone();
    let async_scope_copy = task_scope.clone();
    task_scope.spawn(async move {
        while let Some(request) = svc_fs.next().await {
            match request {
                ExposedCapabilities::CountCheck(mut stream) => {
                    let connection_clone = count_check_clone.clone();
                    async_scope_copy.spawn(async move {
                        while let Some(counter_request) = stream.next().await {
                            match counter_request {
                                Ok(fexample::CounterRequest::Get { responder }) => {
                                    responder
                                        .send(connection_clone.get_receieved_count().await)
                                        .expect("Failed to send response to client");
                                }
                                _ => {
                                    warn!("error reading counter requests, exiting");
                                    break;
                                }
                            }
                        }
                    });
                }
            }
        }
    });

    task_scope.spawn(receive_messages(msg_src, baton_receiver));
    task_scope.join().await;
}
