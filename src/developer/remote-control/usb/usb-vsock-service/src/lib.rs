// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdf_component::{Driver, DriverContext, DriverError, Node, driver_register};
use fidl::endpoints::create_endpoints;
use fidl_fuchsia_hardware_vsock as vsock;
use fidl_fuchsia_hardware_vsockbridge as vsockbridge;
use fuchsia_async::scope::ScopeStream;
use fuchsia_async::{Scope, Socket};
use fuchsia_component::server::ServiceFs;
use futures::channel::{mpsc, oneshot};
use futures::future::{Either, select};
use futures::io::{ReadHalf, WriteHalf};
use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, StreamExt, TryStreamExt};
use log::{debug, error, info, warn};
use std::io::Error;
use std::pin::pin;
use std::sync::Arc;
use usb_vsock::{
    CID_HOST, Connection, ConnectionRequest, Header, Packet, PacketType, ProtocolVersion,
    UsbPacketBuilder, VsockPacketIterator,
};
use zx::{SocketOpts, Status};

mod vsock_service;

use vsock_service::VsockService;

static MTU: usize = 1024;

struct UsbVsockServiceDriver {
    /// A scope for async tasks running under this driver
    _scope: Scope,
    /// The [`Node`] is our handle to the node we bound to. We need to keep this handle
    /// open to keep the node around.
    _node: Node,
}

driver_register!(UsbVsockServiceDriver);

/// Processes a connection to the underlying USB device through a datagram socket where each
/// packet received or sent corresponds to a USB bulk transfer buffer. It will call the callback
/// with a new link and close the old one whenever a magic reset packet is received from the host.
struct UsbConnection {
    vsock_service: Arc<VsockService<Vec<u8>>>,
    usb_socket_reader: ReadHalf<Socket>,
    usb_socket_writer: WriteHalf<Socket>,
    connection_tx: mpsc::Sender<ConnectionRequest>,
}

impl UsbConnection {
    fn new(
        vsock_service: Arc<VsockService<Vec<u8>>>,
        usb_socket: zx::Socket,
        connection_tx: mpsc::Sender<ConnectionRequest>,
    ) -> Self {
        assert!(
            usb_socket.info().unwrap().options.contains(SocketOpts::DATAGRAM),
            "USB socket must be a datagram socket"
        );
        let (usb_socket_reader, usb_socket_writer) = Socket::from_socket(usb_socket).split();
        Self { vsock_service, usb_socket_reader, usb_socket_writer, connection_tx }
    }

    /// Waits for an [`PacketType::Sync`] packet and sends the reply back, and then returns the
    /// negotiated device CID for the connection as well as the negotiated protocol version.
    async fn next_socket(
        &mut self,
        mut found_magic: Option<Vec<u8>>,
    ) -> Option<(ProtocolVersion, u32)> {
        let mut data = [0; MTU];
        while found_magic.is_none() {
            let mut packets = match read_packet_stream(&mut self.usb_socket_reader, &mut data).await
            {
                Ok(None) => {
                    debug!("Usb socket closed");
                    return None;
                }
                Err(err) => {
                    error!("Unexpected error on usb socket: {err}");
                    return None;
                }
                Ok(Some(packets)) => packets,
            };

            while let Some(packet) = packets.next() {
                // note: we will deliberately warn and ignore for any vsock packets in the same
                // usb packet as a sync packet, regardless of whether they were before or after.
                match packet {
                    Ok(Packet {
                        header: Header { packet_type: PacketType::Sync, .. },
                        payload,
                    }) => {
                        found_magic = Some(payload.to_owned());
                    }
                    Ok(packet) => {
                        warn!(
                            "Got unexpected packet of type {:?} and length {} while waiting for sync packet. Ignoring.",
                            packet.header.packet_type, packet.header.payload_len
                        );
                    }
                    Err(err) => {
                        warn!("Got invalid vsock packet while waiting for sync packet: {err:?}");
                    }
                }
            }
        }
        let found_magic =
            found_magic.expect("read loop should not terminate until sync packet is read");

        let incoming_version = ProtocolVersion::from_magic(&found_magic);

        let outgoing_version = if let Some(incoming_version) = incoming_version {
            let Some(outgoing_version) = ProtocolVersion::LATEST.negotiate(&incoming_version)
            else {
                error!(
                    "Could not negotiate protocol version: driver has {}, host wants {incoming_version}",
                    ProtocolVersion::LATEST
                );
                return None;
            };

            outgoing_version
        } else {
            warn!(
                "Did not recognize protocol magic {found_magic:?}. Trying version {}",
                ProtocolVersion::LATEST
            );

            ProtocolVersion::LATEST
        };

        let outgoing_magic = outgoing_version.magic();

        debug!("Read sync packet, sending it back and setting up a new link");
        let mut header = Header::new(PacketType::Sync);
        header.payload_len = (outgoing_magic.len() as u32).into();
        header.device_cid.set(self.vsock_service.current_cid());
        header.host_cid.set(CID_HOST);
        let packet = Packet { header: &header, payload: &outgoing_magic };
        packet.write_to_unchecked(&mut data);
        if let Err(err) = self.usb_socket_writer.write(&data[..packet.size()]).await {
            error!("Error writing vsock bridge magic string to the usb socket: {err:?}");
            return None;
        }
        // Now wait for confirmation packet with cid in it
        loop {
            let mut packets = match read_packet_stream(&mut self.usb_socket_reader, &mut data).await
            {
                Ok(None) => {
                    debug!("Usb socket closed");
                    return None;
                }
                Err(err) => {
                    error!("Unexpected error on usb socket: {err}");
                    return None;
                }
                Ok(Some(packets)) => packets,
            };

            while let Some(packet) = packets.next() {
                // note: we will deliberately warn and ignore for any vsock packets in the same
                // usb packet as a sync packet, regardless of whether they were before or after.
                match packet {
                    Ok(Packet {
                        header: Header { packet_type: PacketType::Sync, device_cid, .. },
                        payload,
                    }) => {
                        if payload != outgoing_magic {
                            error!(
                                "Host gave unsupported protocol version string {payload:?}. Giving up."
                            );
                            return None;
                        }
                        return Some((outgoing_version, device_cid.get()));
                    }
                    Ok(packet) => {
                        warn!(
                            "Got unexpected packet of type {:?} and length {} while waiting for sync packet. Ignoring.",
                            packet.header.packet_type, packet.header.payload_len
                        );
                    }
                    Err(err) => {
                        warn!("Got invalid vsock packet while waiting for sync packet: {err:?}");
                    }
                }
            }
        }
    }

    async fn run(mut self, mut synchronized: Option<oneshot::Sender<()>>) {
        let mut found_magic = None;
        loop {
            let Some((protocol_version, cid)) = self.next_socket(found_magic).await else {
                info!("USB socket closed or failed");
                return;
            };
            // reset whether we found the magic string last time around or not.
            found_magic = None;
            info!("Bridge established with CID {cid}");
            // we allow `_other_end` to drop because we don't expect any further data on the control
            // socket, as it's currently unused. In the future if we want to have side channel data flow
            // between the host and driver, this is the socket it would go in.
            let connection =
                Arc::new(Connection::new(protocol_version, None, self.connection_tx.clone()));
            self.vsock_service.set_connection(connection.clone(), cid).await;
            log::trace!("vsock connection set");
            if let Some(synchronized) = synchronized.take() {
                // we only use this for testing really so ignore the result.
                synchronized.send(()).ok();
            }
            let usb_socket_writer =
                usb_socket_writer::<MTU>(&connection, &mut self.usb_socket_writer);
            let usb_socket_reader = usb_socket_reader::<MTU>(
                &mut found_magic,
                &mut self.usb_socket_reader,
                &connection,
            );
            let client_socket_copy = pin!(usb_socket_writer);
            let usb_socket_copy = pin!(usb_socket_reader);
            let res = select(client_socket_copy, usb_socket_copy).await;
            match res {
                Either::Left((Err(err), _)) => {
                    warn!("Error on client to usb socket transfer: {err:?}");
                }
                Either::Left((Ok(_), _)) => {
                    debug!("client to usb socket closed normally");
                }
                Either::Right((Err(err), _)) => {
                    warn!("Error on usb to client socket transfer: {err:?}");
                }
                Either::Right((Ok(_), _)) => {
                    info!("usb to client socket closed normally");
                }
            }
        }
    }
}

async fn read_packet_stream<'a>(
    reader: &mut (impl AsyncRead + Unpin),
    mut buffer: &'a mut [u8],
) -> Result<Option<VsockPacketIterator<'a>>, std::io::Error> {
    let size = reader.read(&mut buffer).await?;
    if size == 0 {
        return Ok(None);
    }
    Ok(Some(VsockPacketIterator::new(&buffer[0..size])))
}

async fn usb_socket_writer<const MTU: usize>(
    connection: &Connection<Vec<u8>, Socket>,
    usb_writer: &mut (impl AsyncWrite + Unpin),
) -> Result<(), Error> {
    let mut builder = UsbPacketBuilder::new(vec![0; MTU]);
    while let Ok(got) = connection.fill_usb_packet(builder).await {
        builder = got;
        let buf = builder.take_usb_packet().unwrap();
        assert_eq!(
            buf.len(),
            usb_writer.write(buf).await?,
            "datagram socket sent incomplete packet"
        );
    }

    Ok(())
}

async fn usb_socket_reader<const MTU: usize>(
    found_magic: &mut Option<Vec<u8>>,
    usb_reader: &mut (impl AsyncRead + Unpin),
    connection: &Connection<Vec<u8>, Socket>,
) -> Result<(), Error> {
    let mut data = [0; MTU];
    loop {
        let Some(mut packets) = read_packet_stream(usb_reader, &mut data).await? else {
            break;
        };
        while let Some(packet) = packets.next() {
            match packet {
                Ok(Packet { header: Header { packet_type: PacketType::Sync, .. }, payload }) => {
                    debug!("Found sync packet, ending stream");
                    *found_magic = Some(payload.to_owned());
                    return Ok(());
                }
                Ok(packet) => connection.handle_vsock_packet(packet).await?,
                Err(err) => {
                    error!(
                        "Failed to parse vsock packet, going back to waiting for sync packet: {err:?}"
                    );
                    break;
                }
            }
        }
    }
    Ok(())
}

/// Processes a stream of device connections from the parent driver, and for each one initiates a
/// [`UsbConnection`] process to handle individual connections to the host process.
struct UsbCallbackHandler {
    usb_callback_server: vsockbridge::CallbackRequestStream,
    connection_tx: mpsc::Sender<ConnectionRequest>,
}

impl UsbCallbackHandler {
    async fn run(
        mut self,
        vsock_service: Arc<VsockService<Vec<u8>>>,
        mut synchronized: Option<oneshot::Sender<()>>,
    ) -> Result<(), fidl::Error> {
        use vsockbridge::CallbackRequest::*;
        while let Some(req) = self.usb_callback_server.try_next().await? {
            let NewLink { socket, responder } = req;
            responder.send()?;

            debug!("Received new socket from usb driver");
            UsbConnection::new(vsock_service.clone(), socket, self.connection_tx.clone())
                .run(synchronized.take())
                .await;
        }
        Ok(())
    }
}

impl Driver for UsbVsockServiceDriver {
    const NAME: &str = "usb-vsock-service";

    async fn start(mut context: DriverContext) -> Result<Self, DriverError> {
        let node = context.take_node()?;
        let scope = Scope::new_with_name(Self::NAME);
        let mut outgoing = ServiceFs::new();

        let usb_device = get_usb_device(&context)?;

        info!("Offering a vsock service in the outgoing directory");
        outgoing.dir("svc").add_fidl_service_instance("default", move |i| {
            let vsock::ServiceRequest::Device(request_stream) = i;
            request_stream
        });

        context.serve_outgoing(&mut outgoing)?;

        scope.spawn(async move {
            while let Some(request_stream) = outgoing.next().await {
                let (usb_callback, usb_callback_server) = create_endpoints();
                usb_device.set_callback(usb_callback).await.expect("usb device service went away");

                run_connection(usb_callback_server.into_stream(), request_stream, None).await
            }
        });

        Ok(Self { _scope: scope, _node: node })
    }

    async fn stop(&self) {}
}

async fn run_connection(
    usb_callback_server: vsockbridge::CallbackRequestStream,
    mut request_stream: vsock::DeviceRequestStream,
    synchronized: Option<oneshot::Sender<()>>,
) {
    debug!("Waiting for start message on vsock implementation service");
    let (connection_tx, incoming_connections) = mpsc::channel(1);
    let svc = match VsockService::wait_for_start(incoming_connections, &mut request_stream).await {
        Ok(svc) => svc,
        Err(err) => {
            error!("Error while waiting for start message from vsock client: {err:?}");
            return;
        }
    };
    debug!(
        "Received start message on vsock implementation service, waiting for usb socket handles"
    );

    let svc = Arc::new(svc);
    let (mut scopes_stream, scopes) = ScopeStream::new_with_name("usb-vsock-connection".to_owned());

    let usb_callback_handler =
        UsbCallbackHandler { usb_callback_server, connection_tx: connection_tx.clone() };
    let usb_svc = svc.clone();
    scopes.push(async move {
        if let Err(err) = usb_callback_handler.run(usb_svc, synchronized).await {
            error!("Error while waiting for usb device callbacks: {err:?}");
        }
    });
    scopes.push(async move {
        if let Err(err) = svc.run(request_stream).await {
            error!("Error while servicing vsock client: {err:?}");
        }
    });
    // wait for either to finish and then wait for a new client instead.
    scopes_stream.next().await;
}

fn get_usb_device(context: &DriverContext) -> Result<vsockbridge::UsbProxy, Status> {
    let service_proxy = context.incoming.service_marker(vsockbridge::UsbServiceMarker).connect()?;

    service_proxy.connect_to_device().map_err(|err| {
        error!("Error connecting to usb device proxy at driver startup: {err}");
        Status::INTERNAL
    })
}

#[cfg(test)]
mod tests {
    use fidl::endpoints::create_endpoints;
    use fidl_fuchsia_vsock as vsock_api;
    use futures::future::join;
    use log::trace;
    use usb_vsock::CID_ANY;

    use super::*;

    async fn end_to_end_test(
        device_side: impl AsyncFn(vsock_api::ConnectorProxy),
        host_side: impl AsyncFn(
            Arc<Connection<Vec<u8>, Socket>>,
            u32,
            mpsc::Receiver<ConnectionRequest>,
        ),
    ) {
        end_to_end_test_with_version(ProtocolVersion::V2(0x12345678), device_side, host_side).await;
    }

    async fn end_to_end_test_with_version(
        version: ProtocolVersion,
        device_side: impl AsyncFn(vsock_api::ConnectorProxy),
        host_side: impl AsyncFn(
            Arc<Connection<Vec<u8>, Socket>>,
            u32,
            mpsc::Receiver<ConnectionRequest>,
        ),
    ) {
        let scope = Scope::new();
        let (vsock_impl_client, vsock_impl_server) = create_endpoints::<vsock::DeviceMarker>();
        let (usb_callback_client, usb_callback_server) =
            create_endpoints::<vsockbridge::CallbackMarker>();
        let (started_tx, started_rx) = oneshot::channel();
        scope.spawn(run_connection(
            usb_callback_server.into_stream(),
            vsock_impl_server.into_stream(),
            Some(started_tx),
        ));
        let usb_callback_client = usb_callback_client.into_proxy();

        let (vsock_api_service, vsock_api_future) =
            vsock_service_lib::Vsock::new(Some(vsock_impl_client.into_proxy()), None)
                .await
                .unwrap();
        scope.spawn_local(async move {
            vsock_api_future.await.unwrap();
        });

        let (vsock_api_client, vsock_api_server) = create_endpoints::<vsock_api::ConnectorMarker>();
        scope.spawn_local(vsock_api_service.run_client_connection(vsock_api_server.into_stream()));
        let vsock_api_client = vsock_api_client.into_proxy();

        let (usb_packet_socket, usb_packet_server) = zx::Socket::create_datagram();
        let (mut usb_packet_reader, mut usb_packet_writer) =
            Socket::from_socket(usb_packet_socket).split();
        usb_callback_client.new_link(usb_packet_server).await.unwrap();

        let (incoming_tx, incoming_rx) = mpsc::channel(1);
        let host_connection = Arc::new(Connection::new(version, None, incoming_tx));

        // send the initial sync packet with
        let header = &mut Header::new(PacketType::Sync);
        let payload = version.magic();
        header.host_cid.set(CID_HOST);
        header.device_cid.set(CID_ANY);
        header.payload_len.set(payload.len() as u32);
        let sync_packet = Packet { header, payload: &payload };
        let mut buf = [0; 1024];
        sync_packet.write_to_unchecked(&mut buf);
        assert_eq!(
            sync_packet.size(),
            usb_packet_writer.write(&buf[..sync_packet.size()]).await.unwrap()
        );

        // receive sync reply directly (no echoes)
        let mut buf = vec![0; 4096];
        let packet = read_packet_stream(&mut usb_packet_reader, &mut buf)
            .await
            .unwrap()
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        trace!("received packet {packet:?}");
        assert_eq!(packet.header.packet_type, PacketType::Sync);
        assert_eq!(packet.payload, version.magic());
        assert_eq!(packet.header.device_cid.get(), 3);
        assert_eq!(packet.header.host_cid.get(), CID_HOST);

        // send back a different cid just to make sure that works
        let device_cid = 300;
        let header = &mut Header::new(PacketType::Sync);
        let payload = version.magic();
        header.host_cid.set(CID_HOST);
        header.device_cid.set(device_cid);
        header.payload_len.set(payload.len() as u32);
        let sync_packet = Packet { header, payload: &payload };
        let mut buf = [0; 1024];
        sync_packet.write_to_unchecked(&mut buf);
        assert_eq!(
            sync_packet.size(),
            usb_packet_writer.write(&buf[..sync_packet.size()]).await.unwrap()
        );

        started_rx.await.unwrap();

        let writer_connection = host_connection.clone();
        scope.spawn(async move {
            let mut buf = UsbPacketBuilder::new(vec![0; 4096]);
            while let Ok(got) = writer_connection.fill_usb_packet(buf).await {
                buf = got;
                let buf = buf.take_usb_packet().unwrap();
                for packet in VsockPacketIterator::new(buf) {
                    let packet = packet.unwrap();
                    trace!("sending packet {packet:?}");
                }
                let _ = usb_packet_writer.write(buf).await.unwrap();
            }
        });

        let reader_connection = host_connection.clone();
        scope.spawn(async move {
            let mut buf = vec![0; 4096];
            while let Ok(bytes) = usb_packet_reader.read(&mut buf).await {
                for packet in VsockPacketIterator::new(&buf[..bytes]) {
                    let packet = packet.unwrap();
                    trace!("received packet {packet:?}");
                    reader_connection.handle_vsock_packet(packet).await.unwrap();
                }
            }
        });

        let device = device_side(vsock_api_client);
        let host = host_side(host_connection, device_cid, incoming_rx);
        join(device, host).await;
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_device_to_host_connection() {
        end_to_end_test(
            async move |vsock_api_client| {
                let (socket, data) = zx::Socket::create_stream();
                let mut socket = Socket::from_socket(socket);
                let (_con, con) = create_endpoints();
                vsock_api_client
                    .connect(CID_HOST, 200, vsock_api::ConnectionTransport { data, con })
                    .await
                    .unwrap()
                    .map_err(Status::err_from_raw)
                    .unwrap();
                let mut buf = [0; 4];
                socket.read_exact(&mut buf).await.unwrap();
                assert_eq!(&buf, b"boom");
                socket.write_all(b"zoom").await.unwrap();
                assert_eq!(0, socket.read(&mut buf).await.unwrap());
                trace!("vsock api fin");
            },
            async move |host_connection, _device_cid, mut incoming_rx| {
                let incoming = incoming_rx.next().await.unwrap();
                trace!("{incoming:?}");
                let (socket, other_end) = zx::Socket::create_stream();
                let mut socket = Socket::from_socket(socket);
                let _state =
                    host_connection.accept(incoming, Socket::from_socket(other_end)).await.unwrap();
                socket.write_all(b"boom").await.unwrap();
                let mut buf = [0; 4];
                socket.read_exact(&mut buf).await.unwrap();
                assert_eq!(&buf, b"zoom");
                trace!("host fin");
            },
        )
        .await;
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_host_to_device_connection() {
        end_to_end_test(
            async move |vsock_api_client| {
                let (other_end, acceptor) = create_endpoints::<vsock_api::AcceptorMarker>();
                let mut acceptor = acceptor.into_stream();
                vsock_api_client.listen(200, other_end).await.unwrap().unwrap();
                let vsock_api::AcceptorRequest::Accept { addr, responder } =
                    acceptor.next().await.unwrap().unwrap();
                assert_eq!(
                    addr,
                    vsock::Addr { local_port: 200, remote_cid: CID_HOST, remote_port: 9000 }
                );

                let (socket, data) = zx::Socket::create_stream();
                let mut socket = Socket::from_socket(socket);
                let (_con, con) = create_endpoints();
                responder.send(Some(vsock_api::ConnectionTransport { data, con })).unwrap();

                let mut buf = [0; 4];
                socket.read_exact(&mut buf).await.unwrap();
                assert_eq!(&buf, b"boom");
                socket.write_all(b"zoom").await.unwrap();
                assert_eq!(0, socket.read(&mut buf).await.unwrap());
                trace!("vsock api fin");
            },
            async move |host_connection, device_cid, _incoming_rx| {
                let (socket, other_end) = zx::Socket::create_stream();
                let mut socket = Socket::from_socket(socket);
                let _state = host_connection
                    .connect(
                        usb_vsock::Address {
                            host_cid: CID_HOST,
                            host_port: 9000,
                            device_cid,
                            device_port: 200,
                        },
                        Socket::from_socket(other_end),
                    )
                    .await
                    .unwrap();

                socket.write_all(b"boom").await.unwrap();
                let mut buf = [0; 4];
                socket.read_exact(&mut buf).await.unwrap();
                assert_eq!(&buf, b"zoom");
                trace!("host fin");
            },
        )
        .await;
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_downgrade_to_v1() {
        end_to_end_test_with_version(
            ProtocolVersion::V1,
            async move |vsock_api_client| {
                let (socket, data) = zx::Socket::create_stream();
                let mut socket = Socket::from_socket(socket);
                let (_con, con) = create_endpoints();
                vsock_api_client
                    .connect(CID_HOST, 200, vsock_api::ConnectionTransport { data, con })
                    .await
                    .unwrap()
                    .map_err(Status::err_from_raw)
                    .unwrap();
                let mut buf = [0; 4];
                socket.read_exact(&mut buf).await.unwrap();
                assert_eq!(&buf, b"boom");
                socket.write_all(b"zoom").await.unwrap();
                assert_eq!(0, socket.read(&mut buf).await.unwrap());
            },
            async move |host_connection, _device_cid, mut incoming_rx| {
                let incoming = incoming_rx.next().await.unwrap();
                let (socket, other_end) = zx::Socket::create_stream();
                let mut socket = Socket::from_socket(socket);
                let _state =
                    host_connection.accept(incoming, Socket::from_socket(other_end)).await.unwrap();
                socket.write_all(b"boom").await.unwrap();
                let mut buf = [0; 4];
                socket.read_exact(&mut buf).await.unwrap();
                assert_eq!(&buf, b"zoom");
            },
        )
        .await;
    }
}
