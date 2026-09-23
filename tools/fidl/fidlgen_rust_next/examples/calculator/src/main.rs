// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_next::{Client, ClientEnd, Request, Responder, Server, ServerEnd, Transport};
use fidl_next_examples_calculator::calculator::prelude::*;

struct MyCalculatorClient<T: Transport> {
    client: Client<Calculator, T>,
    error: Option<u32>,
}

impl<T: Transport> MyCalculatorClient<T> {
    fn with_client(client: Client<Calculator, T>) -> Self {
        Self { client, error: None }
    }
}

impl<T: Transport> CalculatorClientHandler<T> for MyCalculatorClient<T> {
    async fn on_error(&mut self, request: Request<calculator::OnError, T>) {
        self.error = Some(request.payload().status_code);
        self.client.close();
    }
}

struct MyCalculatorServer {
    last_result: Option<i32>,
}

impl MyCalculatorServer {
    fn new() -> Self {
        Self { last_result: None }
    }
}

impl<T: Transport> CalculatorServerHandler<T> for MyCalculatorServer {
    async fn add(
        &mut self,
        request: Request<calculator::Add, T>,
        responder: Responder<calculator::Add, T>,
    ) {
        let payload = request.payload();

        let sum = payload.a + payload.b;
        self.last_result = Some(sum);

        let _ = responder.respond(sum).await;
    }

    async fn divide(
        &mut self,
        request: Request<calculator::Divide, T>,
        responder: Responder<calculator::Divide, T>,
    ) {
        let payload = request.payload();

        let response = if payload.divisor != 0 {
            let quotient = payload.dividend / payload.divisor;
            self.last_result = Some(quotient);

            Ok(CalculatorDivideResponse {
                quotient: payload.dividend / payload.divisor,
                remainder: payload.dividend % payload.divisor,
            })
        } else {
            Err(DivisionError::DivideByZero)
        };

        let _ = responder.respond_with(response).await;
    }

    async fn clear(&mut self) {
        self.last_result = None;
    }
}

#[cfg(not(target_os = "fuchsia"))]
type Endpoint = fidl_next::fuchsia_async::Mpsc;

#[cfg(target_os = "fuchsia")]
type Endpoint = zx::Channel;

fn create_endpoints() -> (ClientEnd<Calculator, Endpoint>, ServerEnd<Calculator, Endpoint>) {
    #[cfg(not(target_os = "fuchsia"))]
    {
        fidl_next::fuchsia_async::Mpsc::new()
    }
    #[cfg(target_os = "fuchsia")]
    {
        fidl_next::fuchsia::create_channel()
    }
}

async fn add(client: &Client<Calculator, Endpoint>) {
    let response = client.add(16, 26).await.expect("failed to send or receive request");

    assert_eq!(response.sum, 42);
}

async fn divide(client: &Client<Calculator, Endpoint>) {
    // Normal division
    let result = client.divide(100, 3).await.expect("failed to send or receive request");
    let response = result.ok().expect("divide request failed with an error");

    assert_eq!(response.quotient, 33);
    assert_eq!(response.remainder, 1);

    // Cause an error
    let result = client.divide(42, 0).await.expect("failed to send or receive request");

    let error = result.err().expect("divide request succeeded unexpectedly");
    assert_eq!(DivisionError::DivideByZero, error);
}

async fn clear(client: &Client<Calculator, Endpoint>) {
    client.clear().await.expect("failed to send request");
}

async fn on_error(server: &Server<Calculator, Endpoint>) {
    server.on_error(100u32).await.expect("failed to send event");
}

#[fuchsia::main]
async fn main() {
    let (client_end, server_end) = create_endpoints();
    let (client, client_task) = client_end.spawn_handler_full_with(MyCalculatorClient::with_client);
    let (server_task, server) = server_end.spawn_full(MyCalculatorServer::new());

    add(&client).await;
    divide(&client).await;
    clear(&client).await;
    on_error(&server).await;

    client_task.await.unwrap().unwrap();
    server_task.await.unwrap().unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    async fn test_add() {
        let (client_end, server_end) = create_endpoints();
        let (client, client_task) =
            client_end.spawn_handler_full_with(MyCalculatorClient::with_client);
        let server_task = server_end.spawn(MyCalculatorServer::new());

        add(&client).await;

        client.close();

        assert_eq!(client_task.await.unwrap().unwrap().error, None);
        assert_eq!(server_task.await.unwrap().unwrap().last_result, Some(42));
    }

    #[fuchsia::test]
    async fn test_divide() {
        let (client_end, server_end) = create_endpoints();
        let (client, client_task) =
            client_end.spawn_handler_full_with(MyCalculatorClient::with_client);
        let server_task = server_end.spawn(MyCalculatorServer::new());

        divide(&client).await;

        client.close();

        assert_eq!(client_task.await.unwrap().unwrap().error, None);
        assert_eq!(server_task.await.unwrap().unwrap().last_result, Some(33));
    }

    #[fuchsia::test]
    async fn test_clear() {
        let (client_end, server_end) = create_endpoints();
        let (client, client_task) =
            client_end.spawn_handler_full_with(MyCalculatorClient::with_client);
        let server_task = server_end.spawn(MyCalculatorServer::new());

        add(&client).await;
        clear(&client).await;

        client.close();

        assert_eq!(client_task.await.unwrap().unwrap().error, None);
        assert_eq!(server_task.await.unwrap().unwrap().last_result, None);
    }

    #[fuchsia::test]
    async fn test_on_error() {
        let (client_end, server_end) = create_endpoints();
        let (client, client_task) =
            client_end.spawn_handler_full_with(MyCalculatorClient::with_client);
        let (server_task, server) = server_end.spawn_full(MyCalculatorServer::new());

        on_error(&server).await;

        client.close();

        assert_eq!(client_task.await.unwrap().unwrap().error, Some(100));
        assert_eq!(server_task.await.unwrap().unwrap().last_result, None);
    }

    #[fuchsia::test]
    async fn test_send_handlers_run_locally() {
        let (client_end, server_end) = create_endpoints();
        let (client, client_task) =
            client_end.spawn_as_local_handler_full_with(MyCalculatorClient::with_client);
        let server_task = server_end.spawn_as_local(MyCalculatorServer::new());

        add(&client).await;

        client.close();

        assert_eq!(client_task.await.unwrap().unwrap().error, None);
        assert_eq!(server_task.await.unwrap().unwrap().last_result, Some(42));
    }

    #[fuchsia::test]
    async fn test_local_handlers_run_locally() {
        use std::rc::Rc;

        struct MyLocalClient<T: Transport> {
            client: Client<Calculator, T>,
            error: Option<u32>,
            _not_send: Rc<()>,
        }

        impl<T: Transport> MyLocalClient<T> {
            fn with_client(client: Client<Calculator, T>) -> Self {
                Self { client, error: None, _not_send: Rc::new(()) }
            }
        }

        impl<T: Transport> CalculatorLocalClientHandler<T> for MyLocalClient<T> {
            async fn on_error(&mut self, request: Request<calculator::OnError, T>) {
                self.error = Some(request.payload().status_code);
                self.client.close();
            }
        }

        struct MyLocalServer {
            last_result: Option<i32>,
            _not_send: Rc<()>,
        }

        impl MyLocalServer {
            fn new() -> Self {
                Self { last_result: None, _not_send: Rc::new(()) }
            }
        }

        impl<T: Transport> CalculatorLocalServerHandler<T> for MyLocalServer {
            async fn add(
                &mut self,
                request: Request<calculator::Add, T>,
                responder: Responder<calculator::Add, T>,
            ) {
                let payload = request.payload();

                let sum = payload.a + payload.b;
                self.last_result = Some(sum);

                let _ = responder.respond(sum).await;
            }

            async fn divide(
                &mut self,
                request: Request<calculator::Divide, T>,
                responder: Responder<calculator::Divide, T>,
            ) {
                let payload = request.payload();

                let response = if payload.divisor != 0 {
                    let quotient = payload.dividend / payload.divisor;
                    self.last_result = Some(quotient);

                    Ok(CalculatorDivideResponse {
                        quotient: payload.dividend / payload.divisor,
                        remainder: payload.dividend % payload.divisor,
                    })
                } else {
                    Err(DivisionError::DivideByZero)
                };

                let _ = responder.respond_with(response).await;
            }

            async fn clear(&mut self) {
                self.last_result = None;
            }
        }

        let (client_end, server_end) = create_endpoints();
        let (client, client_task) =
            client_end.spawn_local_handler_full_with(MyLocalClient::with_client);
        let server_task = server_end.spawn_local(MyLocalServer::new());

        add(&client).await;

        client.close();

        assert_eq!(client_task.await.unwrap().unwrap().error, None);
        assert_eq!(server_task.await.unwrap().unwrap().last_result, Some(42));
    }
}
