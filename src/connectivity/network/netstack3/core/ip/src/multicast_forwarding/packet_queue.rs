// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Declares types and functionality related to queued multicast packets.

use alloc::vec::Vec;
use arrayvec::ArrayVec;
use core::time::Duration;
use derivative::Derivative;
use lru_cache::LruCache;
use net_types::ip::{Ip, IpVersionMarker};
use netstack3_base::{
    CoreTimerContext, Inspectable, Inspector, Instant as _, LocalFrameDestination,
    StrongDeviceIdentifier as _, WeakDeviceIdentifier,
};
use packet::{Buf, ParseBufferMut};
use packet_formats::ip::IpPacket;
use zerocopy::SplitByteSlice;

use crate::IpLayerIpExt;
use crate::internal::multicast_forwarding::{
    MulticastForwardingBindingsContext, MulticastForwardingBindingsTypes,
    MulticastForwardingTimerId,
};
use crate::multicast_forwarding::MulticastRouteKey;

/// The number of packets that the stack is willing to queue for a given
/// [`MulticastRouteKey`] while waiting for an applicable route to be installed.
///
/// This value is consistent with the defaults on both Netstack2 and Linux.
pub(crate) const PACKET_QUEUE_LEN: usize = 3;

/// The maximum number of pending multicast routes that can be queued.
///
/// The size of each entry is dominated by the packet queue (up to
/// PACKET_QUEUE_LEN). With 1000 entries, each with 3 standard MTU packets, this
/// limit is approximately 4.5MB.
const MAX_PENDING_ROUTES: usize = 1000;

/// The amount of time the stack is willing to queue a packet while waiting
/// for an applicable route to be installed.
///
/// This value is consistent with the defaults on both Netstack2 and Linux.
const PENDING_ROUTE_EXPIRATION: Duration = Duration::from_secs(10);

/// The minimum amount of time after a garbage-collection run across the
/// [`MulticastForwardingPendingPackets`] table that the stack will wait before
/// performing another garbage-collection.
///
/// This value is consistent with the defaults on both Netstack2 and Linux.
const PENDING_ROUTE_GC_PERIOD: Duration = Duration::from_secs(10);

/// A table of pending multicast packets that have not yet been forwarded.
///
/// Packets are placed in this table when, during forwarding, there is no route
/// in the [`MulticastRouteTable`] via which to forward them. If/when such a
/// route is installed, the packets stored here can be forwarded accordingly.
#[derive(Derivative)]
#[derivative(Debug(bound = ""))]
pub struct MulticastForwardingPendingPackets<
    I: IpLayerIpExt,
    D: WeakDeviceIdentifier,
    BT: MulticastForwardingBindingsTypes,
> {
    table: LruCache<MulticastRouteKey<I>, PacketQueue<I, D, BT>>,
    /// Periodically triggers invocations of [`Self::run_garbage_collection`].
    ///
    /// All interactions with the `gc_timer` must uphold the invariant that the
    /// timer is not scheduled if [`Self::table`] is empty.
    ///
    /// Note: When [`Self`] is held by [`MulticastForwardingEnabledState`], it
    /// is lock protected, which prevents method calls on it from racing. E.g.
    /// no overlapping calls to [`Self::try_queue_packet`], [`Self::remove`],
    /// or [`Self::run_garbage_collection`].
    gc_timer: BT::Timer,
}

impl<I: IpLayerIpExt, D: WeakDeviceIdentifier, BC: MulticastForwardingBindingsContext<I, D::Strong>>
    MulticastForwardingPendingPackets<I, D, BC>
{
    pub(crate) fn new<CC>(bindings_ctx: &mut BC) -> Self
    where
        CC: CoreTimerContext<MulticastForwardingTimerId<I>, BC>,
    {
        Self {
            table: LruCache::new(MAX_PENDING_ROUTES),
            gc_timer: CC::new_timer(
                bindings_ctx,
                MulticastForwardingTimerId::PendingPacketsGc(IpVersionMarker::<I>::new()),
            ),
        }
    }

    /// Attempt to queue the packet in the pending_table.
    ///
    /// If the table becomes newly occupied, the GC timer is scheduled.
    pub(crate) fn try_queue_packet<B>(
        &mut self,
        bindings_ctx: &mut BC,
        key: MulticastRouteKey<I>,
        packet: &I::Packet<B>,
        dev: &D::Strong,
        frame_dst: Option<LocalFrameDestination>,
        max_fragment_len: Option<usize>,
    ) -> QueuePacketOutcome
    where
        B: SplitByteSlice,
    {
        let was_empty = self.table.is_empty();
        let outcome = if let Some(queue) = self.table.get_mut(&key) {
            match queue.try_push(|| QueuedPacket::new(dev, packet, frame_dst, max_fragment_len)) {
                Ok(()) => QueuePacketOutcome::QueuedInExistingQueue,
                Err(PacketQueueFullError) => QueuePacketOutcome::ExistingQueueFull,
            }
        } else {
            let mut queue = PacketQueue::new(bindings_ctx);
            queue
                .try_push(|| QueuedPacket::new(dev, packet, frame_dst, max_fragment_len))
                .expect("newly instantiated queue must have capacity");

            let prev = self.table.insert(key, queue);
            debug_assert!(prev.is_none());
            QueuePacketOutcome::QueuedInNewQueue
        };

        // If the table is newly non-empty, schedule the GC. The timer must not
        // already be scheduled (given the invariants on `gc_timer`).
        if was_empty && !self.table.is_empty() {
            let prev = bindings_ctx.schedule_timer(PENDING_ROUTE_GC_PERIOD, &mut self.gc_timer);
            debug_assert!(prev.is_none());
        }

        outcome
    }

    #[cfg(any(debug_assertions, test))]
    pub(crate) fn contains(&self, key: &MulticastRouteKey<I>) -> bool {
        self.table.iter().any(|(k, _)| k == key)
    }

    /// Remove the key from the pending table, returning its queue of packets.
    ///
    /// If the table becomes newly empty, the GC timer is canceled.
    pub(crate) fn remove(
        &mut self,
        key: &MulticastRouteKey<I>,
        bindings_ctx: &mut BC,
    ) -> Option<PacketQueue<I, D, BC>> {
        let was_empty = self.table.is_empty();
        let queue = self.table.remove(key);

        // If the table is newly empty, cancel the GC. Note, we don't assert on
        // the previous state of the timer, because it's possible cancelation
        // will race with the timer firing.
        if !was_empty && self.table.is_empty() {
            let _: Option<BC::Instant> = bindings_ctx.cancel_timer(&mut self.gc_timer);
        }

        queue
    }

    /// Removes expired [`PacketQueue`] entries from [`Self`].
    ///
    /// Returns the number of packets removed as a result.
    pub(crate) fn run_garbage_collection(&mut self, bindings_ctx: &mut BC) -> u64 {
        let now = bindings_ctx.now();
        let mut removed_count = 0u64;
        let expired_keys: Vec<_> = self
            .table
            .iter()
            .filter_map(
                |(key, queue)| if queue.expires_at <= now { Some(key.clone()) } else { None },
            )
            .collect();

        for key in expired_keys {
            let queue = self.table.remove(&key).expect("expired key must be present");
            // NB: "as" conversion is safe because queue_len has a maximum
            // value of `PACKET_QUEUE_LEN`, which fits in a u64.
            removed_count += queue.queue.len() as u64;
        }

        // If the table is still not empty, reschedule the GC. Note that we
        // don't assert on the previous state of the timer, because it's
        // possible that starting GC raced with a new timer being scheduled.
        if !self.table.is_empty() {
            let _: Option<BC::Instant> =
                bindings_ctx.schedule_timer(PENDING_ROUTE_GC_PERIOD, &mut self.gc_timer);
        }

        removed_count
    }
}

impl<I: IpLayerIpExt, D: WeakDeviceIdentifier, BT: MulticastForwardingBindingsTypes> Inspectable
    for MulticastForwardingPendingPackets<I, D, BT>
{
    fn record<II: Inspector>(&self, inspector: &mut II) {
        let MulticastForwardingPendingPackets { table, gc_timer: _ } = self;
        // NB: Don't record all routes, as the size of the table may be quite
        // large, and its contents are dictated by network traffic.
        inspector.record_usize("NumRoutes", table.len())
    }
}

/// Possible outcomes from calling [`MulticastForwardingPendingPackets::try_queue_packet`].
#[derive(Debug, PartialEq)]
pub(crate) enum QueuePacketOutcome {
    /// The packet was successfully queued. There was no existing
    /// [`PacketQueue`] for the given route key, so a new one was instantiated.
    QueuedInNewQueue,
    /// The packet was successfully queued. It was added onto an existing
    /// [`PacketQueue`] for the given route key.
    QueuedInExistingQueue,
    /// The packet was not queued. There was an existing [`PacketQueue`] for the
    /// given route key, but that queue was full.
    ExistingQueueFull,
}

/// A queue of multicast packets that are pending the installation of a route.
#[derive(Derivative)]
#[derivative(Debug(bound = ""))]
pub struct PacketQueue<I: Ip, D: WeakDeviceIdentifier, BT: MulticastForwardingBindingsTypes> {
    queue: ArrayVec<QueuedPacket<I, D>, PACKET_QUEUE_LEN>,
    /// The time after which the PacketQueue is allowed to be garbage collected.
    expires_at: BT::Instant,
}

impl<I: IpLayerIpExt, D: WeakDeviceIdentifier, BC: MulticastForwardingBindingsContext<I, D::Strong>>
    PacketQueue<I, D, BC>
{
    fn new(bindings_ctx: &mut BC) -> Self {
        Self {
            queue: Default::default(),
            expires_at: bindings_ctx.now().panicking_add(PENDING_ROUTE_EXPIRATION),
        }
    }

    /// Try to push a packet into the queue, returning an error when full.
    ///
    /// Note: the packet is taken as a builder closure, because constructing the
    /// packet is an expensive operation (requiring a `Vec` allocation). By
    /// taking a closure we can defer construction until we're certain the queue
    /// has the free space to hold it.
    fn try_push(
        &mut self,
        packet_builder: impl FnOnce() -> QueuedPacket<I, D>,
    ) -> Result<(), PacketQueueFullError> {
        if self.queue.is_full() {
            return Err(PacketQueueFullError);
        }
        self.queue.push(packet_builder());
        Ok(())
    }
}

#[derive(Debug)]
struct PacketQueueFullError;

impl<I: Ip, D: WeakDeviceIdentifier, BT: MulticastForwardingBindingsTypes> IntoIterator
    for PacketQueue<I, D, BT>
{
    type Item = QueuedPacket<I, D>;
    type IntoIter = <ArrayVec<QueuedPacket<I, D>, PACKET_QUEUE_LEN> as IntoIterator>::IntoIter;
    fn into_iter(self) -> Self::IntoIter {
        let Self { queue, expires_at: _ } = self;
        queue.into_iter()
    }
}

/// An individual multicast packet that's queued.
#[derive(Debug, PartialEq)]
pub struct QueuedPacket<I: Ip, D: WeakDeviceIdentifier> {
    /// The device on which the packet arrived.
    pub(crate) device: D,
    /// The packet.
    pub(crate) packet: ValidIpPacketBuf<I>,
    /// The link layer (L2) destination that the packet was sent to, or `None`
    /// if the packet arrived above the link layer (e.g. a Pure IP device).
    pub(crate) frame_dst: Option<LocalFrameDestination>,
    /// The maximum size fragment that was used to reassemble this packet when
    /// it ingressed the stack. None if IP reassembly was not performed.
    pub(crate) max_fragment_len: Option<usize>,
}

impl<I: IpLayerIpExt, D: WeakDeviceIdentifier> QueuedPacket<I, D> {
    fn new<B: SplitByteSlice>(
        device: &D::Strong,
        packet: &I::Packet<B>,
        frame_dst: Option<LocalFrameDestination>,
        max_fragment_len: Option<usize>,
    ) -> Self {
        QueuedPacket {
            device: device.downgrade(),
            packet: ValidIpPacketBuf::new(packet),
            frame_dst,
            max_fragment_len,
        }
    }
}

/// A buffer containing a known-to-be valid IP packet.
///
/// The only constructor of this type takes an `I::Packet`, which is already
/// parsed & validated.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ValidIpPacketBuf<I: Ip> {
    buffer: Buf<Vec<u8>>,
    _version_marker: IpVersionMarker<I>,
}

impl<I: IpLayerIpExt> ValidIpPacketBuf<I> {
    fn new<B: SplitByteSlice>(packet: &I::Packet<B>) -> Self {
        Self { buffer: Buf::new(packet.to_vec(), ..), _version_marker: Default::default() }
    }

    /// Parses the internal buffer into a mutable IP Packet.
    ///
    /// # Panics
    ///
    /// This function panics if called multiple times. Parsing moves the cursor
    /// in the underlying buffer from the start of the IP header to the start
    /// of the IP body.
    pub(crate) fn parse_ip_packet_mut(&mut self) -> I::Packet<&mut [u8]> {
        // NB: Safe to unwrap here because the buffer is known to be valid.
        self.buffer.parse_mut().unwrap()
    }

    pub(crate) fn into_inner(self) -> Buf<Vec<u8>> {
        let Self { buffer, _version_marker } = self;
        buffer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_matches::assert_matches;
    use ip_test_macro::ip_test;
    use netstack3_base::testutil::{
        FakeInstant, FakeTimerCtxExt, FakeWeakDeviceId, MultipleDevicesId,
    };
    use netstack3_base::{CounterContext, InstantContext, StrongDeviceIdentifier, TimerContext};
    use packet::ParseBuffer;
    use static_assertions::const_assert;
    use test_case::test_case;

    use crate::internal::multicast_forwarding;
    use crate::internal::multicast_forwarding::counters::MulticastForwardingCounters;
    use crate::internal::multicast_forwarding::testutil::{
        FakeBindingsCtx, FakeCoreCtx, TestIpExt,
    };

    #[ip_test(I)]
    #[test_case(None, None; "no_metadata")]
    #[test_case(Some(LocalFrameDestination::Multicast), None; "some_frame_dst")]
    #[test_case(None, Some(1400); "some_max_fragment_len")]
    #[test_case(Some(LocalFrameDestination::Multicast), Some(1400); "some_all")]
    fn queue_packet<I: TestIpExt>(
        frame_dst: Option<LocalFrameDestination>,
        max_fragment_len: Option<usize>,
    ) {
        const DEV: MultipleDevicesId = MultipleDevicesId::A;
        let key1 = MulticastRouteKey::new(I::SRC1, I::DST1).unwrap();
        let key2 = MulticastRouteKey::new(I::SRC2, I::DST2).unwrap();
        let key3 = MulticastRouteKey::new(I::SRC1, I::DST2).unwrap();

        // NB: technically the packet's addresses only match `key1`, but for the
        // sake of this test that doesn't cause problems.
        let buf = multicast_forwarding::testutil::new_ip_packet_buf::<I>(I::SRC1, I::DST1);
        let mut buf_ref = buf.as_ref();
        let packet = buf_ref.parse::<I::Packet<_>>().expect("parse should succeed");

        let mut bindings_ctx = FakeBindingsCtx::<I, MultipleDevicesId>::default();

        let mut pending_table =
            MulticastForwardingPendingPackets::<
                I,
                <MultipleDevicesId as StrongDeviceIdentifier>::Weak,
                _,
            >::new::<FakeCoreCtx<I, MultipleDevicesId>>(&mut bindings_ctx);

        // The first packet gets a new queue.
        assert_eq!(
            pending_table.try_queue_packet(
                &mut bindings_ctx,
                key1.clone(),
                &packet,
                &DEV,
                frame_dst,
                max_fragment_len,
            ),
            QueuePacketOutcome::QueuedInNewQueue
        );
        // The second - Nth packets uses the existing queue.
        for _ in 1..PACKET_QUEUE_LEN {
            assert_eq!(
                pending_table.try_queue_packet(
                    &mut bindings_ctx,
                    key1.clone(),
                    &packet,
                    &DEV,
                    frame_dst,
                    max_fragment_len,
                ),
                QueuePacketOutcome::QueuedInExistingQueue
            );
        }
        // The Nth +1 packet is rejected.
        assert_eq!(
            pending_table.try_queue_packet(
                &mut bindings_ctx,
                key1.clone(),
                &packet,
                &DEV,
                frame_dst,
                max_fragment_len,
            ),
            QueuePacketOutcome::ExistingQueueFull
        );

        // A packet with a different key gets a new queue.
        assert_eq!(
            pending_table.try_queue_packet(
                &mut bindings_ctx,
                key2.clone(),
                &packet,
                &DEV,
                frame_dst,
                max_fragment_len,
            ),
            QueuePacketOutcome::QueuedInNewQueue
        );

        // Based on the calls above, `key1` should have a full queue, `key2`
        // should have a queue with only 1 packet, and `key3` shouldn't have
        // a queue.
        let expected_packet = QueuedPacket::new(&DEV, &packet, frame_dst, max_fragment_len);
        let queue =
            pending_table.remove(&key1, &mut bindings_ctx).expect("key1 should have a queue");
        assert_eq!(queue.queue.len(), PACKET_QUEUE_LEN);
        for packet in queue.queue.as_slice() {
            assert_eq!(packet, &expected_packet);
        }

        let queue =
            pending_table.remove(&key2, &mut bindings_ctx).expect("key2 should have a queue");
        let packet = assert_matches!(&queue.queue[..], [p] => p);
        assert_eq!(packet, &expected_packet);

        assert_matches!(pending_table.remove(&key3, &mut bindings_ctx), None);
    }

    /// Helper to observe the next scheduled GC for the core_ctx pending table.
    fn next_gc_time<I: TestIpExt>(
        core_ctx: &mut FakeCoreCtx<I, MultipleDevicesId>,
        bindings_ctx: &mut FakeBindingsCtx<I, MultipleDevicesId>,
    ) -> Option<FakeInstant> {
        multicast_forwarding::testutil::with_pending_table(core_ctx, |pending_table| {
            bindings_ctx.scheduled_instant(&mut pending_table.gc_timer)
        })
    }

    /// Helper to queue packet in the core_ctx pending table.
    fn try_queue_packet<I: TestIpExt>(
        core_ctx: &mut FakeCoreCtx<I, MultipleDevicesId>,
        bindings_ctx: &mut FakeBindingsCtx<I, MultipleDevicesId>,
        key: MulticastRouteKey<I>,
        dev: &MultipleDevicesId,
        frame_dst: Option<LocalFrameDestination>,
        max_fragment_len: Option<usize>,
    ) -> QueuePacketOutcome {
        let buf =
            multicast_forwarding::testutil::new_ip_packet_buf::<I>(key.src_addr(), key.dst_addr());
        let mut buf_ref = buf.as_ref();
        let packet = buf_ref.parse::<I::Packet<_>>().expect("parse should succeed");
        multicast_forwarding::testutil::with_pending_table(core_ctx, |pending_table| {
            pending_table.try_queue_packet(
                bindings_ctx,
                key,
                &packet,
                dev,
                frame_dst,
                max_fragment_len,
            )
        })
    }

    /// Helper to remove a packet queue in the core_ctx pending table.
    fn remove_packet_queue<I: TestIpExt>(
        core_ctx: &mut FakeCoreCtx<I, MultipleDevicesId>,
        bindings_ctx: &mut FakeBindingsCtx<I, MultipleDevicesId>,
        key: &MulticastRouteKey<I>,
    ) -> Option<
        PacketQueue<I, FakeWeakDeviceId<MultipleDevicesId>, FakeBindingsCtx<I, MultipleDevicesId>>,
    > {
        multicast_forwarding::testutil::with_pending_table(core_ctx, |pending_table| {
            pending_table.remove(key, bindings_ctx)
        })
    }

    /// Helper to trigger the GC.
    fn run_gc<I: TestIpExt>(
        core_ctx: &mut FakeCoreCtx<I, MultipleDevicesId>,
        bindings_ctx: &mut FakeBindingsCtx<I, MultipleDevicesId>,
    ) {
        assert_matches!(
            &bindings_ctx.trigger_timers_until_instant(bindings_ctx.now(), core_ctx)[..],
            [MulticastForwardingTimerId::PendingPacketsGc(_)]
        );
    }

    #[ip_test(I)]
    fn garbage_collection<I: TestIpExt>() {
        const DEV: MultipleDevicesId = MultipleDevicesId::A;
        const FRAME_DST: Option<LocalFrameDestination> = None;
        const MAX_FRAGMENT_LEN: Option<usize> = None;
        let key1 = MulticastRouteKey::<I>::new(I::SRC1, I::DST1).unwrap();
        let key2 = MulticastRouteKey::<I>::new(I::SRC2, I::DST2).unwrap();

        let mut api = multicast_forwarding::testutil::new_api();
        assert!(api.enable());
        let (core_ctx, bindings_ctx) = api.contexts();

        // NB: As written, the test requires that
        //  1. `PENDING_ROUTE_GC_PERIOD` >= `PENDING_ROUTE_EXPIRATION`, and
        //  2. `PENDING_ROUTE_EXPIRATION > 0`.
        // If the values are ever changed such that that is not true, the test
        // will need to be re-written.
        const_assert!(PENDING_ROUTE_GC_PERIOD.checked_sub(PENDING_ROUTE_EXPIRATION).is_some());
        const_assert!(!PENDING_ROUTE_EXPIRATION.is_zero());

        // The GC shouldn't be scheduled with an empty table.
        assert!(next_gc_time(core_ctx, bindings_ctx).is_none());
        let counters: &MulticastForwardingCounters<I> = core_ctx.counters();
        assert_eq!(counters.pending_table_gc.get(), 0);
        assert_eq!(counters.pending_packet_drops_gc.get(), 0);

        // Queue a packet, and expect the GC to be scheduled.
        let expected_first_gc = bindings_ctx.now() + PENDING_ROUTE_GC_PERIOD;
        assert_eq!(
            try_queue_packet(
                core_ctx,
                bindings_ctx,
                key1.clone(),
                &DEV,
                FRAME_DST,
                MAX_FRAGMENT_LEN
            ),
            QueuePacketOutcome::QueuedInNewQueue
        );
        assert_eq!(next_gc_time(core_ctx, bindings_ctx), Some(expected_first_gc));

        // Sleep until we're ready to GC, and then queue a second packet under a
        // new key. Expect that the GC timer is still scheduled for the original
        // instant.
        bindings_ctx.timers.instant.sleep(PENDING_ROUTE_GC_PERIOD);
        assert_eq!(
            try_queue_packet(
                core_ctx,
                bindings_ctx,
                key2.clone(),
                &DEV,
                FRAME_DST,
                MAX_FRAGMENT_LEN
            ),
            QueuePacketOutcome::QueuedInNewQueue
        );
        assert_eq!(next_gc_time(core_ctx, bindings_ctx), Some(expected_first_gc));

        // Run the GC, and verify that it was rescheduled after the fact
        // (because `key2` still exists in the table).
        run_gc(core_ctx, bindings_ctx);
        let expected_second_gc = bindings_ctx.timers.instant.now() + PENDING_ROUTE_GC_PERIOD;
        assert_eq!(next_gc_time(core_ctx, bindings_ctx), Some(expected_second_gc));

        // Verify that `key1` was removed, but `key2` remains.
        let counters: &MulticastForwardingCounters<I> = core_ctx.counters();
        assert_eq!(counters.pending_table_gc.get(), 1);
        assert_eq!(counters.pending_packet_drops_gc.get(), 1);
        assert_matches!(remove_packet_queue(core_ctx, bindings_ctx, &key1), None);
        assert_matches!(remove_packet_queue(core_ctx, bindings_ctx, &key2), Some(_));

        // Now that we've explicitly removed `key2`, the table is empty and the
        // GC should have been canceled.
        assert!(next_gc_time(core_ctx, bindings_ctx).is_none());

        // Finally, verify that if the GC clears the table, it doesn't
        // reschedule itself.
        assert_eq!(
            try_queue_packet(
                core_ctx,
                bindings_ctx,
                key1.clone(),
                &DEV,
                FRAME_DST,
                MAX_FRAGMENT_LEN
            ),
            QueuePacketOutcome::QueuedInNewQueue
        );
        assert_eq!(next_gc_time(core_ctx, bindings_ctx), Some(expected_second_gc));
        bindings_ctx.timers.instant.sleep(PENDING_ROUTE_GC_PERIOD);
        run_gc(core_ctx, bindings_ctx);
        let counters: &MulticastForwardingCounters<I> = core_ctx.counters();
        assert_eq!(counters.pending_table_gc.get(), 2);
        assert_eq!(counters.pending_packet_drops_gc.get(), 2);
        assert_matches!(remove_packet_queue(core_ctx, bindings_ctx, &key1), None);
        assert!(next_gc_time(core_ctx, bindings_ctx).is_none());
    }
}
