// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Defines the entry point of TCP packets, by directing them into the correct
//! state machine.

use core::fmt::Debug;
use core::num::NonZeroU16;

use assert_matches::assert_matches;
use log::{debug, error, warn};
use net_types::ip::Ip;
use net_types::{MulticastAddress as _, SpecifiedAddr, Witness as _};
use netstack3_base::socket::{
    AddrIsMappedError, AddrVec, AddrVecIter, ConnAddr, ConnIpAddr, InsertError, IpAddrVec,
    ListenerAddr, ListenerIpAddr, SocketIpAddr, SocketIpAddrExt as _,
};
use netstack3_base::{
    BidirectionalConverter as _, Control, CounterContext, CtxPair, EitherDeviceId, IpDeviceAddr,
    Marks, Mss, NotFoundError, Payload, Segment, SegmentHeader, SeqNum, StrongDeviceIdentifier,
    VerifiedTcpSegment, WeakDeviceIdentifier,
};
use netstack3_filter::{
    FilterIpExt, SocketIngressFilterResult, SocketOpsFilter, TransportPacketSerializer,
};
use netstack3_hashmap::hash_map;
use netstack3_ip::socket::{IpSockCreationError, IpSocketArgs, MmsError};
use netstack3_ip::{
    IpHeaderInfo, IpTransportContext, LocalDeliveryPacketInfo, ReceiveIpPacketMeta,
    TransportIpContext,
};
use netstack3_trace::trace_duration;
use packet::{
    BufferMut, BufferView as _, EmptyBuf, FragmentedByteSlice, InnerPacketBuilder,
    NestablePacketBuilder as _, ParseBuffer,
};
use packet_formats::error::ParseError;
use packet_formats::ip::IpProto;
use packet_formats::tcp::{
    TcpFlowAndSeqNum, TcpOptionsTooLongError, TcpParseArgs, TcpSegment, TcpSegmentBuilder,
    TcpSegmentBuilderWithOptions, TcpSegmentRaw,
};

use crate::internal::base::{BufferSizes, ConnectionError, SocketOptions, TcpIpSockOptions};
use crate::internal::counters::{
    self, TcpCounterContext, TcpCountersRefs, TcpCountersWithoutSocket,
};
use crate::internal::socket::generators::{IsnGenerator, TimestampOffsetGenerator};
use crate::internal::socket::{
    self, AsThisStack as _, Connection, CoreTxMetadataContext, DemuxState, DeviceIpSocketHandler,
    DoSendLimit, DualStackBaseIpExt, DualStackDemuxIdConverter as _, DualStackIpExt, EitherStack,
    HandshakeStatus, Listener, ListenerAddrState, MaybeDualStack, PrimaryRc, TcpApi,
    TcpBindingsContext, TcpBindingsTypes, TcpContext, TcpDemuxContext, TcpDualStackContext,
    TcpIpTransportContext, TcpPortSpec, TcpSocketId, TcpSocketSetEntry, TcpSocketState,
    TcpSocketStateInner, TcpSocketTxMetadata,
};
use crate::internal::state::{
    BufferProvider, Closed, DataAcked, Initial, NewlyClosed, State, TimeWait,
};

impl<BT: TcpBindingsTypes> BufferProvider<BT::ReceiveBuffer, BT::SendBuffer> for BT {
    type ActiveOpen = BT::ListenerNotifierOrProvidedBuffers;

    type PassiveOpen = BT::ReturnedBuffers;

    fn new_passive_open_buffers(
        buffer_sizes: BufferSizes,
    ) -> (BT::ReceiveBuffer, BT::SendBuffer, Self::PassiveOpen) {
        BT::new_passive_open_buffers(buffer_sizes)
    }
}

/// Alias for a SocketId that can reference either V4 or V6 socket.
pub type DualStackTcpSocketId<I, D, BT> = <I as DualStackBaseIpExt>::DemuxSocketId<D, BT>;

impl<I, BC, CC> IpTransportContext<I, BC, CC> for TcpIpTransportContext
where
    I: DualStackIpExt,
    BC: TcpBindingsContext<CC::DeviceId>
        + BufferProvider<
            BC::ReceiveBuffer,
            BC::SendBuffer,
            ActiveOpen = <BC as TcpBindingsTypes>::ListenerNotifierOrProvidedBuffers,
            PassiveOpen = <BC as TcpBindingsTypes>::ReturnedBuffers,
        >,
    CC: TcpContext<I, BC> + TcpContext<I::OtherVersion, BC>,
{
    type EarlyDemuxSocket = DualStackTcpSocketId<I, CC::WeakDeviceId, BC>;

    fn early_demux<B: ParseBuffer>(
        core_ctx: &mut CC,
        device: &CC::DeviceId,
        src_ip: I::Addr,
        dst_ip: I::Addr,
        buffer: B,
    ) -> Option<Self::EarlyDemuxSocket> {
        early_demux_ip_packet::<I, _, _, _>(core_ctx, device, src_ip, dst_ip, buffer)
    }

    fn receive_icmp_error(
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        _device: &CC::DeviceId,
        original_src_ip: Option<SpecifiedAddr<I::Addr>>,
        original_dst_ip: SpecifiedAddr<I::Addr>,
        mut original_body: &[u8],
        err: I::ErrorCode,
    ) {
        let mut buffer = &mut original_body;
        let Some(flow_and_seqnum) = buffer.take_obj_front::<TcpFlowAndSeqNum>() else {
            error!("received an ICMP error but its body is less than 8 bytes");
            return;
        };

        let Some(original_src_ip) = original_src_ip else { return };
        let Some(original_src_port) = NonZeroU16::new(flow_and_seqnum.src_port()) else { return };
        let Some(original_dst_port) = NonZeroU16::new(flow_and_seqnum.dst_port()) else { return };
        let original_seqnum = SeqNum::new(flow_and_seqnum.sequence_num());

        TcpApi::<I, _>::new(CtxPair { core_ctx, bindings_ctx }).on_icmp_error(
            original_src_ip,
            original_dst_ip,
            original_src_port,
            original_dst_port,
            original_seqnum,
            err.into(),
        );
    }

    fn receive_ip_packet<B: BufferMut, H: IpHeaderInfo<I>>(
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        device: &CC::DeviceId,
        remote_ip: I::RecvSrcAddr,
        local_ip: SpecifiedAddr<I::Addr>,
        mut buffer: B,
        info: &mut LocalDeliveryPacketInfo<I, H>,
        early_demux_socket: Option<Self::EarlyDemuxSocket>,
    ) -> Result<(), (B, I::IcmpError)> {
        let LocalDeliveryPacketInfo { meta, header_info, marks } = info;
        let ReceiveIpPacketMeta { broadcast, transparent_override, parsing_context } = meta;
        if let Some(delivery) = transparent_override {
            warn!(
                "TODO(https://fxbug.dev/337009139): transparent proxy not supported for TCP \
                sockets; will not override dispatch to perform local delivery to {delivery:?}"
            );
        }

        // Per RFC 9293, Section 3.9.2.3 (referencing RFC 1122):
        //   A TCP implementation MUST silently discard an incoming SYN
        //   segment that is addressed to a broadcast or multicast address
        //   [(MUST-57)].
        //
        // and
        //
        //   ... this guidance is applicable to all incoming segments, not just
        //   SYNs ...
        if broadcast.is_some() {
            CounterContext::<TcpCountersWithoutSocket<I>>::counters(core_ctx)
                .invalid_ip_addrs_received
                .increment();
            debug!("tcp: dropping broadcast TCP packet");
            return Ok(());
        }
        if local_ip.is_multicast() {
            CounterContext::<TcpCountersWithoutSocket<I>>::counters(core_ctx)
                .invalid_ip_addrs_received
                .increment();
            debug!("tcp: dropping multicast TCP packet");
            return Ok(());
        }

        let remote_ip = match SpecifiedAddr::new(remote_ip.into_addr()) {
            None => {
                CounterContext::<TcpCountersWithoutSocket<I>>::counters(core_ctx)
                    .invalid_ip_addrs_received
                    .increment();
                debug!("tcp: source address unspecified, dropping the packet");
                return Ok(());
            }
            Some(src_ip) => src_ip,
        };
        let remote_ip: SocketIpAddr<_> = match remote_ip.try_into() {
            Ok(remote_ip) => remote_ip,
            Err(AddrIsMappedError {}) => {
                CounterContext::<TcpCountersWithoutSocket<I>>::counters(core_ctx)
                    .invalid_ip_addrs_received
                    .increment();
                debug!("tcp: source address is mapped (ipv4-mapped-ipv6), dropping the packet");
                return Ok(());
            }
        };
        let local_ip: SocketIpAddr<_> = match local_ip.try_into() {
            Ok(local_ip) => local_ip,
            Err(AddrIsMappedError {}) => {
                CounterContext::<TcpCountersWithoutSocket<I>>::counters(core_ctx)
                    .invalid_ip_addrs_received
                    .increment();
                debug!("tcp: local address is mapped (ipv4-mapped-ipv6), dropping the packet");
                return Ok(());
            }
        };
        let packet = match buffer.parse_with::<_, TcpSegment<_>>(TcpParseArgs::with_context(
            remote_ip.addr(),
            local_ip.addr(),
            parsing_context,
        )) {
            Ok(packet) => packet,
            Err(err) => {
                CounterContext::<TcpCountersWithoutSocket<I>>::counters(core_ctx)
                    .invalid_segments_received
                    .increment();
                debug!("tcp: failed parsing incoming packet {:?}", err);
                match err {
                    ParseError::Checksum => {
                        CounterContext::<TcpCountersWithoutSocket<I>>::counters(core_ctx)
                            .checksum_errors
                            .increment();
                    }
                    ParseError::NotSupported | ParseError::NotExpected | ParseError::Format => {}
                }
                return Ok(());
            }
        };
        let local_port = packet.dst_port();
        let remote_port = packet.src_port();
        let incoming = match VerifiedTcpSegment::try_from(packet) {
            Ok(segment) => segment,
            Err(err) => {
                CounterContext::<TcpCountersWithoutSocket<I>>::counters(core_ctx)
                    .invalid_segments_received
                    .increment();
                debug!("tcp: malformed segment {:?}", err);
                return Ok(());
            }
        };
        let conn_addr =
            ConnIpAddr { local: (local_ip, local_port), remote: (remote_ip, remote_port) };

        CounterContext::<TcpCountersWithoutSocket<I>>::counters(core_ctx)
            .valid_segments_received
            .increment();
        handle_incoming_packet::<I, _, _, _>(
            core_ctx,
            bindings_ctx,
            conn_addr,
            device,
            header_info,
            &incoming,
            marks,
            early_demux_socket,
        );
        Ok(())
    }
}

fn early_demux_ip_packet<I, BC, CC, B>(
    core_ctx: &mut CC,
    device: &CC::DeviceId,
    src_ip: I::Addr,
    dst_ip: I::Addr,
    mut buffer: B,
) -> Option<DualStackTcpSocketId<I, CC::WeakDeviceId, BC>>
where
    I: DualStackIpExt,
    BC: TcpBindingsContext<CC::DeviceId>,
    CC: TcpContext<I, BC> + TcpContext<I::OtherVersion, BC>,
    B: ParseBuffer,
{
    let Ok(packet) = buffer.parse_with::<_, TcpSegmentRaw<_>>(()) else {
        // If we fail to parse the packet then just return None. Invalid
        // packets are handled later.
        return None;
    };

    let src_ip = SocketIpAddr::new(src_ip)?;
    let dst_ip = SocketIpAddr::new(dst_ip)?;
    let (src_port, dst_port) = packet.flow_header().src_dst();
    let src_port = NonZeroU16::new(src_port)?;
    let dst_port = NonZeroU16::new(dst_port)?;
    let device = device.downgrade();

    core_ctx.with_demux(|demux: &DemuxState<I, _, _>| {
        demux
            .socketmap
            .lookup_connected((src_ip, src_port), (dst_ip, dst_port), device)
            .map(|entry| entry.id())
    })
}

fn handle_incoming_packet<WireI, BC, CC, H>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    conn_addr: ConnIpAddr<WireI::Addr, NonZeroU16, NonZeroU16>,
    incoming_device: &CC::DeviceId,
    header_info: &H,
    incoming: &VerifiedTcpSegment<'_>,
    marks: &Marks,
    mut early_demux_socket: Option<DualStackTcpSocketId<WireI, CC::WeakDeviceId, BC>>,
) where
    WireI: DualStackIpExt,
    BC: TcpBindingsContext<CC::DeviceId>
        + BufferProvider<
            BC::ReceiveBuffer,
            BC::SendBuffer,
            ActiveOpen = <BC as TcpBindingsTypes>::ListenerNotifierOrProvidedBuffers,
            PassiveOpen = <BC as TcpBindingsTypes>::ReturnedBuffers,
        >,
    CC: TcpContext<WireI, BC> + TcpContext<WireI::OtherVersion, BC>,
    H: IpHeaderInfo<WireI>,
{
    trace_duration!("tcp::handle_incoming_packet");
    let mut tw_reuse = None;

    // If we have an early demux socket, then we don't need to look up
    // connected sockets again. We may still need to lookup listener sockets
    // when the socket we found a in the timed wait state.
    let addr: IpAddrVec<WireI, _> = if early_demux_socket.is_some() {
        let (ip, port) = conn_addr.local;
        IpAddrVec::new_listener(ip, port)
    } else {
        conn_addr.into()
    };
    let mut addrs_to_search = AddrVecIter::<WireI, CC::WeakDeviceId, TcpPortSpec>::with_device(
        addr,
        incoming_device.downgrade(),
    );

    enum FoundSocket<S> {
        // Typically holds the demux ID of the found socket, but may hold
        // `None` if the found socket was destroyed as a result of the segment.
        Yes(Option<S>),
        No,
    }
    let found_socket = loop {
        let sock = if let Some(early_demux_socket) = early_demux_socket.take() {
            let device = match WireI::as_dual_stack_ip_socket(&early_demux_socket) {
                EitherStack::ThisStack(conn_id) => conn_id.get_bound_device(core_ctx),
                EitherStack::OtherStack(conn_id) => conn_id.get_bound_device(core_ctx),
            };
            let conn_addr = ConnAddr { ip: conn_addr, device };
            Some(SocketLookupResult::Connection(early_demux_socket, conn_addr))
        } else {
            core_ctx.with_demux(|demux| lookup_socket::<WireI, CC, BC>(demux, &mut addrs_to_search))
        };
        match sock {
            None => break FoundSocket::No,
            Some(SocketLookupResult::Connection(demux_conn_id, conn_addr)) => {
                // It is not possible to have two same connections that
                // share the same local and remote IPs and ports.
                assert_eq!(tw_reuse, None);
                let disposition = match WireI::as_dual_stack_ip_socket(&demux_conn_id) {
                    EitherStack::ThisStack(conn_id) => {
                        try_handle_incoming_for_connection_dual_stack(
                            core_ctx,
                            bindings_ctx,
                            conn_id,
                            incoming_device,
                            header_info,
                            &incoming,
                        )
                    }
                    EitherStack::OtherStack(conn_id) => {
                        try_handle_incoming_for_connection_dual_stack(
                            core_ctx,
                            bindings_ctx,
                            conn_id,
                            incoming_device,
                            header_info,
                            &incoming,
                        )
                    }
                };
                match disposition {
                    ConnectionIncomingSegmentDisposition::Destroy => {
                        WireI::destroy_socket_with_demux_id(core_ctx, bindings_ctx, demux_conn_id);
                        break FoundSocket::Yes(None);
                    }
                    ConnectionIncomingSegmentDisposition::FoundSocket
                    | ConnectionIncomingSegmentDisposition::Filtered => {
                        break FoundSocket::Yes(Some(demux_conn_id));
                    }
                    ConnectionIncomingSegmentDisposition::ReuseCandidateForListener => {
                        tw_reuse = Some((demux_conn_id, conn_addr));
                    }
                }
            }
            Some(SocketLookupResult::Listener((demux_listener_id, _listener_addr))) => {
                match WireI::as_dual_stack_ip_socket(&demux_listener_id) {
                    EitherStack::ThisStack(listener_id) => {
                        let disposition = core_ctx.with_socket_mut_generators_transport_demux(
                            &listener_id,
                            |core_ctx, socket_state, isn, timestamp_offset| match core_ctx {
                                MaybeDualStack::NotDualStack((core_ctx, converter)) => {
                                    try_handle_incoming_for_listener::<WireI, WireI, CC, BC, _, _>(
                                        core_ctx,
                                        bindings_ctx,
                                        &listener_id,
                                        isn,
                                        timestamp_offset,
                                        socket_state,
                                        header_info,
                                        incoming,
                                        conn_addr,
                                        incoming_device,
                                        &mut tw_reuse,
                                        move |conn, addr| converter.convert_back((conn, addr)),
                                        WireI::into_demux_socket_id,
                                        marks,
                                    )
                                }
                                MaybeDualStack::DualStack((core_ctx, converter)) => {
                                    try_handle_incoming_for_listener::<_, _, CC, BC, _, _>(
                                        core_ctx,
                                        bindings_ctx,
                                        &listener_id,
                                        isn,
                                        timestamp_offset,
                                        socket_state,
                                        header_info,
                                        incoming,
                                        conn_addr,
                                        incoming_device,
                                        &mut tw_reuse,
                                        move |conn, addr| {
                                            converter
                                                .convert_back(EitherStack::ThisStack((conn, addr)))
                                        },
                                        WireI::into_demux_socket_id,
                                        marks,
                                    )
                                }
                            },
                        );
                        if try_handle_listener_incoming_disposition(
                            core_ctx,
                            bindings_ctx,
                            disposition,
                            &demux_listener_id,
                            &mut tw_reuse,
                            &mut addrs_to_search,
                            conn_addr,
                            incoming_device,
                        ) {
                            break FoundSocket::Yes(Some(demux_listener_id));
                        }
                    }
                    EitherStack::OtherStack(listener_id) => {
                        let disposition = core_ctx.with_socket_mut_generators_transport_demux(
                            &listener_id,
                            |core_ctx, socket_state, isn, timestamp_offset| {
                                match core_ctx {
                                    MaybeDualStack::NotDualStack((_core_ctx, _converter)) => {
                                        // TODO(https://issues.fuchsia.dev/316408184):
                                        // Remove this unreachable!.
                                        unreachable!("OtherStack socket ID with non dual stack");
                                    }
                                    MaybeDualStack::DualStack((core_ctx, converter)) => {
                                        let other_demux_id_converter =
                                            core_ctx.other_demux_id_converter();
                                        try_handle_incoming_for_listener::<_, _, CC, BC, _, _>(
                                            core_ctx,
                                            bindings_ctx,
                                            &listener_id,
                                            isn,
                                            timestamp_offset,
                                            socket_state,
                                            header_info,
                                            incoming,
                                            conn_addr,
                                            incoming_device,
                                            &mut tw_reuse,
                                            move |conn, addr| {
                                                converter.convert_back(EitherStack::OtherStack((
                                                    conn, addr,
                                                )))
                                            },
                                            move |id| other_demux_id_converter.convert(id),
                                            marks,
                                        )
                                    }
                                }
                            },
                        );
                        if try_handle_listener_incoming_disposition::<_, _, CC, BC, _>(
                            core_ctx,
                            bindings_ctx,
                            disposition,
                            &demux_listener_id,
                            &mut tw_reuse,
                            &mut addrs_to_search,
                            conn_addr,
                            incoming_device,
                        ) {
                            break FoundSocket::Yes(Some(demux_listener_id));
                        }
                    }
                };
            }
        }
    };

    let demux_id = match found_socket {
        FoundSocket::No => {
            CounterContext::<TcpCountersWithoutSocket<WireI>>::counters(core_ctx)
                .received_segments_no_dispatch
                .increment();

            // There is no existing TCP state, pretend it is closed
            // and generate a RST if needed.
            // Per RFC 793 (https://tools.ietf.org/html/rfc793#page-21):
            // CLOSED is fictional because it represents the state when
            // there is no TCB, and therefore, no connection.
            if let Some(seg) =
                (Closed { reason: None::<Option<ConnectionError>> }.on_segment(&incoming.into()))
            {
                socket::send_tcp_segment::<WireI, WireI, _, _, _>(
                    core_ctx,
                    bindings_ctx,
                    None,
                    None,
                    conn_addr,
                    seg.into_empty(),
                    &TcpIpSockOptions { marks: *marks },
                );
            }
            None
        }
        FoundSocket::Yes(demux_id) => {
            counters::increment_counter_with_optional_demux_id::<WireI, _, _, _, _>(
                core_ctx,
                demux_id.as_ref(),
                |c| &c.received_segments_dispatched,
            );
            demux_id
        }
    };

    if let Some(control) = incoming.control() {
        counters::increment_counter_with_optional_demux_id::<WireI, _, _, _, _>(
            core_ctx,
            demux_id.as_ref(),
            |c| match control {
                Control::RST => &c.resets_received,
                Control::SYN => &c.syns_received,
                Control::FIN => &c.fins_received,
            },
        )
    }
}

enum SocketLookupResult<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes> {
    Connection(I::DemuxSocketId<D, BT>, ConnAddr<ConnIpAddr<I::Addr, NonZeroU16, NonZeroU16>, D>),
    Listener((I::DemuxSocketId<D, BT>, ListenerAddr<ListenerIpAddr<I::Addr, NonZeroU16>, D>)),
}

fn lookup_socket<I, CC, BC>(
    DemuxState { socketmap, .. }: &DemuxState<I, CC::WeakDeviceId, BC>,
    addrs_to_search: &mut AddrVecIter<I, CC::WeakDeviceId, TcpPortSpec>,
) -> Option<SocketLookupResult<I, CC::WeakDeviceId, BC>>
where
    I: DualStackIpExt,
    BC: TcpBindingsContext<CC::DeviceId>,
    CC: TcpContext<I, BC>,
{
    addrs_to_search.find_map(|addr| {
        match addr {
            // Connections are always searched before listeners because they
            // are more specific.
            AddrVec::Conn(conn_addr) => {
                socketmap.conns().get_by_addr(&conn_addr).map(|conn_addr_state| {
                    SocketLookupResult::Connection(conn_addr_state.id(), conn_addr)
                })
            }
            AddrVec::Listen(listener_addr) => {
                // If we have a listener and the incoming segment is a SYN, we
                // allocate a new connection entry in the demuxer.
                // TODO(https://fxbug.dev/42052878): Support SYN cookies.

                socketmap
                    .listeners()
                    .get_by_addr(&listener_addr)
                    .and_then(|addr_state| match addr_state {
                        ListenerAddrState::ExclusiveListener(id) => Some(id.clone()),
                        ListenerAddrState::Shared { listener: Some(id), bound: _ } => {
                            Some(id.clone())
                        }
                        ListenerAddrState::ExclusiveBound(_)
                        | ListenerAddrState::Shared { listener: None, bound: _ } => None,
                    })
                    .map(|id| SocketLookupResult::Listener((id, listener_addr)))
            }
        }
    })
}

#[derive(PartialEq, Eq)]
enum ConnectionIncomingSegmentDisposition {
    FoundSocket,
    Filtered,
    ReuseCandidateForListener,
    Destroy,
}

enum ListenerIncomingSegmentDisposition<S> {
    FoundSocket,
    Filtered,
    ConflictingConnection,
    NoMatchingSocket,
    NewConnection(S),
}

fn try_handle_incoming_for_connection_dual_stack<SockI, WireI, CC, BC, H>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    conn_id: &TcpSocketId<SockI, CC::WeakDeviceId, BC>,
    incoming_device: &CC::DeviceId,
    header_info: &H,
    incoming: &VerifiedTcpSegment<'_>,
) -> ConnectionIncomingSegmentDisposition
where
    SockI: DualStackIpExt,
    WireI: Ip,
    BC: TcpBindingsContext<CC::DeviceId>
        + BufferProvider<
            BC::ReceiveBuffer,
            BC::SendBuffer,
            ActiveOpen = <BC as TcpBindingsTypes>::ListenerNotifierOrProvidedBuffers,
            PassiveOpen = <BC as TcpBindingsTypes>::ReturnedBuffers,
        >,
    CC: TcpContext<SockI, BC>,
    H: IpHeaderInfo<WireI>,
{
    core_ctx.with_socket_mut_transport_demux(conn_id, |core_ctx, socket_state| {
        let TcpSocketState { socket_state, sharing: _, ip_options: _, socket_options } =
            socket_state;

        match run_socket_ingress_filter(
            bindings_ctx,
            incoming_device,
            conn_id.socket_info(),
            socket_options,
            header_info,
            incoming.tcp_segment(),
        ) {
            SocketIngressFilterResult::Accept => (),
            SocketIngressFilterResult::Drop => {
                return ConnectionIncomingSegmentDisposition::Filtered;
            }
        }

        let (conn_and_addr, timer) = assert_matches!(
            socket_state,
            TcpSocketStateInner::Connected { conn, timer } => (conn, timer),
            "invalid socket ID"
        );
        let this_or_other_stack = match core_ctx {
            MaybeDualStack::DualStack((core_ctx, converter)) => {
                match converter.convert(conn_and_addr) {
                    EitherStack::ThisStack((conn, conn_addr)) => {
                        // The socket belongs to the current stack, so we
                        // want to deliver the segment to this stack.
                        // Use `as_this_stack` to make the context types
                        // match with the non-dual-stack case.
                        EitherStack::ThisStack((
                            core_ctx.as_this_stack(),
                            conn,
                            conn_addr,
                            SockI::into_demux_socket_id(conn_id.clone()),
                        ))
                    }
                    EitherStack::OtherStack((conn, conn_addr)) => {
                        // We need to deliver from the other stack. i.e. we
                        // need to deliver an IPv4 packet to the IPv6 stack.
                        let demux_sock_id = core_ctx.into_other_demux_socket_id(conn_id.clone());
                        EitherStack::OtherStack((core_ctx, conn, conn_addr, demux_sock_id))
                    }
                }
            }
            MaybeDualStack::NotDualStack((core_ctx, converter)) => {
                let (conn, conn_addr) = converter.convert(conn_and_addr);
                // Similar to the first case, we need deliver to this stack,
                // but use `as_this_stack` to make the types match.
                EitherStack::ThisStack((
                    core_ctx.as_this_stack(),
                    conn,
                    conn_addr,
                    SockI::into_demux_socket_id(conn_id.clone()),
                ))
            }
        };

        match this_or_other_stack {
            EitherStack::ThisStack((core_ctx, conn, conn_addr, demux_conn_id)) => {
                try_handle_incoming_for_connection::<_, _, CC, _, _>(
                    core_ctx,
                    bindings_ctx,
                    conn_addr.clone(),
                    conn_id,
                    demux_conn_id,
                    socket_options,
                    conn,
                    timer,
                    incoming.into(),
                )
            }
            EitherStack::OtherStack((core_ctx, conn, conn_addr, demux_conn_id)) => {
                try_handle_incoming_for_connection::<_, _, CC, _, _>(
                    core_ctx,
                    bindings_ctx,
                    conn_addr.clone(),
                    conn_id,
                    demux_conn_id,
                    socket_options,
                    conn,
                    timer,
                    incoming.into(),
                )
            }
        }
    })
}

/// Tries to handle the incoming segment by providing it to a connected socket.
///
/// Returns `FoundSocket` if the segment was handled; Otherwise,
/// `ReuseCandidateForListener` will be returned if there is a defunct socket
/// that is currently in TIME_WAIT, which is ready to be reused if there is an
/// active listener listening on the port.
fn try_handle_incoming_for_connection<SockI, WireI, CC, BC, DC>(
    core_ctx: &mut DC,
    bindings_ctx: &mut BC,
    conn_addr: ConnAddr<ConnIpAddr<WireI::Addr, NonZeroU16, NonZeroU16>, CC::WeakDeviceId>,
    conn_id: &TcpSocketId<SockI, CC::WeakDeviceId, BC>,
    demux_id: WireI::DemuxSocketId<CC::WeakDeviceId, BC>,
    socket_options: &SocketOptions,
    conn: &mut Connection<SockI, WireI, CC::WeakDeviceId, BC>,
    timer: &mut BC::Timer,
    incoming: Segment<&[u8]>,
) -> ConnectionIncomingSegmentDisposition
where
    SockI: DualStackIpExt,
    WireI: DualStackIpExt,
    BC: TcpBindingsContext<CC::DeviceId>
        + BufferProvider<
            BC::ReceiveBuffer,
            BC::SendBuffer,
            ActiveOpen = <BC as TcpBindingsTypes>::ListenerNotifierOrProvidedBuffers,
            PassiveOpen = <BC as TcpBindingsTypes>::ReturnedBuffers,
        >,
    CC: TcpContext<SockI, BC>,
    DC: TransportIpContext<WireI, BC, DeviceId = CC::DeviceId, WeakDeviceId = CC::WeakDeviceId>
        + DeviceIpSocketHandler<SockI, BC>
        + TcpDemuxContext<WireI, CC::WeakDeviceId, BC>
        + TcpCounterContext<SockI, CC::WeakDeviceId, BC>
        + CoreTxMetadataContext<TcpSocketTxMetadata<SockI, CC::WeakDeviceId, BC>, BC>,
{
    let Connection { accept_queue, state, ip_sock, defunct, soft_error: _, handshake_status } =
        conn;

    // Per RFC 9293 Section 3.6.1:
    //   When a connection is closed actively, it MUST linger in the TIME-WAIT
    //   state for a time 2xMSL (Maximum Segment Lifetime) (MUST-13). However,
    //   it MAY accept a new SYN from the remote TCP endpoint to reopen the
    //   connection directly from TIME-WAIT state (MAY-2), if it:
    //
    //   (1) assigns its initial sequence number for the new connection to be
    //       larger than the largest sequence number it used on the previous
    //       connection incarnation, and
    //   (2) returns to TIME-WAIT state if the SYN turns out to be an old
    //       duplicate.
    if *defunct
        && incoming.header().control == Some(Control::SYN)
        && incoming.header().ack.is_none()
    {
        if let State::TimeWait(TimeWait { last_seq: _, closed_rcv, expiry: _, snd_info: _ }) = state
        {
            if !incoming.header().seq.before(closed_rcv.ack) {
                return ConnectionIncomingSegmentDisposition::ReuseCandidateForListener;
            }
        }
    }
    let (reply, passive_open, data_acked, newly_closed) = state.on_segment::<_, BC>(
        &conn_id.either(),
        &TcpCountersRefs::from_ctx(core_ctx, conn_id),
        incoming,
        bindings_ctx.now(),
        socket_options,
        *defunct,
    );

    match data_acked {
        DataAcked::Yes => {
            core_ctx.confirm_reachable(bindings_ctx, ip_sock, &socket_options.ip_options)
        }
        DataAcked::No => {}
    }

    match state {
        State::Listen(_) => {
            unreachable!("has an invalid status: {:?}", conn.state)
        }
        State::SynSent(_) | State::SynRcvd(_) => {
            assert_eq!(*handshake_status, HandshakeStatus::Pending)
        }
        State::Established(_)
        | State::FinWait1(_)
        | State::FinWait2(_)
        | State::Closing(_)
        | State::CloseWait(_)
        | State::LastAck(_)
        | State::TimeWait(_) => {
            if handshake_status
                .update_if_pending(HandshakeStatus::Completed { reported: accept_queue.is_some() })
            {
                core_ctx.confirm_reachable(bindings_ctx, ip_sock, &socket_options.ip_options);
            }
        }
        State::Closed(Closed { reason }) => {
            // We remove the socket from the socketmap and cancel the timers
            // regardless of the socket being defunct or not. The justification
            // is that CLOSED is a synthetic state and it means no connection
            // exists, thus it should not exist in the demuxer.
            //
            // If the socket was already in the closed state we can assume it's
            // no longer in the demux.
            socket::handle_newly_closed(
                core_ctx,
                bindings_ctx,
                newly_closed,
                &demux_id,
                &conn_addr,
                timer,
            );
            if let Some(accept_queue) = accept_queue {
                accept_queue.remove(&conn_id);
                *defunct = true;
            }
            if *defunct {
                // If the client has promised to not touch the socket again,
                // we can destroy the socket finally.
                return ConnectionIncomingSegmentDisposition::Destroy;
            }
            let _: bool = handshake_status.update_if_pending(match reason {
                None => HandshakeStatus::Completed { reported: accept_queue.is_some() },
                Some(_err) => HandshakeStatus::Aborted,
            });
        }
    }

    if let Some(seg) = reply {
        socket::send_tcp_segment(
            core_ctx,
            bindings_ctx,
            Some(conn_id),
            Some(&ip_sock),
            conn_addr.ip,
            seg.into_empty(),
            &socket_options.ip_options,
        );
    }

    // Send any enqueued data, if there is any.
    socket::do_send_inner_and_then_handle_newly_closed(
        conn_id,
        &demux_id,
        socket_options,
        conn,
        DoSendLimit::MultipleSegments,
        &conn_addr,
        timer,
        core_ctx,
        bindings_ctx,
    );

    // Enqueue the connection to the associated listener
    // socket's accept queue.
    if let Some(passive_open) = passive_open {
        let accept_queue = conn.accept_queue.as_ref().expect("no accept queue but passive open");
        accept_queue.notify_ready(conn_id, passive_open);
    }

    // We found a valid connection for the segment.
    ConnectionIncomingSegmentDisposition::FoundSocket
}

/// Responds to the disposition returned by [`try_handle_incoming_for_listener`].
///
/// Returns true if we have found the right socket and there is no need to
/// continue the iteration for finding the next-best candidate.
fn try_handle_listener_incoming_disposition<SockI, WireI, CC, BC, Addr>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    disposition: ListenerIncomingSegmentDisposition<PrimaryRc<SockI, CC::WeakDeviceId, BC>>,
    demux_listener_id: &WireI::DemuxSocketId<CC::WeakDeviceId, BC>,
    tw_reuse: &mut Option<(WireI::DemuxSocketId<CC::WeakDeviceId, BC>, Addr)>,
    addrs_to_search: &mut AddrVecIter<WireI, CC::WeakDeviceId, TcpPortSpec>,
    conn_addr: ConnIpAddr<WireI::Addr, NonZeroU16, NonZeroU16>,
    incoming_device: &CC::DeviceId,
) -> bool
where
    SockI: DualStackIpExt,
    WireI: DualStackIpExt,
    CC: TcpContext<SockI, BC> + TcpContext<WireI, BC> + TcpContext<WireI::OtherVersion, BC>,
    BC: TcpBindingsContext<CC::DeviceId>,
{
    match disposition {
        ListenerIncomingSegmentDisposition::FoundSocket => true,
        ListenerIncomingSegmentDisposition::Filtered => true,
        ListenerIncomingSegmentDisposition::ConflictingConnection => {
            // We're about to rewind the lookup. If we got a
            // conflicting connection it means tw_reuse has been
            // removed from the demux state and we need to destroy
            // it.
            if let Some((tw_reuse, _)) = tw_reuse.take() {
                WireI::destroy_socket_with_demux_id(core_ctx, bindings_ctx, tw_reuse);
            }

            // Reset the address vector iterator and go again, a
            // conflicting connection was found.
            *addrs_to_search = AddrVecIter::<WireI, CC::WeakDeviceId, TcpPortSpec>::with_device(
                conn_addr.into(),
                incoming_device.downgrade(),
            );
            false
        }
        ListenerIncomingSegmentDisposition::NoMatchingSocket => false,
        ListenerIncomingSegmentDisposition::NewConnection(primary) => {
            // If we have a new connection, we need to add it to the
            // set of all sockets.

            // First things first, if we got here then tw_reuse is
            // gone so we need to destroy it.
            if let Some((tw_reuse, _)) = tw_reuse.take() {
                WireI::destroy_socket_with_demux_id(core_ctx, bindings_ctx, tw_reuse);
            }

            // Now put the new connection into the socket map.
            //
            // Note that there's a possible subtle race here where
            // another thread could have already operated further on
            // this connection and marked it for destruction which
            // puts the entry in the DOA state, if we see that we
            // must immediately destroy the socket after having put
            // it in the map.
            let id = TcpSocketId(PrimaryRc::clone_strong(&primary));
            let to_destroy = core_ctx.with_all_sockets_mut(move |all_sockets| {
                let insert_entry = TcpSocketSetEntry::Primary(primary);
                match all_sockets.entry(id) {
                    hash_map::Entry::Vacant(v) => {
                        let _: &mut _ = v.insert(insert_entry);
                        None
                    }
                    hash_map::Entry::Occupied(mut o) => {
                        // We're holding on to the primary ref, the
                        // only possible state here should be a DOA
                        // entry.
                        assert_matches!(
                            core::mem::replace(o.get_mut(), insert_entry),
                            TcpSocketSetEntry::DeadOnArrival
                        );
                        Some(o.key().clone())
                    }
                }
            });
            // NB: we're releasing and reaquiring the
            // all_sockets_mut lock here for the convenience of not
            // needing different versions of `destroy_socket`. This
            // should be fine because the race this is solving
            // should not be common. If we have correct thread
            // attribution per flow it should effectively become
            // impossible so we go for code simplicity here.
            if let Some(to_destroy) = to_destroy {
                socket::destroy_socket(core_ctx, bindings_ctx, to_destroy);
            }
            counters::increment_counter_for_demux_id::<WireI, _, _, _, _>(
                core_ctx,
                demux_listener_id,
                |c| &c.passive_connection_openings,
            );
            true
        }
    }
}

/// Tries to handle an incoming segment by passing it to a listening socket.
///
/// Returns `FoundSocket` if the segment was handled, otherwise `NoMatchingSocket`.
fn try_handle_incoming_for_listener<SockI, WireI, CC, BC, DC, H>(
    core_ctx: &mut DC,
    bindings_ctx: &mut BC,
    listener_id: &TcpSocketId<SockI, CC::WeakDeviceId, BC>,
    isn: &IsnGenerator<BC::Instant>,
    timestamp_offset: &TimestampOffsetGenerator<BC::Instant>,
    socket_state: &mut TcpSocketState<SockI, CC::WeakDeviceId, BC>,
    header_info: &H,
    incoming: &VerifiedTcpSegment<'_>,
    incoming_addrs: ConnIpAddr<WireI::Addr, NonZeroU16, NonZeroU16>,
    incoming_device: &CC::DeviceId,
    tw_reuse: &mut Option<(
        WireI::DemuxSocketId<CC::WeakDeviceId, BC>,
        ConnAddr<ConnIpAddr<WireI::Addr, NonZeroU16, NonZeroU16>, CC::WeakDeviceId>,
    )>,
    make_connection: impl FnOnce(
        Connection<SockI, WireI, CC::WeakDeviceId, BC>,
        ConnAddr<ConnIpAddr<WireI::Addr, NonZeroU16, NonZeroU16>, CC::WeakDeviceId>,
    ) -> SockI::ConnectionAndAddr<CC::WeakDeviceId, BC>,
    make_demux_id: impl Fn(
        TcpSocketId<SockI, CC::WeakDeviceId, BC>,
    ) -> WireI::DemuxSocketId<CC::WeakDeviceId, BC>,
    marks: &Marks,
) -> ListenerIncomingSegmentDisposition<PrimaryRc<SockI, CC::WeakDeviceId, BC>>
where
    SockI: DualStackIpExt,
    WireI: DualStackIpExt,
    BC: TcpBindingsContext<CC::DeviceId>
        + BufferProvider<
            BC::ReceiveBuffer,
            BC::SendBuffer,
            ActiveOpen = <BC as TcpBindingsTypes>::ListenerNotifierOrProvidedBuffers,
            PassiveOpen = <BC as TcpBindingsTypes>::ReturnedBuffers,
        >,
    CC: TcpContext<SockI, BC>,
    DC: TransportIpContext<WireI, BC, DeviceId = CC::DeviceId, WeakDeviceId = CC::WeakDeviceId>
        + DeviceIpSocketHandler<WireI, BC>
        + TcpDemuxContext<WireI, CC::WeakDeviceId, BC>
        + TcpCounterContext<SockI, CC::WeakDeviceId, BC>
        + CoreTxMetadataContext<TcpSocketTxMetadata<SockI, CC::WeakDeviceId, BC>, BC>,
    H: IpHeaderInfo<WireI>,
{
    let Listener { addr: listener_addr, accept_queue, backlog, buffer_sizes } =
        match &socket_state.socket_state {
            TcpSocketStateInner::Bound(_) => {
                // If the socket is only bound, but not listening.
                return ListenerIncomingSegmentDisposition::NoMatchingSocket;
            }
            TcpSocketStateInner::Listener(listener) => listener,
            _ => panic!("unexpected socket state: {:?}", socket_state.socket_state),
        };

    let ConnIpAddr { local: (local_ip, local_port), remote: (remote_ip, remote_port) } =
        incoming_addrs;

    match run_socket_ingress_filter(
        bindings_ctx,
        incoming_device,
        listener_id.socket_info(),
        &socket_state.socket_options,
        header_info,
        incoming.tcp_segment(),
    ) {
        SocketIngressFilterResult::Accept => (),
        SocketIngressFilterResult::Drop => {
            return ListenerIncomingSegmentDisposition::Filtered;
        }
    }

    // Note that this checks happens at the very beginning, before we try to
    // reuse the connection in TIME-WAIT, this is because we need to store the
    // reused connection in the accept queue so we have to respect its limit.
    if accept_queue.len() == backlog.get() {
        core_ctx.increment_both(listener_id, |counters| &counters.listener_queue_overflow);
        core_ctx.increment_both(listener_id, |counters| &counters.failed_connection_attempts);
        debug!("incoming SYN dropped because of the full backlog of the listener");
        return ListenerIncomingSegmentDisposition::FoundSocket;
    }

    // Ensure that if the remote address requires a zone, we propagate that to
    // the address for the connected socket.
    let bound_device = listener_addr.as_ref().clone();
    let bound_device = if remote_ip.as_ref().must_have_zone() {
        Some(bound_device.map_or(EitherDeviceId::Strong(incoming_device), EitherDeviceId::Weak))
    } else {
        bound_device.map(EitherDeviceId::Weak)
    };

    let ip_options = TcpIpSockOptions { marks: *marks, ..socket_state.socket_options.ip_options };
    let socket_options = SocketOptions { ip_options, ..socket_state.socket_options };

    let bound_device = bound_device.as_ref().map(|d| d.as_ref());
    let ip_sock = match core_ctx.new_ip_socket(
        bindings_ctx,
        IpSocketArgs {
            device: bound_device,
            local_ip: IpDeviceAddr::new_from_socket_ip_addr(local_ip),
            remote_ip,
            proto: IpProto::Tcp.into(),
            options: &ip_options,
        },
    ) {
        Ok(ip_sock) => ip_sock,
        err @ Err(IpSockCreationError::Route(_)) => {
            core_ctx.increment_both(listener_id, |counters| &counters.passive_open_no_route_errors);
            core_ctx.increment_both(listener_id, |counters| &counters.failed_connection_attempts);
            debug!("cannot construct an ip socket to the SYN originator: {:?}, ignoring", err);
            return ListenerIncomingSegmentDisposition::NoMatchingSocket;
        }
    };

    let isn = isn.generate(
        bindings_ctx.now(),
        (ip_sock.local_ip().clone().into(), local_port),
        (ip_sock.remote_ip().clone(), remote_port),
    );
    let timestamp_offset = timestamp_offset.generate::<SocketIpAddr<WireI::Addr>, NonZeroU16>(
        bindings_ctx.now(),
        (ip_sock.local_ip().clone().into(), local_port),
        (ip_sock.remote_ip().clone(), remote_port),
    );
    let device_mms = match core_ctx.get_mms(bindings_ctx, &ip_sock, &socket_options.ip_options) {
        Ok(mms) => mms,
        Err(err) => {
            // If we cannot find a device or the device's MTU is too small,
            // there isn't much we can do here since sending a RST back is
            // impossible, we just need to silent drop the segment.
            error!("Cannot find a device with large enough MTU for the connection");
            core_ctx.increment_both(listener_id, |counters| &counters.failed_connection_attempts);
            match err {
                MmsError::NoDevice(_) | MmsError::MTUTooSmall(_) => {
                    return ListenerIncomingSegmentDisposition::FoundSocket;
                }
            }
        }
    };
    let Some(device_mss) = Mss::from_mms(device_mms) else {
        return ListenerIncomingSegmentDisposition::FoundSocket;
    };

    let mut state = State::Listen(Closed::<Initial>::listen(
        isn,
        timestamp_offset,
        buffer_sizes.clone(),
        device_mss,
        Mss::default::<WireI>(),
        socket_options.user_timeout,
    ));

    // Prepare a reply to be sent out.
    //
    // We might end up discarding the reply in case we can't instantiate this
    // new connection.
    let result = state.on_segment::<_, BC>(
        // NB: This is a bit of a lie, we're passing the listener ID to process
        // the first segment because we don't have an ID allocated yet. This is
        // okay because the state machine ID is only for debugging purposes.
        &listener_id.either(),
        &TcpCountersRefs::from_ctx(core_ctx, listener_id),
        incoming.into(),
        bindings_ctx.now(),
        &SocketOptions::default(),
        false, /* defunct */
    );
    let reply = assert_matches!(
        result,
        (reply, None, /* data_acked */ _, NewlyClosed::No /* can't become closed */) => reply
    );

    let result = if matches!(state, State::SynRcvd(_)) {
        let poll_send_at = state.poll_send_at().expect("no retrans timer");
        let bound_device = ip_sock.device().cloned();

        let addr = ConnAddr {
            ip: ConnIpAddr { local: (local_ip, local_port), remote: (remote_ip, remote_port) },
            device: bound_device,
        };

        let new_socket = core_ctx.with_demux_mut(|DemuxState { socketmap, .. }| {
            // If we're reusing an entry, remove it from the demux before
            // proceeding.
            //
            // We could just reuse the old allocation for the new connection but
            // because of the restrictions on the socket map data structure (for
            // good reasons), we can't update the sharing info unconditionally.
            // So here we just remove the old connection and create a new one.
            // Also this approach has the benefit of not accidentally persisting
            // the old state that we don't want.
            if let Some((tw_reuse, conn_addr)) = tw_reuse {
                match socketmap.conns_mut().remove(tw_reuse, &conn_addr) {
                    Ok(()) => {
                        // NB: We're removing the tw_reuse connection from the
                        // demux here, but not canceling its timer. The timer is
                        // canceled via drop when we destroy the socket. Special
                        // care is taken when handling timers in the time wait
                        // state to account for this.
                    }
                    Err(NotFoundError) => {
                        // We could lose a race trying to reuse the tw_reuse
                        // socket, so we just accept the loss and be happy that
                        // the conn_addr we want to use is free.
                    }
                }
            }

            // Try to create and add the new socket to the demux.
            let accept_queue_clone = accept_queue.clone();
            let ip_sock = ip_sock.clone();
            let bindings_ctx_moved = &mut *bindings_ctx;
            let sharing = socket_state.sharing;
            match socketmap.conns_mut().try_insert_with(addr, sharing, move |addr, sharing| {
                let conn = make_connection(
                    Connection {
                        accept_queue: Some(accept_queue_clone),
                        state,
                        ip_sock,
                        defunct: false,
                        soft_error: None,
                        handshake_status: HandshakeStatus::Pending,
                    },
                    addr,
                );

                let (id, primary) = TcpSocketId::new_cyclic(
                    |weak| {
                        let mut timer = CC::new_timer(bindings_ctx_moved, weak);
                        // Schedule the timer here because we can't acquire the lock
                        // later. This only runs when inserting into the demux
                        // succeeds so it's okay.
                        assert_eq!(
                            bindings_ctx_moved.schedule_timer_instant(poll_send_at, &mut timer),
                            None
                        );
                        TcpSocketStateInner::Connected { conn, timer }
                    },
                    sharing,
                    socket_options,
                );
                (make_demux_id(id.clone()), (primary, id))
            }) {
                Ok((_entry, (primary, id))) => {
                    // Make sure the new socket is in the pending accept queue
                    // before we release the demux lock.
                    accept_queue.push_pending(id);
                    Some(primary)
                }
                Err(e) => {
                    // The only error we accept here is if the entry exists
                    // fully, any indirect conflicts are unexpected because we
                    // know the listener is still alive and installed in the
                    // demux.
                    assert_matches!(e, InsertError::Exists);
                    // If we fail to insert it means we lost a race and this
                    // packet is destined to a connection that is already
                    // established. In that case we should tell the demux code
                    // to retry demuxing it all over again.
                    None
                }
            }
        });

        match new_socket {
            Some(new_socket) => ListenerIncomingSegmentDisposition::NewConnection(new_socket),
            None => {
                // We didn't create a new connection, short circuit early and
                // don't send out the pending segment.
                core_ctx
                    .increment_both(listener_id, |counters| &counters.failed_connection_attempts);
                return ListenerIncomingSegmentDisposition::ConflictingConnection;
            }
        }
    } else {
        // We found a valid listener for the segment even if the connection
        // state is not a newly pending connection.
        ListenerIncomingSegmentDisposition::FoundSocket
    };

    // We can send a reply now if we got here.
    if let Some(seg) = reply {
        socket::send_tcp_segment(
            core_ctx,
            bindings_ctx,
            Some(&listener_id),
            Some(&ip_sock),
            incoming_addrs,
            seg.into_empty(),
            &socket_options.ip_options,
        );
    }

    result
}

pub(super) fn tcp_serialize_segment<'a, I, P>(
    header: &'a SegmentHeader,
    data: P,
    conn_addr: ConnIpAddr<I::Addr, NonZeroU16, NonZeroU16>,
) -> impl TransportPacketSerializer<I, Buffer = EmptyBuf> + Debug + 'a
where
    I: FilterIpExt,
    P: InnerPacketBuilder + Debug + Payload + 'a,
{
    let SegmentHeader { seq, ack, wnd, control, options, push } = header;
    let ConnIpAddr { local: (local_ip, local_port), remote: (remote_ip, remote_port) } = conn_addr;
    let mut builder = TcpSegmentBuilder::new(
        local_ip.addr(),
        remote_ip.addr(),
        local_port,
        remote_port,
        (*seq).into(),
        ack.map(Into::into),
        u16::from(*wnd),
    );
    builder.psh(*push);
    match control {
        None => {}
        Some(Control::SYN) => builder.syn(true),
        Some(Control::FIN) => builder.fin(true),
        Some(Control::RST) => builder.rst(true),
    }
    TcpSegmentBuilderWithOptions::new(builder, options.builder())
        .unwrap_or_else(|TcpOptionsTooLongError| {
            panic!("Too many TCP options");
        })
        .wrap_body(data.into_serializer())
}

fn run_socket_ingress_filter<I, BC, D>(
    bindings_ctx: &BC,
    incoming_device: &D,
    socket_info: netstack3_base::socket::SocketInfo,
    socket_options: &SocketOptions,
    header_info: &impl IpHeaderInfo<I>,
    tcp_segment: &TcpSegment<&'_ [u8]>,
) -> SocketIngressFilterResult
where
    I: Ip,
    BC: TcpBindingsContext<D>,
    D: StrongDeviceIdentifier,
{
    let [ip_prefix, ip_options] = header_info.as_bytes();
    let [tcp_prefix, tcp_options, _data] = tcp_segment.as_bytes();
    let mut slices = [ip_prefix, ip_options, tcp_prefix, tcp_options, _data];
    let packet = FragmentedByteSlice::new(&mut slices);
    let header_len = ip_prefix.len() + ip_options.len() + tcp_prefix.len() + tcp_options.len();

    bindings_ctx.socket_ops_filter().on_ingress(
        I::VERSION,
        packet,
        header_len,
        incoming_device,
        socket_info,
        &socket_options.ip_options.marks,
    )
}

#[cfg(test)]
mod test {
    use ip_test_macro::ip_test;
    use netstack3_base::{
        HandshakeOptions, NetworkSerializationContext, Options, ResetOptions, SackBlocks,
        SegmentOptions, UnscaledWindowSize,
    };
    use packet::Serializer as _;
    use packet_formats::tcp::options::TcpOptions as _;
    use test_case::test_case;

    use super::*;

    trait TestIpExt: netstack3_base::testutil::TestIpExt + FilterIpExt {}
    impl<T> TestIpExt for T where T: netstack3_base::testutil::TestIpExt + FilterIpExt {}

    const SEQ: SeqNum = SeqNum::new(12345);
    const ACK: SeqNum = SeqNum::new(67890);
    const FAKE_DATA: &'static [u8] = &[1, 2, 3, 4, 5, 6, 7, 8, 9, 0];

    #[ip_test(I)]
    #[test_case(
        Segment::syn(SEQ, UnscaledWindowSize::from(u16::MAX),
        HandshakeOptions::default()), &[]
        ; "syn")]
    #[test_case(
        Segment::syn(SEQ, UnscaledWindowSize::from(u16::MAX),
        HandshakeOptions {
            mss: Some(Mss::new(1440).unwrap()),
            ..Default::default() }), &[]
            ; "syn with mss")]
    #[test_case(
        Segment::ack(SEQ, ACK, UnscaledWindowSize::from(u16::MAX), SegmentOptions::default()),
        &[]; "ack")]
    #[test_case(Segment::with_fake_data(SEQ, ACK, FAKE_DATA), FAKE_DATA; "data")]
    #[test_case(Segment::new_assert_no_discard(SegmentHeader {
            seq: SEQ,
            ack: Some(ACK),
            push: true,
            wnd: UnscaledWindowSize::from(u16::MAX),
            ..Default::default()
        },
        FAKE_DATA
    ), FAKE_DATA; "push")]
    fn tcp_serialize_segment<I: TestIpExt>(segment: Segment<&[u8]>, expected_body: &[u8]) {
        const SOURCE_PORT: NonZeroU16 = NonZeroU16::new(1111).unwrap();
        const DEST_PORT: NonZeroU16 = NonZeroU16::new(2222).unwrap();

        let (header, data) = segment.into_parts();
        let serializer = super::tcp_serialize_segment::<I, _>(
            &header,
            data,
            ConnIpAddr {
                local: (SocketIpAddr::try_from(I::TEST_ADDRS.local_ip).unwrap(), SOURCE_PORT),
                remote: (SocketIpAddr::try_from(I::TEST_ADDRS.remote_ip).unwrap(), DEST_PORT),
            },
        );

        let mut serialized = serializer
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .unwrap_b();
        let parsed_segment = serialized
            .parse_with::<_, TcpSegment<_>>(TcpParseArgs::new(
                *I::TEST_ADDRS.remote_ip,
                *I::TEST_ADDRS.local_ip,
            ))
            .expect("is valid segment");

        assert_eq!(parsed_segment.src_port(), SOURCE_PORT);
        assert_eq!(parsed_segment.dst_port(), DEST_PORT);
        assert_eq!(parsed_segment.seq_num(), u32::from(SEQ));
        assert_eq!(parsed_segment.psh(), header.push);
        assert_eq!(
            UnscaledWindowSize::from(parsed_segment.window_size()),
            UnscaledWindowSize::from(u16::MAX)
        );

        let (mss, window_scale, sack_permitted, sack_blocks, timestamp) = match header.options {
            Options::Handshake(HandshakeOptions {
                mss,
                window_scale,
                sack_permitted,
                timestamp,
            }) => (mss, window_scale, sack_permitted, SackBlocks::EMPTY, timestamp),
            Options::Segment(SegmentOptions { timestamp, sack_blocks }) => {
                (None, None, false, sack_blocks, timestamp)
            }
            Options::Reset(ResetOptions { timestamp }) => {
                (None, None, false, SackBlocks::EMPTY, timestamp)
            }
        };
        assert_eq!(mss.map(|mss| mss.get()), parsed_segment.options().mss());
        assert_eq!(window_scale.map(|ws| ws.get()), parsed_segment.options().window_scale());
        assert_eq!(sack_permitted, parsed_segment.options().sack_permitted());
        assert_eq!(sack_blocks.as_slice(), parsed_segment.options().sack_blocks());
        assert_eq!(
            timestamp.as_ref().map(Into::into).as_ref(),
            parsed_segment.options().timestamp()
        );

        assert_eq!(parsed_segment.into_body(), expected_body);
    }
}
