// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use assert_matches::assert_matches;
use fidl::endpoints;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_dhcpv6 as fnet_dhcpv6;
use fidl_fuchsia_net_interfaces as _;
use fuchsia_async::TimeoutExt as _;
use net_declare::fidl_ip_v6;
use netstack_testing_common::ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT;
use netstack_testing_common::realms::{KnownServiceProvider, Netstack3, TestSandboxExt as _};

#[fuchsia::test]
async fn dhcpv6_client_bind_to_other_interface_address() {
    let sandbox = netemul::TestSandbox::new().unwrap();
    let network = sandbox.create_network("net").await.unwrap();

    let realm = sandbox
        .create_netstack_realm_with::<Netstack3, _, _>(
            "dhcpv6_client_bind_to_other_interface_address",
            &[KnownServiceProvider::Dhcpv6Client],
        )
        .unwrap();

    let iface_a = realm.join_network(&network, "iface_a").await.unwrap();
    let iface_b = realm.join_network(&network, "iface_b").await.unwrap();

    let iface_b_addr = fidl_ip_v6!("2001:db8::2");
    iface_b
        .add_address_and_subnet_route(fnet::Subnet {
            addr: fnet::IpAddress::Ipv6(iface_b_addr),
            prefix_len: 64,
        })
        .await
        .expect("add address to iface_b");

    let client_provider = realm
        .connect_to_protocol::<fnet_dhcpv6::ClientProviderMarker>()
        .expect("connect to ClientProvider should succeed");

    let (client_proxy, server_end) = endpoints::create_proxy::<fnet_dhcpv6::ClientMarker>();

    // Create a DHCPv6 client for `iface_a` that is bound to an address on
    // `iface_b` and expect that the client fails to start.

    let params = fnet_dhcpv6::NewClientParams {
        interface_id: Some(iface_a.id()),
        address: Some(fnet::Ipv6SocketAddress {
            address: iface_b_addr,
            port: fnet_dhcpv6::DEFAULT_CLIENT_PORT,
            zone_index: 0,
        }),
        config: Some(fnet_dhcpv6::ClientConfig {
            information_config: Some(fnet_dhcpv6::InformationConfig {
                dns_servers: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    };

    client_provider.new_client(&params, server_end).expect("new_client call should succeed");

    assert_matches!(
        client_proxy
            .watch_address()
            .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT, || panic!(
                "expected peer to close the channel"
            ))
            .await,
        Err(fidl::Error::ClientChannelClosed { epitaph: fidl::Epitaph::PeerClosed, .. })
    );
}
