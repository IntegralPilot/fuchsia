// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context as _;
use fidl::prelude::*;
use fidl_fuchsia_net_http as net_http;
use fidl_fuchsia_pkg_http as fpkg_http;
use fuchsia_async::{self as fasync, TimeoutExt as _};
use fuchsia_component::escrow::EscrowOperation;
use fuchsia_component::server::{Item, ServiceFs, ServiceFsDir};
use fuchsia_hyper as fhyper;
use fuchsia_inspect as finspect;
use futures::StreamExt;
use futures::prelude::*;
use http_body_util::{BodyStream, Full};
use http_client_config::Config;
use hyper::header::{AUTHORIZATION, COOKIE, HeaderName, PROXY_AUTHORIZATION, WWW_AUTHENTICATE};

pub type Body = Full<hyper::body::Bytes>;
use log::{debug, error, info, trace};
use std::str::FromStr as _;

mod pkg;
mod resuming_get;

static MAX_REDIRECTS: u8 = 10;
static DEFAULT_DEADLINE_DURATION: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(15);

fn to_status_line(version: hyper::Version, status: hyper::StatusCode) -> Vec<u8> {
    match status.canonical_reason() {
        None => format!("{:?} {}", version, status.as_str()),
        Some(canonical_reason) => format!("{:?} {} {}", version, status.as_str(), canonical_reason),
    }
    .as_bytes()
    .to_vec()
}

fn tcp_options() -> fhyper::TcpOptions {
    let mut options: fhyper::TcpOptions = std::default::Default::default();

    // Use TCP keepalive to notice stuck connections.
    // After 60s with no data received send a probe every 15s.
    options.keepalive_idle = Some(std::time::Duration::from_secs(60));
    options.keepalive_interval = Some(std::time::Duration::from_secs(15));
    // After 8 probes go unacknowledged treat the connection as dead.
    options.keepalive_count = Some(8);

    options
}

struct RedirectInfo {
    url: Option<hyper::Uri>,
    referrer: Option<hyper::Uri>,
    method: hyper::Method,
}

fn redirect_info(
    old_uri: &hyper::Uri,
    method: &hyper::Method,
    hyper_response: &hyper::Response<impl hyper::body::Body>,
) -> Option<RedirectInfo> {
    if hyper_response.status().is_redirection() {
        Some(RedirectInfo {
            url: hyper_response
                .headers()
                .get(hyper::header::LOCATION)
                .and_then(|loc| calculate_redirect(old_uri, loc)),
            referrer: hyper_response
                .headers()
                .get(hyper::header::REFERER)
                .and_then(|loc| calculate_redirect(old_uri, loc)),
            method: if hyper_response.status() == hyper::StatusCode::SEE_OTHER {
                hyper::Method::GET
            } else {
                method.clone()
            },
        })
    } else {
        None
    }
}

async fn to_success_response(
    current_url: &hyper::Uri,
    current_method: &hyper::Method,
    hyper_response: hyper::Response<hyper::body::Incoming>,
    scope: vfs::execution_scope::ExecutionScope,
) -> net_http::Response {
    let redirect_info = redirect_info(current_url, current_method, &hyper_response);
    let headers = hyper_response
        .headers()
        .iter()
        .map(|(name, value)| net_http::Header {
            name: name.as_str().as_bytes().to_vec(),
            value: value.as_bytes().to_vec(),
        })
        .collect();

    let (tx, rx) = zx::Socket::create_stream();
    let response = net_http::Response {
        error: None,
        body: Some(rx),
        final_url: Some(current_url.to_string()),
        status_code: Some(hyper_response.status().as_u16() as u32),
        status_line: Some(to_status_line(hyper_response.version(), hyper_response.status())),
        headers: Some(headers),
        redirect: redirect_info.and_then(|info| {
            info.url.map(|url| net_http::RedirectTarget {
                method: Some(info.method.to_string()),
                url: Some(url.to_string()),
                referrer: info.referrer.map(|r| r.to_string()),
                ..Default::default()
            })
        }),
        ..Default::default()
    };

    let _ = scope.spawn(async move {
        let mut hyper_body = BodyStream::new(hyper_response.into_body());
        while let Some(frame) = hyper_body.next().await {
            if let Ok(frame) = frame {
              if let Ok(chunk) = frame.into_data() {
                let mut offset: usize = 0;
                while offset < chunk.len() {
                    let pending = match tx.wait_one(
                        zx::Signals::SOCKET_PEER_CLOSED | zx::Signals::SOCKET_WRITABLE,
                        zx::MonotonicInstant::INFINITE,
                    ).to_result() {
                        Err(status) => {
                            error!("tx.wait() failed - status: {}", status);
                            return;
                        }
                        Ok(pending) => pending,
                    };
                    if pending.contains(zx::Signals::SOCKET_PEER_CLOSED) {
                        info!("tx.wait() saw signal SOCKET_PEER_CLOSED");
                        return;
                    }
                    assert!(pending.contains(zx::Signals::SOCKET_WRITABLE));
                    let written = match tx.write(&chunk[offset..]) {
                        Err(status) => {
                            // Because of the wait above, we shouldn't ever see SHOULD_WAIT here, but to avoid
                            // brittle-ness, continue and wait again in that case.
                            if status == zx::Status::SHOULD_WAIT {
                                error!("Saw SHOULD_WAIT despite waiting first - expected now? - continuing");
                                continue;
                            }
                            info!("tx.write() failed - status: {}", status);
                            return;
                        }
                        Ok(written) => written,
                    };
                    offset += written;
                }
              }
            }
        }
    });

    response
}

fn to_fidl_error(error: &hyper_util::client::legacy::Error) -> net_http::Error {
    use std::error::Error as _;

    if error.is_connect() {
        return net_http::Error::Connect;
    }

    let mut source = error.source();
    while let Some(err) = source {
        if let Some(hyper_err) = err.downcast_ref::<hyper::Error>() {
            if hyper_err.is_parse() || hyper_err.is_parse_status() || hyper_err.is_parse_too_large()
            {
                return net_http::Error::UnableToParse;
            }

            if hyper_err.is_closed() || hyper_err.is_canceled() || hyper_err.is_shutdown() {
                return net_http::Error::ChannelClosed;
            }

            if hyper_err.is_timeout() {
                return net_http::Error::DeadlineExceeded;
            }
        }
        source = err.source();
    }

    net_http::Error::Internal
}

fn to_error_response(error: net_http::Error) -> net_http::Response {
    net_http::Response {
        error: Some(error),
        body: None,
        final_url: None,
        status_code: None,
        status_line: None,
        headers: None,
        redirect: None,
        ..Default::default()
    }
}

struct Loader {
    method: hyper::Method,
    url: hyper::Uri,
    headers: hyper::HeaderMap,
    body: Vec<u8>,
    deadline: fasync::MonotonicInstant,
    scope: vfs::execution_scope::ExecutionScope,
}

impl Loader {
    async fn new(
        req: net_http::Request,
        scope: vfs::execution_scope::ExecutionScope,
    ) -> Result<Self, anyhow::Error> {
        let net_http::Request { method, url, headers, body, deadline, .. } = req;
        let method = method.as_ref().map(|method| hyper::Method::from_str(method)).transpose()?;
        let method = method.unwrap_or(hyper::Method::GET);
        if let Some(url) = url {
            let url = hyper::Uri::try_from(url)?;
            let headers = headers
                .unwrap_or_else(|| vec![])
                .into_iter()
                .map(|net_http::Header { name, value }| {
                    let name = hyper::header::HeaderName::from_bytes(&name)?;
                    let value = hyper::header::HeaderValue::from_bytes(&value)?;
                    Ok((name, value))
                })
                .collect::<Result<hyper::HeaderMap, anyhow::Error>>()?;

            let body = match body {
                Some(net_http::Body::Buffer(buffer)) => {
                    let mut bytes = vec![0; buffer.size as usize];
                    buffer.vmo.read(&mut bytes, 0)?;
                    bytes
                }
                Some(net_http::Body::Stream(socket)) => {
                    let mut stream = fasync::Socket::from_socket(socket)
                        .into_datagram_stream()
                        .map(|r| r.context("reading from datagram stream"));
                    let mut bytes = Vec::new();
                    while let Some(chunk) = stream.next().await {
                        bytes.extend(chunk?);
                    }
                    bytes
                }
                None => Vec::new(),
            };

            let deadline = deadline
                .map(|deadline| fasync::MonotonicInstant::from_nanos(deadline))
                .unwrap_or_else(|| fasync::MonotonicInstant::after(DEFAULT_DEADLINE_DURATION));

            trace!("Starting request {} {}", method, url);

            Ok(Loader { method, url, headers, body, deadline, scope })
        } else {
            Err(anyhow::Error::msg("Request missing URL"))
        }
    }

    fn build_request(&self) -> hyper::Request<Body> {
        let Self { method, url, headers, body, deadline: _, scope: _ } = self;
        let mut request = hyper::Request::new(Full::new(body.clone().into()));
        *request.method_mut() = method.clone();
        *request.uri_mut() = url.clone();
        *request.headers_mut() = headers.clone();
        request
    }

    async fn start(mut self, loader_client: net_http::LoaderClientProxy) -> Result<(), zx::Status> {
        let client = fhyper::new_https_client_from_tcp_options(tcp_options());
        loop {
            break match client.request(self.build_request()).await {
                Ok(hyper_response) => {
                    if let Some((url, method)) =
                        handle_redirect(&self.url, &self.method, &hyper_response, &mut self.headers)
                    {
                        let response = to_success_response(
                            &self.url,
                            &self.method,
                            hyper_response,
                            self.scope.clone(),
                        )
                        .await;
                        self.url = url;
                        self.method = method;
                        trace!("Reporting redirect to OnResponse: {} {}", self.method, self.url);
                        match loader_client.on_response(response).await {
                            Ok(()) => {}
                            Err(e) => {
                                debug!("Not redirecting because: {}", e);
                                break Ok(());
                            }
                        };
                        trace!("Redirect allowed to {} {}", self.method, self.url);
                        continue;
                    }
                    let response = to_success_response(
                        &self.url,
                        &self.method,
                        hyper_response,
                        self.scope.clone(),
                    )
                    .await;
                    // We don't care if on_response returns an error since this is the last
                    // callback.
                    let _: Result<_, _> = loader_client.on_response(response).await;
                    Ok(())
                }
                Err(error) => {
                    info!("Received network level error from hyper: {}", error);
                    // We don't care if on_response returns an error since this is the last
                    // callback.
                    let _: Result<_, _> =
                        loader_client.on_response(to_error_response(to_fidl_error(&error))).await;
                    Ok(())
                }
            };
        }
    }

    async fn fetch(
        mut self,
    ) -> Result<(hyper::Response<hyper::body::Incoming>, hyper::Uri, hyper::Method), net_http::Error>
    {
        let deadline = self.deadline;
        if deadline < fasync::MonotonicInstant::now() {
            return Err(net_http::Error::DeadlineExceeded);
        }
        let client = fhyper::new_https_client_from_tcp_options(tcp_options());

        async move {
            let mut redirects = 0;
            loop {
                break match client.request(self.build_request()).await {
                    Ok(hyper_response) => {
                        if redirects != MAX_REDIRECTS {
                            if let Some((url, method)) = handle_redirect(
                                &self.url,
                                &self.method,
                                &hyper_response,
                                &mut self.headers,
                            ) {
                                self.url = url;
                                self.method = method;
                                trace!("Redirecting to {} {}", self.method, self.url);
                                redirects += 1;
                                continue;
                            }
                        }
                        Ok((hyper_response, self.url, self.method))
                    }
                    Err(e) => {
                        info!("Received network level error from hyper: {}", e);
                        Err(to_fidl_error(&e))
                    }
                };
            }
        }
        .on_timeout(deadline, || Err(net_http::Error::DeadlineExceeded))
        .await
    }
}

fn calculate_redirect(
    old_url: &hyper::Uri,
    location: &hyper::header::HeaderValue,
) -> Option<hyper::Uri> {
    let old_parts = old_url.clone().into_parts();
    let mut new_parts = hyper::Uri::try_from(location.as_bytes()).ok()?.into_parts();

    // Prevent insecure redirect downgrade (https -> http)
    if old_parts.scheme.as_ref().map(|s| s.as_str()) == Some("https")
        && new_parts.scheme.as_ref().map(|s| s.as_str()) == Some("http")
    {
        error!("Not following insecure redirect downgrade");
        return None;
    }

    if new_parts.scheme.is_none() {
        new_parts.scheme = old_parts.scheme;
    }
    if new_parts.authority.is_none() {
        new_parts.authority = old_parts.authority;
    }
    Some(hyper::Uri::from_parts(new_parts).ok()?)
}

// A request is considered cross-origin if the scheme or the authority differs
// between the old and new url.
fn is_cross_origin(old_url: &hyper::Uri, new_url: &hyper::Uri) -> bool {
    old_url.scheme() != new_url.scheme() || old_url.authority() != new_url.authority()
}

fn sensitive_headers() -> [HeaderName; 5] {
    [
        AUTHORIZATION,
        COOKIE,
        HeaderName::from_static("cookie2"),
        PROXY_AUTHORIZATION,
        WWW_AUTHENTICATE,
    ]
}

fn strip_sensitive_headers(headers: &mut hyper::HeaderMap) {
    for header in sensitive_headers() {
        let _ = headers.remove(header);
    }
}

fn handle_redirect(
    old_url: &hyper::Uri,
    method: &hyper::Method,
    hyper_response: &hyper::Response<impl hyper::body::Body>,
    headers: &mut hyper::HeaderMap,
) -> Option<(hyper::Uri, hyper::Method)> {
    let redirect = redirect_info(old_url, method, hyper_response)?;
    let url = redirect.url?;
    if is_cross_origin(old_url, &url) {
        strip_sensitive_headers(headers);
    }
    Some((url, redirect.method))
}

async fn loader_server(
    stream: net_http::LoaderRequestStream,
    idle_timeout: fasync::MonotonicDuration,
) -> Result<(), anyhow::Error> {
    let background_tasks = vfs::execution_scope::ExecutionScope::new();
    let (stream, unbind_if_stalled) = detect_stall::until_stalled(stream, idle_timeout);

    stream
        .err_into::<anyhow::Error>()
        .try_for_each_concurrent(None, |message| {
            let scope = background_tasks.clone();
            async move {
                match message {
                    net_http::LoaderRequest::Fetch { request, responder } => {
                        debug!(
                            "Fetch request received (url: {}): {:?}",
                            request
                                .url
                                .as_ref()
                                .and_then(|url| Some(url.as_str()))
                                .unwrap_or_default(),
                            request
                        );
                        let result = Loader::new(request, scope.clone()).await?.fetch().await;
                        responder.send(match result {
                            Ok((hyper_response, final_url, final_method)) => {
                                to_success_response(
                                    &final_url,
                                    &final_method,
                                    hyper_response,
                                    scope.clone(),
                                )
                                .await
                            }
                            Err(error) => to_error_response(error),
                        })?;
                    }
                    net_http::LoaderRequest::Start { request, client, control_handle } => {
                        debug!(
                            "Start request received (url: {}): {:?}",
                            request
                                .url
                                .as_ref()
                                .and_then(|url| Some(url.as_str()))
                                .unwrap_or_default(),
                            request
                        );
                        Loader::new(request, scope).await?.start(client.into_proxy()).await?;
                        control_handle.shutdown();
                    }
                }
                Ok(())
            }
        })
        .await?;

    background_tasks.wait().await;

    // If the connection did not close or receive new messages within the timeout, send it
    // over to component manager to wait for it on our behalf.
    if let Ok(Some(server_end)) = unbind_if_stalled.await {
        fuchsia_component::client::connect_channel_to_protocol_at::<net_http::LoaderMarker>(
            server_end.into(),
            "/escrow",
        )?;
    }

    Ok(())
}

enum HttpServices {
    Loader(net_http::LoaderRequestStream),
    PkgClient(fpkg_http::ClientRequestStream),
}

#[fuchsia::main]
pub async fn main() -> Result<(), anyhow::Error> {
    log::info!("http-client starting");
    fuchsia_trace_provider::trace_provider_create_with_fdio();
    let inspector = finspect::Inspector::default();
    let pkg_http_node = inspector.root().create_child("pkg-http");
    let pkg_http_connections_node = pkg_http_node.create_child("connections");
    let pkg_http_connection_count = std::sync::atomic::AtomicU64::new(0);
    let _inspect_server_task =
        inspect_runtime::publish(&inspector, inspect_runtime::PublishOptions::default());

    let escrow_operation = EscrowOperation::new();
    escrow_operation.watch_for_stop().expect("Failed to prep escrow operation");

    let config = Config::take_from_startup_handle();
    let tcp_receive_buffer_size = (config.tcp_receive_buffer_size_bytes > 0)
        .then_some(config.tcp_receive_buffer_size_bytes.try_into().unwrap());
    let idle_timeout = if config.stop_on_idle_timeout_millis >= 0 {
        fasync::MonotonicDuration::from_millis(config.stop_on_idle_timeout_millis)
    } else {
        fasync::MonotonicDuration::INFINITE
    };

    let mut fs = ServiceFs::new();
    let _: &mut ServiceFsDir<'_, _> = fs
        .take_and_serve_directory_handle()?
        .dir("svc")
        .add_fidl_service(HttpServices::Loader)
        .add_fidl_service(HttpServices::PkgClient);

    let outgoing_dir_task = async move {
        fs.until_stalled(idle_timeout)
            .for_each_concurrent(None, |item| async {
                match item {
                    Item::Request(services, _active_guard) => match services {
                        HttpServices::Loader(stream) => loader_server(stream, idle_timeout)
                            .await
                            .unwrap_or_else(|e: anyhow::Error| error!("{:?}", e)),
                        HttpServices::PkgClient(stream) => pkg::serve_client_request_stream(
                            stream,
                            idle_timeout,
                            tcp_receive_buffer_size,
                            pkg_http_connections_node.create_child(
                                pkg_http_connection_count
                                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                                    .to_string(),
                            ),
                        )
                        .await
                        .unwrap_or_else(|e: anyhow::Error| error!("{e:#}")),
                    },
                    Item::Stalled(outgoing_directory) => {
                        escrow_operation
                            .run(outgoing_directory.into())
                            .expect("failed to run escrow operation");
                    }
                }
            })
            .await;
    };
    outgoing_dir_task.await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::header::{CONTENT_TYPE, HeaderMap, HeaderValue, LOCATION};

    #[test]
    fn test_is_cross_origin() {
        let origin = hyper::Uri::from_static("https://example.com/path");

        // Same origin, different path = same origin
        assert!(!is_cross_origin(&origin, &hyper::Uri::from_static("https://example.com/other")));

        // Same origin, same path with query = same origin
        assert!(!is_cross_origin(
            &origin,
            &hyper::Uri::from_static("https://example.com/path?foo=bar")
        ));

        // Different host = cross-origin
        assert!(is_cross_origin(&origin, &hyper::Uri::from_static("https://test.com/path")));

        // Different scheme = cross-origin
        assert!(is_cross_origin(&origin, &hyper::Uri::from_static("http://example.com/path")));

        // Different port = cross-origin
        assert!(is_cross_origin(
            &origin,
            &hyper::Uri::from_static("https://example.com:8080/path")
        ));
    }

    #[test]
    fn test_strip_sensitive_headers() {
        let mut headers = HeaderMap::new();
        let (content_label, content_value) =
            (CONTENT_TYPE, HeaderValue::from_static("application/json"));
        assert!(!headers.append(&content_label, content_value.clone()));

        for header in sensitive_headers() {
            assert!(!headers.append(header, HeaderValue::from_static("value")));
        }

        strip_sensitive_headers(&mut headers);

        let mut expected_headers = HeaderMap::new();
        assert!(!expected_headers.append(content_label, content_value));
        assert_eq!(headers, expected_headers);
    }

    #[test]
    fn test_strip_sensitive_headers_multiple_values() {
        let mut headers = HeaderMap::new();
        assert!(!headers.append(COOKIE, HeaderValue::from_static("session1=123"),));
        // Append will return true and add the header value to the list of
        // values for COOKIE.
        assert!(headers.append(COOKIE, HeaderValue::from_static("session2=456"),));

        strip_sensitive_headers(&mut headers);

        assert!(!headers.contains_key(COOKIE));
    }

    fn run_redirect_test(
        redirect_url: &'static str,
        initial_headers: &[(hyper::header::HeaderName, &'static str)],
        expected_url: &'static str,
    ) -> hyper::HeaderMap {
        let old_url = hyper::Uri::from_static("https://example.com/path");
        let method = hyper::Method::GET;

        let mut response =
            hyper::Response::new(http_body_util::Full::<hyper::body::Bytes>::default());
        *response.status_mut() = hyper::StatusCode::MOVED_PERMANENTLY;
        assert!(!response.headers_mut().append(LOCATION, HeaderValue::from_static(redirect_url),));

        let mut headers = HeaderMap::new();
        for (name, val) in initial_headers {
            assert!(!headers.append(name, HeaderValue::from_static(val)));
        }

        let result = handle_redirect(&old_url, &method, &response, &mut headers);
        assert_eq!(result, Some((hyper::Uri::from_static(expected_url), hyper::Method::GET)));
        headers
    }

    #[test]
    fn test_handle_redirect_same_origin() {
        let (auth_key, auth_val) = (AUTHORIZATION, "Bearer token");
        let headers = run_redirect_test(
            "/new-path",
            &[(auth_key.clone(), auth_val)],
            "https://example.com/new-path",
        );
        // Headers must be preserved on same-origin redirect
        assert_eq!(headers.get(&auth_key), Some(&HeaderValue::from_static(auth_val)));
    }

    #[test]
    fn test_handle_redirect_cross_origin() {
        let (auth_key, auth_val) = (AUTHORIZATION, "Bearer token");
        let (content_type_key, content_type_val) = (CONTENT_TYPE, "application/json");
        let headers = run_redirect_test(
            "https://other.com/new-path",
            &[(auth_key.clone(), auth_val), (content_type_key.clone(), content_type_val)],
            "https://other.com/new-path",
        );
        // Authorization must be stripped on cross-origin redirect
        assert!(!headers.contains_key(&auth_key));
        // Content-Type must be preserved
        assert_eq!(
            headers.get(&content_type_key),
            Some(&HeaderValue::from_static(content_type_val))
        );
    }

    #[test]
    fn test_calculate_redirect() {
        let old_url = hyper::Uri::from_static("https://example.com/path");

        // Same scheme, relative path = Perform redirect
        let loc = hyper::header::HeaderValue::from_static("/new-path");
        assert_eq!(
            calculate_redirect(&old_url, &loc),
            Some(hyper::Uri::from_static("https://example.com/new-path"))
        );

        // Same scheme, different host under example namespace = Perform redirect
        let loc = hyper::header::HeaderValue::from_static("https://other.example.com/path");
        assert_eq!(
            calculate_redirect(&old_url, &loc),
            Some(hyper::Uri::from_static("https://other.example.com/path"))
        );

        // Insecure redirect downgrade (https -> http) = Block redirect
        let loc = hyper::header::HeaderValue::from_static("http://example.com/path");
        assert_eq!(calculate_redirect(&old_url, &loc), None);

        // Insecure to insecure redirect = Perform redirect
        let old_url_http = hyper::Uri::from_static("http://example.com/path");
        let loc = hyper::header::HeaderValue::from_static("http://other.example.com/path");
        assert_eq!(
            calculate_redirect(&old_url_http, &loc),
            Some(hyper::Uri::from_static("http://other.example.com/path"))
        );

        // Insecure redirect downgrade with uppercase scheme (https -> HTTP) = Block redirect
        let loc = hyper::header::HeaderValue::from_static("HTTP://example.com/path");
        assert_eq!(calculate_redirect(&old_url, &loc), None);

        // Invalid URL characters = Block redirect
        let loc = hyper::header::HeaderValue::from_bytes(b"https://\xffinvalid.com").unwrap();
        assert_eq!(calculate_redirect(&old_url, &loc), None);
    }
}
