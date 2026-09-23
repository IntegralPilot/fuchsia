// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Counters kept in bindings context.

use net_types::ip::{GenericOverIp, Ip, IpVersionMarker, Ipv4, Ipv6};
use netstack3_core::inspect::{Inspectable, Inspector, InspectorExt as _};
use netstack3_core::types::Counter;

/// Holder for generic bindings counters.
#[derive(Debug, Default)]
pub(crate) struct BindingsCounters {
    pub(crate) power: PowerCounters,
    multicast_admin_v4: MulticastAdminCounters<Ipv4>,
    multicast_admin_v6: MulticastAdminCounters<Ipv6>,
}

impl BindingsCounters {
    pub(crate) fn multicast_admin<I: Ip>(&self) -> &MulticastAdminCounters<I> {
        I::map_ip_out(
            self,
            |bindings_counters| &bindings_counters.multicast_admin_v4,
            |bindings_counters| &bindings_counters.multicast_admin_v6,
        )
    }
}

impl Inspectable for BindingsCounters {
    fn record<I: Inspector>(&self, inspector: &mut I) {
        let Self { power, multicast_admin_v4, multicast_admin_v6 } = self;
        inspector.record_inspectable("Power", power);
        inspector.record_child("MulticastAdmin", |inspector| {
            inspector.record_inspectable("V4", multicast_admin_v4);
            inspector.record_inspectable("V6", multicast_admin_v6);
        });
    }
}

/// Holder for power-related counters.
#[derive(Debug, Default)]
pub(crate) struct PowerCounters {
    pub(crate) dropped_rx_leases: Counter,
}

impl Inspectable for PowerCounters {
    fn record<I: Inspector>(&self, inspector: &mut I) {
        let Self { dropped_rx_leases } = self;
        inspector.record_counter("DroppedRxLeases", dropped_rx_leases);
    }
}

/// Holder for fuchsia.net.multicast.admin related counters.
#[derive(Debug, Default, GenericOverIp)]
#[generic_over_ip(I, Ip)]
pub(crate) struct MulticastAdminCounters<I: Ip> {
    _marker: IpVersionMarker<I>,

    /// The number of routing events that were dropped before being published to
    /// a client via `WatchRoutingEvents`.
    pub(crate) dropped_routing_events: Counter,
}

impl<I: Ip> Inspectable for MulticastAdminCounters<I> {
    fn record<II: Inspector>(&self, inspector: &mut II) {
        let Self { _marker, dropped_routing_events } = self;
        inspector.record_counter("DroppedRoutingEvents", dropped_routing_events);
    }
}
