// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implements a DHCPv6 client.
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::net::{IpAddr, SocketAddr};
use std::ops::Add;
use std::pin::Pin;
use std::str::FromStr as _;
use std::time::Duration;

use fidl::endpoints::ServerEnd;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_dhcpv6::{
    ClientMarker, ClientRequest, ClientRequestStream, ClientWatchAddressResponder,
    ClientWatchPrefixesResponder, ClientWatchServersResponder, Duid, Empty, Lifetimes,
    LinkLayerAddress, LinkLayerAddressPlusTime, Prefix, PrefixDelegationConfig,
    RELAY_AGENT_AND_SERVER_LINK_LOCAL_MULTICAST_ADDRESS, RELAY_AGENT_AND_SERVER_PORT,
};
use fidl_fuchsia_net_dhcpv6_ext::{
    AddressConfig, ClientConfig, InformationConfig, NewClientParams,
};
use fidl_fuchsia_net_ext as fnet_ext;
use fidl_fuchsia_net_name as fnet_name;
use fuchsia_async as fasync;
use futures::{Future, FutureExt as _, StreamExt as _, TryStreamExt as _, select, stream};

use anyhow::{Context as _, Result};
use assert_matches::assert_matches;
use byteorder::{NetworkEndian, WriteBytesExt as _};
use dns_server_watcher::DEFAULT_DNS_PORT;
use log::{debug, warn};
use net_types::MulticastAddress as _;
use net_types::ip::{Ip as _, Ipv6, Ipv6Addr, Subnet, SubnetError};
use packet::ParsablePacket;
use packet_formats_dhcp::v6;
use rand::rngs::StdRng;

/// A thin wrapper around `zx::MonotonicInstant` that implements `dhcpv6_core::Instant`.
#[derive(PartialEq, Eq, PartialOrd, Ord, Copy, Clone, Debug)]
pub(crate) struct MonotonicInstant(zx::MonotonicInstant);

impl MonotonicInstant {
    fn now() -> MonotonicInstant {
        MonotonicInstant(zx::MonotonicInstant::get())
    }
}

impl dhcpv6_core::Instant for MonotonicInstant {
    fn duration_since(&self, MonotonicInstant(earlier): MonotonicInstant) -> Duration {
        let Self(this) = *self;

        let diff: zx::MonotonicDuration = this - earlier;

        Duration::from_nanos(diff.into_nanos().try_into().unwrap_or_else(|e| {
            panic!(
                "failed to calculate duration since {:?} with instant {:?}: {}",
                earlier, this, e,
            )
        }))
    }

    fn checked_add(&self, duration: Duration) -> Option<MonotonicInstant> {
        Some(self.add(duration))
    }
}

impl Add<Duration> for MonotonicInstant {
    type Output = MonotonicInstant;

    fn add(self, duration: Duration) -> MonotonicInstant {
        let MonotonicInstant(this) = self;
        MonotonicInstant(this + duration.into())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("fidl error")]
    Fidl(#[source] fidl::Error),
    #[error("got watch request while the previous one is pending")]
    DoubleWatch,
    #[error("unsupported DHCPv6 configuration")]
    UnsupportedConfigs,
    #[error("socket create error")]
    SocketCreate(std::io::Error),
    #[error("socket receive error")]
    SocketRecv(std::io::Error),
    #[error("unimplemented DHCPv6 functionality: {:?}()", _0)]
    Unimplemented(String),
}

/// Theoretical size limit for UDP datagrams.
///
/// NOTE: This does not take [jumbograms](https://tools.ietf.org/html/rfc2675) into account.
const MAX_UDP_DATAGRAM_SIZE: usize = 65_535;

#[pin_project::pin_project]
struct Timers {
    #[pin]
    retransmission: fasync::Timer,
    #[pin]
    refresh: fasync::Timer,
    #[pin]
    renew: fasync::Timer,
    #[pin]
    rebind: fasync::Timer,
    #[pin]
    restart_server_discovery: fasync::Timer,

    #[cfg(test)]
    scheduled: HashSet<dhcpv6_core::client::ClientTimerType>,
}

impl Default for Timers {
    fn default() -> Self {
        let unscheduled = || fasync::Timer::new(fasync::MonotonicInstant::INFINITE);
        Self {
            retransmission: unscheduled(),
            refresh: unscheduled(),
            renew: unscheduled(),
            rebind: unscheduled(),
            restart_server_discovery: unscheduled(),
            #[cfg(test)]
            scheduled: Default::default(),
        }
    }
}

/// A DHCPv6 client.
pub(crate) struct Client<S: for<'a> AsyncSocket<'a>> {
    /// The interface the client is running on.
    interface_id: fnet::InterfaceId,
    /// Stores the hash of the last observed version of DNS servers by a watcher.
    ///
    /// The client uses this hash to determine whether new changes in DNS servers are observed and
    /// updates should be replied to the watcher.
    last_observed_dns_hash: u64,
    /// Stores a responder to send DNS server updates.
    dns_responder: Option<ClientWatchServersResponder>,
    /// Stores a responder to send acquired addresses.
    address_responder: Option<ClientWatchAddressResponder>,
    /// Holds the discovered prefixes and their lifetimes.
    prefixes: HashMap<fnet::Ipv6AddressWithPrefix, Lifetimes>,
    /// Indicates whether or not the prefixes has changed since last yielded.
    prefixes_changed: bool,
    /// Stores a responder to send acquired prefixes.
    prefixes_responder: Option<ClientWatchPrefixesResponder>,
    /// Maintains the state for the client.
    state_machine: dhcpv6_core::client::ClientStateMachine<MonotonicInstant, StdRng>,
    /// The socket used to communicate with DHCPv6 servers.
    socket: S,
    /// The address to send outgoing messages to.
    server_addr: SocketAddr,
    /// All timers.
    timers: Pin<Box<Timers>>,
    /// A stream of FIDL requests to this client.
    request_stream: ClientRequestStream,
}

/// A trait that allows stubbing [`fuchsia_async::net::UdpSocket`] in tests.
pub(crate) trait AsyncSocket<'a> {
    type RecvFromFut: Future<Output = Result<(usize, SocketAddr), std::io::Error>> + 'a;
    type SendToFut: Future<Output = Result<usize, std::io::Error>> + 'a;

    fn recv_from(&'a self, buf: &'a mut [u8]) -> Self::RecvFromFut;
    fn send_to(&'a self, buf: &'a [u8], addr: SocketAddr) -> Self::SendToFut;
}

impl<'a> AsyncSocket<'a> for fasync::net::UdpSocket {
    type RecvFromFut = fasync::net::UdpRecvFrom<'a>;
    type SendToFut = fasync::net::SendTo<'a>;

    fn recv_from(&'a self, buf: &'a mut [u8]) -> Self::RecvFromFut {
        self.recv_from(buf)
    }
    fn send_to(&'a self, buf: &'a [u8], addr: SocketAddr) -> Self::SendToFut {
        self.send_to(buf, addr)
    }
}

/// Converts `InformationConfig` to a collection of `v6::OptionCode`.
fn to_dhcpv6_option_codes(
    InformationConfig { dns_servers }: InformationConfig,
) -> Vec<v6::OptionCode> {
    dns_servers.then_some(v6::OptionCode::DnsServers).into_iter().collect()
}

fn to_configured_addresses(
    AddressConfig { address_count, preferred_addresses }: AddressConfig,
) -> Result<HashMap<v6::IAID, HashSet<Ipv6Addr>>, ClientError> {
    let preferred_addresses = preferred_addresses.unwrap_or(Vec::new());
    if preferred_addresses.len() > address_count.into() {
        return Err(ClientError::UnsupportedConfigs);
    }

    // TODO(https://fxbug.dev/42157844): make IAID consistent across
    // configurations.
    Ok((0..)
        .map(v6::IAID::new)
        .zip(
            preferred_addresses
                .into_iter()
                .map(|fnet::Ipv6Address { addr, .. }| HashSet::from([Ipv6Addr::from(addr)]))
                .chain(std::iter::repeat_with(HashSet::new)),
        )
        .take(address_count.into())
        .collect())
}

// The client only supports a single IA_PD.
//
// TODO(https://fxbug.dev/42065403): Support multiple IA_PDs.
const IA_PD_IAID: v6::IAID = v6::IAID::new(0);

/// Creates a state machine for the input client config.
fn create_state_machine(
    duid: Option<dhcpv6_core::ClientDuid>,
    ClientConfig {
        information_config,
        non_temporary_address_config,
        prefix_delegation_config,
    }: ClientConfig,
) -> Result<
    (
        dhcpv6_core::client::ClientStateMachine<MonotonicInstant, StdRng>,
        dhcpv6_core::client::Actions<MonotonicInstant>,
    ),
    ClientError,
> {
    let information_option_codes = to_dhcpv6_option_codes(information_config);
    let configured_non_temporary_addresses = to_configured_addresses(non_temporary_address_config)?;
    let configured_delegated_prefixes = prefix_delegation_config
        .map(|prefix_delegation_config| {
            let prefix = match prefix_delegation_config {
                PrefixDelegationConfig::Empty(Empty {}) => Ok(None),
                PrefixDelegationConfig::PrefixLength(prefix_len) => {
                    if prefix_len == 0 {
                        // Should have used `PrefixDelegationConfig::Empty`.
                        return Err(ClientError::UnsupportedConfigs);
                    }

                    Subnet::new(Ipv6::UNSPECIFIED_ADDRESS, prefix_len).map(Some)
                }
                PrefixDelegationConfig::Prefix(fnet::Ipv6AddressWithPrefix {
                    addr: fnet::Ipv6Address { addr, .. },
                    prefix_len,
                }) => {
                    let addr = Ipv6Addr::from_bytes(addr);
                    if addr == Ipv6::UNSPECIFIED_ADDRESS {
                        // Should have used `PrefixDelegationConfig::PrefixLength`.
                        return Err(ClientError::UnsupportedConfigs);
                    }

                    Subnet::new(addr, prefix_len).map(Some)
                }
            };

            match prefix {
                Ok(o) => Ok(HashMap::from([(IA_PD_IAID, HashSet::from_iter(o.into_iter()))])),
                Err(SubnetError::PrefixTooLong | SubnetError::HostBitsSet) => {
                    Err(ClientError::UnsupportedConfigs)
                }
            }
        })
        .transpose()?;

    let now = MonotonicInstant::now();
    match (
        information_option_codes.is_empty(),
        configured_non_temporary_addresses.is_empty(),
        configured_delegated_prefixes,
    ) {
        (true, true, None) => Err(ClientError::UnsupportedConfigs),
        (false, true, None) => {
            if duid.is_some() {
                Err(ClientError::UnsupportedConfigs)
            } else {
                Ok(dhcpv6_core::client::ClientStateMachine::start_stateless(
                    information_option_codes,
                    rand::make_rng(),
                    now,
                ))
            }
        }
        (
            _request_information,
            _configure_non_temporary_addresses,
            configured_delegated_prefixes,
        ) => Ok(dhcpv6_core::client::ClientStateMachine::start_stateful(
            if let Some(duid) = duid {
                duid
            } else {
                return Err(ClientError::UnsupportedConfigs);
            },
            configured_non_temporary_addresses,
            configured_delegated_prefixes.unwrap_or_else(Default::default),
            information_option_codes,
            rand::make_rng(),
            now,
        )),
    }
}

/// Calculates a hash for the input.
fn hash<H: Hash>(h: &H) -> u64 {
    let mut dh = DefaultHasher::new();
    h.hash(&mut dh);
    dh.finish()
}

fn subnet_to_address_with_prefix(prefix: Subnet<Ipv6Addr>) -> fnet::Ipv6AddressWithPrefix {
    fnet::Ipv6AddressWithPrefix {
        addr: fnet::Ipv6Address { addr: prefix.network().ipv6_bytes() },
        prefix_len: prefix.prefix(),
    }
}

impl<S: for<'a> AsyncSocket<'a>> Client<S> {
    /// Starts the client in `config`.
    pub(crate) async fn start(
        duid: Option<dhcpv6_core::ClientDuid>,
        config: ClientConfig,
        interface_id: fnet::InterfaceId,
        socket_fn: impl FnOnce() -> std::io::Result<S>,
        server_addr: SocketAddr,
        request_stream: ClientRequestStream,
    ) -> Result<Self, ClientError> {
        let (state_machine, actions) = create_state_machine(duid, config)?;
        let mut client = Self {
            state_machine,
            interface_id,
            socket: socket_fn().map_err(ClientError::SocketCreate)?,
            server_addr,
            request_stream,
            // Server watcher's API requires blocking iff the first call would return an empty list,
            // so initialize this field with a hash of an empty list.
            last_observed_dns_hash: hash(&Vec::<Ipv6Addr>::new()),
            dns_responder: None,
            address_responder: None,
            prefixes: Default::default(),
            prefixes_changed: false,
            prefixes_responder: None,
            timers: Box::pin(Default::default()),
        };
        client.run_actions(actions).await?;
        Ok(client)
    }

    /// Runs a list of actions sequentially.
    async fn run_actions(
        &mut self,
        actions: dhcpv6_core::client::Actions<MonotonicInstant>,
    ) -> Result<(), ClientError> {
        stream::iter(actions)
            .map(Ok)
            .try_fold(self, |client, action| async move {
                match action {
                    dhcpv6_core::client::Action::SendMessage(buf) => {
                        match client.socket.send_to(&buf, client.server_addr).await {
                            Ok(size) => assert_eq!(size, buf.len()),
                            Err(e) => warn!(
                                "failed to send message to {}: {}; will retransmit later",
                                client.server_addr, e
                            ),
                        };
                    }
                    dhcpv6_core::client::Action::ScheduleTimer(timer_type, timeout) => {
                        client.schedule_timer(timer_type, timeout)
                    }
                    dhcpv6_core::client::Action::CancelTimer(timer_type) => {
                        client.cancel_timer(timer_type)
                    }
                    dhcpv6_core::client::Action::UpdateDnsServers(servers) => {
                        client.maybe_send_dns_server_updates(servers)?;
                    }
                    dhcpv6_core::client::Action::IaNaUpdates(_) => {
                        // TODO(https://fxbug.dev/42178828): add actions to
                        // (re)schedule preferred and valid lifetime timers.
                        // TODO(https://fxbug.dev/42178817): Add
                        // action to remove the previous address.
                        // TODO(https://fxbug.dev/42177252): Add action to add
                        // the new address and cancel timers for old address.
                    }
                    dhcpv6_core::client::Action::IaPdUpdates(mut updates) => {
                        let updates = {
                            let ret =
                                updates.remove(&IA_PD_IAID).expect("Update missing for IAID");
                            debug_assert_eq!(updates, HashMap::new());
                            ret
                        };

                        let Self { prefixes, prefixes_changed, .. } = client;

                        let now = zx::MonotonicInstant::get();
                        let nonzero_timevalue_to_zx_time = |tv| match tv {
                            v6::NonZeroTimeValue::Finite(tv) => {
                                now + zx::MonotonicDuration::from_seconds(tv.get().into())
                            }
                            v6::NonZeroTimeValue::Infinity => zx::MonotonicInstant::INFINITE,
                        };

                        let calculate_lifetimes = |dhcpv6_core::client::Lifetimes {
                            preferred_lifetime,
                            valid_lifetime,
                        }| {
                            Lifetimes {
                                preferred_until: match preferred_lifetime {
                                    v6::TimeValue::Zero => zx::MonotonicInstant::ZERO,
                                    v6::TimeValue::NonZero(preferred_lifetime) => {
                                        nonzero_timevalue_to_zx_time(preferred_lifetime)
                                    },
                                }.into_nanos(),
                                valid_until: nonzero_timevalue_to_zx_time(valid_lifetime)
                                    .into_nanos(),
                            }
                        };

                        for (prefix, update) in updates.into_iter() {
                            let fidl_prefix = subnet_to_address_with_prefix(prefix);

                            match update {
                                dhcpv6_core::client::IaValueUpdateKind::Added(lifetimes) => {
                                    assert_matches!(
                                        prefixes.insert(
                                            fidl_prefix,
                                            calculate_lifetimes(lifetimes)
                                        ),
                                        None,
                                        "must not know about prefix {} to add it with lifetimes {:?}",
                                        prefix, lifetimes,
                                    );
                                }
                                dhcpv6_core::client::IaValueUpdateKind::UpdatedLifetimes(updated_lifetimes) => {
                                    assert_matches!(
                                        prefixes.get_mut(&fidl_prefix),
                                        Some(lifetimes) => {
                                            *lifetimes = calculate_lifetimes(updated_lifetimes);
                                        },
                                        "must know about prefix {} to update lifetimes with {:?}",
                                        prefix, updated_lifetimes,
                                    );
                                }
                                dhcpv6_core::client::IaValueUpdateKind::Removed => {
                                    assert_matches!(
                                        prefixes.remove(&fidl_prefix),
                                        Some(_),
                                        "must know about prefix {} to remove it",
                                        prefix
                                    );
                                }
                            }
                        }

                        // Mark the client has having updated prefixes so that
                        // callers of `WatchPrefixes` receive the update.
                        *prefixes_changed = true;
                        client.maybe_send_prefixes()?;
                    }
                };
                Ok(client)
            })
            .await
            .map(|_: &mut Client<S>| ())
    }

    /// Sends the latest DNS servers if a watcher is watching, and the latest set of servers are
    /// different from what the watcher has observed last time.
    fn maybe_send_dns_server_updates(&mut self, servers: Vec<Ipv6Addr>) -> Result<(), ClientError> {
        let servers_hash = hash(&servers);
        if servers_hash == self.last_observed_dns_hash {
            Ok(())
        } else {
            Ok(match self.dns_responder.take() {
                Some(responder) => {
                    self.send_dns_server_updates(responder, servers, servers_hash)?
                }
                None => (),
            })
        }
    }

    fn maybe_send_prefixes(&mut self) -> Result<(), ClientError> {
        let Self { prefixes, prefixes_changed, prefixes_responder, .. } = self;

        if !*prefixes_changed {
            return Ok(());
        }

        let responder = if let Some(responder) = prefixes_responder.take() {
            responder
        } else {
            return Ok(());
        };

        let prefixes = prefixes
            .iter()
            .map(|(prefix, lifetimes)| Prefix { prefix: *prefix, lifetimes: *lifetimes })
            .collect::<Vec<_>>();

        responder.send(&prefixes).map_err(ClientError::Fidl)?;
        *prefixes_changed = false;
        Ok(())
    }

    /// Sends a list of DNS servers to a watcher through the input responder and updates the last
    /// observed hash.
    fn send_dns_server_updates(
        &mut self,
        responder: ClientWatchServersResponder,
        servers: Vec<Ipv6Addr>,
        hash: u64,
    ) -> Result<(), ClientError> {
        let response: Vec<_> = servers
            .iter()
            .map(|addr| {
                let address = fnet::Ipv6Address { addr: addr.ipv6_bytes() };
                let zone_index = if addr.is_unicast_link_local() { self.interface_id } else { 0 };

                fnet_name::DnsServer_ {
                    address: Some(fnet::SocketAddress::Ipv6(fnet::Ipv6SocketAddress {
                        address,
                        zone_index,
                        port: DEFAULT_DNS_PORT,
                    })),
                    source: Some(fnet_name::DnsServerSource::Dhcpv6(
                        fnet_name::Dhcpv6DnsServerSource {
                            source_interface: Some(self.interface_id),
                            ..Default::default()
                        },
                    )),
                    ..Default::default()
                }
            })
            .collect();
        responder
            .send(&response)
            // The channel will be closed on error, so return an error to stop the client.
            .map_err(ClientError::Fidl)?;
        self.last_observed_dns_hash = hash;
        Ok(())
    }

    /// Schedules a timer for `timer_type` to fire at `instant`.
    ///
    /// If a timer for `timer_type` is already scheduled, the timer is
    /// updated to fire at the new time.
    fn schedule_timer(
        &mut self,
        timer_type: dhcpv6_core::client::ClientTimerType,
        MonotonicInstant(instant): MonotonicInstant,
    ) {
        let timers = self.timers.as_mut().project();
        let timer = match timer_type {
            dhcpv6_core::client::ClientTimerType::Retransmission => timers.retransmission,
            dhcpv6_core::client::ClientTimerType::Refresh => timers.refresh,
            dhcpv6_core::client::ClientTimerType::Renew => timers.renew,
            dhcpv6_core::client::ClientTimerType::Rebind => timers.rebind,
            dhcpv6_core::client::ClientTimerType::RestartServerDiscovery => {
                timers.restart_server_discovery
            }
        };
        #[cfg(test)]
        let _: bool = if instant == zx::MonotonicInstant::INFINITE {
            timers.scheduled.remove(&timer_type)
        } else {
            timers.scheduled.insert(timer_type)
        };
        timer.reset(fasync::MonotonicInstant::from_zx(instant));
    }

    /// Cancels a previously scheduled timer for `timer_type`.
    ///
    /// If a timer was not previously scheduled for `timer_type`, this
    /// call is effectively a no-op.
    fn cancel_timer(&mut self, timer_type: dhcpv6_core::client::ClientTimerType) {
        self.schedule_timer(timer_type, MonotonicInstant(zx::MonotonicInstant::INFINITE))
    }

    /// Handles a timeout.
    async fn handle_timeout(
        &mut self,
        timer_type: dhcpv6_core::client::ClientTimerType,
    ) -> Result<(), ClientError> {
        // This timer just fired.
        self.cancel_timer(timer_type);

        let actions = self.state_machine.handle_timeout(timer_type, MonotonicInstant::now());
        self.run_actions(actions).await
    }

    /// Handles a received message.
    async fn handle_message_recv(&mut self, mut msg: &[u8]) -> Result<(), ClientError> {
        let msg = match v6::Message::parse(&mut msg, ()) {
            Ok(msg) => msg,
            Err(e) => {
                // Discard invalid messages.
                //
                // https://tools.ietf.org/html/rfc8415#section-16.
                warn!("failed to parse received message: {}", e);
                return Ok(());
            }
        };
        let actions = self.state_machine.handle_message_receive(msg, MonotonicInstant::now());
        self.run_actions(actions).await
    }

    /// Handles a FIDL request sent to this client.
    fn handle_client_request(&mut self, request: ClientRequest) -> Result<(), ClientError> {
        debug!("handling client request: {:?}", request);
        match request {
            ClientRequest::WatchServers { responder } => match self.dns_responder {
                Some(_) => {
                    // Drop the previous responder to close the channel.
                    self.dns_responder = None;
                    // Return an error to stop the client because the channel is closed.
                    Err(ClientError::DoubleWatch)
                }
                None => {
                    let dns_servers = self.state_machine.get_dns_servers();
                    let servers_hash = hash(&dns_servers);
                    if servers_hash != self.last_observed_dns_hash {
                        // Something has changed from the last time, update the watcher.
                        let () =
                            self.send_dns_server_updates(responder, dns_servers, servers_hash)?;
                    } else {
                        // Nothing has changed, update the watcher later.
                        self.dns_responder = Some(responder);
                    }
                    Ok(())
                }
            },
            ClientRequest::WatchAddress { responder } => match self.address_responder.take() {
                // The responder will be dropped and cause the channel to be closed.
                Some(ClientWatchAddressResponder { .. }) => Err(ClientError::DoubleWatch),
                None => {
                    // TODO(https://fxbug.dev/42152192): Implement the address watcher.
                    warn!("WatchAddress call will block forever as it is unimplemented");
                    self.address_responder = Some(responder);
                    Ok(())
                }
            },
            ClientRequest::WatchPrefixes { responder } => match self.prefixes_responder.take() {
                // The responder will be dropped and cause the channel to be closed.
                Some(ClientWatchPrefixesResponder { .. }) => Err(ClientError::DoubleWatch),
                None => {
                    self.prefixes_responder = Some(responder);
                    self.maybe_send_prefixes()
                }
            },
            // TODO(https://fxbug.dev/42152193): Implement Shutdown.
            ClientRequest::Shutdown { responder: _ } => {
                Err(ClientError::Unimplemented("Shutdown".to_string()))
            }
        }
    }

    /// Handles the next event and returns the result.
    ///
    /// Takes a pre-allocated buffer to avoid repeated allocation.
    ///
    /// The returned `Option` is `None` if `request_stream` on the client is closed.
    async fn handle_next_event(&mut self, buf: &mut [u8]) -> Result<Option<()>, ClientError> {
        let timers = self.timers.as_mut().project();
        let timer_type = select! {
            () = timers.retransmission => {
                dhcpv6_core::client::ClientTimerType::Retransmission
            },
            () = timers.refresh => {
                dhcpv6_core::client::ClientTimerType::Refresh
            },
            () = timers.renew => {
                dhcpv6_core::client::ClientTimerType::Renew
            },
            () = timers.rebind => {
                dhcpv6_core::client::ClientTimerType::Rebind
            },
            () = timers.restart_server_discovery => {
                dhcpv6_core::client::ClientTimerType::RestartServerDiscovery
            },
            recv_from_res = self.socket.recv_from(buf).fuse() => {
                let (size, _addr) = recv_from_res.map_err(ClientError::SocketRecv)?;
                self.handle_message_recv(&buf[..size]).await?;
                return Ok(Some(()));
            },
            request = self.request_stream.try_next() => {
                let request = request.map_err(ClientError::Fidl)?;
                return request.map(|request| self.handle_client_request(request)).transpose();
            }
        };
        self.handle_timeout(timer_type).await?;
        Ok(Some(()))
    }

    #[cfg(test)]
    fn assert_scheduled(
        &self,
        timers: impl IntoIterator<Item = dhcpv6_core::client::ClientTimerType>,
    ) {
        assert_eq!(self.timers.as_ref().scheduled, timers.into_iter().collect())
    }
}

/// Creates a socket listening on the input address and binds it to the
/// interface.
fn create_socket(
    addr: SocketAddr,
    interface_id: fnet::InterfaceId,
) -> std::io::Result<fasync::net::UdpSocket> {
    let socket = socket2::Socket::new(
        socket2::Domain::IPV6,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )?;
    // It is possible to run multiple clients on the same address.
    socket.set_reuse_port(true)?;

    // The client is created for a specific interface; bind to that interface.
    let interface_id = u32::try_from(interface_id).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "interface ID does not fit in u32")
    })?;
    let name = fuchsia_nix::net::if_::if_indextoname(interface_id)
        .map_err(Into::<std::io::Error>::into)?;
    socket.bind_device(Some(name.as_bytes()))?;

    socket.bind(&addr.into())?;
    fasync::net::UdpSocket::from_socket(socket.into())
}

fn duid_from_fidl(duid: Duid) -> Result<dhcpv6_core::ClientDuid, ()> {
    /// According to [RFC 8415, section 11.2], DUID of type DUID-LLT has a type value of 1
    ///
    /// [RFC 8415, section 11.2]: https://datatracker.ietf.org/doc/html/rfc8415#section-11.2
    const DUID_TYPE_LLT: [u8; 2] = [0, 1];
    /// According to [RFC 8415, section 11.4], DUID of type DUID-LL has a type value of 3
    ///
    /// [RFC 8415, section 11.4]: https://datatracker.ietf.org/doc/html/rfc8415#section-11.4
    const DUID_TYPE_LL: [u8; 2] = [0, 3];
    /// According to [RFC 8415, section 11.5], DUID of type DUID-UUID has a type value of 4.
    ///
    /// [RFC 8415, section 11.5]: https://datatracker.ietf.org/doc/html/rfc8415#section-11.5
    const DUID_TYPE_UUID: [u8; 2] = [0, 4];
    /// According to [RFC 8415, section 11.2], the hardware type of Ethernet as assigned by
    /// [IANA] is 1.
    ///
    /// [RFC 8415, section 11.2]: https://datatracker.ietf.org/doc/html/rfc8415#section-11.2
    /// [IANA]: https://www.iana.org/assignments/arp-parameters/arp-parameters.xhtml
    const HARDWARE_TYPE_ETHERNET: [u8; 2] = [0, 1];
    match duid {
        // DUID-LLT with a MAC address is 14 bytes (2 bytes for the type + 2
        // bytes for the hardware type + 4 bytes for the timestamp + 6 bytes
        // for the MAC address), which is guaranteed to fit in the 18-byte limit
        // of `ClientDuid`.
        Duid::LinkLayerAddressPlusTime(LinkLayerAddressPlusTime {
            time,
            link_layer_address: LinkLayerAddress::Ethernet(mac),
        }) => {
            let mut duid = dhcpv6_core::ClientDuid::new();
            duid.try_extend_from_slice(&DUID_TYPE_LLT).unwrap();
            duid.try_extend_from_slice(&HARDWARE_TYPE_ETHERNET).unwrap();
            duid.write_u32::<NetworkEndian>(time).unwrap();
            duid.try_extend_from_slice(&mac.octets).unwrap();
            Ok(duid)
        }
        // DUID-LL with a MAC address is 10 bytes (2 bytes for the type + 2
        // bytes for the hardware type + 6 bytes for the MAC address), which
        // is guaranteed to fit in the 18-byte limit of `ClientDuid`.
        Duid::LinkLayerAddress(LinkLayerAddress::Ethernet(mac)) => Ok(DUID_TYPE_LL
            .into_iter()
            .chain(HARDWARE_TYPE_ETHERNET.into_iter())
            .chain(mac.octets.into_iter())
            .collect()),
        // DUID-UUID is 18 bytes (2 bytes for the type + 16 bytes for the UUID),
        // which is guaranteed to fit in the 18-byte limit of `ClientDuid`.
        Duid::Uuid(uuid) => Ok(DUID_TYPE_UUID.into_iter().chain(uuid.into_iter()).collect()),
        _ => Err(()),
    }
}

/// Starts a client based on `params`.
///
/// `request` will be serviced by the client.
pub(crate) async fn serve_client(
    NewClientParams { interface_id, address, duid, config }: NewClientParams,
    request: ServerEnd<ClientMarker>,
) -> Result<()> {
    let std_addr = Ipv6Addr::from(address.address.addr);
    if std_addr.is_multicast()
        || (std_addr.is_unicast_link_local() && address.zone_index != interface_id)
    {
        return request
            .close_with_epitaph(zx::Status::INVALID_ARGS)
            .context("closing request channel with epitaph");
    }

    let fnet_ext::SocketAddress(addr) = fnet::SocketAddress::Ipv6(address).into();
    let servers_addr = IpAddr::from_str(RELAY_AGENT_AND_SERVER_LINK_LOCAL_MULTICAST_ADDRESS)
        .with_context(|| {
            format!(
                "{} should be a valid IPv6 address",
                RELAY_AGENT_AND_SERVER_LINK_LOCAL_MULTICAST_ADDRESS,
            )
        })?;
    let duid = match duid.map(|fidl| duid_from_fidl(fidl)).transpose() {
        Ok(duid) => duid,
        Err(()) => {
            return request
                .close_with_epitaph(zx::Status::INVALID_ARGS)
                .context("closing request channel with epitaph");
        }
    };
    let (request_stream, control_handle) = request.into_stream_and_control_handle();
    let mut client = match Client::<fasync::net::UdpSocket>::start(
        duid,
        config,
        interface_id,
        || create_socket(addr, interface_id),
        SocketAddr::new(servers_addr, RELAY_AGENT_AND_SERVER_PORT),
        request_stream,
    )
    .await
    {
        Ok(client) => client,
        Err(ClientError::UnsupportedConfigs) => {
            control_handle.shutdown_with_epitaph(zx::Status::INVALID_ARGS);
            return Ok(());
        }
        Err(e) => {
            return Err(e.into());
        }
    };
    let mut buf = vec![0u8; MAX_UDP_DATAGRAM_SIZE];
    loop {
        match client.handle_next_event(&mut buf).await? {
            Some(()) => (),
            None => break Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::pin::pin;
    use std::task::Poll;

    use fidl::endpoints::{
        ClientEnd, create_proxy, create_proxy_and_stream, create_request_stream,
    };
    use fidl_fuchsia_net_dhcpv6::{self as fnet_dhcpv6, ClientProxy, DEFAULT_CLIENT_PORT};
    use fuchsia_async as fasync;
    use futures::{TryFutureExt as _, join, poll};

    use assert_matches::assert_matches;
    use net_declare::{
        fidl_ip_v6, fidl_ip_v6_with_prefix, fidl_mac, fidl_socket_addr, fidl_socket_addr_v6,
        net_ip_v6, net_subnet_v6, std_socket_addr,
    };
    use net_types::ip::IpAddress as _;
    use packet::serialize::InnerPacketBuilder;
    use test_case::test_case;

    use super::*;

    /// Creates a test socket bound to an ephemeral port on localhost.
    fn create_test_socket() -> (fasync::net::UdpSocket, SocketAddr) {
        let addr: SocketAddr = std_socket_addr!("[::1]:0");
        let socket = std::net::UdpSocket::bind(addr).expect("failed to create test socket");
        let addr = socket.local_addr().expect("failed to get address of test socket");
        (fasync::net::UdpSocket::from_socket(socket).expect("failed to create test socket"), addr)
    }

    struct ReceivedMessage {
        transaction_id: [u8; 3],
        // Client IDs are optional in Information Request messages.
        //
        // Per RFC 8415 section 18.2.6,
        //
        //   The client SHOULD include a Client Identifier option (see
        //   Section 21.2) to identify itself to the server (however, see
        //   Section 4.3.1 of [RFC7844] for reasons why a client may not want to
        //   include this option).
        //
        // Per RFC 7844 section 4.3.1,
        //
        //   According to [RFC3315], a DHCPv6 client includes its client
        //   identifier in most of the messages it sends. There is one exception,
        //   however: the client is allowed to omit its client identifier when
        //   sending Information-request messages.
        client_id: Option<Vec<u8>>,
    }

    /// Asserts `socket` receives a message of `msg_type` from
    /// `want_from_addr`.
    async fn assert_received_message(
        socket: &fasync::net::UdpSocket,
        want_from_addr: SocketAddr,
        msg_type: v6::MessageType,
    ) -> ReceivedMessage {
        let mut buf = vec![0u8; MAX_UDP_DATAGRAM_SIZE];
        let (size, from_addr) =
            socket.recv_from(&mut buf).await.expect("failed to receive on test server socket");
        assert_eq!(from_addr, want_from_addr);
        let buf = &mut &buf[..size]; // Implements BufferView.
        let msg = v6::Message::parse(buf, ()).expect("failed to parse message");
        assert_eq!(msg.msg_type(), msg_type);

        let mut client_id = None;
        for opt in msg.options() {
            match opt {
                v6::ParsedDhcpOption::ClientId(id) => {
                    assert_eq!(core::mem::replace(&mut client_id, Some(id.to_vec())), None)
                }
                _ => {}
            }
        }

        ReceivedMessage { transaction_id: *msg.transaction_id(), client_id: client_id }
    }

    const TEST_MAC: fnet::MacAddress = fidl_mac!("00:01:02:03:04:05");

    #[test_case(
        Duid::LinkLayerAddress(LinkLayerAddress::Ethernet(TEST_MAC)),
        &[0, 3, 0, 1, 0, 1, 2, 3, 4, 5];
        "ll"
    )]
    #[test_case(
        Duid::LinkLayerAddressPlusTime(LinkLayerAddressPlusTime {
            time: 0,
            link_layer_address: LinkLayerAddress::Ethernet(TEST_MAC),
        }),
        &[0, 1, 0, 1, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5];
        "llt"
    )]
    #[test_case(
        Duid::Uuid([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]),
        &[0, 4, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        "uuid"
    )]
    #[fuchsia::test]
    fn test_duid_from_fidl(duid: Duid, want: &[u8]) {
        assert_eq!(duid_from_fidl(duid), Ok(dhcpv6_core::ClientDuid::try_from(want).unwrap()));
    }

    #[fuchsia::test]
    fn test_create_client_with_unsupported_config() {
        let prefix_delegation_configs = [
            None,
            // Prefix length config without a non-zero length.
            Some(PrefixDelegationConfig::PrefixLength(0)),
            // Prefix length too long.
            Some(PrefixDelegationConfig::PrefixLength(Ipv6Addr::BYTES * 8 + 1)),
            // Network-bits unset.
            Some(PrefixDelegationConfig::Prefix(fidl_ip_v6_with_prefix!("::/64"))),
            // Host-bits set.
            Some(PrefixDelegationConfig::Prefix(fidl_ip_v6_with_prefix!("a::1/64"))),
        ];

        for prefix_delegation_config in prefix_delegation_configs.iter() {
            assert_matches!(
                create_state_machine(
                    prefix_delegation_config.is_some().then(|| CLIENT_ID.into()),
                    ClientConfig {
                        information_config: Default::default(),
                        non_temporary_address_config: Default::default(),
                        prefix_delegation_config: prefix_delegation_config.clone(),
                    }
                ),
                Err(ClientError::UnsupportedConfigs),
                "prefix_delegation_config={:?}",
                prefix_delegation_config
            );
        }
    }

    const STATELESS_CLIENT_CONFIG: ClientConfig = ClientConfig {
        information_config: InformationConfig { dns_servers: true },
        non_temporary_address_config: AddressConfig { address_count: 0, preferred_addresses: None },
        prefix_delegation_config: None,
    };

    #[fuchsia::test]
    async fn test_client_stops_on_channel_close() {
        let (client_proxy, server_end) = create_proxy::<ClientMarker>();

        let ((), client_res) = join!(
            async { drop(client_proxy) },
            serve_client(
                NewClientParams {
                    interface_id: 1,
                    address: fidl_socket_addr_v6!("[::1]:546"),
                    config: STATELESS_CLIENT_CONFIG,
                    duid: None,
                },
                server_end,
            ),
        );
        client_res.expect("client future should return with Ok");
    }

    fn client_proxy_watch_servers(
        client_proxy: &fnet_dhcpv6::ClientProxy,
    ) -> impl Future<Output = Result<(), fidl::Error>> {
        client_proxy.watch_servers().map_ok(|_: Vec<fidl_fuchsia_net_name::DnsServer_>| ())
    }

    fn client_proxy_watch_address(
        client_proxy: &fnet_dhcpv6::ClientProxy,
    ) -> impl Future<Output = Result<(), fidl::Error>> {
        client_proxy.watch_address().map_ok(
            |_: (
                fnet::Subnet,
                fidl_fuchsia_net_interfaces_admin::AddressParameters,
                fidl::endpoints::ServerEnd<
                    fidl_fuchsia_net_interfaces_admin::AddressStateProviderMarker,
                >,
            )| (),
        )
    }

    fn client_proxy_watch_prefixes(
        client_proxy: &fnet_dhcpv6::ClientProxy,
    ) -> impl Future<Output = Result<(), fidl::Error>> {
        client_proxy.watch_prefixes().map_ok(|_: Vec<fnet_dhcpv6::Prefix>| ())
    }

    #[test_case(client_proxy_watch_servers; "watch_servers")]
    #[test_case(client_proxy_watch_address; "watch_address")]
    #[test_case(client_proxy_watch_prefixes; "watch_prefixes")]
    #[fuchsia::test]
    async fn test_client_should_return_error_on_double_watch<F>(watch: F)
    where
        F: AsyncFn(&fnet_dhcpv6::ClientProxy) -> Result<(), fidl::Error>,
    {
        let (client_proxy, server_end) = create_proxy::<ClientMarker>();

        let (caller1_res, caller2_res, client_res) = join!(
            watch(&client_proxy),
            watch(&client_proxy),
            serve_client(
                NewClientParams {
                    interface_id: 1,
                    address: fidl_socket_addr_v6!("[::1]:546"),
                    config: STATELESS_CLIENT_CONFIG,
                    duid: None,
                },
                server_end,
            )
        );

        assert_matches!(
            caller1_res,
            Err(fidl::Error::ClientChannelClosed { epitaph: fidl::Epitaph::PeerClosed, .. })
        );
        assert_matches!(
            caller2_res,
            Err(fidl::Error::ClientChannelClosed { epitaph: fidl::Epitaph::PeerClosed, .. })
        );
        assert!(
            client_res
                .expect_err("client should fail with double watch error")
                .to_string()
                .contains("got watch request while the previous one is pending")
        );
    }

    const VALID_INFORMATION_CONFIGS: [InformationConfig; 2] =
        [InformationConfig { dns_servers: false }, InformationConfig { dns_servers: true }];

    const VALID_DELEGATED_PREFIX_CONFIGS: [Option<PrefixDelegationConfig>; 4] = [
        Some(PrefixDelegationConfig::Empty(Empty {})),
        Some(PrefixDelegationConfig::PrefixLength(1)),
        Some(PrefixDelegationConfig::PrefixLength(127)),
        Some(PrefixDelegationConfig::Prefix(fidl_ip_v6_with_prefix!("a::/64"))),
    ];

    // Can't be a const variable because we allocate a vector.
    fn get_valid_non_temporary_address_configs() -> [AddressConfig; 5] {
        [
            Default::default(),
            AddressConfig { address_count: 1, preferred_addresses: None },
            AddressConfig { address_count: 1, preferred_addresses: Some(Vec::new()) },
            AddressConfig {
                address_count: 1,
                preferred_addresses: Some(vec![fidl_ip_v6!("a::1")]),
            },
            AddressConfig {
                address_count: 2,
                preferred_addresses: Some(vec![fidl_ip_v6!("a::2")]),
            },
        ]
    }

    #[fuchsia::test]
    fn test_client_starts_with_valid_args() {
        for information_config in VALID_INFORMATION_CONFIGS {
            for non_temporary_address_config in get_valid_non_temporary_address_configs() {
                for prefix_delegation_config in VALID_DELEGATED_PREFIX_CONFIGS {
                    let mut exec = fasync::TestExecutor::new();

                    let (client_proxy, server_end) = create_proxy::<ClientMarker>();

                    let test_fut = async {
                        join!(
                            client_proxy.watch_servers(),
                            serve_client(
                                NewClientParams {
                                    interface_id: 1,
                                    address: fidl_socket_addr_v6!("[::1]:546"),
                                    config: ClientConfig {
                                        information_config: information_config.clone(),
                                        non_temporary_address_config: non_temporary_address_config
                                            .clone(),
                                        prefix_delegation_config: prefix_delegation_config.clone(),
                                    },
                                    duid: (non_temporary_address_config.address_count != 0
                                        || prefix_delegation_config.is_some())
                                    .then(|| fnet_dhcpv6::Duid::LinkLayerAddress(
                                        fnet_dhcpv6::LinkLayerAddress::Ethernet(fidl_mac!(
                                            "00:11:22:33:44:55"
                                        ))
                                    )),
                                },
                                server_end
                            )
                        )
                    };
                    let mut test_fut = pin!(test_fut);
                    assert_matches!(
                        exec.run_until_stalled(&mut test_fut),
                        Poll::Pending,
                        "information_config={:?}, non_temporary_address_config={:?}, prefix_delegation_config={:?}",
                        information_config,
                        non_temporary_address_config,
                        prefix_delegation_config
                    );
                }
            }
        }
    }

    const CLIENT_ID: [u8; 18] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17];

    #[fuchsia::test]
    async fn test_client_starts_in_correct_mode() {
        for information_config @ InformationConfig { dns_servers } in VALID_INFORMATION_CONFIGS {
            for non_temporary_address_config @ AddressConfig {
                address_count,
                preferred_addresses: _,
            } in get_valid_non_temporary_address_configs()
            {
                for prefix_delegation_config in VALID_DELEGATED_PREFIX_CONFIGS {
                    let (stateful, want_msg_type) =
                        if address_count == 0 && prefix_delegation_config.is_none() {
                            if !dns_servers {
                                continue;
                            } else {
                                (false, v6::MessageType::InformationRequest)
                            }
                        } else {
                            (true, v6::MessageType::Solicit)
                        };

                    let (_, client_stream): (ClientEnd<ClientMarker>, _) =
                        create_request_stream::<ClientMarker>();

                    let (client_socket, client_addr) = create_test_socket();
                    let (server_socket, server_addr) = create_test_socket();
                    println!(
                        "{:?} {:?} {:?}",
                        information_config, non_temporary_address_config, prefix_delegation_config
                    );
                    let _: Client<fasync::net::UdpSocket> = Client::start(
                        stateful.then(|| CLIENT_ID.into()),
                        ClientConfig {
                            information_config: information_config.clone(),
                            non_temporary_address_config: non_temporary_address_config.clone(),
                            prefix_delegation_config: prefix_delegation_config.clone(),
                        },
                        1, /* interface ID */
                        || Ok(client_socket),
                        server_addr,
                        client_stream,
                    )
                    .await
                        .unwrap_or_else(|e| panic!(
                            "failed to create test client: {}; information_config={:?}, non_temporary_address_config={:?}, prefix_delegation_config={:?}",
                            e, information_config, non_temporary_address_config, prefix_delegation_config
                        ));

                    let _: ReceivedMessage =
                        assert_received_message(&server_socket, client_addr, want_msg_type).await;
                }
            }
        }
    }

    // TODO(https://fxbug.dev/335656784): Replace this with a netemul test that isn't
    // sensitive to implementation details.
    #[fuchsia::test]
    async fn test_client_fails_to_start_with_invalid_args() {
        for params in vec![
            // Interface ID and zone index mismatch on link-local address.
            NewClientParams {
                interface_id: 2,
                address: fnet::Ipv6SocketAddress {
                    address: fidl_ip_v6!("fe80::1"),
                    port: DEFAULT_CLIENT_PORT,
                    zone_index: 1,
                },
                config: STATELESS_CLIENT_CONFIG,
                duid: None,
            },
            // Multicast address is invalid.
            NewClientParams {
                interface_id: 1,
                address: fnet::Ipv6SocketAddress {
                    address: fidl_ip_v6!("ff01::1"),
                    port: DEFAULT_CLIENT_PORT,
                    zone_index: 1,
                },
                config: STATELESS_CLIENT_CONFIG,
                duid: None,
            },
            // Stateless with DUID.
            NewClientParams {
                interface_id: 1,
                address: fidl_socket_addr_v6!("[2001:db8::1]:12345"),
                config: STATELESS_CLIENT_CONFIG,
                duid: Some(fnet_dhcpv6::Duid::LinkLayerAddress(
                    fnet_dhcpv6::LinkLayerAddress::Ethernet(fidl_mac!("00:11:22:33:44:55")),
                )),
            },
            // Stateful missing DUID.
            NewClientParams {
                interface_id: 1,
                address: fidl_socket_addr_v6!("[2001:db8::1]:12345"),
                config: ClientConfig {
                    information_config: InformationConfig { dns_servers: true },
                    non_temporary_address_config: AddressConfig {
                        address_count: 1,
                        preferred_addresses: None,
                    },
                    prefix_delegation_config: None,
                },
                duid: None,
            },
        ] {
            let (client_proxy, server_end) = create_proxy::<ClientMarker>();
            let () =
                serve_client(params, server_end).await.expect("start server failed unexpectedly");
            // Calling any function on the client proxy should fail due to channel closed with
            // `INVALID_ARGS`.
            assert_matches!(
                client_proxy.watch_servers().await,
                Err(fidl::Error::ClientChannelClosed { epitaph, .. })
                    if epitaph == zx::Status::INVALID_ARGS
            );
        }
    }

    fn create_test_dns_server(
        address: fnet::Ipv6Address,
        source_interface: u64,
        zone_index: u64,
    ) -> fnet_name::DnsServer_ {
        fnet_name::DnsServer_ {
            address: Some(fnet::SocketAddress::Ipv6(fnet::Ipv6SocketAddress {
                address,
                zone_index,
                port: DEFAULT_DNS_PORT,
            })),
            source: Some(fnet_name::DnsServerSource::Dhcpv6(fnet_name::Dhcpv6DnsServerSource {
                source_interface: Some(source_interface),
                ..Default::default()
            })),
            ..Default::default()
        }
    }

    async fn send_msg_with_options(
        socket: &fasync::net::UdpSocket,
        to_addr: SocketAddr,
        transaction_id: [u8; 3],
        msg_type: v6::MessageType,
        options: &[v6::DhcpOption<'_>],
    ) -> Result<()> {
        let builder = v6::MessageBuilder::new(msg_type, transaction_id, options);
        let mut buf = vec![0u8; builder.bytes_len()];
        builder.serialize(&mut buf);
        let size = socket.send_to(&buf, to_addr).await?;
        assert_eq!(size, buf.len());
        Ok(())
    }

    #[fuchsia::test]
    fn test_client_should_respond_to_dns_watch_requests() {
        let mut exec = fasync::TestExecutor::new();

        let (client_proxy, client_stream) = create_proxy_and_stream::<ClientMarker>();

        let (client_socket, client_addr) = create_test_socket();
        let (server_socket, server_addr) = create_test_socket();
        let mut client = exec
            .run_singlethreaded(Client::<fasync::net::UdpSocket>::start(
                None,
                STATELESS_CLIENT_CONFIG,
                1, /* interface ID */
                || Ok(client_socket),
                server_addr,
                client_stream,
            ))
            .expect("failed to create test client");

        let ReceivedMessage { transaction_id: initial_transaction_id, client_id: _ } = exec
            .run_singlethreaded(assert_received_message(
                &server_socket,
                client_addr,
                v6::MessageType::InformationRequest,
            ));

        type WatchServersResponseFut = <fnet_dhcpv6::ClientProxy as fnet_dhcpv6::ClientProxyInterface>::WatchServersResponseFut;
        type WatchServersResponse = <WatchServersResponseFut as Future>::Output;

        struct Test<'a> {
            client: &'a mut Client<fasync::net::UdpSocket>,
            buf: Vec<u8>,
            watcher_fut: WatchServersResponseFut,
        }

        impl<'a> Test<'a> {
            fn new(
                client: &'a mut Client<fasync::net::UdpSocket>,
                client_proxy: &ClientProxy,
            ) -> Self {
                Self {
                    client,
                    buf: vec![0u8; MAX_UDP_DATAGRAM_SIZE],
                    watcher_fut: client_proxy.watch_servers(),
                }
            }

            async fn handle_next_event(&mut self) {
                self.client
                    .handle_next_event(&mut self.buf)
                    .await
                    .expect("test client failed to handle next event")
                    .expect("request stream closed");
            }

            async fn refresh_client(
                &mut self,
                server_socket: &fasync::net::UdpSocket,
                client_addr: SocketAddr,
            ) -> [u8; 3] {
                // Make the client ready for another reply immediately on signal, so it can
                // start receiving updates without waiting for the full refresh timeout which is
                // unrealistic in tests.
                if self
                    .client
                    .timers
                    .as_ref()
                    .scheduled
                    .contains(&dhcpv6_core::client::ClientTimerType::Refresh)
                {
                    self.client
                        .handle_timeout(dhcpv6_core::client::ClientTimerType::Refresh)
                        .await
                        .expect("test client failed to handle timeout");
                    let ReceivedMessage { transaction_id, client_id: _ } = assert_received_message(
                        server_socket,
                        client_addr,
                        v6::MessageType::InformationRequest,
                    )
                    .await;
                    transaction_id
                } else {
                    panic!("no refresh timer is scheduled and refresh is requested in test");
                }
            }

            // Drive both the DHCPv6 client's event handling logic and the DNS server
            // watcher until the DNS server watcher receives an update from the client (or
            // the client unexpectedly exits).
            fn run(&mut self) -> impl Future<Output = WatchServersResponse> + use<'_, 'a> {
                let Self { client, buf, watcher_fut } = self;
                async move {
                    let client_fut = async {
                        loop {
                            client
                                .handle_next_event(buf)
                                .await
                                .expect("test client failed to handle next event")
                                .expect("request stream closed");
                        }
                    }
                    .fuse();
                    let mut client_fut = pin!(client_fut);
                    let mut watcher_fut = watcher_fut.fuse();
                    select! {
                        () = client_fut => panic!("test client returned unexpectedly"),
                        r = watcher_fut => r,
                    }
                }
            }
        }

        {
            // No DNS configurations received yet.
            let mut test = Test::new(&mut client, &client_proxy);

            // Handle the WatchServers request.
            exec.run_singlethreaded(test.handle_next_event());
            assert!(
                test.client.dns_responder.is_some(),
                "WatchServers responder should be present"
            );

            // Send an empty list to the client, should not update watcher.
            exec.run_singlethreaded(send_msg_with_options(
                &server_socket,
                client_addr,
                initial_transaction_id,
                v6::MessageType::Reply,
                &[v6::DhcpOption::ServerId(&[1, 2, 3]), v6::DhcpOption::DnsServers(&[])],
            ))
            .expect("failed to send test reply");
            // Wait for the client to handle the next event (processing the reply we just
            // sent). Note that it is not enough to simply drive the client future until it
            // is stalled as we do elsewhere in the test, because we have no guarantee that
            // the netstack has delivered the UDP packet to the client by the time the
            // `send_to` call returned.
            exec.run_singlethreaded(test.handle_next_event());
            assert_matches!(exec.run_until_stalled(&mut pin!(test.run())), Poll::Pending);

            // Send a list of DNS servers, the watcher should be updated accordingly.
            let transaction_id =
                exec.run_singlethreaded(test.refresh_client(&server_socket, client_addr));
            let dns_servers = [net_ip_v6!("fe80::1:2")];
            exec.run_singlethreaded(send_msg_with_options(
                &server_socket,
                client_addr,
                transaction_id,
                v6::MessageType::Reply,
                &[v6::DhcpOption::ServerId(&[1, 2, 3]), v6::DhcpOption::DnsServers(&dns_servers)],
            ))
            .expect("failed to send test reply");
            let want_servers = vec![create_test_dns_server(
                fidl_ip_v6!("fe80::1:2"),
                1, /* source interface */
                1, /* zone index */
            )];
            let servers = exec.run_singlethreaded(test.run()).expect("get servers");
            assert_eq!(servers, want_servers);
        } // drop `test_fut` so `client_fut` is no longer mutably borrowed.

        {
            // No new changes, should not update watcher.
            let mut test = Test::new(&mut client, &client_proxy);

            // Handle the WatchServers request.
            exec.run_singlethreaded(test.handle_next_event());
            assert!(
                test.client.dns_responder.is_some(),
                "WatchServers responder should be present"
            );

            // Send the same list of DNS servers, should not update watcher.
            let transaction_id =
                exec.run_singlethreaded(test.refresh_client(&server_socket, client_addr));
            let dns_servers = [net_ip_v6!("fe80::1:2")];
            exec.run_singlethreaded(send_msg_with_options(
                &server_socket,
                client_addr,
                transaction_id,
                v6::MessageType::Reply,
                &[v6::DhcpOption::ServerId(&[1, 2, 3]), v6::DhcpOption::DnsServers(&dns_servers)],
            ))
            .expect("failed to send test reply");
            // Wait for the client to handle the next event (processing the reply we just
            // sent). Note that it is not enough to simply drive the client future until it
            // is stalled as we do elsewhere in the test, because we have no guarantee that
            // the netstack has delivered the UDP packet to the client by the time the
            // `send_to` call returned.
            exec.run_singlethreaded(test.handle_next_event());
            assert_matches!(exec.run_until_stalled(&mut pin!(test.run())), Poll::Pending);

            // Send a different list of DNS servers, should update watcher.
            let transaction_id =
                exec.run_singlethreaded(test.refresh_client(&server_socket, client_addr));
            let dns_servers = [net_ip_v6!("fe80::1:2"), net_ip_v6!("1234::5:6")];
            exec.run_singlethreaded(send_msg_with_options(
                &server_socket,
                client_addr,
                transaction_id,
                v6::MessageType::Reply,
                &[v6::DhcpOption::ServerId(&[1, 2, 3]), v6::DhcpOption::DnsServers(&dns_servers)],
            ))
            .expect("failed to send test reply");
            let want_servers = vec![
                create_test_dns_server(
                    fidl_ip_v6!("fe80::1:2"),
                    1, /* source interface */
                    1, /* zone index */
                ),
                // Only set zone index for link local addresses.
                create_test_dns_server(
                    fidl_ip_v6!("1234::5:6"),
                    1, /* source interface */
                    0, /* zone index */
                ),
            ];
            let servers = exec.run_singlethreaded(test.run()).expect("get servers");
            assert_eq!(servers, want_servers);
        } // drop `test_fut` so `client_fut` is no longer mutably borrowed.

        {
            // Send an empty list of DNS servers, should update watcher,
            // because this is different from what the watcher has seen
            // last time.
            let mut test = Test::new(&mut client, &client_proxy);

            let transaction_id =
                exec.run_singlethreaded(test.refresh_client(&server_socket, client_addr));
            exec.run_singlethreaded(send_msg_with_options(
                &server_socket,
                client_addr,
                transaction_id,
                v6::MessageType::Reply,
                &[v6::DhcpOption::ServerId(&[1, 2, 3]), v6::DhcpOption::DnsServers(&[])],
            ))
            .expect("failed to send test reply");
            let want_servers = Vec::<fnet_name::DnsServer_>::new();
            assert_eq!(exec.run_singlethreaded(test.run()).expect("get servers"), want_servers);
        } // drop `test_fut` so `client_fut` is no longer mutably borrowed.
    }

    #[fuchsia::test]
    async fn test_client_should_respond_with_dns_servers_on_first_watch_if_non_empty() {
        let (client_proxy, client_stream) = create_proxy_and_stream::<ClientMarker>();

        let (client_socket, client_addr) = create_test_socket();
        let (server_socket, server_addr) = create_test_socket();
        let client = Client::<fasync::net::UdpSocket>::start(
            None,
            STATELESS_CLIENT_CONFIG,
            1, /* interface ID */
            || Ok(client_socket),
            server_addr,
            client_stream,
        )
        .await
        .expect("failed to create test client");

        let ReceivedMessage { transaction_id: initial_txid, client_id: _ } =
            assert_received_message(
                &server_socket,
                client_addr,
                v6::MessageType::InformationRequest,
            )
            .await;

        let dns_servers = [net_ip_v6!("fe80::1:2"), net_ip_v6!("1234::5:6")];
        send_msg_with_options(
            &server_socket,
            client_addr,
            initial_txid,
            v6::MessageType::Reply,
            &[v6::DhcpOption::ServerId(&[4, 5, 6]), v6::DhcpOption::DnsServers(&dns_servers)],
        )
        .await
        .expect("failed to send test message");

        let buf = vec![0u8; MAX_UDP_DATAGRAM_SIZE];
        let handle_client_events_fut =
            futures::stream::try_unfold((client, buf), |(mut client, mut buf)| async {
                client
                    .handle_next_event(&mut buf)
                    .await
                    .map(|res| res.map(|()| ((), (client, buf))))
            })
            .try_fold((), |(), ()| futures::future::ready(Ok(())))
            .fuse();
        let mut handle_client_events_fut = pin!(handle_client_events_fut);

        let want_servers = vec![
            create_test_dns_server(
                fidl_ip_v6!("fe80::1:2"),
                1, /* source interface */
                1, /* zone index */
            ),
            create_test_dns_server(
                fidl_ip_v6!("1234::5:6"),
                1, /* source interface */
                0, /* zone index */
            ),
        ];
        let found_servers = select!(
            status = handle_client_events_fut => panic!("client unexpectedly exited: {status:?}"),
            found_servers = client_proxy.watch_servers() => found_servers.expect(
                "watch servers should succeed"),
        );
        assert_eq!(found_servers, want_servers);
    }

    #[fuchsia::test]
    async fn watch_prefixes() {
        const SERVER_ID: [u8; 3] = [3, 4, 5];
        const PREFERRED_LIFETIME_SECS: u32 = 1000;
        const VALID_LIFETIME_SECS: u32 = 2000;
        // Use the smallest possible value to enter the Renewing state
        // as fast as possible to keep the test's run-time as low as possible.
        const T1: u32 = 1;
        const T2: u32 = 2000;

        let (client_proxy, client_stream) = create_proxy_and_stream::<ClientMarker>();

        let (client_socket, client_addr) = create_test_socket();
        let (server_socket, server_addr) = create_test_socket();
        let mut client = Client::<fasync::net::UdpSocket>::start(
            Some(CLIENT_ID.into()),
            ClientConfig {
                information_config: Default::default(),
                non_temporary_address_config: Default::default(),
                prefix_delegation_config: Some(PrefixDelegationConfig::Empty(Empty {})),
            },
            1, /* interface ID */
            || Ok(client_socket),
            server_addr,
            client_stream,
        )
        .await
        .expect("failed to create test client");

        let client_fut = async {
            let mut buf = vec![0u8; MAX_UDP_DATAGRAM_SIZE];
            loop {
                select! {
                    res = client.handle_next_event(&mut buf).fuse() => {
                        match res.expect("test client failed to handle next event") {
                            Some(()) => (),
                            None => break (),
                        };
                    }
                }
            }
        }
        .fuse();
        let mut client_fut = pin!(client_fut);

        let update_prefix = net_subnet_v6!("a::/64");
        let remove_prefix = net_subnet_v6!("b::/64");
        let add_prefix = net_subnet_v6!("c::/64");

        // Go through the motions to assign a prefix.
        let client_id = {
            let ReceivedMessage { client_id, transaction_id } =
                assert_received_message(&server_socket, client_addr, v6::MessageType::Solicit)
                    .await;
            // Client IDs are mandatory in stateful DHCPv6.
            let client_id = client_id.unwrap();

            let ia_prefix = [
                v6::DhcpOption::IaPrefix(v6::IaPrefixSerializer::new(
                    PREFERRED_LIFETIME_SECS,
                    VALID_LIFETIME_SECS,
                    update_prefix,
                    &[],
                )),
                v6::DhcpOption::IaPrefix(v6::IaPrefixSerializer::new(
                    PREFERRED_LIFETIME_SECS,
                    VALID_LIFETIME_SECS,
                    remove_prefix,
                    &[],
                )),
            ];
            send_msg_with_options(
                &server_socket,
                client_addr,
                transaction_id,
                v6::MessageType::Advertise,
                &[
                    v6::DhcpOption::ServerId(&SERVER_ID),
                    v6::DhcpOption::ClientId(&client_id),
                    v6::DhcpOption::Preference(u8::MAX),
                    v6::DhcpOption::IaPd(v6::IaPdSerializer::new(IA_PD_IAID, T1, T2, &ia_prefix)),
                ],
            )
            .await
            .expect("failed to send adv message");

            // Wait for the client to send a Request and send Reply so a prefix
            // is assigned.
            let transaction_id = select! {
                () = client_fut => panic!("should never return"),
                res = assert_received_message(
                    &server_socket,
                    client_addr,
                    v6::MessageType::Request,
                ).fuse() => {
                    let ReceivedMessage { client_id: req_client_id, transaction_id } = res;
                    assert_eq!(Some(&client_id), req_client_id.as_ref());
                    transaction_id
                },
            };

            send_msg_with_options(
                &server_socket,
                client_addr,
                transaction_id,
                v6::MessageType::Reply,
                &[
                    v6::DhcpOption::ServerId(&SERVER_ID),
                    v6::DhcpOption::ClientId(&client_id),
                    v6::DhcpOption::IaPd(v6::IaPdSerializer::new(IA_PD_IAID, T1, T2, &ia_prefix)),
                ],
            )
            .await
            .expect("failed to send reply message");

            client_id
        };

        let check_watch_prefixes_result =
            |res: Result<Vec<Prefix>, _>,
             before_handling_reply,
             preferred_lifetime_secs: u32,
             valid_lifetime_secs: u32,
             expected_prefixes| {
                assert_matches!(
                    res.unwrap()[..],
                    [
                        Prefix {
                            prefix: got_prefix1,
                            lifetimes: Lifetimes {
                                preferred_until: preferred_until1,
                                valid_until: valid_until1,
                            },
                        },
                        Prefix {
                            prefix: got_prefix2,
                            lifetimes: Lifetimes {
                                preferred_until: preferred_until2,
                                valid_until: valid_until2,
                            },
                        },
                    ] => {
                        let now = zx::MonotonicInstant::get();
                        let preferred_until = zx::MonotonicInstant::from_nanos(preferred_until1);
                        let valid_until = zx::MonotonicInstant::from_nanos(valid_until1);

                        let preferred_for = zx::MonotonicDuration::from_seconds(
                            preferred_lifetime_secs.into(),
                        );
                        let valid_for = zx::MonotonicDuration::from_seconds(valid_lifetime_secs.into());

                        assert_eq!(
                            HashSet::from([got_prefix1, got_prefix2]),
                            HashSet::from(expected_prefixes),
                        );
                        assert!(preferred_until >= before_handling_reply + preferred_for);
                        assert!(preferred_until <= now + preferred_for);
                        assert!(valid_until >= before_handling_reply + valid_for);
                        assert!(valid_until <= now + valid_for);

                        assert_eq!(preferred_until1, preferred_until2);
                        assert_eq!(valid_until1, valid_until2);
                    }
                )
            };

        // Wait for a prefix to become assigned from the perspective of the DHCPv6
        // FIDL client.
        {
            // watch_prefixes should not return before a lease is negotiated. Note
            // that the client has not yet handled the Reply message.
            let mut watch_prefixes = client_proxy.watch_prefixes().fuse();
            assert_matches!(poll!(&mut watch_prefixes), Poll::Pending);
            let before_handling_reply = zx::MonotonicInstant::get();
            select! {
                () = client_fut => panic!("should never return"),
                res = watch_prefixes => check_watch_prefixes_result(
                    res,
                    before_handling_reply,
                    PREFERRED_LIFETIME_SECS,
                    VALID_LIFETIME_SECS,
                    [
                        subnet_to_address_with_prefix(update_prefix),
                        subnet_to_address_with_prefix(remove_prefix),
                    ],
                ),
            }
        }

        // Wait for the client to attempt to renew the lease and go through the
        // motions to update the lease.
        {
            let transaction_id = select! {
                () = client_fut => panic!("should never return"),
                res = assert_received_message(
                    &server_socket,
                    client_addr,
                    v6::MessageType::Renew,
                ).fuse() => {
                    let ReceivedMessage { client_id: ren_client_id, transaction_id } = res;
                    assert_eq!(ren_client_id.as_ref(), Some(&client_id));
                    transaction_id
                },
            };

            const NEW_PREFERRED_LIFETIME_SECS: u32 = 2 * PREFERRED_LIFETIME_SECS;
            const NEW_VALID_LIFETIME_SECS: u32 = 2 * VALID_LIFETIME_SECS;
            let ia_prefix = [
                v6::DhcpOption::IaPrefix(v6::IaPrefixSerializer::new(
                    NEW_PREFERRED_LIFETIME_SECS,
                    NEW_VALID_LIFETIME_SECS,
                    update_prefix,
                    &[],
                )),
                v6::DhcpOption::IaPrefix(v6::IaPrefixSerializer::new(
                    NEW_PREFERRED_LIFETIME_SECS,
                    NEW_VALID_LIFETIME_SECS,
                    add_prefix,
                    &[],
                )),
                v6::DhcpOption::IaPrefix(v6::IaPrefixSerializer::new(0, 0, remove_prefix, &[])),
            ];

            send_msg_with_options(
                &server_socket,
                client_addr,
                transaction_id,
                v6::MessageType::Reply,
                &[
                    v6::DhcpOption::ServerId(&SERVER_ID),
                    v6::DhcpOption::ClientId(&client_id),
                    v6::DhcpOption::IaPd(v6::IaPdSerializer::new(
                        v6::IAID::new(0),
                        T1,
                        T2,
                        &ia_prefix,
                    )),
                ],
            )
            .await
            .expect("failed to send reply message");

            let before_handling_reply = zx::MonotonicInstant::get();
            select! {
                () = client_fut => panic!("should never return"),
                res = client_proxy.watch_prefixes().fuse() => check_watch_prefixes_result(
                    res,
                    before_handling_reply,
                    NEW_PREFERRED_LIFETIME_SECS,
                    NEW_VALID_LIFETIME_SECS,
                    [
                        subnet_to_address_with_prefix(update_prefix),
                        subnet_to_address_with_prefix(add_prefix),
                    ],
                ),
            }
        }
    }

    #[fuchsia::test]
    async fn test_client_schedule_and_cancel_timers() {
        let (_client_end, client_stream) = create_request_stream::<ClientMarker>();

        let (client_socket, _client_addr) = create_test_socket();
        let (_server_socket, server_addr) = create_test_socket();
        let mut client = Client::<fasync::net::UdpSocket>::start(
            None,
            STATELESS_CLIENT_CONFIG,
            1, /* interface ID */
            || Ok(client_socket),
            server_addr,
            client_stream,
        )
        .await
        .expect("failed to create test client");

        // Stateless DHCP client starts by scheduling a retransmission timer.
        client.assert_scheduled([dhcpv6_core::client::ClientTimerType::Retransmission]);

        client.cancel_timer(dhcpv6_core::client::ClientTimerType::Retransmission);
        client.assert_scheduled([]);

        let now = MonotonicInstant::now();
        client.schedule_timer(
            dhcpv6_core::client::ClientTimerType::Refresh,
            now + Duration::from_nanos(1),
        );
        client.schedule_timer(
            dhcpv6_core::client::ClientTimerType::Retransmission,
            now + Duration::from_nanos(2),
        );
        client.assert_scheduled([
            dhcpv6_core::client::ClientTimerType::Retransmission,
            dhcpv6_core::client::ClientTimerType::Refresh,
        ]);

        // We are allowed to reschedule a timer to fire at a new time.
        let now = MonotonicInstant::now();
        client.schedule_timer(
            dhcpv6_core::client::ClientTimerType::Refresh,
            now + Duration::from_nanos(1),
        );
        client.schedule_timer(
            dhcpv6_core::client::ClientTimerType::Retransmission,
            now + Duration::from_nanos(2),
        );

        client.cancel_timer(dhcpv6_core::client::ClientTimerType::Refresh);
        client.assert_scheduled([dhcpv6_core::client::ClientTimerType::Retransmission]);

        // Ok to cancel a timer that is not scheduled.
        client.cancel_timer(dhcpv6_core::client::ClientTimerType::Refresh);

        client.cancel_timer(dhcpv6_core::client::ClientTimerType::Retransmission);
        client.assert_scheduled([]);

        // Ok to cancel a timer that is not scheduled.
        client.cancel_timer(dhcpv6_core::client::ClientTimerType::Retransmission);
    }

    #[fuchsia::test]
    async fn test_handle_next_event_on_stateless_client() {
        let (client_proxy, client_stream) = create_proxy_and_stream::<ClientMarker>();

        let (client_socket, client_addr) = create_test_socket();
        let (server_socket, server_addr) = create_test_socket();
        let mut client = Client::<fasync::net::UdpSocket>::start(
            None,
            STATELESS_CLIENT_CONFIG,
            1, /* interface ID */
            || Ok(client_socket),
            server_addr,
            client_stream,
        )
        .await
        .expect("failed to create test client");

        // Starting the client in stateless should send an information request out.
        let ReceivedMessage { client_id, transaction_id: initial_txid } = assert_received_message(
            &server_socket,
            client_addr,
            v6::MessageType::InformationRequest,
        )
        .await;
        client.assert_scheduled([dhcpv6_core::client::ClientTimerType::Retransmission]);

        let mut buf = vec![0u8; MAX_UDP_DATAGRAM_SIZE];
        // Trigger a retransmission.
        assert_matches!(client.handle_next_event(&mut buf).await, Ok(Some(())));
        let ReceivedMessage { client_id: got_client_id, transaction_id: _ } =
            assert_received_message(
                &server_socket,
                client_addr,
                v6::MessageType::InformationRequest,
            )
            .await;
        assert_eq!(got_client_id, client_id);
        client.assert_scheduled([dhcpv6_core::client::ClientTimerType::Retransmission]);

        // Message targeting another transaction ID should be ignored.
        send_msg_with_options(&server_socket, client_addr, [5, 6, 7], v6::MessageType::Reply, &[])
            .await
            .expect("failed to send test message");
        assert_matches!(client.handle_next_event(&mut buf).await, Ok(Some(())));
        client.assert_scheduled([dhcpv6_core::client::ClientTimerType::Retransmission]);

        // Invalid messages should be discarded. Empty buffer is invalid.
        let size =
            server_socket.send_to(&[], client_addr).await.expect("failed to send test message");
        assert_eq!(size, 0);
        assert_matches!(client.handle_next_event(&mut buf).await, Ok(Some(())));
        client.assert_scheduled([dhcpv6_core::client::ClientTimerType::Retransmission]);

        // Message targeting this client should cause the client to transition state.
        send_msg_with_options(
            &server_socket,
            client_addr,
            initial_txid,
            v6::MessageType::Reply,
            &[v6::DhcpOption::ServerId(&[4, 5, 6])],
        )
        .await
        .expect("failed to send test message");
        assert_matches!(client.handle_next_event(&mut buf).await, Ok(Some(())));
        client.assert_scheduled([dhcpv6_core::client::ClientTimerType::Refresh]);

        // Reschedule a shorter timer for Refresh so we don't spend time waiting in test.
        client.schedule_timer(
            dhcpv6_core::client::ClientTimerType::Refresh,
            MonotonicInstant::now() + Duration::from_nanos(1),
        );

        // Trigger a refresh.
        assert_matches!(client.handle_next_event(&mut buf).await, Ok(Some(())));
        let ReceivedMessage { client_id, transaction_id: _ } = assert_received_message(
            &server_socket,
            client_addr,
            v6::MessageType::InformationRequest,
        )
        .await;
        assert_eq!(got_client_id, client_id,);
        client.assert_scheduled([dhcpv6_core::client::ClientTimerType::Retransmission]);

        let test_fut = async {
            assert_matches!(client.handle_next_event(&mut buf).await, Ok(Some(())));
            client
                .dns_responder
                .take()
                .expect("test client did not get a channel responder")
                .send(&[fnet_name::DnsServer_ {
                    address: Some(fidl_socket_addr!("[fe01::2:3]:42")),
                    source: Some(fnet_name::DnsServerSource::Dhcpv6(
                        fnet_name::Dhcpv6DnsServerSource {
                            source_interface: Some(42),
                            ..Default::default()
                        },
                    )),
                    ..Default::default()
                }])
                .expect("failed to send response on test channel");
        };
        let (watcher_res, ()) = join!(client_proxy.watch_servers(), test_fut);
        let servers = watcher_res.expect("failed to watch servers");
        assert_eq!(
            servers,
            vec![fnet_name::DnsServer_ {
                address: Some(fidl_socket_addr!("[fe01::2:3]:42")),
                source: Some(fnet_name::DnsServerSource::Dhcpv6(
                    fnet_name::Dhcpv6DnsServerSource {
                        source_interface: Some(42),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            }]
        );

        // Drop the channel should cause `handle_next_event(&mut buf)` to return `None`.
        drop(client_proxy);
        assert_matches!(client.handle_next_event(&mut buf).await, Ok(None));
    }

    #[fuchsia::test]
    async fn test_handle_next_event_on_stateful_client() {
        let (client_proxy, client_stream) = create_proxy_and_stream::<ClientMarker>();

        let (client_socket, client_addr) = create_test_socket();
        let (server_socket, server_addr) = create_test_socket();
        let mut client = Client::<fasync::net::UdpSocket>::start(
            Some(CLIENT_ID.into()),
            ClientConfig {
                information_config: Default::default(),
                non_temporary_address_config: AddressConfig {
                    address_count: 1,
                    preferred_addresses: None,
                },
                prefix_delegation_config: None,
            },
            1, /* interface ID */
            || Ok(client_socket),
            server_addr,
            client_stream,
        )
        .await
        .expect("failed to create test client");

        // Starting the client in stateful should send out a solicit.
        let _: ReceivedMessage =
            assert_received_message(&server_socket, client_addr, v6::MessageType::Solicit).await;
        client.assert_scheduled([dhcpv6_core::client::ClientTimerType::Retransmission]);

        let mut buf = vec![0u8; MAX_UDP_DATAGRAM_SIZE];
        // Drop the channel should cause `handle_next_event(&mut buf)` to return `None`.
        drop(client_proxy);
        assert_matches!(client.handle_next_event(&mut buf).await, Ok(None));
    }

    #[fuchsia::test]
    #[should_panic = "received unexpected refresh timeout in state InformationRequesting"]
    async fn test_handle_next_event_respects_timer_order() {
        let (_client_end, client_stream) = create_request_stream::<ClientMarker>();

        let (client_socket, client_addr) = create_test_socket();
        let (server_socket, server_addr) = create_test_socket();
        let mut client = Client::<fasync::net::UdpSocket>::start(
            None,
            STATELESS_CLIENT_CONFIG,
            1, /* interface ID */
            || Ok(client_socket),
            server_addr,
            client_stream,
        )
        .await
        .expect("failed to create test client");

        let mut buf = vec![0u8; MAX_UDP_DATAGRAM_SIZE];
        // A retransmission timer is scheduled when starting the client in stateless mode. Cancel
        // it and create a new one with a longer timeout so the test is not flaky.
        client.schedule_timer(
            dhcpv6_core::client::ClientTimerType::Retransmission,
            MonotonicInstant::now() + Duration::from_secs(1_000_000),
        );
        client.assert_scheduled([dhcpv6_core::client::ClientTimerType::Retransmission]);

        // Trigger a message receive, the message is later discarded because transaction ID doesn't
        // match.
        send_msg_with_options(&server_socket, client_addr, [5, 6, 7], v6::MessageType::Reply, &[])
            .await
            .expect("failed to send test message");
        // There are now two pending events, the message receive is handled first because the timer
        // is far into the future.
        assert_matches!(client.handle_next_event(&mut buf).await, Ok(Some(())));
        // The retransmission timer is still here.
        client.assert_scheduled([dhcpv6_core::client::ClientTimerType::Retransmission]);

        // Inserts a refresh timer that precedes the retransmission.
        client.schedule_timer(
            dhcpv6_core::client::ClientTimerType::Refresh,
            MonotonicInstant::now() + Duration::from_nanos(1),
        );
        // This timer is scheduled.
        client.assert_scheduled([
            dhcpv6_core::client::ClientTimerType::Retransmission,
            dhcpv6_core::client::ClientTimerType::Refresh,
        ]);

        // Now handle_next_event(&mut buf) should trigger a refresh because it
        // precedes retransmission. Refresh is not expected while in
        // InformationRequesting state and should lead to a panic.
        let unreachable = client.handle_next_event(&mut buf).await;
        panic!("{unreachable:?}");
    }

    #[fuchsia::test]
    async fn test_handle_next_event_fails_on_recv_err() {
        struct StubSocket {}
        impl<'a> AsyncSocket<'a> for StubSocket {
            type RecvFromFut = futures::future::Ready<Result<(usize, SocketAddr), std::io::Error>>;
            type SendToFut = futures::future::Ready<Result<usize, std::io::Error>>;

            fn recv_from(&'a self, _buf: &'a mut [u8]) -> Self::RecvFromFut {
                futures::future::ready(Err(std::io::Error::other("test recv error")))
            }
            fn send_to(&'a self, buf: &'a [u8], _addr: SocketAddr) -> Self::SendToFut {
                futures::future::ready(Ok(buf.len()))
            }
        }

        let (_client_end, client_stream) = create_request_stream::<ClientMarker>();

        let mut client = Client::<StubSocket>::start(
            None,
            STATELESS_CLIENT_CONFIG,
            1, /* interface ID */
            || Ok(StubSocket {}),
            std_socket_addr!("[::1]:0"),
            client_stream,
        )
        .await
        .expect("failed to create test client");

        assert_matches!(
            client.handle_next_event(&mut [0u8]).await,
            Err(ClientError::SocketRecv(err)) if err.kind() == std::io::ErrorKind::Other
        );
    }
}
