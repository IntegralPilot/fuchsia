// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_sync::Mutex;
use http_body_util::BodyExt;
use hyper::{Request, Response, Result};
use std::sync::Arc;
pub type Body = http_body_util::Full<hyper::body::Bytes>;

#[derive(Clone)]
pub struct HttpsClient {
    last_request: Arc<Mutex<Option<Request<Body>>>>,
}

impl HttpsClient {
    pub fn mock() -> Self {
        Self { last_request: Arc::new(Mutex::new(None)) }
    }

    pub async fn request(&self, req: Request<Body>) -> Result<Response<Body>> {
        self.last_request.lock().replace(req);
        Ok(Response::builder().status(200).body(Body::default()).unwrap())
    }

    pub fn take_last_request(&self) -> Option<Request<Body>> {
        self.last_request.lock().take()
    }
}

pub fn new_https_client() -> HttpsClient {
    HttpsClient::mock()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::{Method, Request};

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_mock_https_client() {
        let client = HttpsClient::mock();
        let request = Request::builder()
            .method(Method::POST)
            .uri("https://example.com")
            .body(Body::from("alpha"))
            .unwrap();

        // Send a request
        let response = client.request(request).await.unwrap();
        assert_eq!(response.status(), 200);

        // Take the last request and verify it
        let last_request = client.take_last_request().unwrap();
        assert_eq!(*last_request.uri(), *"https://example.com");
        assert_eq!(*last_request.method(), Method::POST);
        let body = last_request.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"alpha");

        // After taking, the last request should be None
        assert!(client.take_last_request().is_none());
    }

    #[test]
    fn test_new_https_client() {
        let _client = new_https_client();
    }
}
