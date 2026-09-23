// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use assert_matches::assert_matches;
use fidl::endpoints::create_proxy;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_name as fnet_name;
use fidl_fuchsia_net_policy_properties as fnp_properties;
use fidl_fuchsia_net_policy_socketproxy as fnp_socketproxy;
use fidl_fuchsia_net_reachability as freachability;
use fidl_fuchsia_net_root as fnet_root;
use fidl_fuchsia_net_routes as fnet_routes;
use fidl_fuchsia_net_routes_admin as fnet_routes_admin;
use fidl_fuchsia_net_routes_ext as fnet_routes_ext;
use fidl_fuchsia_posix_socket as fposix_socket;
use fnp_properties::{PropertyInterest, PropertyUpdate};
use fuchsia_async::{self as fasync, DurationExt as _, TimeoutExt as _};
use futures::channel::mpsc;
use futures::future::OptionFuture;
use futures::lock::Mutex;
use futures::{FutureExt as _, SinkExt as _, StreamExt as _};
use log::info;
use net_declare::fidl_ip_v6;
use net_types::ip::{Ip, Ipv4};
use netstack_testing_common::realms::{
    self, Manager, ManagerConfig, Netstack, NetstackExt, SocketProxyType,
};
use netstack_testing_common::{
    ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT, ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT,
    wait_for_component_stopped,
};
use netstack_testing_macros::netstack_test;
use policy_properties::{NetworkTokenExt, NetworksWatchDefaultResponseExt};
use policy_testing_common::{NetcfgOwnedDeviceArgs, add_default_route, with_netcfg_owned_device};
use pretty_assertions::assert_eq;
use std::collections::HashSet;
use std::pin::pin;
use std::sync::Arc;

const DEFAULT_DNS_PORT: u16 = 53;

trait TakeNetwork {
    fn take_network(self) -> Option<fnp_properties::NetworkToken>;
}

impl TakeNetwork for fnp_properties::NetworksWatchDefaultResponse {
    fn take_network(self) -> Option<fidl_fuchsia_net_policy_properties::NetworkToken> {
        match self {
            fnp_properties::NetworksWatchDefaultResponse::Network(network_token) => {
                Some(network_token)
            }
            fnp_properties::NetworksWatchDefaultResponse::NoDefaultNetwork(_) => None,
            _ => None,
        }
    }
}

fn network(network_id: u32, mark: Option<u32>) -> fnp_socketproxy::Network {
    fnp_socketproxy::Network {
        network_id: Some(network_id),
        info: Some(fnp_socketproxy::NetworkInfo::Starnix(fnp_socketproxy::StarnixNetworkInfo {
            mark: mark,
            handle: None,
            ..Default::default()
        })),
        ..Default::default()
    }
}

fn marks(mark_1: Option<u32>, mark_2: Option<u32>) -> fnet::Marks {
    fnet::Marks { mark_1, mark_2, ..Default::default() }
}

fn expect_subsequence(actual: &[Option<PropertyUpdate>], expected: &[Option<PropertyUpdate>]) {
    let mut actual_iter = actual.iter();

    for expect in expected {
        if !actual_iter.any(|value| value == expect) {
            panic!(
                "Missing property in sequence. Next expected property: \
                {expect:?}, actual was: {actual:?}"
            );
        }
    }
}

async fn watch_default_and_record_properties<F, R>(
    networks: fnp_properties::NetworksProxy,
    properties: PropertyInterest,
    last_updates: Arc<Mutex<Vec<Option<PropertyUpdate>>>>,
    mut tx: mpsc::Sender<()>,
    mut shutdown_rx: mpsc::Receiver<()>,
    mut is_new_update: F,
    mut reset_filter: R,
) where
    F: FnMut(&PropertyUpdate) -> bool,
    R: FnMut(),
{
    let mut network = None;
    let mut watcher_opt: Option<fnp_properties::PropertyWatcherProxy> = None;
    let watch_default = |networks: &fnp_properties::NetworksProxy| networks.watch_default().fuse();
    let watch_update = |watcher: &fnp_properties::PropertyWatcherProxy| watcher.watch().fuse();
    let mut next_network = watch_default(&networks);
    let mut watch_for_updates: OptionFuture<_> = None.into();
    loop {
        futures::select! {
            new_network = next_network => {
                match new_network
                    .expect("failed to fetch default network")
                    .take_network()
                {
                    Some(net) => {
                        info!("Observed new network");
                        reset_filter();
                        let net_dup = net.duplicate().expect("couldn't duplicate");
                        let (watcher, server_end) =
                            fidl::endpoints::create_proxy::<fnp_properties::PropertyWatcherMarker>();
                        networks
                            .watch_properties(
                                fnp_properties::NetworksWatchPropertiesRequest {
                                    network: Some(net_dup),
                                    properties: Some(properties.clone()),
                                    watcher: Some(server_end),
                                    ..Default::default()
                                },
                            )
                            .await
                            .expect("fidl error")
                            .expect("protocol error");
                        watcher_opt = Some(watcher);
                        network = Some(net);
                        watch_for_updates = watcher_opt.as_ref().map(watch_update).into();
                    }
                    None => {
                        info!("Default network was lost via Watch");
                        reset_filter();
                        let mut updates = last_updates.lock().await;
                        if network.is_some() && updates.last() != Some(&None) {
                            updates.push(None);
                            tx.send(()).await.expect("Can't send update");
                        }
                        network = None;
                        watch_for_updates = None.into();
                    }
                }
                next_network = watch_default(&networks);
            }
            property_update = watch_for_updates => {
                if let Some(property_update) = property_update {
                    match property_update {
                        Ok(Ok(update)) => {
                            if is_new_update(&update) {
                                info!("Updating last_updates: {:?}", update);
                                last_updates.lock().await.push(Some(update.clone()));
                                tx.send(()).await.expect("Can't send update");
                            }
                            watch_for_updates =
                                watcher_opt.as_ref().map(watch_update).into();
                        }
                        Ok(Err(fnp_properties::PropertyWatcherError::NetworkGone)) => {
                            info!("Default network was lost via Watch");
                            watch_for_updates = None.into();
                        }
                        other => panic!("Unexpected result from property watch: {:?}", other),
                    }
                }
            }
            _ = shutdown_rx.next() => {
                return;
            }
        }
    }
}

#[netstack_test]
#[variant(N, Netstack)]
#[variant(M, Manager)]
async fn test_track_socket_marks<N: Netstack, M: Manager>(name: &str) {
    use fnp_properties::{PropertyInterest, PropertyUpdate};

    let _if_name = with_netcfg_owned_device::<M, N, _>(
        name,
        ManagerConfig::EnableSocketProxy,
        NetcfgOwnedDeviceArgs {
            use_out_of_stack_dhcp_client: N::USE_OUT_OF_STACK_DHCP_CLIENT,
            socket_proxy_type: SocketProxyType::Fake,
            ..Default::default()
        },
        |_if_id, _network, _interface_state, realm, _sandbox| {
            async move {
                let (tx, mut rx) = mpsc::channel::<()>(1);
                let (mut shutdown_tx, shutdown_rx) = mpsc::channel::<()>(1);

                let last_updates = Arc::new(Mutex::new(Vec::new()));
                let last_marks = Arc::new(std::sync::Mutex::new(marks(None, None)));
                let last_marks_filter = last_marks.clone();
                let last_marks_reset = last_marks.clone();
                let background = watch_default_and_record_properties(
                    realm
                        .connect_to_protocol::<fnp_properties::NetworksMarker>()
                        .expect("couldn't connect to fuchsia.net.policy.properties/Networks"),
                    PropertyInterest::SOCKET_MARKS,
                    last_updates.clone(),
                    tx,
                    shutdown_rx,
                    move |update| {
                        if let Some(marks) = &update.socket_marks {
                            let mut last = last_marks_filter.lock().unwrap();
                            if marks != &*last {
                                *last = marks.clone();
                                return true;
                            }
                        }
                        false
                    },
                    move || {
                        *last_marks_reset.lock().unwrap() = marks(None, None);
                    },
                );

                let test = async move {
                    let socket_proxy = realm
                        .connect_to_protocol_from_child::<fnp_socketproxy::NetworkRegistryMarker>(
                            realms::constants::fake_socket_proxy::COMPONENT_NAME,
                        )
                        .expect("failed to connect to FakeSocketProxy");

                    socket_proxy
                        .add(&network(1, Some(1)))
                        .await
                        .expect("fidl error")
                        .expect("protocol error");
                    socket_proxy
                        .set_default(&fposix_socket::OptionalUint32::Value(1))
                        .await
                        .expect("fidl error")
                        .expect("protocol error");
                    rx.next().await.expect("channel closed");

                    socket_proxy
                        .update(&network(1, Some(2)))
                        .await
                        .expect("fidl error")
                        .expect("protocol error");
                    rx.next().await.expect("channel closed");

                    socket_proxy
                        .update(&network(1, Some(4)))
                        .await
                        .expect("fidl error")
                        .expect("protocol error");
                    rx.next().await.expect("channel closed");

                    socket_proxy
                        .set_default(&fposix_socket::OptionalUint32::Unset(fposix_socket::Empty))
                        .await
                        .expect("fidl error")
                        .expect("protocol error");
                    rx.next().await.expect("channel closed");

                    socket_proxy
                        .update(&network(1, Some(8)))
                        .await
                        .expect("fidl error")
                        .expect("protocol error");
                    socket_proxy
                        .set_default(&fposix_socket::OptionalUint32::Value(1))
                        .await
                        .expect("fidl error")
                        .expect("protocol error");
                    rx.next().await.expect("channel closed");

                    let updates = last_updates.lock().await.clone();
                    expect_subsequence(
                        &updates,
                        &vec![
                            Some(PropertyUpdate {
                                socket_marks: Some(marks(Some(1), None)),
                                dns_configuration: None,
                                ..Default::default()
                            }),
                            Some(PropertyUpdate {
                                socket_marks: Some(marks(Some(2), None)),
                                dns_configuration: None,
                                ..Default::default()
                            }),
                            Some(PropertyUpdate {
                                socket_marks: Some(marks(Some(4), None)),
                                dns_configuration: None,
                                ..Default::default()
                            }),
                            // None update represents the empty default_network.update call
                            None,
                            Some(PropertyUpdate {
                                socket_marks: Some(marks(Some(8), None)),
                                dns_configuration: None,
                                ..Default::default()
                            }),
                        ],
                    );

                    shutdown_tx.send(()).await.expect("couldn't trigger clean shutdown");
                };

                // N.B. Waiting for both futures to complete ensures that both
                // the test and the background task clean themselves up, closing
                // all open FIDL channels before shutting down the realm.
                futures::future::join(background, test).await;
            }
            .boxed_local()
        },
    )
    .await;
}

#[netstack_test]
#[variant(N, Netstack)]
#[variant(M, Manager)]
async fn test_track_dns_changes<N: Netstack, M: Manager>(name: &str) -> Result<(), anyhow::Error> {
    const NDP_DNS_SERVER1: fnet::Ipv6Address = fidl_ip_v6!("2001:db8::1");
    const NDP_DNS_SERVER2: fnet::Ipv6Address = fidl_ip_v6!("2001:db8::2");
    const NDP_DNS_SERVER3: fnet::Ipv6Address = fidl_ip_v6!("2001:db8::3");

    const TEST_NETWORK_ID: u32 = 2;
    const DNS_SERVER_LIST: [fnet::Ipv6Address; 3] =
        [NDP_DNS_SERVER1, NDP_DNS_SERVER2, NDP_DNS_SERVER3];

    let _if_name = with_netcfg_owned_device::<M, N, _>(
        name,
        ManagerConfig::EnableSocketProxy,
        NetcfgOwnedDeviceArgs {
            use_out_of_stack_dhcp_client: N::USE_OUT_OF_STACK_DHCP_CLIENT,
            socket_proxy_type: SocketProxyType::Fake,
            ..Default::default()
        },
        |_if_id, _test_network, _interface_state, realm, _sandbox| {
            async move {
                async fn update_dns(
                    socket_proxy: &fnp_socketproxy::NetworkRegistryProxy,
                    addresses: &[fnet::Ipv6Address],
                ) {
                    socket_proxy
                        .update(&fnp_socketproxy::Network {
                            network_id: Some(TEST_NETWORK_ID),
                            dns_servers: Some(fnp_socketproxy::NetworkDnsServers {
                                v6: Some(addresses.to_vec()),
                                v4: Some(vec![]),
                                ..Default::default()
                            }),
                            info: Some(fnp_socketproxy::NetworkInfo::Starnix(
                                fnp_socketproxy::StarnixNetworkInfo {
                                    mark: Some(123),
                                    ..Default::default()
                                },
                            )),
                            ..Default::default()
                        })
                        .await
                        .expect("fidl error")
                        .expect("protocol error");
                }

                let wait_for_netmgr = wait_for_component_stopped(
                    &realm,
                    realms::constants::netcfg::COMPONENT_NAME,
                    None,
                )
                .fuse();
                let mut wait_for_netmgr = pin!(wait_for_netmgr);
                let socket_proxy = realm
                    .connect_to_protocol_from_child::<fnp_socketproxy::NetworkRegistryMarker>(
                        realms::constants::fake_socket_proxy::COMPONENT_NAME,
                    )
                    .expect("failed to connect to FakeSocketProxy");
                socket_proxy
                    .add(&network(TEST_NETWORK_ID, None))
                    .await
                    .expect("fidl error")
                    .expect("protocol error");
                socket_proxy
                    .set_default(&fposix_socket::OptionalUint32::Value(2))
                    .await
                    .expect("fidl error")
                    .expect("protocol error");
                let networks = realm
                    .connect_to_protocol_from_child::<fnp_properties::NetworksMarker>(
                        realms::constants::netcfg::COMPONENT_NAME,
                    )
                    .expect("failed to connect to Networks");
                let network = networks
                    .watch_default()
                    .await
                    .expect("failed to fetch default network")
                    .take_network()
                    .expect("the first return from watch default should never fail");
                let (watcher, server_end) = create_proxy();
                networks
                    .watch_properties(fnp_properties::NetworksWatchPropertiesRequest {
                        network: Some(network.duplicate().expect("couldn't duplicate")),
                        properties: Some(fnp_properties::PropertyInterest::DNS_CONFIGURATION),
                        watcher: Some(server_end),
                        ..Default::default()
                    })
                    .await
                    .expect("failed to create watcher")
                    .expect("could not watch properties");
                let watch_update =
                    |watcher: &fnp_properties::PropertyWatcherProxy| watcher.watch().fuse();
                let mut watch = watch_update(&watcher);
                let mut dns_sequence = std::collections::VecDeque::from([
                    vec![DNS_SERVER_LIST[0]],
                    DNS_SERVER_LIST[0..2].to_vec(),
                    DNS_SERVER_LIST.to_vec(),
                ]);
                let mut last_dns_servers = None;
                let mut seen_dns_servers = Vec::new();

                'main: loop {
                    let () = futures::select! {
                        update = watch => {
                            watch = watch_update(&watcher);
                            let update = update.expect("fidl error").expect("protocol error");
                            let dns_config = update.dns_configuration.unwrap();
                            let servers = dns_config.servers.as_ref();
                            let server_count = servers.map_or(0, |s| s.len());

                            if servers != last_dns_servers.as_ref() {
                                last_dns_servers = dns_config.servers.clone();
                                seen_dns_servers.push(
                                    dns_config.servers.clone().unwrap_or_default()
                                );
                                if let Some(list) = dns_sequence.pop_front() {
                                    // Each update is 1 more server than the
                                    // last. Wait until we see the previous
                                    // update.
                                    if list.len() - 1 == server_count {
                                        update_dns(&socket_proxy, &list).await;
                                    } else {
                                        dns_sequence.push_front(list);
                                    }
                                }
                            }
                            // The final update should have all DNS servers.
                            if server_count >= DNS_SERVER_LIST.len() {
                                break 'main;
                            }
                        },
                        () = fuchsia_async::Timer::new(
                            ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT.after_now()
                        ).fuse() => {
                            panic!("timed out waiting for DNS server list");
                        },
                        stopped_event = wait_for_netmgr => {
                            panic!("the network manager: {stopped_event:?}");
                        }
                    };
                }

                let dns_servers = seen_dns_servers
                    .into_iter()
                    .map(|servers| {
                        servers.iter().map(|server| server.address).collect::<HashSet<_>>()
                    })
                    .collect::<Vec<_>>();

                // We expect there initially be an empty update and then
                // one update per DNS server in the list.
                assert_eq!(dns_servers.len(), DNS_SERVER_LIST.len() + 1);

                // 1st update: Initial empty list.
                assert_eq!(dns_servers[0], HashSet::new());

                // 2nd update: Just NDP_DNS_SERVER1.
                assert_eq!(
                    dns_servers[1],
                    vec![Some(fnet::SocketAddress::Ipv6(fnet::Ipv6SocketAddress {
                        address: NDP_DNS_SERVER1,
                        port: DEFAULT_DNS_PORT,
                        zone_index: 0
                    })),]
                    .into_iter()
                    .collect::<HashSet<_>>()
                );

                // 3rd update: Just NDP_DNS_SERVER1 and NDP_DNS_SERVER2.
                assert_eq!(
                    dns_servers[2],
                    vec![
                        Some(fnet::SocketAddress::Ipv6(fnet::Ipv6SocketAddress {
                            address: NDP_DNS_SERVER1,
                            port: DEFAULT_DNS_PORT,
                            zone_index: 0
                        })),
                        Some(fnet::SocketAddress::Ipv6(fnet::Ipv6SocketAddress {
                            address: NDP_DNS_SERVER2,
                            port: DEFAULT_DNS_PORT,
                            zone_index: 0
                        })),
                    ]
                    .into_iter()
                    .collect::<HashSet<_>>()
                );

                // 4th update: Just NDP_DNS_SERVER1, NDP_DNS_SERVER2, and NDP_DNS_SERVER3.
                assert_eq!(
                    dns_servers[3],
                    vec![
                        Some(fnet::SocketAddress::Ipv6(fnet::Ipv6SocketAddress {
                            address: NDP_DNS_SERVER1,
                            port: DEFAULT_DNS_PORT,
                            zone_index: 0
                        })),
                        Some(fnet::SocketAddress::Ipv6(fnet::Ipv6SocketAddress {
                            address: NDP_DNS_SERVER2,
                            port: DEFAULT_DNS_PORT,
                            zone_index: 0
                        })),
                        Some(fnet::SocketAddress::Ipv6(fnet::Ipv6SocketAddress {
                            address: NDP_DNS_SERVER3,
                            port: DEFAULT_DNS_PORT,
                            zone_index: 0
                        })),
                    ]
                    .into_iter()
                    .collect::<HashSet<_>>()
                );
            }
            .boxed_local()
        },
    )
    .await;

    Ok(())
}

#[netstack_test]
#[variant(N, Netstack)]
#[variant(M, Manager)]
async fn test_network_token_correlation<N: Netstack, M: Manager>(
    name: &str,
) -> Result<(), anyhow::Error> {
    let _if_name = with_netcfg_owned_device::<M, N, _>(
        name,
        ManagerConfig::EnableSocketProxy,
        NetcfgOwnedDeviceArgs {
            use_out_of_stack_dhcp_client: N::USE_OUT_OF_STACK_DHCP_CLIENT,
            socket_proxy_type: SocketProxyType::Fake,
            ..Default::default()
        },
        |if_id, _network, _interface_state, realm, _sandbox| {
            async move {
                let socket_proxy = realm
                    .connect_to_protocol_from_child::<fnp_socketproxy::NetworkRegistryMarker>(
                        realms::constants::fake_socket_proxy::COMPONENT_NAME,
                    )
                    .expect("failed to connect to FakeSocketProxy");
                socket_proxy
                    .add(&network(if_id.try_into().unwrap(), None))
                    .await
                    .expect("fidl error")
                    .expect("failed to add network");
                socket_proxy
                    .set_default(&fposix_socket::OptionalUint32::Value(if_id.try_into().unwrap()))
                    .await
                    .expect("fidl error")
                    .expect("failed to set default");

                let networks = realm
                    .connect_to_protocol_from_child::<fnp_properties::NetworksMarker>(
                        realms::constants::netcfg::COMPONENT_NAME,
                    )
                    .expect("fidl error");
                let networks2 = realm
                    .connect_to_protocol_from_child::<fnp_properties::NetworksMarker>(
                        realms::constants::netcfg::COMPONENT_NAME,
                    )
                    .expect("fidl error");

                let (net1, net2) =
                    futures::future::join(networks.watch_default(), networks2.watch_default())
                        .await;
                let (net1, net2) = (
                    net1.ok().and_then(NetworksWatchDefaultResponseExt::into_network).unwrap(),
                    net2.ok().and_then(NetworksWatchDefaultResponseExt::into_network).unwrap(),
                );

                assert!(net1.koid().is_ok());
                assert_eq!(net1.koid(), net2.koid());

                let resolver = realm
                    .connect_to_protocol_from_child::<fnp_properties::NetworkTokenResolverMarker>(
                        realms::constants::netcfg::COMPONENT_NAME,
                    )
                    .expect("fidl error");
                let resolver2 = realm
                    .connect_to_protocol_from_child::<fnp_properties::NetworkTokenResolverMarker>(
                        realms::constants::netcfg::COMPONENT_NAME,
                    )
                    .expect("fidl error");
                let (resolved1, resolved2) = futures::future::join(
                    resolver.resolve_token(net1.duplicate().expect("can't duplicate")),
                    resolver2.resolve_token(net1.duplicate().expect("can't duplicate")),
                )
                .await;
                let (resolved1, resolved2) = (
                    resolved1.expect("fidl error").expect("bad token"),
                    resolved2.expect("fidl error").expect("bad token"),
                );

                assert_ne!(resolved1.koid(), net1.koid());
                assert!(resolved1.koid().is_ok());
                assert_eq!(resolved1.koid(), resolved2.koid());

                // Unset the default.
                socket_proxy
                    .set_default(&fposix_socket::OptionalUint32::Unset(fposix_socket::Empty))
                    .await
                    .expect("fidl error")
                    .expect("failed to set default");

                // Resolving using a default network token for a network that is
                // no longer the default should fail, since the default network
                // token is no longer valid.
                let no_default_resolved = resolver
                    .resolve_token(net1.duplicate().expect("can't duplicate"))
                    .await
                    .expect("fidl error");
                assert_matches!(
                    no_default_resolved,
                    Err(fnp_properties::NetworkTokenResolverResolveTokenError::InvalidNetworkToken)
                );
            }
            .boxed_local()
        },
    )
    .await;

    Ok(())
}

const TEST_NETWORK_ID: u32 = 2;
const TEST_MARK: u32 = 123;
const TEST_NETWORK_ID_2: u32 = 3;
const TEST_MARK_2: u32 = 456;

#[netstack_test]
#[variant(N, Netstack)]
#[variant(M, Manager)]
async fn test_network_registry_dns_propagation<N: Netstack, M: Manager>(
    name: &str,
) -> Result<(), anyhow::Error> {
    let _if_name = with_netcfg_owned_device::<M, N, _>(
        name,
        ManagerConfig::EnableSocketProxy,
        NetcfgOwnedDeviceArgs {
            use_out_of_stack_dhcp_client: N::USE_OUT_OF_STACK_DHCP_CLIENT,
            socket_proxy_type: SocketProxyType::None,
            ..Default::default()
        },
        |_if_id, _network, _interface_state, realm, _sandbox| {
            async move {
                let delegated_networks = realm
                    .connect_to_protocol_from_child::<fnp_socketproxy::NetworkRegistryMarker>(
                        realms::constants::netcfg::COMPONENT_NAME,
                    )
                    .expect("failed to connect to Netcfg NetworkRegistry");

                let networks = realm
                    .connect_to_protocol_from_child::<fnp_properties::NetworksMarker>(
                        realms::constants::netcfg::COMPONENT_NAME,
                    )
                    .expect("failed to connect to Networks");

                const NDP_DNS_SERVER: fnet::Ipv6Address = fidl_ip_v6!("2001:db8::1");
                const NDP_DNS_SERVER_2: fnet::Ipv6Address = fidl_ip_v6!("2001:db8::2");

                // Add the first delegated network.
                delegated_networks
                    .add(&fnp_socketproxy::Network {
                        network_id: Some(TEST_NETWORK_ID),
                        info: Some(fnp_socketproxy::NetworkInfo::Starnix(
                            fnp_socketproxy::StarnixNetworkInfo {
                                mark: Some(TEST_MARK),
                                ..Default::default()
                            },
                        )),
                        dns_servers: Some(fnp_socketproxy::NetworkDnsServers {
                            v6: Some(vec![NDP_DNS_SERVER]),
                            v4: Some(vec![]),
                            ..Default::default()
                        }),
                        ..Default::default()
                    })
                    .await
                    .expect("fidl error")
                    .expect("failed to add network");

                // Make the first delegated network default.
                delegated_networks
                    .set_default(&fposix_socket::OptionalUint32::Value(TEST_NETWORK_ID))
                    .await
                    .expect("fidl error")
                    .expect("failed to set default");

                // Add second delegated network.
                delegated_networks
                    .add(&fnp_socketproxy::Network {
                        network_id: Some(TEST_NETWORK_ID_2),
                        info: Some(fnp_socketproxy::NetworkInfo::Starnix(
                            fnp_socketproxy::StarnixNetworkInfo {
                                mark: Some(TEST_MARK_2),
                                ..Default::default()
                            },
                        )),
                        dns_servers: Some(fnp_socketproxy::NetworkDnsServers {
                            v6: Some(vec![NDP_DNS_SERVER_2]),
                            v4: Some(vec![]),
                            ..Default::default()
                        }),
                        ..Default::default()
                    })
                    .await
                    .expect("fidl error")
                    .expect("failed to add network");

                // Watch properties on default network and observe the DNS server configuration.
                let network_token = networks
                    .watch_default()
                    .await
                    .expect("failed to watch default network")
                    .take_network()
                    .expect("no default network token");

                let (watcher, server_end) = create_proxy();
                networks
                    .watch_properties(fnp_properties::NetworksWatchPropertiesRequest {
                        network: Some(network_token.duplicate().expect("couldn't duplicate")),
                        properties: Some(fnp_properties::PropertyInterest::DNS_CONFIGURATION),
                        watcher: Some(server_end),
                        ..Default::default()
                    })
                    .await
                    .expect("failed to create watcher")
                    .expect("could not watch properties");
                let watch_update =
                    |watcher: &fnp_properties::PropertyWatcherProxy| watcher.watch().fuse();
                let update =
                    watch_update(&watcher).await.expect("fidl error").expect("protocol error");

                let actual_servers: HashSet<_> = update
                    .dns_configuration
                    .as_ref()
                    .unwrap()
                    .servers
                    .as_ref()
                    .expect("DNS servers must be set")
                    .iter()
                    .map(|server| server.address.expect("server address must be present"))
                    .collect();

                // Per-network properties must ONLY return DNS servers belonging to that network.
                let expected_isolated_servers =
                    HashSet::from([fnet::SocketAddress::Ipv6(fnet::Ipv6SocketAddress {
                        address: NDP_DNS_SERVER,
                        port: DEFAULT_DNS_PORT,
                        zone_index: 0,
                    })]);
                assert_eq!(actual_servers, expected_isolated_servers);

                // Ensure that watch_properties does not return again with additional DNS updates.
                assert!(watch_update(&watcher).now_or_never().is_none());
            }
            .boxed_local()
        },
    )
    .await;

    Ok(())
}

#[netstack_test]
#[variant(N, Netstack)]
#[variant(M, Manager)]
async fn test_network_registry_socket_marks_propagation<N: Netstack, M: Manager>(name: &str) {
    let _if_name = with_netcfg_owned_device::<M, N, _>(
        name,
        ManagerConfig::EnableSocketProxy,
        NetcfgOwnedDeviceArgs {
            use_out_of_stack_dhcp_client: N::USE_OUT_OF_STACK_DHCP_CLIENT,
            socket_proxy_type: SocketProxyType::None,
            ..Default::default()
        },
        |_if_id, _network, _interface_state, realm, _sandbox| {
            async move {
                let delegated_networks = realm
                    .connect_to_protocol_from_child::<fnp_socketproxy::NetworkRegistryMarker>(
                        realms::constants::netcfg::COMPONENT_NAME,
                    )
                    .expect("failed to connect to Netcfg NetworkRegistry");

                let networks = realm
                    .connect_to_protocol_from_child::<fnp_properties::NetworksMarker>(
                        realms::constants::netcfg::COMPONENT_NAME,
                    )
                    .expect("failed to connect to Networks");

                // Add delegated network with a socket mark.
                delegated_networks
                    .add(&network(TEST_NETWORK_ID, Some(TEST_MARK)))
                    .await
                    .expect("fidl error")
                    .expect("failed to add network");

                // Set the delegated network as default.
                delegated_networks
                    .set_default(&fposix_socket::OptionalUint32::Value(TEST_NETWORK_ID))
                    .await
                    .expect("fidl error")
                    .expect("failed to set default");

                // call WatchDefault to get the new default network.
                let network_token = networks
                    .watch_default()
                    .await
                    .expect("failed to watch default network")
                    .take_network()
                    .expect("no default network token");

                let watch_properties =
                    |networks: &fnp_properties::NetworksProxy,
                     token: &fnp_properties::NetworkToken| {
                        let (watcher, server_end) = create_proxy();
                        let res = networks.watch_properties(
                            fnp_properties::NetworksWatchPropertiesRequest {
                                network: Some(token.duplicate().expect("couldn't duplicate")),
                                properties: Some(PropertyInterest::SOCKET_MARKS),
                                watcher: Some(server_end),
                                ..Default::default()
                            },
                        );
                        async move { res.await.map(|res| res.map(|()| watcher)) }
                    };

                // WatchProperties and verify socket mark is propagated.
                let watcher = watch_properties(&networks, &network_token)
                    .await
                    .expect("fidl error")
                    .expect("protocol error");
                let update = watcher.watch().await.expect("fidl error").expect("protocol error");

                assert_eq!(
                    update,
                    PropertyUpdate {
                        socket_marks: Some(fnet::Marks {
                            mark_1: Some(TEST_MARK),
                            ..Default::default()
                        }),
                        dns_configuration: None,
                        ..Default::default()
                    }
                );

                // Update the network mark.
                delegated_networks
                    .update(&network(TEST_NETWORK_ID, Some(TEST_MARK_2)))
                    .await
                    .expect("fidl error")
                    .expect("failed to update network");

                // Verify the mark update is propagated.
                let update2 = watcher.watch().await.expect("fidl error").expect("protocol error");

                assert_eq!(
                    update2,
                    PropertyUpdate {
                        socket_marks: Some(fnet::Marks {
                            mark_1: Some(TEST_MARK_2),
                            ..Default::default()
                        }),
                        dns_configuration: None,
                        ..Default::default()
                    }
                );

                // Add a second network and set it as default.
                delegated_networks
                    .add(&network(TEST_NETWORK_ID_2, Some(TEST_MARK_2)))
                    .await
                    .expect("fidl error")
                    .expect("failed to add network 2");

                delegated_networks
                    .set_default(&fposix_socket::OptionalUint32::Value(TEST_NETWORK_ID_2))
                    .await
                    .expect("fidl error")
                    .expect("failed to set default network 2");

                // WatchDefault should notify that the default network has changed to
                // the second network.
                let network_token_2 = networks
                    .watch_default()
                    .await
                    .expect("failed to watch default network")
                    .take_network()
                    .expect("no default network token 2");

                // Verify the properties of the new default network token.
                let watcher_2 = watch_properties(&networks, &network_token_2)
                    .await
                    .expect("fidl error")
                    .expect("protocol error");
                let update3 = watcher_2.watch().await.expect("fidl error").expect("protocol error");

                assert_eq!(
                    update3,
                    PropertyUpdate {
                        socket_marks: Some(fnet::Marks {
                            mark_1: Some(TEST_MARK_2),
                            ..Default::default()
                        }),
                        dns_configuration: None,
                        ..Default::default()
                    }
                );

                // WatchProperties on the old default token should return InvalidNetworkToken
                // since the token was invalidated and dropped when the default network changed.
                let properties_gone = watch_properties(&networks, &network_token).await;
                assert_matches!(
                    properties_gone,
                    Ok(Err(fnp_properties::WatchError::InvalidNetworkToken))
                );

                // Remove all the networks.
                delegated_networks
                    .set_default(&fposix_socket::OptionalUint32::Unset(fposix_socket::Empty))
                    .await
                    .expect("fidl error")
                    .expect("failed to unset default network");

                delegated_networks
                    .remove(TEST_NETWORK_ID)
                    .await
                    .expect("fidl error")
                    .expect("failed to remove network 1");

                delegated_networks
                    .remove(TEST_NETWORK_ID_2)
                    .await
                    .expect("fidl error")
                    .expect("failed to remove network 2");
            }
            .boxed_local()
        },
    )
    .await;
}

#[netstack_test]
#[variant(N, Netstack)]
#[variant(M, Manager)]
async fn test_network_registry_fuchsia_priority<N: Netstack, M: Manager>(
    name: &str,
) -> Result<(), anyhow::Error> {
    let _if_name = with_netcfg_owned_device::<M, N, _>(
        name,
        ManagerConfig::EnableSocketProxy,
        NetcfgOwnedDeviceArgs {
            use_out_of_stack_dhcp_client: N::USE_OUT_OF_STACK_DHCP_CLIENT,
            socket_proxy_type: SocketProxyType::Fake,
            ..Default::default()
        },
        |if_id, _network, _interface_state, realm, _sandbox| {
            async move {
                let delegated_networks = realm
                    .connect_to_protocol_from_child::<fnp_socketproxy::NetworkRegistryMarker>(
                        realms::constants::fake_socket_proxy::COMPONENT_NAME,
                    )
                    .expect("failed to connect to FakeSocketProxy NetworkRegistry");

                let networks = realm
                    .connect_to_protocol_from_child::<fnp_properties::NetworksMarker>(
                        realms::constants::netcfg::COMPONENT_NAME,
                    )
                    .expect("failed to connect to Networks");

                let root_routes = realm
                    .connect_to_protocol::<fnet_root::RoutesV4Marker>()
                    .expect("connect to fuchsia.net.root.RoutesV4");
                let (route_set, server_end) =
                    fidl::endpoints::create_proxy::<fnet_routes_admin::RouteSetV4Marker>();
                root_routes.global_route_set(server_end).expect("create global RouteSetV4");

                // Add a default route to make Fuchsia the default network.
                assert!(add_default_route::<Ipv4>(realm, if_id, &route_set).await);

                // Watch default network should return the Fuchsia default network token.
                let default_fuchsia_token = networks
                    .watch_default()
                    .await
                    .expect("failed to watch default network")
                    .take_network()
                    .expect("no default network token");

                // Verify that only DnsConfiguration is returned and no SocketMarks property
                // is present since Fuchsia networks have no socket marks.
                let (watcher, server_end) = create_proxy();
                networks
                    .watch_properties(fnp_properties::NetworksWatchPropertiesRequest {
                        network: Some(default_fuchsia_token.duplicate().expect("dup failed")),
                        properties: Some(
                            PropertyInterest::SOCKET_MARKS | PropertyInterest::DNS_CONFIGURATION,
                        ),
                        watcher: Some(server_end),
                        ..Default::default()
                    })
                    .await
                    .expect("fidl error")
                    .expect("protocol error");
                let update = watcher.watch().await.expect("fidl error").expect("protocol error");

                assert_eq!(
                    update,
                    PropertyUpdate {
                        dns_configuration: Some(fnp_properties::DnsConfiguration {
                            servers: Some(vec![]),
                            ..Default::default()
                        }),
                        socket_marks: None,
                        ..Default::default()
                    }
                );

                // Add a delegated network and set it as default.
                delegated_networks
                    .add(&network(TEST_NETWORK_ID, Some(TEST_MARK)))
                    .await
                    .expect("fidl error")
                    .expect("failed to add network");

                delegated_networks
                    .set_default(&fposix_socket::OptionalUint32::Value(TEST_NETWORK_ID))
                    .await
                    .expect("fidl error")
                    .expect("failed to set default");

                // Attempt to watch default. Since Fuchsia still has priority, the call to the
                // hanging get should not complete.
                let mut watch_fut = pin!(networks.watch_default());

                // Verify that watch_fut does not resolve before route removal (times out).
                assert_matches!(
                    watch_fut
                        .as_mut()
                        .map(Ok)
                        .on_timeout(ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT.after_now(), || {
                            Err(zx::Status::TIMED_OUT)
                        })
                        .await,
                    Err(zx::Status::TIMED_OUT)
                );

                // Remove the Fuchsia default route to unset the Fuchsia default network.
                let default_route = fnet_routes_ext::Route {
                    destination: Ipv4::ALL_ADDRS_SUBNET,
                    action: fnet_routes_ext::RouteAction::Forward(fnet_routes_ext::RouteTarget::<
                        Ipv4,
                    > {
                        outbound_interface: if_id,
                        next_hop: None,
                    }),
                    properties: fnet_routes_ext::RouteProperties {
                        specified_properties: fnet_routes_ext::SpecifiedRouteProperties {
                            metric: fnet_routes::SpecifiedMetric::InheritedFromInterface(
                                fnet_routes::Empty,
                            ),
                        },
                    },
                };
                assert!(
                    fnet_routes_ext::admin::remove_route::<Ipv4>(
                        &route_set,
                        &default_route.try_into().expect("convert into fidl route"),
                    )
                    .await
                    .expect("failed to remove default route")
                    .expect("remove route error")
                );

                // WatchDefault should resolve and return the delegated default network.
                let _default_delegated_token = watch_fut
                    .await
                    .expect("failed to watch default network")
                    .take_network()
                    .expect("no default network token");

                // Verify that the Fuchsia token transmits PEER_CLOSED.
                fasync::OnSignals::new(
                    &default_fuchsia_token.value,
                    zx::Signals::EVENTPAIR_PEER_CLOSED,
                )
                .await
                .map(|_signal: zx::Signals| ())
                .unwrap();
            }
            .boxed_local()
        },
    )
    .await;

    Ok(())
}

#[netstack_test]
#[variant(N, Netstack)]
#[variant(M, Manager)]
async fn test_network_token_peer_closed_on_removal<N: Netstack, M: Manager>(
    name: &str,
) -> Result<(), anyhow::Error> {
    let _if_name = with_netcfg_owned_device::<M, N, _>(
        name,
        ManagerConfig::EnableSocketProxy,
        NetcfgOwnedDeviceArgs {
            use_out_of_stack_dhcp_client: N::USE_OUT_OF_STACK_DHCP_CLIENT,
            socket_proxy_type: SocketProxyType::Fake,
            ..Default::default()
        },
        |_if_id, _network, _interface_state, realm, _sandbox| {
            async move {
                let delegated_networks = realm
                    .connect_to_protocol_from_child::<fnp_socketproxy::NetworkRegistryMarker>(
                        realms::constants::fake_socket_proxy::COMPONENT_NAME,
                    )
                    .expect("failed to connect to FakeSocketProxy NetworkRegistry");

                let networks = realm
                    .connect_to_protocol_from_child::<fnp_properties::NetworksMarker>(
                        realms::constants::netcfg::COMPONENT_NAME,
                    )
                    .expect("failed to connect to Networks");

                let token_resolver = realm
                    .connect_to_protocol_from_child::<fnp_properties::NetworkTokenResolverMarker>(
                        realms::constants::netcfg::COMPONENT_NAME,
                    )
                    .expect("failed to connect to NetworkTokenResolver");

                // Add delegated network and set as default.
                delegated_networks
                    .add(&network(TEST_NETWORK_ID, Some(TEST_MARK)))
                    .await
                    .expect("fidl error")
                    .expect("failed to add network");

                delegated_networks
                    .set_default(&fposix_socket::OptionalUint32::Value(TEST_NETWORK_ID))
                    .await
                    .expect("fidl error")
                    .expect("failed to set default");

                // Watch default to get its token, and resolve it.
                let default_starnix_token = networks
                    .watch_default()
                    .await
                    .expect("failed to watch default network")
                    .take_network()
                    .expect("no default network token");

                let resolved_starnix_token = token_resolver
                    .resolve_token(default_starnix_token.duplicate().expect("dup failed"))
                    .await
                    .expect("fidl error")
                    .expect("failed to resolve token");

                // Unset default delegated network.
                delegated_networks
                    .set_default(&fposix_socket::OptionalUint32::Unset(fposix_socket::Empty))
                    .await
                    .expect("fidl error")
                    .expect("failed to unset default");

                // Verify that default_starnix_token transmits PEER_CLOSED since it is no longer
                // the default network.
                fasync::OnSignals::new(
                    &default_starnix_token.value,
                    zx::Signals::EVENTPAIR_PEER_CLOSED,
                )
                .await
                .map(|_signal: zx::Signals| ())
                .unwrap();

                // Remove the delegated network.
                delegated_networks
                    .remove(TEST_NETWORK_ID)
                    .await
                    .expect("fidl error")
                    .expect("failed to remove network");

                // Verify that resolved_starnix_token transmits PEER_CLOSED because the
                // network was removed.
                let peer_closed_fut = fasync::OnSignals::new(
                    &resolved_starnix_token.value,
                    zx::Signals::EVENTPAIR_PEER_CLOSED,
                );
                let _signals = fasync::TimeoutExt::on_timeout(
                    peer_closed_fut,
                    fasync::MonotonicInstant::after(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT),
                    || Err(zx::Status::TIMED_OUT),
                )
                .await
                .expect("failed to wait for resolved token PEER_CLOSED");
            }
            .boxed_local()
        },
    )
    .await;

    Ok(())
}

/// Tests that `PropertyWatcher` tracks DNS configuration changes when the default network switches.
#[netstack_test]
#[variant(N, Netstack)]
#[variant(M, Manager)]
async fn test_track_dns_changes_default_switch<N: Netstack, M: Manager>(name: &str) {
    use fnp_properties::{PropertyInterest, PropertyUpdate};

    let _if_name = with_netcfg_owned_device::<M, N, _>(
        name,
        ManagerConfig::EnableSocketProxy,
        NetcfgOwnedDeviceArgs {
            use_out_of_stack_dhcp_client: N::USE_OUT_OF_STACK_DHCP_CLIENT,
            socket_proxy_type: SocketProxyType::None,
            ..Default::default()
        },
        |_if_id, _network, _interface_state, realm, _sandbox| {
            async move {
                let (tx, mut rx) = mpsc::channel::<()>(1);
                let (mut shutdown_tx, shutdown_rx) = mpsc::channel::<()>(1);

                let last_updates = Arc::new(Mutex::new(Vec::new()));
                // Listen for `WatchDefault` changes. For each new default network token, register a
                // `PropertyWatcher` and record observed updates.
                let last_dns_servers = Arc::new(std::sync::Mutex::new(None));
                let last_dns_filter = last_dns_servers.clone();
                let last_dns_reset = last_dns_servers.clone();
                let background = watch_default_and_record_properties(
                    realm
                        .connect_to_protocol_from_child::<fnp_properties::NetworksMarker>(
                            realms::constants::netcfg::COMPONENT_NAME,
                        )
                        .expect("couldn't connect to fuchsia.net.policy.properties/Networks"),
                    PropertyInterest::DNS_CONFIGURATION,
                    last_updates.clone(),
                    tx,
                    shutdown_rx,
                    move |update| {
                        if let Some(dns_config) = &update.dns_configuration {
                            let servers = dns_config.servers.clone();
                            let mut last = last_dns_filter.lock().unwrap();
                            if servers != *last {
                                *last = servers;
                                return true;
                            }
                        }
                        false
                    },
                    move || {
                        *last_dns_reset.lock().unwrap() = None;
                    },
                );

                let test = async move {
                    let delegated_networks = realm
                        .connect_to_protocol_from_child::<fnp_socketproxy::NetworkRegistryMarker>(
                            realms::constants::netcfg::COMPONENT_NAME,
                        )
                        .expect("failed to connect to Netcfg NetworkRegistry");

                    const DNS_1: fnet::Ipv6Address = fidl_ip_v6!("2001:db8::1");
                    const DNS_2: fnet::Ipv6Address = fidl_ip_v6!("2001:db8::2");

                    // Add a network with DNS_1 and set it as default. The watcher should
                    // emit DNS_1.
                    delegated_networks
                        .add(&fnp_socketproxy::Network {
                            network_id: Some(1),
                            info: Some(fnp_socketproxy::NetworkInfo::Starnix(
                                fnp_socketproxy::StarnixNetworkInfo {
                                    mark: Some(123),
                                    ..Default::default()
                                },
                            )),
                            dns_servers: Some(fnp_socketproxy::NetworkDnsServers {
                                v6: Some(vec![DNS_1]),
                                v4: Some(vec![]),
                                ..Default::default()
                            }),
                            ..Default::default()
                        })
                        .await
                        .expect("fidl error")
                        .expect("protocol error");
                    delegated_networks
                        .set_default(&fposix_socket::OptionalUint32::Value(1))
                        .await
                        .expect("fidl error")
                        .expect("protocol error");
                    let _ = rx.next().await;

                    // Add a network with DNS_2 and set it as default. The watcher should
                    // emit DNS_2.
                    delegated_networks
                        .add(&fnp_socketproxy::Network {
                            network_id: Some(2),
                            info: Some(fnp_socketproxy::NetworkInfo::Starnix(
                                fnp_socketproxy::StarnixNetworkInfo {
                                    mark: Some(456),
                                    ..Default::default()
                                },
                            )),
                            dns_servers: Some(fnp_socketproxy::NetworkDnsServers {
                                v6: Some(vec![DNS_2]),
                                v4: Some(vec![]),
                                ..Default::default()
                            }),
                            ..Default::default()
                        })
                        .await
                        .expect("fidl error")
                        .expect("protocol error");
                    delegated_networks
                        .set_default(&fposix_socket::OptionalUint32::Value(2))
                        .await
                        .expect("fidl error")
                        .expect("protocol error");
                    let _ = rx.next().await;

                    // Change default network from 2 -> 1. The watcher should emit DNS_1.
                    delegated_networks
                        .set_default(&fposix_socket::OptionalUint32::Value(1))
                        .await
                        .expect("fidl error")
                        .expect("protocol error");
                    let _ = rx.next().await;

                    // Unset the default network. The watcher should emit None.
                    delegated_networks
                        .set_default(&fposix_socket::OptionalUint32::Unset(fposix_socket::Empty))
                        .await
                        .expect("fidl error")
                        .expect("protocol error");
                    let _ = rx.next().await;

                    let updates = last_updates.lock().await.clone();

                    let dns_config_1 = fnp_properties::DnsConfiguration {
                        servers: Some(vec![fnet_name::DnsServer_ {
                            address: Some(fnet::SocketAddress::Ipv6(fnet::Ipv6SocketAddress {
                                address: DNS_1,
                                port: DEFAULT_DNS_PORT,
                                zone_index: 0,
                            })),
                            source: Some(fnet_name::DnsServerSource::SocketProxy(
                                fnet_name::SocketProxyDnsServerSource {
                                    source_interface: Some(1),
                                    ..Default::default()
                                },
                            )),
                            ..Default::default()
                        }]),
                        ..Default::default()
                    };
                    let dns_config_2 = fnp_properties::DnsConfiguration {
                        servers: Some(vec![fnet_name::DnsServer_ {
                            address: Some(fnet::SocketAddress::Ipv6(fnet::Ipv6SocketAddress {
                                address: DNS_2,
                                port: DEFAULT_DNS_PORT,
                                zone_index: 0,
                            })),
                            source: Some(fnet_name::DnsServerSource::SocketProxy(
                                fnet_name::SocketProxyDnsServerSource {
                                    source_interface: Some(2),
                                    ..Default::default()
                                },
                            )),
                            ..Default::default()
                        }]),
                        ..Default::default()
                    };

                    // Verify that the PropertyWatcher sequence across default network switches
                    // matches [DNS_1, DNS_2, DNS_1, None].
                    expect_subsequence(
                        &updates,
                        &vec![
                            Some(PropertyUpdate {
                                dns_configuration: Some(dns_config_1.clone()),
                                ..Default::default()
                            }),
                            Some(PropertyUpdate {
                                dns_configuration: Some(dns_config_2),
                                ..Default::default()
                            }),
                            Some(PropertyUpdate {
                                dns_configuration: Some(dns_config_1),
                                ..Default::default()
                            }),
                            None,
                        ],
                    );

                    shutdown_tx.send(()).await.expect("couldn't trigger clean shutdown");
                };

                futures::future::join(background, test).await;
            }
            .boxed_local()
        },
    )
    .await;
}

/// Tests that removing a network while a client is watching properties emits `NETWORK_GONE`.
#[netstack_test]
#[variant(N, Netstack)]
#[variant(M, Manager)]
async fn test_network_removal_reports_network_gone<N: Netstack, M: Manager>(name: &str) {
    let _if_name = with_netcfg_owned_device::<M, N, _>(
        name,
        ManagerConfig::EnableSocketProxy,
        NetcfgOwnedDeviceArgs {
            use_out_of_stack_dhcp_client: N::USE_OUT_OF_STACK_DHCP_CLIENT,
            socket_proxy_type: SocketProxyType::Fake,
            ..Default::default()
        },
        |_if_id, _network, _interface_state, realm, _sandbox| {
            async move {
                let delegated_networks = realm
                    .connect_to_protocol_from_child::<fnp_socketproxy::NetworkRegistryMarker>(
                        realms::constants::fake_socket_proxy::COMPONENT_NAME,
                    )
                    .expect("failed to connect to FakeSocketProxy NetworkRegistry");

                let networks = realm
                    .connect_to_protocol_from_child::<fnp_properties::NetworksMarker>(
                        realms::constants::netcfg::COMPONENT_NAME,
                    )
                    .expect("failed to connect to Networks");

                let token_resolver = realm
                    .connect_to_protocol_from_child::<fnp_properties::NetworkTokenResolverMarker>(
                        realms::constants::netcfg::COMPONENT_NAME,
                    )
                    .expect("failed to connect to NetworkTokenResolver");

                // Add a network with a mark and set it as the default.
                delegated_networks
                    .add(&network(TEST_NETWORK_ID, Some(TEST_MARK)))
                    .await
                    .expect("fidl error")
                    .expect("failed to add network");

                delegated_networks
                    .set_default(&fposix_socket::OptionalUint32::Value(TEST_NETWORK_ID))
                    .await
                    .expect("fidl error")
                    .expect("failed to set default");

                let default_token = networks
                    .watch_default()
                    .await
                    .expect("failed to watch default network")
                    .take_network()
                    .expect("no default network token");

                // We must resolve the default token to get a non-default token because
                // the non-default token is closed by network removal, while the default
                // network token is closed by default network switching.
                let token = token_resolver
                    .resolve_token(default_token)
                    .await
                    .expect("fidl error")
                    .expect("failed to resolve token");

                delegated_networks
                    .set_default(&fposix_socket::OptionalUint32::Unset(fposix_socket::Empty))
                    .await
                    .expect("fidl error")
                    .expect("failed to unset default");

                // Register two PropertyWatchers. One will have an active in-flight Watch(), and the
                // other will remain idle until after the network is removed.
                let (active_watcher, active_server_end) = create_proxy();
                networks
                    .watch_properties(fnp_properties::NetworksWatchPropertiesRequest {
                        network: Some(token.duplicate().expect("duplicate token")),
                        properties: Some(PropertyInterest::SOCKET_MARKS),
                        watcher: Some(active_server_end),
                        ..Default::default()
                    })
                    .await
                    .expect("fidl error")
                    .expect("protocol error");

                let (idle_watcher, idle_server_end) = create_proxy();
                networks
                    .watch_properties(fnp_properties::NetworksWatchPropertiesRequest {
                        network: Some(token),
                        properties: Some(PropertyInterest::SOCKET_MARKS),
                        watcher: Some(idle_server_end),
                        ..Default::default()
                    })
                    .await
                    .expect("fidl error")
                    .expect("protocol error");

                for watcher in [&active_watcher, &idle_watcher] {
                    let initial =
                        watcher.watch().await.expect("fidl error").expect("protocol error");
                    assert_eq!(
                        initial,
                        PropertyUpdate {
                            socket_marks: Some(fnet::Marks {
                                mark_1: Some(TEST_MARK),
                                ..Default::default()
                            }),
                            ..Default::default()
                        }
                    );
                }

                // Register a pending call to Watch() on the active watcher, verify it is pending,
                // then remove the network.
                let mut active_watch_fut = active_watcher.watch();
                assert_matches!((&mut active_watch_fut).now_or_never(), None);

                delegated_networks
                    .remove(TEST_NETWORK_ID)
                    .await
                    .expect("fidl error")
                    .expect("failed to remove network");

                assert_matches!(
                    active_watch_fut.await,
                    Ok(Err(fnp_properties::PropertyWatcherError::NetworkGone))
                );

                // Verify that the idle watcher calling Watch() after removal also receives NetworkGone.
                assert_matches!(
                    idle_watcher.watch().await,
                    Ok(Err(fnp_properties::PropertyWatcherError::NetworkGone))
                );

                // Verify that any subsequent Watch() calls encounter a closed channel.
                for watcher in [&active_watcher, &idle_watcher] {
                    assert_matches!(
                        watcher.watch().await,
                        Err(fidl::Error::ClientChannelClosed {
                            epitaph: fidl::Epitaph::PeerClosed,
                            ..
                        })
                    );
                }
            }
            .boxed_local()
        },
    )
    .await;
}

#[netstack_test]
#[variant(N, Netstack)]
#[variant(M, Manager)]
async fn test_reachability_monitor_default_network_validation<N: Netstack, M: Manager>(name: &str) {
    const TEST_NETWORK_ID: u32 = 123;
    const TEST_MARK: u32 = 321;

    let _if_name = with_netcfg_owned_device::<M, N, _>(
        name,
        ManagerConfig::EnableSocketProxy,
        NetcfgOwnedDeviceArgs {
            use_out_of_stack_dhcp_client: N::USE_OUT_OF_STACK_DHCP_CLIENT,
            socket_proxy_type: SocketProxyType::Fake,
            extra_known_service_providers: Vec::new(),
        },
        |_if_id, _network, _interface_state, realm, _sandbox| {
            async move {
                let delegated_networks = realm
                    .connect_to_protocol_from_child::<fnp_socketproxy::NetworkRegistryMarker>(
                        realms::constants::fake_socket_proxy::COMPONENT_NAME,
                    )
                    .expect("failed to connect to FakeSocketProxy NetworkRegistry");

                let reachability = realm
                    .connect_to_protocol_from_child::<freachability::MonitorMarker>(
                        realms::constants::netcfg::COMPONENT_NAME,
                    )
                    .expect("failed to connect to Reachability Monitor");

                // Snapshots are synthesized from the default network's connectivity state.
                let disconnected = freachability::Snapshot {
                    gateway_reachable: Some(false),
                    dns_active: Some(false),
                    internet_available: Some(false),
                    http_active: Some(false),
                    ..Default::default()
                };
                let unvalidated = freachability::Snapshot {
                    gateway_reachable: Some(true),
                    dns_active: Some(true),
                    internet_available: Some(true),
                    http_active: Some(false),
                    ..Default::default()
                };
                let validated = freachability::Snapshot {
                    gateway_reachable: Some(true),
                    dns_active: Some(true),
                    internet_available: Some(true),
                    http_active: Some(true),
                    ..Default::default()
                };

                // Initial Watch() reports no reachability, as there is no default network.
                let initial = reachability.watch().await.expect("watch error");
                assert_eq!(initial, disconnected);

                // Add network with FullConnectivity and set as default.
                let mut valid_network = network(TEST_NETWORK_ID, Some(TEST_MARK));
                valid_network.connectivity =
                    Some(fnp_socketproxy::ConnectivityState::FullConnectivity);
                delegated_networks
                    .add(&valid_network)
                    .await
                    .expect("fidl error")
                    .expect("failed to add network");

                // Start watch before marking default.
                let mut pending_watch = reachability.watch();
                assert_matches!(futures::poll!(&mut pending_watch), std::task::Poll::Pending);

                delegated_networks
                    .set_default(&fposix_socket::OptionalUint32::Value(TEST_NETWORK_ID))
                    .await
                    .expect("fidl error")
                    .expect("failed to set default");

                // Watch() unblocks and reports full reachability.
                let validated_snapshot = pending_watch.await.expect("watch error");
                assert_eq!(validated_snapshot, validated);

                // Start next watch before updating connectivity.
                let mut pending_unvalidated = reachability.watch();
                assert_matches!(futures::poll!(&mut pending_unvalidated), std::task::Poll::Pending);

                // Transition default network to PartialConnectivity (unvalidated)
                let mut unvalidated_network = network(TEST_NETWORK_ID, Some(TEST_MARK));
                unvalidated_network.connectivity =
                    Some(fnp_socketproxy::ConnectivityState::PartialConnectivity);
                delegated_networks
                    .update(&unvalidated_network)
                    .await
                    .expect("fidl error")
                    .expect("failed to update network");

                // Watch() unblocks and reports DNS as active but HTTP as inactive.
                let unvalidated_snapshot = pending_unvalidated.await.expect("watch error");
                assert_eq!(unvalidated_snapshot, unvalidated);

                // Start next watch before unsetting the default network.
                let mut pending_unset = reachability.watch();
                assert_matches!(futures::poll!(&mut pending_unset), std::task::Poll::Pending);

                delegated_networks
                    .set_default(&fposix_socket::OptionalUint32::Unset(fposix_socket::Empty))
                    .await
                    .expect("fidl error")
                    .expect("failed to unset default");

                // Watch() unblocks and reports no reachability once more.
                let unset_snapshot = pending_unset.await.expect("watch error");
                assert_eq!(unset_snapshot, disconnected);
            }
            .boxed_local()
        },
    )
    .await;
}
