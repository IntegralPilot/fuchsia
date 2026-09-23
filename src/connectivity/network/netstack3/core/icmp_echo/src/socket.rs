// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! ICMP Echo Sockets.

use alloc::vec::Vec;
use core::borrow::Borrow;
use core::convert::Infallible as Never;
use core::fmt::Debug;
use core::marker::PhantomData;
use core::num::{NonZeroU8, NonZeroU16};
use core::ops::ControlFlow;
use lock_order::lock::{DelegatedOrderedLockAccess, OrderedLockAccess, OrderedLockRef};

use derivative::Derivative;
use either::Either;
use log::{debug, trace};
use net_types::ip::{GenericOverIp, Ip, IpVersionMarker};
use net_types::{SpecifiedAddr, Witness as _, ZonedAddr};
use netstack3_base::socket::{
    self, AddrIsMappedError, AddrVec, AddrVecIter, ConnAddr, ConnInfoAddr, ConnIpAddr,
    IncompatibleError, InsertError, ListenerAddrInfo, MaybeDualStack, ShutdownType, SocketCookie,
    SocketIpAddr, SocketMapAddrSpec, SocketMapAddrStateSpec, SocketMapConflictPolicy,
    SocketMapStateSpec, SocketWritableListener,
};
use netstack3_base::socketmap::{IterShadows as _, SocketMap};
use netstack3_base::sync::{RwLock, StrongRc};
use netstack3_base::{
    AnyDevice, ContextPair, CoreTxMetadataContext, CounterContext, DeviceIdContext, IcmpIpExt,
    Inspector, InspectorDeviceExt, LocalAddressError, Mark, MarkDomain, Marks, PortAllocImpl,
    ReferenceNotifiers, RemoveResourceResultWithContext, RngContext, SettingsContext, SocketError,
    StrongDeviceIdentifier, UninstantiableWrapper, WeakDeviceIdentifier,
};
use netstack3_datagram::{
    self as datagram, DatagramApi, DatagramBindingsTypes, DatagramFlowId, DatagramSocketMapSpec,
    DatagramSocketSet, DatagramSocketSpec, DatagramSpecBoundStateContext, DatagramSpecStateContext,
    DatagramStateContext, ExpectedUnboundError, NonDualStackConverter,
    NonDualStackDatagramSpecBoundStateContext,
};
use netstack3_ip::icmp::{EchoTransportContextMarker, IcmpRxCounters};
use netstack3_ip::socket::SocketHopLimits;
use netstack3_ip::{
    IpHeaderInfo, IpLayerIpExt, IpTransportContext, LocalDeliveryPacketInfo,
    MulticastMembershipHandler, ReceiveIpPacketMeta, SocketMetadata, TransportIpContext,
};
use packet::{BufferMut, NestablePacketBuilder as _, ParsablePacket as _, ParseBuffer};
use packet_formats::icmp::{IcmpEchoReply, IcmpEchoRequest, IcmpPacketBuilder, IcmpPacketRaw};
use packet_formats::ip::{IpProtoExt, Ipv4Proto, Ipv6Proto};

use crate::internal::settings::IcmpEchoSettings;

/// A marker trait for all IP extensions required by ICMP sockets.
pub trait IpExt: datagram::IpExt + IcmpIpExt + IpLayerIpExt {}
impl<O: datagram::IpExt + IcmpIpExt + IpLayerIpExt> IpExt for O {}

/// Holds the stack's ICMP echo sockets.
#[derive(Derivative, GenericOverIp)]
#[derivative(Default(bound = ""))]
#[generic_over_ip(I, Ip)]
pub struct IcmpSockets<I: IpExt, D: WeakDeviceIdentifier, BT: IcmpEchoBindingsTypes> {
    bound_and_id_allocator: RwLock<BoundSockets<I, D, BT>>,
    // Destroy all_sockets last so the strong references in the demux are
    // dropped before the primary references in the set.
    all_sockets: RwLock<IcmpSocketSet<I, D, BT>>,
}

impl<I: IpExt, D: WeakDeviceIdentifier, BT: IcmpEchoBindingsTypes>
    OrderedLockAccess<BoundSockets<I, D, BT>> for IcmpSockets<I, D, BT>
{
    type Lock = RwLock<BoundSockets<I, D, BT>>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.bound_and_id_allocator)
    }
}

impl<I: IpExt, D: WeakDeviceIdentifier, BT: IcmpEchoBindingsTypes>
    OrderedLockAccess<IcmpSocketSet<I, D, BT>> for IcmpSockets<I, D, BT>
{
    type Lock = RwLock<IcmpSocketSet<I, D, BT>>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.all_sockets)
    }
}

/// An ICMP socket.
#[derive(GenericOverIp, Derivative)]
#[derivative(Eq(bound = ""), PartialEq(bound = ""), Hash(bound = ""))]
#[generic_over_ip(I, Ip)]
pub struct IcmpSocketId<I: IpExt, D: WeakDeviceIdentifier, BT: IcmpEchoBindingsTypes>(
    datagram::StrongRc<I, D, Icmp<BT>>,
);

impl<I: IpExt, D: WeakDeviceIdentifier, BT: IcmpEchoBindingsTypes> IcmpSocketId<I, D, BT> {
    /// Returns a `SocketCookie` for this socket.
    pub fn socket_cookie(&self) -> SocketCookie {
        let Self(inner) = self;
        SocketCookie::new(inner.resource_token())
    }

    /// Returns a `SocketInfo` for this socket.
    pub fn socket_info(&self) -> netstack3_base::socket::SocketInfo {
        netstack3_base::socket::SocketInfo {
            proto: I::map_ip(
                (),
                |()| netstack3_base::socket::EitherIpProto::V4(Ipv4Proto::Icmp),
                |()| netstack3_base::socket::EitherIpProto::V6(Ipv6Proto::Icmpv6),
            ),
            cookie: self.socket_cookie(),
        }
    }
}

impl<CC, I, BT> SocketMetadata<CC> for IcmpSocketId<I, CC::WeakDeviceId, BT>
where
    CC: IcmpEchoStateContext<I, BT>,
    I: IpExt,
    BT: IcmpEchoBindingsTypes,
{
    fn socket_info(&self, _core_ctx: &mut CC) -> netstack3_base::socket::SocketInfo {
        self.socket_info()
    }

    fn marks(&self, core_ctx: &mut CC) -> Marks {
        core_ctx.with_socket_state(self, |_core_ctx, state| state.options().marks().clone())
    }
}

impl<I: IpExt, D: WeakDeviceIdentifier, BT: IcmpEchoBindingsTypes> Clone
    for IcmpSocketId<I, D, BT>
{
    #[cfg_attr(feature = "instrumented", track_caller)]
    fn clone(&self) -> Self {
        let Self(rc) = self;
        Self(StrongRc::clone(rc))
    }
}

impl<I: IpExt, D: WeakDeviceIdentifier, BT: IcmpEchoBindingsTypes>
    From<datagram::StrongRc<I, D, Icmp<BT>>> for IcmpSocketId<I, D, BT>
{
    fn from(value: datagram::StrongRc<I, D, Icmp<BT>>) -> Self {
        Self(value)
    }
}

impl<I: IpExt, D: WeakDeviceIdentifier, BT: IcmpEchoBindingsTypes>
    Borrow<datagram::StrongRc<I, D, Icmp<BT>>> for IcmpSocketId<I, D, BT>
{
    fn borrow(&self) -> &datagram::StrongRc<I, D, Icmp<BT>> {
        let Self(rc) = self;
        rc
    }
}

impl<I: IpExt, D: WeakDeviceIdentifier, BT: IcmpEchoBindingsTypes>
    PartialEq<WeakIcmpSocketId<I, D, BT>> for IcmpSocketId<I, D, BT>
{
    fn eq(&self, other: &WeakIcmpSocketId<I, D, BT>) -> bool {
        let Self(rc) = self;
        let WeakIcmpSocketId(weak) = other;
        StrongRc::weak_ptr_eq(rc, weak)
    }
}

impl<I: IpExt, D: WeakDeviceIdentifier, BT: IcmpEchoBindingsTypes> Debug
    for IcmpSocketId<I, D, BT>
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let Self(rc) = self;
        f.debug_tuple("IcmpSocketId").field(&StrongRc::debug_id(rc)).finish()
    }
}

impl<I: IpExt, D: WeakDeviceIdentifier, BT: IcmpEchoBindingsTypes> IcmpSocketId<I, D, BT> {
    /// Returns the inner state for this socket, to be used in conjunction with
    /// lock ordering mechanisms.
    #[cfg(any(test, feature = "testutils"))]
    pub fn state(&self) -> &RwLock<IcmpSocketState<I, D, BT>> {
        let Self(rc) = self;
        rc.state()
    }

    /// Returns a means to debug outstanding references to this socket.
    pub fn debug_references(&self) -> impl Debug {
        let Self(rc) = self;
        StrongRc::debug_references(rc)
    }

    /// Downgrades this ID to a weak reference.
    pub fn downgrade(&self) -> WeakIcmpSocketId<I, D, BT> {
        let Self(rc) = self;
        WeakIcmpSocketId(StrongRc::downgrade(rc))
    }

    /// Returns external data associated with this socket.
    pub fn external_data(&self) -> &BT::ExternalData<I> {
        let Self(rc) = self;
        rc.external_data()
    }
}

impl<I: IpExt, D: WeakDeviceIdentifier, BT: IcmpEchoBindingsTypes>
    DelegatedOrderedLockAccess<IcmpSocketState<I, D, BT>> for IcmpSocketId<I, D, BT>
{
    type Inner = datagram::ReferenceState<I, D, Icmp<BT>>;
    fn delegate_ordered_lock_access(&self) -> &Self::Inner {
        let Self(rc) = self;
        &*rc
    }
}

/// A weak reference to an ICMP socket.
#[derive(GenericOverIp, Derivative)]
#[derivative(Eq(bound = ""), PartialEq(bound = ""), Hash(bound = ""), Clone(bound = ""))]
#[generic_over_ip(I, Ip)]
pub struct WeakIcmpSocketId<I: IpExt, D: WeakDeviceIdentifier, BT: IcmpEchoBindingsTypes>(
    datagram::WeakRc<I, D, Icmp<BT>>,
);

impl<I: IpExt, D: WeakDeviceIdentifier, BT: IcmpEchoBindingsTypes> PartialEq<IcmpSocketId<I, D, BT>>
    for WeakIcmpSocketId<I, D, BT>
{
    fn eq(&self, other: &IcmpSocketId<I, D, BT>) -> bool {
        PartialEq::eq(other, self)
    }
}

impl<I: IpExt, D: WeakDeviceIdentifier, BT: IcmpEchoBindingsTypes> Debug
    for WeakIcmpSocketId<I, D, BT>
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let Self(rc) = self;
        f.debug_tuple("WeakIcmpSocketId").field(&rc.debug_id()).finish()
    }
}

impl<I: IpExt, D: WeakDeviceIdentifier, BT: IcmpEchoBindingsTypes> WeakIcmpSocketId<I, D, BT> {
    #[cfg_attr(feature = "instrumented", track_caller)]
    pub fn upgrade(&self) -> Option<IcmpSocketId<I, D, BT>> {
        let Self(rc) = self;
        rc.upgrade().map(IcmpSocketId)
    }
}

/// The set of ICMP sockets.
pub type IcmpSocketSet<I, D, BT> = DatagramSocketSet<I, D, Icmp<BT>>;
/// The state of an ICMP socket.
pub type IcmpSocketState<I, D, BT> = datagram::SocketState<I, D, Icmp<BT>>;
/// The tx metadata for an ICMP echo socket.
pub type IcmpSocketTxMetadata<I, D, BT> = datagram::TxMetadata<I, D, Icmp<BT>>;

/// Errors that Bindings may encounter when receiving an ICMP Echo datagram.
pub enum ReceiveIcmpEchoError {
    /// The socket's receive queue is full and can't hold the datagram.
    QueueFull,
}

/// The context required by the ICMP layer in order to deliver events related to
/// ICMP sockets.
pub trait IcmpEchoBindingsContext<I: IpExt, D: StrongDeviceIdentifier>:
    IcmpEchoBindingsTypes + ReferenceNotifiers + RngContext + SettingsContext<IcmpEchoSettings>
{
    /// Receives an ICMP echo reply.
    fn receive_icmp_echo_reply<B: BufferMut>(
        &mut self,
        conn: &IcmpSocketId<I, D::Weak, Self>,
        device_id: &D,
        src_ip: I::Addr,
        dst_ip: I::Addr,
        id: u16,
        data: B,
    ) -> Result<(), ReceiveIcmpEchoError>;
}

/// The bindings context providing external types to ICMP sockets.
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
pub trait IcmpEchoBindingsTypes: DatagramBindingsTypes + Sized + 'static {
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

/// Resolve coherence issues by requiring a trait implementation with no type
/// parameters, which makes the blanket implementations for the datagram specs
/// viable.
pub trait IcmpEchoContextMarker {}

/// A Context that provides access to the sockets' states.
pub trait IcmpEchoBoundStateContext<I: IcmpIpExt + IpExt, BC: IcmpEchoBindingsTypes>:
    DeviceIdContext<AnyDevice> + IcmpEchoContextMarker
{
    /// The inner context providing IP socket access.
    type IpSocketsCtx<'a>: TransportIpContext<I, BC>
        + MulticastMembershipHandler<I, BC>
        + DeviceIdContext<AnyDevice, DeviceId = Self::DeviceId, WeakDeviceId = Self::WeakDeviceId>
        + CounterContext<IcmpRxCounters<I>>
        + CoreTxMetadataContext<IcmpSocketTxMetadata<I, Self::WeakDeviceId, BC>, BC>;

    /// Calls the function with a mutable reference to `IpSocketsCtx` and
    /// a mutable reference to ICMP sockets.
    fn with_icmp_ctx_and_sockets_mut<
        O,
        F: FnOnce(&mut Self::IpSocketsCtx<'_>, &mut BoundSockets<I, Self::WeakDeviceId, BC>) -> O,
    >(
        &mut self,
        cb: F,
    ) -> O;
}

/// A Context that provides access to the sockets' states.
pub trait IcmpEchoStateContext<I: IcmpIpExt + IpExt, BC: IcmpEchoBindingsTypes>:
    DeviceIdContext<AnyDevice> + IcmpEchoContextMarker
{
    /// The inner socket context.
    type SocketStateCtx<'a>: IcmpEchoBoundStateContext<I, BC>
        + DeviceIdContext<AnyDevice, DeviceId = Self::DeviceId, WeakDeviceId = Self::WeakDeviceId>;

    /// Calls the function with mutable access to the set with all ICMP
    /// sockets.
    fn with_all_sockets_mut<O, F: FnOnce(&mut IcmpSocketSet<I, Self::WeakDeviceId, BC>) -> O>(
        &mut self,
        cb: F,
    ) -> O;

    /// Calls the function with immutable access to the set with all ICMP
    /// sockets.
    fn with_all_sockets<O, F: FnOnce(&IcmpSocketSet<I, Self::WeakDeviceId, BC>) -> O>(
        &mut self,
        cb: F,
    ) -> O;

    /// Calls the function without access to ICMP socket state.
    fn with_bound_state_context<O, F: FnOnce(&mut Self::SocketStateCtx<'_>) -> O>(
        &mut self,
        cb: F,
    ) -> O;

    /// Calls the function with an immutable reference to the given socket's
    /// state.
    fn with_socket_state<
        O,
        F: FnOnce(&mut Self::SocketStateCtx<'_>, &IcmpSocketState<I, Self::WeakDeviceId, BC>) -> O,
    >(
        &mut self,
        id: &IcmpSocketId<I, Self::WeakDeviceId, BC>,
        cb: F,
    ) -> O;

    /// Calls the function with a mutable reference to the given socket's state.
    fn with_socket_state_mut<
        O,
        F: FnOnce(&mut Self::SocketStateCtx<'_>, &mut IcmpSocketState<I, Self::WeakDeviceId, BC>) -> O,
    >(
        &mut self,
        id: &IcmpSocketId<I, Self::WeakDeviceId, BC>,
        cb: F,
    ) -> O;

    /// Call `f` with each socket's state.
    fn for_each_socket<
        F: FnMut(
            &mut Self::SocketStateCtx<'_>,
            &IcmpSocketId<I, Self::WeakDeviceId, BC>,
            &IcmpSocketState<I, Self::WeakDeviceId, BC>,
        ),
    >(
        &mut self,
        cb: F,
    );
}

/// Uninstantiatable type for implementing [`DatagramSocketSpec`].
pub struct Icmp<BT>(PhantomData<BT>, Never);

impl<BT: IcmpEchoBindingsTypes> DatagramSocketSpec for Icmp<BT> {
    const NAME: &'static str = "ICMP_ECHO";
    type AddrSpec = IcmpAddrSpec;

    type SocketId<I: datagram::IpExt, D: WeakDeviceIdentifier> = IcmpSocketId<I, D, BT>;
    type WeakSocketId<I: datagram::IpExt, D: WeakDeviceIdentifier> = WeakIcmpSocketId<I, D, BT>;

    type OtherStackIpOptions<I: datagram::IpExt, D: WeakDeviceIdentifier> = ();

    type SharingState = ();

    type SocketMapSpec<I: datagram::IpExt + datagram::DualStackIpExt, D: WeakDeviceIdentifier> =
        IcmpSocketMapStateSpec<I, D, BT>;

    fn ip_proto<I: IpProtoExt>() -> I::Proto {
        I::map_ip((), |()| Ipv4Proto::Icmp, |()| Ipv6Proto::Icmpv6)
    }

    fn make_bound_socket_map_id<I: datagram::IpExt, D: WeakDeviceIdentifier>(
        s: &Self::SocketId<I, D>,
    ) -> <Self::SocketMapSpec<I, D> as datagram::DatagramSocketMapSpec<
        I,
        D,
        Self::AddrSpec,
    >>::BoundSocketId{
        s.clone()
    }

    type Serializer<I: datagram::IpExt, B: BufferMut> =
        packet::Nested<B, IcmpPacketBuilder<I, IcmpEchoRequest>>;
    type SerializeError = packet_formats::error::ParseError;

    type ExternalData<I: Ip> = BT::ExternalData<I>;
    type Settings = IcmpEchoSettings;

    // NB: At present, there's no need to track per-socket ICMP counters.
    type Counters<I: Ip> = ();
    type SocketWritableListener = BT::SocketWritableListener;
    type SendToken = BT::SendToken;

    // NB: `make_packet` does not add any extra bytes because applications send
    // the ICMP header alongside the message which gets parsed and then rebuilt.
    // That means we incur 0 extra cost here.
    const FIXED_HEADER_SIZE: usize = 0;

    fn make_packet<I: datagram::IpExt, B: BufferMut>(
        mut body: B,
        addr: &socket::ConnIpAddr<
            I::Addr,
            <Self::AddrSpec as SocketMapAddrSpec>::LocalIdentifier,
            <Self::AddrSpec as SocketMapAddrSpec>::RemoteIdentifier,
        >,
    ) -> Result<Self::Serializer<I, B>, Self::SerializeError> {
        let ConnIpAddr { local: (local_ip, id), remote: (remote_ip, ()) } = addr;
        let icmp_echo: packet_formats::icmp::IcmpPacketRaw<I, &[u8], IcmpEchoRequest> =
            body.parse()?;
        debug!(
            "preparing ICMP echo request {local_ip} to {remote_ip}: id={}, seq={}",
            id,
            icmp_echo.message().seq()
        );
        let icmp_builder = IcmpPacketBuilder::<I, _>::new(
            local_ip.addr(),
            remote_ip.addr(),
            packet_formats::icmp::IcmpZeroCode,
            IcmpEchoRequest::new(id.get(), icmp_echo.message().seq()),
        );
        Ok(icmp_builder.wrap_body(body))
    }

    fn try_alloc_listen_identifier<I: datagram::IpExt, D: WeakDeviceIdentifier>(
        bindings_ctx: &mut impl RngContext,
        is_available: impl Fn(
            <Self::AddrSpec as SocketMapAddrSpec>::LocalIdentifier,
        ) -> Result<(), datagram::InUseError>,
    ) -> Option<<Self::AddrSpec as SocketMapAddrSpec>::LocalIdentifier> {
        let mut port = IcmpPortAlloc::<I, D, BT>::rand_ephemeral(&mut bindings_ctx.rng());
        for _ in IcmpPortAlloc::<I, D, BT>::EPHEMERAL_RANGE {
            // We can unwrap here because we know that the EPHEMERAL_RANGE doesn't
            // include 0.
            let tryport = NonZeroU16::new(port.get()).unwrap();
            match is_available(tryport) {
                Ok(()) => return Some(tryport),
                Err(datagram::InUseError {}) => port.next(),
            }
        }
        None
    }

    type ListenerIpAddr<I: datagram::IpExt> = socket::ListenerIpAddr<I::Addr, NonZeroU16>;

    type ConnIpAddr<I: datagram::IpExt> = ConnIpAddr<
        I::Addr,
        <Self::AddrSpec as SocketMapAddrSpec>::LocalIdentifier,
        <Self::AddrSpec as SocketMapAddrSpec>::RemoteIdentifier,
    >;

    type ConnState<I: datagram::IpExt, D: WeakDeviceIdentifier> = datagram::ConnState<I, D, Self>;
    // Store the remote port/id set by `connect`. This does not participate in
    // demuxing, so not part of the socketmap, but we need to store it so that
    // it can be reported later.
    type ConnStateExtra = u16;

    fn conn_info_from_state<I: IpExt, D: WeakDeviceIdentifier>(
        state: &Self::ConnState<I, D>,
    ) -> datagram::ConnInfo<I::Addr, D> {
        let ConnAddr { ip, device } = state.addr();
        let extra = state.extra();
        let ConnInfoAddr { local: (local_ip, local_identifier), remote: (remote_ip, ()) } =
            ip.clone().into();
        datagram::ConnInfo::new(local_ip, local_identifier, remote_ip, *extra, || {
            // The invariant that a zone is present if needed is upheld by connect.
            device.clone().expect("device must be bound for addresses that require zones")
        })
    }

    fn try_alloc_local_id<I: IpExt, D: WeakDeviceIdentifier, BC: RngContext>(
        bound: &IcmpBoundSockets<I, D, BT>,
        bindings_ctx: &mut BC,
        flow: datagram::DatagramFlowId<I::Addr, ()>,
    ) -> Option<NonZeroU16> {
        let mut rng = bindings_ctx.rng();
        netstack3_base::simple_randomized_port_alloc(&mut rng, &flow, &IcmpPortAlloc(bound), &())
            .map(|p| NonZeroU16::new(p).expect("ephemeral ports should be non-zero"))
    }

    fn upgrade_socket_id<I: datagram::IpExt, D: WeakDeviceIdentifier>(
        id: &Self::WeakSocketId<I, D>,
    ) -> Option<Self::SocketId<I, D>> {
        id.upgrade()
    }

    fn downgrade_socket_id<I: datagram::IpExt, D: WeakDeviceIdentifier>(
        id: &Self::SocketId<I, D>,
    ) -> Self::WeakSocketId<I, D> {
        IcmpSocketId::downgrade(id)
    }
}

/// Uninstantiatable type for implementing [`SocketMapAddrSpec`].
pub enum IcmpAddrSpec {}

impl SocketMapAddrSpec for IcmpAddrSpec {
    type RemoteIdentifier = ();
    type LocalIdentifier = NonZeroU16;
}

type IcmpBoundSockets<I, D, BT> = datagram::BoundDatagramSocketMap<I, D, Icmp<BT>>;

struct IcmpPortAlloc<'a, I: IpExt, D: WeakDeviceIdentifier, BT: IcmpEchoBindingsTypes>(
    &'a IcmpBoundSockets<I, D, BT>,
);

impl<I: IpExt, D: WeakDeviceIdentifier, BT: IcmpEchoBindingsTypes> PortAllocImpl
    for IcmpPortAlloc<'_, I, D, BT>
{
    const EPHEMERAL_RANGE: core::ops::RangeInclusive<u16> = 1..=u16::MAX;
    type Id = DatagramFlowId<I::Addr, ()>;
    type PortAvailableArg = ();

    fn is_port_available(&self, id: &Self::Id, port: u16, (): &()) -> bool {
        let Self(socketmap) = self;
        // We can safely unwrap here, because the ports received in
        // `is_port_available` are guaranteed to be in `EPHEMERAL_RANGE`.
        let port = NonZeroU16::new(port).unwrap();
        let conn = ConnAddr {
            ip: ConnIpAddr { local: (id.local_ip, port), remote: (id.remote_ip, ()) },
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

/// The demux state for ICMP echo sockets.
#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct BoundSockets<I: IpExt, D: WeakDeviceIdentifier, BT: IcmpEchoBindingsTypes> {
    pub(crate) socket_map: IcmpBoundSockets<I, D, BT>,
}

impl<I, BC, CC> NonDualStackDatagramSpecBoundStateContext<I, CC, BC> for Icmp<BC>
where
    I: IpExt + datagram::DualStackIpExt,
    BC: IcmpEchoBindingsContext<I, CC::DeviceId>,
    CC: DeviceIdContext<AnyDevice> + IcmpEchoContextMarker,
{
    fn nds_converter(_core_ctx: &CC) -> impl NonDualStackConverter<I, CC::WeakDeviceId, Self> {
        ()
    }
}

impl<I, BC, CC> DatagramSpecBoundStateContext<I, CC, BC> for Icmp<BC>
where
    I: IpExt + datagram::DualStackIpExt,
    BC: IcmpEchoBindingsContext<I, CC::DeviceId>,
    CC: IcmpEchoBoundStateContext<I, BC> + IcmpEchoContextMarker,
{
    type IpSocketsCtx<'a> = CC::IpSocketsCtx<'a>;

    // ICMP sockets doesn't support dual-stack operations.
    type DualStackContext = UninstantiableWrapper<CC>;

    type NonDualStackContext = CC;

    fn with_bound_sockets<
        O,
        F: FnOnce(&mut Self::IpSocketsCtx<'_>, &IcmpBoundSockets<I, CC::WeakDeviceId, BC>) -> O,
    >(
        core_ctx: &mut CC,
        cb: F,
    ) -> O {
        IcmpEchoBoundStateContext::with_icmp_ctx_and_sockets_mut(
            core_ctx,
            |ctx, BoundSockets { socket_map }| cb(ctx, &socket_map),
        )
    }

    fn with_bound_sockets_mut<
        O,
        F: FnOnce(&mut Self::IpSocketsCtx<'_>, &mut IcmpBoundSockets<I, CC::WeakDeviceId, BC>) -> O,
    >(
        core_ctx: &mut CC,
        cb: F,
    ) -> O {
        IcmpEchoBoundStateContext::with_icmp_ctx_and_sockets_mut(
            core_ctx,
            |ctx, BoundSockets { socket_map }| cb(ctx, socket_map),
        )
    }

    fn dual_stack_context(
        core_ctx: &CC,
    ) -> MaybeDualStack<&Self::DualStackContext, &Self::NonDualStackContext> {
        MaybeDualStack::NotDualStack(core_ctx)
    }

    fn dual_stack_context_mut(
        core_ctx: &mut CC,
    ) -> MaybeDualStack<&mut Self::DualStackContext, &mut Self::NonDualStackContext> {
        MaybeDualStack::NotDualStack(core_ctx)
    }

    fn with_transport_context<O, F: FnOnce(&mut Self::IpSocketsCtx<'_>) -> O>(
        core_ctx: &mut CC,
        cb: F,
    ) -> O {
        IcmpEchoBoundStateContext::with_icmp_ctx_and_sockets_mut(core_ctx, |ctx, _sockets| cb(ctx))
    }
}

impl<I, BC, CC> DatagramSpecStateContext<I, CC, BC> for Icmp<BC>
where
    I: IpExt + datagram::DualStackIpExt,
    BC: IcmpEchoBindingsContext<I, CC::DeviceId>,
    CC: IcmpEchoStateContext<I, BC>,
{
    type SocketsStateCtx<'a> = CC::SocketStateCtx<'a>;

    fn with_all_sockets_mut<O, F: FnOnce(&mut IcmpSocketSet<I, CC::WeakDeviceId, BC>) -> O>(
        core_ctx: &mut CC,
        cb: F,
    ) -> O {
        IcmpEchoStateContext::with_all_sockets_mut(core_ctx, cb)
    }

    fn with_all_sockets<O, F: FnOnce(&IcmpSocketSet<I, CC::WeakDeviceId, BC>) -> O>(
        core_ctx: &mut CC,
        cb: F,
    ) -> O {
        IcmpEchoStateContext::with_all_sockets(core_ctx, cb)
    }

    fn with_socket_state<
        O,
        F: FnOnce(&mut Self::SocketsStateCtx<'_>, &IcmpSocketState<I, CC::WeakDeviceId, BC>) -> O,
    >(
        core_ctx: &mut CC,
        id: &IcmpSocketId<I, CC::WeakDeviceId, BC>,
        cb: F,
    ) -> O {
        IcmpEchoStateContext::with_socket_state(core_ctx, id, cb)
    }

    fn with_socket_state_mut<
        O,
        F: FnOnce(&mut Self::SocketsStateCtx<'_>, &mut IcmpSocketState<I, CC::WeakDeviceId, BC>) -> O,
    >(
        core_ctx: &mut CC,
        id: &IcmpSocketId<I, CC::WeakDeviceId, BC>,
        cb: F,
    ) -> O {
        IcmpEchoStateContext::with_socket_state_mut(core_ctx, id, cb)
    }

    fn for_each_socket<
        F: FnMut(
            &mut Self::SocketsStateCtx<'_>,
            &IcmpSocketId<I, CC::WeakDeviceId, BC>,
            &IcmpSocketState<I, CC::WeakDeviceId, BC>,
        ),
    >(
        core_ctx: &mut CC,
        cb: F,
    ) {
        IcmpEchoStateContext::for_each_socket(core_ctx, cb)
    }
}

/// An uninstantiable type providing a [`SocketMapStateSpec`] implementation for
/// ICMP.
pub struct IcmpSocketMapStateSpec<I, D, BT>(PhantomData<(I, D, BT)>, Never);

impl<I: IpExt, D: WeakDeviceIdentifier, BT: IcmpEchoBindingsTypes> SocketMapStateSpec
    for IcmpSocketMapStateSpec<I, D, BT>
{
    type ListenerId = IcmpSocketId<I, D, BT>;
    type ConnId = IcmpSocketId<I, D, BT>;

    type AddrVecTag = ();

    type ListenerSharingState = ();
    type ConnSharingState = ();

    type ListenerAddrState = Self::ListenerId;

    type ConnAddrState = Self::ConnId;
    fn listener_tag(
        ListenerAddrInfo { has_device: _, specified_addr: _ }: ListenerAddrInfo,
        _state: &Self::ListenerAddrState,
    ) -> Self::AddrVecTag {
        ()
    }
    fn connected_tag(_has_device: bool, _state: &Self::ConnAddrState) -> Self::AddrVecTag {
        ()
    }
}

impl<I: IpExt, D: WeakDeviceIdentifier, BT: IcmpEchoBindingsTypes> SocketMapAddrStateSpec
    for IcmpSocketId<I, D, BT>
{
    type Id = Self;

    type SharingState = ();

    type Inserter<'a>
        = core::convert::Infallible
    where
        Self: 'a;

    fn new(_new_sharing_state: &Self::SharingState, id: Self::Id) -> Self {
        id
    }

    fn contains_id(&self, id: &Self::Id) -> bool {
        self == id
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

    fn remove_by_id(&mut self, _id: Self::Id) -> socket::RemoveResult {
        socket::RemoveResult::IsLast
    }

    fn sharing_state(&self) -> Self::SharingState {
        ()
    }
}

impl<I: IpExt, D: WeakDeviceIdentifier, BT: IcmpEchoBindingsTypes>
    DatagramSocketMapSpec<I, D, IcmpAddrSpec> for IcmpSocketMapStateSpec<I, D, BT>
{
    type BoundSocketId = IcmpSocketId<I, D, BT>;
}

impl<AA, I: IpExt, D: WeakDeviceIdentifier, BT: IcmpEchoBindingsTypes>
    SocketMapConflictPolicy<AA, (), I, D, IcmpAddrSpec> for IcmpSocketMapStateSpec<I, D, BT>
where
    AA: Into<AddrVec<I, D, IcmpAddrSpec>> + Clone,
{
    fn check_insert_conflicts(
        _new_sharing_state: &(),
        addr: &AA,
        socketmap: &SocketMap<AddrVec<I, D, IcmpAddrSpec>, socket::Bound<Self>>,
    ) -> Result<(), socket::InsertError> {
        let addr: AddrVec<_, _, _> = addr.clone().into();
        // Having a value present at a shadowed address is disqualifying.
        if addr.iter_shadows().any(|a| socketmap.get(&a).is_some()) {
            return Err(InsertError::ShadowAddrExists);
        }

        // Likewise, the presence of a value that shadows the target address is
        // also disqualifying.
        if socketmap.descendant_counts(&addr).len() != 0 {
            return Err(InsertError::WouldShadowExisting);
        }
        Ok(())
    }
}

/// The ICMP Echo sockets API.
pub struct IcmpEchoSocketApi<I: Ip, C>(C, IpVersionMarker<I>);

impl<I: Ip, C> IcmpEchoSocketApi<I, C> {
    /// Creates a new API instance.
    pub fn new(ctx: C) -> Self {
        Self(ctx, IpVersionMarker::new())
    }
}

/// A local alias for [`IcmpSocketId`] for use in [`IcmpEchoSocketApi`].
///
/// TODO(https://github.com/rust-lang/rust/issues/8995): Make this an inherent
/// associated type.
type IcmpApiSocketId<I, C> = IcmpSocketId<
    I,
    <<C as ContextPair>::CoreContext as DeviceIdContext<AnyDevice>>::WeakDeviceId,
    <C as ContextPair>::BindingsContext,
>;

impl<I, C> IcmpEchoSocketApi<I, C>
where
    I: datagram::IpExt,
    C: ContextPair,
    C::CoreContext: IcmpEchoStateContext<I, C::BindingsContext>
        // NB: This bound is somewhat redundant to StateContext but it helps the
        // compiler know we're using ICMP datagram sockets.
        + DatagramStateContext<I, C::BindingsContext, Icmp<C::BindingsContext>>,
    C::BindingsContext:
        IcmpEchoBindingsContext<I, <C::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId>,
{
    fn core_ctx(&mut self) -> &mut C::CoreContext {
        let Self(pair, IpVersionMarker { .. }) = self;
        pair.core_ctx()
    }

    fn datagram(&mut self) -> &mut DatagramApi<I, C, Icmp<C::BindingsContext>> {
        let Self(pair, IpVersionMarker { .. }) = self;
        DatagramApi::wrap(pair)
    }

    /// Creates a new unbound ICMP socket with default external data.
    pub fn create(&mut self) -> IcmpApiSocketId<I, C>
    where
        <C::BindingsContext as IcmpEchoBindingsTypes>::ExternalData<I>: Default,
        <C::BindingsContext as IcmpEchoBindingsTypes>::SocketWritableListener: Default,
    {
        self.create_with(Default::default(), Default::default())
    }

    /// Creates a new unbound ICMP socket with provided external data.
    pub fn create_with(
        &mut self,
        external_data: <C::BindingsContext as IcmpEchoBindingsTypes>::ExternalData<I>,
        writable_listener: <C::BindingsContext as IcmpEchoBindingsTypes>::SocketWritableListener,
    ) -> IcmpApiSocketId<I, C> {
        self.datagram().create(external_data, writable_listener)
    }

    /// Connects an ICMP socket to remote IP.
    ///
    /// If the socket is never bound, an local ID will be allocated.
    pub fn connect(
        &mut self,
        id: &IcmpApiSocketId<I, C>,
        remote_ip: Option<
            ZonedAddr<
                SpecifiedAddr<I::Addr>,
                <C::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId,
            >,
        >,
        remote_id: u16,
    ) -> Result<(), datagram::ConnectError> {
        self.datagram().connect(id, remote_ip, (), remote_id)
    }

    /// Binds an ICMP socket to a local IP address and a local ID.
    ///
    /// Both the IP and the ID are optional. When IP is missing, the "any" IP is
    /// assumed; When the ID is missing, it will be allocated.
    pub fn bind(
        &mut self,
        id: &IcmpApiSocketId<I, C>,
        local_ip: Option<
            ZonedAddr<
                SpecifiedAddr<I::Addr>,
                <C::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId,
            >,
        >,
        icmp_id: Option<NonZeroU16>,
    ) -> Result<(), Either<ExpectedUnboundError, LocalAddressError>> {
        self.datagram().listen(id, local_ip, icmp_id)
    }

    /// Gets the information about an ICMP socket.
    pub fn get_info(
        &mut self,
        id: &IcmpApiSocketId<I, C>,
    ) -> datagram::SocketInfo<I::Addr, <C::CoreContext as DeviceIdContext<AnyDevice>>::WeakDeviceId>
    {
        self.datagram().get_info(id)
    }

    /// Sets the bound device for a socket.
    ///
    /// Sets the device to be used for sending and receiving packets for a
    /// socket. If the socket is not currently bound to a local address and
    /// port, the device will be used when binding.
    pub fn set_device(
        &mut self,
        id: &IcmpApiSocketId<I, C>,
        device_id: Option<&<C::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId>,
    ) -> Result<(), SocketError> {
        self.datagram().set_device(id, device_id)
    }

    /// Gets the device the specified socket is bound to.
    pub fn get_bound_device(
        &mut self,
        id: &IcmpApiSocketId<I, C>,
    ) -> Option<<C::CoreContext as DeviceIdContext<AnyDevice>>::WeakDeviceId> {
        self.datagram().get_bound_device(id)
    }

    /// Disconnects an ICMP socket.
    pub fn disconnect(
        &mut self,
        id: &IcmpApiSocketId<I, C>,
    ) -> Result<(), datagram::ExpectedConnError> {
        self.datagram().disconnect_connected(id)
    }

    /// Shuts down an ICMP socket.
    pub fn shutdown(
        &mut self,
        id: &IcmpApiSocketId<I, C>,
        shutdown_type: ShutdownType,
    ) -> Result<(), datagram::ExpectedConnError> {
        self.datagram().shutdown_connected(id, shutdown_type)
    }

    /// Gets the current shutdown state of an ICMP socket.
    pub fn get_shutdown(&mut self, id: &IcmpApiSocketId<I, C>) -> Option<ShutdownType> {
        self.datagram().get_shutdown_connected(id)
    }

    /// Closes an ICMP socket.
    pub fn close(
        &mut self,
        id: IcmpApiSocketId<I, C>,
    ) -> RemoveResourceResultWithContext<
        <C::BindingsContext as IcmpEchoBindingsTypes>::ExternalData<I>,
        C::BindingsContext,
    > {
        self.datagram().close(id, |state| {
            let (_state, external_data) = state.into_state_and_external_data();
            external_data
        })
    }

    /// Gets unicast IP hop limit for ICMP sockets.
    pub fn get_unicast_hop_limit(&mut self, id: &IcmpApiSocketId<I, C>) -> NonZeroU8 {
        self.datagram().get_ip_hop_limits(id).unicast
    }

    /// Gets multicast IP hop limit for ICMP sockets.
    pub fn get_multicast_hop_limit(&mut self, id: &IcmpApiSocketId<I, C>) -> NonZeroU8 {
        self.datagram().get_ip_hop_limits(id).multicast
    }

    /// Sets unicast IP hop limit for ICMP sockets.
    pub fn set_unicast_hop_limit(
        &mut self,
        id: &IcmpApiSocketId<I, C>,
        hop_limit: Option<NonZeroU8>,
    ) {
        self.datagram().update_ip_hop_limit(id, SocketHopLimits::set_unicast(hop_limit))
    }

    /// Sets multicast IP hop limit for ICMP sockets.
    pub fn set_multicast_hop_limit(
        &mut self,
        id: &IcmpApiSocketId<I, C>,
        hop_limit: Option<NonZeroU8>,
    ) {
        self.datagram().update_ip_hop_limit(id, SocketHopLimits::set_multicast(hop_limit))
    }

    /// Gets the loopback multicast option.
    pub fn get_multicast_loop(&mut self, id: &IcmpApiSocketId<I, C>) -> bool {
        self.datagram().get_multicast_loop(id)
    }

    /// Sets the loopback multicast option.
    pub fn set_multicast_loop(&mut self, id: &IcmpApiSocketId<I, C>, value: bool) {
        self.datagram().set_multicast_loop(id, value);
    }

    /// Sets the socket mark for the socket domain.
    pub fn set_mark(&mut self, id: &IcmpApiSocketId<I, C>, domain: MarkDomain, mark: Mark) {
        self.datagram().set_mark(id, domain, mark)
    }

    /// Gets the socket mark for the socket domain.
    pub fn get_mark(&mut self, id: &IcmpApiSocketId<I, C>, domain: MarkDomain) -> Mark {
        self.datagram().get_mark(id, domain)
    }

    /// Sets the send buffer maximum size to `size`.
    pub fn set_send_buffer(&mut self, id: &IcmpApiSocketId<I, C>, size: usize) {
        self.datagram().set_send_buffer(id, size)
    }

    /// Returns the current maximum send buffer size.
    pub fn send_buffer(&mut self, id: &IcmpApiSocketId<I, C>) -> usize {
        self.datagram().send_buffer(id)
    }

    /// Sends an ICMP packet through a connection.
    ///
    /// The socket must be connected in order for the operation to succeed.
    pub fn send<B: BufferMut>(
        &mut self,
        id: &IcmpApiSocketId<I, C>,
        body: B,
        send_token: <C::BindingsContext as IcmpEchoBindingsTypes>::SendToken,
    ) -> Result<(), datagram::SendError<packet_formats::error::ParseError>> {
        self.datagram().send_conn(id, body, send_token)
    }

    /// Sends an ICMP packet with an remote address.
    ///
    /// The socket doesn't need to be connected.
    pub fn send_to<B: BufferMut>(
        &mut self,
        id: &IcmpApiSocketId<I, C>,
        remote_ip: Option<
            ZonedAddr<
                SpecifiedAddr<I::Addr>,
                <C::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId,
            >,
        >,
        body: B,
        send_token: <C::BindingsContext as IcmpEchoBindingsTypes>::SendToken,
    ) -> Result<(), datagram::SendToError<packet_formats::error::ParseError>> {
        self.datagram().send_to(id, remote_ip, (), body, send_token)
    }

    /// Collects all currently opened sockets, returning a cloned reference for
    /// each one.
    pub fn collect_all_sockets(&mut self) -> Vec<IcmpApiSocketId<I, C>> {
        self.datagram().collect_all_sockets()
    }

    /// Provides inspect data for ICMP echo sockets.
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
            });
        });
    }
}

/// An [`IpTransportContext`] implementation for handling ICMP Echo replies.
///
/// This special implementation will panic if it receives any packets that are
/// not ICMP echo replies and any error that are not originally an ICMP Echo
/// request.
pub enum IcmpEchoIpTransportContext {}

impl EchoTransportContextMarker for IcmpEchoIpTransportContext {}

impl<I: IpExt, BC: IcmpEchoBindingsContext<I, CC::DeviceId>, CC: IcmpEchoBoundStateContext<I, BC>>
    IpTransportContext<I, BC, CC> for IcmpEchoIpTransportContext
{
    type EarlyDemuxSocket = Never;

    fn early_demux<B: ParseBuffer>(
        _core_ctx: &mut CC,
        _device: &CC::DeviceId,
        _src_ip: I::Addr,
        _dst_ip: I::Addr,
        _buffer: B,
    ) -> Option<Self::EarlyDemuxSocket> {
        None
    }

    fn receive_icmp_error(
        core_ctx: &mut CC,
        _bindings_ctx: &mut BC,
        _device: &CC::DeviceId,
        original_src_ip: Option<SpecifiedAddr<I::Addr>>,
        original_dst_ip: SpecifiedAddr<I::Addr>,
        mut original_body: &[u8],
        err: I::ErrorCode,
    ) {
        let echo_request = original_body
            .parse::<IcmpPacketRaw<I, _, IcmpEchoRequest>>()
            .expect("received non-echo request");

        let original_src_ip = match original_src_ip {
            Some(ip) => ip,
            None => {
                trace!("IcmpIpTransportContext::receive_icmp_error: unspecified source IP address");
                return;
            }
        };
        let original_src_ip: SocketIpAddr<_> = match original_src_ip.try_into() {
            Ok(ip) => ip,
            Err(AddrIsMappedError {}) => {
                trace!("IcmpIpTransportContext::receive_icmp_error: mapped source IP address");
                return;
            }
        };
        let original_dst_ip: SocketIpAddr<_> = match original_dst_ip.try_into() {
            Ok(ip) => ip,
            Err(AddrIsMappedError {}) => {
                trace!("IcmpIpTransportContext::receive_icmp_error: mapped destination IP address");
                return;
            }
        };

        // The netstack will never generate an ID of 0, so we can just throw
        // this away for being invalid.
        let Some(id) = NonZeroU16::new(echo_request.message().id()) else {
            debug!(
                "ICMP received ICMP error {:?} from {:?}, to {:?} with an ID of 0",
                err, original_dst_ip, original_src_ip,
            );
            return;
        };

        core_ctx.with_icmp_ctx_and_sockets_mut(|core_ctx, sockets| {
            if let Some(conn) = sockets.socket_map.conns().get_by_addr(&ConnAddr {
                ip: ConnIpAddr { local: (original_src_ip, id), remote: (original_dst_ip, ()) },
                device: None,
            }) {
                // NB: At the moment bindings has no need to consume ICMP
                // errors, so we swallow them here.
                debug!(
                    "ICMP received ICMP error {:?} from {:?}, to {:?} on socket {:?}",
                    err, original_dst_ip, original_src_ip, conn
                );
                CounterContext::<IcmpRxCounters<I>>::counters(core_ctx)
                    .error_delivered_to_socket
                    .increment()
            } else {
                trace!(
                    "IcmpIpTransportContext::receive_icmp_error: Got ICMP error message for \
                    nonexistent ICMP echo socket; either the socket responsible has since been \
                    removed, or the error message was sent in error or corrupted"
                );
            }
        })
    }

    fn receive_ip_packet<B: BufferMut, H: IpHeaderInfo<I>>(
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        device: &CC::DeviceId,
        src_ip: I::RecvSrcAddr,
        dst_ip: SpecifiedAddr<I::Addr>,
        mut buffer: B,
        info: &mut LocalDeliveryPacketInfo<I, H>,
        _early_demux_socket: Option<Never>,
    ) -> Result<(), (B, I::IcmpError)> {
        let LocalDeliveryPacketInfo { meta, header_info: _, marks: _ } = info;
        let ReceiveIpPacketMeta { broadcast: _, transparent_override, parsing_context: _ } = meta;
        if let Some(delivery) = transparent_override.as_ref() {
            unreachable!(
                "cannot perform transparent local delivery {delivery:?} to an ICMP socket; \
                transparent proxy rules can only be configured for TCP and UDP packets"
            );
        }
        // NB: We're doing raw parsing here just to extract the ID and body to
        // send up to bindings. The IP layer has performed full validation
        // including checksum for us.
        let echo_reply =
            buffer.parse::<IcmpPacketRaw<I, _, IcmpEchoReply>>().expect("received non-echo reply");
        // We never generate requests with ID zero due to local socket map.
        let Some(id) = NonZeroU16::new(echo_reply.message().id()) else { return Ok(()) };

        // Undo parse so we give out the full ICMP header.
        let meta = echo_reply.parse_metadata();
        buffer.undo_parse(meta);

        let src_ip = match SpecifiedAddr::new(src_ip.into_addr()) {
            Some(src_ip) => src_ip,
            None => {
                trace!("receive_icmp_echo_reply: unspecified source address");
                return Ok(());
            }
        };
        let src_ip: SocketIpAddr<_> = match src_ip.try_into() {
            Ok(src_ip) => src_ip,
            Err(AddrIsMappedError {}) => {
                trace!("receive_icmp_echo_reply: mapped source address");
                return Ok(());
            }
        };
        let dst_ip: SocketIpAddr<_> = match dst_ip.try_into() {
            Ok(dst_ip) => dst_ip,
            Err(AddrIsMappedError {}) => {
                trace!("receive_icmp_echo_reply: mapped destination address");
                return Ok(());
            }
        };

        core_ctx.with_icmp_ctx_and_sockets_mut(|core_ctx, sockets| {
            let mut addrs_to_search = AddrVecIter::<I, CC::WeakDeviceId, IcmpAddrSpec>::with_device(
                ConnIpAddr { local: (dst_ip, id), remote: (src_ip, ()) }.into(),
                device.downgrade(),
            );
            let socket = match addrs_to_search.try_for_each(|addr_vec| {
                match addr_vec {
                    AddrVec::Conn(c) => {
                        if let Some(id) = sockets.socket_map.conns().get_by_addr(&c) {
                            return ControlFlow::Break(id);
                        }
                    }
                    AddrVec::Listen(l) => {
                        if let Some(id) = sockets.socket_map.listeners().get_by_addr(&l) {
                            return ControlFlow::Break(id);
                        }
                    }
                }
                ControlFlow::Continue(())
            }) {
                ControlFlow::Continue(()) => None,
                ControlFlow::Break(id) => Some(id),
            };
            if let Some(socket) = socket {
                trace!("receive_icmp_echo_reply: Received echo reply for local socket");
                match bindings_ctx.receive_icmp_echo_reply(
                    socket,
                    device,
                    src_ip.addr(),
                    dst_ip.addr(),
                    id.get(),
                    buffer,
                ) {
                    Ok(()) => {}
                    Err(ReceiveIcmpEchoError::QueueFull) => {
                        core_ctx.counters().queue_full.increment();
                    }
                }
                return;
            }
            // TODO(https://fxbug.dev/42124755): Neither the ICMPv4 or ICMPv6 RFCs
            // explicitly state what to do in case we receive an "unsolicited"
            // echo reply. We only expose the replies if we have a registered
            // connection for the IcmpAddr of the incoming reply for now. Given
            // that a reply should only be sent in response to a request, an
            // ICMP unreachable-type message is probably not appropriate for
            // unsolicited replies. However, it's also possible that we sent a
            // request and then closed the socket before receiving the reply, so
            // this doesn't necessarily indicate a buggy or malicious remote
            // host. We should figure this out definitively.
            //
            // If we do decide to send an ICMP error message, the appropriate
            // thing to do is probably to have this function return a `Result`,
            // and then have the top-level implementation of
            // `IpTransportContext::receive_ip_packet` return the
            // appropriate error.
            trace!("receive_icmp_echo_reply: Received echo reply with no local socket");
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use alloc::rc::Rc;
    use alloc::vec;
    use core::cell::RefCell;
    use core::ops::{Deref, DerefMut};

    use assert_matches::assert_matches;
    use ip_test_macro::ip_test;
    use net_declare::net_ip_v6;
    use net_types::Witness;
    use net_types::ip::Ipv6;
    use netstack3_base::socket::StrictlyZonedAddr;
    use netstack3_base::testutil::{
        FakeBindingsCtx, FakeCoreCtx, FakeDeviceId, FakeSendToken, FakeSocketWritableListener,
        FakeWeakDeviceId, TestIpExt,
    };
    use netstack3_base::{CtxPair, Icmpv4ErrorCode, Icmpv6ErrorCode, NetworkSerializationContext};
    use netstack3_ip::socket::testutil::{FakeDeviceConfig, FakeIpSocketCtx, InnerFakeIpSocketCtx};
    use netstack3_ip::{LocalDeliveryPacketInfo, SendIpPacketMeta};
    use packet::{Buf, EmptyBuf, NestableSerializer as _, Serializer};
    use packet_formats::icmp::{
        IcmpDestUnreachable, IcmpPacket, IcmpParseArgs, IcmpZeroCode, Icmpv4DestUnreachableCode,
        Icmpv6DestUnreachableCode,
    };

    use super::*;

    const REMOTE_ID: u16 = 27;
    const ICMP_ID: NonZeroU16 = NonZeroU16::new(10).unwrap();
    const SEQ_NUM: u16 = 0xF0;

    /// Utilities for accessing locked internal state in tests.
    impl<I: IpExt, D: WeakDeviceIdentifier, BT: IcmpEchoBindingsTypes> IcmpSocketId<I, D, BT> {
        fn get(&self) -> impl Deref<Target = IcmpSocketState<I, D, BT>> + '_ {
            self.state().read()
        }

        fn get_mut(&self) -> impl DerefMut<Target = IcmpSocketState<I, D, BT>> + '_ {
            self.state().write()
        }
    }

    struct FakeIcmpCoreCtxState<I: IpExt> {
        bound_sockets:
            Rc<RefCell<BoundSockets<I, FakeWeakDeviceId<FakeDeviceId>, FakeIcmpBindingsCtx<I>>>>,
        all_sockets: IcmpSocketSet<I, FakeWeakDeviceId<FakeDeviceId>, FakeIcmpBindingsCtx<I>>,
        ip_socket_ctx: FakeIpSocketCtx<I, FakeDeviceId>,
        rx_counters: IcmpRxCounters<I>,
    }

    impl<I: IpExt> InnerFakeIpSocketCtx<I, FakeDeviceId> for FakeIcmpCoreCtxState<I> {
        fn fake_ip_socket_ctx_mut(&mut self) -> &mut FakeIpSocketCtx<I, FakeDeviceId> {
            &mut self.ip_socket_ctx
        }
    }

    impl<I: IpExt + TestIpExt> Default for FakeIcmpCoreCtxState<I> {
        fn default() -> Self {
            Self {
                bound_sockets: Default::default(),
                all_sockets: Default::default(),
                ip_socket_ctx: FakeIpSocketCtx::new(core::iter::once(FakeDeviceConfig {
                    device: FakeDeviceId,
                    local_ips: vec![I::TEST_ADDRS.local_ip],
                    remote_ips: vec![I::TEST_ADDRS.remote_ip],
                })),
                rx_counters: Default::default(),
            }
        }
    }

    type FakeIcmpCoreCtx<I> = FakeCoreCtx<
        FakeIcmpCoreCtxState<I>,
        SendIpPacketMeta<I, FakeDeviceId, SpecifiedAddr<<I as Ip>::Addr>>,
        FakeDeviceId,
    >;
    type FakeIcmpBindingsCtx<I> = FakeBindingsCtx<(), (), FakeIcmpBindingsCtxState<I>, ()>;
    type FakeIcmpCtx<I> = CtxPair<FakeIcmpCoreCtx<I>, FakeIcmpBindingsCtx<I>>;

    #[derive(Derivative)]
    #[derivative(Default)]

    struct FakeIcmpBindingsCtxState<I: IpExt> {
        received: Vec<ReceivedEchoPacket<I>>,
        #[derivative(Default(value = "usize::MAX"))]
        max_size: usize,
    }

    #[derive(Debug)]
    struct ReceivedEchoPacket<I: IpExt> {
        src_ip: I::Addr,
        dst_ip: I::Addr,
        socket: IcmpSocketId<I, FakeWeakDeviceId<FakeDeviceId>, FakeIcmpBindingsCtx<I>>,
        id: u16,
        data: Vec<u8>,
    }

    impl<I: IpExt> IcmpEchoContextMarker for FakeIcmpCoreCtx<I> {}

    impl<I: IpExt> CounterContext<IcmpRxCounters<I>> for FakeIcmpCoreCtxState<I> {
        fn counters(&self) -> &IcmpRxCounters<I> {
            &self.rx_counters
        }
    }

    impl<I: IpExt> IcmpEchoBoundStateContext<I, FakeIcmpBindingsCtx<I>> for FakeIcmpCoreCtx<I> {
        type IpSocketsCtx<'a> = Self;

        fn with_icmp_ctx_and_sockets_mut<
            O,
            F: FnOnce(
                &mut Self::IpSocketsCtx<'_>,
                &mut BoundSockets<I, Self::WeakDeviceId, FakeIcmpBindingsCtx<I>>,
            ) -> O,
        >(
            &mut self,
            cb: F,
        ) -> O {
            let bound_sockets = self.state.bound_sockets.clone();
            let mut bound_sockets = bound_sockets.borrow_mut();
            cb(self, &mut bound_sockets)
        }
    }

    impl<I: IpExt> IcmpEchoStateContext<I, FakeIcmpBindingsCtx<I>> for FakeIcmpCoreCtx<I> {
        type SocketStateCtx<'a> = Self;

        fn with_all_sockets_mut<
            O,
            F: FnOnce(&mut IcmpSocketSet<I, Self::WeakDeviceId, FakeIcmpBindingsCtx<I>>) -> O,
        >(
            &mut self,
            cb: F,
        ) -> O {
            cb(&mut self.state.all_sockets)
        }

        fn with_all_sockets<
            O,
            F: FnOnce(&IcmpSocketSet<I, Self::WeakDeviceId, FakeIcmpBindingsCtx<I>>) -> O,
        >(
            &mut self,
            cb: F,
        ) -> O {
            cb(&self.state.all_sockets)
        }

        fn with_socket_state<
            O,
            F: FnOnce(
                &mut Self::SocketStateCtx<'_>,
                &IcmpSocketState<I, Self::WeakDeviceId, FakeIcmpBindingsCtx<I>>,
            ) -> O,
        >(
            &mut self,
            id: &IcmpSocketId<I, Self::WeakDeviceId, FakeIcmpBindingsCtx<I>>,
            cb: F,
        ) -> O {
            cb(self, &id.get())
        }

        fn with_socket_state_mut<
            O,
            F: FnOnce(
                &mut Self::SocketStateCtx<'_>,
                &mut IcmpSocketState<I, Self::WeakDeviceId, FakeIcmpBindingsCtx<I>>,
            ) -> O,
        >(
            &mut self,
            id: &IcmpSocketId<I, Self::WeakDeviceId, FakeIcmpBindingsCtx<I>>,
            cb: F,
        ) -> O {
            cb(self, &mut id.get_mut())
        }

        fn with_bound_state_context<O, F: FnOnce(&mut Self::SocketStateCtx<'_>) -> O>(
            &mut self,
            cb: F,
        ) -> O {
            cb(self)
        }

        fn for_each_socket<
            F: FnMut(
                &mut Self::SocketStateCtx<'_>,
                &IcmpSocketId<I, Self::WeakDeviceId, FakeIcmpBindingsCtx<I>>,
                &IcmpSocketState<I, Self::WeakDeviceId, FakeIcmpBindingsCtx<I>>,
            ),
        >(
            &mut self,
            mut cb: F,
        ) {
            let socks = self
                .state
                .all_sockets
                .keys()
                .map(|id| IcmpSocketId::from(id.clone()))
                .collect::<Vec<_>>();
            for id in socks {
                cb(self, &id, &id.get());
            }
        }
    }

    impl<I: IpExt> IcmpEchoBindingsContext<I, FakeDeviceId> for FakeIcmpBindingsCtx<I> {
        fn receive_icmp_echo_reply<B: BufferMut>(
            &mut self,
            socket: &IcmpSocketId<I, FakeWeakDeviceId<FakeDeviceId>, FakeIcmpBindingsCtx<I>>,
            _device_id: &FakeDeviceId,
            src_ip: I::Addr,
            dst_ip: I::Addr,
            id: u16,
            data: B,
        ) -> Result<(), ReceiveIcmpEchoError> {
            if self.state.received.len() < self.state.max_size {
                self.state.received.push(ReceivedEchoPacket {
                    src_ip,
                    dst_ip,
                    id,
                    data: data.to_flattened_vec(),
                    socket: socket.clone(),
                });
                Ok(())
            } else {
                Err(ReceiveIcmpEchoError::QueueFull)
            }
        }
    }

    impl<I: IpExt> IcmpEchoBindingsTypes for FakeIcmpBindingsCtx<I> {
        type ExternalData<II: Ip> = ();
        type SocketWritableListener = FakeSocketWritableListener;
        type SendToken = FakeSendToken;
    }

    #[test]
    fn test_connect_dual_stack_fails() {
        // Verify that connecting to an ipv4-mapped-ipv6 address fails, as ICMP
        // sockets do not support dual-stack operations.
        let mut ctx = FakeIcmpCtx::<Ipv6>::default();
        let mut api = IcmpEchoSocketApi::<Ipv6, _>::new(ctx.as_mut());
        let conn = api.create();
        assert_eq!(
            api.connect(
                &conn,
                Some(ZonedAddr::Unzoned(
                    SpecifiedAddr::new(net_ip_v6!("::ffff:192.0.2.1")).unwrap(),
                )),
                REMOTE_ID,
            ),
            Err(datagram::ConnectError::RemoteUnexpectedlyMapped)
        );
    }

    #[ip_test(I)]
    fn send_invalid_icmp_echo<I: TestIpExt + IpExt>() {
        let mut ctx = FakeIcmpCtx::<I>::default();
        let mut api = IcmpEchoSocketApi::<I, _>::new(ctx.as_mut());
        let conn = api.create();
        api.connect(&conn, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), REMOTE_ID).unwrap();

        let pb = IcmpPacketBuilder::<I, _>::new(
            I::TEST_ADDRS.local_ip.get(),
            I::TEST_ADDRS.remote_ip.get(),
            IcmpZeroCode,
            packet_formats::icmp::IcmpEchoReply::new(0, 1),
        );
        let buf = pb
            .wrap_body(Buf::new(Vec::new(), ..))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .into_inner();
        assert_matches!(
            api.send(&conn, buf, FakeSendToken::default()),
            Err(datagram::SendError::SerializeError(
                packet_formats::error::ParseError::NotExpected
            ))
        );
    }

    #[ip_test(I)]
    fn get_info<I: TestIpExt + IpExt>() {
        let mut ctx = FakeIcmpCtx::<I>::default();
        let mut api = IcmpEchoSocketApi::<I, _>::new(ctx.as_mut());

        let id = api.create();
        assert_eq!(api.get_info(&id), datagram::SocketInfo::Unbound);

        api.bind(&id, None, Some(ICMP_ID)).unwrap();
        assert_eq!(
            api.get_info(&id),
            datagram::SocketInfo::Listener(datagram::ListenerInfo {
                local_ip: None,
                local_identifier: ICMP_ID
            })
        );

        api.connect(&id, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), REMOTE_ID).unwrap();
        assert_eq!(
            api.get_info(&id),
            datagram::SocketInfo::Connected(datagram::ConnInfo {
                local_ip: StrictlyZonedAddr::new_unzoned_or_panic(I::TEST_ADDRS.local_ip),
                local_identifier: ICMP_ID,
                remote_ip: StrictlyZonedAddr::new_unzoned_or_panic(I::TEST_ADDRS.remote_ip),
                remote_identifier: REMOTE_ID,
            })
        );
    }

    #[ip_test(I)]
    fn send<I: TestIpExt + IpExt>() {
        let mut ctx = FakeIcmpCtx::<I>::default();
        let mut api = IcmpEchoSocketApi::<I, _>::new(ctx.as_mut());
        let sock = api.create();

        api.bind(&sock, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), Some(ICMP_ID)).unwrap();
        api.connect(&sock, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), REMOTE_ID).unwrap();

        let packet = Buf::new([1u8, 2, 3, 4], ..)
            .wrap_in(IcmpPacketBuilder::<I, _>::new(
                I::UNSPECIFIED_ADDRESS,
                I::UNSPECIFIED_ADDRESS,
                IcmpZeroCode,
                // Use 0 here to show that this is filled by the API.
                IcmpEchoRequest::new(0, SEQ_NUM),
            ))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .unwrap_b();
        api.send(&sock, Buf::new(packet, ..), FakeSendToken::default()).unwrap();
        let frames = ctx.core_ctx.frames.take_frames();
        let (SendIpPacketMeta { device: _, src_ip, dst_ip, .. }, body) =
            assert_matches!(&frames[..], [f] => f);
        assert_eq!(dst_ip, &I::TEST_ADDRS.remote_ip);

        let mut body = &body[..];
        let echo_req: IcmpPacket<I, _, IcmpEchoRequest> =
            body.parse_with(IcmpParseArgs::new(src_ip.get(), dst_ip.get())).unwrap();
        assert_eq!(echo_req.message().id(), ICMP_ID.get());
        assert_eq!(echo_req.message().seq(), SEQ_NUM);
    }

    #[ip_test(I)]
    fn receive<I: TestIpExt + IpExt>() {
        let mut ctx = FakeIcmpCtx::<I>::default();
        let mut api = IcmpEchoSocketApi::<I, _>::new(ctx.as_mut());
        let sock = api.create();

        api.bind(&sock, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), Some(ICMP_ID)).unwrap();

        let reply = IcmpPacketBuilder::<I, _>::new(
            // Use whatever here this is not validated by this module.
            I::UNSPECIFIED_ADDRESS,
            I::UNSPECIFIED_ADDRESS,
            IcmpZeroCode,
            IcmpEchoReply::new(ICMP_ID.get(), SEQ_NUM),
        )
        .wrap_body(Buf::new([1u8, 2, 3, 4], ..))
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap();

        let CtxPair { core_ctx, bindings_ctx } = &mut ctx;
        let src_ip = I::TEST_ADDRS.remote_ip;
        let dst_ip = I::TEST_ADDRS.local_ip;
        <IcmpEchoIpTransportContext as IpTransportContext<I, _, _>>::receive_ip_packet(
            core_ctx,
            bindings_ctx,
            &FakeDeviceId,
            I::RecvSrcAddr::new(src_ip.get()).unwrap(),
            dst_ip,
            reply.clone(),
            &mut LocalDeliveryPacketInfo::default(),
            None,
        )
        .unwrap();

        let received = core::mem::take(&mut bindings_ctx.state.received);
        let ReceivedEchoPacket {
            src_ip: got_src_ip,
            dst_ip: got_dst_ip,
            socket: got_socket,
            id: got_id,
            data: got_data,
        } = assert_matches!(&received[..], [f] => f);
        assert_eq!(got_src_ip, &src_ip.get());
        assert_eq!(got_dst_ip, &dst_ip.get());
        assert_eq!(got_socket, &sock);
        assert_eq!(got_id, &ICMP_ID.get());
        assert_eq!(&got_data[..], reply.as_ref());
    }

    #[ip_test(I)]
    fn receive_no_socket<I: TestIpExt + IpExt>() {
        let mut ctx = FakeIcmpCtx::<I>::default();
        let mut api = IcmpEchoSocketApi::<I, _>::new(ctx.as_mut());
        let sock = api.create();

        const BIND_ICMP_ID: NonZeroU16 = NonZeroU16::new(10).unwrap();
        const OTHER_ICMP_ID: NonZeroU16 = NonZeroU16::new(16).unwrap();

        api.bind(&sock, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), Some(BIND_ICMP_ID))
            .unwrap();

        let reply = IcmpPacketBuilder::<I, _>::new(
            // Use whatever here this is not validated by this module.
            I::UNSPECIFIED_ADDRESS,
            I::UNSPECIFIED_ADDRESS,
            IcmpZeroCode,
            IcmpEchoReply::new(OTHER_ICMP_ID.get(), SEQ_NUM),
        )
        .wrap_body(EmptyBuf)
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap();

        let CtxPair { core_ctx, bindings_ctx } = &mut ctx;
        <IcmpEchoIpTransportContext as IpTransportContext<I, _, _>>::receive_ip_packet(
            core_ctx,
            bindings_ctx,
            &FakeDeviceId,
            I::RecvSrcAddr::new(I::TEST_ADDRS.remote_ip.get()).unwrap(),
            I::TEST_ADDRS.local_ip,
            reply,
            &mut LocalDeliveryPacketInfo::default(),
            None,
        )
        .unwrap();
        assert_matches!(&bindings_ctx.state.received[..], []);
    }

    #[ip_test(I)]
    fn receive_queue_full<I: TestIpExt + IpExt>() {
        let mut ctx = FakeIcmpCtx::<I>::default();
        let mut api = IcmpEchoSocketApi::<I, _>::new(ctx.as_mut());
        let sock = api.create();

        api.bind(&sock, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), Some(ICMP_ID)).unwrap();

        // Simulate a full RX queue.
        let CtxPair { core_ctx, bindings_ctx } = &mut ctx;
        bindings_ctx.state.max_size = 0;

        let reply = IcmpPacketBuilder::<I, _>::new(
            // Use whatever here this is not validated by this module.
            I::UNSPECIFIED_ADDRESS,
            I::UNSPECIFIED_ADDRESS,
            IcmpZeroCode,
            IcmpEchoReply::new(ICMP_ID.get(), SEQ_NUM),
        )
        .wrap_body(Buf::new([1u8, 2, 3, 4], ..))
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap();

        let src_ip = I::TEST_ADDRS.remote_ip;
        let dst_ip = I::TEST_ADDRS.local_ip;
        <IcmpEchoIpTransportContext as IpTransportContext<I, _, _>>::receive_ip_packet(
            core_ctx,
            bindings_ctx,
            &FakeDeviceId,
            I::RecvSrcAddr::new(src_ip.get()).unwrap(),
            dst_ip,
            reply,
            &mut LocalDeliveryPacketInfo::default(),
            None,
        )
        .unwrap();

        assert_eq!(core_ctx.counters().queue_full.get(), 1);
    }

    #[ip_test(I, test = false)]
    #[test_case::test_matrix(
        [MarkDomain::Mark1, MarkDomain::Mark2],
        [None, Some(0), Some(1)]
    )]
    fn icmp_socket_marks<I: TestIpExt + IpExt>(domain: MarkDomain, mark: Option<u32>) {
        let mut ctx = FakeIcmpCtx::<I>::default();
        let mut api = IcmpEchoSocketApi::<I, _>::new(ctx.as_mut());
        let socket = api.create();

        // Doesn't have a mark by default.
        assert_eq!(api.get_mark(&socket, domain), Mark(None));

        let mark = Mark(mark);
        // We can set and get back the mark.
        api.set_mark(&socket, domain, mark);
        assert_eq!(api.get_mark(&socket, domain), mark);
    }

    /// A test for the crash seen in https://fxbug.dev/445389479.
    ///
    /// Because the netstack assumes it could never send an ICMP echo request
    /// with an ID of 0, that it would never see an echo request inside an ICMP
    /// error with an ID of 0.
    #[ip_test(I)]
    fn icmp_error_with_inner_icmp_echo_with_id_0<I: TestIpExt + IpExt>() {
        let mut ctx = FakeIcmpCtx::<I>::default();

        let src_ip = I::TEST_ADDRS.remote_ip;
        let dst_ip = I::TEST_ADDRS.local_ip;

        let original_body = Buf::new([1, 2, 3, 4], ..)
            .wrap_in(IcmpPacketBuilder::<I, _>::new(
                *src_ip,
                *dst_ip,
                IcmpZeroCode,
                // The zero below is sorta invalid because we'd never send an
                // ICMP echo with an ID of 0.
                IcmpEchoRequest::new(0, 1),
            ))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .unwrap_b();

        let CtxPair { core_ctx, bindings_ctx } = &mut ctx;
        <IcmpEchoIpTransportContext as IpTransportContext<I, _, _>>::receive_icmp_error(
            core_ctx,
            bindings_ctx,
            &FakeDeviceId,
            Some(src_ip),
            dst_ip,
            original_body.as_ref(),
            I::map_ip_out(
                (),
                |()| {
                    Icmpv4ErrorCode::DestUnreachable(
                        Icmpv4DestUnreachableCode::DestNetworkUnreachable,
                        IcmpDestUnreachable::default(),
                    )
                },
                |()| Icmpv6ErrorCode::DestUnreachable(Icmpv6DestUnreachableCode::AddrUnreachable),
            ),
        )
    }
}
