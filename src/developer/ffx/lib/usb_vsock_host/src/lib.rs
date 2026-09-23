// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bind_fuchsia_google_platform_usb::{
    BIND_USB_PROTOCOL_VSOCK_BRIDGE, BIND_USB_SUBCLASS_VSOCK_BRIDGE, BIND_USB_VID_GOOGLE,
};
use bind_fuchsia_usb::BIND_USB_CLASS_VENDOR_SPECIFIC;
use fuchsia_async::{self as fasync, DurationExt};
use futures::channel::{mpsc, oneshot};
use futures::future::{AbortHandle, Abortable, Either, select};
use futures::{AsyncRead, AsyncReadExt, AsyncWrite, FutureExt, SinkExt, Stream, StreamExt};
use std::collections::HashMap;
use std::iter::IntoIterator;
use std::num::NonZero;
use std::path::Path;
use std::pin::pin;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Weak};
use thiserror::Error;
use usb_vsock::{
    Address, CID_ANY, CID_HOST, CID_LOOPBACK, Header, Packet, PacketType, ProtocolVersion,
    UsbPacketBuilder, VsockPacketIterator,
};

/// How long to wait for the USB protocol to synchronize.
const MAGIC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// How long to wait to re-add a USB device if it breaks.
const FAIL_RETRY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// How much data to send in a single USB bulk transfer frame.
const MTU: usize = 1024;

/// Range from which we allocate random ports when making a connection.
/// Deliberately non-inclusive as (u32)-1 is a reserved value in the VSOCK spec.
const RANDOM_PORT_RANGE: std::ops::Range<u32> = 32768..u32::MAX;

/// How many URBs to allocate for each device we communicate with.
const URB_POOL_SIZE: usize = 32;

/// Watches the usb devfs for new devices. If `allow_serials` is non-empty, only
/// devices with a serial number in the list will be detected.
async fn listen_for_usb_devices<S: AsyncRead + AsyncWrite + Send + 'static>(
    host: Weak<UsbVsockHost<S>>,
    allow_serials: Vec<String>,
) -> Result<(), usb_rs::Error> {
    log::info!("Listening for USB devices");
    let mut stream = usb_rs::wait_for_devices(true, false)?;
    while let Some(device) = stream.next().await.transpose()? {
        let usb_rs::DeviceEvent::Added(device) = device else {
            continue;
        };

        let Some(host) = host.upgrade() else {
            log::debug!("USB listening task observed host disappeared");
            return Ok(());
        };

        if !allow_serials.is_empty()
            && allow_serials.iter().all(|x| Some(x) != device.serial().as_ref())
        {
            continue;
        }

        host.add_device(device);
    }

    log::warn!("USB listening stopped unexpectedly");
    Ok(())
}

/// Errors that can occur while exchanging magic packets when bringing up a USB VSOCK connection.
#[derive(Debug, Error)]
enum SyncError {
    #[error("Could not write to endpoint while synchronizing: {0}")]
    Send(usb_rs::Error),
    #[error("Could not read from endpoint while synchronizing: {0}")]
    Recv(usb_rs::Error),
    #[error("Timed out waiting for driver to synchronize")]
    TimedOut,
}

/// Creates a new magic packet used to synchronize a USB VSOCK connection.
fn sync_packet(version: ProtocolVersion) -> Vec<u8> {
    let magic = version.magic();
    let header = &mut Header::new(PacketType::Sync);
    header.payload_len = (magic.len() as u32).into();
    header.host_cid = CID_HOST.into();
    header.host_port = 0.into();
    header.device_cid = CID_ANY.into();
    header.device_port = 0.into();
    let packet = Packet { header, payload: &magic };
    let mut packet_storage = vec![0; header.packet_size()];
    packet.write_to_unchecked(&mut packet_storage);
    packet_storage
}

/// Creates a new magic packet used to finish synchronizing a USB VSOCK connection.
fn sync_ack_packet(cid: u32, version: ProtocolVersion) -> Vec<u8> {
    let magic = version.magic();
    let header = &mut Header::new(PacketType::Sync);
    header.payload_len = (magic.len() as u32).into();
    header.host_cid = CID_HOST.into();
    header.host_port = 0.into();
    header.device_cid = cid.into();
    header.device_port = 0.into();
    let packet = Packet { header, payload: &magic };
    let mut packet_storage = vec![0; header.packet_size()];
    packet.write_to_unchecked(&mut packet_storage);
    packet_storage
}

/// Creates a new magic packet used to synchronize a USB VSOCK connection.
fn echo_reply_packet(address: &Address, payload: &[u8]) -> Vec<u8> {
    let header = &mut Header::new(PacketType::EchoReply);
    header.set_address(address);
    header.payload_len = (payload.len() as u32).into();
    let packet = Packet { header, payload: payload };
    let mut packet_storage = vec![0; header.packet_size()];
    packet.write_to_unchecked(&mut packet_storage);
    packet_storage
}

/// Information we got about the connection after the magic exchange.
struct ConnectionInfo {
    protocol_version: ProtocolVersion,
    requested_cid: u32,
}

/// Wait on a USB port for the magic packet indicating the start of our USB
/// VSOCK protocol.
async fn wait_for_magic(
    debug_name: &str,
    out_ep: &usb_rs::BulkOutEndpoint,
    in_ep: &usb_rs::BulkInEndpoint,
) -> Result<ConnectionInfo, SyncError> {
    let mut magic_timer = fasync::Timer::new(MAGIC_TIMEOUT);
    let mut buf = [0u8; MTU];
    let nonce = rand::random::<u32>();
    let sent_version = ProtocolVersion::V2(nonce);
    out_ep
        .write(&sync_packet(sent_version), usb_rs::ZeroPacket::Send)
        .await
        .map_err(SyncError::Send)?;
    loop {
        let size = {
            log::trace!(device:? = debug_name; "Reading from in endpoint for magic string");
            let read_fut = in_ep.read(&mut buf);
            let read_fut = pin!(read_fut);
            match select(read_fut, &mut magic_timer).await {
                Either::Left((got, _)) => got.map_err(SyncError::Recv)?,
                Either::Right((_, fut)) => {
                    if let Some(got) = fut.now_or_never() {
                        got.map_err(SyncError::Recv)?
                    } else {
                        return Err(SyncError::TimedOut);
                    }
                }
            }
        };
        let buf = &buf[..size];

        let mut packets = VsockPacketIterator::new(&buf);
        while let Some(packet) = packets.next() {
            let Ok(packet) = packet else {
                log::warn!(device:? = debug_name; "Packet failed to parse, ignoring.");
                break;
            };
            match packet.header.packet_type {
                PacketType::Sync => {
                    if let Some(protocol_version) = ProtocolVersion::from_magic(packet.payload) {
                        if protocol_version == sent_version {
                            return Ok(ConnectionInfo {
                                protocol_version,
                                requested_cid: packet.header.device_cid.get(),
                            });
                        }
                        if matches!(protocol_version, ProtocolVersion::V1 | ProtocolVersion::V0) {
                            log::info!(
                                device:? = debug_name;
                                "Target requested downgrade to protocol version {protocol_version}"
                            );
                            return Ok(ConnectionInfo {
                                protocol_version,
                                requested_cid: packet.header.device_cid.get(),
                            });
                        }
                        log::warn!(
                            device:? = debug_name;
                            "Received sync packet with unexpected version or non-matching nonce: {protocol_version}, ignoring."
                        );
                    } else {
                        log::warn!(
                            device:? = debug_name;
                            "Invalid USB magic string (len = {}) received, ignoring.",
                            packet.header.payload_len
                        );
                    }
                }
                PacketType::Echo => {
                    log::debug!(
                        device:? = debug_name;
                        "received echo packet while waiting for sync, responding."
                    );
                    out_ep
                        .write(
                            &echo_reply_packet(&Address::from(packet.header), packet.payload),
                            usb_rs::ZeroPacket::Send,
                        )
                        .await
                        .map_err(SyncError::Send)?;
                }
                ty => {
                    log::warn!(
                        device:? = debug_name;
                        "Unexpected packet type '{ty:?}' waiting for packet synchronization, ignoring."
                    );
                }
            }
        }
    }
}

/// Errors which cause `run_usb_link` to fail.
#[derive(Debug, Error)]
enum LinkError {
    #[error(transparent)]
    SyncError(#[from] SyncError),
    #[error("In endpoint missing")]
    InMissing,
    #[error("Out endpoint missing")]
    OutMissing,
    #[error("Could not write to endpoint: {0}")]
    Send(usb_rs::Error),
    #[error("Could not read from endpoint: {0}")]
    Recv(usb_rs::Error),
    #[error("Error decoding packet: {0}")]
    PacketDecode(std::io::Error),
    #[error("Error handling packet: {0}")]
    PacketHandle(std::io::Error),
}

/// Handles sending packets from `usb_vsock::Connection` over the USB device,
/// and giving received packets from the USB device to the same connection.
async fn run_usb_link<S: AsyncRead + AsyncWrite + Send + 'static>(
    host: Weak<UsbVsockHost<S>>,
    debug_name: String,
    serial: Option<String>,
    interface: usb_rs::Interface,
    cid_out: &mut Option<u32>,
) -> Result<(), LinkError> {
    log::info!("Setting up USB link for {debug_name}");
    let debug_name = debug_name.as_str();

    let mut in_ep = None;
    let mut out_ep = None;

    for endpoint in interface.endpoints() {
        match endpoint {
            usb_rs::Endpoint::BulkIn(endpoint) => {
                if in_ep.is_some() {
                    log::warn!(device:? = debug_name; "Multiple bulk in endpoints on interface");
                } else {
                    in_ep = Some(endpoint)
                }
            }
            usb_rs::Endpoint::BulkOut(endpoint) => {
                if out_ep.is_some() {
                    log::warn!(device:? = debug_name; "Multiple bulk out endpoints on interface");
                } else {
                    out_ep = Some(endpoint)
                }
            }
            _ => (),
        }
    }

    let in_ep = in_ep.ok_or(LinkError::InMissing)?;
    let out_ep = out_ep.ok_or(LinkError::OutMissing)?;

    let ConnectionInfo { protocol_version, requested_cid } =
        wait_for_magic(debug_name, &out_ep, &in_ep).await?;

    let (conn_state, incoming_requests) = ConnectionState::new(protocol_version, serial.clone());
    let connection = Arc::clone(&conn_state.connection);
    let cid = if let Some(host) = host.upgrade() {
        let cid = {
            let mut inner = host.inner.lock().unwrap();
            let cid = if requested_cid > CID_HOST
                && requested_cid != CID_ANY
                && !inner.conns.contains_key(&requested_cid)
            {
                requested_cid
            } else {
                host.next_cid.fetch_add(1, Ordering::Relaxed)
            };

            inner.conns.insert(cid, conn_state);
            *cid_out = Some(cid);
            cid
        };

        host.add_incoming_request_handler(cid, incoming_requests);

        let mut sender = host.event_sender.clone();
        host.scope.spawn(async move {
            let _ = sender.send(UsbVsockHostEvent::AddedCid { cid, serial: serial.clone() }).await;
        });

        log::info!("USB link established for {debug_name} now usb:cid:{cid}");
        cid
    } else {
        log::warn!(device:? = debug_name; "Host object disappeared before connection established");
        return Ok(());
    };

    let debug_name = format!("usb:cid:{cid} ({debug_name})");

    out_ep
        .write(&sync_ack_packet(cid, protocol_version), usb_rs::ZeroPacket::Send)
        .await
        .map_err(SyncError::Send)?;

    let tx_conn = connection.clone();
    let tx = async move {
        let (tx_err_sender, tx_errs) = mpsc::unbounded();

        let tx_main = pin!(async {
            let mut builder = UsbPacketBuilder::new(vec![0u8; MTU]);
            while let Ok(got) = tx_conn.fill_usb_packet(builder).await {
                builder = got;
                let err_fut = out_ep
                    .write_defer_wait(builder.take_usb_packet().unwrap(), usb_rs::ZeroPacket::Send)
                    .await
                    .map_err(LinkError::Send)?;
                if tx_err_sender.unbounded_send(err_fut).is_err() {
                    break;
                }
            }
            Ok(())
        });

        let mut tx_errs = pin!(tx_errs.filter_map(|x| async move { x.await.err() }));
        let tx_errs = tx_errs.next();

        match select(tx_main, tx_errs).await {
            Either::Left((main_result, _)) => main_result,
            Either::Right((Some(tx_error), _)) => Err(LinkError::Send(tx_error)),
            Either::Right((None, tx_main)) => tx_main.await,
        }
    };
    let rx_conn = connection.clone();
    let rx = async move {
        let mut stream = in_ep.to_stream(MTU, URB_POOL_SIZE / 2);
        while let Some(res) = stream.next().await {
            let data = res.map_err(LinkError::Recv)?;

            if data.is_empty() {
                continue;
            }

            let mut packets = VsockPacketIterator::new(&data);
            while let Some(packet) = packets.next() {
                rx_conn
                    .handle_vsock_packet(packet.map_err(LinkError::PacketDecode)?)
                    .await
                    .map_err(LinkError::PacketHandle)?;
            }
        }
        Ok(())
    };

    let tx = pin!(tx);
    let rx = pin!(rx);
    match select(tx, rx).await {
        Either::Left((e, _)) => {
            if let Result::<(), LinkError>::Err(e) = e {
                log::warn!(usb_device:? = debug_name; "Transmit failed: {:?}", e);
                Err(e)
            } else {
                log::debug!(usb_device:? = debug_name; "Transmit closed");
                Ok(())
            }
        }
        Either::Right((e, _)) => {
            if let Result::<(), LinkError>::Err(e) = e {
                log::warn!(usb_device:? = debug_name; "Receive failed: {:?}", e);
                Err(e)
            } else {
                log::debug!(usb_device:? = debug_name; "Receive closed");
                Ok(())
            }
        }
    }
}

/// State of a local port.
#[derive(Clone)]
enum PortState<S> {
    /// This port is reserved. Usually means this is the near side of a
    /// connection that was established to some device via `connect()`.
    Reserved,

    /// We are listening on this port. Incoming connections are delivered via
    /// the sender.
    Listening(mpsc::Sender<IncomingConnection<S>>),
}

impl<S: AsyncRead + AsyncWrite + Send + 'static> PortState<S> {
    /// Tries to listen on this port at the given CID. The `port` argument is
    /// for error reporting and should name the port this `PortState` is
    /// associated with.
    fn add_listener(
        &mut self,
        port: u32,
        sender: mpsc::Sender<IncomingConnection<S>>,
    ) -> Result<(), ListenError> {
        match self {
            PortState::Reserved => Err(ListenError::PortInUse(port)),
            PortState::Listening(l) => {
                if l.is_closed() {
                    *l = sender;
                    Ok(())
                } else {
                    Err(ListenError::PortInUse(port))
                }
            }
        }
    }
}

/// Holds a connection to a single USB device.
struct ConnectionState<S> {
    connection: Arc<usb_vsock::Connection<Vec<u8>, S>>,
    serial: Option<String>,
}

impl<S: AsyncRead + AsyncWrite + Send + 'static> ConnectionState<S> {
    /// Create a new connection state.
    fn new(
        protocol_version: ProtocolVersion,
        serial: Option<String>,
    ) -> (Self, mpsc::Receiver<usb_vsock::ConnectionRequest>) {
        let (incoming_requests_tx, incoming_requests) = mpsc::channel(1);
        let connection =
            Arc::new(usb_vsock::Connection::new(protocol_version, None, incoming_requests_tx));

        (ConnectionState { connection: Arc::clone(&connection), serial }, incoming_requests)
    }
}

impl<S> Drop for ConnectionState<S> {
    fn drop(&mut self) {
        self.connection.shutdown();
    }
}

/// Errors returned from operations on [`UsbVsockHost`]
#[derive(Debug, Error)]
pub enum UsbVsockError {
    #[error("No target found with cid {0}")]
    NotFound(u32),
    #[error("Port {0} already in use")]
    PortInUse(u32),
    #[error("Connection failed")]
    ConnectFailed(std::io::Error),
    #[error("Accepting connection failed")]
    AcceptFailed(std::io::Error),
    #[error("Port number was too large")]
    PortOutOfRange,
}

/// Errors returned from [`UsbVsockHost::connect`]
#[derive(Debug, Error)]
pub enum ConnectError {
    #[error("No target found with cid {0}")]
    NotFound(u32),
    #[error("Port {0} already in use")]
    PortInUse(u32),
    #[error("Connection failed")]
    Failed(std::io::Error),
    #[error("Port number was too large")]
    PortOutOfRange,
}

/// Errors returned from [`UsbVsockHost::listen`]
#[derive(Debug, Error)]
pub enum ListenError {
    #[error("No target found with cid {0}")]
    NotFound(u32),
    #[error("Port {0} already in use")]
    PortInUse(u32),
}

/// Lock-protected fields of `UsbVsockHost`.
struct UsbVsockHostInner<S> {
    conns: HashMap<u32, ConnectionState<S>>,
    port_states: HashMap<u32, PortState<S>>,
}

/// Events coming from the `UsbVsockHost` indicating the appearance and
/// disappearance of CIDs.
#[derive(Clone, PartialEq, Eq)]
pub enum UsbVsockHostEvent {
    AddedCid { cid: u32, serial: Option<String> },
    RemovedCid(u32),
}

/// Represents a connection that has been established and just needs a socket to
/// start servicing.
pub enum ReadyConnect<S> {
    Normal(usb_vsock::ReadyConnect<Vec<u8>, S>),
    Loopback(oneshot::Sender<S>),
}

impl<S: AsyncRead + AsyncWrite + Send + 'static> ReadyConnect<S> {
    /// Begin servicing this accepted connection with the given socket.
    pub async fn finish_connect(self, socket: S) {
        match self {
            ReadyConnect::Normal(ready_connect) => ready_connect.finish_connect(socket).await,
            ReadyConnect::Loopback(sender) => {
                if sender.send(socket).is_err() {
                    log::warn!("Accepted connection task disappeared before servicing");
                }
            }
        }
    }
}

/// Represents an incoming connection on a port we are listening on.
pub struct IncomingConnection<S> {
    address: Address,
    acceptor: oneshot::Sender<oneshot::Sender<std::io::Result<ReadyConnect<S>>>>,
}

impl<S: AsyncRead + AsyncWrite + Send + 'static> IncomingConnection<S> {
    /// Address the connector is using.
    pub fn address(&self) -> &Address {
        &self.address
    }

    /// Accept the connection. Data will be sent over the provided socket.
    pub async fn accept(self, socket: S) -> Result<(), UsbVsockError> {
        Ok(self.accept_late().await?.finish_connect(socket).await)
    }

    /// Accept the connection. The returned `ReadyConnect` can be used to provide a socket for data.
    pub async fn accept_late(self) -> Result<ReadyConnect<S>, UsbVsockError> {
        let (sender, receiver) = futures::channel::oneshot::channel();
        self.acceptor
            .send(sender)
            .map_err(|_| UsbVsockError::AcceptFailed(std::io::Error::other("Driver gone")))?;
        receiver
            .await
            .map_err(|_| UsbVsockError::AcceptFailed(std::io::Error::other("Driver gone")))?
            .map_err(UsbVsockError::AcceptFailed)
    }
}

/// Information about an active connection to a device.
#[derive(Clone)]
pub struct ActiveDevice {
    pub cid: u32,
    pub serial: Option<String>,
}

enum DoConnectResult<S> {
    Connect(Arc<usb_vsock::Connection<Vec<u8>, S>>, usb_vsock::Address),
    Loopback(ReadyConnect<S>),
}

/// A container for connections to USB devices that is responsible for assigning
/// them CIDs and routing connections based on those CIDs.
pub struct UsbVsockHost<S> {
    scope: fasync::Scope,
    inner: Mutex<UsbVsockHostInner<S>>,
    next_cid: AtomicU32,
    event_sender: mpsc::Sender<UsbVsockHostEvent>,
    port_close_sender: mpsc::Sender<u32>,
}

impl<S> std::fmt::Debug for UsbVsockHost<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UsbVsockHost").finish()
    }
}

impl<S: AsyncRead + AsyncWrite + Send + 'static> UsbVsockHost<S> {
    /// Async task that cleans up randomly-allocated ports.
    async fn port_reaper(weak_self: Weak<Self>, mut close_ports: mpsc::Receiver<u32>) {
        while let Some(port) = close_ports.next().await {
            let Some(this) = weak_self.upgrade() else {
                break;
            };

            match this.inner.lock().unwrap().port_states.remove(&port) {
                Some(PortState::Reserved) => (),
                Some(PortState::Listening(_)) => panic!("Reaped listening port state!"),
                None => log::warn!(port; "Reaped port state that didn't exist"),
            }
        }
    }

    /// Create a new USB VSOCK host.  If `allow_serials` is non-empty, only
    /// devices with a serial number in the list will be detected.
    pub fn new(
        paths: impl IntoIterator<Item: AsRef<Path>>,
        discover: bool,
        events: mpsc::Sender<UsbVsockHostEvent>,
        allow_serials: Vec<String>,
    ) -> Arc<Self> {
        let (tx, rx) = futures::channel::oneshot::channel();
        let (port_close_sender, close_ports) = mpsc::channel(1);
        let ret = Arc::new_cyclic(|weak_self| {
            let ret = UsbVsockHost {
                scope: fasync::Scope::new(),
                inner: Mutex::new(UsbVsockHostInner {
                    conns: HashMap::new(),
                    port_states: HashMap::new(),
                }),
                next_cid: AtomicU32::new(3),
                event_sender: events,
                port_close_sender,
            };

            if discover {
                let weak_self = weak_self.clone();
                ret.scope.spawn(async move {
                    // Make sure we're out of Arc::new_cyclic before this future gets polled.
                    let _ = rx.await;
                    if let Err(e) = listen_for_usb_devices(weak_self, allow_serials).await {
                        log::warn!(error:? = e; "USB listening encountered an unexpected error");
                    }
                });
            }

            ret.scope.spawn(Self::port_reaper(weak_self.clone(), close_ports));

            ret
        });

        for path in paths {
            ret.add_path(path);
        }

        let _ = tx.send(());
        ret
    }

    /// Get all CIDs for currently-existing connections.
    pub fn active_devices(&self) -> Vec<ActiveDevice> {
        self.inner
            .lock()
            .unwrap()
            .conns
            .iter()
            .map(|(cid, conn)| ActiveDevice { cid: *cid, serial: conn.serial.clone() })
            .collect()
    }

    /// Connect to a new USB device by device path and assign it a CID. Returns
    /// `true` if the device was added successfully. `false` could just indicate
    /// the device wasn't a USB VSOCK device, so it's a normal event, not an
    /// error, hence a bool not a Result.
    pub fn add_path(self: &Arc<Self>, path: impl AsRef<Path>) -> bool {
        self.add_device(usb_rs::DeviceHandle::from_path(path))
    }

    /// Allocate a new port on the host (cid 2) to be used as the local end of a
    /// connection the host initiates.
    fn alloc_port(&self) -> u32 {
        loop {
            let random_port = rand::random_range(RANDOM_PORT_RANGE);

            if let std::collections::hash_map::Entry::Vacant(entry) =
                self.inner.lock().unwrap().port_states.entry(random_port)
            {
                entry.insert(PortState::Reserved);
                return random_port;
            }
        }
    }

    /// Common implementation of [`UsbVsockHost::connect`] and [`UsbVsockHost::connect_late`].
    async fn do_connect(
        &self,
        cid: NonZero<u32>,
        port: u32,
    ) -> Result<DoConnectResult<S>, ConnectError> {
        if port == u32::MAX {
            return Err(ConnectError::PortOutOfRange);
        }

        let cid = cid.get();

        if cid == CID_LOOPBACK || cid == CID_HOST {
            let mut state = self
                .inner
                .lock()
                .unwrap()
                .port_states
                .get(&port)
                .and_then(|state| match state {
                    PortState::Listening(l) => Some(l),
                    PortState::Reserved => None,
                })
                .filter(|x| !x.is_closed())
                .cloned()
                .ok_or_else(|| {
                    ConnectError::Failed(std::io::Error::other(format!(
                        "No listeners on looback port {port:?}"
                    )))
                })?;

            let host_port = self.alloc_port();
            let (acceptor, receiver) = oneshot::channel();
            let incoming = IncomingConnection {
                address: usb_vsock::Address {
                    device_cid: cid,
                    host_cid: CID_HOST,
                    device_port: port,
                    host_port,
                },
                acceptor,
            };
            state.send(incoming).await.expect("Sender closed unexpectedly");

            let Ok(sender) = receiver.await else {
                let _ = self.port_close_sender.clone().send(host_port).await;
                return Err(ConnectError::Failed(std::io::Error::other(format!(
                    "Listener rejected incoming loopback connection"
                ))));
            };

            let (sender_b, receiver_b) = oneshot::channel::<S>();
            let mut port_close_sender = self.port_close_sender.clone();

            self.scope.spawn(async move {
                let got = receiver_b.await;

                match got {
                    Ok(b) => {
                        let (sender_a, receiver_a) = oneshot::channel();
                        let _ = sender.send(Ok(ReadyConnect::Loopback(sender_a)));

                        if let Ok(a) = receiver_a.await {
                            let (a_rx, mut a_tx) = a.split();
                            let (b_rx, mut b_tx) = b.split();
                            let a_rx = futures::io::BufReader::new(a_rx);
                            let b_rx = futures::io::BufReader::new(b_rx);
                            let a_b = futures::io::copy_buf(a_rx, &mut b_tx);
                            let b_a = futures::io::copy_buf(b_rx, &mut a_tx);
                            let _ = futures::future::join(a_b, b_a).await;
                        }
                    }
                    Err(_) => {
                        let _ = sender.send(Err(std::io::Error::other(format!(
                            "Loopback connection hung up early"
                        ))));
                    }
                }
                let _ = port_close_sender.send(host_port).await;
            });

            return Ok(DoConnectResult::Loopback(ReadyConnect::<S>::Loopback(sender_b)));
        }

        let Some(conn) =
            self.inner.lock().unwrap().conns.get_mut(&cid).map(|x| Arc::clone(&x.connection))
        else {
            return Err(ConnectError::NotFound(cid));
        };

        let host_port = self.alloc_port();

        return Ok(DoConnectResult::Connect(
            conn,
            usb_vsock::Address {
                device_cid: cid,
                host_cid: CID_HOST,
                device_port: port,
                host_port,
            },
        ));
    }

    /// Start a task that waits for a connection to close then cleans up the
    /// associated port.
    fn spawn_port_cleanup(&self, host_port: u32, state: usb_vsock::ConnectionState) {
        let mut port_close_sender = self.port_close_sender.clone();
        self.scope.spawn(async move {
            let _ = state.wait_for_close().await;
            if port_close_sender.send(host_port).await.is_err() {
                log::warn!("Port close task went missing");
            }
        });
    }

    /// Connect a new socket to a target with the given CID and port.
    pub async fn connect(
        &self,
        cid: NonZero<u32>,
        port: u32,
        socket: S,
    ) -> Result<(), ConnectError> {
        match self.do_connect(cid, port).await? {
            DoConnectResult::Connect(conn, addr) => {
                let host_port = addr.host_port;
                let state = conn.connect(addr, socket).await.map_err(ConnectError::Failed);
                if state.is_err() {
                    let _ = self.port_close_sender.clone().send(host_port).await;
                }
                self.spawn_port_cleanup(host_port, state?);
            }
            DoConnectResult::Loopback(ready) => {
                ready.finish_connect(socket).await;
            }
        }
        Ok(())
    }

    /// Connect a new socket to a target with the given CID and port, but don't
    /// give the data socket. Instead return a [`ReadyConnect`] which can be
    /// used to provide the socket later.
    pub async fn connect_late(
        &self,
        cid: NonZero<u32>,
        port: u32,
    ) -> Result<ReadyConnect<S>, ConnectError> {
        match self.do_connect(cid, port).await? {
            DoConnectResult::Connect(conn, addr) => {
                let host_port = addr.host_port;
                let got = conn.connect_late(addr).await.map_err(ConnectError::Failed);
                if got.is_err() {
                    let _ = self.port_close_sender.clone().send(host_port).await;
                }
                let (ready, state) = got?;
                self.spawn_port_cleanup(host_port, state);
                Ok(ReadyConnect::Normal(ready))
            }
            DoConnectResult::Loopback(ready) => Ok(ready),
        }
    }

    /// Listen for connections to the host (cid 2) on the given port.
    pub fn listen(
        &self,
        port: u32,
    ) -> Result<impl Stream<Item = IncomingConnection<S>> + 'static, ListenError> {
        let mut inner = self.inner.lock().unwrap();

        let (sender, receiver) = mpsc::channel(1);

        match inner.port_states.entry(port) {
            std::collections::hash_map::Entry::Vacant(port_state) => {
                port_state.insert(PortState::Listening(sender));
            }
            std::collections::hash_map::Entry::Occupied(mut port_state) => {
                port_state.get_mut().add_listener(port, sender)?;
            }
        }

        Ok(receiver)
    }

    /// Establish a connection with a USB device and assign it a CID. Returns
    /// true if the device was added (see `add_path`).
    fn add_device(self: &Arc<Self>, device: usb_rs::DeviceHandle) -> bool {
        let interface = match device.scan_interfaces(URB_POOL_SIZE, |device, interface| {
            let subclass_match = u32::from(device.vendor) == BIND_USB_VID_GOOGLE
                && u32::from(interface.class) == BIND_USB_CLASS_VENDOR_SPECIFIC
                && u32::from(interface.subclass) == BIND_USB_SUBCLASS_VSOCK_BRIDGE;
            let protocol_match = u32::from(interface.protocol) == BIND_USB_PROTOCOL_VSOCK_BRIDGE;
            subclass_match && protocol_match
        }) {
            Ok(dev) => dev,
            Err(usb_rs::Error::InterfaceNotFound) => {
                return false;
            }
            Err(e) => {
                log::warn!(device = device.debug_name().as_str(), error:? = e;
                               "Error scanning USB device");
                return false;
            }
        };

        let weak_this = Arc::downgrade(self);
        self.scope.spawn(async move {
            // This timer will expire N seconds after the instant it was
            // created. If it's not used until N or more seconds after its
            // creation, it will return immediately on await.
            let retry_timer = fasync::Timer::new(FAIL_RETRY_TIMEOUT.after_now());
            let mut cid = None;
            if let Err(e) = run_usb_link(
                weak_this.clone(),
                device.debug_name(),
                device.serial(),
                interface,
                &mut cid,
            )
            .await
            {
                log::warn!("USB link terminated with error: {:?}", e)
            } else {
                log::info!("Shut down USB link for {}", device.debug_name())
            }

            if let Some(this) = weak_this.upgrade() {
                if let Some(cid) = cid {
                    this.remove_device(cid);
                }
                retry_timer.await;
                let _: bool = this.add_device(device);
            }
        });

        true
    }

    /// Remove a device by CID from the connected devices list and shut down all
    /// related connections.
    fn remove_device(&self, cid: u32) {
        let mut inner = self.inner.lock().unwrap();
        let Some(got) = inner.conns.remove(&cid) else {
            return;
        };
        std::mem::drop(got);

        let mut sender = self.event_sender.clone();
        self.scope.spawn(async move {
            let _ = sender.send(UsbVsockHostEvent::RemovedCid(cid)).await;
        });
    }

    /// Set up incoming requests for a given connection to be handled correctly.
    fn add_incoming_request_handler(
        self: &Arc<Self>,
        cid: u32,
        mut incoming_requests: mpsc::Receiver<usb_vsock::ConnectionRequest>,
    ) {
        let weak_this = Arc::downgrade(self);
        self.scope.spawn(async move {
            while let Some(incoming) = incoming_requests.next().await {
                let Some(this) = weak_this.upgrade() else {
                    log::debug!("Host disappeared before incoming request task terminated");
                    break;
                };

                let (accept_channel, connection) = {
                    let mut inner = this.inner.lock().unwrap();
                    let Some(state) = inner.conns.get_mut(&cid) else {
                        log::debug!("Connection state removed before request task terminated");
                        break;
                    };
                    let connection = Arc::clone(&state.connection);

                    let usb_vsock::Address { device_cid, host_cid, device_port: _, host_port } =
                        *incoming.address();

                    let accept_channel = if host_cid != CID_HOST {
                        log::warn!("USB device usb:cid:{cid} tried to connect to non-host cid {host_cid}");
                        None
                    } else if device_cid != cid {
                        log::warn!("USB device usb:cid:{cid} tried to relay a connection from {device_cid}");
                        None
                    } else {
                        let ret = if let Some(PortState::Listening(ch)) = inner.port_states.get(&host_port) &&
                        !ch.is_closed() {
                            Some(ch.clone())
                        } else {
                            None
                        };
                        if ret.is_none() {
                            log::debug!("USB device usb:cid:{cid} tried to connect to closed port {host_port}");
                        }
                        ret
                    };

                    (accept_channel, connection)
                };

                if let Some(mut accept_channel) = accept_channel {
                    let (sender, receiver) = oneshot::channel();
                    if let Err(_) = accept_channel.send(IncomingConnection {acceptor: sender, address: *incoming.address() }).await {
                        log::warn!(cid; "Listener disappeared while accepting connection");
                    } else if let Ok(responder) = receiver.await {
                        let address = incoming.address().clone();
                        if let Err(_) = responder.send(connection.accept_late(incoming).await.map(|x| ReadyConnect::Normal(x.0))) {
                            log::warn!(cid; "Accepting connection request failed");
                            let _: Result<_, _> = connection.reset(&address).await;
                        }
                        continue;
                    } else {
                        log::debug!(cid; "Listener rejected incoming connection");
                    }
                }

                if let Err(e) = connection.reject(incoming).await {
                    log::warn!(cid, error:? = e; "Rejecting connection request failed");
                }
            }
        });
    }

    /// Create a new USB VSOCK host for testing. Guaranteed not to try to touch
    /// the machine's actual USB devices.
    pub fn new_for_test(event_sender: mpsc::Sender<UsbVsockHostEvent>) -> Arc<Self> {
        let (port_close_sender, close_ports) = mpsc::channel(1);
        let ret = Arc::new(UsbVsockHost {
            scope: fasync::Scope::new(),
            inner: Mutex::new(UsbVsockHostInner {
                conns: HashMap::new(),
                port_states: HashMap::new(),
            }),
            next_cid: AtomicU32::new(3),
            event_sender,
            port_close_sender,
        });
        let weak_self = Arc::downgrade(&ret);
        ret.scope.spawn(Self::port_reaper(weak_self, close_ports));
        ret
    }

    /// Add a new test connection to this host.
    pub fn add_connection_for_test(
        self: &Arc<Self>,
        connection: Arc<usb_vsock::Connection<Vec<u8>, S>>,
        incoming_requests: mpsc::Receiver<usb_vsock::ConnectionRequest>,
        serial: Option<String>,
    ) -> u32 {
        let cid = self.next_cid.fetch_add(1, Ordering::Relaxed);
        let success = self
            .inner
            .lock()
            .unwrap()
            .conns
            .insert(cid, ConnectionState { connection, serial: serial.clone() })
            .is_none();
        assert!(success);
        self.add_incoming_request_handler(cid, incoming_requests);
        let mut sender = self.event_sender.clone();
        self.scope.spawn(async move {
            let _ = sender.send(UsbVsockHostEvent::AddedCid { cid, serial }).await;
        });
        cid
    }
}

/// Collection of values related to a UsbVsockHost that has been set up for testing.
pub struct TestHost<S: AsyncRead + AsyncWrite + Send + 'static> {
    pub host: Arc<UsbVsockHost<S>>,
    pub event_receiver: mpsc::Receiver<UsbVsockHostEvent>,
}

/// Collection of values related to a test connection.
pub struct TestConnection<S: AsyncRead + AsyncWrite + Send + 'static> {
    pub cid: u32,
    pub connection: Arc<usb_vsock::Connection<Vec<u8>, S>>,
    pub serial: String,
    pub incoming_requests: mpsc::Receiver<usb_vsock::ConnectionRequest>,
    pub abort_transfer: (AbortHandle, AbortHandle),
    pub scope: fasync::Scope,
}

impl<S: AsyncRead + AsyncWrite + Send + 'static> TestConnection<S> {
    /// Creates a new host with one connected CID inside of it, and also a raw
    /// usb_vsock connection which is the other end of that connection
    /// (representing the target perspective).
    pub fn new() -> (TestHost<S>, TestConnection<S>) {
        let (a_incoming_requests_tx, a_incoming_requests) = mpsc::channel(1);
        let a = Arc::new(usb_vsock::Connection::new(
            ProtocolVersion::LATEST,
            None,
            a_incoming_requests_tx,
        ));

        let (b_incoming_requests_tx, b_incoming_requests) = mpsc::channel(1);
        let b = Arc::new(usb_vsock::Connection::new(
            ProtocolVersion::LATEST,
            None,
            b_incoming_requests_tx,
        ));

        let scope = fasync::Scope::new();
        let (abort_a, abort_a_reg) = AbortHandle::new_pair();
        let (abort_b, abort_b_reg) = AbortHandle::new_pair();
        for (from, to, abort_reg) in [
            (Arc::clone(&a), Arc::clone(&b), abort_a_reg),
            (Arc::clone(&b), Arc::clone(&a), abort_b_reg),
        ] {
            scope.spawn(
                Abortable::new(
                    async move {
                        let mut builder = UsbPacketBuilder::new(vec![0u8; MTU]);
                        while let Ok(got) = from.fill_usb_packet(builder).await {
                            builder = got;
                            let packets =
                                VsockPacketIterator::new(builder.take_usb_packet().unwrap());
                            for packet in packets {
                                to.handle_vsock_packet(packet.unwrap()).await.unwrap();
                            }
                        }
                    },
                    abort_reg,
                )
                .map(std::mem::drop),
            );
        }

        let serial = format!("{:x}", rand::random::<u64>());
        let (event_sender, event_receiver) = mpsc::channel(1);
        let host = UsbVsockHost::new_for_test(event_sender);
        let cid = host.add_connection_for_test(a, a_incoming_requests, Some(serial.clone()));

        (
            TestHost { host, event_receiver },
            TestConnection {
                cid,
                connection: b,
                serial,
                incoming_requests: b_incoming_requests,
                abort_transfer: (abort_a, abort_b),
                scope,
            },
        )
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use futures::{AsyncReadExt, AsyncWriteExt};

    #[fuchsia::test]
    async fn test_connect() {
        let (
            TestHost { host, event_receiver: _ },
            TestConnection {
                cid,
                connection,
                serial: _,
                mut incoming_requests,
                abort_transfer: _,
                scope: _scope,
            },
        ) = TestConnection::<fasync::Socket>::new();

        let (a, other_end) = fasync::emulated_handle::Socket::create_stream();
        let other_end = fasync::Socket::from_socket(other_end);
        let connect_task = {
            let host = Arc::clone(&host);
            fasync::Task::spawn(async move {
                host.connect(cid.try_into().unwrap(), 1234, other_end).await
            })
        };

        let incoming = incoming_requests.next().await.unwrap();

        let addr = incoming.address();

        assert_eq!(cid, addr.device_cid);
        assert_eq!(CID_HOST, addr.host_cid);
        assert_eq!(1234, addr.device_port);

        let (b, other_end) = fasync::emulated_handle::Socket::create_stream();
        let other_end = fasync::Socket::from_socket(other_end);
        let _state = connection.accept(incoming, other_end).await.unwrap();
        connect_task.await.unwrap();

        let mut a = fasync::Socket::from_socket(a);
        let mut b = fasync::Socket::from_socket(b);

        const TEST_STR_1: &[u8] = b"Y'all seem disenchanted with my whimsical diversions.";
        const TEST_STR_2: &[u8] = b"Why were we programmed to get bored anyway?";

        a.write_all(TEST_STR_1).await.unwrap();
        b.write_all(TEST_STR_2).await.unwrap();

        let mut buf = vec![0u8; TEST_STR_2.len()];
        a.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, TEST_STR_2);

        let mut buf = vec![0u8; TEST_STR_1.len()];
        b.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, TEST_STR_1);

        std::mem::drop(b);
        let Err(e) = a.read_exact(&mut buf).await else { panic!() };

        assert_eq!(std::io::ErrorKind::UnexpectedEof, e.kind());
    }

    #[fuchsia::test]
    async fn test_listen() {
        let (
            TestHost { host, event_receiver: _ },
            TestConnection {
                cid,
                connection,
                serial: _,
                incoming_requests: _,
                abort_transfer: _,
                scope: _scope,
            },
        ) = TestConnection::<fasync::Socket>::new();

        let connection = Arc::new(connection);

        for port_offset in 0..2 {
            let (a, other_end) = fasync::emulated_handle::Socket::create_stream();
            let other_end = fasync::Socket::from_socket(other_end);
            let mut listener = host.listen(1234).unwrap();
            let connection_clone = Arc::clone(&connection);
            let connect_task = fasync::Task::spawn(async move {
                connection_clone
                    .connect(
                        usb_vsock::Address {
                            device_cid: cid,
                            host_cid: 2,
                            device_port: 16384 + port_offset,
                            host_port: 1234,
                        },
                        other_end,
                    )
                    .await
            });

            let (b, other_end) = fasync::emulated_handle::Socket::create_stream();
            let other_end = fasync::Socket::from_socket(other_end);
            let _state = listener.next().await.unwrap().accept(other_end).await.unwrap();
            let _remote_state = connect_task.await.unwrap();

            let mut a = fasync::Socket::from_socket(a);
            let mut b = fasync::Socket::from_socket(b);

            const TEST_STR_1: &[u8] = b"Y'all seem disenchanted with my whimsical diversions.";
            const TEST_STR_2: &[u8] = b"Why were we programmed to get bored anyway?";

            a.write_all(TEST_STR_1).await.unwrap();
            b.write_all(TEST_STR_2).await.unwrap();

            let mut buf = vec![0u8; TEST_STR_2.len()];
            a.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, TEST_STR_2);

            let mut buf = vec![0u8; TEST_STR_1.len()];
            b.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, TEST_STR_1);
        }
    }

    #[fuchsia::test]
    async fn test_connect_bad_cid() {
        let (event_sender, _unused) = mpsc::channel(1);
        let host = UsbVsockHost::new_for_test(event_sender);
        let (sock, _) = fasync::emulated_handle::Socket::create_stream();
        let sock = fasync::Socket::from_socket(sock);
        let Err(ConnectError::NotFound(got_cid)) =
            host.connect(3.try_into().unwrap(), 1234, sock).await
        else {
            panic!()
        };

        assert_eq!(3, got_cid);
    }

    #[fuchsia::test]
    async fn test_connect_bad_port() {
        let (
            TestHost { host, event_receiver: _ },
            TestConnection {
                cid,
                connection: _c,
                serial: _,
                incoming_requests: _,
                abort_transfer: _,
                scope: _scope,
            },
        ) = TestConnection::<fasync::Socket>::new();
        let (sock, _) = fasync::emulated_handle::Socket::create_stream();
        let sock = fasync::Socket::from_socket(sock);
        let Err(ConnectError::PortOutOfRange) =
            host.connect(cid.try_into().unwrap(), u32::MAX, sock).await
        else {
            panic!()
        };
    }

    #[fuchsia::test]
    async fn test_connect_rejection() {
        let (
            TestHost { host, event_receiver: _ },
            TestConnection {
                cid,
                connection,
                serial: _,
                abort_transfer: _,
                mut incoming_requests,
                scope: _scope,
            },
        ) = TestConnection::<fasync::Socket>::new();
        let (sock, _) = fasync::emulated_handle::Socket::create_stream();
        let sock = fasync::Socket::from_socket(sock);
        let connect_task = {
            let host = Arc::clone(&host);
            fasync::Task::spawn(
                async move { host.connect(cid.try_into().unwrap(), 1234, sock).await },
            )
        };

        let req = incoming_requests.next().await.unwrap();
        connection.reject(req).await.unwrap();

        let Err(ConnectError::Failed(_)) = connect_task.await else {
            panic!();
        };
    }

    #[fuchsia::test]
    async fn test_refuse_connection() {
        let (
            TestHost { host: _host, event_receiver: _ },
            TestConnection {
                cid,
                connection,
                serial: _,
                abort_transfer: _,
                incoming_requests: _,
                scope: _scope,
            },
        ) = TestConnection::<fasync::Socket>::new();
        let (socket, _) = fasync::emulated_handle::Socket::create_stream();
        let socket = fasync::Socket::from_socket(socket);
        let Err(_) = connection
            .connect(
                usb_vsock::Address {
                    device_cid: cid,
                    host_cid: 2,
                    device_port: 1234,
                    host_port: 1234,
                },
                socket,
            )
            .await
        else {
            panic!();
        };
    }

    #[fuchsia::test]
    async fn test_reject_weird_cid() {
        let (
            TestHost { host, event_receiver: _ },
            TestConnection {
                cid,
                connection,
                serial: _,
                abort_transfer: _,
                incoming_requests: _,
                scope: _scope,
            },
        ) = TestConnection::<fasync::Socket>::new();

        let (socket, _) = fasync::emulated_handle::Socket::create_stream();
        let socket = fasync::Socket::from_socket(socket);
        let _listener = host.listen(1234).unwrap();
        let Err(_) = connection
            .connect(
                usb_vsock::Address {
                    device_cid: cid,
                    host_cid: 60,
                    device_port: 16384,
                    host_port: 1234,
                },
                socket,
            )
            .await
        else {
            panic!();
        };
    }

    #[fuchsia::test]
    async fn test_reject_from_weird_cid() {
        let (
            TestHost { host, event_receiver: _ },
            TestConnection {
                cid: _,
                connection,
                serial: _,
                abort_transfer: _,
                incoming_requests: _,
                scope: _scope,
            },
        ) = TestConnection::<fasync::Socket>::new();

        let (socket, _) = fasync::emulated_handle::Socket::create_stream();
        let socket = fasync::Socket::from_socket(socket);
        let _listener = host.listen(1234).unwrap();
        let Err(_) = connection
            .connect(
                usb_vsock::Address {
                    device_cid: 60,
                    host_cid: 2,
                    device_port: 16384,
                    host_port: 1234,
                },
                socket,
            )
            .await
        else {
            panic!();
        };
    }

    #[allow(clippy::unused_async)]
    #[fuchsia::test]
    async fn test_double_listen() {
        let (
            TestHost { host, event_receiver: _ },
            TestConnection {
                cid: _,
                connection: _connection,
                serial: _,
                abort_transfer: _,
                incoming_requests: _,
                scope: _scope,
            },
        ) = TestConnection::<fasync::Socket>::new();

        let _listener = host.listen(1234).unwrap();
        let Err(ListenError::PortInUse(port)) = host.listen(1234) else {
            panic!();
        };
        assert_eq!(1234, port);
    }

    #[fuchsia::test]
    async fn test_connect_then_listen() {
        let (
            TestHost { host, event_receiver: _ },
            TestConnection {
                cid,
                connection,
                serial: _,
                abort_transfer: _,
                mut incoming_requests,
                scope: _scope,
            },
        ) = TestConnection::<fasync::Socket>::new();

        let (socket, _a) = fasync::emulated_handle::Socket::create_stream();
        let socket = fasync::Socket::from_socket(socket);
        let connect_host = Arc::clone(&host);
        let connect_task = fasync::Task::spawn(async move {
            connect_host.connect(cid.try_into().unwrap(), 1234, socket).await
        });

        let request = incoming_requests.next().await.unwrap();
        let usb_vsock::Address { device_cid: _, host_cid: _, device_port: _, host_port } =
            *request.address();

        let (socket, b) = fasync::emulated_handle::Socket::create_stream();
        let socket = fasync::Socket::from_socket(socket);
        let _remote_state = connection.accept(request, socket).await.unwrap();
        connect_task.await.unwrap();
        let Err(ListenError::PortInUse(port)) = host.listen(host_port) else {
            panic!();
        };
        assert_eq!(host_port, port);
        std::mem::drop(b);
        loop {
            // Ports are reaped in the background, so we need to give the
            // executor a little time to do it.
            fasync::Timer::new(std::time::Duration::from_millis(100)).await;
            if host.listen(host_port).is_ok() {
                return;
            }
        }
    }

    #[fuchsia::test]
    async fn test_connect_then_drop_cid() {
        let (
            TestHost { host, event_receiver: _ },
            TestConnection {
                cid,
                connection,
                serial: _,
                abort_transfer: (abort_a, abort_b),
                mut incoming_requests,
                scope: _scope,
            },
        ) = TestConnection::<fasync::Socket>::new();

        let (a, other_end) = fasync::emulated_handle::Socket::create_stream();
        let other_end = fasync::Socket::from_socket(other_end);
        let connect_task = {
            let host = Arc::clone(&host);
            fasync::Task::spawn(async move {
                host.connect(cid.try_into().unwrap(), 1234, other_end).await
            })
        };

        let incoming = incoming_requests.next().await.unwrap();

        let addr = incoming.address();

        assert_eq!(cid, addr.device_cid);
        assert_eq!(2, addr.host_cid);
        assert_eq!(1234, addr.device_port);

        let (b, other_end) = fasync::emulated_handle::Socket::create_stream();
        let other_end = fasync::Socket::from_socket(other_end);
        let _state = connection.accept(incoming, other_end).await.unwrap();
        connect_task.await.unwrap();
        std::mem::drop(connection);

        let mut a = fasync::Socket::from_socket(a);
        let mut b = fasync::Socket::from_socket(b);

        const TEST_STR_1: &[u8] = b"Y'all seem disenchanted with my whimsical diversions.";

        a.write_all(TEST_STR_1).await.unwrap();

        let mut buf = vec![0u8; TEST_STR_1.len()];
        b.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, TEST_STR_1);

        host.remove_device(cid);
        abort_a.abort();
        abort_b.abort();

        let Err(e) = b.read_exact(&mut buf).await else {
            panic!();
        };

        assert_eq!(std::io::ErrorKind::UnexpectedEof, e.kind());

        let Err(e) = a.read_exact(&mut buf).await else {
            panic!();
        };

        assert_eq!(std::io::ErrorKind::UnexpectedEof, e.kind());

        let (_unused, other_end) = fasync::emulated_handle::Socket::create_stream();
        let other_end = fasync::Socket::from_socket(other_end);
        let Err(ConnectError::NotFound(got_cid)) =
            host.connect(cid.try_into().unwrap(), 1234, other_end).await
        else {
            panic!();
        };

        assert_eq!(cid, got_cid);
    }

    #[allow(clippy::unused_async)]
    #[fuchsia::test]
    async fn test_listen_drop_listen() {
        let (
            TestHost { host, event_receiver: _ },
            TestConnection {
                cid: _,
                connection: _connection,
                serial: _,
                abort_transfer: _,
                incoming_requests: _,
                scope: _scope,
            },
        ) = TestConnection::<fasync::Socket>::new();

        let listener = host.listen(1234).unwrap();
        std::mem::drop(listener);
        let _listener = host.listen(1234).unwrap();
    }

    #[fuchsia::test]
    async fn test_events() {
        let (
            TestHost { host, mut event_receiver },
            TestConnection {
                cid,
                connection: _connection,
                serial,
                abort_transfer: _,
                incoming_requests: _,
                scope: _scope,
            },
        ) = TestConnection::<fasync::Socket>::new();

        let Some(UsbVsockHostEvent::AddedCid { cid: got_cid, serial: got_serial }) =
            event_receiver.next().await
        else {
            panic!();
        };

        assert_eq!(cid, got_cid);
        assert_eq!(Some(serial), got_serial);

        host.remove_device(cid);

        let Some(UsbVsockHostEvent::RemovedCid(got_cid)) = event_receiver.next().await else {
            panic!();
        };

        assert_eq!(cid, got_cid);
    }

    async fn test_loopback_for_cid(cid: u32) {
        const TEST_PORT: u32 = 202;
        let (event_sender, _receiver) = mpsc::channel(0);
        let host = UsbVsockHost::<fasync::Socket>::new_for_test(event_sender);

        let mut incoming_requests = host.listen(TEST_PORT).unwrap();
        let (a, other_end) = fasync::emulated_handle::Socket::create_stream();
        let other_end = fasync::Socket::from_socket(other_end);

        let connect_task = {
            let host = Arc::clone(&host);
            fasync::Task::spawn(async move {
                host.connect(cid.try_into().unwrap(), TEST_PORT, other_end).await
            })
        };

        let incoming = incoming_requests.next().await.unwrap();

        let addr = incoming.address();

        assert_eq!(cid, addr.device_cid);
        assert_eq!(CID_HOST, addr.host_cid);
        assert_eq!(TEST_PORT, addr.device_port);

        let (b, other_end) = fasync::emulated_handle::Socket::create_stream();
        let other_end = fasync::Socket::from_socket(other_end);
        incoming.accept(other_end).await.unwrap();
        connect_task.await.unwrap();

        let a_fut = async move {
            let mut a = fasync::Socket::from_socket(a);

            a.write_all(b"Punch me in the throat.").await.unwrap();

            let mut buf = [0u8; 4];

            a.read_exact(&mut buf).await.unwrap();
            assert_eq!(b"Why?", &buf);

            a.write_all(b"'Cuz we're friends.").await.unwrap();
        };

        let b_fut = async move {
            let mut b = fasync::Socket::from_socket(b);

            let mut buf = [0u8; 23];

            b.read_exact(&mut buf).await.unwrap();
            assert_eq!(b"Punch me in the throat.", &buf);

            b.write_all(b"Why?").await.unwrap();
        };

        futures::future::join(a_fut, b_fut).await;
    }

    #[fuchsia::test]
    async fn test_loopback() {
        test_loopback_for_cid(CID_LOOPBACK).await;
    }

    #[fuchsia::test]
    async fn test_host_cid_loopback() {
        test_loopback_for_cid(CID_HOST).await;
    }

    #[fuchsia::test]
    async fn test_sync_packet_generation() {
        let version = ProtocolVersion::V2(0xabcdef01);
        let raw = sync_packet(version);
        let mut it = VsockPacketIterator::new(&raw);
        let packet = it.next().unwrap().unwrap();
        assert_eq!(packet.header.packet_type, PacketType::Sync);
        assert_eq!(packet.header.host_cid.get(), CID_HOST);
        assert_eq!(packet.header.device_cid.get(), CID_ANY);
        assert_eq!(packet.payload, b"vsock:2:abcdef01");
        assert_eq!(ProtocolVersion::from_magic(packet.payload), Some(version));
    }

    #[fuchsia::test]
    async fn test_sync_ack_packet_downgrade() {
        let ack_v2 = sync_ack_packet(42, ProtocolVersion::V2(0x1234));
        let mut it = VsockPacketIterator::new(&ack_v2);
        let packet = it.next().unwrap().unwrap();
        assert_eq!(packet.header.packet_type, PacketType::Sync);
        assert_eq!(packet.header.device_cid.get(), 42);
        assert_eq!(packet.payload, b"vsock:2:1234");

        let ack_v1 = sync_ack_packet(42, ProtocolVersion::V1);
        let mut it = VsockPacketIterator::new(&ack_v1);
        let packet = it.next().unwrap().unwrap();
        assert_eq!(packet.header.packet_type, PacketType::Sync);
        assert_eq!(packet.header.device_cid.get(), 42);
        assert_eq!(packet.payload, b"vsock:1");
    }
}
