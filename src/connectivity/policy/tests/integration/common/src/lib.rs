// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::pin::pin;

use fidl::endpoints::ProtocolMarker;
use fidl_fuchsia_net_interfaces as fnet_interfaces;
use fidl_fuchsia_net_resources as fnet_resources;
use fidl_fuchsia_net_routes as fnet_routes;
use fidl_fuchsia_net_routes_ext::admin::FidlRouteAdminIpExt;
use fidl_fuchsia_net_routes_ext::{self as fnet_routes_ext, FidlRouteIpExt};
use fidl_fuchsia_netemul_network as fnetemul_network;
use futures::future::{FutureExt as _, LocalBoxFuture};
use net_types::ip::Ip;
use netemul::{TestEndpoint, TestNetwork, TestRealm};
use netstack_testing_common::realms::{
    KnownServiceProvider, Manager, ManagerConfig, Netstack, SocketProxyType, TestRealmExt,
    TestSandboxExt, constants,
};
use netstack_testing_common::{
    ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT, interfaces, wait_for_component_stopped,
};

#[derive(Default)]
pub struct NetcfgOwnedDeviceArgs {
    // Whether to use the out of stack DHCP client.
    pub use_out_of_stack_dhcp_client: bool,
    // Whether to include the socketproxy protocols in netcfg.
    pub socket_proxy_type: SocketProxyType,
    // Additional service providers to include in the realm.
    pub extra_known_service_providers: Vec<KnownServiceProvider>,
}

/// Initialize a realm with a device that is owned by netcfg.
/// The device is discovered through devfs and installed into
/// the Netstack via netcfg. `after_interface_up` is called
/// once the interface has been discovered via the Netstack
/// interfaces watcher.
pub async fn with_netcfg_owned_device<
    M: Manager,
    N: Netstack,
    F: for<'a> FnOnce(
        u64,
        &'a netemul::TestNetwork<'a>,
        &'a fnet_interfaces::StateProxy,
        &'a netemul::TestRealm<'a>,
        &'a netemul::TestSandbox,
    ) -> LocalBoxFuture<'a, ()>,
>(
    name: &str,
    manager_config: ManagerConfig,
    additional_args: NetcfgOwnedDeviceArgs,
    after_interface_up: F,
) -> String {
    let NetcfgOwnedDeviceArgs {
        use_out_of_stack_dhcp_client,
        socket_proxy_type,
        extra_known_service_providers,
    } = additional_args;
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox
        .create_netstack_realm_with::<N, _, _>(
            name,
            [
                KnownServiceProvider::Manager {
                    agent: M::MANAGEMENT_AGENT,
                    use_dhcp_server: false,
                    use_out_of_stack_dhcp_client,
                    socket_proxy_type,
                    config: manager_config,
                },
                KnownServiceProvider::DnsResolver,
                KnownServiceProvider::FakeClock,
            ]
            .into_iter()
            .chain(extra_known_service_providers)
            // If the client requested an out of stack DHCP client or to use
            // the socket proxy, add them to the list of service providers.
            .chain(
                use_out_of_stack_dhcp_client
                    .then_some(KnownServiceProvider::DhcpClient)
                    .into_iter(),
            )
            .chain(socket_proxy_type.known_service_provider().into_iter()),
        )
        .expect("create netstack realm");
    // Add a device to the realm.
    let network = sandbox.create_network(name).await.expect("create network");
    let _endpoint = add_device_to_devfs::<M>(&network, &realm, name.to_string(), None).await;

    // Make sure the Netstack got the new device added.
    let (interface_state, if_id, if_name) = verify_interface_added::<M>(&realm).await;

    after_interface_up(if_id, &network, &interface_state, &realm, &sandbox).await;

    // Wait for orderly shutdown of the test realm to complete before allowing
    // test interfaces to be cleaned up.
    //
    // This is necessary to prevent test interfaces from being removed while
    // NetCfg is still in the process of configuring them after adding them to
    // the Netstack, which causes spurious errors.
    realm.shutdown().await.expect("failed to shutdown realm");

    if_name
}

/// Add a device into devfs to be fully managed by netcfg. Netcfg will discover
/// and install the device into the Netstack. We cannot assume that netcfg has
/// observed and installed the device as a result of this function, that
/// condition must be observed via the interfaces watcher.
///
/// Returns the new endpoint that was added to the realm.
pub async fn add_device_to_devfs<'a, M: Manager>(
    network: &'a TestNetwork<'a>,
    realm: &'a TestRealm<'a>,
    name: String,
    endpoint_config: Option<fnetemul_network::EndpointConfig>,
) -> TestEndpoint<'a> {
    let endpoint = match endpoint_config {
        Some(config) => network.create_endpoint_with(name, config).await,
        None => network.create_endpoint(name).await,
    }
    .expect("create endpoint");
    endpoint.set_link_up(true).await.expect("set link up");
    let endpoint_mount_path = netemul::devfs_device_path("ep");
    let endpoint_mount_path = endpoint_mount_path.as_path();
    realm.add_virtual_device(&endpoint, endpoint_mount_path).await.unwrap_or_else(|e| {
        panic!("add virtual device {}: {:?}", endpoint_mount_path.display(), e)
    });
    endpoint
}

/// Observe a new non-loopback interface via Netstack's interface watcher.
///
/// Returns the interface state proxy and the interface's id and name.
pub async fn verify_interface_added<'a, M: Manager>(
    realm: &'a TestRealm<'a>,
) -> (fnet_interfaces::StateProxy, u64, String) {
    let interface_state = realm
        .connect_to_protocol::<fnet_interfaces::StateMarker>()
        .expect("connect to fuchsia.net.interfaces/State service");
    let wait_for_netmgr =
        wait_for_component_stopped(&realm, constants::netcfg::COMPONENT_NAME, None).fuse();
    let mut wait_for_netmgr = pin!(wait_for_netmgr);
    let (if_id, if_name): (u64, String) = interfaces::wait_for_non_loopback_interface_up(
        &interface_state,
        &mut wait_for_netmgr,
        None,
        ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT,
    )
    .await
    .expect("wait for non loopback interface");
    (interface_state, if_id, if_name.clone())
}

/// Add a default route to the provided interface by id. Must not be called
/// more than once for the same interface.
///
/// Returns whether the route was added to the RouteSet.
pub async fn add_default_route<'a, I>(
    realm: &'a TestRealm<'a>,
    if_id: u64,
    route_set: &<I::RouteSetMarker as ProtocolMarker>::Proxy,
) -> bool
where
    I: Ip + FidlRouteAdminIpExt + FidlRouteIpExt,
{
    // Acquire the control for the interface so we can directly add a default route to the
    // interface without going through DHCP. This meets the condition for the interface to
    // be added to Fuchsia's NetworkRegistry and set as default if it is Locally provisioned.
    let client_interface_control =
        realm.interface_control(if_id).expect("get client interface Control");

    let fnet_resources::GrantForInterfaceAuthorization { interface_id, token } =
        client_interface_control
            .get_authorization_for_interface()
            .await
            .expect("get authorization");

    fnet_routes_ext::admin::authenticate_for_interface::<I>(
        route_set,
        fnet_resources::ProofOfInterfaceAuthorization { interface_id, token },
    )
    .await
    .expect("authenticate for interface should not see FIDL error")
    .expect("authenticate for interface should succeed");

    let default_route = fnet_routes_ext::Route {
        destination: I::ALL_ADDRS_SUBNET,
        action: fnet_routes_ext::RouteAction::Forward(fnet_routes_ext::RouteTarget::<I> {
            outbound_interface: interface_id,
            next_hop: None,
        }),
        properties: fnet_routes_ext::RouteProperties {
            specified_properties: fnet_routes_ext::SpecifiedRouteProperties {
                metric: fnet_routes::SpecifiedMetric::InheritedFromInterface(fnet_routes::Empty),
            },
        },
    };

    fnet_routes_ext::admin::add_route::<I>(
        route_set,
        &default_route.try_into().expect("convert into fidl route"),
    )
    .await
    .expect("should not see RouteSet FIDL error")
    .expect("should not see RouteSet error")
}
