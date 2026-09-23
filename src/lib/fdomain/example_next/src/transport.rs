// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdomain_client::FDomainTransport;
use fdomain_container::FDomain;
use fdomain_container::wire::FDomainCodec;
use fidl::endpoints::Proxy;
use fidl_next::{ClientEnd, ServerDispatcher, ServerEnd};
use fidl_next_fuchsia_examples::echo::prelude::*;
use fidl_next_fuchsia_examples::echo_launcher::prelude::*;
use futures::stream::Stream;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use vfs::directory::helper::DirectlyMutable;

pub struct LocalFDomainTransport(FDomainCodec);

impl FDomainTransport for LocalFDomainTransport {
    fn poll_send_message(
        mut self: Pin<&mut Self>,
        msg: &[u8],
        _ctx: &mut Context<'_>,
    ) -> Poll<Result<(), Option<std::io::Error>>> {
        Poll::Ready(self.0.message(msg).map_err(|x| Some(std::io::Error::other(x))))
    }
}

impl Stream for LocalFDomainTransport {
    type Item = std::io::Result<Box<[u8]>>;

    fn poll_next(mut self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.as_mut().0).poll_next(ctx).map_err(std::io::Error::other)
    }
}

struct EchoServer {
    server: fidl_next::Server<Echo, fidl::Channel>,
    prefix: String,
    quiet: bool,
}

impl EchoServerHandler<fidl::Channel> for EchoServer {
    async fn echo_string(
        &mut self,
        request: fidl_next::Request<echo::EchoString, fidl::Channel>,
        responder: fidl_next::Responder<echo::EchoString>,
    ) {
        let value = request.wire_payload().value.to_owned();
        if !self.quiet {
            log::info!("Received echo request for string {:?}", value);
        }
        let response = format!("{}{value}", self.prefix);

        if responder.respond(response).await.is_err() {
            self.server.close()
        } else if !self.quiet {
            log::info!("echo response sent successfully");
        }
    }

    async fn send_string(&mut self, _request: fidl_next::Request<echo::SendString, fidl::Channel>) {
    }
}

struct EchoLauncherServer {
    server: fidl_next::Server<EchoLauncher, fidl::Channel>,
    quiet: bool,
    scope: fuchsia_async::Scope,
}

impl EchoLauncherServerHandler<fidl::Channel> for EchoLauncherServer {
    async fn get_echo(
        &mut self,
        request: fidl_next::Request<echo_launcher::GetEcho, fidl::Channel>,
        responder: fidl_next::Responder<echo_launcher::GetEcho>,
    ) {
        let prefix = request.wire_payload().echo_prefix.to_owned();

        if !self.quiet {
            log::info!("Received echo launcher request with prefix string {:?}", prefix);
        }

        let (client, server) = fidl::Channel::create();
        let server_end = ServerEnd::<Echo, _>::from_untyped(server);
        let client_end = ClientEnd::<Echo, _>::from_untyped(client);

        server_end.spawn_on_with(
            |server| EchoServer { server: server.clone(), prefix, quiet: self.quiet },
            &self.scope,
        );

        if responder.respond(client_end).await.is_err() {
            self.server.close();
        } else if !self.quiet {
            log::info!("echo launcher response sent successfully");
        }
    }

    async fn get_echo_pipelined(
        &mut self,
        request: fidl_next::Request<echo_launcher::GetEchoPipelined, fidl::Channel>,
    ) {
        let EchoLauncherGetEchoPipelinedRequest { echo_prefix, request } = request.payload();

        if !self.quiet {
            log::info!(
                "Received pipelined echo launcher request with prefix string {:?}",
                echo_prefix
            );
        }

        request.spawn_on_with(
            |server| EchoServer { server: server.clone(), prefix: echo_prefix, quiet: self.quiet },
            &self.scope,
        );
    }
}

pub fn exec_server(quiet: bool) -> LocalFDomainTransport {
    let service = vfs::service::endpoint(move |scope, channel| {
        log::info!("Spawned endpoint");
        let endpoint = ServerEnd::<EchoLauncher, _>::from_untyped(channel.into_zx_channel());
        let server_dispatcher = ServerDispatcher::new(endpoint);
        scope.spawn(async move {
            let server = server_dispatcher.server();
            let ret = server_dispatcher
                .run(EchoLauncherServer { server, quiet, scope: fuchsia_async::Scope::new() })
                .await;

            if let Err(e) = ret {
                log::warn!(error:? = e; "Echo server terminated");
            }
        });
    });
    let namespace = vfs::directory::immutable::simple();
    namespace.add_entry("echo", service).expect("Could not build namespace!");
    LocalFDomainTransport(FDomainCodec::new(FDomain::new(move || {
        log::info!("Spawning vfs client");
        Ok(fidl::endpoints::ClientEnd::new(
            vfs::directory::serve_read_only(
                Arc::clone(&namespace),
                vfs::execution_scope::ExecutionScope::new(),
            )
            .into_channel()
            .unwrap()
            .into_zx_channel(),
        ))
    })))
}
