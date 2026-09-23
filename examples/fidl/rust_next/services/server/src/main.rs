// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_next::{Request, Responder, ServerDispatcher, ServerEnd};
use fidl_next_fuchsia_examples::{Echo, EchoServerHandler, EchoService, EchoServiceHandler, echo};
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use futures::prelude::*;

#[derive(Clone)]
struct EchoImpl {
    reverse: bool,
}

impl EchoServerHandler for EchoImpl {
    async fn echo_string(
        &mut self,
        request: Request<echo::EchoString>,
        responder: Responder<echo::EchoString>,
    ) {
        let value = &request.payload().value;
        println!("Received EchoString request for string {:?}", value);
        let response =
            if self.reverse { value.chars().rev().collect::<String>() } else { value.clone() };
        responder.respond(&response).await.unwrap();
        println!("Response sent successfully");
    }

    async fn send_string(&mut self, request: Request<echo::SendString>) {
        let value = &request.payload().value;
        println!("Received SendString: {}", value);
    }
}

#[derive(Clone)]
struct EchoServer;

impl EchoServiceHandler for EchoServer {
    fn regular_echo(&self, server_end: ServerEnd<Echo>) {
        fasync::Task::spawn(async move {
            println!("Starting regular_echo dispatcher");
            let dispatcher = ServerDispatcher::new(server_end);
            match dispatcher.run(EchoImpl { reverse: false }).await {
                Ok(_) => println!("regular_echo dispatcher finished successfully"),
                Err(e) => println!("regular_echo dispatcher failed: {:?}", e),
            }
        })
        .detach();
    }

    fn reversed_echo(&self, server_end: ServerEnd<Echo>) {
        fasync::Task::spawn(async move {
            println!("Starting reversed_echo dispatcher");
            let dispatcher = ServerDispatcher::new(server_end);
            match dispatcher.run(EchoImpl { reverse: true }).await {
                Ok(_) => println!("reversed_echo dispatcher finished successfully"),
                Err(e) => println!("reversed_echo dispatcher failed: {:?}", e),
            }
        })
        .detach();
    }
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    println!("echo_server: main started");
    let mut fs = ServiceFs::new_local();
    println!("echo_server: ServiceFs created");
    fs.dir("svc").add_fidl_next_service_instance::<EchoService, _>("default", EchoServer);
    fs.take_and_serve_directory_handle()?;
    println!("echo_server: Listening for incoming connections...");
    fs.collect::<()>().await;
    println!("echo_server: Exiting main");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{EchoServer, EchoServiceHandler};
    use fidl_next::fuchsia::create_channel;
    use fidl_next_fuchsia_examples::Echo;

    #[fuchsia::test]
    async fn test_regular_echo() {
        let (client_end, server_end) = create_channel::<Echo>();
        let server = EchoServer;
        server.regular_echo(server_end);
        let client = client_end.spawn();
        let response = client.echo_string("hello").await.unwrap();
        assert_eq!(response.response, "hello");
    }

    #[fuchsia::test]
    async fn test_reversed_echo() {
        let (client_end, server_end) = create_channel::<Echo>();
        let server = EchoServer;
        server.reversed_echo(server_end);
        let client = client_end.spawn();
        let response = client.echo_string("hello").await.unwrap();
        assert_eq!(response.response, "olleh");
    }
}
