// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests that connect to the service provided by the node.

use super::{endpoint, host};

use crate::directory::serve;
use crate::execution_scope::ExecutionScope;
use crate::pseudo_directory;

use assert_matches::assert_matches;
use fidl::Error;
#[cfg(not(feature = "fdomain"))]
use fidl_test_placeholders::{EchoMarker, EchoProxy, EchoRequest, EchoRequestStream};
use flex_client::fidl::RequestStream;
use flex_fuchsia_io as fio;
#[cfg(feature = "fdomain")]
use flex_test_placeholders::{EchoMarker, EchoProxy, EchoRequest, EchoRequestStream};
use fuchsia_sync::Mutex;
use futures::channel::{mpsc, oneshot};
use futures::stream::StreamExt;
use zx_status::Status;

fn connect_at(
    root: &fio::DirectoryProxy,
    path: &str,
    #[cfg(feature = "fdomain")] client: &std::sync::Arc<flex_client::Client>,
) -> EchoProxy {
    #[cfg(feature = "fdomain")]
    let (proxy, server_end) = client.create_proxy::<EchoMarker>();
    #[cfg(not(feature = "fdomain"))]
    let (proxy, server_end) = fidl::endpoints::create_proxy::<EchoMarker>();
    root.open(
        path,
        fio::Flags::PROTOCOL_SERVICE,
        &fio::Options::default(),
        server_end.into_channel(),
    )
    .unwrap();
    proxy
}

async fn echo_server(
    mut requests: EchoRequestStream,
    on_message: Option<mpsc::UnboundedSender<Option<String>>>,
    done: Option<oneshot::Sender<()>>,
) {
    loop {
        match requests.next().await {
            None => break,
            Some(Err(err)) => {
                panic!("echo_server(): failed to receive a message: {}", err);
            }
            Some(Ok(EchoRequest::EchoString { value, responder })) => {
                responder
                    .send(value.as_ref().map(|s| &**s))
                    .expect("echo_server(): responder.send() failed");
                if let Some(on_message) = &on_message {
                    on_message.unbounded_send(value).expect("on_message.unbounded_send() failed");
                }
            }
        }
    }
    if let Some(done) = done {
        done.send(()).expect("done.send() failed");
    }
}

#[fuchsia::test]
async fn simple_endpoint() {
    let dir = pseudo_directory! {
        "server" => endpoint(|scope, channel| {
            scope.spawn(async move {
                echo_server(RequestStream::from_channel(channel), None, None).await;
            });
        }),
    };
    #[cfg(feature = "fdomain")]
    let client = flex_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let scope = ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = ExecutionScope::new();

    let root = serve(dir, scope.clone(), fio::PERM_READABLE);

    #[cfg(feature = "fdomain")]
    let proxy = connect_at(&root, "server", &client);
    #[cfg(not(feature = "fdomain"))]
    let proxy = connect_at(&root, "server");
    let response = proxy.echo_string(Some("test")).await.unwrap();
    assert_eq!(response.as_deref(), Some("test"));
    #[cfg(feature = "fdomain")]
    drop(client);
}

#[fuchsia::test]
async fn simple_host() {
    let dir = pseudo_directory! {
        "server" => host(|requests| echo_server(requests, None, None)),
    };
    #[cfg(feature = "fdomain")]
    let client = flex_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let scope = ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = ExecutionScope::new();

    let root = serve(dir, scope.clone(), fio::PERM_READABLE);

    #[cfg(feature = "fdomain")]
    let proxy = connect_at(&root, "server", &client);
    #[cfg(not(feature = "fdomain"))]
    let proxy = connect_at(&root, "server");
    let response = proxy.echo_string(Some("test")).await.unwrap();
    assert_eq!(response.as_deref(), Some("test"));
}

#[fuchsia::test]
async fn server_state_checking() {
    let (done_tx, done_rx) = oneshot::channel();
    let (on_message_tx, mut on_message_rx) = mpsc::unbounded();

    let done_tx = Mutex::new(Some(done_tx));
    let on_message_tx = Mutex::new(Some(on_message_tx));

    let dir = pseudo_directory! {
        "server" => host(move |requests| {
            echo_server(
                requests,
                Some(on_message_tx.lock().take().unwrap()),
                Some(done_tx.lock().take().unwrap()),
            )
        }),
    };
    #[cfg(feature = "fdomain")]
    let client = flex_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let scope = ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = ExecutionScope::new();

    let root = serve(dir, scope.clone(), fio::PERM_READABLE);

    #[cfg(feature = "fdomain")]
    let proxy = connect_at(&root, "server", &client);
    #[cfg(not(feature = "fdomain"))]
    let proxy = connect_at(&root, "server");

    let response = proxy.echo_string(Some("message 1")).await.unwrap();

    // `next()` wraps in `Option` and our value is `Option<String>`, hence double `Option`.
    assert_eq!(on_message_rx.next().await, Some(Some("message 1".to_string())));
    assert_eq!(response.as_deref(), Some("message 1"));

    let response = proxy.echo_string(Some("message 2")).await.unwrap();

    // `next()` wraps in `Option` and our value is `Option<String>`, hence double `Option`.
    assert_eq!(on_message_rx.next().await, Some(Some("message 2".to_string())));
    assert_eq!(response.as_deref(), Some("message 2"));

    drop(proxy);

    assert_eq!(done_rx.await, Ok(()));
}

#[fuchsia::test]
async fn test_epitaph() {
    let dir = pseudo_directory! {
        "server" => host(|requests| echo_server(requests, None, None)),
    };
    #[cfg(feature = "fdomain")]
    let client = flex_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    let scope = ExecutionScope::new(client.clone());
    #[cfg(not(feature = "fdomain"))]
    let scope = ExecutionScope::new();

    let root = serve(dir, scope.clone(), fio::PERM_READABLE);
    #[cfg(feature = "fdomain")]
    let (proxy, server_end) = client.create_proxy::<EchoMarker>();
    #[cfg(not(feature = "fdomain"))]
    let (proxy, server_end) = fidl::endpoints::create_proxy::<EchoMarker>();
    root.open(
        "server",
        fio::Flags::PROTOCOL_DIRECTORY,
        &fio::Options::default(),
        server_end.into_channel(),
    )
    .unwrap();
    let mut event_stream = proxy.take_event_stream();
    assert_matches!(
        event_stream.next().await,
        Some(Err(Error::ClientChannelClosed { epitaph, .. }))
            if epitaph == Status::NOT_DIR
    );

    assert_matches!(event_stream.next().await, None);
}
