// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Shared code for implementing datagram sockets.

use alloc::vec::Vec;
use core::borrow::Borrow;
use core::convert::Infallible as Never;
use core::error::Error;
use core::fmt::Debug;
use core::hash::Hash;
use core::marker::PhantomData;
use core::num::{NonZeroU8, NonZeroU16};
use core::ops::{Deref, DerefMut};
use lock_order::lock::{OrderedLockAccess, OrderedLockRef};
use netstack3_ip::marker::OptionDelegationMarker;

use derivative::Derivative;
use either::Either;
use net_types::ip::{GenericOverIp, Ip, IpAddress, Ipv4, Ipv6, Mtu};
use net_types::{MulticastAddr, MulticastAddress as _, SpecifiedAddr, Witness, ZonedAddr};
use netstack3_base::socket::{
    self, AddrVec, BoundSocketMap, ConnAddr, ConnInfoAddr, ConnIpAddr, DualStackConnIpAddr,
    DualStackListenerIpAddr, DualStackLocalIp, DualStackRemoteIp, EitherStack, InsertError,
    ListenerAddr, ListenerIpAddr, MaybeDualStack, NotDualStackCapableError, Shutdown, ShutdownType,
    SocketDeviceUpdate, SocketDeviceUpdateNotAllowedError, SocketIpAddr, SocketIpExt,
    SocketMapAddrSpec, SocketMapConflictPolicy, SocketMapStateSpec, SocketStateEntry,
    SocketWritableListener, SocketZonedAddrExt as _, StrictlyZonedAddr,
};
use netstack3_base::sync::{self, RwLock};
use netstack3_base::{
    AnyDevice, BidirectionalConverter, ContextPair, CoreTxMetadataContext, DeviceIdContext,
    DeviceIdentifier, EitherDeviceId, IcmpErrorCode, Icmpv4ErrorCode, Icmpv6ErrorCode, Inspector,
    InspectorDeviceExt, InspectorExt as _, IpDeviceAddr, LocalAddressError, Mark, MarkDomain,
    Marks, NotFoundError, OwnedOrRefsBidirectionalConverter, ReferenceNotifiers,
    ReferenceNotifiersExt, RemoteAddressError, RemoveResourceResultWithContext, RngContext,
    SettingsContext, SocketError, StrongDeviceIdentifier, TxMetadataBindingsTypes,
    WeakDeviceIdentifier, ZonedAddressError,
};
use netstack3_filter::{FilterIpExt, TransportPacketSerializer};
use netstack3_hashmap::{HashMap, HashSet};
use netstack3_ip::socket::{
    DelegatedRouteResolutionOptions, DelegatedSendOptions, IpSock, IpSockCreateAndSendError,
    IpSockCreationError, IpSockSendError, IpSocketArgs, IpSocketHandler, RouteResolutionOptions,
    SendOneShotIpPacketError, SendOptions, SocketHopLimits,
};
use netstack3_ip::{
    BaseTransportIpContext, HopLimits, IpLayerIpExt, MulticastMembershipHandler, ResolveRouteError,
    SocketMetadata, TransportIpContext,
};
use packet::BufferMut;
use packet_formats::icmp::{Icmpv4DestUnreachableCode, Icmpv6DestUnreachableCode};
use packet_formats::ip::{DscpAndEcn, IpProtoExt};
use ref_cast::RefCast;
use thiserror::Error;

use crate::internal::settings::DatagramSettings;
use crate::internal::sndbuf::{SendBufferError, SendBufferTracking, TxMetadata};

/// Top-level struct kept in datagram socket references.
#[derive(Derivative)]
#[derivative(Debug(bound = ""))]
pub struct ReferenceState<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec> {
    pub(crate) state: RwLock<SocketState<I, D, S>>,
    pub(crate) external_data: S::ExternalData<I>,
    pub(crate) send_buffer: SendBufferTracking<S>,
    pub(crate) counters: S::Counters<I>,
}

// Local aliases for brevity.
type PrimaryRc<I, D, S> = sync::PrimaryRc<ReferenceState<I, D, S>>;
/// A convenient alias for a strong reference to a datagram socket.
pub type StrongRc<I, D, S> = sync::StrongRc<ReferenceState<I, D, S>>;
/// A convenient alias for a weak reference to a datagram socket.
pub type WeakRc<I, D, S> = sync::WeakRc<ReferenceState<I, D, S>>;

impl<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec>
    OrderedLockAccess<SocketState<I, D, S>> for ReferenceState<I, D, S>
{
    type Lock = RwLock<SocketState<I, D, S>>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.state)
    }
}

impl<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec> ReferenceState<I, D, S> {
    /// Returns the external data associated with the socket.
    pub fn external_data(&self) -> &S::ExternalData<I> {
        &self.external_data
    }

    /// Consumes the socket and returns the inner state and external data.
    pub fn into_state_and_external_data(self) -> (SocketState<I, D, S>, S::ExternalData<I>) {
        (self.state.into_inner(), self.external_data)
    }

    /// Provides access to the socket state sidestepping lock ordering.
    #[cfg(any(test, feature = "testutils"))]
    pub fn state(&self) -> &RwLock<SocketState<I, D, S>> {
        &self.state
    }

    /// Provides access to the socket's counters.
    pub fn counters(&self) -> &S::Counters<I> {
        &self.counters
    }
}

/// A set containing all datagram sockets for a given implementation.
#[derive(Derivative, GenericOverIp)]
#[derivative(Default(bound = ""))]
#[generic_over_ip(I, Ip)]
pub struct DatagramSocketSet<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec>(
    HashMap<StrongRc<I, D, S>, PrimaryRc<I, D, S>>,
);

impl<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec> Debug
    for DatagramSocketSet<I, D, S>
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let Self(rc) = self;
        f.debug_list().entries(rc.keys().map(StrongRc::debug_id)).finish()
    }
}

impl<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec> Deref
    for DatagramSocketSet<I, D, S>
{
    type Target = HashMap<StrongRc<I, D, S>, PrimaryRc<I, D, S>>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec> DerefMut
    for DatagramSocketSet<I, D, S>
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Marker trait for datagram IP extensions.
pub trait IpExt: netstack3_ip::IpLayerIpExt + DualStackIpExt {}
impl<I: netstack3_ip::IpLayerIpExt + DualStackIpExt> IpExt for I {}

/// Errors surfaced on sockets via GetError.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Error)]
pub enum PendingDatagramSocketError {
    /// The network is unreachable.
    #[error("network is unreachable")]
    NetworkUnreachable,
    /// The destination host is unreachable.
    #[error("host is unreachable")]
    HostUnreachable,
    /// The destination protocol is unreachable.
    #[error("protocol is unreachable")]
    ProtocolUnreachable,
    /// The destination port is unreachable.
    #[error("port is unreachable")]
    PortUnreachable,
    /// The host is down.
    #[error("host is down")]
    DestinationHostDown,
    /// The datagram lacked required permissions.
    #[error("permission denied")]
    PermissionDenied,
    /// There was a protocol error.
    #[error("protocol error")]
    ProtocolError,
    /// A packet sent was too large.
    #[error("packet too big")]
    PacketTooBig,
    /// The connection was aborted by the system.
    #[error("connection was aborted by the system")]
    Aborted,
}

impl PendingDatagramSocketError {
    /// Maps hard ICMP error codes to [`PendingDatagramSocketError`].
    ///
    /// The classification of what constitutes a hard error is meant to match
    /// Linux.
    pub fn from_hard_icmp(err: IcmpErrorCode) -> Option<Self> {
        match err {
            IcmpErrorCode::V4(v4_err) => match v4_err {
                Icmpv4ErrorCode::DestUnreachable(code, _) => match code {
                    Icmpv4DestUnreachableCode::DestPortUnreachable => Some(Self::PortUnreachable),
                    Icmpv4DestUnreachableCode::DestProtocolUnreachable => {
                        Some(Self::ProtocolUnreachable)
                    }
                    Icmpv4DestUnreachableCode::CommAdministrativelyProhibited => {
                        Some(Self::HostUnreachable)
                    }
                    Icmpv4DestUnreachableCode::DestNetworkUnknown => Some(Self::NetworkUnreachable),
                    Icmpv4DestUnreachableCode::DestHostUnknown => Some(Self::DestinationHostDown),
                    Icmpv4DestUnreachableCode::FragmentationRequired => Some(Self::PacketTooBig),
                    Icmpv4DestUnreachableCode::DestNetworkUnreachable
                    | Icmpv4DestUnreachableCode::DestHostUnreachable
                    | Icmpv4DestUnreachableCode::SourceRouteFailed
                    | Icmpv4DestUnreachableCode::SourceHostIsolated
                    | Icmpv4DestUnreachableCode::NetworkAdministrativelyProhibited
                    | Icmpv4DestUnreachableCode::HostAdministrativelyProhibited
                    | Icmpv4DestUnreachableCode::NetworkUnreachableForToS
                    | Icmpv4DestUnreachableCode::HostUnreachableForToS
                    | Icmpv4DestUnreachableCode::HostPrecedenceViolation
                    | Icmpv4DestUnreachableCode::PrecedenceCutoffInEffect => None,
                },
                Icmpv4ErrorCode::ParameterProblem(_) => Some(Self::ProtocolError),
                Icmpv4ErrorCode::TimeExceeded(_) | Icmpv4ErrorCode::Redirect(_) => None,
            },
            IcmpErrorCode::V6(v6_err) => match v6_err {
                Icmpv6ErrorCode::DestUnreachable(code) => match code {
                    Icmpv6DestUnreachableCode::PortUnreachable => Some(Self::PortUnreachable),
                    Icmpv6DestUnreachableCode::CommAdministrativelyProhibited
                    | Icmpv6DestUnreachableCode::SrcAddrFailedPolicy
                    | Icmpv6DestUnreachableCode::RejectRoute => Some(Self::PermissionDenied),
                    Icmpv6DestUnreachableCode::NoRoute
                    | Icmpv6DestUnreachableCode::BeyondScope
                    | Icmpv6DestUnreachableCode::AddrUnreachable => None,
                },
                Icmpv6ErrorCode::ParameterProblem(_) => Some(Self::ProtocolError),
                Icmpv6ErrorCode::PacketTooBig(_) => Some(Self::PacketTooBig),
                Icmpv6ErrorCode::TimeExceeded(_) => None,
            },
        }
    }
}

/// A datagram socket's state.
#[derive(Derivative, GenericOverIp)]
#[generic_over_ip(I, Ip)]
#[derivative(Debug(bound = ""))]
pub struct SocketState<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec> {
    /// Bind/connect state of the socket.
    pub inner: SocketStateInner<I, D, S>,

    /// Socket options that do not depend on the bind/connect state.
    pub(crate) ip_options: IpOptions<I, D, S>,

    /// Sharing state of the socket.
    pub(crate) sharing: S::SharingState,
}

#[derive(Derivative, GenericOverIp)]
#[generic_over_ip(I, Ip)]
#[derivative(Debug(bound = ""))]
#[allow(missing_docs)]
pub enum SocketStateInner<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec> {
    Unbound(UnboundSocketState<D>),
    Bound(BoundSocketState<I, D, S>),
}

impl<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec> SocketState<I, D, S> {
    /// Returns [`SocketInfo`] for this datagram socket.
    pub fn to_socket_info(&self) -> SocketInfo<I::Addr, D> {
        match &self.inner {
            SocketStateInner::Unbound(_) => SocketInfo::Unbound,
            SocketStateInner::Bound(BoundSocketState { socket_type, original_bound_addr: _ }) => {
                match socket_type {
                    BoundSocketStateType::Listener(state) => {
                        let ListenerState { addr } = state;
                        SocketInfo::Listener(addr.clone().into())
                    }
                    BoundSocketStateType::Connected(state) => {
                        SocketInfo::Connected(S::conn_info_from_state(&state))
                    }
                }
            }
        }
    }

    /// Returns the local IP address, if the socket is bound to one.
    pub fn local_ip(&self) -> Option<StrictlyZonedAddr<I::Addr, SpecifiedAddr<I::Addr>, D>> {
        match self.to_socket_info() {
            SocketInfo::Unbound => None,
            SocketInfo::Listener(ListenerInfo { local_ip, .. }) => local_ip,
            SocketInfo::Connected(ConnInfo { local_ip, .. }) => Some(local_ip),
        }
    }

    /// Returns the local socket identifier (e.g. port), if the socket is bound to one.
    pub fn local_identifier(&self) -> Option<NonZeroU16> {
        match self.to_socket_info() {
            SocketInfo::Unbound => None,
            SocketInfo::Listener(ListenerInfo { local_identifier, .. }) => Some(local_identifier),
            SocketInfo::Connected(ConnInfo { local_identifier, .. }) => Some(local_identifier),
        }
    }

    /// Returns the remote IP address, if the datagram socket is connected.
    pub fn remote_ip(&self) -> Option<StrictlyZonedAddr<I::Addr, SpecifiedAddr<I::Addr>, D>> {
        match self.to_socket_info() {
            SocketInfo::Unbound => None,
            SocketInfo::Listener(_) => None,
            SocketInfo::Connected(ConnInfo { remote_ip, .. }) => Some(remote_ip),
        }
    }

    /// Returns the remote identifier (e.g. port), if the datagram socket is connected.
    pub fn remote_identifier(&self) -> Option<u16> {
        match self.to_socket_info() {
            SocketInfo::Unbound => None,
            SocketInfo::Listener(_) => None,
            SocketInfo::Connected(ConnInfo { remote_identifier, .. }) => Some(remote_identifier),
        }
    }

    /// Record inspect information generic to each datagram protocol.
    pub fn record_common_info<N>(&self, inspector: &mut N)
    where
        N: Inspector + InspectorDeviceExt<D>,
    {
        inspector.record_str("TransportProtocol", S::NAME);
        inspector.record_str("NetworkProtocol", I::NAME);

        let socket_info = self.to_socket_info();
        let (local, remote) = match socket_info {
            SocketInfo::Unbound => (None, None),
            SocketInfo::Listener(ListenerInfo { local_ip, local_identifier }) => (
                Some((
                    local_ip.map_or_else(
                        || ZonedAddr::Unzoned(I::UNSPECIFIED_ADDRESS),
                        |addr| addr.into_inner_without_witness(),
                    ),
                    local_identifier,
                )),
                None,
            ),
            SocketInfo::Connected(ConnInfo {
                local_ip,
                local_identifier,
                remote_ip,
                remote_identifier,
            }) => (
                Some((local_ip.into_inner_without_witness(), local_identifier)),
                Some((remote_ip.into_inner_without_witness(), remote_identifier)),
            ),
        };
        inspector.record_local_socket_addr::<N, _, _, _>(local);
        inspector.record_remote_socket_addr::<N, _, _, _>(remote);

        let IpOptions {
            multicast_memberships: MulticastMemberships(multicast_memberships),
            socket_options: _,
            other_stack: _,
            common,
        } = self.options();
        inspector.record_child("MulticastGroupMemberships", |node| {
            for (index, (multicast_addr, device)) in multicast_memberships.iter().enumerate() {
                node.record_debug_child(index, |node| {
                    node.record_ip_addr("MulticastGroup", multicast_addr.get());
                    N::record_device(node, "Device", device);
                })
            }
        });
        inspector.delegate_inspectable(&common.marks);
    }

    /// Returns the device to which the socket is bound.
    pub fn get_device<
        'a,
        BC: DatagramBindingsTypes,
        CC: DatagramBoundStateContext<I, BC, S, WeakDeviceId = D>,
    >(
        &'a self,
        core_ctx: &CC,
    ) -> &'a Option<CC::WeakDeviceId> {
        match &self.inner {
            SocketStateInner::Unbound(UnboundSocketState { device }) => device,
            SocketStateInner::Bound(state) => state.get_device(core_ctx),
        }
    }

    /// Returns `IpOptions`.
    pub fn options(&self) -> &IpOptions<I, D, S> {
        &self.ip_options
    }

    /// Returns mutable `IpOptions`.
    pub fn options_mut(&mut self) -> &mut IpOptions<I, D, S> {
        &mut self.ip_options
    }
}

/// State associated with a Bound Socket.
#[derive(Derivative)]
#[derivative(Debug(bound = "D: Debug"))]
pub struct BoundSocketState<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec> {
    /// The type of bound socket (e.g. Listener vs. Connected), and any
    /// type-specific state.
    pub socket_type: BoundSocketStateType<I, D, S>,
    /// The original bound address of the socket, as requested by the caller.
    /// `None` if:
    ///   * the socket was connected from unbound, or
    ///   * listen was called without providing a local port.
    pub original_bound_addr: Option<S::ListenerIpAddr<I>>,
}

impl<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec> BoundSocketState<I, D, S> {
    fn get_device<
        BC: DatagramBindingsTypes,
        CC: DatagramBoundStateContext<I, BC, S, WeakDeviceId = D>,
    >(
        &self,
        core_ctx: &CC,
    ) -> &Option<D> {
        match &self.socket_type {
            BoundSocketStateType::Listener(ListenerState { addr: ListenerAddr { device, .. } }) => {
                device
            }
            BoundSocketStateType::Connected(state) => match core_ctx.dual_stack_context() {
                MaybeDualStack::DualStack(dual_stack) => {
                    match dual_stack.ds_converter().convert(state) {
                        DualStackConnState::ThisStack(state) => state.get_device(),
                        DualStackConnState::OtherStack(state) => state.get_device(),
                    }
                }
                MaybeDualStack::NotDualStack(not_dual_stack) => {
                    not_dual_stack.nds_converter().convert(state).get_device()
                }
            },
        }
    }
}

/// State for the sub-types of bound socket (e.g. Listener or Connected).
#[derive(Derivative)]
#[derivative(Debug(bound = "D: Debug"))]
#[allow(missing_docs)]
pub enum BoundSocketStateType<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec> {
    Listener(ListenerState<I, D, S>),
    Connected(S::ConnState<I, D>),
}

#[derive(Derivative)]
#[derivative(Debug(bound = ""), Default(bound = ""))]
pub struct UnboundSocketState<D: WeakDeviceIdentifier> {
    device: Option<D>,
}

/// State associated with a listening socket.
#[derive(Derivative)]
#[derivative(Debug(bound = ""))]
pub struct ListenerState<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec + ?Sized> {
    pub(crate) addr: ListenerAddr<S::ListenerIpAddr<I>, D>,
}

/// State associated with a connected socket.
#[derive(Derivative)]
#[derivative(Debug(bound = "D: Debug"))]
pub struct ConnState<WireI: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec + ?Sized> {
    pub(crate) socket: IpSock<WireI, D>,
    pub(crate) shutdown: Shutdown,
    pub(crate) addr: ConnAddr<
        ConnIpAddr<
            WireI::Addr,
            <S::AddrSpec as SocketMapAddrSpec>::LocalIdentifier,
            <S::AddrSpec as SocketMapAddrSpec>::RemoteIdentifier,
        >,
        D,
    >,
    /// Determines whether a call to disconnect this socket should also clear
    /// the device on the socket address.
    ///
    /// This will only be `true` if
    ///   1) the corresponding address has a bound device
    ///   2) the local address does not require a zone
    ///   3) the remote address does require a zone
    ///   4) the device was not set via [`set_unbound_device`]
    ///
    /// In that case, when the socket is disconnected, the device should be
    /// cleared since it was set as part of a `connect` call, not explicitly.
    pub(crate) clear_device_on_disconnect: bool,

    /// The extra state for the connection.
    ///
    /// For UDP it should be [`()`], for ICMP it should be [`NonZeroU16`] to
    /// remember the remote ID set by connect.
    pub(crate) extra: S::ConnStateExtra,
}

impl<WireI: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec> AsRef<Shutdown>
    for ConnState<WireI, D, S>
{
    fn as_ref(&self) -> &Shutdown {
        &self.shutdown
    }
}

impl<WireI: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec> AsMut<Shutdown>
    for ConnState<WireI, D, S>
{
    fn as_mut(&mut self) -> &mut Shutdown {
        &mut self.shutdown
    }
}

impl<WireI: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec> ConnState<WireI, D, S> {
    /// Returns true if the connection can receive traffic.
    pub fn should_receive(&self) -> bool {
        let Self { shutdown, socket: _, clear_device_on_disconnect: _, addr: _, extra: _ } = self;
        let Shutdown { receive, send: _ } = shutdown;
        !*receive
    }

    /// Returns the bound addresses for the connection.
    pub fn addr(
        &self,
    ) -> &ConnAddr<
        ConnIpAddr<
            WireI::Addr,
            <S::AddrSpec as SocketMapAddrSpec>::LocalIdentifier,
            <S::AddrSpec as SocketMapAddrSpec>::RemoteIdentifier,
        >,
        D,
    > {
        &self.addr
    }

    /// Returns the extra opaque information kept in connected state.
    pub fn extra(&self) -> &S::ConnStateExtra {
        &self.extra
    }

    fn get_device(&self) -> &Option<D> {
        let Self { addr: ConnAddr { device, .. }, .. } = self;
        device
    }
}

/// Connection state belong to either this-stack or the other-stack.
#[derive(Derivative)]
#[derivative(Debug(bound = ""))]
pub enum DualStackConnState<
    I: IpExt + DualStackIpExt,
    D: WeakDeviceIdentifier,
    S: DatagramSocketSpec + ?Sized,
> {
    /// The [`ConnState`] for a socked connected with [`I::Version`].
    ThisStack(ConnState<I, D, S>),
    /// The [`ConnState`] for a socked connected with [`I::OtherVersion`].
    OtherStack(ConnState<I::OtherVersion, D, S>),
}

impl<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec> AsRef<Shutdown>
    for DualStackConnState<I, D, S>
{
    fn as_ref(&self) -> &Shutdown {
        match self {
            DualStackConnState::ThisStack(state) => state.as_ref(),
            DualStackConnState::OtherStack(state) => state.as_ref(),
        }
    }
}

impl<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec> AsMut<Shutdown>
    for DualStackConnState<I, D, S>
{
    fn as_mut(&mut self) -> &mut Shutdown {
        match self {
            DualStackConnState::ThisStack(state) => state.as_mut(),
            DualStackConnState::OtherStack(state) => state.as_mut(),
        }
    }
}

/// A datagram socket's options.
///
/// These options are held twice by dual stack sockets, since they hold
/// different values per IP version.
#[derive(Derivative, GenericOverIp)]
#[generic_over_ip(I, Ip)]
#[derivative(Clone(bound = ""), Debug, Default(bound = ""))]
pub struct DatagramIpSpecificSocketOptions<I: IpExt, D: WeakDeviceIdentifier> {
    /// The configured hop limits.
    pub hop_limits: SocketHopLimits<I>,
    /// The selected multicast interface.
    pub multicast_interface: Option<D>,

    /// Whether multicast packet loopback is enabled or not (see
    /// IP_MULTICAST_LOOP flag). Enabled by default.
    #[derivative(Default(value = "true"))]
    pub multicast_loop: bool,

    /// Set to `Some` when the socket can be used to send broadcast packets.
    pub allow_broadcast: Option<I::BroadcastMarker>,

    /// IPV6_TCLASS or IP_TOS option.
    pub dscp_and_ecn: DscpAndEcn,
}

impl<I: IpExt, D: WeakDeviceIdentifier> SendOptions<I> for DatagramIpSpecificSocketOptions<I, D> {
    fn hop_limit(&self, destination: &SpecifiedAddr<I::Addr>) -> Option<NonZeroU8> {
        self.hop_limits.hop_limit_for_dst(destination)
    }

    fn multicast_loop(&self) -> bool {
        self.multicast_loop
    }

    fn allow_broadcast(&self) -> Option<I::BroadcastMarker> {
        self.allow_broadcast
    }

    fn dscp_and_ecn(&self) -> DscpAndEcn {
        self.dscp_and_ecn
    }

    fn mtu(&self) -> Mtu {
        Mtu::no_limit()
    }
}

#[derive(Clone, Debug, Default)]
struct DatagramIpAgnosticOptions {
    transparent: bool,
    marks: Marks,
}

impl<I: Ip> RouteResolutionOptions<I> for DatagramIpAgnosticOptions {
    fn transparent(&self) -> bool {
        self.transparent
    }

    fn marks(&self) -> &Marks {
        &self.marks
    }
}

/// Holds references to provide implementations of [`SendOptions`] and
/// [`RouteResolutionOptions`] with appropriate access to underlying data.
struct IpOptionsRef<'a, I: IpExt, D: WeakDeviceIdentifier> {
    ip_specific: &'a DatagramIpSpecificSocketOptions<I, D>,
    agnostic: &'a DatagramIpAgnosticOptions,
}

impl<'a, I: IpExt, D: WeakDeviceIdentifier> OptionDelegationMarker for IpOptionsRef<'a, I, D> {}

impl<'a, I: IpExt, D: WeakDeviceIdentifier> DelegatedSendOptions<I> for IpOptionsRef<'a, I, D> {
    fn delegate(&self) -> &impl SendOptions<I> {
        self.ip_specific
    }
}

impl<'a, I: IpExt, D: WeakDeviceIdentifier> DelegatedRouteResolutionOptions<I>
    for IpOptionsRef<'a, I, D>
{
    fn delegate(&self) -> &impl RouteResolutionOptions<I> {
        self.agnostic
    }
}

/// A datagram socket's IP options.
#[derive(Derivative, GenericOverIp)]
#[generic_over_ip(I, Ip)]
#[derivative(Clone(bound = ""), Debug, Default(bound = ""))]
pub struct IpOptions<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec + ?Sized> {
    multicast_memberships: MulticastMemberships<I::Addr, D>,
    socket_options: DatagramIpSpecificSocketOptions<I, D>,
    other_stack: S::OtherStackIpOptions<I, D>,
    common: DatagramIpAgnosticOptions,
}

impl<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec> IpOptions<I, D, S> {
    /// Returns the IP options for the other stack.
    pub fn other_stack(&self) -> &S::OtherStackIpOptions<I, D> {
        &self.other_stack
    }

    /// Returns the transparent option.
    pub fn transparent(&self) -> bool {
        self.common.transparent
    }

    /// Returns `Marks`.
    pub fn marks(&self) -> &Marks {
        &self.common.marks
    }

    fn this_stack_options_ref(&self) -> IpOptionsRef<'_, I, D> {
        IpOptionsRef { ip_specific: &self.socket_options, agnostic: &self.common }
    }

    fn other_stack_options_ref<
        'a,
        BC: DatagramBindingsTypes,
        CC: DualStackDatagramBoundStateContext<I, BC, S, WeakDeviceId = D>,
    >(
        &'a self,
        ctx: &CC,
    ) -> IpOptionsRef<'a, I::OtherVersion, D> {
        IpOptionsRef { ip_specific: ctx.to_other_socket_options(self), agnostic: &self.common }
    }
}

#[derive(Clone, Debug, Derivative)]
#[derivative(Default(bound = ""))]
pub(crate) struct MulticastMemberships<A, D>(HashSet<(MulticastAddr<A>, D)>);

#[cfg_attr(test, derive(Debug, PartialEq))]
pub(crate) enum MulticastMembershipChange {
    Join,
    Leave,
}

impl<A: Eq + Hash, D: WeakDeviceIdentifier> MulticastMemberships<A, D> {
    pub(crate) fn apply_membership_change(
        &mut self,
        address: MulticastAddr<A>,
        device: &D,
        want_membership: bool,
    ) -> Option<MulticastMembershipChange> {
        let device = device.clone();

        let Self(map) = self;
        if want_membership {
            map.insert((address, device)).then_some(MulticastMembershipChange::Join)
        } else {
            map.remove(&(address, device)).then_some(MulticastMembershipChange::Leave)
        }
    }
}

impl<A: Eq + Hash, D: Eq + Hash> IntoIterator for MulticastMemberships<A, D> {
    type Item = (MulticastAddr<A>, D);
    type IntoIter = <HashSet<(MulticastAddr<A>, D)> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        let Self(memberships) = self;
        memberships.into_iter()
    }
}

fn leave_all_joined_groups<A: IpAddress, BC, CC: MulticastMembershipHandler<A::Version, BC>>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    memberships: &MulticastMemberships<A, CC::WeakDeviceId>,
) {
    let MulticastMemberships(map) = memberships;
    for (addr, device) in map.iter() {
        let Some(device) = device.upgrade() else {
            continue;
        };
        core_ctx.leave_multicast_group(bindings_ctx, &device, addr.clone())
    }
}

/// Identifies a flow for a datagram socket.
#[derive(Hash)]
pub struct DatagramFlowId<A: IpAddress, RI> {
    /// Socket's local address.
    pub local_ip: SocketIpAddr<A>,
    /// Socket's remote address.
    pub remote_ip: SocketIpAddr<A>,
    /// Socket's remote identifier (port).
    pub remote_id: RI,
}

/// The core context providing access to datagram socket state.
pub trait DatagramStateContext<I: IpExt, BC: DatagramBindingsTypes, S: DatagramSocketSpec>:
    DeviceIdContext<AnyDevice>
{
    /// The core context passed to the callback provided to methods.
    type SocketsStateCtx<'a>: DatagramBoundStateContext<I, BC, S>
        + DeviceIdContext<AnyDevice, DeviceId = Self::DeviceId, WeakDeviceId = Self::WeakDeviceId>;

    /// Calls the function with mutable access to the set with all datagram
    /// sockets.
    fn with_all_sockets_mut<O, F: FnOnce(&mut DatagramSocketSet<I, Self::WeakDeviceId, S>) -> O>(
        &mut self,
        cb: F,
    ) -> O;

    /// Calls the function with immutable access to the set with all datagram
    /// sockets.
    fn with_all_sockets<O, F: FnOnce(&DatagramSocketSet<I, Self::WeakDeviceId, S>) -> O>(
        &mut self,
        cb: F,
    ) -> O;

    /// Calls the function with an immutable reference to the given socket's
    /// state.
    fn with_socket_state<
        O,
        F: FnOnce(&mut Self::SocketsStateCtx<'_>, &SocketState<I, Self::WeakDeviceId, S>) -> O,
    >(
        &mut self,
        id: &S::SocketId<I, Self::WeakDeviceId>,
        cb: F,
    ) -> O;

    /// Calls the function with a mutable reference to the given socket's state.
    fn with_socket_state_mut<
        O,
        F: FnOnce(&mut Self::SocketsStateCtx<'_>, &mut SocketState<I, Self::WeakDeviceId, S>) -> O,
    >(
        &mut self,
        id: &S::SocketId<I, Self::WeakDeviceId>,
        cb: F,
    ) -> O;

    /// Call `f` with each socket's state.
    fn for_each_socket<
        F: FnMut(
            &mut Self::SocketsStateCtx<'_>,
            &S::SocketId<I, Self::WeakDeviceId>,
            &SocketState<I, Self::WeakDeviceId, S>,
        ),
    >(
        &mut self,
        cb: F,
    );
}

/// A convenient alias for the BoundSocketMap type to shorten type signatures.
pub(crate) type BoundSocketsFromSpec<I, CC, S> =
    BoundDatagramSocketMap<I, <CC as DeviceIdContext<AnyDevice>>::WeakDeviceId, S>;

/// A marker trait for bindings types traits used by datagram.
pub trait DatagramBindingsTypes: TxMetadataBindingsTypes {}
impl<BT> DatagramBindingsTypes for BT where BT: TxMetadataBindingsTypes {}

/// The core context providing access to bound datagram sockets.
pub trait DatagramBoundStateContext<
    I: IpExt + DualStackIpExt,
    BC: DatagramBindingsTypes,
    S: DatagramSocketSpec,
>: DeviceIdContext<AnyDevice>
{
    /// The core context passed to the callback provided to methods.
    type IpSocketsCtx<'a>: TransportIpContext<I, BC>
        + CoreTxMetadataContext<TxMetadata<I, Self::WeakDeviceId, S>, BC>
        + MulticastMembershipHandler<I, BC>
        + DeviceIdContext<AnyDevice, DeviceId = Self::DeviceId, WeakDeviceId = Self::WeakDeviceId>;

    /// Context for dual-stack socket state access.
    ///
    /// This type type provides access, via an implementation of the
    /// [`DualStackDatagramBoundStateContext`] trait, to state necessary for
    /// implementing dual-stack socket operations. While a type must always be
    /// provided, implementations of [`DatagramBoundStateContext`] for socket
    /// types that don't support dual-stack operation (like ICMP and raw IP
    /// sockets, and UDPv4) can use the [`UninstantiableDualStackContext`] type,
    /// which is uninstantiable.
    type DualStackContext: DualStackDatagramBoundStateContext<
            I,
            BC,
            S,
            DeviceId = Self::DeviceId,
            WeakDeviceId = Self::WeakDeviceId,
        >;

    /// Context for single-stack socket access.
    ///
    /// This type provides access, via an implementation of the
    /// [`NonDualStackDatagramBoundStateContext`] trait, to functionality
    /// necessary to implement sockets that do not support dual-stack operation.
    type NonDualStackContext: NonDualStackDatagramBoundStateContext<
            I,
            BC,
            S,
            DeviceId = Self::DeviceId,
            WeakDeviceId = Self::WeakDeviceId,
        >;

    /// Calls the function with an immutable reference to the datagram sockets.
    fn with_bound_sockets<
        O,
        F: FnOnce(&mut Self::IpSocketsCtx<'_>, &BoundSocketsFromSpec<I, Self, S>) -> O,
    >(
        &mut self,
        cb: F,
    ) -> O;

    /// Calls the function with a mutable reference to the datagram sockets.
    fn with_bound_sockets_mut<
        O,
        F: FnOnce(&mut Self::IpSocketsCtx<'_>, &mut BoundSocketsFromSpec<I, Self, S>) -> O,
    >(
        &mut self,
        cb: F,
    ) -> O;

    /// Provides access to either the dual-stack or non-dual-stack context.
    ///
    /// For socket types that don't support dual-stack operation (like ICMP,
    /// raw IP sockets, and UDPv4), this method should always return a reference
    /// to the non-dual-stack context to allow the caller to access
    /// non-dual-stack state. Otherwise it should provide an instance of the
    /// `DualStackContext`, which can be used by the caller to access dual-stack
    /// state.
    fn dual_stack_context(
        &self,
    ) -> MaybeDualStack<&Self::DualStackContext, &Self::NonDualStackContext>;

    /// The same as [`dual_stack_context`], but provides mutable references.
    fn dual_stack_context_mut(
        &mut self,
    ) -> MaybeDualStack<&mut Self::DualStackContext, &mut Self::NonDualStackContext>;

    /// Calls the function with only the inner context.
    fn with_transport_context<O, F: FnOnce(&mut Self::IpSocketsCtx<'_>) -> O>(
        &mut self,
        cb: F,
    ) -> O;
}

/// A marker trait for the requirements of
/// [`DualStackDatagramBoundStateContext::ds_converter`].
pub trait DualStackConverter<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec>:
    'static
    + OwnedOrRefsBidirectionalConverter<
        S::ListenerIpAddr<I>,
        DualStackListenerIpAddr<I::Addr, <S::AddrSpec as SocketMapAddrSpec>::LocalIdentifier>,
    >
    + OwnedOrRefsBidirectionalConverter<
        S::ConnIpAddr<I>,
        DualStackConnIpAddr<
            I::Addr,
            <S::AddrSpec as SocketMapAddrSpec>::LocalIdentifier,
            <S::AddrSpec as SocketMapAddrSpec>::RemoteIdentifier,
        >,
    >
    + OwnedOrRefsBidirectionalConverter<S::ConnState<I, D>, DualStackConnState<I, D, S>>
{
}

impl<I, D, S, O> DualStackConverter<I, D, S> for O
where
    I: IpExt,
    D: WeakDeviceIdentifier,
    S: DatagramSocketSpec,
    O: 'static
        + OwnedOrRefsBidirectionalConverter<
            S::ListenerIpAddr<I>,
            DualStackListenerIpAddr<I::Addr, <S::AddrSpec as SocketMapAddrSpec>::LocalIdentifier>,
        >
        + OwnedOrRefsBidirectionalConverter<
            S::ConnIpAddr<I>,
            DualStackConnIpAddr<
                I::Addr,
                <S::AddrSpec as SocketMapAddrSpec>::LocalIdentifier,
                <S::AddrSpec as SocketMapAddrSpec>::RemoteIdentifier,
            >,
        >
        + OwnedOrRefsBidirectionalConverter<S::ConnState<I, D>, DualStackConnState<I, D, S>>,
{
}

/// Provides access to dual-stack socket state.
pub trait DualStackDatagramBoundStateContext<
    I: IpExt,
    BC: DatagramBindingsTypes,
    S: DatagramSocketSpec,
>: DeviceIdContext<AnyDevice>
{
    /// The core context passed to the callbacks to methods.
    type IpSocketsCtx<'a>: TransportIpContext<I, BC>
        + CoreTxMetadataContext<TxMetadata<I, Self::WeakDeviceId, S>, BC>
        + DeviceIdContext<AnyDevice, DeviceId = Self::DeviceId, WeakDeviceId = Self::WeakDeviceId>
        // Allow creating IP sockets for the other IP version.
        + TransportIpContext<I::OtherVersion, BC>
        + CoreTxMetadataContext<TxMetadata<I::OtherVersion, Self::WeakDeviceId, S>, BC>;

    /// Returns if the socket state indicates dual-stack operation is enabled.
    fn dual_stack_enabled(&self, ip_options: &IpOptions<I, Self::WeakDeviceId, S>) -> bool;

    /// Returns the [`DatagramIpSpecificSocketOptions`] to use for packets in the other stack.
    fn to_other_socket_options<'a>(
        &self,
        state: &'a IpOptions<I, Self::WeakDeviceId, S>,
    ) -> &'a DatagramIpSpecificSocketOptions<I::OtherVersion, Self::WeakDeviceId>;

    /// Asserts that the socket options indicates dual-stack operation is enabled.
    ///
    /// Provided trait function.
    fn assert_dual_stack_enabled(&self, ip_options: &IpOptions<I, Self::WeakDeviceId, S>) {
        debug_assert!(self.dual_stack_enabled(ip_options), "socket must be dual-stack enabled")
    }

    /// Returns an instance of a type that implements [`DualStackConverter`]
    /// for addresses.
    fn ds_converter(&self) -> impl DualStackConverter<I, Self::WeakDeviceId, S>;

    /// Converts a socket ID to a bound socket ID.
    ///
    /// Converts a socket ID for IP version `I` into a bound socket ID that can
    /// be inserted into the demultiplexing map for IP version `I::OtherVersion`.
    fn to_other_bound_socket_id(
        &self,
        id: &S::SocketId<I, Self::WeakDeviceId>,
    ) -> <S::SocketMapSpec<I::OtherVersion, Self::WeakDeviceId> as DatagramSocketMapSpec<
        I::OtherVersion,
        Self::WeakDeviceId,
        S::AddrSpec,
    >>::BoundSocketId;

    /// Calls the provided callback with mutable access to both the
    /// demultiplexing maps.
    fn with_both_bound_sockets_mut<
        O,
        F: FnOnce(
            &mut Self::IpSocketsCtx<'_>,
            &mut BoundSocketsFromSpec<I, Self, S>,
            &mut BoundSocketsFromSpec<I::OtherVersion, Self, S>,
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
            &mut BoundSocketsFromSpec<I::OtherVersion, Self, S>,
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

/// A marker trait for the requirements of
/// [`NonDualStackDatagramBoundStateContext::nds_converter`].
pub trait NonDualStackConverter<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec>:
    'static
    + OwnedOrRefsBidirectionalConverter<
        S::ListenerIpAddr<I>,
        ListenerIpAddr<I::Addr, <S::AddrSpec as SocketMapAddrSpec>::LocalIdentifier>,
    >
    + OwnedOrRefsBidirectionalConverter<
        S::ConnIpAddr<I>,
        ConnIpAddr<
            I::Addr,
            <S::AddrSpec as SocketMapAddrSpec>::LocalIdentifier,
            <S::AddrSpec as SocketMapAddrSpec>::RemoteIdentifier,
        >,
    >
    + OwnedOrRefsBidirectionalConverter<S::ConnState<I, D>, ConnState<I, D, S>>
{
}

impl<I, D, S, O> NonDualStackConverter<I, D, S> for O
where
    I: IpExt,
    D: WeakDeviceIdentifier,
    S: DatagramSocketSpec,
    O: 'static
        + OwnedOrRefsBidirectionalConverter<
            S::ListenerIpAddr<I>,
            ListenerIpAddr<I::Addr, <S::AddrSpec as SocketMapAddrSpec>::LocalIdentifier>,
        >
        + OwnedOrRefsBidirectionalConverter<
            S::ConnIpAddr<I>,
            ConnIpAddr<
                I::Addr,
                <S::AddrSpec as SocketMapAddrSpec>::LocalIdentifier,
                <S::AddrSpec as SocketMapAddrSpec>::RemoteIdentifier,
            >,
        >
        + OwnedOrRefsBidirectionalConverter<S::ConnState<I, D>, ConnState<I, D, S>>,
{
}

/// Provides access to socket state for a single IP version.
pub trait NonDualStackDatagramBoundStateContext<I: IpExt, BC, S: DatagramSocketSpec>:
    DeviceIdContext<AnyDevice>
{
    /// Returns an instance of a type that implements [`NonDualStackConverter`]
    /// for addresses.
    fn nds_converter(&self) -> impl NonDualStackConverter<I, Self::WeakDeviceId, S>;
}

/// Blanket trait for bindings context requirements for datagram sockets.
pub trait DatagramBindingsContext: RngContext + ReferenceNotifiers + DatagramBindingsTypes {}
impl<BC> DatagramBindingsContext for BC where
    BC: RngContext + ReferenceNotifiers + DatagramBindingsTypes
{
}

/// Types and behavior for datagram socket demultiplexing map.
///
/// `I: Ip` describes the type of packets that can be received by sockets in
/// the map.
pub trait DatagramSocketMapSpec<I: Ip, D: DeviceIdentifier, A: SocketMapAddrSpec>:
    SocketMapStateSpec<ListenerId = Self::BoundSocketId, ConnId = Self::BoundSocketId>
    + SocketMapConflictPolicy<
        ListenerAddr<ListenerIpAddr<I::Addr, A::LocalIdentifier>, D>,
        <Self as SocketMapStateSpec>::ListenerSharingState,
        I,
        D,
        A,
    > + SocketMapConflictPolicy<
        ConnAddr<ConnIpAddr<I::Addr, A::LocalIdentifier, A::RemoteIdentifier>, D>,
        <Self as SocketMapStateSpec>::ConnSharingState,
        I,
        D,
        A,
    >
{
    /// The type of IDs stored in a [`BoundSocketMap`] for which this is the
    /// specification.
    ///
    /// This can be the same as [`DatagramSocketSpec::SocketId`] but doesn't
    /// have to be. In the case of
    /// dual-stack sockets, for example, an IPv4 socket will have type
    /// `DatagramSocketSpec::SocketId<Ipv4>` but the IPv4 demultiplexing map
    /// might have `BoundSocketId=Either<DatagramSocketSpec::SocketId<Ipv4>,
    /// DatagramSocketSpec::SocketId<Ipv6>>` to allow looking up IPv6 sockets
    /// when receiving IPv4 packets.
    type BoundSocketId: Clone + Debug;
}

/// A marker trait for dual-stack socket features.
///
/// This trait acts as a marker for [`DualStackBaseIpExt`] for both `Self` and
/// `Self::OtherVersion`.
pub trait DualStackIpExt:
    DualStackBaseIpExt
    + socket::DualStackIpExt<OtherVersion: DualStackBaseIpExt + FilterIpExt + IpLayerIpExt>
{
}

impl<I> DualStackIpExt for I where
    I: DualStackBaseIpExt
        + socket::DualStackIpExt<OtherVersion: DualStackBaseIpExt + FilterIpExt + IpLayerIpExt>
{
}

/// Common features of dual-stack sockets that vary by IP version.
///
/// This trait exists to provide per-IP-version associated types that are
/// useful for implementing dual-stack sockets. The types are intentionally
/// asymmetric - `DualStackIpExt::Xxx` has a different shape for the [`Ipv4`]
/// and [`Ipv6`] impls.
pub trait DualStackBaseIpExt:
    socket::DualStackIpExt + SocketIpExt + netstack3_base::IpExt + FilterIpExt + IpLayerIpExt
{
    /// The type of socket that can receive an IP packet.
    ///
    /// For `Ipv4`, this is [`EitherIpSocket<S>`], and for `Ipv6` it is just
    /// `S::SocketId<Ipv6>`.
    ///
    /// [`EitherIpSocket<S>]`: [EitherIpSocket]
    type DualStackBoundSocketId<D: WeakDeviceIdentifier, S: DatagramSocketSpec>: Clone + Debug + Eq;

    /// The IP options type for the other stack that will be held for a socket.
    ///
    /// For [`Ipv4`], this is `()`, and for [`Ipv6`] it is `State`. For a
    /// protocol like UDP or TCP where the IPv6 socket is dual-stack capable,
    /// the generic state struct can have a field with type
    /// `I::OtherStackIpOptions<Ipv4InIpv6Options>`.
    type OtherStackIpOptions<State: Clone + Debug + Default + Send + Sync>: Clone
        + Debug
        + Default
        + Send
        + Sync;

    /// A listener address for dual-stack operation.
    type DualStackListenerIpAddr<LocalIdentifier: Clone + Debug + Send + Sync + Into<NonZeroU16>>: Clone
        + Debug
        + Send
        + Sync
        + Into<(Option<SpecifiedAddr<Self::Addr>>, NonZeroU16)>;

    /// A connected address for dual-stack operation.
    type DualStackConnIpAddr<S: DatagramSocketSpec>: Clone
        + Debug
        + Into<ConnInfoAddr<Self::Addr, <S::AddrSpec as SocketMapAddrSpec>::RemoteIdentifier>>;

    /// Connection state for a dual-stack socket.
    type DualStackConnState<D: WeakDeviceIdentifier, S: DatagramSocketSpec>: Debug + Send + Sync
    where
        Self::OtherVersion: DualStackBaseIpExt;

    /// Convert a socket ID into a `Self::DualStackBoundSocketId`.
    ///
    /// For coherency reasons this can't be a `From` bound on
    /// `DualStackBoundSocketId`. If more methods are added, consider moving
    /// this to its own dedicated trait that bounds `DualStackBoundSocketId`.
    fn into_dual_stack_bound_socket_id<D: WeakDeviceIdentifier, S: DatagramSocketSpec>(
        id: S::SocketId<Self, D>,
    ) -> Self::DualStackBoundSocketId<D, S>
    where
        Self: IpExt;

    /// Retrieves the associated connection address from the connection state.
    fn conn_addr_from_state<D: WeakDeviceIdentifier, S: DatagramSocketSpec>(
        state: &Self::DualStackConnState<D, S>,
    ) -> ConnAddr<Self::DualStackConnIpAddr<S>, D>
    where
        Self::OtherVersion: DualStackBaseIpExt;
}

/// An IP Socket ID that is either `Ipv4` or `Ipv6`.
#[derive(Derivative)]
#[derivative(
    Clone(bound = ""),
    Debug(bound = ""),
    Eq(bound = "S::SocketId<Ipv4, D>: Eq, S::SocketId<Ipv6, D>: Eq"),
    PartialEq(bound = "S::SocketId<Ipv4, D>: PartialEq, S::SocketId<Ipv6, D>: PartialEq")
)]
#[allow(missing_docs)]
pub enum EitherIpSocket<D: WeakDeviceIdentifier, S: DatagramSocketSpec> {
    V4(S::SocketId<Ipv4, D>),
    V6(S::SocketId<Ipv6, D>),
}

impl<CC, D, S> SocketMetadata<CC> for EitherIpSocket<D, S>
where
    D: WeakDeviceIdentifier,
    S: DatagramSocketSpec,
    S::SocketId<Ipv4, D>: SocketMetadata<CC>,
    S::SocketId<Ipv6, D>: SocketMetadata<CC>,
{
    fn socket_info(&self, core_ctx: &mut CC) -> netstack3_base::socket::SocketInfo {
        match self {
            EitherIpSocket::V4(id) => id.socket_info(core_ctx),
            EitherIpSocket::V6(id) => id.socket_info(core_ctx),
        }
    }
    fn marks(&self, core_ctx: &mut CC) -> Marks {
        match self {
            EitherIpSocket::V4(id) => id.marks(core_ctx),
            EitherIpSocket::V6(id) => id.marks(core_ctx),
        }
    }
}

impl DualStackBaseIpExt for Ipv4 {
    /// Incoming IPv4 packets may be received by either IPv4 or IPv6 sockets.
    type DualStackBoundSocketId<D: WeakDeviceIdentifier, S: DatagramSocketSpec> =
        EitherIpSocket<D, S>;
    type OtherStackIpOptions<State: Clone + Debug + Default + Send + Sync> = ();
    /// IPv4 sockets can't listen on dual-stack addresses.
    type DualStackListenerIpAddr<LocalIdentifier: Clone + Debug + Send + Sync + Into<NonZeroU16>> =
        ListenerIpAddr<Self::Addr, LocalIdentifier>;
    /// IPv4 sockets cannot connect on dual-stack addresses.
    type DualStackConnIpAddr<S: DatagramSocketSpec> = ConnIpAddr<
        Self::Addr,
        <S::AddrSpec as SocketMapAddrSpec>::LocalIdentifier,
        <S::AddrSpec as SocketMapAddrSpec>::RemoteIdentifier,
    >;
    /// IPv4 sockets cannot connect on dual-stack addresses.
    type DualStackConnState<D: WeakDeviceIdentifier, S: DatagramSocketSpec> = ConnState<Self, D, S>;

    fn into_dual_stack_bound_socket_id<D: WeakDeviceIdentifier, S: DatagramSocketSpec>(
        id: S::SocketId<Self, D>,
    ) -> Self::DualStackBoundSocketId<D, S> {
        EitherIpSocket::V4(id)
    }

    fn conn_addr_from_state<D: WeakDeviceIdentifier, S: DatagramSocketSpec>(
        state: &Self::DualStackConnState<D, S>,
    ) -> ConnAddr<Self::DualStackConnIpAddr<S>, D> {
        let ConnState { socket: _, shutdown: _, addr, clear_device_on_disconnect: _, extra: _ } =
            state;
        addr.clone()
    }
}

impl DualStackBaseIpExt for Ipv6 {
    /// Incoming IPv6 packets may only be received by IPv6 sockets.
    type DualStackBoundSocketId<D: WeakDeviceIdentifier, S: DatagramSocketSpec> =
        S::SocketId<Self, D>;
    type OtherStackIpOptions<State: Clone + Debug + Default + Send + Sync> = State;
    /// IPv6 listeners can listen on dual-stack addresses (if the protocol
    /// and socket are dual-stack-enabled).
    type DualStackListenerIpAddr<LocalIdentifier: Clone + Debug + Send + Sync + Into<NonZeroU16>> =
        DualStackListenerIpAddr<Self::Addr, LocalIdentifier>;
    /// IPv6 sockets can connect on dual-stack addresses (if the protocol and
    /// socket are dual-stack-enabled).
    type DualStackConnIpAddr<S: DatagramSocketSpec> = DualStackConnIpAddr<
        Self::Addr,
        <S::AddrSpec as SocketMapAddrSpec>::LocalIdentifier,
        <S::AddrSpec as SocketMapAddrSpec>::RemoteIdentifier,
    >;
    /// IPv6 sockets can connect on dual-stack addresses (if the protocol and
    /// socket are dual-stack-enabled).
    type DualStackConnState<D: WeakDeviceIdentifier, S: DatagramSocketSpec> =
        DualStackConnState<Self, D, S>;

    fn into_dual_stack_bound_socket_id<D: WeakDeviceIdentifier, S: DatagramSocketSpec>(
        id: S::SocketId<Self, D>,
    ) -> Self::DualStackBoundSocketId<D, S> {
        id
    }

    fn conn_addr_from_state<D: WeakDeviceIdentifier, S: DatagramSocketSpec>(
        state: &Self::DualStackConnState<D, S>,
    ) -> ConnAddr<Self::DualStackConnIpAddr<S>, D> {
        match state {
            DualStackConnState::ThisStack(state) => {
                let ConnState { addr, .. } = state;
                let ConnAddr { ip, device } = addr.clone();
                ConnAddr { ip: DualStackConnIpAddr::ThisStack(ip), device }
            }
            DualStackConnState::OtherStack(state) => {
                let ConnState {
                    socket: _,
                    shutdown: _,
                    addr,
                    clear_device_on_disconnect: _,
                    extra: _,
                } = state;
                let ConnAddr { ip, device } = addr.clone();
                ConnAddr { ip: DualStackConnIpAddr::OtherStack(ip), device }
            }
        }
    }
}

#[derive(GenericOverIp)]
#[generic_over_ip(I, Ip)]
/// A wrapper to make [`DualStackIpExt::OtherStackIpOptions`] [`GenericOverIp`].
pub struct WrapOtherStackIpOptions<
    'a,
    I: DualStackIpExt,
    S: 'a + Clone + Debug + Default + Send + Sync,
>(pub &'a I::OtherStackIpOptions<S>);

#[derive(GenericOverIp)]
#[generic_over_ip(I, Ip)]
/// A wrapper to make [`DualStackIpExt::OtherStackIpOptions`] [`GenericOverIp`].
pub struct WrapOtherStackIpOptionsMut<
    'a,
    I: DualStackIpExt,
    S: 'a + Clone + Debug + Default + Send + Sync,
>(pub &'a mut I::OtherStackIpOptions<S>);

/// Types and behavior for datagram sockets.
///
/// These sockets may or may not support dual-stack operation.
pub trait DatagramSocketSpec: Sized + 'static {
    /// Name of this datagram protocol.
    const NAME: &'static str;

    /// The socket address spec for the datagram socket type.
    ///
    /// This describes the types of identifiers the socket uses, e.g.
    /// local/remote port for UDP.
    type AddrSpec: SocketMapAddrSpec;

    /// Identifier for an individual socket for a given IP version.
    ///
    /// Corresponds uniquely to a socket resource. This is the type that will
    /// be returned by [`create`] and used to identify which socket is being
    /// acted on by calls like [`listen`], [`connect`], [`remove`], etc.
    type SocketId<I: IpExt, D: WeakDeviceIdentifier>: Clone
        + Debug
        + Eq
        + Send
        + Borrow<StrongRc<I, D, Self>>
        + From<StrongRc<I, D, Self>>;

    /// The weak version of `SocketId`.
    type WeakSocketId<I: IpExt, D: WeakDeviceIdentifier>: Clone + Debug + Eq + Send;

    /// IP-level options for sending `I::OtherVersion` IP packets.
    type OtherStackIpOptions<I: IpExt, D: WeakDeviceIdentifier>: Clone
        + Debug
        + Default
        + Send
        + Sync;

    /// The type of a listener IP address.
    ///
    /// For dual-stack-capable datagram protocols like UDP, this should use
    /// [`DualStackIpExt::ListenerIpAddr`], which will be one of
    /// [`ListenerIpAddr`] or [`DualStackListenerIpAddr`].
    /// Non-dual-stack-capable protocols (like ICMP and raw IP sockets) should
    /// just use [`ListenerIpAddr`].
    type ListenerIpAddr<I: IpExt>: Clone
        + Debug
        + Into<(Option<SpecifiedAddr<I::Addr>>, NonZeroU16)>
        + Send
        + Sync
        + 'static;

    /// The sharing state for a socket.
    ///
    /// NB: The underlying [`BoundSocketMap`]` uses separate types for the
    /// sharing state of connected vs listening sockets. At the moment, datagram
    /// sockets have no need for differentiated sharing states, so consolidate
    /// them under one type.
    type SharingState: Clone + Debug + Default + Send + Sync + 'static;

    /// The type of an IP address for a connected socket.
    ///
    /// For dual-stack-capable datagram protocols like UDP, this should use
    /// [`DualStackIpExt::ConnIpAddr`], which will be one of
    /// [`ConnIpAddr`] or [`DualStackConnIpAddr`].
    /// Non-dual-stack-capable protocols (like ICMP and raw IP sockets) should
    /// just use [`ConnIpAddr`].
    type ConnIpAddr<I: IpExt>: Clone
        + Debug
        + Into<ConnInfoAddr<I::Addr, <Self::AddrSpec as SocketMapAddrSpec>::RemoteIdentifier>>;

    /// The type of a state held by a connected socket.
    ///
    /// For dual-stack-capable datagram protocols like UDP, this should use
    /// [`DualStackIpExt::ConnState`], which will be one of [`ConnState`] or
    /// [`DualStackConnState`]. Non-dual-stack-capable protocols (like ICMP and
    /// raw IP sockets) should just use [`ConnState`].
    type ConnState<I: IpExt, D: WeakDeviceIdentifier>: Debug + Send + Sync;

    /// The extra state that a connection state want to remember.
    ///
    /// For example: UDP sockets does not have any extra state to remember, so
    /// it should just be `()`; ICMP sockets need to remember the remote ID the
    /// socket is 'connected' to, the remote ID is not used when sending nor
    /// participating in the demuxing decisions. So it will be stored in the
    /// extra state so that it can be retrieved later, i.e, it should be
    /// `NonZeroU16` for ICMP sockets.
    type ConnStateExtra: Debug + Send + Sync;

    /// The specification for the [`BoundSocketMap`] for a given IP version.
    ///
    /// Describes the per-address and per-socket values held in the
    /// demultiplexing map for a given IP version.
    type SocketMapSpec<I: IpExt + DualStackIpExt, D: WeakDeviceIdentifier>: DatagramSocketMapSpec<
            I,
            D,
            Self::AddrSpec,
            ListenerSharingState = Self::SharingState,
            ConnSharingState = Self::SharingState,
        >;

    /// External data kept by datagram sockets.
    ///
    /// This is used to store opaque bindings data alongside the core data
    /// inside the socket references.
    type ExternalData<I: Ip>: Debug + Send + Sync + 'static;

    /// Settings type offered by bindings for this datagram socket.
    type Settings: AsRef<DatagramSettings> + Default;

    /// Per-socket counters tracked by datagram sockets.
    type Counters<I: Ip>: Debug + Default + Send + Sync + 'static;

    /// The listener type that is notified about the socket writable state.
    type SocketWritableListener: SocketWritableListener + Debug + Send + Sync + 'static;

    /// A token representing resources allocated for an in-flight send operation.
    ///
    /// Core retains this token until the packet has either been transmitted by
    /// the device or dropped along the egress path. This allows bindings to
    /// track send buffer capacity or other per-packet resources.
    type SendToken: Debug + Send + Sync + 'static;

    /// The size in bytes of the fixed header for the datagram transport.
    ///
    /// This is used to calculate the per-packet send buffer cost of an egress
    /// datagram.
    ///
    /// This value must be the _additional_ bytes wrapped in a body when
    /// [`DatagramSocketSpec::make_packet`] is called.
    const FIXED_HEADER_SIZE: usize;

    /// Returns the IP protocol of this datagram specification.
    fn ip_proto<I: IpProtoExt>() -> I::Proto;

    /// Converts [`Self::SocketId`] to [`DatagramSocketMapSpec::BoundSocketId`].
    ///
    /// Constructs a socket identifier to its in-demultiplexing map form. For
    /// protocols with dual-stack sockets, like UDP, implementations should
    /// perform a transformation. Otherwise it should be the identity function.
    fn make_bound_socket_map_id<I: IpExt, D: WeakDeviceIdentifier>(
        s: &Self::SocketId<I, D>,
    ) -> <Self::SocketMapSpec<I, D> as DatagramSocketMapSpec<I, D, Self::AddrSpec>>::BoundSocketId;

    /// The type of serializer returned by [`DatagramSocketSpec::make_packet`]
    /// for a given IP version and buffer type.
    type Serializer<I: IpExt, B: BufferMut>: TransportPacketSerializer<I, Buffer = B>;
    /// The potential error for serializing a packet. For example, in UDP, this
    /// should be infallible but for ICMP, there will be an error if the input
    /// is not an echo request.
    type SerializeError: Error;

    /// Constructs a packet serializer with `addr` and `body`.
    fn make_packet<I: IpExt, B: BufferMut>(
        body: B,
        addr: &ConnIpAddr<
            I::Addr,
            <Self::AddrSpec as SocketMapAddrSpec>::LocalIdentifier,
            <Self::AddrSpec as SocketMapAddrSpec>::RemoteIdentifier,
        >,
    ) -> Result<Self::Serializer<I, B>, Self::SerializeError>;

    /// Attempts to allocate a local identifier for a listening socket.
    ///
    /// Returns the identifier on success, or `None` on failure.
    fn try_alloc_listen_identifier<I: IpExt, D: WeakDeviceIdentifier>(
        rng: &mut impl RngContext,
        is_available: impl Fn(
            <Self::AddrSpec as SocketMapAddrSpec>::LocalIdentifier,
        ) -> Result<(), InUseError>,
    ) -> Option<<Self::AddrSpec as SocketMapAddrSpec>::LocalIdentifier>;

    /// Retrieves the associated connection info from the connection state.
    fn conn_info_from_state<I: IpExt, D: WeakDeviceIdentifier>(
        state: &Self::ConnState<I, D>,
    ) -> ConnInfo<I::Addr, D>;

    /// Tries to allocate a local identifier.
    fn try_alloc_local_id<I: IpExt, D: WeakDeviceIdentifier, BC: RngContext>(
        bound: &BoundDatagramSocketMap<I, D, Self>,
        bindings_ctx: &mut BC,
        flow: DatagramFlowId<I::Addr, <Self::AddrSpec as SocketMapAddrSpec>::RemoteIdentifier>,
    ) -> Option<<Self::AddrSpec as SocketMapAddrSpec>::LocalIdentifier>;

    /// Downgrades a `SocketId` into a `WeakSocketId`.
    // TODO(https://fxbug.dev/392672414): Replace this with a base trait.
    fn downgrade_socket_id<I: IpExt, D: WeakDeviceIdentifier>(
        id: &Self::SocketId<I, D>,
    ) -> Self::WeakSocketId<I, D>;

    /// Attempts to upgrade a `WeakSocketId` into a `SocketId`.
    // TODO(https://fxbug.dev/392672414): Replace this with a base trait.
    fn upgrade_socket_id<I: IpExt, D: WeakDeviceIdentifier>(
        id: &Self::WeakSocketId<I, D>,
    ) -> Option<Self::SocketId<I, D>>;
}

/// The error returned when an identifier (i.e.) port is already in use.
pub struct InUseError;

/// Creates a primary ID without inserting it into the all socket map.
pub fn create_primary_id<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec>(
    external_data: S::ExternalData<I>,
    writable_listener: S::SocketWritableListener,
    settings: &DatagramSettings,
) -> PrimaryRc<I, D, S> {
    PrimaryRc::new(ReferenceState {
        state: RwLock::new(SocketState {
            inner: SocketStateInner::Unbound(UnboundSocketState::default()),
            ip_options: Default::default(),
            sharing: Default::default(),
        }),
        external_data,
        send_buffer: SendBufferTracking::new(writable_listener, settings),
        counters: Default::default(),
    })
}

/// Information associated with a datagram listener.
#[derive(GenericOverIp, Debug, Eq, PartialEq)]
#[generic_over_ip(A, IpAddress)]
pub struct ListenerInfo<A: IpAddress, D> {
    /// The local address associated with a datagram listener, or `None` for any
    /// address.
    pub local_ip: Option<StrictlyZonedAddr<A, SpecifiedAddr<A>, D>>,
    /// The local port associated with a datagram listener.
    pub local_identifier: NonZeroU16,
}

impl<A: IpAddress, LA: Into<(Option<SpecifiedAddr<A>>, NonZeroU16)>, D> From<ListenerAddr<LA, D>>
    for ListenerInfo<A, D>
{
    fn from(ListenerAddr { ip, device }: ListenerAddr<LA, D>) -> Self {
        let (addr, local_identifier) = ip.into();
        Self {
            local_ip: addr.map(|addr| {
                StrictlyZonedAddr::new_with_zone(addr, || {
                    // The invariant that a zone is present if needed is upheld by
                    // set_bindtodevice and bind.
                    device.expect("device must be bound for addresses that require zones")
                })
            }),
            local_identifier,
        }
    }
}

impl<A: IpAddress, D> From<NonZeroU16> for ListenerInfo<A, D> {
    fn from(local_identifier: NonZeroU16) -> Self {
        Self { local_ip: None, local_identifier }
    }
}

/// Information associated with a datagram connection.
#[derive(Debug, GenericOverIp, PartialEq)]
#[generic_over_ip(A, IpAddress)]
pub struct ConnInfo<A: IpAddress, D> {
    /// The local address associated with a datagram connection.
    pub local_ip: StrictlyZonedAddr<A, SpecifiedAddr<A>, D>,
    /// The local identifier associated with a datagram connection.
    pub local_identifier: NonZeroU16,
    /// The remote address associated with a datagram connection.
    pub remote_ip: StrictlyZonedAddr<A, SpecifiedAddr<A>, D>,
    /// The remote identifier associated with a datagram connection.
    pub remote_identifier: u16,
}

impl<A: IpAddress, D> ConnInfo<A, D> {
    /// Construct a new `ConnInfo`.
    pub fn new(
        local_ip: SpecifiedAddr<A>,
        local_identifier: NonZeroU16,
        remote_ip: SpecifiedAddr<A>,
        remote_identifier: u16,
        mut get_zone: impl FnMut() -> D,
    ) -> Self {
        Self {
            local_ip: StrictlyZonedAddr::new_with_zone(local_ip, &mut get_zone),
            local_identifier,
            remote_ip: StrictlyZonedAddr::new_with_zone(remote_ip, &mut get_zone),
            remote_identifier,
        }
    }
}

/// Information about the addresses for a socket.
#[derive(GenericOverIp, Debug, PartialEq)]
#[generic_over_ip(A, IpAddress)]
pub enum SocketInfo<A: IpAddress, D> {
    /// The socket is not bound.
    Unbound,
    /// The socket is listening.
    Listener(ListenerInfo<A, D>),
    /// The socket is connected.
    Connected(ConnInfo<A, D>),
}

/// A type of an operation that can be performed on a socket entry.
trait EntryOperationType {
    type ReverseOp: EntryOperationType<ReverseOp = Self>;
    type Error: Debug;

    /// Applies the operation to the bound socket map. Returns the reverse operation.
    fn apply<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec>(
        op: SocketEntryOp<I, D, S, Self>,
        sockets: &mut BoundDatagramSocketMap<I, D, S>,
    ) -> Result<SocketEntryOp<I, D, S, Self::ReverseOp>, Self::Error>;
}

enum EntryInsertOp {}
impl EntryOperationType for EntryInsertOp {
    type ReverseOp = EntryRemoveOp;
    type Error = InsertError;

    fn apply<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec>(
        op: SocketEntryOp<I, D, S, Self>,
        sockets: &mut BoundDatagramSocketMap<I, D, S>,
    ) -> Result<SocketEntryOp<I, D, S, Self::ReverseOp>, Self::Error> {
        let SocketEntryOp { socket_id, sharing, addr, _marker } = op;
        match &addr {
            AddrVec::Listen(addr) => {
                let SocketStateEntry { .. } = sockets.listeners_mut().try_insert(
                    addr.clone(),
                    sharing.clone(),
                    socket_id.clone(),
                )?;
            }
            AddrVec::Conn(addr) => {
                let SocketStateEntry { .. } = sockets.conns_mut().try_insert(
                    addr.clone(),
                    sharing.clone(),
                    socket_id.clone(),
                )?;
            }
        };
        Ok(SocketEntryOp { socket_id, sharing, addr, _marker: PhantomData })
    }
}

enum EntryRemoveOp {}
impl EntryOperationType for EntryRemoveOp {
    type ReverseOp = EntryInsertOp;
    type Error = NotFoundError;

    fn apply<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec>(
        op: SocketEntryOp<I, D, S, Self>,
        sockets: &mut BoundDatagramSocketMap<I, D, S>,
    ) -> Result<SocketEntryOp<I, D, S, Self::ReverseOp>, Self::Error> {
        let SocketEntryOp { socket_id, sharing, addr, _marker } = op;
        match &addr {
            AddrVec::Listen(addr) => {
                sockets.listeners_mut().remove(&socket_id, &addr)?;
            }
            AddrVec::Conn(addr) => {
                sockets.conns_mut().remove(&socket_id, &addr)?;
            }
        };
        Ok(SocketEntryOp { socket_id, sharing, addr, _marker: PhantomData })
    }
}

/// State associated with insertion or removal operations on the bound socket map.
struct SocketEntryOp<
    I: IpExt,
    D: WeakDeviceIdentifier,
    S: DatagramSocketSpec,
    O: EntryOperationType + ?Sized,
> {
    socket_id: <S::SocketMapSpec<I, D> as DatagramSocketMapSpec<I, D, S::AddrSpec>>::BoundSocketId,
    sharing: S::SharingState,
    addr: AddrVec<I, D, S::AddrSpec>,
    _marker: PhantomData<O>,
}

type SingleStackRemoveOperation<I, D, S> = SocketEntryOp<I, D, S, EntryRemoveOp>;
type SingleStackInsertOperation<I, D, S> = SocketEntryOp<I, D, S, EntryInsertOp>;

impl<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec> SingleStackRemoveOperation<I, D, S> {
    /// Constructs the remove operation from existing socket state.
    fn new_from_state<BC, CC: NonDualStackDatagramBoundStateContext<I, BC, S, WeakDeviceId = D>>(
        core_ctx: &mut CC,
        socket_id: &S::SocketId<I, D>,
        state: &BoundSocketState<I, D, S>,
        sharing: S::SharingState,
    ) -> Self {
        let BoundSocketState { socket_type: state, original_bound_addr: _ } = state;
        match state {
            BoundSocketStateType::Listener(ListenerState { addr: ListenerAddr { ip, device } }) => {
                Self {
                    addr: AddrVec::Listen(ListenerAddr {
                        ip: core_ctx.nds_converter().convert(ip.clone()),
                        device: device.clone(),
                    }),
                    sharing,
                    socket_id: S::make_bound_socket_map_id(socket_id),
                    _marker: PhantomData,
                }
            }
            BoundSocketStateType::Connected(state) => {
                let ConnState {
                    addr,
                    socket: _,
                    clear_device_on_disconnect: _,
                    shutdown: _,
                    extra: _,
                } = core_ctx.nds_converter().convert(state);
                Self {
                    addr: AddrVec::Conn(addr.clone()),
                    sharing,
                    socket_id: S::make_bound_socket_map_id(socket_id),
                    _marker: PhantomData,
                }
            }
        }
    }
}

impl<I, D, S, O> SocketEntryOp<I, D, S, O>
where
    I: IpExt,
    D: WeakDeviceIdentifier,
    S: DatagramSocketSpec,
    O: EntryOperationType,
{
    /// Applies the operation and returns the reverse operation.
    fn apply(
        self,
        sockets: &mut BoundDatagramSocketMap<I, D, S>,
    ) -> Result<SocketEntryOp<I, D, S, O::ReverseOp>, O::Error> {
        O::apply(self, sockets)
    }
}

struct DualStackListenerOp<I, D, S, O>
where
    I: IpExt,
    D: WeakDeviceIdentifier,
    S: DatagramSocketSpec,
    O: EntryOperationType,
{
    identifier: <S::AddrSpec as SocketMapAddrSpec>::LocalIdentifier,
    device: Option<D>,
    sharing: S::SharingState,
    socket_ids: PairedBoundSocketIds<I, D, S>,
    _marker: PhantomData<O>,
}

type DualStackListenerRemoveOperation<I, D, S> = DualStackListenerOp<I, D, S, EntryRemoveOp>;
type DualStackListenerInsertOperation<I, D, S> = DualStackListenerOp<I, D, S, EntryInsertOp>;

impl<I, D, S, O> DualStackListenerOp<I, D, S, O>
where
    I: IpExt,
    D: WeakDeviceIdentifier,
    S: DatagramSocketSpec,
    O: EntryOperationType,
{
    fn this_stack_op(&self) -> SocketEntryOp<I, D, S, O> {
        SocketEntryOp {
            addr: AddrVec::Listen(ListenerAddr {
                ip: ListenerIpAddr { addr: None, identifier: self.identifier },
                device: self.device.clone(),
            }),
            sharing: self.sharing.clone(),
            socket_id: self.socket_ids.this.clone(),
            _marker: PhantomData,
        }
    }

    fn other_stack_op(&self) -> SocketEntryOp<I::OtherVersion, D, S, O> {
        SocketEntryOp {
            addr: AddrVec::Listen(ListenerAddr {
                ip: ListenerIpAddr { addr: None, identifier: self.identifier },
                device: self.device.clone(),
            }),
            sharing: self.sharing.clone(),
            socket_id: self.socket_ids.other.clone(),
            _marker: PhantomData,
        }
    }

    /// Apply this operation to the given `BoundDatagramSocketMap`s. Returns the reverse
    /// operation.
    fn apply(
        self,
        sockets: &mut BoundDatagramSocketMap<I, D, S>,
        other_sockets: &mut BoundDatagramSocketMap<I::OtherVersion, D, S>,
    ) -> Result<DualStackListenerOp<I, D, S, O::ReverseOp>, O::Error> {
        let this_stack_reverse_op = self.this_stack_op().apply(sockets)?;
        match self.other_stack_op().apply(other_sockets) {
            Ok(SocketEntryOp::<_, _, _, O::ReverseOp> { .. }) => (),
            Err(e) => {
                let _: SocketEntryOp<_, _, _, O> = this_stack_reverse_op
                    .apply(sockets)
                    .expect("Failed to revert socket map operation");
                return Err(e);
            }
        };

        let Self { identifier, device, sharing, socket_ids, _marker: _ } = self;
        Ok(DualStackListenerOp { identifier, device, sharing, socket_ids, _marker: PhantomData })
    }
}

/// State associated with a dual-stack socket entry operation.
enum DualStackSocketEntryOp<I, D, S, O>
where
    I: IpExt,
    D: WeakDeviceIdentifier,
    S: DatagramSocketSpec,
    O: EntryOperationType,
{
    CurrentStack(SocketEntryOp<I, D, S, O>),
    OtherStack(SocketEntryOp<I::OtherVersion, D, S, O>),
    ListenerBothStacks(DualStackListenerOp<I, D, S, O>),
}

type DualStackRemoveOperation<I, D, S> = DualStackSocketEntryOp<I, D, S, EntryRemoveOp>;
type DualStackInsertOperation<I, D, S> = DualStackSocketEntryOp<I, D, S, EntryInsertOp>;

impl<I, D, S> DualStackRemoveOperation<I, D, S>
where
    I: IpExt,
    D: WeakDeviceIdentifier,
    S: DatagramSocketSpec,
{
    /// Constructs the removal operation from existing socket state.
    fn new_from_state<BC, CC>(
        core_ctx: &mut CC,
        socket_id: &S::SocketId<I, D>,
        ip_options: &IpOptions<I, D, S>,
        state: &BoundSocketState<I, D, S>,
        sharing: S::SharingState,
    ) -> Self
    where
        BC: DatagramBindingsTypes,
        CC: DualStackDatagramBoundStateContext<I, BC, S, WeakDeviceId = D>,
    {
        let BoundSocketState { socket_type: state, original_bound_addr: _ } = state;
        match state {
            BoundSocketStateType::Listener(ListenerState { addr }) => {
                let ListenerAddr { ip, device } = addr.clone();
                match (core_ctx.ds_converter().convert(ip), core_ctx.dual_stack_enabled(ip_options))
                {
                    // Dual-stack enabled, bound in both stacks.
                    (DualStackListenerIpAddr::BothStacks(identifier), true) => {
                        DualStackSocketEntryOp::ListenerBothStacks(DualStackListenerOp {
                            identifier: identifier.clone(),
                            device,
                            sharing,
                            socket_ids: PairedBoundSocketIds {
                                this: S::make_bound_socket_map_id(socket_id),
                                other: core_ctx.to_other_bound_socket_id(socket_id),
                            },
                            _marker: PhantomData,
                        })
                    }
                    // Bound in this stack, with/without dual-stack enabled.
                    (DualStackListenerIpAddr::ThisStack(addr), true | false) => {
                        DualStackSocketEntryOp::CurrentStack(SocketEntryOp {
                            addr: AddrVec::Listen(ListenerAddr { ip: addr, device }),
                            sharing,
                            socket_id: S::make_bound_socket_map_id(socket_id),
                            _marker: PhantomData,
                        })
                    }
                    // Dual-stack enabled, bound only in the other stack.
                    (DualStackListenerIpAddr::OtherStack(addr), true) => {
                        DualStackSocketEntryOp::OtherStack(SocketEntryOp {
                            addr: AddrVec::Listen(ListenerAddr { ip: addr, device }),
                            sharing,
                            socket_id: core_ctx.to_other_bound_socket_id(socket_id),
                            _marker: PhantomData,
                        })
                    }
                    (DualStackListenerIpAddr::OtherStack(_), false)
                    | (DualStackListenerIpAddr::BothStacks(_), false) => {
                        unreachable!("dual-stack disabled socket cannot use the other stack")
                    }
                }
            }
            BoundSocketStateType::Connected(state) => {
                match core_ctx.ds_converter().convert(state) {
                    DualStackConnState::ThisStack(ConnState { addr, .. }) => {
                        DualStackSocketEntryOp::CurrentStack(SocketEntryOp {
                            addr: AddrVec::Conn(addr.clone()),
                            sharing,
                            socket_id: S::make_bound_socket_map_id(socket_id),
                            _marker: PhantomData,
                        })
                    }
                    DualStackConnState::OtherStack(ConnState { addr, .. }) => {
                        core_ctx.assert_dual_stack_enabled(&ip_options);
                        DualStackSocketEntryOp::OtherStack(SocketEntryOp {
                            addr: AddrVec::Conn(addr.clone()),
                            sharing,
                            socket_id: core_ctx.to_other_bound_socket_id(socket_id),
                            _marker: PhantomData,
                        })
                    }
                }
            }
        }
    }
}

impl<I, D, S, O> DualStackSocketEntryOp<I, D, S, O>
where
    I: IpExt,
    D: WeakDeviceIdentifier,
    S: DatagramSocketSpec,
    O: EntryOperationType,
{
    /// Apply this operation to the given `BoundSocketMap`s. Returns the reverse
    /// operation.
    fn apply(
        self,
        sockets: &mut BoundDatagramSocketMap<I, D, S>,
        other_sockets: &mut BoundDatagramSocketMap<I::OtherVersion, D, S>,
    ) -> Result<DualStackSocketEntryOp<I, D, S, O::ReverseOp>, O::Error> {
        let result = match self {
            DualStackSocketEntryOp::CurrentStack(remove) => {
                DualStackSocketEntryOp::CurrentStack(remove.apply(sockets)?)
            }
            DualStackSocketEntryOp::OtherStack(remove) => {
                DualStackSocketEntryOp::OtherStack(remove.apply(other_sockets)?)
            }
            DualStackSocketEntryOp::ListenerBothStacks(listener_op) => {
                DualStackSocketEntryOp::ListenerBothStacks(
                    listener_op.apply(sockets, other_sockets)?,
                )
            }
        };
        Ok(result)
    }
}

/// Abstraction for operations over one or two demultiplexing maps.
trait BoundStateHandler<I: IpExt, S: DatagramSocketSpec, D: WeakDeviceIdentifier> {
    /// The type of address that can be inserted or removed for listeners.
    type ListenerAddr: Clone;
    /// The type of ID that can be inserted or removed.
    type BoundSocketId;

    /// Checks whether an entry could be inserted for the specified address and
    /// identifier.
    ///
    /// Returns `true` if a value could be inserted at the specified address and
    /// local ID, with the provided sharing state; otherwise returns `false`.
    fn is_listener_entry_available(
        &self,
        addr: Self::ListenerAddr,
        identifier: <S::AddrSpec as SocketMapAddrSpec>::LocalIdentifier,
        sharing_state: &S::SharingState,
    ) -> bool;

    /// Inserts `id` at a listener address or returns an error.
    ///
    /// Inserts the identifier `id` at the listener address for `addr` and
    /// local `identifier` with device `device` and the given sharing state. If
    /// the insertion conflicts with an existing socket, a `LocalAddressError`
    /// is returned.
    fn try_insert_listener(
        &mut self,
        addr: Self::ListenerAddr,
        identifier: <S::AddrSpec as SocketMapAddrSpec>::LocalIdentifier,
        device: Option<D>,
        sharing: S::SharingState,
        id: Self::BoundSocketId,
    ) -> Result<(), LocalAddressError>;
}

/// An alias for `BoundSocketMap` specialized for datagram sockets.
pub type BoundDatagramSocketMap<I, D, S> = BoundSocketMap<
    I,
    D,
    <S as DatagramSocketSpec>::AddrSpec,
    <S as DatagramSocketSpec>::SocketMapSpec<I, D>,
>;

type BoundDatagramSocketId<I, D, S> =
    <<S as DatagramSocketSpec>::SocketMapSpec<I, D> as DatagramSocketMapSpec<
        I,
        D,
        <S as DatagramSocketSpec>::AddrSpec,
    >>::BoundSocketId;

/// A sentinel type for the unspecified address in a dual-stack context.
///
/// This is kind of like [`Ipv6::UNSPECIFIED_ADDRESS`], but makes it clear that
/// the value is being used in a dual-stack context.
#[derive(Copy, Clone, Debug)]
struct DualStackUnspecifiedAddr;

/// Implementation of BoundStateHandler for a single demultiplexing map.
impl<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec> BoundStateHandler<I, S, D>
    for BoundDatagramSocketMap<I, D, S>
{
    type ListenerAddr = Option<SocketIpAddr<I::Addr>>;
    type BoundSocketId = BoundDatagramSocketId<I, D, S>;

    fn is_listener_entry_available(
        &self,
        addr: Self::ListenerAddr,
        identifier: <S::AddrSpec as SocketMapAddrSpec>::LocalIdentifier,
        sharing: &S::SharingState,
    ) -> bool {
        let check_addr = ListenerAddr { device: None, ip: ListenerIpAddr { identifier, addr } };
        match self.listeners().could_insert(&check_addr, sharing) {
            Ok(()) => true,
            Err(
                InsertError::Exists
                | InsertError::IndirectConflict
                | InsertError::ShadowAddrExists
                | InsertError::WouldShadowExisting,
            ) => false,
        }
    }

    fn try_insert_listener(
        &mut self,
        addr: Self::ListenerAddr,
        identifier: <S::AddrSpec as SocketMapAddrSpec>::LocalIdentifier,
        device: Option<D>,
        sharing: S::SharingState,
        id: Self::BoundSocketId,
    ) -> Result<(), LocalAddressError> {
        let _: SocketStateEntry<'_, _, _, _, _, _> = self
            .listeners_mut()
            .try_insert(
                ListenerAddr { ip: ListenerIpAddr { addr, identifier }, device },
                sharing,
                id,
            )
            .map_err(Into::<LocalAddressError>::into)?;

        Ok(())
    }
}

struct PairedSocketMapMut<'a, I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec> {
    bound: &'a mut BoundDatagramSocketMap<I, D, S>,
    other_bound: &'a mut BoundDatagramSocketMap<I::OtherVersion, D, S>,
}

#[derive(Derivative)]
#[derivative(Clone(bound = ""))]
struct PairedBoundSocketIds<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec> {
    this: BoundDatagramSocketId<I, D, S>,
    other: BoundDatagramSocketId<I::OtherVersion, D, S>,
}

/// Implementation for a pair of demultiplexing maps for different IP versions.
impl<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec> BoundStateHandler<I, S, D>
    for PairedSocketMapMut<'_, I, D, S>
{
    type ListenerAddr = DualStackUnspecifiedAddr;
    type BoundSocketId = PairedBoundSocketIds<I, D, S>;

    fn is_listener_entry_available(
        &self,
        DualStackUnspecifiedAddr: Self::ListenerAddr,
        identifier: <S::AddrSpec as SocketMapAddrSpec>::LocalIdentifier,
        sharing: &S::SharingState,
    ) -> bool {
        let PairedSocketMapMut { bound, other_bound } = self;
        BoundStateHandler::<I, S, D>::is_listener_entry_available(*bound, None, identifier, sharing)
            && BoundStateHandler::<I::OtherVersion, S, D>::is_listener_entry_available(
                *other_bound,
                None,
                identifier,
                sharing,
            )
    }

    fn try_insert_listener(
        &mut self,
        DualStackUnspecifiedAddr: Self::ListenerAddr,
        identifier: <S::AddrSpec as SocketMapAddrSpec>::LocalIdentifier,
        device: Option<D>,
        sharing: S::SharingState,
        id: Self::BoundSocketId,
    ) -> Result<(), LocalAddressError> {
        let PairedSocketMapMut { bound: this, other_bound: other } = self;

        let op = DualStackListenerInsertOperation {
            identifier,
            device,
            sharing,
            socket_ids: id,
            _marker: PhantomData,
        };

        let _: DualStackListenerRemoveOperation<I, D, S> =
            op.apply(this, other).map_err(Into::<LocalAddressError>::into)?;

        Ok(())
    }
}

fn try_pick_identifier<
    I: IpExt,
    S: DatagramSocketSpec,
    D: WeakDeviceIdentifier,
    BS: BoundStateHandler<I, S, D>,
    BC: RngContext,
>(
    addr: BS::ListenerAddr,
    bound: &BS,
    bindings_ctx: &mut BC,
    sharing: &S::SharingState,
) -> Option<<S::AddrSpec as SocketMapAddrSpec>::LocalIdentifier> {
    S::try_alloc_listen_identifier::<I, D>(bindings_ctx, move |identifier| {
        bound
            .is_listener_entry_available(addr.clone(), identifier, sharing)
            .then_some(())
            .ok_or(InUseError)
    })
}

fn try_pick_bound_address<
    I: IpExt,
    CC: TransportIpContext<I, BC>,
    BC: DatagramBindingsTypes,
    LI,
>(
    addr: Option<ZonedAddr<SocketIpAddr<I::Addr>, CC::DeviceId>>,
    device: &Option<CC::WeakDeviceId>,
    core_ctx: &mut CC,
    identifier: LI,
    transparent: bool,
) -> Result<
    (Option<SocketIpAddr<I::Addr>>, Option<EitherDeviceId<CC::DeviceId, CC::WeakDeviceId>>, LI),
    LocalAddressError,
> {
    let (addr, device, identifier) = match addr {
        Some(addr) => {
            // Extract the specified address and the device. The device
            // is either the one from the address or the one to which
            // the socket was previously bound.
            let (addr, device) = addr.resolve_addr_with_device(device.clone())?;

            // Binding to multicast addresses is allowed regardless.
            // Other addresses can only be bound to if they are assigned
            // to the device, or if the socket is transparent.
            if !addr.addr().is_multicast() && !transparent {
                BaseTransportIpContext::<I, _>::with_devices_with_assigned_addr(
                    core_ctx,
                    addr.into(),
                    |mut assigned_to| {
                        if let Some(device) = &device {
                            if !assigned_to.any(|d| device == &EitherDeviceId::Strong(d)) {
                                return Err(LocalAddressError::AddressMismatch);
                            }
                        } else {
                            if !assigned_to.any(|_: CC::DeviceId| true) {
                                return Err(LocalAddressError::CannotBindToAddress);
                            }
                        }
                        Ok(())
                    },
                )?;
            }
            (Some(addr), device, identifier)
        }
        None => (None, device.clone().map(EitherDeviceId::Weak), identifier),
    };
    Ok((addr, device, identifier))
}

fn listen_inner<
    I: IpExt,
    BC: DatagramBindingsContext,
    CC: DatagramBoundStateContext<I, BC, S>,
    S: DatagramSocketSpec,
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    state: &mut SocketState<I, CC::WeakDeviceId, S>,
    id: &S::SocketId<I, CC::WeakDeviceId>,
    addr: Option<ZonedAddr<SpecifiedAddr<I::Addr>, CC::DeviceId>>,
    local_id: Option<<S::AddrSpec as SocketMapAddrSpec>::LocalIdentifier>,
) -> Result<(), Either<ExpectedUnboundError, LocalAddressError>> {
    /// Possible operations that might be performed, depending on whether the
    /// socket state spec supports dual-stack operation and what the address
    /// looks like.
    #[derive(Debug, GenericOverIp)]
    #[generic_over_ip(I, Ip)]
    enum BoundOperation<'a, I: IpExt, DS: DeviceIdContext<AnyDevice>, NDS> {
        /// Bind to the "any" address on both stacks.
        DualStackAnyAddr(&'a mut DS),
        /// Bind to a non-dual-stack address only on the current stack.
        OnlyCurrentStack(
            MaybeDualStack<&'a mut DS, &'a mut NDS>,
            Option<ZonedAddr<SocketIpAddr<I::Addr>, DS::DeviceId>>,
        ),
        /// Bind to an address only on the other stack.
        OnlyOtherStack(
            &'a mut DS,
            Option<ZonedAddr<SocketIpAddr<<I::OtherVersion as Ip>::Addr>, DS::DeviceId>>,
        ),
    }

    let SocketState { inner, ip_options, sharing } = state;
    let UnboundSocketState { device } = match inner {
        SocketStateInner::Unbound(state) => state,
        SocketStateInner::Bound(_) => return Err(Either::Left(ExpectedUnboundError)),
    };

    let dual_stack = core_ctx.dual_stack_context_mut();
    let bound_operation: BoundOperation<'_, I, _, _> = match (dual_stack, addr) {
        // Dual-stack support and unspecified address.
        (MaybeDualStack::DualStack(dual_stack), None) => {
            match dual_stack.dual_stack_enabled(ip_options) {
                // Socket is dual-stack enabled, bind in both stacks.
                true => BoundOperation::DualStackAnyAddr(dual_stack),
                // Dual-stack support but not enabled, so bind unspecified in the
                // current stack.
                false => {
                    BoundOperation::OnlyCurrentStack(MaybeDualStack::DualStack(dual_stack), None)
                }
            }
        }
        // There is dual-stack support and the address is not unspecified so how
        // to proceed is going to depend on the value of `addr`.
        (MaybeDualStack::DualStack(dual_stack), Some(addr)) => {
            match DualStackLocalIp::<I, _>::new(addr) {
                // `addr` can't be represented in the other stack.
                DualStackLocalIp::ThisStack(addr) => BoundOperation::OnlyCurrentStack(
                    MaybeDualStack::DualStack(dual_stack),
                    Some(addr),
                ),
                // There's a representation in the other stack, so use that if possible.
                DualStackLocalIp::OtherStack(addr) => {
                    match dual_stack.dual_stack_enabled(ip_options) {
                        true => BoundOperation::OnlyOtherStack(dual_stack, addr),
                        false => return Err(Either::Right(LocalAddressError::CannotBindToAddress)),
                    }
                }
            }
        }
        // No dual-stack support, so only bind on the current stack.
        (MaybeDualStack::NotDualStack(single_stack), None) => {
            BoundOperation::OnlyCurrentStack(MaybeDualStack::NotDualStack(single_stack), None)
        }
        // No dual-stack support, so check the address is allowed in the current
        // stack.
        (MaybeDualStack::NotDualStack(single_stack), Some(addr)) => {
            match DualStackLocalIp::<I, _>::new(addr) {
                // The address is only representable in the current stack.
                DualStackLocalIp::ThisStack(addr) => BoundOperation::OnlyCurrentStack(
                    MaybeDualStack::NotDualStack(single_stack),
                    Some(addr),
                ),
                // The address has a representation in the other stack but there's
                // no dual-stack support!
                DualStackLocalIp::OtherStack(_addr) => {
                    let _: Option<ZonedAddr<SocketIpAddr<<I::OtherVersion as Ip>::Addr>, _>> =
                        _addr;
                    return Err(Either::Right(LocalAddressError::CannotBindToAddress));
                }
            }
        }
    };

    fn try_bind_single_stack<
        I: IpExt,
        S: DatagramSocketSpec,
        CC: TransportIpContext<I, BC>,
        BC: DatagramBindingsContext,
    >(
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        bound: &mut BoundSocketMap<
            I,
            CC::WeakDeviceId,
            S::AddrSpec,
            S::SocketMapSpec<I, CC::WeakDeviceId>,
        >,
        addr: Option<ZonedAddr<SocketIpAddr<I::Addr>, CC::DeviceId>>,
        device: &Option<CC::WeakDeviceId>,
        local_id: Option<<S::AddrSpec as SocketMapAddrSpec>::LocalIdentifier>,
        id: <S::SocketMapSpec<I, CC::WeakDeviceId> as SocketMapStateSpec>::ListenerId,
        sharing: S::SharingState,
        transparent: bool,
    ) -> Result<
        ListenerAddr<
            ListenerIpAddr<I::Addr, <S::AddrSpec as SocketMapAddrSpec>::LocalIdentifier>,
            CC::WeakDeviceId,
        >,
        LocalAddressError,
    > {
        let identifier = match local_id {
            Some(id) => Some(id),
            None => try_pick_identifier::<I, S, _, _, _>(
                addr.as_ref().map(ZonedAddr::addr),
                bound,
                bindings_ctx,
                &sharing,
            ),
        }
        .ok_or(LocalAddressError::FailedToAllocateLocalPort)?;
        let (addr, device, identifier) =
            try_pick_bound_address::<I, _, _, _>(addr, device, core_ctx, identifier, transparent)?;
        let weak_device = device.map(|d| d.as_weak().into_owned());

        BoundStateHandler::<_, S, _>::try_insert_listener(
            bound,
            addr,
            identifier,
            weak_device.clone(),
            sharing,
            id,
        )
        .map(|()| ListenerAddr { ip: ListenerIpAddr { addr, identifier }, device: weak_device })
    }

    let bound_addr: ListenerAddr<S::ListenerIpAddr<I>, CC::WeakDeviceId> = match bound_operation {
        BoundOperation::OnlyCurrentStack(either_dual_stack, addr) => {
            let converter = match either_dual_stack {
                MaybeDualStack::DualStack(ds) => MaybeDualStack::DualStack(ds.ds_converter()),
                MaybeDualStack::NotDualStack(nds) => {
                    MaybeDualStack::NotDualStack(nds.nds_converter())
                }
            };
            core_ctx
                .with_bound_sockets_mut(|core_ctx, bound| {
                    let id = S::make_bound_socket_map_id(id);

                    try_bind_single_stack::<I, S, _, _>(
                        core_ctx,
                        bindings_ctx,
                        bound,
                        addr,
                        &device,
                        local_id,
                        id,
                        sharing.clone(),
                        ip_options.common.transparent,
                    )
                })
                .map(|ListenerAddr { ip: ListenerIpAddr { addr, identifier }, device }| {
                    let ip = match converter {
                        MaybeDualStack::DualStack(converter) => converter.convert_back(
                            DualStackListenerIpAddr::ThisStack(ListenerIpAddr { addr, identifier }),
                        ),
                        MaybeDualStack::NotDualStack(converter) => {
                            converter.convert_back(ListenerIpAddr { addr, identifier })
                        }
                    };
                    ListenerAddr { ip, device }
                })
        }
        BoundOperation::OnlyOtherStack(core_ctx, addr) => {
            let id = core_ctx.to_other_bound_socket_id(id);
            core_ctx
                .with_other_bound_sockets_mut(|core_ctx, other_bound| {
                    try_bind_single_stack::<_, S, _, _>(
                        core_ctx,
                        bindings_ctx,
                        other_bound,
                        addr,
                        &device,
                        local_id,
                        id,
                        sharing.clone(),
                        ip_options.common.transparent,
                    )
                })
                .map(|ListenerAddr { ip: ListenerIpAddr { addr, identifier }, device }| {
                    ListenerAddr {
                        ip: core_ctx.ds_converter().convert_back(
                            DualStackListenerIpAddr::OtherStack(ListenerIpAddr {
                                addr,
                                identifier,
                            }),
                        ),
                        device,
                    }
                })
        }
        BoundOperation::DualStackAnyAddr(core_ctx) => {
            let ids = PairedBoundSocketIds {
                this: S::make_bound_socket_map_id(id),
                other: core_ctx.to_other_bound_socket_id(id),
            };
            core_ctx
                .with_both_bound_sockets_mut(|core_ctx, bound, other_bound| {
                    let mut bound_pair = PairedSocketMapMut { bound, other_bound };
                    let sharing = sharing.clone();

                    let identifier = match local_id {
                        Some(id) => Some(id),
                        None => try_pick_identifier::<I, S, _, _, _>(
                            DualStackUnspecifiedAddr,
                            &bound_pair,
                            bindings_ctx,
                            &sharing,
                        ),
                    }
                    .ok_or(LocalAddressError::FailedToAllocateLocalPort)?;
                    let (_addr, device, identifier) = try_pick_bound_address::<I, _, _, _>(
                        None,
                        &device,
                        core_ctx,
                        identifier,
                        ip_options.common.transparent,
                    )?;
                    let weak_device = device.map(|d| d.as_weak().into_owned());

                    BoundStateHandler::<_, S, _>::try_insert_listener(
                        &mut bound_pair,
                        DualStackUnspecifiedAddr,
                        identifier,
                        weak_device.clone(),
                        sharing,
                        ids,
                    )
                    .map(|()| (identifier, weak_device))
                })
                .map(|(identifier, device)| ListenerAddr {
                    ip: core_ctx
                        .ds_converter()
                        .convert_back(DualStackListenerIpAddr::BothStacks(identifier)),
                    device,
                })
        }
    }
    .map_err(Either::Right)?;
    // Match Linux behavior by only storing the original bound addr when the
    // local_id was provided by the caller.
    let original_bound_addr = local_id.map(|_id| {
        let ListenerAddr { ip, device: _ } = &bound_addr;
        ip.clone()
    });

    // Replace the unbound state only after we're sure the
    // insertion has succeeded.
    state.inner = SocketStateInner::Bound(BoundSocketState {
        socket_type: BoundSocketStateType::Listener(ListenerState { addr: bound_addr }),
        original_bound_addr,
    });
    Ok(())
}

/// An error when attempting to create a datagram socket.
#[derive(Error, Copy, Clone, Debug, Eq, PartialEq)]
pub enum ConnectError {
    /// An error was encountered creating an IP socket.
    #[error(transparent)]
    Ip(#[from] IpSockCreationError),
    /// No local port was specified, and none could be automatically allocated.
    #[error("a local port could not be allocated")]
    CouldNotAllocateLocalPort,
    /// The specified socket addresses (IP addresses and ports) conflict with an
    /// existing socket.
    #[error("the socket's IP address and port conflict with an existing socket")]
    SockAddrConflict,
    /// There was a problem with the provided address relating to its zone.
    #[error(transparent)]
    Zone(#[from] ZonedAddressError),
    /// The remote address is mapped (i.e. an ipv4-mapped-ipv6 address), but the
    /// socket is not dual-stack enabled.
    #[error("IPv4-mapped-IPv6 addresses are not supported by this socket")]
    RemoteUnexpectedlyMapped,
    /// The remote address is non-mapped (i.e not an ipv4-mapped-ipv6 address),
    /// but the socket is dual stack enabled and bound to a mapped address.
    #[error("non IPv4-mapped-Ipv6 addresses are not supported by this socket")]
    RemoteUnexpectedlyNonMapped,
}

/// Parameters required to connect a socket.
struct ConnectParameters<WireI: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec> {
    local_ip: Option<SocketIpAddr<WireI::Addr>>,
    local_port: Option<<S::AddrSpec as SocketMapAddrSpec>::LocalIdentifier>,
    remote_ip: ZonedAddr<SocketIpAddr<WireI::Addr>, D::Strong>,
    remote_port: <S::AddrSpec as SocketMapAddrSpec>::RemoteIdentifier,
    device: Option<D>,
    sharing: S::SharingState,
    common_ip_options: DatagramIpAgnosticOptions,
    socket_options: DatagramIpSpecificSocketOptions<WireI, D>,
    socket_id:
        <S::SocketMapSpec<WireI, D> as DatagramSocketMapSpec<WireI, D, S::AddrSpec>>::BoundSocketId,
    original_shutdown: Option<Shutdown>,
    extra: S::ConnStateExtra,
}

/// Inserts a connected socket into the bound socket map.
///
/// It accepts two closures that capture the logic required to remove and
/// reinsert the original state from/into the bound_socket_map. The original
/// state will only be reinserted if an error is encountered during connect.
/// The output of `remove_original` is fed into `reinsert_original`.
fn connect_inner<
    WireI: IpExt,
    D: WeakDeviceIdentifier,
    S: DatagramSocketSpec,
    R,
    BC: DatagramBindingsContext,
    CC: IpSocketHandler<WireI, BC, WeakDeviceId = D, DeviceId = D::Strong>,
>(
    connect_params: ConnectParameters<WireI, D, S>,
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    sockets: &mut BoundSocketMap<WireI, D, S::AddrSpec, S::SocketMapSpec<WireI, D>>,
    remove_original: impl FnOnce(
        &mut BoundSocketMap<WireI, D, S::AddrSpec, S::SocketMapSpec<WireI, D>>,
    ) -> R,
    reinsert_original: impl FnOnce(
        &mut BoundSocketMap<WireI, D, S::AddrSpec, S::SocketMapSpec<WireI, D>>,
        R,
    ),
) -> Result<ConnState<WireI, D, S>, ConnectError> {
    let ConnectParameters {
        local_ip,
        local_port,
        remote_ip,
        remote_port,
        device,
        sharing,
        common_ip_options,
        socket_options,
        socket_id,
        original_shutdown,
        extra,
    } = connect_params;

    // Select multicast device if we are connecting to a multicast address.
    let device = device.or_else(|| {
        remote_ip
            .addr()
            .addr()
            .is_multicast()
            .then(|| socket_options.multicast_interface.clone())
            .flatten()
    });

    let (remote_ip, socket_device) = remote_ip.resolve_addr_with_device(device.clone())?;

    let clear_device_on_disconnect = device.is_none() && socket_device.is_some();

    let ip_sock = IpSocketHandler::<WireI, _>::new_ip_socket(
        core_ctx,
        bindings_ctx,
        IpSocketArgs {
            device: socket_device.as_ref().map(|d| d.as_ref()),
            local_ip: local_ip.and_then(IpDeviceAddr::new_from_socket_ip_addr),
            remote_ip,
            proto: S::ip_proto::<WireI>(),
            options: &common_ip_options,
        },
    )?;

    let local_port = match local_port {
        Some(id) => id.clone(),
        None => S::try_alloc_local_id(
            sockets,
            bindings_ctx,
            DatagramFlowId {
                local_ip: SocketIpAddr::from(*ip_sock.local_ip()),
                remote_ip: *ip_sock.remote_ip(),
                remote_id: remote_port.clone(),
            },
        )
        .ok_or(ConnectError::CouldNotAllocateLocalPort)?,
    };
    let conn_addr = ConnAddr {
        ip: ConnIpAddr {
            local: (SocketIpAddr::from(*ip_sock.local_ip()), local_port),
            remote: (*ip_sock.remote_ip(), remote_port),
        },
        device: ip_sock.device().cloned(),
    };
    // Now that all the other checks have been done, actually remove the
    // original state from the socket map.
    let reinsert_op = remove_original(sockets);
    // Try to insert the new connection, restoring the original state on
    // failure.
    let bound_addr = match sockets.conns_mut().try_insert(conn_addr, sharing, socket_id) {
        Ok(bound_entry) => bound_entry.get_addr().clone(),
        Err(
            InsertError::Exists
            | InsertError::IndirectConflict
            | InsertError::ShadowAddrExists
            | InsertError::WouldShadowExisting,
        ) => {
            reinsert_original(sockets, reinsert_op);
            return Err(ConnectError::SockAddrConflict);
        }
    };
    Ok(ConnState {
        socket: ip_sock,
        clear_device_on_disconnect,
        shutdown: original_shutdown.unwrap_or_else(Shutdown::default),
        addr: bound_addr,
        extra,
    })
}

/// State required to perform single-stack connection of a socket.
struct SingleStackConnectOperation<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec> {
    params: ConnectParameters<I, D, S>,
    remove_op: Option<SingleStackRemoveOperation<I, D, S>>,
}

impl<I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec>
    SingleStackConnectOperation<I, D, S>
{
    /// Constructs the connect operation from existing socket state.
    fn new_from_state<
        BC,
        CC: NonDualStackDatagramBoundStateContext<I, BC, S, WeakDeviceId = D, DeviceId = D::Strong>,
    >(
        core_ctx: &mut CC,
        socket_id: &S::SocketId<I, D>,
        state: &SocketState<I, D, S>,
        remote_ip: ZonedAddr<SocketIpAddr<I::Addr>, D::Strong>,
        remote_port: <S::AddrSpec as SocketMapAddrSpec>::RemoteIdentifier,
        extra: S::ConnStateExtra,
    ) -> Self {
        let SocketState { ip_options, inner, sharing } = state;
        match inner {
            SocketStateInner::Unbound(UnboundSocketState { device }) => {
                SingleStackConnectOperation {
                    params: ConnectParameters {
                        local_ip: None,
                        local_port: None,
                        remote_ip,
                        remote_port,
                        device: device.clone(),
                        sharing: sharing.clone(),
                        common_ip_options: ip_options.common.clone(),
                        socket_options: ip_options.socket_options.clone(),
                        socket_id: S::make_bound_socket_map_id(socket_id),
                        original_shutdown: None,
                        extra,
                    },
                    remove_op: None,
                }
            }
            SocketStateInner::Bound(state) => {
                let remove_op = SingleStackRemoveOperation::new_from_state(
                    core_ctx,
                    socket_id,
                    state,
                    sharing.clone(),
                );
                let BoundSocketState { socket_type, original_bound_addr: _ } = state;
                match socket_type {
                    BoundSocketStateType::Listener(ListenerState {
                        addr: ListenerAddr { ip, device },
                    }) => {
                        let ListenerIpAddr { addr, identifier } =
                            core_ctx.nds_converter().convert(ip);
                        SingleStackConnectOperation {
                            params: ConnectParameters {
                                local_ip: addr.clone(),
                                local_port: Some(*identifier),
                                remote_ip,
                                remote_port,
                                device: device.clone(),
                                sharing: sharing.clone(),
                                common_ip_options: ip_options.common.clone(),
                                socket_options: ip_options.socket_options.clone(),
                                socket_id: S::make_bound_socket_map_id(socket_id),
                                original_shutdown: None,
                                extra,
                            },
                            remove_op: Some(remove_op),
                        }
                    }
                    BoundSocketStateType::Connected(state) => {
                        let ConnState {
                            socket: _,
                            shutdown,
                            addr:
                                ConnAddr {
                                    ip: ConnIpAddr { local: (local_ip, local_id), remote: _ },
                                    device,
                                },
                            clear_device_on_disconnect: _,
                            extra: _,
                        } = core_ctx.nds_converter().convert(state);
                        SingleStackConnectOperation {
                            params: ConnectParameters {
                                local_ip: Some(local_ip.clone()),
                                local_port: Some(*local_id),
                                remote_ip,
                                remote_port,
                                device: device.clone(),
                                sharing: sharing.clone(),
                                common_ip_options: ip_options.common.clone(),
                                socket_options: ip_options.socket_options.clone(),
                                socket_id: S::make_bound_socket_map_id(socket_id),
                                original_shutdown: Some(shutdown.clone()),
                                extra,
                            },
                            remove_op: Some(remove_op),
                        }
                    }
                }
            }
        }
    }

    /// Performs this operation and connects the socket.
    ///
    /// This is primarily a wrapper around `connect_inner` that establishes the
    /// remove/reinsert closures for single stack removal.
    ///
    /// Returns the state for the new connection.
    fn apply<
        BC: DatagramBindingsContext,
        CC: IpSocketHandler<I, BC, WeakDeviceId = D, DeviceId = D::Strong>,
    >(
        self,
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        socket_map: &mut BoundSocketMap<I, D, S::AddrSpec, S::SocketMapSpec<I, D>>,
    ) -> Result<ConnState<I, D, S>, ConnectError> {
        let SingleStackConnectOperation { params, remove_op } = self;
        let remove_fn =
            |sockets: &mut BoundSocketMap<I, D, S::AddrSpec, S::SocketMapSpec<I, D>>| {
                remove_op.map(|remove_op| {
                    remove_op.apply(sockets).expect("Failed to remove listener socket")
                })
            };
        let reinsert_fn =
            |sockets: &mut BoundDatagramSocketMap<I, D, S>,
             insert_op: Option<SingleStackInsertOperation<I, D, S>>| {
                if let Some(insert_op) = insert_op {
                    let _: SingleStackRemoveOperation<I, D, S> =
                        insert_op.apply(sockets).expect("Failed to revert listener socket removal");
                }
            };
        connect_inner(params, core_ctx, bindings_ctx, socket_map, remove_fn, reinsert_fn)
    }
}

/// State required to perform dual-stack connection of a socket.
struct DualStackConnectOperation<I: DualStackIpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec>
{
    params: EitherStack<ConnectParameters<I, D, S>, ConnectParameters<I::OtherVersion, D, S>>,
    remove_op: Option<DualStackRemoveOperation<I, D, S>>,
}

impl<I: DualStackIpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec>
    DualStackConnectOperation<I, D, S>
{
    /// Constructs the connect operation from existing socket state.
    fn new_from_state<
        BC: DatagramBindingsContext,
        CC: DualStackDatagramBoundStateContext<I, BC, S, WeakDeviceId = D, DeviceId = D::Strong>,
    >(
        core_ctx: &mut CC,
        socket_id: &S::SocketId<I, D>,
        state: &SocketState<I, D, S>,
        remote_ip: DualStackRemoteIp<I, D::Strong>,
        remote_port: <S::AddrSpec as SocketMapAddrSpec>::RemoteIdentifier,
        extra: S::ConnStateExtra,
    ) -> Result<Self, ConnectError> {
        let SocketState { ip_options, inner, sharing } = state;
        match inner {
            SocketStateInner::Unbound(UnboundSocketState { device }) => {
                // Unbound sockets don't have a predisposition of which stack to
                // connect in. Instead, it's dictated entirely by the remote.
                let params = match remote_ip {
                    DualStackRemoteIp::ThisStack(remote_ip) => {
                        EitherStack::ThisStack(ConnectParameters {
                            local_ip: None,
                            local_port: None,
                            remote_ip,
                            remote_port,
                            device: device.clone(),
                            sharing: sharing.clone(),
                            common_ip_options: ip_options.common.clone(),
                            socket_options: ip_options.socket_options.clone(),
                            socket_id: S::make_bound_socket_map_id(socket_id),
                            original_shutdown: None,
                            extra,
                        })
                    }
                    DualStackRemoteIp::OtherStack(remote_ip) => {
                        if !core_ctx.dual_stack_enabled(ip_options) {
                            return Err(ConnectError::RemoteUnexpectedlyMapped);
                        }
                        EitherStack::OtherStack(ConnectParameters {
                            local_ip: None,
                            local_port: None,
                            remote_ip,
                            remote_port,
                            device: device.clone(),
                            sharing: sharing.clone(),
                            common_ip_options: ip_options.common.clone(),
                            socket_options: core_ctx.to_other_socket_options(ip_options).clone(),
                            socket_id: core_ctx.to_other_bound_socket_id(socket_id),
                            original_shutdown: None,
                            extra,
                        })
                    }
                };
                Ok(DualStackConnectOperation { params, remove_op: None })
            }
            SocketStateInner::Bound(state) => {
                let remove_op = DualStackRemoveOperation::new_from_state(
                    core_ctx,
                    socket_id,
                    ip_options,
                    state,
                    sharing.clone(),
                );

                let BoundSocketState { socket_type, original_bound_addr: _ } = state;
                match socket_type {
                    BoundSocketStateType::Listener(ListenerState {
                        addr: ListenerAddr { ip, device },
                    }) => {
                        match (remote_ip, core_ctx.ds_converter().convert(ip)) {
                            // Disallow connecting to the other stack because the
                            // existing socket state is in this stack.
                            (
                                DualStackRemoteIp::OtherStack(_),
                                DualStackListenerIpAddr::ThisStack(_),
                            ) => Err(ConnectError::RemoteUnexpectedlyMapped),
                            // Disallow connecting to this stack because the existing
                            // socket state is in the other stack.
                            (
                                DualStackRemoteIp::ThisStack(_),
                                DualStackListenerIpAddr::OtherStack(_),
                            ) => Err(ConnectError::RemoteUnexpectedlyNonMapped),
                            // Connect in this stack.
                            (
                                DualStackRemoteIp::ThisStack(remote_ip),
                                DualStackListenerIpAddr::ThisStack(ListenerIpAddr {
                                    addr,
                                    identifier,
                                }),
                            ) => Ok(DualStackConnectOperation {
                                params: EitherStack::ThisStack(ConnectParameters {
                                    local_ip: addr.clone(),
                                    local_port: Some(*identifier),
                                    remote_ip,
                                    remote_port,
                                    device: device.clone(),
                                    sharing: sharing.clone(),
                                    common_ip_options: ip_options.common.clone(),
                                    socket_options: ip_options.socket_options.clone(),
                                    socket_id: S::make_bound_socket_map_id(socket_id),
                                    original_shutdown: None,
                                    extra,
                                }),
                                remove_op: Some(remove_op),
                            }),
                            // Listeners in "both stacks" can connect to either
                            // stack. Connect in this stack as specified by the
                            // remote.
                            (
                                DualStackRemoteIp::ThisStack(remote_ip),
                                DualStackListenerIpAddr::BothStacks(identifier),
                            ) => Ok(DualStackConnectOperation {
                                params: EitherStack::ThisStack(ConnectParameters {
                                    local_ip: None,
                                    local_port: Some(*identifier),
                                    remote_ip,
                                    remote_port,
                                    device: device.clone(),
                                    sharing: sharing.clone(),
                                    common_ip_options: ip_options.common.clone(),
                                    socket_options: ip_options.socket_options.clone(),
                                    socket_id: S::make_bound_socket_map_id(socket_id),
                                    original_shutdown: None,
                                    extra,
                                }),
                                remove_op: Some(remove_op),
                            }),
                            // Connect in the other stack.
                            (
                                DualStackRemoteIp::OtherStack(remote_ip),
                                DualStackListenerIpAddr::OtherStack(ListenerIpAddr {
                                    addr,
                                    identifier,
                                }),
                            ) => Ok(DualStackConnectOperation {
                                params: EitherStack::OtherStack(ConnectParameters {
                                    local_ip: addr.clone(),
                                    local_port: Some(*identifier),
                                    remote_ip,
                                    remote_port,
                                    device: device.clone(),
                                    sharing: sharing.clone(),
                                    common_ip_options: ip_options.common.clone(),
                                    socket_options: core_ctx
                                        .to_other_socket_options(ip_options)
                                        .clone(),
                                    socket_id: core_ctx.to_other_bound_socket_id(socket_id),
                                    original_shutdown: None,
                                    extra,
                                }),
                                remove_op: Some(remove_op),
                            }),
                            // Listeners in "both stacks" can connect to either
                            // stack. Connect in the other stack as specified by
                            // the remote.
                            (
                                DualStackRemoteIp::OtherStack(remote_ip),
                                DualStackListenerIpAddr::BothStacks(identifier),
                            ) => Ok(DualStackConnectOperation {
                                params: EitherStack::OtherStack(ConnectParameters {
                                    local_ip: None,
                                    local_port: Some(*identifier),
                                    remote_ip,
                                    remote_port,
                                    device: device.clone(),
                                    sharing: sharing.clone(),
                                    common_ip_options: ip_options.common.clone(),
                                    socket_options: core_ctx
                                        .to_other_socket_options(ip_options)
                                        .clone(),
                                    socket_id: core_ctx.to_other_bound_socket_id(socket_id),
                                    original_shutdown: None,
                                    extra,
                                }),
                                remove_op: Some(remove_op),
                            }),
                        }
                    }
                    BoundSocketStateType::Connected(state) => {
                        match (remote_ip, core_ctx.ds_converter().convert(state)) {
                            // Disallow connecting to the other stack because the
                            // existing socket state is in this stack.
                            (
                                DualStackRemoteIp::OtherStack(_),
                                DualStackConnState::ThisStack(_),
                            ) => Err(ConnectError::RemoteUnexpectedlyMapped),
                            // Disallow connecting to this stack because the existing
                            // socket state is in the other stack.
                            (
                                DualStackRemoteIp::ThisStack(_),
                                DualStackConnState::OtherStack(_),
                            ) => Err(ConnectError::RemoteUnexpectedlyNonMapped),
                            // Connect in this stack.
                            (
                                DualStackRemoteIp::ThisStack(remote_ip),
                                DualStackConnState::ThisStack(ConnState {
                                    socket: _,
                                    shutdown,
                                    addr:
                                        ConnAddr {
                                            ip:
                                                ConnIpAddr { local: (local_ip, local_id), remote: _ },
                                            device,
                                        },
                                    clear_device_on_disconnect: _,
                                    extra: _,
                                }),
                            ) => Ok(DualStackConnectOperation {
                                params: EitherStack::ThisStack(ConnectParameters {
                                    local_ip: Some(local_ip.clone()),
                                    local_port: Some(*local_id),
                                    remote_ip,
                                    remote_port,
                                    device: device.clone(),
                                    sharing: sharing.clone(),
                                    common_ip_options: ip_options.common.clone(),
                                    socket_options: ip_options.socket_options.clone(),
                                    socket_id: S::make_bound_socket_map_id(socket_id),
                                    original_shutdown: Some(shutdown.clone()),
                                    extra,
                                }),
                                remove_op: Some(remove_op),
                            }),
                            // Connect in the other stack.
                            (
                                DualStackRemoteIp::OtherStack(remote_ip),
                                DualStackConnState::OtherStack(ConnState {
                                    socket: _,
                                    shutdown,
                                    addr:
                                        ConnAddr {
                                            ip:
                                                ConnIpAddr { local: (local_ip, local_id), remote: _ },
                                            device,
                                        },
                                    clear_device_on_disconnect: _,
                                    extra: _,
                                }),
                            ) => Ok(DualStackConnectOperation {
                                params: EitherStack::OtherStack(ConnectParameters {
                                    local_ip: Some(local_ip.clone()),
                                    local_port: Some(*local_id),
                                    remote_ip,
                                    remote_port,
                                    device: device.clone(),
                                    sharing: sharing.clone(),
                                    common_ip_options: ip_options.common.clone(),
                                    socket_options: core_ctx
                                        .to_other_socket_options(ip_options)
                                        .clone(),
                                    socket_id: core_ctx.to_other_bound_socket_id(socket_id),
                                    original_shutdown: Some(shutdown.clone()),
                                    extra,
                                }),
                                remove_op: Some(remove_op),
                            }),
                        }
                    }
                }
            }
        }
    }

    /// Performs this operation and connects the socket.
    ///
    /// This is primarily a wrapper around [`connect_inner`] that establishes the
    /// remove/reinsert closures for dual stack removal.
    ///
    /// Returns a tuple containing the state, and sharing state for the new
    /// connection.
    fn apply<
        BC: DatagramBindingsContext,
        CC: IpSocketHandler<I, BC, WeakDeviceId = D, DeviceId = D::Strong>
            + IpSocketHandler<I::OtherVersion, BC, WeakDeviceId = D, DeviceId = D::Strong>,
    >(
        self,
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        socket_map: &mut BoundDatagramSocketMap<I, D, S>,
        other_socket_map: &mut BoundDatagramSocketMap<I::OtherVersion, D, S>,
    ) -> Result<DualStackConnState<I, D, S>, ConnectError> {
        let DualStackConnectOperation { params, remove_op } = self;
        match params {
            EitherStack::ThisStack(params) => {
                // NB: Because we're connecting in this stack, we receive this
                // stack's sockets as an argument to `remove_fn` and
                // `reinsert_fn`. Thus we need to capture + pass through the
                // other stack's sockets.
                let remove_fn = |sockets: &mut BoundDatagramSocketMap<I, D, S>| {
                    remove_op.map(|remove_op| {
                        let reinsert_op = remove_op
                            .apply(sockets, other_socket_map)
                            .expect("Failed to remove listener socket");
                        (reinsert_op, other_socket_map)
                    })
                };
                let reinsert_fn = |sockets: &mut BoundDatagramSocketMap<I, D, S>,
                                   insert_op: Option<(
                    DualStackInsertOperation<I, D, S>,
                    &mut BoundDatagramSocketMap<I::OtherVersion, D, S>,
                )>| {
                    if let Some((insert_op, other_sockets)) = insert_op {
                        let _: DualStackRemoveOperation<I, D, S> = insert_op
                            .apply(sockets, other_sockets)
                            .expect("Failed to revert listener socket removal");
                    }
                };
                connect_inner(params, core_ctx, bindings_ctx, socket_map, remove_fn, reinsert_fn)
                    .map(DualStackConnState::ThisStack)
            }
            EitherStack::OtherStack(params) => {
                // NB: Because we're connecting in the other stack, we receive
                // the other stack's sockets as an argument to `remove_fn` and
                // `reinsert_fn`. Thus we need to capture + pass through this
                // stack's sockets.
                let remove_fn =
                    |other_sockets: &mut BoundDatagramSocketMap<I::OtherVersion, D, S>| {
                        remove_op.map(|remove_op| {
                            let reinsert_op = remove_op
                                .apply(socket_map, other_sockets)
                                .expect("Failed to remove listener socket");
                            (reinsert_op, socket_map)
                        })
                    };
                let reinsert_fn =
                    |other_sockets: &mut BoundDatagramSocketMap<I::OtherVersion, D, S>,
                     insert_op: Option<(
                        DualStackInsertOperation<I, D, S>,
                        &mut BoundDatagramSocketMap<I, D, S>,
                    )>| {
                        if let Some((insert_op, sockets)) = insert_op {
                            let _: DualStackRemoveOperation<I, D, S> = insert_op
                                .apply(sockets, other_sockets)
                                .expect("Failed to revert listener socket removal");
                        }
                    };
                connect_inner(
                    params,
                    core_ctx,
                    bindings_ctx,
                    other_socket_map,
                    remove_fn,
                    reinsert_fn,
                )
                .map(DualStackConnState::OtherStack)
            }
        }
    }
}

/// A connected socket was expected.
#[derive(Copy, Clone, Debug, Default, Eq, GenericOverIp, PartialEq, Error)]
#[generic_over_ip()]
#[error("expected connected socket")]
pub struct ExpectedConnError;

/// An unbound socket was expected.
#[derive(Copy, Clone, Debug, Default, Eq, GenericOverIp, PartialEq, Error)]
#[generic_over_ip()]
#[error("expected unbound socket")]
pub struct ExpectedUnboundError;

/// Converts a connected socket to an unbound socket.
///
/// Removes the connection's entry from the [`BoundSocketMap`], and returns the
/// socket's new state.
fn disconnect_to_unbound<
    I: IpExt,
    BC: DatagramBindingsContext,
    CC: DatagramBoundStateContext<I, BC, S>,
    S: DatagramSocketSpec,
>(
    core_ctx: &mut CC,
    id: &S::SocketId<I, CC::WeakDeviceId>,
    clear_device_on_disconnect: bool,
    ip_options: &IpOptions<I, CC::WeakDeviceId, S>,
    socket_state: &BoundSocketState<I, CC::WeakDeviceId, S>,
    sharing: S::SharingState,
) -> UnboundSocketState<CC::WeakDeviceId> {
    match core_ctx.dual_stack_context_mut() {
        MaybeDualStack::NotDualStack(nds) => {
            let remove_op =
                SingleStackRemoveOperation::new_from_state(nds, id, socket_state, sharing);
            let _: SingleStackInsertOperation<_, _, _> =
                core_ctx.with_bound_sockets_mut(|_core_ctx, bound| {
                    remove_op.apply(bound).expect("Failed to remove connected socket entry")
                });
        }
        MaybeDualStack::DualStack(ds) => {
            let remove_op =
                DualStackRemoveOperation::new_from_state(ds, id, ip_options, socket_state, sharing);
            let _: DualStackInsertOperation<_, _, _> =
                ds.with_both_bound_sockets_mut(|_core_ctx, bound, other_bound| {
                    remove_op
                        .apply(bound, other_bound)
                        .expect("Failed to remove connected socket entry")
                });
        }
    };
    let device =
        if clear_device_on_disconnect { None } else { socket_state.get_device(core_ctx).clone() };
    UnboundSocketState { device }
}

/// Converts a connected socket to a listener socket.
///
/// Removes the connection's entry from the [`BoundSocketMap`] and returns the
/// socket's new state.
fn disconnect_to_listener<
    I: IpExt,
    BC: DatagramBindingsContext,
    CC: DatagramBoundStateContext<I, BC, S>,
    S: DatagramSocketSpec,
>(
    core_ctx: &mut CC,
    id: &S::SocketId<I, CC::WeakDeviceId>,
    listener_ip: S::ListenerIpAddr<I>,
    clear_device_on_disconnect: bool,
    ip_options: &IpOptions<I, CC::WeakDeviceId, S>,
    socket_state: &BoundSocketState<I, CC::WeakDeviceId, S>,
    sharing: S::SharingState,
) -> BoundSocketState<I, CC::WeakDeviceId, S> {
    let new_device =
        if clear_device_on_disconnect { None } else { socket_state.get_device(core_ctx).clone() };

    match core_ctx.dual_stack_context_mut() {
        MaybeDualStack::NotDualStack(nds) => {
            let ListenerIpAddr { addr, identifier } =
                nds.nds_converter().convert(listener_ip.clone());
            let remove_op =
                SingleStackRemoveOperation::new_from_state(nds, id, socket_state, sharing.clone());
            core_ctx.with_bound_sockets_mut(|_core_ctx, bound| {
                let _: SingleStackInsertOperation<_, _, _> =
                    remove_op.apply(bound).expect("Failed to remove connected socket entry");
                BoundStateHandler::<_, S, _>::try_insert_listener(
                    bound,
                    addr,
                    identifier,
                    new_device.clone(),
                    sharing.clone(),
                    S::make_bound_socket_map_id(id),
                )
                .expect("inserting listener for disconnected socket should succeed");
            })
        }
        MaybeDualStack::DualStack(ds) => {
            let remove_op = DualStackRemoveOperation::new_from_state(
                ds,
                id,
                ip_options,
                socket_state,
                sharing.clone(),
            );
            let other_id = ds.to_other_bound_socket_id(id);
            let id = S::make_bound_socket_map_id(id);
            let converter = ds.ds_converter();
            ds.with_both_bound_sockets_mut(|_core_ctx, bound, other_bound| {
                let _: DualStackInsertOperation<_, _, _> = remove_op
                    .apply(bound, other_bound)
                    .expect("Failed to remove connected socket entry");

                match converter.convert(listener_ip.clone()) {
                    DualStackListenerIpAddr::ThisStack(ListenerIpAddr { addr, identifier }) => {
                        BoundStateHandler::<_, S, _>::try_insert_listener(
                            bound,
                            addr,
                            identifier,
                            new_device.clone(),
                            sharing.clone(),
                            id,
                        )
                    }
                    DualStackListenerIpAddr::OtherStack(ListenerIpAddr { addr, identifier }) => {
                        BoundStateHandler::<_, S, _>::try_insert_listener(
                            other_bound,
                            addr,
                            identifier,
                            new_device.clone(),
                            sharing.clone(),
                            other_id,
                        )
                    }
                    DualStackListenerIpAddr::BothStacks(identifier) => {
                        let ids = PairedBoundSocketIds { this: id, other: other_id };
                        let mut bound_pair = PairedSocketMapMut { bound, other_bound };
                        BoundStateHandler::<_, S, _>::try_insert_listener(
                            &mut bound_pair,
                            DualStackUnspecifiedAddr,
                            identifier,
                            new_device.clone(),
                            sharing.clone(),
                            ids,
                        )
                    }
                }
                .expect("inserting listener for disconnected socket should succeed");
            })
        }
    };
    BoundSocketState {
        original_bound_addr: Some(listener_ip.clone()),
        socket_type: BoundSocketStateType::Listener(ListenerState {
            addr: ListenerAddr { ip: listener_ip, device: new_device },
        }),
    }
}

/// Error encountered when sending a datagram on a socket.
#[derive(Debug, GenericOverIp, Error)]
#[generic_over_ip()]
pub enum SendError<SE: Error> {
    /// The socket is not connected,
    #[error("socket not connected")]
    NotConnected,
    /// The socket is not writeable.
    #[error("socket not writeable")]
    NotWriteable,
    /// There was a problem sending the IP packet.
    #[error("error sending IP packet: {0}")]
    IpSock(#[from] IpSockSendError),
    /// There was a problem when serializing the packet.
    #[error("error serializing packet: {0:?}")]
    SerializeError(#[source] SE),
    /// There is no space available on the send buffer.
    #[error("send buffer full")]
    SendBufferFull,
    /// Invalid message length.
    #[error("invalid message length")]
    InvalidLength,
}

impl<SE: Error> From<SendBufferError> for SendError<SE> {
    fn from(err: SendBufferError) -> Self {
        match err {
            SendBufferError::SendBufferFull => Self::SendBufferFull,
            SendBufferError::InvalidLength => Self::InvalidLength,
        }
    }
}

/// An error encountered while sending a datagram packet to an alternate address.
#[derive(Debug, Error)]
pub enum SendToError<SE: Error> {
    /// The socket is not writeable.
    #[error("socket not writeable")]
    NotWriteable,
    /// An error was encountered while trying to bind a local address for an
    /// unbound socket.
    #[error("local address error: {0}")]
    LocalAddress(#[from] LocalAddressError),
    /// There was a problem with the remote address relating to its zone.
    #[error("problem with zone of remote address: {0}")]
    Zone(#[from] ZonedAddressError),
    /// An error was encountered while trying to create a temporary IP socket
    /// to use for the send operation.
    #[error("error creating temporary IP socket for send: {0}")]
    CreateAndSend(#[from] IpSockCreateAndSendError),
    /// The remote address is mapped (i.e. an ipv4-mapped-ipv6 address), but the
    /// socket is not dual-stack enabled.
    #[error("remote address is mapped, but socket is not dual-stack enabled")]
    RemoteUnexpectedlyMapped,
    /// The remote address is non-mapped (i.e not an ipv4-mapped-ipv6 address),
    /// but the socket is dual stack enabled and bound to a mapped address.
    #[error(
        "remote address is non-mapped, but socket is \
         dual-stack enabled and bound to mapped address"
    )]
    RemoteUnexpectedlyNonMapped,
    /// The provided buffer is not valid.
    #[error("serialize buffer invalid")]
    SerializeError(#[source] SE),
    /// There is no space available on the send buffer.
    #[error("send buffer full")]
    SendBufferFull,
    /// Invalid message length.
    #[error("invalid message length")]
    InvalidLength,
}

impl<SE: Error> From<SendBufferError> for SendToError<SE> {
    fn from(err: SendBufferError) -> Self {
        match err {
            SendBufferError::SendBufferFull => Self::SendBufferFull,
            SendBufferError::InvalidLength => Self::InvalidLength,
        }
    }
}

struct SendOneshotParameters<
    'a,
    SockI: IpExt,
    WireI: IpExt,
    S: DatagramSocketSpec,
    D: WeakDeviceIdentifier,
> {
    local_ip: Option<SocketIpAddr<WireI::Addr>>,
    local_id: <S::AddrSpec as SocketMapAddrSpec>::LocalIdentifier,
    remote_ip: ZonedAddr<SocketIpAddr<WireI::Addr>, D::Strong>,
    remote_id: <S::AddrSpec as SocketMapAddrSpec>::RemoteIdentifier,
    device: &'a Option<D>,
    options: IpOptionsRef<'a, WireI, D>,
    id: &'a S::SocketId<SockI, D>,
    send_token: S::SendToken,
}

fn send_oneshot<
    SockI: IpExt,
    WireI: IpExt,
    S: DatagramSocketSpec,
    CC: IpSocketHandler<WireI, BC> + CoreTxMetadataContext<TxMetadata<SockI, CC::WeakDeviceId, S>, BC>,
    BC: DatagramBindingsContext,
    B: BufferMut,
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    params: SendOneshotParameters<'_, SockI, WireI, S, CC::WeakDeviceId>,
    body: B,
) -> Result<(), SendToError<S::SerializeError>> {
    let SendOneshotParameters {
        local_ip,
        local_id,
        remote_ip,
        remote_id,
        device,
        options,
        id,
        send_token,
    } = params;
    let device = device.clone().or_else(|| {
        remote_ip
            .addr()
            .addr()
            .is_multicast()
            .then(|| options.ip_specific.multicast_interface.clone())
            .flatten()
    });
    let (remote_ip, device) = match remote_ip.resolve_addr_with_device(device) {
        Ok(addr) => addr,
        Err(e) => return Err(SendToError::Zone(e)),
    };

    let tx_metadata =
        id.borrow().send_buffer.prepare_for_send::<WireI, _, _, _>(id, &body, send_token)?;
    let tx_metadata = core_ctx.convert_tx_meta(tx_metadata);

    core_ctx
        .send_oneshot_ip_packet_with_fallible_serializer(
            bindings_ctx,
            IpSocketArgs {
                device: device.as_ref().map(|d| d.as_ref()),
                local_ip: local_ip.and_then(IpDeviceAddr::new_from_socket_ip_addr),
                remote_ip,
                proto: S::ip_proto::<WireI>(),
                options: &options,
            },
            tx_metadata,
            |local_ip| {
                S::make_packet::<WireI, _>(
                    body,
                    &ConnIpAddr {
                        local: (local_ip.into(), local_id),
                        remote: (remote_ip, remote_id),
                    },
                )
            },
        )
        .map_err(|err| match err {
            SendOneShotIpPacketError::CreateAndSendError { err } => SendToError::CreateAndSend(err),
            SendOneShotIpPacketError::SerializeError(err) => SendToError::SerializeError(err),
        })
}

/// Mutably holds the original state of a bound socket required to update the
/// bound device.
enum SetBoundDeviceParameters<'a, I: IpExt, D: WeakDeviceIdentifier, S: DatagramSocketSpec> {
    Listener {
        ip: &'a ListenerIpAddr<I::Addr, <S::AddrSpec as SocketMapAddrSpec>::LocalIdentifier>,
        device: &'a mut Option<D>,
    },
    Connected(&'a mut ConnState<I, D, S>),
}

/// Update the device for a bound socket.
///
/// The update is applied both to the socket's entry in the given
/// [`BoundSocketMap`], and the mutable socket state in the given
/// [`SetBoundDeviceParameters`].
///
/// # Panics
///
/// Panics if the given `socket_id` is not present in the given `sockets` map.
fn set_bound_device_single_stack<
    'a,
    I: IpExt,
    D: WeakDeviceIdentifier,
    S: DatagramSocketSpec,
    BC: DatagramBindingsContext,
    CC: IpSocketHandler<I, BC, WeakDeviceId = D, DeviceId = D::Strong>,
>(
    bindings_ctx: &mut BC,
    core_ctx: &mut CC,
    params: SetBoundDeviceParameters<'a, I, D, S>,
    sockets: &mut BoundDatagramSocketMap<I, D, S>,
    socket_id: &BoundDatagramSocketId<I, D, S>,
    common_ip_options: &DatagramIpAgnosticOptions,
    new_device: Option<&D::Strong>,
    sharing: S::SharingState,
) -> Result<(), SocketError> {
    let (local_ip, remote_ip, old_device) = match &params {
        SetBoundDeviceParameters::Listener {
            ip: ListenerIpAddr { addr, identifier: _ },
            device,
        } => (addr.as_ref(), None, device.as_ref()),
        SetBoundDeviceParameters::Connected(ConnState {
            socket: _,
            addr:
                ConnAddr {
                    ip: ConnIpAddr { local: (local_ip, _local_id), remote: (remote_ip, _remote_id) },
                    device,
                },
            shutdown: _,
            clear_device_on_disconnect: _,
            extra: _,
        }) => (Some(local_ip), Some(remote_ip), device.as_ref()),
    };
    // Don't allow changing the device if one of the IP addresses in the
    // socket address vector requires a zone (scope ID).
    let device_update = SocketDeviceUpdate {
        local_ip: local_ip.map(AsRef::<SpecifiedAddr<I::Addr>>::as_ref),
        remote_ip: remote_ip.map(AsRef::<SpecifiedAddr<I::Addr>>::as_ref),
        old_device,
    };
    match device_update.check_update(new_device) {
        Ok(()) => (),
        Err(SocketDeviceUpdateNotAllowedError) => {
            return Err(SocketError::Local(LocalAddressError::Zone(
                ZonedAddressError::DeviceZoneMismatch,
            )));
        }
    };

    let new_device_strong = new_device.map(EitherDeviceId::Strong);
    let new_device_weak = new_device.map(|d| d.downgrade());

    let (old_addr, new_addr, update_state) = match params {
        SetBoundDeviceParameters::Listener { ip, device } => (
            AddrVec::Listen(ListenerAddr { ip: ip.clone(), device: device.clone() }),
            AddrVec::Listen(ListenerAddr { ip: ip.clone(), device: new_device_weak.clone() }),
            Either::Left(move || {
                *device = new_device_weak;
            }),
        ),
        SetBoundDeviceParameters::Connected(ConnState {
            socket,
            addr,
            shutdown: _,
            clear_device_on_disconnect,
            extra: _,
        }) => {
            let ConnIpAddr { local: (local_ip, _local_id), remote: (remote_ip, _remote_id) } =
                addr.ip;
            let new_socket = core_ctx
                .new_ip_socket(
                    bindings_ctx,
                    IpSocketArgs {
                        device: new_device_strong,
                        local_ip: IpDeviceAddr::new_from_socket_ip_addr(local_ip.clone()),
                        remote_ip: remote_ip.clone(),
                        proto: socket.proto(),
                        options: common_ip_options,
                    },
                )
                .map_err(|_: IpSockCreationError| {
                    SocketError::Remote(RemoteAddressError::NoRoute)
                })?;
            let new_addr = ConnAddr { ip: addr.ip.clone(), device: new_device_weak.clone() };
            (
                AddrVec::Conn(addr.clone()),
                AddrVec::Conn(new_addr.clone()),
                Either::Right(move || {
                    *socket = new_socket;
                    // If this operation explicitly sets the device for the socket, it
                    // should no longer be cleared on disconnect.
                    if new_device.is_some() {
                        *clear_device_on_disconnect = false;
                    }
                    *addr = new_addr
                }),
            )
        }
    };

    // Remove old address from the socket map.
    let remove_op = SingleStackRemoveOperation::<I, D, S> {
        socket_id: socket_id.clone(),
        sharing: sharing.clone(),
        addr: old_addr,
        _marker: PhantomData,
    };
    let reinsert_op = remove_op.apply(sockets).expect("failed to remove socket in set_device");

    // Insert new address into the socket map. This operation may fail, in which case
    // we need to reinsert the old address.
    let insert_op = SingleStackInsertOperation::<I, D, S> {
        socket_id: socket_id.clone(),
        sharing: sharing,
        addr: new_addr,
        _marker: PhantomData,
    };
    match insert_op.apply(sockets) {
        Err(e) => {
            let _: SingleStackRemoveOperation<_, _, _> = reinsert_op
                .apply(sockets)
                .expect("failed to reinsert socket after failed set_device");
            return Err(SocketError::Local(e.into()));
        }
        Ok(_) => {}
    }

    // Update the socket after updating the socket map.
    match update_state {
        Either::Left(f) => f(),
        Either::Right(f) => f(),
    }

    Ok(())
}

/// Update the device for a listener socket in both stacks.
///
/// Either the update is applied successfully to both stacks, or (in the case of
/// an error) both stacks are left in their original state.
///
/// # Panics
///
/// Panics if the given socket IDs are not present in the given socket maps.
fn set_bound_device_listener_both_stacks<
    'a,
    I: IpExt,
    D: WeakDeviceIdentifier,
    S: DatagramSocketSpec,
>(
    old_device: &mut Option<D>,
    identifier: <S::AddrSpec as SocketMapAddrSpec>::LocalIdentifier,
    sockets: PairedSocketMapMut<'a, I, D, S>,
    socket_ids: PairedBoundSocketIds<I, D, S>,
    new_device: Option<D>,
    sharing: S::SharingState,
) -> Result<(), SocketError> {
    let PairedSocketMapMut { bound: sockets, other_bound: other_sockets } = sockets;

    let remove_op = DualStackListenerRemoveOperation {
        identifier,
        device: old_device.clone(),
        sharing: sharing.clone(),
        socket_ids: socket_ids.clone(),
        _marker: PhantomData,
    };
    let reinsert_op =
        remove_op.apply(sockets, other_sockets).expect("failed to remove socket in set_device");

    let insert_op = DualStackListenerInsertOperation {
        identifier,
        device: new_device.clone(),
        sharing,
        socket_ids,
        _marker: PhantomData,
    };
    match insert_op.apply(sockets, other_sockets) {
        Err(e) => {
            let _: DualStackListenerRemoveOperation<_, _, _> = reinsert_op
                .apply(sockets, other_sockets)
                .expect("failed to reinsert socket after failed set_device");
            return Err(SocketError::Local(e.into()));
        }
        Ok(_) => {}
    };

    *old_device = new_device;
    return Ok(());
}

/// Error resulting from attempting to change multicast membership settings for
/// a socket.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Error)]
pub enum SetMulticastMembershipError {
    /// The provided address does not match the provided device.
    #[error("provided address does not match the provided device")]
    AddressNotAvailable,
    /// The device does not exist.
    #[error("device does not exist")]
    DeviceDoesNotExist,
    /// The provided address does not match any address on the host.
    #[error("provided address does not match any address on the host")]
    NoDeviceWithAddress,
    /// No device or address was specified and there is no device with a route to the multicast
    /// address.
    #[error(
        "no device or address was specified and \
         there is no device with a route to the multicast address"
    )]
    NoDeviceAvailable,
    /// Tried to join a group again.
    #[error("tried to join a group again")]
    GroupAlreadyJoined,
    /// Tried to leave an unjoined group.
    #[error("tried to leave an unjoined group")]
    GroupNotJoined,
    /// The socket is bound to a device that doesn't match the one specified.
    #[error("socket is bound to a device that doesn't match the one specified")]
    WrongDevice,
}

/// Selects the interface for the given remote address, optionally with a
/// constraint on the source address.
fn pick_interface_for_addr<
    A: IpAddress,
    S: DatagramSocketSpec,
    BC: DatagramBindingsContext,
    CC: DatagramBoundStateContext<A::Version, BC, S>,
>(
    core_ctx: &mut CC,
    remote_addr: MulticastAddr<A>,
    source_addr: Option<SpecifiedAddr<A>>,
    marks: &Marks,
) -> Result<CC::DeviceId, SetMulticastMembershipError>
where
    A::Version: IpExt,
{
    core_ctx.with_transport_context(|core_ctx| match source_addr {
        Some(source_addr) => {
            BaseTransportIpContext::<A::Version, _>::with_devices_with_assigned_addr(
                core_ctx,
                source_addr,
                |mut devices| {
                    if let Some(d) = devices.next() {
                        if devices.next() == None {
                            return Ok(d);
                        }
                    }
                    Err(SetMulticastMembershipError::NoDeviceAvailable)
                },
            )
        }
        None => {
            let device = MulticastMembershipHandler::select_device_for_multicast_group(
                core_ctx,
                remote_addr,
                marks,
            )
            .map_err(|e| match e {
                ResolveRouteError::NoSrcAddr | ResolveRouteError::Unreachable => {
                    SetMulticastMembershipError::NoDeviceAvailable
                }
            })?;
            Ok(device)
        }
    })
}

/// Selector for the device to affect when changing multicast settings.
#[derive(Copy, Clone, Debug, Eq, GenericOverIp, PartialEq)]
#[generic_over_ip(A, IpAddress)]
pub enum MulticastInterfaceSelector<A: IpAddress, D> {
    /// Use the device with the assigned address.
    LocalAddress(SpecifiedAddr<A>),
    /// Use the device with the specified identifier.
    Interface(D),
}

/// Selector for the device to use when changing multicast membership settings.
///
/// This is like `Option<MulticastInterfaceSelector` except it specifies the
/// semantics of the `None` value as "pick any device".
#[derive(Copy, Clone, Debug, Eq, PartialEq, GenericOverIp)]
#[generic_over_ip(A, IpAddress)]
pub enum MulticastMembershipInterfaceSelector<A: IpAddress, D> {
    /// Use the specified interface.
    Specified(MulticastInterfaceSelector<A, D>),
    /// Pick any device with a route to the multicast target address.
    AnyInterfaceWithRoute,
}

impl<A: IpAddress, D> From<MulticastInterfaceSelector<A, D>>
    for MulticastMembershipInterfaceSelector<A, D>
{
    fn from(selector: MulticastInterfaceSelector<A, D>) -> Self {
        Self::Specified(selector)
    }
}

/// The shared datagram socket API.
#[derive(RefCast)]
#[repr(transparent)]
pub struct DatagramApi<I, C, S>(C, PhantomData<(S, I)>);

impl<I, C, S> DatagramApi<I, C, S> {
    /// Creates a new `DatagramApi` from `ctx`.
    pub fn new(ctx: C) -> Self {
        Self(ctx, PhantomData)
    }

    /// Creates a mutable borrow of a `DatagramApi` from a mutable borrow of
    /// `C`.
    pub fn wrap(ctx: &mut C) -> &mut Self {
        Self::ref_cast_mut(ctx)
    }
}

/// A local alias for [`DatagramSocketSpec::SocketId`] for use in
/// [`DatagramApi`].
///
/// TODO(https://github.com/rust-lang/rust/issues/8995): Make this an inherent
/// associated type.
type DatagramApiSocketId<I, C, S> = <S as DatagramSocketSpec>::SocketId<
    I,
    <<C as ContextPair>::CoreContext as DeviceIdContext<AnyDevice>>::WeakDeviceId,
>;
/// A local alias for [`DeviceIdContext::DeviceId`] for use in
/// [`DatagramApi`].
///
/// TODO(https://github.com/rust-lang/rust/issues/8995): Make this an inherent
/// associated type.
type DatagramApiDeviceId<C> =
    <<C as ContextPair>::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId;
/// A local alias for [`DeviceIdContext::WeakDeviceId`] for use in
/// [`DatagramApi`].
///
/// TODO(https://github.com/rust-lang/rust/issues/8995): Make this an inherent
/// associated type.
type DatagramApiWeakDeviceId<C> =
    <<C as ContextPair>::CoreContext as DeviceIdContext<AnyDevice>>::WeakDeviceId;

impl<I, C, S> DatagramApi<I, C, S>
where
    I: IpExt,
    C: ContextPair,
    C::BindingsContext: DatagramBindingsContext + SettingsContext<S::Settings>,
    C::CoreContext: DatagramStateContext<I, C::BindingsContext, S>,
    S: DatagramSocketSpec,
{
    fn core_ctx(&mut self) -> &mut C::CoreContext {
        let Self(pair, PhantomData) = self;
        pair.core_ctx()
    }

    fn bindings_ctx(&mut self) -> &mut C::BindingsContext {
        let Self(pair, PhantomData) = self;
        pair.bindings_ctx()
    }

    fn contexts(&mut self) -> (&mut C::CoreContext, &mut C::BindingsContext) {
        let Self(pair, PhantomData) = self;
        pair.contexts()
    }

    /// Creates a new datagram socket and inserts it into the list of all open
    /// datagram sockets for the provided spec `S`.
    ///
    /// The caller is responsible for calling  [`close`] when it's done with the
    /// resource.
    pub fn create(
        &mut self,
        external_data: S::ExternalData<I>,
        writable_listener: S::SocketWritableListener,
    ) -> S::SocketId<I, DatagramApiWeakDeviceId<C>> {
        let primary = {
            let settings = self.bindings_ctx().settings();
            create_primary_id(external_data, writable_listener, settings.as_ref())
        };
        let strong = PrimaryRc::clone_strong(&primary);
        self.core_ctx().with_all_sockets_mut(move |socket_set| {
            let strong = PrimaryRc::clone_strong(&primary);
            assert_matches::assert_matches!(socket_set.insert(strong, primary), None);
        });
        strong.into()
    }

    /// Like [`DatagramApi::create`], but uses default values.
    #[cfg(any(test, feature = "testutils"))]
    pub fn create_default(&mut self) -> S::SocketId<I, DatagramApiWeakDeviceId<C>>
    where
        S::ExternalData<I>: Default,
        S::SocketWritableListener: Default,
    {
        self.create(Default::default(), Default::default())
    }

    /// Collects all currently opened sockets.
    pub fn collect_all_sockets(&mut self) -> Vec<S::SocketId<I, DatagramApiWeakDeviceId<C>>> {
        self.core_ctx()
            .with_all_sockets(|socket_set| socket_set.keys().map(|s| s.clone().into()).collect())
    }

    /// Closes the socket and returns a custom payload.
    pub fn close<O, F>(
        &mut self,
        id: DatagramApiSocketId<I, C, S>,
        map: F,
    ) -> RemoveResourceResultWithContext<O, C::BindingsContext>
    where
        O: Send,
        F: Send + Clone + 'static + FnOnce(ReferenceState<I, DatagramApiWeakDeviceId<C>, S>) -> O,
    {
        let (core_ctx, bindings_ctx) = self.contexts();
        // Remove the socket from the list first to prevent double close.
        let primary = core_ctx.with_all_sockets_mut(|all_sockets| {
            all_sockets.remove(id.borrow()).expect("socket already closed")
        });
        core_ctx.with_socket_state(&id, |core_ctx, state| {
            let SocketState { ip_options, inner, sharing } = state;
            match inner {
                SocketStateInner::Unbound(UnboundSocketState { device: _ }) => {}
                SocketStateInner::Bound(state) => match core_ctx.dual_stack_context_mut() {
                    MaybeDualStack::DualStack(dual_stack) => {
                        let op = DualStackRemoveOperation::new_from_state(
                            dual_stack,
                            &id,
                            ip_options,
                            state,
                            sharing.clone(),
                        );
                        let _: DualStackInsertOperation<_, _, _> = dual_stack
                            .with_both_bound_sockets_mut(|_core_ctx, sockets, other_sockets| {
                                op.apply(sockets, other_sockets).expect("Failed to remove socket")
                            });
                    }
                    MaybeDualStack::NotDualStack(not_dual_stack) => {
                        let op = SingleStackRemoveOperation::new_from_state(
                            not_dual_stack,
                            &id,
                            state,
                            sharing.clone(),
                        );
                        let _: SingleStackInsertOperation<_, _, _> = core_ctx
                            .with_bound_sockets_mut(|_core_ctx, sockets| {
                                op.apply(sockets).expect("Failed to remove socket")
                            });
                    }
                },
            };
            DatagramBoundStateContext::<I, _, _>::with_transport_context(core_ctx, |core_ctx| {
                leave_all_joined_groups(core_ctx, bindings_ctx, &ip_options.multicast_memberships)
            });
        });

        // Drop the (hopefully last) strong ID before unwrapping the primary
        // reference.
        core::mem::drop(id);
        <C::BindingsContext as ReferenceNotifiersExt>::unwrap_or_notify_with_new_reference_notifier(
            primary, map,
        )
    }

    /// Returns the socket's bound/connection state information.
    pub fn get_info(
        &mut self,
        id: &DatagramApiSocketId<I, C, S>,
    ) -> SocketInfo<I::Addr, DatagramApiWeakDeviceId<C>> {
        self.core_ctx().with_socket_state(id, |_core_ctx, state| state.to_socket_info())
    }

    /// Binds the socket to a local address and port.
    pub fn listen(
        &mut self,
        id: &DatagramApiSocketId<I, C, S>,
        addr: Option<ZonedAddr<SpecifiedAddr<I::Addr>, DatagramApiDeviceId<C>>>,
        local_id: Option<<S::AddrSpec as SocketMapAddrSpec>::LocalIdentifier>,
    ) -> Result<(), Either<ExpectedUnboundError, LocalAddressError>> {
        let (core_ctx, bindings_ctx) = self.contexts();
        core_ctx.with_socket_state_mut(id, |core_ctx, state| {
            listen_inner::<_, _, _, S>(core_ctx, bindings_ctx, state, id, addr, local_id)
        })
    }

    /// Connects the datagram socket.
    pub fn connect(
        &mut self,
        id: &DatagramApiSocketId<I, C, S>,
        remote_ip: Option<ZonedAddr<SpecifiedAddr<I::Addr>, DatagramApiDeviceId<C>>>,
        remote_id: <S::AddrSpec as SocketMapAddrSpec>::RemoteIdentifier,
        extra: S::ConnStateExtra,
    ) -> Result<(), ConnectError> {
        let (core_ctx, bindings_ctx) = self.contexts();
        core_ctx.with_socket_state_mut(id, |core_ctx, state| {
            let conn_state = match (
                core_ctx.dual_stack_context_mut(),
                DualStackRemoteIp::<I, _>::new(remote_ip.clone()),
            ) {
                (MaybeDualStack::DualStack(ds), remote_ip) => {
                    let connect_op = DualStackConnectOperation::new_from_state(
                        ds, id, state, remote_ip, remote_id, extra,
                    )?;
                    let converter = ds.ds_converter();
                    let conn_state =
                        ds.with_both_bound_sockets_mut(|core_ctx, bound, other_bound| {
                            connect_op.apply(core_ctx, bindings_ctx, bound, other_bound)
                        })?;
                    Ok(converter.convert_back(conn_state))
                }
                (MaybeDualStack::NotDualStack(nds), DualStackRemoteIp::ThisStack(remote_ip)) => {
                    let connect_op = SingleStackConnectOperation::new_from_state(
                        nds, id, state, remote_ip, remote_id, extra,
                    );
                    let converter = nds.nds_converter();
                    let conn_state = core_ctx.with_bound_sockets_mut(|core_ctx, bound| {
                        connect_op.apply(core_ctx, bindings_ctx, bound)
                    })?;
                    Ok(converter.convert_back(conn_state))
                }
                (MaybeDualStack::NotDualStack(_), DualStackRemoteIp::OtherStack(_)) => {
                    Err(ConnectError::RemoteUnexpectedlyMapped)
                }
            }?;
            let original_bound_addr = match &state.inner {
                SocketStateInner::Unbound(_) => None,
                SocketStateInner::Bound(BoundSocketState {
                    socket_type: _,
                    original_bound_addr,
                }) => original_bound_addr.clone(),
            };
            state.inner = SocketStateInner::Bound(BoundSocketState {
                socket_type: BoundSocketStateType::Connected(conn_state),
                original_bound_addr,
            });
            Ok(())
        })
    }

    /// Disconnects a connected socket.
    pub fn disconnect_connected(
        &mut self,
        id: &DatagramApiSocketId<I, C, S>,
    ) -> Result<(), ExpectedConnError> {
        self.core_ctx().with_socket_state_mut(id, |core_ctx, state| {
            let SocketState { ip_options, inner, sharing } = state;
            let inner_state = match inner {
                SocketStateInner::Unbound(_) => return Err(ExpectedConnError),
                SocketStateInner::Bound(state) => state,
            };
            let BoundSocketState { socket_type, original_bound_addr } = inner_state;
            let conn_state = match socket_type {
                BoundSocketStateType::Listener(_) => {
                    return Err(ExpectedConnError);
                }
                BoundSocketStateType::Connected(state) => state,
            };

            let clear_device_on_disconnect = match core_ctx.dual_stack_context_mut() {
                MaybeDualStack::DualStack(dual_stack) => {
                    match dual_stack.ds_converter().convert(conn_state) {
                        DualStackConnState::ThisStack(conn_state) => {
                            conn_state.clear_device_on_disconnect
                        }
                        DualStackConnState::OtherStack(conn_state) => {
                            conn_state.clear_device_on_disconnect
                        }
                    }
                }
                MaybeDualStack::NotDualStack(not_dual_stack) => {
                    not_dual_stack.nds_converter().convert(conn_state).clear_device_on_disconnect
                }
            };

            state.inner = match original_bound_addr {
                None => SocketStateInner::Unbound(disconnect_to_unbound(
                    core_ctx,
                    id,
                    clear_device_on_disconnect,
                    &state.ip_options,
                    inner_state,
                    sharing.clone(),
                )),
                Some(original_bound_addr) => SocketStateInner::Bound(disconnect_to_listener(
                    core_ctx,
                    id,
                    original_bound_addr.clone(),
                    clear_device_on_disconnect,
                    &ip_options,
                    inner_state,
                    sharing.clone(),
                )),
            };
            Ok(())
        })
    }

    /// Disconnects any socket (bound or unbound), resetting it to unbound state
    /// and clearing the bound device.
    pub fn disconnect_any_to_unbound(&mut self, id: &DatagramApiSocketId<I, C, S>) {
        self.core_ctx().with_socket_state_mut(id, |core_ctx, state| {
            let SocketState { ip_options, inner, sharing, .. } = state;
            match inner {
                SocketStateInner::Unbound(UnboundSocketState { device }) => {
                    *device = None;
                }
                SocketStateInner::Bound(bound_state) => {
                    let unbound_state = disconnect_to_unbound(
                        core_ctx,
                        id,
                        true,
                        ip_options,
                        bound_state,
                        sharing.clone(),
                    );
                    state.inner = SocketStateInner::Unbound(unbound_state);
                }
            }
        });
    }

    /// Returns the socket's shutdown state.
    pub fn get_shutdown_connected(
        &mut self,
        id: &DatagramApiSocketId<I, C, S>,
    ) -> Option<ShutdownType> {
        self.core_ctx().with_socket_state(id, |core_ctx, state| {
            let state = match &state.inner {
                SocketStateInner::Unbound(_) => return None,
                SocketStateInner::Bound(BoundSocketState {
                    socket_type,
                    original_bound_addr: _,
                }) => match socket_type {
                    BoundSocketStateType::Listener(_) => return None,
                    BoundSocketStateType::Connected(state) => state,
                },
            };
            let Shutdown { send, receive } = match core_ctx.dual_stack_context_mut() {
                MaybeDualStack::DualStack(ds) => ds.ds_converter().convert(state).as_ref(),
                MaybeDualStack::NotDualStack(nds) => nds.nds_converter().convert(state).as_ref(),
            };
            ShutdownType::from_send_receive(*send, *receive)
        })
    }

    /// Shuts down the socket.
    ///
    /// `which` determines the shutdown type.
    pub fn shutdown_connected(
        &mut self,
        id: &DatagramApiSocketId<I, C, S>,
        which: ShutdownType,
    ) -> Result<(), ExpectedConnError> {
        self.core_ctx().with_socket_state_mut(id, |core_ctx, state| {
            let state = match &mut state.inner {
                SocketStateInner::Unbound(_) => return Err(ExpectedConnError),
                SocketStateInner::Bound(BoundSocketState {
                    socket_type,
                    original_bound_addr: _,
                }) => match socket_type {
                    BoundSocketStateType::Listener(_) => {
                        return Err(ExpectedConnError);
                    }
                    BoundSocketStateType::Connected(state) => state,
                },
            };
            let (shutdown_send, shutdown_receive) = which.to_send_receive();
            let Shutdown { send, receive } = match core_ctx.dual_stack_context_mut() {
                MaybeDualStack::DualStack(ds) => ds.ds_converter().convert(state).as_mut(),
                MaybeDualStack::NotDualStack(nds) => nds.nds_converter().convert(state).as_mut(),
            };
            *send |= shutdown_send;
            *receive |= shutdown_receive;
            Ok(())
        })
    }

    /// Sends data over a connected datagram socket.
    pub fn send_conn<B: BufferMut>(
        &mut self,
        id: &DatagramApiSocketId<I, C, S>,
        body: B,
        send_token: S::SendToken,
    ) -> Result<(), SendError<S::SerializeError>> {
        let (core_ctx, bindings_ctx) = self.contexts();
        core_ctx.with_socket_state(id, |core_ctx, state| {
            let SocketState { inner, ip_options, sharing: _ } = state;
            let state = match inner {
                SocketStateInner::Unbound(_) => return Err(SendError::NotConnected),
                SocketStateInner::Bound(BoundSocketState {
                    socket_type,
                    original_bound_addr: _,
                }) => match socket_type {
                    BoundSocketStateType::Listener(_) => {
                        return Err(SendError::NotConnected);
                    }
                    BoundSocketStateType::Connected(state) => state,
                },
            };

            struct SendParams<
                'a,
                I: IpExt,
                S: DatagramSocketSpec,
                D: WeakDeviceIdentifier,
                O: SendOptions<I> + RouteResolutionOptions<I>,
            > {
                socket: &'a IpSock<I, D>,
                ip: &'a ConnIpAddr<
                    I::Addr,
                    <S::AddrSpec as SocketMapAddrSpec>::LocalIdentifier,
                    <S::AddrSpec as SocketMapAddrSpec>::RemoteIdentifier,
                >,
                options: O,
            }

            enum Operation<
                'a,
                I: DualStackIpExt,
                S: DatagramSocketSpec,
                D: WeakDeviceIdentifier,
                BC: DatagramBindingsContext,
                DualStackSC: DualStackDatagramBoundStateContext<I, BC, S>,
                CC: DatagramBoundStateContext<I, BC, S>,
                O: SendOptions<I> + RouteResolutionOptions<I>,
                OtherO: SendOptions<I::OtherVersion> + RouteResolutionOptions<I::OtherVersion>,
            > {
                SendToThisStack((SendParams<'a, I, S, D, O>, &'a mut CC)),
                SendToOtherStack(
                    (SendParams<'a, I::OtherVersion, S, D, OtherO>, &'a mut DualStackSC),
                ),
                // Allow `Operation` to be generic over `B` and `C` so that they can
                // be used in trait bounds for `DualStackSC` and `SC`.
                _Phantom((Never, PhantomData<BC>)),
            }

            let (shutdown, operation) = match core_ctx.dual_stack_context_mut() {
                MaybeDualStack::DualStack(dual_stack) => {
                    match dual_stack.ds_converter().convert(state) {
                        DualStackConnState::ThisStack(ConnState {
                            socket,
                            clear_device_on_disconnect: _,
                            shutdown,
                            addr: ConnAddr { ip, device: _ },
                            extra: _,
                        }) => (
                            shutdown,
                            Operation::SendToThisStack((
                                SendParams {
                                    socket,
                                    ip,
                                    options: ip_options.this_stack_options_ref(),
                                },
                                core_ctx,
                            )),
                        ),
                        DualStackConnState::OtherStack(ConnState {
                            socket,
                            clear_device_on_disconnect: _,
                            shutdown,
                            addr: ConnAddr { ip, device: _ },
                            extra: _,
                        }) => (
                            shutdown,
                            Operation::SendToOtherStack((
                                SendParams {
                                    socket,
                                    ip,
                                    options: ip_options.other_stack_options_ref(dual_stack),
                                },
                                dual_stack,
                            )),
                        ),
                    }
                }
                MaybeDualStack::NotDualStack(not_dual_stack) => {
                    let ConnState {
                        socket,
                        clear_device_on_disconnect: _,
                        shutdown,
                        addr: ConnAddr { ip, device: _ },
                        extra: _,
                    } = not_dual_stack.nds_converter().convert(state);
                    (
                        shutdown,
                        Operation::SendToThisStack((
                            SendParams { socket, ip, options: ip_options.this_stack_options_ref() },
                            core_ctx,
                        )),
                    )
                }
            };

            let Shutdown { send: shutdown_send, receive: _ } = shutdown;
            if *shutdown_send {
                return Err(SendError::NotWriteable);
            }

            match operation {
                Operation::SendToThisStack((SendParams { socket, ip, options }, core_ctx)) => {
                    let tx_metadata = id
                        .borrow()
                        .send_buffer
                        .prepare_for_send::<I, _, _, _>(id, &body, send_token)?;
                    let packet =
                        S::make_packet::<I, _>(body, &ip).map_err(SendError::SerializeError)?;
                    DatagramBoundStateContext::with_transport_context(core_ctx, |core_ctx| {
                        let tx_metadata = core_ctx.convert_tx_meta(tx_metadata);
                        core_ctx
                            .send_ip_packet(bindings_ctx, &socket, packet, &options, tx_metadata)
                            .map_err(|send_error| SendError::IpSock(send_error))
                    })
                }
                Operation::SendToOtherStack((SendParams { socket, ip, options }, dual_stack)) => {
                    let tx_metadata = id
                        .borrow()
                        .send_buffer
                        .prepare_for_send::<I::OtherVersion, _, _, _>(id, &body, send_token)?;
                    let packet = S::make_packet::<I::OtherVersion, _>(body, &ip)
                        .map_err(SendError::SerializeError)?;
                    DualStackDatagramBoundStateContext::with_transport_context::<_, _>(
                        dual_stack,
                        |core_ctx| {
                            let tx_metadata = core_ctx.convert_tx_meta(tx_metadata);
                            core_ctx
                                .send_ip_packet(
                                    bindings_ctx,
                                    &socket,
                                    packet,
                                    &options,
                                    tx_metadata,
                                )
                                .map_err(|send_error| SendError::IpSock(send_error))
                        },
                    )
                }
            }
        })
    }

    /// Sends a datagram to the provided remote node.
    pub fn send_to<B: BufferMut>(
        &mut self,
        id: &DatagramApiSocketId<I, C, S>,
        remote_ip: Option<ZonedAddr<SpecifiedAddr<I::Addr>, DatagramApiDeviceId<C>>>,
        remote_identifier: <S::AddrSpec as SocketMapAddrSpec>::RemoteIdentifier,
        body: B,
        send_token: S::SendToken,
    ) -> Result<(), SendToError<S::SerializeError>> {
        let (core_ctx, bindings_ctx) = self.contexts();
        core_ctx.with_socket_state_mut(id, |core_ctx, state| {
            match listen_inner(core_ctx, bindings_ctx, state, id, None, None) {
                Ok(()) | Err(Either::Left(ExpectedUnboundError)) => (),
                Err(Either::Right(e)) => return Err(SendToError::LocalAddress(e)),
            };
            let SocketState { inner, ip_options, sharing: _ } = state;
            let state = match inner {
                SocketStateInner::Unbound(_) => panic!("expected bound socket"),
                SocketStateInner::Bound(BoundSocketState {
                    socket_type: state,
                    original_bound_addr: _,
                }) => state,
            };

            enum Operation<
                'a,
                I: DualStackIpExt,
                S: DatagramSocketSpec,
                D: WeakDeviceIdentifier,
                BC: DatagramBindingsContext,
                DualStackSC: DualStackDatagramBoundStateContext<I, BC, S>,
                CC: DatagramBoundStateContext<I, BC, S>,
            > {
                SendToThisStack((SendOneshotParameters<'a, I, I, S, D>, &'a mut CC)),

                SendToOtherStack(
                    (SendOneshotParameters<'a, I, I::OtherVersion, S, D>, &'a mut DualStackSC),
                ),
                // Allow `Operation` to be generic over `B` and `C` so that they can
                // be used in trait bounds for `DualStackSC` and `SC`.
                _Phantom((Never, PhantomData<BC>)),
            }

            let (operation, shutdown) = match (
                core_ctx.dual_stack_context_mut(),
                DualStackRemoteIp::<I, _>::new(remote_ip.clone()),
            ) {
                (MaybeDualStack::NotDualStack(_), DualStackRemoteIp::OtherStack(_)) => {
                    return Err(SendToError::RemoteUnexpectedlyMapped);
                }
                (MaybeDualStack::NotDualStack(nds), DualStackRemoteIp::ThisStack(remote_ip)) => {
                    match state {
                        BoundSocketStateType::Listener(ListenerState {
                            addr: ListenerAddr { ip, device },
                        }) => {
                            let ListenerIpAddr { addr, identifier } =
                                nds.nds_converter().convert(ip.clone());
                            (
                                Operation::SendToThisStack((
                                    SendOneshotParameters {
                                        local_ip: addr,
                                        local_id: identifier,
                                        remote_ip,
                                        remote_id: remote_identifier,
                                        device,
                                        options: ip_options.this_stack_options_ref(),
                                        id,
                                        send_token,
                                    },
                                    core_ctx,
                                )),
                                None,
                            )
                        }
                        BoundSocketStateType::Connected(state) => {
                            let ConnState {
                                socket: _,
                                clear_device_on_disconnect: _,
                                shutdown,
                                addr:
                                    ConnAddr {
                                        ip: ConnIpAddr { local: (local_ip, local_id), remote: _ },
                                        device,
                                    },
                                extra: _,
                            } = nds.nds_converter().convert(state);
                            (
                                Operation::SendToThisStack((
                                    SendOneshotParameters {
                                        local_ip: Some(*local_ip),
                                        local_id: *local_id,
                                        remote_ip,
                                        remote_id: remote_identifier,
                                        device,
                                        options: ip_options.this_stack_options_ref(),
                                        id,
                                        send_token,
                                    },
                                    core_ctx,
                                )),
                                Some(shutdown),
                            )
                        }
                    }
                }
                (MaybeDualStack::DualStack(ds), remote_ip) => match state {
                    BoundSocketStateType::Listener(ListenerState {
                        addr: ListenerAddr { ip, device },
                    }) => match (ds.ds_converter().convert(ip), remote_ip) {
                        (
                            DualStackListenerIpAddr::ThisStack(_),
                            DualStackRemoteIp::OtherStack(_),
                        ) => return Err(SendToError::RemoteUnexpectedlyMapped),
                        (
                            DualStackListenerIpAddr::OtherStack(_),
                            DualStackRemoteIp::ThisStack(_),
                        ) => return Err(SendToError::RemoteUnexpectedlyNonMapped),
                        (
                            DualStackListenerIpAddr::ThisStack(ListenerIpAddr { addr, identifier }),
                            DualStackRemoteIp::ThisStack(remote_ip),
                        ) => (
                            Operation::SendToThisStack((
                                SendOneshotParameters {
                                    local_ip: *addr,
                                    local_id: *identifier,
                                    remote_ip,
                                    remote_id: remote_identifier,
                                    device,
                                    options: ip_options.this_stack_options_ref(),
                                    id,
                                    send_token,
                                },
                                core_ctx,
                            )),
                            None,
                        ),
                        (
                            DualStackListenerIpAddr::BothStacks(identifier),
                            DualStackRemoteIp::ThisStack(remote_ip),
                        ) => (
                            Operation::SendToThisStack((
                                SendOneshotParameters {
                                    local_ip: None,
                                    local_id: *identifier,
                                    remote_ip,
                                    remote_id: remote_identifier,
                                    device,
                                    options: ip_options.this_stack_options_ref(),
                                    id,
                                    send_token,
                                },
                                core_ctx,
                            )),
                            None,
                        ),
                        (
                            DualStackListenerIpAddr::OtherStack(ListenerIpAddr {
                                addr,
                                identifier,
                            }),
                            DualStackRemoteIp::OtherStack(remote_ip),
                        ) => (
                            Operation::SendToOtherStack((
                                SendOneshotParameters {
                                    local_ip: *addr,
                                    local_id: *identifier,
                                    remote_ip,
                                    remote_id: remote_identifier,
                                    device,
                                    options: ip_options.other_stack_options_ref(ds),
                                    id,
                                    send_token,
                                },
                                ds,
                            )),
                            None,
                        ),
                        (
                            DualStackListenerIpAddr::BothStacks(identifier),
                            DualStackRemoteIp::OtherStack(remote_ip),
                        ) => (
                            Operation::SendToOtherStack((
                                SendOneshotParameters {
                                    local_ip: None,
                                    local_id: *identifier,
                                    remote_ip,
                                    remote_id: remote_identifier,
                                    device,
                                    options: ip_options.other_stack_options_ref(ds),
                                    id,
                                    send_token,
                                },
                                ds,
                            )),
                            None,
                        ),
                    },
                    BoundSocketStateType::Connected(state) => {
                        match (ds.ds_converter().convert(state), remote_ip) {
                            (
                                DualStackConnState::ThisStack(_),
                                DualStackRemoteIp::OtherStack(_),
                            ) => return Err(SendToError::RemoteUnexpectedlyMapped),
                            (
                                DualStackConnState::OtherStack(_),
                                DualStackRemoteIp::ThisStack(_),
                            ) => {
                                return Err(SendToError::RemoteUnexpectedlyNonMapped);
                            }
                            (
                                DualStackConnState::ThisStack(state),
                                DualStackRemoteIp::ThisStack(remote_ip),
                            ) => {
                                let ConnState {
                                    socket: _,
                                    clear_device_on_disconnect: _,
                                    shutdown,
                                    addr,
                                    extra: _,
                                } = state;
                                let ConnAddr {
                                    ip: ConnIpAddr { local: (local_ip, local_id), remote: _ },
                                    device,
                                } = addr;
                                (
                                    Operation::SendToThisStack((
                                        SendOneshotParameters {
                                            local_ip: Some(*local_ip),
                                            local_id: *local_id,
                                            remote_ip,
                                            remote_id: remote_identifier,
                                            device,
                                            options: ip_options.this_stack_options_ref(),
                                            id,
                                            send_token,
                                        },
                                        core_ctx,
                                    )),
                                    Some(shutdown),
                                )
                            }
                            (
                                DualStackConnState::OtherStack(state),
                                DualStackRemoteIp::OtherStack(remote_ip),
                            ) => {
                                let ConnState {
                                    socket: _,
                                    clear_device_on_disconnect: _,
                                    shutdown,
                                    addr,
                                    extra: _,
                                } = state;
                                let ConnAddr {
                                    ip: ConnIpAddr { local: (local_ip, local_id), .. },
                                    device,
                                } = addr;
                                (
                                    Operation::SendToOtherStack((
                                        SendOneshotParameters {
                                            local_ip: Some(*local_ip),
                                            local_id: *local_id,
                                            remote_ip,
                                            remote_id: remote_identifier,
                                            device,
                                            options: ip_options.other_stack_options_ref(ds),
                                            id,
                                            send_token,
                                        },
                                        ds,
                                    )),
                                    Some(shutdown),
                                )
                            }
                        }
                    }
                },
            };

            if let Some(Shutdown { send: shutdown_write, receive: _ }) = shutdown {
                if *shutdown_write {
                    return Err(SendToError::NotWriteable);
                }
            }

            match operation {
                Operation::SendToThisStack((params, core_ctx)) => {
                    DatagramBoundStateContext::with_transport_context(core_ctx, |core_ctx| {
                        send_oneshot(core_ctx, bindings_ctx, params, body)
                    })
                }
                Operation::SendToOtherStack((params, core_ctx)) => {
                    DualStackDatagramBoundStateContext::with_transport_context::<_, _>(
                        core_ctx,
                        |core_ctx| send_oneshot(core_ctx, bindings_ctx, params, body),
                    )
                }
            }
        })
    }

    /// Returns the bound device for the socket.
    pub fn get_bound_device(
        &mut self,
        id: &DatagramApiSocketId<I, C, S>,
    ) -> Option<DatagramApiWeakDeviceId<C>> {
        self.core_ctx().with_socket_state(id, |core_ctx, state| state.get_device(core_ctx).clone())
    }

    /// Sets the socket's bound device to `new_device`.
    pub fn set_device(
        &mut self,
        id: &DatagramApiSocketId<I, C, S>,
        new_device: Option<&DatagramApiDeviceId<C>>,
    ) -> Result<(), SocketError> {
        let (core_ctx, bindings_ctx) = self.contexts();
        core_ctx.with_socket_state_mut(id, |core_ctx, state| {
            let SocketState { inner, ip_options, sharing } = state;
            match inner {
                SocketStateInner::Unbound(state) => {
                    let UnboundSocketState { device } = state;
                    *device = new_device.map(|d| d.downgrade());
                    Ok(())
                }
                SocketStateInner::Bound(BoundSocketState {
                    socket_type,
                    original_bound_addr: _,
                }) => {
                    // Information about the set-device operation for the given
                    // socket.
                    enum Operation<
                        'a,
                        I: IpExt,
                        D: WeakDeviceIdentifier,
                        S: DatagramSocketSpec,
                        CC,
                        DualStackSC,
                    > {
                        ThisStack {
                            params: SetBoundDeviceParameters<'a, I, D, S>,
                            core_ctx: CC,
                        },
                        OtherStack {
                            params: SetBoundDeviceParameters<'a, I::OtherVersion, D, S>,
                            core_ctx: DualStackSC,
                        },
                        ListenerBothStacks {
                            identifier: <S::AddrSpec as SocketMapAddrSpec>::LocalIdentifier,
                            device: &'a mut Option<D>,
                            core_ctx: DualStackSC,
                        },
                    }

                    // Determine which operation needs to be applied.
                    let op = match core_ctx.dual_stack_context_mut() {
                        MaybeDualStack::DualStack(ds) => match socket_type {
                            BoundSocketStateType::Listener(ListenerState {
                                addr: ListenerAddr { ip, device },
                            }) => match ds.ds_converter().convert(ip) {
                                DualStackListenerIpAddr::ThisStack(ip) => Operation::ThisStack {
                                    params: SetBoundDeviceParameters::Listener { ip, device },
                                    core_ctx,
                                },
                                DualStackListenerIpAddr::OtherStack(ip) => Operation::OtherStack {
                                    params: SetBoundDeviceParameters::Listener { ip, device },
                                    core_ctx: ds,
                                },
                                DualStackListenerIpAddr::BothStacks(identifier) => {
                                    Operation::ListenerBothStacks {
                                        identifier: *identifier,
                                        device,
                                        core_ctx: ds,
                                    }
                                }
                            },
                            BoundSocketStateType::Connected(state) => {
                                match ds.ds_converter().convert(state) {
                                    DualStackConnState::ThisStack(state) => Operation::ThisStack {
                                        params: SetBoundDeviceParameters::Connected(state),
                                        core_ctx,
                                    },
                                    DualStackConnState::OtherStack(state) => {
                                        Operation::OtherStack {
                                            params: SetBoundDeviceParameters::Connected(state),
                                            core_ctx: ds,
                                        }
                                    }
                                }
                            }
                        },
                        MaybeDualStack::NotDualStack(nds) => match socket_type {
                            BoundSocketStateType::Listener(ListenerState {
                                addr: ListenerAddr { ip, device },
                            }) => Operation::ThisStack {
                                params: SetBoundDeviceParameters::Listener {
                                    ip: nds.nds_converter().convert(ip),
                                    device,
                                },
                                core_ctx,
                            },
                            BoundSocketStateType::Connected(state) => Operation::ThisStack {
                                params: SetBoundDeviceParameters::Connected(
                                    nds.nds_converter().convert(state),
                                ),
                                core_ctx,
                            },
                        },
                    };

                    // Apply the operation
                    match op {
                        Operation::ThisStack { params, core_ctx } => {
                            let socket_id = S::make_bound_socket_map_id(id);
                            DatagramBoundStateContext::<I, _, _>::with_bound_sockets_mut(
                                core_ctx,
                                |core_ctx, bound| {
                                    set_bound_device_single_stack(
                                        bindings_ctx,
                                        core_ctx,
                                        params,
                                        bound,
                                        &socket_id,
                                        &ip_options.common,
                                        new_device,
                                        sharing.clone(),
                                    )
                                },
                            )
                        }
                        Operation::OtherStack { params, core_ctx } => {
                            let socket_id = core_ctx.to_other_bound_socket_id(id);
                            core_ctx.with_other_bound_sockets_mut(|core_ctx, bound| {
                                set_bound_device_single_stack(
                                    bindings_ctx,
                                    core_ctx,
                                    params,
                                    bound,
                                    &socket_id,
                                    &ip_options.common,
                                    new_device,
                                    sharing.clone(),
                                )
                            })
                        }
                        Operation::ListenerBothStacks { identifier, device, core_ctx } => {
                            let socket_id = PairedBoundSocketIds::<_, _, S> {
                                this: S::make_bound_socket_map_id(id),
                                other: core_ctx.to_other_bound_socket_id(id),
                            };
                            core_ctx.with_both_bound_sockets_mut(|_core_ctx, bound, other_bound| {
                                set_bound_device_listener_both_stacks(
                                    device,
                                    identifier,
                                    PairedSocketMapMut { bound, other_bound },
                                    socket_id,
                                    new_device.map(|d| d.downgrade()),
                                    sharing.clone(),
                                )
                            })
                        }
                    }
                }
            }
        })
    }

    /// Sets the specified socket's membership status for the given group.
    ///
    /// An error is returned if the membership change request is invalid
    /// (e.g. leaving a group that was not joined, or joining a group multiple
    /// times) or if the device to use to join is unspecified or conflicts with
    /// the existing socket state.
    pub fn set_multicast_membership(
        &mut self,
        id: &DatagramApiSocketId<I, C, S>,
        multicast_group: MulticastAddr<I::Addr>,
        interface: MulticastMembershipInterfaceSelector<I::Addr, DatagramApiDeviceId<C>>,
        want_membership: bool,
    ) -> Result<(), SetMulticastMembershipError> {
        let (core_ctx, bindings_ctx) = self.contexts();
        core_ctx.with_socket_state_mut(id, |core_ctx, state| {
            let ip_options = state.options();
            let bound_device = state.get_device(core_ctx);

            let interface = match interface {
                MulticastMembershipInterfaceSelector::Specified(selector) => match selector {
                    MulticastInterfaceSelector::Interface(device) => {
                        if bound_device.as_ref().is_some_and(|d| d != &device) {
                            return Err(SetMulticastMembershipError::WrongDevice);
                        } else {
                            EitherDeviceId::Strong(device)
                        }
                    }
                    MulticastInterfaceSelector::LocalAddress(addr) => {
                        EitherDeviceId::Strong(pick_interface_for_addr(
                            core_ctx,
                            multicast_group,
                            Some(addr),
                            &ip_options.common.marks,
                        )?)
                    }
                },
                MulticastMembershipInterfaceSelector::AnyInterfaceWithRoute => {
                    if let Some(bound_device) = bound_device.as_ref() {
                        EitherDeviceId::Weak(bound_device.clone())
                    } else {
                        EitherDeviceId::Strong(pick_interface_for_addr(
                            core_ctx,
                            multicast_group,
                            None,
                            &ip_options.common.marks,
                        )?)
                    }
                }
            };

            let ip_options = state.options_mut();

            let Some(strong_interface) = interface.as_strong() else {
                return Err(SetMulticastMembershipError::DeviceDoesNotExist);
            };

            let change = ip_options
                .multicast_memberships
                .apply_membership_change(multicast_group, &interface.as_weak(), want_membership)
                .ok_or(if want_membership {
                    SetMulticastMembershipError::GroupAlreadyJoined
                } else {
                    SetMulticastMembershipError::GroupNotJoined
                })?;

            DatagramBoundStateContext::<I, _, _>::with_transport_context(core_ctx, |core_ctx| {
                match change {
                    MulticastMembershipChange::Join => {
                        MulticastMembershipHandler::<I, _>::join_multicast_group(
                            core_ctx,
                            bindings_ctx,
                            &strong_interface,
                            multicast_group,
                        )
                    }
                    MulticastMembershipChange::Leave => {
                        MulticastMembershipHandler::<I, _>::leave_multicast_group(
                            core_ctx,
                            bindings_ctx,
                            &strong_interface,
                            multicast_group,
                        )
                    }
                }
            });

            Ok(())
        })
    }

    /// Updates the socket's IP hop limits.
    pub fn update_ip_hop_limit(
        &mut self,
        id: &DatagramApiSocketId<I, C, S>,
        update: impl FnOnce(&mut SocketHopLimits<I>),
    ) {
        self.core_ctx().with_socket_state_mut(id, |_core_ctx, state| {
            let options = state.options_mut();

            update(&mut options.socket_options.hop_limits)
        })
    }

    /// Returns the socket's IP hop limits.
    pub fn get_ip_hop_limits(&mut self, id: &DatagramApiSocketId<I, C, S>) -> HopLimits {
        self.core_ctx().with_socket_state(id, |core_ctx, state| {
            let options = state.options();
            let device = state.get_device(core_ctx);
            let device = device.as_ref().and_then(|d| d.upgrade());
            DatagramBoundStateContext::<I, _, _>::with_transport_context(core_ctx, |core_ctx| {
                options.socket_options.hop_limits.get_limits_with_defaults(
                    &BaseTransportIpContext::<I, _>::get_default_hop_limits(
                        core_ctx,
                        device.as_ref(),
                    ),
                )
            })
        })
    }

    /// Calls the callback with mutable access to [`S::OtherStackIpOptions<I,
    /// D>`].
    ///
    /// If the socket is bound, the callback is not called, and instead an
    /// `ExpectedUnboundError` is returned.
    pub fn with_other_stack_ip_options_mut_if_unbound<R>(
        &mut self,
        id: &DatagramApiSocketId<I, C, S>,
        cb: impl FnOnce(&mut S::OtherStackIpOptions<I, DatagramApiWeakDeviceId<C>>) -> R,
    ) -> Result<R, ExpectedUnboundError> {
        self.core_ctx().with_socket_state_mut(id, |_core_ctx, state| {
            let is_unbound = match &state.inner {
                SocketStateInner::Unbound(_) => true,
                SocketStateInner::Bound(_) => false,
            };
            if is_unbound {
                let options = state.options_mut();
                Ok(cb(&mut options.other_stack))
            } else {
                Err(ExpectedUnboundError)
            }
        })
    }

    /// Calls the callback with mutable access to [`S::OtherStackIpOptions<I,
    /// D>`].
    pub fn with_other_stack_ip_options_mut<R>(
        &mut self,
        id: &DatagramApiSocketId<I, C, S>,
        cb: impl FnOnce(&mut S::OtherStackIpOptions<I, DatagramApiWeakDeviceId<C>>) -> R,
    ) -> R {
        self.core_ctx().with_socket_state_mut(id, |_core_ctx, state| {
            let options = state.options_mut();
            cb(&mut options.other_stack)
        })
    }

    /// Calls the callback with access to [`S::OtherStackIpOptions<I, D>`].
    pub fn with_other_stack_ip_options<R>(
        &mut self,
        id: &DatagramApiSocketId<I, C, S>,
        cb: impl FnOnce(&S::OtherStackIpOptions<I, DatagramApiWeakDeviceId<C>>) -> R,
    ) -> R {
        self.core_ctx().with_socket_state(id, |_core_ctx, state| cb(&state.options().other_stack))
    }

    /// Calls the callback with access to [`S::OtherStackIpOptions<I, D>`], and the
    /// default [`HopLimits`] for `I::OtherVersion`.
    ///
    /// If dualstack operations are not supported, the callback is not called, and
    /// instead `NotDualStackCapableError` is returned.
    pub fn with_other_stack_ip_options_and_default_hop_limits<R>(
        &mut self,
        id: &DatagramApiSocketId<I, C, S>,
        cb: impl FnOnce(&S::OtherStackIpOptions<I, DatagramApiWeakDeviceId<C>>, HopLimits) -> R,
    ) -> Result<R, NotDualStackCapableError> {
        self.core_ctx().with_socket_state(id, |core_ctx, state| {
            let options = state.options();
            let device = state.get_device(core_ctx).as_ref().and_then(|d| d.upgrade());
            match DatagramBoundStateContext::<I, _, _>::dual_stack_context_mut(core_ctx) {
                MaybeDualStack::NotDualStack(_) => Err(NotDualStackCapableError),
                MaybeDualStack::DualStack(ds) => {
                    let default_hop_limits =
                        DualStackDatagramBoundStateContext::<I, _, _>::with_transport_context(
                            ds,
                            |sync_ctx| {
                                BaseTransportIpContext::<I, _>::get_default_hop_limits(
                                    sync_ctx,
                                    device.as_ref(),
                                )
                            },
                        );
                    Ok(cb(&options.other_stack, default_hop_limits))
                }
            }
        })
    }

    /// Calls the callback with mutable access to
    /// [`DatagramIpSpecificSocketOptions<I,D>`] and
    /// [`S::OtherStackIpOptions<I, D>`].
    pub fn with_both_stacks_ip_options_mut<R>(
        &mut self,
        id: &DatagramApiSocketId<I, C, S>,
        cb: impl FnOnce(
            &mut DatagramIpSpecificSocketOptions<I, DatagramApiWeakDeviceId<C>>,
            &mut S::OtherStackIpOptions<I, DatagramApiWeakDeviceId<C>>,
        ) -> R,
    ) -> R {
        self.core_ctx().with_socket_state_mut(id, |_core_ctx, state| {
            let options = state.options_mut();
            cb(&mut options.socket_options, &mut options.other_stack)
        })
    }

    /// Calls the callback with access to [`DatagramIpSpecificSocketOptions<I,
    /// D>`] and [`S::OtherStackIpOptions<I, D>`].
    pub fn with_both_stacks_ip_options<R>(
        &mut self,
        id: &DatagramApiSocketId<I, C, S>,
        cb: impl FnOnce(
            &DatagramIpSpecificSocketOptions<I, DatagramApiWeakDeviceId<C>>,
            &S::OtherStackIpOptions<I, DatagramApiWeakDeviceId<C>>,
        ) -> R,
    ) -> R {
        self.core_ctx().with_socket_state(id, |_core_ctx, state| {
            let options = state.options();
            cb(&options.socket_options, &options.other_stack)
        })
    }

    /// Updates the socket's sharing state to the result of `f`.
    ///
    /// `f` is given mutable access to the sharing state and is called under the
    /// socket lock, allowing for atomic updates to the sharing state.
    pub fn update_sharing(
        &mut self,
        id: &DatagramApiSocketId<I, C, S>,
        f: impl FnOnce(&mut S::SharingState),
    ) -> Result<(), ExpectedUnboundError> {
        self.core_ctx().with_socket_state_mut(id, |_core_ctx, state| {
            match &mut state.inner {
                SocketStateInner::Unbound(_) => (),
                SocketStateInner::Bound(_) => return Err(ExpectedUnboundError),
            };

            f(&mut state.sharing);
            Ok(())
        })
    }

    /// Returns the socket's sharing state.
    pub fn get_sharing(&mut self, id: &DatagramApiSocketId<I, C, S>) -> S::SharingState {
        self.core_ctx().with_socket_state(id, |_core_ctx, state| state.sharing.clone())
    }

    /// Sets the IP transparent option.
    pub fn set_ip_transparent(&mut self, id: &DatagramApiSocketId<I, C, S>, value: bool) {
        self.core_ctx().with_socket_state_mut(id, |_core_ctx, state| {
            state.options_mut().common.transparent = value;
        })
    }

    /// Returns the IP transparent option.
    pub fn get_ip_transparent(&mut self, id: &DatagramApiSocketId<I, C, S>) -> bool {
        self.core_ctx().with_socket_state(id, |_core_ctx, state| state.options().common.transparent)
    }

    /// Sets the socket mark at `domain`.
    pub fn set_mark(&mut self, id: &DatagramApiSocketId<I, C, S>, domain: MarkDomain, mark: Mark) {
        self.core_ctx().with_socket_state_mut(id, |_core_ctx, state| {
            *state.options_mut().common.marks.get_mut(domain) = mark;
        })
    }

    /// Returns the socket mark at `domain`.
    pub fn get_mark(&mut self, id: &DatagramApiSocketId<I, C, S>, domain: MarkDomain) -> Mark {
        self.core_ctx()
            .with_socket_state(id, |_core_ctx, state| *state.options().common.marks.get(domain))
    }

    /// Sets the broadcast option.
    pub fn set_broadcast(
        &mut self,
        id: &DatagramApiSocketId<I, C, S>,
        value: Option<I::BroadcastMarker>,
    ) {
        self.core_ctx().with_socket_state_mut(id, |_core_ctx, state| {
            state.options_mut().socket_options.allow_broadcast = value;
        })
    }

    /// Returns the broadcast option.
    pub fn get_broadcast(
        &mut self,
        id: &DatagramApiSocketId<I, C, S>,
    ) -> Option<I::BroadcastMarker> {
        self.core_ctx().with_socket_state(id, |_core_ctx, state| {
            state.options().socket_options.allow_broadcast
        })
    }

    /// Sets the multicast interface for outgoing multicast packets.
    pub fn set_multicast_interface(
        &mut self,
        id: &DatagramApiSocketId<I, C, S>,
        value: Option<&DatagramApiDeviceId<C>>,
    ) {
        self.core_ctx().with_socket_state_mut(id, |_core_ctx, state| {
            state.options_mut().socket_options.multicast_interface = value.map(|v| v.downgrade());
        })
    }

    /// Returns the configured multicast interface.
    pub fn get_multicast_interface(
        &mut self,
        id: &DatagramApiSocketId<I, C, S>,
    ) -> Option<DatagramApiWeakDeviceId<C>> {
        self.core_ctx().with_socket_state(id, |_core_ctx, state| {
            state.options().socket_options.multicast_interface.clone()
        })
    }

    /// Sets the multicast loopback flag.
    pub fn set_multicast_loop(&mut self, id: &DatagramApiSocketId<I, C, S>, value: bool) {
        self.core_ctx().with_socket_state_mut(id, |_core_ctx, state| {
            state.options_mut().socket_options.multicast_loop = value;
        })
    }

    /// Returns the multicast loopback flag.
    pub fn get_multicast_loop(&mut self, id: &DatagramApiSocketId<I, C, S>) -> bool {
        self.core_ctx()
            .with_socket_state(id, |_core_ctx, state| state.options().socket_options.multicast_loop)
    }

    /// Sets the Traffic Class option.
    pub fn set_dscp_and_ecn(&mut self, id: &DatagramApiSocketId<I, C, S>, value: DscpAndEcn) {
        self.core_ctx().with_socket_state_mut(id, |_core_ctx, state| {
            state.options_mut().socket_options.dscp_and_ecn = value;
        })
    }

    /// Returns the Traffic Class option.
    pub fn get_dscp_and_ecn(&mut self, id: &DatagramApiSocketId<I, C, S>) -> DscpAndEcn {
        self.core_ctx()
            .with_socket_state(id, |_core_ctx, state| state.options().socket_options.dscp_and_ecn)
    }

    /// Sets the send buffer maximum size to `size`.
    pub fn set_send_buffer(&mut self, id: &DatagramApiSocketId<I, C, S>, size: usize) {
        let settings = self.bindings_ctx().settings();
        id.borrow().send_buffer.set_capacity(size, settings.as_ref())
    }

    /// Returns the current maximum send buffer size.
    pub fn send_buffer(&mut self, id: &DatagramApiSocketId<I, C, S>) -> usize {
        id.borrow().send_buffer.capacity()
    }

    /// Returns the currently available send buffer space on the socket.
    #[cfg(any(test, feature = "testutils"))]
    pub fn send_buffer_available(&mut self, id: &DatagramApiSocketId<I, C, S>) -> usize {
        id.borrow().send_buffer.available()
    }
}

#[cfg(any(test, feature = "testutils"))]
pub(crate) mod testutil {
    use super::*;

    use alloc::vec;
    use net_types::Witness;
    use net_types::ip::IpAddr;
    use netstack3_base::CtxPair;
    use netstack3_base::testutil::{FakeStrongDeviceId, TestIpExt};
    use netstack3_ip::socket::testutil::FakeDeviceConfig;

    /// Helper function to ensure the Fake CoreCtx and BindingsCtx are setup
    /// with [`FakeDeviceConfig`] (one per provided device), with remote/local
    /// IPs that support a connection to the given remote_ip.
    pub fn setup_fake_ctx_with_dualstack_conn_addrs<CC, BC: Default, D: FakeStrongDeviceId>(
        local_ip: IpAddr,
        remote_ip: SpecifiedAddr<IpAddr>,
        devices: impl IntoIterator<Item = D>,
        core_ctx_builder: impl FnOnce(Vec<FakeDeviceConfig<D, SpecifiedAddr<IpAddr>>>) -> CC,
    ) -> CtxPair<CC, BC> {
        // A conversion helper to unmap ipv4-mapped-ipv6 addresses.
        fn unmap_ip(addr: IpAddr) -> IpAddr {
            match addr {
                IpAddr::V4(v4) => IpAddr::V4(v4),
                IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                    Some(v4) => IpAddr::V4(v4),
                    None => IpAddr::V6(v6),
                },
            }
        }

        // Convert the local/remote IPs into `IpAddr` in their non-mapped form.
        let local_ip = unmap_ip(local_ip);
        let remote_ip = unmap_ip(remote_ip.get());
        // If the given local_ip is unspecified, use the default from
        // `TEST_ADDRS`. This ensures we always instantiate the
        // FakeDeviceConfig below with at least one local_ip, which is
        // required for connect operations to succeed.
        let local_ip = SpecifiedAddr::new(local_ip).unwrap_or_else(|| match remote_ip {
            IpAddr::V4(_) => Ipv4::TEST_ADDRS.local_ip.into(),
            IpAddr::V6(_) => Ipv6::TEST_ADDRS.local_ip.into(),
        });
        // If the given remote_ip is unspecified, we won't be able to
        // connect; abort the test.
        let remote_ip = SpecifiedAddr::new(remote_ip).expect("remote-ip should be specified");
        CtxPair::with_core_ctx(core_ctx_builder(
            devices
                .into_iter()
                .map(|device| FakeDeviceConfig {
                    device,
                    local_ips: vec![local_ip],
                    remote_ips: vec![remote_ip],
                })
                .collect(),
        ))
    }
}

#[cfg(test)]
mod test {
    use core::convert::Infallible as Never;

    use alloc::vec;
    use assert_matches::assert_matches;
    use derivative::Derivative;
    use ip_test_macro::ip_test;
    use net_declare::{net_ip_v4, net_ip_v6};
    use net_types::Witness;
    use net_types::ip::{IpVersionMarker, Ipv4Addr, Ipv6Addr};
    use netstack3_base::socket::{
        AddrVec, Bound, IncompatibleError, ListenerAddrInfo, RemoveResult, SocketMapAddrStateSpec,
    };
    use netstack3_base::socketmap::SocketMap;
    use netstack3_base::testutil::{
        FakeDeviceId, FakeReferencyDeviceId, FakeSendToken, FakeSocketWritableListener,
        FakeStrongDeviceId, FakeWeakDeviceId, MultipleDevicesId, TestIpExt,
    };
    use netstack3_base::{ContextProvider, CtxPair, UninstantiableWrapper};
    use netstack3_ip::DEFAULT_HOP_LIMITS;
    use netstack3_ip::device::IpDeviceStateIpExt;
    use netstack3_ip::socket::testutil::{
        FakeDeviceConfig, FakeDualStackIpSocketCtx, FakeIpSocketCtx,
    };
    use netstack3_ip::testutil::DualStackSendIpPacketMeta;
    use packet::{Buf, NestableSerializer as _};
    use packet_formats::ip::{Ipv4Proto, Ipv6Proto};
    use test_case::test_case;

    use super::*;
    use crate::internal::spec_context;

    trait DatagramIpExt<D: FakeStrongDeviceId>:
        IpExt + IpDeviceStateIpExt + TestIpExt + DualStackIpExt + DualStackContextsIpExt<D>
    {
    }
    impl<
        D: FakeStrongDeviceId,
        I: Ip + IpExt + IpDeviceStateIpExt + TestIpExt + DualStackIpExt + DualStackContextsIpExt<D>,
    > DatagramIpExt<D> for I
    {
    }

    #[derive(Debug)]
    enum FakeAddrSpec {}

    impl SocketMapAddrSpec for FakeAddrSpec {
        type LocalIdentifier = NonZeroU16;
        type RemoteIdentifier = u16;
    }

    #[derive(Debug)]
    enum FakeStateSpec {}

    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    struct Tag;

    #[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]

    enum Sharing {
        #[default]
        NoConflicts,
        // Any attempt to insert a connection with the following remote port
        // will conflict.
        ConnectionConflicts {
            remote_port: u16,
        },
    }

    #[derive(Clone, Debug, Derivative)]
    #[derivative(Eq(bound = ""), PartialEq(bound = ""))]
    struct Id<I: IpExt, D: WeakDeviceIdentifier>(StrongRc<I, D, FakeStateSpec>);

    /// Utilities for accessing locked internal state in tests.
    impl<I: IpExt, D: WeakDeviceIdentifier> Id<I, D> {
        fn get(&self) -> impl Deref<Target = SocketState<I, D, FakeStateSpec>> + '_ {
            let Self(rc) = self;
            rc.state.read()
        }

        fn get_mut(&self) -> impl DerefMut<Target = SocketState<I, D, FakeStateSpec>> + '_ {
            let Self(rc) = self;
            rc.state.write()
        }
    }

    impl<I: IpExt, D: WeakDeviceIdentifier> From<StrongRc<I, D, FakeStateSpec>> for Id<I, D> {
        fn from(value: StrongRc<I, D, FakeStateSpec>) -> Self {
            Self(value)
        }
    }

    impl<I: IpExt, D: WeakDeviceIdentifier> Borrow<StrongRc<I, D, FakeStateSpec>> for Id<I, D> {
        fn borrow(&self) -> &StrongRc<I, D, FakeStateSpec> {
            let Self(rc) = self;
            rc
        }
    }

    #[derive(Debug)]
    struct AddrState<T>(T);

    struct FakeSocketMapStateSpec<I, D>(PhantomData<(I, D)>, Never);

    impl<I: IpExt, D: WeakDeviceIdentifier> SocketMapStateSpec for FakeSocketMapStateSpec<I, D> {
        type AddrVecTag = Tag;
        type ConnAddrState = AddrState<Self::ConnId>;
        type ConnId = I::DualStackBoundSocketId<D, FakeStateSpec>;
        type ConnSharingState = Sharing;
        type ListenerAddrState = AddrState<Self::ListenerId>;
        type ListenerId = I::DualStackBoundSocketId<D, FakeStateSpec>;
        type ListenerSharingState = Sharing;
        fn listener_tag(_: ListenerAddrInfo, _state: &Self::ListenerAddrState) -> Self::AddrVecTag {
            Tag
        }
        fn connected_tag(_has_device: bool, _state: &Self::ConnAddrState) -> Self::AddrVecTag {
            Tag
        }
    }

    const FAKE_DATAGRAM_IPV4_PROTOCOL: Ipv4Proto = Ipv4Proto::Other(253);
    const FAKE_DATAGRAM_IPV6_PROTOCOL: Ipv6Proto = Ipv6Proto::Other(254);

    impl DatagramSocketSpec for FakeStateSpec {
        const NAME: &'static str = "FAKE";
        type AddrSpec = FakeAddrSpec;
        type SocketId<I: IpExt, D: WeakDeviceIdentifier> = Id<I, D>;
        // NB: We don't have use for real weak IDs here since we only need to be
        // able to make it upgrade.
        type WeakSocketId<I: IpExt, D: WeakDeviceIdentifier> = Id<I, D>;
        type OtherStackIpOptions<I: IpExt, D: WeakDeviceIdentifier> =
            DatagramIpSpecificSocketOptions<I::OtherVersion, D>;
        type SocketMapSpec<I: IpExt, D: WeakDeviceIdentifier> = FakeSocketMapStateSpec<I, D>;
        type SharingState = Sharing;
        type ListenerIpAddr<I: IpExt> =
            I::DualStackListenerIpAddr<<FakeAddrSpec as SocketMapAddrSpec>::LocalIdentifier>;
        type ConnIpAddr<I: IpExt> = I::DualStackConnIpAddr<Self>;
        type ConnStateExtra = ();
        type ConnState<I: IpExt, D: WeakDeviceIdentifier> = I::DualStackConnState<D, Self>;
        type Counters<I: Ip> = ();
        type ExternalData<I: Ip> = ();
        type SocketWritableListener = FakeSocketWritableListener;
        type SendToken = FakeSendToken;
        type Settings = DatagramSettings;

        fn ip_proto<I: IpProtoExt>() -> I::Proto {
            I::map_ip((), |()| FAKE_DATAGRAM_IPV4_PROTOCOL, |()| FAKE_DATAGRAM_IPV6_PROTOCOL)
        }

        fn make_bound_socket_map_id<I: IpExt, D: WeakDeviceIdentifier>(
            s: &Self::SocketId<I, D>,
        ) -> <Self::SocketMapSpec<I, D> as DatagramSocketMapSpec<I, D, Self::AddrSpec>>::BoundSocketId
        {
            I::into_dual_stack_bound_socket_id(s.clone())
        }

        type Serializer<I: IpExt, B: BufferMut> = packet::Nested<B, ()>;
        type SerializeError = Never;
        const FIXED_HEADER_SIZE: usize = 0;
        fn make_packet<I: IpExt, B: BufferMut>(
            body: B,
            _addr: &ConnIpAddr<
                I::Addr,
                <FakeAddrSpec as SocketMapAddrSpec>::LocalIdentifier,
                <FakeAddrSpec as SocketMapAddrSpec>::RemoteIdentifier,
            >,
        ) -> Result<Self::Serializer<I, B>, Never> {
            Ok(body.wrap_in(()))
        }
        fn try_alloc_listen_identifier<I: Ip, D: WeakDeviceIdentifier>(
            _bindings_ctx: &mut impl RngContext,
            is_available: impl Fn(
                <FakeAddrSpec as SocketMapAddrSpec>::LocalIdentifier,
            ) -> Result<(), InUseError>,
        ) -> Option<<FakeAddrSpec as SocketMapAddrSpec>::LocalIdentifier> {
            (1..=u16::MAX).map(|i| NonZeroU16::new(i).unwrap()).find(|i| is_available(*i).is_ok())
        }

        fn conn_info_from_state<I: IpExt, D: WeakDeviceIdentifier>(
            state: &Self::ConnState<I, D>,
        ) -> ConnInfo<I::Addr, D> {
            let ConnAddr { ip, device } = I::conn_addr_from_state(state);
            let ConnInfoAddr { local: (local_ip, local_port), remote: (remote_ip, remote_port) } =
                ip.into();
            ConnInfo::new(local_ip, local_port, remote_ip, remote_port, || {
                device.clone().expect("device must be bound for addresses that require zones")
            })
        }

        fn try_alloc_local_id<I: IpExt, D: WeakDeviceIdentifier, BC: RngContext>(
            bound: &BoundSocketMap<I, D, FakeAddrSpec, FakeSocketMapStateSpec<I, D>>,
            _bindings_ctx: &mut BC,
            _flow: DatagramFlowId<I::Addr, <FakeAddrSpec as SocketMapAddrSpec>::RemoteIdentifier>,
        ) -> Option<<FakeAddrSpec as SocketMapAddrSpec>::LocalIdentifier> {
            (1..u16::MAX).find_map(|identifier| {
                let identifier = NonZeroU16::new(identifier).unwrap();
                bound
                    .listeners()
                    .could_insert(
                        &ListenerAddr {
                            device: None,
                            ip: ListenerIpAddr { addr: None, identifier },
                        },
                        &Default::default(),
                    )
                    .is_ok()
                    .then_some(identifier)
            })
        }

        fn upgrade_socket_id<I: IpExt, D: WeakDeviceIdentifier>(
            id: &Self::WeakSocketId<I, D>,
        ) -> Option<Self::SocketId<I, D>> {
            Some(id.clone())
        }

        fn downgrade_socket_id<I: IpExt, D: WeakDeviceIdentifier>(
            id: &Self::SocketId<I, D>,
        ) -> Self::WeakSocketId<I, D> {
            id.clone()
        }
    }

    impl<I: IpExt, D: WeakDeviceIdentifier> DatagramSocketMapSpec<I, D, FakeAddrSpec>
        for FakeSocketMapStateSpec<I, D>
    {
        type BoundSocketId = I::DualStackBoundSocketId<D, FakeStateSpec>;
    }

    impl<I: IpExt, D: WeakDeviceIdentifier>
        SocketMapConflictPolicy<
            ConnAddr<
                ConnIpAddr<
                    I::Addr,
                    <FakeAddrSpec as SocketMapAddrSpec>::LocalIdentifier,
                    <FakeAddrSpec as SocketMapAddrSpec>::RemoteIdentifier,
                >,
                D,
            >,
            Sharing,
            I,
            D,
            FakeAddrSpec,
        > for FakeSocketMapStateSpec<I, D>
    {
        fn check_insert_conflicts(
            sharing: &Sharing,
            addr: &ConnAddr<
                ConnIpAddr<
                    I::Addr,
                    <FakeAddrSpec as SocketMapAddrSpec>::LocalIdentifier,
                    <FakeAddrSpec as SocketMapAddrSpec>::RemoteIdentifier,
                >,
                D,
            >,
            _socketmap: &SocketMap<AddrVec<I, D, FakeAddrSpec>, Bound<Self>>,
        ) -> Result<(), InsertError> {
            let ConnAddr { ip: ConnIpAddr { local: _, remote: (_remote_ip, port) }, device: _ } =
                addr;
            match sharing {
                Sharing::NoConflicts => Ok(()),
                Sharing::ConnectionConflicts { remote_port } => {
                    if remote_port == port {
                        Err(InsertError::Exists)
                    } else {
                        Ok(())
                    }
                }
            }
        }
    }

    impl<I: IpExt, D: WeakDeviceIdentifier>
        SocketMapConflictPolicy<
            ListenerAddr<
                ListenerIpAddr<I::Addr, <FakeAddrSpec as SocketMapAddrSpec>::LocalIdentifier>,
                D,
            >,
            Sharing,
            I,
            D,
            FakeAddrSpec,
        > for FakeSocketMapStateSpec<I, D>
    {
        fn check_insert_conflicts(
            sharing: &Sharing,
            _addr: &ListenerAddr<
                ListenerIpAddr<I::Addr, <FakeAddrSpec as SocketMapAddrSpec>::LocalIdentifier>,
                D,
            >,
            _socketmap: &SocketMap<AddrVec<I, D, FakeAddrSpec>, Bound<Self>>,
        ) -> Result<(), InsertError> {
            match sharing {
                Sharing::NoConflicts => Ok(()),
                // Since this implementation is strictly for ListenerAddr,
                // ignore connection conflicts.
                Sharing::ConnectionConflicts { remote_port: _ } => Ok(()),
            }
        }
    }

    impl<T: Eq> SocketMapAddrStateSpec for AddrState<T> {
        type Id = T;
        type SharingState = Sharing;
        type Inserter<'a>
            = Never
        where
            Self: 'a;

        fn new(_sharing: &Self::SharingState, id: Self::Id) -> Self {
            AddrState(id)
        }
        fn contains_id(&self, id: &Self::Id) -> bool {
            let Self(inner) = self;
            inner == id
        }
        fn try_get_inserter<'a, 'b>(
            &'b mut self,
            _new_sharing_state: &'a Self::SharingState,
        ) -> Result<Self::Inserter<'b>, IncompatibleError> {
            Err(IncompatibleError)
        }
        fn could_insert(
            &self,
            _new_sharing_state: &Self::SharingState,
        ) -> Result<(), IncompatibleError> {
            Err(IncompatibleError)
        }
        fn remove_by_id(&mut self, _id: Self::Id) -> RemoveResult {
            RemoveResult::IsLast
        }
        fn sharing_state(&self) -> Self::SharingState {
            Sharing::NoConflicts
        }
    }

    #[derive(Derivative, GenericOverIp)]
    #[derivative(Default(bound = ""))]
    #[generic_over_ip()]
    struct FakeBoundSockets<D: FakeStrongDeviceId> {
        v4: BoundDatagramSocketMap<Ipv4, FakeWeakDeviceId<D>, FakeStateSpec>,
        v6: BoundDatagramSocketMap<Ipv6, FakeWeakDeviceId<D>, FakeStateSpec>,
    }

    impl<D: FakeStrongDeviceId, I: IpExt>
        AsRef<
            BoundSocketMap<
                I,
                FakeWeakDeviceId<D>,
                FakeAddrSpec,
                FakeSocketMapStateSpec<I, FakeWeakDeviceId<D>>,
            >,
        > for FakeBoundSockets<D>
    {
        fn as_ref(
            &self,
        ) -> &BoundSocketMap<
            I,
            FakeWeakDeviceId<D>,
            FakeAddrSpec,
            FakeSocketMapStateSpec<I, FakeWeakDeviceId<D>>,
        > {
            #[derive(GenericOverIp)]
            #[generic_over_ip(I, Ip)]
            struct Wrap<'a, I: IpExt, D: FakeStrongDeviceId>(
                &'a BoundDatagramSocketMap<I, FakeWeakDeviceId<D>, FakeStateSpec>,
            );
            let Wrap(state) = I::map_ip(self, |state| Wrap(&state.v4), |state| Wrap(&state.v6));
            state
        }
    }

    impl<D: FakeStrongDeviceId, I: IpExt>
        AsMut<BoundDatagramSocketMap<I, FakeWeakDeviceId<D>, FakeStateSpec>>
        for FakeBoundSockets<D>
    {
        fn as_mut(&mut self) -> &mut BoundDatagramSocketMap<I, FakeWeakDeviceId<D>, FakeStateSpec> {
            #[derive(GenericOverIp)]
            #[generic_over_ip(I, Ip)]
            struct Wrap<'a, I: IpExt, D: FakeStrongDeviceId>(
                &'a mut BoundDatagramSocketMap<I, FakeWeakDeviceId<D>, FakeStateSpec>,
            );
            let Wrap(state) =
                I::map_ip(self, |state| Wrap(&mut state.v4), |state| Wrap(&mut state.v6));
            state
        }
    }

    type FakeBindingsCtx = netstack3_base::testutil::FakeBindingsCtx<(), (), (), ()>;
    type FakeCtx<I, D> = CtxPair<FakeCoreCtx<I, D>, FakeBindingsCtx>;

    type FakeSocketSet<I, D> = DatagramSocketSet<I, FakeWeakDeviceId<D>, FakeStateSpec>;

    type InnerIpSocketCtx<D> = netstack3_base::testutil::FakeCoreCtx<
        FakeDualStackIpSocketCtx<D>,
        DualStackSendIpPacketMeta<D>,
        D,
    >;

    /// A trait providing a shortcut to instantiate a [`DatagramApi`] from a context.
    trait DatagramApiExt: ContextPair + Sized {
        fn datagram_api<I: Ip>(&mut self) -> DatagramApi<I, &mut Self, FakeStateSpec> {
            DatagramApi::new(self)
        }
    }

    impl<O> DatagramApiExt for O where O: ContextPair + Sized {}

    struct FakeDualStackCoreCtx<D: FakeStrongDeviceId> {
        bound_sockets: FakeBoundSockets<D>,
        ip_socket_ctx: InnerIpSocketCtx<D>,
    }

    struct FakeCoreCtx<I: IpExt, D: FakeStrongDeviceId> {
        dual_stack: FakeDualStackCoreCtx<D>,
        // NB: socket set is last in the struct so all the strong refs are
        // dropped before the primary refs contained herein.
        socket_set: FakeSocketSet<I, D>,
    }

    impl<I: IpExt, D: FakeStrongDeviceId> ContextProvider for FakeCoreCtx<I, D> {
        type Context = Self;
        fn context(&mut self) -> &mut Self::Context {
            self
        }
    }

    impl<I: IpExt, D: FakeStrongDeviceId> FakeCoreCtx<I, D> {
        fn new() -> Self {
            Self::new_with_sockets(Default::default(), Default::default())
        }

        fn new_with_sockets(
            socket_set: FakeSocketSet<I, D>,
            bound_sockets: FakeBoundSockets<D>,
        ) -> Self {
            Self {
                socket_set,
                dual_stack: FakeDualStackCoreCtx {
                    bound_sockets,
                    ip_socket_ctx: Default::default(),
                },
            }
        }

        fn new_with_ip_socket_ctx(ip_socket_ctx: FakeDualStackIpSocketCtx<D>) -> Self {
            Self {
                socket_set: Default::default(),
                dual_stack: FakeDualStackCoreCtx {
                    bound_sockets: Default::default(),
                    ip_socket_ctx: InnerIpSocketCtx::with_state(ip_socket_ctx),
                },
            }
        }
    }

    impl<I: IpExt, D: FakeStrongDeviceId> DeviceIdContext<AnyDevice> for FakeCoreCtx<I, D> {
        type DeviceId = D;
        type WeakDeviceId = FakeWeakDeviceId<D>;
    }

    impl<D: FakeStrongDeviceId> DeviceIdContext<AnyDevice> for FakeDualStackCoreCtx<D> {
        type DeviceId = D;
        type WeakDeviceId = FakeWeakDeviceId<D>;
    }

    impl<D: FakeStrongDeviceId, I: DatagramIpExt<D>>
        spec_context::DatagramSpecStateContext<I, FakeCoreCtx<I, D>, FakeBindingsCtx>
        for FakeStateSpec
    {
        type SocketsStateCtx<'a> = FakeDualStackCoreCtx<D>;

        fn with_all_sockets_mut<
            O,
            F: FnOnce(&mut DatagramSocketSet<I, FakeWeakDeviceId<D>, FakeStateSpec>) -> O,
        >(
            core_ctx: &mut FakeCoreCtx<I, D>,
            cb: F,
        ) -> O {
            cb(&mut core_ctx.socket_set)
        }

        fn with_all_sockets<
            O,
            F: FnOnce(&DatagramSocketSet<I, FakeWeakDeviceId<D>, FakeStateSpec>) -> O,
        >(
            core_ctx: &mut FakeCoreCtx<I, D>,
            cb: F,
        ) -> O {
            cb(&core_ctx.socket_set)
        }

        fn with_socket_state<
            O,
            F: FnOnce(
                &mut Self::SocketsStateCtx<'_>,
                &SocketState<I, FakeWeakDeviceId<D>, FakeStateSpec>,
            ) -> O,
        >(
            core_ctx: &mut FakeCoreCtx<I, D>,
            id: &Id<I, FakeWeakDeviceId<D>>,
            cb: F,
        ) -> O {
            cb(&mut core_ctx.dual_stack, &id.get())
        }

        fn with_socket_state_mut<
            O,
            F: FnOnce(
                &mut Self::SocketsStateCtx<'_>,
                &mut SocketState<I, FakeWeakDeviceId<D>, FakeStateSpec>,
            ) -> O,
        >(
            core_ctx: &mut FakeCoreCtx<I, D>,
            id: &Id<I, FakeWeakDeviceId<D>>,
            cb: F,
        ) -> O {
            cb(&mut core_ctx.dual_stack, &mut id.get_mut())
        }

        fn for_each_socket<
            F: FnMut(
                &mut Self::SocketsStateCtx<'_>,
                &Id<I, FakeWeakDeviceId<D>>,
                &SocketState<I, FakeWeakDeviceId<D>, FakeStateSpec>,
            ),
        >(
            core_ctx: &mut FakeCoreCtx<I, D>,
            mut cb: F,
        ) {
            core_ctx.socket_set.keys().for_each(|id| {
                let id = Id::from(id.clone());
                cb(&mut core_ctx.dual_stack, &id, &id.get());
            })
        }
    }

    /// A test-only IpExt trait to specialize the `DualStackContext` and
    /// `NonDualStackContext` associated types on the
    /// `DatagramBoundStateContext`.
    ///
    /// This allows us to implement `DatagramBoundStateContext` for all `I`
    /// while also assigning its associated types different values for `Ipv4`
    /// and `Ipv6`.
    trait DualStackContextsIpExt<D: FakeStrongDeviceId>: IpExt {
        type DualStackContext: DualStackDatagramBoundStateContext<
                Self,
                FakeBindingsCtx,
                FakeStateSpec,
                DeviceId = D,
                WeakDeviceId = FakeWeakDeviceId<D>,
            >;
        type NonDualStackContext: NonDualStackDatagramBoundStateContext<
                Self,
                FakeBindingsCtx,
                FakeStateSpec,
                DeviceId = D,
                WeakDeviceId = FakeWeakDeviceId<D>,
            >;

        fn dual_stack_context(
            core_ctx: &FakeDualStackCoreCtx<D>,
        ) -> MaybeDualStack<&Self::DualStackContext, &Self::NonDualStackContext>;

        fn dual_stack_context_mut(
            core_ctx: &mut FakeDualStackCoreCtx<D>,
        ) -> MaybeDualStack<&mut Self::DualStackContext, &mut Self::NonDualStackContext>;
    }

    impl<D: FakeStrongDeviceId> DualStackContextsIpExt<D> for Ipv4 {
        type DualStackContext = UninstantiableWrapper<FakeDualStackCoreCtx<D>>;
        type NonDualStackContext = FakeDualStackCoreCtx<D>;

        fn dual_stack_context(
            core_ctx: &FakeDualStackCoreCtx<D>,
        ) -> MaybeDualStack<&Self::DualStackContext, &Self::NonDualStackContext> {
            MaybeDualStack::NotDualStack(core_ctx)
        }

        fn dual_stack_context_mut(
            core_ctx: &mut FakeDualStackCoreCtx<D>,
        ) -> MaybeDualStack<&mut Self::DualStackContext, &mut Self::NonDualStackContext> {
            MaybeDualStack::NotDualStack(core_ctx)
        }
    }

    impl<D: FakeStrongDeviceId> DualStackContextsIpExt<D> for Ipv6 {
        type DualStackContext = FakeDualStackCoreCtx<D>;
        type NonDualStackContext = UninstantiableWrapper<FakeDualStackCoreCtx<D>>;

        fn dual_stack_context(
            core_ctx: &FakeDualStackCoreCtx<D>,
        ) -> MaybeDualStack<&Self::DualStackContext, &Self::NonDualStackContext> {
            MaybeDualStack::DualStack(core_ctx)
        }

        fn dual_stack_context_mut(
            core_ctx: &mut FakeDualStackCoreCtx<D>,
        ) -> MaybeDualStack<&mut Self::DualStackContext, &mut Self::NonDualStackContext> {
            MaybeDualStack::DualStack(core_ctx)
        }
    }

    impl<D: FakeStrongDeviceId, I: DualStackContextsIpExt<D>>
        spec_context::DatagramSpecBoundStateContext<I, FakeDualStackCoreCtx<D>, FakeBindingsCtx>
        for FakeStateSpec
    {
        type IpSocketsCtx<'a> = InnerIpSocketCtx<D>;
        type DualStackContext = I::DualStackContext;
        type NonDualStackContext = I::NonDualStackContext;

        fn with_bound_sockets<O, F>(core_ctx: &mut FakeDualStackCoreCtx<D>, cb: F) -> O
        where
            F: FnOnce(
                &mut Self::IpSocketsCtx<'_>,
                &BoundDatagramSocketMap<I, FakeWeakDeviceId<D>, FakeStateSpec>,
            ) -> O,
        {
            let FakeDualStackCoreCtx { bound_sockets, ip_socket_ctx } = core_ctx;
            cb(ip_socket_ctx, bound_sockets.as_ref())
        }
        fn with_bound_sockets_mut<O, F>(core_ctx: &mut FakeDualStackCoreCtx<D>, cb: F) -> O
        where
            F: FnOnce(
                &mut Self::IpSocketsCtx<'_>,
                &mut BoundDatagramSocketMap<I, FakeWeakDeviceId<D>, FakeStateSpec>,
            ) -> O,
        {
            let FakeDualStackCoreCtx { bound_sockets, ip_socket_ctx } = core_ctx;
            cb(ip_socket_ctx, bound_sockets.as_mut())
        }

        fn with_transport_context<O, F>(core_ctx: &mut FakeDualStackCoreCtx<D>, cb: F) -> O
        where
            F: FnOnce(&mut Self::IpSocketsCtx<'_>) -> O,
        {
            cb(&mut core_ctx.ip_socket_ctx)
        }

        fn dual_stack_context(
            core_ctx: &FakeDualStackCoreCtx<D>,
        ) -> MaybeDualStack<&Self::DualStackContext, &Self::NonDualStackContext> {
            I::dual_stack_context(core_ctx)
        }

        fn dual_stack_context_mut(
            core_ctx: &mut FakeDualStackCoreCtx<D>,
        ) -> MaybeDualStack<&mut Self::DualStackContext, &mut Self::NonDualStackContext> {
            I::dual_stack_context_mut(core_ctx)
        }
    }

    impl<D: FakeStrongDeviceId>
        spec_context::NonDualStackDatagramSpecBoundStateContext<
            Ipv4,
            FakeDualStackCoreCtx<D>,
            FakeBindingsCtx,
        > for FakeStateSpec
    {
        fn nds_converter(
            _core_ctx: &FakeDualStackCoreCtx<D>,
        ) -> impl NonDualStackConverter<Ipv4, FakeWeakDeviceId<D>, Self> {
            ()
        }
    }

    impl<D: FakeStrongDeviceId>
        spec_context::DualStackDatagramSpecBoundStateContext<
            Ipv6,
            FakeDualStackCoreCtx<D>,
            FakeBindingsCtx,
        > for FakeStateSpec
    {
        type IpSocketsCtx<'a> = InnerIpSocketCtx<D>;
        fn dual_stack_enabled(
            _core_ctx: &FakeDualStackCoreCtx<D>,
            _ip_options: &IpOptions<Ipv6, FakeWeakDeviceId<D>, FakeStateSpec>,
        ) -> bool {
            // For now, it's simplest to have dual-stack unconditionally enabled
            // for datagram tests. However, in the future this could be stateful
            // and follow an implementation similar to UDP's test fixture.
            true
        }

        fn to_other_socket_options<'a>(
            _core_ctx: &FakeDualStackCoreCtx<D>,
            state: &'a IpOptions<Ipv6, FakeWeakDeviceId<D>, FakeStateSpec>,
        ) -> &'a DatagramIpSpecificSocketOptions<Ipv4, FakeWeakDeviceId<D>> {
            let IpOptions { other_stack, .. } = state;
            other_stack
        }

        fn ds_converter(
            _core_ctx: &FakeDualStackCoreCtx<D>,
        ) -> impl DualStackConverter<Ipv6, FakeWeakDeviceId<D>, Self> {
            ()
        }

        fn to_other_bound_socket_id(
            _core_ctx: &FakeDualStackCoreCtx<D>,
            id: &Id<Ipv6, D::Weak>,
        ) -> EitherIpSocket<D::Weak, FakeStateSpec> {
            EitherIpSocket::V6(id.clone())
        }

        fn with_both_bound_sockets_mut<
            O,
            F: FnOnce(
                &mut Self::IpSocketsCtx<'_>,
                &mut BoundSocketsFromSpec<Ipv6, FakeDualStackCoreCtx<D>, FakeStateSpec>,
                &mut BoundSocketsFromSpec<Ipv4, FakeDualStackCoreCtx<D>, FakeStateSpec>,
            ) -> O,
        >(
            core_ctx: &mut FakeDualStackCoreCtx<D>,
            cb: F,
        ) -> O {
            let FakeDualStackCoreCtx { bound_sockets: FakeBoundSockets { v4, v6 }, ip_socket_ctx } =
                core_ctx;
            cb(ip_socket_ctx, v6, v4)
        }

        fn with_other_bound_sockets_mut<
            O,
            F: FnOnce(
                &mut Self::IpSocketsCtx<'_>,
                &mut BoundSocketsFromSpec<Ipv4, FakeDualStackCoreCtx<D>, FakeStateSpec>,
            ) -> O,
        >(
            core_ctx: &mut FakeDualStackCoreCtx<D>,
            cb: F,
        ) -> O {
            let FakeDualStackCoreCtx { bound_sockets, ip_socket_ctx } = core_ctx;
            cb(ip_socket_ctx, bound_sockets.as_mut())
        }

        fn with_transport_context<O, F: FnOnce(&mut Self::IpSocketsCtx<'_>) -> O>(
            core_ctx: &mut FakeDualStackCoreCtx<D>,
            cb: F,
        ) -> O {
            cb(&mut core_ctx.ip_socket_ctx)
        }
    }

    #[ip_test(I)]
    fn set_get_hop_limits<I: DatagramIpExt<FakeDeviceId>>() {
        let mut ctx = FakeCtx::with_core_ctx(FakeCoreCtx::<I, FakeDeviceId>::new());
        let mut api = ctx.datagram_api::<I>();

        let unbound = api.create_default();
        const EXPECTED_HOP_LIMITS: HopLimits = HopLimits {
            unicast: NonZeroU8::new(45).unwrap(),
            multicast: NonZeroU8::new(23).unwrap(),
        };

        api.update_ip_hop_limit(&unbound, |limits| {
            *limits = SocketHopLimits {
                unicast: Some(EXPECTED_HOP_LIMITS.unicast),
                multicast: Some(EXPECTED_HOP_LIMITS.multicast),
                version: IpVersionMarker::default(),
            }
        });

        assert_eq!(api.get_ip_hop_limits(&unbound), EXPECTED_HOP_LIMITS);
    }

    #[ip_test(I)]
    fn set_get_device_hop_limits<I: DatagramIpExt<FakeReferencyDeviceId>>() {
        let device = FakeReferencyDeviceId::default();
        let mut ctx = FakeCtx::with_core_ctx(FakeCoreCtx::<I, _>::new_with_ip_socket_ctx(
            FakeDualStackIpSocketCtx::new([FakeDeviceConfig::<_, SpecifiedAddr<I::Addr>> {
                device: device.clone(),
                local_ips: Default::default(),
                remote_ips: Default::default(),
            }]),
        ));
        let mut api = ctx.datagram_api::<I>();

        let unbound = api.create_default();
        api.set_device(&unbound, Some(&device)).unwrap();

        let HopLimits { mut unicast, multicast } = DEFAULT_HOP_LIMITS;
        unicast = unicast.checked_add(1).unwrap();
        {
            let device_state =
                api.core_ctx().dual_stack.ip_socket_ctx.state.get_device_state_mut::<I>(&device);
            assert_ne!(device_state.default_hop_limit, unicast);
            device_state.default_hop_limit = unicast;
        }
        assert_eq!(api.get_ip_hop_limits(&unbound), HopLimits { unicast, multicast });

        // If the device is removed, use default hop limits.
        device.mark_removed();
        assert_eq!(api.get_ip_hop_limits(&unbound), DEFAULT_HOP_LIMITS);
    }

    #[ip_test(I)]
    fn default_hop_limits<I: DatagramIpExt<FakeDeviceId>>() {
        let mut ctx = FakeCtx::with_core_ctx(FakeCoreCtx::<I, FakeDeviceId>::new());
        let mut api = ctx.datagram_api::<I>();
        let unbound = api.create_default();
        assert_eq!(api.get_ip_hop_limits(&unbound), DEFAULT_HOP_LIMITS);

        api.update_ip_hop_limit(&unbound, |limits| {
            *limits = SocketHopLimits {
                unicast: Some(NonZeroU8::new(1).unwrap()),
                multicast: Some(NonZeroU8::new(1).unwrap()),
                version: IpVersionMarker::default(),
            }
        });

        // The limits no longer match the default.
        assert_ne!(api.get_ip_hop_limits(&unbound), DEFAULT_HOP_LIMITS);

        // Clear the hop limits set on the socket.
        api.update_ip_hop_limit(&unbound, |limits| *limits = Default::default());

        // The values should be back at the defaults.
        assert_eq!(api.get_ip_hop_limits(&unbound), DEFAULT_HOP_LIMITS);
    }

    #[ip_test(I)]
    fn bind_device_unbound<I: DatagramIpExt<FakeDeviceId>>() {
        let mut ctx = FakeCtx::with_core_ctx(FakeCoreCtx::<I, FakeDeviceId>::new());
        let mut api = ctx.datagram_api::<I>();
        let unbound = api.create_default();

        api.set_device(&unbound, Some(&FakeDeviceId)).unwrap();
        assert_eq!(api.get_bound_device(&unbound), Some(FakeWeakDeviceId(FakeDeviceId)));

        api.set_device(&unbound, None).unwrap();
        assert_eq!(api.get_bound_device(&unbound), None);
    }

    #[ip_test(I)]
    fn send_to_binds_unbound<I: DatagramIpExt<FakeDeviceId>>() {
        let mut ctx =
            FakeCtx::with_core_ctx(FakeCoreCtx::<I, FakeDeviceId>::new_with_ip_socket_ctx(
                FakeDualStackIpSocketCtx::new([FakeDeviceConfig {
                    device: FakeDeviceId,
                    local_ips: vec![I::TEST_ADDRS.local_ip],
                    remote_ips: vec![I::TEST_ADDRS.remote_ip],
                }]),
            ));
        let mut api = ctx.datagram_api::<I>();
        let socket = api.create_default();
        let body = Buf::new(Vec::new(), ..);

        api.send_to(
            &socket,
            Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)),
            1234,
            body,
            FakeSendToken::default(),
        )
        .expect("succeeds");
        assert_matches!(api.get_info(&socket), SocketInfo::Listener(_));
    }

    #[ip_test(I)]
    fn send_to_no_route_still_binds<I: DatagramIpExt<FakeDeviceId>>() {
        let mut ctx = FakeCtx::with_core_ctx(FakeCoreCtx::<I, _>::new_with_ip_socket_ctx(
            FakeDualStackIpSocketCtx::new([FakeDeviceConfig {
                device: FakeDeviceId,
                local_ips: vec![I::TEST_ADDRS.local_ip],
                remote_ips: vec![],
            }]),
        ));
        let mut api = ctx.datagram_api::<I>();
        let socket = api.create_default();
        let body = Buf::new(Vec::new(), ..);

        assert_matches!(
            api.send_to(
                &socket,
                Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)),
                1234,
                body,
                FakeSendToken::default(),
            ),
            Err(SendToError::CreateAndSend(_))
        );
        assert_matches!(api.get_info(&socket), SocketInfo::Listener(_));
    }

    #[ip_test(I)]
    #[test_case(true; "remove device b")]
    #[test_case(false; "dont remove device b")]
    fn multicast_membership_changes<I: DatagramIpExt<FakeReferencyDeviceId> + TestIpExt>(
        remove_device_b: bool,
    ) {
        let device_a = FakeReferencyDeviceId::default();
        let device_b = FakeReferencyDeviceId::default();
        let mut core_ctx = FakeIpSocketCtx::<I, FakeReferencyDeviceId>::new(
            [device_a.clone(), device_b.clone()].into_iter().map(|device| FakeDeviceConfig {
                device,
                local_ips: Default::default(),
                remote_ips: Default::default(),
            }),
        );
        let mut bindings_ctx = FakeBindingsCtx::default();

        let multicast_addr1 = I::get_multicast_addr(1);
        let mut memberships = MulticastMemberships::default();
        assert_eq!(
            memberships.apply_membership_change(
                multicast_addr1,
                &FakeWeakDeviceId(device_a.clone()),
                true /* want_membership */
            ),
            Some(MulticastMembershipChange::Join),
        );
        core_ctx.join_multicast_group(&mut bindings_ctx, &device_a, multicast_addr1);

        let multicast_addr2 = I::get_multicast_addr(2);
        assert_eq!(
            memberships.apply_membership_change(
                multicast_addr2,
                &FakeWeakDeviceId(device_b.clone()),
                true /* want_membership */
            ),
            Some(MulticastMembershipChange::Join),
        );
        core_ctx.join_multicast_group(&mut bindings_ctx, &device_b, multicast_addr2);

        for (device, addr, expected) in [
            (&device_a, multicast_addr1, true),
            (&device_a, multicast_addr2, false),
            (&device_b, multicast_addr1, false),
            (&device_b, multicast_addr2, true),
        ] {
            assert_eq!(
                core_ctx.get_device_state(device).is_in_multicast_group(&addr),
                expected,
                "device={:?}, addr={}",
                device,
                addr,
            );
        }

        if remove_device_b {
            device_b.mark_removed();
        }

        leave_all_joined_groups(&mut core_ctx, &mut bindings_ctx, &memberships);
        for (device, addr, expected) in [
            (&device_a, multicast_addr1, false),
            (&device_a, multicast_addr2, false),
            (&device_b, multicast_addr1, false),
            // Should not attempt to leave the multicast group on the device if
            // the device looks like it was removed. Note that although we mark
            // the device as removed, we do not destroy its state so we can
            // inspect it here.
            (&device_b, multicast_addr2, remove_device_b),
        ] {
            assert_eq!(
                core_ctx.get_device_state(device).is_in_multicast_group(&addr),
                expected,
                "device={:?}, addr={}",
                device,
                addr,
            );
        }
    }

    #[ip_test(I)]
    fn set_get_transparent<I: DatagramIpExt<FakeDeviceId>>() {
        let mut ctx = FakeCtx::with_core_ctx(FakeCoreCtx::<I, _>::new_with_ip_socket_ctx(
            FakeDualStackIpSocketCtx::new([FakeDeviceConfig::<_, SpecifiedAddr<I::Addr>> {
                device: FakeDeviceId,
                local_ips: Default::default(),
                remote_ips: Default::default(),
            }]),
        ));
        let mut api = ctx.datagram_api::<I>();
        let unbound = api.create_default();

        assert!(!api.get_ip_transparent(&unbound));

        api.set_ip_transparent(&unbound, true);

        assert!(api.get_ip_transparent(&unbound));

        api.set_ip_transparent(&unbound, false);

        assert!(!api.get_ip_transparent(&unbound));
    }

    #[ip_test(I)]
    fn transparent_bind_connect_non_local_src_addr<I: DatagramIpExt<FakeDeviceId>>() {
        let mut ctx = FakeCtx::with_core_ctx(FakeCoreCtx::<I, _>::new_with_ip_socket_ctx(
            FakeDualStackIpSocketCtx::new([FakeDeviceConfig {
                device: FakeDeviceId,
                local_ips: vec![],
                remote_ips: vec![I::TEST_ADDRS.remote_ip],
            }]),
        ));
        let mut api = ctx.datagram_api::<I>();
        let socket = api.create_default();
        api.set_ip_transparent(&socket, true);

        const LOCAL_PORT: NonZeroU16 = NonZeroU16::new(10).unwrap();
        const REMOTE_PORT: u16 = 1234;

        // Binding to `local_ip` should succeed even though it is not assigned
        // to an interface because the socket is transparent.
        api.listen(&socket, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), Some(LOCAL_PORT))
            .expect("listen should succeed");

        // Connecting to a valid remote should also succeed even though the
        // local address of the IP socket is not actually local.
        api.connect(
            &socket,
            Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)),
            REMOTE_PORT,
            Default::default(),
        )
        .expect("connect should succeed");

        api.send_to(
            &socket,
            Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)),
            REMOTE_PORT,
            Buf::new(Vec::new(), ..),
            FakeSendToken::default(),
        )
        .expect("send_to should succeed");
    }

    #[derive(Eq, PartialEq)]
    enum OriginalSocketState {
        Unbound,
        Listener,
        Connected,
    }

    #[ip_test(I)]
    #[test_case(OriginalSocketState::Unbound; "reinsert_unbound")]
    #[test_case(OriginalSocketState::Listener; "reinsert_listener")]
    #[test_case(OriginalSocketState::Connected; "reinsert_connected")]
    fn connect_reinserts_on_failure_single_stack<I: DatagramIpExt<FakeDeviceId>>(
        original: OriginalSocketState,
    ) {
        connect_reinserts_on_failure_inner::<I>(
            original,
            I::TEST_ADDRS.local_ip.get(),
            I::TEST_ADDRS.remote_ip,
        );
    }

    #[test_case(OriginalSocketState::Listener, net_ip_v6!("::FFFF:192.0.2.1"),
        net_ip_v4!("192.0.2.2"); "reinsert_listener_other_stack")]
    #[test_case(OriginalSocketState::Listener, net_ip_v6!("::"),
        net_ip_v4!("192.0.2.2"); "reinsert_listener_both_stacks")]
    #[test_case(OriginalSocketState::Connected, net_ip_v6!("::FFFF:192.0.2.1"),
        net_ip_v4!("192.0.2.2"); "reinsert_connected_other_stack")]
    fn connect_reinserts_on_failure_dual_stack(
        original: OriginalSocketState,
        local_ip: Ipv6Addr,
        remote_ip: Ipv4Addr,
    ) {
        let remote_ip = remote_ip.to_ipv6_mapped();
        connect_reinserts_on_failure_inner::<Ipv6>(original, local_ip, remote_ip);
    }

    fn connect_reinserts_on_failure_inner<I: DatagramIpExt<FakeDeviceId>>(
        original: OriginalSocketState,
        local_ip: I::Addr,
        remote_ip: SpecifiedAddr<I::Addr>,
    ) {
        let mut ctx = testutil::setup_fake_ctx_with_dualstack_conn_addrs::<_, FakeBindingsCtx, _>(
            local_ip.to_ip_addr(),
            remote_ip.into(),
            [FakeDeviceId {}],
            |device_configs| {
                FakeCoreCtx::<I, _>::new_with_ip_socket_ctx(FakeDualStackIpSocketCtx::new(
                    device_configs,
                ))
            },
        );
        let mut api = ctx.datagram_api::<I>();
        let socket = api.create_default();
        const LOCAL_PORT: NonZeroU16 = NonZeroU16::new(10).unwrap();
        const ORIGINAL_REMOTE_PORT: u16 = 1234;
        const NEW_REMOTE_PORT: u16 = 5678;

        // Setup the original socket state.
        match original {
            OriginalSocketState::Unbound => {}
            OriginalSocketState::Listener => api
                .listen(
                    &socket,
                    SpecifiedAddr::new(local_ip).map(ZonedAddr::Unzoned),
                    Some(LOCAL_PORT),
                )
                .expect("listen should succeed"),
            OriginalSocketState::Connected => api
                .connect(
                    &socket,
                    Some(ZonedAddr::Unzoned(remote_ip)),
                    ORIGINAL_REMOTE_PORT,
                    Default::default(),
                )
                .expect("connect should succeed"),
        }

        // Update the sharing state to generate conflicts during the call to `connect`.
        api.core_ctx().with_socket_state_mut(
            &socket,
            |_core_ctx, state: &mut SocketState<I, _, FakeStateSpec>| {
                state.sharing = Sharing::ConnectionConflicts { remote_port: NEW_REMOTE_PORT };
            },
        );

        // Try to connect and observe a conflict error.
        assert_matches!(
            api.connect(
                &socket,
                Some(ZonedAddr::Unzoned(remote_ip)),
                NEW_REMOTE_PORT,
                Default::default(),
            ),
            Err(ConnectError::SockAddrConflict)
        );

        // Verify the original socket state is intact.
        let info = api.get_info(&socket);
        match original {
            OriginalSocketState::Unbound => assert_matches!(info, SocketInfo::Unbound),
            OriginalSocketState::Listener => {
                let local_port = assert_matches!(
                    info,
                    SocketInfo::Listener(ListenerInfo {
                        local_ip: _,
                        local_identifier,
                    }) => local_identifier
                );
                assert_eq!(LOCAL_PORT, local_port);
            }
            OriginalSocketState::Connected => {
                let remote_port = assert_matches!(
                    info,
                    SocketInfo::Connected(ConnInfo {
                        local_ip: _,
                        local_identifier: _,
                        remote_ip: _,
                        remote_identifier,
                    }) => remote_identifier
                );
                assert_eq!(ORIGINAL_REMOTE_PORT, remote_port);
            }
        }
    }

    #[test_case(net_ip_v6!("::a:b:c:d"), ShutdownType::Send; "this_stack_send")]
    #[test_case(net_ip_v6!("::a:b:c:d"), ShutdownType::Receive; "this_stack_receive")]
    #[test_case(net_ip_v6!("::a:b:c:d"), ShutdownType::SendAndReceive; "this_stack_send_and_receive")]
    #[test_case(net_ip_v6!("::FFFF:192.0.2.1"), ShutdownType::Send; "other_stack_send")]
    #[test_case(net_ip_v6!("::FFFF:192.0.2.1"), ShutdownType::Receive; "other_stack_receive")]
    #[test_case(net_ip_v6!("::FFFF:192.0.2.1"), ShutdownType::SendAndReceive; "other_stack_send_and_receive")]
    fn set_get_shutdown_dualstack(remote_ip: Ipv6Addr, shutdown: ShutdownType) {
        let remote_ip = SpecifiedAddr::new(remote_ip).expect("remote_ip should be specified");
        let mut ctx = testutil::setup_fake_ctx_with_dualstack_conn_addrs::<_, FakeBindingsCtx, _>(
            Ipv6::UNSPECIFIED_ADDRESS.into(),
            remote_ip.into(),
            [FakeDeviceId {}],
            |device_configs| {
                FakeCoreCtx::<Ipv6, _>::new_with_ip_socket_ctx(FakeDualStackIpSocketCtx::new(
                    device_configs,
                ))
            },
        );
        let mut api = ctx.datagram_api::<Ipv6>();

        const REMOTE_PORT: u16 = 1234;
        let socket = api.create_default();
        api.connect(&socket, Some(ZonedAddr::Unzoned(remote_ip)), REMOTE_PORT, Default::default())
            .expect("connect should succeed");
        assert_eq!(api.get_shutdown_connected(&socket), None);

        api.shutdown_connected(&socket, shutdown).expect("shutdown should succeed");
        assert_eq!(api.get_shutdown_connected(&socket), Some(shutdown));
    }

    #[ip_test(I)]
    #[test_case(OriginalSocketState::Unbound; "unbound")]
    #[test_case(OriginalSocketState::Listener; "listener")]
    #[test_case(OriginalSocketState::Connected; "connected")]
    fn set_get_device_single_stack<I: DatagramIpExt<MultipleDevicesId>>(
        original: OriginalSocketState,
    ) {
        set_get_device_inner::<I>(original, I::TEST_ADDRS.local_ip.get(), I::TEST_ADDRS.remote_ip);
    }

    #[test_case(OriginalSocketState::Listener, net_ip_v6!("::FFFF:192.0.2.1"),
        net_ip_v4!("192.0.2.2"); "listener_other_stack")]
    #[test_case(OriginalSocketState::Listener, net_ip_v6!("::"),
        net_ip_v4!("192.0.2.2"); "listener_both_stacks")]
    #[test_case(OriginalSocketState::Connected, net_ip_v6!("::FFFF:192.0.2.1"),
        net_ip_v4!("192.0.2.2"); "connected_other_stack")]
    fn set_get_device_dual_stack(
        original: OriginalSocketState,
        local_ip: Ipv6Addr,
        remote_ip: Ipv4Addr,
    ) {
        let remote_ip = remote_ip.to_ipv6_mapped();
        set_get_device_inner::<Ipv6>(original, local_ip, remote_ip);
    }

    fn set_get_device_inner<I: DatagramIpExt<MultipleDevicesId>>(
        original: OriginalSocketState,
        local_ip: I::Addr,
        remote_ip: SpecifiedAddr<I::Addr>,
    ) {
        const DEVICE_ID1: MultipleDevicesId = MultipleDevicesId::A;
        const DEVICE_ID2: MultipleDevicesId = MultipleDevicesId::B;

        let mut ctx = testutil::setup_fake_ctx_with_dualstack_conn_addrs::<_, FakeBindingsCtx, _>(
            local_ip.to_ip_addr(),
            remote_ip.into(),
            [DEVICE_ID1, DEVICE_ID2],
            |device_configs| {
                FakeCoreCtx::<I, _>::new_with_ip_socket_ctx(FakeDualStackIpSocketCtx::new(
                    device_configs,
                ))
            },
        );

        const LOCAL_PORT: NonZeroU16 = NonZeroU16::new(10).unwrap();
        const REMOTE_PORT: u16 = 1234;

        let mut api = ctx.datagram_api::<I>();
        let socket1 = api.create_default();
        let socket2 = api.create_default();

        // Initialize each socket to the `original` state, and verify that their
        // device can be set.
        for (socket, device_id) in [(&socket1, DEVICE_ID1), (&socket2, DEVICE_ID2)] {
            match original {
                OriginalSocketState::Unbound => {}
                OriginalSocketState::Listener => api
                    .listen(
                        &socket,
                        SpecifiedAddr::new(local_ip).map(ZonedAddr::Unzoned),
                        Some(LOCAL_PORT),
                    )
                    .expect("listen should succeed"),
                OriginalSocketState::Connected => api
                    .connect(
                        &socket,
                        Some(ZonedAddr::Unzoned(remote_ip)),
                        REMOTE_PORT,
                        Default::default(),
                    )
                    .expect("connect should succeed"),
            }

            assert_eq!(api.get_bound_device(socket), None);
            api.set_device(socket, Some(&device_id)).expect("set device should succeed");
            assert_eq!(api.get_bound_device(socket), Some(FakeWeakDeviceId(device_id)));
        }

        // For bound sockets, try to bind socket 2 to device 1, and expect it
        // it to conflict with socket 1 (They now have identical address keys in
        // the bound socket map)
        if original != OriginalSocketState::Unbound {
            assert_eq!(
                api.set_device(&socket2, Some(&DEVICE_ID1)),
                Err(SocketError::Local(LocalAddressError::AddressInUse))
            );
            // Verify both sockets still have their original device.
            assert_eq!(api.get_bound_device(&socket1), Some(FakeWeakDeviceId(DEVICE_ID1)));
            assert_eq!(api.get_bound_device(&socket2), Some(FakeWeakDeviceId(DEVICE_ID2)));
        }

        // Verify the device can be unset.
        // NB: Close socket2 first, otherwise socket 1 will conflict with it.
        api.close(socket2, |_: ReferenceState<_, _, _>| ()).into_removed();
        api.set_device(&socket1, None).expect("set device should succeed");
        assert_eq!(api.get_bound_device(&socket1), None,);
    }
}
