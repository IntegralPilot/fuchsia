// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Counters for the Internet Control Message Protocol (ICMP).

use core::fmt::Debug;
use net_types::ip::{Ip, Ipv4, Ipv6};
use netstack3_base::{
    Counter, CounterCollectionSpec, CounterRepr, Inspectable, Inspector, InspectorExt,
    TestOnlyPartialEq,
};
use packet_formats::icmp::{
    Icmpv4DestUnreachableCode, Icmpv4ParameterProblemCode, Icmpv4TimeExceededCode,
    Icmpv6DestUnreachableCode, Icmpv6ParameterProblemCode, Icmpv6TimeExceededCode,
};

/// An IP Extension trait for ICMP Counters.
pub trait IcmpCountersIpExt: Ip {
    /// Counters for the ICMP Dest Unreachable message type.
    type DestUnreachableCounters: CounterCollectionSpec<CounterCollection<Counter>: Inspectable>
        + Default
        + Debug
        + TestOnlyPartialEq;

    /// Counters for the ICMP Time Exceeded message type.
    type TimeExceededCounters: CounterCollectionSpec<CounterCollection<Counter>: Inspectable>
        + Default
        + Debug
        + TestOnlyPartialEq;

    /// Counters for the ICMP Parameter Problem message type.
    type ParameterProblemCounters: CounterCollectionSpec<CounterCollection<Counter>: Inspectable>
        + Default
        + Debug
        + TestOnlyPartialEq;
}

impl IcmpCountersIpExt for Ipv4 {
    type DestUnreachableCounters = Icmpv4DestUnreachableCounters;
    type TimeExceededCounters = Icmpv4TimeExceededCounters;
    type ParameterProblemCounters = Icmpv4ParameterProblemCounters;
}

impl IcmpCountersIpExt for Ipv6 {
    type DestUnreachableCounters = Icmpv6DestUnreachableCounters;
    type TimeExceededCounters = Icmpv6TimeExceededCounters;
    type ParameterProblemCounters = Icmpv6ParameterProblemCounters;
}

/// ICMP tx path counters.
#[derive(Default, Debug)]
#[cfg_attr(
    any(test, feature = "testutils"),
    derive(PartialEq, netstack3_macros::CounterCollection)
)]
pub struct IcmpTxCounters<I: IcmpCountersIpExt, C: CounterRepr = Counter> {
    /// Count of reply messages sent.
    pub reply: C,
    /// Count of time exceeded messages sent.
    pub time_exceeded: <I::TimeExceededCounters as CounterCollectionSpec>::CounterCollection<C>,
    /// Count of packet too big messages sent.
    pub packet_too_big: C,
    /// Count of parameter problem messages sent.
    pub parameter_problem:
        <I::ParameterProblemCounters as CounterCollectionSpec>::CounterCollection<C>,
    /// Count of destination unreachable messages sent.
    pub dest_unreachable:
        <I::DestUnreachableCounters as CounterCollectionSpec>::CounterCollection<C>,
    /// Count of error messages sent.
    pub error: C,
}

impl<I: IcmpCountersIpExt> Inspectable for IcmpTxCounters<I> {
    fn record<II: Inspector>(&self, inspector: &mut II) {
        let IcmpTxCounters {
            reply,
            time_exceeded,
            packet_too_big,
            parameter_problem,
            dest_unreachable,
            error,
        } = self;
        inspector.record_counter("Reply", reply);
        inspector.record_inspectable("TimeExceeded", time_exceeded);
        inspector.record_counter("PacketTooBig", packet_too_big);
        inspector.record_inspectable("ParameterProblem", parameter_problem);
        inspector.record_inspectable("DestUnreachable", dest_unreachable);
        inspector.record_counter("Error", error);
    }
}

/// ICMP rx path counters.
#[derive(Default, Debug)]
#[cfg_attr(
    any(test, feature = "testutils"),
    derive(PartialEq, netstack3_macros::CounterCollection)
)]
pub struct IcmpRxCounters<I: IcmpCountersIpExt, C: CounterRepr = Counter> {
    /// Count of error messages received.
    pub error: C,
    /// Count of error messages delivered to the transport layer.
    pub error_delivered_to_transport_layer: C,
    /// Count of error messages delivered to a socket.
    pub error_delivered_to_socket: C,
    /// Count of echo request messages received.
    pub echo_request: C,
    /// Count of echo reply messages received.
    pub echo_reply: C,
    /// Count of timestamp request messages received.
    pub timestamp_request: C,
    /// Count of destination unreachable messages received.
    pub dest_unreachable:
        <I::DestUnreachableCounters as CounterCollectionSpec>::CounterCollection<C>,
    /// Count of time exceeded messages received.
    pub time_exceeded: <I::TimeExceededCounters as CounterCollectionSpec>::CounterCollection<C>,
    /// Count of parameter problem messages received.
    pub parameter_problem:
        <I::ParameterProblemCounters as CounterCollectionSpec>::CounterCollection<C>,
    /// Count of packet too big messages received.
    pub packet_too_big: C,
    /// Count of ICMP Echo datagrams that could not be delivered to the socket
    /// because its receive buffer was full.
    pub queue_full: C,
}

impl<I: IcmpCountersIpExt> Inspectable for IcmpRxCounters<I> {
    fn record<II: Inspector>(&self, inspector: &mut II) {
        let IcmpRxCounters {
            error,
            error_delivered_to_transport_layer,
            error_delivered_to_socket,
            echo_request,
            echo_reply,
            timestamp_request,
            dest_unreachable,
            time_exceeded,
            parameter_problem,
            packet_too_big,
            queue_full,
        } = self;
        inspector.record_counter("EchoRequest", echo_request);
        inspector.record_counter("EchoReply", echo_reply);
        inspector.record_counter("TimestampRequest", timestamp_request);
        inspector.record_inspectable("DestUnreachable", dest_unreachable);
        inspector.record_inspectable("TimeExceeded", time_exceeded);
        inspector.record_inspectable("ParameterProblem", parameter_problem);
        inspector.record_counter("PacketTooBig", packet_too_big);
        inspector.record_counter("Error", error);
        inspector
            .record_counter("ErrorDeliveredToTransportLayer", error_delivered_to_transport_layer);
        inspector.record_counter("ErrorDeliveredToSocket", error_delivered_to_socket);
        inspector.record_counter("DroppedQueueFull", queue_full);
    }
}

/// ICMPv4 counters for Dest Unreachable messages.
///
/// As defined by IANA:
/// https://www.iana.org/assignments/icmp-parameters/icmp-parameters.xhtml#icmp-parameters-codes-3
#[derive(Default, Debug, netstack3_macros::CounterCollection)]
#[cfg_attr(any(test, feature = "testutils"), derive(PartialEq,))]
pub struct Icmpv4DestUnreachableCounters<C: CounterRepr = Counter> {
    /// Network Unreachable, code 0
    pub dest_network_unreachable: C,
    /// Host Unreachable, code 1
    pub dest_host_unreachable: C,
    /// Protocol Unreachable, code 2
    pub dest_protocol_unreachable: C,
    /// Port Unreachable , code 3
    pub dest_port_unreachable: C,
    /// Fragmentation Needed and Don't Fragment was set, code 4
    pub fragmentation_required: C,
    /// Source Route Failed, code 5
    pub source_route_failed: C,
    /// Destination Network Unknown, code 6
    pub dest_network_unknown: C,
    /// Destination Host Unknown, code 7
    pub dest_host_unknown: C,
    /// Source Host Isolated, code 8
    pub source_host_isolated: C,
    /// Communication with Destination Network is Administratively Prohibited, code 9
    pub network_administratively_prohibited: C,
    /// Communication with Destination Host is Administratively Prohibited, code 10
    pub host_administratively_prohibited: C,
    /// Destination Network Unreachable for Type of Service, code 11
    pub network_unreachable_for_tos: C,
    /// Destination Host Unreachable for Type of Service, code 12
    pub host_unreachable_for_tos: C,
    /// Communicaation Administratively Prohibited, code 13
    pub comm_administratively_prohibited: C,
    /// Host Precedence Violation, code 14
    pub host_precedence_violation: C,
    /// Precedence Cutoff in Effect, code 15
    pub precedence_cutoff_in_effect: C,
}

impl Icmpv4DestUnreachableCounters<Counter> {
    pub(crate) fn increment_code(&self, code: Icmpv4DestUnreachableCode) {
        use Icmpv4DestUnreachableCode::*;
        let counter = match code {
            DestNetworkUnreachable => &self.dest_network_unreachable,
            DestHostUnreachable => &self.dest_host_unreachable,
            DestProtocolUnreachable => &self.dest_protocol_unreachable,
            DestPortUnreachable => &self.dest_port_unreachable,
            FragmentationRequired => &self.fragmentation_required,
            SourceRouteFailed => &self.source_route_failed,
            DestNetworkUnknown => &self.dest_network_unknown,
            DestHostUnknown => &self.dest_host_unknown,
            SourceHostIsolated => &self.source_host_isolated,
            NetworkAdministrativelyProhibited => &self.network_administratively_prohibited,
            HostAdministrativelyProhibited => &self.host_administratively_prohibited,
            NetworkUnreachableForToS => &self.network_unreachable_for_tos,
            HostUnreachableForToS => &self.host_unreachable_for_tos,
            CommAdministrativelyProhibited => &self.comm_administratively_prohibited,
            HostPrecedenceViolation => &self.host_precedence_violation,
            PrecedenceCutoffInEffect => &self.precedence_cutoff_in_effect,
        };
        counter.increment()
    }
}

impl<C: CounterRepr> Inspectable for Icmpv4DestUnreachableCounters<C> {
    fn record<I: Inspector>(&self, inspector: &mut I) {
        let Icmpv4DestUnreachableCounters {
            dest_network_unreachable,
            dest_host_unreachable,
            dest_protocol_unreachable,
            dest_port_unreachable,
            fragmentation_required,
            source_route_failed,
            dest_network_unknown,
            dest_host_unknown,
            source_host_isolated,
            network_administratively_prohibited,
            host_administratively_prohibited,
            network_unreachable_for_tos,
            host_unreachable_for_tos,
            comm_administratively_prohibited,
            host_precedence_violation,
            precedence_cutoff_in_effect,
        } = self;
        inspector.record_counter("DestNetworkUnreachable", dest_network_unreachable);
        inspector.record_counter("DestHostUnreachable", dest_host_unreachable);
        inspector.record_counter("DestProtocolUnreachable", dest_protocol_unreachable);
        inspector.record_counter("DestPortUnreachable", dest_port_unreachable);
        inspector.record_counter("FragmentationRequired", fragmentation_required);
        inspector.record_counter("SourceRouteFailed", source_route_failed);
        inspector.record_counter("DestNetworkUnknown", dest_network_unknown);
        inspector.record_counter("DestHostUnknown", dest_host_unknown);
        inspector.record_counter("SourceHostIsolated", source_host_isolated);
        inspector.record_counter(
            "NetworkAdministrativelyProhibited",
            network_administratively_prohibited,
        );
        inspector
            .record_counter("HostAdministrativelyProhibited", host_administratively_prohibited);
        inspector.record_counter("NetworkUnreachableForTos", network_unreachable_for_tos);
        inspector.record_counter("HostUnreachableForTos", host_unreachable_for_tos);
        inspector
            .record_counter("CommAdministrativelyProhibited", comm_administratively_prohibited);
        inspector.record_counter("HostPrecedenceViolation", host_precedence_violation);
        inspector.record_counter("PrecedenceCutoffInEffect", precedence_cutoff_in_effect);
    }
}

/// ICMPv6 counters for Dest Unreachable messages.
///
/// As defined by IANA:
/// https://www.iana.org/assignments/icmpv6-parameters/icmpv6-parameters.xhtml#icmpv6-parameters-codes-2
#[derive(Default, Debug, netstack3_macros::CounterCollection)]
#[cfg_attr(any(test, feature = "testutils"), derive(PartialEq,))]
pub struct Icmpv6DestUnreachableCounters<C: CounterRepr = Counter> {
    /// No route to destination, code 0
    pub no_route: C,
    /// Communication with destination administratively prohibited, code 1
    pub comm_administratively_prohibited: C,
    /// Beyond scope of source address, code 2
    pub beyond_scope: C,
    /// Address unreachable, code 3
    pub addr_unreachable: C,
    /// Port unreachable, code 4
    pub port_unreachable: C,
    /// Source address failed ingress/egress policy, code 5
    pub src_addr_failed_policy: C,
    /// Reject route to destination, code 6
    pub reject_route: C,
}

impl Icmpv6DestUnreachableCounters<Counter> {
    pub(crate) fn increment_code(&self, code: Icmpv6DestUnreachableCode) {
        use Icmpv6DestUnreachableCode::*;
        let counter = match code {
            NoRoute => &self.no_route,
            CommAdministrativelyProhibited => &self.comm_administratively_prohibited,
            BeyondScope => &self.beyond_scope,
            AddrUnreachable => &self.addr_unreachable,
            PortUnreachable => &self.port_unreachable,
            SrcAddrFailedPolicy => &self.src_addr_failed_policy,
            RejectRoute => &self.reject_route,
        };
        counter.increment();
    }
}

impl<C: CounterRepr> Inspectable for Icmpv6DestUnreachableCounters<C> {
    fn record<I: Inspector>(&self, inspector: &mut I) {
        let Icmpv6DestUnreachableCounters {
            no_route,
            comm_administratively_prohibited,
            beyond_scope,
            addr_unreachable,
            port_unreachable,
            src_addr_failed_policy,
            reject_route,
        } = self;
        inspector.record_counter("NoRoute", no_route);
        inspector.record_counter(
            "CommunicationAdministrativelyProhibited",
            comm_administratively_prohibited,
        );
        inspector.record_counter("BeyondScope", beyond_scope);
        inspector.record_counter("AddrUnreachable", addr_unreachable);
        inspector.record_counter("PortUnreachable", port_unreachable);
        inspector.record_counter("SrcAddrFailedPolicy", src_addr_failed_policy);
        inspector.record_counter("RejectRoute", reject_route);
    }
}

/// ICMPv4 counters for the Time Exceeded messages.
///
/// As defined by IANA:
/// https://www.iana.org/assignments/icmp-parameters/icmp-parameters.xhtml#icmp-parameters-codes-11
#[derive(Default, Debug, netstack3_macros::CounterCollection)]
#[cfg_attr(any(test, feature = "testutils"), derive(PartialEq,))]
pub struct Icmpv4TimeExceededCounters<C: CounterRepr = Counter> {
    /// Time to Live exceeded in Transit, code 0
    pub ttl_expired: C,
    /// Fragment Reassembly Time Exceeded, code 1
    pub fragment_reassembly_time_exceeded: C,
}

impl Icmpv4TimeExceededCounters<Counter> {
    pub(crate) fn increment_code(&self, code: Icmpv4TimeExceededCode) {
        use Icmpv4TimeExceededCode::*;
        let counter = match code {
            TtlExpired => &self.ttl_expired,
            FragmentReassemblyTimeExceeded => &self.fragment_reassembly_time_exceeded,
        };
        counter.increment();
    }
}

impl<C: CounterRepr> Inspectable for Icmpv4TimeExceededCounters<C> {
    fn record<I: Inspector>(&self, inspector: &mut I) {
        let Icmpv4TimeExceededCounters { ttl_expired, fragment_reassembly_time_exceeded } = self;
        inspector.record_counter("TtlExpired", ttl_expired);
        inspector
            .record_counter("FragmentReassemblyTimeExceeded", fragment_reassembly_time_exceeded);
    }
}

/// ICMPv6 counters for the Time Exceeded messages.
///
/// As defined by IANA:
/// https://www.iana.org/assignments/icmpv6-parameters/icmpv6-parameters.xhtml#icmpv6-parameters-codes-4
#[derive(Default, Debug, netstack3_macros::CounterCollection)]
#[cfg_attr(any(test, feature = "testutils"), derive(PartialEq,))]
pub struct Icmpv6TimeExceededCounters<C: CounterRepr = Counter> {
    /// Hop limit exceeded in transit, code 0
    pub hop_limit_exceeded: C,
    /// Fragment Reassembly Time Exceeded, code 1
    pub fragment_reassembly_time_exceeded: C,
}

impl Icmpv6TimeExceededCounters<Counter> {
    pub(crate) fn increment_code(&self, code: Icmpv6TimeExceededCode) {
        use Icmpv6TimeExceededCode::*;
        let counter = match code {
            HopLimitExceeded => &self.hop_limit_exceeded,
            FragmentReassemblyTimeExceeded => &self.fragment_reassembly_time_exceeded,
        };
        counter.increment();
    }
}

impl<C: CounterRepr> Inspectable for Icmpv6TimeExceededCounters<C> {
    fn record<I: Inspector>(&self, inspector: &mut I) {
        let Icmpv6TimeExceededCounters { hop_limit_exceeded, fragment_reassembly_time_exceeded } =
            self;
        inspector.record_counter("HopLimitExceeded", hop_limit_exceeded);
        inspector
            .record_counter("FragmentReassemblyTimeExceeded", fragment_reassembly_time_exceeded);
    }
}

/// ICMPv4 Counters for the Parameter Problem messages.
///
/// As defined by IANA:
/// https://www.iana.org/assignments/icmp-parameters/icmp-parameters.xhtml#icmp-parameters-codes-12
#[derive(Default, Debug, netstack3_macros::CounterCollection)]
#[cfg_attr(any(test, feature = "testutils"), derive(PartialEq,))]
pub struct Icmpv4ParameterProblemCounters<C: CounterRepr = Counter> {
    /// Pointer indicates the error, code 0
    pub pointer_indicates_error: C,
    /// Missing a Required Option, code 1
    pub missing_required_option: C,
    /// Bad Length, code 2
    pub bad_length: C,
}

impl Icmpv4ParameterProblemCounters<Counter> {
    pub(crate) fn increment_code(&self, code: Icmpv4ParameterProblemCode) {
        use Icmpv4ParameterProblemCode::*;
        let counter = match code {
            PointerIndicatesError => &self.pointer_indicates_error,
            MissingRequiredOption => &self.missing_required_option,
            BadLength => &self.bad_length,
        };
        counter.increment()
    }
}

impl<C: CounterRepr> Inspectable for Icmpv4ParameterProblemCounters<C> {
    fn record<I: Inspector>(&self, inspector: &mut I) {
        let Icmpv4ParameterProblemCounters {
            pointer_indicates_error,
            missing_required_option,
            bad_length,
        } = self;
        inspector.record_counter("PointerIndicatesError", pointer_indicates_error);
        inspector.record_counter("MissingRequiredOption", missing_required_option);
        inspector.record_counter("BadLength", bad_length);
    }
}

/// ICMPv6 Counters for the Parameter Problem messages.
///
/// As defined by IANA:
/// https://www.iana.org/assignments/icmpv6-parameters/icmpv6-parameters.xhtml#icmpv6-parameters-codes-5
#[derive(Default, Debug, netstack3_macros::CounterCollection)]
#[cfg_attr(any(test, feature = "testutils"), derive(PartialEq,))]
pub struct Icmpv6ParameterProblemCounters<C: CounterRepr = Counter> {
    /// Erroneous header field encountered, code 0
    pub erroneous_header_field: C,
    /// Unrecognized Next Header type encountered, code 1
    pub unrecognized_next_header_type: C,
    /// Unrecognized IPv6 option encountered, code 2
    pub unrecognized_ipv6_option: C,
}

impl Icmpv6ParameterProblemCounters<Counter> {
    pub(crate) fn increment_code(&self, code: Icmpv6ParameterProblemCode) {
        use Icmpv6ParameterProblemCode::*;
        let counter = match code {
            ErroneousHeaderField => &self.erroneous_header_field,
            UnrecognizedNextHeaderType => &self.unrecognized_next_header_type,
            UnrecognizedIpv6Option => &self.unrecognized_ipv6_option,
        };
        counter.increment()
    }
}

impl<C: CounterRepr> Inspectable for Icmpv6ParameterProblemCounters<C> {
    fn record<I: Inspector>(&self, inspector: &mut I) {
        let Icmpv6ParameterProblemCounters {
            erroneous_header_field,
            unrecognized_next_header_type,
            unrecognized_ipv6_option,
        } = self;
        inspector.record_counter("ErroneousHeaderField", erroneous_header_field);
        inspector.record_counter("UnrecognizedNextHeaderType", unrecognized_next_header_type);
        inspector.record_counter("UnrecognizedIpv6Option", unrecognized_ipv6_option);
    }
}
