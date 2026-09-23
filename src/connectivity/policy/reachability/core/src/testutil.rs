// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::HashSet;

use crate::route_table::{Route, RouteTable};

use assert_matches::assert_matches;
use fidl_fuchsia_net_ext::IntoExt;
use net_types::{ip, SpecifiedAddr};
use {
    fidl_fuchsia_net as fnet, fidl_fuchsia_net_routes as fnet_routes,
    fidl_fuchsia_net_routes_ext as fnet_routes_ext,
};

// Converts the given `Route` into an `InstalledRoute` of the given `Ip`,
// panicking if the protocol versions don't match. The extra data held by the
// `InstalledRoute` is arbitrarily populated, since it's not relevant for
// reachability tests.
fn to_installed_route<I: ip::Ip>(
    Route { destination, outbound_interface, next_hop }: Route,
) -> fnet_routes_ext::InstalledRoute<I> {
    const ARBITRARY_METRIC: u32 = 1;
    let destination = I::map_ip_out(
        destination,
        |destination| {
            assert_matches!(
                destination.addr,
                fnet::IpAddress::Ipv4(network) => ip::Subnet::new(
                    network.into_ext(), destination.prefix_len).expect("invalid subnet"))
        },
        |destination| {
            assert_matches!(
                destination.addr,
                fnet::IpAddress::Ipv6(network) => ip::Subnet::new(
                    network.into_ext(), destination.prefix_len).expect("invalid subnet"))
        },
    );
    let next_hop: Option<SpecifiedAddr<I::Addr>> = next_hop.map(|next_hop| {
        SpecifiedAddr::new(I::map_ip_out(
            next_hop,
            |next_hop| {
                assert_matches!(next_hop, fnet::IpAddress::Ipv4(next_hop) => next_hop.into_ext())
            },
            |next_hop| {
                assert_matches!(next_hop, fnet::IpAddress::Ipv6(next_hop) => next_hop.into_ext())
            },
        ))
        .expect("next hop was unspecified")
    });
    fnet_routes_ext::InstalledRoute {
        route: fnet_routes_ext::Route {
            destination,
            action: fnet_routes_ext::RouteAction::Forward(fnet_routes_ext::RouteTarget {
                outbound_interface,
                next_hop,
            }),
            properties: fnet_routes_ext::RouteProperties {
                specified_properties: fnet_routes_ext::SpecifiedRouteProperties {
                    metric: fnet_routes::SpecifiedMetric::ExplicitMetric(ARBITRARY_METRIC),
                },
            },
        },
        effective_properties: fnet_routes_ext::EffectiveRouteProperties {
            metric: ARBITRARY_METRIC,
        },
        table_id: fnet_routes_ext::TableId::new(0),
    }
}

// Builds a `RouteTable` from a single Iterator of `Routes` (interspersed IPv4 &
// IPv6 routes).
pub(crate) fn build_route_table_from_flattened_routes(
    routes: impl IntoIterator<Item = Route>,
) -> RouteTable {
    let (v4_routes, v6_routes) = routes.into_iter().partition::<Vec<_>, _>(
        |Route { destination, outbound_interface: _, next_hop }| match (destination.addr, next_hop)
        {
            (fnet::IpAddress::Ipv4(_), Some(fnet::IpAddress::Ipv4(_)))
            | (fnet::IpAddress::Ipv4(_), None) => true,
            (fnet::IpAddress::Ipv6(_), Some(fnet::IpAddress::Ipv6(_)))
            | (fnet::IpAddress::Ipv6(_), None) => false,
            (destination, Some(next_hop)) => panic!(
                "route's destination and next_hop are different IP versions: {:?}, {:?}",
                destination, next_hop
            ),
        },
    );

    let v4_routes = v4_routes
        .into_iter()
        .map(|route| to_installed_route::<ip::Ipv4>(route))
        .collect::<HashSet<_>>();
    let v6_routes = v6_routes
        .into_iter()
        .map(|route| to_installed_route::<ip::Ipv6>(route))
        .collect::<HashSet<_>>();
    RouteTable::new_with_existing_routes(v4_routes, v6_routes)
}
