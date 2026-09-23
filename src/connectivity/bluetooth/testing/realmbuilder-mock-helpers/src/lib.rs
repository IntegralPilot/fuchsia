// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl::endpoints::{DiscoverableProtocolMarker, ProtocolMarker, Proxy};
use fidl_fuchsia_device::{NameProviderMarker, NameProviderRequestStream};
use fidl_fuchsia_stash::SecureStoreMarker;
use fuchsia_async as fasync;
use fuchsia_component::server::{ServiceFs, ServiceObj};
use fuchsia_component_test::LocalComponentHandles;
use futures::channel::mpsc;
use futures::{SinkExt, StreamExt, TryStream, TryStreamExt};
use log::info;
use std::sync::Arc;
use vfs::directory::entry_container::Directory;

// #! Library for common utilities (mocks, definitions) for the manifest integration tests.

/// Process requests received in the `stream` and relay them to the provided `sender`.
/// Logs incoming requests prefixed with the `tag`.
pub async fn process_request_stream<S, Event>(
    mut stream: S::RequestStream,
    mut sender: mpsc::Sender<Event>,
) where
    S: DiscoverableProtocolMarker,
    Event: std::convert::From<<S::RequestStream as TryStream>::Ok>,
    <S::RequestStream as TryStream>::Ok: std::fmt::Debug,
{
    while let Some(request) = stream.try_next().await.expect("serving request stream failed") {
        info!("Received {} service request: {:?}", S::PROTOCOL_NAME, request);
        sender.send(request.into()).await.expect("should send");
    }
}

/// Adds a handler for the FIDL service `S` which relays the ServerEnd of the service
/// connection request to the provided `sender`.
/// Note: This method does not process requests from the service connection. It only relays
/// the stream to the `sender.
pub fn add_fidl_service_handler<S, Event: 'static>(
    fs: &mut ServiceFs<ServiceObj<'_, ()>>,
    sender: mpsc::Sender<Event>,
) where
    S: DiscoverableProtocolMarker,
    Event: std::convert::From<S::RequestStream> + std::marker::Send,
{
    let _ = fs.dir("svc").add_fidl_service(move |req_stream: S::RequestStream| {
        let mut s = sender.clone();
        fasync::Task::local(async move {
            info!("Received connection for {}", S::PROTOCOL_NAME);
            s.send(req_stream.into()).await.expect("should send");
        })
        .detach()
    });
}

/// A mock component that provides the generic service `S`. The request stream
/// of the service is processed and any requests relayed to the provided `sender`.
pub async fn mock_component<S, Event: 'static>(
    sender: mpsc::Sender<Event>,
    handles: LocalComponentHandles,
) -> Result<(), Error>
where
    S: DiscoverableProtocolMarker,
    Event: std::convert::From<<<S as ProtocolMarker>::RequestStream as TryStream>::Ok>
        + std::marker::Send,
    <<S as ProtocolMarker>::RequestStream as TryStream>::Ok: std::fmt::Debug,
{
    let mut fs = ServiceFs::new();
    let _ = fs.dir("svc").add_fidl_service(move |req_stream: S::RequestStream| {
        let sender_clone = sender.clone();
        info!("Received connection for {}", S::PROTOCOL_NAME);
        fasync::Task::local(process_request_stream::<S, _>(req_stream, sender_clone)).detach();
    });

    let _ = fs.serve_connection(handles.outgoing_dir)?;
    fs.collect::<()>().await;
    Ok(())
}

/// Sets up a mock dev/ directory with the provided `dev_directory` topology.
pub async fn mock_dev(
    handles: LocalComponentHandles,
    dev_directory: Arc<dyn Directory>,
) -> Result<(), Error> {
    let mut fs = ServiceFs::new();
    let _ = fs.add_remote(
        "dev",
        vfs::directory::serve_read_only(dev_directory, vfs::execution_scope::ExecutionScope::new()),
    );
    let _ = fs.serve_connection(handles.outgoing_dir)?;
    fs.collect::<()>().await;
    Ok(())
}

/// Sets up a mock svc/ directory with the provided `svc_directory` topology.
pub async fn mock_svc(
    handles: LocalComponentHandles,
    svc_directory: Arc<dyn Directory>,
) -> Result<(), Error> {
    let mut fs = ServiceFs::new();
    let _ = fs.add_remote(
        "svc",
        vfs::directory::serve_read_only(svc_directory, vfs::execution_scope::ExecutionScope::new()),
    );
    let _ = fs.serve_connection(handles.outgoing_dir)?;
    fs.collect::<()>().await;
    Ok(())
}

/// A mock component serving a protocol `S` on `handles`. Specifically, this services S by calling
/// `responder` for every request of every client connection to S.
pub async fn stateless_mock_responder<S, F>(
    handles: LocalComponentHandles,
    responder: F,
) -> Result<(), anyhow::Error>
where
    S: DiscoverableProtocolMarker,
    <<S as ProtocolMarker>::RequestStream as TryStream>::Ok: std::fmt::Debug,
    F: Fn(<<S as ProtocolMarker>::RequestStream as TryStream>::Ok) -> Result<(), Error>
        + Copy
        + Send
        + 'static,
{
    let mut fs = ServiceFs::new();
    // The FIDL service's task is generated for every client connection, hence the `F: Copy` bound
    // in order to use `responder` inside the task. F's bound could be changed to `Clone` in the
    // future, but we chose not to do so for now to avoid unexpected implicit `clone`s.
    let _ = fs.dir("svc").add_fidl_service(
        move |mut req_stream: <S as ProtocolMarker>::RequestStream| {
            fasync::Task::local(async move {
                let failure_msg = format!("serving {} request stream failed", S::DEBUG_NAME);
                while let Some(req) = req_stream.try_next().await.expect(&failure_msg) {
                    let failed_to_respond = format!("failed to respond to req {:?}", req);
                    responder(req).expect(&failed_to_respond);
                }
            })
            .detach()
        },
    );
    let _ = fs.serve_connection(handles.outgoing_dir)?;
    fs.collect::<()>().await;
    Ok(())
}

/// Exposes implementations of the the services used by bt-gap in the provided `ServiceFs`.
pub fn provide_bt_gap_uses<Event>(
    fs: &mut ServiceFs<ServiceObj<'_, ()>>,
    sender: &mpsc::Sender<Event>,
    handles: &LocalComponentHandles,
) -> Result<(), Error>
where
    Event: From<SecureStoreMarker> + From<NameProviderRequestStream> + Send + 'static,
{
    let svc_dir = handles.clone_from_namespace("svc")?;
    let sender_clone = Some(sender.clone());
    let _ = fs.dir("svc").add_service_at(SecureStoreMarker::PROTOCOL_NAME, move |chan| {
        let mut s = sender_clone.clone();
        let svc_dir = Clone::clone(&svc_dir);
        fasync::Task::local(async move {
            info!(
                "Proxying {} connection to real implementation",
                SecureStoreMarker::PROTOCOL_NAME
            );
            fdio::service_connect_at(
                svc_dir.as_channel().as_ref(),
                SecureStoreMarker::PROTOCOL_NAME,
                chan,
            )
            .expect("unable to forward secure store");
            // We only care that the Secure Store is routed correctly, so if a client connects
            // to it more than once, we only want to report it the first time.
            if let Some(mut sender) = s.take() {
                sender.send(Event::from(SecureStoreMarker)).await.expect("should send");
            }
        })
        .detach();
        None
    });
    add_fidl_service_handler::<NameProviderMarker, _>(fs, sender.clone());
    Ok(())
}
