// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::HashMap;
use std::pin::Pin;

use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_name as fnet_name;
use fidl_fuchsia_net_ndp as fnet_ndp;
use fidl_fuchsia_net_ndp_ext as fnet_ndp_ext;

use anyhow::Context;
use async_utils::stream::{Tagged, WithTag as _};
use dns_server_watcher::{DnsServers, DnsServersUpdateSource};
use fidl::endpoints::{ControlHandle as _, Responder as _};
use fuchsia_async::DurationExt as _;
use futures::stream::{BoxStream, FuturesUnordered};
use futures::{Future, FutureExt as _, Stream, StreamExt};
use log::{error, info, trace, warn};
use net_types::{Scope, ScopeableAddress};
use packet_formats::icmp::ndp as packet_formats_ndp;

/// RFC-1035§4.2 specifies port 53 (decimal) as the default port for DNS requests.
pub const DNS_PORT: u16 = 53;

use crate::{DnsServerLifetime, DnsServerUpdate, DnsWatcherResultPayload, network};

/// Updates the DNS servers used by the DNS resolver.
pub(super) async fn update_servers(
    lookup_admin: &fnet_name::LookupAdminProxy,
    dns_servers: &mut DnsServers,
    dns_server_watch_responders: &mut DnsServerWatchResponders,
    networks_service: &mut network::NetpolNetworksService,
    source: DnsServersUpdateSource,
    servers: Vec<fnet_name::DnsServer_>,
) {
    trace!("updating DNS servers obtained from {:?} to {:?}", source, servers);

    let servers_before = dns_servers.consolidated();
    dns_servers.set_servers_from_source(source, servers);
    let servers = dns_servers.consolidated();
    if servers_before == servers {
        trace!("Update skipped because dns server list has not changed");
        return;
    }
    trace!("updating LookupAdmin with DNS servers = {:?}", servers);

    match lookup_admin.set_dns_servers(&servers).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => warn!("error setting DNS servers: {:?}", zx::Status::err_from_raw(e)),
        Err(e) => warn!("error sending set DNS servers request: {:?}", e),
    }

    dns_server_watch_responders.send(dns_servers.consolidated_dns_servers());

    networks_service.update(network::NetworkRegistryUpdate::dns(dns_servers)).await;
}

/// Creates a stream of RDNSS DNS updates. Returns None if the protocol
/// is not available on the system, indicating the protocol should not be used.
pub(super) async fn create_rdnss_stream(
    watcher_provider: &fnet_ndp::RouterAdvertisementOptionWatcherProviderProxy,
    interface_id: u64,
) -> Option<
    Result<
        impl Stream<
            Item = Result<
                (DnsWatcherResultPayload, Vec<fnet_name::DnsServer_>, DnsServerLifetime),
                fidl::Error,
            >,
        > + use<>,
        fidl::Error,
    >,
> {
    let watcher_result = fnet_ndp_ext::create_watcher_stream(
        &watcher_provider,
        &fnet_ndp::RouterAdvertisementOptionWatcherParams {
            interest_types: Some(vec![
                packet_formats_ndp::options::NdpOptionType::RecursiveDnsServer.into(),
            ]),
            interest_interface_id: Some(interface_id),
            ..Default::default()
        },
    )
    .await?;

    // This cannot be directly returned using `?` operator since the
    // function returns an Option.
    let watcher = match watcher_result {
        Ok(res) => res,
        Err(e) => return Some(Err(e)),
    };

    Some(Ok(watcher.filter_map(move |entry_res| async move {
        let entry = match entry_res {
            Ok(entry) => entry,
            Err(fnet_ndp_ext::OptionWatchStreamError::Fidl(e)) => {
                return Some(Err(e));
            }
            Err(fnet_ndp_ext::OptionWatchStreamError::Conversion(e)) => {
                // Netstack didn't uphold the invariant to populate the
                // fields for `OptionWatchEntry`.
                error!("Failed to convert OptionWatchStream item: {e:?}");
                return None;
            }
        };
        match entry {
            fnet_ndp_ext::OptionWatchStreamItem::Entry(entry) => {
                let router_address = entry.source_address;
                match entry.try_parse_as_rdnss() {
                    fnet_ndp_ext::TryParseAsOptionResult::Parsed(option) => {
                        let lifetime = match option.lifetime() {
                            Some(packet_formats_ndp::NonZeroNdpLifetime::Finite(duration)) => {
                                DnsServerLifetime::Seconds(duration.get().as_secs() as u32)
                            }
                            Some(packet_formats_ndp::NonZeroNdpLifetime::Infinite) => {
                                DnsServerLifetime::Seconds(u32::MAX)
                            }
                            None => DnsServerLifetime::Seconds(0),
                        };
                        let servers = match lifetime {
                            DnsServerLifetime::Undefined => vec![],
                            DnsServerLifetime::Seconds(_) => option
                                .iter_addresses()
                                .iter()
                                .map(|addr| fnet_name::DnsServer_ {
                                    address: Some(fnet::SocketAddress::Ipv6(
                                        fnet::Ipv6SocketAddress {
                                            address: fnet::Ipv6Address { addr: addr.ipv6_bytes() },
                                            port: DNS_PORT,
                                            // Determine whether the address has a zone or not in
                                            // accordance with
                                            // https://datatracker.ietf.org/doc/html/rfc8106
                                            zone_index: addr
                                                .scope()
                                                .can_have_zone()
                                                .then_some(interface_id)
                                                .unwrap_or_default(),
                                        },
                                    )),
                                    source: Some(fnet_name::DnsServerSource::Ndp(
                                        fnet_name::NdpDnsServerSource {
                                            source_interface: Some(interface_id),
                                            ..Default::default()
                                        },
                                    )),
                                    ..Default::default()
                                })
                                .collect::<Vec<_>>(),
                        };
                        Some(Ok((
                            DnsWatcherResultPayload::Ndp { router_address },
                            servers,
                            lifetime,
                        )))
                    }
                    fnet_ndp_ext::TryParseAsOptionResult::OptionTypeMismatch => {
                        // Netstack didn't respect our interest configuration.
                        error!("Option type provided did not match RDNSS option type");
                        None
                    }
                    fnet_ndp_ext::TryParseAsOptionResult::ParseErr(err) => {
                        // A network peer could have included an invalid RDNSS option.
                        warn!("Error while parsing as OptionResult: {err:?}");
                        None
                    }
                }
            }
            fnet_ndp_ext::OptionWatchStreamItem::Dropped(num) => {
                warn!(
                    "The server dropped ({num}) NDP options \
                    due to the HangingGet falling behind"
                );
                None
            }
        }
    })))
}

pub(super) async fn add_rdnss_watcher(
    watcher_provider: &fnet_ndp::RouterAdvertisementOptionWatcherProviderProxy,
    interface_id: crate::InterfaceId,
    watchers: &mut crate::DnsServerWatchers<'_>,
) -> Result<(), anyhow::Error> {
    let source = DnsServersUpdateSource::Ndp { interface_id: interface_id.get() };

    // Returns None when RouterAdvertisementOptionWatcherProvider isn't available on the system.
    let stream = create_rdnss_stream(watcher_provider, interface_id.get()).await;

    match stream {
        Some(result) => {
            let tagged_stream = result
                .context("failed to create watcher stream")?
                .tagged(source)
                .map(|(source, res)| match res {
                    Ok((payload, servers, lifetime)) => {
                        DnsServerUpdate { source, payload, lifetime, result: Ok(servers) }
                    }
                    Err(e) => DnsServerUpdate {
                        source,
                        payload: DnsWatcherResultPayload::Generic,
                        lifetime: DnsServerLifetime::Undefined,
                        result: Err(e),
                    },
                })
                .boxed();
            if let Some(o) = watchers.insert(source, tagged_stream) {
                let _: Pin<Box<BoxStream<'_, _>>> = o;
                unreachable!("DNS server watchers must not contain key {:?}", source);
            }
            info!("started NDP watcher on host interface (id={interface_id})");
        }
        None => {
            info!(
                "NDP protocol unavailable: not starting watcher for interface (id={interface_id})"
            );
        }
    }
    Ok(())
}

pub(super) async fn remove_rdnss_watcher(
    lookup_admin: &fnet_name::LookupAdminProxy,
    dns_servers: &mut DnsServers,
    dns_server_watch_responders: &mut DnsServerWatchResponders,
    netpol_networks_service: &mut network::NetpolNetworksService,
    interface_id: crate::InterfaceId,
    watchers: &mut crate::DnsServerWatchers<'_>,
) {
    let source = DnsServersUpdateSource::Ndp { interface_id: interface_id.get() };

    if let None = watchers.remove(&source) {
        // It's surprising that the DNS Watcher for the interface doesn't exist
        // when the RDNSS stream is getting removed, but this can happen
        // when multiple futures try to stop the NDP watcher at the same time.
        warn!(
            "DNS Watcher for key not present; multiple futures stopped NDP \
            watcher for key {:?}; interface_id={}",
            source, interface_id
        );
    }

    update_servers(
        lookup_admin,
        dns_servers,
        dns_server_watch_responders,
        netpol_networks_service,
        source,
        vec![],
    )
    .await;
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ConnectionId(usize);

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct UpdateGeneration(usize);

/// Tracks the currently registered `fnet_name::DnsServerWatcherWatchServersResponder`s.
///
/// Keeps track of which connection ID has been notified for which generation of
/// the DNS server list.
#[derive(Default)]
pub(crate) struct DnsServerWatchResponders {
    /// The current generation. It gets incremented every time the `responders`
    /// list gets emptied by a call to `DnsServerWatchResponders::take`
    generation: UpdateGeneration,

    /// Tracks the last generation for which a DNS server list update has been sent to each client.
    generations: HashMap<ConnectionId, UpdateGeneration>,

    /// The list of registered responders, indexed by their associated client ID.
    responders: HashMap<ConnectionId, fnet_name::DnsServerWatcherWatchServersResponder>,
}

impl DnsServerWatchResponders {
    fn send(&mut self, next_servers: Vec<fnet_name::DnsServer_>) {
        let responders = std::mem::take(&mut self.responders);
        self.generation.0 += 1;
        for (id, responder) in responders {
            match responder.send(&next_servers) {
                Ok(()) => {
                    let _: Option<UpdateGeneration> = self.generations.insert(id, self.generation);
                }
                Err(e) => warn!("Error responding to DnsServerWatcher request: {e:?}"),
            }
        }
    }

    /// Handles a call to `fuchsia.net.name/DnsServerWatcher.WatchServers`, the
    /// responder may be called immediately, or stored for later.
    pub(crate) fn handle_request(
        &mut self,
        id: ConnectionId,
        request: Result<fnet_name::DnsServerWatcherRequest, fidl::Error>,
        servers: &DnsServers,
    ) -> Result<(), fidl::Error> {
        use std::collections::hash_map::Entry;
        match request {
            Ok(fnet_name::DnsServerWatcherRequest::WatchServers { responder }) => {
                match self.responders.entry(id) {
                    Entry::Occupied(_) => {
                        warn!(
                            "Only one call to fuchsia.net.name/DnsServerWatcher.WatchServers \
                            may be active at once"
                        );
                        responder.control_handle().shutdown()
                    }
                    Entry::Vacant(vacant_entry) => {
                        // None is always less than any Some.
                        // See: https://doc.rust-lang.org/std/option/index.html#comparison-operators
                        if self.generations.get(&id) < Some(&self.generation) {
                            let _: Option<_> = self.generations.insert(id, self.generation);
                            responder.send(&servers.consolidated_dns_servers())?;
                        } else {
                            let _: &fnet_name::DnsServerWatcherWatchServersResponder =
                                vacant_entry.insert(responder);
                        }
                    }
                }
            }
            Err(e) => {
                error!("fuchsia.net.name/DnsServerWatcher request error: {:?}", e)
            }
        }

        Ok(())
    }
}

/// Keep track of all of the connected clients of
/// `fuchsia.net.name/DnsServerWatcher` and assign each of them a unique ID.
#[derive(Default)]
pub(crate) struct DnsServerWatcherRequestStreams {
    /// The ID to be assigned to the next connection.
    next_id: ConnectionId,

    /// The currently connected clients.
    request_streams:
        futures::stream::SelectAll<Tagged<ConnectionId, fnet_name::DnsServerWatcherRequestStream>>,
}

impl DnsServerWatcherRequestStreams {
    pub fn handle_request_stream(&mut self, req_stream: fnet_name::DnsServerWatcherRequestStream) {
        self.request_streams.push(req_stream.tagged(self.next_id));
        self.next_id.0 += 1;
    }
}

impl futures::Stream for DnsServerWatcherRequestStreams {
    type Item = (ConnectionId, Result<fnet_name::DnsServerWatcherRequest, fidl::Error>);

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        std::pin::Pin::new(&mut self.request_streams).poll_next(cx)
    }
}

impl futures::stream::FusedStream for DnsServerWatcherRequestStreams {
    fn is_terminated(&self) -> bool {
        self.request_streams.is_terminated()
    }
}

/// Tracks the expiry of NDP-learned DNS servers.
///
/// Interface ID - DNS server address pairs learned via NDP have associated lifetimes. These
/// lifetimes can be updated by subsequent NDP messages. `NdpDnsExpiryTracker` tracks all of these
/// lifetimes and removes DNS entries when they expire.
pub(crate) struct NdpDnsExpiryTracker {
    // Monotonically increasing generation counter.
    next_generation: u64,

    // Lifetimes are tracked by interface ID - DNS server address pairs.  Whenever a lifetime for
    // one of these pairs is updated, a new generation is assigned and a new timer is started.
    // This ensures that timers associated with older generations do not cause the removal of a DNS
    // entry whose lifetime has been updated.
    generations: HashMap<(crate::InterfaceId, net_types::ip::Ipv6Addr), u64>,

    // Storage for the timers associated with interface ID - server pairs and the generation
    // associated with the timer.
    timers: FuturesUnordered<
        Pin<Box<dyn Future<Output = (crate::InterfaceId, net_types::ip::Ipv6Addr, u64)> + Send>>,
    >,
}

impl NdpDnsExpiryTracker {
    pub fn new() -> Self {
        Self {
            next_generation: 1,
            generations: HashMap::new(),
            timers: futures::stream::FuturesUnordered::new(),
        }
    }

    pub fn record_expiry(
        &mut self,
        interface_id: crate::InterfaceId,
        router_address: net_types::ip::Ipv6Addr,
        lifetime_seconds: u32,
    ) {
        let key = (interface_id, router_address);

        // If lifetime is infinite, we don't need a timer.
        if lifetime_seconds == u32::MAX {
            let _: Option<u64> = self.generations.remove(&key);
            return;
        }

        let current_generation = self.next_generation;
        self.next_generation += 1;
        let _: Option<u64> = self.generations.insert(key, current_generation);

        let timer = fuchsia_async::Timer::new(
            zx::MonotonicDuration::from_seconds(lifetime_seconds as i64).after_now(),
        )
        .map(move |()| (interface_id, router_address, current_generation));

        self.timers.push(Box::pin(timer));
    }
}

impl Stream for NdpDnsExpiryTracker {
    type Item = (crate::InterfaceId, net_types::ip::Ipv6Addr);

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        loop {
            if self.timers.is_empty() {
                return std::task::Poll::Pending;
            }
            match Pin::new(&mut self.timers).poll_next(cx) {
                std::task::Poll::Ready(Some((interface_id, router_address, generation))) => {
                    let key = (interface_id, router_address);
                    if self.generations.get(&key) == Some(&generation) {
                        let _: u64 = self.generations.remove(&key).expect("generation must exist");
                        return std::task::Poll::Ready(Some(key));
                    }
                    // Superseded timer, loop again
                }
                std::task::Poll::Ready(None) => {
                    return std::task::Poll::Pending;
                }
                std::task::Poll::Pending => return std::task::Poll::Pending,
            }
        }
    }
}

impl futures::stream::FusedStream for NdpDnsExpiryTracker {
    fn is_terminated(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;
    use assert_matches::assert_matches;
    use fuchsia_component::server::{ServiceFs, ServiceFsDir};
    use fuchsia_component_test::{
        Capability, ChildOptions, LocalComponentHandles, RealmBuilder, RealmInstance, Ref, Route,
    };
    use futures::channel::mpsc;
    use futures::{SinkExt as _, TryFutureExt as _, TryStreamExt as _};
    use net_declare::{fidl_socket_addr, net_ip_v6};
    use pretty_assertions::assert_eq;
    use std::sync::LazyLock;
    use std::task::Poll;

    use super::*;

    enum StubbedServices {
        LookupAdmin(fnet_name::LookupAdminRequestStream),
    }

    async fn run_lookup_admin(handles: LocalComponentHandles) -> Result<(), anyhow::Error> {
        let mut fs = ServiceFs::new();
        let _: &mut ServiceFsDir<'_, _> =
            fs.dir("svc").add_fidl_service(StubbedServices::LookupAdmin);
        let _: &mut ServiceFs<_> = fs.serve_connection(handles.outgoing_dir)?;

        fs.for_each_concurrent(0, move |StubbedServices::LookupAdmin(stream)| async move {
            stream
                .try_for_each(|request| async move {
                    match request {
                        fidl_fuchsia_net_name::LookupAdminRequest::SetDnsServers { .. } => {
                            // Silently ignore this request.
                        }
                        fidl_fuchsia_net_name::LookupAdminRequest::GetDnsServers { .. } => {
                            unimplemented!("Unused in this test")
                        }
                    }
                    Ok(())
                })
                .await
                .context("Failed to serve request stream")
                .unwrap_or_else(|e| warn!("Error encountered: {:?}", e))
        })
        .await;

        Ok(())
    }

    enum IncomingService {
        DnsServerWatcher(fnet_name::DnsServerWatcherRequestStream),
    }

    async fn run_dns_server_watcher(
        handles: LocalComponentHandles,
        mut receiver: mpsc::Receiver<(crate::DnsServersUpdateSource, Vec<fnet_name::DnsServer_>)>,
    ) -> Result<(), anyhow::Error> {
        let connection = handles.connect_to_protocol()?;

        let mut fs = ServiceFs::new();
        let _: &mut ServiceFsDir<'_, _> =
            fs.dir("svc").add_fidl_service(IncomingService::DnsServerWatcher);
        let _: &mut ServiceFs<_> = fs.serve_connection(handles.outgoing_dir)?;

        let mut dns_server_watcher_incoming_requests = DnsServerWatcherRequestStreams::default();
        let mut dns_servers = DnsServers::default();
        let mut dns_server_watch_responders = DnsServerWatchResponders::default();
        let mut netpol_networks_service = network::NetpolNetworksService::default();

        let mut fs = futures::StreamExt::fuse(fs);

        loop {
            futures::select! {
                req_stream = fs.select_next_some() => {
                    match req_stream {
                        IncomingService::DnsServerWatcher(stream) => {
                            dns_server_watcher_incoming_requests.handle_request_stream(stream)
                        }
                    }
                }
                req = dns_server_watcher_incoming_requests.select_next_some() => {
                    let (id, req) = req;
                    dns_server_watch_responders.handle_request(
                        id,
                        req,
                        &dns_servers,
                    )?;
                }
                update = receiver.select_next_some() => {
                    let (source, servers) = update;
                    update_servers(
                        &connection,
                        &mut dns_servers,
                        &mut dns_server_watch_responders,
                        &mut netpol_networks_service,
                        source,
                        servers,
                    ).await
                }
            }
        }
    }

    async fn setup_test() -> Result<
        (RealmInstance, mpsc::Sender<(crate::DnsServersUpdateSource, Vec<fnet_name::DnsServer_>)>),
        anyhow::Error,
    > {
        let (tx, rx) = mpsc::channel(1);
        let builder = RealmBuilder::new().await?;
        let admin_server = builder
            .add_local_child(
                "lookup_admin",
                move |handles: LocalComponentHandles| Box::pin(run_lookup_admin(handles)),
                ChildOptions::new(),
            )
            .await?;

        let dns_server_watcher = builder
            .add_local_child(
                "dns_server_watcher",
                {
                    let rx = fuchsia_sync::Mutex::new(Some(rx));
                    move |handles: LocalComponentHandles| {
                        Box::pin(run_dns_server_watcher(
                            handles,
                            rx.lock()
                                .take()
                                .expect("Only one instance of run_dns_server_watcher should exist"),
                        ))
                    }
                },
                ChildOptions::new(),
            )
            .await?;

        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<fnet_name::DnsServerWatcherMarker>())
                    .from(&dns_server_watcher)
                    .to(Ref::parent()),
            )
            .await?;
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<fnet_name::LookupAdminMarker>())
                    .from(&admin_server)
                    .to(&dns_server_watcher),
            )
            .await?;

        let realm = builder.build().await?;

        Ok((realm, tx))
    }

    fn server(address: fidl_fuchsia_net::SocketAddress) -> fnet_name::DnsServer_ {
        fnet_name::DnsServer_ { address: Some(address), ..fnet_name::DnsServer_::default() }
    }

    #[fuchsia::test]
    async fn test_dns_server_watcher() -> Result<(), anyhow::Error> {
        let (realm, mut tx) = setup_test().await?;

        let watcher1: fnet_name::DnsServerWatcherProxy = realm
            .root
            .connect_to_protocol_at_exposed_dir()
            .context("While connecting to DnsServerWatcher")?;
        let watcher2: fnet_name::DnsServerWatcherProxy = realm
            .root
            .connect_to_protocol_at_exposed_dir()
            .context("While connecting to DnsServerWatcher")?;

        assert_eq!(watcher1.watch_servers().await?, vec![]);
        assert_eq!(watcher2.watch_servers().await?, vec![]);

        // This next call to watch_servers() should hang, so we expect the on_timeout response.
        let mut watcher1_call = watcher1.watch_servers().fuse();
        futures::select! {
            _ = watcher1_call => {
                return Err(
                    anyhow!("WatchServers should not respond here, there have been no updates")
                );
            },
            _ = fuchsia_async::Timer::new(std::time::Duration::from_millis(100)).fuse() => {}
        }

        // Insert a server from the "Default" source (statically defined).
        let (watch1, watch2, _) = futures::try_join!(
            // This call to watch_servers should now resolve.
            watcher1_call.map_err(|e| anyhow::Error::from(e)),
            watcher2.watch_servers().map_err(|e| anyhow::Error::from(e)),
            tx.send((
                DnsServersUpdateSource::Default,
                vec![server(fidl_socket_addr!("203.0.113.1:1"))],
            ))
            .map_err(|e| anyhow::Error::from(e)),
        )?;
        assert_eq!(watch1, vec![server(fidl_socket_addr!("203.0.113.1:1")),]);
        assert_eq!(watch2, vec![server(fidl_socket_addr!("203.0.113.1:1")),]);

        // Insert a server derived from DHCPv4 interface 1.
        let (watch1, watch2, _) = futures::try_join!(
            watcher1.watch_servers().map_err(|e| anyhow::Error::from(e)),
            watcher2.watch_servers().map_err(|e| anyhow::Error::from(e)),
            tx.send((
                DnsServersUpdateSource::Dhcpv4 { interface_id: 1 },
                vec![server(fidl_socket_addr!("203.0.113.1:2")),],
            ))
            .map_err(|e| anyhow::Error::from(e)),
        )?;
        // The DHCPv4 is expected to be first since the "Default" source is
        // given the lowest priority.
        let expectation = vec![
            server(fidl_socket_addr!("203.0.113.1:2")),
            server(fidl_socket_addr!("203.0.113.1:1")),
        ];
        assert_eq!(watch1, expectation);
        assert_eq!(watch2, expectation);

        // Insert a server derived from DHCPv6 interface 1. Also, only have watcher 1 do the watch.
        let (watch1, _) = futures::try_join!(
            watcher1.watch_servers().map_err(|e| anyhow::Error::from(e)),
            tx.send((
                DnsServersUpdateSource::Dhcpv6 { interface_id: 1 },
                vec![server(fidl_socket_addr!("[2001:db8::]:1")),],
            ))
            .map_err(|e| anyhow::Error::from(e)),
        )?;
        // DHCPv4 is higher priority than DHCPv6, but Default is still the lowest.
        let expectation = vec![
            server(fidl_socket_addr!("203.0.113.1:2")),
            server(fidl_socket_addr!("[2001:db8::]:1")),
            server(fidl_socket_addr!("203.0.113.1:1")),
        ];
        assert_eq!(watch1, expectation);

        // Update the default servers while no watcher is watching. This should
        // increment the generation, meaning that both watchers should respond
        // immediately upon request.
        tx.send((
            DnsServersUpdateSource::Default,
            vec![fnet_name::DnsServer_ {
                address: Some(fidl_socket_addr!("203.0.113.1:5")),
                ..fnet_name::DnsServer_::default()
            }],
        ))
        .await?;
        let (watch1, watch2) = futures::try_join!(
            watcher1.watch_servers().map_err(|e| anyhow::Error::from(e)),
            watcher2.watch_servers().map_err(|e| anyhow::Error::from(e)),
        )?;
        // DHCPv4 is higher priority than DHCPv6, but Default is still the lowest.
        let expectation = vec![
            server(fidl_socket_addr!("203.0.113.1:2")),
            server(fidl_socket_addr!("[2001:db8::]:1")),
            server(fidl_socket_addr!("203.0.113.1:5")),
        ];
        assert_eq!(watch1, expectation);

        // watcher2 has skipped the previous update and just received the most up-to-date.
        assert_eq!(watch2, expectation);

        Ok(())
    }

    // Note: The `lifetime_bytes` are parsed as seconds by the netstack.
    fn run_create_rdnss_stream_test_with_lifetime(
        lifetime_bytes: [u8; 4],
    ) -> (DnsWatcherResultPayload, Vec<fnet_name::DnsServer_>, DnsServerLifetime) {
        let mut exec = fuchsia_async::TestExecutor::new();

        let (provider_proxy, mut provider_stream) = fidl::endpoints::create_proxy_and_stream::<
            fnet_ndp::RouterAdvertisementOptionWatcherProviderMarker,
        >();

        const IFACE_ID: u64 = 1;
        let expected_router_address = net_ip_v6!("fe80::1");

        // Start create_rdnss_stream call.
        let create_stream_fut = create_rdnss_stream(&provider_proxy, IFACE_ID);
        futures::pin_mut!(create_stream_fut);
        assert!(exec.run_until_stalled(&mut create_stream_fut).is_pending());

        // Provider receives NewRouterAdvertisementOptionWatcher request.
        let mut provider_req_fut = provider_stream.next();
        let provider_req = assert_matches!(
            exec.run_until_stalled(&mut provider_req_fut),
            Poll::Ready(Some(Ok(req))) => req
        );
        let (watcher, params, _control_handle) = provider_req
            .into_new_router_advertisement_option_watcher()
            .expect("into new watcher request");
        assert_eq!(params.interest_interface_id, Some(IFACE_ID));

        // Watcher receives Probe request.
        let mut watcher_stream = watcher.into_stream();
        let mut watcher_req_fut = watcher_stream.next();
        let watcher_req = assert_matches!(
            exec.run_until_stalled(&mut watcher_req_fut),
            Poll::Ready(Some(Ok(req))) => req
        );
        let responder = assert_matches!(
            watcher_req,
            fnet_ndp::OptionWatcherRequest::Probe { responder } => responder
        );
        responder.send().expect("send Probe response succeeded");

        // Complete create_rdnss_stream and retrieve option watcher stream.
        let Poll::Ready(Some(Ok(stream))) = exec.run_until_stalled(&mut create_stream_fut) else {
            panic!("create_rdnss_stream failed");
        };
        futures::pin_mut!(stream);

        // Start polling for next item on the stream.
        let mut next_item_fut = stream.next();
        assert!(exec.run_until_stalled(&mut next_item_fut).is_pending());

        // Watcher receives WatchOptions request.
        let watcher_req_fut = watcher_stream.next();
        futures::pin_mut!(watcher_req_fut);
        let watcher_req = assert_matches!(
            exec.run_until_stalled(&mut watcher_req_fut),
            Poll::Ready(Some(Ok(req))) => req
        );
        let responder = assert_matches!(
            watcher_req,
            fnet_ndp::OptionWatcherRequest::WatchOptions { responder } => responder
        );

        let mut rdnss_body = vec![0, 0]; // Reserved (2 bytes)
        rdnss_body.extend_from_slice(&lifetime_bytes); // Lifetime (4 bytes)
        rdnss_body.extend_from_slice(&[0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);

        let entry = fnet_ndp::OptionWatchEntry {
            interface_id: Some(IFACE_ID),
            source_address: Some(fnet::Ipv6Address { addr: expected_router_address.ipv6_bytes() }),
            option_type: Some(25),
            body: Some(rdnss_body),
            ..Default::default()
        };
        responder.send(&[entry], 0).expect("send response succeeded");

        // Poll stream to receive item.
        assert_matches!(
            exec.run_until_stalled(&mut next_item_fut),
            Poll::Ready(Some(Ok((payload, servers, lifetime)))) => (payload, servers, lifetime)
        )
    }

    static IFACE_ID: LazyLock<crate::InterfaceId> =
        LazyLock::new(|| crate::InterfaceId::new(1).unwrap());
    static ROUTER_ADDR: LazyLock<net_types::ip::Ipv6Addr> =
        LazyLock::new(|| net_types::ip::Ipv6Addr::from([10; 16]));

    #[test]
    fn test_ndp_dns_expiry_tracker_timer_expiry() {
        let mut exec = fuchsia_async::TestExecutor::new_with_fake_time();
        let mut tracker = NdpDnsExpiryTracker::new();

        // Record a 5-second expiry.
        tracker.record_expiry(*IFACE_ID, *ROUTER_ADDR, 5);

        // Before 5 seconds, poll should be Pending.
        let mut next_fut = tracker.next();
        assert_matches!(exec.run_until_stalled(&mut next_fut), Poll::Pending);

        // Advance fake time past 5 seconds and wake timers.
        exec.set_fake_time(exec.now() + zx::MonotonicDuration::from_seconds(6));
        assert!(exec.wake_expired_timers());

        // Now poll should be Ready with (IFACE_ID, router_addr).
        assert_matches!(
            exec.run_until_stalled(&mut next_fut),
            Poll::Ready(Some((iface_id, router_addr))) if iface_id == *IFACE_ID && router_addr == *ROUTER_ADDR
        );
    }

    #[test]
    fn test_ndp_dns_expiry_tracker_superseded_renewal() {
        let mut exec = fuchsia_async::TestExecutor::new_with_fake_time();
        let mut tracker = NdpDnsExpiryTracker::new();

        // Record a 5-second expiry, then immediately supersede it with a 20-second expiry.
        tracker.record_expiry(*IFACE_ID, *ROUTER_ADDR, 5);
        tracker.record_expiry(*IFACE_ID, *ROUTER_ADDR, 20);

        // Advance fake time past 5 seconds (to 6 seconds) and wake timers.
        exec.set_fake_time(exec.now() + zx::MonotonicDuration::from_seconds(6));
        assert!(!exec.wake_expired_timers());

        // The 5-second timer fired but was for an older generation, so tracker ignores it.
        let mut next_fut = tracker.next();
        assert_matches!(exec.run_until_stalled(&mut next_fut), Poll::Pending);

        // Advance fake time past 20 seconds (to 21 seconds total) and wake timers.
        exec.set_fake_time(exec.now() + zx::MonotonicDuration::from_seconds(15));
        assert!(exec.wake_expired_timers());

        // Now poll should be Ready with (IFACE_ID, router_addr).
        assert_matches!(
            exec.run_until_stalled(&mut next_fut),
            Poll::Ready(Some((iface_id, router_addr))) if iface_id == *IFACE_ID && router_addr == *ROUTER_ADDR
        );
    }

    #[test]
    fn test_ndp_dns_expiry_tracker_infinite_lifetime() {
        let mut tracker = NdpDnsExpiryTracker::new();

        // Infinite lifetime (u32::MAX) should not schedule a timer.
        tracker.record_expiry(*IFACE_ID, *ROUTER_ADDR, u32::MAX);
        assert!(tracker.generations.is_empty())
    }

    #[test]
    fn test_ndp_dns_expiry_tracker_infinite_lifetime_reset_race() {
        let mut exec = fuchsia_async::TestExecutor::new_with_fake_time();
        let mut tracker = NdpDnsExpiryTracker::new();

        // Record a 5-second expiry (generation 1 assigned).
        tracker.record_expiry(*IFACE_ID, *ROUTER_ADDR, 5);

        // Record an infinite lifetime (removes key from generations).
        tracker.record_expiry(*IFACE_ID, *ROUTER_ADDR, u32::MAX);

        // Record a 20-second expiry (generation 2 assigned via monotonic counter).
        tracker.record_expiry(*IFACE_ID, *ROUTER_ADDR, 20);

        // Advance fake time past 5 seconds (to 6 seconds) and wake timers.
        exec.set_fake_time(exec.now() + zx::MonotonicDuration::from_seconds(6));
        assert!(!exec.wake_expired_timers());

        // The 5-second timer (generation 1) fires, but is ignored because active generation is 2.
        let mut next_fut = tracker.next();
        assert_matches!(exec.run_until_stalled(&mut next_fut), Poll::Pending);

        // Advance fake time past 20 seconds (to 21 seconds total) and wake timers.
        exec.set_fake_time(exec.now() + zx::MonotonicDuration::from_seconds(15));
        assert!(exec.wake_expired_timers());

        // Now poll should be Ready with (IFACE_ID, ROUTER_ADDR) at 20 seconds.
        assert_matches!(
            exec.run_until_stalled(&mut next_fut),
            Poll::Ready(Some((iface_id, router_addr))) => {
                assert_eq!(iface_id, *IFACE_ID);
                assert_eq!(router_addr, *ROUTER_ADDR);
            }
        );
    }

    #[test]
    fn test_create_rdnss_stream_router_address_extraction() {
        use net_declare::fidl_ip_v6;

        let expected_router_address =
            net_types::ip::Ipv6Addr::from([0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        let expected_lifetime = 120;
        let (payload, servers, lifetime) =
            run_create_rdnss_stream_test_with_lifetime([0, 0, 0, expected_lifetime]);

        assert_eq!(
            payload,
            DnsWatcherResultPayload::Ndp { router_address: expected_router_address }
        );
        assert_eq!(lifetime, DnsServerLifetime::Seconds(expected_lifetime.into()));
        assert_eq!(servers.len(), 1);
        assert_eq!(
            servers[0].address,
            Some(fnet::SocketAddress::Ipv6(fnet::Ipv6SocketAddress {
                address: fidl_ip_v6!("2001:db8::1"),
                port: DNS_PORT,
                zone_index: 0,
            }))
        );
    }

    #[test]
    fn test_create_rdnss_stream_infinite_lifetime() {
        let (_payload, _servers, lifetime) =
            run_create_rdnss_stream_test_with_lifetime([0xff, 0xff, 0xff, 0xff]);
        assert_eq!(lifetime, DnsServerLifetime::Seconds(u32::MAX));
    }

    #[test]
    fn test_create_rdnss_stream_zero_lifetime() {
        let (_payload, _servers, lifetime) =
            run_create_rdnss_stream_test_with_lifetime([0, 0, 0, 0]);
        assert_eq!(lifetime, DnsServerLifetime::Seconds(0));
    }
}
