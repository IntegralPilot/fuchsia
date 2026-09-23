// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Extensions for types in the `fidl_fuchsia_net_dhcp` crate.
#![deny(missing_docs)]

use std::collections::HashSet;
use std::num::NonZeroU64;

use anyhow::anyhow;
use async_trait::async_trait;
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_dhcp as fnet_dhcp;
use fidl_fuchsia_net_ext::IntoExt as _;
use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fidl_fuchsia_net_resources as fnet_resources;
use fidl_fuchsia_net_routes as fnet_routes;
use fidl_fuchsia_net_routes_admin as fnet_routes_admin;
use fidl_fuchsia_net_routes_ext as fnet_routes_ext;
use futures::{Future, FutureExt, Stream, StreamExt as _, TryStreamExt as _, pin_mut};
use net_declare::fidl_ip_v4_with_prefix;
use net_types::SpecifiedAddr;
use net_types::ip::{Ipv4, Ipv4Addr};

/// The default `fnet_dhcp::NewClientParams`.
pub fn default_new_client_params() -> fnet_dhcp::NewClientParams {
    fnet_dhcp::NewClientParams {
        configuration_to_request: Some(fnet_dhcp::ConfigurationToRequest {
            routers: Some(true),
            dns_servers: Some(true),
            ..fnet_dhcp::ConfigurationToRequest::default()
        }),
        request_ip_address: Some(true),
        ..fnet_dhcp::NewClientParams::default()
    }
}

/// Configuration acquired by the DHCP client.
#[derive(Default, Debug)]
pub struct Configuration {
    /// The acquired address.
    pub address: Option<Address>,
    /// Acquired DNS servers.
    pub dns_servers: Vec<fnet::Ipv4Address>,
    /// Acquired routers.
    pub routers: Vec<SpecifiedAddr<Ipv4Addr>>,
}

/// Domain errors for this crate.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// A FIDL domain object was invalid.
    #[error("invalid FIDL domain object: {0:?}")]
    ApiViolation(anyhow::Error),
    /// An error was encountered while manipulating a route set.
    #[error("errors while manipulating route set: {0:?}")]
    RouteSet(fnet_routes_admin::RouteSetError),
    /// A FIDL error was encountered.
    #[error("fidl error: {0:?}")]
    Fidl(fidl::Error),
    /// An invalid ClientExitReason was observed on the client's event stream.
    #[error("invalid exit reason: {0:?}")]
    WrongExitReason(fnet_dhcp::ClientExitReason),
    /// No ClientExitReason was provided, when one was expected.
    #[error("missing exit reason")]
    MissingExitReason,
    /// The client unexpectedly exited.
    #[error("unexpected exit; reason: {0:?}")]
    UnexpectedExit(Option<fnet_dhcp::ClientExitReason>),
}

/// The default subnet used as the destination while populating a
/// [`fnet_routes_ext::Route`] while applying newly-discovered routers.
const DEFAULT_SUBNET: net_types::ip::Subnet<Ipv4Addr> = net_declare::net_subnet_v4!("0.0.0.0/0");

/// The default subnet used as the destination while populating a
/// `fuchsia.net.routes.RouteV4` while applying newly-discovered routers.
pub const DEFAULT_ADDR_PREFIX: fnet::Ipv4AddressWithPrefix = fidl_ip_v4_with_prefix!("0.0.0.0/0");

/// Applies a new set of routers
///
/// Applies a new set of routers to a given `fuchsia.net.stack.Stack` and
/// set of configured routers by deleting forwarding entries for
/// newly-absent routers and adding forwarding entries for newly-present
/// ones.
pub async fn apply_new_routers(
    device_id: NonZeroU64,
    route_set: &fnet_routes_admin::RouteSetV4Proxy,
    configured_routers: &mut HashSet<SpecifiedAddr<Ipv4Addr>>,
    new_routers: impl IntoIterator<Item = SpecifiedAddr<Ipv4Addr>>,
) -> Result<(), Error> {
    let route = |next_hop: &SpecifiedAddr<Ipv4Addr>| fnet_routes_ext::Route::<Ipv4> {
        action: fnet_routes_ext::RouteAction::Forward(fnet_routes_ext::RouteTarget {
            outbound_interface: device_id.get(),
            next_hop: Some(*next_hop),
        }),
        destination: DEFAULT_SUBNET,
        properties: fnet_routes_ext::RouteProperties {
            specified_properties: fnet_routes_ext::SpecifiedRouteProperties {
                metric: fnet_routes::SpecifiedMetric::InheritedFromInterface(fnet_routes::Empty),
            },
        },
    };

    let new_routers = new_routers.into_iter().collect::<HashSet<_>>();

    for router in configured_routers.difference(&new_routers) {
        let removed: bool = route_set
            .remove_route(
                &route(router)
                    .try_into()
                    .map_err(|e| Error::ApiViolation(anyhow::Error::new(e)))?,
            )
            .await
            .map_err(Error::Fidl)?
            .map_err(Error::RouteSet)?;
        if !removed {
            log::warn!("attempt to remove {router} from RouteSet was no-op");
        }
    }

    for router in new_routers.difference(configured_routers) {
        let added: bool = route_set
            .add_route(
                &route(router)
                    .try_into()
                    .map_err(|e| Error::ApiViolation(anyhow::Error::new(e)))?,
            )
            .await
            .map_err(Error::Fidl)?
            .map_err(Error::RouteSet)?;
        if !added {
            log::warn!("attempt to add {router} to RouteSet was no-op");
        }
    }

    *configured_routers = new_routers;
    Ok(())
}

impl TryFrom<fnet_dhcp::ClientWatchConfigurationResponse> for Configuration {
    type Error = Error;
    fn try_from(
        fnet_dhcp::ClientWatchConfigurationResponse {
            address,
            dns_servers,
            routers,
            ..
        }: fnet_dhcp::ClientWatchConfigurationResponse,
    ) -> Result<Self, Error> {
        let address = address
            .map(
                |fnet_dhcp::Address {
                     address, address_parameters, address_state_provider, ..
                 }| {
                    Ok(Address {
                        address: address
                            .ok_or_else(|| anyhow!("Ipv4AddressWithPrefix should be present"))?,
                        address_parameters: address_parameters
                            .ok_or_else(|| anyhow!("AddressParameters should be present"))?,
                        address_state_provider: address_state_provider
                            .ok_or_else(|| anyhow!("AddressStateProvider should be present"))?,
                    })
                },
            )
            .transpose()
            .map_err(Error::ApiViolation);
        Ok(Configuration {
            address: address?,
            dns_servers: dns_servers.unwrap_or_default(),
            routers: routers
                .unwrap_or_default()
                .into_iter()
                .flat_map(|addr| SpecifiedAddr::new(addr.into_ext()))
                .collect(),
        })
    }
}

/// An IPv4 address acquired by the DHCP client.
#[derive(Debug)]
pub struct Address {
    /// The acquired address and discovered prefix length.
    pub address: fnet::Ipv4AddressWithPrefix,
    /// Parameters for the acquired address.
    pub address_parameters: fnet_interfaces_admin::AddressParameters,
    /// The server end for the AddressStateProvider owned by the DHCP client.
    pub address_state_provider: ServerEnd<fnet_interfaces_admin::AddressStateProviderMarker>,
}

impl Address {
    /// Adds this address via `fuchsia.net.interfaces.admin.Control`.
    pub fn add_to(
        self,
        control: &fnet_interfaces_ext::admin::Control,
    ) -> Result<
        (),
        (
            fnet::Ipv4AddressWithPrefix,
            fnet_interfaces_ext::admin::TerminalError<
                fnet_interfaces_admin::InterfaceRemovedReason,
            >,
        ),
    > {
        let Self { address, address_parameters, address_state_provider } = self;
        control
            .add_address(&address.into_ext(), &address_parameters, address_state_provider)
            .map_err(|e| (address, e))
    }
}

type ConfigurationStream = async_utils::hanging_get::client::HangingGetStream<
    fnet_dhcp::ClientProxy,
    fnet_dhcp::ClientWatchConfigurationResponse,
>;

/// Produces a stream of acquired DHCP configuration by executing the hanging
/// get on the provided DHCP client proxy.
pub fn configuration_stream(
    client: fnet_dhcp::ClientProxy,
) -> impl futures::Stream<Item = Result<Configuration, Error>> {
    ConfigurationStream::new_eager_with_fn_ptr(client, fnet_dhcp::ClientProxy::watch_configuration)
        .map_err(Error::Fidl)
        .and_then(|config| futures::future::ready(Configuration::try_from(config)))
}

/// Extension trait on `fidl_fuchsia_net_dhcp::ClientProviderProxy`.
pub trait ClientProviderExt {
    /// Construct a new DHCP client.
    fn new_client_ext(
        &self,
        interface_id: NonZeroU64,
        new_client_params: fnet_dhcp::NewClientParams,
    ) -> fnet_dhcp::ClientProxy;

    /// Construct a new DHCP client, returning a ClientEnd instead of a Proxy.
    fn new_client_end_ext(
        &self,
        interface_id: NonZeroU64,
        new_client_params: fnet_dhcp::NewClientParams,
    ) -> fidl::endpoints::ClientEnd<fnet_dhcp::ClientMarker>;
}

impl ClientProviderExt for fnet_dhcp::ClientProviderProxy {
    fn new_client_ext(
        &self,
        interface_id: NonZeroU64,
        new_client_params: fnet_dhcp::NewClientParams,
    ) -> fnet_dhcp::ClientProxy {
        let (client, server) = fidl::endpoints::create_proxy::<fnet_dhcp::ClientMarker>();
        self.new_client(interface_id.get(), &new_client_params, server)
            .expect("create new DHCPv4 client");
        client
    }

    fn new_client_end_ext(
        &self,
        interface_id: NonZeroU64,
        new_client_params: fnet_dhcp::NewClientParams,
    ) -> fidl::endpoints::ClientEnd<fnet_dhcp::ClientMarker> {
        let (client, server) = fidl::endpoints::create_endpoints::<fnet_dhcp::ClientMarker>();
        self.new_client(interface_id.get(), &new_client_params, server)
            .expect("create new DHCPv4 client");
        client
    }
}

/// Extension trait on `fidl_fuchsia_net_dhcp::ClientProxy`.
#[async_trait]
pub trait ClientExt {
    /// Shuts down the client, watching for the `GracefulShutdown` exit event.
    ///
    /// Returns an error if the `GracefulShutdown` exit event is not observed.
    async fn shutdown_ext(&self, event_stream: fnet_dhcp::ClientEventStream) -> Result<(), Error>;
}

#[async_trait]
impl ClientExt for fnet_dhcp::ClientProxy {
    async fn shutdown_ext(&self, event_stream: fnet_dhcp::ClientEventStream) -> Result<(), Error> {
        self.shutdown().map_err(Error::Fidl)?;

        let stream = event_stream.map_err(Error::Fidl).try_filter_map(|event| async move {
            match event {
                fnet_dhcp::ClientEvent::OnExit { reason } => Ok(match reason {
                    fnet_dhcp::ClientExitReason::ClientAlreadyExistsOnInterface
                    | fnet_dhcp::ClientExitReason::WatchConfigurationAlreadyPending
                    | fnet_dhcp::ClientExitReason::InvalidInterface
                    | fnet_dhcp::ClientExitReason::InvalidParams
                    | fnet_dhcp::ClientExitReason::NetworkUnreachable
                    | fnet_dhcp::ClientExitReason::AddressRemovedByUser
                    | fnet_dhcp::ClientExitReason::AddressStateProviderError
                    | fnet_dhcp::ClientExitReason::UnableToOpenSocket => {
                        return Err(Error::WrongExitReason(reason));
                    }
                    fnet_dhcp::ClientExitReason::GracefulShutdown => Some(()),
                }),
            }
        });

        pin_mut!(stream);
        stream.try_next().await.and_then(|option| match option {
            Some(()) => Ok(()),
            None => Err(Error::MissingExitReason),
        })
    }
}

/// Produces a stream that merges together the configuration hanging get
/// and the [`fnet_dhcp::ClientEvent::OnExit`] terminal event.
/// The client will be shut down when `shutdown_future` completes.
pub fn merged_configuration_stream(
    // Takes a `[fidl::endpoints::ClientEnd]` so that we know we can take
    // the event stream without panicking.
    client_end: fidl::endpoints::ClientEnd<fnet_dhcp::ClientMarker>,
    shutdown_future: impl Future<Output = ()> + 'static,
) -> impl Stream<Item = Result<Configuration, Error>> + 'static {
    let client = client_end.into_proxy();
    let event_stream = client.take_event_stream();

    let proxy_for_shutdown = client.clone();
    let shutdown_future = shutdown_future.map(move |()| proxy_for_shutdown.shutdown());
    let configs = configuration_stream(client);

    fn prio_left(_: &mut ()) -> futures::stream::PollNext {
        futures::stream::PollNext::Left
    }

    // Events yielded from a merged stream of client hanging gets or terminal
    // events.
    #[derive(Debug)]
    enum MergedClientEvent {
        // A terminal event yielded by the client's event stream.
        Terminal(Result<fnet_dhcp::ClientEvent, Error>),
        // Configuration acquired via the client's hanging get stream.
        WatchConfiguration(Result<Configuration, Error>),
        // A marker event indicating shutdown was requested by the caller.
        ShutdownRequested,
    }

    futures::stream::select_with_strategy(
        futures::stream::select_with_strategy(
            event_stream.map_err(Error::Fidl).map(MergedClientEvent::Terminal),
            // Merge in any error we observed telling the client to shut down so
            // that it can be observed as a problem with the terminal event
            // stream.
            futures::stream::once(shutdown_future).map(|result| match result {
                Ok(()) => MergedClientEvent::ShutdownRequested,
                Err(shutdown_err) => MergedClientEvent::Terminal(Err(Error::Fidl(shutdown_err))),
            }),
            prio_left,
        )
        // If the terminal event stream ends without showing an event, then
        // we're missing an exit reason.
        .chain(futures::stream::once(futures::future::ready(MergedClientEvent::Terminal(
            Err(Error::MissingExitReason),
        )))),
        configs.map(MergedClientEvent::WatchConfiguration),
        // Prioritize yielding terminal events.
        prio_left,
    )
    .scan((false, false), |(stream_ended, shutdown_requested), item| {
        if *stream_ended {
            return futures::future::ready(None);
        }

        futures::future::ready(Some(match item {
            MergedClientEvent::ShutdownRequested => {
                assert!(!*shutdown_requested);
                *shutdown_requested = true;
                None
            }
            MergedClientEvent::Terminal(terminal_result) => {
                *stream_ended = true;
                match terminal_result {
                    Ok(fnet_dhcp::ClientEvent::OnExit { reason }) => {
                        if *shutdown_requested {
                            match reason {
                                fnet_dhcp::ClientExitReason::GracefulShutdown => None,
                                fnet_dhcp::ClientExitReason::ClientAlreadyExistsOnInterface
                                | fnet_dhcp::ClientExitReason::WatchConfigurationAlreadyPending
                                | fnet_dhcp::ClientExitReason::InvalidInterface
                                | fnet_dhcp::ClientExitReason::InvalidParams
                                | fnet_dhcp::ClientExitReason::NetworkUnreachable
                                | fnet_dhcp::ClientExitReason::UnableToOpenSocket
                                | fnet_dhcp::ClientExitReason::AddressRemovedByUser
                                | fnet_dhcp::ClientExitReason::AddressStateProviderError => {
                                    Some(Err(Error::WrongExitReason(reason)))
                                }
                            }
                        } else {
                            Some(Err(Error::UnexpectedExit(Some(reason))))
                        }
                    }
                    Err(err) => Some(Err(match err {
                        err @ (Error::ApiViolation(_)
                        | Error::RouteSet(_)
                        | Error::Fidl(_)
                        | Error::UnexpectedExit(_)) => err,
                        Error::WrongExitReason(reason) => {
                            if *shutdown_requested {
                                Error::WrongExitReason(reason)
                            } else {
                                Error::UnexpectedExit(Some(reason))
                            }
                        }
                        Error::MissingExitReason => {
                            if *shutdown_requested {
                                Error::MissingExitReason
                            } else {
                                Error::UnexpectedExit(None)
                            }
                        }
                    })),
                }
            }
            MergedClientEvent::WatchConfiguration(watch_result) => {
                match watch_result {
                    Ok(config) => Some(Ok(config)),
                    Err(err) => {
                        // Treat all errors as fatal and stop the stream.
                        *stream_ended = true;
                        Some(Err(err))
                    }
                }
            }
        }))
    })
    .filter_map(futures::future::ready)
}

/// Contains types used when testing the DHCP client.
pub mod testutil {
    use super::*;
    use fuchsia_async as fasync;
    use futures::future::ready;

    /// Task for polling the DHCP client.
    pub struct DhcpClientTask {
        client: fnet_dhcp::ClientProxy,
        task: futures::future::Shared<fasync::Task<()>>,
    }

    impl DhcpClientTask {
        /// Creates and returns an async task that polls the DHCP client.
        pub fn new(
            client: fnet_dhcp::ClientProxy,
            id: NonZeroU64,
            route_set: fnet_routes_admin::RouteSetV4Proxy,
            control: fnet_interfaces_ext::admin::Control,
        ) -> DhcpClientTask {
            DhcpClientTask {
                client: client.clone(),
                task: fasync::Task::spawn(async move {
                    let fnet_resources::GrantForInterfaceAuthorization { interface_id, token } =
                        control
                            .get_authorization_for_interface()
                            .await
                            .expect("get interface authorization");
                    route_set
                        .authenticate_for_interface(fnet_resources::ProofOfInterfaceAuthorization {
                            interface_id,
                            token,
                        })
                        .await
                        .expect("authenticate should not have FIDL error")
                        .expect("authenticate should succeed");

                    let mut final_routers =
                        configuration_stream(client)
                            .scan((), |(), item| {
                                ready(match item {
                                    Err(e) => match e {
                                        // Observing `PEER_CLOSED` is expected after the
                                        // client is shut down, so rather than returning an
                                        // error, simply end the stream.
                                        Error::Fidl(fidl::Error::ClientChannelClosed {
                                            epitaph: fidl::Epitaph::PeerClosed,
                                            ..
                                        }) => None,
                                        Error::Fidl(_)
                                        | Error::ApiViolation(_)
                                        | Error::RouteSet(_)
                                        | Error::WrongExitReason(_)
                                        | Error::UnexpectedExit(_)
                                        | Error::MissingExitReason => Some(Err(e)),
                                    },
                                    Ok(item) => Some(Ok(item)),
                                })
                            })
                            .try_fold(
                                HashSet::<SpecifiedAddr<Ipv4Addr>>::new(),
                                |mut routers,
                                 Configuration {
                                     address,
                                     dns_servers: _,
                                     routers: new_routers,
                                 }| {
                                    let control = &control;
                                    let route_set = &route_set;
                                    async move {
                                        if let Some(address) = address {
                                            address
                                                .add_to(control)
                                                .expect("add address should succeed");
                                        }

                                        apply_new_routers(id, route_set, &mut routers, new_routers)
                                            .await
                                            .expect("applying new routers should succeed");
                                        Ok(routers)
                                    }
                                },
                            )
                            .await
                            .expect("watch_configuration should succeed");

                    // DHCP client is being shut down, so we should remove all the routers.
                    apply_new_routers(id, &route_set, &mut final_routers, Vec::new())
                        .await
                        .expect("removing all routers should succeed");
                })
                .shared(),
            }
        }

        /// Returns a future that resolves when the running DHCP client has shut down.
        pub fn wait_shutdown(&self) -> impl Future<Output = ()> + 'static {
            self.task.clone()
        }

        /// Shuts down the running DHCP client and waits for the poll task to complete.
        pub async fn shutdown(self) -> Result<(), Error> {
            let DhcpClientTask { client, task } = self;
            client
                .shutdown_ext(client.take_event_stream())
                .await
                .expect("client shutdown should succeed");
            task.await;
            Ok(())
        }
    }
}

#[cfg(test)]
mod test {
    use crate::{ClientExt as _, DEFAULT_ADDR_PREFIX, Error};

    use std::collections::HashSet;
    use std::num::NonZeroU64;

    use assert_matches::assert_matches;
    use fidl::endpoints::RequestStream;
    use fidl_fuchsia_net as fnet;
    use fidl_fuchsia_net_dhcp as fnet_dhcp;
    use fidl_fuchsia_net_ext::IntoExt as _;
    use fidl_fuchsia_net_routes as fnet_routes;
    use fidl_fuchsia_net_routes_admin as fnet_routes_admin;
    use fuchsia_async as fasync;
    use futures::channel::oneshot;
    use futures::{FutureExt as _, StreamExt as _, join, pin_mut};
    use net_declare::net_ip_v4;
    use net_types::ip::{Ip, Ipv4, Ipv4Addr};
    use net_types::{SpecifiedAddr, SpecifiedAddress as _, Witness as _};
    use proptest::prelude::*;
    use test_case::test_case;

    #[derive(proptest_derive::Arbitrary, Clone, Debug)]
    struct Address {
        include_address: bool,
        include_address_parameters: bool,
        include_address_state_provider: bool,
    }

    // For the purposes of this test, we only care about exercising the case
    // where addresses are specified or unspecified, with no need to be able
    // to distinguish between specified addresses.
    #[derive(proptest_derive::Arbitrary, Clone, Debug)]
    enum GeneratedIpv4Addr {
        Specified,
        Unspecified,
    }

    impl From<GeneratedIpv4Addr> for Ipv4Addr {
        fn from(value: GeneratedIpv4Addr) -> Self {
            match value {
                GeneratedIpv4Addr::Specified => net_ip_v4!("1.1.1.1"),
                GeneratedIpv4Addr::Unspecified => Ipv4::UNSPECIFIED_ADDRESS,
            }
        }
    }

    #[derive(proptest_derive::Arbitrary, Clone, Debug)]
    struct ClientWatchConfigurationResponse {
        address: Option<Address>,
        dns_servers: Option<Vec<GeneratedIpv4Addr>>,
        routers: Option<Vec<GeneratedIpv4Addr>>,
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            failure_persistence: Some(
                Box::<proptest::test_runner::MapFailurePersistence>::default()
            ),
            ..ProptestConfig::default()
        })]

        #[test]
        fn try_into_configuration(response: ClientWatchConfigurationResponse) {
            let make_fidl = |response: &ClientWatchConfigurationResponse| {
                let ClientWatchConfigurationResponse {
                    address,
                    dns_servers,
                    routers,
                } = response.clone();

                fnet_dhcp::ClientWatchConfigurationResponse {
                    address: address.map(
                        |Address {
                            include_address,
                            include_address_parameters,
                            include_address_state_provider
                        }| {
                        fnet_dhcp::Address {
                            address: include_address.then_some(
                                fidl_fuchsia_net::Ipv4AddressWithPrefix {
                                    addr: net_ip_v4!("1.1.1.1").into_ext(),
                                    prefix_len: 24,
                                }
                            ),
                            address_parameters: include_address_parameters.then_some(
                                fidl_fuchsia_net_interfaces_admin::AddressParameters::default()
                            ),
                            address_state_provider: include_address_state_provider.then_some({
                                let (_, server) = fidl::endpoints::create_endpoints();
                                server
                            }),
                            ..Default::default()
                        }
                    }),
                    dns_servers: dns_servers.map(
                        |list| list.into_iter().map(
                            |addr: GeneratedIpv4Addr| net_types::ip::Ipv4Addr::from(
                                addr
                            ).into_ext()
                        ).collect()),
                    routers: routers.map(
                        |list| list.into_iter().map(
                            |addr: GeneratedIpv4Addr| net_types::ip::Ipv4Addr::from(
                                addr
                            ).into_ext()
                        ).collect()),
                    ..Default::default()
                }
            };

            let result = crate::Configuration::try_from(make_fidl(&response));

            if let Some(crate::Configuration {
                address: result_address,
                dns_servers: result_dns_servers,
                routers: result_routers,
            }) = match response.address {
                Some(
                    Address {
                        include_address,
                        include_address_parameters,
                        include_address_state_provider,
                    }
                ) => {
                    prop_assert_eq!(
                        !(
                            include_address &&
                            include_address_parameters &&
                            include_address_state_provider
                        ),
                        result.is_err(),
                        "must reject partially-filled address object"
                    );

                    result.ok()
                }
                None => {
                    prop_assert!(result.is_ok(), "absent address is always accepted");
                    Some(result.unwrap())
                }
            } {
                let fnet_dhcp::ClientWatchConfigurationResponse {
                    dns_servers: fidl_dns_servers,
                    routers: fidl_routers,
                    address: fidl_address,
                    ..
                } = make_fidl(&response);
                let want_routers: Vec<net_types::ip::Ipv4Addr> = fidl_routers
                    .unwrap_or_default()
                    .into_iter()
                    .flat_map(
                        |addr| Some(addr.into_ext()).filter(net_types::ip::Ipv4Addr::is_specified)
                    )
                    .collect();
                prop_assert_eq!(
                    result_dns_servers,
                    fidl_dns_servers.unwrap_or_default()
                );
                prop_assert_eq!(
                    result_routers.into_iter().map(|addr| addr.get()).collect::<Vec<_>>(),
                    want_routers
                );

                if let Some(
                    crate::Address {
                        address: result_address,
                        address_parameters: result_address_parameters,
                        address_state_provider: _
                    }
                ) = result_address {
                    let fnet_dhcp::Address {
                        address: fidl_address,
                        address_parameters: fidl_address_parameters,
                        ..
                    } = fidl_address.expect("should be present");

                    prop_assert_eq!(Some(result_address), fidl_address);
                    prop_assert_eq!(Some(result_address_parameters), fidl_address_parameters);
                }
            }
        }
    }

    #[fasync::run_singlethreaded(test)]
    async fn apply_new_routers() {
        let (route_set, route_set_stream) =
            fidl::endpoints::create_proxy_and_stream::<fnet_routes_admin::RouteSetV4Marker>();

        const REMOVED_ROUTER: Ipv4Addr = net_ip_v4!("1.1.1.1");
        const KEPT_ROUTER: Ipv4Addr = net_ip_v4!("2.2.2.2");
        const ADDED_ROUTER: Ipv4Addr = net_ip_v4!("3.3.3.3");

        let mut configured_routers = [REMOVED_ROUTER, KEPT_ROUTER]
            .into_iter()
            .map(|addr| SpecifiedAddr::new(addr).unwrap())
            .collect::<HashSet<_>>();

        let device_id = NonZeroU64::new(5).unwrap();

        let apply_fut = crate::apply_new_routers(
            device_id,
            &route_set,
            &mut configured_routers,
            vec![
                SpecifiedAddr::new(KEPT_ROUTER).unwrap(),
                SpecifiedAddr::new(ADDED_ROUTER).unwrap(),
            ],
        )
        .fuse();

        let route_set_fut = async move {
            pin_mut!(route_set_stream);
            let (route, responder) = route_set_stream
                .next()
                .await
                .expect("should not have ended")
                .expect("should not have error")
                .into_remove_route()
                .expect("should be remove route");
            assert_eq!(
                route,
                fnet_routes::RouteV4 {
                    destination: DEFAULT_ADDR_PREFIX,
                    action: fnet_routes::RouteActionV4::Forward(fnet_routes::RouteTargetV4 {
                        outbound_interface: device_id.get(),
                        next_hop: Some(Box::new(REMOVED_ROUTER.into_ext()))
                    }),
                    properties: fnet_routes::RoutePropertiesV4 {
                        specified_properties: Some(fnet_routes::SpecifiedRouteProperties {
                            metric: Some(fnet_routes::SpecifiedMetric::InheritedFromInterface(
                                fnet_routes::Empty
                            )),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }
                }
            );
            responder.send(Ok(true)).expect("responder send");

            let (route, responder) = route_set_stream
                .next()
                .await
                .expect("should not have ended")
                .expect("should not have error")
                .into_add_route()
                .expect("should be add route");
            assert_eq!(
                route,
                fnet_routes::RouteV4 {
                    destination: DEFAULT_ADDR_PREFIX,
                    action: fnet_routes::RouteActionV4::Forward(fnet_routes::RouteTargetV4 {
                        outbound_interface: device_id.get(),
                        next_hop: Some(Box::new(ADDED_ROUTER.into_ext()))
                    }),
                    properties: fnet_routes::RoutePropertiesV4 {
                        specified_properties: Some(fnet_routes::SpecifiedRouteProperties {
                            metric: Some(fnet_routes::SpecifiedMetric::InheritedFromInterface(
                                fnet_routes::Empty
                            )),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }
                }
            );
            responder.send(Ok(true)).expect("responder send");
        }
        .fuse();

        pin_mut!(apply_fut, route_set_fut);
        let (apply_result, ()) = join!(apply_fut, route_set_fut);
        apply_result.expect("apply should succeed");
    }

    #[test_case(
        None => matches Err(Error::MissingExitReason) ; "no exit reason should cause error"
    )]
    #[test_case(
        Some(fnet_dhcp::ClientExitReason::NetworkUnreachable) => matches Err(Error::WrongExitReason(fnet_dhcp::ClientExitReason::NetworkUnreachable)) ;
        "wrong exit reason should cause error"
    )]
    #[test_case(
        Some(fnet_dhcp::ClientExitReason::GracefulShutdown) => matches Ok(()) ;
        "GracefulShutdown is correct exit reason"
    )]
    #[fasync::run_singlethreaded(test)]
    async fn shutdown_ext(exit_reason: Option<fnet_dhcp::ClientExitReason>) -> Result<(), Error> {
        let (client, stream) =
            fidl::endpoints::create_proxy_and_stream::<fnet_dhcp::ClientMarker>();

        if let Some(exit_reason) = exit_reason {
            stream.control_handle().send_on_exit(exit_reason).expect("send on exit");
        }

        let shutdown_fut = client.shutdown_ext(client.take_event_stream()).fuse();
        let server_fut = async move {
            pin_mut!(stream);
            let _client_control_handle = stream
                .next()
                .await
                .expect("should not have ended")
                .expect("should not have FIDL error")
                .into_shutdown()
                .expect("should be shutdown request");
        }
        .fuse();

        let (shutdown_result, ()) = join!(shutdown_fut, server_fut);
        shutdown_result
    }

    #[test_case(
        None ; "client does not exit until we tell it to"
    )]
    #[test_case(
        Some(fnet_dhcp::ClientExitReason::NetworkUnreachable);
        "client exits due to network unreachable"
    )]
    #[test_case(
        Some(fnet_dhcp::ClientExitReason::GracefulShutdown);
        "client exits due to GracefulShutdown of its own accord"
    )]
    #[fasync::run_singlethreaded(test)]
    async fn merged_configuration_stream_exit(exit_reason: Option<fnet_dhcp::ClientExitReason>) {
        const ADDRESS: fnet::Ipv4AddressWithPrefix =
            net_declare::fidl_ip_v4_with_prefix!("192.0.2.1/32");

        let (client, stream) = fidl::endpoints::create_request_stream::<fnet_dhcp::ClientMarker>();

        let server_fut = async move {
            pin_mut!(stream);

            let watch_config_responder = stream
                .next()
                .await
                .expect("should not have ended")
                .expect("should not have FIDL error")
                .into_watch_configuration()
                .expect("should be watch configuration");

            let (_asp_client, asp_server) = fidl::endpoints::create_endpoints::<
                fidl_fuchsia_net_interfaces_admin::AddressStateProviderMarker,
            >();

            watch_config_responder
                .send(fnet_dhcp::ClientWatchConfigurationResponse {
                    address: Some(fnet_dhcp::Address {
                        address: Some(ADDRESS),
                        address_parameters: Some(
                            fidl_fuchsia_net_interfaces_admin::AddressParameters::default(),
                        ),
                        address_state_provider: Some(asp_server),
                        ..Default::default()
                    }),
                    ..Default::default()
                })
                .expect("should successfully respond to hanging get");

            // Should keep polling hanging get.
            let _watch_config_responder = stream
                .next()
                .await
                .expect("should not have ended")
                .expect("should not have FIDL error")
                .into_watch_configuration()
                .expect("should be watch configuration");

            if let Some(exit_reason) = exit_reason {
                stream.control_handle().send_on_exit(exit_reason).expect("send on exit");
            } else {
                let _client_control_handle = stream
                    .next()
                    .await
                    .expect("should not have ended")
                    .expect("should not have FIDL error")
                    .into_shutdown()
                    .expect("should be shutdown request");
                stream
                    .control_handle()
                    .send_on_exit(fnet_dhcp::ClientExitReason::GracefulShutdown)
                    .expect("send on exit");
            }
        }
        .fuse();

        let client_fut = async move {
            let (shutdown_sender, shutdown_receiver) = oneshot::channel();

            let config_stream = crate::merged_configuration_stream(
                client,
                shutdown_receiver.map(|res| res.expect("shutdown_sender should not be dropped")),
            )
            .fuse();
            pin_mut!(config_stream);

            let initial_config = config_stream.next().await.expect("should not have ended");
            let address = assert_matches!(initial_config,
                Ok(crate::Configuration {
                    address: Some(crate::Address { address, .. }),
                    ..
                }) => address
            );
            assert_eq!(address, ADDRESS);

            if let Some(want_reason) = exit_reason {
                // The DHCP client exits on its own.
                let item = config_stream.next().await.expect("should not have ended");
                let got_reason = assert_matches!(item,
                    Err(Error::UnexpectedExit(Some(reason))) => reason
                );
                assert_eq!(got_reason, want_reason);

                // The stream should have ended.
                assert_matches!(config_stream.next().await, None);
            } else {
                // Poll the config stream once to indicate we're still hanging-getting.
                assert_matches!(config_stream.next().now_or_never(), None);
                shutdown_sender.send(()).expect("shutdown receiver should not have been dropped");

                // Having sent a shutdown request, we expect the client to exit
                // and the stream to end with no error.
                assert_matches!(config_stream.next().await, None);
            }
        };

        let ((), ()) = join!(client_fut, server_fut);
    }
}
