// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Manages peer<->peer connections and routing packets over links between nodes.

// General structure:
// A router is a collection of: - streams (each with a StreamId<LinkData>)
//                                These are streams of information flow between processes.
//                              - peers (each with a PeerId)
//                                These are other overnet instances in the overlay network.
//                                There is a client oriented and a server oriented peer per
//                                node.
//                              - links (each with a LinkId)
//                                These are connections between this instance and other
//                                instances in the mesh.
// For each node in the mesh, a routing table tracks which link on which to send data to
// that node (said link may be a third node that will be requested to forward datagrams
// on our behalf).

mod service_map;

use self::service_map::ServiceMap;
use crate::future_help::{Observer, log_errors};
use crate::handle_info::{HandleKey, HandleType, handle_info};
use crate::labels::{NodeId, TransferKey};
use crate::peer::{FramedStreamReader, FramedStreamWriter, Peer, PeerConnRef};
use crate::proxy::{
    IntoProxied, ProxyTransferInitiationReceiver, RemoveFromProxyTable, StreamRefSender,
};
use anyhow::{Context as _, Error, bail, format_err};
use async_utils::mutex_ticket::MutexTicket;
use fidl::{Channel, EventPair, NullableHandle, Socket};
use fidl_fuchsia_overnet_protocol::{
    ChannelHandle, EventPairHandle, EventPairRights, SocketHandle, SocketType, StreamId, StreamRef,
    ZirconHandle,
};
use fuchsia_async::Task;
use futures::channel::oneshot;
use futures::future::poll_fn;
use futures::lock::Mutex;
use futures::prelude::*;
use futures::ready;
use std::collections::{BTreeMap, HashMap};
use std::pin::pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

pub use self::service_map::ListablePeer;

#[derive(Debug)]
enum PendingTransfer {
    Complete(FoundTransfer),
    Waiting(Waker),
}

type PendingTransferMap = BTreeMap<TransferKey, PendingTransfer>;

#[derive(Debug)]
pub(crate) enum FoundTransfer {
    Fused(NullableHandle),
    Remote(FramedStreamWriter, FramedStreamReader),
}

#[derive(Debug)]
pub(crate) enum OpenedTransfer {
    Fused,
    Remote(FramedStreamWriter, FramedStreamReader, NullableHandle),
}

#[derive(Debug)]
#[allow(dead_code)]
enum CircuitState {
    Waiters(Vec<oneshot::Sender<()>>),
    Peer(Arc<Peer>),
}

impl CircuitState {
    fn peer(&self) -> Option<Arc<Peer>> {
        if let CircuitState::Peer(peer) = self { Some(Arc::clone(peer)) } else { None }
    }
}

#[derive(Debug)]
struct PeerMaps {
    circuit_clients: BTreeMap<NodeId, CircuitState>,
}

/// Wrapper to get the right list_peers behavior.
#[derive(Debug)]
pub struct ListPeersContext(Mutex<Option<Observer<Vec<ListablePeer>>>>);

static LIST_PEERS_CALL: AtomicU64 = AtomicU64::new(0);

impl ListPeersContext {
    /// Implementation of ListPeers fidl method.
    pub async fn list_peers(&self) -> Result<Vec<ListablePeer>, Error> {
        let call_id = LIST_PEERS_CALL.fetch_add(1, Ordering::SeqCst);
        log::trace!(list_peers_call = call_id; "get observer");
        let mut obs = self
            .0
            .lock()
            .await
            .take()
            .ok_or_else(|| anyhow::format_err!("Already listing peers"))?;
        log::trace!(list_peers_call = call_id; "wait for value");
        let r = obs.next().await;
        log::trace!(list_peers_call = call_id; "replace observer");
        *self.0.lock().await = Some(obs);
        log::trace!(list_peers_call = call_id; "return");
        Ok(r.unwrap_or_else(Vec::new))
    }
}

/// Whether this node's ascendd clients should be routed to each other
pub enum AscenddClientRouting {
    /// Ascendd client routing is allowed
    Enabled,
    /// Ascendd client routing is prevented
    Disabled,
}

/// Router maintains global state for one node_id.
/// `LinkData` is a token identifying a link for layers above Router.
/// `Time` is a representation of time for the Router, to assist injecting different platforms
/// schemes.
pub struct Router {
    /// Our node id
    node_id: NodeId,
    /// All peers.
    peers: Mutex<PeerMaps>,
    service_map: ServiceMap,
    proxied_streams: Mutex<HashMap<HandleKey, ProxiedHandle>>,
    pending_transfers: Mutex<PendingTransferMap>,
    task: Mutex<Option<Task<()>>>,
    /// Hack to prevent the n^2 scaling of a fully-connected graph of ffxs
    ascendd_client_routing: AtomicBool,
    circuit_node: circuit::ConnectionNode,
}

struct ProxiedHandle {
    remove_sender: futures::channel::oneshot::Sender<RemoveFromProxyTable>,
    original_paired: HandleKey,
    proxy_task: Task<()>,
}

/// Generate a new random node id
fn generate_node_id() -> NodeId {
    rand::random::<u64>().into()
}

fn sorted<T: std::cmp::Ord>(mut v: Vec<T>) -> Vec<T> {
    v.sort();
    v
}

impl std::fmt::Debug for Router {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Router({:?})", self.node_id)
    }
}

/// This is sent when initiating connections between circuit nodes and must be identical between all
/// such nodes.
const OVERNET_CIRCUIT_PROTOCOL: &'static str = "Overnet:0";

impl Router {
    /// Create a new router. If `router_interval` is given, this router will
    /// behave like an interior node and tell its neighbors about each other.
    pub fn new(router_interval: Option<Duration>) -> Result<Arc<Self>, Error> {
        Router::with_node_id(generate_node_id(), router_interval)
    }

    /// Make a router with a specific node ID.
    pub fn with_node_id(
        node_id: NodeId,
        router_interval: Option<Duration>,
    ) -> Result<Arc<Self>, Error> {
        let service_map = ServiceMap::new(node_id);
        let (new_peer_sender, new_peer_receiver) = futures::channel::mpsc::channel(1);
        let (circuit_node, circuit_connections) = if let Some(interval) = router_interval {
            let (a, b) = circuit::ConnectionNode::new_with_router(
                &node_id.circuit_string(),
                OVERNET_CIRCUIT_PROTOCOL,
                interval,
                new_peer_sender,
            )?;
            (a, b.boxed())
        } else {
            let (a, b) = circuit::ConnectionNode::new(
                &node_id.circuit_string(),
                OVERNET_CIRCUIT_PROTOCOL,
                new_peer_sender,
            )?;
            (a, b.boxed())
        };
        let router = Arc::new(Router {
            node_id,
            service_map,
            peers: Mutex::new(PeerMaps { circuit_clients: BTreeMap::new() }),
            proxied_streams: Mutex::new(HashMap::new()),
            pending_transfers: Mutex::new(PendingTransferMap::new()),
            task: Mutex::new(None),
            // Default is to route all clients to each other.
            ascendd_client_routing: AtomicBool::new(true),
            circuit_node,
        });

        let weak_router = Arc::downgrade(&router);
        *router.task.lock().now_or_never().unwrap() = Some(Task::spawn(log_errors(
            run_circuits(weak_router, circuit_connections, new_peer_receiver),
            format!("router {:?} support loop failed", node_id),
        )));

        Ok(router)
    }

    /// Get the circuit protocol node for this router. This will let us create new connections to
    /// other nodes.
    pub fn circuit_node(&self) -> &circuit::Node {
        self.circuit_node.node()
    }

    /// Accessor for the node id of this router.
    pub fn node_id(&self) -> NodeId {
        self.node_id
    }

    /// Accessor for whether to route ascendd clients to each other
    pub fn client_routing(&self) -> AscenddClientRouting {
        if self.ascendd_client_routing.load(std::sync::atomic::Ordering::SeqCst) {
            AscenddClientRouting::Enabled
        } else {
            AscenddClientRouting::Disabled
        }
    }

    /// Setter for whether to route ascendd clients to each other
    pub fn set_client_routing(&self, client_routing: AscenddClientRouting) {
        let client_routing = match client_routing {
            AscenddClientRouting::Enabled => true,
            AscenddClientRouting::Disabled => false,
        };
        self.ascendd_client_routing.store(client_routing, std::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) fn service_map(&self) -> &ServiceMap {
        &self.service_map
    }

    /// Create a new stream to advertised service `service` on remote node id `node`.
    pub async fn connect_to_service(
        self: &Arc<Self>,
        node_id: NodeId,
        service_name: &str,
        chan: Channel,
    ) -> Result<(), Error> {
        let is_local = node_id == self.node_id;
        log::trace!(
            service_name:%,
            node_id = node_id.0,
            local = is_local;
            "Request connect_to_service",
        );
        if is_local {
            self.service_map().connect(service_name, chan).await
        } else {
            self.client_peer(node_id)
                .await
                .with_context(|| {
                    format_err!(
                        "Fetching client peer for new stream to {:?} for service {:?}",
                        node_id,
                        service_name,
                    )
                })?
                .new_stream(service_name, chan, self)
                .await
        }
    }

    /// Register a service. The callback should connect the given channel to the
    /// service in question.
    pub async fn register_service(
        &self,
        service_name: String,
        provider: impl Fn(fidl::Channel) -> Result<(), Error> + Send + 'static,
    ) -> Result<(), Error> {
        self.service_map().register_service(service_name, provider).await;
        Ok(())
    }

    /// Create a new list_peers context
    pub async fn new_list_peers_context(&self) -> ListPeersContext {
        ListPeersContext(Mutex::new(Some(self.service_map.new_list_peers_observer().await)))
    }

    async fn client_peer(self: &Arc<Self>, peer_node_id: NodeId) -> Result<Arc<Peer>, Error> {
        loop {
            let mut peers = self.peers.lock().await;
            match peers.circuit_clients.get_mut(&peer_node_id) {
                Some(CircuitState::Peer(peer)) => break Ok(Arc::clone(&peer)),
                Some(CircuitState::Waiters(waiters)) => {
                    let (sender, receiver) = oneshot::channel();
                    waiters.push(sender);
                    std::mem::drop(peers);
                    let _ = receiver.await;
                }
                None => {
                    peers.circuit_clients.insert(peer_node_id, CircuitState::Waiters(Vec::new()));
                }
            }
        }
    }

    fn add_proxied(
        self: &Arc<Self>,
        proxied_streams: &mut HashMap<HandleKey, ProxiedHandle>,
        this_handle_key: HandleKey,
        pair_handle_key: HandleKey,
        remove_sender: futures::channel::oneshot::Sender<RemoveFromProxyTable>,
        f: impl 'static + Send + Future<Output = Result<(), Error>>,
    ) {
        let router = Arc::downgrade(&self);
        let proxy_task = Task::spawn(async move {
            if let Err(e) = f.await {
                log::trace!(this_handle_key:?, pair_handle_key:?; "Proxy failed: {:?}", e);
            } else {
                log::trace!(this_handle_key:?, pair_handle_key:?; "Proxy completed successfully",);
            }
            if let Some(router) = Weak::upgrade(&router) {
                router.remove_proxied(this_handle_key, pair_handle_key).await;
            }
        });
        assert!(
            proxied_streams
                .insert(
                    this_handle_key,
                    ProxiedHandle { remove_sender, original_paired: pair_handle_key, proxy_task },
                )
                .is_none()
        );
    }

    // Remove a proxied handle from our table.
    // Called by proxy::Proxy::drop.
    async fn remove_proxied(
        self: &Arc<Self>,
        this_handle_key: HandleKey,
        pair_handle_key: HandleKey,
    ) {
        let mut proxied_streams = self.proxied_streams.lock().await;
        log::trace!(
            node_id = self.node_id.0,
            this_handle_key:?,
            pair_handle_key:?,
            all:? = sorted(proxied_streams.keys().map(|x| *x).collect::<Vec<_>>());
            "REMOVE_PROXIED",
        );
        if let Some(removed) = proxied_streams.remove(&this_handle_key) {
            assert_eq!(removed.original_paired, pair_handle_key);
            let _ = removed.remove_sender.send(RemoveFromProxyTable::Dropped);
            removed.proxy_task.detach();
        }
    }

    // Prepare a handle to be sent to another machine.
    // Returns a ZirconHandle describing the established proxy.
    pub(crate) async fn send_proxied(
        self: &Arc<Self>,
        handle: NullableHandle,
        conn: PeerConnRef<'_>,
    ) -> Result<ZirconHandle, Error> {
        let raw_handle = handle.raw_handle(); // for debugging
        let info = handle_info(handle.as_handle_ref())
            .with_context(|| format!("Getting handle information for {}", raw_handle))?;
        let mut proxied_streams = self.proxied_streams.lock().await;
        log::trace!(
            node_id = self.node_id.0,
            handle:?,
            info:?,
            all:? = sorted(proxied_streams.keys().map(|x| *x).collect::<Vec<_>>());
            "SEND_PROXIED",
        );
        if let Some(pair) = proxied_streams.remove(&info.pair_handle_key) {
            // This handle is the other end of an already proxied object...
            // Here we need to inform the existing proxy loop that a transfer is going to be
            // initiated, and to where.
            drop(proxied_streams);
            assert_eq!(info.this_handle_key, pair.original_paired);
            log::trace!(
                handle:?,
                orig_pair:? = pair.original_paired;
                "Send paired proxied"
            );
            // We allocate a drain stream to flush any messages we've buffered locally to the new
            // endpoint.
            let drain_stream = conn.alloc_uni().await?.into();
            let (stream_ref_sender, stream_ref_receiver) = StreamRefSender::new();
            pair.remove_sender
                .send(RemoveFromProxyTable::InitiateTransfer {
                    paired_handle: handle,
                    drain_stream,
                    stream_ref_sender,
                })
                .map_err(|_| format_err!("Failed to initiate transfer"))?;
            let stream_ref = stream_ref_receiver
                .await
                .with_context(|| format!("waiting for stream_ref for {:?}", raw_handle))?;
            pair.proxy_task.detach();
            match info.handle_type {
                HandleType::Channel(rights) => {
                    Ok(ZirconHandle::Channel(ChannelHandle { stream_ref, rights }))
                }
                HandleType::Socket(socket_type, rights) => {
                    Ok(ZirconHandle::Socket(SocketHandle { stream_ref, socket_type, rights }))
                }
                HandleType::EventPair => Ok(ZirconHandle::EventPair(EventPairHandle {
                    stream_ref,
                    rights: EventPairRights::empty(),
                })),
            }
        } else {
            // This handle (and its pair) is previously unseen... establish a proxy stream for it
            log::trace!(handle:?; "Send proxied");
            let (tx, rx) = futures::channel::oneshot::channel();
            let rx = ProxyTransferInitiationReceiver::new(rx.map_err(move |_| {
                format_err!(
                    "cancelled transfer via send_proxied {:?}\n{}",
                    info,
                    111 //std::backtrace::Backtrace::force_capture()
                )
            }));
            let (stream_writer, stream_reader) = conn.alloc_bidi().await?;
            let stream_ref = StreamRef::Creating(StreamId { id: stream_writer.id() });
            Ok(match info.handle_type {
                HandleType::Channel(rights) => {
                    self.add_proxied(
                        &mut *proxied_streams,
                        info.this_handle_key,
                        info.pair_handle_key,
                        tx,
                        crate::proxy::spawn_send(
                            Channel::from(handle).into_proxied()?,
                            rx,
                            stream_writer.into(),
                            stream_reader.into(),
                            Arc::downgrade(&self),
                        ),
                    );
                    ZirconHandle::Channel(ChannelHandle { stream_ref, rights })
                }
                HandleType::Socket(socket_type, rights) => {
                    self.add_proxied(
                        &mut *proxied_streams,
                        info.this_handle_key,
                        info.pair_handle_key,
                        tx,
                        crate::proxy::spawn_send(
                            Socket::from(handle).into_proxied()?,
                            rx,
                            stream_writer.into(),
                            stream_reader.into(),
                            Arc::downgrade(&self),
                        ),
                    );
                    ZirconHandle::Socket(SocketHandle { stream_ref, socket_type, rights })
                }
                HandleType::EventPair => {
                    self.add_proxied(
                        &mut *proxied_streams,
                        info.this_handle_key,
                        info.pair_handle_key,
                        tx,
                        crate::proxy::spawn_send(
                            EventPair::from(handle).into_proxied()?,
                            rx,
                            stream_writer.into(),
                            stream_reader.into(),
                            Arc::downgrade(&self),
                        ),
                    );
                    ZirconHandle::EventPair(EventPairHandle {
                        stream_ref,
                        rights: EventPairRights::empty(),
                    })
                }
            })
        }
    }

    // Take a received handle description and construct a fidl::NullableHandle that represents it
    // whilst establishing proxies as required
    pub(crate) async fn recv_proxied(
        self: &Arc<Self>,
        handle: ZirconHandle,
        conn: PeerConnRef<'_>,
    ) -> Result<NullableHandle, Error> {
        match handle {
            ZirconHandle::Channel(ChannelHandle { stream_ref, rights }) => {
                self.recv_proxied_handle(conn, stream_ref, move || Ok(Channel::create()), rights)
                    .await
            }
            ZirconHandle::Socket(SocketHandle { stream_ref, socket_type, rights }) => {
                self.recv_proxied_handle(
                    conn,
                    stream_ref,
                    move || {
                        Ok(match socket_type {
                            SocketType::Stream => Socket::create_stream(),
                            SocketType::Datagram => Socket::create_datagram(),
                        })
                    },
                    rights,
                )
                .await
            }
            ZirconHandle::EventPair(EventPairHandle { stream_ref, rights }) => {
                self.recv_proxied_handle(conn, stream_ref, move || Ok(EventPair::create()), rights)
                    .await
            }
        }
    }

    async fn recv_proxied_handle<Hdl, CreateType>(
        self: &Arc<Self>,
        conn: PeerConnRef<'_>,
        stream_ref: StreamRef,
        create_handles: impl FnOnce() -> Result<(CreateType, CreateType), Error> + 'static,
        rights: CreateType::Rights,
    ) -> Result<NullableHandle, Error>
    where
        Hdl: 'static + for<'a> crate::proxy::ProxyableRW<'a>,
        CreateType: 'static
            + Into<fidl::NullableHandle>
            + IntoProxied<Proxied = Hdl>
            + std::fmt::Debug
            + crate::handle_info::WithRights,
    {
        let (tx, rx) = futures::channel::oneshot::channel();
        let rx = ProxyTransferInitiationReceiver::new(
            rx.map_err(move |_| format_err!("cancelled transfer via recv_proxied")),
        );
        let (h, p) = crate::proxy::spawn_recv(
            create_handles,
            rights,
            rx,
            stream_ref,
            conn,
            Arc::downgrade(&self),
        )
        .await?;
        if let Some(p) = p {
            let info = handle_info(h.as_handle_ref())?;
            self.add_proxied(
                &mut *self.proxied_streams.lock().await,
                info.pair_handle_key,
                info.this_handle_key,
                tx,
                p,
            );
        }
        Ok(h)
    }

    // Note the endpoint of a transfer that we know about (may complete a transfer operation)
    pub(crate) async fn post_transfer(
        &self,
        transfer_key: TransferKey,
        other_end: FoundTransfer,
    ) -> Result<(), Error> {
        let mut pending_transfers = self.pending_transfers.lock().await;
        match pending_transfers.insert(transfer_key, PendingTransfer::Complete(other_end)) {
            Some(PendingTransfer::Complete(_)) => bail!("Duplicate transfer received"),
            Some(PendingTransfer::Waiting(w)) => w.wake(),
            None => (),
        }
        Ok(())
    }

    fn poll_find_transfer(
        &self,
        ctx: &mut Context<'_>,
        transfer_key: TransferKey,
        lock: &mut MutexTicket<'_, PendingTransferMap>,
    ) -> Poll<Result<FoundTransfer, Error>> {
        let mut pending_transfers = ready!(lock.poll(ctx));
        if let Some(PendingTransfer::Complete(other_end)) = pending_transfers.remove(&transfer_key)
        {
            Poll::Ready(Ok(other_end))
        } else {
            pending_transfers.insert(transfer_key, PendingTransfer::Waiting(ctx.waker().clone()));
            Poll::Pending
        }
    }

    // Lookup a transfer that we're expected to eventually know about
    pub(crate) async fn find_transfer(
        &self,
        transfer_key: TransferKey,
    ) -> Result<FoundTransfer, Error> {
        let mut lock = MutexTicket::new(&self.pending_transfers);
        poll_fn(|ctx| self.poll_find_transfer(ctx, transfer_key, &mut lock)).await
    }

    // Begin a transfer operation (opposite of find_transfer), publishing an endpoint on the remote
    // nodes transfer table.
    pub(crate) async fn open_transfer(
        self: &Arc<Router>,
        target: NodeId,
        transfer_key: TransferKey,
        handle: NullableHandle,
    ) -> Result<OpenedTransfer, Error> {
        if target == self.node_id {
            // The target is local: we just file away the handle.
            // Later, find_transfer will find this and we'll collapse away Overnet's involvement and
            // reunite the channel ends.
            let info = handle_info(handle.as_handle_ref())?;
            let mut proxied_streams = self.proxied_streams.lock().await;
            log::trace!(
                node_id = self.node_id.0,
                key:? = transfer_key,
                info:? = info,
                all:? = sorted(proxied_streams.keys().map(|x| *x).collect::<Vec<_>>());
                "OPEN_TRANSFER_REMOVE_PROXIED",
            );
            if let Some(removed) = proxied_streams.remove(&info.this_handle_key) {
                assert_eq!(removed.original_paired, info.pair_handle_key);
                assert!(removed.remove_sender.send(RemoveFromProxyTable::Dropped).is_ok());
                removed.proxy_task.detach();
            }
            if let Some(removed) = proxied_streams.remove(&info.pair_handle_key) {
                assert_eq!(removed.original_paired, info.this_handle_key);
                assert!(removed.remove_sender.send(RemoveFromProxyTable::Dropped).is_ok());
                removed.proxy_task.detach();
            }
            self.post_transfer(transfer_key, FoundTransfer::Fused(handle)).await?;
            Ok(OpenedTransfer::Fused)
        } else {
            if let Some((writer, reader)) =
                self.client_peer(target).await?.send_open_transfer(transfer_key).await
            {
                Ok(OpenedTransfer::Remote(writer, reader, handle))
            } else {
                bail!("{:?} failed sending open transfer to {:?}", self.node_id, target)
            }
        }
    }

    pub(crate) async fn client_closed(&self, peer_node_id: NodeId) {
        self.peers.lock().await.circuit_clients.remove(&peer_node_id);
    }
}

/// Runs our `ConnectionNode` to set up circuit-based peers.
async fn run_circuits(
    router: Weak<Router>,
    connections: impl futures::Stream<Item = circuit::Connection> + Send,
    peers: impl futures::Stream<Item = String> + Send,
) -> Result<(), Error> {
    // Notes whenever a new peer announces itself on the network.
    let new_peer_fut = {
        let router = router.clone();
        async move {
            let mut peers = pin!(peers);
            while let Some(peer_node_id) = peers.next().await {
                let router = match router.upgrade() {
                    Some(x) => x,
                    None => {
                        log::warn!("Router disappeared from under circuit runner");
                        break;
                    }
                };

                let res = async {
                    let peer_node_id_num = NodeId::from_circuit_string(&peer_node_id)
                        .map_err(|_| format_err!("Invalid node id: {:?}", peer_node_id))?;
                    let mut peers = router.peers.lock().await;

                    if peers.circuit_clients.get(&peer_node_id_num).and_then(|x| x.peer()).is_some()
                    {
                        log::warn!(peer:? = peer_node_id; "Re-establishing connection");
                    }

                    let (reader, writer_remote) = circuit::stream::stream();
                    let (reader_remote, writer) = circuit::stream::stream();
                    let conn = router
                        .circuit_node
                        .connect_to_peer(&peer_node_id, reader_remote, writer_remote)
                        .await?;
                    let peer = Peer::new_circuit_client(
                        conn,
                        writer,
                        reader,
                        router.service_map.new_local_service_observer().await,
                        &router,
                        peer_node_id_num,
                    )?;

                    if let Some(CircuitState::Waiters(waiters)) =
                        peers.circuit_clients.insert(peer_node_id_num, CircuitState::Peer(peer))
                    {
                        for waiter in waiters {
                            let _ = waiter.send(());
                        }
                    }

                    Result::<_, Error>::Ok(())
                }
                .await;

                if let Err(e) = res {
                    log::warn!(peer:? = peer_node_id; "Attempt to connect to peer failed: {:?}", e);
                }
            }
        }
    };

    // Loops over incoming connections and starts servers.
    let new_conn_fut = async move {
        let mut connections = pin!(connections);
        while let Some(conn) = connections.next().await {
            let peer_name = conn.from().to_owned();
            let res = async {
                let router = router.upgrade().ok_or_else(|| format_err!("router gone"))?;
                Peer::new_circuit_server(conn, &router).await?;
                Result::<_, Error>::Ok(())
            }
            .await;

            if let Err(e) = res {
                log::warn!(
                    peer:? = peer_name;
                    "Attempt to receive connection from peer failed: {:?}",
                    e
                );
            }
        }
    };

    futures::future::join(new_peer_fut, new_conn_fut).await;
    Ok(())
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::test_util::*;
    use circuit::Quality;
    use circuit::multi_stream::multi_stream_node_connection;
    use circuit::stream::stream;

    #[fuchsia::test]
    async fn no_op(run: usize) {
        let mut node_id_gen = NodeIdGenerator::new("router::no_op", run);
        node_id_gen.new_router().unwrap();
        let id = node_id_gen.next().unwrap();
        assert_eq!(Router::with_node_id(id, None).unwrap().node_id, id);
    }

    async fn register_test_service(
        serving_router: Arc<Router>,
        client_router: Arc<Router>,
        service: &'static str,
    ) -> futures::channel::oneshot::Receiver<Channel> {
        use fuchsia_sync::Mutex;
        let (send, recv) = futures::channel::oneshot::channel();
        serving_router
            .service_map()
            .register_service(service.to_string(), {
                let sender = Mutex::new(Some(send));
                move |chan| {
                    println!("{} got request", service);
                    sender.lock().take().unwrap().send(chan).unwrap();
                    println!("{} forwarded channel", service);
                    Ok(())
                }
            })
            .await;
        let serving_node_id = serving_router.node_id();
        println!("{} wait for service to appear @ client", service);
        let lpc = client_router.new_list_peers_context().await;
        loop {
            let peers = lpc.list_peers().await.unwrap();
            println!("{} got peers {:?}", service, peers);
            if peers
                .iter()
                .find(move |peer| {
                    serving_node_id == peer.node_id
                        && peer
                            .services
                            .iter()
                            .find(move |&advertised_service| advertised_service == service)
                            .is_some()
                })
                .is_some()
            {
                break;
            }
        }
        recv
    }

    async fn run_two_node<
        F: 'static + Clone + Sync + Send + Fn(Arc<Router>, Arc<Router>) -> Fut,
        Fut: 'static + Send + Future<Output = Result<(), Error>>,
    >(
        name: &'static str,
        run: usize,
        f: F,
    ) -> Result<(), Error> {
        let mut node_id_gen = NodeIdGenerator::new(name, run);
        let router1 = node_id_gen.new_router()?;
        let router2 = node_id_gen.new_router()?;
        let (circuit1_reader, circuit1_writer) = stream();
        let (circuit2_reader, circuit2_writer) = stream();
        let (out_1, _) = futures::channel::mpsc::unbounded();
        let (out_2, _) = futures::channel::mpsc::unbounded();

        let conn_1 = multi_stream_node_connection(
            router1.circuit_node(),
            circuit1_reader,
            circuit2_writer,
            true,
            Quality::IN_PROCESS,
            out_1,
            "router1".to_owned(),
        );
        let conn_2 = multi_stream_node_connection(
            router2.circuit_node(),
            circuit2_reader,
            circuit1_writer,
            false,
            Quality::IN_PROCESS,
            out_2,
            "router2".to_owned(),
        );
        let _fwd = Task::spawn(async move {
            if let Err(e) = futures::future::try_join(conn_1, conn_2).await {
                log::trace!("forwarding failed: {:?}", e)
            }
        });
        f(router1, router2).await
    }

    #[fuchsia::test]
    async fn no_op_env(run: usize) -> Result<(), Error> {
        run_two_node("router::no_op_env", run, |_router1, _router2| async { Ok(()) }).await
    }

    #[fuchsia::test]
    async fn create_stream(run: usize) -> Result<(), Error> {
        run_two_node("create_stream", run, |router1, router2| async move {
            let (_, p) = fidl::Channel::create();
            println!("create_stream: register service");
            let s = register_test_service(router2.clone(), router1.clone(), "create_stream").await;
            println!("create_stream: connect to service");
            router1.connect_to_service(router2.node_id, "create_stream", p).await?;
            println!("create_stream: wait for connection");
            let _ = s.await?;
            Ok(())
        })
        .await
    }

    #[fuchsia::test]
    async fn send_datagram_immediately(run: usize) -> Result<(), Error> {
        run_two_node("send_datagram_immediately", run, |router1, router2| async move {
            let (c, p) = fidl::Channel::create();
            println!("send_datagram_immediately: register service");
            let s = register_test_service(
                router2.clone(),
                router1.clone(),
                "send_datagram_immediately",
            )
            .await;
            println!("send_datagram_immediately: connect to service");
            router1.connect_to_service(router2.node_id, "send_datagram_immediately", p).await?;
            println!("send_datagram_immediately: wait for connection");
            let s = s.await?;
            let c = fidl::AsyncChannel::from_channel(c);
            let s = fidl::AsyncChannel::from_channel(s);
            c.write(&[1, 2, 3, 4, 5], &mut Vec::new())?;
            let mut buf = fidl::MessageBufEtc::new();
            println!("send_datagram_immediately: wait for datagram");
            s.recv_etc_msg(&mut buf).await?;
            assert_eq!(buf.n_handle_infos(), 0);
            assert_eq!(buf.bytes(), &[1, 2, 3, 4, 5]);
            Ok(())
        })
        .await
    }

    #[fuchsia::test]
    async fn ping_pong(run: usize) -> Result<(), Error> {
        run_two_node("ping_pong", run, |router1, router2| async move {
            let (c, p) = fidl::Channel::create();
            println!("ping_pong: register service");
            let s = register_test_service(router2.clone(), router1.clone(), "ping_pong").await;
            println!("ping_pong: connect to service");
            router1.connect_to_service(router2.node_id, "ping_pong", p).await?;
            println!("ping_pong: wait for connection");
            let s = s.await?;
            let c = fidl::AsyncChannel::from_channel(c);
            let s = fidl::AsyncChannel::from_channel(s);
            println!("ping_pong: send ping");
            c.write(&[1, 2, 3, 4, 5], &mut Vec::new())?;
            println!("ping_pong: receive ping");
            let mut buf = fidl::MessageBufEtc::new();
            s.recv_etc_msg(&mut buf).await?;
            assert_eq!(buf.n_handle_infos(), 0);
            assert_eq!(buf.bytes(), &[1, 2, 3, 4, 5]);
            println!("ping_pong: send pong");
            s.write(&[9, 8, 7, 6, 5, 4, 3, 2, 1], &mut Vec::new())?;
            println!("ping_pong: receive pong");
            let mut buf = fidl::MessageBufEtc::new();
            c.recv_etc_msg(&mut buf).await?;
            assert_eq!(buf.n_handle_infos(), 0);
            assert_eq!(buf.bytes(), &[9, 8, 7, 6, 5, 4, 3, 2, 1]);
            Ok(())
        })
        .await
    }

    fn ensure_pending(f: &mut (impl Send + Unpin + Future<Output = ()>)) {
        let mut ctx = Context::from_waker(&std::task::Waker::noop());
        // Poll a bunch of times to convince ourselves the future is pending forever...
        for _ in 0..1000 {
            assert!(f.poll_unpin(&mut ctx).is_pending());
        }
    }

    #[fuchsia::test]
    async fn concurrent_list_peer_calls_will_error(run: usize) -> Result<(), Error> {
        let mut node_id_gen = NodeIdGenerator::new("concurrent_list_peer_calls_will_error", run);
        let n = node_id_gen.new_router().unwrap();
        let lp = n.new_list_peers_context().await;
        lp.list_peers().await.unwrap();
        let mut never_completes = async {
            lp.list_peers().await.unwrap();
        }
        .boxed();
        ensure_pending(&mut never_completes);
        lp.list_peers().await.expect_err("Concurrent list peers should fail");
        ensure_pending(&mut never_completes);
        Ok(())
    }
}
