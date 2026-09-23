// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Defines how TCP state machines are used for TCP sockets.
//!
//! TCP state machine implemented in the parent module aims to only implement
//! RFC 793 which lacks posix semantics.
//!
//! To actually support posix-style sockets:
//! We would need two kinds of active sockets, listeners/connections (or
//! server sockets/client sockets; both are not very accurate terms, the key
//! difference is that the former has only local addresses but the later has
//! remote addresses in addition). [`Connection`]s are backed by a state
//! machine, however the state can be in any state. [`Listener`]s don't have
//! state machines, but they create [`Connection`]s that are backed by
//! [`State::Listen`] an incoming SYN and keep track of whether the connection
//! is established.

pub(crate) mod accept_queue;
pub(crate) mod demux;
pub(crate) mod diagnostics;
pub(crate) mod generators;

use alloc::vec::Vec;
use core::convert::Infallible as Never;
use core::fmt::{self, Debug};
use core::marker::PhantomData;
use core::num::{NonZeroU16, NonZeroUsize};
use core::ops::{Deref, DerefMut, RangeInclusive};

use assert_matches::assert_matches;
use derivative::Derivative;
use lock_order::lock::{OrderedLockAccess, OrderedLockRef};
use log::{debug, error};
use net_types::ip::{
    GenericOverIp, Ip, IpAddr, IpAddress, IpVersion, IpVersionMarker, Ipv4, Ipv4Addr, Ipv6,
    Ipv6Addr,
};
use net_types::{
    AddrAndPortFormatter, AddrAndZone, MulticastAddress as _, SpecifiedAddr, ZonedAddr,
};
use netstack3_base::socket::{
    self, AddrIsMappedError, AddrVec, Bound, ConnAddr, ConnIpAddr, DualStackListenerIpAddr,
    DualStackLocalIp, DualStackRemoteIp, DualStackTuple, EitherIpProto, EitherStack,
    IncompatibleError, InsertError, Inserter, ListenerAddr, ListenerAddrInfo, ListenerIpAddr,
    MaybeDualStack, NotDualStackCapableError, RemoveResult, SetDualStackEnabledError, ShutdownType,
    SocketCookie, SocketDeviceUpdate, SocketDeviceUpdateNotAllowedError, SocketIpAddr, SocketIpExt,
    SocketMapAddrSpec, SocketMapAddrStateSpec, SocketMapAddrStateUpdateSharingSpec,
    SocketMapConflictPolicy, SocketMapStateSpec, SocketMapUpdateSharingPolicy,
    SocketZonedAddrExt as _, UpdateSharingError,
};
use netstack3_base::socketmap::{IterShadows as _, SocketMap, Tagged as _};
use netstack3_base::sync::RwLock;
use netstack3_base::{
    AnyDevice, BidirectionalConverter as _, ContextPair, Control, CoreTimerContext,
    CoreTxMetadataContext, CtxPair, DeferredResourceRemovalContext, DeviceIdContext,
    EitherDeviceId, ExistsError, HandleableTimer, IcmpErrorCode, Inspector, InspectorDeviceExt,
    InspectorExt, InstantBindingsTypes, InstantContext, IpDeviceAddr, IpExt,
    IpSocketPropertiesMatcher, LocalAddressError, Mark, MarkDomain, Marks, MatcherBindingsTypes,
    Mss, OwnedOrRefsBidirectionalConverter, PortAllocImpl, ReferenceNotifiers,
    ReferenceNotifiersExt as _, RemoveResourceResultWithContext, ResourceCounterContext as _,
    RngContext, Segment, SeqNum, SettingsContext, SocketDiagnosticsSeed, StrongDeviceIdentifier,
    TimerBindingsTypes, TimerContext, TxMetadataBindingsTypes, WeakDeviceIdentifier,
    ZonedAddressError,
};
use netstack3_filter::{FilterIpExt, SocketOpsFilterBindingContext, Tuple};
use netstack3_hashmap::{HashMap, hash_map};
use netstack3_ip::socket::{
    DeviceIpSocketHandler, IpSock, IpSockCreateAndSendError, IpSockCreationError, IpSocketArgs,
    IpSocketHandler,
};
use netstack3_ip::{
    self as ip, BaseTransportIpContext, IpLayerIpExt, SocketMetadata, TransportIpContext,
};
use netstack3_trace::{TraceResourceId, trace_duration};
use packet_formats::ip::{IpProto, Ipv4Proto, Ipv6Proto};
use smallvec::{SmallVec, smallvec};
use thiserror::Error;

use crate::internal::base::{
    BufferSizes, BuffersRefMut, ConnectionError, SocketOptions, TcpIpSockOptions,
    TcpSocketTxMetadata,
};
use crate::internal::buffer::{Buffer, IntoBuffers, ReceiveBuffer, SendBuffer};
use crate::internal::counters::{
    self, CombinedTcpCounters, TcpCounterContext, TcpCountersRefs, TcpCountersWithSocket,
};
use crate::internal::settings::TcpSettings;
use crate::internal::socket::accept_queue::{AcceptQueue, ListenerNotifier};
use crate::internal::socket::demux::tcp_serialize_segment;
use crate::internal::socket::diagnostics::{
    TcpSocketDiagnostics, TcpSocketDiagnosticsSeed, TcpSocketStateForMatching,
};

use crate::internal::socket::generators::{IsnGenerator, TimestampOffsetGenerator};
use crate::internal::state::info::TcpSocketInfo;
use crate::internal::state::{
    CloseError, CloseReason, Closed, Initial, NewlyClosed, ShouldRetransmit, State,
    StateMachineDebugId, Takeable, TakeableRef,
};

/// A marker trait for dual-stack socket features.
///
/// This trait acts as a marker for [`DualStackBaseIpExt`] for both `Self` and
/// `Self::OtherVersion`.
pub trait DualStackIpExt:
    DualStackBaseIpExt + netstack3_base::socket::DualStackIpExt<OtherVersion: DualStackBaseIpExt>
{
}

impl<I> DualStackIpExt for I where
    I: DualStackBaseIpExt
        + netstack3_base::socket::DualStackIpExt<OtherVersion: DualStackBaseIpExt>
{
}

/// A dual stack IP extension trait for TCP.
pub trait DualStackBaseIpExt:
    netstack3_base::socket::DualStackIpExt + SocketIpExt + IpLayerIpExt
{
    /// For `Ipv4`, this is [`EitherStack<TcpSocketId<Ipv4, _, _>, TcpSocketId<Ipv6, _, _>>`],
    /// and for `Ipv6` it is just `TcpSocketId<Ipv6>`.
    type DemuxSocketId<D: WeakDeviceIdentifier, BT: TcpBindingsTypes>: SpecSocketId;

    /// The type for a connection, for [`Ipv4`], this will be just the single
    /// stack version of the connection state and the connection address. For
    /// [`Ipv6`], this will be a `EitherStack`.
    type ConnectionAndAddr<D: WeakDeviceIdentifier, BT: TcpBindingsTypes>: Send + Sync + Debug;

    /// The type for the address that the listener is listening on. This should
    /// be just [`ListenerIpAddr`] for [`Ipv4`], but a [`DualStackListenerIpAddr`]
    /// for [`Ipv6`].
    type ListenerIpAddr: Send + Sync + Debug + Clone;

    /// The type for the original destination address of a connection. For
    /// [`Ipv4`], this is always an [`Ipv4Addr`], and for [`Ipv6`], it is an
    /// [`EitherStack<Ipv6Addr, Ipv4Addr>`].
    type OriginalDstAddr;

    /// IP options unique to a particular IP version.
    type DualStackIpOptions: Send + Sync + Debug + Default + Clone + Copy;

    /// Determines which stack the demux socket ID belongs to and converts
    /// (by reference) to a dual stack TCP socket ID.
    fn as_dual_stack_ip_socket<D: WeakDeviceIdentifier, BT: TcpBindingsTypes>(
        id: &Self::DemuxSocketId<D, BT>,
    ) -> EitherStack<&TcpSocketId<Self, D, BT>, &TcpSocketId<Self::OtherVersion, D, BT>>
    where
        Self::OtherVersion: DualStackBaseIpExt;

    /// Determines which stack the demux socket ID belongs to and converts
    /// (by value) to a dual stack TCP socket ID.
    fn into_dual_stack_ip_socket<D: WeakDeviceIdentifier, BT: TcpBindingsTypes>(
        id: Self::DemuxSocketId<D, BT>,
    ) -> EitherStack<TcpSocketId<Self, D, BT>, TcpSocketId<Self::OtherVersion, D, BT>>
    where
        Self::OtherVersion: DualStackBaseIpExt;

    /// Turns a [`TcpSocketId`] of the current stack into the demuxer ID.
    fn into_demux_socket_id<D: WeakDeviceIdentifier, BT: TcpBindingsTypes>(
        id: TcpSocketId<Self, D, BT>,
    ) -> Self::DemuxSocketId<D, BT>
    where
        Self::OtherVersion: DualStackBaseIpExt;

    fn get_conn_info<D: WeakDeviceIdentifier, BT: TcpBindingsTypes>(
        conn_and_addr: &Self::ConnectionAndAddr<D, BT>,
    ) -> ConnectionInfo<Self::Addr, D>;
    fn get_accept_queue_mut<D: WeakDeviceIdentifier, BT: TcpBindingsTypes>(
        conn_and_addr: &mut Self::ConnectionAndAddr<D, BT>,
    ) -> &mut Option<
        AcceptQueue<
            TcpSocketId<Self, D, BT>,
            BT::ReturnedBuffers,
            BT::ListenerNotifierOrProvidedBuffers,
        >,
    >
    where
        Self::OtherVersion: DualStackBaseIpExt;
    fn get_defunct<D: WeakDeviceIdentifier, BT: TcpBindingsTypes>(
        conn_and_addr: &Self::ConnectionAndAddr<D, BT>,
    ) -> bool;
    fn get_state<D: WeakDeviceIdentifier, BT: TcpBindingsTypes>(
        conn_and_addr: &Self::ConnectionAndAddr<D, BT>,
    ) -> &State<BT::Instant, BT::ReceiveBuffer, BT::SendBuffer, BT::ListenerNotifierOrProvidedBuffers>;
    fn get_bound_info<D: WeakDeviceIdentifier>(
        listener_addr: &ListenerAddr<Self::ListenerIpAddr, D>,
    ) -> BoundInfo<Self::Addr, D>;

    fn destroy_socket_with_demux_id<
        CC: TcpContext<Self, BC> + TcpContext<Self::OtherVersion, BC>,
        BC: TcpBindingsContext<CC::DeviceId>,
    >(
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        demux_id: Self::DemuxSocketId<CC::WeakDeviceId, BC>,
    ) where
        Self::OtherVersion: DualStackBaseIpExt;

    /// Take the original destination of the socket's connection and return an
    /// address that is always in this socket's stack. For [`Ipv4`], this is a
    /// no-op, but for [`Ipv6`] it may require mapping a dual-stack IPv4 address
    /// into the IPv6 address space.
    fn get_original_dst(addr: Self::OriginalDstAddr) -> Self::Addr;
}

impl DualStackBaseIpExt for Ipv4 {
    type DemuxSocketId<D: WeakDeviceIdentifier, BT: TcpBindingsTypes> =
        EitherStack<TcpSocketId<Ipv4, D, BT>, TcpSocketId<Ipv6, D, BT>>;
    type ConnectionAndAddr<D: WeakDeviceIdentifier, BT: TcpBindingsTypes> =
        (Connection<Ipv4, Ipv4, D, BT>, ConnAddr<ConnIpAddr<Ipv4Addr, NonZeroU16, NonZeroU16>, D>);
    type ListenerIpAddr = ListenerIpAddr<Ipv4Addr, NonZeroU16>;
    type OriginalDstAddr = Ipv4Addr;
    type DualStackIpOptions = ();

    fn as_dual_stack_ip_socket<D: WeakDeviceIdentifier, BT: TcpBindingsTypes>(
        id: &Self::DemuxSocketId<D, BT>,
    ) -> EitherStack<&TcpSocketId<Self, D, BT>, &TcpSocketId<Self::OtherVersion, D, BT>> {
        match id {
            EitherStack::ThisStack(id) => EitherStack::ThisStack(id),
            EitherStack::OtherStack(id) => EitherStack::OtherStack(id),
        }
    }
    fn into_dual_stack_ip_socket<D: WeakDeviceIdentifier, BT: TcpBindingsTypes>(
        id: Self::DemuxSocketId<D, BT>,
    ) -> EitherStack<TcpSocketId<Self, D, BT>, TcpSocketId<Self::OtherVersion, D, BT>> {
        id
    }
    fn into_demux_socket_id<D: WeakDeviceIdentifier, BT: TcpBindingsTypes>(
        id: TcpSocketId<Self, D, BT>,
    ) -> Self::DemuxSocketId<D, BT> {
        EitherStack::ThisStack(id)
    }
    fn get_conn_info<D: WeakDeviceIdentifier, BT: TcpBindingsTypes>(
        (_conn, addr): &Self::ConnectionAndAddr<D, BT>,
    ) -> ConnectionInfo<Self::Addr, D> {
        addr.clone().into()
    }
    fn get_accept_queue_mut<D: WeakDeviceIdentifier, BT: TcpBindingsTypes>(
        (conn, _addr): &mut Self::ConnectionAndAddr<D, BT>,
    ) -> &mut Option<
        AcceptQueue<
            TcpSocketId<Self, D, BT>,
            BT::ReturnedBuffers,
            BT::ListenerNotifierOrProvidedBuffers,
        >,
    > {
        &mut conn.accept_queue
    }
    fn get_defunct<D: WeakDeviceIdentifier, BT: TcpBindingsTypes>(
        (conn, _addr): &Self::ConnectionAndAddr<D, BT>,
    ) -> bool {
        conn.defunct
    }
    fn get_state<D: WeakDeviceIdentifier, BT: TcpBindingsTypes>(
        (conn, _addr): &Self::ConnectionAndAddr<D, BT>,
    ) -> &State<BT::Instant, BT::ReceiveBuffer, BT::SendBuffer, BT::ListenerNotifierOrProvidedBuffers>
    {
        &conn.state
    }
    fn get_bound_info<D: WeakDeviceIdentifier>(
        listener_addr: &ListenerAddr<Self::ListenerIpAddr, D>,
    ) -> BoundInfo<Self::Addr, D> {
        listener_addr.clone().into()
    }

    fn destroy_socket_with_demux_id<
        CC: TcpContext<Self, BC> + TcpContext<Self::OtherVersion, BC>,
        BC: TcpBindingsContext<CC::DeviceId>,
    >(
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        demux_id: Self::DemuxSocketId<CC::WeakDeviceId, BC>,
    ) {
        match demux_id {
            EitherStack::ThisStack(id) => destroy_socket(core_ctx, bindings_ctx, id),
            EitherStack::OtherStack(id) => destroy_socket(core_ctx, bindings_ctx, id),
        }
    }

    fn get_original_dst(addr: Self::OriginalDstAddr) -> Self::Addr {
        addr
    }
}

/// Socket options that are accessible on IPv6 sockets.
#[derive(Derivative, Debug, Clone, Copy, PartialEq, Eq)]
#[derivative(Default)]
pub struct Ipv6Options {
    /// True if this socket has dual stack enabled.
    #[derivative(Default(value = "true"))]
    pub dual_stack_enabled: bool,
}

impl DualStackBaseIpExt for Ipv6 {
    type DemuxSocketId<D: WeakDeviceIdentifier, BT: TcpBindingsTypes> = TcpSocketId<Ipv6, D, BT>;
    type ConnectionAndAddr<D: WeakDeviceIdentifier, BT: TcpBindingsTypes> = EitherStack<
        (Connection<Ipv6, Ipv6, D, BT>, ConnAddr<ConnIpAddr<Ipv6Addr, NonZeroU16, NonZeroU16>, D>),
        (Connection<Ipv6, Ipv4, D, BT>, ConnAddr<ConnIpAddr<Ipv4Addr, NonZeroU16, NonZeroU16>, D>),
    >;
    type DualStackIpOptions = Ipv6Options;
    type ListenerIpAddr = DualStackListenerIpAddr<Ipv6Addr, NonZeroU16>;
    type OriginalDstAddr = EitherStack<Ipv6Addr, Ipv4Addr>;

    fn as_dual_stack_ip_socket<D: WeakDeviceIdentifier, BT: TcpBindingsTypes>(
        id: &Self::DemuxSocketId<D, BT>,
    ) -> EitherStack<&TcpSocketId<Self, D, BT>, &TcpSocketId<Self::OtherVersion, D, BT>> {
        EitherStack::ThisStack(id)
    }
    fn into_dual_stack_ip_socket<D: WeakDeviceIdentifier, BT: TcpBindingsTypes>(
        id: Self::DemuxSocketId<D, BT>,
    ) -> EitherStack<TcpSocketId<Self, D, BT>, TcpSocketId<Self::OtherVersion, D, BT>> {
        EitherStack::ThisStack(id)
    }

    fn into_demux_socket_id<D: WeakDeviceIdentifier, BT: TcpBindingsTypes>(
        id: TcpSocketId<Self, D, BT>,
    ) -> Self::DemuxSocketId<D, BT> {
        id
    }
    fn get_conn_info<D: WeakDeviceIdentifier, BT: TcpBindingsTypes>(
        conn_and_addr: &Self::ConnectionAndAddr<D, BT>,
    ) -> ConnectionInfo<Self::Addr, D> {
        match conn_and_addr {
            EitherStack::ThisStack((_conn, addr)) => addr.clone().into(),
            EitherStack::OtherStack((
                _conn,
                ConnAddr {
                    ip:
                        ConnIpAddr { local: (local_ip, local_port), remote: (remote_ip, remote_port) },
                    device,
                },
            )) => ConnectionInfo {
                local_addr: SocketAddr {
                    ip: maybe_zoned(local_ip.addr().to_ipv6_mapped(), device),
                    port: *local_port,
                },
                remote_addr: SocketAddr {
                    ip: maybe_zoned(remote_ip.addr().to_ipv6_mapped(), device),
                    port: *remote_port,
                },
                device: device.clone(),
            },
        }
    }
    fn get_accept_queue_mut<D: WeakDeviceIdentifier, BT: TcpBindingsTypes>(
        conn_and_addr: &mut Self::ConnectionAndAddr<D, BT>,
    ) -> &mut Option<
        AcceptQueue<
            TcpSocketId<Self, D, BT>,
            BT::ReturnedBuffers,
            BT::ListenerNotifierOrProvidedBuffers,
        >,
    > {
        match conn_and_addr {
            EitherStack::ThisStack((conn, _addr)) => &mut conn.accept_queue,
            EitherStack::OtherStack((conn, _addr)) => &mut conn.accept_queue,
        }
    }
    fn get_defunct<D: WeakDeviceIdentifier, BT: TcpBindingsTypes>(
        conn_and_addr: &Self::ConnectionAndAddr<D, BT>,
    ) -> bool {
        match conn_and_addr {
            EitherStack::ThisStack((conn, _addr)) => conn.defunct,
            EitherStack::OtherStack((conn, _addr)) => conn.defunct,
        }
    }
    fn get_state<D: WeakDeviceIdentifier, BT: TcpBindingsTypes>(
        conn_and_addr: &Self::ConnectionAndAddr<D, BT>,
    ) -> &State<BT::Instant, BT::ReceiveBuffer, BT::SendBuffer, BT::ListenerNotifierOrProvidedBuffers>
    {
        match conn_and_addr {
            EitherStack::ThisStack((conn, _addr)) => &conn.state,
            EitherStack::OtherStack((conn, _addr)) => &conn.state,
        }
    }
    fn get_bound_info<D: WeakDeviceIdentifier>(
        ListenerAddr { ip, device }: &ListenerAddr<Self::ListenerIpAddr, D>,
    ) -> BoundInfo<Self::Addr, D> {
        match ip {
            DualStackListenerIpAddr::ThisStack(ip) => {
                ListenerAddr { ip: ip.clone(), device: device.clone() }.into()
            }
            DualStackListenerIpAddr::OtherStack(ListenerIpAddr {
                addr,
                identifier: local_port,
            }) => BoundInfo {
                addr: Some(maybe_zoned(
                    addr.map(|a| a.addr()).unwrap_or(Ipv4::UNSPECIFIED_ADDRESS).to_ipv6_mapped(),
                    &device,
                )),
                port: *local_port,
                device: device.clone(),
            },
            DualStackListenerIpAddr::BothStacks(local_port) => {
                BoundInfo { addr: None, port: *local_port, device: device.clone() }
            }
        }
    }

    fn destroy_socket_with_demux_id<
        CC: TcpContext<Self, BC> + TcpContext<Self::OtherVersion, BC>,
        BC: TcpBindingsContext<CC::DeviceId>,
    >(
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        demux_id: Self::DemuxSocketId<CC::WeakDeviceId, BC>,
    ) {
        destroy_socket(core_ctx, bindings_ctx, demux_id)
    }

    fn get_original_dst(addr: Self::OriginalDstAddr) -> Self::Addr {
        match addr {
            EitherStack::ThisStack(addr) => addr,
            EitherStack::OtherStack(addr) => *addr.to_ipv6_mapped(),
        }
    }
}

/// Timer ID for TCP connections.
#[derive(Derivative, GenericOverIp)]
#[generic_over_ip()]
#[derivative(
    Clone(bound = ""),
    Eq(bound = ""),
    PartialEq(bound = ""),
    Hash(bound = ""),
    Debug(bound = "")
)]
#[allow(missing_docs)]
pub enum TcpTimerId<D: WeakDeviceIdentifier, BT: TcpBindingsTypes> {
    V4(WeakTcpSocketId<Ipv4, D, BT>),
    V6(WeakTcpSocketId<Ipv6, D, BT>),
}

impl<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes>
    From<WeakTcpSocketId<I, D, BT>> for TcpTimerId<D, BT>
{
    fn from(f: WeakTcpSocketId<I, D, BT>) -> Self {
        I::map_ip(f, TcpTimerId::V4, TcpTimerId::V6)
    }
}

/// Bindings types for TCP.
///
/// The relationship between buffers  is as follows:
///
/// The Bindings will receive the `ReturnedBuffers` so that it can: 1. give the
/// application a handle to read/write data; 2. Observe whatever signal required
/// from the application so that it can inform Core. The peer end of returned
/// handle will be held by the state machine inside the netstack. Specialized
/// receive/send buffers will be derived from `ProvidedBuffers` from Bindings.
///
/// +-------------------------------+
/// |       +--------------+        |
/// |       |   returned   |        |
/// |       |    buffers   |        |
/// |       +------+-------+        |
/// |              |     application|
/// +--------------+----------------+
///                |
/// +--------------+----------------+
/// |              |        netstack|
/// |   +---+------+-------+---+    |
/// |   |   |  provided    |   |    |
/// |   | +-+-  buffers   -+-+ |    |
/// |   +-+-+--------------+-+-+    |
/// |     v                  v      |
/// |receive buffer     send buffer |
/// +-------------------------------+

pub trait TcpBindingsTypes:
    InstantBindingsTypes + TimerBindingsTypes + TxMetadataBindingsTypes + MatcherBindingsTypes + 'static
{
    /// Receive buffer used by TCP.
    type ReceiveBuffer: ReceiveBuffer + Send + Sync;
    /// Send buffer used by TCP.
    type SendBuffer: SendBuffer + Send + Sync;
    /// The object that will be returned by the state machine when a passive
    /// open connection becomes established. The bindings can use this object
    /// to read/write bytes from/into the created buffers.
    type ReturnedBuffers: Debug + Send + Sync;
    /// The extra information provided by the Bindings that implements platform
    /// dependent behaviors. It serves as a [`ListenerNotifier`] if the socket
    /// was used as a listener and it will be used to provide buffers if used
    /// to establish connections.
    type ListenerNotifierOrProvidedBuffers: Debug
        + IntoBuffers<Self::ReceiveBuffer, Self::SendBuffer>
        + ListenerNotifier
        + Send
        + Sync;

    /// Creates new buffers and returns the object that Bindings need to
    /// read/write from/into the created buffers.
    fn new_passive_open_buffers(
        buffer_sizes: BufferSizes,
    ) -> (Self::ReceiveBuffer, Self::SendBuffer, Self::ReturnedBuffers);
}

/// Allows passing a TCP socket to bindings, which can wait for it to
/// be destroyed and then receive diagnostics information.
pub trait TcpSocketDestructionContext: ReferenceNotifiers + InstantContext {
    /// Takes ownership of waiting for the last reference to the TCP
    /// socket to be dropped and then possibly generates diagnostics
    /// from the seed.
    fn defer_tcp_socket_destruction<I, S>(&self, result: RemoveResourceResultWithContext<S, Self>)
    where
        I: Ip,
        S: SocketDiagnosticsSeed<Output = TcpSocketDiagnostics<I, Self::Instant>> + Send;
}

/// The bindings context for TCP.
///
/// TCP timers are scoped by weak device IDs.
pub trait TcpBindingsContext<D>:
    Sized
    + DeferredResourceRemovalContext
    + TimerContext
    + RngContext
    + TcpBindingsTypes
    + SocketOpsFilterBindingContext<D>
    + SettingsContext<TcpSettings>
    + TcpSocketDestructionContext
{
}

impl<D, BC> TcpBindingsContext<D> for BC where
    BC: Sized
        + DeferredResourceRemovalContext
        + TimerContext
        + RngContext
        + TcpBindingsTypes
        + SocketOpsFilterBindingContext<D>
        + SettingsContext<TcpSettings>
        + TcpSocketDestructionContext
{
}

/// The core execution context abstracting demux state access for TCP.
pub trait TcpDemuxContext<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes>:
    TcpCoreTimerContext<I, D, BT>
{
    /// The inner IP transport context.
    type IpTransportCtx<'a>: TransportIpContext<I, BT, DeviceId = D::Strong, WeakDeviceId = D>
        + DeviceIpSocketHandler<I, BT>
        + TcpCoreTimerContext<I, D, BT>;

    /// Calls `f` with non-mutable access to the demux state.
    fn with_demux<O, F: FnOnce(&DemuxState<I, D, BT>) -> O>(&mut self, cb: F) -> O;

    /// Calls `f` with mutable access to the demux state.
    fn with_demux_mut<O, F: FnOnce(&mut DemuxState<I, D, BT>) -> O>(&mut self, cb: F) -> O;
}

/// Provides access to the current stack of the context.
///
/// This is useful when dealing with logic that applies to the current stack
/// but we want to be version agnostic: we have different associated types for
/// single-stack and dual-stack contexts, we can use this function to turn them
/// into the same type that only provides access to the current version of the
/// stack and trims down access to `I::OtherVersion`.
pub trait AsThisStack<T> {
    /// Get the this stack version of the context.
    fn as_this_stack(&mut self) -> &mut T;
}

impl<T> AsThisStack<T> for T {
    fn as_this_stack(&mut self) -> &mut T {
        self
    }
}

/// A marker traits for all traits used to access TCP socket.
pub trait TcpSocketContext<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes>:
    TcpCounterContext<I, D, BT> + CoreTxMetadataContext<TcpSocketTxMetadata<I, D, BT>, BT>
{
}

impl<CC, I, D, BC> TcpSocketContext<I, D, BC> for CC
where
    I: DualStackIpExt,
    D: WeakDeviceIdentifier,
    BC: TcpBindingsTypes,
    CC: TcpCounterContext<I, D, BC> + CoreTxMetadataContext<TcpSocketTxMetadata<I, D, BC>, BC>,
{
}

/// A shortcut for the `CoreTimerContext` required by TCP.
pub trait TcpCoreTimerContext<I: DualStackIpExt, D: WeakDeviceIdentifier, BC: TcpBindingsTypes>:
    CoreTimerContext<WeakTcpSocketId<I, D, BC>, BC>
{
}

impl<CC, I, D, BC> TcpCoreTimerContext<I, D, BC> for CC
where
    I: DualStackIpExt,
    D: WeakDeviceIdentifier,
    BC: TcpBindingsTypes,
    CC: CoreTimerContext<WeakTcpSocketId<I, D, BC>, BC>,
{
}

/// A marker trait for all dual stack conversions in [`TcpContext`].
pub trait DualStackConverter<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes>:
    OwnedOrRefsBidirectionalConverter<
        I::ConnectionAndAddr<D, BT>,
        EitherStack<
            (
                Connection<I, I, D, BT>,
                ConnAddr<ConnIpAddr<<I as Ip>::Addr, NonZeroU16, NonZeroU16>, D>,
            ),
            (
                Connection<I, I::OtherVersion, D, BT>,
                ConnAddr<ConnIpAddr<<I::OtherVersion as Ip>::Addr, NonZeroU16, NonZeroU16>, D>,
            ),
        >,
    > + OwnedOrRefsBidirectionalConverter<
        I::ListenerIpAddr,
        DualStackListenerIpAddr<I::Addr, NonZeroU16>,
    > + OwnedOrRefsBidirectionalConverter<
        ListenerAddr<I::ListenerIpAddr, D>,
        ListenerAddr<DualStackListenerIpAddr<I::Addr, NonZeroU16>, D>,
    > + OwnedOrRefsBidirectionalConverter<
        I::OriginalDstAddr,
        EitherStack<I::Addr, <I::OtherVersion as Ip>::Addr>,
    >
{
}

impl<I, D, BT, O> DualStackConverter<I, D, BT> for O
where
    I: DualStackIpExt,
    D: WeakDeviceIdentifier,
    BT: TcpBindingsTypes,
    O: OwnedOrRefsBidirectionalConverter<
            I::ConnectionAndAddr<D, BT>,
            EitherStack<
                (
                    Connection<I, I, D, BT>,
                    ConnAddr<ConnIpAddr<<I as Ip>::Addr, NonZeroU16, NonZeroU16>, D>,
                ),
                (
                    Connection<I, I::OtherVersion, D, BT>,
                    ConnAddr<ConnIpAddr<<I::OtherVersion as Ip>::Addr, NonZeroU16, NonZeroU16>, D>,
                ),
            >,
        > + OwnedOrRefsBidirectionalConverter<
            I::ListenerIpAddr,
            DualStackListenerIpAddr<I::Addr, NonZeroU16>,
        > + OwnedOrRefsBidirectionalConverter<
            ListenerAddr<I::ListenerIpAddr, D>,
            ListenerAddr<DualStackListenerIpAddr<I::Addr, NonZeroU16>, D>,
        > + OwnedOrRefsBidirectionalConverter<
            I::OriginalDstAddr,
            EitherStack<I::Addr, <I::OtherVersion as Ip>::Addr>,
        >,
{
}

/// A marker trait for all single stack conversions in [`TcpContext`].
pub trait SingleStackConverter<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes>:
    OwnedOrRefsBidirectionalConverter<
        I::ConnectionAndAddr<D, BT>,
        (Connection<I, I, D, BT>, ConnAddr<ConnIpAddr<<I as Ip>::Addr, NonZeroU16, NonZeroU16>, D>),
    > + OwnedOrRefsBidirectionalConverter<I::ListenerIpAddr, ListenerIpAddr<I::Addr, NonZeroU16>>
    + OwnedOrRefsBidirectionalConverter<
        ListenerAddr<I::ListenerIpAddr, D>,
        ListenerAddr<ListenerIpAddr<I::Addr, NonZeroU16>, D>,
    > + OwnedOrRefsBidirectionalConverter<I::OriginalDstAddr, I::Addr>
{
}

impl<I, D, BT, O> SingleStackConverter<I, D, BT> for O
where
    I: DualStackIpExt,
    D: WeakDeviceIdentifier,
    BT: TcpBindingsTypes,
    O: OwnedOrRefsBidirectionalConverter<
            I::ConnectionAndAddr<D, BT>,
            (
                Connection<I, I, D, BT>,
                ConnAddr<ConnIpAddr<<I as Ip>::Addr, NonZeroU16, NonZeroU16>, D>,
            ),
        > + OwnedOrRefsBidirectionalConverter<I::ListenerIpAddr, ListenerIpAddr<I::Addr, NonZeroU16>>
        + OwnedOrRefsBidirectionalConverter<
            ListenerAddr<I::ListenerIpAddr, D>,
            ListenerAddr<ListenerIpAddr<I::Addr, NonZeroU16>, D>,
        > + OwnedOrRefsBidirectionalConverter<I::OriginalDstAddr, I::Addr>,
{
}

/// Core context for TCP.
pub trait TcpContext<I: DualStackIpExt, BC: TcpBindingsTypes>:
    TcpDemuxContext<I, Self::WeakDeviceId, BC>
    + IpSocketHandler<I, BC>
    + TcpSocketContext<I, Self::WeakDeviceId, BC>
{
    /// The core context for the current version of the IP protocol. This is
    /// used to be version agnostic when the operation is on the current stack.
    type ThisStackIpTransportAndDemuxCtx<'a>: TransportIpContext<I, BC, DeviceId = Self::DeviceId, WeakDeviceId = Self::WeakDeviceId>
        + DeviceIpSocketHandler<I, BC>
        + TcpDemuxContext<I, Self::WeakDeviceId, BC>
        + TcpSocketContext<I, Self::WeakDeviceId, BC>;

    /// The core context that will give access to this version of the IP layer.
    type SingleStackIpTransportAndDemuxCtx<'a>: TransportIpContext<I, BC, DeviceId = Self::DeviceId, WeakDeviceId = Self::WeakDeviceId>
        + DeviceIpSocketHandler<I, BC>
        + TcpDemuxContext<I, Self::WeakDeviceId, BC>
        + AsThisStack<Self::ThisStackIpTransportAndDemuxCtx<'a>>
        + TcpSocketContext<I, Self::WeakDeviceId, BC>;

    /// A collection of type assertions that must be true in the single stack
    /// version, associated types and concrete types must unify and we can
    /// inspect types by converting them into the concrete types.
    type SingleStackConverter: SingleStackConverter<I, Self::WeakDeviceId, BC>;

    /// The core context that will give access to both versions of the IP layer.
    type DualStackIpTransportAndDemuxCtx<'a>: TransportIpContext<I, BC, DeviceId = Self::DeviceId, WeakDeviceId = Self::WeakDeviceId>
        + DeviceIpSocketHandler<I, BC>
        + TcpDemuxContext<I, Self::WeakDeviceId, BC>
        + TransportIpContext<
            I::OtherVersion,
            BC,
            DeviceId = Self::DeviceId,
            WeakDeviceId = Self::WeakDeviceId,
        > + DeviceIpSocketHandler<I::OtherVersion, BC>
        + TcpDemuxContext<I::OtherVersion, Self::WeakDeviceId, BC>
        + TcpDualStackContext<I, Self::WeakDeviceId, BC>
        + AsThisStack<Self::ThisStackIpTransportAndDemuxCtx<'a>>
        + TcpSocketContext<I, Self::WeakDeviceId, BC>
        + TcpCounterContext<I::OtherVersion, Self::WeakDeviceId, BC>;

    /// A collection of type assertions that must be true in the dual stack
    /// version, associated types and concrete types must unify and we can
    /// inspect types by converting them into the concrete types.
    type DualStackConverter: DualStackConverter<I, Self::WeakDeviceId, BC>;

    /// Calls the function with mutable access to the set with all TCP sockets.
    fn with_all_sockets_mut<O, F: FnOnce(&mut TcpSocketSet<I, Self::WeakDeviceId, BC>) -> O>(
        &mut self,
        cb: F,
    ) -> O;

    /// Calls the callback once for each currently installed socket.
    fn for_each_socket<
        F: FnMut(&TcpSocketId<I, Self::WeakDeviceId, BC>, &TcpSocketState<I, Self::WeakDeviceId, BC>),
    >(
        &mut self,
        cb: F,
    );

    /// Calls the function with access to the socket state,
    /// ISN & Timestamp Offset generators, and Transport + Demux context.
    fn with_socket_mut_generators_transport_demux<
        O,
        F: for<'a> FnOnce(
            MaybeDualStack<
                (&'a mut Self::DualStackIpTransportAndDemuxCtx<'a>, Self::DualStackConverter),
                (&'a mut Self::SingleStackIpTransportAndDemuxCtx<'a>, Self::SingleStackConverter),
            >,
            &mut TcpSocketState<I, Self::WeakDeviceId, BC>,
            &IsnGenerator<BC::Instant>,
            &TimestampOffsetGenerator<BC::Instant>,
        ) -> O,
    >(
        &mut self,
        id: &TcpSocketId<I, Self::WeakDeviceId, BC>,
        cb: F,
    ) -> O;

    /// Calls the function with immutable access to the socket state.
    fn with_socket<O, F: FnOnce(&TcpSocketState<I, Self::WeakDeviceId, BC>) -> O>(
        &mut self,
        id: &TcpSocketId<I, Self::WeakDeviceId, BC>,
        cb: F,
    ) -> O {
        self.with_socket_and_converter(id, |socket_state, _converter| cb(socket_state))
    }

    /// Calls the function with the immutable reference to the socket state and
    /// a converter to inspect.
    fn with_socket_and_converter<
        O,
        F: FnOnce(
            &TcpSocketState<I, Self::WeakDeviceId, BC>,
            MaybeDualStack<Self::DualStackConverter, Self::SingleStackConverter>,
        ) -> O,
    >(
        &mut self,
        id: &TcpSocketId<I, Self::WeakDeviceId, BC>,
        cb: F,
    ) -> O;

    /// Calls the function with access to the socket state and Transport + Demux
    /// context.
    fn with_socket_mut_transport_demux<
        O,
        F: for<'a> FnOnce(
            MaybeDualStack<
                (&'a mut Self::DualStackIpTransportAndDemuxCtx<'a>, Self::DualStackConverter),
                (&'a mut Self::SingleStackIpTransportAndDemuxCtx<'a>, Self::SingleStackConverter),
            >,
            &mut TcpSocketState<I, Self::WeakDeviceId, BC>,
        ) -> O,
    >(
        &mut self,
        id: &TcpSocketId<I, Self::WeakDeviceId, BC>,
        cb: F,
    ) -> O {
        self.with_socket_mut_generators_transport_demux(
            id,
            |ctx, socket_state, _isn, _timestamp_offset| cb(ctx, socket_state),
        )
    }

    /// Calls the function with mutable access to the socket state.
    fn with_socket_mut<O, F: FnOnce(&mut TcpSocketState<I, Self::WeakDeviceId, BC>) -> O>(
        &mut self,
        id: &TcpSocketId<I, Self::WeakDeviceId, BC>,
        cb: F,
    ) -> O {
        self.with_socket_mut_generators_transport_demux(
            id,
            |_ctx, socket_state, _isn, _timestamp_offset| cb(socket_state),
        )
    }

    /// Calls the function with the mutable reference to the socket state and a
    /// converter to inspect.
    fn with_socket_mut_and_converter<
        O,
        F: FnOnce(
            &mut TcpSocketState<I, Self::WeakDeviceId, BC>,
            MaybeDualStack<Self::DualStackConverter, Self::SingleStackConverter>,
        ) -> O,
    >(
        &mut self,
        id: &TcpSocketId<I, Self::WeakDeviceId, BC>,
        cb: F,
    ) -> O {
        self.with_socket_mut_generators_transport_demux(
            id,
            |ctx, socket_state, _isn, _timestamp_offset| {
                let converter = match ctx {
                    MaybeDualStack::NotDualStack((_core_ctx, converter)) => {
                        MaybeDualStack::NotDualStack(converter)
                    }
                    MaybeDualStack::DualStack((_core_ctx, converter)) => {
                        MaybeDualStack::DualStack(converter)
                    }
                };
                cb(socket_state, converter)
            },
        )
    }
}

/// A ZST that helps convert IPv6 socket IDs into IPv4 demux IDs.
#[derive(Clone, Copy)]
pub struct Ipv6SocketIdToIpv4DemuxIdConverter;

/// This trait allows us to work around the life-time issue when we need to
/// convert an IPv6 socket ID into an IPv4 demux ID without holding on the
/// a dual-stack CoreContext.
pub trait DualStackDemuxIdConverter<I: DualStackIpExt>: 'static + Clone + Copy {
    /// Turns a [`TcpSocketId`] into the demuxer ID of the other stack.
    fn convert<D: WeakDeviceIdentifier, BT: TcpBindingsTypes>(
        &self,
        id: TcpSocketId<I, D, BT>,
    ) -> <I::OtherVersion as DualStackBaseIpExt>::DemuxSocketId<D, BT>;
}

impl DualStackDemuxIdConverter<Ipv6> for Ipv6SocketIdToIpv4DemuxIdConverter {
    fn convert<D: WeakDeviceIdentifier, BT: TcpBindingsTypes>(
        &self,
        id: TcpSocketId<Ipv6, D, BT>,
    ) -> <Ipv4 as DualStackBaseIpExt>::DemuxSocketId<D, BT> {
        EitherStack::OtherStack(id)
    }
}

/// A provider of dualstack socket functionality required by TCP sockets.
pub trait TcpDualStackContext<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes> {
    /// The inner IP transport context,
    type DualStackIpTransportCtx<'a>: TransportIpContext<I, BT, DeviceId = D::Strong, WeakDeviceId = D>
        + DeviceIpSocketHandler<I, BT>
        + TcpCoreTimerContext<I, D, BT>
        + TransportIpContext<I::OtherVersion, BT, DeviceId = D::Strong, WeakDeviceId = D>
        + DeviceIpSocketHandler<I::OtherVersion, BT>
        + TcpCoreTimerContext<I::OtherVersion, D, BT>;

    /// Gets a converter to get the demux socket ID for the other stack.
    fn other_demux_id_converter(&self) -> impl DualStackDemuxIdConverter<I>;

    /// Turns a [`TcpSocketId`] into the demuxer ID of the other stack.
    fn into_other_demux_socket_id(
        &self,
        id: TcpSocketId<I, D, BT>,
    ) -> <I::OtherVersion as DualStackBaseIpExt>::DemuxSocketId<D, BT> {
        self.other_demux_id_converter().convert(id)
    }

    /// Returns a dual stack tuple with both demux identifiers for `id`.
    fn dual_stack_demux_id(
        &self,
        id: TcpSocketId<I, D, BT>,
    ) -> DualStackTuple<I, DemuxSocketId<I, D, BT>> {
        let this_id = DemuxSocketId::<I, _, _>(I::into_demux_socket_id(id.clone()));
        let other_id = DemuxSocketId::<I::OtherVersion, _, _>(self.into_other_demux_socket_id(id));
        DualStackTuple::new(this_id, other_id)
    }

    /// Gets the enabled state of dual stack operations on the given socket.
    fn dual_stack_enabled(&self, ip_options: &I::DualStackIpOptions) -> bool;
    /// Sets the enabled state of dual stack operations on the given socket.
    fn set_dual_stack_enabled(&self, ip_options: &mut I::DualStackIpOptions, value: bool);

    /// Calls `cb` with mutable access to both demux states.
    fn with_both_demux_mut<
        O,
        F: FnOnce(&mut DemuxState<I, D, BT>, &mut DemuxState<I::OtherVersion, D, BT>) -> O,
    >(
        &mut self,
        cb: F,
    ) -> O;
}

/// Socket address includes the ip address and the port number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, GenericOverIp)]
#[generic_over_ip(A, IpAddress)]
pub struct SocketAddr<A: IpAddress, D> {
    /// The IP component of the address.
    pub ip: ZonedAddr<SpecifiedAddr<A>, D>,
    /// The port component of the address.
    pub port: NonZeroU16,
}

impl<A: IpAddress, D> From<SocketAddr<A, D>>
    for IpAddr<SocketAddr<Ipv4Addr, D>, SocketAddr<Ipv6Addr, D>>
{
    fn from(addr: SocketAddr<A, D>) -> IpAddr<SocketAddr<Ipv4Addr, D>, SocketAddr<Ipv6Addr, D>> {
        <A::Version as Ip>::map_ip_in(addr, |i| IpAddr::V4(i), |i| IpAddr::V6(i))
    }
}

impl<A: IpAddress, D> SocketAddr<A, D> {
    /// Maps the [`SocketAddr`]'s zone type.
    pub fn map_zone<Y>(self, f: impl FnOnce(D) -> Y) -> SocketAddr<A, Y> {
        let Self { ip, port } = self;
        SocketAddr { ip: ip.map_zone(f), port }
    }
}

impl<A: IpAddress, D: fmt::Display> fmt::Display for SocketAddr<A, D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        let Self { ip, port } = self;
        let formatter = AddrAndPortFormatter::<_, _, A::Version>::new(
            ip.as_ref().map_addr(core::convert::AsRef::<A>::as_ref),
            port,
        );
        formatter.fmt(f)
    }
}

/// Uninstantiable type used to implement [`SocketMapAddrSpec`] for TCP
pub(crate) enum TcpPortSpec {}

impl SocketMapAddrSpec for TcpPortSpec {
    type RemoteIdentifier = NonZeroU16;
    type LocalIdentifier = NonZeroU16;
}

/// An implementation of [`IpTransportContext`] for TCP.
pub enum TcpIpTransportContext {}

/// This trait is only used as a marker for the identifier that
/// [`TcpSocketSpec`] keeps in the socket map. This is effectively only
/// implemented for [`TcpSocketId`] but defining a trait effectively reduces the
/// number of type parameters percolating down to the socket map types since
/// they only really care about the identifier's behavior.
pub trait SpecSocketId: Clone + Eq + PartialEq + Debug + 'static {}
impl<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes> SpecSocketId
    for TcpSocketId<I, D, BT>
{
}

impl<A: SpecSocketId, B: SpecSocketId> SpecSocketId for EitherStack<A, B> {}

/// Uninstantiatable type for implementing [`SocketMapStateSpec`].
struct TcpSocketSpec<I, D, BT>(PhantomData<(I, D, BT)>, Never);

impl<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes> SocketMapStateSpec
    for TcpSocketSpec<I, D, BT>
{
    type ListenerId = I::DemuxSocketId<D, BT>;
    type ConnId = I::DemuxSocketId<D, BT>;

    type ListenerSharingState = ListenerSharingState;
    type ConnSharingState = SharingState;
    type AddrVecTag = AddrVecTag;

    type ListenerAddrState = ListenerAddrState<Self::ListenerId>;
    type ConnAddrState = ConnAddrState<Self::ConnId>;

    fn listener_tag(
        ListenerAddrInfo { has_device, specified_addr }: ListenerAddrInfo,
        state: &Self::ListenerAddrState,
    ) -> Self::AddrVecTag {
        let (sharing, state) = match state {
            ListenerAddrState::ExclusiveBound(_) => {
                (SharingState::Exclusive, SocketTagState::Bound)
            }
            ListenerAddrState::ExclusiveListener(_) => {
                (SharingState::Exclusive, SocketTagState::Listener)
            }
            ListenerAddrState::Shared { listener, bound: _ } => (
                SharingState::ReuseAddress,
                match listener {
                    Some(_) => SocketTagState::Listener,
                    None => SocketTagState::Bound,
                },
            ),
        };
        AddrVecTag { sharing, state, has_device, specified_addr }
    }

    fn connected_tag(has_device: bool, state: &Self::ConnAddrState) -> Self::AddrVecTag {
        let ConnAddrState { sharing, id: _ } = state;
        AddrVecTag {
            sharing: *sharing,
            has_device,
            state: SocketTagState::Conn,
            specified_addr: true,
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
struct AddrVecTag {
    sharing: SharingState,
    state: SocketTagState,
    has_device: bool,
    specified_addr: bool,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum SocketTagState {
    Conn,
    Listener,
    Bound,
}

#[derive(Debug)]
enum ListenerAddrState<S> {
    ExclusiveBound(S),
    ExclusiveListener(S),
    Shared { listener: Option<S>, bound: SmallVec<[S; 1]> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListenerSharingState {
    pub(crate) sharing: SharingState,
    pub(crate) listening: bool,
}

enum ListenerAddrInserter<'a, S> {
    Listener(&'a mut Option<S>),
    Bound(&'a mut SmallVec<[S; 1]>),
}

impl<'a, S> Inserter<S> for ListenerAddrInserter<'a, S> {
    fn insert(self, id: S) {
        match self {
            Self::Listener(o) => *o = Some(id),
            Self::Bound(b) => b.push(id),
        }
    }
}

impl<S: SpecSocketId> SocketMapAddrStateSpec for ListenerAddrState<S> {
    type SharingState = ListenerSharingState;
    type Id = S;
    type Inserter<'a> = ListenerAddrInserter<'a, S>;

    fn new(new_sharing_state: &Self::SharingState, id: Self::Id) -> Self {
        let ListenerSharingState { sharing, listening } = new_sharing_state;
        match sharing {
            SharingState::Exclusive => match listening {
                true => Self::ExclusiveListener(id),
                false => Self::ExclusiveBound(id),
            },
            SharingState::ReuseAddress => {
                let (listener, bound) =
                    if *listening { (Some(id), Default::default()) } else { (None, smallvec![id]) };
                Self::Shared { listener, bound }
            }
        }
    }

    fn contains_id(&self, id: &Self::Id) -> bool {
        match self {
            Self::ExclusiveBound(x) | Self::ExclusiveListener(x) => id == x,
            Self::Shared { listener, bound } => {
                listener.as_ref().is_some_and(|x| id == x) || bound.contains(id)
            }
        }
    }

    fn could_insert(
        &self,
        new_sharing_state: &Self::SharingState,
    ) -> Result<(), IncompatibleError> {
        match self {
            Self::ExclusiveBound(_) | Self::ExclusiveListener(_) => Err(IncompatibleError),
            Self::Shared { listener, bound: _ } => {
                let ListenerSharingState { listening: _, sharing } = new_sharing_state;
                match sharing {
                    SharingState::Exclusive => Err(IncompatibleError),
                    SharingState::ReuseAddress => match listener {
                        Some(_) => Err(IncompatibleError),
                        None => Ok(()),
                    },
                }
            }
        }
    }

    fn remove_by_id(&mut self, id: Self::Id) -> RemoveResult {
        match self {
            Self::ExclusiveBound(b) => {
                assert_eq!(*b, id);
                RemoveResult::IsLast
            }
            Self::ExclusiveListener(l) => {
                assert_eq!(*l, id);
                RemoveResult::IsLast
            }
            Self::Shared { listener, bound } => {
                match listener {
                    Some(l) if *l == id => {
                        *listener = None;
                    }
                    Some(_) | None => {
                        let index = bound.iter().position(|b| *b == id).expect("invalid socket ID");
                        let _: S = bound.swap_remove(index);
                    }
                };
                match (listener, bound.is_empty()) {
                    (Some(_), _) => RemoveResult::Success,
                    (None, false) => RemoveResult::Success,
                    (None, true) => RemoveResult::IsLast,
                }
            }
        }
    }

    fn try_get_inserter<'a, 'b>(
        &'b mut self,
        new_sharing_state: &'a Self::SharingState,
    ) -> Result<Self::Inserter<'b>, IncompatibleError> {
        match self {
            Self::ExclusiveBound(_) | Self::ExclusiveListener(_) => Err(IncompatibleError),
            Self::Shared { listener, bound } => {
                let ListenerSharingState { listening, sharing } = new_sharing_state;
                match sharing {
                    SharingState::Exclusive => Err(IncompatibleError),
                    SharingState::ReuseAddress => {
                        match listener {
                            Some(_) => {
                                // Always fail to insert if there is already a
                                // listening socket.
                                Err(IncompatibleError)
                            }
                            None => Ok(match listening {
                                true => ListenerAddrInserter::Listener(listener),
                                false => ListenerAddrInserter::Bound(bound),
                            }),
                        }
                    }
                }
            }
        }
    }

    fn sharing_state(&self) -> Self::SharingState {
        let (sharing, listening) = match self {
            Self::ExclusiveBound(_) => (SharingState::Exclusive, false),
            Self::ExclusiveListener(_) => (SharingState::Exclusive, true),
            Self::Shared { listener, bound: _ } => (SharingState::ReuseAddress, listener.is_some()),
        };
        ListenerSharingState { sharing, listening }
    }
}

/// Verifies that there are no conflicts with a socket at the specified address.
/// Calls the `filter` with a tag for all potential conflicts. It should return
/// true if the tag represents a socket that the new socket would conflict with.
fn check_conflicts<I, D, BT>(
    socketmap: &SocketMap<AddrVec<I, D, TcpPortSpec>, Bound<TcpSocketSpec<I, D, BT>>>,
    addr: &ListenerAddr<ListenerIpAddr<I::Addr, NonZeroU16>, D>,
    filter: impl Fn(&AddrVecTag) -> bool,
) -> Result<(), InsertError>
where
    I: DualStackIpExt,
    D: WeakDeviceIdentifier,
    BT: TcpBindingsTypes,
{
    // Check all potential shadows.
    let addr_vec = AddrVec::Listen(addr.clone());
    for shadow_addr in addr_vec.iter_shadows() {
        if let Some(bound) = socketmap.get(&shadow_addr) {
            let tag = bound.tag(&shadow_addr);
            if filter(&tag) {
                return Err(InsertError::ShadowAddrExists);
            }
        }
    }

    // Check direct descendants.
    if socketmap.descendant_counts(&addr.clone().into()).any(|(tag, _)| filter(tag)) {
        return Err(InsertError::WouldShadowExisting);
    }

    // Check indirect conflicts.
    // If device is specified, then look for socket with unspecified device.
    if addr.device.is_some()
        && socketmap
            .descendant_counts(&addr.without_device().into())
            .any(|(tag, _)| !tag.has_device && filter(tag))
    {
        return Err(InsertError::IndirectConflict);
    }

    // If address is specified, then look for socket with unspecified address.
    if addr.ip.addr.is_some()
        && socketmap
            .descendant_counts(&addr.without_addr().into())
            .any(|(tag, _)| !tag.specified_addr && filter(tag))
    {
        return Err(InsertError::IndirectConflict);
    }

    Ok(())
}

impl<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes>
    SocketMapUpdateSharingPolicy<
        ListenerAddr<ListenerIpAddr<I::Addr, NonZeroU16>, D>,
        ListenerSharingState,
        I,
        D,
        TcpPortSpec,
    > for TcpSocketSpec<I, D, BT>
{
    fn allows_sharing_update(
        socketmap: &SocketMap<AddrVec<I, D, TcpPortSpec>, Bound<Self>>,
        addr: &ListenerAddr<ListenerIpAddr<I::Addr, NonZeroU16>, D>,
        old_state: &ListenerSharingState,
        new_state: &ListenerSharingState,
    ) -> Result<(), UpdateSharingError> {
        match (old_state.listening, new_state.listening) {
            (true, false) => (), // Changing a listener to bound is always okay.
            (true, true) | (false, false) => (), // No change
            (false, true) => {
                check_conflicts(socketmap, addr, |tag| tag.state == SocketTagState::Listener)
                    .map_err(|_conflict| UpdateSharingError)?;
            }
        }

        match (old_state.sharing, new_state.sharing) {
            (SharingState::Exclusive, SharingState::Exclusive)
            | (SharingState::ReuseAddress, SharingState::ReuseAddress)
            | (SharingState::Exclusive, SharingState::ReuseAddress) => (),
            (SharingState::ReuseAddress, SharingState::Exclusive) => {
                // Linux allows this, but it introduces inconsistent socket
                // state: if some sockets were allowed to bind because they all
                // had SO_REUSEADDR set, then allowing clearing SO_REUSEADDR on
                // one of them makes the state inconsistent. We only allow this
                // if it doesn't introduce inconsistencies.
                check_conflicts(socketmap, addr, |tag| tag.state != SocketTagState::Conn)
                    .map_err(|_conflict| UpdateSharingError)?;
            }
        }

        Ok(())
    }
}

impl<S: SpecSocketId> SocketMapAddrStateUpdateSharingSpec for ListenerAddrState<S> {
    fn try_update_sharing(
        &mut self,
        id: Self::Id,
        ListenerSharingState{listening: new_listening, sharing: new_sharing}: &Self::SharingState,
    ) -> Result<(), IncompatibleError> {
        match self {
            Self::ExclusiveBound(i) | Self::ExclusiveListener(i) => {
                assert_eq!(i, &id);
                *self = match new_sharing {
                    SharingState::Exclusive => match new_listening {
                        true => Self::ExclusiveListener(id),
                        false => Self::ExclusiveBound(id),
                    },
                    SharingState::ReuseAddress => {
                        let (listener, bound) = match new_listening {
                            true => (Some(id), Default::default()),
                            false => (None, smallvec![id]),
                        };
                        Self::Shared { listener, bound }
                    }
                };
                Ok(())
            }
            Self::Shared { listener, bound } => {
                if listener.as_ref() == Some(&id) {
                    match new_sharing {
                        SharingState::Exclusive => {
                            if bound.is_empty() {
                                *self = match new_listening {
                                    true => Self::ExclusiveListener(id),
                                    false => Self::ExclusiveBound(id),
                                };
                                Ok(())
                            } else {
                                Err(IncompatibleError)
                            }
                        }
                        SharingState::ReuseAddress => match new_listening {
                            true => Ok(()), // no-op
                            false => {
                                bound.push(id);
                                *listener = None;
                                Ok(())
                            }
                        },
                    }
                } else {
                    let index = bound
                        .iter()
                        .position(|b| b == &id)
                        .expect("ID is neither listener nor bound");
                    if *new_listening && listener.is_some() {
                        return Err(IncompatibleError);
                    }
                    match new_sharing {
                        SharingState::Exclusive => {
                            if bound.len() > 1 {
                                return Err(IncompatibleError);
                            } else {
                                *self = match new_listening {
                                    true => Self::ExclusiveListener(id),
                                    false => Self::ExclusiveBound(id),
                                };
                                Ok(())
                            }
                        }
                        SharingState::ReuseAddress => {
                            match new_listening {
                                false => Ok(()), // no-op
                                true => {
                                    let _: S = bound.swap_remove(index);
                                    *listener = Some(id);
                                    Ok(())
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SharingState {
    Exclusive,
    ReuseAddress,
}

impl Default for SharingState {
    fn default() -> Self {
        Self::Exclusive
    }
}

impl<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes>
    SocketMapConflictPolicy<
        ListenerAddr<ListenerIpAddr<I::Addr, NonZeroU16>, D>,
        ListenerSharingState,
        I,
        D,
        TcpPortSpec,
    > for TcpSocketSpec<I, D, BT>
{
    fn check_insert_conflicts(
        state: &ListenerSharingState,
        addr: &ListenerAddr<ListenerIpAddr<I::Addr, NonZeroU16>, D>,
        socketmap: &SocketMap<AddrVec<I, D, TcpPortSpec>, Bound<Self>>,
    ) -> Result<(), InsertError> {
        fn can_share(s1: SharingState, s2: SharingState) -> bool {
            (s1, s2) == (SharingState::ReuseAddress, SharingState::ReuseAddress)
        }

        check_conflicts(socketmap, addr, |tag| {
            tag.state == SocketTagState::Listener || !can_share(tag.sharing, state.sharing)
        })
    }
}

impl<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes>
    SocketMapConflictPolicy<
        ConnAddr<ConnIpAddr<I::Addr, NonZeroU16, NonZeroU16>, D>,
        SharingState,
        I,
        D,
        TcpPortSpec,
    > for TcpSocketSpec<I, D, BT>
{
    fn check_insert_conflicts(
        _sharing: &SharingState,
        addr: &ConnAddr<ConnIpAddr<I::Addr, NonZeroU16, NonZeroU16>, D>,
        socketmap: &SocketMap<AddrVec<I, D, TcpPortSpec>, Bound<Self>>,
    ) -> Result<(), InsertError> {
        // We need to make sure there are no present sockets that have the same
        // 5-tuple with the to-be-added socket.
        let addr_vec = AddrVec::Conn(addr.clone());
        if socketmap.get(&addr_vec).is_some() {
            return Err(InsertError::Exists);
        }

        match &addr.device {
            // If the new connection does not have a device bound, check if
            // there is already another connection with the same 4-tuple but
            // a device bound. Otherwise the other connection will get all
            // traffic from us.
            None => {
                if socketmap.descendant_counts(&addr_vec).len() > 0 {
                    return Err(InsertError::WouldShadowExisting);
                }
            }
            // If the new connection has a device bound, check if there is
            // a different connection with the same 4-tuple but without a
            // device bound. Otherwise we will get all traffic from them.
            Some(_device) => {
                if socketmap.get(&AddrVec::Conn(ConnAddr { device: None, ip: addr.ip })).is_some() {
                    return Err(InsertError::ShadowAddrExists);
                }
            }
        }
        // Otherwise, connections don't conflict with existing listeners.
        Ok(())
    }
}

#[derive(Debug)]
struct ConnAddrState<S> {
    sharing: SharingState,
    id: S,
}

impl<S: SpecSocketId> ConnAddrState<S> {
    #[cfg_attr(feature = "instrumented", track_caller)]
    pub(crate) fn id(&self) -> S {
        self.id.clone()
    }
}

impl<S: SpecSocketId> SocketMapAddrStateSpec for ConnAddrState<S> {
    type Id = S;
    type Inserter<'a> = Never;
    type SharingState = SharingState;

    fn new(new_sharing_state: &Self::SharingState, id: Self::Id) -> Self {
        Self { sharing: *new_sharing_state, id }
    }

    fn contains_id(&self, id: &Self::Id) -> bool {
        &self.id == id
    }

    fn could_insert(
        &self,
        _new_sharing_state: &Self::SharingState,
    ) -> Result<(), IncompatibleError> {
        Err(IncompatibleError)
    }

    fn remove_by_id(&mut self, id: Self::Id) -> RemoveResult {
        let Self { sharing: _, id: existing_id } = self;
        assert_eq!(*existing_id, id);
        return RemoveResult::IsLast;
    }

    fn try_get_inserter<'a, 'b>(
        &'b mut self,
        _new_sharing_state: &'a Self::SharingState,
    ) -> Result<Self::Inserter<'b>, IncompatibleError> {
        Err(IncompatibleError)
    }

    fn sharing_state(&self) -> Self::SharingState {
        self.sharing
    }
}

#[derive(Debug, Clone)]
#[cfg_attr(test, derive(PartialEq))]
pub struct Unbound<D, Extra> {
    bound_device: Option<D>,
    buffer_sizes: BufferSizes,
    socket_extra: Takeable<Extra>,
}

type PrimaryRc<I, D, BT> = netstack3_base::sync::PrimaryRc<ReferenceState<I, D, BT>>;
type StrongRc<I, D, BT> = netstack3_base::sync::StrongRc<ReferenceState<I, D, BT>>;
type WeakRc<I, D, BT> = netstack3_base::sync::WeakRc<ReferenceState<I, D, BT>>;

#[derive(Derivative)]
#[derivative(Debug(bound = "D: Debug"))]
pub enum TcpSocketSetEntry<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes> {
    /// The socket set is holding a primary reference.
    Primary(PrimaryRc<I, D, BT>),
    /// The socket set is holding a "dead on arrival" (DOA) entry for a strong
    /// reference.
    ///
    /// This mechanism guards against a subtle race between a connected socket
    /// created from a listener being added to the socket set and the same
    /// socket attempting to close itself before the listener has had a chance
    /// to add it to the set.
    ///
    /// See [`destroy_socket`] for the details handling this.
    DeadOnArrival,
}

/// A thin wrapper around a hash map that keeps a set of all the known TCP
/// sockets in the system.
#[derive(Debug, Derivative)]
#[derivative(Default(bound = ""))]
pub struct TcpSocketSet<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes>(
    HashMap<TcpSocketId<I, D, BT>, TcpSocketSetEntry<I, D, BT>>,
);

impl<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes> Deref
    for TcpSocketSet<I, D, BT>
{
    type Target = HashMap<TcpSocketId<I, D, BT>, TcpSocketSetEntry<I, D, BT>>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes> DerefMut
    for TcpSocketSet<I, D, BT>
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// A custom drop impl for the entire set to make tests easier to handle.
///
/// Because [`TcpSocketId`] is not really RAII in respect to closing the socket,
/// tests might finish without closing them and it's easier to deal with that in
/// a single place.
impl<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes> Drop
    for TcpSocketSet<I, D, BT>
{
    fn drop(&mut self) {
        // Listening sockets may hold references to other sockets so we walk
        // through all of the sockets looking for unclosed listeners and close
        // their accept queue so that dropping everything doesn't spring the
        // primary reference checks.
        //
        // Note that we don't pay attention to lock ordering here. Assuming that
        // when the set is dropped everything is going down and no locks are
        // held.
        let Self(map) = self;
        for TcpSocketId(rc) in map.keys() {
            let guard = rc.locked_state.read();
            let accept_queue = match &(*guard).socket_state {
                TcpSocketStateInner::Listener(Listener { accept_queue, .. }) => accept_queue,
                _ => continue,
            };
            if !accept_queue.is_closed() {
                let (_pending_sockets_iterator, _): (_, BT::ListenerNotifierOrProvidedBuffers) =
                    accept_queue.close();
            }
        }
    }
}

type BoundSocketMap<I, D, BT> = socket::BoundSocketMap<I, D, TcpPortSpec, TcpSocketSpec<I, D, BT>>;

/// TCP demux state.
#[derive(GenericOverIp)]
#[generic_over_ip(I, Ip)]
pub struct DemuxState<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes> {
    socketmap: BoundSocketMap<I, D, BT>,
}

/// Holds all the TCP socket states.
pub struct Sockets<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes> {
    demux: RwLock<DemuxState<I, D, BT>>,
    // Destroy all_sockets last so the strong references in the demux are
    // dropped before the primary references in the set.
    all_sockets: RwLock<TcpSocketSet<I, D, BT>>,
}

impl<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes>
    OrderedLockAccess<DemuxState<I, D, BT>> for Sockets<I, D, BT>
{
    type Lock = RwLock<DemuxState<I, D, BT>>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.demux)
    }
}

impl<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes>
    OrderedLockAccess<TcpSocketSet<I, D, BT>> for Sockets<I, D, BT>
{
    type Lock = RwLock<TcpSocketSet<I, D, BT>>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.all_sockets)
    }
}

/// The state held by a [`TcpSocketId`].
#[derive(Derivative)]
#[derivative(Debug(bound = "D: Debug"))]
pub struct ReferenceState<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes> {
    locked_state: RwLock<TcpSocketState<I, D, BT>>,
    counters: TcpCountersWithSocket<I>,
}

/// The locked state held by a TCP socket.
#[derive(Derivative)]
#[derivative(Debug(bound = "D: Debug"))]
pub struct TcpSocketState<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes> {
    socket_state: TcpSocketStateInner<I, D, BT>,
    sharing: SharingState,
    // Options specific to the IP version.
    ip_options: I::DualStackIpOptions,
    // All other options.
    socket_options: SocketOptions,
}

#[derive(Derivative)]
#[derivative(Debug(bound = "D: Debug"))]
pub enum TcpSocketStateInner<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes> {
    Unbound(Unbound<D, BT::ListenerNotifierOrProvidedBuffers>),
    Bound(BoundState<I, D, BT>),
    Listener(Listener<I, D, BT>),
    Connected { conn: I::ConnectionAndAddr<D, BT>, timer: BT::Timer },
}

struct TcpPortAlloc<'a, I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes>(
    &'a BoundSocketMap<I, D, BT>,
);

impl<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes> PortAllocImpl
    for TcpPortAlloc<'_, I, D, BT>
{
    const EPHEMERAL_RANGE: RangeInclusive<u16> = 49152..=65535;
    type Id = Option<SocketIpAddr<I::Addr>>;
    /// The TCP port allocator takes an extra optional argument with a port to
    /// avoid.
    ///
    /// This is used to sidestep possible self-connections when allocating a
    /// local port on a connect call with an unset local port.
    type PortAvailableArg = Option<NonZeroU16>;

    fn is_port_available(&self, addr: &Self::Id, port: u16, arg: &Option<NonZeroU16>) -> bool {
        let Self(socketmap) = self;
        // We can safely unwrap here, because the ports received in
        // `is_port_available` are guaranteed to be in `EPHEMERAL_RANGE`.
        let port = NonZeroU16::new(port).unwrap();

        // Reject ports matching the argument.
        if arg.is_some_and(|a| a == port) {
            return false;
        }

        let root_addr = AddrVec::from(ListenerAddr {
            ip: ListenerIpAddr { addr: *addr, identifier: port },
            device: None,
        });

        // A port is free if there are no sockets currently using it, and if
        // there are no sockets that are shadowing it.

        root_addr.iter_shadows().chain(core::iter::once(root_addr.clone())).all(|a| match &a {
            AddrVec::Listen(l) => socketmap.listeners().get_by_addr(&l).is_none(),
            AddrVec::Conn(_c) => {
                unreachable!("no connection shall be included in an iteration from a listener")
            }
        }) && socketmap.get_shadower_counts(&root_addr) == 0
    }
}

struct TcpDualStackPortAlloc<'a, I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes>(
    &'a BoundSocketMap<I, D, BT>,
    &'a BoundSocketMap<I::OtherVersion, D, BT>,
);

/// When binding to IPv6 ANY address (::), we need to allocate a port that is
/// available in both stacks.
impl<'a, I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes> PortAllocImpl
    for TcpDualStackPortAlloc<'a, I, D, BT>
{
    const EPHEMERAL_RANGE: RangeInclusive<u16> =
        <TcpPortAlloc<'a, I, D, BT> as PortAllocImpl>::EPHEMERAL_RANGE;
    type Id = ();
    type PortAvailableArg = ();

    fn is_port_available(&self, (): &Self::Id, port: u16, (): &Self::PortAvailableArg) -> bool {
        let Self(this, other) = self;
        TcpPortAlloc(this).is_port_available(&None, port, &None)
            && TcpPortAlloc(other).is_port_available(&None, port, &None)
    }
}

impl<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes> Sockets<I, D, BT> {
    pub(crate) fn new() -> Self {
        Self {
            demux: RwLock::new(DemuxState { socketmap: Default::default() }),
            all_sockets: Default::default(),
        }
    }
}

/// The Connection state.
///
/// Note: the `state` is not guaranteed to be [`State::Established`]. The
/// connection can be in any state as long as both the local and remote socket
/// addresses are specified.
#[derive(Derivative)]
#[derivative(Debug(bound = "D: Debug"))]
pub struct Connection<
    SockI: DualStackIpExt,
    WireI: DualStackIpExt,
    D: WeakDeviceIdentifier,
    BT: TcpBindingsTypes,
> {
    accept_queue: Option<
        AcceptQueue<
            TcpSocketId<SockI, D, BT>,
            BT::ReturnedBuffers,
            BT::ListenerNotifierOrProvidedBuffers,
        >,
    >,
    state: State<
        BT::Instant,
        BT::ReceiveBuffer,
        BT::SendBuffer,
        BT::ListenerNotifierOrProvidedBuffers,
    >,
    ip_sock: IpSock<WireI, D>,
    /// The user has indicated that this connection will never be used again, we
    /// keep the connection in the socketmap to perform the shutdown but it will
    /// be auto removed once the state reaches Closed.
    defunct: bool,
    /// In contrast to a hard error, which will cause a connection to be closed,
    /// a soft error will not abort the connection, but it can be read by either
    /// calling `get_socket_error`, or after the connection times out.
    soft_error: Option<ConnectionError>,
    /// Whether the handshake has finished or aborted.
    handshake_status: HandshakeStatus,
}

impl<SockI: DualStackIpExt, WireI: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes>
    Connection<SockI, WireI, D, BT>
{
    /// Updates this connection's state to reflect the error.
    ///
    /// The connection's soft error, if previously unoccupied, holds the error.
    fn on_icmp_error<CC: TcpCounterContext<SockI, D, BT>>(
        &mut self,
        core_ctx: &mut CC,
        id: &TcpSocketId<SockI, D, BT>,
        seq: SeqNum,
        error: IcmpErrorCode,
    ) -> (NewlyClosed, ShouldRetransmit) {
        let Connection { soft_error, state, .. } = self;
        let (new_soft_error, newly_closed, should_send) =
            state.on_icmp_error(&TcpCountersRefs::from_ctx(core_ctx, id), error, seq);
        *soft_error = soft_error.or(new_soft_error);
        (newly_closed, should_send)
    }
}

/// The Listener state.
///
/// State for sockets that participate in the passive open. Contrary to
/// [`Connection`], only the local address is specified.
#[derive(Derivative)]
#[derivative(Debug(bound = "D: Debug"))]
#[cfg_attr(
    test,
    derivative(
        PartialEq(
            bound = "BT::ReturnedBuffers: PartialEq, BT::ListenerNotifierOrProvidedBuffers: PartialEq, I::ListenerIpAddr: PartialEq"
        ),
        Eq(
            bound = "BT::ReturnedBuffers: Eq, BT::ListenerNotifierOrProvidedBuffers: Eq, I::ListenerIpAddr: Eq"
        ),
    )
)]
pub struct Listener<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes> {
    addr: ListenerAddr<I::ListenerIpAddr, D>,
    backlog: NonZeroUsize,
    accept_queue: AcceptQueue<
        TcpSocketId<I, D, BT>,
        BT::ReturnedBuffers,
        BT::ListenerNotifierOrProvidedBuffers,
    >,
    buffer_sizes: BufferSizes,
    // If ip sockets can be half-specified so that only the local address
    // is needed, we can construct an ip socket here to be reused.
}

impl<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes> Listener<I, D, BT> {
    fn new(
        addr: ListenerAddr<I::ListenerIpAddr, D>,
        backlog: NonZeroUsize,
        buffer_sizes: BufferSizes,
        notifier: BT::ListenerNotifierOrProvidedBuffers,
    ) -> Self {
        Self { addr, backlog, accept_queue: AcceptQueue::new(notifier), buffer_sizes }
    }
}

#[derive(Clone, Derivative)]
#[derivative(Debug(bound = "D: Debug"))]
#[cfg_attr(test, derive(Eq, PartialEq))]
pub struct BoundState<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes> {
    addr: ListenerAddr<I::ListenerIpAddr, D>,
    buffer_sizes: BufferSizes,
    socket_extra: Takeable<BT::ListenerNotifierOrProvidedBuffers>,
}

/// A TCP Socket ID.
#[derive(Derivative, GenericOverIp)]
#[generic_over_ip(I, Ip)]
#[derivative(Eq(bound = ""), PartialEq(bound = ""), Hash(bound = ""))]
pub struct TcpSocketId<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes>(
    StrongRc<I, D, BT>,
);

impl<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes> Clone
    for TcpSocketId<I, D, BT>
{
    #[cfg_attr(feature = "instrumented", track_caller)]
    fn clone(&self) -> Self {
        let Self(rc) = self;
        Self(StrongRc::clone(rc))
    }
}

impl<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes> TcpSocketId<I, D, BT> {
    pub(crate) fn new(
        socket_state: TcpSocketStateInner<I, D, BT>,
        socket_options: SocketOptions,
    ) -> (Self, PrimaryRc<I, D, BT>) {
        let primary = PrimaryRc::new(ReferenceState {
            locked_state: RwLock::new(TcpSocketState {
                socket_state,
                sharing: Default::default(),
                ip_options: Default::default(),
                socket_options,
            }),
            counters: Default::default(),
        });
        let socket = Self(PrimaryRc::clone_strong(&primary));
        (socket, primary)
    }

    pub(crate) fn new_cyclic<
        F: FnOnce(WeakTcpSocketId<I, D, BT>) -> TcpSocketStateInner<I, D, BT>,
    >(
        init: F,
        sharing: SharingState,
        socket_options: SocketOptions,
    ) -> (Self, PrimaryRc<I, D, BT>) {
        let primary = PrimaryRc::new_cyclic(move |weak| {
            let socket_state = init(WeakTcpSocketId(weak));
            ReferenceState {
                locked_state: RwLock::new(TcpSocketState {
                    socket_state,
                    sharing,
                    ip_options: Default::default(),
                    socket_options,
                }),
                counters: Default::default(),
            }
        });
        let socket = Self(PrimaryRc::clone_strong(&primary));
        (socket, primary)
    }

    /// Obtains the counters tracked for this TCP socket.
    pub fn counters(&self) -> &TcpCountersWithSocket<I> {
        let Self(rc) = self;
        &rc.counters
    }

    pub(crate) fn trace_id(&self) -> TraceResourceId<'_> {
        let Self(inner) = self;
        TraceResourceId::new(inner.resource_token())
    }

    /// Returns `SocketCookie` for the socket.
    pub fn socket_cookie(&self) -> SocketCookie {
        let Self(inner) = self;
        SocketCookie::new(inner.resource_token())
    }

    /// Returns `SocketInfo` for the socket.
    pub fn socket_info(&self) -> netstack3_base::socket::SocketInfo {
        let Self(inner) = self;
        netstack3_base::socket::SocketInfo {
            proto: I::map_ip(
                (),
                |()| EitherIpProto::V4(Ipv4Proto::Proto(IpProto::Tcp)),
                |()| EitherIpProto::V6(Ipv6Proto::Proto(IpProto::Tcp)),
            ),
            cookie: SocketCookie::new(inner.resource_token()),
        }
    }

    pub(crate) fn either(&self) -> EitherTcpSocketId<'_, D, BT> {
        I::map_ip_in(self, EitherTcpSocketId::V4, EitherTcpSocketId::V6)
    }

    pub(crate) fn get_bound_device<CC>(&self, core_ctx: &mut CC) -> Option<D>
    where
        CC: TcpContext<I, BT, WeakDeviceId = D>,
    {
        core_ctx.with_socket(self, |state| match &state.socket_state {
            TcpSocketStateInner::Unbound(state) => state.bound_device.clone(),
            TcpSocketStateInner::Listener(Listener { addr, .. })
            | TcpSocketStateInner::Bound(BoundState { addr, .. }) => addr.device.clone(),
            TcpSocketStateInner::Connected { conn, .. } => I::get_conn_info(&conn).device,
        })
    }
}

impl<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes> Debug
    for TcpSocketId<I, D, BT>
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let Self(rc) = self;
        f.debug_tuple("TcpSocketId").field(&StrongRc::debug_id(rc)).finish()
    }
}

impl<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes> TcpSocketId<I, D, BT> {
    pub(crate) fn downgrade(&self) -> WeakTcpSocketId<I, D, BT> {
        let Self(this) = self;
        WeakTcpSocketId(StrongRc::downgrade(this))
    }
}

impl<CC, I, BT> SocketMetadata<CC> for TcpSocketId<I, CC::WeakDeviceId, BT>
where
    CC: ?Sized + TcpContext<I, BT>,
    I: DualStackIpExt,
    BT: TcpBindingsTypes,
{
    fn socket_info(&self, _core_ctx: &mut CC) -> netstack3_base::socket::SocketInfo {
        self.socket_info()
    }

    fn marks(&self, core_ctx: &mut CC) -> Marks {
        core_ctx.with_socket(self, |state| state.socket_options.ip_options.marks.clone())
    }
}

/// A Weak TCP Socket ID.
#[derive(Derivative, GenericOverIp)]
#[generic_over_ip(I, Ip)]
#[derivative(Clone(bound = ""), Eq(bound = ""), PartialEq(bound = ""), Hash(bound = ""))]
pub struct WeakTcpSocketId<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes>(
    WeakRc<I, D, BT>,
);

impl<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes> Debug
    for WeakTcpSocketId<I, D, BT>
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let Self(rc) = self;
        f.debug_tuple("WeakTcpSocketId").field(&rc.debug_id()).finish()
    }
}

impl<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes>
    PartialEq<TcpSocketId<I, D, BT>> for WeakTcpSocketId<I, D, BT>
{
    fn eq(&self, other: &TcpSocketId<I, D, BT>) -> bool {
        let Self(this) = self;
        let TcpSocketId(other) = other;
        StrongRc::weak_ptr_eq(other, this)
    }
}

impl<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes> WeakTcpSocketId<I, D, BT> {
    /// Tries to upgrade to a strong reference.
    #[cfg_attr(feature = "instrumented", track_caller)]
    pub fn upgrade(&self) -> Option<TcpSocketId<I, D, BT>> {
        let Self(this) = self;
        this.upgrade().map(TcpSocketId)
    }
}

impl<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes>
    OrderedLockAccess<TcpSocketState<I, D, BT>> for TcpSocketId<I, D, BT>
{
    type Lock = RwLock<TcpSocketState<I, D, BT>>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        let Self(rc) = self;
        OrderedLockRef::new(&rc.locked_state)
    }
}

/// A borrow of either an IPv4 or IPv6 TCP socket.
///
/// This type is used to implement [`StateMachineDebugId`] in a way that doesn't
/// taint the state machine with IP-specific types, avoiding code generation
/// duplication.
#[derive(Derivative)]
#[derivative(Debug(bound = ""))]
pub(crate) enum EitherTcpSocketId<'a, D: WeakDeviceIdentifier, BT: TcpBindingsTypes> {
    #[derivative(Debug = "transparent")]
    V4(&'a TcpSocketId<Ipv4, D, BT>),
    #[derivative(Debug = "transparent")]
    V6(&'a TcpSocketId<Ipv6, D, BT>),
}

impl<D: WeakDeviceIdentifier, BT: TcpBindingsTypes> StateMachineDebugId
    for EitherTcpSocketId<'_, D, BT>
{
    fn trace_id(&self) -> TraceResourceId<'_> {
        match self {
            Self::V4(v4) => v4.trace_id(),
            Self::V6(v6) => v6.trace_id(),
        }
    }
}

/// The status of a handshake.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum HandshakeStatus {
    /// The handshake is still pending.
    Pending,
    /// The handshake is aborted.
    Aborted,
    /// The handshake is completed.
    Completed {
        /// Whether it has been reported to the user yet.
        reported: bool,
    },
}

impl HandshakeStatus {
    fn update_if_pending(&mut self, new_status: Self) -> bool {
        if *self == HandshakeStatus::Pending {
            *self = new_status;
            true
        } else {
            false
        }
    }
}

/// Resolves the demux local address and bound device for the `bind` operation.
fn bind_get_local_addr_and_device<I, BT, CC>(
    core_ctx: &mut CC,
    addr: Option<ZonedAddr<SocketIpAddr<I::Addr>, CC::DeviceId>>,
    bound_device: &Option<CC::WeakDeviceId>,
) -> Result<(Option<SocketIpAddr<I::Addr>>, Option<CC::WeakDeviceId>), LocalAddressError>
where
    I: DualStackIpExt,
    BT: TcpBindingsTypes,
    CC: TransportIpContext<I, BT>,
{
    let (local_ip, device) = match addr {
        Some(addr) => {
            // Extract the specified address and the device. The
            // device is either the one from the address or the one
            // to which the socket was previously bound.
            let (addr, required_device) = addr
                .resolve_addr_with_device(bound_device.clone())
                .map_err(LocalAddressError::Zone)?;

            // TCP sockets cannot bind to multicast addresses.
            // TODO(https://fxbug.dev/424874749): Put this check in a witness type.
            if addr.addr().is_multicast()
                || I::map_ip_in(addr.addr(), |ip| ip.is_limited_broadcast(), |_| false)
            {
                return Err(LocalAddressError::CannotBindToAddress);
            }

            core_ctx.with_devices_with_assigned_addr(addr.clone().into(), |mut assigned_to| {
                if !assigned_to.any(|d| {
                    required_device
                        .as_ref()
                        .map_or(true, |device| device == &EitherDeviceId::Strong(d))
                }) {
                    Err(LocalAddressError::AddressMismatch)
                } else {
                    Ok(())
                }
            })?;
            (Some(addr), required_device)
        }
        None => (None, bound_device.clone().map(EitherDeviceId::Weak)),
    };
    let weak_device = device.map(|d| d.as_weak().into_owned());
    Ok((local_ip, weak_device))
}

fn bind_install_in_demux<I, D, BC>(
    bindings_ctx: &mut BC,
    demux_socket_id: I::DemuxSocketId<D, BC>,
    local_ip: Option<SocketIpAddr<I::Addr>>,
    weak_device: Option<D>,
    port: Option<NonZeroU16>,
    sharing: SharingState,
    DemuxState { socketmap }: &mut DemuxState<I, D, BC>,
) -> Result<ListenerAddr<ListenerIpAddr<I::Addr, NonZeroU16>, D>, LocalAddressError>
where
    I: DualStackIpExt,
    BC: TcpBindingsTypes + RngContext,
    D: WeakDeviceIdentifier,
{
    let port = match port {
        None => {
            match netstack3_base::simple_randomized_port_alloc(
                &mut bindings_ctx.rng(),
                &local_ip,
                &TcpPortAlloc(socketmap),
                &None,
            ) {
                Some(port) => NonZeroU16::new(port).expect("ephemeral ports must be non-zero"),
                None => {
                    return Err(LocalAddressError::FailedToAllocateLocalPort);
                }
            }
        }
        Some(port) => port,
    };

    let addr = ListenerAddr {
        ip: ListenerIpAddr { addr: local_ip, identifier: port },
        device: weak_device,
    };
    let sharing = ListenerSharingState { sharing, listening: false };

    let _inserted = socketmap
        .listeners_mut()
        .try_insert(addr.clone(), sharing, demux_socket_id)
        .map_err(Into::<LocalAddressError>::into)?;

    Ok(addr)
}

fn try_update_listener_sharing<I, CC, BT>(
    core_ctx: MaybeDualStack<
        (&mut CC::DualStackIpTransportAndDemuxCtx<'_>, CC::DualStackConverter),
        (&mut CC::SingleStackIpTransportAndDemuxCtx<'_>, CC::SingleStackConverter),
    >,
    id: &TcpSocketId<I, CC::WeakDeviceId, BT>,
    addr: ListenerAddr<I::ListenerIpAddr, CC::WeakDeviceId>,
    sharing: &ListenerSharingState,
    new_sharing: ListenerSharingState,
) -> Result<(), UpdateSharingError>
where
    I: DualStackIpExt,
    CC: TcpContext<I, BT>,
    BT: TcpBindingsTypes,
{
    match core_ctx {
        MaybeDualStack::NotDualStack((core_ctx, converter)) => {
            core_ctx.with_demux_mut(|DemuxState { socketmap }| {
                let mut entry = socketmap
                    .listeners_mut()
                    .entry(&I::into_demux_socket_id(id.clone()), &converter.convert(addr))
                    .expect("invalid listener id");
                entry.try_update_sharing(sharing, new_sharing)
            })
        }
        MaybeDualStack::DualStack((core_ctx, converter)) => match converter.convert(addr) {
            ListenerAddr { ip: DualStackListenerIpAddr::ThisStack(ip), device } => {
                TcpDemuxContext::<I, _, _>::with_demux_mut(core_ctx, |DemuxState { socketmap }| {
                    let mut entry = socketmap
                        .listeners_mut()
                        .entry(&I::into_demux_socket_id(id.clone()), &ListenerAddr { ip, device })
                        .expect("invalid listener id");
                    entry.try_update_sharing(sharing, new_sharing)
                })
            }
            ListenerAddr { ip: DualStackListenerIpAddr::OtherStack(ip), device } => {
                let demux_id = core_ctx.into_other_demux_socket_id(id.clone());
                TcpDemuxContext::<I::OtherVersion, _, _>::with_demux_mut(
                    core_ctx,
                    |DemuxState { socketmap }| {
                        let mut entry = socketmap
                            .listeners_mut()
                            .entry(&demux_id, &ListenerAddr { ip, device })
                            .expect("invalid listener id");
                        entry.try_update_sharing(sharing, new_sharing)
                    },
                )
            }
            ListenerAddr { ip: DualStackListenerIpAddr::BothStacks(port), device } => {
                let other_demux_id = core_ctx.into_other_demux_socket_id(id.clone());
                let demux_id = I::into_demux_socket_id(id.clone());
                core_ctx.with_both_demux_mut(
                    |DemuxState { socketmap: this_socketmap, .. },
                     DemuxState { socketmap: other_socketmap, .. }| {
                        let this_stack_listener_addr = ListenerAddr {
                            ip: ListenerIpAddr { addr: None, identifier: port },
                            device: device.clone(),
                        };
                        let mut this_stack_entry = this_socketmap
                            .listeners_mut()
                            .entry(&demux_id, &this_stack_listener_addr)
                            .expect("invalid listener id");
                        this_stack_entry.try_update_sharing(sharing, new_sharing)?;
                        let mut other_stack_entry = other_socketmap
                            .listeners_mut()
                            .entry(
                                &other_demux_id,
                                &ListenerAddr {
                                    ip: ListenerIpAddr { addr: None, identifier: port },
                                    device,
                                },
                            )
                            .expect("invalid listener id");
                        match other_stack_entry.try_update_sharing(sharing, new_sharing) {
                            Ok(()) => Ok(()),
                            Err(err) => {
                                this_stack_entry
                                    .try_update_sharing(&new_sharing, *sharing)
                                    .expect("failed to revert the sharing setting");
                                Err(err)
                            }
                        }
                    },
                )
            }
        },
    }
}

struct ErrorReporter<'a, I, R, S, ActiveOpen> {
    state: &'a mut State<I, R, S, ActiveOpen>,
    soft_error: &'a mut Option<ConnectionError>,
}

impl<'a, I, R, S, ActiveOpen> ErrorReporter<'a, I, R, S, ActiveOpen> {
    fn report_error(self) -> Option<ConnectionError> {
        let Self { state, soft_error } = self;
        if let State::Closed(Closed { reason }) = state {
            if let Some(hard_error) = reason.take() {
                return Some(hard_error);
            }
        }
        soft_error.take()
    }

    fn new(
        state: &'a mut State<I, R, S, ActiveOpen>,
        soft_error: &'a mut Option<ConnectionError>,
    ) -> Self {
        Self { state, soft_error }
    }
}

/// The TCP socket API.
pub struct TcpApi<I: Ip, C>(C, IpVersionMarker<I>);

impl<I: Ip, C> TcpApi<I, C> {
    /// Creates a new `TcpApi` from `ctx`.
    pub fn new(ctx: C) -> Self {
        Self(ctx, IpVersionMarker::new())
    }
}

/// A local alias for [`TcpSocketId`] for use in [`TcpApi`].
///
/// TODO(https://github.com/rust-lang/rust/issues/8995): Make this an inherent
/// associated type.
type TcpApiSocketId<I, C> = TcpSocketId<
    I,
    <<C as ContextPair>::CoreContext as DeviceIdContext<AnyDevice>>::WeakDeviceId,
    <C as ContextPair>::BindingsContext,
>;

impl<I, C> TcpApi<I, C>
where
    I: DualStackIpExt,
    C: ContextPair,
    C::CoreContext: TcpContext<I, C::BindingsContext>,
    C::BindingsContext: TcpBindingsContext<
        <<C as ContextPair>::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId,
    >,
{
    pub(crate) fn core_ctx(&mut self) -> &mut C::CoreContext {
        let Self(pair, IpVersionMarker { .. }) = self;
        pair.core_ctx()
    }

    pub(crate) fn contexts(&mut self) -> (&mut C::CoreContext, &mut C::BindingsContext) {
        let Self(pair, IpVersionMarker { .. }) = self;
        pair.contexts()
    }

    /// Creates a new socket in unbound state.
    pub fn create(
        &mut self,
        socket_extra: <C::BindingsContext as TcpBindingsTypes>::ListenerNotifierOrProvidedBuffers,
    ) -> TcpApiSocketId<I, C> {
        let (core_ctx, bindings_ctx) = self.contexts();
        let settings = bindings_ctx.settings();
        let buffer_sizes = BufferSizes {
            send: settings.send_buffer.default().get(),
            receive: settings.receive_buffer.default().get(),
        };
        core_ctx.with_all_sockets_mut(|all_sockets| {
            let (sock, primary) = TcpSocketId::new(
                TcpSocketStateInner::Unbound(Unbound {
                    bound_device: Default::default(),
                    buffer_sizes,
                    socket_extra: Takeable::new(socket_extra),
                }),
                SocketOptions::default(),
            );
            assert_matches::assert_matches!(
                all_sockets.insert(sock.clone(), TcpSocketSetEntry::Primary(primary)),
                None
            );
            sock
        })
    }

    /// Binds an unbound socket to a local socket address.
    ///
    /// Requests that the given socket be bound to the local address, if one is
    /// provided; otherwise to all addresses. If `port` is specified (is
    /// `Some`), the socket will be bound to that port. Otherwise a port will be
    /// selected to not conflict with existing bound or connected sockets.
    pub fn bind(
        &mut self,
        id: &TcpApiSocketId<I, C>,
        addr: Option<
            ZonedAddr<
                SpecifiedAddr<I::Addr>,
                <C::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId,
            >,
        >,
        port: Option<NonZeroU16>,
    ) -> Result<(), BindError> {
        #[derive(GenericOverIp)]
        #[generic_over_ip(I, Ip)]
        enum BindAddr<I: DualStackIpExt, D> {
            BindInBothStacks,
            BindInOneStack(
                EitherStack<
                    Option<ZonedAddr<SocketIpAddr<I::Addr>, D>>,
                    Option<ZonedAddr<SocketIpAddr<<I::OtherVersion as Ip>::Addr>, D>>,
                >,
            ),
        }
        debug!("bind {id:?} to {addr:?}:{port:?}");
        let bind_addr = match addr {
            None => I::map_ip(
                (),
                |()| BindAddr::BindInOneStack(EitherStack::ThisStack(None)),
                |()| BindAddr::BindInBothStacks,
            ),
            Some(addr) => match DualStackLocalIp::<I, _>::new(addr) {
                DualStackLocalIp::ThisStack(addr) => {
                    BindAddr::BindInOneStack(EitherStack::ThisStack(Some(addr)))
                }
                DualStackLocalIp::OtherStack(addr) => {
                    BindAddr::BindInOneStack(EitherStack::OtherStack(addr))
                }
            },
        };

        let (core_ctx, bindings_ctx) = self.contexts();
        let result = core_ctx.with_socket_mut_transport_demux(id, |core_ctx, socket_state| {
            let TcpSocketState { socket_state, sharing, ip_options, socket_options: _ } = socket_state;
            let Unbound { bound_device, buffer_sizes, socket_extra } =
                match socket_state {
                    TcpSocketStateInner::Unbound(u) => u,
                    TcpSocketStateInner::Bound(_)
                    |TcpSocketStateInner::Listener(_)
                    | TcpSocketStateInner::Connected{..}
                     => return Err(BindError::AlreadyBound),
                };

            let listener_addr = match core_ctx {
                MaybeDualStack::NotDualStack((core_ctx, converter)) => match bind_addr {
                    BindAddr::BindInOneStack(EitherStack::ThisStack(local_addr)) => {
                        let (local_addr, device) = bind_get_local_addr_and_device(core_ctx, local_addr, bound_device)?;
                        let addr =
                            core_ctx.with_demux_mut(|demux| {
                                bind_install_in_demux(
                                    bindings_ctx,
                                    I::into_demux_socket_id(id.clone()),
                                    local_addr,
                                    device,
                                    port,
                                    *sharing,
                                    demux,
                                )
                            })?;
                        converter.convert_back(addr)
                    }
                    BindAddr::BindInOneStack(EitherStack::OtherStack(_)) | BindAddr::BindInBothStacks => {
                        return Err(LocalAddressError::CannotBindToAddress.into());
                    }
                },
                MaybeDualStack::DualStack((core_ctx, converter)) => {
                    let bind_addr = match (
                            core_ctx.dual_stack_enabled(&ip_options),
                            bind_addr
                        ) {
                        // Allow binding in both stacks when dual stack is
                        // enabled.
                        (true, BindAddr::BindInBothStacks)
                            => BindAddr::<I, _>::BindInBothStacks,
                        // Only bind in this stack if dual stack is not enabled.
                        (false, BindAddr::BindInBothStacks)
                            => BindAddr::BindInOneStack(EitherStack::ThisStack(None)),
                        // Binding to this stack is always allowed.
                        (true | false, BindAddr::BindInOneStack(EitherStack::ThisStack(ip)))
                            => BindAddr::BindInOneStack(EitherStack::ThisStack(ip)),
                        // Can bind to the other stack only when dual stack is
                        // enabled, otherwise an error is returned.
                        (true, BindAddr::BindInOneStack(EitherStack::OtherStack(ip)))
                            => BindAddr::BindInOneStack(EitherStack::OtherStack(ip)),
                        (false, BindAddr::BindInOneStack(EitherStack::OtherStack(_)))
                            => return Err(LocalAddressError::CannotBindToAddress.into()),
                    };
                    match bind_addr {
                        BindAddr::BindInOneStack(EitherStack::ThisStack(addr)) => {
                            let (addr, device) = bind_get_local_addr_and_device::<I, _, _>(core_ctx, addr, bound_device)?;
                            let ListenerAddr { ip, device } =
                                core_ctx.with_demux_mut(|demux: &mut DemuxState<I, _, _>| {
                                    bind_install_in_demux(
                                        bindings_ctx,
                                        I::into_demux_socket_id(id.clone()),
                                        addr,
                                        device,
                                        port,
                                        *sharing,
                                        demux,
                                    )
                                })?;
                            converter.convert_back(ListenerAddr {
                                ip: DualStackListenerIpAddr::ThisStack(ip),
                                device,
                            })
                        }
                        BindAddr::BindInOneStack(EitherStack::OtherStack(addr)) => {
                            let other_demux_id = core_ctx.into_other_demux_socket_id(id.clone());
                            let (addr, device) = bind_get_local_addr_and_device::<I::OtherVersion, _, _>(core_ctx, addr, bound_device)?;
                            let ListenerAddr { ip, device } =
                                core_ctx.with_demux_mut(|demux: &mut DemuxState<I::OtherVersion, _, _>| {
                                    bind_install_in_demux(
                                        bindings_ctx,
                                        other_demux_id,
                                        addr,
                                        device,
                                        port,
                                        *sharing,
                                        demux,
                                    )
                                })?;
                            converter.convert_back(ListenerAddr {
                                ip: DualStackListenerIpAddr::OtherStack(ip),
                                device,
                            })
                        }
                        BindAddr::BindInBothStacks => {
                            let other_demux_id = core_ctx.into_other_demux_socket_id(id.clone());
                            let (port, device) =
                                core_ctx.with_both_demux_mut(|demux, other_demux| {
                                    // We need to allocate the port for both
                                    // stacks before `bind_inner` tries to make
                                    // a decision, because it might give two
                                    // unrelated ports which is undesired.
                                    let port_alloc = TcpDualStackPortAlloc(
                                        &demux.socketmap,
                                        &other_demux.socketmap
                                    );
                                    let port = match port {
                                        Some(port) => port,
                                        None => match netstack3_base::simple_randomized_port_alloc(
                                            &mut bindings_ctx.rng(),
                                            &(),
                                            &port_alloc,
                                            &(),
                                        ){
                                            Some(port) => NonZeroU16::new(port)
                                                .expect("ephemeral ports must be non-zero"),
                                            None => {
                                                return Err(LocalAddressError::FailedToAllocateLocalPort);
                                            }
                                        }
                                    };
                                    let this_stack_addr = bind_install_in_demux(
                                        bindings_ctx,
                                        I::into_demux_socket_id(id.clone()),
                                        None,
                                        bound_device.clone(),
                                        Some(port),
                                        *sharing,
                                        demux,
                                    )?;
                                    match bind_install_in_demux(
                                        bindings_ctx,
                                        other_demux_id,
                                        None,
                                        bound_device.clone(),
                                        Some(port),
                                        *sharing,
                                        other_demux,
                                    ) {
                                        Ok(ListenerAddr { ip, device }) => {
                                            assert_eq!(this_stack_addr.ip.identifier, ip.identifier);
                                            Ok((port, device))
                                        }
                                        Err(err) => {
                                            demux.socketmap.listeners_mut().remove(&I::into_demux_socket_id(id.clone()), &this_stack_addr).expect("failed to unbind");
                                            Err(err)
                                        }
                                    }
                                })?;
                            converter.convert_back(ListenerAddr {
                                ip: DualStackListenerIpAddr::BothStacks(port),
                                device,
                            })
                        }
                    }
                },
            };

            *socket_state = TcpSocketStateInner::Bound(BoundState {
                addr: listener_addr,
                buffer_sizes: buffer_sizes.clone(),
                socket_extra: Takeable::from_ref(socket_extra.to_ref()),
            });

            Ok(())
        });
        match &result {
            Err(BindError::LocalAddressError(LocalAddressError::FailedToAllocateLocalPort)) => {
                core_ctx.increment_both(id, |c| &c.failed_port_reservations);
            }
            Err(_) | Ok(_) => {}
        }
        result
    }

    /// Listens on an already bound socket.
    pub fn listen(
        &mut self,
        id: &TcpApiSocketId<I, C>,
        backlog: NonZeroUsize,
    ) -> Result<(), ListenError> {
        debug!("listen on {id:?} with backlog {backlog}");
        self.core_ctx().with_socket_mut_transport_demux(id, |core_ctx, socket_state| {
            let TcpSocketState { socket_state, sharing, ip_options: _, socket_options: _ } =
                socket_state;
            let BoundState { addr, buffer_sizes, socket_extra } = match socket_state {
                TcpSocketStateInner::Bound(bound_state) => bound_state,
                TcpSocketStateInner::Connected { .. }
                | TcpSocketStateInner::Unbound(_)
                | TcpSocketStateInner::Listener(_) => {
                    return Err(ListenError::NotSupported);
                }
            };
            try_update_listener_sharing::<_, C::CoreContext, _>(
                core_ctx,
                id,
                addr.clone(),
                &ListenerSharingState { sharing: *sharing, listening: false },
                ListenerSharingState { sharing: *sharing, listening: true },
            )
            .map_err(|UpdateSharingError| ListenError::ListenerExists)?;

            *socket_state = TcpSocketStateInner::Listener(Listener::new(
                addr.clone(),
                backlog,
                buffer_sizes.clone(),
                socket_extra.to_ref().take(),
            ));
            Ok(())
        })
    }

    /// Accepts an established socket from the queue of a listener socket.
    ///
    /// Note: The accepted socket will have the marks of the incoming SYN
    /// instead of the listener itself.
    pub fn accept(
        &mut self,
        id: &TcpApiSocketId<I, C>,
    ) -> Result<
        (
            TcpApiSocketId<I, C>,
            SocketAddr<I::Addr, <C::CoreContext as DeviceIdContext<AnyDevice>>::WeakDeviceId>,
            <C::BindingsContext as TcpBindingsTypes>::ReturnedBuffers,
        ),
        AcceptError,
    > {
        let (conn_id, client_buffers) = self.core_ctx().with_socket_mut(id, |socket_state| {
            debug!("accept on {id:?}");
            let accept_queue = match &mut socket_state.socket_state {
                TcpSocketStateInner::Listener(Listener { accept_queue, .. }) => accept_queue,
                TcpSocketStateInner::Unbound(_)
                | TcpSocketStateInner::Bound(_)
                | TcpSocketStateInner::Connected { .. } => {
                    return Err(AcceptError::NotSupported);
                }
            };
            let (conn_id, client_buffers) =
                accept_queue.pop_ready().ok_or(AcceptError::WouldBlock)?;

            Ok::<_, AcceptError>((conn_id, client_buffers))
        })?;

        let remote_addr =
            self.core_ctx().with_socket_mut_and_converter(&conn_id, |socket_state, _converter| {
                let conn_and_addr = assert_matches!(
                    &mut socket_state.socket_state,
                    TcpSocketStateInner::Connected{ conn, .. } => conn,
                    "invalid socket ID"
                );
                *I::get_accept_queue_mut(conn_and_addr) = None;
                let ConnectionInfo { local_addr: _, remote_addr, device: _ } =
                    I::get_conn_info(conn_and_addr);
                remote_addr
            });

        debug!("accepted connection {conn_id:?} from {remote_addr:?} on {id:?}");
        Ok((conn_id, remote_addr, client_buffers))
    }

    /// Connects a socket to a remote address.
    ///
    /// When the method returns, the connection is not guaranteed to be
    /// established. It is up to the caller (Bindings) to determine when the
    /// connection has been established. Bindings are free to use anything
    /// available on the platform to check, for instance, signals.
    pub fn connect(
        &mut self,
        id: &TcpApiSocketId<I, C>,
        remote_ip: Option<
            ZonedAddr<
                SpecifiedAddr<I::Addr>,
                <C::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId,
            >,
        >,
        remote_port: NonZeroU16,
    ) -> Result<(), ConnectError> {
        let (core_ctx, bindings_ctx) = self.contexts();
        let result = core_ctx.with_socket_mut_generators_transport_demux(
            id,
            |core_ctx, socket_state, isn, timestamp_offset| {
                let TcpSocketState { socket_state, sharing, ip_options, socket_options } =
                    socket_state;
                debug!("connect on {id:?} to {remote_ip:?}:{remote_port}");
                let remote_ip = DualStackRemoteIp::<I, _>::new(remote_ip);
                let (bound_device, local_addr, buffer_sizes, socket_extra) = match socket_state {
                    TcpSocketStateInner::Connected { conn, timer: _ } => {
                        let (handshake_status, error_reporter) = match core_ctx {
                            MaybeDualStack::NotDualStack((_core_ctx, converter)) => {
                                let (conn, _addr) = converter.convert(conn);
                                (
                                    &mut conn.handshake_status,
                                    ErrorReporter::new(&mut conn.state, &mut conn.soft_error),
                                )
                            }
                            MaybeDualStack::DualStack((_core_ctx, converter)) => {
                                match converter.convert(conn) {
                                    EitherStack::ThisStack((conn, _addr)) => (
                                        &mut conn.handshake_status,
                                        ErrorReporter::new(&mut conn.state, &mut conn.soft_error),
                                    ),
                                    EitherStack::OtherStack((conn, _addr)) => (
                                        &mut conn.handshake_status,
                                        ErrorReporter::new(&mut conn.state, &mut conn.soft_error),
                                    ),
                                }
                            }
                        };
                        match handshake_status {
                            HandshakeStatus::Pending => return Err(ConnectError::Pending),
                            HandshakeStatus::Aborted => {
                                return Err(error_reporter
                                    .report_error()
                                    .map(ConnectError::ConnectionError)
                                    .unwrap_or(ConnectError::Aborted));
                            }
                            HandshakeStatus::Completed { reported } => {
                                if *reported {
                                    return Err(ConnectError::Completed);
                                } else {
                                    *reported = true;
                                    return Ok(());
                                }
                            }
                        }
                    }
                    TcpSocketStateInner::Unbound(Unbound {
                        bound_device,
                        socket_extra,
                        buffer_sizes,
                    }) => (
                        bound_device.clone(),
                        DualStackTuple::<I, _>::new(None, None),
                        *buffer_sizes,
                        socket_extra.to_ref(),
                    ),
                    TcpSocketStateInner::Listener(_) => {
                        return Err(ConnectError::Listener);
                    }
                    TcpSocketStateInner::Bound(BoundState { addr, buffer_sizes, socket_extra }) => {
                        let local_addr = match &core_ctx {
                            MaybeDualStack::DualStack((_core_ctx, converter)) => {
                                match converter.convert(addr.clone()) {
                                    ListenerAddr {
                                        ip: DualStackListenerIpAddr::ThisStack(ip),
                                        device,
                                    } => {
                                        DualStackTuple::new(Some(ListenerAddr { ip, device }), None)
                                    }
                                    ListenerAddr {
                                        ip: DualStackListenerIpAddr::OtherStack(ip),
                                        device,
                                    } => {
                                        DualStackTuple::new(None, Some(ListenerAddr { ip, device }))
                                    }
                                    ListenerAddr {
                                        ip: DualStackListenerIpAddr::BothStacks(port),
                                        device,
                                    } => DualStackTuple::new(
                                        Some(ListenerAddr {
                                            ip: ListenerIpAddr { addr: None, identifier: port },
                                            device: device.clone(),
                                        }),
                                        Some(ListenerAddr {
                                            ip: ListenerIpAddr { addr: None, identifier: port },
                                            device,
                                        }),
                                    ),
                                }
                            }
                            MaybeDualStack::NotDualStack((_core_ctx, converter)) => {
                                DualStackTuple::new(Some(converter.convert(addr.clone())), None)
                            }
                        };
                        (None, local_addr, *buffer_sizes, socket_extra.to_ref())
                    }
                };
                // Local addr is a tuple of (this_stack, other_stack) bound
                // local address.
                let local_addr = local_addr.into_inner();
                match (core_ctx, local_addr, remote_ip) {
                    // If not dual stack, we allow the connect operation if socket
                    // was not bound or bound to a this-stack local address before,
                    // and the remote address also belongs to this stack.
                    (
                        MaybeDualStack::NotDualStack((core_ctx, converter)),
                        (local_addr_this_stack, None),
                        DualStackRemoteIp::ThisStack(remote_ip),
                    ) => {
                        *socket_state = connect_inner(
                            core_ctx,
                            bindings_ctx,
                            id,
                            isn,
                            timestamp_offset,
                            LocalAddrForConnect::from_local_addr(
                                local_addr_this_stack.clone(),
                                bound_device,
                            ),
                            remote_ip,
                            remote_port,
                            socket_extra,
                            buffer_sizes,
                            socket_options,
                            *sharing,
                            SingleStackDemuxStateAccessor(
                                &I::into_demux_socket_id(id.clone()),
                                local_addr_this_stack,
                            ),
                            |conn, addr| converter.convert_back((conn, addr)),
                            <C::CoreContext as CoreTimerContext<_, _>>::convert_timer,
                        )?;
                        Ok(())
                    }
                    // If dual stack, we can perform a this-stack only
                    // connection as long as we're not *only* bound in the other
                    // stack.
                    (
                        MaybeDualStack::DualStack((core_ctx, converter)),
                        (local_addr_this_stack, local_addr_other_stack @ None)
                        | (local_addr_this_stack @ Some(_), local_addr_other_stack @ Some(_)),
                        DualStackRemoteIp::ThisStack(remote_ip),
                    ) => {
                        *socket_state = connect_inner(
                            core_ctx,
                            bindings_ctx,
                            id,
                            isn,
                            timestamp_offset,
                            LocalAddrForConnect::from_local_addr(
                                local_addr_this_stack.clone(),
                                bound_device,
                            ),
                            remote_ip,
                            remote_port,
                            socket_extra,
                            buffer_sizes,
                            socket_options,
                            *sharing,
                            DualStackDemuxStateAccessor(
                                id,
                                DualStackTuple::new(local_addr_this_stack, local_addr_other_stack),
                            ),
                            |conn, addr| {
                                converter.convert_back(EitherStack::ThisStack((conn, addr)))
                            },
                            <C::CoreContext as CoreTimerContext<_, _>>::convert_timer,
                        )?;
                        Ok(())
                    }
                    // If dual stack, we can perform an other-stack only
                    // connection as long as we're not *only* bound in this
                    // stack.
                    (
                        MaybeDualStack::DualStack((core_ctx, converter)),
                        (local_addr_this_stack @ None, local_addr_other_stack)
                        | (local_addr_this_stack @ Some(_), local_addr_other_stack @ Some(_)),
                        DualStackRemoteIp::OtherStack(remote_ip),
                    ) => {
                        if !core_ctx.dual_stack_enabled(ip_options) {
                            return Err(ConnectError::NoRoute);
                        }
                        *socket_state = connect_inner(
                            core_ctx,
                            bindings_ctx,
                            id,
                            isn,
                            timestamp_offset,
                            LocalAddrForConnect::from_local_addr(
                                local_addr_other_stack.clone(),
                                bound_device,
                            ),
                            remote_ip,
                            remote_port,
                            socket_extra,
                            buffer_sizes,
                            socket_options,
                            *sharing,
                            DualStackDemuxStateAccessor(
                                id,
                                DualStackTuple::new(local_addr_this_stack, local_addr_other_stack),
                            ),
                            |conn, addr| {
                                converter.convert_back(EitherStack::OtherStack((conn, addr)))
                            },
                            <C::CoreContext as CoreTimerContext<_, _>>::convert_timer,
                        )?;
                        Ok(())
                    }
                    // Not possible for a non-dual-stack socket to bind in the other
                    // stack.
                    (
                        MaybeDualStack::NotDualStack(_),
                        (_, Some(_other_stack_local_addr)),
                        DualStackRemoteIp::ThisStack(_) | DualStackRemoteIp::OtherStack(_),
                    ) => unreachable!("The socket cannot be bound in the other stack"),
                    // Can't connect from one stack to the other.
                    (
                        MaybeDualStack::DualStack(_),
                        (None, Some(_other_stack_local_addr)),
                        DualStackRemoteIp::ThisStack(_),
                    ) => Err(ConnectError::NoRoute),
                    // Can't connect from one stack to the other.
                    (
                        MaybeDualStack::DualStack(_) | MaybeDualStack::NotDualStack(_),
                        (Some(_this_stack_local_addr), _),
                        DualStackRemoteIp::OtherStack(_),
                    ) => Err(ConnectError::NoRoute),
                    // Can't connect to the other stack for non-dual-stack sockets.
                    (
                        MaybeDualStack::NotDualStack(_),
                        (None, None),
                        DualStackRemoteIp::OtherStack(_),
                    ) => Err(ConnectError::NoRoute),
                }
            },
        );
        match &result {
            Ok(()) => {}
            Err(err) => {
                core_ctx.increment_both(id, |counters| &counters.failed_connection_attempts);
                match err {
                    ConnectError::NoRoute => {
                        core_ctx
                            .increment_both(id, |counters| &counters.active_open_no_route_errors);
                    }
                    ConnectError::NoPort => {
                        core_ctx.increment_both(id, |counters| &counters.failed_port_reservations);
                    }
                    _ => {}
                }
            }
        }
        result
    }

    /// Closes a socket.
    pub fn close(&mut self, id: TcpApiSocketId<I, C>) {
        debug!("close on {id:?}");
        let (core_ctx, bindings_ctx) = self.contexts();
        let (destroy, pending) =
            core_ctx.with_socket_mut_transport_demux(&id, |core_ctx, socket_state| {
                let TcpSocketState { socket_state, sharing: _, ip_options: _, socket_options } =
                    socket_state;
                match socket_state {
                    TcpSocketStateInner::Unbound(_) => (true, None),
                    TcpSocketStateInner::Bound(BoundState { addr, .. })
                    | TcpSocketStateInner::Listener(Listener { addr, .. }) => {
                        match core_ctx {
                            MaybeDualStack::NotDualStack((core_ctx, converter)) => {
                                TcpDemuxContext::<I, _, _>::with_demux_mut(
                                    core_ctx,
                                    |DemuxState { socketmap }| {
                                        socketmap
                                            .listeners_mut()
                                            .remove(
                                                &I::into_demux_socket_id(id.clone()),
                                                &converter.convert(addr),
                                            )
                                            .expect("failed to remove from socketmap");
                                    },
                                );
                            }
                            MaybeDualStack::DualStack((core_ctx, converter)) => {
                                match converter.convert(addr.clone()) {
                                    ListenerAddr {
                                        ip: DualStackListenerIpAddr::ThisStack(ip),
                                        device,
                                    } => TcpDemuxContext::<I, _, _>::with_demux_mut(
                                        core_ctx,
                                        |DemuxState { socketmap }| {
                                            socketmap
                                                .listeners_mut()
                                                .remove(
                                                    &I::into_demux_socket_id(id.clone()),
                                                    &ListenerAddr { ip, device },
                                                )
                                                .expect("failed to remove from socketmap");
                                        },
                                    ),
                                    ListenerAddr {
                                        ip: DualStackListenerIpAddr::OtherStack(ip),
                                        device,
                                    } => {
                                        let other_demux_id =
                                            core_ctx.into_other_demux_socket_id(id.clone());
                                        TcpDemuxContext::<I::OtherVersion, _, _>::with_demux_mut(
                                            core_ctx,
                                            |DemuxState { socketmap }| {
                                                socketmap
                                                    .listeners_mut()
                                                    .remove(
                                                        &other_demux_id,
                                                        &ListenerAddr { ip, device },
                                                    )
                                                    .expect("failed to remove from socketmap");
                                            },
                                        );
                                    }
                                    ListenerAddr {
                                        ip: DualStackListenerIpAddr::BothStacks(port),
                                        device,
                                    } => {
                                        let other_demux_id =
                                            core_ctx.into_other_demux_socket_id(id.clone());
                                        core_ctx.with_both_demux_mut(|demux, other_demux| {
                                            demux
                                                .socketmap
                                                .listeners_mut()
                                                .remove(
                                                    &I::into_demux_socket_id(id.clone()),
                                                    &ListenerAddr {
                                                        ip: ListenerIpAddr {
                                                            addr: None,
                                                            identifier: port,
                                                        },
                                                        device: device.clone(),
                                                    },
                                                )
                                                .expect("failed to remove from socketmap");
                                            other_demux
                                                .socketmap
                                                .listeners_mut()
                                                .remove(
                                                    &other_demux_id,
                                                    &ListenerAddr {
                                                        ip: ListenerIpAddr {
                                                            addr: None,
                                                            identifier: port,
                                                        },
                                                        device,
                                                    },
                                                )
                                                .expect("failed to remove from socketmap");
                                        });
                                    }
                                }
                            }
                        };
                        // Move the listener down to a `Bound` state so it won't
                        // accept any more connections and close the accept
                        // queue.
                        if let TcpSocketStateInner::Listener(Listener {
                            addr,
                            backlog: _,
                            accept_queue,
                            buffer_sizes,
                        }) = socket_state
                        {
                            let (pending, socket_extra) = accept_queue.close();
                            let addr = addr.clone();
                            let buffer_sizes = buffer_sizes.clone();
                            *socket_state = TcpSocketStateInner::Bound(BoundState {
                                addr,
                                buffer_sizes,
                                socket_extra: Takeable::new(socket_extra),
                            });
                            (true, Some(pending))
                        } else {
                            (true, None)
                        }
                    }
                    TcpSocketStateInner::Connected { conn, timer } => {
                        fn do_close<SockI, WireI, CC, BC>(
                            core_ctx: &mut CC,
                            bindings_ctx: &mut BC,
                            id: &TcpSocketId<SockI, CC::WeakDeviceId, BC>,
                            demux_id: &WireI::DemuxSocketId<CC::WeakDeviceId, BC>,
                            socket_options: &SocketOptions,
                            conn: &mut Connection<SockI, WireI, CC::WeakDeviceId, BC>,
                            addr: &ConnAddr<
                                ConnIpAddr<<WireI as Ip>::Addr, NonZeroU16, NonZeroU16>,
                                CC::WeakDeviceId,
                            >,
                            timer: &mut BC::Timer,
                        ) -> bool
                        where
                            SockI: DualStackIpExt,
                            WireI: DualStackIpExt,
                            BC: TcpBindingsContext<CC::DeviceId>,
                            CC: TransportIpContext<WireI, BC>
                                + TcpDemuxContext<WireI, CC::WeakDeviceId, BC>
                                + TcpSocketContext<SockI, CC::WeakDeviceId, BC>,
                        {
                            // Ignore the result - errors are handled below after calling `close`.
                            let _: Result<(), CloseError> = conn.state.shutdown_recv();

                            conn.defunct = true;
                            let newly_closed = match conn.state.close(
                                &TcpCountersRefs::from_ctx(core_ctx, id),
                                CloseReason::Close { now: bindings_ctx.now() },
                                socket_options,
                            ) {
                                Err(CloseError::NoConnection) => NewlyClosed::No,
                                Err(CloseError::Closing) | Ok(NewlyClosed::No) => do_send_inner(
                                    &id,
                                    socket_options,
                                    conn,
                                    DoSendLimit::MultipleSegments,
                                    &addr,
                                    timer,
                                    core_ctx,
                                    bindings_ctx,
                                ),
                                Ok(NewlyClosed::Yes) => NewlyClosed::Yes,
                            };
                            // The connection transitions to closed because of
                            // this call, we need to unregister it from the
                            // socketmap.
                            handle_newly_closed(
                                core_ctx,
                                bindings_ctx,
                                newly_closed,
                                demux_id,
                                addr,
                                timer,
                            );
                            let now_closed = matches!(conn.state, State::Closed(_));
                            if now_closed {
                                debug_assert!(
                                    core_ctx.with_demux_mut(|DemuxState { socketmap }| {
                                        socketmap.conns_mut().entry(demux_id, addr).is_none()
                                    }),
                                    "lingering state in socketmap: demux_id: {:?}, addr: {:?}",
                                    demux_id,
                                    addr,
                                );
                                debug_assert_eq!(
                                    bindings_ctx.scheduled_instant(timer),
                                    None,
                                    "lingering timer for {:?}",
                                    id,
                                )
                            };
                            now_closed
                        }
                        let closed = match core_ctx {
                            MaybeDualStack::NotDualStack((core_ctx, converter)) => {
                                let (conn, addr) = converter.convert(conn);
                                do_close(
                                    core_ctx,
                                    bindings_ctx,
                                    &id,
                                    &I::into_demux_socket_id(id.clone()),
                                    socket_options,
                                    conn,
                                    addr,
                                    timer,
                                )
                            }
                            MaybeDualStack::DualStack((core_ctx, converter)) => {
                                match converter.convert(conn) {
                                    EitherStack::ThisStack((conn, addr)) => do_close(
                                        core_ctx,
                                        bindings_ctx,
                                        &id,
                                        &I::into_demux_socket_id(id.clone()),
                                        socket_options,
                                        conn,
                                        addr,
                                        timer,
                                    ),
                                    EitherStack::OtherStack((conn, addr)) => do_close(
                                        core_ctx,
                                        bindings_ctx,
                                        &id,
                                        &core_ctx.into_other_demux_socket_id(id.clone()),
                                        socket_options,
                                        conn,
                                        addr,
                                        timer,
                                    ),
                                }
                            }
                        };
                        (closed, None)
                    }
                }
            });

        close_pending_sockets(core_ctx, bindings_ctx, pending.into_iter().flatten());

        if destroy {
            destroy_socket(core_ctx, bindings_ctx, id);
        }
    }

    /// Shuts down a socket.
    ///
    /// For a connection, calling this function signals the other side of the
    /// connection that we will not be sending anything over the connection; The
    /// connection will be removed from the socketmap if the state moves to the
    /// `Closed` state.
    ///
    /// For a Listener, calling this function brings it back to bound state and
    /// shutdowns all the connection that is currently ready to be accepted.
    ///
    /// Returns Err(NoConnection) if the shutdown option does not apply.
    /// Otherwise, Whether a connection has been shutdown is returned, i.e., if
    /// the socket was a listener, the operation will succeed but false will be
    /// returned.
    pub fn shutdown(
        &mut self,
        id: &TcpApiSocketId<I, C>,
        shutdown_type: ShutdownType,
    ) -> Result<bool, NoConnection> {
        debug!("shutdown [{shutdown_type:?}] for {id:?}");
        let (core_ctx, bindings_ctx) = self.contexts();
        let (result, pending) =
            core_ctx.with_socket_mut_transport_demux(id, |core_ctx, socket_state| {
                let TcpSocketState { socket_state, sharing, ip_options: _, socket_options } =
                    socket_state;
                match socket_state {
                    TcpSocketStateInner::Unbound(_) => Err(NoConnection),
                    TcpSocketStateInner::Connected { conn, timer } => {
                        fn do_shutdown<SockI, WireI, CC, BC>(
                            core_ctx: &mut CC,
                            bindings_ctx: &mut BC,
                            id: &TcpSocketId<SockI, CC::WeakDeviceId, BC>,
                            demux_id: &WireI::DemuxSocketId<CC::WeakDeviceId, BC>,
                            socket_options: &SocketOptions,
                            conn: &mut Connection<SockI, WireI, CC::WeakDeviceId, BC>,
                            addr: &ConnAddr<
                                ConnIpAddr<<WireI as Ip>::Addr, NonZeroU16, NonZeroU16>,
                                CC::WeakDeviceId,
                            >,
                            timer: &mut BC::Timer,
                            shutdown_type: ShutdownType,
                        ) -> Result<(), NoConnection>
                        where
                            SockI: DualStackIpExt,
                            WireI: DualStackIpExt,
                            BC: TcpBindingsContext<CC::DeviceId>,
                            CC: TransportIpContext<WireI, BC>
                                + TcpDemuxContext<WireI, CC::WeakDeviceId, BC>
                                + TcpSocketContext<SockI, CC::WeakDeviceId, BC>,
                        {
                            let (shutdown_send, shutdown_receive) = shutdown_type.to_send_receive();
                            if shutdown_receive {
                                match conn.state.shutdown_recv() {
                                    Ok(()) => (),
                                    Err(CloseError::NoConnection) => return Err(NoConnection),
                                    Err(CloseError::Closing) => (),
                                }
                            }

                            if !shutdown_send {
                                return Ok(());
                            }

                            match conn.state.close(
                                &TcpCountersRefs::from_ctx(core_ctx, id),
                                CloseReason::Shutdown,
                                socket_options,
                            ) {
                                Ok(newly_closed) => {
                                    let newly_closed = match newly_closed {
                                        NewlyClosed::Yes => NewlyClosed::Yes,
                                        NewlyClosed::No => do_send_inner(
                                            id,
                                            socket_options,
                                            conn,
                                            DoSendLimit::MultipleSegments,
                                            addr,
                                            timer,
                                            core_ctx,
                                            bindings_ctx,
                                        ),
                                    };
                                    handle_newly_closed(
                                        core_ctx,
                                        bindings_ctx,
                                        newly_closed,
                                        demux_id,
                                        addr,
                                        timer,
                                    );
                                    Ok(())
                                }
                                Err(CloseError::NoConnection) => Err(NoConnection),
                                Err(CloseError::Closing) => Ok(()),
                            }
                        }
                        match core_ctx {
                            MaybeDualStack::NotDualStack((core_ctx, converter)) => {
                                let (conn, addr) = converter.convert(conn);
                                do_shutdown(
                                    core_ctx,
                                    bindings_ctx,
                                    id,
                                    &I::into_demux_socket_id(id.clone()),
                                    socket_options,
                                    conn,
                                    addr,
                                    timer,
                                    shutdown_type,
                                )?
                            }
                            MaybeDualStack::DualStack((core_ctx, converter)) => {
                                match converter.convert(conn) {
                                    EitherStack::ThisStack((conn, addr)) => do_shutdown(
                                        core_ctx,
                                        bindings_ctx,
                                        id,
                                        &I::into_demux_socket_id(id.clone()),
                                        socket_options,
                                        conn,
                                        addr,
                                        timer,
                                        shutdown_type,
                                    )?,
                                    EitherStack::OtherStack((conn, addr)) => do_shutdown(
                                        core_ctx,
                                        bindings_ctx,
                                        id,
                                        &core_ctx.into_other_demux_socket_id(id.clone()),
                                        socket_options,
                                        conn,
                                        addr,
                                        timer,
                                        shutdown_type,
                                    )?,
                                }
                            }
                        };
                        Ok((true, None))
                    }
                    TcpSocketStateInner::Bound(_) => Err(NoConnection),
                    TcpSocketStateInner::Listener(listener) => {
                        let (_shutdown_send, shutdown_receive) = shutdown_type.to_send_receive();

                        if !shutdown_receive {
                            return Ok((false, None));
                        }

                        let (pending, new_state) =
                            shut_down_listener_socket::<I, C::CoreContext, C::BindingsContext>(
                                core_ctx, id, listener, *sharing,
                            );
                        *socket_state = TcpSocketStateInner::Bound(new_state);
                        Ok((false, Some(pending)))
                    }
                }
            })?;

        close_pending_sockets(core_ctx, bindings_ctx, pending.into_iter().flatten());

        Ok(result)
    }

    /// Polls the state machine after data is dequeued from the receive buffer.
    ///
    /// Possibly sends a window update to the peer if enough data has been read
    /// from the buffer and we suspect that the peer is in SWS avoidance.
    ///
    /// This does nothing for a disconnected socket.
    pub fn on_receive_buffer_read(&mut self, id: &TcpApiSocketId<I, C>) {
        let (core_ctx, bindings_ctx) = self.contexts();
        core_ctx.with_socket_mut_transport_demux(
            id,
            |core_ctx, TcpSocketState { socket_state, sharing: _, ip_options: _, socket_options }| {
                let conn = match socket_state {
                    TcpSocketStateInner::Unbound(_)| TcpSocketStateInner::Bound(_)|
                    TcpSocketStateInner::Listener(_) => return,
                    TcpSocketStateInner::Connected { conn, .. } => conn,
                };

                let now = bindings_ctx.now();
                match core_ctx {
                    MaybeDualStack::NotDualStack((core_ctx, converter)) => {
                        let (conn, addr) = converter.convert(conn);
                        if let Some(ack) = conn.state.poll_receive_data_dequeued(now) {
                            send_tcp_segment(
                                core_ctx,
                                bindings_ctx,
                                Some(id),
                                Some(&conn.ip_sock),
                                addr.ip,
                                ack.into_empty(),
                                &socket_options.ip_options,
                            )
                        }
                    }
                    MaybeDualStack::DualStack((core_ctx, converter)) => {
                        match converter.convert(conn) {
                            EitherStack::ThisStack((conn, addr)) => {
                                if let Some(ack) = conn.state.poll_receive_data_dequeued(now) {
                                    send_tcp_segment(
                                        core_ctx,
                                        bindings_ctx,
                                        Some(id),
                                        Some(&conn.ip_sock),
                                        addr.ip,
                                        ack.into_empty(),
                                        &socket_options.ip_options,
                                    )
                                }
                            }
                            EitherStack::OtherStack((conn, addr)) => {
                                if let Some(ack) = conn.state.poll_receive_data_dequeued(now) {
                                    send_tcp_segment(
                                        core_ctx,
                                        bindings_ctx,
                                        Some(id),
                                        Some(&conn.ip_sock),
                                        addr.ip,
                                        ack.into_empty(),
                                        &socket_options.ip_options,
                                    )
                                }
                            }
                        }
                    }
                }
            },
        )
    }

    fn set_device_conn<SockI, WireI, CC>(
        core_ctx: &mut CC,
        bindings_ctx: &mut C::BindingsContext,
        addr: &mut ConnAddr<ConnIpAddr<WireI::Addr, NonZeroU16, NonZeroU16>, CC::WeakDeviceId>,
        demux_id: &WireI::DemuxSocketId<CC::WeakDeviceId, C::BindingsContext>,
        ip_options: &TcpIpSockOptions,
        conn: &mut Connection<SockI, WireI, CC::WeakDeviceId, C::BindingsContext>,
        new_device: Option<CC::DeviceId>,
    ) -> Result<(), SetDeviceError>
    where
        SockI: DualStackIpExt,
        WireI: DualStackIpExt,
        CC: TransportIpContext<WireI, C::BindingsContext>
            + TcpDemuxContext<WireI, CC::WeakDeviceId, C::BindingsContext>,
    {
        let ConnAddr {
            device: old_device,
            ip: ConnIpAddr { local: (local_ip, _), remote: (remote_ip, _) },
        } = addr;

        let update = SocketDeviceUpdate {
            local_ip: Some(local_ip.as_ref()),
            remote_ip: Some(remote_ip.as_ref()),
            old_device: old_device.as_ref(),
        };
        match update.check_update(new_device.as_ref()) {
            Ok(()) => (),
            Err(SocketDeviceUpdateNotAllowedError) => return Err(SetDeviceError::ZoneChange),
        }
        let new_socket = core_ctx
            .new_ip_socket(
                bindings_ctx,
                IpSocketArgs {
                    device: new_device.as_ref().map(EitherDeviceId::Strong),
                    local_ip: IpDeviceAddr::new_from_socket_ip_addr(*local_ip),
                    remote_ip: *remote_ip,
                    proto: IpProto::Tcp.into(),
                    options: ip_options,
                },
            )
            .map_err(|_: IpSockCreationError| SetDeviceError::Unroutable)?;
        let new_address = ConnAddr { device: new_socket.device().cloned(), ..addr.clone() };
        core_ctx.with_demux_mut(|DemuxState { socketmap }| {
            let entry = match socketmap.conns_mut().entry(demux_id, addr) {
                Some(entry) => entry,
                None => {
                    debug!("no demux entry for {addr:?} with {demux_id:?}");
                    // State must be closed or timewait if we have a bound
                    // and connected socket that is no longer present in the
                    // demux.
                    assert_matches!(&conn.state, State::Closed(_) | State::TimeWait(_));
                    // If the socket has already been removed from the
                    // demux, then we can update our address information
                    // locally.
                    *addr = new_address;
                    return Ok(());
                }
            };

            match entry.try_update_addr(new_address) {
                Ok(entry) => {
                    *addr = entry.get_addr().clone();
                    conn.ip_sock = new_socket;
                    Ok(())
                }
                Err((ExistsError, _entry)) => Err(SetDeviceError::Conflict),
            }
        })
    }

    /// Updates the `old_device` to the new device if it is allowed. Note that
    /// this `old_device` will be updated in-place, so it should come from the
    /// outside socketmap address.
    fn set_device_listener<WireI, D>(
        demux_id: &WireI::DemuxSocketId<D, C::BindingsContext>,
        ip_addr: ListenerIpAddr<WireI::Addr, NonZeroU16>,
        old_device: Option<D>,
        new_device: Option<&D>,
        DemuxState { socketmap }: &mut DemuxState<WireI, D, C::BindingsContext>,
    ) -> Result<(), SetDeviceError>
    where
        WireI: DualStackIpExt,
        D: WeakDeviceIdentifier,
    {
        let entry = socketmap
            .listeners_mut()
            .entry(demux_id, &ListenerAddr { ip: ip_addr, device: old_device.clone() })
            .expect("invalid ID");

        let update = SocketDeviceUpdate {
            local_ip: ip_addr.addr.as_ref().map(|a| a.as_ref()),
            remote_ip: None,
            old_device: old_device.as_ref(),
        };
        match update.check_update(new_device) {
            Ok(()) => (),
            Err(SocketDeviceUpdateNotAllowedError) => return Err(SetDeviceError::ZoneChange),
        }
        match entry.try_update_addr(ListenerAddr { device: new_device.cloned(), ip: ip_addr }) {
            Ok(_entry) => Ok(()),
            Err((ExistsError, _entry)) => Err(SetDeviceError::Conflict),
        }
    }

    /// Sets the device on a socket.
    ///
    /// Passing `None` clears the bound device.
    pub fn set_device(
        &mut self,
        id: &TcpApiSocketId<I, C>,
        new_device: Option<<C::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId>,
    ) -> Result<(), SetDeviceError> {
        let (core_ctx, bindings_ctx) = self.contexts();
        let weak_device = new_device.as_ref().map(|d| d.downgrade());
        core_ctx.with_socket_mut_transport_demux(id, move |core_ctx, socket_state| {
            debug!("set device on {id:?} to {new_device:?}");
            let TcpSocketState { socket_state, sharing: _, ip_options: _, socket_options } =
                socket_state;
            match socket_state {
                TcpSocketStateInner::Unbound(unbound) => {
                    unbound.bound_device = weak_device;
                    Ok(())
                }
                TcpSocketStateInner::Connected { conn, timer: _ } => {
                    let this_or_other_stack = match core_ctx {
                        MaybeDualStack::NotDualStack((core_ctx, converter)) => {
                            let (conn, addr) = converter.convert(conn);
                            EitherStack::ThisStack((
                                core_ctx.as_this_stack(),
                                conn,
                                addr,
                                I::into_demux_socket_id(id.clone()),
                            ))
                        }
                        MaybeDualStack::DualStack((core_ctx, converter)) => {
                            match converter.convert(conn) {
                                EitherStack::ThisStack((conn, addr)) => EitherStack::ThisStack((
                                    core_ctx.as_this_stack(),
                                    conn,
                                    addr,
                                    I::into_demux_socket_id(id.clone()),
                                )),
                                EitherStack::OtherStack((conn, addr)) => {
                                    let demux_id = core_ctx.into_other_demux_socket_id(id.clone());
                                    EitherStack::OtherStack((core_ctx, conn, addr, demux_id))
                                }
                            }
                        }
                    };
                    match this_or_other_stack {
                        EitherStack::ThisStack((core_ctx, conn, addr, demux_id)) => {
                            Self::set_device_conn::<_, I, _>(
                                core_ctx,
                                bindings_ctx,
                                addr,
                                &demux_id,
                                &socket_options.ip_options,
                                conn,
                                new_device,
                            )
                        }
                        EitherStack::OtherStack((core_ctx, conn, addr, demux_id)) => {
                            Self::set_device_conn::<_, I::OtherVersion, _>(
                                core_ctx,
                                bindings_ctx,
                                addr,
                                &demux_id,
                                &socket_options.ip_options,
                                conn,
                                new_device,
                            )
                        }
                    }
                }
                TcpSocketStateInner::Bound(BoundState { addr, .. })
                | TcpSocketStateInner::Listener(Listener { addr, .. }) => match core_ctx {
                    MaybeDualStack::NotDualStack((core_ctx, converter)) => {
                        let ListenerAddr { ip, device } = converter.convert(addr);
                        core_ctx.with_demux_mut(|demux| {
                            Self::set_device_listener(
                                &I::into_demux_socket_id(id.clone()),
                                ip.clone(),
                                device.clone(),
                                weak_device.as_ref(),
                                demux,
                            )?;
                            *device = weak_device;
                            Ok(())
                        })
                    }
                    MaybeDualStack::DualStack((core_ctx, converter)) => {
                        match converter.convert(addr) {
                            ListenerAddr { ip: DualStackListenerIpAddr::ThisStack(ip), device } => {
                                TcpDemuxContext::<I, _, _>::with_demux_mut(core_ctx, |demux| {
                                    Self::set_device_listener(
                                        &I::into_demux_socket_id(id.clone()),
                                        ip.clone(),
                                        device.clone(),
                                        weak_device.as_ref(),
                                        demux,
                                    )?;
                                    *device = weak_device;
                                    Ok(())
                                })
                            }
                            ListenerAddr {
                                ip: DualStackListenerIpAddr::OtherStack(ip),
                                device,
                            } => {
                                let other_demux_id =
                                    core_ctx.into_other_demux_socket_id(id.clone());
                                TcpDemuxContext::<I::OtherVersion, _, _>::with_demux_mut(
                                    core_ctx,
                                    |demux| {
                                        Self::set_device_listener(
                                            &other_demux_id,
                                            ip.clone(),
                                            device.clone(),
                                            weak_device.as_ref(),
                                            demux,
                                        )?;
                                        *device = weak_device;
                                        Ok(())
                                    },
                                )
                            }
                            ListenerAddr {
                                ip: DualStackListenerIpAddr::BothStacks(port),
                                device,
                            } => {
                                let other_demux_id =
                                    core_ctx.into_other_demux_socket_id(id.clone());
                                core_ctx.with_both_demux_mut(|demux, other_demux| {
                                    let old_device = device.clone();
                                    Self::set_device_listener(
                                        &I::into_demux_socket_id(id.clone()),
                                        ListenerIpAddr { addr: None, identifier: *port },
                                        old_device.clone(),
                                        weak_device.as_ref(),
                                        demux,
                                    )?;
                                    match Self::set_device_listener(
                                        &other_demux_id,
                                        ListenerIpAddr { addr: None, identifier: *port },
                                        old_device.clone(),
                                        weak_device.as_ref(),
                                        other_demux,
                                    ) {
                                        Ok(()) => {
                                            *device = weak_device;
                                            Ok(())
                                        }
                                        Err(e) => {
                                            Self::set_device_listener(
                                                &I::into_demux_socket_id(id.clone()),
                                                ListenerIpAddr { addr: None, identifier: *port },
                                                weak_device.clone(),
                                                old_device.as_ref(),
                                                demux,
                                            )
                                            .expect("failed to revert back the device setting");
                                            Err(e)
                                        }
                                    }
                                })
                            }
                        }
                    }
                },
            }
        })
    }

    /// Get information for a TCP socket.
    pub fn get_info(
        &mut self,
        id: &TcpApiSocketId<I, C>,
    ) -> SocketInfo<I::Addr, <C::CoreContext as DeviceIdContext<AnyDevice>>::WeakDeviceId> {
        self.core_ctx().with_socket_and_converter(
            id,
            |TcpSocketState { socket_state, sharing: _, ip_options: _, socket_options: _ },
             _converter| {
                match socket_state {
                    TcpSocketStateInner::Unbound(unbound) => SocketInfo::Unbound(unbound.into()),
                    TcpSocketStateInner::Connected { conn, timer: _ } => {
                        SocketInfo::Connection(I::get_conn_info(conn))
                    }
                    TcpSocketStateInner::Bound(BoundState { addr, .. })
                    | TcpSocketStateInner::Listener(Listener { addr, .. }) => {
                        SocketInfo::Bound(I::get_bound_info(addr))
                    }
                }
            },
        )
    }

    /// Call this function whenever a socket can push out more data. That means
    /// either:
    ///
    /// - A retransmission timer fires.
    /// - An ack received from peer so that our send window is enlarged.
    /// - The user puts data into the buffer and we are notified.
    pub fn do_send(&mut self, conn_id: &TcpApiSocketId<I, C>) {
        let (core_ctx, bindings_ctx) = self.contexts();
        core_ctx.with_socket_mut_transport_demux(conn_id, |core_ctx, socket_state| {
            let TcpSocketState { socket_state, sharing: _, ip_options: _, socket_options } =
                socket_state;
            let (conn, timer) = assert_matches!(
                socket_state,
                TcpSocketStateInner::Connected { conn, timer } => (conn, timer)
            );
            match core_ctx {
                MaybeDualStack::NotDualStack((core_ctx, converter)) => {
                    let (conn, addr) = converter.convert(conn);
                    do_send_inner_and_then_handle_newly_closed(
                        conn_id,
                        &I::into_demux_socket_id(conn_id.clone()),
                        socket_options,
                        conn,
                        DoSendLimit::MultipleSegments,
                        addr,
                        timer,
                        core_ctx,
                        bindings_ctx,
                    );
                }
                MaybeDualStack::DualStack((core_ctx, converter)) => match converter.convert(conn) {
                    EitherStack::ThisStack((conn, addr)) => {
                        do_send_inner_and_then_handle_newly_closed(
                            conn_id,
                            &I::into_demux_socket_id(conn_id.clone()),
                            socket_options,
                            conn,
                            DoSendLimit::MultipleSegments,
                            addr,
                            timer,
                            core_ctx,
                            bindings_ctx,
                        )
                    }
                    EitherStack::OtherStack((conn, addr)) => {
                        let other_demux_id = core_ctx.into_other_demux_socket_id(conn_id.clone());
                        do_send_inner_and_then_handle_newly_closed(
                            conn_id,
                            &other_demux_id,
                            socket_options,
                            conn,
                            DoSendLimit::MultipleSegments,
                            addr,
                            timer,
                            core_ctx,
                            bindings_ctx,
                        );
                    }
                },
            };
        })
    }

    fn handle_timer(
        &mut self,
        weak_id: WeakTcpSocketId<
            I,
            <C::CoreContext as DeviceIdContext<AnyDevice>>::WeakDeviceId,
            C::BindingsContext,
        >,
    ) {
        let id = match weak_id.upgrade() {
            Some(c) => c,
            None => return,
        };
        let (core_ctx, bindings_ctx) = self.contexts();
        debug!("handle_timer on {id:?}");
        // Alias refs so we can move weak_id to the closure.
        let id_alias = &id;
        let bindings_ctx_alias = &mut *bindings_ctx;
        let closed_and_defunct =
            core_ctx.with_socket_mut_transport_demux(&id, move |core_ctx, socket_state| {
                let TcpSocketState { socket_state, sharing: _, ip_options: _, socket_options } =
                    socket_state;
                let id = id_alias;
                trace_duration!("tcp::handle_timer", "id" => id.trace_id());
                let bindings_ctx = bindings_ctx_alias;
                let (conn, timer) = assert_matches!(
                    socket_state,
                    TcpSocketStateInner::Connected{ conn, timer} => (conn, timer)
                );
                fn do_handle_timer<SockI, WireI, CC, BC>(
                    core_ctx: &mut CC,
                    bindings_ctx: &mut BC,
                    id: &TcpSocketId<SockI, CC::WeakDeviceId, BC>,
                    demux_id: &WireI::DemuxSocketId<CC::WeakDeviceId, BC>,
                    socket_options: &SocketOptions,
                    conn: &mut Connection<SockI, WireI, CC::WeakDeviceId, BC>,
                    addr: &ConnAddr<
                        ConnIpAddr<<WireI as Ip>::Addr, NonZeroU16, NonZeroU16>,
                        CC::WeakDeviceId,
                    >,
                    timer: &mut BC::Timer,
                ) -> bool
                where
                    SockI: DualStackIpExt,
                    WireI: DualStackIpExt,
                    BC: TcpBindingsContext<CC::DeviceId>,
                    CC: TransportIpContext<WireI, BC>
                        + TcpDemuxContext<WireI, CC::WeakDeviceId, BC>
                        + TcpSocketContext<SockI, CC::WeakDeviceId, BC>,
                {
                    let time_wait = matches!(conn.state, State::TimeWait(_));
                    let newly_closed = do_send_inner(
                        id,
                        socket_options,
                        conn,
                        DoSendLimit::MultipleSegments,
                        addr,
                        timer,
                        core_ctx,
                        bindings_ctx,
                    );
                    match (newly_closed, time_wait) {
                        // Moved to closed state, remove from demux and cancel
                        // timers.
                        (NewlyClosed::Yes, time_wait) => {
                            let result = core_ctx.with_demux_mut(|DemuxState { socketmap }| {
                                socketmap.conns_mut().remove(demux_id, addr)
                            });
                            // Carve out an exception for time wait demux
                            // removal, since it could've been removed from the
                            // demux already as part of reuse.
                            //
                            // We can log rather silently because the demux will
                            // not allow us to remove the wrong connection, the
                            // panic is here to catch paths that are doing
                            // cleanup in the wrong way.
                            result.unwrap_or_else(|e| {
                                if time_wait {
                                    debug!(
                                        "raced with timewait removal for {id:?} {addr:?}: {e:?}"
                                    );
                                } else {
                                    panic!("failed to remove from socketmap: {e:?}");
                                }
                            });
                            let _: Option<_> = bindings_ctx.cancel_timer(timer);

                            let Closed { reason } = assert_matches!(
                                &conn.state, State::Closed(c) => c
                            );
                            let _: bool = conn.handshake_status.update_if_pending(match reason {
                                None => HandshakeStatus::Completed {
                                    reported: conn.accept_queue.is_some(),
                                },
                                Some(_err) => HandshakeStatus::Aborted,
                            });
                        }
                        (NewlyClosed::No, _) => {}
                    }
                    conn.defunct && matches!(conn.state, State::Closed(_))
                }
                match core_ctx {
                    MaybeDualStack::NotDualStack((core_ctx, converter)) => {
                        let (conn, addr) = converter.convert(conn);
                        do_handle_timer(
                            core_ctx,
                            bindings_ctx,
                            id,
                            &I::into_demux_socket_id(id.clone()),
                            socket_options,
                            conn,
                            addr,
                            timer,
                        )
                    }
                    MaybeDualStack::DualStack((core_ctx, converter)) => {
                        match converter.convert(conn) {
                            EitherStack::ThisStack((conn, addr)) => do_handle_timer(
                                core_ctx,
                                bindings_ctx,
                                id,
                                &I::into_demux_socket_id(id.clone()),
                                socket_options,
                                conn,
                                addr,
                                timer,
                            ),
                            EitherStack::OtherStack((conn, addr)) => do_handle_timer(
                                core_ctx,
                                bindings_ctx,
                                id,
                                &core_ctx.into_other_demux_socket_id(id.clone()),
                                socket_options,
                                conn,
                                addr,
                                timer,
                            ),
                        }
                    }
                }
            });
        if closed_and_defunct {
            // Remove the entry from the primary map and drop primary.
            destroy_socket(core_ctx, bindings_ctx, id);
        }
    }

    /// Access options mutably for a TCP socket.
    pub fn with_socket_options_mut<R, F: FnOnce(&mut SocketOptions) -> R>(
        &mut self,
        id: &TcpApiSocketId<I, C>,
        f: F,
    ) -> R {
        let (core_ctx, _) = self.contexts();
        core_ctx.with_socket_mut(id, |socket| f(&mut socket.socket_options))
    }

    /// Access socket options immutably for a TCP socket
    pub fn with_socket_options<R, F: FnOnce(&SocketOptions) -> R>(
        &mut self,
        id: &TcpApiSocketId<I, C>,
        f: F,
    ) -> R {
        self.core_ctx().with_socket(id, |socket| f(&socket.socket_options))
    }

    /// Set the size of the send buffer for this socket and future derived
    /// sockets.
    pub fn set_send_buffer_size(&mut self, id: &TcpApiSocketId<I, C>, size: usize) {
        let (core_ctx, bindings_ctx) = self.contexts();
        set_buffer_size::<SendBufferSize, I, _, _>(core_ctx, bindings_ctx, id, size)
    }

    /// Get the size of the send buffer for this socket and future derived
    /// sockets.
    pub fn send_buffer_size(&mut self, id: &TcpApiSocketId<I, C>) -> Option<usize> {
        get_buffer_size::<SendBufferSize, I, _, _>(self.core_ctx(), id)
    }

    /// Set the size of the send buffer for this socket and future derived
    /// sockets.
    pub fn set_receive_buffer_size(&mut self, id: &TcpApiSocketId<I, C>, size: usize) {
        let (core_ctx, bindings_ctx) = self.contexts();
        set_buffer_size::<ReceiveBufferSize, I, _, _>(core_ctx, bindings_ctx, id, size)
    }

    /// Get the size of the receive buffer for this socket and future derived
    /// sockets.
    pub fn receive_buffer_size(&mut self, id: &TcpApiSocketId<I, C>) -> Option<usize> {
        get_buffer_size::<ReceiveBufferSize, I, _, _>(self.core_ctx(), id)
    }

    /// Sets the POSIX SO_REUSEADDR socket option on a socket.
    pub fn set_reuseaddr(
        &mut self,
        id: &TcpApiSocketId<I, C>,
        reuse: bool,
    ) -> Result<(), SetReuseAddrError> {
        let new_sharing = match reuse {
            true => SharingState::ReuseAddress,
            false => SharingState::Exclusive,
        };
        self.core_ctx().with_socket_mut_transport_demux(id, |core_ctx, socket_state| {
            let old_sharing = socket_state.sharing;
            if old_sharing == new_sharing {
                return Ok(());
            }

            match &socket_state.socket_state {
                TcpSocketStateInner::Unbound(_) => (),
                TcpSocketStateInner::Bound(BoundState { addr, .. })
                | TcpSocketStateInner::Listener(Listener { addr, .. }) => {
                    let listening =
                        matches!(&socket_state.socket_state, TcpSocketStateInner::Listener(_));
                    try_update_listener_sharing::<_, C::CoreContext, _>(
                        core_ctx,
                        id,
                        addr.clone(),
                        &ListenerSharingState { sharing: old_sharing, listening },
                        ListenerSharingState { sharing: new_sharing, listening },
                    )
                    .map_err(|UpdateSharingError| SetReuseAddrError::AddrInUse)?;
                }
                TcpSocketStateInner::Connected { .. } => {
                    // TODO(https://fxbug.dev/42180094): Support setting the option
                    // for connection sockets.
                    return Err(SetReuseAddrError::NotSupported);
                }
            };

            socket_state.sharing = new_sharing;
            Ok(())
        })
    }

    /// Gets the POSIX SO_REUSEADDR socket option on a socket.
    pub fn reuseaddr(&mut self, id: &TcpApiSocketId<I, C>) -> bool {
        self.core_ctx().with_socket(id, |state| state.sharing == SharingState::ReuseAddress)
    }

    /// Gets the `dual_stack_enabled` option value.
    pub fn dual_stack_enabled(
        &mut self,
        id: &TcpSocketId<
            I,
            <C::CoreContext as DeviceIdContext<AnyDevice>>::WeakDeviceId,
            C::BindingsContext,
        >,
    ) -> Result<bool, NotDualStackCapableError> {
        self.core_ctx().with_socket_mut_transport_demux(id, |core_ctx, socket_state| match core_ctx
        {
            MaybeDualStack::NotDualStack(_) => Err(NotDualStackCapableError),
            MaybeDualStack::DualStack((core_ctx, _converter)) => {
                Ok(core_ctx.dual_stack_enabled(&socket_state.ip_options))
            }
        })
    }

    /// Sets the socket mark for the socket domain.
    pub fn set_mark(&mut self, id: &TcpApiSocketId<I, C>, domain: MarkDomain, mark: Mark) {
        self.with_socket_options_mut(id, |options| *options.ip_options.marks.get_mut(domain) = mark)
    }

    /// Gets the socket mark for the socket domain.
    pub fn get_mark(&mut self, id: &TcpApiSocketId<I, C>, domain: MarkDomain) -> Mark {
        self.with_socket_options(id, |options| *options.ip_options.marks.get(domain))
    }

    /// Sets the `dual_stack_enabled` option value.
    pub fn set_dual_stack_enabled(
        &mut self,
        id: &TcpSocketId<
            I,
            <C::CoreContext as DeviceIdContext<AnyDevice>>::WeakDeviceId,
            C::BindingsContext,
        >,
        value: bool,
    ) -> Result<(), SetDualStackEnabledError> {
        self.core_ctx().with_socket_mut_transport_demux(id, |core_ctx, socket_state| match core_ctx
        {
            MaybeDualStack::NotDualStack(_) => Err(NotDualStackCapableError.into()),
            MaybeDualStack::DualStack((core_ctx, _converter)) => match socket_state.socket_state {
                TcpSocketStateInner::Unbound(_) => {
                    Ok(core_ctx.set_dual_stack_enabled(&mut socket_state.ip_options, value))
                }
                TcpSocketStateInner::Connected { .. }
                | TcpSocketStateInner::Bound(_)
                | TcpSocketStateInner::Listener(_) => Err(SetDualStackEnabledError::SocketIsBound),
            },
        })
    }

    fn on_icmp_error_conn(
        core_ctx: &mut C::CoreContext,
        bindings_ctx: &mut C::BindingsContext,
        id: TcpSocketId<
            I,
            <C::CoreContext as DeviceIdContext<AnyDevice>>::WeakDeviceId,
            C::BindingsContext,
        >,
        seq: SeqNum,
        error: IcmpErrorCode,
    ) {
        let destroy = core_ctx.with_socket_mut_transport_demux(&id, |core_ctx, socket_state| {
            let (conn_and_addr, timer) = assert_matches!(
                &mut socket_state.socket_state,
                TcpSocketStateInner::Connected { conn, timer } => (conn, timer),
                "invalid socket ID");
            let (
                newly_closed,
                accept_queue,
                state,
                soft_error,
                handshake_status,
                this_or_other_stack,
            ) = match core_ctx {
                MaybeDualStack::NotDualStack((core_ctx, converter)) => {
                    let (conn, addr) = converter.convert(conn_and_addr);
                    let (newly_closed, should_send) = conn.on_icmp_error(core_ctx, &id, seq, error);
                    let core_ctx = core_ctx.as_this_stack();
                    let demux_id = I::into_demux_socket_id(id.clone());

                    match should_send {
                        ShouldRetransmit::No => {}
                        ShouldRetransmit::Yes => do_send_inner_and_then_handle_newly_closed(
                            &id,
                            &demux_id,
                            &socket_state.socket_options,
                            conn,
                            DoSendLimit::OneSegment,
                            addr,
                            timer,
                            core_ctx,
                            bindings_ctx,
                        ),
                    }

                    (
                        newly_closed,
                        &mut conn.accept_queue,
                        &mut conn.state,
                        &mut conn.soft_error,
                        &mut conn.handshake_status,
                        EitherStack::ThisStack((core_ctx, demux_id, addr)),
                    )
                }
                MaybeDualStack::DualStack((core_ctx, converter)) => {
                    match converter.convert(conn_and_addr) {
                        EitherStack::ThisStack((conn, addr)) => {
                            let (newly_closed, should_send) =
                                conn.on_icmp_error(core_ctx, &id, seq, error);
                            let core_ctx = core_ctx.as_this_stack();
                            let demux_id = I::into_demux_socket_id(id.clone());

                            match should_send {
                                ShouldRetransmit::No => {}
                                ShouldRetransmit::Yes => {
                                    do_send_inner_and_then_handle_newly_closed(
                                        &id,
                                        &demux_id,
                                        &socket_state.socket_options,
                                        conn,
                                        DoSendLimit::OneSegment,
                                        addr,
                                        timer,
                                        core_ctx,
                                        bindings_ctx,
                                    )
                                }
                            }

                            (
                                newly_closed,
                                &mut conn.accept_queue,
                                &mut conn.state,
                                &mut conn.soft_error,
                                &mut conn.handshake_status,
                                EitherStack::ThisStack((core_ctx, demux_id, addr)),
                            )
                        }
                        EitherStack::OtherStack((conn, addr)) => {
                            let (newly_closed, should_send) =
                                conn.on_icmp_error(core_ctx, &id, seq, error);
                            let demux_id = core_ctx.into_other_demux_socket_id(id.clone());

                            match should_send {
                                ShouldRetransmit::No => {}
                                ShouldRetransmit::Yes => {
                                    do_send_inner_and_then_handle_newly_closed(
                                        &id,
                                        &demux_id,
                                        &socket_state.socket_options,
                                        conn,
                                        DoSendLimit::OneSegment,
                                        addr,
                                        timer,
                                        core_ctx,
                                        bindings_ctx,
                                    )
                                }
                            }

                            (
                                newly_closed,
                                &mut conn.accept_queue,
                                &mut conn.state,
                                &mut conn.soft_error,
                                &mut conn.handshake_status,
                                EitherStack::OtherStack((core_ctx, demux_id, addr)),
                            )
                        }
                    }
                }
            };

            if let State::Closed(Closed { reason }) = state {
                debug!("handshake_status: {handshake_status:?}");
                let _: bool = handshake_status.update_if_pending(HandshakeStatus::Aborted);
                // Unregister the socket from the socketmap if newly closed.
                match this_or_other_stack {
                    EitherStack::ThisStack((core_ctx, demux_id, addr)) => {
                        handle_newly_closed::<I, _, _, _>(
                            core_ctx,
                            bindings_ctx,
                            newly_closed,
                            &demux_id,
                            addr,
                            timer,
                        );
                    }
                    EitherStack::OtherStack((core_ctx, demux_id, addr)) => {
                        handle_newly_closed::<I::OtherVersion, _, _, _>(
                            core_ctx,
                            bindings_ctx,
                            newly_closed,
                            &demux_id,
                            addr,
                            timer,
                        );
                    }
                };
                match accept_queue {
                    Some(accept_queue) => {
                        accept_queue.remove(&id);
                        // destroy the socket if not held by the user.
                        return true;
                    }
                    None => {
                        if let Some(err) = reason {
                            if *err == ConnectionError::TimedOut {
                                *err = soft_error.unwrap_or(ConnectionError::TimedOut);
                            }
                        }
                    }
                }
            }
            false
        });
        if destroy {
            destroy_socket(core_ctx, bindings_ctx, id);
        }
    }

    fn on_icmp_error(
        &mut self,
        orig_src_ip: SpecifiedAddr<I::Addr>,
        orig_dst_ip: SpecifiedAddr<I::Addr>,
        orig_src_port: NonZeroU16,
        orig_dst_port: NonZeroU16,
        seq: SeqNum,
        error: IcmpErrorCode,
    ) where
        C::CoreContext: TcpContext<I::OtherVersion, C::BindingsContext>,
        C::BindingsContext: TcpBindingsContext<
            <<C as ContextPair>::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId,
        >,
    {
        let (core_ctx, bindings_ctx) = self.contexts();

        let orig_src_ip = match SocketIpAddr::try_from(orig_src_ip) {
            Ok(ip) => ip,
            Err(AddrIsMappedError {}) => {
                debug!("ignoring ICMP error from IPv4-mapped-IPv6 source: {}", orig_src_ip);
                return;
            }
        };
        let orig_dst_ip = match SocketIpAddr::try_from(orig_dst_ip) {
            Ok(ip) => ip,
            Err(AddrIsMappedError {}) => {
                debug!("ignoring ICMP error to IPv4-mapped-IPv6 destination: {}", orig_dst_ip);
                return;
            }
        };

        let id = TcpDemuxContext::<I, _, _>::with_demux(core_ctx, |DemuxState { socketmap }| {
            socketmap
                .conns()
                .get_by_addr(&ConnAddr {
                    ip: ConnIpAddr {
                        local: (orig_src_ip, orig_src_port),
                        remote: (orig_dst_ip, orig_dst_port),
                    },
                    device: None,
                })
                .map(|ConnAddrState { sharing: _, id }| id.clone())
        });

        let id = match id {
            Some(id) => id,
            None => return,
        };

        match I::into_dual_stack_ip_socket(id) {
            EitherStack::ThisStack(id) => {
                Self::on_icmp_error_conn(core_ctx, bindings_ctx, id, seq, error)
            }
            EitherStack::OtherStack(id) => TcpApi::<I::OtherVersion, C>::on_icmp_error_conn(
                core_ctx,
                bindings_ctx,
                id,
                seq,
                error,
            ),
        };
    }

    /// Gets the last error on the connection.
    pub fn get_socket_error(&mut self, id: &TcpApiSocketId<I, C>) -> Option<ConnectionError> {
        self.core_ctx().with_socket_mut_and_converter(id, |socket_state, converter| {
            match &mut socket_state.socket_state {
                TcpSocketStateInner::Unbound(_)
                | TcpSocketStateInner::Bound(_)
                | TcpSocketStateInner::Listener(_) => None,
                TcpSocketStateInner::Connected { conn, timer: _ } => {
                    let reporter = match converter {
                        MaybeDualStack::NotDualStack(converter) => {
                            let (conn, _addr) = converter.convert(conn);
                            ErrorReporter::new(&mut conn.state, &mut conn.soft_error)
                        }
                        MaybeDualStack::DualStack(converter) => match converter.convert(conn) {
                            EitherStack::ThisStack((conn, _addr)) => {
                                ErrorReporter::new(&mut conn.state, &mut conn.soft_error)
                            }
                            EitherStack::OtherStack((conn, _addr)) => {
                                ErrorReporter::new(&mut conn.state, &mut conn.soft_error)
                            }
                        },
                    };
                    reporter.report_error()
                }
            }
        })
    }

    /// Gets the original destination address for the socket, if it is connected
    /// and has a destination in the specified stack.
    ///
    /// Note that this always returns the original destination in the IP stack
    /// in which the socket is; for example, for a dual-stack IPv6 socket that
    /// is connected to an IPv4 address, this will return the IPv4-mapped IPv6
    /// version of that address.
    pub fn get_original_destination(
        &mut self,
        id: &TcpApiSocketId<I, C>,
    ) -> Result<(SpecifiedAddr<I::Addr>, NonZeroU16), OriginalDestinationError> {
        self.core_ctx().with_socket_mut_transport_demux(id, |core_ctx, state| {
            let TcpSocketState { socket_state, .. } = state;
            let conn = match socket_state {
                TcpSocketStateInner::Connected { conn, .. } => conn,
                TcpSocketStateInner::Bound(_)
                | TcpSocketStateInner::Listener(_)
                | TcpSocketStateInner::Unbound(_) => {
                    return Err(OriginalDestinationError::NotConnected);
                }
            };

            fn tuple<I: IpExt>(
                ConnIpAddr { local, remote }: ConnIpAddr<I::Addr, NonZeroU16, NonZeroU16>,
            ) -> Tuple<I> {
                let (local_addr, local_port) = local;
                let (remote_addr, remote_port) = remote;
                Tuple {
                    protocol: IpProto::Tcp.into(),
                    src_addr: local_addr.addr(),
                    dst_addr: remote_addr.addr(),
                    src_port_or_id: local_port.get(),
                    dst_port_or_id: remote_port.get(),
                }
            }

            let (addr, port) = match core_ctx {
                MaybeDualStack::NotDualStack((core_ctx, converter)) => {
                    let (_conn, addr) = converter.convert(conn);
                    let tuple: Tuple<I> = tuple(addr.ip);
                    core_ctx
                        .get_original_destination(&tuple)
                        .ok_or(OriginalDestinationError::NotFound)
                }
                MaybeDualStack::DualStack((core_ctx, converter)) => match converter.convert(conn) {
                    EitherStack::ThisStack((_conn, addr)) => {
                        let tuple: Tuple<I> = tuple(addr.ip);
                        let (addr, port) = core_ctx
                            .get_original_destination(&tuple)
                            .ok_or(OriginalDestinationError::NotFound)?;
                        let addr = I::get_original_dst(
                            converter.convert_back(EitherStack::ThisStack(addr)),
                        );
                        Ok((addr, port))
                    }
                    EitherStack::OtherStack((_conn, addr)) => {
                        let tuple: Tuple<I::OtherVersion> = tuple(addr.ip);
                        let (addr, port) = core_ctx
                            .get_original_destination(&tuple)
                            .ok_or(OriginalDestinationError::NotFound)?;
                        let addr = I::get_original_dst(
                            converter.convert_back(EitherStack::OtherStack(addr)),
                        );
                        Ok((addr, port))
                    }
                },
            }?;

            // TCP connections always have a specified destination address and
            // port, but this invariant is not upheld in the type system here
            // because we are retrieving the destination from the connection
            // tracking table.
            let addr = SpecifiedAddr::new(addr).ok_or_else(|| {
                error!("original destination for socket {id:?} had unspecified addr (port {port})");
                OriginalDestinationError::UnspecifiedDestinationAddr
            })?;
            let port = NonZeroU16::new(port).ok_or_else(|| {
                error!("original destination for socket {id:?} had unspecified port (addr {addr})");
                OriginalDestinationError::UnspecifiedDestinationPort
            })?;
            Ok((addr, port))
        })
    }

    /// Get diagnostic information for sockets matching the provided matcher.
    pub fn bound_sockets_diagnostics<M, E>(
        &mut self,
        matcher: &M,
        results: &mut E,
        extended_info: bool,
    ) where
        M: IpSocketPropertiesMatcher<<C::BindingsContext as MatcherBindingsTypes>::DeviceClass>
            + ?Sized,
        E: Extend<TcpSocketDiagnostics<I, <C::BindingsContext as InstantBindingsTypes>::Instant>>,
        <C::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId:
            netstack3_base::InterfaceProperties<
                    <C::BindingsContext as MatcherBindingsTypes>::DeviceClass,
                >,
    {
        self.core_ctx().for_each_socket(|id, state| {
            if !matcher.matches_ip_socket(&TcpSocketStateForMatching { state, id }) {
                return;
            }

            // get_diagnostics returns None if the socket is unbound, which
            // we're not returning in order to match Linux's behavior.
            let counters = id.counters();
            results.extend(state.get_diagnostics(counters, extended_info).map(
                |(tuple, state_machine, marks, tcp_info)| TcpSocketDiagnostics {
                    tuple,
                    state_machine,
                    cookie: id.socket_cookie(),
                    marks,
                    tcp_info,
                },
            ));
        });
    }

    /// Disconnects all bound sockets matching the provided matcher.
    ///
    /// They are moved to state CLOSE, and an RST is sent if required. For
    /// LISTEN sockets, an RST is sent to any sockets that are in the accept
    /// queue. The tuple is *not* cleared.
    ///
    /// Returns the number of sockets that were disconnected.
    pub fn disconnect_bound<M>(&mut self, matcher: &M) -> usize
    where
        M: IpSocketPropertiesMatcher<<C::BindingsContext as MatcherBindingsTypes>::DeviceClass>
            + ?Sized,
        <C::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId:
            netstack3_base::InterfaceProperties<
                    <C::BindingsContext as MatcherBindingsTypes>::DeviceClass,
                >,
    {
        let (core_ctx, bindings_ctx) = self.contexts();

        // We filter and disconnect separately here because disconnection is not a
        // performance-sensitive operation and it's significantly easier to do
        // than to manage the lifetimes and locking. Also, the only expected
        // user at the time of writing is Starnix's SOCK_DESTROY implementation,
        // which will only call this with a single socket at a time.
        let mut ids = Vec::new();
        core_ctx.for_each_socket(|id, state| {
            if matcher.matches_ip_socket(&TcpSocketStateForMatching { state, id }) {
                ids.push(id.clone());
            }
        });

        // It's possible a socket no longer matches. However, this is a
        // small race in comparison to the ones between API calls for different
        // sockets (bind, connect, etc).
        ids.into_iter()
            .filter(|id| match disconnect_socket(core_ctx, bindings_ctx, &id) {
                Ok(()) => true,
                // We're okay with this because it's possible we raced with the
                // socket being closed.
                Err(NoConnection) => false,
            })
            .count()
    }

    /// Provides access to shared and per-socket TCP stats via a visitor.
    pub fn inspect<N>(&mut self, inspector: &mut N)
    where
        N: Inspector
            + InspectorDeviceExt<<C::CoreContext as DeviceIdContext<AnyDevice>>::WeakDeviceId>,
    {
        self.core_ctx().for_each_socket(|socket_id, socket_state| {
            inspector.record_debug_child(socket_id, |node| {
                node.record_str("TransportProtocol", "TCP");
                node.record_str(
                    "NetworkProtocol",
                    match I::VERSION {
                        IpVersion::V4 => "IPv4",
                        IpVersion::V6 => "IPv6",
                    },
                );
                let info = socket_state.tcp_info(socket_id.counters());
                let TcpSocketState { socket_state, sharing: _, ip_options: _, socket_options } =
                    socket_state;
                node.delegate_inspectable(&socket_options.ip_options.marks);
                match socket_state {
                    TcpSocketStateInner::Unbound(_) => {
                        node.record_local_socket_addr::<N, I::Addr, _, NonZeroU16>(None);
                        node.record_remote_socket_addr::<N, I::Addr, _, NonZeroU16>(None);
                    }
                    TcpSocketStateInner::Bound(BoundState { addr, .. }) => {
                        let BoundInfo { addr, port, device } = I::get_bound_info(addr);
                        let local = addr.map_or_else(
                            || ZonedAddr::Unzoned(I::UNSPECIFIED_ADDRESS),
                            |addr| maybe_zoned(addr.addr(), &device).into(),
                        );
                        node.record_local_socket_addr::<N, _, _, _>(Some((local, port)));
                        node.record_remote_socket_addr::<N, I::Addr, _, NonZeroU16>(None);
                    }
                    TcpSocketStateInner::Listener(Listener {
                        addr, accept_queue, backlog, ..
                    }) => {
                        let BoundInfo { addr, port, device } = I::get_bound_info(addr);
                        let local = addr.map_or_else(
                            || ZonedAddr::Unzoned(I::UNSPECIFIED_ADDRESS),
                            |addr| maybe_zoned(addr.addr(), &device).into(),
                        );
                        node.record_local_socket_addr::<N, _, _, _>(Some((local, port)));
                        node.record_remote_socket_addr::<N, I::Addr, _, NonZeroU16>(None);
                        node.record_child("AcceptQueue", |node| {
                            node.record_usize("BacklogSize", *backlog);
                            accept_queue.inspect(node);
                        });
                    }
                    TcpSocketStateInner::Connected { conn, .. } => {
                        if I::get_defunct(conn) {
                            return;
                        }
                        let state = I::get_state(conn);
                        let ConnectionInfo {
                            local_addr: SocketAddr { ip: local_ip, port: local_port },
                            remote_addr: SocketAddr { ip: remote_ip, port: remote_port },
                            device: _,
                        } = I::get_conn_info(conn);
                        node.record_local_socket_addr::<N, I::Addr, _, _>(Some((
                            local_ip.into(),
                            local_port,
                        )));
                        node.record_remote_socket_addr::<N, I::Addr, _, _>(Some((
                            remote_ip.into(),
                            remote_port,
                        )));
                        node.record_display("State", state);
                    }
                }
                node.record_child("TcpInfo", |node| {
                    node.delegate_inspectable(&info);
                });
                node.record_child("Counters", |node| {
                    node.delegate_inspectable(&CombinedTcpCounters {
                        with_socket: socket_id.counters(),
                        without_socket: None,
                    })
                })
            });
        })
    }

    /// Calls the callback with mutable access to the send buffer, if one is
    /// instantiated.
    ///
    /// If no buffer is instantiated returns `None`.
    pub fn with_send_buffer<
        R,
        F: FnOnce(&mut <C::BindingsContext as TcpBindingsTypes>::SendBuffer) -> R,
    >(
        &mut self,
        id: &TcpApiSocketId<I, C>,
        f: F,
    ) -> Option<R> {
        self.core_ctx().with_socket_mut_and_converter(id, |state, converter| {
            get_buffers_mut::<_, C::CoreContext, _>(state, converter).into_send_buffer().map(f)
        })
    }

    /// Calls the callback with mutable access to the receive buffer, if one is
    /// instantiated.
    ///
    /// If no buffer is instantiated returns `None`.
    pub fn with_receive_buffer<
        R,
        F: FnOnce(&mut <C::BindingsContext as TcpBindingsTypes>::ReceiveBuffer) -> R,
    >(
        &mut self,
        id: &TcpApiSocketId<I, C>,
        f: F,
    ) -> Option<R> {
        self.core_ctx().with_socket_mut_and_converter(id, |state, converter| {
            get_buffers_mut::<_, C::CoreContext, _>(state, converter).into_receive_buffer().map(f)
        })
    }
}

/// Destroys the socket with `id`.
fn destroy_socket<I, CC, BC>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    id: TcpSocketId<I, CC::WeakDeviceId, BC>,
) where
    I: DualStackIpExt,
    CC: TcpContext<I, BC>,
    BC: TcpBindingsContext<CC::DeviceId>,
{
    core_ctx.with_all_sockets_mut(move |all_sockets| {
        let cookie = id.socket_cookie();
        let TcpSocketId(rc) = &id;
        let debug_refs = StrongRc::debug_references(rc);
        let entry = all_sockets.entry(id);
        let primary = match entry {
            hash_map::Entry::Occupied(o) => match o.get() {
                TcpSocketSetEntry::DeadOnArrival => {
                    let id = o.key();
                    debug!("{id:?} destruction skipped, socket is DOA. References={debug_refs:?}",);
                    None
                }
                TcpSocketSetEntry::Primary(_) => {
                    assert_matches!(o.remove_entry(), (_, TcpSocketSetEntry::Primary(p)) => Some(p))
                }
            },
            hash_map::Entry::Vacant(v) => {
                let id = v.key();
                let TcpSocketId(rc) = id;
                if !StrongRc::marked_for_destruction(rc) {
                    // Socket is not yet marked for destruction, we've raced
                    // this removal with the addition to the socket set. Mark
                    // the entry as DOA.
                    debug!(
                        "{id:?} raced with insertion, marking socket as DOA. \
                        References={debug_refs:?}",
                    );
                    let _: &mut _ = v.insert(TcpSocketSetEntry::DeadOnArrival);
                } else {
                    debug!("{id:?} destruction is already deferred. References={debug_refs:?}");
                }
                None
            }
        };

        let Some(primary) = primary else {
            // There are a number of races that can happen with attempted socket
            // destruction, but these should not be possible in tests because
            // they're single-threaded.
            cfg_if::cfg_if! {
                if #[cfg(test)] {
                    panic!("deferred destruction not allowed in tests. \
                            References={debug_refs:?}");
                } else {
                    return;
                }
            }
        };

        // `primary` must be dropped while holding the sockets lock. Otherwise a
        // race between two `destroy_socket` calls may result in
        // `TcpSocketSetEntry::DeadOnArrival` being left in the socket set.
        let remove_result =
            BC::unwrap_or_notify_with_new_reference_notifier(primary, move |state| {
                TcpSocketDiagnosticsSeed {
                    state: state.locked_state.into_inner(),
                    counters: state.counters,
                    cookie,
                }
            });

        bindings_ctx.defer_tcp_socket_destruction(remove_result);
    });
}

// Shuts down the listener socket and returns the pending connections and the
// new bound state for the socket. Pending connections should be closed by
// passed `close_pending_sockets`.
fn shut_down_listener_socket<I, CC, BC>(
    core_ctx: MaybeDualStack<
        (&mut CC::DualStackIpTransportAndDemuxCtx<'_>, CC::DualStackConverter),
        (&mut CC::SingleStackIpTransportAndDemuxCtx<'_>, CC::SingleStackConverter),
    >,
    id: &TcpSocketId<I, CC::WeakDeviceId, BC>,
    listener: &Listener<I, CC::WeakDeviceId, BC>,
    sharing: SharingState,
) -> (
    impl Iterator<Item = TcpSocketId<I, CC::WeakDeviceId, BC>> + use<I, CC, BC>,
    BoundState<I, CC::WeakDeviceId, BC>,
)
where
    I: DualStackIpExt,
    BC: TcpBindingsContext<CC::DeviceId>,
    CC: TcpContext<I, BC>,
{
    let Listener { addr, backlog: _, accept_queue, buffer_sizes } = listener;
    let (pending, socket_extra) = accept_queue.close();

    try_update_listener_sharing::<_, CC, _>(
        core_ctx,
        id,
        addr.clone(),
        &ListenerSharingState { listening: true, sharing },
        ListenerSharingState { listening: false, sharing },
    )
    .unwrap_or_else(|e| {
        unreachable!("downgrading a TCP listener to bound should not fail, got {e:?}")
    });

    let bound = BoundState {
        addr: addr.clone(),
        buffer_sizes: buffer_sizes.clone(),
        socket_extra: Takeable::new(socket_extra),
    };

    (pending, bound)
}

fn disconnect_socket<I, CC, BC>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    id: &TcpSocketId<I, CC::WeakDeviceId, BC>,
) -> Result<(), NoConnection>
where
    I: DualStackIpExt,
    BC: TcpBindingsContext<CC::DeviceId>,
    CC: TcpContext<I, BC>,
{
    debug!("disconnect for {id:?}");
    let pending = core_ctx.with_socket_mut_transport_demux(id, |core_ctx, socket_state| {
        let TcpSocketState { socket_state, sharing, ip_options: _, socket_options } = socket_state;
        match socket_state {
            TcpSocketStateInner::Unbound(_) => Err(NoConnection),
            TcpSocketStateInner::Connected { conn, timer } => {
                match core_ctx {
                    MaybeDualStack::NotDualStack((core_ctx, converter)) => {
                        let (conn, addr) = converter.convert(conn);
                        abort_socket(
                            core_ctx,
                            bindings_ctx,
                            id,
                            &I::into_demux_socket_id(id.clone()),
                            socket_options,
                            timer,
                            conn,
                            addr,
                            ConnectionError::Aborted,
                        )
                    }
                    MaybeDualStack::DualStack((core_ctx, converter)) => {
                        match converter.convert(conn) {
                            EitherStack::ThisStack((conn, addr)) => abort_socket(
                                core_ctx,
                                bindings_ctx,
                                id,
                                &I::into_demux_socket_id(id.clone()),
                                socket_options,
                                timer,
                                conn,
                                addr,
                                ConnectionError::Aborted,
                            ),
                            EitherStack::OtherStack((conn, addr)) => abort_socket(
                                core_ctx,
                                bindings_ctx,
                                id,
                                &core_ctx.into_other_demux_socket_id(id.clone()),
                                socket_options,
                                timer,
                                conn,
                                addr,
                                ConnectionError::Aborted,
                            ),
                        }
                    }
                };
                Ok(None)
            }
            TcpSocketStateInner::Bound(_) => Ok(None),
            TcpSocketStateInner::Listener(listener) => {
                let (pending, bound) =
                    shut_down_listener_socket::<I, CC, BC>(core_ctx, id, listener, *sharing);
                *socket_state = TcpSocketStateInner::Bound(bound);
                Ok(Some(pending))
            }
        }
    })?;

    close_pending_sockets(core_ctx, bindings_ctx, pending.into_iter().flatten());

    Ok(())
}

/// Closes all sockets in `pending`.
///
/// Used to cleanup all pending sockets in the accept queue when a listener
/// socket is shutdown or closed.
fn close_pending_sockets<I, CC, BC>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    pending: impl Iterator<Item = TcpSocketId<I, CC::WeakDeviceId, BC>>,
) where
    I: DualStackIpExt,
    BC: TcpBindingsContext<CC::DeviceId>,
    CC: TcpContext<I, BC>,
{
    for conn_id in pending {
        core_ctx.with_socket_mut_transport_demux(&conn_id, |core_ctx, socket_state| {
            let TcpSocketState { socket_state, sharing: _, ip_options: _, socket_options } =
                socket_state;
            let (conn_and_addr, timer) = assert_matches!(
                socket_state,
                TcpSocketStateInner::Connected{ conn, timer } => (conn, timer),
                "invalid socket ID"
            );
            let _: Option<BC::Instant> = bindings_ctx.cancel_timer(timer);
            let this_or_other_stack = match core_ctx {
                MaybeDualStack::NotDualStack((core_ctx, converter)) => {
                    let (conn, addr) = converter.convert(conn_and_addr);
                    EitherStack::ThisStack((
                        core_ctx.as_this_stack(),
                        I::into_demux_socket_id(conn_id.clone()),
                        conn,
                        addr.clone(),
                    ))
                }
                MaybeDualStack::DualStack((core_ctx, converter)) => match converter
                    .convert(conn_and_addr)
                {
                    EitherStack::ThisStack((conn, addr)) => EitherStack::ThisStack((
                        core_ctx.as_this_stack(),
                        I::into_demux_socket_id(conn_id.clone()),
                        conn,
                        addr.clone(),
                    )),
                    EitherStack::OtherStack((conn, addr)) => {
                        let other_demux_id = core_ctx.into_other_demux_socket_id(conn_id.clone());
                        EitherStack::OtherStack((core_ctx, other_demux_id, conn, addr.clone()))
                    }
                },
            };

            match this_or_other_stack {
                EitherStack::ThisStack((core_ctx, demux_id, conn, conn_addr)) => abort_socket(
                    core_ctx,
                    bindings_ctx,
                    &conn_id,
                    &demux_id,
                    socket_options,
                    timer,
                    conn,
                    &conn_addr,
                    ConnectionError::ConnectionReset,
                ),
                EitherStack::OtherStack((core_ctx, demux_id, conn, conn_addr)) => abort_socket(
                    core_ctx,
                    bindings_ctx,
                    &conn_id,
                    &demux_id,
                    socket_options,
                    timer,
                    conn,
                    &conn_addr,
                    ConnectionError::ConnectionReset,
                ),
            }
        });
        destroy_socket(core_ctx, bindings_ctx, conn_id);
    }
}

fn abort_socket<WireI, SockI, DC, BC>(
    core_ctx: &mut DC,
    bindings_ctx: &mut BC,
    sock_id: &TcpSocketId<SockI, DC::WeakDeviceId, BC>,
    demux_id: &WireI::DemuxSocketId<DC::WeakDeviceId, BC>,
    socket_options: &SocketOptions,
    timer: &mut BC::Timer,
    conn: &mut Connection<SockI, WireI, DC::WeakDeviceId, BC>,
    conn_addr: &ConnAddr<ConnIpAddr<WireI::Addr, NonZeroU16, NonZeroU16>, DC::WeakDeviceId>,
    reason: ConnectionError,
) where
    WireI: DualStackIpExt,
    SockI: DualStackIpExt,
    DC: TransportIpContext<WireI, BC>
        + DeviceIpSocketHandler<WireI, BC>
        + TcpDemuxContext<WireI, DC::WeakDeviceId, BC>
        + TcpSocketContext<SockI, DC::WeakDeviceId, BC>,
    BC: TcpBindingsContext<DC::DeviceId>,
{
    debug!("aborting socket {sock_id:?} with reason {reason}");
    let (maybe_reset, newly_closed) =
        conn.state.abort(&TcpCountersRefs::from_ctx(core_ctx, sock_id), bindings_ctx.now(), reason);
    handle_newly_closed(core_ctx, bindings_ctx, newly_closed, demux_id, conn_addr, timer);
    if let Some(reset) = maybe_reset {
        let ConnAddr { ip, device: _ } = conn_addr;
        send_tcp_segment(
            core_ctx,
            bindings_ctx,
            Some(sock_id),
            Some(&conn.ip_sock),
            *ip,
            reset.into_empty(),
            &socket_options.ip_options,
        );
    }
}

// How many segments to send as part of the "do_send" routine.
pub(crate) enum DoSendLimit {
    OneSegment,
    MultipleSegments,
}

// Calls `do_send_inner` and handle the result.
fn do_send_inner_and_then_handle_newly_closed<SockI, WireI, CC, BC>(
    conn_id: &TcpSocketId<SockI, CC::WeakDeviceId, BC>,
    demux_id: &WireI::DemuxSocketId<CC::WeakDeviceId, BC>,
    socket_options: &SocketOptions,
    conn: &mut Connection<SockI, WireI, CC::WeakDeviceId, BC>,
    limit: DoSendLimit,
    addr: &ConnAddr<ConnIpAddr<WireI::Addr, NonZeroU16, NonZeroU16>, CC::WeakDeviceId>,
    timer: &mut BC::Timer,
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
) where
    SockI: DualStackIpExt,
    WireI: DualStackIpExt,
    BC: TcpBindingsContext<CC::DeviceId>,
    CC: TransportIpContext<WireI, BC>
        + TcpSocketContext<SockI, CC::WeakDeviceId, BC>
        + TcpDemuxContext<WireI, CC::WeakDeviceId, BC>,
{
    let newly_closed =
        do_send_inner(conn_id, socket_options, conn, limit, addr, timer, core_ctx, bindings_ctx);
    handle_newly_closed(core_ctx, bindings_ctx, newly_closed, demux_id, addr, timer);
}

#[inline]
fn handle_newly_closed<I, D, CC, BC>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    newly_closed: NewlyClosed,
    demux_id: &I::DemuxSocketId<D, BC>,
    addr: &ConnAddr<ConnIpAddr<I::Addr, NonZeroU16, NonZeroU16>, D>,
    timer: &mut BC::Timer,
) where
    I: DualStackIpExt,
    D: WeakDeviceIdentifier,
    CC: TcpDemuxContext<I, D, BC>,
    BC: TcpBindingsContext<D::Strong>,
{
    if newly_closed == NewlyClosed::Yes {
        core_ctx.with_demux_mut(|DemuxState { socketmap }| {
            socketmap.conns_mut().remove(demux_id, addr).expect("failed to remove from demux");
            let _: Option<_> = bindings_ctx.cancel_timer(timer);
        });
    }
}

fn do_send_inner<SockI, WireI, CC, BC>(
    conn_id: &TcpSocketId<SockI, CC::WeakDeviceId, BC>,
    socket_options: &SocketOptions,
    conn: &mut Connection<SockI, WireI, CC::WeakDeviceId, BC>,
    limit: DoSendLimit,
    addr: &ConnAddr<ConnIpAddr<WireI::Addr, NonZeroU16, NonZeroU16>, CC::WeakDeviceId>,
    timer: &mut BC::Timer,
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
) -> NewlyClosed
where
    SockI: DualStackIpExt,
    WireI: DualStackIpExt,
    BC: TcpBindingsContext<CC::DeviceId>,
    CC: TransportIpContext<WireI, BC> + TcpSocketContext<SockI, CC::WeakDeviceId, BC>,
{
    let newly_closed = loop {
        match conn.state.poll_send(
            &conn_id.either(),
            &TcpCountersRefs::from_ctx(core_ctx, conn_id),
            bindings_ctx.now(),
            socket_options,
        ) {
            Ok(seg) => {
                send_tcp_segment(
                    core_ctx,
                    bindings_ctx,
                    Some(conn_id),
                    Some(&conn.ip_sock),
                    addr.ip.clone(),
                    seg,
                    &socket_options.ip_options,
                );
                match limit {
                    DoSendLimit::OneSegment => break NewlyClosed::No,
                    DoSendLimit::MultipleSegments => {}
                }
            }
            Err(newly_closed) => break newly_closed,
        }
    };

    if let Some(instant) = conn.state.poll_send_at() {
        let _: Option<_> = bindings_ctx.schedule_timer_instant(instant, timer);
    }

    newly_closed
}

enum SendBufferSize {}
enum ReceiveBufferSize {}

trait AccessBufferSize<R, S> {
    fn set_buffer_size(buffers: BuffersRefMut<'_, R, S>, new_size: usize);
    fn get_buffer_size(buffers: BuffersRefMut<'_, R, S>) -> Option<usize>;
    fn allowed_range(settings: &TcpSettings) -> (usize, usize);
}

impl<R: Buffer, S: Buffer> AccessBufferSize<R, S> for SendBufferSize {
    fn set_buffer_size(buffers: BuffersRefMut<'_, R, S>, new_size: usize) {
        match buffers {
            BuffersRefMut::NoBuffers | BuffersRefMut::RecvOnly { .. } => {}
            BuffersRefMut::Both { send, recv: _ } | BuffersRefMut::SendOnly(send) => {
                send.request_capacity(new_size)
            }
            BuffersRefMut::Sizes(BufferSizes { send, receive: _ }) => *send = new_size,
        }
    }

    fn allowed_range(settings: &TcpSettings) -> (usize, usize) {
        (settings.send_buffer.min().get(), settings.send_buffer.max().get())
    }

    fn get_buffer_size(buffers: BuffersRefMut<'_, R, S>) -> Option<usize> {
        match buffers {
            BuffersRefMut::NoBuffers | BuffersRefMut::RecvOnly { .. } => None,
            BuffersRefMut::Both { send, recv: _ } | BuffersRefMut::SendOnly(send) => {
                Some(send.target_capacity())
            }
            BuffersRefMut::Sizes(BufferSizes { send, receive: _ }) => Some(*send),
        }
    }
}

impl<R: Buffer, S: Buffer> AccessBufferSize<R, S> for ReceiveBufferSize {
    fn set_buffer_size(buffers: BuffersRefMut<'_, R, S>, new_size: usize) {
        match buffers {
            BuffersRefMut::NoBuffers | BuffersRefMut::SendOnly(_) => {}
            BuffersRefMut::Both { recv, send: _ } | BuffersRefMut::RecvOnly(recv) => {
                recv.request_capacity(new_size)
            }
            BuffersRefMut::Sizes(BufferSizes { receive, send: _ }) => *receive = new_size,
        }
    }

    fn allowed_range(settings: &TcpSettings) -> (usize, usize) {
        (settings.receive_buffer.min().get(), settings.receive_buffer.max().get())
    }

    fn get_buffer_size(buffers: BuffersRefMut<'_, R, S>) -> Option<usize> {
        match buffers {
            BuffersRefMut::NoBuffers | BuffersRefMut::SendOnly(_) => None,
            BuffersRefMut::Both { recv, send: _ } | BuffersRefMut::RecvOnly(recv) => {
                Some(recv.target_capacity())
            }
            BuffersRefMut::Sizes(BufferSizes { receive, send: _ }) => Some(*receive),
        }
    }
}

fn get_buffers_mut<I, CC, BC>(
    state: &mut TcpSocketState<I, CC::WeakDeviceId, BC>,
    converter: MaybeDualStack<CC::DualStackConverter, CC::SingleStackConverter>,
) -> BuffersRefMut<'_, BC::ReceiveBuffer, BC::SendBuffer>
where
    I: DualStackIpExt,
    CC: TcpContext<I, BC>,
    BC: TcpBindingsContext<CC::DeviceId>,
{
    match &mut state.socket_state {
        TcpSocketStateInner::Unbound(Unbound { buffer_sizes, .. })
        | TcpSocketStateInner::Bound(BoundState { buffer_sizes, .. })
        | TcpSocketStateInner::Listener(Listener { buffer_sizes, .. }) => {
            BuffersRefMut::Sizes(buffer_sizes)
        }
        TcpSocketStateInner::Connected { conn, .. } => {
            let state = match converter {
                MaybeDualStack::NotDualStack(converter) => {
                    let (conn, _addr) = converter.convert(conn);
                    &mut conn.state
                }
                MaybeDualStack::DualStack(converter) => match converter.convert(conn) {
                    EitherStack::ThisStack((conn, _addr)) => &mut conn.state,
                    EitherStack::OtherStack((conn, _addr)) => &mut conn.state,
                },
            };
            state.buffers_mut()
        }
    }
}

fn set_buffer_size<
    Which: AccessBufferSize<BC::ReceiveBuffer, BC::SendBuffer>,
    I: DualStackIpExt,
    BC: TcpBindingsContext<CC::DeviceId>,
    CC: TcpContext<I, BC>,
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    id: &TcpSocketId<I, CC::WeakDeviceId, BC>,
    size: usize,
) {
    let (min, max) = Which::allowed_range(&*bindings_ctx.settings());
    let size = size.clamp(min, max);
    core_ctx.with_socket_mut_and_converter(id, |state, converter| {
        Which::set_buffer_size(get_buffers_mut::<I, CC, BC>(state, converter), size)
    })
}

fn get_buffer_size<
    Which: AccessBufferSize<BC::ReceiveBuffer, BC::SendBuffer>,
    I: DualStackIpExt,
    BC: TcpBindingsContext<CC::DeviceId>,
    CC: TcpContext<I, BC>,
>(
    core_ctx: &mut CC,
    id: &TcpSocketId<I, CC::WeakDeviceId, BC>,
) -> Option<usize> {
    core_ctx.with_socket_mut_and_converter(id, |state, converter| {
        Which::get_buffer_size(get_buffers_mut::<I, CC, BC>(state, converter))
    })
}

/// Error returned when failing to set the bound device for a socket.
#[derive(Debug, GenericOverIp, Error)]
#[generic_over_ip()]
pub enum SetDeviceError {
    /// The socket would conflict with another socket.
    #[error("cannot set bound device due to conflict with another socket")]
    Conflict,
    /// The socket would become unroutable.
    #[error("cannot set bound device as socket would become unroutable")]
    Unroutable,
    /// The socket has an address with a different zone.
    #[error("cannot set bound device as socket's address has a different zone")]
    ZoneChange,
}

/// Possible errors for accept operation.
#[derive(Debug, GenericOverIp, Error)]
#[generic_over_ip()]
pub enum AcceptError {
    /// There is no established socket currently.
    #[error("would block: no currently-established socket")]
    WouldBlock,
    /// Cannot accept on this socket.
    #[error("this socket does not support accept")]
    NotSupported,
}

/// Errors for the listen operation.
#[derive(Debug, GenericOverIp, PartialEq, Error)]
#[generic_over_ip()]
pub enum ListenError {
    /// There would be a conflict with another listening socket.
    #[error("conflict with another listening socket")]
    ListenerExists,
    /// Cannot listen on such socket.
    #[error("listening not supported")]
    NotSupported,
}

/// Possible error for calling `shutdown` on a not-yet connected socket.
#[derive(Debug, GenericOverIp, Eq, PartialEq, Error)]
#[generic_over_ip()]
#[error("no connection")]
pub struct NoConnection;

/// Error returned when attempting to set the ReuseAddress option.
#[derive(Debug, GenericOverIp, Error)]
#[generic_over_ip()]
pub enum SetReuseAddrError {
    /// Cannot share the address because it is already used.
    #[error("cannot share in-use address")]
    AddrInUse,
    /// Cannot set ReuseAddr on a connected socket.
    #[error("cannot set ReuseAddr on a connected socket")]
    NotSupported,
}

/// Possible errors when connecting a socket.
#[derive(Debug, Error, GenericOverIp)]
#[generic_over_ip()]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub enum ConnectError {
    /// Cannot allocate a local port for the connection.
    #[error("unable to allocate a port")]
    NoPort,
    /// Cannot find a route to the remote host.
    #[error("no route to remote host")]
    NoRoute,
    /// There was a problem with the provided address relating to its zone.
    #[error(transparent)]
    Zone(#[from] ZonedAddressError),
    /// There is an existing connection with the same 4-tuple.
    #[error("there is already a connection at the address requested")]
    ConnectionExists,
    /// Doesn't support `connect` for a listener.
    #[error("called connect on a listener")]
    Listener,
    /// The handshake is still going on.
    #[error("the handshake has already started")]
    Pending,
    /// Cannot call connect on a connection that is already established.
    #[error("the handshake is completed")]
    Completed,
    /// The handshake is refused by the remote host.
    #[error("the handshake is aborted")]
    Aborted,
    /// A connection error (ICMP error or timeout) occurred.
    #[error("connection error: {0}")]
    ConnectionError(#[from] ConnectionError),
}

/// Possible errors when connecting a socket.
#[derive(Debug, Error, GenericOverIp, PartialEq)]
#[generic_over_ip()]
pub enum BindError {
    /// The socket was already bound.
    #[error("the socket was already bound")]
    AlreadyBound,
    /// The socket cannot bind to the local address.
    #[error(transparent)]
    LocalAddressError(#[from] LocalAddressError),
}

/// Possible errors when retrieving the original destination of a socket.
#[derive(GenericOverIp, Debug, Error)]
#[generic_over_ip()]
pub enum OriginalDestinationError {
    /// Cannot retrieve original destination for an unconnected socket.
    #[error("cannot retrieve original destination for unconnected socket")]
    NotConnected,
    /// The socket's original destination could not be found in the connection
    /// tracking table.
    #[error("socket's original destination could not be found in connection tracking table")]
    NotFound,
    /// The socket's original destination had an unspecified address, which is
    /// invalid for TCP.
    #[error("socket's original destination address should be specified for TCP")]
    UnspecifiedDestinationAddr,
    /// The socket's original destination had an unspecified port, which is
    /// invalid for TCP.
    #[error("socket's original destination port should be specified for TCP")]
    UnspecifiedDestinationPort,
}

/// A `GenericOverIp` wrapper for `I::DemuxSocketId`.
#[derive(GenericOverIp)]
#[generic_over_ip(I, Ip)]
pub struct DemuxSocketId<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes>(
    I::DemuxSocketId<D, BT>,
);

/// A helper trait to implement dual stack demux state access for connect.
///
/// `I` gives access to demux version `I`, which should be the wire IP version.
trait DemuxStateAccessor<I: DualStackIpExt, CC: DeviceIdContext<AnyDevice>, BT: TcpBindingsTypes> {
    /// Calls the callback with access to the demux state for IP version `I`.
    ///
    /// If `cb` returns `Ok`, implementations must remove previous bound-state
    /// demux entries.
    fn update_demux_state_for_connect<
        O,
        E,
        F: FnOnce(
            &I::DemuxSocketId<CC::WeakDeviceId, BT>,
            &mut DemuxState<I, CC::WeakDeviceId, BT>,
        ) -> Result<O, E>,
    >(
        self,
        core_ctx: &mut CC,
        cb: F,
    ) -> Result<O, E>;
}

struct SingleStackDemuxStateAccessor<
    'a,
    I: DualStackIpExt,
    CC: DeviceIdContext<AnyDevice>,
    BT: TcpBindingsTypes,
>(
    &'a I::DemuxSocketId<CC::WeakDeviceId, BT>,
    Option<ListenerAddr<ListenerIpAddr<I::Addr, NonZeroU16>, CC::WeakDeviceId>>,
);

impl<'a, I, CC, BT> DemuxStateAccessor<I, CC, BT> for SingleStackDemuxStateAccessor<'a, I, CC, BT>
where
    I: DualStackIpExt,
    BT: TcpBindingsTypes,
    CC: DeviceIdContext<AnyDevice> + TcpDemuxContext<I, CC::WeakDeviceId, BT>,
{
    fn update_demux_state_for_connect<
        O,
        E,
        F: FnOnce(
            &I::DemuxSocketId<CC::WeakDeviceId, BT>,
            &mut DemuxState<I, CC::WeakDeviceId, BT>,
        ) -> Result<O, E>,
    >(
        self,
        core_ctx: &mut CC,
        cb: F,
    ) -> Result<O, E> {
        core_ctx.with_demux_mut(|demux| {
            let Self(demux_id, listener_addr) = self;
            let output = cb(demux_id, demux)?;

            // If update is successful we must remove the listener address
            // from the demux.

            if let Some(listener_addr) = listener_addr {
                demux
                    .socketmap
                    .listeners_mut()
                    .remove(demux_id, &listener_addr)
                    .expect("failed to remove a bound socket");
            }
            Ok(output)
        })
    }
}

struct DualStackDemuxStateAccessor<
    'a,
    I: DualStackIpExt,
    CC: DeviceIdContext<AnyDevice>,
    BT: TcpBindingsTypes,
>(
    &'a TcpSocketId<I, CC::WeakDeviceId, BT>,
    DualStackTuple<I, Option<ListenerAddr<ListenerIpAddr<I::Addr, NonZeroU16>, CC::WeakDeviceId>>>,
);

impl<'a, SockI, WireI, CC, BT> DemuxStateAccessor<WireI, CC, BT>
    for DualStackDemuxStateAccessor<'a, SockI, CC, BT>
where
    SockI: DualStackIpExt,
    WireI: DualStackIpExt,
    BT: TcpBindingsTypes,
    CC: DeviceIdContext<AnyDevice>
        + TcpDualStackContext<SockI, CC::WeakDeviceId, BT>
        + TcpDemuxContext<WireI, CC::WeakDeviceId, BT>
        + TcpDemuxContext<WireI::OtherVersion, CC::WeakDeviceId, BT>,
{
    fn update_demux_state_for_connect<
        O,
        E,
        F: FnOnce(
            &WireI::DemuxSocketId<CC::WeakDeviceId, BT>,
            &mut DemuxState<WireI, CC::WeakDeviceId, BT>,
        ) -> Result<O, E>,
    >(
        self,
        core_ctx: &mut CC,
        cb: F,
    ) -> Result<O, E> {
        let Self(id, local_addr) = self;
        let (DemuxSocketId(wire_id), DemuxSocketId(other_id)) =
            core_ctx.dual_stack_demux_id(id.clone()).cast::<WireI>().into_inner();
        let (wire_local_addr, other_local_addr) = local_addr.cast::<WireI>().into_inner();
        let output = core_ctx.with_demux_mut(|wire_demux: &mut DemuxState<WireI, _, _>| {
            let output = cb(&wire_id, wire_demux)?;

            // On success we must remove our local address.
            if let Some(wire_local_addr) = wire_local_addr {
                wire_demux
                    .socketmap
                    .listeners_mut()
                    .remove(&wire_id, &wire_local_addr)
                    .expect("failed to remove a bound socket");
            }
            Ok(output)
        })?;

        // If the operation succeeded and we're bound on the other stack then we
        // must clean that up as well.
        if let Some(other_local_addr) = other_local_addr {
            core_ctx.with_demux_mut(|other_demux: &mut DemuxState<WireI::OtherVersion, _, _>| {
                other_demux
                    .socketmap
                    .listeners_mut()
                    .remove(&other_id, &other_local_addr)
                    .expect("failed to remove a bound socket");
            });
        }

        Ok(output)
    }
}

#[derive(Debug)]
enum LocalAddrForConnect<D, A: IpAddress> {
    Unbound { device: Option<D> },
    Listener { addr: ListenerAddr<ListenerIpAddr<A, NonZeroU16>, D> },
}

impl<D, A: IpAddress> LocalAddrForConnect<D, A> {
    fn from_local_addr(
        listener_addr: Option<ListenerAddr<ListenerIpAddr<A, NonZeroU16>, D>>,
        device: Option<D>,
    ) -> Self {
        match listener_addr {
            Some(addr) => Self::Listener { addr },
            None => Self::Unbound { device },
        }
    }
}

fn connect_inner<CC, BC, SockI, WireI, Demux>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    sock_id: &TcpSocketId<SockI, CC::WeakDeviceId, BC>,
    isn: &IsnGenerator<BC::Instant>,
    timestamp_offset: &TimestampOffsetGenerator<BC::Instant>,
    local_addr: LocalAddrForConnect<CC::WeakDeviceId, WireI::Addr>,
    remote_ip: ZonedAddr<SocketIpAddr<WireI::Addr>, CC::DeviceId>,
    remote_port: NonZeroU16,
    active_open: TakeableRef<'_, BC::ListenerNotifierOrProvidedBuffers>,
    buffer_sizes: BufferSizes,
    socket_options: &SocketOptions,
    sharing: SharingState,
    demux: Demux,
    convert_back_op: impl FnOnce(
        Connection<SockI, WireI, CC::WeakDeviceId, BC>,
        ConnAddr<ConnIpAddr<WireI::Addr, NonZeroU16, NonZeroU16>, CC::WeakDeviceId>,
    ) -> SockI::ConnectionAndAddr<CC::WeakDeviceId, BC>,
    convert_timer: impl FnOnce(WeakTcpSocketId<SockI, CC::WeakDeviceId, BC>) -> BC::DispatchId,
) -> Result<TcpSocketStateInner<SockI, CC::WeakDeviceId, BC>, ConnectError>
where
    SockI: DualStackIpExt,
    WireI: DualStackIpExt,
    BC: TcpBindingsContext<CC::DeviceId>,
    CC: TransportIpContext<WireI, BC>
        + DeviceIpSocketHandler<WireI, BC>
        + TcpSocketContext<SockI, CC::WeakDeviceId, BC>,
    Demux: DemuxStateAccessor<WireI, CC, BC>,
{
    let (local_ip, bound_device, local_port) = match local_addr {
        LocalAddrForConnect::Listener {
            addr: ListenerAddr { ip: ListenerIpAddr { addr, identifier }, device },
        } => (addr.and_then(IpDeviceAddr::new_from_socket_ip_addr), device, Some(identifier)),
        LocalAddrForConnect::Unbound { device } => (None, device, None),
    };
    let (remote_ip, device) = remote_ip.resolve_addr_with_device(bound_device)?;

    // TCP sockets cannot connect to multicast addresses so error out early.
    // Per RFC 9293 (https://datatracker.ietf.org/doc/html/rfc9293#name-open):
    //   A TCP implementation MUST reject as an error a local OPEN call for an invalid remote
    //   IP address (e.g., a broadcast or multicast address) (MUST-46).
    if remote_ip.addr().is_multicast()
        || WireI::map_ip_in(remote_ip.addr(), |ip| ip.is_limited_broadcast(), |_| false)
    {
        return Err(ConnectError::NoRoute);
    }

    let ip_sock = core_ctx
        .new_ip_socket(
            bindings_ctx,
            IpSocketArgs {
                device: device.as_ref().map(|d| d.as_ref()),
                local_ip,
                remote_ip,
                proto: IpProto::Tcp.into(),
                options: &socket_options.ip_options,
            },
        )
        .map_err(|err| match err {
            IpSockCreationError::Route(_) => ConnectError::NoRoute,
        })?;

    let device_mms = core_ctx.get_mms(bindings_ctx, &ip_sock, &socket_options.ip_options).map_err(
        |_err: ip::socket::MmsError| {
            // We either cannot find the route, or the device for
            // the route cannot handle the smallest TCP/IP packet.
            ConnectError::NoRoute
        },
    )?;

    let conn_addr =
        demux.update_demux_state_for_connect(core_ctx, |demux_id, DemuxState { socketmap }| {
            let local_port = local_port.map_or_else(
                // NB: Pass the remote port into the allocator to avoid
                // unexpected self-connections when allocating a local port.
                // This could be optimized by checking if the IP socket has
                // resolved to local delivery, but excluding a single port
                // should be enough here and avoids adding more dependencies.
                || match netstack3_base::simple_randomized_port_alloc(
                    &mut bindings_ctx.rng(),
                    &Some(SocketIpAddr::from(*ip_sock.local_ip())),
                    &TcpPortAlloc(socketmap),
                    &Some(remote_port),
                ) {
                    Some(port) => {
                        Ok(NonZeroU16::new(port).expect("ephemeral ports must be non-zero"))
                    }
                    None => Err(ConnectError::NoPort),
                },
                Ok,
            )?;

            let conn_addr = ConnAddr {
                ip: ConnIpAddr {
                    local: (SocketIpAddr::from(*ip_sock.local_ip()), local_port),
                    remote: (*ip_sock.remote_ip(), remote_port),
                },
                device: ip_sock.device().cloned(),
            };

            let _entry = socketmap
                .conns_mut()
                .try_insert(conn_addr.clone(), sharing, demux_id.clone())
                .map_err(|err| match err {
                    // The connection will conflict with an existing one.
                    InsertError::Exists | InsertError::WouldShadowExisting => {
                        ConnectError::ConnectionExists
                    }
                    // Connections don't conflict with listeners, and we should
                    // not observe the following errors.
                    InsertError::ShadowAddrExists | InsertError::IndirectConflict => {
                        panic!("failed to insert connection: {:?}", err)
                    }
                })?;
            Ok::<_, ConnectError>(conn_addr)
        })?;

    let isn = isn.generate::<SocketIpAddr<WireI::Addr>, NonZeroU16>(
        bindings_ctx.now(),
        conn_addr.ip.local,
        conn_addr.ip.remote,
    );
    let timestamp_offset = timestamp_offset.generate::<SocketIpAddr<WireI::Addr>, NonZeroU16>(
        bindings_ctx.now(),
        conn_addr.ip.local,
        conn_addr.ip.remote,
    );

    let now = bindings_ctx.now();
    let mss = Mss::from_mms(device_mms).ok_or(ConnectError::NoRoute)?;

    // No more errors can occur after here, because we're taking active_open
    // buffers out. Use a closure to guard against bad evolution.
    let active_open = active_open.take();
    Ok((move || {
        let (syn_sent, syn) = Closed::<Initial>::connect(
            isn,
            timestamp_offset,
            now,
            active_open,
            buffer_sizes,
            mss,
            Mss::default::<WireI>(),
            socket_options,
        );
        let state = State::<_, BC::ReceiveBuffer, BC::SendBuffer, _>::SynSent(syn_sent);
        let poll_send_at = state.poll_send_at().expect("no retrans timer");

        // Send first SYN packet.
        send_tcp_segment(
            core_ctx,
            bindings_ctx,
            Some(&sock_id),
            Some(&ip_sock),
            conn_addr.ip,
            syn.into_empty(),
            &socket_options.ip_options,
        );

        let mut timer = bindings_ctx.new_timer(convert_timer(sock_id.downgrade()));
        assert_eq!(bindings_ctx.schedule_timer_instant(poll_send_at, &mut timer), None);

        let conn = convert_back_op(
            Connection {
                accept_queue: None,
                state,
                ip_sock,
                defunct: false,
                soft_error: None,
                handshake_status: HandshakeStatus::Pending,
            },
            conn_addr,
        );
        core_ctx.increment_both(sock_id, |counters| &counters.active_connection_openings);
        TcpSocketStateInner::Connected { conn, timer }
    })())
}

/// Information about a socket.
#[derive(Clone, Debug, Eq, PartialEq, GenericOverIp)]
#[generic_over_ip(A, IpAddress)]
pub enum SocketInfo<A: IpAddress, D> {
    /// Unbound socket info.
    Unbound(UnboundInfo<D>),
    /// Bound or listener socket info.
    Bound(BoundInfo<A, D>),
    /// Connection socket info.
    Connection(ConnectionInfo<A, D>),
}

/// Information about an unbound socket.
#[derive(Clone, Debug, Eq, PartialEq, GenericOverIp)]
#[generic_over_ip()]
pub struct UnboundInfo<D> {
    /// The device the socket will be bound to.
    pub device: Option<D>,
}

/// Information about a bound socket's address.
#[derive(Clone, Debug, Eq, PartialEq, GenericOverIp)]
#[generic_over_ip(A, IpAddress)]
pub struct BoundInfo<A: IpAddress, D> {
    /// The IP address the socket is bound to, or `None` for all local IPs.
    pub addr: Option<ZonedAddr<SpecifiedAddr<A>, D>>,
    /// The port number the socket is bound to.
    pub port: NonZeroU16,
    /// The device the socket is bound to.
    pub device: Option<D>,
}

/// Information about a connected socket's address.
#[derive(Clone, Debug, Eq, PartialEq, GenericOverIp)]
#[generic_over_ip(A, IpAddress)]
pub struct ConnectionInfo<A: IpAddress, D> {
    /// The local address the socket is bound to.
    pub local_addr: SocketAddr<A, D>,
    /// The remote address the socket is connected to.
    pub remote_addr: SocketAddr<A, D>,
    /// The device the socket is bound to.
    pub device: Option<D>,
}

impl<D: Clone, Extra> From<&'_ Unbound<D, Extra>> for UnboundInfo<D> {
    fn from(unbound: &Unbound<D, Extra>) -> Self {
        Self { device: unbound.bound_device.clone() }
    }
}

fn maybe_zoned<A: IpAddress, D: Clone>(
    ip: SpecifiedAddr<A>,
    device: &Option<D>,
) -> ZonedAddr<SpecifiedAddr<A>, D> {
    device
        .as_ref()
        .and_then(|device| {
            AddrAndZone::new(ip, device).map(|az| ZonedAddr::Zoned(az.map_zone(Clone::clone)))
        })
        .unwrap_or(ZonedAddr::Unzoned(ip))
}

impl<A: IpAddress, D: Clone> From<ListenerAddr<ListenerIpAddr<A, NonZeroU16>, D>>
    for BoundInfo<A, D>
{
    fn from(addr: ListenerAddr<ListenerIpAddr<A, NonZeroU16>, D>) -> Self {
        let ListenerAddr { ip: ListenerIpAddr { addr, identifier }, device } = addr;
        let addr = addr.map(|ip| maybe_zoned(ip.into(), &device));
        BoundInfo { addr, port: identifier, device }
    }
}

impl<A: IpAddress, D: Clone> From<ConnAddr<ConnIpAddr<A, NonZeroU16, NonZeroU16>, D>>
    for ConnectionInfo<A, D>
{
    fn from(addr: ConnAddr<ConnIpAddr<A, NonZeroU16, NonZeroU16>, D>) -> Self {
        let ConnAddr { ip: ConnIpAddr { local, remote }, device } = addr;
        let convert = |(ip, port): (SocketIpAddr<A>, NonZeroU16)| SocketAddr {
            ip: maybe_zoned(ip.into(), &device),
            port,
        };
        Self { local_addr: convert(local), remote_addr: convert(remote), device }
    }
}

impl<CC, BC> HandleableTimer<CC, BC> for TcpTimerId<CC::WeakDeviceId, BC>
where
    BC: TcpBindingsContext<CC::DeviceId>,
    CC: TcpContext<Ipv4, BC> + TcpContext<Ipv6, BC>,
{
    fn handle(self, core_ctx: &mut CC, bindings_ctx: &mut BC, _: BC::UniqueTimerId) {
        let ctx_pair = CtxPair { core_ctx, bindings_ctx };
        match self {
            TcpTimerId::V4(conn_id) => TcpApi::new(ctx_pair).handle_timer(conn_id),
            TcpTimerId::V6(conn_id) => TcpApi::new(ctx_pair).handle_timer(conn_id),
        }
    }
}

/// Send the given TCP Segment.
///
/// A centralized send path for TCP segments that increments counters and logs
/// errors.
///
/// When `ip_sock` is some, it is used to send the segment, otherwise, one is
/// constructed on demand to send a oneshot segment.
fn send_tcp_segment<'a, WireI, SockI, CC, BC, D>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    socket_id: Option<&TcpSocketId<SockI, D, BC>>,
    ip_sock: Option<&IpSock<WireI, D>>,
    conn_addr: ConnIpAddr<WireI::Addr, NonZeroU16, NonZeroU16>,
    segment: Segment<<BC::SendBuffer as SendBuffer>::Payload<'a>>,
    ip_sock_options: &TcpIpSockOptions,
) where
    WireI: IpExt + FilterIpExt,
    SockI: IpExt + DualStackIpExt,
    CC: TcpSocketContext<SockI, D, BC>
        + IpSocketHandler<WireI, BC, DeviceId = D::Strong, WeakDeviceId = D>,
    BC: TcpBindingsTypes,
    D: WeakDeviceIdentifier,
{
    // NB: TCP does not use tx metadata to enforce send buffer. The TCP
    // application buffers only open send buffer space once the data is
    // acknowledged by the peer. That lives entirely in the TCP module and we
    // don't need to track segments sitting in device queues.
    let tx_metadata: BC::TxMetadata = match socket_id {
        Some(socket_id) => {
            core_ctx.convert_tx_meta(TcpSocketTxMetadata::new(socket_id.downgrade()))
        }
        None => Default::default(),
    };

    let (header, data) = segment.into_parts();
    let control = header.control;
    let result = match ip_sock {
        Some(ip_sock) => {
            let body = tcp_serialize_segment(&header, data, conn_addr);
            core_ctx
                .send_ip_packet(bindings_ctx, ip_sock, body, ip_sock_options, tx_metadata)
                .map_err(|err| IpSockCreateAndSendError::Send(err))
        }
        None => {
            let ConnIpAddr { local: (local_ip, _), remote: (remote_ip, _) } = conn_addr;
            core_ctx.send_oneshot_ip_packet(
                bindings_ctx,
                IpSocketArgs {
                    device: None,
                    local_ip: IpDeviceAddr::new_from_socket_ip_addr(local_ip),
                    remote_ip,
                    proto: IpProto::Tcp.into(),
                    options: ip_sock_options,
                },
                tx_metadata,
                |_addr| tcp_serialize_segment(&header, data, conn_addr),
            )
        }
    };
    match result {
        Ok(()) => {
            counters::increment_counter_with_optional_socket_id(core_ctx, socket_id, |counters| {
                &counters.segments_sent
            });
            if let Some(control) = control {
                counters::increment_counter_with_optional_socket_id(
                    core_ctx,
                    socket_id,
                    |counters| match control {
                        Control::RST => &counters.resets_sent,
                        Control::SYN => &counters.syns_sent,
                        Control::FIN => &counters.fins_sent,
                    },
                )
            }
        }
        Err(err) => {
            counters::increment_counter_with_optional_socket_id(core_ctx, socket_id, |counters| {
                &counters.segment_send_errors
            });
            match socket_id {
                Some(socket_id) => debug!("{:?}: failed to send segment: {:?}", socket_id, err),
                None => debug!("TCP: failed to send segment: {:?}", err),
            }
        }
    }
}

impl<I, C> TcpApi<I, C>
where
    I: DualStackIpExt,
    C: ContextPair,
    C::CoreContext: TcpContext<I, C::BindingsContext>,
    C::BindingsContext: TcpBindingsContext<
        <<C as ContextPair>::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId,
    >,
{
    /// Gets detailed diagnostic information about the TCP socket.
    pub fn get_tcp_info(
        &mut self,
        id: &TcpApiSocketId<I, C>,
    ) -> TcpSocketInfo<<C::BindingsContext as InstantBindingsTypes>::Instant> {
        self.core_ctx().with_socket(id, |socket_state| socket_state.tcp_info(id.counters()))
    }
}

#[cfg(test)]
mod tests {
    use alloc::rc::Rc;
    use alloc::string::String;
    use alloc::sync::Arc;
    use alloc::vec::Vec;
    use alloc::{format, vec};
    use core::cell::RefCell;
    use core::num::NonZeroU16;
    use core::time::Duration;

    use ip_test_macro::ip_test;
    use net_declare::{net_ip_v4, net_ip_v6};
    use net_types::ip::{Ip, IpAddr, IpVersion, Ipv4, Ipv4SourceAddr, Ipv6, Ipv6SourceAddr, Mtu};
    use net_types::{LinkLocalAddr, Witness};
    use netstack3_base::sync::{DynDebugReferences, Mutex};
    use netstack3_base::testutil::{
        AlwaysDefaultsSettingsContext, FakeAtomicInstant, FakeCoreCtx, FakeCryptoRng, FakeDeviceId,
        FakeInstant, FakeNetwork, FakeNetworkSpec, FakeStrongDeviceId, FakeTimerCtx, FakeTimerId,
        FakeTxMetadata, FakeWeakDeviceId, InstantAndData, MultipleDevicesId, PendingFrameData,
        StepResult, TestIpExt, WithFakeFrameContext, WithFakeTimerContext, new_rng,
        run_with_many_seeds, set_logger_for_test,
    };
    use netstack3_base::{
        ContextProvider, CounterCollection, CounterContext, IcmpIpExt, Icmpv4ErrorCode,
        Icmpv6ErrorCode, Instant as _, InstantContext, LinkDevice, Mark, MarkDomain,
        MatcherBindingsTypes, Mms, NetworkSerializationContext, ReferenceNotifiers,
        ResourceCounterContext, StrongDeviceIdentifier, Uninstantiable, UninstantiableWrapper,
    };
    use netstack3_filter::testutil::NoOpSocketOpsFilter;
    use netstack3_filter::{SocketOpsFilter, TransportPacketSerializer, Tuple};
    use netstack3_ip::device::IpDeviceStateIpExt;
    use netstack3_ip::nud::LinkResolutionContext;
    use netstack3_ip::nud::testutil::FakeLinkResolutionNotifier;
    use netstack3_ip::socket::testutil::{FakeDeviceConfig, FakeDualStackIpSocketCtx};
    use netstack3_ip::socket::{IpSockSendError, MmsError, RouteResolutionOptions, SendOptions};
    use netstack3_ip::testutil::DualStackSendIpPacketMeta;
    use netstack3_ip::{
        BaseTransportIpContext, HopLimits, IpTransportContext, LocalDeliveryPacketInfo,
    };
    use packet::{Buf, BufferMut, NestablePacketBuilder as _, ParseBuffer as _, Serializer as _};
    use packet_formats::icmp::{
        IcmpDestUnreachable, Icmpv4DestUnreachableCode, Icmpv4ParameterProblemCode,
        Icmpv4TimeExceededCode, Icmpv6DestUnreachableCode, Icmpv6ParameterProblemCode,
        Icmpv6TimeExceededCode,
    };
    use packet_formats::tcp::{TcpParseArgs, TcpSegment, TcpSegmentBuilder};
    use rand::RngExt as _;
    use test_case::test_case;
    use test_util::assert_gt;

    use super::*;
    use crate::internal::base::{ConnectionError, DEFAULT_FIN_WAIT2_TIMEOUT};
    use crate::internal::buffer::BufferLimits;
    use crate::internal::buffer::testutil::{
        ClientBuffers, ProvidedBuffers, RingBuffer, TestSendBuffer, WriteBackClientBuffers,
    };
    use crate::internal::congestion::CongestionWindow;
    use crate::internal::counters::TcpCountersWithoutSocket;
    use crate::internal::counters::testutil::{
        CounterExpectations, CounterExpectationsWithoutSocket,
    };
    use crate::internal::state::{Established, MSL, TimeWait};

    pub(crate) trait TcpTestIpExt:
        DualStackIpExt + TestIpExt + IpDeviceStateIpExt + DualStackIpExt
    {
        type SingleStackConverter: SingleStackConverter<Self, FakeWeakDeviceId<FakeDeviceId>, TcpBindingsCtx<FakeDeviceId>>;
        type DualStackConverter: DualStackConverter<Self, FakeWeakDeviceId<FakeDeviceId>, TcpBindingsCtx<FakeDeviceId>>;
        fn recv_src_addr(addr: Self::Addr) -> Self::RecvSrcAddr;

        fn converter() -> MaybeDualStack<Self::DualStackConverter, Self::SingleStackConverter>;
    }

    /// This trait anchors the timer DispatchId for our context implementations
    /// that require a core converter.
    ///
    /// This is required because we implement the traits on [`TcpCoreCtx`]
    /// abstracting away the bindings types, even though they're always
    /// [`TcpBindingsCtx`].
    trait TcpTestBindingsTypes<D: StrongDeviceIdentifier>:
        TcpBindingsTypes<DispatchId = TcpTimerId<D::Weak, Self>> + Sized
    {
    }

    impl<D, BT> TcpTestBindingsTypes<D> for BT
    where
        BT: TcpBindingsTypes<DispatchId = TcpTimerId<D::Weak, Self>> + Sized,
        D: StrongDeviceIdentifier,
    {
    }

    struct FakeTcpState<I: TcpTestIpExt, D: FakeStrongDeviceId, BT: TcpBindingsTypes> {
        isn_generator: Rc<IsnGenerator<BT::Instant>>,
        timestamp_offset_generator: Rc<TimestampOffsetGenerator<BT::Instant>>,
        demux: Rc<RefCell<DemuxState<I, D::Weak, BT>>>,
        // Always destroy all sockets last so the strong references in the demux
        // are gone.
        all_sockets: TcpSocketSet<I, D::Weak, BT>,
        counters_with_socket: TcpCountersWithSocket<I>,
        counters_without_socket: TcpCountersWithoutSocket<I>,
    }

    impl<I, D, BT> Default for FakeTcpState<I, D, BT>
    where
        I: TcpTestIpExt,
        D: FakeStrongDeviceId,
        BT: TcpBindingsTypes,
        BT::Instant: Default,
    {
        fn default() -> Self {
            Self {
                isn_generator: Default::default(),
                timestamp_offset_generator: Default::default(),
                all_sockets: Default::default(),
                demux: Rc::new(RefCell::new(DemuxState { socketmap: Default::default() })),
                counters_with_socket: Default::default(),
                counters_without_socket: Default::default(),
            }
        }
    }

    struct FakeDualStackTcpState<D: FakeStrongDeviceId, BT: TcpBindingsTypes> {
        v4: FakeTcpState<Ipv4, D, BT>,
        v6: FakeTcpState<Ipv6, D, BT>,
    }

    impl<D, BT> Default for FakeDualStackTcpState<D, BT>
    where
        D: FakeStrongDeviceId,
        BT: TcpBindingsTypes,
        BT::Instant: Default,
    {
        fn default() -> Self {
            Self { v4: Default::default(), v6: Default::default() }
        }
    }

    type InnerCoreCtx<D> =
        FakeCoreCtx<FakeDualStackIpSocketCtx<D>, DualStackSendIpPacketMeta<D>, D>;

    pub(crate) struct TcpCoreCtx<D: FakeStrongDeviceId, BT: TcpBindingsTypes> {
        tcp: FakeDualStackTcpState<D, BT>,
        ip_socket_ctx: InnerCoreCtx<D>,
        // Marks to attach for incoming packets.
        recv_packet_marks: netstack3_base::Marks,
    }

    impl<D: FakeStrongDeviceId, BT: TcpBindingsTypes> ContextProvider for TcpCoreCtx<D, BT> {
        type Context = Self;

        fn context(&mut self) -> &mut Self::Context {
            self
        }
    }

    impl<D, BT> DeviceIdContext<AnyDevice> for TcpCoreCtx<D, BT>
    where
        D: FakeStrongDeviceId,
        BT: TcpBindingsTypes,
    {
        type DeviceId = D;
        type WeakDeviceId = FakeWeakDeviceId<D>;
    }

    pub(crate) type TcpCtx<D> = CtxPair<TcpCoreCtx<D, TcpBindingsCtx<D>>, TcpBindingsCtx<D>>;

    pub(crate) struct FakeTcpNetworkSpec<D: FakeStrongDeviceId>(PhantomData<D>, Never);
    impl<D: FakeStrongDeviceId> FakeNetworkSpec for FakeTcpNetworkSpec<D> {
        type Context = TcpCtx<D>;
        type TimerId = TcpTimerId<D::Weak, TcpBindingsCtx<D>>;
        type SendMeta = DualStackSendIpPacketMeta<D>;
        type RecvMeta = DualStackSendIpPacketMeta<D>;
        fn handle_frame(ctx: &mut Self::Context, meta: Self::RecvMeta, buffer: Buf<Vec<u8>>) {
            let TcpCtx { core_ctx, bindings_ctx } = ctx;
            match meta {
                DualStackSendIpPacketMeta::V4(meta) => {
                    let early_demux_socket =
                        <TcpIpTransportContext as IpTransportContext<Ipv4, _, _>>::early_demux(
                            core_ctx,
                            &meta.device,
                            *meta.src_ip,
                            *meta.dst_ip,
                            buffer.as_ref(),
                        );
                    <TcpIpTransportContext as IpTransportContext<Ipv4, _, _>>::receive_ip_packet(
                        core_ctx,
                        bindings_ctx,
                        &meta.device,
                        Ipv4::recv_src_addr(*meta.src_ip),
                        meta.dst_ip,
                        buffer,
                        &mut LocalDeliveryPacketInfo {
                            marks: core_ctx.recv_packet_marks,
                            ..Default::default()
                        },
                        early_demux_socket,
                    )
                    .expect("failed to deliver bytes");
                }
                DualStackSendIpPacketMeta::V6(meta) => {
                    let early_demux_socket =
                        <TcpIpTransportContext as IpTransportContext<Ipv6, _, _>>::early_demux(
                            core_ctx,
                            &meta.device,
                            *meta.src_ip,
                            *meta.dst_ip,
                            buffer.as_ref(),
                        );
                    <TcpIpTransportContext as IpTransportContext<Ipv6, _, _>>::receive_ip_packet(
                        core_ctx,
                        bindings_ctx,
                        &meta.device,
                        Ipv6::recv_src_addr(*meta.src_ip),
                        meta.dst_ip,
                        buffer,
                        &mut LocalDeliveryPacketInfo {
                            marks: core_ctx.recv_packet_marks,
                            ..Default::default()
                        },
                        early_demux_socket,
                    )
                    .expect("failed to deliver bytes");
                }
            }
        }
        fn handle_timer(ctx: &mut Self::Context, dispatch: Self::TimerId, _: FakeTimerId) {
            match dispatch {
                TcpTimerId::V4(id) => ctx.tcp_api().handle_timer(id),
                TcpTimerId::V6(id) => ctx.tcp_api().handle_timer(id),
            }
        }
        fn process_queues(_ctx: &mut Self::Context) -> bool {
            false
        }
        fn fake_frames(ctx: &mut Self::Context) -> &mut impl WithFakeFrameContext<Self::SendMeta> {
            &mut ctx.core_ctx.ip_socket_ctx.frames
        }
    }

    impl<D: FakeStrongDeviceId> WithFakeTimerContext<TcpTimerId<D::Weak, TcpBindingsCtx<D>>>
        for TcpCtx<D>
    {
        fn with_fake_timer_ctx<
            O,
            F: FnOnce(&FakeTimerCtx<TcpTimerId<D::Weak, TcpBindingsCtx<D>>>) -> O,
        >(
            &self,
            f: F,
        ) -> O {
            let Self { core_ctx: _, bindings_ctx } = self;
            f(&bindings_ctx.timers)
        }

        fn with_fake_timer_ctx_mut<
            O,
            F: FnOnce(&mut FakeTimerCtx<TcpTimerId<D::Weak, TcpBindingsCtx<D>>>) -> O,
        >(
            &mut self,
            f: F,
        ) -> O {
            let Self { core_ctx: _, bindings_ctx } = self;
            f(&mut bindings_ctx.timers)
        }
    }

    #[derive(Derivative)]
    #[derivative(Default(bound = ""))]
    pub(crate) struct TcpBindingsCtx<D: FakeStrongDeviceId> {
        rng: FakeCryptoRng,
        timers: FakeTimerCtx<TcpTimerId<D::Weak, Self>>,
    }

    impl<D: FakeStrongDeviceId> ContextProvider for TcpBindingsCtx<D> {
        type Context = Self;
        fn context(&mut self) -> &mut Self::Context {
            self
        }
    }

    impl<D: LinkDevice + FakeStrongDeviceId> LinkResolutionContext<D> for TcpBindingsCtx<D> {
        type Notifier = FakeLinkResolutionNotifier<D>;
    }

    /// Delegate implementation to internal thing.
    impl<D: FakeStrongDeviceId> TimerBindingsTypes for TcpBindingsCtx<D> {
        type Timer = <FakeTimerCtx<TcpTimerId<D::Weak, Self>> as TimerBindingsTypes>::Timer;
        type DispatchId =
            <FakeTimerCtx<TcpTimerId<D::Weak, Self>> as TimerBindingsTypes>::DispatchId;
        type UniqueTimerId =
            <FakeTimerCtx<TcpTimerId<D::Weak, Self>> as TimerBindingsTypes>::UniqueTimerId;
    }

    /// Delegate implementation to internal thing.
    impl<D: FakeStrongDeviceId> InstantBindingsTypes for TcpBindingsCtx<D> {
        type Instant = FakeInstant;
        type AtomicInstant = FakeAtomicInstant;
    }

    impl<D: FakeStrongDeviceId> SocketOpsFilterBindingContext<D> for TcpBindingsCtx<D> {
        fn socket_ops_filter(&self) -> impl SocketOpsFilter<D> {
            NoOpSocketOpsFilter
        }
    }

    /// Delegate implementation to internal thing.
    impl<D: FakeStrongDeviceId> InstantContext for TcpBindingsCtx<D> {
        fn now(&self) -> FakeInstant {
            self.timers.now()
        }
    }

    /// Delegate implementation to internal thing.
    impl<D: FakeStrongDeviceId> TimerContext for TcpBindingsCtx<D> {
        fn new_timer(&mut self, id: Self::DispatchId) -> Self::Timer {
            self.timers.new_timer(id)
        }

        fn schedule_timer_instant(
            &mut self,
            time: Self::Instant,
            timer: &mut Self::Timer,
        ) -> Option<Self::Instant> {
            self.timers.schedule_timer_instant(time, timer)
        }

        fn cancel_timer(&mut self, timer: &mut Self::Timer) -> Option<Self::Instant> {
            self.timers.cancel_timer(timer)
        }

        fn scheduled_instant(&self, timer: &mut Self::Timer) -> Option<Self::Instant> {
            self.timers.scheduled_instant(timer)
        }

        fn unique_timer_id(&self, timer: &Self::Timer) -> Self::UniqueTimerId {
            self.timers.unique_timer_id(timer)
        }
    }

    impl<D: FakeStrongDeviceId> ReferenceNotifiers for TcpBindingsCtx<D> {
        type ReferenceReceiver<T: 'static> = Never;

        type ReferenceNotifier<T: Send + 'static> = Never;

        fn new_reference_notifier<T: Send + 'static>(
            debug_references: DynDebugReferences,
        ) -> (Self::ReferenceNotifier<T>, Self::ReferenceReceiver<T>) {
            // We don't support deferred destruction, tests are single threaded.
            panic!(
                "can't create deferred reference notifiers for type {}: \
                debug_references={debug_references:?}",
                core::any::type_name::<T>()
            );
        }
    }

    impl<D: FakeStrongDeviceId> DeferredResourceRemovalContext for TcpBindingsCtx<D> {
        fn defer_removal<T: Send + 'static>(&mut self, receiver: Self::ReferenceReceiver<T>) {
            match receiver {}
        }
    }

    impl<D: FakeStrongDeviceId> RngContext for TcpBindingsCtx<D> {
        type Rng<'a> = &'a mut FakeCryptoRng;
        fn rng(&mut self) -> Self::Rng<'_> {
            &mut self.rng
        }
    }

    impl<D: FakeStrongDeviceId> TxMetadataBindingsTypes for TcpBindingsCtx<D> {
        type TxMetadata = FakeTxMetadata;
    }

    impl<D: FakeStrongDeviceId> MatcherBindingsTypes for TcpBindingsCtx<D> {
        type DeviceClass = ();
        type BindingsPacketMatcher = Never;
    }

    impl<D: FakeStrongDeviceId> TcpBindingsTypes for TcpBindingsCtx<D> {
        type ReceiveBuffer = Arc<Mutex<RingBuffer>>;
        type SendBuffer = TestSendBuffer;
        type ReturnedBuffers = ClientBuffers;
        type ListenerNotifierOrProvidedBuffers = ProvidedBuffers;

        fn new_passive_open_buffers(
            buffer_sizes: BufferSizes,
        ) -> (Self::ReceiveBuffer, Self::SendBuffer, Self::ReturnedBuffers) {
            let client = ClientBuffers::new(buffer_sizes);
            (
                Arc::clone(&client.receive),
                TestSendBuffer::new(Arc::clone(&client.send), RingBuffer::default()),
                client,
            )
        }
    }

    impl<D: FakeStrongDeviceId> AlwaysDefaultsSettingsContext for TcpBindingsCtx<D> {}

    impl<D: FakeStrongDeviceId> TcpSocketDestructionContext for TcpBindingsCtx<D> {
        fn defer_tcp_socket_destruction<I, S>(
            &self,
            _result: RemoveResourceResultWithContext<S, Self>,
        ) where
            I: Ip,
            S: SocketDiagnosticsSeed<Output = TcpSocketDiagnostics<I, Self::Instant>>
                + Send
                + 'static,
        {
            // Do nothing since we don't care about these notifications in unit tests.
        }
    }

    const LINK_MTU: Mtu = Mtu::new(1500);

    impl<I, D, BC> DeviceIpSocketHandler<I, BC> for TcpCoreCtx<D, BC>
    where
        I: TcpTestIpExt,
        D: FakeStrongDeviceId,
        BC: TcpTestBindingsTypes<D>,
    {
        fn get_mms<O>(
            &mut self,
            _bindings_ctx: &mut BC,
            _ip_sock: &IpSock<I, Self::WeakDeviceId>,
            _options: &O,
        ) -> Result<Mms, MmsError>
        where
            O: RouteResolutionOptions<I>,
        {
            Ok(Mms::from_mtu::<I>(LINK_MTU, 0).unwrap())
        }
    }

    /// Delegate implementation to inner context.
    impl<I, D, BC> BaseTransportIpContext<I, BC> for TcpCoreCtx<D, BC>
    where
        I: TcpTestIpExt,
        D: FakeStrongDeviceId,
        BC: TcpTestBindingsTypes<D>,
    {
        type DevicesWithAddrIter<'a>
            = <InnerCoreCtx<D> as BaseTransportIpContext<I, BC>>::DevicesWithAddrIter<'a>
        where
            Self: 'a;

        fn with_devices_with_assigned_addr<O, F: FnOnce(Self::DevicesWithAddrIter<'_>) -> O>(
            &mut self,
            addr: SpecifiedAddr<I::Addr>,
            cb: F,
        ) -> O {
            BaseTransportIpContext::<I, BC>::with_devices_with_assigned_addr(
                &mut self.ip_socket_ctx,
                addr,
                cb,
            )
        }

        fn get_default_hop_limits(&mut self, device: Option<&Self::DeviceId>) -> HopLimits {
            BaseTransportIpContext::<I, BC>::get_default_hop_limits(&mut self.ip_socket_ctx, device)
        }

        fn get_original_destination(&mut self, tuple: &Tuple<I>) -> Option<(I::Addr, u16)> {
            BaseTransportIpContext::<I, BC>::get_original_destination(
                &mut self.ip_socket_ctx,
                tuple,
            )
        }
    }

    /// Delegate implementation to inner context.
    impl<I: TcpTestIpExt, D: FakeStrongDeviceId, BC: TcpTestBindingsTypes<D>> IpSocketHandler<I, BC>
        for TcpCoreCtx<D, BC>
    {
        fn new_ip_socket<O>(
            &mut self,
            bindings_ctx: &mut BC,
            args: IpSocketArgs<'_, Self::DeviceId, I, O>,
        ) -> Result<IpSock<I, Self::WeakDeviceId>, IpSockCreationError>
        where
            O: RouteResolutionOptions<I>,
        {
            IpSocketHandler::<I, BC>::new_ip_socket(&mut self.ip_socket_ctx, bindings_ctx, args)
        }

        fn send_ip_packet<S, O>(
            &mut self,
            bindings_ctx: &mut BC,
            socket: &IpSock<I, Self::WeakDeviceId>,
            body: S,
            options: &O,
            tx_meta: BC::TxMetadata,
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
            bindings_ctx: &mut BC,
            socket: &IpSock<I, Self::WeakDeviceId>,
            options: &O,
        ) where
            O: RouteResolutionOptions<I>,
        {
            self.ip_socket_ctx.confirm_reachable(bindings_ctx, socket, options)
        }
    }

    impl<D, BC> TcpDemuxContext<Ipv4, D::Weak, BC> for TcpCoreCtx<D, BC>
    where
        D: FakeStrongDeviceId,
        BC: TcpTestBindingsTypes<D>,
    {
        type IpTransportCtx<'a> = Self;
        fn with_demux<O, F: FnOnce(&DemuxState<Ipv4, D::Weak, BC>) -> O>(&mut self, cb: F) -> O {
            cb(&self.tcp.v4.demux.borrow())
        }

        fn with_demux_mut<O, F: FnOnce(&mut DemuxState<Ipv4, D::Weak, BC>) -> O>(
            &mut self,
            cb: F,
        ) -> O {
            cb(&mut self.tcp.v4.demux.borrow_mut())
        }
    }

    impl<D, BC> TcpDemuxContext<Ipv6, D::Weak, BC> for TcpCoreCtx<D, BC>
    where
        D: FakeStrongDeviceId,
        BC: TcpTestBindingsTypes<D>,
    {
        type IpTransportCtx<'a> = Self;
        fn with_demux<O, F: FnOnce(&DemuxState<Ipv6, D::Weak, BC>) -> O>(&mut self, cb: F) -> O {
            cb(&self.tcp.v6.demux.borrow())
        }

        fn with_demux_mut<O, F: FnOnce(&mut DemuxState<Ipv6, D::Weak, BC>) -> O>(
            &mut self,
            cb: F,
        ) -> O {
            cb(&mut self.tcp.v6.demux.borrow_mut())
        }
    }

    impl<I, D, BT> CoreTimerContext<WeakTcpSocketId<I, D::Weak, BT>, BT> for TcpCoreCtx<D, BT>
    where
        I: DualStackIpExt,
        D: FakeStrongDeviceId,
        BT: TcpTestBindingsTypes<D>,
    {
        fn convert_timer(dispatch_id: WeakTcpSocketId<I, D::Weak, BT>) -> BT::DispatchId {
            dispatch_id.into()
        }
    }

    impl<I, D, BC> CoreTxMetadataContext<TcpSocketTxMetadata<I, D::Weak, BC>, BC> for TcpCoreCtx<D, BC>
    where
        I: TcpTestIpExt,
        D: FakeStrongDeviceId,
        BC: TcpTestBindingsTypes<D>,
    {
        fn convert_tx_meta(&self, _tx_meta: TcpSocketTxMetadata<I, D::Weak, BC>) -> BC::TxMetadata {
            Default::default()
        }
    }

    impl<D: FakeStrongDeviceId, BC: TcpTestBindingsTypes<D>> TcpContext<Ipv6, BC>
        for TcpCoreCtx<D, BC>
    {
        type ThisStackIpTransportAndDemuxCtx<'a> = Self;
        type SingleStackIpTransportAndDemuxCtx<'a> = UninstantiableWrapper<Self>;
        type SingleStackConverter = Uninstantiable;
        type DualStackIpTransportAndDemuxCtx<'a> = Self;
        type DualStackConverter = ();
        fn with_all_sockets_mut<
            O,
            F: FnOnce(&mut TcpSocketSet<Ipv6, Self::WeakDeviceId, BC>) -> O,
        >(
            &mut self,
            cb: F,
        ) -> O {
            cb(&mut self.tcp.v6.all_sockets)
        }

        fn for_each_socket<
            F: FnMut(
                &TcpSocketId<Ipv6, Self::WeakDeviceId, BC>,
                &TcpSocketState<Ipv6, Self::WeakDeviceId, BC>,
            ),
        >(
            &mut self,
            mut cb: F,
        ) {
            for id in self.tcp.v6.all_sockets.keys() {
                cb(id, &id.get());
            }
        }

        fn with_socket_mut_generators_transport_demux<
            O,
            F: for<'a> FnOnce(
                MaybeDualStack<
                    (&'a mut Self::DualStackIpTransportAndDemuxCtx<'a>, Self::DualStackConverter),
                    (
                        &'a mut Self::SingleStackIpTransportAndDemuxCtx<'a>,
                        Self::SingleStackConverter,
                    ),
                >,
                &mut TcpSocketState<Ipv6, Self::WeakDeviceId, BC>,
                &IsnGenerator<BC::Instant>,
                &TimestampOffsetGenerator<BC::Instant>,
            ) -> O,
        >(
            &mut self,
            id: &TcpSocketId<Ipv6, Self::WeakDeviceId, BC>,
            cb: F,
        ) -> O {
            let isn = Rc::clone(&self.tcp.v6.isn_generator);
            let timestamp_offset = Rc::clone(&self.tcp.v6.timestamp_offset_generator);
            cb(
                MaybeDualStack::DualStack((self, ())),
                id.get_mut().deref_mut(),
                isn.deref(),
                timestamp_offset.deref(),
            )
        }

        fn with_socket_and_converter<
            O,
            F: FnOnce(
                &TcpSocketState<Ipv6, Self::WeakDeviceId, BC>,
                MaybeDualStack<Self::DualStackConverter, Self::SingleStackConverter>,
            ) -> O,
        >(
            &mut self,
            id: &TcpSocketId<Ipv6, Self::WeakDeviceId, BC>,
            cb: F,
        ) -> O {
            cb(id.get_mut().deref_mut(), MaybeDualStack::DualStack(()))
        }
    }

    impl<D: FakeStrongDeviceId, BC: TcpTestBindingsTypes<D>> TcpContext<Ipv4, BC>
        for TcpCoreCtx<D, BC>
    {
        type ThisStackIpTransportAndDemuxCtx<'a> = Self;
        type SingleStackIpTransportAndDemuxCtx<'a> = Self;
        type SingleStackConverter = ();
        type DualStackIpTransportAndDemuxCtx<'a> = UninstantiableWrapper<Self>;
        type DualStackConverter = Uninstantiable;
        fn with_all_sockets_mut<
            O,
            F: FnOnce(&mut TcpSocketSet<Ipv4, Self::WeakDeviceId, BC>) -> O,
        >(
            &mut self,
            cb: F,
        ) -> O {
            cb(&mut self.tcp.v4.all_sockets)
        }

        fn for_each_socket<
            F: FnMut(
                &TcpSocketId<Ipv4, Self::WeakDeviceId, BC>,
                &TcpSocketState<Ipv4, Self::WeakDeviceId, BC>,
            ),
        >(
            &mut self,
            mut cb: F,
        ) {
            for id in self.tcp.v4.all_sockets.keys() {
                cb(id, &id.get());
            }
        }

        fn with_socket_mut_generators_transport_demux<
            O,
            F: for<'a> FnOnce(
                MaybeDualStack<
                    (&'a mut Self::DualStackIpTransportAndDemuxCtx<'a>, Self::DualStackConverter),
                    (
                        &'a mut Self::SingleStackIpTransportAndDemuxCtx<'a>,
                        Self::SingleStackConverter,
                    ),
                >,
                &mut TcpSocketState<Ipv4, Self::WeakDeviceId, BC>,
                &IsnGenerator<BC::Instant>,
                &TimestampOffsetGenerator<BC::Instant>,
            ) -> O,
        >(
            &mut self,
            id: &TcpSocketId<Ipv4, Self::WeakDeviceId, BC>,
            cb: F,
        ) -> O {
            let isn: Rc<IsnGenerator<<BC as InstantBindingsTypes>::Instant>> =
                Rc::clone(&self.tcp.v4.isn_generator);
            let timestamp_offset: Rc<
                TimestampOffsetGenerator<<BC as InstantBindingsTypes>::Instant>,
            > = Rc::clone(&self.tcp.v4.timestamp_offset_generator);
            cb(
                MaybeDualStack::NotDualStack((self, ())),
                id.get_mut().deref_mut(),
                isn.deref(),
                timestamp_offset.deref(),
            )
        }

        fn with_socket_and_converter<
            O,
            F: FnOnce(
                &TcpSocketState<Ipv4, Self::WeakDeviceId, BC>,
                MaybeDualStack<Self::DualStackConverter, Self::SingleStackConverter>,
            ) -> O,
        >(
            &mut self,
            id: &TcpSocketId<Ipv4, Self::WeakDeviceId, BC>,
            cb: F,
        ) -> O {
            cb(id.get_mut().deref_mut(), MaybeDualStack::NotDualStack(()))
        }
    }

    impl<D: FakeStrongDeviceId, BT: TcpTestBindingsTypes<D>>
        TcpDualStackContext<Ipv6, FakeWeakDeviceId<D>, BT> for TcpCoreCtx<D, BT>
    {
        type DualStackIpTransportCtx<'a> = Self;
        fn other_demux_id_converter(&self) -> impl DualStackDemuxIdConverter<Ipv6> {
            Ipv6SocketIdToIpv4DemuxIdConverter
        }
        fn dual_stack_enabled(&self, ip_options: &Ipv6Options) -> bool {
            ip_options.dual_stack_enabled
        }
        fn set_dual_stack_enabled(&self, ip_options: &mut Ipv6Options, value: bool) {
            ip_options.dual_stack_enabled = value;
        }
        fn with_both_demux_mut<
            O,
            F: FnOnce(
                &mut DemuxState<Ipv6, FakeWeakDeviceId<D>, BT>,
                &mut DemuxState<Ipv4, FakeWeakDeviceId<D>, BT>,
            ) -> O,
        >(
            &mut self,
            cb: F,
        ) -> O {
            cb(&mut self.tcp.v6.demux.borrow_mut(), &mut self.tcp.v4.demux.borrow_mut())
        }
    }

    impl<I: Ip, D: FakeStrongDeviceId, BT: TcpTestBindingsTypes<D>>
        CounterContext<TcpCountersWithSocket<I>> for TcpCoreCtx<D, BT>
    {
        fn counters(&self) -> &TcpCountersWithSocket<I> {
            I::map_ip(
                (),
                |()| &self.tcp.v4.counters_with_socket,
                |()| &self.tcp.v6.counters_with_socket,
            )
        }
    }

    impl<I: Ip, D: FakeStrongDeviceId, BT: TcpTestBindingsTypes<D>>
        CounterContext<TcpCountersWithoutSocket<I>> for TcpCoreCtx<D, BT>
    {
        fn counters(&self) -> &TcpCountersWithoutSocket<I> {
            I::map_ip(
                (),
                |()| &self.tcp.v4.counters_without_socket,
                |()| &self.tcp.v6.counters_without_socket,
            )
        }
    }

    impl<I: DualStackIpExt, D: FakeStrongDeviceId, BT: TcpTestBindingsTypes<D>>
        ResourceCounterContext<TcpSocketId<I, FakeWeakDeviceId<D>, BT>, TcpCountersWithSocket<I>>
        for TcpCoreCtx<D, BT>
    {
        fn per_resource_counters<'a>(
            &'a self,
            resource: &'a TcpSocketId<I, FakeWeakDeviceId<D>, BT>,
        ) -> &'a TcpCountersWithSocket<I> {
            resource.counters()
        }
    }

    impl<D, BT> TcpCoreCtx<D, BT>
    where
        D: FakeStrongDeviceId,
        BT: TcpBindingsTypes,
        BT::Instant: Default,
    {
        fn with_ip_socket_ctx_state(state: FakeDualStackIpSocketCtx<D>) -> Self {
            Self {
                tcp: Default::default(),
                ip_socket_ctx: FakeCoreCtx::with_state(state),
                recv_packet_marks: Default::default(),
            }
        }
    }

    impl TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>> {
        pub(crate) fn new<I: TcpTestIpExt>(
            addr: SpecifiedAddr<I::Addr>,
            peer: SpecifiedAddr<I::Addr>,
        ) -> Self {
            Self::with_ip_socket_ctx_state(FakeDualStackIpSocketCtx::new(core::iter::once(
                FakeDeviceConfig {
                    device: FakeDeviceId,
                    local_ips: vec![addr],
                    remote_ips: vec![peer],
                },
            )))
        }
    }

    impl TcpCoreCtx<MultipleDevicesId, TcpBindingsCtx<MultipleDevicesId>> {
        fn new_multiple_devices() -> Self {
            let ip_v4_local =
                SpecifiedAddr::new(IpAddr::V4(*<Ipv4 as TestIpExt>::TEST_ADDRS.local_ip)).unwrap();
            let ip_v4_remote =
                SpecifiedAddr::new(IpAddr::V4(*<Ipv4 as TestIpExt>::TEST_ADDRS.remote_ip)).unwrap();
            let ip_v6_local =
                SpecifiedAddr::new(IpAddr::V6(*<Ipv6 as TestIpExt>::TEST_ADDRS.local_ip)).unwrap();
            let ip_v6_remote =
                SpecifiedAddr::new(IpAddr::V6(*<Ipv6 as TestIpExt>::TEST_ADDRS.remote_ip)).unwrap();

            let ips = vec![ip_v4_local, ip_v4_remote, ip_v6_local, ip_v6_remote];

            Self::with_ip_socket_ctx_state(FakeDualStackIpSocketCtx::new(
                MultipleDevicesId::all().into_iter().map(|device| {
                    let local_ips = match device {
                        MultipleDevicesId::A | MultipleDevicesId::B => ips.clone(),
                        MultipleDevicesId::C => Vec::new(),
                    };
                    FakeDeviceConfig { device, remote_ips: local_ips.clone(), local_ips }
                }),
            ))
        }
    }

    impl<D: WeakDeviceIdentifier, BT: TcpBindingsTypes> TcpTimerId<D, BT> {
        fn assert_ip_version<I: DualStackIpExt>(self) -> WeakTcpSocketId<I, D, BT> {
            I::map_ip_out(
                self,
                |v4| assert_matches!(v4, TcpTimerId::V4(v4) => v4),
                |v6| assert_matches!(v6, TcpTimerId::V6(v6) => v6),
            )
        }
    }

    const LOCAL: &'static str = "local";
    const REMOTE: &'static str = "remote";
    pub(crate) const PORT_1: NonZeroU16 = NonZeroU16::new(42).unwrap();
    const PORT_2: NonZeroU16 = NonZeroU16::new(43).unwrap();

    impl TcpTestIpExt for Ipv4 {
        type SingleStackConverter = ();
        type DualStackConverter = Uninstantiable;
        fn converter() -> MaybeDualStack<Self::DualStackConverter, Self::SingleStackConverter> {
            MaybeDualStack::NotDualStack(())
        }
        fn recv_src_addr(addr: Self::Addr) -> Self::RecvSrcAddr {
            Ipv4SourceAddr::new(addr).unwrap()
        }
    }

    impl TcpTestIpExt for Ipv6 {
        type SingleStackConverter = Uninstantiable;
        type DualStackConverter = ();
        fn converter() -> MaybeDualStack<Self::DualStackConverter, Self::SingleStackConverter> {
            MaybeDualStack::DualStack(())
        }
        fn recv_src_addr(addr: Self::Addr) -> Self::RecvSrcAddr {
            Ipv6SourceAddr::new(addr).unwrap()
        }
    }

    type TcpTestNetwork = FakeNetwork<
        FakeTcpNetworkSpec<FakeDeviceId>,
        &'static str,
        fn(
            &'static str,
            DualStackSendIpPacketMeta<FakeDeviceId>,
        ) -> Vec<(
            &'static str,
            DualStackSendIpPacketMeta<FakeDeviceId>,
            Option<core::time::Duration>,
        )>,
    >;

    fn new_test_net<I: TcpTestIpExt>() -> TcpTestNetwork {
        FakeTcpNetworkSpec::new_network(
            [
                (
                    LOCAL,
                    TcpCtx {
                        core_ctx: TcpCoreCtx::new::<I>(
                            I::TEST_ADDRS.local_ip,
                            I::TEST_ADDRS.remote_ip,
                        ),
                        bindings_ctx: TcpBindingsCtx::default(),
                    },
                ),
                (
                    REMOTE,
                    TcpCtx {
                        core_ctx: TcpCoreCtx::new::<I>(
                            I::TEST_ADDRS.remote_ip,
                            I::TEST_ADDRS.local_ip,
                        ),
                        bindings_ctx: TcpBindingsCtx::default(),
                    },
                ),
            ],
            move |net, meta: DualStackSendIpPacketMeta<_>| {
                if net == LOCAL {
                    alloc::vec![(REMOTE, meta, None)]
                } else {
                    alloc::vec![(LOCAL, meta, None)]
                }
            },
        )
    }

    /// Utilities for accessing locked internal state in tests.
    impl<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes> TcpSocketId<I, D, BT> {
        fn get(&self) -> impl Deref<Target = TcpSocketState<I, D, BT>> + '_ {
            let Self(rc) = self;
            rc.locked_state.read()
        }

        fn get_mut(&self) -> impl DerefMut<Target = TcpSocketState<I, D, BT>> + '_ {
            let Self(rc) = self;
            rc.locked_state.write()
        }
    }

    fn assert_this_stack_conn<
        'a,
        I: DualStackIpExt,
        BC: TcpBindingsContext<CC::DeviceId>,
        CC: TcpContext<I, BC>,
    >(
        conn: &'a I::ConnectionAndAddr<CC::WeakDeviceId, BC>,
        converter: &MaybeDualStack<CC::DualStackConverter, CC::SingleStackConverter>,
    ) -> &'a (
        Connection<I, I, CC::WeakDeviceId, BC>,
        ConnAddr<ConnIpAddr<I::Addr, NonZeroU16, NonZeroU16>, CC::WeakDeviceId>,
    ) {
        match converter {
            MaybeDualStack::NotDualStack(nds) => nds.convert(conn),
            MaybeDualStack::DualStack(ds) => {
                assert_matches!(ds.convert(conn), EitherStack::ThisStack(conn) => conn)
            }
        }
    }

    /// A trait providing a shortcut to instantiate a [`TcpApi`] from a context.
    pub(crate) trait TcpApiExt: ContextPair + Sized {
        fn tcp_api<I: Ip>(&mut self) -> TcpApi<I, &mut Self> {
            TcpApi::new(self)
        }
    }

    impl<O> TcpApiExt for O where O: ContextPair + Sized {}

    /// How to bind the client socket in `bind_listen_connect_accept_inner`.
    struct BindConfig {
        /// Which port to bind the client to.
        client_port: Option<NonZeroU16>,
        /// Which port to bind the server to.
        server_port: NonZeroU16,
        /// Whether to set REUSE_ADDR for the client.
        client_reuse_addr: bool,
        /// Whether to send bidirectional test data after establishing the
        /// connection.
        send_test_data: bool,
    }

    /// The following test sets up two connected testing context - one as the
    /// server and the other as the client. Tests if a connection can be
    /// established using `bind`, `listen`, `connect` and `accept`.
    ///
    /// # Arguments
    ///
    /// * `listen_addr` - The address to listen on.
    /// * `bind_config` - Specifics about how to bind the client socket.
    ///
    /// # Returns
    ///
    /// Returns a tuple of
    ///   - the created test network.
    ///   - the client socket from local.
    ///   - the send end of the client socket.
    ///   - the accepted socket from remote.
    fn bind_listen_connect_accept_inner<I: TcpTestIpExt>(
        listen_addr: I::Addr,
        BindConfig { client_port, server_port, client_reuse_addr, send_test_data }: BindConfig,
        seed: u128,
        drop_rate: f64,
    ) -> (
        TcpTestNetwork,
        TcpSocketId<I, FakeWeakDeviceId<FakeDeviceId>, TcpBindingsCtx<FakeDeviceId>>,
        Arc<Mutex<Vec<u8>>>,
        TcpSocketId<I, FakeWeakDeviceId<FakeDeviceId>, TcpBindingsCtx<FakeDeviceId>>,
    )
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>: TcpContext<
                I,
                TcpBindingsCtx<FakeDeviceId>,
                SingleStackConverter = I::SingleStackConverter,
                DualStackConverter = I::DualStackConverter,
            >,
    {
        let mut net = new_test_net::<I>();
        let mut rng = new_rng(seed);

        let mut maybe_drop_frame =
            |_: &mut TcpCtx<_>, meta: DualStackSendIpPacketMeta<_>, buffer: Buf<Vec<u8>>| {
                let x: f64 = rng.random();
                (x > drop_rate).then_some((meta, buffer))
            };

        let backlog = NonZeroUsize::new(1).unwrap();
        let server = net.with_context(REMOTE, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            let server = api.create(Default::default());
            api.bind(
                &server,
                SpecifiedAddr::new(listen_addr).map(|a| ZonedAddr::Unzoned(a)),
                Some(server_port),
            )
            .expect("failed to bind the server socket");
            api.listen(&server, backlog).expect("can listen");
            server
        });

        let client_ends = WriteBackClientBuffers::default();
        let client = net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            let socket = api.create(ProvidedBuffers::Buffers(client_ends.clone()));
            if client_reuse_addr {
                api.set_reuseaddr(&socket, true).expect("can set");
            }
            if let Some(port) = client_port {
                api.bind(&socket, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), Some(port))
                    .expect("failed to bind the client socket")
            }
            api.connect(&socket, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), server_port)
                .expect("failed to connect");
            socket
        });
        // If drop rate is 0, the SYN is guaranteed to be delivered, so we can
        // look at the SYN queue deterministically.
        if drop_rate == 0.0 {
            // Step once for the SYN packet to be sent.
            let _: StepResult = net.step();
            // The listener should create a pending socket.
            assert_matches!(
                &server.get().deref().socket_state,
                TcpSocketStateInner::Listener(Listener { accept_queue, .. }) => {
                    assert_eq!(accept_queue.ready_len(), 0);
                    assert_eq!(accept_queue.pending_len(), 1);
                }
            );
            // The handshake is not done, calling accept here should not succeed.
            net.with_context(REMOTE, |ctx| {
                let mut api = ctx.tcp_api::<I>();
                assert_matches!(api.accept(&server), Err(AcceptError::WouldBlock));
            });
        }

        // Step the test network until the handshake is done.
        net.run_until_idle_with(&mut maybe_drop_frame);
        let (accepted, addr, accepted_ends) = net.with_context(REMOTE, |ctx| {
            ctx.tcp_api::<I>().accept(&server).expect("failed to accept")
        });
        if let Some(port) = client_port {
            assert_eq!(
                addr,
                SocketAddr { ip: ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip), port: port }
            );
        } else {
            assert_eq!(addr.ip, ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip));
        }

        net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            assert_eq!(
                api.connect(
                    &client,
                    Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)),
                    server_port,
                ),
                Ok(())
            );
        });

        let assert_connected = |conn_id: &TcpSocketId<I, _, _>| {
            assert_matches!(
                &conn_id.get().deref().socket_state,
                TcpSocketStateInner::Connected { conn, .. } => {
                        let (conn, _addr) = assert_this_stack_conn::<I, _, TcpCoreCtx<_, _>>(conn, &I::converter());
                        assert_matches!(
                            conn,
                            Connection {
                                accept_queue: None,
                                state: State::Established(_),
                                ip_sock: _,
                                defunct: false,
                                soft_error: None,
                                handshake_status: HandshakeStatus::Completed { reported: true },
                            }
                        );
                    }
            )
        };

        assert_connected(&client);
        assert_connected(&accepted);

        let ClientBuffers { send: client_snd_end, receive: client_rcv_end } =
            client_ends.0.as_ref().lock().take().unwrap();
        let ClientBuffers { send: accepted_snd_end, receive: accepted_rcv_end } = accepted_ends;

        if send_test_data {
            for snd_end in [client_snd_end.clone(), accepted_snd_end] {
                snd_end.lock().extend_from_slice(b"Hello");
            }

            for (c, id) in [(LOCAL, &client), (REMOTE, &accepted)] {
                net.with_context(c, |ctx| ctx.tcp_api::<I>().do_send(id))
            }
            net.run_until_idle_with(&mut maybe_drop_frame);

            for rcv_end in [client_rcv_end, accepted_rcv_end] {
                assert_eq!(
                    rcv_end.lock().read_with(|avail| {
                        let avail = avail.concat();
                        assert_eq!(avail, b"Hello");
                        avail.len()
                    }),
                    5
                );
            }
        }

        // Check the listener is in correct state.
        assert_matches!(
            &server.get().deref().socket_state,
            TcpSocketStateInner::Listener(Listener {
                addr: _,
                backlog: actual_backlog,
                accept_queue: _,
                buffer_sizes
            }) => {
                assert_eq!(*actual_backlog, backlog);
                assert_eq!(*buffer_sizes, BufferSizes::default());
            }
        );

        net.with_context(REMOTE, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            assert_eq!(api.shutdown(&server, ShutdownType::Receive), Ok(false));
            api.close(server);
        });

        (net, client, client_snd_end, accepted)
    }

    #[test]
    fn test_socket_addr_display() {
        assert_eq!(
            format!(
                "{}",
                SocketAddr {
                    ip: maybe_zoned(
                        SpecifiedAddr::new(Ipv4Addr::new([192, 168, 0, 1]))
                            .expect("failed to create specified addr"),
                        &None::<usize>,
                    ),
                    port: NonZeroU16::new(1024).expect("failed to create NonZeroU16"),
                }
            ),
            String::from("192.168.0.1:1024"),
        );
        assert_eq!(
            format!(
                "{}",
                SocketAddr {
                    ip: maybe_zoned(
                        SpecifiedAddr::new(Ipv6Addr::new([0x2001, 0xDB8, 0, 0, 0, 0, 0, 1]))
                            .expect("failed to create specified addr"),
                        &None::<usize>,
                    ),
                    port: NonZeroU16::new(1024).expect("failed to create NonZeroU16"),
                }
            ),
            String::from("[2001:db8::1]:1024")
        );
        assert_eq!(
            format!(
                "{}",
                SocketAddr {
                    ip: maybe_zoned(
                        SpecifiedAddr::new(Ipv6Addr::new([0xFE80, 0, 0, 0, 0, 0, 0, 1]))
                            .expect("failed to create specified addr"),
                        &Some(42),
                    ),
                    port: NonZeroU16::new(1024).expect("failed to create NonZeroU16"),
                }
            ),
            String::from("[fe80::1%42]:1024")
        );
    }

    #[ip_test(I)]
    #[test_case(BindConfig { client_port: None, server_port: PORT_1, client_reuse_addr: false, send_test_data: true }, I::UNSPECIFIED_ADDRESS)]
    #[test_case(BindConfig { client_port: Some(PORT_1), server_port: PORT_1, client_reuse_addr: false, send_test_data: true }, I::UNSPECIFIED_ADDRESS)]
    #[test_case(BindConfig { client_port: None, server_port: PORT_1, client_reuse_addr: true, send_test_data: true }, I::UNSPECIFIED_ADDRESS)]
    #[test_case(BindConfig { client_port: Some(PORT_1), server_port: PORT_1, client_reuse_addr: true, send_test_data: true }, I::UNSPECIFIED_ADDRESS)]
    #[test_case(BindConfig { client_port: None, server_port: PORT_1, client_reuse_addr: false, send_test_data: true }, *<I as TestIpExt>::TEST_ADDRS.remote_ip)]
    #[test_case(BindConfig { client_port: Some(PORT_1), server_port: PORT_1, client_reuse_addr: false, send_test_data: true }, *<I as TestIpExt>::TEST_ADDRS.remote_ip)]
    #[test_case(BindConfig { client_port: None, server_port: PORT_1, client_reuse_addr: true, send_test_data: true }, *<I as TestIpExt>::TEST_ADDRS.remote_ip)]
    #[test_case(BindConfig { client_port: Some(PORT_1), server_port: PORT_1, client_reuse_addr: true, send_test_data: true }, *<I as TestIpExt>::TEST_ADDRS.remote_ip)]
    fn bind_listen_connect_accept<I: TcpTestIpExt>(bind_config: BindConfig, listen_addr: I::Addr)
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>: TcpContext<
                I,
                TcpBindingsCtx<FakeDeviceId>,
                SingleStackConverter = I::SingleStackConverter,
                DualStackConverter = I::DualStackConverter,
            >,
    {
        set_logger_for_test();
        let (mut net, client, _client_snd_end, accepted) =
            bind_listen_connect_accept_inner::<I>(listen_addr, bind_config, 0, 0.0);

        let mut assert_counters =
            |context_name: &'static str,
             socket: &TcpSocketId<I, _, _>,
             expected: CounterExpectations,
             expected_without_socket: CounterExpectationsWithoutSocket,
             expected_per_socket: CounterExpectations| {
                net.with_context(context_name, |ctx| {
                    let counters =
                        CounterContext::<TcpCountersWithSocket<I>>::counters(&ctx.core_ctx);
                    let counters_without_socket =
                        CounterContext::<TcpCountersWithoutSocket<I>>::counters(&ctx.core_ctx);
                    let counters_per_socket = ctx.core_ctx.per_resource_counters(socket);
                    assert_eq!(expected, counters.as_ref().cast(), "{context_name}");
                    assert_eq!(
                        expected_without_socket,
                        counters_without_socket.as_ref().cast(),
                        "{context_name}"
                    );
                    assert_eq!(
                        expected_per_socket,
                        counters_per_socket.as_ref().cast(),
                        "{context_name}"
                    )
                })
            };

        // Communication done by `bind_listen_connect_accept_inner`:
        //   LOCAL -> REMOTE: SYN to initiate the connection.
        //   LOCAL <- REMOTE: ACK the connection.
        //   LOCAL -> REMOTE: ACK the ACK.
        //   LOCAL -> REMOTE: Send "hello".
        //   LOCAL <- REMOTE: ACK "hello".
        //   LOCAL <- REMOTE: Send "hello".
        //   LOCAL -> REMOTE: ACK "hello".
        let local_with_socket_expects = || CounterExpectations {
            segments_sent: 4,
            received_segments_dispatched: 3,
            active_connection_openings: 1,
            syns_sent: 1,
            syns_received: 1,
            ..Default::default()
        };
        assert_counters(
            LOCAL,
            &client,
            local_with_socket_expects(),
            CounterExpectationsWithoutSocket { valid_segments_received: 3, ..Default::default() },
            // Note: The local side only has 1 socket, so the stack-wide and
            // per-socket expectations are identical.
            local_with_socket_expects(),
        );

        assert_counters(
            REMOTE,
            &accepted,
            CounterExpectations {
                segments_sent: 3,
                received_segments_dispatched: 4,
                passive_connection_openings: 1,
                syns_sent: 1,
                syns_received: 1,
                ..Default::default()
            },
            CounterExpectationsWithoutSocket { valid_segments_received: 4, ..Default::default() },
            // Note: The remote side has a listener socket and the accepted
            // socket. The stack-wide counters are higher than the accepted
            // socket's counters, because some events are attributed to the
            // listener.
            CounterExpectations {
                segments_sent: 2,
                received_segments_dispatched: 3,
                ..Default::default()
            },
        );
    }

    #[ip_test(I)]
    #[test_case(*<I as TestIpExt>::TEST_ADDRS.local_ip; "same addr")]
    #[test_case(I::UNSPECIFIED_ADDRESS; "any addr")]
    fn bind_conflict<I: TcpTestIpExt>(conflict_addr: I::Addr)
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        set_logger_for_test();
        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.local_ip,
        ));
        let mut api = ctx.tcp_api::<I>();
        let s1 = api.create(Default::default());
        let s2 = api.create(Default::default());

        api.bind(&s1, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), Some(PORT_1))
            .expect("first bind should succeed");
        assert_matches!(
            api.bind(&s2, SpecifiedAddr::new(conflict_addr).map(ZonedAddr::Unzoned), Some(PORT_1)),
            Err(BindError::LocalAddressError(LocalAddressError::AddressInUse))
        );
        api.bind(&s2, SpecifiedAddr::new(conflict_addr).map(ZonedAddr::Unzoned), Some(PORT_2))
            .expect("able to rebind to a free address");
    }

    #[ip_test(I)]
    #[test_case(NonZeroU16::new(u16::MAX).unwrap(), Ok(NonZeroU16::new(u16::MAX).unwrap()); "ephemeral available")]
    #[test_case(NonZeroU16::new(100).unwrap(), Err(LocalAddressError::FailedToAllocateLocalPort);
                "no ephemeral available")]
    fn bind_picked_port_all_others_taken<I: TcpTestIpExt>(
        available_port: NonZeroU16,
        expected_result: Result<NonZeroU16, LocalAddressError>,
    ) where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.local_ip,
        ));
        let mut api = ctx.tcp_api::<I>();
        for port in 1..=u16::MAX {
            let port = NonZeroU16::new(port).unwrap();
            if port == available_port {
                continue;
            }
            let socket = api.create(Default::default());

            api.bind(&socket, None, Some(port)).expect("uncontested bind");
            api.listen(&socket, NonZeroUsize::new(1).unwrap()).expect("can listen");
        }

        // Now that all but the LOCAL_PORT are occupied, ask the stack to
        // select a port.
        let socket = api.create(Default::default());
        let result = api.bind(&socket, None, None).map(|()| {
            assert_matches!(
                api.get_info(&socket),
                SocketInfo::Bound(bound) => bound.port
            )
        });
        assert_eq!(result, expected_result.map_err(From::from));

        // Now close the socket and try a connect call to ourselves on the
        // available port. Self-connection protection should always prevent us
        // from doing that even when the port is in the ephemeral range.
        api.close(socket);
        let socket = api.create(Default::default());
        let result =
            api.connect(&socket, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), available_port);
        assert_eq!(result, Err(ConnectError::NoPort));
    }

    #[ip_test(I)]
    fn bind_to_non_existent_address<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<I>();
        let unbound = api.create(Default::default());
        assert_matches!(
            api.bind(&unbound, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), None),
            Err(BindError::LocalAddressError(LocalAddressError::AddressMismatch))
        );

        assert_matches!(unbound.get().deref().socket_state, TcpSocketStateInner::Unbound(_));
    }

    #[test]
    fn bind_addr_requires_zone() {
        let local_ip = LinkLocalAddr::new(net_ip_v6!("fe80::1")).unwrap().into_specified();

        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<Ipv6>(
            Ipv6::TEST_ADDRS.local_ip,
            Ipv6::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<Ipv6>();
        let unbound = api.create(Default::default());
        assert_matches!(
            api.bind(&unbound, Some(ZonedAddr::Unzoned(local_ip)), None),
            Err(BindError::LocalAddressError(LocalAddressError::Zone(
                ZonedAddressError::RequiredZoneNotProvided
            )))
        );

        assert_matches!(unbound.get().deref().socket_state, TcpSocketStateInner::Unbound(_));
    }

    #[test]
    fn connect_bound_requires_zone() {
        let ll_ip = LinkLocalAddr::new(net_ip_v6!("fe80::1")).unwrap().into_specified();

        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<Ipv6>(
            Ipv6::TEST_ADDRS.local_ip,
            Ipv6::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<Ipv6>();
        let socket = api.create(Default::default());
        api.bind(&socket, None, None).expect("bind succeeds");
        assert_matches!(
            api.connect(&socket, Some(ZonedAddr::Unzoned(ll_ip)), PORT_1,),
            Err(ConnectError::Zone(ZonedAddressError::RequiredZoneNotProvided))
        );

        assert_matches!(
            socket.get().deref().socket_state,
            TcpSocketStateInner::Bound { .. } | TcpSocketStateInner::Connected { .. }
        );
    }

    // This is a regression test for https://fxbug.dev/361402347.
    #[ip_test(I)]
    fn bind_listen_on_same_port_different_addrs<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        set_logger_for_test();

        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::with_ip_socket_ctx_state(
            FakeDualStackIpSocketCtx::new(core::iter::once(FakeDeviceConfig {
                device: FakeDeviceId,
                local_ips: vec![I::TEST_ADDRS.local_ip, I::TEST_ADDRS.remote_ip],
                remote_ips: vec![],
            })),
        ));
        let mut api = ctx.tcp_api::<I>();

        let s1 = api.create(Default::default());
        api.bind(&s1, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), Some(PORT_1)).unwrap();
        api.listen(&s1, NonZeroUsize::MIN).unwrap();

        let s2 = api.create(Default::default());
        api.bind(&s2, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), Some(PORT_1)).unwrap();
        api.listen(&s2, NonZeroUsize::MIN).unwrap();
    }

    #[ip_test(I)]
    #[test_case(None, None; "both any addr")]
    #[test_case(None, Some(<I as TestIpExt>::TEST_ADDRS.local_ip); "any then specified")]
    #[test_case(Some(<I as TestIpExt>::TEST_ADDRS.local_ip), None; "specified then any")]
    #[test_case(
        Some(<I as TestIpExt>::TEST_ADDRS.local_ip),
        Some(<I as TestIpExt>::TEST_ADDRS.local_ip);
        "both specified"
    )]
    fn cannot_listen_on_same_port_with_shadowed_address<I: TcpTestIpExt>(
        first: Option<SpecifiedAddr<I::Addr>>,
        second: Option<SpecifiedAddr<I::Addr>>,
    ) where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        set_logger_for_test();

        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::with_ip_socket_ctx_state(
            FakeDualStackIpSocketCtx::new(core::iter::once(FakeDeviceConfig {
                device: FakeDeviceId,
                local_ips: vec![I::TEST_ADDRS.local_ip],
                remote_ips: vec![],
            })),
        ));
        let mut api = ctx.tcp_api::<I>();

        let s1 = api.create(Default::default());
        api.set_reuseaddr(&s1, true).unwrap();
        api.bind(&s1, first.map(ZonedAddr::Unzoned), Some(PORT_1)).unwrap();

        let s2 = api.create(Default::default());
        api.set_reuseaddr(&s2, true).unwrap();
        api.bind(&s2, second.map(ZonedAddr::Unzoned), Some(PORT_1)).unwrap();

        api.listen(&s1, NonZeroUsize::MIN).unwrap();
        assert_eq!(api.listen(&s2, NonZeroUsize::MIN), Err(ListenError::ListenerExists));
    }

    #[test]
    fn connect_unbound_picks_link_local_source_addr() {
        set_logger_for_test();
        let client_ip = SpecifiedAddr::new(net_ip_v6!("fe80::1")).unwrap();
        let server_ip = SpecifiedAddr::new(net_ip_v6!("1:2:3:4::")).unwrap();
        let mut net = FakeTcpNetworkSpec::new_network(
            [
                (LOCAL, TcpCtx::with_core_ctx(TcpCoreCtx::new::<Ipv6>(client_ip, server_ip))),
                (REMOTE, TcpCtx::with_core_ctx(TcpCoreCtx::new::<Ipv6>(server_ip, client_ip))),
            ],
            |net, meta| {
                if net == LOCAL {
                    alloc::vec![(REMOTE, meta, None)]
                } else {
                    alloc::vec![(LOCAL, meta, None)]
                }
            },
        );
        const PORT: NonZeroU16 = NonZeroU16::new(100).unwrap();
        let client_connection = net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api();
            let socket: TcpSocketId<Ipv6, _, _> = api.create(Default::default());
            api.connect(&socket, Some(ZonedAddr::Unzoned(server_ip)), PORT).expect("can connect");
            socket
        });
        net.with_context(REMOTE, |ctx| {
            let mut api = ctx.tcp_api::<Ipv6>();
            let socket = api.create(Default::default());
            api.bind(&socket, None, Some(PORT)).expect("failed to bind the client socket");
            let _listener = api.listen(&socket, NonZeroUsize::MIN).expect("can listen");
        });

        // Advance until the connection is established.
        net.run_until_idle();

        net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api();
            assert_eq!(
                api.connect(&client_connection, Some(ZonedAddr::Unzoned(server_ip)), PORT),
                Ok(())
            );

            let info = assert_matches!(
                api.get_info(&client_connection),
                SocketInfo::Connection(info) => info
            );
            // The local address picked for the connection is link-local, which
            // means the device for the connection must also be set (since the
            // address requires a zone).
            let (local_ip, remote_ip) = assert_matches!(
                info,
                ConnectionInfo {
                    local_addr: SocketAddr { ip: local_ip, port: _ },
                    remote_addr: SocketAddr { ip: remote_ip, port: PORT },
                    device: Some(FakeWeakDeviceId(FakeDeviceId))
                } => (local_ip, remote_ip)
            );
            assert_eq!(
                local_ip,
                ZonedAddr::Zoned(
                    AddrAndZone::new(client_ip, FakeWeakDeviceId(FakeDeviceId)).unwrap()
                )
            );
            assert_eq!(remote_ip, ZonedAddr::Unzoned(server_ip));

            // Double-check that the bound device can't be changed after being set
            // implicitly.
            assert_matches!(
                api.set_device(&client_connection, None),
                Err(SetDeviceError::ZoneChange)
            );
        });
    }

    #[test]
    fn accept_connect_picks_link_local_addr() {
        set_logger_for_test();
        let server_ip = SpecifiedAddr::new(net_ip_v6!("fe80::1")).unwrap();
        let client_ip = SpecifiedAddr::new(net_ip_v6!("1:2:3:4::")).unwrap();
        let mut net = FakeTcpNetworkSpec::new_network(
            [
                (LOCAL, TcpCtx::with_core_ctx(TcpCoreCtx::new::<Ipv6>(server_ip, client_ip))),
                (REMOTE, TcpCtx::with_core_ctx(TcpCoreCtx::new::<Ipv6>(client_ip, server_ip))),
            ],
            |net, meta| {
                if net == LOCAL {
                    alloc::vec![(REMOTE, meta, None)]
                } else {
                    alloc::vec![(LOCAL, meta, None)]
                }
            },
        );
        const PORT: NonZeroU16 = NonZeroU16::new(100).unwrap();
        let server_listener = net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api::<Ipv6>();
            let socket: TcpSocketId<Ipv6, _, _> = api.create(Default::default());
            api.bind(&socket, None, Some(PORT)).expect("failed to bind the client socket");
            api.listen(&socket, NonZeroUsize::MIN).expect("can listen");
            socket
        });
        let client_connection = net.with_context(REMOTE, |ctx| {
            let mut api = ctx.tcp_api::<Ipv6>();
            let socket = api.create(Default::default());
            api.connect(
                &socket,
                Some(ZonedAddr::Zoned(AddrAndZone::new(server_ip, FakeDeviceId).unwrap())),
                PORT,
            )
            .expect("failed to open a connection");
            socket
        });

        // Advance until the connection is established.
        net.run_until_idle();

        net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api();
            let (server_connection, _addr, _buffers) =
                api.accept(&server_listener).expect("connection is waiting");

            let info = assert_matches!(
                api.get_info(&server_connection),
                SocketInfo::Connection(info) => info
            );
            // The local address picked for the connection is link-local, which
            // means the device for the connection must also be set (since the
            // address requires a zone).
            let (local_ip, remote_ip) = assert_matches!(
                info,
                ConnectionInfo {
                    local_addr: SocketAddr { ip: local_ip, port: PORT },
                    remote_addr: SocketAddr { ip: remote_ip, port: _ },
                    device: Some(FakeWeakDeviceId(FakeDeviceId))
                } => (local_ip, remote_ip)
            );
            assert_eq!(
                local_ip,
                ZonedAddr::Zoned(
                    AddrAndZone::new(server_ip, FakeWeakDeviceId(FakeDeviceId)).unwrap()
                )
            );
            assert_eq!(remote_ip, ZonedAddr::Unzoned(client_ip));

            // Double-check that the bound device can't be changed after being set
            // implicitly.
            assert_matches!(
                api.set_device(&server_connection, None),
                Err(SetDeviceError::ZoneChange)
            );
        });
        net.with_context(REMOTE, |ctx| {
            assert_eq!(
                ctx.tcp_api().connect(
                    &client_connection,
                    Some(ZonedAddr::Zoned(AddrAndZone::new(server_ip, FakeDeviceId).unwrap())),
                    PORT,
                ),
                Ok(())
            );
        });
    }

    // The test verifies that if client tries to connect to a closed port on
    // server, the connection is aborted and RST is received.
    #[ip_test(I)]
    fn connect_reset<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>: TcpContext<
                I,
                TcpBindingsCtx<FakeDeviceId>,
                SingleStackConverter = I::SingleStackConverter,
                DualStackConverter = I::DualStackConverter,
            >,
    {
        set_logger_for_test();
        let mut net = new_test_net::<I>();

        let client = net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            let conn = api.create(Default::default());
            api.bind(&conn, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), Some(PORT_1))
                .expect("failed to bind the client socket");
            api.connect(&conn, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), PORT_1)
                .expect("failed to connect");
            conn
        });

        // Step one time for SYN packet to be delivered.
        let _: StepResult = net.step();
        // Assert that we got a RST back.
        net.collect_frames();
        assert_matches!(
            &net.iter_pending_frames().collect::<Vec<_>>()[..],
            [InstantAndData(_instant, PendingFrameData {
                dst_context: _,
                meta,
                frame,
            })] => {
            let mut buffer = Buf::new(frame, ..);
            match I::VERSION {
                IpVersion::V4 => {
                    let meta = assert_matches!(meta, DualStackSendIpPacketMeta::V4(v4) => v4);
                    let parsed = buffer.parse_with::<_, TcpSegment<_>>(
                        TcpParseArgs::new(*meta.src_ip, *meta.dst_ip)
                    ).expect("failed to parse");
                    assert!(parsed.rst())
                }
                IpVersion::V6 => {
                    let meta = assert_matches!(meta, DualStackSendIpPacketMeta::V6(v6) => v6);
                    let parsed = buffer.parse_with::<_, TcpSegment<_>>(
                        TcpParseArgs::new(*meta.src_ip, *meta.dst_ip)
                    ).expect("failed to parse");
                    assert!(parsed.rst())
                }
            }
        });

        net.run_until_idle();
        // Finally, the connection should be reset and bindings should have been
        // signaled.
        assert_matches!(
            &client.get().deref().socket_state,
            TcpSocketStateInner::Connected { conn, .. } => {
                let (conn, _addr) = assert_this_stack_conn::<I, _, TcpCoreCtx<_, _>>(conn, &I::converter());
                assert_matches!(
                    conn,
                    Connection {
                    accept_queue: None,
                    state: State::Closed(Closed {
                        reason: Some(ConnectionError::ConnectionRefused)
                    }),
                    ip_sock: _,
                    defunct: false,
                    soft_error: None,
                    handshake_status: HandshakeStatus::Aborted,
                    }
                );
            }
        );
        net.with_context(LOCAL, |ctx| {
            assert_matches!(
                ctx.tcp_api().connect(
                    &client,
                    Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)),
                    PORT_1
                ),
                Err(ConnectError::ConnectionError(ConnectionError::ConnectionRefused))
            );
            // Connect already yielded the error.
            assert_eq!(ctx.tcp_api().get_socket_error(&client), None);
        });
    }

    #[ip_test(I)]
    fn retransmission<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>: TcpContext<
                I,
                TcpBindingsCtx<FakeDeviceId>,
                SingleStackConverter = I::SingleStackConverter,
                DualStackConverter = I::DualStackConverter,
            >,
    {
        set_logger_for_test();
        run_with_many_seeds(|seed| {
            let (_net, _client, _client_snd_end, _accepted) = bind_listen_connect_accept_inner::<I>(
                I::UNSPECIFIED_ADDRESS,
                BindConfig {
                    client_port: None,
                    server_port: PORT_1,
                    client_reuse_addr: false,
                    send_test_data: true,
                },
                seed,
                0.2,
            );
        });
    }

    const LOCAL_PORT: NonZeroU16 = NonZeroU16::new(1845).unwrap();

    #[ip_test(I)]
    fn listener_with_bound_device_conflict<I: TcpTestIpExt>()
    where
        TcpCoreCtx<MultipleDevicesId, TcpBindingsCtx<MultipleDevicesId>>:
            TcpContext<I, TcpBindingsCtx<MultipleDevicesId>>,
    {
        set_logger_for_test();
        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new_multiple_devices());
        let mut api = ctx.tcp_api::<I>();
        let sock_a = api.create(Default::default());
        assert_matches!(api.set_device(&sock_a, Some(MultipleDevicesId::A),), Ok(()));
        api.bind(&sock_a, None, Some(LOCAL_PORT)).expect("bind should succeed");
        api.listen(&sock_a, NonZeroUsize::new(10).unwrap()).expect("can listen");

        let socket = api.create(Default::default());
        // Binding `socket` to the unspecified address should fail since the
        // address is shadowed by `sock_a`.
        assert_matches!(
            api.bind(&socket, None, Some(LOCAL_PORT)),
            Err(BindError::LocalAddressError(LocalAddressError::AddressInUse))
        );

        assert_matches!(api.set_device(&socket, Some(MultipleDevicesId::B),), Ok(()));
        api.bind(&socket, None, Some(LOCAL_PORT)).expect("no conflict");
    }

    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    enum TestIp {
        A,
        B,
    }

    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    enum TestSocketConfig {
        AnyIp,
        AnyIpWithDev(MultipleDevicesId),
        IpOnly(TestIp),
        IpWithDev(MultipleDevicesId, TestIp),
    }

    impl TestSocketConfig {
        fn create<'a, I: TcpTestIpExt>(
            self,
            api: &mut TcpApi<I, &'a mut TcpCtx<MultipleDevicesId>>,
            reuseaddr: bool,
        ) -> TcpApiSocketId<I, &'a mut TcpCtx<MultipleDevicesId>>
        where
            TcpCoreCtx<MultipleDevicesId, TcpBindingsCtx<MultipleDevicesId>>:
                TcpContext<I, TcpBindingsCtx<MultipleDevicesId>>,
        {
            let socket = api.create(Default::default());
            if reuseaddr {
                api.set_reuseaddr(&socket, true).unwrap();
            }
            match self {
                TestSocketConfig::AnyIp | TestSocketConfig::IpOnly(_) => {}
                TestSocketConfig::AnyIpWithDev(device) | TestSocketConfig::IpWithDev(device, _) => {
                    assert_matches!(api.set_device(&socket, Some(device)), Ok(()));
                }
            }
            socket
        }

        fn get_bind_addr<I: TcpTestIpExt>(
            self,
        ) -> Option<ZonedAddr<SpecifiedAddr<I::Addr>, MultipleDevicesId>> {
            match self {
                TestSocketConfig::AnyIp | TestSocketConfig::AnyIpWithDev(_) => None,
                TestSocketConfig::IpOnly(ip) | TestSocketConfig::IpWithDev(_, ip) => {
                    let addr = match ip {
                        TestIp::A => I::TEST_ADDRS.local_ip,
                        TestIp::B => I::TEST_ADDRS.remote_ip,
                    };
                    Some(ZonedAddr::Unzoned(addr))
                }
            }
        }

        fn create_and_bind<'a, I: TcpTestIpExt>(
            self,
            api: &mut TcpApi<I, &'a mut TcpCtx<MultipleDevicesId>>,
            port: NonZeroU16,
            reuseaddr: bool,
        ) -> TcpApiSocketId<I, &'a mut TcpCtx<MultipleDevicesId>>
        where
            TcpCoreCtx<MultipleDevicesId, TcpBindingsCtx<MultipleDevicesId>>:
                TcpContext<I, TcpBindingsCtx<MultipleDevicesId>>,
        {
            let socket = self.create(api, reuseaddr);
            api.bind(&socket, self.get_bind_addr::<I>(), Some(port)).expect("bind should succeed");
            socket
        }
    }

    #[ip_test(I)]
    #[test_case(TestSocketConfig::AnyIp,
                TestSocketConfig::AnyIp,
                true; "both_wildcard")]
    #[test_case(TestSocketConfig::AnyIp,
                TestSocketConfig::AnyIpWithDev(MultipleDevicesId::A),
                true; "wildcard_then_wildcard_dev_a")]
    #[test_case(TestSocketConfig::AnyIp,
                TestSocketConfig::IpOnly(TestIp::A),
                true; "wildcard_vs_specific")]
    #[test_case(TestSocketConfig::AnyIp,
                TestSocketConfig::IpWithDev(MultipleDevicesId::A, TestIp::A),
                true; "wildcard_then_specific_dev_a")]
    #[test_case(TestSocketConfig::AnyIpWithDev(MultipleDevicesId::A),
                TestSocketConfig::AnyIp,
                true; "wildcard_dev_a_then_wildcard")]
    #[test_case(TestSocketConfig::AnyIpWithDev(MultipleDevicesId::A),
                TestSocketConfig::IpOnly(TestIp::A),
                true; "any_dev_a_then_ip_only")]
    #[test_case(TestSocketConfig::IpOnly(TestIp::A),
                TestSocketConfig::AnyIpWithDev(MultipleDevicesId::A),
                true; "ip_only_then_any_dev_a")]
    #[test_case(TestSocketConfig::IpOnly(TestIp::A),
                TestSocketConfig::IpOnly(TestIp::A),
                true; "same_ip")]
    #[test_case(TestSocketConfig::IpWithDev(MultipleDevicesId::A, TestIp::A),
                TestSocketConfig::AnyIp,
                true; "specific_dev_a_then_wildcard")]
    #[test_case(TestSocketConfig::IpOnly(TestIp::A),
                TestSocketConfig::AnyIp,
                true; "specific_vs_wildcard_diff_ip")]
    // Non-conflicting cases (different devices):
    #[test_case(TestSocketConfig::AnyIpWithDev(MultipleDevicesId::A),
                TestSocketConfig::AnyIpWithDev(MultipleDevicesId::B),
                false; "different_devices_wildcard")]
    #[test_case(TestSocketConfig::IpWithDev(MultipleDevicesId::A, TestIp::A),
                TestSocketConfig::IpWithDev(MultipleDevicesId::B, TestIp::A),
                false; "different_devices_specific")]
    // Non-conflicting cases (different IPs):
    #[test_case(TestSocketConfig::IpOnly(TestIp::A),
                TestSocketConfig::IpOnly(TestIp::B),
                false; "different_ips")]
    #[test_case(TestSocketConfig::IpWithDev(MultipleDevicesId::A, TestIp::A),
                TestSocketConfig::IpWithDev(MultipleDevicesId::A, TestIp::B),
                false; "different_ips_same_device")]
    #[test_case(TestSocketConfig::IpWithDev(MultipleDevicesId::A, TestIp::A),
                TestSocketConfig::IpOnly(TestIp::B),
                false; "different_ips_overlapping_device")]
    fn test_address_conflict_detection<I: TcpTestIpExt>(
        sock_a_config: TestSocketConfig,
        sock_b_config: TestSocketConfig,
        have_conflict: bool,
    ) where
        TcpCoreCtx<MultipleDevicesId, TcpBindingsCtx<MultipleDevicesId>>:
            TcpContext<I, TcpBindingsCtx<MultipleDevicesId>>,
    {
        set_logger_for_test();

        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new_multiple_devices());
        let mut api = ctx.tcp_api::<I>();

        // Test conflict detection on bind.
        {
            let sock_a = sock_a_config.create_and_bind(&mut api, LOCAL_PORT, false);

            let sock_b = sock_b_config.create(&mut api, false);
            let res = api.bind(&sock_b, sock_b_config.get_bind_addr::<I>(), Some(LOCAL_PORT));
            if have_conflict {
                assert_matches!(
                    res,
                    Err(BindError::LocalAddressError(LocalAddressError::AddressInUse))
                );
            } else {
                res.expect("bind should succeed");
            }

            api.close(sock_a);
            api.close(sock_b);
        }

        // Test conflict detection when the first socket is listening and the second socket
        // tries to bind.
        {
            let sock_a = sock_a_config.create_and_bind(&mut api, LOCAL_PORT, false);
            api.listen(&sock_a, NonZeroUsize::new(10).unwrap()).expect("listen should succeed");

            let sock_b = sock_b_config.create(&mut api, false);
            let res = api.bind(&sock_b, sock_b_config.get_bind_addr::<I>(), Some(LOCAL_PORT));
            if have_conflict {
                assert_matches!(
                    res,
                    Err(BindError::LocalAddressError(LocalAddressError::AddressInUse))
                );
            } else {
                res.expect("bind should succeed");
            }

            api.close(sock_a);
            api.close(sock_b);
        }

        // Verify that SO_REUSEADDR does not allow binding to an address that is
        // actively listened on by an overlapping socket.
        {
            let sock_a = sock_a_config.create_and_bind(&mut api, LOCAL_PORT, true);
            api.listen(&sock_a, NonZeroUsize::new(10).unwrap()).expect("listen should succeed");

            let sock_b = sock_b_config.create(&mut api, true);
            let res = api.bind(&sock_b, sock_b_config.get_bind_addr::<I>(), Some(LOCAL_PORT));
            if have_conflict {
                assert_matches!(
                    res,
                    Err(BindError::LocalAddressError(LocalAddressError::AddressInUse))
                );
            } else {
                res.expect("bind should succeed");
            }

            api.close(sock_a);
            api.close(sock_b);
        }

        // Test conflict detection for listening sockets when both sockets are
        // bound with SO_REUSEADDR, and then we try to listen. First listen will
        // succeed, but the second is expected to fail when there is a conflict.
        {
            let sock_a = sock_a_config.create_and_bind(&mut api, LOCAL_PORT, true);
            let sock_b = sock_b_config.create_and_bind(&mut api, LOCAL_PORT, true);

            // First listen succeeds.
            api.listen(&sock_a, NonZeroUsize::new(10).unwrap())
                .expect("first listen should succeed");

            // Second listen.
            let res = api.listen(&sock_b, NonZeroUsize::new(10).unwrap());
            if have_conflict {
                assert_matches!(res, Err(ListenError::ListenerExists));
            } else {
                res.expect("second listen should succeed");
            }

            api.close(sock_a);
            api.close(sock_b);
        }

        // Verify that resetting SO_REUSEADDR (setting it to false) is only
        // allowed if it would not result in a conflict with other existing
        // sockets.
        {
            let sock_a = sock_a_config.create_and_bind(&mut api, LOCAL_PORT, true);
            let sock_b = sock_b_config.create_and_bind(&mut api, LOCAL_PORT, true);

            // Try to reset SO_REUSEADDR on sock_a.
            let res_a = api.set_reuseaddr(&sock_a, false);
            if have_conflict {
                assert_matches!(res_a, Err(SetReuseAddrError::AddrInUse));
            } else {
                res_a.expect("reset sock_a should succeed");
            }

            // Try to reset SO_REUSEADDR on sock_b.
            let res_b = api.set_reuseaddr(&sock_b, false);
            if have_conflict {
                assert_matches!(res_b, Err(SetReuseAddrError::AddrInUse));
            } else {
                res_b.expect("reset sock_b should succeed");
            }

            api.close(sock_a);
            api.close(sock_b);
        }
    }

    #[test_case(None)]
    #[test_case(Some(MultipleDevicesId::B); "other")]
    fn set_bound_device_listener_on_zoned_addr(set_device: Option<MultipleDevicesId>) {
        set_logger_for_test();
        let ll_addr = LinkLocalAddr::new(Ipv6::LINK_LOCAL_UNICAST_SUBNET.network()).unwrap();

        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::with_ip_socket_ctx_state(
            FakeDualStackIpSocketCtx::new(MultipleDevicesId::all().into_iter().map(|device| {
                FakeDeviceConfig {
                    device,
                    local_ips: vec![ll_addr.into_specified()],
                    remote_ips: vec![ll_addr.into_specified()],
                }
            })),
        ));
        let mut api = ctx.tcp_api::<Ipv6>();
        let socket = api.create(Default::default());
        api.bind(
            &socket,
            Some(ZonedAddr::Zoned(
                AddrAndZone::new(ll_addr.into_specified(), MultipleDevicesId::A).unwrap(),
            )),
            Some(LOCAL_PORT),
        )
        .expect("bind should succeed");

        assert_matches!(api.set_device(&socket, set_device), Err(SetDeviceError::ZoneChange));
    }

    #[test_case(None)]
    #[test_case(Some(MultipleDevicesId::B); "other")]
    fn set_bound_device_connected_to_zoned_addr(set_device: Option<MultipleDevicesId>) {
        set_logger_for_test();
        let ll_addr = LinkLocalAddr::new(Ipv6::LINK_LOCAL_UNICAST_SUBNET.network()).unwrap();

        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::with_ip_socket_ctx_state(
            FakeDualStackIpSocketCtx::new(MultipleDevicesId::all().into_iter().map(|device| {
                FakeDeviceConfig {
                    device,
                    local_ips: vec![ll_addr.into_specified()],
                    remote_ips: vec![ll_addr.into_specified()],
                }
            })),
        ));
        let mut api = ctx.tcp_api::<Ipv6>();
        let socket = api.create(Default::default());
        api.connect(
            &socket,
            Some(ZonedAddr::Zoned(
                AddrAndZone::new(ll_addr.into_specified(), MultipleDevicesId::A).unwrap(),
            )),
            LOCAL_PORT,
        )
        .expect("connect should succeed");

        assert_matches!(api.set_device(&socket, set_device), Err(SetDeviceError::ZoneChange));
    }

    // Regression test for https://fxbug.dev/388656903.
    #[ip_test(I)]
    fn set_bound_to_device_after_connect_fails<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        set_logger_for_test();
        let mut net = new_test_net::<I>();
        let socket = net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            let socket = api.create(Default::default());
            api.connect(&socket, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), PORT_1)
                .expect("bind should succeed");
            socket
        });

        net.run_until_idle();

        net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            assert_matches!(api.set_device(&socket, Some(FakeDeviceId)), Ok(()));
            let ConnectionInfo { local_addr: _, remote_addr, device } =
                assert_matches!(api.get_info(&socket), SocketInfo::Connection(c) => c);
            assert_eq!(
                remote_addr,
                SocketAddr { ip: ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip), port: PORT_1 }
            );
            assert_eq!(device, Some(FakeWeakDeviceId(FakeDeviceId)));
            api.close(socket);
        });
    }

    #[test]
    fn set_device_dual_stack_listener() {
        set_logger_for_test();
        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new_multiple_devices());
        let mut api = ctx.tcp_api::<Ipv6>();
        let socket = api.create(Default::default());
        api.bind(&socket, None, Some(LOCAL_PORT)).expect("bind should succeed");
        assert_matches!(api.set_device(&socket, Some(MultipleDevicesId::A)), Ok(()));
        let info = api.get_info(&socket);
        let device = assert_matches!(info, SocketInfo::Bound(BoundInfo { device, .. }) => device);
        assert_eq!(device, Some(MultipleDevicesId::A.downgrade()));
    }

    #[ip_test(I)]
    fn set_device_unbound_connect<I: TcpTestIpExt>()
    where
        TcpCoreCtx<MultipleDevicesId, TcpBindingsCtx<MultipleDevicesId>>:
            TcpContext<I, TcpBindingsCtx<MultipleDevicesId>>,
    {
        set_logger_for_test();
        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new_multiple_devices());
        let mut api = ctx.tcp_api::<I>();
        let socket = api.create(Default::default());
        assert_matches!(api.set_device(&socket, Some(MultipleDevicesId::A)), Ok(()));
        api.connect(&socket, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), PORT_1)
            .expect("connect should succeed");
        let info = api.get_info(&socket);
        let device =
            assert_matches!(info, SocketInfo::Connection(ConnectionInfo { device, .. }) => device);
        assert_eq!(device, Some(MultipleDevicesId::A.downgrade()));

        // Connecting with device C (which has no IP addresses or routes) should fail.
        let socket_c = api.create(Default::default());
        assert_matches!(api.set_device(&socket_c, Some(MultipleDevicesId::C)), Ok(()));
        assert_matches!(
            api.connect(&socket_c, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), PORT_1),
            Err(ConnectError::NoRoute)
        );
    }

    #[test]
    fn set_device_unbound_dual_stack_connect() {
        set_logger_for_test();
        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new_multiple_devices());
        let mut api = ctx.tcp_api::<Ipv6>();
        let socket = api.create(Default::default());
        assert_matches!(api.set_device(&socket, Some(MultipleDevicesId::A)), Ok(()));
        api.connect(
            &socket,
            Some(ZonedAddr::Unzoned((*Ipv4::TEST_ADDRS.remote_ip).to_ipv6_mapped())),
            PORT_1,
        )
        .expect("connect should succeed");
        let info = api.get_info(&socket);
        let device =
            assert_matches!(info, SocketInfo::Connection(ConnectionInfo { device, .. }) => device);
        assert_eq!(device, Some(MultipleDevicesId::A.downgrade()));

        // Connecting with device C (which has no IP addresses or routes) should fail.
        let socket_c = api.create(Default::default());
        assert_matches!(api.set_device(&socket_c, Some(MultipleDevicesId::C)), Ok(()));
        assert_matches!(
            api.connect(
                &socket_c,
                Some(ZonedAddr::Unzoned((*Ipv4::TEST_ADDRS.remote_ip).to_ipv6_mapped())),
                PORT_1,
            ),
            Err(ConnectError::NoRoute)
        );
    }

    #[ip_test(I)]
    #[test_case(*<I as TestIpExt>::TEST_ADDRS.local_ip, true; "specified bound")]
    #[test_case(I::UNSPECIFIED_ADDRESS, true; "unspecified bound")]
    #[test_case(*<I as TestIpExt>::TEST_ADDRS.local_ip, false; "specified listener")]
    #[test_case(I::UNSPECIFIED_ADDRESS, false; "unspecified listener")]
    fn bound_socket_info<I: TcpTestIpExt>(ip_addr: I::Addr, listen: bool)
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<I>();
        let socket = api.create(Default::default());

        let (addr, port) = (SpecifiedAddr::new(ip_addr).map(ZonedAddr::Unzoned), PORT_1);

        api.bind(&socket, addr, Some(port)).expect("bind should succeed");
        if listen {
            api.listen(&socket, NonZeroUsize::new(25).unwrap()).expect("can listen");
        }
        let info = api.get_info(&socket);
        assert_eq!(
            info,
            SocketInfo::Bound(BoundInfo {
                addr: addr.map(|a| a.map_zone(FakeWeakDeviceId)),
                port,
                device: None
            })
        );
    }

    #[ip_test(I)]
    fn connection_info<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<I>();
        let local = SocketAddr { ip: ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip), port: PORT_1 };
        let remote = SocketAddr { ip: ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip), port: PORT_2 };

        let socket = api.create(Default::default());
        api.bind(&socket, Some(local.ip), Some(local.port)).expect("bind should succeed");

        api.connect(&socket, Some(remote.ip), remote.port).expect("connect should succeed");

        assert_eq!(
            api.get_info(&socket),
            SocketInfo::Connection(ConnectionInfo {
                local_addr: local.map_zone(FakeWeakDeviceId),
                remote_addr: remote.map_zone(FakeWeakDeviceId),
                device: None,
            }),
        );
    }

    #[test_case(Ipv6::get_multicast_addr(1).into(), PhantomData::<Ipv6>)]
    #[test_case(Ipv4::get_multicast_addr(1).into(), PhantomData::<Ipv4>)]
    #[test_case(Ipv4::LIMITED_BROADCAST_ADDRESS, PhantomData::<Ipv4>)]
    fn non_unicast_ip_bind<I>(local_ip: SpecifiedAddr<I::Addr>, _ip: PhantomData<I>)
    where
        I: TcpTestIpExt + Ip,
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        let mut ctx =
            TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(local_ip, I::TEST_ADDRS.remote_ip));
        let mut api = ctx.tcp_api::<I>();
        let local = SocketAddr { ip: ZonedAddr::Unzoned(local_ip), port: PORT_1 };
        let socket = api.create(Default::default());

        assert_eq!(
            api.bind(&socket, Some(local.ip), Some(local.port))
                .expect_err("bind should fail for non-unicast address"),
            BindError::LocalAddressError(LocalAddressError::CannotBindToAddress)
        );
    }

    #[test_case(Ipv6::get_multicast_addr(1).into(), PhantomData::<Ipv6>)]
    #[test_case(Ipv4::get_multicast_addr(1).into(), PhantomData::<Ipv4>)]
    #[test_case(Ipv4::LIMITED_BROADCAST_ADDRESS, PhantomData::<Ipv4>)]
    fn non_unicast_ip_peer<I>(remote_ip: SpecifiedAddr<I::Addr>, _ip: PhantomData<I>)
    where
        I: TcpTestIpExt + Ip,
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        let mut ctx =
            TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(I::TEST_ADDRS.local_ip, remote_ip));
        let mut api = ctx.tcp_api::<I>();
        let remote = SocketAddr { ip: ZonedAddr::Unzoned(remote_ip), port: PORT_2 };
        let socket = api.create(Default::default());

        assert_eq!(
            api.connect(&socket, Some(remote.ip), remote.port)
                .expect_err("connect should fail for non-unicast peer"),
            ConnectError::NoRoute
        );
    }

    #[test_case(true; "any")]
    #[test_case(false; "link local")]
    fn accepted_connection_info_zone(listen_any: bool) {
        set_logger_for_test();
        let client_ip = SpecifiedAddr::new(net_ip_v6!("fe80::1")).unwrap();
        let server_ip = SpecifiedAddr::new(net_ip_v6!("fe80::2")).unwrap();
        let mut net = FakeTcpNetworkSpec::new_network(
            [
                (LOCAL, TcpCtx::with_core_ctx(TcpCoreCtx::new::<Ipv6>(server_ip, client_ip))),
                (REMOTE, TcpCtx::with_core_ctx(TcpCoreCtx::new::<Ipv6>(client_ip, server_ip))),
            ],
            move |net, meta: DualStackSendIpPacketMeta<_>| {
                if net == LOCAL {
                    alloc::vec![(REMOTE, meta, None)]
                } else {
                    alloc::vec![(LOCAL, meta, None)]
                }
            },
        );

        let local_server = net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api::<Ipv6>();
            let socket = api.create(Default::default());
            let device = FakeDeviceId;
            let bind_addr = match listen_any {
                true => None,
                false => Some(ZonedAddr::Zoned(AddrAndZone::new(server_ip, device).unwrap())),
            };

            api.bind(&socket, bind_addr, Some(PORT_1)).expect("failed to bind the client socket");
            api.listen(&socket, NonZeroUsize::new(1).unwrap()).expect("can listen");
            socket
        });

        let _remote_client = net.with_context(REMOTE, |ctx| {
            let mut api = ctx.tcp_api::<Ipv6>();
            let socket = api.create(Default::default());
            let device = FakeDeviceId;
            api.connect(
                &socket,
                Some(ZonedAddr::Zoned(AddrAndZone::new(server_ip, device).unwrap())),
                PORT_1,
            )
            .expect("failed to connect");
            socket
        });

        net.run_until_idle();

        let ConnectionInfo { remote_addr, local_addr, device } = net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api();
            let (server_conn, _addr, _buffers) =
                api.accept(&local_server).expect("connection is available");
            assert_matches!(
                api.get_info(&server_conn),
                SocketInfo::Connection(info) => info
            )
        });

        let device = assert_matches!(device, Some(device) => device);
        assert_eq!(
            local_addr,
            SocketAddr {
                ip: ZonedAddr::Zoned(AddrAndZone::new(server_ip, device).unwrap()),
                port: PORT_1
            }
        );
        let SocketAddr { ip: remote_ip, port: _ } = remote_addr;
        assert_eq!(remote_ip, ZonedAddr::Zoned(AddrAndZone::new(client_ip, device).unwrap()));
    }

    #[test]
    fn bound_connection_info_zoned_addrs() {
        let local_ip = LinkLocalAddr::new(net_ip_v6!("fe80::1")).unwrap().into_specified();
        let remote_ip = LinkLocalAddr::new(net_ip_v6!("fe80::2")).unwrap().into_specified();
        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<Ipv6>(local_ip, remote_ip));

        let local_addr = SocketAddr {
            ip: ZonedAddr::Zoned(AddrAndZone::new(local_ip, FakeDeviceId).unwrap()),
            port: PORT_1,
        };
        let remote_addr = SocketAddr {
            ip: ZonedAddr::Zoned(AddrAndZone::new(remote_ip, FakeDeviceId).unwrap()),
            port: PORT_2,
        };
        let mut api = ctx.tcp_api::<Ipv6>();

        let socket = api.create(Default::default());
        api.bind(&socket, Some(local_addr.ip), Some(local_addr.port)).expect("bind should succeed");

        assert_eq!(
            api.get_info(&socket),
            SocketInfo::Bound(BoundInfo {
                addr: Some(local_addr.ip.map_zone(FakeWeakDeviceId)),
                port: local_addr.port,
                device: Some(FakeWeakDeviceId(FakeDeviceId))
            })
        );

        api.connect(&socket, Some(remote_addr.ip), remote_addr.port)
            .expect("connect should succeed");

        assert_eq!(
            api.get_info(&socket),
            SocketInfo::Connection(ConnectionInfo {
                local_addr: local_addr.map_zone(FakeWeakDeviceId),
                remote_addr: remote_addr.map_zone(FakeWeakDeviceId),
                device: Some(FakeWeakDeviceId(FakeDeviceId))
            })
        );
    }

    #[ip_test(I)]
    // Assuming instant delivery of segments:
    // - If peer calls close, then the timeout we need to wait is in
    // TIME_WAIT, which is 2MSL.
    #[test_case(true, 2 * MSL; "peer calls close")]
    // - If not, we will be in the FIN_WAIT2 state and waiting for its
    // timeout.
    #[test_case(false, DEFAULT_FIN_WAIT2_TIMEOUT; "peer doesn't call close")]
    fn connection_close_peer_calls_close<I: TcpTestIpExt>(
        peer_calls_close: bool,
        expected_time_to_close: Duration,
    ) where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>: TcpContext<
                I,
                TcpBindingsCtx<FakeDeviceId>,
                SingleStackConverter = I::SingleStackConverter,
                DualStackConverter = I::DualStackConverter,
            >,
    {
        set_logger_for_test();
        let (mut net, local, _local_snd_end, remote) = bind_listen_connect_accept_inner::<I>(
            I::UNSPECIFIED_ADDRESS,
            BindConfig {
                client_port: None,
                server_port: PORT_1,
                client_reuse_addr: false,
                send_test_data: false,
            },
            0,
            0.0,
        );

        let weak_local = local.downgrade();
        let close_called = net.with_context(LOCAL, |ctx| {
            ctx.tcp_api().close(local);
            ctx.bindings_ctx.now()
        });

        while {
            assert!(!net.step().is_idle());
            let is_fin_wait_2 = {
                let local = weak_local.upgrade().unwrap();
                let state = local.get();
                let state = assert_matches!(
                    &state.deref().socket_state,
                    TcpSocketStateInner::Connected { conn, .. } => {
                    let (conn, _addr) = assert_this_stack_conn::<I, _, TcpCoreCtx<_, _>>(conn, &I::converter());
                    assert_matches!(
                        conn,
                        Connection {
                            state,
                            ..
                        } => state
                    )
                }
                );
                matches!(state, State::FinWait2(_))
            };
            !is_fin_wait_2
        } {}

        let weak_remote = remote.downgrade();
        if peer_calls_close {
            net.with_context(REMOTE, |ctx| {
                ctx.tcp_api().close(remote);
            });
        }

        net.run_until_idle();

        net.with_context(LOCAL, |TcpCtx { core_ctx: _, bindings_ctx }| {
            assert_eq!(
                bindings_ctx.now().checked_duration_since(close_called).unwrap(),
                expected_time_to_close
            );
            assert_eq!(weak_local.upgrade(), None);
        });
        if peer_calls_close {
            assert_eq!(weak_remote.upgrade(), None);
        }
    }

    #[ip_test(I)]
    fn connection_shutdown_then_close_peer_doesnt_call_close<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>: TcpContext<
                I,
                TcpBindingsCtx<FakeDeviceId>,
                SingleStackConverter = I::SingleStackConverter,
                DualStackConverter = I::DualStackConverter,
            >,
    {
        set_logger_for_test();
        let (mut net, local, _local_snd_end, _remote) = bind_listen_connect_accept_inner::<I>(
            I::UNSPECIFIED_ADDRESS,
            BindConfig {
                client_port: None,
                server_port: PORT_1,
                client_reuse_addr: false,
                send_test_data: false,
            },
            0,
            0.0,
        );
        net.with_context(LOCAL, |ctx| {
            assert_eq!(ctx.tcp_api().shutdown(&local, ShutdownType::Send), Ok(true));
        });
        loop {
            assert!(!net.step().is_idle());
            let is_fin_wait_2 = {
                let state = local.get();
                let state = assert_matches!(
                &state.deref().socket_state,
                TcpSocketStateInner::Connected { conn, .. } => {
                let (conn, _addr) = assert_this_stack_conn::<I, _, TcpCoreCtx<_, _>>(conn, &I::converter());
                assert_matches!(
                    conn,
                    Connection {
                        state, ..
                    } => state
                )});
                matches!(state, State::FinWait2(_))
            };
            if is_fin_wait_2 {
                break;
            }
        }

        let weak_local = local.downgrade();
        net.with_context(LOCAL, |ctx| {
            ctx.tcp_api().close(local);
        });
        net.run_until_idle();
        assert_eq!(weak_local.upgrade(), None);
    }

    #[ip_test(I)]
    fn connection_shutdown_then_close<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>: TcpContext<
                I,
                TcpBindingsCtx<FakeDeviceId>,
                SingleStackConverter = I::SingleStackConverter,
                DualStackConverter = I::DualStackConverter,
            >,
    {
        set_logger_for_test();
        let (mut net, local, _local_snd_end, remote) = bind_listen_connect_accept_inner::<I>(
            I::UNSPECIFIED_ADDRESS,
            BindConfig {
                client_port: None,
                server_port: PORT_1,
                client_reuse_addr: false,
                send_test_data: false,
            },
            0,
            0.0,
        );

        for (name, id) in [(LOCAL, &local), (REMOTE, &remote)] {
            net.with_context(name, |ctx| {
                let mut api = ctx.tcp_api();
                assert_eq!(
                    api.shutdown(id,ShutdownType::Send),
                    Ok(true)
                );
                assert_matches!(
                    &id.get().deref().socket_state,
                    TcpSocketStateInner::Connected { conn, .. } => {
                    let (conn, _addr) = assert_this_stack_conn::<I, _, TcpCoreCtx<_, _>>(conn, &I::converter());
                    assert_matches!(
                        conn,
                        Connection {
                            state: State::FinWait1(_),
                            ..
                        }
                    );
                });
                assert_eq!(
                    api.shutdown(id,ShutdownType::Send),
                    Ok(true)
                );
            });
        }
        net.run_until_idle();
        for (name, id) in [(LOCAL, local), (REMOTE, remote)] {
            net.with_context(name, |ctx| {
                assert_matches!(
                    &id.get().deref().socket_state,
                    TcpSocketStateInner::Connected { conn, .. } => {
                    let (conn, _addr) = assert_this_stack_conn::<I, _, TcpCoreCtx<_, _>>(conn, &I::converter());
                    assert_matches!(
                        conn,
                        Connection {
                            state: State::Closed(_),
                            ..
                        }
                    );
                });
                let weak_id = id.downgrade();
                ctx.tcp_api().close(id);
                assert_eq!(weak_id.upgrade(), None)
            });
        }
    }

    #[ip_test(I)]
    fn remove_unbound<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<I>();
        let unbound = api.create(Default::default());
        let weak_unbound = unbound.downgrade();
        api.close(unbound);
        assert_eq!(weak_unbound.upgrade(), None);
    }

    #[ip_test(I)]
    fn remove_bound<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<I>();
        let socket = api.create(Default::default());
        api.bind(&socket, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), None)
            .expect("bind should succeed");
        let weak_socket = socket.downgrade();
        api.close(socket);
        assert_eq!(weak_socket.upgrade(), None);
    }

    #[ip_test(I)]
    fn shutdown_listener<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>: TcpContext<
                I,
                TcpBindingsCtx<FakeDeviceId>,
                SingleStackConverter = I::SingleStackConverter,
                DualStackConverter = I::DualStackConverter,
            >,
    {
        set_logger_for_test();
        let mut net = new_test_net::<I>();
        let local_listener = net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            let socket = api.create(Default::default());
            api.bind(&socket, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), Some(PORT_1))
                .expect("bind should succeed");
            api.listen(&socket, NonZeroUsize::new(5).unwrap()).expect("can listen");
            socket
        });

        let remote_connection = net.with_context(REMOTE, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            let socket = api.create(Default::default());
            api.connect(&socket, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), PORT_1)
                .expect("connect should succeed");
            socket
        });

        // After the following step, we should have one established connection
        // in the listener's accept queue, which ought to be aborted during
        // shutdown.
        net.run_until_idle();

        // The incoming connection was signaled, and the remote end was notified
        // of connection establishment.
        net.with_context(REMOTE, |ctx| {
            assert_eq!(
                ctx.tcp_api().connect(
                    &remote_connection,
                    Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)),
                    PORT_1
                ),
                Ok(())
            );
        });

        // Create a second half-open connection so that we have one entry in the
        // pending queue.
        let second_connection = net.with_context(REMOTE, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            let socket = api.create(Default::default());
            api.connect(&socket, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), PORT_1)
                .expect("connect should succeed");
            socket
        });

        let _: StepResult = net.step();

        // We have a timer scheduled for the pending connection.
        net.with_context(LOCAL, |TcpCtx { core_ctx: _, bindings_ctx }| {
            assert_matches!(bindings_ctx.timers.timers().len(), 1);
        });

        net.with_context(LOCAL, |ctx| {
            assert_eq!(ctx.tcp_api().shutdown(&local_listener, ShutdownType::Receive,), Ok(false));
        });

        // The timer for the pending connection should be cancelled.
        net.with_context(LOCAL, |TcpCtx { core_ctx: _, bindings_ctx }| {
            assert_eq!(bindings_ctx.timers.timers().len(), 0);
        });

        net.run_until_idle();

        // Both remote sockets should now be reset to Closed state.
        net.with_context(REMOTE, |ctx| {
            for conn in [&remote_connection, &second_connection] {
                assert_eq!(
                    ctx.tcp_api().get_socket_error(conn),
                    Some(ConnectionError::ConnectionReset),
                )
            }

            assert_matches!(
                &remote_connection.get().deref().socket_state,
                TcpSocketStateInner::Connected { conn, .. } => {
                        let (conn, _addr) = assert_this_stack_conn::<I, _, TcpCoreCtx<_, _>>(conn, &I::converter());
                        assert_matches!(
                            conn,
                            Connection {
                                state: State::Closed(Closed {
                                    // Error was cleared by get_socket_error.
                                    reason: None
                                }),
                                ..
                            }
                        );
                    }
            );
        });

        net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            let new_unbound = api.create(Default::default());
            assert_matches!(
                api.bind(
                    &new_unbound,
                    Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip,)),
                    Some(PORT_1),
                ),
                Err(BindError::LocalAddressError(LocalAddressError::AddressInUse))
            );
            // Bring the already-shutdown listener back to listener again.
            api.listen(&local_listener, NonZeroUsize::new(5).unwrap()).expect("can listen again");
        });

        let new_remote_connection = net.with_context(REMOTE, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            let socket = api.create(Default::default());
            api.connect(&socket, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), PORT_1)
                .expect("connect should succeed");
            socket
        });

        net.run_until_idle();

        net.with_context(REMOTE, |ctx| {
            assert_matches!(
                &new_remote_connection.get().deref().socket_state,
                TcpSocketStateInner::Connected { conn, .. } => {
                    let (conn, _addr) = assert_this_stack_conn::<I, _, TcpCoreCtx<_, _>>(conn, &I::converter());
                    assert_matches!(
                        conn,
                        Connection {
                            state: State::Established(_),
                            ..
                        }
                    );
                    });
            assert_eq!(
                ctx.tcp_api().connect(
                    &new_remote_connection,
                    Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)),
                    PORT_1,
                ),
                Ok(())
            );
        });
    }

    #[ip_test(I)]
    fn clamp_buffer_size<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        set_logger_for_test();
        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<I>();
        let socket = api.create(Default::default());

        let (min, max) =
            SettingsContext::<TcpSettings>::settings(&ctx.bindings_ctx).send_buffer.min_max();
        let mut api = ctx.tcp_api::<I>();
        api.set_send_buffer_size(&socket, min.get() - 1);
        assert_eq!(api.send_buffer_size(&socket), Some(min.get()));
        api.set_send_buffer_size(&socket, max.get() + 1);
        assert_eq!(api.send_buffer_size(&socket), Some(max.get()));

        let (min, max) =
            SettingsContext::<TcpSettings>::settings(&ctx.bindings_ctx).receive_buffer.min_max();
        let mut api = ctx.tcp_api::<I>();
        api.set_receive_buffer_size(&socket, min.get() - 1);
        assert_eq!(api.receive_buffer_size(&socket), Some(min.get()));
        api.set_receive_buffer_size(&socket, max.get() + 1);
        assert_eq!(api.receive_buffer_size(&socket), Some(max.get()));
    }

    #[ip_test(I)]
    fn set_reuseaddr_unbound<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<I>();

        let first_bound = {
            let socket = api.create(Default::default());
            api.set_reuseaddr(&socket, true).expect("can set");
            api.bind(&socket, None, None).expect("bind succeeds");
            socket
        };
        let _second_bound = {
            let socket = api.create(Default::default());
            api.set_reuseaddr(&socket, true).expect("can set");
            api.bind(&socket, None, None).expect("bind succeeds");
            socket
        };

        api.listen(&first_bound, NonZeroUsize::new(10).unwrap()).expect("can listen");
    }

    #[ip_test(I)]
    #[test_case([true, true], Ok(()); "allowed with set")]
    #[test_case([false, true], Err(LocalAddressError::AddressInUse); "first unset")]
    #[test_case([true, false], Err(LocalAddressError::AddressInUse); "second unset")]
    #[test_case([false, false], Err(LocalAddressError::AddressInUse); "both unset")]
    fn reuseaddr_multiple_bound<I: TcpTestIpExt>(
        set_reuseaddr: [bool; 2],
        expected: Result<(), LocalAddressError>,
    ) where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<I>();

        let first = api.create(Default::default());
        api.set_reuseaddr(&first, set_reuseaddr[0]).expect("can set");
        api.bind(&first, None, Some(PORT_1)).expect("bind succeeds");

        let second = api.create(Default::default());
        api.set_reuseaddr(&second, set_reuseaddr[1]).expect("can set");
        let second_bind_result = api.bind(&second, None, Some(PORT_1));

        assert_eq!(second_bind_result, expected.map_err(From::from));
    }

    #[ip_test(I)]
    fn toggle_reuseaddr_bound_different_addrs<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        let addrs = [1, 2].map(|i| I::get_other_ip_address(i));
        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::with_ip_socket_ctx_state(
            FakeDualStackIpSocketCtx::new(core::iter::once(FakeDeviceConfig {
                device: FakeDeviceId,
                local_ips: addrs.iter().cloned().map(SpecifiedAddr::<IpAddr>::from).collect(),
                remote_ips: Default::default(),
            })),
        ));
        let mut api = ctx.tcp_api::<I>();

        let first = api.create(Default::default());
        api.bind(&first, Some(ZonedAddr::Unzoned(addrs[0])), Some(PORT_1)).unwrap();

        let second = api.create(Default::default());
        api.bind(&second, Some(ZonedAddr::Unzoned(addrs[1])), Some(PORT_1)).unwrap();
        // Setting and un-setting ReuseAddr should be fine since these sockets
        // don't conflict.
        api.set_reuseaddr(&first, true).expect("can set");
        api.set_reuseaddr(&first, false).expect("can un-set");
    }

    #[ip_test(I)]
    fn unset_reuseaddr_bound_unspecified_specified<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<I>();
        let first = api.create(Default::default());
        api.set_reuseaddr(&first, true).expect("can set");
        api.bind(&first, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), Some(PORT_1)).unwrap();

        let second = api.create(Default::default());
        api.set_reuseaddr(&second, true).expect("can set");
        api.bind(&second, None, Some(PORT_1)).unwrap();

        // Both sockets can be bound because they have ReuseAddr set. Since
        // removing it would introduce inconsistent state, that's not allowed.
        assert_matches!(api.set_reuseaddr(&first, false), Err(SetReuseAddrError::AddrInUse));
        assert_matches!(api.set_reuseaddr(&second, false), Err(SetReuseAddrError::AddrInUse));
    }

    #[ip_test(I)]
    fn reuseaddr_allows_binding_under_connection<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        set_logger_for_test();
        let mut net = new_test_net::<I>();

        let server = net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            let server = api.create(Default::default());
            api.set_reuseaddr(&server, true).expect("can set");
            api.bind(&server, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), Some(PORT_1))
                .expect("failed to bind the client socket");
            api.listen(&server, NonZeroUsize::new(10).unwrap()).expect("can listen");
            server
        });

        let client = net.with_context(REMOTE, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            let client = api.create(Default::default());
            api.connect(&client, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), PORT_1)
                .expect("connect should succeed");
            client
        });
        // Finish the connection establishment.
        net.run_until_idle();
        net.with_context(REMOTE, |ctx| {
            assert_eq!(
                ctx.tcp_api().connect(
                    &client,
                    Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)),
                    PORT_1
                ),
                Ok(())
            );
        });

        // Now accept the connection and close the listening socket. Then
        // binding a new socket on the same local address should fail unless the
        // socket has SO_REUSEADDR set.
        net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api();
            let (_server_conn, _, _): (_, SocketAddr<_, _>, ClientBuffers) =
                api.accept(&server).expect("pending connection");

            assert_eq!(api.shutdown(&server, ShutdownType::Receive), Ok(false));
            api.close(server);

            let unbound = api.create(Default::default());
            assert_eq!(
                api.bind(&unbound, None, Some(PORT_1)),
                Err(BindError::LocalAddressError(LocalAddressError::AddressInUse))
            );

            // Binding should succeed after setting ReuseAddr.
            api.set_reuseaddr(&unbound, true).expect("can set");
            api.bind(&unbound, None, Some(PORT_1)).expect("bind succeeds");
        });
    }

    #[ip_test(I)]
    #[test_case([true, true]; "specified specified")]
    #[test_case([false, true]; "any specified")]
    #[test_case([true, false]; "specified any")]
    #[test_case([false, false]; "any any")]
    fn set_reuseaddr_bound_allows_other_bound<I: TcpTestIpExt>(bind_specified: [bool; 2])
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<I>();

        let [first_addr, second_addr] =
            bind_specified.map(|b| b.then_some(I::TEST_ADDRS.local_ip).map(ZonedAddr::Unzoned));
        let first_bound = {
            let socket = api.create(Default::default());
            api.bind(&socket, first_addr, Some(PORT_1)).expect("bind succeeds");
            socket
        };

        let second = api.create(Default::default());

        // Binding the second socket will fail because the first doesn't have
        // SO_REUSEADDR set.
        assert_matches!(
            api.bind(&second, second_addr, Some(PORT_1)),
            Err(BindError::LocalAddressError(LocalAddressError::AddressInUse))
        );

        // Setting SO_REUSEADDR for the second socket isn't enough.
        api.set_reuseaddr(&second, true).expect("can set");
        assert_matches!(
            api.bind(&second, second_addr, Some(PORT_1)),
            Err(BindError::LocalAddressError(LocalAddressError::AddressInUse))
        );

        // Setting SO_REUSEADDR for the first socket lets the second bind.
        api.set_reuseaddr(&first_bound, true).expect("only socket");
        api.bind(&second, second_addr, Some(PORT_1)).expect("can bind");
    }

    #[ip_test(I)]
    fn clear_reuseaddr_listener<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<I>();

        let bound = {
            let socket = api.create(Default::default());
            api.set_reuseaddr(&socket, true).expect("can set");
            api.bind(&socket, None, Some(PORT_1)).expect("bind succeeds");
            socket
        };

        let listener = {
            let socket = api.create(Default::default());
            api.set_reuseaddr(&socket, true).expect("can set");

            api.bind(&socket, None, Some(PORT_1)).expect("bind succeeds");
            api.listen(&socket, NonZeroUsize::new(5).unwrap()).expect("can listen");
            socket
        };

        // We can't clear SO_REUSEADDR on the listener because it's sharing with
        // the bound socket.
        assert_matches!(api.set_reuseaddr(&listener, false), Err(SetReuseAddrError::AddrInUse));

        // We can, however, connect to the listener with the bound socket. Then
        // the unencumbered listener can clear SO_REUSEADDR.
        api.connect(&bound, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), PORT_1)
            .expect("can connect");
        api.set_reuseaddr(&listener, false).expect("can unset")
    }

    fn deliver_icmp_error<
        I: TcpTestIpExt + IcmpIpExt,
        CC: TcpContext<I, BC, DeviceId = FakeDeviceId>
            + TcpContext<I::OtherVersion, BC, DeviceId = FakeDeviceId>,
        BC: TcpBindingsContext<CC::DeviceId>,
    >(
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        original_src_ip: SpecifiedAddr<I::Addr>,
        original_dst_ip: SpecifiedAddr<I::Addr>,
        original_body: &[u8],
        err: I::ErrorCode,
    ) {
        <TcpIpTransportContext as IpTransportContext<I, _, _>>::receive_icmp_error(
            core_ctx,
            bindings_ctx,
            &FakeDeviceId,
            Some(original_src_ip),
            original_dst_ip,
            original_body,
            err,
        );
    }

    #[test_case(Icmpv4ErrorCode::DestUnreachable(Icmpv4DestUnreachableCode::DestNetworkUnreachable, IcmpDestUnreachable::default()) => ConnectionError::NetworkUnreachable)]
    #[test_case(Icmpv4ErrorCode::DestUnreachable(Icmpv4DestUnreachableCode::DestHostUnreachable, IcmpDestUnreachable::default()) => ConnectionError::HostUnreachable)]
    #[test_case(Icmpv4ErrorCode::DestUnreachable(Icmpv4DestUnreachableCode::DestProtocolUnreachable, IcmpDestUnreachable::default()) => ConnectionError::ProtocolUnreachable)]
    #[test_case(Icmpv4ErrorCode::DestUnreachable(Icmpv4DestUnreachableCode::DestPortUnreachable, IcmpDestUnreachable::default()) => ConnectionError::PortUnreachable)]
    #[test_case(Icmpv4ErrorCode::DestUnreachable(Icmpv4DestUnreachableCode::SourceRouteFailed, IcmpDestUnreachable::default()) => ConnectionError::SourceRouteFailed)]
    #[test_case(Icmpv4ErrorCode::DestUnreachable(Icmpv4DestUnreachableCode::DestNetworkUnknown, IcmpDestUnreachable::default()) => ConnectionError::NetworkUnreachable)]
    #[test_case(Icmpv4ErrorCode::DestUnreachable(Icmpv4DestUnreachableCode::DestHostUnknown, IcmpDestUnreachable::default()) => ConnectionError::DestinationHostDown)]
    #[test_case(Icmpv4ErrorCode::DestUnreachable(Icmpv4DestUnreachableCode::SourceHostIsolated, IcmpDestUnreachable::default()) => ConnectionError::SourceHostIsolated)]
    #[test_case(Icmpv4ErrorCode::DestUnreachable(Icmpv4DestUnreachableCode::NetworkAdministrativelyProhibited, IcmpDestUnreachable::default()) => ConnectionError::NetworkUnreachable)]
    #[test_case(Icmpv4ErrorCode::DestUnreachable(Icmpv4DestUnreachableCode::HostAdministrativelyProhibited, IcmpDestUnreachable::default()) => ConnectionError::HostUnreachable)]
    #[test_case(Icmpv4ErrorCode::DestUnreachable(Icmpv4DestUnreachableCode::NetworkUnreachableForToS, IcmpDestUnreachable::default()) => ConnectionError::NetworkUnreachable)]
    #[test_case(Icmpv4ErrorCode::DestUnreachable(Icmpv4DestUnreachableCode::HostUnreachableForToS, IcmpDestUnreachable::default()) => ConnectionError::HostUnreachable)]
    #[test_case(Icmpv4ErrorCode::DestUnreachable(Icmpv4DestUnreachableCode::CommAdministrativelyProhibited, IcmpDestUnreachable::default()) => ConnectionError::HostUnreachable)]
    #[test_case(Icmpv4ErrorCode::DestUnreachable(Icmpv4DestUnreachableCode::HostPrecedenceViolation, IcmpDestUnreachable::default()) => ConnectionError::HostUnreachable)]
    #[test_case(Icmpv4ErrorCode::DestUnreachable(Icmpv4DestUnreachableCode::PrecedenceCutoffInEffect, IcmpDestUnreachable::default()) => ConnectionError::HostUnreachable)]
    #[test_case(Icmpv4ErrorCode::ParameterProblem(Icmpv4ParameterProblemCode::PointerIndicatesError) => ConnectionError::ProtocolError)]
    #[test_case(Icmpv4ErrorCode::ParameterProblem(Icmpv4ParameterProblemCode::MissingRequiredOption) => ConnectionError::ProtocolError)]
    #[test_case(Icmpv4ErrorCode::ParameterProblem(Icmpv4ParameterProblemCode::BadLength) => ConnectionError::ProtocolError)]
    #[test_case(Icmpv4ErrorCode::TimeExceeded(Icmpv4TimeExceededCode::TtlExpired) => ConnectionError::HostUnreachable)]
    #[test_case(Icmpv4ErrorCode::TimeExceeded(Icmpv4TimeExceededCode::FragmentReassemblyTimeExceeded) => ConnectionError::TimedOut)]
    fn icmp_destination_unreachable_connect_v4(error: Icmpv4ErrorCode) -> ConnectionError {
        icmp_destination_unreachable_connect_inner::<Ipv4>(error)
    }

    #[test_case(Icmpv6ErrorCode::DestUnreachable(Icmpv6DestUnreachableCode::NoRoute) => ConnectionError::NetworkUnreachable)]
    #[test_case(Icmpv6ErrorCode::DestUnreachable(Icmpv6DestUnreachableCode::CommAdministrativelyProhibited) => ConnectionError::PermissionDenied)]
    #[test_case(Icmpv6ErrorCode::DestUnreachable(Icmpv6DestUnreachableCode::BeyondScope) => ConnectionError::HostUnreachable)]
    #[test_case(Icmpv6ErrorCode::DestUnreachable(Icmpv6DestUnreachableCode::AddrUnreachable) => ConnectionError::HostUnreachable)]
    #[test_case(Icmpv6ErrorCode::DestUnreachable(Icmpv6DestUnreachableCode::PortUnreachable) => ConnectionError::PortUnreachable)]
    #[test_case(Icmpv6ErrorCode::DestUnreachable(Icmpv6DestUnreachableCode::SrcAddrFailedPolicy) => ConnectionError::PermissionDenied)]
    #[test_case(Icmpv6ErrorCode::DestUnreachable(Icmpv6DestUnreachableCode::RejectRoute) => ConnectionError::PermissionDenied)]
    #[test_case(Icmpv6ErrorCode::ParameterProblem(Icmpv6ParameterProblemCode::ErroneousHeaderField) => ConnectionError::ProtocolError)]
    #[test_case(Icmpv6ErrorCode::ParameterProblem(Icmpv6ParameterProblemCode::UnrecognizedNextHeaderType) => ConnectionError::ProtocolError)]
    #[test_case(Icmpv6ErrorCode::ParameterProblem(Icmpv6ParameterProblemCode::UnrecognizedIpv6Option) => ConnectionError::ProtocolError)]
    #[test_case(Icmpv6ErrorCode::TimeExceeded(Icmpv6TimeExceededCode::HopLimitExceeded) => ConnectionError::HostUnreachable)]
    #[test_case(Icmpv6ErrorCode::TimeExceeded(Icmpv6TimeExceededCode::FragmentReassemblyTimeExceeded) => ConnectionError::HostUnreachable)]
    fn icmp_destination_unreachable_connect_v6(error: Icmpv6ErrorCode) -> ConnectionError {
        icmp_destination_unreachable_connect_inner::<Ipv6>(error)
    }

    fn icmp_destination_unreachable_connect_inner<I: TcpTestIpExt + IcmpIpExt>(
        icmp_error: I::ErrorCode,
    ) -> ConnectionError
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>: TcpContext<I, TcpBindingsCtx<FakeDeviceId>>
            + TcpContext<I::OtherVersion, TcpBindingsCtx<FakeDeviceId>>,
    {
        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<I>();

        let connection = api.create(Default::default());
        api.connect(&connection, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), PORT_1)
            .expect("failed to create a connection socket");

        let (core_ctx, bindings_ctx) = api.contexts();
        let frames = core_ctx.ip_socket_ctx.take_frames();
        let frame = assert_matches!(&frames[..], [(_meta, frame)] => frame);

        deliver_icmp_error::<I, _, _>(
            core_ctx,
            bindings_ctx,
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
            &frame[0..8],
            icmp_error,
        );
        // The TCP handshake should fail.
        let err = api
            .connect(&connection, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), PORT_1)
            .expect_err("should fail");
        // The connect call should've taken the error.
        assert_eq!(api.get_socket_error(&connection), None);
        // Failure due to ICMP error.
        assert_matches!(err, ConnectError::ConnectionError(e) => e)
    }

    #[test_case(Icmpv4ErrorCode::DestUnreachable(Icmpv4DestUnreachableCode::DestNetworkUnreachable, IcmpDestUnreachable::default()) => ConnectionError::NetworkUnreachable)]
    #[test_case(Icmpv4ErrorCode::DestUnreachable(Icmpv4DestUnreachableCode::DestHostUnreachable, IcmpDestUnreachable::default()) => ConnectionError::HostUnreachable)]
    #[test_case(Icmpv4ErrorCode::DestUnreachable(Icmpv4DestUnreachableCode::DestProtocolUnreachable, IcmpDestUnreachable::default()) => ConnectionError::ProtocolUnreachable)]
    #[test_case(Icmpv4ErrorCode::DestUnreachable(Icmpv4DestUnreachableCode::DestPortUnreachable, IcmpDestUnreachable::default()) => ConnectionError::PortUnreachable)]
    #[test_case(Icmpv4ErrorCode::DestUnreachable(Icmpv4DestUnreachableCode::SourceRouteFailed, IcmpDestUnreachable::default()) => ConnectionError::SourceRouteFailed)]
    #[test_case(Icmpv4ErrorCode::DestUnreachable(Icmpv4DestUnreachableCode::DestNetworkUnknown, IcmpDestUnreachable::default()) => ConnectionError::NetworkUnreachable)]
    #[test_case(Icmpv4ErrorCode::DestUnreachable(Icmpv4DestUnreachableCode::DestHostUnknown, IcmpDestUnreachable::default()) => ConnectionError::DestinationHostDown)]
    #[test_case(Icmpv4ErrorCode::DestUnreachable(Icmpv4DestUnreachableCode::SourceHostIsolated, IcmpDestUnreachable::default()) => ConnectionError::SourceHostIsolated)]
    #[test_case(Icmpv4ErrorCode::DestUnreachable(Icmpv4DestUnreachableCode::NetworkAdministrativelyProhibited, IcmpDestUnreachable::default()) => ConnectionError::NetworkUnreachable)]
    #[test_case(Icmpv4ErrorCode::DestUnreachable(Icmpv4DestUnreachableCode::HostAdministrativelyProhibited, IcmpDestUnreachable::default()) => ConnectionError::HostUnreachable)]
    #[test_case(Icmpv4ErrorCode::DestUnreachable(Icmpv4DestUnreachableCode::NetworkUnreachableForToS, IcmpDestUnreachable::default()) => ConnectionError::NetworkUnreachable)]
    #[test_case(Icmpv4ErrorCode::DestUnreachable(Icmpv4DestUnreachableCode::HostUnreachableForToS, IcmpDestUnreachable::default()) => ConnectionError::HostUnreachable)]
    #[test_case(Icmpv4ErrorCode::DestUnreachable(Icmpv4DestUnreachableCode::CommAdministrativelyProhibited, IcmpDestUnreachable::default()) => ConnectionError::HostUnreachable)]
    #[test_case(Icmpv4ErrorCode::DestUnreachable(Icmpv4DestUnreachableCode::HostPrecedenceViolation, IcmpDestUnreachable::default()) => ConnectionError::HostUnreachable)]
    #[test_case(Icmpv4ErrorCode::DestUnreachable(Icmpv4DestUnreachableCode::PrecedenceCutoffInEffect, IcmpDestUnreachable::default()) => ConnectionError::HostUnreachable)]
    #[test_case(Icmpv4ErrorCode::ParameterProblem(Icmpv4ParameterProblemCode::PointerIndicatesError) => ConnectionError::ProtocolError)]
    #[test_case(Icmpv4ErrorCode::ParameterProblem(Icmpv4ParameterProblemCode::MissingRequiredOption) => ConnectionError::ProtocolError)]
    #[test_case(Icmpv4ErrorCode::ParameterProblem(Icmpv4ParameterProblemCode::BadLength) => ConnectionError::ProtocolError)]
    #[test_case(Icmpv4ErrorCode::TimeExceeded(Icmpv4TimeExceededCode::TtlExpired) => ConnectionError::HostUnreachable)]
    #[test_case(Icmpv4ErrorCode::TimeExceeded(Icmpv4TimeExceededCode::FragmentReassemblyTimeExceeded) => ConnectionError::TimedOut)]
    fn icmp_destination_unreachable_established_v4(error: Icmpv4ErrorCode) -> ConnectionError {
        icmp_destination_unreachable_established_inner::<Ipv4>(error)
    }

    #[test_case(Icmpv6ErrorCode::DestUnreachable(Icmpv6DestUnreachableCode::NoRoute) => ConnectionError::NetworkUnreachable)]
    #[test_case(Icmpv6ErrorCode::DestUnreachable(Icmpv6DestUnreachableCode::CommAdministrativelyProhibited) => ConnectionError::PermissionDenied)]
    #[test_case(Icmpv6ErrorCode::DestUnreachable(Icmpv6DestUnreachableCode::BeyondScope) => ConnectionError::HostUnreachable)]
    #[test_case(Icmpv6ErrorCode::DestUnreachable(Icmpv6DestUnreachableCode::AddrUnreachable) => ConnectionError::HostUnreachable)]
    #[test_case(Icmpv6ErrorCode::DestUnreachable(Icmpv6DestUnreachableCode::PortUnreachable) => ConnectionError::PortUnreachable)]
    #[test_case(Icmpv6ErrorCode::DestUnreachable(Icmpv6DestUnreachableCode::SrcAddrFailedPolicy) => ConnectionError::PermissionDenied)]
    #[test_case(Icmpv6ErrorCode::DestUnreachable(Icmpv6DestUnreachableCode::RejectRoute) => ConnectionError::PermissionDenied)]
    #[test_case(Icmpv6ErrorCode::ParameterProblem(Icmpv6ParameterProblemCode::ErroneousHeaderField) => ConnectionError::ProtocolError)]
    #[test_case(Icmpv6ErrorCode::ParameterProblem(Icmpv6ParameterProblemCode::UnrecognizedNextHeaderType) => ConnectionError::ProtocolError)]
    #[test_case(Icmpv6ErrorCode::ParameterProblem(Icmpv6ParameterProblemCode::UnrecognizedIpv6Option) => ConnectionError::ProtocolError)]
    #[test_case(Icmpv6ErrorCode::TimeExceeded(Icmpv6TimeExceededCode::HopLimitExceeded) => ConnectionError::HostUnreachable)]
    #[test_case(Icmpv6ErrorCode::TimeExceeded(Icmpv6TimeExceededCode::FragmentReassemblyTimeExceeded) => ConnectionError::HostUnreachable)]
    fn icmp_destination_unreachable_established_v6(error: Icmpv6ErrorCode) -> ConnectionError {
        icmp_destination_unreachable_established_inner::<Ipv6>(error)
    }

    fn icmp_destination_unreachable_established_inner<I: TcpTestIpExt + IcmpIpExt>(
        icmp_error: I::ErrorCode,
    ) -> ConnectionError
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>: TcpContext<
                I,
                TcpBindingsCtx<FakeDeviceId>,
                SingleStackConverter = I::SingleStackConverter,
                DualStackConverter = I::DualStackConverter,
            > + TcpContext<I::OtherVersion, TcpBindingsCtx<FakeDeviceId>>,
    {
        let (mut net, local, local_snd_end, _remote) = bind_listen_connect_accept_inner::<I>(
            I::UNSPECIFIED_ADDRESS,
            BindConfig {
                client_port: None,
                server_port: PORT_1,
                client_reuse_addr: false,
                send_test_data: false,
            },
            0,
            0.0,
        );
        local_snd_end.lock().extend_from_slice(b"Hello");
        net.with_context(LOCAL, |ctx| {
            ctx.tcp_api().do_send(&local);
        });
        net.collect_frames();
        let original_body = assert_matches!(
            &net.iter_pending_frames().collect::<Vec<_>>()[..],
            [InstantAndData(_instant, PendingFrameData {
                dst_context: _,
                meta: _,
                frame,
            })] => {
            frame.clone()
        });
        net.with_context(LOCAL, |ctx| {
            let TcpCtx { core_ctx, bindings_ctx } = ctx;
            deliver_icmp_error::<I, _, _>(
                core_ctx,
                bindings_ctx,
                I::TEST_ADDRS.local_ip,
                I::TEST_ADDRS.remote_ip,
                &original_body[..],
                icmp_error,
            );
            // An error should be posted on the connection.
            let error = assert_matches!(
                ctx.tcp_api().get_socket_error(&local),
                Some(error) => error
            );
            // But it should stay established.
            assert_matches!(
                &local.get().deref().socket_state,
                TcpSocketStateInner::Connected { conn, .. } => {
                    let (conn, _addr) = assert_this_stack_conn::<I, _, TcpCoreCtx<_, _>>(conn, &I::converter());
                    assert_matches!(
                        conn,
                        Connection {
                            state: State::Established(_),
                            ..
                        }
                    );
                }
            );
            error
        })
    }

    #[ip_test(I)]
    fn icmp_destination_unreachable_listener<I: TcpTestIpExt + IcmpIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>: TcpContext<I, TcpBindingsCtx<FakeDeviceId>>
            + TcpContext<I::OtherVersion, TcpBindingsCtx<FakeDeviceId>>
            + CounterContext<TcpCountersWithSocket<I>>,
    {
        let mut net = new_test_net::<I>();

        let backlog = NonZeroUsize::new(1).unwrap();
        let server = net.with_context(REMOTE, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            let server = api.create(Default::default());
            api.bind(&server, None, Some(PORT_1)).expect("failed to bind the server socket");
            api.listen(&server, backlog).expect("can listen");
            server
        });

        net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            let conn = api.create(Default::default());
            api.connect(&conn, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), PORT_1)
                .expect("failed to connect");
        });

        assert!(!net.step().is_idle());

        net.collect_frames();
        let original_body = assert_matches!(
            &net.iter_pending_frames().collect::<Vec<_>>()[..],
            [InstantAndData(_instant, PendingFrameData {
                dst_context: _,
                meta: _,
                frame,
            })] => {
            frame.clone()
        });
        let icmp_error = I::map_ip(
            (),
            |()| {
                Icmpv4ErrorCode::DestUnreachable(
                    Icmpv4DestUnreachableCode::DestPortUnreachable,
                    IcmpDestUnreachable::default(),
                )
            },
            |()| Icmpv6ErrorCode::DestUnreachable(Icmpv6DestUnreachableCode::PortUnreachable),
        );
        net.with_context(REMOTE, |TcpCtx { core_ctx, bindings_ctx }| {
            let in_queue = {
                let state = server.get();
                let accept_queue = assert_matches!(
                    &state.deref().socket_state,
                    TcpSocketStateInner::Listener(Listener { accept_queue, .. }) => accept_queue
                );
                assert_eq!(accept_queue.len(), 1);
                accept_queue.collect_pending().first().unwrap().downgrade()
            };
            deliver_icmp_error::<I, _, _>(
                core_ctx,
                bindings_ctx,
                I::TEST_ADDRS.remote_ip,
                I::TEST_ADDRS.local_ip,
                &original_body[..],
                icmp_error,
            );
            {
                let state = server.get();
                let queue_len = assert_matches!(
                    &state.deref().socket_state,
                    TcpSocketStateInner::Listener(Listener { accept_queue, .. }) => accept_queue.len()
                );
                assert_eq!(queue_len, 0);
            }
            // Socket must've been destroyed.
            assert_eq!(in_queue.upgrade(), None);
        });
    }

    #[ip_test(I)]
    fn time_wait_reuse<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>: TcpContext<
                I,
                TcpBindingsCtx<FakeDeviceId>,
                SingleStackConverter = I::SingleStackConverter,
                DualStackConverter = I::DualStackConverter,
            >,
    {
        set_logger_for_test();
        const CLIENT_PORT: NonZeroU16 = NonZeroU16::new(2).unwrap();
        const SERVER_PORT: NonZeroU16 = NonZeroU16::new(1).unwrap();
        let (mut net, local, _local_snd_end, remote) = bind_listen_connect_accept_inner::<I>(
            I::UNSPECIFIED_ADDRESS,
            BindConfig {
                client_port: Some(CLIENT_PORT),
                server_port: SERVER_PORT,
                client_reuse_addr: true,
                send_test_data: false,
            },
            0,
            0.0,
        );
        // Locally, we create a connection with a full accept queue.
        let listener = net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            let listener = api.create(Default::default());
            api.set_reuseaddr(&listener, true).expect("can set");
            api.bind(
                &listener,
                Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)),
                Some(CLIENT_PORT),
            )
            .expect("failed to bind");
            api.listen(&listener, NonZeroUsize::new(1).unwrap()).expect("failed to listen");
            listener
        });
        // This connection is never used, just to keep accept queue full.
        let extra_conn = net.with_context(REMOTE, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            let extra_conn = api.create(Default::default());
            api.connect(&extra_conn, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), CLIENT_PORT)
                .expect("failed to connect");
            extra_conn
        });
        net.run_until_idle();

        net.with_context(REMOTE, |ctx| {
            assert_eq!(
                ctx.tcp_api().connect(
                    &extra_conn,
                    Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)),
                    CLIENT_PORT,
                ),
                Ok(())
            );
        });

        // Now we shutdown the sockets and try to bring the local socket to
        // TIME-WAIT.
        let weak_local = local.downgrade();
        net.with_context(LOCAL, |ctx| {
            ctx.tcp_api().close(local);
        });
        assert!(!net.step().is_idle());
        assert!(!net.step().is_idle());
        net.with_context(REMOTE, |ctx| {
            ctx.tcp_api().close(remote);
        });
        assert!(!net.step().is_idle());
        assert!(!net.step().is_idle());
        // The connection should go to TIME-WAIT.
        let (tw_last_seq, tw_last_ack, tw_expiry) = {
            assert_matches!(
                &weak_local.upgrade().unwrap().get().deref().socket_state,
                TcpSocketStateInner::Connected { conn, .. } => {
                    let (conn, _addr) = assert_this_stack_conn::<I, _, TcpCoreCtx<_, _>>(conn, &I::converter());
                    assert_matches!(
                        conn,
                        Connection {
                        state: State::TimeWait(TimeWait {
                            last_seq,
                            closed_rcv,
                            expiry,
                            ..
                        }), ..
                        } => (*last_seq, closed_rcv.ack, *expiry)
                    )
                }
            )
        };

        // Try to initiate a connection from the remote since we have an active
        // listener locally.
        let conn = net.with_context(REMOTE, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            let conn = api.create(Default::default());
            api.connect(&conn, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), CLIENT_PORT)
                .expect("failed to connect");
            conn
        });
        while net.next_step() != Some(tw_expiry) {
            assert!(!net.step().is_idle());
        }
        // This attempt should fail due the full accept queue at the listener.
        assert_matches!(
        &conn.get().deref().socket_state,
        TcpSocketStateInner::Connected { conn, .. } => {
                let (conn, _addr) = assert_this_stack_conn::<I, _, TcpCoreCtx<_, _>>(conn, &I::converter());
                assert_matches!(
                    conn,
                Connection {
                    state: State::Closed(Closed { reason: Some(ConnectionError::TimedOut) }),
                    ..
                }
                );
            });

        // Now free up the accept queue by accepting the connection.
        net.with_context(LOCAL, |ctx| {
            let _accepted =
                ctx.tcp_api().accept(&listener).expect("failed to accept a new connection");
        });
        let conn = net.with_context(REMOTE, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            let socket = api.create(Default::default());
            api.bind(&socket, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), Some(SERVER_PORT))
                .expect("failed to bind");
            api.connect(&socket, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), CLIENT_PORT)
                .expect("failed to connect");
            socket
        });
        net.collect_frames();
        assert_matches!(
            &net.iter_pending_frames().collect::<Vec<_>>()[..],
            [InstantAndData(_instant, PendingFrameData {
                dst_context: _,
                meta,
                frame,
            })] => {
            let mut buffer = Buf::new(frame, ..);
            let iss = match I::VERSION {
                IpVersion::V4 => {
                    let meta = assert_matches!(meta, DualStackSendIpPacketMeta::V4(meta) => meta);
                    let parsed = buffer.parse_with::<_, TcpSegment<_>>(
                        TcpParseArgs::new(*meta.src_ip, *meta.dst_ip)
                    ).expect("failed to parse");
                    assert!(parsed.syn());
                    SeqNum::new(parsed.seq_num())
                }
                IpVersion::V6 => {
                    let meta = assert_matches!(meta, DualStackSendIpPacketMeta::V6(meta) => meta);
                    let parsed = buffer.parse_with::<_, TcpSegment<_>>(
                        TcpParseArgs::new(*meta.src_ip, *meta.dst_ip)
                    ).expect("failed to parse");
                    assert!(parsed.syn());
                    SeqNum::new(parsed.seq_num())
                }
            };
            assert!(iss.after(tw_last_ack) && iss.before(tw_last_seq));
        });
        // The TIME-WAIT socket should be reused to establish the connection.
        net.run_until_idle();
        net.with_context(REMOTE, |ctx| {
            assert_eq!(
                ctx.tcp_api().connect(
                    &conn,
                    Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)),
                    CLIENT_PORT
                ),
                Ok(())
            );
        });
    }

    #[ip_test(I)]
    fn conn_addr_not_available<I: TcpTestIpExt + IcmpIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>: TcpContext<
                I,
                TcpBindingsCtx<FakeDeviceId>,
                SingleStackConverter = I::SingleStackConverter,
                DualStackConverter = I::DualStackConverter,
            >,
    {
        set_logger_for_test();
        let (mut net, _local, _local_snd_end, _remote) = bind_listen_connect_accept_inner::<I>(
            I::UNSPECIFIED_ADDRESS,
            BindConfig {
                client_port: Some(PORT_1),
                server_port: PORT_1,
                client_reuse_addr: true,
                send_test_data: false,
            },
            0,
            0.0,
        );
        // Now we are using the same 4-tuple again to try to create a new
        // connection, this should fail.
        net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            let socket = api.create(Default::default());
            api.set_reuseaddr(&socket, true).expect("can set");
            api.bind(&socket, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), Some(PORT_1))
                .expect("failed to bind");
            assert_eq!(
                api.connect(&socket, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), PORT_1),
                Err(ConnectError::ConnectionExists),
            )
        });
    }

    #[test_case::test_matrix(
        [None, Some(ZonedAddr::Unzoned((*Ipv4::TEST_ADDRS.remote_ip).to_ipv6_mapped()))],
        [None, Some(PORT_1)],
        [true, false]
    )]
    fn dual_stack_connect(
        server_bind_ip: Option<ZonedAddr<SpecifiedAddr<Ipv6Addr>, FakeDeviceId>>,
        server_bind_port: Option<NonZeroU16>,
        bind_client: bool,
    ) {
        set_logger_for_test();
        let mut net = new_test_net::<Ipv4>();
        let backlog = NonZeroUsize::new(1).unwrap();
        let (server, listen_port) = net.with_context(REMOTE, |ctx| {
            let mut api = ctx.tcp_api::<Ipv6>();
            let server = api.create(Default::default());
            api.bind(&server, server_bind_ip, server_bind_port)
                .expect("failed to bind the server socket");
            api.listen(&server, backlog).expect("can listen");
            let port = assert_matches!(
                api.get_info(&server),
                SocketInfo::Bound(info) => info.port
            );
            (server, port)
        });

        let client_ends = WriteBackClientBuffers::default();
        let client = net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api::<Ipv6>();
            let socket = api.create(ProvidedBuffers::Buffers(client_ends.clone()));
            if bind_client {
                api.bind(&socket, None, None).expect("failed to bind");
            }
            api.connect(
                &socket,
                Some(ZonedAddr::Unzoned((*Ipv4::TEST_ADDRS.remote_ip).to_ipv6_mapped())),
                listen_port,
            )
            .expect("failed to connect");
            socket
        });

        // Step the test network until the handshake is done.
        net.run_until_idle();
        let (accepted, addr, accepted_ends) = net
            .with_context(REMOTE, |ctx| ctx.tcp_api().accept(&server).expect("failed to accept"));
        assert_eq!(addr.ip, ZonedAddr::Unzoned((*Ipv4::TEST_ADDRS.local_ip).to_ipv6_mapped()));

        let ClientBuffers { send: client_snd_end, receive: client_rcv_end } =
            client_ends.0.as_ref().lock().take().unwrap();
        let ClientBuffers { send: accepted_snd_end, receive: accepted_rcv_end } = accepted_ends;
        for snd_end in [client_snd_end, accepted_snd_end] {
            snd_end.lock().extend_from_slice(b"Hello");
        }
        net.with_context(LOCAL, |ctx| ctx.tcp_api().do_send(&client));
        net.with_context(REMOTE, |ctx| ctx.tcp_api().do_send(&accepted));
        net.run_until_idle();

        for rcv_end in [client_rcv_end, accepted_rcv_end] {
            assert_eq!(
                rcv_end.lock().read_with(|avail| {
                    let avail = avail.concat();
                    assert_eq!(avail, b"Hello");
                    avail.len()
                }),
                5
            );
        }

        // Verify that the client is connected to the IPv4 remote and has been
        // assigned an IPv4 local IP.
        let info = assert_matches!(
            net.with_context(LOCAL, |ctx| ctx.tcp_api().get_info(&client)),
            SocketInfo::Connection(info) => info
        );
        let (local_ip, remote_ip, port) = assert_matches!(
            info,
            ConnectionInfo {
                local_addr: SocketAddr { ip: local_ip, port: _ },
                remote_addr: SocketAddr { ip: remote_ip, port },
                device: _
            } => (local_ip.addr(), remote_ip.addr(), port)
        );
        assert_eq!(remote_ip, Ipv4::TEST_ADDRS.remote_ip.to_ipv6_mapped());
        assert_matches!(local_ip.to_ipv4_mapped(), Some(_));
        assert_eq!(port, listen_port);
    }

    #[test]
    fn ipv6_dual_stack_enabled() {
        set_logger_for_test();
        let mut net = new_test_net::<Ipv4>();
        net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api::<Ipv6>();
            let socket = api.create(Default::default());
            assert_eq!(api.dual_stack_enabled(&socket), Ok(true));
            api.set_dual_stack_enabled(&socket, false).expect("failed to disable dual stack");
            assert_eq!(api.dual_stack_enabled(&socket), Ok(false));
            assert_eq!(
                api.bind(
                    &socket,
                    Some(ZonedAddr::Unzoned((*Ipv4::TEST_ADDRS.local_ip).to_ipv6_mapped())),
                    Some(PORT_1),
                ),
                Err(BindError::LocalAddressError(LocalAddressError::CannotBindToAddress))
            );
            assert_eq!(
                api.connect(
                    &socket,
                    Some(ZonedAddr::Unzoned((*Ipv4::TEST_ADDRS.remote_ip).to_ipv6_mapped())),
                    PORT_1,
                ),
                Err(ConnectError::NoRoute)
            );
        });
    }

    #[test]
    fn ipv4_dual_stack_enabled() {
        set_logger_for_test();
        let mut net = new_test_net::<Ipv4>();
        net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api::<Ipv4>();
            let socket = api.create(Default::default());
            assert_eq!(api.dual_stack_enabled(&socket), Err(NotDualStackCapableError));
            assert_eq!(
                api.set_dual_stack_enabled(&socket, true),
                Err(NotDualStackCapableError.into())
            );
        });
    }

    #[ip_test(I)]
    fn closed_not_in_demux<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>: TcpContext<
                I,
                TcpBindingsCtx<FakeDeviceId>,
                SingleStackConverter = I::SingleStackConverter,
                DualStackConverter = I::DualStackConverter,
            >,
    {
        let (mut net, local, _local_snd_end, remote) = bind_listen_connect_accept_inner::<I>(
            I::UNSPECIFIED_ADDRESS,
            BindConfig {
                client_port: None,
                server_port: PORT_1,
                client_reuse_addr: false,
                send_test_data: false,
            },
            0,
            0.0,
        );
        // Assert that the sockets are bound in the socketmap.
        for ctx_name in [LOCAL, REMOTE] {
            net.with_context(ctx_name, |CtxPair { core_ctx, bindings_ctx: _ }| {
                TcpDemuxContext::<I, _, _>::with_demux(core_ctx, |DemuxState { socketmap }| {
                    assert_eq!(socketmap.len(), 1);
                })
            });
        }
        for (ctx_name, socket) in [(LOCAL, &local), (REMOTE, &remote)] {
            net.with_context(ctx_name, |ctx| {
                assert_eq!(ctx.tcp_api().shutdown(socket, ShutdownType::SendAndReceive), Ok(true));
            });
        }
        net.run_until_idle();
        // Both sockets are closed by now, but they are not defunct because we
        // never called `close` on them, but they should not be in the demuxer
        // regardless.
        for ctx_name in [LOCAL, REMOTE] {
            net.with_context(ctx_name, |CtxPair { core_ctx, bindings_ctx: _ }| {
                TcpDemuxContext::<I, _, _>::with_demux(core_ctx, |DemuxState { socketmap }| {
                    assert_eq!(socketmap.len(), 0);
                })
            });
        }
    }

    #[ip_test(I)]
    fn tcp_accept_queue_clean_up_closed<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        let mut net = new_test_net::<I>();
        let backlog = NonZeroUsize::new(1).unwrap();
        let server_port = NonZeroU16::new(1024).unwrap();
        let server = net.with_context(REMOTE, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            let server = api.create(Default::default());
            api.bind(&server, None, Some(server_port)).expect("failed to bind the server socket");
            api.listen(&server, backlog).expect("can listen");
            server
        });

        let client = net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            let socket = api.create(ProvidedBuffers::Buffers(WriteBackClientBuffers::default()));
            api.connect(&socket, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), server_port)
                .expect("failed to connect");
            socket
        });
        // Step so that SYN is received by the server.
        assert!(!net.step().is_idle());
        // Make sure the server now has a pending socket in the accept queue.
        assert_matches!(
            &server.get().deref().socket_state,
            TcpSocketStateInner::Listener(Listener { accept_queue, .. }) => {
                assert_eq!(accept_queue.ready_len(), 0);
                assert_eq!(accept_queue.pending_len(), 1);
            }
        );
        // Now close the client socket.
        net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            api.close(client);
        });
        // Server's SYN-ACK will get a RST response because the connection is
        // no longer there.
        net.run_until_idle();
        // We verify that no lingering socket in the accept_queue.
        assert_matches!(
            &server.get().deref().socket_state,
            TcpSocketStateInner::Listener(Listener { accept_queue, .. }) => {
                assert_eq!(accept_queue.ready_len(), 0);
                assert_eq!(accept_queue.pending_len(), 0);
            }
        );
        // Server should be the only socket in `all_sockets`.
        net.with_context(REMOTE, |ctx| {
            ctx.core_ctx.with_all_sockets_mut(|all_sockets| {
                assert_eq!(all_sockets.keys().collect::<Vec<_>>(), [&server]);
            })
        })
    }

    #[ip_test(I, test = false)]
    #[test_case::test_matrix(
        [MarkDomain::Mark1, MarkDomain::Mark2],
        [None, Some(0), Some(1)]
    )]
    fn tcp_socket_marks<I: TcpTestIpExt>(domain: MarkDomain, mark: Option<u32>)
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<I>();
        let socket = api.create(Default::default());

        // Doesn't have a mark by default.
        assert_eq!(api.get_mark(&socket, domain), Mark(None));

        let mark = Mark(mark);
        // We can set and get back the mark.
        api.set_mark(&socket, domain, mark);
        assert_eq!(api.get_mark(&socket, domain), mark);
    }

    #[ip_test(I)]
    fn tcp_marks_for_accepted_sockets<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>: TcpContext<
                I,
                TcpBindingsCtx<FakeDeviceId>,
                SingleStackConverter = I::SingleStackConverter,
                DualStackConverter = I::DualStackConverter,
            >,
    {
        // We want the accepted socket to be marked 101 for MARK_1 and 102 for MARK_2.
        let expected_marks = [(MarkDomain::Mark1, 101), (MarkDomain::Mark2, 102)];
        let marks = netstack3_base::Marks::new(expected_marks);
        let mut net = new_test_net::<I>();

        for c in [LOCAL, REMOTE] {
            net.with_context(c, |ctx| {
                ctx.core_ctx.recv_packet_marks = marks;
            })
        }

        let backlog = NonZeroUsize::new(1).unwrap();
        let server_port = NonZeroU16::new(1234).unwrap();

        let server = net.with_context(REMOTE, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            let server = api.create(Default::default());
            api.set_mark(&server, MarkDomain::Mark1, Mark(Some(1)));
            api.bind(&server, None, Some(server_port)).expect("failed to bind the server socket");
            api.listen(&server, backlog).expect("can listen");
            server
        });

        let client_ends = WriteBackClientBuffers::default();
        let _client = net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            let socket = api.create(ProvidedBuffers::Buffers(client_ends.clone()));
            api.connect(&socket, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), server_port)
                .expect("failed to connect");
            socket
        });
        net.run_until_idle();
        net.with_context(REMOTE, |ctx| {
            let (accepted, _addr, _accepted_ends) =
                ctx.tcp_api::<I>().accept(&server).expect("failed to accept");
            for (domain, expected) in expected_marks {
                assert_eq!(ctx.tcp_api::<I>().get_mark(&accepted, domain), Mark(Some(expected)));
            }
        });
    }

    #[ip_test(I)]
    fn do_send_can_remove_sockets_from_demux_state<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>: TcpContext<
                I,
                TcpBindingsCtx<FakeDeviceId>,
                SingleStackConverter = I::SingleStackConverter,
                DualStackConverter = I::DualStackConverter,
            >,
    {
        let (mut net, client, _client_snd_end, accepted) = bind_listen_connect_accept_inner(
            I::UNSPECIFIED_ADDRESS,
            BindConfig {
                client_port: None,
                server_port: PORT_1,
                client_reuse_addr: false,
                send_test_data: false,
            },
            0,
            0.0,
        );
        net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            assert_eq!(api.shutdown(&client, ShutdownType::Send), Ok(true));
        });
        // client -> accepted FIN.
        assert!(!net.step().is_idle());
        // accepted -> client ACK.
        assert!(!net.step().is_idle());
        net.with_context(REMOTE, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            assert_eq!(api.shutdown(&accepted, ShutdownType::Send), Ok(true));
        });
        // accepted -> client FIN.
        assert!(!net.step().is_idle());
        // client -> accepted ACK.
        assert!(!net.step().is_idle());

        // client is now in TIME_WAIT
        net.with_context(LOCAL, |CtxPair { core_ctx, bindings_ctx: _ }| {
            TcpDemuxContext::<I, _, _>::with_demux(core_ctx, |DemuxState { socketmap }| {
                assert_eq!(socketmap.len(), 1);
            })
        });
        assert_matches!(
            &client.get().deref().socket_state,
            TcpSocketStateInner::Connected { conn, .. } => {
                let (conn, _addr) = assert_this_stack_conn::<I, _, TcpCoreCtx<_, _>>(
                    conn,
                    &I::converter()
                );
                assert_matches!(
                    conn,
                    Connection {
                        state: State::TimeWait(_),
                        ..
                    }
                );
            }
        );
        net.with_context(LOCAL, |ctx| {
            // Advance the current time but don't fire the timer.
            ctx.with_fake_timer_ctx_mut(|ctx| {
                ctx.instant.time =
                    ctx.instant.time.checked_add(Duration::from_secs(120 * 60)).unwrap()
            });
            // Race with `do_send`.
            let mut api = ctx.tcp_api::<I>();
            api.do_send(&client);
        });
        assert_matches!(
            &client.get().deref().socket_state,
            TcpSocketStateInner::Connected { conn, .. } => {
                let (conn, _addr) = assert_this_stack_conn::<I, _, TcpCoreCtx<_, _>>(
                    conn,
                    &I::converter()
                );
                assert_matches!(
                    conn,
                    Connection {
                        state: State::Closed(_),
                        ..
                    }
                );
            }
        );
        net.with_context(LOCAL, |CtxPair { core_ctx, bindings_ctx: _ }| {
            TcpDemuxContext::<I, _, _>::with_demux(core_ctx, |DemuxState { socketmap }| {
                assert_eq!(socketmap.len(), 0);
            })
        });
    }

    #[ip_test(I)]
    #[test_case(true; "server read over mss")]
    #[test_case(false; "server read under mss")]
    fn tcp_data_dequeue_sends_window_update<I: TcpTestIpExt>(server_read_over_mss: bool)
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>: TcpContext<
                I,
                TcpBindingsCtx<FakeDeviceId>,
                SingleStackConverter = I::SingleStackConverter,
                DualStackConverter = I::DualStackConverter,
            >,
    {
        const EXTRA_DATA_AMOUNT: usize = 128;
        set_logger_for_test();

        let (mut net, client, client_snd_end, accepted) = bind_listen_connect_accept_inner(
            I::UNSPECIFIED_ADDRESS,
            BindConfig {
                client_port: None,
                server_port: PORT_1,
                client_reuse_addr: false,
                send_test_data: false,
            },
            0,
            0.0,
        );

        let accepted_rcv_bufsize = net
            .with_context(REMOTE, |ctx| ctx.tcp_api::<I>().receive_buffer_size(&accepted).unwrap());

        // Send enough data to the server to fill up its receive buffer.
        client_snd_end.lock().extend(core::iter::repeat(0xAB).take(accepted_rcv_bufsize));
        net.with_context(LOCAL, |ctx| {
            ctx.tcp_api().do_send(&client);
        });
        net.run_until_idle();

        // From now on, we don't want to trigger timers
        // because that would result in either:
        // 1. The client to time out, since the server isn't going to read any
        //    data from its buffer.
        // 2. ZWP from the client, which would make this test pointless.

        // Push extra data into the send buffer that won't be sent because the
        // receive window is zero.
        client_snd_end.lock().extend(core::iter::repeat(0xAB).take(EXTRA_DATA_AMOUNT));
        net.with_context(LOCAL, |ctx| {
            ctx.tcp_api().do_send(&client);
        });
        let _ = net.step_deliver_frames();

        let send_buf_len = net
            .with_context(LOCAL, |ctx| {
                ctx.tcp_api::<I>().with_send_buffer(&client, |buf| {
                    let BufferLimits { len, capacity: _ } = buf.limits();
                    len
                })
            })
            .unwrap();
        assert_eq!(send_buf_len, EXTRA_DATA_AMOUNT);

        if server_read_over_mss {
            // Clear out the receive buffer
            let nread = net
                .with_context(REMOTE, |ctx| {
                    ctx.tcp_api::<I>().with_receive_buffer(&accepted, |buf| {
                        buf.lock().read_with(|readable| readable.iter().map(|buf| buf.len()).sum())
                    })
                })
                .unwrap();
            assert_eq!(nread, accepted_rcv_bufsize);

            // The server sends a window update because the window went from 0 to
            // larger than MSS.
            net.with_context(REMOTE, |ctx| ctx.tcp_api::<I>().on_receive_buffer_read(&accepted));

            let (server_snd_max, server_acknum) = {
                let socket = accepted.get();
                let state = assert_matches!(
                    &socket.deref().socket_state,
                    TcpSocketStateInner::Connected { conn, .. } => {
                        assert_matches!(I::get_state(conn), State::Established(e) => e)
                    }
                );

                (state.snd.max, state.rcv.nxt())
            };

            // Deliver the window update to the client.
            assert_eq!(
                net.step_deliver_frames_with(|_, meta, frame| {
                    let mut buffer = Buf::new(frame.clone(), ..);

                    let (packet_seq, packet_ack, window_size, body_len) = match I::VERSION {
                        IpVersion::V4 => {
                            let meta =
                                assert_matches!(&meta, DualStackSendIpPacketMeta::V4(v4) => v4);

                            // Server -> Client.
                            assert_eq!(*meta.src_ip, Ipv4::TEST_ADDRS.remote_ip.into_addr());
                            assert_eq!(*meta.dst_ip, Ipv4::TEST_ADDRS.local_ip.into_addr());

                            let parsed = buffer
                                .parse_with::<_, TcpSegment<_>>(TcpParseArgs::new(
                                    *meta.src_ip,
                                    *meta.dst_ip,
                                ))
                                .expect("failed to parse");

                            (
                                parsed.seq_num(),
                                parsed.ack_num().unwrap(),
                                parsed.window_size(),
                                parsed.body().len(),
                            )
                        }
                        IpVersion::V6 => {
                            let meta =
                                assert_matches!(&meta, DualStackSendIpPacketMeta::V6(v6) => v6);

                            // Server -> Client.
                            assert_eq!(*meta.src_ip, Ipv6::TEST_ADDRS.remote_ip.into_addr());
                            assert_eq!(*meta.dst_ip, Ipv6::TEST_ADDRS.local_ip.into_addr());

                            let parsed = buffer
                                .parse_with::<_, TcpSegment<_>>(TcpParseArgs::new(
                                    *meta.src_ip,
                                    *meta.dst_ip,
                                ))
                                .expect("failed to parse");

                            (
                                parsed.seq_num(),
                                parsed.ack_num().unwrap(),
                                parsed.window_size(),
                                parsed.body().len(),
                            )
                        }
                    };

                    // Ensure that this is actually a window update, and no data
                    // is being sent or ACKed.
                    assert_eq!(packet_seq, u32::from(server_snd_max));
                    assert_eq!(packet_ack, u32::from(server_acknum));
                    assert_eq!(window_size, 65535);
                    assert_eq!(body_len, 0);

                    Some((meta, frame))
                })
                .frames_sent,
                1
            );

            // Deliver the data send to the server.
            assert_eq!(
                net.step_deliver_frames_with(|_, meta, frame| {
                    let mut buffer = Buf::new(frame.clone(), ..);

                    let body_len = match I::VERSION {
                        IpVersion::V4 => {
                            let meta =
                                assert_matches!(&meta, DualStackSendIpPacketMeta::V4(v4) => v4);

                            // Client -> Server.
                            assert_eq!(*meta.src_ip, Ipv4::TEST_ADDRS.local_ip.into_addr());
                            assert_eq!(*meta.dst_ip, Ipv4::TEST_ADDRS.remote_ip.into_addr());

                            let parsed = buffer
                                .parse_with::<_, TcpSegment<_>>(TcpParseArgs::new(
                                    *meta.src_ip,
                                    *meta.dst_ip,
                                ))
                                .expect("failed to parse");

                            parsed.body().len()
                        }
                        IpVersion::V6 => {
                            let meta =
                                assert_matches!(&meta, DualStackSendIpPacketMeta::V6(v6) => v6);

                            // Client -> Server.
                            assert_eq!(*meta.src_ip, Ipv6::TEST_ADDRS.local_ip.into_addr());
                            assert_eq!(*meta.dst_ip, Ipv6::TEST_ADDRS.remote_ip.into_addr());

                            let parsed = buffer
                                .parse_with::<_, TcpSegment<_>>(TcpParseArgs::new(
                                    *meta.src_ip,
                                    *meta.dst_ip,
                                ))
                                .expect("failed to parse");

                            parsed.body().len()
                        }
                    };

                    assert_eq!(body_len, EXTRA_DATA_AMOUNT);

                    Some((meta, frame))
                })
                .frames_sent,
                1
            );

            // Deliver the ACK of the data send to the client so it will flush the
            // data from its buffers.
            assert_eq!(
                net.step_deliver_frames_with(|_, meta, frame| {
                    let mut buffer = Buf::new(frame.clone(), ..);

                    let (packet_seq, packet_ack, body_len) = match I::VERSION {
                        IpVersion::V4 => {
                            let meta =
                                assert_matches!(&meta, DualStackSendIpPacketMeta::V4(v4) => v4);

                            // Server -> Client.
                            assert_eq!(*meta.src_ip, Ipv4::TEST_ADDRS.remote_ip.into_addr());
                            assert_eq!(*meta.dst_ip, Ipv4::TEST_ADDRS.local_ip.into_addr());

                            let parsed = buffer
                                .parse_with::<_, TcpSegment<_>>(TcpParseArgs::new(
                                    *meta.src_ip,
                                    *meta.dst_ip,
                                ))
                                .expect("failed to parse");

                            (parsed.seq_num(), parsed.ack_num().unwrap(), parsed.body().len())
                        }
                        IpVersion::V6 => {
                            let meta =
                                assert_matches!(&meta, DualStackSendIpPacketMeta::V6(v6) => v6);

                            // Server -> Client.
                            assert_eq!(*meta.src_ip, Ipv6::TEST_ADDRS.remote_ip.into_addr());
                            assert_eq!(*meta.dst_ip, Ipv6::TEST_ADDRS.local_ip.into_addr());

                            let parsed = buffer
                                .parse_with::<_, TcpSegment<_>>(TcpParseArgs::new(
                                    *meta.src_ip,
                                    *meta.dst_ip,
                                ))
                                .expect("failed to parse");

                            (parsed.seq_num(), parsed.ack_num().unwrap(), parsed.body().len())
                        }
                    };

                    assert_eq!(packet_seq, u32::from(server_snd_max));
                    assert_eq!(
                        packet_ack,
                        u32::from(server_acknum) + u32::try_from(EXTRA_DATA_AMOUNT).unwrap()
                    );
                    assert_eq!(body_len, 0);

                    Some((meta, frame))
                })
                .frames_sent,
                1
            );

            let send_buf_len = net
                .with_context(LOCAL, |ctx| {
                    ctx.tcp_api::<I>().with_send_buffer(&client, |buf| {
                        let BufferLimits { len, capacity: _ } = buf.limits();
                        len
                    })
                })
                .unwrap();
            assert_eq!(send_buf_len, 0);
        } else {
            // Read a single byte out of the receive buffer, which is guaranteed
            // to be less than MSS.
            let nread = net
                .with_context(REMOTE, |ctx| {
                    ctx.tcp_api::<I>()
                        .with_receive_buffer(&accepted, |buf| buf.lock().read_with(|_readable| 1))
                })
                .unwrap();
            assert_eq!(nread, 1);

            // The server won't send a window update because it wouldn't be
            // advertising a window that's larger than the MSS.
            net.with_context(REMOTE, |ctx| ctx.tcp_api::<I>().on_receive_buffer_read(&accepted));
            assert_eq!(net.step_deliver_frames().frames_sent, 0);

            let send_buf_len = net
                .with_context(LOCAL, |ctx| {
                    ctx.tcp_api::<I>().with_send_buffer(&client, |buf| {
                        let BufferLimits { len, capacity: _ } = buf.limits();
                        len
                    })
                })
                .unwrap();
            // The client didn't hear about the data being read, since no window
            // update was sent.
            assert_eq!(send_buf_len, EXTRA_DATA_AMOUNT);
        }
    }

    impl<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes> TcpSocketId<I, D, BT> {
        fn established_state(
            state: &impl Deref<Target = TcpSocketState<I, D, BT>>,
        ) -> &Established<BT::Instant, BT::ReceiveBuffer, BT::SendBuffer> {
            assert_matches!(
                &state.deref().socket_state,
                TcpSocketStateInner::Connected { conn, .. } => {
                    assert_matches!(I::get_state(conn), State::Established(e) => e)
                }
            )
        }

        fn mss(&self) -> Mss {
            *Self::established_state(&self.get()).snd.congestion_control().mss().mss()
        }

        fn cwnd(&self) -> CongestionWindow {
            Self::established_state(&self.get()).snd.congestion_control().inspect_cwnd()
        }
    }

    #[derive(PartialEq)]
    enum MssUpdate {
        Decrease,
        DecreaseBelowMin,
        Same,
        Increase,
    }

    #[ip_test(I)]
    #[test_case(MssUpdate::Decrease; "update if decrease")]
    #[test_case(MssUpdate::DecreaseBelowMin; "update to min if decreased below min")]
    #[test_case(MssUpdate::Same; "ignore if same")]
    #[test_case(MssUpdate::Increase; "ignore if increase")]
    fn pmtu_update_mss<I: TcpTestIpExt + IcmpIpExt>(mss_update: MssUpdate)
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>: TcpContext<I, TcpBindingsCtx<FakeDeviceId>>
            + TcpContext<I::OtherVersion, TcpBindingsCtx<FakeDeviceId>>,
    {
        let mut net = new_test_net::<I>();

        let server = net.with_context(REMOTE, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            let server = api.create(Default::default());
            api.bind(&server, None, Some(PORT_1)).expect("bind to port");
            api.listen(&server, NonZeroUsize::MIN).expect("can listen");
            server
        });

        let client_buffers = WriteBackClientBuffers::default();
        let client = net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            let client = api.create(ProvidedBuffers::Buffers(client_buffers.clone()));
            api.connect(&client, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), PORT_1)
                .expect("connect to server");
            client
        });

        // Allow the connection to be established.
        net.run_until_idle();
        let (_accepted, accepted_buffers) = net.with_context(REMOTE, |ctx| {
            let (accepted, _addr, accepted_ends) =
                ctx.tcp_api::<I>().accept(&server).expect("accept incoming connection");
            (accepted, accepted_ends)
        });

        let initial_mss = client.mss();

        // The minimum link MTU needed to support TCP connections.
        let min_mtu = u32::from(Mss::MIN)
            + I::IP_HEADER_LENGTH.get()
            + packet_formats::tcp::HDR_PREFIX_LEN as u32;

        let pmtu_update = match mss_update {
            MssUpdate::DecreaseBelowMin => Mtu::new(min_mtu - 1),
            MssUpdate::Decrease => Mtu::new(min_mtu),
            MssUpdate::Same => LINK_MTU,
            MssUpdate::Increase => Mtu::max(),
        };
        let icmp_error = I::map_ip(
            (),
            |()| {
                let mtu = u16::try_from(pmtu_update.get()).unwrap_or(u16::MAX);
                let mtu = NonZeroU16::new(mtu).unwrap();
                Icmpv4ErrorCode::DestUnreachable(
                    Icmpv4DestUnreachableCode::FragmentationRequired,
                    IcmpDestUnreachable::new_for_frag_req(mtu),
                )
            },
            |()| Icmpv6ErrorCode::PacketTooBig(pmtu_update),
        );

        // Send a payload that is large enough that it will need to be re-segmented if
        // the PMTU decreases, and deliver a PMTU update.
        let ClientBuffers { send: client_snd_end, receive: _ } =
            client_buffers.0.as_ref().lock().take().unwrap();
        let payload = vec![0xFF; min_mtu.try_into().unwrap()];
        client_snd_end.lock().extend_from_slice(&payload);
        net.with_context(LOCAL, |ctx| {
            ctx.tcp_api().do_send(&client);
            let (core_ctx, bindings_ctx) = ctx.contexts();
            let frames = core_ctx.ip_socket_ctx.take_frames();
            let frame = assert_matches!(&frames[..], [(_meta, frame)] => frame);

            deliver_icmp_error::<I, _, _>(
                core_ctx,
                bindings_ctx,
                I::TEST_ADDRS.local_ip,
                I::TEST_ADDRS.remote_ip,
                &frame[0..8],
                icmp_error,
            );
        });

        let requested_mms = Mms::from_mtu::<I>(pmtu_update, 0 /* no IP options */).unwrap();
        let requested_mss = Mss::from_mms(requested_mms);
        match mss_update {
            MssUpdate::DecreaseBelowMin => {
                // NB: The requested MSS is invalid.
                assert_eq!(requested_mss, None);
            }
            MssUpdate::Decrease => {
                assert_matches!(requested_mss, Some(mss) if mss < initial_mss);
            }
            MssUpdate::Same => {
                assert_eq!(requested_mss, Some(initial_mss));
            }
            MssUpdate::Increase => {
                assert_matches!(requested_mss, Some(mss) if mss > initial_mss);
            }
        };

        // The socket should only update its MSS if the new MSS is a decrease.
        match mss_update {
            MssUpdate::Decrease | MssUpdate::DecreaseBelowMin => {}
            MssUpdate::Same | MssUpdate::Increase => {
                assert_eq!(client.mss(), initial_mss);
                return;
            }
        }

        // Note the MSS & MMS used by the stack will be clamped to minimum valid
        // value.
        let expected_mss = requested_mss.unwrap_or(Mss::MIN);
        let expected_mms = usize::from(expected_mss) + packet_formats::tcp::HDR_PREFIX_LEN;

        assert_eq!(client.mss(), expected_mss);
        // The PMTU update should not represent a congestion event.
        assert_gt!(client.cwnd().cwnd(), u32::from(expected_mss));

        // The segment that was too large should be eagerly retransmitted.
        net.with_context(LOCAL, |ctx| {
            let frames = ctx.core_ctx().ip_socket_ctx.frames();
            let frame = assert_matches!(&frames[..], [(_meta, frame)] => frame);
            assert_eq!(frame.len(), expected_mms);
        });

        // The remaining in-flight segment(s) are retransmitted via the retransmission
        // timer (rather than immediately).
        net.run_until_idle();
        let ClientBuffers { send: _, receive: accepted_rcv_end } = accepted_buffers;
        let read = accepted_rcv_end.lock().read_with(|avail| {
            let avail = avail.concat();
            assert_eq!(avail, payload);
            avail.len()
        });
        assert_eq!(read, payload.len());
    }

    #[ip_test(I)]
    fn connect_timeout<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>: TcpContext<
                I,
                TcpBindingsCtx<FakeDeviceId>,
                SingleStackConverter = I::SingleStackConverter,
                DualStackConverter = I::DualStackConverter,
            >,
    {
        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<I>();
        let socket = api.create(Default::default());
        let connect_addr = Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip));
        api.connect(&socket, connect_addr, PORT_1).expect("first connect should succeed");
        assert_eq!(api.connect(&socket, connect_addr, PORT_1), Err(ConnectError::Pending));
        while let Some(id) = ctx.bindings_ctx.timers.pop_next_timer_and_advance_time() {
            let mut api = ctx.tcp_api::<I>();
            assert_eq!(api.connect(&socket, connect_addr, PORT_1), Err(ConnectError::Pending));
            api.handle_timer(id.assert_ip_version());
        }
        // Once we timeout, we're done and an error should be reported.
        let mut api = ctx.tcp_api::<I>();
        assert_eq!(
            api.connect(&socket, connect_addr, PORT_1),
            Err(ConnectError::ConnectionError(ConnectionError::TimedOut))
        );

        // NB: This matches the linux behavior. Whenever errors are reported
        // from the connect call, they're not observable from SO_ERROR later.
        assert_eq!(api.get_socket_error(&socket), None);
    }

    // Regression test for https://issues.fuchsia.dev/475191687.
    #[test]
    fn conflict_with_same_link_local_addr_on_different_interfaces() {
        set_logger_for_test();
        const LOCAL_IP: Ipv6Addr = net_ip_v6!("fe80::1");
        const REMOTE_IP: Ipv6Addr = net_ip_v6!("fe80::2");
        const LOCAL_PORT: NonZeroU16 = NonZeroU16::new(12345).unwrap();
        const REMOTE_PORT: NonZeroU16 = NonZeroU16::new(54321).unwrap();

        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::with_ip_socket_ctx_state(
            FakeDualStackIpSocketCtx::new(MultipleDevicesId::all().into_iter().map(|device| {
                FakeDeviceConfig {
                    device,
                    local_ips: vec![SpecifiedAddr::new(LOCAL_IP).unwrap()],
                    remote_ips: vec![SpecifiedAddr::new(REMOTE_IP).unwrap()],
                }
            })),
        ));
        let mut api = ctx.tcp_api::<Ipv6>();
        let socket = api.create(Default::default());
        api.bind(&socket, None, Some(LOCAL_PORT)).expect("bind should succeed");

        api.listen(&socket, NonZeroUsize::new(5).unwrap()).unwrap();

        let mut builder =
            TcpSegmentBuilder::new(REMOTE_IP, LOCAL_IP, REMOTE_PORT, LOCAL_PORT, 1, None, u16::MAX);
        builder.syn(true);
        let syn = builder
            .wrap_body(Buf::new(vec![], ..))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .into_inner();

        <TcpIpTransportContext as IpTransportContext<Ipv6, _, _>>::receive_ip_packet(
            &mut ctx.core_ctx,
            &mut ctx.bindings_ctx,
            &MultipleDevicesId::A,
            Ipv6::recv_src_addr(REMOTE_IP),
            SpecifiedAddr::new(LOCAL_IP).unwrap(),
            syn.clone(),
            &mut Default::default(),
            None,
        )
        .expect("failed to deliver bytes");

        <TcpIpTransportContext as IpTransportContext<Ipv6, _, _>>::receive_ip_packet(
            &mut ctx.core_ctx,
            &mut ctx.bindings_ctx,
            &MultipleDevicesId::B,
            Ipv6::recv_src_addr(REMOTE_IP),
            SpecifiedAddr::new(LOCAL_IP).unwrap(),
            syn,
            &mut Default::default(),
            None,
        )
        .expect("failed to deliver bytes");
    }

    #[ip_test(I)]
    fn multicast_syn_ignored<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>: TcpContext<I, TcpBindingsCtx<FakeDeviceId>>
            + TcpContext<I::OtherVersion, TcpBindingsCtx<FakeDeviceId>>,
    {
        set_logger_for_test();
        let local_ip = I::TEST_ADDRS.local_ip;
        let remote_ip = I::TEST_ADDRS.remote_ip;

        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(local_ip, remote_ip));
        let mut api = ctx.tcp_api::<I>();
        let listener = api.create(Default::default());
        api.bind(&listener, None, Some(PORT_1)).expect("bind should succeed");
        api.listen(&listener, NonZeroUsize::new(5).unwrap()).unwrap();

        let multicast_ip = I::map_ip((), |()| net_ip_v4!("224.0.0.1"), |()| net_ip_v6!("ff02::1"));
        let multicast_addr = SpecifiedAddr::new(multicast_ip).unwrap();

        let mut builder =
            TcpSegmentBuilder::new(*remote_ip, *multicast_addr, PORT_2, PORT_1, 1, None, u16::MAX);
        builder.syn(true);
        let syn = builder
            .wrap_body(Buf::new(vec![], ..))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .into_inner();

        let (core_ctx, bindings_ctx) = api.contexts();

        <TcpIpTransportContext as IpTransportContext<I, _, _>>::receive_ip_packet(
            core_ctx,
            bindings_ctx,
            &FakeDeviceId,
            I::recv_src_addr(*remote_ip),
            multicast_addr,
            syn,
            &mut Default::default(),
            None,
        )
        .expect("failed to deliver bytes");

        assert_eq!(
            CounterContext::<TcpCountersWithoutSocket<I>>::counters(core_ctx)
                .as_ref()
                .invalid_ip_addrs_received
                .get(),
            1
        );

        assert_matches!(
            &listener.get().deref().socket_state,
            TcpSocketStateInner::Listener(Listener { accept_queue, .. }) => {
                assert_eq!(accept_queue.ready_len(), 0);
                assert_eq!(accept_queue.pending_len(), 0);
            }
        );
    }
}
