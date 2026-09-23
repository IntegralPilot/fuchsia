// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assert_matches::assert_matches;
use fidl_fuchsia_net_routes_ext::{self as fnet_routes_ext, FidlRouteIpExt};
use fnet_routes_ext::admin::FidlRouteAdminIpExt;
use fnet_routes_ext::rules::{
    DEFAULT_RULE_SET_PRIORITY, FidlRuleAdminIpExt, FidlRuleIpExt, InstalledRule, RuleAction,
    RuleEvent, RuleIndex, RuleMatcher, add_rule, close_rule_set, new_rule_set, remove_rule,
};
use futures::{StreamExt as _, TryStreamExt as _};
use net_declare::{fidl_ip, fidl_subnet, net_ip_v4, net_ip_v6};
use net_types::SpecifiedAddr;
use net_types::ip::Ip;
use netstack_testing_common::realms::{Netstack3, TestSandboxExt};
use netstack_testing_macros::netstack_test;
use std::pin::pin;

// Verifies the watcher protocols correctly report `added` and `removed` events.
#[netstack_test]
#[variant(I, Ip)]
async fn rule_watcher_add_remove<
    I: Ip + FidlRouteIpExt + FidlRouteAdminIpExt + FidlRuleIpExt + FidlRuleAdminIpExt,
>(
    name: &str,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    // Connect to the watcher protocol and consume all existing events.
    let state_proxy =
        realm.connect_to_protocol::<I::StateMarker>().expect("failed to connect to routes/State");
    let event_stream = fnet_routes_ext::rules::rule_event_stream_from_state::<I>(&state_proxy)
        .expect("failed to connect to routes watcher");
    let mut event_stream = pin!(event_stream);

    // There should be a default rule that points to the main table.
    let main_table =
        realm.connect_to_protocol::<I::RouteTableMarker>().expect("connect to main route table");
    let main_table_id =
        fnet_routes_ext::admin::get_table_id::<I>(&main_table).await.expect("get main table id");

    let default_rule = assert_matches!(
        event_stream.try_next().await,
        Ok(Some(RuleEvent::Existing(existing))) => existing
    );
    assert_eq!(
        default_rule,
        InstalledRule {
            priority: DEFAULT_RULE_SET_PRIORITY,
            index: RULE_INDEX_0,
            matcher: RuleMatcher::default(),
            action: RuleAction::Lookup(main_table_id),
        }
    );

    assert_matches!(event_stream.try_next().await, Ok(Some(RuleEvent::Idle)));

    let rule_table =
        realm.connect_to_protocol::<I::RuleTableMarker>().expect("connect to rule table");
    let priority = fnet_routes_ext::rules::RuleSetPriority::from(0);
    let rule_set = new_rule_set::<I>(&rule_table, priority).expect("fidl error");

    const RULE_INDEX_0: RuleIndex = RuleIndex::new(0);
    const RULE_INDEX_1: RuleIndex = RuleIndex::new(1);

    add_rule::<I>(&rule_set, RULE_INDEX_0, RuleMatcher::default(), RuleAction::Unreachable)
        .await
        .expect("fidl error")
        .expect("failed to add a new rule");
    let added = assert_matches!(
        event_stream.try_next().await,
        Ok(Some(RuleEvent::Added(added))) => added
    );
    assert_eq!(
        added,
        InstalledRule {
            priority,
            index: RULE_INDEX_0,
            matcher: RuleMatcher::default(),
            action: RuleAction::Unreachable,
        }
    );

    add_rule::<I>(&rule_set, RULE_INDEX_1, RuleMatcher::default(), RuleAction::Unreachable)
        .await
        .expect("fidl error")
        .expect("failed to add a new rule");

    let added = assert_matches!(
        event_stream.try_next().await,
        Ok(Some(RuleEvent::Added(added))) => added
    );
    assert_eq!(
        added,
        InstalledRule {
            priority,
            index: RULE_INDEX_1,
            matcher: RuleMatcher::default(),
            action: RuleAction::Unreachable,
        }
    );

    remove_rule::<I>(&rule_set, RULE_INDEX_0)
        .await
        .expect("fidl error")
        .expect("failed to remove an installed rule");
    let removed = assert_matches!(
        event_stream.try_next().await,
        Ok(Some(RuleEvent::Removed(removed))) => removed
    );
    assert_eq!(
        removed,
        InstalledRule {
            priority,
            index: RULE_INDEX_0,
            matcher: RuleMatcher::default(),
            action: RuleAction::Unreachable,
        }
    );

    close_rule_set::<I>(rule_set).await.expect("failed to remove rule set");
    let removed = assert_matches!(
        event_stream.try_next().await,
        Ok(Some(RuleEvent::Removed(removed))) => removed
    );
    assert_eq!(
        removed,
        InstalledRule {
            priority,
            index: RULE_INDEX_1,
            matcher: RuleMatcher::default(),
            action: RuleAction::Unreachable,
        }
    )
}

#[netstack_test]
#[variant(I, Ip)]
async fn resolve_with_marks<
    I: Ip + FidlRouteIpExt + FidlRouteAdminIpExt + FidlRuleIpExt + FidlRuleAdminIpExt,
>(
    name: &str,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let table_provider = realm
        .connect_to_protocol::<I::RouteTableProviderMarker>()
        .expect("failed to connect to the route table provider");

    let rules_table = realm
        .connect_to_protocol::<I::RuleTableMarker>()
        .expect("failed to connect to the rule table");
    let rs = fnet_routes_ext::rules::new_rule_set::<I>(
        &rules_table,
        fnet_routes_ext::rules::RuleSetPriority::new(0),
    )
    .expect("failed to create new rule set");
    let net = sandbox.create_network("net").await.expect("create network");
    let neighbor_control = realm
        .connect_to_protocol::<fidl_fuchsia_net_neighbor::ControllerMarker>()
        .expect("failed to connect to neighbor controller");
    const GATEWAY_MAC: fidl_fuchsia_net::MacAddress = net_declare::fidl_mac!("02:11:22:33:44:55");
    let gateway_addr_fidl = match I::VERSION {
        net_types::ip::IpVersion::V4 => fidl_ip!("192.168.0.100"),
        net_types::ip::IpVersion::V6 => fidl_ip!("fd00:0:0:1::100"),
    };
    let gateway_addr = SpecifiedAddr::new(I::map_ip(
        (),
        |()| net_ip_v4!("192.168.0.100"),
        |()| net_ip_v6!("fd00:0:0:1::100"),
    ));

    let mark_values = [None, Some(0), Some(1)];
    let marks = mark_values.iter().flat_map(|m1| mark_values.iter().map(|m2| (*m1, *m2)));
    let marks_and_expected_iface = futures::stream::iter(marks.enumerate())
        .then(async |(i, (m1, m2))| {
            let mut subnet = match I::VERSION {
                net_types::ip::IpVersion::V4 => fidl_subnet!("192.168.0.0/24"),
                net_types::ip::IpVersion::V6 => fidl_subnet!("fd00:0:0:1::/64"),
            };
            let addr = match &mut subnet.addr {
                fidl_fuchsia_net::IpAddress::Ipv4(fidl_fuchsia_net::Ipv4Address { addr }) => {
                    &mut addr[..]
                }
                fidl_fuchsia_net::IpAddress::Ipv6(fidl_fuchsia_net::Ipv6Address { addr }) => {
                    &mut addr[..]
                }
            };
            *addr.last_mut().unwrap() = (i + 1).try_into().unwrap();
            let iface = realm
                .join_network(&net, format!("ep-{}", i))
                .await
                .expect("install interface in netstack");
            iface.add_address_and_subnet_route(subnet).await.expect("configure address");
            neighbor_control
                .add_entry(iface.id(), &gateway_addr_fidl, &GATEWAY_MAC)
                .await
                .expect("fidl failed")
                .expect("failed to add static entry");
            let route_set = routes_common::add_default_route_for_mark::<I>(
                &table_provider,
                &rs,
                &iface,
                gateway_addr,
                i.try_into().unwrap(),
                m1.map_or(
                    routes_common::MarkMatcher::MatchUnmarked,
                    routes_common::MarkMatcher::MatchMarked,
                ),
                m2.map_or(
                    routes_common::MarkMatcher::MatchUnmarked,
                    routes_common::MarkMatcher::MatchMarked,
                ),
            )
            .await;
            ((m1, m2), (iface, subnet.addr, route_set))
        })
        .collect::<Vec<_>>()
        .await;

    let state_proxy = realm
        .connect_to_protocol::<fidl_fuchsia_net_routes::StateMarker>()
        .expect("failed to connect to routes/State");
    let dest = match I::VERSION {
        net_types::ip::IpVersion::V4 => fidl_ip!("0.0.0.0"),
        net_types::ip::IpVersion::V6 => fidl_ip!("::"),
    };
    for ((mark_1, mark_2), (iface, source_ip, _rs)) in marks_and_expected_iface {
        let resolved = state_proxy
            .resolve2(
                &dest,
                &fnet_routes_ext::ResolveOptions {
                    marks: fidl_fuchsia_net_ext::Marks { mark_1, mark_2 },
                }
                .into(),
            )
            .await
            .expect("fidl failed")
            .expect("failed to resolve");
        assert_eq!(
            resolved,
            fidl_fuchsia_net_routes::ResolveResult::Gateway(fidl_fuchsia_net_routes::Destination {
                address: Some(gateway_addr_fidl),
                mac: Some(GATEWAY_MAC),
                interface_id: Some(iface.id()),
                source_address: Some(source_ip),
                __source_breaking: fidl::marker::SourceBreaking,
            })
        );
    }
}
