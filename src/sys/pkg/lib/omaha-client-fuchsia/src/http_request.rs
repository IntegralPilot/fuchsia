// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_async::TimeoutExt;
use futures::future::BoxFuture;
use futures::prelude::*;
use http_body_util::BodyExt;
use hyper::{Request, Response};
type Body = http_body_util::Full<hyper::body::Bytes>;
use omaha_client::http_request::{Error, HttpRequest};
use std::time::Duration;

pub struct FuchsiaHyperHttpRequest {
    timeout: Duration,
    client: fuchsia_hyper::HttpsClient,
}

impl HttpRequest for FuchsiaHyperHttpRequest {
    fn request(&mut self, req: Request<Body>) -> BoxFuture<'_, Result<Response<Vec<u8>>, Error>> {
        // create the initial response future
        let response = self.client.request(req);
        let timeout = self.timeout;

        collect_from_future(response).on_timeout(timeout, || Err(Error::new_timeout())).boxed()
    }
}

// Helper to clarify the types of the futures involved
async fn collect_from_future(
    response_future: impl Future<
        Output = Result<Response<hyper::body::Incoming>, hyper_util::client::legacy::Error>,
    >,
) -> Result<Response<Vec<u8>>, Error> {
    let response = response_future.await.map_err(Error::new_transport)?;
    let (parts, body) = response.into_parts();
    let bytes = body.collect().await.map_err(Error::new_transport)?.to_bytes();
    Ok(Response::from_parts(parts, bytes.to_vec()))
}

impl FuchsiaHyperHttpRequest {
    /// Construct a new client that uses a default timeout.
    pub fn new() -> Self {
        Self::using_timeout(Duration::from_secs(30))
    }

    /// Construct a new client which always uses the provided duration instead of the default
    pub fn using_timeout(timeout: Duration) -> Self {
        FuchsiaHyperHttpRequest { timeout, client: fuchsia_hyper::new_https_client() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_hyper_test_support::TestServer;
    use fuchsia_hyper_test_support::fault_injection::{Hang, HangBody};
    use fuchsia_hyper_test_support::handler::StaticResponse;

    /// Helper that constructs a Request for a given path on the given test server.
    fn make_request_for(server: &TestServer, path: &str) -> Request<Body> {
        Request::builder().uri(server.local_url_for_path(path)).body(Body::default()).unwrap()
    }

    /// Test that the HttpRequest implementation works against a simple server and returns
    /// the expected response body.
    #[fuchsia::test]
    async fn test_simple_request() {
        let server =
            TestServer::builder().handler(StaticResponse::ok_body("some data")).start().await;
        let mut client = FuchsiaHyperHttpRequest::using_timeout(Duration::from_secs(5));
        let response = client.request(make_request_for(&server, "some/path")).await.unwrap();
        let string = String::from_utf8(response.into_body()).unwrap();
        assert_eq!(string, "some data");
    }

    /// Test that the HttpRequest implementation properly times out if the server doesn't return
    /// a response over the socket after accepting the connection.
    #[fuchsia::test]
    async fn test_hang() {
        let server = TestServer::builder().handler(Hang).start().await;
        let mut client = FuchsiaHyperHttpRequest::using_timeout(Duration::from_secs(1));
        let response = client.request(make_request_for(&server, "some/path")).await;
        assert!(response.unwrap_err().is_timeout());
    }

    /// Test that the HttpRequest implementation properly times out if the server doesn't return
    /// a the entire body that's expected (after returning a response header).
    #[fuchsia::test]
    async fn test_hang_body() {
        let server = TestServer::builder().handler(HangBody::content_length(500)).start().await;
        let mut client = FuchsiaHyperHttpRequest::using_timeout(Duration::from_secs(1));
        let response = client.request(make_request_for(&server, "some/path")).await;
        assert!(response.unwrap_err().is_timeout());
    }
}
