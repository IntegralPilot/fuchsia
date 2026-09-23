// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Parsing and serialization of IPv6 extension headers.
//!
//! The IPv6 extension header format is defined in [RFC 8200 Section 4].
//!
//! [RFC 8200 Section 4]: https://datatracker.ietf.org/doc/html/rfc8200#section-4

use core::convert::Infallible as Never;
use core::marker::PhantomData;

use byteorder::{ByteOrder, NetworkEndian};
use packet::records::options::{
    AlignedOptionBuilder, LengthEncoding, OptionBuilder, OptionLayout, OptionParseErr,
    OptionParseLayout,
};
use packet::records::{
    ParsedRecord, RecordParseResult, Records, RecordsContext, RecordsImpl, RecordsImplLayout,
    RecordsRawImpl,
};
use packet::{BufferView, BufferViewMut};
use zerocopy::byteorder::network_endian::U16;

use crate::ip::{FragmentOffset, IpProto, Ipv6ExtHdrType, Ipv6Proto};

use crate::ipv6::{IPV6_FIXED_HDR_LEN, NEXT_HEADER_OFFSET};

/// The length of an IPv6 Fragment Extension Header.
pub const IPV6_FRAGMENT_EXT_HDR_LEN: usize = 8;

/// An IPv6 Extension Header.
#[allow(missing_docs)]
#[derive(Debug)]
pub enum Ipv6ExtensionHeader<'a> {
    HopByHopOptions { options: HopByHopOptionsData<'a> },
    Routing { routing_data: RoutingData<'a> },
    Fragment { fragment_data: FragmentData },
    DestinationOptions { options: DestinationOptionsData<'a> },
}

//
// Records parsing for IPv6 Extension Header
//

/// Possible errors that can happen when parsing IPv6 Extension Headers.
#[allow(missing_docs)]
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Ipv6ExtensionHeaderParsingError {
    // `pointer` is the offset from the beginning of the first extension header
    // to the point of error. `must_send_icmp` is a flag that requires us to send
    // an ICMP response if true. `header_len` is the size of extension headers before
    // encountering an error (number of bytes from successfully parsed
    // extension headers).
    ErroneousHeaderField { pointer: u32, must_send_icmp: bool },
    UnrecognizedNextHeader { pointer: u32, must_send_icmp: bool },
    UnrecognizedOption { pointer: u32, must_send_icmp: bool, action: ExtensionHeaderOptionAction },
    BufferExhausted,
    MalformedData,
}

impl From<Never> for Ipv6ExtensionHeaderParsingError {
    fn from(err: Never) -> Ipv6ExtensionHeaderParsingError {
        match err {}
    }
}

/// Context that gets passed around when parsing IPv6 Extension Headers.
#[derive(Debug, Clone)]
pub(super) struct Ipv6ExtensionHeaderParsingContext {
    // Next expected header.
    // Marked as `pub(super)` because it is inly used in tests within
    // the `crate::ipv6` (`super`) module.
    pub(super) next_header: u8,

    // Whether context is being used for iteration or not.
    iter: bool,

    // Counter for number of extension headers parsed.
    headers_parsed: usize,

    // Current position relative to the start of the packet.
    pub(super) position: usize,

    // Offset of the current `next_header` value relative to the start of the packet.
    pub(super) next_header_offset: usize,
}

impl Ipv6ExtensionHeaderParsingContext {
    /// Returns a new `Ipv6ExtensionHeaderParsingContext` which expects the
    /// first header to have the ID specified by `next_header`.
    pub(super) fn new(next_header: u8) -> Ipv6ExtensionHeaderParsingContext {
        Ipv6ExtensionHeaderParsingContext {
            iter: false,
            headers_parsed: 0,
            next_header,
            next_header_offset: NEXT_HEADER_OFFSET.into(),
            position: IPV6_FIXED_HDR_LEN,
        }
    }
}

impl RecordsContext for Ipv6ExtensionHeaderParsingContext {
    fn clone_for_iter(&self) -> Self {
        let mut ret = self.clone();
        ret.iter = true;
        ret
    }
}

/// Implement the actual parsing of IPv6 Extension Headers.
#[derive(Debug)]
pub(super) struct Ipv6ExtensionHeaderImpl;

impl Ipv6ExtensionHeaderImpl {
    /// Parse the first two bytes containing `next_header` and header length.
    ///
    /// Takes the first two bytes from `data` and treats them as the `next_header`
    /// and `hdr_ext_len` fields. Updates `next_header` in `context` and then
    /// returns `hdr_ext_len`.
    fn parse_next_hdr_and_len<'a, BV: BufferView<&'a [u8]>>(
        data: &mut BV,
        context: &mut Ipv6ExtensionHeaderParsingContext,
    ) -> Result<u8, Ipv6ExtensionHeaderParsingError> {
        let next_header =
            data.take_byte_front().ok_or(Ipv6ExtensionHeaderParsingError::BufferExhausted)?;
        let hdr_ext_len =
            data.take_byte_front().ok_or(Ipv6ExtensionHeaderParsingError::BufferExhausted)?;

        context.next_header = next_header;
        context.next_header_offset = context.position;
        context.position += 2;

        Ok(hdr_ext_len)
    }

    /// Parse Hop By Hop Options Extension Header.
    // TODO(ghanan): Look into implementing the IPv6 Jumbo Payload option
    //               (https://tools.ietf.org/html/rfc2675) and the router
    //               alert option (https://tools.ietf.org/html/rfc2711).
    fn parse_hop_by_hop_options<'a, BV: BufferView<&'a [u8]>>(
        data: &mut BV,
        context: &mut Ipv6ExtensionHeaderParsingContext,
    ) -> Result<ParsedRecord<Ipv6ExtensionHeader<'a>>, Ipv6ExtensionHeaderParsingError> {
        let hdr_ext_len = Self::parse_next_hdr_and_len(data, context)?;

        // As per RFC 8200 section 4.3, Hdr Ext Len is the length of this extension
        // header in  8-octect units, not including the first 8 octets (where 2 of
        // them are the Next Header and the Hdr Ext Len fields). Since we already
        // 'took' the Next Header and Hdr Ext Len octets, we need to make sure
        // we have (Hdr Ext Len) * 8 + 6 bytes bytes in `data`.
        let expected_len = (hdr_ext_len as usize) * 8 + 6;

        let options = data
            .take_front(expected_len)
            .ok_or(Ipv6ExtensionHeaderParsingError::BufferExhausted)?;

        let options_context = ExtensionHeaderOptionContext::new(context.position);
        let options = Records::parse_with_context(options, options_context)
            .map_err(ext_hdr_opt_err_to_ext_hdr_err)?;
        let options = HopByHopOptionsData::new(options);

        // Update context
        context.position += expected_len;
        context.headers_parsed += 1;

        Ok(ParsedRecord::Parsed(Ipv6ExtensionHeader::HopByHopOptions { options }))
    }

    /// Parse Routing Extension Header.
    fn parse_routing<'a, BV: BufferView<&'a [u8]>>(
        data: &mut BV,
        context: &mut Ipv6ExtensionHeaderParsingContext,
    ) -> Result<ParsedRecord<Ipv6ExtensionHeader<'a>>, Ipv6ExtensionHeaderParsingError> {
        let hdr_ext_len = Self::parse_next_hdr_and_len(data, context)?;

        // As per RFC 8200 section 4.4, Hdr Ext Len is the length of this extension
        // header in  8-octect units, not including the first 8 octets (where 2 of
        // them are the Next Header and the Hdr Ext Len fields). Since we already
        // 'took' the Next Header and Hdr Ext Len octets, we need to make sure
        // we have (Hdr Ext Len) * 8 + 6 bytes bytes in `data`.
        let expected_len = (hdr_ext_len as usize) * 8 + 6;
        let bytes = data
            .take_front(expected_len)
            .ok_or(Ipv6ExtensionHeaderParsingError::BufferExhausted)?;
        let routing_data = RoutingData { bytes };

        let segments_left = routing_data.segments_left();

        // Currently we do not support any routing type.
        //
        // Note, this includes routing type 0 which is defined in RFC 2460 as it has been
        // deprecated as of RFC 5095 for security reasons.

        // If we receive a routing header with an unrecognized routing type,
        // what we do depends on the segments left. If segments left is 0, we
        // must ignore the routing header and continue processing other headers
        // (note that we still return a record here to support operations that
        // depend on the packet structure; any consumers are expected to ignore
        // it). If segments left is not 0, we need to discard this packet and
        // send an ICMP Parameter Problem, Code 0 with a pointer to this
        // unrecognized routing type.
        if segments_left == 0 {
            // Update context
            context.position += expected_len;
            context.headers_parsed += 1;

            Ok(ParsedRecord::Parsed(Ipv6ExtensionHeader::Routing { routing_data }))
        } else {
            // As per RFC 8200, if we encounter a routing header with an unrecognized
            // routing type, and segments left is non-zero, we MUST discard the packet
            // and send and ICMP Parameter Problem response.
            Err(Ipv6ExtensionHeaderParsingError::ErroneousHeaderField {
                pointer: u32::try_from(context.position).unwrap(),
                must_send_icmp: true,
            })
        }
    }

    /// Parse Fragment Extension Header.
    fn parse_fragment<'a, BV: BufferView<&'a [u8]>>(
        data: &mut BV,
        context: &mut Ipv6ExtensionHeaderParsingContext,
    ) -> Result<ParsedRecord<Ipv6ExtensionHeader<'a>>, Ipv6ExtensionHeaderParsingError> {
        // Fragment Extension Header requires exactly 8 bytes so make sure
        // `data` has at least 8 bytes left. If `data` has at least 8 bytes left,
        // we are guaranteed that all `take_front` calls done by this
        // method will succeed since we will never attempt to call `take_front`
        // with more than 8 bytes total.
        if data.len() < 8 {
            return Err(Ipv6ExtensionHeaderParsingError::BufferExhausted);
        }

        // For Fragment headers, we do not actually have a HdrExtLen field. Instead,
        // the second byte in the header (where HdrExtLen would normally exist), is
        // a reserved field, so we can simply ignore it for now.
        let _ = Self::parse_next_hdr_and_len(data, context)?;

        // Update context
        context.position += 6;
        context.headers_parsed += 1;

        Ok(ParsedRecord::Parsed(Ipv6ExtensionHeader::Fragment {
            // First unwrap is safe because we already know data is at least
            // 8 bytes long and we've consumed 2 bytes.
            //
            // Second unwrap is safe because we're converting from a slice
            // of length 6 to an array of length 6.
            fragment_data: FragmentData { bytes: data.take_front(6).unwrap().try_into().unwrap() },
        }))
    }

    /// Parse Destination Options Extension Header.
    fn parse_destination_options<'a, BV: BufferView<&'a [u8]>>(
        data: &mut BV,
        context: &mut Ipv6ExtensionHeaderParsingContext,
    ) -> Result<ParsedRecord<Ipv6ExtensionHeader<'a>>, Ipv6ExtensionHeaderParsingError> {
        let hdr_ext_len = Self::parse_next_hdr_and_len(data, context)?;

        // As per RFC 8200 section 4.6, Hdr Ext Len is the length of this extension
        // header in  8-octet units, not including the first 8 octets (where 2 of
        // them are the Next Header and the Hdr Ext Len fields).
        let expected_len = (hdr_ext_len as usize) * 8 + 6;

        let options = data
            .take_front(expected_len)
            .ok_or(Ipv6ExtensionHeaderParsingError::BufferExhausted)?;

        let options_context = ExtensionHeaderOptionContext::new(context.position);
        let options = Records::parse_with_context(options, options_context)
            .map_err(ext_hdr_opt_err_to_ext_hdr_err)?;
        let options = DestinationOptionsData::new(options);

        // Update context
        context.position += expected_len;
        context.headers_parsed += 1;

        Ok(ParsedRecord::Parsed(Ipv6ExtensionHeader::DestinationOptions { options }))
    }
}

impl RecordsImplLayout for Ipv6ExtensionHeaderImpl {
    type Context = Ipv6ExtensionHeaderParsingContext;
    type Error = Ipv6ExtensionHeaderParsingError;
}

impl RecordsImpl for Ipv6ExtensionHeaderImpl {
    type Record<'a> = Ipv6ExtensionHeader<'a>;

    fn parse_with_context<'a, BV: BufferView<&'a [u8]>>(
        data: &mut BV,
        context: &mut Self::Context,
    ) -> RecordParseResult<Self::Record<'a>, Self::Error> {
        let expected_hdr = context.next_header;

        match Ipv6ExtHdrType::from(expected_hdr) {
            Ipv6ExtHdrType::HopByHopOptions => {
                if context.headers_parsed == 0 {
                    Self::parse_hop_by_hop_options(data, context)
                } else {
                    // Hop-by-hop extension is allowed only immediately after the fixed header.
                    Err(Ipv6ExtensionHeaderParsingError::UnrecognizedNextHeader {
                        pointer: context.next_header_offset as u32,
                        must_send_icmp: false,
                    })
                }
            }
            Ipv6ExtHdrType::Routing => Self::parse_routing(data, context),
            Ipv6ExtHdrType::Fragment => Self::parse_fragment(data, context),
            Ipv6ExtHdrType::DestinationOptions => Self::parse_destination_options(data, context),
            Ipv6ExtHdrType::EncapsulatingSecurityPayload | Ipv6ExtHdrType::Authentication => {
                // We don't implement these extension header types.
                //
                // Per RFC 2460:
                //   If, as a result of processing a header, a node is required to
                //   proceed to the next header but the Next Header value in the
                //   current header is unrecognized by the node, it should discard
                //   the packet and send an ICMP Parameter Problem message to the
                //   source of the packet, with an ICMP Code value of 1
                //   ("unrecognized Next Header type encountered") and the ICMP
                //   Pointer field containing the offset of the unrecognized value
                //   within the original packet.
                Err(Ipv6ExtensionHeaderParsingError::UnrecognizedNextHeader {
                    pointer: context.next_header_offset as u32,
                    // This is false because of the "should" in the quoted RFC
                    // text.
                    must_send_icmp: false,
                })
            }
            Ipv6ExtHdrType::Other(_) if is_valid_next_header_upper_layer(expected_hdr) => {
                // Stop parsing extension headers when we find a Next Header value
                // for a higher level protocol.
                Ok(ParsedRecord::Done)
            }
            Ipv6ExtHdrType::Other(_) => {
                // TODO(https://fxbug.dev/553060282): Ignore unknown options and
                // continue parsing.
                Err(Ipv6ExtensionHeaderParsingError::UnrecognizedNextHeader {
                    pointer: context.next_header_offset as u32,
                    must_send_icmp: false,
                })
            }
        }
    }
}

impl<'a> RecordsRawImpl<'a> for Ipv6ExtensionHeaderImpl {
    fn parse_raw_with_context<BV: BufferView<&'a [u8]>>(
        data: &mut BV,
        context: &mut Self::Context,
    ) -> Result<bool, Self::Error> {
        let (next, skip) = match Ipv6ExtHdrType::from(context.next_header) {
            Ipv6ExtHdrType::HopByHopOptions => {
                if context.headers_parsed == 0 {
                    // take next header and header len, and skip the next 6
                    // octets + the number of 64 bit words in header len.
                    data.take_front(2)
                        .map(|x| (x[0], (x[1] as usize) * 8 + 6))
                        .ok_or(Ipv6ExtensionHeaderParsingError::BufferExhausted)?
                } else {
                    // Hop-by-hop extension is allowed only immediately after the fixed header.
                    return Err(Ipv6ExtensionHeaderParsingError::UnrecognizedNextHeader {
                        pointer: context.next_header_offset as u32,
                        must_send_icmp: false,
                    });
                }
            }

            Ipv6ExtHdrType::Routing | Ipv6ExtHdrType::DestinationOptions => {
                // take next header and header len, and skip the next 6
                // octets + the number of 64 bit words in header len.
                data.take_front(2)
                    .map(|x| (x[0], (x[1] as usize) * 8 + 6))
                    .ok_or(Ipv6ExtensionHeaderParsingError::BufferExhausted)?
            }
            Ipv6ExtHdrType::Fragment => {
                // take next header from first, then skip next 7
                (
                    data.take_byte_front()
                        .ok_or(Ipv6ExtensionHeaderParsingError::BufferExhausted)?,
                    7,
                )
            }
            Ipv6ExtHdrType::EncapsulatingSecurityPayload => {
                // TODO(brunodalbo): We don't support ESP yet, so return
                //  an error instead of panicking "unimplemented" to avoid
                //  having a panic-path that can be remotely triggered.
                return debug_err!(
                    Err(Ipv6ExtensionHeaderParsingError::MalformedData),
                    "ESP extension header not supported"
                );
            }
            Ipv6ExtHdrType::Authentication => {
                // take next header and payload len, and skip the next
                // (payload_len + 2) 32 bit words, minus the 2 octets
                // already consumed.
                data.take_front(2)
                    .map(|x| (x[0], (x[1] as usize + 2) * 4 - 2))
                    .ok_or(Ipv6ExtensionHeaderParsingError::BufferExhausted)?
            }
            Ipv6ExtHdrType::Other(next_header) if is_valid_next_header_upper_layer(next_header) => {
                return Ok(false);
            }

            Ipv6ExtHdrType::Other(_) => {
                return Err(Ipv6ExtensionHeaderParsingError::UnrecognizedNextHeader {
                    pointer: context.next_header_offset as u32,
                    must_send_icmp: false,
                });
            }
        };
        let _: &[u8] =
            data.take_front(skip).ok_or(Ipv6ExtensionHeaderParsingError::BufferExhausted)?;
        context.next_header = next;
        context.next_header_offset = context.position;
        context.position += skip;
        context.headers_parsed += 1;
        Ok(true)
    }
}

//
// Hop-By-Hop Options
//

/// Hop By Hop Options extension header data.
#[derive(Debug)]
pub struct HopByHopOptionsData<'a> {
    options: Records<&'a [u8], HopByHopOptionsImpl>,
}

impl<'a> HopByHopOptionsData<'a> {
    /// Returns a new `HopByHopOptionsData` with `options`.
    fn new(options: Records<&'a [u8], HopByHopOptionsImpl>) -> HopByHopOptionsData<'a> {
        HopByHopOptionsData { options }
    }

    /// Returns an iterator over the [`HopByHopOptions`] in this
    /// `HopByHopOptionsData`.
    pub fn iter(&'a self) -> impl Iterator<Item = HopByHopOption<'a>> {
        self.options.iter()
    }
}

/// An option found in a Hop By Hop Options extension header.
pub type HopByHopOption<'a> = ExtensionHeaderOption<HopByHopOptionData<'a>>;

/// An implementation of [`OptionsImpl`] for options found in a Hop By Hop Options
/// extension header.
pub(super) type HopByHopOptionsImpl = ExtensionHeaderOptionImpl<HopByHopOptionDataImpl>;

/// Hop-By-Hop Option Type number as per [RFC 2711 section-2.1]
///
/// [RFC 2711 section-2.1]: https://tools.ietf.org/html/rfc2711#section-2.1
const HBH_OPTION_KIND_RTRALRT: u8 = 5;

/// Length for RouterAlert as per [RFC 2711 section-2.1]
///
/// [RFC 2711 section-2.1]: https://tools.ietf.org/html/rfc2711#section-2.1
const HBH_OPTION_RTRALRT_LEN: usize = 2;

/// HopByHop Options Extension header data.
#[allow(missing_docs)]
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum HopByHopOptionData<'a> {
    Unrecognized { kind: u8, len: u8, data: &'a [u8] },
    RouterAlert { data: u16 },
}

/// Impl for Hop By Hop Options parsing.
#[derive(Debug)]
pub(super) struct HopByHopOptionDataImpl;

impl ExtensionHeaderOptionDataImplLayout for HopByHopOptionDataImpl {
    type Context = ();
}

impl ExtensionHeaderOptionDataImpl for HopByHopOptionDataImpl {
    type OptionData<'a> = HopByHopOptionData<'a>;

    fn parse_option<'a>(
        kind: u8,
        data: &'a [u8],
        _context: &mut Self::Context,
        allow_unrecognized: bool,
    ) -> ExtensionHeaderOptionDataParseResult<Self::OptionData<'a>> {
        match kind {
            HBH_OPTION_KIND_RTRALRT => {
                if data.len() == HBH_OPTION_RTRALRT_LEN {
                    ExtensionHeaderOptionDataParseResult::Ok(HopByHopOptionData::RouterAlert {
                        data: NetworkEndian::read_u16(data),
                    })
                } else {
                    // Since the length is wrong, and the length is indicated at the second byte within
                    // the option itself. We count from 0 of course.
                    ExtensionHeaderOptionDataParseResult::ErrorAt(1)
                }
            }
            _ => {
                if allow_unrecognized {
                    ExtensionHeaderOptionDataParseResult::Ok(HopByHopOptionData::Unrecognized {
                        kind,
                        len: data.len() as u8,
                        data,
                    })
                } else {
                    ExtensionHeaderOptionDataParseResult::UnrecognizedKind
                }
            }
        }
    }
}

impl OptionLayout for HopByHopOptionsImpl {
    type KindLenField = u8;
    const LENGTH_ENCODING: LengthEncoding = LengthEncoding::ValueOnly;
}

impl OptionParseLayout for HopByHopOptionsImpl {
    type Error = OptionParseErr;
    const END_OF_OPTIONS: Option<u8> = Some(0);
    const NOP: Option<u8> = Some(1);
}

/// Provides an implementation of `OptionLayout` for Hop-by-Hop options.
///
/// Use this instead of `HopByHopOptionsImpl` for `<HopByHopOption as
/// OptionBuilder>::Layout` in order to avoid having to make a ton of other
/// things `pub` which are reachable from `HopByHopOptionsImpl`.
#[doc(hidden)]
pub enum HopByHopOptionLayout {}

impl OptionLayout for HopByHopOptionLayout {
    type KindLenField = u8;
    const LENGTH_ENCODING: LengthEncoding = LengthEncoding::ValueOnly;
}

impl<'a> OptionBuilder for HopByHopOption<'a> {
    type Layout = HopByHopOptionLayout;
    fn serialized_len(&self) -> usize {
        match self.data {
            HopByHopOptionData::RouterAlert { .. } => HBH_OPTION_RTRALRT_LEN,
            HopByHopOptionData::Unrecognized { len, .. } => len as usize,
        }
    }

    fn option_kind(&self) -> u8 {
        let action: u8 = self.action.into();
        let mutable = self.mutable as u8;
        let type_number = match self.data {
            HopByHopOptionData::Unrecognized { kind, .. } => kind,
            HopByHopOptionData::RouterAlert { .. } => HBH_OPTION_KIND_RTRALRT,
        };
        (action << 6) | (mutable << 5) | type_number
    }

    fn serialize_into(&self, mut buffer: &mut [u8]) {
        match self.data {
            HopByHopOptionData::Unrecognized { data, .. } => buffer.copy_from_slice(data),
            HopByHopOptionData::RouterAlert { data } => {
                // If the buffer doesn't contain enough space, it is a
                // contract violation, panic here.
                (&mut buffer).write_obj_front(&U16::new(data)).unwrap()
            }
        }
    }
}

impl<'a> AlignedOptionBuilder for HopByHopOption<'a> {
    fn alignment_requirement(&self) -> (usize, usize) {
        match self.data {
            // RouterAlert must be aligned at 2 * n + 0 bytes.
            // See: https://tools.ietf.org/html/rfc2711#section-2.1
            HopByHopOptionData::RouterAlert { .. } => (2, 0),
            _ => (1, 0),
        }
    }

    fn serialize_padding(buf: &mut [u8], length: usize) {
        assert!(length <= buf.len());
        assert!(length <= (u8::MAX as usize) + 2);

        #[allow(clippy::comparison_chain)]
        if length == 1 {
            // Use Pad1
            buf[0] = 0
        } else if length > 1 {
            // Use PadN
            buf[0] = 1;
            buf[1] = (length - 2) as u8;
            #[allow(clippy::needless_range_loop)]
            for i in 2..length {
                buf[i] = 0
            }
        }
    }
}

//
// Routing
//

/// Routing Extension header data.
///
/// As per RFC 8200, section 4.4 the Routing header is structured as:
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// |  Next Header  |  Hdr Ext Len  |  Routing Type | Segments Left |
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// |                                                               |
/// .                                                               .
/// .                       type-specific data                      .
/// .                                                               .
/// |                                                               |
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
///
/// where the format of the type-specific data is determined by the Routing
/// Type.
#[derive(Debug)]
pub struct RoutingData<'a> {
    bytes: &'a [u8],
}

/// Supported Routing Types.
#[derive(Debug, PartialEq, Eq)]
pub enum RoutingType {}

/// Error returned when the routing type failed to parse.
#[derive(Debug, PartialEq, Eq)]
pub enum RoutingTypeParseError {
    /// The Routing header has an unknown routing type and must be ignored per
    /// RFC 8200 section 4.4.
    UnsupportedType(u8),
}

impl TryFrom<u8> for RoutingType {
    type Error = RoutingTypeParseError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Err(RoutingTypeParseError::UnsupportedType(value))
    }
}

impl<'a> RoutingData<'a> {
    /// Returns the routing type.
    pub fn routing_type(&self) -> Result<RoutingType, RoutingTypeParseError> {
        debug_assert!(self.bytes.len() >= 6);
        RoutingType::try_from(self.bytes[0])
    }

    /// Returns the number of segments left.
    pub fn segments_left(&self) -> u8 {
        debug_assert!(self.bytes.len() >= 6);
        self.bytes[1]
    }
}

//
// Fragment
//

/// Fragment Extension header data.
///
/// As per RFC 8200, section 4.5 the fragment header is structured as:
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// |  Next Header  |   Reserved    |      Fragment Offset    |Res|M|
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// |                         Identification                        |
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
///
/// where Fragment Offset is 13 bits, Res is a reserved 2 bits and M
/// is a 1 bit flag. Identification is a 32bit value.
#[derive(Debug, Copy, Clone)]
pub struct FragmentData {
    bytes: [u8; 6],
}

impl FragmentData {
    /// Returns the fragment offset.
    pub fn fragment_offset(&self) -> FragmentOffset {
        FragmentOffset::new_with_msb(U16::from_bytes([self.bytes[0], self.bytes[1]]).get())
    }

    /// Returns the more fragments flags.
    pub fn m_flag(&self) -> bool {
        (self.bytes[1] & 0x1) == 0x01
    }

    /// Returns the identification value.
    pub fn identification(&self) -> u32 {
        NetworkEndian::read_u32(&self.bytes[2..6])
    }
}

//
// Destination Options
//

/// Destination Options extension header data.
#[derive(Debug)]
pub struct DestinationOptionsData<'a> {
    options: Records<&'a [u8], DestinationOptionsImpl>,
}

impl<'a> DestinationOptionsData<'a> {
    /// Returns a new `DestinationOptionsData` with `options`.
    fn new(options: Records<&'a [u8], DestinationOptionsImpl>) -> DestinationOptionsData<'a> {
        DestinationOptionsData { options }
    }

    /// Returns an iterator over the [`DestinationOptions`] in this
    /// `DestinationOptionsData`.
    pub fn iter(&'a self) -> impl Iterator<Item = DestinationOption<'a>> {
        self.options.iter()
    }
}

/// An option found in a Destination Options extension header.
pub type DestinationOption<'a> = ExtensionHeaderOption<DestinationOptionData<'a>>;

/// An implementation of [`OptionsImpl`] for options found in a Destination Options
/// extension header.
pub(super) type DestinationOptionsImpl = ExtensionHeaderOptionImpl<DestinationOptionDataImpl>;

/// Destination Options extension header data.
#[allow(missing_docs)]
#[derive(Debug)]
pub enum DestinationOptionData<'a> {
    Unrecognized { kind: u8, len: u8, data: &'a [u8] },
}

/// Impl for Destination Options parsing.
#[derive(Debug)]
pub(super) struct DestinationOptionDataImpl;

impl ExtensionHeaderOptionDataImplLayout for DestinationOptionDataImpl {
    type Context = ();
}

impl ExtensionHeaderOptionDataImpl for DestinationOptionDataImpl {
    type OptionData<'a> = DestinationOptionData<'a>;

    fn parse_option<'a>(
        kind: u8,
        data: &'a [u8],
        _context: &mut Self::Context,
        allow_unrecognized: bool,
    ) -> ExtensionHeaderOptionDataParseResult<Self::OptionData<'a>> {
        if allow_unrecognized {
            ExtensionHeaderOptionDataParseResult::Ok(DestinationOptionData::Unrecognized {
                kind,
                len: data.len() as u8,
                data,
            })
        } else {
            ExtensionHeaderOptionDataParseResult::UnrecognizedKind
        }
    }
}

//
// Generic Extension Header who's data are options.
//

/// Context that gets passed around when parsing IPv6 Extension Header options.
#[derive(Debug, Clone)]
pub(super) struct ExtensionHeaderOptionContext<C: Sized + Clone> {
    // Counter for number of options parsed.
    options_parsed: usize,

    // Current position relative to the start of the packet.
    position: usize,

    // Extension header specific context data.
    specific_context: C,
}

impl<C: Sized + Clone + Default> ExtensionHeaderOptionContext<C> {
    fn new(offset: usize) -> Self {
        ExtensionHeaderOptionContext {
            options_parsed: 0,
            position: offset,
            specific_context: C::default(),
        }
    }
}

impl<C: Sized + Clone> RecordsContext for ExtensionHeaderOptionContext<C> {}

/// Basic associated types required by `ExtensionHeaderOptionDataImpl`.
pub(super) trait ExtensionHeaderOptionDataImplLayout {
    /// A context type that can be used to maintain state while parsing multiple
    /// records.
    type Context: RecordsContext;
}

/// The result of parsing an extension header option data.
#[derive(PartialEq, Eq, Debug)]
pub enum ExtensionHeaderOptionDataParseResult<D> {
    /// Successfully parsed data.
    Ok(D),

    /// An error occurred at the indicated offset within the option.
    ///
    /// For example, if the data length goes wrong, you should probably
    /// make the offset to be 1 because in most (almost all) cases, the
    /// length is at the second byte of the option.
    ErrorAt(u32),

    /// The option kind is not recognized.
    UnrecognizedKind,
}

/// An implementation of an extension header specific option data parser.
pub(super) trait ExtensionHeaderOptionDataImpl: ExtensionHeaderOptionDataImplLayout {
    /// Extension header specific option data.
    ///
    /// Note, `OptionData` does not need to hold general option data as defined by
    /// RFC 8200 section 4.2. It should only hold extension header specific option
    /// data.
    type OptionData<'a>: Sized;

    /// Parse an option of a given `kind` from `data`.
    ///
    /// When `kind` is recognized returns `Ok(o)` where `o` is a successfully parsed
    /// option. When `kind` is not recognized, returns `UnrecognizedKind` if `allow_unrecognized`
    /// is `false`. If `kind` is not recognized but `allow_unrecognized` is `true`,
    /// returns an `Ok(o)` where `o` holds option data without actually parsing it
    /// (i.e. an unrecognized type that simply keeps track of the `kind` and `data`
    /// that was passed to `parse_option`). A recognized option `kind` with incorrect
    /// `data` must return `ErrorAt(offset)`, where the offset indicates where the
    /// erroneous field is within the option data buffer.
    fn parse_option<'a>(
        kind: u8,
        data: &'a [u8],
        context: &mut Self::Context,
        allow_unrecognized: bool,
    ) -> ExtensionHeaderOptionDataParseResult<Self::OptionData<'a>>;
}

/// Generic implementation of extension header options parsing.
///
/// `ExtensionHeaderOptionImpl` handles the common implementation details
/// of extension header options and lets `O` (which implements
/// `ExtensionHeaderOptionDataImpl`) handle the extension header specific
/// option parsing.
#[derive(Debug)]
pub(super) struct ExtensionHeaderOptionImpl<O>(PhantomData<O>);

impl<O> ExtensionHeaderOptionImpl<O> {
    const PAD1: u8 = 0;
    const PADN: u8 = 1;
}

impl<O> RecordsImplLayout for ExtensionHeaderOptionImpl<O>
where
    O: ExtensionHeaderOptionDataImplLayout,
{
    type Error = ExtensionHeaderOptionParsingError;
    type Context = ExtensionHeaderOptionContext<O::Context>;
}

impl<O> RecordsImpl for ExtensionHeaderOptionImpl<O>
where
    O: ExtensionHeaderOptionDataImpl,
{
    type Record<'a> = ExtensionHeaderOption<O::OptionData<'a>>;

    fn parse_with_context<'a, BV: BufferView<&'a [u8]>>(
        data: &mut BV,
        context: &mut Self::Context,
    ) -> RecordParseResult<Self::Record<'a>, Self::Error> {
        // If we have no more bytes left, we are done.
        let kind = match data.take_byte_front() {
            None => return Ok(ParsedRecord::Done),
            Some(k) => k,
        };

        // Will never get an error because we only use the 2 least significant bits which
        // can only have a max value of 3 and all values in [0, 3] are valid values of
        // `ExtensionHeaderOptionAction`.
        let action =
            ExtensionHeaderOptionAction::try_from((kind >> 6) & 0x3).expect("Unexpected error");
        let mutable = ((kind >> 5) & 0x1) == 0x1;
        // Note that `kind` remains unmodified here: per RFC 8200 section 4.2,
        // the three high-order bits parsed above are to be treated as part of
        // the Option Type.

        // If our kind is a PAD1, consider it a NOP.
        if kind == Self::PAD1 {
            // Update context.
            context.options_parsed += 1;
            context.position += 1;

            return Ok(ParsedRecord::Skipped);
        }

        let len =
            data.take_byte_front().ok_or(ExtensionHeaderOptionParsingError::BufferExhausted)?;

        let data = data
            .take_front(len as usize)
            .ok_or(ExtensionHeaderOptionParsingError::BufferExhausted)?;

        // If our kind is a PADN, consider it a NOP as well.
        if kind == Self::PADN {
            // Update context.
            context.options_parsed += 1;
            context.position += 2 + (len as usize);

            return Ok(ParsedRecord::Skipped);
        }

        // Parse the actual option data.
        match O::parse_option(
            kind,
            data,
            &mut context.specific_context,
            action == ExtensionHeaderOptionAction::SkipAndContinue,
        ) {
            ExtensionHeaderOptionDataParseResult::Ok(o) => {
                // Update context.
                context.options_parsed += 1;
                context.position += 2 + (len as usize);

                Ok(ParsedRecord::Parsed(ExtensionHeaderOption { action, mutable, data: o }))
            }
            ExtensionHeaderOptionDataParseResult::ErrorAt(offset) => {
                // The precondition here is that `position + offset` must point inside the
                // packet. So as reasoned in the next match arm, it is not possible to exceed
                // `core::u32::max`. Given this reasoning, we know the call to `unwrap` should not
                // panic.
                Err(ExtensionHeaderOptionParsingError::ErroneousOptionField {
                    pointer: u32::try_from(context.position + offset as usize).unwrap(),
                })
            }
            ExtensionHeaderOptionDataParseResult::UnrecognizedKind => {
                // Unrecognized option type.
                match action {
                    // `O::parse_option` should never return
                    // `ExtensionHeaderOptionDataParseResult::UnrecognizedKind` when the
                    // action is `ExtensionHeaderOptionAction::SkipAndContinue` because
                    // we expect `O::parse_option` to return something that holds the
                    // option data without actually parsing it since we pass `true` for its
                    // `allow_unrecognized` parameter.
                    ExtensionHeaderOptionAction::SkipAndContinue => unreachable!(
                        "Should never end up here since action was set to skip and continue"
                    ),
                    // We know the below `try_from` call will not result in a `None` value because
                    // the maximum size of an IPv6 packet's payload (extension headers + body) is
                    // `core::u32::MAX`. This maximum size is only possible when using IPv6
                    // jumbograms as defined by RFC 2675, which uses a 32 bit field for the payload
                    // length. If we receive such a hypothetical packet with the maximum possible
                    // payload length which only contains extension headers, we know that the offset
                    // of any location within the payload must fit within an `u32`. If the packet is
                    // a normal IPv6 packet (not a jumbogram), the maximum size of the payload is
                    // `core::u16::MAX` (as the normal payload length field is only 16 bits), which
                    // is significantly less than the maximum possible size of a jumbogram.
                    _ => Err(ExtensionHeaderOptionParsingError::UnrecognizedOption {
                        pointer: u32::try_from(context.position).unwrap(),
                        action,
                    }),
                }
            }
        }
    }
}

/// Possible errors when parsing extension header options.
#[allow(missing_docs)]
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ExtensionHeaderOptionParsingError {
    ErroneousOptionField { pointer: u32 },
    UnrecognizedOption { pointer: u32, action: ExtensionHeaderOptionAction },
    BufferExhausted,
}

impl From<Never> for ExtensionHeaderOptionParsingError {
    fn from(err: Never) -> ExtensionHeaderOptionParsingError {
        match err {}
    }
}

/// Action to take when an unrecognized option type is encountered.
///
/// `ExtensionHeaderOptionAction` is an action that MUST be taken (according
/// to RFC 8200 section 4.2) when an IPv6 processing node does not
/// recognize an option's type.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ExtensionHeaderOptionAction {
    /// Skip over the option and continue processing the header.
    /// value = 0.
    SkipAndContinue,

    /// Just discard the packet.
    /// value = 1.
    DiscardPacket,

    /// Discard the packet and, regardless of whether or not the packet's
    /// destination address was a multicast address, send an ICMP parameter
    /// problem, code 2 (unrecognized option), message to the packet's source
    /// address, pointing to the unrecognized type.
    /// value = 2.
    DiscardPacketSendIcmp,

    /// Discard the packet and, and only if the packet's destination address
    /// was not a multicast address, send an ICMP parameter problem, code 2
    /// (unrecognized option), message to the packet's source address, pointing
    /// to the unrecognized type.
    /// value = 3.
    DiscardPacketSendIcmpNoMulticast,
}

impl TryFrom<u8> for ExtensionHeaderOptionAction {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, ()> {
        match value {
            0 => Ok(ExtensionHeaderOptionAction::SkipAndContinue),
            1 => Ok(ExtensionHeaderOptionAction::DiscardPacket),
            2 => Ok(ExtensionHeaderOptionAction::DiscardPacketSendIcmp),
            3 => Ok(ExtensionHeaderOptionAction::DiscardPacketSendIcmpNoMulticast),
            _ => Err(()),
        }
    }
}

impl From<ExtensionHeaderOptionAction> for u8 {
    fn from(a: ExtensionHeaderOptionAction) -> u8 {
        match a {
            ExtensionHeaderOptionAction::SkipAndContinue => 0,
            ExtensionHeaderOptionAction::DiscardPacket => 1,
            ExtensionHeaderOptionAction::DiscardPacketSendIcmp => 2,
            ExtensionHeaderOptionAction::DiscardPacketSendIcmpNoMulticast => 3,
        }
    }
}

/// Extension header option.
///
/// Generic Extension header option type that has extension header specific
/// option data (`data`) defined by an `O`. The common option format is defined in
/// section 4.2 of RFC 8200, outlining actions and mutability for option types.
#[derive(PartialEq, Eq, Debug, Clone)]
pub struct ExtensionHeaderOption<O> {
    /// Action to take if the option type is unrecognized.
    pub action: ExtensionHeaderOptionAction,

    /// Whether or not the option data of the option can change en route to the
    /// packet's final destination. When an Authentication header is present in
    /// the packet, the option data must be treated as 0s when computing or
    /// verifying the packet's authenticating value when the option data can change
    /// en route.
    pub mutable: bool,

    /// Option data associated with a specific extension header.
    pub data: O,
}

//
// Helper functions
//

/// Make sure a Next Header is a valid upper layer protocol.
///
/// Make sure a Next Header is a valid upper layer protocol in an IPv6 packet. Note,
/// we intentionally are not allowing ICMP(v4) since we are working on IPv6 packets.
pub(super) fn is_valid_next_header_upper_layer(next_header: u8) -> bool {
    match Ipv6Proto::from(next_header) {
        Ipv6Proto::Proto(IpProto::Tcp)
        | Ipv6Proto::Proto(IpProto::Udp)
        | Ipv6Proto::Icmpv6
        | Ipv6Proto::NoNextHeader => true,
        Ipv6Proto::Proto(IpProto::Reserved) | Ipv6Proto::Other(_) => false,
    }
}

/// Convert an `ExtensionHeaderOptionParsingError` to an
/// `Ipv6ExtensionHeaderParsingError`.
///
/// `offset` is the offset of the start of the options containing the error, `err`,
/// from the end of the fixed header in an IPv6 packet.
fn ext_hdr_opt_err_to_ext_hdr_err(
    err: ExtensionHeaderOptionParsingError,
) -> Ipv6ExtensionHeaderParsingError {
    match err {
        ExtensionHeaderOptionParsingError::ErroneousOptionField { pointer } => {
            Ipv6ExtensionHeaderParsingError::ErroneousHeaderField {
                pointer: pointer,
                // TODO: RFC only suggests we SHOULD generate an ICMP message,
                // and ideally, we should generate ICMP messages only when the problem
                // is severe enough, we do not want to flood the network. So we
                // should investigate the criteria for this field to become true.
                must_send_icmp: false,
            }
        }
        ExtensionHeaderOptionParsingError::UnrecognizedOption { pointer, action } => {
            Ipv6ExtensionHeaderParsingError::UnrecognizedOption {
                pointer: pointer,
                must_send_icmp: true,
                action,
            }
        }
        ExtensionHeaderOptionParsingError::BufferExhausted => {
            Ipv6ExtensionHeaderParsingError::BufferExhausted
        }
    }
}

#[cfg(test)]
mod tests {
    use packet::records::{AlignedRecordSequenceBuilder, RecordBuilder};

    use crate::ip::Ipv4Proto;
    use crate::ipv6::IPV6_FIXED_HDR_LEN;

    use super::*;

    #[test]
    fn test_is_valid_next_header_upper_layer() {
        // Make sure upper layer protocols like TCP are valid
        assert!(is_valid_next_header_upper_layer(IpProto::Tcp.into()));
        assert!(is_valid_next_header_upper_layer(IpProto::Tcp.into()));

        // Make sure upper layer protocol ICMP(v4) is not valid
        assert!(!is_valid_next_header_upper_layer(Ipv4Proto::Icmp.into()));
        assert!(!is_valid_next_header_upper_layer(Ipv4Proto::Icmp.into()));
    }

    #[test]
    fn test_hop_by_hop_options() {
        // Test parsing of Pad1 (marked as NOP)
        let buffer = [0; 10];
        let mut context = ExtensionHeaderOptionContext::new(10);
        let options =
            Records::<_, HopByHopOptionsImpl>::parse_with_mut_context(&buffer[..], &mut context)
                .unwrap();
        assert_eq!(options.iter().count(), 0);
        assert_eq!(context.position, 20);
        assert_eq!(context.options_parsed, 10);

        // Test parsing of Pad1 w/ PadN (treated as NOP)
        #[rustfmt::skip]
        let buffer = [
            0,                            // Pad1
            1, 0,                         // Pad2
            1, 8, 0, 0, 0, 0, 0, 0, 0, 0, // Pad10
        ];
        let mut context = ExtensionHeaderOptionContext::new(1);
        let options =
            Records::<_, HopByHopOptionsImpl>::parse_with_mut_context(&buffer[..], &mut context)
                .unwrap();
        assert_eq!(options.iter().count(), 0);
        assert_eq!(context.position, 14);
        assert_eq!(context.options_parsed, 3);

        // Test parsing with an unknown option type but its action is
        // skip/continue
        #[rustfmt::skip]
        let buffer = [
            0,                            // Pad1
            63, 1, 0,                     // Unrecognized Option Type but can skip/continue
            1,  6, 0, 0, 0, 0, 0, 0,      // Pad8
        ];
        let mut context = ExtensionHeaderOptionContext::new(1);
        let options =
            Records::<_, HopByHopOptionsImpl>::parse_with_mut_context(&buffer[..], &mut context)
                .unwrap();
        let options: Vec<HopByHopOption<'_>> = options.iter().collect();
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].action, ExtensionHeaderOptionAction::SkipAndContinue);
        assert_eq!(context.position, 13);
        assert_eq!(context.options_parsed, 3);
    }

    #[test]
    fn test_hop_by_hop_options_err() {
        // Test parsing but missing last 2 bytes
        #[rustfmt::skip]
        let buffer = [
            0,                            // Pad1
            1, 0,                         // Pad2
            1, 8, 0, 0, 0, 0, 0, 0,       // Pad10 (but missing 2 bytes)
        ];
        let mut context = ExtensionHeaderOptionContext::new(5);
        assert_eq!(
            Records::<_, HopByHopOptionsImpl>::parse_with_mut_context(&buffer[..], &mut context)
                .expect_err("Parsed successfully when we were short 2 bytes"),
            ExtensionHeaderOptionParsingError::BufferExhausted
        );
        assert_eq!(context.position, 8);
        assert_eq!(context.options_parsed, 2);

        // Test parsing with unknown option type but action set to discard
        #[rustfmt::skip]
        let buffer = [
            1,   1, 0,                    // Pad3
            127, 0,                       // Unrecognized Option Type w/ action to discard
            1,   6, 0, 0, 0, 0, 0, 0,     // Pad8
        ];
        let mut context = ExtensionHeaderOptionContext::new(5);
        assert_eq!(
            Records::<_, HopByHopOptionsImpl>::parse_with_mut_context(&buffer[..], &mut context)
                .expect_err("Parsed successfully when we had an unrecognized option type"),
            ExtensionHeaderOptionParsingError::UnrecognizedOption {
                pointer: 8,
                action: ExtensionHeaderOptionAction::DiscardPacket,
            }
        );
        assert_eq!(context.position, 8);
        assert_eq!(context.options_parsed, 1);

        // Test parsing with unknown option type but action set to discard and
        // send ICMP.
        #[rustfmt::skip]
        let buffer = [
            1,   1, 0,                    // Pad3
            191, 0,                       // Unrecognized Option Type w/ action to discard
                                          // & send icmp
            1,   6, 0, 0, 0, 0, 0, 0,     // Pad8
        ];
        let mut context = ExtensionHeaderOptionContext::new(5);
        assert_eq!(
            Records::<_, HopByHopOptionsImpl>::parse_with_mut_context(&buffer[..], &mut context)
                .expect_err("Parsed successfully when we had an unrecognized option type"),
            ExtensionHeaderOptionParsingError::UnrecognizedOption {
                pointer: 8,
                action: ExtensionHeaderOptionAction::DiscardPacketSendIcmp,
            }
        );
        assert_eq!(context.position, 8);
        assert_eq!(context.options_parsed, 1);

        // Test parsing with unknown option type but action set to discard and
        // send ICMP if not sending to a multicast address
        #[rustfmt::skip]
        let buffer = [
            1,   1, 0,                    // Pad3
            255, 0,                       // Unrecognized Option Type w/ action to discard
                                          // & send icmp if no multicast
            1,   6, 0, 0, 0, 0, 0, 0,     // Pad8
        ];
        let mut context = ExtensionHeaderOptionContext::new(5);
        assert_eq!(
            Records::<_, HopByHopOptionsImpl>::parse_with_mut_context(&buffer[..], &mut context)
                .expect_err("Parsed successfully when we had an unrecognized option type"),
            ExtensionHeaderOptionParsingError::UnrecognizedOption {
                pointer: 8,
                action: ExtensionHeaderOptionAction::DiscardPacketSendIcmpNoMulticast,
            }
        );
        assert_eq!(context.position, 8);
        assert_eq!(context.options_parsed, 1);

        // Test parsing Pad1 but with upper bits set.
        #[rustfmt::skip]
        let buffer = [
            // 0b11000000 -> action = 0b11, mutable = 0b0, option type = 0b00000
            // (matching lower-order bits of Pad1).
            0xC0,
            1, 0,                         // Pad2
            1, 0, 0, 0, 0, 0, 0, 0, 0, 0, // Pad10
        ];
        let mut context = ExtensionHeaderOptionContext::new(5);
        assert_eq!(
            Records::<_, HopByHopOptionsImpl>::parse_with_mut_context(&buffer[..], &mut context)
                .expect_err("Parsed successfully when we had Pad1 with upper bits set"),
            ExtensionHeaderOptionParsingError::UnrecognizedOption {
                pointer: 5,
                action: ExtensionHeaderOptionAction::DiscardPacketSendIcmpNoMulticast,
            }
        );
        assert_eq!(context.position, 5);
        assert_eq!(context.options_parsed, 0);

        // Test parsing Pad2 but with upper bits set.
        #[rustfmt::skip]
        let buffer = [
            0,                            // Pad1
            // 0b11000001 -> action = 0b11, mutable = 0b0, option type = 0b00001
            // (matching lower-order bits of Pad2).
            0xC1, 0,
            1, 0, 0, 0, 0, 0, 0, 0, 0, 0, // Pad10
        ];
        let mut context = ExtensionHeaderOptionContext::new(5);
        assert_eq!(
            Records::<_, HopByHopOptionsImpl>::parse_with_mut_context(&buffer[..], &mut context)
                .expect_err("Parsed successfully when we had Pad2 with upper bits set"),
            ExtensionHeaderOptionParsingError::UnrecognizedOption {
                pointer: 6,
                action: ExtensionHeaderOptionAction::DiscardPacketSendIcmpNoMulticast,
            }
        );
        assert_eq!(context.position, 6);
        assert_eq!(context.options_parsed, 1);

        // Test parsing PadN but with upper bits set.
        #[rustfmt::skip]
        let buffer = [
            0,                               // Pad1
            1, 0,                            // Pad2
            // 0b11000001 -> action = 0b11, mutable = 0b0, option type = 0b00001
            // (matching lower-order bits of PadN).
            0xC1, 8, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let mut context = ExtensionHeaderOptionContext::new(5);
        assert_eq!(
            Records::<_, HopByHopOptionsImpl>::parse_with_mut_context(&buffer[..], &mut context)
                .expect_err("Parsed successfully when we had PadN with upper bits set"),
            ExtensionHeaderOptionParsingError::UnrecognizedOption {
                pointer: 8,
                action: ExtensionHeaderOptionAction::DiscardPacketSendIcmpNoMulticast,
            }
        );
        assert_eq!(context.position, 8);
        assert_eq!(context.options_parsed, 2);
    }

    #[test]
    fn test_destination_options() {
        // Test parsing of Pad1 (marked as NOP)
        let buffer = [0; 10];
        let mut context = ExtensionHeaderOptionContext::new(5);
        let options =
            Records::<_, DestinationOptionsImpl>::parse_with_mut_context(&buffer[..], &mut context)
                .unwrap();
        assert_eq!(options.iter().count(), 0);
        assert_eq!(context.position, 15);
        assert_eq!(context.options_parsed, 10);

        // Test parsing of Pad1 w/ PadN (treated as NOP)
        #[rustfmt::skip]
        let buffer = [
            0,                            // Pad1
            1, 0,                         // Pad2
            1, 8, 0, 0, 0, 0, 0, 0, 0, 0, // Pad10
        ];
        let mut context = ExtensionHeaderOptionContext::new(5);
        let options =
            Records::<_, DestinationOptionsImpl>::parse_with_mut_context(&buffer[..], &mut context)
                .unwrap();
        assert_eq!(options.iter().count(), 0);
        assert_eq!(context.position, 18);
        assert_eq!(context.options_parsed, 3);

        // Test parsing with an unknown option type but its action is
        // skip/continue
        #[rustfmt::skip]
        let buffer = [
            0,                            // Pad1
            63, 1, 0,                     // Unrecognized Option Type but can skip/continue
            1,  6, 0, 0, 0, 0, 0, 0,      // Pad8
        ];
        let mut context = ExtensionHeaderOptionContext::new(5);
        let options =
            Records::<_, DestinationOptionsImpl>::parse_with_mut_context(&buffer[..], &mut context)
                .unwrap();
        let options: Vec<DestinationOption<'_>> = options.iter().collect();
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].action, ExtensionHeaderOptionAction::SkipAndContinue);
        assert_eq!(context.position, 17);
        assert_eq!(context.options_parsed, 3);
    }

    #[test]
    fn test_destination_options_err() {
        // Test parsing but missing last 2 bytes
        #[rustfmt::skip]
        let buffer = [
            0,                            // Pad1
            1, 0,                         // Pad2
            1, 8, 0, 0, 0, 0, 0, 0,       // Pad10 (but missing 2 bytes)
        ];
        let mut context = ExtensionHeaderOptionContext::new(5);
        assert_eq!(
            Records::<_, DestinationOptionsImpl>::parse_with_mut_context(&buffer[..], &mut context)
                .expect_err("Parsed successfully when we were short 2 bytes"),
            ExtensionHeaderOptionParsingError::BufferExhausted
        );
        assert_eq!(context.position, 8);
        assert_eq!(context.options_parsed, 2);

        // Test parsing with unknown option type but action set to discard
        #[rustfmt::skip]
        let buffer = [
            1,   1, 0,                    // Pad3
            127, 0,                       // Unrecognized Option Type w/ action to discard
            1,   6, 0, 0, 0, 0, 0, 0,     // Pad8
        ];
        let mut context = ExtensionHeaderOptionContext::new(5);
        assert_eq!(
            Records::<_, DestinationOptionsImpl>::parse_with_mut_context(&buffer[..], &mut context)
                .expect_err("Parsed successfully when we had an unrecognized option type"),
            ExtensionHeaderOptionParsingError::UnrecognizedOption {
                pointer: 8,
                action: ExtensionHeaderOptionAction::DiscardPacket,
            }
        );
        assert_eq!(context.position, 8);
        assert_eq!(context.options_parsed, 1);

        // Test parsing with unknown option type but action set to discard and
        // send ICMP.
        #[rustfmt::skip]
        let buffer = [
            1,   1, 0,                    // Pad3
            191, 0,                       // Unrecognized Option Type w/ action to discard
                                          // & send icmp
            1,   6, 0, 0, 0, 0, 0, 0,     // Pad8
        ];
        let mut context = ExtensionHeaderOptionContext::new(5);
        assert_eq!(
            Records::<_, DestinationOptionsImpl>::parse_with_mut_context(&buffer[..], &mut context)
                .expect_err("Parsed successfully when we had an unrecognized option type"),
            ExtensionHeaderOptionParsingError::UnrecognizedOption {
                pointer: 8,
                action: ExtensionHeaderOptionAction::DiscardPacketSendIcmp,
            }
        );
        assert_eq!(context.position, 8);
        assert_eq!(context.options_parsed, 1);

        // Test parsing with unknown option type but action set to discard and
        // send ICMP if not sending to a multicast address
        #[rustfmt::skip]
        let buffer = [
            1,   1, 0,                    // Pad3
            255, 0,                       // Unrecognized Option Type w/ action to discard
                                          // & send icmp if no multicast
            1,   6, 0, 0, 0, 0, 0, 0,     // Pad8
        ];
        let mut context = ExtensionHeaderOptionContext::new(5);
        assert_eq!(
            Records::<_, DestinationOptionsImpl>::parse_with_mut_context(&buffer[..], &mut context)
                .expect_err("Parsed successfully when we had an unrecognized option type"),
            ExtensionHeaderOptionParsingError::UnrecognizedOption {
                pointer: 8,
                action: ExtensionHeaderOptionAction::DiscardPacketSendIcmpNoMulticast,
            }
        );
        assert_eq!(context.position, 8);
        assert_eq!(context.options_parsed, 1);
    }

    #[test]
    fn test_hop_by_hop_options_ext_hdr() {
        // Test parsing of just a single Hop By Hop Extension Header.
        // The hop by hop options will only be pad options.
        let context =
            Ipv6ExtensionHeaderParsingContext::new(Ipv6ExtHdrType::HopByHopOptions.into());
        #[rustfmt::skip]
        let buffer = [
            IpProto::Tcp.into(),     // Next Header
            1,                       // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            1,  4, 0, 0, 0, 0,       // Pad6
            63, 6, 0, 0, 0, 0, 0, 0, // Unrecognized option type w/ action set to skip/continue
        ];
        let ext_hdrs =
            Records::<&[u8], Ipv6ExtensionHeaderImpl>::parse_with_context(&buffer[..], context)
                .unwrap();
        let ext_hdrs: Vec<Ipv6ExtensionHeader<'_>> = ext_hdrs.iter().collect();
        assert_eq!(ext_hdrs.len(), 1);
        if let Ipv6ExtensionHeader::HopByHopOptions { options } = &ext_hdrs[0] {
            // Everything should have been a NOP/ignore except for the unrecognized type
            let options: Vec<HopByHopOption<'_>> = options.iter().collect();
            assert_eq!(options.len(), 1);
            assert_eq!(options[0].action, ExtensionHeaderOptionAction::SkipAndContinue);
        } else {
            panic!("Should have matched HopByHopOptions {:?}", ext_hdrs[0]);
        }
    }

    #[test]
    fn test_hop_by_hop_options_ext_hdr_err() {
        // Test parsing of just a single Hop By Hop Extension Header with errors.

        // Test with invalid Next Header
        let context =
            Ipv6ExtensionHeaderParsingContext::new(Ipv6ExtHdrType::HopByHopOptions.into());
        #[rustfmt::skip]
        let buffer = [
            255,                  // Next Header (Invalid)
            0,                    // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            1, 4, 0, 0, 0, 0,     // Pad6
        ];
        let error =
            Records::<&[u8], Ipv6ExtensionHeaderImpl>::parse_with_context(&buffer[..], context)
                .expect_err("Parsed successfully when the next header was invalid");
        if let Ipv6ExtensionHeaderParsingError::UnrecognizedNextHeader { pointer, must_send_icmp } =
            error
        {
            assert_eq!(pointer, IPV6_FIXED_HDR_LEN as u32);
            assert!(!must_send_icmp);
        } else {
            panic!("Should have matched with UnrecognizedNextHeader: {:?}", error);
        }

        // Test with invalid option type w/ action = discard.
        let context =
            Ipv6ExtensionHeaderParsingContext::new(Ipv6ExtHdrType::HopByHopOptions.into());
        #[rustfmt::skip]
        let buffer = [
            IpProto::Tcp.into(),      // Next Header
            1,                        // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            1,   4, 0, 0, 0, 0,       // Pad6
            127, 6, 0, 0, 0, 0, 0, 0, // Unrecognized option type w/ action = discard
        ];
        let error =
            Records::<&[u8], Ipv6ExtensionHeaderImpl>::parse_with_context(&buffer[..], context)
                .expect_err("Parsed successfully with an unrecognized option type");
        if let Ipv6ExtensionHeaderParsingError::UnrecognizedOption {
            pointer,
            must_send_icmp,
            action,
        } = error
        {
            assert_eq!(pointer, IPV6_FIXED_HDR_LEN as u32 + 8);
            assert!(must_send_icmp);
            assert_eq!(action, ExtensionHeaderOptionAction::DiscardPacket);
        } else {
            panic!("Should have matched with UnrecognizedOption: {:?}", error);
        }

        // Test with invalid option type w/ action = discard & send icmp
        let context =
            Ipv6ExtensionHeaderParsingContext::new(Ipv6ExtHdrType::HopByHopOptions.into());
        #[rustfmt::skip]
        let buffer = [
            IpProto::Tcp.into(),      // Next Header
            1,                        // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            1,   4, 0, 0, 0, 0,       // Pad6
            191, 6, 0, 0, 0, 0, 0, 0, // Unrecognized option type w/ action = discard & send icmp
        ];
        let error =
            Records::<&[u8], Ipv6ExtensionHeaderImpl>::parse_with_context(&buffer[..], context)
                .expect_err("Parsed successfully with an unrecognized option type");
        if let Ipv6ExtensionHeaderParsingError::UnrecognizedOption {
            pointer,
            must_send_icmp,
            action,
        } = error
        {
            assert_eq!(pointer, IPV6_FIXED_HDR_LEN as u32 + 8);
            assert!(must_send_icmp);
            assert_eq!(action, ExtensionHeaderOptionAction::DiscardPacketSendIcmp);
        } else {
            panic!("Should have matched with UnrecognizedOption: {:?}", error);
        }

        // Test with invalid option type w/ action = discard & send icmp if not multicast
        let context =
            Ipv6ExtensionHeaderParsingContext::new(Ipv6ExtHdrType::HopByHopOptions.into());
        #[rustfmt::skip]
        let buffer = [
            IpProto::Tcp.into(),      // Next Header
            1,                        // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            1,   4, 0, 0, 0, 0,       // Pad6
            255, 6, 0, 0, 0, 0, 0, 0, // Unrecognized option type w/ action = discard & send icmp
                                      // if destination address is not a multicast
        ];
        let error =
            Records::<&[u8], Ipv6ExtensionHeaderImpl>::parse_with_context(&buffer[..], context)
                .expect_err("Parsed successfully with an unrecognized option type");
        if let Ipv6ExtensionHeaderParsingError::UnrecognizedOption {
            pointer,
            must_send_icmp,
            action,
        } = error
        {
            assert_eq!(pointer, IPV6_FIXED_HDR_LEN as u32 + 8);
            assert!(must_send_icmp);
            assert_eq!(action, ExtensionHeaderOptionAction::DiscardPacketSendIcmpNoMulticast);
        } else {
            panic!("Should have matched with UnrecognizedOption: {:?}", error);
        }

        // Test with valid option type and invalid data w/ action = skip & continue
        let context =
            Ipv6ExtensionHeaderParsingContext::new(Ipv6ExtHdrType::HopByHopOptions.into());
        #[rustfmt::skip]
            let buffer = [
            IpProto::Tcp.into(),      // Next Header
            0,                        // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            5,   3, 0, 0, 0,          // RouterAlert, but with a wrong data length.
            0,                        // Pad1
        ];
        let error =
            Records::<&[u8], Ipv6ExtensionHeaderImpl>::parse_with_context(&buffer[..], context)
                .expect_err(
                    "Should fail to parse the header because one of the option is malformed",
                );
        if let Ipv6ExtensionHeaderParsingError::ErroneousHeaderField { pointer, .. } = error {
            assert_eq!(pointer, IPV6_FIXED_HDR_LEN as u32 + 3);
        } else {
            panic!("Should have matched with UnrecognizedOption: {:?}", error);
        }
    }

    #[test]
    fn test_routing_ext_hdr() {
        // Test parsing of just a single Routing Extension Header.
        let context = Ipv6ExtensionHeaderParsingContext::new(Ipv6ExtHdrType::Routing.into());
        #[rustfmt::skip]
        let buffer = [
            IpProto::Tcp.into(), // Next Header
            4,                   // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                   // Routing Type
            0,                   // Segments Left (0 so no error)
            0, 0, 0, 0,          // Reserved
            // Addresses for Routing Header w/ Type 0
            0,  1,  2,  3,  4,  5,  6,  7,  8,  9,  10, 11, 12, 13, 14, 15,
            16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,

        ];
        let ext_hdrs =
            Records::<&[u8], Ipv6ExtensionHeaderImpl>::parse_with_context(&buffer[..], context)
                .unwrap();
        let results: Vec<_> = ext_hdrs.iter().collect();
        assert_eq!(results.len(), 1);
        if let Ipv6ExtensionHeader::Routing { routing_data } = &results[0] {
            assert_eq!(routing_data.routing_type(), Err(RoutingTypeParseError::UnsupportedType(0)));
            assert_eq!(routing_data.segments_left(), 0);
        } else {
            panic!("Should have matched with RoutingExtensionHeader");
        }
    }

    #[test]
    fn test_routing_ext_hdr_err() {
        // Test parsing of just a single Routing Extension Header with errors.

        // Explicitly test to make sure we do not support routing type 0 as per RFC 5095
        let context = Ipv6ExtensionHeaderParsingContext::new(Ipv6ExtHdrType::Routing.into());
        #[rustfmt::skip]
        let buffer = [
            IpProto::Tcp.into(), // Next Header
            4,                   // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                   // Routing Type (0 which we should not support)
            1,                   // Segments Left
            0, 0, 0, 0,          // Reserved
            // Addresses for Routing Header w/ Type 0
            0,  1,  2,  3,  4,  5,  6,  7,  8,  9,  10, 11, 12, 13, 14, 15,
            16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
        ];
        let error =
            Records::<&[u8], Ipv6ExtensionHeaderImpl>::parse_with_context(&buffer[..], context)
                .expect_err("Parsed successfully when the routing type was set to 0");
        if let Ipv6ExtensionHeaderParsingError::ErroneousHeaderField { pointer, must_send_icmp } =
            error
        {
            assert_eq!(pointer, IPV6_FIXED_HDR_LEN as u32 + 2);
            assert!(must_send_icmp);
        } else {
            panic!("Should have matched with ErroneousHeaderField: {:?}", error);
        }

        // Test Invalid Next Header
        let context = Ipv6ExtensionHeaderParsingContext::new(Ipv6ExtHdrType::Routing.into());
        #[rustfmt::skip]
        let buffer = [
            255,                 // Next Header (Invalid)
            4,                   // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                   // Routing Type
            0,                   // Segments Left
            0, 0, 0, 0,          // Reserved
            // Addresses for Routing Header w/ Type 0
            0,  1,  2,  3,  4,  5,  6,  7,  8,  9,  10, 11, 12, 13, 14, 15,
            16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,

        ];
        let error =
            Records::<&[u8], Ipv6ExtensionHeaderImpl>::parse_with_context(&buffer[..], context)
                .expect_err("Parsed successfully when the next header was invalid");
        if let Ipv6ExtensionHeaderParsingError::UnrecognizedNextHeader { pointer, must_send_icmp } =
            error
        {
            assert_eq!(pointer, IPV6_FIXED_HDR_LEN as u32);
            assert!(!must_send_icmp);
        } else {
            panic!("Should have matched with UnrecognizedNextHeader: {:?}", error);
        }

        // Test Unrecognized Routing Type
        let context = Ipv6ExtensionHeaderParsingContext::new(Ipv6ExtHdrType::Routing.into());
        #[rustfmt::skip]
        let buffer = [
            IpProto::Tcp.into(), // Next Header
            4,                   // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            255,                 // Routing Type (Invalid)
            1,                   // Segments Left
            0, 0, 0, 0,          // Reserved
            // Addresses for Routing Header w/ Type 0
            0,  1,  2,  3,  4,  5,  6,  7,  8,  9,  10, 11, 12, 13, 14, 15,
            16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,

        ];
        let error =
            Records::<&[u8], Ipv6ExtensionHeaderImpl>::parse_with_context(&buffer[..], context)
                .expect_err("Parsed successfully with an unrecognized routing type");
        if let Ipv6ExtensionHeaderParsingError::ErroneousHeaderField { pointer, must_send_icmp } =
            error
        {
            // Should point to the location of the routing type.
            assert_eq!(pointer, IPV6_FIXED_HDR_LEN as u32 + 2);
            assert!(must_send_icmp);
        } else {
            panic!("Should have matched with ErroneousHeaderField: {:?}", error);
        }
    }

    #[test]
    fn test_fragment_ext_hdr() {
        // Test parsing of just a single Fragment Extension Header.
        let context = Ipv6ExtensionHeaderParsingContext::new(Ipv6ExtHdrType::Fragment.into());
        let frag_offset_res_m_flag: u16 = (5063 << 3) | 1;
        let identification: u32 = 3266246449;
        #[rustfmt::skip]
        let buffer = [
            IpProto::Tcp.into(),                   // Next Header
            0,                                     // Reserved
            (frag_offset_res_m_flag >> 8) as u8,   // Fragment Offset MSB
            (frag_offset_res_m_flag & 0xFF) as u8, // Fragment Offset LS5bits w/ Res w/ M Flag
            // Identification
            (identification >> 24) as u8,
            ((identification >> 16) & 0xFF) as u8,
            ((identification >> 8) & 0xFF) as u8,
            (identification & 0xFF) as u8,
        ];
        let ext_hdrs =
            Records::<&[u8], Ipv6ExtensionHeaderImpl>::parse_with_context(&buffer[..], context)
                .unwrap();
        let ext_hdrs: Vec<Ipv6ExtensionHeader<'_>> = ext_hdrs.iter().collect();
        assert_eq!(ext_hdrs.len(), 1);

        if let Ipv6ExtensionHeader::Fragment { fragment_data } = &ext_hdrs[0] {
            assert_eq!(fragment_data.fragment_offset().into_raw(), 5063);
            assert_eq!(fragment_data.m_flag(), true);
            assert_eq!(fragment_data.identification(), 3266246449);
        } else {
            panic!("Should have matched Fragment: {:?}", &ext_hdrs[0]);
        }
    }

    #[test]
    fn test_fragment_ext_hdr_err() {
        // Test parsing of just a single Fragment Extension Header with errors.

        // Test invalid Next Header
        let context = Ipv6ExtensionHeaderParsingContext::new(Ipv6ExtHdrType::Fragment.into());
        let frag_offset_res_m_flag: u16 = (5063 << 3) | 1;
        let identification: u32 = 3266246449;
        #[rustfmt::skip]
        let buffer = [
            255,                                   // Next Header (Invalid)
            0,                                     // Reserved
            (frag_offset_res_m_flag >> 8) as u8,   // Fragment Offset MSB
            (frag_offset_res_m_flag & 0xFF) as u8, // Fragment Offset LS5bits w/ Res w/ M Flag
            // Identification
            (identification >> 24) as u8,
            ((identification >> 16) & 0xFF) as u8,
            ((identification >> 8) & 0xFF) as u8,
            (identification & 0xFF) as u8,
        ];
        let error =
            Records::<&[u8], Ipv6ExtensionHeaderImpl>::parse_with_context(&buffer[..], context)
                .expect_err("Parsed successfully when the next header was invalid");
        if let Ipv6ExtensionHeaderParsingError::UnrecognizedNextHeader { pointer, must_send_icmp } =
            error
        {
            assert_eq!(pointer, IPV6_FIXED_HDR_LEN as u32);
            assert!(!must_send_icmp);
        } else {
            panic!("Should have matched with UnrecognizedNextHeader: {:?}", error);
        }
    }

    #[test]
    fn test_no_next_header_ext_hdr() {
        // Test parsing of just a single NoNextHeader Extension Header.
        let context = Ipv6ExtensionHeaderParsingContext::new(Ipv6Proto::NoNextHeader.into());
        #[rustfmt::skip]
        let buffer = [0, 0, 0, 0,];
        let ext_hdrs =
            Records::<&[u8], Ipv6ExtensionHeaderImpl>::parse_with_context(&buffer[..], context)
                .unwrap();
        assert_eq!(ext_hdrs.iter().count(), 0);
    }

    #[test]
    fn test_destination_options_ext_hdr() {
        // Test parsing of just a single Destination options Extension Header.
        // The destination options will only be pad options.
        let context =
            Ipv6ExtensionHeaderParsingContext::new(Ipv6ExtHdrType::DestinationOptions.into());
        #[rustfmt::skip]
        let buffer = [
            IpProto::Tcp.into(),     // Next Header
            1,                       // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            1, 4, 0, 0, 0, 0,        // Pad6
            63, 6, 0, 0, 0, 0, 0, 0, // Unrecognized option type w/ action set to skip/continue
        ];
        let ext_hdrs =
            Records::<&[u8], Ipv6ExtensionHeaderImpl>::parse_with_context(&buffer[..], context)
                .unwrap();
        let ext_hdrs: Vec<Ipv6ExtensionHeader<'_>> = ext_hdrs.iter().collect();
        assert_eq!(ext_hdrs.len(), 1);
        if let Ipv6ExtensionHeader::DestinationOptions { options } = &ext_hdrs[0] {
            // Everything should have been a NOP/ignore except for the unrecognized type
            let options: Vec<DestinationOption<'_>> = options.iter().collect();
            assert_eq!(options.len(), 1);
            assert_eq!(options[0].action, ExtensionHeaderOptionAction::SkipAndContinue);
        } else {
            panic!("Should have matched DestinationOptions: {:?}", &ext_hdrs[0]);
        }
    }

    #[test]
    fn test_destination_options_ext_hdr_err() {
        // Test parsing of just a single Destination Options Extension Header with errors.
        let context =
            Ipv6ExtensionHeaderParsingContext::new(Ipv6ExtHdrType::DestinationOptions.into());

        // Test with invalid Next Header
        #[rustfmt::skip]
        let buffer = [
            255,                  // Next Header (Invalid)
            0,                    // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            1, 4, 0, 0, 0, 0,     // Pad6
        ];
        let error =
            Records::<&[u8], Ipv6ExtensionHeaderImpl>::parse_with_context(&buffer[..], context)
                .expect_err("Parsed successfully when the next header was invalid");
        if let Ipv6ExtensionHeaderParsingError::UnrecognizedNextHeader { pointer, must_send_icmp } =
            error
        {
            assert_eq!(pointer, IPV6_FIXED_HDR_LEN as u32);
            assert!(!must_send_icmp);
        } else {
            panic!("Should have matched with UnrecognizedNextHeader: {:?}", error);
        }

        // Test with invalid option type w/ action = discard.
        let context =
            Ipv6ExtensionHeaderParsingContext::new(Ipv6ExtHdrType::DestinationOptions.into());
        #[rustfmt::skip]
        let buffer = [
            IpProto::Tcp.into(),      // Next Header
            1,                        // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            1,   4, 0, 0, 0, 0,       // Pad6
            127, 6, 0, 0, 0, 0, 0, 0, // Unrecognized option type w/ action = discard
        ];
        let error =
            Records::<&[u8], Ipv6ExtensionHeaderImpl>::parse_with_context(&buffer[..], context)
                .expect_err("Parsed successfully with an unrecognized option type");
        if let Ipv6ExtensionHeaderParsingError::UnrecognizedOption {
            pointer,
            must_send_icmp,
            action,
        } = error
        {
            assert_eq!(pointer, IPV6_FIXED_HDR_LEN as u32 + 8);
            assert!(must_send_icmp);
            assert_eq!(action, ExtensionHeaderOptionAction::DiscardPacket);
        } else {
            panic!("Should have matched with UnrecognizedOption: {:?}", error);
        }

        // Test with invalid option type w/ action = discard & send icmp
        let context =
            Ipv6ExtensionHeaderParsingContext::new(Ipv6ExtHdrType::DestinationOptions.into());
        #[rustfmt::skip]
        let buffer = [
            IpProto::Tcp.into(),      // Next Header
            1,                        // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            1,   4, 0, 0, 0, 0,       // Pad6
            191, 6, 0, 0, 0, 0, 0, 0, // Unrecognized option type w/ action = discard & send icmp
        ];
        let error =
            Records::<&[u8], Ipv6ExtensionHeaderImpl>::parse_with_context(&buffer[..], context)
                .expect_err("Parsed successfully with an unrecognized option type");
        if let Ipv6ExtensionHeaderParsingError::UnrecognizedOption {
            pointer,
            must_send_icmp,
            action,
        } = error
        {
            assert_eq!(pointer, IPV6_FIXED_HDR_LEN as u32 + 8);
            assert!(must_send_icmp);
            assert_eq!(action, ExtensionHeaderOptionAction::DiscardPacketSendIcmp);
        } else {
            panic!("Should have matched with UnrecognizedOption: {:?}", error);
        }

        // Test with invalid option type w/ action = discard & send icmp if not multicast
        let context =
            Ipv6ExtensionHeaderParsingContext::new(Ipv6ExtHdrType::DestinationOptions.into());
        #[rustfmt::skip]
        let buffer = [
            IpProto::Tcp.into(),      // Next Header
            1,                        // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            1,   4, 0, 0, 0, 0,       // Pad6
            255, 6, 0, 0, 0, 0, 0, 0, // Unrecognized option type w/ action = discard & send icmp
                                      // if destination address is not a multicast
        ];
        let error =
            Records::<&[u8], Ipv6ExtensionHeaderImpl>::parse_with_context(&buffer[..], context)
                .expect_err("Parsed successfully with an unrecognized option type");
        if let Ipv6ExtensionHeaderParsingError::UnrecognizedOption {
            pointer,
            must_send_icmp,
            action,
        } = error
        {
            assert_eq!(pointer, IPV6_FIXED_HDR_LEN as u32 + 8);
            assert!(must_send_icmp);
            assert_eq!(action, ExtensionHeaderOptionAction::DiscardPacketSendIcmpNoMulticast);
        } else {
            panic!("Should have matched with UnrecognizedOption: {:?}", error);
        }
    }

    #[test]
    fn test_multiple_ext_hdrs() {
        // Test parsing of multiple extension headers.
        let context =
            Ipv6ExtensionHeaderParsingContext::new(Ipv6ExtHdrType::HopByHopOptions.into());
        #[rustfmt::skip]
        let buffer = [
            // HopByHop Options Extension Header
            Ipv6ExtHdrType::Routing.into(), // Next Header
            0,                       // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                       // Pad1
            1, 0,                    // Pad2
            1, 1, 0,                 // Pad3

            // Routing Extension Header
            Ipv6ExtHdrType::DestinationOptions.into(), // Next Header
            4,                                  // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                                  // Routing Type
            0,                                  // Segments Left
            0, 0, 0, 0,                         // Reserved
            // Addresses for Routing Header w/ Type 0
            0,  1,  2,  3,  4,  5,  6,  7,  8,  9,  10, 11, 12, 13, 14, 15,
            16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,

            // Destination Options Extension Header
            IpProto::Tcp.into(),     // Next Header
            1,                       // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                       // Pad1
            1,  0,                   // Pad2
            1,  1, 0,                // Pad3
            63, 6, 0, 0, 0, 0, 0, 0, // Unrecognized type w/ action = discard
        ];
        let ext_hdrs =
            Records::<&[u8], Ipv6ExtensionHeaderImpl>::parse_with_context(&buffer[..], context)
                .unwrap();

        let ext_hdrs: Vec<Ipv6ExtensionHeader<'_>> = ext_hdrs.iter().collect();
        assert_eq!(ext_hdrs.len(), 3);

        // Check first extension header (hop-by-hop options)
        if let Ipv6ExtensionHeader::HopByHopOptions { options } = &ext_hdrs[0] {
            // Everything should have been a NOP/ignore
            assert_eq!(options.iter().count(), 0);
        } else {
            panic!("Should have matched HopByHopOptions: {:?}", &ext_hdrs[0]);
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
            // Everything should have been a NOP/ignore except for the unrecognized type
            let options: Vec<DestinationOption<'_>> = options.iter().collect();
            assert_eq!(options.len(), 1);
            assert_eq!(options[0].action, ExtensionHeaderOptionAction::SkipAndContinue);
        } else {
            panic!("Should have matched DestinationOptions: {:?}", ext_hdrs[2]);
        }
    }

    #[test]
    fn test_multiple_ext_hdrs_errs() {
        // Test parsing of multiple extension headers with errors.

        // Test Invalid next header in the second extension header.
        let context =
            Ipv6ExtensionHeaderParsingContext::new(Ipv6ExtHdrType::HopByHopOptions.into());
        #[rustfmt::skip]
        let buffer = [
            // HopByHop Options Extension Header
            Ipv6ExtHdrType::Routing.into(), // Next Header
            0,                       // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                       // Pad1
            1, 0,                    // Pad2
            1, 1, 0,                 // Pad3

            // Routing Extension Header
            255,                                // Next Header (Invalid)
            4,                                  // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                                  // Routing Type
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
        ];
        let error =
            Records::<&[u8], Ipv6ExtensionHeaderImpl>::parse_with_context(&buffer[..], context)
                .expect_err("Parsed successfully when the next header was invalid");
        if let Ipv6ExtensionHeaderParsingError::UnrecognizedNextHeader { pointer, must_send_icmp } =
            error
        {
            assert_eq!(pointer, IPV6_FIXED_HDR_LEN as u32 + 8);
            assert!(!must_send_icmp);
        } else {
            panic!("Should have matched with UnrecognizedNextHeader: {:?}", error);
        }

        // Test HopByHop extension header not being the very first extension header
        let context = Ipv6ExtensionHeaderParsingContext::new(Ipv6ExtHdrType::Routing.into());
        #[rustfmt::skip]
        let buffer = [
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
            Ipv6ExtHdrType::DestinationOptions.into(), // Next Header
            0,                                  // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                                  // Pad1
            1, 0,                               // Pad2
            1, 1, 0,                            // Pad3

            // Destination Options Extension Header
            IpProto::Tcp.into(),    // Next Header
            1,                      // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                      // Pad1
            1, 0,                   // Pad2
            1, 1, 0,                // Pad3
            1, 6, 0, 0, 0, 0, 0, 0, // Pad8
        ];
        let error =
            Records::<&[u8], Ipv6ExtensionHeaderImpl>::parse_with_context(&buffer[..], context)
                .expect_err("Parsed successfully when a hop by hop extension header was not the fist extension header");
        if let Ipv6ExtensionHeaderParsingError::UnrecognizedNextHeader { pointer, must_send_icmp } =
            error
        {
            assert_eq!(pointer, IPV6_FIXED_HDR_LEN as u32);
            assert!(!must_send_icmp);
        } else {
            panic!("Should have matched with UnrecognizedNextHeader: {:?}", error);
        }

        // Test parsing of destination options with an unrecognized option type w/ action
        // set to discard and send icmp
        let context =
            Ipv6ExtensionHeaderParsingContext::new(Ipv6ExtHdrType::HopByHopOptions.into());
        #[rustfmt::skip]
        let buffer = [
            // HopByHop Options Extension Header
            Ipv6ExtHdrType::DestinationOptions.into(), // Next Header
            0,                       // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                       // Pad1
            1, 0,                    // Pad2
            1, 1, 0,                 // Pad3

            // Destination Options Extension Header
            IpProto::Tcp.into(),      // Next Header
            1,                        // Hdr Ext Len (In 8-octet units, not including first 8 octets)
            0,                        // Pad1
            1,   0,                   // Pad2
            1,   1, 0,                // Pad3
            191, 6, 0, 0, 0, 0, 0, 0, // Unrecognized type w/ action = discard
        ];
        let error =
            Records::<&[u8], Ipv6ExtensionHeaderImpl>::parse_with_context(&buffer[..], context)
                .expect_err("Parsed successfully with an unrecognized destination option type");
        if let Ipv6ExtensionHeaderParsingError::UnrecognizedOption {
            pointer,
            must_send_icmp,
            action,
        } = error
        {
            assert_eq!(pointer, IPV6_FIXED_HDR_LEN as u32 + 16);
            assert!(must_send_icmp);
            assert_eq!(action, ExtensionHeaderOptionAction::DiscardPacketSendIcmp);
        } else {
            panic!("Should have matched with UnrecognizedOption: {:?}", error);
        }
    }

    #[test]
    fn test_serialize_hbh_router_alert() {
        let mut buffer = [0u8; 4];
        let option = HopByHopOption {
            action: ExtensionHeaderOptionAction::SkipAndContinue,
            mutable: false,
            data: HopByHopOptionData::RouterAlert { data: 0 },
        };
        <HopByHopOption<'_> as RecordBuilder>::serialize_into(&option, &mut buffer);
        assert_eq!(&buffer[..], &[5, 2, 0, 0]);
    }

    #[test]
    fn test_parse_hbh_router_alert() {
        // Test RouterAlert with correct data length.
        let context = ExtensionHeaderOptionContext::new(0);
        let buffer = [5, 2, 0, 0];

        let options =
            Records::<_, HopByHopOptionsImpl>::parse_with_context(&buffer[..], context).unwrap();
        let rtralrt = options.iter().next().unwrap();
        assert!(!rtralrt.mutable);
        assert_eq!(rtralrt.action, ExtensionHeaderOptionAction::SkipAndContinue);
        assert_eq!(rtralrt.data, HopByHopOptionData::RouterAlert { data: 0 });

        // Test that the three higher-order bits are considered part of the
        // Option Type when parsing options.
        let context = ExtensionHeaderOptionContext::new(5);
        // 0b11000101 -> action = 0b11, mutable = 0b0, option type = 0b00101
        // (matching lower-order bits of RouterAlert).
        let buffer = [0xC5, 2, 0, 0];

        let error = Records::<_, HopByHopOptionsImpl>::parse_with_context(&buffer[..], context)
            .expect_err("UnrecognizedOption should have been returned");
        assert_eq!(
            error,
            ExtensionHeaderOptionParsingError::UnrecognizedOption {
                pointer: 5,
                action: ExtensionHeaderOptionAction::DiscardPacketSendIcmpNoMulticast
            }
        );

        // Test RouterAlert with wrong data length.
        let result = <HopByHopOptionDataImpl as ExtensionHeaderOptionDataImpl>::parse_option(
            5,
            &buffer[1..],
            &mut (),
            false,
        );
        assert_eq!(result, ExtensionHeaderOptionDataParseResult::ErrorAt(1));

        let context = ExtensionHeaderOptionContext::new(5);
        let buffer = [5, 3, 0, 0, 0];

        let error = Records::<_, HopByHopOptionsImpl>::parse_with_context(&buffer[..], context)
            .expect_err(
                "Parsing a malformed option with recognized kind but with wrong data should fail",
            );
        assert_eq!(error, ExtensionHeaderOptionParsingError::ErroneousOptionField { pointer: 6 });
    }

    // Construct a bunch of `HopByHopOption`s according to lengths:
    // if `length` is
    //   - `None`: RouterAlert is generated.
    //   - `Some(l)`: the Unrecognized option with length `l - 2` is constructed.
    //     It is `l - 2` so that the whole record has size l.
    // This function is used so that the alignment of RouterAlert can be tested.
    fn trivial_hbh_options(lengths: &[Option<usize>]) -> Vec<HopByHopOption<'static>> {
        static ZEROES: [u8; 16] = [0u8; 16];
        lengths
            .iter()
            .map(|l| HopByHopOption {
                mutable: false,
                action: ExtensionHeaderOptionAction::SkipAndContinue,
                data: match l {
                    Some(l) => HopByHopOptionData::Unrecognized {
                        kind: 1,
                        len: (*l - 2) as u8,
                        data: &ZEROES[0..*l - 2],
                    },
                    None => HopByHopOptionData::RouterAlert { data: 0 },
                },
            })
            .collect()
    }

    #[test]
    fn test_aligned_records_serializer() {
        // Test whether we can serialize our RouterAlert at 2-byte boundary
        for i in 2..12 {
            let options = trivial_hbh_options(&[Some(i), None]);
            let ser = AlignedRecordSequenceBuilder::<
                ExtensionHeaderOption<HopByHopOptionData<'_>>,
                _,
            >::new(2, options.iter());
            let mut buf = [0u8; 16];
            ser.serialize_into(&mut buf[0..16]);
            let base = (i + 1) & !1;
            // we want to make sure that our RouterAlert is aligned at 2-byte boundary.
            assert_eq!(&buf[base..base + 4], &[5, 2, 0, 0]);
        }
    }
}
