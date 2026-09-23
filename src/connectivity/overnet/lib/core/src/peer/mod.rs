// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod framed_stream;

pub(crate) use self::framed_stream::{
    FrameType, FramedStreamReadResult, FramedStreamReader, FramedStreamWriter,
};
use crate::coding::{decode_fidl, encode_fidl};
use crate::future_help::Observer;
use crate::labels::{Endpoint, NodeId, TransferKey};
use crate::router::{FoundTransfer, Router};
use anyhow::{Context as _, Error, bail, format_err};
use fidl::Channel;
use fidl_fuchsia_overnet_protocol::{
    ChannelHandle, ConfigRequest, ConfigResponse, ConnectToService, ConnectToServiceOptions,
    OpenTransfer, PeerDescription, PeerMessage, PeerReply, StreamId, ZirconHandle,
};
use fuchsia_async::{Task, TimeoutExt};
use futures::channel::{mpsc, oneshot};
use futures::lock::Mutex;
use futures::prelude::*;
use std::sync::{Arc, Weak};
use std::time::Duration;

#[derive(Debug)]
struct Config {}

impl Config {
    fn negotiate(_request: ConfigRequest) -> (Self, ConfigResponse) {
        (Config {}, ConfigResponse::default())
    }

    fn from_response(_response: ConfigResponse) -> Self {
        Config {}
    }
}

#[derive(Debug)]
enum ClientPeerCommand {
    ConnectToService(ConnectToService),
    OpenTransfer(u64, TransferKey, oneshot::Sender<()>),
}

#[derive(Clone)]
pub(crate) struct PeerConn {
    conn: circuit::Connection,
    node_id: NodeId,
}

impl std::fmt::Debug for PeerConn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_ref().fmt(f)
    }
}

impl PeerConn {
    pub fn from_circuit(conn: circuit::Connection, node_id: NodeId) -> Self {
        PeerConn { conn, node_id }
    }

    pub fn as_ref(&self) -> PeerConnRef<'_> {
        PeerConnRef { conn: &self.conn, node_id: self.node_id }
    }

    pub fn node_id(&self) -> NodeId {
        self.node_id
    }
}

#[derive(Clone, Copy)]
pub(crate) struct PeerConnRef<'a> {
    conn: &'a circuit::Connection,
    node_id: NodeId,
}

impl<'a> std::fmt::Debug for PeerConnRef<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PeerConn({})", self.node_id.0)
    }
}

impl<'a> PeerConnRef<'a> {
    pub fn from_circuit(conn: &'a circuit::Connection, node_id: NodeId) -> Self {
        PeerConnRef { conn, node_id }
    }

    pub fn into_peer_conn(&self) -> PeerConn {
        PeerConn { conn: self.conn.clone(), node_id: self.node_id }
    }

    pub fn endpoint(&self) -> Endpoint {
        if self.conn.is_client() { Endpoint::Client } else { Endpoint::Server }
    }

    pub fn peer_node_id(&self) -> NodeId {
        self.node_id
    }

    pub async fn alloc_uni(&self) -> Result<FramedStreamWriter, Error> {
        let (circuit_reader, writer) = circuit::stream::stream();
        let (_reader, circuit_writer) = circuit::stream::stream();
        let id = self.conn.alloc_stream(circuit_reader, circuit_writer).await?;
        Ok(FramedStreamWriter::from_circuit(writer, id, self.conn.clone(), self.node_id))
    }

    pub async fn alloc_bidi(&self) -> Result<(FramedStreamWriter, FramedStreamReader), Error> {
        let (circuit_reader, writer) = circuit::stream::stream();
        let (reader, circuit_writer) = circuit::stream::stream();
        let id = self.conn.alloc_stream(circuit_reader, circuit_writer).await?;
        Ok((
            FramedStreamWriter::from_circuit(writer, id, self.conn.clone(), self.node_id),
            FramedStreamReader::from_circuit(reader, self.conn.clone(), self.node_id),
        ))
    }

    pub async fn bind_uni_id(&self, id: u64) -> Result<FramedStreamReader, Error> {
        Ok(FramedStreamReader::from_circuit(
            self.conn
                .bind_stream(id)
                .await
                .ok_or_else(|| format_err!("Stream id {} unavailable", id))?
                .0,
            self.conn.clone(),
            self.node_id,
        ))
    }

    pub async fn bind_id(
        &self,
        id: u64,
    ) -> Result<(FramedStreamWriter, FramedStreamReader), Error> {
        let (r, w) = self
            .conn
            .bind_stream(id)
            .await
            .ok_or_else(|| format_err!("Stream id {} unavailable", id))?;
        Ok((
            FramedStreamWriter::from_circuit(w, id, self.conn.clone(), self.node_id),
            FramedStreamReader::from_circuit(r, self.conn.clone(), self.node_id),
        ))
    }
}

pub(crate) struct Peer {
    endpoint: Endpoint,
    conn: PeerConn,
    commands: Option<mpsc::Sender<ClientPeerCommand>>,
    _task: Task<()>,
}

impl std::fmt::Debug for Peer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.debug_id().fmt(f)
    }
}

/// Error from the run loops for a peer (client or server) - captures a little semantic detail
/// to help direct reactions to this peer disappearing.
#[derive(Debug)]
enum RunnerError {
    RouterGone,
    ConnectionClosed(Option<String>),
    BadFrameType { _frame_type: FrameType },
    HandshakeError { _error: Error },
    ServiceError { _error: Error },
}

impl Peer {
    pub(crate) fn node_id(&self) -> NodeId {
        self.conn.node_id()
    }

    pub(crate) fn debug_id(&self) -> impl std::fmt::Debug + std::cmp::PartialEq {
        (self.node_id(), self.endpoint)
    }

    /// Construct a new client peer - spawns tasks to handle making control stream requests, and
    /// publishing link metadata
    pub(crate) fn new_circuit_client(
        conn: circuit::Connection,
        conn_stream_writer: circuit::stream::Writer,
        conn_stream_reader: circuit::stream::Reader,
        service_observer: Observer<Vec<String>>,
        router: &Arc<Router>,
        peer_node_id: NodeId,
    ) -> Result<Arc<Self>, Error> {
        let node_id =
            NodeId::from_circuit_string(conn.from()).map_err(|_| format_err!("Invalid node ID"))?;
        log::trace!(node_id = router.node_id().0, peer = node_id.0; "NEW CLIENT",);
        let (command_sender, command_receiver) = mpsc::channel(1);
        let weak_router = Arc::downgrade(router);

        let client_conn_fut = client_conn_stream(
            Arc::downgrade(router),
            node_id,
            conn_stream_writer,
            conn_stream_reader,
            conn.clone(),
            command_receiver,
            service_observer,
        );

        Ok(Arc::new(Self {
            endpoint: Endpoint::Client,
            commands: Some(command_sender),
            _task: Task::spawn(Peer::runner(Endpoint::Client, weak_router.clone(), async move {
                let result = client_conn_fut.await;
                if let Some(router) = weak_router.upgrade() {
                    router.client_closed(peer_node_id).await;
                }
                result
            })),
            conn: PeerConn::from_circuit(conn, node_id),
        }))
    }

    /// Construct a new server peer - spawns tasks to handle responding to control stream requests
    pub(crate) async fn new_circuit_server(
        conn: circuit::Connection,
        router: &Arc<Router>,
    ) -> Result<(), Error> {
        let node_id =
            NodeId::from_circuit_string(conn.from()).map_err(|_| format_err!("Invalid node ID"))?;
        log::trace!(node_id = router.node_id().0, peer = node_id.0; "NEW SERVER",);
        let (conn_stream_reader, conn_stream_writer) = conn
            .bind_stream(0)
            .await
            .ok_or_else(|| format_err!("Could not establish connection"))?;
        Task::spawn(Peer::runner(
            Endpoint::Server,
            Arc::downgrade(router),
            server_conn_stream(
                node_id,
                conn_stream_writer,
                conn_stream_reader,
                conn.clone(),
                Arc::downgrade(router),
            ),
        ))
        .detach();
        Ok(())
    }

    async fn runner(
        endpoint: Endpoint,
        router: Weak<Router>,
        f: impl Future<Output = Result<(), RunnerError>>,
    ) {
        let result = f.await;
        let get_router_node_id = || {
            Weak::upgrade(&router).map(|r| format!("{:?}", r.node_id())).unwrap_or_else(String::new)
        };
        if let Err(e) = &result {
            match e {
                RunnerError::ConnectionClosed(Some(s)) => log::debug!(
                    node_id:% = get_router_node_id(),
                    endpoint:?;
                    "connection closed (reason: {s})"
                ),
                RunnerError::ConnectionClosed(None) => log::debug!(
                    node_id:% = get_router_node_id(),
                    endpoint:?;
                    "connection closed"
                ),
                e @ RunnerError::HandshakeError { .. } => {
                    log::debug!(
                        node_id:% = get_router_node_id(),
                        endpoint:?;
                        "runner error during handshake: {:?}",
                        e
                    )
                }
                _ => log::warn!(
                    node_id:% = get_router_node_id(),
                    endpoint:?;
                    "runner error: {:?}",
                    e
                ),
            }
        } else {
            log::trace!(
                node_id:% = get_router_node_id(),
                endpoint:?;
                "finished successfully",
            );
        }
    }

    pub async fn new_stream(
        &self,
        service: &str,
        chan: Channel,
        router: &Arc<Router>,
    ) -> Result<(), Error> {
        if let ZirconHandle::Channel(ChannelHandle { stream_ref, rights }) =
            router.send_proxied(chan.into_handle(), self.peer_conn_ref()).await?
        {
            self.commands
                .as_ref()
                .unwrap()
                .clone()
                .send(ClientPeerCommand::ConnectToService(ConnectToService {
                    service_name: service.to_string(),
                    stream_ref,
                    rights,
                    options: ConnectToServiceOptions::default(),
                }))
                .await?;
            Ok(())
        } else {
            unreachable!();
        }
    }

    pub async fn send_open_transfer(
        &self,
        transfer_key: TransferKey,
    ) -> Option<(FramedStreamWriter, FramedStreamReader)> {
        let io = self.peer_conn_ref().alloc_bidi().await.ok()?;
        let (tx, rx) = oneshot::channel();
        self.commands
            .as_ref()
            .unwrap()
            .clone()
            .send(ClientPeerCommand::OpenTransfer(io.0.id(), transfer_key, tx))
            .await
            .ok()?;
        rx.await.ok()?;
        Some(io)
    }

    fn peer_conn_ref(&self) -> PeerConnRef<'_> {
        self.conn.as_ref()
    }
}

const QUIC_CONNECTION_TIMEOUT: Duration = Duration::from_secs(60);

async fn client_handshake(
    my_node_id: NodeId,
    peer_node_id: NodeId,
    writer: circuit::stream::Writer,
    reader: circuit::stream::Reader,
    conn: circuit::Connection,
) -> Result<(FramedStreamWriter, FramedStreamReader), Error> {
    log::trace!(
        my_node_id = my_node_id.0,
        clipeer = peer_node_id.0;
        "client connection stream started",
    );
    // Send FIDL header
    log::trace!(
        my_node_id = my_node_id.0,
        clipeer:? = peer_node_id;
        "send fidl header"
    );
    let msg = [0, 0, 0, fidl::encoding::MAGIC_NUMBER_INITIAL];
    writer.write(msg.len(), |buf| {
        buf[..msg.len()].copy_from_slice(&msg);
        Ok(msg.len())
    })?;
    async move {
        log::trace!(my_node_id = my_node_id.0, clipeer = peer_node_id.0; "send config request");
        // Send config request
        let mut conn_stream_writer =
            FramedStreamWriter::from_circuit(writer, 0, conn.clone(), peer_node_id);
        let conn_stream_reader_fut = async move {
            // Receive FIDL header
            log::trace!(my_node_id = my_node_id.0, clipeer = peer_node_id.0; "read fidl header");
            reader.read(4, |_| Ok(((), 4))).await?;
            Result::<_, Error>::Ok(FramedStreamReader::from_circuit(reader, conn, peer_node_id))
        }
        .boxed();

        conn_stream_writer
            .send(FrameType::Data, &encode_fidl(&mut ConfigRequest::default().clone())?)
            .await?;
        // Await config response
        log::trace!(my_node_id = my_node_id.0, clipeer = peer_node_id.0; "read config");
        let mut conn_stream_reader = conn_stream_reader_fut.await?;
        let _ = Config::from_response(match conn_stream_reader.next().await? {
            FramedStreamReadResult::Frame(FrameType::Data, mut bytes) => decode_fidl(&mut bytes)?,
            FramedStreamReadResult::Frame(_, _) => {
                bail!("Failed to read config response (wrong frame type)")
            }
            FramedStreamReadResult::Closed(Some(s)) => {
                bail!("Failed to read config response ({s})")
            }
            FramedStreamReadResult::Closed(None) => bail!("Failed to read config response"),
        });
        log::trace!(my_node_id = my_node_id.0, clipeer = peer_node_id.0; "handshake completed");

        Ok((conn_stream_writer, conn_stream_reader))
    }
    .on_timeout(QUIC_CONNECTION_TIMEOUT, || Err(format_err!("timeout performing handshake")))
    .await
}

struct TrackClientConnection {
    router: Weak<Router>,
    node_id: NodeId,
}

impl TrackClientConnection {
    async fn new(router: &Arc<Router>, node_id: NodeId) -> TrackClientConnection {
        router.service_map().add_client_connection(node_id).await;
        TrackClientConnection { router: Arc::downgrade(router), node_id }
    }
}

impl Drop for TrackClientConnection {
    fn drop(&mut self) {
        if let Some(router) = Weak::upgrade(&self.router) {
            let node_id = self.node_id;
            Task::spawn(
                async move { router.service_map().remove_client_connection(node_id).await },
            )
            .detach();
        }
    }
}

async fn client_conn_stream(
    router: Weak<Router>,
    peer_node_id: NodeId,
    writer: circuit::stream::Writer,
    reader: circuit::stream::Reader,
    conn: circuit::Connection,
    mut commands: mpsc::Receiver<ClientPeerCommand>,
    mut services: Observer<Vec<String>>,
) -> Result<(), RunnerError> {
    let get_router = move || Weak::upgrade(&router).ok_or_else(|| RunnerError::RouterGone);
    let my_node_id = get_router()?.node_id();

    let (conn_stream_writer, mut conn_stream_reader) =
        client_handshake(my_node_id, peer_node_id, writer, reader, conn)
            .map_err(|e| RunnerError::HandshakeError { _error: e })
            .await?;

    let _track_connection = TrackClientConnection::new(&get_router()?, peer_node_id).await;

    let conn_stream_writer = &Mutex::new(conn_stream_writer);

    let _: ((), (), ()) = futures::future::try_join3(
        async move {
            while let Some(command) = commands.next().await {
                log::trace!(
                    my_node_id = my_node_id.0,
                    clipeer = peer_node_id.0;
                    "handle command: {:?}",
                    command
                );
                client_conn_handle_command(command, &mut *conn_stream_writer.lock().await).await?;
            }
            log::trace!(my_node_id = my_node_id.0, clipeer = peer_node_id.0; "done commands");
            Ok(())
        }
        .map_err(|e| RunnerError::ServiceError { _error: e }),
        async move {
            loop {
                let (frame_type, mut bytes) = match conn_stream_reader
                    .next()
                    .await
                    .map_err(|e| RunnerError::ServiceError { _error: e })?
                {
                    FramedStreamReadResult::Frame(frame_type, bytes) => (frame_type, bytes),
                    FramedStreamReadResult::Closed(s) => {
                        return Err(RunnerError::ConnectionClosed(s));
                    }
                };
                match frame_type {
                    FrameType::Hello | FrameType::Control | FrameType::Signal => {
                        return Err(RunnerError::BadFrameType { _frame_type: frame_type });
                    }
                    FrameType::Data => {
                        client_conn_handle_incoming_frame(my_node_id, peer_node_id, &mut bytes)
                            .await
                            .map_err(|e| RunnerError::ServiceError { _error: e })?;
                    }
                }
            }
        },
        async move {
            loop {
                let services = services.next().await;
                log::trace!(
                    my_node_id = my_node_id.0,
                    clipeer = peer_node_id.0;
                    "Send update node description with services: {:?}",
                    services
                );
                conn_stream_writer
                    .lock()
                    .await
                    .send(
                        FrameType::Data,
                        &encode_fidl(&mut PeerMessage::UpdateNodeDescription(PeerDescription {
                            services,
                            ..Default::default()
                        }))?,
                    )
                    .await?;
            }
        }
        .map_err(|e| RunnerError::ServiceError { _error: e }),
    )
    .await?;

    Ok(())
}

async fn client_conn_handle_command(
    command: ClientPeerCommand,
    conn_stream_writer: &mut FramedStreamWriter,
) -> Result<(), Error> {
    match command {
        ClientPeerCommand::ConnectToService(conn) => {
            conn_stream_writer
                .send(FrameType::Data, &encode_fidl(&mut PeerMessage::ConnectToService(conn))?)
                .await?;
        }
        ClientPeerCommand::OpenTransfer(stream_id, transfer_key, sent) => {
            conn_stream_writer
                .send(
                    FrameType::Data,
                    &encode_fidl(&mut PeerMessage::OpenTransfer(OpenTransfer {
                        stream_id: StreamId { id: stream_id },
                        transfer_key,
                    }))?,
                )
                .await?;
            let _ = sent.send(());
        }
    }
    Ok(())
}

async fn client_conn_handle_incoming_frame(
    my_node_id: NodeId,
    peer_node_id: NodeId,
    bytes: &mut [u8],
) -> Result<(), Error> {
    let msg: PeerReply = decode_fidl(bytes)?;
    log::trace!(my_node_id = my_node_id.0, clipeer = peer_node_id.0; "got reply {:?}", msg);
    match msg {
        PeerReply::UpdateLinkStatusAck(_) => {
            // XXX(raggi): prior code attempted to send to a None under a lock
            // here, seemingly unused, but may have influenced total ordering?
        }
    }
    Ok(())
}

async fn server_handshake(
    my_node_id: NodeId,
    node_id: NodeId,
    writer: circuit::stream::Writer,
    reader: circuit::stream::Reader,
    conn: circuit::Connection,
) -> Result<(FramedStreamWriter, FramedStreamReader), Error> {
    // Receive FIDL header
    log::trace!(my_node_id = my_node_id.0, svrpeer = node_id.0; "read fidl header");
    reader.read(4, |_| Ok(((), 4))).await.context("reading FIDL header")?;
    // Send FIDL header
    log::trace!(
        my_node_id = my_node_id.0,
        svrpeer:? = node_id;
        "send fidl header"
    );
    let handshake = [0, 0, 0, fidl::encoding::MAGIC_NUMBER_INITIAL];
    writer.write(handshake.len(), |buf| {
        buf[..handshake.len()].copy_from_slice(&handshake);
        Ok(handshake.len())
    })?;
    let mut conn_stream_reader = FramedStreamReader::from_circuit(reader, conn.clone(), node_id);
    let mut conn_stream_writer = FramedStreamWriter::from_circuit(writer, 0, conn.clone(), node_id);
    // Await config request
    log::trace!(my_node_id = my_node_id.0, svrpeer = node_id.0; "read config");
    let (_, mut response) = Config::negotiate(match conn_stream_reader.next().await? {
        FramedStreamReadResult::Frame(FrameType::Data, mut bytes) => decode_fidl(&mut bytes)?,
        FramedStreamReadResult::Frame(_, _) => {
            bail!("Failed to read config response (wrong frame type)")
        }
        FramedStreamReadResult::Closed(Some(s)) => {
            bail!("Failed to read config response (Connection closed: {s})")
        }
        FramedStreamReadResult::Closed(None) => {
            bail!("Failed to read config response (Connection closed)")
        }
    });
    // Send config response
    log::trace!(my_node_id = my_node_id.0, svrpeer = node_id.0; "send config");
    conn_stream_writer.send(FrameType::Data, &encode_fidl(&mut response)?).await?;
    Ok((conn_stream_writer, conn_stream_reader))
}

async fn server_conn_stream(
    node_id: NodeId,
    writer: circuit::stream::Writer,
    reader: circuit::stream::Reader,
    conn: circuit::Connection,
    router: Weak<Router>,
) -> Result<(), RunnerError> {
    let my_node_id = Weak::upgrade(&router).ok_or_else(|| RunnerError::RouterGone)?.node_id();
    let (conn_stream_writer, mut conn_stream_reader) =
        server_handshake(my_node_id, node_id, writer, reader, conn)
            .map_err(|e| RunnerError::HandshakeError { _error: e })
            .await?;

    loop {
        log::trace!(my_node_id = my_node_id.0, svrpeer = node_id.0; "await message");
        let (frame_type, mut bytes) = match conn_stream_reader
            .next()
            .await
            .map_err(|e| RunnerError::ServiceError { _error: e })?
        {
            FramedStreamReadResult::Frame(frame_type, bytes) => (frame_type, bytes),
            FramedStreamReadResult::Closed(s) => return Err(RunnerError::ConnectionClosed(s)),
        };

        let router = Weak::upgrade(&router).ok_or_else(|| RunnerError::RouterGone)?;
        match frame_type {
            FrameType::Hello | FrameType::Control | FrameType::Signal => {
                return Err(RunnerError::BadFrameType { _frame_type: frame_type });
            }
            FrameType::Data => {
                let msg: PeerMessage =
                    decode_fidl(&mut bytes).map_err(|e| RunnerError::ServiceError { _error: e })?;
                log::trace!(
                    my_node_id = my_node_id.0,
                    svrpeer = node_id.0;
                    "Got peer request: {:?}",
                    msg
                );
                match msg {
                    PeerMessage::ConnectToService(ConnectToService {
                        service_name,
                        stream_ref,
                        rights,
                        options: _,
                    }) => {
                        let app_channel = Channel::from(
                            router
                                .recv_proxied(
                                    ZirconHandle::Channel(ChannelHandle { stream_ref, rights }),
                                    conn_stream_writer.conn(),
                                )
                                .map_err(|e| RunnerError::ServiceError { _error: e })
                                .await?,
                        );
                        router
                            .service_map()
                            .connect(&service_name, app_channel)
                            .map_err(|e| RunnerError::ServiceError { _error: e })
                            .await?;
                    }
                    PeerMessage::UpdateNodeDescription(PeerDescription { services, .. }) => {
                        router
                            .service_map()
                            .update_node(node_id, services.unwrap_or(vec![]))
                            .map_err(|e| RunnerError::ServiceError { _error: e })
                            .await?;
                    }
                    PeerMessage::OpenTransfer(OpenTransfer {
                        stream_id: StreamId { id: stream_id },
                        transfer_key,
                    }) => {
                        let (tx, rx) = conn_stream_writer
                            .conn()
                            .bind_id(stream_id)
                            .await
                            .map_err(|e| RunnerError::ServiceError { _error: e })?;
                        router
                            .post_transfer(transfer_key, FoundTransfer::Remote(tx, rx))
                            .map_err(|e| RunnerError::ServiceError { _error: e })
                            .await?;
                    }
                }
            }
        }
    }
}
