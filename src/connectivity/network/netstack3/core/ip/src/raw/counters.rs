// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Declares counters for observability/debugging of raw IP sockets.

use net_types::ip::{Ip, IpVersionMarker};
use netstack3_base::{Counter, Inspectable, Inspector, InspectorExt};

/// Raw IP socket counters.
///
/// These counters are used both on a per-socket basis, and in a stack-wide
/// aggregate.
#[derive(Default)]
pub struct RawIpSocketCounters<I: Ip> {
    _marker: IpVersionMarker<I>,
    /// Count of incoming IP packets that were delivered to the socket.
    ///
    /// Note that a single IP packet may be delivered to multiple raw IP
    /// sockets. Thus this counter, when tracking the stack-wide aggregate, may
    /// exceed the total number of IP packets received by the stack.
    pub(super) rx_packets: Counter,
    /// Count of incoming IP packets that could not be delivered to a socket
    /// because its receive buffer was full.
    pub rx_queue_full: Counter,
    /// Count of outgoing IP packets that were sent by the socket.
    pub(super) tx_packets: Counter,
    /// Count of incoming IP packets that were not delivered due to an invalid
    /// checksum.
    pub(super) rx_checksum_errors: Counter,
    /// Count of outgoing IP packets that were not sent because a checksum could
    /// not be computed.
    pub(super) tx_checksum_errors: Counter,
    /// Count of incoming IP packets that were not delivered because they failed
    /// the socket's ICMP filter.
    pub(super) rx_icmp_filtered: Counter,
}

impl<I: Ip> Inspectable for RawIpSocketCounters<I> {
    fn record<II: Inspector>(&self, inspector: &mut II) {
        let Self {
            _marker,
            rx_packets,
            rx_queue_full,
            tx_packets,
            rx_checksum_errors,
            tx_checksum_errors,
            rx_icmp_filtered,
        } = self;
        inspector.record_child("Rx", |inspector| {
            inspector.record_counter("DeliveredPackets", rx_packets);
            inspector.record_counter("ChecksumErrors", rx_checksum_errors);
            inspector.record_counter("IcmpPacketsFiltered", rx_icmp_filtered);
            inspector.record_counter("DroppedQueueFull", rx_queue_full);
        });
        inspector.record_child("Tx", |inspector| {
            inspector.record_counter("SentPackets", tx_packets);
            inspector.record_counter("ChecksumErrors", tx_checksum_errors);
        });
    }
}
