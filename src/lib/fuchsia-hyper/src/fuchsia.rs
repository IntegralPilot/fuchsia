// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::happy_eyeballs::{self, RealSocketConnector};
use crate::{
    HyperConnectorFuture, SocketOptions, TcpOptions, TcpStream, connect_and_bind_device,
    parse_ip_addr,
};
use fidl::endpoints::{ClientEnd, create_endpoints};
use fidl_connector::{Connect, ServiceReconnector};
use fidl_fuchsia_net_name::{LookupIpOptions, LookupMarker, LookupProxy, LookupResult};
use fidl_fuchsia_posix_socket::{Domain, ProviderMarker, StreamSocketProtocol};
use fuchsia_async::net;

use futures::future::{Future, FutureExt};
use futures::io;
use futures::task::{Context, Poll};
use http::uri::{Scheme, Uri};
use hyper_util::rt::TokioIo;
use rustls::RootCertStore;
use std::convert::TryFrom as _;
use std::net::SocketAddr;
use std::num::TryFromIntError;
use std::sync::{Arc, LazyLock};
use tower_service::Service;
use zx::StatusExt;

pub fn new_root_cert_store() -> Arc<RootCertStore> {
    // It can be expensive to parse the certs, so cache them
    static ROOT_STORE: LazyLock<Arc<RootCertStore>> = LazyLock::new(|| {
        let mut root_store = rustls::RootCertStore::empty();

        root_store.extend(webpki_roots_fuchsia::TLS_SERVER_ROOTS.iter().cloned());

        Arc::new(root_store)
    });

    Arc::clone(&ROOT_STORE)
}

/// A Fuchsia-compatible implementation of hyper's `Connect` trait which allows
/// creating a TcpStream to a particular destination.
#[derive(Clone)]
pub struct HyperConnector {
    tcp_options: TcpOptions,
    socket_options: SocketOptions,
    provider: RealServiceConnector,
}

impl From<(TcpOptions, SocketOptions)> for HyperConnector {
    fn from((tcp_options, socket_options): (TcpOptions, SocketOptions)) -> Self {
        Self { tcp_options, socket_options, provider: RealServiceConnector::new() }
    }
}

impl HyperConnector {
    pub fn new() -> Self {
        Self::from_tcp_options(TcpOptions::default())
    }

    pub fn from_tcp_options(tcp_options: TcpOptions) -> Self {
        Self {
            tcp_options,
            socket_options: SocketOptions::default(),
            provider: RealServiceConnector::new(),
        }
    }
}

impl Service<Uri> for HyperConnector {
    type Response = TokioIo<TcpStream>;
    type Error = std::io::Error;
    type Future = HyperConnectorFuture;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // This connector is always ready, but others might not be.
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, dst: Uri) -> Self::Future {
        let self_ = self.clone();
        HyperConnectorFuture { fut: Box::pin(async move { self_.call_async(dst).await }) }
    }
}

impl HyperConnector {
    async fn call_async(&self, dst: Uri) -> Result<TokioIo<TcpStream>, io::Error> {
        let host = dst.host().ok_or_else(|| io::Error::other("destination host is unspecified"))?;
        let port = match dst.port() {
            Some(port) => port.as_u16(),
            None => {
                if dst.scheme() == Some(&Scheme::HTTPS) {
                    443
                } else {
                    80
                }
            }
        };

        let stream = connect_to_addr(
            self.provider.clone(),
            host,
            port,
            self.socket_options.bind_device.as_deref(),
        )
        .await?;
        let () = self.tcp_options.apply(stream.std())?;

        Ok(TokioIo::new(TcpStream { stream }))
    }
}

#[allow(dead_code)] // TODO(https://fxbug.dev/421409340)
#[derive(Clone)]
pub struct Executor;

impl<F: Future + Send + 'static> hyper::rt::Executor<F> for Executor {
    fn execute(&self, fut: F) {
        fuchsia_async::Task::spawn(fut.map(|_| ())).detach()
    }
}

#[allow(dead_code)] // TODO(https://fxbug.dev/421409340)
#[derive(Clone)]
pub struct LocalExecutor;

impl<F: Future + 'static> hyper::rt::Executor<F> for LocalExecutor {
    fn execute(&self, fut: F) {
        fuchsia_async::Task::local(fut.map(drop)).detach()
    }
}

pub(crate) trait ProviderConnector {
    fn connect(&self) -> Result<ClientEnd<ProviderMarker>, io::Error>;
}

trait LookupConnector {
    fn connect(&self) -> Result<LookupProxy, io::Error>;
}

#[derive(Clone)]
struct RealServiceConnector {
    name_lookup_connector: ServiceReconnector<LookupMarker>,
}

impl RealServiceConnector {
    fn new() -> Self {
        RealServiceConnector { name_lookup_connector: ServiceReconnector::<LookupMarker>::new() }
    }
}

impl ProviderConnector for RealServiceConnector {
    fn connect(&self) -> Result<ClientEnd<ProviderMarker>, io::Error> {
        let (client_end, server_end) = create_endpoints::<ProviderMarker>();
        fuchsia_component::client::connect_channel_to_protocol::<ProviderMarker>(server_end.into())
            .map_err(|err| {
                io::Error::other(format!("failed to connect to socket provider service: {}", err))
            })?;
        Ok(client_end)
    }
}

impl LookupConnector for RealServiceConnector {
    fn connect(&self) -> Result<LookupProxy, io::Error> {
        self.name_lookup_connector.connect().map_err(|err| {
            io::Error::other(format!("failed to connect to name lookup service: {}", err))
        })
    }
}

async fn connect_to_addr<T: ProviderConnector + LookupConnector + 'static>(
    provider: T,
    host: &str,
    port: u16,
    bind_device: Option<&str>,
) -> Result<net::TcpStream, io::Error> {
    if let Some(addr) = parse_ip_addr_with_provider(&provider, host, port).await? {
        return connect_and_bind_device(&provider, addr, bind_device)?.await;
    }

    happy_eyeballs::happy_eyeballs(
        resolve_ip_addr(&provider, host, port).await?,
        RealSocketConnector::new(provider),
        happy_eyeballs::RECOMMENDED_MIN_CONN_ATT_DELAY,
        happy_eyeballs::RECOMMENDED_CONN_ATT_DELAY,
        bind_device,
    )
    .await
}

async fn resolve_ip_addr<T: LookupConnector>(
    name_lookup: &T,
    host: &str,
    port: u16,
) -> Result<impl Iterator<Item = SocketAddr> + use<T>, io::Error> {
    let proxy = name_lookup.connect()?;
    let LookupResult { addresses, .. } = proxy
        .lookup_ip(
            host,
            &LookupIpOptions {
                ipv4_lookup: Some(true),
                ipv6_lookup: Some(true),
                sort_addresses: Some(true),
                ..Default::default()
            },
        )
        .await
        .map_err(|err| io::Error::other(format!("failed to call NameProvider.LookupIp: {}", err)))?
        .map_err(|err| {
            // Match stdlib's behavior, which maps all GAI errors but EAI_SYSTEM
            // to io::ErrorKind::Other.
            io::Error::other(format!("NameProvider.LookupIp failure: {:?}", err))
        })?;

    Ok(addresses
        .ok_or_else(|| io::Error::other("addresses not provided in NameProvider response"))?
        .into_iter()
        .map(move |addr| {
            let fidl_fuchsia_net_ext::IpAddress(addr) = addr.into();
            SocketAddr::new(addr, port)
        }))
}

async fn parse_ip_addr_with_provider(
    provider: &impl ProviderConnector,
    host: &str,
    port: u16,
) -> Result<Option<SocketAddr>, io::Error> {
    parse_ip_addr(host, port, |zone_id| async {
        let proxy = provider.connect()?.into_proxy();
        let id = proxy
            .interface_name_to_index(zone_id)
            .await
            .map_err(|err| {
                io::Error::other(format!(
                    "failed to communicate with socket provider while getting interface index: {}",
                    err
                ))
            })?
            .map_err(|status| {
                let status = zx::Status::err_from_raw(status);
                io::Error::new(
                    status.into_io_error_kind(),
                    format!("failed to get interface index from socket provider: {status}"),
                )
            })?;

        // SocketAddrV6 only works with 32 bit scope ids.
        u32::try_from(id).map_err(|TryFromIntError { .. }| {
            io::Error::other("interface index too large to convert to scope_id")
        })
    })
    .await
}

pub(crate) fn stream_socket<T: ProviderConnector>(
    provider: &T,
    domain: Domain,
    proto: StreamSocketProtocol,
) -> io::Result<socket2::Socket> {
    let socket_provider = provider.connect()?.into_sync_proxy();
    let sock = socket_provider
        .stream_socket(domain, proto, zx::MonotonicInstant::INFINITE)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?
        .map_err(|e| std::io::Error::from_raw_os_error(e.into_primitive()))?;

    Ok(fdio::create_fd(sock.into()).map_err(|s| s.into_io_error())?.into())
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::*;
    use assert_matches::assert_matches;
    use fidl::endpoints::{create_proxy_and_stream, create_request_stream};
    use fidl_fuchsia_net_name::{LookupError, LookupRequest};
    use fidl_fuchsia_posix_socket::ProviderRequest;
    use fuchsia_async::net::TcpListener;
    use fuchsia_async::{self as fasync, LocalExecutor};
    use futures::prelude::*;
    use std::cell::RefCell;

    struct PanicConnector;

    impl ProviderConnector for PanicConnector {
        fn connect(&self) -> Result<ClientEnd<ProviderMarker>, io::Error> {
            panic!("should not be trying to talk to the Provider service")
        }
    }

    #[test]
    fn can_create_client() {
        let _exec = LocalExecutor::default();
        let _client = new_client();
    }

    #[test]
    fn can_create_https_client() {
        let _exec = LocalExecutor::default();
        let _client = new_https_client();
    }

    #[fasync::run_singlethreaded(test)]
    async fn hyper_connector_sets_tcp_options() {
        let addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0);
        let listener = TcpListener::bind(&addr).unwrap();
        let addr = listener.local_addr().unwrap();

        let idle = std::time::Duration::from_secs(36);
        let interval = std::time::Duration::from_secs(47);
        let count = 58;
        let rcvbuf = 1 << 20;
        let uri = format!("https://{}", addr).parse::<hyper::Uri>().unwrap();
        let (io, _server) = future::try_join(
            HyperConnector::from_tcp_options(TcpOptions {
                keepalive_idle: Some(idle),
                keepalive_interval: Some(interval),
                keepalive_count: Some(count),
                tcp_receive_buffer_size: Some(rcvbuf),
            })
            .call(uri),
            listener.accept_stream().try_next(),
        )
        .await
        .unwrap();

        let tcp_stream = io.into_inner();
        let stream = socket2::SockRef::from(tcp_stream.stream.std());

        assert_matches!(stream.keepalive(), Ok(v) if v);
        assert_matches!(stream.tcp_keepalive_time(), Ok(v) if v == idle);
        assert_matches!(stream.tcp_keepalive_interval(), Ok(v) if v == interval);
        assert_matches!(stream.tcp_keepalive_retries(), Ok(v) if v == count);
        assert_matches!(stream.recv_buffer_size(), Ok(v) if v == rcvbuf);
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_parse_ipv6_addr_with_provider() {
        let expected = "fe80::1:2:3:4".parse::<Ipv6Addr>().unwrap();

        assert_matches!(
            parse_ip_addr_with_provider(&PanicConnector, "[fe80::1:2:3:4%250]", 8080).await,
            Ok(Some(addr)) if addr == SocketAddr::V6(SocketAddrV6::new(expected, 8080, 0, 0))
        );

        assert_matches!(
            parse_ip_addr_with_provider(&PanicConnector, "[fe80::1:2:3:4%252]", 8080).await,
            Ok(Some(addr)) if addr == SocketAddr::V6(SocketAddrV6::new(expected, 8080, 0, 2))
        );
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_parse_ipv6_addr_with_provider_supports_interface_names() {
        let connector = RealServiceConnector::new();
        let expected = "fe80::1:2:3:4".parse::<Ipv6Addr>().unwrap();

        assert_matches!(
            parse_ip_addr_with_provider(&connector, "[fe80::1:2:3:4%25lo]", 8080).await,
            Ok(Some(addr)) if addr == SocketAddr::V6(SocketAddrV6::new(expected, 8080, 0, 1))
        );

        assert_matches!(
            parse_ip_addr_with_provider(&connector, "[fe80::1:2:3:4%25]", 8080).await,
            Err(err) if err.kind() == io::ErrorKind::NotFound
        );

        assert_matches!(
            parse_ip_addr_with_provider(&connector, "[fe80::1:2:3:4%25unknownif]", 8080).await,
            Err(err) if err.kind() == io::ErrorKind::NotFound
        );
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_parse_ipv6_addr_handles_connection_errors() {
        struct ErrorConnector;

        impl ProviderConnector for ErrorConnector {
            fn connect(&self) -> Result<ClientEnd<ProviderMarker>, io::Error> {
                Err(io::Error::other("something bad happened"))
            }
        }

        assert_matches!(parse_ip_addr_with_provider(&ErrorConnector, "[fe80::1:2:3:4%25lo]", 8080).await,
            Err(err) if err.kind() == io::ErrorKind::Other);
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_parse_ipv6_addr_handles_large_interface_indices() {
        let (client_end, mut stream) = create_request_stream::<ProviderMarker>();

        let provider_fut = async move {
            while let Some(req) = stream.try_next().await.unwrap_or(None) {
                match req {
                    ProviderRequest::InterfaceNameToIndex { name: _, responder } => {
                        responder.send(Ok(u64::MAX)).unwrap()
                    }
                    _ => panic!("unexpected request"),
                }
            }
        };

        struct ErrorConnector {
            client_end: RefCell<Option<ClientEnd<ProviderMarker>>>,
        }

        impl ProviderConnector for ErrorConnector {
            fn connect(&self) -> Result<ClientEnd<ProviderMarker>, io::Error> {
                let client_end = self.client_end.borrow_mut().take().unwrap();
                Ok(client_end)
            }
        }

        let connector = ErrorConnector { client_end: RefCell::new(Some(client_end)) };

        let parse_ip_fut = parse_ip_addr_with_provider(&connector, "[fe80::1:2:3:4%25lo]", 8080);

        // Join the two futures to make sure they both complete.
        let ((), res) = future::join(provider_fut, parse_ip_fut).await;

        assert_matches!(res, Err(err) if err.kind() == io::ErrorKind::Other);
    }

    struct ProxyConnector<T> {
        proxy: T,
    }

    impl LookupConnector for ProxyConnector<LookupProxy> {
        fn connect(&self) -> Result<LookupProxy, io::Error> {
            Ok(self.proxy.clone())
        }
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_resolve_ip_addr() {
        let (sender, receiver) =
            futures::channel::mpsc::unbounded::<Result<LookupResult, LookupError>>();
        let (proxy, stream) = create_proxy_and_stream::<LookupMarker>();
        const TEST_HOSTNAME: &'static str = "foobar.com";
        let name_lookup_fut = stream.zip(receiver).for_each(|(req, rsp)| match req {
            Ok(LookupRequest::LookupIp { hostname, options, responder }) => {
                assert_eq!(hostname.as_str(), TEST_HOSTNAME);
                assert_eq!(
                    options,
                    LookupIpOptions {
                        ipv4_lookup: Some(true),
                        ipv6_lookup: Some(true),
                        sort_addresses: Some(true),
                        ..Default::default()
                    }
                );
                let rsp = rsp.as_ref().map_err(|e| *e);
                futures::future::ready(responder.send(rsp).expect("failed to send FIDL response"))
            }
            req => panic!("unexpected item in request stream {:?}", req),
        });

        let connector = ProxyConnector { proxy };

        let ip_v4 = Ipv4Addr::LOCALHOST.into();
        let ip_v6 = Ipv6Addr::LOCALHOST.into();
        const PORT1: u16 = 1234;
        const PORT2: u16 = 4321;

        let test_fut = async move {
            // Test expectation's error variant is a tuple of the lookup error
            // to inject and the expected io error kind returned.
            type Expectation = Result<Vec<std::net::IpAddr>, (LookupError, io::ErrorKind)>;
            let test_resolve = |port, expect: Expectation| {
                let fidl_response = expect
                    .clone()
                    .map(|addrs| LookupResult {
                        addresses: Some(
                            addrs
                                .into_iter()
                                .map(|std| fidl_fuchsia_net_ext::IpAddress(std).into())
                                .collect(),
                        ),
                        ..Default::default()
                    })
                    .map_err(|(fidl_err, _io_err)| fidl_err);
                let expect = expect
                    .map(|addrs| {
                        addrs.into_iter().map(|addr| SocketAddr::new(addr, port)).collect()
                    })
                    .map_err(|(_fidl_err, io_err)| io_err);
                let () = sender.unbounded_send(fidl_response).expect("failed to send expectation");
                resolve_ip_addr(&connector, TEST_HOSTNAME, port)
                    .map_ok(Iterator::collect::<Vec<_>>)
                    // Map IO error to kind so we can do equality.
                    .map_err(|err| err.kind())
                    .map(move |result| {
                        assert_eq!(result, expect);
                    })
            };
            let () = test_resolve(PORT1, Ok(vec![ip_v4])).await;
            let () = test_resolve(PORT2, Ok(vec![ip_v6])).await;
            let () = test_resolve(PORT1, Ok(vec![ip_v4, ip_v6])).await;
            let () = test_resolve(PORT1, Err((LookupError::NotFound, io::ErrorKind::Other))).await;
        };

        let ((), ()) = futures::future::join(name_lookup_fut, test_fut).await;
    }
}
