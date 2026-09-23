// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Facilities for tracking the counts of various TCP events.

use net_types::ip::{Ip, IpMarked};
use netstack3_base::socket::EitherStack;
use netstack3_base::{
    Counter, CounterContext, Inspectable, Inspector, InspectorExt as _, ResourceCounterContext,
    WeakDeviceIdentifier,
};

use crate::internal::socket::{DualStackIpExt, TcpBindingsTypes, TcpSocketId};

/// A marker trait to simplify bounds for TCP counters.
pub trait TcpCounterContext<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes>:
    ResourceCounterContext<TcpSocketId<I, D, BT>, TcpCountersWithSocket<I>>
    + CounterContext<TcpCountersWithoutSocket<I>>
{
}

impl<I, D, BT, CC> TcpCounterContext<I, D, BT> for CC
where
    I: DualStackIpExt,
    D: WeakDeviceIdentifier,
    BT: TcpBindingsTypes,
    CC: ResourceCounterContext<TcpSocketId<I, D, BT>, TcpCountersWithSocket<I>>
        + CounterContext<TcpCountersWithoutSocket<I>>,
{
}

/// Counters for TCP events that cannot be attributed to an individual socket.
///
/// These counters are tracked stack wide.
///
/// Note on dual stack sockets: These counters are tracked for `WireI`.
pub type TcpCountersWithoutSocket<I> = IpMarked<I, TcpCountersWithoutSocketInner>;

/// The IP agnostic version of [`TcpCountersWithoutSocket`].
///
/// The counter type `C` is generic to facilitate testing.
#[derive(Default, Debug)]
#[cfg_attr(
    any(test, feature = "testutils"),
    derive(PartialEq, netstack3_macros::CounterCollection)
)]
pub struct TcpCountersWithoutSocketInner<C = Counter> {
    /// Count of received IP packets that were dropped because they had
    /// unexpected IP addresses (either src or dst).
    pub invalid_ip_addrs_received: C,
    /// Count of received TCP segments that were dropped because they could not
    /// be parsed.
    pub invalid_segments_received: C,
    /// Count of received TCP segments that were valid.
    pub valid_segments_received: C,
    /// Count of received TCP segments that were not associated with any
    /// existing sockets.
    pub received_segments_no_dispatch: C,
    /// Count of received segments whose checksums were invalid.
    pub checksum_errors: C,
}

/// Counters for TCP events that can be attributed to an individual socket.
///
/// These counters are tracked stack wide and per socket.
///
/// Note on dual stack sockets: These counters are tracked for `SockI`.
// TODO(https://fxbug.dev/396127493): For some of these events, it would be
// better to track them for `WireI` (e.g. `received_segments_dispatched`,
// `segments_sent`, etc.). Doing so may require splitting up the struct and/or
// reworking the `ResourceCounterContext` trait.
pub type TcpCountersWithSocket<I> = IpMarked<I, TcpCountersWithSocketInner<Counter>>;

/// The IP agnostic version of [`TcpCountersWithSocket`].
///
/// The counter type `C` is generic to facilitate testing.
// TODO(https://fxbug.dev/42052878): Add counters for SYN cookies.
#[derive(Default, Debug)]
#[cfg_attr(
    any(test, feature = "testutils"),
    derive(PartialEq, netstack3_macros::CounterCollection)
)]
pub struct TcpCountersWithSocketInner<C = Counter> {
    /// Count of received TCP segments that were successfully dispatched to a
    /// socket.
    pub received_segments_dispatched: C,
    /// Count of received TCP segments that were dropped because the listener
    /// queue was full.
    pub listener_queue_overflow: C,
    /// Count of TCP segments that failed to send.
    pub segment_send_errors: C,
    /// Count of TCP segments that were sent.
    pub segments_sent: C,
    /// Count of passive open attempts that failed because the stack doesn't
    /// have route to the peer.
    pub passive_open_no_route_errors: C,
    /// Count of passive connections that have been opened.
    pub passive_connection_openings: C,
    /// Count of active open attempts that have failed because the stack doesn't
    /// have a route to the peer.
    pub active_open_no_route_errors: C,
    /// Count of active connections that have been opened.
    pub active_connection_openings: C,
    /// Count of all failed connection attempts, including both passive and
    /// active opens.
    pub failed_connection_attempts: C,
    /// Count of port reservation attempts that failed.
    pub failed_port_reservations: C,
    /// Count of received segments with the RST flag set.
    pub resets_received: C,
    /// Count of sent segments with the RST flag set.
    pub resets_sent: C,
    /// Count of received segments with the SYN flag set.
    pub syns_received: C,
    /// Count of sent segments with the SYN flag set.
    pub syns_sent: C,
    /// Count of received segments with the FIN flag set.
    pub fins_received: C,
    /// Count of sent segments with the FIN flag set.
    pub fins_sent: C,
    /// Count of retransmission timeouts.
    pub timeouts: C,
    /// Count of retransmissions of segments.
    pub retransmits: C,
    /// Count of retransmissions of segments while in slow start.
    pub slow_start_retransmits: C,
    /// Count of retransmissions of segments while in fast recovery.
    pub fast_retransmits: C,
    /// Count of retransmissions of segments while in SACK recovery.
    pub sack_retransmits: C,
    /// Count of times fast recovery was initiated to recover from packet loss.
    pub fast_recovery: C,
    /// Count of times SACK recovery was initiated to recover from packet loss.
    pub sack_recovery: C,
    /// Count of times an established TCP connection transitioned to CLOSED.
    pub established_closed: C,
    /// Count of times an established TCP connection transitioned to CLOSED due
    /// to a RST segment.
    pub established_resets: C,
    /// Count of times an established TCP connection transitioned to CLOSED due
    /// to a timeout (e.g. a keep-alive or retransmit timeout).
    pub established_timedout: C,
    /// Count of times loss recovery completes successfully.
    pub loss_recovered: C,
    /// Count of times duplicate ACKs were received.
    pub dup_acks: C,
}

/// A composition of the TCP Counters with and without a socket.
pub struct CombinedTcpCounters<'a, I: Ip> {
    /// The TCP Counters that can be associated with a socket.
    pub with_socket: &'a TcpCountersWithSocket<I>,
    /// The TCP Counters that cannot be associated with a socket.
    ///
    /// This field is optional so that the same [`Inspectable`] implementation
    /// can be used for both the stack-wide counters, and the per-socket
    /// counters.
    pub without_socket: Option<&'a TcpCountersWithoutSocket<I>>,
}

impl<I: Ip> Inspectable for CombinedTcpCounters<'_, I> {
    fn record<II: Inspector>(&self, inspector: &mut II) {
        let CombinedTcpCounters { with_socket, without_socket } = self;
        let TcpCountersWithSocketInner {
            received_segments_dispatched,
            listener_queue_overflow,
            segment_send_errors,
            segments_sent,
            passive_open_no_route_errors,
            passive_connection_openings,
            active_open_no_route_errors,
            active_connection_openings,
            failed_connection_attempts,
            failed_port_reservations,
            resets_received,
            resets_sent,
            syns_received,
            syns_sent,
            fins_received,
            fins_sent,
            timeouts,
            retransmits,
            slow_start_retransmits,
            fast_retransmits,
            sack_retransmits,
            fast_recovery,
            sack_recovery,
            established_closed,
            established_resets,
            established_timedout,
            loss_recovered,
            dup_acks,
        } = with_socket.as_ref();

        // Note: Organize the "without socket" counters into helper structs to
        // make the optionality more ergonomic to handle.
        struct WithoutSocketRx<'a> {
            valid_segments_received: &'a Counter,
        }
        struct WithoutSocketRxError<'a> {
            invalid_ip_addrs_received: &'a Counter,
            invalid_segments_received: &'a Counter,
            received_segments_no_dispatch: &'a Counter,
            checksum_errors: &'a Counter,
        }
        let (without_socket_rx, without_socket_rx_error) = match without_socket.map(AsRef::as_ref) {
            None => (None, None),
            Some(TcpCountersWithoutSocketInner {
                invalid_ip_addrs_received,
                invalid_segments_received,
                valid_segments_received,
                received_segments_no_dispatch,
                checksum_errors,
            }) => (
                Some(WithoutSocketRx { valid_segments_received }),
                Some(WithoutSocketRxError {
                    invalid_ip_addrs_received,
                    invalid_segments_received,
                    received_segments_no_dispatch,
                    checksum_errors,
                }),
            ),
        };

        inspector.record_child("Rx", |inspector| {
            if let Some(WithoutSocketRx { valid_segments_received }) = without_socket_rx {
                inspector.record_counter("ValidSegmentsReceived", valid_segments_received);
            }
            inspector.record_counter("ReceivedSegmentsDispatched", received_segments_dispatched);
            inspector.record_counter("ResetsReceived", resets_received);
            inspector.record_counter("SynsReceived", syns_received);
            inspector.record_counter("FinsReceived", fins_received);
            inspector.record_counter("DupAcks", dup_acks);
            inspector.record_child("Errors", |inspector| {
                inspector.record_counter("ListenerQueueOverflow", listener_queue_overflow);
                inspector.record_counter("PassiveOpenNoRouteErrors", passive_open_no_route_errors);
                if let Some(WithoutSocketRxError {
                    invalid_ip_addrs_received,
                    invalid_segments_received,
                    received_segments_no_dispatch,
                    checksum_errors,
                }) = without_socket_rx_error
                {
                    inspector.record_counter("InvalidIpAddrsReceived", invalid_ip_addrs_received);
                    inspector.record_counter("InvalidSegmentsReceived", invalid_segments_received);
                    inspector.record_counter(
                        "ReceivedSegmentsNoDispatch",
                        received_segments_no_dispatch,
                    );
                    inspector.record_counter("ChecksumErrors", checksum_errors);
                }
            })
        });
        inspector.record_child("Tx", |inspector| {
            inspector.record_counter("SegmentsSent", segments_sent);
            inspector.record_counter("ResetsSent", resets_sent);
            inspector.record_counter("SynsSent", syns_sent);
            inspector.record_counter("FinsSent", fins_sent);
            inspector.record_counter("Timeouts", timeouts);
            inspector.record_counter("Retransmits", retransmits);
            inspector.record_counter("SlowStartRetransmits", slow_start_retransmits);
            inspector.record_counter("FastRetransmits", fast_retransmits);
            inspector.record_counter("SackRetransmits", sack_retransmits);
            inspector.record_child("Errors", |inspector| {
                inspector.record_counter("SegmentSendErrors", segment_send_errors);
                inspector.record_counter("ActiveOpenNoRouteErrors", active_open_no_route_errors);
            });
        });
        inspector.record_counter("PassiveConnectionOpenings", passive_connection_openings);
        inspector.record_counter("ActiveConnectionOpenings", active_connection_openings);
        inspector.record_counter("FastRecovery", fast_recovery);
        inspector.record_counter("SackRecovery", sack_recovery);
        inspector.record_counter("LossRecovered", loss_recovered);
        inspector.record_counter("EstablishedClosed", established_closed);
        inspector.record_counter("EstablishedResets", established_resets);
        inspector.record_counter("EstablishedTimedout", established_timedout);
        inspector.record_child("Errors", |inspector| {
            inspector.record_counter("FailedConnectionOpenings", failed_connection_attempts);
            inspector.record_counter("FailedPortReservations", failed_port_reservations);
        })
    }
}

/// Holds references to the stack-wide and per-socket counters.
///
/// This is used to easily increment both counters in contexts that don't have
/// access to `ResourceCounterContext`. This is currently used by the TCP state
/// machine, which does not operate on a core context so that it can remain
/// IP agnostic (and thus avoid duplicate code generation).
pub(crate) struct TcpCountersRefs<'a> {
    pub(crate) stack_wide: &'a TcpCountersWithSocketInner,
    pub(crate) per_socket: &'a TcpCountersWithSocketInner,
}

impl<'a> TcpCountersRefs<'a> {
    pub(crate) fn from_ctx<I: Ip, R, CC: ResourceCounterContext<R, TcpCountersWithSocket<I>>>(
        ctx: &'a CC,
        resource: &'a R,
    ) -> Self {
        TcpCountersRefs {
            stack_wide: ctx.counters(),
            per_socket: ctx.per_resource_counters(resource),
        }
    }

    pub(crate) fn increment<F: Fn(&TcpCountersWithSocketInner<Counter>) -> &Counter>(&self, cb: F) {
        let Self { stack_wide, per_socket } = self;
        cb(stack_wide).increment();
        cb(per_socket).increment();
    }
}

/// Increments the stack-wide counters, and optionally the per-resouce counters.
///
/// Used to increment counters that can, but are not required to, have an
/// associated socket. For example, sent segment counts.
pub(crate) fn increment_counter_with_optional_socket_id<I, CC, BT, D, F>(
    core_ctx: &CC,
    socket_id: Option<&TcpSocketId<I, D, BT>>,
    cb: F,
) where
    I: DualStackIpExt,
    CC: TcpCounterContext<I, D, BT>,
    D: WeakDeviceIdentifier,
    BT: TcpBindingsTypes,
    F: Fn(&TcpCountersWithSocket<I>) -> &Counter,
{
    match socket_id {
        Some(id) => core_ctx.increment_both(id, cb),
        None => cb(core_ctx.counters()).increment(),
    }
}

/// Increments the stack-wide counters, and optionally the per-resouce counters.
///
/// Used to increment counters that can, but are not required to, have an
/// associated socket. For example, received segment counts.
pub(crate) fn increment_counter_with_optional_demux_id<I, CC, BT, D, F>(
    core_ctx: &CC,
    demux_id: Option<&I::DemuxSocketId<D, BT>>,
    cb: F,
) where
    I: DualStackIpExt,
    CC: TcpCounterContext<I, D, BT> + TcpCounterContext<I::OtherVersion, D, BT>,
    D: WeakDeviceIdentifier,
    BT: TcpBindingsTypes,
    F: Fn(&TcpCountersWithSocketInner) -> &Counter,
{
    match demux_id {
        Some(id) => increment_counter_for_demux_id::<I, _, _, _, _>(core_ctx, &id, cb),
        None => {
            cb(CounterContext::<TcpCountersWithSocket<I>>::counters(core_ctx).as_ref()).increment()
        }
    }
}

/// Increment a counter for TCP demux_ids, which may exist in either stack.
pub(crate) fn increment_counter_for_demux_id<I, D, BT, CC, F>(
    core_ctx: &CC,
    demux_id: &I::DemuxSocketId<D, BT>,
    cb: F,
) where
    I: DualStackIpExt,
    D: WeakDeviceIdentifier,
    BT: TcpBindingsTypes,
    CC: TcpCounterContext<I, D, BT> + TcpCounterContext<I::OtherVersion, D, BT>,
    F: Fn(&TcpCountersWithSocketInner<Counter>) -> &Counter,
{
    match I::as_dual_stack_ip_socket(demux_id) {
        EitherStack::ThisStack(socket_id) => core_ctx
            .increment_both(socket_id, |counters: &TcpCountersWithSocket<I>| cb(counters.as_ref())),
        EitherStack::OtherStack(socket_id) => core_ctx
            .increment_both(socket_id, |counters: &TcpCountersWithSocket<I::OtherVersion>| {
                cb(counters.as_ref())
            }),
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;

    pub(crate) type CounterExpectations = TcpCountersWithSocketInner<u64>;

    pub(crate) type CounterExpectationsWithoutSocket = TcpCountersWithoutSocketInner<u64>;
}
