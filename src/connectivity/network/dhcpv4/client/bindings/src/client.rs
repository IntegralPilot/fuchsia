// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::cell::RefCell;
use std::num::NonZeroU64;
use std::sync::Arc;

use dhcp_client_core::inspect::Counters;
use diagnostics_traits::Inspector;
use fidl::endpoints;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_dhcp::{
    self as fdhcp, ClientExitReason, ClientRequestStream, ClientWatchConfigurationResponse,
    ConfigurationToRequest, NewClientParams,
};
use fidl_fuchsia_net_ext::IntoExt as _;
use fidl_fuchsia_net_interfaces as fnet_interfaces;
use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fuchsia_async as fasync;
use futures::channel::mpsc;
use futures::{StreamExt, TryStreamExt as _};
use indexmap::IndexSet;
use net_types::ip::{Ipv4, Ipv4Addr, PrefixLength};
use net_types::{SpecifiedAddr, Witness as _};
use rand::SeedableRng as _;

use crate::inspect::{Inspect, LeaseChangeInspect, LeaseInspectProperties, StateInspect};

#[derive(thiserror::Error, Debug)]
pub(crate) enum Error {
    #[error("DHCP client exiting: {0:?}")]
    Exit(ClientExitReason),

    #[error("error observed by DHCP client core: {0:?}")]
    Core(dhcp_client_core::client::Error),

    #[error("fidl error: {0}")]
    Fidl(fidl::Error),
}

impl Error {
    fn from_core(core_error: dhcp_client_core::client::Error) -> Self {
        match core_error {
            dhcp_client_core::client::Error::Socket(socket_error) => match socket_error {
                dhcp_client_core::deps::SocketError::NoInterface
                | dhcp_client_core::deps::SocketError::UnsupportedHardwareType => {
                    Self::Exit(ClientExitReason::InvalidInterface)
                }
                dhcp_client_core::deps::SocketError::FailedToOpen(e) => {
                    log::error!("error while trying to open socket: {:?}", e);
                    Self::Exit(ClientExitReason::UnableToOpenSocket)
                }
                dhcp_client_core::deps::SocketError::HostUnreachable
                | dhcp_client_core::deps::SocketError::AddrNotAvailable
                | dhcp_client_core::deps::SocketError::BrokenPipe
                | dhcp_client_core::deps::SocketError::ConnectionAborted
                | dhcp_client_core::deps::SocketError::Other(_) => {
                    Self::Core(dhcp_client_core::client::Error::Socket(socket_error))
                }
                dhcp_client_core::deps::SocketError::NetworkUnreachable => {
                    Self::Exit(ClientExitReason::NetworkUnreachable)
                }
            },
            dhcp_client_core::client::Error::AddressEventReceiverEnded => {
                Self::Exit(ClientExitReason::AddressStateProviderError)
            }
        }
    }
}

pub(crate) async fn serve_client(
    mac: net_types::ethernet::Mac,
    interface_id: NonZeroU64,
    provider: &crate::packetsocket::PacketSocketProviderImpl,
    udp_socket_provider: &impl dhcp_client_core::deps::UdpSocketProvider,
    params: NewClientParams,
    requests: ClientRequestStream,
    inspect_root: &fuchsia_inspect::Node,
) -> Result<(), Error> {
    let (stop_sender, stop_receiver) = mpsc::unbounded();
    let stop_sender = &stop_sender;
    let debug_log_prefix = dhcp_client_core::client::DebugLogPrefix { interface_id };
    let inspect = Arc::new(Inspect::new());
    let client = RefCell::new(Client::new(
        mac,
        interface_id,
        params,
        rand::rngs::StdRng::seed_from_u64(rand::random()),
        stop_receiver,
        debug_log_prefix,
    )?);
    let counters = Arc::new(Counters::default());
    let _node = inspect_root.create_lazy_child(interface_id.get().to_string(), {
        let counters = counters.clone();
        let inspect = inspect.clone();
        move || {
            let inspector = fuchsia_inspect::Inspector::default();
            {
                let mut inspector =
                    diagnostics_traits::FuchsiaInspector::<'_, ()>::new(inspector.root());
                inspector.record_uint("InterfaceId", interface_id.get());
                inspect.record(&mut inspector);
                inspector.record_child("Counters", |inspector| {
                    counters.record(inspector);
                });
            }
            Box::pin(futures::future::ready(Ok(inspector)))
        }
    });

    let counters = counters.as_ref();
    let inspect = inspect.as_ref();
    requests
        .map_err(Error::Fidl)
        .try_for_each_concurrent(None, |request| {
            let client = &client;
            async move {
                match request {
                    fidl_fuchsia_net_dhcp::ClientRequest::WatchConfiguration { responder } => {
                        let mut client = client.try_borrow_mut().map_err(|_| {
                            Error::Exit(ClientExitReason::WatchConfigurationAlreadyPending)
                        })?;
                        responder
                            .send(
                                client
                                    .watch_configuration(
                                        provider,
                                        udp_socket_provider,
                                        counters,
                                        inspect,
                                    )
                                    .await?,
                            )
                            .map_err(Error::Fidl)?;
                        Ok(())
                    }
                    fidl_fuchsia_net_dhcp::ClientRequest::Shutdown { control_handle: _ } => {
                        match stop_sender.unbounded_send(()) {
                            Ok(()) => stop_sender.close_channel(),
                            Err(try_send_error) => {
                                // Note that `try_send_error` cannot be exhaustively matched on.
                                if try_send_error.is_disconnected() {
                                    log::warn!(
                                        "{debug_log_prefix} tried to send shutdown request on \
                                        already-closed channel to client core"
                                    );
                                } else {
                                    log::error!(
                                        "{debug_log_prefix} error while sending shutdown request \
                                        to client core: {:?}",
                                        try_send_error
                                    );
                                }
                            }
                        }
                        Ok(())
                    }
                }
            }
        })
        .await
}

struct Clock;

impl dhcp_client_core::deps::Clock for Clock {
    type Instant = fasync::MonotonicInstant;

    fn now(&self) -> Self::Instant {
        fasync::MonotonicInstant::now()
    }

    async fn wait_until(&self, time: Self::Instant) {
        fasync::Timer::new(time).await
    }
}

/// Encapsulates all DHCP client state.
struct Client {
    config: dhcp_client_core::client::ClientConfig,
    core: dhcp_client_core::client::State<fasync::MonotonicInstant>,
    rng: rand::rngs::StdRng,
    stop_receiver: mpsc::UnboundedReceiver<()>,
    current_lease: Option<Lease>,
    interface_id: NonZeroU64,
}

struct Lease {
    address_state_provider: fnet_interfaces_admin::AddressStateProviderProxy,
    // The stream of address_assignment state changes for the address.
    // This stream is stateful and intertwined with the above
    // address_state_provider proxy. Care must be taken not to call either
    // `take_event_stream()` or `watch_address_assignment_state()` directly on
    // the proxy.
    assignment_state_stream: futures::stream::BoxStream<
        'static,
        Result<
            fnet_interfaces::AddressAssignmentState,
            fnet_interfaces_ext::admin::AddressStateProviderError,
        >,
    >,
    ip_address: SpecifiedAddr<net_types::ip::Ipv4Addr>,
}

impl Client {
    fn new(
        mac: net_types::ethernet::Mac,
        interface_id: NonZeroU64,
        NewClientParams { configuration_to_request, request_ip_address, .. }: NewClientParams,
        rng: rand::rngs::StdRng,
        stop_receiver: mpsc::UnboundedReceiver<()>,
        debug_log_prefix: dhcp_client_core::client::DebugLogPrefix,
    ) -> Result<Self, Error> {
        if !request_ip_address.unwrap_or(false) {
            log::error!(
                "{debug_log_prefix} client creation failed: \
                DHCPINFORM is unimplemented"
            );
            return Err(Error::Exit(ClientExitReason::InvalidParams));
        }
        let ConfigurationToRequest { routers, dns_servers, .. } =
            configuration_to_request.unwrap_or_else(ConfigurationToRequest::default);

        let config = dhcp_client_core::client::ClientConfig {
            client_hardware_address: mac,
            client_identifier: None,
            requested_parameters: std::iter::once((
                dhcp_protocol::OptionCode::SubnetMask,
                dhcp_client_core::parse::OptionRequested::Required,
            ))
            .chain(routers.unwrap_or(false).then_some((
                dhcp_protocol::OptionCode::Router,
                dhcp_client_core::parse::OptionRequested::Optional,
            )))
            .chain(dns_servers.unwrap_or(false).then_some((
                dhcp_protocol::OptionCode::DomainNameServer,
                dhcp_client_core::parse::OptionRequested::Optional,
            )))
            .collect::<dhcp_client_core::parse::OptionCodeMap<_>>(),
            preferred_lease_time_secs: None,
            requested_ip_address: None,
            debug_log_prefix,
        };
        Ok(Self {
            core: dhcp_client_core::client::State::default(),
            rng,
            config,
            stop_receiver,
            current_lease: None,
            interface_id,
        })
    }

    async fn handle_newly_acquired_lease(
        &mut self,
        dhcp_client_core::client::NewlyAcquiredLease {
            ip_address,
            start_time,
            lease_time,
            parameters,
        }: dhcp_client_core::client::NewlyAcquiredLease<fasync::MonotonicInstant>,
    ) -> Result<(ClientWatchConfigurationResponse, LeaseChangeInspect), Error> {
        let Self {
            core: _,
            rng: _,
            config: dhcp_client_core::client::ClientConfig { debug_log_prefix, .. },
            stop_receiver: _,
            current_lease,
            interface_id: _,
        } = self;

        // Store the DNS Servers & Routers in an `IndexSet` to preserve
        // insertion order & guarantee uniqueness.
        let mut dns_servers: Option<IndexSet<_>> = None;
        let mut routers: Option<IndexSet<_>> = None;
        let mut prefix_len: Option<PrefixLength<Ipv4>> = None;
        let mut unrequested_options = Vec::new();

        for option in parameters {
            match option {
                dhcp_protocol::DhcpOption::SubnetMask(len) => {
                    let previous_prefix_len = prefix_len.replace(len);
                    if let Some(prev) = previous_prefix_len {
                        log::warn!("expected previous_prefix_len to be None, got {prev:?}");
                    }
                }
                dhcp_protocol::DhcpOption::DomainNameServer(list) => {
                    // Note: The DomainNameServer option may occur multiple
                    // times. Merge the options into a single list.
                    dns_servers.get_or_insert_default().extend(list);
                }
                dhcp_protocol::DhcpOption::Router(list) => {
                    // Note: The Router option may occur multiple times. Merge
                    // the options into a single list.
                    routers.get_or_insert_default().extend(list);
                }
                _ => {
                    unrequested_options.push(option);
                }
            }
        }

        if !unrequested_options.is_empty() {
            log::warn!(
                "{debug_log_prefix} Received options from core that we didn't ask for: {:#?}",
                unrequested_options
            );
        }

        let prefix_len = prefix_len
            .expect(
                "subnet mask should be present \
                because it was specified to core as required",
            )
            .get();

        let (asp_proxy, asp_server_end) =
            endpoints::create_proxy::<fnet_interfaces_admin::AddressStateProviderMarker>();

        let previous_lease = current_lease.replace(Lease {
            address_state_provider: asp_proxy.clone(),
            assignment_state_stream: fnet_interfaces_ext::admin::assignment_state_stream(asp_proxy)
                .boxed(),
            ip_address,
        });

        if let Some(previous_lease) = previous_lease {
            self.remove_address_for_lease(previous_lease).await?;
        }

        let lease_inspect_properties = LeaseInspectProperties {
            ip_address,
            lease_length: lease_time.into(),
            dns_servers: dns_servers
                .as_ref()
                .map(|list| list.iter().cloned().map(Into::into).collect()),
            routers: routers.as_ref().map(|list| list.iter().cloned().map(Into::into).collect()),
        };

        Ok((
            ClientWatchConfigurationResponse {
                address: Some(fdhcp::Address {
                    address: Some(fnet::Ipv4AddressWithPrefix {
                        addr: ip_address.get().into_ext(),
                        prefix_len,
                    }),
                    address_parameters: Some(fnet_interfaces_admin::AddressParameters {
                        initial_properties: Some(fnet_interfaces_admin::AddressProperties {
                            preferred_lifetime_info: None,
                            valid_lifetime_end: Some(
                                zx::MonotonicInstant::from(start_time + lease_time.into())
                                    .into_nanos(),
                            ),
                            ..Default::default()
                        }),
                        add_subnet_route: Some(true),
                        perform_dad: Some(true),
                        ..Default::default()
                    }),
                    address_state_provider: Some(asp_server_end),
                    ..Default::default()
                }),
                dns_servers: dns_servers.map(into_fidl_list),
                routers: routers.map(into_fidl_list),
                ..Default::default()
            },
            LeaseChangeInspect::LeaseAdded {
                start_time,
                prefix_len,
                properties: lease_inspect_properties,
            },
        ))
    }

    async fn handle_lease_renewal(
        &mut self,
        dhcp_client_core::client::LeaseRenewal {
            start_time,
            lease_time,
            parameters,
        }: dhcp_client_core::client::LeaseRenewal<fasync::MonotonicInstant>,
    ) -> Result<(ClientWatchConfigurationResponse, LeaseChangeInspect), Error> {
        let Self {
            core: _,
            rng: _,
            config: dhcp_client_core::client::ClientConfig { debug_log_prefix, .. },
            stop_receiver: _,
            current_lease,
            interface_id: _,
        } = self;

        // Store the DNS Servers & Routers in an `IndexSet` to preserve
        // insertion order & guarantee uniqueness.
        let mut dns_servers: Option<IndexSet<_>> = None;
        let mut routers: Option<IndexSet<_>> = None;
        let mut unrequested_options = Vec::new();

        for option in parameters {
            match option {
                dhcp_protocol::DhcpOption::SubnetMask(len) => {
                    log::info!(
                        "{debug_log_prefix} ignoring prefix length={:?} for renewed lease",
                        len
                    );
                }
                dhcp_protocol::DhcpOption::DomainNameServer(list) => {
                    // Note: The DomainNameServer option may occur multiple
                    // times. Merge the options into a single list.
                    dns_servers.get_or_insert_default().extend(list);
                }
                dhcp_protocol::DhcpOption::Router(list) => {
                    // Note: The Router option may occur multiple times. Merge
                    // the options into a single list.
                    routers.get_or_insert_default().extend(list);
                }
                option => {
                    unrequested_options.push(option);
                }
            }
        }

        if !unrequested_options.is_empty() {
            log::warn!(
                "{debug_log_prefix} Received options from core that we didn't ask for: {:#?}",
                unrequested_options
            );
        }

        let Lease { address_state_provider, assignment_state_stream: _, ip_address } =
            current_lease.as_mut().expect("should have current lease if we're handling a renewal");

        address_state_provider
            .update_address_properties(&fnet_interfaces_admin::AddressProperties {
                preferred_lifetime_info: None,
                valid_lifetime_end: Some(
                    zx::MonotonicInstant::from(start_time + lease_time.into()).into_nanos(),
                ),
                ..Default::default()
            })
            .await
            .map_err(Error::Fidl)?;

        let lease_inspect_properties = LeaseInspectProperties {
            ip_address: *ip_address,
            lease_length: lease_time.into(),
            dns_servers: dns_servers
                .as_ref()
                .map(|list| list.iter().cloned().map(Into::into).collect()),
            routers: routers.as_ref().map(|list| list.iter().cloned().map(Into::into).collect()),
        };

        Ok((
            ClientWatchConfigurationResponse {
                address: None,
                dns_servers: dns_servers.map(into_fidl_list),
                routers: routers.map(into_fidl_list),
                ..Default::default()
            },
            LeaseChangeInspect::LeaseRenewed {
                renewed_time: start_time,
                properties: lease_inspect_properties,
            },
        ))
    }

    async fn remove_address_for_lease(&mut self, lease: Lease) -> Result<(), Error> {
        let Lease { address_state_provider, assignment_state_stream, ip_address } = lease;
        address_state_provider.remove().map_err(Error::Fidl)?;
        // Wait to observe an error on the assignment_state_stream.
        let watch_result = assignment_state_stream
            .filter_map(|result| futures::future::ready(result.err()))
            .next()
            .await;
        let debug_log_prefix = &self.config.debug_log_prefix;

        match watch_result {
            None => log::error!(
                "{debug_log_prefix} assignment_state_stream unexpectedly ended \
                while watching for AddressRemovalReason after explicitly \
                removing address {ip_address}",
            ),
            Some(fnet_interfaces_ext::admin::AddressStateProviderError::ChannelClosed) => {
                log::error!(
                    "{debug_log_prefix} channel closed while watching for \
                    AddressRemovalReason after explicitly removing address {ip_address}",
                )
            }
            Some(fnet_interfaces_ext::admin::AddressStateProviderError::Fidl(e)) => log::error!(
                "{debug_log_prefix} error watching for \
                AddressRemovalReason after explicitly removing address {ip_address}: {e:?}",
            ),
            Some(fnet_interfaces_ext::admin::AddressStateProviderError::AddressRemoved(reason)) => {
                match reason {
                    fnet_interfaces_admin::AddressRemovalReason::UserRemoved => (),
                    reason @ (fnet_interfaces_admin::AddressRemovalReason::Invalid
                    | fnet_interfaces_admin::AddressRemovalReason::InvalidProperties
                    | fnet_interfaces_admin::AddressRemovalReason::AlreadyAssigned
                    | fnet_interfaces_admin::AddressRemovalReason::DadFailed
                    | fnet_interfaces_admin::AddressRemovalReason::Forfeited
                    | fnet_interfaces_admin::AddressRemovalReason::InterfaceRemoved) => {
                        log::error!(
                            "{debug_log_prefix} unexpected removal reason \
                            after explicitly removing address {ip_address}: {reason:?}",
                        );
                    }
                }
            }
        };
        Ok(())
    }

    async fn watch_configuration(
        &mut self,
        packet_socket_provider: &crate::packetsocket::PacketSocketProviderImpl,
        udp_socket_provider: &impl dhcp_client_core::deps::UdpSocketProvider,
        counters: &Counters,
        inspect: &Inspect,
    ) -> Result<ClientWatchConfigurationResponse, Error> {
        loop {
            let step = self
                .watch_configuration_step(packet_socket_provider, udp_socket_provider, counters)
                .await?;
            let HandledWatchConfigurationStep { state_inspect, lease_inspect, response_to_return } =
                self.handle_watch_configuration_step(step, packet_socket_provider).await?;

            // watch_configuration_step only resolves once there's been a state
            // transition, so we should always update the state inspect and its
            // history.
            inspect.update(state_inspect, lease_inspect, self.config.debug_log_prefix);
            if let Some(response) = response_to_return {
                return Ok(response);
            }
        }
    }

    async fn handle_watch_configuration_step(
        &mut self,
        step: dhcp_client_core::client::Step<fasync::MonotonicInstant, ClientExitReason>,
        _packet_socket_provider: &crate::packetsocket::PacketSocketProviderImpl,
    ) -> Result<HandledWatchConfigurationStep, Error> {
        let Self { core, rng: _, config, stop_receiver: _, current_lease: _, interface_id: _ } =
            self;
        match step {
            dhcp_client_core::client::Step::NextState(transition) => {
                let (next_core, effect) = core.apply(config, transition);
                *core = next_core;
                match effect {
                    Some(dhcp_client_core::client::TransitionEffect::DropLease {
                        address_rejected,
                    }) => {
                        let current_lease =
                            self.current_lease.take().expect("should have current lease");
                        // Skip manual removal of the address, if it was
                        // rejected (and hence already removed).
                        if !address_rejected {
                            self.remove_address_for_lease(current_lease).await?;
                        }
                        Ok(HandledWatchConfigurationStep {
                            state_inspect: StateInspect {
                                state: next_core,
                                time: fasync::MonotonicInstant::now(),
                            },
                            lease_inspect: LeaseChangeInspect::LeaseDropped,
                            response_to_return: None,
                        })
                    }
                    Some(dhcp_client_core::client::TransitionEffect::HandleNewLease(
                        newly_acquired_lease,
                    )) => {
                        let (response, lease_inspect) =
                            self.handle_newly_acquired_lease(newly_acquired_lease).await?;
                        let start_time = fasync::MonotonicInstant::now();
                        Ok(HandledWatchConfigurationStep {
                            state_inspect: StateInspect { state: next_core, time: start_time },
                            lease_inspect,
                            response_to_return: Some(response),
                        })
                    }
                    Some(dhcp_client_core::client::TransitionEffect::HandleRenewedLease(
                        lease_renewal,
                    )) => {
                        let (response, lease_inspect) =
                            self.handle_lease_renewal(lease_renewal).await?;
                        Ok(HandledWatchConfigurationStep {
                            state_inspect: StateInspect {
                                state: next_core,
                                time: fasync::MonotonicInstant::now(),
                            },
                            lease_inspect,
                            response_to_return: Some(response),
                        })
                    }
                    None => Ok(HandledWatchConfigurationStep {
                        state_inspect: StateInspect {
                            state: *core,
                            time: fasync::MonotonicInstant::now(),
                        },
                        lease_inspect: LeaseChangeInspect::NoChange,
                        response_to_return: None,
                    }),
                }
            }
            dhcp_client_core::client::Step::Exit(reason) => match reason {
                dhcp_client_core::client::ExitReason::GracefulShutdown => {
                    if let Some(current_lease) = self.current_lease.take() {
                        // TODO(https://fxbug.dev/42079439): Send DHCPRELEASE.
                        self.remove_address_for_lease(current_lease).await?;
                    }
                    return Err(Error::Exit(ClientExitReason::GracefulShutdown));
                }
                dhcp_client_core::client::ExitReason::AddressRemoved(reason) => {
                    return Err(Error::Exit(reason));
                }
            },
        }
    }

    async fn watch_configuration_step(
        &mut self,
        packet_socket_provider: &crate::packetsocket::PacketSocketProviderImpl,
        udp_socket_provider: &impl dhcp_client_core::deps::UdpSocketProvider,
        counters: &Counters,
    ) -> Result<dhcp_client_core::client::Step<fasync::MonotonicInstant, ClientExitReason>, Error>
    {
        let Self { core, rng, config, stop_receiver, current_lease, interface_id } = self;
        let clock = Clock;

        let mut address_event_stream = match current_lease {
            None => futures::stream::pending().left_stream(),
            Some(Lease { address_state_provider: _, assignment_state_stream, ip_address }) => {
                assignment_state_stream
                    .map(|event| into_address_event(event, config, ip_address, interface_id))
                    .right_stream()
            }
        }
        .fuse();

        core.run(
            config,
            packet_socket_provider,
            udp_socket_provider,
            rng,
            &clock,
            stop_receiver,
            &mut address_event_stream,
            counters,
        )
        .await
        .map_err(Error::from_core)
    }
}

/// Convert an event on the AddressStateProvider assignment_state_stream
/// into a [`dhcp_client_core::client::AddressEvent`].
fn into_address_event(
    event: Result<
        fnet_interfaces::AddressAssignmentState,
        fnet_interfaces_ext::admin::AddressStateProviderError,
    >,
    config: &dhcp_client_core::client::ClientConfig,
    ip_address: &SpecifiedAddr<Ipv4Addr>,
    interface_id: &NonZeroU64,
) -> dhcp_client_core::client::AddressEvent<ClientExitReason> {
    let debug_log_prefix = &config.debug_log_prefix;
    match event {
        Ok(state) => {
            let new_state = match state {
                fnet_interfaces::AddressAssignmentState::Assigned => {
                    dhcp_client_core::client::AddressAssignmentState::Assigned
                }
                fnet_interfaces::AddressAssignmentState::Tentative => {
                    dhcp_client_core::client::AddressAssignmentState::Tentative
                }
                fnet_interfaces::AddressAssignmentState::Unavailable => {
                    dhcp_client_core::client::AddressAssignmentState::Unavailable
                }
            };
            dhcp_client_core::client::AddressEvent::AssignmentStateChanged(new_state)
        }
        Err(fnet_interfaces_ext::admin::AddressStateProviderError::AddressRemoved(reason)) => {
            match reason {
                fnet_interfaces_admin::AddressRemovalReason::InterfaceRemoved => {
                    log::warn!("{debug_log_prefix} interface removed");
                    dhcp_client_core::client::AddressEvent::Removed(
                        ClientExitReason::InvalidInterface,
                    )
                }
                fnet_interfaces_admin::AddressRemovalReason::UserRemoved => {
                    log::warn!("{debug_log_prefix} address administratively removed");
                    dhcp_client_core::client::AddressEvent::Removed(
                        ClientExitReason::AddressRemovedByUser,
                    )
                }
                r @ fnet_interfaces_admin::AddressRemovalReason::Invalid
                | r @ fnet_interfaces_admin::AddressRemovalReason::InvalidProperties
                | r @ fnet_interfaces_admin::AddressRemovalReason::AlreadyAssigned
                | r @ fnet_interfaces_admin::AddressRemovalReason::DadFailed
                | r @ fnet_interfaces_admin::AddressRemovalReason::Forfeited => {
                    log::warn!("{debug_log_prefix} address rejected: {r:?}");
                    dhcp_client_core::client::AddressEvent::Rejected
                }
            }
        }
        Err(fnet_interfaces_ext::admin::AddressStateProviderError::Fidl(e)) => {
            log::error!(
                "{debug_log_prefix} observed error {e:?} while watching for \
                address event for address {ip_address} on interface {interface_id}; \
                removing address",
            );
            // Note: treat AddressStateProvider FIDL errors the same as address
            // removal. This ultimately leads to the client exiting.
            dhcp_client_core::client::AddressEvent::Removed(
                ClientExitReason::AddressStateProviderError,
            )
        }
        Err(fnet_interfaces_ext::admin::AddressStateProviderError::ChannelClosed) => {
            log::error!(
                "{debug_log_prefix} observed channel closed while watching for \
                address event for address {ip_address} on interface {interface_id};\
                removing address",
            );
            // Note: treat AddressStateProvider channel closure the same as
            // address removal. This ultimately leads to the client exiting.
            dhcp_client_core::client::AddressEvent::Removed(
                ClientExitReason::AddressStateProviderError,
            )
        }
    }
}

struct HandledWatchConfigurationStep {
    state_inspect: StateInspect,
    lease_inspect: LeaseChangeInspect,
    response_to_return: Option<ClientWatchConfigurationResponse>,
}

fn into_fidl_list(
    list: impl IntoIterator<Item = std::net::Ipv4Addr>,
) -> Vec<fidl_fuchsia_net::Ipv4Address> {
    list.into_iter().map(|addr| net_types::ip::Ipv4Addr::from(addr).into_ext()).collect()
}
