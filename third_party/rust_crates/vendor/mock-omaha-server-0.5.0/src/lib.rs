// Copyright 2020 The Fuchsia Authors
//
// Licensed under a BSD-style license <LICENSE-BSD>, Apache License, Version 2.0
// <LICENSE-APACHE or https://www.apache.org/licenses/LICENSE-2.0>, or the MIT
// license <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your option.
// This file may not be copied, modified, or distributed except according to
// those terms.

use anyhow::Error;
use derive_builder::Builder;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode, header};
use omaha_client::cup_ecdsa::PublicKeyId;
use omaha_client::cup_ecdsa::test_support::{
    make_default_private_key_for_test, make_default_public_key_id_for_test,
};
use omaha_client::http_request::{Body, empty_body, to_bytes};
use p256::ecdsa::signature::Signer;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use url::Url;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize)]
pub enum OmahaResponse {
    NoUpdate,
    Update,
    UrgentUpdate,
    InvalidResponse,
    InvalidURL,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ResponseAndMetadata {
    pub response: OmahaResponse,
    pub check_assertion: UpdateCheckAssertion,
    pub version: Option<String>,
    pub cohort_assertion: Option<String>,
    pub codebase: String,
    pub package_name: String,
}

impl Default for ResponseAndMetadata {
    fn default() -> ResponseAndMetadata {
        // This default uses examples from Fuchsia, https://fuchsia.dev/
        ResponseAndMetadata {
            response: OmahaResponse::NoUpdate,
            check_assertion: UpdateCheckAssertion::UpdatesEnabled,
            version: Some("0.1.2.3".to_string()),
            cohort_assertion: None,
            codebase: "fuchsia-pkg://integration.test.fuchsia.com/".to_string(),
            package_name:
                "update?hash=deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
                    .to_string(),
        }
    }
}

// The corresponding private key to lib/omaha-client's PublicKey. For testing
// only, since omaha-client never needs to hold a private key.
pub type PrivateKey = p256::ecdsa::SigningKey;

#[derive(Clone, Debug)]
pub struct PrivateKeyAndId {
    pub id: PublicKeyId,
    pub key: PrivateKey,
}

#[derive(Clone, Debug)]
pub struct PrivateKeys {
    pub latest: PrivateKeyAndId,
    pub historical: Vec<PrivateKeyAndId>,
}

impl PrivateKeys {
    pub fn find(&self, id: PublicKeyId) -> Option<&PrivateKey> {
        if self.latest.id == id {
            return Some(&self.latest.key);
        }
        for pair in &self.historical {
            if pair.id == id {
                return Some(&pair.key);
            }
        }
        None
    }
}

pub fn make_default_private_keys_for_test() -> PrivateKeys {
    PrivateKeys {
        latest: PrivateKeyAndId {
            id: make_default_public_key_id_for_test(),
            key: make_default_private_key_for_test(),
        },
        historical: vec![],
    }
}

pub type ResponseMap = HashMap<String, ResponseAndMetadata>;

#[derive(Copy, Clone, Debug, Deserialize)]
pub enum UpdateCheckAssertion {
    UpdatesEnabled,
    UpdatesDisabled,
}

/// Trait for spawning background futures in an executor/runtime-agnostic way.
pub trait Executor {
    /// Spawns a future on the executor.
    fn spawn(&self, fut: impl Future<Output = ()> + Send + 'static);
}

/// An abstract async listener for incoming network connections.
///
/// Implementations of this trait allow `OmahaServer` to bind and accept socket
/// connections without depending on a specific async TCP stream implementation.
pub trait Listener {
    /// The async I/O stream type that implements Hyper's Read and Write traits.
    type Io: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static;

    /// The error type returned when accepting connections.
    type Error;

    /// Accepts an incoming connection asynchronously.
    fn accept(&mut self) -> impl Future<Output = Result<Self::Io, Self::Error>> + Send;
}

#[derive(Clone, Debug, Builder)]
#[builder(pattern = "owned")]
#[builder(derive(Debug))]
pub struct OmahaServer {
    #[builder(default, setter(into))]
    pub responses_by_appid: ResponseMap,
    #[builder(default = "make_default_private_keys_for_test()")]
    pub private_keys: PrivateKeys,
    #[builder(default = "None")]
    pub etag_override: Option<String>,
    #[builder(default)]
    pub require_cup: bool,
}

impl OmahaServer {
    /// Sets the special assertion to make on any future update check requests
    pub fn set_all_update_check_assertions(&mut self, value: UpdateCheckAssertion) {
        for response_and_metadata in self.responses_by_appid.values_mut() {
            response_and_metadata.check_assertion = value;
        }
    }

    /// Sets the special assertion to make on any future cohort in requests
    pub fn set_all_cohort_assertions(&mut self, value: Option<String>) {
        for response_and_metadata in self.responses_by_appid.values_mut() {
            response_and_metadata.cohort_assertion = value.clone();
        }
    }

    /// Start the server with a custom listener and executor, running the accept loop.
    pub async fn start(
        arc_server: Arc<Mutex<OmahaServer>>,
        mut listener: impl Listener + Send + 'static,
        executor: impl Executor + Send + 'static,
    ) -> Result<(), Error> {
        while let Ok(io) = listener.accept().await {
            let arc_server = Arc::clone(&arc_server);
            let service = service_fn(move |req| {
                let arc_server = Arc::clone(&arc_server);
                async move { handle_request(req, &arc_server).await }
            });
            executor.spawn(async move {
                let _ =
                    hyper::server::conn::http1::Builder::new().serve_connection(io, service).await;
            });
        }

        Ok(())
    }
}

/// An [`Executor`] implementation that spawns futures on the `tokio` runtime.
#[cfg(feature = "tokio")]
#[derive(Clone, Copy, Debug, Default)]
pub struct TokioExecutor;

#[cfg(feature = "tokio")]
impl Executor for TokioExecutor {
    fn spawn(&self, fut: impl Future<Output = ()> + Send + 'static) {
        tokio::spawn(fut);
    }
}

#[cfg(feature = "tokio")]
impl Listener for tokio::net::TcpListener {
    type Io = hyper_util::rt::TokioIo<tokio::net::TcpStream>;
    type Error = std::io::Error;

    async fn accept(&mut self) -> Result<Self::Io, Self::Error> {
        let (stream, _) = tokio::net::TcpListener::accept(self).await?;
        Ok(hyper_util::rt::TokioIo::new(stream))
    }
}

fn make_etag(
    request_body: &[u8],
    uri: &str,
    private_keys: &PrivateKeys,
    response_data: &[u8],
) -> Option<String> {
    if uri == "/" {
        return None;
    }

    let parsed_uri = Url::parse(&format!("https://example.com{uri}")).unwrap();
    let mut query_pairs = parsed_uri.query_pairs();

    let (cup2key_key, cup2key_val) = query_pairs.next().unwrap();
    assert_eq!(cup2key_key, "cup2key");

    let (public_key_id_str, _nonce_str) = cup2key_val.split_once(':').unwrap();
    let public_key_id: PublicKeyId = public_key_id_str.parse().unwrap();
    let private_key: &PrivateKey = match private_keys.find(public_key_id) {
        Some(pk) => Some(pk),
        None => {
            log::error!(
                "Could not find public_key_id {:?} in the private_keys map, which only knows about the latest key_id {:?} and the historical key_ids {:?}",
                public_key_id,
                private_keys.latest.id,
                private_keys.historical.iter().map(|pkid| pkid.id).collect::<Vec<_>>(),
            );
            None
        }
    }?;

    let request_hash = Sha256::digest(request_body);
    let response_hash = Sha256::digest(response_data);

    let mut hasher = Sha256::new();
    hasher.update(request_hash);
    hasher.update(response_hash);
    hasher.update(&*cup2key_val);
    let transaction_hash = hasher.finalize();

    let sig: p256::ecdsa::Signature = private_key.sign(&transaction_hash);
    Some(format!("{}:{}", hex::encode(sig.to_der()), hex::encode(request_hash)))
}

pub async fn handle_request<B>(
    req: Request<B>,
    omaha_server: &Mutex<OmahaServer>,
) -> Result<Response<Body>, Error>
where
    B: http_body::Body + std::fmt::Debug + 'static,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    log::debug!("{req:#?}");
    if req.uri().path() == "/set_responses_by_appid" {
        return handle_set_responses(req, omaha_server).await;
    }

    handle_omaha_request(req, omaha_server).await
}

pub async fn handle_set_responses<B>(
    req: Request<B>,
    omaha_server: &Mutex<OmahaServer>,
) -> Result<Response<Body>, Error>
where
    B: http_body::Body + 'static,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    assert_eq!(req.method(), Method::POST);

    let req_body = to_bytes(req).await.map_err(|_| anyhow::anyhow!("failed to read body"))?;
    let req_json: HashMap<String, ResponseAndMetadata> =
        serde_json::from_slice(&req_body).expect("parse json");
    omaha_server.lock().unwrap().responses_by_appid = req_json;

    let builder = Response::builder().status(StatusCode::OK).header(header::CONTENT_LENGTH, 0);
    Ok(builder.body(empty_body()).unwrap())
}

pub async fn handle_omaha_request<B>(
    req: Request<B>,
    omaha_server: &Mutex<OmahaServer>,
) -> Result<Response<Body>, Error>
where
    B: http_body::Body + 'static,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    let omaha_server = omaha_server.lock().unwrap().clone();
    assert_eq!(req.method(), Method::POST);

    if omaha_server.responses_by_appid.is_empty() {
        let builder = Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .header(header::CONTENT_LENGTH, 0);
        log::error!(
            "Received a request before |responses_by_appid| was set; returning an empty response with status 500."
        );
        return Ok(builder.body(empty_body()).unwrap());
    }

    let uri_string = req.uri().to_string();

    let req_body = to_bytes(req).await.map_err(|_| anyhow::anyhow!("failed to read body"))?;
    let req_json: serde_json::Value = serde_json::from_slice(&req_body).expect("parse json");

    let request = req_json.get("request").unwrap();
    let apps = request.get("app").unwrap().as_array().unwrap();

    // If this request contains updatecheck, make sure the mock has the right number of configured apps.
    match apps.iter().filter(|app| app.get("updatecheck").is_some()).count() {
        0 => {}
        x => assert_eq!(x, omaha_server.responses_by_appid.len()),
    }

    let apps: Vec<serde_json::Value> = apps
        .iter()
        .map(|app| {
            let appid = app.get("appid").unwrap();
            let expected = &omaha_server.responses_by_appid[appid.as_str().unwrap()];

            if let Some(expected_version) = &expected.version {
                let version = app.get("version").unwrap();
                assert_eq!(version, expected_version);
            }

            if let Some(expected_update_check) = app.get("updatecheck") {
                let updatedisabled = expected_update_check
                    .get("updatedisabled")
                    .map(|v| v.as_bool().unwrap())
                    .unwrap_or(false);
                match expected.check_assertion {
                    UpdateCheckAssertion::UpdatesEnabled => {
                        assert!(!updatedisabled);
                    }
                    UpdateCheckAssertion::UpdatesDisabled => {
                        assert!(updatedisabled);
                    }
                }

                if let Some(cohort_assertion) = &expected.cohort_assertion {
                    assert_eq!(
                        app.get("cohort")
                            .expect("expected cohort")
                            .as_str()
                            .expect("cohort is string"),
                        cohort_assertion
                    );
                }

                let updatecheck = match expected.response {
                    OmahaResponse::Update => json!({
                        "status": "ok",
                        "urls": {
                            "url": [
                                {
                                    "codebase": expected.codebase,
                                }
                            ]
                        },
                        "manifest": {
                            "version": "0.1.2.3",
                            "actions": {
                                "action": [
                                    {
                                        "run": &expected.package_name,
                                        "event": "install"
                                    },
                                    {
                                        "event": "postinstall"
                                    }
                                ]
                            },
                            "packages": {
                                "package": [
                                    {
                                        "name": &expected.package_name,
                                        "fp": "2.0.1.2.3",
                                        "required": true
                                    }
                                ]
                            }
                        }
                    }),
                    OmahaResponse::UrgentUpdate => json!({
                        "status": "ok",
                        "urls": {
                            "url": [
                                {
                                    "codebase": expected.codebase,
                                }
                            ]
                        },
                        "manifest": {
                            "version": "0.1.2.3",
                            "actions": {
                                "action": [
                                    {
                                        "run": &expected.package_name,
                                        "event": "install"
                                    },
                                    {
                                        "event": "postinstall"
                                    }
                                ]
                            },
                            "packages": {
                                "package": [
                                    {
                                        "name": &expected.package_name,
                                        "fp": "2.0.1.2.3",
                                        "required": true
                                    }
                                ]
                            }
                        },
                        "_urgent_update": true
                    }),
                    OmahaResponse::NoUpdate => json!({
                        "status": "noupdate",
                    }),
                    OmahaResponse::InvalidResponse => json!({
                        "invalid_status": "invalid",
                    }),
                    OmahaResponse::InvalidURL => json!({
                        "status": "ok",
                        "urls": {
                            "url": [
                                {
                                    "codebase": "http://integration.test.fuchsia.com/"
                                }
                            ]
                        },
                        "manifest": {
                            "version": "0.1.2.3",
                            "actions": {
                                "action": [
                                    {
                                        "run": &expected.package_name,
                                        "event": "install"
                                    },
                                    {
                                        "event": "postinstall"
                                    }
                                ]
                            },
                            "packages": {
                                "package": [
                                    {
                                        "name": &expected.package_name,
                                        "fp": "2.0.1.2.3",
                                        "required": true
                                    }
                                ]
                            }
                        }
                    }),
                };
                json!(
                {
                    "cohorthint": "integration-test",
                    "appid": appid,
                    "cohort": "1:1:",
                    "status": "ok",
                    "cohortname": "integration-test",
                    "updatecheck": updatecheck,
                })
            } else {
                assert!(app.get("event").is_some());
                json!(
                {
                    "cohorthint": "integration-test",
                    "appid": appid,
                    "cohort": "1:1:",
                    "status": "ok",
                    "cohortname": "integration-test",
                })
            }
        })
        .collect();
    let response = json!({
        "response": {
            "server": "prod",
            "protocol": "3.0",
            "daystart": {
                "elapsed_seconds": 48810,
                "elapsed_days": 4775
            },
            "app": apps
        }
    });

    let response_data: Vec<u8> = serde_json::to_vec(&response).unwrap();

    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_LENGTH, response_data.len());

    // It is only possible to calculate an induced etag if the incoming request
    // had a valid cup2key query argument.
    let induced_etag: Option<String> =
        make_etag(&req_body, &uri_string, &omaha_server.private_keys, &response_data);

    if omaha_server.require_cup && induced_etag.is_none() {
        panic!(
            "mock-omaha-server was configured to expect CUP, but we received a request without it."
        );
    }

    if let Some(etag) = omaha_server.etag_override.as_ref().or(induced_etag.as_ref()) {
        builder = builder.header(header::ETAG, etag);
    }

    Ok(builder.body(Body::from(response_data)).unwrap())
}

#[cfg(any(all(test, feature = "tokio"), export_testing_macro))]
pub mod tests {
    use super::*;
    use anyhow::Context as _;
    use hyper_util::client::legacy::connect::Connect;
    use std::net::{Ipv4Addr, SocketAddr};

    pub async fn test_no_validate_version<S, Fut, C, CFut, Conn>(
        start_server: S,
        new_http_client: C,
    ) -> Result<(), Error>
    where
        S: FnOnce(Arc<Mutex<OmahaServer>>) -> Fut,
        Fut: Future<Output = Result<String, Error>>,
        C: Fn() -> CFut,
        CFut: Future<Output = hyper_util::client::legacy::Client<Conn, Body>>,
        Conn: Connect + Clone + Send + Sync + 'static,
    {
        // Send a request with no specified version and assert that we don't check.
        // See 0.0.0.1 vs 9.9.9.9 below.
        let server = start_server(Arc::new(Mutex::new(
            OmahaServerBuilder::default()
                .responses_by_appid([(
                    "integration-test-appid-1".to_string(),
                    ResponseAndMetadata {
                        response: OmahaResponse::NoUpdate,
                        version: None,
                        ..Default::default()
                    },
                )])
                .build()
                .unwrap(),
        )))
        .await
        .context("starting server")?;

        let client = new_http_client().await;
        let body = json!({
            "request": {
                "app": [
                    {
                        "appid": "integration-test-appid-1",
                        "version": "9.9.9.9",
                        "updatecheck": { "updatedisabled": false }
                    },
                ]
            }
        });
        let request = Request::post(&server).body(Body::from(body.to_string())).unwrap();

        let response = client.request(request).await?;

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response).await.context("reading response body")?;
        let obj: serde_json::Value =
            serde_json::from_slice(&body).context("parsing response json")?;

        let response = obj.get("response").unwrap();
        let apps = response.get("app").unwrap().as_array().unwrap();
        assert_eq!(apps.len(), 1);
        let status = apps[0].get("updatecheck").unwrap().get("status").unwrap();
        assert_eq!(status, "noupdate");
        Ok(())
    }

    pub async fn test_server_replies<S, Fut, C, CFut, Conn>(
        start_server: S,
        new_http_client: C,
    ) -> Result<(), Error>
    where
        S: FnOnce(Arc<Mutex<OmahaServer>>) -> Fut,
        Fut: Future<Output = Result<String, Error>>,
        C: Fn() -> CFut,
        CFut: Future<Output = hyper_util::client::legacy::Client<Conn, Body>>,
        Conn: Connect + Clone + Send + Sync + 'static,
    {
        let server_url = start_server(Arc::new(Mutex::new(
            OmahaServerBuilder::default()
                .responses_by_appid([
                    (
                        "integration-test-appid-1".to_string(),
                        ResponseAndMetadata {
                            response: OmahaResponse::NoUpdate,
                            version: Some("0.0.0.1".to_string()),
                            ..Default::default()
                        },
                    ),
                    (
                        "integration-test-appid-2".to_string(),
                        ResponseAndMetadata {
                            response: OmahaResponse::NoUpdate,
                            version: Some("0.0.0.2".to_string()),
                            ..Default::default()
                        },
                    ),
                ])
                .build()
                .unwrap(),
        )))
        .await
        .context("starting server")?;

        {
            let client = new_http_client().await;
            let body = json!({
                "request": {
                    "app": [
                        {
                            "appid": "integration-test-appid-1",
                            "version": "0.0.0.1",
                            "updatecheck": { "updatedisabled": false }
                        },
                        {
                            "appid": "integration-test-appid-2",
                            "version": "0.0.0.2",
                            "updatecheck": { "updatedisabled": false }
                        },
                    ]
                }
            });
            let request = Request::post(&server_url).body(Body::from(body.to_string())).unwrap();

            let response = client.request(request).await?;

            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response).await.context("reading response body")?;
            let obj: serde_json::Value =
                serde_json::from_slice(&body).context("parsing response json")?;

            let response = obj.get("response").unwrap();
            let apps = response.get("app").unwrap().as_array().unwrap();
            assert_eq!(apps.len(), 2);
            for app in apps {
                let status = app.get("updatecheck").unwrap().get("status").unwrap();
                assert_eq!(status, "noupdate");
            }
        }

        {
            // change the expected responses; now we only configure one app,
            // 'integration-test-appid-1', which will respond with an update.
            let body = json!({
                "integration-test-appid-1": {
                    "response": "Update",
                    "check_assertion": "UpdatesEnabled",
                    "version": "0.0.0.1",
                    "codebase": "fuchsia-pkg://integration.test.fuchsia.com/",
                    "package_name": "update?hash=deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
                }
            });
            let request = Request::post(format!("{server_url}set_responses_by_appid"))
                .body(Body::from(body.to_string()))
                .unwrap();
            let client = new_http_client().await;
            let response = client.request(request).await?;
            assert_eq!(response.status(), StatusCode::OK);
        }

        {
            let body = json!({
                "request": {
                    "app": [
                        {
                            "appid": "integration-test-appid-1",
                            "version": "0.0.0.1",
                            "updatecheck": { "updatedisabled": false }
                        },
                    ]
                }
            });
            let request = Request::post(&server_url).body(Body::from(body.to_string())).unwrap();

            let client = new_http_client().await;
            let response = client.request(request).await?;

            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response).await.context("reading response body")?;
            let obj: serde_json::Value =
                serde_json::from_slice(&body).context("parsing response json")?;

            let response = obj.get("response").unwrap();
            let apps = response.get("app").unwrap().as_array().unwrap();
            assert_eq!(apps.len(), 1);
            for app in apps {
                let status = app.get("updatecheck").unwrap().get("status").unwrap();
                // We configured 'integration-test-appid-1' to respond with an update.
                assert_eq!(status, "ok");
            }
        }

        Ok(())
    }

    pub async fn test_no_configured_responses<S, Fut, C, CFut, Conn>(
        start_server: S,
        new_http_client: C,
    ) -> Result<(), Error>
    where
        S: FnOnce(Arc<Mutex<OmahaServer>>) -> Fut,
        Fut: Future<Output = Result<String, Error>>,
        C: Fn() -> CFut,
        CFut: Future<Output = hyper_util::client::legacy::Client<Conn, Body>>,
        Conn: Connect + Clone + Send + Sync + 'static,
    {
        let server = start_server(Arc::new(Mutex::new(
            OmahaServerBuilder::default().responses_by_appid([]).build().unwrap(),
        )))
        .await
        .context("starting server")?;

        let client = new_http_client().await;
        let body = json!({
            "request": {
                "app": [
                    {
                        "appid": "integration-test-appid-1",
                        "version": "0.1.2.3",
                        "updatecheck": { "updatedisabled": false }
                    },
                ]
            }
        });
        let request = Request::post(&server).body(Body::from(body.to_string())).unwrap();
        let response = client.request(request).await?;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        Ok(())
    }

    pub async fn test_server_expect_cup_nopanic<S, Fut, C, CFut, Conn>(
        start_server: S,
        new_http_client: C,
    ) -> Result<(), Error>
    where
        S: FnOnce(Arc<Mutex<OmahaServer>>) -> Fut,
        Fut: Future<Output = Result<String, Error>>,
        C: Fn() -> CFut,
        CFut: Future<Output = hyper_util::client::legacy::Client<Conn, Body>>,
        Conn: Connect + Clone + Send + Sync + 'static,
    {
        let server_url = start_server(Arc::new(Mutex::new(
            OmahaServerBuilder::default()
                .responses_by_appid([(
                    "integration-test-appid-1".to_string(),
                    ResponseAndMetadata {
                        response: OmahaResponse::NoUpdate,
                        version: Some("0.0.0.1".to_string()),
                        ..Default::default()
                    },
                )])
                .require_cup(true)
                .build()
                .unwrap(),
        )))
        .await
        .context("starting server")?;

        let client = new_http_client().await;
        let body = json!({
            "request": {
                "app": [
                    {
                        "appid": "integration-test-appid-1",
                        "version": "0.0.0.1",
                        "updatecheck": { "updatedisabled": false }
                    },
                ]
            }
        });
        // CUP attached.
        let request = Request::post(format!(
            "{}?cup2key={}:nonce",
            server_url,
            make_default_public_key_id_for_test()
        ))
        .body(Body::from(body.to_string()))
        .unwrap();

        let response = client.request(request).await?;

        assert_eq!(response.status(), StatusCode::OK);
        Ok(())
    }

    pub async fn test_server_expect_cup_panic<S, Fut, C, CFut, Conn>(
        start_server: S,
        new_http_client: C,
    ) where
        S: FnOnce(Arc<Mutex<OmahaServer>>) -> Fut,
        Fut: Future<Output = Result<String, Error>>,
        C: Fn() -> CFut,
        CFut: Future<Output = hyper_util::client::legacy::Client<Conn, Body>>,
        Conn: Connect + Clone + Send + Sync + 'static,
    {
        let server_url = start_server(Arc::new(Mutex::new(
            OmahaServerBuilder::default()
                .responses_by_appid([(
                    "integration-test-appid-1".to_string(),
                    ResponseAndMetadata {
                        response: OmahaResponse::NoUpdate,
                        version: Some("0.0.0.1".to_string()),
                        ..Default::default()
                    },
                )])
                .require_cup(true)
                .build()
                .unwrap(),
        )))
        .await
        .context("starting server")
        .unwrap();

        let client = new_http_client().await;
        let body = json!({
            "request": {
                "app": [
                    {
                        "appid": "integration-test-appid-1",
                        "version": "0.0.0.1",
                        "updatecheck": { "updatedisabled": false }
                    },
                ]
            }
        });
        // no CUP, but we set .require_cup(true) above, so mock-omaha-server will
        // panic. (See should_panic above.)
        let request = Request::post(&server_url).body(Body::from(body.to_string())).unwrap();
        let _response = client.request(request).await.unwrap();
    }

    #[macro_export]
    macro_rules! declare_tests {
        (
            test_attr: #[$test_attr:meta],
            start_server: $start_server:expr,
            new_http_client: $new_http_client:expr,
            cup_expect_panic: $panic_expected:expr,
        ) => {
            #[$test_attr]
            async fn test_tokio_no_validate_version() -> Result<(), ::anyhow::Error> {
                $crate::tests::test_no_validate_version($start_server, $new_http_client).await
            }

            #[$test_attr]
            async fn test_tokio_server_replies() -> Result<(), ::anyhow::Error> {
                $crate::tests::test_server_replies($start_server, $new_http_client).await
            }

            #[$test_attr]
            async fn test_tokio_no_configured_responses() -> Result<(), ::anyhow::Error> {
                $crate::tests::test_no_configured_responses($start_server, $new_http_client).await
            }

            #[$test_attr]
            async fn test_tokio_server_expect_cup_nopanic() -> Result<(), ::anyhow::Error> {
                $crate::tests::test_server_expect_cup_nopanic($start_server, $new_http_client).await
            }

            #[$test_attr]
            #[should_panic(expected = $panic_expected)]
            async fn test_tokio_server_expect_cup_panic() {
                $crate::tests::test_server_expect_cup_panic($start_server, $new_http_client).await;
            }
        };
    }

    #[cfg(feature = "tokio")]
    declare_tests! {
        test_attr: #[tokio::test],
        start_server: async |server| {
            let addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0);
            let listener = tokio::net::TcpListener::bind(&addr).await?;
            let addr = listener.local_addr()?;
            TokioExecutor.spawn(async move {
                let _ = OmahaServer::start(server, listener, TokioExecutor).await;
            });
            Ok(format!("http://{addr}/"))
        },
        new_http_client: async || {
            hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
                .build_http()
        },
        cup_expect_panic: "hyper::Error(IncompleteMessage)",
    }
}
