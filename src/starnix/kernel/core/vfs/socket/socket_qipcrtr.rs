// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::task::{
    CurrentTask, EventHandler, SignalHandler, SignalHandlerInner, WaitCanceler, Waiter,
};
use crate::vfs::buffers::{AncillaryData, InputBuffer, MessageReadInfo, OutputBuffer};
use crate::vfs::socket::{
    SockOptValue, Socket, SocketAddress, SocketHandle, SocketMessageFlags, SocketOps, SocketPeer,
    SocketShutdownFlags, SocketType,
};
use anyhow::Context;
use fidl::endpoints::{SynchronousProxy, create_sync_proxy};
use fidl_fuchsia_hardware_qualcomm_router as fqrtr;
use starnix_logging::{log_warn, track_stub};
use starnix_sync::{LockDepGuard, LockDepMutex, MappedLockDepGuard, QipcrtrSocketInnerLock};
use starnix_uapi::errors::{Errno, from_status_like_fdio};
use starnix_uapi::vfs::FdEvents;
use starnix_uapi::{
    AF_QIPCRTR, SO_RCVBUF, SO_SNDBUF, SOL_SOCKET, errno, error, sockaddr_qrtr, socklen_t, ucred,
};
use zerocopy::{FromBytes, IntoBytes};

const QRTR_CLIENT_SERVICE_DIRECTORY: &str = "/svc/fuchsia.hardware.qualcomm.router.ClientService";
fn connect_to_connector() -> Result<fqrtr::QrtrConnectorSynchronousProxy, anyhow::Error> {
    let mut dir = std::fs::read_dir(QRTR_CLIENT_SERVICE_DIRECTORY)
        .context("Failed to read ClientService directory")?;
    let entry = dir
        .next()
        .ok_or_else(|| anyhow::format_err!("Missing ClientService instance"))?
        .context("Unable to read ClientService instance")?;
    let path = entry
        .path()
        .join("qrtr_connector")
        .into_os_string()
        .into_string()
        .map_err(|_| anyhow::format_err!("Failed to get qrtr_connector path"))?;

    let (client_end, server_end) = zx::Channel::create();
    fdio::service_connect(&path, server_end)?;
    Ok(fqrtr::QrtrConnectorSynchronousProxy::from_channel(client_end))
}

// From socket(7).
pub const SEND_BUF_MIN_SIZE: usize = 2048;
pub const SEND_BUF_MAX_SIZE: usize = 1 << 31;
pub const SEND_BUF_DEFAULT_SIZE: usize = 2048;

// From socket(7).
pub const RECV_BUF_MIN_SIZE: usize = 256;
pub const RECV_BUF_MAX_SIZE: usize = 1 << 31;
pub const RECV_BUF_DEFAULT_SIZE: usize = 256;

pub struct QipcrtrSocket {
    inner: LockDepMutex<Option<QipcrtrSocketInner>, QipcrtrSocketInnerLock>,
}

struct QipcrtrSocketInner {
    /// The proxy representing the socket in the QRTR driver.
    proxy: fqrtr::QrtrClientConnectionSynchronousProxy,

    /// The event pair representing the readable and writable signals.
    events: zx::EventPair,

    /// The peer for a connected socket, which is the default address to send messages to when no
    /// destination is given.
    peer: Option<sockaddr_qrtr>,

    /// The socket's send buffer size.
    ///
    /// This value is only used to serve getsockopt calls for `SO_SNDBUF`. It does not yet enforce
    /// a limit on the buffer size.
    /// TODO(https://fxbug.dev/478337980): Limit the size of the send buffer.
    send_buf_size: usize,

    /// The socket's receive buffer size.
    ///
    /// This value is only used to serve getsockopt calls for `SO_RCVBUF`. It does not yet enforce
    /// a limit on the buffer size.
    /// TODO(https://fxbug.dev/478337980): Limit the size of the receive buffer.
    recv_buf_size: usize,
}

impl QipcrtrSocket {
    pub fn new(_socket_type: SocketType) -> Self {
        Self { inner: Default::default() }
    }

    /// Locks and returns the inner state of the socket. If the socket is not connected to the
    /// driver, a connection will be established, binding to an ephemeral port number.
    fn connecting_lock(&self) -> Result<MappedLockDepGuard<'_, QipcrtrSocketInner>, Errno> {
        let mut inner = self.inner.lock();
        if inner.is_none() {
            *inner = Some(QipcrtrSocketInner::new(fqrtr::ConnectionOptions {
                blocking: Some(false),
                ..Default::default()
            })?);
        }
        Ok(LockDepGuard::map(inner, |inner| inner.as_mut().unwrap()))
    }

    fn close(&self) {
        *self.inner.lock() = None;
    }
}

impl QipcrtrSocketInner {
    fn new(options: fqrtr::ConnectionOptions) -> Result<Self, Errno> {
        let connector = connect_to_connector().map_err(|e| errno!(ENETUNREACH, e))?;

        let (client_end, server_end) = create_sync_proxy::<fqrtr::QrtrClientConnectionMarker>();
        connector
            .get_connection(&options, server_end, zx::MonotonicInstant::INFINITE)
            .map_err(|e| errno!(ENETUNREACH, e))?
            .map_err(qrtr_error_to_errno)?;

        let proxy = fqrtr::QrtrClientConnectionSynchronousProxy::new(client_end.into_channel());
        let events = proxy
            .get_signals(zx::MonotonicInstant::INFINITE)
            .map_err(|e| errno!(ENETUNREACH, e))?;

        Ok(Self {
            proxy,
            events,
            peer: None,
            send_buf_size: SEND_BUF_DEFAULT_SIZE,
            recv_buf_size: RECV_BUF_DEFAULT_SIZE,
        })
    }

    /// Returns the [`sockaddr_qrtr`] of this connection.
    fn bound_addr(&self) -> Result<sockaddr_qrtr, Errno> {
        let addr = sockaddr_qrtr {
            sq_family: AF_QIPCRTR,
            sq_node: self
                .proxy
                .get_node_id(zx::MonotonicInstant::INFINITE)
                .map_err(|e| errno!(EINVAL, e))?,
            sq_port: self
                .proxy
                .get_port_id(zx::MonotonicInstant::INFINITE)
                .map_err(|e| errno!(EINVAL, e))?,
            ..Default::default()
        };
        Ok(addr)
    }
}

impl Drop for QipcrtrSocketInner {
    fn drop(&mut self) {
        if let Err(e) = self.proxy.close_connection(zx::MonotonicInstant::INFINITE) {
            log_warn!("Failed to close QRTR connection: {e:?}");
        }
    }
}

impl SocketOps for QipcrtrSocket {
    fn connect(
        &self,
        _socket: &SocketHandle,
        _current_task: &CurrentTask,
        peer: SocketPeer,
    ) -> Result<(), Errno> {
        let peer = match peer {
            SocketPeer::Address(addr) => extract_qrtr_sockaddr(&addr)?,
            _ => {
                return error!(EINVAL);
            }
        };

        let mut inner = self.inner.lock();
        if inner.is_some() {
            return error!(EISCONN);
        }

        // Establish a connection without a specific port number. The driver will automatically
        // assign one, resulting in a bound socket.
        let mut new_inner = QipcrtrSocketInner::new(fqrtr::ConnectionOptions {
            blocking: Some(false),
            ..Default::default()
        })?;
        new_inner.peer = Some(peer);

        *inner = Some(new_inner);
        Ok(())
    }

    fn listen(&self, _socket: &Socket, _backlog: i32, _credentials: ucred) -> Result<(), Errno> {
        error!(ENOTSUP)
    }

    fn accept(&self, _socket: &Socket, _current_task: &CurrentTask) -> Result<SocketHandle, Errno> {
        error!(ENOTSUP)
    }

    fn bind(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
        socket_address: SocketAddress,
    ) -> Result<(), Errno> {
        let addr = extract_qrtr_sockaddr(&socket_address)?;

        let mut inner = self.inner.lock();
        if inner.is_some() {
            return error!(EINVAL);
        }

        // Establish a connection with the specified port number.
        *inner = Some(QipcrtrSocketInner::new(fqrtr::ConnectionOptions {
            blocking: Some(false),
            port: Some(addr.sq_port),
            ..Default::default()
        })?);

        Ok(())
    }

    fn read(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
        data: &mut dyn OutputBuffer,
        flags: SocketMessageFlags,
    ) -> Result<MessageReadInfo, Errno> {
        if flags.contains(SocketMessageFlags::PEEK) {
            track_stub!(
                TODO("https://fxbug.dev/388082019"),
                "SocketMessageFlags::PEEK is unsupported"
            );
            return error!(EINVAL);
        }

        let inner = self.connecting_lock()?;

        if flags.contains(SocketMessageFlags::DONTWAIT) {
            match inner.events.wait_one(
                zx::Signals::from_bits_truncate(fqrtr::SIGNAL_READABLE)
                    | zx::Signals::EVENTPAIR_PEER_CLOSED,
                zx::MonotonicInstant::INFINITE_PAST,
            ) {
                zx::WaitResult::Ok(_) => {}
                zx::WaitResult::TimedOut(_) | zx::WaitResult::Canceled(_) => return error!(EAGAIN),
                zx::WaitResult::Err(status) => return Err(from_status_like_fdio!(status)),
            }
        }

        let (src_node, src_port, src_data) = inner
            .proxy
            .read(zx::MonotonicInstant::INFINITE)
            .map_err(|e| errno!(ECONNRESET, e))?
            .map_err(qrtr_error_to_errno)?;

        let bytes_read = data.write(src_data.as_bytes())?;
        Ok(MessageReadInfo {
            bytes_read,
            message_length: src_data.len(),
            address: Some(pack_qrtr_sockaddr(src_node, src_port)),
            ..Default::default()
        })
    }

    fn write(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
        data: &mut dyn InputBuffer,
        dest_address: &mut Option<SocketAddress>,
        _ancillary_data: &mut Vec<AncillaryData>,
    ) -> Result<usize, Errno> {
        let inner = self.connecting_lock()?;

        // If no destination address is specified, send to the peer address, which is set if
        // connect() is called.
        let dest = match dest_address {
            Some(addr) => extract_qrtr_sockaddr(addr)?,
            None => inner.peer.ok_or_else(|| errno!(EDESTADDRREQ))?,
        };

        match inner.events.wait_one(
            zx::Signals::from_bits_truncate(fqrtr::SIGNAL_WRITABLE)
                | zx::Signals::EVENTPAIR_PEER_CLOSED,
            zx::MonotonicInstant::INFINITE_PAST,
        ) {
            zx::WaitResult::Ok(_) => {}
            zx::WaitResult::TimedOut(_) | zx::WaitResult::Canceled(_) => return error!(EAGAIN),
            zx::WaitResult::Err(status) => return Err(from_status_like_fdio!(status)),
        }

        let data_written = data.read_all()?;
        let _ = inner
            .proxy
            .write(
                dest.sq_node,
                dest.sq_port,
                data_written.as_ref(),
                zx::MonotonicInstant::INFINITE,
            )
            .map_err(|e| errno!(ECONNRESET, e))?
            .map_err(qrtr_error_to_errno)?;
        Ok(data_written.len())
    }

    fn wait_async(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> WaitCanceler {
        let Ok(inner) = self.connecting_lock() else {
            return WaitCanceler::new_noop();
        };
        let signal_handler = SignalHandler {
            inner: SignalHandlerInner::ZxHandle(qrtr_signals_to_fd_events),
            event_handler: handler,
            err_code: None,
        };
        let canceler = waiter
            .wake_on_zircon_signals(
                &inner.events,
                fd_events_to_qrtr_signals(events),
                signal_handler,
            )
            .unwrap();
        WaitCanceler::new_port(canceler)
    }

    fn query_events(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
    ) -> Result<FdEvents, Errno> {
        let inner = self.connecting_lock()?;
        let signals = inner
            .events
            .as_handle_ref()
            .wait_one(zx::Signals::all(), zx::MonotonicInstant::INFINITE_PAST)
            .map_err(|e| from_status_like_fdio!(e))?;
        Ok(qrtr_signals_to_fd_events(signals))
    }

    fn shutdown(&self, _socket: &Socket, _how: SocketShutdownFlags) -> Result<(), Errno> {
        self.close();
        Ok(())
    }

    fn close(&self, _current_task: &CurrentTask, _socket: &Socket) {
        self.close();
    }

    fn getsockname(&self, _socket: &Socket) -> Result<SocketAddress, Errno> {
        let name = self.connecting_lock()?.bound_addr()?;
        Ok(SocketAddress::Qipcrtr(name.as_bytes().to_vec()))
    }

    fn getpeername(&self, _socket: &Socket) -> Result<SocketAddress, Errno> {
        let peer = self.connecting_lock()?.peer.ok_or_else(|| errno!(ENOTCONN))?;
        Ok(SocketAddress::Qipcrtr(peer.as_bytes().to_vec()))
    }

    fn setsockopt(
        &self,
        _socket: &Socket,
        current_task: &CurrentTask,
        level: u32,
        optname: u32,
        optval: SockOptValue,
    ) -> Result<(), Errno> {
        let mut inner = self.connecting_lock()?;
        match level {
            SOL_SOCKET => match optname {
                SO_SNDBUF => {
                    let requested_capacity: socklen_t = optval.read(current_task)?;
                    // SO_SNDBUF doubles the requested capacity to leave space for bookkeeping.
                    // See https://man7.org/linux/man-pages/man7/socket.7.html
                    let capacity = usize::try_from(requested_capacity * 2).unwrap_or(usize::MAX);
                    // TODO(https://fxbug.dev/322907334): Clamp to `wmem_max`.
                    let capacity = capacity.clamp(SEND_BUF_MIN_SIZE, SEND_BUF_MAX_SIZE);
                    inner.send_buf_size = capacity;
                }
                SO_RCVBUF => {
                    let requested_capacity: socklen_t = optval.read(current_task)?;
                    // SO_RCVBUF doubles the requested capacity to leave space for bookkeeping.
                    // See https://man7.org/linux/man-pages/man7/socket.7.html
                    let capacity = usize::try_from(requested_capacity * 2).unwrap_or(usize::MAX);
                    // TODO(https://fxbug.dev/322906968): Clamp to `rmem_max`.
                    let capacity = capacity.clamp(RECV_BUF_MIN_SIZE, RECV_BUF_MAX_SIZE);
                    inner.recv_buf_size = capacity;
                }
                _ => return error!(ENOSYS),
            },
            _ => return error!(ENOSYS),
        }

        Ok(())
    }

    fn getsockopt(
        &self,
        _socket: &Socket,
        _current_task: &CurrentTask,
        level: u32,
        optname: u32,
        _optlen: u32,
    ) -> Result<Vec<u8>, Errno> {
        let inner = self.connecting_lock()?;
        Ok(match level {
            SOL_SOCKET => match optname {
                SO_SNDBUF => (inner.send_buf_size as socklen_t).to_ne_bytes().to_vec(),
                SO_RCVBUF => (inner.recv_buf_size as socklen_t).to_ne_bytes().to_vec(),
                _ => return error!(ENOSYS),
            },
            _ => vec![],
        })
    }
}

/// Returns the [`sockaddr_qrtr`] within a [`SocketAddress`] or `EINVAL` if the address is not a
/// QRTR address.
fn extract_qrtr_sockaddr(addr: &SocketAddress) -> Result<sockaddr_qrtr, Errno> {
    match addr {
        SocketAddress::Qipcrtr(bytes) => sockaddr_qrtr::read_from_prefix(bytes.as_bytes())
            .map(|(addr, _)| addr)
            .map_err(|e| errno!(EINVAL, e)),
        _ => error!(EINVAL),
    }
}

/// Returns the [`SocketAddress`] representing a given node and port number.
fn pack_qrtr_sockaddr(node: u32, port: u32) -> SocketAddress {
    let addr =
        sockaddr_qrtr { sq_family: AF_QIPCRTR, sq_node: node, sq_port: port, ..Default::default() };
    SocketAddress::Qipcrtr(addr.as_bytes().into())
}

/// Maps a [`fqrtr::Error`] to an [`Errno`]. This mapping is not one-to-one.
fn qrtr_error_to_errno(e: fqrtr::Error) -> Errno {
    match e {
        fqrtr::Error::InternalError => errno!(EINVAL),
        fqrtr::Error::AlreadyPending => errno!(EBUSY),
        fqrtr::Error::RemoteNodeUnavailable => errno!(ECONNRESET),
        fqrtr::Error::AlreadyBound => errno!(EADDRINUSE),
        fqrtr::Error::NotSupported => errno!(ENOTSUP),
        fqrtr::Error::WouldBlock => errno!(EAGAIN),
        fqrtr::Error::NoResources => errno!(ENOMEM),
        fqrtr::Error::InvalidArgs => errno!(EINVAL),
        _ => errno!(EINVAL),
    }
}

/// Maps [`FdEvents`] to [`zx::Signals`] for a QRTR connection.
fn fd_events_to_qrtr_signals(events: FdEvents) -> zx::Signals {
    let mut signals = zx::Signals::empty();
    if events.contains(FdEvents::POLLIN) {
        signals |= zx::Signals::from_bits_truncate(fqrtr::SIGNAL_READABLE);
    }
    if events.contains(FdEvents::POLLOUT) {
        signals |= zx::Signals::from_bits_truncate(fqrtr::SIGNAL_WRITABLE);
    }

    // Always wait for the peer to be closed, which can generate POLLHUP.
    signals |= zx::Signals::EVENTPAIR_PEER_CLOSED;
    signals
}

/// Maps [`zx::Signals`] to [`FdEvents`] for a QRTR connection.
fn qrtr_signals_to_fd_events(signals: zx::Signals) -> FdEvents {
    let mut events = FdEvents::empty();
    if signals.contains(zx::Signals::from_bits_truncate(fqrtr::SIGNAL_READABLE)) {
        events |= FdEvents::POLLIN;
    }
    if signals.contains(zx::Signals::from_bits_truncate(fqrtr::SIGNAL_WRITABLE)) {
        events |= FdEvents::POLLOUT;
    }
    if signals.contains(zx::Signals::EVENTPAIR_PEER_CLOSED) {
        events |= FdEvents::POLLHUP;
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::spawn_kernel_and_run;
    use crate::vfs::buffers::{VecInputBuffer, VecOutputBuffer};
    use crate::vfs::socket::{SocketDomain, SocketProtocol, SocketType};
    use fidl::endpoints::create_sync_proxy;
    use futures::StreamExt;

    /// Creates a `QipcrtrSocket` with a mock inner state.
    ///
    /// The mock state is connected to a `QrtrClientConnection` proxy, and the stream for that
    /// proxy is returned to allow the test to drive the mock FIDL behavior.
    fn mock_qipcrtr_socket()
    -> (QipcrtrSocket, fidl::endpoints::ServerEnd<fqrtr::QrtrClientConnectionMarker>) {
        let (proxy, server_end) = create_sync_proxy::<fqrtr::QrtrClientConnectionMarker>();

        // We need an event pair for the socket.
        let (events, _) = zx::EventPair::create();

        let inner = QipcrtrSocketInner {
            proxy,
            events,
            peer: None,
            send_buf_size: SEND_BUF_DEFAULT_SIZE,
            recv_buf_size: RECV_BUF_DEFAULT_SIZE,
        };

        (QipcrtrSocket { inner: Some(inner).into() }, server_end)
    }

    #[::fuchsia::test]
    async fn test_qipcrtr_socket_new() {
        spawn_kernel_and_run(async |current_task| {
            let _kernel = current_task.kernel();
            // This test just checks basic creation without panic, but for QIPCRTR it tries
            // to connect to the global service, which might fail in test env if not mocked
            // correctly or if we rely on real service. The existing test `test_qipcrtr_socket_new`
            // calls `Socket::new` which calls `QipcrtrSocket::new`.
            // `QipcrtrSocket::new` creates a None inner, so it doesn't connect yet.
            // Connection happens on first use or explicit connect.
            let _socket = Socket::new(
                &current_task,
                SocketDomain::Qipcrtr,
                SocketType::Datagram,
                SocketProtocol::default(),
                /* kernel_private = */ false,
            )
            .expect("Failed to create socket.");
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_qipcrtr_sockopt() {
        spawn_kernel_and_run(async |current_task| {
            let socket = mock_qipcrtr_socket();
            let socket_obj = Socket::new_with_ops_and_info(
                Box::new(socket.0),
                SocketDomain::Qipcrtr,
                SocketType::Datagram,
                SocketProtocol::default(),
            );
            let _server_end = socket.1;

            // Test SO_SNDBUF
            let sndbuf = socket_obj.getsockopt(&current_task, SOL_SOCKET, SO_SNDBUF, 4).unwrap();
            let sndbuf_val = u32::from_ne_bytes(sndbuf.as_slice().try_into().unwrap());
            assert_eq!(sndbuf_val, SEND_BUF_DEFAULT_SIZE as u32);

            let new_sndbuf: u32 = 4096;
            socket_obj
                .setsockopt(
                    &current_task,
                    SOL_SOCKET,
                    SO_SNDBUF,
                    SockOptValue::from(new_sndbuf.as_bytes().to_vec()),
                )
                .unwrap();

            let sndbuf = socket_obj.getsockopt(&current_task, SOL_SOCKET, SO_SNDBUF, 4).unwrap();
            let sndbuf_val = u32::from_ne_bytes(sndbuf.as_slice().try_into().unwrap());
            // Setsockopt doubles the value.
            assert_eq!(sndbuf_val, new_sndbuf * 2);

            // Test SO_RCVBUF
            let rcvbuf = socket_obj.getsockopt(&current_task, SOL_SOCKET, SO_RCVBUF, 4).unwrap();
            let rcvbuf_val = u32::from_ne_bytes(rcvbuf.as_slice().try_into().unwrap());
            assert_eq!(rcvbuf_val, RECV_BUF_DEFAULT_SIZE as u32);

            let new_rcvbuf: u32 = 1024;
            socket_obj
                .setsockopt(
                    &current_task,
                    SOL_SOCKET,
                    SO_RCVBUF,
                    SockOptValue::from(new_rcvbuf.as_bytes().to_vec()),
                )
                .unwrap();

            let rcvbuf = socket_obj.getsockopt(&current_task, SOL_SOCKET, SO_RCVBUF, 4).unwrap();
            let rcvbuf_val = u32::from_ne_bytes(rcvbuf.as_slice().try_into().unwrap());
            // Setsockopt doubles the value.
            assert_eq!(rcvbuf_val, new_rcvbuf * 2);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_qipcrtr_sockname() {
        let (socket_inner, server_end) = mock_qipcrtr_socket();
        // Handle get_node_id and get_port_id requests
        std::thread::spawn(move || {
            let mut executor = fuchsia_async::LocalExecutor::default();
            executor.run_singlethreaded(async move {
                let mut stream = server_end.into_stream();
                while let Some(Ok(request)) = stream.next().await {
                    match request {
                        fqrtr::QrtrClientConnectionRequest::GetNodeId { responder, .. } => {
                            let _ = responder.send(123).unwrap();
                        }
                        fqrtr::QrtrClientConnectionRequest::GetPortId { responder, .. } => {
                            let _ = responder.send(456).unwrap();
                        }
                        fqrtr::QrtrClientConnectionRequest::CloseConnection {
                            responder, ..
                        } => {
                            let _ = responder.send();
                        }
                        _ => panic!("Unexpected request: {:?}", request),
                    }
                }
            });
        });

        spawn_kernel_and_run(async |_current_task| {
            let socket_obj = Socket::new_with_ops_and_info(
                Box::new(socket_inner),
                SocketDomain::Qipcrtr,
                SocketType::Datagram,
                SocketProtocol::default(),
            );

            let addr = socket_obj.getsockname().unwrap();
            let qrtr_addr = extract_qrtr_sockaddr(&addr).unwrap();
            assert_eq!(qrtr_addr.sq_node, 123);
            assert_eq!(qrtr_addr.sq_port, 456);

            // Set peer
            let peer_addr = sockaddr_qrtr {
                sq_family: AF_QIPCRTR,
                sq_node: 10,
                sq_port: 20,
                ..Default::default()
            };
            socket_obj
                .downcast_socket::<QipcrtrSocket>()
                .unwrap()
                .inner
                .lock()
                .as_mut()
                .unwrap()
                .peer = Some(peer_addr);

            let peer = socket_obj.getpeername().unwrap();
            let peer_qrtr = extract_qrtr_sockaddr(&peer).unwrap();
            assert_eq!(peer_qrtr.sq_node, 10);
            assert_eq!(peer_qrtr.sq_port, 20);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_qipcrtr_read_write() {
        let (socket_inner, server_end) = mock_qipcrtr_socket();
        std::thread::spawn(move || {
            let mut executor = fuchsia_async::LocalExecutor::default();
            executor.run_singlethreaded(async move {
                let mut stream = server_end.into_stream();
                while let Some(Ok(request)) = stream.next().await {
                    match request {
                        fqrtr::QrtrClientConnectionRequest::Write {
                            dst_node_id,
                            dst_port,
                            data,
                            responder,
                            ..
                        } => {
                            assert_eq!(dst_node_id, 10);
                            assert_eq!(dst_port, 20);
                            assert_eq!(data, b"hello");
                            let _ = responder.send(Ok(())).unwrap();
                        }
                        fqrtr::QrtrClientConnectionRequest::Read { responder, .. } => {
                            let _ = responder.send(Ok((5, 15, b"world"))).unwrap();
                        }
                        fqrtr::QrtrClientConnectionRequest::CloseConnection {
                            responder, ..
                        } => {
                            let _ = responder.send();
                        }
                        _ => panic!("Unexpected request: {:?}", request),
                    }
                }
            });
        });

        spawn_kernel_and_run(async |current_task| {
            let socket_obj = Socket::new_with_ops_and_info(
                Box::new(socket_inner),
                SocketDomain::Qipcrtr,
                SocketType::Datagram,
                SocketProtocol::default(),
            );
            // Connect to set default peer
            let peer_addr = sockaddr_qrtr {
                sq_family: AF_QIPCRTR,
                sq_node: 10,
                sq_port: 20,
                ..Default::default()
            };
            socket_obj
                .downcast_socket::<QipcrtrSocket>()
                .unwrap()
                .inner
                .lock()
                .as_mut()
                .unwrap()
                .peer = Some(peer_addr);

            // Test Write
            let mut input = VecInputBuffer::new(b"hello");
            let written =
                socket_obj.write(&current_task, &mut input, &mut None, &mut vec![]).unwrap();
            assert_eq!(written, 5);

            // Test Read
            let mut output = VecOutputBuffer::new(100);
            let info =
                socket_obj.read(&current_task, &mut output, SocketMessageFlags::empty()).unwrap();
            assert_eq!(info.bytes_read, 5);
            assert_eq!(output.data(), b"world");

            let addr = extract_qrtr_sockaddr(&info.address.unwrap()).unwrap();
            assert_eq!(addr.sq_node, 5);
            assert_eq!(addr.sq_port, 15);
        })
        .await;
    }
}
