// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Extensions for the fuchsia.sockets FIDL library.

#![warn(
    missing_docs,
    unreachable_patterns,
    clippy::useless_conversion,
    clippy::redundant_clone,
    clippy::precedence
)]

use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_ext::{IntoExt, Marks};
use fidl_fuchsia_net_matchers as fnet_matchers;
use fidl_fuchsia_net_matchers_ext as fnet_matchers_ext;
use fidl_fuchsia_net_sockets as fnet_sockets;
use fidl_fuchsia_net_tcp as fnet_tcp;
use fidl_fuchsia_net_udp as fnet_udp;
use futures::{Stream, TryStreamExt as _};
use net_types::ip::{self, GenericOverIp, Ip, IpInvariant, Ipv4, Ipv6};
use thiserror::Error;

/// An extension type for [`fnet_sockets::IpSocketMatcher`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpSocketMatcher {
    /// Matches against the IP version of the socket.
    Family(ip::IpVersion),
    /// Matches against the source address of the socket.
    SrcAddr(fnet_matchers_ext::BoundAddress),
    /// Matches against the destination address of the socket.
    DstAddr(fnet_matchers_ext::BoundAddress),
    /// Matches against transport protocol fields of the socket.
    Proto(fnet_matchers_ext::SocketTransportProtocol),
    /// Matches against the (bound, i.e. SO_BINDTODEVICE) interface of the
    /// socket.
    BoundInterface(fnet_matchers_ext::BoundInterface),
    /// Matches against the cookie of the socket (i.e. SO_COOKIE)
    Cookie(fnet_matchers::SocketCookie),
    /// Matches against one mark of the socket.
    Mark(fnet_matchers_ext::MarkInDomain),
}

/// Errors returned by the conversion from [`fnet_sockets::IpSocketMatcher`]
/// to [`IpSocketMatcher`].
#[derive(Debug, PartialEq, Error)]
pub enum IpSocketMatcherError {
    /// A union type was unknown.
    #[error("got unexpected union variant: {0}")]
    UnknownUnionVariant(u64),
    /// An error was encountered when converting one of the address matchers.
    #[error("address matcher conversion failure: {0}")]
    Address(fnet_matchers_ext::BoundAddressError),
    /// An error was encountered when converting the transport protocol
    /// matcher.
    #[error("protocol matcher conversion failure: {0}")]
    TransportProtocol(fnet_matchers_ext::SocketTransportProtocolError),
    /// An error was encountered while converting the interface matcher.
    #[error("bound interface matcher conversion failure: {0}")]
    BoundInterface(fnet_matchers_ext::BoundInterfaceError),
    /// An error was encountered when converting one of the mark matchers.
    #[error("mark matcher conversion failure: {0}")]
    Mark(fnet_matchers_ext::MarkInDomainError),
}

impl TryFrom<fnet_sockets::IpSocketMatcher> for IpSocketMatcher {
    type Error = IpSocketMatcherError;

    fn try_from(matcher: fnet_sockets::IpSocketMatcher) -> Result<Self, Self::Error> {
        match matcher {
            fnet_sockets::IpSocketMatcher::Family(ip_version) => {
                Ok(Self::Family(ip_version.into_ext()))
            }
            fnet_sockets::IpSocketMatcher::SrcAddr(addr) => {
                Ok(Self::SrcAddr(addr.try_into().map_err(|e| IpSocketMatcherError::Address(e))?))
            }
            fnet_sockets::IpSocketMatcher::DstAddr(addr) => {
                Ok(Self::DstAddr(addr.try_into().map_err(|e| IpSocketMatcherError::Address(e))?))
            }
            fnet_sockets::IpSocketMatcher::Proto(proto) => Ok(Self::Proto(
                proto.try_into().map_err(|e| IpSocketMatcherError::TransportProtocol(e))?,
            )),
            fnet_sockets::IpSocketMatcher::BoundInterface(bound_interface) => {
                Ok(Self::BoundInterface(
                    bound_interface
                        .try_into()
                        .map_err(|e| IpSocketMatcherError::BoundInterface(e))?,
                ))
            }
            fnet_sockets::IpSocketMatcher::Cookie(cookie) => Ok(Self::Cookie(cookie)),
            fnet_sockets::IpSocketMatcher::Mark(mark) => {
                Ok(Self::Mark(mark.try_into().map_err(|e| IpSocketMatcherError::Mark(e))?))
            }
            fnet_sockets::IpSocketMatcher::__SourceBreaking { unknown_ordinal } => {
                Err(IpSocketMatcherError::UnknownUnionVariant(unknown_ordinal))
            }
        }
    }
}

impl From<IpSocketMatcher> for fnet_sockets::IpSocketMatcher {
    fn from(value: IpSocketMatcher) -> Self {
        match value {
            IpSocketMatcher::Family(ip_version) => {
                fnet_sockets::IpSocketMatcher::Family(ip_version.into_ext())
            }
            IpSocketMatcher::SrcAddr(address) => {
                fnet_sockets::IpSocketMatcher::SrcAddr(address.into())
            }
            IpSocketMatcher::DstAddr(address) => {
                fnet_sockets::IpSocketMatcher::DstAddr(address.into())
            }
            IpSocketMatcher::Proto(socket_transport_protocol) => {
                fnet_sockets::IpSocketMatcher::Proto(socket_transport_protocol.into())
            }
            IpSocketMatcher::BoundInterface(mark) => {
                fnet_sockets::IpSocketMatcher::BoundInterface(mark.into())
            }
            IpSocketMatcher::Cookie(socket_cookie) => {
                fnet_sockets::IpSocketMatcher::Cookie(socket_cookie)
            }
            IpSocketMatcher::Mark(mark) => fnet_sockets::IpSocketMatcher::Mark(mark.into()),
        }
    }
}

/// Extension type for [`fnet_sockets::IpSocketState`].
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum IpSocketState {
    /// IPv4 socket state.
    V4(IpSocketStateSpecific<Ipv4>),
    /// IPv6 socket state.
    V6(IpSocketStateSpecific<Ipv6>),
}

/// Error type for [`IpSocketState`] conversion.
#[derive(Debug, Error, PartialEq)]
pub enum IpSocketStateError {
    /// Missing a required field.
    #[error("missing field: {0}")]
    MissingField(&'static str),
    /// The socket address version does not match the expected version.
    #[error("version mismatch")]
    VersionMismatch,
    /// The transport state is invalid.
    #[error("transport state error: {0}")]
    Transport(IpSocketTransportStateError),
}

impl TryFrom<fnet_sockets::IpSocketState> for IpSocketState {
    type Error = IpSocketStateError;

    fn try_from(value: fnet_sockets::IpSocketState) -> Result<Self, Self::Error> {
        fn convert_address<I: Ip>(addr: fnet::IpAddress) -> Result<I::Addr, IpSocketStateError> {
            I::map_ip::<_, Option<I::Addr>>(
                IpInvariant(addr.into_ext()),
                |IpInvariant(addr)| match addr {
                    net_types::ip::IpAddr::V4(addr) => Some(addr),
                    _ => None,
                },
                |IpInvariant(addr)| match addr {
                    net_types::ip::IpAddr::V6(addr) => Some(addr),
                    _ => None,
                },
            )
            .ok_or(IpSocketStateError::VersionMismatch)
        }

        fn to_ip_socket_specific<I: Ip>(
            src_addr: Option<fnet::IpAddress>,
            dst_addr: Option<fnet::IpAddress>,
            cookie: u64,
            marks: fnet::Marks,
            transport: fnet_sockets::IpSocketTransportState,
        ) -> Result<IpSocketStateSpecific<I>, IpSocketStateError> {
            let src_addr: Option<I::Addr> = src_addr.map(convert_address::<I>).transpose()?;
            let dst_addr: Option<I::Addr> = dst_addr.map(convert_address::<I>).transpose()?;

            Ok(IpSocketStateSpecific {
                src_addr,
                dst_addr,
                cookie,
                marks: marks.into(),
                transport: transport.try_into().map_err(IpSocketStateError::Transport)?,
            })
        }

        let fnet_sockets::IpSocketState {
            family,
            src_addr,
            dst_addr,
            cookie,
            marks,
            transport,
            __source_breaking,
        } = value;

        let family = family.ok_or(IpSocketStateError::MissingField("family"))?;
        let cookie = cookie.ok_or(IpSocketStateError::MissingField("cookie"))?;
        let marks = marks.ok_or(IpSocketStateError::MissingField("marks"))?;
        let transport = transport.ok_or(IpSocketStateError::MissingField("transport"))?;

        match family {
            fnet::IpVersion::V4 => Ok(IpSocketState::V4(to_ip_socket_specific(
                src_addr, dst_addr, cookie, marks, transport,
            )?)),
            fnet::IpVersion::V6 => Ok(IpSocketState::V6(to_ip_socket_specific(
                src_addr, dst_addr, cookie, marks, transport,
            )?)),
        }
    }
}

impl From<IpSocketState> for fnet_sockets::IpSocketState {
    fn from(state: IpSocketState) -> Self {
        match state {
            IpSocketState::V4(state) => state.into(),
            IpSocketState::V6(state) => state.into(),
        }
    }
}

/// Lowest-level socket state information that ensures all fields are for the
/// same IP version.
#[derive(Debug, PartialEq, Eq, Clone, GenericOverIp)]
#[generic_over_ip(I, Ip)]
pub struct IpSocketStateSpecific<I: Ip> {
    /// The source address of the socket.
    pub src_addr: Option<I::Addr>,
    /// The destination address of the socket.
    pub dst_addr: Option<I::Addr>,
    /// The cookie of the socket.
    pub cookie: u64,
    /// The marks of the socket.
    pub marks: Marks,
    /// The transport state of the socket.
    pub transport: IpSocketTransportState,
}

impl<I: Ip> From<IpSocketStateSpecific<I>> for fnet_sockets::IpSocketState {
    fn from(value: IpSocketStateSpecific<I>) -> Self {
        let IpSocketStateSpecific { src_addr, dst_addr, cookie, marks, transport } = value;

        fnet_sockets::IpSocketState {
            family: Some(I::VERSION.into_ext()),
            src_addr: src_addr.map(|a| net_types::ip::IpAddr::from(a).into_ext()),
            dst_addr: dst_addr.map(|a| net_types::ip::IpAddr::from(a).into_ext()),
            cookie: Some(cookie),
            marks: Some(marks.into()),
            transport: Some(transport.into()),
            __source_breaking: fidl::marker::SourceBreaking,
        }
    }
}

/// Extension type for [`fnet_sockets::IpSocketTransportState`].
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum IpSocketTransportState {
    /// TCP socket state.
    Tcp(IpSocketTcpState),
    /// UDP socket state.
    Udp(IpSocketUdpState),
}

/// Error type for [`IpSocketTransportState`] conversion.
#[derive(Debug, PartialEq, Error)]
pub enum IpSocketTransportStateError {
    /// Error converting a TCP socket state.
    #[error("tcp validation error: {0}")]
    Tcp(IpSocketTcpStateError),
    /// Error converting a UDP socket state.
    #[error("udp validation error: {0}")]
    Udp(IpSocketUdpStateError),
    /// A union type was unknown.
    #[error("got unexpected union variant: {0}")]
    UnknownUnionVariant(u64),
}

impl TryFrom<fnet_sockets::IpSocketTransportState> for IpSocketTransportState {
    type Error = IpSocketTransportStateError;

    fn try_from(value: fnet_sockets::IpSocketTransportState) -> Result<Self, Self::Error> {
        match value {
            fnet_sockets::IpSocketTransportState::Tcp(tcp) => Ok(IpSocketTransportState::Tcp(
                tcp.try_into().map_err(IpSocketTransportStateError::Tcp)?,
            )),
            fnet_sockets::IpSocketTransportState::Udp(udp) => Ok(IpSocketTransportState::Udp(
                udp.try_into().map_err(IpSocketTransportStateError::Udp)?,
            )),
            fnet_sockets::IpSocketTransportState::__SourceBreaking { unknown_ordinal } => {
                Err(IpSocketTransportStateError::UnknownUnionVariant(unknown_ordinal))
            }
        }
    }
}

impl From<IpSocketTransportState> for fnet_sockets::IpSocketTransportState {
    fn from(state: IpSocketTransportState) -> Self {
        match state {
            IpSocketTransportState::Tcp(tcp) => {
                fnet_sockets::IpSocketTransportState::Tcp(tcp.into())
            }
            IpSocketTransportState::Udp(udp) => {
                fnet_sockets::IpSocketTransportState::Udp(udp.into())
            }
        }
    }
}

/// Extension type for [`fnet_sockets::IpSocketTcpState`].
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct IpSocketTcpState {
    /// The source port of the socket.
    pub src_port: Option<u16>,
    /// The destination port of the socket.
    pub dst_port: Option<u16>,
    /// The TCP state machine state for the socket.
    pub state: fnet_tcp::State,
    /// Extended TCP information if the TCP_INFO extension was requested.
    pub tcp_info: Option<TcpInfo>,
}

/// Error type for [`IpSocketTcpState`] conversion.
#[derive(Debug, PartialEq, Error)]
pub enum IpSocketTcpStateError {
    /// Missing a required field.
    #[error("missing field: {0}")]
    MissingField(&'static str),
    /// Error converting a [`TcpInfo`].
    #[error("tcp info error: {0}")]
    TcpInfo(TcpInfoError),
}

impl TryFrom<fnet_sockets::IpSocketTcpState> for IpSocketTcpState {
    type Error = IpSocketTcpStateError;

    fn try_from(value: fnet_sockets::IpSocketTcpState) -> Result<Self, Self::Error> {
        let fnet_sockets::IpSocketTcpState {
            src_port,
            dst_port,
            state,
            tcp_info,
            __source_breaking,
        } = value;

        let state = state.ok_or(IpSocketTcpStateError::MissingField("state"))?;

        Ok(IpSocketTcpState {
            src_port,
            dst_port,
            state,
            tcp_info: tcp_info
                .map(|t| t.try_into())
                .transpose()
                .map_err(|e| IpSocketTcpStateError::TcpInfo(e))?,
        })
    }
}

impl From<IpSocketTcpState> for fnet_sockets::IpSocketTcpState {
    fn from(state: IpSocketTcpState) -> Self {
        let IpSocketTcpState { src_port, dst_port, state, tcp_info } = state;
        fnet_sockets::IpSocketTcpState {
            src_port,
            dst_port,
            state: Some(state),
            tcp_info: tcp_info.map(Into::into),
            __source_breaking: fidl::marker::SourceBreaking,
        }
    }
}

/// Extension type for [`fnet_tcp::Info`].
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct TcpInfo {
    /// The state of the TCP connection.
    pub state: fnet_tcp::State,
    /// The congestion control state of the TCP connection.
    pub ca_state: fnet_tcp::CongestionControlState,
    /// The retransmission timeout of the TCP connection in microseconds.
    pub rto_usec: Option<u32>,
    /// The time since the most recent data was sent on the connection in milliseconds.
    pub tcpi_last_data_sent_msec: Option<u32>,
    /// The time since the most recent ACK was received in milliseconds.
    pub tcpi_last_ack_recv_msec: Option<u32>,
    /// The estimated smoothed roundtrip time in microseconds.
    pub rtt_usec: Option<u32>,
    /// The smoothed mean deviation of the roundtrip time in microseconds.
    pub rtt_var_usec: Option<u32>,
    /// The sending slow start threshold in segments.
    pub snd_ssthresh: u32,
    /// The current sending congestion window in segments.
    pub snd_cwnd: u32,
    /// The total number of retransmissions.
    pub tcpi_total_retrans: u32,
    /// The total number of segments sent.
    pub tcpi_segs_out: u64,
    /// The total number of segments received.
    pub tcpi_segs_in: u64,
    /// Whether reordering has been seen on the connection.
    pub reorder_seen: bool,
    /// The send MSS for this endpoint.
    pub tcpi_snd_mss: Option<u32>,
    /// The receive MSS for this endpoint.
    pub tcpi_rcv_mss: Option<u32>,
}

/// Error type for [`TcpInfo`] conversion.
#[derive(Debug, PartialEq, Error)]
pub enum TcpInfoError {
    /// Missing a required field.
    #[error("missing field: {0}")]
    MissingField(&'static str),
}

impl TryFrom<fnet_tcp::Info> for TcpInfo {
    type Error = TcpInfoError;

    fn try_from(value: fnet_tcp::Info) -> Result<Self, Self::Error> {
        let fnet_tcp::Info {
            state,
            ca_state,
            rto_usec,
            tcpi_last_data_sent_msec,
            tcpi_last_ack_recv_msec,
            rtt_usec,
            rtt_var_usec,
            snd_ssthresh,
            snd_cwnd,
            tcpi_total_retrans,
            tcpi_segs_out,
            tcpi_segs_in,
            reorder_seen,
            tcpi_snd_mss,
            tcpi_rcv_mss,
            __source_breaking,
        } = value;

        Ok(TcpInfo {
            state: state.ok_or(TcpInfoError::MissingField("state"))?,
            ca_state: ca_state.ok_or(TcpInfoError::MissingField("ca_state"))?,
            rto_usec,
            tcpi_last_data_sent_msec,
            tcpi_last_ack_recv_msec,
            rtt_usec,
            rtt_var_usec,
            snd_ssthresh: snd_ssthresh.ok_or(TcpInfoError::MissingField("snd_ssthresh"))?,
            snd_cwnd: snd_cwnd.ok_or(TcpInfoError::MissingField("snd_cwnd"))?,
            tcpi_total_retrans: tcpi_total_retrans
                .ok_or(TcpInfoError::MissingField("tcpi_total_retrans"))?,
            tcpi_segs_out: tcpi_segs_out.ok_or(TcpInfoError::MissingField("tcpi_segs_out"))?,
            tcpi_segs_in: tcpi_segs_in.ok_or(TcpInfoError::MissingField("tcpi_segs_in"))?,
            reorder_seen: reorder_seen.ok_or(TcpInfoError::MissingField("reorder_seen"))?,
            tcpi_snd_mss,
            tcpi_rcv_mss,
        })
    }
}

impl From<TcpInfo> for fnet_tcp::Info {
    fn from(info: TcpInfo) -> Self {
        let TcpInfo {
            state,
            ca_state,
            rto_usec,
            tcpi_last_data_sent_msec,
            tcpi_last_ack_recv_msec,
            rtt_usec,
            rtt_var_usec,
            snd_ssthresh,
            snd_cwnd,
            tcpi_total_retrans,
            tcpi_segs_out,
            tcpi_segs_in,
            reorder_seen,
            tcpi_snd_mss,
            tcpi_rcv_mss,
        } = info;
        fnet_tcp::Info {
            state: Some(state),
            ca_state: Some(ca_state),
            rto_usec: rto_usec,
            tcpi_last_data_sent_msec,
            tcpi_last_ack_recv_msec,
            rtt_usec: rtt_usec,
            rtt_var_usec: rtt_var_usec,
            snd_ssthresh: Some(snd_ssthresh),
            snd_cwnd: Some(snd_cwnd),
            tcpi_total_retrans: Some(tcpi_total_retrans),
            tcpi_segs_out: Some(tcpi_segs_out),
            tcpi_segs_in: Some(tcpi_segs_in),
            reorder_seen: Some(reorder_seen),
            tcpi_snd_mss,
            tcpi_rcv_mss,
            __source_breaking: fidl::marker::SourceBreaking,
        }
    }
}

/// Extension type for [`fnet_sockets::IpSocketUdpState`].
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct IpSocketUdpState {
    /// The source port of the socket.
    pub src_port: Option<u16>,
    /// The destination port of the socket.
    pub dst_port: Option<u16>,
    /// The UDP pseudo-state machine state for the socket.
    pub state: fnet_udp::State,
}

/// Error type for [`IpSocketUdpState`] conversion.
#[derive(Debug, PartialEq, Error)]
pub enum IpSocketUdpStateError {
    /// Missing a required field.
    #[error("missing field: {0}")]
    MissingField(&'static str),
}

impl TryFrom<fnet_sockets::IpSocketUdpState> for IpSocketUdpState {
    type Error = IpSocketUdpStateError;

    fn try_from(value: fnet_sockets::IpSocketUdpState) -> Result<Self, Self::Error> {
        let fnet_sockets::IpSocketUdpState { src_port, dst_port, state, __source_breaking } = value;

        let state = state.ok_or(IpSocketUdpStateError::MissingField("state"))?;

        Ok(IpSocketUdpState { src_port, dst_port, state })
    }
}

impl From<IpSocketUdpState> for fnet_sockets::IpSocketUdpState {
    fn from(state: IpSocketUdpState) -> Self {
        let IpSocketUdpState { src_port, dst_port, state } = state;
        fnet_sockets::IpSocketUdpState {
            src_port,
            dst_port,
            state: Some(state),
            __source_breaking: fidl::marker::SourceBreaking,
        }
    }
}

/// Errors returned by [`iterate_ip`]
#[derive(Debug, Error)]
pub enum IterateIpError {
    /// The specified matcher was the first invalid one.
    #[error("invalid matcher at position {0}")]
    InvalidMatcher(usize),
    /// An unknown response was received on the call to `Diagnostics.IterateIp`
    #[error("unknown ordinal on Diagnostics.IterateIp call: {0}")]
    UnknownOrdinal(u64),
    /// A low-level FIDL error was encountered on the call to
    /// `Diagnostics.IterateIp`.
    #[error("fidl error during Diagnostics.IterateIp call: {0}")]
    Fidl(fidl::Error),
}

impl From<fidl::Error> for IterateIpError {
    fn from(e: fidl::Error) -> Self {
        IterateIpError::Fidl(e)
    }
}

/// Errors returned by the stream returned from [`iterate_ip`].
#[derive(Debug, Error)]
pub enum IpIteratorError {
    /// The netstack returned an empty batch of sockets
    #[error("received empty batch of sockets")]
    EmptyBatch,
    /// A low-level FIDL error was encountered on the call to
    /// `Diagnostics.IterateIp`.
    #[error("fidl error during Diagnostics.IterateIp call: {0}")]
    Fidl(fidl::Error),
    /// An error was encountered while converting a socket state.
    #[error("error converting socket state: {0}")]
    Conversion(IpSocketStateError),
}

impl From<fidl::Error> for IpIteratorError {
    fn from(e: fidl::Error) -> Self {
        IpIteratorError::Fidl(e)
    }
}

/// Send a request to `Diagnostics.IterateIp` and drive the resulting
/// `IpIterator`.
///
/// `IpIterator` returns a series of batches of sockets matching the query, the
/// returned stream flattens those batches into individual sockets. If an error
/// is encuontered during iteration, it is returned and iteration halts.
//
// TODO(https://github.com/rust-lang/rust/issues/130043): Remove types from the
// precise capturing clause on the stream.
pub async fn iterate_ip<M, I>(
    diagnostics: &fnet_sockets::DiagnosticsProxy,
    extensions: fnet_sockets::Extensions,
    matchers: M,
) -> Result<impl Stream<Item = Result<IpSocketState, IpIteratorError>> + use<M, I>, IterateIpError>
where
    M: IntoIterator<Item = I>,
    I: Into<fnet_sockets::IpSocketMatcher>,
{
    let (proxy, server_end) = fidl::endpoints::create_proxy::<fnet_sockets::IpIteratorMarker>();
    match diagnostics
        .iterate_ip(
            server_end,
            extensions,
            &matchers.into_iter().map(Into::into).collect::<Vec<_>>()[..],
        )
        .await?
    {
        fnet_sockets::IterateIpResult::Ok(_empty) => Ok(()),
        fnet_sockets::IterateIpResult::InvalidMatcher(fnet_sockets::InvalidMatcher { index }) => {
            Err(IterateIpError::InvalidMatcher(index as usize))
        }
        fnet_sockets::IterateIpResult::__SourceBreaking { unknown_ordinal } => {
            Err(IterateIpError::UnknownOrdinal(unknown_ordinal))
        }
    }?;

    Ok(futures::stream::try_unfold((proxy, true), |(proxy, has_more)| async move {
        if !has_more {
            return Ok(None);
        }

        let (batch, has_more) = proxy.next().await?;
        if batch.is_empty() && has_more {
            Err(IpIteratorError::EmptyBatch)
        } else {
            Ok(Some((
                futures::stream::iter(
                    batch
                        .into_iter()
                        .map(|s| s.try_into().map_err(|e| IpIteratorError::Conversion(e))),
                ),
                (proxy, has_more),
            )))
        }
    })
    .try_flatten())
}

/// Errors returned by [`disconnect_ip`]
#[derive(Debug, Error)]
pub enum DisconnectIpError {
    /// The specified matcher was the first invalid one.
    #[error("invalid matcher at position {0}")]
    InvalidMatcher(usize),
    /// Specified matchers would a priori match all sockets.
    #[error("matchers were unconstrained")]
    UnconstrainedMatchers,
    /// An unknown response was received on the call to `Control.DisconnectIp`
    #[error("unknown ordinal on Control.DisconnectIp call: {0}")]
    UnknownOrdinal(u64),
    /// A low-level FIDL error was encountered on the call to
    /// `Control.DisconnectIp`.
    #[error("fidl error during Control.DisconnectIp call: {0}")]
    Fidl(fidl::Error),
}

/// Send a request to `Control.DisconnectIp` with the provided matchers.
pub async fn disconnect_ip<M, I>(
    control: &fnet_sockets::ControlProxy,
    matchers: M,
) -> Result<usize, DisconnectIpError>
where
    M: IntoIterator<Item = I>,
    I: Into<fnet_sockets::IpSocketMatcher>,
{
    match control
        .disconnect_ip(&fnet_sockets::ControlDisconnectIpRequest {
            matchers: Some(matchers.into_iter().map(Into::into).collect()),
            __source_breaking: fidl::marker::SourceBreaking,
        })
        .await
    {
        Ok(r) => match r {
            fnet_sockets::DisconnectIpResult::Ok(fnet_sockets::DisconnectIpResponse {
                disconnected,
            }) => {
                // Unwrap is safe because usize is always at least u32.
                Ok(disconnected.try_into().unwrap())
            }
            fnet_sockets::DisconnectIpResult::InvalidMatcher(fnet_sockets::InvalidMatcher {
                index,
            }) => {
                // Unwrap is safe because usize is always at least u32.
                Err(DisconnectIpError::InvalidMatcher(index.try_into().unwrap()))
            }
            fnet_sockets::DisconnectIpResult::UnconstrainedMatchers(fnet_sockets::Empty) => {
                Err(DisconnectIpError::UnconstrainedMatchers)
            }
            fnet_sockets::DisconnectIpResult::__SourceBreaking { unknown_ordinal } => {
                Err(DisconnectIpError::UnknownOrdinal(unknown_ordinal))
            }
        },
        Err(e) => Err(DisconnectIpError::Fidl(e)),
    }
}

/// Errors returned by the stream returned from [`watch_destruction`].
#[derive(Debug, Error)]
pub enum DestructionWatcherError {
    /// A low-level FIDL error was encountered on the call to
    /// `DestructionWatcher.Watch`.
    #[error("fidl error during DestructionWatcher.Watch call: {0}")]
    Fidl(fidl::Error),
    /// An error was encountered while converting a socket state.
    #[error("error converting socket state: {0}")]
    Conversion(IpSocketStateError),
    /// The netstack returned an empty batch of sockets.
    #[error("received empty batch of sockets")]
    EmptyBatch,
    /// Events were dropped
    #[error("events were dropped")]
    DroppedEvents(u64),
}

impl From<fidl::Error> for DestructionWatcherError {
    fn from(e: fidl::Error) -> Self {
        DestructionWatcherError::Fidl(e)
    }
}

/// Get a destruction watcher and drive it to yield individual sockets.
pub async fn watch_destruction(
    diagnostics: &fnet_sockets::DiagnosticsProxy,
) -> Result<impl Stream<Item = Result<IpSocketState, DestructionWatcherError>> + use<>, fidl::Error>
{
    let (proxy, server_end) =
        fidl::endpoints::create_proxy::<fnet_sockets::DestructionWatcherMarker>();
    diagnostics.get_destruction_watcher(server_end).await?;

    Ok(futures::stream::try_unfold(proxy, |proxy| async {
        let (sockets, dropped_events) = proxy.watch().await?;

        let items = if sockets.is_empty() {
            vec![Err(DestructionWatcherError::EmptyBatch)]
        } else {
            let dropped_events_iter = if dropped_events > 0 {
                Some(Err(DestructionWatcherError::DroppedEvents(dropped_events)))
            } else {
                None
            };

            sockets
                .into_iter()
                .map(|s| s.try_into().map_err(DestructionWatcherError::Conversion))
                .chain(dropped_events_iter)
                .collect()
        };

        Ok::<_, fidl::Error>(Some((futures::stream::iter(items), proxy)))
    })
    .try_flatten())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::num::NonZeroU64;

    use assert_matches::assert_matches;
    use fidl_fuchsia_net as fnet;
    use fidl_fuchsia_net_tcp as fnet_tcp;
    use futures::{FutureExt as _, StreamExt as _, future, pin_mut};
    use net_declare::{fidl_ip, fidl_subnet, net_ip_v4, net_ip_v6};
    use test_case::test_case;

    #[test_case(
        fnet_sockets::IpSocketMatcher::Family(fnet::IpVersion::V4),
        IpSocketMatcher::Family(ip::IpVersion::V4);
        "FamilyIpv4"
    )]
    #[test_case(
        fnet_sockets::IpSocketMatcher::Family(fnet::IpVersion::V6),
        IpSocketMatcher::Family(ip::IpVersion::V6);
        "FamilyIpv6"
    )]
    #[test_case(
        fnet_sockets::IpSocketMatcher::SrcAddr(fnet_matchers::BoundAddress::Bound(
            fnet_matchers::Address {
                matcher: fnet_matchers::AddressMatcherType::Subnet(fidl_subnet!("192.0.2.0/24")),
                invert: true,
            }
        )),
        IpSocketMatcher::SrcAddr(fnet_matchers_ext::BoundAddress::Bound(
            fnet_matchers_ext::Address {
                matcher: fnet_matchers_ext::AddressMatcherType::Subnet(
                    fnet_matchers_ext::Subnet::try_from(fidl_subnet!("192.0.2.0/24")).unwrap()
                ),
                invert: true,
            }
        ));
        "SrcAddr"
    )]
    #[test_case(
        fnet_sockets::IpSocketMatcher::DstAddr(fnet_matchers::BoundAddress::Bound(
            fnet_matchers::Address {
                matcher: fnet_matchers::AddressMatcherType::Subnet(fidl_subnet!("2001:db8::/32")),
                invert: false,
            }
        )),
        IpSocketMatcher::DstAddr(fnet_matchers_ext::BoundAddress::Bound(
            fnet_matchers_ext::Address {
                matcher: fnet_matchers_ext::AddressMatcherType::Subnet(
                    fnet_matchers_ext::Subnet::try_from(fidl_subnet!("2001:db8::/32")).unwrap()
                ),
                invert: false,
            }
        ));
        "DstAddr"
    )]
    #[test_case(
        fnet_sockets::IpSocketMatcher::Proto(fnet_matchers::SocketTransportProtocol::Tcp(
            fnet_matchers::TcpSocket::Empty(fnet_matchers::Empty)
        )),
        IpSocketMatcher::Proto(fnet_matchers_ext::SocketTransportProtocol::Tcp(
            fnet_matchers_ext::TcpSocket::Empty
        ));
        "ProtoTcp"
    )]
    #[test_case(
        fnet_sockets::IpSocketMatcher::Proto(fnet_matchers::SocketTransportProtocol::Udp(
            fnet_matchers::UdpSocket::Empty(fnet_matchers::Empty)
        )),
        IpSocketMatcher::Proto(fnet_matchers_ext::SocketTransportProtocol::Udp(
            fnet_matchers_ext::UdpSocket::Empty
        ));
        "ProtoUdp"
    )]
    #[test_case(
        fnet_sockets::IpSocketMatcher::BoundInterface(fnet_matchers::BoundInterface::Unbound(
            fnet_matchers::Empty
        )),
        IpSocketMatcher::BoundInterface(fnet_matchers_ext::BoundInterface::Unbound);
        "BoundInterfaceUnbound"
    )]
    #[test_case(
        fnet_sockets::IpSocketMatcher::BoundInterface(fnet_matchers::BoundInterface::Bound(
            fnet_matchers::Interface::Id(1)
        )),
        IpSocketMatcher::BoundInterface(fnet_matchers_ext::BoundInterface::Bound(
            fnet_matchers_ext::Interface::Id(NonZeroU64::new(1).unwrap())
        ));
        "BoundInterfaceBound"
    )]
    #[test_case(
        fnet_sockets::IpSocketMatcher::Cookie(fnet_matchers::SocketCookie {
            cookie: 12345,
            invert: false,
        }),
        IpSocketMatcher::Cookie(fnet_matchers::SocketCookie {
            cookie: 12345,
            invert: false,
        });
        "Cookie"
    )]
    #[test_case(
        fnet_sockets::IpSocketMatcher::Mark(fnet_matchers::MarkInDomain {
            domain: fnet::MarkDomain::Mark1,
            mark: fnet_matchers::Mark::Unmarked(fnet_matchers::Unmarked),
        }),
        IpSocketMatcher::Mark(fnet_matchers_ext::MarkInDomain {
            domain: fnet::MarkDomain::Mark1,
            mark: fnet_matchers_ext::Mark::Unmarked,
        });
        "Mark"
    )]
    #[test_case(
        fnet_sockets::IpSocketMatcher::SrcAddr(fnet_matchers::BoundAddress::Unbound(fnet_matchers::Empty)),
        IpSocketMatcher::SrcAddr(fnet_matchers_ext::BoundAddress::Unbound);
        "SrcAddrUnbound"
    )]
    #[test_case(
        fnet_sockets::IpSocketMatcher::DstAddr(fnet_matchers::BoundAddress::Unbound(fnet_matchers::Empty)),
        IpSocketMatcher::DstAddr(fnet_matchers_ext::BoundAddress::Unbound);
        "DstAddrUnbound"
    )]
    #[test_case(
        fnet_sockets::IpSocketMatcher::Proto(fnet_matchers::SocketTransportProtocol::Tcp(
            fnet_matchers::TcpSocket::SrcPort(fnet_matchers::BoundPort::Unbound(fnet_matchers::Empty))
        )),
        IpSocketMatcher::Proto(fnet_matchers_ext::SocketTransportProtocol::Tcp(
            fnet_matchers_ext::TcpSocket::SrcPort(fnet_matchers_ext::BoundPort::Unbound)
        ));
        "ProtoTcpSrcPortUnbound"
    )]
    #[test_case(
        fnet_sockets::IpSocketMatcher::Proto(fnet_matchers::SocketTransportProtocol::Tcp(
            fnet_matchers::TcpSocket::DstPort(fnet_matchers::BoundPort::Unbound(fnet_matchers::Empty))
        )),
        IpSocketMatcher::Proto(fnet_matchers_ext::SocketTransportProtocol::Tcp(
            fnet_matchers_ext::TcpSocket::DstPort(fnet_matchers_ext::BoundPort::Unbound)
        ));
        "ProtoTcpDstPortUnbound"
    )]
    #[test_case(
        fnet_sockets::IpSocketMatcher::Proto(fnet_matchers::SocketTransportProtocol::Udp(
            fnet_matchers::UdpSocket::SrcPort(fnet_matchers::BoundPort::Unbound(fnet_matchers::Empty))
        )),
        IpSocketMatcher::Proto(fnet_matchers_ext::SocketTransportProtocol::Udp(
            fnet_matchers_ext::UdpSocket::SrcPort(fnet_matchers_ext::BoundPort::Unbound)
        ));
        "ProtoUdpSrcPortUnbound"
    )]
    #[test_case(
        fnet_sockets::IpSocketMatcher::Proto(fnet_matchers::SocketTransportProtocol::Udp(
            fnet_matchers::UdpSocket::DstPort(fnet_matchers::BoundPort::Unbound(fnet_matchers::Empty))
        )),
        IpSocketMatcher::Proto(fnet_matchers_ext::SocketTransportProtocol::Udp(
            fnet_matchers_ext::UdpSocket::DstPort(fnet_matchers_ext::BoundPort::Unbound)
        ));
        "ProtoUdpDstPortUnbound"
    )]
    #[test_case(
        fnet_tcp::Info {
            state: Some(fnet_tcp::State::Established),
            ca_state: Some(fnet_tcp::CongestionControlState::Open),
            rto_usec: Some(1),
            tcpi_last_data_sent_msec: Some(2),
            tcpi_last_ack_recv_msec: Some(3),
            rtt_usec: Some(4),
            rtt_var_usec: Some(5),
            snd_ssthresh: Some(6),
            snd_cwnd: Some(7),
            tcpi_total_retrans: Some(8),
            tcpi_segs_out: Some(9),
            tcpi_segs_in: Some(10),
            reorder_seen: Some(true),
            tcpi_snd_mss: Some(11),
            tcpi_rcv_mss: Some(12),
            __source_breaking: fidl::marker::SourceBreaking,
        },
        TcpInfo {
            state: fnet_tcp::State::Established,
            ca_state: fnet_tcp::CongestionControlState::Open,
            rto_usec: Some(1),
            tcpi_last_data_sent_msec: Some(2),
            tcpi_last_ack_recv_msec: Some(3),
            rtt_usec: Some(4),
            rtt_var_usec: Some(5),
            snd_ssthresh: 6,
            snd_cwnd: 7,
            tcpi_total_retrans: 8,
            tcpi_segs_out: 9,
            tcpi_segs_in: 10,
            reorder_seen: true,
            tcpi_snd_mss: Some(11),
            tcpi_rcv_mss: Some(12),
        };
        "TcpInfo"
    )]
    #[test_case(
        fnet_sockets::IpSocketState {
            family: Some(fnet::IpVersion::V4),
            src_addr: Some(fidl_ip!("192.168.1.1")),
            dst_addr: Some(fidl_ip!("192.168.1.2")),
            cookie: Some(1234),
            marks: Some(fnet::Marks {
                mark_1: Some(1111),
                mark_2: None,
                __source_breaking: fidl::marker::SourceBreaking,
            }),
            transport: Some(fnet_sockets::IpSocketTransportState::Tcp(
                fnet_sockets::IpSocketTcpState {
                    src_port: Some(1111),
                    dst_port: Some(2222),
                    state: Some(fnet_tcp::State::Established),
                    tcp_info: None,
                    __source_breaking: fidl::marker::SourceBreaking,
                },
            )),
            __source_breaking: fidl::marker::SourceBreaking,
        },
        IpSocketState::V4(IpSocketStateSpecific {
            src_addr: Some(net_ip_v4!("192.168.1.1")),
            dst_addr: Some(net_ip_v4!("192.168.1.2")),
            cookie: 1234,
            marks: fnet::Marks {
                mark_1: Some(1111),
                mark_2: None,
                __source_breaking: fidl::marker::SourceBreaking,
            }.into(),
            transport: IpSocketTransportState::Tcp(IpSocketTcpState {
                src_port: Some(1111),
                dst_port: Some(2222),
                state: fnet_tcp::State::Established,
                tcp_info: None,
            }),
        });
        "IpSocketStateV4"
    )]
    #[test_case(
        fnet_sockets::IpSocketState {
            family: Some(fnet::IpVersion::V6),
            src_addr: Some(fidl_ip!("2001:db8::1")),
            dst_addr: Some(fidl_ip!("2001:db8::2")),
            cookie: Some(1234),
            marks: Some(fnet::Marks {
                mark_1: Some(1111),
                mark_2: None,
                __source_breaking: fidl::marker::SourceBreaking,
            }),
            transport: Some(fnet_sockets::IpSocketTransportState::Udp(
                fnet_sockets::IpSocketUdpState {
                    src_port: Some(3333),
                    dst_port: Some(4444),
                    state: Some(fnet_udp::State::Connected),
                    __source_breaking: fidl::marker::SourceBreaking,
                },
            )),
            __source_breaking: fidl::marker::SourceBreaking,
        },
        IpSocketState::V6(IpSocketStateSpecific {
            src_addr: Some(net_ip_v6!("2001:db8::1")),
            dst_addr: Some(net_ip_v6!("2001:db8::2")),
            cookie: 1234,
            marks: fnet::Marks {
                mark_1: Some(1111),
                mark_2: None,
                __source_breaking: fidl::marker::SourceBreaking,
            }.into(),
            transport: IpSocketTransportState::Udp(IpSocketUdpState {
                src_port: Some(3333),
                dst_port: Some(4444),
                state: fnet_udp::State::Connected,
            }),
        });
        "IpSocketStateV6"
    )]
    fn convert_from_fidl_and_back<F, E>(fidl_type: F, local_type: E)
    where
        E: TryFrom<F> + Clone + std::fmt::Debug + PartialEq,
        <E as TryFrom<F>>::Error: std::fmt::Debug + PartialEq,
        F: From<E> + Clone + std::fmt::Debug + PartialEq,
    {
        assert_eq!(fidl_type.clone().try_into(), Ok(local_type.clone()));
        assert_eq!(<_ as Into<F>>::into(local_type), fidl_type);
    }

    #[test_case(
        fnet_sockets::IpSocketMatcher::__SourceBreaking { unknown_ordinal: 100 } =>
            Err(IpSocketMatcherError::UnknownUnionVariant(100));
        "UnknownUnionVariant"
    )]
    #[test_case(
        fnet_sockets::IpSocketMatcher::SrcAddr(fnet_matchers::BoundAddress::Bound(
            fnet_matchers::Address {
                matcher: fnet_matchers::AddressMatcherType::__SourceBreaking { unknown_ordinal: 100 },
                invert: false,
            }
        )) => Err(IpSocketMatcherError::Address(fnet_matchers_ext::BoundAddressError::Address(
            fnet_matchers_ext::AddressError::AddressMatcherType(
                fnet_matchers_ext::AddressMatcherTypeError::UnknownUnionVariant
            )
        )));
        "AddressError"
    )]
    #[test_case(
        fnet_sockets::IpSocketMatcher::Proto(
            fnet_matchers::SocketTransportProtocol::__SourceBreaking { unknown_ordinal: 100 }
        ) => Err(IpSocketMatcherError::TransportProtocol(
            fnet_matchers_ext::SocketTransportProtocolError::UnknownUnionVariant(100)
        ));
        "TransportProtocolError"
    )]
    #[test_case(
        fnet_sockets::IpSocketMatcher::BoundInterface(
            fnet_matchers::BoundInterface::__SourceBreaking { unknown_ordinal: 100 }
        ) => Err(IpSocketMatcherError::BoundInterface(
            fnet_matchers_ext::BoundInterfaceError::UnknownUnionVariant(100)
        ));
        "BoundInterfaceError"
    )]
    #[test_case(
        fnet_sockets::IpSocketMatcher::Mark(fnet_matchers::MarkInDomain {
            domain: fnet::MarkDomain::Mark1,
            mark: fnet_matchers::Mark::__SourceBreaking { unknown_ordinal: 100 },
        }) => Err(IpSocketMatcherError::Mark(
            fnet_matchers_ext::MarkInDomainError::Mark(
                fnet_matchers_ext::MarkError::UnknownUnionVariant(100)
            )
        ));
        "MarkError"
    )]
    fn ip_socket_matcher_try_from_error(
        fidl: fnet_sockets::IpSocketMatcher,
    ) -> Result<IpSocketMatcher, IpSocketMatcherError> {
        IpSocketMatcher::try_from(fidl)
    }

    #[test_case(
        fnet_sockets::IpSocketState {
            family: None,
            src_addr: Some(fidl_ip!("192.168.1.1")),
            dst_addr: Some(fidl_ip!("192.168.1.2")),
            cookie: Some(1234),
            marks: Some(fnet::Marks {
                mark_1: Some(1111),
                mark_2: None,
                __source_breaking: fidl::marker::SourceBreaking,
            }),
            transport: Some(fnet_sockets::IpSocketTransportState::Tcp(
                fnet_sockets::IpSocketTcpState {
                    src_port: Some(1111),
                    dst_port: Some(2222),
                    state: Some(fnet_tcp::State::Established),
                    tcp_info: None,
                    __source_breaking: fidl::marker::SourceBreaking,
                },
            )),
            __source_breaking: fidl::marker::SourceBreaking,
        } => Err(IpSocketStateError::MissingField("family"));
        "MissingFamily"
    )]
    #[test_case(
        fnet_sockets::IpSocketState {
            family: Some(fnet::IpVersion::V4),
            src_addr: Some(fidl_ip!("192.168.1.1")),
            dst_addr: Some(fidl_ip!("192.168.1.2")),
            cookie: None,
            marks: Some(fnet::Marks {
                mark_1: Some(1111),
                mark_2: None,
                __source_breaking: fidl::marker::SourceBreaking,
            }),
            transport: Some(fnet_sockets::IpSocketTransportState::Tcp(
                fnet_sockets::IpSocketTcpState {
                    src_port: Some(1111),
                    dst_port: Some(2222),
                    state: Some(fnet_tcp::State::Established),
                    tcp_info: None,
                    __source_breaking: fidl::marker::SourceBreaking,
                },
            )),
            __source_breaking: fidl::marker::SourceBreaking,
        } => Err(IpSocketStateError::MissingField("cookie"));
        "MissingCookie"
    )]
    #[test_case(
        fnet_sockets::IpSocketState {
            family: Some(fnet::IpVersion::V4),
            src_addr: Some(fidl_ip!("192.168.1.1")),
            dst_addr: Some(fidl_ip!("192.168.1.2")),
            cookie: Some(1234),
            marks: None,
            transport: Some(fnet_sockets::IpSocketTransportState::Tcp(
                fnet_sockets::IpSocketTcpState {
                    src_port: Some(1111),
                    dst_port: Some(2222),
                    state: Some(fnet_tcp::State::Established),
                    tcp_info: None,
                    __source_breaking: fidl::marker::SourceBreaking,
                },
            )),
            __source_breaking: fidl::marker::SourceBreaking,
        } => Err(IpSocketStateError::MissingField("marks"));
        "MissingMarks"
    )]
    #[test_case(
        fnet_sockets::IpSocketState {
            family: Some(fnet::IpVersion::V4),
            src_addr: Some(fidl_ip!("192.168.1.1")),
            dst_addr: Some(fidl_ip!("192.168.1.2")),
            cookie: Some(1234),
            marks: Some(fnet::Marks {
                mark_1: Some(1111),
                mark_2: None,
                __source_breaking: fidl::marker::SourceBreaking,
            }),
            transport: None,
            __source_breaking: fidl::marker::SourceBreaking,
        } => Err(IpSocketStateError::MissingField("transport"));
        "MissingTransport"
    )]
    #[test_case(
        fnet_sockets::IpSocketState {
            family: Some(fnet::IpVersion::V4),
            src_addr: Some(fidl_ip!("192.168.1.1")),
            dst_addr: Some(fidl_ip!("2001:db8::2")),
            cookie: Some(1234),
            marks: Some(fnet::Marks {
                mark_1: Some(1111),
                mark_2: None,
                __source_breaking: fidl::marker::SourceBreaking,
            }),
            transport: Some(fnet_sockets::IpSocketTransportState::Tcp(
                fnet_sockets::IpSocketTcpState {
                    src_port: Some(1111),
                    dst_port: Some(2222),
                    state: Some(fnet_tcp::State::Established),
                    tcp_info: None,
                    __source_breaking: fidl::marker::SourceBreaking,
                },
            )),
            __source_breaking: fidl::marker::SourceBreaking,
        } => Err(IpSocketStateError::VersionMismatch);
        "VersionMismatchV4"
    )]
    #[test_case(
        fnet_sockets::IpSocketState {
            family: Some(fnet::IpVersion::V6),
            src_addr: Some(fidl_ip!("192.168.1.1")),
            dst_addr: Some(fidl_ip!("2001:db8::2")),
            cookie: Some(1234),
            marks: Some(fnet::Marks {
                mark_1: Some(1111),
                mark_2: None,
                __source_breaking: fidl::marker::SourceBreaking,
            }),
            transport: Some(fnet_sockets::IpSocketTransportState::Tcp(
                fnet_sockets::IpSocketTcpState {
                    src_port: Some(1111),
                    dst_port: Some(2222),
                    state: Some(fnet_tcp::State::Established),
                    tcp_info: None,
                    __source_breaking: fidl::marker::SourceBreaking,
                },
            )),
            __source_breaking: fidl::marker::SourceBreaking,
        } => Err(IpSocketStateError::VersionMismatch);
        "VersionMismatchV6"
    )]
    #[test_case(
        fnet_sockets::IpSocketState {
            family: Some(fnet::IpVersion::V4),
            src_addr: Some(fidl_ip!("192.168.1.1")),
            dst_addr: Some(fidl_ip!("192.168.1.2")),
            cookie: Some(1234),
            marks: Some(fnet::Marks {
                mark_1: Some(1111),
                mark_2: None,
                __source_breaking: fidl::marker::SourceBreaking,
            }),
            transport: Some(fnet_sockets::IpSocketTransportState::Tcp(
                fnet_sockets::IpSocketTcpState {
                    src_port: Some(1111),
                    dst_port: Some(2222),
                    state: None,
                    tcp_info: None,
                    __source_breaking: fidl::marker::SourceBreaking,
                },
            )),
            __source_breaking: fidl::marker::SourceBreaking,
        } => Err(IpSocketStateError::Transport(IpSocketTransportStateError::Tcp(
                IpSocketTcpStateError::MissingField("state"),
        )));
        "MissingTcpState"
    )]
    #[test_case(
        fnet_sockets::IpSocketState {
            family: Some(fnet::IpVersion::V6),
            src_addr: Some(fidl_ip!("2001:db8::1")),
            dst_addr: Some(fidl_ip!("2001:db8::2")),
            cookie: Some(1234),
            marks: Some(fnet::Marks {
                mark_1: Some(1111),
                mark_2: None,
                __source_breaking: fidl::marker::SourceBreaking,
            }),
            transport: Some(fnet_sockets::IpSocketTransportState::Udp(
                fnet_sockets::IpSocketUdpState {
                    src_port: Some(3333),
                    dst_port: Some(4444),
                    state: None,
                    __source_breaking: fidl::marker::SourceBreaking,
                },
            )),
            __source_breaking: fidl::marker::SourceBreaking,
        } => Err(IpSocketStateError::Transport(IpSocketTransportStateError::Udp(
                IpSocketUdpStateError::MissingField("state"),
        )));
        "MissingUdpState"
    )]
    fn ip_socket_state_try_from_error(
        fidl: fnet_sockets::IpSocketState,
    ) -> Result<IpSocketState, IpSocketStateError> {
        IpSocketState::try_from(fidl)
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn iterate_ip_diagnostics_iterate_ip_error() {
        async fn serve_matcher_error(req: fnet_sockets::DiagnosticsRequest) {
            match req {
                fnet_sockets::DiagnosticsRequest::IterateIp {
                    s: _,
                    extensions: _,
                    matchers: _,
                    responder,
                } => responder
                    .send(&fnet_sockets::IterateIpResult::InvalidMatcher(
                        fnet_sockets::InvalidMatcher { index: 0 },
                    ))
                    .unwrap(),
                fnet_sockets::DiagnosticsRequest::GetDestructionWatcher { .. } => unreachable!(),
            };
        }

        let (diagnostics, diagnostics_server_end) =
            fidl::endpoints::create_proxy::<fnet_sockets::DiagnosticsMarker>();

        let (mut diagnostics_request_stream, _control_handle) =
            diagnostics_server_end.into_stream_and_control_handle();
        let server_fut = diagnostics_request_stream
            .next()
            .then(|req| {
                serve_matcher_error(req.expect("Request stream ended unexpectedly").unwrap())
            })
            .fuse();
        let client_fut = iterate_ip::<[IpSocketMatcher; 0], _>(
            &diagnostics,
            fnet_sockets::Extensions::empty(),
            [],
        );

        pin_mut!(server_fut);
        pin_mut!(client_fut);

        let ((), resp) = future::join(server_fut, client_fut).await;

        assert_matches!(
            // Discard the stream because it can't be formatted.
            resp.map(|_| ()),
            Err(IterateIpError::InvalidMatcher(0))
        );
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn iterate_ip_next_error() {
        async fn serve_matcher(req: fnet_sockets::DiagnosticsRequest) {
            match req {
                fnet_sockets::DiagnosticsRequest::IterateIp {
                    s,
                    extensions: _,
                    matchers: _,
                    responder,
                } => {
                    s.close_with_epitaph(zx_status::Status::PEER_CLOSED).unwrap();
                    responder.send(&fnet_sockets::IterateIpResult::Ok(fnet_sockets::Empty)).unwrap()
                }
                fnet_sockets::DiagnosticsRequest::GetDestructionWatcher { .. } => unreachable!(),
            }
        }

        let (diagnostics, diagnostics_server_end) =
            fidl::endpoints::create_proxy::<fnet_sockets::DiagnosticsMarker>();

        let (mut diagnostics_request_stream, _control_handle) =
            diagnostics_server_end.into_stream_and_control_handle();
        let server_fut = diagnostics_request_stream
            .next()
            .then(|req| serve_matcher(req.expect("Request stream ended unexpectedly").unwrap()))
            .fuse();

        let client_fut = iterate_ip::<[IpSocketMatcher; 0], _>(
            &diagnostics,
            fnet_sockets::Extensions::empty(),
            [],
        );

        let ((), resp) = future::join(server_fut, client_fut).await;
        let stream = resp.unwrap();
        pin_mut!(stream);

        assert_matches!(
            stream.try_next().await,
            Err(IpIteratorError::Fidl(fidl::Error::ClientChannelClosed { .. }))
        );
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn iterate_ip_empty_batch() {
        async fn serve_matcher(req: fnet_sockets::DiagnosticsRequest) {
            match req {
                fnet_sockets::DiagnosticsRequest::IterateIp {
                    s,
                    extensions: _,
                    matchers: _,
                    responder,
                } => {
                    responder
                        .send(&fnet_sockets::IterateIpResult::Ok(fnet_sockets::Empty))
                        .unwrap();

                    let (mut stream, _control) = s.into_stream_and_control_handle();
                    match stream.next().await.unwrap().unwrap() {
                        fidl_fuchsia_net_sockets::IpIteratorRequest::Next { responder } => {
                            // Send an empty batch but indicate there's more to come.
                            responder.send(&[], true).unwrap();
                        }
                        fidl_fuchsia_net_sockets::IpIteratorRequest::_UnknownMethod { .. } => {
                            unreachable!()
                        }
                    }
                }
                fnet_sockets::DiagnosticsRequest::GetDestructionWatcher { .. } => unreachable!(),
            }
        }

        let (diagnostics, diagnostics_server_end) =
            fidl::endpoints::create_proxy::<fnet_sockets::DiagnosticsMarker>();

        let (mut diagnostics_request_stream, _control_handle) =
            diagnostics_server_end.into_stream_and_control_handle();
        let server_fut = diagnostics_request_stream
            .next()
            .then(|req| serve_matcher(req.expect("Request stream ended unexpectedly").unwrap()))
            .fuse();

        let client_fut = async {
            let stream = iterate_ip::<[IpSocketMatcher; 0], _>(
                &diagnostics,
                fnet_sockets::Extensions::empty(),
                [],
            )
            .await
            .unwrap();
            pin_mut!(stream);
            stream.try_next().await
        };

        let ((), resp) = future::join(server_fut, client_fut).await;
        assert_matches!(resp, Err(IpIteratorError::EmptyBatch));
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn iterate_ip_success() {
        let socket_1 = fnet_sockets::IpSocketState {
            family: Some(fnet::IpVersion::V4),
            src_addr: Some(fidl_ip!("192.168.1.1")),
            dst_addr: Some(fidl_ip!("192.168.1.2")),
            cookie: Some(1234),
            marks: Some(fnet::Marks {
                mark_1: Some(1111),
                mark_2: None,
                __source_breaking: fidl::marker::SourceBreaking,
            }),
            transport: Some(fnet_sockets::IpSocketTransportState::Tcp(
                fnet_sockets::IpSocketTcpState {
                    src_port: Some(1111),
                    dst_port: Some(2222),
                    state: Some(fnet_tcp::State::Established),
                    tcp_info: None,
                    __source_breaking: fidl::marker::SourceBreaking,
                },
            )),
            __source_breaking: fidl::marker::SourceBreaking,
        };

        let socket_2 = fnet_sockets::IpSocketState {
            family: Some(fnet::IpVersion::V4),
            src_addr: Some(fidl_ip!("192.168.8.1")),
            dst_addr: Some(fidl_ip!("192.168.8.2")),
            cookie: Some(9876),
            marks: Some(fnet::Marks {
                mark_1: None,
                mark_2: Some(2222),
                __source_breaking: fidl::marker::SourceBreaking,
            }),
            transport: Some(fnet_sockets::IpSocketTransportState::Tcp(
                fnet_sockets::IpSocketTcpState {
                    src_port: Some(3333),
                    dst_port: Some(4444),
                    state: Some(fnet_tcp::State::TimeWait),
                    tcp_info: None,
                    __source_breaking: fidl::marker::SourceBreaking,
                },
            )),
            __source_breaking: fidl::marker::SourceBreaking,
        };

        let socket_3 = fnet_sockets::IpSocketState {
            family: Some(fnet::IpVersion::V6),
            src_addr: Some(fidl_ip!("2001:db8::1")),
            dst_addr: Some(fidl_ip!("2001:db8::2")),
            cookie: Some(5678),
            marks: Some(fnet::Marks {
                mark_1: None,
                mark_2: None,
                __source_breaking: fidl::marker::SourceBreaking,
            }),
            transport: Some(fnet_sockets::IpSocketTransportState::Tcp(
                fnet_sockets::IpSocketTcpState {
                    src_port: Some(5555),
                    dst_port: Some(6666),
                    state: Some(fnet_tcp::State::TimeWait),
                    tcp_info: None,
                    __source_breaking: fidl::marker::SourceBreaking,
                },
            )),
            __source_breaking: fidl::marker::SourceBreaking,
        };

        let (diagnostics, diagnostics_server_end) =
            fidl::endpoints::create_proxy::<fnet_sockets::DiagnosticsMarker>();

        let serve_matcher = async |req: fnet_sockets::DiagnosticsRequest| {
            let responses = &[vec![socket_1.clone()], vec![socket_2.clone(), socket_3.clone()]];

            match req {
                fnet_sockets::DiagnosticsRequest::IterateIp {
                    s,
                    extensions: _,
                    matchers: _,
                    responder,
                } => {
                    responder
                        .send(&fnet_sockets::IterateIpResult::Ok(fnet_sockets::Empty))
                        .unwrap();

                    let (mut stream, _control) = s.into_stream_and_control_handle();
                    for (i, resp) in responses.iter().enumerate() {
                        match stream.next().await.unwrap().unwrap() {
                            fidl_fuchsia_net_sockets::IpIteratorRequest::Next { responder } => {
                                let has_more = i < responses.len() - 1;
                                responder.send(&resp, has_more).unwrap();
                            }
                            fidl_fuchsia_net_sockets::IpIteratorRequest::_UnknownMethod {
                                ..
                            } => {
                                unreachable!()
                            }
                        }
                    }
                }
                fnet_sockets::DiagnosticsRequest::GetDestructionWatcher { .. } => unreachable!(),
            };
        };

        let (mut diagnostics_request_stream, _control_handle) =
            diagnostics_server_end.into_stream_and_control_handle();

        let server_fut = diagnostics_request_stream
            .next()
            .then(|req| serve_matcher(req.expect("Request stream ended unexpectedly").unwrap()))
            .fuse();

        let client_fut = async {
            iterate_ip::<[IpSocketMatcher; 0], _>(
                &diagnostics,
                fnet_sockets::Extensions::empty(),
                [],
            )
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap()
        };

        let ((), sockets) = future::join(server_fut, client_fut).await;
        assert_eq!(
            sockets,
            vec![
                socket_1.clone().try_into().unwrap(),
                socket_2.clone().try_into().unwrap(),
                socket_3.clone().try_into().unwrap()
            ]
        );
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn watch_destruction_success() {
        let socket_1 = fnet_sockets::IpSocketState {
            family: Some(fnet::IpVersion::V4),
            src_addr: Some(fidl_ip!("192.168.1.1")),
            dst_addr: Some(fidl_ip!("192.168.1.2")),
            cookie: Some(1234),
            marks: Some(fnet::Marks {
                mark_1: Some(1111),
                mark_2: None,
                __source_breaking: fidl::marker::SourceBreaking,
            }),
            transport: Some(fnet_sockets::IpSocketTransportState::Tcp(
                fnet_sockets::IpSocketTcpState {
                    src_port: Some(1111),
                    dst_port: Some(2222),
                    state: Some(fnet_tcp::State::Established),
                    tcp_info: None,
                    __source_breaking: fidl::marker::SourceBreaking,
                },
            )),
            __source_breaking: fidl::marker::SourceBreaking,
        };

        let socket_2 = fnet_sockets::IpSocketState {
            family: Some(fnet::IpVersion::V4),
            src_addr: Some(fidl_ip!("192.168.8.1")),
            dst_addr: Some(fidl_ip!("192.168.8.2")),
            cookie: Some(9876),
            marks: Some(fnet::Marks {
                mark_1: None,
                mark_2: Some(2222),
                __source_breaking: fidl::marker::SourceBreaking,
            }),
            transport: Some(fnet_sockets::IpSocketTransportState::Tcp(
                fnet_sockets::IpSocketTcpState {
                    src_port: Some(3333),
                    dst_port: Some(4444),
                    state: Some(fnet_tcp::State::TimeWait),
                    tcp_info: None,
                    __source_breaking: fidl::marker::SourceBreaking,
                },
            )),
            __source_breaking: fidl::marker::SourceBreaking,
        };

        let (diagnostics, diagnostics_server_end) =
            fidl::endpoints::create_proxy::<fnet_sockets::DiagnosticsMarker>();
        let serve_watcher = async |req: fnet_sockets::DiagnosticsRequest| match req {
            fnet_sockets::DiagnosticsRequest::GetDestructionWatcher { watcher, responder } => {
                responder.send().unwrap();
                let mut stream = watcher.into_stream();
                let batches = [
                    Some(vec![socket_1.clone()]),
                    Some(vec![socket_2.clone(), socket_1.clone()]),
                    None,
                ];
                for batch in batches {
                    let req = stream.next().await.unwrap().unwrap();
                    let responder = match req {
                        fnet_sockets::DestructionWatcherRequest::Watch { responder } => responder,
                        fnet_sockets::DestructionWatcherRequest::_UnknownMethod { .. } => {
                            unreachable!()
                        }
                    };
                    if let Some(batch) = batch {
                        responder.send(&batch, 0).unwrap();
                    } else {
                        drop(responder);
                    }
                }
            }
            fnet_sockets::DiagnosticsRequest::IterateIp { .. } => unreachable!(),
        };

        let (mut diagnostics_request_stream, _control_handle) =
            diagnostics_server_end.into_stream_and_control_handle();
        let server_fut = diagnostics_request_stream
            .next()
            .then(|req| serve_watcher(req.expect("Request stream ended unexpectedly").unwrap()))
            .fuse();

        let expected_socket_1: IpSocketState = socket_1.clone().try_into().unwrap();
        let expected_socket_2: IpSocketState = socket_2.clone().try_into().unwrap();

        let client_fut = async {
            let stream = watch_destruction(&diagnostics).await.unwrap();
            pin_mut!(stream);

            assert_matches!(
                stream.next().await,
                Some(Ok(sock)) => assert_eq!(sock, expected_socket_1)
            );
            assert_matches!(
                stream.next().await,
                Some(Ok(sock)) => assert_eq!(sock, expected_socket_2)
            );
            assert_matches!(
                stream.next().await,
                Some(Ok(sock)) => assert_eq!(sock, expected_socket_1)
            );
            assert_matches!(stream.next().await, Some(Err(DestructionWatcherError::Fidl(_))));
        };

        let _: ((), ()) = future::join(server_fut, client_fut).await;
    }

    #[test_case(
        None,
        DestructionWatcherError::EmptyBatch;
        "empty_batch"
    )]
    #[test_case(
        Some(fnet_sockets::IpSocketState {
            family: None,
            src_addr: Some(fidl_ip!("192.168.1.1")),
            dst_addr: Some(fidl_ip!("192.168.1.2")),
            cookie: Some(1234),
            marks: Some(fnet::Marks {
                mark_1: Some(1111),
                mark_2: None,
                __source_breaking: fidl::marker::SourceBreaking,
            }),
            transport: Some(fnet_sockets::IpSocketTransportState::Tcp(
                fnet_sockets::IpSocketTcpState {
                    src_port: Some(1111),
                    dst_port: Some(2222),
                    state: Some(fnet_tcp::State::Established),
                    tcp_info: None,
                    __source_breaking: fidl::marker::SourceBreaking,
                },
            )),
            __source_breaking: fidl::marker::SourceBreaking,
        }),
        DestructionWatcherError::Conversion(IpSocketStateError::MissingField("family"));
        "conversion_error"
    )]
    #[fuchsia_async::run_singlethreaded(test)]
    async fn watch_destruction_error(
        socket: Option<fnet_sockets::IpSocketState>,
        expected_error: DestructionWatcherError,
    ) {
        let (diagnostics, diagnostics_server_end) =
            fidl::endpoints::create_proxy::<fnet_sockets::DiagnosticsMarker>();
        let serve_watcher = async |req: fnet_sockets::DiagnosticsRequest| match req {
            fnet_sockets::DiagnosticsRequest::GetDestructionWatcher { watcher, responder } => {
                responder.send().unwrap();
                let mut stream = watcher.into_stream();
                let req = stream.next().await.unwrap().unwrap();
                let responder = match req {
                    fnet_sockets::DestructionWatcherRequest::Watch { responder } => responder,
                    fnet_sockets::DestructionWatcherRequest::_UnknownMethod { .. } => {
                        unreachable!()
                    }
                };
                let batch = match socket {
                    None => vec![],
                    Some(s) => vec![s],
                };
                responder.send(&batch, 0).unwrap();
            }
            fnet_sockets::DiagnosticsRequest::IterateIp { .. } => unreachable!(),
        };

        let (mut diagnostics_request_stream, _control_handle) =
            diagnostics_server_end.into_stream_and_control_handle();
        let server_fut = diagnostics_request_stream
            .next()
            .then(|req| serve_watcher(req.expect("Request stream ended unexpectedly").unwrap()))
            .fuse();

        let client_fut = async {
            let stream = watch_destruction(&diagnostics).await.unwrap();
            pin_mut!(stream);

            let result = stream.next().await.expect("got a result");
            result
        };

        let ((), result) = future::join(server_fut, client_fut).await;
        match expected_error {
            DestructionWatcherError::EmptyBatch => {
                assert_matches!(result, Err(DestructionWatcherError::EmptyBatch));
            }
            DestructionWatcherError::Conversion(b) => {
                assert_matches!(result, Err(DestructionWatcherError::Conversion(a)) if a == b);
            }
            DestructionWatcherError::Fidl(_) | DestructionWatcherError::DroppedEvents(_) => {
                unreachable!()
            }
        }
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn watch_destruction_dropped_events() {
        let socket = fnet_sockets::IpSocketState {
            family: Some(fnet::IpVersion::V4),
            src_addr: Some(fidl_ip!("192.168.1.1")),
            dst_addr: Some(fidl_ip!("192.168.1.2")),
            cookie: Some(1234),
            marks: Some(fnet::Marks {
                mark_1: Some(1111),
                mark_2: None,
                __source_breaking: fidl::marker::SourceBreaking,
            }),
            transport: Some(fnet_sockets::IpSocketTransportState::Tcp(
                fnet_sockets::IpSocketTcpState {
                    src_port: Some(1111),
                    dst_port: Some(2222),
                    state: Some(fnet_tcp::State::Established),
                    tcp_info: None,
                    __source_breaking: fidl::marker::SourceBreaking,
                },
            )),
            __source_breaking: fidl::marker::SourceBreaking,
        };

        let (diagnostics, diagnostics_server_end) =
            fidl::endpoints::create_proxy::<fnet_sockets::DiagnosticsMarker>();
        let serve_watcher = {
            let socket = socket.clone();
            async |req: fnet_sockets::DiagnosticsRequest| match req {
                fnet_sockets::DiagnosticsRequest::GetDestructionWatcher { watcher, responder } => {
                    responder.send().unwrap();
                    let mut stream = watcher.into_stream();
                    let req = stream.next().await.unwrap().unwrap();
                    let responder = match req {
                        fnet_sockets::DestructionWatcherRequest::Watch { responder } => responder,
                        fnet_sockets::DestructionWatcherRequest::_UnknownMethod { .. } => {
                            unreachable!()
                        }
                    };
                    responder.send(&[socket], 100).unwrap();
                }
                fnet_sockets::DiagnosticsRequest::IterateIp { .. } => unreachable!(),
            }
        };

        let (mut diagnostics_request_stream, _control_handle) =
            diagnostics_server_end.into_stream_and_control_handle();
        let server_fut = diagnostics_request_stream
            .next()
            .then(|req| serve_watcher(req.expect("Request stream ended unexpectedly").unwrap()))
            .fuse();

        let expected_socket: IpSocketState = socket.try_into().unwrap();

        let client_fut = async {
            let stream = watch_destruction(&diagnostics).await.unwrap();
            pin_mut!(stream);

            assert_matches!(
                stream.next().await,
                Some(Ok(sock)) => assert_eq!(sock, expected_socket)
            );
            assert_matches!(
                stream.next().await,
                Some(Err(DestructionWatcherError::DroppedEvents(100)))
            );
        };

        let _: ((), ()) = future::join(server_fut, client_fut).await;
    }
}
