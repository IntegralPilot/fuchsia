// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! General-purpose socket utilities common to device layer and IP layer
//! sockets.

use core::convert::Infallible as Never;
use core::fmt::Debug;
use core::hash::Hash;
use core::marker::PhantomData;
use core::num::NonZeroU16;

use derivative::Derivative;
use net_types::ip::{GenericOverIp, Ip, IpAddress, IpVersion, IpVersionMarker, Ipv4, Ipv6};
use net_types::{
    AddrAndZone, MulticastAddress, ScopeableAddress, SpecifiedAddr, Witness, ZonedAddr,
};
use thiserror::Error;

use crate::LocalAddressError;
use crate::data_structures::socketmap::{
    Entry, IterShadows, OccupiedEntry as SocketMapOccupiedEntry, SocketMap, Tagged,
};
use crate::device::{
    DeviceIdentifier, EitherDeviceId, StrongDeviceIdentifier, WeakDeviceIdentifier,
};
use crate::error::{ExistsError, NotFoundError, ZonedAddressError};
use crate::ip::BroadcastIpExt;
use crate::socket::SocketCookie;
use crate::socket::address::{
    AddrVecIter, ConnAddr, ConnIpAddr, ListenerAddr, ListenerIpAddr, SocketIpAddr,
};
use packet_formats::ip::{IpProto, Ipv4Proto, Ipv6Proto};

/// A dual stack IP extention trait that provides the `OtherVersion` associated
/// type.
pub trait DualStackIpExt: Ip {
    /// The "other" IP version, e.g. [`Ipv4`] for [`Ipv6`] and vice-versa.
    type OtherVersion: DualStackIpExt<OtherVersion = Self>;
}

impl DualStackIpExt for Ipv4 {
    type OtherVersion = Ipv6;
}

impl DualStackIpExt for Ipv6 {
    type OtherVersion = Ipv4;
}

/// A tuple of values for `T` for both `I` and `I::OtherVersion`.
pub struct DualStackTuple<I: DualStackIpExt, T: GenericOverIp<I> + GenericOverIp<I::OtherVersion>> {
    this_stack: <T as GenericOverIp<I>>::Type,
    other_stack: <T as GenericOverIp<I::OtherVersion>>::Type,
    _marker: IpVersionMarker<I>,
}

impl<I: DualStackIpExt, T: GenericOverIp<I> + GenericOverIp<I::OtherVersion>> DualStackTuple<I, T> {
    /// Creates a new tuple with `this_stack` and `other_stack` values.
    pub fn new(this_stack: T, other_stack: <T as GenericOverIp<I::OtherVersion>>::Type) -> Self
    where
        T: GenericOverIp<I, Type = T>,
    {
        Self { this_stack, other_stack, _marker: IpVersionMarker::new() }
    }

    /// Retrieves `(this_stack, other_stack)` from the tuple.
    pub fn into_inner(
        self,
    ) -> (<T as GenericOverIp<I>>::Type, <T as GenericOverIp<I::OtherVersion>>::Type) {
        let Self { this_stack, other_stack, _marker } = self;
        (this_stack, other_stack)
    }

    /// Retrieves `this_stack` from the tuple.
    pub fn into_this_stack(self) -> <T as GenericOverIp<I>>::Type {
        self.this_stack
    }

    /// Borrows `this_stack` from the tuple.
    pub fn this_stack(&self) -> &<T as GenericOverIp<I>>::Type {
        &self.this_stack
    }

    /// Retrieves `other_stack` from the tuple.
    pub fn into_other_stack(self) -> <T as GenericOverIp<I::OtherVersion>>::Type {
        self.other_stack
    }

    /// Borrows `other_stack` from the tuple.
    pub fn other_stack(&self) -> &<T as GenericOverIp<I::OtherVersion>>::Type {
        &self.other_stack
    }

    /// Flips the types, making `this_stack` `other_stack` and vice-versa.
    pub fn flip(self) -> DualStackTuple<I::OtherVersion, T> {
        let Self { this_stack, other_stack, _marker } = self;
        DualStackTuple {
            this_stack: other_stack,
            other_stack: this_stack,
            _marker: IpVersionMarker::new(),
        }
    }

    /// Casts to IP version `X`.
    ///
    /// Given `DualStackTuple` contains complete information for both IP
    /// versions, it can be easily cast into an arbitrary `X` IP version.
    ///
    /// This can be used to tie together type parameters when dealing with dual
    /// stack sockets. For example, a `DualStackTuple` defined for `SockI` can
    /// be cast to any `WireI`.
    pub fn cast<X>(self) -> DualStackTuple<X, T>
    where
        X: DualStackIpExt,
        T: GenericOverIp<X>
            + GenericOverIp<X::OtherVersion>
            + GenericOverIp<Ipv4>
            + GenericOverIp<Ipv6>,
    {
        I::map_ip_in(
            self,
            |v4| X::map_ip_out(v4, |t| t, |t| t.flip()),
            |v6| X::map_ip_out(v6, |t| t.flip(), |t| t),
        )
    }
}

impl<
    I: DualStackIpExt,
    NewIp: DualStackIpExt,
    T: GenericOverIp<NewIp>
        + GenericOverIp<NewIp::OtherVersion>
        + GenericOverIp<I>
        + GenericOverIp<I::OtherVersion>,
> GenericOverIp<NewIp> for DualStackTuple<I, T>
{
    type Type = DualStackTuple<NewIp, T>;
}

/// Extension trait for `Ip` providing socket-specific functionality.
pub trait SocketIpExt: Ip {
    /// `Self::LOOPBACK_ADDRESS`, but wrapped in the `SocketIpAddr` type.
    const LOOPBACK_ADDRESS_AS_SOCKET_IP_ADDR: SocketIpAddr<Self::Addr> = unsafe {
        // SAFETY: The loopback address is a valid SocketIpAddr, as verified
        // in the `loopback_addr_is_valid_socket_addr` test.
        SocketIpAddr::new_from_specified_unchecked(Self::LOOPBACK_ADDRESS)
    };
}

impl<I: Ip> SocketIpExt for I {}

#[cfg(test)]
mod socket_ip_ext_test {
    use super::*;
    use ip_test_macro::ip_test;

    #[ip_test(I)]
    fn loopback_addr_is_valid_socket_addr<I: SocketIpExt>() {
        // `LOOPBACK_ADDRESS_AS_SOCKET_IP_ADDR is defined with the "unchecked"
        // constructor (which supports const construction). Verify here that the
        // addr actually satisfies all the requirements (protecting against far
        // away changes)
        let _addr = SocketIpAddr::new(I::LOOPBACK_ADDRESS_AS_SOCKET_IP_ADDR.addr())
            .expect("loopback address should be a valid SocketIpAddr");
    }
}

/// An IP version-specific protocol.
#[derive(Copy, Clone, Debug, PartialEq, Eq, GenericOverIp)]
#[generic_over_ip()]
pub enum EitherIpProto {
    /// An IPv4 protocol.
    V4(Ipv4Proto),
    /// An IPv6 protocol.
    V6(Ipv6Proto),
}

impl EitherIpProto {
    /// Returns the IP version of the protocol.
    pub fn ip_version(&self) -> IpVersion {
        match self {
            Self::V4(_) => IpVersion::V4,
            Self::V6(_) => IpVersion::V6,
        }
    }

    /// Returns the transport protocol if it is standard.
    pub fn ip_proto(&self) -> Option<IpProto> {
        match self {
            Self::V4(p) => match p {
                Ipv4Proto::Proto(proto) => Some(*proto),
                _ => None,
            },
            Self::V6(p) => match p {
                Ipv6Proto::Proto(proto) => Some(*proto),
                _ => None,
            },
        }
    }

    /// Returns the raw protocol number as a u8.
    pub fn u8_value(&self) -> u8 {
        match self {
            Self::V4(p) => (*p).into(),
            Self::V6(p) => (*p).into(),
        }
    }
}

/// Information about a socket passed to a socket operations filter.
#[derive(Clone, Debug)]
#[cfg_attr(any(test, feature = "testutils"), derive(PartialEq, Eq))]
pub struct SocketInfo {
    /// The IP-version-specific transport protocol.
    pub proto: EitherIpProto,
    /// The socket cookie.
    pub cookie: SocketCookie,
}

/// State belonging to either IP stack.
///
/// Like `[either::Either]`, but with more helpful variant names.
///
/// Note that this type is not optimally type-safe, because `T` and `O` are not
/// bound by `IP` and `IP::OtherVersion`, respectively. In many cases it may be
/// more appropriate to define a one-off enum parameterized over `I: Ip`.
#[derive(Debug, PartialEq, Eq)]
pub enum EitherStack<T, O> {
    /// In the current stack version.
    ThisStack(T),
    /// In the other version of the stack.
    OtherStack(O),
}

impl<T, O> Clone for EitherStack<T, O>
where
    T: Clone,
    O: Clone,
{
    #[cfg_attr(feature = "instrumented", track_caller)]
    fn clone(&self) -> Self {
        match self {
            Self::ThisStack(t) => Self::ThisStack(t.clone()),
            Self::OtherStack(t) => Self::OtherStack(t.clone()),
        }
    }
}

/// Control flow type containing either a dual-stack or non-dual-stack context.
///
/// This type exists to provide nice names to the result of
/// [`BoundStateContext::dual_stack_context`], and to allow generic code to
/// match on when checking whether a socket protocol and IP version support
/// dual-stack operation. If dual-stack operation is supported, a
/// [`MaybeDualStack::DualStack`] value will be held, otherwise a `NonDualStack`
/// value.
///
/// Note that the templated types to not have trait bounds; those are provided
/// by the trait with the `dual_stack_context` function.
///
/// In monomorphized code, this type frequently has exactly one template
/// parameter that is uninstantiable (it contains an instance of
/// [`core::convert::Infallible`] or some other empty enum, or a reference to
/// the same)! That lets the compiler optimize it out completely, creating no
/// actual runtime overhead.
#[derive(Debug)]
#[allow(missing_docs)]
pub enum MaybeDualStack<DS, NDS> {
    DualStack(DS),
    NotDualStack(NDS),
}

// Implement `GenericOverIp` for a `MaybeDualStack` whose `DS` and `NDS` also
// implement `GenericOverIp`.
impl<I: DualStackIpExt, DS: GenericOverIp<I>, NDS: GenericOverIp<I>> GenericOverIp<I>
    for MaybeDualStack<DS, NDS>
{
    type Type = MaybeDualStack<<DS as GenericOverIp<I>>::Type, <NDS as GenericOverIp<I>>::Type>;
}

/// An error encountered while enabling or disabling dual-stack operation.
#[derive(Copy, Clone, Debug, Eq, GenericOverIp, PartialEq, Error)]
#[generic_over_ip()]
pub enum SetDualStackEnabledError {
    /// A socket can only have dual stack enabled or disabled while unbound.
    #[error("a socket can only have dual stack enabled or disabled while unbound")]
    SocketIsBound,
    /// The socket's protocol is not dual stack capable.
    #[error(transparent)]
    NotCapable(#[from] NotDualStackCapableError),
}

/// An error encountered when attempting to perform dual stack operations on
/// socket with a non dual stack capable protocol.
#[derive(Copy, Clone, Debug, Eq, GenericOverIp, PartialEq, Error)]
#[generic_over_ip()]
#[error("socket's protocol is not dual-stack capable")]
pub struct NotDualStackCapableError;

/// Describes which direction(s) of the data path should be shut down.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub struct Shutdown {
    /// True if the send path is shut down for the owning socket.
    ///
    /// If this is true, the socket should not be able to send packets.
    pub send: bool,
    /// True if the receive path is shut down for the owning socket.
    ///
    /// If this is true, the socket should not be able to receive packets.
    pub receive: bool,
}

/// Which direction(s) to shut down for a socket.
#[derive(Copy, Clone, Debug, Eq, GenericOverIp, PartialEq)]
#[generic_over_ip()]
pub enum ShutdownType {
    /// Prevent sending packets on the socket.
    Send,
    /// Prevent receiving packets on the socket.
    Receive,
    /// Prevent sending and receiving packets on the socket.
    SendAndReceive,
}

impl ShutdownType {
    /// Returns a tuple of booleans for `(shutdown_send, shutdown_receive)`.
    pub fn to_send_receive(&self) -> (bool, bool) {
        match self {
            Self::Send => (true, false),
            Self::Receive => (false, true),
            Self::SendAndReceive => (true, true),
        }
    }

    /// Creates a [`ShutdownType`] from a pair of bools for send and receive.
    pub fn from_send_receive(send: bool, receive: bool) -> Option<Self> {
        match (send, receive) {
            (true, false) => Some(Self::Send),
            (false, true) => Some(Self::Receive),
            (true, true) => Some(Self::SendAndReceive),
            (false, false) => None,
        }
    }
}

/// Extensions to IP Address witnesses useful in the context of sockets.
pub trait SocketIpAddrExt<A: IpAddress>: Witness<A> + ScopeableAddress {
    /// Determines whether the provided address is underspecified by itself.
    ///
    /// Some addresses are ambiguous and so must have a zone identifier in order
    /// to be used in a socket address. This function returns true for IPv6
    /// link-local addresses and false for all others.
    fn must_have_zone(&self) -> bool
    where
        Self: Copy,
    {
        self.try_into_null_zoned().is_some()
    }

    /// Converts into a [`AddrAndZone<A, ()>`] if the address requires a zone.
    ///
    /// Otherwise returns `None`.
    fn try_into_null_zoned(self) -> Option<AddrAndZone<Self, ()>> {
        if self.get().is_loopback() {
            return None;
        }
        AddrAndZone::new(self, ())
    }
}

impl<A: IpAddress, W: Witness<A> + ScopeableAddress> SocketIpAddrExt<A> for W {}

/// An extention trait for [`ZonedAddr`].
pub trait SocketZonedAddrExt<W, A, D> {
    /// Returns the address and device that should be used for a socket.
    ///
    /// Given an address for a socket and an optional device that the socket is
    /// already bound on, returns the address and device that should be used
    /// for the socket. If `addr` and `device` require inconsistent devices,
    /// or if `addr` requires a zone but there is none specified (by `addr` or
    /// `device`), an error is returned.
    fn resolve_addr_with_device(
        self,
        device: Option<D::Weak>,
    ) -> Result<(W, Option<EitherDeviceId<D, D::Weak>>), ZonedAddressError>
    where
        D: StrongDeviceIdentifier;
}

impl<W, A, D> SocketZonedAddrExt<W, A, D> for ZonedAddr<W, D>
where
    W: ScopeableAddress + AsRef<SpecifiedAddr<A>>,
    A: IpAddress,
{
    fn resolve_addr_with_device(
        self,
        device: Option<D::Weak>,
    ) -> Result<(W, Option<EitherDeviceId<D, D::Weak>>), ZonedAddressError>
    where
        D: StrongDeviceIdentifier,
    {
        let (addr, zone) = self.into_addr_zone();
        let device = match (zone, device) {
            (Some(zone), Some(device)) => {
                if device != zone {
                    return Err(ZonedAddressError::DeviceZoneMismatch);
                }
                Some(EitherDeviceId::Strong(zone))
            }
            (Some(zone), None) => Some(EitherDeviceId::Strong(zone)),
            (None, Some(device)) => Some(EitherDeviceId::Weak(device)),
            (None, None) => {
                if addr.as_ref().must_have_zone() {
                    return Err(ZonedAddressError::RequiredZoneNotProvided);
                } else {
                    None
                }
            }
        };
        Ok((addr, device))
    }
}

/// A helper type to verify if applying socket updates is allowed for a given
/// current state.
///
/// The fields in `SocketDeviceUpdate` define the current state,
/// [`SocketDeviceUpdate::try_update`] applies the verification logic.
pub struct SocketDeviceUpdate<'a, A: IpAddress, D: WeakDeviceIdentifier> {
    /// The current local IP address.
    pub local_ip: Option<&'a SpecifiedAddr<A>>,
    /// The current remote IP address.
    pub remote_ip: Option<&'a SpecifiedAddr<A>>,
    /// The currently bound device.
    pub old_device: Option<&'a D>,
}

impl<'a, A: IpAddress, D: WeakDeviceIdentifier> SocketDeviceUpdate<'a, A, D> {
    /// Checks if an update from `old_device` to `new_device` is allowed,
    /// returning an error if not.
    pub fn check_update<N>(
        self,
        new_device: Option<&N>,
    ) -> Result<(), SocketDeviceUpdateNotAllowedError>
    where
        D: PartialEq<N>,
    {
        let Self { local_ip, remote_ip, old_device } = self;
        let must_have_zone = local_ip.is_some_and(|a| a.must_have_zone())
            || remote_ip.is_some_and(|a| a.must_have_zone());

        if !must_have_zone {
            return Ok(());
        }

        let old_device = old_device.unwrap_or_else(|| {
            panic!("local_ip={:?} or remote_ip={:?} must have zone", local_ip, remote_ip)
        });

        if new_device.is_some_and(|new_device| old_device == new_device) {
            Ok(())
        } else {
            Err(SocketDeviceUpdateNotAllowedError)
        }
    }
}

/// The device can't be updated on a socket.
pub struct SocketDeviceUpdateNotAllowedError;

/// Specification for the identifiers in an [`AddrVec`].
///
/// This is a convenience trait for bundling together the local and remote
/// identifiers for a protocol.
pub trait SocketMapAddrSpec {
    /// The local identifier portion of a socket address.
    type LocalIdentifier: Copy + Clone + Debug + Send + Sync + Hash + Eq + Into<NonZeroU16>;
    /// The remote identifier portion of a socket address.
    type RemoteIdentifier: Copy + Clone + Debug + Send + Sync + Hash + Eq;
}

/// Information about the address in a [`ListenerAddr`].
pub struct ListenerAddrInfo {
    /// Whether the address has a device bound.
    pub has_device: bool,
    /// Whether the listener is on a specified address (as opposed to a blanket
    /// listener).
    pub specified_addr: bool,
}

impl<A: IpAddress, D: DeviceIdentifier, LI> ListenerAddr<ListenerIpAddr<A, LI>, D> {
    pub(crate) fn info(&self) -> ListenerAddrInfo {
        let Self { device, ip: ListenerIpAddr { addr, identifier: _ } } = self;
        ListenerAddrInfo { has_device: device.is_some(), specified_addr: addr.is_some() }
    }
}

/// Specifies the types parameters for [`BoundSocketMap`] state as a single bundle.
pub trait SocketMapStateSpec {
    /// The tag value of a socket address vector entry.
    ///
    /// These values are derived from [`Self::ListenerAddrState`] and
    /// [`Self::ConnAddrState`].
    type AddrVecTag: Eq + Copy + Debug + 'static;

    /// Returns a the tag for a listener in the socket map.
    fn listener_tag(info: ListenerAddrInfo, state: &Self::ListenerAddrState) -> Self::AddrVecTag;

    /// Returns a the tag for a connected socket in the socket map.
    fn connected_tag(has_device: bool, state: &Self::ConnAddrState) -> Self::AddrVecTag;

    /// An identifier for a listening socket.
    type ListenerId: Clone + Debug;
    /// An identifier for a connected socket.
    type ConnId: Clone + Debug;

    /// The state stored for a listening socket that is used to determine
    /// whether sockets can share an address.
    type ListenerSharingState: Clone + Debug;

    /// The state stored for a connected socket that is used to determine
    /// whether sockets can share an address.
    type ConnSharingState: Clone + Debug;

    /// The state stored for a listener socket address.
    type ListenerAddrState: SocketMapAddrStateSpec<Id = Self::ListenerId, SharingState = Self::ListenerSharingState>
        + Debug;

    /// The state stored for a connected socket address.
    type ConnAddrState: SocketMapAddrStateSpec<Id = Self::ConnId, SharingState = Self::ConnSharingState>
        + Debug;
}

/// Error returned by implementations of [`SocketMapAddrStateSpec`] to indicate
/// incompatible changes to a socket map.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct IncompatibleError;

/// An inserter into a [`SocketMap`].
pub trait Inserter<T> {
    /// Inserts the provided item and consumes `self`.
    ///
    /// Inserts a single item and consumes the inserter (thus preventing
    /// additional insertions).
    fn insert(self, item: T);
}

impl<'a, T, E: Extend<T>> Inserter<T> for &'a mut E {
    fn insert(self, item: T) {
        self.extend([item])
    }
}

impl<T> Inserter<T> for Never {
    fn insert(self, _: T) {
        match self {}
    }
}

/// Describes an entry in a [`SocketMap`] for a listener or connection address.
pub trait SocketMapAddrStateSpec {
    /// The type of ID that can be present at the address.
    type Id;

    /// The sharing state for the address.
    ///
    /// This can be used to determine whether a socket can be inserted at the
    /// address. Every socket has its own sharing state associated with it,
    /// though the sharing state is not necessarily stored in the address
    /// entry.
    type SharingState;

    /// The type of inserter returned by [`SocketMapAddrStateSpec::try_get_inserter`].
    type Inserter<'a>: Inserter<Self::Id> + 'a
    where
        Self: 'a,
        Self::Id: 'a;

    /// Creates a new `Self` holding the provided socket with the given new
    /// sharing state at the specified address.
    fn new(new_sharing_state: &Self::SharingState, id: Self::Id) -> Self;

    /// Looks up the ID in self, returning `true` if it is present.
    fn contains_id(&self, id: &Self::Id) -> bool;

    /// Enables insertion in `self` for a new socket with the provided sharing
    /// state.
    ///
    /// If the new state is incompatible with the existing socket(s),
    /// implementations of this function should return `Err(IncompatibleError)`.
    /// If `Ok(x)` is returned, calling `x.insert(y)` will insert `y` into
    /// `self`.
    fn try_get_inserter<'a, 'b>(
        &'b mut self,
        new_sharing_state: &'a Self::SharingState,
    ) -> Result<Self::Inserter<'b>, IncompatibleError>;

    /// Returns `Ok` if an entry with the given sharing state could be added
    /// to `self`.
    ///
    /// If this returns `Ok`, `try_get_dest` should succeed.
    fn could_insert(&self, new_sharing_state: &Self::SharingState)
    -> Result<(), IncompatibleError>;

    /// Removes the given socket from the existing state.
    ///
    /// Implementations should assume that `id` is contained in `self`.
    fn remove_by_id(&mut self, id: Self::Id) -> RemoveResult;

    /// Returns the sharing state for the address.
    fn sharing_state(&self) -> Self::SharingState;
}

/// Provides behavior on updating the sharing state of a [`SocketMap`] entry.
pub trait SocketMapAddrStateUpdateSharingSpec: SocketMapAddrStateSpec {
    /// Attempts to update the sharing state of the address state with id `id`
    /// to `new_sharing_state`.
    fn try_update_sharing(
        &mut self,
        id: Self::Id,
        new_sharing_state: &Self::SharingState,
    ) -> Result<(), IncompatibleError>;
}

/// Provides conflict detection for a [`SocketMapStateSpec`].
pub trait SocketMapConflictPolicy<
    Addr,
    SharingState,
    I: Ip,
    D: DeviceIdentifier,
    A: SocketMapAddrSpec,
>: SocketMapStateSpec
{
    /// Checks whether a new socket with the provided state can be inserted at
    /// the given address in the existing socket map, returning an error
    /// otherwise.
    ///
    /// Implementations of this function should check for any potential
    /// conflicts that would arise when inserting a socket with state
    /// `new_sharing_state` into a new or existing entry at `addr` in
    /// `socketmap`.
    fn check_insert_conflicts(
        new_sharing_state: &SharingState,
        addr: &Addr,
        socketmap: &SocketMap<AddrVec<I, D, A>, Bound<Self>>,
    ) -> Result<(), InsertError>;
}

/// Defines the policy for updating the sharing state of entries in the
/// [`SocketMap`].
pub trait SocketMapUpdateSharingPolicy<Addr, SharingState, I: Ip, D: DeviceIdentifier, A>:
    SocketMapConflictPolicy<Addr, SharingState, I, D, A>
where
    A: SocketMapAddrSpec,
{
    /// Returns whether the entry `addr` in `socketmap` allows the sharing state
    /// to transition from `old_sharing` to `new_sharing`.
    fn allows_sharing_update(
        socketmap: &SocketMap<AddrVec<I, D, A>, Bound<Self>>,
        addr: &Addr,
        old_sharing: &SharingState,
        new_sharing: &SharingState,
    ) -> Result<(), UpdateSharingError>;
}

/// A bound socket state that is either a listener or a connection.
#[derive(Derivative)]
#[derivative(Debug(bound = "S::ListenerAddrState: Debug, S::ConnAddrState: Debug"))]
#[allow(missing_docs)]
pub enum Bound<S: SocketMapStateSpec + ?Sized> {
    Listen(S::ListenerAddrState),
    Conn(S::ConnAddrState),
}

/// An "address vector" type that can hold any address in a [`SocketMap`].
///
/// This is a "vector" in the mathematical sense, in that it denotes an address
/// in a space. Here, the space is the possible addresses to which a socket
/// receiving IP packets can be bound.
///
/// `AddrVec`s are used as keys for the `SocketMap` type. Since an incoming
/// packet can match more than one address, for each incoming packet there is a
/// set of possible `AddrVec` keys whose entries (sockets) in a `SocketMap`
/// might receive the packet.
///
/// This set of keys can be ordered by precedence as described in the
/// documentation for [`AddrVecIter`]. Calling [`IterShadows::iter_shadows`] on
/// an instance will produce the sequence of addresses it has precedence over.
#[derive(Derivative)]
#[derivative(
    Debug(bound = "D: Debug"),
    Clone(bound = "D: Clone"),
    Eq(bound = "D: Eq"),
    PartialEq(bound = "D: PartialEq"),
    Hash(bound = "D: Hash")
)]
#[allow(missing_docs)]
pub enum AddrVec<I: Ip, D, A: SocketMapAddrSpec + ?Sized> {
    Listen(ListenerAddr<ListenerIpAddr<I::Addr, A::LocalIdentifier>, D>),
    Conn(ConnAddr<ConnIpAddr<I::Addr, A::LocalIdentifier, A::RemoteIdentifier>, D>),
}

impl<I: Ip, D: DeviceIdentifier, A: SocketMapAddrSpec, S: SocketMapStateSpec + ?Sized>
    Tagged<AddrVec<I, D, A>> for Bound<S>
{
    type Tag = S::AddrVecTag;
    fn tag(&self, address: &AddrVec<I, D, A>) -> Self::Tag {
        match (self, address) {
            (Bound::Listen(l), AddrVec::Listen(addr)) => S::listener_tag(addr.info(), l),
            (Bound::Conn(c), AddrVec::Conn(ConnAddr { device, ip: _ })) => {
                S::connected_tag(device.is_some(), c)
            }
            (Bound::Listen(_), AddrVec::Conn(_)) => {
                unreachable!("found listen state for conn addr")
            }
            (Bound::Conn(_), AddrVec::Listen(_)) => {
                unreachable!("found conn state for listen addr")
            }
        }
    }
}

impl<I: Ip, D: DeviceIdentifier, A: SocketMapAddrSpec> IterShadows for AddrVec<I, D, A> {
    type IterShadows = AddrVecIter<I, D, A>;

    fn iter_shadows(&self) -> Self::IterShadows {
        let (socket_ip_addr, device) = match self.clone() {
            AddrVec::Conn(ConnAddr { ip, device }) => (ip.into(), device),
            AddrVec::Listen(ListenerAddr { ip, device }) => (ip.into(), device),
        };
        let mut iter = match device {
            Some(device) => AddrVecIter::with_device(socket_ip_addr, device),
            None => AddrVecIter::without_device(socket_ip_addr),
        };
        // Skip the first element, which is always `*self`.
        assert_eq!(iter.next().as_ref(), Some(self));
        iter
    }
}

/// How a socket is bound on the system.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
#[allow(missing_docs)]
pub enum SocketAddrType {
    AnyListener,
    SpecificListener,
    Connected,
}

impl<'a, A: IpAddress, LI> From<&'a ListenerIpAddr<A, LI>> for SocketAddrType {
    fn from(ListenerIpAddr { addr, identifier: _ }: &'a ListenerIpAddr<A, LI>) -> Self {
        match addr {
            Some(_) => SocketAddrType::SpecificListener,
            None => SocketAddrType::AnyListener,
        }
    }
}

impl<'a, A: IpAddress, LI, RI> From<&'a ConnIpAddr<A, LI, RI>> for SocketAddrType {
    fn from(_: &'a ConnIpAddr<A, LI, RI>) -> Self {
        SocketAddrType::Connected
    }
}

/// The result of attempting to remove a socket from a collection of sockets.
pub enum RemoveResult {
    /// The value was removed successfully.
    Success,
    /// The value is the last value in the collection so the entire collection
    /// should be removed.
    IsLast,
}

#[derive(Derivative)]
#[derivative(Clone(bound = "S::ListenerId: Clone, S::ConnId: Clone"), Debug(bound = ""))]
pub enum SocketId<S: SocketMapStateSpec> {
    Listener(S::ListenerId),
    Connection(S::ConnId),
}

/// A map from socket addresses to sockets.
///
/// The types of keys and IDs is determined by the [`SocketMapStateSpec`]
/// parameter. Each listener and connected socket stores additional state.
/// Listener and connected sockets are keyed independently, but share the same
/// address vector space. Conflicts are detected on attempted insertion of new
/// sockets.
///
/// Listener addresses map to listener-address-specific state, and likewise
/// with connected addresses. Depending on protocol (determined by the
/// `SocketMapStateSpec` protocol), these address states can hold one or more
/// socket identifiers (e.g. UDP sockets with `SO_REUSEPORT` set can share an
/// address).
#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct BoundSocketMap<I: Ip, D: DeviceIdentifier, A: SocketMapAddrSpec, S: SocketMapStateSpec> {
    addr_to_state: SocketMap<AddrVec<I, D, A>, Bound<S>>,
}

impl<I: Ip, D: DeviceIdentifier, A: SocketMapAddrSpec, S: SocketMapStateSpec>
    BoundSocketMap<I, D, A, S>
{
    /// Returns the number of entries in the map.
    pub fn len(&self) -> usize {
        self.addr_to_state.len()
    }
}

/// Uninstantiable tag type for denoting listening sockets.
pub enum Listener {}
/// Uninstantiable tag type for denoting connected sockets.
pub enum Connection {}

/// View struct over one type of sockets in a [`BoundSocketMap`].
pub struct Sockets<AddrToStateMap, SocketType>(AddrToStateMap, PhantomData<SocketType>);

impl<
    'a,
    I: Ip,
    D: DeviceIdentifier,
    SocketType: ConvertSocketMapState<I, D, A, S>,
    A: SocketMapAddrSpec,
    S: SocketMapConflictPolicy<SocketType::Addr, SocketType::SharingState, I, D, A>,
> Sockets<&'a SocketMap<AddrVec<I, D, A>, Bound<S>>, SocketType>
{
    /// Returns the state at an address, if there is any.
    pub fn get_by_addr(self, addr: &SocketType::Addr) -> Option<&'a SocketType::AddrState> {
        let Self(addr_to_state, _marker) = self;
        addr_to_state.get(&SocketType::to_addr_vec(addr)).map(|state| {
            SocketType::from_bound_ref(state)
                .unwrap_or_else(|| unreachable!("found {:?} for address {:?}", state, addr))
        })
    }

    /// Returns `Ok(())` if a socket could be inserted, otherwise an error.
    ///
    /// Goes through a dry run of inserting a socket at the given address and
    /// with the given sharing state, returning `Ok(())` if the insertion would
    /// succeed, otherwise the error that would be returned.
    pub fn could_insert(
        self,
        addr: &SocketType::Addr,
        sharing: &SocketType::SharingState,
    ) -> Result<(), InsertError> {
        let Self(addr_to_state, _) = self;
        match self.get_by_addr(addr) {
            Some(state) => {
                state.could_insert(sharing).map_err(|IncompatibleError| InsertError::Exists)
            }
            None => S::check_insert_conflicts(&sharing, &addr, &addr_to_state),
        }
    }
}

/// A borrowed state entry in a [`SocketMap`].
#[derive(Derivative)]
#[derivative(Debug(bound = ""))]
pub struct SocketStateEntry<
    'a,
    I: Ip,
    D: DeviceIdentifier,
    A: SocketMapAddrSpec,
    S: SocketMapStateSpec,
    SocketType,
> {
    id: SocketId<S>,
    addr_entry: SocketMapOccupiedEntry<'a, AddrVec<I, D, A>, Bound<S>>,
    _marker: PhantomData<SocketType>,
}

impl<
    'a,
    I: Ip,
    D: DeviceIdentifier,
    SocketType: ConvertSocketMapState<I, D, A, S>,
    A: SocketMapAddrSpec,
    S: SocketMapConflictPolicy<SocketType::Addr, SocketType::SharingState, I, D, A>,
> Sockets<&'a mut SocketMap<AddrVec<I, D, A>, Bound<S>>, SocketType>
where
    SocketType::SharingState: Clone,
    SocketType::Id: Clone,
{
    /// Attempts to insert a new entry into the [`SocketMap`] backing this
    /// `Sockets`.
    pub fn try_insert(
        self,
        socket_addr: SocketType::Addr,
        tag_state: SocketType::SharingState,
        id: SocketType::Id,
    ) -> Result<SocketStateEntry<'a, I, D, A, S, SocketType>, InsertError> {
        self.try_insert_with(socket_addr, tag_state, |_addr, _sharing| (id, ()))
            .map(|(entry, ())| entry)
    }

    /// Like [`Sockets::try_insert`] but calls `make_id` to create a socket ID
    /// before inserting into the map.
    ///
    /// `make_id` returns type `R` that is returned to the caller on success.
    pub fn try_insert_with<R>(
        self,
        socket_addr: SocketType::Addr,
        tag_state: SocketType::SharingState,
        make_id: impl FnOnce(SocketType::Addr, SocketType::SharingState) -> (SocketType::Id, R),
    ) -> Result<(SocketStateEntry<'a, I, D, A, S, SocketType>, R), InsertError> {
        let Self(addr_to_state, _) = self;
        S::check_insert_conflicts(&tag_state, &socket_addr, &addr_to_state)?;

        let addr = SocketType::to_addr_vec(&socket_addr);

        match addr_to_state.entry(addr) {
            Entry::Occupied(mut o) => {
                let (id, ret) = o.map_mut(|bound| {
                    let bound = match SocketType::from_bound_mut(bound) {
                        Some(bound) => bound,
                        None => unreachable!("found {:?} for address {:?}", bound, socket_addr),
                    };
                    match <SocketType::AddrState as SocketMapAddrStateSpec>::try_get_inserter(
                        bound, &tag_state,
                    ) {
                        Ok(v) => {
                            let (id, ret) = make_id(socket_addr, tag_state);
                            v.insert(id.clone());
                            Ok((SocketType::to_socket_id(id), ret))
                        }
                        Err(IncompatibleError) => Err(InsertError::Exists),
                    }
                })?;
                Ok((SocketStateEntry { id, addr_entry: o, _marker: Default::default() }, ret))
            }
            Entry::Vacant(v) => {
                let (id, ret) = make_id(socket_addr, tag_state.clone());
                let addr_entry = v.insert(SocketType::to_bound(SocketType::AddrState::new(
                    &tag_state,
                    id.clone(),
                )));
                let id = SocketType::to_socket_id(id);
                Ok((SocketStateEntry { id, addr_entry, _marker: Default::default() }, ret))
            }
        }
    }

    /// Returns a borrowed entry at `id` and `addr`.
    pub fn entry(
        self,
        id: &SocketType::Id,
        addr: &SocketType::Addr,
    ) -> Option<SocketStateEntry<'a, I, D, A, S, SocketType>> {
        let Self(addr_to_state, _) = self;
        let addr_entry = match addr_to_state.entry(SocketType::to_addr_vec(addr)) {
            Entry::Vacant(_) => return None,
            Entry::Occupied(o) => o,
        };
        let state = SocketType::from_bound_ref(addr_entry.get())?;

        state.contains_id(id).then_some(SocketStateEntry {
            id: SocketType::to_socket_id(id.clone()),
            addr_entry,
            _marker: PhantomData::default(),
        })
    }

    /// Removes the entry with `id` and `addr`.
    pub fn remove(self, id: &SocketType::Id, addr: &SocketType::Addr) -> Result<(), NotFoundError> {
        self.entry(id, addr)
            .map(|entry| {
                entry.remove();
            })
            .ok_or(NotFoundError)
    }
}

/// The error returned when updating the sharing state for a [`SocketMap`] entry
/// fails.
#[derive(Debug)]
pub struct UpdateSharingError;

impl<
    'a,
    I: Ip,
    D: DeviceIdentifier,
    SocketType: ConvertSocketMapState<I, D, A, S>,
    A: SocketMapAddrSpec,
    S: SocketMapConflictPolicy<SocketType::Addr, SocketType::SharingState, I, D, A>,
> SocketStateEntry<'a, I, D, A, S, SocketType>
where
    SocketType::Id: Clone,
{
    /// Returns this entry's address.
    pub fn get_addr(&self) -> &SocketType::Addr {
        let Self { id: _, addr_entry, _marker } = self;
        SocketType::from_addr_vec_ref(addr_entry.key())
    }

    /// Returns this entry's identifier.
    pub fn id(&self) -> &SocketType::Id {
        let Self { id, addr_entry: _, _marker } = self;
        SocketType::from_socket_id_ref(id)
    }

    /// Attempts to update the address for this entry.
    pub fn try_update_addr(self, new_addr: SocketType::Addr) -> Result<Self, (ExistsError, Self)> {
        let Self { id, addr_entry, _marker } = self;

        let new_addrvec = SocketType::to_addr_vec(&new_addr);
        let old_addr = addr_entry.key().clone();
        let (addr_state, addr_to_state) = addr_entry.remove_from_map();
        let addr_to_state = match addr_to_state.entry(new_addrvec) {
            Entry::Occupied(o) => o.into_map(),
            Entry::Vacant(v) => {
                let sharing_state = SocketType::from_bound_ref(&addr_state)
                    .unwrap_or_else(|| {
                        unreachable!("found {:?} for address {:?}", addr_state, old_addr)
                    })
                    .sharing_state();
                match S::check_insert_conflicts(&sharing_state, &new_addr, v.get_map()) {
                    Ok(_) => {
                        let new_addr_entry = v.insert(addr_state);
                        return Ok(SocketStateEntry { id, addr_entry: new_addr_entry, _marker });
                    }
                    Err(_) => v.into_map(),
                }
            }
        };
        let to_restore = addr_state;
        // Restore the old state before returning an error.
        let addr_entry = match addr_to_state.entry(old_addr) {
            Entry::Occupied(_) => unreachable!("just-removed-from entry is occupied"),
            Entry::Vacant(v) => v.insert(to_restore),
        };
        return Err((ExistsError, SocketStateEntry { id, addr_entry, _marker }));
    }

    /// Removes this entry from the map.
    pub fn remove(self) {
        let Self { id, mut addr_entry, _marker } = self;
        let addr = addr_entry.key().clone();
        match addr_entry.map_mut(|value| {
            let value = match SocketType::from_bound_mut(value) {
                Some(value) => value,
                None => unreachable!("found {:?} for address {:?}", value, addr),
            };
            value.remove_by_id(SocketType::from_socket_id_ref(&id).clone())
        }) {
            RemoveResult::Success => (),
            RemoveResult::IsLast => {
                let _: Bound<S> = addr_entry.remove();
            }
        }
    }

    /// Attempts to update the sharing state for this entry.
    pub fn try_update_sharing(
        &mut self,
        old_sharing_state: &SocketType::SharingState,
        new_sharing_state: SocketType::SharingState,
    ) -> Result<(), UpdateSharingError>
    where
        SocketType::AddrState: SocketMapAddrStateUpdateSharingSpec,
        S: SocketMapUpdateSharingPolicy<SocketType::Addr, SocketType::SharingState, I, D, A>,
    {
        let Self { id, addr_entry, _marker } = self;
        let addr = SocketType::from_addr_vec_ref(addr_entry.key());

        S::allows_sharing_update(
            addr_entry.get_map(),
            addr,
            old_sharing_state,
            &new_sharing_state,
        )?;

        addr_entry
            .map_mut(|value| {
                let value = match SocketType::from_bound_mut(value) {
                    Some(value) => value,
                    // We shouldn't ever be storing listener state in a bound
                    // address, or bound state in a listener address. Doing so means
                    // we've got a serious bug.
                    None => unreachable!("found invalid state {:?}", value),
                };

                value.try_update_sharing(
                    SocketType::from_socket_id_ref(id).clone(),
                    &new_sharing_state,
                )
            })
            .map_err(|IncompatibleError| UpdateSharingError)
    }
}

impl<I: Ip, D: DeviceIdentifier, A: SocketMapAddrSpec, S> BoundSocketMap<I, D, A, S>
where
    AddrVec<I, D, A>: IterShadows,
    S: SocketMapStateSpec,
{
    /// Returns an iterator over the listeners on the socket map.
    pub fn listeners(&self) -> Sockets<&SocketMap<AddrVec<I, D, A>, Bound<S>>, Listener>
    where
        S: SocketMapConflictPolicy<
                ListenerAddr<ListenerIpAddr<I::Addr, A::LocalIdentifier>, D>,
                <S as SocketMapStateSpec>::ListenerSharingState,
                I,
                D,
                A,
            >,
        S::ListenerAddrState:
            SocketMapAddrStateSpec<Id = S::ListenerId, SharingState = S::ListenerSharingState>,
    {
        let Self { addr_to_state } = self;
        Sockets(addr_to_state, Default::default())
    }

    /// Returns a mutable iterator over the listeners on the socket map.
    pub fn listeners_mut(&mut self) -> Sockets<&mut SocketMap<AddrVec<I, D, A>, Bound<S>>, Listener>
    where
        S: SocketMapConflictPolicy<
                ListenerAddr<ListenerIpAddr<I::Addr, A::LocalIdentifier>, D>,
                <S as SocketMapStateSpec>::ListenerSharingState,
                I,
                D,
                A,
            >,
        S::ListenerAddrState:
            SocketMapAddrStateSpec<Id = S::ListenerId, SharingState = S::ListenerSharingState>,
    {
        let Self { addr_to_state } = self;
        Sockets(addr_to_state, Default::default())
    }

    /// Returns an iterator over the connections on the socket map.
    pub fn conns(&self) -> Sockets<&SocketMap<AddrVec<I, D, A>, Bound<S>>, Connection>
    where
        S: SocketMapConflictPolicy<
                ConnAddr<ConnIpAddr<I::Addr, A::LocalIdentifier, A::RemoteIdentifier>, D>,
                <S as SocketMapStateSpec>::ConnSharingState,
                I,
                D,
                A,
            >,
        S::ConnAddrState:
            SocketMapAddrStateSpec<Id = S::ConnId, SharingState = S::ConnSharingState>,
    {
        let Self { addr_to_state } = self;
        Sockets(addr_to_state, Default::default())
    }

    /// Returns a mutable iterator over the connections on the socket map.
    pub fn conns_mut(&mut self) -> Sockets<&mut SocketMap<AddrVec<I, D, A>, Bound<S>>, Connection>
    where
        S: SocketMapConflictPolicy<
                ConnAddr<ConnIpAddr<I::Addr, A::LocalIdentifier, A::RemoteIdentifier>, D>,
                <S as SocketMapStateSpec>::ConnSharingState,
                I,
                D,
                A,
            >,
        S::ConnAddrState:
            SocketMapAddrStateSpec<Id = S::ConnId, SharingState = S::ConnSharingState>,
    {
        let Self { addr_to_state } = self;
        Sockets(addr_to_state, Default::default())
    }

    #[cfg(test)]
    pub(crate) fn iter_addrs(&self) -> impl Iterator<Item = &AddrVec<I, D, A>> {
        let Self { addr_to_state } = self;
        addr_to_state.iter().map(|(a, _v): (_, &Bound<S>)| a)
    }

    /// Gets the number of shadower entries for `addr`.
    pub fn get_shadower_counts(&self, addr: &AddrVec<I, D, A>) -> usize {
        let Self { addr_to_state } = self;
        addr_to_state.descendant_counts(&addr).map(|(_sharing, size)| size.get()).sum()
    }
}

/// The type returned by [`BoundSocketMap::iter_receivers`].
pub enum FoundSockets<A, It> {
    /// A single recipient was found for the address.
    Single(A),
    /// Indicates the looked-up address was multicast, and holds an iterator of
    /// the found receivers.
    Multicast(It),
}

/// A borrowed entry in a [`BoundSocketMap`].
#[allow(missing_docs)]
#[derive(Debug)]
pub enum AddrEntry<'a, I: Ip, D, A: SocketMapAddrSpec, S: SocketMapStateSpec> {
    Listen(&'a S::ListenerAddrState, ListenerAddr<ListenerIpAddr<I::Addr, A::LocalIdentifier>, D>),
    Conn(
        &'a S::ConnAddrState,
        ConnAddr<ConnIpAddr<I::Addr, A::LocalIdentifier, A::RemoteIdentifier>, D>,
    ),
}

impl<I, D, A, S> BoundSocketMap<I, D, A, S>
where
    I: BroadcastIpExt<Addr: MulticastAddress>,
    D: DeviceIdentifier,
    A: SocketMapAddrSpec,
    S: SocketMapStateSpec
        + SocketMapConflictPolicy<
            ListenerAddr<ListenerIpAddr<I::Addr, A::LocalIdentifier>, D>,
            <S as SocketMapStateSpec>::ListenerSharingState,
            I,
            D,
            A,
        > + SocketMapConflictPolicy<
            ConnAddr<ConnIpAddr<I::Addr, A::LocalIdentifier, A::RemoteIdentifier>, D>,
            <S as SocketMapStateSpec>::ConnSharingState,
            I,
            D,
            A,
        >,
{
    /// Looks up a connected socket.
    ///
    /// This is a lightweight version of `iter_receivers()` that doesn't try to
    /// lookup listening sockets. It is used for early demux, which applies only
    /// to connected sockets.
    pub fn lookup_connected(
        &self,
        (src_ip, src_port): (SocketIpAddr<I::Addr>, A::RemoteIdentifier),
        (dst_ip, dst_port): (SocketIpAddr<I::Addr>, A::LocalIdentifier),
        device: D,
    ) -> Option<&'_ S::ConnAddrState> {
        let mut addr = ConnAddr {
            ip: ConnIpAddr { local: (dst_ip, dst_port), remote: (src_ip, src_port) },
            device: Some(device),
        };
        let entry = self.conns().get_by_addr(&addr);
        if entry.is_some() {
            return entry;
        }
        addr.device = None;
        self.conns().get_by_addr(&addr)
    }

    /// Finds the socket(s) that should receive an incoming packet.
    ///
    /// Uses the provided addresses and receiving device to look up sockets that
    /// should receive a matching incoming packet. Returns `None` if no sockets
    /// were found, or the results of the lookup.
    pub fn iter_receivers(
        &self,
        (src_ip, src_port): (Option<SocketIpAddr<I::Addr>>, Option<A::RemoteIdentifier>),
        (dst_ip, dst_port): (SocketIpAddr<I::Addr>, A::LocalIdentifier),
        device: D,
        broadcast: Option<I::BroadcastMarker>,
    ) -> Option<
        FoundSockets<
            AddrEntry<'_, I, D, A, S>,
            impl Iterator<Item = AddrEntry<'_, I, D, A, S>> + '_,
        >,
    > {
        let mut matching_entries = AddrVecIter::with_device(
            match (src_ip, src_port) {
                (Some(specified_src_ip), Some(src_port)) => {
                    ConnIpAddr { local: (dst_ip, dst_port), remote: (specified_src_ip, src_port) }
                        .into()
                }
                _ => ListenerIpAddr { addr: Some(dst_ip), identifier: dst_port }.into(),
            },
            device,
        )
        .filter_map(move |addr: AddrVec<I, D, A>| match addr {
            AddrVec::Listen(l) => {
                self.listeners().get_by_addr(&l).map(|state| AddrEntry::Listen(state, l))
            }
            AddrVec::Conn(c) => self.conns().get_by_addr(&c).map(|state| AddrEntry::Conn(state, c)),
        });

        if broadcast.is_some() || dst_ip.addr().is_multicast() {
            Some(FoundSockets::Multicast(matching_entries))
        } else {
            let single_entry: Option<_> = matching_entries.next();
            single_entry.map(FoundSockets::Single)
        }
    }
}

/// Errors observed by [`SocketMapConflictPolicy`].
#[derive(Debug, Eq, PartialEq)]
pub enum InsertError {
    /// A shadow address exists for the entry.
    ShadowAddrExists,
    /// Entry already exists.
    Exists,
    /// Entry would shadow existing entry.
    WouldShadowExisting,
    /// An indirect conflict was detected.
    IndirectConflict,
}

impl From<InsertError> for LocalAddressError {
    fn from(value: InsertError) -> Self {
        match value {
            InsertError::ShadowAddrExists
            | InsertError::Exists
            | InsertError::IndirectConflict
            | InsertError::WouldShadowExisting => LocalAddressError::AddressInUse,
        }
    }
}

/// Helper trait for converting between [`AddrVec`] and [`Bound`] and their
/// variants.
pub trait ConvertSocketMapState<I: Ip, D, A: SocketMapAddrSpec, S: SocketMapStateSpec> {
    type Id;
    type SharingState;
    type Addr: Debug;
    type AddrState: SocketMapAddrStateSpec<Id = Self::Id, SharingState = Self::SharingState>;

    fn to_addr_vec(addr: &Self::Addr) -> AddrVec<I, D, A>;
    fn from_addr_vec_ref(addr: &AddrVec<I, D, A>) -> &Self::Addr;
    fn from_bound_ref(bound: &Bound<S>) -> Option<&Self::AddrState>;
    fn from_bound_mut(bound: &mut Bound<S>) -> Option<&mut Self::AddrState>;
    fn to_bound(state: Self::AddrState) -> Bound<S>;
    fn to_socket_id(id: Self::Id) -> SocketId<S>;
    fn from_socket_id_ref(id: &SocketId<S>) -> &Self::Id;
}

impl<I: Ip, D: DeviceIdentifier, A: SocketMapAddrSpec, S: SocketMapStateSpec>
    ConvertSocketMapState<I, D, A, S> for Listener
{
    type Id = S::ListenerId;
    type SharingState = S::ListenerSharingState;
    type Addr = ListenerAddr<ListenerIpAddr<I::Addr, A::LocalIdentifier>, D>;
    type AddrState = S::ListenerAddrState;
    fn to_addr_vec(addr: &Self::Addr) -> AddrVec<I, D, A> {
        AddrVec::Listen(addr.clone())
    }

    fn from_addr_vec_ref(addr: &AddrVec<I, D, A>) -> &Self::Addr {
        match addr {
            AddrVec::Listen(l) => l,
            AddrVec::Conn(c) => unreachable!("conn addr for listener: {c:?}"),
        }
    }

    fn from_bound_ref(bound: &Bound<S>) -> Option<&S::ListenerAddrState> {
        match bound {
            Bound::Listen(l) => Some(l),
            Bound::Conn(_c) => None,
        }
    }

    fn from_bound_mut(bound: &mut Bound<S>) -> Option<&mut S::ListenerAddrState> {
        match bound {
            Bound::Listen(l) => Some(l),
            Bound::Conn(_c) => None,
        }
    }

    fn to_bound(state: S::ListenerAddrState) -> Bound<S> {
        Bound::Listen(state)
    }
    fn from_socket_id_ref(id: &SocketId<S>) -> &Self::Id {
        match id {
            SocketId::Listener(id) => id,
            SocketId::Connection(_) => unreachable!("connection ID for listener"),
        }
    }
    fn to_socket_id(id: Self::Id) -> SocketId<S> {
        SocketId::Listener(id)
    }
}

impl<I: Ip, D: DeviceIdentifier, A: SocketMapAddrSpec, S: SocketMapStateSpec>
    ConvertSocketMapState<I, D, A, S> for Connection
{
    type Id = S::ConnId;
    type SharingState = S::ConnSharingState;
    type Addr = ConnAddr<ConnIpAddr<I::Addr, A::LocalIdentifier, A::RemoteIdentifier>, D>;
    type AddrState = S::ConnAddrState;
    fn to_addr_vec(addr: &Self::Addr) -> AddrVec<I, D, A> {
        AddrVec::Conn(addr.clone())
    }

    fn from_addr_vec_ref(addr: &AddrVec<I, D, A>) -> &Self::Addr {
        match addr {
            AddrVec::Conn(c) => c,
            AddrVec::Listen(l) => unreachable!("listener addr for conn: {l:?}"),
        }
    }

    fn from_bound_ref(bound: &Bound<S>) -> Option<&S::ConnAddrState> {
        match bound {
            Bound::Listen(_l) => None,
            Bound::Conn(c) => Some(c),
        }
    }

    fn from_bound_mut(bound: &mut Bound<S>) -> Option<&mut S::ConnAddrState> {
        match bound {
            Bound::Listen(_l) => None,
            Bound::Conn(c) => Some(c),
        }
    }

    fn to_bound(state: S::ConnAddrState) -> Bound<S> {
        Bound::Conn(state)
    }

    fn from_socket_id_ref(id: &SocketId<S>) -> &Self::Id {
        match id {
            SocketId::Connection(id) => id,
            SocketId::Listener(_) => unreachable!("listener ID for connection"),
        }
    }
    fn to_socket_id(id: Self::Id) -> SocketId<S> {
        SocketId::Connection(id)
    }
}

/// An identifier of a sharing domain used for SO_REUSEPORT.
#[derive(Debug, Eq, PartialEq, Clone, Copy, Hash)]
pub struct SharingDomain(u64);

impl SharingDomain {
    /// Creates a new instance with the specified ID. Caller must ensure that the `id`
    /// uniquely identifies the sharing domain and that the client is authorized to use it,
    /// e.g. on Fuchsia the ID is the KOID of a handle provided by the client.
    pub const fn new(id: u64) -> Self {
        SharingDomain(id)
    }
}

/// A value of the SO_REUSEPORT option. Also encodes the sharing domain, which allows
/// to ensure that only sockets in the same domain can share ports.
#[derive(Default, Debug, Eq, PartialEq, Clone, Copy, Hash)]
pub enum ReusePortOption {
    /// The option is disabled.
    #[default]
    Disabled,

    /// The option is enabled: the port is shareable with other sockets in the
    /// same sharing domain.
    Enabled(SharingDomain),
}

impl ReusePortOption {
    /// Returns `true` if the option is enabled.
    pub fn is_enabled(&self) -> bool {
        matches!(self, ReusePortOption::Enabled(_))
    }

    /// Returns `true` if the socket is shareable with a socket with the
    /// specified value of the SO_REUSEPORT option.
    pub fn is_shareable_with(&self, other: &Self) -> bool {
        match (self, other) {
            (ReusePortOption::Enabled(domain1), ReusePortOption::Enabled(domain2)) => {
                domain1 == domain2
            }
            _ => false,
        }
    }
}

/// A trait abstracting over the data required to calculate socket
/// diagnostics. Allows delaying the computation arbitrarily long.
pub trait SocketDiagnosticsSeed {
    /// The concrete output type.
    type Output;

    /// Completes the diagnostics construction.
    fn resolve(self) -> Option<Self::Output>;
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use assert_matches::assert_matches;
    use net_declare::{net_ip_v4, net_ip_v6};
    use net_types::ip::{Ipv4Addr, Ipv6, Ipv6Addr};
    use netstack3_hashmap::HashSet;
    use test_case::test_case;

    use crate::device::testutil::{FakeDeviceId, FakeWeakDeviceId, MultipleDevicesId};
    use crate::testutil::set_logger_for_test;

    use super::*;

    #[test_case(net_ip_v4!("8.8.8.8"))]
    #[test_case(net_ip_v4!("127.0.0.1"))]
    #[test_case(net_ip_v4!("127.0.8.9"))]
    #[test_case(net_ip_v4!("224.1.2.3"))]
    fn must_never_have_zone_ipv4(addr: Ipv4Addr) {
        // No IPv4 addresses are allowed to have a zone.
        let addr = SpecifiedAddr::new(addr).unwrap();
        assert_eq!(addr.must_have_zone(), false);
    }

    #[test_case(net_ip_v6!("1::2:3"), false)]
    #[test_case(net_ip_v6!("::1"), false; "localhost")]
    #[test_case(net_ip_v6!("1::"), false)]
    #[test_case(net_ip_v6!("ff03:1:2:3::1"), false)]
    #[test_case(net_ip_v6!("ff02:1:2:3::1"), true)]
    #[test_case(Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS.get(), true)]
    #[test_case(net_ip_v6!("fe80::1"), true)]
    fn must_have_zone_ipv6(addr: Ipv6Addr, must_have: bool) {
        // Only link-local unicast and multicast addresses are allowed to have
        // zones.
        let addr = SpecifiedAddr::new(addr).unwrap();
        assert_eq!(addr.must_have_zone(), must_have);
    }

    #[test]
    fn try_into_null_zoned_ipv6() {
        assert_eq!(Ipv6::LOOPBACK_ADDRESS.try_into_null_zoned(), None);
        let zoned = Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS.into_specified();
        const ZONE: u32 = 5;
        assert_eq!(
            zoned.try_into_null_zoned().map(|a| a.map_zone(|()| ZONE)),
            Some(AddrAndZone::new(zoned, ZONE).unwrap())
        );
    }

    enum FakeSpec {}

    #[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
    struct Listener(usize);

    #[derive(PartialEq, Eq, Debug, Copy, Clone)]
    struct SharingState {
        tag: char,
        shared: bool,
    }

    impl SharingState {
        fn exclusive(tag: char) -> Self {
            Self { tag, shared: false }
        }

        fn shared(tag: char) -> Self {
            Self { tag, shared: true }
        }
    }

    impl SharingState {
        fn can_share_with(&self, other: &Self) -> bool {
            self.tag == other.tag && self.shared && other.shared
        }
    }

    #[derive(PartialEq, Eq, Debug)]
    struct Multiple<T> {
        sharing_state: SharingState,
        entries: Vec<T>,
    }

    impl<T> Multiple<T> {
        fn new_exclusive(tag: char, entries: Vec<T>) -> Self {
            Self { sharing_state: SharingState { tag, shared: false }, entries }
        }
    }

    #[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
    struct Conn(usize);

    enum FakeAddrSpec {}

    impl SocketMapAddrSpec for FakeAddrSpec {
        type LocalIdentifier = NonZeroU16;
        type RemoteIdentifier = ();
    }

    impl SocketMapStateSpec for FakeSpec {
        type AddrVecTag = SharingState;

        type ListenerId = Listener;
        type ConnId = Conn;

        type ListenerSharingState = SharingState;
        type ConnSharingState = SharingState;

        type ListenerAddrState = Multiple<Listener>;
        type ConnAddrState = Multiple<Conn>;

        fn listener_tag(_: ListenerAddrInfo, state: &Self::ListenerAddrState) -> Self::AddrVecTag {
            state.sharing_state
        }

        fn connected_tag(_has_device: bool, state: &Self::ConnAddrState) -> Self::AddrVecTag {
            state.sharing_state
        }
    }

    type FakeBoundSocketMap =
        BoundSocketMap<Ipv4, FakeWeakDeviceId<FakeDeviceId>, FakeAddrSpec, FakeSpec>;

    /// Generator for unique socket IDs that don't have any state.
    ///
    /// Calling [`FakeSocketIdGen::next`] returns a unique ID.
    #[derive(Default)]
    struct FakeSocketIdGen {
        next_id: usize,
    }

    impl FakeSocketIdGen {
        fn next(&mut self) -> usize {
            let next_next_id = self.next_id + 1;
            core::mem::replace(&mut self.next_id, next_next_id)
        }
    }

    impl<I: Eq> SocketMapAddrStateSpec for Multiple<I> {
        type Id = I;
        type SharingState = SharingState;
        type Inserter<'a>
            = &'a mut Vec<I>
        where
            I: 'a;

        fn new(sharing_state: &SharingState, id: I) -> Self {
            Self { sharing_state: *sharing_state, entries: vec![id] }
        }

        fn contains_id(&self, id: &Self::Id) -> bool {
            self.entries.contains(id)
        }

        fn try_get_inserter<'a, 'b>(
            &'b mut self,
            new_sharing_state: &'a SharingState,
        ) -> Result<Self::Inserter<'b>, IncompatibleError> {
            (self.sharing_state == *new_sharing_state)
                .then_some(&mut self.entries)
                .ok_or(IncompatibleError)
        }

        fn could_insert(&self, new_sharing_state: &SharingState) -> Result<(), IncompatibleError> {
            (self.sharing_state == *new_sharing_state).then_some(()).ok_or(IncompatibleError)
        }

        fn remove_by_id(&mut self, id: I) -> RemoveResult {
            let index = self.entries.iter().position(|i| i == &id).expect("did not find id");
            let _: I = self.entries.swap_remove(index);
            if self.entries.is_empty() { RemoveResult::IsLast } else { RemoveResult::Success }
        }

        fn sharing_state(&self) -> Self::SharingState {
            self.sharing_state
        }
    }

    impl<D: DeviceIdentifier, A: Into<AddrVec<Ipv4, D, FakeAddrSpec>> + Clone>
        SocketMapConflictPolicy<A, SharingState, Ipv4, D, FakeAddrSpec> for FakeSpec
    {
        fn check_insert_conflicts(
            new_sharing_state: &SharingState,
            addr: &A,
            socketmap: &SocketMap<AddrVec<Ipv4, D, FakeAddrSpec>, Bound<FakeSpec>>,
        ) -> Result<(), InsertError> {
            let dest: AddrVec<_, _, _> = addr.clone().into();
            if dest.iter_shadows().any(|a| {
                let entry = socketmap.get(&a);
                match entry {
                    Some(Bound::Listen(Multiple { sharing_state, .. }))
                    | Some(Bound::Conn(Multiple { sharing_state, .. })) => {
                        !sharing_state.can_share_with(new_sharing_state)
                    }
                    None => false,
                }
            }) {
                return Err(InsertError::ShadowAddrExists);
            }

            match socketmap.get(&dest) {
                Some(Bound::Listen(Multiple { sharing_state, .. }))
                | Some(Bound::Conn(Multiple { sharing_state, .. })) => {
                    // Require that all sockets inserted in a `Multiple` entry
                    // have the same sharing state.
                    if sharing_state != new_sharing_state {
                        return Err(InsertError::Exists);
                    }
                }
                None => (),
            }

            if socketmap
                .descendant_counts(&dest)
                .any(|(sharing_state, _count)| !sharing_state.can_share_with(new_sharing_state))
            {
                Err(InsertError::WouldShadowExisting)
            } else {
                Ok(())
            }
        }
    }

    impl<I: Eq> SocketMapAddrStateUpdateSharingSpec for Multiple<I> {
        fn try_update_sharing(
            &mut self,
            id: Self::Id,
            new_sharing_state: &Self::SharingState,
        ) -> Result<(), IncompatibleError> {
            if self.sharing_state == *new_sharing_state {
                return Ok(());
            }

            // Preserve the invariant that all sockets inserted in a `Multiple`
            // entry have the same sharing state. That means we can't change
            // the sharing state of all the sockets at the address unless there
            // is exactly one!
            if self.entries.len() != 1 {
                return Err(IncompatibleError);
            }
            assert!(self.entries.contains(&id));
            self.sharing_state = *new_sharing_state;
            Ok(())
        }
    }

    impl<D: DeviceIdentifier, A: Into<AddrVec<Ipv4, D, FakeAddrSpec>> + Clone>
        SocketMapUpdateSharingPolicy<A, SharingState, Ipv4, D, FakeAddrSpec> for FakeSpec
    {
        fn allows_sharing_update(
            _socketmap: &SocketMap<AddrVec<Ipv4, D, FakeAddrSpec>, Bound<Self>>,
            _addr: &A,
            _old_sharing: &SharingState,
            _new_sharing_state: &SharingState,
        ) -> Result<(), UpdateSharingError> {
            Ok(())
        }
    }

    const LISTENER_ADDR: ListenerAddr<
        ListenerIpAddr<Ipv4Addr, NonZeroU16>,
        FakeWeakDeviceId<FakeDeviceId>,
    > = ListenerAddr {
        ip: ListenerIpAddr {
            addr: Some(unsafe { SocketIpAddr::new_unchecked(net_ip_v4!("1.2.3.4")) }),
            identifier: NonZeroU16::new(1).unwrap(),
        },
        device: None,
    };

    const CONN_ADDR: ConnAddr<
        ConnIpAddr<Ipv4Addr, NonZeroU16, ()>,
        FakeWeakDeviceId<FakeDeviceId>,
    > = ConnAddr {
        ip: ConnIpAddr {
            local: (
                unsafe { SocketIpAddr::new_unchecked(net_ip_v4!("5.6.7.8")) },
                NonZeroU16::new(1).unwrap(),
            ),
            remote: unsafe { (SocketIpAddr::new_unchecked(net_ip_v4!("8.7.6.5")), ()) },
        },
        device: None,
    };

    #[test]
    fn bound_insert_get_remove_listener() {
        set_logger_for_test();
        let mut bound = FakeBoundSocketMap::default();
        let mut fake_id_gen = FakeSocketIdGen::default();

        let addr = LISTENER_ADDR;

        let id = {
            let entry = bound
                .listeners_mut()
                .try_insert(addr, SharingState::exclusive('v'), Listener(fake_id_gen.next()))
                .unwrap();
            assert_eq!(entry.get_addr(), &addr);
            entry.id().clone()
        };

        assert_eq!(
            bound.listeners().get_by_addr(&addr),
            Some(&Multiple::new_exclusive('v', vec![id]))
        );

        assert_eq!(bound.listeners_mut().remove(&id, &addr), Ok(()));
        assert_eq!(bound.listeners().get_by_addr(&addr), None);
    }

    #[test]
    fn bound_insert_get_remove_conn() {
        set_logger_for_test();
        let mut bound = FakeBoundSocketMap::default();
        let mut fake_id_gen = FakeSocketIdGen::default();

        let addr = CONN_ADDR;

        let id = {
            let entry = bound
                .conns_mut()
                .try_insert(addr, SharingState::exclusive('v'), Conn(fake_id_gen.next()))
                .unwrap();
            assert_eq!(entry.get_addr(), &addr);
            entry.id().clone()
        };

        assert_eq!(bound.conns().get_by_addr(&addr), Some(&Multiple::new_exclusive('v', vec![id])));

        assert_eq!(bound.conns_mut().remove(&id, &addr), Ok(()));
        assert_eq!(bound.conns().get_by_addr(&addr), None);
    }

    #[test]
    fn bound_iter_addrs() {
        set_logger_for_test();
        let mut bound = FakeBoundSocketMap::default();
        let mut fake_id_gen = FakeSocketIdGen::default();

        let listener_addrs = [
            (Some(net_ip_v4!("1.1.1.1")), 1),
            (Some(net_ip_v4!("2.2.2.2")), 2),
            (Some(net_ip_v4!("1.1.1.1")), 3),
            (None, 4),
        ]
        .map(|(ip, identifier)| ListenerAddr {
            device: None,
            ip: ListenerIpAddr {
                addr: ip.map(|x| SocketIpAddr::new(x).unwrap()),
                identifier: NonZeroU16::new(identifier).unwrap(),
            },
        });
        let conn_addrs = [
            (net_ip_v4!("3.3.3.3"), 3, net_ip_v4!("4.4.4.4")),
            (net_ip_v4!("4.4.4.4"), 3, net_ip_v4!("3.3.3.3")),
        ]
        .map(|(local_ip, local_identifier, remote_ip)| ConnAddr {
            ip: ConnIpAddr {
                local: (
                    SocketIpAddr::new(local_ip).unwrap(),
                    NonZeroU16::new(local_identifier).unwrap(),
                ),
                remote: (SocketIpAddr::new(remote_ip).unwrap(), ()),
            },
            device: None,
        });

        for addr in listener_addrs.iter().cloned() {
            let _entry = bound
                .listeners_mut()
                .try_insert(addr, SharingState::exclusive('a'), Listener(fake_id_gen.next()))
                .unwrap();
        }
        for addr in conn_addrs.iter().cloned() {
            let _entry = bound
                .conns_mut()
                .try_insert(addr, SharingState::exclusive('a'), Conn(fake_id_gen.next()))
                .unwrap();
        }
        let expected_addrs = listener_addrs
            .into_iter()
            .map(Into::into)
            .chain(conn_addrs.into_iter().map(Into::into))
            .collect::<HashSet<_>>();

        assert_eq!(expected_addrs, bound.iter_addrs().cloned().collect());
    }

    #[test]
    fn try_insert_with_callback_not_called_on_error() {
        // TODO(https://fxbug.dev/42076891): remove this test along with
        // try_insert_with.
        set_logger_for_test();
        let mut bound = FakeBoundSocketMap::default();
        let addr = LISTENER_ADDR;

        // Insert a listener so that future calls can conflict.
        let _: &Listener = bound
            .listeners_mut()
            .try_insert(addr, SharingState::exclusive('a'), Listener(0))
            .unwrap()
            .id();

        // All of the below try_insert_with calls should fail, but more
        // importantly, they should not call the `make_id` callback (because it
        // is only called once success is certain).
        fn is_never_called<A, B, T>(_: A, _: B) -> (T, ()) {
            panic!("should never be called");
        }

        assert_matches!(
            bound.listeners_mut().try_insert_with(
                addr,
                SharingState::exclusive('b'),
                is_never_called
            ),
            Err(InsertError::Exists)
        );
        assert_matches!(
            bound.listeners_mut().try_insert_with(
                ListenerAddr { device: Some(FakeWeakDeviceId(FakeDeviceId)), ..addr },
                SharingState::exclusive('b'),
                is_never_called
            ),
            Err(InsertError::ShadowAddrExists)
        );
        assert_matches!(
            bound.conns_mut().try_insert_with(
                ConnAddr {
                    device: None,
                    ip: ConnIpAddr {
                        local: (addr.ip.addr.unwrap(), addr.ip.identifier),
                        remote: (SocketIpAddr::new(net_ip_v4!("1.1.1.1")).unwrap(), ()),
                    },
                },
                SharingState::exclusive('b'),
                is_never_called,
            ),
            Err(InsertError::ShadowAddrExists)
        );
    }

    #[test]
    fn insert_listener_conflict_with_listener() {
        set_logger_for_test();
        let mut bound = FakeBoundSocketMap::default();
        let mut fake_id_gen = FakeSocketIdGen::default();
        let addr = LISTENER_ADDR;

        let _: &Listener = bound
            .listeners_mut()
            .try_insert(addr, SharingState::exclusive('a'), Listener(fake_id_gen.next()))
            .unwrap()
            .id();
        assert_matches!(
            bound.listeners_mut().try_insert(
                addr,
                SharingState::exclusive('b'),
                Listener(fake_id_gen.next())
            ),
            Err(InsertError::Exists)
        );
    }

    #[test]
    fn insert_listener_conflict_with_shadower() {
        set_logger_for_test();
        let mut bound = FakeBoundSocketMap::default();
        let mut fake_id_gen = FakeSocketIdGen::default();
        let addr = LISTENER_ADDR;
        let shadows_addr = {
            assert_eq!(addr.device, None);
            ListenerAddr { device: Some(FakeWeakDeviceId(FakeDeviceId)), ..addr }
        };

        let _: &Listener = bound
            .listeners_mut()
            .try_insert(addr, SharingState::exclusive('a'), Listener(fake_id_gen.next()))
            .unwrap()
            .id();
        assert_matches!(
            bound.listeners_mut().try_insert(
                shadows_addr,
                SharingState::exclusive('b'),
                Listener(fake_id_gen.next())
            ),
            Err(InsertError::ShadowAddrExists)
        );
    }

    #[test]
    fn insert_conn_conflict_with_listener() {
        set_logger_for_test();
        let mut bound = FakeBoundSocketMap::default();
        let mut fake_id_gen = FakeSocketIdGen::default();
        let addr = LISTENER_ADDR;
        let shadows_addr = ConnAddr {
            device: None,
            ip: ConnIpAddr {
                local: (addr.ip.addr.unwrap(), addr.ip.identifier),
                remote: (SocketIpAddr::new(net_ip_v4!("1.1.1.1")).unwrap(), ()),
            },
        };

        let _: &Listener = bound
            .listeners_mut()
            .try_insert(addr, SharingState::exclusive('a'), Listener(fake_id_gen.next()))
            .unwrap()
            .id();
        assert_matches!(
            bound.conns_mut().try_insert(
                shadows_addr,
                SharingState::exclusive('b'),
                Conn(fake_id_gen.next())
            ),
            Err(InsertError::ShadowAddrExists)
        );
    }

    #[test]
    fn insert_and_remove_listener() {
        set_logger_for_test();
        let mut bound = FakeBoundSocketMap::default();
        let mut fake_id_gen = FakeSocketIdGen::default();
        let addr = LISTENER_ADDR;

        let a = bound
            .listeners_mut()
            .try_insert(addr, SharingState::exclusive('x'), Listener(fake_id_gen.next()))
            .unwrap()
            .id()
            .clone();
        let b = bound
            .listeners_mut()
            .try_insert(addr, SharingState::exclusive('x'), Listener(fake_id_gen.next()))
            .unwrap()
            .id()
            .clone();
        assert_ne!(a, b);

        assert_eq!(bound.listeners_mut().remove(&a, &addr), Ok(()));
        assert_eq!(
            bound.listeners().get_by_addr(&addr),
            Some(&Multiple::new_exclusive('x', vec![b]))
        );
    }

    #[test]
    fn insert_and_remove_conn() {
        set_logger_for_test();
        let mut bound = FakeBoundSocketMap::default();
        let mut fake_id_gen = FakeSocketIdGen::default();
        let addr = CONN_ADDR;

        let a = bound
            .conns_mut()
            .try_insert(addr, SharingState::exclusive('x'), Conn(fake_id_gen.next()))
            .unwrap()
            .id()
            .clone();
        let b = bound
            .conns_mut()
            .try_insert(addr, SharingState::exclusive('x'), Conn(fake_id_gen.next()))
            .unwrap()
            .id()
            .clone();
        assert_ne!(a, b);

        assert_eq!(bound.conns_mut().remove(&a, &addr), Ok(()));
        assert_eq!(bound.conns().get_by_addr(&addr), Some(&Multiple::new_exclusive('x', vec![b])));
    }

    #[test]
    fn update_listener_to_shadowed_addr_fails() {
        let mut bound = FakeBoundSocketMap::default();
        let mut fake_id_gen = FakeSocketIdGen::default();

        let first_addr = LISTENER_ADDR;
        let second_addr = ListenerAddr {
            ip: ListenerIpAddr {
                addr: Some(SocketIpAddr::new(net_ip_v4!("1.1.1.1")).unwrap()),
                ..LISTENER_ADDR.ip
            },
            ..LISTENER_ADDR
        };
        let both_shadow = ListenerAddr {
            ip: ListenerIpAddr { addr: None, identifier: first_addr.ip.identifier },
            device: None,
        };

        let first = bound
            .listeners_mut()
            .try_insert(first_addr, SharingState::exclusive('a'), Listener(fake_id_gen.next()))
            .unwrap()
            .id()
            .clone();
        let second = bound
            .listeners_mut()
            .try_insert(second_addr, SharingState::exclusive('b'), Listener(fake_id_gen.next()))
            .unwrap()
            .id()
            .clone();

        // Moving from (1, "aaa") to (1, None) should fail since it is shadowed
        // by (1, "yyy"), and vise versa.
        let (ExistsError, entry) = bound
            .listeners_mut()
            .entry(&second, &second_addr)
            .unwrap()
            .try_update_addr(both_shadow)
            .expect_err("update should fail");

        // The entry should correspond to `second`.
        assert_eq!(entry.id(), &second);
        drop(entry);

        let (ExistsError, entry) = bound
            .listeners_mut()
            .entry(&first, &first_addr)
            .unwrap()
            .try_update_addr(both_shadow)
            .expect_err("update should fail");
        assert_eq!(entry.get_addr(), &first_addr);
    }

    #[test]
    fn update_listener_to_conflicting_addr_fails() {
        let mut bound = BoundSocketMap::<
            Ipv4,
            FakeWeakDeviceId<MultipleDevicesId>,
            FakeAddrSpec,
            FakeSpec,
        >::default();
        let mut fake_id_gen = FakeSocketIdGen::default();
        let device_a_wildcard_addr = ListenerAddr {
            ip: ListenerIpAddr { addr: None, identifier: NonZeroU16::new(80).unwrap() },
            device: Some(FakeWeakDeviceId(MultipleDevicesId::A)),
        };
        let device_b_specific_addr = ListenerAddr {
            ip: ListenerIpAddr {
                addr: Some(SocketIpAddr::new(net_ip_v4!("192.168.1.1")).unwrap()),
                identifier: NonZeroU16::new(80).unwrap(),
            },
            device: Some(FakeWeakDeviceId(MultipleDevicesId::B)),
        };
        let device_a_specific_addr = ListenerAddr {
            ip: ListenerIpAddr {
                addr: Some(SocketIpAddr::new(net_ip_v4!("192.168.1.1")).unwrap()),
                identifier: NonZeroU16::new(80).unwrap(),
            },
            device: Some(FakeWeakDeviceId(MultipleDevicesId::A)),
        };

        let _ = bound
            .listeners_mut()
            .try_insert(
                device_a_wildcard_addr,
                SharingState::exclusive('a'),
                Listener(fake_id_gen.next()),
            )
            .expect("binding wildcard listener should succeed");

        // Binding to a specific listener on device A should fail.
        assert_matches!(
            bound.listeners_mut().try_insert(
                device_a_specific_addr,
                SharingState::exclusive('b'),
                Listener(fake_id_gen.next()),
            ),
            Err(_)
        );

        // Binding to a specific listener on device B and then moving it to
        // device A should also fail.
        let specific_entry = bound
            .listeners_mut()
            .try_insert(
                device_b_specific_addr,
                SharingState::exclusive('b'),
                Listener(fake_id_gen.next()),
            )
            .expect("binding dev A specific listener should succeed");

        assert_matches!(specific_entry.try_update_addr(device_a_specific_addr), Err(_));
    }

    #[test]
    fn nonexistent_conn_entry() {
        let mut map = FakeBoundSocketMap::default();
        let mut fake_id_gen = FakeSocketIdGen::default();
        let addr = CONN_ADDR;
        let conn_id = map
            .conns_mut()
            .try_insert(addr.clone(), SharingState::exclusive('a'), Conn(fake_id_gen.next()))
            .expect("failed to insert")
            .id()
            .clone();
        assert_matches!(map.conns_mut().remove(&conn_id, &addr), Ok(()));

        assert!(map.conns_mut().entry(&conn_id, &addr).is_none());
    }

    #[test]
    fn update_conn_sharing() {
        let mut map = FakeBoundSocketMap::default();
        let mut fake_id_gen = FakeSocketIdGen::default();
        let addr = CONN_ADDR;
        let mut entry = map
            .conns_mut()
            .try_insert(addr.clone(), SharingState::exclusive('a'), Conn(fake_id_gen.next()))
            .expect("failed to insert");

        entry
            .try_update_sharing(&SharingState::exclusive('a'), SharingState::exclusive('d'))
            .expect("worked");
        // Updating sharing is only allowed if there are no other occupants at
        // the address.
        let mut second_conn = map
            .conns_mut()
            .try_insert(addr.clone(), SharingState::exclusive('d'), Conn(fake_id_gen.next()))
            .expect("can insert");
        assert_matches!(
            second_conn
                .try_update_sharing(&SharingState::exclusive('d'), SharingState::exclusive('e')),
            Err(UpdateSharingError)
        );
    }

    #[test]
    fn lookup_connected() {
        let mut map = FakeBoundSocketMap::default();
        let mut fake_id_gen = FakeSocketIdGen::default();

        let sharing_state = SharingState::shared('a');

        let device_id = FakeWeakDeviceId(FakeDeviceId);
        let entry1 = map
            .conns_mut()
            .try_insert(CONN_ADDR, sharing_state, Conn(fake_id_gen.next()))
            .expect("failed to insert")
            .id()
            .clone();
        let conn = map
            .lookup_connected(CONN_ADDR.ip.remote, CONN_ADDR.ip.local, device_id)
            .expect("lookup should succeed");
        assert!(conn.contains_id(&entry1));

        // Add a second entry with a device ID. This one should be preferred
        // over the first one.
        let addr_with_device = ConnAddr { device: Some(device_id), ..CONN_ADDR };
        let entry2 = map
            .conns_mut()
            .try_insert(addr_with_device, sharing_state, Conn(fake_id_gen.next()))
            .expect("failed to insert")
            .id()
            .clone();
        let conn = map
            .lookup_connected(CONN_ADDR.ip.remote, CONN_ADDR.ip.local, device_id)
            .expect("lookup should succeed");
        assert!(conn.contains_id(&entry2));
    }
}
