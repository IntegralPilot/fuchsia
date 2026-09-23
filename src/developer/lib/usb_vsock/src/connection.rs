// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use futures::channel::{mpsc, oneshot};
use futures::lock::{Mutex, OwnedMutexGuard};
use log::{debug, trace, warn};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::future::Future;
use std::io::{Error, ErrorKind};
use std::ops::DerefMut;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker, ready};

use fuchsia_async::Scope;
use futures::io::{ReadHalf, WriteHalf};
use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, FutureExt, SinkExt, StreamExt};

use crate::connection::overflow_writer::OverflowHandleFut;
use crate::{
    Address, Header, Packet, PacketType, ProtocolVersion, ShutdownError, UsbPacketBuilder,
    UsbPacketFiller, WritePacketErrorExt,
};

mod overflow_writer;
mod pause_state;

use overflow_writer::OverflowWriter;
use pause_state::PauseState;

/// A marker trait for types that are capable of being used as buffers for a [`Connection`].
pub trait PacketBuffer: DerefMut<Target = [u8]> + Send + Unpin + 'static {}
impl<T> PacketBuffer for T where T: DerefMut<Target = [u8]> + Send + Unpin + 'static {}

#[derive(Copy, Clone, PartialEq, Eq)]
enum PausePacket {
    Pause,
    UnPause,
}

impl PausePacket {
    fn bytes(&self) -> [u8; 1] {
        match self {
            PausePacket::Pause => [1],
            PausePacket::UnPause => [0],
        }
    }
}

/// A connection that has been established with the other end and now just needs
/// a socket to start transmitting.
pub struct ReadyConnect<B, S> {
    connections: Arc<fuchsia_sync::Mutex<HashMap<Address, VsockConnection<S>>>>,
    packet_filler: Arc<UsbPacketFiller<B>>,
    address: Address,
}

impl<B: PacketBuffer, S: AsyncRead + AsyncWrite + Send + 'static> ReadyConnect<B, S> {
    /// Finish establishing the connection by providing a socket for data transfer.
    pub async fn finish_connect(self, socket: S) {
        let (read_socket, write_socket) = socket.split();
        let writer = {
            let conns = self.connections.lock();
            let Some(conn) = conns.get(&self.address) else {
                warn!("Connection state was missing after connection success!");
                return;
            };
            let VsockConnectionState::Connected { writer, reader_scope, pause_state, .. } =
                &conn.state
            else {
                warn!("Connection state was invalid after connection success!");
                return;
            };
            reader_scope.spawn(Connection::<B, S>::run_socket(
                read_socket,
                self.address,
                self.packet_filler,
                Arc::clone(pause_state),
            ));
            Arc::clone(writer)
        };
        let mut writer = writer.lock().await;
        let ConnectionStateWriter::NotYetAvailable(wakers) = std::mem::replace(
            &mut *writer,
            ConnectionStateWriter::Available(OverflowWriter::new(write_socket)),
        ) else {
            unreachable!("Connection completed multiple times!")
        };

        wakers.into_iter().for_each(Waker::wake);
    }
}

/// Manages the state of a vsock-over-usb connection and the sockets over which data is being
/// transmitted for them.
///
/// This implementation aims to be agnostic to both the underlying transport and the buffers used
/// to read and write from it. The buffer type must conform to [`PacketBuffer`], which is essentially
/// a type that holds a mutable slice of bytes and is [`Send`] and [`Unpin`]-able.
///
/// The client of this library will:
/// - Use methods on this struct to initiate actions like connecting and accepting
/// connections to the other end.
/// - Provide buffers to be filled and sent to the other end with [`Connection::fill_usb_packet`].
/// - Pump usb packets received into it using [`Connection::handle_vsock_packet`].
pub struct Connection<B, S> {
    control_socket_writer: Option<Mutex<WriteHalf<S>>>,
    packet_filler: Arc<UsbPacketFiller<B>>,
    protocol_version: ProtocolVersion,
    connections: Arc<fuchsia_sync::Mutex<HashMap<Address, VsockConnection<S>>>>,
    incoming_requests_tx: mpsc::Sender<ConnectionRequest>,
    task_scope: Scope,
}

impl<B: PacketBuffer, S: AsyncRead + AsyncWrite + Send + 'static> Connection<B, S> {
    /// Creates a new connection with:
    /// - a `control_socket`, over which data addressed to and from cid 0, port 0 (a control channel
    /// between host and device) can be read and written from. If this is `None`
    /// we will discard control data.
    /// - An `incoming_requests_tx` that is the sender half of a request queue for incoming
    /// connection requests from the other side.
    pub fn new(
        protocol_version: ProtocolVersion,
        control_socket: Option<S>,
        incoming_requests_tx: mpsc::Sender<ConnectionRequest>,
    ) -> Self {
        let packet_filler = Arc::new(UsbPacketFiller::default());
        let connections = Default::default();
        let task_scope = Scope::new_with_name("vsock_usb");
        let control_socket_writer = control_socket.map(|control_socket| {
            let (control_socket_reader, control_socket_writer) = control_socket.split();
            task_scope.spawn(Self::run_socket(
                control_socket_reader,
                Address::default(),
                packet_filler.clone(),
                PauseState::new(),
            ));
            Mutex::new(control_socket_writer)
        });
        Self {
            control_socket_writer,
            packet_filler,
            connections,
            incoming_requests_tx,
            protocol_version,
            task_scope,
        }
    }

    async fn send_close_packet(address: &Address, usb_packet_filler: &Arc<UsbPacketFiller<B>>) {
        let header = &mut Header::new(PacketType::Finish);
        header.set_address(address);
        let _: Result<_, ShutdownError> = usb_packet_filler
            .write_vsock_packet(&Packet { header, payload: &[] })
            .await
            .expect_right_size("Finish packet should never be too big");
    }

    async fn run_socket(
        mut reader: ReadHalf<S>,
        address: Address,
        usb_packet_filler: Arc<UsbPacketFiller<B>>,
        pause_state: Arc<PauseState>,
    ) {
        let mut buf = [0; 4096];
        loop {
            log::trace!("reading from control socket");
            let read = match pause_state.while_unpaused(reader.read(&mut buf)).await {
                Ok(0) => {
                    if !address.is_zeros() {
                        Self::send_close_packet(&address, &usb_packet_filler).await;
                    }
                    return;
                }
                Ok(read) => read,
                Err(err) => {
                    if address.is_zeros() {
                        log::error!("Error reading usb socket: {err:?}");
                    } else {
                        Self::send_close_packet(&address, &usb_packet_filler).await;
                    }
                    return;
                }
            };
            log::trace!("writing {read} bytes to vsock packet");
            if usb_packet_filler.write_vsock_data_all(&address, &buf[..read]).await.is_err() {
                log::trace!("transport shut down during read");
                return;
            }
            log::trace!("wrote {read} bytes to vsock packet");
        }
    }

    fn set_connection(
        &self,
        address: Address,
        state: VsockConnectionState<S>,
    ) -> Result<(), Error> {
        let mut connections = self.connections.lock();
        if !connections.contains_key(&address) {
            connections.insert(address.clone(), VsockConnection { _address: address, state });
            Ok(())
        } else {
            Err(Error::other(format!("connection on address {address:?} already set")))
        }
    }

    /// Sends an echo packet to the remote end that you don't care about the reply, so it doesn't
    /// have a distinct target address or payload.
    pub async fn send_empty_echo(&self) {
        debug!("Sending empty echo packet");
        let header = &mut Header::new(PacketType::Echo);
        let _: Result<_, ShutdownError> = self
            .packet_filler
            .write_vsock_packet(&Packet { header, payload: &[] })
            .await
            .expect_right_size(
                "empty echo packet should never be too large to fit in a usb packet",
            );
    }

    /// Starts a connection attempt to the other end of the USB connection, and provides a socket
    /// to read and write from. The function will complete when the other end has accepted or
    /// rejected the connection, and the returned [`ConnectionState`] handle can be used to wait
    /// for the connection to be closed.
    pub async fn connect(&self, addr: Address, socket: S) -> Result<ConnectionState, Error> {
        let (ready, state) = self.connect_late(addr).await?;
        ready.finish_connect(socket).await;
        Ok(state)
    }

    /// Same as [`connect`] but doesn't require the socket to be passed. Instead
    /// we return a [`ReadyConnect`] which can be given the socket later. This
    /// shouldn't be deferred very long but it is useful if the socket is
    /// starting out speaking a different protocol and needs to execute a
    /// protocol switch, but needs to know the connection status before doing
    /// that switch.
    pub async fn connect_late(
        &self,
        addr: Address,
    ) -> Result<(ReadyConnect<B, S>, ConnectionState), Error> {
        let (connected_tx, connected_rx) = oneshot::channel();

        self.set_connection(addr.clone(), VsockConnectionState::ConnectingOutgoing(connected_tx))?;

        let header = &mut Header::new(PacketType::Connect);
        header.set_address(&addr);
        self.packet_filler
            .write_vsock_packet(&Packet { header, payload: &[] })
            .await
            .assert_right_size()?;
        let Ok(conn_state) = connected_rx.await else {
            return Err(Error::other("Accept was never received for {addr:?}"));
        };

        Ok((
            ReadyConnect {
                connections: Arc::clone(&self.connections),
                packet_filler: Arc::clone(&self.packet_filler),
                address: addr,
            },
            conn_state,
        ))
    }

    /// Sends a request for the other end to close the connection.
    pub async fn close(&self, address: &Address) {
        Self::send_close_packet(address, &self.packet_filler).await
    }

    /// Resets the named connection without going through a close request.
    pub async fn reset(&self, address: &Address) -> Result<(), Error> {
        reset(address, &self.connections, &self.packet_filler).await
    }

    /// Accepts a connection for which an outstanding connection request has been made, and
    /// provides a socket to read and write data packets to and from. The returned [`ConnectionState`]
    /// can be used to wait for the connection to be closed.
    pub async fn accept(
        &self,
        request: ConnectionRequest,
        socket: S,
    ) -> Result<ConnectionState, Error> {
        let (ready, state) = self.accept_late(request).await?;
        ready.finish_connect(socket).await;
        Ok(state)
    }

    /// Accepts a connection for which an outstanding connection request has been made, and
    /// provides a socket to read and write data packets to and from. The returned [`ConnectionState`]
    /// can be used to wait for the connection to be closed.
    pub async fn accept_late(
        &self,
        request: ConnectionRequest,
    ) -> Result<(ReadyConnect<B, S>, ConnectionState), Error> {
        let address = request.address;
        let notify_closed_rx;
        if let Some(conn) = self.connections.lock().get_mut(&address) {
            let VsockConnectionState::ConnectingIncoming = &conn.state else {
                return Err(Error::other(format!(
                    "Attempted to accept connection that was not waiting at {address:?}"
                )));
            };

            let notify_closed = mpsc::channel(2);
            notify_closed_rx = notify_closed.1;
            let notify_closed = notify_closed.0;
            let pause_state = PauseState::new();

            let reader_scope = Scope::new_with_name("connection-reader");

            conn.state = VsockConnectionState::Connected {
                writer: Arc::new(Mutex::new(ConnectionStateWriter::NotYetAvailable(Vec::new()))),
                reader_scope,
                notify_closed,
                pause_state,
            };
        } else {
            return Err(Error::other(format!(
                "Attempting to accept connection that did not exist at {address:?}"
            )));
        }
        let header = &mut Header::new(PacketType::Accept);
        header.set_address(&address);
        self.packet_filler
            .write_vsock_packet(&Packet { header, payload: &[] })
            .await
            .assert_right_size()?;
        Ok((
            ReadyConnect {
                connections: Arc::clone(&self.connections),
                packet_filler: Arc::clone(&self.packet_filler),
                address,
            },
            ConnectionState(notify_closed_rx),
        ))
    }

    /// Rejects a pending connection request from the other side.
    pub async fn reject(&self, request: ConnectionRequest) -> Result<(), Error> {
        let address = request.address;
        match self.connections.lock().entry(address.clone()) {
            Entry::Occupied(entry) => {
                let VsockConnectionState::ConnectingIncoming = &entry.get().state else {
                    return Err(Error::other(format!(
                        "Attempted to reject connection that was not waiting at {address:?}"
                    )));
                };
                entry.remove();
            }
            Entry::Vacant(_) => {
                return Err(Error::other(format!(
                    "Attempted to reject connection that was not waiting at {address:?}"
                )));
            }
        }

        let header = &mut Header::new(PacketType::Reset);
        header.set_address(&address);
        self.packet_filler
            .write_vsock_packet(&Packet { header, payload: &[] })
            .await
            .expect_right_size("accept packet should never be too large for packet buffer")?;
        Ok(())
    }

    async fn handle_data_packet(&self, address: Address, payload: &[u8]) -> Result<(), Error> {
        // all zero data packets go to the control channel
        if address.is_zeros() {
            if let Some(writer) = self.control_socket_writer.as_ref() {
                writer.lock().await.write_all(payload).await?;
            } else {
                trace!("Discarding {} bytes of data sent to control socket", payload.len());
            }
            Ok(())
        } else {
            let payload_socket;
            if let Some(conn) = self.connections.lock().get_mut(&address) {
                let VsockConnectionState::Connected { writer, .. } = &conn.state else {
                    warn!(
                        "Received data packet for connection in unexpected state for {address:?}"
                    );
                    return Ok(());
                };
                payload_socket = writer.clone();
            } else {
                warn!("Received data packet for connection that didn't exist at {address:?}");
                return Ok(());
            }
            let mut socket_guard =
                ConnectionStateWriter::wait_available(Arc::clone(&payload_socket)).await;
            let ConnectionStateWriter::Available(socket) = &mut *socket_guard else {
                unreachable!("wait_available didn't wait until socket was available!");
            };
            match socket.write_all(payload) {
                Err(err) => {
                    debug!(
                        "Write to socket address {address:?} failed, \
                         resetting connection immediately: {err:?}"
                    );
                    self.reset(&address)
                        .await
                        .inspect_err(|err| {
                            warn!(
                                "Attempt to reset connection to {address:?} \
                                   failed after write error: {err:?}"
                            );
                        })
                        .ok();
                }
                Ok(status) => {
                    if status.overflowed() {
                        if self.protocol_version.has_pause_packets() {
                            let header = &mut Header::new(PacketType::Pause);
                            let payload = &PausePacket::Pause.bytes();
                            header.set_address(&address);
                            header.payload_len.set(payload.len() as u32);
                            self.packet_filler
                                .write_vsock_packet(&Packet { header, payload })
                                .await
                                .expect_right_size(
                                    "pause packet should never be too large to fit in a usb packet",
                                )?;
                        }

                        let weak_payload_socket = Arc::downgrade(&payload_socket);
                        let connections = Arc::clone(&self.connections);
                        let has_pause_packets = self.protocol_version.has_pause_packets();
                        let packet_filler = Arc::clone(&self.packet_filler);
                        self.task_scope.spawn(async move {
                            let res = OverflowHandleFut::new(weak_payload_socket).await;

                            if let Err(err) = res {
                                debug!(
                                    "Write to socket address {address:?} failed while \
                                     processing backlog, resetting connection at next poll: {err:?}"
                                );
                                if let Err(err) = reset(&address, &connections, &packet_filler).await {
                                    debug!("Error sending reset frame after overflow write failed: {err:?}");
                                }
                            } else if has_pause_packets {
                                let header = &mut Header::new(PacketType::Pause);
                                let payload = &PausePacket::UnPause.bytes();
                                header.set_address(&address);
                                header.payload_len.set(payload.len() as u32);
                                let _: Result<_, ShutdownError> =
                                packet_filler
                                    .write_vsock_packet(&Packet { header, payload })
                                    .await
                                    .expect_right_size("pause packet should never be too large to fit in a usb packet");
                            }
                        });
                    }
                }
            }
            Ok(())
        }
    }

    async fn handle_echo_packet(&self, address: Address, payload: &[u8]) -> Result<(), Error> {
        debug!("received echo for {address:?} with payload {payload:?}");
        let header = &mut Header::new(PacketType::EchoReply);
        header.payload_len.set(payload.len() as u32);
        header.set_address(&address);
        self.packet_filler.write_vsock_packet(&Packet { header, payload }).await.map_err(
            |e| match e {
                crate::WritePacketError::PacketTooBig(_) => {
                    Error::other("Echo packet was too large to be sent back")
                }
                crate::WritePacketError::Shutdown(shutdown_error) => shutdown_error.into(),
            },
        )
    }

    async fn handle_echo_reply_packet(
        &self,
        address: Address,
        payload: &[u8],
    ) -> Result<(), Error> {
        // ignore but log replies
        debug!("received echo reply for {address:?} with payload {payload:?}");
        Ok(())
    }

    async fn handle_accept_packet(&self, address: Address) -> Result<(), Error> {
        if let Some(conn) = self.connections.lock().get_mut(&address) {
            let state = std::mem::replace(&mut conn.state, VsockConnectionState::Invalid);
            let VsockConnectionState::ConnectingOutgoing(connected_tx) = state else {
                warn!("Received accept packet for connection in unexpected state for {address:?}");
                return Ok(());
            };
            let (notify_closed, notify_closed_rx) = mpsc::channel(2);
            if connected_tx.send(ConnectionState(notify_closed_rx)).is_err() {
                warn!(
                    "Accept packet received for {address:?} but connect caller stopped waiting for it"
                );
            }
            let pause_state = PauseState::new();

            let reader_scope = Scope::new_with_name("connection-reader");
            conn.state = VsockConnectionState::Connected {
                writer: Arc::new(Mutex::new(ConnectionStateWriter::NotYetAvailable(Vec::new()))),
                reader_scope,
                notify_closed,
                pause_state,
            };
        } else {
            warn!("Got accept packet for connection that was not being made at {address:?}");
            return Ok(());
        }
        Ok(())
    }

    async fn handle_connect_packet(&self, address: Address) -> Result<(), Error> {
        trace!("received connect packet for {address:?}");
        match self.connections.lock().entry(address.clone()) {
            Entry::Vacant(entry) => {
                debug!("valid connect request for {address:?}");
                entry.insert(VsockConnection {
                    _address: address,
                    state: VsockConnectionState::ConnectingIncoming,
                });
            }
            Entry::Occupied(_) => {
                warn!(
                    "Received connect packet for already existing \
                     connection for address {address:?}. Ignoring"
                );
                return Ok(());
            }
        }

        trace!("sending incoming connection request to client for {address:?}");
        let connection_request = ConnectionRequest { address };
        self.incoming_requests_tx
            .clone()
            .send(connection_request)
            .await
            .inspect(|_| trace!("sent incoming request for {address:?}"))
            .map_err(|_| Error::other("Failed to send connection request"))
    }

    async fn handle_finish_packet(&self, address: Address) -> Result<(), Error> {
        trace!("received finish packet for {address:?}");
        let mut notify;
        if let Some(conn) = self.connections.lock().remove(&address) {
            let VsockConnectionState::Connected { notify_closed, .. } = conn.state else {
                warn!(
                    "Received finish (close) packet for {address:?} \
                     which was not in a connected state. Ignoring and dropping connection state."
                );
                return Ok(());
            };
            notify = notify_closed;
        } else {
            warn!(
                "Received finish (close) packet for connection that didn't exist \
                 on address {address:?}. Ignoring"
            );
            return Ok(());
        }

        notify.send(Ok(())).await.ok();

        let header = &mut Header::new(PacketType::Reset);
        header.set_address(&address);
        self.packet_filler
            .write_vsock_packet(&Packet { header, payload: &[] })
            .await
            .expect_right_size("accept packet should never be too large for packet buffer")?;
        Ok(())
    }

    async fn handle_reset_packet(&self, address: Address) -> Result<(), Error> {
        trace!("received reset packet for {address:?}");
        let mut notify = None;
        if let Some(conn) = self.connections.lock().remove(&address) {
            if let VsockConnectionState::Connected { notify_closed, .. } = conn.state {
                notify = Some(notify_closed);
            } else {
                debug!(
                    "Received reset packet for connection that wasn't in a connecting or \
                    disconnected state on address {address:?}."
                );
            }
        } else {
            trace!(
                "Received reset packet for connection that didn't \
                exist on address {address:?}. Ignoring"
            );
        }

        if let Some(mut notify) = notify {
            notify.send(Ok(())).await.ok();
        }
        Ok(())
    }

    async fn handle_pause_packet(&self, address: Address, payload: &[u8]) -> Result<(), Error> {
        if !self.protocol_version.has_pause_packets() {
            warn!(
                "Got a pause packet while using protocol \
                 version {} which does not support them. Ignoring",
                self.protocol_version
            );
            return Ok(());
        }

        let pause = match payload {
            [1] => true,
            [0] => false,
            other => {
                warn!("Ignoring unexpected pause packet payload {other:?}");
                return Ok(());
            }
        };

        if let Some(conn) = self.connections.lock().get(&address) {
            if let VsockConnectionState::Connected { pause_state, .. } = &conn.state {
                pause_state.set_paused(pause);
            } else {
                warn!("Received pause packet for unestablished connection. Ignoring");
            };
        } else {
            warn!(
                "Received pause packet for connection that didn't exist on address {address:?}. Ignoring"
            );
        }

        Ok(())
    }

    /// Dispatches the given vsock packet type and handles its effect on any outstanding connections
    /// or the overall state of the connection.
    pub async fn handle_vsock_packet(&self, packet: Packet<'_>) -> Result<(), Error> {
        trace!("received vsock packet {header:?}", header = packet.header);
        let payload_len = packet.header.payload_len.get() as usize;
        let payload = &packet.payload[..payload_len];
        let address = Address::from(packet.header);
        match packet.header.packet_type {
            PacketType::Sync => Err(Error::other("Received sync packet mid-stream")),
            PacketType::Data => self.handle_data_packet(address, payload).await,
            PacketType::Accept => self.handle_accept_packet(address).await,
            PacketType::Connect => self.handle_connect_packet(address).await,
            PacketType::Finish => self.handle_finish_packet(address).await,
            PacketType::Reset => self.handle_reset_packet(address).await,
            PacketType::Echo => self.handle_echo_packet(address, payload).await,
            PacketType::EchoReply => self.handle_echo_reply_packet(address, payload).await,
            PacketType::Pause => self.handle_pause_packet(address, payload).await,
        }
    }

    /// Provides a packet builder for the state machine to write packets to. Returns a future that
    /// will be fulfilled when there is data available to send on the packet.
    ///
    /// # Panics
    ///
    /// Panics if called while another [`Self::fill_usb_packet`] future is pending.
    pub async fn fill_usb_packet(
        &self,
        builder: UsbPacketBuilder<B>,
    ) -> Result<UsbPacketBuilder<B>, ShutdownError> {
        self.packet_filler.fill_usb_packet(builder).await
    }
}

impl<B: PacketBuffer, S> Connection<B, S> {
    /// Inform this connection that whatever transport was providing and
    /// receiving packets has hung up and no more data will be written or read.
    pub fn shutdown(&self) {
        self.packet_filler.shutdown();
        self.connections.lock().clear();
    }
}

async fn reset<B: PacketBuffer, S: AsyncRead + AsyncWrite + Send + 'static>(
    address: &Address,
    connections: &fuchsia_sync::Mutex<HashMap<Address, VsockConnection<S>>>,
    packet_filler: &UsbPacketFiller<B>,
) -> Result<(), Error> {
    let mut notify = None;
    if let Some(conn) = connections.lock().remove(&address) {
        if let VsockConnectionState::Connected { notify_closed, .. } = conn.state {
            notify = Some(notify_closed);
        }
    } else {
        return Err(Error::other(
            "Client asked to reset connection {address:?} that did not exist",
        ));
    }

    if let Some(mut notify) = notify {
        notify.send(Err(ErrorKind::ConnectionReset.into())).await.ok();
    }

    let header = &mut Header::new(PacketType::Reset);
    header.set_address(address);
    packet_filler
        .write_vsock_packet(&Packet { header, payload: &[] })
        .await
        .expect_right_size("Reset packet should never be too big")?;
    Ok(())
}

/// A writer inside of a [`ConnectionState`]. This is essentially an
/// option-monad around an [`OverflowWriter`], but unlike
/// [`std::option::Option`] the empty variant stores wakers that by convention
/// will be woken when we replace it with the occupied variant.
enum ConnectionStateWriter<S> {
    NotYetAvailable(Vec<Waker>),
    Available(OverflowWriter<S>),
}

impl<S> ConnectionStateWriter<S> {
    /// Wait for the given `ConnectionStateWriter` to contain an actual writer.
    fn wait_available(this: Arc<Mutex<ConnectionStateWriter<S>>>) -> ConnectionStateWriterFut<S> {
        ConnectionStateWriterFut { writer: this, lock_fut: None }
    }
}

/// Future returned by [`ConnectionStateWriter::wait_available`].
struct ConnectionStateWriterFut<S> {
    writer: Arc<Mutex<ConnectionStateWriter<S>>>,
    lock_fut: Option<futures::lock::OwnedMutexLockFuture<ConnectionStateWriter<S>>>,
}

impl<S> Future for ConnectionStateWriterFut<S> {
    type Output = OwnedMutexGuard<ConnectionStateWriter<S>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let writer = Arc::clone(&self.writer);
        let lock_fut = self.lock_fut.get_or_insert_with(|| writer.lock_owned());
        let mut lock = ready!(lock_fut.poll_unpin(cx));
        self.lock_fut = None;
        match &mut *lock {
            ConnectionStateWriter::Available(_) => Poll::Ready(lock),
            ConnectionStateWriter::NotYetAvailable(queue) => {
                queue.push(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

enum VsockConnectionState<S> {
    ConnectingOutgoing(oneshot::Sender<ConnectionState>),
    ConnectingIncoming,
    Connected {
        writer: Arc<Mutex<ConnectionStateWriter<S>>>,
        notify_closed: mpsc::Sender<Result<(), Error>>,
        pause_state: Arc<PauseState>,
        reader_scope: Scope,
    },
    Invalid,
}

struct VsockConnection<S> {
    _address: Address,
    state: VsockConnectionState<S>,
}

/// A handle for the state of a connection established with either [`Connection::connect`] or
/// [`Connection::accept`]. Use this to get notified when the connection has been closed without
/// needing to hold on to the Socket end.
#[derive(Debug)]
pub struct ConnectionState(mpsc::Receiver<Result<(), Error>>);

impl ConnectionState {
    /// Wait for this connection to close. Returns Ok(()) if the connection was closed without error,
    /// and an error if it closed because of an error.
    pub async fn wait_for_close(mut self) -> Result<(), Error> {
        self.0
            .next()
            .await
            .ok_or_else(|| Error::other("Connection state's other end was dropped"))?
    }
}

/// An outstanding connection request that needs to be either [`Connection::accept`]ed or
/// [`Connection::reject`]ed.
#[derive(Debug)]
pub struct ConnectionRequest {
    address: Address,
}

impl ConnectionRequest {
    /// Creates a new connection request for the given address.
    pub fn new(address: Address) -> Self {
        Self { address }
    }

    /// The address this connection request is being made for.
    pub fn address(&self) -> &Address {
        &self.address
    }
}

#[cfg(test)]
mod test {
    use std::sync::Arc;
    use test_case::test_case;

    use crate::VsockPacketIterator;

    use super::*;

    #[cfg(not(target_os = "fuchsia"))]
    use fuchsia_async::emulated_handle::Socket as SyncSocket;
    use fuchsia_async::{Socket, Task};
    use futures::StreamExt;
    #[cfg(target_os = "fuchsia")]
    use zx::Socket as SyncSocket;

    async fn usb_echo_server(echo_connection: Arc<Connection<Vec<u8>, Socket>>) {
        let mut builder = UsbPacketBuilder::new(vec![0; 128]);
        loop {
            println!("waiting for usb packet");
            builder = echo_connection.fill_usb_packet(builder).await.unwrap();
            let packets = VsockPacketIterator::new(builder.take_usb_packet().unwrap());
            println!("got usb packet, echoing it back to the other side");
            let mut packet_count = 0;
            for packet in packets {
                let packet = packet.unwrap();
                match packet.header.packet_type {
                    PacketType::Connect => {
                        // respond with an accept packet
                        let mut reply_header = packet.header.clone();
                        reply_header.packet_type = PacketType::Accept;
                        echo_connection
                            .handle_vsock_packet(Packet { header: &reply_header, payload: &[] })
                            .await
                            .unwrap();
                    }
                    PacketType::Accept => {
                        // just ignore it
                    }
                    _ => echo_connection.handle_vsock_packet(packet).await.unwrap(),
                }
                packet_count += 1;
            }
            println!("handled {packet_count} packets");
        }
    }

    #[fuchsia::test]
    async fn data_over_control_socket() {
        let (socket, other_socket) = SyncSocket::create_stream();
        let (incoming_requests_tx, _incoming_requests) = mpsc::channel(5);
        let mut socket = Socket::from_socket(socket);
        let connection = Arc::new(Connection::new(
            ProtocolVersion::LATEST,
            Some(Socket::from_socket(other_socket)),
            incoming_requests_tx,
        ));

        let echo_task = Task::spawn(usb_echo_server(connection.clone()));

        for size in [1u8, 2, 8, 16, 32, 64, 128, 255] {
            println!("round tripping packet of size {size}");
            socket.write_all(&vec![size; size as usize]).await.unwrap();
            let mut buf = vec![0u8; size as usize];
            socket.read_exact(&mut buf).await.unwrap();
            assert_eq!(buf, vec![size; size as usize]);
        }
        echo_task.abort().await;
    }

    #[fuchsia::test]
    async fn data_over_normal_outgoing_socket() {
        let (_control_socket, other_socket) = SyncSocket::create_stream();
        let (incoming_requests_tx, _incoming_requests) = mpsc::channel(5);
        let connection = Arc::new(Connection::new(
            ProtocolVersion::LATEST,
            Some(Socket::from_socket(other_socket)),
            incoming_requests_tx,
        ));

        let echo_task = Task::spawn(usb_echo_server(connection.clone()));

        let (socket, other_socket) = SyncSocket::create_stream();
        let mut socket = Socket::from_socket(socket);
        connection
            .connect(
                Address { device_cid: 1, host_cid: 2, device_port: 3, host_port: 4 },
                Socket::from_socket(other_socket),
            )
            .await
            .unwrap();

        for size in [1u8, 2, 8, 16, 32, 64, 128, 255] {
            println!("round tripping packet of size {size}");
            socket.write_all(&vec![size; size as usize]).await.unwrap();
            let mut buf = vec![0u8; size as usize];
            socket.read_exact(&mut buf).await.unwrap();
            assert_eq!(buf, vec![size; size as usize]);
        }
        echo_task.abort().await;
    }

    #[fuchsia::test]
    async fn data_over_normal_incoming_socket() {
        let (_control_socket, other_socket) = SyncSocket::create_stream();
        let (incoming_requests_tx, mut incoming_requests) = mpsc::channel(5);
        let connection = Arc::new(Connection::new(
            ProtocolVersion::LATEST,
            Some(Socket::from_socket(other_socket)),
            incoming_requests_tx,
        ));

        let echo_task = Task::spawn(usb_echo_server(connection.clone()));

        let header = &mut Header::new(PacketType::Connect);
        header.set_address(&Address { device_cid: 1, host_cid: 2, device_port: 3, host_port: 4 });
        connection.handle_vsock_packet(Packet { header, payload: &[] }).await.unwrap();

        let request = incoming_requests.next().await.unwrap();
        assert_eq!(
            request.address,
            Address { device_cid: 1, host_cid: 2, device_port: 3, host_port: 4 }
        );

        let (socket, other_socket) = SyncSocket::create_stream();
        let mut socket = Socket::from_socket(socket);
        connection.accept(request, Socket::from_socket(other_socket)).await.unwrap();

        for size in [1u8, 2, 8, 16, 32, 64, 128, 255] {
            println!("round tripping packet of size {size}");
            socket.write_all(&vec![size; size as usize]).await.unwrap();
            let mut buf = vec![0u8; size as usize];
            socket.read_exact(&mut buf).await.unwrap();
            assert_eq!(buf, vec![size; size as usize]);
        }
        echo_task.abort().await;
    }

    async fn copy_connection(from: &Connection<Vec<u8>, Socket>, to: &Connection<Vec<u8>, Socket>) {
        let mut builder = UsbPacketBuilder::new(vec![0; 1024]);
        loop {
            builder = from.fill_usb_packet(builder).await.unwrap();
            let packets = VsockPacketIterator::new(builder.take_usb_packet().unwrap());
            for packet in packets {
                println!("forwarding vsock packet");
                to.handle_vsock_packet(packet.unwrap()).await.unwrap();
            }
        }
    }

    pub(crate) trait EndToEndTestFn<R>:
        AsyncFnOnce(Arc<Connection<Vec<u8>, Socket>>, mpsc::Receiver<ConnectionRequest>) -> R
    {
    }
    impl<T, R> EndToEndTestFn<R> for T where
        T: AsyncFnOnce(Arc<Connection<Vec<u8>, Socket>>, mpsc::Receiver<ConnectionRequest>) -> R
    {
    }

    pub(crate) async fn end_to_end_test<R1, R2>(
        left_side: impl EndToEndTestFn<R1>,
        right_side: impl EndToEndTestFn<R2>,
    ) -> (R1, R2) {
        type Connection = crate::Connection<Vec<u8>, Socket>;
        let (_control_socket1, other_socket1) = SyncSocket::create_stream();
        let (_control_socket2, other_socket2) = SyncSocket::create_stream();
        let (incoming_requests_tx1, incoming_requests1) = mpsc::channel(5);
        let (incoming_requests_tx2, incoming_requests2) = mpsc::channel(5);

        let connection1 = Arc::new(Connection::new(
            ProtocolVersion::LATEST,
            Some(Socket::from_socket(other_socket1)),
            incoming_requests_tx1,
        ));
        let connection2 = Arc::new(Connection::new(
            ProtocolVersion::LATEST,
            Some(Socket::from_socket(other_socket2)),
            incoming_requests_tx2,
        ));

        let conn1 = connection1.clone();
        let conn2 = connection2.clone();
        let passthrough_task = Task::spawn(async move {
            futures::join!(copy_connection(&conn1, &conn2), copy_connection(&conn2, &conn1),);
            println!("passthrough task loop ended");
        });

        let res = futures::join!(
            left_side(connection1, incoming_requests1),
            right_side(connection2, incoming_requests2)
        );
        passthrough_task.abort().await;
        res
    }

    #[fuchsia::test]
    async fn data_over_end_to_end() {
        end_to_end_test(
            async |conn, _incoming| {
                println!("sending request on connection 1");
                let (socket, other_socket) = SyncSocket::create_stream();
                let mut socket = Socket::from_socket(socket);
                let state = conn
                    .connect(
                        Address { device_cid: 1, host_cid: 2, device_port: 3, host_port: 4 },
                        Socket::from_socket(other_socket),
                    )
                    .await
                    .unwrap();

                for size in [1u8, 2, 8, 16, 32, 64, 128, 255] {
                    println!("round tripping packet of size {size}");
                    socket.write_all(&vec![size; size as usize]).await.unwrap();
                }
                drop(socket);
                state.wait_for_close().await.unwrap();
            },
            async |conn, mut incoming| {
                println!("accepting request on connection 2");
                let request = incoming.next().await.unwrap();
                assert_eq!(
                    request.address,
                    Address { device_cid: 1, host_cid: 2, device_port: 3, host_port: 4 }
                );

                let (socket, other_socket) = SyncSocket::create_stream();
                let mut socket = Socket::from_socket(socket);
                let state = conn.accept(request, Socket::from_socket(other_socket)).await.unwrap();

                println!("accepted request on connection 2");
                for size in [1u8, 2, 8, 16, 32, 64, 128, 255] {
                    let mut buf = vec![0u8; size as usize];
                    socket.read_exact(&mut buf).await.unwrap();
                    assert_eq!(buf, vec![size; size as usize]);
                }
                assert_eq!(socket.read(&mut [0u8; 1]).await.unwrap(), 0);
                state.wait_for_close().await.unwrap();
            },
        )
        .await;
    }

    #[fuchsia::test]
    async fn normal_close_end_to_end() {
        let addr = Address { device_cid: 1, host_cid: 2, device_port: 3, host_port: 4 };
        end_to_end_test(
            async |conn, _incoming| {
                let (socket, other_socket) = SyncSocket::create_stream();
                let mut socket = Socket::from_socket(socket);
                let state =
                    conn.connect(addr.clone(), Socket::from_socket(other_socket)).await.unwrap();
                conn.close(&addr).await;
                assert_eq!(socket.read(&mut [0u8; 1]).await.unwrap(), 0);
                state.wait_for_close().await.unwrap();
            },
            async |conn, mut incoming| {
                println!("accepting request on connection 2");
                let request = incoming.next().await.unwrap();
                assert_eq!(request.address, addr.clone(),);

                let (socket, other_socket) = SyncSocket::create_stream();
                let mut socket = Socket::from_socket(socket);
                let state = conn.accept(request, Socket::from_socket(other_socket)).await.unwrap();
                assert_eq!(socket.read(&mut [0u8; 1]).await.unwrap(), 0);
                state.wait_for_close().await.unwrap();
            },
        )
        .await;
    }

    #[fuchsia::test]
    async fn reset_end_to_end() {
        let addr = Address { device_cid: 1, host_cid: 2, device_port: 3, host_port: 4 };
        end_to_end_test(
            async |conn, _incoming| {
                let (socket, other_socket) = SyncSocket::create_stream();
                let mut socket = Socket::from_socket(socket);
                let state =
                    conn.connect(addr.clone(), Socket::from_socket(other_socket)).await.unwrap();
                conn.reset(&addr).await.unwrap();
                assert_eq!(socket.read(&mut [0u8; 1]).await.unwrap(), 0);
                state.wait_for_close().await.expect_err("expected reset");
            },
            async |conn, mut incoming| {
                println!("accepting request on connection 2");
                let request = incoming.next().await.unwrap();
                assert_eq!(request.address, addr.clone(),);

                let (socket, other_socket) = SyncSocket::create_stream();
                let mut socket = Socket::from_socket(socket);
                let state = conn.accept(request, Socket::from_socket(other_socket)).await.unwrap();
                assert_eq!(socket.read(&mut [0u8; 1]).await.unwrap(), 0);
                state.wait_for_close().await.unwrap();
            },
        )
        .await;
    }

    #[test_case(false; "in packet handling")]
    #[test_case(true; "in reply wait")]
    #[fuchsia::test]
    async fn conn_shutdown(fill_packets: bool) {
        let (incoming_requests_tx, _incoming_requests) = mpsc::channel(5);

        let connection = Arc::new(Connection::<Vec<u8>, fuchsia_async::Socket>::new(
            ProtocolVersion::LATEST,
            None,
            incoming_requests_tx,
        ));

        let mut filler = if fill_packets {
            Some(std::pin::pin!(connection.fill_usb_packet(UsbPacketBuilder::new(Vec::new()))))
        } else {
            None
        };

        let addr = Address { device_cid: 1, host_cid: 2, device_port: 3, host_port: 4 };
        let mut fut = std::pin::pin!(connection.connect_late(addr));

        for _ in 0..5 {
            assert!(fut.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
            if let Some(filler) = filler.as_mut() {
                assert!(filler.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending())
            }
        }

        connection.shutdown();
        let Poll::Ready(res) = fut.poll(&mut Context::from_waker(Waker::noop())) else { panic!() };
        assert!(res.is_err());
        if let Some(filler) = filler {
            let Poll::Ready(res) = filler.poll(&mut Context::from_waker(Waker::noop())) else {
                panic!()
            };
            assert!(res.is_err());
        }
    }
}
