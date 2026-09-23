// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use fidl_fuchsia_net_ext::IntoExt as _;
use net_declare::{fidl_subnet, net_addr_subnet};
use net_types::Witness as _;
use net_types::ip::{AddrSubnetEither, Ipv4, Ipv6};
use netemul::InterfaceConfig;
use netstack_testing_common::interfaces::TestInterfaceExt as _;
use netstack_testing_common::ping;
use netstack_testing_common::realms::{Netstack, TestSandboxExt as _};
use netstack_testing_macros::netstack_test;
use test_case::test_case;

#[netstack_test]
#[variant(N, Netstack)]
async fn ping<N: Netstack>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let net = sandbox.create_network("net").await.expect("failed to create network");

    let create_realm = |suffix, addr| {
        let sandbox = &sandbox;
        let net = &net;
        async move {
            let realm = sandbox
                .create_netstack_realm::<N, _>(format!("{}_{}", name, suffix))
                .expect("failed to create realm");
            let interface = realm
                .join_network(&net, format!("ep_{}", suffix))
                .await
                .expect("failed to join network in realm");
            interface.add_address_and_subnet_route(addr).await.expect("configure address");
            interface.apply_nud_flake_workaround().await.expect("nud flake workaround");
            (realm, interface)
        }
    };

    let (realm_a, if_a) = create_realm("a", fidl_subnet!("192.168.1.1/16")).await;
    let (realm_b, if_b) = create_realm("b", fidl_subnet!("192.168.1.2/16")).await;

    let node_a = ping::Node::new_with_v4_and_v6_link_local(&realm_a, &if_a)
        .await
        .expect("failed to construct node A");
    let node_b = ping::Node::new_with_v4_and_v6_link_local(&realm_b, &if_b)
        .await
        .expect("failed to construct node B");

    node_a
        .ping_pairwise(std::slice::from_ref(&node_b))
        .await
        .expect("failed to ping between nodes");
}

#[netstack_test]
#[variant(N, Netstack)]
#[test_case(net_addr_subnet!("192.0.2.1/32"); "v4")]
#[test_case(net_addr_subnet!("fe80::1234:5678:90ab:cdef/128"); "v6_link_local")]
#[test_case(net_addr_subnet!("2001:db8::1/128"); "v6")]
async fn ping_self<N: Netstack>(name: &str, addr: AddrSubnetEither) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = sandbox.create_netstack_realm::<N, _>(name).expect("create realm");
    let ep = sandbox.create_endpoint(name).await.expect("create endpoint");
    let interface =
        realm.install_endpoint(ep, InterfaceConfig::default()).await.expect("install endpoint");
    interface.add_address(addr.into_ext()).await.expect("add address");

    const UNSPECIFIED_PORT: u16 = 0;
    const PING_SEQ: u16 = 1;
    match addr {
        AddrSubnetEither::V4(v4) => {
            realm
                .ping_once::<Ipv4>(
                    std::net::SocketAddrV4::new(v4.addr().get().into(), UNSPECIFIED_PORT),
                    PING_SEQ,
                )
                .await
        }
        AddrSubnetEither::V6(v6) => {
            let v6 = v6.addr().get();
            realm
                .ping_once::<Ipv6>(
                    std::net::SocketAddrV6::new(
                        v6.into(),
                        UNSPECIFIED_PORT,
                        0,
                        if v6.is_unicast_link_local() {
                            u32::try_from(interface.id()).expect("interface ID should fit into u32")
                        } else {
                            0
                        },
                    ),
                    PING_SEQ,
                )
                .await
        }
    }
    .expect("ping self address");
}
