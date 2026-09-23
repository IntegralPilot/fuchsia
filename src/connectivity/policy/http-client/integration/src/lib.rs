// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use component_events::events::*;
use component_events::matcher::*;
use fidl_fuchsia_net_http as http;
use fuchsia_async as fasync;
use fuchsia_component_test::ScopedInstance;
use futures::stream::FuturesUnordered;
use futures::{FutureExt as _, StreamExt as _, select};
use std::future::Future;
use std::net::SocketAddr;
use std::pin::pin;
use test_case::test_case;

mod pkg;

const ROOT_DOCUMENT: &str = "Root document\n";

fn big_vec() -> Vec<u8> {
    (0..(32usize << 20)).map(|n| n as u8).collect()
}

const TRIGGER_301: &str = "/trigger_301";
const SEE_OTHER: &str = "/see_other";
const LOOP1: &str = "/loop1";
const LOOP2: &str = "/loop2";
const PENDING: &str = "/pending";
const BIG_STREAM: &str = "/big_stream";

async fn run_without_connecting<F: Future<Output = ()>>(
    behavior: &str,
    func: impl FnOnce(SocketAddr, ScopedInstance) -> F,
) {
    use futures::future::Either;
    use hyper::{Method, Response, StatusCode, header};
    use std::convert::{Infallible, TryInto as _};
    use std::net::{IpAddr, Ipv6Addr};
    use std::sync::Arc;

    let addr = SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0);
    let listener = fasync::net::TcpListener::bind(&addr).expect("bind");
    let server_addr = listener.local_addr().expect("local address");

    const ROOT: &str = "/";
    let root = || ROOT.try_into().expect("root location");
    let loop1 = || LOOP1.try_into().expect("loop1 location");
    let loop2 = || LOOP2.try_into().expect("loop2 location");

    let svc = Arc::new(move |req: hyper::Request<hyper::body::Incoming>| {
        let ready = |t: Response<http_body_util::Full<hyper::body::Bytes>>| {
            std::future::ready(t).left_future()
        };
        let pending = std::future::pending().right_future();

        match (req.method(), req.uri().path()) {
            (&Method::GET, ROOT) => {
                let response = Response::new(http_body_util::Full::new(ROOT_DOCUMENT.into()));
                ready(response)
            }
            (&Method::GET, TRIGGER_301) => {
                let mut response = Response::new(http_body_util::Full::default());
                *response.status_mut() = StatusCode::MOVED_PERMANENTLY;
                assert_eq!(response.headers_mut().insert(header::LOCATION, root()), None);
                ready(response)
            }
            (&Method::POST, SEE_OTHER) => {
                let mut response = Response::new(http_body_util::Full::default());
                *response.status_mut() = StatusCode::SEE_OTHER;
                assert_eq!(response.headers_mut().insert(header::LOCATION, root()), None);
                ready(response)
            }
            (&Method::GET, LOOP1) => {
                let mut response = Response::new(http_body_util::Full::default());
                *response.status_mut() = StatusCode::MOVED_PERMANENTLY;
                assert_eq!(response.headers_mut().insert(header::LOCATION, loop2()), None);
                ready(response)
            }
            (&Method::GET, LOOP2) => {
                let mut response = Response::new(http_body_util::Full::default());
                *response.status_mut() = StatusCode::MOVED_PERMANENTLY;
                assert_eq!(response.headers_mut().insert(header::LOCATION, loop1()), None);
                ready(response)
            }
            (&Method::GET, PENDING) => pending,
            (&Method::GET, BIG_STREAM) => {
                let encoding = "application/octet-stream".try_into().expect("octet stream");
                let mut response = Response::new(http_body_util::Full::new(big_vec().into()));
                assert_eq!(
                    response.headers_mut().insert(header::TRANSFER_ENCODING, encoding),
                    None
                );
                ready(response)
            }
            _ => {
                let mut response = Response::new(http_body_util::Full::default());
                *response.status_mut() = StatusCode::NOT_FOUND;
                ready(response)
            }
        }
        .map(Ok::<_, Infallible>)
    });

    let server = fasync::Task::spawn(async move {
        let mut listener = listener;
        loop {
            match listener.accept().await {
                Ok((next_listener, conn, _)) => {
                    listener = next_listener;
                    let io =
                        hyper_util::rt::TokioIo::new(fuchsia_hyper::TcpStream { stream: conn });
                    let svc = svc.clone();
                    let service = hyper::service::service_fn(move |req| (*svc)(req));
                    fasync::Task::spawn(async move {
                        let _ =
                            hyper_util::server::conn::auto::Builder::new(fuchsia_hyper::Executor)
                                .serve_connection(io, service)
                                .await;
                    })
                    .detach();
                }
                Err(_) => break,
            }
        }
    });

    const HTTP_CLIENT_URL: &str = "#meta/http-client.cm";
    let http_client = ScopedInstance::new(behavior.into(), HTTP_CLIENT_URL.into())
        .await
        .expect("failed to create http-client");
    let fut = pin!(func(server_addr, http_client));
    match futures::future::select(fut, server).await {
        Either::Left(((), _server)) => {}
        Either::Right((result, _fut)) => {
            panic!("hyper server exited: {:?}", result);
        }
    }
}

async fn run<F: Future<Output = ()>>(
    behavior: &str,
    func: impl FnOnce(http::LoaderProxy, SocketAddr) -> F,
) {
    run_without_connecting(behavior, move |addr, http_client| async move {
        let loader = http_client
            .connect_to_protocol_at_exposed_dir()
            .expect("failed to connect to http client");
        func(loader, addr).await;
    })
    .await
}

fn make_request(
    method: &str,
    url: String,
    deadline: Option<zx::MonotonicInstant>,
) -> http::Request {
    // Unless specified by the caller, use an infinite timeout to avoid flakes.
    let deadline = deadline.unwrap_or(zx::MonotonicInstant::INFINITE);
    http::Request {
        url: Some(url),
        method: Some(method.to_string()),
        headers: None,
        body: None,
        deadline: Some(deadline.into_nanos()),
        ..Default::default()
    }
}

fn check_response_common(response: &http::Response, expected_header_names: &[&str]) {
    let http::Response { error, status_code, headers, .. } = response;
    assert_eq!(error, &None);
    assert_eq!(status_code, &Some(200));
    // If the webserver started above ever returns different headers, or changes the order, this
    // assertion will fail. Note that case doesn't matter, and can vary across HTTP client
    // implementations, so we lowercase all the header keys before checking.
    let response_header_names = headers
        .as_ref()
        .map(|headers| {
            headers
                .iter()
                .map(|http::Header { name, value: _ }| {
                    std::str::from_utf8(name).map(str::to_lowercase)
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose();
    let response_header_names = response_header_names.as_ref().map(|option| {
        option.as_ref().map(|vec| vec.iter().map(std::string::String::as_str).collect::<Vec<_>>())
    });
    let response_header_names =
        response_header_names.as_ref().map(|option| option.as_ref().map(|vec| vec.as_slice()));
    assert_eq!(response_header_names, Ok(Some(expected_header_names)));
}

fn check_response(response: &http::Response) {
    check_response_common(response, &["content-length", "date"]);
}

fn check_response_big(response: &http::Response) {
    check_response_common(response, &["transfer-encoding", "date"]);
}

async fn check_body(body: Option<zx::Socket>, mut expected: &[u8]) {
    use futures::AsyncReadExt as _;

    let body = body.expect("response did not include body socket");
    let mut body = fasync::Socket::from_socket(body);

    let mut buf = [0; 1024];
    while !expected.is_empty() {
        let n = body.read(&mut buf).await.expect("failed to read from response body socket");
        assert_eq!(buf[..n], expected[..n]);
        expected = &expected[n..];
    }
}

async fn check_loader_http(loader: http::LoaderProxy, addr: SocketAddr) {
    let response = loader
        .fetch(make_request("GET", format!("http://{}", addr), None))
        .await
        .expect("failed to fetch");
    let () = check_response(&response);
    let http::Response { body, .. } = response;
    let () = check_body(body, ROOT_DOCUMENT.as_bytes()).await;
}

#[test_case("never_idle")]
#[test_case("idle_1ms")]
#[fuchsia::test]
async fn test_fetch_http(behavior: &str) {
    run(behavior, check_loader_http).await
}

#[test_case("never_idle")]
#[test_case("idle_1ms")]
#[fuchsia::test]
async fn test_fetch_past_deadline(behavior: &str) {
    run(behavior, |loader, addr| async move {
        // Deadline expired 10 minutes ago!
        let deadline = Some(zx::MonotonicInstant::after(zx::MonotonicDuration::from_minutes(-10)));
        let http::Response { error, body, .. } = loader
            .fetch(make_request("GET", format!("http://{}", addr), deadline))
            .await
            .expect("failed to fetch");

        assert_eq!(error, Some(http::Error::DeadlineExceeded));
        assert_eq!(body, None);
    })
    .await
}

#[test_case("never_idle")]
#[test_case("idle_1ms")]
#[fuchsia::test]
async fn test_fetch_response_too_slow(behavior: &str) {
    run(behavior, |loader, addr| async move {
        // Deadline expires 100ms from now.
        let deadline = Some(zx::MonotonicInstant::after(zx::MonotonicDuration::from_millis(100)));
        let http::Response { error, body, .. } = loader
            .fetch(make_request("GET", format!("http://{}{}", addr, PENDING), deadline))
            .await
            .expect("failed to fetch");

        assert_eq!(error, Some(http::Error::DeadlineExceeded));
        assert_eq!(body, None);
    })
    .await
}

#[test_case("never_idle")]
#[test_case("idle_1ms")]
#[fuchsia::test]
async fn test_fetch_https(behavior: &str) {
    run(behavior, |loader, addr| async move {
        let http::Response { error, body, .. } = loader
            .fetch(make_request("GET", format!("https://{}", addr), None))
            .await
            .expect("failed to fetch");

        assert_eq!(error, Some(http::Error::Connect));
        assert_eq!(body, None);
    })
    .await
}

#[test_case("never_idle")]
#[test_case("idle_1ms")]
#[fuchsia::test]
async fn test_start_http(behavior: &str) {
    run(behavior, |loader, addr| async move {
        let (tx, rx) = fidl::endpoints::create_endpoints();

        let () = loader
            .start(make_request("GET", format!("http://{}", addr), None), tx)
            .expect("failed to start");

        let mut rx = rx.into_stream();

        let (response, responder) = rx
            .next()
            .await
            .expect("stream error")
            .expect("request error")
            .into_on_response()
            .expect("failed to convert to event stream");

        let () = check_response(&response);
        let http::Response { body, .. } = response;
        let () = check_body(body, ROOT_DOCUMENT.as_bytes()).await;

        let () = responder.send().expect("failed to respond");
    })
    .await
}

#[test_case("never_idle")]
#[test_case("idle_1ms")]
#[fuchsia::test]
async fn test_fetch_redirect(behavior: &str) {
    run(behavior, |loader, addr| async move {
        let response = loader
            .fetch(make_request("GET", format!("http://{}{}", addr, TRIGGER_301), None))
            .await
            .expect("failed to fetch");
        let () = check_response(&response);
        let http::Response { body, final_url, .. } = response;
        let () = check_body(body, ROOT_DOCUMENT.as_bytes()).await;
        assert_eq!(final_url, Some(format!("http://{}/", addr)));
    })
    .await
}

#[test_case("never_idle")]
#[test_case("idle_1ms")]
#[fuchsia::test]
async fn test_start_redirect(behavior: &str) {
    run(behavior, |loader, addr| async move {
        let (tx, rx) = fidl::endpoints::create_endpoints();

        let () = loader
            .start(make_request("GET", format!("http://{}{}", addr, TRIGGER_301), None), tx)
            .expect("failed to start");

        let mut rx = rx.into_stream();

        let (http::Response { status_code, redirect, .. }, responder) = rx
            .next()
            .await
            .expect("stream error")
            .expect("request error")
            .into_on_response()
            .expect("failed to convert to event stream");

        assert_eq!(status_code, Some(301));
        assert_eq!(
            redirect,
            Some(http::RedirectTarget {
                method: Some("GET".to_string()),
                url: Some(format!("http://{}/", addr)),
                referrer: None,
                ..Default::default()
            })
        );

        let () = responder.send().expect("failed to respond");

        let (response, responder) = rx
            .next()
            .await
            .expect("stream error")
            .expect("request error")
            .into_on_response()
            .expect("failed to convert to event stream");
        let () = check_response(&response);
        let http::Response { body, final_url, .. } = response;
        let () = check_body(body, ROOT_DOCUMENT.as_bytes()).await;
        assert_eq!(final_url, Some(format!("http://{}/", addr)));

        let () = responder.send().expect("failed to respond");
    })
    .await
}

#[test_case("never_idle")]
#[test_case("idle_1ms")]
#[fuchsia::test]
async fn test_fetch_see_other(behavior: &str) {
    run(behavior, |loader, addr| async move {
        let response = loader
            .fetch(make_request("POST", format!("http://{}{}", addr, SEE_OTHER), None))
            .await
            .expect("failed to fetch");
        let () = check_response(&response);
        let http::Response { body, final_url, .. } = response;
        let () = check_body(body, ROOT_DOCUMENT.as_bytes()).await;
        assert_eq!(final_url, Some(format!("http://{}/", addr)));
    })
    .await
}

#[test_case("never_idle")]
#[test_case("idle_1ms")]
#[fuchsia::test]
async fn test_start_see_other(behavior: &str) {
    run(behavior, |loader, addr| async move {
        let (tx, rx) = fidl::endpoints::create_endpoints();

        let () = loader
            .start(make_request("POST", format!("http://{}{}", addr, SEE_OTHER), None), tx)
            .expect("failed to start");

        let mut rx = rx.into_stream();

        let (http::Response { status_code, redirect, .. }, responder) = rx
            .next()
            .await
            .expect("stream error")
            .expect("request error")
            .into_on_response()
            .expect("failed to convert to event stream");

        assert_eq!(status_code, Some(303));
        assert_eq!(
            redirect,
            Some(http::RedirectTarget {
                method: Some("GET".to_string()),
                url: Some(format!("http://{}/", addr)),
                referrer: None,
                ..Default::default()
            })
        );

        let () = responder.send().expect("failed to respond");

        let (response, responder) = rx
            .next()
            .await
            .expect("stream error")
            .expect("request error")
            .into_on_response()
            .expect("failed to convert to event stream");

        let () = check_response(&response);
        let http::Response { body, final_url, .. } = response;
        let () = check_body(body, ROOT_DOCUMENT.as_bytes()).await;
        assert_eq!(final_url, Some(format!("http://{}/", addr)));

        let () = responder.send().expect("failed to respond");
    })
    .await
}

#[test_case("never_idle")]
#[test_case("idle_1ms")]
#[fuchsia::test]
async fn test_fetch_max_redirect(behavior: &str) {
    run(behavior, |loader, addr| async move {
        let http::Response { status_code, redirect, .. } = loader
            .fetch(make_request("GET", format!("http://{}{}", addr, LOOP1), None))
            .await
            .expect("failed to fetch");
        // The last request in the redirect loop will always return status code 301
        assert_eq!(status_code, Some(301));
        assert_eq!(
            redirect,
            Some(http::RedirectTarget {
                method: Some("GET".to_string()),
                url: Some(format!("http://{}{}", addr, LOOP2)),
                referrer: None,
                ..Default::default()
            })
        );
    })
    .await
}

#[test_case("never_idle")]
#[test_case("idle_1ms")]
#[fuchsia::test]
async fn test_start_redirect_loop(behavior: &str) {
    run(behavior, |loader, addr| async move {
        let (tx, rx) = fidl::endpoints::create_endpoints();

        let () = loader
            .start(make_request("GET", format!("http://{}{}", addr, LOOP1), None), tx)
            .expect("failed to start");

        let mut rx = rx.into_stream();

        for () in std::iter::repeat(()).take(3) {
            let (http::Response { status_code, redirect, .. }, responder) = rx
                .next()
                .await
                .expect("stream error")
                .expect("request error")
                .into_on_response()
                .expect("failed to convert to event stream");

            assert_eq!(status_code, Some(301));
            assert_eq!(
                redirect,
                Some(http::RedirectTarget {
                    method: Some("GET".to_string()),
                    url: Some(format!("http://{}{}", addr, LOOP2)),
                    referrer: None,
                    ..Default::default()
                })
            );

            let () = responder.send().expect("failed to respond");

            let (http::Response { status_code, redirect, .. }, responder) = rx
                .next()
                .await
                .expect("stream error")
                .expect("request error")
                .into_on_response()
                .expect("failed to convert to event stream");

            assert_eq!(status_code, Some(301));
            assert_eq!(
                redirect,
                Some(http::RedirectTarget {
                    method: Some("GET".to_string()),
                    url: Some(format!("http://{}{}", addr, LOOP1)),
                    referrer: None,
                    ..Default::default()
                })
            );

            let () = responder.send().expect("failed to respond");
        }
    })
    .await
}

#[test_case("never_idle")]
#[test_case("idle_1ms")]
#[fuchsia::test]
async fn test_fetch_http_big_stream(behavior: &str) {
    run(behavior, |loader, addr| async move {
        let response = loader
            .fetch(make_request("GET", format!("http://{}{}", addr, BIG_STREAM), None))
            .await
            .expect("failed to fetch");

        let () = check_response_big(&response);
        let http::Response { body, .. } = response;
        let () = check_body(body, &big_vec()).await;
    })
    .await
}

/// Tests that the `http-client` component can actually stop if we configure it so.
#[test_case("idle_1ms")]
#[fuchsia::test]
async fn test_http_client_stops(behavior: &str) {
    run_without_connecting(behavior, |addr, http_client| async move {
        let mut event_stream = EventStream::open().await.unwrap();

        // Connect to the loader.
        let loader: http::LoaderProxy = http_client.connect_to_protocol_at_exposed_dir().unwrap();

        // Cause the framework to deliver the loader server endpoint to the component.
        // This will start the component.
        check_loader_http(loader.clone(), addr).await;
        _ = EventMatcher::ok()
            .moniker(http_client.moniker())
            .wait::<Started>(&mut event_stream)
            .await
            .unwrap();

        // Wait for the component to stop because the connection stalled.
        _ = EventMatcher::ok()
            .stop(Some(ExitStatusMatcher::Clean))
            .moniker(http_client.moniker())
            .wait::<Stopped>(&mut event_stream)
            .await
            .unwrap();

        // Now make a two-way call on the connection again and it should still work
        // (by starting the component again).
        check_loader_http(loader.clone(), addr).await;
        _ = EventMatcher::ok()
            .moniker(http_client.moniker())
            .wait::<Started>(&mut event_stream)
            .await
            .unwrap();
    })
    .await
}

/// Tests that the `http-client` component doesn't stop if we configure it so.
#[test_case("never_idle")]
#[fuchsia::test]
async fn test_http_client_never_stops(behavior: &str) {
    run_without_connecting(behavior, |addr, http_client| async move {
        let mut event_stream = EventStream::open().await.unwrap();

        // Connect to the loader.
        let loader: http::LoaderProxy = http_client.connect_to_protocol_at_exposed_dir().unwrap();

        // Cause the framework to deliver the loader server endpoint to the component.
        // This will start the component.
        check_loader_http(loader.clone(), addr).await;
        _ = EventMatcher::ok()
            .moniker(http_client.moniker())
            .wait::<Started>(&mut event_stream)
            .await
            .unwrap();

        // Wait a while. `http-client` should not stop.
        // A timeout is not the most ideal thing. But if `http-client` incorrectly stopped,
        // this test will flake. Having a flakily failing test should better than no test.
        let mut stop_event = Box::pin(
            EventMatcher::ok()
                .moniker(http_client.moniker())
                .wait::<Stopped>(&mut event_stream)
                .fuse(),
        );
        select! {
            event = &mut stop_event => panic!("Unexpected stop event {event:?}"),
            _ = fasync::Timer::new(fasync::MonotonicDuration::from_millis(200)).fuse() => {},
        };
    })
    .await
}

/// If the client makes a `Start` FIDL call, the `LoaderClient` connection should prevent the
/// component from exiting, even if there were no requests on that connection. The reason is
/// the derived `LoaderClient` connection has state/context associated with it, and we
/// configure the `http-client.cm` to not stop if those connections are open.
#[test_case("idle_1ms")]
#[fuchsia::test]
async fn test_fetch_http_long_start_call_blocks_stop(behavior: &str) {
    run_without_connecting(behavior, |addr, http_client| async move {
        let mut event_stream = EventStream::open().await.unwrap();

        // Connect to the loader with a buffered `Start` call.
        let (client_end, server_end) = fidl::endpoints::create_endpoints::<http::LoaderMarker>();
        let loader = client_end.into_proxy();
        let (tx, rx) = fidl::endpoints::create_endpoints();
        let () = loader
            .start(make_request("GET", format!("http://{}{}", addr, LOOP1), None), tx)
            .expect("failed to start");
        http_client.connect_request_to_protocol_at_exposed_dir(server_end).unwrap();

        _ = EventMatcher::ok()
            .moniker(http_client.moniker())
            .wait::<Started>(&mut event_stream)
            .await
            .unwrap();
        _ = fasync::OnSignals::new(&rx, zx::Signals::CHANNEL_READABLE).await.unwrap();

        // Wait beyond the 1ms timeout. `http-client` should not stop.
        // A timeout is not the most ideal thing. But if `http-client` incorrectly stopped,
        // this test will flake. Having a flakily failing test should better than no test.
        let mut stop_event = Box::pin(
            EventMatcher::ok()
                .moniker(http_client.moniker())
                .wait::<Stopped>(&mut event_stream)
                .fuse(),
        );
        select! {
            event = &mut stop_event => panic!("Unexpected stop event {event:?}"),
            _ = fasync::Timer::new(fasync::MonotonicDuration::from_millis(200)).fuse() => {},
        };

        // `http-client` should still respond to connection requests. This would hang if the
        // component incorrectly prematurely escrowed the outgoing directory.
        check_loader_http(http_client.connect_to_protocol_at_exposed_dir().unwrap(), addr).await;

        // Close the `LoaderClient`. `http-client` should then stop.
        drop(rx);
        _ = stop_event.await;
    })
    .await
}

#[test_case("never_idle")]
#[test_case("idle_1ms")]
#[fuchsia::test]
async fn test_fetch_http_many_requests_at_different_intervals(behavior: &str) {
    run(behavior, |loader, addr| async move {
        // The http-client component may be configured to stop when idling for 1ms in the test,
        // so we exercise request delays around that range.
        for delay in 990..1010 {
            check_loader_http(loader.clone(), addr).await;
            fasync::Timer::new(fasync::MonotonicDuration::from_micros(delay)).await;
        }
    })
    .await
}

#[test_case("never_idle")]
#[test_case("idle_1ms")]
#[fuchsia::test]
async fn test_fetch_http_many_concurrent_connections_at_different_intervals(behavior: &str) {
    run_without_connecting(behavior, |addr, http_client| async move {
        // The http-client component may be configured to stop when idling for 1ms in the test,
        // so we exercise request delays around that range.
        for delay in 990..1010 {
            let tasks = FuturesUnordered::new();

            for _ in 0..5 {
                let loader: http::LoaderProxy =
                    http_client.connect_to_protocol_at_exposed_dir().unwrap();
                tasks.push(check_loader_http(loader, addr));
            }

            let () = tasks.collect().await;
            fasync::Timer::new(fasync::MonotonicDuration::from_micros(delay)).await;
        }
    })
    .await
}
