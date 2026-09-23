// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Asynchronous stream abstractions and connection helpers for UART devices.
//!
//! Provides [`UartStream`], an asynchronous duplex stream that unifies communication
//! across physical serial character devices (wrapped by [`AsyncUart`]) and UNIX domain
//! sockets. Also includes routines for initiating protocol negotiation handshakes with
//! remote targets.

use nix::libc;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd, RawFd};
use std::os::unix::fs::FileTypeExt;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf, Ready};
use uart_driver_api::ConnectionError;
use uart_fpl::{
    CONTROL_CHANNEL_ID, FrameParser, FrameType, HostHandshake, ProtocolId, encode_frame,
};

const SOCKET_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Errors encountered during UART protocol negotiation handshake.
#[derive(Debug, thiserror::Error)]
pub enum HandshakeError {
    /// The remote peer or daemon closed the connection before completing the handshake.
    #[error("Connection closed by peer")]
    ConnectionClosed,

    /// An underlying I/O error occurred during handshake transmission or reception.
    #[error("I/O error during handshake: {0}")]
    Io(#[from] std::io::Error),

    /// A protocol framing error occurred while encoding handshake frames.
    #[error("Framing error during handshake: {0}")]
    Framing(#[from] uart_fpl::FrameError),

    /// Target rejected the handshake or returned an invalid negotiation response.
    #[error("Negotiation error: {0}")]
    Negotiation(#[from] uart_fpl::HandshakeError),

    /// The handshake timed out without receiving a response after the configured attempts.
    #[error("Handshake timed out after {attempts} attempts")]
    Timeout {
        /// Number of retry attempts made.
        attempts: usize,
    },
}

/// An asynchronous duplex byte stream connected to a UART target or driver daemon.
///
/// Unifies I/O handling over physical serial character devices and local UNIX
/// domain sockets, implementing [`tokio::io::AsyncRead`] and [`tokio::io::AsyncWrite`].
pub enum UartStream {
    /// A UNIX domain socket connection, used for communication with virtual targets
    /// or intermediate daemon proxies.
    Socket(tokio::net::UnixStream),
    /// An asynchronous physical serial character device managed via [`AsyncUart`].
    Tty(AsyncUart),
}

impl AsyncRead for UartStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Socket(s) => Pin::new(s).poll_read(cx, buf),
            Self::Tty(t) => Pin::new(t).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for UartStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Self::Socket(s) => Pin::new(s).poll_write(cx, buf),
            Self::Tty(t) => Pin::new(t).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Socket(s) => Pin::new(s).poll_flush(cx),
            Self::Tty(t) => Pin::new(t).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Socket(s) => Pin::new(s).poll_shutdown(cx),
            Self::Tty(t) => Pin::new(t).poll_shutdown(cx),
        }
    }
}

/// An asynchronous, non-blocking wrapper around a serial character device file descriptor.
///
/// Integrates a physical TTY descriptor with Tokio's event loop via [`tokio::io::unix::AsyncFd`],
/// handling edge-triggered epoll readiness notifications and managing non-blocking read/write
/// operations without blocking runtime worker threads.
pub struct AsyncUart(tokio::io::unix::AsyncFd<OwnedFd>);

impl AsyncRead for AsyncUart {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        loop {
            let mut r = ready!(self.0.poll_read_ready(cx))?;

            match nix::unistd::read(r.get_inner(), buf.initialize_unfilled()) {
                Ok(n) => {
                    buf.advance(n);
                    return Poll::Ready(Ok(()));
                }
                Err(nix::Error::EINTR) => continue,
                Err(nix::Error::EAGAIN) => {
                    r.clear_ready_matching(Ready::READABLE);
                }
                Err(other) => return Poll::Ready(Err(std::io::Error::from(other))),
            }
        }
    }
}

impl AsyncWrite for AsyncUart {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        loop {
            let mut w = ready!(self.0.poll_write_ready(cx))?;

            match nix::unistd::write(w.get_inner(), buf) {
                Ok(x) => return Poll::Ready(Ok(x)),
                Err(nix::Error::EINTR) => continue,
                Err(nix::Error::EAGAIN) => {
                    w.clear_ready_matching(Ready::WRITABLE);
                }
                Err(other) => return Poll::Ready(Err(std::io::Error::from(other))),
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// Asynchronously opens and configures a UART connection at the given path.
///
/// If `path` refers to a UNIX domain socket, establishes a stream connection to the socket.
/// If `path` refers to a character device, acquires an exclusive advisory file lock (`flock`),
/// configures the serial terminal for raw mode at the specified `baud` rate, and wraps the
/// descriptor in a non-blocking [`AsyncUart`].
pub async fn connect_uart_stream(path: &str, baud: u32) -> Result<UartStream, ConnectionError> {
    // Acts as the entrypoint for target streams; this will be expanded in
    // downstream changes to dispatch across additional stream types (such as TCP).
    connect_uart_stream_file(path, baud).await
}

async fn connect_unix_socket(
    path: &std::path::Path,
) -> Result<tokio::net::UnixStream, ConnectionError> {
    let path_str = path.to_string_lossy().into_owned();
    log::info!("Connecting to UART UNIX socket at {path_str}");
    let stream_res =
        tokio::time::timeout(SOCKET_CONNECT_TIMEOUT, tokio::net::UnixStream::connect(path)).await;
    match stream_res {
        Ok(Ok(s)) => Ok(s),
        Ok(Err(e)) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                Err(ConnectionError::PathNotFound { path: path_str })
            } else {
                Err(ConnectionError::UnixConnectFailed { path: path_str, error: e.to_string() })
            }
        }
        Err(_) => Err(ConnectionError::UnixConnectFailed {
            path: path_str,
            error: "Connection timed out".to_string(),
        }),
    }
}

async fn open_and_lock_tty(path: &std::path::Path) -> Result<std::fs::File, ConnectionError> {
    let path_str = path.to_string_lossy().into_owned();
    let tokio_file = tokio::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags((nix::fcntl::OFlag::O_NONBLOCK | nix::fcntl::OFlag::O_NOCTTY).bits())
        .open(path)
        .await
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => ConnectionError::PathNotFound { path: path_str },
            std::io::ErrorKind::PermissionDenied => {
                ConnectionError::PermissionDenied { path: path_str, error: e.to_string() }
            }
            _ => ConnectionError::TtyConfigureFailed {
                error: format!("Failed to open TTY {path_str}: {e}"),
            },
        })?;

    let file = tokio_file.into_std().await;
    // SAFETY: libc::flock is a standard POSIX advisory locking syscall on a valid open descriptor.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } < 0 {
        let err = std::io::Error::last_os_error();
        return Err(ConnectionError::TtyConfigureFailed {
            error: format!("Port is already in use by another driver instance: {err}"),
        });
    }

    Ok(file)
}

#[cfg(target_os = "linux")]
use linux as platform;

#[cfg(not(target_os = "linux"))]
use non_linux as platform;

#[cfg(target_os = "linux")]
mod linux {
    use super::*;

    #[repr(C)]
    struct SerialStruct {
        type_: libc::c_int,
        line: libc::c_int,
        port: libc::c_uint,
        irq: libc::c_int,
        flags: libc::c_int,
        xmit_fifo_size: libc::c_int,
        custom_divisor: libc::c_int,
        baud_base: libc::c_int,
        close_delay: libc::c_ushort,
        io_type: libc::c_char,
        reserved_char: [libc::c_char; 1],
        hub6: libc::c_int,
        closing_wait: libc::c_ushort,
        closing_wait2: libc::c_ushort,
        iomem_base: *mut libc::c_uchar,
        iomem_reg_shift: libc::c_ushort,
        port_high: libc::c_uint,
        iomap_base: libc::c_ulong,
        reserved: [libc::c_int; 4],
    }

    const ASYNC_LOW_LATENCY: libc::c_int = 0x2000;

    pub fn configure_tty(fd: BorrowedFd<'_>, baud: u32) -> Result<(), ConnectionError> {
        let raw_fd = fd.as_raw_fd();
        // SAFETY: isatty is a standard POSIX query syscall on a valid open descriptor guaranteed by BorrowedFd.
        if unsafe { libc::isatty(raw_fd) } == 0 {
            return Err(ConnectionError::TtyConfigureFailed {
                error: format!("Target path is not a TTY: {}", std::io::Error::last_os_error()),
            });
        }

        set_termios2_raw(raw_fd, baud)?;
        enable_low_latency_mode(raw_fd);

        // Discard any accumulated bootloader garbage/noise
        // SAFETY: tcflush is a standard libc function on a valid open descriptor guaranteed by BorrowedFd.
        if unsafe { libc::tcflush(raw_fd, libc::TCIOFLUSH) } < 0 {
            log::warn!("tcflush failed: {}", std::io::Error::last_os_error());
        }

        Ok(())
    }

    fn set_termios2_raw(fd: RawFd, baud: u32) -> Result<(), ConnectionError> {
        // SAFETY: std::mem::zeroed is safe to initialize a termios2 struct since it contains only primitive integers/arrays.
        let mut term: libc::termios2 = unsafe { std::mem::zeroed() };

        // SAFETY: libc::ioctl TCGETS2 is a standard tty query operation. We pass a valid raw fd and a mutable reference to term.
        if unsafe { libc::ioctl(fd, libc::TCGETS2, &mut term) } < 0 {
            return Err(ConnectionError::TtyConfigureFailed {
                error: format!("TCGETS2 failed: {}", std::io::Error::last_os_error()),
            });
        }

        term.c_iflag = 0;
        term.c_oflag = 0;
        term.c_lflag = 0;
        term.c_cflag &= !(libc::CSIZE | libc::PARENB | libc::CSTOPB | libc::CRTSCTS | libc::CBAUD);
        term.c_cflag |= libc::CS8 | libc::CREAD | libc::CLOCAL | libc::BOTHER;
        // With O_NONBLOCK, VMIN=1 ensures read() returns EAGAIN instead of 0 (EOF) when empty.
        term.c_cc[libc::VMIN] = 1;
        term.c_cc[libc::VTIME] = 0;
        term.c_ispeed = baud;
        term.c_ospeed = baud;

        // SAFETY: libc::ioctl TCSETS2 is a standard tty configuration operation. We pass a valid raw fd and a read-only reference to term.
        if unsafe { libc::ioctl(fd, libc::TCSETS2, &term) } < 0 {
            return Err(ConnectionError::TtyConfigureFailed {
                error: format!("TCSETS2 failed: {}", std::io::Error::last_os_error()),
            });
        }

        Ok(())
    }

    fn enable_low_latency_mode(fd: RawFd) {
        // SAFETY: std::mem::zeroed is safe to initialize SerialStruct since all fields are primitive integers, arrays, or pointers.
        let mut ser: SerialStruct = unsafe { std::mem::zeroed() };
        // SAFETY: TIOCGSERIAL is a standard Linux ioctl to query serial port parameters. fd is a valid open descriptor and &mut ser points to valid memory.
        if unsafe { libc::ioctl(fd, libc::TIOCGSERIAL, &mut ser) } == 0 {
            ser.flags |= ASYNC_LOW_LATENCY;
            // SAFETY: TIOCSSERIAL is a standard Linux ioctl to set serial port parameters. fd is a valid open descriptor and &ser is a valid reference.
            if unsafe { libc::ioctl(fd, libc::TIOCSSERIAL, &ser) } < 0 {
                log::warn!(
                    "TIOCSSERIAL ASYNC_LOW_LATENCY failed: {}",
                    std::io::Error::last_os_error()
                );
            } else {
                log::info!("Configured ASYNC_LOW_LATENCY on TTY");
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod non_linux {
    use super::*;

    pub fn configure_tty(_fd: BorrowedFd<'_>, _baud: u32) -> Result<(), ConnectionError> {
        Err(ConnectionError::TtyConfigureFailed {
            error: "Physical TTY configuration is only supported on Linux".to_string(),
        })
    }
}

async fn connect_uart_stream_file(path: &str, baud: u32) -> Result<UartStream, ConnectionError> {
    let p = std::path::Path::new(path);
    let metadata = tokio::fs::metadata(p).await.map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => ConnectionError::PathNotFound { path: path.to_string() },
        _ => ConnectionError::PermissionDenied { path: path.to_string(), error: e.to_string() },
    })?;

    if metadata.file_type().is_socket() {
        let stream = connect_unix_socket(p).await?;
        Ok(UartStream::Socket(stream))
    } else {
        log::info!("Opening UART TTY at {} with baud rate {}", path, baud);
        let file = open_and_lock_tty(p).await?;
        platform::configure_tty(file.as_fd(), baud)?;
        let owned_fd = OwnedFd::from(file);
        let async_fd = tokio::io::unix::AsyncFd::new(owned_fd).map_err(|e| {
            ConnectionError::TtyConfigureFailed { error: format!("Failed to create AsyncFd: {e}") }
        })?;

        Ok(UartStream::Tty(AsyncUart(async_fd)))
    }
}

async fn read_handshake_response<S: AsyncRead + Unpin>(
    stream: &mut S,
    host: &uart_fpl::HostHandshake,
    parser: &mut uart_fpl::FrameParser,
    timeout: std::time::Duration,
) -> Result<Option<(ProtocolId, Vec<u8>)>, HandshakeError> {
    let mut buf = [0u8; 1024];
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, stream.read(&mut buf)).await {
            Ok(Ok(0)) => return Err(HandshakeError::ConnectionClosed),
            Ok(Ok(n)) => {
                parser.feed(&buf[..n]);
                while let Some(frame) = parser.next_frame() {
                    if frame.channel_id == CONTROL_CHANNEL_ID
                        && frame.frame_type == FrameType::NegotiateResp
                    {
                        let protocol = host.handle_response(&frame.payload)?;
                        log::info!("Handshake succeeded! Negotiated protocol: {:?}", protocol);
                        return Ok(Some((protocol, parser.take_unconsumed())));
                    }
                }
            }
            Ok(Err(e)) => return Err(HandshakeError::Io(e)),
            Err(_) => break,
        }
    }
    Ok(None)
}

/// Executes the host-side protocol negotiation handshake over a duplex stream.
pub async fn run_host_handshake<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    protocols: Vec<ProtocolId>,
    timeout: std::time::Duration,
    retries: usize,
) -> Result<(ProtocolId, u32, Vec<u8>), HandshakeError> {
    let host = HostHandshake::new(protocols);
    let (req_type, req_payload) = host.start()?;
    let session_id = rand::random::<u32>().max(1);

    let req_frame = encode_frame(session_id, CONTROL_CHANNEL_ID, 0, req_type, &req_payload)?;
    let mut parser = FrameParser::new();

    for attempt in 0..retries {
        log::info!("Sending handshake request (attempt {})...", attempt + 1);
        stream.write_all(&req_frame).await?;

        if let Some((protocol, unconsumed)) =
            read_handshake_response(stream, &host, &mut parser, timeout).await?
        {
            return Ok((protocol, session_id, unconsumed));
        }
    }

    Err(HandshakeError::Timeout { attempts: retries })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use uart_fpl::{FrameParser, FrameType, TargetHandshake, encode_frame};

    #[fuchsia::test]
    async fn test_connect_uart_stream_nonexistent_path() {
        let res = connect_uart_stream("/tmp/nonexistent_uart_test_path_12345", 115200).await;
        assert!(matches!(res, Err(ConnectionError::PathNotFound { .. })));
    }

    #[fuchsia::test]
    async fn test_async_uart_read_write() {
        let (s1, s2) = std::os::unix::net::UnixStream::pair().unwrap();
        s1.set_nonblocking(true).unwrap();
        s2.set_nonblocking(true).unwrap();

        let mut uart1 = AsyncUart(tokio::io::unix::AsyncFd::new(OwnedFd::from(s1)).unwrap());
        let mut uart2 = AsyncUart(tokio::io::unix::AsyncFd::new(OwnedFd::from(s2)).unwrap());

        uart1.write_all(b"hello uart").await.unwrap();
        let mut buf = [0u8; 10];
        uart2.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello uart");
    }

    fn spawn_mock_handshake_server(listener: tokio::net::UnixListener) -> fuchsia_async::Task<()> {
        fuchsia_async::Task::local(async move {
            if let Ok((mut server_stream, _)) = listener.accept().await {
                let mut parser = FrameParser::new();
                let mut buf = [0u8; 1024];
                let target = TargetHandshake::new(vec![ProtocolId::ResendSP]);
                while let Ok(n) = server_stream.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    parser.feed(&buf[..n]);
                    if let Some(req) = parser.next_frame() {
                        let (_, payload) = target.handle_request(&req.payload).unwrap();
                        let resp =
                            encode_frame(req.session_id, 0, 0, FrameType::NegotiateResp, &payload)
                                .unwrap();
                        let _ = server_stream.write_all(&resp).await;
                        return;
                    }
                }
            }
        })
    }

    #[fuchsia::test]
    async fn test_connect_uart_stream_socket_and_handshake() {
        let temp = tempdir().unwrap();
        let sock_path = temp.path().join("uart_test.sock");
        let sock_path_str = sock_path.to_str().unwrap().to_string();
        let listener = tokio::net::UnixListener::bind(&sock_path).unwrap();
        let server_task = spawn_mock_handshake_server(listener);

        let mut client = connect_uart_stream(&sock_path_str, 115200)
            .await
            .expect("connect to socket should succeed");

        let (protocol, session_id, _unconsumed) = run_host_handshake(
            &mut client,
            vec![ProtocolId::ResendSP],
            std::time::Duration::from_secs(2),
            2,
        )
        .await
        .expect("handshake should succeed");

        assert_eq!(protocol, ProtocolId::ResendSP);
        assert!(session_id > 0);
        server_task.await;
    }
}
