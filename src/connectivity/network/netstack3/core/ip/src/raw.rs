// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Facilities backing raw IP sockets.

use alloc::collections::BTreeMap;
use alloc::collections::btree_map::Entry;
use core::fmt::{self, Debug, Display};
use core::num::NonZeroU8;
use derivative::Derivative;
use log::debug;
use net_types::ip::{GenericOverIp, Ip, IpVersionMarker, Mtu};
use net_types::{SpecifiedAddr, ZonedAddr};
use netstack3_base::socket::{
    DualStackIpExt, DualStackRemoteIp, SocketCookie, SocketZonedAddrExt as _,
};
use netstack3_base::sync::{PrimaryRc, StrongRc, WeakRc};
use netstack3_base::{
    AnyDevice, ContextPair, DeviceIdContext, Inspector, InspectorDeviceExt, InspectorExt,
    IpDeviceAddr, IpExt, Mark, MarkDomain, Marks, ReferenceNotifiers, ReferenceNotifiersExt as _,
    RemoveResourceResultWithContext, ResourceCounterContext, StrongDeviceIdentifier,
    TxMetadataBindingsTypes, WeakDeviceIdentifier, ZonedAddressError,
};
use netstack3_filter::{FilterIpExt, RawIpBody};
use netstack3_hashmap::HashMap;
use packet::{BufferMut, SliceBufViewMut};
use packet_formats::icmp;
use packet_formats::ip::{DscpAndEcn, IpPacket};
use thiserror::Error;
use zerocopy::SplitByteSlice;

use crate::DEFAULT_HOP_LIMITS;
use crate::internal::raw::counters::RawIpSocketCounters;
use crate::internal::raw::filter::RawIpSocketIcmpFilter;
use crate::internal::raw::protocol::RawIpSocketProtocol;
use crate::internal::raw::state::{RawIpSocketLockedState, RawIpSocketState};
use crate::internal::socket::{
    IpSockCreateAndSendError, IpSocketArgs, IpSocketHandler, RouteResolutionOptions,
    SendOneShotIpPacketError, SendOptions, SocketHopLimits,
};

mod checksum;
pub(crate) mod counters;
pub(crate) mod filter;
pub(crate) mod protocol;
pub(crate) mod state;

/// Types provided by bindings used in the raw IP socket implementation.
pub trait RawIpSocketsBindingsTypes: TxMetadataBindingsTypes {
    /// The bindings state (opaque to core) associated with a socket.
    type RawIpSocketState<I: Ip>: Send + Sync + Debug;
}

/// Errors that Bindings may encounter when receiving a packet.
pub enum ReceivePacketError {
    /// The socket's receive queue is full and can't hold the packet.
    QueueFull,
}

/// Functionality provided by bindings used in the raw IP socket implementation.
pub trait RawIpSocketsBindingsContext<I: IpExt, D: StrongDeviceIdentifier>:
    RawIpSocketsBindingsTypes + Sized
{
    /// Called for each received IP packet that matches the provided socket.
    fn receive_packet<B: SplitByteSlice>(
        &self,
        socket: &RawIpSocketId<I, D::Weak, Self>,
        packet: &I::Packet<B>,
        device: &D,
    ) -> Result<(), ReceivePacketError>;
}

/// The raw IP socket API.
pub struct RawIpSocketApi<I: Ip, C> {
    ctx: C,
    _ip_mark: IpVersionMarker<I>,
}

impl<I: Ip, C> RawIpSocketApi<I, C> {
    /// Constructs a new RAW IP socket API.
    pub fn new(ctx: C) -> Self {
        Self { ctx, _ip_mark: IpVersionMarker::new() }
    }
}

impl<I: IpExt + DualStackIpExt + FilterIpExt, C> RawIpSocketApi<I, C>
where
    C: ContextPair,
    C::BindingsContext: RawIpSocketsBindingsTypes + ReferenceNotifiers + 'static,
    C::CoreContext: RawIpSocketMapContext<I, C::BindingsContext>
        + RawIpSocketStateContext<I, C::BindingsContext>
        + ResourceCounterContext<RawIpApiSocketId<I, C>, RawIpSocketCounters<I>>,
{
    fn core_ctx(&mut self) -> &mut C::CoreContext {
        let Self { ctx, _ip_mark } = self;
        ctx.core_ctx()
    }

    fn contexts(&mut self) -> (&mut C::CoreContext, &mut C::BindingsContext) {
        let Self { ctx, _ip_mark } = self;
        ctx.contexts()
    }

    /// Creates a raw IP socket for the given protocol.
    pub fn create(
        &mut self,
        protocol: RawIpSocketProtocol<I>,
        external_state: <C::BindingsContext as RawIpSocketsBindingsTypes>::RawIpSocketState<I>,
    ) -> RawIpApiSocketId<I, C> {
        let socket =
            PrimaryRawIpSocketId(PrimaryRc::new(RawIpSocketState::new(protocol, external_state)));
        let strong = self.core_ctx().with_socket_map_mut(|socket_map| socket_map.insert(socket));
        debug!("created raw IP socket {strong:?}, on protocol {protocol:?}");

        if protocol.requires_system_checksums() {
            self.core_ctx().with_locked_state_mut(&strong, |state| state.system_checksums = true)
        }

        strong
    }

    /// Removes the raw IP socket from the system, returning its external state.
    pub fn close(
        &mut self,
        id: RawIpApiSocketId<I, C>,
    ) -> RemoveResourceResultWithContext<
        <C::BindingsContext as RawIpSocketsBindingsTypes>::RawIpSocketState<I>,
        C::BindingsContext,
    > {
        let primary = self.core_ctx().with_socket_map_mut(|socket_map| socket_map.remove(id));
        debug!("removed raw IP socket {primary:?}");
        let PrimaryRawIpSocketId(primary) = primary;

        C::BindingsContext::unwrap_or_notify_with_new_reference_notifier(
            primary,
            |state: RawIpSocketState<I, _, C::BindingsContext>| state.into_external_state(),
        )
    }

    /// Sends an IP packet on the raw IP socket to the provided destination.
    ///
    /// The provided `body` is not expected to include an IP header; a system
    /// determined header will automatically be applied.
    pub fn send_to<B: BufferMut>(
        &mut self,
        id: &RawIpApiSocketId<I, C>,
        remote_ip: Option<
            ZonedAddr<
                SpecifiedAddr<I::Addr>,
                <C::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId,
            >,
        >,
        mut body: B,
    ) -> Result<(), RawIpSocketSendToError> {
        match id.protocol() {
            RawIpSocketProtocol::Raw => return Err(RawIpSocketSendToError::ProtocolRaw),
            RawIpSocketProtocol::Proto(_) => {}
        }
        // TODO(https://fxbug.dev/339692009): Return an error if IP_HDRINCL is
        // set.

        // TODO(https://fxbug.dev/342579393): Use the socket's bound address.
        let local_ip = None;

        let remote_ip = match DualStackRemoteIp::<I, _>::new(remote_ip) {
            DualStackRemoteIp::ThisStack(addr) => addr,
            DualStackRemoteIp::OtherStack(_addr) => {
                return Err(RawIpSocketSendToError::MappedRemoteIp);
            }
        };
        let protocol = id.protocol().proto();

        let (core_ctx, bindings_ctx) = self.contexts();
        let result = core_ctx.with_locked_state_and_socket_handler(id, |state, core_ctx| {
            let RawIpSocketLockedState {
                bound_device,
                icmp_filter: _,
                hop_limits,
                multicast_loop,
                system_checksums,
                marks,
            } = state;
            let (remote_ip, device) = remote_ip
                .resolve_addr_with_device(bound_device.clone())
                .map_err(RawIpSocketSendToError::Zone)?;
            let send_options = RawIpSocketOptions {
                hop_limits: &hop_limits,
                multicast_loop: *multicast_loop,
                marks: &marks,
            };

            let build_packet_fn =
                |src_ip: IpDeviceAddr<I::Addr>| -> Result<RawIpBody<_, _>, RawIpSocketSendToError> {
                    if *system_checksums {
                        let buf = SliceBufViewMut::new(body.as_mut());
                        if !checksum::populate_checksum::<I, _>(
                            src_ip.addr(),
                            remote_ip.addr(),
                            protocol,
                            buf,
                        ) {
                            return Err(RawIpSocketSendToError::InvalidBody);
                        }
                    }
                    Ok(RawIpBody::new(protocol, src_ip.addr(), remote_ip.addr(), body))
                };

            // TODO(https://fxbug.dev/392111277): Enforce send buffer for raw ip
            // sockets.
            let tx_metadata = Default::default();

            core_ctx
                .send_oneshot_ip_packet_with_fallible_serializer(
                    bindings_ctx,
                    IpSocketArgs {
                        device: device.as_ref().map(|d| d.as_ref()),
                        local_ip,
                        remote_ip,
                        proto: protocol,
                        options: &send_options,
                    },
                    tx_metadata,
                    build_packet_fn,
                )
                .map_err(|e| match e {
                    SendOneShotIpPacketError::CreateAndSendError { err } => {
                        RawIpSocketSendToError::Ip(err)
                    }
                    SendOneShotIpPacketError::SerializeError(err) => err,
                })
        });
        match &result {
            Ok(()) => core_ctx
                .increment_both(&id, |counters: &RawIpSocketCounters<I>| &counters.tx_packets),
            Err(RawIpSocketSendToError::InvalidBody) => core_ctx
                .increment_both(&id, |counters: &RawIpSocketCounters<I>| {
                    &counters.tx_checksum_errors
                }),
            Err(_) => {}
        }
        result
    }

    // TODO(https://fxbug.dev/342577389): Add a `send` function that does not
    // require a remote_ip to support sending on connected sockets.
    // TODO(https://fxbug.dev/339692009): Add a `send` function that does not
    // require a remote_ip to support sending when the remote_ip is provided via
    // IP_HDRINCL.

    /// Sets the socket's bound device, returning the original value.
    pub fn set_device(
        &mut self,
        id: &RawIpApiSocketId<I, C>,
        device: Option<&<C::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId>,
    ) -> Option<<C::CoreContext as DeviceIdContext<AnyDevice>>::WeakDeviceId> {
        let device = device.map(|strong| strong.downgrade());
        // TODO(https://fxbug.dev/342579393): Verify the device is compatible
        // with the socket's bound address.
        // TODO(https://fxbug.dev/342577389): Verify the device is compatible
        // with the socket's peer address.
        self.core_ctx()
            .with_locked_state_mut(id, |state| core::mem::replace(&mut state.bound_device, device))
    }

    /// Gets the socket's bound device,
    pub fn get_device(
        &mut self,
        id: &RawIpApiSocketId<I, C>,
    ) -> Option<<C::CoreContext as DeviceIdContext<AnyDevice>>::WeakDeviceId> {
        self.core_ctx().with_locked_state(id, |state| state.bound_device.clone())
    }

    /// Sets the socket's ICMP filter, returning the original value.
    ///
    /// Note, if the socket's protocol is not compatible (e.g. ICMPv4 for an
    /// IPv4 socket, or ICMPv6 for an IPv6 socket), an error is returned.
    pub fn set_icmp_filter(
        &mut self,
        id: &RawIpApiSocketId<I, C>,
        filter: Option<RawIpSocketIcmpFilter<I>>,
    ) -> Result<Option<RawIpSocketIcmpFilter<I>>, RawIpSocketIcmpFilterError> {
        debug!("setting ICMP Filter on {id:?}: {filter:?}");
        if !id.protocol().is_icmp() {
            return Err(RawIpSocketIcmpFilterError::ProtocolNotIcmp);
        }
        Ok(self
            .core_ctx()
            .with_locked_state_mut(id, |state| core::mem::replace(&mut state.icmp_filter, filter)))
    }

    /// Gets the socket's ICMP
    ///
    /// Note, if the socket's protocol is not compatible (e.g. ICMPv4 for an
    /// IPv4 socket, or ICMPv6 for an IPv6 socket), an error is returned.
    pub fn get_icmp_filter(
        &mut self,
        id: &RawIpApiSocketId<I, C>,
    ) -> Result<Option<RawIpSocketIcmpFilter<I>>, RawIpSocketIcmpFilterError> {
        if !id.protocol().is_icmp() {
            return Err(RawIpSocketIcmpFilterError::ProtocolNotIcmp);
        }
        Ok(self.core_ctx().with_locked_state(id, |state| state.icmp_filter.clone()))
    }

    /// Sets the socket's unicast hop limit, returning the original value.
    ///
    /// If `None` is provided, the hop limit will be restored to the system
    /// default.
    pub fn set_unicast_hop_limit(
        &mut self,
        id: &RawIpApiSocketId<I, C>,
        new_limit: Option<NonZeroU8>,
    ) -> Option<NonZeroU8> {
        self.core_ctx().with_locked_state_mut(id, |state| {
            core::mem::replace(&mut state.hop_limits.unicast, new_limit)
        })
    }

    /// Gets the socket's unicast hop limit, or the system default, if unset.
    pub fn get_unicast_hop_limit(&mut self, id: &RawIpApiSocketId<I, C>) -> NonZeroU8 {
        self.core_ctx().with_locked_state(id, |state| {
            state.hop_limits.get_limits_with_defaults(&DEFAULT_HOP_LIMITS).unicast
        })
    }

    /// Sets the socket's multicast hop limit, returning the original value.
    ///
    /// If `None` is provided, the hop limit will be restored to the system
    /// default.
    pub fn set_multicast_hop_limit(
        &mut self,
        id: &RawIpApiSocketId<I, C>,
        new_limit: Option<NonZeroU8>,
    ) -> Option<NonZeroU8> {
        self.core_ctx().with_locked_state_mut(id, |state| {
            core::mem::replace(&mut state.hop_limits.multicast, new_limit)
        })
    }

    /// Gets the socket's multicast hop limit, or the system default, if unset.
    pub fn get_multicast_hop_limit(&mut self, id: &RawIpApiSocketId<I, C>) -> NonZeroU8 {
        self.core_ctx().with_locked_state(id, |state| {
            state.hop_limits.get_limits_with_defaults(&DEFAULT_HOP_LIMITS).multicast
        })
    }

    /// Sets `multicast_loop` on the socket, returning the original value.
    ///
    /// When true, the socket will loop back all sent multicast traffic.
    pub fn set_multicast_loop(&mut self, id: &RawIpApiSocketId<I, C>, value: bool) -> bool {
        self.core_ctx()
            .with_locked_state_mut(id, |state| core::mem::replace(&mut state.multicast_loop, value))
    }

    /// Gets the `multicast_loop` value on the socket.
    pub fn get_multicast_loop(&mut self, id: &RawIpApiSocketId<I, C>) -> bool {
        self.core_ctx().with_locked_state(id, |state| state.multicast_loop)
    }

    /// Sets the socket mark for the socket domain.
    pub fn set_mark(&mut self, id: &RawIpApiSocketId<I, C>, domain: MarkDomain, mark: Mark) {
        self.core_ctx().with_locked_state_mut(id, |state| {
            *state.marks.get_mut(domain) = mark;
        })
    }

    /// Gets the socket mark for the socket domain.
    pub fn get_mark(&mut self, id: &RawIpApiSocketId<I, C>, domain: MarkDomain) -> Mark {
        self.core_ctx().with_locked_state(id, |state| *state.marks.get(domain))
    }

    /// Provides inspect data for raw IP sockets.
    pub fn inspect<N>(&mut self, inspector: &mut N)
    where
        N: Inspector
            + InspectorDeviceExt<<C::CoreContext as DeviceIdContext<AnyDevice>>::WeakDeviceId>,
    {
        self.core_ctx().with_socket_map_and_state_ctx(|socket_map, core_ctx| {
            socket_map.iter_sockets().for_each(|socket| {
                inspector.record_debug_child(socket, |node| {
                    node.record_display("TransportProtocol", socket.protocol().proto());
                    node.record_str("NetworkProtocol", I::NAME);
                    // TODO(https://fxbug.dev/342579393): Support `bind`.
                    node.record_local_socket_addr::<
                        N,
                        I::Addr,
                        <C::CoreContext as DeviceIdContext<AnyDevice>>::WeakDeviceId,
                        NoPortMarker,
                    >(None);
                    // TODO(https://fxbug.dev/342577389): Support `connect`.
                    node.record_remote_socket_addr::<
                        N,
                        I::Addr,
                        <C::CoreContext as DeviceIdContext<AnyDevice>>::WeakDeviceId,
                        NoPortMarker,
                    >(None);
                    core_ctx.with_locked_state(socket, |state| {
                        let RawIpSocketLockedState {
                            bound_device,
                            icmp_filter,
                            hop_limits: _,
                            multicast_loop: _,
                            marks,
                            system_checksums: _,
                        } = state;
                        if let Some(bound_device) = bound_device {
                            N::record_device(node, "BoundDevice", bound_device);
                        } else {
                            node.record_str("BoundDevice", "None");
                        }
                        if let Some(icmp_filter) = icmp_filter {
                            node.record_display("IcmpFilter", icmp_filter);
                        } else {
                            node.record_str("IcmpFilter", "None");
                        }
                        node.delegate_inspectable(marks);
                    });
                    node.record_child("Counters", |node| {
                        node.delegate_inspectable(socket.state().counters())
                    })
                })
            })
        })
    }
}

/// Errors that may occur when calling [`RawIpSocketApi::send_to`].
#[derive(Debug, Error)]
pub enum RawIpSocketSendToError {
    /// The socket's protocol is `RawIpSocketProtocol::Raw`, which disallows
    /// `send_to` (the remote IP should be specified in the included header, not
    /// as a separate address argument).
    #[error("send_to is disallowed for raw IP socket with protocol = raw")]
    ProtocolRaw,
    /// The provided remote_ip was an IPv4-mapped-IPv6 address. Dual stack
    /// operations are not supported on raw IP sockets.
    #[error("dual-stack operations are not supported on raw IP sockets")]
    MappedRemoteIp,
    /// The provided packet body was invalid, and could not be sent. Typically
    /// originates when the stack is asked to inspect the packet body, e.g. to
    /// compute and populate the checksum value.
    #[error("provided packet body was invalid (e.g. bad checksum)")]
    InvalidBody,
    /// There was an error when resolving the remote_ip's zone.
    #[error("resolving remote IP's zone: {0}")]
    Zone(#[from] ZonedAddressError),
    /// The IP layer failed to send the packet.
    #[error("failed to send packet: {0}")]
    Ip(#[from] IpSockCreateAndSendError),
}

/// Errors that may occur getting/setting the ICMP filter for a raw IP socket.
#[derive(Debug, PartialEq, Error)]
pub enum RawIpSocketIcmpFilterError {
    /// The socket's protocol does not allow ICMP filters.
    #[error("socket's protocol does not allow ICMP filters")]
    ProtocolNotIcmp,
}

/// The owner of socket state.
struct PrimaryRawIpSocketId<I: IpExt, D: WeakDeviceIdentifier, BT: RawIpSocketsBindingsTypes>(
    PrimaryRc<RawIpSocketState<I, D, BT>>,
);

impl<I: IpExt, D: WeakDeviceIdentifier, BT: RawIpSocketsBindingsTypes> Debug
    for PrimaryRawIpSocketId<I, D, BT>
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let Self(rc) = self;
        f.debug_tuple("RawIpSocketId").field(&PrimaryRc::debug_id(rc)).finish()
    }
}

/// Reference to the state of a live socket.
#[derive(Derivative, GenericOverIp)]
#[derivative(Clone(bound = ""), Eq(bound = ""), Hash(bound = ""), PartialEq(bound = ""))]
#[generic_over_ip(I, Ip)]
pub struct RawIpSocketId<I: IpExt, D: WeakDeviceIdentifier, BT: RawIpSocketsBindingsTypes>(
    StrongRc<RawIpSocketState<I, D, BT>>,
);

impl<I: IpExt, D: WeakDeviceIdentifier, BT: RawIpSocketsBindingsTypes> RawIpSocketId<I, D, BT> {
    /// Returns [`SocketCookie`] for this socket.
    pub fn socket_cookie(&self) -> SocketCookie {
        let Self(rc) = self;
        SocketCookie::new(rc.resource_token())
    }
}

impl<I: IpExt, D: WeakDeviceIdentifier, BT: RawIpSocketsBindingsTypes> RawIpSocketId<I, D, BT> {
    /// Return the bindings state associated with this socket.
    pub fn external_state(&self) -> &BT::RawIpSocketState<I> {
        let RawIpSocketId(strong_rc) = self;
        strong_rc.external_state()
    }
    /// Return the protocol associated with this socket.
    pub fn protocol(&self) -> &RawIpSocketProtocol<I> {
        let RawIpSocketId(strong_rc) = self;
        strong_rc.protocol()
    }
    /// Downgrades this ID to a weak reference.
    pub fn downgrade(&self) -> WeakRawIpSocketId<I, D, BT> {
        let Self(rc) = self;
        WeakRawIpSocketId(StrongRc::downgrade(rc))
    }
    /// Gets the socket state.
    pub fn state(&self) -> &RawIpSocketState<I, D, BT> {
        let RawIpSocketId(strong_rc) = self;
        &*strong_rc
    }
}

impl<I: IpExt, D: WeakDeviceIdentifier, BT: RawIpSocketsBindingsTypes> Debug
    for RawIpSocketId<I, D, BT>
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let Self(rc) = self;
        f.debug_tuple("RawIpSocketId").field(&StrongRc::debug_id(rc)).finish()
    }
}

/// A weak reference to a raw IP socket.
pub struct WeakRawIpSocketId<I: IpExt, D: WeakDeviceIdentifier, BT: RawIpSocketsBindingsTypes>(
    WeakRc<RawIpSocketState<I, D, BT>>,
);

impl<I: IpExt, D: WeakDeviceIdentifier, BT: RawIpSocketsBindingsTypes> Debug
    for WeakRawIpSocketId<I, D, BT>
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let Self(rc) = self;
        f.debug_tuple("WeakRawIpSocketId").field(&WeakRc::debug_id(rc)).finish()
    }
}

/// An alias for [`RawIpSocketId`] in [`RawIpSocketApi`], for brevity.
type RawIpApiSocketId<I, C> = RawIpSocketId<
    I,
    <<C as ContextPair>::CoreContext as DeviceIdContext<AnyDevice>>::WeakDeviceId,
    <C as ContextPair>::BindingsContext,
>;

/// Provides access to the [`RawIpSocketLockedState`] for a raw IP socket.
///
/// Implementations must ensure a proper lock ordering is adhered to.
pub trait RawIpSocketStateContext<I: IpExt + FilterIpExt, BT: RawIpSocketsBindingsTypes>:
    DeviceIdContext<AnyDevice>
{
    /// The implementation of `IpSocketHandler` available after having locked
    /// the state for an individual socket.
    type SocketHandler<'a>: IpSocketHandler<I, BT, DeviceId = Self::DeviceId, WeakDeviceId = Self::WeakDeviceId>;

    /// Calls the callback with an immutable reference to the socket's locked
    /// state.
    fn with_locked_state<O, F: FnOnce(&RawIpSocketLockedState<I, Self::WeakDeviceId>) -> O>(
        &mut self,
        id: &RawIpSocketId<I, Self::WeakDeviceId, BT>,
        cb: F,
    ) -> O;

    /// Calls the callback with an immutable reference to the socket's locked
    /// state and the `SocketHandler`.
    fn with_locked_state_and_socket_handler<
        O,
        F: FnOnce(&RawIpSocketLockedState<I, Self::WeakDeviceId>, &mut Self::SocketHandler<'_>) -> O,
    >(
        &mut self,
        id: &RawIpSocketId<I, Self::WeakDeviceId, BT>,
        cb: F,
    ) -> O;

    /// Calls the callback with a mutable reference to the socket's locked
    /// state.
    fn with_locked_state_mut<
        O,
        F: FnOnce(&mut RawIpSocketLockedState<I, Self::WeakDeviceId>) -> O,
    >(
        &mut self,
        id: &RawIpSocketId<I, Self::WeakDeviceId, BT>,
        cb: F,
    ) -> O;
}

/// The collection of all raw IP sockets installed in the system.
///
/// Implementations must ensure a proper lock ordering is adhered to.
#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct RawIpSocketMap<I: IpExt, D: WeakDeviceIdentifier, BT: RawIpSocketsBindingsTypes> {
    /// All sockets installed in the system.
    ///
    /// This is a nested collection, with the outer `BTreeMap` indexable by the
    /// socket's protocol, which allows for more efficient delivery of received
    /// IP packets.
    ///
    /// NB: The inner map is a `HashMap` keyed by strong IDs, rather than an
    /// `HashSet` keyed by primary IDs, because it would be impossible to build
    /// a lookup key for the hashset (there can only ever exist 1 primary ID,
    /// which is *in* the set).
    sockets: BTreeMap<
        RawIpSocketProtocol<I>,
        HashMap<RawIpSocketId<I, D, BT>, PrimaryRawIpSocketId<I, D, BT>>,
    >,
}

impl<I: IpExt, D: WeakDeviceIdentifier, BT: RawIpSocketsBindingsTypes> RawIpSocketMap<I, D, BT> {
    fn insert(&mut self, socket: PrimaryRawIpSocketId<I, D, BT>) -> RawIpSocketId<I, D, BT> {
        let RawIpSocketMap { sockets } = self;
        let PrimaryRawIpSocketId(primary) = &socket;
        let strong = RawIpSocketId(PrimaryRc::clone_strong(primary));
        // NB: The socket must be newly inserted because there can only ever
        // be a single primary ID for a socket.
        assert!(
            sockets.entry(*strong.protocol()).or_default().insert(strong.clone(), socket).is_none()
        );
        strong
    }

    fn remove(&mut self, socket: RawIpSocketId<I, D, BT>) -> PrimaryRawIpSocketId<I, D, BT> {
        // NB: This function asserts on the presence of `protocol` in the
        // outer map, and the `socket` in the inner map.  The strong ID is
        // witness to the liveness of socket.
        let RawIpSocketMap { sockets } = self;
        let protocol = *socket.protocol();
        match sockets.entry(protocol) {
            Entry::Vacant(_) => unreachable!(
                "{socket:?} with protocol {protocol:?} must be present in the socket map"
            ),
            Entry::Occupied(mut entry) => {
                let map = entry.get_mut();
                let primary = map.remove(&socket).unwrap();
                // NB: If this was the last socket for this protocol, remove
                // the entry from the outer `BTreeMap`.
                if map.is_empty() {
                    let _: HashMap<RawIpSocketId<I, D, BT>, PrimaryRawIpSocketId<I, D, BT>> =
                        entry.remove();
                }
                primary
            }
        }
    }

    fn iter_sockets(&self) -> impl Iterator<Item = &RawIpSocketId<I, D, BT>> {
        let RawIpSocketMap { sockets } = self;
        sockets.values().flat_map(|sockets_by_protocol| sockets_by_protocol.keys())
    }

    fn iter_sockets_for_protocol(
        &self,
        protocol: &RawIpSocketProtocol<I>,
    ) -> impl Iterator<Item = &RawIpSocketId<I, D, BT>> {
        let RawIpSocketMap { sockets } = self;
        sockets.get(protocol).map(|sockets| sockets.keys()).into_iter().flatten()
    }
}

/// A type that provides access to the `RawIpSocketMap` used by the system.
pub trait RawIpSocketMapContext<I: IpExt + FilterIpExt, BT: RawIpSocketsBindingsTypes>:
    DeviceIdContext<AnyDevice>
{
    /// The implementation of `RawIpSocketStateContext` available after having
    /// accessed the system's socket map.
    type StateCtx<'a>: RawIpSocketStateContext<I, BT, DeviceId = Self::DeviceId, WeakDeviceId = Self::WeakDeviceId>
        + ResourceCounterContext<RawIpSocketId<I, Self::WeakDeviceId, BT>, RawIpSocketCounters<I>>;

    /// Calls the callback with an immutable reference to the socket map.
    fn with_socket_map_and_state_ctx<
        O,
        F: FnOnce(&RawIpSocketMap<I, Self::WeakDeviceId, BT>, &mut Self::StateCtx<'_>) -> O,
    >(
        &mut self,
        cb: F,
    ) -> O;
    /// Calls the callback with a mutable reference to the socket map.
    fn with_socket_map_mut<O, F: FnOnce(&mut RawIpSocketMap<I, Self::WeakDeviceId, BT>) -> O>(
        &mut self,
        cb: F,
    ) -> O;
}

/// A type that provides the raw IP socket functionality required by core.
pub trait RawIpSocketHandler<I: IpExt, BC>: DeviceIdContext<AnyDevice> {
    /// Deliver a received IP packet to all appropriate raw IP sockets.
    fn deliver_packet_to_raw_ip_sockets<B: SplitByteSlice>(
        &mut self,
        bindings_ctx: &mut BC,
        packet: &I::Packet<B>,
        device: &Self::DeviceId,
    );
}

impl<I, BC, CC> RawIpSocketHandler<I, BC> for CC
where
    I: IpExt + FilterIpExt,
    BC: RawIpSocketsBindingsContext<I, CC::DeviceId>,
    CC: RawIpSocketMapContext<I, BC>,
{
    fn deliver_packet_to_raw_ip_sockets<B: SplitByteSlice>(
        &mut self,
        bindings_ctx: &mut BC,
        packet: &I::Packet<B>,
        device: &CC::DeviceId,
    ) {
        let protocol = RawIpSocketProtocol::new(packet.proto());

        // NB: sockets with `RawIpSocketProtocol::Raw` are send only, and cannot
        // receive packets.
        match protocol {
            RawIpSocketProtocol::Raw => {
                debug!("received IP packet with raw protocol (IANA Reserved - 255); dropping");
                return;
            }
            RawIpSocketProtocol::Proto(_) => {}
        };

        self.with_socket_map_and_state_ctx(|socket_map, core_ctx| {
            socket_map.iter_sockets_for_protocol(&protocol).for_each(|socket| {
                match core_ctx.with_locked_state(socket, |state| {
                    check_packet_for_delivery(packet, device, state)
                }) {
                    DeliveryOutcome::Deliver => {
                        core_ctx.increment_both(&socket, |counters: &RawIpSocketCounters<I>| {
                            &counters.rx_packets
                        });
                        match bindings_ctx.receive_packet(socket, packet, device) {
                            Ok(()) => {}
                            Err(ReceivePacketError::QueueFull) => {
                                core_ctx.increment_both(
                                    &socket,
                                    |counters: &RawIpSocketCounters<I>| &counters.rx_queue_full,
                                );
                            }
                        }
                    }
                    DeliveryOutcome::WrongChecksum => {
                        core_ctx.increment_both(&socket, |counters: &RawIpSocketCounters<I>| {
                            &counters.rx_checksum_errors
                        });
                    }
                    DeliveryOutcome::WrongIcmpMessageType => {
                        core_ctx.increment_both(&socket, |counters: &RawIpSocketCounters<I>| {
                            &counters.rx_icmp_filtered
                        });
                    }
                    DeliveryOutcome::WrongDevice => {}
                }
            })
        })
    }
}

/// Represents whether an IP packet should be delivered to a socket.
enum DeliveryOutcome {
    /// The packet should be delivered.
    Deliver,
    /// Don't deliver. The packet was received on an incorrect device.
    WrongDevice,
    /// Don't deliver. The packet does not have a valid checksum.
    WrongChecksum,
    /// Don't deliver. The packet's inner ICMP message type does not pass the
    /// socket's ICMP filter.
    WrongIcmpMessageType,
}

/// Returns whether the given packet should be delivered to the given socket.
fn check_packet_for_delivery<I: IpExt, D: StrongDeviceIdentifier, B: SplitByteSlice>(
    packet: &I::Packet<B>,
    device: &D,
    socket: &RawIpSocketLockedState<I, D::Weak>,
) -> DeliveryOutcome {
    let RawIpSocketLockedState {
        bound_device,
        icmp_filter,
        hop_limits: _,
        marks: _,
        multicast_loop: _,
        system_checksums,
    } = socket;
    // Verify the received device matches the socket's bound device, if any.
    if bound_device.as_ref().is_some_and(|bound_device| bound_device != device) {
        return DeliveryOutcome::WrongDevice;
    }

    // Verify the inner message's checksum, if requested.
    // NB: The checksum was not previously validated by the IP layer, because
    // packets are delivered to raw sockets before the IP layer attempts to
    // parse the inner message.
    if *system_checksums && !checksum::has_valid_checksum::<I, B>(packet) {
        return DeliveryOutcome::WrongChecksum;
    }

    // Verify the packet passes the socket's icmp_filter, if any.
    if icmp_filter.as_ref().is_some_and(|icmp_filter| {
        // NB: If the socket has an icmp_filter, its protocol must be ICMP.
        // That means the packet must be ICMP, because we're considering
        // delivering it to this socket.
        debug_assert!(RawIpSocketProtocol::<I>::new(packet.proto()).is_icmp());
        match icmp::peek_message_type(packet.body()) {
            // NB: The peek call above will fail if 1) the body doesn't have
            // enough bytes to be an ICMP header, or if 2) the message_type from
            // the header is unrecognized. In either case, don't deliver the
            // packet. This is consistent with Linux in the first case, but not
            // the second (e.g. linux *will* deliver the packet if it has an
            // invalid ICMP message type). This divergence is not expected to be
            // problematic for clients, and as such it is kept for the improved
            // type safety when operating on a known to be valid ICMP message
            // type.
            Err(_) => true,
            Ok(message_type) => !icmp_filter.allows_type(message_type),
        }
    }) {
        return DeliveryOutcome::WrongIcmpMessageType;
    }

    DeliveryOutcome::Deliver
}

/// An implementation of [`SendOptions`] for raw IP sockets.
struct RawIpSocketOptions<'a, I: Ip> {
    hop_limits: &'a SocketHopLimits<I>,
    multicast_loop: bool,
    marks: &'a Marks,
}

impl<I: Ip> RouteResolutionOptions<I> for RawIpSocketOptions<'_, I> {
    fn transparent(&self) -> bool {
        false
    }

    fn marks(&self) -> &Marks {
        self.marks
    }
}

impl<I: IpExt> SendOptions<I> for RawIpSocketOptions<'_, I> {
    fn hop_limit(&self, destination: &SpecifiedAddr<I::Addr>) -> Option<NonZeroU8> {
        self.hop_limits.hop_limit_for_dst(destination)
    }

    fn multicast_loop(&self) -> bool {
        self.multicast_loop
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

/// A marker type capturing that raw IP sockets don't have ports.
struct NoPortMarker {}

impl Display for NoPortMarker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NoPort")
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use alloc::rc::Rc;
    use alloc::vec;
    use alloc::vec::Vec;
    use assert_matches::assert_matches;
    use core::cell::RefCell;
    use core::convert::Infallible as Never;
    use core::marker::PhantomData;
    use core::ops::DerefMut;
    use ip_test_macro::ip_test;
    use net_types::ip::{IpVersion, Ipv4, Ipv6};
    use netstack3_base::sync::{DynDebugReferences, Mutex};
    use netstack3_base::testutil::{
        FakeStrongDeviceId, FakeTxMetadata, FakeWeakDeviceId, MultipleDevicesId, TestIpExt,
    };
    use netstack3_base::{ContextProvider, CounterContext, CtxPair, NetworkSerializationContext};
    use packet::{
        Buf, InnerPacketBuilder as _, NestablePacketBuilder as _, ParseBuffer as _, Serializer as _,
    };
    use packet_formats::icmp::{
        IcmpEchoReply, IcmpMessage, IcmpPacketBuilder, IcmpZeroCode, Icmpv6MessageType,
    };
    use packet_formats::ip::{IpPacketBuilder, IpProto, IpProtoExt, Ipv6Proto};
    use packet_formats::ipv6::Ipv6Packet;
    use test_case::test_case;

    use crate::internal::socket::testutil::{FakeIpSocketCtx, InnerFakeIpSocketCtx};
    use crate::socket::testutil::FakeDeviceConfig;
    use crate::{DEFAULT_HOP_LIMITS, SendIpPacketMeta};

    #[derive(Derivative, Debug)]
    #[derivative(Default(bound = ""))]
    struct FakeExternalSocketState<D> {
        /// The collection of IP packets received on this socket.
        received_packets: Mutex<RxQueue<D>>,
    }

    #[derive(Derivative, Debug)]
    #[derivative(Default(bound = ""))]
    struct RxQueue<D> {
        packets: Vec<ReceivedIpPacket<D>>,
        #[derivative(Default(value = "usize::MAX"))]
        max_size: usize,
    }

    #[derive(Debug, PartialEq)]
    struct ReceivedIpPacket<D> {
        data: Vec<u8>,
        device: D,
    }

    #[derive(Derivative)]
    #[derivative(Default(bound = ""))]
    struct FakeBindingsCtx<D> {
        _device_id_type: PhantomData<D>,
    }

    /// State required to test raw IP sockets. Held by `FakeCoreCtx`.
    struct FakeCoreCtxState<I: IpExt, D: FakeStrongDeviceId> {
        // NB: Hold in an `Rc<RefCell<...>>` to switch to runtime borrow
        // checking. This allows us to borrow the socket map at the same time
        // as the outer `FakeCoreCtx` is mutably borrowed (Required to implement
        // `RawIpSocketMapContext::with_socket_map_and_state_ctx`).
        socket_map: Rc<RefCell<RawIpSocketMap<I, D::Weak, FakeBindingsCtx<D>>>>,
        /// An inner fake implementation of `IpSocketHandler`. By implementing
        /// `InnerFakeIpSocketCtx` below, the `FakeCoreCtx` will be eligible for
        /// a blanket impl of `IpSocketHandler`.
        ip_socket_ctx: FakeIpSocketCtx<I, D>,
        /// The aggregate counters for raw ip sockets.
        counters: RawIpSocketCounters<I>,
    }

    impl<I: IpExt, D: FakeStrongDeviceId> InnerFakeIpSocketCtx<I, D> for FakeCoreCtxState<I, D> {
        fn fake_ip_socket_ctx_mut(&mut self) -> &mut FakeIpSocketCtx<I, D> {
            &mut self.ip_socket_ctx
        }
    }

    type FakeCoreCtx<I, D> = netstack3_base::testutil::FakeCoreCtx<
        FakeCoreCtxState<I, D>,
        SendIpPacketMeta<I, D, SpecifiedAddr<<I as Ip>::Addr>>,
        D,
    >;

    impl<D: FakeStrongDeviceId> TxMetadataBindingsTypes for FakeBindingsCtx<D> {
        type TxMetadata = FakeTxMetadata;
    }

    impl<D: FakeStrongDeviceId> RawIpSocketsBindingsTypes for FakeBindingsCtx<D> {
        type RawIpSocketState<I: Ip> = FakeExternalSocketState<D>;
    }

    impl<I: IpExt, D: Copy + FakeStrongDeviceId> RawIpSocketsBindingsContext<I, D>
        for FakeBindingsCtx<D>
    {
        fn receive_packet<B: SplitByteSlice>(
            &self,
            socket: &RawIpSocketId<I, D::Weak, Self>,
            packet: &I::Packet<B>,
            device: &D,
        ) -> Result<(), ReceivePacketError> {
            let packet = ReceivedIpPacket { data: packet.to_vec(), device: *device };
            let FakeExternalSocketState { received_packets } = socket.external_state();
            let mut lock_guard = received_packets.lock();
            let RxQueue { packets, max_size } = lock_guard.deref_mut();
            if packets.len() < *max_size {
                packets.push(packet);
                Ok(())
            } else {
                Err(ReceivePacketError::QueueFull)
            }
        }
    }

    impl<I: IpExt + FilterIpExt, D: FakeStrongDeviceId>
        RawIpSocketStateContext<I, FakeBindingsCtx<D>> for FakeCoreCtx<I, D>
    {
        type SocketHandler<'a> = FakeCoreCtx<I, D>;
        fn with_locked_state<O, F: FnOnce(&RawIpSocketLockedState<I, D::Weak>) -> O>(
            &mut self,
            id: &RawIpSocketId<I, D::Weak, FakeBindingsCtx<D>>,
            cb: F,
        ) -> O {
            let RawIpSocketId(state_rc) = id;
            let guard = state_rc.locked_state().read();
            cb(&guard)
        }
        fn with_locked_state_and_socket_handler<
            O,
            F: FnOnce(&RawIpSocketLockedState<I, D::Weak>, &mut Self::SocketHandler<'_>) -> O,
        >(
            &mut self,
            id: &RawIpSocketId<I, D::Weak, FakeBindingsCtx<D>>,
            cb: F,
        ) -> O {
            let RawIpSocketId(state_rc) = id;
            let guard = state_rc.locked_state().read();
            cb(&guard, self)
        }
        fn with_locked_state_mut<O, F: FnOnce(&mut RawIpSocketLockedState<I, D::Weak>) -> O>(
            &mut self,
            id: &RawIpSocketId<I, D::Weak, FakeBindingsCtx<D>>,
            cb: F,
        ) -> O {
            let RawIpSocketId(state_rc) = id;
            let mut guard = state_rc.locked_state().write();
            cb(&mut guard)
        }
    }

    impl<I: IpExt, D: FakeStrongDeviceId> CounterContext<RawIpSocketCounters<I>> for FakeCoreCtx<I, D> {
        fn counters(&self) -> &RawIpSocketCounters<I> {
            &self.state.counters
        }
    }

    impl<I: IpExt, D: FakeStrongDeviceId>
        ResourceCounterContext<
            RawIpSocketId<I, D::Weak, FakeBindingsCtx<D>>,
            RawIpSocketCounters<I>,
        > for FakeCoreCtx<I, D>
    {
        fn per_resource_counters<'a>(
            &'a self,
            socket: &'a RawIpSocketId<I, D::Weak, FakeBindingsCtx<D>>,
        ) -> &'a RawIpSocketCounters<I> {
            socket.state().counters()
        }
    }

    impl<I: IpExt + FilterIpExt, D: FakeStrongDeviceId> RawIpSocketMapContext<I, FakeBindingsCtx<D>>
        for FakeCoreCtx<I, D>
    {
        type StateCtx<'a> = FakeCoreCtx<I, D>;
        fn with_socket_map_and_state_ctx<
            O,
            F: FnOnce(&RawIpSocketMap<I, D::Weak, FakeBindingsCtx<D>>, &mut Self::StateCtx<'_>) -> O,
        >(
            &mut self,
            cb: F,
        ) -> O {
            let socket_map = self.state.socket_map.clone();
            let borrow = socket_map.borrow();
            cb(&borrow, self)
        }
        fn with_socket_map_mut<
            O,
            F: FnOnce(&mut RawIpSocketMap<I, D::Weak, FakeBindingsCtx<D>>) -> O,
        >(
            &mut self,
            cb: F,
        ) -> O {
            cb(&mut self.state.socket_map.borrow_mut())
        }
    }

    impl<D> ContextProvider for FakeBindingsCtx<D> {
        type Context = FakeBindingsCtx<D>;
        fn context(&mut self) -> &mut Self::Context {
            self
        }
    }

    impl<D> ReferenceNotifiers for FakeBindingsCtx<D> {
        type ReferenceReceiver<T: 'static> = Never;

        type ReferenceNotifier<T: Send + 'static> = Never;

        fn new_reference_notifier<T: Send + 'static>(
            _debug_references: DynDebugReferences,
        ) -> (Self::ReferenceNotifier<T>, Self::ReferenceReceiver<T>) {
            unimplemented!("raw IP socket removal shouldn't be deferred in tests");
        }
    }

    fn new_raw_ip_socket_api<I: IpExt + TestIpExt>() -> RawIpSocketApi<
        I,
        CtxPair<FakeCoreCtx<I, MultipleDevicesId>, FakeBindingsCtx<MultipleDevicesId>>,
    > {
        // Set up all devices with a local IP and a route to the remote IP.
        let device_configs = [MultipleDevicesId::A, MultipleDevicesId::B, MultipleDevicesId::C]
            .into_iter()
            .map(|device| FakeDeviceConfig {
                device,
                local_ips: vec![I::TEST_ADDRS.local_ip],
                remote_ips: vec![I::TEST_ADDRS.remote_ip],
            });
        let state = FakeCoreCtxState {
            socket_map: Default::default(),
            ip_socket_ctx: FakeIpSocketCtx::new(device_configs),
            counters: Default::default(),
        };

        RawIpSocketApi::new(CtxPair::with_core_ctx(FakeCoreCtx::with_state(state)))
    }

    /// Arbitrary data to put inside of an IP packet.
    const IP_BODY: [u8; 10] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];

    /// Constructs a buffer containing an IP packet with sensible defaults.
    fn new_ip_packet_buf<I: IpExt + TestIpExt>(
        ip_body: &[u8],
        proto: I::Proto,
    ) -> impl AsRef<[u8]> {
        const TTL: u8 = 255;
        I::PacketBuilder::new(*I::TEST_ADDRS.local_ip, *I::TEST_ADDRS.remote_ip, TTL, proto)
            .wrap_body(ip_body.into_serializer())
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
    }

    /// Construct a buffer containing an ICMP message with sensible defaults.
    fn new_icmp_message_buf<I: IpExt + TestIpExt, M: IcmpMessage<I> + Debug>(
        message: M,
        code: M::Code,
    ) -> impl AsRef<[u8]> {
        IcmpPacketBuilder::new(*I::TEST_ADDRS.local_ip, *I::TEST_ADDRS.remote_ip, code, message)
            .wrap_body([].into_serializer())
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
    }

    #[ip_test(I)]
    #[test_case(IpProto::Udp; "UDP")]
    #[test_case(IpProto::Reserved; "IPPROTO_RAW")]
    fn create_and_close<I: IpExt + DualStackIpExt + TestIpExt + FilterIpExt>(proto: IpProto) {
        let mut api = new_raw_ip_socket_api::<I>();
        let sock = api.create(RawIpSocketProtocol::new(proto.into()), Default::default());
        let FakeExternalSocketState { received_packets: _ } = api.close(sock).into_removed();
    }

    #[ip_test(I)]
    fn set_device<I: IpExt + DualStackIpExt + TestIpExt + FilterIpExt>() {
        let mut api = new_raw_ip_socket_api::<I>();
        let sock = api.create(RawIpSocketProtocol::new(IpProto::Udp.into()), Default::default());

        assert_eq!(api.get_device(&sock), None);
        assert_eq!(api.set_device(&sock, Some(&MultipleDevicesId::A)), None);
        assert_eq!(api.get_device(&sock), Some(FakeWeakDeviceId(MultipleDevicesId::A)));
        assert_eq!(
            api.set_device(&sock, Some(&MultipleDevicesId::B)),
            Some(FakeWeakDeviceId(MultipleDevicesId::A))
        );
        assert_eq!(api.get_device(&sock), Some(FakeWeakDeviceId(MultipleDevicesId::B)));
        assert_eq!(api.set_device(&sock, None), Some(FakeWeakDeviceId(MultipleDevicesId::B)));
        assert_eq!(api.get_device(&sock), None);
    }

    #[ip_test(I)]
    fn set_icmp_filter<I: IpExt + DualStackIpExt + TestIpExt + FilterIpExt>() {
        let filter1 = RawIpSocketIcmpFilter::<I>::new([123; 32]);
        let filter2 = RawIpSocketIcmpFilter::<I>::new([234; 32]);
        let mut api = new_raw_ip_socket_api::<I>();

        let sock = api.create(RawIpSocketProtocol::new(I::ICMP_IP_PROTO), Default::default());
        assert_eq!(api.get_icmp_filter(&sock), Ok(None));
        assert_eq!(api.set_icmp_filter(&sock, Some(filter1.clone())), Ok(None));
        assert_eq!(api.get_icmp_filter(&sock), Ok(Some(filter1.clone())));
        assert_eq!(api.set_icmp_filter(&sock, Some(filter2.clone())), Ok(Some(filter1.clone())));
        assert_eq!(api.get_icmp_filter(&sock), Ok(Some(filter2.clone())));
        assert_eq!(api.set_icmp_filter(&sock, None), Ok(Some(filter2)));
        assert_eq!(api.get_icmp_filter(&sock), Ok(None));

        // Sockets created with a non ICMP protocol cannot set an ICMP filter.
        let sock = api.create(RawIpSocketProtocol::new(IpProto::Udp.into()), Default::default());
        assert_eq!(
            api.set_icmp_filter(&sock, Some(filter1)),
            Err(RawIpSocketIcmpFilterError::ProtocolNotIcmp)
        );
        assert_eq!(api.get_icmp_filter(&sock), Err(RawIpSocketIcmpFilterError::ProtocolNotIcmp));
    }

    #[ip_test(I)]
    fn set_unicast_hop_limits<I: IpExt + DualStackIpExt + TestIpExt + FilterIpExt>() {
        let mut api = new_raw_ip_socket_api::<I>();
        let sock = api.create(RawIpSocketProtocol::new(IpProto::Udp.into()), Default::default());

        let limit1 = NonZeroU8::new(1).unwrap();
        let limit2 = NonZeroU8::new(2).unwrap();

        assert_eq!(api.get_unicast_hop_limit(&sock), DEFAULT_HOP_LIMITS.unicast);
        assert_eq!(api.set_unicast_hop_limit(&sock, Some(limit1)), None);
        assert_eq!(api.get_unicast_hop_limit(&sock), limit1);
        assert_eq!(api.set_unicast_hop_limit(&sock, Some(limit2)), Some(limit1));
        assert_eq!(api.get_unicast_hop_limit(&sock), limit2);
        assert_eq!(api.set_unicast_hop_limit(&sock, None), Some(limit2));
        assert_eq!(api.get_unicast_hop_limit(&sock), DEFAULT_HOP_LIMITS.unicast);
    }

    #[ip_test(I)]
    fn set_multicast_hop_limit<I: IpExt + DualStackIpExt + TestIpExt + FilterIpExt>() {
        let mut api = new_raw_ip_socket_api::<I>();
        let sock = api.create(RawIpSocketProtocol::new(IpProto::Udp.into()), Default::default());

        let limit1 = NonZeroU8::new(1).unwrap();
        let limit2 = NonZeroU8::new(2).unwrap();

        assert_eq!(api.get_multicast_hop_limit(&sock), DEFAULT_HOP_LIMITS.multicast);
        assert_eq!(api.set_multicast_hop_limit(&sock, Some(limit1)), None);
        assert_eq!(api.get_multicast_hop_limit(&sock), limit1);
        assert_eq!(api.set_multicast_hop_limit(&sock, Some(limit2)), Some(limit1));
        assert_eq!(api.get_multicast_hop_limit(&sock), limit2);
        assert_eq!(api.set_multicast_hop_limit(&sock, None), Some(limit2));
        assert_eq!(api.get_multicast_hop_limit(&sock), DEFAULT_HOP_LIMITS.multicast);
    }

    #[ip_test(I)]
    fn set_multicast_loop<I: IpExt + DualStackIpExt + TestIpExt + FilterIpExt>() {
        let mut api = new_raw_ip_socket_api::<I>();
        let sock = api.create(RawIpSocketProtocol::new(IpProto::Udp.into()), Default::default());

        // NB: multicast loopback is enabled by default.
        assert_eq!(api.get_multicast_loop(&sock), true);
        assert_eq!(api.set_multicast_loop(&sock, false), true);
        assert_eq!(api.get_multicast_loop(&sock), false);
        assert_eq!(api.set_multicast_loop(&sock, true), false);
        assert_eq!(api.get_multicast_loop(&sock), true);
    }

    #[ip_test(I)]
    fn receive_ip_packet<I: IpExt + DualStackIpExt + TestIpExt + FilterIpExt>() {
        let mut api = new_raw_ip_socket_api::<I>();

        // Create two sockets with the right protocol, and one socket with the
        // wrong protocol.
        let proto: I::Proto = IpProto::Udp.into();
        let wrong_proto: I::Proto = IpProto::Tcp.into();
        let sock1 = api.create(RawIpSocketProtocol::new(proto), Default::default());
        let sock2 = api.create(RawIpSocketProtocol::new(proto), Default::default());
        let wrong_sock = api.create(RawIpSocketProtocol::new(wrong_proto), Default::default());

        // Receive an IP packet with protocol `proto`.
        const DEVICE: MultipleDevicesId = MultipleDevicesId::A;
        let buf = new_ip_packet_buf::<I>(&IP_BODY, proto);
        let mut buf_ref = buf.as_ref();
        let packet = buf_ref.parse::<I::Packet<_>>().expect("parse should succeed");
        {
            let (core_ctx, bindings_ctx) = api.ctx.contexts();
            core_ctx.deliver_packet_to_raw_ip_sockets(bindings_ctx, &packet, &DEVICE);
        }

        // Verify the counters were updated.
        assert_eq!(api.core_ctx().state.counters.rx_packets.get(), 2);
        assert_eq!(sock1.state().counters().rx_packets.get(), 1);
        assert_eq!(sock2.state().counters().rx_packets.get(), 1);
        assert_eq!(wrong_sock.state().counters().rx_packets.get(), 0);

        let FakeExternalSocketState { received_packets: sock1_packets } =
            api.close(sock1).into_removed();
        let FakeExternalSocketState { received_packets: sock2_packets } =
            api.close(sock2).into_removed();
        let FakeExternalSocketState { received_packets: wrong_sock_packets } =
            api.close(wrong_sock).into_removed();

        // Expect delivery to the two right sockets, but not the wrong socket.
        for packets in [sock1_packets, sock2_packets] {
            let lock_guard = packets.lock();
            let ReceivedIpPacket { data, device } =
                assert_matches!(&lock_guard.packets[..], [packet] => packet);
            assert_eq!(&data[..], buf.as_ref());
            assert_eq!(*device, DEVICE);
        }
        assert_matches!(&wrong_sock_packets.lock().packets[..], []);
    }

    // Verify that sockets created with `RawIpSocketProtocol::Raw` cannot
    // receive packets
    #[ip_test(I)]
    fn cannot_receive_ip_packet_with_proto_raw<
        I: IpExt + DualStackIpExt + TestIpExt + FilterIpExt,
    >() {
        let mut api = new_raw_ip_socket_api::<I>();
        let sock = api.create(RawIpSocketProtocol::Raw, Default::default());

        // Try to deliver to an arbitrary proto (UDP), and to the reserved
        // proto; neither should be delivered to the socket.
        let protocols_to_test = match I::VERSION {
            IpVersion::V4 => vec![IpProto::Udp, IpProto::Reserved],
            // NB: Don't test `Reserved` with IPv6; the packet will fail to
            // parse.
            IpVersion::V6 => vec![IpProto::Udp],
        };
        for proto in protocols_to_test {
            let buf = new_ip_packet_buf::<I>(&IP_BODY, proto.into());
            let mut buf_ref = buf.as_ref();
            let packet = buf_ref.parse::<I::Packet<_>>().expect("parse should succeed");
            let (core_ctx, bindings_ctx) = api.ctx.contexts();
            core_ctx.deliver_packet_to_raw_ip_sockets(bindings_ctx, &packet, &MultipleDevicesId::A);
        }

        let FakeExternalSocketState { received_packets } = api.close(sock).into_removed();
        assert_matches!(&received_packets.lock().packets[..], []);
    }

    #[ip_test(I)]
    #[test_case(MultipleDevicesId::A, None, true; "no_bound_device")]
    #[test_case(MultipleDevicesId::A, Some(MultipleDevicesId::A), true; "bound_same_device")]
    #[test_case(MultipleDevicesId::A, Some(MultipleDevicesId::B), false; "bound_diff_device")]
    fn receive_ip_packet_with_bound_device<I: IpExt + DualStackIpExt + TestIpExt + FilterIpExt>(
        send_dev: MultipleDevicesId,
        bound_dev: Option<MultipleDevicesId>,
        should_deliver: bool,
    ) {
        const PROTO: IpProto = IpProto::Udp;
        let mut api = new_raw_ip_socket_api::<I>();
        let sock = api.create(RawIpSocketProtocol::new(PROTO.into()), Default::default());

        assert_eq!(api.set_device(&sock, bound_dev.as_ref()), None);

        // Deliver an arbitrary packet on `send_dev`.
        let buf = new_ip_packet_buf::<I>(&IP_BODY, PROTO.into());
        let mut buf_ref = buf.as_ref();
        let packet = buf_ref.parse::<I::Packet<_>>().expect("parse should succeed");
        {
            let (core_ctx, bindings_ctx) = api.ctx.contexts();
            core_ctx.deliver_packet_to_raw_ip_sockets(bindings_ctx, &packet, &send_dev);
        }

        // Verify the packet was/wasn't received, as expected.
        let FakeExternalSocketState { received_packets } = api.close(sock).into_removed();
        if should_deliver {
            let lock_guard = received_packets.lock();
            let ReceivedIpPacket { data, device } =
                assert_matches!(&lock_guard.packets[..], [packet] => packet);
            assert_eq!(&data[..], buf.as_ref());
            assert_eq!(*device, send_dev);
        } else {
            assert_matches!(&received_packets.lock().packets[..], []);
        }
    }

    #[ip_test(I)]
    // NB: Don't bother testing for individual ICMP codes. The `filter` sub
    // module already covers that extensively.
    #[test_case(None, true; "no_filter")]
    #[test_case(Some(RawIpSocketIcmpFilter::<I>::ALLOW_ALL), true; "allow_all")]
    #[test_case(Some(RawIpSocketIcmpFilter::<I>::DENY_ALL), false; "deny_all")]
    fn receive_ip_packet_with_icmp_filter<I: IpExt + DualStackIpExt + TestIpExt + FilterIpExt>(
        filter: Option<RawIpSocketIcmpFilter<I>>,
        should_deliver: bool,
    ) {
        let mut api = new_raw_ip_socket_api::<I>();
        let sock = api.create(RawIpSocketProtocol::new(I::ICMP_IP_PROTO), Default::default());

        let assert_counters = |core_ctx: &mut FakeCoreCtx<_, _>, count: u64| {
            assert_eq!(core_ctx.state.counters.rx_icmp_filtered.get(), count);
            assert_eq!(sock.state().counters().rx_icmp_filtered.get(), count);
        };
        assert_counters(api.core_ctx(), 0);

        assert_matches!(api.set_icmp_filter(&sock, filter), Ok(None));

        // Deliver an arbitrary ICMP message.
        let icmp_body = new_icmp_message_buf::<I, _>(IcmpEchoReply::new(0, 0), IcmpZeroCode);
        let buf = new_ip_packet_buf::<I>(icmp_body.as_ref(), I::ICMP_IP_PROTO);
        let mut buf_ref = buf.as_ref();
        let packet = buf_ref.parse::<I::Packet<_>>().expect("parse should succeed");
        {
            let (core_ctx, bindings_ctx) = api.ctx.contexts();
            core_ctx.deliver_packet_to_raw_ip_sockets(bindings_ctx, &packet, &MultipleDevicesId::A);
        }

        // Verify the packet was/wasn't received, as expected.
        assert_counters(api.core_ctx(), should_deliver.then_some(0).unwrap_or(1));
        let FakeExternalSocketState { received_packets } = api.close(sock).into_removed();
        if should_deliver {
            let lock_guard = received_packets.lock();
            let ReceivedIpPacket { data, device: _ } =
                assert_matches!(&lock_guard.packets[..], [packet] => packet);
            assert_eq!(&data[..], buf.as_ref());
        } else {
            assert_matches!(&received_packets.lock().packets[..], []);
        }
    }

    // Verify that ICMPv6 messages with an invalid checksum won't be received.
    // Note that the successful delivery case is tested by
    // `receive_ip_packet_with_icmp_filter`.
    #[test]
    fn do_not_receive_icmpv6_packet_with_bad_checksum() {
        let mut api = new_raw_ip_socket_api::<Ipv6>();
        let sock = api.create(RawIpSocketProtocol::new(Ipv6Proto::Icmpv6), Default::default());

        let assert_counters = |core_ctx: &mut FakeCoreCtx<_, _>, count: u64| {
            assert_eq!(core_ctx.state.counters.rx_checksum_errors.get(), count);
            assert_eq!(sock.state().counters().rx_checksum_errors.get(), count);
        };
        assert_counters(api.core_ctx(), 0);

        // Use a valid ICMP message, but intentionally corrupt the checksum.
        // The checksum is present at bytes 2 & 3.
        let mut icmp_body = new_icmp_message_buf::<Ipv6, _>(IcmpEchoReply::new(0, 0), IcmpZeroCode)
            .as_ref()
            .to_vec();
        const CORRUPT_CHECKSUM: [u8; 2] = [123, 234];
        assert_ne!(
            packet_formats::testutil::overwrite_icmpv6_checksum(
                icmp_body.as_mut(),
                CORRUPT_CHECKSUM
            )
            .expect("parse should succeed"),
            CORRUPT_CHECKSUM
        );

        let buf = new_ip_packet_buf::<Ipv6>(icmp_body.as_ref(), Ipv6Proto::Icmpv6);
        let mut buf_ref = buf.as_ref();
        let packet = buf_ref.parse::<Ipv6Packet<_>>().expect("parse should succeed");
        {
            let (core_ctx, bindings_ctx) = api.ctx.contexts();
            core_ctx.deliver_packet_to_raw_ip_sockets(bindings_ctx, &packet, &MultipleDevicesId::A);
        }

        // Verify the packet wasn't received.
        assert_counters(api.core_ctx(), 1);
        let FakeExternalSocketState { received_packets } = api.close(sock).into_removed();
        assert_matches!(&received_packets.lock().packets[..], []);
    }

    #[ip_test(I)]
    fn receive_ip_packet_queue_full<I: IpExt + DualStackIpExt + TestIpExt + FilterIpExt>() {
        let mut api = new_raw_ip_socket_api::<I>();

        let proto: I::Proto = IpProto::Udp.into();
        // Simulate a full RX queue for sock1.
        let sock1 = api.create(
            RawIpSocketProtocol::new(proto),
            FakeExternalSocketState {
                received_packets: Mutex::new(RxQueue { packets: vec![], max_size: 0 }),
            },
        );
        let sock2 = api.create(RawIpSocketProtocol::new(proto), Default::default());

        // Receive an IP packet with protocol `proto`.
        const DEVICE: MultipleDevicesId = MultipleDevicesId::A;
        let buf = new_ip_packet_buf::<I>(&IP_BODY, proto);
        let mut buf_ref = buf.as_ref();
        let packet = buf_ref.parse::<I::Packet<_>>().expect("parse should succeed");
        {
            let (core_ctx, bindings_ctx) = api.ctx.contexts();
            core_ctx.deliver_packet_to_raw_ip_sockets(bindings_ctx, &packet, &DEVICE);
        }

        // Verify the counters were updated.
        assert_eq!(api.core_ctx().state.counters.rx_packets.get(), 2);
        assert_eq!(api.core_ctx().state.counters.rx_queue_full.get(), 1);
        assert_eq!(sock1.state().counters().rx_packets.get(), 1);
        assert_eq!(sock1.state().counters().rx_queue_full.get(), 1);
        assert_eq!(sock2.state().counters().rx_packets.get(), 1);
        assert_eq!(sock2.state().counters().rx_queue_full.get(), 0);
    }

    #[ip_test(I)]
    #[test_case(None, None; "default_send")]
    #[test_case(Some(MultipleDevicesId::A), None; "with_bound_dev")]
    #[test_case(None, Some(123); "with_hop_limit")]
    fn send_to<I: IpExt + DualStackIpExt + TestIpExt + FilterIpExt>(
        bound_dev: Option<MultipleDevicesId>,
        hop_limit: Option<u8>,
    ) {
        const PROTO: IpProto = IpProto::Udp;
        let mut api = new_raw_ip_socket_api::<I>();
        let sock = api.create(RawIpSocketProtocol::new(PROTO.into()), Default::default());

        let assert_counters = |core_ctx: &mut FakeCoreCtx<_, _>, count: u64| {
            assert_eq!(core_ctx.state.counters.tx_packets.get(), count);
            assert_eq!(sock.state().counters().tx_packets.get(), count);
        };
        assert_counters(api.core_ctx(), 0);

        assert_eq!(api.set_device(&sock, bound_dev.as_ref()), None);
        let hop_limit = hop_limit.and_then(NonZeroU8::new);
        assert_eq!(api.set_unicast_hop_limit(&sock, hop_limit), None);

        let remote_ip = ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip);
        assert_matches!(&api.ctx.core_ctx().take_frames()[..], []);
        api.send_to(&sock, Some(remote_ip), Buf::new(IP_BODY.to_vec(), ..))
            .expect("send should succeed");
        let frames = api.core_ctx().take_frames();
        let (SendIpPacketMeta { device, src_ip, dst_ip, proto, mtu, ttl, .. }, data) =
            assert_matches!( &frames[..], [packet] => packet);
        assert_eq!(&data[..], &IP_BODY[..]);
        assert_eq!(*dst_ip, remote_ip.addr());
        assert_eq!(*src_ip, I::TEST_ADDRS.local_ip);
        if let Some(bound_dev) = bound_dev {
            assert_eq!(*device, bound_dev);
        }
        assert_eq!(*proto, <I as IpProtoExt>::Proto::from(PROTO));
        assert_eq!(*mtu, Mtu::max());
        assert_eq!(*ttl, hop_limit);

        assert_counters(api.core_ctx(), 1);
    }

    #[ip_test(I)]
    fn send_to_disallows_raw_protocol<I: IpExt + DualStackIpExt + TestIpExt + FilterIpExt>() {
        let mut api = new_raw_ip_socket_api::<I>();
        let sock = api.create(RawIpSocketProtocol::Raw, Default::default());
        assert_matches!(
            api.send_to(&sock, None, Buf::new(IP_BODY.to_vec(), ..)),
            Err(RawIpSocketSendToError::ProtocolRaw)
        );
    }

    #[test]
    fn send_to_disallows_dualstack() {
        let mut api = new_raw_ip_socket_api::<Ipv6>();
        let sock = api.create(RawIpSocketProtocol::new(IpProto::Udp.into()), Default::default());
        let mapped_remote_ip = ZonedAddr::Unzoned(Ipv4::TEST_ADDRS.local_ip.to_ipv6_mapped());
        assert_matches!(
            api.send_to(&sock, Some(mapped_remote_ip), Buf::new(IP_BODY.to_vec(), ..)),
            Err(RawIpSocketSendToError::MappedRemoteIp)
        );
    }

    // Verify that packets sent on ICMPv6 raw IP sockets have their checksum
    // automatically populated by the netstack.
    #[test]
    fn icmpv6_send_to_generates_checksum() {
        let mut api = new_raw_ip_socket_api::<Ipv6>();
        let sock = api.create(RawIpSocketProtocol::new(Ipv6Proto::Icmpv6), Default::default());

        // Use a valid ICMP body, but intentionally corrupt the checksum.
        // The checksum is present at bytes 2 & 3.
        let icmp_body_with_checksum =
            new_icmp_message_buf::<Ipv6, _>(IcmpEchoReply::new(0, 0), IcmpZeroCode)
                .as_ref()
                .to_vec();
        const CORRUPT_CHECKSUM: [u8; 2] = [123, 234];
        let mut icmp_body_without_checksum = icmp_body_with_checksum.clone();
        assert_ne!(
            packet_formats::testutil::overwrite_icmpv6_checksum(
                icmp_body_without_checksum.as_mut(),
                CORRUPT_CHECKSUM,
            )
            .expect("parse should succeed"),
            CORRUPT_CHECKSUM
        );

        // Send the buffer that has the corrupt checksum.
        let remote_ip = ZonedAddr::Unzoned(Ipv6::TEST_ADDRS.remote_ip);
        assert_matches!(&api.ctx.core_ctx().take_frames()[..], []);
        api.send_to(&sock, Some(remote_ip), Buf::new(icmp_body_without_checksum.to_vec(), ..))
            .expect("send should succeed");

        // Observe that the checksum is populated.
        let frames = api.core_ctx().take_frames();
        let (_send_ip_packet_meta, data) = assert_matches!( &frames[..], [packet] => packet);
        assert_eq!(&data[..], icmp_body_with_checksum);
    }

    // Verify that invalid ICMPv6 packets cannot be sent on raw IP sockets.
    #[test_case(Icmpv6MessageType::DestUnreachable.into(), 4; "header-too-short")]
    #[test_case(0, 8; "message-type-zero-not-supported")]
    fn icmpv6_send_to_invalid_body(message_type: u8, header_len: usize) {
        let mut api = new_raw_ip_socket_api::<Ipv6>();
        let sock = api.create(RawIpSocketProtocol::new(Ipv6Proto::Icmpv6), Default::default());

        let assert_counters = |core_ctx: &mut FakeCoreCtx<_, _>, count: u64| {
            assert_eq!(core_ctx.state.counters.tx_checksum_errors.get(), count);
            assert_eq!(sock.state().counters().tx_checksum_errors.get(), count);
        };

        let mut body = vec![0; header_len];
        body[0] = message_type;
        assert_counters(api.core_ctx(), 0);

        let remote_ip = ZonedAddr::Unzoned(Ipv6::TEST_ADDRS.remote_ip);
        assert_matches!(
            api.send_to(&sock, Some(remote_ip), Buf::new(body, ..)),
            Err(RawIpSocketSendToError::InvalidBody)
        );

        assert_counters(api.core_ctx(), 1);
    }

    #[ip_test(I, test = false)]
    #[test_case::test_matrix(
        [MarkDomain::Mark1, MarkDomain::Mark2],
        [None, Some(0), Some(1)]
    )]
    fn raw_ip_socket_marks<I: TestIpExt + DualStackIpExt + IpExt + FilterIpExt>(
        domain: MarkDomain,
        mark: Option<u32>,
    ) {
        let mut api = new_raw_ip_socket_api::<I>();
        let socket = api.create(RawIpSocketProtocol::Raw, Default::default());

        // Doesn't have a mark by default.
        assert_eq!(api.get_mark(&socket, domain), Mark(None));

        let mark = Mark(mark);
        // We can set and get back the mark.
        api.set_mark(&socket, domain, mark);
        assert_eq!(api.get_mark(&socket, domain), mark);
    }
}
