// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use detect_stall;
use fidl::endpoints;
use fidl_fidl_test_components::{TriggerMarker, TriggerRequestStream};
use fidl_fuchsia_component_runtime as fruntime;
use fidl_fuchsia_process_lifecycle as flifecycle;
use fuchsia_async as fasync;
use fuchsia_component::runtime;
use fuchsia_component::server::{Item, ServiceFs};
use fuchsia_runtime::{HandleInfo, HandleType};
use futures::{FutureExt, StreamExt, TryStreamExt, future, select};
use log::*;
use std::future::Future;
use std::pin::pin;

/// Duration to wait for FIDL requests before stalling the component. This has to be long enough to
/// avoid test flakes resulting from the component starting and stopping more times than expected.
/// Otherwise, if this is too short, the component may start and stop more than once e.g. if
/// component manager is delayed delivering a request to its Receiver.
const STALL_INTERVAL: fasync::MonotonicDuration = fasync::MonotonicDuration::from_millis(100);

/// Protocol used by clients to connect. In production, this would likely be
/// generated at runtime.
const DYNAMIC_TRIGGER_PROTOCOL: &str = "fidl.test.components.Trigger-dynamic";

/// Escrowable protocol used by Component Manager to connect to this component
/// again after the client channel to [`DYNAMIC_TRIGGER_PROTOCOL`] idles.
const STATIC_TRIGGER_PROTOCOL: &str = "fidl.test.components.Trigger-static";

/// Escrowable protocol used by Component Manager to connect the server-side
/// channel to [`DYNAMIC_TRIGGER_PROTOCOL`] to this component.
const STATIC_RECEIVER_PROTOCOL: &str = "fuchsia.component.runtime.ConnectorReceiver-static";

enum IncomingRequest {
    Router(fruntime::DictionaryRouterRequestStream),
    Receiver(fruntime::ReceiverRequestStream),
    Static(TriggerRequestStream),
}

/// Load a previous escrowed dictionary if available, otherwise generate a
/// dictionary. This dictionary contains the entries necessary for a dynamic
/// dictionary to map [`DYNAMIC_TRIGGER_PROTOCOL`] to
/// [`STATIC_TRIGGER_PROTOCOL`].
pub async fn get_escrowed_dict() -> (runtime::Dictionary, Option<impl Future<Output = ()>>) {
    if let Some(handle) =
        fuchsia_runtime::take_startup_handle(HandleInfo::new(HandleType::EscrowedDictionary, 0))
    {
        info!("Reusing escrowed dictionary");
        return (zx::EventPair::from(handle).into(), None);
    }

    info!("No escrowed dictionary available; generating one");
    let dictionary = runtime::Dictionary::new().await;
    let (connector, receiver) = runtime::Connector::new().await;
    dictionary.insert(DYNAMIC_TRIGGER_PROTOCOL, connector).await;
    let receiver_task = handle_receiver(receiver.stream);
    (dictionary, Some(receiver_task))
}

/// See the `stop_with_dynamic_dictionary` test case.
#[fuchsia::main]
pub async fn main() {
    info!("Started");

    let (dictionary, receiver_task) = get_escrowed_dict().await;

    // Serve FIDL services for all static protocols.
    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(IncomingRequest::Router);
    fs.dir("svc").add_fidl_service_at(STATIC_TRIGGER_PROTOCOL, |rs| IncomingRequest::Static(rs));
    fs.dir("svc").add_fidl_service_at(STATIC_RECEIVER_PROTOCOL, |rs| IncomingRequest::Receiver(rs));

    fs.take_and_serve_directory_handle().unwrap();

    // Ignore stop requests in favor of relying on `until_stalled` timeouts.
    //
    // TODO(https://fxbug.dev/333080598): This is quite some boilerplate to escrow the outgoing dir.
    // Design some library function to handle the lifecycle requests.
    let lifecycle =
        fuchsia_runtime::take_startup_handle(HandleInfo::new(HandleType::Lifecycle, 0)).unwrap();
    let lifecycle: zx::Channel = lifecycle.into();
    let lifecycle: endpoints::ServerEnd<flifecycle::LifecycleMarker> = lifecycle.into();
    let (mut lifecycle_request_stream, lifecycle_control_handle) =
        lifecycle.into_stream_and_control_handle();
    let mut lifecycle_task = async move {
        let Some(Ok(request)) = lifecycle_request_stream.next().await else {
            return std::future::pending::<()>().await;
        };
        match request {
            flifecycle::LifecycleRequest::Stop { .. } => {
                // TODO(https://fxbug.dev/332341289): Teach `ServiceFs` and
                // others to skip the `until_stalled` timeout when this happens
                // so we can cleanly stop the component.
                return;
            }
        }
    }
    .boxed_local()
    .fuse();

    let outgoing_dir_task =
        fs.until_stalled(STALL_INTERVAL).for_each_concurrent(None, move |item| {
            let lifecycle_control_handle = lifecycle_control_handle.clone();
            let dictionary = dictionary.clone();
            async move {
                match item {
                    Item::Request(services, _active_guard) => match services {
                        IncomingRequest::Router(stream) => handle_router(stream, dictionary).await,
                        IncomingRequest::Receiver(stream) => handle_receiver(stream).await,
                        IncomingRequest::Static(stream) => handle_trigger(stream).await,
                    },
                    Item::Stalled(outgoing_directory) => {
                        lifecycle_control_handle
                            .send_on_escrow(flifecycle::LifecycleOnEscrowRequest {
                                outgoing_dir: Some(outgoing_directory.into()),
                                escrowed_dictionary_handle: Some(dictionary.clone().handle),
                                ..Default::default()
                            })
                            .unwrap();
                    }
                }
            }
        });

    let mut server_tasks = match receiver_task {
        Some(task) => {
            future::join(outgoing_dir_task.fuse(), task.fuse()).map(|_| ()).boxed_local().fuse()
        }
        None => outgoing_dir_task.boxed_local().fuse(),
    };

    select! {
        _ = lifecycle_task => info!("Stopping due to lifecycle request"),
        _ = server_tasks => info!("Stopping due to idle activity"),
    }
}

/// Handle fuchsia.component.runtime.DictionaryRouter requests with support for
/// escrowing in case the client doesn't send a request right away.
async fn handle_router(
    stream: fruntime::DictionaryRouterRequestStream,
    dictionary: runtime::Dictionary,
) {
    let (stream, stalled) = detect_stall::until_stalled(stream, STALL_INTERVAL);
    let mut stream = pin!(stream);
    while let Ok(Some(request)) = stream.try_next().await {
        info!("Received fuchsia.component.runtime.DictionaryRouterRequest");
        match request {
            fruntime::DictionaryRouterRequest::Route { handle, responder, .. } => {
                dictionary.associate_with_handle(handle).await;
                if let Err(e) = responder.send(Ok(fruntime::RouterResponse::Success)) {
                    warn!("Failed to send RouteResponse {e:?}")
                }
            }
            fruntime::DictionaryRouterRequest::_UnknownMethod { ordinal, .. } => {
                warn!(ordinal:%; "Unknown DictionaryRouter request");
            }
        }
    }
    _ = stalled.await;
    // We don't need to escrow the Router protocol, we only escrow the dictionary.
}

/// Handle fuchsia.component.runtime.Receiver requests with support for
/// escrowing in case the client doesn't send a request right away.
async fn handle_receiver(stream: fruntime::ReceiverRequestStream) {
    let (stream, stalled) = detect_stall::until_stalled(stream, STALL_INTERVAL);
    let mut stream = pin!(stream);
    while let Ok(Some(request)) = stream.try_next().await {
        info!("Received fuchsia.component.runtime.ReceiverRequest");
        match request {
            fruntime::ReceiverRequest::Receive { channel, control_handle: _ } => {
                let server_end = endpoints::ServerEnd::<TriggerMarker>::new(channel);
                handle_trigger(server_end.into_stream()).await;
            }
        }
    }
    if let Ok(Some(server_end)) = stalled.await {
        // Send the server endpoint back to the framework.
        info!("Escrowing {STATIC_RECEIVER_PROTOCOL}");
        fuchsia_component::client::connect_channel_to_protocol_at_path(
            server_end.into(),
            &format!("/escrow/{STATIC_RECEIVER_PROTOCOL}"),
        )
        .unwrap();
    }
}

/// Handle fidl.test.components.Trigger requests with support for escrowing in
/// case the client doesn't send a request right away.
async fn handle_trigger(stream: TriggerRequestStream) {
    let (stream, stalled) = detect_stall::until_stalled(stream, STALL_INTERVAL);
    let mut stream = pin!(stream);
    while let Ok(Some(request)) = stream.try_next().await {
        info!("Received fidl.test.components.TriggerRequest");
        match request {
            fidl_fidl_test_components::TriggerRequest::Run { responder } => {
                responder.send(&format!("hello from {STATIC_TRIGGER_PROTOCOL}")).unwrap()
            }
        }
    }
    if let Ok(Some(server_end)) = stalled.await {
        // Send the server endpoint back to the framework.
        info!("Escrowing {STATIC_TRIGGER_PROTOCOL}");
        fuchsia_component::client::connect_channel_to_protocol_at_path(
            server_end.into(),
            &format!("/escrow/{STATIC_TRIGGER_PROTOCOL}"),
        )
        .unwrap();
    }
}
