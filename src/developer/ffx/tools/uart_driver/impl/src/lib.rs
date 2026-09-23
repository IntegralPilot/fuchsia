// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Stream and framing primitives for the Fuchsia UART host driver.
//!
//! Provides packet framing via `uart_fpl::FrameParser` ([`UartReader`]), UNIX domain
//! socket binding with stale file and orphaned process cleanup, and channel event types.

use anyhow::{Context as _, Result, anyhow};
use ffx_tool_uart::DaemonMetrics;
use futures::channel::mpsc;
use futures::future::poll_fn;
use std::os::unix::fs::FileTypeExt as _;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt as _};
use tokio::net::{UnixListener, UnixStream};
use uart_driver_api::ConnectionMetadata;
use uart_fpl::{Frame, FrameParser, FrameType, encode_frame};

pub const DEFAULT_CLIENT_DATA_CAPACITY: usize = 64;
const UART_READ_BUFFER_SIZE: usize = 16 * 1024;
const TERMINATE_POLL_RETRIES: usize = 5;
const TERMINATE_POLL_INTERVAL: Duration = Duration::from_millis(100);

pub fn is_char_device(path: &str) -> bool {
    std::fs::metadata(path).map(|m| m.file_type().is_char_device()).unwrap_or(false)
}

pub use ffx_tool_uart::stream::{AsyncUart, UartStream, connect_uart_stream};

/// Errors that can occur when managing Unix domain socket lifecycle and stale socket cleanup.
#[derive(thiserror::Error, Debug)]
pub enum RemoveAndBindError {
    /// The target socket path already exists and an active daemon is actively listening.
    #[error("Socket {0} already exists and is in use")]
    InUse(PathBuf),
    /// A stale socket file was detected but could not be removed from the filesystem.
    #[error("Could not remove stale socket at {0}: {1}")]
    RemoveStale(PathBuf, #[source] std::io::Error),
    /// An unexpected I/O error occurred while probing whether a pre-existing socket is active.
    #[error("Unexpected error when checking for stale socket at {0}: {1}")]
    ConnectCheck(PathBuf, #[source] std::io::Error),
    /// Binding the Unix domain listener to the target path failed.
    #[error("Could not listen on socket at {0}: {1}")]
    Bind(PathBuf, #[source] std::io::Error),
}

async fn terminate_process(pid: u32) {
    if pid <= 1 || pid > i32::MAX as u32 {
        return;
    }
    // SAFETY: libc::kill is a standard POSIX system call to send SIGTERM for graceful termination.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGTERM);
    }
    for _ in 0..TERMINATE_POLL_RETRIES {
        tokio::time::sleep(TERMINATE_POLL_INTERVAL).await;
        if !ffx_tool_uart::is_running(pid) {
            log::info!("Orphaned daemon {} exited after SIGTERM.", pid);
            return;
        }
    }
    log::warn!("Orphaned daemon {} still running after SIGTERM. Sending SIGKILL...", pid);
    // SAFETY: libc::kill with SIGKILL forcefully terminates an unresponsive orphaned process.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
    }
}

async fn check_and_kill_orphaned_daemon(socket_path: &std::path::Path, verify_daemon: bool) {
    let json_path = socket_path.with_extension("json");
    if !json_path.exists() {
        return;
    }
    let content = match std::fs::read_to_string(&json_path) {
        Ok(c) => c,
        Err(_) => return,
    };
    let metadata: ConnectionMetadata = match serde_json::from_str(&content) {
        Ok(m) => m,
        Err(_) => return,
    };
    let old_pid = metadata.pid;
    if old_pid == 0 {
        return;
    }
    let should_kill = if verify_daemon {
        ffx_tool_uart::is_driver_running(old_pid)
    } else {
        ffx_tool_uart::is_running(old_pid)
    };
    if should_kill {
        log::warn!(
            "Detected orphaned daemon with PID {} for target '{}' (socket is dead). Killing it...",
            old_pid,
            metadata.target
        );
        terminate_process(old_pid).await;
    }
}

/// Binds a [`UnixListener`] to `socket_path`, safely detecting and cleaning up stale socket files.
///
/// Attempts to connect to `socket_path` to verify whether an active daemon is running:
/// * If connection succeeds, the socket is actively in use, returning [`RemoveAndBindError::InUse`].
/// * If connection is refused, the socket is stale. If an associated metadata file indicates
///   an orphaned process, terminates the orphan and unlinks the stale socket before binding.
///
/// # Arguments
///
/// * `socket_path` - Filesystem path where the UNIX domain socket should be bound.
/// * `verify_daemon` - If `true`, verifies that any process recorded in the metadata file is an
///   actual `ffx-uart-driver` process before terminating it.
///
/// # Returns
///
/// The newly bound [`UnixListener`].
///
/// # Errors
///
/// Returns [`RemoveAndBindError`] if the socket is in active use, if removing a stale socket fails,
/// or if binding the listener fails.
pub async fn remove_and_bind_socket(
    socket_path: PathBuf,
    verify_daemon: bool,
) -> Result<UnixListener, RemoveAndBindError> {
    match UnixStream::connect(&socket_path).await {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            check_and_kill_orphaned_daemon(&socket_path, verify_daemon).await;
        }
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
            check_and_kill_orphaned_daemon(&socket_path, verify_daemon).await;
            if let Err(e) = std::fs::remove_file(&socket_path) {
                return Err(RemoveAndBindError::RemoveStale(socket_path, e));
            }
        }
        Ok(_) => {
            return Err(RemoveAndBindError::InUse(socket_path));
        }
        Err(e) => {
            return Err(RemoveAndBindError::ConnectCheck(socket_path, e));
        }
    }

    match UnixListener::bind(&socket_path) {
        Ok(s) => Ok(s),
        Err(e) => Err(RemoveAndBindError::Bind(socket_path, e)),
    }
}

/// Events emitted by client channel readers/writers or the UNIX domain listener.
pub enum ClientEvent {
    /// A new client connected on `channel_id`.
    NewChannel { channel_id: u16, stream: UnixStream },
    /// Incoming data read from a client socket on `channel_id`.
    ChannelData { channel_id: u16, data: Vec<u8> },
    /// A client socket closed or encountered an EOF/error on `channel_id`.
    ChannelClose { channel_id: u16 },
}

/// Events emitted by the UART session reader or supervisor loop.
pub enum UartEvent {
    /// Decoded data payload received from the UART link for `channel_id`.
    UartData { channel_id: u16, data: Vec<u8> },
    /// Remote close frame received from the UART link for `channel_id`.
    UartClose { channel_id: u16 },
    /// A new UART session was established with an outgoing `sender_tx` queue.
    UpdateSender { sender_tx: mpsc::Sender<SenderMessage> },
    /// The active UART session disconnected or failed.
    UartDown,
}

/// Outgoing channel messages queued for serial transmission.
#[derive(Clone, Debug)]
pub enum SenderMessage {
    /// Data payload to frame and transmit on `channel_id`.
    Data { channel_id: u16, payload: Vec<u8> },
    /// Close notification frame to transmit on `channel_id`.
    Close { channel_id: u16 },
}

pub struct UartReader<R: AsyncRead + Unpin> {
    client: R,
    parser: FrameParser,
    buf: Box<[u8; UART_READ_BUFFER_SIZE]>,
}

impl<R: AsyncRead + Unpin> UartReader<R> {
    pub fn new(client: R) -> Self {
        Self::with_initial_data(client, Vec::new())
    }

    pub fn with_initial_data(client: R, initial_data: Vec<u8>) -> Self {
        let mut parser = FrameParser::new();
        if !initial_data.is_empty() {
            parser.feed(&initial_data);
        }
        Self { client, parser, buf: Box::new([0u8; UART_READ_BUFFER_SIZE]) }
    }

    async fn drain_available(&mut self) -> Result<()> {
        loop {
            let mut read_buf = tokio::io::ReadBuf::new(&mut self.buf[..]);
            let res = poll_fn(|cx| match Pin::new(&mut self.client).poll_read(cx, &mut read_buf) {
                Poll::Ready(Ok(())) => Poll::Ready(Some(read_buf.filled().len())),
                Poll::Ready(Err(_)) => Poll::Ready(None),
                Poll::Pending => Poll::Ready(None),
            })
            .await;

            match res {
                Some(n) if n > 0 => {
                    self.parser.feed(&self.buf[..n]);
                }
                _ => break,
            }
        }
        Ok(())
    }

    /// Reads from the underlying stream until a complete, checksum-verified [`Frame`] is decoded.
    pub async fn next_frame(&mut self, metrics: &Arc<Mutex<DaemonMetrics>>) -> Result<Frame> {
        loop {
            let _ = self.drain_available().await;
            if let Some(frame) = self.parser.next_frame() {
                let mut m = metrics.lock().unwrap();
                m.checksum_errors = self.parser.checksum_errors();
                return Ok(frame);
            }
            {
                let mut m = metrics.lock().unwrap();
                m.checksum_errors = self.parser.checksum_errors();
            }
            let n =
                self.client.read(&mut self.buf[..]).await.context("Failed to read from UART")?;
            if n == 0 {
                return Err(anyhow!("EOF from UART"));
            }
            {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                let mut m = metrics.lock().unwrap();
                m.last_read_timestamp_ms = now;
            }
            self.parser.feed(&self.buf[..n]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt as _;

    #[fuchsia::test]
    async fn test_uart_reader_drain_and_frame() {
        let (mut client, server) = tokio::io::duplex(1024);
        let mut reader = UartReader::new(server);
        let metrics = Arc::new(Mutex::new(DaemonMetrics::default()));

        let frame_bytes = encode_frame(123, 1, 0, FrameType::Data, b"hello uart").unwrap();
        client.write_all(&frame_bytes).await.unwrap();

        let frame = reader.next_frame(&metrics).await.unwrap();
        assert_eq!(frame.session_id, 123);
        assert_eq!(frame.channel_id, 1);
        assert_eq!(frame.seq, 0);
        assert_eq!(frame.payload, b"hello uart");
    }

    #[fuchsia::test]
    async fn test_remove_and_bind_socket() {
        let temp = tempfile::tempdir().unwrap();
        let sock_path = temp.path().join("test.sock");

        let listener = remove_and_bind_socket(sock_path.clone(), false).await.unwrap();
        drop(listener);

        // Bind again to verify stale socket removal
        let listener2 = remove_and_bind_socket(sock_path, false).await.unwrap();
        drop(listener2);
    }
}
