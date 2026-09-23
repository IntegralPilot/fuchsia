// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::convert::Infallible as Never;
use std::fmt::Debug;
use std::marker::PhantomData;
use std::num::{NonZeroU16, NonZeroU64};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use fidl_fuchsia_net as fidl_net;
use fidl_fuchsia_net_ext as fnet_ext;
use fidl_fuchsia_net_interfaces as fnet_interfaces;
use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fidl_fuchsia_net_routes as fnet_routes;
use fidl_fuchsia_net_routes_ext as fnet_routes_ext;
use fidl_fuchsia_net_stack as fidl_net_stack;
use fidl_fuchsia_posix as fposix;
use fidl_fuchsia_posix_socket as fposix_socket;
use futures::task::AtomicWaker;
use futures::{Future, Stream};
use log::debug;
use net_types::ethernet::Mac;
use net_types::ip::{
    AddrSubnetEither, AddrSubnetError, GenericOverIp, Ip, IpAddr, IpAddress, Ipv4Addr, Ipv6Addr,
    SubnetEither, SubnetError,
};
use net_types::{AddrAndZone, MulticastAddr, SpecifiedAddr, Witness, ZonedAddr};
use netstack3_core::device::{DeviceId, WeakDeviceId};
use netstack3_core::error::{ExistsError, NotFoundError};
use netstack3_core::ip::{
    IgmpConfigMode, Lifetime, MarkDomain, MldConfigMode, PreferredLifetime,
    RouteDiscoveryConfiguration, RouteDiscoveryConfigurationUpdate, SlaacConfiguration,
    SlaacConfigurationUpdate,
};
use netstack3_core::neighbor::{NudUserConfig, NudUserConfigUpdate};
use netstack3_core::routes::{
    AddRouteError, AddableEntry, AddableEntryEither, AddableMetric, Entry, EntryEither, Metric,
    RawMetric,
};
use netstack3_core::socket::{
    self as core_socket, MulticastInterfaceSelector, MulticastMembershipInterfaceSelector,
};
use netstack3_core::sync::RemoveResourceResult;
use packet_formats::utils::NonZeroDuration;
use thiserror::Error;

use crate::bindings::devices::BindingId;
use crate::bindings::socket::{IntoErrno, IpSockAddrExt, SockAddr};
use crate::bindings::{BindingsCtx, LifetimeExt as _, routes};

mod data_available;
pub(crate) mod rcu;
mod result_ext;
mod scope_ext;
pub(crate) use data_available::*;
pub(crate) use result_ext::*;
pub(crate) use scope_ext::*;

/// The value used to specify that a `ForwardingEntry.metric` is unset, and the
/// entry's metric should track the interface's routing metric.
const UNSET_FORWARDING_ENTRY_METRIC: u32 = 0;

/// A signal used between Core and Bindings, whenever Bindings receive a
/// notification by the protocol (Core), it should kick the associated task
/// to do work.
#[derive(Debug)]
struct NeedsData {
    ready: AtomicBool,
    waker: AtomicWaker,
}

impl Default for NeedsData {
    fn default() -> NeedsData {
        NeedsData { ready: AtomicBool::new(false), waker: AtomicWaker::new() }
    }
}

impl NeedsData {
    fn poll_ready(&self, cx: &mut std::task::Context<'_>) -> std::task::Poll<()> {
        self.waker.register(cx.waker());
        match self.ready.compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed) {
            Ok(_) => std::task::Poll::Ready(()),
            Err(_) => std::task::Poll::Pending,
        }
    }

    fn schedule(&self) {
        self.ready.store(true, Ordering::Release);
        self.waker.wake();
    }
}

impl Drop for NeedsData {
    fn drop(&mut self) {
        self.schedule()
    }
}

/// The notifier side of the underlying signal struct, it is meant to be held
/// by the Core side and schedule signals to be received by the Bindings.
#[derive(Default, Debug, Clone)]
pub(crate) struct NeedsDataNotifier {
    inner: Arc<NeedsData>,
}

impl NeedsDataNotifier {
    pub(crate) fn schedule(&self) {
        self.inner.schedule()
    }

    pub(crate) fn watcher(&self) -> NeedsDataWatcher {
        NeedsDataWatcher { inner: Arc::downgrade(&self.inner) }
    }
}

/// The receiver side of the underlying signal struct, it is meant to be held
/// by the Bindings side. It is a [`Stream`] of wakeups scheduled by the Core
/// and upon receiving those wakeups, Bindings should perform any blocked
/// work.
#[derive(Debug, Clone)]
pub(crate) struct NeedsDataWatcher {
    inner: Weak<NeedsData>,
}

impl Stream for NeedsDataWatcher {
    type Item = ();

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match this.inner.upgrade() {
            None => std::task::Poll::Ready(None),
            Some(needs_data) => {
                std::task::Poll::Ready(Some(std::task::ready!(needs_data.poll_ready(cx))))
            }
        }
    }
}

pub(crate) trait RemoveResourceResultExt<T> {
    fn into_future(self) -> impl Future<Output = T>;
}

impl<T, F> RemoveResourceResultExt<T> for RemoveResourceResult<T, F>
where
    F: Future<Output = T>,
{
    fn into_future(self) -> impl Future<Output = T> {
        match self {
            Self::Removed(r) => futures::future::Either::Left(futures::future::ready(r)),
            Self::Deferred(d) => futures::future::Either::Right(d),
        }
    }
}

/// Provides the common pattern in FIDL APIs to not report empty FIDL tables by
/// returning `None` if `t` is equal to the default value for `T`.
#[inline]
pub(crate) fn some_if_not_default<T: Default + PartialEq>(t: T) -> Option<T> {
    (t != T::default()).then_some(t)
}

/// A core type which can be fallibly converted from the FIDL type `F`.
///
/// For all `C: TryFromFidl<F>`, we provide a blanket impl of
/// [`F: TryIntoCore<C>`].
///
/// [`F: TryIntoCore<C>`]: TryIntoCore
pub(crate) trait TryFromFidl<F>: Sized {
    /// The type of error returned from [`try_from_fidl`].
    ///
    /// [`try_from_fidl`]: TryFromFidl::try_from_fidl
    type Error;

    /// Attempt to convert from `fidl` into an instance of `Self`.
    fn try_from_fidl(fidl: F) -> Result<Self, Self::Error>;
}

/// A core type which can be fallibly converted to the FIDL type `F`.
pub(crate) trait TryIntoFidl<F>: Sized {
    /// The type of error returned from [`try_into_fidl`].
    ///
    /// [`try_into_fidl`]: TryIntoFidl::try_into_fidl
    type Error;

    /// Attempt to convert `self` into an instance of `F`.
    fn try_into_fidl(self) -> Result<F, Self::Error>;
}

/// A core type which can be infallibly converted into the FIDL type `F`.
///
/// `IntoFidl<F>` extends [`TryIntoFidl<F, Error = Never>`], and provides the
/// infallible conversion method [`into_fidl`].
///
/// [`TryIntoFidl<F, Error = Never>`]: TryIntoFidl
/// [`into_fidl`]: IntoFidl::into_fidl
pub(crate) trait IntoFidl<F> {
    /// Infallibly convert `self` into an instance of `F`.
    fn into_fidl(self) -> F;
}

impl<C: TryIntoFidl<F, Error = Never>, F> IntoFidl<F> for C {
    fn into_fidl(self) -> F {
        match self.try_into_fidl() {
            Ok(f) => f,
        }
    }
}

/// Adapts a type that implements [`Extend<T>`] to accept items implementing
/// [`IntoFidl<T>`].
///
/// Items are converted using [`IntoFidl::into_fidl`] before being passed to the
/// underlying extender.
pub(crate) struct IntoFidlExtender<E, T> {
    extender: E,
    _target: PhantomData<T>,
}

impl<E, T> IntoFidlExtender<E, T> {
    pub fn new(extender: E) -> Self {
        Self { extender, _target: PhantomData }
    }

    pub fn into_inner(self) -> E {
        let Self { extender, .. } = self;
        extender
    }
}

impl<E, T, IF> Extend<IF> for IntoFidlExtender<E, T>
where
    E: Extend<T>,
    IF: IntoFidl<T>,
{
    fn extend<I: IntoIterator<Item = IF>>(&mut self, iter: I) {
        let Self { extender, .. } = self;
        extender.extend(iter.into_iter().map(IntoFidl::into_fidl))
    }
}

/// A FIDL type which can be fallibly converted into the core type `C`.
///
/// `TryIntoCore<C>` is automatically implemented for all `F` where
/// [`C: TryFromFidl<F>`].
///
/// [`C: TryFromFidl<F>`]: TryFromFidl
pub(crate) trait TryIntoCore<C>: Sized {
    /// The error returned on conversion failure.
    type Error;

    /// Attempt to convert from `self` into an instance of `C`.
    ///
    /// This is equivalent to [`C::try_from_fidl(self)`].
    ///
    /// [`C::try_from_fidl(self)`]: TryFromFidl::try_from_fidl
    fn try_into_core(self) -> Result<C, Self::Error>;
}

impl<F, C: TryFromFidl<F>> TryIntoCore<C> for F {
    type Error = C::Error;
    fn try_into_core(self) -> Result<C, Self::Error> {
        C::try_from_fidl(self)
    }
}

/// A FIDL type which can be infallibly converted into the core type `C`.
///
/// `IntoCore<C>` extends [`TryIntoCore<C>`] where `<C as TryFromFidl<_>>::Error
/// = Never`, and provides the infallible conversion method [`into_core`].
///
/// [`TryIntoCore<C>`]: TryIntoCore
/// [`into_core`]: IntoCore::into_core
pub(crate) trait IntoCore<C> {
    /// Infallibly convert `self` into an instance of `C`.
    fn into_core(self) -> C;
}

impl<F, C: TryFromFidl<F, Error = Never>> IntoCore<C> for F {
    fn into_core(self) -> C {
        match self.try_into_core() {
            Ok(c) => c,
        }
    }
}

impl<T> TryIntoFidl<T> for Never {
    type Error = Never;

    fn try_into_fidl(self) -> Result<T, Never> {
        match self {}
    }
}

impl TryIntoFidl<fidl_net_stack::Error> for SubnetError {
    type Error = Never;

    fn try_into_fidl(self) -> Result<fidl_net_stack::Error, Never> {
        Ok(fidl_net_stack::Error::InvalidArgs)
    }
}

impl TryIntoFidl<fidl_net_stack::Error> for AddrSubnetError {
    type Error = Never;

    fn try_into_fidl(self) -> Result<fidl_net_stack::Error, Never> {
        Ok(fidl_net_stack::Error::InvalidArgs)
    }
}

impl TryIntoFidl<fidl_net_stack::Error> for ExistsError {
    type Error = Never;

    fn try_into_fidl(self) -> Result<fidl_net_stack::Error, Never> {
        Ok(fidl_net_stack::Error::AlreadyExists)
    }
}

impl TryIntoFidl<fidl_net_stack::Error> for NotFoundError {
    type Error = Never;

    fn try_into_fidl(self) -> Result<fidl_net_stack::Error, Never> {
        Ok(fidl_net_stack::Error::NotFound)
    }
}

impl TryIntoFidl<fidl_net_stack::Error> for AddRouteError {
    type Error = Never;

    fn try_into_fidl(self) -> Result<fidl_net_stack::Error, Never> {
        match self {
            AddRouteError::AlreadyExists => Ok(fidl_net_stack::Error::AlreadyExists),
            AddRouteError::GatewayNotNeighbor => Ok(fidl_net_stack::Error::BadState),
        }
    }
}

impl TryFromFidl<fidl_net::IpAddress> for IpAddr {
    type Error = Never;

    fn try_from_fidl(addr: fidl_net::IpAddress) -> Result<IpAddr, Never> {
        match addr {
            fidl_net::IpAddress::Ipv4(v4) => Ok(IpAddr::V4(v4.into_core())),
            fidl_net::IpAddress::Ipv6(v6) => Ok(IpAddr::V6(v6.into_core())),
        }
    }
}

impl TryIntoFidl<fidl_net::IpAddress> for IpAddr {
    type Error = Never;

    fn try_into_fidl(self) -> Result<fidl_net::IpAddress, Never> {
        match self {
            IpAddr::V4(addr) => Ok(fidl_net::IpAddress::Ipv4(addr.into_fidl())),
            IpAddr::V6(addr) => Ok(fidl_net::IpAddress::Ipv6(addr.into_fidl())),
        }
    }
}

impl TryFromFidl<fidl_net::Ipv4Address> for Ipv4Addr {
    type Error = Never;

    fn try_from_fidl(addr: fidl_net::Ipv4Address) -> Result<Ipv4Addr, Never> {
        Ok(addr.addr.into())
    }
}

impl TryIntoFidl<fidl_net::Ipv4Address> for Ipv4Addr {
    type Error = Never;

    fn try_into_fidl(self) -> Result<fidl_net::Ipv4Address, Never> {
        Ok(fidl_net::Ipv4Address { addr: self.ipv4_bytes() })
    }
}

impl TryFromFidl<fidl_net::Ipv6Address> for Ipv6Addr {
    type Error = Never;

    fn try_from_fidl(addr: fidl_net::Ipv6Address) -> Result<Ipv6Addr, Never> {
        Ok(addr.addr.into())
    }
}

impl TryIntoFidl<fidl_net::Ipv6Address> for Ipv6Addr {
    type Error = Never;

    fn try_into_fidl(self) -> Result<fidl_net::Ipv6Address, Never> {
        Ok(fidl_net::Ipv6Address { addr: self.ipv6_bytes() })
    }
}

impl TryFromFidl<fidl_net::MacAddress> for Mac {
    type Error = Never;

    fn try_from_fidl(mac: fidl_net::MacAddress) -> Result<Mac, Never> {
        Ok(Mac::new(mac.octets))
    }
}

impl TryIntoFidl<fidl_net::MacAddress> for Mac {
    type Error = Never;

    fn try_into_fidl(self) -> Result<fidl_net::MacAddress, Never> {
        Ok(fidl_net::MacAddress { octets: self.bytes() })
    }
}

/// An error indicating that an address was a member of the wrong class (for
/// example, a unicast address used where a multicast address is required).
#[derive(Debug)]
pub(crate) struct AddrClassError;

// TODO(joshlf): Introduce a separate variant to `fidl_net_stack::Error` for
// `AddrClassError`?
impl TryIntoFidl<fidl_net_stack::Error> for AddrClassError {
    type Error = Never;

    fn try_into_fidl(self) -> Result<fidl_net_stack::Error, Never> {
        Ok(fidl_net_stack::Error::InvalidArgs)
    }
}

impl TryFromFidl<fidl_net::IpAddress> for SpecifiedAddr<IpAddr> {
    type Error = AddrClassError;

    fn try_from_fidl(fidl: fidl_net::IpAddress) -> Result<SpecifiedAddr<IpAddr>, AddrClassError> {
        SpecifiedAddr::new(fidl.into_core()).ok_or(AddrClassError)
    }
}

impl TryIntoFidl<fidl_net::IpAddress> for SpecifiedAddr<IpAddr> {
    type Error = Never;

    fn try_into_fidl(self) -> Result<fidl_net::IpAddress, Never> {
        Ok(self.get().into_fidl())
    }
}

impl TryFromFidl<fidl_net::Subnet> for AddrSubnetEither {
    type Error = AddrSubnetError;

    fn try_from_fidl(fidl: fidl_net::Subnet) -> Result<AddrSubnetEither, AddrSubnetError> {
        AddrSubnetEither::new(fidl.addr.into_core(), fidl.prefix_len)
    }
}

impl TryIntoFidl<fidl_net::Subnet> for AddrSubnetEither {
    type Error = Never;

    fn try_into_fidl(self) -> Result<fidl_net::Subnet, Never> {
        let (addr, prefix) = self.addr_prefix();
        Ok(fidl_net::Subnet { addr: addr.into_fidl(), prefix_len: prefix })
    }
}

impl TryFromFidl<fidl_net::Subnet> for SubnetEither {
    type Error = SubnetError;

    fn try_from_fidl(fidl: fidl_net::Subnet) -> Result<SubnetEither, SubnetError> {
        SubnetEither::new(fidl.addr.into_core(), fidl.prefix_len)
    }
}

impl TryIntoFidl<fidl_net::Subnet> for SubnetEither {
    type Error = Never;

    fn try_into_fidl(self) -> Result<fidl_net::Subnet, Never> {
        let (net, prefix) = self.net_prefix();
        Ok(fidl_net::Subnet { addr: net.into_fidl(), prefix_len: prefix })
    }
}

impl TryFromFidl<fposix_socket::OptionalUint8> for Option<u8> {
    type Error = Never;

    fn try_from_fidl(fidl: fposix_socket::OptionalUint8) -> Result<Self, Self::Error> {
        Ok(match fidl {
            fposix_socket::OptionalUint8::Unset(fposix_socket::Empty) => None,
            fposix_socket::OptionalUint8::Value(u) => Some(u),
        })
    }
}

impl TryIntoFidl<fposix_socket::OptionalUint8> for Option<u8> {
    type Error = Never;

    fn try_into_fidl(self) -> Result<fposix_socket::OptionalUint8, Self::Error> {
        Ok(self
            .map(fposix_socket::OptionalUint8::Value)
            .unwrap_or(fposix_socket::OptionalUint8::Unset(fposix_socket::Empty)))
    }
}

impl TryIntoFidl<fnet_interfaces::AddressAssignmentState> for netstack3_core::ip::IpAddressState {
    type Error = Never;

    fn try_into_fidl(self) -> Result<fnet_interfaces::AddressAssignmentState, Never> {
        match self {
            netstack3_core::ip::IpAddressState::Unavailable => {
                Ok(fnet_interfaces::AddressAssignmentState::Unavailable)
            }
            netstack3_core::ip::IpAddressState::Assigned => {
                Ok(fnet_interfaces::AddressAssignmentState::Assigned)
            }
            netstack3_core::ip::IpAddressState::Tentative => {
                Ok(fnet_interfaces::AddressAssignmentState::Tentative)
            }
        }
    }
}

impl<A: IpAddress> TryIntoFidl<<A::Version as IpSockAddrExt>::SocketAddress>
    for (Option<SpecifiedAddr<A>>, NonZeroU16)
where
    A::Version: IpSockAddrExt,
{
    type Error = Never;

    fn try_into_fidl(self) -> Result<<A::Version as IpSockAddrExt>::SocketAddress, Self::Error> {
        let (addr, port) = self;
        Ok(SockAddr::new(addr.map(|a| ZonedAddr::Unzoned(a)), port.get()))
    }
}

impl TryIntoFidl<fposix_socket::OptionalUint32> for Option<u32> {
    type Error = Never;

    fn try_into_fidl(self) -> Result<fposix_socket::OptionalUint32, Self::Error> {
        Ok(match self {
            Some(value) => fposix_socket::OptionalUint32::Value(value),
            None => fposix_socket::OptionalUint32::Unset(fposix_socket::Empty),
        })
    }
}

impl TryFromFidl<fposix_socket::OptionalUint32> for Option<u32> {
    type Error = Never;

    fn try_from_fidl(fidl: fposix_socket::OptionalUint32) -> Result<Self, Self::Error> {
        Ok(match fidl {
            fposix_socket::OptionalUint32::Value(value) => Some(value),
            fposix_socket::OptionalUint32::Unset(fposix_socket::Empty) => None,
        })
    }
}

#[derive(Debug, Error)]
#[error("wrong IP version")]
pub(crate) struct WrongIpVersionError;

impl IntoErrno for WrongIpVersionError {
    fn to_errno(&self) -> fposix::Errno {
        let WrongIpVersionError = self;
        fposix::Errno::Enoprotoopt
    }
}

#[derive(Debug, Error)]
pub(crate) enum MulticastMembershipConversionError {
    #[error("address is not multicast")]
    AddrNotMulticast,
    #[error(transparent)]
    WrongIpVersion(#[from] WrongIpVersionError),
}

impl<I: Ip> GenericOverIp<I> for MulticastMembershipConversionError {
    type Type = Self;
}

impl IntoErrno for MulticastMembershipConversionError {
    fn to_errno(&self) -> fposix::Errno {
        match self {
            Self::AddrNotMulticast => fposix::Errno::Einval,
            Self::WrongIpVersion(e) => e.to_errno(),
        }
    }
}

impl<A: IpAddress> TryFromFidl<fposix_socket::IpMulticastMembership>
    for (MulticastAddr<A>, Option<MulticastInterfaceSelector<A, BindingId>>)
{
    type Error = MulticastMembershipConversionError;

    fn try_from_fidl(fidl: fposix_socket::IpMulticastMembership) -> Result<Self, Self::Error> {
        <A::Version as Ip>::map_ip_out(
            fidl,
            |fidl| {
                let fposix_socket::IpMulticastMembership { iface, local_addr, mcast_addr } = fidl;
                let mcast_addr = MulticastAddr::new(mcast_addr.into_core())
                    .ok_or(Self::Error::AddrNotMulticast)?;
                // Match Linux behavior by ignoring the address if an interface
                // identifier is provided.
                let selector = BindingId::new(iface)
                    .map(MulticastInterfaceSelector::Interface)
                    .or_else(|| {
                        SpecifiedAddr::new(local_addr.into_core())
                            .map(MulticastInterfaceSelector::LocalAddress)
                    });
                Ok((mcast_addr, selector))
            },
            |_fidl| Err(Self::Error::WrongIpVersion(WrongIpVersionError)),
        )
    }
}

impl<A: IpAddress> TryFromFidl<fposix_socket::Ipv6MulticastMembership>
    for (MulticastAddr<A>, Option<MulticastInterfaceSelector<A, BindingId>>)
{
    type Error = MulticastMembershipConversionError;

    fn try_from_fidl(fidl: fposix_socket::Ipv6MulticastMembership) -> Result<Self, Self::Error> {
        <A::Version as Ip>::map_ip_out(
            fidl,
            |_fidl| Err(Self::Error::WrongIpVersion(WrongIpVersionError)),
            |fidl| {
                let fposix_socket::Ipv6MulticastMembership { iface, mcast_addr } = fidl;
                let mcast_addr = MulticastAddr::new(mcast_addr.into_core())
                    .ok_or(Self::Error::AddrNotMulticast)?;
                let selector = BindingId::new(iface).map(MulticastInterfaceSelector::Interface);
                Ok((mcast_addr, selector))
            },
        )
    }
}

/// Provides a stateful context for operations that require state-keeping to be
/// completed.
///
/// `ConversionContext` is used by conversion functions in
/// [`TryFromFidlWithContext`] and [`TryIntoFidlWithContext`].
pub(crate) trait ConversionContext {
    /// Converts a binding identifier (exposed in FIDL as `u64`) to a core
    /// identifier `DeviceId`.
    ///
    /// Returns `None` if there is no core mapping equivalent for `binding_id`.
    fn get_core_id(&self, binding_id: BindingId) -> Option<DeviceId<BindingsCtx>>;

    /// Converts a core identifier `DeviceId` to a FIDL-compatible [`BindingId`].
    fn get_binding_id(&self, core_id: DeviceId<BindingsCtx>) -> BindingId;
}

/// A core type which can be fallibly converted from the FIDL type `F` given a
/// context that implements [`ConversionContext`].
///
/// For all `C: TryFromFidlWithContext<F>`, we provide a blanket impl of
/// [`F: TryIntoCoreWithContext<C>`].
///
/// [`F: TryIntoCoreWithContext<C>`]: TryIntoCoreWithContext
pub(crate) trait TryFromFidlWithContext<F>: Sized {
    /// The type of error returned from [`try_from_fidl_with_ctx`].
    ///
    /// [`try_from_fidl_with_ctx`]: TryFromFidlWithContext::try_from_fidl_with_ctx
    type Error;

    /// Attempt to convert from `fidl` into an instance of `Self`.
    fn try_from_fidl_with_ctx<C: ConversionContext>(ctx: &C, fidl: F) -> Result<Self, Self::Error>;
}

/// A core type which can be fallibly converted to the FIDL type `F` given a
/// context that implements [`ConversionContext`].
pub(crate) trait TryIntoFidlWithContext<F>: Sized {
    /// The type of error returned from [`try_into_fidl_with_ctx`].
    ///
    /// [`try_into_fidl_with_ctx`]: TryIntoFidlWithContext::try_into_fidl_with_ctx
    type Error;

    /// Attempt to convert from `self` into an instance of `F`.
    fn try_into_fidl_with_ctx<C: ConversionContext>(self, ctx: &C) -> Result<F, Self::Error>;
}

/// A core type which can be infallibly converted into the FIDL type `F` given a
/// [`ConversionContext`].
pub(crate) trait IntoFidlWithContext<F> {
    /// Infallibly convert `self` into an instance of `F`.
    fn into_fidl_with_ctx<X: ConversionContext>(self, ctx: &X) -> F;
}

impl<C: TryIntoFidlWithContext<F, Error = Never>, F> IntoFidlWithContext<F> for C {
    fn into_fidl_with_ctx<X: ConversionContext>(self, ctx: &X) -> F {
        match self.try_into_fidl_with_ctx(ctx) {
            Ok(f) => f,
        }
    }
}

/// A FIDL type which can be fallibly converted into the core type `C` given a
/// context that implements [`ConversionContext`].
///
/// `TryIntoCoreWithContext<C>` is automatically implemented for all `F` where
/// [`C: TryFromFidlWithContext<F>`].
///
/// [`C: TryFromFidlWithContext<F>`]: TryFromFidlWithContext
pub(crate) trait TryIntoCoreWithContext<C>: Sized {
    /// The type of error returned from [`try_into_core_with_ctx`].
    ///
    /// [`try_into_core_with_ctx`]: TryIntoCoreWithContext::try_into_core_with_ctx
    type Error;

    /// Attempt to convert from `self` into an instance of `C`.
    fn try_into_core_with_ctx<X: ConversionContext>(self, ctx: &X) -> Result<C, Self::Error>;
}

impl<F, C: TryFromFidlWithContext<F>> TryIntoCoreWithContext<C> for F {
    type Error = C::Error;

    fn try_into_core_with_ctx<X: ConversionContext>(self, ctx: &X) -> Result<C, Self::Error> {
        C::try_from_fidl_with_ctx(ctx, self)
    }
}

#[derive(Debug, PartialEq, Error)]
#[error("device not found")]
pub(crate) struct DeviceNotFoundError;

#[derive(Debug, PartialEq, Error)]
pub(crate) enum SocketAddressError {
    #[error(transparent)]
    Device(DeviceNotFoundError),
}

impl IntoErrno for SocketAddressError {
    fn to_errno(&self) -> fposix::Errno {
        match self {
            SocketAddressError::Device(d) => d.to_errno(),
        }
    }
}

impl<A: IpAddress, D> TryFromFidlWithContext<<A::Version as IpSockAddrExt>::SocketAddress>
    for (Option<ZonedAddr<SpecifiedAddr<A>, D>>, u16)
where
    A::Version: IpSockAddrExt,
    D: TryFromFidlWithContext<NonZeroU64, Error = DeviceNotFoundError>,
{
    type Error = SocketAddressError;

    fn try_from_fidl_with_ctx<C: ConversionContext>(
        ctx: &C,
        fidl: <A::Version as IpSockAddrExt>::SocketAddress,
    ) -> Result<Self, Self::Error> {
        let port = fidl.port();
        let specified = match fidl.get_specified_addr() {
            Some(addr) => addr,
            None => return Ok((None, port)),
        };

        let zoned = match fidl.zone() {
            Some(zone) => {
                match AddrAndZone::new(specified, zone) {
                    None => {
                        // For conformance with Linux, allow callers to provide
                        // a scope ID for addresses that don't allow zones.
                        debug!("ignoring zone ({zone:?}) provided for address ({specified})");
                        ZonedAddr::Unzoned(specified)
                    }
                    Some(addr_and_zone) => addr_and_zone
                        .try_map_zone(|zone| {
                            TryFromFidlWithContext::try_from_fidl_with_ctx(ctx, zone)
                                .map_err(SocketAddressError::Device)
                        })
                        .map(|a| ZonedAddr::Zoned(a))?,
                }
            }
            None => ZonedAddr::Unzoned(specified),
        };
        Ok((Some(zoned), port))
    }
}

impl<A: IpAddress, D> TryIntoFidlWithContext<<A::Version as IpSockAddrExt>::SocketAddress>
    for (Option<ZonedAddr<SpecifiedAddr<A>, D>>, u16)
where
    A::Version: IpSockAddrExt,
    D: TryIntoFidlWithContext<NonZeroU64>,
{
    type Error = D::Error;

    fn try_into_fidl_with_ctx<C: ConversionContext>(
        self,
        ctx: &C,
    ) -> Result<<A::Version as IpSockAddrExt>::SocketAddress, Self::Error> {
        let (addr, port) = self;
        let addr = addr
            .map(|addr| {
                Ok(match addr {
                    ZonedAddr::Unzoned(addr) => ZonedAddr::Unzoned(addr),
                    ZonedAddr::Zoned(z) => z
                        .try_map_zone(|zone| {
                            TryIntoFidlWithContext::try_into_fidl_with_ctx(zone, ctx)
                        })?
                        .into(),
                })
            })
            .transpose()?;
        Ok(SockAddr::new(addr, port))
    }
}

impl<A: IpAddress, D> TryIntoFidlWithContext<<A::Version as IpSockAddrExt>::SocketAddress>
    for (Option<core_socket::StrictlyZonedAddr<A, SpecifiedAddr<A>, D>>, u16)
where
    A::Version: IpSockAddrExt,
    D: TryIntoFidlWithContext<NonZeroU64>,
{
    type Error = D::Error;

    fn try_into_fidl_with_ctx<C: ConversionContext>(
        self,
        ctx: &C,
    ) -> Result<<A::Version as IpSockAddrExt>::SocketAddress, Self::Error> {
        let (addr, port) = self;
        (addr.map(core_socket::StrictlyZonedAddr::into_inner), port).try_into_fidl_with_ctx(ctx)
    }
}

impl<A: IpAddress, D> TryIntoFidlWithContext<<A::Version as IpSockAddrExt>::SocketAddress>
    for (ZonedAddr<SpecifiedAddr<A>, D>, NonZeroU16)
where
    A::Version: IpSockAddrExt,
    D: TryIntoFidlWithContext<NonZeroU64>,
{
    type Error = D::Error;

    fn try_into_fidl_with_ctx<C: ConversionContext>(
        self,
        ctx: &C,
    ) -> Result<<A::Version as IpSockAddrExt>::SocketAddress, Self::Error> {
        let (addr, port) = self;
        (Some(addr), port.get()).try_into_fidl_with_ctx(ctx)
    }
}

impl<A: IpAddress, D> TryIntoFidlWithContext<<A::Version as IpSockAddrExt>::SocketAddress>
    for (Option<ZonedAddr<SpecifiedAddr<A>, D>>, NonZeroU16)
where
    A::Version: IpSockAddrExt,
    D: TryIntoFidlWithContext<NonZeroU64>,
{
    type Error = D::Error;

    fn try_into_fidl_with_ctx<C: ConversionContext>(
        self,
        ctx: &C,
    ) -> Result<<A::Version as IpSockAddrExt>::SocketAddress, Self::Error> {
        let (addr, port) = self;
        (addr, port.get()).try_into_fidl_with_ctx(ctx)
    }
}

impl<A, D1, D2> TryFromFidlWithContext<MulticastMembershipInterfaceSelector<A, D1>>
    for MulticastMembershipInterfaceSelector<A, D2>
where
    A: IpAddress,
    D2: TryFromFidlWithContext<D1>,
{
    type Error = D2::Error;

    fn try_from_fidl_with_ctx<C: ConversionContext>(
        ctx: &C,
        selector: MulticastMembershipInterfaceSelector<A, D1>,
    ) -> Result<Self, Self::Error> {
        use MulticastMembershipInterfaceSelector::*;
        Ok(match selector {
            Specified(MulticastInterfaceSelector::Interface(id)) => {
                Specified(MulticastInterfaceSelector::Interface(id.try_into_core_with_ctx(ctx)?))
            }
            Specified(MulticastInterfaceSelector::LocalAddress(addr)) => {
                Specified(MulticastInterfaceSelector::LocalAddress(addr))
            }
            AnyInterfaceWithRoute => AnyInterfaceWithRoute,
        })
    }
}

impl TryFromFidlWithContext<BindingId> for DeviceId<BindingsCtx> {
    type Error = DeviceNotFoundError;

    fn try_from_fidl_with_ctx<C: ConversionContext>(
        ctx: &C,
        fidl: BindingId,
    ) -> Result<Self, Self::Error> {
        ctx.get_core_id(fidl).ok_or(DeviceNotFoundError)
    }
}

impl TryIntoFidlWithContext<BindingId> for DeviceId<BindingsCtx> {
    type Error = Never;

    fn try_into_fidl_with_ctx<C: ConversionContext>(self, ctx: &C) -> Result<BindingId, Never> {
        Ok(ctx.get_binding_id(self))
    }
}

impl TryIntoFidlWithContext<BindingId> for WeakDeviceId<BindingsCtx> {
    type Error = DeviceNotFoundError;

    fn try_into_fidl_with_ctx<C: ConversionContext>(
        self,
        ctx: &C,
    ) -> Result<BindingId, DeviceNotFoundError> {
        self.upgrade().map(|d| ctx.get_binding_id(d)).ok_or(DeviceNotFoundError)
    }
}

/// A wrapper type that provides an infallible conversion from a
/// [`WeakDeviceId`] to [`BindingId`].
///
/// By default we don't want to provide this conversion infallibly and hidden
/// behind the conversion traits, so `AllowBindingIdFromWeak` acts as an
/// explicit opt-in for that conversion.
pub(crate) struct AllowBindingIdFromWeak(pub(crate) WeakDeviceId<BindingsCtx>);

impl TryIntoFidlWithContext<BindingId> for AllowBindingIdFromWeak {
    type Error = Never;

    fn try_into_fidl_with_ctx<C: ConversionContext>(self, _ctx: &C) -> Result<BindingId, Never> {
        let Self(weak) = self;
        Ok(weak.bindings_id().id)
    }
}

impl IntoErrno for DeviceNotFoundError {
    fn to_errno(&self) -> fposix::Errno {
        fposix::Errno::Enodev
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum ForwardingConversionError {
    DeviceNotFound,
    TypeMismatch,
    Subnet(SubnetError),
    AddrClassError,
}

impl From<DeviceNotFoundError> for ForwardingConversionError {
    fn from(_: DeviceNotFoundError) -> Self {
        ForwardingConversionError::DeviceNotFound
    }
}

impl From<SubnetError> for ForwardingConversionError {
    fn from(err: SubnetError) -> Self {
        ForwardingConversionError::Subnet(err)
    }
}

impl From<AddrClassError> for ForwardingConversionError {
    fn from(_: AddrClassError) -> Self {
        ForwardingConversionError::AddrClassError
    }
}

impl From<ForwardingConversionError> for fidl_net_stack::Error {
    fn from(fwd_error: ForwardingConversionError) -> Self {
        match fwd_error {
            ForwardingConversionError::DeviceNotFound => fidl_net_stack::Error::NotFound,
            ForwardingConversionError::TypeMismatch
            | ForwardingConversionError::Subnet(_)
            | ForwardingConversionError::AddrClassError => fidl_net_stack::Error::InvalidArgs,
        }
    }
}

impl TryFromFidlWithContext<fidl_net_stack::ForwardingEntry>
    for AddableEntryEither<Option<DeviceId<BindingsCtx>>>
{
    type Error = ForwardingConversionError;

    fn try_from_fidl_with_ctx<C: ConversionContext>(
        ctx: &C,
        fidl: fidl_net_stack::ForwardingEntry,
    ) -> Result<AddableEntryEither<Option<DeviceId<BindingsCtx>>>, ForwardingConversionError> {
        let fidl_net_stack::ForwardingEntry { subnet, device_id, next_hop, metric } = fidl;
        let subnet = subnet.try_into_core()?;
        let device =
            BindingId::new(device_id).map(|d| d.try_into_core_with_ctx(ctx)).transpose()?;
        let next_hop: Option<SpecifiedAddr<IpAddr>> =
            next_hop.map(|next_hop| (*next_hop).try_into_core()).transpose()?;
        let metric = if metric == UNSET_FORWARDING_ENTRY_METRIC {
            AddableMetric::MetricTracksInterface
        } else {
            AddableMetric::ExplicitMetric(RawMetric(metric))
        };

        Ok(match (subnet, device, next_hop.map(Into::into)) {
            (subnet, device, None) => Self::without_gateway(subnet, device, metric),
            (SubnetEither::V4(subnet), device, Some(IpAddr::V4(gateway))) => {
                AddableEntry::with_gateway(subnet, device, gateway, metric).into()
            }
            (SubnetEither::V6(subnet), device, Some(IpAddr::V6(gateway))) => {
                AddableEntry::with_gateway(subnet, device, gateway, metric).into()
            }
            (SubnetEither::V4(_), _, Some(IpAddr::V6(_)))
            | (SubnetEither::V6(_), _, Some(IpAddr::V4(_))) => {
                return Err(ForwardingConversionError::TypeMismatch);
            }
        })
    }
}

#[derive(Debug, Copy, Clone)]
pub(crate) enum AddableEntryFromRoutesExtError {
    UnknownAction,
    DeviceNotFound,
}

impl<I: Ip> TryFromFidlWithContext<fnet_routes_ext::Route<I>>
    for AddableEntry<I::Addr, DeviceId<BindingsCtx>>
{
    type Error = AddableEntryFromRoutesExtError;

    fn try_from_fidl_with_ctx<C: ConversionContext>(
        ctx: &C,
        fidl: fnet_routes_ext::Route<I>,
    ) -> Result<Self, Self::Error> {
        let fnet_routes_ext::Route {
            destination,
            action,
            properties:
                fnet_routes_ext::RouteProperties {
                    specified_properties: fnet_routes_ext::SpecifiedRouteProperties { metric },
                },
        } = fidl;
        let fnet_routes_ext::RouteTarget { outbound_interface, next_hop } = match action {
            fnet_routes_ext::RouteAction::Unknown => {
                return Err(AddableEntryFromRoutesExtError::UnknownAction);
            }
            fnet_routes_ext::RouteAction::Forward(target) => target,
        };
        let device: DeviceId<BindingsCtx> = BindingId::new(outbound_interface)
            .ok_or(AddableEntryFromRoutesExtError::DeviceNotFound)?
            .try_into_core_with_ctx(ctx)
            .map_err(|DeviceNotFoundError| AddableEntryFromRoutesExtError::DeviceNotFound)?;

        let metric = match metric {
            fnet_routes::SpecifiedMetric::InheritedFromInterface(fnet_routes::Empty) => {
                AddableMetric::MetricTracksInterface
            }
            fnet_routes::SpecifiedMetric::ExplicitMetric(metric) => {
                AddableMetric::ExplicitMetric(RawMetric(metric))
            }
        };

        Ok(AddableEntry {
            subnet: destination,
            device,
            gateway: next_hop,
            metric,
            route_preference: Default::default(),
        })
    }
}

impl TryIntoFidlWithContext<fidl_net_stack::ForwardingEntry>
    for EntryEither<DeviceId<BindingsCtx>>
{
    type Error = Never;

    fn try_into_fidl_with_ctx<C: ConversionContext>(
        self,
        ctx: &C,
    ) -> Result<fidl_net_stack::ForwardingEntry, Never> {
        let (subnet, device, gateway, metric): (
            SubnetEither,
            _,
            Option<IpAddr<SpecifiedAddr<Ipv4Addr>, SpecifiedAddr<Ipv6Addr>>>,
            _,
        ) = match self {
            EntryEither::V4(Entry { subnet, device, gateway, metric, route_preference: _ }) => {
                (subnet.into(), device, gateway.map(|gateway| gateway.into()), metric)
            }
            EntryEither::V6(Entry { subnet, device, gateway, metric, route_preference: _ }) => {
                (subnet.into(), device, gateway.map(|gateway| gateway.into()), metric)
            }
        };
        let RawMetric(metric) = metric.value();
        let device_id: BindingId = device.try_into_fidl_with_ctx(ctx)?;
        let next_hop = gateway.map(|next_hop| {
            let next_hop: SpecifiedAddr<IpAddr> = next_hop.into();
            Box::new(next_hop.into_fidl())
        });
        Ok(fidl_net_stack::ForwardingEntry {
            subnet: subnet.into_fidl(),
            device_id: device_id.get(),
            next_hop,
            metric: metric,
        })
    }
}

impl<I: Ip> TryIntoFidlWithContext<fnet_routes_ext::Route<I>>
    for Entry<I::Addr, DeviceId<BindingsCtx>>
{
    type Error = Never;

    fn try_into_fidl_with_ctx<C: ConversionContext>(
        self,
        ctx: &C,
    ) -> Result<fnet_routes_ext::Route<I>, Never> {
        let Entry { subnet, device, gateway, metric, route_preference: _ } = self;

        let device_id: BindingId = device.try_into_fidl_with_ctx(ctx)?;

        Ok(fnet_routes_ext::Route::<I>::new_forward(
            subnet,
            device_id.get(),
            gateway,
            match metric {
                Metric::ExplicitMetric(RawMetric(metric)) => {
                    fnet_routes::SpecifiedMetric::ExplicitMetric(metric)
                }
                Metric::MetricTracksInterface(_) => {
                    fnet_routes::SpecifiedMetric::InheritedFromInterface(fnet_routes::Empty)
                }
            },
        ))
    }
}

pub(crate) struct EntryAndTableId<I: Ip> {
    pub(crate) entry: Entry<I::Addr, DeviceId<BindingsCtx>>,
    pub(crate) table_id: routes::TableId<I>,
}

impl<I: Ip> TryIntoFidlWithContext<fnet_routes_ext::InstalledRoute<I>> for EntryAndTableId<I> {
    type Error = Never;

    fn try_into_fidl_with_ctx<C: ConversionContext>(
        self,
        ctx: &C,
    ) -> Result<fnet_routes_ext::InstalledRoute<I>, Never> {
        let EntryAndTableId {
            entry: Entry { subnet, device, gateway, metric, route_preference: _ },
            table_id,
        } = self;
        let device: BindingId = device.try_into_fidl_with_ctx(ctx)?;
        let specified_metric = match metric {
            Metric::ExplicitMetric(value) => {
                fnet_routes::SpecifiedMetric::ExplicitMetric(value.into())
            }
            Metric::MetricTracksInterface(_value) => {
                fnet_routes::SpecifiedMetric::InheritedFromInterface(fnet_routes::Empty)
            }
        };
        Ok(fnet_routes_ext::InstalledRoute {
            route: fnet_routes_ext::Route {
                destination: subnet,
                action: fnet_routes_ext::RouteAction::Forward(fnet_routes_ext::RouteTarget {
                    outbound_interface: device.get(),
                    next_hop: gateway,
                }),
                properties: fnet_routes_ext::RouteProperties {
                    specified_properties: fnet_routes_ext::SpecifiedRouteProperties {
                        metric: specified_metric,
                    },
                },
            },
            effective_properties: fnet_routes_ext::EffectiveRouteProperties {
                metric: metric.value().into(),
            },
            table_id: table_id.into(),
        })
    }
}

impl TryFromFidl<fnet_ext::Marks> for netstack3_core::ip::Marks {
    type Error = Never;

    fn try_from_fidl(marks: fnet_ext::Marks) -> Result<Self, Self::Error> {
        Ok(Self::new(marks.into_iter().map(|(domain, mark)| (domain.into_core(), mark))))
    }
}

impl TryIntoFidl<fidl_net::Marks> for netstack3_core::ip::Marks {
    type Error = Never;

    fn try_into_fidl(self) -> Result<fidl_net::Marks, Self::Error> {
        let netstack3_core::ip::Mark(mark_1) = *self.get(MarkDomain::Mark1);
        let netstack3_core::ip::Mark(mark_2) = *self.get(MarkDomain::Mark2);

        Ok(fidl_net::Marks { mark_1, mark_2, __source_breaking: fidl::marker::SourceBreaking })
    }
}

impl TryIntoFidl<fnet_ext::Marks> for netstack3_core::ip::Marks {
    type Error = Never;

    fn try_into_fidl(self) -> Result<fnet_ext::Marks, Self::Error> {
        let netstack3_core::ip::Mark(mark_1) = *self.get(MarkDomain::Mark1);
        let netstack3_core::ip::Mark(mark_2) = *self.get(MarkDomain::Mark2);

        Ok(fnet_ext::Marks { mark_1, mark_2 })
    }
}

impl TryFromFidl<fnet_routes_ext::ResolveOptions> for netstack3_core::routes::RouteResolveOptions {
    type Error = Never;

    fn try_from_fidl(options: fnet_routes_ext::ResolveOptions) -> Result<Self, Self::Error> {
        let fnet_routes_ext::ResolveOptions { marks } = options;
        Ok(Self { marks: marks.into_core() })
    }
}

#[derive(Debug)]
pub(crate) struct IllegalZeroValueError;

#[derive(Debug)]
pub(crate) enum IllegalNonPositiveValueError {
    Zero,
    Negative,
}

impl From<IllegalZeroValueError> for IllegalNonPositiveValueError {
    fn from(_: IllegalZeroValueError) -> Self {
        Self::Zero
    }
}

impl IntoFidl<fnet_interfaces_admin::ControlSetConfigurationError>
    for IllegalNonPositiveValueError
{
    fn into_fidl(self) -> fnet_interfaces_admin::ControlSetConfigurationError {
        match self {
            IllegalNonPositiveValueError::Zero => {
                fnet_interfaces_admin::ControlSetConfigurationError::IllegalZeroValue
            }
            IllegalNonPositiveValueError::Negative => {
                fnet_interfaces_admin::ControlSetConfigurationError::IllegalNegativeValue
            }
        }
    }
}

impl TryFromFidl<u16> for NonZeroU16 {
    type Error = IllegalZeroValueError;

    fn try_from_fidl(fidl: u16) -> Result<Self, Self::Error> {
        NonZeroU16::new(fidl).ok_or(IllegalZeroValueError)
    }
}

impl TryFromFidl<i64> for NonZeroDuration {
    type Error = IllegalNonPositiveValueError;

    fn try_from_fidl(fidl: i64) -> Result<Self, Self::Error> {
        NonZeroDuration::from_nanos(
            u64::try_from(fidl).map_err(|_| IllegalNonPositiveValueError::Negative)?,
        )
        .ok_or(IllegalNonPositiveValueError::Zero)
    }
}

impl TryFromFidl<fnet_interfaces_admin::NudConfiguration> for NudUserConfigUpdate {
    type Error = IllegalNonPositiveValueError;

    fn try_from_fidl(fidl: fnet_interfaces_admin::NudConfiguration) -> Result<Self, Self::Error> {
        let fnet_interfaces_admin::NudConfiguration {
            max_multicast_solicitations,
            max_unicast_solicitations,
            base_reachable_time,
            retrans_timer,
            __source_breaking,
        } = fidl;
        Ok(NudUserConfigUpdate {
            max_multicast_solicitations: max_multicast_solicitations
                .map(TryIntoCore::try_into_core)
                .transpose()?,
            max_unicast_solicitations: max_unicast_solicitations
                .map(TryIntoCore::try_into_core)
                .transpose()?,
            base_reachable_time: base_reachable_time.map(TryIntoCore::try_into_core).transpose()?,
            retrans_timer: retrans_timer.map(TryIntoCore::try_into_core).transpose()?,
            ..Default::default()
        })
    }
}

pub const fn fidl_mark_domain_to_core(
    fidl: fidl_net::MarkDomain,
) -> netstack3_core::ip::MarkDomain {
    match fidl {
        fidl_net::MarkDomain::Mark1 => netstack3_core::ip::MarkDomain::Mark1,
        fidl_net::MarkDomain::Mark2 => netstack3_core::ip::MarkDomain::Mark2,
    }
}

impl TryFromFidl<fidl_net::MarkDomain> for netstack3_core::ip::MarkDomain {
    type Error = Never;

    fn try_from_fidl(fidl: fidl_net::MarkDomain) -> Result<Self, Self::Error> {
        Ok(fidl_mark_domain_to_core(fidl))
    }
}

impl IntoFidl<fidl_net::MarkDomain> for netstack3_core::ip::MarkDomain {
    fn into_fidl(self) -> fidl_net::MarkDomain {
        match self {
            netstack3_core::ip::MarkDomain::Mark1 => fidl_net::MarkDomain::Mark1,
            netstack3_core::ip::MarkDomain::Mark2 => fidl_net::MarkDomain::Mark2,
        }
    }
}

impl TryFromFidl<fposix_socket::OptionalUint32> for netstack3_core::ip::Mark {
    type Error = Never;

    fn try_from_fidl(fidl: fposix_socket::OptionalUint32) -> Result<Self, Self::Error> {
        Ok(match fidl {
            fposix_socket::OptionalUint32::Value(mark) => netstack3_core::ip::Mark(Some(mark)),
            fposix_socket::OptionalUint32::Unset(_) => netstack3_core::ip::Mark(None),
        })
    }
}

impl IntoFidl<fposix_socket::OptionalUint32> for netstack3_core::ip::Mark {
    fn into_fidl(self) -> fposix_socket::OptionalUint32 {
        match self.0 {
            Some(mark) => fposix_socket::OptionalUint32::Value(mark),
            None => fposix_socket::OptionalUint32::Unset(fposix_socket::Empty),
        }
    }
}

impl IntoFidl<fnet_interfaces_admin::NudConfiguration> for NudUserConfigUpdate {
    fn into_fidl(self) -> fnet_interfaces_admin::NudConfiguration {
        let NudUserConfigUpdate {
            max_unicast_solicitations,
            max_multicast_solicitations,
            base_reachable_time,
            retrans_timer,
        } = self;
        fnet_interfaces_admin::NudConfiguration {
            max_multicast_solicitations: max_multicast_solicitations.map(|c| c.get()),
            max_unicast_solicitations: max_unicast_solicitations.map(|c| c.get()),
            base_reachable_time: base_reachable_time.map(|c| {
                // Even though `as_nanos` returns a `u128`, the value will
                // always fit in an `i64` because it is either set via FIDL
                // (stored as a `zx_duration_t`, i.e. `i64`) or learnt via
                // the Reachable Time field in RA messages which is a 32-bit
                // value in milliseconds.
                c.get().as_nanos().try_into().unwrap()
            }),
            retrans_timer: retrans_timer.map(|c| {
                // Even though `as_nanos` returns a `u128`, the value will
                // always fit in an `i64` because it is either set via FIDL
                // (stored as a `zx_duration_t`, i.e. `i64`) or learnt via
                // the RetransTimer field in RA messages which is a 32-bit
                // value in milliseconds.
                c.get().as_nanos().try_into().unwrap()
            }),
            __source_breaking: fidl::marker::SourceBreaking,
        }
    }
}

/// A helper function to transform a NudUserConfig to a NudUserConfigUpdate with
/// all the fields set so we can maximize reusing FIDL conversion functions.
fn nud_user_config_to_update(c: NudUserConfig) -> NudUserConfigUpdate {
    let NudUserConfig {
        max_multicast_solicitations,
        max_unicast_solicitations,
        base_reachable_time,
        retrans_timer,
    } = c;
    NudUserConfigUpdate {
        max_unicast_solicitations: Some(max_unicast_solicitations),
        max_multicast_solicitations: Some(max_multicast_solicitations),
        base_reachable_time: Some(base_reachable_time),
        retrans_timer: Some(retrans_timer),
    }
}

impl IntoFidl<fnet_interfaces_admin::NudConfiguration> for NudUserConfig {
    fn into_fidl(self) -> fnet_interfaces_admin::NudConfiguration {
        nud_user_config_to_update(self).into_fidl()
    }
}

impl IntoFidl<fnet_interfaces_admin::SlaacConfiguration> for SlaacConfigurationUpdate {
    fn into_fidl(self) -> fnet_interfaces_admin::SlaacConfiguration {
        let SlaacConfigurationUpdate {
            temporary_address_configuration,
            stable_address_configuration: _,
        } = self;
        fnet_interfaces_admin::SlaacConfiguration {
            temporary_address: temporary_address_configuration.map(|config| config.is_enabled()),
            __source_breaking: fidl::marker::SourceBreaking,
        }
    }
}

impl IntoFidl<fnet_interfaces_admin::SlaacConfiguration> for SlaacConfiguration {
    fn into_fidl(self) -> fnet_interfaces_admin::SlaacConfiguration {
        let SlaacConfiguration { stable_address_configuration: _, temporary_address_configuration } =
            self;
        fnet_interfaces_admin::SlaacConfiguration {
            temporary_address: Some(temporary_address_configuration.is_enabled()),
            __source_breaking: fidl::marker::SourceBreaking,
        }
    }
}

impl IntoFidl<fnet_interfaces_admin::RouteDiscoveryConfiguration> for RouteDiscoveryConfiguration {
    fn into_fidl(self) -> fnet_interfaces_admin::RouteDiscoveryConfiguration {
        let RouteDiscoveryConfiguration { allow_default_route } = self;
        fnet_interfaces_admin::RouteDiscoveryConfiguration {
            allow_default_route: Some(allow_default_route),
            __source_breaking: fidl::marker::SourceBreaking,
        }
    }
}

impl IntoFidl<fnet_interfaces_admin::RouteDiscoveryConfiguration>
    for RouteDiscoveryConfigurationUpdate
{
    fn into_fidl(self) -> fnet_interfaces_admin::RouteDiscoveryConfiguration {
        let RouteDiscoveryConfigurationUpdate { allow_default_route } = self;
        fnet_interfaces_admin::RouteDiscoveryConfiguration {
            allow_default_route,
            __source_breaking: fidl::marker::SourceBreaking,
        }
    }
}

impl TryIntoFidl<fnet_interfaces_ext::PreferredLifetimeInfo>
    for PreferredLifetime<zx::MonotonicInstant>
{
    type Error = fnet_interfaces_ext::NotPositiveMonotonicInstantError;

    fn try_into_fidl(self) -> Result<fnet_interfaces_ext::PreferredLifetimeInfo, Self::Error> {
        match self {
            PreferredLifetime::Deprecated => {
                Ok(fnet_interfaces_ext::PreferredLifetimeInfo::Deprecated)
            }
            PreferredLifetime::Preferred(p) => {
                Ok(fnet_interfaces_ext::PreferredLifetimeInfo::PreferredUntil(
                    p.into_zx_time().try_into()?,
                ))
            }
        }
    }
}

impl TryFromFidl<fnet_interfaces_ext::PreferredLifetimeInfo>
    for PreferredLifetime<zx::MonotonicInstant>
{
    type Error = Never;

    fn try_from_fidl(
        fidl: fnet_interfaces_ext::PreferredLifetimeInfo,
    ) -> Result<Self, Self::Error> {
        Ok(match fidl {
            fnet_interfaces_ext::PreferredLifetimeInfo::Deprecated => Self::Deprecated,
            fnet_interfaces_ext::PreferredLifetimeInfo::PreferredUntil(i) => {
                Self::Preferred(Lifetime::from_zx_time(i.into()))
            }
        })
    }
}

impl IntoFidl<fnet_interfaces_admin::MldVersion> for MldConfigMode {
    fn into_fidl(self) -> fnet_interfaces_admin::MldVersion {
        match self {
            Self::V1 => fnet_interfaces_admin::MldVersion::V1,
            Self::V2 => fnet_interfaces_admin::MldVersion::V2,
        }
    }
}

impl IntoFidl<fnet_interfaces_admin::IgmpVersion> for IgmpConfigMode {
    fn into_fidl(self) -> fnet_interfaces_admin::IgmpVersion {
        match self {
            Self::V1 => fnet_interfaces_admin::IgmpVersion::V1,
            Self::V2 => fnet_interfaces_admin::IgmpVersion::V2,
            Self::V3 => fnet_interfaces_admin::IgmpVersion::V3,
        }
    }
}

/// A [`core::fmt::Display`] implementation for a socket operation and a socket
/// address.
pub(crate) struct OpAndAddr {
    pub(crate) op: &'static str,
    pub(crate) addr: Option<fidl_net::SocketAddress>,
}

impl core::fmt::Display for OpAndAddr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> Result<(), core::fmt::Error> {
        let Self { op, addr } = self;
        match addr {
            Some(fidl_net::SocketAddress::Ipv4(addr)) => {
                write!(
                    f,
                    "{op}(addr={}, port={}, zone={})",
                    addr.addr(),
                    addr.port(),
                    addr.zone().map(|z| z.get()).unwrap_or(0)
                )
            }
            Some(fidl_net::SocketAddress::Ipv6(addr)) => {
                write!(
                    f,
                    "{op}(addr={}, port={}, zone={})",
                    addr.addr(),
                    addr.port(),
                    addr.zone().map(|z| z.get()).unwrap_or(0)
                )
            }
            None => write!(f, "{op}(addr=None, port=None, zone=None)"),
        }
    }
}

#[cfg(test)]
pub(crate) mod testutils {
    use crate::bindings::integration_tests::{StackSetupBuilder, TestSetup, TestSetupBuilder};

    use super::*;

    pub(crate) struct FakeConversionContext {
        test_setup: TestSetup,
    }

    impl FakeConversionContext {
        pub(crate) const BINDING_ID1: BindingId = NonZeroU64::new(1).unwrap();
        pub(crate) const BINDING_ID2: BindingId = NonZeroU64::new(2).unwrap();
        pub(crate) const INVALID_BINDING_ID: BindingId = NonZeroU64::new(3).unwrap();

        pub(crate) async fn shutdown(self) {
            let Self { test_setup } = self;
            test_setup.shutdown().await
        }

        pub(crate) async fn new() -> Self {
            // Create a `TestStack` with two interfaces. Loopback implicitly
            // exists, so we only need to actually install one endpoint.
            let mut test_setup = TestSetupBuilder::new()
                .add_endpoint()
                .add_stack(StackSetupBuilder::new().add_endpoint(1, None))
                .build()
                .await;

            let test_stack = test_setup.get_mut(0);
            test_stack.wait_for_interface_online(Self::BINDING_ID1).await;
            test_stack.wait_for_interface_online(Self::BINDING_ID2).await;

            Self { test_setup }
        }
    }

    impl ConversionContext for FakeConversionContext {
        fn get_core_id(&self, binding_id: BindingId) -> Option<DeviceId<BindingsCtx>> {
            self.test_setup.get(0).ctx().bindings_ctx().get_core_id(binding_id)
        }

        fn get_binding_id(&self, core_id: DeviceId<BindingsCtx>) -> BindingId {
            core_id.bindings_id().id
        }
    }
}

#[cfg(test)]
mod tests {
    use fidl_fuchsia_net as fidl_net;
    use fidl_fuchsia_net_ext::IntoExt;

    use net_declare::{net_ip_v4, net_ip_v6};
    use test_case::test_case;

    use crate::bindings::util::testutils::FakeConversionContext;

    use super::*;

    fn create_addr_v4(bytes: [u8; 4]) -> (IpAddr, fidl_net::IpAddress) {
        let core = IpAddr::V4(Ipv4Addr::from(bytes));
        let fidl = fidl_net::IpAddress::Ipv4(fidl_net::Ipv4Address { addr: bytes });
        (core, fidl)
    }

    fn create_addr_v6(bytes: [u8; 16]) -> (IpAddr, fidl_net::IpAddress) {
        let core = IpAddr::V6(Ipv6Addr::from(bytes));
        let fidl = fidl_net::IpAddress::Ipv6(fidl_net::Ipv6Address { addr: bytes });
        (core, fidl)
    }

    fn create_subnet(
        subnet: (IpAddr, fidl_net::IpAddress),
        prefix: u8,
    ) -> (SubnetEither, fidl_net::Subnet) {
        let (core, fidl) = subnet;
        (
            SubnetEither::new(core, prefix).unwrap(),
            fidl_net::Subnet { addr: fidl, prefix_len: prefix },
        )
    }

    fn create_addr_subnet(
        addr: (IpAddr, fidl_net::IpAddress),
        prefix: u8,
    ) -> (AddrSubnetEither, fidl_net::Subnet) {
        let (core, fidl) = addr;
        (
            AddrSubnetEither::new(core, prefix).unwrap(),
            fidl_net::Subnet { addr: fidl, prefix_len: prefix },
        )
    }

    #[test]
    fn addr_v4() {
        let bytes = [192, 168, 0, 1];
        let (core, fidl) = create_addr_v4(bytes);

        assert_eq!(core, fidl.into_core());
        assert_eq!(fidl, core.into_fidl());
    }

    #[test]
    fn addr_v6() {
        let bytes = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let (core, fidl) = create_addr_v6(bytes);

        assert_eq!(core, fidl.into_core());
        assert_eq!(fidl, core.into_fidl());
    }

    #[test]
    fn addr_subnet_v4() {
        let bytes = [192, 168, 0, 1];
        let prefix = 24;
        let (core, fidl) = create_addr_subnet(create_addr_v4(bytes), prefix);

        assert_eq!(fidl, core.into_fidl());
        assert_eq!(core, fidl.try_into_core().unwrap());
    }

    #[test]
    fn addr_subnet_v6() {
        let bytes = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let prefix = 64;
        let (core, fidl) = create_addr_subnet(create_addr_v6(bytes), prefix);

        assert_eq!(fidl, core.into_fidl());
        assert_eq!(core, fidl.try_into_core().unwrap());
    }

    #[test]
    fn subnet_v4() {
        let bytes = [192, 168, 0, 0];
        let prefix = 24;
        let (core, fidl) = create_subnet(create_addr_v4(bytes), prefix);

        assert_eq!(fidl, core.into_fidl());
        assert_eq!(core, fidl.try_into_core().unwrap());
    }

    #[test]
    fn subnet_v6() {
        let bytes = [1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0, 0, 0, 0, 0];
        let prefix = 64;
        let (core, fidl) = create_subnet(create_addr_v6(bytes), prefix);

        assert_eq!(fidl, core.into_fidl());
        assert_eq!(core, fidl.try_into_core().unwrap());
    }

    #[test]
    fn ip_address_state() {
        use fnet_interfaces::AddressAssignmentState;
        use netstack3_core::ip::IpAddressState;
        assert_eq!(IpAddressState::Unavailable.into_fidl(), AddressAssignmentState::Unavailable);
        assert_eq!(IpAddressState::Tentative.into_fidl(), AddressAssignmentState::Tentative);
        assert_eq!(IpAddressState::Assigned.into_fidl(), AddressAssignmentState::Assigned);
    }

    #[fixture::teardown(FakeConversionContext::shutdown)]
    #[test_case(
        fidl_net::Ipv6SocketAddress {
            address: net_ip_v6!("fe80::1").into_ext(),
            port: 8080,
            zone_index: FakeConversionContext::INVALID_BINDING_ID.into(),
        },
        SocketAddressError::Device(DeviceNotFoundError);
        "IPv6 specified invalid zone")]
    #[fuchsia::test(logging = false)]
    async fn sock_addr_into_core_err<A: SockAddr>(addr: A, expected: SocketAddressError)
    where
        (Option<ZonedAddr<SpecifiedAddr<A::AddrType>, DeviceId<BindingsCtx>>>, u16):
            TryFromFidlWithContext<A, Error = SocketAddressError>,
        <A::AddrType as IpAddress>::Version: IpSockAddrExt<SocketAddress = A>,
        DeviceId<BindingsCtx>: TryFromFidlWithContext<NonZeroU64, Error = DeviceNotFoundError>,
    {
        let ctx = FakeConversionContext::new().await;
        let result: Result<(Option<_>, _), _> = addr.try_into_core_with_ctx(&ctx);
        assert_eq!(result.expect_err("should fail"), expected);
        ctx
    }

    /// Placeholder for an ID that should be replaced with the real `DeviceId`
    /// from the `FakeConversionContext`.
    struct ReplaceWithCoreId;

    #[fixture::teardown(FakeConversionContext::shutdown)]
    #[test_case(
        fidl_net::Ipv4SocketAddress {address: net_ip_v4!("192.168.0.0").into_ext(), port: 8080},
        (Some(ZonedAddr::Unzoned(SpecifiedAddr::new(net_ip_v4!("192.168.0.0")).unwrap())), 8080);
        "IPv4 specified")]
    #[test_case(
        fidl_net::Ipv4SocketAddress {address: net_ip_v4!("0.0.0.0").into_ext(), port: 8000},
        (None, 8000);
        "IPv4 unspecified")]
    #[test_case(
        fidl_net::Ipv6SocketAddress {
            address: net_ip_v6!("1:2:3:4::").into_ext(),
            port: 8080,
            zone_index: 0
        },
        (Some(ZonedAddr::Unzoned(SpecifiedAddr::new(net_ip_v6!("1:2:3:4::")).unwrap())), 8080);
        "IPv6 specified no zone")]
    #[test_case(
        fidl_net::Ipv6SocketAddress {
            address: net_ip_v6!("::").into_ext(),
            port: 8080,
            zone_index: 0,
        },
        (None, 8080);
        "IPv6 unspecified")]
    #[test_case(
        fidl_net::Ipv6SocketAddress {
            address: net_ip_v6!("fe80::1").into_ext(),
            port: 8080,
            zone_index: FakeConversionContext::BINDING_ID1.into()
        },
        (Some(
            ZonedAddr::Zoned(AddrAndZone::new(SpecifiedAddr::new(net_ip_v6!("fe80::1")).unwrap(), ReplaceWithCoreId).unwrap())
        ), 8080);
        "IPv6 specified valid zone")]
    #[fuchsia::test(logging = false)]
    async fn sock_addr_conversion_reversible<A: SockAddr + Eq + Clone>(
        addr: A,
        (zoned, port): (Option<ZonedAddr<SpecifiedAddr<A::AddrType>, ReplaceWithCoreId>>, u16),
    ) where
        (Option<ZonedAddr<SpecifiedAddr<A::AddrType>, DeviceId<BindingsCtx>>>, u16):
            TryFromFidlWithContext<A, Error = SocketAddressError> + TryIntoFidlWithContext<A>,
        <(Option<ZonedAddr<SpecifiedAddr<A::AddrType>, DeviceId<BindingsCtx>>>, u16) as
                 TryIntoFidlWithContext<A>>::Error: Debug,
        <A::AddrType as IpAddress>::Version: IpSockAddrExt<SocketAddress = A>,
        DeviceId<BindingsCtx>:
            TryFromFidlWithContext<NonZeroU64, Error = DeviceNotFoundError>,
{
        let ctx = FakeConversionContext::new().await;
        let zoned = zoned.map(|z| match z {
            ZonedAddr::Unzoned(z) => ZonedAddr::Unzoned(z),
            ZonedAddr::Zoned(z) => ZonedAddr::Zoned(z.map_zone(|ReplaceWithCoreId| {
                ctx.get_core_id(FakeConversionContext::BINDING_ID1).unwrap()
            })),
        });

        let result: (Option<ZonedAddr<_, _>>, _) =
            addr.clone().try_into_core_with_ctx(&ctx).expect("into core should succeed");
        assert_eq!(result, (zoned, port));

        let result = result.try_into_fidl_with_ctx(&ctx).expect("reverse should succeed");
        assert_eq!(result, addr);
        ctx
    }

    // Verify that the unnecessary zone IDs are ignored and result in
    // `Unzoned` addresses. This is a regression test for
    // https://fxbug.dev/329694011.
    #[fixture::teardown(FakeConversionContext::shutdown)]
    #[fuchsia::test(logging = false)]
    async fn sock_addr_conversion_with_unnecessary_zone_id() {
        let ctx = FakeConversionContext::new().await;
        const GLOBAL_IPV6_ADDR: Ipv6Addr = net_ip_v6!("a:b:c:d::");
        const PORT: u16 = 8080;
        let fidl_addr = fidl_net::Ipv6SocketAddress {
            address: GLOBAL_IPV6_ADDR.into_ext(),
            port: PORT,
            zone_index: 1,
        };
        let expected_addr: ZonedAddr<_, DeviceId<BindingsCtx>> =
            ZonedAddr::Unzoned(SpecifiedAddr::new(GLOBAL_IPV6_ADDR).unwrap());
        let addr: (Option<ZonedAddr<_, _>>, _) =
            fidl_addr.try_into_core_with_ctx(&ctx).expect("into core should succeed");
        assert_eq!(addr, (Some(expected_addr), PORT));
        ctx
    }

    #[test]
    fn test_unzoned_ip_port_into_fidl() {
        let ip = net_ip_v4!("1.7.2.4");
        let port = 3893;
        assert_eq!(
            (SpecifiedAddr::new(ip), NonZeroU16::new(port).unwrap()).into_fidl(),
            fidl_net::Ipv4SocketAddress { address: ip.into_ext(), port }
        );

        let ip = net_ip_v6!("1:2:3:4:5::");
        assert_eq!(
            (SpecifiedAddr::new(ip), NonZeroU16::new(port).unwrap()).into_fidl(),
            fidl_net::Ipv6SocketAddress { address: ip.into_ext(), port, zone_index: 0 }
        );
    }

    #[fixture::teardown(FakeConversionContext::shutdown)]
    #[fuchsia::test(logging = false)]
    async fn device_id_from_bindings_id() {
        let ctx = FakeConversionContext::new().await;
        let device_id: DeviceId<_> =
            FakeConversionContext::BINDING_ID1.try_into_core_with_ctx(&ctx).unwrap();
        let core = ctx.get_core_id(FakeConversionContext::BINDING_ID1).unwrap();
        assert_eq!(device_id, core);

        assert_eq!(
            FakeConversionContext::INVALID_BINDING_ID.try_into_core_with_ctx(&ctx),
            Err::<DeviceId<_>, _>(DeviceNotFoundError)
        );
        ctx
    }

    #[test]
    fn optional_u8_conversion() {
        let empty = fposix_socket::OptionalUint8::Unset(fposix_socket::Empty);
        let empty_core: Option<u8> = empty.into_core();
        assert_eq!(empty_core, None);
        assert_eq!(empty_core.into_fidl(), empty);

        let value = fposix_socket::OptionalUint8::Value(46);
        let value_core: Option<u8> = value.into_core();
        assert_eq!(value_core, Some(46));
        assert_eq!(value_core.into_fidl(), value);
    }
}
