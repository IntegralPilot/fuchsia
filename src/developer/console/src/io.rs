// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::pin::pin;

use fidl::endpoints::Proxy as _;
use fidl_fuchsia_device as fdevice;
use fidl_fuchsia_hardware_pty as fpty;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_process as fprocess;
use fuchsia_async as fasync;
use futures::{AsyncReadExt as _, AsyncWriteExt as _, FutureExt as _};
use log::warn;
use thiserror::Error;

use crate::error::MissingFidlFieldError;
use crate::util::{self, ConnectToProtocolError};

#[derive(Default)]
pub struct IoHandles {
    pub stdin: Option<zx::NullableHandle>,
    pub stdout: Option<zx::NullableHandle>,
    pub stderr: Option<zx::NullableHandle>,
}

impl IoHandles {
    pub fn into_handle_infos(self) -> Vec<fprocess::HandleInfo> {
        let Self { stdin, stdout, stderr } = self;
        [stdin, stdout, stderr]
            .into_iter()
            .enumerate()
            .filter_map(|(index, handle)| {
                handle.map(|handle| fprocess::HandleInfo {
                    handle,
                    id: fuchsia_runtime::HandleInfo::new(
                        fuchsia_runtime::HandleType::FileDescriptor,
                        index.try_into().unwrap(),
                    )
                    .as_raw(),
                })
            })
            .collect()
    }

    pub async fn new_pty_from_socket(socket: zx::Socket) -> Result<Self, IoHandlesError> {
        let pty = util::connect_to_protocol::<fpty::DeviceMarker>()?;

        // Open a new controlling client and make it active.
        let (client, server_end) = fidl::endpoints::create_proxy::<fpty::DeviceMarker>();
        let status = pty.open_client(0, server_end).await?;
        zx::Status::ok(status).map_err(IoHandlesError::Pty)?;

        // Assume that the terminal is 1024 x 768. When using a socket, we cannot
        // find out the terminal dimensions.
        let status = pty.set_window_size(&fpty::WindowSize { width: 1024, height: 768 }).await?;

        zx::Status::ok(status).map_err(IoHandlesError::Pty)?;

        let fpty::DeviceDescribeResponse { event, .. } = pty.describe().await?;
        let eventpair = event.ok_or(MissingFidlFieldError("DeviceDescribeResponse.event"))?;
        let socket = fasync::Socket::from_socket(socket);

        let (stdout, server_end) = fidl::endpoints::create_endpoints();
        client.clone(server_end)?;
        let (stderr, server_end) = fidl::endpoints::create_endpoints();
        client.clone(server_end)?;

        let handles = Self {
            stdin: Some(client.into_channel().unwrap().into_zx_channel().into_handle()),
            stdout: Some(stdout.into_channel().into()),
            stderr: Some(stderr.into_channel().into()),
        };

        let scope = fasync::Scope::current();
        let guard = scope.active_guard().ok_or(IoHandlesError::ScopeCancelled)?;

        // Spawn detached workers task; the scope will take care of cancelling
        // them.
        let _join_handle: fasync::JoinHandle<()> = scope.spawn(async move {
            let (socket_reader, socket_writer) = socket.split();
            futures::future::try_join(
                pty_to_socket_worker(guard, &pty, &eventpair, socket_writer),
                socket_to_pty_worker(socket_reader, &pty, &eventpair),
            )
            .await
            .map(|((), ())| ())
            .unwrap_or_else(|e| warn!("error operating Pty <=> Socket worker: {e}"));
        });

        Ok(handles)
    }
}

async fn socket_to_pty_worker(
    mut socket: impl futures::AsyncRead + Unpin,
    pty: &fpty::DeviceProxy,
    eventpair: &zx::EventPair,
) -> Result<(), WorkerError> {
    let writable = zx::Signals::from_bits(fdevice::DeviceSignal::WRITABLE.bits()).unwrap();
    let hangup = zx::Signals::from_bits(fdevice::DeviceSignal::HANGUP.bits()).unwrap();
    let mut buf = vec![0u8; fio::MAX_BUF as usize];
    loop {
        let bytes_read = socket.read(&mut buf).await.map_err(WorkerError::SocketRead)?;
        if bytes_read == 0 {
            break Ok(());
        }
        let mut to_pty = &buf[..bytes_read];
        loop {
            match pty.write(to_pty).await?.map_err(zx::Status::err_from_raw) {
                Ok(wr) => {
                    let wr = usize::try_from(wr).unwrap();
                    if wr < to_pty.len() {
                        to_pty = &to_pty[wr..];
                        continue;
                    } else {
                        break;
                    }
                }
                Err(zx::Status::PEER_CLOSED) => {
                    return Ok(());
                }
                Err(zx::Status::SHOULD_WAIT) => {
                    let signals = fasync::OnSignals::new(eventpair, writable | hangup)
                        .await
                        .map_err(WorkerError::PtyWrite)?;
                    if signals.contains(hangup) {
                        return Ok(());
                    }
                }
                Err(e) => return Err(WorkerError::PtyWrite(e)),
            }
        }
    }
}

async fn pty_to_socket_worker(
    guard: fasync::scope::ScopeActiveGuard,
    pty: &fpty::DeviceProxy,
    eventpair: &zx::EventPair,
    mut socket: impl futures::AsyncWrite + Unpin,
) -> Result<(), WorkerError> {
    let readable = zx::Signals::from_bits(fdevice::DeviceSignal::READABLE.bits()).unwrap();
    let hangup = zx::Signals::from_bits(fdevice::DeviceSignal::HANGUP.bits()).unwrap();
    // Hold a guard to ensure we've drained the PTY into the socket when the
    // scope is cancelled.
    let mut on_cancel = pin!(guard.on_cancel().fuse());
    loop {
        match pty.read(fio::MAX_BUF).await?.map_err(zx::Status::err_from_raw) {
            Ok(bytes) => {
                if bytes.is_empty() {
                    return Ok(());
                }
                socket.write_all(&bytes).await.map_err(WorkerError::SocketWrite)?;
                socket.flush().await.map_err(WorkerError::SocketWrite)?;
            }
            Err(zx::Status::PEER_CLOSED) => {
                return Ok(());
            }
            Err(zx::Status::SHOULD_WAIT) => {
                let mut on_signals =
                    pin!(fasync::OnSignals::new(eventpair, readable | hangup).fuse());
                let signals = futures::select_biased! {
                    signals = on_signals => signals.map_err(WorkerError::PtyRead)?,
                    () = on_cancel => {
                        return Ok(());
                    }
                };
                if signals.contains(readable) {
                    continue;
                }
                // Hang up otherwise.
                return Ok(());
            }
            Err(e) => {
                return Err(WorkerError::PtyRead(e));
            }
        }
    }
}

#[derive(Error, Debug)]
enum WorkerError {
    #[error("failed to read from socket: {0}")]
    SocketRead(std::io::Error),
    #[error("failed to write to socket: {0}")]
    SocketWrite(std::io::Error),
    #[error("unexpected FIDL error: {0}")]
    Fidl(#[from] fidl::Error),
    #[error("failed to read from PTY: {0}")]
    PtyRead(zx::Status),
    #[error("failed to write to PTY: {0}")]
    PtyWrite(zx::Status),
}

#[derive(Error, Debug)]
pub enum IoHandlesError {
    #[error(transparent)]
    ConnectToProtocol(#[from] ConnectToProtocolError),
    #[error("unexpected FIDL error: {0}")]
    Fidl(#[from] fidl::Error),
    #[error("pty device error: {0}")]
    Pty(zx::Status),
    #[error(transparent)]
    MissingFidlField(#[from] MissingFidlFieldError),
    #[error("scope was cancelled during set up")]
    ScopeCancelled,
}
