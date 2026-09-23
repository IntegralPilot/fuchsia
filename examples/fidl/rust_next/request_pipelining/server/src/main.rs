// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};
use fidl_next::fuchsia::create_channel;
use fidl_next::{Request, Responder};
use fidl_next_fuchsia_examples::{
    Echo, EchoLauncher, EchoLauncherServerHandler, EchoServerHandler, echo, echo_launcher,
};
use fuchsia_component::server::ServiceFs;
use fuchsia_inspect::component;
use fuchsia_inspect::health::Reporter;
use futures::prelude::*;

struct EchoServer {
    prefix: String,
}

impl EchoServerHandler for EchoServer {
    async fn echo_string(
        &mut self,
        request: Request<echo::EchoString>,
        responder: Responder<echo::EchoString>,
    ) {
        let value = request.payload().value;
        log::info!("Got echo request for prefix {}", self.prefix);
        let response = format!("{}: {}", self.prefix, value);
        responder.respond(&response).await.unwrap();
    }

    async fn send_string(&mut self, _request: Request<echo::SendString>) {
        // Not used in this example
    }
}

struct EchoLauncherServer;

impl EchoLauncherServerHandler for EchoLauncherServer {
    async fn get_echo(
        &mut self,
        request: Request<echo_launcher::GetEcho>,
        responder: Responder<echo_launcher::GetEcho>,
    ) {
        let prefix = request.payload().echo_prefix;
        log::info!("Got non pipelined request");
        let (client_end, server_end) = create_channel::<Echo>();
        match responder.respond(client_end).await {
            Ok(_) => {
                server_end.spawn(EchoServer { prefix });
            }
            Err(e) => log::error!("Failed to send client end: {}", e),
        }
    }

    async fn get_echo_pipelined(&mut self, request: Request<echo_launcher::GetEchoPipelined>) {
        let payload = request.payload();
        let (prefix, server_end) = (payload.echo_prefix, payload.request);
        log::info!("Got pipelined request");
        server_end.spawn(EchoServer { prefix });
    }
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let mut fs = ServiceFs::new_local();

    // Initialize inspect.
    let _inspect_server_task = inspect_runtime::publish(
        component::inspector(),
        inspect_runtime::PublishOptions::default(),
    );
    component::health().set_starting_up();

    fs.dir("svc").add_fidl_next_protocol::<EchoLauncher, _>(|_| EchoLauncherServer);

    fs.take_and_serve_directory_handle().context("failed to serve outgoing namespace")?;

    component::health().set_ok();
    log::info!("Running echo launcher server");

    fs.collect::<()>().await;

    Ok(())
}
