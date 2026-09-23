// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdomain_fuchsia_developer_remotecontrol::RemoteControlProxy;
use futures::{AsyncReadExt, AsyncWriteExt, StreamExt, TryStreamExt};
use netext::{TcpListenerStream, TokioAsyncReadExt};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;
use tokio::net::TcpListener;

use crate::Error;
use crate::socket_provider::{SocketProvider, TargetTcpListener, TargetTcpStream};

#[derive(Clone)]
pub struct PortForwarder {
    socket_provider: SocketProvider,
    counters: Arc<Counters<Counter>>,
}

impl PortForwarder {
    /// Creates a new [`PortForwarder`] with target connections via
    /// [`SocketProvider`].
    pub fn new(socket_provider: SocketProvider) -> Self {
        Self { socket_provider, counters: Arc::new(Default::default()) }
    }

    /// Creates a new [`PortForwarder`] fetching the FIDL proxies with
    /// `rcs_proxy`.
    pub async fn new_with_rcs(
        connect_timeout: Duration,
        rcs_proxy: &RemoteControlProxy,
    ) -> Result<Self, Error> {
        SocketProvider::new_with_rcs(connect_timeout, rcs_proxy).await.map(Self::new)
    }

    /// Returns the socket provider backing this `PortForwarder`.
    pub fn socket_provider(&self) -> &SocketProvider {
        &self.socket_provider
    }

    /// Reverse forwarding from target to host.
    ///
    /// Returns a future that proxies connections accepted from
    /// `target_listener` to new connections to `host_address`.
    pub fn reverse(
        &self,
        target_listener: TargetTcpListener,
        host_address: SocketAddr,
    ) -> impl Future<Output = Result<(), Error>> + 'static {
        let counters = self.counters.clone();
        target_listener.into_stream().try_for_each_concurrent(None, move |socket| {
            let counters = counters.clone();
            async move {
                let addr = socket.peer_addr();
                log::info!("Connection from {addr} reverse forwarding to {host_address}");
                let tcp_stream = match tokio::net::TcpStream::connect(&host_address).await {
                    Ok(stream) => stream,
                    Err(e) => {
                        log::error!("Could not connect to {:?}: {:?}", host_address, e);
                        return Ok(());
                    }
                };
                counters.active_connections.target_to_host.add(1);
                scopeguard::defer! {
                    counters.active_connections.target_to_host.subtract(1);
                }
                shuttle_bytes(tcp_stream, socket, &counters).await;
                log::info!("Reverse-forwarded connection from {addr} closed");
                Result::Ok(())
            }
        })
    }

    /// Direct forwarding from host to target.
    ///
    /// Returns a future that proxies connections accepted from `host_listener`
    /// to new connections to `target_address` on the target.
    pub fn forward(
        &self,
        host_listener: TcpListener,
        target_address: SocketAddr,
    ) -> impl Future<Output = Result<(), Error>> + 'static {
        let socket_provider = self.socket_provider.clone();
        let counters = self.counters.clone();
        TcpListenerStream(host_listener).map(Ok).try_for_each_concurrent(None, move |conn| {
            let socket_provider = socket_provider.clone();
            let counters = counters.clone();
            async move {
                let conn = conn.and_then(|c| {
                    let addr = c.peer_addr()?;
                    Ok((c, addr))
                });
                let (conn, addr) = match conn {
                    Ok(conn) => conn,
                    Err(e) => {
                        log::error!("Error accepting connection for TCP forwarding: {:?}", e);
                        return Ok(());
                    }
                };
                log::info!("Connection from {addr} forwarding to {target_address}");
                let socket = match socket_provider.connect(target_address).await {
                    Ok(socket) => socket,
                    Err(e) => {
                        log::error!("Error requesting port forward from RCS: {:?}", e);
                        return Ok(());
                    }
                };
                counters.active_connections.host_to_target.add(1);
                scopeguard::defer! {
                    counters.active_connections.host_to_target.subtract(1);
                }
                shuttle_bytes(conn, socket, &counters).await;
                log::info!("Forwarded connection from {addr} closed");
                Ok(())
            }
        })
    }

    /// Loads a snapshot of the connection counters started from this forwarder.
    pub fn read_counters(&self) -> Counters<usize> {
        self.counters.read()
    }
}

async fn shuttle_bytes(
    host: tokio::net::TcpStream,
    target: TargetTcpStream,
    counters: &Counters<Counter>,
) {
    let (mut host_read, mut host_write) = host.into_futures_stream().split();
    let (mut target_read, mut target_write) = target.split();

    let host_to_target = async move {
        let mut buf = vec![0; 4096];
        loop {
            let bytes = host_read.read(&mut buf).await?;
            if bytes == 0 {
                break Ok(());
            }
            target_write.write_all(&buf[..bytes]).await?;
            target_write.flush().await?;
            counters.total_bytes.host_to_target.add(bytes);
        }
    };
    let target_to_host = async move {
        let mut buf = vec![0; 4096];
        loop {
            let bytes = target_read.read(&mut buf).await?;
            if bytes == 0 {
                break Ok(()) as Result<(), std::io::Error>;
            }
            host_write.write_all(&buf[..bytes]).await?;
            host_write.flush().await?;
            counters.total_bytes.target_to_host.add(bytes);
        }
    };
    match futures::future::join(host_to_target, target_to_host).await {
        (Err(a), Err(b)) => {
            log::warn!("Port forward closed with errors:\n  {:?}\n  {:?}", a, b)
        }
        (Err(e), _) | (_, Err(e)) => {
            log::warn!("Port forward closed with error: {:?}", e)
        }
        (Ok(()), Ok(())) => (),
    }
}

/// Counters kept by [`PortForwarder`].
#[derive(Debug, Default, Eq, PartialEq, Copy, Clone)]
pub struct Counters<C> {
    /// Number of active connections.
    pub active_connections: Bidirectional<C>,
    /// Total number of bytes shuttled by direction.
    pub total_bytes: Bidirectional<C>,
}

impl Counters<Counter> {
    fn read(&self) -> Counters<usize> {
        let Self { active_connections, total_bytes } = self;
        Counters { active_connections: active_connections.read(), total_bytes: total_bytes.read() }
    }
}

/// Organizing type for splitting host-to-target and target-to-host values.
#[derive(Debug, Default, Eq, PartialEq, Copy, Clone)]
pub struct Bidirectional<C> {
    pub host_to_target: C,
    pub target_to_host: C,
}

impl Bidirectional<Counter> {
    fn read(&self) -> Bidirectional<usize> {
        let Self { host_to_target, target_to_host } = self;
        Bidirectional {
            host_to_target: host_to_target.read(),
            target_to_host: target_to_host.read(),
        }
    }
}

#[derive(Debug, Default)]
struct Counter(AtomicUsize);

impl Counter {
    fn add(&self, val: usize) {
        let _: usize = self.0.fetch_add(val, std::sync::atomic::Ordering::Relaxed);
    }

    fn subtract(&self, val: usize) {
        let _: usize = self.0.fetch_sub(val, std::sync::atomic::Ordering::Relaxed);
    }

    fn read(&self) -> usize {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }
}
