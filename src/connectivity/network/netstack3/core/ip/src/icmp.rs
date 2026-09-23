// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The Internet Control Message Protocol (ICMP).

pub mod counters;

use alloc::boxed::Box;
use core::convert::{Infallible as Never, TryInto as _};
use core::fmt::Debug;
use core::num::{NonZeroU8, NonZeroU16};

use lock_order::lock::{OrderedLockAccess, OrderedLockRef};
use log::{debug, error, trace};
use net_types::ip::{
    GenericOverIp, Ip, IpMarked, Ipv4, Ipv4Addr, Ipv4SourceAddr, Ipv6, Ipv6Addr, Ipv6SourceAddr,
    Mtu, SubnetError,
};
use net_types::{
    LinkLocalAddress, LinkLocalUnicastAddr, MulticastAddr, MulticastAddress, SpecifiedAddr,
    UnicastAddr, Witness,
};
use netstack3_base::socket::{AddrIsMappedError, SocketIpAddr, SocketIpAddrExt as _};
use netstack3_base::sync::Mutex;
use netstack3_base::{
    AnyDevice, Counter, CounterContext, DeviceIdContext, EitherDeviceId, FrameDestination,
    IcmpIpExt, Icmpv4ErrorCode, Icmpv6ErrorCode, InstantBindingsTypes, InstantContext,
    IpDeviceAddr, IpExt, LocalFrameDestination, Marks, NetworkPartialSerializer, NetworkSerializer,
    RngContext, TokenBucket, TxMetadataBindingsTypes,
};
use netstack3_filter::{DynTransportSerializer, DynamicTransportSerializer, FilterIpExt};
use packet::{
    BufferMut, InnerPacketBuilder as _, NestablePacketBuilder as _, ParsablePacket as _,
    ParseBuffer, TruncateDirection, TruncatingSerializer,
};
use packet_formats::icmp::ndp::options::{NdpOption, NdpOptionBuilder};
use packet_formats::icmp::ndp::{
    NdpPacket, NeighborAdvertisement, NeighborSolicitation, NonZeroNdpLifetime,
    OptionSequenceBuilder, RouterSolicitation,
};
use packet_formats::icmp::{
    IcmpDestUnreachable, IcmpEchoRequest, IcmpMessage, IcmpMessageType, IcmpPacket,
    IcmpPacketBuilder, IcmpPacketRaw, IcmpParseArgs, IcmpTimeExceeded, IcmpZeroCode,
    Icmpv4DestUnreachableCode, Icmpv4Packet, Icmpv4ParameterProblem, Icmpv4ParameterProblemCode,
    Icmpv4TimeExceededCode, Icmpv6DestUnreachableCode, Icmpv6Packet, Icmpv6PacketTooBig,
    Icmpv6ParameterProblem, Icmpv6ParameterProblemCode, Icmpv6TimeExceededCode, MessageBody,
    OriginalPacket, peek_message_type,
};
use packet_formats::ip::{DscpAndEcn, Ipv4Proto, Ipv6Proto};
use packet_formats::ipv4::Ipv4Header;
use packet_formats::ipv6::{ExtHdrParseError, Ipv6Header};
use zerocopy::SplitByteSlice;

use crate::IpLayerIpExt;
use crate::internal::base::{
    AddressStatus, IPV6_DEFAULT_SUBNET, IpDeviceIngressStateContext, IpLayerHandler,
    IpPacketDestination, IpSendFrameError, IpTransportContext, Ipv6PresentAddressStatus,
    NdpBindingsContext, RouterAdvertisementEvent, SendIpPacketMeta,
};
use crate::internal::device::nud::{ConfirmationFlags, NudIpHandler};
use crate::internal::device::route_discovery::{
    Ipv6DiscoveredRoute, Ipv6DiscoveredRouteProperties,
};
use crate::internal::device::{
    IpAddressState, IpDeviceHandler, Ipv6DeviceHandler, Ipv6LinkLayerAddr,
};
use crate::internal::icmp::counters::{IcmpCountersIpExt, IcmpRxCounters, IcmpTxCounters};
use crate::internal::local_delivery::{IpHeaderInfo, LocalDeliveryPacketInfo, ReceiveIpPacketMeta};
use crate::internal::path_mtu::PmtuHandler;
use crate::internal::socket::{
    DelegatedRouteResolutionOptions, DelegatedSendOptions, IpSocketArgs, IpSocketHandler,
    OptionDelegationMarker, RouteResolutionOptions, SendOptions,
};
use crate::internal::types::RoutePreference;

/// The IP packet hop limit for all NDP packets.
///
/// See [RFC 4861 section 4.1], [RFC 4861 section 4.2], [RFC 4861 section 4.2],
/// [RFC 4861 section 4.3], [RFC 4861 section 4.4], and [RFC 4861 section 4.5]
/// for more information.
///
/// [RFC 4861 section 4.1]: https://tools.ietf.org/html/rfc4861#section-4.1
/// [RFC 4861 section 4.2]: https://tools.ietf.org/html/rfc4861#section-4.2
/// [RFC 4861 section 4.3]: https://tools.ietf.org/html/rfc4861#section-4.3
/// [RFC 4861 section 4.4]: https://tools.ietf.org/html/rfc4861#section-4.4
/// [RFC 4861 section 4.5]: https://tools.ietf.org/html/rfc4861#section-4.5
pub const REQUIRED_NDP_IP_PACKET_HOP_LIMIT: u8 = 255;

/// The default number of ICMP error messages to send per second.
///
/// Beyond this rate, error messages will be silently dropped.
///
/// The current value (1000) was inspired by Netstack2 (gVisor).
// TODO(https://fxbug.dev/407541323): Consider tuning the ICMP rate limiting
// behavior to conform more closely to Linux.
pub const DEFAULT_ERRORS_PER_SECOND: u64 = 1000;
/// The IP layer's ICMP state.
#[derive(GenericOverIp)]
#[generic_over_ip(I, Ip)]
pub struct IcmpState<I: IpExt + IcmpCountersIpExt, BT: IcmpBindingsTypes> {
    error_send_bucket: Mutex<IpMarked<I, TokenBucket<BT::Instant>>>,
    /// ICMP transmit counters.
    pub tx_counters: IcmpTxCounters<I>,
    /// ICMP receive counters.
    pub rx_counters: IcmpRxCounters<I>,
}

impl<I, BT> OrderedLockAccess<IpMarked<I, TokenBucket<BT::Instant>>> for IcmpState<I, BT>
where
    I: IpExt + IcmpCountersIpExt,
    BT: IcmpBindingsTypes,
{
    type Lock = Mutex<IpMarked<I, TokenBucket<BT::Instant>>>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.error_send_bucket)
    }
}

/// Receive NDP counters.
#[derive(Default)]
pub struct NdpRxCounters {
    /// Count of neighbor solicitation messages received.
    pub neighbor_solicitation: Counter,
    /// Count of neighbor advertisement messages received.
    pub neighbor_advertisement: Counter,
    /// Count of router advertisement messages received.
    pub router_advertisement: Counter,
    /// Count of router solicitation messages received.
    pub router_solicitation: Counter,
}

/// Transmit NDP counters.
#[derive(Default)]
pub struct NdpTxCounters {
    /// Count of neighbor advertisement messages sent.
    pub neighbor_advertisement: Counter,
    /// Count of neighbor solicitation messages sent.
    pub neighbor_solicitation: Counter,
}

/// Counters for NDP messages.
#[derive(Default)]
pub struct NdpCounters {
    /// Receive counters.
    pub rx: NdpRxCounters,
    /// Transmit counters.
    pub tx: NdpTxCounters,
}

/// A builder for ICMPv4 state.
#[derive(Copy, Clone)]
pub struct Icmpv4StateBuilder {
    send_timestamp_reply: bool,
    errors_per_second: u64,
}

impl Default for Icmpv4StateBuilder {
    fn default() -> Icmpv4StateBuilder {
        Icmpv4StateBuilder {
            send_timestamp_reply: false,
            errors_per_second: DEFAULT_ERRORS_PER_SECOND,
        }
    }
}

impl Icmpv4StateBuilder {
    /// Enable or disable replying to ICMPv4 Timestamp Request messages with
    /// Timestamp Reply messages (default: disabled).
    ///
    /// Enabling this can introduce a very minor vulnerability in which an
    /// attacker can learn the system clock's time, which in turn can aid in
    /// attacks against time-based authentication systems.
    pub fn send_timestamp_reply(&mut self, send_timestamp_reply: bool) -> &mut Self {
        self.send_timestamp_reply = send_timestamp_reply;
        self
    }

    /// Builds an [`Icmpv4State`].
    pub fn build<BT: IcmpBindingsTypes>(self) -> Icmpv4State<BT> {
        Icmpv4State {
            inner: IcmpState {
                error_send_bucket: Mutex::new(IpMarked::new(TokenBucket::new(
                    self.errors_per_second,
                ))),
                tx_counters: Default::default(),
                rx_counters: Default::default(),
            },
            send_timestamp_reply: self.send_timestamp_reply,
        }
    }
}

/// The state associated with the ICMPv4 protocol.
pub struct Icmpv4State<BT: IcmpBindingsTypes> {
    /// The inner common ICMP state.
    pub inner: IcmpState<Ipv4, BT>,
    /// Whether the stack is configured to send ICMP timestamp replies.
    pub send_timestamp_reply: bool,
}

impl<BT: IcmpBindingsTypes> AsRef<IcmpState<Ipv4, BT>> for Icmpv4State<BT> {
    fn as_ref(&self) -> &IcmpState<Ipv4, BT> {
        &self.inner
    }
}

impl<BT: IcmpBindingsTypes> AsMut<IcmpState<Ipv4, BT>> for Icmpv4State<BT> {
    fn as_mut(&mut self) -> &mut IcmpState<Ipv4, BT> {
        &mut self.inner
    }
}

/// A builder for ICMPv6 state.
#[derive(Copy, Clone)]
pub(crate) struct Icmpv6StateBuilder {
    errors_per_second: u64,
}

impl Default for Icmpv6StateBuilder {
    fn default() -> Icmpv6StateBuilder {
        Icmpv6StateBuilder { errors_per_second: DEFAULT_ERRORS_PER_SECOND }
    }
}

impl Icmpv6StateBuilder {
    pub(crate) fn build<BT: IcmpBindingsTypes>(self) -> Icmpv6State<BT> {
        Icmpv6State {
            inner: IcmpState {
                error_send_bucket: Mutex::new(IpMarked::new(TokenBucket::new(
                    self.errors_per_second,
                ))),
                tx_counters: Default::default(),
                rx_counters: Default::default(),
            },
            ndp_counters: Default::default(),
        }
    }
}

/// The state associated with the ICMPv6 protocol.
pub struct Icmpv6State<BT: IcmpBindingsTypes> {
    /// The inner common ICMP state.
    pub inner: IcmpState<Ipv6, BT>,
    /// Neighbor discovery protocol counters.
    pub ndp_counters: NdpCounters,
}

impl<BT: IcmpBindingsTypes> AsRef<IcmpState<Ipv6, BT>> for Icmpv6State<BT> {
    fn as_ref(&self) -> &IcmpState<Ipv6, BT> {
        &self.inner
    }
}

impl<BT: IcmpBindingsTypes> AsMut<IcmpState<Ipv6, BT>> for Icmpv6State<BT> {
    fn as_mut(&mut self) -> &mut IcmpState<Ipv6, BT> {
        &mut self.inner
    }
}

/// An extension trait providing ICMP handler properties.
pub trait IcmpHandlerIpExt: IpExt {
    /// The type of ICMP error messages.
    type IcmpError: IcmpError
        + GenericOverIp<Self, Type = Self::IcmpError>
        + GenericOverIp<Ipv4, Type = Icmpv4Error>
        + GenericOverIp<Ipv6, Type = Icmpv6Error>;

    /// A try-conversion from [`Self::RecvSrcAddr`] to [`SocketIpAddr`].
    fn received_source_as_icmp_source(src: Self::RecvSrcAddr) -> Option<SocketIpAddr<Self::Addr>>;

    /// Returns the ICMP error to send when NUD fails.
    fn nud_failure_icmp_error() -> Self::IcmpError;
}

impl IcmpHandlerIpExt for Ipv4 {
    type IcmpError = Icmpv4Error;

    fn received_source_as_icmp_source(src: Ipv4SourceAddr) -> Option<SocketIpAddr<Ipv4Addr>> {
        SocketIpAddr::new_from_ipv4_source(src)
    }

    fn nud_failure_icmp_error() -> Icmpv4Error {
        Icmpv4Error::HostUnreachable
    }
}

impl IcmpHandlerIpExt for Ipv6 {
    type IcmpError = Icmpv6Error;

    fn received_source_as_icmp_source(src: Ipv6SourceAddr) -> Option<SocketIpAddr<Ipv6Addr>> {
        SocketIpAddr::new_from_ipv6_source(src)
    }

    fn nud_failure_icmp_error() -> Icmpv6Error {
        Icmpv6Error::AddressUnreachable
    }
}

/// A trait for `Icmpv4Error` and `Icmpv6Error`.
pub trait IcmpError: Sized + Debug + PartialEq {
    /// A port unreachable error.
    fn port_unreachable() -> Self;
    /// A time to live expired error.
    fn ttl_expired() -> Self;
    /// An MTU exceeded error. Returns `None` for IPv4 since it doesn't have an
    /// MTU exceeded error.
    fn mtu_exceeded(mtu: Mtu) -> Option<Self>;
}

/// A kind of ICMPv4 error.
#[derive(Debug, PartialEq)]
pub enum Icmpv4Error {
    /// Parameter problem.
    ParameterProblem {
        /// The parameter problem code.
        code: Icmpv4ParameterProblemCode,
        /// The pointer to the byte in the original datagram that caused the error.
        pointer: u8,
    },
    /// Time to live expired.
    TtlExpired,
    /// Destination network is unreachable.
    NetUnreachable,
    /// Destination protocol is unreachable.
    ProtocolUnreachable,
    /// Destination port is unreachable.
    PortUnreachable,
    /// Host is unreachable.
    HostUnreachable,
    /// Network administratively prohibited.
    NetworkProhibited,
    /// Host administratively prohibited.
    HostProhibited,
    /// Communication administratively prohibited.
    AdminProhibited,
}

impl IcmpError for Icmpv4Error {
    fn port_unreachable() -> Self {
        Icmpv4Error::PortUnreachable
    }
    fn ttl_expired() -> Self {
        Icmpv4Error::TtlExpired
    }
    fn mtu_exceeded(_mtu: Mtu) -> Option<Self> {
        // TODO(https://fxbug.dev/552550972): Send ICMP Destination Unreachable
        // error with "Fragmentation needed and DF Set" code.
        None
    }
}

impl<I: IcmpHandlerIpExt> GenericOverIp<I> for Icmpv4Error {
    type Type = I::IcmpError;
}

/// A type to allow implementing the required filtering traits on a concrete
/// subset of message types.
enum Icmpv4ErrorMessage {
    TimeExceeded {
        message: IcmpTimeExceeded,
        code: <IcmpTimeExceeded as IcmpMessage<Ipv4>>::Code,
    },
    ParameterProblem {
        message: Icmpv4ParameterProblem,
        code: <Icmpv4ParameterProblem as IcmpMessage<Ipv4>>::Code,
    },
    DestUnreachable {
        message: IcmpDestUnreachable,
        code: <IcmpDestUnreachable as IcmpMessage<Ipv4>>::Code,
    },
}

impl Icmpv4Error {
    fn update_counters(&self, counters: &IcmpTxCounters<Ipv4>) {
        match self {
            Icmpv4Error::ParameterProblem { code, pointer: _ } => {
                counters.parameter_problem.increment_code(*code);
            }
            Icmpv4Error::TtlExpired => {
                counters.time_exceeded.increment_code(Icmpv4TimeExceededCode::TtlExpired);
            }
            Icmpv4Error::NetUnreachable => {
                counters
                    .dest_unreachable
                    .increment_code(Icmpv4DestUnreachableCode::DestNetworkUnreachable);
            }
            Icmpv4Error::ProtocolUnreachable => {
                counters
                    .dest_unreachable
                    .increment_code(Icmpv4DestUnreachableCode::DestProtocolUnreachable);
            }
            Icmpv4Error::PortUnreachable => {
                counters
                    .dest_unreachable
                    .increment_code(Icmpv4DestUnreachableCode::DestPortUnreachable);
            }
            Icmpv4Error::HostUnreachable => {
                counters
                    .dest_unreachable
                    .increment_code(Icmpv4DestUnreachableCode::DestHostUnreachable);
            }
            Icmpv4Error::NetworkProhibited => {
                counters
                    .dest_unreachable
                    .increment_code(Icmpv4DestUnreachableCode::NetworkAdministrativelyProhibited);
            }
            Icmpv4Error::HostProhibited => {
                counters
                    .dest_unreachable
                    .increment_code(Icmpv4DestUnreachableCode::HostAdministrativelyProhibited);
            }
            Icmpv4Error::AdminProhibited => {
                counters
                    .dest_unreachable
                    .increment_code(Icmpv4DestUnreachableCode::CommAdministrativelyProhibited);
            }
        }
    }

    fn create_message(&self) -> Icmpv4ErrorMessage {
        match self {
            Icmpv4Error::ParameterProblem { code, pointer } => {
                Icmpv4ErrorMessage::ParameterProblem {
                    message: Icmpv4ParameterProblem::new(*pointer),
                    code: *code,
                }
            }
            Icmpv4Error::TtlExpired => Icmpv4ErrorMessage::TimeExceeded {
                message: IcmpTimeExceeded::default(),
                code: Icmpv4TimeExceededCode::TtlExpired,
            },
            Icmpv4Error::NetUnreachable => Icmpv4ErrorMessage::DestUnreachable {
                message: IcmpDestUnreachable::default(),
                code: Icmpv4DestUnreachableCode::DestNetworkUnreachable,
            },
            Icmpv4Error::ProtocolUnreachable => Icmpv4ErrorMessage::DestUnreachable {
                message: IcmpDestUnreachable::default(),
                code: Icmpv4DestUnreachableCode::DestProtocolUnreachable,
            },
            Icmpv4Error::PortUnreachable => Icmpv4ErrorMessage::DestUnreachable {
                message: IcmpDestUnreachable::default(),
                code: Icmpv4DestUnreachableCode::DestPortUnreachable,
            },
            Icmpv4Error::HostUnreachable => Icmpv4ErrorMessage::DestUnreachable {
                message: IcmpDestUnreachable::default(),
                code: Icmpv4DestUnreachableCode::DestHostUnreachable,
            },
            Icmpv4Error::NetworkProhibited => Icmpv4ErrorMessage::DestUnreachable {
                message: IcmpDestUnreachable::default(),
                code: Icmpv4DestUnreachableCode::NetworkAdministrativelyProhibited,
            },
            Icmpv4Error::HostProhibited => Icmpv4ErrorMessage::DestUnreachable {
                message: IcmpDestUnreachable::default(),
                code: Icmpv4DestUnreachableCode::HostAdministrativelyProhibited,
            },
            Icmpv4Error::AdminProhibited => Icmpv4ErrorMessage::DestUnreachable {
                message: IcmpDestUnreachable::default(),
                code: Icmpv4DestUnreachableCode::CommAdministrativelyProhibited,
            },
        }
    }
}

impl<I: IcmpHandlerIpExt> GenericOverIp<I> for Icmpv6Error {
    type Type = I::IcmpError;
}

/// A kind of ICMPv6 error.
#[derive(Debug, PartialEq, Eq)]
pub enum Icmpv6Error {
    /// Parameter problem.
    ParameterProblem {
        /// The parameter problem code.
        code: Icmpv6ParameterProblemCode,
        /// The pointer to the byte in the original datagram that caused the error.
        pointer: u32,
        /// Whether the destination multicast address is allowed.
        allow_dst_multicast: bool,
    },
    /// Time to live expired.
    TtlExpired,
    /// Destination network is unreachable.
    NetUnreachable,
    /// Packet too big.
    PacketTooBig {
        /// The MTU of the link.
        mtu: Mtu,
    },
    /// Destination port is unreachable.
    PortUnreachable,
    /// Address is unreachable.
    AddressUnreachable,
    /// Reject route to destination.
    RejectRoute,
    /// Source address failed ingress/egress policy.
    SourceAddressPolicyFailed,
    /// Communication with destination administratively prohibited.
    AdminProhibited,
}

impl IcmpError for Icmpv6Error {
    fn port_unreachable() -> Self {
        Icmpv6Error::PortUnreachable
    }
    fn ttl_expired() -> Self {
        Icmpv6Error::TtlExpired
    }
    fn mtu_exceeded(mtu: Mtu) -> Option<Self> {
        Some(Icmpv6Error::PacketTooBig { mtu })
    }
}

/// A type to allow implementing the required filtering traits on a concrete
/// subset of message types.
enum Icmpv6ErrorMessage {
    TimeExceeded {
        message: IcmpTimeExceeded,
        code: <IcmpTimeExceeded as IcmpMessage<Ipv6>>::Code,
    },
    PacketTooBig {
        message: Icmpv6PacketTooBig,
        code: <Icmpv6PacketTooBig as IcmpMessage<Ipv6>>::Code,
    },
    ParameterProblem {
        message: Icmpv6ParameterProblem,
        code: <Icmpv6ParameterProblem as IcmpMessage<Ipv6>>::Code,
    },
    DestUnreachable {
        message: IcmpDestUnreachable,
        code: <IcmpDestUnreachable as IcmpMessage<Ipv6>>::Code,
    },
}

impl Icmpv6Error {
    fn update_counters(&self, counters: &IcmpTxCounters<Ipv6>) {
        match self {
            Icmpv6Error::ParameterProblem { code, pointer: _, allow_dst_multicast: _ } => {
                counters.parameter_problem.increment_code(*code);
            }
            Icmpv6Error::TtlExpired => {
                counters.time_exceeded.increment_code(Icmpv6TimeExceededCode::HopLimitExceeded);
            }
            Icmpv6Error::NetUnreachable => {
                counters.dest_unreachable.increment_code(Icmpv6DestUnreachableCode::NoRoute);
            }
            Icmpv6Error::PacketTooBig { mtu: _ } => {
                counters.packet_too_big.increment();
            }
            Icmpv6Error::PortUnreachable => {
                counters
                    .dest_unreachable
                    .increment_code(Icmpv6DestUnreachableCode::PortUnreachable);
            }
            Icmpv6Error::AddressUnreachable => {
                counters
                    .dest_unreachable
                    .increment_code(Icmpv6DestUnreachableCode::AddrUnreachable);
            }
            Icmpv6Error::RejectRoute => {
                counters.dest_unreachable.increment_code(Icmpv6DestUnreachableCode::RejectRoute);
            }
            Icmpv6Error::SourceAddressPolicyFailed => {
                counters
                    .dest_unreachable
                    .increment_code(Icmpv6DestUnreachableCode::SrcAddrFailedPolicy);
            }
            Icmpv6Error::AdminProhibited => {
                counters
                    .dest_unreachable
                    .increment_code(Icmpv6DestUnreachableCode::CommAdministrativelyProhibited);
            }
        }
    }

    fn create_message(&self) -> Icmpv6ErrorMessage {
        match self {
            Icmpv6Error::ParameterProblem { code, pointer, allow_dst_multicast: _ } => {
                Icmpv6ErrorMessage::ParameterProblem {
                    message: Icmpv6ParameterProblem::new(*pointer),
                    code: *code,
                }
            }
            Icmpv6Error::TtlExpired => Icmpv6ErrorMessage::TimeExceeded {
                message: IcmpTimeExceeded::default(),
                code: Icmpv6TimeExceededCode::HopLimitExceeded,
            },
            Icmpv6Error::NetUnreachable => Icmpv6ErrorMessage::DestUnreachable {
                message: IcmpDestUnreachable::default(),
                code: Icmpv6DestUnreachableCode::NoRoute,
            },
            Icmpv6Error::PacketTooBig { mtu } => Icmpv6ErrorMessage::PacketTooBig {
                message: Icmpv6PacketTooBig::new((*mtu).into()),
                code: IcmpZeroCode,
            },
            Icmpv6Error::PortUnreachable => Icmpv6ErrorMessage::DestUnreachable {
                message: IcmpDestUnreachable::default(),
                code: Icmpv6DestUnreachableCode::PortUnreachable,
            },
            Icmpv6Error::AddressUnreachable => Icmpv6ErrorMessage::DestUnreachable {
                message: IcmpDestUnreachable::default(),
                code: Icmpv6DestUnreachableCode::AddrUnreachable,
            },
            Icmpv6Error::RejectRoute => Icmpv6ErrorMessage::DestUnreachable {
                message: IcmpDestUnreachable::default(),
                code: Icmpv6DestUnreachableCode::RejectRoute,
            },
            Icmpv6Error::SourceAddressPolicyFailed => Icmpv6ErrorMessage::DestUnreachable {
                message: IcmpDestUnreachable::default(),
                code: Icmpv6DestUnreachableCode::SrcAddrFailedPolicy,
            },
            Icmpv6Error::AdminProhibited => Icmpv6ErrorMessage::DestUnreachable {
                message: IcmpDestUnreachable::default(),
                code: Icmpv6DestUnreachableCode::CommAdministrativelyProhibited,
            },
        }
    }

    fn allow_dst_multicast(&self) -> bool {
        // As per RFC 4443 section 2.4.e,
        //
        //   An ICMPv6 error message MUST NOT be originated as a result of
        //   receiving the following:
        //
        //     (e.3) A packet destined to an IPv6 multicast address.  (There are
        //           two exceptions to this rule: (1) the Packet Too Big Message
        //           (Section 3.2) to allow Path MTU discovery to work for IPv6
        //           multicast, and (2) the Parameter Problem Message, Code 2
        //           (Section 3.4) reporting an unrecognized IPv6 option (see
        //           Section 4.2 of [IPv6]) that has the Option Type highest-
        //           order two bits set to 10).
        match self {
            Icmpv6Error::ParameterProblem { allow_dst_multicast, code, pointer: _ } => {
                assert!(
                    !allow_dst_multicast
                        || *code == Icmpv6ParameterProblemCode::UnrecognizedIpv6Option
                );
                *allow_dst_multicast
            }
            Icmpv6Error::PacketTooBig { .. } => true,
            _ => false,
        }
    }
}

/// The handler exposed by ICMP.
pub trait IcmpErrorHandler<I: IcmpHandlerIpExt, BC>: DeviceIdContext<AnyDevice> {
    /// Sends an error message in response to an incoming packet.
    ///
    /// `src_ip` and `dst_ip` are the source and destination addresses of the
    /// incoming packet.
    /// `original_packet` contains the contents of the entire original packet,
    /// including the IP header. This must be a whole packet, not a packet fragment.
    fn send_icmp_error_message<B: BufferMut>(
        &mut self,
        bindings_ctx: &mut BC,
        device: Option<&Self::DeviceId>,
        frame_dst: Option<LocalFrameDestination>,
        src_ip: SocketIpAddr<I::Addr>,
        dst_ip: SocketIpAddr<I::Addr>,
        original_packet: B,
        error: I::IcmpError,
        header_len: usize,
        proto: I::Proto,
        marks: &Marks,
    );
}

impl<BC: IcmpBindingsContext, CC: IcmpSendContext<Ipv4, BC> + CounterContext<IcmpTxCounters<Ipv4>>>
    IcmpErrorHandler<Ipv4, BC> for CC
{
    fn send_icmp_error_message<B: BufferMut>(
        &mut self,
        bindings_ctx: &mut BC,
        device: Option<&CC::DeviceId>,
        frame_dst: Option<LocalFrameDestination>,
        src_ip: SocketIpAddr<Ipv4Addr>,
        dst_ip: SocketIpAddr<Ipv4Addr>,
        original_packet: B,
        icmp_error: Icmpv4Error,
        header_len: usize,
        proto: Ipv4Proto,
        marks: &Marks,
    ) {
        // Check whether we MUST NOT send an ICMP error message
        // because the original packet was itself an ICMP error message.
        if is_icmp_error_or_redirect_message::<Ipv4>(proto, &original_packet.as_ref()[header_len..])
        {
            return;
        }

        icmp_error.update_counters(&self.counters());
        send_icmpv4_error_message(
            self,
            bindings_ctx,
            device,
            frame_dst,
            src_ip,
            dst_ip,
            icmp_error.create_message(),
            original_packet,
            header_len,
            marks,
        );
    }
}

impl<BC: IcmpBindingsContext, CC: IcmpSendContext<Ipv6, BC> + CounterContext<IcmpTxCounters<Ipv6>>>
    IcmpErrorHandler<Ipv6, BC> for CC
{
    fn send_icmp_error_message<B: BufferMut>(
        &mut self,
        bindings_ctx: &mut BC,
        device: Option<&CC::DeviceId>,
        frame_dst: Option<LocalFrameDestination>,
        src_ip: SocketIpAddr<Ipv6Addr>,
        dst_ip: SocketIpAddr<Ipv6Addr>,
        original_packet: B,
        error: Icmpv6Error,
        header_len: usize,
        proto: Ipv6Proto,
        marks: &Marks,
    ) {
        // Check whether we MUST NOT send an ICMP error message because the
        // original packet was itself an ICMP error or redirect message.
        if is_icmp_error_or_redirect_message::<Ipv6>(proto, &original_packet.as_ref()[header_len..])
        {
            return;
        }

        error.update_counters(&self.counters());
        send_icmpv6_error_message(
            self,
            bindings_ctx,
            device,
            frame_dst,
            src_ip,
            dst_ip,
            error.create_message(),
            original_packet,
            error.allow_dst_multicast(),
            marks,
        )
    }
}

/// A marker for all the contexts provided by bindings require by the ICMP
/// module.
pub trait IcmpBindingsContext: InstantContext + RngContext + IcmpBindingsTypes {}
impl<BC> IcmpBindingsContext for BC where
    BC: InstantContext + RngContext + IcmpBindingsTypes + IcmpBindingsTypes
{
}

/// A marker trait for all bindings types required by the ICMP module.
pub trait IcmpBindingsTypes: InstantBindingsTypes + TxMetadataBindingsTypes {}
impl<BT: InstantBindingsTypes + TxMetadataBindingsTypes> IcmpBindingsTypes for BT {}

/// Empty trait to work around coherence issues.
///
/// This serves only to convince the coherence checker that a particular blanket
/// trait implementation could only possibly conflict with other blanket impls
/// in this crate. It can be safely implemented for any type.
/// TODO(https://github.com/rust-lang/rust/issues/97811): Remove this once the
/// coherence checker doesn't require it.
pub trait IcmpStateContext {}

/// A marker trait to prevent integration from creating a recursive loop when
/// handling Echo sockets.
///
/// This is a requirement for [`InnerIcmpContext::EchoTransportContext`] which
/// disallows the integration layer from using [`IcmpIpTransportContext`] as the
/// associated type, which would create a recursive loop.
///
/// By *not implementing* this trait for [`IcmpIpTransporContext`] we prevent
/// the mistake.
pub trait EchoTransportContextMarker {}

/// The execution context shared by ICMP(v4) and ICMPv6 for the internal
/// operations of the IP stack.
pub trait InnerIcmpContext<I, BC>: IpSocketHandler<I, BC>
where
    I: IpLayerIpExt,
    BC: IcmpBindingsTypes,
{
    /// A type implementing [`IpTransportContext`] that handles ICMP Echo
    /// replies.
    type EchoTransportContext: IpTransportContext<I, BC, Self> + EchoTransportContextMarker;

    // TODO(joshlf): If we end up needing to respond to these messages with new
    // outbound packets, then perhaps it'd be worth passing the original buffer
    // so that it can be reused?
    //
    // NOTE(joshlf): We don't guarantee the packet body length here for two
    // reasons:
    // - It's possible that some IPv4 protocol does or will exist for which
    //   valid packets are less than 8 bytes in length. If we were to reject all
    //   packets with bodies of less than 8 bytes, we might silently discard
    //   legitimate error messages for such protocols.
    // - Even if we were to guarantee this, there's no good way to encode such a
    //   guarantee in the type system, and so the caller would have no recourse
    //   but to panic, and panics have a habit of becoming bugs or DoS
    //   vulnerabilities when invariants change.

    /// Receives an ICMP error message and demultiplexes it to a transport layer
    /// protocol.
    ///
    /// All arguments beginning with `original_` are fields from the IP packet
    /// that triggered the error. The `original_body` is provided here so that
    /// the error can be associated with a transport-layer socket. `device`
    /// identifies the device on which the packet was received.
    ///
    /// While ICMPv4 error messages are supposed to contain the first 8 bytes of
    /// the body of the offending packet, and ICMPv6 error messages are supposed
    /// to contain as much of the offending packet as possible without violating
    /// the IPv6 minimum MTU, the caller does NOT guarantee that either of these
    /// hold. It is `receive_icmp_error`'s responsibility to handle any length
    /// of `original_body`, and to perform any necessary validation.
    fn receive_icmp_error(
        &mut self,
        bindings_ctx: &mut BC,
        device: &Self::DeviceId,
        original_src_ip: Option<SpecifiedAddr<I::Addr>>,
        original_dst_ip: SpecifiedAddr<I::Addr>,
        original_proto: I::Proto,
        original_body: &[u8],
        err: I::ErrorCode,
    );
}

/// The execution context for ICMPv4.
///
/// `InnerIcmpv4Context` is a shorthand for a larger collection of traits.
pub trait InnerIcmpv4Context<BC: IcmpBindingsTypes>: InnerIcmpContext<Ipv4, BC> {
    /// Returns true if a timestamp reply may be sent.
    fn should_send_timestamp_reply(&self) -> bool;
}

/// Context for sending ICMP messages.
pub trait IcmpSendContext<I, BC>: IpSocketHandler<I, BC>
where
    I: IpLayerIpExt,
    BC: IcmpBindingsTypes,
{
    /// Calls the function with a mutable reference to ICMP error send tocket
    /// bucket.
    fn with_error_send_bucket_mut<O, F: FnOnce(&mut TokenBucket<BC::Instant>) -> O>(
        &mut self,
        cb: F,
    ) -> O;
}

/// Attempt to send an ICMP or ICMPv6 error message, applying a rate limit.
///
/// `try_send_error!($core_ctx, $bindings_ctx, $e)` attempts to consume a token from the
/// token bucket at `$core_ctx.get_state_mut().error_send_bucket`. If it
/// succeeds, it invokes the expression `$e`, and otherwise does nothing. It
/// assumes that the type of `$e` is `Result<(), _>` and, in the case that the
/// rate limit is exceeded and it does not invoke `$e`, returns `Ok(())`.
///
/// [RFC 4443 Section 2.4] (f) requires that we MUST limit the rate of outbound
/// ICMPv6 error messages. To our knowledge, there is no similar requirement for
/// ICMPv4, but the same rationale applies, so we do it for ICMPv4 as well.
///
/// [RFC 4443 Section 2.4]: https://tools.ietf.org/html/rfc4443#section-2.4
macro_rules! try_send_error {
    ($core_ctx:expr, $bindings_ctx:expr, $e:expr) => {{
        let send = $core_ctx.with_error_send_bucket_mut(|error_send_bucket| {
            error_send_bucket.try_take($bindings_ctx)
        });

        if send {
            $core_ctx.counters().error.increment();
            $e
        } else {
            trace!("ip::icmp::try_send_error!: dropping rate-limited ICMP error message");
            Ok(())
        }
    }};
}

/// An implementation of [`IpTransportContext`] for ICMP.
pub enum IcmpIpTransportContext {}

fn receive_ip_transport_icmp_error<
    I: IpLayerIpExt,
    CC: InnerIcmpContext<I, BC> + CounterContext<IcmpRxCounters<I>>,
    BC: IcmpBindingsContext,
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device: &CC::DeviceId,
    original_src_ip: Option<SpecifiedAddr<I::Addr>>,
    original_dst_ip: SpecifiedAddr<I::Addr>,
    original_body: &[u8],
    err: I::ErrorCode,
) {
    core_ctx.counters().error_delivered_to_transport_layer.increment();
    trace!("IcmpIpTransportContext::receive_icmp_error({:?})", err);

    let mut parse_body = original_body;
    match parse_body.parse::<IcmpPacketRaw<I, _, IcmpEchoRequest>>() {
        // Only pass things along to the Echo socket layer if this is an echo
        // request.
        Ok(_echo_request) => (),
        Err(_) => {
            // NOTE: This might just mean that the error message was in response
            // to a packet that we sent that wasn't an echo request, so we just
            // silently ignore it.
            return;
        }
    }

    <CC::EchoTransportContext as IpTransportContext<I, BC, CC>>::receive_icmp_error(
        core_ctx,
        bindings_ctx,
        device,
        original_src_ip,
        original_dst_ip,
        original_body,
        err,
    );
}

impl<
    BC: IcmpBindingsContext,
    CC: InnerIcmpv4Context<BC>
        + PmtuHandler<Ipv4, BC>
        + CounterContext<IcmpRxCounters<Ipv4>>
        + CounterContext<IcmpTxCounters<Ipv4>>,
> IpTransportContext<Ipv4, BC, CC> for IcmpIpTransportContext
{
    type EarlyDemuxSocket = Never;

    fn early_demux<B: ParseBuffer>(
        _core_ctx: &mut CC,
        _device: &CC::DeviceId,
        _src_ip: Ipv4Addr,
        _dst_ip: Ipv4Addr,
        _buffer: B,
    ) -> Option<Self::EarlyDemuxSocket> {
        None
    }

    fn receive_icmp_error(
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        device: &CC::DeviceId,
        original_src_ip: Option<SpecifiedAddr<Ipv4Addr>>,
        original_dst_ip: SpecifiedAddr<Ipv4Addr>,
        original_body: &[u8],
        err: Icmpv4ErrorCode,
    ) {
        receive_ip_transport_icmp_error(
            core_ctx,
            bindings_ctx,
            device,
            original_src_ip,
            original_dst_ip,
            original_body,
            err,
        )
    }

    fn receive_ip_packet<B: BufferMut, H: IpHeaderInfo<Ipv4>>(
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        device: &CC::DeviceId,
        src_ip: Ipv4SourceAddr,
        dst_ip: SpecifiedAddr<Ipv4Addr>,
        mut buffer: B,
        info: &mut LocalDeliveryPacketInfo<Ipv4, H>,
        _early_demux_socket: Option<Never>,
    ) -> Result<(), (B, Icmpv4Error)> {
        let LocalDeliveryPacketInfo { meta, header_info: _, marks } = info;
        let ReceiveIpPacketMeta { broadcast: _, transparent_override, parsing_context: _ } = meta;
        if let Some(delivery) = transparent_override {
            unreachable!(
                "cannot perform transparent local delivery {delivery:?} to an ICMP socket; \
                transparent proxy rules can only be configured for TCP and UDP packets"
            );
        }

        trace!(
            "<IcmpIpTransportContext as IpTransportContext<Ipv4>>::receive_ip_packet({}, {})",
            src_ip, dst_ip
        );
        let packet =
            match buffer.parse_with::<_, Icmpv4Packet<_>>(IcmpParseArgs::new(src_ip, dst_ip)) {
                Ok(packet) => packet,
                Err(_) => return Ok(()), // TODO(joshlf): Do something else here?
            };

        match packet {
            Icmpv4Packet::EchoRequest(echo_request) => {
                CounterContext::<IcmpRxCounters<Ipv4>>::counters(core_ctx).echo_request.increment();

                if let Ipv4SourceAddr::Specified(src_ip) = src_ip {
                    let req = *echo_request.message();
                    let code = echo_request.code();
                    let (local_ip, remote_ip) = (dst_ip, src_ip);
                    debug!(
                        "replying to ICMP echo request from {remote_ip} to {local_ip}%{device:?}: \
                        id={}, seq={}",
                        req.id(),
                        req.seq()
                    );
                    send_icmp_reply(
                        core_ctx,
                        bindings_ctx,
                        device,
                        SocketIpAddr::new_ipv4_specified(remote_ip.get()),
                        SocketIpAddr::new_ipv4_specified(local_ip),
                        |src_ip| {
                            IcmpPacketBuilder::<Ipv4, _>::new(src_ip, *remote_ip, code, req.reply())
                                .wrap_body(buffer)
                        },
                        &WithMarks(marks),
                    );
                } else {
                    trace!(
                        "<IcmpIpTransportContext as IpTransportContext<Ipv4>>::receive_ip_packet: \
                        Received echo request with an unspecified source address"
                    );
                }
            }
            Icmpv4Packet::EchoReply(echo_reply) => {
                CounterContext::<IcmpRxCounters<Ipv4>>::counters(core_ctx).echo_reply.increment();
                trace!(
                    "<IcmpIpTransportContext as IpTransportContext<Ipv4>>::receive_ip_packet: \
                    Received an EchoReply message"
                );
                let parse_metadata = echo_reply.parse_metadata();
                buffer.undo_parse(parse_metadata);
                return <CC::EchoTransportContext
                            as IpTransportContext<Ipv4, BC, CC>>::receive_ip_packet(
                        core_ctx,
                        bindings_ctx,
                        device,
                        src_ip,
                        dst_ip,
                        buffer,
                        info,
                        None,
                );
            }
            Icmpv4Packet::TimestampRequest(timestamp_request) => {
                CounterContext::<IcmpRxCounters<Ipv4>>::counters(core_ctx)
                    .timestamp_request
                    .increment();
                if let Ipv4SourceAddr::Specified(src_ip) = src_ip {
                    if core_ctx.should_send_timestamp_reply() {
                        trace!(
                            "<IcmpIpTransportContext as IpTransportContext<Ipv4>>::\
                            receive_ip_packet: Responding to Timestamp Request message"
                        );
                        // We're supposed to respond with the time that we
                        // processed this message as measured in milliseconds
                        // since midnight UT. However, that would require that
                        // we knew the local time zone and had a way to convert
                        // `InstantContext::Instant` to a `u32` value. We can't
                        // do that, and probably don't want to introduce all of
                        // the machinery necessary just to support this one use
                        // case. Luckily, RFC 792 page 17 provides us with an
                        // out:
                        //
                        //   If the time is not available in miliseconds [sic]
                        //   or cannot be provided with respect to midnight UT
                        //   then any time can be inserted in a timestamp
                        //   provided the high order bit of the timestamp is
                        //   also set to indicate this non-standard value.
                        //
                        // Thus, we provide a zero timestamp with the high order
                        // bit set.
                        const NOW: u32 = 0x80000000;
                        let reply = timestamp_request.message().reply(NOW, NOW);
                        let (local_ip, remote_ip) = (dst_ip, src_ip);
                        // We don't actually want to use any of the _contents_
                        // of the buffer, but we would like to reuse it as
                        // scratch space. Eventually, `IcmpPacketBuilder` will
                        // implement `InnerPacketBuilder` for messages without
                        // bodies, but until that happens, we need to give it an
                        // empty buffer.
                        buffer.shrink_front_to(0);
                        send_icmp_reply(
                            core_ctx,
                            bindings_ctx,
                            device,
                            SocketIpAddr::new_ipv4_specified(remote_ip.get()),
                            SocketIpAddr::new_ipv4_specified(local_ip),
                            |src_ip| {
                                IcmpPacketBuilder::<Ipv4, _>::new(
                                    src_ip,
                                    *remote_ip,
                                    IcmpZeroCode,
                                    reply,
                                )
                                .wrap_body(buffer)
                            },
                            &WithMarks(marks),
                        );
                    } else {
                        trace!(
                            "<IcmpIpTransportContext as IpTransportContext<Ipv4>>::\
                            receive_ip_packet: Silently ignoring Timestamp Request message"
                        );
                    }
                } else {
                    trace!(
                        "<IcmpIpTransportContext as IpTransportContext<Ipv4>>::\
                        receive_ip_packet: Received timestamp request with an unspecified source \
                        address"
                    );
                }
            }
            Icmpv4Packet::TimestampReply(_) => {
                // TODO(joshlf): Support sending Timestamp Requests and
                // receiving Timestamp Replies?
                debug!(
                    "<IcmpIpTransportContext as IpTransportContext<Ipv4>>::receive_ip_packet: \
                    Received unsolicited Timestamp Reply message"
                );
            }
            Icmpv4Packet::DestUnreachable(dest_unreachable) => {
                CounterContext::<IcmpRxCounters<Ipv4>>::counters(core_ctx)
                    .dest_unreachable
                    .increment_code(dest_unreachable.code());
                trace!(
                    "<IcmpIpTransportContext as IpTransportContext<Ipv4>>::receive_ip_packet: \
                    Received a Destination Unreachable message"
                );

                let error = if dest_unreachable.code()
                    == Icmpv4DestUnreachableCode::FragmentationRequired
                {
                    let mtu = if let Some(next_hop_mtu) = dest_unreachable.message().next_hop_mtu()
                    {
                        // We are updating the path MTU from the destination
                        // address of this `packet` (which is an IP address on
                        // this node) to some remote (identified by the source
                        // address of this `packet`).
                        //
                        // `update_pmtu_if_less` will only update the PMTU if
                        // the Dest Unreachable message's MTU field had a value
                        // that was at least the IPv4 minimum MTU (which is
                        // required by IPv4 RFC 791).
                        core_ctx.update_pmtu_if_less(
                            bindings_ctx,
                            dst_ip.get(),
                            src_ip.get(),
                            Mtu::new(u32::from(next_hop_mtu.get())),
                        )
                    } else {
                        // If the Next-Hop MTU from an incoming ICMP message is
                        // `0`, then we assume the source node of the ICMP
                        // message does not implement RFC 1191 and therefore
                        // does not actually use the Next-Hop MTU field and
                        // still considers it as an unused field.
                        //
                        // In this case, the only information we have is the
                        // size of the original IP packet that was too big (the
                        // original packet header should be included in the ICMP
                        // response). Here we will simply reduce our PMTU
                        // estimate to a value less than the total length of the
                        // original packet. See RFC 1191 Section 5.
                        //
                        // `update_pmtu_next_lower` may return an error, but it
                        // will only happen if no valid lower value exists from
                        // the original packet's length. It is safe to silently
                        // ignore the error when we have no valid lower PMTU
                        // value as the node from `src_ip` would not be IP RFC
                        // compliant and we expect this to be very rare (for
                        // IPv4, the lowest MTU value for a link can be 68
                        // bytes).
                        let (original_packet_buf, inner_body) = dest_unreachable.body().bytes();
                        // Note: ICMP Dest Unreachable messages don't have a variable size body.
                        debug_assert!(inner_body.is_none());
                        if original_packet_buf.len() >= 4 {
                            // We need the first 4 bytes as the total length
                            // field is at bytes 2/3 of the original packet
                            // buffer.
                            let total_len =
                                u16::from_be_bytes(original_packet_buf[2..4].try_into().unwrap());

                            trace!(
                                "<IcmpIpTransportContext as IpTransportContext<Ipv4>>::\
                                receive_ip_packet: Next-Hop MTU is 0 so using the next best PMTU \
                                value from {total_len}"
                            );

                            core_ctx.update_pmtu_next_lower(
                                bindings_ctx,
                                dst_ip.get(),
                                src_ip.get(),
                                Mtu::new(u32::from(total_len)),
                            )
                        } else {
                            // Ok to silently ignore as RFC 792 requires nodes
                            // to send the original IP packet header + 64 bytes
                            // of the original IP packet's body so the node
                            // itself is already violating the RFC.
                            trace!(
                                "<IcmpIpTransportContext as IpTransportContext<Ipv4>>::\
                                receive_ip_packet: Original packet buf is too small to get \
                                original packet len so ignoring"
                            );
                            None
                        }
                    };
                    mtu.and_then(|mtu| {
                        let mtu = u16::try_from(mtu.get()).unwrap_or(u16::MAX);
                        let mtu = NonZeroU16::new(mtu)?;
                        Some(Icmpv4ErrorCode::DestUnreachable(
                            dest_unreachable.code(),
                            IcmpDestUnreachable::new_for_frag_req(mtu),
                        ))
                    })
                } else {
                    Some(Icmpv4ErrorCode::DestUnreachable(
                        dest_unreachable.code(),
                        *dest_unreachable.message(),
                    ))
                };

                if let Some(error) = error {
                    receive_icmpv4_error(core_ctx, bindings_ctx, device, &dest_unreachable, error);
                }
            }
            Icmpv4Packet::TimeExceeded(time_exceeded) => {
                CounterContext::<IcmpRxCounters<Ipv4>>::counters(core_ctx)
                    .time_exceeded
                    .increment_code(time_exceeded.code());
                trace!(
                    "<IcmpIpTransportContext as IpTransportContext<Ipv4>>::receive_ip_packet: \
                    Received a Time Exceeded message"
                );

                receive_icmpv4_error(
                    core_ctx,
                    bindings_ctx,
                    device,
                    &time_exceeded,
                    Icmpv4ErrorCode::TimeExceeded(time_exceeded.code()),
                );
            }
            // TODO(https://fxbug.dev/323400954): Support ICMP Redirect.
            Icmpv4Packet::Redirect(_) => {
                debug!(
                    "Unimplemented: <IcmpIpTransportContext as IpTransportContext<Ipv4>>::\
                    receive_ip_packet::redirect"
                )
            }
            Icmpv4Packet::ParameterProblem(parameter_problem) => {
                CounterContext::<IcmpRxCounters<Ipv4>>::counters(core_ctx)
                    .parameter_problem
                    .increment_code(parameter_problem.code());
                trace!(
                    "<IcmpIpTransportContext as IpTransportContext<Ipv4>>::receive_ip_packet: \
                    Received a Parameter Problem message"
                );

                receive_icmpv4_error(
                    core_ctx,
                    bindings_ctx,
                    device,
                    &parameter_problem,
                    Icmpv4ErrorCode::ParameterProblem(parameter_problem.code()),
                );
            }
        }

        Ok(())
    }
}

/// A type to allow implementing the required filtering traits on a concrete
/// subset of message types.
#[allow(missing_docs)]
pub enum NdpMessage {
    NeighborSolicitation {
        message: NeighborSolicitation,
        code: <NeighborSolicitation as IcmpMessage<Ipv6>>::Code,
    },

    RouterSolicitation {
        message: RouterSolicitation,
        code: <RouterSolicitation as IcmpMessage<Ipv6>>::Code,
    },

    NeighborAdvertisement {
        message: NeighborAdvertisement,
        code: <NeighborAdvertisement as IcmpMessage<Ipv6>>::Code,
    },
}

/// Sends an NDP packet from `device_id` with the provided parameters.
pub fn send_ndp_packet<BC, CC, S>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &CC::DeviceId,
    src_ip: Option<SpecifiedAddr<Ipv6Addr>>,
    dst_ip: SpecifiedAddr<Ipv6Addr>,
    body: S,
    message: NdpMessage,
) -> Result<(), IpSendFrameError<S>>
where
    CC: IpLayerHandler<Ipv6, BC>,
    S: NetworkSerializer + NetworkPartialSerializer,
    S::Buffer: BufferMut,
{
    macro_rules! send {
        ($message:expr, $code:expr) => {{
            // TODO(https://fxbug.dev/42177356): Send through ICMPv6 send path.
            let mut ser = IcmpPacketBuilder::<Ipv6, _>::new(
                src_ip.map_or(Ipv6::UNSPECIFIED_ADDRESS, |a| a.get()),
                dst_ip.get(),
                $code,
                $message,
            )
            .wrap_body(body);
            match IpLayerHandler::<Ipv6, _>::send_ip_packet_from_device(
                core_ctx,
                bindings_ctx,
                SendIpPacketMeta {
                    device: device_id,
                    src_ip,
                    dst_ip,
                    destination: IpPacketDestination::from_addr(dst_ip),
                    ttl: NonZeroU8::new(REQUIRED_NDP_IP_PACKET_HOP_LIMIT),
                    proto: Ipv6Proto::Icmpv6,
                    mtu: Mtu::no_limit(),
                    dscp_and_ecn: DscpAndEcn::default(),
                },
                DynTransportSerializer::new(&mut ser),
            ) {
                Ok(()) => Ok(()),
                Err(e) => Err(e
                    .map_serializer(|s| {
                        // Get rid of the borrow to the serializer.
                        let _: DynTransportSerializer<'_, _> = s;
                    })
                    .map_serializer(|()| ser.into_inner())),
            }
        }};
    }

    match message {
        NdpMessage::NeighborSolicitation { message, code } => send!(message, code),
        NdpMessage::RouterSolicitation { message, code } => send!(message, code),
        NdpMessage::NeighborAdvertisement { message, code } => send!(message, code),
    }
}

fn send_neighbor_advertisement<
    BC,
    CC: Ipv6DeviceHandler<BC>
        + IpDeviceHandler<Ipv6, BC>
        + IpLayerHandler<Ipv6, BC>
        + CounterContext<NdpCounters>,
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &CC::DeviceId,
    solicited: bool,
    device_addr: UnicastAddr<Ipv6Addr>,
    dst_ip: SpecifiedAddr<Ipv6Addr>,
) {
    core_ctx.counters().tx.neighbor_advertisement.increment();
    debug!("send_neighbor_advertisement from {:?} to {:?}", device_addr, dst_ip);
    // We currently only allow the destination address to be:
    // 1) a unicast address.
    // 2) a multicast destination but the message should be an unsolicited
    //    neighbor advertisement.
    // NOTE: this assertion may need change if more messages are to be allowed
    // in the future.
    debug_assert!(dst_ip.is_valid_unicast() || (!solicited && dst_ip.is_multicast()));

    // We must call into the higher level send_ip_packet_from_device function
    // because it is not guaranteed that we actually know the link-layer
    // address of the destination IP. Typically, the solicitation request will
    // carry that information, but it is not necessary. So it is perfectly valid
    // that trying to send this advertisement will end up triggering a neighbor
    // solicitation to be sent.
    let src_ll = core_ctx.get_link_layer_addr(&device_id);

    // Nothing reasonable to do with the error.
    let advertisement = NeighborAdvertisement::new(
        core_ctx.is_router_device(&device_id),
        solicited,
        // Per RFC 4861, section 7.2.4:
        //
        //    If the Target Address is either an anycast address or a unicast
        //    address for which the node is providing proxy service, or the Target
        //    Link-Layer Address option is not included, the Override flag SHOULD
        //    be set to zero.  Otherwise, the Override flag SHOULD be set to one.
        //
        // We don't support anycast addresses or proxy ARP, and we only send neighbor
        // advertisements for addresses we own, so always set the Override flag.
        //
        // [RFC 4861, section 7.2.4]: https://tools.ietf.org/html/rfc4861#section-7.2.4
        true, /* override_flag */
        device_addr.get(),
    );
    let _: Result<(), _> = send_ndp_packet(
        core_ctx,
        bindings_ctx,
        &device_id,
        Some(device_addr.into_specified()),
        dst_ip,
        OptionSequenceBuilder::new(
            src_ll
                .as_ref()
                .map(Ipv6LinkLayerAddr::as_bytes)
                .map(NdpOptionBuilder::TargetLinkLayerAddress)
                .iter(),
        )
        .into_serializer(),
        NdpMessage::NeighborAdvertisement { message: advertisement, code: IcmpZeroCode },
    );
}

fn receive_ndp_packet<
    B: SplitByteSlice,
    BC: IcmpBindingsContext + NdpBindingsContext<CC::DeviceId>,
    CC: InnerIcmpContext<Ipv6, BC>
        + Ipv6DeviceHandler<BC>
        + IpDeviceHandler<Ipv6, BC>
        + IpDeviceIngressStateContext<Ipv6>
        + NudIpHandler<Ipv6, BC>
        + IpLayerHandler<Ipv6, BC>
        + CounterContext<NdpCounters>,
    H: IpHeaderInfo<Ipv6>,
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &CC::DeviceId,
    src_ip: Ipv6SourceAddr,
    dst_ip: SpecifiedAddr<Ipv6Addr>,
    packet: NdpPacket<B>,
    header_info: &H,
) {
    // All NDP messages should be dropped if the hop-limit != 255. See
    //   Router Solicitations: RFC 4861 section 6.1.1,
    //   Router Advertisements: RFC 4861 section 6.1.2,
    //   Neighbor Solicitations: RFC 4861 section 7.1.1,
    //   Neighbor Advertisements: RFC 4861 section 7.1.2, and
    //   Redirect: RFC 4861 section 8.1:
    //
    //       A node MUST silently discard any received [NDP Message Type]
    //       messages that do not satisfy all of the following validity
    //       checks:
    //
    //          ...
    //
    //          - The IP Hop Limit field has a value of 255, i.e., the packet
    //            could not possibly have been forwarded by a router.
    //
    //          ...
    if header_info.hop_limit() != REQUIRED_NDP_IP_PACKET_HOP_LIMIT {
        trace!("dropping NDP packet from {src_ip} with invalid hop limit");
        return;
    }

    match packet {
        NdpPacket::RouterSolicitation(_) => {}
        // TODO(https://fxbug.dev/42095002): Support NDP Redirect messages.
        NdpPacket::Redirect(_) => {}
        NdpPacket::NeighborSolicitation(ref p) => {
            // Per RFC 4861, section 7.1.1:
            //   A node MUST silently discard any received Neighbor Solicitation
            //   messages that do not satisfy all of the following validity
            //   checks:
            //   [...]
            //     - Target Address is not a multicast address.
            let target_address = p.message().target_address();
            let target_address = match UnicastAddr::new(*target_address) {
                Some(a) => a,
                None => {
                    trace!(
                        "dropping NS from {} with non-unicast target={:?}",
                        src_ip, target_address
                    );
                    return;
                }
            };

            // Extract the options relevant to Neighbor Solicitations
            let (source_link_addr, nonce) = p.body().iter().fold(
                (None, None),
                |(found_source_link_addr, found_nonce), option| {
                    match option {
                        NdpOption::Nonce(nonce) => (found_source_link_addr, Some(nonce)),
                        NdpOption::SourceLinkLayerAddress(source_link_addr) => {
                            (Some(source_link_addr), found_nonce)
                        }
                        // The following options are not expected to be present
                        // in Neighbor Solicitations. As per RFC 4861,
                        // section 7.1.1:
                        //   The contents of any defined options that are not
                        //   specified to be used with Neighbor Solicitation
                        //   messages MUST be ignored and the packet processed
                        //   as normal.
                        NdpOption::Mtu(_)
                        | NdpOption::PrefixInformation(_)
                        | NdpOption::RecursiveDnsServer(_)
                        | NdpOption::RedirectedHeader { .. }
                        | NdpOption::RouteInformation(_)
                        | NdpOption::TargetLinkLayerAddress(_) => {
                            (found_source_link_addr, found_nonce)
                        }
                    }
                },
            );

            if src_ip == Ipv6SourceAddr::Unspecified {
                // Per RFC 4861, section 7.1.1:
                //   A node MUST silently discard any received Neighbor
                //   Solicitation messages that do not satisfy all of the
                //   following validity checks:
                //   [...]
                //     - If the IP source address is the unspecified
                //       address, the IP destination address is a
                //       solicited-node multicast address.
                if target_address.get().to_solicited_node_address().get() != dst_ip.get() {
                    debug!(
                        "dropping NS from {} for {} with invalid IPv6 dst ({}).",
                        src_ip, target_address, dst_ip
                    );
                    return;
                }
                //   [...]
                //     - If the IP source address is the unspecified address,
                //       there is no source link-layer address option in the
                //       message.
                if let Some(addr) = source_link_addr {
                    debug!(
                        "dropping NS from {} for {} with source link-layer addr option ({:?}).",
                        src_ip, target_address, addr
                    );
                    return;
                }
            }

            core_ctx.counters().rx.neighbor_solicitation.increment();

            match src_ip {
                Ipv6SourceAddr::Unspecified => {
                    // The neighbor is performing Duplicate address detection.
                    //
                    // As per RFC 4861 section 4.3,
                    //
                    //   Source Address
                    //       Either an address assigned to the interface from
                    //       which this message is sent or (if Duplicate Address
                    //       Detection is in progress [ADDRCONF]) the
                    //       unspecified address.
                    match IpDeviceHandler::handle_received_dad_packet(
                        core_ctx,
                        bindings_ctx,
                        &device_id,
                        target_address.into_specified(),
                        nonce,
                    ) {
                        Some(IpAddressState::Assigned) => {
                            // Address is assigned to us to we let the
                            // remote node performing DAD that we own the
                            // address.
                            send_neighbor_advertisement(
                                core_ctx,
                                bindings_ctx,
                                &device_id,
                                false,
                                target_address,
                                Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS.into_specified(),
                            );
                        }
                        Some(IpAddressState::Tentative) => {
                            // Nothing further to do in response to DAD
                            // messages.
                        }
                        Some(IpAddressState::Unavailable) | None => {
                            // Nothing further to do for unassigned target
                            // addresses.
                        }
                    }

                    return;
                }
                Ipv6SourceAddr::Unicast(src_ip) => {
                    // Neighbor is performing link address resolution.
                    match core_ctx
                        .address_status_for_device(target_address.into_specified(), device_id)
                    {
                        AddressStatus::Present(Ipv6PresentAddressStatus::UnicastAssigned) => {}
                        AddressStatus::Present(
                            Ipv6PresentAddressStatus::UnicastTentative
                            | Ipv6PresentAddressStatus::Multicast,
                        )
                        | AddressStatus::Unassigned => {
                            // Address is not considered assigned to us as a
                            // unicast so don't send a neighbor advertisement
                            // reply.
                            return;
                        }
                    }

                    if let Some(link_addr) = source_link_addr {
                        NudIpHandler::handle_neighbor_probe(
                            core_ctx,
                            bindings_ctx,
                            &device_id,
                            src_ip.into_specified(),
                            link_addr,
                        );
                    }

                    send_neighbor_advertisement(
                        core_ctx,
                        bindings_ctx,
                        &device_id,
                        true,
                        target_address,
                        src_ip.into_specified(),
                    );
                }
            }
        }
        NdpPacket::NeighborAdvertisement(ref p) => {
            // TODO(https://fxbug.dev/42179526): Invalidate discovered routers when
            // neighbor entry's IsRouter field transitions to false.

            let target_address = p.message().target_address();

            // As Per RFC 4861, section 7.1.2:
            //   A node MUST silently discard any received Neighbor
            //   Advertisement messages that do not satisfy all of the following
            //   validity checks:
            //   [...]
            //     - Target Address is not a multicast address.
            let target_address = match UnicastAddr::new(*target_address) {
                Some(a) => a,
                None => {
                    debug!(
                        "dropping NA from {} with non-unicast target={:?}",
                        src_ip, target_address
                    );
                    return;
                }
            };
            //   [...]
            //     - If the IP Destination Address is a multicast address the
            //       Solicited flag is zero.
            if let Some(dst_ip) = MulticastAddr::new(dst_ip.get())
                && p.message().solicited_flag()
            {
                debug!(
                    "dropping NA from {} with solicited flag and multicast dst {}",
                    src_ip, dst_ip
                );
                return;
            }

            core_ctx.counters().rx.neighbor_advertisement.increment();

            // Note: Neighbor Advertisements don't carry a nonce value. Handle
            // a NA in the same way that we would handle an NS that omitted the
            // nonce (i.e. conclude it's not-looped-back).
            let nonce = None;
            match IpDeviceHandler::handle_received_dad_packet(
                core_ctx,
                bindings_ctx,
                &device_id,
                target_address.into_specified(),
                nonce,
            ) {
                Some(IpAddressState::Assigned) => {
                    // A neighbor is advertising that it owns an address
                    // that we also have assigned. This is out of scope
                    // for DAD.
                    //
                    // As per RFC 4862 section 5.4.4,
                    //
                    //   2.  If the target address matches a unicast address
                    //       assigned to the receiving interface, it would
                    //       possibly indicate that the address is a
                    //       duplicate but it has not been detected by the
                    //       Duplicate Address Detection procedure (recall
                    //       that Duplicate Address Detection is not
                    //       completely reliable). How to handle such a case
                    //       is beyond the scope of this document.
                    //
                    // TODO(https://fxbug.dev/42111744): Signal to bindings
                    // that a duplicate address is detected.
                    error!(
                        "NA from {src_ip} with target address {target_address} that is also \
                        assigned on device {device_id:?}",
                    );
                }
                Some(IpAddressState::Tentative) => {
                    // Nothing further to do for an NA from a neighbor that
                    // targets an address we also have assigned.
                    return;
                }
                Some(IpAddressState::Unavailable) | None => {
                    // Address not targeting us so we know its for a neighbor.
                    //
                    // TODO(https://fxbug.dev/42182317): Move NUD to IP.
                }
            }

            let link_addr = p.body().iter().find_map(|o| o.target_link_layer_address());

            NudIpHandler::handle_neighbor_confirmation(
                core_ctx,
                bindings_ctx,
                &device_id,
                target_address.into_specified(),
                link_addr,
                ConfirmationFlags {
                    solicited_flag: p.message().solicited_flag(),
                    override_flag: p.message().override_flag(),
                },
            );
        }
        NdpPacket::RouterAdvertisement(ref p) => {
            // As per RFC 4861 section 6.1.2,
            //
            //   A node MUST silently discard any received Router Advertisement
            //   messages that do not satisfy all of the following validity
            //   checks:
            //
            //      - IP Source Address is a link-local address.  Routers must
            //        use their link-local address as the source for Router
            //        Advertisement and Redirect messages so that hosts can
            //        uniquely identify routers.
            //
            //        ...
            let src_ip = match src_ip {
                Ipv6SourceAddr::Unicast(ip) => match LinkLocalUnicastAddr::new(*ip) {
                    Some(ip) => ip,
                    None => return,
                },
                Ipv6SourceAddr::Unspecified => return,
            };

            let ra = p.message();
            debug!("received router advertisement from {:?}: {:?}", src_ip, ra);
            core_ctx.counters().rx.router_advertisement.increment();

            // As per RFC 4861 section 6.3.4,
            //   The RetransTimer variable SHOULD be copied from the Retrans
            //   Timer field, if it is specified.
            //
            // TODO(https://fxbug.dev/42052173): Control whether or not we should
            // update the retransmit timer.
            if let Some(retransmit_timer) = ra.retransmit_timer() {
                Ipv6DeviceHandler::set_discovered_retrans_timer(
                    core_ctx,
                    bindings_ctx,
                    &device_id,
                    retransmit_timer,
                );
            }

            // As per RFC 4861 section 6.3.4:
            //   If the received Cur Hop Limit value is specified, the host
            //   SHOULD set its CurHopLimit variable to the received value.
            //
            // TODO(https://fxbug.dev/42052173): Control whether or not we should
            // update the default hop limit.
            if let Some(hop_limit) = ra.current_hop_limit() {
                trace!(
                    "receive_ndp_packet: NDP RA: updating device's hop limit to {:?} for router: {:?}",
                    ra.current_hop_limit(),
                    src_ip
                );
                IpDeviceHandler::set_default_hop_limit(core_ctx, &device_id, hop_limit);
            }

            Ipv6DeviceHandler::update_discovered_ipv6_route(
                core_ctx,
                bindings_ctx,
                &device_id,
                Ipv6DiscoveredRoute { subnet: IPV6_DEFAULT_SUBNET, gateway: Some(src_ip) },
                Ipv6DiscoveredRouteProperties { route_preference: p.message().preference().into() },
                p.message().router_lifetime().map(NonZeroNdpLifetime::Finite),
            );

            for option in p.body().iter() {
                match option {
                    NdpOption::TargetLinkLayerAddress(_)
                    | NdpOption::RedirectedHeader { .. }
                    | NdpOption::RecursiveDnsServer(_)
                    | NdpOption::Nonce(_) => {}
                    NdpOption::SourceLinkLayerAddress(addr) => {
                        debug!("processing SourceLinkLayerAddress option in RA: {:?}", addr);
                        // As per RFC 4861 section 6.3.4,
                        //
                        //   If the advertisement contains a Source Link-Layer
                        //   Address option, the link-layer address SHOULD be
                        //   recorded in the Neighbor Cache entry for the router
                        //   (creating an entry if necessary) and the IsRouter
                        //   flag in the Neighbor Cache entry MUST be set to
                        //   TRUE. If no Source Link-Layer Address is included,
                        //   but a corresponding Neighbor Cache entry exists,
                        //   its IsRouter flag MUST be set to TRUE. The IsRouter
                        //   flag is used by Neighbor Unreachability Detection
                        //   to determine when a router changes to being a host
                        //   (i.e., no longer capable of forwarding packets).
                        //   If a Neighbor Cache entry is created for the
                        //   router, its reachability state MUST be set to STALE
                        //   as specified in Section 7.3.3.  If a cache entry
                        //   already exists and is updated with a different
                        //   link-layer address, the reachability state MUST
                        //   also be set to STALE.if a Neighbor Cache entry
                        //
                        // We do not yet support NUD as described in RFC 4861
                        // so for now we just record the link-layer address in
                        // our neighbor table.
                        //
                        // TODO(https://fxbug.dev/42083367): Add support for routers in NUD.
                        NudIpHandler::handle_neighbor_probe(
                            core_ctx,
                            bindings_ctx,
                            &device_id,
                            {
                                let src_ip: UnicastAddr<_> = src_ip.into_addr();
                                src_ip.into_specified()
                            },
                            addr,
                        );
                    }
                    NdpOption::PrefixInformation(prefix_info) => {
                        debug!("processing Prefix Information option in RA: {:?}", prefix_info);
                        // As per RFC 4861 section 6.3.4,
                        //
                        //   For each Prefix Information option with the on-link
                        //   flag set, a host does the following:
                        //
                        //      - If the prefix is the link-local prefix,
                        //        silently ignore the Prefix Information option.
                        //
                        // Also as per RFC 4862 section 5.5.3,
                        //
                        //   For each Prefix-Information option in the Router
                        //   Advertisement:
                        //
                        //    ..
                        //
                        //    b)  If the prefix is the link-local prefix,
                        //        silently ignore the Prefix Information option.
                        if prefix_info.prefix().is_link_local() {
                            continue;
                        }

                        let subnet = match prefix_info.subnet() {
                            Ok(subnet) => subnet,
                            Err(err) => match err {
                                SubnetError::PrefixTooLong | SubnetError::HostBitsSet => continue,
                            },
                        };

                        match UnicastAddr::new(subnet.network()) {
                            Some(UnicastAddr { .. }) => {}
                            None => continue,
                        }

                        let valid_lifetime = prefix_info.valid_lifetime();

                        if prefix_info.on_link_flag() {
                            Ipv6DeviceHandler::update_discovered_ipv6_route(
                                core_ctx,
                                bindings_ctx,
                                &device_id,
                                Ipv6DiscoveredRoute { subnet, gateway: None },
                                Ipv6DiscoveredRouteProperties {
                                    route_preference: RoutePreference::Medium,
                                },
                                valid_lifetime,
                            )
                        }

                        if prefix_info.autonomous_address_configuration_flag() {
                            Ipv6DeviceHandler::apply_slaac_update(
                                core_ctx,
                                bindings_ctx,
                                &device_id,
                                subnet,
                                prefix_info.preferred_lifetime(),
                                valid_lifetime,
                            );
                        }
                    }
                    NdpOption::RouteInformation(rio) => {
                        debug!("processing Route Information option in RA: {:?}", rio);
                        Ipv6DeviceHandler::update_discovered_ipv6_route(
                            core_ctx,
                            bindings_ctx,
                            &device_id,
                            Ipv6DiscoveredRoute {
                                subnet: rio.prefix().clone(),
                                gateway: Some(src_ip),
                            },
                            Ipv6DiscoveredRouteProperties {
                                route_preference: rio.preference().into(),
                            },
                            rio.route_lifetime(),
                        )
                    }
                    NdpOption::Mtu(mtu) => {
                        debug!("processing MTU option in RA: {:?}", mtu);
                        // TODO(https://fxbug.dev/42052173): Control whether or
                        // not we should update the link's MTU in response to
                        // RAs.
                        Ipv6DeviceHandler::set_link_mtu(core_ctx, &device_id, Mtu::new(mtu));
                    }
                }
            }

            bindings_ctx.on_event(RouterAdvertisementEvent {
                options_bytes: Box::from(p.body().bytes()),
                source: **src_ip,
                device: device_id.clone(),
            });
        }
    }
}

impl<
    BC: IcmpBindingsContext + NdpBindingsContext<CC::DeviceId>,
    CC: InnerIcmpContext<Ipv6, BC>
        + InnerIcmpContext<Ipv6, BC>
        + Ipv6DeviceHandler<BC>
        + IpDeviceHandler<Ipv6, BC>
        + IpDeviceIngressStateContext<Ipv6>
        + PmtuHandler<Ipv6, BC>
        + NudIpHandler<Ipv6, BC>
        + IpLayerHandler<Ipv6, BC>
        + CounterContext<IcmpRxCounters<Ipv6>>
        + CounterContext<IcmpTxCounters<Ipv6>>
        + CounterContext<NdpCounters>,
> IpTransportContext<Ipv6, BC, CC> for IcmpIpTransportContext
{
    type EarlyDemuxSocket = Never;

    fn early_demux<B: ParseBuffer>(
        _core_ctx: &mut CC,
        _device: &CC::DeviceId,
        _src_ip: Ipv6Addr,
        _dst_ip: Ipv6Addr,
        _buffer: B,
    ) -> Option<Self::EarlyDemuxSocket> {
        None
    }

    fn receive_icmp_error(
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        device: &CC::DeviceId,
        original_src_ip: Option<SpecifiedAddr<Ipv6Addr>>,
        original_dst_ip: SpecifiedAddr<Ipv6Addr>,
        original_body: &[u8],
        err: Icmpv6ErrorCode,
    ) {
        receive_ip_transport_icmp_error(
            core_ctx,
            bindings_ctx,
            device,
            original_src_ip,
            original_dst_ip,
            original_body,
            err,
        )
    }

    fn receive_ip_packet<B: BufferMut, H: IpHeaderInfo<Ipv6>>(
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        device: &CC::DeviceId,
        src_ip: Ipv6SourceAddr,
        dst_ip: SpecifiedAddr<Ipv6Addr>,
        mut buffer: B,
        info: &mut LocalDeliveryPacketInfo<Ipv6, H>,
        _early_demux_socket: Option<Never>,
    ) -> Result<(), (B, Icmpv6Error)> {
        let LocalDeliveryPacketInfo { meta, header_info, marks } = info;
        let ReceiveIpPacketMeta { broadcast: _, transparent_override, parsing_context: _ } = meta;
        if let Some(delivery) = transparent_override {
            unreachable!(
                "cannot perform transparent local delivery {delivery:?} to an ICMP socket; \
                transparent proxy rules can only be configured for TCP and UDP packets"
            );
        }

        trace!(
            "<IcmpIpTransportContext as IpTransportContext<Ipv6>>::receive_ip_packet({:?}, {})",
            src_ip, dst_ip
        );

        let packet = match buffer
            .parse_with::<_, Icmpv6Packet<_>>(IcmpParseArgs::new(src_ip.get(), dst_ip))
        {
            Ok(packet) => packet,
            Err(_) => return Ok(()), // TODO(joshlf): Do something else here?
        };

        match packet {
            Icmpv6Packet::EchoRequest(echo_request) => {
                CounterContext::<IcmpRxCounters<Ipv6>>::counters(core_ctx).echo_request.increment();

                if let Some(src_ip) = SocketIpAddr::new_from_ipv6_source(src_ip) {
                    match SocketIpAddr::try_from(dst_ip) {
                        Ok(dst_ip) => {
                            let req = *echo_request.message();
                            let code = echo_request.code();
                            let (local_ip, remote_ip) = (dst_ip, src_ip);
                            debug!(
                                "replying to ICMP echo request from {remote_ip}: id={}, seq={}",
                                req.id(),
                                req.seq()
                            );
                            send_icmp_reply(
                                core_ctx,
                                bindings_ctx,
                                device,
                                remote_ip,
                                local_ip,
                                |src_ip| {
                                    IcmpPacketBuilder::<Ipv6, _>::new(
                                        src_ip,
                                        remote_ip.addr(),
                                        code,
                                        req.reply(),
                                    )
                                    .wrap_body(buffer)
                                },
                                &WithMarks(marks),
                            );
                        }
                        Err(AddrIsMappedError {}) => {
                            trace!(
                                "IpTransportContext<Ipv6>::receive_ip_packet: Received echo request with an ipv4-mapped-ipv6 destination address"
                            );
                        }
                    }
                } else {
                    trace!(
                        "<IcmpIpTransportContext as IpTransportContext<Ipv6>>::receive_ip_packet: Received echo request with an unspecified source address"
                    );
                }
            }
            Icmpv6Packet::EchoReply(echo_reply) => {
                CounterContext::<IcmpRxCounters<Ipv6>>::counters(core_ctx).echo_reply.increment();
                trace!(
                    "<IcmpIpTransportContext as IpTransportContext<Ipv6>>::receive_ip_packet: Received an EchoReply message"
                );
                let parse_metadata = echo_reply.parse_metadata();
                buffer.undo_parse(parse_metadata);
                return <CC::EchoTransportContext
                            as IpTransportContext<Ipv6, BC, CC>>::receive_ip_packet(
                        core_ctx,
                        bindings_ctx,
                        device,
                        src_ip,
                        dst_ip,
                        buffer,
                        info,
                        None
                );
            }
            Icmpv6Packet::Ndp(packet) => receive_ndp_packet(
                core_ctx,
                bindings_ctx,
                device,
                src_ip,
                dst_ip,
                packet,
                header_info,
            ),
            Icmpv6Packet::PacketTooBig(packet_too_big) => {
                CounterContext::<IcmpRxCounters<Ipv6>>::counters(core_ctx)
                    .packet_too_big
                    .increment();
                trace!(
                    "<IcmpIpTransportContext as IpTransportContext<Ipv6>>::receive_ip_packet: Received a Packet Too Big message"
                );
                if let Ipv6SourceAddr::Unicast(src_ip) = src_ip {
                    // We are updating the path MTU from the destination address
                    // of this `packet` (which is an IP address on this node) to
                    // some remote (identified by the source address of this
                    // `packet`).
                    //
                    // `update_pmtu_if_less` will only update the PMTU if the
                    // Packet Too Big message's MTU field had a value that was
                    // at least the IPv6 minimum MTU (which is required by IPv6
                    // RFC 8200).
                    let mtu = core_ctx.update_pmtu_if_less(
                        bindings_ctx,
                        dst_ip.get(),
                        src_ip.get(),
                        Mtu::new(packet_too_big.message().mtu()),
                    );
                    if let Some(mtu) = mtu {
                        receive_icmpv6_error(
                            core_ctx,
                            bindings_ctx,
                            device,
                            &packet_too_big,
                            Icmpv6ErrorCode::PacketTooBig(mtu),
                        );
                    }
                }
            }
            Icmpv6Packet::Mld(packet) => {
                core_ctx.receive_mld_packet(
                    bindings_ctx,
                    &device,
                    src_ip,
                    dst_ip,
                    packet,
                    header_info,
                );
            }
            Icmpv6Packet::DestUnreachable(dest_unreachable) => {
                CounterContext::<IcmpRxCounters<Ipv6>>::counters(core_ctx)
                    .dest_unreachable
                    .increment_code(dest_unreachable.code());
                receive_icmpv6_error(
                    core_ctx,
                    bindings_ctx,
                    device,
                    &dest_unreachable,
                    Icmpv6ErrorCode::DestUnreachable(dest_unreachable.code()),
                )
            }
            Icmpv6Packet::TimeExceeded(time_exceeded) => {
                CounterContext::<IcmpRxCounters<Ipv6>>::counters(core_ctx)
                    .time_exceeded
                    .increment_code(time_exceeded.code());
                receive_icmpv6_error(
                    core_ctx,
                    bindings_ctx,
                    device,
                    &time_exceeded,
                    Icmpv6ErrorCode::TimeExceeded(time_exceeded.code()),
                )
            }
            Icmpv6Packet::ParameterProblem(parameter_problem) => {
                CounterContext::<IcmpRxCounters<Ipv6>>::counters(core_ctx)
                    .parameter_problem
                    .increment_code(parameter_problem.code());
                receive_icmpv6_error(
                    core_ctx,
                    bindings_ctx,
                    device,
                    &parameter_problem,
                    Icmpv6ErrorCode::ParameterProblem(parameter_problem.code()),
                )
            }
        }

        Ok(())
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
struct WithMarks<'a>(&'a Marks);

impl<'a> OptionDelegationMarker for WithMarks<'a> {}

impl<'a, I: IpExt> DelegatedRouteResolutionOptions<I> for WithMarks<'a> {
    fn marks(&self) -> &Marks {
        let Self(marks) = self;
        marks
    }
}

impl<'a, I: IpExt> DelegatedSendOptions<I> for WithMarks<'a> {}

/// Sends an ICMP reply to a remote host.
///
/// `send_icmp_reply` sends a reply to a non-error message (e.g., "echo request"
/// or "timestamp request" messages).
///
/// `get_body_from_src_ip` returns a `Serializer` with the bytes of the ICMP
/// packet, and, when called, is given the source IP address chosen for the
/// outbound packet. This allows `get_body_from_src_ip` to properly compute the
/// ICMP checksum, which relies on both the source and destination IP addresses
/// of the IP packet it's encapsulated in.
fn send_icmp_reply<I, BC, CC, S, F, O>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device: &CC::DeviceId,
    original_src_ip: SocketIpAddr<I::Addr>,
    original_dst_ip: SocketIpAddr<I::Addr>,
    get_body_from_src_ip: F,
    ip_options: &O,
) where
    I: IpExt + FilterIpExt + IcmpCountersIpExt,
    CC: IpSocketHandler<I, BC> + DeviceIdContext<AnyDevice> + CounterContext<IcmpTxCounters<I>>,
    BC: TxMetadataBindingsTypes,
    S: DynamicTransportSerializer<I>,
    F: FnOnce(SpecifiedAddr<I::Addr>) -> S,
    O: SendOptions<I> + RouteResolutionOptions<I>,
{
    trace!("send_icmp_reply({:?}, {}, {})", device, original_src_ip, original_dst_ip);
    core_ctx.counters().reply.increment();
    let tx_metadata: BC::TxMetadata = Default::default();

    // Force the egress device if the original destination is multicast or
    // requires a zone (i.e. link-local non-loopback), ensuring we pick the
    // correct return route.
    let egress_device = (original_dst_ip.as_ref().is_multicast()
        || original_dst_ip.as_ref().must_have_zone())
    .then_some(EitherDeviceId::Strong(device));

    core_ctx
        .send_oneshot_ip_packet_with_dyn_serializer(
            bindings_ctx,
            IpSocketArgs {
                device: egress_device,
                local_ip: IpDeviceAddr::new_from_socket_ip_addr(original_dst_ip),
                remote_ip: original_src_ip,
                proto: I::ICMP_IP_PROTO,
                options: ip_options,
            },
            tx_metadata,
            |src_ip| get_body_from_src_ip(src_ip.into()),
        )
        .unwrap_or_else(|err| {
            debug!("failed to send ICMP reply: {}", err);
        })
}

/// Receive an ICMP(v4) error message.
///
/// `receive_icmpv4_error` handles an incoming ICMP error message by parsing the
/// original IPv4 packet and then delegating to the context.
fn receive_icmpv4_error<
    BC: IcmpBindingsContext,
    CC: InnerIcmpContext<Ipv4, BC>,
    B: SplitByteSlice,
    M: IcmpMessage<Ipv4, Body<B> = OriginalPacket<B>>,
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device: &CC::DeviceId,
    packet: &IcmpPacket<Ipv4, B, M>,
    err: Icmpv4ErrorCode,
) {
    packet.with_original_packet(|res| match res {
        Ok(original_packet) => {
            let dst_ip = match SpecifiedAddr::new(original_packet.dst_ip()) {
                Some(ip) => ip,
                None => {
                    trace!("receive_icmpv4_error: Got ICMP error message whose original IPv4 packet contains an unspecified destination address; discarding");
                    return;
                },
            };
            InnerIcmpContext::receive_icmp_error(
                core_ctx,
                bindings_ctx,
                device,
                SpecifiedAddr::new(original_packet.src_ip()),
                dst_ip,
                original_packet.proto(),
                original_packet.body().into_inner(),
                err,
            );
        }
        Err(_) => debug!(
            "receive_icmpv4_error: Got ICMP error message with unparsable original IPv4 packet"
        ),
    })
}

/// Receive an ICMPv6 error message.
///
/// `receive_icmpv6_error` handles an incoming ICMPv6 error message by parsing
/// the original IPv6 packet and then delegating to the context.
fn receive_icmpv6_error<
    BC: IcmpBindingsContext,
    CC: InnerIcmpContext<Ipv6, BC>,
    B: SplitByteSlice,
    M: IcmpMessage<Ipv6, Body<B> = OriginalPacket<B>>,
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device: &CC::DeviceId,
    packet: &IcmpPacket<Ipv6, B, M>,
    err: Icmpv6ErrorCode,
) {
    packet.with_original_packet(|res| match res {
        Ok(original_packet) => {
            let dst_ip = match SpecifiedAddr::new(original_packet.dst_ip()) {
                Some(ip)=>ip,
                None => {
                    trace!("receive_icmpv6_error: Got ICMP error message whose original IPv6 packet contains an unspecified destination address; discarding");
                    return;
                },
            };
            match original_packet.body_proto() {
                Ok((body, proto)) => {
                    InnerIcmpContext::receive_icmp_error(
                        core_ctx,
                        bindings_ctx,
                        device,
                        SpecifiedAddr::new(original_packet.src_ip()),
                        dst_ip,
                        proto,
                        body.into_inner(),
                        err,
                    );
                }
                Err(ExtHdrParseError) => {
                    trace!("receive_icmpv6_error: We could not parse the original packet's extension headers, and so we don't know where the original packet's body begins; discarding");
                    // There's nothing we can do in this case, so we just
                    // return.
                    return;
                }
            }
        }
        Err(_body) => debug!(
            "receive_icmpv6_error: Got ICMPv6 error message with unparsable original IPv6 packet"
        ),
    })
}

fn send_icmpv4_error_message<B, BC, CC>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device: Option<&CC::DeviceId>,
    frame_dst: Option<LocalFrameDestination>,
    original_src_ip: SocketIpAddr<Ipv4Addr>,
    original_dst_ip: SocketIpAddr<Ipv4Addr>,
    message: Icmpv4ErrorMessage,
    mut original_packet: B,
    header_len: usize,
    marks: &Marks,
) where
    B: BufferMut,
    BC: IcmpBindingsContext,
    CC: IcmpSendContext<Ipv4, BC> + CounterContext<IcmpTxCounters<Ipv4>>,
{
    // TODO(https://fxbug.dev/42177876): Come up with rules for when to send ICMP
    // error messages.

    if !should_send_icmpv4_error(frame_dst, original_src_ip.into(), original_dst_ip.into()) {
        return;
    }

    // Per RFC 792, body contains entire IPv4 header + 64 bytes of original
    // body.
    original_packet.shrink_back_to(header_len + 64);

    let tx_metadata: BC::TxMetadata = Default::default();

    macro_rules! send {
        ($message:expr, $code:expr) => {{
            // TODO(https://fxbug.dev/42177877): Improve source address selection for ICMP
            // errors sent from unnumbered/router interfaces.
            let _ = try_send_error!(
                core_ctx,
                bindings_ctx,
                core_ctx.send_oneshot_ip_packet_with_dyn_serializer(
                    bindings_ctx,
                    IpSocketArgs {
                        device: device.map(EitherDeviceId::Strong),
                        local_ip: None,
                        remote_ip: original_src_ip,
                        proto: Ipv4Proto::Icmp,
                        options: &WithMarks(marks),
                    },
                    tx_metadata,
                    |local_ip| {
                        IcmpPacketBuilder::<Ipv4, _>::new(
                            local_ip.addr(),
                            original_src_ip.addr(),
                            $code,
                            $message,
                        )
                        .wrap_body(original_packet)
                    },
                )
            );
        }};
    }

    match message {
        Icmpv4ErrorMessage::TimeExceeded { message, code } => send!(message, code),
        Icmpv4ErrorMessage::ParameterProblem { message, code } => send!(message, code),
        Icmpv4ErrorMessage::DestUnreachable { message, code } => send!(message, code),
    }
}

fn send_icmpv6_error_message<B, BC, CC>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device: Option<&CC::DeviceId>,
    frame_dst: Option<LocalFrameDestination>,
    original_src_ip: SocketIpAddr<Ipv6Addr>,
    original_dst_ip: SocketIpAddr<Ipv6Addr>,
    message: Icmpv6ErrorMessage,
    original_packet: B,
    allow_dst_multicast: bool,
    marks: &Marks,
) where
    B: BufferMut,
    BC: IcmpBindingsContext,
    CC: IcmpSendContext<Ipv6, BC> + CounterContext<IcmpTxCounters<Ipv6>>,
{
    // TODO(https://fxbug.dev/42177876): Come up with rules for when to send ICMP
    // error messages.

    if !should_send_icmpv6_error(
        frame_dst,
        original_src_ip.into(),
        original_dst_ip.into(),
        allow_dst_multicast,
    ) {
        return;
    }

    struct Icmpv6ErrorOptions<'a>(&'a Marks);
    impl<'a> OptionDelegationMarker for Icmpv6ErrorOptions<'a> {}
    impl<'a> DelegatedSendOptions<Ipv6> for Icmpv6ErrorOptions<'a> {
        fn mtu(&self) -> Mtu {
            Ipv6::MINIMUM_LINK_MTU
        }
    }
    impl<'a> DelegatedRouteResolutionOptions<Ipv6> for Icmpv6ErrorOptions<'a> {
        fn marks(&self) -> &Marks {
            let Self(marks) = self;
            marks
        }
    }

    let tx_metadata: BC::TxMetadata = Default::default();

    macro_rules! send {
        ($message:expr, $code:expr) => {{
            // TODO(https://fxbug.dev/42177877): Improve source address selection for ICMP
            // errors sent from unnumbered/router interfaces.
            let _ = try_send_error!(
                core_ctx,
                bindings_ctx,
                core_ctx.send_oneshot_ip_packet_with_dyn_serializer(
                    bindings_ctx,
                    IpSocketArgs {
                        device: device.map(EitherDeviceId::Strong),
                        local_ip: None,
                        remote_ip: original_src_ip,
                        proto: Ipv6Proto::Icmpv6,
                        options: &Icmpv6ErrorOptions(marks),
                    },
                    tx_metadata,
                    |local_ip| {
                        let icmp_builder = IcmpPacketBuilder::<Ipv6, _>::new(
                            local_ip.addr(),
                            original_src_ip.addr(),
                            $code,
                            $message,
                        );

                        // Per RFC 4443, body contains as much of the original body as
                        // possible without exceeding IPv6 minimum MTU.
                        icmp_builder.wrap_body(TruncatingSerializer::new(
                            original_packet,
                            TruncateDirection::DiscardBack,
                        ))
                    },
                )
            );
        }};
    }

    match message {
        Icmpv6ErrorMessage::TimeExceeded { message, code } => send!(message, code),
        Icmpv6ErrorMessage::PacketTooBig { message, code } => send!(message, code),
        Icmpv6ErrorMessage::ParameterProblem { message, code } => send!(message, code),
        Icmpv6ErrorMessage::DestUnreachable { message, code } => send!(message, code),
    }
}

/// Should we send an ICMP(v4) response?
///
/// `should_send_icmpv4_error` implements the logic described in RFC 1122
/// Section 3.2.2. It decides whether, upon receiving an incoming packet with
/// the given parameters, we should send an ICMP response or not. In particular,
/// we do not send an ICMP response if we've received:
/// - a packet destined to a broadcast or multicast address
/// - a packet sent in a link-layer broadcast
/// - a non-initial fragment
/// - a packet whose source address does not define a single host (a
///   zero/unspecified address, a broadcast address, a multicast address, or a
///   Class E address)
///
/// RFC Non-Compliance: RFC 1122 Section 3.2.2 also considers the loopback
/// address to be one that "does not define a single host". However, that breaks
/// error delivery for loopback sockets. This deviation matches the behavior of
/// Netstack2 and Linux.
///
/// Note that `should_send_icmpv4_error` does NOT check whether the incoming
/// packet contained an ICMP error message. This is because that check is
/// unnecessary for some ICMP error conditions. The ICMP error message check can
/// be performed separately with `is_icmp_error_message`.
fn should_send_icmpv4_error(
    frame_dst: Option<LocalFrameDestination>,
    src_ip: SpecifiedAddr<Ipv4Addr>,
    dst_ip: SpecifiedAddr<Ipv4Addr>,
) -> bool {
    // NOTE: We do not explicitly implement the "unspecified address" check, as
    // it is enforced by the types of the arguments.

    // TODO(joshlf): Implement the rest of the rules:
    // - a packet destined to a subnet broadcast address
    // - a packet whose source address is a subnet broadcast address

    // NOTE: The FrameDestination type has variants for unicast, multicast, and
    // broadcast. One implication of the fact that we only check for broadcast
    // here (in compliance with the RFC) is that we could, in one very unlikely
    // edge case, respond with an ICMP error message to an IP packet which was
    // sent in a link-layer multicast frame. In particular, that can happen if
    // we subscribe to a multicast IP group and, as a result, subscribe to the
    // corresponding multicast MAC address, and we receive a unicast IP packet
    // in a multicast link-layer frame destined to that MAC address.
    //
    // TODO(joshlf): Should we filter incoming multicast IP traffic to make sure
    // that it matches the multicast MAC address of the frame it was
    // encapsulated in?
    !(dst_ip.is_multicast()
        || dst_ip.is_limited_broadcast()
        || frame_dst.is_some_and(|dst| dst.is_broadcast())
        || src_ip.is_limited_broadcast()
        || src_ip.is_multicast()
        || src_ip.is_class_e())
}

/// Should we send an ICMPv6 response?
///
/// `should_send_icmpv6_error` implements the logic described in RFC 4443
/// Section 2.4.e. It decides whether, upon receiving an incoming packet with
/// the given parameters, we should send an ICMP response or not. In particular,
/// we do not send an ICMP response if we've received:
/// - a packet destined to a multicast address
///   - Two exceptions to this rules:
///     1) the Packet Too Big Message to allow Path MTU discovery to work for
///        IPv6 multicast
///     2) the Parameter Problem Message, Code 2 reporting an unrecognized IPv6
///        option that has the Option Type highest-order two bits set to 10
/// - a packet sent as a link-layer multicast or broadcast
///   - same exceptions apply here as well.
/// - a packet whose source address does not define a single host (a
///   zero/unspecified address, or a multicast address)
///
/// RFC Non-Compliance: We send ICMP errors over loopback. See the comment on
/// [`should_send_icmpv4_error`] for more information.
///
/// If an ICMP response will be a Packet Too Big Message or a Parameter Problem
/// Message, Code 2 reporting an unrecognized IPv6 option that has the Option
/// Type highest-order two bits set to 10, `info.allow_dst_multicast` must be
/// set to `true` so this function will allow the exception mentioned above.
///
/// Note that `should_send_icmpv6_error` does NOT check whether the incoming
/// packet contained an ICMP error message. This is because that check is
/// unnecessary for some ICMP error conditions. The ICMP error message check can
/// be performed separately with `is_icmp_error_message`.
fn should_send_icmpv6_error(
    frame_dst: Option<LocalFrameDestination>,
    src_ip: SpecifiedAddr<Ipv6Addr>,
    dst_ip: SpecifiedAddr<Ipv6Addr>,
    allow_dst_multicast: bool,
) -> bool {
    // NOTE: We do not explicitly implement the "unspecified address" check, as
    // it is enforced by the types of the arguments.
    let multicast_frame_dst = match frame_dst {
        Some(FrameDestination::Individual { local: () }) | None => false,
        Some(FrameDestination::Broadcast) | Some(FrameDestination::Multicast) => true,
    };
    if (dst_ip.is_multicast() || multicast_frame_dst) && !allow_dst_multicast {
        return false;
    }
    if src_ip.is_multicast() {
        return false;
    }
    true
}

/// Determine whether or not an IP packet body contains an ICMP error message
/// for the purposes of determining whether or not to send an ICMP response.
///
/// `is_icmp_error_or_redirect_message` checks whether `proto` is ICMP(v4) for
/// IPv4 or ICMPv6 for IPv6 and, if so, attempts to parse `buf` as an ICMP
/// packet in order to determine whether it is an error message or not. If
/// parsing fails, it conservatively assumes that it is an error packet in
/// order to avoid violating the MUST NOT directives of RFC 1122 Section 3.2.2
/// and [RFC 4443 Section 2.4.e].
///
/// [RFC 4443 Section 2.4.e]: https://tools.ietf.org/html/rfc4443#section-2.4
fn is_icmp_error_or_redirect_message<I: IcmpIpExt>(proto: I::Proto, buf: &[u8]) -> bool {
    proto == I::ICMP_IP_PROTO
        && peek_message_type::<I::IcmpMessageType>(buf)
            .map(IcmpMessageType::is_error_or_redirect)
            .unwrap_or(true)
}

/// Test utilities for ICMP.
#[cfg(any(test, feature = "testutils"))]
pub(crate) mod testutil {
    use alloc::vec::Vec;
    use net_types::ethernet::Mac;
    use net_types::ip::{Ipv6, Ipv6Addr};
    use netstack3_base::NetworkSerializationContext;
    use packet::{Buf, InnerPacketBuilder as _, NestableSerializer as _, Serializer as _};
    use packet_formats::icmp::ndp::options::NdpOptionBuilder;
    use packet_formats::icmp::ndp::{
        NeighborAdvertisement, NeighborSolicitation, OptionSequenceBuilder,
    };
    use packet_formats::icmp::{IcmpPacketBuilder, IcmpZeroCode};
    use packet_formats::ip::Ipv6Proto;
    use packet_formats::ipv6::Ipv6PacketBuilder;

    use super::REQUIRED_NDP_IP_PACKET_HOP_LIMIT;

    /// Serialize an IP packet containing a neighbor advertisement with the
    /// provided parameters.
    pub fn neighbor_advertisement_ip_packet(
        src_ip: Ipv6Addr,
        dst_ip: Ipv6Addr,
        router_flag: bool,
        solicited_flag: bool,
        override_flag: bool,
        mac: Mac,
    ) -> Buf<Vec<u8>> {
        OptionSequenceBuilder::new([NdpOptionBuilder::TargetLinkLayerAddress(&mac.bytes())].iter())
            .into_serializer()
            .wrap_in(IcmpPacketBuilder::<Ipv6, _>::new(
                src_ip,
                dst_ip,
                IcmpZeroCode,
                NeighborAdvertisement::new(router_flag, solicited_flag, override_flag, src_ip),
            ))
            .wrap_in(Ipv6PacketBuilder::new(
                src_ip,
                dst_ip,
                REQUIRED_NDP_IP_PACKET_HOP_LIMIT,
                Ipv6Proto::Icmpv6,
            ))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .unwrap_b()
    }

    /// Serialize an IP packet containing a neighbor solicitation with the
    /// provided parameters.
    pub fn neighbor_solicitation_ip_packet(
        src_ip: Ipv6Addr,
        dst_ip: Ipv6Addr,
        target_addr: Ipv6Addr,
        mac: Mac,
    ) -> Buf<Vec<u8>> {
        OptionSequenceBuilder::new([NdpOptionBuilder::SourceLinkLayerAddress(&mac.bytes())].iter())
            .into_serializer()
            .wrap_in(IcmpPacketBuilder::<Ipv6, _>::new(
                src_ip,
                dst_ip,
                IcmpZeroCode,
                NeighborSolicitation::new(target_addr),
            ))
            .wrap_in(Ipv6PacketBuilder::new(
                src_ip,
                dst_ip,
                REQUIRED_NDP_IP_PACKET_HOP_LIMIT,
                Ipv6Proto::Icmpv6,
            ))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .unwrap_b()
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;
    use packet_formats::icmp::ndp::options::NdpNonce;

    use core::fmt::Debug;
    use core::time::Duration;

    use net_types::ip::Subnet;
    use netstack3_base::testutil::{
        FakeBindingsCtx, FakeCoreCtx, FakeDeviceId, FakeInstant, FakeTxMetadata, FakeWeakDeviceId,
        TEST_ADDRS_V4, TEST_ADDRS_V6, TestIpExt, set_logger_for_test,
    };
    use netstack3_base::{CtxPair, NetworkSerializationContext, Uninstantiable};
    use netstack3_filter::TransportPacketSerializer;
    use packet::{Buf, EmptyBuf, NestableSerializer as _, Serializer as _};
    use packet_formats::icmp::mld::MldPacket;
    use packet_formats::ip::IpProto;
    use packet_formats::utils::NonZeroDuration;

    use super::*;
    use crate::internal::base::{IpDeviceEgressStateContext, RouterAdvertisementEvent};
    use crate::internal::socket::testutil::{FakeDeviceConfig, FakeIpSocketCtx};
    use crate::internal::socket::{
        IpSock, IpSockCreationError, IpSockSendError, IpSocketHandler, SendOptions,
    };
    use crate::socket::RouteResolutionOptions;

    use test_util::assert_geq;

    pub(super) trait IcmpTestIpExt:
        TestIpExt + IpExt + FilterIpExt + IcmpCountersIpExt
    {
    }
    impl<I: TestIpExt + IpExt + FilterIpExt + IcmpCountersIpExt> IcmpTestIpExt for I {}

    /// The FakeCoreCtx held as the inner state of [`FakeIcmpCoreCtx`].
    type InnerIpSocketCtx<I> = FakeCoreCtx<
        FakeIpSocketCtx<I, FakeDeviceId>,
        SendIpPacketMeta<I, FakeDeviceId, SpecifiedAddr<<I as Ip>::Addr>>,
        FakeDeviceId,
    >;

    /// `FakeCoreCtx` specialized for ICMP.
    pub(super) struct FakeIcmpCoreCtx<I: IcmpTestIpExt> {
        ip_socket_ctx: InnerIpSocketCtx<I>,
        icmp: FakeIcmpCoreCtxState<I>,
    }

    /// `FakeBindingsCtx` specialized for ICMP.
    type FakeIcmpBindingsCtx<I> = FakeBindingsCtx<
        (),
        RouterAdvertisementEvent<FakeDeviceId>,
        FakeIcmpBindingsCtxState<I>,
        (),
    >;

    /// A fake ICMP bindings and core contexts.
    ///
    /// This is exposed to super so it can be shared with the socket tests.
    pub(super) type FakeIcmpCtx<I> = CtxPair<FakeIcmpCoreCtx<I>, FakeIcmpBindingsCtx<I>>;

    pub(super) struct FakeIcmpCoreCtxState<I: IcmpTestIpExt> {
        error_send_bucket: TokenBucket<FakeInstant>,
        receive_icmp_error: Vec<I::ErrorCode>,
        rx_counters: IcmpRxCounters<I>,
        tx_counters: IcmpTxCounters<I>,
        ndp_counters: NdpCounters,
    }

    impl<I: IcmpTestIpExt> FakeIcmpCoreCtx<I> {
        fn with_errors_per_second(errors_per_second: u64) -> Self {
            Self {
                icmp: FakeIcmpCoreCtxState {
                    error_send_bucket: TokenBucket::new(errors_per_second),
                    receive_icmp_error: Default::default(),
                    rx_counters: Default::default(),
                    tx_counters: Default::default(),
                    ndp_counters: Default::default(),
                },
                ip_socket_ctx: InnerIpSocketCtx::with_state(FakeIpSocketCtx::new(
                    core::iter::once(FakeDeviceConfig {
                        device: FakeDeviceId,
                        local_ips: vec![I::TEST_ADDRS.local_ip],
                        remote_ips: vec![I::TEST_ADDRS.remote_ip],
                    }),
                )),
            }
        }
    }

    impl<I: IcmpTestIpExt> Default for FakeIcmpCoreCtx<I> {
        fn default() -> Self {
            Self::with_errors_per_second(DEFAULT_ERRORS_PER_SECOND)
        }
    }

    impl<I: IcmpTestIpExt> DeviceIdContext<AnyDevice> for FakeIcmpCoreCtx<I> {
        type DeviceId = FakeDeviceId;
        type WeakDeviceId = FakeWeakDeviceId<FakeDeviceId>;
    }

    impl<I: IcmpTestIpExt> IcmpStateContext for FakeIcmpCoreCtx<I> {}
    impl<I: IcmpTestIpExt> IcmpStateContext for InnerIpSocketCtx<I> {}

    impl<I: IcmpTestIpExt> CounterContext<IcmpRxCounters<I>> for FakeIcmpCoreCtx<I> {
        fn counters(&self) -> &IcmpRxCounters<I> {
            &self.icmp.rx_counters
        }
    }

    impl<I: IcmpTestIpExt> CounterContext<IcmpTxCounters<I>> for FakeIcmpCoreCtx<I> {
        fn counters(&self) -> &IcmpTxCounters<I> {
            &self.icmp.tx_counters
        }
    }

    impl<I: IcmpTestIpExt> CounterContext<NdpCounters> for FakeIcmpCoreCtx<I> {
        fn counters(&self) -> &NdpCounters {
            &self.icmp.ndp_counters
        }
    }

    pub enum FakeEchoIpTransportContext {}

    impl EchoTransportContextMarker for FakeEchoIpTransportContext {}

    impl<I> IpTransportContext<I, FakeIcmpBindingsCtx<I>, FakeIcmpCoreCtx<I>>
        for FakeEchoIpTransportContext
    where
        I: IcmpTestIpExt + IpLayerIpExt,
    {
        type EarlyDemuxSocket = Never;

        fn early_demux<B: ParseBuffer>(
            _core_ctx: &mut FakeIcmpCoreCtx<I>,
            _device: &FakeDeviceId,
            _src_ip: I::Addr,
            _dst_ip: I::Addr,
            _buffer: B,
        ) -> Option<Self::EarlyDemuxSocket> {
            None
        }

        fn receive_icmp_error(
            core_ctx: &mut FakeIcmpCoreCtx<I>,
            _bindings_ctx: &mut FakeIcmpBindingsCtx<I>,
            _device: &FakeDeviceId,
            _original_src_ip: Option<SpecifiedAddr<I::Addr>>,
            _original_dst_ip: SpecifiedAddr<I::Addr>,
            _original_body: &[u8],
            _err: I::ErrorCode,
        ) {
            core_ctx.icmp.rx_counters.error_delivered_to_socket.increment()
        }

        fn receive_ip_packet<B: BufferMut, H: IpHeaderInfo<I>>(
            _core_ctx: &mut FakeIcmpCoreCtx<I>,
            _bindings_ctx: &mut FakeIcmpBindingsCtx<I>,
            _device: &FakeDeviceId,
            _src_ip: I::RecvSrcAddr,
            _dst_ip: SpecifiedAddr<I::Addr>,
            _buffer: B,
            _info: &mut LocalDeliveryPacketInfo<I, H>,
            _early_demux_socket: Option<Never>,
        ) -> Result<(), (B, I::IcmpError)> {
            unimplemented!()
        }
    }

    impl<I: IpLayerIpExt + IcmpTestIpExt> InnerIcmpContext<I, FakeIcmpBindingsCtx<I>>
        for FakeIcmpCoreCtx<I>
    {
        type EchoTransportContext = FakeEchoIpTransportContext;

        fn receive_icmp_error(
            &mut self,
            bindings_ctx: &mut FakeIcmpBindingsCtx<I>,
            device: &Self::DeviceId,
            original_src_ip: Option<SpecifiedAddr<I::Addr>>,
            original_dst_ip: SpecifiedAddr<I::Addr>,
            original_proto: I::Proto,
            original_body: &[u8],
            err: I::ErrorCode,
        ) {
            CounterContext::<IcmpRxCounters<I>>::counters(self).error.increment();
            self.icmp.receive_icmp_error.push(err);
            if original_proto == I::ICMP_IP_PROTO {
                receive_ip_transport_icmp_error(
                    self,
                    bindings_ctx,
                    device,
                    original_src_ip,
                    original_dst_ip,
                    original_body,
                    err,
                )
            }
        }
    }

    impl<I: IpLayerIpExt + IcmpTestIpExt> IcmpSendContext<I, FakeIcmpBindingsCtx<I>>
        for FakeIcmpCoreCtx<I>
    {
        fn with_error_send_bucket_mut<O, F: FnOnce(&mut TokenBucket<FakeInstant>) -> O>(
            &mut self,
            cb: F,
        ) -> O {
            cb(&mut self.icmp.error_send_bucket)
        }
    }

    #[test]
    fn test_should_send_icmpv4_error() {
        let src_ip = TEST_ADDRS_V4.local_ip;
        let dst_ip = TEST_ADDRS_V4.remote_ip;
        let frame_dst = FrameDestination::Individual { local: () };
        let multicast_ip_1 = SpecifiedAddr::new(Ipv4Addr::new([224, 0, 0, 1])).unwrap();
        let multicast_ip_2 = SpecifiedAddr::new(Ipv4Addr::new([224, 0, 0, 2])).unwrap();

        // Should send to unicast addresses.
        assert!(should_send_icmpv4_error(Some(frame_dst), src_ip, dst_ip));
        assert!(should_send_icmpv4_error(None, src_ip, dst_ip));

        // Should send because loopback addresses are allowed
        assert!(should_send_icmpv4_error(
            Some(frame_dst),
            Ipv4::LOOPBACK_ADDRESS,
            Ipv4::LOOPBACK_ADDRESS,
        ));

        // Should not send because destined for IP broadcast addr
        assert!(!should_send_icmpv4_error(
            Some(frame_dst),
            src_ip,
            Ipv4::LIMITED_BROADCAST_ADDRESS,
        ));

        // Should not send because destined for multicast addr
        assert!(!should_send_icmpv4_error(Some(frame_dst), src_ip, multicast_ip_1,));

        // Should not send because Link Layer Broadcast.
        assert!(!should_send_icmpv4_error(Some(FrameDestination::Broadcast), src_ip, dst_ip,));

        // Should not send because from limited broadcast addr
        assert!(!should_send_icmpv4_error(
            Some(frame_dst),
            Ipv4::LIMITED_BROADCAST_ADDRESS,
            dst_ip,
        ));

        // Should not send because from multicast addr
        assert!(!should_send_icmpv4_error(Some(frame_dst), multicast_ip_2, dst_ip));

        // Should not send because from class E addr
        assert!(!should_send_icmpv4_error(
            Some(frame_dst),
            SpecifiedAddr::new(Ipv4Addr::new([240, 0, 0, 1])).unwrap(),
            dst_ip,
        ));
    }

    #[test]
    fn test_should_send_icmpv6_error() {
        let src_ip = TEST_ADDRS_V6.local_ip;
        let dst_ip = TEST_ADDRS_V6.remote_ip;
        let frame_dst = FrameDestination::Individual { local: () };
        let multicast_ip_1 =
            SpecifiedAddr::new(Ipv6Addr::new([0xff00, 0, 0, 0, 0, 0, 0, 1])).unwrap();
        let multicast_ip_2 =
            SpecifiedAddr::new(Ipv6Addr::new([0xff00, 0, 0, 0, 0, 0, 0, 2])).unwrap();

        // Should Send.
        assert!(should_send_icmpv6_error(
            Some(frame_dst),
            src_ip,
            dst_ip,
            false /* allow_dst_multicast */
        ));
        assert!(should_send_icmpv6_error(
            None, src_ip, dst_ip, false /* allow_dst_multicast */
        ));
        assert!(should_send_icmpv6_error(
            Some(frame_dst),
            src_ip,
            dst_ip,
            true /* allow_dst_multicast */
        ));

        // Should send because loopback is allowed
        assert!(should_send_icmpv6_error(
            Some(frame_dst),
            Ipv6::LOOPBACK_ADDRESS,
            Ipv6::LOOPBACK_ADDRESS,
            false /* allow_dst_multicast */
        ));
        assert!(should_send_icmpv6_error(
            Some(frame_dst),
            Ipv6::LOOPBACK_ADDRESS,
            Ipv6::LOOPBACK_ADDRESS,
            true /* allow_dst_multicast */
        ));

        // Should not send because destined for multicast addr, unless exception
        // applies.
        assert!(!should_send_icmpv6_error(
            Some(frame_dst),
            src_ip,
            multicast_ip_1,
            false /* allow_dst_multicast */
        ));
        assert!(should_send_icmpv6_error(
            Some(frame_dst),
            src_ip,
            multicast_ip_1,
            true /* allow_dst_multicast */
        ));

        // Should not send because Link Layer Broadcast, unless exception
        // applies.
        assert!(!should_send_icmpv6_error(
            Some(FrameDestination::Broadcast),
            src_ip,
            dst_ip,
            false /* allow_dst_multicast */
        ));
        assert!(should_send_icmpv6_error(
            Some(FrameDestination::Broadcast),
            src_ip,
            dst_ip,
            true /* allow_dst_multicast */
        ));

        // Should not send because from multicast addr.
        assert!(!should_send_icmpv6_error(
            Some(frame_dst),
            multicast_ip_2,
            dst_ip,
            false /* allow_dst_multicast */
        ));
        assert!(!should_send_icmpv6_error(
            Some(frame_dst),
            multicast_ip_2,
            dst_ip,
            true /* allow_dst_multicast */
        ));

        // Should not send because from multicast addr, even though dest
        // multicast exception applies.
        assert!(!should_send_icmpv6_error(
            Some(FrameDestination::Broadcast),
            multicast_ip_2,
            dst_ip,
            false /* allow_dst_multicast */
        ));
        assert!(!should_send_icmpv6_error(
            Some(FrameDestination::Broadcast),
            multicast_ip_2,
            dst_ip,
            true /* allow_dst_multicast */
        ));
        assert!(!should_send_icmpv6_error(
            Some(frame_dst),
            multicast_ip_2,
            multicast_ip_1,
            false /* allow_dst_multicast */
        ));
        assert!(!should_send_icmpv6_error(
            Some(frame_dst),
            multicast_ip_2,
            multicast_ip_1,
            true /* allow_dst_multicast */
        ));
    }

    // Tests that only require an ICMP stack. Unlike the preceding tests, these
    // only test the ICMP stack and state, and fake everything else. We define
    // the `FakeIcmpv4Ctx` and `FakeIcmpv6Ctx` types, which we wrap in a
    // `FakeCtx` to provide automatic implementations of a number of required
    // traits. The rest we implement manually.

    #[derive(Default)]
    pub(super) struct FakeIcmpBindingsCtxState<I: IpExt> {
        _marker: core::marker::PhantomData<I>,
    }

    impl InnerIcmpv4Context<FakeIcmpBindingsCtx<Ipv4>> for FakeIcmpCoreCtx<Ipv4> {
        fn should_send_timestamp_reply(&self) -> bool {
            false
        }
    }
    impl_pmtu_handler!(FakeIcmpCoreCtx<Ipv4>, FakeIcmpBindingsCtx<Ipv4>, Ipv4);
    impl_pmtu_handler!(FakeIcmpCoreCtx<Ipv6>, FakeIcmpBindingsCtx<Ipv6>, Ipv6);

    impl<I: IcmpTestIpExt> IpSocketHandler<I, FakeIcmpBindingsCtx<I>> for FakeIcmpCoreCtx<I> {
        fn new_ip_socket<O>(
            &mut self,
            bindings_ctx: &mut FakeIcmpBindingsCtx<I>,
            args: IpSocketArgs<'_, Self::DeviceId, I, O>,
        ) -> Result<IpSock<I, Self::WeakDeviceId>, IpSockCreationError>
        where
            O: RouteResolutionOptions<I>,
        {
            self.ip_socket_ctx.new_ip_socket(bindings_ctx, args)
        }

        fn send_ip_packet<S, O>(
            &mut self,
            bindings_ctx: &mut FakeIcmpBindingsCtx<I>,
            socket: &IpSock<I, Self::WeakDeviceId>,
            body: S,
            options: &O,
            tx_meta: FakeTxMetadata,
        ) -> Result<(), IpSockSendError>
        where
            S: TransportPacketSerializer<I>,
            S::Buffer: BufferMut,
            O: SendOptions<I> + RouteResolutionOptions<I>,
        {
            self.ip_socket_ctx.send_ip_packet(bindings_ctx, socket, body, options, tx_meta)
        }

        fn confirm_reachable<O>(
            &mut self,
            bindings_ctx: &mut FakeIcmpBindingsCtx<I>,
            socket: &IpSock<I, Self::WeakDeviceId>,
            options: &O,
        ) where
            O: RouteResolutionOptions<I>,
        {
            self.ip_socket_ctx.confirm_reachable(bindings_ctx, socket, options)
        }
    }

    impl IpDeviceHandler<Ipv6, FakeIcmpBindingsCtx<Ipv6>> for FakeIcmpCoreCtx<Ipv6> {
        fn is_router_device(&mut self, _device_id: &Self::DeviceId) -> bool {
            unimplemented!()
        }

        fn set_default_hop_limit(&mut self, _device_id: &Self::DeviceId, _hop_limit: NonZeroU8) {
            unreachable!()
        }

        fn handle_received_dad_packet(
            &mut self,
            _bindings_ctx: &mut FakeIcmpBindingsCtx<Ipv6>,
            _device_id: &Self::DeviceId,
            _addr: SpecifiedAddr<Ipv6Addr>,
            _probe_data: Option<NdpNonce<&'_ [u8]>>,
        ) -> Option<IpAddressState> {
            unimplemented!()
        }
    }

    impl IpDeviceEgressStateContext<Ipv6> for FakeIcmpCoreCtx<Ipv6> {
        fn with_next_packet_id<O, F: FnOnce(&()) -> O>(&self, cb: F) -> O {
            cb(&())
        }

        fn get_local_addr_for_remote(
            &mut self,
            _device_id: &Self::DeviceId,
            _remote: Option<SpecifiedAddr<Ipv6Addr>>,
        ) -> Option<IpDeviceAddr<Ipv6Addr>> {
            unimplemented!()
        }

        fn get_hop_limit(&mut self, _device_id: &Self::DeviceId) -> NonZeroU8 {
            unimplemented!()
        }
    }

    impl IpDeviceIngressStateContext<Ipv6> for FakeIcmpCoreCtx<Ipv6> {
        fn address_status_for_device(
            &mut self,
            _addr: SpecifiedAddr<Ipv6Addr>,
            _device_id: &Self::DeviceId,
        ) -> AddressStatus<Ipv6PresentAddressStatus> {
            unimplemented!()
        }
    }

    impl Ipv6DeviceHandler<FakeIcmpBindingsCtx<Ipv6>> for FakeIcmpCoreCtx<Ipv6> {
        type LinkLayerAddr = Uninstantiable;

        fn get_link_layer_addr(&mut self, _device_id: &Self::DeviceId) -> Option<Uninstantiable> {
            unimplemented!()
        }

        fn set_discovered_retrans_timer(
            &mut self,
            _bindings_ctx: &mut FakeIcmpBindingsCtx<Ipv6>,
            _device_id: &Self::DeviceId,
            _retrans_timer: NonZeroDuration,
        ) {
            unimplemented!()
        }

        fn set_link_mtu(&mut self, _device_id: &Self::DeviceId, _mtu: Mtu) {
            unimplemented!()
        }

        fn update_discovered_ipv6_route(
            &mut self,
            _bindings_ctx: &mut FakeIcmpBindingsCtx<Ipv6>,
            _device_id: &Self::DeviceId,
            _route: Ipv6DiscoveredRoute,
            _properties: Ipv6DiscoveredRouteProperties,
            _lifetime: Option<NonZeroNdpLifetime>,
        ) {
            unimplemented!()
        }

        fn apply_slaac_update(
            &mut self,
            _bindings_ctx: &mut FakeIcmpBindingsCtx<Ipv6>,
            _device_id: &Self::DeviceId,
            _subnet: Subnet<Ipv6Addr>,
            _preferred_lifetime: Option<NonZeroNdpLifetime>,
            _valid_lifetime: Option<NonZeroNdpLifetime>,
        ) {
            unimplemented!()
        }

        fn receive_mld_packet<B: SplitByteSlice, H: IpHeaderInfo<Ipv6>>(
            &mut self,
            _bindings_ctx: &mut FakeIcmpBindingsCtx<Ipv6>,
            _device: &FakeDeviceId,
            _src_ip: Ipv6SourceAddr,
            _dst_ip: SpecifiedAddr<Ipv6Addr>,
            _packet: MldPacket<B>,
            _header_info: &H,
        ) {
            unimplemented!()
        }
    }

    impl IpLayerHandler<Ipv6, FakeIcmpBindingsCtx<Ipv6>> for FakeIcmpCoreCtx<Ipv6> {
        fn send_ip_packet_from_device<S>(
            &mut self,
            _bindings_ctx: &mut FakeIcmpBindingsCtx<Ipv6>,
            _meta: SendIpPacketMeta<Ipv6, &Self::DeviceId, Option<SpecifiedAddr<Ipv6Addr>>>,
            _body: S,
        ) -> Result<(), IpSendFrameError<S>> {
            unimplemented!()
        }

        fn send_ip_frame<S>(
            &mut self,
            _bindings_ctx: &mut FakeIcmpBindingsCtx<Ipv6>,
            _device: &Self::DeviceId,
            _destination: IpPacketDestination<Ipv6, &Self::DeviceId>,
            _body: S,
        ) -> Result<(), IpSendFrameError<S>>
        where
            S: NetworkSerializer,
            S::Buffer: BufferMut,
        {
            unimplemented!()
        }
    }

    impl NudIpHandler<Ipv6, FakeIcmpBindingsCtx<Ipv6>> for FakeIcmpCoreCtx<Ipv6> {
        fn handle_neighbor_probe(
            &mut self,
            _bindings_ctx: &mut FakeIcmpBindingsCtx<Ipv6>,
            _device_id: &Self::DeviceId,
            _neighbor: SpecifiedAddr<Ipv6Addr>,
            _link_addr: &[u8],
        ) {
            unimplemented!()
        }

        fn handle_neighbor_confirmation(
            &mut self,
            _bindings_ctx: &mut FakeIcmpBindingsCtx<Ipv6>,
            _device_id: &Self::DeviceId,
            _neighbor: SpecifiedAddr<Ipv6Addr>,
            _link_addr: Option<&[u8]>,
            _flags: ConfirmationFlags,
        ) {
            unimplemented!()
        }

        fn flush_neighbor_table(
            &mut self,
            _bindings_ctx: &mut FakeIcmpBindingsCtx<Ipv6>,
            _device_id: &Self::DeviceId,
        ) {
            unimplemented!()
        }
    }

    #[test]
    fn test_receive_icmpv4_error() {
        // Chosen arbitrarily to be a) non-zero (it's easy to accidentally get
        // the value 0) and, b) different from each other.
        const ICMP_ID: u16 = 0x0F;
        const SEQ_NUM: u16 = 0xF0;

        /// Test receiving an ICMP error message.
        ///
        /// Test that receiving an ICMP error message with the given code and
        /// message contents, and containing the given original IPv4 packet,
        /// results in the counter values in `assert_counters`. After that
        /// assertion passes, `f` is called on the context so that the caller
        /// can perform whatever extra validation they want.
        ///
        /// The error message will be sent from `TEST_ADDRS_V4.remote_ip` to
        /// `TEST_ADDRS_V4.local_ip`. Before the message is sent, an ICMP
        /// socket will be established with the ID `ICMP_ID`, and
        /// `test_receive_icmpv4_error_helper` will assert that its `SocketId`
        /// is 0. This allows the caller to craft the `original_packet` so that
        /// it should be delivered to this socket.
        fn test_receive_icmpv4_error_helper<
            C: Debug,
            M: IcmpMessage<Ipv4, Code = C> + Debug,
            F: Fn(&FakeIcmpCtx<Ipv4>),
        >(
            original_packet: &mut [u8],
            code: C,
            msg: M,
            f: F,
        ) {
            set_logger_for_test();

            let mut ctx: FakeIcmpCtx<Ipv4> = FakeIcmpCtx::default();

            let CtxPair { core_ctx, bindings_ctx } = &mut ctx;
            <IcmpIpTransportContext as IpTransportContext<Ipv4, _, _>>::receive_ip_packet(
                core_ctx,
                bindings_ctx,
                &FakeDeviceId,
                Ipv4SourceAddr::new(*TEST_ADDRS_V4.remote_ip).unwrap(),
                TEST_ADDRS_V4.local_ip,
                IcmpPacketBuilder::new(TEST_ADDRS_V4.remote_ip, TEST_ADDRS_V4.local_ip, code, msg)
                    .wrap_body(Buf::new(original_packet, ..))
                    .serialize_vec_outer(&mut NetworkSerializationContext::default())
                    .unwrap(),
                &mut LocalDeliveryPacketInfo::default(),
                None,
            )
            .unwrap();
            f(&ctx);
        }
        // Test that, when we receive various ICMPv4 error messages, we properly
        // pass them up to the IP layer and, sometimes, to the transport layer.

        // First, test with an original packet containing an ICMP message. Since
        // this test fake supports ICMP sockets, this error can be delivered all
        // the way up the stack.

        // A buffer containing an ICMP echo request with ID `ICMP_ID` and
        // sequence number `SEQ_NUM` from the local IP to the remote IP. Any
        // ICMP error message which contains this as its original packet should
        // be delivered to the socket created in
        // `test_receive_icmpv4_error_helper`.
        let mut buffer = EmptyBuf
            .wrap_in(IcmpPacketBuilder::<Ipv4, _>::new(
                TEST_ADDRS_V4.local_ip,
                TEST_ADDRS_V4.remote_ip,
                IcmpZeroCode,
                IcmpEchoRequest::new(ICMP_ID, SEQ_NUM),
            ))
            .wrap_in(<Ipv4 as packet_formats::ip::IpExt>::PacketBuilder::<
                NetworkSerializationContext,
            >::new(
                TEST_ADDRS_V4.local_ip, TEST_ADDRS_V4.remote_ip, 64, Ipv4Proto::Icmp
            ))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap();

        test_receive_icmpv4_error_helper(
            buffer.as_mut(),
            Icmpv4DestUnreachableCode::DestNetworkUnreachable,
            IcmpDestUnreachable::default(),
            |CtxPair { core_ctx, bindings_ctx: _ }| {
                assert_eq!(core_ctx.icmp.rx_counters.error.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_transport_layer.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_socket.get(), 1);
                assert_eq!(
                    core_ctx.icmp.rx_counters.dest_unreachable.dest_network_unreachable.get(),
                    1
                );
                let err = Icmpv4ErrorCode::DestUnreachable(
                    Icmpv4DestUnreachableCode::DestNetworkUnreachable,
                    IcmpDestUnreachable::default(),
                );
                assert_eq!(core_ctx.icmp.receive_icmp_error, [err]);
            },
        );

        test_receive_icmpv4_error_helper(
            buffer.as_mut(),
            Icmpv4TimeExceededCode::TtlExpired,
            IcmpTimeExceeded::default(),
            |CtxPair { core_ctx, bindings_ctx: _ }| {
                assert_eq!(core_ctx.icmp.rx_counters.error.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_transport_layer.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_socket.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.time_exceeded.ttl_expired.get(), 1);
                let err = Icmpv4ErrorCode::TimeExceeded(Icmpv4TimeExceededCode::TtlExpired);
                assert_eq!(core_ctx.icmp.receive_icmp_error, [err]);
            },
        );

        test_receive_icmpv4_error_helper(
            buffer.as_mut(),
            Icmpv4ParameterProblemCode::PointerIndicatesError,
            Icmpv4ParameterProblem::new(0),
            |CtxPair { core_ctx, bindings_ctx: _ }| {
                assert_eq!(core_ctx.icmp.rx_counters.error.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_transport_layer.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_socket.get(), 1);
                assert_eq!(
                    core_ctx.icmp.rx_counters.parameter_problem.pointer_indicates_error.get(),
                    1
                );
                let err = Icmpv4ErrorCode::ParameterProblem(
                    Icmpv4ParameterProblemCode::PointerIndicatesError,
                );
                assert_eq!(core_ctx.icmp.receive_icmp_error, [err]);
            },
        );

        // Second, test with an original packet containing a malformed ICMP
        // packet (we accomplish this by leaving the IP packet's body empty). We
        // should process this packet in
        // `IcmpIpTransportContext::receive_icmp_error`, but we should go no
        // further - in particular, we should not dispatch to the Echo sockets.

        let mut buffer = <Ipv4 as packet_formats::ip::IpExt>::PacketBuilder::<
            NetworkSerializationContext,
        >::new(
            TEST_ADDRS_V4.local_ip, TEST_ADDRS_V4.remote_ip, 64, Ipv4Proto::Icmp
        )
        .wrap_body(EmptyBuf)
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap();

        test_receive_icmpv4_error_helper(
            buffer.as_mut(),
            Icmpv4DestUnreachableCode::DestNetworkUnreachable,
            IcmpDestUnreachable::default(),
            |CtxPair { core_ctx, bindings_ctx: _ }| {
                assert_eq!(core_ctx.icmp.rx_counters.error.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_transport_layer.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_socket.get(), 0);
                assert_eq!(
                    core_ctx.icmp.rx_counters.dest_unreachable.dest_network_unreachable.get(),
                    1
                );
                let err = Icmpv4ErrorCode::DestUnreachable(
                    Icmpv4DestUnreachableCode::DestNetworkUnreachable,
                    IcmpDestUnreachable::default(),
                );
                assert_eq!(core_ctx.icmp.receive_icmp_error, [err]);
            },
        );

        test_receive_icmpv4_error_helper(
            buffer.as_mut(),
            Icmpv4TimeExceededCode::TtlExpired,
            IcmpTimeExceeded::default(),
            |CtxPair { core_ctx, bindings_ctx: _ }| {
                assert_eq!(core_ctx.icmp.rx_counters.error.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_transport_layer.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_socket.get(), 0);
                assert_eq!(core_ctx.icmp.rx_counters.time_exceeded.ttl_expired.get(), 1);
                let err = Icmpv4ErrorCode::TimeExceeded(Icmpv4TimeExceededCode::TtlExpired);
                assert_eq!(core_ctx.icmp.receive_icmp_error, [err]);
            },
        );

        test_receive_icmpv4_error_helper(
            buffer.as_mut(),
            Icmpv4ParameterProblemCode::PointerIndicatesError,
            Icmpv4ParameterProblem::new(0),
            |CtxPair { core_ctx, bindings_ctx: _ }| {
                assert_eq!(core_ctx.icmp.rx_counters.error.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_transport_layer.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_socket.get(), 0);
                assert_eq!(
                    core_ctx.icmp.rx_counters.parameter_problem.pointer_indicates_error.get(),
                    1
                );
                let err = Icmpv4ErrorCode::ParameterProblem(
                    Icmpv4ParameterProblemCode::PointerIndicatesError,
                );
                assert_eq!(core_ctx.icmp.receive_icmp_error, [err]);
            },
        );

        // Third, test with an original packet containing a UDP packet. This
        // allows us to verify that protocol numbers are handled properly by
        // checking that `IcmpIpTransportContext::receive_icmp_error` was NOT
        // called.

        let mut buffer = <Ipv4 as packet_formats::ip::IpExt>::PacketBuilder::<
            NetworkSerializationContext,
        >::new(
            TEST_ADDRS_V4.local_ip, TEST_ADDRS_V4.remote_ip, 64, IpProto::Udp.into()
        )
        .wrap_body(EmptyBuf)
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap();

        test_receive_icmpv4_error_helper(
            buffer.as_mut(),
            Icmpv4DestUnreachableCode::DestNetworkUnreachable,
            IcmpDestUnreachable::default(),
            |CtxPair { core_ctx, bindings_ctx: _ }| {
                assert_eq!(core_ctx.icmp.rx_counters.error.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_transport_layer.get(), 0);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_socket.get(), 0);
                assert_eq!(
                    core_ctx.icmp.rx_counters.dest_unreachable.dest_network_unreachable.get(),
                    1
                );
                let err = Icmpv4ErrorCode::DestUnreachable(
                    Icmpv4DestUnreachableCode::DestNetworkUnreachable,
                    IcmpDestUnreachable::default(),
                );
                assert_eq!(core_ctx.icmp.receive_icmp_error, [err]);
            },
        );

        test_receive_icmpv4_error_helper(
            buffer.as_mut(),
            Icmpv4TimeExceededCode::TtlExpired,
            IcmpTimeExceeded::default(),
            |CtxPair { core_ctx, bindings_ctx: _ }| {
                assert_eq!(core_ctx.icmp.rx_counters.error.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_transport_layer.get(), 0);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_socket.get(), 0);
                assert_eq!(core_ctx.icmp.rx_counters.time_exceeded.ttl_expired.get(), 1);
                let err = Icmpv4ErrorCode::TimeExceeded(Icmpv4TimeExceededCode::TtlExpired);
                assert_eq!(core_ctx.icmp.receive_icmp_error, [err]);
            },
        );

        test_receive_icmpv4_error_helper(
            buffer.as_mut(),
            Icmpv4ParameterProblemCode::PointerIndicatesError,
            Icmpv4ParameterProblem::new(0),
            |CtxPair { core_ctx, bindings_ctx: _ }| {
                assert_eq!(core_ctx.icmp.rx_counters.error.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_transport_layer.get(), 0);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_socket.get(), 0);
                assert_eq!(
                    core_ctx.icmp.rx_counters.parameter_problem.pointer_indicates_error.get(),
                    1
                );
                let err = Icmpv4ErrorCode::ParameterProblem(
                    Icmpv4ParameterProblemCode::PointerIndicatesError,
                );
                assert_eq!(core_ctx.icmp.receive_icmp_error, [err]);
            },
        );
    }

    #[test]
    fn test_receive_icmpv6_error() {
        // Chosen arbitrarily to be a) non-zero (it's easy to accidentally get
        // the value 0) and, b) different from each other.
        const ICMP_ID: u16 = 0x0F;
        const SEQ_NUM: u16 = 0xF0;

        /// Test receiving an ICMPv6 error message.
        ///
        /// Test that receiving an ICMP error message with the given code and
        /// message contents, and containing the given original IPv4 packet,
        /// results in the counter values in `assert_counters`. After that
        /// assertion passes, `f` is called on the context so that the caller
        /// can perform whatever extra validation they want.
        ///
        /// The error message will be sent from `TEST_ADDRS_V6.remote_ip` to
        /// `TEST_ADDRS_V6.local_ip`. Before the message is sent, an ICMP
        /// socket will be established with the ID `ICMP_ID`, and
        /// `test_receive_icmpv6_error_helper` will assert that its `SocketId`
        /// is 0. This allows the caller to craft the `original_packet` so that
        /// it should be delivered to this socket.
        fn test_receive_icmpv6_error_helper<
            C: Debug,
            M: IcmpMessage<Ipv6, Code = C> + Debug,
            F: Fn(&FakeIcmpCtx<Ipv6>),
        >(
            original_packet: &mut [u8],
            code: C,
            msg: M,
            f: F,
        ) {
            set_logger_for_test();

            let mut ctx = FakeIcmpCtx::<Ipv6>::default();
            let CtxPair { core_ctx, bindings_ctx } = &mut ctx;
            <IcmpIpTransportContext as IpTransportContext<Ipv6, _, _>>::receive_ip_packet(
                core_ctx,
                bindings_ctx,
                &FakeDeviceId,
                TEST_ADDRS_V6.remote_ip.get().try_into().unwrap(),
                TEST_ADDRS_V6.local_ip,
                IcmpPacketBuilder::new(TEST_ADDRS_V6.remote_ip, TEST_ADDRS_V6.local_ip, code, msg)
                    .wrap_body(Buf::new(original_packet, ..))
                    .serialize_vec_outer(&mut NetworkSerializationContext::default())
                    .unwrap(),
                &mut LocalDeliveryPacketInfo::default(),
                None,
            )
            .unwrap();
            f(&ctx);
        }
        // Test that, when we receive various ICMPv6 error messages, we properly
        // pass them up to the IP layer and, sometimes, to the transport layer.

        // First, test with an original packet containing an ICMPv6 message.
        // Since this test fake supports ICMPv6 sockets, this error can be
        // delivered all the way up the stack.

        // A buffer containing an ICMPv6 echo request with ID `ICMP_ID` and
        // sequence number `SEQ_NUM` from the local IP to the remote IP. Any
        // ICMPv6 error message which contains this as its original packet
        // should be delivered to the socket created in
        // `test_receive_icmpv6_error_helper`.
        let mut buffer = EmptyBuf
            .wrap_in(IcmpPacketBuilder::<Ipv6, _>::new(
                TEST_ADDRS_V6.local_ip,
                TEST_ADDRS_V6.remote_ip,
                IcmpZeroCode,
                IcmpEchoRequest::new(ICMP_ID, SEQ_NUM),
            ))
            .wrap_in(<Ipv6 as packet_formats::ip::IpExt>::PacketBuilder::<
                NetworkSerializationContext,
            >::new(
                TEST_ADDRS_V6.local_ip, TEST_ADDRS_V6.remote_ip, 64, Ipv6Proto::Icmpv6
            ))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap();

        test_receive_icmpv6_error_helper(
            buffer.as_mut(),
            Icmpv6DestUnreachableCode::NoRoute,
            IcmpDestUnreachable::default(),
            |CtxPair { core_ctx, bindings_ctx: _ }| {
                assert_eq!(core_ctx.icmp.rx_counters.error.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_transport_layer.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_socket.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.dest_unreachable.no_route.get(), 1);
                let err = Icmpv6ErrorCode::DestUnreachable(Icmpv6DestUnreachableCode::NoRoute);
                assert_eq!(core_ctx.icmp.receive_icmp_error, [err]);
            },
        );

        test_receive_icmpv6_error_helper(
            buffer.as_mut(),
            Icmpv6TimeExceededCode::HopLimitExceeded,
            IcmpTimeExceeded::default(),
            |CtxPair { core_ctx, bindings_ctx: _ }| {
                assert_eq!(core_ctx.icmp.rx_counters.error.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_transport_layer.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_socket.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.time_exceeded.hop_limit_exceeded.get(), 1);
                let err = Icmpv6ErrorCode::TimeExceeded(Icmpv6TimeExceededCode::HopLimitExceeded);
                assert_eq!(core_ctx.icmp.receive_icmp_error, [err]);
            },
        );

        test_receive_icmpv6_error_helper(
            buffer.as_mut(),
            Icmpv6ParameterProblemCode::UnrecognizedNextHeaderType,
            Icmpv6ParameterProblem::new(0),
            |CtxPair { core_ctx, bindings_ctx: _ }| {
                assert_eq!(core_ctx.icmp.rx_counters.error.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_transport_layer.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_socket.get(), 1);
                assert_eq!(
                    core_ctx.icmp.rx_counters.parameter_problem.unrecognized_next_header_type.get(),
                    1
                );
                let err = Icmpv6ErrorCode::ParameterProblem(
                    Icmpv6ParameterProblemCode::UnrecognizedNextHeaderType,
                );
                assert_eq!(core_ctx.icmp.receive_icmp_error, [err]);
            },
        );

        // Second, test with an original packet containing a malformed ICMPv6
        // packet (we accomplish this by leaving the IP packet's body empty). We
        // should process this packet in
        // `IcmpIpTransportContext::receive_icmp_error`, but we should go no
        // further - in particular, we should not call into Echo sockets.

        let mut buffer = EmptyBuf
            .wrap_in(<Ipv6 as packet_formats::ip::IpExt>::PacketBuilder::<
                NetworkSerializationContext,
            >::new(
                TEST_ADDRS_V6.local_ip, TEST_ADDRS_V6.remote_ip, 64, Ipv6Proto::Icmpv6
            ))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap();

        test_receive_icmpv6_error_helper(
            buffer.as_mut(),
            Icmpv6DestUnreachableCode::NoRoute,
            IcmpDestUnreachable::default(),
            |CtxPair { core_ctx, bindings_ctx: _ }| {
                assert_eq!(core_ctx.icmp.rx_counters.error.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_transport_layer.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_socket.get(), 0);
                assert_eq!(core_ctx.icmp.rx_counters.dest_unreachable.no_route.get(), 1);
                let err = Icmpv6ErrorCode::DestUnreachable(Icmpv6DestUnreachableCode::NoRoute);
                assert_eq!(core_ctx.icmp.receive_icmp_error, [err]);
            },
        );

        test_receive_icmpv6_error_helper(
            buffer.as_mut(),
            Icmpv6TimeExceededCode::HopLimitExceeded,
            IcmpTimeExceeded::default(),
            |CtxPair { core_ctx, bindings_ctx: _ }| {
                assert_eq!(core_ctx.icmp.rx_counters.error.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_transport_layer.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_socket.get(), 0);
                assert_eq!(core_ctx.icmp.rx_counters.time_exceeded.hop_limit_exceeded.get(), 1);
                let err = Icmpv6ErrorCode::TimeExceeded(Icmpv6TimeExceededCode::HopLimitExceeded);
                assert_eq!(core_ctx.icmp.receive_icmp_error, [err]);
            },
        );

        test_receive_icmpv6_error_helper(
            buffer.as_mut(),
            Icmpv6ParameterProblemCode::UnrecognizedNextHeaderType,
            Icmpv6ParameterProblem::new(0),
            |CtxPair { core_ctx, bindings_ctx: _ }| {
                assert_eq!(core_ctx.icmp.rx_counters.error.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_transport_layer.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_socket.get(), 0);
                assert_eq!(
                    core_ctx.icmp.rx_counters.parameter_problem.unrecognized_next_header_type.get(),
                    1
                );
                let err = Icmpv6ErrorCode::ParameterProblem(
                    Icmpv6ParameterProblemCode::UnrecognizedNextHeaderType,
                );
                assert_eq!(core_ctx.icmp.receive_icmp_error, [err]);
            },
        );

        // Third, test with an original packet containing a UDP packet. This
        // allows us to verify that protocol numbers are handled properly by
        // checking that `IcmpIpTransportContext::receive_icmp_error` was NOT
        // called.

        let mut buffer = <Ipv6 as packet_formats::ip::IpExt>::PacketBuilder::<
            NetworkSerializationContext,
        >::new(
            TEST_ADDRS_V6.local_ip, TEST_ADDRS_V6.remote_ip, 64, IpProto::Udp.into()
        )
        .wrap_body(EmptyBuf)
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap();

        test_receive_icmpv6_error_helper(
            buffer.as_mut(),
            Icmpv6DestUnreachableCode::NoRoute,
            IcmpDestUnreachable::default(),
            |CtxPair { core_ctx, bindings_ctx: _ }| {
                assert_eq!(core_ctx.icmp.rx_counters.error.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_transport_layer.get(), 0);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_socket.get(), 0);
                assert_eq!(core_ctx.icmp.rx_counters.dest_unreachable.no_route.get(), 1);
                let err = Icmpv6ErrorCode::DestUnreachable(Icmpv6DestUnreachableCode::NoRoute);
                assert_eq!(core_ctx.icmp.receive_icmp_error, [err]);
            },
        );

        test_receive_icmpv6_error_helper(
            buffer.as_mut(),
            Icmpv6TimeExceededCode::HopLimitExceeded,
            IcmpTimeExceeded::default(),
            |CtxPair { core_ctx, bindings_ctx: _ }| {
                assert_eq!(core_ctx.icmp.rx_counters.error.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_transport_layer.get(), 0);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_socket.get(), 0);
                assert_eq!(core_ctx.icmp.rx_counters.time_exceeded.hop_limit_exceeded.get(), 1);
                let err = Icmpv6ErrorCode::TimeExceeded(Icmpv6TimeExceededCode::HopLimitExceeded);
                assert_eq!(core_ctx.icmp.receive_icmp_error, [err]);
            },
        );

        test_receive_icmpv6_error_helper(
            buffer.as_mut(),
            Icmpv6ParameterProblemCode::UnrecognizedNextHeaderType,
            Icmpv6ParameterProblem::new(0),
            |CtxPair { core_ctx, bindings_ctx: _ }| {
                assert_eq!(core_ctx.icmp.rx_counters.error.get(), 1);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_transport_layer.get(), 0);
                assert_eq!(core_ctx.icmp.rx_counters.error_delivered_to_socket.get(), 0);
                assert_eq!(
                    core_ctx.icmp.rx_counters.parameter_problem.unrecognized_next_header_type.get(),
                    1
                );
                let err = Icmpv6ErrorCode::ParameterProblem(
                    Icmpv6ParameterProblemCode::UnrecognizedNextHeaderType,
                );
                assert_eq!(core_ctx.icmp.receive_icmp_error, [err]);
            },
        );
    }

    #[test]
    fn test_error_rate_limit() {
        set_logger_for_test();

        /// Call `send_icmpv4_ttl_expired` with fake values.
        fn send_icmpv4_ttl_expired_helper(
            CtxPair { core_ctx, bindings_ctx }: &mut FakeIcmpCtx<Ipv4>,
        ) {
            let error = Icmpv4Error::TtlExpired;
            core_ctx.send_icmp_error_message(
                bindings_ctx,
                Some(&FakeDeviceId),
                Some(FrameDestination::Individual { local: () }),
                TEST_ADDRS_V4.remote_ip.try_into().unwrap(),
                TEST_ADDRS_V4.local_ip.try_into().unwrap(),
                EmptyBuf,
                error,
                0,
                packet_formats::ip::IpProto::Udp.into(),
                &Default::default(),
            );
            assert_geq!(core_ctx.icmp.tx_counters.time_exceeded.ttl_expired.get(), 1);
        }

        /// Call `send_icmpv4_parameter_problem` with fake values.
        fn send_icmpv4_parameter_problem_helper(
            CtxPair { core_ctx, bindings_ctx }: &mut FakeIcmpCtx<Ipv4>,
        ) {
            let error = Icmpv4Error::ParameterProblem {
                code: Icmpv4ParameterProblemCode::PointerIndicatesError,
                pointer: 0,
            };
            core_ctx.send_icmp_error_message(
                bindings_ctx,
                Some(&FakeDeviceId),
                Some(FrameDestination::Individual { local: () }),
                TEST_ADDRS_V4.remote_ip.try_into().unwrap(),
                TEST_ADDRS_V4.local_ip.try_into().unwrap(),
                EmptyBuf,
                error,
                0,
                packet_formats::ip::IpProto::Udp.into(),
                &Default::default(),
            );
            assert_geq!(
                core_ctx.icmp.tx_counters.parameter_problem.pointer_indicates_error.get(),
                1
            );
        }

        /// Call `send_icmpv4_dest_unreachable` with fake values.
        fn send_icmpv4_dest_unreachable_helper(
            CtxPair { core_ctx, bindings_ctx }: &mut FakeIcmpCtx<Ipv4>,
        ) {
            core_ctx.send_icmp_error_message(
                bindings_ctx,
                Some(&FakeDeviceId),
                Some(FrameDestination::Individual { local: () }),
                TEST_ADDRS_V4.remote_ip.try_into().unwrap(),
                TEST_ADDRS_V4.local_ip.try_into().unwrap(),
                EmptyBuf,
                Icmpv4Error::NetUnreachable,
                0,
                packet_formats::ip::IpProto::Udp.into(),
                &Default::default(),
            );
            assert_geq!(
                core_ctx.icmp.tx_counters.dest_unreachable.dest_network_unreachable.get(),
                1
            );
        }

        /// Call `send_icmpv6_ttl_expired` with fake values.
        fn send_icmpv6_ttl_expired_helper(
            CtxPair { core_ctx, bindings_ctx }: &mut FakeIcmpCtx<Ipv6>,
        ) {
            core_ctx.send_icmp_error_message(
                bindings_ctx,
                Some(&FakeDeviceId),
                Some(FrameDestination::Individual { local: () }),
                TEST_ADDRS_V6.remote_ip.try_into().unwrap(),
                TEST_ADDRS_V6.local_ip.try_into().unwrap(),
                EmptyBuf,
                Icmpv6Error::TtlExpired,
                0,
                Ipv6Proto::NoNextHeader,
                &Default::default(),
            );
            assert_geq!(core_ctx.icmp.tx_counters.time_exceeded.hop_limit_exceeded.get(), 1);
        }

        /// Call `send_icmpv6_packet_too_big` with fake values.
        fn send_icmpv6_packet_too_big_helper(
            CtxPair { core_ctx, bindings_ctx }: &mut FakeIcmpCtx<Ipv6>,
        ) {
            core_ctx.send_icmp_error_message(
                bindings_ctx,
                Some(&FakeDeviceId),
                Some(FrameDestination::Individual { local: () }),
                TEST_ADDRS_V6.remote_ip.try_into().unwrap(),
                TEST_ADDRS_V6.local_ip.try_into().unwrap(),
                EmptyBuf,
                Icmpv6Error::PacketTooBig { mtu: Mtu::new(1280) },
                0,
                Ipv6Proto::NoNextHeader,
                &Default::default(),
            );
            assert_geq!(core_ctx.icmp.tx_counters.packet_too_big.get(), 1);
        }

        /// Call `send_icmpv6_parameter_problem` with fake values.
        fn send_icmpv6_parameter_problem_helper(
            CtxPair { core_ctx, bindings_ctx }: &mut FakeIcmpCtx<Ipv6>,
        ) {
            let error = Icmpv6Error::ParameterProblem {
                code: Icmpv6ParameterProblemCode::ErroneousHeaderField,
                pointer: 0,
                allow_dst_multicast: false,
            };
            core_ctx.send_icmp_error_message(
                bindings_ctx,
                Some(&FakeDeviceId),
                Some(FrameDestination::Individual { local: () }),
                TEST_ADDRS_V6.remote_ip.try_into().unwrap(),
                TEST_ADDRS_V6.local_ip.try_into().unwrap(),
                EmptyBuf,
                error,
                0,
                Ipv6Proto::NoNextHeader,
                &Default::default(),
            );
            assert_geq!(
                core_ctx.icmp.tx_counters.parameter_problem.erroneous_header_field.get(),
                1
            );
        }

        /// Call `send_icmpv6_dest_unreachable` with fake values.
        fn send_icmpv6_dest_unreachable_helper(
            CtxPair { core_ctx, bindings_ctx }: &mut FakeIcmpCtx<Ipv6>,
        ) {
            core_ctx.send_icmp_error_message(
                bindings_ctx,
                Some(&FakeDeviceId),
                Some(FrameDestination::Individual { local: () }),
                TEST_ADDRS_V6.remote_ip.try_into().unwrap(),
                TEST_ADDRS_V6.local_ip.try_into().unwrap(),
                EmptyBuf,
                Icmpv6Error::NetUnreachable,
                0,
                Ipv6Proto::NoNextHeader,
                &Default::default(),
            );
            assert_geq!(core_ctx.icmp.tx_counters.dest_unreachable.no_route.get(), 1);
        }

        // Run tests for each function that sends error messages to make sure
        // they're all properly rate limited.

        fn run_test<I: IcmpTestIpExt, W: Fn(u64) -> FakeIcmpCtx<I>, S: Fn(&mut FakeIcmpCtx<I>)>(
            with_errors_per_second: W,
            send: S,
        ) {
            // Note that we could theoretically have more precise tests here
            // (e.g., a test that we send at the correct rate over the long
            // term), but those would amount to testing the `TokenBucket`
            // implementation, which has its own exhaustive tests. Instead, we
            // just have a few sanity checks to make sure that we're actually
            // invoking it when we expect to (as opposed to bypassing it
            // entirely or something).

            // Test that, if no time has elapsed, we can successfully send up to
            // `ERRORS_PER_SECOND` error messages, but no more.

            // Don't use `DEFAULT_ERRORS_PER_SECOND` because it's 2^16 and it
            // makes this test take a long time.
            const ERRORS_PER_SECOND: u64 = 64;

            let mut ctx = with_errors_per_second(ERRORS_PER_SECOND);

            for i in 0..ERRORS_PER_SECOND {
                send(&mut ctx);
                assert_eq!(ctx.core_ctx.icmp.tx_counters.error.get(), i + 1);
            }

            assert_eq!(ctx.core_ctx.icmp.tx_counters.error.get(), ERRORS_PER_SECOND);
            send(&mut ctx);
            assert_eq!(ctx.core_ctx.icmp.tx_counters.error.get(), ERRORS_PER_SECOND);

            // Test that, if we set a rate of 0, we are not able to send any
            // error messages regardless of how much time has elapsed.

            let mut ctx = with_errors_per_second(0);
            send(&mut ctx);
            assert_eq!(ctx.core_ctx.icmp.tx_counters.error.get(), 0);
            ctx.bindings_ctx.timers.instant.sleep(Duration::from_secs(1));
            send(&mut ctx);
            assert_eq!(ctx.core_ctx.icmp.tx_counters.error.get(), 0);
            ctx.bindings_ctx.timers.instant.sleep(Duration::from_secs(1));
            send(&mut ctx);
            assert_eq!(ctx.core_ctx.icmp.tx_counters.error.get(), 0);
        }

        fn with_errors_per_second_v4(errors_per_second: u64) -> FakeIcmpCtx<Ipv4> {
            CtxPair::with_core_ctx(FakeIcmpCoreCtx::with_errors_per_second(errors_per_second))
        }
        run_test::<Ipv4, _, _>(with_errors_per_second_v4, send_icmpv4_ttl_expired_helper);
        run_test::<Ipv4, _, _>(with_errors_per_second_v4, send_icmpv4_parameter_problem_helper);
        run_test::<Ipv4, _, _>(with_errors_per_second_v4, send_icmpv4_dest_unreachable_helper);

        fn with_errors_per_second_v6(errors_per_second: u64) -> FakeIcmpCtx<Ipv6> {
            CtxPair::with_core_ctx(FakeIcmpCoreCtx::with_errors_per_second(errors_per_second))
        }

        run_test::<Ipv6, _, _>(with_errors_per_second_v6, send_icmpv6_ttl_expired_helper);
        run_test::<Ipv6, _, _>(with_errors_per_second_v6, send_icmpv6_packet_too_big_helper);
        run_test::<Ipv6, _, _>(with_errors_per_second_v6, send_icmpv6_parameter_problem_helper);
        run_test::<Ipv6, _, _>(with_errors_per_second_v6, send_icmpv6_dest_unreachable_helper);
    }
}
