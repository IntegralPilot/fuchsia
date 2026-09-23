// Copyright 2024 The Fuchsia Authors
//
// Licensed under a BSD-style license <LICENSE-BSD>, Apache License, Version 2.0
// <LICENSE-APACHE or https://www.apache.org/licenses/LICENSE-2.0>, or the MIT
// license <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your option.
// This file may not be copied, modified, or distributed except according to
// those terms.
use {
    futures::{FutureExt as _, future::BoxFuture},
    http::{Request, Response},
    hyper_util::client::legacy::{Client, ResponseFuture, connect::HttpConnector},
    omaha_client::http_request::{Body, Error, HttpRequest, to_bytes},
    std::time::Duration,
};

// This implements the minimal trait requirements for a http request for the omaha state machine.
// For a detailed implementation, see
// https://cs.opensource.google/fuchsia/fuchsia/+/main:src/sys/pkg/lib/omaha-client-fuchsia/src/http_request.rs
pub struct MinimalHttpRequest {
    timeout: Duration,
    client: Client<hyper_rustls::HttpsConnector<HttpConnector>, Body>,
}

impl HttpRequest for MinimalHttpRequest {
    fn request(&mut self, req: Request<Body>) -> BoxFuture<'_, Result<Response<Vec<u8>>, Error>> {
        // create the initial response future
        let response = self.client.request(req);
        let timeout = self.timeout;

        collect_from_future_on_timeout(response, timeout).boxed()
    }
}

async fn collect_from_future_on_timeout(
    response_future: ResponseFuture,
    timeout: Duration,
) -> Result<Response<Vec<u8>>, Error> {
    tokio::time::timeout(timeout, collect_from_future(response_future))
        .await
        .map_err(|_| Error::new_timeout())?
}

// Helper to clarify the types of the futures involved
async fn collect_from_future(response_future: ResponseFuture) -> Result<Response<Vec<u8>>, Error> {
    let response = response_future.await.map_err(Error::new_transport)?;
    let (parts, body) = response.into_parts();
    let bytes = to_bytes(body).await?;
    Ok(Response::from_parts(parts, bytes.to_vec()))
}

impl MinimalHttpRequest {
    /// Constructs a new client that uses a default timeout.
    pub fn new() -> Self {
        Self::using_timeout(Duration::from_secs(30))
    }

    /// Constructs a new client which always uses the provided duration instead of the default
    pub fn using_timeout(timeout: Duration) -> Self {
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_native_roots()
            .unwrap()
            .https_or_http()
            .enable_all_versions()
            .build();
        let client: Client<_, Body> =
            Client::builder(hyper_util::rt::TokioExecutor::new()).build(https);
        MinimalHttpRequest { timeout, client }
    }
}
