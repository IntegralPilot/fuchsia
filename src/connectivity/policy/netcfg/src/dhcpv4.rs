// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::HashSet;
use std::pin::Pin;

use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_dhcp as fnet_dhcp;
use fidl_fuchsia_net_dhcp_ext::{self as fnet_dhcp_ext, ClientProviderExt as _};
use fidl_fuchsia_net_ext::FromExt as _;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fidl_fuchsia_net_name as fnet_name;
use fidl_fuchsia_net_resources as fnet_resources;
use fidl_fuchsia_net_routes_admin as fnet_routes_admin;

use anyhow::Context as _;
use async_utils::stream::{StreamMap, Tagged, WithTag as _};
use dns_server_watcher::{DEFAULT_DNS_PORT, DnsServers, DnsServersUpdateSource};
use fuchsia_async::TimeoutExt as _;
use futures::FutureExt;
use futures::channel::oneshot;
use futures::stream::StreamExt as _;
use log::{info, warn};
use net_types::SpecifiedAddr;
use net_types::ip::Ipv4Addr;

use crate::{InterfaceId, dns, errors, network};

#[derive(Debug)]
pub(super) struct ClientState {
    pub(super) routers: HashSet<SpecifiedAddr<Ipv4Addr>>,
    pub(super) route_set: fnet_routes_admin::RouteSetV4Proxy,
    pub(super) shutdown_sender: oneshot::Sender<()>,
}

pub(super) fn new_client_params() -> fnet_dhcp::NewClientParams {
    fnet_dhcp_ext::default_new_client_params()
}

pub(super) type ConfigurationStreamMap =
    StreamMap<InterfaceId, InterfaceIdTaggedConfigurationStream>;
pub(super) type InterfaceIdTaggedConfigurationStream = Tagged<InterfaceId, ConfigurationStream>;
pub(super) type ConfigurationStream =
    futures::stream::BoxStream<'static, Result<fnet_dhcp_ext::Configuration, fnet_dhcp_ext::Error>>;

pub(super) async fn probe_for_presence(provider: &fnet_dhcp::ClientProviderProxy) -> bool {
    match provider.check_presence().await {
        Ok(()) => true,
        Err(fidl::Error::ClientChannelClosed { .. }) => false,
        Err(e) => panic!("unexpected error while probing: {e}"),
    }
}

pub(super) async fn update_configuration(
    interface_id: InterfaceId,
    ClientState { shutdown_sender: _, routers: configured_routers, route_set }: &mut ClientState,
    configuration: fnet_dhcp_ext::Configuration,
    dns_servers: &mut DnsServers,
    dns_server_watch_responders: &mut dns::DnsServerWatchResponders,
    netpol_networks_service: &mut network::NetpolNetworksService,
    control: &fnet_interfaces_ext::admin::Control,
    lookup_admin: &fnet_name::LookupAdminProxy,
) {
    let fnet_dhcp_ext::Configuration {
        address,
        dns_servers: new_dns_servers,
        routers: new_routers,
        ..
    } = configuration;
    // Verify that the address parameters request performing DAD and adding a
    // subnet route, as expected of a DHCPv4 client.
    if let Some(address) = &address {
        if address.address_parameters.perform_dad != Some(true)
            || address.address_parameters.add_subnet_route != Some(true)
        {
            warn!(
                "invalid address parameters for DHCPv4: perform_dad={:?}, add_subnet_route={:?}",
                address.address_parameters.perform_dad, address.address_parameters.add_subnet_route
            );
            return;
        }
    }
    if let Some(address) = address {
        match address.add_to(control) {
            Ok(()) => {}
            Err((address, e)) => {
                let fnet::Ipv4AddressWithPrefix { addr, prefix_len } = address;
                warn!(
                    "error adding DHCPv4-acquired address {}/{} for interface {}: {}",
                    // Make sure address is pretty-printed so that PII filtering
                    // is properly applied on addresses.
                    Ipv4Addr::from_ext(addr),
                    prefix_len,
                    interface_id,
                    e,
                )
            }
        }
    }

    dns::update_servers(
        lookup_admin,
        dns_servers,
        dns_server_watch_responders,
        netpol_networks_service,
        DnsServersUpdateSource::Dhcpv4 { interface_id: interface_id.get() },
        new_dns_servers
            .iter()
            .map(|&address| fnet_name::DnsServer_ {
                address: Some(fnet::SocketAddress::Ipv4(fnet::Ipv4SocketAddress {
                    address,
                    port: DEFAULT_DNS_PORT,
                })),
                source: Some(fnet_name::DnsServerSource::Dhcp(fnet_name::DhcpDnsServerSource {
                    source_interface: Some(interface_id.get()),
                    ..fnet_name::DhcpDnsServerSource::default()
                })),
                ..fnet_name::DnsServer_::default()
            })
            .collect(),
    )
    .await;

    fnet_dhcp_ext::apply_new_routers(
        interface_id.into(),
        route_set,
        configured_routers,
        new_routers,
    )
    .await
    .unwrap_or_else(|e| {
        warn!("error applying new DHCPv4-acquired router configuration: {:?}", e);
    })
}

pub(super) async fn start_client(
    interface_id: InterfaceId,
    interface_name: &str,
    client_provider: &fnet_dhcp::ClientProviderProxy,
    route_set_provider: &fnet_routes_admin::RouteTableV4Proxy,
    interface_admin_auth: &fnet_resources::GrantForInterfaceAuthorization,
    configuration_streams: &mut ConfigurationStreamMap,
) -> Result<ClientState, errors::Error> {
    // LINT.IfChange(netcfg_dhcp_start_tefmo)
    info!("starting DHCPv4 client for {} (id={})", interface_name, interface_id);
    // LINT.ThenChange(//tools/testing/tefmocheck/cdc_ethernet_state_check.go:netcfg_dhcp_start_tefmo)

    let (route_set, server_end) =
        fidl::endpoints::create_proxy::<fnet_routes_admin::RouteSetV4Marker>();

    route_set_provider.new_route_set(server_end).expect("create new route set");

    let fnet_resources::GrantForInterfaceAuthorization { interface_id: id, token } =
        interface_admin_auth;

    route_set
        .authenticate_for_interface(fnet_resources::ProofOfInterfaceAuthorization {
            interface_id: *id,
            token: token
                .duplicate_handle(zx::Rights::TRANSFER)
                .expect("interface auth grant is guaranteed to have ZX_RIGHT_DUPLICATE"),
        })
        .await
        .map_err(|err| {
            errors::Error::NonFatal(anyhow::anyhow!(
                "FIDL error while getting authorization: {}",
                err
            ))
        })?
        .map_err(|err| {
            errors::Error::NonFatal(anyhow::anyhow!("error while getting authorization: {:?}", err))
        })?;

    let client = client_provider.new_client_end_ext(interface_id.into(), new_client_params());
    let (shutdown_sender, shutdown_receiver) = oneshot::channel();

    if let Some(stream) = configuration_streams.insert(
        interface_id,
        fnet_dhcp_ext::merged_configuration_stream(
            client,
            shutdown_receiver.map(|receive_result| match receive_result {
                Ok(()) => (),
                Err(oneshot::Canceled) => (),
            }),
        )
        .boxed()
        .tagged(interface_id),
    ) {
        let _: Pin<Box<InterfaceIdTaggedConfigurationStream>> = stream;
        unreachable!("only one DHCPv4 client may exist on {} (id={})", interface_name, interface_id)
    }

    Ok(ClientState { shutdown_sender, route_set, routers: Default::default() })
}

#[derive(Debug)]
pub(super) enum AlreadyObservedClientExit {
    Yes,
    No,
}

const TIMEOUT_WAITING_FOR_CLIENT_SHUTDOWN: std::time::Duration = std::time::Duration::from_secs(10);

pub(super) async fn stop_client(
    interface_id: InterfaceId,
    interface_name: &str,
    mut state: ClientState,
    configuration_streams: &mut ConfigurationStreamMap,
    dns_servers: &mut DnsServers,
    dns_server_watch_responders: &mut dns::DnsServerWatchResponders,
    netpol_networks_service: &mut network::NetpolNetworksService,
    control: &fnet_interfaces_ext::admin::Control,
    lookup_admin: &fnet_name::LookupAdminProxy,
    already_observed_exit: AlreadyObservedClientExit,
) {
    info!("stopping DHCPv4 client for {} (id={})", interface_name, interface_id);

    update_configuration(
        interface_id,
        &mut state,
        fnet_dhcp_ext::Configuration::default(),
        dns_servers,
        dns_server_watch_responders,
        netpol_networks_service,
        control,
        lookup_admin,
    )
    .await;

    let ClientState { shutdown_sender, route_set: _, routers: _ } = state;

    let stream: Pin<Box<InterfaceIdTaggedConfigurationStream>> =
        configuration_streams.remove(&interface_id).unwrap_or_else(|| {
            unreachable!(
                "DHCPv4 client must exist when stopping on {} (id={})",
                interface_name, interface_id,
            )
        });

    match already_observed_exit {
        AlreadyObservedClientExit::Yes => {
            // We already saw the client exit due to some error, so just
            // drop its configuration stream after having cleaned up
            // any configuration we obtained from it.
            drop(stream);
        }
        AlreadyObservedClientExit::No => {
            // Tell the client to shut down.
            shutdown_sender.send(()).expect(
                "shutdown_receiver should not have been dropped, \
                      as we still hold a stream holding it",
            );

            // Await the shutdown event.
            let terminal_event_result = stream
                .filter_map(|(_interface_id, item)| match item {
                    Ok(config) => {
                        warn!(
                            "observed DHCPv4 config stream while awaiting shutdown on \
                            {interface_name} (id={interface_id}): \
                            {config:?}"
                        );
                        futures::future::ready(None)
                    }
                    Err(err) => futures::future::ready(Some(err)),
                })
                .next()
                .map(Some)
                // Avoid blocking forever on the DHCP client if it's not
                // shutting down.
                .on_timeout(TIMEOUT_WAITING_FOR_CLIENT_SHUTDOWN, || None)
                .await;

            match terminal_event_result {
                None => {
                    log::error!(
                        "did not observe terminal event for DHCPv4 client on \
                        {interface_name} (id={interface_id})"
                    );
                }
                Some(err) => match err {
                    None => {
                        info!(
                            "DHCPv4 client gracefully shut down on {interface_name} \
                               (id={interface_id})"
                        );
                    }
                    Some(err) => {
                        warn!(
                            "DHCPv4 client exited on {interface_name} (id={interface_id}) \
                                (error={err:?})"
                        );
                    }
                },
            }
        }
    }
}

/// Start the DHCP server.
pub(super) async fn start_server(
    dhcp_server: &fnet_dhcp::Server_Proxy,
) -> Result<(), errors::Error> {
    dhcp_server
        .start_serving()
        .await
        .context("error sending start DHCP server request")
        .map_err(errors::Error::NonFatal)?
        .map_err(zx::Status::err_from_raw)
        .context("error starting DHCP server")
        .map_err(errors::Error::NonFatal)
}

/// Stop the DHCP server.
pub(super) async fn stop_server(
    dhcp_server: &fnet_dhcp::Server_Proxy,
) -> Result<(), errors::Error> {
    dhcp_server
        .stop_serving()
        .await
        .context("error sending stop DHCP server request")
        .map_err(errors::Error::NonFatal)
}
