// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The User Datagram Protocol (UDP).

use alloc::vec::Vec;
use core::borrow::Borrow;
use core::convert::Infallible as Never;
use core::fmt::Debug;
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;
use core::num::{NonZeroU8, NonZeroU16, NonZeroUsize};
use core::ops::RangeInclusive;

use derivative::Derivative;
use either::Either;
use lock_order::lock::{DelegatedOrderedLockAccess, OrderedLockAccess, OrderedLockRef};
use log::{debug, trace};
use net_types::ip::{GenericOverIp, Ip, IpInvariant, IpVersion, IpVersionMarker, Ipv4, Ipv6};
use net_types::{MulticastAddr, SpecifiedAddr, Witness, ZonedAddr};
use netstack3_base::socket::{
    AddrEntry, AddrIsMappedError, AddrVec, Bound, ConnAddr, ConnInfoAddr, ConnIpAddr,
    EitherIpProto, FoundSockets, IncompatibleError, InsertError, Inserter, ListenerAddr,
    ListenerAddrInfo, ListenerIpAddr, MaybeDualStack, NotDualStackCapableError, RemoveResult,
    ReusePortOption, SetDualStackEnabledError, SharingDomain, ShutdownType, SocketAddrType,
    SocketCookie, SocketIpAddr, SocketMapAddrSpec, SocketMapAddrStateSpec, SocketMapConflictPolicy,
    SocketMapStateSpec, SocketWritableListener,
};
use netstack3_base::socketmap::{IterShadows as _, SocketMap, Tagged};
use netstack3_base::sync::{RwLock, StrongRc};
use netstack3_base::{
    AnyDevice, BidirectionalConverter, ContextPair, CoreTxMetadataContext, CounterContext,
    DeviceIdContext, Inspector, InspectorDeviceExt, InstantContext, IpSocketPropertiesMatcher,
    LocalAddressError, Mark, MarkDomain, Marks, MatcherBindingsTypes, NetworkParsingContext,
    PortAllocImpl, ReferenceNotifiers, RemoveResourceResultWithContext, ResourceCounterContext,
    RngContext, SettingsContext, SocketError, StrongDeviceIdentifier, WeakDeviceIdentifier,
    ZonedAddressError,
};
use netstack3_datagram::{
    self as datagram, BoundDatagramSocketMap, BoundSocketState as DatagramBoundSocketState,
    BoundSocketStateType as DatagramBoundSocketStateType, ConnectError, DatagramApi,
    DatagramBindingsTypes, DatagramBoundStateContext, DatagramFlowId,
    DatagramIpSpecificSocketOptions, DatagramSocketMapSpec, DatagramSocketSet, DatagramSocketSpec,
    DatagramSpecBoundStateContext, DatagramSpecStateContext, DatagramStateContext,
    DualStackBaseIpExt, DualStackConnState, DualStackConverter, DualStackDatagramBoundStateContext,
    DualStackDatagramSpecBoundStateContext, DualStackIpExt, EitherIpSocket, ExpectedConnError,
    ExpectedUnboundError, InUseError, IpExt, IpOptions, MulticastMembershipInterfaceSelector,
    NonDualStackConverter, NonDualStackDatagramBoundStateContext,
    NonDualStackDatagramSpecBoundStateContext, PendingDatagramSocketError,
    SendError as DatagramSendError, SetMulticastMembershipError, SocketInfo,
    SocketState as DatagramSocketState, SocketStateInner as DatagramSocketStateInner,
    WrapOtherStackIpOptions, WrapOtherStackIpOptionsMut,
};
use netstack3_filter::{SocketIngressFilterResult, SocketOpsFilter, SocketOpsFilterBindingContext};
use netstack3_hashmap::hash_map::DefaultHasher;
use netstack3_ip::icmp::IcmpError;
use netstack3_ip::socket::{
    IpSockCreateAndSendError, IpSockCreationError, IpSockSendError, SocketHopLimits,
};
use netstack3_ip::{
    HopLimits, IpHeaderInfo, IpTransportContext, LocalDeliveryPacketInfo,
    MulticastMembershipHandler, ReceiveIpPacketMeta, SocketMetadata, TransparentLocalDelivery,
    TransportIpContext,
};
use netstack3_trace::trace_duration;
use packet::{
    BufferMut, FragmentedByteSlice, NestablePacketBuilder as _, Nested, ParsablePacket, ParseBuffer,
};
use packet_formats::ip::{DscpAndEcn, IpProto, IpProtoExt, Ipv4Proto, Ipv6Proto};
use packet_formats::udp::{UdpPacket, UdpPacketBuilder, UdpPacketRaw, UdpParseArgs};
use thiserror::Error;

use crate::internal::counters::{
    CombinedUdpCounters, UdpCounterContext, UdpCountersWithSocket, UdpCountersWithoutSocket,
};
use crate::internal::diagnostics::{UdpSocketDiagnostics, UdpSocketDiagnosticsSeed};
use crate::internal::settings::UdpSettings;

/// Convenience alias to make names shorter.
pub(crate) type UdpBoundSocketMap<I, D, BT> = BoundDatagramSocketMap<I, D, Udp<BT>>;
/// Tx metadata sent by UDP sockets.
pub type UdpSocketTxMetadata<I, D, BT> = datagram::TxMetadata<I, D, Udp<BT>>;

/// UDP bound sockets, i.e., the UDP demux.
#[derive(Derivative, GenericOverIp)]
#[generic_over_ip(I, Ip)]
#[derivative(Default(bound = ""))]
pub struct BoundSockets<I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes> {
    bound_sockets: UdpBoundSocketMap<I, D, BT>,
}

/// A collection of UDP sockets.
#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct Sockets<I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes> {
    bound: RwLock<BoundSockets<I, D, BT>>,
    // Destroy all_sockets last so the strong references in the demux are
    // dropped before the primary references in the set.
    all_sockets: RwLock<UdpSocketSet<I, D, BT>>,
}

impl<I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes>
    OrderedLockAccess<BoundSockets<I, D, BT>> for Sockets<I, D, BT>
{
    type Lock = RwLock<BoundSockets<I, D, BT>>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.bound)
    }
}

impl<I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes>
    OrderedLockAccess<UdpSocketSet<I, D, BT>> for Sockets<I, D, BT>
{
    type Lock = RwLock<UdpSocketSet<I, D, BT>>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.all_sockets)
    }
}

/// The state associated with the UDP protocol.
///
/// `D` is the device ID type.
#[derive(Derivative, GenericOverIp)]
#[generic_over_ip(I, Ip)]
#[derivative(Default(bound = ""))]
pub struct UdpState<I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes> {
    /// System's UDP sockets.
    pub sockets: Sockets<I, D, BT>,
    /// Stack-wide UDP "with socket" counters.
    pub counters_with_socket: UdpCountersWithSocket<I>,
    /// Stack-wide UDP "without socket" counters.
    pub counters_without_socket: UdpCountersWithoutSocket<I>,
}

/// Uninstantiatable type for implementing [`DatagramSocketSpec`].
pub struct Udp<BT>(PhantomData<BT>, Never);

/// Produces an iterator over eligible receiving socket addresses.
#[cfg(test)]
fn iter_receiving_addrs<I: IpExt, D: WeakDeviceIdentifier>(
    addr: ConnIpAddr<I::Addr, NonZeroU16, UdpRemotePort>,
    device: D,
) -> impl Iterator<Item = AddrVec<I, D, UdpAddrSpec>> {
    netstack3_base::socket::AddrVecIter::with_device(addr.into(), device)
}

fn check_posix_sharing<I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes>(
    new_sharing: Sharing,
    dest: AddrVec<I, D, UdpAddrSpec>,
    socketmap: &SocketMap<AddrVec<I, D, UdpAddrSpec>, Bound<UdpSocketMapStateSpec<I, D, BT>>>,
) -> Result<(), InsertError> {
    // Having a value present at a shadowed address is disqualifying, unless
    // both the new and existing sockets allow port sharing.
    if dest.iter_shadows().any(|a| {
        socketmap.get(&a).is_some_and(|bound| {
            !bound.tag(&a).to_sharing_options().is_shareable_with_new_state(new_sharing)
        })
    }) {
        return Err(InsertError::ShadowAddrExists);
    }

    // Likewise, the presence of a value that shadows the target address is
    // disqualifying unless both allow port sharing.
    match &dest {
        AddrVec::Conn(ConnAddr { ip: _, device: None }) | AddrVec::Listen(_) => {
            if socketmap.descendant_counts(&dest).any(|(tag, _): &(_, NonZeroUsize)| {
                !tag.to_sharing_options().is_shareable_with_new_state(new_sharing)
            }) {
                return Err(InsertError::WouldShadowExisting);
            }
        }
        AddrVec::Conn(ConnAddr { ip: _, device: Some(_) }) => {
            // No need to check shadows here because there are no addresses
            // that shadow a ConnAddr with a device.
            debug_assert_eq!(socketmap.descendant_counts(&dest).len(), 0)
        }
    }

    // There are a few combinations of addresses that can conflict with
    // each other even though there is not a direct shadowing relationship:
    // - listener address with device and connected address without.
    // - "any IP" listener with device and specific IP listener without.
    // - "any IP" listener with device and connected address without.
    //
    // The complication is that since these pairs of addresses don't have a
    // direct shadowing relationship, it's not possible to query for one
    // from the other in the socketmap without a linear scan. Instead. we
    // rely on the fact that the tag values in the socket map have different
    // values for entries with and without device IDs specified.
    fn conflict_exists<I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes>(
        new_sharing: Sharing,
        socketmap: &SocketMap<AddrVec<I, D, UdpAddrSpec>, Bound<UdpSocketMapStateSpec<I, D, BT>>>,
        addr: impl Into<AddrVec<I, D, UdpAddrSpec>>,
        mut is_conflicting: impl FnMut(&AddrVecTag) -> bool,
    ) -> bool {
        socketmap.descendant_counts(&addr.into()).any(|(tag, _): &(_, NonZeroUsize)| {
            is_conflicting(tag)
                && !tag.to_sharing_options().is_shareable_with_new_state(new_sharing)
        })
    }

    let found_indirect_conflict = match dest {
        AddrVec::Listen(ListenerAddr {
            ip: ListenerIpAddr { addr: None, identifier },
            device: Some(_device),
        }) => {
            // An address with a device will shadow an any-IP listener
            // `dest` with a device so we only need to check for addresses
            // without a device. Likewise, an any-IP listener will directly
            // shadow `dest`, so an indirect conflict can only come from a
            // specific listener or connected socket (without a device).
            conflict_exists(
                new_sharing,
                socketmap,
                ListenerAddr { ip: ListenerIpAddr { addr: None, identifier }, device: None },
                |AddrVecTag { has_device, addr_type, sharing: _ }| {
                    !*has_device
                        && match addr_type {
                            SocketAddrType::SpecificListener | SocketAddrType::Connected => true,
                            SocketAddrType::AnyListener => false,
                        }
                },
            )
        }
        AddrVec::Listen(ListenerAddr {
            ip: ListenerIpAddr { addr: Some(ip), identifier },
            device: Some(_device),
        }) => {
            // A specific-IP listener `dest` with a device will be shadowed
            // by a connected socket with a device and will shadow
            // specific-IP addresses without a device and any-IP listeners
            // with and without devices. That means an indirect conflict can
            // only come from a connected socket without a device.
            conflict_exists(
                new_sharing,
                socketmap,
                ListenerAddr { ip: ListenerIpAddr { addr: Some(ip), identifier }, device: None },
                |AddrVecTag { has_device, addr_type, sharing: _ }| {
                    !*has_device
                        && match addr_type {
                            SocketAddrType::Connected => true,
                            SocketAddrType::AnyListener | SocketAddrType::SpecificListener => false,
                        }
                },
            )
        }
        AddrVec::Listen(ListenerAddr {
            ip: ListenerIpAddr { addr: Some(_), identifier },
            device: None,
        }) => {
            // A specific-IP listener `dest` without a device will be
            // shadowed by a specific-IP listener with a device and by any
            // connected socket (with or without a device).  It will also
            // shadow an any-IP listener without a device, which means an
            // indirect conflict can only come from an any-IP listener with
            // a device.
            conflict_exists(
                new_sharing,
                socketmap,
                ListenerAddr { ip: ListenerIpAddr { addr: None, identifier }, device: None },
                |AddrVecTag { has_device, addr_type, sharing: _ }| {
                    *has_device
                        && match addr_type {
                            SocketAddrType::AnyListener => true,
                            SocketAddrType::SpecificListener | SocketAddrType::Connected => false,
                        }
                },
            )
        }
        AddrVec::Conn(ConnAddr {
            ip: ConnIpAddr { local: (local_ip, local_identifier), remote: _ },
            device: None,
        }) => {
            // A connected socket `dest` without a device shadows listeners
            // without devices, and is shadowed by a connected socket with
            // a device. It can indirectly conflict with listening sockets
            // with devices.

            // Check for specific-IP listeners with devices, which would
            // indirectly conflict.
            conflict_exists(
                new_sharing,
                socketmap,
                ListenerAddr {
                    ip: ListenerIpAddr {
                        addr: Some(local_ip),
                        identifier: local_identifier.clone(),
                    },
                    device: None,
                },
                |AddrVecTag { has_device, addr_type, sharing: _ }| {
                    *has_device
                        && match addr_type {
                            SocketAddrType::SpecificListener => true,
                            SocketAddrType::AnyListener | SocketAddrType::Connected => false,
                        }
                },
            ) ||
            // Check for any-IP listeners with devices since they conflict.
            // Note that this check cannot be combined with the one above
            // since they examine tag counts for different addresses. While
            // the counts of tags matched above *will* also be propagated to
            // the any-IP listener entry, they would be indistinguishable
            // from non-conflicting counts. For a connected address with
            // `Some(local_ip)`, the descendant counts at the listener
            // address with `addr = None` would include any
            // `SpecificListener` tags for both addresses with
            // `Some(local_ip)` and `Some(other_local_ip)`. The former
            // indirectly conflicts with `dest` but the latter does not,
            // hence this second distinct check.
            conflict_exists(
                new_sharing,
                socketmap,
                ListenerAddr {
                    ip: ListenerIpAddr { addr: None, identifier: local_identifier },
                    device: None,
                },
                |AddrVecTag { has_device, addr_type, sharing: _ }| {
                    *has_device
                        && match addr_type {
                            SocketAddrType::AnyListener => true,
                            SocketAddrType::SpecificListener | SocketAddrType::Connected => false,
                        }
                },
            )
        }
        AddrVec::Listen(ListenerAddr {
            ip: ListenerIpAddr { addr: None, identifier: _ },
            device: _,
        }) => false,
        AddrVec::Conn(ConnAddr { ip: _, device: Some(_device) }) => false,
    };
    if found_indirect_conflict { Err(InsertError::IndirectConflict) } else { Ok(()) }
}

/// The remote port for a UDP socket.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
pub enum UdpRemotePort {
    /// The remote port is set to the following value.
    Set(NonZeroU16),
    /// The remote port is unset (i.e. "0") value. An unset remote port is
    /// treated specially in a few places:
    ///
    /// 1) Attempting to send to an unset remote port results in a
    /// [`UdpSerializeError::RemotePortUnset`] error. Note that this behavior
    /// diverges from Linux, which does allow sending to a remote_port of 0
    /// (supported by `send` but not `send_to`). The rationale for this
    /// divergence originates from RFC 8085 Section 5.1:
    ///
    ///    A UDP sender SHOULD NOT use a source port value of zero.  A source
    ///    port number that cannot be easily determined from the address or
    ///    payload type provides protection at the receiver from data injection
    ///    attacks by off-path devices. A UDP receiver SHOULD NOT bind to port
    ///    zero.
    ///
    ///    Applications SHOULD implement receiver port and address checks at the
    ///    application layer or explicitly request that the operating system
    ///    filter the received packets to prevent receiving packets with an
    ///    arbitrary port.  This measure is designed to provide additional
    ///    protection from data injection attacks from an off-path source (where
    ///    the port values may not be known).
    ///
    /// Combined, these two stanzas recommend hosts discard incoming traffic
    /// destined to remote port 0 for security reasons. Thus we choose to not
    /// allow hosts to send such packets under the assumption that it will be
    /// dropped by the receiving end.
    ///
    /// 2) A socket connected to a remote host on port 0 will not receive any
    /// packets from the remote host. This is because the
    /// [`BoundSocketMap::lookup`] implementation only delivers packets that
    /// specify a remote port to connected sockets with an exact match. Further,
    /// packets that don't specify a remote port are only delivered to listener
    /// sockets. This diverges from Linux (which treats a remote_port of 0) as
    /// wild card. If and when a concrete need for such behavior is identified,
    /// the [`BoundSocketMap`] lookup behavior can be adjusted accordingly.
    Unset,
}

impl From<NonZeroU16> for UdpRemotePort {
    fn from(p: NonZeroU16) -> Self {
        Self::Set(p)
    }
}

impl From<u16> for UdpRemotePort {
    fn from(p: u16) -> Self {
        NonZeroU16::new(p).map(UdpRemotePort::from).unwrap_or(UdpRemotePort::Unset)
    }
}

impl From<UdpRemotePort> for u16 {
    fn from(p: UdpRemotePort) -> Self {
        match p {
            UdpRemotePort::Unset => 0,
            UdpRemotePort::Set(p) => p.into(),
        }
    }
}

/// Uninstantiatable type for implementing [`SocketMapAddrSpec`].
pub enum UdpAddrSpec {}

impl SocketMapAddrSpec for UdpAddrSpec {
    type RemoteIdentifier = UdpRemotePort;
    type LocalIdentifier = NonZeroU16;
}

pub struct UdpSocketMapStateSpec<I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes>(
    PhantomData<(I, D, BT)>,
    Never,
);

impl<I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes> SocketMapStateSpec
    for UdpSocketMapStateSpec<I, D, BT>
{
    type ListenerId = I::DualStackBoundSocketId<D, Udp<BT>>;
    type ConnId = I::DualStackBoundSocketId<D, Udp<BT>>;

    type AddrVecTag = AddrVecTag;

    type ListenerSharingState = Sharing;
    type ConnSharingState = Sharing;

    type ListenerAddrState = AddrState<Self::ListenerId>;

    type ConnAddrState = AddrState<Self::ConnId>;
    fn listener_tag(
        ListenerAddrInfo { has_device, specified_addr }: ListenerAddrInfo,
        state: &Self::ListenerAddrState,
    ) -> Self::AddrVecTag {
        AddrVecTag {
            has_device,
            addr_type: specified_addr
                .then_some(SocketAddrType::SpecificListener)
                .unwrap_or(SocketAddrType::AnyListener),
            sharing: state.to_sharing_options(),
        }
    }
    fn connected_tag(has_device: bool, state: &Self::ConnAddrState) -> Self::AddrVecTag {
        AddrVecTag {
            has_device,
            addr_type: SocketAddrType::Connected,
            sharing: state.to_sharing_options(),
        }
    }
}

impl<AA, I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes>
    SocketMapConflictPolicy<AA, Sharing, I, D, UdpAddrSpec> for UdpSocketMapStateSpec<I, D, BT>
where
    AA: Into<AddrVec<I, D, UdpAddrSpec>> + Clone,
{
    fn check_insert_conflicts(
        new_sharing_state: &Sharing,
        addr: &AA,
        socketmap: &SocketMap<AddrVec<I, D, UdpAddrSpec>, Bound<Self>>,
    ) -> Result<(), InsertError> {
        check_posix_sharing(*new_sharing_state, addr.clone().into(), socketmap)
    }
}

/// State held for IPv6 sockets related to dual-stack operation.
#[derive(Clone, Derivative)]
#[derivative(Default(bound = ""), Debug(bound = ""))]
pub struct DualStackSocketState<D: WeakDeviceIdentifier> {
    /// Whether dualstack operations are enabled on this socket.
    /// Match Linux's behavior by enabling dualstack operations by default.
    #[derivative(Default(value = "true"))]
    dual_stack_enabled: bool,

    /// Send options used when sending on the IPv4 stack.
    socket_options: DatagramIpSpecificSocketOptions<Ipv4, D>,
}

/// Serialization errors for Udp Packets.
#[derive(Debug, Error)]
pub enum UdpSerializeError {
    /// Disallow sending packets with a remote port of 0. See
    /// [`UdpRemotePort::Unset`] for the rationale.
    #[error("sending packets with a remote port of 0 is not allowed")]
    RemotePortUnset,
}

impl<BT: UdpBindingsTypes> DatagramSocketSpec for Udp<BT> {
    const NAME: &'static str = "UDP";

    type AddrSpec = UdpAddrSpec;
    type SocketId<I: IpExt, D: WeakDeviceIdentifier> = UdpSocketId<I, D, BT>;
    type WeakSocketId<I: IpExt, D: WeakDeviceIdentifier> = WeakUdpSocketId<I, D, BT>;
    type OtherStackIpOptions<I: IpExt, D: WeakDeviceIdentifier> =
        I::OtherStackIpOptions<DualStackSocketState<D>>;
    type ListenerIpAddr<I: IpExt> = I::DualStackListenerIpAddr<NonZeroU16>;
    type ConnIpAddr<I: IpExt> = I::DualStackConnIpAddr<Self>;
    type ConnStateExtra = ();
    type ConnState<I: IpExt, D: WeakDeviceIdentifier> = I::DualStackConnState<D, Self>;
    type SocketMapSpec<I: IpExt, D: WeakDeviceIdentifier> = UdpSocketMapStateSpec<I, D, BT>;
    type SharingState = Sharing;

    type Serializer<I: IpExt, B: BufferMut> = Nested<B, UdpPacketBuilder<I::Addr>>;
    type SerializeError = UdpSerializeError;

    type ExternalData<I: Ip> = BT::ExternalData<I>;
    type Settings = UdpSettings;
    type Counters<I: Ip> = UdpCountersWithSocket<I>;
    type SocketWritableListener = BT::SocketWritableListener;
    type SendToken = BT::SendToken;

    fn ip_proto<I: IpProtoExt>() -> I::Proto {
        IpProto::Udp.into()
    }

    fn make_bound_socket_map_id<I: IpExt, D: WeakDeviceIdentifier>(
        s: &Self::SocketId<I, D>,
    ) -> I::DualStackBoundSocketId<D, Udp<BT>> {
        I::into_dual_stack_bound_socket_id(s.clone())
    }

    const FIXED_HEADER_SIZE: usize = packet_formats::udp::HEADER_BYTES;

    fn make_packet<I: IpExt, B: BufferMut>(
        body: B,
        addr: &ConnIpAddr<I::Addr, NonZeroU16, UdpRemotePort>,
    ) -> Result<Self::Serializer<I, B>, UdpSerializeError> {
        let ConnIpAddr { local: (local_ip, local_port), remote: (remote_ip, remote_port) } = addr;
        let remote_port = match remote_port {
            UdpRemotePort::Unset => return Err(UdpSerializeError::RemotePortUnset),
            UdpRemotePort::Set(remote_port) => *remote_port,
        };
        Ok(UdpPacketBuilder::new(local_ip.addr(), remote_ip.addr(), Some(*local_port), remote_port)
            .wrap_body(body))
    }

    fn try_alloc_listen_identifier<I: IpExt, D: WeakDeviceIdentifier>(
        rng: &mut impl RngContext,
        is_available: impl Fn(NonZeroU16) -> Result<(), InUseError>,
    ) -> Option<NonZeroU16> {
        try_alloc_listen_port::<I, D, BT>(rng, is_available)
    }

    fn conn_info_from_state<I: IpExt, D: WeakDeviceIdentifier>(
        state: &Self::ConnState<I, D>,
    ) -> datagram::ConnInfo<I::Addr, D> {
        let ConnAddr { ip, device } = I::conn_addr_from_state(state);
        let ConnInfoAddr { local: (local_ip, local_port), remote: (remote_ip, remote_port) } =
            ip.into();
        datagram::ConnInfo::new(local_ip, local_port, remote_ip, remote_port.into(), || {
            // The invariant that a zone is present if needed is upheld by connect.
            device.clone().expect("device must be bound for addresses that require zones")
        })
    }

    fn try_alloc_local_id<I: IpExt, D: WeakDeviceIdentifier, BC: RngContext>(
        bound: &UdpBoundSocketMap<I, D, BT>,
        bindings_ctx: &mut BC,
        flow: datagram::DatagramFlowId<I::Addr, UdpRemotePort>,
    ) -> Option<NonZeroU16> {
        let mut rng = bindings_ctx.rng();
        netstack3_base::simple_randomized_port_alloc(&mut rng, &flow, &UdpPortAlloc(bound), &())
            .map(|p| NonZeroU16::new(p).expect("ephemeral ports should be non-zero"))
    }

    fn upgrade_socket_id<I: IpExt, D: WeakDeviceIdentifier>(
        id: &Self::WeakSocketId<I, D>,
    ) -> Option<Self::SocketId<I, D>> {
        id.upgrade()
    }

    fn downgrade_socket_id<I: IpExt, D: WeakDeviceIdentifier>(
        id: &Self::SocketId<I, D>,
    ) -> Self::WeakSocketId<I, D> {
        UdpSocketId::downgrade(id)
    }
}

impl<I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes>
    DatagramSocketMapSpec<I, D, UdpAddrSpec> for UdpSocketMapStateSpec<I, D, BT>
{
    type BoundSocketId = I::DualStackBoundSocketId<D, Udp<BT>>;
}

enum LookupResult<'a, I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes> {
    Conn(
        &'a I::DualStackBoundSocketId<D, Udp<BT>>,
        ConnAddr<ConnIpAddr<I::Addr, NonZeroU16, UdpRemotePort>, D>,
    ),
    Listener(
        &'a I::DualStackBoundSocketId<D, Udp<BT>>,
        ListenerAddr<ListenerIpAddr<I::Addr, NonZeroU16>, D>,
    ),
}

#[derive(Hash, Copy, Clone)]
struct SocketSelectorParams<I: Ip, A: AsRef<I::Addr>> {
    src_ip: I::Addr,
    dst_ip: A,
    src_port: u16,
    dst_port: u16,
    _ip: IpVersionMarker<I>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct LoadBalancedEntry<T> {
    id: T,
    sharing_domain: SharingDomain,
    reuse_addr: bool,
}

#[derive(Debug, Eq, PartialEq)]
pub enum AddrState<T> {
    Exclusive(T),
    Shared {
        // Entries with the SO_REUSEADDR flag. If this list is not empty then
        // new packets are delivered to the last socket in this list.
        priority: Vec<T>,

        // Entries with the SO_REUSEPORT flag. Some of them may have
        // SO_REUSEADDR flag set as well. If `priority` list is empty then
        // incoming packets are load-balanced between sockets in this list.
        load_balanced: Vec<LoadBalancedEntry<T>>,
    },
}

#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq, Default)]
pub struct Sharing {
    reuse_addr: bool,
    reuse_port: ReusePortOption,
}

impl Sharing {
    pub(crate) fn is_shareable_with_new_state(&self, new_state: Sharing) -> bool {
        let Sharing { reuse_addr, reuse_port } = self;
        let Sharing { reuse_addr: new_reuse_addr, reuse_port: new_reuse_port } = new_state;
        (*reuse_addr && new_reuse_addr) || reuse_port.is_shareable_with(&new_reuse_port)
    }
}

#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
pub struct AddrVecTag {
    pub(crate) has_device: bool,
    pub(crate) addr_type: SocketAddrType,
    pub(crate) sharing: Sharing,
}

pub(crate) trait ToSharingOptions {
    fn to_sharing_options(&self) -> Sharing;
}

impl ToSharingOptions for AddrVecTag {
    fn to_sharing_options(&self) -> Sharing {
        let AddrVecTag { has_device: _, addr_type: _, sharing } = self;
        *sharing
    }
}

impl<T> ToSharingOptions for AddrState<T> {
    fn to_sharing_options(&self) -> Sharing {
        match self {
            AddrState::Exclusive(_) => {
                Sharing { reuse_addr: false, reuse_port: ReusePortOption::Disabled }
            }
            AddrState::Shared { priority, load_balanced } => {
                // All sockets in `priority` have `REUSE_ADDR` flag set. Check
                // that all sockets in `load_balanced` have it set as well.
                let reuse_addr = load_balanced.iter().all(|e| e.reuse_addr);

                // All sockets in `load_balanced` have `REUSE_PORT` flag set,
                // while the sockets in `priority` don't. `REUSE_PORT` requires
                // all sockets to have the same sharing domain.
                let reuse_port = if priority.is_empty() {
                    load_balanced
                        .iter()
                        .map(|e| Some(e.sharing_domain))
                        .reduce(|acc, sharing_domain| match (acc, sharing_domain) {
                            (Some(acc), Some(sharing_domain)) if acc == sharing_domain => {
                                Some(sharing_domain)
                            }
                            _ => None,
                        })
                        .flatten()
                } else {
                    None
                };
                let reuse_port = match reuse_port {
                    Some(domain) => ReusePortOption::Enabled(domain),
                    None => ReusePortOption::Disabled,
                };

                Sharing { reuse_addr, reuse_port }
            }
        }
    }
}

impl<T> ToSharingOptions for (T, Sharing) {
    fn to_sharing_options(&self) -> Sharing {
        let (_state, sharing) = self;
        *sharing
    }
}

pub struct SocketMapAddrInserter<'a, I> {
    state: &'a mut AddrState<I>,
    sharing_state: Sharing,
}

impl<'a, I> Inserter<I> for SocketMapAddrInserter<'a, I> {
    fn insert(self, id: I) {
        match self {
            Self {
                state: _,
                sharing_state: Sharing { reuse_addr: false, reuse_port: ReusePortOption::Disabled },
            }
            | Self { state: AddrState::Exclusive(_), sharing_state: _ } => {
                panic!("Can't insert entry in a non-shareable entry")
            }

            // If only `SO_REUSEADDR` flag is set then insert the entry in the `priority` list.
            Self {
                state: AddrState::Shared { priority, load_balanced: _ },
                sharing_state: Sharing { reuse_addr: true, reuse_port: ReusePortOption::Disabled },
            } => priority.push(id),

            // If `SO_REUSEPORT` flag is set then insert the entry in the `load_balanced` list.
            Self {
                state: AddrState::Shared { priority: _, load_balanced },
                sharing_state:
                    Sharing { reuse_addr, reuse_port: ReusePortOption::Enabled(sharing_domain) },
            } => load_balanced.push(LoadBalancedEntry { id, reuse_addr, sharing_domain }),
        }
    }
}

impl<I: Debug + Eq> SocketMapAddrStateSpec for AddrState<I> {
    type Id = I;
    type SharingState = Sharing;
    type Inserter<'a>
        = SocketMapAddrInserter<'a, I>
    where
        I: 'a;

    fn new(new_sharing_state: &Sharing, id: I) -> Self {
        match new_sharing_state {
            Sharing { reuse_addr: false, reuse_port: ReusePortOption::Disabled } => {
                Self::Exclusive(id)
            }
            Sharing { reuse_addr: true, reuse_port: ReusePortOption::Disabled } => {
                Self::Shared { priority: Vec::from([id]), load_balanced: Vec::new() }
            }
            Sharing { reuse_addr, reuse_port: ReusePortOption::Enabled(sharing_domain) } => {
                Self::Shared {
                    priority: Vec::new(),
                    load_balanced: Vec::from([LoadBalancedEntry {
                        id,
                        reuse_addr: *reuse_addr,
                        sharing_domain: *sharing_domain,
                    }]),
                }
            }
        }
    }

    fn contains_id(&self, id: &Self::Id) -> bool {
        match self {
            Self::Exclusive(x) => id == x,
            Self::Shared { priority, load_balanced } => {
                priority.contains(id) || load_balanced.iter().any(|e| e.id == *id)
            }
        }
    }

    fn try_get_inserter<'a, 'b>(
        &'b mut self,
        new_sharing_state: &'a Sharing,
    ) -> Result<SocketMapAddrInserter<'b, I>, IncompatibleError> {
        self.could_insert(new_sharing_state)?;
        Ok(SocketMapAddrInserter { state: self, sharing_state: *new_sharing_state })
    }

    fn could_insert(&self, new_sharing_state: &Sharing) -> Result<(), IncompatibleError> {
        self.to_sharing_options()
            .is_shareable_with_new_state(*new_sharing_state)
            .then_some(())
            .ok_or(IncompatibleError)
    }

    fn remove_by_id(&mut self, id: I) -> RemoveResult {
        match self {
            Self::Exclusive(_) => RemoveResult::IsLast,
            Self::Shared { priority, load_balanced } => {
                if let Some(pos) = priority.iter().position(|i| *i == id) {
                    let _removed: I = priority.remove(pos);
                } else {
                    let pos = load_balanced
                        .iter()
                        .position(|e| e.id == id)
                        .expect("couldn't find ID to remove");
                    let _removed: LoadBalancedEntry<I> = load_balanced.remove(pos);
                }

                if priority.is_empty() && load_balanced.is_empty() {
                    RemoveResult::IsLast
                } else {
                    RemoveResult::Success
                }
            }
        }
    }

    fn sharing_state(&self) -> Self::SharingState {
        self.to_sharing_options()
    }
}

impl<T> AddrState<T> {
    fn select_receiver<I: Ip, A: AsRef<I::Addr> + Hash>(
        &self,
        selector: SocketSelectorParams<I, A>,
    ) -> &T {
        match self {
            AddrState::Exclusive(id) => id,
            AddrState::Shared { priority, load_balanced } => {
                if let Some(id) = priority.last() {
                    id
                } else if load_balanced.len() == 1 {
                    &load_balanced[0].id
                } else {
                    let mut hasher = DefaultHasher::new();
                    selector.hash(&mut hasher);
                    let index: usize = hasher.finish() as usize % load_balanced.len();
                    &load_balanced[index].id
                }
            }
        }
    }

    fn first(&self) -> &T {
        match self {
            AddrState::Exclusive(id) => id,
            AddrState::Shared { priority, load_balanced } => {
                if let Some(id) = priority.last() {
                    id
                } else {
                    &load_balanced[0].id
                }
            }
        }
    }

    fn collect_all_ids(&self) -> impl Iterator<Item = &'_ T> {
        match self {
            AddrState::Exclusive(id) => Either::Left(core::iter::once(id)),
            AddrState::Shared { priority, load_balanced } => {
                Either::Right(priority.iter().chain(load_balanced.iter().map(|i| &i.id)))
            }
        }
    }
}

/// Finds the socket(s) that should receive an incoming packet.
///
/// Uses the provided addresses and receiving device to look up sockets that
/// should receive a matching incoming packet. The returned iterator may
/// yield 0, 1, or multiple sockets.
fn lookup<'s, I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes>(
    bound: &'s UdpBoundSocketMap<I, D, BT>,
    (src_ip, src_port): (Option<SocketIpAddr<I::Addr>>, Option<NonZeroU16>),
    (dst_ip, dst_port): (SocketIpAddr<I::Addr>, NonZeroU16),
    device: D,
    broadcast: Option<I::BroadcastMarker>,
) -> impl Iterator<Item = LookupResult<'s, I, D, BT>> + 's {
    let matching_entries = bound.iter_receivers(
        (src_ip, src_port.map(UdpRemotePort::from)),
        (dst_ip, dst_port),
        device,
        broadcast,
    );
    match matching_entries {
        None => Either::Left(None),
        Some(FoundSockets::Single(entry)) => {
            Either::Left(Some(match entry {
                AddrEntry::Listen(state, l) => {
                    let selector = SocketSelectorParams::<I, SpecifiedAddr<I::Addr>> {
                        src_ip: src_ip.map_or(I::UNSPECIFIED_ADDRESS, SocketIpAddr::addr),
                        dst_ip: dst_ip.into(),
                        src_port: src_port.map_or(0, NonZeroU16::get),
                        dst_port: dst_port.get(),
                        _ip: IpVersionMarker::default(),
                    };
                    LookupResult::Listener(state.select_receiver(selector), l)
                }
                AddrEntry::Conn(state, c) => {
                    // Always take the first socket when there are multiple
                    // connected sockets. We cannot load-balance between
                    // connected sockets because the source address is always
                    // the same. This also ensures that we return a result
                    // consistent with `early_demux_ip_packet()`.
                    LookupResult::Conn(state.first(), c)
                }
            }))
        }

        Some(FoundSockets::Multicast(entries)) => {
            Either::Right(entries.into_iter().flat_map(|entry| match entry {
                AddrEntry::Listen(state, l) => Either::Left(
                    state.collect_all_ids().map(move |id| LookupResult::Listener(id, l.clone())),
                ),
                AddrEntry::Conn(state, c) => Either::Right(
                    state.collect_all_ids().map(move |id| LookupResult::Conn(id, c.clone())),
                ),
            }))
        }
    }
    .into_iter()
}

/// Helper function to allocate a listen port.
///
/// Finds a random ephemeral port that is not in the provided `used_ports` set.
fn try_alloc_listen_port<I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes>(
    bindings_ctx: &mut impl RngContext,
    is_available: impl Fn(NonZeroU16) -> Result<(), InUseError>,
) -> Option<NonZeroU16> {
    let mut port = UdpPortAlloc::<I, D, BT>::rand_ephemeral(&mut bindings_ctx.rng());
    for _ in UdpPortAlloc::<I, D, BT>::EPHEMERAL_RANGE {
        // We can unwrap here because we know that the EPHEMERAL_RANGE doesn't
        // include 0.
        let tryport = NonZeroU16::new(port.get()).unwrap();
        match is_available(tryport) {
            Ok(()) => return Some(tryport),
            Err(InUseError {}) => port.next(),
        }
    }
    None
}

struct UdpPortAlloc<'a, I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes>(
    &'a UdpBoundSocketMap<I, D, BT>,
);

impl<I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes> PortAllocImpl
    for UdpPortAlloc<'_, I, D, BT>
{
    const EPHEMERAL_RANGE: RangeInclusive<u16> = 49152..=65535;
    type Id = DatagramFlowId<I::Addr, UdpRemotePort>;
    type PortAvailableArg = ();

    fn is_port_available(&self, id: &Self::Id, local_port: u16, (): &()) -> bool {
        let Self(socketmap) = self;
        // We can safely unwrap here, because the ports received in
        // `is_port_available` are guaranteed to be in `EPHEMERAL_RANGE`.
        let local_port = NonZeroU16::new(local_port).unwrap();
        let DatagramFlowId { local_ip, remote_ip, remote_id } = id;
        let conn = ConnAddr {
            ip: ConnIpAddr { local: (*local_ip, local_port), remote: (*remote_ip, *remote_id) },
            device: None,
        };

        // A port is free if there are no sockets currently using it, and if
        // there are no sockets that are shadowing it.
        AddrVec::from(conn).iter_shadows().all(|a| match &a {
            AddrVec::Listen(l) => socketmap.listeners().get_by_addr(&l).is_none(),
            AddrVec::Conn(c) => socketmap.conns().get_by_addr(&c).is_none(),
        } && socketmap.get_shadower_counts(&a) == 0)
    }
}

/// A UDP socket.
#[derive(GenericOverIp, Derivative)]
#[derivative(Eq(bound = ""), PartialEq(bound = ""), Hash(bound = ""))]
#[generic_over_ip(I, Ip)]
pub struct UdpSocketId<I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes>(
    datagram::StrongRc<I, D, Udp<BT>>,
);

impl<I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes> UdpSocketId<I, D, BT> {
    /// Returns `SocketCookie` for the socket.
    pub fn socket_cookie(&self) -> SocketCookie {
        let Self(inner) = self;
        SocketCookie::new(inner.resource_token())
    }

    /// Returns `SocketInfo` for the socket.
    pub fn socket_info(&self) -> netstack3_base::socket::SocketInfo {
        netstack3_base::socket::SocketInfo {
            proto: I::map_ip(
                (),
                |()| EitherIpProto::V4(Ipv4Proto::Proto(IpProto::Udp)),
                |()| EitherIpProto::V6(Ipv6Proto::Proto(IpProto::Udp)),
            ),
            cookie: self.socket_cookie(),
        }
    }
}

impl<CC, I, BT> SocketMetadata<CC> for UdpSocketId<I, CC::WeakDeviceId, BT>
where
    CC: StateContext<I, BT>,
    I: IpExt,
    BT: UdpBindingsContext<I, CC::DeviceId>,
{
    fn socket_info(&self, _core_ctx: &mut CC) -> netstack3_base::socket::SocketInfo {
        self.socket_info()
    }

    fn marks(&self, core_ctx: &mut CC) -> Marks {
        core_ctx.with_socket_state(self, |_core_ctx, state| state.options().marks().clone())
    }
}

impl<I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes> Clone for UdpSocketId<I, D, BT> {
    #[cfg_attr(feature = "instrumented", track_caller)]
    fn clone(&self) -> Self {
        let Self(rc) = self;
        Self(StrongRc::clone(rc))
    }
}

impl<I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes>
    From<datagram::StrongRc<I, D, Udp<BT>>> for UdpSocketId<I, D, BT>
{
    fn from(value: datagram::StrongRc<I, D, Udp<BT>>) -> Self {
        Self(value)
    }
}

impl<I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes>
    Borrow<datagram::StrongRc<I, D, Udp<BT>>> for UdpSocketId<I, D, BT>
{
    fn borrow(&self) -> &datagram::StrongRc<I, D, Udp<BT>> {
        let Self(rc) = self;
        rc
    }
}

impl<I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes> PartialEq<WeakUdpSocketId<I, D, BT>>
    for UdpSocketId<I, D, BT>
{
    fn eq(&self, other: &WeakUdpSocketId<I, D, BT>) -> bool {
        let Self(rc) = self;
        let WeakUdpSocketId(weak) = other;
        StrongRc::weak_ptr_eq(rc, weak)
    }
}

impl<I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes> Debug for UdpSocketId<I, D, BT> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let Self(rc) = self;
        f.debug_tuple("UdpSocketId").field(&StrongRc::debug_id(rc)).finish()
    }
}

impl<I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes>
    DelegatedOrderedLockAccess<UdpSocketState<I, D, BT>> for UdpSocketId<I, D, BT>
{
    type Inner = datagram::ReferenceState<I, D, Udp<BT>>;
    fn delegate_ordered_lock_access(&self) -> &Self::Inner {
        let Self(rc) = self;
        &*rc
    }
}

impl<I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes> UdpSocketId<I, D, BT> {
    /// Returns the inner state for this socket, sidestepping locking
    /// mechanisms.
    #[cfg(any(test, feature = "testutils"))]
    pub fn state(&self) -> &RwLock<UdpSocketState<I, D, BT>> {
        let Self(rc) = self;
        rc.state()
    }

    /// Returns a means to debug outstanding references to this socket.
    pub fn debug_references(&self) -> impl Debug {
        let Self(rc) = self;
        StrongRc::debug_references(rc)
    }

    /// Downgrades this ID to a weak reference.
    pub fn downgrade(&self) -> WeakUdpSocketId<I, D, BT> {
        let Self(rc) = self;
        WeakUdpSocketId(StrongRc::downgrade(rc))
    }

    /// Returns external data associated with this socket.
    pub fn external_data(&self) -> &BT::ExternalData<I> {
        let Self(rc) = self;
        rc.external_data()
    }

    /// Returns the counters tracked for this socket.
    pub fn counters(&self) -> &UdpCountersWithSocket<I> {
        let Self(rc) = self;
        rc.counters()
    }
}

/// A weak reference to a UDP socket.
#[derive(GenericOverIp, Derivative)]
#[derivative(Eq(bound = ""), PartialEq(bound = ""), Hash(bound = ""), Clone(bound = ""))]
#[generic_over_ip(I, Ip)]
pub struct WeakUdpSocketId<I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes>(
    datagram::WeakRc<I, D, Udp<BT>>,
);

impl<I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes> PartialEq<UdpSocketId<I, D, BT>>
    for WeakUdpSocketId<I, D, BT>
{
    fn eq(&self, other: &UdpSocketId<I, D, BT>) -> bool {
        PartialEq::eq(other, self)
    }
}

impl<I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes> Debug for WeakUdpSocketId<I, D, BT> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let Self(rc) = self;
        f.debug_tuple("WeakUdpSocketId").field(&rc.debug_id()).finish()
    }
}

impl<I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes> WeakUdpSocketId<I, D, BT> {
    #[cfg_attr(feature = "instrumented", track_caller)]
    pub fn upgrade(&self) -> Option<UdpSocketId<I, D, BT>> {
        let Self(rc) = self;
        rc.upgrade().map(UdpSocketId)
    }
}

/// A set containing all UDP sockets.
pub type UdpSocketSet<I, D, BT> = DatagramSocketSet<I, D, Udp<BT>>;
/// A UDP socket's state.
pub type UdpSocketState<I, D, BT> = DatagramSocketState<I, D, Udp<BT>>;

/// Auxiliary information about an incoming UDP packet.
#[derive(Debug, GenericOverIp, Clone, PartialEq, Eq)]
#[generic_over_ip(I, Ip)]
pub struct UdpPacketMeta<I: Ip> {
    /// Source address specified in the IP header.
    pub src_ip: I::Addr,

    /// Source port.
    pub src_port: Option<NonZeroU16>,

    /// Destination address specified in the IP header.
    pub dst_ip: I::Addr,

    /// Destination port.
    pub dst_port: NonZeroU16,

    /// DSCP and ECN values received in Traffic Class or TOS field.
    pub dscp_and_ecn: DscpAndEcn,
}

impl UdpPacketMeta<Ipv4> {
    fn to_ipv6_mapped(&self) -> UdpPacketMeta<Ipv6> {
        let Self { dst_ip, dst_port, src_ip, src_port, dscp_and_ecn } = self;
        UdpPacketMeta {
            dst_ip: dst_ip.to_ipv6_mapped().get(),
            dst_port: *dst_port,
            src_ip: src_ip.to_ipv6_mapped().get(),
            src_port: *src_port,
            dscp_and_ecn: *dscp_and_ecn,
        }
    }
}

/// Errors that Bindings may encounter when receiving a UDP datagram.
pub enum ReceiveUdpError {
    /// The socket's receive queue is full and can't hold the datagram.
    QueueFull,
}

/// The bindings context handling received UDP frames.
pub trait UdpReceiveBindingsContext<I: IpExt, D: StrongDeviceIdentifier>: UdpBindingsTypes {
    /// Receives a UDP packet on a socket.
    fn receive_udp(
        &mut self,
        id: &UdpSocketId<I, D::Weak, Self>,
        device_id: &D,
        meta: UdpPacketMeta<I>,
        body: &[u8],
    ) -> Result<(), ReceiveUdpError>;

    /// Notifies Bindings that an error was set on a socket.
    fn on_socket_error(
        &mut self,
        id: &UdpSocketId<I, D::Weak, Self>,
        err: PendingDatagramSocketError,
    );
}

/// The bindings context providing external types to UDP sockets.
///
/// # Discussion
///
/// We'd like this trait to take an `I` type parameter instead of using GAT to
/// get the IP version, however we end up with problems due to the shape of
/// [`DatagramSocketSpec`] and the underlying support for dual stack sockets.
///
/// This is completely fine for all known implementations, except for a rough
/// edge in fake tests bindings contexts that are already parameterized on I
/// themselves. This is still better than relying on `Box<dyn Any>` to keep the
/// external data in our references so we take the rough edge.
pub trait UdpBindingsTypes: DatagramBindingsTypes + MatcherBindingsTypes + Sized + 'static {
    /// Opaque bindings data held by core for a given IP version.
    type ExternalData<I: Ip>: Debug + Send + Sync + 'static;
    /// The listener notified when sockets' writable state changes.
    type SocketWritableListener: SocketWritableListener + Debug + Send + Sync + 'static;
    /// A token representing resources allocated for an in-flight send operation.
    ///
    /// Core holds this token until the packet is either transmitted by the
    /// device or dropped along the egress path. This allows bindings to track
    /// send buffer capacity or other per-packet resources.
    type SendToken: Debug + Send + Sync + 'static;
}

/// The bindings context for UDP.
pub trait UdpBindingsContext<I: IpExt, D: StrongDeviceIdentifier>:
    InstantContext
    + RngContext
    + UdpReceiveBindingsContext<I, D>
    + ReferenceNotifiers
    + UdpBindingsTypes
    + SocketOpsFilterBindingContext<D>
    + SettingsContext<UdpSettings>
    + MatcherBindingsTypes
{
}
impl<
    I: IpExt,
    BC: InstantContext
        + RngContext
        + UdpReceiveBindingsContext<I, D>
        + ReferenceNotifiers
        + UdpBindingsTypes
        + SocketOpsFilterBindingContext<D>
        + SettingsContext<UdpSettings>,
    D: StrongDeviceIdentifier,
> UdpBindingsContext<I, D> for BC
{
}

/// An execution context for the UDP protocol which also provides access to state.
pub trait BoundStateContext<I: IpExt, BC: UdpBindingsContext<I, Self::DeviceId>>:
    DeviceIdContext<AnyDevice> + UdpStateContext
{
    /// The core context passed to the callback provided to methods.
    type IpSocketsCtx<'a>: TransportIpContext<I, BC>
        + MulticastMembershipHandler<I, BC>
        + CoreTxMetadataContext<UdpSocketTxMetadata<I, Self::WeakDeviceId, BC>, BC>
        + DeviceIdContext<AnyDevice, DeviceId = Self::DeviceId, WeakDeviceId = Self::WeakDeviceId>;

    /// The inner dual stack context.
    type DualStackContext: DualStackDatagramBoundStateContext<
            I,
            BC,
            Udp<BC>,
            DeviceId = Self::DeviceId,
            WeakDeviceId = Self::WeakDeviceId,
        >;
    /// The inner non dual stack context.
    type NonDualStackContext: NonDualStackDatagramBoundStateContext<
            I,
            BC,
            Udp<BC>,
            DeviceId = Self::DeviceId,
            WeakDeviceId = Self::WeakDeviceId,
        >;

    /// Calls the function with an immutable reference to UDP sockets.
    fn with_bound_sockets<
        O,
        F: FnOnce(&mut Self::IpSocketsCtx<'_>, &BoundSockets<I, Self::WeakDeviceId, BC>) -> O,
    >(
        &mut self,
        cb: F,
    ) -> O;

    /// Calls the function with a mutable reference to UDP sockets.
    fn with_bound_sockets_mut<
        O,
        F: FnOnce(&mut Self::IpSocketsCtx<'_>, &mut BoundSockets<I, Self::WeakDeviceId, BC>) -> O,
    >(
        &mut self,
        cb: F,
    ) -> O;

    /// Returns a context for dual- or non-dual-stack operation.
    fn dual_stack_context(
        &self,
    ) -> MaybeDualStack<&Self::DualStackContext, &Self::NonDualStackContext>;

    /// Same as [`BoundStateContext::dual_stack_context`], but returns mutable references.
    fn dual_stack_context_mut(
        &mut self,
    ) -> MaybeDualStack<&mut Self::DualStackContext, &mut Self::NonDualStackContext>;

    /// Calls the function without access to the UDP bound socket state.
    fn with_transport_context<O, F: FnOnce(&mut Self::IpSocketsCtx<'_>) -> O>(
        &mut self,
        cb: F,
    ) -> O;
}

/// Core context abstracting state access to UDP state.
pub trait StateContext<I: IpExt, BC: UdpBindingsContext<I, Self::DeviceId>>:
    DeviceIdContext<AnyDevice>
{
    /// The core context passed to the callback.
    type SocketStateCtx<'a>: BoundStateContext<I, BC>
        + DeviceIdContext<AnyDevice, DeviceId = Self::DeviceId, WeakDeviceId = Self::WeakDeviceId>
        + UdpStateContext;

    /// Calls the function with mutable access to the set with all UDP
    /// sockets.
    fn with_all_sockets_mut<O, F: FnOnce(&mut UdpSocketSet<I, Self::WeakDeviceId, BC>) -> O>(
        &mut self,
        cb: F,
    ) -> O;

    /// Calls the function with immutable access to the set with all UDP
    /// sockets.
    fn with_all_sockets<O, F: FnOnce(&UdpSocketSet<I, Self::WeakDeviceId, BC>) -> O>(
        &mut self,
        cb: F,
    ) -> O;

    /// Calls the function without access to UDP socket state.
    fn with_bound_state_context<O, F: FnOnce(&mut Self::SocketStateCtx<'_>) -> O>(
        &mut self,
        cb: F,
    ) -> O;

    /// Calls the function with an immutable reference to the given socket's
    /// state.
    fn with_socket_state<
        O,
        F: FnOnce(&mut Self::SocketStateCtx<'_>, &UdpSocketState<I, Self::WeakDeviceId, BC>) -> O,
    >(
        &mut self,
        id: &UdpSocketId<I, Self::WeakDeviceId, BC>,
        cb: F,
    ) -> O;

    /// Calls the function with a mutable reference to the given socket's state.
    fn with_socket_state_mut<
        O,
        F: FnOnce(&mut Self::SocketStateCtx<'_>, &mut UdpSocketState<I, Self::WeakDeviceId, BC>) -> O,
    >(
        &mut self,
        id: &UdpSocketId<I, Self::WeakDeviceId, BC>,
        cb: F,
    ) -> O;

    /// Call `f` with each socket's state.
    fn for_each_socket<
        F: FnMut(
            &mut Self::SocketStateCtx<'_>,
            &UdpSocketId<I, Self::WeakDeviceId, BC>,
            &UdpSocketState<I, Self::WeakDeviceId, BC>,
        ),
    >(
        &mut self,
        cb: F,
    );
}

/// Empty trait to work around coherence issues.
///
/// This serves only to convince the coherence checker that a particular blanket
/// trait implementation could only possibly conflict with other blanket impls
/// in this crate. It can be safely implemented for any type.
/// TODO(https://github.com/rust-lang/rust/issues/97811): Remove this once the
/// coherence checker doesn't require it.
pub trait UdpStateContext {}

/// An execution context for UDP dual-stack operations.
pub trait DualStackBoundStateContext<I: IpExt, BC: UdpBindingsContext<I, Self::DeviceId>>:
    DeviceIdContext<AnyDevice>
{
    /// The core context passed to the callbacks to methods.
    type IpSocketsCtx<'a>: TransportIpContext<I, BC>
        + DeviceIdContext<AnyDevice, DeviceId = Self::DeviceId, WeakDeviceId = Self::WeakDeviceId>
        + CoreTxMetadataContext<UdpSocketTxMetadata<I, Self::WeakDeviceId, BC>, BC>
        // Allow creating IP sockets for the other IP version.
        + TransportIpContext<I::OtherVersion, BC>
        + CoreTxMetadataContext<UdpSocketTxMetadata<I::OtherVersion, Self::WeakDeviceId, BC>, BC>;

    /// Calls the provided callback with mutable access to both the
    /// demultiplexing maps.
    fn with_both_bound_sockets_mut<
        O,
        F: FnOnce(
            &mut Self::IpSocketsCtx<'_>,
            &mut BoundSockets<I, Self::WeakDeviceId, BC>,
            &mut BoundSockets<I::OtherVersion, Self::WeakDeviceId, BC>,
        ) -> O,
    >(
        &mut self,
        cb: F,
    ) -> O;

    /// Calls the provided callback with mutable access to the demultiplexing
    /// map for the other IP version.
    fn with_other_bound_sockets_mut<
        O,
        F: FnOnce(
            &mut Self::IpSocketsCtx<'_>,
            &mut BoundSockets<I::OtherVersion, Self::WeakDeviceId, BC>,
        ) -> O,
    >(
        &mut self,
        cb: F,
    ) -> O;

    /// Calls the provided callback with access to the `IpSocketsCtx`.
    fn with_transport_context<O, F: FnOnce(&mut Self::IpSocketsCtx<'_>) -> O>(
        &mut self,
        cb: F,
    ) -> O;
}

/// An execution context for UDP non-dual-stack operations.
pub trait NonDualStackBoundStateContext<I: IpExt, BC: UdpBindingsContext<I, Self::DeviceId>>:
    DeviceIdContext<AnyDevice>
{
}

/// An implementation of [`IpTransportContext`] for UDP.
pub enum UdpIpTransportContext {}

fn early_demux_ip_packet<
    I: IpExt,
    B: ParseBuffer,
    BC: UdpBindingsContext<I, CC::DeviceId> + UdpBindingsContext<I::OtherVersion, CC::DeviceId>,
    CC: StateContext<I, BC>
        + StateContext<I::OtherVersion, BC>
        + UdpCounterContext<I, CC::WeakDeviceId, BC>
        + UdpCounterContext<I::OtherVersion, CC::WeakDeviceId, BC>,
>(
    core_ctx: &mut CC,
    device: &CC::DeviceId,
    src_ip: I::Addr,
    dst_ip: I::Addr,
    mut buffer: B,
) -> Option<I::DualStackBoundSocketId<CC::WeakDeviceId, Udp<BC>>> {
    trace_duration!(c"udp::early_demux");

    let Ok(packet) = buffer.parse_with::<_, UdpPacketRaw<_>>(I::VERSION_MARKER) else {
        // If we fail to parse the packet then just return None. Invalid
        // packets are handled later.
        return None;
    };

    let src_ip = SocketIpAddr::new(src_ip)?;
    let dst_ip = SocketIpAddr::new(dst_ip)?;
    let src_port = packet.src_port()?;
    let dst_port = packet.dst_port()?;

    // Find a connected socket matching the packet.
    StateContext::<I, _>::with_bound_state_context(core_ctx, |core_ctx| {
        let device_weak = device.downgrade();
        DatagramBoundStateContext::<_, _, Udp<_>>::with_bound_sockets(
            core_ctx,
            |_core_ctx, bound_sockets| {
                bound_sockets
                    .lookup_connected((src_ip, src_port.into()), (dst_ip, dst_port), device_weak)
                    .map(|entry| entry.first().clone())
            },
        )
    })
}
fn receive_ip_packet<
    I: IpExt,
    B: BufferMut,
    H: IpHeaderInfo<I>,
    BC: UdpBindingsContext<I, CC::DeviceId> + UdpBindingsContext<I::OtherVersion, CC::DeviceId>,
    CC: StateContext<I, BC>
        + StateContext<I::OtherVersion, BC>
        + UdpCounterContext<I, CC::WeakDeviceId, BC>
        + UdpCounterContext<I::OtherVersion, CC::WeakDeviceId, BC>,
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device: &CC::DeviceId,
    src_ip: I::RecvSrcAddr,
    dst_ip: SpecifiedAddr<I::Addr>,
    mut buffer: B,
    info: &mut LocalDeliveryPacketInfo<I, H>,
    early_demux_socket: Option<DualStackUdpSocketId<I, CC::WeakDeviceId, BC>>,
) -> Result<(), (B, I::IcmpError)> {
    let LocalDeliveryPacketInfo { meta, header_info, marks: _ } = info;
    let ReceiveIpPacketMeta { broadcast, transparent_override, parsing_context } = meta;

    trace_duration!("udp::receive_ip_packet");
    CounterContext::<UdpCountersWithoutSocket<I>>::counters(core_ctx).rx.increment();
    trace!("received UDP packet: {:x?}", buffer.as_mut());
    let src_ip: I::Addr = src_ip.into_addr();

    let Ok(packet) = buffer.parse_with::<_, UdpPacket<_>>(UdpParseArgs::with_context(
        src_ip,
        dst_ip.get(),
        parsing_context,
    )) else {
        // There isn't much we can do if the UDP packet is
        // malformed.
        CounterContext::<UdpCountersWithoutSocket<I>>::counters(core_ctx).rx_malformed.increment();
        return Ok(());
    };

    let src_ip = if let Some(src_ip) = SpecifiedAddr::new(src_ip) {
        match src_ip.try_into() {
            Ok(addr) => Some(addr),
            Err(AddrIsMappedError {}) => {
                CounterContext::<UdpCountersWithoutSocket<I>>::counters(core_ctx)
                    .rx_mapped_addr
                    .increment();
                trace!("udp::receive_ip_packet: mapped source address");
                return Ok(());
            }
        }
    } else {
        None
    };

    let dst_port = packet.dst_port();
    let (delivery_ip, delivery_port, require_transparent) = match transparent_override {
        Some(TransparentLocalDelivery { addr, port }) => (*addr, *port, true),
        None => (dst_ip, dst_port, false),
    };

    let delivery_ip = match delivery_ip.try_into() {
        Ok(addr) => addr,
        Err(AddrIsMappedError {}) => {
            CounterContext::<UdpCountersWithoutSocket<I>>::counters(core_ctx)
                .rx_mapped_addr
                .increment();
            trace!("udp::receive_ip_packet: mapped destination address");
            return Ok(());
        }
    };

    let src_port = packet.src_port();
    // Unfortunately, type inference isn't smart enough for us to just do
    // packet.parse_metadata().
    let parse_meta =
        ParsablePacket::<_, UdpParseArgs<I::Addr, &mut NetworkParsingContext>>::parse_metadata(
            &packet,
        );

    /// The maximum number of socket IDs that are expected to receive a given
    /// packet. While it's possible for this number to be exceeded, it's
    /// unlikely.
    const MAX_EXPECTED_IDS: usize = 16;

    /// Collection of sockets that will receive a packet.
    ///
    /// Making this a [`smallvec::SmallVec`] lets us keep all the retrieved ids
    /// on the stack most of the time. If there are more than
    /// [`MAX_EXPECTED_IDS`], this will spill and allocate on the heap.
    type Recipients<Id> = smallvec::SmallVec<[Id; MAX_EXPECTED_IDS]>;

    let recipients = if let Some(socket) = early_demux_socket {
        Recipients::from_iter([socket])
    } else {
        StateContext::<I, _>::with_bound_state_context(core_ctx, |core_ctx| {
            let device_weak = device.downgrade();
            DatagramBoundStateContext::<_, _, Udp<_>>::with_bound_sockets(
                core_ctx,
                |_core_ctx, bound_sockets| {
                    lookup(
                        bound_sockets,
                        (src_ip, src_port),
                        (delivery_ip, delivery_port),
                        device_weak,
                        *broadcast,
                    )
                    .map(|result| match result {
                        LookupResult::Conn(id, _) | LookupResult::Listener(id, _) => id.clone(),
                    })
                    // Collect into an array on the stack.
                    .collect::<Recipients<_>>()
                },
            )
        })
    };

    let meta = UdpPacketMeta {
        src_ip: src_ip.map_or(I::UNSPECIFIED_ADDRESS, SocketIpAddr::addr),
        src_port,
        dst_ip: *dst_ip,
        dst_port,
        dscp_and_ecn: header_info.dscp_and_ecn(),
    };
    let was_delivered = recipients.into_iter().fold(false, |was_delivered, lookup_result| {
        let delivered = try_dual_stack_deliver::<I, BC, CC, H>(
            core_ctx,
            bindings_ctx,
            lookup_result,
            device,
            &meta,
            require_transparent,
            header_info,
            packet.clone(),
        );
        was_delivered | delivered
    });

    if !was_delivered {
        buffer.undo_parse(parse_meta);
        CounterContext::<UdpCountersWithoutSocket<I>>::counters(core_ctx)
            .rx_unknown_dest_port
            .increment();
        Err((buffer, I::IcmpError::port_unreachable()))
    } else {
        Ok(())
    }
}

/// Tries to deliver the given UDP packet to the given UDP socket.
fn try_deliver<
    I: IpExt,
    CC: StateContext<I, BC> + UdpCounterContext<I, CC::WeakDeviceId, BC>,
    BC: UdpBindingsContext<I, CC::DeviceId>,
    WireI: IpExt,
    H: IpHeaderInfo<WireI>,
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    id: &UdpSocketId<I, CC::WeakDeviceId, BC>,
    device_id: &CC::DeviceId,
    meta: UdpPacketMeta<I>,
    require_transparent: bool,
    header_info: &H,
    packet: UdpPacket<&[u8]>,
) -> bool {
    let delivered = core_ctx.with_socket_state(&id, |core_ctx, state| {
        let should_deliver = match &state.inner {
            DatagramSocketStateInner::Bound(DatagramBoundSocketState {
                socket_type,
                original_bound_addr: _,
            }) => match socket_type {
                DatagramBoundSocketStateType::Connected(state) => {
                    match BoundStateContext::dual_stack_context_mut(core_ctx) {
                        MaybeDualStack::DualStack(dual_stack) => {
                            match dual_stack.ds_converter().convert(state) {
                                DualStackConnState::ThisStack(state) => state.should_receive(),
                                DualStackConnState::OtherStack(state) => state.should_receive(),
                            }
                        }
                        MaybeDualStack::NotDualStack(not_dual_stack) => {
                            not_dual_stack.nds_converter().convert(state).should_receive()
                        }
                    }
                }
                DatagramBoundSocketStateType::Listener(_) => true,
            },
            DatagramSocketStateInner::Unbound(_) => true,
        };

        if !should_deliver {
            return None;
        }

        // Transparently proxied packets are only delivered to transparent
        // sockets.
        if require_transparent && !state.options().transparent() {
            return None;
        }

        let [ip_prefix, ip_options] = header_info.as_bytes();
        let [udp_header, data] = packet.as_bytes();
        let mut slices = [ip_prefix, ip_options, udp_header, data];
        let packet_buf = FragmentedByteSlice::new(&mut slices);
        let header_len = ip_prefix.len() + ip_options.len() + udp_header.len();
        let filter_result = bindings_ctx.socket_ops_filter().on_ingress(
            WireI::VERSION,
            packet_buf,
            header_len,
            device_id,
            id.socket_info(),
            state.options().marks(),
        );

        match filter_result {
            SocketIngressFilterResult::Accept => {
                Some(bindings_ctx.receive_udp(id, device_id, meta, packet.body()))
            }
            SocketIngressFilterResult::Drop => None,
        }
    });

    match delivered {
        None => false,
        Some(result) => {
            core_ctx.increment_both(id, |c| &c.rx_delivered);
            match result {
                Ok(()) => {}
                Err(ReceiveUdpError::QueueFull) => {
                    core_ctx.increment_both(id, |c| &c.rx_queue_full);
                }
            }
            true
        }
    }
}

/// A wrapper for [`try_deliver`] that supports dual stack delivery.
fn try_dual_stack_deliver<
    I: IpExt,
    BC: UdpBindingsContext<I, CC::DeviceId> + UdpBindingsContext<I::OtherVersion, CC::DeviceId>,
    CC: StateContext<I, BC>
        + StateContext<I::OtherVersion, BC>
        + UdpCounterContext<I, CC::WeakDeviceId, BC>
        + UdpCounterContext<I::OtherVersion, CC::WeakDeviceId, BC>,
    H: IpHeaderInfo<I>,
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    socket: I::DualStackBoundSocketId<CC::WeakDeviceId, Udp<BC>>,
    device_id: &CC::DeviceId,
    meta: &UdpPacketMeta<I>,
    require_transparent: bool,
    header_info: &H,
    packet: UdpPacket<&[u8]>,
) -> bool {
    #[derive(GenericOverIp)]
    #[generic_over_ip(I, Ip)]
    struct Inputs<'a, I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes> {
        meta: &'a UdpPacketMeta<I>,
        socket: I::DualStackBoundSocketId<D, Udp<BT>>,
    }

    struct Outputs<I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes> {
        meta: UdpPacketMeta<I>,
        socket: UdpSocketId<I, D, BT>,
    }

    #[derive(GenericOverIp)]
    #[generic_over_ip(I, Ip)]
    enum DualStackOutputs<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes> {
        CurrentStack(Outputs<I, D, BT>),
        OtherStack(Outputs<I::OtherVersion, D, BT>),
    }

    let dual_stack_outputs = I::map_ip(
        Inputs { meta, socket },
        |Inputs { meta, socket }| match socket {
            EitherIpSocket::V4(socket) => {
                DualStackOutputs::CurrentStack(Outputs { meta: meta.clone(), socket })
            }
            EitherIpSocket::V6(socket) => {
                DualStackOutputs::OtherStack(Outputs { meta: meta.to_ipv6_mapped(), socket })
            }
        },
        |Inputs { meta, socket }| {
            DualStackOutputs::CurrentStack(Outputs { meta: meta.clone(), socket })
        },
    );

    match dual_stack_outputs {
        DualStackOutputs::CurrentStack(Outputs { meta, socket }) => try_deliver(
            core_ctx,
            bindings_ctx,
            &socket,
            device_id,
            meta,
            require_transparent,
            header_info,
            packet,
        ),
        DualStackOutputs::OtherStack(Outputs { meta, socket }) => try_deliver(
            core_ctx,
            bindings_ctx,
            &socket,
            device_id,
            meta,
            require_transparent,
            header_info,
            packet,
        ),
    }
}

fn receive_icmp_error<I, BC, CC>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device: &CC::DeviceId,
    original_src_ip: Option<SpecifiedAddr<I::Addr>>,
    original_dst_ip: SpecifiedAddr<I::Addr>,
    original_udp_packet: &[u8],
    err: I::ErrorCode,
) where
    I: IpExt,
    BC: UdpBindingsContext<I, CC::DeviceId> + UdpBindingsContext<I::OtherVersion, CC::DeviceId>,
    CC: StateContext<I, BC>
        + StateContext<I::OtherVersion, BC>
        + UdpCounterContext<I, CC::WeakDeviceId, BC>
        + UdpCounterContext<I::OtherVersion, CC::WeakDeviceId, BC>,
{
    let icmp_err = err.into();
    let Some(pending_err) = PendingDatagramSocketError::from_hard_icmp(icmp_err) else {
        CounterContext::<UdpCountersWithoutSocket<I>>::counters(core_ctx)
            .rx_icmp_error_soft
            .increment();
        return;
    };

    CounterContext::<UdpCountersWithoutSocket<I>>::counters(core_ctx)
        .rx_icmp_error_hard
        .increment();

    let mut buffer = original_udp_packet;
    let packet = match buffer.parse_with::<_, UdpPacketRaw<_>>(I::VERSION_MARKER) {
        Ok(p) => p,
        Err(_) => {
            CounterContext::<UdpCountersWithoutSocket<I>>::counters(core_ctx)
                .rx_icmp_error_hard_malformed
                .increment();
            return;
        }
    };

    let Some(orig_src_port) = packet.src_port() else {
        CounterContext::<UdpCountersWithoutSocket<I>>::counters(core_ctx)
            .rx_icmp_error_hard_malformed
            .increment();
        return;
    };
    let Some(orig_dst_port) = packet.dst_port() else {
        CounterContext::<UdpCountersWithoutSocket<I>>::counters(core_ctx)
            .rx_icmp_error_hard_malformed
            .increment();
        return;
    };
    let Some(orig_src_ip) = original_src_ip else {
        CounterContext::<UdpCountersWithoutSocket<I>>::counters(core_ctx)
            .rx_icmp_error_hard_malformed
            .increment();
        return;
    };
    let orig_src_ip = match SocketIpAddr::try_from(orig_src_ip) {
        Ok(ip) => ip,
        Err(AddrIsMappedError {}) => {
            debug!("ignoring ICMP error from IPv4-mapped-IPv6 source: {}", orig_src_ip);
            return;
        }
    };
    let orig_dst_ip = match SocketIpAddr::try_from(original_dst_ip) {
        Ok(ip) => ip,
        Err(AddrIsMappedError {}) => {
            debug!("ignoring ICMP error to IPv4-mapped-IPv6 destination: {}", original_dst_ip);
            return;
        }
    };

    let socket_id = StateContext::<I, _>::with_bound_state_context(core_ctx, |core_ctx| {
        let device_weak = device.downgrade();
        DatagramBoundStateContext::<_, _, Udp<_>>::with_bound_sockets(
            core_ctx,
            |_core_ctx, bound_sockets| {
                bound_sockets
                    .lookup_connected(
                        (orig_dst_ip, UdpRemotePort::from(orig_dst_port)),
                        (orig_src_ip, orig_src_port),
                        device_weak,
                    )
                    .map(|entry| entry.first().clone())
            },
        )
    });

    let Some(socket_id) = socket_id else {
        CounterContext::<UdpCountersWithoutSocket<I>>::counters(core_ctx)
            .rx_icmp_error_hard_no_socket
            .increment();
        return;
    };

    #[derive(GenericOverIp)]
    #[generic_over_ip(I, Ip)]
    struct Inputs<I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes> {
        socket_id: I::DualStackBoundSocketId<D, Udp<BT>>,
    }

    #[derive(GenericOverIp)]
    #[generic_over_ip(I, Ip)]
    enum DualStackOutputs<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes> {
        CurrentStack(UdpSocketId<I, D, BT>),
        OtherStack(UdpSocketId<I::OtherVersion, D, BT>),
    }

    let dual_stack_outputs = I::map_ip(
        Inputs { socket_id },
        |Inputs { socket_id }| match socket_id {
            EitherIpSocket::V4(socket_id) => DualStackOutputs::CurrentStack(socket_id),
            EitherIpSocket::V6(socket_id) => DualStackOutputs::OtherStack(socket_id),
        },
        |Inputs { socket_id }| DualStackOutputs::CurrentStack(socket_id),
    );

    match dual_stack_outputs {
        DualStackOutputs::CurrentStack(socket_id) => {
            core_ctx.increment_both(&socket_id, |c| &c.rx_icmp_error_hard_delivered);
            bindings_ctx.on_socket_error(&socket_id, pending_err);
        }
        DualStackOutputs::OtherStack(socket_id) => {
            ResourceCounterContext::<
                UdpSocketId<I::OtherVersion, CC::WeakDeviceId, BC>,
                UdpCountersWithSocket<I::OtherVersion>,
            >::increment_both(core_ctx, &socket_id, |c| {
                &c.rx_icmp_error_hard_delivered
            });
            bindings_ctx.on_socket_error(&socket_id, pending_err);
        }
    }
}

/// Enables a blanket implementation of [`IpTransportContext`] for
/// [`UdpIpTransportContext`].
///
/// Implementing this marker trait for a type enables a blanket implementation
/// of `IpTransportContext` given the other requirements are met.
// For some reason rustc insists that this trait is not used, but it's required
// to mark types that want the blanket impl. This should be lifted when this
// type is pulled into the UDP crate and the trait is exported.
pub trait UseUdpIpTransportContextBlanket {}

/// Alias for a SocketId that can reference either V4 or V6 socket.
pub type DualStackUdpSocketId<I, D, BT> =
    <I as DualStackBaseIpExt>::DualStackBoundSocketId<D, Udp<BT>>;

impl<
    I: IpExt,
    BC: UdpBindingsContext<I, CC::DeviceId> + UdpBindingsContext<I::OtherVersion, CC::DeviceId>,
    CC: StateContext<I, BC>
        + StateContext<I::OtherVersion, BC>
        + UseUdpIpTransportContextBlanket
        + UdpCounterContext<I, CC::WeakDeviceId, BC>
        + UdpCounterContext<I::OtherVersion, CC::WeakDeviceId, BC>,
> IpTransportContext<I, BC, CC> for UdpIpTransportContext
{
    type EarlyDemuxSocket = DualStackUdpSocketId<I, CC::WeakDeviceId, BC>;

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
        device: &CC::DeviceId,
        original_src_ip: Option<SpecifiedAddr<I::Addr>>,
        original_dst_ip: SpecifiedAddr<I::Addr>,
        original_udp_packet: &[u8],
        err: I::ErrorCode,
    ) {
        CounterContext::<UdpCountersWithoutSocket<I>>::counters(core_ctx).rx_icmp_error.increment();
        debug!(
            "UDP received ICMP error {:?} from {:?} to {:?}",
            err, original_dst_ip, original_src_ip
        );

        receive_icmp_error::<I, _, _>(
            core_ctx,
            bindings_ctx,
            device,
            original_src_ip,
            original_dst_ip,
            original_udp_packet,
            err,
        )
    }

    fn receive_ip_packet<B: BufferMut, H: IpHeaderInfo<I>>(
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        device: &CC::DeviceId,
        src_ip: I::RecvSrcAddr,
        dst_ip: SpecifiedAddr<I::Addr>,
        buffer: B,
        info: &mut LocalDeliveryPacketInfo<I, H>,
        early_demux_socket: Option<Self::EarlyDemuxSocket>,
    ) -> Result<(), (B, I::IcmpError)> {
        receive_ip_packet::<I, _, _, _, _>(
            core_ctx,
            bindings_ctx,
            device,
            src_ip,
            dst_ip,
            buffer,
            info,
            early_demux_socket,
        )
    }
}

/// An error encountered while sending a UDP packet to an alternate address.
#[derive(Error, Debug, PartialEq)]
pub enum SendToError {
    /// The socket is not writeable.
    #[error("not writeable")]
    NotWriteable,
    /// An error was encountered while trying to bind a local address for an
    /// unbound socket.
    #[error("local address error: {0}")]
    LocalAddress(#[from] LocalAddressError),
    /// An error was encountered while trying to create a temporary IP socket
    /// to use for the send operation.
    #[error("could not create a temporary connection socket: {0}")]
    CreateSock(#[from] IpSockCreationError),
    /// An error was encountered while trying to send via the temporary IP
    /// socket.
    #[error("could not send via temporary socket: {0}")]
    Send(#[from] IpSockSendError),
    /// There was a problem with the remote address relating to its zone.
    #[error("zone error: {0}")]
    Zone(#[from] ZonedAddressError),
    /// Disallow sending packets with a remote port of 0. See
    /// [`UdpRemotePort::Unset`] for the rationale.
    #[error("the remote port was unset")]
    RemotePortUnset,
    /// The remote address is mapped (i.e. an ipv4-mapped-ipv6 address), but the
    /// socket is not dual-stack enabled.
    #[error("the remote ip was unexpectedly an ipv4-mapped-ipv6 address")]
    RemoteUnexpectedlyMapped,
    /// The remote address is non-mapped (i.e not an ipv4-mapped-ipv6 address),
    /// but the socket is dual stack enabled and bound to a mapped address.
    #[error("the remote ip was unexpectedly not an ipv4-mapped-ipv6 address")]
    RemoteUnexpectedlyNonMapped,
    /// The socket's send buffer is full.
    #[error("send buffer full")]
    SendBufferFull,
    /// Invalid message length.
    #[error("invalid message length")]
    InvalidLength,
}

/// The UDP socket API.
pub struct UdpApi<I: Ip, C>(C, IpVersionMarker<I>);

impl<I: Ip, C> UdpApi<I, C> {
    /// Creates a new `UdpApi` from `ctx`.
    pub fn new(ctx: C) -> Self {
        Self(ctx, IpVersionMarker::new())
    }
}

/// A local alias for [`UdpSocketId`] for use in [`UdpApi`].
///
/// TODO(https://github.com/rust-lang/rust/issues/8995): Make this an inherent
/// associated type.
type UdpApiSocketId<I, C> = UdpSocketId<
    I,
    <<C as ContextPair>::CoreContext as DeviceIdContext<AnyDevice>>::WeakDeviceId,
    <C as ContextPair>::BindingsContext,
>;

impl<I, C> UdpApi<I, C>
where
    I: IpExt,
    C: ContextPair,
    C::CoreContext: StateContext<I, C::BindingsContext>
        + UdpCounterContext<
            I,
            <C::CoreContext as DeviceIdContext<AnyDevice>>::WeakDeviceId,
            C::BindingsContext,
        >
        // NB: This bound is somewhat redundant to StateContext but it helps the
        // compiler know we're using UDP datagram sockets.
        + DatagramStateContext<I, C::BindingsContext, Udp<C::BindingsContext>>,
    C::BindingsContext:
        UdpBindingsContext<I, <C::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId>,
    <C::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId: netstack3_base::InterfaceProperties<
            <C::BindingsContext as MatcherBindingsTypes>::DeviceClass,
        >,
{
    fn core_ctx(&mut self) -> &mut C::CoreContext {
        let Self(pair, IpVersionMarker { .. }) = self;
        pair.core_ctx()
    }

    fn contexts(&mut self) -> (&mut C::CoreContext, &mut C::BindingsContext) {
        let Self(pair, IpVersionMarker { .. }) = self;
        pair.contexts()
    }

    /// Get diagnostic information for sockets matching the provided matcher.
    pub fn bound_sockets_diagnostics<M, E>(&mut self, matcher: &M, results: &mut E)
    where
        M: IpSocketPropertiesMatcher<<C::BindingsContext as MatcherBindingsTypes>::DeviceClass>
            + ?Sized,
        E: Extend<UdpSocketDiagnostics<I>>,
    {
        DatagramStateContext::for_each_socket(self.core_ctx(), |ctx, id, state| {
            if !matcher
                .matches_ip_socket(&netstack3_datagram::SocketStateForMatching::new(state, id, ctx))
            {
                return;
            }

            results.extend(UdpSocketDiagnostics::from_parts(state, id.socket_cookie()));
        });
    }

    /// Disconnects all bound sockets matching the provided matcher.
    ///
    /// Returns the number of sockets that were disconnected.
    pub fn disconnect_bound<M>(&mut self, matcher: &M) -> usize
    where
        M: IpSocketPropertiesMatcher<<C::BindingsContext as MatcherBindingsTypes>::DeviceClass>
            + ?Sized,
    {
        // It's technically possible to avoid the Vec here by putting the
        // disconnection logic into the datagram crate and providing trait hooks
        // for specific functionality. This is how bound_socket_diagnostics does
        // things. However, this is not a performance-sensitive operation, and
        // doing so would add a lot of complexity to the code.
        let mut ids = Vec::new();
        DatagramStateContext::for_each_socket(self.core_ctx(), |ctx, id, state| {
            if matcher
                .matches_ip_socket(&netstack3_datagram::SocketStateForMatching::new(state, id, ctx))
            {
                ids.push(id.clone());
            }
        });

        for id in &ids {
            self.datagram().disconnect_any_to_unbound(id);

            let (_, bindings_ctx) = self.contexts();
            bindings_ctx.on_socket_error(id, PendingDatagramSocketError::Aborted);
        }

        ids.len()
    }

    fn datagram(&mut self) -> &mut DatagramApi<I, C, Udp<C::BindingsContext>> {
        let Self(pair, IpVersionMarker { .. }) = self;
        DatagramApi::wrap(pair)
    }

    /// Creates a new unbound UDP socket with default external data.
    pub fn create(&mut self) -> UdpApiSocketId<I, C>
    where
        <C::BindingsContext as UdpBindingsTypes>::ExternalData<I>: Default,
        <C::BindingsContext as UdpBindingsTypes>::SocketWritableListener: Default,
    {
        self.create_with(Default::default(), Default::default())
    }

    /// Creates a new unbound UDP socket with provided external data.
    pub fn create_with(
        &mut self,
        external_data: <C::BindingsContext as UdpBindingsTypes>::ExternalData<I>,
        writable_listener: <C::BindingsContext as UdpBindingsTypes>::SocketWritableListener,
    ) -> UdpApiSocketId<I, C> {
        self.datagram().create(external_data, writable_listener)
    }

    /// Connect a UDP socket
    ///
    /// `connect` binds `id` as a connection to the remote address and port. It
    /// is also bound to a local address and port, meaning that packets sent on
    /// this connection will always come from that address and port. The local
    /// address will be chosen based on the route to the remote address, and the
    /// local port will be chosen from the available ones.
    ///
    /// # Errors
    ///
    /// `connect` will fail in the following cases:
    /// - If both `local_ip` and `local_port` are specified but conflict with an
    ///   existing connection or listener
    /// - If one or both are left unspecified but there is still no way to
    ///   satisfy the request (e.g., `local_ip` is specified but there are no
    ///   available local ports for that address)
    /// - If there is no route to `remote_ip`
    /// - If `id` belongs to an already-connected socket
    pub fn connect(
        &mut self,
        id: &UdpApiSocketId<I, C>,
        remote_ip: Option<
            ZonedAddr<
                SpecifiedAddr<I::Addr>,
                <C::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId,
            >,
        >,
        remote_port: UdpRemotePort,
    ) -> Result<(), ConnectError> {
        debug!("connect on {id:?} to {remote_ip:?}:{remote_port:?}");
        self.datagram().connect(id, remote_ip, remote_port, ())
    }

    /// Sets the bound device for a socket.
    ///
    /// Sets the device to be used for sending and receiving packets for a socket.
    /// If the socket is not currently bound to a local address and port, the device
    /// will be used when binding.
    pub fn set_device(
        &mut self,
        id: &UdpApiSocketId<I, C>,
        device_id: Option<&<C::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId>,
    ) -> Result<(), SocketError> {
        debug!("set device on {id:?} to {device_id:?}");
        self.datagram().set_device(id, device_id)
    }

    /// Gets the device the specified socket is bound to.
    pub fn get_bound_device(
        &mut self,
        id: &UdpApiSocketId<I, C>,
    ) -> Option<<C::CoreContext as DeviceIdContext<AnyDevice>>::WeakDeviceId> {
        self.datagram().get_bound_device(id)
    }

    /// Enable or disable dual stack operations on the given socket.
    ///
    /// This is notionally the inverse of the `IPV6_V6ONLY` socket option.
    ///
    /// # Errors
    ///
    /// Returns an error if the socket does not support the `IPV6_V6ONLY` socket
    /// option (e.g. an IPv4 socket).
    pub fn set_dual_stack_enabled(
        &mut self,
        id: &UdpApiSocketId<I, C>,
        enabled: bool,
    ) -> Result<(), SetDualStackEnabledError> {
        self.datagram()
            .with_other_stack_ip_options_mut_if_unbound(id, |other_stack| {
                I::map_ip(
                    (enabled, WrapOtherStackIpOptionsMut(other_stack)),
                    |(_enabled, _v4)| Err(NotDualStackCapableError.into()),
                    |(enabled, WrapOtherStackIpOptionsMut(other_stack))| {
                        let DualStackSocketState { dual_stack_enabled, .. } = other_stack;
                        *dual_stack_enabled = enabled;
                        Ok(())
                    },
                )
            })
            .map_err(|ExpectedUnboundError| {
                // NB: Match Linux and prefer to return `NotCapable` errors over
                // `SocketIsBound` errors, for IPv4 sockets.
                match I::VERSION {
                    IpVersion::V4 => NotDualStackCapableError.into(),
                    IpVersion::V6 => SetDualStackEnabledError::SocketIsBound,
                }
            })?
    }

    /// Get the enabled state of dual stack operations on the given socket.
    ///
    /// This is notionally the inverse of the `IPV6_V6ONLY` socket option.
    ///
    /// # Errors
    ///
    /// Returns an error if the socket does not support the `IPV6_V6ONLY` socket
    /// option (e.g. an IPv4 socket).
    pub fn get_dual_stack_enabled(
        &mut self,
        id: &UdpApiSocketId<I, C>,
    ) -> Result<bool, NotDualStackCapableError> {
        self.datagram().with_other_stack_ip_options(id, |other_stack| {
            I::map_ip(
                WrapOtherStackIpOptions(other_stack),
                |_v4| Err(NotDualStackCapableError),
                |WrapOtherStackIpOptions(other_stack)| {
                    let DualStackSocketState { dual_stack_enabled, .. } = other_stack;
                    Ok(*dual_stack_enabled)
                },
            )
        })
    }

    /// Sets the POSIX `SO_REUSEADDR` option for the specified socket.
    ///
    /// # Errors
    ///
    /// Returns an error if the socket is already bound.
    pub fn set_posix_reuse_addr(
        &mut self,
        id: &UdpApiSocketId<I, C>,
        reuse_addr: bool,
    ) -> Result<(), ExpectedUnboundError> {
        self.datagram().update_sharing(id, |sharing| {
            sharing.reuse_addr = reuse_addr;
        })
    }

    /// Gets the POSIX `SO_REUSEADDR` option for the specified socket.
    pub fn get_posix_reuse_addr(&mut self, id: &UdpApiSocketId<I, C>) -> bool {
        self.datagram().get_sharing(id).reuse_addr
    }

    /// Sets the POSIX `SO_REUSEPORT` option for the specified socket.
    ///
    /// # Errors
    ///
    /// Returns an error if the socket is already bound.
    pub fn set_posix_reuse_port(
        &mut self,
        id: &UdpApiSocketId<I, C>,
        reuse_port: ReusePortOption,
    ) -> Result<(), ExpectedUnboundError> {
        self.datagram().update_sharing(id, |sharing| {
            sharing.reuse_port = reuse_port;
        })
    }

    /// Gets the POSIX `SO_REUSEPORT` option for the specified socket.
    pub fn get_posix_reuse_port(&mut self, id: &UdpApiSocketId<I, C>) -> bool {
        self.datagram().get_sharing(id).reuse_port.is_enabled()
    }

    /// Sets the specified socket's membership status for the given group.
    ///
    /// An error is returned if the membership change request is invalid
    /// (e.g. leaving a group that was not joined, or joining a group multiple
    /// times) or if the device to use to join is unspecified or conflicts with
    /// the existing socket state.
    pub fn set_multicast_membership(
        &mut self,
        id: &UdpApiSocketId<I, C>,
        multicast_group: MulticastAddr<I::Addr>,
        interface: MulticastMembershipInterfaceSelector<
            I::Addr,
            <C::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId,
        >,
        want_membership: bool,
    ) -> Result<(), SetMulticastMembershipError> {
        debug!(
            "set multicast membership on {id:?} for group {multicast_group:?} with interface \
            selector: {interface:?}: want_membership={want_membership}"
        );
        self.datagram().set_multicast_membership(id, multicast_group, interface, want_membership)
    }

    /// Sets the hop limit for packets sent by the socket to a unicast
    /// destination.
    ///
    /// Sets the IPv4 TTL when `ip_version` is [`IpVersion::V4`], and the IPv6
    /// hop limits when `ip_version` is [`IpVersion::V6`].
    ///
    /// Returns [`NotDualStackCapableError`] if called on an IPv4 Socket with an
    /// `ip_version` of [`IpVersion::V6`].
    pub fn set_unicast_hop_limit(
        &mut self,
        id: &UdpApiSocketId<I, C>,
        unicast_hop_limit: Option<NonZeroU8>,
        ip_version: IpVersion,
    ) -> Result<(), NotDualStackCapableError> {
        if ip_version == I::VERSION {
            return Ok(self
                .datagram()
                .update_ip_hop_limit(id, SocketHopLimits::set_unicast(unicast_hop_limit)));
        }
        self.datagram().with_other_stack_ip_options_mut(id, |other_stack| {
            I::map_ip(
                (IpInvariant(unicast_hop_limit), WrapOtherStackIpOptionsMut(other_stack)),
                |(IpInvariant(_unicast_hop_limit), _v4)| Err(NotDualStackCapableError),
                |(IpInvariant(unicast_hop_limit), WrapOtherStackIpOptionsMut(other_stack))| {
                    let DualStackSocketState {
                        socket_options:
                            DatagramIpSpecificSocketOptions {
                                hop_limits: SocketHopLimits { unicast, multicast: _, version: _ },
                                ..
                            },
                        ..
                    } = other_stack;
                    *unicast = unicast_hop_limit;
                    Ok(())
                },
            )
        })
    }

    /// Sets the hop limit for packets sent by the socket to a multicast
    /// destination.
    ///
    /// Sets the IPv4 TTL when `ip_version` is [`IpVersion::V4`], and the IPv6
    /// hop limits when `ip_version` is [`IpVersion::V6`].
    ///
    /// Returns [`NotDualStackCapableError`] if called on an IPv4 Socket with an
    /// `ip_version` of [`IpVersion::V6`].
    pub fn set_multicast_hop_limit(
        &mut self,
        id: &UdpApiSocketId<I, C>,
        multicast_hop_limit: Option<NonZeroU8>,
        ip_version: IpVersion,
    ) -> Result<(), NotDualStackCapableError> {
        if ip_version == I::VERSION {
            return Ok(self
                .datagram()
                .update_ip_hop_limit(id, SocketHopLimits::set_multicast(multicast_hop_limit)));
        }
        self.datagram().with_other_stack_ip_options_mut(id, |other_stack| {
            I::map_ip(
                (IpInvariant(multicast_hop_limit), WrapOtherStackIpOptionsMut(other_stack)),
                |(IpInvariant(_multicast_hop_limit), _v4)| Err(NotDualStackCapableError),
                |(IpInvariant(multicast_hop_limit), WrapOtherStackIpOptionsMut(other_stack))| {
                    let DualStackSocketState {
                        socket_options:
                            DatagramIpSpecificSocketOptions {
                                hop_limits: SocketHopLimits { unicast: _, multicast, version: _ },
                                ..
                            },
                        ..
                    } = other_stack;
                    *multicast = multicast_hop_limit;
                    Ok(())
                },
            )
        })
    }

    /// Gets the hop limit for packets sent by the socket to a unicast
    /// destination.
    ///
    /// Gets the IPv4 TTL when `ip_version` is [`IpVersion::V4`], and the IPv6
    /// hop limits when `ip_version` is [`IpVersion::V6`].
    ///
    /// Returns [`NotDualStackCapableError`] if called on an IPv4 Socket with an
    /// `ip_version` of [`IpVersion::V6`].
    pub fn get_unicast_hop_limit(
        &mut self,
        id: &UdpApiSocketId<I, C>,
        ip_version: IpVersion,
    ) -> Result<NonZeroU8, NotDualStackCapableError> {
        if ip_version == I::VERSION {
            return Ok(self.datagram().get_ip_hop_limits(id).unicast);
        }
        self.datagram().with_other_stack_ip_options_and_default_hop_limits(
            id,
            |other_stack, default_hop_limits| {
                I::map_ip_in(
                    (WrapOtherStackIpOptions(other_stack), IpInvariant(default_hop_limits)),
                    |_v4| Err(NotDualStackCapableError),
                    |(
                        WrapOtherStackIpOptions(other_stack),
                        IpInvariant(HopLimits { unicast: default_unicast, multicast: _ }),
                    )| {
                        let DualStackSocketState {
                            socket_options:
                                DatagramIpSpecificSocketOptions {
                                    hop_limits:
                                        SocketHopLimits { unicast, multicast: _, version: _ },
                                    ..
                                },
                            ..
                        } = other_stack;
                        Ok(unicast.unwrap_or(default_unicast))
                    },
                )
            },
        )?
    }

    /// Gets the hop limit for packets sent by the socket to a multicast
    /// destination.
    ///
    /// Gets the IPv4 TTL when `ip_version` is [`IpVersion::V4`], and the IPv6
    /// hop limits when `ip_version` is [`IpVersion::V6`].
    ///
    /// Returns [`NotDualStackCapableError`] if called on an IPv4 Socket with an
    /// `ip_version` of [`IpVersion::V6`].
    pub fn get_multicast_hop_limit(
        &mut self,
        id: &UdpApiSocketId<I, C>,
        ip_version: IpVersion,
    ) -> Result<NonZeroU8, NotDualStackCapableError> {
        if ip_version == I::VERSION {
            return Ok(self.datagram().get_ip_hop_limits(id).multicast);
        }
        self.datagram().with_other_stack_ip_options_and_default_hop_limits(
            id,
            |other_stack, default_hop_limits| {
                I::map_ip_in(
                    (WrapOtherStackIpOptions(other_stack), IpInvariant(default_hop_limits)),
                    |_v4| Err(NotDualStackCapableError),
                    |(
                        WrapOtherStackIpOptions(other_stack),
                        IpInvariant(HopLimits { unicast: _, multicast: default_multicast }),
                    )| {
                        let DualStackSocketState {
                            socket_options:
                                DatagramIpSpecificSocketOptions {
                                    hop_limits:
                                        SocketHopLimits { unicast: _, multicast, version: _ },
                                    ..
                                },
                            ..
                        } = other_stack;
                        Ok(multicast.unwrap_or(default_multicast))
                    },
                )
            },
        )?
    }

    /// Returns the configured multicast interface for the socket.
    pub fn get_multicast_interface(
        &mut self,
        id: &UdpApiSocketId<I, C>,
        ip_version: IpVersion,
    ) -> Result<
        Option<<C::CoreContext as DeviceIdContext<AnyDevice>>::WeakDeviceId>,
        NotDualStackCapableError,
    > {
        if ip_version == I::VERSION {
            return Ok(self.datagram().get_multicast_interface(id));
        };

        self.datagram().with_other_stack_ip_options(id, |other_stack| {
            I::map_ip_in(
                WrapOtherStackIpOptions(other_stack),
                |_v4| Err(NotDualStackCapableError),
                |WrapOtherStackIpOptions(other_stack)| {
                    Ok(other_stack.socket_options.multicast_interface.clone())
                },
            )
        })
    }

    /// Sets the multicast interface to `interface` for a socket.
    pub fn set_multicast_interface(
        &mut self,
        id: &UdpApiSocketId<I, C>,
        interface: Option<&<C::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId>,
        ip_version: IpVersion,
    ) -> Result<(), NotDualStackCapableError> {
        if ip_version == I::VERSION {
            self.datagram().set_multicast_interface(id, interface);
            return Ok(());
        };

        self.datagram().with_other_stack_ip_options_mut(id, |other_stack| {
            I::map_ip(
                (IpInvariant(interface), WrapOtherStackIpOptionsMut(other_stack)),
                |(IpInvariant(_interface), _v4)| Err(NotDualStackCapableError),
                |(IpInvariant(interface), WrapOtherStackIpOptionsMut(other_stack))| {
                    other_stack.socket_options.multicast_interface =
                        interface.map(|device| device.downgrade());
                    Ok(())
                },
            )
        })
    }

    /// Gets the transparent option.
    pub fn get_transparent(&mut self, id: &UdpApiSocketId<I, C>) -> bool {
        self.datagram().get_ip_transparent(id)
    }

    /// Sets the transparent option.
    pub fn set_transparent(&mut self, id: &UdpApiSocketId<I, C>, value: bool) {
        self.datagram().set_ip_transparent(id, value)
    }

    /// Gets the socket mark at the mark domain.
    pub fn get_mark(&mut self, id: &UdpApiSocketId<I, C>, domain: MarkDomain) -> Mark {
        self.datagram().get_mark(id, domain)
    }

    /// Sets the socket mark at the mark domain.
    pub fn set_mark(&mut self, id: &UdpApiSocketId<I, C>, domain: MarkDomain, mark: Mark) {
        self.datagram().set_mark(id, domain, mark)
    }

    /// Gets the broadcast option.
    pub fn get_broadcast(&mut self, id: &UdpApiSocketId<I, C>) -> bool {
        self.datagram().with_both_stacks_ip_options(id, |this_stack, other_stack| {
            I::map_ip_in(
                (this_stack, WrapOtherStackIpOptions(other_stack)),
                |(this_stack, _)| this_stack.allow_broadcast.is_some(),
                |(_, WrapOtherStackIpOptions(other_stack))| {
                    other_stack.socket_options.allow_broadcast.is_some()
                },
            )
        })
    }

    /// Sets the broadcast option.
    pub fn set_broadcast(&mut self, id: &UdpApiSocketId<I, C>, value: bool) {
        self.datagram().with_both_stacks_ip_options_mut(id, |this_stack, other_stack| {
            let value = value.then_some(());
            I::map_ip_in(
                (this_stack, WrapOtherStackIpOptionsMut(other_stack)),
                |(this_stack, _)| this_stack.allow_broadcast = value,
                |(_, WrapOtherStackIpOptionsMut(other_stack))| {
                    other_stack.socket_options.allow_broadcast = value;
                },
            )
        })
    }

    /// Gets the loopback multicast option.
    pub fn get_multicast_loop(
        &mut self,
        id: &UdpApiSocketId<I, C>,
        ip_version: IpVersion,
    ) -> Result<bool, NotDualStackCapableError> {
        if ip_version == I::VERSION {
            return Ok(self.datagram().get_multicast_loop(id));
        };

        self.datagram().with_other_stack_ip_options(id, |other_stack| {
            I::map_ip_in(
                WrapOtherStackIpOptions(other_stack),
                |_v4| Err(NotDualStackCapableError),
                |WrapOtherStackIpOptions(other_stack)| {
                    Ok(other_stack.socket_options.multicast_loop)
                },
            )
        })
    }

    /// Sets the loopback multicast option.
    pub fn set_multicast_loop(
        &mut self,
        id: &UdpApiSocketId<I, C>,
        value: bool,
        ip_version: IpVersion,
    ) -> Result<(), NotDualStackCapableError> {
        if ip_version == I::VERSION {
            self.datagram().set_multicast_loop(id, value);
            return Ok(());
        };

        self.datagram().with_other_stack_ip_options_mut(id, |other_stack| {
            I::map_ip(
                (IpInvariant(value), WrapOtherStackIpOptionsMut(other_stack)),
                |(IpInvariant(_interface), _v4)| Err(NotDualStackCapableError),
                |(IpInvariant(value), WrapOtherStackIpOptionsMut(other_stack))| {
                    other_stack.socket_options.multicast_loop = value;
                    Ok(())
                },
            )
        })
    }

    /// Gets the TCLASS/TOS option.
    pub fn get_dscp_and_ecn(
        &mut self,
        id: &UdpApiSocketId<I, C>,
        ip_version: IpVersion,
    ) -> Result<DscpAndEcn, NotDualStackCapableError> {
        if ip_version == I::VERSION {
            return Ok(self.datagram().get_dscp_and_ecn(id));
        };

        self.datagram().with_other_stack_ip_options(id, |other_stack| {
            I::map_ip_in(
                WrapOtherStackIpOptions(other_stack),
                |_v4| Err(NotDualStackCapableError),
                |WrapOtherStackIpOptions(other_stack)| Ok(other_stack.socket_options.dscp_and_ecn),
            )
        })
    }

    /// Sets the TCLASS/TOS option.
    pub fn set_dscp_and_ecn(
        &mut self,
        id: &UdpApiSocketId<I, C>,
        value: DscpAndEcn,
        ip_version: IpVersion,
    ) -> Result<(), NotDualStackCapableError> {
        if ip_version == I::VERSION {
            self.datagram().set_dscp_and_ecn(id, value);
            return Ok(());
        };

        self.datagram().with_other_stack_ip_options_mut(id, |other_stack| {
            I::map_ip(
                (IpInvariant(value), WrapOtherStackIpOptionsMut(other_stack)),
                |(IpInvariant(_interface), _v4)| Err(NotDualStackCapableError),
                |(IpInvariant(value), WrapOtherStackIpOptionsMut(other_stack))| {
                    other_stack.socket_options.dscp_and_ecn = value;
                    Ok(())
                },
            )
        })
    }

    /// Sets the send buffer maximum size to `size`.
    pub fn set_send_buffer(&mut self, id: &UdpApiSocketId<I, C>, size: usize) {
        self.datagram().set_send_buffer(id, size)
    }

    /// Returns the current maximum send buffer size.
    pub fn send_buffer(&mut self, id: &UdpApiSocketId<I, C>) -> usize {
        self.datagram().send_buffer(id)
    }

    /// Returns the currently available send buffer space on the socket.
    #[cfg(any(test, feature = "testutils"))]
    pub fn send_buffer_available(&mut self, id: &UdpApiSocketId<I, C>) -> usize {
        self.datagram().send_buffer_available(id)
    }

    /// Disconnects a connected UDP socket.
    ///
    /// `disconnect` removes an existing connected socket and replaces it with a
    /// listening socket bound to the same local address and port.
    ///
    /// # Errors
    ///
    /// Returns an error if the socket is not connected.
    pub fn disconnect(&mut self, id: &UdpApiSocketId<I, C>) -> Result<(), ExpectedConnError> {
        debug!("disconnect {id:?}");
        self.datagram().disconnect_connected(id)
    }

    /// Shuts down a socket for reading and/or writing.
    ///
    /// # Errors
    ///
    /// Returns an error if the socket is not connected.
    pub fn shutdown(
        &mut self,
        id: &UdpApiSocketId<I, C>,
        which: ShutdownType,
    ) -> Result<(), ExpectedConnError> {
        debug!("shutdown {id:?} {which:?}");
        self.datagram().shutdown_connected(id, which)
    }

    /// Get the shutdown state for a socket.
    ///
    /// If the socket is not connected, or if `shutdown` was not called on it,
    /// returns `None`.
    pub fn get_shutdown(&mut self, id: &UdpApiSocketId<I, C>) -> Option<ShutdownType> {
        self.datagram().get_shutdown_connected(id)
    }

    /// Removes a socket that was previously created.
    pub fn close(
        &mut self,
        id: UdpApiSocketId<I, C>,
    ) -> RemoveResourceResultWithContext<
        (
            UdpSocketDiagnosticsSeed<
                I,
                <C::CoreContext as DeviceIdContext<AnyDevice>>::WeakDeviceId,
                C::BindingsContext,
            >,
            <C::BindingsContext as UdpBindingsTypes>::ExternalData<I>,
        ),
        C::BindingsContext,
    > {
        debug!("close {id:?}");
        let cookie = id.socket_cookie();
        self.datagram().close(id, move |reference_state| {
            let (state, external_data) = reference_state.into_state_and_external_data();
            (UdpSocketDiagnosticsSeed { state, cookie }, external_data)
        })
    }

    /// Gets the [`SocketInfo`] associated with the UDP socket referenced by
    /// `id`.
    pub fn get_info(
        &mut self,
        id: &UdpApiSocketId<I, C>,
    ) -> SocketInfo<I::Addr, <C::CoreContext as DeviceIdContext<AnyDevice>>::WeakDeviceId> {
        self.datagram().get_info(id)
    }

    /// Use an existing socket to listen for incoming UDP packets.
    ///
    /// `listen_udp` converts `id` into a listening socket and registers the new
    /// socket as a listener for incoming UDP packets on the given `port`. If
    /// `addr` is `None`, the listener is a "wildcard listener", and is bound to
    /// all local addresses. See the [`crate::transport`] module documentation
    /// for more details.
    ///
    /// If `addr` is `Some``, and `addr` is already bound on the given port
    /// (either by a listener or a connection), `listen_udp` will fail. If
    /// `addr` is `None`, and a wildcard listener is already bound to the given
    /// port, `listen_udp` will fail.
    ///
    /// # Errors
    ///
    /// Returns an error if the socket is not currently unbound.
    pub fn listen(
        &mut self,
        id: &UdpApiSocketId<I, C>,
        addr: Option<
            ZonedAddr<
                SpecifiedAddr<I::Addr>,
                <C::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId,
            >,
        >,
        port: Option<NonZeroU16>,
    ) -> Result<(), Either<ExpectedUnboundError, LocalAddressError>> {
        debug!("listen on {id:?} on {addr:?}:{port:?}");
        self.datagram().listen(id, addr, port)
    }

    /// Sends a UDP packet on an existing socket.
    ///
    /// # Errors
    ///
    /// Returns an error if the socket is not connected or the packet cannot be
    /// sent. On error, the original `body` is returned unmodified so that it
    /// can be reused by the caller.
    pub fn send<B: BufferMut>(
        &mut self,
        id: &UdpApiSocketId<I, C>,
        body: B,
        send_token: <C::BindingsContext as UdpBindingsTypes>::SendToken,
    ) -> Result<(), Either<SendError, ExpectedConnError>> {
        self.core_ctx().increment_both(id, |c| &c.tx);
        self.datagram().send_conn(id, body, send_token).map_err(|err| {
            self.core_ctx().increment_both(id, |c| &c.tx_error);
            match err {
                DatagramSendError::NotConnected => Either::Right(ExpectedConnError),
                DatagramSendError::NotWriteable => Either::Left(SendError::NotWriteable),
                DatagramSendError::SendBufferFull => Either::Left(SendError::SendBufferFull),
                DatagramSendError::InvalidLength => Either::Left(SendError::InvalidLength),
                DatagramSendError::IpSock(err) => Either::Left(SendError::IpSock(err)),
                DatagramSendError::SerializeError(err) => match err {
                    UdpSerializeError::RemotePortUnset => Either::Left(SendError::RemotePortUnset),
                },
            }
        })
    }

    /// Sends a UDP packet to the provided destination address.
    ///
    /// If this is called with an unbound socket, the socket will be implicitly
    /// bound. If that succeeds, the ID for the new socket is returned.
    ///
    /// # Errors
    ///
    /// Returns an error if the socket is unbound and connecting fails, or if the
    /// packet could not be sent. If the socket is unbound and connecting succeeds
    /// but sending fails, the socket remains connected.
    pub fn send_to<B: BufferMut>(
        &mut self,
        id: &UdpApiSocketId<I, C>,
        remote_ip: Option<
            ZonedAddr<
                SpecifiedAddr<I::Addr>,
                <C::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId,
            >,
        >,
        remote_port: UdpRemotePort,
        body: B,
        send_token: <C::BindingsContext as UdpBindingsTypes>::SendToken,
    ) -> Result<(), SendToError> {
        // Match Linux's behavior and verify the remote port is set.
        match remote_port {
            UdpRemotePort::Unset => return Err(SendToError::RemotePortUnset),
            UdpRemotePort::Set(_) => {}
        }

        self.core_ctx().increment_both(id, |c| &c.tx);
        self.datagram().send_to(id, remote_ip, remote_port, body, send_token).map_err(|e| {
            self.core_ctx().increment_both(id, |c| &c.tx_error);
            match e {
                datagram::SendToError::LocalAddress(e) => SendToError::LocalAddress(e),
                datagram::SendToError::SerializeError(err) => match err {
                    UdpSerializeError::RemotePortUnset => SendToError::RemotePortUnset,
                },
                datagram::SendToError::NotWriteable => SendToError::NotWriteable,
                datagram::SendToError::SendBufferFull => SendToError::SendBufferFull,
                datagram::SendToError::InvalidLength => SendToError::InvalidLength,
                datagram::SendToError::Zone(e) => SendToError::Zone(e),
                datagram::SendToError::CreateAndSend(e) => match e {
                    IpSockCreateAndSendError::Send(e) => SendToError::Send(e),
                    IpSockCreateAndSendError::Create(e) => SendToError::CreateSock(e),
                },
                datagram::SendToError::RemoteUnexpectedlyMapped => {
                    SendToError::RemoteUnexpectedlyMapped
                }
                datagram::SendToError::RemoteUnexpectedlyNonMapped => {
                    SendToError::RemoteUnexpectedlyNonMapped
                }
            }
        })
    }

    /// Collects all currently opened sockets, returning a cloned reference for
    /// each one.
    pub fn collect_all_sockets(&mut self) -> Vec<UdpApiSocketId<I, C>> {
        self.datagram().collect_all_sockets()
    }

    /// Provides inspect data for UDP sockets.
    pub fn inspect<N>(&mut self, inspector: &mut N)
    where
        N: Inspector
            + InspectorDeviceExt<<C::CoreContext as DeviceIdContext<AnyDevice>>::WeakDeviceId>,
        for<'a> N::ChildInspector<'a>:
            InspectorDeviceExt<<C::CoreContext as DeviceIdContext<AnyDevice>>::WeakDeviceId>,
    {
        DatagramStateContext::for_each_socket(self.core_ctx(), |_ctx, socket_id, socket_state| {
            inspector.record_debug_child(socket_id, |inspector| {
                socket_state.record_common_info(inspector);
                inspector.record_child("Counters", |inspector| {
                    inspector.delegate_inspectable(&CombinedUdpCounters {
                        with_socket: socket_id.counters(),
                        without_socket: None,
                    });
                });
            });
        });
    }
}

/// Error when sending a packet on a socket.
#[derive(Copy, Clone, Debug, Eq, PartialEq, GenericOverIp, Error)]
#[generic_over_ip()]
pub enum SendError {
    /// The socket is not writeable.
    #[error("socket not writable")]
    NotWriteable,
    /// The packet couldn't be sent.
    #[error("packet couldn't be sent: {0}")]
    IpSock(#[from] IpSockSendError),
    /// Disallow sending packets with a remote port of 0. See
    /// [`UdpRemotePort::Unset`] for the rationale.
    #[error("remote port unset")]
    RemotePortUnset,
    /// The socket's send buffer is full.
    #[error("send buffer is full")]
    SendBufferFull,
    /// Invalid message length.
    #[error("invalid message length")]
    InvalidLength,
}

impl<I: IpExt, BC: UdpBindingsContext<I, CC::DeviceId>, CC: StateContext<I, BC>>
    DatagramSpecStateContext<I, CC, BC> for Udp<BC>
{
    type SocketsStateCtx<'a> = CC::SocketStateCtx<'a>;

    fn with_all_sockets_mut<O, F: FnOnce(&mut UdpSocketSet<I, CC::WeakDeviceId, BC>) -> O>(
        core_ctx: &mut CC,
        cb: F,
    ) -> O {
        StateContext::with_all_sockets_mut(core_ctx, cb)
    }

    fn with_all_sockets<O, F: FnOnce(&UdpSocketSet<I, CC::WeakDeviceId, BC>) -> O>(
        core_ctx: &mut CC,
        cb: F,
    ) -> O {
        StateContext::with_all_sockets(core_ctx, cb)
    }

    fn with_socket_state<
        O,
        F: FnOnce(&mut Self::SocketsStateCtx<'_>, &UdpSocketState<I, CC::WeakDeviceId, BC>) -> O,
    >(
        core_ctx: &mut CC,
        id: &UdpSocketId<I, CC::WeakDeviceId, BC>,
        cb: F,
    ) -> O {
        StateContext::with_socket_state(core_ctx, id, cb)
    }

    fn with_socket_state_mut<
        O,
        F: FnOnce(&mut Self::SocketsStateCtx<'_>, &mut UdpSocketState<I, CC::WeakDeviceId, BC>) -> O,
    >(
        core_ctx: &mut CC,
        id: &UdpSocketId<I, CC::WeakDeviceId, BC>,
        cb: F,
    ) -> O {
        StateContext::with_socket_state_mut(core_ctx, id, cb)
    }

    fn for_each_socket<
        F: FnMut(
            &mut Self::SocketsStateCtx<'_>,
            &UdpSocketId<I, CC::WeakDeviceId, BC>,
            &UdpSocketState<I, CC::WeakDeviceId, BC>,
        ),
    >(
        core_ctx: &mut CC,
        cb: F,
    ) {
        StateContext::for_each_socket(core_ctx, cb)
    }
}

impl<
    I: IpExt,
    BC: UdpBindingsContext<I, CC::DeviceId>,
    CC: BoundStateContext<I, BC> + UdpStateContext,
> DatagramSpecBoundStateContext<I, CC, BC> for Udp<BC>
{
    type IpSocketsCtx<'a> = CC::IpSocketsCtx<'a>;

    fn with_bound_sockets<O, F>(core_ctx: &mut CC, cb: F) -> O
    where
        F: FnOnce(&mut Self::IpSocketsCtx<'_>, &UdpBoundSocketMap<I, CC::WeakDeviceId, BC>) -> O,
    {
        core_ctx.with_bound_sockets(|core_ctx, BoundSockets { bound_sockets }| {
            cb(core_ctx, bound_sockets)
        })
    }

    fn with_bound_sockets_mut<O, F>(core_ctx: &mut CC, cb: F) -> O
    where
        F: FnOnce(
            &mut Self::IpSocketsCtx<'_>,
            &mut UdpBoundSocketMap<I, CC::WeakDeviceId, BC>,
        ) -> O,
    {
        core_ctx.with_bound_sockets_mut(|core_ctx, BoundSockets { bound_sockets }| {
            cb(core_ctx, bound_sockets)
        })
    }

    type DualStackContext = CC::DualStackContext;
    type NonDualStackContext = CC::NonDualStackContext;
    fn dual_stack_context_mut(
        core_ctx: &mut CC,
    ) -> MaybeDualStack<&mut Self::DualStackContext, &mut Self::NonDualStackContext> {
        BoundStateContext::dual_stack_context_mut(core_ctx)
    }

    fn dual_stack_context(
        core_ctx: &CC,
    ) -> MaybeDualStack<&Self::DualStackContext, &Self::NonDualStackContext> {
        BoundStateContext::dual_stack_context(core_ctx)
    }

    fn with_transport_context<O, F>(core_ctx: &mut CC, cb: F) -> O
    where
        F: FnOnce(&mut Self::IpSocketsCtx<'_>) -> O,
    {
        core_ctx.with_transport_context(cb)
    }
}

impl<
    BC: UdpBindingsContext<Ipv6, CC::DeviceId> + UdpBindingsContext<Ipv4, CC::DeviceId>,
    CC: DualStackBoundStateContext<Ipv6, BC> + UdpStateContext,
> DualStackDatagramSpecBoundStateContext<Ipv6, CC, BC> for Udp<BC>
{
    type IpSocketsCtx<'a> = CC::IpSocketsCtx<'a>;
    fn dual_stack_enabled(
        _core_ctx: &CC,
        ip_options: &IpOptions<Ipv6, CC::WeakDeviceId, Udp<BC>>,
    ) -> bool {
        let DualStackSocketState { dual_stack_enabled, .. } = ip_options.other_stack();
        *dual_stack_enabled
    }

    fn to_other_socket_options<'a>(
        _core_ctx: &CC,
        state: &'a IpOptions<Ipv6, CC::WeakDeviceId, Udp<BC>>,
    ) -> &'a DatagramIpSpecificSocketOptions<Ipv4, CC::WeakDeviceId> {
        &state.other_stack().socket_options
    }

    fn ds_converter(_core_ctx: &CC) -> impl DualStackConverter<Ipv6, CC::WeakDeviceId, Self> {
        ()
    }

    fn to_other_bound_socket_id(
        _core_ctx: &CC,
        id: &UdpSocketId<Ipv6, CC::WeakDeviceId, BC>,
    ) -> EitherIpSocket<CC::WeakDeviceId, Udp<BC>> {
        EitherIpSocket::V6(id.clone())
    }

    fn with_both_bound_sockets_mut<O, F>(core_ctx: &mut CC, cb: F) -> O
    where
        F: FnOnce(
            &mut Self::IpSocketsCtx<'_>,
            &mut UdpBoundSocketMap<Ipv6, CC::WeakDeviceId, BC>,
            &mut UdpBoundSocketMap<Ipv4, CC::WeakDeviceId, BC>,
        ) -> O,
    {
        core_ctx.with_both_bound_sockets_mut(
            |core_ctx,
             BoundSockets { bound_sockets: bound_first },
             BoundSockets { bound_sockets: bound_second }| {
                cb(core_ctx, bound_first, bound_second)
            },
        )
    }

    fn with_other_bound_sockets_mut<
        O,
        F: FnOnce(
            &mut Self::IpSocketsCtx<'_>,
            &mut UdpBoundSocketMap<Ipv4, CC::WeakDeviceId, BC>,
        ) -> O,
    >(
        core_ctx: &mut CC,
        cb: F,
    ) -> O {
        core_ctx.with_other_bound_sockets_mut(|core_ctx, BoundSockets { bound_sockets }| {
            cb(core_ctx, bound_sockets)
        })
    }

    fn with_transport_context<O, F: FnOnce(&mut Self::IpSocketsCtx<'_>) -> O>(
        core_ctx: &mut CC,
        cb: F,
    ) -> O {
        core_ctx.with_transport_context(|core_ctx| cb(core_ctx))
    }
}

impl<
    BC: UdpBindingsContext<Ipv4, CC::DeviceId>,
    CC: BoundStateContext<Ipv4, BC> + NonDualStackBoundStateContext<Ipv4, BC> + UdpStateContext,
> NonDualStackDatagramSpecBoundStateContext<Ipv4, CC, BC> for Udp<BC>
{
    fn nds_converter(_core_ctx: &CC) -> impl NonDualStackConverter<Ipv4, CC::WeakDeviceId, Self> {
        ()
    }
}

#[cfg(test)]
pub(crate) mod testutils {
    use alloc::borrow::ToOwned;
    use alloc::vec;
    use core::ops::{Deref, DerefMut};
    use netstack3_ip::IpLayerIpExt;

    use net_types::ip::{IpAddr, Ipv4, Ipv4Addr, Ipv4SourceAddr, Ipv6, Ipv6Addr, Ipv6SourceAddr};
    use netstack3_base::testutil::{
        FakeBindingsCtx, FakeCoreCtx, FakeDeviceId, FakeSendToken, FakeSocketWritableListener,
        FakeStrongDeviceId, FakeWeakDeviceId,
    };
    use netstack3_base::{CtxPair, ResourceCounterContext, UninstantiableWrapper};
    use netstack3_hashmap::HashMap;
    use netstack3_ip::device::IpDeviceStateIpExt;
    use netstack3_ip::socket::testutil::{FakeDeviceConfig, FakeDualStackIpSocketCtx};
    use netstack3_ip::testutil::DualStackSendIpPacketMeta;

    use super::*;
    /// A packet received on a socket.
    #[derive(Debug, Derivative, PartialEq)]
    #[derivative(Default(bound = ""))]
    pub(crate) struct SocketReceived<I: Ip> {
        pub(crate) packets: Vec<ReceivedPacket<I>>,
        #[derivative(Default(value = "usize::MAX"))]
        pub(crate) max_size: usize,
    }

    #[derive(Debug, PartialEq)]
    pub(crate) struct ReceivedPacket<I: Ip> {
        pub(crate) meta: UdpPacketMeta<I>,
        pub(crate) body: Vec<u8>,
    }

    impl<D: FakeStrongDeviceId> FakeUdpCoreCtx<D> {
        pub(crate) fn new_with_device<I: TestIpExt>(device: D) -> Self {
            Self::with_local_remote_ip_addrs_and_device(
                vec![local_ip::<I>()],
                vec![remote_ip::<I>()],
                device,
            )
        }

        fn with_local_remote_ip_addrs_and_device<A: Into<SpecifiedAddr<IpAddr>>>(
            local_ips: Vec<A>,
            remote_ips: Vec<A>,
            device: D,
        ) -> Self {
            Self::with_ip_socket_ctx_state(FakeDualStackIpSocketCtx::new([FakeDeviceConfig {
                device,
                local_ips,
                remote_ips,
            }]))
        }

        pub(crate) fn with_ip_socket_ctx_state(state: FakeDualStackIpSocketCtx<D>) -> Self {
            Self {
                all_sockets: Default::default(),
                bound_sockets: FakeUdpBoundSocketsCtx {
                    bound_sockets: Default::default(),
                    ip_socket_ctx: InnerIpSocketCtx::with_state(state),
                },
            }
        }
    }

    impl FakeUdpCoreCtx<FakeDeviceId> {
        pub(crate) fn new_fake_device<I: TestIpExt>() -> Self {
            Self::new_with_device::<I>(FakeDeviceId)
        }

        pub(crate) fn with_local_remote_ip_addrs<A: Into<SpecifiedAddr<IpAddr>>>(
            local_ips: Vec<A>,
            remote_ips: Vec<A>,
        ) -> Self {
            Self::with_local_remote_ip_addrs_and_device(local_ips, remote_ips, FakeDeviceId)
        }
    }

    /// UDP tests context pair.
    pub(crate) type FakeUdpCtx<D> = CtxPair<FakeUdpCoreCtx<D>, FakeUdpBindingsCtx<D>>;

    #[derive(Derivative)]
    #[derivative(Default(bound = ""))]
    pub(crate) struct FakeBoundSockets<D: StrongDeviceIdentifier> {
        v4: BoundSockets<Ipv4, D::Weak, FakeUdpBindingsCtx<D>>,
        v6: BoundSockets<Ipv6, D::Weak, FakeUdpBindingsCtx<D>>,
    }

    impl<D: StrongDeviceIdentifier> FakeBoundSockets<D> {
        fn bound_sockets<I: IpExt>(&self) -> &BoundSockets<I, D::Weak, FakeUdpBindingsCtx<D>> {
            I::map_ip_out(self, |state| &state.v4, |state| &state.v6)
        }

        fn bound_sockets_mut<I: IpExt>(
            &mut self,
        ) -> &mut BoundSockets<I, D::Weak, FakeUdpBindingsCtx<D>> {
            I::map_ip_out(self, |state| &mut state.v4, |state| &mut state.v6)
        }
    }

    pub(crate) struct FakeUdpBoundSocketsCtx<D: FakeStrongDeviceId> {
        pub(crate) bound_sockets: FakeBoundSockets<D>,
        pub(crate) ip_socket_ctx: InnerIpSocketCtx<D>,
    }

    /// `FakeBindingsCtx` specialized for UDP.
    pub(crate) type FakeUdpBindingsCtx<D> = FakeBindingsCtx<(), (), FakeBindingsCtxState<D>, ()>;

    /// The inner context providing a fake IP socket context to
    /// [`FakeUdpBoundSocketsCtx`].
    type InnerIpSocketCtx<D> =
        FakeCoreCtx<FakeDualStackIpSocketCtx<D>, DualStackSendIpPacketMeta<D>, D>;

    pub(crate) type UdpFakeDeviceCtx = FakeUdpCtx<FakeDeviceId>;
    pub(crate) type UdpFakeDeviceCoreCtx = FakeUdpCoreCtx<FakeDeviceId>;

    #[derive(Derivative)]
    #[derivative(Default(bound = ""))]
    pub(crate) struct FakeBindingsCtxState<D: StrongDeviceIdentifier> {
        received_v4:
            HashMap<WeakUdpSocketId<Ipv4, D::Weak, FakeUdpBindingsCtx<D>>, SocketReceived<Ipv4>>,
        received_v6:
            HashMap<WeakUdpSocketId<Ipv6, D::Weak, FakeUdpBindingsCtx<D>>, SocketReceived<Ipv6>>,
        pending_errors_v4: HashMap<
            WeakUdpSocketId<Ipv4, D::Weak, FakeUdpBindingsCtx<D>>,
            Option<PendingDatagramSocketError>,
        >,
        pending_errors_v6: HashMap<
            WeakUdpSocketId<Ipv6, D::Weak, FakeUdpBindingsCtx<D>>,
            Option<PendingDatagramSocketError>,
        >,
    }

    impl<D: StrongDeviceIdentifier> FakeBindingsCtxState<D> {
        pub(crate) fn received<I: TestIpExt>(
            &self,
        ) -> &HashMap<WeakUdpSocketId<I, D::Weak, FakeUdpBindingsCtx<D>>, SocketReceived<I>>
        {
            #[derive(GenericOverIp)]
            #[generic_over_ip(I, Ip)]
            struct Wrap<'a, I: TestIpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes>(
                &'a HashMap<WeakUdpSocketId<I, D, BT>, SocketReceived<I>>,
            );
            let Wrap(map) = I::map_ip_out(
                self,
                |state| Wrap(&state.received_v4),
                |state| Wrap(&state.received_v6),
            );
            map
        }

        pub(crate) fn received_mut<I: IpExt>(
            &mut self,
        ) -> &mut HashMap<WeakUdpSocketId<I, D::Weak, FakeUdpBindingsCtx<D>>, SocketReceived<I>>
        {
            #[derive(GenericOverIp)]
            #[generic_over_ip(I, Ip)]
            struct Wrap<'a, I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes>(
                &'a mut HashMap<WeakUdpSocketId<I, D, BT>, SocketReceived<I>>,
            );
            let Wrap(map) = I::map_ip_out(
                self,
                |state| Wrap(&mut state.received_v4),
                |state| Wrap(&mut state.received_v6),
            );
            map
        }

        pub(crate) fn pending_errors_mut<I: IpExt>(
            &mut self,
        ) -> &mut HashMap<
            WeakUdpSocketId<I, D::Weak, FakeUdpBindingsCtx<D>>,
            Option<PendingDatagramSocketError>,
        > {
            #[derive(GenericOverIp)]
            #[generic_over_ip(I, Ip)]
            struct Wrap<'a, I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes>(
                &'a mut HashMap<WeakUdpSocketId<I, D, BT>, Option<PendingDatagramSocketError>>,
            );
            let Wrap(map) = I::map_ip_out(
                self,
                |state| Wrap(&mut state.pending_errors_v4),
                |state| Wrap(&mut state.pending_errors_v6),
            );
            map
        }

        pub(crate) fn take_pending_error<I: IpExt>(
            &mut self,
            id: &WeakUdpSocketId<I, D::Weak, FakeUdpBindingsCtx<D>>,
        ) -> Option<PendingDatagramSocketError> {
            self.pending_errors_mut::<I>().remove(id).flatten()
        }

        pub(crate) fn socket_data<I: TestIpExt>(
            &self,
        ) -> HashMap<WeakUdpSocketId<I, D::Weak, FakeUdpBindingsCtx<D>>, Vec<&'_ [u8]>> {
            self.received::<I>()
                .iter()
                .map(|(id, SocketReceived { packets, .. })| {
                    (
                        id.clone(),
                        packets.iter().map(|ReceivedPacket { meta: _, body }| &body[..]).collect(),
                    )
                })
                .collect()
        }
    }

    impl<I: IpExt, D: StrongDeviceIdentifier> UdpReceiveBindingsContext<I, D>
        for FakeUdpBindingsCtx<D>
    {
        fn receive_udp(
            &mut self,
            id: &UdpSocketId<I, D::Weak, Self>,
            _device_id: &D,
            meta: UdpPacketMeta<I>,
            body: &[u8],
        ) -> Result<(), ReceiveUdpError> {
            let SocketReceived { packets, max_size } =
                self.state.received_mut::<I>().entry(id.downgrade()).or_default();
            if packets.len() < *max_size {
                packets.push(ReceivedPacket { meta, body: body.to_owned() });
                Ok(())
            } else {
                Err(ReceiveUdpError::QueueFull)
            }
        }

        fn on_socket_error(
            &mut self,
            id: &UdpSocketId<I, D::Weak, Self>,
            err: PendingDatagramSocketError,
        ) {
            let _ = self.state.pending_errors_mut::<I>().insert(id.downgrade(), Some(err));
        }
    }

    impl<D: StrongDeviceIdentifier> UdpBindingsTypes for FakeUdpBindingsCtx<D> {
        type ExternalData<I: Ip> = ();
        type SocketWritableListener = FakeSocketWritableListener;
        type SendToken = FakeSendToken;
    }

    /// Utilities for accessing locked internal state in tests.
    impl<I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes> UdpSocketId<I, D, BT> {
        fn get(&self) -> impl Deref<Target = UdpSocketState<I, D, BT>> + '_ {
            self.state().read()
        }

        fn get_mut(&self) -> impl DerefMut<Target = UdpSocketState<I, D, BT>> + '_ {
            self.state().write()
        }
    }

    impl<D: FakeStrongDeviceId> DeviceIdContext<AnyDevice> for FakeUdpCoreCtx<D> {
        type DeviceId = D;
        type WeakDeviceId = FakeWeakDeviceId<D>;
    }

    impl<D: FakeStrongDeviceId> DeviceIdContext<AnyDevice> for FakeUdpBoundSocketsCtx<D> {
        type DeviceId = D;
        type WeakDeviceId = FakeWeakDeviceId<D>;
    }

    impl<I: TestIpExt, D: FakeStrongDeviceId> StateContext<I, FakeUdpBindingsCtx<D>>
        for FakeUdpCoreCtx<D>
    {
        type SocketStateCtx<'a> = FakeUdpBoundSocketsCtx<D>;

        fn with_all_sockets_mut<
            O,
            F: FnOnce(&mut UdpSocketSet<I, Self::WeakDeviceId, FakeUdpBindingsCtx<D>>) -> O,
        >(
            &mut self,
            cb: F,
        ) -> O {
            cb(self.all_sockets.socket_set_mut())
        }

        fn with_all_sockets<
            O,
            F: FnOnce(&UdpSocketSet<I, Self::WeakDeviceId, FakeUdpBindingsCtx<D>>) -> O,
        >(
            &mut self,
            cb: F,
        ) -> O {
            cb(self.all_sockets.socket_set())
        }

        fn with_socket_state<
            O,
            F: FnOnce(
                &mut Self::SocketStateCtx<'_>,
                &UdpSocketState<I, Self::WeakDeviceId, FakeUdpBindingsCtx<D>>,
            ) -> O,
        >(
            &mut self,
            id: &UdpSocketId<I, Self::WeakDeviceId, FakeUdpBindingsCtx<D>>,
            cb: F,
        ) -> O {
            cb(&mut self.bound_sockets, &id.get())
        }

        fn with_socket_state_mut<
            O,
            F: FnOnce(
                &mut Self::SocketStateCtx<'_>,
                &mut UdpSocketState<I, Self::WeakDeviceId, FakeUdpBindingsCtx<D>>,
            ) -> O,
        >(
            &mut self,
            id: &UdpSocketId<I, Self::WeakDeviceId, FakeUdpBindingsCtx<D>>,
            cb: F,
        ) -> O {
            cb(&mut self.bound_sockets, &mut id.get_mut())
        }

        fn with_bound_state_context<O, F: FnOnce(&mut Self::SocketStateCtx<'_>) -> O>(
            &mut self,
            cb: F,
        ) -> O {
            cb(&mut self.bound_sockets)
        }

        fn for_each_socket<
            F: FnMut(
                &mut Self::SocketStateCtx<'_>,
                &UdpSocketId<I, Self::WeakDeviceId, FakeUdpBindingsCtx<D>>,
                &UdpSocketState<I, Self::WeakDeviceId, FakeUdpBindingsCtx<D>>,
            ),
        >(
            &mut self,
            mut cb: F,
        ) {
            self.all_sockets.socket_set().keys().for_each(|id| {
                let id = UdpSocketId::from(id.clone());
                cb(&mut self.bound_sockets, &id, &id.get());
            })
        }
    }

    impl<I: TestIpExt, D: FakeStrongDeviceId> BoundStateContext<I, FakeUdpBindingsCtx<D>>
        for FakeUdpBoundSocketsCtx<D>
    {
        type IpSocketsCtx<'a> = InnerIpSocketCtx<D>;
        type DualStackContext = I::UdpDualStackBoundStateContext<D>;
        type NonDualStackContext = I::UdpNonDualStackBoundStateContext<D>;

        fn with_bound_sockets<
            O,
            F: FnOnce(
                &mut Self::IpSocketsCtx<'_>,
                &BoundSockets<I, Self::WeakDeviceId, FakeUdpBindingsCtx<D>>,
            ) -> O,
        >(
            &mut self,
            cb: F,
        ) -> O {
            let Self { bound_sockets, ip_socket_ctx } = self;
            cb(ip_socket_ctx, bound_sockets.bound_sockets())
        }

        fn with_bound_sockets_mut<
            O,
            F: FnOnce(
                &mut Self::IpSocketsCtx<'_>,
                &mut BoundSockets<I, Self::WeakDeviceId, FakeUdpBindingsCtx<D>>,
            ) -> O,
        >(
            &mut self,
            cb: F,
        ) -> O {
            let Self { bound_sockets, ip_socket_ctx } = self;
            cb(ip_socket_ctx, bound_sockets.bound_sockets_mut())
        }

        fn with_transport_context<O, F: FnOnce(&mut Self::IpSocketsCtx<'_>) -> O>(
            &mut self,
            cb: F,
        ) -> O {
            cb(&mut self.ip_socket_ctx)
        }

        fn dual_stack_context(
            &self,
        ) -> MaybeDualStack<&Self::DualStackContext, &Self::NonDualStackContext> {
            struct Wrap<'a, I: TestIpExt, D: FakeStrongDeviceId + 'static>(
                MaybeDualStack<
                    &'a I::UdpDualStackBoundStateContext<D>,
                    &'a I::UdpNonDualStackBoundStateContext<D>,
                >,
            );
            // TODO(https://fxbug.dev/42082123): Replace this with a derived impl.
            impl<'a, I: TestIpExt, NewIp: TestIpExt, D: FakeStrongDeviceId + 'static>
                GenericOverIp<NewIp> for Wrap<'a, I, D>
            {
                type Type = Wrap<'a, NewIp, D>;
            }

            let Wrap(context) = I::map_ip_out(
                self,
                |this| Wrap(MaybeDualStack::NotDualStack(this)),
                |this| Wrap(MaybeDualStack::DualStack(this)),
            );
            context
        }

        fn dual_stack_context_mut(
            &mut self,
        ) -> MaybeDualStack<&mut Self::DualStackContext, &mut Self::NonDualStackContext> {
            struct Wrap<'a, I: TestIpExt, D: FakeStrongDeviceId + 'static>(
                MaybeDualStack<
                    &'a mut I::UdpDualStackBoundStateContext<D>,
                    &'a mut I::UdpNonDualStackBoundStateContext<D>,
                >,
            );
            // TODO(https://fxbug.dev/42082123): Replace this with a derived impl.
            impl<'a, I: TestIpExt, NewIp: TestIpExt, D: FakeStrongDeviceId + 'static>
                GenericOverIp<NewIp> for Wrap<'a, I, D>
            {
                type Type = Wrap<'a, NewIp, D>;
            }

            let Wrap(context) = I::map_ip_out(
                self,
                |this| Wrap(MaybeDualStack::NotDualStack(this)),
                |this| Wrap(MaybeDualStack::DualStack(this)),
            );
            context
        }
    }

    impl<D: FakeStrongDeviceId + 'static> UdpStateContext for FakeUdpBoundSocketsCtx<D> {}

    impl<D: FakeStrongDeviceId> NonDualStackBoundStateContext<Ipv4, FakeUdpBindingsCtx<D>>
        for FakeUdpBoundSocketsCtx<D>
    {
    }

    impl<D: FakeStrongDeviceId> DualStackBoundStateContext<Ipv6, FakeUdpBindingsCtx<D>>
        for FakeUdpBoundSocketsCtx<D>
    {
        type IpSocketsCtx<'a> = InnerIpSocketCtx<D>;

        fn with_both_bound_sockets_mut<
            O,
            F: FnOnce(
                &mut Self::IpSocketsCtx<'_>,
                &mut BoundSockets<Ipv6, Self::WeakDeviceId, FakeUdpBindingsCtx<D>>,
                &mut BoundSockets<Ipv4, Self::WeakDeviceId, FakeUdpBindingsCtx<D>>,
            ) -> O,
        >(
            &mut self,
            cb: F,
        ) -> O {
            let Self { ip_socket_ctx, bound_sockets: FakeBoundSockets { v4, v6 } } = self;
            cb(ip_socket_ctx, v6, v4)
        }

        fn with_other_bound_sockets_mut<
            O,
            F: FnOnce(
                &mut Self::IpSocketsCtx<'_>,
                &mut BoundSockets<Ipv4, Self::WeakDeviceId, FakeUdpBindingsCtx<D>>,
            ) -> O,
        >(
            &mut self,
            cb: F,
        ) -> O {
            DualStackBoundStateContext::with_both_bound_sockets_mut(
                self,
                |core_ctx, _bound, other_bound| cb(core_ctx, other_bound),
            )
        }

        fn with_transport_context<O, F: FnOnce(&mut Self::IpSocketsCtx<'_>) -> O>(
            &mut self,
            cb: F,
        ) -> O {
            cb(&mut self.ip_socket_ctx)
        }
    }

    /// Ip packet delivery for the [`FakeUdpCoreCtx`].
    impl<I: IpLayerIpExt + TestIpExt, D: FakeStrongDeviceId>
        IpTransportContext<I, FakeUdpBindingsCtx<D>, FakeUdpCoreCtx<D>> for UdpIpTransportContext
    {
        type EarlyDemuxSocket = DualStackUdpSocketId<I, D::Weak, FakeUdpBindingsCtx<D>>;

        fn early_demux<B: ParseBuffer>(
            core_ctx: &mut FakeUdpCoreCtx<D>,
            device: &D,
            src_ip: I::Addr,
            dst_ip: I::Addr,
            buffer: B,
        ) -> Option<Self::EarlyDemuxSocket> {
            early_demux_ip_packet::<I, _, _, _>(core_ctx, device, src_ip, dst_ip, buffer)
        }

        fn receive_icmp_error(
            core_ctx: &mut FakeUdpCoreCtx<D>,
            bindings_ctx: &mut FakeUdpBindingsCtx<D>,
            device: &D,
            original_src_ip: Option<SpecifiedAddr<I::Addr>>,
            original_dst_ip: SpecifiedAddr<I::Addr>,
            original_body: &[u8],
            err: I::ErrorCode,
        ) {
            CounterContext::<UdpCountersWithoutSocket<I>>::counters(core_ctx)
                .rx_icmp_error
                .increment();
            receive_icmp_error::<I, _, _>(
                core_ctx,
                bindings_ctx,
                device,
                original_src_ip,
                original_dst_ip,
                original_body,
                err,
            )
        }

        fn receive_ip_packet<B: BufferMut, H: IpHeaderInfo<I>>(
            core_ctx: &mut FakeUdpCoreCtx<D>,
            bindings_ctx: &mut FakeUdpBindingsCtx<D>,
            device: &D,
            src_ip: I::RecvSrcAddr,
            dst_ip: SpecifiedAddr<I::Addr>,
            buffer: B,
            info: &mut LocalDeliveryPacketInfo<I, H>,
            early_demux_socket: Option<Self::EarlyDemuxSocket>,
        ) -> Result<(), (B, I::IcmpError)> {
            receive_ip_packet::<I, _, _, _, _>(
                core_ctx,
                bindings_ctx,
                device,
                src_ip,
                dst_ip,
                buffer,
                info,
                early_demux_socket,
            )
        }
    }

    #[derive(Derivative)]
    #[derivative(Default(bound = ""))]
    pub(crate) struct FakeDualStackSocketState<D: StrongDeviceIdentifier> {
        v4: UdpSocketSet<Ipv4, D::Weak, FakeUdpBindingsCtx<D>>,
        v6: UdpSocketSet<Ipv6, D::Weak, FakeUdpBindingsCtx<D>>,
        udpv4_counters_with_socket: UdpCountersWithSocket<Ipv4>,
        udpv6_counters_with_socket: UdpCountersWithSocket<Ipv6>,
        udpv4_counters_without_socket: UdpCountersWithoutSocket<Ipv4>,
        udpv6_counters_without_socket: UdpCountersWithoutSocket<Ipv6>,
    }

    impl<D: StrongDeviceIdentifier> FakeDualStackSocketState<D> {
        fn socket_set<I: IpExt>(&self) -> &UdpSocketSet<I, D::Weak, FakeUdpBindingsCtx<D>> {
            I::map_ip_out(self, |dual| &dual.v4, |dual| &dual.v6)
        }

        fn socket_set_mut<I: IpExt>(
            &mut self,
        ) -> &mut UdpSocketSet<I, D::Weak, FakeUdpBindingsCtx<D>> {
            I::map_ip_out(self, |dual| &mut dual.v4, |dual| &mut dual.v6)
        }

        fn udp_counters_with_socket<I: Ip>(&self) -> &UdpCountersWithSocket<I> {
            I::map_ip_out(
                self,
                |dual| &dual.udpv4_counters_with_socket,
                |dual| &dual.udpv6_counters_with_socket,
            )
        }
        fn udp_counters_without_socket<I: Ip>(&self) -> &UdpCountersWithoutSocket<I> {
            I::map_ip_out(
                self,
                |dual| &dual.udpv4_counters_without_socket,
                |dual| &dual.udpv6_counters_without_socket,
            )
        }
    }
    pub(crate) struct FakeUdpCoreCtx<D: FakeStrongDeviceId> {
        pub(crate) bound_sockets: FakeUdpBoundSocketsCtx<D>,
        // NB: socket sets are last in the struct so all the strong refs are
        // dropped before the primary refs contained herein.
        pub(crate) all_sockets: FakeDualStackSocketState<D>,
    }

    impl<I: Ip, D: FakeStrongDeviceId> CounterContext<UdpCountersWithSocket<I>> for FakeUdpCoreCtx<D> {
        fn counters(&self) -> &UdpCountersWithSocket<I> {
            &self.all_sockets.udp_counters_with_socket()
        }
    }

    impl<I: Ip, D: FakeStrongDeviceId> CounterContext<UdpCountersWithoutSocket<I>>
        for FakeUdpCoreCtx<D>
    {
        fn counters(&self) -> &UdpCountersWithoutSocket<I> {
            &self.all_sockets.udp_counters_without_socket()
        }
    }

    impl<I: DualStackIpExt, D: FakeStrongDeviceId>
        ResourceCounterContext<
            UdpSocketId<I, FakeWeakDeviceId<D>, FakeUdpBindingsCtx<D>>,
            UdpCountersWithSocket<I>,
        > for FakeUdpCoreCtx<D>
    {
        fn per_resource_counters<'a>(
            &'a self,
            resource: &'a UdpSocketId<I, FakeWeakDeviceId<D>, FakeUdpBindingsCtx<D>>,
        ) -> &'a UdpCountersWithSocket<I> {
            resource.counters()
        }
    }

    pub(crate) fn local_ip<I: TestIpExt>() -> SpecifiedAddr<I::Addr> {
        I::get_other_ip_address(1)
    }

    pub(crate) fn remote_ip<I: TestIpExt>() -> SpecifiedAddr<I::Addr> {
        I::get_other_ip_address(2)
    }

    pub(crate) trait BaseTestIpExt:
        netstack3_base::testutil::TestIpExt + IpExt + IpDeviceStateIpExt
    {
        type UdpDualStackBoundStateContext<D: FakeStrongDeviceId + 'static>:
            DualStackDatagramBoundStateContext<Self, FakeUdpBindingsCtx<D>, Udp<FakeUdpBindingsCtx<D>>, DeviceId=D, WeakDeviceId=D::Weak>;
        type UdpNonDualStackBoundStateContext<D: FakeStrongDeviceId + 'static>:
            NonDualStackDatagramBoundStateContext<Self, FakeUdpBindingsCtx<D>, Udp<FakeUdpBindingsCtx<D>>, DeviceId=D, WeakDeviceId=D::Weak>;
        fn into_recv_src_addr(addr: Self::Addr) -> Self::RecvSrcAddr;
    }

    impl BaseTestIpExt for Ipv4 {
        type UdpDualStackBoundStateContext<D: FakeStrongDeviceId + 'static> =
            UninstantiableWrapper<FakeUdpBoundSocketsCtx<D>>;

        type UdpNonDualStackBoundStateContext<D: FakeStrongDeviceId + 'static> =
            FakeUdpBoundSocketsCtx<D>;

        fn into_recv_src_addr(addr: Ipv4Addr) -> Ipv4SourceAddr {
            Ipv4SourceAddr::new(addr).unwrap_or_else(|| panic!("{addr} is not a valid source addr"))
        }
    }

    impl BaseTestIpExt for Ipv6 {
        type UdpDualStackBoundStateContext<D: FakeStrongDeviceId + 'static> =
            FakeUdpBoundSocketsCtx<D>;
        type UdpNonDualStackBoundStateContext<D: FakeStrongDeviceId + 'static> =
            UninstantiableWrapper<FakeUdpBoundSocketsCtx<D>>;

        fn into_recv_src_addr(addr: Ipv6Addr) -> Ipv6SourceAddr {
            Ipv6SourceAddr::new(addr).unwrap_or_else(|| panic!("{addr} is not a valid source addr"))
        }
    }

    pub(crate) trait TestIpExt: BaseTestIpExt<OtherVersion: BaseTestIpExt> {}
    impl<I: BaseTestIpExt<OtherVersion: BaseTestIpExt>> TestIpExt for I {}
}

#[cfg(test)]
mod tests {
    use alloc::borrow::ToOwned;
    use alloc::vec;
    use core::convert::TryInto as _;
    use core::num::NonZeroU16;
    use packet_formats::icmp::{Icmpv4DestUnreachableCode, Icmpv6DestUnreachableCode};

    use assert_matches::assert_matches;
    use ip_test_macro::ip_test;
    use itertools::Itertools as _;
    use net_declare::{net_ip_v4 as ip_v4, net_ip_v6};
    use net_types::ip::{IpAddr, IpAddress, Ipv4, Ipv4Addr, Ipv6, Ipv6Addr};
    use net_types::{
        AddrAndZone, LinkLocalAddr, MulticastAddr, Scope as _, ScopeableAddress as _, ZonedAddr,
    };
    use netstack3_base::socket::{SocketIpAddrExt as _, StrictlyZonedAddr};
    use netstack3_base::sync::PrimaryRc;
    use netstack3_base::testutil::{
        FakeDeviceId, FakeReferencyDeviceId, FakeSendToken, FakeStrongDeviceId, FakeWeakDeviceId,
        MultipleDevicesId, TestIpExt as _, set_logger_for_test,
    };
    use netstack3_base::{
        CounterCollection, Icmpv4ErrorCode, Icmpv6ErrorCode, Mark, MarkDomain,
        NetworkSerializationContext, RemoteAddressError, SendFrameErrorReason,
    };
    use netstack3_datagram::MulticastInterfaceSelector;
    use netstack3_hashmap::{HashMap, HashSet};
    use netstack3_ip::socket::testutil::{FakeDeviceConfig, FakeDualStackIpSocketCtx};
    use netstack3_ip::testutil::{DualStackSendIpPacketMeta, FakeIpHeaderInfo};
    use netstack3_ip::{IpLayerIpExt, IpPacketDestination, ResolveRouteError, SendIpPacketMeta};
    use packet::{Buf, Serializer};
    use test_case::test_case;

    use crate::internal::counters::testutil::{
        CounterExpectationsWithSocket, CounterExpectationsWithoutSocket,
    };

    use super::testutils::{
        FakeUdpBindingsCtx, FakeUdpCoreCtx, FakeUdpCtx, ReceivedPacket, SocketReceived, TestIpExt,
        UdpFakeDeviceCoreCtx, UdpFakeDeviceCtx, local_ip, remote_ip,
    };
    use super::*;

    #[derive(Debug, PartialEq, Eq, Copy, Clone)]
    enum EarlyDemuxMode {
        Enabled,
        Disabled,
    }
    use EarlyDemuxMode::{Disabled as NoEarlyDemux, Enabled as WithEarlyDemux};

    /// Helper function to inject an UDP packet with the provided parameters.
    fn receive_udp_packet<I, D, CC>(
        core_ctx: &mut CC,
        bindings_ctx: &mut FakeUdpBindingsCtx<D>,
        device: D,
        meta: UdpPacketMeta<I>,
        body: &[u8],
        early_demux_mode: EarlyDemuxMode,
    ) -> Result<(), I::IcmpError>
    where
        UdpIpTransportContext: IpTransportContext<I, FakeUdpBindingsCtx<D>, CC>,
        I: IpLayerIpExt + TestIpExt,
        D: FakeStrongDeviceId,
        CC: DeviceIdContext<AnyDevice, DeviceId = D>,
    {
        let UdpPacketMeta { src_ip, src_port, dst_ip, dst_port, dscp_and_ecn } = meta;
        let builder = UdpPacketBuilder::new(src_ip, dst_ip, src_port, dst_port);

        let buffer = builder
            .wrap_body(Buf::new(body.to_owned(), ..))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .into_inner();

        let early_demux_socket = match early_demux_mode {
            EarlyDemuxMode::Enabled => {
                <UdpIpTransportContext as IpTransportContext<I, _, _>>::early_demux(
                    core_ctx,
                    &device,
                    src_ip,
                    dst_ip,
                    buffer.as_ref(),
                )
            }
            EarlyDemuxMode::Disabled => None,
        };

        <UdpIpTransportContext as IpTransportContext<I, _, _>>::receive_ip_packet(
            core_ctx,
            bindings_ctx,
            &device,
            I::into_recv_src_addr(src_ip),
            SpecifiedAddr::new(dst_ip).unwrap(),
            buffer,
            &mut LocalDeliveryPacketInfo {
                header_info: FakeIpHeaderInfo { dscp_and_ecn, ..Default::default() },
                ..Default::default()
            },
            early_demux_socket,
        )
        .map_err(|(_buffer, e)| e)
    }

    const LOCAL_PORT: NonZeroU16 = NonZeroU16::new(100).unwrap();
    const OTHER_LOCAL_PORT: NonZeroU16 = LOCAL_PORT.checked_add(1).unwrap();
    const REMOTE_PORT: NonZeroU16 = NonZeroU16::new(200).unwrap();
    const OTHER_REMOTE_PORT: NonZeroU16 = REMOTE_PORT.checked_add(1).unwrap();

    fn conn_addr<I>(
        device: Option<FakeWeakDeviceId<FakeDeviceId>>,
    ) -> AddrVec<I, FakeWeakDeviceId<FakeDeviceId>, UdpAddrSpec>
    where
        I: TestIpExt,
    {
        let local_ip = SocketIpAddr::try_from(local_ip::<I>()).unwrap();
        let remote_ip = SocketIpAddr::try_from(remote_ip::<I>()).unwrap();
        ConnAddr {
            ip: ConnIpAddr {
                local: (local_ip, LOCAL_PORT),
                remote: (remote_ip, REMOTE_PORT.into()),
            },
            device,
        }
        .into()
    }

    fn local_listener<I>(
        device: Option<FakeWeakDeviceId<FakeDeviceId>>,
    ) -> AddrVec<I, FakeWeakDeviceId<FakeDeviceId>, UdpAddrSpec>
    where
        I: TestIpExt,
    {
        let local_ip = SocketIpAddr::try_from(local_ip::<I>()).unwrap();
        ListenerAddr { ip: ListenerIpAddr { identifier: LOCAL_PORT, addr: Some(local_ip) }, device }
            .into()
    }

    fn wildcard_listener<I>(
        device: Option<FakeWeakDeviceId<FakeDeviceId>>,
    ) -> AddrVec<I, FakeWeakDeviceId<FakeDeviceId>, UdpAddrSpec>
    where
        I: TestIpExt,
    {
        ListenerAddr { ip: ListenerIpAddr { identifier: LOCAL_PORT, addr: None }, device }.into()
    }

    #[track_caller]
    fn assert_counters<
        'a,
        I: IpExt,
        D: WeakDeviceIdentifier,
        BT: UdpBindingsTypes,
        CC: UdpCounterContext<I, D, BT>,
    >(
        core_ctx: &CC,
        with_socket_expects: CounterExpectationsWithSocket,
        without_socket_expects: CounterExpectationsWithoutSocket,
        per_socket_expects: impl IntoIterator<
            Item = (&'a UdpSocketId<I, D, BT>, CounterExpectationsWithSocket),
        >,
    ) {
        assert_eq!(
            CounterContext::<UdpCountersWithSocket<I>>::counters(core_ctx).cast(),
            with_socket_expects
        );
        assert_eq!(
            CounterContext::<UdpCountersWithoutSocket<I>>::counters(core_ctx).cast(),
            without_socket_expects
        );
        for (id, expects) in per_socket_expects.into_iter() {
            assert_eq!(core_ctx.per_resource_counters(id).cast(), expects);
        }
    }

    #[ip_test(I)]
    #[test_case(conn_addr(Some(FakeWeakDeviceId(FakeDeviceId))), [
            conn_addr(None), local_listener(Some(FakeWeakDeviceId(FakeDeviceId))), local_listener(None),
            wildcard_listener(Some(FakeWeakDeviceId(FakeDeviceId))), wildcard_listener(None)
        ]; "conn with device")]
    #[test_case(local_listener(Some(FakeWeakDeviceId(FakeDeviceId))),
        [local_listener(None), wildcard_listener(Some(FakeWeakDeviceId(FakeDeviceId))), wildcard_listener(None)];
        "local listener with device")]
    #[test_case(wildcard_listener(Some(FakeWeakDeviceId(FakeDeviceId))), [wildcard_listener(None)];
        "wildcard listener with device")]
    #[test_case(conn_addr(None), [local_listener(None), wildcard_listener(None)]; "conn no device")]
    #[test_case(local_listener(None), [wildcard_listener(None)]; "local listener no device")]
    #[test_case(wildcard_listener(None), []; "wildcard listener no device")]
    fn test_udp_addr_vec_iter_shadows_conn<I: IpExt, D: WeakDeviceIdentifier, const N: usize>(
        addr: AddrVec<I, D, UdpAddrSpec>,
        expected_shadows: [AddrVec<I, D, UdpAddrSpec>; N],
    ) {
        assert_eq!(addr.iter_shadows().collect::<HashSet<_>>(), HashSet::from(expected_shadows));
    }

    #[ip_test(I)]
    fn test_iter_receiving_addrs<I: TestIpExt>() {
        let addr = ConnIpAddr {
            local: (SocketIpAddr::try_from(local_ip::<I>()).unwrap(), LOCAL_PORT),
            remote: (SocketIpAddr::try_from(remote_ip::<I>()).unwrap(), REMOTE_PORT.into()),
        };
        assert_eq!(
            iter_receiving_addrs::<I, _>(addr, FakeWeakDeviceId(FakeDeviceId)).collect::<Vec<_>>(),
            vec![
                // A socket connected on exactly the receiving vector has precedence.
                conn_addr(Some(FakeWeakDeviceId(FakeDeviceId))),
                // Connected takes precedence over listening with device match.
                conn_addr(None),
                local_listener(Some(FakeWeakDeviceId(FakeDeviceId))),
                // Specific IP takes precedence over device match.
                local_listener(None),
                wildcard_listener(Some(FakeWeakDeviceId(FakeDeviceId))),
                // Fallback to least specific
                wildcard_listener(None)
            ]
        );
    }

    /// Tests UDP listeners over different IP versions.
    ///
    /// Tests that a listener can be created, that the context receives packet
    /// notifications for that listener, and that we can send data using that
    /// listener.
    #[ip_test(I)]
    fn test_listen_udp<I: TestIpExt>() {
        set_logger_for_test();
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let local_ip = local_ip::<I>();
        let remote_ip = remote_ip::<I>();
        let socket = api.create();
        // Create a listener on the local port, bound to the local IP:
        api.listen(&socket, Some(ZonedAddr::Unzoned(local_ip)), Some(LOCAL_PORT))
            .expect("listen_udp failed");

        // Inject a packet and check that the context receives it:
        let body = [1, 2, 3, 4, 5];
        let (core_ctx, bindings_ctx) = api.contexts();
        let meta = UdpPacketMeta::<I> {
            src_ip: remote_ip.get(),
            src_port: Some(REMOTE_PORT),
            dst_ip: local_ip.get(),
            dst_port: LOCAL_PORT,
            dscp_and_ecn: DscpAndEcn::default(),
        };
        receive_udp_packet(
            core_ctx,
            bindings_ctx,
            FakeDeviceId,
            meta.clone(),
            &body[..],
            WithEarlyDemux,
        )
        .expect("receive udp packet should succeed");

        assert_eq!(
            bindings_ctx.state.received::<I>(),
            &HashMap::from([(
                socket.downgrade(),
                SocketReceived {
                    packets: vec![ReceivedPacket { meta, body: body.into() }],
                    max_size: usize::MAX
                }
            )])
        );

        // Send a packet providing a local ip:
        api.send_to(
            &socket,
            Some(ZonedAddr::Unzoned(remote_ip)),
            REMOTE_PORT.into(),
            Buf::new(body.to_vec(), ..),
            FakeSendToken::default(),
        )
        .expect("send_to suceeded");

        // And send a packet that doesn't:
        api.send_to(
            &socket,
            Some(ZonedAddr::Unzoned(remote_ip)),
            REMOTE_PORT.into(),
            Buf::new(body.to_vec(), ..),
            FakeSendToken::default(),
        )
        .expect("send_to succeeded");
        let frames = api.core_ctx().bound_sockets.ip_socket_ctx.frames();
        assert_eq!(frames.len(), 2);
        let check_frame =
            |(meta, frame_body): &(DualStackSendIpPacketMeta<FakeDeviceId>, Vec<u8>)| {
                let SendIpPacketMeta {
                    device: _,
                    src_ip,
                    dst_ip,
                    destination,
                    proto,
                    ttl: _,
                    mtu: _,
                    dscp_and_ecn: _,
                } = meta.try_as::<I>().unwrap();
                assert_eq!(destination, &IpPacketDestination::Neighbor(remote_ip));
                assert_eq!(src_ip, &local_ip);
                assert_eq!(dst_ip, &remote_ip);
                assert_eq!(proto, &IpProto::Udp.into());
                let mut buf = &frame_body[..];
                let udp_packet =
                    UdpPacket::parse(&mut buf, UdpParseArgs::new(src_ip.get(), dst_ip.get()))
                        .expect("Parsed sent UDP packet");
                assert_eq!(udp_packet.src_port().unwrap(), LOCAL_PORT);
                assert_eq!(udp_packet.dst_port(), REMOTE_PORT);
                assert_eq!(udp_packet.body(), &body[..]);
            };
        check_frame(&frames[0]);
        check_frame(&frames[1]);
    }

    #[ip_test(I)]
    fn test_receive_udp_queue_full<I: TestIpExt>() {
        set_logger_for_test();
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let local_ip = local_ip::<I>();
        let remote_ip = remote_ip::<I>();
        let socket = api.create();

        // Create a listener on the local port, bound to the local IP:
        api.listen(&socket, Some(ZonedAddr::Unzoned(local_ip)), Some(LOCAL_PORT))
            .expect("listen_udp failed");

        let (core_ctx, bindings_ctx) = api.contexts();
        // Simulate a full RX queue.
        {
            let received =
                bindings_ctx.state.received_mut::<I>().entry(socket.downgrade()).or_default();
            received.max_size = 0;
        }

        // Inject a packet.
        let body = [1, 2, 3, 4, 5];
        let meta = UdpPacketMeta::<I> {
            src_ip: remote_ip.get(),
            src_port: Some(REMOTE_PORT),
            dst_ip: local_ip.get(),
            dst_port: LOCAL_PORT,
            dscp_and_ecn: DscpAndEcn::default(),
        };
        receive_udp_packet(core_ctx, bindings_ctx, FakeDeviceId, meta, &body[..], WithEarlyDemux)
            .expect("receive udp packet should succeed");

        assert_counters(
            api.core_ctx(),
            CounterExpectationsWithSocket {
                rx_delivered: 1,
                rx_queue_full: 1,
                ..Default::default()
            },
            CounterExpectationsWithoutSocket { rx: 1, ..Default::default() },
            [(
                &socket,
                CounterExpectationsWithSocket {
                    rx_delivered: 1,
                    rx_queue_full: 1,
                    ..Default::default()
                },
            )],
        )
    }

    /// Tests that UDP packets without a connection are dropped.
    ///
    /// Tests that receiving a UDP packet on a port over which there isn't a
    /// listener causes the packet to be dropped correctly.
    #[ip_test(I)]
    fn test_udp_drop<I: TestIpExt>() {
        set_logger_for_test();
        let UdpFakeDeviceCtx { mut core_ctx, mut bindings_ctx } =
            UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let local_ip = local_ip::<I>();
        let remote_ip = remote_ip::<I>();

        let meta = UdpPacketMeta::<I> {
            src_ip: remote_ip.get(),
            src_port: Some(REMOTE_PORT),
            dst_ip: local_ip.get(),
            dst_port: LOCAL_PORT,
            dscp_and_ecn: DscpAndEcn::default(),
        };
        let body = [1, 2, 3, 4, 5];
        assert_eq!(
            receive_udp_packet(
                &mut core_ctx,
                &mut bindings_ctx,
                FakeDeviceId,
                meta,
                &body[..],
                WithEarlyDemux,
            ),
            Err(I::IcmpError::port_unreachable())
        );
        assert_eq!(&bindings_ctx.state.socket_data::<I>(), &HashMap::new());
    }

    /// Tests that UDP connections can be created and data can be transmitted
    /// over it.
    ///
    /// Only tests with specified local port and address bounds.
    #[ip_test(I)]
    #[test_case(EarlyDemuxMode::Enabled; "with early demux")]
    #[test_case(EarlyDemuxMode::Disabled; "without early demux")]
    fn test_udp_conn_basic<I: TestIpExt>(early_demux_mode: EarlyDemuxMode) {
        set_logger_for_test();
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let local_ip = local_ip::<I>();
        let remote_ip = remote_ip::<I>();
        let socket = api.create();
        // Create a UDP connection with a specified local port and local IP.
        api.listen(&socket, Some(ZonedAddr::Unzoned(local_ip)), Some(LOCAL_PORT))
            .expect("listen_udp failed");
        api.connect(&socket, Some(ZonedAddr::Unzoned(remote_ip)), REMOTE_PORT.into())
            .expect("connect failed");

        // Inject a UDP packet and see if we receive it on the context.
        let meta = UdpPacketMeta::<I> {
            src_ip: remote_ip.get(),
            src_port: Some(REMOTE_PORT),
            dst_ip: local_ip.get(),
            dst_port: LOCAL_PORT,
            dscp_and_ecn: DscpAndEcn::default(),
        };
        let body = [1, 2, 3, 4, 5];
        let (core_ctx, bindings_ctx) = api.contexts();
        receive_udp_packet(core_ctx, bindings_ctx, FakeDeviceId, meta, &body[..], early_demux_mode)
            .expect("receive udp packet should succeed");

        assert_eq!(
            bindings_ctx.state.socket_data(),
            HashMap::from([(socket.downgrade(), vec![&body[..]])])
        );

        // Now try to send something over this new connection.
        api.send(&socket, Buf::new(body.to_vec(), ..), FakeSendToken::default())
            .expect("send_udp_conn returned an error");

        let (meta, frame_body) =
            assert_matches!(api.core_ctx().bound_sockets.ip_socket_ctx.frames(), [frame] => frame);
        // Check first frame.
        let SendIpPacketMeta {
            device: _,
            src_ip,
            dst_ip,
            destination,
            proto,
            ttl: _,
            mtu: _,
            dscp_and_ecn: _,
        } = meta.try_as::<I>().unwrap();
        assert_eq!(destination, &IpPacketDestination::Neighbor(remote_ip));
        assert_eq!(src_ip, &local_ip);
        assert_eq!(dst_ip, &remote_ip);
        assert_eq!(proto, &IpProto::Udp.into());
        let mut buf = &frame_body[..];
        let udp_packet = UdpPacket::parse(&mut buf, UdpParseArgs::new(src_ip.get(), dst_ip.get()))
            .expect("Parsed sent UDP packet");
        assert_eq!(udp_packet.src_port().unwrap(), LOCAL_PORT);
        assert_eq!(udp_packet.dst_port(), REMOTE_PORT);
        assert_eq!(udp_packet.body(), &body[..]);

        let expects_with_socket =
            || CounterExpectationsWithSocket { rx_delivered: 1, tx: 1, ..Default::default() };
        assert_counters(
            api.core_ctx(),
            expects_with_socket(),
            CounterExpectationsWithoutSocket { rx: 1, ..Default::default() },
            [(&socket, expects_with_socket())],
        )
    }

    /// Tests that UDP connections fail with an appropriate error for
    /// non-routable remote addresses.
    #[ip_test(I)]
    fn test_udp_conn_unroutable<I: TestIpExt>() {
        set_logger_for_test();
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        // Set fake context callback to treat all addresses as unroutable.
        let remote_ip = I::get_other_ip_address(127);
        // Create a UDP connection with a specified local port and local IP.
        let unbound = api.create();
        let conn_err = api
            .connect(&unbound, Some(ZonedAddr::Unzoned(remote_ip)), REMOTE_PORT.into())
            .unwrap_err();

        assert_eq!(conn_err, ConnectError::Ip(ResolveRouteError::Unreachable.into()));
    }

    /// Tests that UDP listener creation fails with an appropriate error when
    /// local address is non-local.
    #[ip_test(I)]
    fn test_udp_conn_cannot_bind<I: TestIpExt>() {
        set_logger_for_test();
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());

        // Use remote address to trigger IpSockCreationError::LocalAddrNotAssigned.
        let remote_ip = remote_ip::<I>();
        // Create a UDP listener with a specified local port and local ip:
        let unbound = api.create();
        let result = api.listen(&unbound, Some(ZonedAddr::Unzoned(remote_ip)), Some(LOCAL_PORT));

        assert_eq!(result, Err(Either::Right(LocalAddressError::CannotBindToAddress)));
    }

    #[test]
    fn test_udp_conn_picks_link_local_source_address() {
        set_logger_for_test();
        // When the remote address has global scope but the source address
        // is link-local, make sure that the socket implicitly has its bound
        // device set.
        set_logger_for_test();
        let local_ip = SpecifiedAddr::new(net_ip_v6!("fe80::1")).unwrap();
        let remote_ip = SpecifiedAddr::new(net_ip_v6!("1:2:3:4::")).unwrap();
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(
            UdpFakeDeviceCoreCtx::with_local_remote_ip_addrs(vec![local_ip], vec![remote_ip]),
        );
        let mut api = UdpApi::<Ipv6, _>::new(ctx.as_mut());
        let socket = api.create();
        api.connect(&socket, Some(ZonedAddr::Unzoned(remote_ip)), REMOTE_PORT.into())
            .expect("can connect");

        let info = api.get_info(&socket);
        let (conn_local_ip, conn_remote_ip) = assert_matches!(
            info,
            SocketInfo::Connected(datagram::ConnInfo {
                local_ip: conn_local_ip,
                remote_ip: conn_remote_ip,
                local_identifier: _,
                remote_identifier: _,
            }) => (conn_local_ip, conn_remote_ip)
        );
        assert_eq!(
            conn_local_ip,
            StrictlyZonedAddr::new_with_zone(local_ip, || FakeWeakDeviceId(FakeDeviceId)),
        );
        assert_eq!(conn_remote_ip, StrictlyZonedAddr::new_unzoned_or_panic(remote_ip));

        // Double-check that the bound device can't be changed after being set
        // implicitly.
        assert_eq!(
            api.set_device(&socket, None),
            Err(SocketError::Local(LocalAddressError::Zone(ZonedAddressError::DeviceZoneMismatch)))
        );
    }

    #[ip_test(I)]
    #[test_case(
        true,
        Err(IpSockCreationError::Route(ResolveRouteError::Unreachable).into()); "remove device")]
    #[test_case(false, Ok(()); "dont remove device")]
    fn test_udp_conn_device_removed<I: TestIpExt>(
        remove_device: bool,
        expected: Result<(), ConnectError>,
    ) {
        set_logger_for_test();
        let device = FakeReferencyDeviceId::default();
        let mut ctx =
            FakeUdpCtx::with_core_ctx(FakeUdpCoreCtx::new_with_device::<I>(device.clone()));
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());

        let unbound = api.create();
        api.set_device(&unbound, Some(&device)).unwrap();

        if remove_device {
            device.mark_removed();
        }

        let remote_ip = remote_ip::<I>();
        assert_eq!(
            api.connect(&unbound, Some(ZonedAddr::Unzoned(remote_ip)), REMOTE_PORT.into()),
            expected,
        );
    }

    /// Tests that UDP connections fail with an appropriate error when local
    /// ports are exhausted.
    #[ip_test(I)]
    fn test_udp_conn_exhausted<I: TestIpExt>() {
        // NB: We don't enable logging for this test because it's very spammy.
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());

        let local_ip = local_ip::<I>();
        // Exhaust local ports to trigger FailedToAllocateLocalPort error.
        for port_num in FakePortAlloc::<I>::EPHEMERAL_RANGE {
            let socket = api.create();
            api.listen(&socket, Some(ZonedAddr::Unzoned(local_ip)), NonZeroU16::new(port_num))
                .unwrap();
        }

        let remote_ip = remote_ip::<I>();
        let unbound = api.create();
        let conn_err = api
            .connect(&unbound, Some(ZonedAddr::Unzoned(remote_ip)), REMOTE_PORT.into())
            .unwrap_err();

        assert_eq!(conn_err, ConnectError::CouldNotAllocateLocalPort);
    }

    #[ip_test(I)]
    fn test_connect_success<I: TestIpExt>() {
        set_logger_for_test();
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());

        let local_ip = local_ip::<I>();
        let remote_ip = remote_ip::<I>();
        let multicast_addr = I::get_multicast_addr(3);
        let socket = api.create();
        let sharing_domain = SharingDomain::new(1);

        // Set some properties on the socket that should be preserved.
        api.set_posix_reuse_port(&socket, ReusePortOption::Enabled(sharing_domain))
            .expect("is unbound");
        api.set_multicast_membership(
            &socket,
            multicast_addr,
            MulticastInterfaceSelector::LocalAddress(local_ip).into(),
            true,
        )
        .expect("join multicast group should succeed");

        api.listen(&socket, Some(ZonedAddr::Unzoned(local_ip)), Some(LOCAL_PORT))
            .expect("Initial call to listen_udp was expected to succeed");

        api.connect(&socket, Some(ZonedAddr::Unzoned(remote_ip)), REMOTE_PORT.into())
            .expect("connect should succeed");

        // Check that socket options set on the listener are propagated to the
        // connected socket.
        assert!(api.get_posix_reuse_port(&socket));
        assert_eq!(
            api.core_ctx().bound_sockets.ip_socket_ctx.state.multicast_memberships::<I>(),
            HashMap::from([((FakeDeviceId, multicast_addr), NonZeroUsize::new(1).unwrap())])
        );
        assert_eq!(
            api.set_multicast_membership(
                &socket,
                multicast_addr,
                MulticastInterfaceSelector::LocalAddress(local_ip).into(),
                true
            ),
            Err(SetMulticastMembershipError::GroupAlreadyJoined)
        );
    }

    #[ip_test(I)]
    fn test_connect_fails<I: TestIpExt>() {
        set_logger_for_test();
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let local_ip = local_ip::<I>();
        let remote_ip = I::get_other_ip_address(127);
        let multicast_addr = I::get_multicast_addr(3);
        let socket = api.create();

        // Set some properties on the socket that should be preserved.
        let sharing_domain = SharingDomain::new(1);
        api.set_posix_reuse_port(&socket, ReusePortOption::Enabled(sharing_domain))
            .expect("is unbound");
        api.set_multicast_membership(
            &socket,
            multicast_addr,
            MulticastInterfaceSelector::LocalAddress(local_ip).into(),
            true,
        )
        .expect("join multicast group should succeed");

        // Create a UDP connection with a specified local port and local IP.
        api.listen(&socket, Some(ZonedAddr::Unzoned(local_ip)), Some(LOCAL_PORT))
            .expect("Initial call to listen_udp was expected to succeed");

        assert_matches!(
            api.connect(&socket, Some(ZonedAddr::Unzoned(remote_ip)), REMOTE_PORT.into()),
            Err(ConnectError::Ip(IpSockCreationError::Route(ResolveRouteError::Unreachable)))
        );

        // Check that the listener was unchanged by the failed connection.
        assert!(api.get_posix_reuse_port(&socket));
        assert_eq!(
            api.core_ctx().bound_sockets.ip_socket_ctx.state.multicast_memberships::<I>(),
            HashMap::from([((FakeDeviceId, multicast_addr), NonZeroUsize::new(1).unwrap())])
        );
        assert_eq!(
            api.set_multicast_membership(
                &socket,
                multicast_addr,
                MulticastInterfaceSelector::LocalAddress(local_ip).into(),
                true
            ),
            Err(SetMulticastMembershipError::GroupAlreadyJoined)
        );
    }

    #[ip_test(I)]
    fn test_reconnect_udp_conn_success<I: TestIpExt>() {
        set_logger_for_test();

        let local_ip = local_ip::<I>();
        let remote_ip = remote_ip::<I>();
        let other_remote_ip = I::get_other_ip_address(3);

        let mut ctx =
            UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::with_local_remote_ip_addrs(
                vec![local_ip],
                vec![remote_ip, other_remote_ip],
            ));
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());

        let socket = api.create();
        api.listen(&socket, Some(ZonedAddr::Unzoned(local_ip)), Some(LOCAL_PORT))
            .expect("listen should succeed");

        api.connect(&socket, Some(ZonedAddr::Unzoned(remote_ip)), REMOTE_PORT.into())
            .expect("connect was expected to succeed");

        api.connect(&socket, Some(ZonedAddr::Unzoned(other_remote_ip)), OTHER_REMOTE_PORT.into())
            .expect("connect should succeed");
        assert_eq!(
            api.get_info(&socket),
            SocketInfo::Connected(datagram::ConnInfo {
                local_ip: StrictlyZonedAddr::new_unzoned_or_panic(local_ip),
                local_identifier: LOCAL_PORT,
                remote_ip: StrictlyZonedAddr::new_unzoned_or_panic(other_remote_ip),
                remote_identifier: OTHER_REMOTE_PORT.into(),
            })
        );
    }

    #[ip_test(I)]
    fn test_reconnect_udp_conn_fails<I: TestIpExt>() {
        set_logger_for_test();
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let local_ip = local_ip::<I>();
        let remote_ip = remote_ip::<I>();
        let other_remote_ip = I::get_other_ip_address(3);

        let socket = api.create();
        api.listen(&socket, Some(ZonedAddr::Unzoned(local_ip)), Some(LOCAL_PORT))
            .expect("listen should succeed");

        api.connect(&socket, Some(ZonedAddr::Unzoned(remote_ip)), REMOTE_PORT.into())
            .expect("connect was expected to succeed");
        let error = api
            .connect(&socket, Some(ZonedAddr::Unzoned(other_remote_ip)), OTHER_REMOTE_PORT.into())
            .expect_err("connect should fail");
        assert_matches!(
            error,
            ConnectError::Ip(IpSockCreationError::Route(ResolveRouteError::Unreachable))
        );

        assert_eq!(
            api.get_info(&socket),
            SocketInfo::Connected(datagram::ConnInfo {
                local_ip: StrictlyZonedAddr::new_unzoned_or_panic(local_ip),
                local_identifier: LOCAL_PORT,
                remote_ip: StrictlyZonedAddr::new_unzoned_or_panic(remote_ip),
                remote_identifier: REMOTE_PORT.into()
            })
        );
    }

    #[ip_test(I)]
    fn test_send_to<I: TestIpExt>() {
        set_logger_for_test();

        let local_ip = local_ip::<I>();
        let remote_ip = remote_ip::<I>();
        let other_remote_ip = I::get_other_ip_address(3);

        let mut ctx =
            UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::with_local_remote_ip_addrs(
                vec![local_ip],
                vec![remote_ip, other_remote_ip],
            ));
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());

        let socket = api.create();
        api.listen(&socket, Some(ZonedAddr::Unzoned(local_ip)), Some(LOCAL_PORT))
            .expect("listen should succeed");
        api.connect(&socket, Some(ZonedAddr::Unzoned(remote_ip)), REMOTE_PORT.into())
            .expect("connect should succeed");

        let body = [1, 2, 3, 4, 5];
        // Try to send something with send_to
        api.send_to(
            &socket,
            Some(ZonedAddr::Unzoned(other_remote_ip)),
            REMOTE_PORT.into(),
            Buf::new(body.to_vec(), ..),
            FakeSendToken::default(),
        )
        .expect("send_to failed");

        // The socket should not have been affected.
        let info = api.get_info(&socket);
        let info = assert_matches!(info, SocketInfo::Connected(info) => info);
        assert_eq!(info.local_ip.into_inner(), ZonedAddr::Unzoned(local_ip));
        assert_eq!(info.remote_ip.into_inner(), ZonedAddr::Unzoned(remote_ip));
        assert_eq!(info.remote_identifier, u16::from(REMOTE_PORT));

        // Check first frame.
        let (meta, frame_body) =
            assert_matches!(api.core_ctx().bound_sockets.ip_socket_ctx.frames(), [frame] => frame);
        let SendIpPacketMeta {
            device: _,
            src_ip,
            dst_ip,
            destination,
            proto,
            ttl: _,
            mtu: _,
            dscp_and_ecn: _,
        } = meta.try_as::<I>().unwrap();

        assert_eq!(destination, &IpPacketDestination::Neighbor(other_remote_ip));
        assert_eq!(src_ip, &local_ip);
        assert_eq!(dst_ip, &other_remote_ip);
        assert_eq!(proto, &I::Proto::from(IpProto::Udp));
        let mut buf = &frame_body[..];
        let udp_packet = UdpPacket::parse(&mut buf, UdpParseArgs::new(src_ip.get(), dst_ip.get()))
            .expect("Parsed sent UDP packet");
        assert_eq!(udp_packet.src_port().unwrap(), LOCAL_PORT);
        assert_eq!(udp_packet.dst_port(), REMOTE_PORT);
        assert_eq!(udp_packet.body(), &body[..]);
    }

    /// Tests that UDP send failures are propagated as errors.
    ///
    /// Only tests with specified local port and address bounds.
    #[ip_test(I)]
    fn test_send_udp_conn_failure<I: TestIpExt>() {
        set_logger_for_test();
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let remote_ip = remote_ip::<I>();
        // Create a UDP connection with a specified local port and local IP.
        let socket = api.create();
        api.connect(&socket, Some(ZonedAddr::Unzoned(remote_ip)), REMOTE_PORT.into())
            .expect("connect failed");

        // Instruct the fake frame context to throw errors.
        api.core_ctx().bound_sockets.ip_socket_ctx.frames.set_should_error_for_frame(
            |_frame_meta| Some(SendFrameErrorReason::SizeConstraintsViolation),
        );

        // Now try to send something over this new connection:
        let send_err =
            api.send(&socket, Buf::new(Vec::new(), ..), FakeSendToken::default()).unwrap_err();
        assert_eq!(send_err, Either::Left(SendError::IpSock(IpSockSendError::Mtu)));

        let expects_with_socket =
            || CounterExpectationsWithSocket { tx: 1, tx_error: 1, ..Default::default() };
        assert_counters(
            api.core_ctx(),
            expects_with_socket(),
            Default::default(),
            [(&socket, expects_with_socket())],
        )
    }

    #[ip_test(I)]
    fn test_send_udp_conn_device_removed<I: TestIpExt>() {
        set_logger_for_test();
        let device = FakeReferencyDeviceId::default();
        let mut ctx =
            FakeUdpCtx::with_core_ctx(FakeUdpCoreCtx::new_with_device::<I>(device.clone()));
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let remote_ip = remote_ip::<I>();
        let socket = api.create();
        api.set_device(&socket, Some(&device)).unwrap();
        api.connect(&socket, Some(ZonedAddr::Unzoned(remote_ip)), REMOTE_PORT.into())
            .expect("connect failed");

        for (device_removed, expected_res) in [
            (false, Ok(())),
            (
                true,
                Err(Either::Left(SendError::IpSock(IpSockSendError::Unroutable(
                    ResolveRouteError::Unreachable,
                )))),
            ),
        ] {
            if device_removed {
                device.mark_removed();
            }

            assert_eq!(
                api.send(&socket, Buf::new(Vec::new(), ..), FakeSendToken::default()),
                expected_res
            )
        }
    }

    #[ip_test(I)]
    #[test_case(false, ShutdownType::Send; "shutdown send then send")]
    #[test_case(false, ShutdownType::SendAndReceive; "shutdown both then send")]
    #[test_case(true, ShutdownType::Send; "shutdown send then sendto")]
    #[test_case(true, ShutdownType::SendAndReceive; "shutdown both then sendto")]
    fn test_send_udp_after_shutdown<I: TestIpExt>(send_to: bool, shutdown: ShutdownType) {
        set_logger_for_test();

        #[derive(Debug)]
        struct NotWriteableError;

        let send = |remote_ip, api: &mut UdpApi<_, _>, id| -> Result<(), NotWriteableError> {
            match remote_ip {
                Some(remote_ip) => api.send_to(
                    id,
                    Some(remote_ip),
                    REMOTE_PORT.into(),
                    Buf::new(Vec::new(), ..),
                    FakeSendToken::default(),
                )
                .map_err(
                    |e| assert_matches!(e, SendToError::NotWriteable => NotWriteableError)
                ),
                None => api.send(
                    id,
                    Buf::new(Vec::new(), ..),
                    FakeSendToken::default(),
                )
                .map_err(|e| assert_matches!(e, Either::Left(SendError::NotWriteable) => NotWriteableError)),
            }
        };

        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());

        let remote_ip = ZonedAddr::Unzoned(remote_ip::<I>());
        let send_to_ip = send_to.then_some(remote_ip);

        let socket = api.create();
        api.connect(&socket, Some(remote_ip), REMOTE_PORT.into()).expect("connect failed");

        send(send_to_ip, &mut api, &socket).expect("can send");
        api.shutdown(&socket, shutdown).expect("is connected");

        assert_matches!(send(send_to_ip, &mut api, &socket), Err(NotWriteableError));
    }

    #[ip_test(I, test = false)]
    #[test_case::test_matrix(
        [ShutdownType::Receive, ShutdownType::SendAndReceive],
        [EarlyDemuxMode::Enabled, EarlyDemuxMode::Disabled]
    )]
    fn test_marked_for_receive_shutdown<I: TestIpExt>(
        which: ShutdownType,
        early_demux_mode: EarlyDemuxMode,
    ) {
        set_logger_for_test();

        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());

        let socket = api.create();
        api.listen(&socket, Some(ZonedAddr::Unzoned(local_ip::<I>())), Some(LOCAL_PORT))
            .expect("can bind");
        api.connect(&socket, Some(ZonedAddr::Unzoned(remote_ip::<I>())), REMOTE_PORT.into())
            .expect("can connect");

        // Receive once, then set the shutdown flag, then receive again and
        // check that it doesn't get to the socket.

        let meta = UdpPacketMeta::<I> {
            src_ip: remote_ip::<I>().get(),
            src_port: Some(REMOTE_PORT),
            dst_ip: local_ip::<I>().get(),
            dst_port: LOCAL_PORT,
            dscp_and_ecn: DscpAndEcn::default(),
        };
        let packet = [1, 1, 1, 1];
        let (core_ctx, bindings_ctx) = api.contexts();

        receive_udp_packet(
            core_ctx,
            bindings_ctx,
            FakeDeviceId,
            meta.clone(),
            &packet[..],
            early_demux_mode,
        )
        .expect("receive udp packet should succeed");

        assert_eq!(
            bindings_ctx.state.socket_data(),
            HashMap::from([(socket.downgrade(), vec![&packet[..]])])
        );
        api.shutdown(&socket, which).expect("is connected");
        let (core_ctx, bindings_ctx) = api.contexts();
        assert_eq!(
            receive_udp_packet(
                core_ctx,
                bindings_ctx,
                FakeDeviceId,
                meta.clone(),
                &packet[..],
                early_demux_mode
            ),
            Err(I::IcmpError::port_unreachable())
        );
        assert_eq!(
            bindings_ctx.state.socket_data(),
            HashMap::from([(socket.downgrade(), vec![&packet[..]])])
        );

        // Calling shutdown for the send direction doesn't change anything.
        api.shutdown(&socket, ShutdownType::Send).expect("is connected");
        let (core_ctx, bindings_ctx) = api.contexts();
        assert_eq!(
            receive_udp_packet(
                core_ctx,
                bindings_ctx,
                FakeDeviceId,
                meta,
                &packet[..],
                early_demux_mode
            ),
            Err(I::IcmpError::port_unreachable())
        );
        assert_eq!(
            bindings_ctx.state.socket_data(),
            HashMap::from([(socket.downgrade(), vec![&packet[..]])])
        );
    }

    /// Tests that if we have multiple listeners and connections, demuxing the
    /// flows is performed correctly.
    #[ip_test(I)]
    #[test_case(WithEarlyDemux; "with early demux")]
    #[test_case(NoEarlyDemux; "without early demux")]
    fn test_udp_demux<I: TestIpExt>(early_demux_mode: EarlyDemuxMode) {
        set_logger_for_test();
        let local_ip = local_ip::<I>();
        let remote_ip_a = I::get_other_ip_address(70);
        let remote_ip_b = I::get_other_ip_address(72);
        let local_port_a = NonZeroU16::new(100).unwrap();
        let local_port_b = NonZeroU16::new(101).unwrap();
        let local_port_c = NonZeroU16::new(102).unwrap();
        let local_port_d = NonZeroU16::new(103).unwrap();

        let mut ctx =
            UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::with_local_remote_ip_addrs(
                vec![local_ip],
                vec![remote_ip_a, remote_ip_b],
            ));
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());

        let sharing_domain = SharingDomain::new(1);

        // Create some UDP connections and listeners:
        // conn2 has just a remote addr different than conn1, which requires
        // allowing them to share the local port.
        let [conn1, conn2] = [remote_ip_a, remote_ip_b].map(|remote_ip| {
            let socket = api.create();
            api.set_posix_reuse_port(&socket, ReusePortOption::Enabled(sharing_domain))
                .expect("is unbound");
            api.listen(&socket, Some(ZonedAddr::Unzoned(local_ip)), Some(local_port_d))
                .expect("listen_udp failed");
            api.connect(&socket, Some(ZonedAddr::Unzoned(remote_ip)), REMOTE_PORT.into())
                .expect("connect failed");
            socket
        });
        let list1 = api.create();
        api.listen(&list1, Some(ZonedAddr::Unzoned(local_ip)), Some(local_port_a))
            .expect("listen_udp failed");
        let list2 = api.create();
        api.listen(&list2, Some(ZonedAddr::Unzoned(local_ip)), Some(local_port_b))
            .expect("listen_udp failed");
        let wildcard_list = api.create();
        api.listen(&wildcard_list, None, Some(local_port_c)).expect("listen_udp failed");

        let mut expectations = HashMap::<WeakUdpSocketId<I, _, _>, SocketReceived<I>>::new();
        // Now inject UDP packets that each of the created connections should
        // receive.
        let meta = UdpPacketMeta {
            src_ip: remote_ip_a.get(),
            src_port: Some(REMOTE_PORT),
            dst_ip: local_ip.get(),
            dst_port: local_port_d,
            dscp_and_ecn: DscpAndEcn::default(),
        };
        let body_conn1 = [1, 1, 1, 1];
        let (core_ctx, bindings_ctx) = api.contexts();
        receive_udp_packet(
            core_ctx,
            bindings_ctx,
            FakeDeviceId,
            meta.clone(),
            &body_conn1[..],
            early_demux_mode,
        )
        .expect("receive udp packet should succeed");
        expectations
            .entry(conn1.downgrade())
            .or_default()
            .packets
            .push(ReceivedPacket { meta: meta, body: body_conn1.into() });
        assert_eq!(bindings_ctx.state.received(), &expectations);

        let meta = UdpPacketMeta {
            src_ip: remote_ip_b.get(),
            src_port: Some(REMOTE_PORT),
            dst_ip: local_ip.get(),
            dst_port: local_port_d,
            dscp_and_ecn: DscpAndEcn::default(),
        };
        let body_conn2 = [2, 2, 2, 2];
        receive_udp_packet(
            core_ctx,
            bindings_ctx,
            FakeDeviceId,
            meta.clone(),
            &body_conn2[..],
            early_demux_mode,
        )
        .expect("receive udp packet should succeed");
        expectations
            .entry(conn2.downgrade())
            .or_default()
            .packets
            .push(ReceivedPacket { meta: meta, body: body_conn2.into() });
        assert_eq!(bindings_ctx.state.received(), &expectations);

        let meta = UdpPacketMeta {
            src_ip: remote_ip_a.get(),
            src_port: Some(REMOTE_PORT),
            dst_ip: local_ip.get(),
            dst_port: local_port_a,
            dscp_and_ecn: DscpAndEcn::default(),
        };
        let body_list1 = [3, 3, 3, 3];
        receive_udp_packet(
            core_ctx,
            bindings_ctx,
            FakeDeviceId,
            meta.clone(),
            &body_list1[..],
            early_demux_mode,
        )
        .expect("receive udp packet should succeed");
        expectations
            .entry(list1.downgrade())
            .or_default()
            .packets
            .push(ReceivedPacket { meta: meta, body: body_list1.into() });
        assert_eq!(bindings_ctx.state.received(), &expectations);

        let meta = UdpPacketMeta {
            src_ip: remote_ip_a.get(),
            src_port: Some(REMOTE_PORT),
            dst_ip: local_ip.get(),
            dst_port: local_port_b,
            dscp_and_ecn: DscpAndEcn::default(),
        };
        let body_list2 = [4, 4, 4, 4];
        receive_udp_packet(
            core_ctx,
            bindings_ctx,
            FakeDeviceId,
            meta.clone(),
            &body_list2[..],
            early_demux_mode,
        )
        .expect("receive udp packet should succeed");
        expectations
            .entry(list2.downgrade())
            .or_default()
            .packets
            .push(ReceivedPacket { meta: meta, body: body_list2.into() });
        assert_eq!(bindings_ctx.state.received(), &expectations);

        let meta = UdpPacketMeta {
            src_ip: remote_ip_a.get(),
            src_port: Some(REMOTE_PORT),
            dst_ip: local_ip.get(),
            dst_port: local_port_c,
            dscp_and_ecn: DscpAndEcn::default(),
        };
        let body_wildcard_list = [5, 5, 5, 5];
        receive_udp_packet(
            core_ctx,
            bindings_ctx,
            FakeDeviceId,
            meta.clone(),
            &body_wildcard_list[..],
            early_demux_mode,
        )
        .expect("receive udp packet should succeed");
        expectations
            .entry(wildcard_list.downgrade())
            .or_default()
            .packets
            .push(ReceivedPacket { meta: meta, body: body_wildcard_list.into() });
        assert_eq!(bindings_ctx.state.received(), &expectations);
    }

    /// Tests UDP wildcard listeners for different IP versions.
    #[ip_test(I)]
    #[test_case(WithEarlyDemux; "with early demux")]
    #[test_case(NoEarlyDemux; "without early demux")]
    fn test_wildcard_listeners<I: TestIpExt>(early_demux_mode: EarlyDemuxMode) {
        set_logger_for_test();
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let local_ip_a = I::get_other_ip_address(1);
        let local_ip_b = I::get_other_ip_address(2);
        let remote_ip_a = I::get_other_ip_address(70);
        let remote_ip_b = I::get_other_ip_address(72);
        let listener = api.create();
        api.listen(&listener, None, Some(LOCAL_PORT)).expect("listen_udp failed");

        let body = [1, 2, 3, 4, 5];
        let (core_ctx, bindings_ctx) = api.contexts();
        let meta_1 = UdpPacketMeta {
            src_ip: remote_ip_a.get(),
            src_port: Some(REMOTE_PORT),
            dst_ip: local_ip_a.get(),
            dst_port: LOCAL_PORT,
            dscp_and_ecn: DscpAndEcn::default(),
        };
        receive_udp_packet(
            core_ctx,
            bindings_ctx,
            FakeDeviceId,
            meta_1.clone(),
            &body[..],
            early_demux_mode,
        )
        .expect("receive udp packet should succeed");

        // Receive into a different local IP.
        let meta_2 = UdpPacketMeta {
            src_ip: remote_ip_b.get(),
            src_port: Some(REMOTE_PORT),
            dst_ip: local_ip_b.get(),
            dst_port: LOCAL_PORT,
            dscp_and_ecn: DscpAndEcn::default(),
        };
        receive_udp_packet(
            core_ctx,
            bindings_ctx,
            FakeDeviceId,
            meta_2.clone(),
            &body[..],
            early_demux_mode,
        )
        .expect("receive udp packet should succeed");

        // Check that we received both packets for the listener.
        assert_eq!(
            bindings_ctx.state.received::<I>(),
            &HashMap::from([(
                listener.downgrade(),
                SocketReceived {
                    packets: vec![
                        ReceivedPacket { meta: meta_1, body: body.into() },
                        ReceivedPacket { meta: meta_2, body: body.into() }
                    ],
                    max_size: usize::MAX,
                }
            )])
        );
    }

    #[ip_test(I)]
    #[test_case(WithEarlyDemux; "with early demux")]
    #[test_case(NoEarlyDemux; "without early demux")]
    fn test_receive_source_port_zero_on_listener<I: TestIpExt>(early_demux_mode: EarlyDemuxMode) {
        set_logger_for_test();
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let listener = api.create();
        api.listen(&listener, None, Some(LOCAL_PORT)).expect("listen_udp failed");

        let body = [];
        let meta = UdpPacketMeta::<I> {
            src_ip: I::TEST_ADDRS.remote_ip.get(),
            src_port: None,
            dst_ip: I::TEST_ADDRS.local_ip.get(),
            dst_port: LOCAL_PORT,
            dscp_and_ecn: DscpAndEcn::default(),
        };

        let (core_ctx, bindings_ctx) = api.contexts();
        receive_udp_packet(
            core_ctx,
            bindings_ctx,
            FakeDeviceId,
            meta.clone(),
            &body[..],
            early_demux_mode,
        )
        .expect("receive udp packet should succeed");
        // Check that we received both packets for the listener.
        assert_eq!(
            bindings_ctx.state.received(),
            &HashMap::from([(
                listener.downgrade(),
                SocketReceived {
                    packets: vec![ReceivedPacket { meta, body: vec![] }],
                    max_size: usize::MAX
                }
            )])
        );
    }

    #[ip_test(I)]
    #[test_case(WithEarlyDemux; "with early demux")]
    #[test_case(NoEarlyDemux; "without early demux")]
    fn test_receive_source_addr_unspecified_on_listener<I: TestIpExt>(
        early_demux_mode: EarlyDemuxMode,
    ) {
        set_logger_for_test();
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let listener = api.create();
        api.listen(&listener, None, Some(LOCAL_PORT)).expect("listen_udp failed");

        let meta = UdpPacketMeta::<I> {
            src_ip: I::UNSPECIFIED_ADDRESS,
            src_port: Some(REMOTE_PORT),
            dst_ip: I::TEST_ADDRS.local_ip.get(),
            dst_port: LOCAL_PORT,
            dscp_and_ecn: DscpAndEcn::default(),
        };
        let body = [];
        let (core_ctx, bindings_ctx) = api.contexts();
        receive_udp_packet(core_ctx, bindings_ctx, FakeDeviceId, meta, &body[..], early_demux_mode)
            .expect("receive udp packet should succeed");
        // Check that we received the packet on the listener.
        assert_eq!(
            bindings_ctx.state.socket_data(),
            HashMap::from([(listener.downgrade(), vec![&body[..]])])
        );
    }

    #[ip_test(I)]
    #[test_case(NonZeroU16::new(u16::MAX).unwrap(), Ok(NonZeroU16::new(u16::MAX).unwrap()); "ephemeral available")]
    #[test_case(NonZeroU16::new(100).unwrap(), Err(LocalAddressError::FailedToAllocateLocalPort);
        "no ephemeral available")]
    fn test_bind_picked_port_all_others_taken<I: TestIpExt>(
        available_port: NonZeroU16,
        expected_result: Result<NonZeroU16, LocalAddressError>,
    ) {
        // NB: We don't enable logging for this test because it's very spammy.
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());

        for port in 1..=u16::MAX {
            let port = NonZeroU16::new(port).unwrap();
            if port == available_port {
                continue;
            }
            let unbound = api.create();
            api.listen(&unbound, None, Some(port)).expect("uncontested bind");
        }

        // Now that all but the LOCAL_PORT are occupied, ask the stack to
        // select a port.
        let socket = api.create();
        let result = api
            .listen(&socket, None, None)
            .map(|()| {
                let info = api.get_info(&socket);
                assert_matches!(info, SocketInfo::Listener(info) => info.local_identifier)
            })
            .map_err(Either::unwrap_right);
        assert_eq!(result, expected_result);
    }

    #[ip_test(I)]
    #[test_case(WithEarlyDemux; "with early demux")]
    #[test_case(NoEarlyDemux; "without early demux")]
    fn test_receive_multicast_packet<I: TestIpExt>(early_demux_mode: EarlyDemuxMode) {
        set_logger_for_test();
        let local_ip = local_ip::<I>();
        let remote_ip = I::get_other_ip_address(70);
        let multicast_addr = I::get_multicast_addr(0);
        let multicast_addr_other = I::get_multicast_addr(1);

        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(
            UdpFakeDeviceCoreCtx::with_local_remote_ip_addrs(vec![local_ip], vec![remote_ip]),
        );
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());

        let sharing_domain = SharingDomain::new(1);

        // Create 3 sockets: one listener for all IPs, two listeners on the same
        // local address.
        let any_listener = {
            let socket = api.create();
            api.set_posix_reuse_port(&socket, ReusePortOption::Enabled(sharing_domain))
                .expect("is unbound");
            api.listen(&socket, None, Some(LOCAL_PORT)).expect("listen_udp failed");
            socket
        };

        let specific_listeners = [(); 2].map(|()| {
            let socket = api.create();
            api.set_posix_reuse_port(&socket, ReusePortOption::Enabled(sharing_domain))
                .expect("is unbound");
            api.listen(
                &socket,
                Some(ZonedAddr::Unzoned(multicast_addr.into_specified())),
                Some(LOCAL_PORT),
            )
            .expect("listen_udp failed");
            socket
        });

        let (core_ctx, bindings_ctx) = api.contexts();
        let mut receive_packet = |body, local_ip: MulticastAddr<I::Addr>| {
            let meta = UdpPacketMeta::<I> {
                src_ip: remote_ip.get(),
                src_port: Some(REMOTE_PORT),
                dst_ip: local_ip.get(),
                dst_port: LOCAL_PORT,
                dscp_and_ecn: DscpAndEcn::default(),
            };
            let body = [body];
            receive_udp_packet(core_ctx, bindings_ctx, FakeDeviceId, meta, &body, early_demux_mode)
                .expect("receive udp packet should succeed")
        };

        // These packets should be received by all listeners.
        receive_packet(1, multicast_addr);
        receive_packet(2, multicast_addr);

        // This packet should be received only by the all-IPs listener.
        receive_packet(3, multicast_addr_other);

        assert_eq!(
            bindings_ctx.state.socket_data(),
            HashMap::from([
                (specific_listeners[0].downgrade(), vec![[1].as_slice(), &[2]]),
                (specific_listeners[1].downgrade(), vec![&[1], &[2]]),
                (any_listener.downgrade(), vec![&[1], &[2], &[3]]),
            ]),
        );

        assert_counters(
            api.core_ctx(),
            CounterExpectationsWithSocket { rx_delivered: 7, ..Default::default() },
            CounterExpectationsWithoutSocket { rx: 3, ..Default::default() },
            [
                (
                    &any_listener,
                    CounterExpectationsWithSocket { rx_delivered: 3, ..Default::default() },
                ),
                (
                    &specific_listeners[0],
                    CounterExpectationsWithSocket { rx_delivered: 2, ..Default::default() },
                ),
                (
                    &specific_listeners[1],
                    CounterExpectationsWithSocket { rx_delivered: 2, ..Default::default() },
                ),
            ],
        )
    }

    type UdpMultipleDevicesCtx = FakeUdpCtx<MultipleDevicesId>;
    type UdpMultipleDevicesCoreCtx = FakeUdpCoreCtx<MultipleDevicesId>;
    type UdpMultipleDevicesBindingsCtx = FakeUdpBindingsCtx<MultipleDevicesId>;

    impl FakeUdpCoreCtx<MultipleDevicesId> {
        fn new_multiple_devices<I: TestIpExt>() -> Self {
            let remote_ips = vec![I::get_other_remote_ip_address(1)];
            Self::with_ip_socket_ctx_state(FakeDualStackIpSocketCtx::new(
                MultipleDevicesId::all().into_iter().enumerate().map(|(i, device)| {
                    FakeDeviceConfig {
                        device,
                        local_ips: vec![Self::local_ip(i)],
                        remote_ips: remote_ips.clone(),
                    }
                }),
            ))
        }

        fn local_ip<A: IpAddress>(index: usize) -> SpecifiedAddr<A>
        where
            A::Version: TestIpExt,
        {
            A::Version::get_other_ip_address((index + 1).try_into().unwrap())
        }
    }

    /// Tests that if sockets are bound to devices, they will only receive
    /// packets that are received on those devices.
    #[ip_test(I)]
    #[test_case(WithEarlyDemux; "with early demux")]
    #[test_case(NoEarlyDemux; "without early demux")]
    fn test_bound_to_device_receive<I: TestIpExt>(early_demux_mode: EarlyDemuxMode) {
        set_logger_for_test();
        let mut ctx = UdpMultipleDevicesCtx::with_core_ctx(
            UdpMultipleDevicesCoreCtx::new_multiple_devices::<I>(),
        );
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let bound_first_device = api.create();
        api.listen(
            &bound_first_device,
            Some(ZonedAddr::Unzoned(local_ip::<I>())),
            Some(LOCAL_PORT),
        )
        .expect("listen should succeed");
        api.connect(
            &bound_first_device,
            Some(ZonedAddr::Unzoned(I::get_other_remote_ip_address(1))),
            REMOTE_PORT.into(),
        )
        .expect("connect should succeed");
        api.set_device(&bound_first_device, Some(&MultipleDevicesId::A))
            .expect("bind should succeed");

        let bound_second_device = api.create();
        api.set_device(&bound_second_device, Some(&MultipleDevicesId::B)).unwrap();
        api.listen(&bound_second_device, None, Some(LOCAL_PORT)).expect("listen should succeed");

        // Inject a packet received on `MultipleDevicesId::A` from the specified
        // remote; this should go to the first socket.
        let meta = UdpPacketMeta::<I> {
            src_ip: I::get_other_remote_ip_address(1).get(),
            src_port: Some(REMOTE_PORT),
            dst_ip: local_ip::<I>().get(),
            dst_port: LOCAL_PORT,
            dscp_and_ecn: DscpAndEcn::default(),
        };
        let body = [1, 2, 3, 4, 5];
        let (core_ctx, bindings_ctx) = api.contexts();
        receive_udp_packet(
            core_ctx,
            bindings_ctx,
            MultipleDevicesId::A,
            meta.clone(),
            &body[..],
            early_demux_mode,
        )
        .expect("receive udp packet should succeed");

        // A second packet received on `MultipleDevicesId::B` will go to the
        // second socket.
        receive_udp_packet(
            core_ctx,
            bindings_ctx,
            MultipleDevicesId::B,
            meta,
            &body[..],
            early_demux_mode,
        )
        .expect("receive udp packet should succeed");
        assert_eq!(
            bindings_ctx.state.socket_data(),
            HashMap::from([
                (bound_first_device.downgrade(), vec![&body[..]]),
                (bound_second_device.downgrade(), vec![&body[..]])
            ])
        );
    }

    /// Tests that if sockets are bound to devices, they will send packets out
    /// of those devices.
    #[ip_test(I)]
    fn test_bound_to_device_send<I: TestIpExt>() {
        set_logger_for_test();
        let mut ctx = UdpMultipleDevicesCtx::with_core_ctx(
            UdpMultipleDevicesCoreCtx::new_multiple_devices::<I>(),
        );
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let bound_on_devices = MultipleDevicesId::all().map(|device| {
            let socket = api.create();
            api.set_device(&socket, Some(&device)).unwrap();
            api.listen(&socket, None, Some(LOCAL_PORT)).expect("listen should succeed");
            socket
        });

        // Send a packet from each socket.
        let body = [1, 2, 3, 4, 5];
        for socket in bound_on_devices {
            api.send_to(
                &socket,
                Some(ZonedAddr::Unzoned(I::get_other_remote_ip_address(1))),
                REMOTE_PORT.into(),
                Buf::new(body.to_vec(), ..),
                FakeSendToken::default(),
            )
            .expect("send should succeed");
        }

        let mut received_devices = api
            .core_ctx()
            .bound_sockets
            .ip_socket_ctx
            .frames()
            .iter()
            .map(|(meta, _body)| {
                let SendIpPacketMeta {
                    device,
                    src_ip: _,
                    dst_ip,
                    destination: _,
                    proto,
                    ttl: _,
                    mtu: _,
                    dscp_and_ecn: _,
                } = meta.try_as::<I>().unwrap();
                assert_eq!(proto, &IpProto::Udp.into());
                assert_eq!(dst_ip, &I::get_other_remote_ip_address(1));
                *device
            })
            .collect::<Vec<_>>();
        received_devices.sort();
        assert_eq!(received_devices, &MultipleDevicesId::all());
    }

    fn receive_packet_on<I: TestIpExt>(
        core_ctx: &mut UdpMultipleDevicesCoreCtx,
        bindings_ctx: &mut UdpMultipleDevicesBindingsCtx,
        device: MultipleDevicesId,
        early_demux_mode: EarlyDemuxMode,
    ) -> Result<(), I::IcmpError> {
        let meta = UdpPacketMeta::<I> {
            src_ip: I::get_other_remote_ip_address(1).get(),
            src_port: Some(REMOTE_PORT),
            dst_ip: local_ip::<I>().get(),
            dst_port: LOCAL_PORT,
            dscp_and_ecn: DscpAndEcn::default(),
        };
        const BODY: [u8; 5] = [1, 2, 3, 4, 5];
        receive_udp_packet(core_ctx, bindings_ctx, device, meta, &BODY[..], early_demux_mode)
    }

    /// Check that sockets can be bound to and unbound from devices.
    #[ip_test(I)]
    #[test_case(WithEarlyDemux; "with early demux")]
    #[test_case(NoEarlyDemux; "without early demux")]
    fn test_bind_unbind_device<I: TestIpExt>(early_demux_mode: EarlyDemuxMode) {
        set_logger_for_test();
        let mut ctx = UdpMultipleDevicesCtx::with_core_ctx(
            UdpMultipleDevicesCoreCtx::new_multiple_devices::<I>(),
        );
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());

        // Start with `socket` bound to a device.
        let socket = api.create();
        api.set_device(&socket, Some(&MultipleDevicesId::A)).unwrap();
        api.listen(&socket, None, Some(LOCAL_PORT)).expect("listen failed");

        // Since it is bound, it does not receive a packet from another device.
        let (core_ctx, bindings_ctx) = api.contexts();
        assert_eq!(
            receive_packet_on::<I>(core_ctx, bindings_ctx, MultipleDevicesId::B, early_demux_mode),
            Err(I::IcmpError::port_unreachable())
        );
        let received = &bindings_ctx.state.socket_data::<I>();
        assert_eq!(received, &HashMap::new());

        // When unbound, the socket can receive packets on the other device.
        api.set_device(&socket, None).expect("clearing bound device failed");
        let (core_ctx, bindings_ctx) = api.contexts();
        receive_packet_on::<I>(core_ctx, bindings_ctx, MultipleDevicesId::B, early_demux_mode)
            .expect("receive udp packet should succeed");
        let received = bindings_ctx.state.received::<I>().iter().collect::<Vec<_>>();
        let (rx_socket, socket_received) =
            assert_matches!(received[..], [(rx_socket, packets)] => (rx_socket, packets));
        assert_eq!(rx_socket, &socket);
        assert_matches!(socket_received.packets[..], [_]);
    }

    /// Check that bind fails as expected when it would cause illegal shadowing.
    #[ip_test(I)]
    fn test_unbind_device_fails<I: TestIpExt>() {
        set_logger_for_test();
        let mut ctx = UdpMultipleDevicesCtx::with_core_ctx(
            UdpMultipleDevicesCoreCtx::new_multiple_devices::<I>(),
        );
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());

        let bound_on_devices = MultipleDevicesId::all().map(|device| {
            let socket = api.create();
            api.set_device(&socket, Some(&device)).unwrap();
            api.listen(&socket, None, Some(LOCAL_PORT)).expect("listen should succeed");
            socket
        });

        // Clearing the bound device is not allowed for either socket since it
        // would then be shadowed by the other socket.
        for socket in bound_on_devices {
            assert_matches!(
                api.set_device(&socket, None),
                Err(SocketError::Local(LocalAddressError::AddressInUse))
            );
        }
    }

    /// Check that binding a device fails if it would make a connected socket
    /// unroutable.
    #[ip_test(I)]
    fn test_bind_conn_socket_device_fails<I: TestIpExt>() {
        set_logger_for_test();
        let device_configs = HashMap::from(
            [(MultipleDevicesId::A, 1), (MultipleDevicesId::B, 2)].map(|(device, i)| {
                (
                    device,
                    FakeDeviceConfig {
                        device,
                        local_ips: vec![I::get_other_ip_address(i)],
                        remote_ips: vec![I::get_other_remote_ip_address(i)],
                    },
                )
            }),
        );
        let mut ctx = UdpMultipleDevicesCtx::with_core_ctx(
            UdpMultipleDevicesCoreCtx::with_ip_socket_ctx_state(FakeDualStackIpSocketCtx::new(
                device_configs.iter().map(|(_, v)| v).cloned(),
            )),
        );
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let socket = api.create();
        api.connect(
            &socket,
            Some(ZonedAddr::Unzoned(device_configs[&MultipleDevicesId::A].remote_ips[0])),
            REMOTE_PORT.into(),
        )
        .expect("connect should succeed");

        // `socket` is not explicitly bound to device `A` but its route must
        // go through it because of the destination address. Therefore binding
        // to device `B` wil not work.
        assert_matches!(
            api.set_device(&socket, Some(&MultipleDevicesId::B)),
            Err(SocketError::Remote(RemoteAddressError::NoRoute))
        );

        // Binding to device `A` should be fine.
        api.set_device(&socket, Some(&MultipleDevicesId::A)).expect("routing picked A already");
    }

    #[ip_test(I)]
    #[test_case(WithEarlyDemux; "with early demux")]
    #[test_case(NoEarlyDemux; "without early demux")]
    fn test_bound_device_receive_multicast_packet<I: TestIpExt>(early_demux_mode: EarlyDemuxMode) {
        set_logger_for_test();
        let remote_ip = I::get_other_ip_address(1);
        let multicast_addr = I::get_multicast_addr(0);

        let mut ctx = UdpMultipleDevicesCtx::with_core_ctx(
            UdpMultipleDevicesCoreCtx::new_multiple_devices::<I>(),
        );
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());

        let sharing_domain = SharingDomain::new(1);

        // Create 3 sockets: one listener bound on each device and one not bound
        // to a device.

        let bound_on_devices = MultipleDevicesId::all().map(|device| {
            let listener = api.create();
            api.set_device(&listener, Some(&device)).unwrap();
            api.set_posix_reuse_port(&listener, ReusePortOption::Enabled(sharing_domain))
                .expect("is unbound");
            api.listen(&listener, None, Some(LOCAL_PORT)).expect("listen should succeed");

            (device, listener)
        });

        let listener = api.create();
        api.set_posix_reuse_port(&listener, ReusePortOption::Enabled(sharing_domain))
            .expect("is unbound");
        api.listen(&listener, None, Some(LOCAL_PORT)).expect("listen should succeed");

        fn index_for_device(id: MultipleDevicesId) -> u8 {
            match id {
                MultipleDevicesId::A => 0,
                MultipleDevicesId::B => 1,
                MultipleDevicesId::C => 2,
            }
        }

        let (core_ctx, bindings_ctx) = api.contexts();
        let mut receive_packet = |remote_ip: SpecifiedAddr<I::Addr>, device: MultipleDevicesId| {
            let meta = UdpPacketMeta::<I> {
                src_ip: remote_ip.get(),
                src_port: Some(REMOTE_PORT),
                dst_ip: multicast_addr.get(),
                dst_port: LOCAL_PORT,
                dscp_and_ecn: DscpAndEcn::default(),
            };
            let body = vec![index_for_device(device)];
            receive_udp_packet(core_ctx, bindings_ctx, device, meta, &body, early_demux_mode)
                .expect("receive udp packet should succeed")
        };

        // Receive packets from the remote IP on each device (2 packets total).
        // Listeners bound on devices should receive one, and the other listener
        // should receive both.
        for device in MultipleDevicesId::all() {
            receive_packet(remote_ip, device);
        }

        let per_socket_data = bindings_ctx.state.socket_data();
        for (device, listener) in bound_on_devices {
            assert_eq!(per_socket_data[&listener.downgrade()], vec![&[index_for_device(device)]]);
        }
        let expected_listener_data = &MultipleDevicesId::all().map(|d| vec![index_for_device(d)]);
        assert_eq!(&per_socket_data[&listener.downgrade()], expected_listener_data);
    }

    /// Tests establishing a UDP connection without providing a local IP
    #[ip_test(I)]
    fn test_conn_unspecified_local_ip<I: TestIpExt>() {
        set_logger_for_test();
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let socket = api.create();
        api.listen(&socket, None, Some(LOCAL_PORT)).expect("listen_udp failed");
        api.connect(&socket, Some(ZonedAddr::Unzoned(remote_ip::<I>())), REMOTE_PORT.into())
            .expect("connect failed");
        let info = api.get_info(&socket);
        assert_eq!(
            info,
            SocketInfo::Connected(datagram::ConnInfo {
                local_ip: StrictlyZonedAddr::new_unzoned_or_panic(local_ip::<I>()),
                local_identifier: LOCAL_PORT,
                remote_ip: StrictlyZonedAddr::new_unzoned_or_panic(remote_ip::<I>()),
                remote_identifier: REMOTE_PORT.into(),
            })
        );
    }

    #[ip_test(I)]
    fn test_multicast_sendto<I: TestIpExt>() {
        set_logger_for_test();

        let mut ctx = UdpMultipleDevicesCtx::with_core_ctx(
            UdpMultipleDevicesCoreCtx::new_multiple_devices::<I>(),
        );

        // Add multicsat route for every device.
        for device in MultipleDevicesId::all().iter() {
            ctx.core_ctx
                .bound_sockets
                .ip_socket_ctx
                .state
                .add_subnet_route(*device, I::MULTICAST_SUBNET);
        }

        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let socket = api.create();

        for (i, target_device) in MultipleDevicesId::all().iter().enumerate() {
            api.set_multicast_interface(&socket, Some(&target_device), I::VERSION)
                .expect("bind should succeed");

            let multicast_ip = I::get_multicast_addr(i.try_into().unwrap());
            api.send_to(
                &socket,
                Some(ZonedAddr::Unzoned(multicast_ip.into())),
                REMOTE_PORT.into(),
                Buf::new(b"packet".to_vec(), ..),
                FakeSendToken::default(),
            )
            .expect("send should succeed");

            let packets = api.core_ctx().bound_sockets.ip_socket_ctx.take_frames();
            assert_eq!(packets.len(), 1usize);
            for (meta, _body) in packets {
                let meta = meta.try_as::<I>().unwrap();
                assert_eq!(meta.device, *target_device);
                assert_eq!(meta.proto, IpProto::Udp.into());
                assert_eq!(meta.src_ip, UdpMultipleDevicesCoreCtx::local_ip(i));
                assert_eq!(meta.dst_ip, multicast_ip.into());
                assert_eq!(meta.destination, IpPacketDestination::Multicast(multicast_ip));
            }
        }
    }

    #[ip_test(I)]
    fn test_multicast_send<I: TestIpExt>() {
        set_logger_for_test();

        let mut ctx = UdpMultipleDevicesCtx::with_core_ctx(
            UdpMultipleDevicesCoreCtx::new_multiple_devices::<I>(),
        );

        // Add multicsat route for every device.
        for device in MultipleDevicesId::all().iter() {
            ctx.core_ctx
                .bound_sockets
                .ip_socket_ctx
                .state
                .add_subnet_route(*device, I::MULTICAST_SUBNET);
        }

        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let multicast_ip = I::get_multicast_addr(42);

        for (i, target_device) in MultipleDevicesId::all().iter().enumerate() {
            let socket = api.create();

            api.set_multicast_interface(&socket, Some(&target_device), I::VERSION)
                .expect("set_multicast_interface should succeed");

            api.connect(&socket, Some(ZonedAddr::Unzoned(multicast_ip.into())), REMOTE_PORT.into())
                .expect("send should succeed");

            api.send(&socket, Buf::new(b"packet".to_vec(), ..), FakeSendToken::default())
                .expect("send should succeed");

            let packets = api.core_ctx().bound_sockets.ip_socket_ctx.take_frames();
            assert_eq!(packets.len(), 1usize);
            for (meta, _body) in packets {
                let meta = meta.try_as::<I>().unwrap();
                assert_eq!(meta.device, *target_device);
                assert_eq!(meta.proto, IpProto::Udp.into());
                assert_eq!(meta.src_ip, UdpMultipleDevicesCoreCtx::local_ip(i));
                assert_eq!(meta.dst_ip, multicast_ip.into());
                assert_eq!(meta.destination, IpPacketDestination::Multicast(multicast_ip));
            }
        }
    }

    /// Tests local port allocation for [`connect`].
    ///
    /// Tests that calling [`connect`] causes a valid local port to be
    /// allocated.
    #[ip_test(I)]
    fn test_udp_local_port_alloc<I: TestIpExt>() {
        let local_ip = local_ip::<I>();
        let ip_a = I::get_other_ip_address(100);
        let ip_b = I::get_other_ip_address(200);

        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(
            UdpFakeDeviceCoreCtx::with_local_remote_ip_addrs(vec![local_ip], vec![ip_a, ip_b]),
        );
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());

        let conn_a = api.create();
        api.connect(&conn_a, Some(ZonedAddr::Unzoned(ip_a)), REMOTE_PORT.into())
            .expect("connect failed");
        let conn_b = api.create();
        api.connect(&conn_b, Some(ZonedAddr::Unzoned(ip_b)), REMOTE_PORT.into())
            .expect("connect failed");
        let conn_c = api.create();
        api.connect(&conn_c, Some(ZonedAddr::Unzoned(ip_a)), OTHER_REMOTE_PORT.into())
            .expect("connect failed");
        let conn_d = api.create();
        api.connect(&conn_d, Some(ZonedAddr::Unzoned(ip_a)), REMOTE_PORT.into())
            .expect("connect failed");
        let valid_range = &FakePortAlloc::<I>::EPHEMERAL_RANGE;
        let mut get_conn_port = |id| {
            let info = api.get_info(&id);
            let info = assert_matches!(info, SocketInfo::Connected(info) => info);
            let datagram::ConnInfo {
                local_ip: _,
                local_identifier,
                remote_ip: _,
                remote_identifier: _,
            } = info;
            local_identifier
        };
        let port_a = get_conn_port(conn_a).get();
        let port_b = get_conn_port(conn_b).get();
        let port_c = get_conn_port(conn_c).get();
        let port_d = get_conn_port(conn_d).get();
        assert!(valid_range.contains(&port_a));
        assert!(valid_range.contains(&port_b));
        assert!(valid_range.contains(&port_c));
        assert!(valid_range.contains(&port_d));
        assert_ne!(port_a, port_b);
        assert_ne!(port_a, port_c);
        assert_ne!(port_a, port_d);
    }

    /// Tests that if `listen_udp` fails, it can be retried later.
    #[ip_test(I)]
    fn test_udp_retry_listen_after_removing_conflict<I: TestIpExt>() {
        set_logger_for_test();
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());

        let listen_unbound = |api: &mut UdpApi<_, _>, socket: &UdpSocketId<_, _, _>| {
            api.listen(socket, Some(ZonedAddr::Unzoned(local_ip::<I>())), Some(LOCAL_PORT))
        };

        // Tie up the address so the second call to `connect` fails.
        let listener = api.create();
        listen_unbound(&mut api, &listener)
            .expect("Initial call to listen_udp was expected to succeed");

        // Trying to connect on the same address should fail.
        let unbound = api.create();
        assert_eq!(
            listen_unbound(&mut api, &unbound),
            Err(Either::Right(LocalAddressError::AddressInUse))
        );

        // Once the first listener is removed, the second socket can be
        // connected.
        let _: (UdpSocketDiagnosticsSeed<I, _, _>, ()) = api.close(listener).into_removed();

        listen_unbound(&mut api, &unbound).expect("listen should succeed");
    }

    /// Tests local port allocation for [`listen_udp`].
    ///
    /// Tests that calling [`listen_udp`] causes a valid local port to be
    /// allocated when no local port is passed.
    #[ip_test(I)]
    fn test_udp_listen_port_alloc<I: TestIpExt>() {
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let local_ip = local_ip::<I>();

        let wildcard_list = api.create();
        api.listen(&wildcard_list, None, None).expect("listen_udp failed");
        let specified_list = api.create();
        api.listen(&specified_list, Some(ZonedAddr::Unzoned(local_ip)), None)
            .expect("listen_udp failed");
        let mut get_listener_port = |id| {
            let info = api.get_info(&id);
            let info = assert_matches!(info, SocketInfo::Listener(info) => info);
            let datagram::ListenerInfo { local_ip: _, local_identifier } = info;
            local_identifier
        };
        let wildcard_port = get_listener_port(wildcard_list);
        let specified_port = get_listener_port(specified_list);
        assert!(FakePortAlloc::<I>::EPHEMERAL_RANGE.contains(&wildcard_port.get()));
        assert!(FakePortAlloc::<I>::EPHEMERAL_RANGE.contains(&specified_port.get()));
        assert_ne!(wildcard_port, specified_port);
    }

    #[ip_test(I)]
    fn test_bind_multiple_reuse_port<I: TestIpExt>() {
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let listeners = [(), ()].map(|()| {
            let socket = api.create();
            let sharing_domain = SharingDomain::new(1);
            api.set_posix_reuse_port(&socket, ReusePortOption::Enabled(sharing_domain))
                .expect("is unbound");
            api.listen(&socket, None, Some(LOCAL_PORT)).expect("listen_udp failed");
            socket
        });

        for listener in listeners {
            assert_eq!(
                api.get_info(&listener),
                SocketInfo::Listener(datagram::ListenerInfo {
                    local_ip: None,
                    local_identifier: LOCAL_PORT
                })
            );
        }
    }

    #[ip_test(I)]
    fn test_set_unset_reuse_port_unbound<I: TestIpExt>() {
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let unbound = api.create();
        let sharing_domain = SharingDomain::new(1);
        api.set_posix_reuse_port(&unbound, ReusePortOption::Enabled(sharing_domain))
            .expect("is unbound");
        api.set_posix_reuse_port(&unbound, ReusePortOption::Disabled).expect("is unbound");
        api.listen(&unbound, None, Some(LOCAL_PORT)).expect("listen_udp failed");

        // Because there is already a listener bound without `SO_REUSEPORT` set,
        // the next bind to the same address should fail.
        assert_eq!(
            {
                let unbound = api.create();
                api.listen(&unbound, None, Some(LOCAL_PORT))
            },
            Err(Either::Right(LocalAddressError::AddressInUse))
        );
    }

    #[ip_test(I)]
    #[test_case(bind_as_listener)]
    #[test_case(bind_as_connected)]
    fn test_set_unset_reuse_port_bound<I: TestIpExt>(
        set_up_socket: impl FnOnce(
            &mut UdpMultipleDevicesCtx,
            &UdpSocketId<
                I,
                FakeWeakDeviceId<MultipleDevicesId>,
                FakeUdpBindingsCtx<MultipleDevicesId>,
            >,
        ),
    ) {
        let mut ctx = UdpMultipleDevicesCtx::with_core_ctx(
            UdpMultipleDevicesCoreCtx::new_multiple_devices::<I>(),
        );
        let socket = UdpApi::<I, _>::new(ctx.as_mut()).create();
        set_up_socket(&mut ctx, &socket);

        // Per src/connectivity/network/netstack3/docs/POSIX_COMPATIBILITY.md,
        // Netstack3 only allows setting SO_REUSEPORT on unbound sockets.
        assert_matches!(
            UdpApi::<I, _>::new(ctx.as_mut())
                .set_posix_reuse_port(&socket, ReusePortOption::Disabled),
            Err(ExpectedUnboundError)
        )
    }

    /// Tests [`remove_udp`]
    #[ip_test(I)]
    fn test_remove_udp_conn<I: TestIpExt>() {
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());

        let local_ip = ZonedAddr::Unzoned(local_ip::<I>());
        let remote_ip = ZonedAddr::Unzoned(remote_ip::<I>());
        let socket = api.create();
        api.listen(&socket, Some(local_ip), Some(LOCAL_PORT)).unwrap();
        api.connect(&socket, Some(remote_ip), REMOTE_PORT.into()).expect("connect failed");
        let _: (UdpSocketDiagnosticsSeed<I, _, _>, ()) = api.close(socket).into_removed();
    }

    /// Tests [`remove_udp`]
    #[ip_test(I)]
    fn test_remove_udp_listener<I: TestIpExt>() {
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let local_ip = ZonedAddr::Unzoned(local_ip::<I>());

        // Test removing a specified listener.
        let specified = api.create();
        api.listen(&specified, Some(local_ip), Some(LOCAL_PORT)).expect("listen_udp failed");
        let _: (UdpSocketDiagnosticsSeed<I, _, _>, ()) = api.close(specified).into_removed();

        // Test removing a wildcard listener.
        let wildcard = api.create();
        api.listen(&wildcard, None, Some(LOCAL_PORT)).expect("listen_udp failed");
        let _: (UdpSocketDiagnosticsSeed<I, _, _>, ()) = api.close(wildcard).into_removed();
    }

    fn try_join_leave_multicast<I: TestIpExt>(
        mcast_addr: MulticastAddr<I::Addr>,
        interface: MulticastMembershipInterfaceSelector<I::Addr, MultipleDevicesId>,
        set_up_ctx: impl FnOnce(&mut UdpMultipleDevicesCtx),
        set_up_socket: impl FnOnce(
            &mut UdpMultipleDevicesCtx,
            &UdpSocketId<
                I,
                FakeWeakDeviceId<MultipleDevicesId>,
                FakeUdpBindingsCtx<MultipleDevicesId>,
            >,
        ),
    ) -> (
        Result<(), SetMulticastMembershipError>,
        HashMap<(MultipleDevicesId, MulticastAddr<I::Addr>), NonZeroUsize>,
    ) {
        let mut ctx = UdpMultipleDevicesCtx::with_core_ctx(
            UdpMultipleDevicesCoreCtx::new_multiple_devices::<I>(),
        );
        set_up_ctx(&mut ctx);

        let socket = UdpApi::<I, _>::new(ctx.as_mut()).create();
        set_up_socket(&mut ctx, &socket);
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let result = api.set_multicast_membership(&socket, mcast_addr, interface, true);

        let memberships_snapshot =
            api.core_ctx().bound_sockets.ip_socket_ctx.state.multicast_memberships::<I>();
        if let Ok(()) = result {
            api.set_multicast_membership(&socket, mcast_addr, interface, false)
                .expect("leaving group failed");
        }
        assert_eq!(
            api.core_ctx().bound_sockets.ip_socket_ctx.state.multicast_memberships::<I>(),
            HashMap::default()
        );

        (result, memberships_snapshot)
    }

    fn leave_unbound<I: TestIpExt>(
        _ctx: &mut UdpMultipleDevicesCtx,
        _unbound: &UdpSocketId<
            I,
            FakeWeakDeviceId<MultipleDevicesId>,
            FakeUdpBindingsCtx<MultipleDevicesId>,
        >,
    ) {
    }

    fn bind_as_listener<I: TestIpExt>(
        ctx: &mut UdpMultipleDevicesCtx,
        unbound: &UdpSocketId<
            I,
            FakeWeakDeviceId<MultipleDevicesId>,
            FakeUdpBindingsCtx<MultipleDevicesId>,
        >,
    ) {
        UdpApi::<I, _>::new(ctx.as_mut())
            .listen(unbound, Some(ZonedAddr::Unzoned(local_ip::<I>())), Some(LOCAL_PORT))
            .expect("listen should succeed")
    }

    fn bind_as_connected<I: TestIpExt>(
        ctx: &mut UdpMultipleDevicesCtx,
        unbound: &UdpSocketId<
            I,
            FakeWeakDeviceId<MultipleDevicesId>,
            FakeUdpBindingsCtx<MultipleDevicesId>,
        >,
    ) {
        UdpApi::<I, _>::new(ctx.as_mut())
            .connect(
                unbound,
                Some(ZonedAddr::Unzoned(I::get_other_remote_ip_address(1))),
                REMOTE_PORT.into(),
            )
            .expect("connect should succeed")
    }

    fn iface_id<A: IpAddress>(
        id: MultipleDevicesId,
    ) -> MulticastMembershipInterfaceSelector<A, MultipleDevicesId> {
        MulticastInterfaceSelector::Interface(id).into()
    }
    fn iface_addr<A: IpAddress>(
        addr: SpecifiedAddr<A>,
    ) -> MulticastMembershipInterfaceSelector<A, MultipleDevicesId> {
        MulticastInterfaceSelector::LocalAddress(addr).into()
    }

    #[ip_test(I)]
    #[test_case(iface_id(MultipleDevicesId::A), leave_unbound::<I>; "device_no_addr_unbound")]
    #[test_case(iface_addr(local_ip::<I>()), leave_unbound::<I>; "addr_no_device_unbound")]
    #[test_case(MulticastMembershipInterfaceSelector::AnyInterfaceWithRoute, leave_unbound::<I>;
        "any_interface_unbound")]
    #[test_case(iface_id(MultipleDevicesId::A), bind_as_listener::<I>; "device_no_addr_listener")]
    #[test_case(iface_addr(local_ip::<I>()), bind_as_listener::<I>; "addr_no_device_listener")]
    #[test_case(MulticastMembershipInterfaceSelector::AnyInterfaceWithRoute, bind_as_listener::<I>;
        "any_interface_listener")]
    #[test_case(iface_id(MultipleDevicesId::A), bind_as_connected::<I>; "device_no_addr_connected")]
    #[test_case(iface_addr(local_ip::<I>()), bind_as_connected::<I>; "addr_no_device_connected")]
    #[test_case(MulticastMembershipInterfaceSelector::AnyInterfaceWithRoute, bind_as_connected::<I>;
        "any_interface_connected")]
    fn test_join_leave_multicast_succeeds<I: TestIpExt>(
        interface: MulticastMembershipInterfaceSelector<I::Addr, MultipleDevicesId>,
        set_up_socket: impl FnOnce(
            &mut UdpMultipleDevicesCtx,
            &UdpSocketId<
                I,
                FakeWeakDeviceId<MultipleDevicesId>,
                FakeUdpBindingsCtx<MultipleDevicesId>,
            >,
        ),
    ) {
        let mcast_addr = I::get_multicast_addr(3);

        let set_up_ctx = |ctx: &mut UdpMultipleDevicesCtx| {
            // Ensure there is a route to the multicast address, if the interface
            // selector requires it.
            match interface {
                MulticastMembershipInterfaceSelector::Specified(_) => {}
                MulticastMembershipInterfaceSelector::AnyInterfaceWithRoute => {
                    ctx.core_ctx
                        .bound_sockets
                        .ip_socket_ctx
                        .state
                        .add_route(MultipleDevicesId::A, mcast_addr.into_specified().into());
                }
            }
        };

        let (result, ip_options) =
            try_join_leave_multicast(mcast_addr, interface, set_up_ctx, set_up_socket);
        assert_eq!(result, Ok(()));
        assert_eq!(
            ip_options,
            HashMap::from([((MultipleDevicesId::A, mcast_addr), NonZeroUsize::new(1).unwrap())])
        );
    }

    #[ip_test(I)]
    #[test_case(leave_unbound::<I>; "unbound")]
    #[test_case(bind_as_listener::<I>; "listener")]
    #[test_case(bind_as_connected::<I>; "connected")]
    fn test_join_multicast_fails_without_route<I: TestIpExt>(
        set_up_socket: impl FnOnce(
            &mut UdpMultipleDevicesCtx,
            &UdpSocketId<
                I,
                FakeWeakDeviceId<MultipleDevicesId>,
                FakeUdpBindingsCtx<MultipleDevicesId>,
            >,
        ),
    ) {
        let mcast_addr = I::get_multicast_addr(3);

        let (result, ip_options) = try_join_leave_multicast(
            mcast_addr,
            MulticastMembershipInterfaceSelector::AnyInterfaceWithRoute,
            |_: &mut UdpMultipleDevicesCtx| { /* Don't install a route to `mcast_addr` */ },
            set_up_socket,
        );
        assert_eq!(result, Err(SetMulticastMembershipError::NoDeviceAvailable));
        assert_eq!(ip_options, HashMap::new());
    }

    #[ip_test(I)]
    #[test_case(MultipleDevicesId::A, Some(local_ip::<I>()), leave_unbound, Ok(());
        "with_ip_unbound")]
    #[test_case(MultipleDevicesId::A, None, leave_unbound, Ok(());
        "without_ip_unbound")]
    #[test_case(MultipleDevicesId::A, Some(local_ip::<I>()), bind_as_listener, Ok(());
        "with_ip_listener")]
    #[test_case(MultipleDevicesId::A, Some(local_ip::<I>()), bind_as_connected, Ok(());
        "with_ip_connected")]
    fn test_join_leave_multicast_interface_inferred_from_bound_device<I: TestIpExt>(
        bound_device: MultipleDevicesId,
        interface_addr: Option<SpecifiedAddr<I::Addr>>,
        set_up_socket: impl FnOnce(
            &mut UdpMultipleDevicesCtx,
            &UdpSocketId<
                I,
                FakeWeakDeviceId<MultipleDevicesId>,
                FakeUdpBindingsCtx<MultipleDevicesId>,
            >,
        ),
        expected_result: Result<(), SetMulticastMembershipError>,
    ) {
        let mcast_addr = I::get_multicast_addr(3);
        let (result, ip_options) = try_join_leave_multicast(
            mcast_addr,
            interface_addr
                .map(MulticastInterfaceSelector::LocalAddress)
                .map(Into::into)
                .unwrap_or(MulticastMembershipInterfaceSelector::AnyInterfaceWithRoute),
            |_: &mut UdpMultipleDevicesCtx| { /* No ctx setup required */ },
            |ctx, unbound| {
                UdpApi::<I, _>::new(ctx.as_mut())
                    .set_device(&unbound, Some(&bound_device))
                    .unwrap();
                set_up_socket(ctx, &unbound)
            },
        );
        assert_eq!(result, expected_result);
        assert_eq!(
            ip_options,
            expected_result.map_or_else(
                |_| HashMap::default(),
                |()| HashMap::from([((bound_device, mcast_addr), NonZeroUsize::new(1).unwrap())])
            )
        );
    }

    #[ip_test(I)]
    fn test_multicast_membership_with_removed_device<I: TestIpExt>() {
        let device = FakeReferencyDeviceId::default();
        let mut ctx =
            FakeUdpCtx::with_core_ctx(FakeUdpCoreCtx::new_with_device::<I>(device.clone()));
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());

        let unbound = api.create();
        api.set_device(&unbound, Some(&device)).unwrap();

        device.mark_removed();

        let group = I::get_multicast_addr(4);
        assert_eq!(
            api.set_multicast_membership(
                &unbound,
                group,
                // Will use the socket's bound device.
                MulticastMembershipInterfaceSelector::AnyInterfaceWithRoute,
                true,
            ),
            Err(SetMulticastMembershipError::DeviceDoesNotExist),
        );

        // Should not have updated the device's multicast state.
        //
        // Note that even though we mock the device being removed above, its
        // state still exists in the fake IP socket context so we can inspect
        // it here.
        assert_eq!(
            api.core_ctx().bound_sockets.ip_socket_ctx.state.multicast_memberships::<I>(),
            HashMap::default(),
        );
    }

    #[ip_test(I)]
    fn test_remove_udp_unbound_leaves_multicast_groups<I: TestIpExt>() {
        let mut ctx = UdpMultipleDevicesCtx::with_core_ctx(
            UdpMultipleDevicesCoreCtx::new_multiple_devices::<I>(),
        );
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());

        let unbound = api.create();
        let group = I::get_multicast_addr(4);
        api.set_multicast_membership(
            &unbound,
            group,
            MulticastInterfaceSelector::LocalAddress(local_ip::<I>()).into(),
            true,
        )
        .expect("join group failed");

        assert_eq!(
            api.core_ctx().bound_sockets.ip_socket_ctx.state.multicast_memberships::<I>(),
            HashMap::from([((MultipleDevicesId::A, group), NonZeroUsize::new(1).unwrap())])
        );

        let _: (UdpSocketDiagnosticsSeed<I, _, _>, ()) = api.close(unbound).into_removed();
        assert_eq!(
            api.core_ctx().bound_sockets.ip_socket_ctx.state.multicast_memberships::<I>(),
            HashMap::default()
        );
    }

    #[ip_test(I)]
    fn test_remove_udp_listener_leaves_multicast_groups<I: TestIpExt>() {
        let mut ctx = UdpMultipleDevicesCtx::with_core_ctx(
            UdpMultipleDevicesCoreCtx::new_multiple_devices::<I>(),
        );
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let local_ip = local_ip::<I>();

        let socket = api.create();
        let first_group = I::get_multicast_addr(4);
        api.set_multicast_membership(
            &socket,
            first_group,
            MulticastInterfaceSelector::LocalAddress(local_ip).into(),
            true,
        )
        .expect("join group failed");

        api.listen(&socket, Some(ZonedAddr::Unzoned(local_ip)), Some(LOCAL_PORT))
            .expect("listen_udp failed");
        let second_group = I::get_multicast_addr(5);
        api.set_multicast_membership(
            &socket,
            second_group,
            MulticastInterfaceSelector::LocalAddress(local_ip).into(),
            true,
        )
        .expect("join group failed");

        assert_eq!(
            api.core_ctx().bound_sockets.ip_socket_ctx.state.multicast_memberships::<I>(),
            HashMap::from([
                ((MultipleDevicesId::A, first_group), NonZeroUsize::new(1).unwrap()),
                ((MultipleDevicesId::A, second_group), NonZeroUsize::new(1).unwrap())
            ])
        );

        let _: (UdpSocketDiagnosticsSeed<I, _, _>, ()) = api.close(socket).into_removed();
        assert_eq!(
            api.core_ctx().bound_sockets.ip_socket_ctx.state.multicast_memberships::<I>(),
            HashMap::default()
        );
    }

    #[ip_test(I)]
    fn test_remove_udp_connected_leaves_multicast_groups<I: TestIpExt>() {
        let mut ctx = UdpMultipleDevicesCtx::with_core_ctx(
            UdpMultipleDevicesCoreCtx::new_multiple_devices::<I>(),
        );
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let local_ip = local_ip::<I>();

        let socket = api.create();
        let first_group = I::get_multicast_addr(4);
        api.set_multicast_membership(
            &socket,
            first_group,
            MulticastInterfaceSelector::LocalAddress(local_ip).into(),
            true,
        )
        .expect("join group failed");

        api.connect(
            &socket,
            Some(ZonedAddr::Unzoned(I::get_other_remote_ip_address(1))),
            REMOTE_PORT.into(),
        )
        .expect("connect failed");

        let second_group = I::get_multicast_addr(5);
        api.set_multicast_membership(
            &socket,
            second_group,
            MulticastInterfaceSelector::LocalAddress(local_ip).into(),
            true,
        )
        .expect("join group failed");

        assert_eq!(
            api.core_ctx().bound_sockets.ip_socket_ctx.state.multicast_memberships::<I>(),
            HashMap::from([
                ((MultipleDevicesId::A, first_group), NonZeroUsize::new(1).unwrap()),
                ((MultipleDevicesId::A, second_group), NonZeroUsize::new(1).unwrap())
            ])
        );

        let _: (UdpSocketDiagnosticsSeed<I, _, _>, ()) = api.close(socket).into_removed();
        assert_eq!(
            api.core_ctx().bound_sockets.ip_socket_ctx.state.multicast_memberships::<I>(),
            HashMap::default()
        );
    }

    #[ip_test(I)]
    #[should_panic(expected = "listen again failed")]
    fn test_listen_udp_removes_unbound<I: TestIpExt>() {
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let local_ip = local_ip::<I>();
        let socket = api.create();

        api.listen(&socket, Some(ZonedAddr::Unzoned(local_ip)), Some(LOCAL_PORT))
            .expect("listen_udp failed");

        // Attempting to create a new listener from the same unbound ID should
        // panic since the unbound socket ID is now invalid.
        api.listen(&socket, Some(ZonedAddr::Unzoned(local_ip)), Some(OTHER_LOCAL_PORT))
            .expect("listen again failed");
    }

    #[ip_test(I)]
    fn test_get_conn_info<I: TestIpExt>() {
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let local_ip = ZonedAddr::Unzoned(local_ip::<I>());
        let remote_ip = ZonedAddr::Unzoned(remote_ip::<I>());
        // Create a UDP connection with a specified local port and local IP.
        let socket = api.create();
        api.listen(&socket, Some(local_ip), Some(LOCAL_PORT)).expect("listen_udp failed");
        api.connect(&socket, Some(remote_ip), REMOTE_PORT.into()).expect("connect failed");
        let info = api.get_info(&socket);
        let info = assert_matches!(info, SocketInfo::Connected(info) => info);
        assert_eq!(info.local_ip.into_inner(), local_ip.map_zone(FakeWeakDeviceId));
        assert_eq!(info.local_identifier, LOCAL_PORT);
        assert_eq!(info.remote_ip.into_inner(), remote_ip.map_zone(FakeWeakDeviceId));
        assert_eq!(info.remote_identifier, u16::from(REMOTE_PORT));
    }

    #[ip_test(I)]
    fn test_get_listener_info<I: TestIpExt>() {
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let local_ip = ZonedAddr::Unzoned(local_ip::<I>());

        // Check getting info on specified listener.
        let specified = api.create();
        api.listen(&specified, Some(local_ip), Some(LOCAL_PORT)).expect("listen_udp failed");
        let info = api.get_info(&specified);
        let info = assert_matches!(info, SocketInfo::Listener(info) => info);
        assert_eq!(info.local_ip.unwrap().into_inner(), local_ip.map_zone(FakeWeakDeviceId));
        assert_eq!(info.local_identifier, LOCAL_PORT);

        // Check getting info on wildcard listener.
        let wildcard = api.create();
        api.listen(&wildcard, None, Some(OTHER_LOCAL_PORT)).expect("listen_udp failed");
        let info = api.get_info(&wildcard);
        let info = assert_matches!(info, SocketInfo::Listener(info) => info);
        assert_eq!(info.local_ip, None);
        assert_eq!(info.local_identifier, OTHER_LOCAL_PORT);
    }

    #[ip_test(I)]
    fn test_get_reuse_port<I: TestIpExt>() {
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let first = api.create();
        assert_eq!(api.get_posix_reuse_port(&first), false);

        let sharing_domain = SharingDomain::new(1);
        api.set_posix_reuse_port(&first, ReusePortOption::Enabled(sharing_domain))
            .expect("is unbound");

        assert_eq!(api.get_posix_reuse_port(&first), true);

        api.listen(&first, Some(ZonedAddr::Unzoned(local_ip::<I>())), None).expect("listen failed");
        assert_eq!(api.get_posix_reuse_port(&first), true);
        let _: (UdpSocketDiagnosticsSeed<I, _, _>, ()) = api.close(first).into_removed();

        let second = api.create();
        api.set_posix_reuse_port(&second, ReusePortOption::Enabled(sharing_domain))
            .expect("is unbound");
        api.connect(&second, Some(ZonedAddr::Unzoned(remote_ip::<I>())), REMOTE_PORT.into())
            .expect("connect failed");

        assert_eq!(api.get_posix_reuse_port(&second), true);
    }

    #[ip_test(I)]
    fn test_get_bound_device_unbound<I: TestIpExt>() {
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let unbound = api.create();

        assert_eq!(api.get_bound_device(&unbound), None);

        api.set_device(&unbound, Some(&FakeDeviceId)).unwrap();
        assert_eq!(api.get_bound_device(&unbound), Some(FakeWeakDeviceId(FakeDeviceId)));
    }

    #[ip_test(I)]
    fn test_get_bound_device_listener<I: TestIpExt>() {
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let socket = api.create();

        api.set_device(&socket, Some(&FakeDeviceId)).unwrap();
        api.listen(&socket, Some(ZonedAddr::Unzoned(local_ip::<I>())), Some(LOCAL_PORT))
            .expect("failed to listen");
        assert_eq!(api.get_bound_device(&socket), Some(FakeWeakDeviceId(FakeDeviceId)));

        api.set_device(&socket, None).expect("failed to set device");
        assert_eq!(api.get_bound_device(&socket), None);
    }

    #[ip_test(I)]
    fn test_get_bound_device_connected<I: TestIpExt>() {
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let socket = api.create();
        api.set_device(&socket, Some(&FakeDeviceId)).unwrap();
        api.connect(&socket, Some(ZonedAddr::Unzoned(remote_ip::<I>())), REMOTE_PORT.into())
            .expect("failed to connect");
        assert_eq!(api.get_bound_device(&socket), Some(FakeWeakDeviceId(FakeDeviceId)));
        api.set_device(&socket, None).expect("failed to set device");
        assert_eq!(api.get_bound_device(&socket), None);
    }

    #[ip_test(I)]
    fn test_listen_udp_forwards_errors<I: TestIpExt>() {
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let remote_ip = remote_ip::<I>();

        // Check listening to a non-local IP fails.
        let unbound = api.create();
        let listen_err = api
            .listen(&unbound, Some(ZonedAddr::Unzoned(remote_ip)), Some(LOCAL_PORT))
            .expect_err("listen_udp unexpectedly succeeded");
        assert_eq!(listen_err, Either::Right(LocalAddressError::CannotBindToAddress));

        let unbound = api.create();
        let _ = api.listen(&unbound, None, Some(OTHER_LOCAL_PORT)).expect("listen_udp failed");
        let unbound = api.create();
        let listen_err = api
            .listen(&unbound, None, Some(OTHER_LOCAL_PORT))
            .expect_err("listen_udp unexpectedly succeeded");
        assert_eq!(listen_err, Either::Right(LocalAddressError::AddressInUse));
    }

    const IPV6_LINK_LOCAL_ADDR: Ipv6Addr = net_ip_v6!("fe80::1234");
    #[test_case(IPV6_LINK_LOCAL_ADDR, IPV6_LINK_LOCAL_ADDR; "unicast")]
    #[test_case(IPV6_LINK_LOCAL_ADDR, MulticastAddr::new(net_ip_v6!("ff02::1234")).unwrap().get(); "multicast")]
    fn test_listen_udp_ipv6_link_local_requires_zone(
        interface_addr: Ipv6Addr,
        bind_addr: Ipv6Addr,
    ) {
        type I = Ipv6;
        let interface_addr = LinkLocalAddr::new(interface_addr).unwrap().into_specified();

        let mut ctx =
            UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::with_local_remote_ip_addrs(
                vec![interface_addr],
                vec![remote_ip::<I>()],
            ));
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());

        let bind_addr = LinkLocalAddr::new(bind_addr).unwrap().into_specified();
        assert!(bind_addr.scope().can_have_zone());

        let unbound = api.create();
        let result = api.listen(&unbound, Some(ZonedAddr::Unzoned(bind_addr)), Some(LOCAL_PORT));
        assert_eq!(
            result,
            Err(Either::Right(LocalAddressError::Zone(ZonedAddressError::RequiredZoneNotProvided)))
        );
    }

    #[test_case(MultipleDevicesId::A, Ok(()); "matching")]
    #[test_case(MultipleDevicesId::B, Err(LocalAddressError::Zone(ZonedAddressError::DeviceZoneMismatch)); "not matching")]
    fn test_listen_udp_ipv6_link_local_with_bound_device_set(
        zone_id: MultipleDevicesId,
        expected_result: Result<(), LocalAddressError>,
    ) {
        type I = Ipv6;
        let ll_addr = LinkLocalAddr::new(net_ip_v6!("fe80::1234")).unwrap().into_specified();
        assert!(ll_addr.scope().can_have_zone());

        let remote_ips = vec![remote_ip::<I>()];
        let mut ctx = UdpMultipleDevicesCtx::with_core_ctx(
            UdpMultipleDevicesCoreCtx::with_ip_socket_ctx_state(FakeDualStackIpSocketCtx::new(
                [(MultipleDevicesId::A, ll_addr), (MultipleDevicesId::B, local_ip::<I>())].map(
                    |(device, local_ip)| FakeDeviceConfig {
                        device,
                        local_ips: vec![local_ip],
                        remote_ips: remote_ips.clone(),
                    },
                ),
            )),
        );
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());

        let socket = api.create();
        api.set_device(&socket, Some(&MultipleDevicesId::A)).unwrap();

        let result = api
            .listen(
                &socket,
                Some(ZonedAddr::Zoned(AddrAndZone::new(ll_addr, zone_id).unwrap())),
                Some(LOCAL_PORT),
            )
            .map_err(Either::unwrap_right);
        assert_eq!(result, expected_result);
    }

    #[test_case(MultipleDevicesId::A, Ok(()); "matching")]
    #[test_case(MultipleDevicesId::B, Err(LocalAddressError::AddressMismatch); "not matching")]
    fn test_listen_udp_ipv6_link_local_with_zone_requires_addr_assigned_to_device(
        zone_id: MultipleDevicesId,
        expected_result: Result<(), LocalAddressError>,
    ) {
        type I = Ipv6;
        let ll_addr = LinkLocalAddr::new(net_ip_v6!("fe80::1234")).unwrap().into_specified();
        assert!(ll_addr.scope().can_have_zone());

        let remote_ips = vec![remote_ip::<I>()];
        let mut ctx = UdpMultipleDevicesCtx::with_core_ctx(
            UdpMultipleDevicesCoreCtx::with_ip_socket_ctx_state(FakeDualStackIpSocketCtx::new(
                [(MultipleDevicesId::A, ll_addr), (MultipleDevicesId::B, local_ip::<I>())].map(
                    |(device, local_ip)| FakeDeviceConfig {
                        device,
                        local_ips: vec![local_ip],
                        remote_ips: remote_ips.clone(),
                    },
                ),
            )),
        );
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());

        let socket = api.create();
        let result = api
            .listen(
                &socket,
                Some(ZonedAddr::Zoned(AddrAndZone::new(ll_addr, zone_id).unwrap())),
                Some(LOCAL_PORT),
            )
            .map_err(Either::unwrap_right);
        assert_eq!(result, expected_result);
    }

    #[test_case(None, Err(LocalAddressError::Zone(ZonedAddressError::DeviceZoneMismatch)); "clear device")]
    #[test_case(Some(MultipleDevicesId::A), Ok(()); "set same device")]
    #[test_case(Some(MultipleDevicesId::B),
                Err(LocalAddressError::Zone(ZonedAddressError::DeviceZoneMismatch)); "change device")]
    fn test_listen_udp_ipv6_listen_link_local_update_bound_device(
        new_device: Option<MultipleDevicesId>,
        expected_result: Result<(), LocalAddressError>,
    ) {
        type I = Ipv6;
        let ll_addr = LinkLocalAddr::new(net_ip_v6!("fe80::1234")).unwrap().into_specified();
        assert!(ll_addr.scope().can_have_zone());

        let remote_ips = vec![remote_ip::<I>()];
        let mut ctx = UdpMultipleDevicesCtx::with_core_ctx(
            UdpMultipleDevicesCoreCtx::with_ip_socket_ctx_state(FakeDualStackIpSocketCtx::new(
                [(MultipleDevicesId::A, ll_addr), (MultipleDevicesId::B, local_ip::<I>())].map(
                    |(device, local_ip)| FakeDeviceConfig {
                        device,
                        local_ips: vec![local_ip],
                        remote_ips: remote_ips.clone(),
                    },
                ),
            )),
        );
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());

        let socket = api.create();
        api.listen(
            &socket,
            Some(ZonedAddr::Zoned(AddrAndZone::new(ll_addr, MultipleDevicesId::A).unwrap())),
            Some(LOCAL_PORT),
        )
        .expect("listen failed");

        assert_eq!(api.get_bound_device(&socket), Some(FakeWeakDeviceId(MultipleDevicesId::A)));

        assert_eq!(
            api.set_device(&socket, new_device.as_ref()),
            expected_result.map_err(SocketError::Local),
        );
    }

    #[test_case(None; "bind all IPs")]
    #[test_case(Some(ZonedAddr::Unzoned(local_ip::<Ipv6>())); "bind unzoned")]
    #[test_case(Some(ZonedAddr::Zoned(AddrAndZone::new(SpecifiedAddr::new(net_ip_v6!("fe80::1")).unwrap(),
        MultipleDevicesId::A).unwrap())); "bind with same zone")]
    fn test_udp_ipv6_connect_with_unzoned(
        bound_addr: Option<ZonedAddr<SpecifiedAddr<Ipv6Addr>, MultipleDevicesId>>,
    ) {
        let remote_ips = vec![remote_ip::<Ipv6>()];

        let mut ctx = UdpMultipleDevicesCtx::with_core_ctx(
            UdpMultipleDevicesCoreCtx::with_ip_socket_ctx_state(FakeDualStackIpSocketCtx::new([
                FakeDeviceConfig {
                    device: MultipleDevicesId::A,
                    local_ips: vec![
                        local_ip::<Ipv6>(),
                        SpecifiedAddr::new(net_ip_v6!("fe80::1")).unwrap(),
                    ],
                    remote_ips: remote_ips.clone(),
                },
                FakeDeviceConfig {
                    device: MultipleDevicesId::B,
                    local_ips: vec![SpecifiedAddr::new(net_ip_v6!("fe80::2")).unwrap()],
                    remote_ips: remote_ips,
                },
            ])),
        );
        let mut api = UdpApi::<Ipv6, _>::new(ctx.as_mut());

        let socket = api.create();

        api.listen(&socket, bound_addr, Some(LOCAL_PORT)).unwrap();

        assert_matches!(
            api.connect(
                &socket,
                Some(ZonedAddr::Unzoned(remote_ip::<Ipv6>())),
                REMOTE_PORT.into(),
            ),
            Ok(())
        );
    }

    #[test]
    fn test_udp_ipv6_connect_zoned_get_info() {
        let ll_addr = LinkLocalAddr::new(net_ip_v6!("fe80::1234")).unwrap().into_specified();
        assert!(ll_addr.must_have_zone());

        let remote_ips = vec![remote_ip::<Ipv6>()];
        let mut ctx = UdpMultipleDevicesCtx::with_core_ctx(
            UdpMultipleDevicesCoreCtx::with_ip_socket_ctx_state(FakeDualStackIpSocketCtx::new(
                [(MultipleDevicesId::A, ll_addr), (MultipleDevicesId::B, local_ip::<Ipv6>())].map(
                    |(device, local_ip)| FakeDeviceConfig {
                        device,
                        local_ips: vec![local_ip],
                        remote_ips: remote_ips.clone(),
                    },
                ),
            )),
        );

        let mut api = UdpApi::<Ipv6, _>::new(ctx.as_mut());
        let socket = api.create();
        api.set_device(&socket, Some(&MultipleDevicesId::A)).unwrap();

        let zoned_local_addr =
            ZonedAddr::Zoned(AddrAndZone::new(ll_addr, MultipleDevicesId::A).unwrap());
        api.listen(&socket, Some(zoned_local_addr), Some(LOCAL_PORT)).unwrap();

        api.connect(&socket, Some(ZonedAddr::Unzoned(remote_ip::<Ipv6>())), REMOTE_PORT.into())
            .expect("connect should succeed");

        assert_eq!(
            api.get_info(&socket),
            SocketInfo::Connected(datagram::ConnInfo {
                local_ip: StrictlyZonedAddr::new_with_zone(ll_addr, || FakeWeakDeviceId(
                    MultipleDevicesId::A
                )),
                local_identifier: LOCAL_PORT,
                remote_ip: StrictlyZonedAddr::new_unzoned_or_panic(remote_ip::<Ipv6>()),
                remote_identifier: REMOTE_PORT.into(),
            })
        );
    }

    #[test_case(ZonedAddr::Zoned(AddrAndZone::new(SpecifiedAddr::new(net_ip_v6!("fe80::2")).unwrap(),
        MultipleDevicesId::B).unwrap()),
        Err(ConnectError::Zone(ZonedAddressError::DeviceZoneMismatch));
        "connect to different zone")]
    #[test_case(ZonedAddr::Unzoned(SpecifiedAddr::new(net_ip_v6!("fe80::3")).unwrap()),
        Ok(FakeWeakDeviceId(MultipleDevicesId::A)); "connect implicit zone")]
    fn test_udp_ipv6_bind_zoned(
        remote_addr: ZonedAddr<SpecifiedAddr<Ipv6Addr>, MultipleDevicesId>,
        expected: Result<FakeWeakDeviceId<MultipleDevicesId>, ConnectError>,
    ) {
        let remote_ips = vec![SpecifiedAddr::new(net_ip_v6!("fe80::3")).unwrap()];

        let mut ctx = UdpMultipleDevicesCtx::with_core_ctx(
            UdpMultipleDevicesCoreCtx::with_ip_socket_ctx_state(FakeDualStackIpSocketCtx::new([
                FakeDeviceConfig {
                    device: MultipleDevicesId::A,
                    local_ips: vec![SpecifiedAddr::new(net_ip_v6!("fe80::1")).unwrap()],
                    remote_ips: remote_ips.clone(),
                },
                FakeDeviceConfig {
                    device: MultipleDevicesId::B,
                    local_ips: vec![SpecifiedAddr::new(net_ip_v6!("fe80::2")).unwrap()],
                    remote_ips: remote_ips,
                },
            ])),
        );

        let mut api = UdpApi::<Ipv6, _>::new(ctx.as_mut());

        let socket = api.create();

        api.listen(
            &socket,
            Some(ZonedAddr::Zoned(
                AddrAndZone::new(
                    SpecifiedAddr::new(net_ip_v6!("fe80::1")).unwrap(),
                    MultipleDevicesId::A,
                )
                .unwrap(),
            )),
            Some(LOCAL_PORT),
        )
        .unwrap();

        let result = api
            .connect(&socket, Some(remote_addr), REMOTE_PORT.into())
            .map(|()| api.get_bound_device(&socket).unwrap());
        assert_eq!(result, expected);
    }

    #[ip_test(I)]
    fn test_listen_udp_loopback_no_zone_is_required<I: TestIpExt>() {
        let loopback_addr = I::LOOPBACK_ADDRESS;
        let remote_ips = vec![remote_ip::<I>()];

        let mut ctx = UdpMultipleDevicesCtx::with_core_ctx(
            UdpMultipleDevicesCoreCtx::with_ip_socket_ctx_state(FakeDualStackIpSocketCtx::new(
                [(MultipleDevicesId::A, loopback_addr), (MultipleDevicesId::B, local_ip::<I>())]
                    .map(|(device, local_ip)| FakeDeviceConfig {
                        device,
                        local_ips: vec![local_ip],
                        remote_ips: remote_ips.clone(),
                    }),
            )),
        );
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());

        let unbound = api.create();
        api.set_device(&unbound, Some(&MultipleDevicesId::A)).unwrap();

        let result =
            api.listen(&unbound, Some(ZonedAddr::Unzoned(loopback_addr)), Some(LOCAL_PORT));
        assert_matches!(result, Ok(_));
    }

    #[test_case(None, true, Ok(()); "connected success")]
    #[test_case(None, false, Ok(()); "listening success")]
    #[test_case(Some(MultipleDevicesId::A), true, Ok(()); "conn bind same device")]
    #[test_case(Some(MultipleDevicesId::A), false, Ok(()); "listen bind same device")]
    #[test_case(
        Some(MultipleDevicesId::B),
        true,
        Err(SendToError::Zone(ZonedAddressError::DeviceZoneMismatch));
        "conn bind different device")]
    #[test_case(
        Some(MultipleDevicesId::B),
        false,
        Err(SendToError::Zone(ZonedAddressError::DeviceZoneMismatch));
        "listen bind different device")]
    fn test_udp_ipv6_send_to_zoned(
        bind_device: Option<MultipleDevicesId>,
        connect: bool,
        expected: Result<(), SendToError>,
    ) {
        let ll_addr = LinkLocalAddr::new(net_ip_v6!("fe80::1234")).unwrap().into_specified();
        assert!(ll_addr.must_have_zone());
        let conn_remote_ip = Ipv6::get_other_remote_ip_address(1);

        let mut ctx = UdpMultipleDevicesCtx::with_core_ctx(
            UdpMultipleDevicesCoreCtx::with_ip_socket_ctx_state(FakeDualStackIpSocketCtx::new(
                [
                    (MultipleDevicesId::A, Ipv6::get_other_ip_address(1)),
                    (MultipleDevicesId::B, Ipv6::get_other_ip_address(2)),
                ]
                .map(|(device, local_ip)| FakeDeviceConfig {
                    device,
                    local_ips: vec![local_ip],
                    remote_ips: vec![ll_addr, conn_remote_ip],
                }),
            )),
        );

        let mut api = UdpApi::<Ipv6, _>::new(ctx.as_mut());
        let socket = api.create();

        if let Some(device) = bind_device {
            api.set_device(&socket, Some(&device)).unwrap();
        }

        let send_to_remote_addr =
            ZonedAddr::Zoned(AddrAndZone::new(ll_addr, MultipleDevicesId::A).unwrap());
        let result = if connect {
            api.connect(&socket, Some(ZonedAddr::Unzoned(conn_remote_ip)), REMOTE_PORT.into())
                .expect("connect should succeed");
            api.send_to(
                &socket,
                Some(send_to_remote_addr),
                REMOTE_PORT.into(),
                Buf::new(Vec::new(), ..),
                FakeSendToken::default(),
            )
        } else {
            api.listen(&socket, None, Some(LOCAL_PORT)).expect("listen should succeed");
            api.send_to(
                &socket,
                Some(send_to_remote_addr),
                REMOTE_PORT.into(),
                Buf::new(Vec::new(), ..),
                FakeSendToken::default(),
            )
        };

        assert_eq!(result, expected);
    }

    #[test_case(true; "connected")]
    #[test_case(false; "listening")]
    fn test_udp_ipv6_bound_zoned_send_to_zoned(connect: bool) {
        let ll_addr = LinkLocalAddr::new(net_ip_v6!("fe80::5678")).unwrap().into_specified();
        let device_a_local_ip = net_ip_v6!("fe80::1111");
        let conn_remote_ip = Ipv6::get_other_remote_ip_address(1);

        let mut ctx = UdpMultipleDevicesCtx::with_core_ctx(
            UdpMultipleDevicesCoreCtx::with_ip_socket_ctx_state(FakeDualStackIpSocketCtx::new(
                [
                    (MultipleDevicesId::A, device_a_local_ip),
                    (MultipleDevicesId::B, net_ip_v6!("fe80::2222")),
                ]
                .map(|(device, local_ip)| FakeDeviceConfig {
                    device,
                    local_ips: vec![LinkLocalAddr::new(local_ip).unwrap().into_specified()],
                    remote_ips: vec![ll_addr, conn_remote_ip],
                }),
            )),
        );
        let mut api = UdpApi::<Ipv6, _>::new(ctx.as_mut());

        let socket = api.create();
        api.listen(
            &socket,
            Some(ZonedAddr::Zoned(
                AddrAndZone::new(
                    SpecifiedAddr::new(device_a_local_ip).unwrap(),
                    MultipleDevicesId::A,
                )
                .unwrap(),
            )),
            Some(LOCAL_PORT),
        )
        .expect("listen should succeed");

        // Use a remote address on device B, while the socket is listening on
        // device A. This should cause a failure when sending.
        let send_to_remote_addr =
            ZonedAddr::Zoned(AddrAndZone::new(ll_addr, MultipleDevicesId::B).unwrap());

        let result = if connect {
            api.connect(&socket, Some(ZonedAddr::Unzoned(conn_remote_ip)), REMOTE_PORT.into())
                .expect("connect should succeed");
            api.send_to(
                &socket,
                Some(send_to_remote_addr),
                REMOTE_PORT.into(),
                Buf::new(Vec::new(), ..),
                FakeSendToken::default(),
            )
        } else {
            api.send_to(
                &socket,
                Some(send_to_remote_addr),
                REMOTE_PORT.into(),
                Buf::new(Vec::new(), ..),
                FakeSendToken::default(),
            )
        };

        assert_matches!(result, Err(SendToError::Zone(ZonedAddressError::DeviceZoneMismatch)));
    }

    #[test_case(None; "removes implicit")]
    #[test_case(Some(FakeDeviceId); "preserves implicit")]
    fn test_connect_disconnect_affects_bound_device(bind_device: Option<FakeDeviceId>) {
        // If a socket is bound to an unzoned address, whether or not it has a
        // bound device should be restored after `connect` then `disconnect`.
        let ll_addr = LinkLocalAddr::new(net_ip_v6!("fe80::1234")).unwrap().into_specified();
        assert!(ll_addr.must_have_zone());

        let local_ip = local_ip::<Ipv6>();
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(
            UdpFakeDeviceCoreCtx::with_local_remote_ip_addrs(vec![local_ip], vec![ll_addr]),
        );
        let mut api = UdpApi::<Ipv6, _>::new(ctx.as_mut());

        let socket = api.create();
        api.set_device(&socket, bind_device.as_ref()).unwrap();

        api.listen(&socket, Some(ZonedAddr::Unzoned(local_ip)), Some(LOCAL_PORT)).unwrap();
        api.connect(
            &socket,
            Some(ZonedAddr::Zoned(AddrAndZone::new(ll_addr, FakeDeviceId).unwrap())),
            REMOTE_PORT.into(),
        )
        .expect("connect should succeed");

        assert_eq!(api.get_bound_device(&socket), Some(FakeWeakDeviceId(FakeDeviceId)));

        api.disconnect(&socket).expect("was connected");

        assert_eq!(api.get_bound_device(&socket), bind_device.map(FakeWeakDeviceId));
    }

    #[test]
    fn test_bind_zoned_addr_connect_disconnect() {
        // If a socket is bound to a zoned address, the address's device should
        // be retained after `connect` then `disconnect`.
        let ll_addr = LinkLocalAddr::new(net_ip_v6!("fe80::1234")).unwrap().into_specified();
        assert!(ll_addr.must_have_zone());

        let remote_ip = remote_ip::<Ipv6>();
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(
            UdpFakeDeviceCoreCtx::with_local_remote_ip_addrs(vec![ll_addr], vec![remote_ip]),
        );

        let mut api = UdpApi::<Ipv6, _>::new(ctx.as_mut());

        let socket = api.create();
        api.listen(
            &socket,
            Some(ZonedAddr::Zoned(AddrAndZone::new(ll_addr, FakeDeviceId).unwrap())),
            Some(LOCAL_PORT),
        )
        .unwrap();
        api.connect(&socket, Some(ZonedAddr::Unzoned(remote_ip)), REMOTE_PORT.into())
            .expect("connect should succeed");

        assert_eq!(api.get_bound_device(&socket), Some(FakeWeakDeviceId(FakeDeviceId)));

        api.disconnect(&socket).expect("was connected");
        assert_eq!(api.get_bound_device(&socket), Some(FakeWeakDeviceId(FakeDeviceId)));
    }

    #[test]
    fn test_bind_device_after_connect_persists_after_disconnect() {
        // If a socket is bound to an unzoned address, connected to a zoned address, and then has
        // its device set, the device should be *retained* after `disconnect`.
        let ll_addr = LinkLocalAddr::new(net_ip_v6!("fe80::1234")).unwrap().into_specified();
        assert!(ll_addr.must_have_zone());

        let local_ip = local_ip::<Ipv6>();
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(
            UdpFakeDeviceCoreCtx::with_local_remote_ip_addrs(vec![local_ip], vec![ll_addr]),
        );
        let mut api = UdpApi::<Ipv6, _>::new(ctx.as_mut());
        let socket = api.create();
        api.listen(&socket, Some(ZonedAddr::Unzoned(local_ip)), Some(LOCAL_PORT)).unwrap();
        api.connect(
            &socket,
            Some(ZonedAddr::Zoned(AddrAndZone::new(ll_addr, FakeDeviceId).unwrap())),
            REMOTE_PORT.into(),
        )
        .expect("connect should succeed");

        assert_eq!(api.get_bound_device(&socket), Some(FakeWeakDeviceId(FakeDeviceId)));

        // This is a no-op functionally since the socket is already bound to the
        // device but it implies that we shouldn't unbind the device on
        // disconnect.
        api.set_device(&socket, Some(&FakeDeviceId)).expect("binding same device should succeed");

        api.disconnect(&socket).expect("was connected");
        assert_eq!(api.get_bound_device(&socket), Some(FakeWeakDeviceId(FakeDeviceId)));
    }

    #[ip_test(I)]
    fn test_remove_udp_unbound<I: TestIpExt>() {
        let mut ctx = UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let unbound = api.create();
        let _: (UdpSocketDiagnosticsSeed<I, _, _>, ()) = api.close(unbound).into_removed();
    }

    #[ip_test(I)]
    fn test_hop_limits_used_for_sending_packets<I: TestIpExt>() {
        let some_multicast_addr: MulticastAddr<I::Addr> = I::map_ip(
            (),
            |()| Ipv4::ALL_SYSTEMS_MULTICAST_ADDRESS,
            |()| MulticastAddr::new(net_ip_v6!("ff0e::1")).unwrap(),
        );

        let mut ctx =
            UdpFakeDeviceCtx::with_core_ctx(UdpFakeDeviceCoreCtx::with_local_remote_ip_addrs(
                vec![local_ip::<I>()],
                vec![remote_ip::<I>(), some_multicast_addr.into_specified()],
            ));
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let listener = api.create();

        const UNICAST_HOPS: NonZeroU8 = NonZeroU8::new(23).unwrap();
        const MULTICAST_HOPS: NonZeroU8 = NonZeroU8::new(98).unwrap();
        api.set_unicast_hop_limit(&listener, Some(UNICAST_HOPS), I::VERSION).unwrap();
        api.set_multicast_hop_limit(&listener, Some(MULTICAST_HOPS), I::VERSION).unwrap();

        api.listen(&listener, None, None).expect("listen failed");

        let mut send_and_get_ttl = |remote_ip| {
            api.send_to(
                &listener,
                Some(ZonedAddr::Unzoned(remote_ip)),
                REMOTE_PORT.into(),
                Buf::new(vec![], ..),
                FakeSendToken::default(),
            )
            .expect("send failed");

            let (meta, _body) = api.core_ctx().bound_sockets.ip_socket_ctx.frames().last().unwrap();
            let SendIpPacketMeta { dst_ip, ttl, .. } = meta.try_as::<I>().unwrap();
            assert_eq!(*dst_ip, remote_ip);
            *ttl
        };

        assert_eq!(send_and_get_ttl(some_multicast_addr.into_specified()), Some(MULTICAST_HOPS));
        assert_eq!(send_and_get_ttl(remote_ip::<I>()), Some(UNICAST_HOPS));
    }

    const DUAL_STACK_ANY_ADDR: Ipv6Addr = net_ip_v6!("::");
    const DUAL_STACK_V4_ANY_ADDR: Ipv6Addr = net_ip_v6!("::FFFF:0.0.0.0");

    #[derive(Copy, Clone, Debug)]
    enum DualStackBindAddr {
        Any,
        V4Any,
        V4Specific,
    }

    impl DualStackBindAddr {
        const fn v6_addr(&self) -> Option<Ipv6Addr> {
            match self {
                Self::Any => Some(DUAL_STACK_ANY_ADDR),
                Self::V4Any => Some(DUAL_STACK_V4_ANY_ADDR),
                Self::V4Specific => None,
            }
        }
    }
    const V4_LOCAL_IP: Ipv4Addr = ip_v4!("192.168.1.10");
    const V4_LOCAL_IP_MAPPED: Ipv6Addr = net_ip_v6!("::ffff:192.168.1.10");
    const V6_LOCAL_IP: Ipv6Addr = net_ip_v6!("2201::1");
    const V6_REMOTE_IP: SpecifiedAddr<Ipv6Addr> =
        unsafe { SpecifiedAddr::new_unchecked(net_ip_v6!("2001:db8::1")) };
    const V4_REMOTE_IP_MAPPED: SpecifiedAddr<Ipv6Addr> =
        unsafe { SpecifiedAddr::new_unchecked(net_ip_v6!("::FFFF:192.0.2.1")) };

    fn get_dual_stack_context<
        'a,
        BC: UdpBindingsTypes + 'a,
        CC: DatagramBoundStateContext<Ipv6, BC, Udp<BC>>,
    >(
        core_ctx: &'a mut CC,
    ) -> &'a mut CC::DualStackContext {
        match core_ctx.dual_stack_context_mut() {
            MaybeDualStack::NotDualStack(_) => unreachable!("UDP is a dual stack enabled protocol"),
            MaybeDualStack::DualStack(ds) => ds,
        }
    }

    #[test_case::test_matrix(
        [DualStackBindAddr::Any, DualStackBindAddr::V4Any, DualStackBindAddr::V4Specific],
        [WithEarlyDemux, NoEarlyDemux]
    )]
    fn dual_stack_delivery(bind_addr: DualStackBindAddr, early_demux_mode: EarlyDemuxMode) {
        const REMOTE_IP: Ipv4Addr = ip_v4!("8.8.8.8");
        const REMOTE_IP_MAPPED: Ipv6Addr = net_ip_v6!("::ffff:8.8.8.8");
        let bind_addr = bind_addr.v6_addr().unwrap_or(V4_LOCAL_IP_MAPPED);
        let mut ctx = FakeUdpCtx::with_core_ctx(FakeUdpCoreCtx::with_local_remote_ip_addrs(
            vec![SpecifiedAddr::new(V4_LOCAL_IP).unwrap()],
            vec![SpecifiedAddr::new(REMOTE_IP).unwrap()],
        ));

        let mut api = UdpApi::<Ipv6, _>::new(ctx.as_mut());
        let listener = api.create();
        api.listen(
            &listener,
            SpecifiedAddr::new(bind_addr).map(|a| ZonedAddr::Unzoned(a)),
            Some(LOCAL_PORT),
        )
        .expect("can bind");

        const BODY: &[u8] = b"abcde";
        let (core_ctx, bindings_ctx) = api.contexts();
        receive_udp_packet(
            core_ctx,
            bindings_ctx,
            FakeDeviceId,
            UdpPacketMeta::<Ipv4> {
                src_ip: REMOTE_IP,
                src_port: Some(REMOTE_PORT),
                dst_ip: V4_LOCAL_IP,
                dst_port: LOCAL_PORT,
                dscp_and_ecn: DscpAndEcn::default(),
            },
            BODY,
            early_demux_mode,
        )
        .expect("receive udp packet should succeed");

        assert_eq!(
            bindings_ctx.state.received::<Ipv6>(),
            &HashMap::from([(
                listener.downgrade(),
                SocketReceived {
                    packets: vec![ReceivedPacket {
                        body: BODY.into(),
                        meta: UdpPacketMeta::<Ipv6> {
                            src_ip: REMOTE_IP_MAPPED,
                            src_port: Some(REMOTE_PORT),
                            dst_ip: V4_LOCAL_IP_MAPPED,
                            dst_port: LOCAL_PORT,
                            dscp_and_ecn: DscpAndEcn::default(),
                        }
                    }],
                    max_size: usize::MAX,
                }
            )])
        );
    }

    #[test_case(DualStackBindAddr::Any, true; "dual-stack any bind v4 first")]
    #[test_case(DualStackBindAddr::V4Any, true; "v4 any bind v4 first")]
    #[test_case(DualStackBindAddr::V4Specific, true; "v4 specific bind v4 first")]
    #[test_case(DualStackBindAddr::Any, false; "dual-stack any bind v4 second")]
    #[test_case(DualStackBindAddr::V4Any, false; "v4 any bind v4 second")]
    #[test_case(DualStackBindAddr::V4Specific, false; "v4 specific bind v4 second")]
    fn dual_stack_bind_conflict(bind_addr: DualStackBindAddr, bind_v4_first: bool) {
        let mut ctx = FakeUdpCtx::with_core_ctx(FakeUdpCoreCtx::with_local_remote_ip_addrs(
            vec![SpecifiedAddr::new(V4_LOCAL_IP).unwrap()],
            vec![],
        ));

        let v4_listener = UdpApi::<Ipv4, _>::new(ctx.as_mut()).create();
        let v6_listener = UdpApi::<Ipv6, _>::new(ctx.as_mut()).create();

        let bind_v4 = |mut api: UdpApi<Ipv4, _>| {
            api.listen(
                &v4_listener,
                SpecifiedAddr::new(V4_LOCAL_IP).map(|a| ZonedAddr::Unzoned(a)),
                Some(LOCAL_PORT),
            )
        };
        let bind_v6 = |mut api: UdpApi<Ipv6, _>| {
            api.listen(
                &v6_listener,
                SpecifiedAddr::new(bind_addr.v6_addr().unwrap_or(V4_LOCAL_IP_MAPPED))
                    .map(ZonedAddr::Unzoned),
                Some(LOCAL_PORT),
            )
        };

        let second_bind_error = if bind_v4_first {
            bind_v4(UdpApi::<Ipv4, _>::new(ctx.as_mut())).expect("no conflict");
            bind_v6(UdpApi::<Ipv6, _>::new(ctx.as_mut())).expect_err("should conflict")
        } else {
            bind_v6(UdpApi::<Ipv6, _>::new(ctx.as_mut())).expect("no conflict");
            bind_v4(UdpApi::<Ipv4, _>::new(ctx.as_mut())).expect_err("should conflict")
        };
        assert_eq!(second_bind_error, Either::Right(LocalAddressError::AddressInUse));
    }

    // Verifies that port availability in both the IPv4 and IPv6 bound socket
    // maps is considered when allocating a local port for a dual-stack UDP
    // socket listening in both stacks.
    #[test_case(IpVersion::V4; "v4_is_constrained")]
    #[test_case(IpVersion::V6; "v6_is_constrained")]
    fn dual_stack_local_port_alloc(ip_version_with_constrained_ports: IpVersion) {
        let mut ctx = FakeUdpCtx::with_core_ctx(FakeUdpCoreCtx::with_local_remote_ip_addrs(
            vec![
                SpecifiedAddr::new(V4_LOCAL_IP.to_ip_addr()).unwrap(),
                SpecifiedAddr::new(V6_LOCAL_IP.to_ip_addr()).unwrap(),
            ],
            vec![],
        ));

        // Specifically selected to be in the `EPHEMERAL_RANGE`.
        const AVAILABLE_PORT: NonZeroU16 = NonZeroU16::new(54321).unwrap();

        // Densely pack the port space for one IP Version.
        for port in 1..=u16::MAX {
            let port = NonZeroU16::new(port).unwrap();
            if port == AVAILABLE_PORT {
                continue;
            }
            match ip_version_with_constrained_ports {
                IpVersion::V4 => {
                    let mut api = UdpApi::<Ipv4, _>::new(ctx.as_mut());
                    let listener = api.create();
                    api.listen(
                        &listener,
                        SpecifiedAddr::new(V4_LOCAL_IP).map(|a| ZonedAddr::Unzoned(a)),
                        Some(port),
                    )
                    .expect("listen v4 should succeed")
                }
                IpVersion::V6 => {
                    let mut api = UdpApi::<Ipv6, _>::new(ctx.as_mut());
                    let listener = api.create();
                    api.listen(
                        &listener,
                        SpecifiedAddr::new(V6_LOCAL_IP).map(|a| ZonedAddr::Unzoned(a)),
                        Some(port),
                    )
                    .expect("listen v6 should succeed")
                }
            }
        }

        // Create a listener on the dualstack any address, expecting it to be
        // allocated `AVAILABLE_PORT`.
        let mut api = UdpApi::<Ipv6, _>::new(ctx.as_mut());
        let listener = api.create();
        api.listen(&listener, None, None).expect("dualstack listen should succeed");
        let port = assert_matches!(api.get_info(&listener), SocketInfo::Listener(info) => info.local_identifier);
        assert_eq!(port, AVAILABLE_PORT);
    }

    #[test_case(DualStackBindAddr::V4Any; "v4 any")]
    #[test_case(DualStackBindAddr::V4Specific; "v4 specific")]
    fn dual_stack_enable(bind_addr: DualStackBindAddr) {
        let mut ctx = FakeUdpCtx::with_core_ctx(FakeUdpCoreCtx::with_local_remote_ip_addrs(
            vec![SpecifiedAddr::new(V4_LOCAL_IP).unwrap()],
            vec![],
        ));
        let mut api = UdpApi::<Ipv6, _>::new(ctx.as_mut());

        let bind_addr = bind_addr.v6_addr().unwrap_or(V4_LOCAL_IP_MAPPED);
        let listener = api.create();

        assert_eq!(api.get_dual_stack_enabled(&listener), Ok(true));
        api.set_dual_stack_enabled(&listener, false).expect("can set dual-stack enabled");

        // With dual-stack behavior disabled, the IPv6 socket can't bind to
        // an IPv4-mapped IPv6 address.
        assert_eq!(
            api.listen(
                &listener,
                SpecifiedAddr::new(bind_addr).map(|a| ZonedAddr::Unzoned(a)),
                Some(LOCAL_PORT),
            ),
            Err(Either::Right(LocalAddressError::CannotBindToAddress))
        );
        api.set_dual_stack_enabled(&listener, true).expect("can set dual-stack enabled");
        // Try again now that dual-stack sockets are enabled.
        assert_eq!(
            api.listen(
                &listener,
                SpecifiedAddr::new(bind_addr).map(|a| ZonedAddr::Unzoned(a)),
                Some(LOCAL_PORT),
            ),
            Ok(())
        );
    }

    #[test]
    fn dual_stack_bind_unassigned_v4_address() {
        const NOT_ASSIGNED_MAPPED: Ipv6Addr = net_ip_v6!("::ffff:8.8.8.8");
        let mut ctx = FakeUdpCtx::with_core_ctx(FakeUdpCoreCtx::with_local_remote_ip_addrs(
            vec![SpecifiedAddr::new(V4_LOCAL_IP).unwrap()],
            vec![],
        ));
        let mut api = UdpApi::<Ipv6, _>::new(ctx.as_mut());

        let listener = api.create();
        assert_eq!(
            api.listen(
                &listener,
                SpecifiedAddr::new(NOT_ASSIGNED_MAPPED).map(|a| ZonedAddr::Unzoned(a)),
                Some(LOCAL_PORT),
            ),
            Err(Either::Right(LocalAddressError::CannotBindToAddress))
        );
    }

    // Calling `connect` on an already bound socket will cause the existing
    // `listener` entry in the bound state map to be upgraded to a `connected`
    // entry. Dual-stack listeners may exist in both the IPv4 and IPv6 bound
    // state maps, so make sure both entries are properly removed.
    #[test]
    fn dual_stack_connect_cleans_up_existing_listener() {
        let mut ctx = FakeUdpCtx::with_core_ctx(FakeUdpCoreCtx::with_local_remote_ip_addrs(
            vec![Ipv6::TEST_ADDRS.local_ip],
            vec![Ipv6::TEST_ADDRS.remote_ip],
        ));

        const DUAL_STACK_ANY_ADDR: Option<ZonedAddr<SpecifiedAddr<Ipv6Addr>, FakeDeviceId>> = None;

        fn assert_listeners(core_ctx: &mut FakeUdpCoreCtx<FakeDeviceId>, expect_present: bool) {
            const V4_LISTENER_ADDR: ListenerAddr<
                ListenerIpAddr<Ipv4Addr, NonZeroU16>,
                FakeWeakDeviceId<FakeDeviceId>,
            > = ListenerAddr {
                ip: ListenerIpAddr { addr: None, identifier: LOCAL_PORT },
                device: None,
            };
            const V6_LISTENER_ADDR: ListenerAddr<
                ListenerIpAddr<Ipv6Addr, NonZeroU16>,
                FakeWeakDeviceId<FakeDeviceId>,
            > = ListenerAddr {
                ip: ListenerIpAddr { addr: None, identifier: LOCAL_PORT },
                device: None,
            };

            DualStackBoundStateContext::with_both_bound_sockets_mut(
                get_dual_stack_context(&mut core_ctx.bound_sockets),
                |_core_ctx, v6_sockets, v4_sockets| {
                    let v4 = v4_sockets.bound_sockets.listeners().get_by_addr(&V4_LISTENER_ADDR);
                    let v6 = v6_sockets.bound_sockets.listeners().get_by_addr(&V6_LISTENER_ADDR);
                    if expect_present {
                        assert_matches!(v4, Some(_));
                        assert_matches!(v6, Some(_));
                    } else {
                        assert_matches!(v4, None);
                        assert_matches!(v6, None);
                    }
                },
            );
        }

        // Create a socket and listen on the IPv6 any address. Verify we have
        // listener state for both IPv4 and IPv6.
        let mut api = UdpApi::<Ipv6, _>::new(ctx.as_mut());
        let socket = api.create();
        assert_eq!(api.listen(&socket, DUAL_STACK_ANY_ADDR, Some(LOCAL_PORT)), Ok(()));
        assert_listeners(api.core_ctx(), true);

        // Connect the socket to a remote V6 address and verify that both
        // the IPv4 and IPv6 listener state has been removed.
        assert_eq!(
            api.connect(
                &socket,
                Some(ZonedAddr::Unzoned(Ipv6::TEST_ADDRS.remote_ip)),
                REMOTE_PORT.into(),
            ),
            Ok(())
        );
        assert_matches!(api.get_info(&socket), SocketInfo::Connected(_));
        assert_listeners(api.core_ctx(), false);
    }

    #[test_case(net_ip_v6!("::"), true; "dual stack any")]
    #[test_case(net_ip_v6!("::"), false; "v6 any")]
    #[test_case(net_ip_v6!("::ffff:0.0.0.0"), true; "v4 unspecified")]
    #[test_case(V4_LOCAL_IP_MAPPED, true; "v4 specified")]
    #[test_case(V6_LOCAL_IP, true; "v6 specified dual stack enabled")]
    #[test_case(V6_LOCAL_IP, false; "v6 specified dual stack disabled")]
    fn dual_stack_get_info(bind_addr: Ipv6Addr, enable_dual_stack: bool) {
        let mut ctx = FakeUdpCtx::with_core_ctx(FakeUdpCoreCtx::with_local_remote_ip_addrs::<
            SpecifiedAddr<IpAddr>,
        >(
            vec![
                SpecifiedAddr::new(V4_LOCAL_IP).unwrap().into(),
                SpecifiedAddr::new(V6_LOCAL_IP).unwrap().into(),
            ],
            vec![],
        ));
        let mut api = UdpApi::<Ipv6, _>::new(ctx.as_mut());

        let listener = api.create();
        api.set_dual_stack_enabled(&listener, enable_dual_stack)
            .expect("can set dual-stack enabled");
        let bind_addr = SpecifiedAddr::new(bind_addr);
        assert_eq!(
            api.listen(&listener, bind_addr.map(|a| ZonedAddr::Unzoned(a)), Some(LOCAL_PORT),),
            Ok(())
        );

        assert_eq!(
            api.get_info(&listener),
            SocketInfo::Listener(datagram::ListenerInfo {
                local_ip: bind_addr.map(StrictlyZonedAddr::new_unzoned_or_panic),
                local_identifier: LOCAL_PORT,
            })
        );
    }

    #[test_case(net_ip_v6!("::"), true; "dual stack any")]
    #[test_case(net_ip_v6!("::"), false; "v6 any")]
    #[test_case(net_ip_v6!("::ffff:0.0.0.0"), true; "v4 unspecified")]
    #[test_case(V4_LOCAL_IP_MAPPED, true; "v4 specified")]
    #[test_case(V6_LOCAL_IP, true; "v6 specified dual stack enabled")]
    #[test_case(V6_LOCAL_IP, false; "v6 specified dual stack disabled")]
    fn dual_stack_remove_listener(bind_addr: Ipv6Addr, enable_dual_stack: bool) {
        // Ensure that when a socket is removed, it doesn't leave behind state
        // in the demultiplexing maps. Do this by binding a new socket at the
        // same address and asserting success.
        let mut ctx = FakeUdpCtx::with_core_ctx(FakeUdpCoreCtx::with_local_remote_ip_addrs::<
            SpecifiedAddr<IpAddr>,
        >(
            vec![
                SpecifiedAddr::new(V4_LOCAL_IP).unwrap().into(),
                SpecifiedAddr::new(V6_LOCAL_IP).unwrap().into(),
            ],
            vec![],
        ));
        let mut api = UdpApi::<Ipv6, _>::new(ctx.as_mut());

        let mut bind_listener = || {
            let listener = api.create();
            api.set_dual_stack_enabled(&listener, enable_dual_stack)
                .expect("can set dual-stack enabled");
            let bind_addr = SpecifiedAddr::new(bind_addr);
            assert_eq!(
                api.listen(&listener, bind_addr.map(|a| ZonedAddr::Unzoned(a)), Some(LOCAL_PORT)),
                Ok(())
            );

            let _: (UdpSocketDiagnosticsSeed<Ipv6, _, _>, ()) = api.close(listener).into_removed();
        };

        // The first time should succeed because the state is empty.
        bind_listener();
        // The second time should succeed because the first removal didn't
        // leave any state behind.
        bind_listener();
    }

    #[test_case(V6_REMOTE_IP, true; "This stack with dualstack enabled")]
    #[test_case(V6_REMOTE_IP, false; "This stack with dualstack disabled")]
    #[test_case(V4_REMOTE_IP_MAPPED, true; "other stack with dualstack enabled")]
    fn dualstack_remove_connected(remote_ip: SpecifiedAddr<Ipv6Addr>, enable_dual_stack: bool) {
        // Ensure that when a socket is removed, it doesn't leave behind state
        // in the demultiplexing maps. Do this by binding a new socket at the
        // same address and asserting success.
        let mut ctx = datagram::testutil::setup_fake_ctx_with_dualstack_conn_addrs(
            Ipv6::UNSPECIFIED_ADDRESS.to_ip_addr(),
            remote_ip.into(),
            [FakeDeviceId {}],
            |device_configs| {
                FakeUdpCoreCtx::with_ip_socket_ctx_state(FakeDualStackIpSocketCtx::new(
                    device_configs,
                ))
            },
        );
        let mut api = UdpApi::<Ipv6, _>::new(ctx.as_mut());

        let mut bind_connected = || {
            let socket = api.create();
            api.set_dual_stack_enabled(&socket, enable_dual_stack)
                .expect("can set dual-stack enabled");
            assert_eq!(
                api.connect(&socket, Some(ZonedAddr::Unzoned(remote_ip)), REMOTE_PORT.into(),),
                Ok(())
            );

            let _: (UdpSocketDiagnosticsSeed<Ipv6, _, _>, ()) = api.close(socket).into_removed();
        };

        // The first time should succeed because the state is empty.
        bind_connected();
        // The second time should succeed because the first removal didn't
        // leave any state behind.
        bind_connected();
    }

    #[test_case(false, V6_REMOTE_IP, Ok(());
        "connect to this stack with dualstack disabled")]
    #[test_case(true, V6_REMOTE_IP, Ok(());
        "connect to this stack with dualstack enabled")]
    #[test_case(false, V4_REMOTE_IP_MAPPED, Err(ConnectError::RemoteUnexpectedlyMapped);
        "connect to other stack with dualstack disabled")]
    #[test_case(true, V4_REMOTE_IP_MAPPED, Ok(());
        "connect to other stack with dualstack enabled")]
    fn dualstack_connect_unbound(
        enable_dual_stack: bool,
        remote_ip: SpecifiedAddr<Ipv6Addr>,
        expected_outcome: Result<(), ConnectError>,
    ) {
        let mut ctx = datagram::testutil::setup_fake_ctx_with_dualstack_conn_addrs(
            Ipv6::UNSPECIFIED_ADDRESS.to_ip_addr(),
            remote_ip.into(),
            [FakeDeviceId {}],
            |device_configs| {
                FakeUdpCoreCtx::with_ip_socket_ctx_state(FakeDualStackIpSocketCtx::new(
                    device_configs,
                ))
            },
        );
        let mut api = UdpApi::<Ipv6, _>::new(ctx.as_mut());

        let socket = api.create();

        api.set_dual_stack_enabled(&socket, enable_dual_stack).expect("can set dual-stack enabled");

        assert_eq!(
            api.connect(&socket, Some(ZonedAddr::Unzoned(remote_ip)), REMOTE_PORT.into()),
            expected_outcome
        );

        if expected_outcome.is_ok() {
            assert_matches!(
                api.get_info(&socket),
                SocketInfo::Connected(datagram::ConnInfo{
                    local_ip: _,
                    local_identifier: _,
                    remote_ip: found_remote_ip,
                    remote_identifier: found_remote_port,
                }) if found_remote_ip.addr() == remote_ip &&
                    found_remote_port == u16::from(REMOTE_PORT)
            );
            // Disconnect the socket, returning it to the original state.
            assert_eq!(api.disconnect(&socket), Ok(()));
        }

        // Verify the original state is preserved.
        assert_eq!(api.get_info(&socket), SocketInfo::Unbound);
    }

    #[test_case(V6_LOCAL_IP, V6_REMOTE_IP, Ok(());
        "listener in this stack connected in this stack")]
    #[test_case(V6_LOCAL_IP, V4_REMOTE_IP_MAPPED, Err(ConnectError::RemoteUnexpectedlyMapped);
        "listener in this stack connected in other stack")]
    #[test_case(Ipv6::UNSPECIFIED_ADDRESS, V6_REMOTE_IP, Ok(());
        "listener in both stacks connected in this stack")]
    #[test_case(Ipv6::UNSPECIFIED_ADDRESS, V4_REMOTE_IP_MAPPED, Ok(());
        "listener in both stacks connected in other stack")]
    #[test_case(V4_LOCAL_IP_MAPPED, V6_REMOTE_IP,
        Err(ConnectError::RemoteUnexpectedlyNonMapped);
        "listener in other stack connected in this stack")]
    #[test_case(V4_LOCAL_IP_MAPPED, V4_REMOTE_IP_MAPPED, Ok(());
        "listener in other stack connected in other stack")]
    fn dualstack_connect_listener(
        local_ip: Ipv6Addr,
        remote_ip: SpecifiedAddr<Ipv6Addr>,
        expected_outcome: Result<(), ConnectError>,
    ) {
        let mut ctx = datagram::testutil::setup_fake_ctx_with_dualstack_conn_addrs(
            local_ip.to_ip_addr(),
            remote_ip.into(),
            [FakeDeviceId {}],
            |device_configs| {
                FakeUdpCoreCtx::with_ip_socket_ctx_state(FakeDualStackIpSocketCtx::new(
                    device_configs,
                ))
            },
        );
        let mut api = UdpApi::<Ipv6, _>::new(ctx.as_mut());
        let socket = api.create();

        assert_eq!(
            api.listen(
                &socket,
                SpecifiedAddr::new(local_ip).map(|local_ip| ZonedAddr::Unzoned(local_ip)),
                Some(LOCAL_PORT),
            ),
            Ok(())
        );

        assert_eq!(
            api.connect(&socket, Some(ZonedAddr::Unzoned(remote_ip)), REMOTE_PORT.into()),
            expected_outcome
        );

        if expected_outcome.is_ok() {
            assert_matches!(
                api.get_info(&socket),
                SocketInfo::Connected(datagram::ConnInfo{
                    local_ip: _,
                    local_identifier: _,
                    remote_ip: found_remote_ip,
                    remote_identifier: found_remote_port,
                }) if found_remote_ip.addr() == remote_ip &&
                    found_remote_port == u16::from(REMOTE_PORT)
            );
            // Disconnect the socket, returning it to the original state.
            assert_eq!(api.disconnect(&socket), Ok(()));
        }

        // Verify the original state is preserved.
        assert_matches!(
            api.get_info(&socket),
            SocketInfo::Listener(datagram::ListenerInfo {
                local_ip: found_local_ip,
                local_identifier: found_local_port,
            }) if found_local_port == LOCAL_PORT &&
                local_ip == found_local_ip.map(
                    |a| a.addr().get()
                ).unwrap_or(Ipv6::UNSPECIFIED_ADDRESS)
        );
    }

    #[test_case(V6_REMOTE_IP, V6_REMOTE_IP, Ok(());
        "connected in this stack reconnected in this stack")]
    #[test_case(V6_REMOTE_IP, V4_REMOTE_IP_MAPPED, Err(ConnectError::RemoteUnexpectedlyMapped);
        "connected in this stack reconnected in other stack")]
    #[test_case(V4_REMOTE_IP_MAPPED, V6_REMOTE_IP,
        Err(ConnectError::RemoteUnexpectedlyNonMapped);
        "connected in other stack reconnected in this stack")]
    #[test_case(V4_REMOTE_IP_MAPPED, V4_REMOTE_IP_MAPPED, Ok(());
        "connected in other stack reconnected in other stack")]
    fn dualstack_connect_connected(
        original_remote_ip: SpecifiedAddr<Ipv6Addr>,
        new_remote_ip: SpecifiedAddr<Ipv6Addr>,
        expected_outcome: Result<(), ConnectError>,
    ) {
        let mut ctx = datagram::testutil::setup_fake_ctx_with_dualstack_conn_addrs(
            Ipv6::UNSPECIFIED_ADDRESS.to_ip_addr(),
            original_remote_ip.into(),
            [FakeDeviceId {}],
            |device_configs| {
                FakeUdpCoreCtx::with_ip_socket_ctx_state(FakeDualStackIpSocketCtx::new(
                    device_configs,
                ))
            },
        );

        let mut api = UdpApi::<Ipv6, _>::new(ctx.as_mut());
        let socket = api.create();

        assert_eq!(
            api.connect(&socket, Some(ZonedAddr::Unzoned(original_remote_ip)), REMOTE_PORT.into(),),
            Ok(())
        );

        assert_eq!(
            api.connect(
                &socket,
                Some(ZonedAddr::Unzoned(new_remote_ip)),
                OTHER_REMOTE_PORT.into(),
            ),
            expected_outcome
        );

        let (expected_remote_ip, expected_remote_port) = if expected_outcome.is_ok() {
            (new_remote_ip, OTHER_REMOTE_PORT)
        } else {
            // Verify the original state is preserved.
            (original_remote_ip, REMOTE_PORT)
        };
        assert_matches!(
            api.get_info(&socket),
            SocketInfo::Connected(datagram::ConnInfo{
                local_ip: _,
                local_identifier: _,
                remote_ip: found_remote_ip,
                remote_identifier: found_remote_port,
            }) if found_remote_ip.addr() == expected_remote_ip &&
                found_remote_port == u16::from(expected_remote_port)
        );

        // Disconnect the socket and verify it returns to unbound state.
        assert_eq!(api.disconnect(&socket), Ok(()));
        assert_eq!(api.get_info(&socket), SocketInfo::Unbound);
    }

    type FakeBoundSocketMap<I> =
        UdpBoundSocketMap<I, FakeWeakDeviceId<FakeDeviceId>, FakeUdpBindingsCtx<FakeDeviceId>>;
    type FakePortAlloc<'a, I> =
        UdpPortAlloc<'a, I, FakeWeakDeviceId<FakeDeviceId>, FakeUdpBindingsCtx<FakeDeviceId>>;

    fn listen<I: IpExt>(
        ip: I::Addr,
        port: u16,
    ) -> AddrVec<I, FakeWeakDeviceId<FakeDeviceId>, UdpAddrSpec> {
        let addr = SpecifiedAddr::new(ip).map(|a| SocketIpAddr::try_from(a).unwrap());
        let port = NonZeroU16::new(port).expect("port must be nonzero");
        AddrVec::Listen(ListenerAddr {
            ip: ListenerIpAddr { addr, identifier: port },
            device: None,
        })
    }

    fn listen_device<I: IpExt>(
        ip: I::Addr,
        port: u16,
        device: FakeWeakDeviceId<FakeDeviceId>,
    ) -> AddrVec<I, FakeWeakDeviceId<FakeDeviceId>, UdpAddrSpec> {
        let addr = SpecifiedAddr::new(ip).map(|a| SocketIpAddr::try_from(a).unwrap());
        let port = NonZeroU16::new(port).expect("port must be nonzero");
        AddrVec::Listen(ListenerAddr {
            ip: ListenerIpAddr { addr, identifier: port },
            device: Some(device),
        })
    }

    fn conn<I: IpExt>(
        local_ip: I::Addr,
        local_port: u16,
        remote_ip: I::Addr,
        remote_port: u16,
    ) -> AddrVec<I, FakeWeakDeviceId<FakeDeviceId>, UdpAddrSpec> {
        let local_ip = SocketIpAddr::new(local_ip).expect("addr must be specified & non-mapped");
        let local_port = NonZeroU16::new(local_port).expect("port must be nonzero");
        let remote_ip = SocketIpAddr::new(remote_ip).expect("addr must be specified & non-mapped");
        let remote_port = NonZeroU16::new(remote_port).expect("port must be nonzero").into();
        AddrVec::Conn(ConnAddr {
            ip: ConnIpAddr { local: (local_ip, local_port), remote: (remote_ip, remote_port) },
            device: None,
        })
    }

    const SHARING_DOMAIN1: SharingDomain = SharingDomain::new(1);
    const SHARING_DOMAIN2: SharingDomain = SharingDomain::new(42);
    const EXCLUSIVE: Sharing = Sharing { reuse_addr: false, reuse_port: ReusePortOption::Disabled };
    const REUSE_ADDR: Sharing = Sharing { reuse_addr: true, reuse_port: ReusePortOption::Disabled };
    const REUSE_PORT: Sharing =
        Sharing { reuse_addr: false, reuse_port: ReusePortOption::Enabled(SHARING_DOMAIN1) };
    const REUSE_ADDR_PORT: Sharing =
        Sharing { reuse_addr: true, reuse_port: ReusePortOption::Enabled(SHARING_DOMAIN1) };
    const REUSE_PORT2: Sharing =
        Sharing { reuse_addr: false, reuse_port: ReusePortOption::Enabled(SHARING_DOMAIN2) };
    const REUSE_ADDR_PORT2: Sharing =
        Sharing { reuse_addr: true, reuse_port: ReusePortOption::Enabled(SHARING_DOMAIN2) };

    #[test_case([
        (listen(ip_v4!("0.0.0.0"), 1), EXCLUSIVE),
        (listen(ip_v4!("0.0.0.0"), 2), EXCLUSIVE)],
            Ok(()); "listen_any_ip_different_port")]
    #[test_case([
        (listen(ip_v4!("0.0.0.0"), 1), EXCLUSIVE),
        (listen(ip_v4!("0.0.0.0"), 1), EXCLUSIVE)],
            Err(InsertError::Exists); "any_ip_same_port")]
    #[test_case([
        (listen(ip_v4!("1.1.1.1"), 1), EXCLUSIVE),
        (listen(ip_v4!("1.1.1.1"), 1), EXCLUSIVE)],
            Err(InsertError::Exists); "listen_same_specific_ip")]
    #[test_case([
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_ADDR),
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_ADDR)],
            Ok(()); "listen_same_specific_ip_reuse_addr")]
    #[test_case([
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_PORT),
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_PORT)],
            Ok(()); "listen_same_specific_ip_reuse_port")]
    #[test_case([
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_ADDR_PORT),
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_ADDR)],
            Ok(()); "listen_same_specific_ip_reuse_addr_port_and_reuse_addr")]
    #[test_case([
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_ADDR),
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_ADDR_PORT)],
            Ok(()); "listen_same_specific_ip_reuse_addr_and_reuse_addr_port")]
    #[test_case([
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_ADDR_PORT),
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_PORT)],
            Ok(()); "listen_same_specific_ip_reuse_addr_port_and_reuse_port")]
    #[test_case([
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_PORT),
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_ADDR_PORT)],
            Ok(()); "listen_same_specific_ip_reuse_port_and_reuse_addr_port")]
    #[test_case([
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_ADDR_PORT),
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_ADDR_PORT)],
            Ok(()); "listen_same_specific_ip_reuse_addr_port_and_reuse_addr_port")]
    #[test_case([
        (listen(ip_v4!("1.1.1.1"), 1), EXCLUSIVE),
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_ADDR)],
            Err(InsertError::Exists); "listen_same_specific_ip_exclusive_reuse_addr")]
    #[test_case([
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_ADDR),
        (listen(ip_v4!("1.1.1.1"), 1), EXCLUSIVE)],
            Err(InsertError::Exists); "listen_same_specific_ip_reuse_addr_exclusive")]
    #[test_case([
        (listen(ip_v4!("1.1.1.1"), 1), EXCLUSIVE),
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_PORT)],
            Err(InsertError::Exists); "listen_same_specific_ip_exclusive_reuse_port")]
    #[test_case([
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_PORT),
        (listen(ip_v4!("1.1.1.1"), 1), EXCLUSIVE)],
            Err(InsertError::Exists); "listen_same_specific_ip_reuse_port_exclusive")]
    #[test_case([
        (listen(ip_v4!("1.1.1.1"), 1), EXCLUSIVE),
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_ADDR_PORT)],
            Err(InsertError::Exists); "listen_same_specific_ip_exclusive_reuse_addr_port")]
    #[test_case([
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_ADDR_PORT),
        (listen(ip_v4!("1.1.1.1"), 1), EXCLUSIVE)],
            Err(InsertError::Exists); "listen_same_specific_ip_reuse_addr_port_exclusive")]
    #[test_case([
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_PORT),
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_ADDR)],
            Err(InsertError::Exists); "listen_same_specific_ip_reuse_port_reuse_addr")]
    #[test_case([
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_ADDR),
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_PORT)],
            Err(InsertError::Exists); "listen_same_specific_ip_reuse_addr_reuse_port")]
    #[test_case([
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_ADDR_PORT),
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_PORT),
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_ADDR),],
            Err(InsertError::Exists); "listen_same_specific_ip_reuse_addr_port_and_reuse_port_and_reuse_addr")]
    #[test_case([
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_ADDR_PORT),
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_ADDR),
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_PORT),],
            Err(InsertError::Exists); "listen_same_specific_ip_reuse_addr_port_and_reuse_addr_and_reuse_port")]
    #[test_case([
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_PORT),
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_PORT2)],
            Err(InsertError::Exists); "listen_same_specific_ip_reuse_port_and_reuse_port2")]
    #[test_case([
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_ADDR_PORT),
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_ADDR_PORT2)],
            Ok(()); "listen_same_specific_ip_reuse_addr_port_and_reuse_addr_port2")]
    #[test_case([
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_ADDR_PORT),
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_ADDR_PORT2),
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_PORT)],
            Err(InsertError::Exists); "listen_same_specific_ip_reuse_addr_port_and_reuse_addr_port2_and_reuse_port")]
    #[test_case([
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_PORT),
        (conn(ip_v4!("1.1.1.1"), 1, ip_v4!("2.2.2.2"), 2), REUSE_PORT)],
            Ok(()); "conn_shadows_listener_reuse_port")]
    #[test_case([
        (listen(ip_v4!("1.1.1.1"), 1), EXCLUSIVE),
        (conn(ip_v4!("1.1.1.1"), 1, ip_v4!("2.2.2.2"), 2), EXCLUSIVE)],
            Err(InsertError::ShadowAddrExists); "conn_shadows_listener_exclusive")]
    #[test_case([
        (listen(ip_v4!("1.1.1.1"), 1), EXCLUSIVE),
        (conn(ip_v4!("1.1.1.1"), 1, ip_v4!("2.2.2.2"), 2), REUSE_PORT)],
            Err(InsertError::ShadowAddrExists); "conn_shadows_listener_exclusive_reuse_port")]
    #[test_case([
        (listen(ip_v4!("1.1.1.1"), 1), REUSE_PORT),
        (conn(ip_v4!("1.1.1.1"), 1, ip_v4!("2.2.2.2"), 2), EXCLUSIVE)],
            Err(InsertError::ShadowAddrExists); "conn_shadows_listener_reuse_port_exclusive")]
    #[test_case([
        (listen_device(ip_v4!("1.1.1.1"), 1, FakeWeakDeviceId(FakeDeviceId)), EXCLUSIVE),
        (conn(ip_v4!("1.1.1.1"), 1, ip_v4!("2.2.2.2"), 2), EXCLUSIVE)],
            Err(InsertError::IndirectConflict); "conn_indirect_conflict_specific_listener")]
    #[test_case([
        (listen_device(ip_v4!("0.0.0.0"), 1, FakeWeakDeviceId(FakeDeviceId)), EXCLUSIVE),
        (conn(ip_v4!("1.1.1.1"), 1, ip_v4!("2.2.2.2"), 2), EXCLUSIVE)],
            Err(InsertError::IndirectConflict); "conn_indirect_conflict_any_listener")]
    #[test_case([
        (conn(ip_v4!("1.1.1.1"), 1, ip_v4!("2.2.2.2"), 2), EXCLUSIVE),
        (listen_device(ip_v4!("1.1.1.1"), 1, FakeWeakDeviceId(FakeDeviceId)), EXCLUSIVE)],
            Err(InsertError::IndirectConflict); "specific_listener_indirect_conflict_conn")]
    #[test_case([
        (conn(ip_v4!("1.1.1.1"), 1, ip_v4!("2.2.2.2"), 2), EXCLUSIVE),
        (listen_device(ip_v4!("0.0.0.0"), 1, FakeWeakDeviceId(FakeDeviceId)), EXCLUSIVE)],
            Err(InsertError::IndirectConflict); "any_listener_indirect_conflict_conn")]
    fn bind_sequence<
        C: IntoIterator<Item = (AddrVec<Ipv4, FakeWeakDeviceId<FakeDeviceId>, UdpAddrSpec>, Sharing)>,
    >(
        spec: C,
        expected: Result<(), InsertError>,
    ) {
        let mut primary_ids = Vec::new();

        let mut create_socket = || {
            let primary =
                datagram::testutil::create_primary_id((), Default::default(), &Default::default());
            let id = UdpSocketId(PrimaryRc::clone_strong(&primary));
            primary_ids.push(primary);
            id
        };

        let mut map = FakeBoundSocketMap::<Ipv4>::default();
        let mut spec = spec.into_iter().peekable();
        let mut try_insert = |(addr, options)| match addr {
            AddrVec::Conn(c) => map
                .conns_mut()
                .try_insert(c, options, EitherIpSocket::V4(create_socket()))
                .map(|_| ()),
            AddrVec::Listen(l) => map
                .listeners_mut()
                .try_insert(l, options, EitherIpSocket::V4(create_socket()))
                .map(|_| ()),
        };
        let last = loop {
            let one_spec = spec.next().expect("empty list of test cases");
            if spec.peek().is_none() {
                break one_spec;
            } else {
                try_insert(one_spec).expect("intermediate bind failed")
            }
        };

        let result = try_insert(last);
        assert_eq!(result, expected);
    }

    #[test_case([
            (listen(ip_v4!("1.1.1.1"), 1), EXCLUSIVE),
            (listen(ip_v4!("2.2.2.2"), 2), EXCLUSIVE),
        ]; "distinct")]
    #[test_case([
            (listen(ip_v4!("1.1.1.1"), 1), REUSE_PORT),
            (listen(ip_v4!("1.1.1.1"), 1), REUSE_PORT),
        ]; "listen_reuse_port")]
    #[test_case([
            (conn(ip_v4!("1.1.1.1"), 1, ip_v4!("2.2.2.2"), 3), REUSE_PORT),
            (conn(ip_v4!("1.1.1.1"), 1, ip_v4!("2.2.2.2"), 3), REUSE_PORT),
        ]; "conn_reuse_port")]
    fn remove_sequence<I>(spec: I)
    where
        I: IntoIterator<
            Item = (AddrVec<Ipv4, FakeWeakDeviceId<FakeDeviceId>, UdpAddrSpec>, Sharing),
        >,
        I::IntoIter: ExactSizeIterator,
    {
        enum Socket<I: IpExt, D: WeakDeviceIdentifier, BT: UdpBindingsTypes, LI, RI> {
            Listener(UdpSocketId<I, D, BT>, ListenerAddr<ListenerIpAddr<I::Addr, LI>, D>),
            Conn(UdpSocketId<I, D, BT>, ConnAddr<ConnIpAddr<I::Addr, LI, RI>, D>),
        }
        let spec = spec.into_iter();
        let spec_len = spec.len();

        let mut primary_ids = Vec::new();

        let mut create_socket = || {
            let primary =
                datagram::testutil::create_primary_id((), Default::default(), &Default::default());
            let id = UdpSocketId(PrimaryRc::clone_strong(&primary));
            primary_ids.push(primary);
            id
        };

        for spec in spec.permutations(spec_len) {
            let mut map = FakeBoundSocketMap::<Ipv4>::default();
            let sockets = spec
                .into_iter()
                .map(|(addr, options)| match addr {
                    AddrVec::Conn(c) => map
                        .conns_mut()
                        .try_insert(c, options, EitherIpSocket::V4(create_socket()))
                        .map(|entry| {
                            Socket::Conn(
                                assert_matches!(entry.id(), EitherIpSocket::V4(id) => id.clone()),
                                entry.get_addr().clone(),
                            )
                        })
                        .expect("insert_failed"),
                    AddrVec::Listen(l) => map
                        .listeners_mut()
                        .try_insert(l, options, EitherIpSocket::V4(create_socket()))
                        .map(|entry| {
                            Socket::Listener(
                                assert_matches!(entry.id(), EitherIpSocket::V4(id) => id.clone()),
                                entry.get_addr().clone(),
                            )
                        })
                        .expect("insert_failed"),
                })
                .collect::<Vec<_>>();

            for socket in sockets {
                match socket {
                    Socket::Listener(l, addr) => {
                        assert_matches!(
                            map.listeners_mut().remove(&EitherIpSocket::V4(l), &addr),
                            Ok(())
                        );
                    }
                    Socket::Conn(c, addr) => {
                        assert_matches!(
                            map.conns_mut().remove(&EitherIpSocket::V4(c), &addr),
                            Ok(())
                        );
                    }
                }
            }
        }
    }

    enum OriginalSocketState {
        Unbound,
        Listener,
        Connected,
    }

    impl OriginalSocketState {
        fn create_socket<I, C>(&self, api: &mut UdpApi<I, C>) -> UdpApiSocketId<I, C>
        where
            I: TestIpExt,
            C: ContextPair,
            C::CoreContext: StateContext<I, C::BindingsContext>
                + UdpCounterContext<
                    I,
                    <C::CoreContext as DeviceIdContext<AnyDevice>>::WeakDeviceId,
                    C::BindingsContext,
                >,
            C::BindingsContext:
                UdpBindingsContext<I, <C::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId>,
            <C::BindingsContext as UdpBindingsTypes>::ExternalData<I>: Default,
            <C::BindingsContext as UdpBindingsTypes>::SocketWritableListener: Default,
            <C::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId:
                netstack3_base::InterfaceProperties<
                        <C::BindingsContext as MatcherBindingsTypes>::DeviceClass,
                    >,
        {
            let socket = api.create();
            match self {
                OriginalSocketState::Unbound => {}
                OriginalSocketState::Listener => {
                    api.listen(
                        &socket,
                        Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)),
                        Some(LOCAL_PORT),
                    )
                    .expect("listen should succeed");
                }
                OriginalSocketState::Connected => {
                    api.connect(
                        &socket,
                        Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)),
                        UdpRemotePort::Set(REMOTE_PORT),
                    )
                    .expect("connect should succeed");
                }
            }
            socket
        }
    }

    #[test_case(OriginalSocketState::Unbound; "unbound")]
    #[test_case(OriginalSocketState::Listener; "listener")]
    #[test_case(OriginalSocketState::Connected; "connected")]
    fn set_get_dual_stack_enabled_v4(original_state: OriginalSocketState) {
        let mut ctx = FakeUdpCtx::with_core_ctx(FakeUdpCoreCtx::with_local_remote_ip_addrs(
            vec![Ipv4::TEST_ADDRS.local_ip],
            vec![Ipv4::TEST_ADDRS.remote_ip],
        ));
        let mut api = UdpApi::<Ipv4, _>::new(ctx.as_mut());
        let socket = original_state.create_socket(&mut api);

        for enabled in [true, false] {
            assert_eq!(
                api.set_dual_stack_enabled(&socket, enabled),
                Err(NotDualStackCapableError.into())
            );
            assert_eq!(api.get_dual_stack_enabled(&socket), Err(NotDualStackCapableError));
        }
    }

    #[test_case(OriginalSocketState::Unbound, Ok(()); "unbound")]
    #[test_case(OriginalSocketState::Listener, Err(SetDualStackEnabledError::SocketIsBound);
        "listener")]
    #[test_case(OriginalSocketState::Connected, Err(SetDualStackEnabledError::SocketIsBound);
        "connected")]
    fn set_get_dual_stack_enabled_v6(
        original_state: OriginalSocketState,
        expected_result: Result<(), SetDualStackEnabledError>,
    ) {
        let mut ctx = FakeUdpCtx::with_core_ctx(FakeUdpCoreCtx::with_local_remote_ip_addrs(
            vec![Ipv6::TEST_ADDRS.local_ip],
            vec![Ipv6::TEST_ADDRS.remote_ip],
        ));
        let mut api = UdpApi::<Ipv6, _>::new(ctx.as_mut());
        let socket = original_state.create_socket(&mut api);

        // Expect dual stack to be enabled by default.
        const ORIGINALLY_ENABLED: bool = true;
        assert_eq!(api.get_dual_stack_enabled(&socket), Ok(ORIGINALLY_ENABLED));

        for enabled in [false, true] {
            assert_eq!(api.set_dual_stack_enabled(&socket, enabled), expected_result);
            let expect_enabled = match expected_result {
                Ok(_) => enabled,
                // If the set was unsuccessful expect the state to be unchanged.
                Err(_) => ORIGINALLY_ENABLED,
            };
            assert_eq!(api.get_dual_stack_enabled(&socket), Ok(expect_enabled));
        }
    }

    #[ip_test(I, test = false)]
    #[test_case::test_matrix(
        [MarkDomain::Mark1, MarkDomain::Mark2],
        [None, Some(0), Some(1)]
    )]
    fn udp_socket_marks<I: TestIpExt>(domain: MarkDomain, mark: Option<u32>) {
        let mut ctx = FakeUdpCtx::with_core_ctx(FakeUdpCoreCtx::with_local_remote_ip_addrs(
            vec![I::TEST_ADDRS.local_ip],
            vec![I::TEST_ADDRS.remote_ip],
        ));
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());
        let socket = api.create();

        // Doesn't have a mark by default.
        assert_eq!(api.get_mark(&socket, domain), Mark(None));

        let mark = Mark(mark);
        // We can set and get back the mark.
        api.set_mark(&socket, domain, mark);
        assert_eq!(api.get_mark(&socket, domain), mark);
    }

    #[ip_test(I)]
    fn udp_early_demux<I: TestIpExt>() {
        set_logger_for_test();
        let mut ctx = FakeUdpCtx::with_core_ctx(FakeUdpCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());

        let local_ip = local_ip::<I>();
        let remote_ip = remote_ip::<I>();
        let socket = api.create();

        api.listen(&socket, Some(ZonedAddr::Unzoned(local_ip)), Some(LOCAL_PORT))
            .expect("Initial call to listen_udp was expected to succeed");

        let builder =
            UdpPacketBuilder::new(remote_ip.get(), local_ip.get(), Some(REMOTE_PORT), LOCAL_PORT);

        let buffer = builder
            .wrap_body(Buf::new(vec![1, 2, 3, 4], ..))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .into_inner();

        // Early demux should find only connected sockets.
        let early_demux_socket =
            <UdpIpTransportContext as IpTransportContext<I, _, _>>::early_demux(
                api.core_ctx(),
                &FakeDeviceId,
                remote_ip.get(),
                local_ip.get(),
                buffer.as_ref(),
            );
        assert_eq!(early_demux_socket, None);

        api.connect(&socket, Some(ZonedAddr::Unzoned(remote_ip)), REMOTE_PORT.into())
            .expect("connect should succeed");

        let early_demux_socket =
            <UdpIpTransportContext as IpTransportContext<I, _, _>>::early_demux(
                api.core_ctx(),
                &FakeDeviceId,
                remote_ip.get(),
                local_ip.get(),
                buffer.as_ref(),
            );
        assert_matches!(early_demux_socket, Some(_));
    }

    fn so_error_inner<I: TestIpExt>(
        icmp_err: I::ErrorCode,
        expected_err: Option<PendingDatagramSocketError>,
    ) {
        set_logger_for_test();

        let mut ctx = FakeUdpCtx::with_core_ctx(FakeUdpCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());

        let local_ip = local_ip::<I>();
        let remote_ip = remote_ip::<I>();
        let socket = api.create();

        api.listen(&socket, Some(ZonedAddr::Unzoned(local_ip)), Some(LOCAL_PORT))
            .expect("listen should succeed");
        api.connect(&socket, Some(ZonedAddr::Unzoned(remote_ip)), REMOTE_PORT.into())
            .expect("connect should succeed");

        let (_, bindings_ctx) = api.contexts();
        assert_eq!(bindings_ctx.state.take_pending_error::<I>(&socket.downgrade()), None);

        // Inject the ICMP error.
        let mut original_body = vec![0u8; 8];
        original_body[0..2].copy_from_slice(&LOCAL_PORT.get().to_be_bytes());
        original_body[2..4].copy_from_slice(&REMOTE_PORT.get().to_be_bytes());
        original_body[4..6].copy_from_slice(&8u16.to_be_bytes());

        let (core_ctx, bindings_ctx) = api.contexts();

        <UdpIpTransportContext as IpTransportContext<I, _, _>>::receive_icmp_error(
            core_ctx,
            bindings_ctx,
            &FakeDeviceId,
            Some(local_ip),
            remote_ip,
            &original_body,
            icmp_err,
        );

        let (_, bindings_ctx) = api.contexts();
        assert_eq!(bindings_ctx.state.take_pending_error::<I>(&socket.downgrade()), expected_err);
        assert_eq!(bindings_ctx.state.take_pending_error::<I>(&socket.downgrade()), None);

        let (core_ctx, _) = api.contexts();
        let (with_socket_expects, without_socket_expects) = match expected_err {
            Some(_) => (
                CounterExpectationsWithSocket {
                    rx_icmp_error_hard_delivered: 1,
                    ..Default::default()
                },
                CounterExpectationsWithoutSocket {
                    rx_icmp_error: 1,
                    rx_icmp_error_hard: 1,
                    ..Default::default()
                },
            ),
            None => (
                CounterExpectationsWithSocket::default(),
                CounterExpectationsWithoutSocket {
                    rx_icmp_error: 1,
                    rx_icmp_error_soft: 1,
                    ..Default::default()
                },
            ),
        };
        let per_socket_expects = match expected_err {
            Some(_) => CounterExpectationsWithSocket {
                rx_icmp_error_hard_delivered: 1,
                ..Default::default()
            },
            None => CounterExpectationsWithSocket::default(),
        };
        assert_counters(
            core_ctx,
            with_socket_expects,
            without_socket_expects,
            [(&socket, per_socket_expects)],
        );
    }

    #[test_case(
        Icmpv4ErrorCode::DestUnreachable(
            Icmpv4DestUnreachableCode::DestNetworkUnreachable,
            Default::default()
        ),
        None;
        "v4 network unreachable"
    )]
    #[test_case(
        Icmpv4ErrorCode::DestUnreachable(
            Icmpv4DestUnreachableCode::DestHostUnreachable,
            Default::default()
        ),
        None;
        "v4 host unreachable"
    )]
    #[test_case(
        Icmpv4ErrorCode::DestUnreachable(
            Icmpv4DestUnreachableCode::DestProtocolUnreachable,
            Default::default()
        ),
        Some(PendingDatagramSocketError::ProtocolUnreachable);
        "v4 protocol unreachable"
    )]
    #[test_case(
        Icmpv4ErrorCode::DestUnreachable(
            Icmpv4DestUnreachableCode::DestPortUnreachable,
            Default::default()
        ),
        Some(PendingDatagramSocketError::PortUnreachable);
        "v4 connection refused"
    )]
    fn so_error_v4(icmp_err: Icmpv4ErrorCode, expected_err: Option<PendingDatagramSocketError>) {
        so_error_inner::<Ipv4>(icmp_err, expected_err)
    }

    #[test_case(
        Icmpv6ErrorCode::DestUnreachable(Icmpv6DestUnreachableCode::NoRoute),
        None;
        "v6 no route"
    )]
    #[test_case(
        Icmpv6ErrorCode::DestUnreachable(Icmpv6DestUnreachableCode::AddrUnreachable),
        None;
        "v6 addr unreachable"
    )]
    #[test_case(
        Icmpv6ErrorCode::DestUnreachable(Icmpv6DestUnreachableCode::PortUnreachable),
        Some(PendingDatagramSocketError::PortUnreachable);
        "v6 connection refused"
    )]
    #[test_case(
        Icmpv6ErrorCode::DestUnreachable(Icmpv6DestUnreachableCode::CommAdministrativelyProhibited),
        Some(PendingDatagramSocketError::PermissionDenied);
        "v6 permission denied"
    )]
    fn so_error_v6(icmp_err: Icmpv6ErrorCode, expected_err: Option<PendingDatagramSocketError>) {
        so_error_inner::<Ipv6>(icmp_err, expected_err)
    }

    #[test]
    fn so_error_dual_stack() {
        set_logger_for_test();

        const REMOTE_IP: Ipv4Addr = ip_v4!("8.8.8.8");
        const REMOTE_IP_MAPPED: Ipv6Addr = net_ip_v6!("::ffff:8.8.8.8");

        let mut ctx = FakeUdpCtx::with_core_ctx(FakeUdpCoreCtx::with_local_remote_ip_addrs(
            vec![SpecifiedAddr::new(V4_LOCAL_IP).unwrap()],
            vec![SpecifiedAddr::new(REMOTE_IP).unwrap()],
        ));
        let mut api = UdpApi::<Ipv6, _>::new(ctx.as_mut());
        let socket = api.create();

        api.listen(
            &socket,
            Some(ZonedAddr::Unzoned(SpecifiedAddr::new(V4_LOCAL_IP_MAPPED).unwrap())),
            Some(LOCAL_PORT),
        )
        .expect("listen should succeed");

        api.connect(
            &socket,
            Some(ZonedAddr::Unzoned(SpecifiedAddr::new(REMOTE_IP_MAPPED).unwrap())),
            REMOTE_PORT.into(),
        )
        .expect("connect should succeed");

        let (_, bindings_ctx) = api.contexts();
        assert_eq!(bindings_ctx.state.take_pending_error::<Ipv6>(&socket.downgrade()), None);

        let mut original_body = vec![0u8; 8];
        original_body[0..2].copy_from_slice(&LOCAL_PORT.get().to_be_bytes());
        original_body[2..4].copy_from_slice(&REMOTE_PORT.get().to_be_bytes());
        original_body[4..6].copy_from_slice(&8u16.to_be_bytes());

        let (core_ctx, bindings_ctx) = api.contexts();

        let err = Icmpv4ErrorCode::DestUnreachable(
            Icmpv4DestUnreachableCode::DestPortUnreachable,
            Default::default(),
        );

        <UdpIpTransportContext as IpTransportContext<Ipv4, _, _>>::receive_icmp_error(
            core_ctx,
            bindings_ctx,
            &FakeDeviceId,
            Some(SpecifiedAddr::new(V4_LOCAL_IP).unwrap()),
            SpecifiedAddr::new(REMOTE_IP).unwrap(),
            &original_body,
            err,
        );

        let (_, bindings_ctx) = api.contexts();
        assert_eq!(
            bindings_ctx.state.take_pending_error::<Ipv6>(&socket.downgrade()),
            Some(PendingDatagramSocketError::PortUnreachable)
        );

        let (core_ctx, _) = api.contexts();
        assert_counters(
            core_ctx,
            CounterExpectationsWithSocket { rx_icmp_error_hard_delivered: 1, ..Default::default() },
            CounterExpectationsWithoutSocket::default(),
            [(
                &socket,
                CounterExpectationsWithSocket {
                    rx_icmp_error_hard_delivered: 1,
                    ..Default::default()
                },
            )],
        );
        assert_eq!(
            CounterContext::<UdpCountersWithoutSocket<Ipv4>>::counters(core_ctx).cast(),
            CounterExpectationsWithoutSocket {
                rx_icmp_error: 1,
                rx_icmp_error_hard: 1,
                ..Default::default()
            }
        );
    }

    fn icmp_error_failed_counters_inner<I: TestIpExt>(icmp_err: I::ErrorCode) {
        set_logger_for_test();

        let mut ctx = FakeUdpCtx::with_core_ctx(FakeUdpCoreCtx::new_fake_device::<I>());
        let mut api = UdpApi::<I, _>::new(ctx.as_mut());

        let local_ip = local_ip::<I>();
        let remote_ip = remote_ip::<I>();

        // ICMP body too short

        let malformed_body = vec![0u8; 4];
        let (core_ctx, bindings_ctx) = api.contexts();

        <UdpIpTransportContext as IpTransportContext<I, _, _>>::receive_icmp_error(
            core_ctx,
            bindings_ctx,
            &FakeDeviceId,
            Some(local_ip),
            remote_ip,
            &malformed_body,
            icmp_err.clone(),
        );

        let (core_ctx, _) = api.contexts();
        assert_eq!(
            CounterContext::<UdpCountersWithoutSocket<I>>::counters(core_ctx).cast(),
            CounterExpectationsWithoutSocket {
                rx_icmp_error: 1,
                rx_icmp_error_hard: 1,
                rx_icmp_error_hard_malformed: 1,
                ..Default::default()
            }
        );

        // No matching socket

        let mut no_socket_body = vec![0u8; 8];
        no_socket_body[0..2].copy_from_slice(&LOCAL_PORT.get().to_be_bytes());
        no_socket_body[2..4].copy_from_slice(&REMOTE_PORT.get().to_be_bytes());
        no_socket_body[4..6].copy_from_slice(&8u16.to_be_bytes());

        let (core_ctx, bindings_ctx) = api.contexts();
        <UdpIpTransportContext as IpTransportContext<I, _, _>>::receive_icmp_error(
            core_ctx,
            bindings_ctx,
            &FakeDeviceId,
            Some(local_ip),
            remote_ip,
            &no_socket_body,
            icmp_err,
        );

        let (core_ctx, _) = api.contexts();
        assert_eq!(
            CounterContext::<UdpCountersWithoutSocket<I>>::counters(core_ctx).cast(),
            CounterExpectationsWithoutSocket {
                rx_icmp_error: 2,
                rx_icmp_error_hard: 2,
                rx_icmp_error_hard_malformed: 1,
                rx_icmp_error_hard_no_socket: 1,
                ..Default::default()
            }
        );
    }

    #[test_case(
        Icmpv4ErrorCode::DestUnreachable(
            Icmpv4DestUnreachableCode::DestPortUnreachable,
            Default::default()
        );
        "v4 failed icmp counters"
    )]
    fn icmp_error_failed_counters_v4(icmp_err: Icmpv4ErrorCode) {
        icmp_error_failed_counters_inner::<Ipv4>(icmp_err);
    }

    #[test_case(
        Icmpv6ErrorCode::DestUnreachable(Icmpv6DestUnreachableCode::PortUnreachable);
        "v6 failed icmp counters"
    )]
    fn icmp_error_failed_counters_v6(icmp_err: Icmpv6ErrorCode) {
        icmp_error_failed_counters_inner::<Ipv6>(icmp_err);
    }
}
