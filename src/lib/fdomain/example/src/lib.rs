// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This is mostly cobbled together from the examples in //examples/fidl/rust,
//! particularly //examples/fidl/rust/request_pipeilining

use anyhow::Context;
use fdomain_client::{Channel, Client};
use future::Either;
use futures::join;
use futures::prelude::*;
use std::pin::pin;
use std::sync::Arc;
use {fdomain_fuchsia_examples as echo, fdomain_fuchsia_io as fio};

mod transport;

async fn run_echo_server(stream: echo::EchoRequestStream, prefix: &str) -> anyhow::Result<()> {
    stream
        .map(|result| result.context("failed request"))
        .try_for_each(|request| async move {
            match request {
                // The SendString request is not used in this example, so just
                // ignore it
                echo::EchoRequest::SendString { value: _, control_handle: _ } => {}
                echo::EchoRequest::EchoString { value, responder } => {
                    println!("Got echo request for prefix {}", prefix);
                    let response = format!("{}: {}", prefix, value);
                    responder.send(&response).context("error sending response")?;
                }
            }
            Ok(())
        })
        .await
}

async fn run_echo_launcher_server(
    client: &Arc<Client>,
    stream: echo::EchoLauncherRequestStream,
) -> anyhow::Result<()> {
    // Currently the client only connects at most two Echo clients for each EchoLauncher
    stream
        .map(|result| result.context("request error"))
        .try_for_each_concurrent(2, |request| async move {
            let (echo_prefix, server_end) = match request {
                // In the non pipelined case, we need to initialize the
                // communication channel ourselves
                echo::EchoLauncherRequest::GetEcho { echo_prefix, responder } => {
                    println!("Got non pipelined request");
                    let (client_end, server_end) = client.create_endpoints::<echo::EchoMarker>();
                    responder.send(client_end)?;
                    (echo_prefix, server_end)
                }
                // In the pipelined case, the client is responsible for
                // initializing the channel, and passes the server its end of
                // the channel
                echo::EchoLauncherRequest::GetEchoPipelined {
                    echo_prefix,
                    request,
                    control_handle: _,
                } => {
                    println!("Got pipelined request");
                    (echo_prefix, request)
                }
            };
            // Run the Echo server with the specified prefix
            run_echo_server(server_end.into_stream(), &echo_prefix).await
        })
        .await
}

async fn run_server(
    client: &Arc<Client>,
    server_end: fdomain_client::fidl::ServerEnd<echo::EchoLauncherMarker>,
) -> anyhow::Result<()> {
    let stream = server_end.into_stream();
    let fut = run_echo_launcher_server(client, stream);

    println!("Running echo launcher server");
    fut.await
}

async fn test_clients_with_server(client: &Arc<Client>, server: Channel) -> anyhow::Result<()> {
    let echo_launcher = echo::EchoLauncherProxy::new(server);

    // Create a future that obtains an Echo protocol using the non-pipelined
    // GetEcho method
    let non_pipelined_fut = async {
        println!("Getting echo from launcher proxy");
        let client_end = echo_launcher.get_echo("not pipelined").await?;
        println!("Got echo from launcher proxy");
        // "Upgrade" the client end in the response into an Echo proxy, and
        // make an EchoString request on it
        let proxy = client_end.into_proxy();
        proxy.echo_string("hello").map_ok(|val| println!("Got echo response {}", val)).await?;
        anyhow::Result::Ok(())
    };

    // Create a future that obtains an Echo protocol using the pipelined GetEcho
    // method
    let (proxy, server_end) = client.create_proxy::<echo::EchoMarker>();
    echo_launcher.get_echo_pipelined("pipelined", server_end)?;
    // We can make a request to the server right after sending the pipelined request
    let pipelined_fut =
        proxy.echo_string("hello").map_ok(|val| println!("Got echo response {}", val));

    // Run the two futures to completion
    let (non_pipelined_result, pipelined_result): (anyhow::Result<()>, Result<(), fidl::Error>) =
        join!(non_pipelined_fut, pipelined_fut);
    pipelined_result?;
    non_pipelined_result?;
    Ok(())
}

/// Test the interaction between an echo service and a client where both the
/// server and the client are interacting with their channels via FDomain.
#[fuchsia::test]
async fn server_is_fdomain() {
    let (client, fut) = Client::new(transport::exec_server(false));

    fuchsia_async::Task::spawn(fut).detach();

    let (client_end, server_end) = client.create_endpoints::<echo::EchoLauncherMarker>();
    let server_fut = run_server(&client, server_end);
    let client_fut = test_clients_with_server(&client, client_end.into_channel());
    match futures::future::select(pin!(server_fut), pin!(client_fut)).await {
        Either::Left((server_result, client_fut)) => {
            server_result.unwrap();
            client_fut.await.unwrap();
        }
        Either::Right((client_result, _)) => {
            client_result.unwrap();
        }
    };
}

/// Test the interaction between an echo service and a client where the client
/// is interacting with the channel via FDomain, but the server has the other
/// end of the channel as a real Zircon channel and is using normal FIDL to
/// serve the protocol.
///
/// The client uses the FDomain namespace to contact the server so we also
/// exercise FDomain's namespace functionality.
#[fuchsia::test]
async fn server_is_fidl_in_ns() {
    let (client, fut) = Client::new(transport::exec_server(false));

    fuchsia_async::Task::spawn(fut).detach();

    let namespace = client.namespace().await.unwrap();
    let namespace =
        fdomain_client::fidl::ClientEnd::<fio::DirectoryMarker>::new(namespace).into_proxy();
    let (echo_client, echo_server) = client.create_channel();
    namespace
        .open("echo", fio::Flags::PROTOCOL_SERVICE, &fio::Options::default(), echo_server)
        .unwrap();
    test_clients_with_server(&client, echo_client).await.unwrap();
}
