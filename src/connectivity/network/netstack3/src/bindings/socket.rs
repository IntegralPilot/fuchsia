// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Socket features exposed by netstack3.

use std::convert::Infallible as Never;
use std::fmt::Debug;
use std::num::NonZeroU64;
use std::panic::Location;

use either::Either;
use fidl::encoding::ProxyChannelBox as _;
use fidl::endpoints::RequestStream as _;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_posix::Errno;
use fidl_fuchsia_posix_socket as psocket;
use futures::TryStreamExt as _;
use log::warn;
use net_types::ip::{Ip, IpAddress, Ipv4, Ipv4Addr, Ipv6, Ipv6Addr};
use net_types::{ScopeableAddress, SpecifiedAddr, Witness, ZonedAddr};
use netstack3_core::device::DeviceId;
use netstack3_core::error::{
    LocalAddressError, RemoteAddressError, SocketError, ZonedAddressError,
};
use netstack3_core::ip::{IpSockCreationError, IpSockSendError, ResolveRouteError};
use netstack3_core::socket::{
    ConnectError, NotDualStackCapableError, SetDualStackEnabledError, SetMulticastMembershipError,
    SharingDomain,
};
use netstack3_core::{tcp, udp};
use zx::AsHandleRef;

use crate::bindings::devices::{
    BindingId, DeviceIdAndName, DeviceSpecificInfo, Devices, DynamicCommonInfo,
    DynamicEthernetInfo, DynamicNetdeviceInfo,
};
use crate::bindings::errno::ErrnoError;
use crate::bindings::error::Error;
use crate::bindings::util::{
    DeviceNotFoundError, IntoCore as _, IntoFidl as _, ResultExt as _, TryIntoCoreWithContext,
};
use crate::bindings::{Ctx, DeviceIdExt as _};

#[track_caller]
fn log_not_supported(name: &str) {
    let location = Location::caller();
    // TODO(https://fxbug.dev/343992493): don't embed location in the log
    // message when better track_caller support is available.
    warn!("{location}: {} not supported", name);
}

macro_rules! respond_not_supported {
    ($name:expr, $responder:expr) => {{
        crate::bindings::socket::log_not_supported($name);
        crate::bindings::util::ResultExt::unwrap_or_log(
            $responder.send(Err(fidl_fuchsia_posix::Errno::Eopnotsupp)),
            "failed to respond",
        )
    }};
}

pub(crate) mod datagram;
pub(crate) mod event_pair;
pub(crate) mod packet;
pub(crate) mod queue;
pub(crate) mod raw;
pub(crate) mod stream;
pub(crate) mod worker;

const ZXSIO_SIGNAL_INCOMING: zx::Signals =
    zx::Signals::from_bits(psocket::SIGNAL_DATAGRAM_INCOMING).unwrap();
const ZXSIO_SIGNAL_OUTGOING: zx::Signals =
    zx::Signals::from_bits(psocket::SIGNAL_DATAGRAM_OUTGOING).unwrap();
const ZXSIO_SIGNAL_ERROR: zx::Signals =
    zx::Signals::from_bits(psocket::SIGNAL_DATAGRAM_ERROR).unwrap();
const ZXSIO_SIGNAL_CONNECTED: zx::Signals =
    zx::Signals::from_bits(psocket::SIGNAL_STREAM_CONNECTED).unwrap();

/// Common properties for socket workers.
#[derive(Debug)]
pub(crate) struct SocketWorkerProperties {}

pub(crate) async fn serve(
    mut ctx: crate::bindings::Ctx,
    stream: psocket::ProviderRequestStream,
) -> Result<(), fidl::Error> {
    // Get KOID from the FIDL channel and use it as `SharingDomain` for the SO_REUSEPORT
    // implementation until all clients are updated to supply sharing domain explicitly.
    // TODO(https://fxbug.dev/451615802): Remove this once all clients are updated to
    // supply sharing domain explicitly.
    let (stream_inner, terminated) = stream.into_inner();
    let channel_koid = stream_inner
        .channel()
        .as_channel()
        .as_handle_ref()
        .koid()
        .expect("failed to get channel koid")
        .raw_koid();
    let mut stream = psocket::ProviderRequestStream::from_inner(stream_inner, terminated);
    let sharing_domain = SharingDomain::new(channel_koid);

    while let Some(req) = stream.try_next().await? {
        match req {
            psocket::ProviderRequest::InterfaceIndexToName { index, responder } => {
                let response = {
                    let bindings_ctx = ctx.bindings_ctx();
                    BindingId::new(index)
                        .ok_or(DeviceNotFoundError)
                        .and_then(|id| id.try_into_core_with_ctx(bindings_ctx))
                        .map(|core_id: DeviceId<_>| core_id.bindings_id().name.clone())
                        .map_err(|DeviceNotFoundError| zx::Status::NOT_FOUND.into_raw())
                };
                responder
                    .send(response.as_deref().map_err(|e| *e))
                    .unwrap_or_log("failed to respond");
            }
            psocket::ProviderRequest::InterfaceNameToIndex { name, responder } => {
                let response = {
                    let bindings_ctx = ctx.bindings_ctx();
                    let devices = AsRef::<Devices<_>>::as_ref(bindings_ctx);
                    let result = devices
                        .get_device_by_name(&name)
                        .map(|d| d.bindings_id().id.get())
                        .ok_or_else(|| zx::Status::NOT_FOUND.into_raw());
                    result
                };
                responder.send(response).unwrap_or_log("failed to respond");
            }
            psocket::ProviderRequest::InterfaceNameToFlags { name, responder } => {
                responder.send(get_interface_flags(&ctx, &name)).unwrap_or_log("failed to respond");
            }
            psocket::ProviderRequest::StreamSocket { domain, proto, responder } => {
                let (client, request_stream) = create_request_stream();
                stream::spawn_worker(
                    domain,
                    proto,
                    ctx.clone(),
                    request_stream,
                    Default::default(),
                );
                responder.send(Ok(client)).unwrap_or_log("failed to respond");
            }
            psocket::ProviderRequest::StreamSocketWithOptions {
                domain,
                proto,
                opts,
                responder,
            } => {
                let (client, request_stream) = create_request_stream();
                stream::spawn_worker(domain, proto, ctx.clone(), request_stream, opts);
                responder.send(Ok(client)).unwrap_or_log("failed to respond");
            }
            psocket::ProviderRequest::DatagramSocketDeprecated { domain, proto, responder } => {
                let (client, request_stream) = create_request_stream();
                datagram::spawn_worker(
                    domain,
                    proto,
                    ctx.clone(),
                    request_stream,
                    SocketWorkerProperties {},
                    Default::default(),
                    sharing_domain,
                );
                responder.send(Ok(client)).unwrap_or_log("failed to respond");
            }
            psocket::ProviderRequest::DatagramSocket { domain, proto, responder } => {
                let (client, request_stream) = create_request_stream();
                datagram::spawn_worker(
                    domain,
                    proto,
                    ctx.clone(),
                    request_stream,
                    SocketWorkerProperties {},
                    Default::default(),
                    sharing_domain,
                );
                responder
                    .send(Ok(psocket::ProviderDatagramSocketResponse::SynchronousDatagramSocket(
                        client,
                    )))
                    .unwrap_or_log("failed to respond");
            }
            psocket::ProviderRequest::DatagramSocketWithOptions {
                domain,
                proto,
                responder,
                opts,
            } => {
                let (client, request_stream) = create_request_stream();
                datagram::spawn_worker(
                    domain,
                    proto,
                    ctx.clone(),
                    request_stream,
                    SocketWorkerProperties {},
                    opts,
                    sharing_domain,
                );
                use psocket::ProviderDatagramSocketWithOptionsResponse as Response;
                responder
                    .send(Ok(Response::SynchronousDatagramSocket(client)))
                    .unwrap_or_log("failed to respond");
            }
            psocket::ProviderRequest::GetInterfaceAddresses { responder } => {
                responder
                    .send(&get_interface_addresses(&mut ctx))
                    .unwrap_or_log("failed to respond");
            }
        }
    }
    Ok(())
}

pub(crate) fn create_request_stream<T: fidl::endpoints::ProtocolMarker>()
-> (fidl::endpoints::ClientEnd<T>, T::RequestStream) {
    fidl::endpoints::create_request_stream()
}

fn get_interface_addresses(ctx: &mut Ctx) -> Vec<psocket::InterfaceAddresses> {
    // Snapshot devices out so we don't hold any locks while calling into core.
    let devices =
        ctx.bindings_ctx().devices.with_devices(|devices| devices.cloned().collect::<Vec<_>>());
    devices
        .into_iter()
        .map(|d| {
            let mut addresses = Vec::new();
            ctx.api().device_ip_any().for_each_assigned_ip_addr_subnet(&d, |a| {
                addresses.push(fidl_fuchsia_net_ext::FromExt::from_ext(a))
            });

            let DeviceIdAndName { id, name } = d.bindings_id();
            let info = d.external_state();
            let flags = flags_for_device(&info);

            psocket::InterfaceAddresses {
                id: Some(id.get()),
                name: Some(name.clone()),
                addresses: Some(addresses),
                interface_flags: Some(flags),
                ..Default::default()
            }
        })
        .collect::<Vec<_>>()
}

fn get_interface_flags(
    ctx: &Ctx,
    name: &str,
) -> Result<psocket::InterfaceFlags, zx::sys::zx_status_t> {
    let bindings_ctx = ctx.bindings_ctx();
    let device = bindings_ctx
        .devices
        .get_device_by_name(name)
        .ok_or_else(|| zx::Status::NOT_FOUND.into_raw())?;
    Ok(flags_for_device(&device.external_state()))
}

fn flags_for_device(info: &DeviceSpecificInfo<'_>) -> psocket::InterfaceFlags {
    struct Flags {
        physical_up: bool,
        admin_enabled: bool,
        loopback: bool,
    }

    // NB: Exists to force destructuring `DynamicCommonInfo` without repetition
    // in the match statement below.
    struct FromDynamicInfo {
        admin_enabled: bool,
    }
    impl<'a> From<&'a DynamicCommonInfo> for FromDynamicInfo {
        fn from(value: &'a DynamicCommonInfo) -> Self {
            let DynamicCommonInfo {
                mtu: _,
                admin_enabled,
                ipv4_enabled: _,
                ipv6_enabled: _,
                events: _,
                control_hook: _,
                addresses: _,
            } = value;
            FromDynamicInfo { admin_enabled: *admin_enabled }
        }
    }

    let Flags { physical_up, admin_enabled, loopback } = match info {
        DeviceSpecificInfo::Ethernet(info) => info.with_dynamic_info(
            |DynamicEthernetInfo {
                 netdevice: DynamicNetdeviceInfo { common_info, phy_up },
                 neighbor_event_sink: _,
             }| {
                let FromDynamicInfo { admin_enabled } = common_info.into();
                Flags { physical_up: *phy_up, admin_enabled, loopback: false }
            },
        ),
        DeviceSpecificInfo::Loopback(info) => info.with_dynamic_info(|common_info| {
            let FromDynamicInfo { admin_enabled } = common_info.into();
            Flags { physical_up: true, admin_enabled: admin_enabled, loopback: true }
        }),
        DeviceSpecificInfo::Blackhole(info) => info.with_dynamic_info(|common_info| {
            let FromDynamicInfo { admin_enabled } = common_info.into();
            Flags { physical_up: true, admin_enabled: admin_enabled, loopback: false }
        }),
        DeviceSpecificInfo::PureIp(info) => {
            info.with_dynamic_info(|DynamicNetdeviceInfo { common_info, phy_up }| {
                let FromDynamicInfo { admin_enabled } = common_info.into();
                Flags { physical_up: *phy_up, admin_enabled, loopback: false }
            })
        }
    };

    // Approximate that all interfaces support multicasting.
    // TODO(https://fxbug.dev/42076301): Set this more precisely.
    let multicast = true;

    // Note that the interface flags are not all intuitively named. Quotes below
    // are from https://www.xml.com/ldd/chapter/book/ch14.html#INDEX-3,507.
    [
        // IFF_UP is "on when the interface is active and ready to transfer
        // packets".
        (physical_up, psocket::InterfaceFlags::UP),
        // IFF_LOOPBACK is "set only in the loopback interface".
        (loopback, psocket::InterfaceFlags::LOOPBACK),
        // IFF_RUNNING "indicates that the interface is up and running".
        (admin_enabled && physical_up, psocket::InterfaceFlags::RUNNING),
        // IFF_MULTICAST is set for "interfaces that are capable of multicast
        // transmission".
        (multicast, psocket::InterfaceFlags::MULTICAST),
    ]
    .into_iter()
    .fold(psocket::InterfaceFlags::empty(), |mut flags, (b, flag)| {
        flags.set(flag, b);
        flags
    })
}

/// A trait generalizing the data structures passed as arguments to POSIX socket
/// calls.
///
/// `SockAddr` implementers are typically passed to POSIX socket calls as a blob
/// of bytes. It represents a type that can be parsed from a C API `struct
/// sockaddr`, expressed as a stream of bytes.
pub(crate) trait SockAddr: Debug + Sized + Send {
    /// The concrete address type for this `SockAddr`.
    type AddrType: IpAddress + ScopeableAddress;

    /// The socket's domain.
    #[cfg(test)]
    const DOMAIN: psocket::Domain;

    /// The unspecified instance of this `SockAddr`.
    const UNSPECIFIED: Self;

    /// Creates a new `SockAddr` from the provided address and port.
    ///
    /// `addr` is either `Some(a)` where `a` holds a specified address an
    /// optional zone, or `None` for the unspecified address (which can't have a
    /// zone).
    fn new(addr: Option<ZonedAddr<SpecifiedAddr<Self::AddrType>, NonZeroU64>>, port: u16) -> Self;

    /// Gets this `SockAddr`'s address.
    fn addr(&self) -> Self::AddrType;

    /// Gets this `SockAddr`'s port.
    fn port(&self) -> u16;

    /// Gets a `SpecifiedAddr` witness type for this `SockAddr`'s address.
    fn get_specified_addr(&self) -> Option<SpecifiedAddr<Self::AddrType>> {
        SpecifiedAddr::<Self::AddrType>::new(self.addr())
    }

    /// Gets this `SockAddr`'s zone identifier.
    fn zone(&self) -> Option<NonZeroU64>;

    /// Converts this `SockAddr` into an [`fnet::SocketAddress`].
    fn into_sock_addr(self) -> fnet::SocketAddress;

    /// Converts an [`fnet::SocketAddress`] into a `SockAddr`.
    fn from_sock_addr(addr: fnet::SocketAddress) -> Result<Self, ErrnoError>;
}

impl SockAddr for fnet::Ipv6SocketAddress {
    type AddrType = Ipv6Addr;
    #[cfg(test)]
    const DOMAIN: psocket::Domain = psocket::Domain::Ipv6;
    const UNSPECIFIED: Self = fnet::Ipv6SocketAddress {
        address: fnet::Ipv6Address { addr: [0; 16] },
        port: 0,
        zone_index: 0,
    };

    /// Creates a new `SockAddr6`.
    fn new(addr: Option<ZonedAddr<SpecifiedAddr<Ipv6Addr>, NonZeroU64>>, port: u16) -> Self {
        let (addr, zone_index) = addr.map_or((Ipv6::UNSPECIFIED_ADDRESS, 0), |addr| {
            let (addr, zone) = addr.into_addr_zone();
            (addr.get(), zone.map_or(0, NonZeroU64::get))
        });
        fnet::Ipv6SocketAddress { address: addr.into_fidl(), port, zone_index }
    }

    fn addr(&self) -> Ipv6Addr {
        self.address.into_core()
    }

    fn port(&self) -> u16 {
        self.port
    }

    fn zone(&self) -> Option<NonZeroU64> {
        NonZeroU64::new(self.zone_index)
    }

    fn into_sock_addr(self) -> fnet::SocketAddress {
        fnet::SocketAddress::Ipv6(self)
    }

    fn from_sock_addr(addr: fnet::SocketAddress) -> Result<Self, ErrnoError> {
        match addr {
            fnet::SocketAddress::Ipv6(a) => Ok(a),
            fnet::SocketAddress::Ipv4(_) => Err(ErrnoError::new(
                Errno::Eafnosupport,
                "Tried to get Ipv6SocketAddress from SocketAddress::Ipv4(_)",
            )),
        }
    }
}

impl SockAddr for fnet::Ipv4SocketAddress {
    type AddrType = Ipv4Addr;
    #[cfg(test)]
    const DOMAIN: psocket::Domain = psocket::Domain::Ipv4;
    const UNSPECIFIED: Self =
        fnet::Ipv4SocketAddress { address: fnet::Ipv4Address { addr: [0; 4] }, port: 0 };

    /// Creates a new `SockAddr4`.
    fn new(addr: Option<ZonedAddr<SpecifiedAddr<Ipv4Addr>, NonZeroU64>>, port: u16) -> Self {
        let addr = addr.map_or(Ipv4::UNSPECIFIED_ADDRESS, |zoned| zoned.into_unzoned().get());
        fnet::Ipv4SocketAddress { address: addr.into_fidl(), port }
    }

    fn addr(&self) -> Ipv4Addr {
        self.address.into_core()
    }

    fn port(&self) -> u16 {
        self.port
    }

    fn zone(&self) -> Option<NonZeroU64> {
        None
    }

    fn into_sock_addr(self) -> fnet::SocketAddress {
        fnet::SocketAddress::Ipv4(self)
    }

    fn from_sock_addr(addr: fnet::SocketAddress) -> Result<Self, ErrnoError> {
        match addr {
            fnet::SocketAddress::Ipv4(a) => Ok(a),
            fnet::SocketAddress::Ipv6(_) => Err(ErrnoError::new(
                Errno::Eafnosupport,
                "tried to get Ipv4SocketAddress from SocketAddress::Ipv6(_)",
            )),
        }
    }
}

/// Extension trait that associates a [`SockAddr`] and [`MulticastMembership`]
/// implementation to an IP version. We provide implementations for [`Ipv4`] and
/// [`Ipv6`].
pub(crate) trait IpSockAddrExt: Ip {
    type SocketAddress: SockAddr<AddrType = Self::Addr>;
}

impl IpSockAddrExt for Ipv4 {
    type SocketAddress = fnet::Ipv4SocketAddress;
}

impl IpSockAddrExt for Ipv6 {
    type SocketAddress = fnet::Ipv6SocketAddress;
}

#[cfg(test)]
mod testutil {
    use net_types::ip::{AddrSubnetEither, IpAddr};

    use super::*;

    /// A trait that exposes common test behavior to implementers of
    /// [`SockAddr`].
    pub(crate) trait TestSockAddr: SockAddr {
        /// A different domain.
        ///
        /// `Ipv4SocketAddress` defines it as `Ipv6SocketAddress` and
        /// vice-versa.
        type DifferentDomain: TestSockAddr;
        /// The local address used for tests.
        const LOCAL_ADDR: Self::AddrType;
        /// The remote address used for tests.
        const REMOTE_ADDR: Self::AddrType;
        /// An alternate remote address used for tests.
        const REMOTE_ADDR_2: Self::AddrType;
        /// An non-local address which is unreachable, used for tests.
        const UNREACHABLE_ADDR: Self::AddrType;

        /// The default subnet prefix used for tests.
        const DEFAULT_PREFIX: u8;

        /// Creates an [`fnet::SocketAddress`] with the given `addr` and `port`.
        fn create(addr: Self::AddrType, port: u16) -> fnet::SocketAddress {
            Self::new(SpecifiedAddr::new(addr).map(|a| ZonedAddr::Unzoned(a)), port)
                .into_sock_addr()
        }

        /// Gets the local address and prefix configured for the test
        /// [`SockAddr`].
        fn config_addr_subnet() -> AddrSubnetEither {
            AddrSubnetEither::new(IpAddr::from(Self::LOCAL_ADDR), Self::DEFAULT_PREFIX).unwrap()
        }

        /// Gets the remote address and prefix to use for the test [`SockAddr`].
        fn config_addr_subnet_remote() -> AddrSubnetEither {
            AddrSubnetEither::new(IpAddr::from(Self::REMOTE_ADDR), Self::DEFAULT_PREFIX).unwrap()
        }
    }

    impl TestSockAddr for fnet::Ipv6SocketAddress {
        type DifferentDomain = fnet::Ipv4SocketAddress;

        const LOCAL_ADDR: Ipv6Addr =
            Ipv6Addr::from_bytes([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 192, 168, 0, 1]);
        const REMOTE_ADDR: Ipv6Addr =
            Ipv6Addr::from_bytes([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 192, 168, 0, 2]);
        const REMOTE_ADDR_2: Ipv6Addr =
            Ipv6Addr::from_bytes([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 192, 168, 0, 3]);
        const UNREACHABLE_ADDR: Ipv6Addr =
            Ipv6Addr::from_bytes([0, 0, 0, 0, 0, 0, 0, 42, 0, 0, 0, 0, 192, 168, 0, 1]);
        const DEFAULT_PREFIX: u8 = 64;
    }

    impl TestSockAddr for fnet::Ipv4SocketAddress {
        type DifferentDomain = fnet::Ipv6SocketAddress;

        const LOCAL_ADDR: Ipv4Addr = Ipv4Addr::new([192, 168, 0, 1]);
        const REMOTE_ADDR: Ipv4Addr = Ipv4Addr::new([192, 168, 0, 2]);
        const REMOTE_ADDR_2: Ipv4Addr = Ipv4Addr::new([192, 168, 0, 3]);
        const UNREACHABLE_ADDR: Ipv4Addr = Ipv4Addr::new([192, 168, 42, 1]);
        const DEFAULT_PREFIX: u8 = 24;
    }
}

/// Trait expressing the conversion of error types into
/// [`fidl_fuchsia_posix::Errno`] errors for the POSIX-lite wrappers.
pub(crate) trait IntoErrno: Sized + Debug + Into<Error> {
    /// Returns the most equivalent POSIX error code for `self`.
    fn to_errno(&self) -> Errno;

    #[track_caller]
    fn into_errno_error(self) -> ErrnoError {
        ErrnoError::new(self.to_errno(), self)
    }
}

impl IntoErrno for Never {
    fn to_errno(&self) -> Errno {
        match *self {}
    }
}

impl<A: IntoErrno, B: IntoErrno> IntoErrno for Either<A, B> {
    fn to_errno(&self) -> Errno {
        match self {
            Either::Left(a) => a.to_errno(),
            Either::Right(b) => b.to_errno(),
        }
    }
}

impl IntoErrno for LocalAddressError {
    fn to_errno(&self) -> Errno {
        match self {
            LocalAddressError::CannotBindToAddress
            | LocalAddressError::FailedToAllocateLocalPort => Errno::Eaddrnotavail,
            LocalAddressError::AddressMismatch => Errno::Eaddrnotavail,
            LocalAddressError::AddressUnexpectedlyMapped => Errno::Einval,
            LocalAddressError::AddressInUse => Errno::Eaddrinuse,
            LocalAddressError::Zone(e) => e.to_errno(),
        }
    }
}

impl IntoErrno for netstack3_core::PendingDatagramSocketError {
    fn to_errno(&self) -> Errno {
        match self {
            Self::NetworkUnreachable => Errno::Enetunreach,
            Self::HostUnreachable => Errno::Ehostunreach,
            Self::ProtocolUnreachable => Errno::Econnrefused,
            Self::PortUnreachable => Errno::Econnrefused,
            Self::DestinationHostDown => Errno::Ehostdown,
            Self::PermissionDenied => Errno::Eacces,
            Self::ProtocolError => Errno::Eproto,
            Self::Aborted => Errno::Econnaborted,
            Self::PacketTooBig => Errno::Emsgsize,
        }
    }
}

impl IntoErrno for RemoteAddressError {
    fn to_errno(&self) -> Errno {
        match self {
            RemoteAddressError::NoRoute => Errno::Enetunreach,
        }
    }
}

impl IntoErrno for SocketError {
    fn to_errno(&self) -> Errno {
        match self {
            SocketError::Remote(e) => e.to_errno(),
            SocketError::Local(e) => e.to_errno(),
        }
    }
}

impl IntoErrno for ResolveRouteError {
    fn to_errno(&self) -> Errno {
        match self {
            ResolveRouteError::NoSrcAddr => Errno::Eaddrnotavail,
            ResolveRouteError::Unreachable => Errno::Enetunreach,
        }
    }
}

impl IntoErrno for IpSockCreationError {
    fn to_errno(&self) -> Errno {
        match self {
            IpSockCreationError::Route(e) => e.to_errno(),
        }
    }
}

impl IntoErrno for IpSockSendError {
    fn to_errno(&self) -> Errno {
        match self {
            IpSockSendError::Mtu | IpSockSendError::IllegalLoopbackAddress => Errno::Einval,
            IpSockSendError::Unroutable(e) => e.to_errno(),
            IpSockSendError::BroadcastNotAllowed => Errno::Eacces,
        }
    }
}

impl IntoErrno for udp::SendToError {
    fn to_errno(&self) -> Errno {
        match self {
            Self::NotWriteable => Errno::Epipe,
            Self::LocalAddress(err) => err.to_errno(),
            Self::CreateSock(err) => err.to_errno(),
            Self::Zone(err) => err.to_errno(),
            // NB: Mapping MTU to EMSGSIZE is different from the impl on
            // `IpSockSendError` which maps to EINVAL instead.
            Self::Send(IpSockSendError::Mtu) => Errno::Emsgsize,
            Self::Send(err) => err.to_errno(),
            Self::RemotePortUnset => Errno::Einval,
            Self::RemoteUnexpectedlyMapped => Errno::Enetunreach,
            Self::RemoteUnexpectedlyNonMapped => Errno::Eafnosupport,
            Self::SendBufferFull => Errno::Eagain,
            Self::InvalidLength => Errno::Emsgsize,
        }
    }
}

impl IntoErrno for udp::SendError {
    fn to_errno(&self) -> Errno {
        match self {
            // NB: Mapping MTU to EMSGSIZE is different from the impl on
            // `IpSockSendError` which maps to EINVAL instead.
            Self::IpSock(IpSockSendError::Mtu) => Errno::Emsgsize,
            Self::IpSock(err) => err.to_errno(),
            Self::NotWriteable => Errno::Epipe,
            Self::RemotePortUnset => Errno::Edestaddrreq,
            Self::SendBufferFull => Errno::Eagain,
            Self::InvalidLength => Errno::Emsgsize,
        }
    }
}

impl IntoErrno for ConnectError {
    fn to_errno(&self) -> Errno {
        match self {
            Self::Ip(err) => err.to_errno(),
            Self::Zone(err) => err.to_errno(),
            Self::CouldNotAllocateLocalPort => Errno::Eaddrnotavail,
            Self::SockAddrConflict => Errno::Eaddrinuse,
            Self::RemoteUnexpectedlyMapped => Errno::Enetunreach,
            Self::RemoteUnexpectedlyNonMapped => Errno::Eafnosupport,
        }
    }
}

impl IntoErrno for SetMulticastMembershipError {
    fn to_errno(&self) -> Errno {
        match self {
            Self::AddressNotAvailable
            | Self::DeviceDoesNotExist
            | Self::NoDeviceWithAddress
            | Self::NoDeviceAvailable => Errno::Enodev,
            Self::GroupNotJoined => Errno::Eaddrnotavail,
            Self::GroupAlreadyJoined => Errno::Eaddrinuse,
            Self::WrongDevice => Errno::Einval,
        }
    }
}

impl IntoErrno for ZonedAddressError {
    fn to_errno(&self) -> Errno {
        match self {
            Self::RequiredZoneNotProvided => Errno::Einval,
            Self::DeviceZoneMismatch => Errno::Einval,
        }
    }
}

impl IntoErrno for tcp::SetDeviceError {
    fn to_errno(&self) -> Errno {
        match self {
            Self::Conflict => Errno::Eaddrinuse,
            Self::Unroutable => Errno::Ehostunreach,
            Self::ZoneChange => Errno::Einval,
        }
    }
}

impl IntoErrno for SetDualStackEnabledError {
    fn to_errno(&self) -> Errno {
        match self {
            SetDualStackEnabledError::SocketIsBound => Errno::Einval,
            SetDualStackEnabledError::NotCapable(e) => e.to_errno(),
        }
    }
}

impl IntoErrno for NotDualStackCapableError {
    fn to_errno(&self) -> Errno {
        Errno::Enoprotoopt
    }
}
