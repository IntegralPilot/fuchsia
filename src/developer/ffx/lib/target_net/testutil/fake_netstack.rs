// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::{HashMap, VecDeque, hash_map};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::num::NonZeroU16;
use std::ops::RangeInclusive;
use std::sync::{Arc, Mutex};

use fdomain_client::{AsHandleRef as _, HandleBased as _, Peered as _};
use ffx_target_net::SocketProvider;
use fidl_fuchsia_net_ext::SocketAddress as SocketAddressExt;
use futures::channel::mpsc;
use futures::future::{AbortHandle, AbortRegistration};
use futures::{AsyncReadExt as _, FutureExt as _, StreamExt as _, TryStreamExt as _};
use log::warn;
use rand::prelude::*;

use fdomain_fuchsia_posix as fposix;
use fdomain_fuchsia_posix_socket as fsock;
use fuchsia_async as fasync;

/// A fake netstack for use in tests.
///
/// This implementation is meant to cover the basic socket APIs needed for tests
/// that need a fake backing [`SocketProvider`] and other types in
/// [`ffx_target_net`]. It may *NOT* be compatible with the behavior of
/// zxio/fdio clients.
///
/// On drop, `FakeNetstack` cancels all running tasks serving its resources. Use
/// [`FakeNetstack::detach`] to allow any resources to continue running.
pub struct FakeNetstack {
    scope: fasync::Scope,
    connections: mpsc::UnboundedSender<Protocol>,
    client: Arc<fdomain_client::Client>,
}

impl FakeNetstack {
    /// Creates a new fake netstack.
    pub fn new(client: Arc<fdomain_client::Client>) -> Self {
        let scope = fasync::Scope::new();
        let inner = Inner(Arc::new(State { scope: scope.to_handle(), tcp: Default::default() }));
        let (connections, receiver) = mpsc::unbounded();
        inner.spawn(inner.clone().serve(receiver));
        Self { scope, connections, client }
    }

    /// Connects the provided server end to the fake netstack.
    pub fn connect_socket_provider(
        &self,
        server_end: fdomain_client::fidl::ServerEnd<fsock::ProviderMarker>,
    ) {
        self.connections
            .unbounded_send(Protocol::SocketProvider(server_end))
            .expect("fake netstack not running");
    }

    /// Returns a [`SocketProvider`] connected to this fake netstack.
    pub fn new_socket_provider(&self) -> SocketProvider {
        let (client, server) = self.client.create_endpoints();
        self.connect_socket_provider(server);
        SocketProvider::new(client.into_proxy())
    }

    /// Detaches the netstack, allowing all resources to continue running.
    pub fn detach(self) {
        let Self { scope, connections: _, client: _ } = self;
        scope.detach();
    }
}

enum Protocol {
    SocketProvider(fdomain_client::fidl::ServerEnd<fsock::ProviderMarker>),
}

#[derive(Clone)]
struct Inner(Arc<State>);

struct State {
    scope: fasync::ScopeHandle,
    tcp: TcpState,
}

const EPHEMERAL_RANGE: RangeInclusive<u16> = RangeInclusive::new(49152, 65535);

#[derive(Default)]
struct TcpState {
    demux: Mutex<HashMap<(fsock::Domain, NonZeroU16), TcpSocketState>>,
}

impl TcpState {
    fn bind_local_port(
        &self,
        domain: fsock::Domain,
        port: NonZeroU16,
    ) -> Result<(), fposix::Errno> {
        let mut demux = self.demux.lock().unwrap();
        match demux.entry((domain, port)) {
            hash_map::Entry::Occupied(_) => Err(fposix::Errno::Eaddrinuse),
            hash_map::Entry::Vacant(vacant_entry) => {
                let _: &mut _ = vacant_entry.insert(TcpSocketState::Bound);
                Ok(())
            }
        }
    }

    fn alloc_local_port(&self, domain: fsock::Domain) -> Result<NonZeroU16, fposix::Errno> {
        let mut rng: SmallRng = rand::make_rng();
        for _ in 0..10_000 {
            let port = NonZeroU16::new(rng.random_range(EPHEMERAL_RANGE)).unwrap();
            match self.bind_local_port(domain, port) {
                Ok(()) => return Ok(port),
                Err(_) => {}
            }
        }
        Err(fposix::Errno::Eaddrnotavail)
    }

    fn autobind(
        &self,
        domain: fsock::Domain,
        port: Option<NonZeroU16>,
    ) -> Result<NonZeroU16, fposix::Errno> {
        match port {
            Some(port) => self.bind_local_port(domain, port).map(|()| port),
            None => self.alloc_local_port(domain),
        }
    }

    async fn connect(
        &self,
        client: &Arc<fdomain_client::Client>,
        domain: fsock::Domain,
        local_port: NonZeroU16,
        remote_port: NonZeroU16,
    ) -> Result<(fdomain_client::Socket, AbortRegistration), fposix::Errno> {
        let (fut, client, abort_registration) = {
            let mut demux = self.demux.lock().unwrap();
            let sock = demux.get_mut(&(domain, remote_port)).ok_or(fposix::Errno::Econnrefused)?;
            match sock {
                TcpSocketState::Bound => return Err(fposix::Errno::Econnrefused),
                TcpSocketState::Listening { socket, queue } => {
                    let (client, server) = client.create_stream_socket();
                    let (abort_handle, abort_registration) = AbortOnDrop::new_pair();
                    let incoming = fidl::Signals::from_bits(fsock::SIGNAL_STREAM_INCOMING).unwrap();
                    let fut = socket.signal_peer(fidl::Signals::empty(), incoming);
                    queue.push_back((server, local_port, abort_handle));
                    (fut, client, abort_registration)
                }
            }
        };
        fut.await.unwrap_or_else(|e| {
            let fdomain_client::Error::FDomain(fdomain_client::FDomainError::TargetError(e)) = e
            else {
                panic!("unexpected error: {e:?}");
            };
            assert_eq!(
                fidl::Status::ok(e),
                Err(fidl::Status::PEER_CLOSED),
                "failed to signal peer"
            );
        });
        Ok((client, abort_registration))
    }

    fn accept(
        &self,
        domain: fsock::Domain,
        port: NonZeroU16,
    ) -> Result<(fdomain_client::Socket, NonZeroU16, AbortOnDrop), fposix::Errno> {
        let mut demux = self.demux.lock().unwrap();
        let sock = demux.get_mut(&(domain, port)).ok_or(fposix::Errno::Einval)?;
        match sock {
            TcpSocketState::Bound => Err(fposix::Errno::Einval),
            TcpSocketState::Listening { queue, .. } => {
                queue.pop_front().ok_or(fposix::Errno::Eagain)
            }
        }
    }

    fn listen(
        &self,
        domain: fsock::Domain,
        port: NonZeroU16,
        get_socket: impl FnOnce() -> fdomain_client::Socket,
    ) -> Result<(), fposix::Errno> {
        let mut demux = self.demux.lock().unwrap();
        let sock = demux.get_mut(&(domain, port)).ok_or(fposix::Errno::Einval)?;
        match sock {
            TcpSocketState::Bound => {
                *sock =
                    TcpSocketState::Listening { socket: get_socket(), queue: Default::default() };
                Ok(())
            }
            TcpSocketState::Listening { .. } => Err(fposix::Errno::Ealready),
        }
    }

    fn close(&self, domain: fsock::Domain, port: NonZeroU16) {
        let mut demux = self.demux.lock().unwrap();
        let _: TcpSocketState =
            demux.remove(&(domain, port)).expect("closed socket not in the demux");
    }
}

enum TcpSocketState {
    Bound,
    Listening {
        socket: fdomain_client::Socket,
        queue: VecDeque<(fdomain_client::Socket, NonZeroU16, AbortOnDrop)>,
    },
}

struct Connection {
    sockname: NonZeroU16,
    peername: NonZeroU16,
    sock_client: fdomain_client::Socket,
    abort: AbortOnDrop,
}

impl Inner {
    fn tcp(&self) -> &TcpState {
        &self.0.tcp
    }

    fn spawn(&self, future: impl Future<Output = ()> + Send + 'static) {
        let _: fasync::JoinHandle<()> = self.0.scope.spawn(future);
    }

    fn spawn_fidl(&self, future: impl Future<Output = Result<(), fidl::Error>> + Send + 'static) {
        let _: fasync::JoinHandle<()> = self.0.scope.spawn(future.map(|r| {
            r.unwrap_or_else(|e| {
                if !e.is_closed() {
                    panic!("fidl error {e:?}")
                }
            })
        }));
    }

    async fn serve(self, mut receiver: mpsc::UnboundedReceiver<Protocol>) {
        while let Some(protocol) = receiver.next().await {
            match protocol {
                Protocol::SocketProvider(server_end) => {
                    self.spawn_fidl(self.clone().serve_socket_provider(server_end));
                }
            }
        }
    }

    async fn serve_socket_provider(
        self,
        server_end: fdomain_client::fidl::ServerEnd<fsock::ProviderMarker>,
    ) -> Result<(), fidl::Error> {
        let client = server_end.domain();
        let mut stream = server_end.into_stream();
        while let Some(req) = stream.try_next().await? {
            match req {
                fsock::ProviderRequest::StreamSocket { domain, proto, responder } => {
                    let fsock::StreamSocketProtocol::Tcp = proto;
                    let (client, server) = client.create_endpoints::<fsock::StreamSocketMarker>();

                    self.spawn_tcp_socket(domain, server, None);
                    responder.send(Ok(client))?;
                }
                req => unimplemented!("fake netstack doesn't support {req:?}"),
            }
        }
        Ok(())
    }

    fn spawn_tcp_socket(
        &self,
        domain: fsock::Domain,
        server_end: fdomain_client::fidl::ServerEnd<fsock::StreamSocketMarker>,
        conn: Option<Connection>,
    ) {
        self.spawn_fidl(self.clone().serve_tcp_socket(domain, server_end, conn));
    }

    async fn serve_tcp_socket(
        self,
        domain: fsock::Domain,
        server_end: fdomain_client::fidl::ServerEnd<fsock::StreamSocketMarker>,
        conn: Option<Connection>,
    ) -> Result<(), fidl::Error> {
        let client = server_end.domain();
        let mut stream = server_end.into_stream();

        struct SocketState {
            sockname: Option<NonZeroU16>,
            peername: Option<NonZeroU16>,
            remove_from_demux: bool,
            abort_handle: Option<AbortOnDrop>,
        }

        let (sock_client, mut sock_server, state) = match conn {
            Some(Connection { sockname, peername, sock_client, abort }) => {
                let state = SocketState {
                    sockname: Some(sockname),
                    peername: Some(peername),
                    remove_from_demux: false,
                    abort_handle: Some(abort),
                };
                (sock_client, None, state)
            }
            None => {
                let (sock_client, sock_server) = client.create_stream_socket();
                let state = SocketState {
                    sockname: None,
                    peername: None,
                    remove_from_demux: true,
                    abort_handle: None,
                };
                (sock_client, Some(sock_server), state)
            }
        };

        let mut state = scopeguard::guard(state, |state| {
            let SocketState { sockname, peername: _, remove_from_demux, abort_handle: _ } = state;
            // Accepted sockets collide with the listener socket's local port,
            // so we don't need to clean them from the demux.
            if remove_from_demux {
                if let Some(sockname) = sockname {
                    self.tcp().close(domain, sockname);
                }
            }
        });

        while let Some(req) = stream.try_next().await? {
            match req {
                fsock::StreamSocketRequest::Describe { responder } => {
                    responder.send(fsock::StreamSocketDescribeResponse {
                        socket: Some(
                            sock_client
                                .duplicate_handle(fidl::Rights::SAME_RIGHTS)
                                .await
                                .expect("failed to duplicate socket"),
                        ),
                        __source_breaking: fidl::marker::SourceBreaking,
                    })?;
                }
                fsock::StreamSocketRequest::Connect { addr, responder } => {
                    let rsp = (async {
                        let SocketAddressExt(addr) = addr.into();
                        validate_domain(addr, domain)?;
                        let remote_port =
                            NonZeroU16::new(addr.port()).ok_or(fposix::Errno::Einval)?;
                        if state.peername.is_some() {
                            return Err(fposix::Errno::Ealready);
                        }
                        let local_port = match state.sockname {
                            Some(s) => s,
                            None => {
                                let p = self.tcp().autobind(domain, None)?;
                                state.sockname = Some(p);
                                p
                            }
                        };
                        if !addr.ip().is_loopback() {
                            return Err(fposix::Errno::Econnrefused);
                        }
                        let (sock_peer, abort_registration) =
                            self.tcp().connect(&client, domain, local_port, remote_port).await?;
                        state.abort_handle = Some(abort_registration.handle().into());
                        self.spawn(self.clone().run_connection(
                            sock_peer,
                            sock_server.take().expect("already connected"),
                            abort_registration,
                        ));
                        state.peername = Some(remote_port);

                        Ok(())
                    })
                    .await;
                    responder.send(rsp)?;
                }
                fsock::StreamSocketRequest::Accept { want_addr, responder } => {
                    let rsp = (|| {
                        let local = state.sockname.ok_or(fposix::Errno::Einval)?;
                        let (sock, remote, abort) = self.tcp().accept(domain, local)?;
                        let remote_addr = want_addr.then(|| {
                            SocketAddressExt(SocketAddr::new(
                                domain_to_loopback_addr(domain),
                                remote.get(),
                            ))
                            .into()
                        });
                        Ok((
                            remote_addr,
                            Connection {
                                sockname: local,
                                peername: remote,
                                sock_client: sock,
                                abort,
                            },
                        ))
                    })();
                    match rsp {
                        Ok((remote, connection)) => {
                            let (client, server_end) = client.create_endpoints();
                            self.spawn_tcp_socket(domain, server_end, Some(connection));
                            responder.send(Ok((remote.as_ref(), client)))?
                        }
                        Err(e) => responder.send(Err(e))?,
                    }
                }
                fsock::StreamSocketRequest::Listen { backlog: _, responder } => {
                    let rsp = (|| {
                        let local_port = match state.sockname {
                            Some(s) => s,
                            None => {
                                let p = self.tcp().autobind(domain, None)?;
                                state.sockname = Some(p);
                                p
                            }
                        };
                        self.tcp().listen(domain, local_port, || {
                            sock_server.take().expect("unexpected listen when already listening")
                        })
                    })();
                    responder.send(rsp)?
                }
                fsock::StreamSocketRequest::Bind { addr, responder } => {
                    let rsp = (|| {
                        let SocketAddressExt(addr) = addr.into();
                        validate_domain(addr, domain)?;
                        if !(addr.ip().is_loopback() || addr.ip().is_unspecified()) {
                            return Err(fposix::Errno::Eaddrnotavail);
                        }
                        if state.sockname.is_some() {
                            return Err(fposix::Errno::Ealready);
                        }
                        let port = self.tcp().autobind(domain, NonZeroU16::new(addr.port()))?;
                        state.sockname = Some(port);
                        Ok(())
                    })();
                    responder.send(rsp)?;
                }
                fsock::StreamSocketRequest::GetPeerName { responder } => {
                    let rsp = state
                        .peername
                        .map(|p| {
                            SocketAddressExt(SocketAddr::new(
                                domain_to_loopback_addr(domain),
                                p.get(),
                            ))
                            .into()
                        })
                        .ok_or(fposix::Errno::Enotconn);
                    responder.send(rsp.as_ref().map_err(|e| *e))?;
                }
                fsock::StreamSocketRequest::GetSockName { responder } => {
                    let rsp = state
                        .sockname
                        .map(|p| SocketAddr::new(domain_to_loopback_addr(domain), p.get()))
                        .unwrap_or_else(|| SocketAddr::new(domain_to_unspecified_addr(domain), 0));
                    let rsp = SocketAddressExt(rsp).into();
                    responder.send(Ok(&rsp))?;
                }
                fsock::StreamSocketRequest::Close { responder } => {
                    responder.send(Ok(()))?;
                    return Ok(());
                }
                req => unimplemented!("fake netstack doesn't support {req:?}"),
            }
        }
        Ok(())
    }

    async fn run_connection(
        self,
        a: fdomain_client::Socket,
        b: fdomain_client::Socket,
        abort: AbortRegistration,
    ) {
        let (ar, mut aw) = a.split();
        let (br, mut bw) = b.split();

        let fut =
            futures::future::join(futures::io::copy(ar, &mut bw), futures::io::copy(br, &mut aw));
        match futures::future::Abortable::new(fut, abort).await {
            Ok((Err(a), Err(b))) => {
                warn!("errors running fake connection {a:?} {b:?}")
            }
            Ok((Err(e), _)) | Ok((_, Err(e))) => warn!("error running fake connection {e:?}"),
            Ok((Ok(_), Ok(_))) | Err(futures::future::Aborted) => {}
        }
    }
}

struct AbortOnDrop(AbortHandle);

impl From<AbortHandle> for AbortOnDrop {
    fn from(value: AbortHandle) -> Self {
        Self(value)
    }
}

impl AbortOnDrop {
    fn new_pair() -> (Self, AbortRegistration) {
        let (handle, registration) = AbortHandle::new_pair();
        (Self(handle), registration)
    }
}

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        let Self(h) = self;
        h.abort()
    }
}

fn domain_to_loopback_addr(domain: fsock::Domain) -> IpAddr {
    match domain {
        fsock::Domain::Ipv4 => Ipv4Addr::LOCALHOST.into(),
        fsock::Domain::Ipv6 => Ipv6Addr::LOCALHOST.into(),
    }
}

fn domain_to_unspecified_addr(domain: fsock::Domain) -> IpAddr {
    match domain {
        fsock::Domain::Ipv4 => Ipv4Addr::UNSPECIFIED.into(),
        fsock::Domain::Ipv6 => Ipv6Addr::UNSPECIFIED.into(),
    }
}

fn validate_domain(addr: SocketAddr, domain: fsock::Domain) -> Result<(), fposix::Errno> {
    match (addr, domain) {
        (SocketAddr::V4(_), fsock::Domain::Ipv4) | (SocketAddr::V6(_), fsock::Domain::Ipv6) => {
            Ok(())
        }
        _ => return Err(fposix::Errno::Eafnosupport),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_matches::assert_matches;
    use ffx_target_net::Error;
    use futures::AsyncWriteExt;
    use test_case::test_case;

    fn domain_to_sockaddr(domain: fsock::Domain, port: u16) -> SocketAddr {
        SocketAddr::new(domain_to_loopback_addr(domain), port)
    }

    #[test_case(fsock::Domain::Ipv4)]
    #[test_case(fsock::Domain::Ipv6)]
    #[fuchsia::test]
    async fn connect_to_listener(domain: fsock::Domain) {
        let client = fdomain_local::local_client_empty();
        let netstack = FakeNetstack::new(client);
        let socket_provider = netstack.new_socket_provider();

        let listen_addr = domain_to_sockaddr(domain, 1234);
        let listener = socket_provider.listen(listen_addr, None).await.expect("listen");
        assert_eq!(listener.local_addr(), listen_addr);

        let mut connected = socket_provider.connect(listen_addr).await.expect("connect");
        assert_eq!(connected.peer_addr(), listen_addr);
        assert_eq!(connected.local_addr().ip(), domain_to_loopback_addr(domain));
        assert!(EPHEMERAL_RANGE.contains(&connected.local_addr().port()));

        let mut accepted = listener.accept().await.expect("accept");
        assert_eq!(accepted.local_addr(), listen_addr);
        assert_eq!(accepted.peer_addr(), connected.local_addr());

        let msg = b"hello world";
        let mut buf = vec![0; msg.len()];
        connected.write_all(msg).await.expect("write");
        accepted.read_exact(&mut buf).await.expect("read");
        assert_eq!(msg, buf.as_slice());
        buf.fill(0);
        accepted.write_all(msg).await.expect("write");
        connected.read_exact(&mut buf).await.expect("read");
        assert_eq!(msg, buf.as_slice());

        drop(accepted);
        buf.clear();
        assert_eq!(connected.read_to_end(&mut buf).await.expect("read to end"), 0);
        assert_eq!(buf, Vec::<u8>::new());
    }

    #[test_case(fsock::Domain::Ipv4)]
    #[test_case(fsock::Domain::Ipv6)]
    #[fuchsia::test]
    async fn connect_without_listener(domain: fsock::Domain) {
        let netstack = FakeNetstack::new(fdomain_local::local_client_empty());
        let socket_provider = netstack.new_socket_provider();
        assert_matches!(
            socket_provider.connect(domain_to_sockaddr(domain, 1234)).await,
            Err(Error::Connect(fposix::Errno::Econnrefused))
        );
    }

    #[test_case(fsock::Domain::Ipv4)]
    #[test_case(fsock::Domain::Ipv6)]
    #[fuchsia::test]
    async fn no_reuse_port(domain: fsock::Domain) {
        let netstack = FakeNetstack::new(fdomain_local::local_client_empty());
        let socket_provider = netstack.new_socket_provider();
        let addr = domain_to_sockaddr(domain, 1234);
        let listener = socket_provider.listen(addr, None).await.expect("listen");
        assert_matches!(
            socket_provider.listen(addr, None).await,
            Err(Error::Bind(fposix::Errno::Eaddrinuse))
        );
        listener.close().await.expect("close");

        // Can listen again.
        let _listener = socket_provider.listen(addr, None).await.expect("listen");
    }

    #[test_case(fsock::Domain::Ipv4)]
    #[test_case(fsock::Domain::Ipv6)]
    #[fuchsia::test]
    async fn drop_listener_with_accept_queue(domain: fsock::Domain) {
        let netstack = FakeNetstack::new(fdomain_local::local_client_empty());
        let socket_provider = netstack.new_socket_provider();
        let addr = domain_to_sockaddr(domain, 1234);
        let listener = socket_provider.listen(addr, None).await.expect("listen");
        let connected = futures::stream::iter(0..3)
            .then(|_| async { socket_provider.connect(addr).await.expect("connect") })
            .collect::<Vec<_>>()
            .await;
        drop(listener);
        for mut c in connected {
            let mut buf = Vec::new();
            assert_eq!(c.read_to_end(&mut buf).await.expect("read to end"), 0);
        }
    }

    #[fuchsia::test]
    async fn stop_on_drop() {
        let client = fdomain_local::local_client_empty();
        let netstack = FakeNetstack::new(client.clone());
        let socket_provider = netstack.new_socket_provider();
        let addr = domain_to_sockaddr(fsock::Domain::Ipv4, 1234);
        let listener = socket_provider.listen(addr, None).await.expect("listen");
        let mut connected = socket_provider.connect(addr).await.expect("connect");
        let mut accepted = listener.accept().await.expect("accept");
        drop(netstack);
        let mut buf = Vec::new();
        assert_eq!(connected.read_to_end(&mut buf).await.expect("read to end"), 0);
        assert_eq!(accepted.read_to_end(&mut buf).await.expect("read to end"), 0);
        assert_matches!(
            socket_provider.connect(addr).await,
            Err(Error::Fidl(fidl::Error::ClientChannelClosed { .. }))
        );
        assert_matches!(
            listener.accept().await,
            Err(Error::Fidl(fidl::Error::ClientChannelClosed { .. }))
        );
    }

    #[fuchsia::test]
    async fn detach() {
        let client = fdomain_local::local_client_empty();
        let netstack = FakeNetstack::new(client.clone());
        let socket_provider = netstack.new_socket_provider();
        netstack.detach();
        let addr = domain_to_sockaddr(fsock::Domain::Ipv4, 1234);
        let listener = socket_provider.listen(addr, None).await.expect("listen");
        let _connected = socket_provider.connect(addr).await.expect("connect");
        let _accepted = listener.accept().await.expect("accept");
    }
}
