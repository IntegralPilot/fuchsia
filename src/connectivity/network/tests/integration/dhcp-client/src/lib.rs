// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use assert_matches::assert_matches;
use bstr::BString;
use diagnostics_assertions::AnyUintProperty;
use fidl::endpoints;
use fidl::endpoints::Responder as _;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_dhcp::{
    self as fnet_dhcp, ClientEvent, ClientExitReason, ClientMarker, ClientProviderMarker,
    NewClientParams,
};
use fidl_fuchsia_net_dhcp_ext::{self as fnet_dhcp_ext, ClientProviderExt as _};
use fidl_fuchsia_net_ext::{self as fnet_ext, IntoExt as _};
use fidl_fuchsia_net_interfaces as fnet_interfaces;
use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use fidl_fuchsia_netemul_network as fnetemul_network;
use fnet_dhcp_ext::ClientExt;
use fuchsia_async as fasync;
use futures::future::ready;
use futures::{FutureExt, StreamExt, TryStreamExt, join};
use net_declare::std_ip_v4;
use netemul::RealmUdpSocket as _;
use netstack_testing_common::interfaces::TestInterfaceExt as _;
use netstack_testing_common::realms::{
    KnownServiceProvider, Netstack, Netstack3, TestSandboxExt as _,
};
use netstack_testing_common::{annotate, dhcpv4 as dhcpv4_helper};
use netstack_testing_macros::netstack_test;
use std::pin::pin;
use test_case::test_case;

const MAC: net_types::ethernet::Mac = net_declare::net_mac!("00:00:00:00:00:01");
const SERVER_MAC: net_types::ethernet::Mac = net_declare::net_mac!("02:02:02:02:02:02");

struct DhcpTestRealm<'a> {
    client_realm: netemul::TestRealm<'a>,
    client_iface: netemul::TestInterface<'a>,
    server_realm: netemul::TestRealm<'a>,
    server_iface: netemul::TestInterface<'a>,
    _network: netemul::TestNetwork<'a>,
}

#[derive(Clone, Copy, Debug)]
enum DhcpServerAddress {
    Primary,
    Secondary,
}

fn server_test_config(address: DhcpServerAddress) -> dhcpv4_helper::TestConfig {
    dhcpv4_helper::TestConfig::new(
        match address {
            DhcpServerAddress::Primary => 1,
            DhcpServerAddress::Secondary => 2,
        },
        3..6,
    )
}

impl<'a> DhcpTestRealm<'a> {
    async fn start_dhcp_server(&self, address: DhcpServerAddress) {
        self.start_dhcp_server_with_options(address, [], []).await
    }

    async fn start_dhcp_server_with_options(
        &self,
        address: DhcpServerAddress,
        parameters: impl IntoIterator<Item = fnet_dhcp::Parameter>,
        options: impl IntoIterator<Item = fnet_dhcp::Option_>,
    ) {
        let Self { server_realm, server_iface, client_realm: _, client_iface: _, _network: _ } =
            self;

        let config = server_test_config(address);

        server_iface
            .add_address_and_subnet_route(config.server_addr_with_prefix().into_ext())
            .await
            .expect("add address should succeed");

        let server_proxy = server_realm
            .connect_to_protocol::<fnet_dhcp::Server_Marker>()
            .expect("connect to Server_ protocol should succeed");

        dhcpv4_helper::set_server_settings(
            &server_proxy,
            config
                .dhcp_parameters()
                .into_iter()
                .chain([fnet_dhcp::Parameter::BoundDeviceNames(vec![
                    server_iface
                        .get_interface_name()
                        .await
                        .expect("get interface name should succeed"),
                ])])
                .chain(parameters),
            options,
        )
        .await;

        server_proxy
            .start_serving()
            .await
            .expect("start_serving should not encounter FIDL error")
            .expect("start_serving should succeed");
    }

    async fn switch_dhcp_server_address(
        &self,
        old_address: DhcpServerAddress,
        new_address: DhcpServerAddress,
    ) {
        let Self { server_realm, server_iface, client_realm: _, client_iface: _, _network: _ } =
            self;
        let server_proxy = server_realm
            .connect_to_protocol::<fnet_dhcp::Server_Marker>()
            .expect("connect to Server_ protocol should succeed");
        server_proxy.stop_serving().await.expect("stop_serving should not encounter FIDL error");
        let old_config = server_test_config(old_address);
        assert!(
            server_iface
                .del_address_and_subnet_route(old_config.server_addr_with_prefix().into_ext())
                .await
                .expect("removing address should succeed")
        );

        let new_config = server_test_config(new_address);
        server_proxy
            .set_parameter(&fnet_dhcp::Parameter::IpAddrs(vec![new_config.server_addr]))
            .await
            .expect("setting parameter shouldn't have FIDL error")
            .expect("setting new server addr should succeed");

        server_iface
            .add_address_and_subnet_route(new_config.server_addr_with_prefix().into_ext())
            .await
            .expect("add address should succeed");
        server_proxy
            .start_serving()
            .await
            .expect("start_serving should not encounter FIDL error")
            .expect("start_serving should succeed");
    }

    async fn stop_dhcp_server(&self, address: DhcpServerAddress) {
        let Self { server_realm, server_iface, client_realm: _, client_iface: _, _network: _ } =
            self;
        let server_proxy = server_realm
            .connect_to_protocol::<fnet_dhcp::Server_Marker>()
            .expect("connect to Server_ protocol should succeed");
        server_proxy.stop_serving().await.expect("stop_serving should not encounter FIDL error");

        let config = server_test_config(address);
        assert!(
            server_iface
                .del_address_and_subnet_route(config.server_addr_with_prefix().into_ext())
                .await
                .expect("removing address should succeed")
        );
    }
}

async fn create_test_realm<'a, N: Netstack>(
    sandbox: &'a netemul::TestSandbox,
    name: &'a str,
) -> DhcpTestRealm<'a> {
    let network =
        sandbox.create_network("dhcp-test-network").await.expect("create network should succeed");
    let client_realm: netemul::TestRealm<'_> = sandbox
        .create_netstack_realm_with::<N, _, _>(
            format!("client-realm-{name}"),
            &[KnownServiceProvider::DhcpClient],
        )
        .expect("create realm should succeed");

    let client_iface = client_realm
        .join_network_with(
            &network,
            "clientiface",
            fnetemul_network::EndpointConfig {
                mtu: netemul::DEFAULT_MTU,
                mac: Some(Box::new(fnet_ext::MacAddress { octets: MAC.bytes() }.into())),
                port_class: fidl_fuchsia_hardware_network::PortClass::Virtual,
                checksum_offload: false,
            },
            netemul::InterfaceConfig { name: Some("clientiface".into()), ..Default::default() },
        )
        .await
        .expect("join network with realm should succeed");
    client_iface.apply_nud_flake_workaround().await.expect("nud flake workaround");

    let server_realm: netemul::TestRealm<'_> = sandbox
        .create_netstack_realm_with::<N, _, _>(
            format!("server-realm-{name}"),
            &[KnownServiceProvider::DhcpServer { persistent: false }],
        )
        .expect("create realm should succeed");

    let server_iface = server_realm
        .join_network_with(
            &network,
            "serveriface",
            fnetemul_network::EndpointConfig {
                mtu: netemul::DEFAULT_MTU,
                mac: Some(Box::new(fnet_ext::MacAddress { octets: SERVER_MAC.bytes() }.into())),
                port_class: fidl_fuchsia_hardware_network::PortClass::Virtual,
                checksum_offload: false,
            },
            netemul::InterfaceConfig { name: Some("serveriface".into()), ..Default::default() },
        )
        .await
        .expect("join network with realm should succeed");
    server_iface.apply_nud_flake_workaround().await.expect("nud flake workaround");

    DhcpTestRealm { client_realm, client_iface, server_realm, server_iface, _network: network }
}

#[netstack_test]
#[variant(N, Netstack)]
async fn client_provider_two_overlapping_clients_on_same_interface<N: Netstack>(name: &str) {
    let sandbox: netemul::TestSandbox = netemul::TestSandbox::new().unwrap();
    let DhcpTestRealm { client_realm, client_iface, server_realm: _, server_iface: _, _network: _ } =
        &create_test_realm::<N>(&sandbox, name).await;

    let proxy = client_realm.connect_to_protocol::<ClientProviderMarker>().unwrap();
    let client_iface = &client_iface;

    let (client_a, server_end_a) = endpoints::create_proxy::<ClientMarker>();
    let (client_b, server_end_b) = endpoints::create_proxy::<ClientMarker>();

    proxy
        .new_client(
            client_iface.id(),
            &NewClientParams {
                configuration_to_request: None,
                request_ip_address: Some(true),
                ..Default::default()
            },
            server_end_a,
        )
        .expect("creating new client should succeed");

    proxy
        .new_client(
            client_iface.id(),
            &NewClientParams {
                configuration_to_request: None,
                request_ip_address: Some(true),
                ..Default::default()
            },
            server_end_b,
        )
        .expect("creating new client should succeed");

    let watch_fut_a = client_a.watch_configuration();
    let watch_fut_b = client_b.watch_configuration();
    let (result_a, result_b) = join!(watch_fut_a, watch_fut_b);

    assert_matches!(result_a, Err(fidl::Error::ClientChannelClosed { .. }));
    assert_matches!(result_b, Err(fidl::Error::ClientChannelClosed { .. }));

    let on_exit = client_b
        .take_event_stream()
        .try_next()
        .now_or_never()
        .expect("event should be already available")
        .expect("event stream should not have ended before yielding exit reason")
        .expect("event stream should not have FIDL error");
    assert_matches!(
        on_exit,
        ClientEvent::OnExit { reason: ClientExitReason::ClientAlreadyExistsOnInterface }
    )
}

#[netstack_test]
#[variant(N, Netstack)]
async fn client_provider_two_non_overlapping_clients_on_same_interface<N: Netstack>(name: &str) {
    let sandbox: netemul::TestSandbox = netemul::TestSandbox::new().unwrap();

    let DhcpTestRealm { client_realm, client_iface, server_realm: _, server_iface: _, _network: _ } =
        &create_test_realm::<N>(&sandbox, name).await;

    let proxy = client_realm.connect_to_protocol::<ClientProviderMarker>().unwrap();
    let client_iface = &client_iface;

    let proxy = &proxy;
    // Executing the following block twice demonstrates that we can run
    // and shutdown DHCP clients on the same interface without running
    // afoul of the multiple-clients-on-same-interface restriction.
    for () in [(), ()] {
        let (client, server_end) = endpoints::create_proxy::<ClientMarker>();

        proxy
            .new_client(
                client_iface.id(),
                &NewClientParams {
                    configuration_to_request: None,
                    request_ip_address: Some(true),
                    ..Default::default()
                },
                server_end,
            )
            .expect("creating new client should succeed");

        client.shutdown().expect("shutdown call should not have FIDL error");
        let watch_result = client.watch_configuration().await;

        assert_matches!(watch_result, Err(fidl::Error::ClientChannelClosed { .. }));

        let on_exit = client
            .take_event_stream()
            .try_next()
            .now_or_never()
            .expect("event should be already available")
            .expect("event stream should not have ended before yielding exit reason")
            .expect("event stream should not have FIDL error");
        assert_matches!(
            on_exit,
            ClientEvent::OnExit { reason: ClientExitReason::GracefulShutdown }
        );
    }
}

#[netstack_test]
#[variant(N, Netstack)]
async fn client_provider_double_watch<N: Netstack>(name: &str) {
    let sandbox: netemul::TestSandbox = netemul::TestSandbox::new().unwrap();

    let DhcpTestRealm { client_realm, client_iface, server_realm: _, server_iface: _, _network: _ } =
        &create_test_realm::<N>(&sandbox, name).await;

    let proxy = client_realm.connect_to_protocol::<ClientProviderMarker>().unwrap();
    let client_iface = &client_iface;

    let (client, server_end) = endpoints::create_proxy::<ClientMarker>();
    proxy
        .new_client(
            client_iface.id(),
            &NewClientParams {
                configuration_to_request: None,
                request_ip_address: Some(true),
                ..Default::default()
            },
            server_end,
        )
        .expect("new client");

    let watch_fut_a = client.watch_configuration();
    let watch_fut_b = client.watch_configuration();
    let (result_a, result_b) = join!(watch_fut_a, watch_fut_b);

    assert_matches!(result_a, Err(_));
    assert_matches!(result_b, Err(_));

    let on_exit = client
        .take_event_stream()
        .try_next()
        .now_or_never()
        .expect("event should be already available")
        .expect("event stream should not have ended before yielding exit reason")
        .expect("event stream should not have FIDL error");
    assert_matches!(
        on_exit,
        ClientEvent::OnExit { reason: ClientExitReason::WatchConfigurationAlreadyPending }
    )
}

#[netstack_test]
#[variant(N, Netstack)]
async fn client_provider_shutdown<N: Netstack>(name: &str) {
    let sandbox: netemul::TestSandbox = netemul::TestSandbox::new().unwrap();

    let DhcpTestRealm { client_realm, client_iface, server_realm: _, server_iface: _, _network: _ } =
        &create_test_realm::<N>(&sandbox, name).await;

    let proxy = client_realm.connect_to_protocol::<ClientProviderMarker>().unwrap();
    let client_iface = &client_iface;

    let (client, server_end) = endpoints::create_proxy::<ClientMarker>();
    proxy
        .new_client(
            client_iface.id(),
            &NewClientParams {
                configuration_to_request: None,
                request_ip_address: Some(true),
                ..Default::default()
            },
            server_end,
        )
        .expect("creating new client should succeed");

    let watch_fut = client.watch_configuration();
    let shutdown_fut = async {
        futures_lite::future::yield_now().await;
        client.shutdown().expect("shutdown should not have FIDL error");
    };
    let (watch_result, ()) = join!(watch_fut, shutdown_fut);

    assert_matches!(watch_result, Err(_));

    let on_exit = client
        .take_event_stream()
        .try_next()
        .now_or_never()
        .expect("event should be already available")
        .expect("event stream should not have ended before yielding exit reason")
        .expect("event stream should not have FIDL error");
    assert_matches!(on_exit, ClientEvent::OnExit { reason: ClientExitReason::GracefulShutdown })
}

enum AspServerEnd {
    ServerEnd(fidl::endpoints::ServerEnd<fnet_interfaces_admin::AddressStateProviderMarker>),
    Stream(fnet_interfaces_admin::AddressStateProviderRequestStream),
}

impl AspServerEnd {
    fn into_stream(self) -> fnet_interfaces_admin::AddressStateProviderRequestStream {
        match self {
            AspServerEnd::ServerEnd(server_end) => server_end.into_stream(),
            AspServerEnd::Stream(stream) => stream,
        }
    }
}

impl From<fidl::endpoints::ServerEnd<fnet_interfaces_admin::AddressStateProviderMarker>>
    for AspServerEnd
{
    fn from(
        value: fidl::endpoints::ServerEnd<fnet_interfaces_admin::AddressStateProviderMarker>,
    ) -> Self {
        Self::ServerEnd(value)
    }
}

impl From<fnet_interfaces_admin::AddressStateProviderRequestStream> for AspServerEnd {
    fn from(value: fnet_interfaces_admin::AddressStateProviderRequestStream) -> Self {
        Self::Stream(value)
    }
}

async fn assert_client_shutdown(
    client: fnet_dhcp::ClientProxy,
    address_state_provider: impl Into<AspServerEnd>,
) {
    let address_state_provider = address_state_provider.into();
    let asp_server_fut = async move {
        let request_stream = address_state_provider.into_stream();
        let mut request_stream = pin!(request_stream);

        let control_handle = assert_matches!(
            request_stream.try_next().await.expect("should succeed").expect("should not have ended"),
            fnet_interfaces_admin::AddressStateProviderRequest::Remove { control_handle } => control_handle,
            "client should explicitly remove address on shutdown"
        );
        control_handle
            .send_on_address_removed(fnet_interfaces_admin::AddressRemovalReason::UserRemoved)
            .expect("should succeed");
    };
    let client_fut = async move {
        // Shut down the client so it won't complain about us dropping the AddressStateProvider
        // without sending a terminal event.
        client.shutdown_ext(client.take_event_stream()).await.expect("shutdown should succeed");
    };

    let ((), ()) = join!(asp_server_fut, client_fut);
}

async fn get_watch_address_assignment_state(
    request_stream: &mut fnet_interfaces_admin::AddressStateProviderRequestStream,
) -> fnet_interfaces_admin::AddressStateProviderWatchAddressAssignmentStateResponder {
    assert_matches!(
        request_stream.try_next().await.expect("should succeed").expect("should not have ended"),
        fnet_interfaces_admin::AddressStateProviderRequest::WatchAddressAssignmentState {
            responder
        } => responder,
        "client should watch address assignment state"
    )
}

/// Pull a `WatchAddressAssignmentState` request out of the request stream.
/// Don't respond, but also don't shutdown the responder. This mimics the server
/// side of the protocol "stashing the request".
async fn swallow_watch_address_assignment_state_request(
    request_stream: &mut fnet_interfaces_admin::AddressStateProviderRequestStream,
) {
    let responder = get_watch_address_assignment_state(request_stream).await;
    responder.drop_without_shutdown();
}

#[netstack_test]
#[variant(N, Netstack)]
async fn client_provider_watch_configuration_acquires_lease<N: Netstack>(name: &str) {
    let sandbox: netemul::TestSandbox = netemul::TestSandbox::new().unwrap();
    let test_realm @ DhcpTestRealm {
        client_realm,
        client_iface,
        server_realm: _,
        server_iface: _,
        _network: _,
    } = &create_test_realm::<N>(&sandbox, name).await;

    test_realm.start_dhcp_server(DhcpServerAddress::Primary).await;

    let provider =
        client_realm.connect_to_protocol::<ClientProviderMarker>().expect("connect should succeed");

    let client = provider.new_client_ext(
        client_iface.id().try_into().expect("should be nonzero"),
        fnet_dhcp_ext::default_new_client_params(),
    );

    let config_stream = fnet_dhcp_ext::configuration_stream(client.clone()).fuse();
    let mut config_stream = pin!(config_stream);

    let fnet_dhcp_ext::Configuration { address, dns_servers, routers } = config_stream
        .try_next()
        .await
        .expect("watch configuration should succeed")
        .expect("configuration stream should not have ended");

    assert_eq!(dns_servers, Vec::new());
    assert_eq!(routers, Vec::new());

    let fnet_dhcp_ext::Address { address, address_parameters, address_state_provider } =
        address.expect("address should be present in response");
    assert_eq!(
        address,
        fnet::Ipv4AddressWithPrefix {
            addr: net_types::ip::Ipv4Addr::from(
                server_test_config(DhcpServerAddress::Primary).managed_addrs.pool_range_start
            )
            .into_ext(),
            prefix_len: dhcpv4_helper::DEFAULT_TEST_ADDRESS_POOL_PREFIX_LENGTH.get(),
        }
    );
    let fnet_interfaces_admin::AddressParameters {
        add_subnet_route,
        perform_dad,
        temporary: _,
        initial_properties: _,
        __source_breaking,
    } = address_parameters;
    // DHCP addresses should be added with the corresponding subnet route.
    assert_eq!(add_subnet_route, Some(true));
    // DHCP addresses should have DAD performed before being assigned.
    assert_eq!(perform_dad, Some(true));

    let mut address_state_provider = address_state_provider.into_stream();
    swallow_watch_address_assignment_state_request(&mut address_state_provider).await;
    assert_client_shutdown(client, address_state_provider).await;
}

#[netstack_test]
#[variant(N, Netstack)]
async fn client_explicitly_removes_address_when_lease_expires<N: Netstack>(name: &str) {
    let sandbox: netemul::TestSandbox = netemul::TestSandbox::new().unwrap();
    let test_realm @ DhcpTestRealm {
        client_realm,
        client_iface,
        server_realm: _,
        server_iface: _,
        _network: _,
    } = &create_test_realm::<N>(&sandbox, name).await;

    test_realm
        .start_dhcp_server_with_options(
            DhcpServerAddress::Primary,
            [fnet_dhcp::Parameter::Lease(fnet_dhcp::LeaseLength {
                default: Some(5), // short enough to expire during this test
                ..Default::default()
            })],
            [],
        )
        .await;

    let provider =
        client_realm.connect_to_protocol::<ClientProviderMarker>().expect("connect should succeed");

    let client = provider.new_client_ext(
        client_iface.id().try_into().expect("should be nonzero"),
        fnet_dhcp_ext::default_new_client_params(),
    );

    let config_stream = fnet_dhcp_ext::configuration_stream(client.clone()).fuse();
    let mut config_stream = pin!(config_stream);

    let fnet_dhcp_ext::Configuration { address, dns_servers, routers } = config_stream
        .try_next()
        .await
        .expect("watch configuration should succeed")
        .expect("configuration stream should not have ended");

    assert_eq!(dns_servers, Vec::new());
    assert_eq!(routers, Vec::new());

    let fnet_dhcp_ext::Address { address, address_parameters: _, address_state_provider } =
        address.expect("address should be present in response");
    assert_eq!(
        address,
        fnet::Ipv4AddressWithPrefix {
            addr: net_types::ip::Ipv4Addr::from(
                server_test_config(DhcpServerAddress::Primary).managed_addrs.pool_range_start
            )
            .into_ext(),
            prefix_len: dhcpv4_helper::DEFAULT_TEST_ADDRESS_POOL_PREFIX_LENGTH.get(),
        }
    );

    // Install the address so that the client doesn't error while trying to renew.
    client_iface
        .add_address_and_subnet_route(fnet::Subnet {
            addr: fnet::IpAddress::Ipv4(address.addr),
            prefix_len: address.prefix_len,
        })
        .await
        .expect("should succeed");

    // Inform the client that the address has become assigned. Otherwise it
    // won't try to renew.
    let mut request_stream = address_state_provider.into_stream();
    let responder = get_watch_address_assignment_state(&mut request_stream).await;
    responder
        .send(fnet_interfaces::AddressAssignmentState::Assigned)
        .expect("responding should succeed");
    // Note: the client will have tried to watch the address assignment state
    // again. We don't need to respond, but we can't drop the responder.
    let _responder = get_watch_address_assignment_state(&mut request_stream).await;

    // Stop the DHCP server to prevent it from renewing the lease.
    test_realm.stop_dhcp_server(DhcpServerAddress::Primary).await;

    // The client should fail to renew and have the lease expire, causing it to
    // remove the address.
    let mut request_stream = pin!(request_stream);

    let control_handle = assert_matches!(
        request_stream.try_next().await.expect("should succeed").expect("should not have ended"),
        fnet_interfaces_admin::AddressStateProviderRequest::Remove { control_handle } => control_handle,
        "client should explicitly remove address on shutdown"
    );
    control_handle
        .send_on_address_removed(fnet_interfaces_admin::AddressRemovalReason::UserRemoved)
        .expect("should succeed");
}

#[netstack_test]
#[variant(N, Netstack)]
async fn client_rebinds_same_lease_to_other_server<N: Netstack>(name: &str) {
    let sandbox: netemul::TestSandbox = netemul::TestSandbox::new().unwrap();
    let test_realm @ DhcpTestRealm {
        client_realm,
        client_iface,
        server_realm: _,
        server_iface: _,
        _network: _,
    } = &create_test_realm::<N>(&sandbox, name).await;

    // Have a shorter lease length so that this test fails faster when Rebinding
    // doesn't work or is not implemented.
    const LEASE_LENGTH_SECS: u32 = 20;

    test_realm
        .start_dhcp_server_with_options(
            DhcpServerAddress::Primary,
            [fnet_dhcp::Parameter::Lease(fnet_dhcp::LeaseLength {
                default: Some(LEASE_LENGTH_SECS),
                ..Default::default()
            })],
            // Force the client to start rebinding sooner so that we have more
            // time to complete the rebind (this helps avoid flakes due to CI
            // timing woes).
            [
                // These are not 0 because we still need to give the DHCP server
                // time to stop and then start again with a different address
                // before the client tries to rebind.
                fnet_dhcp::Option_::RenewalTimeValue(LEASE_LENGTH_SECS / 3),
                fnet_dhcp::Option_::RebindingTimeValue(LEASE_LENGTH_SECS / 2),
            ],
        )
        .await;

    let provider =
        client_realm.connect_to_protocol::<ClientProviderMarker>().expect("connect should succeed");

    let client = provider.new_client_ext(
        client_iface.id().try_into().expect("should be nonzero"),
        fnet_dhcp_ext::default_new_client_params(),
    );

    let config_stream = fnet_dhcp_ext::configuration_stream(client.clone()).fuse();
    let mut config_stream = pin!(config_stream);

    let fnet_dhcp_ext::Configuration { address, dns_servers, routers } = config_stream
        .try_next()
        .await
        .expect("watch configuration should succeed")
        .expect("configuration stream should not have ended");

    assert_eq!(dns_servers, Vec::new());
    assert_eq!(routers, Vec::new());

    let fnet_dhcp_ext::Address { address, address_parameters, address_state_provider } =
        address.expect("address should be present in response");
    assert_eq!(
        address,
        fnet::Ipv4AddressWithPrefix {
            addr: net_types::ip::Ipv4Addr::from(
                server_test_config(DhcpServerAddress::Primary).managed_addrs.pool_range_start
            )
            .into_ext(),
            prefix_len: dhcpv4_helper::DEFAULT_TEST_ADDRESS_POOL_PREFIX_LENGTH.get(),
        }
    );
    let fnet_interfaces_admin::AddressParameters { initial_properties, .. } = address_parameters;
    let fnet_interfaces_admin::AddressProperties { valid_lifetime_end, .. } =
        initial_properties.expect("should be set");
    let initial_valid_lifetime_end = valid_lifetime_end.expect("valid_lifetime_end should be set");

    // Install the address so that the client doesn't error while trying to renew.
    client_iface
        .add_address_and_subnet_route(fnet::Subnet {
            addr: fnet::IpAddress::Ipv4(address.addr),
            prefix_len: address.prefix_len,
        })
        .await
        .expect("should succeed");

    // Inform the client that the address has become assigned. Otherwise it
    // won't try to renew.
    let mut request_stream = address_state_provider.into_stream();
    let responder = get_watch_address_assignment_state(&mut request_stream).await;
    responder
        .send(fnet_interfaces::AddressAssignmentState::Assigned)
        .expect("responding should succeed");
    // Note: the client will have tried to watch the address assignment state
    // again. We don't need to respond, but we can't drop the responder.
    let _responder = get_watch_address_assignment_state(&mut request_stream).await;

    // Switch the DHCP server to a different address so that the client can't
    // find it at the old one.
    test_realm
        .switch_dhcp_server_address(DhcpServerAddress::Primary, DhcpServerAddress::Secondary)
        .await;

    // The client should successfully renew without ever removing the address.
    let mut watch_fut = pin!(config_stream.try_for_each(|_config| ready(Ok(()))).fuse());

    let mut renewal_fut =
        pin!({
        let request_stream = &mut request_stream;
        async move {
            let mut request_stream = pin!(request_stream);
            let request = request_stream
                .try_next()
                .await
                .expect("should succeed")
                .expect("should not have ended");
            let (responder, valid_lifetime_end) = assert_matches!(
                request,
                fnet_interfaces_admin::AddressStateProviderRequest::UpdateAddressProperties {
                    address_properties: fnet_interfaces_admin::AddressProperties {
                        valid_lifetime_end: Some(valid_lifetime_end),
                        ..
                    },
                    responder,
                } => (responder, valid_lifetime_end),
                "client should successfully renew and update the address's valid lifetime"
            );
            assert!(
                valid_lifetime_end > initial_valid_lifetime_end,
                "valid lifetime should be extended"
            );
            responder.send().expect("responding to UpdateAddressProperties should succeed");
        }
    }
    .fuse());

    futures::select! {
        result = watch_fut => panic!("watch should not complete: {result:?}"),
        () = renewal_fut => (),
    }

    let shutdown_fut = assert_client_shutdown(client, request_stream);
    let (watch_result, ()) = join!(watch_fut, shutdown_fut);
    assert_matches!(
        watch_result,
        Err(fnet_dhcp_ext::Error::Fidl(fidl::Error::ClientChannelClosed { .. }))
    );
}

const DEBUG_PRINT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

#[netstack_test]
#[variant(N, Netstack)]
async fn watch_configuration_handles_interface_removal<N: Netstack>(name: &str) {
    let sandbox: netemul::TestSandbox = netemul::TestSandbox::new().unwrap();
    let DhcpTestRealm {
        client_realm,
        client_iface,
        server_realm,
        server_iface: _server_iface,
        _network,
    } = create_test_realm::<N>(&sandbox, name).await;

    let provider =
        client_realm.connect_to_protocol::<ClientProviderMarker>().expect("connect should succeed");

    let client = provider.new_client_ext(
        client_iface.id().try_into().expect("should be nonzero"),
        fnet_dhcp_ext::default_new_client_params(),
    );

    let client_fut = pin!(
        async {
            let config_stream = fnet_dhcp_ext::configuration_stream(client.clone()).fuse();
            let mut config_stream = pin!(config_stream);

            let watch_config_result =
                annotate(config_stream.try_next(), DEBUG_PRINT_INTERVAL, "watch_configuration")
                    .await;
            assert_matches!(
                watch_config_result,
                Err(fnet_dhcp_ext::Error::Fidl(fidl::Error::ClientChannelClosed {
                    epitaph: fidl::Epitaph::PeerClosed,
                    ..
                }))
            );

            let fnet_dhcp::ClientEvent::OnExit { reason } = client
                .take_event_stream()
                .try_next()
                .await
                .expect("event stream should not have FIDL error")
                .expect("event stream should not have ended");
            assert_eq!(reason, fnet_dhcp::ClientExitReason::InvalidInterface);
        }
        .fuse()
    );

    let sock = fasync::net::UdpSocket::bind_in_realm(
        &server_realm,
        std::net::SocketAddr::V4(std::net::SocketAddrV4::new(
            std::net::Ipv4Addr::UNSPECIFIED,
            dhcp_protocol::SERVER_PORT.into(),
        )),
    )
    .await
    .expect("bind_in_realm should succeed");
    sock.set_broadcast(true).expect("set_broadcast should succeed");

    let interface_removal_fut = pin!(
        async move {
            // Wait until we see one message from the client before removing the interface.
            let mut buf = [0u8; 1500];
            let (_, client_addr): (usize, std::net::SocketAddr) =
                annotate(sock.recv_from(&mut buf), DEBUG_PRINT_INTERVAL, "recv_from")
                    .await
                    .expect("recv_from should succeed");

            // The client message will be from the unspecified address.
            assert_eq!(
                client_addr,
                std::net::SocketAddr::V4(std::net::SocketAddrV4::new(
                    std::net::Ipv4Addr::UNSPECIFIED,
                    dhcp_protocol::CLIENT_PORT.into()
                ))
            );

            let _ = client_iface.remove_device();
        }
        .fuse()
    );

    let ((), ()) = join!(client_fut, interface_removal_fut);
}

#[netstack_test]
#[variant(N, Netstack)]
#[test_case(
    Some(fnet_interfaces_admin::AddressRemovalReason::AlreadyAssigned),
    None;
    "should decline and restart if already assigned"
)]
#[test_case(
    Some(fnet_interfaces_admin::AddressRemovalReason::DadFailed),
    None;
    "should decline and restart if duplicate address detected"
)]
#[test_case(
    Some(fnet_interfaces_admin::AddressRemovalReason::Invalid),
    None;
    "should decline and restart if Invalid"
)]
#[test_case(
    Some(fnet_interfaces_admin::AddressRemovalReason::InvalidProperties),
    None;
    "should decline and restart if InvalidProperties"
)]
#[test_case(
    Some(fnet_interfaces_admin::AddressRemovalReason::UserRemoved),
    Some(fnet_dhcp::ClientExitReason::AddressRemovedByUser);
    "should stop client if user removed"
)]
#[test_case(
    Some(fnet_interfaces_admin::AddressRemovalReason::InterfaceRemoved),
    Some(fnet_dhcp::ClientExitReason::InvalidInterface);
    "should stop client if interface removed"
)]
#[test_case(
    None,
    Some(fnet_dhcp::ClientExitReason::AddressStateProviderError);
    "should stop client if address removed with no terminal event"
)]
async fn client_handles_address_removal<N: Netstack>(
    name: &str,
    removal_reason: Option<fnet_interfaces_admin::AddressRemovalReason>,
    expected_exit_reason: Option<fnet_dhcp::ClientExitReason>,
) {
    let sandbox: netemul::TestSandbox = netemul::TestSandbox::new().unwrap();
    let test_realm = create_test_realm::<N>(&sandbox, name).await;

    test_realm.start_dhcp_server(DhcpServerAddress::Primary).await;

    let DhcpTestRealm {
        client_realm,
        client_iface,
        server_realm: _server_realm,
        server_iface: _server_iface,
        _network,
    } = test_realm;

    let provider =
        client_realm.connect_to_protocol::<ClientProviderMarker>().expect("connect should succeed");

    let client = provider.new_client_ext(
        client_iface.id().try_into().expect("should be nonzero"),
        fnet_dhcp_ext::default_new_client_params(),
    );

    let config_stream = fnet_dhcp_ext::configuration_stream(client.clone()).fuse();
    let mut config_stream = pin!(config_stream);

    let fnet_dhcp_ext::Configuration { address, dns_servers, routers } = config_stream
        .try_next()
        .await
        .expect("watch configuration should succeed")
        .expect("configuration stream should not have ended");

    assert_eq!(dns_servers, Vec::new());
    assert_eq!(routers, Vec::new());

    let fnet_dhcp_ext::Address { address, address_parameters: _, address_state_provider } =
        address.expect("address should be present in response");
    assert_eq!(
        address,
        fnet::Ipv4AddressWithPrefix {
            addr: net_types::ip::Ipv4Addr::from(
                server_test_config(DhcpServerAddress::Primary).managed_addrs.pool_range_start
            )
            .into_ext(),
            prefix_len: dhcpv4_helper::DEFAULT_TEST_ADDRESS_POOL_PREFIX_LENGTH.get(),
        }
    );

    {
        let (_asp_request_stream, asp_control_handle) =
            address_state_provider.into_stream_and_control_handle();
        // NB: We're acting as the AddressStateProvider server_end.
        // Send the removal reason and close the channel to indicate the address
        // is being removed.
        if let Some(removal_reason) = removal_reason {
            asp_control_handle
                .send_on_address_removed(removal_reason)
                .expect("send removed event should succeed");

            if removal_reason == fnet_interfaces_admin::AddressRemovalReason::InterfaceRemoved {
                let _: (
                    fnetemul_network::EndpointProxy,
                    Option<fnet_interfaces_admin::DeviceControlProxy>,
                ) = client_iface.remove().await.expect("interface removal should succeed");
            }
        }
    }

    if let Some(want_reason) = expected_exit_reason {
        assert_matches!(
            config_stream.try_next().await,
            Err(fnet_dhcp_ext::Error::Fidl(fidl::Error::ClientChannelClosed {
                epitaph: fidl::Epitaph::PeerClosed,
                ..
            }))
        );
        let terminal_event = client
            .take_event_stream()
            .try_next()
            .await
            .expect("should not have error on event stream")
            .expect("event stream should not have ended");
        let got_reason = assert_matches!(
            terminal_event,
            fnet_dhcp::ClientEvent::OnExit {
                reason
            } => reason
        );
        assert_eq!(got_reason, want_reason);
        return;
    }

    // After restarting, if the client has not stopped, we should get a new lease.
    let fnet_dhcp_ext::Configuration { address, dns_servers: _, routers: _ } = config_stream
        .try_next()
        .await
        .expect("watch configuration should succeed")
        .expect("configuration stream should not have ended");

    let fnet_dhcp_ext::Address { address, address_parameters: _, address_state_provider } =
        address.expect("address should be present in response");
    assert_eq!(
        address,
        fnet::Ipv4AddressWithPrefix {
            addr: net_types::ip::Ipv4Addr::from(std::net::Ipv4Addr::from(
                u32::from(
                    server_test_config(DhcpServerAddress::Primary).managed_addrs.pool_range_start
                ) + 1
            ))
            .into_ext(),
            prefix_len: dhcpv4_helper::DEFAULT_TEST_ADDRESS_POOL_PREFIX_LENGTH.get(),
        }
    );

    let mut address_state_provider = address_state_provider.into_stream();
    swallow_watch_address_assignment_state_request(&mut address_state_provider).await;
    assert_client_shutdown(client, address_state_provider).await;
}

#[fuchsia::test]
async fn inspect_with_lease_acquired() {
    let name = "inspect_with_lease_acquired";
    let sandbox: netemul::TestSandbox = netemul::TestSandbox::new().unwrap();
    let test_realm @ DhcpTestRealm {
        client_realm,
        client_iface,
        server_realm: _,
        server_iface: _,
        _network: _,
    } = &create_test_realm::<Netstack3>(&sandbox, name).await;

    test_realm.start_dhcp_server(DhcpServerAddress::Primary).await;

    let provider =
        client_realm.connect_to_protocol::<ClientProviderMarker>().expect("connect should succeed");

    let client = provider.new_client_ext(
        client_iface.id().try_into().expect("should be nonzero"),
        fnet_dhcp_ext::default_new_client_params(),
    );

    let config_stream = fnet_dhcp_ext::configuration_stream(client.clone()).fuse();
    let mut config_stream = pin!(config_stream);

    let fnet_dhcp_ext::Configuration { address, dns_servers, routers } = config_stream
        .try_next()
        .await
        .expect("watch configuration should succeed")
        .expect("configuration stream should not have ended");

    assert_eq!(dns_servers, Vec::new());
    assert_eq!(routers, Vec::new());

    let fnet_dhcp_ext::Address { address, address_parameters: _, address_state_provider } =
        address.expect("address should be present in response");
    assert_eq!(
        address,
        fnet::Ipv4AddressWithPrefix {
            addr: net_types::ip::Ipv4Addr::from(
                server_test_config(DhcpServerAddress::Primary).managed_addrs.pool_range_start
            )
            .into_ext(),
            prefix_len: dhcpv4_helper::DEFAULT_TEST_ADDRESS_POOL_PREFIX_LENGTH.get(),
        }
    );

    let data = netstack_testing_common::get_inspect_data(client_realm, "dhcp-client", "root")
        .await
        .expect("should successfully retrieve inspect");
    // Debug print the tree to make debugging easier in case of failures.
    println!("Got inspect data: {:#?}", data);

    diagnostics_assertions::assert_data_tree!(data, "root": {
        Clients: {
            client_iface.id().to_string() => {
                InterfaceId: client_iface.id(),
                Counters: {
                    Init: {
                        Entered: 0u64,
                    },
                    Selecting: {
                        Entered: 1u64,
                        IllegallyIncludedOption: 0u64,
                        MissingRequiredOption: 0u64,
                        NoLeaseTime: 0u64,
                        NoServerIdentifier: 0u64,
                        NotBootReply: 0u64,
                        NotDhcpOffer: 0u64,
                        ParserMissingField: 0u64,
                        RebindingTimeOutlivesLease: 0u64,
                        RecvFailedDhcpParse: 0u64,
                        RecvMessage: 1u64,
                        RecvMessageFatalSocketError: 0u64,
                        RecvMessageNonFatalSocketError: 0u64,
                        RecvTimeOut: 0u64,
                        RecvWrongChaddr: 0u64,
                        RecvWrongXid: 0u64,
                        RenewalTimeAfterRebindingTime: 0u64,
                        SendMessage: 1u64,
                        UnspecifiedServerIdentifier: 0u64,
                        UnspecifiedYiaddr: 0u64,
                    },
                    Requesting: {
                        Entered: 1u64,
                        IllegallyIncludedOption: 0u64,
                        MissingRequiredOption: 0u64,
                        NoLeaseTime: 0u64,
                        NoServerIdentifier: 0u64,
                        NotBootReply: 0u64,
                        NotDhcpAckOrNak: 0u64,
                        ParserMissingField: 0u64,
                        RebindingTimeOutlivesLease: 0u64,
                        RecvFailedDhcpParse: 0u64,
                        RecvMessage: 1u64,
                        RecvMessageFatalSocketError: 0u64,
                        RecvMessageNonFatalSocketError: 0u64,
                        RecvNak: 0u64,
                        RecvTimeOut: 0u64,
                        RecvWrongChaddr: 0u64,
                        RecvWrongXid: 0u64,
                        RenewalTimeAfterRebindingTime: 0u64,
                        SendMessage: 1u64,
                        UnspecifiedServerIdentifier: 0u64,
                        UnspecifiedYiaddr: 0u64,
                    },
                    Bound: {
                        Entered: 1u64,
                        Assigned: 0u64,
                    },
                    Renewing: {
                        Entered: 0u64,
                        IllegallyIncludedOption: 0u64,
                        MissingRequiredOption: 0u64,
                        NoLeaseTime: 0u64,
                        NoServerIdentifier: 0u64,
                        NotBootReply: 0u64,
                        NotDhcpAckOrNak: 0u64,
                        ParserMissingField: 0u64,
                        RebindingTimeOutlivesLease: 0u64,
                        RecvFailedDhcpParse: 0u64,
                        RecvMessage: 0u64,
                        RecvMessageFatalSocketError: 0u64,
                        RecvMessageNonFatalSocketError: 0u64,
                        RecvNak: 0u64,
                        RecvTimeOut: 0u64,
                        RecvWrongChaddr: 0u64,
                        RecvWrongXid: 0u64,
                        RenewalTimeAfterRebindingTime: 0u64,
                        SendMessage: 0u64,
                        UnspecifiedServerIdentifier: 0u64,
                        UnspecifiedYiaddr: 0u64,
                    },
                    Rebinding: {
                        Entered: 0u64,
                        IllegallyIncludedOption: 0u64,
                        MissingRequiredOption: 0u64,
                        NoLeaseTime: 0u64,
                        NoServerIdentifier: 0u64,
                        NotBootReply: 0u64,
                        NotDhcpAckOrNak: 0u64,
                        ParserMissingField: 0u64,
                        RebindingTimeOutlivesLease: 0u64,
                        RecvFailedDhcpParse: 0u64,
                        RecvMessage: 0u64,
                        RecvMessageFatalSocketError: 0u64,
                        RecvMessageNonFatalSocketError: 0u64,
                        RecvNak: 0u64,
                        RecvTimeOut: 0u64,
                        RecvWrongChaddr: 0u64,
                        RecvWrongXid: 0u64,
                        RenewalTimeAfterRebindingTime: 0u64,
                        SendMessage: 0u64,
                        UnspecifiedServerIdentifier: 0u64,
                        UnspecifiedYiaddr: 0u64,
                    },
                    WaitingToRestart: {
                        Entered: 0u64,
                    },
                },
                CurrentLease: {
                    PrefixLen: 25u64,
                    "Start@time": AnyUintProperty,
                    Routers: {},
                    LeaseLengthSecs: 86400u64,
                    "Renewed@time": "None",
                    DnsServers: {},
                    IpAddress: "192.168.0.3",
                },
                CurrentState: {
                    "Entered@time": AnyUintProperty,
                    State: {
                        Kind: "Bound and awaiting assignment",
                        IpAddressLeaseTimeSecs: 86400u64,
                        ServerIdentifier: "192.168.0.1",
                        "Start@time": AnyUintProperty,
                        RenewalTimeSecs: 43200u64,
                        RebindingTimeSecs: 64800u64,
                        Xid: AnyUintProperty,
                        Yiaddr: "192.168.0.3",
                    }
                },
                StateHistory: {
                    "2": {
                        "Entered@time": AnyUintProperty,
                        State: "Requesting",
                    },
                    "1": {
                        "Entered@time": AnyUintProperty,
                        State: "Selecting",
                    },
                    "0": {
                        "Entered@time": AnyUintProperty,
                        State: "Init",
                    },
                },
                LeaseHistory: {},
            }
        }
    });

    let mut address_state_provider = address_state_provider.into_stream();
    swallow_watch_address_assignment_state_request(&mut address_state_provider).await;
    assert_client_shutdown(client, address_state_provider).await;
}

/// A regression test for https://fxbug.dev/445178160.
///
/// Verify that if the DHCP server sends the `Router` or `DomainNameServer`
/// option multiple times, the client will merge the routers into a single
/// cohesive list.
///
/// Because Fuchsia's DHCP server implementation disallows sending the same
/// option multiple times, we instead impersonate a DHCP server by writing
/// messages directly to the network.
#[netstack_test]
#[variant(N, Netstack)]
async fn client_gracefully_handles_duplicate_options<N: Netstack>(name: &str) {
    let sandbox: netemul::TestSandbox = netemul::TestSandbox::new().unwrap();
    let DhcpTestRealm { client_realm, client_iface, server_realm, server_iface, _network: _ } =
        &create_test_realm::<N>(&sandbox, name).await;

    const SERVER_ADDR: std::net::Ipv4Addr = std_ip_v4!("192.168.0.1");
    const CLIENT_ADDR: std::net::Ipv4Addr = std_ip_v4!("192.168.0.2");
    const PREFIX_LEN: u8 = 24;
    const ROUTER1_ADDR: std::net::Ipv4Addr = std_ip_v4!("192.168.0.3");
    const ROUTER2_ADDR: std::net::Ipv4Addr = std_ip_v4!("192.168.0.4");
    const ROUTER3_ADDR: std::net::Ipv4Addr = std_ip_v4!("192.168.0.5");

    // We're going to drive the DHCP protocol ourselves, so don't start the DHCP
    // server. Instead add the address and create a socket.
    server_iface
        .add_address_and_subnet_route(fnet::Subnet {
            addr: fnet_ext::IpAddress(SERVER_ADDR.into()).into(),
            prefix_len: PREFIX_LEN,
        })
        .await
        .expect("add address should succeed");
    let server_sock = fuchsia_async::net::UdpSocket::bind_in_realm(
        &server_realm,
        std::net::SocketAddr::new(
            std::net::Ipv4Addr::UNSPECIFIED.into(),
            dhcp_protocol::SERVER_PORT.get(),
        ),
    )
    .await
    .expect("failed to create server socket");

    // NB: The real DHCP server uses packet sockets to avoid neighbor resolution,
    // but for the purpose of the test it's far easier to add a static neighbor
    // entry.
    server_realm
        .add_neighbor_entry(
            server_iface.id(),
            fnet_ext::IpAddress(CLIENT_ADDR.into()).into(),
            fnet::MacAddress { octets: MAC.bytes() },
        )
        .await
        .expect("failed to add static neighbor entry");

    // Now, start the client.
    let provider =
        client_realm.connect_to_protocol::<ClientProviderMarker>().expect("connect should succeed");

    let client = provider.new_client_ext(
        client_iface.id().try_into().expect("should be nonzero"),
        fnet_dhcp_ext::default_new_client_params(),
    );

    let config_stream = fnet_dhcp_ext::configuration_stream(client.clone()).fuse();
    let mut config_stream = pin!(config_stream);

    // Wait to observe the DHCPDISCOVER from the client.
    let mut buf = vec![0; 1024];
    let (len, _from) = server_sock.recv_from(&mut buf).await.expect("server receive failed");
    let discover =
        dhcp_protocol::Message::from_buffer(&buf[..len]).expect("failed to parse DHCP message");
    assert_eq!(discover.get_dhcp_type(), Ok(dhcp_protocol::MessageType::DHCPDISCOVER));

    // Send a DHCPOffer to the client.
    let offer = dhcp_protocol::Message {
        op: dhcp_protocol::OpCode::BOOTREPLY,
        xid: discover.xid,
        secs: 0,
        bdcast_flag: false,
        ciaddr: std::net::Ipv4Addr::UNSPECIFIED,
        yiaddr: CLIENT_ADDR,
        siaddr: std::net::Ipv4Addr::UNSPECIFIED,
        giaddr: std::net::Ipv4Addr::UNSPECIFIED,
        chaddr: MAC,
        sname: BString::default(),
        file: BString::default(),
        options: vec![
            // NB: these options are required.
            dhcp_protocol::DhcpOption::DhcpMessageType(dhcp_protocol::MessageType::DHCPOFFER),
            dhcp_protocol::DhcpOption::ServerIdentifier(SERVER_ADDR),
            dhcp_protocol::DhcpOption::SubnetMask(
                net_types::ip::PrefixLength::new(PREFIX_LEN).unwrap(),
            ),
        ],
    };
    let offer = offer.serialize();
    let sent_len = server_sock
        .send_to(
            &offer,
            std::net::SocketAddr::new(CLIENT_ADDR.into(), dhcp_protocol::CLIENT_PORT.get()),
        )
        .await
        .expect("failed to send DHCPOFFER");
    assert_eq!(sent_len, offer.len());

    // Wait to see a DHCPREQUEST from the client.
    let request = loop {
        let (len, _from) = server_sock.recv_from(&mut buf).await.expect("server receive failed");
        let request =
            dhcp_protocol::Message::from_buffer(&buf[..len]).expect("failed to parse DHCP message");
        match request.get_dhcp_type().expect("get DHCP message type") {
            // Ignore DHCPDISCOVERs which may have been retransmitted by the client.
            dhcp_protocol::MessageType::DHCPDISCOVER => continue,
            dhcp_protocol::MessageType::DHCPREQUEST => break request,
            dhcp_type => panic!("unexpected DHCP message type: {dhcp_type}"),
        }
    };

    // Send a DHCPACK to the client.
    let ack = dhcp_protocol::Message {
        op: dhcp_protocol::OpCode::BOOTREPLY,
        xid: request.xid,
        secs: 0,
        bdcast_flag: false,
        ciaddr: std::net::Ipv4Addr::UNSPECIFIED,
        yiaddr: CLIENT_ADDR,
        siaddr: std::net::Ipv4Addr::UNSPECIFIED,
        giaddr: std::net::Ipv4Addr::UNSPECIFIED,
        chaddr: MAC,
        sname: BString::default(),
        file: BString::default(),
        options: vec![
            // NB: these options are required.
            dhcp_protocol::DhcpOption::DhcpMessageType(dhcp_protocol::MessageType::DHCPACK),
            dhcp_protocol::DhcpOption::ServerIdentifier(SERVER_ADDR),
            dhcp_protocol::DhcpOption::SubnetMask(
                net_types::ip::PrefixLength::new(PREFIX_LEN).unwrap(),
            ),
            dhcp_protocol::DhcpOption::IpAddressLeaseTime(3600),
            // Send two sets of routers that overlap.
            dhcp_protocol::DhcpOption::Router(
                dhcp_protocol::AtLeast::try_from(
                    dhcp_protocol::AtMostBytes::try_from(vec![ROUTER1_ADDR, ROUTER2_ADDR]).unwrap(),
                )
                .unwrap(),
            ),
            dhcp_protocol::DhcpOption::Router(
                dhcp_protocol::AtLeast::try_from(
                    dhcp_protocol::AtMostBytes::try_from(vec![ROUTER2_ADDR, ROUTER3_ADDR]).unwrap(),
                )
                .unwrap(),
            ),
            // Send two sets of DNS servers that overlap.
            dhcp_protocol::DhcpOption::DomainNameServer(
                dhcp_protocol::AtLeast::try_from(
                    dhcp_protocol::AtMostBytes::try_from(vec![ROUTER3_ADDR, ROUTER2_ADDR]).unwrap(),
                )
                .unwrap(),
            ),
            dhcp_protocol::DhcpOption::DomainNameServer(
                dhcp_protocol::AtLeast::try_from(
                    dhcp_protocol::AtMostBytes::try_from(vec![ROUTER2_ADDR, ROUTER1_ADDR]).unwrap(),
                )
                .unwrap(),
            ),
        ],
    };
    let ack = ack.serialize();
    let sent_len = server_sock
        .send_to(
            &ack,
            std::net::SocketAddr::new(CLIENT_ADDR.into(), dhcp_protocol::CLIENT_PORT.get()),
        )
        .await
        .expect("failed to send DHCPACK");
    assert_eq!(sent_len, ack.len());

    // Expect the client to produce a configuration.
    let fnet_dhcp_ext::Configuration { address, dns_servers, routers } = config_stream
        .try_next()
        .await
        .expect("watch configuration should succeed")
        .expect("configuration stream should not have ended");

    // The client should have consolidated the two lists of DNS servers.
    assert_eq!(
        dns_servers,
        vec![
            fnet_ext::Ipv4Address(ROUTER3_ADDR).into(),
            fnet_ext::Ipv4Address(ROUTER2_ADDR).into(),
            fnet_ext::Ipv4Address(ROUTER1_ADDR).into(),
        ]
    );

    // The client should have consolidated the two lists of routers.
    assert_eq!(
        routers,
        vec![
            net_types::SpecifiedAddr::new(ROUTER1_ADDR.into()).unwrap(),
            net_types::SpecifiedAddr::new(ROUTER2_ADDR.into()).unwrap(),
            net_types::SpecifiedAddr::new(ROUTER3_ADDR.into()).unwrap(),
        ]
    );

    let fnet_dhcp_ext::Address { address, address_parameters, address_state_provider } =
        address.expect("address should be present in response");
    assert_eq!(
        address,
        fnet::Ipv4AddressWithPrefix {
            addr: fnet_ext::Ipv4Address(CLIENT_ADDR).into(),
            prefix_len: PREFIX_LEN
        }
    );
    let fnet_interfaces_admin::AddressParameters {
        add_subnet_route,
        perform_dad,
        temporary: _,
        initial_properties: _,
        __source_breaking,
    } = address_parameters;
    // DHCP addresses should be added with the corresponding subnet route.
    assert_eq!(add_subnet_route, Some(true));
    // DHCP addresses should have DAD performed before being assigned.
    assert_eq!(perform_dad, Some(true));

    let mut address_state_provider = address_state_provider.into_stream();
    swallow_watch_address_assignment_state_request(&mut address_state_provider).await;
    assert_client_shutdown(client, address_state_provider).await;
}

/// A regression test for https://fxbug.dev/493882212.
///
/// Verify that if the DHCP server sends an illegal option as part of it's
/// DHCPOFFER, or DHCPACK, the client will ignore the option and continue
/// to process the message.
///
/// Because Fuchsia's DHCP server implementation disallows sending illegal
/// options, we instead impersonate a DHCP server by writing messages directly
/// to the network.
#[netstack_test]
#[variant(N, Netstack)]
async fn client_gracefully_handles_illegal_options<N: Netstack>(name: &str) {
    let sandbox: netemul::TestSandbox = netemul::TestSandbox::new().unwrap();
    let DhcpTestRealm { client_realm, client_iface, server_realm, server_iface, _network: _ } =
        &create_test_realm::<N>(&sandbox, name).await;

    const SERVER_ADDR: std::net::Ipv4Addr = std_ip_v4!("192.168.0.1");
    const CLIENT_ADDR: std::net::Ipv4Addr = std_ip_v4!("192.168.0.2");
    const PREFIX_LEN: u8 = 24;

    let common_options = vec![
        // NB: The DHCP server is required to provide these options.
        dhcp_protocol::DhcpOption::ServerIdentifier(SERVER_ADDR),
        dhcp_protocol::DhcpOption::SubnetMask(
            net_types::ip::PrefixLength::new(PREFIX_LEN).unwrap(),
        ),
        // Options that DHCP server's MUST NOT send, according to RFC 2131.
        // We expect our client to ignore these.
        dhcp_protocol::DhcpOption::RequestedIpAddress(CLIENT_ADDR),
        dhcp_protocol::DhcpOption::ParameterRequestList(
            dhcp_protocol::AtLeast::try_from(
                dhcp_protocol::AtMostBytes::try_from(vec![
                    dhcp_protocol::OptionCode::ServerIdentifier,
                ])
                .unwrap(),
            )
            .unwrap(),
        ),
        dhcp_protocol::DhcpOption::MaxDhcpMessageSize(1000),
    ];

    // We're going to drive the DHCP protocol ourselves, so don't start the DHCP
    // server. Instead add the address and create a socket.
    server_iface
        .add_address_and_subnet_route(fnet::Subnet {
            addr: fnet_ext::IpAddress(SERVER_ADDR.into()).into(),
            prefix_len: PREFIX_LEN,
        })
        .await
        .expect("add address should succeed");
    let server_sock = fuchsia_async::net::UdpSocket::bind_in_realm(
        &server_realm,
        std::net::SocketAddr::new(
            std::net::Ipv4Addr::UNSPECIFIED.into(),
            dhcp_protocol::SERVER_PORT.get(),
        ),
    )
    .await
    .expect("failed to create server socket");

    // NB: The real DHCP server uses packet sockets to avoid neighbor resolution,
    // but for the purpose of the test it's far easier to add a static neighbor
    // entry.
    server_realm
        .add_neighbor_entry(
            server_iface.id(),
            fnet_ext::IpAddress(CLIENT_ADDR.into()).into(),
            fnet::MacAddress { octets: MAC.bytes() },
        )
        .await
        .expect("failed to add static neighbor entry");

    // Now, start the client.
    let provider =
        client_realm.connect_to_protocol::<ClientProviderMarker>().expect("connect should succeed");

    let client = provider.new_client_ext(
        client_iface.id().try_into().expect("should be nonzero"),
        fnet_dhcp_ext::default_new_client_params(),
    );

    let config_stream = fnet_dhcp_ext::configuration_stream(client.clone()).fuse();
    let mut config_stream = pin!(config_stream);

    // Wait to observe the DHCPDISCOVER from the client.
    let mut buf = vec![0; 1024];
    let (len, _from) = server_sock.recv_from(&mut buf).await.expect("server receive failed");
    let discover =
        dhcp_protocol::Message::from_buffer(&buf[..len]).expect("failed to parse DHCP message");
    assert_eq!(discover.get_dhcp_type(), Ok(dhcp_protocol::MessageType::DHCPDISCOVER));

    // Send a DHCPOffer to the client.
    let mut offer_options = common_options.clone();
    offer_options
        .push(dhcp_protocol::DhcpOption::DhcpMessageType(dhcp_protocol::MessageType::DHCPOFFER));
    let offer = dhcp_protocol::Message {
        op: dhcp_protocol::OpCode::BOOTREPLY,
        xid: discover.xid,
        secs: 0,
        bdcast_flag: false,
        ciaddr: std::net::Ipv4Addr::UNSPECIFIED,
        yiaddr: CLIENT_ADDR,
        siaddr: std::net::Ipv4Addr::UNSPECIFIED,
        giaddr: std::net::Ipv4Addr::UNSPECIFIED,
        chaddr: MAC,
        sname: BString::default(),
        file: BString::default(),
        options: offer_options,
    };
    let offer = offer.serialize();
    let sent_len = server_sock
        .send_to(
            &offer,
            std::net::SocketAddr::new(CLIENT_ADDR.into(), dhcp_protocol::CLIENT_PORT.get()),
        )
        .await
        .expect("failed to send DHCPOFFER");
    assert_eq!(sent_len, offer.len());

    // Wait to see a DHCPREQUEST from the client.
    let request = loop {
        let (len, _from) = server_sock.recv_from(&mut buf).await.expect("server receive failed");
        let request =
            dhcp_protocol::Message::from_buffer(&buf[..len]).expect("failed to parse DHCP message");
        match request.get_dhcp_type().expect("get DHCP message type") {
            // Ignore DHCPDISCOVERs which may have been retransmitted by the client.
            dhcp_protocol::MessageType::DHCPDISCOVER => continue,
            dhcp_protocol::MessageType::DHCPREQUEST => break request,
            dhcp_type => panic!("unexpected DHCP message type: {dhcp_type}"),
        }
    };

    // Send a DHCPACK to the client.
    let mut ack_options = common_options;
    ack_options
        .push(dhcp_protocol::DhcpOption::DhcpMessageType(dhcp_protocol::MessageType::DHCPACK));
    let ack = dhcp_protocol::Message {
        op: dhcp_protocol::OpCode::BOOTREPLY,
        xid: request.xid,
        secs: 0,
        bdcast_flag: false,
        ciaddr: std::net::Ipv4Addr::UNSPECIFIED,
        yiaddr: CLIENT_ADDR,
        siaddr: std::net::Ipv4Addr::UNSPECIFIED,
        giaddr: std::net::Ipv4Addr::UNSPECIFIED,
        chaddr: MAC,
        sname: BString::default(),
        file: BString::default(),
        options: ack_options,
    };
    let ack = ack.serialize();
    let sent_len = server_sock
        .send_to(
            &ack,
            std::net::SocketAddr::new(CLIENT_ADDR.into(), dhcp_protocol::CLIENT_PORT.get()),
        )
        .await
        .expect("failed to send DHCPACK");
    assert_eq!(sent_len, ack.len());

    // Expect the client to produce a configuration.
    let fnet_dhcp_ext::Configuration { address, dns_servers: _, routers: _ } = config_stream
        .try_next()
        .await
        .expect("watch configuration should succeed")
        .expect("configuration stream should not have ended");

    let fnet_dhcp_ext::Address { address, address_parameters: _, address_state_provider } =
        address.expect("address should be present in response");
    assert_eq!(
        address,
        fnet::Ipv4AddressWithPrefix {
            addr: fnet_ext::Ipv4Address(CLIENT_ADDR).into(),
            prefix_len: PREFIX_LEN
        }
    );

    let mut address_state_provider = address_state_provider.into_stream();
    swallow_watch_address_assignment_state_request(&mut address_state_provider).await;
    assert_client_shutdown(client, address_state_provider).await;
}
