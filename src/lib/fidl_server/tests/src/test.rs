// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{anyhow, Error, Result};
use async_trait::async_trait;
use fidl::endpoints::{create_endpoints, create_proxy_and_stream};
use fidl_fuchsia_examples::{EchoMarker, EchoRequest};
use fidl_server::*;
use fuchsia_async as fasync;
use futures::future::FutureExt;
use futures::{pin_mut, select};
use log::info;
use std::sync::{mpsc, Arc, Mutex};

#[derive(Clone)]
struct EchoHandler;

impl RequestHandler<EchoMarker> for EchoHandler {
    fn handle_request(&self, request: EchoRequest) -> Result<(), Error> {
        handle_echo_request(request)
    }
}

#[async_trait]
impl AsyncRequestHandler<EchoMarker> for EchoHandler {
    async fn handle_request(&self, request: EchoRequest) -> Result<(), Error> {
        handle_echo_request(request)
    }
}

fn handle_echo_request(request: EchoRequest) -> Result<(), Error> {
    info!("{:?}", request);
    let EchoRequest::EchoString { value, responder } = request else {
        panic!();
    };
    responder.send(&value)?;
    Ok(())
}

// Holds a value from a "sender" request until the next "receiver" request takes the value.
// To send a value, echo a non-empty string. To receive a value, echo the empty string.
// This responds to senders with the empty string, and receivers with the most recently sent message.
// This handler is used to test for concurrency: If a server handles requests in serial,
// `RendezvousEchoHandler::handle_request` will block. If requests are handled concurrently,
// they will successfully exchange values.
#[derive(Clone)]
struct RendezvousEchoHandler {
    sender: Arc<mpsc::SyncSender<String>>,
    receiver: Arc<Mutex<mpsc::Receiver<String>>>,
}

impl RendezvousEchoHandler {
    fn new() -> Self {
        // bound is 0 so that so that senders and recievers block one another without concurrency.
        let (tx, rx) = mpsc::sync_channel(0);
        Self { sender: Arc::new(tx), receiver: Arc::new(Mutex::new(rx)) }
    }
}

#[async_trait]
impl AsyncRequestHandler<EchoMarker> for RendezvousEchoHandler {
    async fn handle_request(&self, request: EchoRequest) -> Result<(), Error> {
        let EchoRequest::EchoString { value, responder } = request else {
            panic!();
        };
        if value == "" {
            let receiver = self.receiver.clone();
            let value = fasync::unblock(move || {
                let receiver = receiver.lock().unwrap();
                receiver.recv().unwrap()
            })
            .await;
            responder.send(&value)?;
        } else {
            let sender = self.sender.clone();
            fasync::unblock(move || sender.send(value).unwrap()).await;
            responder.send("")?;
        }

        Ok(())
    }
}

#[fasync::run_singlethreaded(test)]
async fn should_accept_handler_function() -> Result<(), Error> {
    let (client, stream) = create_proxy_and_stream::<EchoMarker>();

    serve_detached(stream, handle_echo_request);

    assert_eq!(client.echo_string("message 1").await?, "message 1");
    assert_eq!(client.echo_string("message 2").await?, "message 2");
    assert_eq!(client.echo_string("message 3").await?, "message 3");
    Ok(())
}

#[fasync::run_singlethreaded(test)]
async fn should_accept_handler_closure() -> Result<(), Error> {
    let (client, stream) = create_proxy_and_stream::<EchoMarker>();

    serve_detached(stream, |request| handle_echo_request(request));

    assert_eq!(client.echo_string("message 1").await?, "message 1");
    assert_eq!(client.echo_string("message 2").await?, "message 2");
    assert_eq!(client.echo_string("message 3").await?, "message 3");
    Ok(())
}

#[fasync::run_singlethreaded(test)]
async fn should_accept_handler_object() -> Result<(), Error> {
    let (client, stream) = create_proxy_and_stream::<EchoMarker>();

    serve_detached(stream, EchoHandler);

    assert_eq!(client.echo_string("message 1").await?, "message 1");
    assert_eq!(client.echo_string("message 2").await?, "message 2");
    assert_eq!(client.echo_string("message 3").await?, "message 3");
    Ok(())
}

#[fasync::run_singlethreaded(test)]
async fn should_accept_async_handler_object() -> Result<(), Error> {
    let (client, stream) = create_proxy_and_stream::<EchoMarker>();

    serve_async_detached(stream, EchoHandler);

    assert_eq!(client.echo_string("message 1").await?, "message 1");
    assert_eq!(client.echo_string("message 2").await?, "message 2");
    assert_eq!(client.echo_string("message 3").await?, "message 3");
    Ok(())
}

#[fasync::run_singlethreaded(test)]
async fn should_serve_all_requests() -> Result<(), Error> {
    let (client, stream) = create_proxy_and_stream::<EchoMarker>();
    let server_fut = async move {
        serve(stream, EchoHandler).await.unwrap();
    };
    let client_fut = async move {
        assert_eq!(client.echo_string("message 1").await.unwrap(), "message 1");
        assert_eq!(client.echo_string("message 2").await.unwrap(), "message 2");
        assert_eq!(client.echo_string("message 3").await.unwrap(), "message 3");
    };
    futures::join!(server_fut, client_fut);
    Ok(())
}

// This test will hang indefinitely if requests are not handled concurrently.
#[fasync::run_singlethreaded(test)]
async fn should_serve_all_requests_concurrently() -> Result<(), Error> {
    let (client, stream) = create_proxy_and_stream::<EchoMarker>();
    let server_fut = async move {
        serve_async_concurrent(stream, 0, RendezvousEchoHandler::new()).await.unwrap();
    };

    let client_fut = async move {
        let recv_fut = client.echo_string("").fuse();
        let send_fut = client.echo_string("the message").fuse();
        pin_mut!(send_fut, recv_fut);

        loop {
            select! {
                res = send_fut => assert_eq!(res.unwrap(), ""),
                res = recv_fut => assert_eq!(res.unwrap(), "the message"),
                complete => break,
            };
        }
    };

    futures::join!(server_fut, client_fut);
    Ok(())
}

#[fasync::run_singlethreaded(test)]
async fn should_stop_after_fidl_error() -> Result<(), Error> {
    let (client, server) = create_endpoints::<EchoMarker>();
    let server_fut = async move {
        let server = server.into_stream();
        serve(server, EchoHandler).await.unwrap_err();
    };
    let client_fut = async move {
        // Write an invalid request.
        client.channel().write(&[0xab, 0xcd, 0xef], &mut []).unwrap();
        let client = client.into_proxy();
        assert!(client.echo_string("message 2").await.unwrap_err().is_closed());
        assert!(client.echo_string("message 3").await.unwrap_err().is_closed());
    };
    futures::join!(server_fut, client_fut);
    Ok(())
}

#[fasync::run_singlethreaded(test)]
async fn should_stop_after_handler_returns_error() -> Result<(), Error> {
    let (client, stream) = create_proxy_and_stream::<EchoMarker>();
    let handler = |request| {
        let EchoRequest::EchoString { value, responder } = request else {
            panic!();
        };
        if value == "message 2" {
            Err(anyhow!("failing on message 2"))
        } else {
            responder.send(&value)?;
            Ok(())
        }
    };
    let server_fut = async move {
        serve(stream, handler).await.unwrap_err();
    };
    let client_fut = async move {
        assert_eq!(client.echo_string("message 1").await.unwrap(), "message 1");
        assert!(client.echo_string("message 2").await.unwrap_err().is_closed());
        assert!(client.echo_string("message 3").await.unwrap_err().is_closed());
    };
    futures::join!(server_fut, client_fut);
    Ok(())
}
