// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Trait definition for matchers.

use alloc::format;
use alloc::string::String;
use core::convert::Infallible as Never;
use core::fmt::Debug;
use core::num::NonZeroU64;
use core::ops::RangeInclusive;

use bitflags::bitflags;
use derivative::Derivative;
use net_types::ip::{Ip, IpAddr, IpAddress, Ipv4Addr, Ipv6Addr, Subnet};

use crate::{InspectableValue, Inspector, Mark, MarkDomain, MarkStorage, Marks};

/// Trait defining required types for matchers provided by bindings.
///
/// Allows rules that match on device class to be installed, storing the
/// [`MatcherBindingsTypes::DeviceClass`] type at rest, while allowing Netstack3
/// Core to have Bindings provide the type since it is platform-specific.
pub trait MatcherBindingsTypes {
    /// The device class type for devices installed in the netstack.
    type DeviceClass: Clone + Debug;
    /// The type used to represent a custom filter matcher.
    ///
    /// `Clone` is required because filter validator clones rules when
    /// installing them.
    type BindingsPacketMatcher: Clone + Debug + InspectableValue;
}

/// Common pattern to define a matcher for a metadata input `T`.
///
/// Used in matching engines like filtering and routing rules.
pub trait Matcher<T> {
    /// Returns whether the provided value matches.
    fn matches(&self, actual: &T) -> bool;

    /// Returns whether the provided value is set and matches.
    fn required_matches(&self, actual: Option<&T>) -> bool {
        actual.map_or(false, |actual| self.matches(actual))
    }
}

/// Implement `Matcher` for optional matchers, so that if a matcher is left
/// unspecified, it matches all inputs by default.
impl<T, O> Matcher<T> for Option<O>
where
    O: Matcher<T>,
{
    fn matches(&self, actual: &T) -> bool {
        self.as_ref().map_or(true, |expected| expected.matches(actual))
    }

    fn required_matches(&self, actual: Option<&T>) -> bool {
        self.as_ref().map_or(true, |expected| expected.required_matches(actual))
    }
}

/// Matcher that matches IP addresses in a subnet.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct SubnetMatcher<A: IpAddress>(pub Subnet<A>);

impl<A: IpAddress> Matcher<A> for SubnetMatcher<A> {
    fn matches(&self, actual: &A) -> bool {
        let Self(matcher) = self;
        matcher.contains(actual)
    }
}

/// A matcher for network interfaces.
#[derive(Clone, Derivative, PartialEq, Eq)]
#[derivative(Debug)]
pub enum InterfaceMatcher<DeviceClass> {
    /// The ID of the interface as assigned by the netstack.
    Id(NonZeroU64),
    /// Match based on name.
    Name(String),
    /// The device class of the interface.
    DeviceClass(DeviceClass),
}

impl<DeviceClass: Debug> InspectableValue for InterfaceMatcher<DeviceClass> {
    fn record<I: Inspector>(&self, name: &str, inspector: &mut I) {
        match self {
            InterfaceMatcher::Id(id) => inspector.record_string(name, format!("Id({})", id.get())),
            InterfaceMatcher::Name(iface_name) => {
                inspector.record_string(name, format!("Name({iface_name})"))
            }
            InterfaceMatcher::DeviceClass(class) => {
                inspector.record_debug(name, format!("Class({class:?})"))
            }
        };
    }
}

/// Allows code to match on properties of an interface (ID, name, and device
/// class) without Netstack3 Core (or Bindings, in the case of the device class)
/// having to specifically expose that state.
pub trait InterfaceProperties<DeviceClass> {
    /// Returns whether the provided ID matches the interface.
    fn id_matches(&self, id: &NonZeroU64) -> bool;

    /// Returns whether the provided name matches the interface.
    fn name_matches(&self, name: &str) -> bool;

    /// Returns whether the provided device class matches the interface.
    fn device_class_matches(&self, device_class: &DeviceClass) -> bool;
}

impl<DeviceClass, I: InterfaceProperties<DeviceClass>> Matcher<I>
    for InterfaceMatcher<DeviceClass>
{
    fn matches(&self, actual: &I) -> bool {
        match self {
            InterfaceMatcher::Id(id) => actual.id_matches(id),
            InterfaceMatcher::Name(name) => actual.name_matches(name),
            InterfaceMatcher::DeviceClass(device_class) => {
                actual.device_class_matches(device_class)
            }
        }
    }
}

/// Matcher for the bound device of locally generated traffic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoundInterfaceMatcher<DeviceClass> {
    /// The packet is bound to a device which is matched by the matcher.
    Bound(InterfaceMatcher<DeviceClass>),
    /// There is no bound device.
    Unbound,
}

impl<'a, DeviceClass, D: InterfaceProperties<DeviceClass>> Matcher<Option<&'a D>>
    for BoundInterfaceMatcher<DeviceClass>
{
    fn matches(&self, actual: &Option<&'a D>) -> bool {
        match self {
            BoundInterfaceMatcher::Bound(matcher) => matcher.required_matches(actual.as_deref()),
            BoundInterfaceMatcher::Unbound => actual.is_none(),
        }
    }
}

impl<DeviceClass: Debug> InspectableValue for BoundInterfaceMatcher<DeviceClass> {
    fn record<I: Inspector>(&self, name: &str, inspector: &mut I) {
        match self {
            BoundInterfaceMatcher::Unbound => inspector.record_str(name, "Unbound"),
            BoundInterfaceMatcher::Bound(interface) => {
                inspector.record_inspectable_value(name, interface)
            }
        }
    }
}

/// A matcher to the socket mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkMatcher {
    /// Matches a packet if it is unmarked.
    Unmarked,
    /// The packet carries a mark that is in the range after masking.
    Marked {
        /// The mask to apply.
        mask: u32,
        /// Start of the range, inclusive.
        start: u32,
        /// End of the range, inclusive.
        end: u32,
        /// Inverts the meaning of the match.
        invert: bool,
    },
}

impl Matcher<Mark> for MarkMatcher {
    fn matches(&self, Mark(actual): &Mark) -> bool {
        match self {
            MarkMatcher::Unmarked => actual.is_none(),
            MarkMatcher::Marked { mask, start, end, invert } => {
                let val = actual.is_some_and(|actual| (*start..=*end).contains(&(actual & *mask)));

                if *invert { !val } else { val }
            }
        }
    }
}

/// A matcher for the mark in a specific domain..
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkInDomainMatcher {
    /// The domain of the mark to match.
    pub domain: MarkDomain,
    /// The matcher for the mark.
    pub matcher: MarkMatcher,
}

/// The 2 mark matchers a rule can specify. All non-none markers must match.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkMatchers(MarkStorage<Option<MarkMatcher>>);

impl MarkMatchers {
    /// Creates [`MarkMatcher`]s from an iterator of `(MarkDomain, MarkMatcher)`.
    ///
    /// An unspecified domain will not have a matcher.
    ///
    /// # Panics
    ///
    /// Panics if the same domain is specified more than once.
    pub fn new(matchers: impl IntoIterator<Item = (MarkDomain, MarkMatcher)>) -> Self {
        MarkMatchers(MarkStorage::new(matchers))
    }

    /// Returns an iterator over the mark matchers of all domains.
    pub fn iter(&self) -> impl Iterator<Item = (MarkDomain, &Option<MarkMatcher>)> {
        let Self(storage) = self;
        storage.iter()
    }
}

impl Matcher<Marks> for MarkMatchers {
    fn matches(&self, actual: &Marks) -> bool {
        let Self(matchers) = self;
        matchers.zip_with(actual).all(|(_domain, matcher, actual)| matcher.matches(actual))
    }
}

/// A matcher for a socket's cookie.
pub struct SocketCookieMatcher {
    /// The cookie to check against.
    pub cookie: u64,
    /// Invert the matching criterion (i.e. if the socket cookie isn't the same,
    /// it matches).
    pub invert: bool,
}

impl Matcher<u64> for SocketCookieMatcher {
    fn matches(&self, actual: &u64) -> bool {
        let val = *actual == self.cookie;
        if self.invert { !val } else { val }
    }
}

/// A matcher for transport-layer port numbers.
#[derive(Clone, Debug)]
pub struct PortMatcher {
    /// The range of port numbers in which the tested port number must fall.
    pub range: RangeInclusive<u16>,
    /// Whether to check for an "inverse" or "negative" match (in which case,
    /// if the matcher criteria do *not* apply, it *is* considered a match, and
    /// vice versa).
    pub invert: bool,
}

impl Matcher<u16> for PortMatcher {
    fn matches(&self, actual: &u16) -> bool {
        let Self { range, invert } = self;
        range.contains(actual) ^ *invert
    }
}

/// A matcher for (possibly bound) transport-layer port numbers.
#[derive(Clone, Debug)]
pub enum BoundPortMatcher {
    /// The target is bound to a port matched by the inner matcher.
    Bound(PortMatcher),
    /// The target is not bound to a specific port.
    Unbound,
}

impl Matcher<Option<u16>> for BoundPortMatcher {
    fn matches(&self, actual: &Option<u16>) -> bool {
        match (self, actual) {
            (BoundPortMatcher::Unbound, None) => true,
            (BoundPortMatcher::Bound(matcher), Some(port)) => matcher.matches(&port),
            _ => false,
        }
    }
}

bitflags! {
    /// A matcher for TCP state machine state.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct TcpStateMatcher: u32 {
        /// The TCP ESTABLISHED state.
        const ESTABLISHED = 1 << 0;
        /// The TCP SYN_SENT state.
        const SYN_SENT = 1 << 1;
        /// The TCP SYN_RECV state.
        const SYN_RECV = 1 << 2;
        /// The TCP FIN_WAIT1 state.
        const FIN_WAIT1 = 1 << 3;
        /// The TCP FIN_WAIT2 state.
        const FIN_WAIT2 = 1 << 4;
        /// The TCP TIME_WAIT state.
        const TIME_WAIT = 1 << 5;
        /// The TCP CLOSE state.
        const CLOSE = 1 << 6;
        /// The TCP CLOSE_WAIT state.
        const CLOSE_WAIT = 1 << 7;
        /// The TCP LAST_ACK state.
        const LAST_ACK = 1 << 8;
        /// The TCP LISTEN state.
        const LISTEN = 1 << 9;
        /// The TCP CLOSING state.
        const CLOSING = 1 << 10;
    }
}

impl Matcher<TcpSocketState> for TcpStateMatcher {
    fn matches(&self, actual: &TcpSocketState) -> bool {
        self.contains(actual.matcher_flag())
    }
}

/// Represents the state of a TCP socket's state machine.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum TcpSocketState {
    Established,
    SynSent,
    SynRecv,
    FinWait1,
    FinWait2,
    TimeWait,
    Close,
    CloseWait,
    LastAck,
    Listen,
    Closing,
}

impl TcpSocketState {
    fn matcher_flag(&self) -> TcpStateMatcher {
        match self {
            TcpSocketState::Established => TcpStateMatcher::ESTABLISHED,
            TcpSocketState::SynSent => TcpStateMatcher::SYN_SENT,
            TcpSocketState::SynRecv => TcpStateMatcher::SYN_RECV,
            TcpSocketState::FinWait1 => TcpStateMatcher::FIN_WAIT1,
            TcpSocketState::FinWait2 => TcpStateMatcher::FIN_WAIT2,
            TcpSocketState::TimeWait => TcpStateMatcher::TIME_WAIT,
            TcpSocketState::Close => TcpStateMatcher::CLOSE,
            TcpSocketState::CloseWait => TcpStateMatcher::CLOSE_WAIT,
            TcpSocketState::LastAck => TcpStateMatcher::LAST_ACK,
            TcpSocketState::Listen => TcpStateMatcher::LISTEN,
            TcpSocketState::Closing => TcpStateMatcher::CLOSING,
        }
    }
}

/// Allows code to match on properties of a TCP socket without Netstack3 Core
/// having to specifically expose that state.
pub trait TcpSocketProperties {
    /// Returns whether the socket's source port is matched by the matcher.
    fn src_port_matches(&self, matcher: &BoundPortMatcher) -> bool;

    /// Returns whether the socket's destination port is matched by the matcher.
    fn dst_port_matches(&self, matcher: &BoundPortMatcher) -> bool;

    /// Returns whether the socket's TCP state is matched by the matcher.
    fn state_matches(&self, matcher: &TcpStateMatcher) -> bool;
}

impl TcpSocketProperties for Never {
    fn src_port_matches(&self, _matcher: &BoundPortMatcher) -> bool {
        unimplemented!()
    }

    fn dst_port_matches(&self, _matcher: &BoundPortMatcher) -> bool {
        unimplemented!()
    }

    fn state_matches(&self, _matcher: &TcpStateMatcher) -> bool {
        unimplemented!()
    }
}

impl<T> TcpSocketProperties for &T
where
    T: TcpSocketProperties,
{
    fn src_port_matches(&self, matcher: &BoundPortMatcher) -> bool {
        (*self).src_port_matches(matcher)
    }

    fn dst_port_matches(&self, matcher: &BoundPortMatcher) -> bool {
        (*self).dst_port_matches(matcher)
    }

    fn state_matches(&self, matcher: &TcpStateMatcher) -> bool {
        (*self).state_matches(matcher)
    }
}

/// The top-level matcher for TCP sockets.
pub enum TcpSocketMatcher {
    /// Match any TCP socket without further constraints.
    Empty,
    /// Match on the source port.
    SrcPort(BoundPortMatcher),
    /// Match on the destination port.
    DstPort(BoundPortMatcher),
    /// Match on the state of the TCP state machine.
    State(TcpStateMatcher),
}

impl<T: TcpSocketProperties> Matcher<T> for TcpSocketMatcher {
    fn matches(&self, actual: &T) -> bool {
        match self {
            TcpSocketMatcher::Empty => true,
            TcpSocketMatcher::SrcPort(matcher) => actual.src_port_matches(matcher),
            TcpSocketMatcher::DstPort(matcher) => actual.dst_port_matches(matcher),
            TcpSocketMatcher::State(matcher) => actual.state_matches(matcher),
        }
    }
}

bitflags! {
    /// A matcher for UDP states.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct UdpStateMatcher: u32 {
        /// The UDP socket is bound but not connected.
        const BOUND = 1 << 0;
        /// The UDP socket is explicitly connected.
        const CONNECTED = 1 << 1;
    }
}

impl Matcher<UdpSocketState> for UdpStateMatcher {
    fn matches(&self, actual: &UdpSocketState) -> bool {
        self.contains(actual.matcher_flag())
    }
}

/// Represents the state of a UDP socket.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum UdpSocketState {
    /// The socket is bound to a local address and (maybe) port.
    Bound,
    /// The socket is connected to a remote peer and has a full 4-tuple.
    Connected,
}

impl UdpSocketState {
    fn matcher_flag(&self) -> UdpStateMatcher {
        match self {
            UdpSocketState::Bound => UdpStateMatcher::BOUND,
            UdpSocketState::Connected => UdpStateMatcher::CONNECTED,
        }
    }
}

/// Allows code to match on properties of a UDP socket without Netstack3 Core
/// having to specifically expose that state.
pub trait UdpSocketProperties {
    /// Returns whether the socket's source port is matched by the matcher.
    fn src_port_matches(&self, matcher: &BoundPortMatcher) -> bool;

    /// Returns whether the socket's destination port is matched by the matcher.
    fn dst_port_matches(&self, matcher: &BoundPortMatcher) -> bool;

    /// Returns whether the socket's UDP state is matched by the matcher.
    fn state_matches(&self, matcher: &UdpStateMatcher) -> bool;
}

impl UdpSocketProperties for Never {
    fn src_port_matches(&self, _matcher: &BoundPortMatcher) -> bool {
        unimplemented!()
    }

    fn dst_port_matches(&self, _matcher: &BoundPortMatcher) -> bool {
        unimplemented!()
    }

    fn state_matches(&self, _matcher: &UdpStateMatcher) -> bool {
        unimplemented!()
    }
}

impl<U> UdpSocketProperties for &U
where
    U: UdpSocketProperties,
{
    fn src_port_matches(&self, matcher: &BoundPortMatcher) -> bool {
        (*self).src_port_matches(matcher)
    }

    fn dst_port_matches(&self, matcher: &BoundPortMatcher) -> bool {
        (*self).dst_port_matches(matcher)
    }

    fn state_matches(&self, matcher: &UdpStateMatcher) -> bool {
        (*self).state_matches(matcher)
    }
}

/// The top-level matcher for UDP sockets.
pub enum UdpSocketMatcher {
    /// Match any UDP socket without further constraints.
    Empty,
    /// Match the source port.
    SrcPort(BoundPortMatcher),
    /// Match the destination port.
    DstPort(BoundPortMatcher),
    /// Match the UDP state.
    State(UdpStateMatcher),
}

impl<T: UdpSocketProperties> Matcher<T> for UdpSocketMatcher {
    fn matches(&self, actual: &T) -> bool {
        match self {
            UdpSocketMatcher::Empty => true,
            UdpSocketMatcher::SrcPort(matcher) => actual.src_port_matches(matcher),
            UdpSocketMatcher::DstPort(matcher) => actual.dst_port_matches(matcher),
            UdpSocketMatcher::State(matcher) => actual.state_matches(matcher),
        }
    }
}

/// Provides optional access to TCP socket properties.
pub trait MaybeSocketTransportProperties {
    /// The type that encapsulates TCP socket properties.
    type TcpProps<'a>: TcpSocketProperties
    where
        Self: 'a;

    /// The type that encapsulates UDP socket properties.
    type UdpProps<'a>: UdpSocketProperties
    where
        Self: 'a;

    /// Returns TCP socket properties if the socket is a TCP socket.
    fn tcp_socket_properties(&self) -> Option<&Self::TcpProps<'_>>;

    /// Returns UDP socket properties if the socket is a UDP socket.
    fn udp_socket_properties(&self) -> Option<&Self::UdpProps<'_>>;
}

impl MaybeSocketTransportProperties for Never {
    type TcpProps<'a>
        = Never
    where
        Self: 'a;

    type UdpProps<'a>
        = Never
    where
        Self: 'a;

    fn tcp_socket_properties(&self) -> Option<&Self::TcpProps<'_>> {
        unimplemented!()
    }

    fn udp_socket_properties(&self) -> Option<&Self::UdpProps<'_>> {
        unimplemented!()
    }
}

/// A matcher for the transport protocol of a socket.
pub enum SocketTransportProtocolMatcher {
    /// Match against a TCP socket.
    Tcp(TcpSocketMatcher),
    /// Match against a UDP socket.
    Udp(UdpSocketMatcher),
}

impl<T: MaybeSocketTransportProperties> Matcher<T> for SocketTransportProtocolMatcher {
    fn matches(&self, actual: &T) -> bool {
        match self {
            SocketTransportProtocolMatcher::Tcp(tcp_matcher) => {
                actual.tcp_socket_properties().map_or(false, |props| tcp_matcher.matches(props))
            }
            SocketTransportProtocolMatcher::Udp(udp_matcher) => {
                actual.udp_socket_properties().map_or(false, |props| udp_matcher.matches(props))
            }
        }
    }
}

/// A matcher for IP addresses.
#[derive(Clone, Derivative)]
#[derivative(Debug)]
pub enum AddressMatcherType<A: IpAddress> {
    /// A subnet that must contain the address.
    #[derivative(Debug = "transparent")]
    Subnet(SubnetMatcher<A>),
    /// An inclusive range of IP addresses that must contain the address.
    Range(RangeInclusive<A>),
}

impl<A: IpAddress> Matcher<A> for AddressMatcherType<A> {
    fn matches(&self, actual: &A) -> bool {
        match self {
            Self::Subnet(subnet_matcher) => subnet_matcher.matches(actual),
            Self::Range(range) => range.contains(actual),
        }
    }
}

/// A matcher for IP addresses.
#[derive(Clone, Debug)]
pub struct AddressMatcher<A: IpAddress> {
    /// The type of the address matcher.
    pub matcher: AddressMatcherType<A>,
    /// Whether to check for an "inverse" or "negative" match (in which case,
    /// if the matcher criteria do *not* apply, it *is* considered a match, and
    /// vice versa).
    pub invert: bool,
}

impl<A: IpAddress> AddressMatcher<A> {
    /// Creates a matcher that matches all IP addresses.
    pub fn match_all() -> Self {
        Self {
            matcher: AddressMatcherType::Subnet(SubnetMatcher(A::Version::ALL_ADDRS_SUBNET)),
            invert: false,
        }
    }
}

impl<A: IpAddress> InspectableValue for AddressMatcher<A> {
    fn record<I: Inspector>(&self, name: &str, inspector: &mut I) {
        let AddressMatcher { matcher, invert } = self;

        inspector.record_child(name, |inspector| {
            inspector.record_bool("invert", *invert);
            match matcher {
                AddressMatcherType::Subnet(SubnetMatcher(subnet)) => {
                    inspector.record_display("subnet", subnet)
                }
                AddressMatcherType::Range(range) => {
                    inspector.record_display("start", range.start());
                    inspector.record_display("end", range.end());
                }
            }
        })
    }
}

impl<A: IpAddress> Matcher<A> for AddressMatcher<A> {
    fn matches(&self, addr: &A) -> bool {
        let Self { matcher, invert } = self;
        matcher.matches(addr) ^ *invert
    }
}

/// An address matcher that matches any IP version as specified at runtime.
pub enum AddressMatcherEither {
    /// The top-level IPv4 address matcher.
    V4(AddressMatcher<Ipv4Addr>),
    /// The top-level IPv6 address matcher.
    V6(AddressMatcher<Ipv6Addr>),
}

/// An IP-generic address matcher that matches a (possibly bound) IP address.
pub enum BoundAddressMatcherEither {
    /// The target is bound to an address matched by the inner matcher.
    Bound(AddressMatcherEither),
    /// The target is not bound to a specific address.
    Unbound,
}

impl Matcher<Option<IpAddr>> for BoundAddressMatcherEither {
    fn matches(&self, addr: &Option<IpAddr>) -> bool {
        match (self, addr) {
            (BoundAddressMatcherEither::Unbound, None) => true,
            (BoundAddressMatcherEither::Bound(matcher), Some(addr)) => match (matcher, addr) {
                (AddressMatcherEither::V4(matcher), IpAddr::V4(addr)) => matcher.matches(addr),
                (AddressMatcherEither::V6(matcher), IpAddr::V6(addr)) => matcher.matches(addr),
                _ => false,
            },
            _ => false,
        }
    }
}

/// Allows code to match on properties of a socket without Netstack3 Core
/// having to specifically expose that state.
pub trait IpSocketProperties<DeviceClass> {
    /// Returns whether the provided IP version matches the socket.
    fn family_matches(&self, family: &net_types::ip::IpVersion) -> bool;

    /// Returns whether the provided address matcher matches the socket's source
    /// address.
    fn src_addr_matches(&self, addr: &BoundAddressMatcherEither) -> bool;

    /// Returns whether the provided address matcher matches the socket's
    /// destination address.
    fn dst_addr_matches(&self, addr: &BoundAddressMatcherEither) -> bool;

    /// Returns whether the transport protocol matches the socket's
    /// transport-layer information.
    fn transport_protocol_matches(&self, matcher: &SocketTransportProtocolMatcher) -> bool;

    /// Returns whether the provided interface matcher matches the socket's
    /// bound interface, if present.
    fn bound_interface_matches(&self, iface: &BoundInterfaceMatcher<DeviceClass>) -> bool;

    /// Returns whether the provided cookie matcher matches the socket's cookie.
    fn cookie_matches(&self, cookie: &SocketCookieMatcher) -> bool;

    /// Returns whether the provided mark matcher matches the corresponding mark.
    fn mark_matches(&self, matcher: &MarkInDomainMatcher) -> bool;
}

/// The top-level matcher for IP sockets.
pub enum IpSocketMatcher<DeviceClass> {
    /// Matches the socket's address family.
    Family(net_types::ip::IpVersion),
    /// Matches the socket's source address.
    SrcAddr(BoundAddressMatcherEither),
    /// Matches the socket's destination address.
    DstAddr(BoundAddressMatcherEither),
    /// Matches the socket's transport protocol.
    Proto(SocketTransportProtocolMatcher),
    /// Matches the socket's bound interface.
    BoundInterface(BoundInterfaceMatcher<DeviceClass>),
    /// Matches the socket's cookie.
    Cookie(SocketCookieMatcher),
    /// Matches the socket's mark.
    Mark(MarkInDomainMatcher),
}

impl<DeviceClass, S: IpSocketProperties<DeviceClass>> Matcher<S> for IpSocketMatcher<DeviceClass> {
    fn matches(&self, actual: &S) -> bool {
        match self {
            IpSocketMatcher::Family(family) => actual.family_matches(family),
            IpSocketMatcher::SrcAddr(addr) => actual.src_addr_matches(addr),
            IpSocketMatcher::DstAddr(addr) => actual.dst_addr_matches(addr),
            IpSocketMatcher::Proto(proto) => actual.transport_protocol_matches(proto),
            IpSocketMatcher::BoundInterface(iface) => actual.bound_interface_matches(iface),
            IpSocketMatcher::Cookie(cookie) => actual.cookie_matches(cookie),
            IpSocketMatcher::Mark(mark) => actual.mark_matches(mark),
        }
    }
}

/// Allows code to take an opaque matcher that works on IP sockets without
/// needing to know the type(s) of the underlying matcher(s).
pub trait IpSocketPropertiesMatcher<DeviceClass> {
    /// Whether the matcher matches `actual`.
    fn matches_ip_socket<S: IpSocketProperties<DeviceClass>>(&self, actual: &S) -> bool;
}

impl<DeviceClass> IpSocketPropertiesMatcher<DeviceClass> for IpSocketMatcher<DeviceClass> {
    fn matches_ip_socket<S: IpSocketProperties<DeviceClass>>(&self, actual: &S) -> bool {
        self.matches(actual)
    }
}

impl<DeviceClass> IpSocketPropertiesMatcher<DeviceClass> for [IpSocketMatcher<DeviceClass>] {
    fn matches_ip_socket<S: IpSocketProperties<DeviceClass>>(&self, actual: &S) -> bool {
        self.iter().all(|matcher| matcher.matches(actual))
    }
}

#[cfg(any(test, feature = "testutils"))]
pub(crate) mod testutil {
    use alloc::string::String;
    use core::num::NonZeroU64;

    use crate::matchers::InterfaceProperties;
    use crate::testutil::{FakeDeviceClass, FakeStrongDeviceId, FakeWeakDeviceId};
    use crate::{DeviceIdentifier, StrongDeviceIdentifier};

    /// A fake device ID for testing matchers.
    #[derive(Clone, Debug, PartialOrd, Ord, PartialEq, Eq, Hash)]
    #[allow(missing_docs)]
    pub struct FakeMatcherDeviceId {
        pub id: NonZeroU64,
        pub name: String,
        pub class: FakeDeviceClass,
    }

    impl FakeMatcherDeviceId {
        /// Returns a [`FakeMatcherDeviceId`] for an arbitrary WLAN interface.
        ///
        /// The interface returned will always be identical.
        pub fn wlan_interface() -> FakeMatcherDeviceId {
            FakeMatcherDeviceId {
                id: NonZeroU64::new(1).unwrap(),
                name: String::from("wlan"),
                class: FakeDeviceClass::Wlan,
            }
        }

        /// Returns a [`FakeMatcherDeviceId`] for an arbitrary Ethernet interface.
        ///
        /// The interface returned will always be identical.
        pub fn ethernet_interface() -> FakeMatcherDeviceId {
            FakeMatcherDeviceId {
                id: NonZeroU64::new(2).unwrap(),
                name: String::from("eth"),
                class: FakeDeviceClass::Ethernet,
            }
        }
    }

    impl StrongDeviceIdentifier for FakeMatcherDeviceId {
        type Weak = FakeWeakDeviceId<Self>;

        fn downgrade(&self) -> Self::Weak {
            FakeWeakDeviceId(self.clone())
        }
    }

    impl DeviceIdentifier for FakeMatcherDeviceId {
        fn is_loopback(&self) -> bool {
            false
        }
    }

    impl FakeStrongDeviceId for FakeMatcherDeviceId {
        fn is_alive(&self) -> bool {
            true
        }
    }

    impl PartialEq<FakeWeakDeviceId<FakeMatcherDeviceId>> for FakeMatcherDeviceId {
        fn eq(&self, FakeWeakDeviceId(other): &FakeWeakDeviceId<FakeMatcherDeviceId>) -> bool {
            self == other
        }
    }

    impl InterfaceProperties<FakeDeviceClass> for FakeMatcherDeviceId {
        fn id_matches(&self, id: &NonZeroU64) -> bool {
            &self.id == id
        }

        fn name_matches(&self, name: &str) -> bool {
            &self.name == name
        }

        fn device_class_matches(&self, class: &FakeDeviceClass) -> bool {
            &self.class == class
        }
    }
}

#[cfg(test)]
mod tests {
    use ip_test_macro::ip_test;
    use net_types::Witness;
    use net_types::ip::{Ip, IpVersion, Ipv4, Ipv6};
    use test_case::test_case;

    use super::*;
    use crate::testutil::{FakeDeviceClass, FakeMatcherDeviceId, TestIpExt};

    /// Only matches `true`.
    #[derive(Debug)]
    struct TrueMatcher;

    impl Matcher<bool> for TrueMatcher {
        fn matches(&self, actual: &bool) -> bool {
            *actual
        }
    }

    #[test]
    fn test_optional_matcher_optional_value() {
        assert!(TrueMatcher.matches(&true));
        assert!(!TrueMatcher.matches(&false));

        assert!(TrueMatcher.required_matches(Some(&true)));
        assert!(!TrueMatcher.required_matches(Some(&false)));
        assert!(!TrueMatcher.required_matches(None));

        assert!(Some(TrueMatcher).matches(&true));
        assert!(!Some(TrueMatcher).matches(&false));
        assert!(None::<TrueMatcher>.matches(&true));
        assert!(None::<TrueMatcher>.matches(&false));

        assert!(Some(TrueMatcher).required_matches(Some(&true)));
        assert!(!Some(TrueMatcher).required_matches(Some(&false)));
        assert!(!Some(TrueMatcher).required_matches(None));
        assert!(None::<TrueMatcher>.required_matches(Some(&true)));
        assert!(None::<TrueMatcher>.required_matches(Some(&false)));
        assert!(None::<TrueMatcher>.required_matches(None));
    }

    #[test_case(
        InterfaceMatcher::Id(FakeMatcherDeviceId::wlan_interface().id),
        FakeMatcherDeviceId::wlan_interface() => true
    )]
    #[test_case(
        InterfaceMatcher::Id(FakeMatcherDeviceId::wlan_interface().id),
        FakeMatcherDeviceId::ethernet_interface() => false
    )]
    #[test_case(
        InterfaceMatcher::Name(FakeMatcherDeviceId::wlan_interface().name),
        FakeMatcherDeviceId::wlan_interface() => true
    )]
    #[test_case(
        InterfaceMatcher::Name(FakeMatcherDeviceId::wlan_interface().name),
        FakeMatcherDeviceId::ethernet_interface() => false
    )]
    #[test_case(
        InterfaceMatcher::DeviceClass(FakeDeviceClass::Wlan),
        FakeMatcherDeviceId::wlan_interface() => true
    )]
    #[test_case(
        InterfaceMatcher::DeviceClass(FakeDeviceClass::Wlan),
        FakeMatcherDeviceId::ethernet_interface() => false
    )]
    fn interface_matcher(
        matcher: InterfaceMatcher<FakeDeviceClass>,
        device: FakeMatcherDeviceId,
    ) -> bool {
        matcher.matches(&device)
    }

    #[test_case(BoundInterfaceMatcher::Unbound, None => true)]
    #[test_case(
        BoundInterfaceMatcher::Unbound,
        Some(FakeMatcherDeviceId::wlan_interface()) => false
    )]
    #[test_case(
        BoundInterfaceMatcher::Bound(
            InterfaceMatcher::Id(FakeMatcherDeviceId::wlan_interface().id)
        ),
        None => false
    )]
    #[test_case(
        BoundInterfaceMatcher::Bound(
            InterfaceMatcher::Id(FakeMatcherDeviceId::wlan_interface().id)
        ),
        Some(FakeMatcherDeviceId::wlan_interface()) => true
    )]
    #[test_case(
        BoundInterfaceMatcher::Bound(
            InterfaceMatcher::Id(FakeMatcherDeviceId::wlan_interface().id)
        ),
        Some(FakeMatcherDeviceId::ethernet_interface()) => false
    )]
    #[test_case(
        BoundInterfaceMatcher::Bound(
            InterfaceMatcher::Name(FakeMatcherDeviceId::wlan_interface().name)
        ),
        None => false
    )]
    #[test_case(
        BoundInterfaceMatcher::Bound(
            InterfaceMatcher::Name(FakeMatcherDeviceId::wlan_interface().name)
        ),
        Some(FakeMatcherDeviceId::wlan_interface()) => true
    )]
    #[test_case(
        BoundInterfaceMatcher::Bound(
            InterfaceMatcher::Name(FakeMatcherDeviceId::wlan_interface().name)
        ),
        Some(FakeMatcherDeviceId::ethernet_interface()) => false
    )]
    #[test_case(
        BoundInterfaceMatcher::Bound(
            InterfaceMatcher::DeviceClass(FakeDeviceClass::Wlan)
        ),
        None => false
    )]
    #[test_case(
        BoundInterfaceMatcher::Bound(
            InterfaceMatcher::DeviceClass(FakeDeviceClass::Wlan)
        ),
        Some(FakeMatcherDeviceId::wlan_interface()) => true
    )]
    #[test_case(
        BoundInterfaceMatcher::Bound(
            InterfaceMatcher::DeviceClass(FakeDeviceClass::Wlan)
        ),
        Some(FakeMatcherDeviceId::ethernet_interface()) => false
    )]
    fn bound_interface_matcher(
        matcher: BoundInterfaceMatcher<FakeDeviceClass>,
        device: Option<FakeMatcherDeviceId>,
    ) -> bool {
        matcher.matches(&device.as_ref())
    }

    #[ip_test(I)]
    fn subnet_matcher<I: Ip + TestIpExt>() {
        let matcher = SubnetMatcher(I::TEST_ADDRS.subnet);
        assert!(matcher.matches(&I::TEST_ADDRS.local_ip));
        assert!(!matcher.matches(&I::get_other_remote_ip_address(1)));
    }

    #[test_case(MarkMatcher::Unmarked, Mark(None) => true; "unmarked matches none")]
    #[test_case(MarkMatcher::Unmarked, Mark(Some(0)) => false; "unmarked does not match some")]
    #[test_case(MarkMatcher::Marked {
        mask: 1,
        start: 0,
        end: 0,
        invert: false,
    }, Mark(None) => false; "marked does not match none")]
    #[test_case(MarkMatcher::Marked {
        mask: 1,
        start: 0,
        end: 0,
        invert: false,
    }, Mark(Some(0)) => true; "marked 0 mask 1 matches 0")]
    #[test_case(MarkMatcher::Marked {
        mask: 1,
        start: 0,
        end: 0,
        invert: false,
    }, Mark(Some(1)) => false; "marked 0 mask 1 does not match 1")]
    #[test_case(MarkMatcher::Marked {
        mask: 1,
        start: 0,
        end: 0,
        invert: false,
    }, Mark(Some(2)) => true; "marked 0 mask 1 matches 2")]
    #[test_case(MarkMatcher::Marked {
        mask: 1,
        start: 0,
        end: 0,
        invert: false,
    }, Mark(Some(3)) => false; "marked 0 mask 1 does not match 3")]
    #[test_case(MarkMatcher::Marked {
        mask: !0,
        start: 0,
        end: 10,
        invert: true,
    }, Mark(Some(5)) => false; "marked invert no match in range")]
    #[test_case(MarkMatcher::Marked {
        mask: !0,
        start: 0,
        end: 10,
        invert: true,
    }, Mark(Some(11)) => true; "marked invert matches out of range")]
    fn mark_matcher(matcher: MarkMatcher, mark: Mark) -> bool {
        matcher.matches(&mark)
    }

    #[test_case(
        MarkMatchers::new(
            [(MarkDomain::Mark1, MarkMatcher::Unmarked),
            (MarkDomain::Mark2, MarkMatcher::Unmarked)]
        ),
        Marks::new([]) => true;
        "all unmarked matches empty"
    )]
    #[test_case(
        MarkMatchers::new(
            [(MarkDomain::Mark1, MarkMatcher::Unmarked),
            (MarkDomain::Mark2, MarkMatcher::Unmarked)]
        ),
        Marks::new([(MarkDomain::Mark1, 1)]) => false;
        "all unmarked does not match mark1"
    )]
    #[test_case(
        MarkMatchers::new(
            [(MarkDomain::Mark1, MarkMatcher::Unmarked),
            (MarkDomain::Mark2, MarkMatcher::Unmarked)]
        ),
        Marks::new([(MarkDomain::Mark2, 1)]) => false;
        "all unmarked does not match mark2"
    )]
    #[test_case(
        MarkMatchers::new(
            [(MarkDomain::Mark1, MarkMatcher::Unmarked),
            (MarkDomain::Mark2, MarkMatcher::Unmarked)]
        ),
        Marks::new([
            (MarkDomain::Mark1, 1),
            (MarkDomain::Mark2, 1),
        ]) => false;
        "all unmarked does not match mark1 and mark2"
    )]
    #[test_case(
        MarkMatchers::new(
            [(MarkDomain::Mark1, MarkMatcher::Marked { mask: !0, start: 1, end: 1, invert: false }),
            (MarkDomain::Mark2, MarkMatcher::Unmarked)]
        ),
        Marks::new([(MarkDomain::Mark1, 1)]) => true;
        "mark1 marked matches"
    )]
    #[test_case(
        MarkMatchers::new(
            [(MarkDomain::Mark1, MarkMatcher::Marked { mask: !0, start: 1, end: 1, invert: false }),
            (MarkDomain::Mark2, MarkMatcher::Unmarked)]
        ),
        Marks::new([(MarkDomain::Mark1, 2)]) => false;
        "mark1 marked no match"
    )]
    #[test_case(
        MarkMatchers::new(
            [(MarkDomain::Mark1, MarkMatcher::Marked { mask: !0, start: 1, end: 1, invert: false }),
            (MarkDomain::Mark2, MarkMatcher::Marked { mask: !0, start: 2, end: 2, invert: false })]
        ),
        Marks::new([(MarkDomain::Mark1, 1), (MarkDomain::Mark2, 2)]) => true;
        "all marked matches"
    )]
    #[test_case(
        MarkMatchers::new(
            [(MarkDomain::Mark1, MarkMatcher::Marked { mask: !0, start: 1, end: 1, invert: false }),
            (MarkDomain::Mark2, MarkMatcher::Marked { mask: !0, start: 2, end: 2, invert: false })]
        ),
        Marks::new([(MarkDomain::Mark1, 1), (MarkDomain::Mark2, 3)]) => false;
        "all marked no match mark2"
    )]
    fn mark_matchers(matchers: MarkMatchers, marks: Marks) -> bool {
        matchers.matches(&marks)
    }

    #[test_case(SocketCookieMatcher { cookie: 123, invert: false }, 123 => true)]
    #[test_case(SocketCookieMatcher { cookie: 123, invert: false }, 456 => false)]
    #[test_case(SocketCookieMatcher { cookie: 123, invert: true }, 123 => false)]
    #[test_case(SocketCookieMatcher { cookie: 123, invert: true }, 456 => true)]
    fn socket_cookie_matcher(matcher: SocketCookieMatcher, actual: u64) -> bool {
        matcher.matches(&actual)
    }

    #[test_case(PortMatcher { range: 10..=20, invert: false }, 9 => false)]
    #[test_case(PortMatcher { range: 10..=20, invert: false }, 10 => true)]
    #[test_case(PortMatcher { range: 10..=20, invert: false }, 15 => true)]
    #[test_case(PortMatcher { range: 10..=20, invert: false }, 20 => true)]
    #[test_case(PortMatcher { range: 10..=20, invert: false }, 21 => false)]
    #[test_case(PortMatcher { range: 10..=20, invert: true }, 9 => true)]
    #[test_case(PortMatcher { range: 10..=20, invert: true }, 10 => false)]
    #[test_case(PortMatcher { range: 10..=20, invert: true }, 15 => false)]
    #[test_case(PortMatcher { range: 10..=20, invert: true }, 20 => false)]
    #[test_case(PortMatcher { range: 10..=20, invert: true }, 21 => true)]
    fn port_matcher(matcher: PortMatcher, actual: u16) -> bool {
        matcher.matches(&actual)
    }

    #[test_case(BoundPortMatcher::Unbound, None => true)]
    #[test_case(BoundPortMatcher::Unbound, Some(80) => false)]
    #[test_case(
        BoundPortMatcher::Bound(PortMatcher { range: 10..=20, invert: false }),
        None => false
    )]
    #[test_case(
        BoundPortMatcher::Bound(PortMatcher { range: 10..=20, invert: false }),
        Some(10) => true
    )]
    #[test_case(
        BoundPortMatcher::Bound(PortMatcher { range: 10..=20, invert: false }),
        Some(9) => false
    )]
    fn bound_port_matcher(matcher: BoundPortMatcher, actual: Option<u16>) -> bool {
        matcher.matches(&actual)
    }

    struct FakeTcpSocket {
        src_port: Option<u16>,
        dst_port: Option<u16>,
        state: TcpSocketState,
    }

    impl MaybeSocketTransportProperties for FakeTcpSocket {
        type TcpProps<'a>
            = Self
        where
            Self: 'a;

        type UdpProps<'a>
            = Never
        where
            Self: 'a;

        fn tcp_socket_properties(&self) -> Option<&Self::TcpProps<'_>> {
            Some(self)
        }

        fn udp_socket_properties(&self) -> Option<&Self::UdpProps<'_>> {
            None
        }
    }

    impl TcpSocketProperties for FakeTcpSocket {
        fn src_port_matches(&self, matcher: &BoundPortMatcher) -> bool {
            matcher.matches(&self.src_port)
        }

        fn dst_port_matches(&self, matcher: &BoundPortMatcher) -> bool {
            matcher.matches(&self.dst_port)
        }

        fn state_matches(&self, matcher: &TcpStateMatcher) -> bool {
            matcher.matches(&self.state)
        }
    }

    struct FakeUdpSocket {
        src_port: Option<u16>,
        dst_port: Option<u16>,
        state: UdpSocketState,
    }

    impl MaybeSocketTransportProperties for FakeUdpSocket {
        type TcpProps<'a>
            = Never
        where
            Self: 'a;

        type UdpProps<'a>
            = Self
        where
            Self: 'a;

        fn tcp_socket_properties(&self) -> Option<&Self::TcpProps<'_>> {
            None
        }

        fn udp_socket_properties(&self) -> Option<&Self::UdpProps<'_>> {
            Some(self)
        }
    }

    impl UdpSocketProperties for FakeUdpSocket {
        fn src_port_matches(&self, matcher: &BoundPortMatcher) -> bool {
            matcher.matches(&self.src_port)
        }

        fn dst_port_matches(&self, matcher: &BoundPortMatcher) -> bool {
            matcher.matches(&self.dst_port)
        }

        fn state_matches(&self, matcher: &UdpStateMatcher) -> bool {
            matcher.matches(&self.state)
        }
    }

    struct FakeIpSocket<I, T>
    where
        I: TestIpExt,
        T: MaybeSocketTransportProperties,
    {
        src_ip: Option<I::Addr>,
        dst_ip: Option<I::Addr>,
        proto: T,
        intf: Option<FakeMatcherDeviceId>,
        cookie: u64,
        marks: Marks,
    }

    impl<I, T> MaybeSocketTransportProperties for FakeIpSocket<I, T>
    where
        I: TestIpExt,
        T: MaybeSocketTransportProperties,
    {
        type TcpProps<'a>
            = T::TcpProps<'a>
        where
            Self: 'a;

        type UdpProps<'a>
            = T::UdpProps<'a>
        where
            Self: 'a;

        fn tcp_socket_properties(&self) -> Option<&Self::TcpProps<'_>> {
            self.proto.tcp_socket_properties()
        }

        fn udp_socket_properties(&self) -> Option<&Self::UdpProps<'_>> {
            self.proto.udp_socket_properties()
        }
    }

    impl<I, T> IpSocketProperties<FakeDeviceClass> for FakeIpSocket<I, T>
    where
        I: TestIpExt,
        T: MaybeSocketTransportProperties,
    {
        fn family_matches(&self, family: &net_types::ip::IpVersion) -> bool {
            *family == I::VERSION
        }

        fn src_addr_matches(&self, addr: &BoundAddressMatcherEither) -> bool {
            addr.matches(&self.src_ip.map(|a| a.into()))
        }

        fn dst_addr_matches(&self, addr: &BoundAddressMatcherEither) -> bool {
            addr.matches(&self.dst_ip.map(|a| a.into()))
        }

        fn transport_protocol_matches(&self, matcher: &SocketTransportProtocolMatcher) -> bool {
            matcher.matches(self)
        }

        fn bound_interface_matches(&self, iface: &BoundInterfaceMatcher<FakeDeviceClass>) -> bool {
            iface.matches(&self.intf.as_ref())
        }

        fn cookie_matches(&self, cookie: &SocketCookieMatcher) -> bool {
            cookie.matches(&self.cookie)
        }

        fn mark_matches(&self, matcher: &MarkInDomainMatcher) -> bool {
            matcher.matcher.matches(self.marks.get(matcher.domain))
        }
    }

    #[test_case(
        TcpSocketMatcher::Empty,
        FakeTcpSocket {
            src_port: Some(80), dst_port: Some(12345), state: TcpSocketState::Established
        } => true;
        "empty matcher"
    )]
    #[test_case(
        TcpSocketMatcher::SrcPort(BoundPortMatcher::Bound(
            PortMatcher { range: 80..=80, invert: false }
        )),
        FakeTcpSocket {
            src_port: Some(80), dst_port: Some(12345), state: TcpSocketState::Established
        } => true;
        "src_port match"
    )]
    #[test_case(
        TcpSocketMatcher::SrcPort(BoundPortMatcher::Bound(
            PortMatcher { range: 80..=80, invert: false }
        )),
        FakeTcpSocket {
            src_port: Some(81), dst_port: Some(12345), state: TcpSocketState::Established
        } => false;
        "src_port no match"
    )]
    #[test_case(
        TcpSocketMatcher::SrcPort(BoundPortMatcher::Bound(PortMatcher {
            range: 80..=80, invert: true
        })),
        FakeTcpSocket {
            src_port: Some(80), dst_port: Some(12345), state: TcpSocketState::Established
        } => false;
        "src_port invert no match"
    )]
    #[test_case(
        TcpSocketMatcher::SrcPort(BoundPortMatcher::Bound(
            PortMatcher { range: 80..=80, invert: true }
        )),
        FakeTcpSocket {
            src_port: Some(81), dst_port: Some(12345), state: TcpSocketState::Established
        } => true;
        "src_port invert match"
    )]
    #[test_case(
        TcpSocketMatcher::DstPort(BoundPortMatcher::Bound(
            PortMatcher {range: 12345..=12345, invert: false }
        )),
        FakeTcpSocket {
            src_port: Some(80), dst_port: Some(12345), state: TcpSocketState::Established
        } => true;
        "dst_port match"
    )]
    #[test_case(
        TcpSocketMatcher::DstPort(BoundPortMatcher::Bound(
            PortMatcher { range: 12345..=12345, invert: false }
        )),
        FakeTcpSocket {
            src_port: Some(80), dst_port: Some(12346), state: TcpSocketState::Established
        } => false;
        "dst_port no match"
    )]
    #[test_case(
        TcpSocketMatcher::State(TcpStateMatcher::ESTABLISHED),
        FakeTcpSocket {
            src_port: Some(80), dst_port: Some(12345), state: TcpSocketState::Established
        } => true;
        "state match"
    )]
    #[test_case(
        TcpSocketMatcher::State(TcpStateMatcher::SYN_SENT),
        FakeTcpSocket {
            src_port: Some(80), dst_port: Some(12345), state: TcpSocketState::Established
        } => false;
        "state no match"
    )]
    #[test_case(
        TcpSocketMatcher::State(TcpStateMatcher::ESTABLISHED | TcpStateMatcher::SYN_SENT),
        FakeTcpSocket {
            src_port: Some(80), dst_port: Some(12345), state: TcpSocketState::Established
        } => true;
        "state multi match established"
    )]
    #[test_case(
        TcpSocketMatcher::State(TcpStateMatcher::ESTABLISHED | TcpStateMatcher::SYN_SENT),
        FakeTcpSocket {
            src_port: Some(80), dst_port: Some(12345), state: TcpSocketState::SynSent
        } => true;
        "state multi match syn_sent"
    )]
    #[test_case(
        TcpSocketMatcher::State(TcpStateMatcher::ESTABLISHED | TcpStateMatcher::SYN_SENT),
        FakeTcpSocket {
            src_port: Some(80), dst_port: Some(12345), state: TcpSocketState::FinWait1
        } => false;
        "state multi no match"
    )]
    #[test_case(
        TcpSocketMatcher::SrcPort(BoundPortMatcher::Unbound),
        FakeTcpSocket {
            src_port: None, dst_port: Some(12345), state: TcpSocketState::Established
        } => true;
        "src_port unbound match"
    )]
    #[test_case(
        TcpSocketMatcher::SrcPort(BoundPortMatcher::Unbound),
        FakeTcpSocket {
            src_port: Some(80), dst_port: Some(12345), state: TcpSocketState::Established
        } => false;
        "src_port unbound no match"
    )]
    #[test_case(
        TcpSocketMatcher::DstPort(BoundPortMatcher::Unbound),
        FakeTcpSocket {
            src_port: Some(80), dst_port: None, state: TcpSocketState::Established
        } => true;
        "dst_port unbound match"
    )]
    #[test_case(
        TcpSocketMatcher::DstPort(BoundPortMatcher::Unbound),
        FakeTcpSocket {
            src_port: Some(80), dst_port: Some(12345), state: TcpSocketState::Established
        } => false;
        "dst_port unbound no match"
    )]
    fn tcp_socket_matcher(matcher: TcpSocketMatcher, socket: FakeTcpSocket) -> bool {
        matcher.matches(&socket)
    }

    #[test_case(
        UdpSocketMatcher::Empty,
        FakeUdpSocket {
            src_port: Some(53), dst_port: Some(12345), state: UdpSocketState::Bound
        } => true;
        "empty matcher"
    )]
    #[test_case(
        UdpSocketMatcher::SrcPort(BoundPortMatcher::Bound(
            PortMatcher { range: 53..=53, invert: false }
        )),
        FakeUdpSocket {
            src_port: Some(53), dst_port: Some(12345), state: UdpSocketState::Bound
        } => true;
        "src_port match"
    )]
    #[test_case(
        UdpSocketMatcher::SrcPort(BoundPortMatcher::Bound(
            PortMatcher { range: 53..=53, invert: false }
        )),
        FakeUdpSocket {
            src_port: Some(54), dst_port: Some(12345), state: UdpSocketState::Bound
        } => false;
        "src_port no match"
    )]
    #[test_case(
        UdpSocketMatcher::DstPort(BoundPortMatcher::Bound(
            PortMatcher { range: 12345..=12345, invert: false }
        )),
        FakeUdpSocket {
            src_port: Some(53), dst_port: Some(12345), state: UdpSocketState::Bound
         } => true;
        "dst_port match"
    )]
    #[test_case(
        UdpSocketMatcher::DstPort(BoundPortMatcher::Bound(
            PortMatcher { range: 12345..=12345, invert: false }
        )),
        FakeUdpSocket {
            src_port: Some(53), dst_port: Some(12346), state: UdpSocketState::Bound
        } => false;
        "dst_port no match"
    )]
    #[test_case(
        UdpSocketMatcher::State(UdpStateMatcher::BOUND),
        FakeUdpSocket {
            src_port: Some(53), dst_port: Some(12345), state: UdpSocketState::Bound
         } => true;
        "state match bound"
    )]
    #[test_case(
        UdpSocketMatcher::State(UdpStateMatcher::CONNECTED),
        FakeUdpSocket {
            src_port: Some(53), dst_port: Some(12345), state: UdpSocketState::Bound
        } => false;
        "state no match connected"
    )]
    #[test_case(
        UdpSocketMatcher::State(UdpStateMatcher::BOUND | UdpStateMatcher::CONNECTED),
        FakeUdpSocket {
            src_port: Some(53), dst_port: Some(12345), state: UdpSocketState::Bound
         } => true;
        "state multi match bound"
    )]
    #[test_case(
        UdpSocketMatcher::State(UdpStateMatcher::BOUND | UdpStateMatcher::CONNECTED),
        FakeUdpSocket {
            src_port: Some(53), dst_port: Some(12345), state: UdpSocketState::Connected
         } => true;
        "state multi match connected"
    )]
    #[test_case(
        UdpSocketMatcher::SrcPort(BoundPortMatcher::Unbound),
        FakeUdpSocket {
            src_port: None, dst_port: Some(12345), state: UdpSocketState::Bound
         } => true;
        "src_port unbound match"
    )]
    #[test_case(
        UdpSocketMatcher::SrcPort(BoundPortMatcher::Unbound),
        FakeUdpSocket {
            src_port: Some(53), dst_port: Some(12345), state: UdpSocketState::Bound
        } => false;
        "src_port unbound no match"
    )]
    #[test_case(
        UdpSocketMatcher::DstPort(BoundPortMatcher::Unbound),
        FakeUdpSocket { src_port: Some(53), dst_port: None, state: UdpSocketState::Bound } => true;
        "dst_port unbound match"
    )]
    #[test_case(
        UdpSocketMatcher::DstPort(BoundPortMatcher::Unbound),
        FakeUdpSocket {
            src_port: Some(53), dst_port: Some(12345), state: UdpSocketState::Bound
        } => false;
        "dst_port unbound no match"
    )]
    fn udp_socket_matcher(matcher: UdpSocketMatcher, socket: FakeUdpSocket) -> bool {
        matcher.matches(&socket)
    }

    #[ip_test(I)]
    #[test_case(
        IpSocketMatcher::Proto(SocketTransportProtocolMatcher::Tcp(TcpSocketMatcher::Empty)),
        FakeIpSocket {
            src_ip: Some(<I as TestIpExt>::TEST_ADDRS.local_ip.get()),
            dst_ip: Some(<I as TestIpExt>::TEST_ADDRS.remote_ip.get()),
            proto: FakeTcpSocket {
                src_port: Some(80),
                dst_port: Some(12345),
                state: TcpSocketState::Established,
            },
            cookie: 0,
            intf: None,
            marks: Marks::default(),
        } => true;
        "tcpm empty"
    )]
    #[test_case(
        IpSocketMatcher::Proto(SocketTransportProtocolMatcher::Tcp(TcpSocketMatcher::Empty)),
        FakeIpSocket {
            src_ip: Some(<I as TestIpExt>::TEST_ADDRS.local_ip.get()),
            dst_ip: Some(<I as TestIpExt>::TEST_ADDRS.remote_ip.get()),
            proto: FakeUdpSocket {
                src_port: Some(53),
                dst_port: Some(12345),
                state: UdpSocketState::Bound,
            },
            cookie: 0,
            intf: None,
            marks: Marks::default(),
        } => false;
        "tcp empty no match udp"
    )]
    #[test_case(
        IpSocketMatcher::Proto(SocketTransportProtocolMatcher::Udp(UdpSocketMatcher::Empty)),
        FakeIpSocket {
            src_ip: Some(<I as TestIpExt>::TEST_ADDRS.local_ip.get()),
            dst_ip: Some(<I as TestIpExt>::TEST_ADDRS.remote_ip.get()),
            proto: FakeTcpSocket {
                src_port: Some(80),
                dst_port: Some(12345),
                state: TcpSocketState::Established
            },
            cookie: 0,
            intf: None,
            marks: Marks::default(),
        } => false;
        "udp empty no match tcp"
    )]
    #[test_case(
        IpSocketMatcher::Proto(SocketTransportProtocolMatcher::Udp(UdpSocketMatcher::Empty)),
        FakeIpSocket {
            src_ip: Some(<I as TestIpExt>::TEST_ADDRS.local_ip.get()),
            dst_ip: Some(<I as TestIpExt>::TEST_ADDRS.remote_ip.get()),
            proto: FakeUdpSocket {
                src_port: Some(53),
                dst_port: Some(12345),
                state: UdpSocketState::Bound,
            },
            cookie: 0,
            intf: None,
            marks: Marks::default(),
        } => true;
        "udp empty"
    )]
    #[test_case(
        IpSocketMatcher::Proto(
            SocketTransportProtocolMatcher::Tcp(
                TcpSocketMatcher::SrcPort(BoundPortMatcher::Bound(
                    PortMatcher { range: 80..=80, invert: false }
                ))
            )
        ),
        FakeIpSocket {
            src_ip: Some(<I as TestIpExt>::TEST_ADDRS.local_ip.get()),
            dst_ip: Some(<I as TestIpExt>::TEST_ADDRS.remote_ip.get()),
            proto: FakeTcpSocket {
                src_port: Some(80),
                dst_port: Some(12345),
                state: TcpSocketState::Established,
            },
            cookie: 0,
            intf: None,
            marks: Marks::default(),
        } => true;
        "tcp src_port match"
    )]
    #[test_case(
        IpSocketMatcher::Proto(
            SocketTransportProtocolMatcher::Tcp(
                TcpSocketMatcher::SrcPort(BoundPortMatcher::Bound(
                    PortMatcher { range: 80..=80, invert: false }
                ))
            )
        ),
        FakeIpSocket {
            src_ip: Some(<I as TestIpExt>::TEST_ADDRS.local_ip.get()),
            dst_ip: Some(<I as TestIpExt>::TEST_ADDRS.remote_ip.get()),
            proto: FakeTcpSocket {
                src_port: Some(81),
                dst_port: Some(12345),
                state: TcpSocketState::Established,
            },
            cookie: 0,
            intf: None,
            marks: Marks::default(),
        } => false;
        "tcp src_port no match"
    )]
    #[test_case(
        IpSocketMatcher::Proto(
            SocketTransportProtocolMatcher::Udp(
                UdpSocketMatcher::SrcPort(BoundPortMatcher::Bound(
                    PortMatcher { range: 53..=53, invert: false }
                ))
            )
        ),
        FakeIpSocket {
            src_ip: Some(<I as TestIpExt>::TEST_ADDRS.local_ip.get()),
            dst_ip: Some(<I as TestIpExt>::TEST_ADDRS.remote_ip.get()),
            proto: FakeUdpSocket {
                src_port: Some(53),
                dst_port: Some(12345),
                state: UdpSocketState::Bound,
            },
            cookie: 0,
            intf: None,
            marks: Marks::default(),
        } => true;
        "udp src_port match"
    )]
    #[test_case(
        IpSocketMatcher::Proto(
            SocketTransportProtocolMatcher::Udp(
                UdpSocketMatcher::SrcPort(BoundPortMatcher::Bound(
                    PortMatcher { range: 53..=53, invert: false }
                ))
            )
        ),
        FakeIpSocket {
            src_ip: Some(<I as TestIpExt>::TEST_ADDRS.local_ip.get()),
            dst_ip: Some(<I as TestIpExt>::TEST_ADDRS.remote_ip.get()),
            proto: FakeUdpSocket {
                src_port: Some(54),
                dst_port: Some(12345),
                state: UdpSocketState::Bound,
            },
            cookie: 0,
            intf: None,
            marks: Marks::default(),
        } => false;
        "udp src_port no match"
    )]
    #[test_case(
        IpSocketMatcher::Cookie(SocketCookieMatcher { cookie: 123, invert: false }),
        FakeIpSocket {
            src_ip: Some(<I as TestIpExt>::TEST_ADDRS.local_ip.get()),
            dst_ip: Some(<I as TestIpExt>::TEST_ADDRS.remote_ip.get()),
            proto: FakeTcpSocket {
                src_port: Some(80),
                dst_port: Some(12345),
                state: TcpSocketState::Established,
            },
            cookie: 123,
            intf: None,
            marks: Marks::default(),
        } => true;
        "cookie match"
    )]
    #[test_case(
        IpSocketMatcher::Cookie(SocketCookieMatcher { cookie: 123, invert: false }),
        FakeIpSocket {
            src_ip: Some(<I as TestIpExt>::TEST_ADDRS.local_ip.get()),
            dst_ip: Some(<I as TestIpExt>::TEST_ADDRS.remote_ip.get()),
            proto: FakeTcpSocket {
                src_port: Some(80),
                dst_port: Some(12345),
                state: TcpSocketState::Established,
            },
            cookie: 456,
            intf: None,
            marks: Marks::default(),
        } => false;
        "cookie no match"
    )]
    #[test_case(
        IpSocketMatcher::Mark(MarkInDomainMatcher {
            domain: MarkDomain::Mark1,
            matcher: MarkMatcher::Unmarked,
        }),
        FakeIpSocket {
            src_ip: Some(<I as TestIpExt>::TEST_ADDRS.local_ip.get()),
            dst_ip: Some(<I as TestIpExt>::TEST_ADDRS.remote_ip.get()),
            proto: FakeTcpSocket {
                src_port: Some(80),
                dst_port: Some(12345),
                state: TcpSocketState::Established,
            },
            cookie: 0,
            intf: None,
            marks: Marks::default(),
        } => true;
        "mark1 unmarked match"
    )]
    #[test_case(
        IpSocketMatcher::Mark(MarkInDomainMatcher {
            domain: MarkDomain::Mark1,
            matcher: MarkMatcher::Unmarked,
        }),
        FakeIpSocket {
            src_ip: Some(<I as TestIpExt>::TEST_ADDRS.local_ip.get()),
            dst_ip: Some(<I as TestIpExt>::TEST_ADDRS.remote_ip.get()),
            proto: FakeTcpSocket {
                src_port: Some(80),
                dst_port: Some(12345),
                state: TcpSocketState::Established,
            },
            cookie: 0,
            intf: None,
            marks: Marks::new([(MarkDomain::Mark1, 1)]),
        } => false;
        "mark1 unmarked no match"
    )]
    #[test_case(
        IpSocketMatcher::Mark(MarkInDomainMatcher {
            domain: MarkDomain::Mark2,
            matcher: MarkMatcher::Unmarked,
        }),
        FakeIpSocket {
            src_ip: Some(<I as TestIpExt>::TEST_ADDRS.local_ip.get()),
            dst_ip: Some(<I as TestIpExt>::TEST_ADDRS.remote_ip.get()),
            proto: FakeTcpSocket {
                src_port: Some(80),
                dst_port: Some(12345),
                state: TcpSocketState::Established,
            },
            cookie: 0,
            intf: None,
            marks: Marks::default(),
        } => true;
        "mark2 unmarked match"
    )]
    #[test_case(
        IpSocketMatcher::Mark(MarkInDomainMatcher {
            domain: MarkDomain::Mark2,
            matcher: MarkMatcher::Unmarked,
        }),
        FakeIpSocket {
            src_ip: Some(<I as TestIpExt>::TEST_ADDRS.local_ip.get()),
            dst_ip: Some(<I as TestIpExt>::TEST_ADDRS.remote_ip.get()),
            proto: FakeTcpSocket {
                src_port: Some(80),
                dst_port: Some(12345),
                state: TcpSocketState::Established,
            },
            cookie: 0,
            intf: None,
            marks: Marks::new([(MarkDomain::Mark2, 1)]),
        } => false;
        "mark2 unmarked no match"
    )]
    #[test_case(
        IpSocketMatcher::BoundInterface(BoundInterfaceMatcher::Bound(
            InterfaceMatcher::Id(FakeMatcherDeviceId::wlan_interface().id)
        )),
        FakeIpSocket {
            src_ip: Some(<I as TestIpExt>::TEST_ADDRS.local_ip.get()),
            dst_ip: Some(<I as TestIpExt>::TEST_ADDRS.remote_ip.get()),
            proto: FakeTcpSocket {
                src_port: Some(80),
                dst_port: Some(12345),
                state: TcpSocketState::Established,
            },
            cookie: 0,
            intf: Some(FakeMatcherDeviceId::wlan_interface()),
            marks: Marks::default(),
        } => true;
        "bound_interface match"
    )]
    #[test_case(
        IpSocketMatcher::BoundInterface(BoundInterfaceMatcher::Bound(
            InterfaceMatcher::Id(FakeMatcherDeviceId::wlan_interface().id)
        )),
        FakeIpSocket {
            src_ip: Some(<I as TestIpExt>::TEST_ADDRS.local_ip.get()),
            dst_ip: Some(<I as TestIpExt>::TEST_ADDRS.remote_ip.get()),
            proto: FakeTcpSocket {
                src_port: Some(80),
                dst_port: Some(12345),
                state: TcpSocketState::Established,
            },
            cookie: 0,
            intf: Some(FakeMatcherDeviceId::ethernet_interface()),
            marks: Marks::default(),
        } => false;
        "bound_interface no match"
    )]
    #[test_case(
        IpSocketMatcher::BoundInterface(BoundInterfaceMatcher::Unbound),
        FakeIpSocket {
            src_ip: Some(<I as TestIpExt>::TEST_ADDRS.local_ip.get()),
            dst_ip: Some(<I as TestIpExt>::TEST_ADDRS.remote_ip.get()),
            proto: FakeTcpSocket {
                src_port: Some(80),
                dst_port: Some(12345),
                state: TcpSocketState::Established,
            },
            cookie: 0,
            intf: None,
            marks: Marks::default(),
        } => true;
        "bound_interface unbound match"
    )]
    #[test_case(
        IpSocketMatcher::BoundInterface(BoundInterfaceMatcher::Unbound),
        FakeIpSocket {
            src_ip: Some(<I as TestIpExt>::TEST_ADDRS.local_ip.get()),
            dst_ip: Some(<I as TestIpExt>::TEST_ADDRS.remote_ip.get()),
            proto: FakeTcpSocket {
                src_port: Some(80),
                dst_port: Some(12345),
                state: TcpSocketState::Established,
            },
            cookie: 0,
            intf: Some(FakeMatcherDeviceId::wlan_interface()),
            marks: Marks::default(),
        } => false;
        "bound_interface unbound no match"
    )]
    fn ip_socket_matcher<I: TestIpExt, T: MaybeSocketTransportProperties>(
        matcher: IpSocketMatcher<FakeDeviceClass>,
        socket: FakeIpSocket<I, T>,
    ) -> bool {
        matcher.matches(&socket)
    }

    #[ip_test(I)]
    fn address_matcher_type<I: TestIpExt>() {
        let local_ip = I::TEST_ADDRS.local_ip.get();
        let remote_ip = I::TEST_ADDRS.remote_ip.get();

        let matcher = AddressMatcherType::Subnet(SubnetMatcher(I::TEST_ADDRS.subnet));
        assert!(matcher.matches(&local_ip));
        assert!(!matcher.matches(&I::get_other_remote_ip_address(1)));

        let matcher = AddressMatcherType::Range(local_ip..=remote_ip);
        assert!(matcher.matches(&local_ip));
        assert!(matcher.matches(&remote_ip));
        assert!(!matcher.matches(&I::get_other_remote_ip_address(1)));
    }

    #[ip_test(I)]
    fn address_matcher<I: TestIpExt>() {
        let local_ip = I::TEST_ADDRS.local_ip.get();
        let remote_ip = I::TEST_ADDRS.remote_ip.get();

        let matcher = AddressMatcher {
            matcher: AddressMatcherType::Subnet(SubnetMatcher(I::TEST_ADDRS.subnet)),
            invert: false,
        };
        assert!(matcher.matches(&local_ip));
        assert!(matcher.matches(&remote_ip));
        assert!(!matcher.matches(&I::get_other_remote_ip_address(1)));

        let matcher = AddressMatcher {
            matcher: AddressMatcherType::Subnet(SubnetMatcher(I::TEST_ADDRS.subnet)),
            invert: true,
        };
        assert!(!matcher.matches(&local_ip));
        assert!(!matcher.matches(&remote_ip));
        assert!(matcher.matches(&I::get_other_remote_ip_address(1)));

        let matcher = AddressMatcher {
            matcher: AddressMatcherType::Range(local_ip..=remote_ip),
            invert: false,
        };
        assert!(matcher.matches(&local_ip));
        assert!(matcher.matches(&remote_ip));
        assert!(!matcher.matches(&I::get_other_remote_ip_address(1)));

        let matcher = AddressMatcher {
            matcher: AddressMatcherType::Range(local_ip..=remote_ip),
            invert: true,
        };
        assert!(!matcher.matches(&local_ip));
        assert!(!matcher.matches(&remote_ip));
        assert!(matcher.matches(&I::get_other_remote_ip_address(1)));
    }

    #[test]
    fn agnostic_address_matcher() {
        let v4_addr = IpAddr::V4(Ipv4Addr::new([192, 0, 2, 1]));
        let v6_addr = IpAddr::V6(Ipv6Addr::new([0x2001, 0xdb8, 0, 0, 0, 0, 0, 1]));

        let v4_subnet = Subnet::new(Ipv4Addr::new([192, 0, 2, 0]), 24).unwrap();
        let v6_subnet = Subnet::new(Ipv6Addr::new([0x2001, 0xdb8, 0, 0, 0, 0, 0, 0]), 32).unwrap();

        let v4_matcher =
            BoundAddressMatcherEither::Bound(AddressMatcherEither::V4(AddressMatcher {
                matcher: AddressMatcherType::Subnet(SubnetMatcher(v4_subnet)),
                invert: false,
            }));
        assert!(v4_matcher.matches(&Some(v4_addr)));
        assert!(!v4_matcher.matches(&Some(v6_addr)));

        let v6_matcher =
            BoundAddressMatcherEither::Bound(AddressMatcherEither::V6(AddressMatcher {
                matcher: AddressMatcherType::Subnet(SubnetMatcher(v6_subnet)),
                invert: false,
            }));
        assert!(!v6_matcher.matches(&Some(v4_addr)));
        assert!(v6_matcher.matches(&Some(v6_addr)));
    }

    #[test_case(IpSocketMatcher::Family(IpVersion::V4) => true; "v4 family matcher on v4 socket")]
    #[test_case(IpSocketMatcher::Family(IpVersion::V6) => false; "v6 family matcher on v4 socket")]
    #[test_case(IpSocketMatcher::SrcAddr(BoundAddressMatcherEither::Bound(AddressMatcherEither::V4(
        AddressMatcher {
            matcher: AddressMatcherType::Subnet(SubnetMatcher(Ipv4::TEST_ADDRS.subnet)),
            invert: false,
        }
    ))) => true; "src_addr match")]
    #[test_case(IpSocketMatcher::SrcAddr(BoundAddressMatcherEither::Bound(AddressMatcherEither::V4(
        AddressMatcher {
            matcher: AddressMatcherType::Subnet(SubnetMatcher(
                Subnet::new(Ipv4Addr::new([0, 0, 0, 0]), 32).unwrap()
            )),
            invert: false,
        }
    ))) => false; "src_addr no match")]
    #[test_case(IpSocketMatcher::DstAddr(BoundAddressMatcherEither::Bound(AddressMatcherEither::V4(
        AddressMatcher {
            matcher: AddressMatcherType::Subnet(SubnetMatcher(Ipv4::TEST_ADDRS.subnet)),
            invert: false,
        }
    ))) => true; "dst_addr match")]
    #[test_case(IpSocketMatcher::DstAddr(BoundAddressMatcherEither::Bound(AddressMatcherEither::V4(
        AddressMatcher {
            matcher: AddressMatcherType::Subnet(SubnetMatcher(
                Subnet::new(Ipv4Addr::new([0, 0, 0, 0]), 32).unwrap()
            )),
        invert: false,
    }))) => false; "dst_addr no match")]
    #[test_case(
        IpSocketMatcher::SrcAddr(BoundAddressMatcherEither::Unbound) => false;
        "src_addr unbound mismatch"
    )]
    #[test_case(
        IpSocketMatcher::DstAddr(BoundAddressMatcherEither::Unbound) => false;
        "dst_addr unbound mismatch"
    )]
    fn ip_socket_matcher_test_v4(matcher: IpSocketMatcher<FakeDeviceClass>) -> bool {
        let socket = FakeIpSocket::<Ipv4, _> {
            src_ip: Some(<Ipv4 as TestIpExt>::TEST_ADDRS.local_ip.get()),
            dst_ip: Some(<Ipv4 as TestIpExt>::TEST_ADDRS.remote_ip.get()),
            proto: FakeTcpSocket {
                src_port: Some(80),
                dst_port: Some(12345),
                state: TcpSocketState::Established,
            },
            cookie: 0,
            intf: None,
            marks: Marks::default(),
        };
        matcher.matches(&socket)
    }

    #[test_case(IpSocketMatcher::Family(IpVersion::V4) => false; "v4 family matcher on v6 socket")]
    #[test_case(IpSocketMatcher::Family(IpVersion::V6) => true; "v6 family matcher on v6 socket")]
    #[test_case(IpSocketMatcher::SrcAddr(BoundAddressMatcherEither::Bound(AddressMatcherEither::V6(
        AddressMatcher {
            matcher: AddressMatcherType::Subnet(SubnetMatcher(Ipv6::TEST_ADDRS.subnet)),
            invert: false,
        }
    ))) => true; "src_addr match v6")]
    #[test_case(IpSocketMatcher::SrcAddr(BoundAddressMatcherEither::Bound(AddressMatcherEither::V6(
        AddressMatcher {
            matcher: AddressMatcherType::Subnet(SubnetMatcher(
                Subnet::new(Ipv6Addr::new([0; 8]), 128).unwrap()
            )),
            invert: false,
        }
    ))) => false; "src_addr no match v6")]
    #[test_case(IpSocketMatcher::DstAddr(BoundAddressMatcherEither::Bound(AddressMatcherEither::V6(
        AddressMatcher {
            matcher: AddressMatcherType::Subnet(SubnetMatcher(Ipv6::TEST_ADDRS.subnet)),
            invert: false,
        }
    ))) => true; "dst_addr match v6")]
    #[test_case(IpSocketMatcher::DstAddr(BoundAddressMatcherEither::Bound(AddressMatcherEither::V6(
        AddressMatcher {
            matcher: AddressMatcherType::Subnet(SubnetMatcher(
                Subnet::new(Ipv6Addr::new([0; 8]), 128).unwrap()
            )),
            invert: false,
        }
    ))) => false; "dst_addr no match v6")]
    fn ip_socket_matcher_test_v6(matcher: IpSocketMatcher<FakeDeviceClass>) -> bool {
        let socket = FakeIpSocket::<Ipv6, _> {
            src_ip: Some(<Ipv6 as TestIpExt>::TEST_ADDRS.local_ip.get()),
            dst_ip: Some(<Ipv6 as TestIpExt>::TEST_ADDRS.remote_ip.get()),
            proto: FakeTcpSocket {
                src_port: Some(80),
                dst_port: Some(12345),
                state: TcpSocketState::Established,
            },
            cookie: 0,
            intf: None,
            marks: Marks::default(),
        };
        matcher.matches(&socket)
    }

    #[test_case(
        IpSocketMatcher::SrcAddr(BoundAddressMatcherEither::Unbound) => true;
        "src_addr unbound match"
    )]
    #[test_case(IpSocketMatcher::SrcAddr(BoundAddressMatcherEither::Bound(AddressMatcherEither::V4(
        AddressMatcher {
            matcher: AddressMatcherType::Subnet(SubnetMatcher(Ipv4::TEST_ADDRS.subnet)),
            invert: false,
        }
    ))) => false; "src_addr bound mismatch")]
    #[test_case(
        IpSocketMatcher::DstAddr(BoundAddressMatcherEither::Unbound) => true;
        "dst_addr unbound match"
    )]
    #[test_case(IpSocketMatcher::DstAddr(BoundAddressMatcherEither::Bound(AddressMatcherEither::V4(
        AddressMatcher {
            matcher: AddressMatcherType::Subnet(SubnetMatcher(Ipv4::TEST_ADDRS.subnet)),
            invert: false,
        }
    ))) => false; "dst_addr bound mismatch")]

    fn ip_socket_matcher_unbound(matcher: IpSocketMatcher<FakeDeviceClass>) -> bool {
        let socket = FakeIpSocket::<Ipv4, _> {
            src_ip: None,
            dst_ip: None,
            proto: FakeTcpSocket {
                src_port: Some(80),
                dst_port: Some(12345),
                state: TcpSocketState::Established,
            },
            cookie: 0,
            intf: None,
            marks: Marks::default(),
        };
        matcher.matches(&socket)
    }
}
