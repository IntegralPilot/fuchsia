// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![warn(
    missing_docs,
    unreachable_patterns,
    clippy::useless_conversion,
    clippy::redundant_clone,
    clippy::precedence
)]

//! Pcapng parser and serializer.
//!
//! The reference document is the [pcapng RFC].
//!
//! [pcapng RFC]: https://www.ietf.org/archive/id/draft-ietf-opsawg-pcapng-05.html

use nom::error::{ErrorKind, ParseError};
use nom::{Finish as _, IResult, Parser};
use std::borrow::Cow;
use thiserror::Error;
use zerocopy::byteorder::little_endian::{U16, U32, U64};
use zerocopy::{Immutable, IntoBytes};

/// Generated bindings for libpcap.
#[cfg(feature = "compile")]
mod bindings;
/// Module for compiling pcap filters using libpcap.
#[cfg(feature = "compile")]
pub mod compile;

/// Link type of an interface.
///
/// The values are defined in the pcap linktype [pcapng RFC].
///
/// [pcapng RFC]: https://datatracker.ietf.org/doc/html/draft-ietf-opsawg-pcaplinktype-17.
#[derive(Copy, Clone, Debug, PartialEq, Eq, strum_macros::FromRepr, strum_macros::Display)]
#[repr(u16)]
pub enum LinkType {
    /// Ethernet link type.
    Ethernet = 1,
    /// Pure IP link type.
    PureIp = 101,
}

impl From<LinkType> for u16 {
    fn from(link_type: LinkType) -> Self {
        link_type as u16
    }
}

/// Option codes specific to Interface Description Blocks.
///
/// See [pcapng RFC Section 4.2] for the full list of codes.
///
/// [pcapng RFC Section 4.2]: https://www.ietf.org/archive/id/draft-ietf-opsawg-pcapng-05.html#name-interface-description-block
#[derive(Copy, Clone, Debug, PartialEq, Eq, strum_macros::FromRepr, strum_macros::Display)]
#[repr(u16)]
pub enum InterfaceDescriptionOptionCode {
    /// End of options.
    EndOfOpt = 0,
    /// Interface name.
    IfName = 2,
}

impl From<InterfaceDescriptionOptionCode> for u16 {
    fn from(option_code: InterfaceDescriptionOptionCode) -> Self {
        option_code as u16
    }
}

/// Interface Description Block options.
///
/// See [pcapng RFC Section 4.2] for the list of options.
///
/// [pcapng RFC Section 4.2]: https://www.ietf.org/archive/id/draft-ietf-opsawg-pcapng-05.html#name-interface-description-block
#[derive(Debug, PartialEq, Eq)]
pub enum InterfaceDescriptionOption<'a> {
    /// Interface name.
    IfName(Cow<'a, str>),
    /// End of options.
    EndOfOpt,
}

impl<'a> InterfaceDescriptionOption<'a> {
    /// Returns the Interface Description Block option code as listed in
    /// [pcapng RFC Section 4.2].
    ///
    /// [pcapng RFC Section 4.2]: https://www.ietf.org/archive/id/draft-ietf-opsawg-pcapng-05.html#name-interface-description-block
    pub fn code(&self) -> InterfaceDescriptionOptionCode {
        match self {
            Self::IfName(_) => InterfaceDescriptionOptionCode::IfName,
            Self::EndOfOpt => InterfaceDescriptionOptionCode::EndOfOpt,
        }
    }
}

/// Errors that can occur while parsing a pcapng file.
#[derive(Error, Debug, PartialEq)]
pub enum ParsingError<'a> {
    /// Encountered a block type different from expected.
    #[error("Unexpected block type: got {got}, want {want:?}")]
    UnexpectedBlockType {
        /// The block type found.
        got: u32,
        /// The block type expected.
        want: BlockType,
    },

    /// Block length is shorter than the minimum 12 bytes.
    #[error("Block length is shorter than 12 bytes: {0}")]
    BlockLengthTooShort(u32),

    /// Header and footer block lengths do not match.
    #[error("Header and footer block lengths disagree: header {header} footer {footer}")]
    BlockLengthsDisagree {
        /// Length in header.
        header: u32,
        /// Length in footer.
        footer: u32,
    },

    /// Section Header Block options are not supported.
    #[error("Unsupported Section Header Block options")]
    SectionHeaderBlockOptionsUnspported,

    /// Enhanced Packet Block options are not supported.
    #[error("Unsupported Enhanced Packet Block options")]
    EnhancedPacketBlockOptionsUnspported,

    /// Unsupported option code in Interface Description Block.
    #[error("Unsupported IDB option code: {0}")]
    UnsupportedInterfaceDescriptionOption(u16),

    /// End-of-option encountered with non-zero length.
    #[error("End-of-option encountered with non-zero length")]
    EndOfOptLengthNotZero(u16),

    /// Duplicate Interface Name option found.
    #[error("Duplicate Interface Name option: name1 {name1} name2 {name2}")]
    DuplicateIfNameOption {
        /// First name.
        name1: Cow<'a, str>,
        /// Second name.
        name2: Cow<'a, str>,
    },

    /// Data found after end-of-opt.
    #[error("Data after end-of-opt")]
    DataAfterEndOfOpt,

    /// Unsupported link type.
    #[error("Unsupported link type")]
    UnsupportedLinkType(u16),

    /// Invalid Section Header Block magic.
    #[error("Invalid Section Header Block magic")]
    InvalidMagic,

    /// Unsupported Section Header Block major version.
    #[error("Unsupported Section Header Block major version")]
    UnsupportedMajorVersion(u16),

    /// Unsupported Section Header Block minor version.
    #[error("Unsupported Section Header Block minor version")]
    UnsupportedMinorVersion(u16),

    /// Big endian pcapng is not supported.
    #[error("Big endian pcapng is not supported")]
    BigEndianNotSupported,

    /// Nom parsing error.
    #[error("Nom error at {0:?}: {1:?}")]
    Nom(&'a [u8], ErrorKind),
}

impl<'a> ParseError<&'a [u8]> for ParsingError<'a> {
    fn from_error_kind(input: &'a [u8], kind: ErrorKind) -> Self {
        ParsingError::Nom(input, kind)
    }
    fn append(_: &'a [u8], _: ErrorKind, other: Self) -> Self {
        // NB: Don't accumulate multiple parse errors. Keep the original.
        other
    }
}

type PcapResult<'a, T> = IResult<&'a [u8], T, ParsingError<'a>>;

fn parse_idb_option<'a>(input: &'a [u8]) -> PcapResult<'a, InterfaceDescriptionOption<'a>> {
    let (input, code) = nom::number::complete::le_u16(input)?;
    let (input, len) = nom::number::complete::le_u16(input)?;
    match InterfaceDescriptionOptionCode::from_repr(code)
        .ok_or(nom::Err::Failure(ParsingError::UnsupportedInterfaceDescriptionOption(code)))?
    {
        InterfaceDescriptionOptionCode::EndOfOpt => {
            if len != 0 {
                return Err(nom::Err::Failure(ParsingError::EndOfOptLengthNotZero(len)));
            }
            Ok((input, InterfaceDescriptionOption::EndOfOpt))
        }
        InterfaceDescriptionOptionCode::IfName => {
            let (input, value) = nom::bytes::complete::take(len)(input)?;
            let padding_len = len.next_multiple_of(4) - len;
            let (input, _) = nom::bytes::complete::take(padding_len)(input)?;
            // Choose to repair UTF-8 strings with replacement characters.
            // pcapng RFC Section 3.6.3 states:
            //
            //  Implementations MAY discard a string that are invalid UTF-8 or
            //  MAY repair the string by replacing invalid octet sequences with
            //  valid sequences.
            Ok((input, InterfaceDescriptionOption::IfName(String::from_utf8_lossy(value))))
        }
    }
}

/// Options parsed from an Interface Description Block as defined in [pcapng RFC Section 4.2].
///
/// [pcapng RFC Section 4.2]: https://www.ietf.org/archive/id/draft-ietf-opsawg-pcapng-05.html#name-interface-description-block
#[derive(Debug, PartialEq, Eq)]
pub struct ParsedInterfaceDescriptionOptions<'a> {
    /// The interface name option.
    pub if_name: Option<Cow<'a, str>>,
}

fn parse_idb_options<'a>(
    mut input: &'a [u8],
) -> PcapResult<'a, ParsedInterfaceDescriptionOptions<'a>> {
    let mut rtn = ParsedInterfaceDescriptionOptions { if_name: None };
    while input.len() > 0 {
        let (remaining, option) = parse_idb_option(input)?;
        input = remaining;
        match option {
            InterfaceDescriptionOption::EndOfOpt => {
                if input.len() > 0 {
                    return Err(nom::Err::Failure(ParsingError::DataAfterEndOfOpt));
                }
            }
            InterfaceDescriptionOption::IfName(if_name) => {
                if let Some(name) = rtn.if_name.take() {
                    return Err(nom::Err::Failure(ParsingError::DuplicateIfNameOption {
                        name1: name,
                        name2: if_name,
                    }));
                }
                rtn.if_name = Some(if_name);
            }
        }
    }
    Ok((input, rtn))
}

/// A parsed Section Header Block as defined in [pcapng RFC Section 4.1].
///
/// [pcapng RFC Section 4.1]: https://www.ietf.org/archive/id/draft-ietf-opsawg-pcapng-05.html#name-section-header-block
#[derive(Debug, PartialEq, Eq)]
pub struct ParsedSectionHeader {
    /// The length of the section.
    pub section_length: u64,
}

/// Byte order magic as defined in [pcapng RFC Section 4.1].
///
/// [pcapng RFC Section 4.1]: https://www.ietf.org/archive/id/draft-ietf-opsawg-pcapng-05.html#name-section-header-block
const BYTE_ORDER_MAGIC: u32 = 0x1A2B3C4D;

/// A parsed Interface Description Block as defined in [pcapng RFC Section 4.2].
///
/// [pcapng RFC Section 4.2]: https://www.ietf.org/archive/id/draft-ietf-opsawg-pcapng-05.html#name-interface-description-block
#[derive(Debug, PartialEq, Eq)]
pub struct ParsedInterfaceDescription<'a> {
    /// The link type of the interface.
    pub link_type: LinkType,
    /// The maximum number of bytes captured from each packet.
    pub snap_len: u32,
    /// Options associated with the interface.
    pub options: ParsedInterfaceDescriptionOptions<'a>,
}

/// A parsed Enhanced Packet Block as defined in [pcapng RFC Section 4.3].
///
/// [pcapng RFC Section 4.3]: https://www.ietf.org/archive/id/draft-ietf-opsawg-pcapng-05.html#name-enhanced-packet-block
#[derive(Debug, PartialEq, Eq)]
pub struct ParsedEnhancedPacket<'a> {
    /// The ID of the interface the packet was captured on.
    pub interface_id: u32,
    /// The time the packet was captured.
    pub timestamp: std::time::SystemTime,
    /// The length of the packet data actually captured.
    pub captured_length: u32,
    /// The original length of the packet off the wire.
    pub original_length: u32,
    /// The captured packet data.
    pub packet_data: &'a [u8],
}

/// Pcapng block type.
///
/// See [pcapng RFC Section 10.1] for the list of block type codes.
///
/// [pcapng RFC Section 10.1]: https://www.ietf.org/archive/id/draft-ietf-opsawg-pcapng-05.html#section-10.1
#[derive(Copy, Clone, Debug, PartialEq, Eq, strum_macros::FromRepr, strum_macros::Display)]
#[repr(u32)]
pub enum BlockType {
    /// Section Header Block.
    SectionHeader = 0x0A0D0D0A,
    /// Interface Description Block.
    InterfaceDescription = 0x00000001,
    /// Enhanced Packet Block.
    EnhancedPacket = 0x00000006,
}

impl From<BlockType> for u32 {
    fn from(block_type: BlockType) -> Self {
        block_type as u32
    }
}

/// A parsed pcapng section.
///
/// Provides an iterator to iterate over the Enhanced Packet Blocks.
#[derive(Debug)]
pub struct PcapNgSection<'a> {
    /// The Section Header Block as defined in [pcapng RFC Section 4.1].
    ///
    /// [pcapng RFC Section 4.1]: https://www.ietf.org/archive/id/draft-ietf-opsawg-pcapng-05.html#name-section-header-block
    pub header: ParsedSectionHeader,
    /// The Interface Description Blocks as defined in [pcapng RFC Section 4.2].
    ///
    /// [pcapng RFC Section 4.2]: https://www.ietf.org/archive/id/draft-ietf-opsawg-pcapng-05.html#name-interface-description-block
    pub interfaces: Vec<ParsedInterfaceDescription<'a>>,
    input: &'a [u8],
}

impl<'a> PcapNgSection<'a> {
    /// Returns an iterator over the packet blocks in the section.
    pub fn packet_blocks(&self) -> PcapNgPacketIter<'a> {
        PcapNgPacketIter { input: self.input }
    }
}

/// An iterator over Enhanced Packet Blocks in a pcapng section as defined in [pcapng RFC Section 4.3].
///
/// [pcapng RFC Section 4.3]: https://www.ietf.org/archive/id/draft-ietf-opsawg-pcapng-05.html#name-enhanced-packet-block
pub struct PcapNgPacketIter<'a> {
    input: &'a [u8],
}

impl<'a> Iterator for PcapNgPacketIter<'a> {
    type Item = Result<ParsedEnhancedPacket<'a>, nom::Err<ParsingError<'a>>>;

    fn next(&mut self) -> Option<Self::Item> {
        (!self.input.is_empty()).then(|| {
            parse_block::<ParsedEnhancedPacket<'a>>(self.input).map(|(rem, epb)| {
                self.input = rem;
                epb
            })
        })
    }
}

const BLOCK_HEADER_FOOTER_LEN: u32 = 12;

/// The only major version of pcapng supported by this crate as found in [pcapng
/// RFC Section 4.1].
///
/// [pcapng RFC section 4.1]: https://www.ietf.org/archive/id/draft-ietf-opsawg-pcapng-05.html#name-section-header-block
const MAJOR_VERSION: u16 = 1;

/// The only minor version of pcapng supported by this crate as found in [pcapng
/// RFC Section 4.1].
///
/// [pcapng RFC section 4.1]: https://www.ietf.org/archive/id/draft-ietf-opsawg-pcapng-05.html#name-section-header-block
const MINOR_VERSION: u16 = 0;

/// A trait for types that represent a pcapng block.
pub trait PcapNgBlock<'a>: Sized {
    /// The type of block.
    const BLOCK_TYPE: BlockType;
    /// Parses the block body excluding the block type and block total length
    /// header and the repeated block total length footer.
    fn parse(body: &'a [u8]) -> PcapResult<'a, Self>;
}

impl<'a> PcapNgBlock<'a> for ParsedSectionHeader {
    const BLOCK_TYPE: BlockType = BlockType::SectionHeader;

    // Parse a Section Header Block as defined in [pcapng RFC Section 4.1].
    //
    // The header is as follows:
    //
    //                         1                   2                   3
    //     0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
    //    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //  0 |                   Block Type = 0x0A0D0D0A                     |
    //    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //  4 |                      Block Total Length                       |
    //    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //  8 |                      Byte-Order Magic                         |
    //    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    // 12 |          Major Version        |         Minor Version         |
    //    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    // 16 |                                                               |
    //    |                          Section Length                       |
    //    |                                                               |
    //    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    // 24 /                                                               /
    //    /                      Options (variable)                       /
    //    /                                                               /
    //    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //    |                      Block Total Length                       |
    //    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //
    // [pcapng RFC Section 4.1]: https://www.ietf.org/archive/id/draft-ietf-opsawg-pcapng-05.html#name-section-header-block
    fn parse(body: &'a [u8]) -> PcapResult<'a, Self> {
        let (body, _magic) = nom::bytes::complete::tag(&BYTE_ORDER_MAGIC.to_le_bytes()[..])(body)?;
        let (body, major_version) = nom::number::complete::le_u16(body)?;
        if major_version != MAJOR_VERSION {
            return Err(nom::Err::Failure(ParsingError::UnsupportedMajorVersion(major_version)));
        }
        let (body, minor_version) = nom::number::complete::le_u16(body)?;
        // According to the pcapng RFC:
        //
        //   Some pcapng file writers have used a minor version of 2, but the file
        //   format did not change incompatibly (new block types were added);
        //   Readers of pcapng files MUST treat a Minor Version of 2 as equivalent
        //   to a Minor Version of 0
        if minor_version != MINOR_VERSION && minor_version != 2 {
            return Err(nom::Err::Failure(ParsingError::UnsupportedMinorVersion(minor_version)));
        }
        let (body, section_length) = nom::number::complete::le_u64(body)?;
        if body.len() > 0 {
            return Err(nom::Err::Failure(ParsingError::SectionHeaderBlockOptionsUnspported));
        }
        Ok((body, ParsedSectionHeader { section_length }))
    }
}

impl<'a> PcapNgBlock<'a> for ParsedInterfaceDescription<'a> {
    const BLOCK_TYPE: BlockType = BlockType::InterfaceDescription;

    // Parse an Interface Description Block as defined in [pcapng RFC Section 4.2].
    //
    // The header is as follows:
    //
    //                         1                   2                   3
    //     0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
    //    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //  0 |                    Block Type = 0x00000001                    |
    //    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //  4 |                      Block Total Length                       |
    //    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //  8 |           LinkType            |           Reserved            |
    //    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    // 12 |                            SnapLen                            |
    //    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    // 16 /                                                               /
    //    /                      Options (variable)                       /
    //    /                                                               /
    //    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //    |                      Block Total Length                       |
    //    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //
    // [pcapng RFC Section 4.2]: https://www.ietf.org/archive/id/draft-ietf-opsawg-pcapng-05.html#name-interface-description-block
    fn parse(body: &'a [u8]) -> PcapResult<'a, Self> {
        let (body, link_type_code) = nom::number::complete::le_u16(body)?;
        let link_type = LinkType::from_repr(link_type_code)
            .ok_or(nom::Err::Failure(ParsingError::UnsupportedLinkType(link_type_code)))?;
        let (body, _reserved) = nom::number::complete::le_u16(body)?;
        let (body, snap_len) = nom::number::complete::le_u32(body)?;

        let (body, options) = parse_idb_options(body)?;
        Ok((body, ParsedInterfaceDescription { link_type, snap_len, options }))
    }
}

impl<'a> PcapNgBlock<'a> for ParsedEnhancedPacket<'a> {
    const BLOCK_TYPE: BlockType = BlockType::EnhancedPacket;

    // Parse an Enhanced Packet Block as defined in [pcapng RFC Section 4.3].
    //
    // The header is as follows:
    //
    //                         1                   2                   3
    //     0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
    //    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //  0 |                    Block Type = 0x00000006                    |
    //    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //  4 |                      Block Total Length                       |
    //    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //  8 |                         Interface ID                          |
    //    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    // 12 |                       (Upper 32 bits)                         |
    //    + - - - - - - - - - - - -  Timestamp  - - - - - - - - - - - - - +
    // 16 |                       (Lower 32 bits)                         |
    //    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    // 20 |                    Captured Packet Length                     |
    //    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    // 24 |                    Original Packet Length                     |
    //    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    // 28 /                                                               /
    //    /                          Packet Data                          /
    //    /              variable length, padded to 32 bits               /
    //    /                                                               /
    //    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //    /                                                               /
    //    /                      Options (variable)                       /
    //    /                                                               /
    //    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //    |                      Block Total Length                       |
    //    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //
    // [pcapng RFC Section 4.3]: https://www.ietf.org/archive/id/draft-ietf-opsawg-pcapng-05.html#name-enhanced-packet-block
    fn parse(body: &'a [u8]) -> PcapResult<'a, Self> {
        let (body, interface_id) = nom::number::complete::le_u32(body)?;
        let (body, timestamp_high) = nom::number::complete::le_u32(body)?;
        let (body, timestamp_low) = nom::number::complete::le_u32(body)?;
        let timestamp = std::time::UNIX_EPOCH
            + std::time::Duration::from_micros(
                (u64::from(timestamp_high) << 32) | u64::from(timestamp_low),
            );
        let (body, captured_length) = nom::number::complete::le_u32(body)?;
        let (body, original_length) = nom::number::complete::le_u32(body)?;
        let (body, packet_data) = nom::bytes::complete::take(captured_length)(body)?;
        let padded_len = captured_length.next_multiple_of(4) - captured_length;
        let (body, _) = nom::bytes::complete::take(padded_len)(body)?;
        if body.len() > 0 {
            return Err(nom::Err::Failure(ParsingError::EnhancedPacketBlockOptionsUnspported));
        }
        Ok((
            body,
            ParsedEnhancedPacket {
                interface_id,
                timestamp,
                captured_length,
                original_length,
                packet_data,
            },
        ))
    }
}

fn parse_block<'a, T: PcapNgBlock<'a>>(input: &'a [u8]) -> PcapResult<'a, T> {
    let (input, block_type) = nom::number::complete::le_u32(input)?;
    if block_type != u32::from(T::BLOCK_TYPE) {
        return Err(nom::Err::Error(ParsingError::UnexpectedBlockType {
            got: block_type,
            want: T::BLOCK_TYPE,
        }));
    }

    // NB: The total_length value should not be used until after the byte order
    // magic is checked for endianness!
    let (input, total_length) = nom::number::complete::le_u32(input)?;
    if T::BLOCK_TYPE == BlockType::SectionHeader {
        let (_, byte_order_magic) =
            nom::combinator::peek(nom::bytes::complete::take(4usize)).parse(input)?;
        if byte_order_magic == BYTE_ORDER_MAGIC.to_be_bytes() {
            return Err(nom::Err::Failure(ParsingError::BigEndianNotSupported));
        }
        if byte_order_magic != BYTE_ORDER_MAGIC.to_le_bytes() {
            return Err(nom::Err::Failure(ParsingError::InvalidMagic));
        }
    }
    let body_len = total_length
        .checked_sub(BLOCK_HEADER_FOOTER_LEN)
        .ok_or(nom::Err::Failure(ParsingError::BlockLengthTooShort(total_length)))?;

    let (input, body) = nom::bytes::complete::take(body_len)(input)?;
    let (input, verify_length) = nom::number::complete::le_u32(input)?;
    if total_length != verify_length {
        return Err(nom::Err::Failure(ParsingError::BlockLengthsDisagree {
            header: total_length,
            footer: verify_length,
        }));
    }

    let (_, result) = T::parse(body)?;
    Ok((input, result))
}

/// Parses a pcapng file from a byte slice.
///
/// This parsing logic is not aiming to support general purpose pcap files.
/// It expects a single section block, which begins with N interface blocks and
/// ends with M enhanced packet blocks.
pub fn parse_pcapng<'a>(input: &'a [u8]) -> Result<PcapNgSection<'a>, ParsingError<'a>> {
    let (input, header) = parse_block::<ParsedSectionHeader>(input).finish()?;
    let (input, interfaces) =
        nom::multi::many1(parse_block::<ParsedInterfaceDescription<'a>>).parse(input).finish()?;
    Ok(PcapNgSection { header, interfaces, input })
}

/// A Section Header Block structure for serialization as defined in [pcapng RFC Section 4.1].
///
/// [pcapng RFC Section 4.1]: https://www.ietf.org/archive/id/draft-ietf-opsawg-pcapng-05.html#name-section-header-block
#[derive(IntoBytes, Immutable)]
#[repr(C)]
pub struct SectionHeaderBlock {
    /// Block type code.
    pub block_type: U32,
    /// Total length of the block.
    pub block_total_length: U32,
    /// Byte order magic.
    pub byte_order_magic: U32,
    /// Major version.
    pub major_version: U16,
    /// Minor version.
    pub minor_version: U16,
    /// Section length.
    pub section_length: U64,
    /// Total length of the block (repeated).
    pub block_total_length2: U32,
}

impl SectionHeaderBlock {
    /// The size of [`Self`].
    pub const SIZE: u32 = std::mem::size_of::<Self>() as u32;
}

/// According to [pcapng RFC Section 4.1]:
///
///  If the Section Length is -1 (0xFFFFFFFFFFFFFFFF), this means that the size
///  of the section is not specified, and the only way to skip the section is to
///  parse the blocks that it contains.
///
/// [pcapng RFC Section 4.1]:
///     https://www.ietf.org/archive/id/draft-ietf-opsawg-pcapng-05.html#name-section-header-block
pub const UNSPECIFIED_SECTION_SIZE: u64 = u64::MAX;

impl SectionHeaderBlock {
    /// Creates a new Section Header Block with default values as defined in
    /// [pcapng RFC Section 4.1].
    ///
    /// [pcapng RFC Section 4.1]:
    ///     https://www.ietf.org/archive/id/draft-ietf-opsawg-pcapng-05.html#name-section-header-block
    pub fn new() -> Self {
        Self {
            block_type: U32::new(BlockType::SectionHeader.into()),
            block_total_length: U32::new(Self::SIZE),
            byte_order_magic: U32::new(BYTE_ORDER_MAGIC),
            major_version: U16::new(MAJOR_VERSION),
            minor_version: U16::new(MINOR_VERSION),
            section_length: U64::new(UNSPECIFIED_SECTION_SIZE),
            block_total_length2: U32::new(Self::SIZE),
        }
    }
}

/// An Interface Description Block structure for serialization as defined in [pcapng RFC Section 4.2].
///
/// [pcapng RFC Section 4.2]: https://www.ietf.org/archive/id/draft-ietf-opsawg-pcapng-05.html#name-interface-description-block
#[derive(IntoBytes, Immutable)]
#[repr(C)]
pub struct InterfaceDescriptionBlockHeader {
    /// Block type code.
    pub block_type: U32,
    /// Total length of the block.
    pub block_total_length: U32,
    /// Link type.
    pub link_type: U16,
    /// Reserved.
    pub reserved: U16,
    /// Maximum number of bytes from the start of a packet that gets captured.
    pub snap_len: U32,
}

impl InterfaceDescriptionBlockHeader {
    /// The size of [`Self`].
    pub const SIZE: u32 = std::mem::size_of::<Self>() as u32;

    /// Creates a new Interface Description Block header as defined in [pcapng RFC Section 4.2].
    ///
    /// [pcapng RFC Section 4.2]: https://www.ietf.org/archive/id/draft-ietf-opsawg-pcapng-05.html#name-interface-description-block
    pub fn new(link_type: LinkType, block_total_length: u32) -> Self {
        Self {
            block_type: U32::new(BlockType::InterfaceDescription.into()),
            block_total_length: U32::new(block_total_length),
            link_type: U16::new(link_type.into()),
            reserved: U16::new(0),
            snap_len: U32::new(0),
        }
    }
}

/// The size of the block length footer that appears in all pcapng blocks.
pub const BLOCK_LEN_FOOTER_SIZE: u32 = 4;

/// Write an interface description block with an interface name option.
pub fn write_interface_description_block<W: std::io::Write>(
    mut writer: W,
    link_type: LinkType,
    interface_name: &str,
) -> Result<(), std::io::Error> {
    let total_len = InterfaceDescriptionBlockHeader::SIZE
        + OptionHeader::SIZE // if_name option header
        + u32::try_from(interface_name.len().next_multiple_of(4)).unwrap()
        + OptionHeader::SIZE // end_of_opt option header
        + BLOCK_LEN_FOOTER_SIZE;
    let idb_header = InterfaceDescriptionBlockHeader::new(link_type, total_len);
    writer.write_all(idb_header.as_bytes())?;
    write_if_name_option(&mut writer, interface_name)?;
    writer.write_all(OptionHeader::new_end_of_opt().as_bytes())?;
    writer.write_all(&total_len.to_le_bytes())?;
    Ok(())
}

/// Writes a Section Header Block and an Interface Description Block.
pub fn write_prelude<W: std::io::Write>(
    mut writer: W,
    link_type: LinkType,
    interface_name: &str,
) -> Result<(), std::io::Error> {
    writer.write_all(SectionHeaderBlock::new().as_bytes())?;
    write_interface_description_block(&mut writer, link_type, interface_name)?;
    Ok(())
}

/// An Enhanced Packet Block structure for serialization as defined in [pcapng RFC Section 4.3].
///
/// [pcapng RFC Section 4.3]: https://www.ietf.org/archive/id/draft-ietf-opsawg-pcapng-05.html#name-enhanced-packet-block
#[derive(IntoBytes, Immutable)]
#[repr(C)]
pub struct EnhancedPacketBlockHeader {
    /// Block type code.
    pub block_type: U32,
    /// Total length of the block.
    pub block_total_length: U32,
    /// Interface ID.
    pub interface_id: U32,
    /// Upper 32 bits of timestamp.
    pub timestamp_upper: U32,
    /// Lower 32 bits of timestamp.
    pub timestamp_lower: U32,
    /// Captured packet length.
    pub captured_packet_length: U32,
    /// Original packet length.
    pub original_packet_length: U32,
}

impl EnhancedPacketBlockHeader {
    /// The size of [`Self`].
    pub const SIZE: u32 = std::mem::size_of::<Self>() as u32;

    /// Creates a new Enhanced Packet Block header as defined in [pcapng RFC Section 4.3].
    ///
    /// [pcapng RFC Section 4.3]: https://www.ietf.org/archive/id/draft-ietf-opsawg-pcapng-05.html#name-enhanced-packet-block
    pub fn new(
        block_total_length: u32,
        interface_id: u32,
        timestamp: std::time::SystemTime,
        captured_packet_length: u32,
        original_packet_length: u32,
    ) -> Self {
        let timestamp = timestamp
            .duration_since(std::time::UNIX_EPOCH)
            .expect("current time should be after UNIX epoch")
            .as_micros();
        let timestamp = u64::try_from(timestamp)
            .expect("current time overflows microseconds since UNIX epoch in u64");
        Self {
            block_type: U32::new(BlockType::EnhancedPacket.into()),
            block_total_length: U32::new(block_total_length),
            interface_id: U32::new(interface_id),
            timestamp_upper: U32::new((timestamp >> 32) as u32),
            timestamp_lower: U32::new(timestamp as u32),
            captured_packet_length: U32::new(captured_packet_length),
            original_packet_length: U32::new(original_packet_length),
        }
    }
}

/// Writes an Enhanced Packet Block.
pub fn write_enhanced_packet_block<W: std::io::Write>(
    mut writer: W,
    interface_id: u32,
    timestamp: std::time::SystemTime,
    packet: &[u8],
    original_packet_len: u32,
) -> Result<(), std::io::Error> {
    let packet_padded_len = packet.len().next_multiple_of(4);
    let total_len = EnhancedPacketBlockHeader::SIZE
        + u32::try_from(packet_padded_len).unwrap()
        + BLOCK_LEN_FOOTER_SIZE;
    let header = EnhancedPacketBlockHeader::new(
        total_len,
        interface_id,
        timestamp,
        u32::try_from(packet.len()).unwrap(),
        original_packet_len,
    );
    writer.write_all(header.as_bytes())?;
    writer.write_all(packet)?;
    writer.write_all(&[0; 3][..packet_padded_len - packet.len()])?;
    writer.write_all(&total_len.to_le_bytes())?;
    Ok(())
}

/// An option header structure for serialization.
#[derive(IntoBytes, Immutable)]
#[repr(C)]
pub struct OptionHeader {
    /// Option code.
    pub code: U16,
    /// Option length.
    pub length: U16,
}

impl OptionHeader {
    /// The size of [`Self`].
    pub const SIZE: u32 = std::mem::size_of::<Self>() as u32;

    /// Creates a new Option Header for EndOfOpt.
    pub fn new_end_of_opt() -> Self {
        Self {
            code: U16::new(InterfaceDescriptionOptionCode::EndOfOpt.into()),
            length: U16::new(0),
        }
    }

    fn new_if_name(length: u16) -> Self {
        Self {
            code: U16::new(InterfaceDescriptionOptionCode::IfName.into()),
            length: U16::new(length),
        }
    }
}

/// Writes a single if_name option.
///
/// `buf` must be long enough to write the option and padding.
pub fn write_if_name_option<W: std::io::Write>(mut writer: W, name: &str) -> std::io::Result<()> {
    let len = name.len();
    let total_len = len.next_multiple_of(4);
    writer.write_all(OptionHeader::new_if_name(len.try_into().unwrap()).as_bytes())?;
    writer.write_all(name.as_bytes())?;
    writer.write_all(&[0; 3][..total_len - len])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocopy::IntoBytes;
    use zerocopy::byteorder::little_endian::U16;

    #[test]
    fn test_parse_idb_options() {
        let mut buf = Vec::new();
        write_if_name_option(&mut buf, "lo").expect("write if name option");
        buf.extend_from_slice(OptionHeader::new_end_of_opt().as_bytes());

        let (rem, options) = parse_idb_options(&buf).expect("parse options failed");
        assert!(rem.is_empty());
        assert_eq!(
            options,
            ParsedInterfaceDescriptionOptions { if_name: Some(Cow::Borrowed("lo")) }
        );
    }

    #[test]
    fn test_write_idb_includes_end_of_opt() {
        let mut buf = Vec::new();
        let if_name = "lo";
        write_interface_description_block(&mut buf, LinkType::Ethernet, if_name)
            .expect("write interface description block");

        // Sanity check with regular parser.
        let (remaining, _parsed_idb) =
            parse_block::<ParsedInterfaceDescription<'_>>(&buf).expect("parse block failed");
        assert!(remaining.is_empty());

        // Now strictly verify EndOfOpt is present by parsing options manually.
        // IDB header is 16 bytes. Footer is 4 bytes.
        let option_start_offset = std::mem::size_of::<InterfaceDescriptionBlockHeader>();
        let option_end_offset = buf.len()
            - usize::try_from(BLOCK_LEN_FOOTER_SIZE).expect("block len footer size fits in usize");
        let mut options_buf = &buf[option_start_offset..option_end_offset];

        let mut end_of_opt_found = false;
        while !options_buf.is_empty() {
            let (rem, option) = parse_idb_option(options_buf).expect("parse option failed");
            options_buf = rem;
            if let InterfaceDescriptionOption::EndOfOpt = option {
                assert_eq!(options_buf, &[] as &[u8]);
                end_of_opt_found = true;
                break;
            }
        }
        assert!(end_of_opt_found);
    }

    #[test]
    fn test_parse_options_succeed_no_end_of_opt() {
        let mut buf = Vec::new();
        write_if_name_option(&mut buf, "lo").expect("write if name option");

        let (rem, options) = parse_idb_options(&buf).expect("parse options failed");
        assert!(rem.is_empty());
        assert_eq!(
            options,
            ParsedInterfaceDescriptionOptions { if_name: Some(Cow::Borrowed("lo")) }
        );
    }

    #[test]
    fn test_parse_options_fail_non_zero_len_end_of_opt() {
        let mut buf = Vec::new();
        buf.extend_from_slice(
            (OptionHeader {
                code: U16::new(InterfaceDescriptionOptionCode::EndOfOpt.into()),
                length: U16::new(1),
            })
            .as_bytes(),
        );

        let res = parse_idb_options(&buf);
        assert_eq!(res, Err(nom::Err::Failure(ParsingError::EndOfOptLengthNotZero(1))));
    }

    #[test]
    fn test_parse_options_fail_if_name_after_end_of_opt() {
        let mut buf = Vec::new();
        buf.extend_from_slice(OptionHeader::new_end_of_opt().as_bytes());
        write_if_name_option(&mut buf, "lo").expect("write if name option");

        let res = parse_idb_options(&buf);
        assert_eq!(res, Err(nom::Err::Failure(ParsingError::DataAfterEndOfOpt)));
    }

    #[test]
    fn test_parse_shb_invalid_magic() {
        let mut buf = Vec::new();
        let mut shb = SectionHeaderBlock::new();
        shb.byte_order_magic = U32::new(BYTE_ORDER_MAGIC + 1);
        buf.extend_from_slice(shb.as_bytes());

        let res = parse_block::<ParsedSectionHeader>(&buf);
        assert_eq!(res, Err(nom::Err::Failure(ParsingError::InvalidMagic)));
    }

    #[test]
    fn test_parse_shb_unsupported_major() {
        let mut buf = Vec::new();
        let mut shb = SectionHeaderBlock::new();
        let wrong_major_version = 2;
        shb.major_version = U16::new(wrong_major_version);
        buf.extend_from_slice(shb.as_bytes());

        let res = parse_block::<ParsedSectionHeader>(&buf);
        assert_eq!(
            res,
            Err(nom::Err::Failure(ParsingError::UnsupportedMajorVersion(wrong_major_version)))
        );
    }

    #[test]
    fn test_parse_shb_unsupported_minor() {
        let mut buf = Vec::new();
        let mut shb = SectionHeaderBlock::new();
        let wrong_minor_version = 1;
        shb.minor_version = U16::new(wrong_minor_version);
        buf.extend_from_slice(shb.as_bytes());

        let res = parse_block::<ParsedSectionHeader>(&buf);
        assert_eq!(
            res,
            Err(nom::Err::Failure(ParsingError::UnsupportedMinorVersion(wrong_minor_version)))
        );
    }

    #[test]
    fn test_parse_pcap_valid() {
        let mut buf = Vec::new();

        // Section Header Block
        let shb = SectionHeaderBlock::new();
        buf.extend_from_slice(shb.as_bytes());

        // Interface Description Block
        write_interface_description_block(&mut buf, LinkType::Ethernet, "lo")
            .expect("write interface description block");

        // Enhanced Packet Block
        let timestamp = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        let packet = [1, 2, 3, 4, 5];
        let packet_len = packet.len() as u32;
        write_enhanced_packet_block(
            &mut buf, 0, /* interface id */
            timestamp, &packet, packet_len,
        )
        .expect("write enhanced packet block");

        // Second Enhanced Packet Block
        let timestamp2 = timestamp + std::time::Duration::from_secs(2_000_000_000);
        let packet2 = [6, 7, 8];
        let packet_len2 = packet2.len() as u32;
        write_enhanced_packet_block(
            &mut buf,
            0, /* interface id */
            timestamp2,
            &packet2,
            packet_len2,
        )
        .expect("write enhanced packet block");

        let file = parse_pcapng(&buf).expect("parse pcap failed");

        assert_eq!(file.header, ParsedSectionHeader { section_length: u64::MAX });
        assert_eq!(
            file.interfaces,
            vec![ParsedInterfaceDescription {
                link_type: LinkType::Ethernet,
                snap_len: 0,
                options: ParsedInterfaceDescriptionOptions { if_name: Some(Cow::Borrowed("lo")) },
            }]
        );

        let packets: Vec<_> =
            file.packet_blocks().collect::<Result<Vec<_>, _>>().expect("iterate failed");

        let expected = vec![
            ParsedEnhancedPacket {
                interface_id: 0,
                timestamp,
                captured_length: packet_len,
                original_length: packet_len,
                packet_data: &packet,
            },
            ParsedEnhancedPacket {
                interface_id: 0,
                timestamp: timestamp2,
                captured_length: packet_len2,
                original_length: packet_len2,
                packet_data: &packet2,
            },
        ];
        assert_eq!(packets, expected);
    }
    #[test]
    fn test_parse_block_too_short() {
        let mut buf = Vec::new();
        let mut shb = SectionHeaderBlock::new();
        let short_block_len = BLOCK_HEADER_FOOTER_LEN - 1;
        shb.block_total_length = U32::new(short_block_len);
        buf.extend_from_slice(shb.as_bytes());

        let res = parse_block::<ParsedSectionHeader>(&buf);
        assert_eq!(res, Err(nom::Err::Failure(ParsingError::BlockLengthTooShort(short_block_len))));
    }

    #[test]
    fn test_parse_block_length_disagrees() {
        let mut buf = Vec::new();
        let mut shb = SectionHeaderBlock::new();
        shb.block_total_length2 = U32::new(SectionHeaderBlock::SIZE + 1);
        buf.extend_from_slice(shb.as_bytes());

        let res = parse_block::<ParsedSectionHeader>(&buf);
        assert_eq!(
            res,
            Err(nom::Err::Failure(ParsingError::BlockLengthsDisagree {
                header: SectionHeaderBlock::SIZE,
                footer: SectionHeaderBlock::SIZE + 1,
            }))
        );
    }

    #[test]
    fn test_parse_shb_big_endian_not_supported() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&u32::from(BlockType::SectionHeader).to_be_bytes());
        // NB: It's important that the block size is big endian because this has
        // a higher probability of tripping up a bad parser.
        buf.extend_from_slice(&SectionHeaderBlock::SIZE.to_be_bytes());
        buf.extend_from_slice(&BYTE_ORDER_MAGIC.to_be_bytes());
        // This is an incomplete SHB but the byte order magic value should be
        // checked before anything else happens anyway.

        let res = parse_block::<ParsedSectionHeader>(&buf);
        assert_eq!(res, Err(nom::Err::Failure(ParsingError::BigEndianNotSupported)));
    }
}
