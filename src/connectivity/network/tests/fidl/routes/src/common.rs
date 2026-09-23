// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::ProtocolMarker;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fidl_fuchsia_net_matchers_ext as fnet_matchers_ext;
use fidl_fuchsia_net_routes as fnet_routes;
use fidl_fuchsia_net_routes_ext::admin::FidlRouteAdminIpExt;
use fidl_fuchsia_net_routes_ext::rules::{FidlRuleAdminIpExt, RuleIndex};
use fidl_fuchsia_net_routes_ext::{self as fnet_routes_ext, FidlRouteIpExt};
use net_types::SpecifiedAddr;
use net_types::ip::Ip;
use netstack_testing_common::realms::{Netstack3, TestSandboxExt as _};

/// Common test setup that can be shared by all routes tests.
pub struct TestSetup<'a, I: Ip + FidlRouteIpExt + FidlRouteAdminIpExt> {
    pub realm: netemul::TestRealm<'a>,
    pub network: netemul::TestNetwork<'a>,
    pub interface: netemul::TestInterface<'a>,
    pub route_table: <I::RouteTableMarker as ProtocolMarker>::Proxy,
    pub global_route_table: <I::GlobalRouteTableMarker as ProtocolMarker>::Proxy,
    pub state: <I::StateMarker as ProtocolMarker>::Proxy,
}

impl<'a, I: Ip + FidlRouteIpExt + FidlRouteAdminIpExt> TestSetup<'a, I> {
    /// Creates a new test setup.
    pub async fn new(sandbox: &'a netemul::TestSandbox, name: &str) -> TestSetup<'a, I> {
        let realm = sandbox
            .create_netstack_realm::<Netstack3, _>(format!("routes-admin-{name}"))
            .expect("create realm");
        let network =
            sandbox.create_network(format!("routes-admin-{name}")).await.expect("create network");
        let interface = realm.join_network(&network, "ep1").await.expect("join network");
        let route_table = realm
            .connect_to_protocol::<I::RouteTableMarker>()
            .expect("connect to routes-admin RouteTable");
        let global_route_table = realm
            .connect_to_protocol::<I::GlobalRouteTableMarker>()
            .expect("connect to global route set provider");

        let state = realm.connect_to_protocol::<I::StateMarker>().expect("connect to routes State");
        TestSetup { realm, network, interface, route_table, global_route_table, state }
    }
}

/// A route for testing.
pub fn test_route<I: Ip>(
    interface: &netemul::TestInterface<'_>,
    metric: fnet_routes::SpecifiedMetric,
) -> fnet_routes_ext::Route<I> {
    let destination = I::map_ip(
        (),
        |()| net_declare::net_subnet_v4!("192.0.2.0/24"),
        |()| net_declare::net_subnet_v6!("2001:DB8::/64"),
    );
    let next_hop_addr = I::map_ip(
        (),
        |()| net_declare::net_ip_v4!("192.0.2.1"),
        |()| net_declare::net_ip_v6!("2001:DB8::1"),
    );

    fnet_routes_ext::Route {
        destination,
        action: fnet_routes_ext::RouteAction::Forward(fnet_routes_ext::RouteTarget {
            outbound_interface: interface.id(),
            next_hop: Some(SpecifiedAddr::new(next_hop_addr).expect("is specified")),
        }),
        properties: fnet_routes_ext::RouteProperties {
            specified_properties: fnet_routes_ext::SpecifiedRouteProperties { metric },
        },
    }
}

/// Possible mark matcher settings for a rule.
pub enum MarkMatcher {
    /// The matcher is not present, it does not match on the marks.
    DontMatch,
    /// The matcher only matches when the mark is not present.
    MatchUnmarked,
    /// The matcher only matches when the mark is set to the given number.
    MatchMarked(u32),
}

impl From<MarkMatcher> for Option<fnet_matchers_ext::Mark> {
    fn from(value: MarkMatcher) -> Self {
        match value {
            MarkMatcher::DontMatch => None,
            MarkMatcher::MatchUnmarked => Some(fnet_matchers_ext::Mark::Unmarked),
            MarkMatcher::MatchMarked(m) => {
                Some(fnet_matchers_ext::Mark::Marked { mask: !0, between: m..=m, invert: false })
            }
        }
    }
}

// Creates a new route table that has a default route using the given
// `interface`; Also installs a rule that matches on the given `mark`
// to lookup the created route table.
pub async fn add_default_route_for_mark<
    I: FidlRouteAdminIpExt + FidlRuleAdminIpExt + FidlRouteIpExt,
>(
    route_table_provider: &<I::RouteTableProviderMarker as ProtocolMarker>::Proxy,
    rule_set: &<I::RuleSetMarker as ProtocolMarker>::Proxy,
    interface: &netemul::TestInterface<'_>,
    next_hop: Option<SpecifiedAddr<I::Addr>>,
    index: u32,
    mark_1: MarkMatcher,
    mark_2: MarkMatcher,
) -> <I::RouteSetMarker as ProtocolMarker>::Proxy {
    let route_table = fnet_routes_ext::admin::new_route_table::<I>(route_table_provider, None)
        .expect("new route table");
    let route_set =
        fnet_routes_ext::admin::new_route_set::<I>(&route_table).expect("new route set");
    let route_to_add = fnet_routes_ext::Route {
        destination: I::ALL_ADDRS_SUBNET,
        action: fnet_routes_ext::RouteAction::Forward(fnet_routes_ext::RouteTarget::<I> {
            outbound_interface: interface.id(),
            next_hop,
        }),
        properties: fnet_routes_ext::RouteProperties {
            specified_properties: fnet_routes_ext::SpecifiedRouteProperties {
                metric: fnet_routes::SpecifiedMetric::InheritedFromInterface(fnet_routes::Empty),
            },
        },
    };
    let grant = interface.get_authorization().await.expect("getting grant should succeed");
    let proof = fnet_interfaces_ext::admin::proof_from_grant(&grant);
    fnet_routes_ext::admin::authenticate_for_interface::<I>(&route_set, proof)
        .await
        .expect("no FIDL error")
        .expect("authentication should succeed");
    assert!(
        fnet_routes_ext::admin::add_route::<I>(
            &route_set,
            &route_to_add.try_into().expect("convert to FIDL")
        )
        .await
        .expect("fidl")
        .expect("add route")
    );
    fnet_routes_ext::admin::detach_route_table::<I>(&route_table).await.expect("fidl error");

    let table_id =
        fnet_routes_ext::admin::get_table_id::<I>(&route_table).await.expect("fidl error");

    let auth = fnet_routes_ext::admin::get_authorization_for_route_table::<I>(&route_table)
        .await
        .expect("fidl error");
    fnet_routes_ext::rules::authenticate_for_route_table::<I>(&rule_set, auth.table_id, auth.token)
        .await
        .expect("fidl error")
        .expect("authentication error");
    fnet_routes_ext::rules::add_rule::<I>(
        &rule_set,
        RuleIndex::new(index),
        fnet_routes_ext::rules::RuleMatcher {
            mark_1: mark_1.into(),
            mark_2: mark_2.into(),
            ..Default::default()
        },
        fnet_routes_ext::rules::RuleAction::Lookup(table_id),
    )
    .await
    .expect("fidl")
    .expect("add rule");

    route_set
}
