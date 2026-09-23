// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::string::parse_padded_string;
use crate::{METADATA_RECORD_TYPE, ParseError, ParseResult, trace_header};
use flyweights::FlyStr;
use nom::Parser;
use nom::combinator::all_consuming;

const PROVIDER_INFO_METADATA_TYPE: u8 = 1;
const PROVIDER_SECTION_METADATA_TYPE: u8 = 2;
const PROVIDER_EVENT_METADATA_TYPE: u8 = 3;
const TRACE_INFO_METADATA_TYPE: u8 = 4;

#[derive(Clone, Debug, PartialEq)]
pub struct Provider {
    pub id: u32,
    pub name: FlyStr,
}

#[derive(Debug, PartialEq)]
pub(super) enum MetadataRecord {
    ProviderInfo(ProviderInfoMetadataRecord),
    ProviderSection(ProviderSectionMetadataRecord),
    ProviderEvent(ProviderEventMetadataRecord),
    TraceInfo(TraceInfoMetadataRecord),
    Unknown { raw_type: u8 },
}

impl MetadataRecord {
    pub(super) fn parse(buf: &[u8]) -> ParseResult<'_, Self> {
        use nom::combinator::map;
        match BaseMetadataHeader::parse(buf)?.1.metadata_type() {
            PROVIDER_INFO_METADATA_TYPE => {
                map(ProviderInfoMetadataRecord::parse, |i| Self::ProviderInfo(i)).parse(buf)
            }
            PROVIDER_SECTION_METADATA_TYPE => {
                map(ProviderSectionMetadataRecord::parse, |s| Self::ProviderSection(s)).parse(buf)
            }
            PROVIDER_EVENT_METADATA_TYPE => {
                map(ProviderEventMetadataRecord::parse, |e| Self::ProviderEvent(e)).parse(buf)
            }
            TRACE_INFO_METADATA_TYPE => {
                map(TraceInfoMetadataRecord::parse, |t| Self::TraceInfo(t)).parse(buf)
            }
            unknown => Ok((buf, Self::Unknown { raw_type: unknown })),
        }
    }
}

macro_rules! metadata_header {
    ($name:ident $(($metadata_ty:expr))? { $($record_specific:tt)* }) => {
        trace_header! {
            $name (METADATA_RECORD_TYPE) {
                $($record_specific)*
                u8, metadata_type: 16, 19;
            } => |_h: &$name| {
                $(if _h.metadata_type() != $metadata_ty {
                    return Err(ParseError::WrongType {
                        context: stringify!($name),
                        expected: $metadata_ty,
                        observed: _h.metadata_type(),
                    });
                })?
                Ok(())
            }
        }
    };
}

metadata_header! { BaseMetadataHeader {} }

#[derive(Debug, PartialEq)]
pub(super) struct ProviderInfoMetadataRecord {
    pub provider_id: u32,
    pub name: FlyStr,
}

metadata_header! {
    ProviderInfoMetadataHeader (PROVIDER_INFO_METADATA_TYPE) {
        u32, provider_id: 20, 51;
        u8, name_len: 52, 59;
    }
}

impl ProviderInfoMetadataRecord {
    fn parse(buf: &[u8]) -> ParseResult<'_, Self> {
        let (buf, header) = ProviderInfoMetadataHeader::parse(buf)?;
        let (rem, payload) = header.take_payload(buf)?;
        let (empty, name) =
            all_consuming(|p| parse_padded_string(header.name_len() as usize, p)).parse(payload)?;
        assert!(empty.is_empty(), "all_consuming must not return any remaining buffer");
        Ok((rem, Self { provider_id: header.provider_id(), name: name.into() }))
    }
}

#[derive(Debug, PartialEq)]
pub(super) struct ProviderSectionMetadataRecord {
    pub provider_id: u32,
}

metadata_header! {
    ProviderSectionMetadataHeader (PROVIDER_SECTION_METADATA_TYPE) {
        u32, provider_id: 20, 51;
    }
}

impl ProviderSectionMetadataRecord {
    fn parse(buf: &[u8]) -> ParseResult<'_, Self> {
        let (buf, header) = ProviderSectionMetadataHeader::parse(buf)?;
        let (rem, payload) = header.take_payload(buf)?;
        if !payload.is_empty() {
            return Err(nom::Err::Failure(ParseError::InvalidSize));
        }
        Ok((rem, Self { provider_id: header.provider_id() }))
    }
}

const PROVIDER_EVENT_BUFFER_FULL: u8 = 0;

#[derive(Debug, PartialEq)]
pub(super) struct ProviderEventMetadataRecord {
    pub provider_id: u32,
    pub event: ProviderEvent,
}

metadata_header! {
    ProviderEventMetadataHeader (PROVIDER_EVENT_METADATA_TYPE) {
        u32, provider_id: 20, 51;
        u8, event_id: 52, 55;
    }
}

impl ProviderEventMetadataRecord {
    fn parse(buf: &[u8]) -> ParseResult<'_, Self> {
        let (buf, header) = ProviderEventMetadataHeader::parse(buf)?;
        let provider_id = header.provider_id();
        let event = match header.event_id() {
            PROVIDER_EVENT_BUFFER_FULL => ProviderEvent::BufferFull,
            unknown => ProviderEvent::Unknown { raw_type: unknown },
        };
        let (rem, payload) = header.take_payload(buf)?;
        if !payload.is_empty() {
            return Err(nom::Err::Failure(ParseError::InvalidSize));
        }
        Ok((rem, Self { provider_id, event }))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ProviderEvent {
    BufferFull,
    Unknown { raw_type: u8 },
}

const MAGIC_NUMBER_TRACE_INFO_TYPE: u8 = 0;
const MAGIC_NUMBER: u32 = 0x16547846;

#[derive(Debug, PartialEq)]
pub(super) enum TraceInfoMetadataRecord {
    MagicNumber,
}

impl TraceInfoMetadataRecord {
    fn parse(buf: &[u8]) -> ParseResult<'_, Self> {
        // The magic number record is the only trace info record currently specified, so we can
        // parse it directly here without confusing it with other record types.
        let (buf, header) = MagicNumberHeader::parse(buf)?;
        if header.trace_info_type() != MAGIC_NUMBER_TRACE_INFO_TYPE {
            return Err(nom::Err::Error(ParseError::WrongType {
                context: "MagicNumber",
                expected: MAGIC_NUMBER_TRACE_INFO_TYPE,
                observed: header.trace_info_type(),
            }));
        }
        if header.magic_number() != MAGIC_NUMBER {
            return Err(nom::Err::Failure(ParseError::InvalidMagicNumber {
                observed: header.magic_number(),
            }));
        }

        let (rem, payload) = header.take_payload(buf)?;
        if !payload.is_empty() {
            return Err(nom::Err::Failure(ParseError::InvalidSize));
        }

        Ok((rem, Self::MagicNumber))
    }
}

metadata_header! {
    MagicNumberHeader (TRACE_INFO_METADATA_TYPE) {
        u8, trace_info_type: 20, 23;
        u32, magic_number: 24, 55;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RawTraceRecord;
    use crate::fxt_builder::FxtBuilder;

    #[test]
    fn basic_provider_info() {
        let name = "hello";
        let mut header = ProviderInfoMetadataHeader::empty();
        header.set_metadata_type(PROVIDER_INFO_METADATA_TYPE);
        header.set_provider_id(16);
        header.set_name_len(name.len() as _);

        assert_parses_to_record!(
            FxtBuilder::new(header).atom(name).build(),
            RawTraceRecord::Metadata(MetadataRecord::ProviderInfo(ProviderInfoMetadataRecord {
                provider_id: 16,
                name: "hello".into(),
            })),
        );
    }

    #[test]
    fn basic_provider_section() {
        let mut header = ProviderSectionMetadataHeader::empty();
        header.set_metadata_type(PROVIDER_SECTION_METADATA_TYPE);
        header.set_provider_id(16);

        assert_parses_to_record!(
            FxtBuilder::new(header).build(),
            RawTraceRecord::Metadata(MetadataRecord::ProviderSection(
                ProviderSectionMetadataRecord { provider_id: 16 }
            )),
        );
    }

    #[test]
    fn basic_provider_event() {
        let mut header = ProviderEventMetadataHeader::empty();
        header.set_metadata_type(PROVIDER_EVENT_METADATA_TYPE);
        header.set_provider_id(16);
        header.set_event_id(PROVIDER_EVENT_BUFFER_FULL);

        assert_parses_to_record!(
            FxtBuilder::new(header).build(),
            RawTraceRecord::Metadata(MetadataRecord::ProviderEvent(ProviderEventMetadataRecord {
                provider_id: 16,
                event: ProviderEvent::BufferFull,
            })),
        );
    }

    #[test]
    fn magic_number_literal() {
        let mut buf = 0x0016547846040010u64.to_le_bytes().to_vec(); // header
        buf.extend([1, 1, 1, 1]); // trailing

        let (trailing, parsed) = RawTraceRecord::parse(&buf).unwrap();
        assert_eq!(
            parsed.parsed,
            RawTraceRecord::Metadata(MetadataRecord::TraceInfo(
                TraceInfoMetadataRecord::MagicNumber
            )),
        );
        assert_eq!(trailing, [1, 1, 1, 1]);
    }
}
