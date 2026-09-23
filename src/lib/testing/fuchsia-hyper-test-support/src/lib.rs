// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(missing_docs)]

//! This module provides a test HTTP(S) server that can be instantiated simply by a unit test, for
//! connecting components to where you need to vary the response(s) from the HTTP(S) server during
//! the operation of the test.
//!
//! It handles the TCP setup, letting the user specify `Handler` implementations which return the
//! responses from the server.  `Handler` implementations are meant to be composable to provide
//! for fault injection and varying behavior in tests.

// This is gratuitously borrowed from src/sys/pkg/lib/fuchsia-pkg-testing/src/serve.rs, and then
// made generic across all requests by removing the repo-serving aspects of it.

use anyhow::Error;
use chrono::Utc;
use fuchsia_async::{self as fasync, Task};
use futures::future::BoxFuture;
use futures::prelude::*;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
#[cfg(not(target_os = "fuchsia"))]
use netext::TokioAsyncReadExt;
use std::convert::Infallible;
use std::net::{Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;

/// Body type implementation for hyper 1.0 test server support.
pub mod body;
pub use crate::body::Body;

// Some provided Handler implementations.
pub mod handler;

// Some provided Handler implementations for injecting faults into the server's behavior.
pub mod fault_injection;

/// A "test" HTTP(S) server which is composed of `Handler` implementations, and holding the
/// connection state.
pub struct TestServer {
    stop: futures::channel::oneshot::Sender<()>,
    addr: SocketAddr,
    use_https: bool,
    task: Task<()>,
}

/// Base trait that all Handlers implement.
pub trait Handler: 'static + Send + Sync {
    /// A Handler impl signals that it wishes to handle a request by returning a response for it,
    /// otherwise it returns None.
    fn handles(
        &self,
        request: &Request<hyper::body::Incoming>,
    ) -> Option<BoxFuture<'_, Response<Body>>>;
}

impl Handler for Arc<dyn Handler> {
    fn handles(
        &self,
        request: &Request<hyper::body::Incoming>,
    ) -> Option<BoxFuture<'_, Response<Body>>> {
        (**self).handles(request)
    }
}

impl TestServer {
    /// return the scheme of the TestServer
    fn scheme(&self) -> &'static str {
        if self.use_https { "https" } else { "http" }
    }

    /// Returns the URL that can be used to connect to this repository from this device.
    pub fn local_url(&self) -> String {
        format!("{}://localhost:{}", self.scheme(), self.addr.port())
    }

    /// Returns the URL for the given path that can be used to connect to this repository from this
    /// device.
    pub fn local_url_for_path(&self, path: &str) -> String {
        let path = path.trim_start_matches('/');
        format!("{}://localhost:{}/{}", self.scheme(), self.addr.port(), path)
    }

    /// Gracefully signal the server to stop and returns a future that resolves when it terminates.
    pub fn stop(self) -> impl Future<Output = ()> {
        self.stop.send(()).expect("remote end to still be open");
        self.task
    }

    /// Internal helper which iterates over all Handlers until it finds one that will respond to the
    /// request.  It then returns that response.  If not response is found, it returns 404 NOT_FOUND.
    async fn handle_request(
        handlers: Arc<Vec<Arc<dyn Handler>>>,
        req: Request<hyper::body::Incoming>,
    ) -> Response<Body> {
        let response = handlers.iter().find_map(|h| h.handles(&req));

        match response {
            Some(response) => response.await,
            None => Response::builder().status(StatusCode::NOT_FOUND).body(Body::empty()).unwrap(),
        }
    }

    /// Create a Builder
    pub fn builder() -> TestServerBuilder {
        TestServerBuilder::new()
    }
}

/// A builder to construct a `TestServer`.
#[derive(Default)]
pub struct TestServerBuilder {
    handlers: Vec<Arc<dyn Handler>>,
    https_certs: Option<(
        Vec<rustls::pki_types::CertificateDer<'static>>,
        rustls::pki_types::PrivateKeyDer<'static>,
    )>,
}

impl TestServerBuilder {
    /// Create a new TestServerBuilder
    pub fn new() -> Self {
        Self::default()
    }

    /// Serve over TLS, using a server certificate rooted the provided certs
    pub fn use_https(mut self, cert_chain: &[u8], private_key: &[u8]) -> Self {
        let cert_chain = parse_cert_chain(cert_chain);
        let private_key = parse_private_key(private_key);
        self.https_certs = Some((cert_chain, private_key));
        self
    }

    /// Add a Handler which implements the server's behavior.  These are given the ability to
    /// handle a request in the order in which they are added to the `TestServerBuilder`.
    pub fn handler(mut self, handler: impl Handler + 'static) -> Self {
        self.handlers.push(Arc::new(handler));
        self
    }

    /// Spawn the server on the current executor, returning a handle to manage the server.
    pub async fn start(self) -> TestServer {
        let (mut listener, addr) = {
            let addr = SocketAddr::new(Ipv6Addr::UNSPECIFIED.into(), 0);
            let listener = bind_listener(&addr).await;
            let local_addr = listener.local_addr().unwrap();
            (listener, local_addr)
        };

        let (stop, rx_stop) = futures::channel::oneshot::channel();

        let (tls_acceptor, use_https) = if let Some((cert_chain, private_key)) = self.https_certs {
            // build a server configuration using a test CA and cert chain
            let tls_config = rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(cert_chain, private_key)
                .unwrap();
            let tls_acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls_config));

            (Some(tls_acceptor), true)
        } else {
            (None, false)
        };

        let task = fasync::Task::spawn(async move {
            let listener = accept_stream(&mut listener);
            #[cfg(target_os = "fuchsia")]
            let listener = listener
                .map_err(Error::from)
                .map_ok(|conn| fuchsia_hyper::TcpStream { stream: conn });
            #[cfg(not(target_os = "fuchsia"))]
            let listener = listener.map_err(Error::from).map_ok(|conn| fuchsia_hyper::TcpStream {
                stream: conn.into_multithreaded_futures_stream(),
            });

            let connections = if let Some(tls_acceptor) = tls_acceptor {
                // wrap incoming tcp streams
                listener
                    .and_then(move |conn| {
                        tls_acceptor.accept(conn).map(|res| match res {
                            Ok(conn) => {
                                Ok(Pin::new(Box::new(conn)) as Pin<Box<dyn AsyncReadWrite>>)
                            }
                            Err(e) => Err(Error::from(e)),
                        })
                    })
                    .boxed() // connections
            } else {
                listener
                    .map_ok(|conn| Pin::new(Box::new(conn)) as Pin<Box<dyn AsyncReadWrite>>)
                    .boxed() // connections
            };

            // This is the root Arc<Vec<Arc<dyn Handler>>>.
            let handlers = Arc::new(self.handlers);
            let builder = hyper_util::server::conn::auto::Builder::new(fuchsia_hyper::Executor);
            let mut rx_stop = rx_stop.fuse();
            let mut connections = connections.fuse();
            let mut tasks = futures::stream::FuturesUnordered::new();
            loop {
                futures::select! {
                    conn_res = connections.next() => {
                        match conn_res {
                            Some(Ok(conn)) => {
                                let handlers = Arc::clone(&handlers);
                                let builder = builder.clone();
                                tasks.push(fasync::Task::spawn(async move {
                                    let _ = builder.serve_connection(
                                        hyper_util::rt::TokioIo::new(conn),
                                        service_fn(move |req| {
                                            let method = req.method().to_owned();
                                            let path = req.uri().path().to_owned();
                                            TestServer::handle_request(Arc::clone(&handlers), req)
                                                .inspect(move |x| {
                                                    println!(
                                                        "{} [test http] {} {} => {}",
                                                        Utc::now().format("%T.%6f"),
                                                        method,
                                                        path,
                                                        x.status()
                                                    )
                                                })
                                                .map(Ok::<_, Infallible>)
                                        }),
                                    ).await;
                                }));
                            }
                            _ => break,
                        }
                    }
                    _ = tasks.next() => {}
                    _ = rx_stop => break,
                }
            }
        });

        TestServer { stop, addr, use_https, task }
    }
}

#[cfg(target_os = "fuchsia")]
async fn bind_listener(addr: &SocketAddr) -> fuchsia_async::net::TcpListener {
    fuchsia_async::net::TcpListener::bind(addr).unwrap()
}

#[cfg(not(target_os = "fuchsia"))]
async fn bind_listener(&addr: &SocketAddr) -> tokio::net::TcpListener {
    tokio::net::TcpListener::bind(addr).await.unwrap()
}

#[cfg(target_os = "fuchsia")]
fn accept_stream<'a>(
    listener: &'a mut fuchsia_async::net::TcpListener,
) -> impl Stream<Item = std::io::Result<fuchsia_async::net::TcpStream>> + 'a {
    use std::task::{Context, Poll};

    #[pin_project::pin_project]
    struct AcceptStream<'a> {
        #[pin]
        listener: &'a mut fuchsia_async::net::TcpListener,
    }

    impl<'a> Stream for AcceptStream<'a> {
        type Item = std::io::Result<fuchsia_async::net::TcpStream>;

        fn poll_next(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Option<<Self as Stream>::Item>> {
            let mut this = self.project();
            match this.listener.async_accept(cx) {
                Poll::Ready(Ok((conn, _addr))) => Poll::Ready(Some(Ok(conn))),
                Poll::Ready(Err(err)) => Poll::Ready(Some(Err(err))),
                Poll::Pending => Poll::Pending,
            }
        }
    }

    AcceptStream { listener }
}

#[cfg(not(target_os = "fuchsia"))]
fn accept_stream<'a>(
    listener: &'a mut tokio::net::TcpListener,
) -> impl Stream<Item = std::io::Result<tokio::net::TcpStream>> + 'a {
    netext::TcpListenerRefStream(listener)
}

fn parse_cert_chain(mut bytes: &[u8]) -> Vec<rustls::pki_types::CertificateDer<'static>> {
    rustls_pemfile::certs(&mut bytes).collect::<Result<Vec<_>, _>>().expect("certs to parse")
}

fn parse_private_key(mut bytes: &[u8]) -> rustls::pki_types::PrivateKeyDer<'static> {
    rustls_pemfile::private_key(&mut bytes)
        .expect("private keys to parse")
        .expect("one private key")
}

trait AsyncReadWrite: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send {}
impl<T> AsyncReadWrite for T where T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send {}

// These are a set of useful functions when writing tests.

/// Create a GET request for a given url, which can be used with any hyper client.
pub fn make_get(
    url: impl AsRef<str>,
) -> Result<Request<http_body_util::Full<hyper::body::Bytes>>, Error> {
    Request::get(url.as_ref()).body(http_body_util::Full::default()).map_err(Error::from)
}

/// Perform an HTTP GET for the given url, returning the result.
pub async fn get(url: impl AsRef<str>) -> Result<Response<hyper::body::Incoming>, Error> {
    let request = make_get(url)?;
    let client = fuchsia_hyper::new_client();
    let response = client.request(request).await?;
    Ok(response)
}

/// Collect a Response into a single Vec of bytes.
pub async fn body_as_bytes<B>(response: Response<B>) -> Result<Vec<u8>, Error>
where
    B: hyper::body::Body,
    B::Error: Into<Error>,
{
    use http_body_util::BodyExt as _;
    let bytes = response.into_body().collect().await.map_err(Into::into)?.to_bytes();
    Ok(bytes.to_vec())
}

/// Collect a Response's Body and convert the body to a string.
pub async fn body_as_string<B>(response: Response<B>) -> Result<String, Error>
where
    B: hyper::body::Body,
    B::Error: Into<Error>,
{
    let bytes = body_as_bytes(response).await?;
    let string = String::from_utf8(bytes)?;
    Ok(string)
}

/// Get a url and return the body of the response as a string.
pub async fn get_body_as_string(url: impl AsRef<str>) -> Result<String, Error> {
    let response = get(url).await?;
    body_as_string(response).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fault_injection::*;
    use crate::handler::*;
    use anyhow::anyhow;
    use fasync::TimeoutExt;

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_start_stop() {
        let server = TestServer::builder().start().await;
        server.stop().await;
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_empty_server_404s() {
        let server = TestServer::builder().start().await;
        let result = get(server.local_url()).await;
        assert_eq!(result.unwrap().status(), StatusCode::NOT_FOUND);
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_shared_handler() {
        let shared: Arc<dyn Handler> = Arc::new(StaticResponse::ok_body("shared"));

        let server = TestServer::builder()
            .handler(ForPath::new("/a", Arc::clone(&shared)))
            .handler(shared)
            .start()
            .await;

        assert_eq!(get_body_as_string(server.local_url_for_path("/a")).await.unwrap(), "shared");
        assert_eq!(get_body_as_string(server.local_url_for_path("/foo")).await.unwrap(), "shared");
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_simple_responder() {
        let server =
            TestServer::builder().handler(StaticResponse::ok_body("some data")).start().await;
        assert_eq!(
            get_body_as_string(server.local_url_for_path("ignored")).await.unwrap(),
            "some data"
        );
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_simple_path() {
        let server = TestServer::builder()
            .handler(ForPath::new("/some/path", StaticResponse::ok_body("some data")))
            .start()
            .await;
        assert_eq!(
            get_body_as_string(server.local_url_for_path("/some/path")).await.unwrap(),
            "some data"
        );
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_simple_path_doesnt_respond_to_wrong_path() {
        let server = TestServer::builder()
            .handler(ForPath::new("/some/path", StaticResponse::ok_body("some data")))
            .start()
            .await;
        // make sure a non-matching path fails
        let result = get(server.local_url_for_path("/other/path")).await;
        assert_eq!(result.unwrap().status(), StatusCode::NOT_FOUND);
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_hang() {
        let server = TestServer::builder().handler(Hang).start().await;
        let result = get(server.local_url_for_path("ignored"))
            .on_timeout(std::time::Duration::from_secs(1), || Err(anyhow!("timed out")))
            .await;
        assert_eq!(result.unwrap_err().to_string(), Error::msg("timed out").to_string());
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_hang_body() {
        let server = TestServer::builder().handler(HangBody::content_length(500)).start().await;
        let result = get_body_as_string(server.local_url_for_path("ignored"))
            .on_timeout(std::time::Duration::from_secs(1), || Err(anyhow!("timed out")))
            .await;
        assert_eq!(result.unwrap_err().to_string(), Error::msg("timed out").to_string());
    }
}
