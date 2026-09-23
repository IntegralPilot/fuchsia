// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(missing_docs)]

//! Extension types and helpers for the fuchsia.net.dhcpv6 FIDL library.

use fidl_table_validation::{ValidFidlTable, Validate};
use futures::FutureExt as _;
use futures::future::Either;

/// Parameters to configure a new client.
///
/// See [`fidl_fuchsia_net_dhcpv6::NewClientParams`].
#[derive(ValidFidlTable, Debug, Clone, PartialEq)]
#[fidl_table_src(fidl_fuchsia_net_dhcpv6::NewClientParams)]
#[fidl_table_strict]
pub struct NewClientParams {
    /// The ID of the interface the client will run on.
    ///
    /// See [`fidl_fuchsia_net_dhcpv6::NewClientParams::interface_id`].
    pub interface_id: fidl_fuchsia_net::InterfaceId,
    /// The socket address to use when communicating with servers.
    ///
    /// DHCPv6 servers listen for link-local multicasts, so not using a
    /// link-local address here may cause interoperability issues.
    ///
    /// Client creation will fail with `INVALID_ARGS` if:
    ///
    /// * a multicast address is provided;
    /// * or a link-local address is provided, and its zone index
    ///     doesn't match `interface_id` (Fuchsia has a 1:1 mapping from
    ///     zone index to interface ID).
    ///
    /// Client creation will fail if it fails to bind a socket to this
    /// address.
    ///
    /// See [`fidl_fuchsia_net_dhcpv6::NewClientParams::address`].
    pub address: fidl_fuchsia_net::Ipv6SocketAddress,
    /// Configuration for starting the DHCPv6 client.
    ///
    /// If the configuration requests both addresses and other
    /// configuration parameters, all information is requested in the
    /// same message exchange, running in stateful mode. If only
    /// configuration parameters are requested (no addresses), the
    /// client runs in stateless mode, as described in
    /// [RFC 8415, Section 6.1].
    ///
    /// Client creation will fail if `config` is not requesting any
    /// information (all fields are empty), or if it contains invalid
    /// fields.
    ///
    /// See [`fidl_fuchsia_net_dhcpv6::NewClientParams::config`].
    ///
    /// [RFC 8415, Section 6.1]: https://tools.ietf.org/html/rfc8415#section-6.1
    pub config: ClientConfig,
    #[fidl_field_type(optional)]
    /// DHCP Unique Identifier (DUID) configuration.
    ///
    /// The DUID is used by the client to identify itself to servers, as defined
    /// in [RFC 8415 section 11].
    ///
    /// [RFC 8415 section 11]: https://datatracker.ietf.org/doc/html/rfc8415#section-11
    pub duid: Option<fidl_fuchsia_net_dhcpv6::Duid>,
}

/// Configuration for what the client should request from DHCPv6 server(s).
///
/// See [`fidl_fuchsia_net_dhcpv6::ClientConfig`].
#[derive(ValidFidlTable, Debug, Clone, PartialEq)]
#[fidl_table_src(fidl_fuchsia_net_dhcpv6::ClientConfig)]
#[fidl_table_strict]
pub struct ClientConfig {
    #[fidl_field_type(default)]
    /// Configuration for requesting configuration information.
    ///
    /// See [`fidl_fuchsia_net_dhcpv6::ClientConfig::information_config`].
    pub information_config: InformationConfig,
    #[fidl_field_type(default)]
    /// Non-temporary address configuration.
    ///
    /// Configures the client to negotiate non-temporary
    /// addresses (IA_NA), as defined in
    /// [RFC 8415, section 6.2].
    ///
    /// See [`fidl_fuchsia_net_dhcpv6::ClientConfig::non_temporary_address_config`].
    ///
    /// [RFC 8415, section 6.2]: https://tools.ietf.org/html/rfc8415#section-6.2
    pub non_temporary_address_config: AddressConfig,
    #[fidl_field_type(optional)]
    /// Prefix delegation configuration.
    ///
    /// Configures the client to negotiate a delegated prefix
    /// (IA_PD), as defined in [RFC 8415, section 6.3][RFC 8415 6.3].
    ///
    /// Optional. If not set, delegated prefixes will not be
    /// requested. If invalid, client creation will fail and
    /// the pipelined channel will be closed.
    ///
    /// See [`fidl_fuchsia_net_dhcpv6::ClientConfig::prefix_delegation_config`].
    ///
    /// [RFC 8415 6.3]: https://datatracker.ietf.org/doc/html/rfc8415#section-6.3
    pub prefix_delegation_config: Option<fidl_fuchsia_net_dhcpv6::PrefixDelegationConfig>,
}

/// Configuration for informational data to request.
///
/// See [`fidl_fuchsia_net_dhcpv6::InformationConfig`].
#[derive(ValidFidlTable, Debug, Clone, PartialEq, Default)]
#[fidl_table_src(fidl_fuchsia_net_dhcpv6::InformationConfig)]
#[fidl_table_strict]
pub struct InformationConfig {
    #[fidl_field_type(default)]
    /// See [`fidl_fuchsia_net_dhcpv6::InformationConfig::dns_servers`].
    pub dns_servers: bool,
}

/// [`AddressConfig`] custom validation error.
#[derive(thiserror::Error, Debug)]
pub enum AddressConfigCustomValidationError {
    /// More preferred addresses than address count.
    #[error("more preferred addresses in {preferred_addresses:?} than count {address_count}")]
    TooManyPreferredAddresses {
        /// Address count.
        address_count: u8,
        /// Preferred addresses.
        preferred_addresses: Vec<fidl_fuchsia_net::Ipv6Address>,
    },
}

/// Custom [`AddressConfig`] validator.
pub struct AddressConfigValidator;

impl Validate<AddressConfig> for AddressConfigValidator {
    type Error = AddressConfigCustomValidationError;

    fn validate(
        AddressConfig { address_count, preferred_addresses }: &AddressConfig,
    ) -> Result<(), Self::Error> {
        match preferred_addresses.as_ref() {
            Some(preferred_addresses) => {
                if preferred_addresses.len() > (*address_count).into() {
                    Err(AddressConfigCustomValidationError::TooManyPreferredAddresses {
                        address_count: *address_count,
                        preferred_addresses: preferred_addresses.clone(),
                    })
                } else {
                    Ok(())
                }
            }
            None => Ok(()),
        }
    }
}

/// Configuration for requesting addresses.
///
/// See [`fidl_fuchsia_net_dhcpv6::AddressConfig`].
#[derive(ValidFidlTable, Debug, Clone, PartialEq, Default)]
#[fidl_table_src(fidl_fuchsia_net_dhcpv6::AddressConfig)]
#[fidl_table_validator(AddressConfigValidator)]
#[fidl_table_strict]
pub struct AddressConfig {
    #[fidl_field_type(default)]
    /// Number of addresses.
    ///
    /// If the value is 0, the client will not negotiate
    /// non-temporary addresses, i.e. its messages to the
    /// server will not contain the IA_NA option.
    ///
    /// See [`fidl_fuchsia_net_dhcpv6::AddressConfig::address_count`].
    pub address_count: u8,
    #[fidl_field_type(optional)]
    /// Preferred addresses.
    ///
    /// The addresses are used as hints by DHCPv6 servers,
    /// but may be ignored.
    ///
    /// The size of `preferred_addresses` must be less than
    /// or equal to `address_count`, otherwise the
    /// `AddressConfig` is invalid.
    ///
    /// Optional field. If not set, or if
    /// `preferred_addresses` is empty, no address hints are
    /// provided.
    ///
    /// See [`fidl_fuchsia_net_dhcpv6::AddressConfig::preferred_addresses`].
    pub preferred_addresses: Option<Vec<fidl_fuchsia_net::Ipv6Address>>,
}

/// Responses from watch methods on `fuchsia.net.dhcpv6/Client`.
#[derive(Debug)]
pub enum WatchItem {
    /// Return value of `fuchsia.net.dhcpv6/Client.WatchServers`.
    DnsServers(Vec<fidl_fuchsia_net_name::DnsServer_>),
    /// Return value of `fuchsia.net.dhcpv6/Client.WatchAddress`.
    Address {
        /// The address bits and prefix.
        addr: fidl_fuchsia_net::Subnet,
        /// Address parameters.
        parameters: fidl_fuchsia_net_interfaces_admin::AddressParameters,
        /// Server end of a `fuchsia.net.interfaces.admin/AddressStateProvider`
        /// protocol channel.
        address_state_provider_server_end: fidl::endpoints::ServerEnd<
            fidl_fuchsia_net_interfaces_admin::AddressStateProviderMarker,
        >,
    },
}

impl WatchItem {
    /// Constructs a new [`WatchItem::Address`].
    pub fn new_address(
        addr: fidl_fuchsia_net::Subnet,
        parameters: fidl_fuchsia_net_interfaces_admin::AddressParameters,
        address_state_provider_server_end: fidl::endpoints::ServerEnd<
            fidl_fuchsia_net_interfaces_admin::AddressStateProviderMarker,
        >,
    ) -> Self {
        Self::Address { addr, parameters, address_state_provider_server_end }
    }
}

/// Turns a [`fidl_fuchsia_net_dhcpv6::ClientProxy`] into a stream of items
/// yielded by calling all hanging get methods on the protocol.
///
/// [`fidl_fuchsia_net_dhcpv6::ClientProxy::watch_servers`] and
/// [`fidl_fuchsia_net_dhcpv6::ClientProxy::watch_address`] must never be
/// called on the protocol channel `client_proxy` belongs to once this function
/// returns until the stream ends or returns an error, as only one pending call
/// is allowed at a time.
pub fn into_watch_stream(
    client_proxy: fidl_fuchsia_net_dhcpv6::ClientProxy,
) -> impl futures::Stream<Item = Result<WatchItem, fidl::Error>> + Unpin {
    let watch_servers_fut = client_proxy.watch_servers();
    let watch_address_fut = client_proxy.watch_address();
    futures::stream::try_unfold(
        (client_proxy, watch_servers_fut, watch_address_fut),
        |(client_proxy, watch_servers_fut, watch_address_fut)| {
            futures::future::select(watch_servers_fut, watch_address_fut).map(|either| {
                match either {
                    Either::Left((servers_res, watch_address_fut)) => servers_res.map(|servers| {
                        let watch_servers_fut = client_proxy.watch_servers();
                        Some((
                            WatchItem::DnsServers(servers),
                            (client_proxy, watch_servers_fut, watch_address_fut),
                        ))
                    }),
                    Either::Right((address_res, watch_servers_fut)) => {
                        address_res.map(|(addr, parameters, address_state_provider_server_end)| {
                            let watch_address_fut = client_proxy.watch_address();
                            Some((
                                WatchItem::new_address(
                                    addr,
                                    parameters,
                                    address_state_provider_server_end,
                                ),
                                (client_proxy, watch_servers_fut, watch_address_fut),
                            ))
                        })
                    }
                }
                .or_else(|e| if e.is_closed() { Ok(None) } else { Err(e) })
            })
        },
    )
}

#[cfg(test)]
mod tests {
    use super::{WatchItem, into_watch_stream};

    use assert_matches::assert_matches;
    use futures::{StreamExt as _, TryStreamExt as _};
    use net_declare::fidl_ip_v6;
    use test_case::test_case;

    #[test_case(fidl_fuchsia_net_dhcpv6::AddressConfig {
        address_count: Some(0),
        preferred_addresses: Some(vec![fidl_ip_v6!("2001:db8::1")]),
        ..Default::default()
    })]
    #[test_case(fidl_fuchsia_net_dhcpv6::AddressConfig {
        address_count: Some(1),
        preferred_addresses: Some(vec![fidl_ip_v6!("2001:db8::1"), fidl_ip_v6!("2001:db8::2")]),
        ..Default::default()
    })]
    #[fuchsia::test]
    fn address_config_custom_validation(address_config: fidl_fuchsia_net_dhcpv6::AddressConfig) {
        let (want_address_count, want_preferred_addresses) = assert_matches!(
            address_config.clone(),
            fidl_fuchsia_net_dhcpv6::AddressConfig {
            address_count: Some(address_count),
            preferred_addresses: Some(preferred_addresses),
            ..
        } => (address_count, preferred_addresses));

        assert_matches!(
            crate::AddressConfig::try_from(address_config),
            Err(crate::AddressConfigValidationError::Logical(
                crate::AddressConfigCustomValidationError::TooManyPreferredAddresses {
                    address_count,
                    preferred_addresses,
                }
            )) => {
                assert_eq!(address_count, want_address_count);
                assert_eq!(preferred_addresses, want_preferred_addresses);
            }
        );
    }

    #[derive(Debug, Clone, Copy)]
    enum WatchType {
        DnsServers,
        Address,
    }

    const SUBNET: fidl_fuchsia_net::Subnet = net_declare::fidl_subnet!("abcd::1/128");

    /// Run a fake server which reads requests from `request_stream` and
    /// makes responses in the order as given in `response_types`.
    async fn run_fake_server(
        request_stream: &mut fidl_fuchsia_net_dhcpv6::ClientRequestStream,
        response_types: &[WatchType],
    ) {
        let (_, _, _): (
            &mut fidl_fuchsia_net_dhcpv6::ClientRequestStream,
            Option<fidl_fuchsia_net_dhcpv6::ClientWatchServersResponder>,
            Option<fidl_fuchsia_net_dhcpv6::ClientWatchAddressResponder>,
        ) = futures::stream::iter(response_types)
            .fold(
                (request_stream, None, None),
                |(request_stream, mut dns_servers_responder, mut address_responder),
                 watch_type_to_unblock| async move {
                    while dns_servers_responder.is_none() || address_responder.is_none() {
                        match request_stream
                            .try_next()
                            .await
                            .expect("FIDL error")
                            .expect("request stream ended")
                        {
                            fidl_fuchsia_net_dhcpv6::ClientRequest::WatchServers { responder } => {
                                assert_matches!(dns_servers_responder.replace(responder), None);
                            }
                            fidl_fuchsia_net_dhcpv6::ClientRequest::WatchAddress { responder } => {
                                assert_matches!(address_responder.replace(responder), None);
                            }
                            fidl_fuchsia_net_dhcpv6::ClientRequest::WatchPrefixes {
                                responder: _,
                            } => {
                                panic!("WatchPrefix method should not be called");
                            }
                            fidl_fuchsia_net_dhcpv6::ClientRequest::Shutdown { responder: _ } => {
                                panic!("Shutdown method should not be called");
                            }
                        }
                    }
                    match watch_type_to_unblock {
                        WatchType::Address => {
                            let (_, server_end) = fidl::endpoints::create_endpoints::<
                                fidl_fuchsia_net_interfaces_admin::AddressStateProviderMarker,
                            >();
                            address_responder
                                .take()
                                .expect("must have address responder")
                                .send(&SUBNET, &Default::default(), server_end)
                                .expect("FIDL error");
                        }
                        WatchType::DnsServers => {
                            dns_servers_responder
                                .take()
                                .expect("must have DNS servers responder")
                                .send(&[])
                                .expect("FIDL error");
                        }
                    };
                    (request_stream, dns_servers_responder, address_responder)
                },
            )
            .await;
    }

    /// Tests that polling the watcher stream causes all hanging get methods
    /// to be called, and the items yielded by the stream are in the order
    /// as the fake server is instructed to unblock the calls.
    #[test_case(&[WatchType::DnsServers, WatchType::DnsServers]; "dns_servers")]
    #[test_case(&[WatchType::Address, WatchType::Address]; "address")]
    #[test_case(&[WatchType::DnsServers, WatchType::Address]; "dns_servers_then_address")]
    #[test_case(&[WatchType::Address, WatchType::DnsServers]; "address_then_dns_servers")]
    #[fuchsia::test]
    async fn watch_stream(watch_types: &[WatchType]) {
        let (client_proxy, mut request_stream) =
            fidl::endpoints::create_proxy_and_stream::<fidl_fuchsia_net_dhcpv6::ClientMarker>();
        let mut watch_stream = into_watch_stream(client_proxy);
        let client_fut = watch_stream.by_ref().take(watch_types.len()).try_collect::<Vec<_>>();
        let (r, ()) = futures::future::join(
            client_fut,
            run_fake_server(request_stream.by_ref(), watch_types),
        )
        .await;
        let items = r.expect("watch stream error");
        assert_eq!(items.len(), watch_types.len());
        for (item, watch_type) in items.into_iter().zip(watch_types) {
            match watch_type {
                WatchType::Address => {
                    assert_matches!(
                        item,
                        WatchItem::Address {
                            addr,
                            parameters: fidl_fuchsia_net_interfaces_admin::AddressParameters {
                                initial_properties: None,
                                temporary: None,
                                ..
                            },
                            address_state_provider_server_end: _,
                        } if addr == SUBNET
                    );
                }
                WatchType::DnsServers => {
                    assert_matches!(
                        item,
                        WatchItem::DnsServers(dns_servers) if dns_servers.is_empty()
                    );
                }
            }
        }

        drop(request_stream);
        assert_matches!(
            watch_stream.try_collect::<Vec<_>>().await.expect("watch stream error").as_slice(),
            &[]
        );
    }
}
