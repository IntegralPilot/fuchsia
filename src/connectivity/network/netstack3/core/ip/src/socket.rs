// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! IPv4 and IPv6 sockets.

use core::cmp::Ordering;
use core::convert::Infallible;
use core::num::NonZeroU8;

use log::{debug, error};
use net_types::ip::{Ip, IpVersionMarker, Ipv6Addr, Mtu};
use net_types::{MulticastAddress, ScopeableAddress, SpecifiedAddr, Witness as _};
use netstack3_base::socket::{SocketIpAddr, SocketIpAddrExt as _};
use netstack3_base::{
    AnyDevice, CounterContext, DeviceIdContext, DeviceIdentifier, EitherDeviceId, InstantContext,
    InterfaceProperties, IpDeviceAddr, IpExt, Marks, Mms, NetworkSerializationContext,
    SendFrameErrorReason, StrongDeviceIdentifier, TxMetadata, TxMetadataBindingsTypes,
    WeakDeviceIdentifier,
};
use netstack3_filter::{
    self as filter, DynTransportSerializer, DynamicTransportSerializer, FilterBindingsContext,
    FilterHandler as _, FilterIpExt, RawIpBody, SocketEgressFilterResult, SocketOpsFilter,
    SocketOpsFilterBindingContext, TransportPacketSerializer,
};
use netstack3_trace::trace_duration;
use packet::{
    BufferMut, NestablePacketBuilder as _, PacketConstraints, SerializeError, Serializer,
};
use packet_formats::ip::{DscpAndEcn, IpPacketBuilder as _};
use thiserror::Error;

use crate::icmp::IcmpErrorHandler;
use crate::internal::base::{
    FilterHandlerProvider, IpDeviceMtuContext, IpLayerIpExt, IpLayerPacketMetadata,
    IpPacketDestination, IpSendFrameError, IpSendFrameErrorReason, ResolveRouteError,
    SendIpPacketMeta, reject_type_to_icmpv4_error, reject_type_to_icmpv6_error,
};
use crate::internal::counters::IpCounters;
use crate::internal::device::state::IpDeviceStateIpExt;
use crate::internal::routing::PacketOrigin;
use crate::internal::routing::rules::RuleInput;
use crate::internal::types::{InternalForwarding, ResolvedRoute, RoutableIpAddr};
use crate::{HopLimits, NextHop};

/// The arguments used for creating an [`IpSock`]
pub struct IpSocketArgs<'a, D: StrongDeviceIdentifier, I: IpExt, O> {
    /// The device the socket is bound to.
    pub device: Option<EitherDeviceId<&'a D, &'a D::Weak>>,
    /// The local IP to use for the connection. One is selected if not provided
    /// based on the output route.
    pub local_ip: Option<IpDeviceAddr<I::Addr>>,
    /// The remote IP address for this connection.
    pub remote_ip: RoutableIpAddr<I::Addr>,
    /// The IP protocol in use.
    pub proto: I::Proto,
    /// Additional IP layer options.
    pub options: &'a O,
}
/// An execution context defining a type of IP socket.
pub trait IpSocketHandler<I: IpExt + FilterIpExt, BC: TxMetadataBindingsTypes>:
    DeviceIdContext<AnyDevice>
{
    /// Constructs a new [`IpSock`].
    ///
    /// `new_ip_socket` constructs a new `IpSock` to the given remote IP
    /// address from the given local IP address with the given IP protocol. If
    /// no local IP address is given, one will be chosen automatically. If
    /// `device` is `Some`, the socket will be bound to the given device - only
    /// routes which egress over the device will be used. If no route is
    /// available which egresses over the device - even if routes are available
    /// which egress over other devices - the socket will be considered
    /// unroutable.
    ///
    /// `new_ip_socket` returns an error if no route to the remote was found in
    /// the forwarding table or if the given local IP address is not valid for
    /// the found route.
    fn new_ip_socket<O>(
        &mut self,
        bindings_ctx: &mut BC,
        args: IpSocketArgs<'_, Self::DeviceId, I, O>,
    ) -> Result<IpSock<I, Self::WeakDeviceId>, IpSockCreationError>
    where
        O: RouteResolutionOptions<I>;

    /// Sends an IP packet on a socket.
    ///
    /// The generated packet has its metadata initialized from `socket`,
    /// including the source and destination addresses, the Time To Live/Hop
    /// Limit, and the Protocol/Next Header. The outbound device is also chosen
    /// based on information stored in the socket.
    ///
    /// `mtu` may be used to optionally impose an MTU on the outgoing packet.
    /// Note that the device's MTU will still be imposed on the packet. That is,
    /// the smaller of `mtu` and the device's MTU will be imposed on the packet.
    ///
    /// If the socket is currently unroutable, an error is returned.
    fn send_ip_packet<S, O>(
        &mut self,
        bindings_ctx: &mut BC,
        socket: &IpSock<I, Self::WeakDeviceId>,
        body: S,
        options: &O,
        tx_metadata: BC::TxMetadata,
    ) -> Result<(), IpSockSendError>
    where
        S: TransportPacketSerializer<I>,
        S::Buffer: BufferMut,
        O: SendOptions<I> + RouteResolutionOptions<I>;

    /// Confirms the provided IP socket destination is reachable.
    ///
    /// Implementations must retrieve the next hop given the provided
    /// IP socket and confirm neighbor reachability for the resolved target
    /// device.
    fn confirm_reachable<O>(
        &mut self,
        bindings_ctx: &mut BC,
        socket: &IpSock<I, Self::WeakDeviceId>,
        options: &O,
    ) where
        O: RouteResolutionOptions<I>;

    /// Creates a temporary IP socket and sends a single packet on it.
    ///
    /// `local_ip`, `remote_ip`, `proto`, and `options` are passed directly to
    /// [`IpSocketHandler::new_ip_socket`]. `get_body_from_src_ip` is given the
    /// source IP address for the packet - which may have been chosen
    /// automatically if `local_ip` is `None` - and returns the body to be
    /// encapsulated. This is provided in case the body's contents depend on the
    /// chosen source IP address.
    ///
    /// If `device` is specified, the available routes are limited to those that
    /// egress over the device.
    ///
    /// `mtu` may be used to optionally impose an MTU on the outgoing packet.
    /// Note that the device's MTU will still be imposed on the packet. That is,
    /// the smaller of `mtu` and the device's MTU will be imposed on the packet.
    ///
    /// # Errors
    ///
    /// If an error is encountered while constructing the temporary IP socket
    /// or sending the packet, `options` will be returned along with the
    /// error. `get_body_from_src_ip` is fallible, and if there's an error,
    /// it will be returned as well.
    fn send_oneshot_ip_packet_with_fallible_serializer<S, E, F, O>(
        &mut self,
        bindings_ctx: &mut BC,
        args: IpSocketArgs<'_, Self::DeviceId, I, O>,
        tx_metadata: BC::TxMetadata,
        get_body_from_src_ip: F,
    ) -> Result<(), SendOneShotIpPacketError<E>>
    where
        S: TransportPacketSerializer<I>,
        S::Buffer: BufferMut,
        F: FnOnce(IpDeviceAddr<I::Addr>) -> Result<S, E>,
        O: SendOptions<I> + RouteResolutionOptions<I>,
    {
        let options = args.options;
        let tmp = self
            .new_ip_socket(bindings_ctx, args)
            .map_err(|err| SendOneShotIpPacketError::CreateAndSendError { err: err.into() })?;
        let packet = get_body_from_src_ip(*tmp.local_ip())
            .map_err(SendOneShotIpPacketError::SerializeError)?;
        self.send_ip_packet(bindings_ctx, &tmp, packet, options, tx_metadata)
            .map_err(|err| SendOneShotIpPacketError::CreateAndSendError { err: err.into() })
    }

    /// Like `send_oneshot_ip_packet_with_fallible_serializer`, but a dynamic
    /// transport serializer is used.
    ///
    /// This reduces code generation cost at the expense of some runtime
    /// overhead.
    fn send_oneshot_ip_packet_with_dyn_fallible_serializer<S, E, F, O>(
        &mut self,
        bindings_ctx: &mut BC,
        args: IpSocketArgs<'_, Self::DeviceId, I, O>,
        tx_metadata: BC::TxMetadata,
        get_body_from_src_ip: F,
    ) -> Result<(), SendOneShotIpPacketError<E>>
    where
        S: DynamicTransportSerializer<I>,
        F: FnOnce(IpDeviceAddr<I::Addr>) -> Result<S, E>,
        O: SendOptions<I> + RouteResolutionOptions<I>,
    {
        let options = args.options;
        let tmp = self
            .new_ip_socket(bindings_ctx, args)
            .map_err(|err| SendOneShotIpPacketError::CreateAndSendError { err: err.into() })?;
        let mut packet = get_body_from_src_ip(*tmp.local_ip())
            .map_err(SendOneShotIpPacketError::SerializeError)?;
        self.send_ip_packet(
            bindings_ctx,
            &tmp,
            DynTransportSerializer::new(&mut packet),
            options,
            tx_metadata,
        )
        .map_err(|err| SendOneShotIpPacketError::CreateAndSendError { err: err.into() })
    }

    /// Sends a one-shot IP packet but with a non-fallible serializer.
    fn send_oneshot_ip_packet<S, F, O>(
        &mut self,
        bindings_ctx: &mut BC,
        args: IpSocketArgs<'_, Self::DeviceId, I, O>,
        tx_metadata: BC::TxMetadata,
        get_body_from_src_ip: F,
    ) -> Result<(), IpSockCreateAndSendError>
    where
        S: TransportPacketSerializer<I>,
        S::Buffer: BufferMut,
        F: FnOnce(IpDeviceAddr<I::Addr>) -> S,
        O: SendOptions<I> + RouteResolutionOptions<I>,
    {
        self.send_oneshot_ip_packet_with_fallible_serializer(
            bindings_ctx,
            args,
            tx_metadata,
            |ip| Ok::<_, Infallible>(get_body_from_src_ip(ip)),
        )
        .map_err(|err| match err {
            SendOneShotIpPacketError::CreateAndSendError { err } => err,
        })
    }

    /// Like `send_oneshot_ip_packet`, but a dynamic transport serializer is
    /// used.
    ///
    /// This reduces code generation cost at the expense of some runtime
    /// overhead.
    fn send_oneshot_ip_packet_with_dyn_serializer<S, F, O>(
        &mut self,
        bindings_ctx: &mut BC,
        args: IpSocketArgs<'_, Self::DeviceId, I, O>,
        tx_metadata: BC::TxMetadata,
        get_body_from_src_ip: F,
    ) -> Result<(), IpSockCreateAndSendError>
    where
        S: DynamicTransportSerializer<I>,
        F: FnOnce(IpDeviceAddr<I::Addr>) -> S,
        O: SendOptions<I> + RouteResolutionOptions<I>,
    {
        self.send_oneshot_ip_packet_with_dyn_fallible_serializer(
            bindings_ctx,
            args,
            tx_metadata,
            |ip| Ok::<_, Infallible>(get_body_from_src_ip(ip)),
        )
        .map_err(|err| match err {
            SendOneShotIpPacketError::CreateAndSendError { err } => err,
        })
    }
}

/// An error in sending a packet on an IP socket.
#[derive(Error, Copy, Clone, Debug, Eq, PartialEq)]
pub enum IpSockSendError {
    /// An MTU was exceeded.
    ///
    /// This could be caused by an MTU at any layer of the stack, including both
    /// device MTUs and packet format body size limits.
    #[error("a maximum transmission unit (MTU) was exceeded")]
    Mtu,
    /// The socket is currently unroutable.
    #[error("the socket is currently unroutable: {0}")]
    Unroutable(#[from] ResolveRouteError),
    /// The socket operation would've resulted in illegal loopback addresses on
    /// a non-loopback device.
    #[error("illegal loopback address")]
    IllegalLoopbackAddress,
    /// Broadcast send is not allowed.
    #[error("broadcast send is not enabled for the socket")]
    BroadcastNotAllowed,
}

impl From<SerializeError<Infallible>> for IpSockSendError {
    fn from(err: SerializeError<Infallible>) -> IpSockSendError {
        match err {
            SerializeError::SizeLimitExceeded => IpSockSendError::Mtu,
        }
    }
}

impl IpSockSendError {
    /// Constructs a `Result` from an [`IpSendFrameErrorReason`] with
    /// application-visible [`IpSockSendError`]s in the `Err` variant.
    ///
    /// Errors that are not bubbled up to applications are dropped.
    fn from_ip_send_frame(e: IpSendFrameErrorReason) -> Result<(), Self> {
        match e {
            IpSendFrameErrorReason::Device(d) => Self::from_send_frame(d),
            IpSendFrameErrorReason::IllegalLoopbackAddress => Err(Self::IllegalLoopbackAddress),
        }
    }

    /// Constructs a `Result` from a [`SendFrameErrorReason`] with
    /// application-visible [`IpSockSendError`]s in the `Err` variant.
    ///
    /// Errors that are not bubbled up to applications are dropped.
    fn from_send_frame(e: SendFrameErrorReason) -> Result<(), Self> {
        match e {
            SendFrameErrorReason::Alloc
            | SendFrameErrorReason::QueueFull
            | SendFrameErrorReason::AddressResolutionFailed => Ok(()),
            SendFrameErrorReason::SizeConstraintsViolation => Err(Self::Mtu),
        }
    }
}

/// An error in sending a packet on a temporary IP socket.
#[derive(Error, Copy, Clone, Debug)]
pub enum IpSockCreateAndSendError {
    /// Cannot send via temporary socket.
    #[error("cannot send via temporary socket: {0}")]
    Send(#[from] IpSockSendError),
    /// The temporary socket could not be created.
    #[error("the temporary socket could not be created: {0}")]
    Create(#[from] IpSockCreationError),
}

/// The error returned by
/// [`IpSocketHandler::send_oneshot_ip_packet_with_fallible_serializer`].
#[derive(Debug)]
#[allow(missing_docs)]
pub enum SendOneShotIpPacketError<E> {
    CreateAndSendError { err: IpSockCreateAndSendError },
    SerializeError(E),
}

/// Possible errors when retrieving the maximum transport message size.
#[derive(Error, Copy, Clone, Debug, Eq, PartialEq)]
pub enum MmsError {
    /// Cannot find the device that is used for the ip socket, possibly because
    /// there is no route.
    #[error("cannot find the device: {0}")]
    NoDevice(#[from] ResolveRouteError),
    /// The MTU provided by the device is too small such that there is no room
    /// for a transport message at all.
    #[error("invalid MTU: {0:?}")]
    MTUTooSmall(Mtu),
}

/// Gets device related information of an IP socket.
pub trait DeviceIpSocketHandler<I: IpExt, BC>: DeviceIdContext<AnyDevice> {
    /// Gets the maximum message size for the transport layer, it equals the
    /// device MTU minus the IP header size.
    ///
    /// This corresponds to the GET_MAXSIZES call described in:
    /// https://www.rfc-editor.org/rfc/rfc1122#section-3.4
    fn get_mms<O: RouteResolutionOptions<I>>(
        &mut self,
        bindings_ctx: &mut BC,
        ip_sock: &IpSock<I, Self::WeakDeviceId>,
        options: &O,
    ) -> Result<Mms, MmsError>;
}

/// An error encountered when creating an IP socket.
#[derive(Error, Copy, Clone, Debug, Eq, PartialEq)]
pub enum IpSockCreationError {
    /// An error occurred while looking up a route.
    #[error("a route cannot be determined: {0}")]
    Route(#[from] ResolveRouteError),
}

/// An IP socket.
#[derive(Clone, Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub struct IpSock<I: IpExt, D> {
    /// The definition of the socket.
    ///
    /// This does not change for the lifetime of the socket.
    definition: IpSockDefinition<I, D>,
}

impl<I: IpExt, D> IpSock<I, D> {
    /// Returns the socket's definition.
    #[cfg(any(test, feature = "testutils"))]
    pub fn definition(&self) -> &IpSockDefinition<I, D> {
        &self.definition
    }
}

/// The definition of an IP socket.
///
/// These values are part of the socket's definition, and never change.
#[derive(Clone, Debug, PartialEq)]
pub struct IpSockDefinition<I: IpExt, D> {
    /// The socket's remote address.
    pub remote_ip: SocketIpAddr<I::Addr>,
    /// The socket's local address.
    ///
    /// Guaranteed to be unicast in its subnet since it's always equal to an
    /// address assigned to the local device. We can't use the `UnicastAddr`
    /// witness type since `Ipv4Addr` doesn't implement `UnicastAddress`.
    //
    // TODO(joshlf): Support unnumbered interfaces. Once we do that, a few
    // issues arise: A) Does the unicast restriction still apply, and is that
    // even well-defined for IPv4 in the absence of a subnet? B) Presumably we
    // have to always bind to a particular interface?
    pub local_ip: IpDeviceAddr<I::Addr>,
    /// The socket's bound output device.
    pub device: Option<D>,
    /// The IP protocol the socket is bound to.
    pub proto: I::Proto,
}

impl<I: IpExt, D> IpSock<I, D> {
    /// Returns the socket's local IP address.
    pub fn local_ip(&self) -> &IpDeviceAddr<I::Addr> {
        &self.definition.local_ip
    }
    /// Returns the socket's remote IP address.
    pub fn remote_ip(&self) -> &SocketIpAddr<I::Addr> {
        &self.definition.remote_ip
    }
    /// Returns the selected output interface for the socket, if any.
    pub fn device(&self) -> Option<&D> {
        self.definition.device.as_ref()
    }
    /// Returns the socket's protocol.
    pub fn proto(&self) -> I::Proto {
        self.definition.proto
    }
}

// TODO(joshlf): Once we support configuring transport-layer protocols using
// type parameters, use that to ensure that `proto` is the right protocol for
// the caller. We will still need to have a separate enforcement mechanism for
// raw IP sockets once we support those.

/// The bindings execution context for IP sockets.
pub trait IpSocketBindingsContext<D>:
    InstantContext
    + FilterBindingsContext<D>
    + TxMetadataBindingsTypes
    + SocketOpsFilterBindingContext<D>
{
}
impl<
    D,
    BC: InstantContext
        + FilterBindingsContext<D>
        + TxMetadataBindingsTypes
        + SocketOpsFilterBindingContext<D>,
> IpSocketBindingsContext<D> for BC
{
}

/// The context required in order to implement [`IpSocketHandler`].
///
/// Blanket impls of `IpSocketHandler` are provided in terms of
/// `IpSocketContext`.
pub trait IpSocketContext<I, BC>:
    DeviceIdContext<AnyDevice, DeviceId: InterfaceProperties<BC::DeviceClass>>
    + FilterHandlerProvider<I, BC>
    + IcmpErrorHandler<I, BC>
where
    I: IpLayerIpExt,
    BC: IpSocketBindingsContext<Self::DeviceId>,
{
    /// Returns a route for a socket.
    ///
    /// If `device` is specified, the available routes are limited to those that
    /// egress over the device.
    fn lookup_route(
        &mut self,
        bindings_ctx: &mut BC,
        device: Option<&Self::DeviceId>,
        src_ip: Option<IpDeviceAddr<I::Addr>>,
        dst_ip: RoutableIpAddr<I::Addr>,
        transparent: bool,
        marks: &Marks,
    ) -> Result<ResolvedRoute<I, Self::DeviceId>, ResolveRouteError>;

    /// Send an IP packet to the next-hop node.
    fn send_ip_packet<S>(
        &mut self,
        bindings_ctx: &mut BC,
        meta: SendIpPacketMeta<I, &Self::DeviceId, SpecifiedAddr<I::Addr>>,
        body: S,
        packet_metadata: IpLayerPacketMetadata<I, Self::WeakAddressId, BC>,
    ) -> Result<(), IpSendFrameError<S>>
    where
        S: TransportPacketSerializer<I>,
        S::Buffer: BufferMut;

    /// Returns `DeviceId` for the loopback device.
    fn get_loopback_device(&mut self) -> Option<Self::DeviceId>;

    /// Confirms the provided IP socket destination is reachable.
    ///
    /// Implementations must retrieve the next hop given the provided
    /// IP socket and confirm neighbor reachability for the resolved target
    /// device.
    fn confirm_reachable(
        &mut self,
        bindings_ctx: &mut BC,
        dst: SpecifiedAddr<I::Addr>,
        input: RuleInput<'_, I, Self::DeviceId>,
    );
}

/// Enables a blanket implementation of [`IpSocketHandler`].
///
/// Implementing this marker trait for a type enables a blanket implementation
/// of `IpSocketHandler` given the other requirements are met.
pub trait UseIpSocketHandlerBlanket {}

impl<I, BC, CC> IpSocketHandler<I, BC> for CC
where
    I: IpLayerIpExt + IpDeviceStateIpExt,
    BC: IpSocketBindingsContext<Self::DeviceId>,
    CC: IpSocketContext<I, BC> + CounterContext<IpCounters<I>> + UseIpSocketHandlerBlanket,
    CC::DeviceId: netstack3_base::InterfaceProperties<BC::DeviceClass>,
{
    fn new_ip_socket<O>(
        &mut self,
        bindings_ctx: &mut BC,
        args: IpSocketArgs<'_, Self::DeviceId, I, O>,
    ) -> Result<IpSock<I, CC::WeakDeviceId>, IpSockCreationError>
    where
        O: RouteResolutionOptions<I>,
    {
        let IpSocketArgs { device, local_ip, remote_ip, proto, options } = args;
        let device = device
            .as_ref()
            .map(|d| d.as_strong_ref().ok_or(ResolveRouteError::Unreachable))
            .transpose()?;
        let device = device.as_ref().map(|d| d.as_ref());

        // Make sure the remote is routable with a local address before creating
        // the socket. We do not care about the actual destination here because
        // we will recalculate it when we send a packet so that the best route
        // available at the time is used for each outgoing packet.
        let resolved_route = self.lookup_route(
            bindings_ctx,
            device,
            local_ip,
            remote_ip,
            options.transparent(),
            options.marks(),
        )?;
        Ok(new_ip_socket(device, resolved_route, remote_ip, proto))
    }

    fn send_ip_packet<S, O>(
        &mut self,
        bindings_ctx: &mut BC,
        ip_sock: &IpSock<I, CC::WeakDeviceId>,
        body: S,
        options: &O,
        tx_metadata: BC::TxMetadata,
    ) -> Result<(), IpSockSendError>
    where
        S: TransportPacketSerializer<I>,
        S::Buffer: BufferMut,
        O: SendOptions<I> + RouteResolutionOptions<I>,
    {
        send_ip_packet(self, bindings_ctx, ip_sock, body, options, tx_metadata)
    }

    fn confirm_reachable<O>(
        &mut self,
        bindings_ctx: &mut BC,
        socket: &IpSock<I, CC::WeakDeviceId>,
        options: &O,
    ) where
        O: RouteResolutionOptions<I>,
    {
        let bound_device = socket.device().and_then(|weak| weak.upgrade());
        let bound_device = bound_device.as_ref();
        let bound_address = Some((*socket.local_ip()).into());
        let destination = (*socket.remote_ip()).into();
        IpSocketContext::confirm_reachable(
            self,
            bindings_ctx,
            destination,
            RuleInput {
                packet_origin: PacketOrigin::Local { bound_address, bound_device },
                marks: options.marks(),
            },
        )
    }
}

/// Provides hooks for altering route resolution behavior of [`IpSock`].
///
/// Must be implemented by the socket option type of an `IpSock` when using it
/// to call [`IpSocketHandler::new_ip_socket`] or
/// [`IpSocketHandler::send_ip_packet`]. This is implemented as a trait instead
/// of an inherent impl on a type so that users of sockets that don't need
/// certain option types can avoid allocating space for those options.
// TODO(https://fxbug.dev/323389672): We need a mechanism to inform `IpSock` of
// changes in the route resolution options when it starts caching previously
// calculated routes. Any changes to the options here *MUST* cause the route to
// be re-calculated.
pub trait RouteResolutionOptions<I: Ip> {
    /// Whether the socket is transparent.
    ///
    /// This allows transparently proxying traffic to the socket, and allows the
    /// socket to be bound to a non-local address.
    fn transparent(&self) -> bool;

    /// Returns the marks carried by packets created on the socket.
    fn marks(&self) -> &Marks;
}

/// Provides hooks for altering sending behavior of [`IpSock`].
///
/// Must be implemented by the socket option type of an `IpSock` when using it
/// to call [`IpSocketHandler::send_ip_packet`]. This is implemented as a trait
/// instead of an inherent impl on a type so that users of sockets that don't
/// need certain option types, like TCP for anything multicast-related, can
/// avoid allocating space for those options.
pub trait SendOptions<I: IpExt> {
    /// Returns the hop limit to set on a packet going to the given destination.
    ///
    /// If `Some(u)`, `u` will be used as the hop limit (IPv6) or TTL (IPv4) for
    /// a packet going to the given destination. Otherwise the default value
    /// will be used.
    fn hop_limit(&self, destination: &SpecifiedAddr<I::Addr>) -> Option<NonZeroU8>;

    /// Returns true if outgoing multicast packets should be looped back and
    /// delivered to local receivers who joined the multicast group.
    fn multicast_loop(&self) -> bool;

    /// `Some` if the socket can be used to send broadcast packets.
    fn allow_broadcast(&self) -> Option<I::BroadcastMarker>;

    /// Returns TCLASS/TOS field value that should be set in IP headers.
    fn dscp_and_ecn(&self) -> DscpAndEcn;

    /// The IP MTU to use for this transmission.
    ///
    /// Note that the minimum overall MTU is used considering the device and
    /// path. This option can be used to restrict an MTU to an upper bound.
    fn mtu(&self) -> Mtu;
}

/// Empty send and creation options that never overrides default values.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub struct DefaultIpSocketOptions;

impl<I: IpExt> SendOptions<I> for DefaultIpSocketOptions {
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

impl<I: Ip> RouteResolutionOptions<I> for DefaultIpSocketOptions {
    fn transparent(&self) -> bool {
        false
    }

    fn marks(&self) -> &Marks {
        &Marks::UNMARKED
    }
}

/// A trait providing send options delegation to an inner type.
///
/// A blanket impl of [`SendOptions`] is provided to all implementers. This
/// trait has the same shape as `SendOptions` but all the methods provide
/// default implementations that delegate to the value returned by
/// `DelegatedSendOptions::Delegate`. For brevity, the default `delegate` is
/// [`DefaultIpSocketOptions`].
#[allow(missing_docs)]
pub trait DelegatedSendOptions<I: IpExt>: OptionDelegationMarker {
    /// Returns the delegate providing the impl for all default methods.
    fn delegate(&self) -> &impl SendOptions<I> {
        &DefaultIpSocketOptions
    }

    fn hop_limit(&self, destination: &SpecifiedAddr<I::Addr>) -> Option<NonZeroU8> {
        self.delegate().hop_limit(destination)
    }

    fn multicast_loop(&self) -> bool {
        self.delegate().multicast_loop()
    }

    fn allow_broadcast(&self) -> Option<I::BroadcastMarker> {
        self.delegate().allow_broadcast()
    }

    fn dscp_and_ecn(&self) -> DscpAndEcn {
        self.delegate().dscp_and_ecn()
    }

    fn mtu(&self) -> Mtu {
        self.delegate().mtu()
    }
}

impl<O: DelegatedSendOptions<I> + OptionDelegationMarker, I: IpExt> SendOptions<I> for O {
    fn hop_limit(&self, destination: &SpecifiedAddr<I::Addr>) -> Option<NonZeroU8> {
        self.hop_limit(destination)
    }

    fn multicast_loop(&self) -> bool {
        self.multicast_loop()
    }

    fn allow_broadcast(&self) -> Option<I::BroadcastMarker> {
        self.allow_broadcast()
    }

    fn dscp_and_ecn(&self) -> DscpAndEcn {
        self.dscp_and_ecn()
    }

    fn mtu(&self) -> Mtu {
        self.mtu()
    }
}

/// A trait providing route resolution options delegation to an inner type.
///
/// A blanket impl of [`RouteResolutionOptions`] is provided to all
/// implementers. This trait has the same shape as `RouteResolutionOptions` but
/// all the methods provide default implementations that delegate to the value
/// returned by `DelegatedRouteResolutionOptions::Delegate`. For brevity, the
/// default `delegate` is [`DefaultIpSocketOptions`].
#[allow(missing_docs)]
pub trait DelegatedRouteResolutionOptions<I: Ip>: OptionDelegationMarker {
    /// Returns the delegate providing the impl for all default methods.
    fn delegate(&self) -> &impl RouteResolutionOptions<I> {
        &DefaultIpSocketOptions
    }

    fn transparent(&self) -> bool {
        self.delegate().transparent()
    }

    fn marks(&self) -> &Marks {
        self.delegate().marks()
    }
}

impl<O: DelegatedRouteResolutionOptions<I> + OptionDelegationMarker, I: IpExt>
    RouteResolutionOptions<I> for O
{
    fn transparent(&self) -> bool {
        self.transparent()
    }

    fn marks(&self) -> &Marks {
        self.marks()
    }
}

/// A marker trait to allow option delegation traits.
///
/// This trait sidesteps trait resolution rules around the delegation traits
/// because of the `Ip` parameter in them.
pub trait OptionDelegationMarker {}

/// The configurable hop limits for a socket.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub struct SocketHopLimits<I: Ip> {
    /// Unicast hop limit.
    pub unicast: Option<NonZeroU8>,
    /// Multicast hop limit.
    // TODO(https://fxbug.dev/42059735): Make this an Option<u8> to allow sending
    // multicast packets destined only for the local machine.
    pub multicast: Option<NonZeroU8>,
    /// An unused marker type signifying the IP version for which these hop
    /// limits are valid. Including this helps prevent using the wrong hop limits
    /// when operating on dualstack sockets.
    pub version: IpVersionMarker<I>,
}

impl<I: Ip> SocketHopLimits<I> {
    /// Returns a function that updates the unicast hop limit.
    pub fn set_unicast(value: Option<NonZeroU8>) -> impl FnOnce(&mut Self) {
        move |limits| limits.unicast = value
    }

    /// Returns a function that updates the multicast hop limit.
    pub fn set_multicast(value: Option<NonZeroU8>) -> impl FnOnce(&mut Self) {
        move |limits| limits.multicast = value
    }

    /// Returns the hop limits, or the provided defaults if unset.
    pub fn get_limits_with_defaults(&self, defaults: &HopLimits) -> HopLimits {
        let Self { unicast, multicast, version: _ } = self;
        HopLimits {
            unicast: unicast.unwrap_or(defaults.unicast),
            multicast: multicast.unwrap_or(defaults.multicast),
        }
    }

    /// Returns the appropriate hop limit to use for the given destination addr.
    pub fn hop_limit_for_dst(&self, destination: &SpecifiedAddr<I::Addr>) -> Option<NonZeroU8> {
        let Self { unicast, multicast, version: _ } = self;
        if destination.is_multicast() { *multicast } else { *unicast }
    }
}

fn new_ip_socket<I, D>(
    requested_device: Option<&D>,
    route: ResolvedRoute<I, D>,
    remote_ip: SocketIpAddr<I::Addr>,
    proto: I::Proto,
) -> IpSock<I, D::Weak>
where
    I: IpExt,
    D: StrongDeviceIdentifier,
{
    // TODO(https://fxbug.dev/323389672): Cache a reference to the route to
    // avoid the route lookup on send as long as the routing table hasn't
    // changed in between these operations.
    let ResolvedRoute {
        src_addr,
        device: route_device,
        local_delivery_device,
        next_hop: _,
        internal_forwarding: _,
    } = route;

    // If the source or destination address require a device, make sure to
    // set that in the socket definition. Otherwise defer to what was provided.
    let socket_device = (src_addr.as_ref().must_have_zone() || remote_ip.as_ref().must_have_zone())
        .then(|| {
            // NB: The route device might be loopback, and in such cases
            // we want to bind the socket to the device the source IP is
            // assigned to instead.
            local_delivery_device.unwrap_or(route_device)
        })
        .as_ref()
        .or(requested_device)
        .map(|d| d.downgrade());

    let definition =
        IpSockDefinition { local_ip: src_addr, remote_ip, device: socket_device, proto };
    IpSock { definition }
}

fn send_ip_packet<I, S, BC, CC, O>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    socket: &IpSock<I, CC::WeakDeviceId>,
    mut body: S,
    options: &O,
    tx_metadata: BC::TxMetadata,
) -> Result<(), IpSockSendError>
where
    I: IpLayerIpExt,
    S: TransportPacketSerializer<I>,
    S::Buffer: BufferMut,
    BC: IpSocketBindingsContext<CC::DeviceId>,
    CC: IpSocketContext<I, BC> + CounterContext<IpCounters<I>>,
    CC::DeviceId: netstack3_base::InterfaceProperties<BC::DeviceClass>,
    O: SendOptions<I> + RouteResolutionOptions<I>,
{
    trace_duration!("ip::send_packet");

    // Extracted to a function without the serializer parameter to ease code
    // generation.
    fn resolve<
        I: IpLayerIpExt,
        CC: IpSocketContext<I, BC>,
        BC: IpSocketBindingsContext<CC::DeviceId>,
    >(
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        device: &Option<CC::WeakDeviceId>,
        local_ip: IpDeviceAddr<I::Addr>,
        remote_ip: RoutableIpAddr<I::Addr>,
        transparent: bool,
        marks: &Marks,
    ) -> Result<ResolvedRoute<I, CC::DeviceId>, IpSockSendError> {
        let device = match device.as_ref().map(|d| d.upgrade()) {
            Some(Some(device)) => Some(device),
            Some(None) => return Err(ResolveRouteError::Unreachable.into()),
            None => None,
        };
        let route = core_ctx
            .lookup_route(
                bindings_ctx,
                device.as_ref(),
                Some(local_ip),
                remote_ip,
                transparent,
                marks,
            )
            .map_err(|e| IpSockSendError::Unroutable(e))?;
        assert_eq!(local_ip, route.src_addr);
        Ok(route)
    }

    let IpSock {
        definition: IpSockDefinition { remote_ip, local_ip, device: socket_device, proto },
    } = socket;
    let ResolvedRoute {
        src_addr: local_ip,
        device: mut egress_device,
        mut next_hop,
        mut local_delivery_device,
        mut internal_forwarding,
    } = resolve(
        core_ctx,
        bindings_ctx,
        socket_device,
        *local_ip,
        *remote_ip,
        options.transparent(),
        options.marks(),
    )?;

    if matches!(next_hop, NextHop::Broadcast(_)) && options.allow_broadcast().is_none() {
        return Err(IpSockSendError::BroadcastNotAllowed);
    }

    let previous_dst = remote_ip.addr();
    let mut packet = filter::TxPacket::new(local_ip.addr(), remote_ip.addr(), *proto, &mut body);
    let mut packet_metadata =
        IpLayerPacketMetadata::from_tx_metadata_and_marks(tx_metadata, *options.marks());

    let filter_result = core_ctx.filter_handler().local_egress_hook(
        bindings_ctx,
        &mut packet,
        &egress_device,
        &mut packet_metadata,
    );
    match filter_result {
        filter::Verdict::Stop(filter::DropOrReject::Drop) => {
            packet_metadata.acknowledge_drop();
            return Ok(());
        }
        filter::Verdict::Stop(filter::DropOrReject::Reject(reject_type)) => {
            packet_metadata.acknowledge_drop();

            let Some(icmp_error): Option<I::IcmpError> = I::map_ip_out(
                reject_type,
                |reject_type| reject_type_to_icmpv4_error(reject_type),
                |reject_type| reject_type_to_icmpv6_error(reject_type),
            ) else {
                debug!("Unsupported reject type: {:?}", reject_type);
                return Ok(());
            };

            let src_ip = SocketIpAddr::new_from_witness(local_ip.into_inner().get());
            let dst_ip = *remote_ip;
            let ttl = options.hop_limit(&dst_ip.into()).map(|v| v.into()).unwrap_or(1);
            let packet_builder = I::PacketBuilder::new(
                src_ip.into_inner().get(),
                dst_ip.into_inner().get(),
                ttl,
                *proto,
            );
            let header_len = packet_builder.constraints().header_len();
            let ip_frame = packet_builder.wrap_body(body);
            let packet = match ip_frame.serialize_outer(
                &mut NetworkSerializationContext::default(),
                packet::NoReuseBufferProvider(packet::new_buf_vec),
            ) {
                Ok(packet) => packet,
                Err((error, _frame)) => {
                    debug!("Failed to serialize packet {:?}", error);
                    return Ok(());
                }
            };

            // Invoke `send_icmp_error_message` with the `local_ip` as the
            // `original_source_ip`, which will result in the ICMP error
            // message getting sent back to the `socket`.
            core_ctx.send_icmp_error_message(
                bindings_ctx,
                /*device=*/ None,
                /*frame_dst=*/ None,
                src_ip,
                dst_ip,
                packet,
                icmp_error,
                header_len,
                *proto,
                &options.marks(),
            );

            return Ok(());
        }
        filter::Verdict::Proceed(filter::Accept) => {}
    }

    let Some(mut local_ip) = IpDeviceAddr::new(packet.src_addr()) else {
        packet_metadata.acknowledge_drop();
        return Err(IpSockSendError::Unroutable(ResolveRouteError::NoSrcAddr));
    };
    let Some(remote_ip) = RoutableIpAddr::new(packet.dst_addr()) else {
        packet_metadata.acknowledge_drop();
        return Err(IpSockSendError::Unroutable(ResolveRouteError::Unreachable));
    };

    // If the LOCAL_EGRESS hook ended up rewriting the packet's destination, perform
    // re-routing based on the new destination.
    if remote_ip.addr() != previous_dst {
        let ResolvedRoute {
            src_addr: new_local_ip,
            device: new_device,
            next_hop: new_next_hop,
            local_delivery_device: new_local_delivery_device,
            internal_forwarding: new_internal_forwarding,
        } = match resolve(
            core_ctx,
            bindings_ctx,
            socket_device,
            local_ip,
            remote_ip,
            options.transparent(),
            options.marks(),
        ) {
            Ok(r) => r,
            Err(err) => {
                packet_metadata.acknowledge_drop();
                return Err(err);
            }
        };
        local_ip = new_local_ip;
        egress_device = new_device;
        next_hop = new_next_hop;
        local_delivery_device = new_local_delivery_device;
        internal_forwarding = new_internal_forwarding;
    }

    // NB: Hit the forwarding hook if the route leverages internal forwarding.
    match internal_forwarding {
        InternalForwarding::Used(ingress_device) => {
            match core_ctx.filter_handler().forwarding_hook(
                &mut packet,
                &ingress_device,
                &egress_device,
                &mut packet_metadata,
            ) {
                filter::Verdict::Stop(filter::DropOrReject::Drop) => {
                    packet_metadata.acknowledge_drop();
                    return Ok(());
                }
                filter::Verdict::Stop(filter::DropOrReject::Reject(_reject_type)) => {
                    // TODO(https://fxbug.dev/466098884): Send reject packet.
                    packet_metadata.acknowledge_drop();
                    return Ok(());
                }
                filter::Verdict::Proceed(filter::Accept) => {}
            }
        }
        InternalForwarding::NotUsed => {}
    }

    if let Some(socket_info) = packet_metadata.tx_metadata().socket_info() {
        let egress_filter_result = bindings_ctx.socket_ops_filter().on_egress(
            &packet,
            &egress_device,
            socket_info,
            packet_metadata.marks(),
        );

        // TODO(https://fxbug.dev/412426836): Implement congestion signal handling.
        match egress_filter_result {
            SocketEgressFilterResult::Pass { congestion: _ } => (),
            SocketEgressFilterResult::Drop { congestion: _ } => {
                core_ctx.counters().socket_egress_filter_dropped.increment();
                packet_metadata.acknowledge_drop();
                return Ok(());
            }
        }
    }

    // The packet needs to be delivered locally if it's sent to a broadcast
    // or multicast address. For multicast packets this feature can be disabled
    // with IP_MULTICAST_LOOP.

    let loopback_packet = (!egress_device.is_loopback()
        && ((options.multicast_loop() && remote_ip.addr().is_multicast())
            || next_hop.is_broadcast()))
    .then(|| {
        body.serialize_new_buf(
            &mut NetworkSerializationContext::default(),
            PacketConstraints::UNCONSTRAINED,
            packet::new_buf_vec,
        )
    })
    .transpose()?
    .map(|buf| RawIpBody::new(*proto, local_ip.addr(), remote_ip.addr(), buf));

    let destination = match &local_delivery_device {
        Some(d) => IpPacketDestination::Loopback(d),
        None => IpPacketDestination::from_next_hop(next_hop, remote_ip.into()),
    };
    let ttl = options.hop_limit(&remote_ip.into());
    let meta = SendIpPacketMeta {
        device: &egress_device,
        src_ip: local_ip.into(),
        dst_ip: remote_ip.into(),
        destination,
        ttl,
        proto: *proto,
        mtu: options.mtu(),
        dscp_and_ecn: options.dscp_and_ecn(),
    };
    IpSocketContext::send_ip_packet(core_ctx, bindings_ctx, meta, body, packet_metadata).or_else(
        |IpSendFrameError { serializer: _, error }| IpSockSendError::from_ip_send_frame(error),
    )?;

    match (loopback_packet, core_ctx.get_loopback_device()) {
        (Some(loopback_packet), Some(loopback_device)) => {
            let meta = SendIpPacketMeta {
                device: &loopback_device,
                src_ip: local_ip.into(),
                dst_ip: remote_ip.into(),
                destination: IpPacketDestination::Loopback(&egress_device),
                ttl,
                proto: *proto,
                mtu: options.mtu(),
                dscp_and_ecn: options.dscp_and_ecn(),
            };
            let packet_metadata = IpLayerPacketMetadata::default();

            // The loopback packet will hit the egress hook. LOCAL_EGRESS hook
            // is not called again.
            IpSocketContext::send_ip_packet(
                core_ctx,
                bindings_ctx,
                meta,
                loopback_packet,
                packet_metadata,
            )
            .unwrap_or_else(|IpSendFrameError { serializer: _, error }| {
                error!("failed to send loopback packet: {error:?}")
            });
        }
        (Some(_loopback_packet), None) => {
            error!("can't send a loopback packet without the loopback device")
        }
        _ => (),
    }

    Ok(())
}

/// Enables a blanket implementation of [`DeviceIpSocketHandler`].
///
/// Implementing this marker trait for a type enables a blanket implementation
/// of `DeviceIpSocketHandler` given the other requirements are met.
pub trait UseDeviceIpSocketHandlerBlanket {}

impl<I, BC, CC> DeviceIpSocketHandler<I, BC> for CC
where
    I: IpLayerIpExt + IpDeviceStateIpExt,
    BC: IpSocketBindingsContext<CC::DeviceId>,
    CC: IpDeviceMtuContext<I> + IpSocketContext<I, BC> + UseDeviceIpSocketHandlerBlanket,
{
    fn get_mms<O: RouteResolutionOptions<I>>(
        &mut self,
        bindings_ctx: &mut BC,
        ip_sock: &IpSock<I, Self::WeakDeviceId>,
        options: &O,
    ) -> Result<Mms, MmsError> {
        let IpSockDefinition { remote_ip, local_ip, device, proto: _ } = &ip_sock.definition;
        let device = device
            .as_ref()
            .map(|d| d.upgrade().ok_or(ResolveRouteError::Unreachable))
            .transpose()?;

        let ResolvedRoute {
            src_addr: _,
            local_delivery_device: _,
            device,
            next_hop: _,
            internal_forwarding: _,
        } = self
            .lookup_route(
                bindings_ctx,
                device.as_ref(),
                Some(*local_ip),
                *remote_ip,
                options.transparent(),
                options.marks(),
            )
            .map_err(MmsError::NoDevice)?;
        let mtu = self.get_mtu(&device);
        // TODO(https://fxbug.dev/42072935): Calculate the options size when they
        // are supported.
        Mms::from_mtu::<I>(mtu, 0 /* no ip options used */).ok_or(MmsError::MTUTooSmall(mtu))
    }
}

/// IPv6 source address selection as defined in [RFC 6724 Section 5].
pub(crate) mod ipv6_source_address_selection {
    use net_types::ip::{AddrSubnet, IpAddress as _};

    use super::*;

    use netstack3_base::Ipv6DeviceAddr;

    /// A source address selection candidate.
    pub struct SasCandidate<D> {
        /// The candidate address and subnet.
        pub addr_sub: AddrSubnet<Ipv6Addr, Ipv6DeviceAddr>,
        /// True if the address is assigned (i.e. non tentative).
        pub assigned: bool,
        /// True if the address is deprecated (i.e. not preferred).
        pub deprecated: bool,
        /// True if the address is temporary (i.e. not permanent).
        pub temporary: bool,
        /// The device this address belongs to.
        pub device: D,
    }

    /// Selects the source address for an IPv6 socket using the algorithm
    /// defined in [RFC 6724 Section 5].
    ///
    /// This algorithm is only applicable when the user has not explicitly
    /// specified a source address.
    ///
    /// `remote_ip` is the remote IP address of the socket, `outbound_device` is
    /// the device over which outbound traffic to `remote_ip` is sent (according
    /// to the forwarding table), and `addresses` is an iterator of all
    /// addresses on all devices. The algorithm works by iterating over
    /// `addresses` and selecting the address which is most preferred according
    /// to a set of selection criteria.
    pub fn select_ipv6_source_address<
        'a,
        D: PartialEq,
        A,
        I: Iterator<Item = A>,
        F: FnMut(&A) -> SasCandidate<D>,
    >(
        remote_ip: Option<SpecifiedAddr<Ipv6Addr>>,
        outbound_device: &D,
        addresses: I,
        mut get_candidate: F,
    ) -> Option<A> {
        // Source address selection as defined in RFC 6724 Section 5.
        //
        // The algorithm operates by defining a partial ordering on available
        // source addresses, and choosing one of the best address as defined by
        // that ordering (given multiple best addresses, the choice from among
        // those is implementation-defined). The partial order is defined in
        // terms of a sequence of rules. If a given rule defines an order
        // between two addresses, then that is their order. Otherwise, the next
        // rule must be consulted, and so on until all of the rules are
        // exhausted.

        addresses
            .map(|item| {
                let candidate = get_candidate(&item);
                (item, candidate)
            })
            // Tentative addresses are not considered available to the source
            // selection algorithm.
            .filter(|(_, candidate)| candidate.assigned)
            .max_by(|(_, a), (_, b)| {
                select_ipv6_source_address_cmp(remote_ip, outbound_device, a, b)
            })
            .map(|(item, _candidate)| item)
    }

    /// Comparison operator used by `select_ipv6_source_address`.
    fn select_ipv6_source_address_cmp<D: PartialEq>(
        remote_ip: Option<SpecifiedAddr<Ipv6Addr>>,
        outbound_device: &D,
        a: &SasCandidate<D>,
        b: &SasCandidate<D>,
    ) -> Ordering {
        // TODO(https://fxbug.dev/42123500): Implement rules 4, 5.5, and 6.
        let SasCandidate {
            addr_sub: a_addr_sub,
            assigned: a_assigned,
            deprecated: a_deprecated,
            temporary: a_temporary,
            device: a_device,
        } = a;
        let SasCandidate {
            addr_sub: b_addr_sub,
            assigned: b_assigned,
            deprecated: b_deprecated,
            temporary: b_temporary,
            device: b_device,
        } = b;

        let a_addr = a_addr_sub.addr().into_specified();
        let b_addr = b_addr_sub.addr().into_specified();

        // Assertions required in order for this implementation to be valid.

        // Required by the implementation of Rule 1.
        if let Some(remote_ip) = remote_ip {
            debug_assert!(!(a_addr == remote_ip && b_addr == remote_ip));
        }

        // Addresses that are not considered assigned are not valid source
        // addresses.
        debug_assert!(a_assigned);
        debug_assert!(b_assigned);

        rule_1(remote_ip, a_addr, b_addr)
            .then_with(|| rule_2(remote_ip, a_addr, b_addr))
            .then_with(|| rule_3(*a_deprecated, *b_deprecated))
            .then_with(|| rule_5(outbound_device, a_device, b_device))
            .then_with(|| rule_7(*a_temporary, *b_temporary))
            .then_with(|| rule_8(remote_ip, *a_addr_sub, *b_addr_sub))
    }

    // Assumes that `a` and `b` are not both equal to `remote_ip`.
    fn rule_1(
        remote_ip: Option<SpecifiedAddr<Ipv6Addr>>,
        a: SpecifiedAddr<Ipv6Addr>,
        b: SpecifiedAddr<Ipv6Addr>,
    ) -> Ordering {
        let remote_ip = match remote_ip {
            Some(remote_ip) => remote_ip,
            None => return Ordering::Equal,
        };
        if (a == remote_ip) != (b == remote_ip) {
            // Rule 1: Prefer same address.
            //
            // Note that both `a` and `b` cannot be equal to `remote_ip` since
            // that would imply that we had added the same address twice to the
            // same device.
            //
            // If `(a == remote_ip) != (b == remote_ip)`, then exactly one of
            // them is equal. If this inequality does not hold, then they must
            // both be unequal to `remote_ip`. In the first case, we have a tie,
            // and in the second case, the rule doesn't apply. In either case,
            // we move onto the next rule.
            if a == remote_ip { Ordering::Greater } else { Ordering::Less }
        } else {
            Ordering::Equal
        }
    }

    fn rule_2(
        remote_ip: Option<SpecifiedAddr<Ipv6Addr>>,
        a: SpecifiedAddr<Ipv6Addr>,
        b: SpecifiedAddr<Ipv6Addr>,
    ) -> Ordering {
        // Scope ordering is defined by the Multicast Scope ID, see
        // https://datatracker.ietf.org/doc/html/rfc6724#section-3.1 .
        let remote_scope = match remote_ip {
            Some(remote_ip) => remote_ip.scope().multicast_scope_id(),
            None => return Ordering::Equal,
        };
        let a_scope = a.scope().multicast_scope_id();
        let b_scope = b.scope().multicast_scope_id();
        if a_scope < b_scope {
            if a_scope < remote_scope { Ordering::Less } else { Ordering::Greater }
        } else if a_scope > b_scope {
            if b_scope < remote_scope { Ordering::Greater } else { Ordering::Less }
        } else {
            Ordering::Equal
        }
    }

    fn rule_3(a_deprecated: bool, b_deprecated: bool) -> Ordering {
        match (a_deprecated, b_deprecated) {
            (true, false) => Ordering::Less,
            (true, true) | (false, false) => Ordering::Equal,
            (false, true) => Ordering::Greater,
        }
    }

    fn rule_5<D: PartialEq>(outbound_device: &D, a_device: &D, b_device: &D) -> Ordering {
        if (a_device == outbound_device) != (b_device == outbound_device) {
            // Rule 5: Prefer outgoing interface.
            if a_device == outbound_device { Ordering::Greater } else { Ordering::Less }
        } else {
            Ordering::Equal
        }
    }

    // Prefer temporary addresses following rule 7.
    fn rule_7(a_temporary: bool, b_temporary: bool) -> Ordering {
        match (a_temporary, b_temporary) {
            (true, false) => Ordering::Greater,
            (true, true) | (false, false) => Ordering::Equal,
            (false, true) => Ordering::Less,
        }
    }

    fn rule_8(
        remote_ip: Option<SpecifiedAddr<Ipv6Addr>>,
        a: AddrSubnet<Ipv6Addr, Ipv6DeviceAddr>,
        b: AddrSubnet<Ipv6Addr, Ipv6DeviceAddr>,
    ) -> Ordering {
        let remote_ip = match remote_ip {
            Some(remote_ip) => remote_ip,
            None => return Ordering::Equal,
        };
        // Per RFC 6724 Section 2.2:
        //
        //   We define the common prefix length CommonPrefixLen(S, D) of a
        //   source address S and a destination address D as the length of the
        //   longest prefix (looking at the most significant, or leftmost, bits)
        //   that the two addresses have in common, up to the length of S's
        //   prefix (i.e., the portion of the address not including the
        //   interface ID).  For example, CommonPrefixLen(fe80::1, fe80::2) is
        //   64.
        fn common_prefix_len(
            src: AddrSubnet<Ipv6Addr, Ipv6DeviceAddr>,
            dst: SpecifiedAddr<Ipv6Addr>,
        ) -> u8 {
            core::cmp::min(src.addr().common_prefix_len(&dst), src.subnet().prefix())
        }

        // Rule 8: Use longest matching prefix.
        //
        // Note that, per RFC 6724 Section 5:
        //
        //   Rule 8 MAY be superseded if the implementation has other means of
        //   choosing among source addresses.  For example, if the
        //   implementation somehow knows which source address will result in
        //   the "best" communications performance.
        //
        // We don't currently make use of this option, but it's an option for
        // the future.
        common_prefix_len(a, remote_ip).cmp(&common_prefix_len(b, remote_ip))
    }

    #[cfg(test)]
    mod tests {
        use net_declare::net_ip_v6;

        use super::*;

        #[test]
        fn test_select_ipv6_source_address() {
            // Test the comparison operator used by `select_ipv6_source_address`
            // by separately testing each comparison condition.

            let remote = SpecifiedAddr::new(net_ip_v6!("2001:0db8:1::")).unwrap();
            let local0 = SpecifiedAddr::new(net_ip_v6!("2001:0db8:2::")).unwrap();
            let local1 = SpecifiedAddr::new(net_ip_v6!("2001:0db8:3::")).unwrap();
            let link_local_remote = SpecifiedAddr::new(net_ip_v6!("fe80::1:2:42")).unwrap();
            let link_local = SpecifiedAddr::new(net_ip_v6!("fe80::1:2:4")).unwrap();
            let dev0 = &0;
            let dev1 = &1;
            let dev2 = &2;

            // Rule 1: Prefer same address
            assert_eq!(rule_1(Some(remote), remote, local0), Ordering::Greater);
            assert_eq!(rule_1(Some(remote), local0, remote), Ordering::Less);
            assert_eq!(rule_1(Some(remote), local0, local1), Ordering::Equal);
            assert_eq!(rule_1(None, local0, local1), Ordering::Equal);

            // Rule 2: Prefer appropriate scope
            assert_eq!(rule_2(Some(remote), local0, local1), Ordering::Equal);
            assert_eq!(rule_2(Some(remote), local1, local0), Ordering::Equal);
            assert_eq!(rule_2(Some(remote), local0, link_local), Ordering::Greater);
            assert_eq!(rule_2(Some(remote), link_local, local0), Ordering::Less);
            assert_eq!(rule_2(Some(link_local_remote), local0, link_local), Ordering::Less);
            assert_eq!(rule_2(Some(link_local_remote), link_local, local0), Ordering::Greater);
            assert_eq!(rule_1(None, local0, link_local), Ordering::Equal);

            // Rule 3: Avoid deprecated states
            assert_eq!(rule_3(false, true), Ordering::Greater);
            assert_eq!(rule_3(true, false), Ordering::Less);
            assert_eq!(rule_3(true, true), Ordering::Equal);
            assert_eq!(rule_3(false, false), Ordering::Equal);

            // Rule 5: Prefer outgoing interface
            assert_eq!(rule_5(dev0, dev0, dev2), Ordering::Greater);
            assert_eq!(rule_5(dev0, dev2, dev0), Ordering::Less);
            assert_eq!(rule_5(dev0, dev0, dev0), Ordering::Equal);
            assert_eq!(rule_5(dev0, dev2, dev2), Ordering::Equal);

            // Rule 7: Prefer temporary address.
            assert_eq!(rule_7(true, false), Ordering::Greater);
            assert_eq!(rule_7(false, true), Ordering::Less);
            assert_eq!(rule_7(true, true), Ordering::Equal);
            assert_eq!(rule_7(false, false), Ordering::Equal);

            // Rule 8: Use longest matching prefix.
            {
                let new_addr_entry = |addr, prefix_len| AddrSubnet::new(addr, prefix_len).unwrap();

                // First, test that the longest prefix match is preferred when
                // using addresses whose common prefix length is shorter than
                // the subnet prefix length.

                // 4 leading 0x01 bytes.
                let remote = SpecifiedAddr::new(net_ip_v6!("1111::")).unwrap();
                // 3 leading 0x01 bytes.
                let local0 = new_addr_entry(net_ip_v6!("1110::"), 64);
                // 2 leading 0x01 bytes.
                let local1 = new_addr_entry(net_ip_v6!("1100::"), 64);

                assert_eq!(rule_8(Some(remote), local0, local1), Ordering::Greater);
                assert_eq!(rule_8(Some(remote), local1, local0), Ordering::Less);
                assert_eq!(rule_8(Some(remote), local0, local0), Ordering::Equal);
                assert_eq!(rule_8(Some(remote), local1, local1), Ordering::Equal);
                assert_eq!(rule_8(None, local0, local1), Ordering::Equal);

                // Second, test that the common prefix length is capped at the
                // subnet prefix length.

                // 3 leading 0x01 bytes, but a subnet prefix length of 8 (1 byte).
                let local0 = new_addr_entry(net_ip_v6!("1110::"), 8);
                // 2 leading 0x01 bytes, but a subnet prefix length of 8 (1 byte).
                let local1 = new_addr_entry(net_ip_v6!("1100::"), 8);

                assert_eq!(rule_8(Some(remote), local0, local1), Ordering::Equal);
                assert_eq!(rule_8(Some(remote), local1, local0), Ordering::Equal);
                assert_eq!(rule_8(Some(remote), local0, local0), Ordering::Equal);
                assert_eq!(rule_8(Some(remote), local1, local1), Ordering::Equal);
                assert_eq!(rule_8(None, local0, local1), Ordering::Equal);
            }

            {
                let new_addr_entry = |addr, device| SasCandidate {
                    addr_sub: AddrSubnet::new(addr, 128).unwrap(),
                    deprecated: false,
                    assigned: true,
                    temporary: false,
                    device,
                };

                // If no rules apply, then the two address entries are equal.
                assert_eq!(
                    select_ipv6_source_address_cmp(
                        Some(remote),
                        dev0,
                        &new_addr_entry(*local0, *dev1),
                        &new_addr_entry(*local1, *dev2),
                    ),
                    Ordering::Equal
                );
            }
        }

        #[test]
        fn test_select_ipv6_source_address_no_remote() {
            // Verify that source address selection correctly applies all
            // applicable rules when the remote is `None`.
            let dev0 = &0;
            let dev1 = &1;
            let dev2 = &2;

            let local0 = SpecifiedAddr::new(net_ip_v6!("2001:0db8:2::")).unwrap();
            let local1 = SpecifiedAddr::new(net_ip_v6!("2001:0db8:3::")).unwrap();

            let new_addr_entry = |addr, deprecated, device| SasCandidate {
                addr_sub: AddrSubnet::new(addr, 128).unwrap(),
                deprecated,
                assigned: true,
                temporary: false,
                device,
            };

            // Verify that Rule 3 still applies (avoid deprecated states).
            assert_eq!(
                select_ipv6_source_address_cmp(
                    None,
                    dev0,
                    &new_addr_entry(*local0, false, *dev1),
                    &new_addr_entry(*local1, true, *dev2),
                ),
                Ordering::Greater
            );

            // Verify that Rule 5 still applies (Prefer outgoing interface).
            assert_eq!(
                select_ipv6_source_address_cmp(
                    None,
                    dev0,
                    &new_addr_entry(*local0, false, *dev0),
                    &new_addr_entry(*local1, false, *dev1),
                ),
                Ordering::Greater
            );
        }
    }
}

/// Test fake implementations of the traits defined in the `socket` module.
#[cfg(any(test, feature = "testutils"))]
pub(crate) mod testutil {
    use alloc::boxed::Box;
    use alloc::vec::Vec;
    use core::num::NonZeroUsize;

    use crate::internal::types::RoutePreference;
    use derivative::Derivative;
    use net_types::MulticastAddr;
    use net_types::ip::{GenericOverIp, IpAddr, IpAddress, Ipv4, Ipv4Addr, Ipv6, Subnet};
    use netstack3_base::testutil::{FakeCoreCtx, FakeStrongDeviceId, FakeWeakDeviceId};
    use netstack3_base::{SendFrameContext, SendFrameError};
    use netstack3_filter::Tuple;
    use netstack3_hashmap::HashMap;

    use super::*;
    use crate::internal::base::{
        BaseTransportIpContext, DEFAULT_HOP_LIMITS, HopLimits, MulticastMembershipHandler,
    };
    use crate::internal::routing::testutil::FakeIpRoutingCtx;
    use crate::internal::routing::{self, RoutingTable};
    use crate::internal::types::{Destination, Entry, Metric, RawMetric};

    /// A fake implementation of the traits required by the transport layer from
    /// the IP layer.
    #[derive(Derivative, GenericOverIp)]
    #[generic_over_ip(I, Ip)]
    #[derivative(Default(bound = ""))]
    pub struct FakeIpSocketCtx<I: Ip, D> {
        pub(crate) table: RoutingTable<I, D>,
        forwarding: FakeIpRoutingCtx<D>,
        devices: HashMap<D, FakeDeviceState<I>>,
    }

    /// A trait enabling [`FakeIpSockeCtx`]'s implementations for
    /// [`FakeCoreCtx`] with types that hold a [`FakeIpSocketCtx`] internally,
    pub trait InnerFakeIpSocketCtx<I: Ip, D> {
        /// Gets a mutable reference to the inner fake context.
        fn fake_ip_socket_ctx_mut(&mut self) -> &mut FakeIpSocketCtx<I, D>;
    }

    impl<I: Ip, D> InnerFakeIpSocketCtx<I, D> for FakeIpSocketCtx<I, D> {
        fn fake_ip_socket_ctx_mut(&mut self) -> &mut FakeIpSocketCtx<I, D> {
            self
        }
    }

    impl<I: IpExt, D: FakeStrongDeviceId, BC> BaseTransportIpContext<I, BC> for FakeIpSocketCtx<I, D> {
        fn get_default_hop_limits(&mut self, device: Option<&D>) -> HopLimits {
            device.map_or(DEFAULT_HOP_LIMITS, |device| {
                let hop_limit = self.get_device_state(device).default_hop_limit;
                HopLimits { unicast: hop_limit, multicast: DEFAULT_HOP_LIMITS.multicast }
            })
        }

        type DevicesWithAddrIter<'a> = Box<dyn Iterator<Item = D> + 'a>;

        fn with_devices_with_assigned_addr<O, F: FnOnce(Self::DevicesWithAddrIter<'_>) -> O>(
            &mut self,
            addr: SpecifiedAddr<I::Addr>,
            cb: F,
        ) -> O {
            cb(Box::new(self.devices.iter().filter_map(move |(device, state)| {
                state.addresses.contains(&addr).then(|| device.clone())
            })))
        }

        fn get_original_destination(&mut self, _tuple: &Tuple<I>) -> Option<(I::Addr, u16)> {
            unimplemented!()
        }
    }

    impl<I: IpExt, D: FakeStrongDeviceId> DeviceIdContext<AnyDevice> for FakeIpSocketCtx<I, D> {
        type DeviceId = D;
        type WeakDeviceId = D::Weak;
    }

    impl<I, State, D, Meta, BC> IpSocketHandler<I, BC> for FakeCoreCtx<State, Meta, D>
    where
        I: IpExt + FilterIpExt,
        State: InnerFakeIpSocketCtx<I, D>,
        D: FakeStrongDeviceId,
        BC: TxMetadataBindingsTypes,
        FakeCoreCtx<State, Meta, D>:
            SendFrameContext<BC, SendIpPacketMeta<I, Self::DeviceId, SpecifiedAddr<I::Addr>>>,
    {
        fn new_ip_socket<O>(
            &mut self,
            _bindings_ctx: &mut BC,
            args: IpSocketArgs<'_, Self::DeviceId, I, O>,
        ) -> Result<IpSock<I, Self::WeakDeviceId>, IpSockCreationError>
        where
            O: RouteResolutionOptions<I>,
        {
            self.state.fake_ip_socket_ctx_mut().new_ip_socket(args)
        }

        fn send_ip_packet<S, O>(
            &mut self,
            bindings_ctx: &mut BC,
            socket: &IpSock<I, Self::WeakDeviceId>,
            body: S,
            options: &O,
            // NB: Tx metadata plumbing is not supported for fake socket
            // contexts. Drop at the end of the scope.
            _tx_meta: BC::TxMetadata,
        ) -> Result<(), IpSockSendError>
        where
            S: TransportPacketSerializer<I>,
            S::Buffer: BufferMut,
            O: SendOptions<I> + RouteResolutionOptions<I>,
        {
            let meta = self.state.fake_ip_socket_ctx_mut().resolve_send_meta(socket, options)?;
            self.send_frame(bindings_ctx, meta, body).or_else(
                |SendFrameError { serializer: _, error }| IpSockSendError::from_send_frame(error),
            )
        }

        fn confirm_reachable<O>(
            &mut self,
            _bindings_ctx: &mut BC,
            _socket: &IpSock<I, Self::WeakDeviceId>,
            _options: &O,
        ) {
        }
    }

    impl<I: IpExt, D: FakeStrongDeviceId, BC> MulticastMembershipHandler<I, BC>
        for FakeIpSocketCtx<I, D>
    {
        fn join_multicast_group(
            &mut self,
            _bindings_ctx: &mut BC,
            device: &Self::DeviceId,
            addr: MulticastAddr<<I as Ip>::Addr>,
        ) {
            let value = self.get_device_state_mut(device).multicast_groups.entry(addr).or_insert(0);
            *value = value.checked_add(1).unwrap();
        }

        fn leave_multicast_group(
            &mut self,
            _bindings_ctx: &mut BC,
            device: &Self::DeviceId,
            addr: MulticastAddr<<I as Ip>::Addr>,
        ) {
            let value = self
                .get_device_state_mut(device)
                .multicast_groups
                .get_mut(&addr)
                .unwrap_or_else(|| panic!("no entry for {addr} on {device:?}"));
            *value = value.checked_sub(1).unwrap();
        }

        fn select_device_for_multicast_group(
            &mut self,
            addr: MulticastAddr<<I as Ip>::Addr>,
            _marks: &Marks,
        ) -> Result<Self::DeviceId, ResolveRouteError> {
            let remote_ip = SocketIpAddr::new_from_multicast(addr);
            self.lookup_route(None, None, remote_ip, /* transparent */ false)
                .map(|ResolvedRoute { device, .. }| device)
        }
    }

    impl<I, BC, D, State, Meta> BaseTransportIpContext<I, BC> for FakeCoreCtx<State, Meta, D>
    where
        I: IpExt + FilterIpExt,
        D: FakeStrongDeviceId,
        State: InnerFakeIpSocketCtx<I, D>,
        BC: TxMetadataBindingsTypes,
        Self: IpSocketHandler<I, BC, DeviceId = D, WeakDeviceId = FakeWeakDeviceId<D>>,
    {
        type DevicesWithAddrIter<'a> = Box<dyn Iterator<Item = D> + 'a>;

        fn with_devices_with_assigned_addr<O, F: FnOnce(Self::DevicesWithAddrIter<'_>) -> O>(
            &mut self,
            addr: SpecifiedAddr<I::Addr>,
            cb: F,
        ) -> O {
            BaseTransportIpContext::<I, BC>::with_devices_with_assigned_addr(
                self.state.fake_ip_socket_ctx_mut(),
                addr,
                cb,
            )
        }

        fn get_default_hop_limits(&mut self, device: Option<&Self::DeviceId>) -> HopLimits {
            BaseTransportIpContext::<I, BC>::get_default_hop_limits(
                self.state.fake_ip_socket_ctx_mut(),
                device,
            )
        }

        fn get_original_destination(&mut self, tuple: &Tuple<I>) -> Option<(I::Addr, u16)> {
            BaseTransportIpContext::<I, BC>::get_original_destination(
                self.state.fake_ip_socket_ctx_mut(),
                tuple,
            )
        }
    }

    /// A fake context providing [`IpSocketHandler`] for tests.
    #[derive(Derivative)]
    #[derivative(Default(bound = ""))]
    pub struct FakeDualStackIpSocketCtx<D> {
        v4: FakeIpSocketCtx<Ipv4, D>,
        v6: FakeIpSocketCtx<Ipv6, D>,
    }

    impl<D: FakeStrongDeviceId> FakeDualStackIpSocketCtx<D> {
        /// Creates a new [`FakeDualStackIpSocketCtx`] with `devices`.
        pub fn new<A: Into<SpecifiedAddr<IpAddr>>>(
            devices: impl IntoIterator<Item = FakeDeviceConfig<D, A>>,
        ) -> Self {
            let partition =
                |v: Vec<A>| -> (Vec<SpecifiedAddr<Ipv4Addr>>, Vec<SpecifiedAddr<Ipv6Addr>>) {
                    v.into_iter().fold((Vec::new(), Vec::new()), |(mut v4, mut v6), i| {
                        match IpAddr::from(i.into()) {
                            IpAddr::V4(a) => v4.push(a),
                            IpAddr::V6(a) => v6.push(a),
                        }
                        (v4, v6)
                    })
                };

            let (v4, v6): (Vec<_>, Vec<_>) = devices
                .into_iter()
                .map(|FakeDeviceConfig { device, local_ips, remote_ips }| {
                    let (local_v4, local_v6) = partition(local_ips);
                    let (remote_v4, remote_v6) = partition(remote_ips);
                    (
                        FakeDeviceConfig {
                            device: device.clone(),
                            local_ips: local_v4,
                            remote_ips: remote_v4,
                        },
                        FakeDeviceConfig { device, local_ips: local_v6, remote_ips: remote_v6 },
                    )
                })
                .unzip();
            Self { v4: FakeIpSocketCtx::new(v4), v6: FakeIpSocketCtx::new(v6) }
        }

        /// Returns the [`FakeIpSocketCtx`] for IP version `I`.
        pub fn inner_mut<I: Ip>(&mut self) -> &mut FakeIpSocketCtx<I, D> {
            I::map_ip_out(self, |s| &mut s.v4, |s| &mut s.v6)
        }

        fn inner<I: Ip>(&self) -> &FakeIpSocketCtx<I, D> {
            I::map_ip_out(self, |s| &s.v4, |s| &s.v6)
        }

        /// Adds a fake direct route to `ip` through `device`.
        pub fn add_route(&mut self, device: D, ip: SpecifiedAddr<IpAddr>) {
            match IpAddr::from(ip) {
                IpAddr::V4(ip) => {
                    routing::testutil::add_on_link_routing_entry(&mut self.v4.table, ip, device)
                }
                IpAddr::V6(ip) => {
                    routing::testutil::add_on_link_routing_entry(&mut self.v6.table, ip, device)
                }
            }
        }

        /// Adds a fake route to `subnet` through `device`.
        pub fn add_subnet_route<A: IpAddress>(&mut self, device: D, subnet: Subnet<A>) {
            let entry = Entry {
                subnet,
                device,
                gateway: None,
                metric: Metric::ExplicitMetric(RawMetric(0)),
                route_preference: RoutePreference::Medium,
            };
            A::Version::map_ip::<_, ()>(
                entry,
                |entry_v4| {
                    let _ = routing::testutil::add_entry(&mut self.v4.table, entry_v4)
                        .expect("Failed to add route");
                },
                |entry_v6| {
                    let _ = routing::testutil::add_entry(&mut self.v6.table, entry_v6)
                        .expect("Failed to add route");
                },
            );
        }

        /// Returns a mutable reference to fake device state.
        pub fn get_device_state_mut<I: IpExt>(&mut self, device: &D) -> &mut FakeDeviceState<I> {
            self.inner_mut::<I>().get_device_state_mut(device)
        }

        /// Returns the fake multicast memberships.
        pub fn multicast_memberships<I: IpExt>(
            &self,
        ) -> HashMap<(D, MulticastAddr<I::Addr>), NonZeroUsize> {
            self.inner::<I>().multicast_memberships()
        }
    }

    impl<I: IpExt, S: InnerFakeIpSocketCtx<I, D>, Meta, D: FakeStrongDeviceId, BC>
        MulticastMembershipHandler<I, BC> for FakeCoreCtx<S, Meta, D>
    {
        fn join_multicast_group(
            &mut self,
            bindings_ctx: &mut BC,
            device: &Self::DeviceId,
            addr: MulticastAddr<<I as Ip>::Addr>,
        ) {
            MulticastMembershipHandler::<I, BC>::join_multicast_group(
                self.state.fake_ip_socket_ctx_mut(),
                bindings_ctx,
                device,
                addr,
            )
        }

        fn leave_multicast_group(
            &mut self,
            bindings_ctx: &mut BC,
            device: &Self::DeviceId,
            addr: MulticastAddr<<I as Ip>::Addr>,
        ) {
            MulticastMembershipHandler::<I, BC>::leave_multicast_group(
                self.state.fake_ip_socket_ctx_mut(),
                bindings_ctx,
                device,
                addr,
            )
        }

        fn select_device_for_multicast_group(
            &mut self,
            addr: MulticastAddr<<I as Ip>::Addr>,
            marks: &Marks,
        ) -> Result<Self::DeviceId, ResolveRouteError> {
            MulticastMembershipHandler::<I, BC>::select_device_for_multicast_group(
                self.state.fake_ip_socket_ctx_mut(),
                addr,
                marks,
            )
        }
    }

    impl<I: Ip, D, State: InnerFakeIpSocketCtx<I, D>, Meta> InnerFakeIpSocketCtx<I, D>
        for FakeCoreCtx<State, Meta, D>
    {
        fn fake_ip_socket_ctx_mut(&mut self) -> &mut FakeIpSocketCtx<I, D> {
            self.state.fake_ip_socket_ctx_mut()
        }
    }

    impl<I: Ip, D: FakeStrongDeviceId> InnerFakeIpSocketCtx<I, D> for FakeDualStackIpSocketCtx<D> {
        fn fake_ip_socket_ctx_mut(&mut self) -> &mut FakeIpSocketCtx<I, D> {
            self.inner_mut::<I>()
        }
    }

    /// A device configuration for fake socket contexts.
    #[derive(Clone, GenericOverIp)]
    #[generic_over_ip()]
    pub struct FakeDeviceConfig<D, A> {
        /// The device.
        pub device: D,
        /// The device's local IPs.
        pub local_ips: Vec<A>,
        /// The remote IPs reachable from this device.
        pub remote_ips: Vec<A>,
    }

    /// State associated with a fake device in [`FakeIpSocketCtx`].
    pub struct FakeDeviceState<I: Ip> {
        /// The default hop limit used by the device.
        pub default_hop_limit: NonZeroU8,
        /// The assigned device addresses.
        pub addresses: Vec<SpecifiedAddr<I::Addr>>,
        /// The joined multicast groups.
        pub multicast_groups: HashMap<MulticastAddr<I::Addr>, usize>,
    }

    impl<I: Ip> FakeDeviceState<I> {
        /// Returns whether this fake device has joined multicast group `addr`.
        pub fn is_in_multicast_group(&self, addr: &MulticastAddr<I::Addr>) -> bool {
            self.multicast_groups.get(addr).is_some_and(|v| *v != 0)
        }
    }

    impl<I: IpExt, D: FakeStrongDeviceId> FakeIpSocketCtx<I, D> {
        /// Creates a new `FakeIpSocketCtx` with the given device
        /// configs.
        pub fn new(
            device_configs: impl IntoIterator<Item = FakeDeviceConfig<D, SpecifiedAddr<I::Addr>>>,
        ) -> Self {
            let mut table = RoutingTable::default();
            let mut devices = HashMap::default();
            for FakeDeviceConfig { device, local_ips, remote_ips } in device_configs {
                for addr in remote_ips {
                    routing::testutil::add_on_link_routing_entry(&mut table, addr, device.clone())
                }
                let state = FakeDeviceState {
                    default_hop_limit: DEFAULT_HOP_LIMITS.unicast,
                    addresses: local_ips,
                    multicast_groups: Default::default(),
                };
                assert!(
                    devices.insert(device.clone(), state).is_none(),
                    "duplicate entries for {device:?}",
                );
            }

            Self { table, devices, forwarding: Default::default() }
        }

        /// Returns an immutable reference to the fake device state.
        pub fn get_device_state(&self, device: &D) -> &FakeDeviceState<I> {
            self.devices.get(device).unwrap_or_else(|| panic!("no device {device:?}"))
        }

        /// Returns a mutable reference to the fake device state.
        pub fn get_device_state_mut(&mut self, device: &D) -> &mut FakeDeviceState<I> {
            self.devices.get_mut(device).unwrap_or_else(|| panic!("no device {device:?}"))
        }

        pub(crate) fn multicast_memberships(
            &self,
        ) -> HashMap<(D, MulticastAddr<I::Addr>), NonZeroUsize> {
            self.devices
                .iter()
                .map(|(device, state)| {
                    state.multicast_groups.iter().filter_map(|(group, count)| {
                        NonZeroUsize::new(*count).map(|count| ((device.clone(), *group), count))
                    })
                })
                .flatten()
                .collect()
        }

        fn new_ip_socket<O>(
            &mut self,
            args: IpSocketArgs<'_, D, I, O>,
        ) -> Result<IpSock<I, D::Weak>, IpSockCreationError>
        where
            O: RouteResolutionOptions<I>,
        {
            let IpSocketArgs { device, local_ip, remote_ip, proto, options } = args;
            let device = device
                .as_ref()
                .map(|d| d.as_strong_ref().ok_or(ResolveRouteError::Unreachable))
                .transpose()?;
            let device = device.as_ref().map(|d| d.as_ref());
            let resolved_route =
                self.lookup_route(device, local_ip, remote_ip, options.transparent())?;
            Ok(new_ip_socket(device, resolved_route, remote_ip, proto))
        }

        fn lookup_route(
            &mut self,
            device: Option<&D>,
            local_ip: Option<IpDeviceAddr<I::Addr>>,
            addr: RoutableIpAddr<I::Addr>,
            transparent: bool,
        ) -> Result<ResolvedRoute<I, D>, ResolveRouteError> {
            let Self { table, devices, forwarding } = self;
            let (destination, ()) = table
                .lookup_filter_map(forwarding, device, addr.addr(), |_, d| match &local_ip {
                    None => Some(()),
                    Some(local_ip) => {
                        if transparent {
                            return Some(());
                        }
                        devices.get(d).and_then(|state| {
                            state.addresses.contains(local_ip.as_ref()).then_some(())
                        })
                    }
                })
                .next()
                .ok_or(ResolveRouteError::Unreachable)?;

            let Destination { device, next_hop } = destination;
            let mut addrs = devices.get(device).unwrap().addresses.iter();
            let local_ip = match local_ip {
                None => {
                    let addr = addrs.next().ok_or(ResolveRouteError::NoSrcAddr)?;
                    IpDeviceAddr::new(addr.get()).expect("not valid device addr")
                }
                Some(local_ip) => {
                    if !transparent {
                        // We already constrained the set of devices so this
                        // should be a given.
                        assert!(
                            addrs.any(|a| a.get() == local_ip.addr()),
                            "didn't find IP {:?} in {:?}",
                            local_ip,
                            addrs.collect::<Vec<_>>()
                        );
                    }
                    local_ip
                }
            };

            Ok(ResolvedRoute {
                src_addr: local_ip,
                device: device.clone(),
                local_delivery_device: None,
                next_hop,
                // NB: Keep unit tests simple and skip internal forwarding
                // logic. Instead, this is verified by integration tests.
                internal_forwarding: InternalForwarding::NotUsed,
            })
        }

        fn resolve_send_meta<O>(
            &mut self,
            socket: &IpSock<I, D::Weak>,
            options: &O,
        ) -> Result<SendIpPacketMeta<I, D, SpecifiedAddr<I::Addr>>, IpSockSendError>
        where
            O: SendOptions<I> + RouteResolutionOptions<I>,
        {
            let IpSockDefinition { remote_ip, local_ip, device, proto } = &socket.definition;
            let device = device
                .as_ref()
                .map(|d| d.upgrade().ok_or(ResolveRouteError::Unreachable))
                .transpose()?;
            let ResolvedRoute {
                src_addr,
                device,
                next_hop,
                local_delivery_device: _,
                internal_forwarding: _,
            } = self.lookup_route(
                device.as_ref(),
                Some(*local_ip),
                *remote_ip,
                options.transparent(),
            )?;

            let remote_ip: &SpecifiedAddr<_> = remote_ip.as_ref();

            let destination = IpPacketDestination::from_next_hop(next_hop, *remote_ip);
            Ok(SendIpPacketMeta {
                device,
                src_ip: src_addr.into(),
                dst_ip: *remote_ip,
                destination,
                proto: *proto,
                ttl: options.hop_limit(remote_ip),
                mtu: options.mtu(),
                dscp_and_ecn: DscpAndEcn::default(),
            })
        }
    }
}
