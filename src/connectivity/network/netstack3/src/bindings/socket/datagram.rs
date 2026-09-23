// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Datagram socket bindings.

use std::convert::{Infallible as Never, TryInto as _};
use std::fmt::Debug;
use std::hash::Hash;
use std::num::{NonZeroU8, NonZeroU16, NonZeroU64, NonZeroUsize, TryFromIntError};
use std::ops::{ControlFlow, Deref};

use either::Either;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_posix as fposix;
use fidl_fuchsia_posix_socket as fposix_socket;
use netstack3_core::types::BufferSizeSettings;
use netstack3_core::{MapDerefExt as _, PendingDatagramSocketError};

use derivative::Derivative;
use explicit::ResultExt as _;
use fidl::endpoints::{DiscoverableProtocolMarker as _, RequestStream as _};
use fuchsia_async as fasync;
use log::{debug, trace, warn};
use net_types::ip::{GenericOverIp, Ip, IpInvariant, IpVersion, Ipv4, Ipv4Addr, Ipv6};
use net_types::{
    MulticastAddr, MulticastAddress, SpecifiedAddr, SpecifiedAddress, Witness, ZonedAddr,
};
use netstack3_core::device::{DeviceId, WeakDeviceId};
use netstack3_core::error::{LocalAddressError, NotSupportedError, SocketError};
use netstack3_core::icmp::ReceiveIcmpEchoError;
use netstack3_core::ip::{IpSockCreateAndSendError, IpSockSendError, Mark, MarkDomain};
use netstack3_core::routes::{RoutableIpAddr, RouteResolveOptions};
use netstack3_core::socket::{
    self as core_socket, ConnInfo, ConnectError, ExpectedConnError, ExpectedUnboundError,
    ListenerInfo, MulticastInterfaceSelector, MulticastMembershipInterfaceSelector,
    NotDualStackCapableError, ReusePortOption, SetDualStackEnabledError,
    SetMulticastMembershipError, SharingDomain, ShutdownType, SocketCookie, SocketInfo,
};
use netstack3_core::sync::Mutex as CoreMutex;
use netstack3_core::trace::trace_duration;
use netstack3_core::udp::UdpPacketMeta;
use netstack3_core::{IpExt, icmp, udp};
use packet::{Buf, BufferMut};
use packet_formats::ip::DscpAndEcn;
use thiserror::Error;

use crate::bindings::errno::ErrnoError;
use crate::bindings::error::Error;
use crate::bindings::socket::event_pair::SocketEventPair;
use crate::bindings::socket::queue::{BodyLen, MessageQueue, NoSpace, QueueReadableListener as _};
use crate::bindings::socket::worker::{self, SocketWorker};
use crate::bindings::util::{
    DeviceNotFoundError, ErrnoResultExt as _, IntoCore as _, IntoFidl, OpAndAddr,
    RemoveResourceResultExt as _, ResultExt as _, ScopeExt as _, TryFromFidlWithContext,
    TryIntoCore, TryIntoCoreWithContext, TryIntoFidl, TryIntoFidlWithContext,
};
use crate::bindings::waker::WakeGroupId;
use crate::bindings::{BindingId, BindingsCtx, Ctx};

use super::{IntoErrno, IpSockAddrExt, SockAddr, SocketWorkerProperties};

/// The types of supported datagram protocols.
#[derive(Debug)]
pub(crate) enum DatagramProtocol {
    Udp,
    IcmpEcho,
}

/// A minimal abstraction over transport protocols that allows bindings-side state to be stored.
pub(crate) trait Transport<I: Ip>: Debug + Sized + Send + Sync + 'static {
    const PROTOCOL: DatagramProtocol;
    /// Whether the Transport Protocol supports dualstack sockets.
    const SUPPORTS_DUALSTACK: bool;
    type SocketId: Hash + Eq + Debug + Send + Sync + Clone;

    /// Match Linux and implicitly map IPv4 addresses to IPv6 addresses for
    /// dual-stack capable protocols.
    fn maybe_map_sock_addr(addr: fnet::SocketAddress) -> fnet::SocketAddress {
        match (I::VERSION, addr, Self::SUPPORTS_DUALSTACK) {
            (IpVersion::V6, fnet::SocketAddress::Ipv4(v4_addr), true) => {
                let port = v4_addr.port();
                let address = v4_addr.addr().to_ipv6_mapped();
                fnet::SocketAddress::Ipv6(fnet::Ipv6SocketAddress::new(
                    Some(ZonedAddr::Unzoned(address)),
                    port,
                ))
            }
            (_, _, _) => addr,
        }
    }

    fn external_data(id: &Self::SocketId) -> &DatagramSocketExternalData<I>;

    fn get_rcvbuf_settings(ctx: &mut Ctx) -> impl Deref<Target = BufferSizeSettings<NonZeroUsize>>;

    #[cfg(test)]
    fn collect_all_sockets(ctx: &mut Ctx) -> Vec<Self::SocketId>;
}

/// Bindings data held by datagram sockets.
#[derive(Debug)]
pub(crate) struct DatagramSocketExternalData<I: Ip> {
    message_queue: CoreMutex<MessageQueue<AvailableMessage<I>, SocketEventPair>>,
    sharing_domain_token: CoreMutex<Option<zx::Event>>,
}

/// A special case of TryFrom that avoids the associated error type in generic contexts.
pub(crate) trait OptionFromU16: Sized {
    fn from_u16(_: u16) -> Option<Self>;
}

pub(crate) struct LocalAddress<I: Ip, D, L> {
    address: Option<core_socket::StrictlyZonedAddr<I::Addr, SpecifiedAddr<I::Addr>, D>>,
    identifier: Option<L>,
}
pub(crate) struct RemoteAddress<I: Ip, D, R> {
    address: core_socket::StrictlyZonedAddr<I::Addr, SpecifiedAddr<I::Addr>, D>,
    identifier: R,
}

/// An abstraction over transport protocols that allows generic manipulation of Core state.
pub(crate) trait TransportState<I: Ip>: Transport<I> + Send + Sync + 'static {
    type ConnectError: IntoErrno;
    type ListenError: IntoErrno;
    type DisconnectError: IntoErrno;
    type SetSocketDeviceError: IntoErrno;
    type SetMulticastMembershipError: IntoErrno;
    type MulticastInterfaceError: IntoErrno;
    type MulticastLoopError: IntoErrno;
    type SetReuseAddrError: IntoErrno;
    type SetReusePortError: IntoErrno;
    type ShutdownError: IntoErrno;
    type SetIpTransparentError: IntoErrno;
    type SetBroadcastError: IntoErrno;
    type LocalIdentifier: OptionFromU16 + Into<u16> + Send;
    type RemoteIdentifier: From<u16> + Into<u16> + Send;
    type SocketInfo: IntoFidl<LocalAddress<I, WeakDeviceId<BindingsCtx>, Self::LocalIdentifier>>
        + TryIntoFidl<RemoteAddress<I, WeakDeviceId<BindingsCtx>, u16>, Error = ErrnoError>;
    type SendError: IntoErrno;
    type DscpAndEcnError: IntoErrno;

    fn create_unbound(
        ctx: &mut Ctx,
        external_data: DatagramSocketExternalData<I>,
        writable_listener: SocketEventPair,
    ) -> Self::SocketId;

    fn connect(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        remote_ip: Option<ZonedAddr<SpecifiedAddr<I::Addr>, DeviceId<BindingsCtx>>>,
        remote_id: Self::RemoteIdentifier,
    ) -> Result<(), Self::ConnectError>;

    fn bind(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        addr: Option<ZonedAddr<SpecifiedAddr<I::Addr>, DeviceId<BindingsCtx>>>,
        port: Option<Self::LocalIdentifier>,
    ) -> Result<(), Self::ListenError>;

    fn disconnect(ctx: &mut Ctx, id: &Self::SocketId) -> Result<(), Self::DisconnectError>;

    fn shutdown(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        which: ShutdownType,
    ) -> Result<(), Self::ShutdownError>;

    fn get_shutdown(ctx: &mut Ctx, id: &Self::SocketId) -> Option<ShutdownType>;

    fn get_socket_info(ctx: &mut Ctx, id: &Self::SocketId) -> Self::SocketInfo;

    async fn close(ctx: &mut Ctx, id: Self::SocketId);

    fn set_socket_device(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        device: Option<&DeviceId<BindingsCtx>>,
    ) -> Result<(), Self::SetSocketDeviceError>;

    fn get_bound_device(ctx: &mut Ctx, id: &Self::SocketId) -> Option<WeakDeviceId<BindingsCtx>>;

    fn set_dual_stack_enabled(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        enabled: bool,
    ) -> Result<(), SetDualStackEnabledError>;

    fn get_dual_stack_enabled(
        ctx: &mut Ctx,
        id: &Self::SocketId,
    ) -> Result<bool, NotDualStackCapableError>;

    fn set_reuse_addr(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        reuse_addr: bool,
    ) -> Result<(), Self::SetReuseAddrError>;

    fn get_reuse_addr(ctx: &mut Ctx, id: &Self::SocketId) -> bool;

    fn set_reuse_port(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        reuse_port: ReusePortOption,
        token: Option<zx::Event>,
    ) -> Result<(), Self::SetReusePortError>;

    fn get_reuse_port(ctx: &mut Ctx, id: &Self::SocketId) -> bool;

    fn set_multicast_membership(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        multicast_group: MulticastAddr<I::Addr>,
        interface: MulticastMembershipInterfaceSelector<I::Addr, DeviceId<BindingsCtx>>,
        want_membership: bool,
    ) -> Result<(), Self::SetMulticastMembershipError>;

    fn set_unicast_hop_limit(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        hop_limit: Option<NonZeroU8>,
        ip_version: IpVersion,
    ) -> Result<(), NotDualStackCapableError>;

    fn set_multicast_hop_limit(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        hop_limit: Option<NonZeroU8>,
        ip_version: IpVersion,
    ) -> Result<(), NotDualStackCapableError>;

    fn get_unicast_hop_limit(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        ip_version: IpVersion,
    ) -> Result<NonZeroU8, NotDualStackCapableError>;

    fn get_multicast_hop_limit(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        ip_version: IpVersion,
    ) -> Result<NonZeroU8, NotDualStackCapableError>;

    fn set_ip_transparent(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        value: bool,
    ) -> Result<(), Self::SetIpTransparentError>;

    fn get_ip_transparent(ctx: &mut Ctx, id: &Self::SocketId) -> bool;

    fn set_multicast_interface(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        interface: Option<&DeviceId<BindingsCtx>>,
        ip_version: IpVersion,
    ) -> Result<(), Self::MulticastInterfaceError>;

    fn get_multicast_interface(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        ip_version: IpVersion,
    ) -> Result<Option<WeakDeviceId<BindingsCtx>>, Self::MulticastInterfaceError>;

    fn set_multicast_loop(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        value: bool,
        ip_version: IpVersion,
    ) -> Result<(), Self::MulticastLoopError>;

    fn get_multicast_loop(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        ip_version: IpVersion,
    ) -> Result<bool, Self::MulticastLoopError>;

    fn set_broadcast(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        value: bool,
    ) -> Result<(), Self::SetBroadcastError>;

    fn get_broadcast(ctx: &mut Ctx, id: &Self::SocketId) -> bool;

    fn set_dscp_and_ecn(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        dscp_and_ecn: DscpAndEcn,
        ip_version: IpVersion,
    ) -> Result<(), Self::DscpAndEcnError>;

    fn get_dscp_and_ecn(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        ip_version: IpVersion,
    ) -> Result<DscpAndEcn, Self::DscpAndEcnError>;

    fn send<B: BufferMut>(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        remote: Option<(
            Option<ZonedAddr<SpecifiedAddr<I::Addr>, DeviceId<BindingsCtx>>>,
            Self::RemoteIdentifier,
        )>,
        body: B,
    ) -> Result<(), Self::SendError>;

    fn set_mark(ctx: &mut Ctx, id: &Self::SocketId, domain: MarkDomain, mark: Mark);

    fn get_mark(ctx: &mut Ctx, id: &Self::SocketId, domain: MarkDomain) -> Mark;

    fn get_cookie(ctx: &mut Ctx, id: &Self::SocketId) -> SocketCookie;

    fn set_send_buffer(ctx: &mut Ctx, id: &Self::SocketId, send_buffer: usize);
    fn get_send_buffer(ctx: &mut Ctx, id: &Self::SocketId) -> usize;

    fn take_pending_error(ctx: &mut Ctx, id: &Self::SocketId)
    -> Option<PendingDatagramSocketError>;
}

#[derive(Debug)]
pub(crate) enum Udp {}

type UdpSocketId<I> = udp::UdpSocketId<I, WeakDeviceId<BindingsCtx>, BindingsCtx>;

impl<I: IpExt> Transport<I> for Udp {
    const PROTOCOL: DatagramProtocol = DatagramProtocol::Udp;
    const SUPPORTS_DUALSTACK: bool = true;
    type SocketId = UdpSocketId<I>;

    fn external_data(id: &Self::SocketId) -> &DatagramSocketExternalData<I> {
        id.external_data()
    }

    fn get_rcvbuf_settings(ctx: &mut Ctx) -> impl Deref<Target = BufferSizeSettings<NonZeroUsize>> {
        ctx.bindings_ctx().settings.udp.read().map_deref(|u| &u.receive_buffer)
    }

    #[cfg(test)]
    fn collect_all_sockets(ctx: &mut Ctx) -> Vec<Self::SocketId> {
        net_types::map_ip_twice!(I, IpInvariant(ctx), |IpInvariant(ctx)| ctx
            .api()
            .udp::<I>()
            .collect_all_sockets())
    }
}

impl OptionFromU16 for NonZeroU16 {
    fn from_u16(t: u16) -> Option<Self> {
        Self::new(t)
    }
}

#[derive(Error, Debug)]
pub(crate) enum UdpSendError {
    #[error(transparent)]
    Core(#[from] udp::SendToError),
    #[error(transparent)]
    PendingDatagramSocket(#[from] PendingDatagramSocketError),
    #[error("cannot send on non-connected UDP socket")]
    NotConnected,
}

impl From<Either<udp::SendError, ExpectedConnError>> for UdpSendError {
    fn from(value: Either<udp::SendError, ExpectedConnError>) -> Self {
        match value {
            Either::Left(e) => Self::Core(match e {
                udp::SendError::NotWriteable => udp::SendToError::NotWriteable,
                udp::SendError::SendBufferFull => udp::SendToError::SendBufferFull,
                udp::SendError::InvalidLength => udp::SendToError::InvalidLength,
                udp::SendError::IpSock(err) => udp::SendToError::Send(err),
                udp::SendError::RemotePortUnset => udp::SendToError::RemotePortUnset,
            }),
            Either::Right(ExpectedConnError) => Self::NotConnected,
        }
    }
}

impl IntoErrno for UdpSendError {
    fn to_errno(&self) -> fidl_fuchsia_posix::Errno {
        match self {
            UdpSendError::Core(err) => err.to_errno(),
            UdpSendError::PendingDatagramSocket(err) => err.to_errno(),
            UdpSendError::NotConnected => fposix::Errno::Edestaddrreq,
        }
    }
}

#[netstack3_core::context_ip_bounds(I, BindingsCtx)]
impl<I> TransportState<I> for Udp
where
    I: IpExt,
{
    type ConnectError = ConnectError;
    type ListenError = Either<ExpectedUnboundError, LocalAddressError>;
    type DisconnectError = ExpectedConnError;
    type ShutdownError = ExpectedConnError;
    type SetSocketDeviceError = SocketError;
    type SetMulticastMembershipError = SetMulticastMembershipError;
    type MulticastInterfaceError = NotDualStackCapableError;
    type MulticastLoopError = NotDualStackCapableError;
    type SetReuseAddrError = ExpectedUnboundError;
    type SetReusePortError = ExpectedUnboundError;
    type SetIpTransparentError = Never;
    type SetBroadcastError = Never;
    type LocalIdentifier = NonZeroU16;
    type RemoteIdentifier = udp::UdpRemotePort;
    type SocketInfo = SocketInfo<I::Addr, WeakDeviceId<BindingsCtx>>;
    type SendError = UdpSendError;
    type DscpAndEcnError = NotDualStackCapableError;

    fn create_unbound(
        ctx: &mut Ctx,
        external_data: DatagramSocketExternalData<I>,
        writable_listener: SocketEventPair,
    ) -> Self::SocketId {
        ctx.api().udp().create_with(external_data, writable_listener)
    }

    fn connect(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        remote_ip: Option<ZonedAddr<SpecifiedAddr<<I as Ip>::Addr>, DeviceId<BindingsCtx>>>,
        remote_id: Self::RemoteIdentifier,
    ) -> Result<(), Self::ConnectError> {
        ctx.api().udp().connect(id, remote_ip, remote_id)
    }

    fn bind(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        addr: Option<ZonedAddr<SpecifiedAddr<<I as Ip>::Addr>, DeviceId<BindingsCtx>>>,
        port: Option<Self::LocalIdentifier>,
    ) -> Result<(), Self::ListenError> {
        ctx.api().udp().listen(id, addr, port)
    }

    fn disconnect(ctx: &mut Ctx, id: &Self::SocketId) -> Result<(), Self::DisconnectError> {
        ctx.api().udp().disconnect(id)
    }

    fn shutdown(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        which: ShutdownType,
    ) -> Result<(), Self::ShutdownError> {
        ctx.api().udp().shutdown(id, which)
    }

    fn get_shutdown(ctx: &mut Ctx, id: &Self::SocketId) -> Option<ShutdownType> {
        ctx.api().udp().get_shutdown(id)
    }

    fn get_socket_info(ctx: &mut Ctx, id: &Self::SocketId) -> Self::SocketInfo {
        ctx.api().udp().get_info(id)
    }

    async fn close(ctx: &mut Ctx, id: Self::SocketId) {
        let weak = id.downgrade();
        let (seed, _external_data) = ctx
            .api()
            .udp()
            .close(id)
            .map_deferred(|d| d.into_future("udp socket", &weak, ctx))
            .into_future()
            .await;

        ctx.bindings_ctx().destruction_dispatcher.notify(seed);
    }

    fn set_socket_device(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        device: Option<&DeviceId<BindingsCtx>>,
    ) -> Result<(), Self::SetSocketDeviceError> {
        ctx.api().udp().set_device(id, device)
    }

    fn get_bound_device(ctx: &mut Ctx, id: &Self::SocketId) -> Option<WeakDeviceId<BindingsCtx>> {
        ctx.api().udp().get_bound_device(id)
    }

    fn set_reuse_addr(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        reuse_addr: bool,
    ) -> Result<(), Self::SetReusePortError> {
        ctx.api().udp().set_posix_reuse_addr(id, reuse_addr).inspect_err(|_| {
            warn!("tried to set SO_REUSEADDR on a bound socket; see https://fxbug.dev/42051599")
        })
    }

    fn set_reuse_port(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        reuse_port: ReusePortOption,
        token: Option<zx::Event>,
    ) -> Result<(), Self::SetReusePortError> {
        let result = ctx.api().udp().set_posix_reuse_port(id, reuse_port).inspect_err(|_| {
            warn!("tried to set SO_REUSEPORT on a bound socket; see https://fxbug.dev/42051599")
        });
        if result.is_ok() {
            // Update the token if the operation succeeded.
            *id.external_data().sharing_domain_token.lock() = token;
        }
        result
    }

    fn set_dual_stack_enabled(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        enabled: bool,
    ) -> Result<(), SetDualStackEnabledError> {
        ctx.api().udp().set_dual_stack_enabled(id, enabled)
    }

    fn get_dual_stack_enabled(
        ctx: &mut Ctx,
        id: &Self::SocketId,
    ) -> Result<bool, NotDualStackCapableError> {
        ctx.api().udp().get_dual_stack_enabled(id)
    }

    fn get_reuse_addr(ctx: &mut Ctx, id: &Self::SocketId) -> bool {
        ctx.api().udp().get_posix_reuse_addr(id)
    }

    fn get_reuse_port(ctx: &mut Ctx, id: &Self::SocketId) -> bool {
        ctx.api().udp().get_posix_reuse_port(id)
    }

    fn set_multicast_membership(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        multicast_group: MulticastAddr<I::Addr>,
        interface: MulticastMembershipInterfaceSelector<I::Addr, DeviceId<BindingsCtx>>,
        want_membership: bool,
    ) -> Result<(), Self::SetMulticastMembershipError> {
        ctx.api().udp().set_multicast_membership(id, multicast_group, interface, want_membership)
    }

    fn set_unicast_hop_limit(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        hop_limit: Option<NonZeroU8>,
        ip_version: IpVersion,
    ) -> Result<(), NotDualStackCapableError> {
        ctx.api().udp().set_unicast_hop_limit(id, hop_limit, ip_version)
    }

    fn set_multicast_hop_limit(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        hop_limit: Option<NonZeroU8>,
        ip_version: IpVersion,
    ) -> Result<(), NotDualStackCapableError> {
        ctx.api().udp().set_multicast_hop_limit(id, hop_limit, ip_version)
    }

    fn get_unicast_hop_limit(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        ip_version: IpVersion,
    ) -> Result<NonZeroU8, NotDualStackCapableError> {
        ctx.api().udp().get_unicast_hop_limit(id, ip_version)
    }

    fn get_multicast_hop_limit(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        ip_version: IpVersion,
    ) -> Result<NonZeroU8, NotDualStackCapableError> {
        ctx.api().udp().get_multicast_hop_limit(id, ip_version)
    }

    fn set_ip_transparent(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        value: bool,
    ) -> Result<(), Self::SetIpTransparentError> {
        Ok(ctx.api().udp().set_transparent(id, value))
    }

    fn get_ip_transparent(ctx: &mut Ctx, id: &Self::SocketId) -> bool {
        ctx.api().udp().get_transparent(id)
    }

    fn set_multicast_interface(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        interface: Option<&DeviceId<BindingsCtx>>,
        ip_version: IpVersion,
    ) -> Result<(), Self::MulticastInterfaceError> {
        ctx.api().udp().set_multicast_interface(id, interface, ip_version)
    }

    fn get_multicast_interface(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        ip_version: IpVersion,
    ) -> Result<Option<WeakDeviceId<BindingsCtx>>, Self::MulticastInterfaceError> {
        ctx.api().udp().get_multicast_interface(id, ip_version)
    }

    fn set_multicast_loop(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        value: bool,
        ip_version: IpVersion,
    ) -> Result<(), Self::MulticastLoopError> {
        ctx.api().udp().set_multicast_loop(id, value, ip_version)
    }

    fn get_multicast_loop(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        ip_version: IpVersion,
    ) -> Result<bool, Self::MulticastLoopError> {
        ctx.api().udp().get_multicast_loop(id, ip_version)
    }

    fn set_broadcast(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        value: bool,
    ) -> Result<(), Self::SetBroadcastError> {
        Ok(ctx.api().udp().set_broadcast(id, value))
    }

    fn get_broadcast(ctx: &mut Ctx, id: &Self::SocketId) -> bool {
        ctx.api().udp().get_broadcast(id)
    }

    fn set_dscp_and_ecn(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        traffic_class: DscpAndEcn,
        ip_version: IpVersion,
    ) -> Result<(), Self::DscpAndEcnError> {
        ctx.api().udp().set_dscp_and_ecn(id, traffic_class, ip_version)
    }

    fn get_dscp_and_ecn(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        ip_version: IpVersion,
    ) -> Result<DscpAndEcn, Self::DscpAndEcnError> {
        ctx.api().udp().get_dscp_and_ecn(id, ip_version)
    }

    fn send<B: BufferMut>(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        remote: Option<(
            Option<ZonedAddr<SpecifiedAddr<I::Addr>, DeviceId<BindingsCtx>>>,
            Self::RemoteIdentifier,
        )>,
        body: B,
    ) -> Result<(), Self::SendError> {
        if let Some(err) = Self::take_pending_error(ctx, id) {
            return Err(UdpSendError::PendingDatagramSocket(err));
        }

        // NOTE: It's possible an ICMP error arrived on the socket between the
        // check above and now. However, it's not possible for the application
        // to detect the difference between that and the error coming in later.
        match remote {
            Some((remote_ip, remote_port)) => {
                ctx.api().udp().send_to(id, remote_ip, remote_port, body, ()).map_err(|e| e.into())
            }
            None => ctx.api().udp().send(id, body, ()).map_err(|e| e.into()),
        }
    }

    fn set_mark(ctx: &mut Ctx, id: &Self::SocketId, domain: MarkDomain, mark: Mark) {
        ctx.api().udp().set_mark(id, domain, mark)
    }

    fn get_mark(ctx: &mut Ctx, id: &Self::SocketId, domain: MarkDomain) -> Mark {
        ctx.api().udp().get_mark(id, domain)
    }

    fn get_cookie(_ctx: &mut Ctx, id: &Self::SocketId) -> SocketCookie {
        id.socket_cookie()
    }

    fn set_send_buffer(ctx: &mut Ctx, id: &Self::SocketId, send_buffer: usize) {
        ctx.api().udp().set_send_buffer(id, send_buffer)
    }

    fn get_send_buffer(ctx: &mut Ctx, id: &Self::SocketId) -> usize {
        ctx.api().udp().send_buffer(id)
    }

    fn take_pending_error(
        _ctx: &mut Ctx,
        id: &Self::SocketId,
    ) -> Option<PendingDatagramSocketError> {
        id.external_data().message_queue.lock().take_pending_error()
    }
}

impl<I: IpExt> DatagramSocketExternalData<I> {
    pub(crate) fn receive_udp(
        &self,
        device_id: &DeviceId<BindingsCtx>,
        meta: UdpPacketMeta<I>,
        body: &[u8],
    ) -> Result<(), NoSpace> {
        // TODO(https://fxbug.dev/326102014): Store `UdpPacketMeta` in `AvailableMessage`.
        let UdpPacketMeta { src_ip, src_port, dst_ip, dst_port, .. } = meta;

        // NB: Perform the expensive tasks before taking the message queue lock.
        let message = AvailableMessage {
            interface_id: device_id.bindings_id().id,
            source_addr: src_ip,
            source_port: src_port.map_or(0, NonZeroU16::get),
            destination_addr: dst_ip,
            destination_port: dst_port.get(),
            timestamp: fasync::MonotonicInstant::now(),
            data: body.to_vec(),
            dscp_and_ecn: meta.dscp_and_ecn,
        };

        self.message_queue.lock().receive(message)
    }

    pub(crate) fn on_socket_error(&self, err: PendingDatagramSocketError) {
        self.message_queue.lock().set_pending_error(err);
    }
}

#[derive(Debug)]
pub(crate) enum IcmpEcho {}

type IcmpSocketId<I> = icmp::IcmpSocketId<I, WeakDeviceId<BindingsCtx>, BindingsCtx>;

impl<I: IpExt> Transport<I> for IcmpEcho {
    const PROTOCOL: DatagramProtocol = DatagramProtocol::IcmpEcho;
    const SUPPORTS_DUALSTACK: bool = false;
    type SocketId = IcmpSocketId<I>;

    fn external_data(id: &Self::SocketId) -> &DatagramSocketExternalData<I> {
        id.external_data()
    }

    fn get_rcvbuf_settings(ctx: &mut Ctx) -> impl Deref<Target = BufferSizeSettings<NonZeroUsize>> {
        ctx.bindings_ctx().settings.icmp.read().map_deref(|i| &i.echo_receive_buffer)
    }

    #[cfg(test)]
    fn collect_all_sockets(ctx: &mut Ctx) -> Vec<Self::SocketId> {
        net_types::map_ip_twice!(I, IpInvariant(ctx), |IpInvariant(ctx)| ctx
            .api()
            .icmp_echo::<I>()
            .collect_all_sockets())
    }
}

impl OptionFromU16 for u16 {
    fn from_u16(t: u16) -> Option<Self> {
        Some(t)
    }
}

#[derive(Error, Debug)]
pub(crate) enum IcmpSendError {
    #[error(transparent)]
    CoreSendTo(#[from] core_socket::SendToError<packet_formats::error::ParseError>),
    #[error(transparent)]
    CoreSend(#[from] core_socket::SendError<packet_formats::error::ParseError>),
}

impl IntoErrno for IcmpSendError {
    fn to_errno(&self) -> fidl_fuchsia_posix::Errno {
        match self {
            IcmpSendError::CoreSendTo(err) => err.to_errno(),
            IcmpSendError::CoreSend(err) => err.to_errno(),
        }
    }
}

#[netstack3_core::context_ip_bounds(I, BindingsCtx)]
impl<I> TransportState<I> for IcmpEcho
where
    I: IpExt,
{
    type ConnectError = ConnectError;
    type ListenError = Either<ExpectedUnboundError, LocalAddressError>;
    type DisconnectError = ExpectedConnError;
    type ShutdownError = ExpectedConnError;
    type SetSocketDeviceError = SocketError;
    type SetMulticastMembershipError = NotSupportedError;
    type MulticastInterfaceError = NotSupportedError;
    type MulticastLoopError = NotDualStackCapableError;
    type SetBroadcastError = NotSupportedError;
    type SetReuseAddrError = NotSupportedError;
    type SetReusePortError = NotSupportedError;
    type SetIpTransparentError = NotSupportedError;
    type LocalIdentifier = NonZeroU16;
    type RemoteIdentifier = u16;
    type SocketInfo = SocketInfo<I::Addr, WeakDeviceId<BindingsCtx>>;
    type SendError = IcmpSendError;
    type DscpAndEcnError = NotSupportedError;

    fn create_unbound(
        ctx: &mut Ctx,
        external_data: DatagramSocketExternalData<I>,
        writable_listener: SocketEventPair,
    ) -> Self::SocketId {
        ctx.api().icmp_echo().create_with(external_data, writable_listener)
    }

    fn connect(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        remote_ip: Option<ZonedAddr<SpecifiedAddr<I::Addr>, DeviceId<BindingsCtx>>>,
        remote_id: Self::RemoteIdentifier,
    ) -> Result<(), Self::ConnectError> {
        ctx.api().icmp_echo().connect(id, remote_ip, remote_id)
    }

    fn bind(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        addr: Option<ZonedAddr<SpecifiedAddr<<I as Ip>::Addr>, DeviceId<BindingsCtx>>>,
        port: Option<Self::LocalIdentifier>,
    ) -> Result<(), Self::ListenError> {
        ctx.api().icmp_echo().bind(id, addr, port)
    }

    fn disconnect(ctx: &mut Ctx, id: &Self::SocketId) -> Result<(), Self::DisconnectError> {
        ctx.api().icmp_echo().disconnect(id)
    }

    fn shutdown(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        which: ShutdownType,
    ) -> Result<(), Self::ShutdownError> {
        ctx.api().icmp_echo().shutdown(id, which)
    }

    fn get_shutdown(ctx: &mut Ctx, id: &Self::SocketId) -> Option<ShutdownType> {
        ctx.api().icmp_echo().get_shutdown(id)
    }

    fn get_socket_info(ctx: &mut Ctx, id: &Self::SocketId) -> Self::SocketInfo {
        ctx.api().icmp_echo().get_info(id)
    }

    async fn close(ctx: &mut Ctx, id: Self::SocketId) {
        let weak = id.downgrade();
        let _external_data = ctx
            .api()
            .icmp_echo()
            .close(id)
            .map_deferred(|d| d.into_future("icmp socket", &weak, ctx))
            .into_future()
            .await;
    }

    fn set_socket_device(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        device: Option<&DeviceId<BindingsCtx>>,
    ) -> Result<(), Self::SetSocketDeviceError> {
        ctx.api().icmp_echo().set_device(id, device)
    }

    fn get_bound_device(ctx: &mut Ctx, id: &Self::SocketId) -> Option<WeakDeviceId<BindingsCtx>> {
        ctx.api().icmp_echo().get_bound_device(id)
    }

    fn set_dual_stack_enabled(
        _ctx: &mut Ctx,
        _id: &Self::SocketId,
        _enabled: bool,
    ) -> Result<(), SetDualStackEnabledError> {
        // NB: Despite ICMP's lack of support for dual stack operations, Linux
        // allows the `IPV6_V6ONLY` socket option to be set/unset. Here we
        // disallow setting the option, which more accurately reflects that ICMP
        // sockets do not support dual stack operations.
        return Err(NotDualStackCapableError.into());
    }

    fn get_dual_stack_enabled(
        _ctx: &mut Ctx,
        _id: &Self::SocketId,
    ) -> Result<bool, NotDualStackCapableError> {
        match I::VERSION {
            IpVersion::V4 => Err(NotDualStackCapableError),
            // NB: Despite ICMP's lack of support for dual stack operations,
            // Linux allows the `IPV6_V6ONLY` socket option to be set/unset.
            // Here we always report that the dual stack operations are
            // disabled, which more accurately reflects that ICMP sockets do not
            // support dual stack operations.
            IpVersion::V6 => Ok(false),
        }
    }

    fn set_reuse_addr(
        _ctx: &mut Ctx,
        _id: &Self::SocketId,
        _reuse_addr: bool,
    ) -> Result<(), Self::SetReuseAddrError> {
        Err(NotSupportedError)
    }

    fn get_reuse_addr(_ctx: &mut Ctx, _id: &Self::SocketId) -> bool {
        false
    }

    fn set_reuse_port(
        _ctx: &mut Ctx,
        _id: &Self::SocketId,
        _reuse_port: ReusePortOption,
        _token: Option<zx::Event>,
    ) -> Result<(), Self::SetReusePortError> {
        Err(NotSupportedError)
    }

    fn get_reuse_port(_ctx: &mut Ctx, _id: &Self::SocketId) -> bool {
        false
    }

    fn set_multicast_membership(
        _ctx: &mut Ctx,
        _id: &Self::SocketId,
        _multicast_group: MulticastAddr<I::Addr>,
        _interface: MulticastMembershipInterfaceSelector<I::Addr, DeviceId<BindingsCtx>>,
        _want_membership: bool,
    ) -> Result<(), Self::SetMulticastMembershipError> {
        Err(NotSupportedError)
    }

    fn set_unicast_hop_limit(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        hop_limit: Option<NonZeroU8>,
        ip_version: IpVersion,
    ) -> Result<(), NotDualStackCapableError> {
        // Disallow updates when the hop limit's version doesn't match the
        // socket's version. This matches Linux's behavior for IPv4 sockets, but
        // diverges from Linux's behavior for IPv6 sockets. Rejecting updates to
        // the IPv4 TTL for IPv6 sockets more accurately reflects that ICMP
        // sockets do not support dual stack operations.
        if I::VERSION != ip_version {
            return Err(NotDualStackCapableError);
        }
        Ok(ctx.api().icmp_echo().set_unicast_hop_limit(id, hop_limit))
    }

    fn set_multicast_hop_limit(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        hop_limit: Option<NonZeroU8>,
        ip_version: IpVersion,
    ) -> Result<(), NotDualStackCapableError> {
        // Disallow updates when the hop limit's version doesn't match the
        // socket's version. This matches Linux's behavior for IPv4 sockets, but
        // diverges from Linux's behavior for IPv6 sockets. Rejecting updates to
        // the IPv4 TTL for IPv6 sockets more accurately reflects that ICMP
        // sockets do not support dual stack operations.
        if I::VERSION != ip_version {
            return Err(NotDualStackCapableError);
        }
        Ok(ctx.api().icmp_echo().set_multicast_hop_limit(id, hop_limit))
    }

    fn get_unicast_hop_limit(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        ip_version: IpVersion,
    ) -> Result<NonZeroU8, NotDualStackCapableError> {
        // Disallow fetching the hop limit when its version doesn't match the
        // socket's version. This matches Linux's behavior for IPv4 sockets, but
        // diverges from Linux's behavior for IPv6 sockets. Rejecting fetches of
        // the IPv4 TTL for IPv6 sockets more accurately reflects that ICMP
        // sockets do not support dual stack operations.
        if I::VERSION != ip_version {
            return Err(NotDualStackCapableError);
        }
        Ok(ctx.api().icmp_echo().get_unicast_hop_limit(id))
    }

    fn get_multicast_hop_limit(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        ip_version: IpVersion,
    ) -> Result<NonZeroU8, NotDualStackCapableError> {
        // Disallow fetching the hop limit when its version doesn't match the
        // socket's version. This matches Linux's behavior for IPv4 sockets, but
        // diverges from Linux's behavior for IPv6 sockets. Rejecting fetches of
        // the IPv4 TTL for IPv6 sockets more accurately reflects that ICMP
        // sockets do not support dual stack operations.
        if I::VERSION != ip_version {
            return Err(NotDualStackCapableError);
        }
        Ok(ctx.api().icmp_echo().get_multicast_hop_limit(id))
    }

    fn set_multicast_interface(
        _ctx: &mut Ctx,
        _id: &Self::SocketId,
        _interface: Option<&DeviceId<BindingsCtx>>,
        _ip_version: IpVersion,
    ) -> Result<(), NotSupportedError> {
        Err(NotSupportedError)
    }

    fn get_multicast_interface(
        _ctx: &mut Ctx,
        _id: &Self::SocketId,
        _ip_version: IpVersion,
    ) -> Result<Option<WeakDeviceId<BindingsCtx>>, Self::MulticastInterfaceError> {
        Err(NotSupportedError)
    }

    fn set_ip_transparent(
        _ctx: &mut Ctx,
        _id: &Self::SocketId,
        _value: bool,
    ) -> Result<(), Self::SetIpTransparentError> {
        Err(NotSupportedError)
    }

    fn get_ip_transparent(_ctx: &mut Ctx, _id: &Self::SocketId) -> bool {
        false
    }

    fn set_multicast_loop(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        value: bool,
        ip_version: IpVersion,
    ) -> Result<(), Self::MulticastLoopError> {
        // Disallow setting multicast loop when its version doesn't match the
        // socket's version. This matches Linux's behavior for IPv4 sockets, but
        // diverges from Linux's behavior for IPv6 sockets. Rejecting setting
        // the IPv4 multicast loop for IPv6 sockets more accurately reflects
        // that ICMP sockets do not support dual stack operations.
        if I::VERSION != ip_version {
            return Err(NotDualStackCapableError);
        }
        Ok(ctx.api().icmp_echo().set_multicast_loop(id, value))
    }

    fn get_multicast_loop(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        ip_version: IpVersion,
    ) -> Result<bool, Self::MulticastLoopError> {
        // Disallow fetching multicast loop when its version doesn't match the
        // socket's version. This matches Linux's behavior for IPv4 sockets, but
        // diverges from Linux's behavior for IPv6 sockets. Rejecting fetches of
        // the IPv4 multicast loop for IPv6 sockets more accurately reflects
        // that ICMP sockets do not support dual stack operations.
        if I::VERSION != ip_version {
            return Err(NotDualStackCapableError);
        }
        Ok(ctx.api().icmp_echo().get_multicast_loop(id))
    }

    fn set_broadcast(
        _ctx: &mut Ctx,
        _id: &Self::SocketId,
        _value: bool,
    ) -> Result<(), Self::SetBroadcastError> {
        Err(NotSupportedError)
    }

    fn get_broadcast(_ctx: &mut Ctx, _id: &Self::SocketId) -> bool {
        false
    }

    fn set_dscp_and_ecn(
        _ctx: &mut Ctx,
        _id: &Self::SocketId,
        _traffic_class: DscpAndEcn,
        _ip_version: IpVersion,
    ) -> Result<(), Self::DscpAndEcnError> {
        Err(NotSupportedError)
    }

    fn get_dscp_and_ecn(
        _ctx: &mut Ctx,
        _id: &Self::SocketId,
        _ip_version: IpVersion,
    ) -> Result<DscpAndEcn, Self::DscpAndEcnError> {
        Err(NotSupportedError)
    }

    fn send<B: BufferMut>(
        ctx: &mut Ctx,
        id: &Self::SocketId,
        remote: Option<(
            Option<ZonedAddr<SpecifiedAddr<I::Addr>, DeviceId<BindingsCtx>>>,
            Self::RemoteIdentifier,
        )>,
        body: B,
    ) -> Result<(), Self::SendError> {
        match remote {
            Some((remote_ip, _remote_id)) => {
                ctx.api().icmp_echo().send_to(id, remote_ip, body, ()).map_err(|e| e.into())
            }
            None => ctx.api().icmp_echo().send(id, body, ()).map_err(|e| e.into()),
        }
    }

    fn set_mark(ctx: &mut Ctx, id: &Self::SocketId, domain: MarkDomain, mark: Mark) {
        ctx.api().icmp_echo().set_mark(id, domain, mark)
    }

    fn get_mark(ctx: &mut Ctx, id: &Self::SocketId, domain: MarkDomain) -> Mark {
        ctx.api().icmp_echo().get_mark(id, domain)
    }

    fn get_cookie(_ctx: &mut Ctx, id: &Self::SocketId) -> SocketCookie {
        id.socket_cookie()
    }

    fn set_send_buffer(ctx: &mut Ctx, id: &Self::SocketId, send_buffer: usize) {
        ctx.api().icmp_echo().set_send_buffer(id, send_buffer)
    }

    fn get_send_buffer(ctx: &mut Ctx, id: &Self::SocketId) -> usize {
        ctx.api().icmp_echo().send_buffer(id)
    }

    fn take_pending_error(
        _ctx: &mut Ctx,
        id: &Self::SocketId,
    ) -> Option<PendingDatagramSocketError> {
        id.external_data().message_queue.lock().take_pending_error()
    }
}

impl<E: std::error::Error> IntoErrno for core_socket::SendError<E>
where
    Self: Into<Error>,
{
    fn to_errno(&self) -> fposix::Errno {
        match self {
            core_socket::SendError::NotConnected => fposix::Errno::Edestaddrreq,
            core_socket::SendError::NotWriteable => fposix::Errno::Epipe,
            core_socket::SendError::SendBufferFull => fposix::Errno::Eagain,
            core_socket::SendError::InvalidLength => fposix::Errno::Emsgsize,
            core_socket::SendError::IpSock(err) => err.to_errno(),
            core_socket::SendError::SerializeError(_e) => fposix::Errno::Einval,
        }
    }
}

impl<E: std::error::Error> IntoErrno for core_socket::SendToError<E>
where
    Self: Into<Error>,
{
    fn to_errno(&self) -> fposix::Errno {
        match self {
            core_socket::SendToError::NotWriteable => fposix::Errno::Epipe,
            core_socket::SendToError::LocalAddress(err) => err.to_errno(),
            core_socket::SendToError::SendBufferFull => fposix::Errno::Eagain,
            core_socket::SendToError::InvalidLength => fposix::Errno::Emsgsize,
            core_socket::SendToError::Zone(err) => err.to_errno(),
            // NB: Mapping MTU to EMSGSIZE is different from the impl on
            // `IpSockSendError` which maps to EINVAL instead.
            core_socket::SendToError::CreateAndSend(IpSockCreateAndSendError::Send(
                IpSockSendError::Mtu,
            )) => fposix::Errno::Emsgsize,
            core_socket::SendToError::CreateAndSend(IpSockCreateAndSendError::Send(
                IpSockSendError::IllegalLoopbackAddress,
            )) => fposix::Errno::Einval,
            core_socket::SendToError::CreateAndSend(IpSockCreateAndSendError::Send(
                IpSockSendError::BroadcastNotAllowed,
            )) => fposix::Errno::Eacces,
            core_socket::SendToError::CreateAndSend(IpSockCreateAndSendError::Send(
                IpSockSendError::Unroutable(err),
            )) => err.to_errno(),
            core_socket::SendToError::CreateAndSend(IpSockCreateAndSendError::Create(err)) => {
                err.to_errno()
            }
            core_socket::SendToError::RemoteUnexpectedlyMapped => fposix::Errno::Enetunreach,
            core_socket::SendToError::RemoteUnexpectedlyNonMapped => fposix::Errno::Eafnosupport,
            core_socket::SendToError::SerializeError(_e) => fposix::Errno::Einval,
        }
    }
}

impl<I: IpExt> DatagramSocketExternalData<I> {
    pub(crate) fn receive_icmp_echo_reply<B: BufferMut>(
        &self,
        device: &DeviceId<BindingsCtx>,
        src_ip: I::Addr,
        dst_ip: I::Addr,
        id: u16,
        data: B,
    ) -> Result<(), ReceiveIcmpEchoError> {
        debug!("Received ICMP echo reply in binding: {:?}, id: {id}", I::VERSION);
        // NB: Perform the expensive tasks before taking the message queue lock.
        let message = AvailableMessage {
            source_addr: src_ip,
            source_port: 0,
            interface_id: device.bindings_id().id,
            destination_addr: dst_ip,
            destination_port: id,
            timestamp: fasync::MonotonicInstant::now(),
            data: data.as_ref().to_vec(),
            dscp_and_ecn: DscpAndEcn::default(),
        };
        self.message_queue
            .lock()
            .receive(message)
            .map_err(|NoSpace {}| ReceiveIcmpEchoError::QueueFull)
    }
}

#[derive(Debug, Derivative)]
#[derivative(Clone(bound = ""))]
struct AvailableMessage<I: Ip> {
    interface_id: BindingId,
    source_addr: I::Addr,
    source_port: u16,
    destination_addr: I::Addr,
    destination_port: u16,
    timestamp: fasync::MonotonicInstant,
    data: Vec<u8>,
    dscp_and_ecn: DscpAndEcn,
}

impl<I: Ip> BodyLen for AvailableMessage<I> {
    fn body_len(&self) -> usize {
        self.data.len()
    }
}

/// IP extension providing separate types of bindings data for IPv4 and IPv6.
trait BindingsDataIpExt: Ip {
    /// The version specific bindings data.
    ///
    /// [`Ipv4BindingsData`] for IPv4, and [`Ipv6BindingsData`] for IPv6.
    type VersionSpecificData: Default
        + Send
        + GenericOverIp<Self, Type = Self::VersionSpecificData>
        + GenericOverIp<Ipv4, Type = Ipv4BindingsData>
        + GenericOverIp<Ipv6, Type = Ipv6BindingsData>;
}

impl BindingsDataIpExt for Ipv4 {
    type VersionSpecificData = Ipv4BindingsData;
}

impl BindingsDataIpExt for Ipv6 {
    type VersionSpecificData = Ipv6BindingsData;
}

/// Datagram bindings data specific to IPv4 sockets.
#[derive(Default)]
struct Ipv4BindingsData {
    // NB: At the moment, IPv4 sockets don't need to hold any unique data.
    // IPv4-specific options must be exposed in the generic `BindingData` so
    // that they can be handled for dual-stack sockets.
}
impl<I: Ip + BindingsDataIpExt> GenericOverIp<I> for Ipv4BindingsData {
    type Type = I::VersionSpecificData;
}

/// Datagram bindings data specific to IPv6 sockets.
#[derive(Default)]
struct Ipv6BindingsData {
    /// `IPV6_RECVPKTINFO` option.
    recv_pkt_info: bool,

    /// `IPV6_RECVTCLASS` option.
    recv_tclass: bool,
}
impl<I: Ip + BindingsDataIpExt> GenericOverIp<I> for Ipv6BindingsData {
    type Type = I::VersionSpecificData;
}

#[derive(Debug)]
struct BindingData<I: BindingsDataIpExt, T: Transport<I>> {
    peer_event: zx::EventPair,
    info: SocketControlInfo<I, T>,
    /// The bindings data specific to `I`.
    version_specific_data: I::VersionSpecificData,
    /// If true, return the original received destination address in the control data.  This is
    /// modified using the SetIpReceiveOriginalDestinationAddress method (a.k.a. IP_RECVORIGDSTADDR)
    /// and is useful for transparent sockets (IP_TRANSPARENT).
    ip_receive_original_destination_address: bool,
    /// `SO_TIMESTAMP` and `SO_TIMESTAMPNS` state.
    timestamp_option: fposix_socket::TimestampOption,
    /// `IP_MULTICAST_IF` option. It can be set separately from `IPV6_MULTICAST_IF`.
    ipv4_multicast_if_addr: Option<SpecifiedAddr<Ipv4Addr>>,
    /// `IP_RECVTOS` options.
    ip_recv_tos: bool,
    /// `IP_PKTINFO` option.
    ip_pkt_info: bool,
    /// Sharing domain for SO_REUSEPORT. Used only for clients that do not specify it explicitly.
    // TODO(https://fxbug.dev/451615802): Remove this once all clients are updated to
    // specify sharing domain explicitly.
    sharing_domain: SharingDomain,
}

impl<I, T> BindingData<I, T>
where
    I: IpExt + IpSockAddrExt + BindingsDataIpExt,
    T: Transport<Ipv4>,
    T: Transport<Ipv6>,
    T: TransportState<I>,
{
    /// Creates a new `BindingData`.
    fn new(
        ctx: &mut Ctx,
        properties: SocketWorkerProperties,
        wake_group: Option<WakeGroupId>,
        sharing_domain: SharingDomain,
    ) -> Self {
        let (local_event, peer_event) = SocketEventPair::create();

        let notifier = wake_group.as_ref().and_then(|group| {
            if let Some(notifier) = ctx.bindings_ctx().wake_groups.get_data_notifier(&group) {
                Some(notifier)
            } else {
                warn!("could not attach socket to nonexistent wake group {group:?}");
                None
            }
        });

        let external_data = DatagramSocketExternalData {
            message_queue: CoreMutex::new(MessageQueue::new(
                local_event.clone(),
                notifier.clone(),
                <T as Transport<I>>::get_rcvbuf_settings(ctx).default(),
            )),
            sharing_domain_token: CoreMutex::new(None),
        };
        let id = T::create_unbound(ctx, external_data, local_event);

        if let Some(group) = wake_group
            && notifier.is_some()
        {
            debug!("attaching socket {:?} to wake group {group:?}", id);
        }

        Self {
            peer_event,
            info: SocketControlInfo { _properties: properties, id },
            version_specific_data: I::VersionSpecificData::default(),
            ip_receive_original_destination_address: false,
            timestamp_option: fposix_socket::TimestampOption::Disabled,
            ipv4_multicast_if_addr: None,
            ip_recv_tos: false,
            ip_pkt_info: false,
            sharing_domain,
        }
    }
}

/// Information on socket control plane.
#[derive(Debug)]
pub(crate) struct SocketControlInfo<I: Ip, T: Transport<I>> {
    _properties: SocketWorkerProperties,
    id: T::SocketId,
}

pub(super) fn spawn_worker(
    domain: fposix_socket::Domain,
    proto: fposix_socket::DatagramSocketProtocol,
    ctx: crate::bindings::Ctx,
    request_stream: fposix_socket::SynchronousDatagramSocketRequestStream,
    properties: SocketWorkerProperties,
    creation_opts: fposix_socket::SocketCreationOptions,
    sharing_domain: SharingDomain,
) {
    let fposix_socket::SocketCreationOptions { marks, group, __source_breaking } = creation_opts;

    match (domain, proto) {
        (fposix_socket::Domain::Ipv4, fposix_socket::DatagramSocketProtocol::Udp) => {
            fasync::Scope::current().spawn_request_stream_handler(request_stream, |rs| {
                SocketWorker::serve_stream_with(
                    ctx,
                    move |ctx, properties| {
                        BindingData::<Ipv4, Udp>::new(
                            ctx,
                            properties,
                            group.map(Into::into),
                            sharing_domain,
                        )
                    },
                    properties,
                    rs,
                    marks,
                )
            })
        }
        (fposix_socket::Domain::Ipv6, fposix_socket::DatagramSocketProtocol::Udp) => {
            fasync::Scope::current().spawn_request_stream_handler(request_stream, |rs| {
                SocketWorker::serve_stream_with(
                    ctx,
                    move |ctx, properties| {
                        BindingData::<Ipv6, Udp>::new(
                            ctx,
                            properties,
                            group.map(Into::into),
                            sharing_domain,
                        )
                    },
                    properties,
                    rs,
                    marks,
                )
            })
        }
        (fposix_socket::Domain::Ipv4, fposix_socket::DatagramSocketProtocol::IcmpEcho) => {
            fasync::Scope::current().spawn_request_stream_handler(request_stream, |rs| {
                SocketWorker::serve_stream_with(
                    ctx,
                    move |ctx, properties| {
                        BindingData::<Ipv4, IcmpEcho>::new(
                            ctx,
                            properties,
                            /* notifier */ None,
                            sharing_domain,
                        )
                    },
                    properties,
                    rs,
                    marks,
                )
            })
        }
        (fposix_socket::Domain::Ipv6, fposix_socket::DatagramSocketProtocol::IcmpEcho) => {
            fasync::Scope::current().spawn_request_stream_handler(request_stream, |rs| {
                SocketWorker::serve_stream_with(
                    ctx,
                    move |ctx, properties| {
                        BindingData::<Ipv6, IcmpEcho>::new(
                            ctx,
                            properties,
                            /* notifier */ None,
                            sharing_domain,
                        )
                    },
                    properties,
                    rs,
                    marks,
                )
            })
        }
    }
}

impl worker::CloseResponder for fposix_socket::SynchronousDatagramSocketCloseResponder {
    fn send(self, arg: Result<(), i32>) -> Result<(), fidl::Error> {
        fposix_socket::SynchronousDatagramSocketCloseResponder::send(self, arg)
    }
}

impl<I, T> worker::SocketWorkerHandler for BindingData<I, T>
where
    I: IpExt + IpSockAddrExt + BindingsDataIpExt,
    T: Transport<Ipv4>,
    T: Transport<Ipv6>,
    T: TransportState<I>,
    T: Send + Sync + 'static,
    DeviceId<BindingsCtx>: TryFromFidlWithContext<NonZeroU64, Error = DeviceNotFoundError>,
    WeakDeviceId<BindingsCtx>: TryIntoFidlWithContext<NonZeroU64, Error = DeviceNotFoundError>,
{
    type Request = fposix_socket::SynchronousDatagramSocketRequest;
    type RequestStream = fposix_socket::SynchronousDatagramSocketRequestStream;
    type CloseResponder = fposix_socket::SynchronousDatagramSocketCloseResponder;
    type SetupArgs = Option<fidl_fuchsia_net::Marks>;

    fn setup(&mut self, ctx: &mut Ctx, marks: Option<fidl_fuchsia_net::Marks>) {
        for (domain, mark) in marks.into_iter().map(fidl_fuchsia_net_ext::Marks::from).flatten() {
            T::set_mark(ctx, &self.info.id, domain.into_core(), Mark(Some(mark)))
        }
    }

    async fn handle_request(
        &mut self,
        ctx: &mut Ctx,
        request: Self::Request,
    ) -> ControlFlow<Self::CloseResponder, Option<Self::RequestStream>> {
        RequestHandler { ctx, data: self }.handle_request(request)
    }

    async fn close(self, ctx: &mut Ctx) {
        let id = self.info.id;
        T::close(ctx, id).await;
    }
}

/// A borrow into a [`SocketWorker`]'s state.
struct RequestHandler<'a, I: BindingsDataIpExt, T: Transport<I>> {
    ctx: &'a mut crate::bindings::Ctx,
    data: &'a mut BindingData<I, T>,
}

impl<'a, I, T> RequestHandler<'a, I, T>
where
    I: IpExt + IpSockAddrExt + BindingsDataIpExt,
    T: Transport<Ipv4>,
    T: Transport<Ipv6>,
    T: TransportState<I>,
    T: Send + Sync + 'static,
    DeviceId<BindingsCtx>: TryFromFidlWithContext<NonZeroU64, Error = DeviceNotFoundError>,
    WeakDeviceId<BindingsCtx>: TryIntoFidlWithContext<NonZeroU64, Error = DeviceNotFoundError>,
{
    fn handle_request(
        mut self,
        request: fposix_socket::SynchronousDatagramSocketRequest,
    ) -> ControlFlow<
        fposix_socket::SynchronousDatagramSocketCloseResponder,
        Option<fposix_socket::SynchronousDatagramSocketRequestStream>,
    > {
        type Request = fposix_socket::SynchronousDatagramSocketRequest;

        match request {
            Request::Describe { responder } => {
                responder.send(self.describe()).unwrap_or_log("failed to respond")
            }
            Request::Connect { addr, responder } => {
                let result = self.connect(addr);
                responder
                    .send(result.log_errno_error(OpAndAddr { op: "connect", addr: Some(addr) }))
                    .unwrap_or_log("failed to respond");
            }
            Request::Disconnect { responder } => {
                let result = self.disconnect();
                responder
                    .send(result.log_errno_error("disconnect"))
                    .unwrap_or_log("failed to respond");
            }
            Request::Clone { request, control_handle: _ } => {
                let channel = fidl::AsyncChannel::from_channel(request.into_channel());
                let stream =
                    fposix_socket::SynchronousDatagramSocketRequestStream::from_channel(channel);
                return ControlFlow::Continue(Some(stream));
            }
            Request::Close { responder } => {
                return ControlFlow::Break(responder);
            }
            Request::Bind { addr, responder } => {
                let result = self.bind(addr);
                responder
                    .send(result.log_errno_error(OpAndAddr { op: "bind", addr: Some(addr) }))
                    .unwrap_or_log("failed to respond");
            }
            Request::Query { responder } => {
                responder
                    .send(fposix_socket::SynchronousDatagramSocketMarker::PROTOCOL_NAME.as_bytes())
                    .unwrap_or_log("failed to respond");
            }
            Request::GetSockName { responder } => {
                let result = self.get_sock_name();
                responder
                    .send(result.log_errno_error("get_sock_name").as_ref().map_err(|e| *e))
                    .unwrap_or_log("failed to respond");
            }
            Request::GetPeerName { responder } => {
                let result = self.get_peer_name();
                responder
                    .send(result.log_errno_error("get_peer_name").as_ref().map_err(|e| *e))
                    .unwrap_or_log("failed to respond");
            }
            Request::Shutdown { mode, responder } => {
                let result = self.shutdown(mode);
                responder
                    .send(result.log_errno_error("shutdown"))
                    .unwrap_or_log("failed to respond")
            }
            Request::RecvMsg { want_addr, data_len, want_control, flags, responder } => {
                let result = self.recv_msg(want_addr, data_len as usize, want_control, flags);
                responder
                    .send(match result.log_errno_error("recvmsg") {
                        Ok((ref addr, ref data, ref control, truncated)) => {
                            Ok((addr.as_ref(), data.as_slice(), control, truncated))
                        }
                        Err(err) => Err(err),
                    })
                    .unwrap_or_log("failed to respond")
            }
            Request::SendMsg { addr, data, control: _, flags: _, responder } => {
                let addr = addr.map(|addr| *addr);
                // TODO(https://fxbug.dev/42094933): handle control.
                let result = self.send_msg(addr, data);
                responder
                    .send(result.log_errno_error(OpAndAddr { op: "sendmsg", addr }))
                    .unwrap_or_log("failed to respond");
            }
            Request::GetInfo { responder } => {
                let result = self.get_sock_info();
                responder
                    .send(result.log_errno_error("get_info"))
                    .unwrap_or_log("failed to respond")
            }
            Request::GetTimestamp { responder } => {
                responder.send(Ok(self.get_timestamp_option())).unwrap_or_log("failed to respond")
            }
            Request::SetTimestamp { value, responder } => responder
                .send(Ok(self.set_timestamp_option(value)))
                .unwrap_or_log("failed to respond"),
            Request::GetOriginalDestination { responder } => {
                responder.send(Err(fposix::Errno::Enoprotoopt)).unwrap_or_log("failed to respond");
            }
            Request::GetError { responder } => {
                let err = T::take_pending_error(self.ctx, &self.data.info.id);
                let result = err.map(|e| e.to_errno()).map_or(Ok(()), Err);
                responder.send(result).unwrap_or_log("failed to respond");
            }
            Request::SetSendBuffer { value_bytes, responder } => {
                self.set_send_buffer(value_bytes);
                responder.send(Ok(())).unwrap_or_log("failed to respond");
            }
            Request::GetSendBuffer { responder } => {
                let send_buffer = self.get_send_buffer();
                responder.send(Ok(send_buffer)).unwrap_or_log("failed to respond");
            }
            Request::SetReceiveBuffer { value_bytes, responder } => {
                responder
                    .send({
                        self.set_max_receive_buffer_size(value_bytes);
                        Ok(())
                    })
                    .unwrap_or_log("failed to respond");
            }
            Request::GetReceiveBuffer { responder } => {
                responder
                    .send(Ok(self.get_max_receive_buffer_size()))
                    .unwrap_or_log("failed to respond");
            }
            Request::SetReuseAddress { value, responder } => {
                let result = self.set_reuse_addr(value);
                responder
                    .send(result.log_errno_error("set_reuse_addr"))
                    .unwrap_or_log("failed to respond");
            }
            Request::GetReuseAddress { responder } => {
                responder.send(Ok(self.get_reuse_addr())).unwrap_or_log("failed to respond");
            }
            Request::SetReusePortDeprecated { value, responder } => {
                let value = if value {
                    ReusePortOption::Enabled(self.data.sharing_domain)
                } else {
                    ReusePortOption::Disabled
                };
                let result = self.set_reuse_port(value, None);
                responder
                    .send(result.log_errno_error("set_reuse_port"))
                    .unwrap_or_log("failed to respond");
            }
            Request::SetReusePort { value, responder } => {
                let (value, token) = match value {
                    fposix_socket::ReusePortOption::Disabled(fposix_socket::Empty) => {
                        (ReusePortOption::Disabled, None)
                    }
                    fposix_socket::ReusePortOption::Enabled(token) => {
                        let koid =
                            token.koid().expect("invalid sharing domain token in ReusePortOption");
                        (ReusePortOption::Enabled(SharingDomain::new(koid.raw_koid())), Some(token))
                    }
                };

                let result = self.set_reuse_port(value, token);
                responder
                    .send(result.log_errno_error("set_reuse_port"))
                    .unwrap_or_log("failed to respond");
            }
            Request::GetReusePort { responder } => {
                responder.send(Ok(self.get_reuse_port())).unwrap_or_log("failed to respond");
            }
            Request::GetAcceptConn { responder } => {
                respond_not_supported!("syncudp::GetAcceptConn", responder)
            }
            Request::SetBindToDevice { value, responder } => {
                let identifier = (!value.is_empty()).then_some(value.as_str());
                let result = self.bind_to_device(identifier).log_errno_error("set_bind_to_device");
                responder.send(result).unwrap_or_log("failed to respond");
            }
            Request::GetBindToDevice { responder } => {
                let result = self.get_bound_device().log_errno_error("get_bind_to_device");
                responder
                    .send(match result {
                        Ok(ref d) => Ok(d.as_deref().unwrap_or("")),
                        Err(e) => Err(e),
                    })
                    .unwrap_or_log("failed to respond")
            }
            Request::SetBindToInterfaceIndex { value, responder } => {
                let result =
                    self.bind_to_device_index(value).log_errno_error("set_bind_to_if_index");
                responder.send(result).unwrap_or_log("failed to respond");
            }
            Request::GetBindToInterfaceIndex { responder } => {
                let result = self.get_bound_device_index().log_errno_error("get_bind_to_if_index");
                responder
                    .send(match result {
                        Ok(d) => Ok(d.map(|d| d.get()).unwrap_or(0)),
                        Err(e) => Err(e),
                    })
                    .unwrap_or_log("failed to respond")
            }
            Request::SetBroadcast { value, responder } => {
                let result = self.set_broadcast(value).map_err(IntoErrno::into_errno_error);
                responder
                    .send(result.log_errno_error("syncudp::SetBroadcast"))
                    .unwrap_or_log("failed to respond");
            }
            Request::GetBroadcast { responder } => {
                responder.send(Ok(self.get_broadcast())).unwrap_or_log("failed to respond");
            }
            Request::SetKeepAlive { value: _, responder } => {
                respond_not_supported!("syncudp::SetKeepAlive", responder)
            }
            Request::GetKeepAlive { responder } => {
                respond_not_supported!("syncudp::GetKeepAlive", responder)
            }
            Request::SetLinger { linger: _, length_secs: _, responder } => {
                respond_not_supported!("syncudp::SetLinger", responder)
            }
            Request::GetLinger { responder } => {
                // Return the default since we don't support `SetLinger`.
                responder.send(Ok((false, 0))).unwrap_or_log("failed to respond")
            }
            Request::SetOutOfBandInline { value: _, responder } => {
                respond_not_supported!("syncudp::SetOutOfBandInline", responder)
            }
            Request::GetOutOfBandInline { responder } => {
                respond_not_supported!("syncudp::GetOutOfBandInline", responder)
            }
            Request::SetNoCheck { value: _, responder } => {
                respond_not_supported!("syncudp::value", responder)
            }
            Request::GetNoCheck { responder } => {
                respond_not_supported!("syncudp::GetNoCheck", responder)
            }
            Request::SetIpv6Only { value, responder } => {
                let result = self.set_dual_stack_enabled(!value);
                responder
                    .send(result.log_errno_error("set_ipv6_only"))
                    .unwrap_or_log("failed to respond");
            }
            Request::GetIpv6Only { responder } => {
                let result = self.get_dual_stack_enabled().map(|enabled| !enabled);
                responder
                    .send(result.log_errno_error("get_ipv6_only"))
                    .unwrap_or_log("failed to respond");
            }
            Request::SetIpv6TrafficClass { value, responder } => {
                let value: Option<u8> = value.into_core();
                let result = self.set_traffic_class(Ipv6::VERSION, value.unwrap_or(0));
                responder
                    .send(result.log_errno_error("set_ipv6_traffic_class"))
                    .unwrap_or_log("failed to respond")
            }
            Request::GetIpv6TrafficClass { responder } => {
                let result = self.get_traffic_class(Ipv6::VERSION);
                responder
                    .send(result.log_errno_error("get_ipv6_traffic_class"))
                    .unwrap_or_log("failed to respond")
            }
            Request::SetIpv6MulticastInterface { value, responder } => {
                let result = self.set_multicast_interface_ipv6(NonZeroU64::new(value));
                responder
                    .send(result.log_errno_error("set_ipv6_multicast_interface"))
                    .unwrap_or_log("failed to respond")
            }
            Request::GetIpv6MulticastInterface { responder } => {
                let result = self
                    .get_multicast_interface_ipv6()
                    .map(|v| v.map(NonZeroU64::get).unwrap_or(0));
                responder
                    .send(result.log_errno_error("get_ipv6_multicast_interface"))
                    .unwrap_or_log("failed to respond")
            }
            Request::SetIpv6UnicastHops { value, responder } => {
                let result = self.set_unicast_hop_limit(Ipv6::VERSION, value);
                responder
                    .send(result.log_errno_error("set_ipv6_unicast_hops"))
                    .unwrap_or_log("failed to respond")
            }
            Request::GetIpv6UnicastHops { responder } => {
                let result = self.get_unicast_hop_limit(Ipv6::VERSION);
                responder
                    .send(result.log_errno_error("get_ipv6_unicast_hops"))
                    .unwrap_or_log("failed to respond")
            }
            Request::SetIpv6MulticastHops { value, responder } => {
                let result = self.set_multicast_hop_limit(Ipv6::VERSION, value);
                responder
                    .send(result.log_errno_error("set_ipv6_multicast_hops"))
                    .unwrap_or_log("failed to respond")
            }
            Request::GetIpv6MulticastHops { responder } => {
                let result = self.get_multicast_hop_limit(Ipv6::VERSION);
                responder
                    .send(result.log_errno_error("get_ipv6_multicast_hops"))
                    .unwrap_or_log("failed to respond")
            }
            Request::SetIpv6MulticastLoopback { value, responder } => {
                let result = self.set_multicast_loop(Ipv6::VERSION, value);
                responder
                    .send(result.log_errno_error("set_ipv6_multicast_loop"))
                    .unwrap_or_log("failed to respond")
            }
            Request::GetIpv6MulticastLoopback { responder } => {
                let result = self.get_multicast_loop(Ipv6::VERSION);
                responder
                    .send(result.log_errno_error("get_ipv6_multicast_loop"))
                    .unwrap_or_log("failed to respond")
            }
            Request::SetIpTtl { value, responder } => {
                let result = self.set_unicast_hop_limit(Ipv4::VERSION, value);
                responder
                    .send(result.log_errno_error("set_ip_ttl"))
                    .unwrap_or_log("failed to respond")
            }
            Request::GetIpTtl { responder } => {
                let result = self.get_unicast_hop_limit(Ipv4::VERSION);
                responder
                    .send(result.log_errno_error("get_ip_ttl"))
                    .unwrap_or_log("failed to respond")
            }
            Request::SetIpMulticastTtl { value, responder } => {
                let result = self.set_multicast_hop_limit(Ipv4::VERSION, value);
                responder
                    .send(result.log_errno_error("set_ip_multicast_ttl"))
                    .unwrap_or_log("failed to respond")
            }
            Request::GetIpMulticastTtl { responder } => {
                let result = self.get_multicast_hop_limit(Ipv4::VERSION);
                responder
                    .send(result.log_errno_error("get_ip_multicast_ttl"))
                    .unwrap_or_log("failed to respond")
            }
            Request::SetIpMulticastInterface { iface, address, responder } => {
                let result = self.set_multicast_interface_ipv4(NonZeroU64::new(iface), address);
                responder
                    .send(result.log_errno_error("set_multicast_interface"))
                    .unwrap_or_log("failed to respond")
            }
            Request::GetIpMulticastInterface { responder } => {
                let result = self.get_multicast_interface_ipv4();
                responder
                    .send(
                        result
                            .log_errno_error("get_ip_multicast_interface")
                            .as_ref()
                            .map_err(|e| *e),
                    )
                    .unwrap_or_log("failed to respond")
            }
            Request::SetIpMulticastLoopback { value, responder } => {
                let result = self.set_multicast_loop(Ipv4::VERSION, value);
                responder
                    .send(result.log_errno_error("set_ip_multicast_loop"))
                    .unwrap_or_log("failed to respond")
            }
            Request::GetIpMulticastLoopback { responder } => {
                let result = self.get_multicast_loop(Ipv4::VERSION);
                responder
                    .send(result.log_errno_error("get_ip_multicast_loop"))
                    .unwrap_or_log("failed to respond")
            }
            Request::SetIpTypeOfService { value, responder } => {
                let result = self.set_traffic_class(Ipv4::VERSION, value);
                responder
                    .send(result.log_errno_error("set_ip_type_of_service"))
                    .unwrap_or_log("failed to respond")
            }
            Request::GetIpTypeOfService { responder } => {
                let result = self.get_traffic_class(Ipv4::VERSION);
                responder
                    .send(result.log_errno_error("get_ip_type_of_service"))
                    .unwrap_or_log("failed to respond")
            }
            Request::AddIpMembership { membership, responder } => {
                let result = self.set_multicast_membership(membership, true);
                responder
                    .send(result.log_errno_error("add_ip_membership"))
                    .unwrap_or_log("failed to respond");
            }
            Request::DropIpMembership { membership, responder } => {
                let result = self.set_multicast_membership(membership, false);
                responder
                    .send(result.log_errno_error("drop_ip_membership"))
                    .unwrap_or_log("failed to respond");
            }
            Request::SetIpTransparent { value, responder } => {
                let result = self.set_ip_transparent(value).map_err(IntoErrno::into_errno_error);
                responder
                    .send(result.log_errno_error("set_ip_transparent"))
                    .unwrap_or_log("failed to respond");
            }
            Request::GetIpTransparent { responder } => {
                responder.send(Ok(self.get_ip_transparent())).unwrap_or_log("failed to respond");
            }
            Request::SetIpReceiveOriginalDestinationAddress { value, responder } => {
                self.data.ip_receive_original_destination_address = value;
                responder.send(Ok(())).unwrap_or_log("failed to respond");
            }
            Request::GetIpReceiveOriginalDestinationAddress { responder } => {
                responder
                    .send(Ok(self.data.ip_receive_original_destination_address))
                    .unwrap_or_log("failed to respond");
            }
            Request::AddIpv6Membership { membership, responder } => {
                let result = self.set_multicast_membership(membership, true);
                responder
                    .send(result.log_errno_error("add_ipv6_membership"))
                    .unwrap_or_log("failed to respond");
            }
            Request::DropIpv6Membership { membership, responder } => {
                let result = self.set_multicast_membership(membership, false);
                responder
                    .send(result.log_errno_error("drop_ipv6_membership"))
                    .unwrap_or_log("failed to respond");
            }
            Request::SetIpv6ReceiveTrafficClass { value, responder } => {
                let result = self.set_ipv6_receive_traffic_class(value);
                responder
                    .send(result.log_errno_error("set_ipv6_receive_traffic_class"))
                    .unwrap_or_log("failed to respond");
            }
            Request::GetIpv6ReceiveTrafficClass { responder } => {
                let result = self.get_ipv6_receive_traffic_class();
                responder
                    .send(result.log_errno_error("get_ipv6_receive_traffic_class"))
                    .unwrap_or_log("failed to respond");
            }
            Request::SetIpv6ReceiveHopLimit { value, responder } => {
                warn!("syncudp::SetIpv6ReceiveHopLimit({value}) is not implemented, returning Ok");
                responder.send(Ok(())).unwrap_or_log("failed to respond");
            }
            Request::GetIpv6ReceiveHopLimit { responder } => {
                respond_not_supported!("syncudp::GetIpv6ReceiveHopLimit", responder)
            }
            Request::SetIpReceiveTypeOfService { value, responder } => {
                responder
                    .send(Ok(self.ip_set_receive_type_of_service(value)))
                    .unwrap_or_log("failed to respond");
            }
            Request::GetIpReceiveTypeOfService { responder } => {
                responder
                    .send(Ok(self.ip_get_receive_type_of_service()))
                    .unwrap_or_log("failed to respond");
            }
            Request::SetIpv6ReceivePacketInfo { value, responder } => {
                let result = self.set_ipv6_recv_pkt_info(value);
                responder
                    .send(result.log_errno_error("set_ipv6_recv_pkt_info"))
                    .unwrap_or_log("failed to respond");
            }
            Request::GetIpv6ReceivePacketInfo { responder } => {
                let result = self.get_ipv6_recv_pkt_info();
                responder
                    .send(result.log_errno_error("get_ipv6_recv_pkt_info"))
                    .unwrap_or_log("failed to respond");
            }
            Request::SetIpReceiveTtl { value: _, responder } => {
                respond_not_supported!("syncudp::SetIpReceiveTtl", responder)
            }
            Request::GetIpReceiveTtl { responder } => {
                respond_not_supported!("syncudp::GetIpReceiveTtl", responder)
            }
            Request::SetIpPacketInfo { value, responder } => {
                self.data.ip_pkt_info = value;
                responder.send(Ok(())).unwrap_or_log("failed to respond");
            }
            Request::GetIpPacketInfo { responder } => {
                responder.send(Ok(self.data.ip_pkt_info)).unwrap_or_log("failed to respond");
            }
            Request::SetMark { domain, mark, responder } => {
                self.set_mark(domain, mark);
                responder.send(Ok(())).unwrap_or_log("failed to respond")
            }
            Request::GetMark { domain, responder } => {
                responder.send(Ok(&self.get_mark(domain))).unwrap_or_log("failed to respond")
            }
            Request::GetCookie { responder } => responder
                .send(Ok(self.get_cookie().export_value()))
                .unwrap_or_log("failed to respond"),
        }
        ControlFlow::Continue(None)
    }

    fn describe(&self) -> fposix_socket::SynchronousDatagramSocketDescribeResponse {
        let peer = self
            .data
            .peer_event
            .duplicate_handle(
                // The peer doesn't need to be able to signal, just receive signals,
                // so attenuate that right when duplicating.
                zx::Rights::BASIC,
            )
            .expect("failed to duplicate");

        fposix_socket::SynchronousDatagramSocketDescribeResponse {
            event: Some(peer),
            ..Default::default()
        }
    }

    fn external_data(&self) -> &DatagramSocketExternalData<I> {
        T::external_data(&self.data.info.id)
    }

    fn get_max_receive_buffer_size(&self) -> u64 {
        self.external_data()
            .message_queue
            .lock()
            .max_available_messages_size()
            .get()
            .try_into()
            .unwrap_or(u64::MAX)
    }

    fn set_max_receive_buffer_size(&mut self, max_bytes: u64) {
        let max_bytes = max_bytes.try_into().ok_checked::<TryFromIntError>().unwrap_or(usize::MAX);
        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        let settings = <T as Transport<I>>::get_rcvbuf_settings(ctx);
        <T as Transport<I>>::external_data(id)
            .message_queue
            .lock()
            .set_max_available_messages_size(max_bytes, &*settings)
    }

    /// Handles a [POSIX socket connect request].
    ///
    /// [POSIX socket connect request]: fposix_socket::SynchronousDatagramSocketRequest::Connect
    fn connect(self, addr: fnet::SocketAddress) -> Result<(), ErrnoError> {
        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        let sockaddr =
            I::SocketAddress::from_sock_addr(<T as Transport<I>>::maybe_map_sock_addr(addr))?;
        trace!("connect sockaddr: {:?}", sockaddr);
        let (remote_addr, remote_port) = sockaddr
            .try_into_core_with_ctx(ctx.bindings_ctx())
            .map_err(IntoErrno::into_errno_error)?;

        T::connect(ctx, id, remote_addr, remote_port.into())
            .map_err(IntoErrno::into_errno_error)?;

        Ok(())
    }

    /// Handles a [POSIX socket bind request].
    ///
    /// [POSIX socket bind request]: fposix_socket::SynchronousDatagramSocketRequest::Bind
    fn bind(self, addr: fnet::SocketAddress) -> Result<(), ErrnoError> {
        // Match Linux and return `Einval` when asked to bind an IPv6 socket to
        // an Ipv4 address. This Errno is unique to bind.
        let sockaddr = match (I::VERSION, &addr) {
            (IpVersion::V6, fnet::SocketAddress::Ipv4(_)) => Err(ErrnoError::new(
                fposix::Errno::Einval,
                "cannot bind IPv6 socket to IPv4 address",
            )),
            (_, _) => I::SocketAddress::from_sock_addr(addr),
        }?;
        trace!("bind sockaddr: {:?}", sockaddr);

        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        let (sockaddr, port) =
            TryFromFidlWithContext::try_from_fidl_with_ctx(ctx.bindings_ctx(), sockaddr)
                .map_err(IntoErrno::into_errno_error)?;
        let local_port = T::LocalIdentifier::from_u16(port);

        T::bind(ctx, id, sockaddr, local_port).map_err(IntoErrno::into_errno_error)?;
        Ok(())
    }

    /// Handles a [POSIX socket disconnect request].
    ///
    /// [POSIX socket connect request]: fposix_socket::SynchronousDatagramSocketRequest::Disconnect
    fn disconnect(self) -> Result<(), ErrnoError> {
        trace!("disconnect socket");

        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        T::disconnect(ctx, id).map_err(IntoErrno::into_errno_error)?;
        Ok(())
    }

    /// Handles a [POSIX socket get_sock_name request].
    ///
    /// [POSIX socket get_sock_name request]: fposix_socket::SynchronousDatagramSocketRequest::GetSockName
    fn get_sock_name(self) -> Result<fnet::SocketAddress, ErrnoError> {
        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        let l: LocalAddress<_, _, _> = T::get_socket_info(ctx, id).into_fidl();
        l.try_into_fidl_with_ctx(ctx.bindings_ctx()).map(SockAddr::into_sock_addr)
    }

    /// Handles a [POSIX socket get_info request].
    ///
    /// [POSIX socket get_info request]: fposix_socket::SynchronousDatagramSocketRequest::GetInfo
    fn get_sock_info(
        self,
    ) -> Result<(fposix_socket::Domain, fposix_socket::DatagramSocketProtocol), ErrnoError> {
        let domain = match I::VERSION {
            IpVersion::V4 => fposix_socket::Domain::Ipv4,
            IpVersion::V6 => fposix_socket::Domain::Ipv6,
        };
        let protocol = match <T as Transport<I>>::PROTOCOL {
            DatagramProtocol::Udp => fposix_socket::DatagramSocketProtocol::Udp,
            DatagramProtocol::IcmpEcho => fposix_socket::DatagramSocketProtocol::IcmpEcho,
        };

        Ok((domain, protocol))
    }

    /// Handles a [POSIX socket get_peer_name request].
    ///
    /// [POSIX socket get_peer_name request]: fposix_socket::SynchronousDatagramSocketRequest::GetPeerName
    fn get_peer_name(self) -> Result<fnet::SocketAddress, ErrnoError> {
        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        T::get_socket_info(ctx, id).try_into_fidl().and_then(|r: RemoteAddress<_, _, _>| {
            r.try_into_fidl_with_ctx(ctx.bindings_ctx()).map(SockAddr::into_sock_addr)
        })
    }

    fn recv_msg(
        self,
        want_addr: bool,
        data_len: usize,
        want_control: bool,
        recv_flags: fposix_socket::RecvMsgFlags,
    ) -> Result<
        (Option<fnet::SocketAddress>, Vec<u8>, fposix_socket::DatagramSocketRecvControlData, u32),
        ErrnoError,
    > {
        trace_duration!("datagram::recv_msg");

        let Self {
            ctx,
            data:
                BindingData {
                    info: SocketControlInfo { id, .. },
                    version_specific_data,
                    ip_receive_original_destination_address,
                    ip_recv_tos,
                    ip_pkt_info,
                    timestamp_option,
                    ..
                },
        } = self;

        if let Some(err) = T::take_pending_error(ctx, id) {
            return Err(ErrnoError::new(err.to_errno(), "pending error on socket"));
        }

        let front = {
            let mut messages = <T as Transport<I>>::external_data(id).message_queue.lock();
            if recv_flags.contains(fposix_socket::RecvMsgFlags::PEEK) {
                messages.peek().cloned()
            } else {
                messages.pop()
            }
        };

        let AvailableMessage {
            interface_id,
            source_addr,
            source_port,
            destination_addr,
            destination_port,
            timestamp,
            mut data,
            dscp_and_ecn,
        } = match front {
            None => {
                // This is safe from races only because the setting of the
                // shutdown flag can only be done by the worker executing this
                // code. Otherwise, a packet being delivered, followed by
                // another thread setting the shutdown flag, then this check
                // executing, could result in a race that causes this this code
                // to signal EOF with a packet still waiting.
                let shutdown = T::get_shutdown(ctx, id);
                return match shutdown {
                    Some(ShutdownType::Receive | ShutdownType::SendAndReceive) => {
                        // Return empty data to signal EOF.
                        Ok((
                            None,
                            Vec::new(),
                            fposix_socket::DatagramSocketRecvControlData::default(),
                            0,
                        ))
                    }
                    None | Some(ShutdownType::Send) => Err(ErrnoError::new(
                        fposix::Errno::Eagain,
                        "no available message and receive side not shut down",
                    )),
                };
            }
            Some(front) => front,
        };
        let addr = want_addr.then(|| {
            I::SocketAddress::new(
                SpecifiedAddr::new(source_addr).map(|a| {
                    core_socket::StrictlyZonedAddr::new_with_zone(a, || interface_id).into_inner()
                }),
                source_port,
            )
            .into_sock_addr()
        });
        let truncated = data.len().saturating_sub(data_len);
        data.truncate(data_len);

        let mut network: Option<fposix_socket::NetworkSocketRecvControlData> = None;
        if want_control {
            // Destination IPv4 address if this was an IPv4 packet.
            let ipv4_dest_ip = I::map_ip_in(
                destination_addr,
                |ipv4_addr| Some(ipv4_addr),
                |ipv6_addr| ipv6_addr.to_ipv4_mapped(),
            );

            let mut ip_data: Option<fposix_socket::IpRecvControlData> = None;

            // `IP_TOS` is included only for IPv4 packets.
            if *ip_recv_tos && ipv4_dest_ip.is_some() {
                ip_data.get_or_insert_default().tos = Some(dscp_and_ecn.raw());
            }

            if *ip_receive_original_destination_address {
                // `IP_ORIGDSTADDR` is included only for IPv4 packets.
                if let Some(ipv4_dest_ip) = ipv4_dest_ip {
                    ip_data.get_or_insert_default().original_destination_address =
                        Some(fnet::SocketAddress::Ipv4(fnet::Ipv4SocketAddress {
                            address: ipv4_dest_ip.into_fidl(),
                            port: destination_port,
                        }))
                }
            }

            if *ip_pkt_info {
                if let Some(ipv4_dest_ip) = ipv4_dest_ip {
                    let ipv4_src_ip = I::map_ip_in(
                        source_addr,
                        |ipv4_addr| Some(ipv4_addr),
                        |ipv6_addr| ipv6_addr.to_ipv4_mapped(),
                    );
                    let local_addr = Self::select_ipv4_pktinfo_local_addr(
                        ctx,
                        interface_id,
                        ipv4_src_ip,
                        ipv4_dest_ip,
                    );
                    ip_data.get_or_insert_default().pktinfo =
                        Some(fposix_socket::Ipv4PktInfoRecvControlData {
                            iface: interface_id.into(),
                            local_addr: local_addr.into_fidl(),
                            header_destination_addr: ipv4_dest_ip.into_fidl(),
                        });
                }
            }

            let ipv6_control_data = I::map_ip_in(
                (version_specific_data, destination_addr, IpInvariant(interface_id)),
                |(Ipv4BindingsData {}, _ipv4_dst_addr, _interface_id)| None,
                |(
                    Ipv6BindingsData { recv_pkt_info, recv_tclass },
                    ipv6_dst_addr,
                    IpInvariant(interface_id),
                )| {
                    let mut ipv6_data: Option<fposix_socket::Ipv6RecvControlData> = None;

                    // An IPv6-mapped IPv4 packet will receive both IPv4 and
                    // IPv6 info.
                    if *recv_pkt_info {
                        ipv6_data.get_or_insert_default().pktinfo =
                            Some(fposix_socket::Ipv6PktInfoRecvControlData {
                                iface: interface_id.into(),
                                header_destination_addr: ipv6_dst_addr.into_fidl(),
                            });
                    }

                    // `IPV6_TCLASS` is included only if this is an IPv6 packet.
                    if *recv_tclass && ipv4_dest_ip.is_none() {
                        ipv6_data.get_or_insert_default().tclass = Some(dscp_and_ecn.raw());
                    }

                    // TODO(https://fxbug.dev/326102020): Support SOL_IPV6,
                    // IPV6_RECVHOPLIMIT.
                    ipv6_data
                },
            );

            let timestamp =
                (*timestamp_option != fposix_socket::TimestampOption::Disabled).then(|| {
                    fposix_socket::Timestamp {
                        nanoseconds: timestamp.into_nanos(),
                        requested: *timestamp_option,
                    }
                });

            if ip_data.is_some() {
                network.get_or_insert_default().ip = ip_data;
            }

            if ipv6_control_data.is_some() {
                network.get_or_insert_default().ipv6 = ipv6_control_data;
            }

            if let Some(timestamp) = timestamp {
                network.get_or_insert_default().socket.get_or_insert_default().timestamp =
                    Some(timestamp);
            };
        };

        let control_data =
            fposix_socket::DatagramSocketRecvControlData { network, ..Default::default() };

        Ok((addr, data, control_data, truncated.try_into().unwrap_or(u32::MAX)))
    }

    /// Selects the preferred source address for replies to a packet sent from
    /// `ipv4_src_ip` to `ipv4_dest_ip` and received on `interface_id`.
    // Note: the logic that this function uses is based on the logic used by
    // Linux for the sake of compatibility (see `fib_compute_spec_dst`).
    fn select_ipv4_pktinfo_local_addr(
        ctx: &mut Ctx,
        interface_id: BindingId,
        ipv4_src_ip: Option<Ipv4Addr>,
        ipv4_dest_ip: Ipv4Addr,
    ) -> Ipv4Addr {
        let addrs = DeviceId::try_from_fidl_with_ctx(ctx.bindings_ctx(), interface_id)
            .ok()
            .map(|device_id| ctx.api().device_ip::<Ipv4>().get_assigned_ip_addr_subnets(&device_id))
            .unwrap_or_default();

        let is_subnet_broadcast = addrs.iter().any(|addr_subnet| {
            addr_subnet.subnet().prefix() < 31 && addr_subnet.subnet().broadcast() == ipv4_dest_ip
        });

        // If `ipv4_dest_ip` is unicast, use it as the local addr.
        if !ipv4_dest_ip.is_multicast()
            && !ipv4_dest_ip.is_limited_broadcast()
            && !is_subnet_broadcast
            && ipv4_dest_ip.is_specified()
        {
            return ipv4_dest_ip;
        }

        // Otherwise, see if there's a route to the source address and use the
        // route's src addr if so. An unspecified addr isn't routable so it'll
        // hit the fallback below.
        if let Some(src) = ipv4_src_ip.and_then(RoutableIpAddr::new) {
            if let Ok(route) =
                ctx.api().routes::<Ipv4>().resolve_route(Some(src), &RouteResolveOptions::default())
            {
                return route.src_addr.addr();
            }
        }

        // If there's no route to `ipv4_src_ip`, use the first address assigned
        // to `interface_id`.
        if let Some(first) = addrs.first() {
            return first.addr().get();
        }

        Ipv4Addr::new([0, 0, 0, 0])
    }

    fn send_msg(self, addr: Option<fnet::SocketAddress>, data: Vec<u8>) -> Result<i64, ErrnoError> {
        trace_duration!("datagram::send_msg");
        let remote_addr = addr
            .map(|addr| {
                I::SocketAddress::from_sock_addr(<T as Transport<I>>::maybe_map_sock_addr(addr))
            })
            .transpose()?;
        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        let remote = remote_addr
            .map(|remote_addr| {
                let (remote_addr, port) =
                    TryFromFidlWithContext::try_from_fidl_with_ctx(ctx.bindings_ctx(), remote_addr)
                        .map_err(IntoErrno::into_errno_error)?;
                Ok((remote_addr, port.into()))
            })
            .transpose()?;
        let len = data.len() as i64;
        let body = Buf::new(data, ..);
        T::send(ctx, id, remote, body).map_err(|e| e.into_errno_error()).map(|()| len)
    }

    fn bind_to_device_id(self, device: Option<DeviceId<BindingsCtx>>) -> Result<(), ErrnoError> {
        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;

        T::set_socket_device(ctx, id, device.as_ref()).map_err(IntoErrno::into_errno_error)
    }

    fn bind_to_device(self, device: Option<&str>) -> Result<(), ErrnoError> {
        let Self { ctx, .. } = &self;
        let device = device
            .map(|name| {
                ctx.bindings_ctx()
                    .devices
                    .get_device_by_name(name)
                    .ok_or_else(|| DeviceNotFoundError.into_errno_error())
            })
            .transpose()?;

        self.bind_to_device_id(device)
    }

    fn bind_to_device_index(self, device: u64) -> Result<(), ErrnoError> {
        let Self { ctx, .. } = &self;

        // If `device` is 0, then this will clear the bound device.
        let device = NonZeroU64::new(device)
            .map(|index| {
                ctx.bindings_ctx()
                    .devices
                    .get_core_id(index)
                    .ok_or_else(|| DeviceNotFoundError.into_errno_error())
            })
            .transpose()?;

        self.bind_to_device_id(device)
    }

    fn get_bound_device_id(self) -> Result<Option<DeviceId<BindingsCtx>>, ErrnoError> {
        // NB: Ensure that we do not return a device that was removed from the
        // stack. This matches Linux behavior.
        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        let device = match T::get_bound_device(ctx, id) {
            None => return Ok(None),
            Some(d) => d,
        };

        device
            .upgrade()
            .ok_or_else(|| ErrnoError::new(fposix::Errno::Enodev, "bound device was removed"))
            .map(Some)
    }

    fn get_bound_device(self) -> Result<Option<String>, ErrnoError> {
        Ok(self.get_bound_device_id()?.map(|core_id| core_id.bindings_id().name.clone()))
    }

    fn get_bound_device_index(self) -> Result<Option<NonZeroU64>, ErrnoError> {
        Ok(self.get_bound_device_id()?.map(|core_id| core_id.bindings_id().id))
    }

    fn set_dual_stack_enabled(self, enabled: bool) -> Result<(), ErrnoError> {
        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        T::set_dual_stack_enabled(ctx, id, enabled).map_err(IntoErrno::into_errno_error)
    }

    fn get_dual_stack_enabled(self) -> Result<bool, ErrnoError> {
        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        T::get_dual_stack_enabled(ctx, id).map_err(IntoErrno::into_errno_error)
    }

    fn set_ipv6_recv_pkt_info(self, new: bool) -> Result<(), ErrnoError> {
        let correct_ip_version: Option<()> = I::map_ip(
            &mut self.data.version_specific_data,
            |_v4_data| None,
            |v6_data| {
                v6_data.recv_pkt_info = new;
                Some(())
            },
        );
        correct_ip_version.ok_or_else(|| {
            ErrnoError::new(
                fposix::Errno::Enoprotoopt,
                "cannot set_ipv6_recv_pkt_info on IPv4 socket",
            )
        })
    }

    fn get_ipv6_recv_pkt_info(self) -> Result<bool, ErrnoError> {
        let correct_ip_version: Option<bool> = I::map_ip(
            &self.data.version_specific_data,
            |_v4_data| None,
            |v6_data| Some(v6_data.recv_pkt_info),
        );
        correct_ip_version.ok_or_else(|| {
            ErrnoError::new(
                fposix::Errno::Eopnotsupp,
                "cannot get_ipv6_recv_pkt_info on IPv4 socket",
            )
        })
    }

    fn set_reuse_addr(self, reuse_addr: bool) -> Result<(), ErrnoError> {
        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        T::set_reuse_addr(ctx, id, reuse_addr).map_err(IntoErrno::into_errno_error)
    }

    fn get_reuse_addr(self) -> bool {
        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        T::get_reuse_addr(ctx, id)
    }

    fn set_reuse_port(
        self,
        reuse_port: ReusePortOption,
        token: Option<zx::Event>,
    ) -> Result<(), ErrnoError> {
        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        T::set_reuse_port(ctx, id, reuse_port, token).map_err(IntoErrno::into_errno_error)
    }

    fn get_reuse_port(self) -> bool {
        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        T::get_reuse_port(ctx, id)
    }

    fn shutdown(self, how: fposix_socket::ShutdownMode) -> Result<(), ErrnoError> {
        let Self { data: BindingData { info: SocketControlInfo { id, .. }, .. }, ctx } = self;
        let how = ShutdownType::from_send_receive(
            how.contains(fposix_socket::ShutdownMode::WRITE),
            how.contains(fposix_socket::ShutdownMode::READ),
        )
        .ok_or_else(|| {
            ErrnoError::new(
                fposix::Errno::Einval,
                "shutdown must shutdown at least one of {read, write}",
            )
        })?;
        T::shutdown(ctx, id, how).map_err(IntoErrno::into_errno_error)?;
        match how {
            ShutdownType::Receive | ShutdownType::SendAndReceive => {
                // Make sure to signal the peer so any ongoing call to
                // receive that is waiting for a signal will poll again.
                <T as Transport<I>>::external_data(id)
                    .message_queue
                    .lock()
                    .listener_mut()
                    .on_readable_changed(true)
            }
            ShutdownType::Send => (),
        }

        Ok(())
    }

    fn set_multicast_membership<
        M: TryIntoCore<(
            MulticastAddr<I::Addr>,
            Option<MulticastInterfaceSelector<I::Addr, NonZeroU64>>,
        )>,
    >(
        self,
        membership: M,
        want_membership: bool,
    ) -> Result<(), ErrnoError>
    where
        M::Error: IntoErrno,
    {
        let (multicast_group, interface) =
            membership.try_into_core().map_err(IntoErrno::into_errno_error)?;
        let interface = interface
            .map_or(MulticastMembershipInterfaceSelector::AnyInterfaceWithRoute, Into::into);

        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;

        let interface = interface
            .try_into_core_with_ctx(ctx.bindings_ctx())
            .map_err(IntoErrno::into_errno_error)?;

        T::set_multicast_membership(ctx, id, multicast_group, interface, want_membership)
            .map_err(IntoErrno::into_errno_error)
    }

    fn set_unicast_hop_limit(
        self,
        ip_version: IpVersion,
        hop_limit: fposix_socket::OptionalUint8,
    ) -> Result<(), ErrnoError> {
        let hop_limit: Option<u8> = hop_limit.into_core();
        let hop_limit = hop_limit
            .map(|u| {
                NonZeroU8::new(u).ok_or_else(|| {
                    ErrnoError::new(fposix::Errno::Einval, "unicast hop limit must be nonzero")
                })
            })
            .transpose()?;

        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        T::set_unicast_hop_limit(ctx, id, hop_limit, ip_version)
            .map_err(IntoErrno::into_errno_error)
    }

    fn set_multicast_hop_limit(
        self,
        ip_version: IpVersion,
        hop_limit: fposix_socket::OptionalUint8,
    ) -> Result<(), ErrnoError> {
        let hop_limit: Option<u8> = hop_limit.into_core();
        // TODO(https://fxbug.dev/42059735): Support setting a multicast hop limit
        // of 0.
        let hop_limit = hop_limit
            .map(|u| {
                NonZeroU8::new(u).ok_or_else(|| {
                    ErrnoError::new(fposix::Errno::Einval, "multicast hop limit must be nonzero")
                })
            })
            .transpose()?;

        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        T::set_multicast_hop_limit(ctx, id, hop_limit, ip_version)
            .map_err(IntoErrno::into_errno_error)
    }

    fn get_unicast_hop_limit(self, ip_version: IpVersion) -> Result<u8, ErrnoError> {
        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        T::get_unicast_hop_limit(ctx, id, ip_version)
            .map(NonZeroU8::get)
            .map_err(IntoErrno::into_errno_error)
    }

    fn get_multicast_hop_limit(self, ip_version: IpVersion) -> Result<u8, ErrnoError> {
        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        T::get_multicast_hop_limit(ctx, id, ip_version)
            .map(NonZeroU8::get)
            .map_err(IntoErrno::into_errno_error)
    }

    fn set_multicast_interface_ipv4(
        self,
        interface: Option<NonZeroU64>,
        addr: fnet::Ipv4Address,
    ) -> Result<(), ErrnoError> {
        // Multicast interface for IPv4 multicast packets can be selected by
        // the IP address. Linux also uses the specified address as the source
        // address for outgoing multicast packets. Our implementation of
        // IP_MULTICAST_IF diverges from Linux: the address is used only to
        // select the interface and is saved to return from `getsockopts()`.

        let Self {
            ctx,
            data: BindingData { info: SocketControlInfo { id, .. }, ipv4_multicast_if_addr, .. },
        } = self;

        let addr: Option<SpecifiedAddr<Ipv4Addr>> = SpecifiedAddr::new(addr.into_core());
        let device_id = match (interface, addr) {
            // If both the interface index and the address are specified then
            // we use the index to select the interface. The address still
            // saved to return it in the future.
            (Some(index), _) => Some(
                // `setsockopt(IP_MULTICAST_IF)` is supposed to fail with
                // `EADDRNOTAVAIL` when the IP or the interface index is invalid. This is
                // different from `IPV6_MULTICAST_IF`.
                TryFromFidlWithContext::try_from_fidl_with_ctx(ctx.bindings_ctx(), index)
                    .map_err(|e| ErrnoError::new(fposix::Errno::Eaddrnotavail, e))?,
            ),
            (None, Some(addr)) => {
                let device = ctx
                    .bindings_ctx()
                    .devices
                    .with_devices(|devices| devices.cloned().collect::<Vec<_>>())
                    .into_iter()
                    .find(|device| {
                        let mut ip_found = false;
                        ctx.api().device_ip::<Ipv4>().for_each_assigned_ip_addr_subnet(
                            device,
                            |ip_subnet| {
                                if ip_subnet.addr() == addr {
                                    ip_found = true;
                                }
                            },
                        );
                        ip_found
                    })
                    .ok_or_else(|| {
                        ErrnoError::new(
                            fposix::Errno::Eaddrnotavail,
                            "address used to specify MULTICAST_IF \
                            not found on any interface",
                        )
                    })?;
                Some(device)
            }
            (None, None) => None,
        };

        T::set_multicast_interface(ctx, id, device_id.as_ref(), Ipv4::VERSION)
            .map_err(IntoErrno::into_errno_error)
            .inspect(|_| *ipv4_multicast_if_addr = addr)
    }

    fn get_multicast_interface_ipv4(self) -> Result<fnet::Ipv4Address, ErrnoError> {
        let Self { data: BindingData { ipv4_multicast_if_addr, .. }, .. } = self;
        Ok(ipv4_multicast_if_addr.map(Into::into).unwrap_or(Ipv4::UNSPECIFIED_ADDRESS).into_fidl())
    }

    fn set_multicast_interface_ipv6(self, interface: Option<NonZeroU64>) -> Result<(), ErrnoError> {
        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        let interface = interface
            .map(|index| TryFromFidlWithContext::try_from_fidl_with_ctx(ctx.bindings_ctx(), index))
            .transpose()
            .map_err(IntoErrno::into_errno_error)?;
        T::set_multicast_interface(ctx, id, interface.as_ref(), Ipv6::VERSION)
            .map_err(IntoErrno::into_errno_error)
    }

    fn get_multicast_interface_ipv6(self) -> Result<Option<NonZeroU64>, ErrnoError> {
        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        T::get_multicast_interface(ctx, id, Ipv6::VERSION)
            .map_err(IntoErrno::into_errno_error)
            .map_err(|e| {
                e.map_errno(|e| match e {
                    // `getsockopt()` should fail with `EOPNOTSUPP` instead of `ENOPROTOOPT`.
                    fposix::Errno::Enoprotoopt => fposix::Errno::Eopnotsupp,
                    e => e,
                })
            })?
            .map(|id| id.try_into_fidl_with_ctx(ctx.bindings_ctx()))
            .transpose()
            .map_err(IntoErrno::into_errno_error)
    }

    fn set_multicast_loop(self, ip_version: IpVersion, loop_: bool) -> Result<(), ErrnoError> {
        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        T::set_multicast_loop(ctx, id, loop_, ip_version).map_err(IntoErrno::into_errno_error)
    }

    fn get_multicast_loop(self, ip_version: IpVersion) -> Result<bool, ErrnoError> {
        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        T::get_multicast_loop(ctx, id, ip_version).map_err(IntoErrno::into_errno_error)
    }

    fn set_ip_transparent(self, value: bool) -> Result<(), T::SetIpTransparentError> {
        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        T::set_ip_transparent(ctx, id, value)
    }

    fn get_ip_transparent(self) -> bool {
        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        T::get_ip_transparent(ctx, id)
    }

    fn set_broadcast(self, value: bool) -> Result<(), T::SetBroadcastError> {
        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        T::set_broadcast(ctx, id, value)
    }

    fn get_broadcast(self) -> bool {
        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        T::get_broadcast(ctx, id)
    }

    fn get_timestamp_option(self) -> fposix_socket::TimestampOption {
        self.data.timestamp_option
    }

    fn set_timestamp_option(self, value: fposix_socket::TimestampOption) {
        self.data.timestamp_option = value;
    }

    fn set_traffic_class(self, ip_version: IpVersion, traffic_class: u8) -> Result<(), ErrnoError> {
        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        T::set_dscp_and_ecn(ctx, id, traffic_class.into(), ip_version)
            .map_err(IntoErrno::into_errno_error)
    }

    fn get_traffic_class(self, ip_version: IpVersion) -> Result<u8, ErrnoError> {
        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        T::get_dscp_and_ecn(ctx, id, ip_version)
            .map_err(IntoErrno::into_errno_error)
            .map_err(|e| {
                e.map_errno(|e| match e {
                    // fail with `EOPNOTSUPP` instead of `ENOPROTOOPT`.
                    fposix::Errno::Enoprotoopt => fposix::Errno::Eopnotsupp,
                    e => e,
                })
            })
            .map(DscpAndEcn::raw)
    }

    fn set_ipv6_receive_traffic_class(self, value: bool) -> Result<(), ErrnoError> {
        I::map_ip_in(
            &mut self.data.version_specific_data,
            |_v4_data| {
                Err(ErrnoError::new(
                    fposix::Errno::Enoprotoopt,
                    "cannot set_ipv6_receive_traffic_class on IPv4 socket",
                ))
            },
            |v6_data| {
                v6_data.recv_tclass = value;
                Ok(())
            },
        )
    }

    fn get_ipv6_receive_traffic_class(self) -> Result<bool, ErrnoError> {
        I::map_ip_in(
            &self.data.version_specific_data,
            |_v4_data| {
                Err(ErrnoError::new(
                    fposix::Errno::Eopnotsupp,
                    "cannot get_ipv6_receive_traffic_class on IPv4 socket",
                ))
            },
            |v6_data| Ok(v6_data.recv_tclass),
        )
    }

    fn ip_set_receive_type_of_service(self, value: bool) {
        self.data.ip_recv_tos = value;
    }

    fn ip_get_receive_type_of_service(self) -> bool {
        self.data.ip_recv_tos
    }

    fn set_mark(self, domain: fnet::MarkDomain, mark: fposix_socket::OptionalUint32) {
        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        T::set_mark(ctx, id, domain.into_core(), mark.into_core())
    }

    fn get_mark(self, domain: fnet::MarkDomain) -> fposix_socket::OptionalUint32 {
        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        T::get_mark(ctx, id, domain.into_core()).into_fidl()
    }

    fn get_cookie(self) -> SocketCookie {
        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        T::get_cookie(ctx, id)
    }

    fn set_send_buffer(self, send_buffer: u64) {
        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        T::set_send_buffer(ctx, id, usize::try_from(send_buffer).unwrap_or(usize::MAX));
    }

    fn get_send_buffer(self) -> u64 {
        let Self { ctx, data: BindingData { info: SocketControlInfo { id, .. }, .. } } = self;
        T::get_send_buffer(ctx, id).try_into().unwrap_or(u64::MAX)
    }
}

impl IntoErrno for ExpectedUnboundError {
    fn to_errno(&self) -> fposix::Errno {
        let ExpectedUnboundError = self;
        fposix::Errno::Einval
    }
}

impl IntoErrno for ExpectedConnError {
    fn to_errno(&self) -> fposix::Errno {
        let ExpectedConnError = self;
        fposix::Errno::Enotconn
    }
}

impl IntoErrno for NotSupportedError {
    fn to_errno(&self) -> fposix::Errno {
        fposix::Errno::Eopnotsupp
    }
}

impl<I: Ip, D> IntoFidl<LocalAddress<I, D, NonZeroU16>> for SocketInfo<I::Addr, D> {
    fn into_fidl(self) -> LocalAddress<I, D, NonZeroU16> {
        let (local_ip, local_identifier) = match self {
            Self::Unbound => (None, None),
            Self::Listener(ListenerInfo { local_ip, local_identifier }) => {
                (local_ip, Some(local_identifier))
            }
            Self::Connected(ConnInfo {
                local_ip,
                local_identifier,
                remote_ip: _,
                remote_identifier: _,
            }) => (Some(local_ip), Some(local_identifier)),
        };
        LocalAddress { address: local_ip, identifier: local_identifier }
    }
}

impl<I: Ip, D> TryIntoFidl<RemoteAddress<I, D, u16>> for SocketInfo<I::Addr, D> {
    type Error = ErrnoError;
    fn try_into_fidl(self) -> Result<RemoteAddress<I, D, u16>, Self::Error> {
        match self {
            Self::Unbound | Self::Listener(_) => Err(ErrnoError::new(
                fposix::Errno::Enotconn,
                "cannot get remote address of unconnected socket",
            )),
            Self::Connected(ConnInfo {
                local_ip: _,
                local_identifier: _,
                remote_ip,
                remote_identifier,
            }) => {
                if remote_identifier == 0 {
                    // Match Linux and report `ENOTCONN` for requests to
                    // 'get_peername` when the connection's remote port is 0 for
                    // both UDP and ICMP Echo sockets.
                    Err(ErrnoError::new(fposix::Errno::Enotconn, "remote port is 0"))
                } else {
                    Ok(RemoteAddress { address: remote_ip, identifier: remote_identifier })
                }
            }
        }
    }
}

impl<I: IpSockAddrExt, D, L: Into<u16>> TryIntoFidlWithContext<I::SocketAddress>
    for LocalAddress<I, D, L>
where
    D: TryIntoFidlWithContext<NonZeroU64, Error = DeviceNotFoundError>,
{
    type Error = ErrnoError;

    fn try_into_fidl_with_ctx<Ctx: crate::bindings::util::ConversionContext>(
        self,
        ctx: &Ctx,
    ) -> Result<I::SocketAddress, Self::Error> {
        let Self { address, identifier } = self;
        (address, identifier.map_or(0, Into::into))
            .try_into_fidl_with_ctx(ctx)
            .map_err(IntoErrno::into_errno_error)
    }
}

impl<I: IpSockAddrExt, D, R: Into<u16>> TryIntoFidlWithContext<I::SocketAddress>
    for RemoteAddress<I, D, R>
where
    D: TryIntoFidlWithContext<NonZeroU64, Error = DeviceNotFoundError>,
{
    type Error = ErrnoError;

    fn try_into_fidl_with_ctx<Ctx: crate::bindings::util::ConversionContext>(
        self,
        ctx: &Ctx,
    ) -> Result<I::SocketAddress, Self::Error> {
        let Self { address, identifier } = self;
        (Some(address), identifier.into())
            .try_into_fidl_with_ctx(ctx)
            .map_err(IntoErrno::into_errno_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_matches::assert_matches;
    use fidl::endpoints::{Proxy, ServerEnd};
    use fuchsia_async as fasync;
    use futures::StreamExt;
    use netstack3_core::NetworkSerializationContext;
    use packet::{NestablePacketBuilder as _, Serializer as _};
    use packet_formats::icmp::IcmpIpExt;

    use crate::bindings::integration_tests::{
        StackSetupBuilder, TestSetup, TestSetupBuilder, TestStack, test_ep_name,
    };
    use crate::bindings::socket::testutil::TestSockAddr;
    use crate::bindings::socket::{
        ZXSIO_SIGNAL_ERROR, ZXSIO_SIGNAL_INCOMING, ZXSIO_SIGNAL_OUTGOING,
    };
    use net_types::ip::{IpAddr, IpAddress};

    async fn prepare_test<A: TestSockAddr>(
        proto: fposix_socket::DatagramSocketProtocol,
    ) -> (TestSetup, fposix_socket::SynchronousDatagramSocketProxy, zx::EventPair) {
        // Setup the test with two endpoints, one in `A`'s domain, and the other
        // in `A::DifferentDomain` (e.g. IPv4 and IPv6).
        let mut t = TestSetupBuilder::new()
            .add_endpoint()
            .add_endpoint()
            .add_stack(
                StackSetupBuilder::new()
                    .add_named_endpoint(test_ep_name(1), Some(A::config_addr_subnet()))
                    .add_named_endpoint(
                        test_ep_name(2),
                        Some(A::DifferentDomain::config_addr_subnet()),
                    ),
            )
            .build()
            .await;

        let (proxy, event) = get_socket_and_event::<A>(t.get_mut(0), proto).await;
        (t, proxy, event)
    }

    async fn get_socket<A: TestSockAddr>(
        test_stack: &mut TestStack,
        proto: fposix_socket::DatagramSocketProtocol,
    ) -> fposix_socket::SynchronousDatagramSocketProxy {
        let socket_provider = test_stack.connect_socket_provider();
        let response = socket_provider
            .datagram_socket(A::DOMAIN, proto)
            .await
            .unwrap()
            .expect("Socket succeeds");
        match response {
            fposix_socket::ProviderDatagramSocketResponse::SynchronousDatagramSocket(sock) => {
                fposix_socket::SynchronousDatagramSocketProxy::new(fasync::Channel::from_channel(
                    sock.into_channel(),
                ))
            }
            // TODO(https://fxrev.dev/99905): Implement Fast UDP sockets in Netstack3.
            fposix_socket::ProviderDatagramSocketResponse::DatagramSocket(sock) => {
                let _: fidl::endpoints::ClientEnd<fposix_socket::DatagramSocketMarker> = sock;
                panic!("expected SynchronousDatagramSocket, found DatagramSocket")
            }
        }
    }

    async fn get_socket_and_event<A: TestSockAddr>(
        test_stack: &mut TestStack,
        proto: fposix_socket::DatagramSocketProtocol,
    ) -> (fposix_socket::SynchronousDatagramSocketProxy, zx::EventPair) {
        let ctlr = get_socket::<A>(test_stack, proto).await;
        let fposix_socket::SynchronousDatagramSocketDescribeResponse { event, .. } =
            ctlr.describe().await.expect("describe succeeds");
        (ctlr, event.expect("Socket describe contains event"))
    }

    macro_rules! declare_tests {
        ($test_fn:ident, icmp $(#[$icmp_attributes:meta])*) => {
            mod $test_fn {
                use super::*;

                #[fuchsia::test(logging = false)]
                async fn udp_v4() {
                    $test_fn::<fnet::Ipv4SocketAddress, Udp>(
                        fposix_socket::DatagramSocketProtocol::Udp,
                    )
                    .await
                }

                #[fuchsia::test(logging = false)]
                async fn udp_v6() {
                    $test_fn::<fnet::Ipv6SocketAddress, Udp>(
                        fposix_socket::DatagramSocketProtocol::Udp,
                    )
                    .await
                }

                $(#[$icmp_attributes])*
                #[fuchsia::test(logging = false)]
                async fn icmp_v4() {
                    $test_fn::<fnet::Ipv4SocketAddress, IcmpEcho>(
                        fposix_socket::DatagramSocketProtocol::IcmpEcho,
                    )
                    .await
                }

                $(#[$icmp_attributes])*
                #[fuchsia::test(logging = false)]
                async fn icmp_v6() {
                    $test_fn::<fnet::Ipv6SocketAddress, IcmpEcho>(
                        fposix_socket::DatagramSocketProtocol::IcmpEcho,
                    )
                    .await
                }
            }
        };
        ($test_fn:ident) => {
            declare_tests!($test_fn, icmp);
        };
    }

    #[fixture::teardown(TestSetup::shutdown)]
    async fn connect_failure<A: TestSockAddr, T>(proto: fposix_socket::DatagramSocketProtocol) {
        let (t, proxy, _event) = prepare_test::<A>(proto).await;

        // Pass a socket address of the wrong domain, which should fail for IPv4
        // but pass for IPv6 on UDP (as it's implicitly converted to an
        // IPv4-mapped-IPv6 address).
        let res = proxy
            .connect(&A::DifferentDomain::create(A::DifferentDomain::REMOTE_ADDR, 1010))
            .await
            .unwrap();
        match (proto, <<<A as SockAddr>::AddrType as IpAddress>::Version as Ip>::VERSION) {
            (fposix_socket::DatagramSocketProtocol::Udp, IpVersion::V6) => {
                assert_eq!(res, Ok(()));
                // NB: The socket is connected in the IPv4 stack; disconnect it
                // so that we can connect it in the IPv6 stack below.
                proxy.disconnect().await.unwrap().expect("disconnect should succeed");
            }
            (_, _) => assert_eq!(res, Err(fposix::Errno::Eafnosupport)),
        }

        // Pass a zero port. UDP and ICMP both allow it.
        let res = proxy.connect(&A::create(A::LOCAL_ADDR, 0)).await.unwrap();
        assert_eq!(res, Ok(()));

        // Pass an unreachable address (tests error forwarding from `create_connection`).
        let res = proxy
            .connect(&A::create(A::UNREACHABLE_ADDR, 1010))
            .await
            .unwrap()
            .expect_err("connect fails");
        assert_eq!(res, fposix::Errno::Enetunreach);

        t
    }

    declare_tests!(connect_failure);

    #[fixture::teardown(TestSetup::shutdown)]
    async fn connect<A: TestSockAddr, T>(proto: fposix_socket::DatagramSocketProtocol) {
        let (t, proxy, _event) = prepare_test::<A>(proto).await;
        proxy.connect(&A::create(A::REMOTE_ADDR, 200)).await.unwrap().expect("connect succeeds");

        // Can connect again to a different remote should succeed.
        proxy.connect(&A::create(A::REMOTE_ADDR_2, 200)).await.unwrap().expect("connect suceeds");

        t
    }

    declare_tests!(connect);

    #[fixture::teardown(TestSetup::shutdown)]
    async fn connect_loopback<A: TestSockAddr, T>(proto: fposix_socket::DatagramSocketProtocol) {
        let (t, proxy, _event) = prepare_test::<A>(proto).await;
        proxy
            .connect(&A::create(
                <<A::AddrType as IpAddress>::Version as Ip>::LOOPBACK_ADDRESS.get(),
                200,
            ))
            .await
            .unwrap()
            .expect("connect succeeds");

        t
    }

    declare_tests!(connect_loopback);

    #[fixture::teardown(TestSetup::shutdown)]
    async fn connect_any<A: TestSockAddr, T>(proto: fposix_socket::DatagramSocketProtocol) {
        // Pass an unspecified remote address. This should be treated as the
        // loopback address.
        let (t, proxy, _event) = prepare_test::<A>(proto).await;

        const PORT: u16 = 1010;
        proxy
            .connect(&A::create(<A::AddrType as IpAddress>::Version::UNSPECIFIED_ADDRESS, PORT))
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            proxy.get_peer_name().await.unwrap().unwrap(),
            A::create(<A::AddrType as IpAddress>::Version::LOOPBACK_ADDRESS.get(), PORT)
        );

        t
    }

    declare_tests!(connect_any);

    #[fixture::teardown(TestSetup::shutdown)]
    async fn bind<A: TestSockAddr, T>(proto: fposix_socket::DatagramSocketProtocol) {
        let (mut t, socket, _event) = prepare_test::<A>(proto).await;
        let stack = t.get_mut(0);
        // Can bind to local address.
        socket.bind(&A::create(A::LOCAL_ADDR, 200)).await.unwrap().expect("bind succeeds");

        // Can't bind again (to another port).
        let res =
            socket.bind(&A::create(A::LOCAL_ADDR, 201)).await.unwrap().expect_err("bind fails");
        assert_eq!(res, fposix::Errno::Einval);

        // Can bind another socket to a different port.
        let socket = get_socket::<A>(stack, proto).await;
        socket.bind(&A::create(A::LOCAL_ADDR, 201)).await.unwrap().expect("bind succeeds");

        // Can bind to unspecified address in a different port.
        let socket = get_socket::<A>(stack, proto).await;
        socket
            .bind(&A::create(<A::AddrType as IpAddress>::Version::UNSPECIFIED_ADDRESS, 202))
            .await
            .unwrap()
            .expect("bind succeeds");

        t
    }

    declare_tests!(bind);

    #[fixture::teardown(TestSetup::shutdown)]
    async fn bind_then_connect<A: TestSockAddr, T>(proto: fposix_socket::DatagramSocketProtocol) {
        let (t, socket, _event) = prepare_test::<A>(proto).await;
        // Can bind to local address.
        socket.bind(&A::create(A::LOCAL_ADDR, 200)).await.unwrap().expect("bind suceeds");

        socket.connect(&A::create(A::REMOTE_ADDR, 1010)).await.unwrap().expect("connect succeeds");

        t
    }

    declare_tests!(bind_then_connect);

    #[fixture::teardown(TestSetup::shutdown)]
    async fn connect_then_disconnect<A: TestSockAddr, T>(
        proto: fposix_socket::DatagramSocketProtocol,
    ) {
        let (t, socket, _event) = prepare_test::<A>(proto).await;

        let remote_addr = A::create(A::REMOTE_ADDR, 1010);
        socket.connect(&remote_addr).await.unwrap().expect("connect succeeds");

        assert_eq!(
            socket.get_peer_name().await.unwrap().expect("get_peer_name should suceed"),
            remote_addr
        );
        socket.disconnect().await.unwrap().expect("disconnect succeeds");

        assert_eq!(
            socket.get_peer_name().await.unwrap().expect_err("alice getpeername fails"),
            fposix::Errno::Enotconn
        );

        t
    }

    /// ICMP echo sockets require the buffer to be a valid ICMP echo request,
    /// this function performs transformations allowing the majority of the
    /// sending logic to be common with UDP.
    fn prepare_buffer_to_send<A: TestSockAddr>(
        proto: fposix_socket::DatagramSocketProtocol,
        buf: Vec<u8>,
    ) -> Vec<u8>
    where
        <A::AddrType as IpAddress>::Version: IcmpIpExt,
    {
        match proto {
            fposix_socket::DatagramSocketProtocol::Udp => buf,
            fposix_socket::DatagramSocketProtocol::IcmpEcho => {
                packet_formats::icmp::IcmpPacketBuilder::<
                    <A::AddrType as IpAddress>::Version,
                    _,
                >::new(
                    <<A::AddrType as IpAddress>::Version as Ip>::LOOPBACK_ADDRESS.get(),
                    <<A::AddrType as IpAddress>::Version as Ip>::LOOPBACK_ADDRESS.get(),
                    packet_formats::icmp::IcmpZeroCode,
                    packet_formats::icmp::IcmpEchoRequest::new(0, 1),
                )
                .wrap_body(Buf::new(buf, ..))
                .serialize_vec_outer(&mut NetworkSerializationContext::default())
                .unwrap()
                .into_inner()
                .into_inner()
            }
        }
    }

    /// ICMP echo sockets receive a buffer that is an ICMP echo reply, this
    /// function performs transformations allowing the majority of the receiving
    /// logic to be common with UDP.
    fn expected_buffer_to_receive<A: TestSockAddr>(
        proto: fposix_socket::DatagramSocketProtocol,
        buf: Vec<u8>,
        id: u16,
        src_ip: A::AddrType,
        dst_ip: A::AddrType,
    ) -> Vec<u8>
    where
        <A::AddrType as IpAddress>::Version: IcmpIpExt,
    {
        match proto {
            fposix_socket::DatagramSocketProtocol::Udp => buf,
            fposix_socket::DatagramSocketProtocol::IcmpEcho => {
                packet_formats::icmp::IcmpPacketBuilder::<
                    <A::AddrType as IpAddress>::Version,
                    _,
                >::new(
                    src_ip,
                    dst_ip,
                    packet_formats::icmp::IcmpZeroCode,
                    packet_formats::icmp::IcmpEchoReply::new(id, 1),
                )
                .wrap_body(Buf::new(buf, ..))
                .serialize_vec_outer(&mut NetworkSerializationContext::default())
                .unwrap()
                .into_inner()
                .into_inner()
            }
        }
    }

    declare_tests!(connect_then_disconnect);

    /// Tests a simple UDP setup with a client and a server, where the client
    /// can send data to the server and the server receives it.
    // TODO(https://fxbug.dev/42124055): this test is incorrect for ICMP sockets. At the time of this
    // writing it crashes before reaching the wrong parts, but we will need to specialize the body
    // of this test for ICMP before calling the feature complete.
    #[fixture::teardown(TestSetup::shutdown)]
    async fn hello<A: TestSockAddr, T>(proto: fposix_socket::DatagramSocketProtocol)
    where
        <A::AddrType as IpAddress>::Version: IcmpIpExt,
    {
        // We create two stacks, Alice (server listening on LOCAL_ADDR:200), and
        // Bob (client, bound on REMOTE_ADDR:300). After setup, Bob connects to
        // Alice and sends a datagram. Finally, we verify that Alice receives
        // the datagram.
        let mut t = TestSetupBuilder::new()
            .add_endpoint()
            .add_endpoint()
            .add_stack(
                StackSetupBuilder::new()
                    .add_named_endpoint(test_ep_name(1), Some(A::config_addr_subnet())),
            )
            .add_stack(
                StackSetupBuilder::new()
                    .add_named_endpoint(test_ep_name(2), Some(A::config_addr_subnet_remote())),
            )
            .build()
            .await;

        let alice = t.get_mut(0);
        let (alice_socket, alice_events) = get_socket_and_event::<A>(alice, proto).await;

        // Verify that Alice has no local or peer addresses bound
        assert_eq!(
            alice_socket.get_sock_name().await.unwrap().unwrap(),
            A::new(None, 0).into_sock_addr(),
        );
        assert_eq!(
            alice_socket.get_peer_name().await.unwrap().expect_err("alice getpeername fails"),
            fposix::Errno::Enotconn
        );

        // Setup Alice as a server, bound to LOCAL_ADDR:200
        println!("Configuring alice...");
        alice_socket
            .bind(&A::create(A::LOCAL_ADDR, 200))
            .await
            .unwrap()
            .expect("alice bind suceeds");

        // Verify that Alice is listening on the local socket, but still has no
        // peer socket
        assert_eq!(
            alice_socket.get_sock_name().await.unwrap().expect("alice getsockname succeeds"),
            A::create(A::LOCAL_ADDR, 200)
        );
        assert_eq!(
            alice_socket.get_peer_name().await.unwrap().expect_err("alice getpeername should fail"),
            fposix::Errno::Enotconn
        );

        // check that alice has no data to read, and it'd block waiting for
        // events:
        assert_eq!(
            alice_socket
                .recv_msg(false, 2048, false, fposix_socket::RecvMsgFlags::empty())
                .await
                .unwrap()
                .expect_err("Reading from alice should fail"),
            fposix::Errno::Eagain
        );
        assert_eq!(
            alice_events
                .wait_one(ZXSIO_SIGNAL_INCOMING, zx::MonotonicInstant::from_nanos(0))
                .expect_err("Alice incoming event should not be signaled"),
            zx::Status::TIMED_OUT
        );

        // Setup Bob as a client, bound to REMOTE_ADDR:300
        println!("Configuring bob...");
        let bob = t.get_mut(1);
        let (bob_socket, bob_events) = get_socket_and_event::<A>(bob, proto).await;
        bob_socket.bind(&A::create(A::REMOTE_ADDR, 300)).await.unwrap().expect("bob bind suceeds");

        // Verify that Bob is listening on the local socket, but has no peer
        // socket
        assert_eq!(
            bob_socket.get_sock_name().await.unwrap().expect("bob getsockname suceeds"),
            A::create(A::REMOTE_ADDR, 300)
        );
        assert_eq!(
            bob_socket
                .get_peer_name()
                .await
                .unwrap()
                .expect_err("get peer name should fail before connected"),
            fposix::Errno::Enotconn
        );

        // Connect Bob to Alice on LOCAL_ADDR:200
        println!("Connecting bob to alice...");
        bob_socket
            .connect(&A::create(A::LOCAL_ADDR, 200))
            .await
            .unwrap()
            .expect("Connect succeeds");

        // Verify that Bob still has the right local socket name.
        assert_eq!(
            bob_socket.get_sock_name().await.unwrap().expect("bob getsockname suceeds"),
            A::create(A::REMOTE_ADDR, 300)
        );
        // Verify that Bob has the peer socket set correctly
        assert_eq!(
            bob_socket.get_peer_name().await.unwrap().expect("bob getpeername suceeds"),
            A::create(A::LOCAL_ADDR, 200)
        );

        // We don't care which signals are on, only that SIGNAL_OUTGOING is, we
        // can ignore the return value.
        let _signals = bob_events
            .wait_one(ZXSIO_SIGNAL_OUTGOING, zx::MonotonicInstant::from_nanos(0))
            .expect("Bob outgoing event should be signaled");

        // Send datagram from Bob's socket.
        println!("Writing datagram to bob");
        let body = "Hello".as_bytes();
        let to_send = prepare_buffer_to_send::<A>(proto, body.to_vec());
        assert_eq!(
            bob_socket
                .send_msg(
                    None,
                    &to_send,
                    &fposix_socket::DatagramSocketSendControlData::default(),
                    fposix_socket::SendMsgFlags::empty()
                )
                .await
                .unwrap()
                .expect("sendmsg suceeds"),
            to_send.len() as i64
        );

        let (events, socket, port, expected_src_ip) = match proto {
            fposix_socket::DatagramSocketProtocol::Udp => {
                (&alice_events, &alice_socket, 300, A::REMOTE_ADDR)
            }
            fposix_socket::DatagramSocketProtocol::IcmpEcho => {
                (&bob_events, &bob_socket, 0, A::LOCAL_ADDR)
            }
        };

        println!("Waiting for signals");
        assert_eq!(
            fasync::OnSignals::new(events, ZXSIO_SIGNAL_INCOMING).await,
            Ok(ZXSIO_SIGNAL_INCOMING | ZXSIO_SIGNAL_OUTGOING)
        );

        let to_recv = expected_buffer_to_receive::<A>(
            proto,
            body.to_vec(),
            300,
            A::LOCAL_ADDR,
            A::REMOTE_ADDR,
        );
        let (from, data, _, truncated) = socket
            .recv_msg(true, 2048, false, fposix_socket::RecvMsgFlags::empty())
            .await
            .unwrap()
            .expect("recvmsg suceeeds");
        let source = A::from_sock_addr(*from.expect("socket address returned"))
            .expect("bad socket address return");
        assert_eq!(source.addr(), expected_src_ip);
        assert_eq!(source.port(), port);
        assert_eq!(truncated, 0);
        assert_eq!(&data[..], to_recv);
        t
    }

    declare_tests!(hello);

    #[fixture::teardown(TestSetup::shutdown)]
    #[test_case::test_matrix(
        [
            fposix_socket::Domain::Ipv4,
            fposix_socket::Domain::Ipv6,
        ],
        [
            fposix_socket::DatagramSocketProtocol::Udp,
            // ICMP Echo sockets don't support SO_ERROR.
        ]
    )]
    #[fuchsia::test(logging = false)]
    async fn so_error(domain: fposix_socket::Domain, proto: fposix_socket::DatagramSocketProtocol) {
        let (local_subnet, connect_addr) = match domain {
            fposix_socket::Domain::Ipv4 => (
                fnet::Ipv4SocketAddress::config_addr_subnet(),
                fnet::Ipv4SocketAddress::create(fnet::Ipv4SocketAddress::LOCAL_ADDR, 1234),
            ),
            fposix_socket::Domain::Ipv6 => (
                fnet::Ipv6SocketAddress::config_addr_subnet(),
                fnet::Ipv6SocketAddress::create(fnet::Ipv6SocketAddress::LOCAL_ADDR, 1234),
            ),
        };

        let mut t = TestSetupBuilder::new()
            .add_endpoint()
            .add_stack(
                StackSetupBuilder::new().add_named_endpoint(test_ep_name(1), Some(local_subnet)),
            )
            .build()
            .await;

        let bob = t.get_mut(0);
        let (bob_socket, bob_events) = match domain {
            fposix_socket::Domain::Ipv4 => {
                get_socket_and_event::<fnet::Ipv4SocketAddress>(bob, proto).await
            }
            fposix_socket::Domain::Ipv6 => {
                get_socket_and_event::<fnet::Ipv6SocketAddress>(bob, proto).await
            }
        };

        bob_socket.connect(&connect_addr).await.unwrap().expect("Connect succeeds");

        let body = "Hello".as_bytes();
        let to_send = match domain {
            fposix_socket::Domain::Ipv4 => {
                prepare_buffer_to_send::<fnet::Ipv4SocketAddress>(proto, body.to_vec())
            }
            fposix_socket::Domain::Ipv6 => {
                prepare_buffer_to_send::<fnet::Ipv6SocketAddress>(proto, body.to_vec())
            }
        };
        assert_eq!(
            bob_socket
                .send_msg(
                    None,
                    &to_send,
                    &fposix_socket::DatagramSocketSendControlData::default(),
                    fposix_socket::SendMsgFlags::empty()
                )
                .await
                .unwrap()
                .expect("sendmsg succeeds"),
            to_send.len() as i64
        );

        let signals = fasync::OnSignals::new(&bob_events, ZXSIO_SIGNAL_ERROR).await.unwrap();
        assert!(signals.contains(ZXSIO_SIGNAL_ERROR));

        let res = bob_socket.get_error().await.unwrap();
        assert_eq!(res, Err(fposix::Errno::Econnrefused));

        let res = bob_socket.get_error().await.unwrap();
        assert_eq!(res, Ok(()));

        t
    }

    #[fixture::teardown(TestSetup::shutdown)]
    #[test_case::test_matrix(
        [
            fposix_socket::Domain::Ipv4,
            fposix_socket::Domain::Ipv6,
        ],
        [
            fposix_socket::DatagramSocketProtocol::Udp,
            fposix_socket::DatagramSocketProtocol::IcmpEcho,
        ]
    )]
    #[fuchsia::test(logging = false)]
    async fn socket_describe(
        domain: fposix_socket::Domain,
        proto: fposix_socket::DatagramSocketProtocol,
    ) {
        let t = TestSetupBuilder::new().add_endpoint().add_empty_stack().build().await;
        let test_stack = t.get(0);
        let socket_provider = test_stack.connect_socket_provider();
        let response = socket_provider
            .datagram_socket(domain, proto)
            .await
            .unwrap()
            .expect("Socket call succeeds");
        let socket = match response {
            fposix_socket::ProviderDatagramSocketResponse::SynchronousDatagramSocket(sock) => sock,
            // TODO(https://fxrev.dev/99905): Implement Fast UDP sockets in Netstack3.
            fposix_socket::ProviderDatagramSocketResponse::DatagramSocket(sock) => {
                let _: fidl::endpoints::ClientEnd<fposix_socket::DatagramSocketMarker> = sock;
                panic!("expected SynchronousDatagramSocket, found DatagramSocket")
            }
        };
        let fposix_socket::SynchronousDatagramSocketDescribeResponse { event, .. } =
            socket.into_proxy().describe().await.expect("Describe call succeeds");
        let _: zx::EventPair = event.expect("Describe call returns event");
        t
    }

    #[fixture::teardown(TestSetup::shutdown)]
    #[test_case::test_matrix(
        [
            fposix_socket::Domain::Ipv4,
            fposix_socket::Domain::Ipv6,
        ],
        [
            fposix_socket::DatagramSocketProtocol::Udp,
            fposix_socket::DatagramSocketProtocol::IcmpEcho,
        ]
    )]
    #[fuchsia::test(logging = false)]
    async fn socket_get_info(
        domain: fposix_socket::Domain,
        proto: fposix_socket::DatagramSocketProtocol,
    ) {
        let t = TestSetupBuilder::new().add_endpoint().add_empty_stack().build().await;
        let test_stack = t.get(0);
        let socket_provider = test_stack.connect_socket_provider();
        let response = socket_provider
            .datagram_socket(domain, proto)
            .await
            .unwrap()
            .expect("Socket call succeeds");
        let socket = match response {
            fposix_socket::ProviderDatagramSocketResponse::SynchronousDatagramSocket(sock) => sock,
            // TODO(https://fxrev.dev/99905): Implement Fast UDP sockets in Netstack3.
            fposix_socket::ProviderDatagramSocketResponse::DatagramSocket(sock) => {
                let _: fidl::endpoints::ClientEnd<fposix_socket::DatagramSocketMarker> = sock;
                panic!("expected SynchronousDatagramSocket, found DatagramSocket")
            }
        };
        let info = socket.into_proxy().get_info().await.expect("get_info call succeeds");
        assert_eq!(info, Ok((domain, proto)));

        t
    }

    fn socket_clone(
        socket: &fposix_socket::SynchronousDatagramSocketProxy,
    ) -> fposix_socket::SynchronousDatagramSocketProxy {
        let (client, server) =
            fidl::endpoints::create_proxy::<fposix_socket::SynchronousDatagramSocketMarker>();
        let server = ServerEnd::new(server.into_channel());
        socket.clone(server).expect("socket clone");
        client
    }

    type IpFromSockAddr<A> = <<A as SockAddr>::AddrType as IpAddress>::Version;

    #[fixture::teardown(TestSetup::shutdown)]
    async fn clone<A: TestSockAddr, T>(proto: fposix_socket::DatagramSocketProtocol)
    where
        <A::AddrType as IpAddress>::Version: IcmpIpExt,
        T: Transport<Ipv4>,
        T: Transport<Ipv6>,
        T: Transport<<A::AddrType as IpAddress>::Version>,
    {
        let mut t = TestSetupBuilder::new()
            .add_endpoint()
            .add_endpoint()
            .add_stack(
                StackSetupBuilder::new()
                    .add_named_endpoint(test_ep_name(1), Some(A::config_addr_subnet())),
            )
            .add_stack(
                StackSetupBuilder::new()
                    .add_named_endpoint(test_ep_name(2), Some(A::config_addr_subnet_remote())),
            )
            .build()
            .await;

        let (alice_socket, alice_events) = get_socket_and_event::<A>(t.get_mut(0), proto).await;
        let alice_cloned = socket_clone(&alice_socket);
        let fposix_socket::SynchronousDatagramSocketDescribeResponse { event: alice_event, .. } =
            alice_cloned.describe().await.expect("Describe call succeeds");
        let _: zx::EventPair = alice_event.expect("Describe call returns event");

        alice_socket
            .bind(&A::create(A::LOCAL_ADDR, 200))
            .await
            .unwrap()
            .expect("failed to bind for alice");
        // We should be able to read that back from the cloned socket.
        assert_eq!(
            alice_cloned.get_sock_name().await.unwrap().expect("failed to getsockname for alice"),
            A::create(A::LOCAL_ADDR, 200)
        );

        let (bob_socket, bob_events) = get_socket_and_event::<A>(t.get_mut(1), proto).await;
        let bob_cloned = socket_clone(&bob_socket);
        bob_cloned
            .bind(&A::create(A::REMOTE_ADDR, 200))
            .await
            .unwrap()
            .expect("failed to bind for bob");
        // We should be able to read that back from the original socket.
        assert_eq!(
            bob_socket.get_sock_name().await.unwrap().expect("failed to getsockname for bob"),
            A::create(A::REMOTE_ADDR, 200)
        );

        let body = "Hello".as_bytes();
        let to_send = prepare_buffer_to_send::<A>(proto, body.to_vec());
        assert_eq!(
            alice_socket
                .send_msg(
                    Some(&A::create(A::REMOTE_ADDR, 200)),
                    &to_send,
                    &fposix_socket::DatagramSocketSendControlData::default(),
                    fposix_socket::SendMsgFlags::empty()
                )
                .await
                .unwrap()
                .expect("failed to send_msg"),
            to_send.len() as i64
        );

        let (cloned_events, cloned_socket, expected_from) = match proto {
            fposix_socket::DatagramSocketProtocol::Udp => {
                (&bob_events, &bob_cloned, A::create(A::LOCAL_ADDR, 200))
            }
            fposix_socket::DatagramSocketProtocol::IcmpEcho => {
                (&alice_events, &alice_cloned, A::create(A::REMOTE_ADDR, 0))
            }
        };

        assert_eq!(
            fasync::OnSignals::new(cloned_events, ZXSIO_SIGNAL_INCOMING).await,
            Ok(ZXSIO_SIGNAL_INCOMING | ZXSIO_SIGNAL_OUTGOING)
        );

        // Receive from the cloned socket.
        let (from, data, _, truncated) = cloned_socket
            .recv_msg(true, 2048, false, fposix_socket::RecvMsgFlags::empty())
            .await
            .unwrap()
            .expect("failed to recv_msg");
        let to_recv = expected_buffer_to_receive::<A>(
            proto,
            body.to_vec(),
            200,
            A::REMOTE_ADDR,
            A::LOCAL_ADDR,
        );
        assert_eq!(&data[..], to_recv);
        assert_eq!(truncated, 0);
        assert_eq!(from.map(|a| *a), Some(expected_from));
        // The data have already been received on the cloned socket
        assert_eq!(
            cloned_socket
                .recv_msg(false, 2048, false, fposix_socket::RecvMsgFlags::empty())
                .await
                .unwrap()
                .expect_err("Reading from bob should fail"),
            fposix::Errno::Eagain
        );

        match proto {
            fposix_socket::DatagramSocketProtocol::Udp => {
                // Close the socket should not invalidate the cloned socket.
                bob_socket
                    .close()
                    .await
                    .expect("FIDL error")
                    .map_err(zx::Status::err_from_raw)
                    .expect("close failed");

                assert_eq!(
                    bob_cloned
                        .send_msg(
                            Some(&A::create(A::LOCAL_ADDR, 200)),
                            &body,
                            &fposix_socket::DatagramSocketSendControlData::default(),
                            fposix_socket::SendMsgFlags::empty()
                        )
                        .await
                        .unwrap()
                        .expect("failed to send_msg"),
                    body.len() as i64
                );

                alice_cloned
                    .close()
                    .await
                    .expect("FIDL error")
                    .map_err(zx::Status::err_from_raw)
                    .expect("close failed");
                assert_eq!(
                    fasync::OnSignals::new(&alice_events, ZXSIO_SIGNAL_INCOMING).await,
                    Ok(ZXSIO_SIGNAL_INCOMING | ZXSIO_SIGNAL_OUTGOING)
                );

                let (from, data, _, truncated) = alice_socket
                    .recv_msg(true, 2048, false, fposix_socket::RecvMsgFlags::empty())
                    .await
                    .unwrap()
                    .expect("failed to recv_msg");
                assert_eq!(&data[..], body);
                assert_eq!(truncated, 0);
                assert_eq!(from.map(|a| *a), Some(A::create(A::REMOTE_ADDR, 200)));

                // Make sure the sockets are still in the stack.
                for i in 0..2 {
                    t.get_mut(i).with_ctx(|ctx| {
                        assert_matches!(
                            &<T as Transport<IpFromSockAddr<A>>>::collect_all_sockets(ctx)[..],
                            [_]
                        );
                    });
                }

                alice_socket
                    .close()
                    .await
                    .expect("FIDL error")
                    .map_err(zx::Status::err_from_raw)
                    .expect("close failed");
                bob_cloned
                    .close()
                    .await
                    .expect("FIDL error")
                    .map_err(zx::Status::err_from_raw)
                    .expect("close failed");

                // But the sockets should have gone here.
                for i in 0..2 {
                    t.get_mut(i).with_ctx(|ctx| {
                        assert_matches!(
                            &<T as Transport<IpFromSockAddr<A>>>::collect_all_sockets(ctx)[..],
                            []
                        );
                    });
                }
            }
            fposix_socket::DatagramSocketProtocol::IcmpEcho => {
                // For ICMP sockets, the sending and receiving socket are the
                // the same socket, so the above test for UDP will not apply -
                // closing alice_socket and bob_cloned will keep both sockets
                // alive, but closing bob_socket and bob_cloned will actually
                // close bob. There is no interesting behavior to test for a
                // closed socket.
            }
        }

        t
    }

    declare_tests!(clone);

    #[fixture::teardown(TestSetup::shutdown)]
    async fn close_twice<A: TestSockAddr, T>(proto: fposix_socket::DatagramSocketProtocol)
    where
        T: Transport<Ipv4>,
        T: Transport<Ipv6>,
        T: Transport<<A::AddrType as IpAddress>::Version>,
    {
        // Make sure we cannot close twice from the same channel so that we
        // maintain the correct refcount.
        let mut t = TestSetupBuilder::new().add_endpoint().add_empty_stack().build().await;
        let test_stack = t.get_mut(0);
        let socket = get_socket::<A>(test_stack, proto).await;
        let cloned = socket_clone(&socket);
        socket
            .close()
            .await
            .expect("FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("close failed");
        let _: fidl::Error = socket
            .close()
            .await
            .expect_err("should not be able to close the socket twice on the same channel");
        assert!(socket.into_channel().unwrap().is_closed());
        // Since we still hold the cloned socket, the binding_data shouldn't be
        // empty
        test_stack.with_ctx(|ctx| {
            assert_matches!(
                &<T as Transport<IpFromSockAddr<A>>>::collect_all_sockets(ctx)[..],
                [_]
            );
        });
        cloned
            .close()
            .await
            .expect("FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("close failed");
        // Now it should become empty
        test_stack.with_ctx(|ctx| {
            assert_matches!(&<T as Transport<IpFromSockAddr<A>>>::collect_all_sockets(ctx)[..], []);
        });

        t
    }

    declare_tests!(close_twice);

    #[fixture::teardown(TestSetup::shutdown)]
    async fn implicit_close<A: TestSockAddr, T>(proto: fposix_socket::DatagramSocketProtocol)
    where
        T: Transport<Ipv4>,
        T: Transport<Ipv6>,
        T: Transport<<A::AddrType as IpAddress>::Version>,
    {
        let mut t = TestSetupBuilder::new().add_endpoint().add_empty_stack().build().await;
        let test_stack = t.get_mut(0);
        let cloned = {
            let socket = get_socket::<A>(test_stack, proto).await;
            socket_clone(&socket)
            // socket goes out of scope indicating an implicit close.
        };
        // Using an explicit close here.
        cloned
            .close()
            .await
            .expect("FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("close failed");
        // No socket should be there now.
        test_stack.with_ctx(|ctx| {
            assert_matches!(&<T as Transport<IpFromSockAddr<A>>>::collect_all_sockets(ctx)[..], []);
        });

        t
    }

    declare_tests!(implicit_close);

    #[fixture::teardown(TestSetup::shutdown)]
    async fn invalid_clone_args<A: TestSockAddr, T>(proto: fposix_socket::DatagramSocketProtocol)
    where
        T: Transport<Ipv4>,
        T: Transport<Ipv6>,
        T: Transport<<A::AddrType as IpAddress>::Version>,
    {
        let mut t = TestSetupBuilder::new().add_endpoint().add_empty_stack().build().await;
        let test_stack = t.get_mut(0);
        let socket = get_socket::<A>(test_stack, proto).await;
        socket
            .close()
            .await
            .expect("FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("close failed");

        // make sure we don't leak anything.
        test_stack.with_ctx(|ctx| {
            assert_matches!(&<T as Transport<IpFromSockAddr<A>>>::collect_all_sockets(ctx)[..], []);
        });

        t
    }

    declare_tests!(invalid_clone_args);

    #[fixture::teardown(TestSetup::shutdown)]
    async fn shutdown<A: TestSockAddr, T>(proto: fposix_socket::DatagramSocketProtocol) {
        let mut t = TestSetupBuilder::new()
            .add_endpoint()
            .add_stack(
                StackSetupBuilder::new()
                    .add_named_endpoint(test_ep_name(1), Some(A::config_addr_subnet())),
            )
            .build()
            .await;

        let (socket, events) = get_socket_and_event::<A>(t.get_mut(0), proto).await;
        let local = A::create(A::LOCAL_ADDR, 200);
        let remote = A::create(A::REMOTE_ADDR, 300);
        assert_eq!(
            socket
                .shutdown(fposix_socket::ShutdownMode::WRITE)
                .await
                .unwrap()
                .expect_err("should not shutdown an unconnected socket"),
            fposix::Errno::Enotconn,
        );
        socket.bind(&local).await.unwrap().expect("failed to bind");
        assert_eq!(
            socket
                .shutdown(fposix_socket::ShutdownMode::WRITE)
                .await
                .unwrap()
                .expect_err("should not shutdown an unconnected socket"),
            fposix::Errno::Enotconn,
        );
        socket.connect(&remote).await.unwrap().expect("failed to connect");
        assert_eq!(
            socket
                .shutdown(fposix_socket::ShutdownMode::empty())
                .await
                .unwrap()
                .expect_err("invalid args"),
            fposix::Errno::Einval
        );

        // Cannot send
        let body = "Hello".as_bytes();
        socket
            .shutdown(fposix_socket::ShutdownMode::WRITE)
            .await
            .unwrap()
            .expect("failed to shutdown");
        assert_eq!(
            socket
                .send_msg(
                    None,
                    &body,
                    &fposix_socket::DatagramSocketSendControlData::default(),
                    fposix_socket::SendMsgFlags::empty()
                )
                .await
                .unwrap()
                .expect_err("writing to an already-shutdown socket should fail"),
            fposix::Errno::Epipe,
        );
        let invalid_addr = A::create(A::REMOTE_ADDR, 0);
        let errno = match proto {
            fposix_socket::DatagramSocketProtocol::Udp => fposix::Errno::Einval,
            fposix_socket::DatagramSocketProtocol::IcmpEcho => fposix::Errno::Epipe,
        };
        assert_eq!(
            socket
                .send_msg(
                    Some(&invalid_addr),
                    &body,
                    &fposix_socket::DatagramSocketSendControlData::default(),
                    fposix_socket::SendMsgFlags::empty()
                )
                .await
                .unwrap()
                .expect_err("writing to port 0 should fail"),
            errno
        );

        let left = async {
            assert_eq!(
                fasync::OnSignals::new(&events, ZXSIO_SIGNAL_INCOMING).await,
                Ok(ZXSIO_SIGNAL_INCOMING | ZXSIO_SIGNAL_OUTGOING)
            );
        };

        let right = async {
            socket
                .shutdown(fposix_socket::ShutdownMode::READ)
                .await
                .unwrap()
                .expect("failed to shutdown");
            let (_, data, _, _) = socket
                .recv_msg(false, 2048, false, fposix_socket::RecvMsgFlags::empty())
                .await
                .unwrap()
                .expect("recvmsg should return empty data");
            assert!(data.is_empty());
        };
        let ((), ()) = futures::future::join(left, right).await;

        socket
            .shutdown(fposix_socket::ShutdownMode::READ)
            .await
            .unwrap()
            .expect("failed to shutdown the socket twice");
        socket
            .shutdown(fposix_socket::ShutdownMode::WRITE)
            .await
            .unwrap()
            .expect("failed to shutdown the socket twice");
        socket
            .shutdown(fposix_socket::ShutdownMode::READ | fposix_socket::ShutdownMode::WRITE)
            .await
            .unwrap()
            .expect("failed to shutdown the socket twice");

        t
    }

    declare_tests!(shutdown);

    #[fixture::teardown(TestSetup::shutdown)]
    async fn set_receive_buffer_after_delivery<
        A: TestSockAddr,
        T: Transport<<A::AddrType as IpAddress>::Version> + Transport<Ipv4> + Transport<Ipv6>,
    >(
        proto: fposix_socket::DatagramSocketProtocol,
    ) where
        <A::AddrType as IpAddress>::Version: IcmpIpExt,
    {
        let mut t = TestSetupBuilder::new().add_stack(StackSetupBuilder::new()).build().await;

        let (socket, _events) = get_socket_and_event::<A>(t.get_mut(0), proto).await;
        let addr =
            A::create(<<A::AddrType as IpAddress>::Version as Ip>::LOOPBACK_ADDRESS.get(), 200);
        socket.bind(&addr).await.unwrap().expect("bind should succeed");
        let min_recv_buf =
            <T as Transport<<A::AddrType as IpAddress>::Version>>::get_rcvbuf_settings(
                &mut t.get_mut(0).ctx(),
            )
            .min()
            .get();

        const SENT_PACKETS: u8 = 10;
        for i in 0..SENT_PACKETS {
            let buf = prepare_buffer_to_send::<A>(proto, vec![i; min_recv_buf]);
            let sent: usize = socket
                .send_msg(
                    Some(&addr),
                    &buf,
                    &fposix_socket::DatagramSocketSendControlData::default(),
                    fposix_socket::SendMsgFlags::empty(),
                )
                .await
                .unwrap()
                .expect("send_msg should succeed")
                .try_into()
                .unwrap();
            assert_eq!(sent, buf.len());
        }

        // Wait for all packets to be delivered before changing the buffer size.
        let stack = t.get_mut(0);
        let has_all_delivered = |messages: &MessageQueue<_, _>| {
            messages.available_messages().len() == usize::from(SENT_PACKETS)
        };
        loop {
            let all_delivered = stack.with_ctx(|ctx| {
                let socket = <T as Transport<IpFromSockAddr<A>>>::collect_all_sockets(ctx)
                    .into_iter()
                    .next()
                    .unwrap();
                let external_data = <T as Transport<IpFromSockAddr<A>>>::external_data(&socket);
                let message_queue = external_data.message_queue.lock();
                has_all_delivered(&message_queue)
            });
            if all_delivered {
                break;
            }
            // Give other futures on the same executor a chance to run. In a
            // single-threaded context, without the yield, this future would
            // always be able to re-lock the stack after unlocking, and so no
            // other future would make progress.
            futures_lite::future::yield_now().await;
        }

        // Use a buffer size of 0, which will be substituted with the minimum size.
        let () =
            socket.set_receive_buffer(0).await.unwrap().expect("set buffer size should succeed");

        let rx_count = futures::stream::unfold(socket, |socket| async {
            let result = socket
                .recv_msg(false, u32::MAX, false, fposix_socket::RecvMsgFlags::empty())
                .await
                .unwrap();
            match result {
                Ok((addr, data, control, size)) => {
                    let _: (
                        Option<Box<fnet::SocketAddress>>,
                        fposix_socket::DatagramSocketRecvControlData,
                        u32,
                    ) = (addr, control, size);
                    Some((data, socket))
                }
                Err(fposix::Errno::Eagain) => None,
                Err(e) => panic!("unexpected error: {:?}", e),
            }
        })
        .enumerate()
        .map(|(i, data)| {
            assert_eq!(
                &data,
                &expected_buffer_to_receive::<A>(
                    proto,
                    vec![u8::try_from(i).unwrap(); min_recv_buf],
                    200,
                    <<A::AddrType as IpAddress>::Version as Ip>::LOOPBACK_ADDRESS.get(),
                    <<A::AddrType as IpAddress>::Version as Ip>::LOOPBACK_ADDRESS.get(),
                )
            )
        })
        .count()
        .await;
        assert_eq!(rx_count, usize::from(SENT_PACKETS));

        t
    }

    declare_tests!(set_receive_buffer_after_delivery);

    #[fixture::teardown(TestSetup::shutdown)]
    async fn send_recv_loopback_peek<A: TestSockAddr, T>(
        proto: fposix_socket::DatagramSocketProtocol,
    ) where
        <A::AddrType as IpAddress>::Version: IcmpIpExt,
    {
        let (t, proxy, _event) = prepare_test::<A>(proto).await;
        let addr =
            A::create(<<A::AddrType as IpAddress>::Version as Ip>::LOOPBACK_ADDRESS.get(), 100);

        proxy.bind(&addr).await.unwrap().expect("bind succeeds");
        proxy.connect(&addr).await.unwrap().expect("connect succeeds");

        const DATA: &[u8] = &[1, 2, 3, 4, 5];
        let to_send = prepare_buffer_to_send::<A>(proto, DATA.to_vec());
        assert_eq!(
            usize::try_from(
                proxy
                    .send_msg(
                        None,
                        &to_send,
                        &fposix_socket::DatagramSocketSendControlData::default(),
                        fposix_socket::SendMsgFlags::empty()
                    )
                    .await
                    .unwrap()
                    .expect("send_msg should succeed"),
            )
            .unwrap(),
            to_send.len()
        );

        // First try receiving the message with PEEK set.
        let (_addr, data, _control, truncated) = loop {
            match proxy
                .recv_msg(false, u32::MAX, false, fposix_socket::RecvMsgFlags::PEEK)
                .await
                .unwrap()
            {
                Ok(peek) => break peek,
                Err(fposix::Errno::Eagain) => {
                    // The sent datagram hasn't been received yet, so check for
                    // it again in a moment.
                    continue;
                }
                Err(e) => panic!("unexpected error: {e:?}"),
            }
        };
        let expected = expected_buffer_to_receive::<A>(
            proto,
            DATA.to_vec(),
            100,
            <<A::AddrType as IpAddress>::Version as Ip>::LOOPBACK_ADDRESS.get(),
            <<A::AddrType as IpAddress>::Version as Ip>::LOOPBACK_ADDRESS.get(),
        );
        assert_eq!(truncated, 0);
        assert_eq!(data.as_slice(), expected,);

        // Now that the message has for sure been received, it can be retrieved
        // without checking for Eagain.
        let (_addr, data, _control, truncated) = proxy
            .recv_msg(false, u32::MAX, false, fposix_socket::RecvMsgFlags::empty())
            .await
            .unwrap()
            .expect("recv should succeed");
        assert_eq!(truncated, 0);
        assert_eq!(data.as_slice(), expected);

        t
    }

    declare_tests!(send_recv_loopback_peek);

    // TODO(https://fxbug.dev/42174378): add a syscall test to exercise this
    // behavior.
    #[fixture::teardown(TestSetup::shutdown)]
    async fn multicast_join_receive<A: TestSockAddr, T>(
        proto: fposix_socket::DatagramSocketProtocol,
    ) {
        let (t, proxy, event) = prepare_test::<A>(proto).await;

        let mcast_addr = <<A::AddrType as IpAddress>::Version as Ip>::MULTICAST_SUBNET.network();
        let id = t.get(0).get_endpoint_id(1);

        match mcast_addr.into() {
            IpAddr::V4(mcast_addr) => {
                proxy.add_ip_membership(&fposix_socket::IpMulticastMembership {
                    mcast_addr: mcast_addr.into_fidl(),
                    iface: id.get(),
                    local_addr: fnet::Ipv4Address { addr: [0; 4] },
                })
            }
            IpAddr::V6(mcast_addr) => {
                proxy.add_ipv6_membership(&fposix_socket::Ipv6MulticastMembership {
                    mcast_addr: mcast_addr.into_fidl(),
                    iface: id.get(),
                })
            }
        }
        .await
        .unwrap()
        .expect("add membership should succeed");

        const PORT: u16 = 100;
        const DATA: &[u8] = &[1, 2, 3, 4, 5];

        proxy
            .bind(&A::create(
                <<A::AddrType as IpAddress>::Version as Ip>::UNSPECIFIED_ADDRESS,
                PORT,
            ))
            .await
            .unwrap()
            .expect("bind succeeds");

        assert_eq!(
            usize::try_from(
                proxy
                    .send_msg(
                        Some(&A::create(mcast_addr, PORT)),
                        DATA,
                        &fposix_socket::DatagramSocketSendControlData::default(),
                        fposix_socket::SendMsgFlags::empty()
                    )
                    .await
                    .unwrap()
                    .expect("send_msg should succeed"),
            )
            .unwrap(),
            DATA.len()
        );

        let _signals = event
            .wait_one(ZXSIO_SIGNAL_INCOMING, zx::MonotonicInstant::INFINITE)
            .expect("socket should receive");

        let (_addr, data, _control, truncated) = proxy
            .recv_msg(false, u32::MAX, false, fposix_socket::RecvMsgFlags::empty())
            .await
            .unwrap()
            .expect("recv should succeed");
        assert_eq!(truncated, 0);
        assert_eq!(data.as_slice(), DATA);

        t
    }

    declare_tests!(
        multicast_join_receive,
        icmp #[should_panic = "Eopnotsupp"]
    );

    #[fixture::teardown(TestSetup::shutdown)]
    async fn set_get_hop_limit_unicast<A: TestSockAddr, T>(
        proto: fposix_socket::DatagramSocketProtocol,
    ) {
        let (t, proxy, _event) = prepare_test::<A>(proto).await;

        const HOP_LIMIT: u8 = 200;
        match <<A::AddrType as IpAddress>::Version as Ip>::VERSION {
            IpVersion::V4 => proxy.set_ip_multicast_ttl(&Some(HOP_LIMIT).into_fidl()),
            IpVersion::V6 => proxy.set_ipv6_multicast_hops(&Some(HOP_LIMIT).into_fidl()),
        }
        .await
        .unwrap()
        .expect("set hop limit should succeed");

        assert_eq!(
            match <<A::AddrType as IpAddress>::Version as Ip>::VERSION {
                IpVersion::V4 => proxy.get_ip_multicast_ttl(),
                IpVersion::V6 => proxy.get_ipv6_multicast_hops(),
            }
            .await
            .unwrap()
            .expect("get hop limit should succeed"),
            HOP_LIMIT
        );

        t
    }

    declare_tests!(set_get_hop_limit_unicast);

    #[fixture::teardown(TestSetup::shutdown)]
    async fn set_get_hop_limit_multicast<A: TestSockAddr, T>(
        proto: fposix_socket::DatagramSocketProtocol,
    ) {
        let (t, proxy, _event) = prepare_test::<A>(proto).await;

        const HOP_LIMIT: u8 = 200;
        match <<A::AddrType as IpAddress>::Version as Ip>::VERSION {
            IpVersion::V4 => proxy.set_ip_ttl(&Some(HOP_LIMIT).into_fidl()),
            IpVersion::V6 => proxy.set_ipv6_unicast_hops(&Some(HOP_LIMIT).into_fidl()),
        }
        .await
        .unwrap()
        .expect("set hop limit should succeed");

        assert_eq!(
            match <<A::AddrType as IpAddress>::Version as Ip>::VERSION {
                IpVersion::V4 => proxy.get_ip_ttl(),
                IpVersion::V6 => proxy.get_ipv6_unicast_hops(),
            }
            .await
            .unwrap()
            .expect("get hop limit should succeed"),
            HOP_LIMIT
        );

        t
    }

    declare_tests!(set_get_hop_limit_multicast);

    #[fixture::teardown(TestSetup::shutdown)]
    async fn set_hop_limit_wrong_type<A: TestSockAddr, T>(
        proto: fposix_socket::DatagramSocketProtocol,
    ) {
        let (t, proxy, _event) = prepare_test::<A>(proto).await;

        const HOP_LIMIT: u8 = 200;
        let (multicast_result, unicast_result) =
            match <<A::AddrType as IpAddress>::Version as Ip>::VERSION {
                IpVersion::V4 => (
                    proxy.set_ipv6_multicast_hops(&Some(HOP_LIMIT).into_fidl()).await.unwrap(),
                    proxy.set_ipv6_unicast_hops(&Some(HOP_LIMIT).into_fidl()).await.unwrap(),
                ),
                IpVersion::V6 => (
                    proxy.set_ip_multicast_ttl(&Some(HOP_LIMIT).into_fidl()).await.unwrap(),
                    proxy.set_ip_ttl(&Some(HOP_LIMIT).into_fidl()).await.unwrap(),
                ),
            };

        match (proto, <<A::AddrType as IpAddress>::Version as Ip>::VERSION) {
            // UDPv6 is a dualstack capable protocol, so it allows setting the
            // TTL of IPv6 sockets.
            (fposix_socket::DatagramSocketProtocol::Udp, IpVersion::V6) => {
                assert_matches!(multicast_result, Ok(_));
                assert_matches!(unicast_result, Ok(_));
            }
            // All other [protocol, ip_version] are not dualstack capable.
            (_, _) => {
                assert_matches!(multicast_result, Err(_));
                assert_matches!(unicast_result, Err(_));
            }
        }

        t
    }

    declare_tests!(set_hop_limit_wrong_type);

    #[fixture::teardown(TestSetup::shutdown)]
    async fn get_hop_limit_wrong_type<A: TestSockAddr, T>(
        proto: fposix_socket::DatagramSocketProtocol,
    ) {
        let (t, proxy, _event) = prepare_test::<A>(proto).await;

        let (multicast_result, unicast_result) =
            match <<A::AddrType as IpAddress>::Version as Ip>::VERSION {
                IpVersion::V4 => (
                    proxy.get_ipv6_multicast_hops().await.unwrap(),
                    proxy.get_ipv6_unicast_hops().await.unwrap(),
                ),
                IpVersion::V6 => {
                    (proxy.get_ip_multicast_ttl().await.unwrap(), proxy.get_ip_ttl().await.unwrap())
                }
            };

        match (proto, <<A::AddrType as IpAddress>::Version as Ip>::VERSION) {
            // UDPv6 is a dualstack capable protocol, so it allows getting the
            // TTL of IPv6 sockets.
            (fposix_socket::DatagramSocketProtocol::Udp, IpVersion::V6) => {
                assert_matches!(multicast_result, Ok(_));
                assert_matches!(unicast_result, Ok(_));
            }
            // All other [protocol, ip_version] are not dualstack capable.
            (_, _) => {
                assert_matches!(multicast_result, Err(_));
                assert_matches!(unicast_result, Err(_));
            }
        }

        t
    }

    declare_tests!(get_hop_limit_wrong_type);

    #[fixture::teardown(TestSetup::shutdown)]
    async fn set_get_multicast_loop<A: TestSockAddr, T>(
        proto: fposix_socket::DatagramSocketProtocol,
    ) {
        let (t, proxy, _event) = prepare_test::<A>(proto).await;

        for multicast_loop in [false, true] {
            match <<A::AddrType as IpAddress>::Version as Ip>::VERSION {
                IpVersion::V4 => proxy.set_ip_multicast_loopback(multicast_loop),
                IpVersion::V6 => proxy.set_ipv6_multicast_loopback(multicast_loop),
            }
            .await
            .unwrap()
            .expect("set multicast loop should succeed");

            assert_eq!(
                match <<A::AddrType as IpAddress>::Version as Ip>::VERSION {
                    IpVersion::V4 => proxy.get_ip_multicast_loopback(),
                    IpVersion::V6 => proxy.get_ipv6_multicast_loopback(),
                }
                .await
                .unwrap()
                .expect("get multicast loop should succeed"),
                multicast_loop
            );
        }

        t
    }

    declare_tests!(set_get_multicast_loop);

    #[fixture::teardown(TestSetup::shutdown)]
    async fn set_get_multicast_loop_wrong_type<A: TestSockAddr, T>(
        proto: fposix_socket::DatagramSocketProtocol,
    ) {
        let (t, proxy, _event) = prepare_test::<A>(proto).await;

        const MULTICAST_LOOP: bool = false;
        let (get_result, set_result) = match <<A::AddrType as IpAddress>::Version as Ip>::VERSION {
            IpVersion::V4 => (
                proxy.get_ipv6_multicast_loopback().await.unwrap(),
                proxy.set_ipv6_multicast_loopback(MULTICAST_LOOP).await.unwrap(),
            ),
            IpVersion::V6 => (
                proxy.get_ip_multicast_loopback().await.unwrap(),
                proxy.set_ip_multicast_loopback(MULTICAST_LOOP).await.unwrap(),
            ),
        };

        match (proto, <<A::AddrType as IpAddress>::Version as Ip>::VERSION) {
            // UDPv6 is a dualstack capable protocol, so it allows getting &
            // setting the IP_MULTICAST_LOOP of IPv6 sockets.
            (fposix_socket::DatagramSocketProtocol::Udp, IpVersion::V6) => {
                assert_matches!(get_result, Ok(_));
                assert_matches!(set_result, Ok(_));
            }
            // All other [protocol, ip_version] are not dualstack capable.
            (_, _) => {
                assert_matches!(get_result, Err(_));
                assert_matches!(set_result, Err(_));
            }
        }

        t
    }

    declare_tests!(set_get_multicast_loop_wrong_type);

    #[fixture::teardown(TestSetup::shutdown)]
    async fn receive_original_destination_address<A: TestSockAddr, T>(
        proto: fposix_socket::DatagramSocketProtocol,
    ) {
        // Follow the same steps as the hello test above: Create two stacks, Alice (server listening
        // on LOCAL_ADDR:200), and Bob (client, bound on REMOTE_ADDR:300).
        let mut t = TestSetupBuilder::new()
            .add_endpoint()
            .add_endpoint()
            .add_stack(
                StackSetupBuilder::new()
                    .add_named_endpoint(test_ep_name(1), Some(A::config_addr_subnet())),
            )
            .add_stack(
                StackSetupBuilder::new()
                    .add_named_endpoint(test_ep_name(2), Some(A::config_addr_subnet_remote())),
            )
            .build()
            .await;

        let alice = t.get_mut(0);
        let (alice_socket, alice_events) = get_socket_and_event::<A>(alice, proto).await;

        // Setup Alice as a server, bound to LOCAL_ADDR:200
        println!("Configuring alice...");
        alice_socket
            .bind(&A::create(A::LOCAL_ADDR, 200))
            .await
            .unwrap()
            .expect("alice bind suceeds");

        // Setup Bob as a client, bound to REMOTE_ADDR:300
        println!("Configuring bob...");
        let bob = t.get_mut(1);
        let bob_socket = get_socket::<A>(bob, proto).await;
        bob_socket.bind(&A::create(A::REMOTE_ADDR, 300)).await.unwrap().expect("bob bind suceeds");

        // Connect Bob to Alice on LOCAL_ADDR:200
        println!("Connecting bob to alice...");
        bob_socket
            .connect(&A::create(A::LOCAL_ADDR, 200))
            .await
            .unwrap()
            .expect("Connect succeeds");

        // Send datagram from Bob's socket.
        println!("Writing datagram to bob");
        let body = "Hello".as_bytes();
        assert_eq!(
            bob_socket
                .send_msg(
                    None,
                    &body,
                    &fposix_socket::DatagramSocketSendControlData::default(),
                    fposix_socket::SendMsgFlags::empty()
                )
                .await
                .unwrap()
                .expect("sendmsg suceeds"),
            body.len() as i64
        );

        // Wait for datagram to arrive on Alice's socket:

        println!("Waiting for signals");
        assert_eq!(
            fasync::OnSignals::new(&alice_events, ZXSIO_SIGNAL_INCOMING).await,
            Ok(ZXSIO_SIGNAL_INCOMING | ZXSIO_SIGNAL_OUTGOING)
        );

        // Check the option is currently false.
        assert!(
            !alice_socket
                .get_ip_receive_original_destination_address()
                .await
                .expect("get_ip_receive_original_destination_address (FIDL) failed")
                .expect("get_ip_receive_original_destination_address failed"),
        );

        alice_socket
            .set_ip_receive_original_destination_address(true)
            .await
            .expect("set_ip_receive_original_destination_address (FIDL) failed")
            .expect("set_ip_receive_original_destination_address failed");

        // The option should now be reported as set.
        assert!(
            alice_socket
                .get_ip_receive_original_destination_address()
                .await
                .expect("get_ip_receive_original_destination_address (FIDL) failed")
                .expect("get_ip_receive_original_destination_address failed"),
        );

        let recvmsg_result = alice_socket
            .recv_msg(false, 2048, true, fposix_socket::RecvMsgFlags::empty())
            .await
            .unwrap()
            .expect("recvmsg suceeeds");

        // `original_destination_address` should be sent only for IPv4 packets.
        if A::DOMAIN == fposix_socket::Domain::Ipv4 {
            assert_matches!(recvmsg_result,
                (
                    _,
                    _,
                    fposix_socket::DatagramSocketRecvControlData {
                        network:
                            Some(fposix_socket::NetworkSocketRecvControlData {
                                ip:
                                    Some(fposix_socket::IpRecvControlData {
                                        original_destination_address: Some(addr),
                                        ..
                                    }),
                                ..
                            }),
                        ..
                    },
                    _,
                ) => {
                    let addr = A::from_sock_addr(addr).expect("bad socket address return");
                    assert_eq!(addr.addr(), A::LOCAL_ADDR);
                    assert_eq!(addr.port(), 200);
                }
            );
        } else {
            assert_matches!(
                recvmsg_result,
                (_, _, fposix_socket::DatagramSocketRecvControlData { network: None, .. }, _,)
            );
        }

        // Turn it off.
        alice_socket
            .set_ip_receive_original_destination_address(false)
            .await
            .expect("set_ip_receive_original_destination_address (FIDL) failed")
            .expect("set_ip_receive_original_destination_address failed");

        assert!(
            !alice_socket
                .get_ip_receive_original_destination_address()
                .await
                .expect("get_ip_receive_original_destination_address (FIDL) failed")
                .expect("get_ip_receive_original_destination_address failed"),
        );

        assert_eq!(
            bob_socket
                .send_msg(
                    None,
                    &body,
                    &fposix_socket::DatagramSocketSendControlData::default(),
                    fposix_socket::SendMsgFlags::empty()
                )
                .await
                .unwrap()
                .expect("sendmsg suceeds"),
            body.len() as i64
        );

        // Wait for datagram to arrive on Alice's socket:
        println!("Waiting for signals");
        assert_eq!(
            fasync::OnSignals::new(&alice_events, ZXSIO_SIGNAL_INCOMING).await,
            Ok(ZXSIO_SIGNAL_INCOMING | ZXSIO_SIGNAL_OUTGOING)
        );

        assert_matches!(
            alice_socket
                .recv_msg(false, 2048, true, fposix_socket::RecvMsgFlags::empty())
                .await
                .unwrap()
                .expect("recvmsg suceeeds"),
            (_, _, fposix_socket::DatagramSocketRecvControlData { network: None, .. }, _)
        );

        t
    }

    declare_tests!(
        receive_original_destination_address,
        icmp #[ignore] // ICMP sockets' send/recv are different from what UDP
        // does, i.e., alice doesn't receive what bob sends, but rather bob
        // receives the echo reply for the echo request they send. If we need
        // this option for ICMP sockets, we should write a dedicated test for
        // ICMP.
    );
}
