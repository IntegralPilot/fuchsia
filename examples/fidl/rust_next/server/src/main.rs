// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_next::{Request, Responder, Server};
use fidl_next_fuchsia_examples::echo::{EchoString, SendString};
use fidl_next_fuchsia_examples::{Echo, EchoServerHandler};
use fuchsia_component::server::ServiceFs;
use futures::prelude::*;

struct EchoServer {
    sender: Server<Echo>,
}

impl EchoServerHandler for EchoServer {
    async fn echo_string(
        &mut self,
        request: Request<EchoString>,
        responder: Responder<EchoString>,
    ) {
        let payload = request.payload();
        let response = payload.value.as_str();
        println!("Received EchoString request for string {:?}", response);
        responder.respond(response).await.expect("failed to send response");
        println!("Response sent successfully");
    }

    async fn send_string(&mut self, request: Request<SendString>) {
        let payload = request.payload();
        println!("Received SendString request for string {:?}", payload.value);
        self.sender.on_string(&payload.value).await.expect("failed to send event");
        println!("Event sent successfully");
    }
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_next_protocol::<Echo, _>(|sender| EchoServer { sender });
    fs.take_and_serve_directory_handle()?;
    println!("Listening for incoming connections...");
    fs.collect::<()>().await;
    Ok(())
}
