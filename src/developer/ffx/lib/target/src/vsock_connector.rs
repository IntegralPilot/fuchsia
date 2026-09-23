// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::Resolution;
use crate::target_connector::{
    BUFFER_SIZE, FDomainConnection, OvernetConnection, TargetConnection, TargetConnectionError,
    TargetConnector,
};
use anyhow::Result;
use ffx_config::{EnvironmentContext, TryFromEnvContext};
use futures::future::LocalBoxFuture;
use nix::errno::Errno;
use nix::sys::socket::sockopt::SocketError;
use nix::sys::socket::{
    AddressFamily, Shutdown, SockFlag, SockType, VsockAddr, connect, getsockopt, shutdown, socket,
};
use nix::unistd::{read, write};
use std::fmt::Debug;
use std::os::fd::{AsRawFd, OwnedFd};
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use thiserror::Error;
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncWrite, BufReader, Ready};

const OVERNET_VSOCK_PORT: u32 = 202;
const FDOMAIN_VSOCK_PORT: u32 = 203;

#[derive(Debug, Error)]
enum ConnectError {
    #[error("Could not create VSOCK socket for cid {0} port {1}: {2}")]
    CreateFailed(u32, u32, std::io::Error),
    #[error("Could not create Tokio AsyncFD for VSOCK socket (cid {0}, port {1}): {2}")]
    AsyncCreateFailed(u32, u32, std::io::Error),
    #[error("Could not connect VSOCK socket to cid {0} port {1}: {2}")]
    ConnectFailed(u32, u32, std::io::Error),
    #[error("Error waiting for VSOCK socket for cid {0} port {1} to become writable: {2}")]
    WritableError(u32, u32, std::io::Error),
    #[error("Could not verify connection for VSOCK socket for cid {0} port {1}: {2}")]
    VerifyError(u32, u32, std::io::Error),
    #[error("Could not complete connection for VSOCK socket for cid {0} port {1}: {2}")]
    CompletionError(u32, u32, std::io::Error),
}

pub struct VSockConnector {
    cid: u32,
}

impl Debug for VSockConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VSockConnector").field("cid", &self.cid).finish()
    }
}

impl VSockConnector {
    pub fn new(cid: u32) -> Self {
        Self { cid }
    }

    async fn do_connect(&self, port: u32) -> Result<AsyncVsockSocket, ConnectError> {
        let addr = VsockAddr::new(self.cid, port);
        let flags = SockFlag::SOCK_CLOEXEC | SockFlag::SOCK_NONBLOCK;

        let socket = match socket(AddressFamily::Vsock, SockType::Stream, flags, None) {
            Ok(s) => s,
            Err(error) => {
                return Err(ConnectError::CreateFailed(
                    self.cid,
                    port,
                    std::io::Error::from(error),
                ));
            }
        };

        let socket = match tokio::io::unix::AsyncFd::new(socket) {
            Ok(s) => s,
            Err(error) => {
                return Err(ConnectError::AsyncCreateFailed(self.cid, port, error));
            }
        };

        if let Err(error) = connect(socket.as_raw_fd(), &addr)
            && error != Errno::EINPROGRESS
        {
            return Err(ConnectError::ConnectFailed(self.cid, port, std::io::Error::from(error)));
        }

        // If we got EINPROGRESS above then the connection isn't actually
        // established and we need to block until it is.
        if let Err(error) = socket.writable().await {
            return Err(ConnectError::WritableError(self.cid, port, error));
        }

        let error = match getsockopt(&socket, SocketError) {
            Ok(e) => e,
            Err(error) => {
                return Err(ConnectError::VerifyError(self.cid, port, std::io::Error::from(error)));
            }
        };

        if error != 0 {
            return Err(ConnectError::CompletionError(
                self.cid,
                port,
                std::io::Error::from_raw_os_error(error),
            ));
        }

        Ok(AsyncVsockSocket(socket))
    }
}

impl VSockConnector {
    async fn connect_overnet(&mut self) -> Result<OvernetConnection, TargetConnectionError> {
        let conn = self.do_connect(OVERNET_VSOCK_PORT).await.map_err(|e| {
            TargetConnectionError::Fatal(anyhow::anyhow!("Connection error: {e:?}"))
        })?;
        let (output, input) = tokio::io::split(conn);
        let output = BufReader::with_capacity(BUFFER_SIZE, output);
        let (_sender, errors) = async_channel::unbounded();
        Ok(OvernetConnection {
            output: Box::new(output),
            input: Box::new(input),
            errors,
            main_task: None,
        })
    }

    async fn connect_fdomain(&mut self) -> Result<FDomainConnection, TargetConnectionError> {
        let conn = self.do_connect(FDOMAIN_VSOCK_PORT).await.map_err(|e| {
            TargetConnectionError::Fatal(anyhow::anyhow!("Connection error: {e:?}"))
        })?;
        let (output, input) = tokio::io::split(conn);
        let output = BufReader::with_capacity(BUFFER_SIZE, output);
        let (_sender, errors) = async_channel::unbounded();
        Ok(FDomainConnection {
            output: Box::new(output),
            input: Box::new(input),
            errors,
            main_task: None,
        })
    }
}

impl TryFromEnvContext for VSockConnector {
    fn try_from_env_context<'a>(
        env: &'a EnvironmentContext,
    ) -> LocalBoxFuture<'a, ffx_command_error::Result<Self>> {
        Box::pin(async {
            let resolution = Resolution::try_from_env_context(env).await?;
            let cid = resolution.vsock_cid().ok_or_else(|| {
                ffx_command_error::user_error!(
                    "query did not resolve a VSOCK CID. Resolved the following: {:?}",
                    resolution,
                )
            })?;
            Ok(VSockConnector::new(cid))
        })
    }
}

impl TargetConnector for VSockConnector {
    const CONNECTION_TYPE: &'static str = "VSOCK";

    async fn connect(&mut self) -> Result<TargetConnection, TargetConnectionError> {
        match self.connect_fdomain().await {
            Ok(fdomain) => Ok(TargetConnection::FDomain(fdomain)),
            Err(e) => {
                // Eventually we should just return the error here, making
                // FDomain authoritative about whether the device is
                // connectable. For now we'll fall through because it's less
                // likely to cause breakages prior to migration.
                log::warn!("Connecting with FDomain encountered error {e:?}");
                self.connect_overnet().await.map(TargetConnection::Overnet)
            }
        }
    }
}

struct AsyncVsockSocket(AsyncFd<OwnedFd>);

impl AsyncRead for AsyncVsockSocket {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let mut r = ready!(self.0.poll_read_ready(cx))?;

        match read(r.get_inner(), buf.initialize_unfilled()) {
            Ok(n) => {
                buf.advance(n);
                Poll::Ready(Ok(()))
            }
            Err(nix::Error::EAGAIN) => {
                r.clear_ready_matching(Ready::READABLE);
                Poll::Pending
            }
            Err(other) => Poll::Ready(Err(std::io::Error::from(other))),
        }
    }
}

impl AsyncWrite for AsyncVsockSocket {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let mut w = ready!(self.0.poll_write_ready(cx))?;

        match write(w.get_inner(), buf) {
            Ok(x) => Poll::Ready(Ok(x)),
            Err(nix::Error::EAGAIN) => {
                w.clear_ready_matching(Ready::WRITABLE);
                Poll::Pending
            }
            Err(other) => Poll::Ready(Err(std::io::Error::from(other))),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(shutdown(self.0.as_raw_fd(), Shutdown::Write).map_err(std::io::Error::from))
    }
}
