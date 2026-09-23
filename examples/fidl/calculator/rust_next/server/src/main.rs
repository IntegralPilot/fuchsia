// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A server to handle calculator requests.
//!
//! This component (and the accompying parent realm) is a realistic example of
//! how to create & route client/server components in Fuchsia. It aims to be
//! fully fleshed out and showcase best practices such as:
//!
//! 1. Testing
//! 2. Exposing capabilities
//! 3. Well commented code
//! 4. FIDL interaction
//! 5. Error handling

use anyhow::Context;
use fidl_next::{Request, Responder};
use fidl_next_fuchsia_examples_calculator::{Calculator, CalculatorServerHandler, calculator};
use fuchsia_component::server::ServiceFs;
use fuchsia_inspect::component;
use fuchsia_inspect::health::Reporter;
use futures::prelude::*;

#[derive(Clone)]
pub struct CalculatorServer;

impl CalculatorServerHandler for CalculatorServer {
    async fn add(
        &mut self,
        request: Request<calculator::Add>,
        responder: Responder<calculator::Add>,
    ) {
        let payload = request.payload();
        responder.respond(payload.a + payload.b).await.unwrap();
    }

    async fn subtract(
        &mut self,
        request: Request<calculator::Subtract>,
        responder: Responder<calculator::Subtract>,
    ) {
        let payload = request.payload();
        responder.respond(payload.a - payload.b).await.unwrap();
    }

    async fn multiply(
        &mut self,
        request: Request<calculator::Multiply>,
        responder: Responder<calculator::Multiply>,
    ) {
        let payload = request.payload();
        responder.respond(payload.a * payload.b).await.unwrap();
    }

    async fn divide(
        &mut self,
        request: Request<calculator::Divide>,
        responder: Responder<calculator::Divide>,
    ) {
        let payload = request.payload();
        responder.respond(payload.dividend / payload.divisor).await.unwrap();
    }

    async fn pow(
        &mut self,
        request: Request<calculator::Pow>,
        responder: Responder<calculator::Pow>,
    ) {
        let payload = request.payload();
        responder.respond(payload.base.powf(payload.exponent)).await.unwrap();
    }
}

/// Calculator server entry point.
#[fuchsia::main]
async fn main() -> Result<(), anyhow::Error> {
    let mut service_fs = ServiceFs::new_local();

    // Initialize inspect.
    let _inspect_server_task = inspect_runtime::publish(
        component::inspector(),
        inspect_runtime::PublishOptions::default(),
    );
    component::health().set_starting_up();

    service_fs.dir("svc").add_fidl_next_protocol::<Calculator, _>(|_| CalculatorServer);

    service_fs.take_and_serve_directory_handle().context("failed to serve outgoing namespace")?;

    component::health().set_ok();
    log::debug!("Initialized.");

    service_fs.collect::<()>().await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::CalculatorServer;

    use fidl_next::fuchsia::create_channel;
    use fidl_next_fuchsia_examples_calculator::Calculator;

    #[fuchsia::test]
    async fn test_add() {
        let (client_end, server_end) = create_channel::<Calculator>();
        let sender = client_end.spawn();
        let server = server_end.spawn(CalculatorServer);

        let response = sender.add(4.5, 3.2).await.unwrap();
        assert_eq!(response.sum, 7.7);

        sender.close();
        server.await.unwrap().unwrap();
    }

    #[fuchsia::test]
    async fn test_subtract() {
        let (client_end, server_end) = create_channel::<Calculator>();
        let sender = client_end.spawn();
        let server = server_end.spawn(CalculatorServer);

        let response = sender.subtract(7.7, 3.2).await.unwrap();
        assert_eq!(response.difference, 4.5);

        sender.close();
        server.await.unwrap().unwrap();
    }

    #[fuchsia::test]
    async fn test_multiply() {
        let (client_end, server_end) = create_channel::<Calculator>();
        let sender = client_end.spawn();
        let server = server_end.spawn(CalculatorServer);

        let response = sender.multiply(1.5, 2.0).await.unwrap();
        assert_eq!(response.product, 3.0);

        sender.close();
        server.await.unwrap().unwrap();
    }

    #[fuchsia::test]
    async fn test_divide() {
        let (client_end, server_end) = create_channel::<Calculator>();
        let sender = client_end.spawn();
        let server = server_end.spawn(CalculatorServer);

        let response = sender.divide(2.0, 4.0).await.unwrap();
        assert_eq!(response.quotient, 0.5);

        sender.close();
        server.await.unwrap().unwrap();
    }

    #[fuchsia::test]
    async fn test_pow() {
        let (client_end, server_end) = create_channel::<Calculator>();
        let sender = client_end.spawn();
        let server = server_end.spawn(CalculatorServer);

        let response = sender.pow(3.0, 4.0).await.unwrap();
        assert_eq!(response.power, 81.0);

        sender.close();
        server.await.unwrap().unwrap();
    }
}
