// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Declares counters for observability/debugging of multicast forwarding.

use net_types::ip::{Ip, IpVersionMarker};
use netstack3_base::{Counter, Inspectable, Inspector, InspectorExt as _};

/// Multicast Forwarding counters.
#[derive(Default)]
pub struct MulticastForwardingCounters<I: Ip> {
    _marker: IpVersionMarker<I>,

    /// Count of packets that were received and consulted the multicast
    /// forwarding engine.
    pub(super) rx: Counter,
    /// Count of packets that were forwarded.
    ///
    /// This does not include packets that were forwarded from the pending table
    /// (i.e. `pending_packet_tx`).
    ///
    /// Note this counter is incremented once per packet, not once per target.
    /// Forwarding a packet via a route with multiple targets will only increase
    /// this counter by 1.
    pub(super) tx: Counter,
    /// Count of packets that were not forwarded because their src/dst addresses
    /// did not constitute a valid multicast routing key.
    pub(super) no_tx_invalid_key: Counter,
    /// Count of packets that were not forwarded because their input device had
    /// multicast forwarding disabled.
    pub(super) no_tx_disabled_dev: Counter,
    /// Count of packets that were not forwarded because multicast forwarding
    /// is disabled stack wide.
    pub(super) no_tx_disabled_stack_wide: Counter,
    /// Count of packets that were not forwarded because their input device
    /// does not match the route's expected input device.
    pub(super) no_tx_wrong_dev: Counter,
    /// Count of packets that did not have an applicable route and were
    /// (attempted to be) added to the pending table.
    pub(super) pending_packets: Counter,
    /// Count of packets that failed to be added to the pending table because
    /// the queue was full.
    pub(super) pending_packet_drops_queue_full: Counter,
    /// Count of pending packets that were forwarded.
    pub(super) pending_packet_tx: Counter,
    /// Count of pending packets that were dropped because their input device
    /// had multicast forwarding disabled.
    pub(super) pending_packet_drops_disabled_dev: Counter,
    /// Count of pending packets that were dropped because their input device
    /// does not match the route's expected input device.
    pub(super) pending_packet_drops_wrong_dev: Counter,
    /// Count of pending packets that were garbage collected before an
    /// applicable route was installed.
    pub(super) pending_packet_drops_gc: Counter,
    /// Count of times the pending table has been garbage collected.
    pub(super) pending_table_gc: Counter,
}

impl<I: Ip> Inspectable for MulticastForwardingCounters<I> {
    fn record<II: Inspector>(&self, inspector: &mut II) {
        let Self {
            _marker,
            rx,
            tx,
            no_tx_invalid_key,
            no_tx_disabled_dev,
            no_tx_disabled_stack_wide,
            no_tx_wrong_dev,
            pending_packets,
            pending_packet_drops_queue_full,
            pending_packet_tx,
            pending_packet_drops_disabled_dev,
            pending_packet_drops_wrong_dev,
            pending_packet_drops_gc,
            pending_table_gc,
        } = self;
        inspector.record_counter("PacketsReceived", rx);
        inspector.record_counter("PacketsForwarded", tx);
        inspector.record_child("PacketsNotForwardedWithReason", |inspector| {
            inspector.record_counter("InvalidKey", no_tx_invalid_key);
            inspector.record_counter("ForwardingDisabledOnInputDevice", no_tx_disabled_dev);
            inspector.record_counter("ForwardingDisabledForStack", no_tx_disabled_stack_wide);
            inspector.record_counter("WrongInputDevice", no_tx_wrong_dev);
        });
        inspector.record_counter("PendingPackets", pending_packets);
        inspector.record_counter("PendingPacketsForwarded", pending_packet_tx);
        inspector.record_child("PendingPacketsNotForwardedWithReason", |inspector| {
            inspector.record_counter("QueueFull", pending_packet_drops_queue_full);
            inspector.record_counter(
                "ForwardingDisabledOnInputDevice",
                pending_packet_drops_disabled_dev,
            );
            inspector.record_counter("WrongInputDevice", pending_packet_drops_wrong_dev);
            inspector.record_counter("GarbageCollected", pending_packet_drops_gc);
        });
        inspector.record_counter("PendingTableGcRuns", pending_table_gc);
    }
}
