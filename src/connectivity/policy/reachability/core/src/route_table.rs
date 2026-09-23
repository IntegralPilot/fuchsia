// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_net_ext::IntoExt;
use log::error;
use net_types::ip::{GenericOverIp, Ip, IpAddress, Ipv4, Ipv6};
use std::collections::HashSet;
use {fidl_fuchsia_net as fnet, fidl_fuchsia_net_routes_ext as fnet_routes_ext};

/// Keeps track of the current routing state, as observed from the Netstack
/// `fuchsia.net.routes/WatcherV{4,6}` protocols.
#[derive(Debug, Default)]
pub struct RouteTable {
    v4_routes: HashSet<fnet_routes_ext::InstalledRoute<Ipv4>>,
    v6_routes: HashSet<fnet_routes_ext::InstalledRoute<Ipv6>>,
}

/// A flattened version of [`fnet_routes_ext::InstalledRoute`] that drops fields
/// irrelevant for Reachability and converts address types to `fnet`.
#[derive(Debug, PartialEq)]
pub(crate) struct Route {
    pub(crate) destination: fnet::Subnet,
    pub(crate) outbound_interface: u64,
    pub(crate) next_hop: Option<fnet::IpAddress>,
}

pub(crate) struct RouteActionNotForwardingError {}

// Implement conversions from `InstalledRoute` to `Route` which is
// fallible iff, the route's action is not `Forward`.
impl<I: Ip> TryFrom<fnet_routes_ext::InstalledRoute<I>> for Route {
    type Error = RouteActionNotForwardingError;
    fn try_from(
        fnet_routes_ext::InstalledRoute {
            route: fnet_routes_ext::Route { destination, action, properties: _ },
            effective_properties: _,
            // TODO(https://fxbug.dev/342634598): Reachability monitor should
            // use the route table for the network it is monitoring.
            table_id: _,
        }: fnet_routes_ext::InstalledRoute<I>,
    ) -> Result<Self, Self::Error> {
        match action {
            fnet_routes_ext::RouteAction::Forward(fnet_routes_ext::RouteTarget {
                outbound_interface,
                next_hop,
            }) => Ok(Route {
                destination: destination.into_ext(),
                outbound_interface,
                next_hop: next_hop.map(|next_hop| (*next_hop).to_ip_addr().into_ext()),
            }),
            fnet_routes_ext::RouteAction::Unknown => Err(RouteActionNotForwardingError {}),
        }
    }
}

// Implement optional conversions from `InstalledRoute` to `Route`.
// `Ok` becomes `Some`, while `Err` is logged and becomes `None`.
impl Route {
    fn optionally_from<I: Ip>(route: fnet_routes_ext::InstalledRoute<I>) -> Option<Route> {
        match route.try_into() {
            Ok(route) => Some(route),
            Err(RouteActionNotForwardingError {}) => {
                error!("Unexpected route in routing table: {:?}", route);
                None
            }
        }
    }
}

impl RouteTable {
    /// Constructs a new RouteTable with the provided existing routes.
    pub fn new_with_existing_routes(
        v4_routes: HashSet<fnet_routes_ext::InstalledRoute<Ipv4>>,
        v6_routes: HashSet<fnet_routes_ext::InstalledRoute<Ipv6>>,
    ) -> RouteTable {
        return RouteTable { v4_routes, v6_routes };
    }

    // Returns a mutable reference to the underlying route table for the
    // specified IP Protocol.
    fn get_ip_specific_table_mut<I: Ip>(
        &mut self,
    ) -> &mut HashSet<fnet_routes_ext::InstalledRoute<I>> {
        #[derive(GenericOverIp)]
        #[generic_over_ip(I, Ip)]
        struct TableHolder<'a, I: Ip>(&'a mut HashSet<fnet_routes_ext::InstalledRoute<I>>);
        let TableHolder(ip_specific_table) = I::map_ip_out(
            self,
            |route_table| TableHolder(&mut route_table.v4_routes),
            |route_table| TableHolder(&mut route_table.v6_routes),
        );
        ip_specific_table
    }

    // Adds the given route to this `RouteTable`; returns false if the route
    // was already present, true otherwise.
    pub fn add_route<I: Ip>(&mut self, route: fnet_routes_ext::InstalledRoute<I>) -> bool {
        self.get_ip_specific_table_mut::<I>().insert(route)
    }

    // Removes the given route from this `RouteTable`; returns false if the
    // route was not present, true otherwise.
    pub fn remove_route<I: Ip>(&mut self, route: &fnet_routes_ext::InstalledRoute<I>) -> bool {
        self.get_ip_specific_table_mut::<I>().remove(route)
    }

    /// Returns an Iterator of all routes via the given interface.
    pub(crate) fn device_routes(&self, device: u64) -> impl Iterator<Item = Route> + '_ {
        let RouteTable { v4_routes, v6_routes } = self;
        v4_routes
            .iter()
            .filter_map(|route| Route::optionally_from(*route))
            .chain(v6_routes.iter().filter_map(|route| Route::optionally_from(*route)))
            .filter(move |Route { destination: _, outbound_interface, next_hop: _ }| {
                *outbound_interface == device
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;
    use net_declare::fidl_subnet;
    use test_case::test_case;

    #[test_case([], 1, []; "empty route table")]
    #[test_case(
        [Route{
            destination: fidl_subnet!("192.0.2.0/24"),
            outbound_interface: 2,
            next_hop: None
        },
        Route{
            destination: fidl_subnet!("2001:db8::/32"),
            outbound_interface: 2,
            next_hop: None
        }],
        1,
        [];
        "0 device routes")]
    #[test_case(
        [Route{
            destination: fidl_subnet!("192.0.2.0/24"),
            outbound_interface: 1,
            next_hop: None
        },
        Route{
            destination: fidl_subnet!("2001:db8::/32"),
            outbound_interface: 2,
            next_hop: None
        }],
        1,
        [Route{
            destination: fidl_subnet!("192.0.2.0/24"),
            outbound_interface: 1,
            next_hop: None
        }];
        "IPv4 device route")]
    #[test_case(
        [Route{
            destination: fidl_subnet!("192.0.2.0/24"),
            outbound_interface: 2,
            next_hop: None
        },
        Route{
            destination: fidl_subnet!("2001:db8::/32"),
            outbound_interface: 1,
            next_hop: None
        }],
        1,
        [Route{
            destination: fidl_subnet!("2001:db8::/32"),
            outbound_interface: 1,
            next_hop: None
        }];
        "IPv6 device route")]
    #[test_case(
        [Route{
            destination: fidl_subnet!("192.0.2.0/24"),
            outbound_interface: 1,
            next_hop: None
        },
        Route{
            destination: fidl_subnet!("2001:db8::/32"),
            outbound_interface: 1,
            next_hop: None
        }],
        1,
        [Route{
            destination: fidl_subnet!("192.0.2.0/24"),
            outbound_interface: 1,
            next_hop: None
        },
        Route{
            destination: fidl_subnet!("2001:db8::/32"),
            outbound_interface: 1,
            next_hop: None
        }];
        "IPv4 & IPv6 device routes")]
    fn device_routes(
        all_routes: impl IntoIterator<Item = Route>,
        device_id: u64,
        expected_routes: impl IntoIterator<Item = Route>,
    ) {
        assert_eq!(
            testutil::build_route_table_from_flattened_routes(all_routes)
                .device_routes(device_id)
                .collect::<Vec<_>>(),
            expected_routes.into_iter().collect::<Vec<_>>()
        );
    }
}
