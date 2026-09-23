// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::args::{Arg, RawArg};
use crate::error::ParseWarning;
use crate::init::Ticks;
use crate::session::ResolveCtx;
use crate::string::StringRef;
use crate::thread::{ProcessKoid, ProcessRef, ThreadKoid, ThreadRef};
use crate::{
    BLOB_RECORD_TYPE, LARGE_RECORD_TYPE, ParseResult, Provider, take_n_padded, trace_header,
};
use flyweights::FlyStr;
use nom::Parser;
use nom::combinator::all_consuming;
use nom::number::complete::le_u64;

const BLOB_TYPE_DATA: u8 = 0x01;
const BLOB_TYPE_LAST_BRANCH: u8 = 0x02;
const BLOB_TYPE_PERFETTO: u8 = 0x03;

const LARGE_BLOB_WITH_METADATA_TYPE: u8 = 0;
const LARGE_BLOB_NO_METADATA_TYPE: u8 = 1;

#[derive(Clone, Debug, PartialEq)]
pub struct BlobRecord {
    pub provider: Option<Provider>,
    pub name: FlyStr,
    pub ty: BlobType,
    pub bytes: Vec<u8>,
}

impl BlobRecord {
    pub(super) fn resolve(ctx: &mut ResolveCtx, raw: RawBlobRecord<'_>) -> Self {
        Self {
            provider: ctx.current_provider(),
            name: ctx.resolve_str(raw.name),
            ty: raw.ty,
            bytes: raw.bytes.to_owned(),
        }
    }
}

#[derive(Debug, PartialEq)]
pub(super) struct RawBlobRecord<'a> {
    name: StringRef<'a>,
    ty: BlobType,
    bytes: &'a [u8],
}

impl<'a> RawBlobRecord<'a> {
    pub(super) fn parse(buf: &'a [u8]) -> ParseResult<'a, Self> {
        let (buf, header) = BlobHeader::parse(buf)?;
        let ty = BlobType::from(header.blob_format_type());
        let (rem, payload) = header.take_payload(buf)?;
        let (payload, name) = StringRef::parse(header.name_ref(), payload)?;

        // NB: most records are parsed with all_consuming for the last element to ensure we've
        // used all of the payload reported in the header, but in practice it seems that some
        // providers emit some trailing words.
        let (_should_be_empty, bytes) = take_n_padded(header.payload_len() as usize, payload)?;
        Ok((rem, Self { name, ty, bytes }))
    }
}

#[derive(Clone, Debug, PartialEq)]
#[repr(u8)]
pub enum BlobType {
    Data = BLOB_TYPE_DATA,
    LastBranch = BLOB_TYPE_LAST_BRANCH,
    Perfetto = BLOB_TYPE_PERFETTO,
    Unknown { raw: u8 },
}

impl From<u8> for BlobType {
    fn from(raw: u8) -> Self {
        match raw {
            BLOB_TYPE_DATA => BlobType::Data,
            BLOB_TYPE_LAST_BRANCH => BlobType::LastBranch,
            BLOB_TYPE_PERFETTO => BlobType::Perfetto,
            raw => BlobType::Unknown { raw },
        }
    }
}

impl Into<u8> for BlobType {
    fn into(self) -> u8 {
        match self {
            BlobType::Data => BLOB_TYPE_DATA,
            BlobType::LastBranch => BLOB_TYPE_LAST_BRANCH,
            BlobType::Perfetto => BLOB_TYPE_PERFETTO,
            BlobType::Unknown { raw } => raw,
        }
    }
}

trace_header! {
    BlobHeader (BLOB_RECORD_TYPE) {
        u16, name_ref: 16, 31;
        u16, payload_len: 32, 46;
        u8, blob_format_type: 48, 55;
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LargeBlobRecord {
    pub provider: Option<Provider>,
    pub ty: BlobType,
    pub category: FlyStr,
    pub name: FlyStr,
    pub bytes: Vec<u8>,
    pub metadata: Option<LargeBlobMetadata>,
}

impl LargeBlobRecord {
    pub(super) fn resolve(ctx: &mut ResolveCtx, raw: RawLargeBlobRecord<'_>) -> Option<Self> {
        let (bytes, metadata) = match raw.payload {
            RawLargeBlobPayload::BytesAndMetadata(bytes, metadata) => {
                (bytes.to_owned(), Some(LargeBlobMetadata::resolve(ctx, metadata)))
            }
            RawLargeBlobPayload::BytesOnly(bytes) => (bytes.to_owned(), None),
            RawLargeBlobPayload::UnknownLargeBlobType { raw_type, .. } => {
                ctx.add_warning(ParseWarning::UnknownLargeBlobType(raw_type));
                return None;
            }
        };
        Some(Self {
            provider: ctx.current_provider(),
            ty: raw.ty,
            category: ctx.resolve_str(raw.category),
            name: ctx.resolve_str(raw.name),
            bytes,
            metadata,
        })
    }
}

#[derive(Debug, PartialEq)]
pub(super) struct RawLargeBlobRecord<'a> {
    ty: BlobType,
    category: StringRef<'a>,
    name: StringRef<'a>,
    payload: RawLargeBlobPayload<'a>,
}

#[derive(Debug, PartialEq)]
enum RawLargeBlobPayload<'a> {
    BytesOnly(&'a [u8]),
    BytesAndMetadata(&'a [u8], RawLargeBlobMetadata<'a>),
    UnknownLargeBlobType { raw_type: u8, remaining_bytes: &'a [u8] },
}

impl<'a> RawLargeBlobRecord<'a> {
    pub(super) fn parse(buf: &'a [u8]) -> ParseResult<'a, Self> {
        let (buf, header) = LargeBlobHeader::parse(buf)?;
        let ty = BlobType::from(header.blob_format_type());
        let (rem, payload) = header.take_payload(buf)?;
        let (payload, format_header) =
            nom::combinator::map(le_u64, LargeBlobFormatHeader).parse(payload)?;
        let (payload, category) = StringRef::parse(format_header.category_ref(), payload)?;
        let (payload, name) = StringRef::parse(format_header.name_ref(), payload)?;
        let (payload, metadata) = match header.large_record_type() {
            LARGE_BLOB_WITH_METADATA_TYPE => {
                let (payload, ticks) = Ticks::parse(payload)?;
                let (payload, process) = ProcessRef::parse(format_header.thread_ref(), payload)?;
                let (payload, thread) = ThreadRef::parse(format_header.thread_ref(), payload)?;
                let (payload, args) = RawArg::parse_n(format_header.num_args(), payload)?;
                (payload, Some(RawLargeBlobMetadata { ticks, process, thread, args }))
            }
            LARGE_BLOB_NO_METADATA_TYPE => (payload, None),
            unknown => {
                // Without knowing which metadata type we have, we don't know where the blob size
                // and bytes payload start, so we'll put them all together.
                let payload = RawLargeBlobPayload::UnknownLargeBlobType {
                    raw_type: unknown,
                    remaining_bytes: payload,
                };
                return Ok((rem, Self { ty, category, name, payload }));
            }
        };
        let (payload, blob_size) = le_u64(payload)?;

        let (empty, bytes) =
            all_consuming(|p| take_n_padded(blob_size as usize, p)).parse(payload)?;
        assert_eq!(empty, [] as [u8; 0], "all_consuming must not return any trailing bytes");

        let payload = if let Some(metadata) = metadata {
            RawLargeBlobPayload::BytesAndMetadata(bytes, metadata)
        } else {
            RawLargeBlobPayload::BytesOnly(bytes)
        };

        Ok((rem, Self { ty, category, name, payload }))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LargeBlobMetadata {
    pub timestamp: i64,
    pub process: ProcessKoid,
    pub thread: ThreadKoid,
    pub args: Vec<Arg>,
}

impl LargeBlobMetadata {
    fn resolve(ctx: &mut ResolveCtx, raw: RawLargeBlobMetadata<'_>) -> Self {
        Self {
            timestamp: ctx.resolve_ticks(raw.ticks),
            process: ctx.resolve_process(raw.process),
            thread: ctx.resolve_thread(raw.thread),
            args: Arg::resolve_n(ctx, raw.args),
        }
    }
}

#[derive(Debug, PartialEq)]
pub(super) struct RawLargeBlobMetadata<'a> {
    ticks: Ticks,
    process: ProcessRef,
    thread: ThreadRef,
    args: Vec<RawArg<'a>>,
}

trace_header! {
    LargeBlobHeader (max_size_bit: 35) (u32) (LARGE_RECORD_TYPE) {
        u8, large_record_type: 36, 39;
        u8, blob_format_type: 40, 43;
    }
}

bitfield::bitfield! {
    struct LargeBlobFormatHeader(u64);
    impl Debug;

    u16, category_ref, set_category_ref: 15, 0;
    u16, name_ref, set_name_ref: 31, 16;

    // These are only meaningful if large_record_type includes metadata.
    u8, num_args, set_num_args: 35, 32;
    u8, thread_ref, set_thread_ref: 43, 36;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RawTraceRecord;
    use crate::fxt_builder::FxtBuilder;
    use std::num::{NonZeroU8, NonZeroU16};

    #[test]
    fn blob_name_index() {
        let payload = &[
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
        ][..];

        let mut header = BlobHeader::empty();
        header.set_name_ref(15);
        header.set_payload_len(payload.len() as u16);
        header.set_blob_format_type(BLOB_TYPE_DATA);

        assert_parses_to_record!(
            FxtBuilder::new(header).atom(payload).build(),
            RawTraceRecord::Blob(RawBlobRecord {
                name: StringRef::Index(NonZeroU16::new(15).unwrap()),
                ty: BlobType::Data,
                bytes: &payload,
            }),
        );
    }

    #[test]
    fn blob_long_blob() {
        let payload = &[0xEFu8; 1024][..];

        let mut header = BlobHeader::empty();
        header.set_name_ref(15);
        header.set_payload_len(payload.len() as u16);
        header.set_blob_format_type(BLOB_TYPE_DATA);

        assert_parses_to_record!(
            FxtBuilder::new(header).atom(payload).build(),
            RawTraceRecord::Blob(RawBlobRecord {
                name: StringRef::Index(NonZeroU16::new(15).unwrap()),
                ty: BlobType::Data,
                bytes: &payload,
            }),
        );
    }

    #[test]
    fn blob_name_inline() {
        let name = "foo_blob";
        let payload = &[
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
            25,
        ][..];

        let mut header = BlobHeader::empty();
        header.set_name_ref(fxt_layout::StringRefHeader::inline(name.len() as u16).bits());
        header.set_payload_len(payload.len() as u16);
        header.set_blob_format_type(BLOB_TYPE_PERFETTO);

        assert_parses_to_record!(
            FxtBuilder::new(header).atom(name).atom(payload).build(),
            RawTraceRecord::Blob(RawBlobRecord {
                name: StringRef::Inline(name),
                ty: BlobType::Perfetto,
                bytes: payload,
            }),
        );
    }

    #[test]
    fn large_blob_no_metadata() {
        let payload = &[
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
            25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36,
        ][..];

        let mut header = LargeBlobHeader::empty();
        header.set_blob_format_type(BLOB_TYPE_DATA);
        header.set_large_record_type(LARGE_BLOB_NO_METADATA_TYPE);

        let mut format_header = LargeBlobFormatHeader(0);
        format_header.set_category_ref(7);
        format_header.set_name_ref(8);

        assert_parses_to_record!(
            FxtBuilder::new(header)
                .atom(format_header.0.to_le_bytes())
                .atom(payload.len().to_le_bytes())
                .atom(payload)
                .build(),
            RawTraceRecord::LargeBlob(RawLargeBlobRecord {
                ty: BlobType::Data,
                category: StringRef::Index(NonZeroU16::new(7).unwrap()),
                name: StringRef::Index(NonZeroU16::new(8).unwrap()),
                payload: RawLargeBlobPayload::BytesOnly(payload),
            })
        );
    }

    #[test]
    fn large_blob_with_metadata() {
        let payload = &[
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
            25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36,
        ][..];

        let mut header = LargeBlobHeader::empty();
        header.set_blob_format_type(BLOB_TYPE_DATA);
        header.set_large_record_type(LARGE_BLOB_WITH_METADATA_TYPE);

        let mut format_header = LargeBlobFormatHeader(0);
        format_header.set_category_ref(7);
        format_header.set_name_ref(8);
        format_header.set_thread_ref(31);
        format_header.set_num_args(1);

        let mut arg_header = crate::args::U32Header::empty();
        arg_header.set_name_ref(10);
        arg_header.set_value(100);

        assert_parses_to_record!(
            FxtBuilder::new(header)
                .atom(format_header.0.to_le_bytes())
                .atom(1024u64.to_le_bytes())
                .atom(FxtBuilder::new(arg_header).build())
                .atom(payload.len().to_le_bytes())
                .atom(payload)
                .build(),
            RawTraceRecord::LargeBlob(RawLargeBlobRecord {
                ty: BlobType::Data,
                category: StringRef::Index(NonZeroU16::new(7).unwrap()),
                name: StringRef::Index(NonZeroU16::new(8).unwrap()),
                payload: RawLargeBlobPayload::BytesAndMetadata(
                    payload,
                    RawLargeBlobMetadata {
                        ticks: Ticks(1024),
                        process: ProcessRef::Index(NonZeroU8::new(31).unwrap()),
                        thread: ThreadRef::Index(NonZeroU8::new(31).unwrap()),
                        args: vec![RawArg {
                            name: StringRef::Index(NonZeroU16::new(10).unwrap()),
                            value: crate::args::RawArgValue::Unsigned32(100),
                        }],
                    },
                )
            })
        );
    }

    #[test]
    fn large_blob_large_size() {
        let payload = vec![0xEFu8; 32768];
        let mut header = LargeBlobHeader::empty();
        header.set_blob_format_type(BLOB_TYPE_DATA);
        header.set_large_record_type(LARGE_BLOB_NO_METADATA_TYPE);

        let mut format_header = LargeBlobFormatHeader(0);
        format_header.set_category_ref(7);
        format_header.set_name_ref(8);

        let mut buf = Vec::new();
        // size in words: Large Blob Header (1 word) + format_header (1 word) + payload_len (1 word) + 32768 bytes payload (4096 words) = 4099 words.
        let size_words = (8 + 8 + 8 + payload.len() as u64) / 8;
        header.set_size_words(size_words as u32);

        buf.extend_from_slice(&header.0.to_le_bytes());
        buf.extend_from_slice(&format_header.0.to_le_bytes());
        buf.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        buf.extend_from_slice(&payload);

        assert_parses_to_record!(
            buf,
            RawTraceRecord::LargeBlob(RawLargeBlobRecord {
                ty: BlobType::Data,
                category: StringRef::Index(NonZeroU16::new(7).unwrap()),
                name: StringRef::Index(NonZeroU16::new(8).unwrap()),
                payload: RawLargeBlobPayload::BytesOnly(&payload[..]),
            })
        );
    }
}
