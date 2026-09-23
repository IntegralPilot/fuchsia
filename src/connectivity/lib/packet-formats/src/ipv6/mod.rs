// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Parsing and serialization of IPv6 packets.
//!
//! The IPv6 packet format is defined in [RFC 8200] Sections 3 and 4.
//!
//! [RFC 8200]: https://datatracker.ietf.org/doc/html/rfc8200

pub mod ext_hdrs;

use alloc::vec::Vec;
use core::borrow::Borrow;
use core::fmt::{self, Debug, Formatter};
use core::ops::Range;

use log::debug;
use net_types::ip::{GenericOverIp, Ipv4Addr, Ipv6, Ipv6Addr, Ipv6SourceAddr};
use packet::records::{AlignedRecordSequenceBuilder, Records, RecordsRaw};
use packet::{
    BufferProvider, BufferView, BufferViewMut, EmptyBuf, FragmentedBytesMut, FromRaw,
    GrowBufferMut, InnerPacketBuilder, LayoutBufferAlloc, MaybeParsed, NestablePacketBuilder,
    NestableSerializer, NoOpSerializationContext, PacketBuilder, PacketConstraints, ParsablePacket,
    ParseMetadata, PartialPacketBuilder, PartialSerializer, SerializeError, SerializeTarget,
    Serializer,
};
use zerocopy::byteorder::network_endian::{U16, U32};
use zerocopy::{
    FromBytes, Immutable, IntoBytes, KnownLayout, Ref, SplitByteSlice, SplitByteSliceMut, Unaligned,
};

use crate::TRANSPORT_HEADER_MAX_SIZE;
use crate::error::{IpParseErrorAction, IpParseResult, Ipv6ParseError, ParseError};
use crate::icmp::Icmpv6ParameterProblemCode;
use crate::ip::{
    DscpAndEcn, FragmentOffset, IpEnvelope, IpExt, IpPacketBuilder, IpProto,
    IpSerializationContext, Ipv4Proto, Ipv6ExtHdrType, Ipv6Proto, Nat64Error,
    Nat64TranslationResult,
};
use crate::ipv4::{HDR_PREFIX_LEN, Ipv4PacketBuilder};
use crate::ipv6::ext_hdrs::ExtensionHeaderOptionAction;
use crate::tcp::{TcpParseArgs, TcpSegment};
use crate::udp::{UdpPacket, UdpParseArgs};

use ext_hdrs::{
    HopByHopOption, HopByHopOptionData, IPV6_FRAGMENT_EXT_HDR_LEN, Ipv6ExtensionHeader,
    Ipv6ExtensionHeaderImpl, Ipv6ExtensionHeaderParsingContext, Ipv6ExtensionHeaderParsingError,
    is_valid_next_header_upper_layer,
};

/// Length of the IPv6 fixed header.
pub const IPV6_FIXED_HDR_LEN: usize = 40;

/// The range of bytes within an IPv6 header buffer that the
/// payload length field uses.
pub const IPV6_PAYLOAD_LEN_BYTE_RANGE: Range<usize> = 4..6;

// Offset to the Next Header field within the fixed IPv6 header
const NEXT_HEADER_OFFSET: u8 = 6;

// The maximum length for Hop-by-Hop Options. The stored byte's maximum
// representable value is `u8::MAX` and it means the header has
// that many 8-octets, not including the first 8 octets.
const IPV6_HBH_OPTIONS_MAX_LEN: usize = (u8::MAX as usize) * 8 + 8;

/// The maximum payload length after an IPv6 header.
///
/// The maximum IPv6 payload is the total number of bytes after the fixed header
/// and must fit in a u16 as defined in [RFC 8200 Section 3].
///
/// [RFC 8200 Section 3]: https://datatracker.ietf.org/doc/html/rfc8200#section-3.
const IPV6_MAX_PAYLOAD_LENGTH: usize = u16::MAX as usize;

/// Convert an extension header parsing error to an IP packet
/// parsing error.
fn ext_hdr_err_fn(hdr: &FixedHeader, err: Ipv6ExtensionHeaderParsingError) -> Ipv6ParseError {
    // Below, we set parameter problem data's `pointer` to `IPV6_FIXED_HDR_LEN` + `pointer`
    // since the the `pointer` we get from an `Ipv6ExtensionHeaderParsingError` is calculated
    // from the start of the extension headers. Within an IPv6 packet, extension headers
    // start right after the fixed header with a length of `IPV6_FIXED_HDR_LEN` so we add `pointer`
    // to `IPV6_FIXED_HDR_LEN` to get the pointer to the field with the parameter problem error
    // from the start of the IPv6 packet. For a non-jumbogram packet, we know that
    // `IPV6_FIXED_HDR_LEN` + `pointer` will not overflow because the maximum size of an
    // IPv6 packet is 65575 bytes (fixed header + extension headers + body) and 65575 definitely
    // fits within an `u32`. This may no longer hold true if/when jumbogram packets are supported.
    // For the jumbogram case when the size of extension headers could be >= (4 GB - 41 bytes) (which
    // we almost certainly will never encounter), the pointer calculation may overflow. To account for
    // this scenario, we check for overflows when adding `IPV6_FIXED_HDR_LEN` to `pointer`. If
    // we do end up overflowing, we will discard the packet (even if we were normally required to
    // send back an ICMP error message) because we will be unable to construct a correct ICMP error
    // message (the pointer field of the ICMP message will not be able to hold a value > (4^32 - 1)
    // which is what we would have if the pointer calculation overflows). But again, we should almost
    // never encounter this scenario so we don't care if we have incorrect behaviour.

    match err {
        Ipv6ExtensionHeaderParsingError::ErroneousHeaderField { pointer, must_send_icmp } => {
            Ipv6ParseError::ParameterProblem {
                src_ip: hdr.src_ip,
                dst_ip: hdr.dst_ip,
                code: Icmpv6ParameterProblemCode::ErroneousHeaderField,
                pointer,
                must_send_icmp,
                action: IpParseErrorAction::DiscardPacketSendIcmpNoMulticast,
            }
        }
        Ipv6ExtensionHeaderParsingError::UnrecognizedNextHeader { pointer, must_send_icmp } => {
            Ipv6ParseError::ParameterProblem {
                src_ip: hdr.src_ip,
                dst_ip: hdr.dst_ip,
                code: Icmpv6ParameterProblemCode::UnrecognizedNextHeaderType,
                pointer,
                must_send_icmp,
                action: IpParseErrorAction::DiscardPacketSendIcmpNoMulticast,
            }
        }
        Ipv6ExtensionHeaderParsingError::UnrecognizedOption { pointer, must_send_icmp, action } => {
            let action = match action {
                ExtensionHeaderOptionAction::SkipAndContinue => unreachable!(
                    "Should never end up here because this action should never result in an error"
                ),
                ExtensionHeaderOptionAction::DiscardPacket => IpParseErrorAction::DiscardPacket,
                ExtensionHeaderOptionAction::DiscardPacketSendIcmp => {
                    IpParseErrorAction::DiscardPacketSendIcmp
                }
                ExtensionHeaderOptionAction::DiscardPacketSendIcmpNoMulticast => {
                    IpParseErrorAction::DiscardPacketSendIcmpNoMulticast
                }
            };

            Ipv6ParseError::ParameterProblem {
                src_ip: hdr.src_ip,
                dst_ip: hdr.dst_ip,
                code: Icmpv6ParameterProblemCode::UnrecognizedIpv6Option,
                pointer,
                must_send_icmp,
                action,
            }
        }
        Ipv6ExtensionHeaderParsingError::BufferExhausted
        | Ipv6ExtensionHeaderParsingError::MalformedData => {
            // Unexpectedly running out of a buffer or encountering malformed
            // data when parsing is a formatting error.
            Ipv6ParseError::Parse { error: ParseError::Format }
        }
    }
}

/// The IPv6 fixed header which precedes any extension headers and the body.
#[derive(Debug, Default, KnownLayout, FromBytes, IntoBytes, Immutable, Unaligned, PartialEq)]
#[repr(C)]
pub struct FixedHeader {
    version_tc_flowlabel: [u8; 4],
    payload_len: U16,
    next_hdr: u8,
    hop_limit: u8,
    src_ip: Ipv6Addr,
    dst_ip: Ipv6Addr,
}

const IP_VERSION: u8 = 6;
const VERSION_OFFSET: u8 = 4;
const FLOW_LABEL_MAX: u32 = (1 << 20) - 1;

impl FixedHeader {
    #[allow(clippy::too_many_arguments)]
    fn new(
        dscp_and_ecn: DscpAndEcn,
        flow_label: u32,
        payload_len: u16,
        next_hdr: u8,
        hop_limit: u8,
        src_ip: Ipv6Addr,
        dst_ip: Ipv6Addr,
    ) -> FixedHeader {
        debug_assert!(flow_label <= FLOW_LABEL_MAX);

        let traffic_class = dscp_and_ecn.raw();
        FixedHeader {
            version_tc_flowlabel: [
                IP_VERSION << VERSION_OFFSET | traffic_class >> 4,
                (traffic_class << 4) | ((flow_label >> 16) as u8),
                (flow_label >> 8) as u8,
                flow_label as u8,
            ],
            payload_len: U16::new(payload_len),
            next_hdr,
            hop_limit,
            src_ip,
            dst_ip,
        }
    }

    fn version(&self) -> u8 {
        self.version_tc_flowlabel[0] >> 4
    }

    fn dscp_and_ecn(&self) -> DscpAndEcn {
        ((self.version_tc_flowlabel[0] & 0xF) << 4 | self.version_tc_flowlabel[1] >> 4).into()
    }

    fn flowlabel(&self) -> u32 {
        (u32::from(self.version_tc_flowlabel[1]) & 0xF) << 16
            | u32::from(self.version_tc_flowlabel[2]) << 8
            | u32::from(self.version_tc_flowlabel[3])
    }
}

/// Provides common access to IPv6 header fields.
///
/// `Ipv6Header` provides access to IPv6 header fields as a common
/// implementation for both [`Ipv6Packet`] and [`Ipv6PacketRaw`].
pub trait Ipv6Header {
    /// Gets a reference to the IPv6 [`FixedHeader`].
    fn get_fixed_header(&self) -> &FixedHeader;

    /// The Hop Limit.
    fn hop_limit(&self) -> u8 {
        self.get_fixed_header().hop_limit
    }

    /// The Next Header.
    fn next_header(&self) -> u8 {
        self.get_fixed_header().next_hdr
    }

    /// The source IP address.
    fn src_ip(&self) -> Ipv6Addr {
        self.get_fixed_header().src_ip
    }

    /// The destination IP address.
    fn dst_ip(&self) -> Ipv6Addr {
        self.get_fixed_header().dst_ip
    }

    /// The Differentiated Services Code Point (DSCP) and the Explicit
    /// Congestion Notification (ECN).
    fn dscp_and_ecn(&self) -> DscpAndEcn {
        self.get_fixed_header().dscp_and_ecn()
    }
}

impl Ipv6Header for FixedHeader {
    fn get_fixed_header(&self) -> &FixedHeader {
        self
    }
}

/// An IPv6 packet.
///
/// An `Ipv6Packet` shares its underlying memory with the byte slice it was
/// parsed from or serialized to, meaning that no copying or extra allocation is
/// necessary.
pub struct Ipv6Packet<B> {
    fixed_hdr: Ref<B, FixedHeader>,
    extension_hdrs: Records<B, Ipv6ExtensionHeaderImpl>,
    body: B,
    proto: Ipv6Proto,
}

impl<B: SplitByteSlice, I: IpExt> GenericOverIp<I> for Ipv6Packet<B> {
    type Type = <I as IpExt>::Packet<B>;
}

impl<B: SplitByteSlice> Ipv6Header for Ipv6Packet<B> {
    fn get_fixed_header(&self) -> &FixedHeader {
        &self.fixed_hdr
    }
}

impl<B: SplitByteSlice> ParsablePacket<B, ()> for Ipv6Packet<B> {
    type Error = Ipv6ParseError;

    fn parse_metadata(&self) -> ParseMetadata {
        let header_len = Ref::bytes(&self.fixed_hdr).len() + self.extension_hdrs.bytes().len();
        ParseMetadata::from_packet(header_len, self.body.len(), 0)
    }

    fn parse<BV: BufferView<B>>(buffer: BV, _args: ()) -> Result<Self, Ipv6ParseError> {
        Ipv6PacketRaw::parse(buffer, ()).and_then(Ipv6Packet::try_from_raw)
    }
}

impl<B: SplitByteSlice> FromRaw<Ipv6PacketRaw<B>, ()> for Ipv6Packet<B> {
    type Error = Ipv6ParseError;

    fn try_from_raw_with(raw: Ipv6PacketRaw<B>, _args: ()) -> Result<Self, Self::Error> {
        let fixed_hdr = raw.fixed_hdr;

        let extension_hdrs = match raw.extension_hdrs {
            MaybeParsed::Complete(v) => Records::try_from_raw(v),
            MaybeParsed::Incomplete(buffer) => {
                // If raw parser failed then try full parser again. This is
                // expected to fail, but the returned error may be different
                // from the error reported by the raw parser.
                let context = Ipv6ExtensionHeaderParsingContext::new(fixed_hdr.next_hdr);
                match Records::<_, Ipv6ExtensionHeaderImpl>::parse_with_context(buffer, context) {
                    Err(err) => Err(err),
                    Ok(_) => panic!("Extension Header parsing succeeded after raw parse failure."),
                }
            }
        };
        let extension_hdrs = extension_hdrs.map_err(|e| ext_hdr_err_fn(&fixed_hdr, e))?;

        // If extension headers parse successfully, then proto and a
        // `MaybeParsed` body MUST be available, and the proto must be a valid
        // next header for upper layers.
        let (body, proto) =
            raw.body_proto.expect("Unable to retrieve Ipv6Proto or MaybeParsed body from raw");
        debug_assert!(is_valid_next_header_upper_layer(proto.into()));

        let body = match body {
            MaybeParsed::Complete(b) => b,
            MaybeParsed::Incomplete(_b) => {
                return debug_err!(Err(ParseError::Format.into()), "IPv6 body unretrievable.");
            }
        };

        // check that the lengths match:
        //
        // As per Section 3 of RFC 8200, payload length includes the length of
        // the extension headers as well.
        if extension_hdrs.bytes().len() + body.len() != usize::from(fixed_hdr.payload_len.get()) {
            return debug_err!(
                Err(ParseError::Format.into()),
                "Payload len does not match body and extension headers"
            );
        }

        // validate IP version in header
        if fixed_hdr.version() != 6 {
            return debug_err!(
                Err(ParseError::Format.into()),
                "unexpected IP version: {}",
                fixed_hdr.version()
            );
        }

        Ok(Ipv6Packet { fixed_hdr, extension_hdrs, body, proto })
    }
}

impl<B, C> PartialSerializer<C> for Ipv6Packet<B>
where
    B: SplitByteSlice,
    C: IpSerializationContext<Ipv6>,
{
    // TODO(https://fxbug.dev/473824085): Keep the reference to the whole
    // serialized packet and return it from `partial_serialize()` as
    // `PartialSerializeResult::Slice`.

    fn partial_serialize_new_buf<BB: GrowBufferMut, A: LayoutBufferAlloc<BB>>(
        &self,
        _context: &mut C,
        constraints: PacketConstraints,
        alloc: A,
    ) -> Result<(BB, usize), SerializeError<A::Error>> {
        // Copy IP header, extension header and up to 64 bytes of the body,
        // which includes the transport headers.
        let fixed_hdr = Ref::bytes(&self.fixed_hdr);
        let extension_hdrs = self.extension_hdrs.bytes();
        let fixed_hdr_len = fixed_hdr.len();
        let header_len = fixed_hdr_len + extension_hdrs.len();
        let body_to_copy = self.body().len().min(TRANSPORT_HEADER_MAX_SIZE);
        let outer_header_len = constraints.header_len();
        let mut buffer = alloc.layout_alloc(outer_header_len + header_len, body_to_copy, 0)?;
        buffer.with_parts_mut(|prefix, mut body, _suffix| {
            let extensions_pos = outer_header_len + fixed_hdr_len;
            prefix[outer_header_len..extensions_pos].copy_from_slice(fixed_hdr);
            prefix[extensions_pos..].copy_from_slice(extension_hdrs);
            body.copy_from_slice(&self.body()[..body_to_copy]);
        });
        buffer.grow_front(header_len);
        let total_size = header_len + self.body.len();
        Ok((buffer, total_size))
    }
}

impl<B: SplitByteSlice> Ipv6Packet<B> {
    /// Returns an iterator over the extension headers.
    pub fn iter_extension_hdrs(&self) -> impl Iterator<Item = Ipv6ExtensionHeader<'_>> {
        self.extension_hdrs.iter()
    }

    /// The packet body.
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// The Differentiated Services Code Point (DSCP) and the Explicit
    /// Congestion Notification (ECN).
    pub fn dscp_and_ecn(&self) -> DscpAndEcn {
        self.fixed_hdr.dscp_and_ecn()
    }

    /// The flow label.
    pub fn flowlabel(&self) -> u32 {
        self.fixed_hdr.flowlabel()
    }

    /// The Upper layer protocol for this packet.
    ///
    /// This is found in the fixed header's Next Header if there are no extension
    /// headers, or the Next Header value in the last extension header if there are.
    /// This also  uses the same codes, encoded by the Rust type `Ipv6Proto`.
    pub fn proto(&self) -> Ipv6Proto {
        self.proto
    }

    /// The source IP address represented as an [`Ipv6SourceAddr`].
    ///
    /// Unlike [`IpHeader::src_ip`], `src_ipv6` returns an `Ipv6SourceAddr`,
    /// which represents the valid values that a source address can take
    /// (namely, a unicast or unspecified address) or `None` if the address is
    /// invalid (namely, a multicast address or an ipv4-mapped-ipv6 address).
    pub fn src_ipv6(&self) -> Option<Ipv6SourceAddr> {
        Ipv6SourceAddr::new(self.fixed_hdr.src_ip)
    }

    /// Return a buffer that is a copy of the header bytes in this
    /// packet, including the fixed and extension headers, but without
    /// the first fragment extension header.
    ///
    /// Note, if there are multiple fragment extension headers, only
    /// the first fragment extension header will be removed.
    ///
    /// # Panics
    ///
    /// Panics if there is no fragment extension header in this packet.
    pub fn copy_header_bytes_for_fragment(&self) -> Vec<u8> {
        // Since the final header will not include a fragment header, we don't
        // need to allocate bytes for it (`IPV6_FRAGMENT_EXT_HDR_LEN` bytes).
        let expected_bytes_len = self.header_len() - IPV6_FRAGMENT_EXT_HDR_LEN;
        let mut bytes = Vec::with_capacity(expected_bytes_len);

        bytes.extend_from_slice(Ref::bytes(&self.fixed_hdr));

        // We cannot simply copy over the extension headers because we want
        // discard the first fragment header, so we iterate over our
        // extension headers and find out where our fragment header starts at.
        let mut iter = self.extension_hdrs.iter();

        // This should never panic because we must only call this function
        // when the packet is fragmented so it must have at least one extension
        // header (the fragment extension header).
        let ext_hdr = iter.next().expect("packet must have at least one extension header");

        if self.fixed_hdr.next_hdr == Ipv6ExtHdrType::Fragment.into() {
            // The fragment header is the first extension header so
            // we need to patch the fixed header.

            // Update the next header value in the fixed header within the buffer
            // to the next header value from the fragment header.
            bytes[6] = iter.context().next_header;

            // Copy extension headers that appear after the fragment header
            bytes.extend_from_slice(&self.extension_hdrs.bytes()[IPV6_FRAGMENT_EXT_HDR_LEN..]);
        } else {
            let mut ext_hdr = ext_hdr;
            let mut ext_hdr_start = IPV6_FIXED_HDR_LEN;
            let mut ext_hdr_end = iter.context().position;

            // Here we keep looping until `next_ext_hdr` points to the fragment header.
            // Once we find the fragment header, we update the next header value within
            // the extension header preceeding the fragment header, `ext_hdr`. Note,
            // we keep track of where in the extension header buffer the current `ext_hdr`
            // starts and ends so we can patch its next header value.
            loop {
                // This should never panic because if we panic, it means that we got a
                // `None` value from `iter.next()` which would mean we exhausted all the
                // extension headers while looking for the fragment header, meaning there
                // is no fragment header. This function should never be called if there
                // is no fragment extension header in the packet.
                let next_ext_hdr = iter
                    .next()
                    .expect("exhausted all extension headers without finding fragment header");

                if let Ipv6ExtensionHeader::Fragment { .. } = next_ext_hdr {
                    // The next extension header is the fragment header
                    // so we copy the buffer before and after the extension header
                    // into `bytes` and patch the next header value within the
                    // current extension header in `bytes`.

                    // Header position relative to the extension header buffer.
                    let fragment_hdr_start = ext_hdr_end - IPV6_FIXED_HDR_LEN;

                    // Size of the fragment header should be exactly `IPV6_FRAGMENT_EXT_HDR_LEN`.
                    let fragment_hdr_end = fragment_hdr_start + IPV6_FRAGMENT_EXT_HDR_LEN;
                    assert_eq!(fragment_hdr_end, iter.context().position - IPV6_FIXED_HDR_LEN);

                    let extension_hdr_bytes = self.extension_hdrs.bytes();

                    // Copy extension headers that appear before the fragment header
                    bytes.extend_from_slice(&extension_hdr_bytes[..fragment_hdr_start]);

                    // Copy extension headers that appear after the fragment header
                    bytes.extend_from_slice(&extension_hdr_bytes[fragment_hdr_end..]);

                    // Update the current `ext_hdr`'s next header value to the next
                    // header value within the fragment extension header.
                    match ext_hdr {
                        // The next header value is located in the first byte of the
                        // extension header.
                        Ipv6ExtensionHeader::HopByHopOptions { .. }
                        | Ipv6ExtensionHeader::DestinationOptions { .. }
                        | Ipv6ExtensionHeader::Routing { .. } => {
                            bytes[ext_hdr_start] = iter.context().next_header;
                        }
                        Ipv6ExtensionHeader::Fragment { .. } => unreachable!(
                            "If we had a fragment header before `ext_hdr`, we should have used that instead"
                        ),
                    }

                    break;
                }

                ext_hdr = next_ext_hdr;
                ext_hdr_start = ext_hdr_end;
                ext_hdr_end = iter.context().position;
            }
        }

        // `bytes`'s length should be exactly `expected_bytes_len`.
        assert_eq!(bytes.len(), expected_bytes_len);
        bytes
    }

    /// Returns an [`Ipv6PerFragmentHeaderBuilder`] for this packet.
    ///
    /// This builder will include the extension headers that should be part
    /// of the per-fragment header, and omit the extension headers that should
    /// be part of the fragment body. All bytes in the original packet after
    /// `per_fragment_builder().headers_len()` should be considered the fragment
    /// body.
    ///
    /// Per [RFC 8200 Section 4.5]:
    ///   The Per-Fragment headers must consist of the IPv6 header plus any
    ///   extension headers that must be processed by nodes en route to the
    ///   destination, that is, all headers up to and including the Routing
    ///   header if present, else the Hop-by-Hop Options header if present,
    ///   else no extension headers.
    ///
    /// [RFC 8200 Section 4.5]: https://datatracker.ietf.org/doc/html/rfc8200#section-4.5
    pub fn per_fragment_builder(&self) -> Ipv6PerFragmentHeaderBuilder<Vec<u8>> {
        let mut routing_point = None;
        let mut hbh_point = None;
        let mut iter = self.extension_hdrs.iter();
        let mut is_first_ext_hdr = true;

        while let Some(ext_hdr) = iter.next() {
            match ext_hdr {
                Ipv6ExtensionHeader::HopByHopOptions { .. } => {
                    // Per RFC 8200 Section 4.1
                    //   IPv6 nodes must accept and attempt to process extension
                    //   headers in any order and occurring any number of times
                    //   in the same packet, except for the Hop-by-Hop Options
                    //   header, which is restricted to appear immediately after
                    //   an IPv6 header only.
                    // Therefore we only copy over the Hop-By-Hop option if it
                    // is first.
                    if is_first_ext_hdr {
                        hbh_point = Some((
                            iter.context().position,
                            iter.context().next_header_offset,
                            iter.context().next_header,
                        ));
                    }
                }
                Ipv6ExtensionHeader::Routing { .. } => {
                    routing_point = Some((
                        iter.context().position,
                        iter.context().next_header_offset,
                        iter.context().next_header,
                    ));
                    break;
                }
                _ => {}
            }
            is_first_ext_hdr = false;
        }

        let split_point = routing_point.or(hbh_point);
        let (meta, next_header) = match split_point {
            Some((position, next_header_offset, next_header)) => (
                Some(Ipv6PerFragmentMeta {
                    ext_hdrs: self.extension_hdrs.bytes()[..position - IPV6_FIXED_HDR_LEN].to_vec(),
                    first_ext_hdr: Ipv6ExtHdrType::from(self.fixed_hdr.next_hdr),
                    last_next_hdr_offset: next_header_offset - IPV6_FIXED_HDR_LEN,
                }),
                next_header,
            ),
            None => (None, self.fixed_hdr.next_hdr),
        };
        // NB: All extension headers are after the split point are now part of
        // of the fragment body. In order to serialize the fragment header
        // properly, update the protocol for the builder to be the first
        // extension header in the body (or the upperlayer proto, if none).
        let mut prefix_builder = self.builder();
        prefix_builder.proto = Ipv6Proto::from(next_header);

        Ipv6PerFragmentHeaderBuilder { prefix_builder, meta }
    }

    /// The size of the fixed header plus extension headers.
    pub fn header_len(&self) -> usize {
        Ref::bytes(&self.fixed_hdr).len() + self.extension_hdrs.bytes().len()
    }

    fn fragment_header_present(&self) -> bool {
        for ext_hdr in self.extension_hdrs.iter() {
            if matches!(ext_hdr, Ipv6ExtensionHeader::Fragment { .. }) {
                return true;
            }
        }
        false
    }

    /// Construct a builder with the same contents as this packet.
    pub fn builder(&self) -> Ipv6PacketBuilder {
        Ipv6PacketBuilder {
            dscp_and_ecn: self.dscp_and_ecn(),
            flowlabel: self.flowlabel(),
            hop_limit: self.hop_limit(),
            proto: self.proto(),
            src_ip: self.src_ip(),
            dst_ip: self.dst_ip(),
        }
    }

    /// Performs the header translation part of NAT64 as described in [RFC
    /// 7915].
    ///
    /// `nat64_translate` follows the rules described in RFC 7915 to construct
    /// the IPv4 equivalent of this IPv6 packet. If the payload is a TCP segment
    /// or a UDP packet, its checksum will be updated. If the payload is an
    /// ICMPv6 packet, it will be converted to the equivalent ICMPv4 packet. For
    /// all other payloads, the payload will be unchanged, and the IP header will
    /// be translated. On success, a [`Serializer`] is returned which describes
    /// the new packet to be sent.
    ///
    /// Note that the IPv4 TTL/IPv6 Hop Limit field is not modified. It is the
    /// caller's responsibility to decrement and process this field per RFC
    /// 7915.
    ///
    /// In some cases, the packet has no IPv4 equivalent, in which case the
    /// value [`Nat64TranslationResult::Drop`] will be returned, instructing the
    /// caller to silently drop the packet.
    ///
    /// # Errors
    ///
    /// `nat64_translate` will return an error if support has not yet been
    /// implemented for translating a particular IP protocol.
    ///
    /// [RFC 7915]: https://datatracker.ietf.org/doc/html/rfc7915
    pub fn nat64_translate(
        &self,
        v4_src_addr: Ipv4Addr,
        v4_dst_addr: Ipv4Addr,
    ) -> Nat64TranslationResult<
        impl Serializer<NoOpSerializationContext, Buffer = EmptyBuf> + Debug + '_,
        Nat64Error,
    > {
        // A single `Serializer` type so that all possible return values from
        // this function have the same type.
        #[derive(Debug)]
        enum Nat64Serializer<T, U, O> {
            Tcp(T),
            Udp(U),
            Other(O),
        }
        impl<T, U, O> Serializer<NoOpSerializationContext> for Nat64Serializer<T, U, O>
        where
            T: Serializer<NoOpSerializationContext, Buffer = EmptyBuf>,
            U: Serializer<NoOpSerializationContext, Buffer = EmptyBuf>,
            O: Serializer<NoOpSerializationContext, Buffer = EmptyBuf>,
        {
            type Buffer = EmptyBuf;
            fn serialize<B, P>(
                self,
                context: &mut NoOpSerializationContext,
                outer: PacketConstraints,
                provider: P,
            ) -> Result<B, (SerializeError<P::Error>, Self)>
            where
                B: GrowBufferMut,
                P: BufferProvider<Self::Buffer, B>,
            {
                match self {
                    Nat64Serializer::Tcp(serializer) => serializer
                        .serialize(context, outer, provider)
                        .map_err(|(err, ser)| (err, Nat64Serializer::Tcp(ser))),
                    Nat64Serializer::Udp(serializer) => serializer
                        .serialize(context, outer, provider)
                        .map_err(|(err, ser)| (err, Nat64Serializer::Udp(ser))),
                    Nat64Serializer::Other(serializer) => serializer
                        .serialize(context, outer, provider)
                        .map_err(|(err, ser)| (err, Nat64Serializer::Other(ser))),
                }
            }

            fn serialize_new_buf<B: GrowBufferMut, A: LayoutBufferAlloc<B>>(
                &self,
                context: &mut NoOpSerializationContext,
                outer: PacketConstraints,
                alloc: A,
            ) -> Result<B, SerializeError<A::Error>> {
                match self {
                    Nat64Serializer::Tcp(serializer) => {
                        serializer.serialize_new_buf(context, outer, alloc)
                    }
                    Nat64Serializer::Udp(serializer) => {
                        serializer.serialize_new_buf(context, outer, alloc)
                    }
                    Nat64Serializer::Other(serializer) => {
                        serializer.serialize_new_buf(context, outer, alloc)
                    }
                }
            }
        }

        impl<T, U, O> NestableSerializer for Nat64Serializer<T, U, O>
        where
            T: Serializer<NoOpSerializationContext, Buffer = EmptyBuf>,
            U: Serializer<NoOpSerializationContext, Buffer = EmptyBuf>,
            O: Serializer<NoOpSerializationContext, Buffer = EmptyBuf>,
        {
        }

        // TODO(https://fxbug.dev/42174049): Add support for fragmented packets
        // forwarding.
        if self.fragment_header_present() {
            return Nat64TranslationResult::Err(Nat64Error::NotImplemented);
        }

        let v4_builder = |v4_proto| {
            let mut builder =
                Ipv4PacketBuilder::new(v4_src_addr, v4_dst_addr, self.hop_limit(), v4_proto);
            builder.dscp_and_ecn(self.dscp_and_ecn());

            // The IPv4 header length is 20 bytes (so IHL field value is 5), as
            // no header options are present in translated IPv4 packet.
            // As per RFC 7915 Section 5.1:
            //  "Internet Header Length:  5 (no IPv4 options)"
            const IPV4_HEADER_LEN_BYTES: usize = HDR_PREFIX_LEN;

            // As per RFC 7915 Section 5.1,
            //    "Flags:  The More Fragments flag is set to zero.  The Don't Fragment
            //        (DF) flag is set as follows: If the size of the translated IPv4
            //        packet is less than or equal to 1260 bytes, it is set to zero;
            //        otherwise, it is set to one."
            builder.df_flag(self.body().len() + IPV4_HEADER_LEN_BYTES > 1260);

            // TODO(https://fxbug.dev/42174049): This needs an update once
            // we don't return early for fragment_header_present case.
            builder.fragment_offset(FragmentOffset::ZERO);
            builder.mf_flag(false);

            builder
        };

        match self.proto() {
            Ipv6Proto::Proto(IpProto::Tcp) => {
                let v4_pkt_builder = v4_builder(Ipv4Proto::Proto(IpProto::Tcp));
                let args = TcpParseArgs::new(self.src_ip(), self.dst_ip());
                // TODO(https://fxbug.dev/42174405): We're doing roughly similar work
                // in valid/invalid parsing case. Remove match statement and
                // update the checksum in place without needing to parse the TCP
                // segment once we have ability to update the checksum.
                match TcpSegment::parse(&mut self.body.as_bytes(), args) {
                    Ok(tcp) => {
                        // Creating a new tcp_serializer for IPv6 packet from
                        // the existing one ensures that checksum is
                        // updated due to changed IP addresses.
                        let tcp_serializer =
                            Nat64Serializer::Tcp(tcp.into_serializer(v4_src_addr, v4_dst_addr));
                        Nat64TranslationResult::Forward(v4_pkt_builder.wrap_body(tcp_serializer))
                    }
                    Err(msg) => {
                        debug!("Parsing of TCP segment failed: {:?}", msg);

                        // This means we can't create a TCP segment builder with
                        // updated checksum. Parsing may fail due to a variety of
                        // reasons, including incorrect checksum in incoming packet.
                        // We should still return a packet with IP payload copied
                        // as is from IPv6 to IPv4. This handling is similar to
                        // the handling of the case with unsupported protocol type
                        // as done in `Ipv6Proto::Other(val)` case below. The similar
                        // reasoning from RFC appiles here as well.
                        let common_serializer =
                            Nat64Serializer::Other(self.body().into_serializer());
                        Nat64TranslationResult::Forward(v4_pkt_builder.wrap_body(common_serializer))
                    }
                }
            }

            // TODO(https://fxbug.dev/42174405): We're doing roughly similar work
            // in valid/invalid parsing case. Remove match statement and
            // update the checksum in place without needing to parse the UDP segment
            // once we have ability to update checksum.
            Ipv6Proto::Proto(IpProto::Udp) => {
                let v4_pkt_builder = v4_builder(Ipv4Proto::Proto(IpProto::Udp));
                let args = UdpParseArgs::new(self.src_ip(), self.dst_ip());
                match UdpPacket::parse(&mut self.body.as_bytes(), args) {
                    Ok(udp) => {
                        // Creating a new udp_serializer for IPv6 packet from
                        // the existing one ensures that checksum is
                        // updated due to changed IP addresses.
                        let udp_serializer =
                            Nat64Serializer::Udp(udp.into_serializer(v4_src_addr, v4_dst_addr));
                        Nat64TranslationResult::Forward(v4_pkt_builder.wrap_body(udp_serializer))
                    }
                    Err(msg) => {
                        debug!("Parsing of UDP packet failed: {:?}", msg);

                        // This means we can't create a UDP packet builder with
                        // updated checksum. Parsing may fail due to a variety of
                        // reasons, including incorrect checksum in incoming packet.
                        // We should still return a packet with IP payload copied
                        // as is from IPv6 to IPv4. This handling is similar to
                        // the handling of the case with unsupported protocol type
                        // as done in `Ipv6Proto::Other(val)` case below. The similar
                        // reasoning from RFC appiles here as well.

                        let common_serializer =
                            Nat64Serializer::Other(self.body().into_serializer());
                        Nat64TranslationResult::Forward(v4_pkt_builder.wrap_body(common_serializer))
                    }
                }
            }

            // TODO(https://fxbug.dev/42174051): Implement ICMP packet translation
            // support here.
            Ipv6Proto::Icmpv6 => Nat64TranslationResult::Err(Nat64Error::NotImplemented),

            // For all other protocols, an IPv4 packet must be forwarded even if
            // the transport layer checksum update is not implemented.
            // As per RFC 7915 Section 5.1,
            //     "Protocol:
            //       ...
            //
            //       For the first 'next header' that does not match one of the cases
            //       above, its Next Header value (which contains the transport
            //       protocol number) is copied to the protocol field in the IPv4
            //       header.  This means that all transport protocols are translated.
            //
            //       Note:  Some translated protocols will fail at the receiver for
            //          various reasons: some are known to fail when translated (e.g.,
            //          IPsec Authentication Header (51)), and others will fail
            //          checksum validation if the address translation is not checksum
            //          neutral [RFC6052] and the translator does not update the
            //          transport protocol's checksum (because the translator doesn't
            //          support recalculating the checksum for that transport protocol;
            //          see Section 5.5)."
            Ipv6Proto::Other(val) => {
                let v4_pkt_builder = v4_builder(Ipv4Proto::Other(val));
                let common_serializer = Nat64Serializer::Other(self.body().into_serializer());
                Nat64TranslationResult::Forward(v4_pkt_builder.wrap_body(common_serializer))
            }

            Ipv6Proto::NoNextHeader => {
                let v4_pkt_builder = v4_builder(Ipv4Proto::Other(Ipv6Proto::NoNextHeader.into()));
                let common_serializer = Nat64Serializer::Other(self.body().into_serializer());
                Nat64TranslationResult::Forward(v4_pkt_builder.wrap_body(common_serializer))
            }

            // Don't forward packets that use IANA's reserved protocol; they're
            // invalid.
            Ipv6Proto::Proto(IpProto::Reserved) => Nat64TranslationResult::Drop,
        }
    }

    /// Copies the packet (Header + Extensions + Body) into a `Vec`.
    pub fn to_vec(&self) -> Vec<u8> {
        let Ipv6Packet { fixed_hdr, extension_hdrs, body, proto: _ } = self;
        let mut buf = Vec::with_capacity(
            Ref::bytes(&fixed_hdr).len() + extension_hdrs.bytes().len() + body.as_bytes().len(),
        );
        buf.extend(Ref::bytes(&fixed_hdr));
        buf.extend(extension_hdrs.bytes());
        buf.extend(body.as_bytes());
        buf
    }
}

impl<B: SplitByteSliceMut> Ipv6Packet<B> {
    /// Set the source IP address.
    pub fn set_src_ip(&mut self, addr: Ipv6Addr) {
        self.fixed_hdr.src_ip = addr;
    }

    /// Set the destination IP address.
    pub fn set_dst_ip(&mut self, addr: Ipv6Addr) {
        self.fixed_hdr.dst_ip = addr;
    }

    /// Set the hop limit.
    pub fn set_hop_limit(&mut self, hlim: u8) {
        self.fixed_hdr.hop_limit = hlim;
    }

    /// The packet body.
    pub fn body_mut(&mut self) -> &mut [u8] {
        &mut self.body
    }

    /// Provides simultaneous access to header, extension headers, and mutable
    /// body.
    pub fn parts_with_body_mut(&mut self) -> (&FixedHeader, ExtensionHeaders<'_>, &mut [u8]) {
        (&self.fixed_hdr, ExtensionHeaders(self.extension_hdrs.as_ref()), &mut self.body)
    }
}

impl<B: SplitByteSlice> Debug for Ipv6Packet<B> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), fmt::Error> {
        f.debug_struct("Ipv6Packet")
            .field("src_ip", &self.src_ip())
            .field("dst_ip", &self.dst_ip())
            .field("hop_limit", &self.hop_limit())
            .field("proto", &self.proto())
            .field("dscp", &self.dscp_and_ecn().dscp())
            .field("ecn", &self.dscp_and_ecn().ecn())
            .field("flowlabel", &self.flowlabel())
            .field("extension headers", &"TODO")
            .field("body", &alloc::format!("<{} bytes>", self.body.len()))
            .finish()
    }
}

/// The extension headers in an [`Ipv6Packet`].
pub struct ExtensionHeaders<'a>(Records<&'a [u8], Ipv6ExtensionHeaderImpl>);

impl<'a> ExtensionHeaders<'a> {
    /// Returns an iterator over the extension headers.
    pub fn iter(&self) -> impl Iterator<Item = Ipv6ExtensionHeader<'_>> {
        self.0.iter()
    }

    /// Returns the raw bytes of the extension headers.
    pub fn bytes(&self) -> &[u8] {
        self.0.bytes()
    }
}

/// We were unable to parse the extension headers.
///
/// As a result, we were unable to determine the upper-layer Protocol Number
/// (which is stored in the last extension header's Next Header field) and were
/// unable figure out where the body begins.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct ExtHdrParseError;

/// A partially parsed and not yet validated IPv6 packet.
///
/// `Ipv6PacketRaw` provides minimal parsing of an IPv6 packet, namely
/// it only requires that the fixed header part ([`HeaderPrefix`]) be retrieved,
/// all the other parts of the packet may be missing when attempting to create
/// it.
///
/// [`Ipv6Packet`] provides a [`FromRaw`] implementation that can be used to
/// validate an `Ipv6PacketRaw`.
pub struct Ipv6PacketRaw<B> {
    /// A raw packet always contains at least a fully parsed `FixedHeader`.
    fixed_hdr: Ref<B, FixedHeader>,
    /// When `extension_hdrs` is [`MaybeParsed::Complete`], it contains the
    /// `RecordsRaw` that can be validated for full extension headers parsing.
    /// Otherwise, it just contains the extension header bytes that were
    /// successfully consumed before reaching an error (typically "buffer
    /// exhausted").
    extension_hdrs: MaybeParsed<RecordsRaw<B, Ipv6ExtensionHeaderImpl>, B>,
    /// The body and upper-layer Protocol Number.
    ///
    /// If extension headers failed to parse, this will be
    /// `Err(ExtHdrParseError)`. Extension headers must be parsed in order to
    /// find the bounds of the upper-layer payload and to find that last
    /// extension header's Next Header field, which is the Protocol Number of
    /// the upper-layer payload.
    ///
    /// The body will be [`MaybeParsed::Complete`] if all the body bytes were
    /// consumed (as stated by the header's payload length value) or
    /// [`MaybeParsed::Incomplete`] containing the bytes that were present
    /// otherwise.
    body_proto: Result<(MaybeParsed<B, B>, Ipv6Proto), ExtHdrParseError>,
}

impl<B> Ipv6PacketRaw<B> {
    /// Returns a mutable reference to the body bytes of this [`Ipv6PacketRaw`].
    ///
    /// Might not be complete if a full packet was not received.
    pub fn body_mut(&mut self) -> Option<&mut B> {
        match self.body_proto {
            Ok(ref mut b) => match b {
                (MaybeParsed::Complete(b), _) => Some(b),
                (MaybeParsed::Incomplete(b), _) => Some(b),
            },
            Err(_) => None,
        }
    }
}

impl<B: SplitByteSlice> Ipv6Header for Ipv6PacketRaw<B> {
    fn get_fixed_header(&self) -> &FixedHeader {
        &self.fixed_hdr
    }
}

impl<B: SplitByteSlice> ParsablePacket<B, ()> for Ipv6PacketRaw<B> {
    type Error = Ipv6ParseError;

    fn parse<BV: BufferView<B>>(mut buffer: BV, _args: ()) -> Result<Self, Self::Error> {
        let fixed_hdr = buffer
            .take_obj_front::<FixedHeader>()
            .ok_or_else(debug_err_fn!(ParseError::Format, "too few bytes for header"))?;
        let payload_len = fixed_hdr.payload_len.get().into();
        // Trim the buffer if it exceeds the length specified in the header.
        let _: Option<B> = buffer.len().checked_sub(payload_len).map(|padding| {
            buffer.take_back(padding).unwrap_or_else(|| {
                panic!("buffer.len()={} padding={}", buffer.len(), padding);
            })
        });

        let mut extension_hdr_context = Ipv6ExtensionHeaderParsingContext::new(fixed_hdr.next_hdr);

        let extension_hdrs =
            RecordsRaw::parse_raw_with_mut_context(&mut buffer, &mut extension_hdr_context)
                .map_incomplete(|(b, _)| b);

        let body_proto = match &extension_hdrs {
            MaybeParsed::Complete(r) => {
                let _: &RecordsRaw<B, _> = r;
                // If we have extension headers our context's
                // (`extension_hdr_context`) `next_header` would be updated with
                // the last extension header's Next Header value. This will also
                // work if we don't have any extension headers. Let's consider
                // that scenario: When we have no extension headers, the Next
                // Header value in the fixed header will be a valid upper layer
                // protocol value.  `parse_bv_with_mut_context` will return
                // almost immediately without doing any actual work when it
                // checks the context's (`extension_hdr_context`) `next_header`
                // value and ends parsing since, according to our context, its
                // data is for an upper layer protocol. Now, since nothing was
                // parsed, our context was never modified, so the next header
                // value it was initialized with when calling
                // `Ipv6ExtensionHeaderParsingContext::new`, will not have
                // changed. We simply use that value and assign it to proto
                // below.

                // Extension header raw parsing only finishes when we have a
                // valid next header that is meant for the upper layer. The
                // assertion below enforces that contract.
                assert!(is_valid_next_header_upper_layer(extension_hdr_context.next_header));
                let proto = Ipv6Proto::from(extension_hdr_context.next_header);
                let body = MaybeParsed::new_with_min_len(
                    buffer.into_rest(),
                    payload_len.saturating_sub(extension_hdrs.len()),
                );
                Ok((body, proto))
            }
            MaybeParsed::Incomplete(b) => {
                let _: &B = b;
                Err(ExtHdrParseError)
            }
        };

        Ok(Ipv6PacketRaw { fixed_hdr, extension_hdrs, body_proto })
    }

    fn parse_metadata(&self) -> ParseMetadata {
        let header_len = Ref::bytes(&self.fixed_hdr).len() + self.extension_hdrs.len();
        let body_len = self.body_proto.as_ref().map(|(b, _p)| b.len()).unwrap_or(0);
        ParseMetadata::from_packet(header_len, body_len, 0)
    }
}

impl<B: SplitByteSlice> Ipv6PacketRaw<B> {
    /// Returns the body and upper-layer Protocol Number.
    ///
    /// If extension headers failed to parse, `body_proto` returns
    /// `Err(ExtHdrParseError)`. Extension headers must be parsed in order to
    /// find the bounds of the upper-layer payload and to find that last
    /// extension header's Next Header field, which is the Protocol Number of
    /// the upper-layer payload.
    ///
    /// The returned body will be [`MaybeParsed::Complete`] if all the body
    /// bytes were consumed (as stated by the header's payload length value) or
    /// [`MaybeParsed::Incomplete`] containing the bytes that were present
    /// otherwise.
    pub fn body_proto(&self) -> Result<(MaybeParsed<&[u8], &[u8]>, Ipv6Proto), ExtHdrParseError> {
        self.body_proto
            .as_ref()
            .map(|(mp, proto)| {
                (mp.as_ref().map(|b| b.deref()).map_incomplete(|b| b.deref()), *proto)
            })
            .map_err(|e| *e)
    }

    /// Returns the body.
    ///
    /// If extension headers failed to parse, `body` returns
    /// `Err(ExtHdrParseError)`. Extension headers must be parsed in order to
    /// find the bounds of the upper-layer payload.
    ///
    /// The returned body will be [`MaybeParsed::Complete`] if all the body
    /// bytes were consumed (as stated by the header's payload length value) or
    /// [`MaybeParsed::Incomplete`] containing the bytes that were present
    /// otherwise.
    pub fn body(&self) -> Result<MaybeParsed<&[u8], &[u8]>, ExtHdrParseError> {
        self.body_proto().map(|(body, _proto)| body)
    }

    /// Returns the upper-layer Protocol Number.
    ///
    /// If extension headers failed to parse, `body_proto` returns
    /// `Err(ExtHdrParseError)`. Extension headers must be parsed in order to
    /// find the last extension header's Next Header field, which is the
    /// Protocol Number of the upper-layer payload.
    pub fn proto(&self) -> Result<Ipv6Proto, ExtHdrParseError> {
        self.body_proto().map(|(_body, proto)| proto)
    }
}

impl<B: SplitByteSliceMut> Ipv6PacketRaw<B> {
    /// Set the source IP address.
    pub fn set_src_ip(&mut self, addr: Ipv6Addr) {
        self.fixed_hdr.src_ip = addr;
    }

    /// Set the destination IP address.
    pub fn set_dst_ip(&mut self, addr: Ipv6Addr) {
        self.fixed_hdr.dst_ip = addr;
    }
}

/// A next header that may be either a next layer header or an IPv6 extension
/// header.
pub enum NextHeader {
    /// A next layer header follows.
    NextLayer(Ipv6Proto),
    /// An extension header follows.
    Extension(Ipv6ExtHdrType),
}

impl From<NextHeader> for u8 {
    fn from(next_hdr: NextHeader) -> Self {
        match next_hdr {
            NextHeader::NextLayer(n) => n.into(),
            NextHeader::Extension(e) => e.into(),
        }
    }
}

mod sealed {
    use super::*;
    /// A marker trait for IPv6 headers that can be serialized before header
    /// `T`.
    ///
    /// This trait is used to enforce IPv6 extension header ordering according
    /// to [RFC 8200 Section 4.1].
    ///
    /// [RFC 8200 Section 4.1]: https://datatracker.ietf.org/doc/html/rfc8200#section-4.1
    pub trait Ipv6HeaderBefore<T> {}

    impl<'a, O, T> Ipv6HeaderBefore<T> for &'a O where O: Ipv6HeaderBefore<T> {}

    /// A trait abstracting all types of IPv6 header builders.
    pub trait Ipv6HeaderBuilder {
        /// Returns an immutable reference to the fixed header builder.
        fn fixed_header(&self) -> &Ipv6PacketBuilder;

        /// Returns the total header length of the extension headers, including
        /// previous extension headers, but excluding the fixed header size.
        fn extension_headers_len(&self) -> usize;

        /// Serializes the header into `buffer`.
        ///
        /// `next_header` is the header immediately after the current one.
        /// `payload_len` is the total size of the frame after this header.
        fn serialize_header<B: SplitByteSliceMut, BV: BufferViewMut<B>>(
            &self,
            buffer: &mut BV,
            next_header: NextHeader,
            payload_len: usize,
        );
    }
}
use sealed::{Ipv6HeaderBefore, Ipv6HeaderBuilder};

impl<'a, O> Ipv6HeaderBuilder for &'a O
where
    O: Ipv6HeaderBuilder,
{
    fn fixed_header(&self) -> &Ipv6PacketBuilder {
        O::fixed_header(self)
    }

    fn extension_headers_len(&self) -> usize {
        O::extension_headers_len(self)
    }

    fn serialize_header<B: SplitByteSliceMut, BV: BufferViewMut<B>>(
        &self,
        buffer: &mut BV,
        next_header: NextHeader,
        payload_len: usize,
    ) {
        O::serialize_header(self, buffer, next_header, payload_len)
    }
}

/// A helper macro to implement `PacketBuilder` methods for implementers of
/// `Ipv6HeaderBuilder`.
///
/// This can't be a blanket impl because `PacketBuilder` is a foreign trait.
macro_rules! impl_packet_builder_base {
    {} => {
        fn constraints(&self) -> PacketConstraints {
            let ext_headers = self.extension_headers_len();
            let header_len = IPV6_FIXED_HDR_LEN + ext_headers;
            let footer_len = 0;
            let min_body_len = 0;
            // Extension headers take from the IPv6 available payload size.
            // See RFC 8200 Section 3 for details.
            let max_body_len = IPV6_MAX_PAYLOAD_LENGTH - ext_headers;
            PacketConstraints::new(header_len, footer_len, min_body_len, max_body_len)
        }
    }
}

macro_rules! impl_packet_builder {
    {} => {
        fn context_state(&self) -> C::ContextState {
            C::envelope_to_state(IpEnvelope::new(self.extension_headers_len() > 0))
        }

        fn serialize(
            &self,
            _context: &mut C,
            target: &mut SerializeTarget<'_>,
            body: FragmentedBytesMut<'_, '_>,
        ) {
            let mut bv = &mut target.header;
            self.serialize_header(
                &mut bv,
                NextHeader::NextLayer(
                    <Ipv6PacketBuilder as IpPacketBuilder<C, Ipv6>>::proto(self.fixed_header())
                ),
                body.len(),
            );
        }
    }
}

macro_rules! impl_partial_packet_builder {
    {} => {
        fn partial_serialize(
            &self,
            _context: &mut C,
            body_len: usize,
            mut buffer: &mut [u8],
        ) {
            self.serialize_header(
                &mut &mut buffer,
                NextHeader::NextLayer(
                    <Ipv6PacketBuilder as IpPacketBuilder<C, Ipv6>>::proto(self.fixed_header())
                ),
                body_len,
            );
        }
    }
}
/// A builder for IPv6 packets.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Ipv6PacketBuilder {
    dscp_and_ecn: DscpAndEcn,
    flowlabel: u32,
    hop_limit: u8,
    // The protocol number of the upper layer protocol, not the Next Header
    // value of the first extension header (if one exists).
    proto: Ipv6Proto,
    src_ip: Ipv6Addr,
    dst_ip: Ipv6Addr,
}

impl Ipv6PacketBuilder {
    /// Constructs a new `Ipv6PacketBuilder`.
    ///
    /// The `proto` field encodes the protocol number identifying the upper
    /// layer payload, not the Next Header value of the first extension header
    /// (if one exists).
    pub fn new<S: Into<Ipv6Addr>, D: Into<Ipv6Addr>>(
        src_ip: S,
        dst_ip: D,
        hop_limit: u8,
        proto: Ipv6Proto,
    ) -> Ipv6PacketBuilder {
        Ipv6PacketBuilder {
            dscp_and_ecn: DscpAndEcn::default(),
            flowlabel: 0,
            hop_limit,
            proto,
            src_ip: src_ip.into(),
            dst_ip: dst_ip.into(),
        }
    }

    /// Set the Differentiated Services Code Point (DSCP) and the Explicit
    /// Congestion Notification (ECN).
    pub fn dscp_and_ecn(&mut self, dscp_and_ecn: DscpAndEcn) {
        self.dscp_and_ecn = dscp_and_ecn;
    }

    /// Set the flowlabel.
    ///
    /// # Panics
    ///
    /// `flowlabel` panics if `flowlabel` is greater than 2^20 - 1.
    pub fn flowlabel(&mut self, flowlabel: u32) {
        assert!(flowlabel <= 1 << 20, "invalid flowlabel: {:x}", flowlabel);
        self.flowlabel = flowlabel;
    }
}

impl Ipv6HeaderBuilder for Ipv6PacketBuilder {
    fn fixed_header(&self) -> &Ipv6PacketBuilder {
        self
    }

    fn extension_headers_len(&self) -> usize {
        0
    }

    fn serialize_header<B: SplitByteSliceMut, BV: BufferViewMut<B>>(
        &self,
        buffer: &mut BV,
        next_header: NextHeader,
        payload_len: usize,
    ) {
        buffer
            .write_obj_front(&FixedHeader::new(
                self.dscp_and_ecn,
                self.flowlabel,
                {
                    // The caller promises to supply a body whose length
                    // does not exceed max_body_len. Doing this as a
                    // debug_assert (rather than an assert) is fine because,
                    // with debug assertions disabled, we'll just write an
                    // incorrect header value, which is acceptable if the
                    // caller has violated their contract.
                    debug_assert!(payload_len <= u16::MAX as usize);
                    payload_len as u16
                },
                next_header.into(),
                self.hop_limit,
                self.src_ip,
                self.dst_ip,
            ))
            .expect("not enough bytes for IPv6 fixed header");
    }
}

impl NestablePacketBuilder for Ipv6PacketBuilder {
    impl_packet_builder_base! {}
}

impl<C: IpSerializationContext<Ipv6>> PacketBuilder<C> for Ipv6PacketBuilder {
    impl_packet_builder! {}
}

impl<C: IpSerializationContext<Ipv6>> PartialPacketBuilder<C> for Ipv6PacketBuilder {
    impl_partial_packet_builder! {}
}

/// A builder for Ipv6 packets with HBH Options.
#[derive(Debug, Clone)]
pub struct Ipv6PacketBuilderWithHbhOptions<'a, I> {
    prefix_builder: Ipv6PacketBuilder,
    hbh_options: AlignedRecordSequenceBuilder<HopByHopOption<'a>, I>,
}

impl<'a, I> Ipv6PacketBuilderWithHbhOptions<'a, I>
where
    I: Iterator + Clone,
    I::Item: Borrow<HopByHopOption<'a>>,
{
    /// Creates a IPv6 packet builder with a Hop By Hop Options extension header.
    pub fn new<T: IntoIterator<Item = I::Item, IntoIter = I>>(
        prefix_builder: Ipv6PacketBuilder,
        options: T,
    ) -> Option<Ipv6PacketBuilderWithHbhOptions<'a, I>> {
        let iter = options.into_iter();
        // https://tools.ietf.org/html/rfc2711#section-2.1 specifies that
        // an RouterAlert option can only appear once.
        if iter
            .clone()
            .filter(|r| matches!(r.borrow().data, HopByHopOptionData::RouterAlert { .. }))
            .count()
            > 1
        {
            return None;
        }
        let hbh_options = AlignedRecordSequenceBuilder::new(2, iter);
        // And we don't want our options to become too long.
        if next_multiple_of_eight(2 + hbh_options.serialized_len()) > IPV6_HBH_OPTIONS_MAX_LEN {
            return None;
        }
        Some(Ipv6PacketBuilderWithHbhOptions { prefix_builder, hbh_options })
    }

    fn aligned_hbh_len(&self) -> usize {
        let opt_len = self.hbh_options.serialized_len();
        let hbh_len = opt_len + 2;
        next_multiple_of_eight(hbh_len)
    }
}

fn next_multiple_of_eight(x: usize) -> usize {
    (x + 7) & (!7)
}

impl<C: IpSerializationContext<Ipv6>> IpPacketBuilder<C, Ipv6> for Ipv6PacketBuilder {
    fn new(src_ip: Ipv6Addr, dst_ip: Ipv6Addr, ttl: u8, proto: Ipv6Proto) -> Self {
        Ipv6PacketBuilder::new(src_ip, dst_ip, ttl, proto)
    }

    fn src_ip(&self) -> Ipv6Addr {
        self.src_ip
    }

    fn set_src_ip(&mut self, addr: Ipv6Addr) {
        self.src_ip = addr;
    }

    fn dst_ip(&self) -> Ipv6Addr {
        self.dst_ip
    }

    fn set_dst_ip(&mut self, addr: Ipv6Addr) {
        self.dst_ip = addr;
    }

    fn proto(&self) -> Ipv6Proto {
        self.proto
    }

    fn set_dscp_and_ecn(&mut self, dscp_and_ecn: DscpAndEcn) {
        self.dscp_and_ecn = dscp_and_ecn;
    }
}

impl<'a, I> Ipv6HeaderBuilder for Ipv6PacketBuilderWithHbhOptions<'a, I>
where
    I: Iterator + Clone,
    I::Item: Borrow<HopByHopOption<'a>>,
{
    fn fixed_header(&self) -> &Ipv6PacketBuilder {
        &self.prefix_builder
    }

    fn extension_headers_len(&self) -> usize {
        self.prefix_builder.extension_headers_len() + self.aligned_hbh_len()
    }

    fn serialize_header<B: SplitByteSliceMut, BV: BufferViewMut<B>>(
        &self,
        buffer: &mut BV,
        next_header: NextHeader,
        payload_len: usize,
    ) {
        let aligned_hbh_len = self.aligned_hbh_len();
        // The next header in the fixed header now should be 0 (Hop-by-Hop Extension Header)
        self.prefix_builder.serialize_header(
            buffer,
            NextHeader::Extension(Ipv6ExtHdrType::HopByHopOptions),
            payload_len + aligned_hbh_len,
        );
        // take the first two bytes to write in proto and length information.
        let mut hbh_header = buffer.take_front(aligned_hbh_len).unwrap();
        let hbh_header: &mut [u8] = hbh_header.as_mut();
        hbh_header[0] = next_header.into();
        hbh_header[1] = u8::try_from((aligned_hbh_len - 8) / 8).expect("extension header too big");
        self.hbh_options.serialize_into(&mut hbh_header[2..]);
    }
}

impl<'a, I> NestablePacketBuilder for Ipv6PacketBuilderWithHbhOptions<'a, I>
where
    I: Iterator + Clone,
    I::Item: Borrow<HopByHopOption<'a>>,
{
    impl_packet_builder_base! {}
}

impl<'a, I, C: IpSerializationContext<Ipv6>> PacketBuilder<C>
    for Ipv6PacketBuilderWithHbhOptions<'a, I>
where
    I: Iterator + Clone,
    I::Item: Borrow<HopByHopOption<'a>>,
{
    impl_packet_builder! {}
}

impl<'a, I, C: IpSerializationContext<Ipv6>> PartialPacketBuilder<C>
    for Ipv6PacketBuilderWithHbhOptions<'a, I>
where
    I: Iterator + Clone,
    I::Item: Borrow<HopByHopOption<'a>>,
{
    impl_partial_packet_builder! {}
}

impl<'a, C: IpSerializationContext<Ipv6>, I> IpPacketBuilder<C, Ipv6>
    for Ipv6PacketBuilderWithHbhOptions<'a, I>
where
    I: Iterator<Item: Borrow<HopByHopOption<'a>>> + Debug + Default + Clone,
{
    fn new(src_ip: Ipv6Addr, dst_ip: Ipv6Addr, ttl: u8, proto: Ipv6Proto) -> Self {
        Ipv6PacketBuilderWithHbhOptions::new(
            Ipv6PacketBuilder::new(src_ip, dst_ip, ttl, proto),
            I::default(),
        )
        .expect("packet builder with no options should be valid")
    }

    fn src_ip(&self) -> Ipv6Addr {
        self.prefix_builder.src_ip
    }

    fn set_src_ip(&mut self, addr: Ipv6Addr) {
        self.prefix_builder.src_ip = addr;
    }

    fn dst_ip(&self) -> Ipv6Addr {
        self.prefix_builder.dst_ip
    }

    fn set_dst_ip(&mut self, addr: Ipv6Addr) {
        self.prefix_builder.dst_ip = addr;
    }

    fn proto(&self) -> Ipv6Proto {
        self.prefix_builder.proto
    }

    fn set_dscp_and_ecn(&mut self, dscp_and_ecn: DscpAndEcn) {
        <Ipv6PacketBuilder as IpPacketBuilder<C, Ipv6>>::set_dscp_and_ecn(
            &mut self.prefix_builder,
            dscp_and_ecn,
        )
    }
}

/// Metadata about extension headers for the `Ipv6PerFragmentHeaderBuilder`.
#[derive(Debug, Clone, Eq, PartialEq)]
struct Ipv6PerFragmentMeta<B> {
    ext_hdrs: B,
    first_ext_hdr: Ipv6ExtHdrType,
    last_next_hdr_offset: usize,
}

/// A builder for IPv6 packets containing the per-fragment extension headers.
///
/// Generally, this should be wrapped with an
/// [`Ipv6PacketBuilderWithFragmentHeader`].
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Ipv6PerFragmentHeaderBuilder<B> {
    prefix_builder: Ipv6PacketBuilder,
    meta: Option<Ipv6PerFragmentMeta<B>>,
}

impl<B: AsRef<[u8]>> Ipv6PerFragmentHeaderBuilder<B> {
    /// Returns the length of the header (fixed header and extension headers).
    pub fn header_len(&self) -> usize {
        self.extension_headers_len() + IPV6_FIXED_HDR_LEN
    }

    /// Converts `self` into an identical builder with references to the
    /// underlying extension header bytes.
    pub fn as_ref(&self) -> Ipv6PerFragmentHeaderBuilder<&[u8]> {
        let Self { prefix_builder, meta } = self;
        let meta = meta.as_ref().map(
            |Ipv6PerFragmentMeta { ext_hdrs, first_ext_hdr, last_next_hdr_offset }| {
                Ipv6PerFragmentMeta {
                    ext_hdrs: ext_hdrs.as_ref(),
                    first_ext_hdr: *first_ext_hdr,
                    last_next_hdr_offset: *last_next_hdr_offset,
                }
            },
        );
        Ipv6PerFragmentHeaderBuilder { prefix_builder: prefix_builder.clone(), meta }
    }
}

impl<B: AsRef<[u8]>> Ipv6HeaderBuilder for Ipv6PerFragmentHeaderBuilder<B> {
    fn fixed_header(&self) -> &Ipv6PacketBuilder {
        &self.prefix_builder
    }

    fn extension_headers_len(&self) -> usize {
        self.meta
            .as_ref()
            .map(|Ipv6PerFragmentMeta { ext_hdrs, .. }| ext_hdrs.as_ref().len())
            .unwrap_or(0)
    }

    fn serialize_header<BB: SplitByteSliceMut, BV: BufferViewMut<BB>>(
        &self,
        buffer: &mut BV,
        next_header: NextHeader,
        payload_len: usize,
    ) {
        let Self { prefix_builder, meta } = self;
        match meta {
            None => prefix_builder.serialize_header(buffer, next_header, payload_len),
            Some(Ipv6PerFragmentMeta { ext_hdrs, first_ext_hdr, last_next_hdr_offset }) => {
                let ext_hdrs = ext_hdrs.as_ref();
                let ext_hdrs_len = ext_hdrs.len();
                prefix_builder.serialize_header(
                    buffer,
                    NextHeader::Extension(*first_ext_hdr),
                    payload_len + ext_hdrs_len,
                );
                let mut ext_hdr_buf =
                    buffer.take_front(ext_hdrs_len).expect("buffer should be long enough");
                let ext_hdr_buf: &mut [u8] = ext_hdr_buf.as_mut();
                ext_hdr_buf.copy_from_slice(ext_hdrs);
                ext_hdr_buf[*last_next_hdr_offset] = u8::from(next_header);
            }
        }
    }
}

impl<B: AsRef<[u8]>> NestablePacketBuilder for Ipv6PerFragmentHeaderBuilder<B> {
    impl_packet_builder_base! {}
}

impl<B: AsRef<[u8]>, C: IpSerializationContext<Ipv6>> PacketBuilder<C>
    for Ipv6PerFragmentHeaderBuilder<B>
{
    impl_packet_builder! {}
}

impl<B: AsRef<[u8]>, C: IpSerializationContext<Ipv6>> PartialPacketBuilder<C>
    for Ipv6PerFragmentHeaderBuilder<B>
{
    impl_partial_packet_builder! {}
}

/// An IPv6 packet builder that includes the fragmentation header.
///
/// `Ipv6PacketBuilderWithFragmentHeader` wraps another compatible packet
/// builder to attach the fragment header on it.
///
/// See [RFC 8200 Section 2.5] for the fragment header format.
///
/// [RFC 8200 Section 2.5]: https://datatracker.ietf.org/doc/html/rfc8200#section-4.5
#[derive(Debug, Eq, PartialEq)]
pub struct Ipv6PacketBuilderWithFragmentHeader<B> {
    header_builder: B,
    fragment_offset: FragmentOffset,
    more_fragments: bool,
    identification: u32,
}

impl<B: Ipv6HeaderBefore<Self>> Ipv6PacketBuilderWithFragmentHeader<B> {
    /// Creates a new `Ipv6PacketBuilderWithFragmentHeader`.
    pub fn new(
        header_builder: B,
        fragment_offset: FragmentOffset,
        more_fragments: bool,
        identification: u32,
    ) -> Self {
        Self { header_builder, fragment_offset, more_fragments, identification }
    }
}

impl<B> Ipv6HeaderBefore<Ipv6PacketBuilderWithFragmentHeader<B>> for Ipv6PacketBuilder {}
impl<B, I> Ipv6HeaderBefore<Ipv6PacketBuilderWithFragmentHeader<B>>
    for Ipv6PacketBuilderWithHbhOptions<'_, I>
{
}
impl<HB, B> Ipv6HeaderBefore<Ipv6PacketBuilderWithFragmentHeader<HB>>
    for Ipv6PerFragmentHeaderBuilder<B>
{
}

/// A marker trait for all header builder types that can be used to construct
/// and serialize IPv6 headers using [`Ipv6PacketBuilderWithFragmentHeader`].
pub trait Ipv6PacketBuilderBeforeFragment:
    Ipv6HeaderBefore<Ipv6PacketBuilderWithFragmentHeader<Self>> + Ipv6HeaderBuilder + Sized
{
}
impl<B> Ipv6PacketBuilderBeforeFragment for B where
    B: Ipv6HeaderBefore<Ipv6PacketBuilderWithFragmentHeader<Self>> + Ipv6HeaderBuilder + Sized
{
}

impl<B: Ipv6HeaderBuilder> Ipv6HeaderBuilder for Ipv6PacketBuilderWithFragmentHeader<B> {
    fn fixed_header(&self) -> &Ipv6PacketBuilder {
        self.header_builder.fixed_header()
    }

    fn extension_headers_len(&self) -> usize {
        self.header_builder.extension_headers_len() + IPV6_FRAGMENT_EXT_HDR_LEN
    }

    fn serialize_header<BB: SplitByteSliceMut, BV: BufferViewMut<BB>>(
        &self,
        buffer: &mut BV,
        next_header: NextHeader,
        payload_len: usize,
    ) {
        let Self { header_builder, fragment_offset, more_fragments, identification } = self;
        let payload_len = payload_len + IPV6_FRAGMENT_EXT_HDR_LEN;
        header_builder.serialize_header(
            buffer,
            NextHeader::Extension(Ipv6ExtHdrType::Fragment),
            payload_len,
        );
        buffer.write_obj_front(&u8::from(next_header)).unwrap();
        // Reserved.
        let _: BB = buffer.take_front_zero(1).unwrap();
        let more_fragments = u16::from(*more_fragments);
        buffer
            .write_obj_front(&U16::new(fragment_offset.into_raw() << 3 | more_fragments))
            .unwrap();
        buffer.write_obj_front(&U32::new(*identification)).unwrap();
    }
}

impl<B: Ipv6HeaderBuilder> NestablePacketBuilder for Ipv6PacketBuilderWithFragmentHeader<B> {
    impl_packet_builder_base! {}
}

impl<B: Ipv6HeaderBuilder, C: IpSerializationContext<Ipv6>> PacketBuilder<C>
    for Ipv6PacketBuilderWithFragmentHeader<B>
{
    impl_packet_builder! {}
}

impl<B: Ipv6HeaderBuilder, C: IpSerializationContext<Ipv6>> PartialPacketBuilder<C>
    for Ipv6PacketBuilderWithFragmentHeader<B>
{
    impl_partial_packet_builder! {}
}

/// Reassembles a fragmented packet into a parsed IP packet.
///
/// # Panics
///
/// Panics if the provided header is too small to hold a valid header.
pub(crate) fn reassemble_fragmented_packet<
    'a,
    B: SplitByteSliceMut,
    BV: BufferViewMut<B>,
    I: Iterator<Item = &'a [u8]>,
>(
    mut buffer: BV,
    header: &[u8],
    body_fragments: I,
) -> IpParseResult<Ipv6, ()> {
    assert!(header.len() >= IPV6_FIXED_HDR_LEN);

    let bytes = buffer.as_mut();

    // First, copy over the header data.
    bytes[0..header.len()].copy_from_slice(header);
    let mut byte_count = header.len();

    // Next, copy over the body fragments.
    for p in body_fragments {
        bytes[byte_count..byte_count + p.len()].copy_from_slice(p);
        byte_count += p.len();
    }

    //
    // Fix up the IPv6 header
    //

    // For IPv6, the payload length is the sum of the length of the
    // extension headers and the packet body. The header as it is stored
    // includes the IPv6 fixed header and all extension headers, so
    // `bytes_count` is the sum of the size of the fixed header,
    // extension headers and packet body. To calculate the payload
    // length we subtract the size of the fixed header from the total
    // byte count of a reassembled packet.
    let payload_length = byte_count - IPV6_FIXED_HDR_LEN;

    // Make sure that the payload length is not more than the maximum
    // possible IPv6 packet length.
    if payload_length > usize::from(u16::MAX) {
        return debug_err!(
            Err(ParseError::Format.into()),
            "fragmented packet payload length of {} bytes is too large",
            payload_length
        );
    }

    // We know the call to `unwrap` will not fail because we verified the length
    // of `header` and copied it's bytes into `bytes`.
    let mut header = Ref::<_, FixedHeader>::from_prefix(bytes).unwrap().0;

    // Update the payload length field.
    header.payload_len.set(u16::try_from(payload_length).unwrap());

    Ok(())
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use packet::{Buf, FragmentedBuffer, ParseBuffer, PartialSerializeResult};
    use test_case::test_case;

    use crate::ethernet::{EthernetFrame, EthernetFrameLengthCheck};
    use crate::testutil::*;

    use super::ext_hdrs::*;
    use super::*;

    const DEFAULT_SRC_IP: Ipv6Addr =
        Ipv6Addr::from_bytes([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
    const DEFAULT_DST_IP: Ipv6Addr =
        Ipv6Addr::from_bytes([17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32]);

    const DEFAULT_V4_SRC_IP: Ipv4Addr = Ipv4Addr::new([1, 2, 3, 4]);
    const DEFAULT_V4_DST_IP: Ipv4Addr = Ipv4Addr::new([5, 6, 7, 8]);

    #[test]
    fn test_parse_serialize_full_tcp() {
        use crate::testdata::syn_v6::*;

        let mut buf = ETHERNET_FRAME.bytes;
        let frame = buf.parse_with::<_, EthernetFrame<_>>(EthernetFrameLengthCheck::Check).unwrap();
        verify_ethernet_frame(&frame, ETHERNET_FRAME);

        let mut body = frame.body();
        let packet = body.parse::<Ipv6Packet<_>>().unwrap();
        verify_ipv6_packet(&packet, IPV6_PACKET);

        // Verify serialization via builders.
        let buffer = packet
            .body()
            .into_serializer()
            .wrap_in(packet.builder())
            .wrap_in(frame.builder())
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap();
        assert_eq!(buffer.as_ref(), ETHERNET_FRAME.bytes);

        // Verify serialization via `to_vec`.
        assert_eq!(&packet.to_vec()[..], IPV6_PACKET.bytes);
    }

    #[test]
    fn test_parse_serialize_full_udp() {
        use crate::testdata::dns_request_v6::*;

        let mut buf = ETHERNET_FRAME.bytes;
        let frame = buf.parse_with::<_, EthernetFrame<_>>(EthernetFrameLengthCheck::Check).unwrap();
        verify_ethernet_frame(&frame, ETHERNET_FRAME);

        let mut body = frame.body();
        let packet = body.parse::<Ipv6Packet<_>>().unwrap();
        verify_ipv6_packet(&packet, IPV6_PACKET);

        // Verify serialization via builders.
        let buffer = packet
            .body()
            .into_serializer()
            .wrap_in(packet.builder())
            .wrap_in(frame.builder())
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap();
        assert_eq!(buffer.as_ref(), ETHERNET_FRAME.bytes);

        // Verify serialization via `to_vec`.
        assert_eq!(&packet.to_vec()[..], IPV6_PACKET.bytes);
    }

    #[test]
    fn test_parse_serialize_with_extension_headers() {
        // NB; Use MLD as test data arbitrarily, because it includes IPv6
        // extension headers.
        use crate::testdata::mld_router_report::*;

        let mut buf = REPORT;
        let packet = buf.parse::<Ipv6Packet<_>>().unwrap();
        assert_eq!(packet.iter_extension_hdrs().count(), 1);

        // NB: Don't verify serialization via builders, as they omit IPv6
        // extension headers.

        // Verify serialization via `to_vec`.
        assert_eq!(&packet.to_vec()[..], REPORT);
    }

    fn fixed_hdr_to_bytes(fixed_hdr: FixedHeader) -> [u8; IPV6_FIXED_HDR_LEN] {
        zerocopy::transmute!(fixed_hdr)
    }

    // Return a new FixedHeader with reasonable defaults.
    fn new_fixed_hdr() -> FixedHeader {
        FixedHeader::new(
            DscpAndEcn::new(0, 2),
            0x77,
            0,
            IpProto::Tcp.into(),
            64,
            DEFAULT_SRC_IP,
            DEFAULT_DST_IP,
        )
    }

    #[test]
    fn test_parse() {
        let mut buf = &fixed_hdr_to_bytes(new_fixed_hdr())[..];
        let packet = buf.parse::<Ipv6Packet<_>>().unwrap();
        assert_eq!(packet.dscp_and_ecn().dscp(), 0);
        assert_eq!(packet.dscp_and_ecn().ecn(), 2);
        assert_eq!(packet.flowlabel(), 0x77);
        assert_eq!(packet.hop_limit(), 64);
        assert_eq!(packet.fixed_hdr.next_hdr, IpProto::Tcp.into());
        assert_eq!(packet.proto(), IpProto::Tcp.into());
        assert_eq!(packet.src_ip(), DEFAULT_SRC_IP);
        assert_eq!(packet.dst_ip(), DEFAULT_DST_IP);
        assert_eq!(packet.body(), []);
    }

    #[test]
    fn test_parse_with_ext_hdrs() {
        #[rustfmt::skip]
        let mut buf = [
            // FixedHeader (will be replaced later)
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,

            // HopByHop Options Extension Header
            Ipv6ExtHdrType::Routing.into(), // Next Header
            0,                       // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                       // Pad1
            1, 0,                    // Pad2
            1, 1, 0,                 // Pad3

            // Routing Extension Header
            Ipv6ExtHdrType::DestinationOptions.into(), // Next Header
            4,                                  // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                                  // Routing Type (Deprecated as per RFC 5095)
            0,                                  // Segments Left
            0, 0, 0, 0,                         // Reserved
            // Addresses for Routing Header w/ Type 0
            0,  1,  2,  3,  4,  5,  6,  7,  8,  9,  10, 11, 12, 13, 14, 15,
            16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,

            // Destination Options Extension Header
            IpProto::Tcp.into(),    // Next Header
            1,                      // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                      // Pad1
            1, 0,                   // Pad2
            1, 1, 0,                // Pad3
            1, 6, 0, 0, 0, 0, 0, 0, // Pad8

            // Body
            1, 2, 3, 4, 5,
        ];
        let mut fixed_hdr = new_fixed_hdr();
        fixed_hdr.next_hdr = Ipv6ExtHdrType::HopByHopOptions.into();
        fixed_hdr.payload_len = U16::new((buf.len() - IPV6_FIXED_HDR_LEN) as u16);
        let fixed_hdr_buf = fixed_hdr_to_bytes(fixed_hdr);
        buf[..IPV6_FIXED_HDR_LEN].copy_from_slice(&fixed_hdr_buf);
        let mut buf = &buf[..];
        let packet = buf.parse::<Ipv6Packet<_>>().unwrap();
        assert_eq!(packet.dscp_and_ecn().dscp(), 0);
        assert_eq!(packet.dscp_and_ecn().ecn(), 2);
        assert_eq!(packet.flowlabel(), 0x77);
        assert_eq!(packet.hop_limit(), 64);
        assert_eq!(packet.fixed_hdr.next_hdr, Ipv6ExtHdrType::HopByHopOptions.into());
        assert_eq!(packet.proto(), IpProto::Tcp.into());
        assert_eq!(packet.src_ip(), DEFAULT_SRC_IP);
        assert_eq!(packet.dst_ip(), DEFAULT_DST_IP);
        assert_eq!(packet.body(), [1, 2, 3, 4, 5]);
        let ext_hdrs: Vec<Ipv6ExtensionHeader<'_>> = packet.iter_extension_hdrs().collect();
        assert_eq!(ext_hdrs.len(), 3);
        // Check first extension header (hop-by-hop options)
        if let Ipv6ExtensionHeader::HopByHopOptions { options } = &ext_hdrs[0] {
            // Everything should have been a NOP/ignore
            assert_eq!(options.iter().count(), 0);
        } else {
            panic!("Should have matched HopByHopOptions!");
        }

        // Check second extension header (routing)
        if let Ipv6ExtensionHeader::Routing { routing_data } = &ext_hdrs[1] {
            assert_eq!(routing_data.routing_type(), Err(RoutingTypeParseError::UnsupportedType(0)));
            assert_eq!(routing_data.segments_left(), 0);
        } else {
            panic!("Should have matched RoutingExtensionHeader: {:?}", &ext_hdrs[1]);
        }

        // Check the third extension header (destination options)
        if let Ipv6ExtensionHeader::DestinationOptions { options } = &ext_hdrs[2] {
            // Everything should have been a NOP/ignore
            assert_eq!(options.iter().count(), 0);
        } else {
            panic!("Should have matched DestinationOptions!");
        }

        // Test with a NoNextHeader as the final Next Header
        #[rustfmt::skip]
        let mut buf = [
            // FixedHeader (will be replaced later)
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,

            // HopByHop Options Extension Header w/ NoNextHeader as the next header
            Ipv6Proto::NoNextHeader.into(), // Next Header
            0,                       // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                       // Pad1
            1, 0,                    // Pad2
            1, 1, 0,                 // Pad3

            // Body
            1, 2, 3, 4, 5,
        ];
        let mut fixed_hdr = new_fixed_hdr();
        fixed_hdr.next_hdr = Ipv6ExtHdrType::HopByHopOptions.into();
        fixed_hdr.payload_len = U16::new((buf.len() - IPV6_FIXED_HDR_LEN) as u16);
        let fixed_hdr_buf = fixed_hdr_to_bytes(fixed_hdr);
        buf[..IPV6_FIXED_HDR_LEN].copy_from_slice(&fixed_hdr_buf);
        let mut buf = &buf[..];
        let packet = buf.parse::<Ipv6Packet<_>>().unwrap();
        assert_eq!(packet.dscp_and_ecn().dscp(), 0);
        assert_eq!(packet.dscp_and_ecn().ecn(), 2);
        assert_eq!(packet.flowlabel(), 0x77);
        assert_eq!(packet.hop_limit(), 64);
        assert_eq!(packet.fixed_hdr.next_hdr, Ipv6ExtHdrType::HopByHopOptions.into());
        assert_eq!(packet.proto(), Ipv6Proto::NoNextHeader);
        assert_eq!(packet.src_ip(), DEFAULT_SRC_IP);
        assert_eq!(packet.dst_ip(), DEFAULT_DST_IP);
        assert_eq!(packet.body(), [1, 2, 3, 4, 5]);
        let ext_hdrs: Vec<Ipv6ExtensionHeader<'_>> = packet.iter_extension_hdrs().collect();
        assert_eq!(ext_hdrs.len(), 1);
        // Check first extension header (hop-by-hop options)
        if let Ipv6ExtensionHeader::HopByHopOptions { options } = &ext_hdrs[0] {
            // Everything should have been a NOP/ignore
            assert_eq!(options.iter().count(), 0);
        } else {
            panic!("Should have matched HopByHopOptions!");
        }
    }

    #[test]
    fn test_parse_error() {
        // Set the version to 5. The version must be 6.
        let mut fixed_hdr = new_fixed_hdr();
        fixed_hdr.version_tc_flowlabel[0] = 0x50;
        assert_eq!(
            (&fixed_hdr_to_bytes(fixed_hdr)[..]).parse::<Ipv6Packet<_>>().unwrap_err(),
            ParseError::Format.into()
        );

        // Set the payload len to 2, even though there's no payload.
        let mut fixed_hdr = new_fixed_hdr();
        fixed_hdr.payload_len = U16::new(2);
        assert_eq!(
            (&fixed_hdr_to_bytes(fixed_hdr)[..]).parse::<Ipv6Packet<_>>().unwrap_err(),
            ParseError::Format.into()
        );

        // Use invalid next header.
        let mut fixed_hdr = new_fixed_hdr();
        fixed_hdr.next_hdr = 255;
        let packet = fixed_hdr_to_bytes(fixed_hdr);

        // Raw parsing should succeed even with unrecognized Next Header.
        assert!((&packet[..]).parse::<Ipv6PacketRaw<_>>().is_ok());

        // Full parse should fail with unrecognized next header error.
        assert_eq!(
            (&packet[..]).parse::<Ipv6Packet<_>>().unwrap_err(),
            Ipv6ParseError::ParameterProblem {
                src_ip: DEFAULT_SRC_IP,
                dst_ip: DEFAULT_DST_IP,
                code: Icmpv6ParameterProblemCode::UnrecognizedNextHeaderType,
                pointer: u32::from(NEXT_HEADER_OFFSET),
                must_send_icmp: false,
                action: IpParseErrorAction::DiscardPacketSendIcmpNoMulticast,
            }
        );

        // Use ICMP(v4) as next header.
        let mut fixed_hdr = new_fixed_hdr();
        fixed_hdr.next_hdr = Ipv4Proto::Icmp.into();
        assert_eq!(
            (&fixed_hdr_to_bytes(fixed_hdr)[..]).parse::<Ipv6Packet<_>>().unwrap_err(),
            Ipv6ParseError::ParameterProblem {
                src_ip: DEFAULT_SRC_IP,
                dst_ip: DEFAULT_DST_IP,
                code: Icmpv6ParameterProblemCode::UnrecognizedNextHeaderType,
                pointer: u32::from(NEXT_HEADER_OFFSET),
                must_send_icmp: false,
                action: IpParseErrorAction::DiscardPacketSendIcmpNoMulticast,
            }
        );

        // Test HopByHop extension header not being the very first extension header
        #[rustfmt::skip]
        let mut buf = [
            // FixedHeader (will be replaced later)
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,

            // Routing Extension Header
            Ipv6ExtHdrType::HopByHopOptions.into(),    // Next Header (Valid but HopByHop restricted to first extension header)
            4,                                  // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                                  // Routing Type
            0,                                  // Segments Left
            0, 0, 0, 0,                         // Reserved
            // Addresses for Routing Header w/ Type 0
            0,  1,  2,  3,  4,  5,  6,  7,  8,  9,  10, 11, 12, 13, 14, 15,
            16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,

            // HopByHop Options Extension Header
            IpProto::Tcp.into(),             // Next Header
            0,                                  // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                                  // Pad1
            1, 0,                               // Pad2
            1, 1, 0,                            // Pad3

            // Body
            1, 2, 3, 4, 5,
        ];
        let mut fixed_hdr = new_fixed_hdr();
        fixed_hdr.next_hdr = Ipv6ExtHdrType::Routing.into();
        fixed_hdr.payload_len = U16::new((buf.len() - IPV6_FIXED_HDR_LEN) as u16);
        let fixed_hdr_buf = fixed_hdr_to_bytes(fixed_hdr);
        buf[..IPV6_FIXED_HDR_LEN].copy_from_slice(&fixed_hdr_buf);
        let mut buf = &buf[..];
        assert_eq!(
            buf.parse::<Ipv6Packet<_>>().unwrap_err(),
            Ipv6ParseError::ParameterProblem {
                src_ip: DEFAULT_SRC_IP,
                dst_ip: DEFAULT_DST_IP,
                code: Icmpv6ParameterProblemCode::UnrecognizedNextHeaderType,
                pointer: IPV6_FIXED_HDR_LEN as u32,
                must_send_icmp: false,
                action: IpParseErrorAction::DiscardPacketSendIcmpNoMulticast,
            }
        );

        // Test Unrecognized Routing Type
        #[rustfmt::skip]
        let mut buf = [
            // FixedHeader (will be replaced later)
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,

            // Routing Extension Header
            IpProto::Tcp.into(),                // Next Header
            4,                                  // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            255,                                // Routing Type (Invalid)
            1,                                  // Segments Left
            0, 0, 0, 0,                         // Reserved
            // Addresses for Routing Header w/ Type 0
            0,  1,  2,  3,  4,  5,  6,  7,  8,  9,  10, 11, 12, 13, 14, 15,
            16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,

            // Body
            1, 2, 3, 4, 5,
        ];
        let mut fixed_hdr = new_fixed_hdr();
        fixed_hdr.next_hdr = Ipv6ExtHdrType::Routing.into();
        fixed_hdr.payload_len = U16::new((buf.len() - IPV6_FIXED_HDR_LEN) as u16);
        let fixed_hdr_buf = fixed_hdr_to_bytes(fixed_hdr);
        buf[..IPV6_FIXED_HDR_LEN].copy_from_slice(&fixed_hdr_buf);
        let expected_error = Ipv6ParseError::ParameterProblem {
            src_ip: DEFAULT_SRC_IP,
            dst_ip: DEFAULT_DST_IP,
            code: Icmpv6ParameterProblemCode::ErroneousHeaderField,
            pointer: (IPV6_FIXED_HDR_LEN as u32) + 2,
            must_send_icmp: true,
            action: IpParseErrorAction::DiscardPacketSendIcmpNoMulticast,
        };
        assert_eq!((&buf[..]).parse::<Ipv6Packet<_>>().unwrap_err(), expected_error);

        // Test an unrecognized Routing Type with an unrecognized Next Header.
        // This shouldn't change the result: each header should be processed
        // before validating the Next Header field.
        buf[IPV6_FIXED_HDR_LEN] = 250; // Next Header (Invalid)

        assert_eq!((&buf[..]).parse::<Ipv6Packet<_>>().unwrap_err(), expected_error);
    }

    #[test]
    fn test_parse_all_next_header_values() {
        // Test that, when parsing a packet with the fixed header's Next Header
        // field set to any value, parsing does not panic. A previous version
        // of this code would panic on some Next Header values.

        // This packet was found via fuzzing to trigger a panic.
        let mut buf = [
            0x81, 0x13, 0x27, 0xeb, 0x75, 0x92, 0x33, 0x89, 0x01, 0xcc, 0xcc, 0xcc, 0xcc, 0xcc,
            0x03, 0x70, 0x00, 0x22, 0xf7, 0x30, 0x2c, 0x06, 0xfe, 0xc9, 0x00, 0x2d, 0x3b, 0xeb,
            0xad, 0x3e, 0x5c, 0x41, 0xc8, 0x70, 0xff, 0xff, 0xff, 0xff, 0xff, 0xf6, 0x11, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x4f, 0x4f, 0x4f, 0x6f, 0x4f, 0x4f, 0x4f, 0x4f,
            0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x19, 0x19,
            0x19, 0x19, 0x19, 0x4f, 0x4f, 0x4f, 0x4f, 0x29, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x4f, 0x4f, 0x5a, 0x5a, 0x5a, 0xc9, 0x5a, 0x46, 0x5a, 0x5a, 0x5a, 0x5a, 0x5a,
            0x5a, 0x5a, 0x5a, 0x5a, 0x5a, 0x5a, 0x5a, 0x5a, 0xe4, 0x5a, 0x5a, 0x5a, 0x5a,
        ];

        // First, assert that the Next Header value found by the fuzzer (51)
        // produces the error we expect.
        assert_matches!(
            (&buf[..]).parse::<Ipv6Packet<_>>(),
            Err(Ipv6ParseError::ParameterProblem {
                src_ip: _,
                dst_ip: _,
                code: Icmpv6ParameterProblemCode::UnrecognizedNextHeaderType,
                pointer: 6,
                must_send_icmp: false,
                action: IpParseErrorAction::DiscardPacketSendIcmpNoMulticast,
            })
        );

        // Second, ensure that, regardless of the exact result produced, no Next
        // Header value causes parsing to panic.
        for b in 0u8..=255 {
            // Overwrite the Next Header field.
            buf[6] = b;
            let _: Result<_, _> = (&buf[..]).parse::<Ipv6Packet<_>>();
        }
    }

    #[test]
    fn test_esp_packet() {
        // Encapsulating Security Payload (ESP) is not supported yet. Verify
        // that for ESP packets Ipv6Packet parsing fails, but Ipv6PacketRaw
        // parser succeeds.

        const ESP_NEXT_HEADER: u8 = 50;

        // 1. ESP as first extension header (Next Header in Fixed Header = ESP)
        let mut fixed_hdr = new_fixed_hdr();
        fixed_hdr.next_hdr = ESP_NEXT_HEADER;
        fixed_hdr.payload_len = U16::new(8);
        let mut buf = fixed_hdr_to_bytes(fixed_hdr).to_vec();
        buf.extend_from_slice(&[0; 8]);

        // Ipv6Packet parsing fails.
        assert_matches!(
            (&buf[..]).parse::<Ipv6Packet<_>>(),
            Err(Ipv6ParseError::ParameterProblem {
                src_ip: _,
                dst_ip: _,
                code: Icmpv6ParameterProblemCode::UnrecognizedNextHeaderType,
                pointer: 6, // Next Header field in Fixed Header.
                must_send_icmp: false,
                action: IpParseErrorAction::DiscardPacketSendIcmpNoMulticast,
            })
        );

        // Ipv6PacketRaw parsing succeeds
        let mut buf_ref = &buf[..];
        assert!(buf_ref.parse::<Ipv6PacketRaw<_>>().is_ok());

        // 2. ESP in middle (after Hop-by-Hop)
        let mut fixed_hdr = new_fixed_hdr();
        fixed_hdr.next_hdr = Ipv6ExtHdrType::HopByHopOptions.into();
        fixed_hdr.payload_len = U16::new(16);
        let mut buf = fixed_hdr_to_bytes(fixed_hdr).to_vec();
        // Hop-by-Hop header: Next Header = ESP (50), Hdr Ext Len = 0 (8 bytes total)
        buf.extend_from_slice(&[ESP_NEXT_HEADER, 0, 0, 0, 0, 0, 0, 0]);
        // ESP body
        buf.extend_from_slice(&[0; 8]);

        // Ipv6Packet parsing fails
        assert_matches!(
            (&buf[..]).parse::<Ipv6Packet<_>>(),
            Err(Ipv6ParseError::ParameterProblem {
                src_ip: _,
                dst_ip: _,
                code: Icmpv6ParameterProblemCode::UnrecognizedNextHeaderType,
                pointer: 40, // Next Header in the Hop-by-Hop header.
                must_send_icmp: false,
                action: IpParseErrorAction::DiscardPacketSendIcmpNoMulticast,
            })
        );

        // Ipv6PacketRaw parsing succeeds
        let mut buf_ref = &buf[..];
        assert!(buf_ref.parse::<Ipv6PacketRaw<_>>().is_ok());
    }

    #[test]
    fn test_parse_ext_hdr_unrecognized_next_header() {
        // Test that parsing an IPv6 packet with an unrecognized Next Header value
        // in an extension header succeeds for Ipv6PacketRaw, but fails for Ipv6Packet.

        #[rustfmt::skip]
        let mut buf = [
            // FixedHeader (will be replaced later)
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,

            // HopByHop Options Extension Header
            250,                     // Next Header (unrecognized next header type)
            0,                       // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                       // Pad1
            1, 0,                    // Pad2
            1, 1, 0,                 // Pad3

            // Body
            1, 2, 3, 4, 5,
        ];
        let mut fixed_hdr = new_fixed_hdr();
        fixed_hdr.next_hdr = Ipv6ExtHdrType::HopByHopOptions.into();
        fixed_hdr.payload_len = U16::new((buf.len() - IPV6_FIXED_HDR_LEN) as u16);
        let fixed_hdr_buf = fixed_hdr_to_bytes(fixed_hdr);
        buf[..IPV6_FIXED_HDR_LEN].copy_from_slice(&fixed_hdr_buf);

        // Raw parsing should succeed.
        assert!((&buf[..]).parse::<Ipv6PacketRaw<_>>().is_ok());

        // Full packet validation should fail.
        assert_eq!(
            (&buf[..]).parse::<Ipv6Packet<_>>().unwrap_err(),
            Ipv6ParseError::ParameterProblem {
                src_ip: DEFAULT_SRC_IP,
                dst_ip: DEFAULT_DST_IP,
                code: Icmpv6ParameterProblemCode::UnrecognizedNextHeaderType,
                pointer: IPV6_FIXED_HDR_LEN as u32,
                must_send_icmp: false,
                action: IpParseErrorAction::DiscardPacketSendIcmpNoMulticast,
            }
        );
    }

    #[test]
    fn test_partial_parse() {
        use core::convert::TryInto as _;
        use core::ops::Deref as _;

        // Can't partial parse extension headers:
        #[rustfmt::skip]
        let mut buf = [
            // FixedHeader (will be replaced later)
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,

            // HopByHop Options Extension Header
            IpProto::Tcp.into(), // Next Header
            0,                   // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                   // Pad1
            1, 0,                // Pad2
            1, 1, 0,             // Pad3

            // Body
            1, 2, 3, 4, 5,
        ];
        let len = buf.len() - IPV6_FIXED_HDR_LEN;
        let len = len.try_into().unwrap();
        let make_fixed_hdr = || {
            let mut fixed_hdr = new_fixed_hdr();
            fixed_hdr.next_hdr = Ipv6ExtHdrType::HopByHopOptions.into();
            fixed_hdr.payload_len = U16::new(len);
            fixed_hdr
        };
        // make HopByHop malformed:
        const MALFORMED_BYTE: u8 = 10;
        buf[IPV6_FIXED_HDR_LEN + 1] = MALFORMED_BYTE;
        let fixed_hdr = fixed_hdr_to_bytes(make_fixed_hdr());
        buf[..IPV6_FIXED_HDR_LEN].copy_from_slice(&fixed_hdr);
        let mut buf = &buf[..];
        let partial = buf.parse::<Ipv6PacketRaw<_>>().unwrap();
        let Ipv6PacketRaw { fixed_hdr, extension_hdrs, body_proto } = &partial;
        assert_eq!(fixed_hdr.deref(), &make_fixed_hdr());
        let b = extension_hdrs.as_ref().incomplete().unwrap();
        assert_eq!(*b, &[IpProto::Tcp.into(), MALFORMED_BYTE][..]);
        assert_eq!(body_proto, &Err(ExtHdrParseError));
        assert!(Ipv6Packet::try_from_raw(partial).is_err());

        // Incomplete body:
        let mut buf = [
            // FixedHeader (will be replaced later)
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // Body
            1, 2, 3, 4, 5,
        ];
        let make_fixed_hdr = || {
            let mut fixed_hdr = new_fixed_hdr();
            fixed_hdr.next_hdr = IpProto::Tcp.into();
            fixed_hdr.payload_len = U16::new(10);
            fixed_hdr
        };
        let fixed_hdr = fixed_hdr_to_bytes(make_fixed_hdr());
        buf[..IPV6_FIXED_HDR_LEN].copy_from_slice(&fixed_hdr);
        let mut parsebuff = &buf[..];
        let partial = parsebuff.parse::<Ipv6PacketRaw<_>>().unwrap();
        let Ipv6PacketRaw { fixed_hdr, extension_hdrs, body_proto } = &partial;
        assert_eq!(fixed_hdr.deref(), &make_fixed_hdr());
        assert_eq!(extension_hdrs.as_ref().complete().unwrap().deref(), []);
        let (body, proto) = body_proto.unwrap();
        assert_eq!(body.incomplete().unwrap(), &buf[IPV6_FIXED_HDR_LEN..]);
        assert_eq!(proto, IpProto::Tcp.into());
        assert!(Ipv6Packet::try_from_raw(partial).is_err());
    }

    // Return a stock Ipv6PacketBuilder with reasonable default values.
    fn new_builder() -> Ipv6PacketBuilder {
        Ipv6PacketBuilder::new(DEFAULT_SRC_IP, DEFAULT_DST_IP, 64, IpProto::Tcp.into())
    }

    #[test]
    fn test_serialize() {
        let mut builder = new_builder();
        builder.dscp_and_ecn(DscpAndEcn::new(0x12, 3));
        builder.flowlabel(0x10405);
        let mut buf = (&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9])
            .into_serializer()
            .wrap_in(builder)
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap();
        // assert that we get the literal bytes we expected
        assert_eq!(
            buf.as_ref(),
            &[
                100, 177, 4, 5, 0, 10, 6, 64, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
                16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 0, 1, 2, 3, 4,
                5, 6, 7, 8, 9
            ][..],
        );

        let packet = buf.parse::<Ipv6Packet<_>>().unwrap();
        // assert that when we parse those bytes, we get the values we set in
        // the builder
        assert_eq!(packet.dscp_and_ecn().dscp(), 0x12);
        assert_eq!(packet.dscp_and_ecn().ecn(), 3);
        assert_eq!(packet.flowlabel(), 0x10405);
    }

    #[test]
    fn test_partial_serialize() {
        let mut builder = new_builder();
        builder.dscp_and_ecn(DscpAndEcn::new(0x12, 3));
        builder.flowlabel(0x10405);
        const BODY: &[u8] = &[0, 1, 2, 3, 3, 4, 5, 7, 8, 9];
        let packet = (&BODY).into_serializer().wrap_in(builder);

        // Note that this header is different from the one in test_serialize
        // because the checksum is not calculated.
        const HEADER: &[u8] = &[
            100, 177, 4, 5, 0, 10, 6, 64, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16,
            17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32,
        ];
        const PACKET_SIZE: usize = HEADER.len() + BODY.len();

        // PartialSerializer serializes the header only if the buffer is
        // large enough to fit the whole header.
        let buf = assert_matches!(
            packet.partial_serialize(&mut NoOpSerializationContext, packet::new_buf_vec),
            Ok(PartialSerializeResult::NewBuffer { buffer, total_size: PACKET_SIZE }) => buffer
        );
        assert_eq!(buf.as_ref(), HEADER);
    }

    #[test]
    fn test_serialize_zeroes() {
        // Test that Ipv6PacketBuilder::serialize properly zeroes memory before
        // serializing the header.
        let mut buf_0 = [0; IPV6_FIXED_HDR_LEN];
        let _: Buf<&mut [u8]> = Buf::new(&mut buf_0[..], IPV6_FIXED_HDR_LEN..)
            .wrap_in(new_builder())
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap()
            .unwrap_a();
        let mut buf_1 = [0xFF; IPV6_FIXED_HDR_LEN];
        let _: Buf<&mut [u8]> = Buf::new(&mut buf_1[..], IPV6_FIXED_HDR_LEN..)
            .wrap_in(new_builder())
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap()
            .unwrap_a();
        assert_eq!(&buf_0[..], &buf_1[..]);
    }

    #[test]
    fn test_packet_builder_proto_not_next_header() {
        // Test that Ipv6PacketBuilder's `proto` field is used as the Protocol
        // Number for the upper layer payload, not the Next Header value for the
        // extension header.
        let mut buf = (&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9])
            .into_serializer()
            .wrap_in(
                Ipv6PacketBuilderWithHbhOptions::new(
                    new_builder(),
                    &[HopByHopOption {
                        action: ExtensionHeaderOptionAction::SkipAndContinue,
                        mutable: false,
                        data: HopByHopOptionData::RouterAlert { data: 0 },
                    }],
                )
                .unwrap(),
            )
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap();
        let packet = buf.parse::<Ipv6Packet<_>>().unwrap();
        assert_eq!(packet.proto(), IpProto::Tcp.into());
        assert_eq!(packet.next_header(), Ipv6ExtHdrType::HopByHopOptions.into());
    }

    #[test]
    #[should_panic(expected = "SizeLimitExceeded, Nested { inner: Buf { buf:")]
    fn test_serialize_panic_packet_length() {
        // Test that a packet whose payload is longer than 2^16 - 1 bytes is
        // rejected.
        let _: Buf<&mut [u8]> = Buf::new(&mut [0; 1 << 16][..], ..)
            .wrap_in(new_builder())
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap()
            .unwrap_a();
    }

    #[test]
    #[should_panic(expected = "packet must have at least one extension header")]
    fn test_copy_header_bytes_for_fragment_without_ext_hdrs() {
        let mut buf = &fixed_hdr_to_bytes(new_fixed_hdr())[..];
        let packet = buf.parse::<Ipv6Packet<_>>().unwrap();
        let _: Vec<_> = packet.copy_header_bytes_for_fragment();
    }

    #[test]
    #[should_panic(expected = "exhausted all extension headers without finding fragment header")]
    fn test_copy_header_bytes_for_fragment_with_1_ext_hdr_no_fragment() {
        #[rustfmt::skip]
        let mut buf = [
            // FixedHeader (will be replaced later)
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,

            // HopByHop Options Extension Header
            IpProto::Tcp.into(),     // Next Header
            0,                       // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                       // Pad1
            1, 0,                    // Pad2
            1, 1, 0,                 // Pad3

            // Body
            1, 2, 3, 4, 5,
        ];
        let mut fixed_hdr = new_fixed_hdr();
        fixed_hdr.next_hdr = Ipv6ExtHdrType::HopByHopOptions.into();
        fixed_hdr.payload_len = U16::new((buf.len() - IPV6_FIXED_HDR_LEN) as u16);
        let fixed_hdr_buf = fixed_hdr_to_bytes(fixed_hdr);
        buf[..IPV6_FIXED_HDR_LEN].copy_from_slice(&fixed_hdr_buf);
        let mut buf = &buf[..];
        let packet = buf.parse::<Ipv6Packet<_>>().unwrap();
        let _: Vec<_> = packet.copy_header_bytes_for_fragment();
    }

    #[test]
    #[should_panic(expected = "exhausted all extension headers without finding fragment header")]
    fn test_copy_header_bytes_for_fragment_with_2_ext_hdr_no_fragment() {
        #[rustfmt::skip]
        let mut buf = [
            // FixedHeader (will be replaced later)
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,

            // HopByHop Options Extension Header
            Ipv6ExtHdrType::DestinationOptions.into(), // Next Header
            0,                       // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                       // Pad1
            1, 0,                    // Pad2
            1, 1, 0,                 // Pad3

            // Destination Options Extension Header
            IpProto::Tcp.into(),    // Next Header
            1,                      // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                      // Pad1
            1, 0,                   // Pad2
            1, 1, 0,                // Pad3
            1, 6, 0, 0, 0, 0, 0, 0, // Pad8

            // Body
            1, 2, 3, 4, 5,
        ];
        let mut fixed_hdr = new_fixed_hdr();
        fixed_hdr.next_hdr = Ipv6ExtHdrType::HopByHopOptions.into();
        fixed_hdr.payload_len = U16::new((buf.len() - IPV6_FIXED_HDR_LEN) as u16);
        let fixed_hdr_buf = fixed_hdr_to_bytes(fixed_hdr);
        buf[..IPV6_FIXED_HDR_LEN].copy_from_slice(&fixed_hdr_buf);
        let mut buf = &buf[..];
        let packet = buf.parse::<Ipv6Packet<_>>().unwrap();
        let _: Vec<_> = packet.copy_header_bytes_for_fragment();
    }

    #[test]
    fn test_copy_header_bytes_for_fragment() {
        //
        // Only a fragment extension header
        //

        #[rustfmt::skip]
        let mut bytes = [
            // FixedHeader (will be replaced later)
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,

            // Fragment Extension Header
            IpProto::Tcp.into(),     // Next Header
            0,                       // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0, 0,                    // Fragment Offset, Res, M (M_flag)
            1, 1, 1, 1,              // Identification

            // Body
            1, 2, 3, 4, 5,
        ];
        let mut fixed_hdr = new_fixed_hdr();
        fixed_hdr.next_hdr = Ipv6ExtHdrType::Fragment.into();
        fixed_hdr.payload_len = U16::new((bytes.len() - IPV6_FIXED_HDR_LEN) as u16);
        let fixed_hdr_buf = fixed_hdr_to_bytes(fixed_hdr);
        bytes[..IPV6_FIXED_HDR_LEN].copy_from_slice(&fixed_hdr_buf);
        let mut buf = &bytes[..];
        let packet = buf.parse::<Ipv6Packet<_>>().unwrap();
        let copied_bytes = packet.copy_header_bytes_for_fragment();
        bytes[6] = IpProto::Tcp.into();
        assert_eq!(&copied_bytes[..], &bytes[..IPV6_FIXED_HDR_LEN]);

        //
        // Fragment header after a single extension header
        //

        #[rustfmt::skip]
        let mut bytes = [
            // FixedHeader (will be replaced later)
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,

            // HopByHop Options Extension Header
            Ipv6ExtHdrType::Fragment.into(),    // Next Header
            0,                       // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                       // Pad1
            1, 0,                    // Pad2
            1, 1, 0,                 // Pad3

            // Fragment Extension Header
            IpProto::Tcp.into(),     // Next Header
            0,                       // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0, 0,                    // Fragment Offset, Res, M (M_flag)
            1, 1, 1, 1,              // Identification

            // Body
            1, 2, 3, 4, 5,
        ];
        let mut fixed_hdr = new_fixed_hdr();
        fixed_hdr.next_hdr = Ipv6ExtHdrType::HopByHopOptions.into();
        fixed_hdr.payload_len = U16::new((bytes.len() - IPV6_FIXED_HDR_LEN) as u16);
        let fixed_hdr_buf = fixed_hdr_to_bytes(fixed_hdr);
        bytes[..IPV6_FIXED_HDR_LEN].copy_from_slice(&fixed_hdr_buf);
        let mut buf = &bytes[..];
        let packet = buf.parse::<Ipv6Packet<_>>().unwrap();
        let copied_bytes = packet.copy_header_bytes_for_fragment();
        bytes[IPV6_FIXED_HDR_LEN] = IpProto::Tcp.into();
        assert_eq!(&copied_bytes[..], &bytes[..IPV6_FIXED_HDR_LEN + 8]);

        //
        // Fragment header after many extension headers (many = 2)
        //

        #[rustfmt::skip]
        let mut bytes = [
            // FixedHeader (will be replaced later)
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,

            // HopByHop Options Extension Header
            Ipv6ExtHdrType::DestinationOptions.into(), // Next Header
            0,                       // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                       // Pad1
            1, 0,                    // Pad2
            1, 1, 0,                 // Pad3

            // Destination Options Extension Header
            Ipv6ExtHdrType::Fragment.into(),    // Next Header
            1,                      // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                      // Pad1
            1, 0,                   // Pad2
            1, 1, 0,                // Pad3
            1, 6, 0, 0, 0, 0, 0, 0, // Pad8

            // Fragment Extension Header
            IpProto::Tcp.into(),     // Next Header
            0,                       // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0, 0,                    // Fragment Offset, Res, M (M_flag)
            1, 1, 1, 1,              // Identification

            // Body
            1, 2, 3, 4, 5,
        ];
        let mut fixed_hdr = new_fixed_hdr();
        fixed_hdr.next_hdr = Ipv6ExtHdrType::HopByHopOptions.into();
        fixed_hdr.payload_len = U16::new((bytes.len() - IPV6_FIXED_HDR_LEN) as u16);
        let fixed_hdr_buf = fixed_hdr_to_bytes(fixed_hdr);
        bytes[..IPV6_FIXED_HDR_LEN].copy_from_slice(&fixed_hdr_buf);
        let mut buf = &bytes[..];
        let packet = buf.parse::<Ipv6Packet<_>>().unwrap();
        let copied_bytes = packet.copy_header_bytes_for_fragment();
        bytes[IPV6_FIXED_HDR_LEN + 8] = IpProto::Tcp.into();
        assert_eq!(&copied_bytes[..], &bytes[..IPV6_FIXED_HDR_LEN + 24]);

        //
        // Fragment header before an extension header
        //

        #[rustfmt::skip]
        let mut bytes = [
            // FixedHeader (will be replaced later)
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,

            // Fragment Extension Header
            Ipv6ExtHdrType::DestinationOptions.into(), // Next Header
            0,                       // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0, 0,                    // Fragment Offset, Res, M (M_flag)
            1, 1, 1, 1,              // Identification

            // Destination Options Extension Header
            IpProto::Tcp.into(),    // Next Header
            1,                      // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                      // Pad1
            1, 0,                   // Pad2
            1, 1, 0,                // Pad3
            1, 6, 0, 0, 0, 0, 0, 0, // Pad8

            // Body
            1, 2, 3, 4, 5,
        ];
        let mut fixed_hdr = new_fixed_hdr();
        fixed_hdr.next_hdr = Ipv6ExtHdrType::Fragment.into();
        fixed_hdr.payload_len = U16::new((bytes.len() - IPV6_FIXED_HDR_LEN) as u16);
        let fixed_hdr_buf = fixed_hdr_to_bytes(fixed_hdr);
        bytes[..IPV6_FIXED_HDR_LEN].copy_from_slice(&fixed_hdr_buf);
        let mut buf = &bytes[..];
        let packet = buf.parse::<Ipv6Packet<_>>().unwrap();
        let copied_bytes = packet.copy_header_bytes_for_fragment();
        let mut expected_bytes = Vec::new();
        expected_bytes.extend_from_slice(&bytes[..IPV6_FIXED_HDR_LEN]);
        expected_bytes.extend_from_slice(&bytes[IPV6_FIXED_HDR_LEN + 8..bytes.len() - 5]);
        expected_bytes[6] = Ipv6ExtHdrType::DestinationOptions.into();
        assert_eq!(&copied_bytes[..], &expected_bytes[..]);

        //
        // Fragment header before many extension headers (many = 2)
        //

        #[rustfmt::skip]
        let mut bytes = [
            // FixedHeader (will be replaced later)
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,

            // Fragment Extension Header
            Ipv6ExtHdrType::DestinationOptions.into(), // Next Header
            0,                       // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0, 0,                    // Fragment Offset, Res, M (M_flag)
            1, 1, 1, 1,              // Identification

            // Destination Options Extension Header
            Ipv6ExtHdrType::Routing.into(),    // Next Header
            1,                      // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                      // Pad1
            1, 0,                   // Pad2
            1, 1, 0,                // Pad3
            1, 6, 0, 0, 0, 0, 0, 0, // Pad8

            // Routing extension header
            IpProto::Tcp.into(),                // Next Header
            4,                                  // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                                  // Routing Type (Deprecated as per RFC 5095)
            0,                                  // Segments Left
            0, 0, 0, 0,                         // Reserved
            // Addresses for Routing Header w/ Type 0
            0,  1,  2,  3,  4,  5,  6,  7,  8,  9,  10, 11, 12, 13, 14, 15,
            16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,

            // Body
            1, 2, 3, 4, 5,
        ];
        let mut fixed_hdr = new_fixed_hdr();
        fixed_hdr.next_hdr = Ipv6ExtHdrType::Fragment.into();
        fixed_hdr.payload_len = U16::new((bytes.len() - IPV6_FIXED_HDR_LEN) as u16);
        let fixed_hdr_buf = fixed_hdr_to_bytes(fixed_hdr);
        bytes[..IPV6_FIXED_HDR_LEN].copy_from_slice(&fixed_hdr_buf);
        let mut buf = &bytes[..];
        let packet = buf.parse::<Ipv6Packet<_>>().unwrap();
        let copied_bytes = packet.copy_header_bytes_for_fragment();
        let mut expected_bytes = Vec::new();
        expected_bytes.extend_from_slice(&bytes[..IPV6_FIXED_HDR_LEN]);
        expected_bytes.extend_from_slice(&bytes[IPV6_FIXED_HDR_LEN + 8..bytes.len() - 5]);
        expected_bytes[6] = Ipv6ExtHdrType::DestinationOptions.into();
        assert_eq!(&copied_bytes[..], &expected_bytes[..]);

        //
        // Fragment header between extension headers
        //

        #[rustfmt::skip]
        let mut bytes = [
            // FixedHeader (will be replaced later)
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,

            // HopByHop Options Extension Header
            Ipv6ExtHdrType::Fragment.into(),    // Next Header
            0,                       // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                       // Pad1
            1, 0,                    // Pad2
            1, 1, 0,                 // Pad3

            // Fragment Extension Header
            Ipv6ExtHdrType::DestinationOptions.into(), // Next Header
            0,                       // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0, 0,                    // Fragment Offset, Res, M (M_flag)
            1, 1, 1, 1,              // Identification

            // Destination Options Extension Header
            IpProto::Tcp.into(),    // Next Header
            1,                      // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                      // Pad1
            1, 0,                   // Pad2
            1, 1, 0,                // Pad3
            1, 6, 0, 0, 0, 0, 0, 0, // Pad8

            // Body
            1, 2, 3, 4, 5,
        ];
        let mut fixed_hdr = new_fixed_hdr();
        fixed_hdr.next_hdr = Ipv6ExtHdrType::HopByHopOptions.into();
        fixed_hdr.payload_len = U16::new((bytes.len() - IPV6_FIXED_HDR_LEN) as u16);
        let fixed_hdr_buf = fixed_hdr_to_bytes(fixed_hdr);
        bytes[..IPV6_FIXED_HDR_LEN].copy_from_slice(&fixed_hdr_buf);
        let mut buf = &bytes[..];
        let packet = buf.parse::<Ipv6Packet<_>>().unwrap();
        let copied_bytes = packet.copy_header_bytes_for_fragment();
        let mut expected_bytes = Vec::new();
        expected_bytes.extend_from_slice(&bytes[..IPV6_FIXED_HDR_LEN + 8]);
        expected_bytes.extend_from_slice(&bytes[IPV6_FIXED_HDR_LEN + 16..bytes.len() - 5]);
        expected_bytes[IPV6_FIXED_HDR_LEN] = Ipv6ExtHdrType::DestinationOptions.into();
        assert_eq!(&copied_bytes[..], &expected_bytes[..]);

        //
        // Multiple fragment extension headers
        //

        #[rustfmt::skip]
        let mut bytes = [
            // FixedHeader (will be replaced later)
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,

            // Fragment Extension Header
            Ipv6ExtHdrType::Fragment.into(),     // Next Header
            0,                       // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0, 0,                    // Fragment Offset, Res, M (M_flag)
            1, 1, 1, 1,              // Identification

            // Fragment Extension Header
            IpProto::Tcp.into(),     // Next Header
            0,                       // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0, 0,                    // Fragment Offset, Res, M (M_flag)
            2, 2, 2, 2,              // Identification

            // Body
            1, 2, 3, 4, 5,
        ];
        let mut fixed_hdr = new_fixed_hdr();
        fixed_hdr.next_hdr = Ipv6ExtHdrType::Fragment.into();
        fixed_hdr.payload_len = U16::new((bytes.len() - IPV6_FIXED_HDR_LEN) as u16);
        let fixed_hdr_buf = fixed_hdr_to_bytes(fixed_hdr);
        bytes[..IPV6_FIXED_HDR_LEN].copy_from_slice(&fixed_hdr_buf);
        let mut buf = &bytes[..];
        let packet = buf.parse::<Ipv6Packet<_>>().unwrap();
        let copied_bytes = packet.copy_header_bytes_for_fragment();
        let mut expected_bytes = Vec::new();
        expected_bytes.extend_from_slice(&bytes[..IPV6_FIXED_HDR_LEN]);
        expected_bytes.extend_from_slice(&bytes[IPV6_FIXED_HDR_LEN + 8..bytes.len() - 5]);
        assert_eq!(&copied_bytes[..], &expected_bytes[..]);

        //
        // Fragment header immediately following Routing header.
        // Regression test for https://fxbug.dev/517297331.
        //

        #[rustfmt::skip]
        let mut bytes = [
            // FixedHeader (will be replaced later)
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,

            // HopByHop Options Extension Header
            Ipv6ExtHdrType::Routing.into(), // Next Header (Routing)
            0,                       // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                       // Pad1
            1, 0,                    // Pad2
            1, 1, 0,                 // Pad3

            // Routing extension header
            Ipv6ExtHdrType::Fragment.into(), // Next Header (Fragment)
            4,                       // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                       // Routing Type
            0,                       // Segments Left
            0, 0, 0, 0,              // Reserved
            // Addresses for Routing Header w/ Type 0
            0,  1,  2,  3,  4,  5,  6,  7,  8,  9,  10, 11, 12, 13, 14, 15,
            16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,

            // Fragment Extension Header
            IpProto::Tcp.into(),     // Next Header (TCP)
            0,                       // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0, 0,                    // Fragment Offset, Res, M (M_flag)
            1, 1, 1, 1,              // Identification

            // Body (TCP packet mock bytes)
            1, 2, 3, 4, 5,
        ];
        let mut fixed_hdr = new_fixed_hdr();
        fixed_hdr.next_hdr = Ipv6ExtHdrType::HopByHopOptions.into();
        fixed_hdr.payload_len = U16::new((bytes.len() - IPV6_FIXED_HDR_LEN) as u16);
        let fixed_hdr_buf = fixed_hdr_to_bytes(fixed_hdr);
        bytes[..IPV6_FIXED_HDR_LEN].copy_from_slice(&fixed_hdr_buf);
        let mut buf = &bytes[..];
        let packet = buf.parse::<Ipv6Packet<_>>().unwrap();
        let copied_bytes = packet.copy_header_bytes_for_fragment();
        let mut expected_bytes = Vec::new();
        // 8 (HopByHop) + (8 + 16 + 16) (Routing).
        expected_bytes.extend_from_slice(&bytes[..IPV6_FIXED_HDR_LEN + 48]);
        expected_bytes[IPV6_FIXED_HDR_LEN + 8] = IpProto::Tcp.into();
        assert_eq!(copied_bytes, expected_bytes);
    }

    #[test_case(
        &[],
        &[],
        None;
        "no_ext_hdrs"
    )]
    #[test_case(
        &[Ipv6ExtHdrType::DestinationOptions],
        &[],
        Some(Ipv6ExtHdrType::DestinationOptions);
        "ignore_dst_options"
    )]
    #[test_case(
        &[
            Ipv6ExtHdrType::DestinationOptions,
            Ipv6ExtHdrType::Routing,
            Ipv6ExtHdrType::DestinationOptions
        ],
        &[Ipv6ExtHdrType::DestinationOptions, Ipv6ExtHdrType::Routing],
        Some(Ipv6ExtHdrType::DestinationOptions);
        "include_routing_and_everything_before"
    )]
    #[test_case(
        &[Ipv6ExtHdrType::HopByHopOptions, Ipv6ExtHdrType::DestinationOptions],
        &[Ipv6ExtHdrType::HopByHopOptions],
        Some(Ipv6ExtHdrType::DestinationOptions);
        "include_hop_by_hop_if_first"
    )]
    #[test_case(
        &[
            Ipv6ExtHdrType::HopByHopOptions,
            Ipv6ExtHdrType::DestinationOptions,
            Ipv6ExtHdrType::Routing
        ],
        &[
            Ipv6ExtHdrType::HopByHopOptions,
            Ipv6ExtHdrType::DestinationOptions,
            Ipv6ExtHdrType::Routing
        ],
        None;
        "routing_header_takes_precedence_over_hop_by_hop"
    )]
    fn test_per_fragment_header_builder(
        original_ext_hdrs: &[Ipv6ExtHdrType],
        expected_ext_hdrs: &[Ipv6ExtHdrType],
        expected_next_hdr: Option<Ipv6ExtHdrType>,
    ) {
        #[rustfmt::skip]
        fn build_routing_header(nh: u8) -> [u8; 40] {
            [
                nh, 4,  0,  0,  0,  0,  0,  0,
                1,  2,  3,  4,  5,  6,  7,  8,
                9,  10, 11, 12, 13, 14, 15, 16,
                17, 18, 19, 20, 21, 22, 23, 24,
                25, 26, 27, 28, 29, 30, 31, 32
            ]
        }
        fn build_hop_by_hop_header(nh: u8) -> [u8; 8] {
            [nh, 0, 0, 1, 0, 1, 1, 0]
        }
        #[rustfmt::skip]
        fn build_destination_options_header(nh: u8) -> [u8; 16] {
            [
                nh, 1, 0, 1, 0, 1, 1, 0,
                1,  6, 0, 0, 0, 0, 0, 0
            ]
        }
        fn build_fragment_header(nh: u8, id: u32) -> [u8; 8] {
            let [id1, id2, id3, id4] = id.to_be_bytes();
            [nh, 0, 0, 0, id1, id2, id3, id4]
        }
        fn build_header_bytes(ext_hdrs: &[Ipv6ExtHdrType], final_header: u8) -> Vec<u8> {
            // Prepare the fixed header.
            let mut fixed_hdr = new_fixed_hdr();
            if ext_hdrs.is_empty() {
                fixed_hdr.next_hdr = final_header;
            } else {
                fixed_hdr.next_hdr = u8::from(ext_hdrs[0]);
            }

            // Prepare the extension headers.
            let mut ext_hdr_bytes = Vec::new();
            for i in 0..ext_hdrs.len() {
                let next_header =
                    if i + 1 >= ext_hdrs.len() { final_header } else { u8::from(ext_hdrs[i + 1]) };
                match ext_hdrs[i] {
                    Ipv6ExtHdrType::DestinationOptions => ext_hdr_bytes
                        .extend_from_slice(&build_destination_options_header(next_header)),
                    Ipv6ExtHdrType::Routing => {
                        ext_hdr_bytes.extend_from_slice(&build_routing_header(next_header))
                    }
                    Ipv6ExtHdrType::HopByHopOptions => {
                        ext_hdr_bytes.extend_from_slice(&build_hop_by_hop_header(next_header))
                    }
                    h => panic!("unexpected header type: {h}"),
                }
            }

            let mut bytes = fixed_hdr_to_bytes(fixed_hdr).to_vec();
            bytes.extend_from_slice(&ext_hdr_bytes[..]);
            bytes
        }

        // Generate the original packet.
        const BODY: [u8; 5] = [1, 2, 3, 4, 5];
        let payload_header = u8::from(IpProto::Tcp);
        let mut bytes = build_header_bytes(original_ext_hdrs, payload_header);
        bytes.extend_from_slice(&BODY);
        let len = u16::try_from(bytes.len() - IPV6_FIXED_HDR_LEN).expect("should fit in a u16");
        bytes.as_mut_slice()[IPV6_PAYLOAD_LEN_BYTE_RANGE].copy_from_slice(&len.to_be_bytes());
        let mut buf = &bytes[..];
        let packet = buf.parse::<Ipv6Packet<_>>().expect("parse should succeed");

        // Serialize the header directly.
        let serialized = packet
            .per_fragment_builder()
            .wrap_body(EmptyBuf)
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap()
            .unwrap_b();
        let expected_next_hdr = expected_next_hdr.map(u8::from).unwrap_or(payload_header);
        let mut expected_bytes = build_header_bytes(expected_ext_hdrs, expected_next_hdr);
        let len =
            u16::try_from(expected_bytes.len() - IPV6_FIXED_HDR_LEN).expect("should fit in a u16");
        expected_bytes.as_mut_slice()[IPV6_PAYLOAD_LEN_BYTE_RANGE]
            .copy_from_slice(&len.to_be_bytes());
        assert_eq!(serialized.as_ref(), &expected_bytes[..]);

        // Serialize the per fragment header, followed by a fragment header.
        // NB: Everything from the original packet that wasn't included in the
        // header is part of the body (i.e. skipped extension headers).
        let builder = packet.per_fragment_builder();
        let body = &bytes[builder.header_len()..];
        const ID: u32 = 0x12345678;
        let frag_builder =
            Ipv6PacketBuilderWithFragmentHeader::new(builder, FragmentOffset::ZERO, false, ID);
        let serialized = frag_builder
            .wrap_body(body.into_serializer())
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap()
            .unwrap_b();
        let mut expected_bytes =
            build_header_bytes(expected_ext_hdrs, u8::from(Ipv6ExtHdrType::Fragment));
        expected_bytes.extend_from_slice(&build_fragment_header(expected_next_hdr, ID));
        expected_bytes.extend_from_slice(body);
        let len =
            u16::try_from(expected_bytes.len() - IPV6_FIXED_HDR_LEN).expect("should fit in a u16");
        expected_bytes.as_mut_slice()[IPV6_PAYLOAD_LEN_BYTE_RANGE]
            .copy_from_slice(&len.to_be_bytes());
        assert_eq!(serialized.as_ref(), &expected_bytes[..]);
    }

    #[test]
    fn test_next_multiple_of_eight() {
        for x in 0usize..=IPV6_HBH_OPTIONS_MAX_LEN {
            let y = next_multiple_of_eight(x);
            assert_eq!(y % 8, 0);
            assert!(y >= x);
            if x % 8 == 0 {
                assert_eq!(x, y);
            } else {
                assert_eq!(x + (8 - x % 8), y);
            }
        }
    }

    fn create_ipv4_and_ipv6_builders(
        proto_v4: Ipv4Proto,
        proto_v6: Ipv6Proto,
    ) -> (Ipv4PacketBuilder, Ipv6PacketBuilder) {
        const IP_DSCP_AND_ECN: DscpAndEcn = DscpAndEcn::new(0x12, 3);
        const IP_TTL: u8 = 64;

        let mut ipv4_builder =
            Ipv4PacketBuilder::new(DEFAULT_V4_SRC_IP, DEFAULT_V4_DST_IP, IP_TTL, proto_v4);
        ipv4_builder.dscp_and_ecn(IP_DSCP_AND_ECN);
        ipv4_builder.df_flag(false);
        ipv4_builder.mf_flag(false);
        ipv4_builder.fragment_offset(FragmentOffset::ZERO);

        let mut ipv6_builder =
            Ipv6PacketBuilder::new(DEFAULT_SRC_IP, DEFAULT_DST_IP, IP_TTL, proto_v6);
        ipv6_builder.dscp_and_ecn(IP_DSCP_AND_ECN);
        ipv6_builder.flowlabel(0x456);

        (ipv4_builder, ipv6_builder)
    }

    fn create_tcp_ipv4_and_ipv6_pkt()
    -> (packet::Either<EmptyBuf, Buf<Vec<u8>>>, packet::Either<EmptyBuf, Buf<Vec<u8>>>) {
        use crate::tcp::TcpSegmentBuilder;
        use core::num::NonZeroU16;

        let tcp_src_port: NonZeroU16 = NonZeroU16::new(20).unwrap();
        let tcp_dst_port: NonZeroU16 = NonZeroU16::new(30).unwrap();
        const TCP_SEQ_NUM: u32 = 4321;
        const TCP_ACK_NUM: Option<u32> = Some(1234);
        const TCP_WINDOW_SIZE: u16 = 12345;
        const PAYLOAD: [u8; 10] = [0, 1, 2, 3, 3, 4, 5, 7, 8, 9];

        let (ipv4_builder, ipv6_builder) =
            create_ipv4_and_ipv6_builders(IpProto::Tcp.into(), IpProto::Tcp.into());

        let tcp_builder = TcpSegmentBuilder::new(
            DEFAULT_V4_SRC_IP,
            DEFAULT_V4_DST_IP,
            tcp_src_port,
            tcp_dst_port,
            TCP_SEQ_NUM,
            TCP_ACK_NUM,
            TCP_WINDOW_SIZE,
        );

        let v4_pkt_buf = (&PAYLOAD)
            .into_serializer()
            .wrap_in(tcp_builder)
            .wrap_in(ipv4_builder)
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .expect("Failed to serialize to v4_pkt_buf");

        let v6_tcp_builder = TcpSegmentBuilder::new(
            DEFAULT_SRC_IP,
            DEFAULT_DST_IP,
            tcp_src_port,
            tcp_dst_port,
            TCP_SEQ_NUM,
            TCP_ACK_NUM,
            TCP_WINDOW_SIZE,
        );

        let v6_pkt_buf = (&PAYLOAD)
            .into_serializer()
            .wrap_in(v6_tcp_builder)
            .wrap_in(ipv6_builder)
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .expect("Failed to serialize to v4_pkt_buf");

        (v4_pkt_buf, v6_pkt_buf)
    }

    #[test]
    fn test_nat64_translate_tcp() {
        let (expected_v4_pkt_buf, mut v6_pkt_buf) = create_tcp_ipv4_and_ipv6_pkt();

        let parsed_v6_packet =
            v6_pkt_buf.parse::<Ipv6Packet<_>>().expect("Failed to parse v6_pkt_buf");
        let nat64_translation_result =
            parsed_v6_packet.nat64_translate(DEFAULT_V4_SRC_IP, DEFAULT_V4_DST_IP);

        let serializable_pkt =
            assert_matches!(nat64_translation_result, Nat64TranslationResult::Forward(s) => s);

        let translated_v4_pkt_buf = serializable_pkt
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .expect("Failed to serialize to translated_v4_pkt_buf");

        assert_eq!(
            expected_v4_pkt_buf.to_flattened_vec(),
            translated_v4_pkt_buf.to_flattened_vec()
        );
    }

    fn create_udp_ipv4_and_ipv6_pkt()
    -> (packet::Either<EmptyBuf, Buf<Vec<u8>>>, packet::Either<EmptyBuf, Buf<Vec<u8>>>) {
        use crate::udp::UdpPacketBuilder;
        use core::num::NonZeroU16;

        let udp_src_port: NonZeroU16 = NonZeroU16::new(35000).unwrap();
        let udp_dst_port: NonZeroU16 = NonZeroU16::new(53).unwrap();
        const PAYLOAD: [u8; 10] = [0, 1, 2, 3, 3, 4, 5, 7, 8, 9];

        let (ipv4_builder, ipv6_builder) =
            create_ipv4_and_ipv6_builders(IpProto::Udp.into(), IpProto::Udp.into());

        let v4_udp_builder = UdpPacketBuilder::new(
            DEFAULT_V4_SRC_IP,
            DEFAULT_V4_DST_IP,
            Some(udp_src_port),
            udp_dst_port,
        );

        let v4_pkt_buf = (&PAYLOAD)
            .into_serializer()
            .wrap_in(v4_udp_builder)
            .wrap_in(ipv4_builder)
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .expect("Unable to serialize to v4_pkt_buf");

        let v6_udp_builder =
            UdpPacketBuilder::new(DEFAULT_SRC_IP, DEFAULT_DST_IP, Some(udp_src_port), udp_dst_port);

        let v6_pkt_buf = (&PAYLOAD)
            .into_serializer()
            .wrap_in(v6_udp_builder)
            .wrap_in(ipv6_builder)
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .expect("Unable to serialize to v6_pkt_buf");

        (v4_pkt_buf, v6_pkt_buf)
    }

    #[test]
    fn test_nat64_translate_udp() {
        let (expected_v4_pkt_buf, mut v6_pkt_buf) = create_udp_ipv4_and_ipv6_pkt();

        let parsed_v6_packet =
            v6_pkt_buf.parse::<Ipv6Packet<_>>().expect("Unable to parse Ipv6Packet");
        let nat64_translation_result =
            parsed_v6_packet.nat64_translate(DEFAULT_V4_SRC_IP, DEFAULT_V4_DST_IP);

        let serializable_pkt = assert_matches!(nat64_translation_result,
                                               Nat64TranslationResult::Forward(s) => s);

        let translated_v4_pkt_buf = serializable_pkt
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .expect("Unable to serialize to translated_v4_pkt_buf");

        assert_eq!(
            expected_v4_pkt_buf.to_flattened_vec(),
            translated_v4_pkt_buf.to_flattened_vec()
        );
    }

    #[test]
    fn test_nat64_translate_non_tcp_udp_icmp() {
        const PAYLOAD: [u8; 10] = [0, 1, 2, 3, 3, 4, 5, 7, 8, 9];

        let (ipv4_builder, ipv6_builder) =
            create_ipv4_and_ipv6_builders(Ipv4Proto::Other(59), Ipv6Proto::Other(59));

        let expected_v4_pkt_buf = (&PAYLOAD)
            .into_serializer()
            .wrap_in(ipv4_builder)
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .expect("Unable to serialize to expected_v4_pkt_buf");

        let mut v6_pkt_buf = (&PAYLOAD)
            .into_serializer()
            .wrap_in(ipv6_builder)
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .expect("Unable to serialize to v6_pkt_buf");

        let translated_v4_pkt_buf = {
            let parsed_v6_packet = v6_pkt_buf
                .parse::<Ipv6Packet<_>>()
                .expect("Unable to serialize to translated_v4_pkt_buf");

            let nat64_translation_result =
                parsed_v6_packet.nat64_translate(DEFAULT_V4_SRC_IP, DEFAULT_V4_DST_IP);

            let serializable_pkt = assert_matches!(nat64_translation_result,
                                                   Nat64TranslationResult::Forward(s) => s);

            let translated_buf = serializable_pkt
                .serialize_vec_outer(&mut NoOpSerializationContext)
                .expect("Unable to serialize to translated_buf");

            translated_buf
        };

        assert_eq!(
            expected_v4_pkt_buf.to_flattened_vec(),
            translated_v4_pkt_buf.to_flattened_vec()
        );
    }

    #[test_case(new_builder(), true; "fixed header more frags")]
    #[test_case(Ipv6PacketBuilderWithHbhOptions::new(
        new_builder(),
        &[HopByHopOption {
            action: ExtensionHeaderOptionAction::SkipAndContinue,
            mutable: false,
            data: HopByHopOptionData::RouterAlert { data: 0 },
        }]).unwrap(), false; "hbh last frag")]
    fn ipv6_packet_builder_with_fragment_header<
        B: Ipv6HeaderBuilder + Ipv6HeaderBefore<Ipv6PacketBuilderWithFragmentHeader<B>> + Debug,
    >(
        inner: B,
        more_fragments: bool,
    ) {
        const PAYLOAD: [u8; 10] = [0, 1, 2, 3, 3, 4, 5, 7, 8, 9];
        let fragment_offset = FragmentOffset::new(13).unwrap();
        let identification = 0xABCDABCD;
        let builder = Ipv6PacketBuilderWithFragmentHeader::new(
            inner,
            fragment_offset,
            more_fragments,
            identification,
        );
        let mut serialized = builder
            .wrap_body(PAYLOAD.into_serializer())
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap()
            .unwrap_b();
        let packet = serialized.parse::<Ipv6Packet<_>>().unwrap();
        assert!(packet.fragment_header_present());
        assert_eq!(packet.proto(), Ipv6Proto::Proto(IpProto::Tcp));
        let fragment_data = packet
            .extension_hdrs
            .into_iter()
            .find_map(|ext_hdr| match ext_hdr {
                Ipv6ExtensionHeader::Fragment { fragment_data } => Some(fragment_data),
                _ => None,
            })
            .unwrap();
        assert_eq!(fragment_data.fragment_offset(), fragment_offset);
        assert_eq!(fragment_data.identification(), identification);
        assert_eq!(fragment_data.m_flag(), more_fragments);
    }

    // Tests that the PacketBuilder implementations correct the maximum body
    // length in PacketConstraints to remove any extension header bytes used.
    #[test]
    fn extension_headers_take_from_max_body_size() {
        let builder = new_builder();
        assert_eq!(builder.constraints().max_body_len(), IPV6_MAX_PAYLOAD_LENGTH);
        let builder =
            Ipv6PacketBuilderWithFragmentHeader::new(builder, FragmentOffset::ZERO, false, 1234);
        assert_eq!(
            builder.constraints().max_body_len(),
            IPV6_MAX_PAYLOAD_LENGTH - IPV6_FRAGMENT_EXT_HDR_LEN
        );
    }

    #[test]
    fn test_partial_serialize_parsed() {
        const PACKET_BYTES: &[u8] = &[
            100, 177, 4, 5, 0, 10, 6, 64, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16,
            17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 0, 1, 2, 3, 4, 5, 6, 7,
            8, 9,
        ];
        const PACKET_LEN: usize = PACKET_BYTES.len();
        let mut packet_bytes_copy = Vec::from(PACKET_BYTES);
        let mut packet_bytes_ref: &mut [u8] = &mut packet_bytes_copy[..];
        let packet = packet_bytes_ref.parse::<Ipv6Packet<_>>().unwrap();

        let buf = assert_matches!(
            packet.partial_serialize(&mut NoOpSerializationContext, packet::new_buf_vec),
            Ok(PartialSerializeResult::NewBuffer { buffer, total_size: PACKET_LEN }) => buffer
        );
        assert_eq!(buf.as_ref(), PACKET_BYTES);
    }
}
