// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fuchsia_async as fasync;
use futures::stream::StreamExt as _;
use hyper_util::rt::TokioIo;
use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};

pub use mock_omaha_server::*;

/// An [`Executor`] implementation that spawns detached tasks on `fuchsia_async`.
#[derive(Clone, Copy, Debug, Default)]
pub struct FuchsiaExecutor;

impl Executor for FuchsiaExecutor {
    fn spawn(&self, fut: impl Future<Output = ()> + Send + 'static) {
        fasync::Task::spawn(fut).detach();
    }
}

pub struct FuchsiaListener {
    addr: SocketAddr,
    stream: fasync::net::AcceptStream,
}

impl FuchsiaListener {
    pub fn bind(addr: &SocketAddr) -> Result<Self, std::io::Error> {
        let listener = fasync::net::TcpListener::bind(addr)?;
        let addr = listener.local_addr()?;
        let stream = listener.accept_stream();
        Ok(Self { addr, stream })
    }
}

impl Listener for FuchsiaListener {
    type Io = TokioIo<fuchsia_hyper::TcpStream>;
    type Error = std::io::Error;

    async fn accept(&mut self) -> Result<Self::Io, Self::Error> {
        let (conn, _) = self.stream.next().await.ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "stream closed")
        })??;
        Ok(TokioIo::new(fuchsia_hyper::TcpStream { stream: conn }))
    }
}

pub trait OmahaServerExt {
    fn start_and_detach(
        arc_server: Arc<Mutex<OmahaServer>>,
        addr: Option<SocketAddr>,
    ) -> impl Future<Output = Result<String, Error>>;
}

impl OmahaServerExt for OmahaServer {
    async fn start_and_detach(
        arc_server: Arc<Mutex<OmahaServer>>,
        addr: Option<SocketAddr>,
    ) -> Result<String, Error> {
        let addr = addr.unwrap_or_else(|| SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0));
        let listener = FuchsiaListener::bind(&addr)?;
        let local_addr = listener.addr;
        fasync::Task::spawn(async move {
            let _ = OmahaServer::start(arc_server, listener, FuchsiaExecutor).await;
        })
        .detach();
        Ok(format!("http://{local_addr}/"))
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    mock_omaha_server::declare_tests! {
        test_attr: #[fasync::run_singlethreaded(test)],
        start_server: async |server| <OmahaServer as OmahaServerExt>::start_and_detach(server, None).await,
        new_http_client: async || fuchsia_hyper::new_client(),
        cup_expect_panic: "mock-omaha-server was configured to expect CUP, but we received a request without it.",
    }
}
