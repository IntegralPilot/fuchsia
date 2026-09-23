// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{HyperConnectorFuture, SocketOptions, TcpOptions, TcpStream, parse_ip_addr};
use futures::io;
use http::uri::{Scheme, Uri};
use hyper_util::rt::TokioIo;
use log::warn;
use netext::TokioAsyncReadExt;
use rustls::RootCertStore;
use std::net::ToSocketAddrs;
use std::sync::{Arc, LazyLock};
use std::task::{Context, Poll};
use tokio::net;
use tower_service::Service;

pub fn new_root_cert_store() -> Arc<RootCertStore> {
    // It can be expensive to parse the certs, so cache them
    static ROOT_STORE: LazyLock<Arc<RootCertStore>> = LazyLock::new(|| {
        let mut root_store = rustls::RootCertStore::empty();

        let certs = rustls_native_certs::load_native_certs()
            .expect("Could not load TLS CA certificates from platform root store");

        if !certs.is_empty() {
            let (added, ignored) = root_store.add_parsable_certificates(certs);

            if ignored != 0 {
                warn!("Failed to load {ignored} certificates into the root store");
            }

            if added == 0 {
                panic!("Unable to load any TLS CA certificates from platform root store")
            }
        }

        Arc::new(root_store)
    });

    Arc::clone(&ROOT_STORE)
}

/// A Async-std-compatible implementation of hyper's `Connect` trait which allows
/// creating a TcpStream to a particular destination.
#[derive(Clone, Debug)]
pub struct HyperConnector {
    tcp_options: TcpOptions,
    socket_options: SocketOptions,
}

impl From<(TcpOptions, SocketOptions)> for HyperConnector {
    fn from((tcp_options, socket_options): (TcpOptions, SocketOptions)) -> Self {
        Self { tcp_options, socket_options }
    }
}

impl HyperConnector {
    pub fn new() -> Self {
        Self::from_tcp_options(TcpOptions::default())
    }

    pub fn from_tcp_options(tcp_options: TcpOptions) -> Self {
        Self { tcp_options, socket_options: SocketOptions::default() }
    }
}

impl Service<Uri> for HyperConnector {
    type Response = TokioIo<TcpStream>;
    type Error = std::io::Error;
    type Future = HyperConnectorFuture;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, dst: Uri) -> Self::Future {
        let self_ = self.clone();
        HyperConnectorFuture { fut: Box::pin(async move { self_.call_async(dst).await }) }
    }
}

impl HyperConnector {
    async fn call_async(&self, dst: Uri) -> Result<TokioIo<TcpStream>, io::Error> {
        let port = match dst.port() {
            Some(p) => p.as_u16(),
            None => {
                if dst.scheme() == Some(&Scheme::HTTPS) {
                    443
                } else {
                    80
                }
            }
        };

        let host = match dst.host() {
            Some(host) => host,
            _ => return Err(io::Error::other("missing host in Uri")),
        };

        let addr = parse_ip_addr(host, port, |_| async {
            Err(io::Error::other("does not yet support non-integer zone ids"))
        })
        .await?;

        if self.socket_options.bind_device.is_some() {
            unimplemented!(
                "TODO(https://fxbug.dev/42083862) fuchsia-hyper does not support bind_device on non-fuchsia devices"
            );
        }

        let stream = if let Some(addr) = addr {
            net::TcpStream::connect(addr).await?
        } else {
            resolve_host_port(host, port).await?
        };
        let () = self.tcp_options.apply(&stream)?;

        Ok(TokioIo::new(TcpStream { stream: stream.into_multithreaded_futures_stream() }))
    }
}

/// Resolve a hostname into an address.
async fn resolve_host_port(host: &str, port: u16) -> Result<net::TcpStream, io::Error> {
    // TODO(https://fxbug.dev/42075095): Implement happy eyeballs algorithm to make this
    // more efficient.
    let mut last_err = None;
    for addr in (host, port).to_socket_addrs()? {
        match net::TcpStream::connect(addr).await {
            Ok(stream) => {
                return Ok(stream);
            }
            Err(err) => {
                last_err = Some(err);
            }
        }
    }

    if let Some(err) = last_err {
        Err(err)
    } else {
        Err(io::Error::other("destination resolved to no address"))
    }
}

////////////////////////////////////////////////////////////////////////////////
///// tests

#[cfg(test)]
mod test {
    use crate::*;
    use anyhow::{Error, Result};
    use futures::future::BoxFuture;
    use futures::stream::FuturesUnordered;
    use futures::{StreamExt, TryStreamExt};
    use http_body_util::BodyExt as _;
    use hyper::{Response, StatusCode};
    use std::convert::Infallible;
    use std::io::Write;
    use tokio::net::TcpListener;

    trait AsyncReadWrite: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send {}
    impl<T> AsyncReadWrite for T where T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send {}

    async fn fetch_url<W: Write>(url: hyper::Uri, mut buffer: W) -> Result<StatusCode> {
        let client = new_https_client();

        let mut res = client.get(url).await?;
        let status = res.status();

        if status == StatusCode::OK {
            while let Some(frame) = res.frame().await {
                if let Ok(data) = frame?.into_data() {
                    buffer.write_all(&data)?;
                }
            }
            buffer.flush()?;
        }

        Ok(status)
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_download_succeeds() -> Result<()> {
        let (listener, addr) = {
            let addr = SocketAddr::new(Ipv6Addr::LOCALHOST.into(), 0);
            let listener = TcpListener::bind(&addr).await.unwrap();
            let local_addr = listener.local_addr().unwrap();
            (listener, local_addr)
        };

        #[cfg(target_os = "fuchsia")]
        let listener =
            listener.incoming().map_err(Error::from).map_ok(|conn| TcpStream { stream: conn });
        #[cfg(not(target_os = "fuchsia"))]
        let listener = netext::TcpListenerStream(listener).map_err(Error::from).map_ok(|conn| {
            TcpStream { stream: netext::TokioAsyncReadExt::into_multithreaded_futures_stream(conn) }
        });

        let mut connections = listener
            .map_ok(|conn| Pin::new(Box::new(conn)) as Pin<Box<dyn AsyncReadWrite>>)
            .boxed();

        let (stop, mut rx_stop) = futures::channel::oneshot::channel();

        let server = async move {
            while let Some(Ok(conn)) = connections.next().await {
                let io = hyper_util::rt::TokioIo::new(conn);
                let service = hyper::service::service_fn(
                    move |_req: hyper::Request<hyper::body::Incoming>| async move {
                        Ok::<_, Infallible>(Response::new(http_body_util::Full::new(
                            hyper::body::Bytes::from("Hello"),
                        )))
                    },
                );
                let builder = hyper_util::server::conn::auto::Builder::new(Executor);
                let mut conn_fut = Box::pin(builder.serve_connection(io, service));
                match futures::future::select(&mut conn_fut, &mut rx_stop).await {
                    futures::future::Either::Left(_) => {}
                    futures::future::Either::Right(_) => {
                        conn_fut.as_mut().graceful_shutdown();
                        let _ = conn_fut.await;
                        break;
                    }
                }
            }
            Ok(())
        };

        let client = async {
            let output: Vec<u8> = Vec::new();
            let status = fetch_url(format!("http://{addr}").parse::<hyper::Uri>().unwrap(), output)
                .await
                .unwrap();
            match status {
                StatusCode::OK | StatusCode::FOUND => {}
                _ => assert!(false, "Unexpected status code: {}", status),
            }
            stop.send(()).expect("server to still be running");
            Ok(())
        };

        let mut tasks: FuturesUnordered<BoxFuture<'_, Result<(), Error>>> = FuturesUnordered::new();
        tasks.push(Box::pin(server));
        tasks.push(Box::pin(client));
        while let Some(Ok(())) = tasks.next().await {}
        Ok(())
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_download_handles_bad_domain() -> Result<()> {
        let output: Vec<u8> = Vec::new();
        let res = fetch_url("https://domain.invalid".parse::<hyper::Uri>()?, output).await;
        assert!(res.is_err());
        Ok(())
    }
}
