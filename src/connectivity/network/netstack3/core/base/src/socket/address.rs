// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A collection of types that represent the various parts of socket addresses.

use core::fmt::{self, Debug, Display, Formatter};
use core::marker::PhantomData;
use core::num::NonZeroU16;
use core::ops::Deref;

use derivative::Derivative;
use net_types::ip::{
    GenericOverIp, Ip, IpAddress, Ipv4, Ipv4Addr, Ipv4SourceAddr, Ipv6Addr, Ipv6SourceAddr,
};
use net_types::{
    MulticastAddr, NonMappedAddr, ScopeableAddress, SpecifiedAddr, UnicastAddr, Witness, ZonedAddr,
};

use crate::socket::base::{
    AddrVec, DualStackIpExt, EitherStack, SocketIpAddrExt as _, SocketIpExt, SocketMapAddrSpec,
};

/// A [`ZonedAddr`] whose address is `Zoned` iff a zone is required.
///
/// Any address whose scope can have a zone, will be the `Zoned` variant. The
/// one exception is the loopback address, which is represented as `Unzoned`.
/// This is because the loopback address is allowed to have, but does not
/// require having, a zone.
///
/// # Type Parameters
///
/// A: The base [`IpAddress`] type.
/// W: The [`Witness`] types of the `A`.
/// Z: The zone of `A`.
#[derive(Copy, Clone, Eq, Hash, PartialEq)]
pub struct StrictlyZonedAddr<A, W, Z> {
    addr: ZonedAddr<W, Z>,
    marker: PhantomData<A>,
}

impl<A: Debug, W: Debug, Z: Debug> Debug for StrictlyZonedAddr<A, W, Z> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let StrictlyZonedAddr { addr, marker: PhantomData } = self;
        write!(f, "{:?}", addr)
    }
}

impl<A: IpAddress, W: Witness<A> + ScopeableAddress + Copy, Z> StrictlyZonedAddr<A, W, Z> {
    /// Convert self into the inner [`ZonedAddr`]
    pub fn into_inner(self) -> ZonedAddr<W, Z> {
        let StrictlyZonedAddr { addr, marker: PhantomData } = self;
        addr
    }

    /// Convert self into the inner [`ZonedAddr`] while discarding the witness.
    pub fn into_inner_without_witness(self) -> ZonedAddr<A, Z> {
        self.into_inner().map_addr(|addr| addr.into_addr())
    }

    /// Creates from a specified IP address and an optional zone.
    ///
    /// If `addr` requires a zone, then `get_zone` will be called to provide
    /// the zone.
    ///
    /// # Panics
    /// This method panics if the `addr` wants a zone and `get_zone` will panic
    /// when called.
    pub fn new_with_zone(addr: W, get_zone: impl FnOnce() -> Z) -> Self {
        if let Some(addr_and_zone) = addr.try_into_null_zoned() {
            StrictlyZonedAddr {
                addr: ZonedAddr::Zoned(addr_and_zone.map_zone(move |()| get_zone())),
                marker: PhantomData,
            }
        } else {
            StrictlyZonedAddr { addr: ZonedAddr::Unzoned(addr), marker: PhantomData }
        }
    }

    #[cfg(feature = "testutils")]
    /// Creates the unzoned variant, or panics if the addr's scope needs a zone.
    pub fn new_unzoned_or_panic(addr: W) -> Self {
        Self::new_with_zone(addr, || panic!("addr unexpectedly required a zone."))
    }
}

impl<A: IpAddress, W: Witness<A>, Z> Deref for StrictlyZonedAddr<A, W, Z> {
    type Target = ZonedAddr<W, Z>;
    fn deref(&self) -> &Self::Target {
        let StrictlyZonedAddr { addr, marker: PhantomData } = self;
        addr
    }
}

/// An IP address that witnesses all required properties of a socket address.
///
/// Requires `SpecifiedAddr` because most contexts do not permit unspecified
/// addresses; those that do can hold a `Option<SocketIpAddr>`.
///
/// Requires `NonMappedAddr` because mapped addresses (i.e. ipv4-mapped-ipv6
/// addresses) are converted from their original IP version to their target IP
/// version when entering the stack.
#[derive(Copy, Clone, Eq, GenericOverIp, Hash, PartialEq)]
#[generic_over_ip(A, IpAddress)]
pub struct SocketIpAddr<A: IpAddress>(NonMappedAddr<SpecifiedAddr<A>>);

impl<A: IpAddress> Display for SocketIpAddr<A> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let Self(addr) = self;
        write!(f, "{}", addr)
    }
}

impl<A: IpAddress> Debug for SocketIpAddr<A> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let Self(addr) = self;
        write!(f, "{:?}", addr)
    }
}

impl<A: IpAddress> SocketIpAddr<A> {
    /// Constructs a [`SocketIpAddr`] if the address is compliant, else `None`.
    pub fn new(addr: A) -> Option<SocketIpAddr<A>> {
        Some(SocketIpAddr(NonMappedAddr::new(SpecifiedAddr::new(addr)?)?))
    }

    /// Constructs a [`SocketIpAddr`] from the inner witness.
    pub fn new_from_witness(addr: NonMappedAddr<SpecifiedAddr<A>>) -> Self {
        Self(addr)
    }

    /// Constructs a [`SocketIpAddr`] without verify the address's properties.
    ///
    ///
    /// # Safety
    ///
    /// Callers must ensure that the addr is both a [`SpecifiedAddr`] and
    /// a [`NonMappedAddr`].
    pub const unsafe fn new_unchecked(addr: A) -> SocketIpAddr<A> {
        // SAFETY: delegated to caller.
        let non_mapped =
            unsafe { NonMappedAddr::new_unchecked(SpecifiedAddr::new_unchecked(addr)) };
        SocketIpAddr(non_mapped)
    }

    /// Like [`SocketIpAddr::new_unchecked`], but the address is specified.
    ///
    /// # Safety
    ///
    /// Callers must ensure that the addr is a [`NonMappedAddr`].
    pub const unsafe fn new_from_specified_unchecked(addr: SpecifiedAddr<A>) -> SocketIpAddr<A> {
        // SAFETY: delegated to caller.
        let non_mapped = unsafe { NonMappedAddr::new_unchecked(addr) };
        SocketIpAddr(non_mapped)
    }

    /// Returns the inner address, dropping all witnesses.
    pub fn addr(self) -> A {
        let SocketIpAddr(addr) = self;
        **addr
    }

    /// Returns the inner address, including all witness types.
    pub fn into_inner(self) -> NonMappedAddr<SpecifiedAddr<A>> {
        let SocketIpAddr(addr) = self;
        addr
    }

    /// Constructs a [`SocketIpAddr`] from the given multicast address.
    pub fn new_from_multicast(addr: MulticastAddr<A>) -> SocketIpAddr<A> {
        let addr: MulticastAddr<NonMappedAddr<_>> = addr.non_mapped().transpose();
        let addr: NonMappedAddr<SpecifiedAddr<_>> = addr.into_specified().transpose();
        SocketIpAddr(addr)
    }
}

impl SocketIpAddr<Ipv4Addr> {
    /// Constructs a [`SocketIpAddr`] from a given specified IPv4 address.
    pub fn new_ipv4_specified(addr: SpecifiedAddr<Ipv4Addr>) -> Self {
        addr.try_into().unwrap_or_else(|AddrIsMappedError {}| {
            unreachable!("IPv4 addresses must be non-mapped")
        })
    }

    /// Constructs a [`SocketIpAddr`] from the given [`Ipv4SourceAddr`].
    ///
    /// Returns `None` if the given address is unspecified.
    pub fn new_from_ipv4_source(addr: Ipv4SourceAddr) -> Option<Self> {
        match addr {
            Ipv4SourceAddr::Unspecified => None,
            Ipv4SourceAddr::Specified(addr) => Some(SocketIpAddr::new_ipv4_specified(addr.get())),
        }
    }
}

impl SocketIpAddr<Ipv6Addr> {
    /// Constructs a [`SocketIpAddr`] from the given [`Ipv6DeviceAddr`].
    pub fn new_from_ipv6_non_mapped_unicast(addr: NonMappedAddr<UnicastAddr<Ipv6Addr>>) -> Self {
        let addr: UnicastAddr<NonMappedAddr<_>> = addr.transpose();
        let addr: NonMappedAddr<SpecifiedAddr<_>> = addr.into_specified().transpose();
        SocketIpAddr(addr)
    }

    /// Optionally constructs a [`SocketIpAddr`] from the given
    /// [`Ipv6SourceAddr`], returning `None` if the given addr is `Unspecified`.
    pub fn new_from_ipv6_source(addr: Ipv6SourceAddr) -> Option<Self> {
        match addr {
            Ipv6SourceAddr::Unspecified => None,
            Ipv6SourceAddr::Unicast(addr) => {
                Some(SocketIpAddr::new_from_ipv6_non_mapped_unicast(addr))
            }
        }
    }
}

impl<A: IpAddress> From<SocketIpAddr<A>> for SpecifiedAddr<A> {
    fn from(addr: SocketIpAddr<A>) -> Self {
        let SocketIpAddr(addr) = addr;
        *addr
    }
}

impl<A: IpAddress> AsRef<SpecifiedAddr<A>> for SocketIpAddr<A> {
    fn as_ref(&self) -> &SpecifiedAddr<A> {
        let SocketIpAddr(addr) = self;
        addr.as_ref()
    }
}

/// The addr could not be converted to a `NonMappedAddr`.
///
/// Perhaps the address was an ipv4-mapped-ipv6 addresses.
#[derive(Debug)]
pub struct AddrIsMappedError {}

impl<A: IpAddress> TryFrom<SpecifiedAddr<A>> for SocketIpAddr<A> {
    type Error = AddrIsMappedError;
    fn try_from(addr: SpecifiedAddr<A>) -> Result<Self, Self::Error> {
        NonMappedAddr::new(addr).map(SocketIpAddr).ok_or(AddrIsMappedError {})
    }
}

/// Allows [`SocketIpAddr`] to be used inside of a [`ZonedAddr`].
impl<A: IpAddress> ScopeableAddress for SocketIpAddr<A> {
    type Scope = A::Scope;
    fn scope(&self) -> Self::Scope {
        let SocketIpAddr(addr) = self;
        addr.scope()
    }
}

/// The IP address and identifier (port) of a listening socket.
#[derive(Copy, Clone, Debug, Eq, GenericOverIp, Hash, PartialEq)]
#[generic_over_ip(A, IpAddress)]
pub struct ListenerIpAddr<A: IpAddress, LI> {
    /// The specific address being listened on, or `None` for all addresses.
    pub addr: Option<SocketIpAddr<A>>,
    /// The local identifier (i.e. port for TCP/UDP).
    pub identifier: LI,
}

impl<A: IpAddress, LI: Into<NonZeroU16>> Into<(Option<SpecifiedAddr<A>>, NonZeroU16)>
    for ListenerIpAddr<A, LI>
{
    fn into(self) -> (Option<SpecifiedAddr<A>>, NonZeroU16) {
        let Self { addr, identifier } = self;
        (addr.map(Into::into), identifier.into())
    }
}

/// The address of a listening socket.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq, GenericOverIp)]
#[generic_over_ip(A, GenericOverIp)]
pub struct ListenerAddr<A, D> {
    /// The IP address of the listening socket.
    pub ip: A,
    /// The bound device of the listening socket.
    pub device: Option<D>,
}

impl<A, D> ListenerAddr<A, D>
where
    A: Clone,
{
    /// Returns this same `ListenerAddr` but with unspecified device.
    pub fn without_device(&self) -> Self {
        Self { ip: self.ip.clone(), device: None }
    }

    /// True if device is specified.
    pub fn has_device(&self) -> bool {
        self.device.is_some()
    }
}

impl<A, LI, D> ListenerAddr<ListenerIpAddr<A, LI>, D>
where
    A: IpAddress,
    LI: Clone,
    D: Clone,
{
    /// Returns this same `ListenerAddr` but with the IP address set to unspecified.
    pub fn without_addr(&self) -> Self {
        Self {
            ip: ListenerIpAddr { addr: None, identifier: self.ip.identifier.clone() },
            device: self.device.clone(),
        }
    }

    /// True if address is specified.
    pub fn has_addr(&self) -> bool {
        self.ip.addr.is_some()
    }
}

impl<A, D> AsRef<Option<D>> for ListenerAddr<A, D> {
    fn as_ref(&self) -> &Option<D> {
        &self.device
    }
}

impl<TA, OA, D> AsRef<Option<D>> for EitherStack<ListenerAddr<TA, D>, ListenerAddr<OA, D>> {
    fn as_ref(&self) -> &Option<D> {
        match self {
            EitherStack::ThisStack(l) => &l.device,
            EitherStack::OtherStack(l) => &l.device,
        }
    }
}

/// The IP addresses and identifiers (ports) of a connected socket.
#[derive(Copy, Clone, Debug, Eq, GenericOverIp, Hash, PartialEq)]
#[generic_over_ip(A, IpAddress)]
pub struct ConnIpAddrInner<A, LI, RI> {
    /// The local address, port tuple.
    pub local: (A, LI),
    /// The remote address, port tuple.
    pub remote: (A, RI),
}

/// The IP addresses and identifiers (ports) of a connected socket.
pub type ConnIpAddr<A, LI, RI> = ConnIpAddrInner<SocketIpAddr<A>, LI, RI>;
/// The IP addresses (mapped if dual-stack) and identifiers (ports) of a connected socket.
pub type ConnInfoAddr<A, RI> = ConnIpAddrInner<SpecifiedAddr<A>, NonZeroU16, RI>;

impl<A: IpAddress, LI: Into<NonZeroU16>, RI> From<ConnIpAddr<A, LI, RI>> for ConnInfoAddr<A, RI> {
    fn from(
        ConnIpAddr { local: (local_ip, local_identifier), remote: (remote_ip, remote_identifier) }: ConnIpAddr<A, LI, RI>,
    ) -> Self {
        Self {
            local: (local_ip.into(), local_identifier.into()),
            remote: (remote_ip.into(), remote_identifier),
        }
    }
}

/// The address of a connected socket.
#[derive(Copy, Clone, Debug, Eq, GenericOverIp, Hash, PartialEq)]
#[generic_over_ip()]
pub struct ConnAddr<A, D> {
    /// The IP address for the connected socket.
    pub ip: A,
    /// The bound device for the connected socket.
    pub device: Option<D>,
}

/// The IP address and identifier (port) of a dual-stack listening socket.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
pub enum DualStackListenerIpAddr<A: IpAddress, LI: Into<NonZeroU16>>
where
    A::Version: DualStackIpExt,
{
    /// The socket is bound to this stack.
    ThisStack(ListenerIpAddr<A, LI>),
    /// The socket is bound to the other stack.
    OtherStack(ListenerIpAddr<<<A::Version as DualStackIpExt>::OtherVersion as Ip>::Addr, LI>),
    /// The socket is dual-stack enabled and bound to the IPv6 any address.
    BothStacks(LI),
}

impl<A: IpAddress, NewIp: DualStackIpExt, LI: Into<NonZeroU16>> GenericOverIp<NewIp>
    for DualStackListenerIpAddr<A, LI>
where
    A::Version: DualStackIpExt,
{
    type Type = DualStackListenerIpAddr<NewIp::Addr, LI>;
}

impl<LI: Into<NonZeroU16>> Into<(Option<SpecifiedAddr<Ipv6Addr>>, NonZeroU16)>
    for DualStackListenerIpAddr<Ipv6Addr, LI>
{
    fn into(self) -> (Option<SpecifiedAddr<Ipv6Addr>>, NonZeroU16) {
        match self {
            Self::ThisStack(listener_ip_addr) => listener_ip_addr.into(),
            Self::OtherStack(ListenerIpAddr { addr, identifier }) => (
                Some(addr.map_or(Ipv4::UNSPECIFIED_ADDRESS, SocketIpAddr::addr).to_ipv6_mapped()),
                identifier.into(),
            ),
            Self::BothStacks(identifier) => (None, identifier.into()),
        }
    }
}

/// The IP address and identifiers (ports) of a dual-stack connected socket.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
#[allow(missing_docs)]
pub enum DualStackConnIpAddr<A: IpAddress, LI, RI>
where
    A::Version: DualStackIpExt,
{
    ThisStack(ConnIpAddr<A, LI, RI>),
    OtherStack(ConnIpAddr<<<A::Version as DualStackIpExt>::OtherVersion as Ip>::Addr, LI, RI>),
}

impl<A: IpAddress, NewIp: DualStackIpExt, LI, RI> GenericOverIp<NewIp>
    for DualStackConnIpAddr<A, LI, RI>
where
    A::Version: DualStackIpExt,
{
    type Type = DualStackConnIpAddr<NewIp::Addr, LI, RI>;
}

impl<LI: Into<NonZeroU16>, RI> From<DualStackConnIpAddr<Ipv6Addr, LI, RI>>
    for ConnInfoAddr<Ipv6Addr, RI>
{
    fn from(addr: DualStackConnIpAddr<Ipv6Addr, LI, RI>) -> Self {
        match addr {
            DualStackConnIpAddr::ThisStack(conn_ip_addr) => conn_ip_addr.into(),
            DualStackConnIpAddr::OtherStack(ConnIpAddr {
                local: (local_ip, local_identifier),
                remote: (remote_ip, remote_identifier),
            }) => ConnInfoAddr {
                local: (local_ip.addr().to_ipv6_mapped(), local_identifier.into()),
                remote: (remote_ip.addr().to_ipv6_mapped(), remote_identifier),
            },
        }
    }
}

impl<I: Ip, A: SocketMapAddrSpec> From<ListenerIpAddr<I::Addr, A::LocalIdentifier>>
    for IpAddrVec<I, A>
{
    fn from(listener: ListenerIpAddr<I::Addr, A::LocalIdentifier>) -> Self {
        IpAddrVec::Listener(listener)
    }
}

impl<I: Ip, A: SocketMapAddrSpec> From<ConnIpAddr<I::Addr, A::LocalIdentifier, A::RemoteIdentifier>>
    for IpAddrVec<I, A>
{
    fn from(conn: ConnIpAddr<I::Addr, A::LocalIdentifier, A::RemoteIdentifier>) -> Self {
        IpAddrVec::Connected(conn)
    }
}

impl<I: Ip, D, A: SocketMapAddrSpec>
    From<ListenerAddr<ListenerIpAddr<I::Addr, A::LocalIdentifier>, D>> for AddrVec<I, D, A>
{
    fn from(listener: ListenerAddr<ListenerIpAddr<I::Addr, A::LocalIdentifier>, D>) -> Self {
        AddrVec::Listen(listener)
    }
}

impl<I: Ip, D, A: SocketMapAddrSpec>
    From<ConnAddr<ConnIpAddr<I::Addr, A::LocalIdentifier, A::RemoteIdentifier>, D>>
    for AddrVec<I, D, A>
{
    fn from(
        conn: ConnAddr<ConnIpAddr<I::Addr, A::LocalIdentifier, A::RemoteIdentifier>, D>,
    ) -> Self {
        AddrVec::Conn(conn)
    }
}

/// An address vector containing the portions of a socket address that are
/// visible in an IP packet.
#[derive(Derivative)]
#[derivative(
    Debug(bound = ""),
    Clone(bound = ""),
    Eq(bound = ""),
    PartialEq(bound = ""),
    Hash(bound = "")
)]
#[allow(missing_docs)]
pub enum IpAddrVec<I: Ip, A: SocketMapAddrSpec> {
    Listener(ListenerIpAddr<I::Addr, A::LocalIdentifier>),
    Connected(ConnIpAddr<I::Addr, A::LocalIdentifier, A::RemoteIdentifier>),
}

impl<I: Ip, A: SocketMapAddrSpec> IpAddrVec<I, A> {
    fn with_device<D>(self, device: Option<D>) -> AddrVec<I, D, A> {
        match self {
            IpAddrVec::Listener(ip) => AddrVec::Listen(ListenerAddr { ip, device }),
            IpAddrVec::Connected(ip) => AddrVec::Conn(ConnAddr { ip, device }),
        }
    }

    /// Creates a new listener address vector.
    pub fn new_listener(ip: SocketIpAddr<I::Addr>, identifier: A::LocalIdentifier) -> Self {
        IpAddrVec::Listener(ListenerIpAddr { addr: Some(ip), identifier })
    }
}

impl<I: Ip, A: SocketMapAddrSpec> IpAddrVec<I, A> {
    /// Returns the next smallest address vector that would receive all the same
    /// packets as this one.
    ///
    /// Address vectors are ordered by their shadowing relationship, such that
    /// a "smaller" vector shadows a "larger" one. This function returns the
    /// smallest of the set of shadows of `self`.
    fn widen(self) -> Option<Self> {
        match self {
            IpAddrVec::Listener(ListenerIpAddr { addr: None, identifier }) => {
                let _: A::LocalIdentifier = identifier;
                None
            }
            IpAddrVec::Connected(ConnIpAddr { local: (local_ip, local_identifier), remote }) => {
                let _: (SocketIpAddr<I::Addr>, A::RemoteIdentifier) = remote;
                Some(ListenerIpAddr { addr: Some(local_ip), identifier: local_identifier })
            }
            IpAddrVec::Listener(ListenerIpAddr { addr: Some(addr), identifier }) => {
                let _: SocketIpAddr<I::Addr> = addr;
                Some(ListenerIpAddr { addr: None, identifier })
            }
        }
        .map(IpAddrVec::Listener)
    }
}

pub(crate) enum AddrVecIterInner<I: Ip, D, A: SocketMapAddrSpec> {
    WithDevice { device: D, emitted_device: bool, addr: IpAddrVec<I, A> },
    NoDevice { addr: IpAddrVec<I, A> },
    Done,
}

impl<I: Ip, D: Clone, A: SocketMapAddrSpec> Iterator for AddrVecIterInner<I, D, A> {
    type Item = AddrVec<I, D, A>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Done => None,
            Self::WithDevice { device, emitted_device, addr } => {
                if !*emitted_device {
                    *emitted_device = true;
                    Some(addr.clone().with_device(Some(device.clone())))
                } else {
                    let r = addr.clone().with_device(None);
                    if let Some(next) = addr.clone().widen() {
                        *addr = next;
                        *emitted_device = false;
                    } else {
                        *self = Self::Done;
                    }
                    Some(r)
                }
            }
            Self::NoDevice { addr } => {
                let r = addr.clone().with_device(None);
                if let Some(next) = addr.clone().widen() {
                    *addr = next;
                } else {
                    *self = Self::Done
                }
                Some(r)
            }
        }
    }
}

/// An iterator over socket addresses.
///
/// The generated address vectors are ordered according to the following
/// rules (ordered by precedence):
///   - a connected address is preferred over a listening address,
///   - a listening address for a specific IP address is preferred over one
///     for all addresses,
///   - an address with a specific device is preferred over one for all
///     devices.
///
/// The first yielded address is always the one provided via
/// [`AddrVecIter::with_device`] or [`AddrVecIter::without_device`].
pub struct AddrVecIter<I: Ip, D, A: SocketMapAddrSpec>(AddrVecIterInner<I, D, A>);

impl<I: Ip, D, A: SocketMapAddrSpec> AddrVecIter<I, D, A> {
    /// Constructs an [`AddrVecIter`] with `addr` and a specified `device`.
    pub fn with_device(addr: IpAddrVec<I, A>, device: D) -> Self {
        Self(AddrVecIterInner::WithDevice { device, emitted_device: false, addr })
    }

    /// Constructs an [`AddrVecIter`] with `addr` without a specified device.
    pub fn without_device(addr: IpAddrVec<I, A>) -> Self {
        Self(AddrVecIterInner::NoDevice { addr })
    }
}

impl<I: Ip, D: Clone, A: SocketMapAddrSpec> Iterator for AddrVecIter<I, D, A> {
    type Item = AddrVec<I, D, A>;

    fn next(&mut self) -> Option<Self::Item> {
        let Self(it) = self;
        it.next()
    }
}

#[derive(GenericOverIp)]
#[generic_over_ip(I, Ip)]
enum TryUnmapResult<I: DualStackIpExt, D> {
    /// The address does not have an un-mapped representation.
    ///
    /// This spits back the input address unmodified.
    CannotBeUnmapped(ZonedAddr<SocketIpAddr<I::Addr>, D>),
    /// The address in the other stack that corresponds to the input.
    ///
    /// Since [`SocketIpAddr`] is guaranteed to hold a specified address,
    /// this must hold an `Option<SocketIpAddr>`. Since `::FFFF:0.0.0.0` is
    /// a legal IPv4-mapped IPv6 address, this allows us to represent it as the
    /// unspecified IPv4 address.
    Mapped(Option<ZonedAddr<SocketIpAddr<<I::OtherVersion as Ip>::Addr>, D>>),
}

/// Try to convert a specified address into the address that maps to it from
/// the other stack.
///
/// This is an IP-generic function that tries to invert the
/// IPv4-to-IPv4-mapped-IPv6 conversion that is performed by
/// [`Ipv4Addr::to_ipv6_mapped`].
///
/// The only inputs that will produce [`TryUnmapResult::Mapped`] are
/// IPv4-mapped IPv6 addresses. All other inputs will produce
/// [`TryUnmapResult::CannotBeUnmapped`].
fn try_unmap<A: IpAddress, D>(addr: ZonedAddr<SpecifiedAddr<A>, D>) -> TryUnmapResult<A::Version, D>
where
    A::Version: DualStackIpExt,
{
    <A::Version as Ip>::map_ip(
        addr,
        |v4| {
            let addr = SocketIpAddr::new_ipv4_specified(v4.addr());
            TryUnmapResult::CannotBeUnmapped(ZonedAddr::Unzoned(addr))
        },
        |v6| match v6.addr().to_ipv4_mapped() {
            Some(v4) => {
                let addr = SpecifiedAddr::new(v4).map(SocketIpAddr::new_ipv4_specified);
                TryUnmapResult::Mapped(addr.map(ZonedAddr::Unzoned))
            }
            None => {
                let (addr, zone) = v6.into_addr_zone();
                let addr: SocketIpAddr<_> =
                    addr.try_into().unwrap_or_else(|AddrIsMappedError {}| {
                        unreachable!(
                            "addr cannot be mapped because `to_ipv4_mapped` returned `None`"
                        )
                    });
                TryUnmapResult::CannotBeUnmapped(ZonedAddr::new(addr, zone).unwrap_or_else(|| {
                    unreachable!("addr should still be scopeable after wrapping in `SocketIpAddr`")
                }))
            }
        },
    )
}

/// Provides a specified IP address to use in-place of an unspecified
/// remote.
///
/// Concretely, this method is called during `connect()` and `send_to()`
/// socket operations to transform an unspecified remote IP address to the
/// loopback address. This ensures conformance with Linux and BSD.
fn specify_unspecified_remote<I: SocketIpExt, A: From<SocketIpAddr<I::Addr>>, Z>(
    addr: Option<ZonedAddr<A, Z>>,
) -> ZonedAddr<A, Z> {
    addr.unwrap_or_else(|| ZonedAddr::Unzoned(I::LOOPBACK_ADDRESS_AS_SOCKET_IP_ADDR.into()))
}

/// A remote IP address that's either in the current stack or the other stack.
#[allow(missing_docs)]
pub enum DualStackRemoteIp<I: DualStackIpExt, D> {
    ThisStack(ZonedAddr<SocketIpAddr<I::Addr>, D>),
    OtherStack(ZonedAddr<SocketIpAddr<<I::OtherVersion as Ip>::Addr>, D>),
}

impl<I: SocketIpExt + DualStackIpExt<OtherVersion: SocketIpExt>, D> DualStackRemoteIp<I, D> {
    /// Returns the [`DualStackRemoteIp`] for the given `remote_ip``.
    ///
    /// An IPv4-mapped-IPv6 address will be unmapped to the inner IPv4 address,
    /// and an unspecified address will be populated with
    /// [`specify_unspecified_remote`].
    pub fn new(remote_ip: Option<ZonedAddr<SpecifiedAddr<I::Addr>, D>>) -> Self {
        let remote_ip = specify_unspecified_remote::<I, _, _>(remote_ip);
        match try_unmap(remote_ip) {
            TryUnmapResult::CannotBeUnmapped(remote_ip) => Self::ThisStack(remote_ip),
            TryUnmapResult::Mapped(remote_ip) => {
                // NB: Even though we ensured the address was specified above by
                // calling `specify_unspecified_remote`, it's possible that
                // unmapping the address made it unspecified (e.g. `::FFFF:0.0.0.0`
                // is a specified IPv6 addr but an unspecified IPv4 addr). Call
                // `specify_unspecified_remote` again to ensure the unmapped address
                // is specified.
                let remote_ip = specify_unspecified_remote::<I::OtherVersion, _, _>(remote_ip);
                Self::OtherStack(remote_ip)
            }
        }
    }
}

/// A local IP address that's either in the current stack or the other stack.
#[allow(missing_docs)]
pub enum DualStackLocalIp<I: DualStackIpExt, D> {
    ThisStack(ZonedAddr<SocketIpAddr<I::Addr>, D>),
    OtherStack(Option<ZonedAddr<SocketIpAddr<<I::OtherVersion as Ip>::Addr>, D>>),
}

impl<I: SocketIpExt + DualStackIpExt<OtherVersion: SocketIpExt>, D> DualStackLocalIp<I, D> {
    /// Returns the [`DualStackLocalIp`] for the given `local_ip``.
    ///
    /// If `local_ip` is the unspecified address for the other stack, returns
    /// `Self::OtherStack(None)`.
    pub fn new(local_ip: ZonedAddr<SpecifiedAddr<I::Addr>, D>) -> Self {
        match try_unmap(local_ip) {
            TryUnmapResult::CannotBeUnmapped(local_ip) => Self::ThisStack(local_ip),
            TryUnmapResult::Mapped(local_ip) => Self::OtherStack(local_ip),
        }
    }
}
