// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::AnalyticsError;
use http_body_util::BodyExt;
use hyper::{Method, Request};
type Body = http_body_util::Full<hyper::body::Bytes>;

use crate::ga4_event::Post;

use cfg_if::cfg_if;
cfg_if! {
    if #[cfg(test)] {
        // To avoid unused-crate-dependencies error in test
        use fuchsia_hyper as _;
        use crate::mock_https_client::{new_https_client, HttpsClient};
    } else {
        use fuchsia_hyper::{HttpsClient, new_https_client};
    }
}

use std::sync::OnceLock;

const DOMAIN: &str = "www.google-analytics.com";
const ENDPOINT: &str = "/mp/collect";

pub struct GA4AnalyticsClient {
    client: OnceLock<HttpsClient>,
    ga4_key: String,
    ga4_product_code: String,
}

impl GA4AnalyticsClient {
    pub fn new(ga4_key: String, ga4_product_code: String) -> Self {
        Self { client: OnceLock::new(), ga4_key, ga4_product_code }
    }

    #[cfg(test)]
    fn new_with_client(ga4_key: String, ga4_product_code: String, client: HttpsClient) -> Self {
        let cell = OnceLock::new();
        let _ = cell.set(client);
        Self { client: cell, ga4_key, ga4_product_code }
    }

    fn client(&self) -> &HttpsClient {
        self.client.get_or_init(new_https_client)
    }

    fn get_url(&self) -> String {
        format!(
            "https://{}{}?api_secret={}&measurement_id={}",
            DOMAIN, ENDPOINT, self.ga4_key, self.ga4_product_code
        )
    }

    pub async fn send(&self, post: &mut Post) -> Result<(), AnalyticsError> {
        let post_body = post.to_json();
        let url = self.get_url();
        log::trace!(url:%, post_body:%; "POSTING GA4 ANALYTICS");

        let req = Request::builder()
            .method(Method::POST)
            .uri(url)
            .header("Content-Type", "application/json")
            .body(Body::from(post_body))?;
        let res = self.client().request(req).await;
        Ok(match res {
            Ok(mut res) => {
                log::trace!("GA 4 Analytics response: {}", res.status());
                while let Some(frame) = res.body_mut().frame().await {
                    log::trace!(frame:?; "");
                }
            }
            Err(e) => log::trace!("Error posting GA 4 analytics: {}", e),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::ga4_event::{Event, Post};
    use fuchsia_async;
    use hyper::Method;
    use url::form_urlencoded;

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_send() {
        // Create an analytics client
        let https_client = HttpsClient::mock();

        let client = GA4AnalyticsClient::new_with_client(
            "key".to_string(),
            "code".to_string(),
            https_client,
        );

        // Construct and send the post
        let mut post = Post::new(
            "test_client".to_string(),
            None,
            None,
            vec![Event::new("test_event".to_string(), None)],
        );
        let result = client.send(&mut post).await;
        assert!(result.is_ok());

        // Validate the request
        let req = client.client().take_last_request().expect("Request should not be empty.");
        // Validate the request method
        assert_eq!(*req.method(), Method::POST);
        // Validate the request URI
        let uri = req.uri();
        assert_eq!(uri.authority().unwrap(), DOMAIN);
        assert_eq!(uri.path(), ENDPOINT);
        let query_map: HashMap<_, _> =
            form_urlencoded::parse(uri.query().unwrap().as_bytes()).collect();
        assert_eq!(query_map.len(), 2);
        assert_eq!(query_map["api_secret"], "key");
        assert_eq!(query_map["measurement_id"], "code");
        // Validate the request body
        let body_bytes = req.into_body().collect().await.unwrap().to_bytes();
        let body_json: serde_json::Value = serde_json::from_slice(&body_bytes[..]).unwrap();
        assert_eq!(body_json["client_id"], "test_client");
        assert_eq!(body_json["events"][0]["name"], "test_event");
    }
}
