// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdomain_client::{Channel, Client};
use fdomain_next_fuchsia_examples::echo::prelude::*;
use fdomain_next_fuchsia_examples::echo_launcher::prelude::*;
use fdomain_next_fuchsia_io as fio;
use future::Either;
use futures::join;
use futures::prelude::*;
use std::pin::pin;
use std::sync::Arc;

mod transport;

struct EchoServer {
    server: fidl_next::Server<Echo, fdomain_client::Channel>,
    prefix: String,
}

impl EchoServerHandler<fdomain_client::Channel> for EchoServer {
    async fn echo_string(
        &mut self,
        request: fidl_next::Request<echo::EchoString, fdomain_client::Channel>,
        responder: fidl_next::Responder<echo::EchoString, fdomain_client::Channel>,
    ) {
        let value = request.payload().value;
        let response = format!("{}: {}", self.prefix, value);

        if responder.respond(response).await.is_err() {
            self.server.close();
        }
    }

    async fn send_string(
        &mut self,
        _request: fidl_next::Request<echo::SendString, fdomain_client::Channel>,
    ) {
        // The SendString request is not used in this example, so just
        // ignore it
    }
}

struct EchoLauncherServer {
    server: fidl_next::Server<EchoLauncher, fdomain_client::Channel>,
    client: Arc<Client>,
    scope: fuchsia_async::Scope,
}

impl EchoLauncherServer {
    fn run_echo_server(
        &self,
        server: fidl_next::ServerEnd<Echo, fdomain_client::Channel>,
        echo_prefix: String,
    ) {
        self.scope.spawn(async move {
            println!("Running echo server with prefix {echo_prefix}");
            let dispatcher = fidl_next::ServerDispatcher::new(server);
            let server = dispatcher.server();

            let result = dispatcher.run(EchoServer { server, prefix: echo_prefix }).await;
            if let Err(result) = result {
                println!("Echo server failed: {result:?}");
            }
        });
    }
}

impl EchoLauncherServerHandler for EchoLauncherServer {
    async fn get_echo(
        &mut self,
        request: fidl_next::Request<echo_launcher::GetEcho>,
        responder: fidl_next::Responder<echo_launcher::GetEcho>,
    ) {
        println!("Got non pipelined request");
        let echo_prefix = request.payload().echo_prefix;
        let (client_end, server_end) = self.client.create_channel();
        let client_end = fidl_next::ClientEnd::<Echo, _>::from_untyped(client_end);
        let server_end = fidl_next::ServerEnd::<Echo, _>::from_untyped(server_end);

        if responder.respond(client_end).await.is_err() {
            self.server.close();
            return;
        }
        self.run_echo_server(server_end, echo_prefix);
    }

    async fn get_echo_pipelined(
        &mut self,
        request: fidl_next::Request<echo_launcher::GetEchoPipelined>,
    ) {
        let EchoLauncherGetEchoPipelinedRequest { echo_prefix, request } = request.payload();
        println!("Got pipelined request");
        self.run_echo_server(request, echo_prefix);
    }
}

async fn run_server(
    client: &Arc<Client>,
    server_end: fidl_next::ServerEnd<EchoLauncher>,
) -> anyhow::Result<()> {
    let dispatcher = fidl_next::ServerDispatcher::new(server_end);
    let server = dispatcher.server();
    let fut = dispatcher.run(EchoLauncherServer {
        server,
        client: Arc::clone(&client),
        scope: fuchsia_async::Scope::new(),
    });

    println!("Running echo launcher server");
    fut.await?;
    Ok(())
}

async fn test_clients_with_server(client: &Arc<Client>, server: Channel) -> anyhow::Result<()> {
    let echo_launcher = fidl_next::ClientEnd::<EchoLauncher, _>::from_untyped(server).spawn();

    // Create a future that obtains an Echo protocol using the non-pipelined
    // GetEcho method
    let non_pipelined_fut = async {
        println!("Getting echo from launcher proxy");
        let EchoLauncherGetEchoResponse { response } =
            echo_launcher.get_echo("not pipelined").await?;
        // "Upgrade" the client end in the response into an Echo client, and
        // make an EchoString request on it
        response
            .spawn()
            .echo_string("hello")
            .map_ok(|val| {
                let EchoEchoStringResponse { response } = val;
                println!("Got echo response {}", response);
            })
            .await?;
        anyhow::Result::<(), anyhow::Error>::Ok(())
    };

    // Create a future that obtains an Echo protocol using the pipelined GetEcho
    // method
    let (client_end, server_end) = client.create_channel();
    let client_end = fidl_next::ClientEnd::<Echo, _>::from_untyped(client_end);
    let server_end = fidl_next::ServerEnd::<Echo, _>::from_untyped(server_end);
    echo_launcher.get_echo_pipelined("pipelined", server_end).await?;
    let pipeline_sender = client_end.spawn();
    // We can make a request to the server right after sending the pipelined request
    let pipelined_fut = pipeline_sender.echo_string("hello").map_ok(|val| {
        let EchoEchoStringResponse { response } = val;
        println!("Got echo response {}", response);
    });

    // Run the two futures to completion
    let (non_pipelined_result, pipelined_result): (anyhow::Result<()>, Result<(), _>) =
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

    let (client_end, server_end) = client.create_channel();
    let server_end = fidl_next::ServerEnd::<EchoLauncher, _>::from_untyped(server_end);
    let server_fut = run_server(&client, server_end);
    let client_fut = test_clients_with_server(&client, client_end);
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
    let namespace = fidl_next::ClientEnd::<fio::Directory, _>::from_untyped(namespace).spawn();
    let (echo_client, echo_server) = client.create_channel();
    namespace
        .open("echo", fio::Flags::PROTOCOL_SERVICE, &fio::Options::default(), echo_server)
        .await
        .unwrap();
    test_clients_with_server(&client, echo_client).await.unwrap();
}
