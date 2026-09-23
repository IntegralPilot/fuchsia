// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This module contains the bulk of the logic for connecting user applications to a
// vsock driver.
//
// Handling user requests is complicated as there are multiple communication channels
// involved. For example a request to 'connect' will result in sending a message
// to the driver over the single DeviceProxy. If this returns with success then
// eventually a message will come over the single Callbacks stream indicating
// whether the remote accepted or rejected.
//
// Fundamentally then there needs to be mutual exclusion in accessing DeviceProxy,
// and de-multiplexing of incoming messages on the Callbacks stream. There are
// a two high level options for doing this.
//  1. Force a single task event driver model. This would mean that additional
//     asynchronous executions are never spawned, and any use of await! or otherwise
//     blocking with additional futures requires collection futures in future sets
//     or having custom polling logic etc. Whilst this is probably the most resource
//     efficient it restricts the service to be single task forever by its design,
//     is harder to reason about as cannot be written very idiomatically with futures
//     and is even more complicated to avoid blocking other requests whilst waiting
//     on responses from the driver.
//  2. Allow multiple asynchronous executions and use some form of message passing
//     and mutual exclusion checking to handle DeviceProxy access and sharing access
//     to the Callbacks stream. Potentially more resource intensive with unnecessary
//     refcells etc, but allows for the potential to have actual concurrent execution
//     and is much simpler to write the logic.
// The chosen option is (2) and the access to DeviceProxy is handled with an Rc<Refcell<State>>,
// and de-multiplexing of the Callbacks is done by registering an event whilst holding
// the refcell, and having a single asynchronous task that is dedicated to converting
// incoming Callbacks to signaling registered events.

use crate::{addr, port};
use anyhow::{Context as _, format_err};
use fidl::endpoints;
use fidl::endpoints::{ControlHandle, RequestStream};
use fidl_fuchsia_hardware_vsock::{
    CallbacksMarker, CallbacksRequest, CallbacksRequestStream, DeviceProxy, VMADDR_CID_HOST,
    VMADDR_CID_LOCAL,
};
use fidl_fuchsia_vsock::{
    AcceptorProxy, ConnectionRequest, ConnectionRequestStream, ConnectionTransport,
    ConnectorRequest, ConnectorRequestStream, ListenerControlHandle, ListenerRequest,
    ListenerRequestStream, SIGNAL_STREAM_INCOMING,
};
use fuchsia_async as fasync;
use futures::channel::{mpsc, oneshot};
use futures::{Future, FutureExt, Stream, StreamExt, TryFutureExt, TryStreamExt, future, select};
use std::cell::{Ref, RefCell, RefMut};
use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::ops::Deref;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};
use thiserror::Error;

const ZXIO_SIGNAL_INCOMING: zx::Signals = zx::Signals::from_bits(SIGNAL_STREAM_INCOMING).unwrap();

type Cid = u32;
type Port = u32;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Addr(Cid, Port);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum EventType {
    Shutdown,
    Response,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Event {
    action: EventType,
    addr: addr::Vsock,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
enum Deregister {
    Event(Event),
    Listen(Addr),
    Port(Addr),
}

#[derive(Error, Debug)]
enum Error {
    #[error("Driver returned failure status {}", _0)]
    Driver(#[source] zx::Status),
    #[error("All ephemeral ports are allocated")]
    OutOfPorts,
    #[error("Addr has already been bound")]
    AlreadyBound,
    #[error("Connection refused by remote")]
    ConnectionRefused,
    #[error("Error whilst communication with client")]
    ClientCommunication(#[source] anyhow::Error),
    #[error("Error whilst communication with client")]
    DriverCommunication(#[source] anyhow::Error),
    #[error("Driver reset the connection")]
    ConnectionReset,
    #[error("There are no more connections in the accept queue")]
    NoConnectionsInQueue,
}

impl From<oneshot::Canceled> for Error {
    fn from(_: oneshot::Canceled) -> Error {
        Error::ConnectionReset
    }
}

impl Error {
    pub fn into_status(&self) -> zx::Status {
        match self {
            Error::Driver(status) => *status,
            Error::OutOfPorts => zx::Status::NO_RESOURCES,
            Error::AlreadyBound => zx::Status::ALREADY_BOUND,
            Error::ConnectionRefused => zx::Status::UNAVAILABLE,
            Error::ClientCommunication(err) | Error::DriverCommunication(err) => {
                *err.downcast_ref::<zx::Status>().unwrap_or(&zx::Status::INTERNAL)
            }
            Error::ConnectionReset => zx::Status::PEER_CLOSED,
            Error::NoConnectionsInQueue => zx::Status::SHOULD_WAIT,
        }
    }
    pub fn is_comm_failure(&self) -> bool {
        match self {
            Error::ClientCommunication(_) | Error::DriverCommunication(_) => true,
            _ => false,
        }
    }
}

fn map_driver_result(result: Result<Result<(), i32>, fidl::Error>) -> Result<(), Error> {
    result
        .map_err(|x| Error::DriverCommunication(x.into()))?
        .map_err(|e| Error::Driver(zx::Status::err_from_raw(e)))
}

struct SocketContextState {
    port: Addr,
    accept_queue: VecDeque<addr::Vsock>,
    backlog: Option<u32>,
    control: ListenerControlHandle,
    signaled: bool,
}

#[derive(Clone)]
pub struct SocketContext(Rc<RefCell<SocketContextState>>);

impl SocketContext {
    fn new(port: Addr, control: ListenerControlHandle) -> SocketContext {
        SocketContext(Rc::new(RefCell::new(SocketContextState {
            port,
            accept_queue: VecDeque::new(),
            backlog: None,
            signaled: false,
            control,
        })))
    }

    fn listen(&self, backlog: u32) -> Result<(), Error> {
        let mut ctx = self.0.borrow_mut();
        if ctx.backlog.is_some() {
            return Err(Error::AlreadyBound);
        }
        // TODO: Update listener?
        ctx.backlog = Some(backlog);
        Ok(())
    }

    fn push_addr(&self, addr: addr::Vsock) -> bool {
        let mut ctx = self.0.borrow_mut();
        if Addr(addr.remote_cid, addr.local_port) != ctx.port {
            panic!("request address doesn't match local socket address");
        }
        let Some(ref mut backlog) = ctx.backlog else {
            panic!("pushing address when not yet bound");
        };
        if *backlog == 0 {
            return false;
        }
        *backlog -= 1;
        ctx.accept_queue.push_back(addr);
        if ctx.signaled == false {
            let _ = ctx.control.signal_peer(zx::Signals::empty(), ZXIO_SIGNAL_INCOMING);
            ctx.signaled = true
        }
        return true;
    }

    fn pop_addr(&self) -> Option<addr::Vsock> {
        let mut ctx = self.0.borrow_mut();
        if let Some(addr) = ctx.accept_queue.pop_front() {
            let Some(ref mut backlog) = ctx.backlog else {
                return None;
            };
            *backlog += 1;
            if ctx.accept_queue.len() == 0 {
                let _ = ctx.control.signal_peer(ZXIO_SIGNAL_INCOMING, zx::Signals::empty());
                ctx.signaled = false;
            }
            Some(addr)
        } else {
            None
        }
    }

    fn port(&self) -> Addr {
        self.0.borrow_mut().port
    }
}

enum Listener {
    Bound,
    Channel(mpsc::UnboundedSender<addr::Vsock>),
    Queue(SocketContext),
}

struct State {
    guest_vsock_device: Option<DeviceProxy>,
    loopback_vsock_device: Option<DeviceProxy>,
    local_cid: Cid,
    events: HashMap<Event, oneshot::Sender<()>>,
    used_ports: HashMap<Cid, port::Tracker>,
    listeners: HashMap<Addr, Listener>,
    tasks: fasync::Scope,
}

impl State {
    fn device(&self, cid: u32) -> Option<&DeviceProxy> {
        match (cid, &self.guest_vsock_device, &self.loopback_vsock_device) {
            (VMADDR_CID_LOCAL, _, Some(loopback)) => Some(&loopback),
            (VMADDR_CID_HOST, Some(guest), _) => Some(&guest),
            (VMADDR_CID_HOST, None, Some(loopback)) => Some(&loopback),
            (cid, None, Some(loopback)) if cid == self.local_cid => Some(&loopback),
            _ => None,
        }
    }
}

#[derive(Clone)]
pub struct Vsock(Rc<RefCell<State>>);

impl Vsock {
    /// Creates a new vsock service connected to the given `DeviceProxy`
    ///
    /// The creation is asynchronous due to need to invoke methods on the given `DeviceProxy`. On
    /// success a pair of `Self, impl Future<Result<_, Error>>` is returned. The `impl Future` is
    /// a future that is listening for and processing messages from the `device`. This future needs
    /// to be evaluated for other methods on the returned `Self` to complete successfully. Unless
    /// a fatal error occurs the future will never yield a result and will execute infinitely.
    pub async fn new(
        guest_vsock_device: Option<DeviceProxy>,
        loopback_vsock_device: Option<DeviceProxy>,
    ) -> Result<(Self, impl Future<Output = Result<Vec<Infallible>, anyhow::Error>>), anyhow::Error>
    {
        let mut server_streams = Vec::new();
        let mut start_device = |device: &DeviceProxy| {
            let (callbacks_client, callbacks_server) =
                endpoints::create_endpoints::<CallbacksMarker>();
            server_streams.push(callbacks_server.into_stream());

            device.start(callbacks_client).map(map_driver_result).err_into::<anyhow::Error>()
        };
        let mut local_cid = VMADDR_CID_LOCAL;
        if let Some(ref device) = guest_vsock_device {
            start_device(device).await.context("Failed to start guest device")?;
            local_cid = device.get_cid().await?;
        }
        if let Some(ref device) = loopback_vsock_device {
            start_device(device).await.context("Failed to start loopback device")?;
        }
        let service = State {
            guest_vsock_device,
            loopback_vsock_device,
            local_cid,
            events: HashMap::new(),
            used_ports: HashMap::new(),
            listeners: HashMap::new(),
            tasks: fasync::Scope::new(),
        };

        let service = Vsock(Rc::new(RefCell::new(service)));
        let callback_loops: Vec<_> = server_streams
            .into_iter()
            .map(|stream| service.clone().run_callbacks(stream))
            .collect();

        Ok((service, future::try_join_all(callback_loops)))
    }
    async fn run_callbacks(
        self,
        mut callbacks: CallbacksRequestStream,
    ) -> Result<Infallible, anyhow::Error> {
        while let Some(Ok(cb)) = callbacks.next().await {
            self.borrow_mut().do_callback(cb);
        }
        // The only way to get here is if our callbacks stream ended, since our notifications
        // cannot disconnect as we are holding a reference to them in |service|.
        Err(format_err!("Driver disconnected"))
    }

    // Spawns a new asynchronous task for listening for incoming connections on a port.
    fn start_listener(
        &self,
        acceptor: fidl::endpoints::ClientEnd<fidl_fuchsia_vsock::AcceptorMarker>,
        local_port: u32,
    ) -> Result<(), Error> {
        let acceptor = acceptor.into_proxy();
        let stream = self.listen_port(local_port)?;
        self.borrow_mut().tasks.spawn_local(
            self.clone()
                .run_connection_listener(stream, acceptor)
                .unwrap_or_else(|err| log::warn!("Error {} running connection listener", err)),
        );
        Ok(())
    }

    // Spawns a new asynchronous task for listening for incoming connections on a port.
    fn start_listener2(
        &self,
        listener: fidl::endpoints::ServerEnd<fidl_fuchsia_vsock::ListenerMarker>,
        port: Addr,
    ) -> Result<(), Error> {
        let stream = listener.into_stream();
        self.bind_port(port.clone())?;
        self.borrow_mut().tasks.spawn_local(
            self.clone()
                .run_connection_listener2(stream, port)
                .unwrap_or_else(|err| log::warn!("Error {} running connection listener", err)),
        );
        Ok(())
    }

    // Handles a single incoming client request.
    async fn handle_request(&self, request: ConnectorRequest) -> Result<(), Error> {
        match request {
            ConnectorRequest::Connect { remote_cid, remote_port, con, responder } => responder
                .send(
                    self.make_connection(remote_cid, remote_port, con)
                        .await
                        .map_err(|e| e.into_status().into_raw()),
                ),
            ConnectorRequest::Listen { local_port, acceptor, responder } => responder.send(
                self.start_listener(acceptor, local_port).map_err(|e| e.into_status().into_raw()),
            ),
            ConnectorRequest::Bind { remote_cid, local_port, listener, responder } => responder
                .send(
                    self.start_listener2(listener, Addr(remote_cid, local_port))
                        .map_err(|e| e.into_status().into_raw()),
                ),
            ConnectorRequest::GetCid { responder } => responder.send(self.borrow().local_cid),
        }
        .map_err(|e| Error::ClientCommunication(e.into()))
    }

    /// Evaluates messages on a `ConnectorRequestStream` until completion or error
    ///
    /// Takes ownership of a `RequestStream` that is most likely created from a `ServicesServer`
    /// and processes any incoming requests on it.
    pub async fn run_client_connection(self, request: ConnectorRequestStream) {
        let self_ref = &self;
        let fut = request
            .map_err(|err| Error::ClientCommunication(err.into()))
            // TODO: The parallel limit of 4 is currently invented with no basis and should
            // made something more sensible.
            .try_for_each_concurrent(4, |request| {
                self_ref
                    .handle_request(request)
                    .or_else(|e| future::ready(if e.is_comm_failure() { Err(e) } else { Ok(()) }))
            });
        if let Err(e) = fut.await {
            log::info!("Failed to handle request {}", e);
        }
    }
    fn alloc_ephemeral_port(self, cid: Cid) -> Option<AllocatedPort> {
        let p = self.borrow_mut().used_ports.entry(cid).or_default().allocate();
        p.map(|p| AllocatedPort { port: Addr(cid, p), service: self })
    }
    // Creates a `ListenStream` that will retrieve raw incoming connection requests.
    // These requests come from the device via the run_callbacks future.
    fn listen_port(&self, port: u32) -> Result<ListenStream, Error> {
        if port::is_ephemeral(port) {
            log::info!("Rejecting request to listen on ephemeral port {}", port);
            return Err(Error::ConnectionRefused);
        }
        match self.borrow_mut().listeners.entry(Addr(VMADDR_CID_HOST, port)) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                let (sender, receiver) = mpsc::unbounded();
                let listen =
                    ListenStream { local_port: port, service: self.clone(), stream: receiver };
                entry.insert(Listener::Channel(sender));
                Ok(listen)
            }
            _ => {
                log::info!("Attempt to listen on already bound port {}", port);
                Err(Error::AlreadyBound)
            }
        }
    }

    fn bind_port(&self, port: Addr) -> Result<(), Error> {
        if port::is_ephemeral(port.1) {
            log::info!("Rejecting request to listen on ephemeral port {}", port.1);
            return Err(Error::ConnectionRefused);
        }
        match self.borrow_mut().listeners.entry(port) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(Listener::Bound);
                Ok(())
            }
            _ => {
                log::info!("Attempt to listen on already bound port {:?}", port);
                Err(Error::AlreadyBound)
            }
        }
    }

    // Helper for inserting an event into the events hashmap
    fn register_event(&self, event: Event) -> Result<OneshotEvent, Error> {
        match self.borrow_mut().events.entry(event) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                let (sender, receiver) = oneshot::channel();
                let event = OneshotEvent {
                    event: Some(entry.key().clone()),
                    service: self.clone(),
                    oneshot: receiver,
                };
                entry.insert(sender);
                Ok(event)
            }
            _ => Err(Error::AlreadyBound),
        }
    }

    // These helpers are wrappers around sending a message to the device, and creating events that
    // will be signaled by the run_callbacks future when it receives a message from the device.
    fn send_request(
        &self,
        addr: &addr::Vsock,
        data: zx::Socket,
    ) -> Result<impl Future<Output = Result<(OneshotEvent, OneshotEvent), Error>> + 'static, Error>
    {
        let shutdown_callback =
            self.register_event(Event { action: EventType::Shutdown, addr: addr.clone() })?;
        let response_callback =
            self.register_event(Event { action: EventType::Response, addr: addr.clone() })?;

        let send_request_fut = self.borrow_mut().send_request(&addr, data);

        Ok(async move {
            send_request_fut.await?;
            Ok((shutdown_callback, response_callback))
        })
    }
    fn send_response(
        &self,
        addr: &addr::Vsock,
        data: zx::Socket,
    ) -> Result<impl Future<Output = Result<OneshotEvent, Error>> + 'static, Error> {
        let shutdown_callback =
            self.register_event(Event { action: EventType::Shutdown, addr: addr.clone() })?;

        let send_request_fut = self.borrow_mut().send_response(&addr, data);

        Ok(async move {
            send_request_fut.await?;
            Ok(shutdown_callback)
        })
    }

    // Runs a connected socket until completion. Processes any VMO sends and shutdown events.
    async fn run_connection<ShutdownFut>(
        self,
        addr: addr::Vsock,
        shutdown_event: ShutdownFut,
        mut requests: ConnectionRequestStream,
        _port: Option<AllocatedPort>,
    ) -> Result<(), Error>
    where
        ShutdownFut:
            Future<Output = Result<(), futures::channel::oneshot::Canceled>> + std::marker::Unpin,
    {
        let mut shutdown_event = shutdown_event.fuse();
        select! {
            shutdown_event = shutdown_event => {
                let fut = future::ready(shutdown_event)
                    .err_into()
                    .and_then(|()| self.borrow_mut().send_rst(&addr));
                return fut.await;
            },
            request = requests.next() => {
                match request {
                    Some(Ok(ConnectionRequest::Shutdown{control_handle: _control_handle})) => {
                        let fut =
                            self.borrow_mut().send_shutdown(&addr)
                                // Wait to either receive the RST for the client or to be
                                // shut down for some other reason
                                .and_then(|()| shutdown_event.err_into());
                        return fut.await;
                    },
                    // Generate a RST for a non graceful client disconnect.
                    Some(Err(e)) => {
                        let fut = self.borrow_mut().send_rst(&addr);
                        fut.await?;
                        return Err(Error::ClientCommunication(e.into()));
                    },
                    None => {
                        let fut = self.borrow_mut().send_rst(&addr);
                        return fut.await;
                    },
                }
            }
        }
    }

    fn listen(&self, socket: &SocketContext, backlog: u32) -> Result<(), Error> {
        socket.listen(backlog)?;
        // Replace "bound" listener with a socket accept queue.
        match self.borrow_mut().listeners.entry(socket.port()) {
            std::collections::hash_map::Entry::Vacant(_) => {
                // We should be in bound state. Something went wrong if we end up here.
                log::warn!("Expected listener to be in bound state, but listener not found!");
                return Err(Error::AlreadyBound);
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                if !matches!(entry.get(), Listener::Bound) {
                    // Listen was probably already called. The call to socket.listen should
                    // probably already have failed in this case.
                    log::warn!("Listen called multiple times.");
                    return Err(Error::AlreadyBound);
                }
                entry.insert(Listener::Queue(socket.clone()));
            }
        };

        Ok(())
    }

    async fn accept(
        &self,
        socket: &SocketContext,
        con: ConnectionTransport,
    ) -> Result<addr::Vsock, Error> {
        if let Some(addr) = socket.pop_addr() {
            let data = con.data;
            let con = con.con.into_stream();
            let shutdown_event = self.send_response(&addr, data)?.await?;
            self.borrow_mut().tasks.spawn_local(
                self.clone()
                    .run_connection(addr.clone(), shutdown_event, con, None)
                    .map_err(|err| log::warn!("Error {} whilst running connection", err))
                    .map(|_| ()),
            );
            // TODO: check if we want want to return the local port for the connection or the local
            // port which the request came over.
            Ok(addr)
        } else {
            Err(Error::NoConnectionsInQueue)
        }
    }

    // Handles a single incoming client request.
    async fn handle_listener_request(
        &self,
        socket: &SocketContext,
        request: ListenerRequest,
    ) -> Result<(), Error> {
        match request {
            ListenerRequest::Listen { backlog, responder } => {
                responder.send(self.listen(socket, backlog).map_err(|e| e.into_status().into_raw()))
            }
            ListenerRequest::Accept { con, responder } => match self.accept(socket, con).await {
                Ok(addr) => responder.send(Ok(&addr)),
                Err(e) => responder.send(Err(e.into_status().into_raw())),
            },
        }
        .map_err(|e| Error::ClientCommunication(e.into()))
    }

    async fn run_connection_listener2(
        self,
        request: ListenerRequestStream,
        port: Addr,
    ) -> Result<(), Error> {
        let socket = SocketContext::new(port, request.control_handle());
        let self_ref = &self;
        let fut = request
            .map_err(|err| Error::ClientCommunication(err.into()))
            .try_for_each_concurrent(None, |request| {
                self_ref
                    .handle_listener_request(&socket, request)
                    .or_else(|e| future::ready(if e.is_comm_failure() { Err(e) } else { Ok(()) }))
            });
        if let Err(e) = fut.await {
            log::info!("Failed to handle request {}", e);
        }
        self.deregister(Deregister::Listen(socket.port()));
        Ok(())
    }

    // Waits for incoming connections on the given `ListenStream`, checks with the
    // user via the `acceptor` if it should be accepted, and if so spawns a new
    // asynchronous task to run the connection.
    async fn run_connection_listener(
        self,
        incoming: ListenStream,
        acceptor: AcceptorProxy,
    ) -> Result<(), Error> {
        incoming
            .then(|addr| acceptor.accept(&*addr.clone()).map_ok(|maybe_con| (maybe_con, addr)))
            .map_err(|e| Error::ClientCommunication(e.into()))
            .try_for_each(|(maybe_con, addr)| async {
                match maybe_con {
                    Some(con) => {
                        let data = con.data;
                        let con = con.con.into_stream();
                        let shutdown_event = self.send_response(&addr, data)?.await?;
                        self.borrow_mut().tasks.spawn_local(
                            self.clone()
                                .run_connection(addr, shutdown_event, con, None)
                                .map_err(|err| {
                                    log::warn!("Error {} whilst running connection", err)
                                })
                                .map(|_| ()),
                        );
                        Ok(())
                    }
                    None => {
                        let fut = self.borrow_mut().send_rst(&addr);
                        fut.await
                    }
                }
            })
            .await
    }

    // Attempts to connect to the given remote cid/port. If successful spawns a new
    // asynchronous task to run the connection until completion.
    async fn make_connection(
        &self,
        remote_cid: u32,
        remote_port: u32,
        con: ConnectionTransport,
    ) -> Result<u32, Error> {
        let data = con.data;
        let con = con.con.into_stream();
        let port = self.clone().alloc_ephemeral_port(remote_cid).ok_or(Error::OutOfPorts)?;
        let port_value = port.port.1;
        let addr = addr::Vsock::new(port_value, remote_port, remote_cid);
        let (shutdown_event, response_event) = self.send_request(&addr, data)?.await?;
        let mut shutdown_event = shutdown_event.fuse();
        select! {
            _shutdown_event = shutdown_event => {
                // Getting a RST here just indicates a rejection and
                // not any underlying issues.
                return Err(Error::ConnectionRefused);
            },
            response_event = response_event.fuse() => response_event?,
        }

        self.borrow_mut().tasks.spawn_local(
            self.clone()
                .run_connection(addr, shutdown_event, con, Some(port))
                .unwrap_or_else(|err| log::warn!("Error {} whilst running connection", err)),
        );
        Ok(port_value)
    }

    /// Mutably borrow the wrapped value.
    fn borrow_mut(&self) -> RefMut<'_, State> {
        self.0.borrow_mut()
    }

    fn borrow(&self) -> Ref<'_, State> {
        self.0.borrow()
    }

    // Deregisters the specified event.
    fn deregister(&self, event: Deregister) {
        self.borrow_mut().deregister(event);
    }
}

impl State {
    // Remove the `event` from the `events` `HashMap`
    fn deregister(&mut self, event: Deregister) {
        match event {
            Deregister::Event(e) => {
                self.events.remove(&e);
            }
            Deregister::Listen(a) => {
                self.listeners.remove(&a);
            }
            Deregister::Port(p) => {
                self.used_ports.get_mut(&p.0).unwrap().free(p.1);
            }
        }
    }

    // Wrappers around device functions with nicer type signatures
    fn send_request(
        &mut self,
        addr: &addr::Vsock,
        data: zx::Socket,
    ) -> impl Future<Output = Result<(), Error>> + use<> {
        let result = self
            .device(addr.remote_cid)
            .ok_or(Error::ConnectionRefused)
            .and_then(|device| Ok(device.send_request(&addr.clone(), data).map(map_driver_result)));
        async { result?.await }
    }
    fn send_response(
        &mut self,
        addr: &addr::Vsock,
        data: zx::Socket,
    ) -> impl Future<Output = Result<(), Error>> + use<> {
        let result =
            self.device(addr.remote_cid).ok_or(Error::ConnectionRefused).and_then(|device| {
                Ok(device.send_response(&addr.clone(), data).map(map_driver_result))
            });
        async { result?.await }
    }
    fn send_rst(
        &mut self,
        addr: &addr::Vsock,
    ) -> impl Future<Output = Result<(), Error>> + 'static + use<> {
        let result = self
            .device(addr.remote_cid)
            .ok_or(Error::ConnectionRefused)
            .and_then(|device| Ok(device.send_rst(&addr.clone()).map(map_driver_result)));
        async { result?.await }
    }
    fn send_shutdown(
        &mut self,
        addr: &addr::Vsock,
    ) -> impl Future<Output = Result<(), Error>> + 'static + use<> {
        let result = self
            .device(addr.remote_cid)
            .ok_or(Error::ConnectionRefused)
            .and_then(|device| Ok(device.send_shutdown(&addr.clone()).map(map_driver_result)));
        async { result?.await }
    }

    // Processes a single callback from the `device`. This is intended to be used by
    // `Vsock::run_callbacks`
    fn do_callback(&mut self, callback: CallbacksRequest) {
        match callback {
            CallbacksRequest::Response { addr, control_handle: _control_handle } => {
                self.events
                    .remove(&Event { action: EventType::Response, addr: addr::Vsock::from(addr) })
                    .map(|channel| channel.send(()));
            }
            CallbacksRequest::Rst { addr, control_handle: _control_handle } => {
                self.events
                    .remove(&Event { action: EventType::Shutdown, addr: addr::Vsock::from(addr) });
            }
            CallbacksRequest::Request { addr, control_handle: _control_handle } => {
                let addr = addr::Vsock::from(addr);
                let reset = |state: &mut State| {
                    let task = state.send_rst(&addr).map(|_| ());
                    state.tasks.spawn_local(task);
                };
                match self.listeners.get(&Addr(addr.remote_cid, addr.local_port)) {
                    Some(Listener::Bound) => {
                        log::warn!(
                            "Request on port {} denied due to socket only bound, not yet listening",
                            addr.local_port
                        );
                        reset(self);
                    }
                    Some(Listener::Channel(sender)) => {
                        let _ = sender.unbounded_send(addr.clone());
                    }
                    Some(Listener::Queue(socket)) => {
                        if !socket.push_addr(addr.clone()) {
                            log::warn!(
                                "Request on port {} denied due to full backlog",
                                addr.local_port
                            );
                            reset(self);
                        }
                    }
                    None => {
                        log::warn!("Request on port {} with no listener", addr.local_port);
                        reset(self);
                    }
                }
            }
            CallbacksRequest::Shutdown { addr, control_handle: _control_handle } => {
                self.events
                    .remove(&Event { action: EventType::Shutdown, addr: addr::Vsock::from(addr) })
                    .map(|channel| channel.send(()));
            }
            CallbacksRequest::TransportReset { new_cid: _new_cid, responder } => {
                self.events.clear();
                let _ = responder.send();
            }
        }
    }
}

struct AllocatedPort {
    service: Vsock,
    port: Addr,
}

impl Deref for AllocatedPort {
    type Target = Addr;

    fn deref(&self) -> &Addr {
        &self.port
    }
}

impl Drop for AllocatedPort {
    fn drop(&mut self) {
        self.service.deregister(Deregister::Port(self.port));
    }
}

struct OneshotEvent {
    event: Option<Event>,
    service: Vsock,
    oneshot: oneshot::Receiver<()>,
}

impl Drop for OneshotEvent {
    fn drop(&mut self) {
        self.event.take().map(|e| self.service.deregister(Deregister::Event(e)));
    }
}

impl Future for OneshotEvent {
    type Output = <oneshot::Receiver<()> as Future>::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.oneshot.poll_unpin(cx) {
            Poll::Ready(x) => {
                // Take the event so that we don't try to deregister it later,
                // as by having sent the message we just received the callbacks
                // task will already have removed it
                self.event.take();
                Poll::Ready(x)
            }
            p => p,
        }
    }
}

struct ListenStream {
    local_port: Port,
    service: Vsock,
    stream: mpsc::UnboundedReceiver<addr::Vsock>,
}

impl Drop for ListenStream {
    fn drop(&mut self) {
        self.service.deregister(Deregister::Listen(Addr(VMADDR_CID_HOST, self.local_port)));
    }
}

impl Stream for ListenStream {
    type Item = addr::Vsock;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.stream.poll_next_unpin(cx)
    }
}
