// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::task::{Context, Poll};

use derivative::Derivative;
use ffx_target_net::{SocketProvider, TargetTcpStream};
use futures::future::BoxFuture;
use futures::{AsyncRead as _, AsyncWrite as _, FutureExt as _};
use http::uri::{Scheme, Uri};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::{Connected, Connection};
use thiserror::Error;
use tower_service::Service;

/// An [`hyper`] compatible implementation of an HTTP client that can use
/// target-side sockets to back connections.
#[derive(Clone, Derivative)]
#[derivative(Debug)]
pub struct TargetHyperConnector {
    #[derivative(Debug = "ignore")]
    socket_provider: SocketProvider,
}

impl TargetHyperConnector {
    /// Creates a new connector with a target side [`SocketProvider`].
    pub fn new(socket_provider: SocketProvider) -> Self {
        Self { socket_provider }
    }

    /// Creates a new [`hyper::Client`] from this connector.
    pub fn into_client(self) -> Client<Self, http_body_util::Full<hyper::body::Bytes>> {
        Client::builder(fuchsia_hyper::Executor).build(self)
    }
}

#[derive(Debug, Error)]
pub enum TargetHyperConnectorError {
    #[error(transparent)]
    Target(#[from] ffx_target_net::Error),
    #[error("missing host in Uri")]
    MissingHost,
    #[error("unknown scheme {0}")]
    UnknownScheme(Scheme),
    #[error("failed to parse host '{0}' as IPv4 or IPv6 address")]
    BadHost(String),
}

impl Service<Uri> for TargetHyperConnector {
    type Response = hyper_util::rt::TokioIo<TargetHyperTcpStream>;
    type Error = TargetHyperConnectorError;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, dst: Uri) -> Self::Future {
        let socket_provider = self.socket_provider.clone();
        async move {
            let port = dst.port_u16().map(Ok).unwrap_or_else(|| {
                let scheme = dst.scheme().unwrap_or(&Scheme::HTTP);
                if scheme == &Scheme::HTTP {
                    return Ok(80);
                }
                if scheme == &Scheme::HTTPS {
                    return Ok(443);
                }
                Err(TargetHyperConnectorError::UnknownScheme(scheme.clone()))
            })?;
            let host = dst.host().ok_or(TargetHyperConnectorError::MissingHost)?;
            let host = host
                .parse::<Ipv4Addr>()
                .map(IpAddr::from)
                .or_else(|_| host.parse::<Ipv6Addr>().map(Into::into))
                .map_err(|_| TargetHyperConnectorError::BadHost(host.to_string()))?;

            Ok(hyper_util::rt::TokioIo::new(TargetHyperTcpStream(
                socket_provider.connect(SocketAddr::new(host, port)).await?,
            )))
        }
        .boxed()
    }
}

pub struct TargetHyperTcpStream(TargetTcpStream);

impl tokio::io::AsyncRead for TargetHyperTcpStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<tokio::io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf.initialize_unfilled()).map_ok(|sz| {
            buf.advance(sz);
            ()
        })
    }
}

impl tokio::io::AsyncWrite for TargetHyperTcpStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<tokio::io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<tokio::io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<tokio::io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_close(cx)
    }
}

impl Connection for TargetHyperTcpStream {
    fn connected(&self) -> Connected {
        Connected::new()
    }
}
