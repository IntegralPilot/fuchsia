// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::convert::Infallible as Never;
use core::fmt::Debug;
use core::num::NonZeroU32;

use diagnostics_traits::{Inspectable, Inspector};
use net_types::Witness;
use net_types::ip::{GenericOverIp, Ip, Ipv4, Ipv4SourceAddr, Ipv6, Ipv6SourceAddr, Mtu};
use packet_formats::icmp::{
    IcmpDestUnreachable, Icmpv4DestUnreachableCode, Icmpv4ParameterProblemCode, Icmpv4RedirectCode,
    Icmpv4TimeExceededCode, Icmpv6DestUnreachableCode, Icmpv6ParameterProblemCode,
    Icmpv6TimeExceededCode,
};
use packet_formats::ip::IpProtoExt;
use strum::{EnumCount as _, IntoEnumIterator as _};
use strum_macros::{EnumCount, EnumIter};

/// `Ip` extension trait to assist in defining [`NextHop`].
pub trait BroadcastIpExt: Ip {
    /// A marker type carried by the [`NextHop::Broadcast`] variant to indicate
    /// that it is uninhabited for IPv6.
    type BroadcastMarker: Debug + Copy + Clone + PartialEq + Eq + Send + Sync + 'static;
}

impl BroadcastIpExt for Ipv4 {
    type BroadcastMarker = ();
}

impl BroadcastIpExt for Ipv6 {
    type BroadcastMarker = Never;
}

/// Wrapper struct to provide a convenient [`GenericOverIp`] impl for use
/// with [`BroadcastIpExt::BroadcastMarker`].
#[derive(GenericOverIp)]
#[generic_over_ip(I, Ip)]
pub struct WrapBroadcastMarker<I: BroadcastIpExt>(pub I::BroadcastMarker);

/// Maximum packet size, that is the maximum IP payload one packet can carry.
///
/// More details from https://www.rfc-editor.org/rfc/rfc1122#section-3.3.2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mms(NonZeroU32);

impl Mms {
    /// Creates a maximum packet size from an [`Mtu`] value.
    pub fn from_mtu<I: IpExt>(mtu: Mtu, options_size: u32) -> Option<Self> {
        NonZeroU32::new(mtu.get().saturating_sub(I::IP_HEADER_LENGTH.get() + options_size))
            .map(|mms| Self(mms.min(I::IP_MAX_PAYLOAD_LENGTH)))
    }

    /// Returns the maximum packet size.
    pub fn get(&self) -> NonZeroU32 {
        let Self(mms) = *self;
        mms
    }
}

/// An ICMPv4 error type and code.
///
/// Each enum variant corresponds to a particular error type, and contains the
/// relevant information (e.g. code) for that error type.
#[derive(Copy, Clone, Debug, PartialEq)]
#[allow(missing_docs)]
pub enum Icmpv4ErrorCode {
    DestUnreachable(Icmpv4DestUnreachableCode, IcmpDestUnreachable),
    Redirect(Icmpv4RedirectCode),
    TimeExceeded(Icmpv4TimeExceededCode),
    ParameterProblem(Icmpv4ParameterProblemCode),
}

impl<I: IcmpIpExt> GenericOverIp<I> for Icmpv4ErrorCode {
    type Type = I::ErrorCode;
}

/// An ICMPv6 error type and code.
///
/// Each enum variant corresponds to a particular error type, and contains the
/// relevant information (e.g. code) for that error type.
#[derive(Copy, Clone, Debug, PartialEq)]
#[allow(missing_docs)]
pub enum Icmpv6ErrorCode {
    DestUnreachable(Icmpv6DestUnreachableCode),
    PacketTooBig(Mtu),
    TimeExceeded(Icmpv6TimeExceededCode),
    ParameterProblem(Icmpv6ParameterProblemCode),
}

impl<I: IcmpIpExt> GenericOverIp<I> for Icmpv6ErrorCode {
    type Type = I::ErrorCode;
}

/// An ICMP error of either IPv4 or IPv6.
#[derive(Debug, Clone, Copy)]
pub enum IcmpErrorCode {
    /// ICMPv4 error.
    V4(Icmpv4ErrorCode),
    /// ICMPv6 error.
    V6(Icmpv6ErrorCode),
}

impl From<Icmpv4ErrorCode> for IcmpErrorCode {
    fn from(v4_err: Icmpv4ErrorCode) -> Self {
        IcmpErrorCode::V4(v4_err)
    }
}

impl From<Icmpv6ErrorCode> for IcmpErrorCode {
    fn from(v6_err: Icmpv6ErrorCode) -> Self {
        IcmpErrorCode::V6(v6_err)
    }
}

/// An extension trait adding extra ICMP-related functionality to IP versions.
pub trait IcmpIpExt: packet_formats::ip::IpExt + packet_formats::icmp::IcmpIpExt {
    /// The type of error code for this version of ICMP - [`Icmpv4ErrorCode`] or
    /// [`Icmpv6ErrorCode`].
    type ErrorCode: Debug
        + Copy
        + PartialEq
        + GenericOverIp<Self, Type = Self::ErrorCode>
        + GenericOverIp<Ipv4, Type = Icmpv4ErrorCode>
        + GenericOverIp<Ipv6, Type = Icmpv6ErrorCode>
        + Into<IcmpErrorCode>;
}

impl IcmpIpExt for Ipv4 {
    type ErrorCode = Icmpv4ErrorCode;
}

impl IcmpIpExt for Ipv6 {
    type ErrorCode = Icmpv6ErrorCode;
}

/// `Ip` extension trait to assist in defining [`NextHop`].
pub trait IpTypesIpExt: packet_formats::ip::IpExt {
    /// A marker type carried by the [`NextHop::Broadcast`] variant to indicate
    /// that it is uninhabited for IPv6.
    type BroadcastMarker: Debug + Copy + Clone + PartialEq + Eq;
}

impl IpTypesIpExt for Ipv4 {
    type BroadcastMarker = ();
}

impl IpTypesIpExt for Ipv6 {
    type BroadcastMarker = Never;
}

/// An [`Ip`] extension trait adding functionality specific to the IP layer.
pub trait IpExt: packet_formats::ip::IpExt + IcmpIpExt + BroadcastIpExt + IpProtoExt {
    /// The type used to specify an IP packet's source address in a call to
    /// [`IpTransportContext::receive_ip_packet`].
    ///
    /// For IPv4, this is `Ipv4Addr`. For IPv6, this is [`Ipv6SourceAddr`].
    type RecvSrcAddr: Witness<Self::Addr> + Copy + Clone;
    /// The length of an IP header without any IP options.
    const IP_HEADER_LENGTH: NonZeroU32;
    /// The maximum payload size an IP payload can have.
    const IP_MAX_PAYLOAD_LENGTH: NonZeroU32;
}

impl IpExt for Ipv4 {
    type RecvSrcAddr = Ipv4SourceAddr;
    const IP_HEADER_LENGTH: NonZeroU32 =
        NonZeroU32::new(packet_formats::ipv4::HDR_PREFIX_LEN as u32).unwrap();
    const IP_MAX_PAYLOAD_LENGTH: NonZeroU32 =
        NonZeroU32::new(u16::MAX as u32 - Self::IP_HEADER_LENGTH.get()).unwrap();
}

impl IpExt for Ipv6 {
    type RecvSrcAddr = Ipv6SourceAddr;
    const IP_HEADER_LENGTH: NonZeroU32 =
        NonZeroU32::new(packet_formats::ipv6::IPV6_FIXED_HDR_LEN as u32).unwrap();
    const IP_MAX_PAYLOAD_LENGTH: NonZeroU32 = NonZeroU32::new(u16::MAX as u32).unwrap();
}

/// A mark attached to packets/sockets.
///
/// A mark can either be `None` or a `u32`. The mark can be carried by a
/// socket or a packet, and `None` means the socket/packet is unmarked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mark(pub Option<u32>);

impl From<Option<u32>> for Mark {
    fn from(m: Option<u32>) -> Self {
        Self(m)
    }
}

/// Mark domains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumCount, EnumIter)]
pub enum MarkDomain {
    /// The first mark.
    Mark1,
    /// The second mark.
    Mark2,
}

const MARK_DOMAINS: usize = MarkDomain::COUNT;

/// A storage backed by an array with the same cardinality of [`MarkDomain`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MarkStorage<T>([T; MARK_DOMAINS]);

impl<T> MarkStorage<T> {
    /// Creates [`MarkStorage`]s from an iterator of `(MarkDomain, U)`.
    ///
    /// An unspecified domain will remain default. For the same domain,
    /// a later value in the iterator will override an earlier value.
    ///
    /// For a specified domain, `(Some(value)).into()` will be written.
    pub fn new<U, IntoIter>(iter: IntoIter) -> Self
    where
        IntoIter: IntoIterator<Item = (MarkDomain, U)>,
        T: From<Option<U>> + Copy,
    {
        let mut storage = MarkStorage([None.into(); MARK_DOMAINS]);
        for (domain, value) in iter.into_iter() {
            *storage.get_mut(domain) = Some(value).into();
        }
        storage
    }

    fn domain_as_index(domain: MarkDomain) -> usize {
        match domain {
            MarkDomain::Mark1 => 0,
            MarkDomain::Mark2 => 1,
        }
    }

    /// Gets an immutable reference to the mark at the given domain.
    pub fn get(&self, domain: MarkDomain) -> &T {
        let Self(inner) = self;
        &inner[Self::domain_as_index(domain)]
    }

    /// Gets a mutable reference to the mark at the given domain.
    pub fn get_mut(&mut self, domain: MarkDomain) -> &mut T {
        let Self(inner) = self;
        &mut inner[Self::domain_as_index(domain)]
    }

    /// Returns an iterator over the mark domains.
    pub fn iter(&self) -> impl Iterator<Item = (MarkDomain, &T)> {
        let Self(inner) = self;
        MarkDomain::iter().map(move |domain| (domain, &inner[Self::domain_as_index(domain)]))
    }

    /// Zips with another storage so that the domains align.
    pub fn zip_with<'a, U>(
        &'a self,
        MarkStorage(other): &'a MarkStorage<U>,
    ) -> impl Iterator<Item = (MarkDomain, &'a T, &'a U)> + 'a {
        let Self(this) = self;
        MarkDomain::iter().zip(this.iter().zip(other.iter())).map(|(d, (t, u))| (d, t, u))
    }
}

/// The 2 marks that can be attached to packets.
pub type Marks = MarkStorage<Mark>;

impl Marks {
    /// Unmarked marks.
    pub const UNMARKED: Self = MarkStorage([Mark(None), Mark(None)]);
}

impl Inspectable for Marks {
    fn record<I: Inspector>(&self, inspector: &mut I) {
        for (domain, Mark(mark)) in self.iter() {
            if let Some(mark) = mark {
                let domain_name = match domain {
                    MarkDomain::Mark1 => "Mark1",
                    MarkDomain::Mark2 => "Mark2",
                };
                inspector.record_uint(domain_name, *mark);
            }
        }
    }
}
