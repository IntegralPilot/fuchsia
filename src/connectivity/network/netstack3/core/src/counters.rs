// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Types for working with and exposing packet statistic counters.

use net_types::ip::{Ip, Ipv4, Ipv6};
use netstack3_base::{ContextPair, CounterContext, Inspector, InspectorExt as _};
use netstack3_device::ethernet::EthernetDeviceCounters;
use netstack3_device::socket::DeviceSocketCounters;
use netstack3_device::{ArpCounters, DeviceCounters};
use netstack3_ip::IpCounters;
use netstack3_ip::gmp::{IgmpCounters, MldCounters};
use netstack3_ip::icmp::{
    IcmpRxCounters, IcmpTxCounters, NdpCounters, NdpRxCounters, NdpTxCounters,
};
use netstack3_ip::multicast_forwarding::MulticastForwardingCounters;
use netstack3_ip::nud::{NudCounters, NudCountersInner};
use netstack3_ip::raw::RawIpSocketCounters;
use netstack3_tcp::{CombinedTcpCounters, TcpCountersWithSocket, TcpCountersWithoutSocket};
use netstack3_udp::{CombinedUdpCounters, UdpCountersWithSocket, UdpCountersWithoutSocket};

/// An API struct for accessing all stack counters.
pub struct CountersApi<C>(C);

impl<C> CountersApi<C> {
    pub(crate) fn new(ctx: C) -> Self {
        Self(ctx)
    }
}

impl<C> CountersApi<C>
where
    C: ContextPair,
    C::CoreContext: CounterContext<IpCounters<Ipv4>>
        + CounterContext<IpCounters<Ipv6>>
        + CounterContext<MulticastForwardingCounters<Ipv4>>
        + CounterContext<MulticastForwardingCounters<Ipv6>>
        + CounterContext<RawIpSocketCounters<Ipv4>>
        + CounterContext<RawIpSocketCounters<Ipv6>>
        + CounterContext<UdpCountersWithSocket<Ipv4>>
        + CounterContext<UdpCountersWithSocket<Ipv6>>
        + CounterContext<UdpCountersWithoutSocket<Ipv4>>
        + CounterContext<UdpCountersWithoutSocket<Ipv6>>
        + CounterContext<TcpCountersWithSocket<Ipv4>>
        + CounterContext<TcpCountersWithSocket<Ipv6>>
        + CounterContext<TcpCountersWithoutSocket<Ipv4>>
        + CounterContext<TcpCountersWithoutSocket<Ipv6>>
        + CounterContext<IcmpRxCounters<Ipv4>>
        + CounterContext<IcmpRxCounters<Ipv6>>
        + CounterContext<IcmpTxCounters<Ipv4>>
        + CounterContext<IcmpTxCounters<Ipv4>>
        + CounterContext<IcmpTxCounters<Ipv6>>
        + CounterContext<IgmpCounters>
        + CounterContext<MldCounters>
        + CounterContext<NudCounters<Ipv4>>
        + CounterContext<NudCounters<Ipv6>>
        + CounterContext<NdpCounters>
        + CounterContext<ArpCounters>
        + CounterContext<DeviceCounters>
        + CounterContext<EthernetDeviceCounters>
        + CounterContext<DeviceSocketCounters>,
{
    fn core_ctx(&mut self) -> &mut C::CoreContext {
        let Self(pair) = self;
        pair.core_ctx()
    }

    /// Exposes all of the stack wide counters through `inspector`.
    pub fn inspect_stack_counters<I: Inspector>(&mut self, inspector: &mut I) {
        inspector.record_child("Device", |inspector| {
            inspector
                .delegate_inspectable(CounterContext::<DeviceCounters>::counters(self.core_ctx()));
            inspector.delegate_inspectable(CounterContext::<EthernetDeviceCounters>::counters(
                self.core_ctx(),
            ));
        });
        inspector.record_child("Arp", |inspector| {
            inspect_arp_counters(inspector, self.core_ctx().counters());
        });
        inspector.record_child("NUD", |inspector| {
            inspector.record_child("V4", |inspector| {
                inspect_nud_counters::<Ipv4>(inspector, self.core_ctx().counters());
            });
            inspector.record_child("V6", |inspector| {
                inspect_nud_counters::<Ipv6>(inspector, self.core_ctx().counters());
            });
        });
        inspector.record_child("ICMP", |inspector| {
            inspector.record_child("V4", |inspector| {
                inspector.record_child("Rx", |inspector| {
                    inspector.delegate_inspectable(
                        CounterContext::<IcmpRxCounters<Ipv4>>::counters(self.core_ctx()),
                    );
                });
                inspector.record_child("Tx", |inspector| {
                    inspector.delegate_inspectable(
                        CounterContext::<IcmpTxCounters<Ipv4>>::counters(self.core_ctx()),
                    );
                });
            });
            inspector.record_child("V6", |inspector| {
                inspector.record_child("Rx", |inspector| {
                    inspector.delegate_inspectable(
                        CounterContext::<IcmpRxCounters<Ipv6>>::counters(self.core_ctx()),
                    );
                    inspector.record_child("NDP", |inspector| {
                        let NdpCounters { rx, tx: _ } = self.core_ctx().counters();
                        inspect_ndp_rx_counters(inspector, rx);
                    })
                });
                inspector.record_child("Tx", |inspector| {
                    inspector.delegate_inspectable(
                        CounterContext::<IcmpTxCounters<Ipv6>>::counters(self.core_ctx()),
                    );
                    inspector.record_child("NDP", |inspector| {
                        let NdpCounters { rx: _, tx } = self.core_ctx().counters();
                        inspect_ndp_tx_counters(inspector, tx);
                    })
                });
            });
        });
        inspector.record_child("IGMP", |inspector| {
            inspector
                .delegate_inspectable(CounterContext::<IgmpCounters>::counters(self.core_ctx()));
        });
        inspector.record_child("MLD", |inspector| {
            inspector
                .delegate_inspectable(CounterContext::<MldCounters>::counters(self.core_ctx()));
        });
        inspector.record_child("IPv4", |inspector| {
            inspector.delegate_inspectable(CounterContext::<IpCounters<Ipv4>>::counters(
                self.core_ctx(),
            ));
        });
        inspector.record_child("IPv6", |inspector| {
            inspector.delegate_inspectable(CounterContext::<IpCounters<Ipv6>>::counters(
                self.core_ctx(),
            ));
        });
        inspector.record_child("MulticastForwarding", |inspector| {
            inspector.record_child("V4", |inspector| {
                inspector.delegate_inspectable(
                    CounterContext::<MulticastForwardingCounters<Ipv4>>::counters(self.core_ctx()),
                );
            });
            inspector.record_child("V6", |inspector| {
                inspector.delegate_inspectable(
                    CounterContext::<MulticastForwardingCounters<Ipv6>>::counters(self.core_ctx()),
                );
            });
        });
        inspector.record_child("DeviceSockets", |inspector| {
            inspector.delegate_inspectable(CounterContext::<DeviceSocketCounters>::counters(
                self.core_ctx(),
            ));
        });
        inspector.record_child("RawIpSockets", |inspector| {
            inspector.record_child("V4", |inspector| {
                inspector.delegate_inspectable(
                    CounterContext::<RawIpSocketCounters<Ipv4>>::counters(self.core_ctx()),
                );
            });
            inspector.record_child("V6", |inspector| {
                inspector.delegate_inspectable(
                    CounterContext::<RawIpSocketCounters<Ipv6>>::counters(self.core_ctx()),
                );
            });
        });
        inspector.record_child("UDP", |inspector| {
            inspector.record_child("V4", |inspector| {
                let ctx = self.core_ctx();
                let with_socket = CounterContext::<UdpCountersWithSocket<Ipv4>>::counters(ctx);
                let without_socket =
                    CounterContext::<UdpCountersWithoutSocket<Ipv4>>::counters(ctx);
                inspector.delegate_inspectable(&CombinedUdpCounters {
                    with_socket,
                    without_socket: Some(without_socket),
                });
            });
            inspector.record_child("V6", |inspector| {
                let ctx = self.core_ctx();
                let with_socket = CounterContext::<UdpCountersWithSocket<Ipv6>>::counters(ctx);
                let without_socket =
                    CounterContext::<UdpCountersWithoutSocket<Ipv6>>::counters(ctx);
                inspector.delegate_inspectable(&CombinedUdpCounters {
                    with_socket,
                    without_socket: Some(without_socket),
                });
            });
        });
        inspector.record_child("TCP", |inspector| {
            inspector.record_child("V4", |inspector| {
                let ctx = self.core_ctx();
                let with_socket = CounterContext::<TcpCountersWithSocket<Ipv4>>::counters(ctx);
                let without_socket =
                    CounterContext::<TcpCountersWithoutSocket<Ipv4>>::counters(ctx);
                inspector.delegate_inspectable(&CombinedTcpCounters {
                    with_socket,
                    without_socket: Some(without_socket),
                });
            });
            inspector.record_child("V6", |inspector| {
                let ctx = self.core_ctx();
                let with_socket = CounterContext::<TcpCountersWithSocket<Ipv6>>::counters(ctx);
                let without_socket =
                    CounterContext::<TcpCountersWithoutSocket<Ipv6>>::counters(ctx);
                inspector.delegate_inspectable(&CombinedTcpCounters {
                    with_socket,
                    without_socket: Some(without_socket),
                });
            });
        });
    }
}

fn inspect_nud_counters<I: Ip>(inspector: &mut impl Inspector, counters: &NudCounters<I>) {
    let NudCountersInner { icmp_dest_unreachable_dropped } = counters.as_ref();
    inspector.record_counter("IcmpDestUnreachableDropped", icmp_dest_unreachable_dropped);
}

fn inspect_arp_counters(inspector: &mut impl Inspector, counters: &ArpCounters) {
    let ArpCounters {
        rx_dropped_non_local_target,
        rx_malformed_packets,
        rx_echoed_packets,
        rx_packets,
        rx_requests,
        rx_responses,
        tx_requests,
        tx_requests_dropped_no_local_addr,
        tx_responses,
    } = counters;
    inspector.record_child("Rx", |inspector| {
        inspector.record_counter("TotalPackets", rx_packets);
        inspector.record_counter("Requests", rx_requests);
        inspector.record_counter("Responses", rx_responses);
        inspector.record_counter("Malformed", rx_malformed_packets);
        inspector.record_counter("Echoed", rx_echoed_packets);
        inspector.record_counter("NonLocalDstAddr", rx_dropped_non_local_target);
    });
    inspector.record_child("Tx", |inspector| {
        inspector.record_counter("Requests", tx_requests);
        inspector.record_counter("RequestsNonLocalSrcAddr", tx_requests_dropped_no_local_addr);
        inspector.record_counter("Responses", tx_responses);
    });
}

fn inspect_ndp_tx_counters(inspector: &mut impl Inspector, counters: &NdpTxCounters) {
    let NdpTxCounters { neighbor_advertisement, neighbor_solicitation } = counters;
    inspector.record_counter("NeighborAdvertisement", neighbor_advertisement);
    inspector.record_counter("NeighborSolicitation", neighbor_solicitation);
}

fn inspect_ndp_rx_counters(inspector: &mut impl Inspector, counters: &NdpRxCounters) {
    let NdpRxCounters {
        neighbor_solicitation,
        neighbor_advertisement,
        router_advertisement,
        router_solicitation,
    } = counters;
    inspector.record_counter("NeighborSolicitation", neighbor_solicitation);
    inspector.record_counter("NeighborAdvertisement", neighbor_advertisement);
    inspector.record_counter("RouterSolicitation", router_solicitation);
    inspector.record_counter("RouterAdvertisement", router_advertisement);
}
