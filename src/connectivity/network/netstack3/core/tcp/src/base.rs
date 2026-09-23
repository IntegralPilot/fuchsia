// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The Transmission Control Protocol (TCP).

use core::num::NonZeroU8;
use core::time::Duration;

use derivative::Derivative;
use net_types::SpecifiedAddr;
use net_types::ip::{GenericOverIp, Ip, Ipv4, Ipv6, Mtu};
use netstack3_base::{
    ChecksumOffloadResult, IcmpErrorCode, Icmpv4ErrorCode, Icmpv6ErrorCode, IpExt, Marks, Mms,
    UnscaledWindowSize, WeakDeviceIdentifier, WindowSize,
};
use netstack3_ip::socket::{RouteResolutionOptions, SendOptions};
use packet_formats::icmp::{
    Icmpv4DestUnreachableCode, Icmpv4TimeExceededCode, Icmpv6DestUnreachableCode,
};
use packet_formats::ip::DscpAndEcn;
use packet_formats::utils::NonZeroDuration;
use rand::Rng;
use thiserror::Error;

use crate::internal::buffer::BufferLimits;
use crate::internal::counters::{TcpCountersWithSocket, TcpCountersWithoutSocket};
use crate::internal::socket::generators::{IsnGenerator, TimestampOffsetGenerator};
use crate::internal::socket::{DualStackIpExt, Sockets, TcpBindingsTypes, WeakTcpSocketId};
use crate::internal::state::DEFAULT_MAX_SYN_RETRIES;

/// Default lifetime for a orphaned connection in FIN_WAIT2.
pub const DEFAULT_FIN_WAIT2_TIMEOUT: Duration = Duration::from_secs(60);

/// Errors surfaced to the user.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Error)]
pub enum ConnectionError {
    /// The connection was refused, RST segment received while in SYN_SENT state.
    #[error("connection refused (RST segment received while in SYN_SENT state")]
    ConnectionRefused,
    /// The connection was reset because of a RST segment.
    #[error("connection was reset because of a RST segment")]
    ConnectionReset,
    /// The connection was closed because the network is unreachable.
    #[error("connection was closed because the network is unreachable")]
    NetworkUnreachable,
    /// The connection was closed because the host is unreachable.
    #[error("connection was closed because the host is unreachable")]
    HostUnreachable,
    /// The connection was closed because the protocol is unreachable.
    #[error("connection was closed because the protocol is unreachable")]
    ProtocolUnreachable,
    /// The connection was closed because the port is unreachable.
    #[error("connection was closed because the port is unreachable")]
    PortUnreachable,
    /// The connection was closed because the host is down.
    #[error("connection was closed because the host is down")]
    DestinationHostDown,
    /// The connection was closed because the source route failed.
    #[error("connection was closed because the source route failed")]
    SourceRouteFailed,
    /// The connection was closed because the source host is isolated.
    #[error("connection was closed because the source host is isolated")]
    SourceHostIsolated,
    /// The connection was closed because of a time out.
    #[error("connection was closed because of a time out")]
    TimedOut,
    /// The connection was closed because of a lack of required permissions.
    #[error("connection was closed because of a lack of required permissions")]
    PermissionDenied,
    /// The connection was closed because there was a protocol error.
    #[error("connection was closed because there was a protocol error")]
    ProtocolError,
    /// The connection was aborted by the system.
    #[error("connection was aborted by the system")]
    Aborted,
}

/// The meaning of a particular ICMP error to a TCP socket.
pub(crate) enum IcmpErrorResult {
    /// There has been an error on the connection that must be handled.
    ConnectionError(ConnectionError),
    /// The PMTU used by the connection has been updated.
    PmtuUpdate(Mms),
}

impl IcmpErrorResult {
    // Notes: the following mappings are guided by the packetimpact test here:
    // https://cs.opensource.google/gvisor/gvisor/+/master:test/packetimpact/tests/tcp_network_unreachable_test.go;drc=611e6e1247a0691f5fd198f411c68b3bc79d90af
    pub(crate) fn try_from_icmp_error(err: IcmpErrorCode) -> Option<IcmpErrorResult> {
        match err {
            IcmpErrorCode::V4(Icmpv4ErrorCode::DestUnreachable(code, message)) => {
                match code {
                    Icmpv4DestUnreachableCode::DestNetworkUnreachable => {
                        Some(IcmpErrorResult::ConnectionError(ConnectionError::NetworkUnreachable))
                    }
                    Icmpv4DestUnreachableCode::DestHostUnreachable => {
                        Some(IcmpErrorResult::ConnectionError(ConnectionError::HostUnreachable))
                    }
                    Icmpv4DestUnreachableCode::DestProtocolUnreachable => {
                        Some(IcmpErrorResult::ConnectionError(ConnectionError::ProtocolUnreachable))
                    }
                    Icmpv4DestUnreachableCode::DestPortUnreachable => {
                        Some(IcmpErrorResult::ConnectionError(ConnectionError::PortUnreachable))
                    }
                    Icmpv4DestUnreachableCode::SourceRouteFailed => {
                        Some(IcmpErrorResult::ConnectionError(ConnectionError::SourceRouteFailed))
                    }
                    Icmpv4DestUnreachableCode::DestNetworkUnknown => {
                        Some(IcmpErrorResult::ConnectionError(ConnectionError::NetworkUnreachable))
                    }
                    Icmpv4DestUnreachableCode::DestHostUnknown => {
                        Some(IcmpErrorResult::ConnectionError(ConnectionError::DestinationHostDown))
                    }
                    Icmpv4DestUnreachableCode::SourceHostIsolated => {
                        Some(IcmpErrorResult::ConnectionError(ConnectionError::SourceHostIsolated))
                    }
                    Icmpv4DestUnreachableCode::NetworkAdministrativelyProhibited => {
                        Some(IcmpErrorResult::ConnectionError(ConnectionError::NetworkUnreachable))
                    }
                    Icmpv4DestUnreachableCode::HostAdministrativelyProhibited => {
                        Some(IcmpErrorResult::ConnectionError(ConnectionError::HostUnreachable))
                    }
                    Icmpv4DestUnreachableCode::NetworkUnreachableForToS => {
                        Some(IcmpErrorResult::ConnectionError(ConnectionError::NetworkUnreachable))
                    }
                    Icmpv4DestUnreachableCode::HostUnreachableForToS => {
                        Some(IcmpErrorResult::ConnectionError(ConnectionError::HostUnreachable))
                    }
                    Icmpv4DestUnreachableCode::CommAdministrativelyProhibited => {
                        Some(IcmpErrorResult::ConnectionError(ConnectionError::HostUnreachable))
                    }
                    Icmpv4DestUnreachableCode::HostPrecedenceViolation => {
                        Some(IcmpErrorResult::ConnectionError(ConnectionError::HostUnreachable))
                    }
                    Icmpv4DestUnreachableCode::PrecedenceCutoffInEffect => {
                        Some(IcmpErrorResult::ConnectionError(ConnectionError::HostUnreachable))
                    }
                    Icmpv4DestUnreachableCode::FragmentationRequired => {
                        let mtu = message.next_hop_mtu().expect("stack should always fill in MTU");
                        let mtu = Mtu::new(mtu.get().into());
                        let mms = Mms::from_mtu::<Ipv4>(mtu, 0 /* no IP options used */)?;
                        Some(IcmpErrorResult::PmtuUpdate(mms))
                    }
                }
            }
            IcmpErrorCode::V4(Icmpv4ErrorCode::ParameterProblem(_)) => {
                Some(IcmpErrorResult::ConnectionError(ConnectionError::ProtocolError))
            }
            IcmpErrorCode::V4(Icmpv4ErrorCode::TimeExceeded(
                Icmpv4TimeExceededCode::TtlExpired,
            )) => Some(IcmpErrorResult::ConnectionError(ConnectionError::HostUnreachable)),
            IcmpErrorCode::V4(Icmpv4ErrorCode::TimeExceeded(
                Icmpv4TimeExceededCode::FragmentReassemblyTimeExceeded,
            )) => Some(IcmpErrorResult::ConnectionError(ConnectionError::TimedOut)),
            IcmpErrorCode::V4(Icmpv4ErrorCode::Redirect(_)) => None,
            IcmpErrorCode::V6(Icmpv6ErrorCode::DestUnreachable(code)) => {
                Some(IcmpErrorResult::ConnectionError(match code {
                    Icmpv6DestUnreachableCode::NoRoute => ConnectionError::NetworkUnreachable,
                    Icmpv6DestUnreachableCode::CommAdministrativelyProhibited => {
                        ConnectionError::PermissionDenied
                    }
                    Icmpv6DestUnreachableCode::BeyondScope => ConnectionError::HostUnreachable,
                    Icmpv6DestUnreachableCode::AddrUnreachable => ConnectionError::HostUnreachable,
                    Icmpv6DestUnreachableCode::PortUnreachable => ConnectionError::PortUnreachable,
                    Icmpv6DestUnreachableCode::SrcAddrFailedPolicy => {
                        ConnectionError::PermissionDenied
                    }
                    Icmpv6DestUnreachableCode::RejectRoute => ConnectionError::PermissionDenied,
                }))
            }
            IcmpErrorCode::V6(Icmpv6ErrorCode::ParameterProblem(_)) => {
                Some(IcmpErrorResult::ConnectionError(ConnectionError::ProtocolError))
            }
            IcmpErrorCode::V6(Icmpv6ErrorCode::TimeExceeded(_)) => {
                Some(IcmpErrorResult::ConnectionError(ConnectionError::HostUnreachable))
            }
            IcmpErrorCode::V6(Icmpv6ErrorCode::PacketTooBig(mtu)) => {
                let mms = Mms::from_mtu::<Ipv6>(mtu, 0 /* no IP options used */)?;
                Some(IcmpErrorResult::PmtuUpdate(mms))
            }
        }
    }
}

/// Metadata associated with an outgoing TCP packet.
#[derive(Derivative, GenericOverIp)]
#[generic_over_ip(I, Ip)]
#[derivative(Debug(bound = ""))]
#[cfg_attr(any(test, feature = "testutils"), derivative(PartialEq(bound = "")))]
pub struct TcpSocketTxMetadata<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes> {
    /// The socket from which the packet originates.
    socket: WeakTcpSocketId<I, D, BT>,
    checksum_offload_result: Option<ChecksumOffloadResult>,
}

impl<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes>
    TcpSocketTxMetadata<I, D, BT>
{
    /// Creates a new `TcpSocketTxMetadata`.
    pub(crate) fn new(socket: WeakTcpSocketId<I, D, BT>) -> Self {
        Self { socket, checksum_offload_result: None }
    }

    /// Gets the socket from which the packet originates.
    pub fn socket(&self) -> &WeakTcpSocketId<I, D, BT> {
        &self.socket
    }

    /// Returns the checksum offload result.
    pub fn checksum_offload_result(&self) -> Option<ChecksumOffloadResult> {
        self.checksum_offload_result.clone()
    }

    /// Sets the checksum offload result.
    pub fn set_checksum_offload_result(&mut self, result: Option<ChecksumOffloadResult>) {
        self.checksum_offload_result = result;
    }
}

/// Stack wide state supporting TCP.
#[derive(GenericOverIp)]
#[generic_over_ip(I, Ip)]
pub struct TcpState<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes> {
    /// The initial sequence number generator.
    pub isn_generator: IsnGenerator<BT::Instant>,
    /// The timestamp offset generator.
    pub timestamp_offset_generator: TimestampOffsetGenerator<BT::Instant>,
    /// TCP sockets state.
    pub sockets: Sockets<I, D, BT>,
    /// TCP counters that cannot be attributed to a specific socket.
    pub counters_without_socket: TcpCountersWithoutSocket<I>,
    /// TCP counters that can be attributed to a specific socket.
    pub counters_with_socket: TcpCountersWithSocket<I>,
}

impl<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes> TcpState<I, D, BT> {
    /// Creates a new TCP stack state.
    pub fn new(now: BT::Instant, rng: &mut impl Rng) -> Self {
        Self {
            isn_generator: IsnGenerator::new(now, rng),
            timestamp_offset_generator: TimestampOffsetGenerator::new(now, rng),
            sockets: Sockets::new(),
            counters_without_socket: Default::default(),
            counters_with_socket: Default::default(),
        }
    }
}

/// Named tuple for holding sizes of buffers for a socket.
#[derive(Copy, Clone, Debug)]
#[cfg_attr(test, derive(Eq, PartialEq))]
pub struct BufferSizes {
    /// The size of the send buffer.
    pub send: usize,
    /// The size of the receive buffer.
    pub receive: usize,
}
/// Sensible defaults only for testing.
#[cfg(any(test, feature = "testutils"))]
impl Default for BufferSizes {
    fn default() -> Self {
        BufferSizes { send: WindowSize::DEFAULT.into(), receive: WindowSize::DEFAULT.into() }
    }
}

impl BufferSizes {
    pub(crate) fn rcv_limits(&self) -> BufferLimits {
        let Self { send: _, receive } = self;
        BufferLimits { capacity: *receive, len: 0 }
    }

    pub(crate) fn rwnd(&self) -> WindowSize {
        let Self { send: _, receive } = *self;
        WindowSize::new(receive).unwrap_or(WindowSize::MAX)
    }

    pub(crate) fn rwnd_unscaled(&self) -> UnscaledWindowSize {
        let Self { send: _, receive } = *self;
        UnscaledWindowSize::from(u16::try_from(receive).unwrap_or(u16::MAX))
    }
}

/// A mutable reference to buffer configuration.
pub(crate) enum BuffersRefMut<'a, R, S> {
    /// All buffers are dropped.
    NoBuffers,
    /// Buffer sizes are configured but not instantiated yet.
    Sizes(&'a mut BufferSizes),
    /// Buffers are instantiated and mutable references are provided.
    Both { send: &'a mut S, recv: &'a mut R },
    /// Only the send buffer is still instantiated, which happens in Closing
    /// states.
    SendOnly(&'a mut S),
    /// Only the receive buffer is still instantiated, which happens in Finwait
    /// states.
    RecvOnly(&'a mut R),
}

impl<'a, R, S> BuffersRefMut<'a, R, S> {
    pub(crate) fn into_send_buffer(self) -> Option<&'a mut S> {
        match self {
            Self::NoBuffers | Self::Sizes(_) | Self::RecvOnly(_) => None,
            Self::Both { send, recv: _ } | Self::SendOnly(send) => Some(send),
        }
    }

    pub(crate) fn into_receive_buffer(self) -> Option<&'a mut R> {
        match self {
            Self::NoBuffers | Self::Sizes(_) | Self::SendOnly(_) => None,
            Self::Both { send: _, recv } | Self::RecvOnly(recv) => Some(recv),
        }
    }
}

/// The IP sock options used by TCP.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct TcpIpSockOptions {
    /// Socket marks used for routing.
    pub marks: Marks,
}

impl<I: Ip> RouteResolutionOptions<I> for TcpIpSockOptions {
    fn marks(&self) -> &Marks {
        &self.marks
    }

    fn transparent(&self) -> bool {
        false
    }
}

impl<I: IpExt> SendOptions<I> for TcpIpSockOptions {
    fn hop_limit(&self, _destination: &SpecifiedAddr<I::Addr>) -> Option<NonZeroU8> {
        None
    }

    fn multicast_loop(&self) -> bool {
        false
    }

    fn allow_broadcast(&self) -> Option<I::BroadcastMarker> {
        None
    }

    fn dscp_and_ecn(&self) -> DscpAndEcn {
        DscpAndEcn::default()
    }

    fn mtu(&self) -> Mtu {
        Mtu::no_limit()
    }
}

/// TCP socket options.
///
/// This only stores options that are trivial to get and set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SocketOptions {
    /// Socket options that control TCP keep-alive mechanism, see [`KeepAlive`].
    pub keep_alive: KeepAlive,
    /// Switch to turn nagle algorithm on/off.
    pub nagle_enabled: bool,
    /// The period of time after which the connection should be aborted if no
    /// ACK is received.
    pub user_timeout: Option<NonZeroDuration>,
    /// Switch to turn delayed ACK on/off.
    pub delayed_ack: bool,
    /// The period of time after with a dangling FIN_WAIT2 state should be
    /// reclaimed.
    pub fin_wait2_timeout: Option<Duration>,
    /// The maximum SYN retransmissions before aborting a connection.
    pub max_syn_retries: NonZeroU8,
    /// Ip socket options.
    pub ip_options: TcpIpSockOptions,
}

impl Default for SocketOptions {
    fn default() -> Self {
        Self {
            keep_alive: KeepAlive::default(),
            // RFC 9293 Section 3.7.4:
            //   A TCP implementation SHOULD implement the Nagle algorithm to
            //   coalesce short segments
            nagle_enabled: true,
            user_timeout: None,
            delayed_ack: true,
            fin_wait2_timeout: Some(DEFAULT_FIN_WAIT2_TIMEOUT),
            max_syn_retries: DEFAULT_MAX_SYN_RETRIES,
            ip_options: TcpIpSockOptions::default(),
        }
    }
}

/// Options that are related to TCP keep-alive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeepAlive {
    /// The amount of time for an idle connection to wait before sending out
    /// probes.
    pub idle: NonZeroDuration,
    /// Interval between consecutive probes.
    pub interval: NonZeroDuration,
    /// Maximum number of probes we send before considering the connection dead.
    ///
    /// `u8` is enough because if a connection doesn't hear back from the peer
    /// after 256 probes, then chances are that the connection is already dead.
    pub count: NonZeroU8,
    /// Only send probes if keep-alive is enabled.
    pub enabled: bool,
}

impl Default for KeepAlive {
    fn default() -> Self {
        // Default values inspired by Linux's TCP implementation:
        // https://github.com/torvalds/linux/blob/0326074ff4652329f2a1a9c8685104576bd8d131/include/net/tcp.h#L155-L157
        const DEFAULT_IDLE_DURATION: NonZeroDuration =
            NonZeroDuration::from_secs(2 * 60 * 60).unwrap();
        const DEFAULT_INTERVAL: NonZeroDuration = NonZeroDuration::from_secs(75).unwrap();
        const DEFAULT_COUNT: NonZeroU8 = NonZeroU8::new(9).unwrap();

        Self {
            idle: DEFAULT_IDLE_DURATION,
            interval: DEFAULT_INTERVAL,
            count: DEFAULT_COUNT,
            // Per RFC 9293(https://datatracker.ietf.org/doc/html/rfc9293#section-3.8.4):
            //   ... they MUST default to off.
            enabled: false,
        }
    }
}
