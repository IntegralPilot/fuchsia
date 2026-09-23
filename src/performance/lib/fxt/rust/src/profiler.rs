// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::init::Ticks;
use crate::session::ResolveCtx;
use crate::thread::{ProcessKoid, ProcessRef, ThreadKoid, ThreadRef};
use crate::{
    PROFILER_RECORD_TYPE, ParseError, ParseResult, ParseWarning, take_n_padded, trace_header,
};
use flyweights::FlyStr;
use nom::Parser;
use nom::combinator::all_consuming;
use nom::number::complete::le_u64;

const MODULE_RECORD_TYPE: u8 = 0;
const MAPPING_RECORD_TYPE: u8 = 1;
const BACKTRACE_RECORD_TYPE: u8 = 2;

macro_rules! profiler_header {
    ($name:ident $(($profiler_ty:expr))? { $($record_specific:tt)* }) => {
        trace_header! {
            $name (PROFILER_RECORD_TYPE) {
                $($record_specific)*
                u8, profiler_sub_type: 16, 19;
                u8, thread_ref: 20, 27;
            } => |_h: &$name| {
                $(if _h.profiler_sub_type() != $profiler_ty {
                    return Err(ParseError::WrongType {
                        context: stringify!($name),
                        expected: $profiler_ty,
                        observed: _h.profiler_sub_type(),
                    });
                })?
                Ok(())
            }
        }
    };
}

profiler_header! {BaseProfilerRecordHeader{}}

profiler_header! {
    ModuleRecordHeader(MODULE_RECORD_TYPE) {
        // [28 .. 43]: module id (16 bit)
        u16, module_id: 28, 43;
        // [44 .. 51]: the length of name in bytes
        u8, name_length: 44, 51;
        // [52 .. 59]: the length of build id in bytes
        u8, build_id_length: 52, 59;
    }
}

profiler_header! {
    MmapRecordHeader(MAPPING_RECORD_TYPE) {
        // [28 .. 43]: module id (16 bit)
        u16, module_id: 28, 43;
        // [44 .. 46]: flags
        u8, flags: 44, 46;
    }
}

profiler_header! {
    BacktraceRecordHeader(BACKTRACE_RECORD_TYPE) {
        // [28 .. 35]: the number of backtrace record
        u8, num_records: 28, 35;
        // [36 .. 63]: flags
        u32, flags: 36, 63;
    }
}

#[derive(Debug, PartialEq)]
pub(super) enum RawProfilerRecordType<'a> {
    RawModuleRecord(RawModuleRecord<'a>),
    RawMappingRecord(RawMappingRecord),
    RawBacktraceRecord(RawBacktraceRecord),
    Unknown { raw_type: u8 },
}

#[derive(Debug, PartialEq)]
pub(super) struct RawModuleRecord<'a> {
    ticks: Ticks,
    process: ProcessRef,
    thread: ThreadRef,
    module_id: u16,
    name: String,
    build_id: &'a [u8],
}

impl<'a> RawModuleRecord<'a> {
    pub(super) fn parse(buf: &'a [u8]) -> ParseResult<'a, Self> {
        let (buf, header) = ModuleRecordHeader::parse(buf)?;
        let (rem, payload) = header.take_payload(buf)?;
        let (payload, ticks) = Ticks::parse(payload)?;
        let (payload, process) = ProcessRef::parse(header.thread_ref(), payload)?;
        let (payload, thread) = ThreadRef::parse(header.thread_ref(), payload)?;
        let (payload, name) = parse_padded_module_name(header.name_length() as usize, payload)?;
        let (empty, build_id) =
            all_consuming(|p| take_n_padded(header.build_id_length() as usize, p))
                .parse(payload)?;
        assert!(empty.is_empty(), "all_consuming must not return any remaining buffer");
        Ok((rem, Self { ticks, process, thread, module_id: header.module_id(), name, build_id }))
    }
}

pub(crate) fn parse_padded_module_name(unpadded_len: usize, buf: &[u8]) -> ParseResult<'_, String> {
    let (rem, bytes) = take_n_padded(unpadded_len, buf)?;
    Ok((rem, String::from_utf8_lossy(bytes).into_owned()))
}

#[derive(Debug, PartialEq)]
pub(super) struct RawMappingRecord {
    ticks: Ticks,
    process: ProcessRef,
    thread: ThreadRef,
    module_id: u16,
    start_addr: u64,
    range: u64,
    vaddr: u64,
    flags: u8,
}

impl RawMappingRecord {
    pub(super) fn parse(buf: &[u8]) -> ParseResult<'_, Self> {
        let (buf, header) = MmapRecordHeader::parse(buf)?;
        let (rem, payload) = header.take_payload(buf)?;
        let (payload, ticks) = Ticks::parse(payload)?;
        let (payload, process) = ProcessRef::parse(header.thread_ref(), payload)?;
        let (payload, thread) = ThreadRef::parse(header.thread_ref(), payload)?;
        let (payload, start_addr) = le_u64(payload)?;
        let (payload, range) = le_u64(payload)?;
        let (empty, vaddr) = le_u64(payload)?;
        assert!(empty.is_empty(), "after vaddr must not return any remaining buffer");
        Ok((
            rem,
            Self {
                ticks,
                process,
                thread,
                module_id: header.module_id(),
                start_addr,
                range,
                vaddr,
                flags: header.flags(),
            },
        ))
    }
}

#[derive(Debug, PartialEq)]
pub(super) struct RawBacktraceRecord {
    ticks: Ticks,
    process: ProcessRef,
    thread: ThreadRef,
    num_records: u8,
    data: Vec<u64>,
}

impl RawBacktraceRecord {
    pub(super) fn parse(buf: &[u8]) -> ParseResult<'_, Self> {
        let (buf, header) = BacktraceRecordHeader::parse(buf)?;
        let (rem, payload) = header.take_payload(buf)?;
        let (payload, ticks) = Ticks::parse(payload)?;
        let (payload, process) = ProcessRef::parse(header.thread_ref(), payload)?;
        let (payload, thread) = ThreadRef::parse(header.thread_ref(), payload)?;
        let (empty, data) = all_consuming(nom::multi::count(le_u64, header.num_records() as usize))
            .parse(payload)?;
        assert!(empty.is_empty(), "all_consuming must not return any remaining buffer");
        Ok((rem, Self { ticks, process, thread, num_records: header.num_records(), data }))
    }
}

impl<'a> RawProfilerRecordType<'a> {
    pub(super) fn parse(buf: &'a [u8]) -> ParseResult<'a, Self> {
        use nom::combinator::map;
        match BaseProfilerRecordHeader::parse(buf)?.1.profiler_sub_type() {
            MODULE_RECORD_TYPE => {
                map(RawModuleRecord::parse, |module| Self::RawModuleRecord(module)).parse(buf)
            }
            MAPPING_RECORD_TYPE => {
                map(RawMappingRecord::parse, |mapping| Self::RawMappingRecord(mapping)).parse(buf)
            }
            BACKTRACE_RECORD_TYPE => {
                map(RawBacktraceRecord::parse, |backtrace| Self::RawBacktraceRecord(backtrace))
                    .parse(buf)
            }
            unknown => Ok((buf, Self::Unknown { raw_type: unknown })),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ProfilerRecord {
    Module(ModuleRecord),
    Mapping(MappingRecord),
    Backtrace(BacktraceRecord),
}

impl ProfilerRecord {
    pub(super) fn resolve<'a>(
        ctx: &mut ResolveCtx,
        raw: RawProfilerRecordType<'a>,
    ) -> Option<Self> {
        match raw {
            RawProfilerRecordType::RawModuleRecord(raw) => {
                Some(Self::Module(ModuleRecord::resolve(ctx, raw)))
            }
            RawProfilerRecordType::RawMappingRecord(raw) => {
                Some(Self::Mapping(MappingRecord::resolve(ctx, raw)))
            }
            RawProfilerRecordType::RawBacktraceRecord(raw) => {
                Some(Self::Backtrace(BacktraceRecord::resolve(ctx, raw)))
            }
            RawProfilerRecordType::Unknown { raw_type } => {
                ctx.add_warning(ParseWarning::UnknownProfilerRecordType(raw_type));
                None
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModuleRecord {
    pub timestamp: i64,
    pub process: ProcessKoid,
    pub thread: ThreadKoid,
    pub module_id: u16,
    pub name: FlyStr,
    pub build_id: Vec<u8>,
}

impl ModuleRecord {
    pub(super) fn resolve<'a>(ctx: &mut ResolveCtx, raw: RawModuleRecord<'a>) -> Self {
        Self {
            timestamp: ctx.resolve_ticks(raw.ticks),
            process: ctx.resolve_process(raw.process),
            thread: ctx.resolve_thread(raw.thread),
            module_id: raw.module_id,
            name: raw.name.into(),
            build_id: raw.build_id.to_vec(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MappingRecord {
    pub timestamp: i64,
    pub process: ProcessKoid,
    pub thread: ThreadKoid,
    pub module_id: u16,
    pub start_addr: u64,
    pub range: u64,
    pub vaddr: u64,
    pub flags: u8,
}

impl MappingRecord {
    pub(super) fn resolve(ctx: &mut ResolveCtx, raw: RawMappingRecord) -> Self {
        Self {
            timestamp: ctx.resolve_ticks(raw.ticks),
            process: ctx.resolve_process(raw.process),
            thread: ctx.resolve_thread(raw.thread),
            module_id: raw.module_id,
            start_addr: raw.start_addr,
            range: raw.range,
            vaddr: raw.vaddr,
            flags: raw.flags,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BacktraceRecord {
    pub timestamp: i64,
    pub process: ProcessKoid,
    pub thread: ThreadKoid,
    pub num_records: u8,
    pub data: Vec<u64>,
}

impl BacktraceRecord {
    pub(super) fn resolve(ctx: &mut ResolveCtx, raw: RawBacktraceRecord) -> Self {
        Self {
            timestamp: ctx.resolve_ticks(raw.ticks),
            process: ctx.resolve_process(raw.process),
            thread: ctx.resolve_thread(raw.thread),
            num_records: raw.num_records,
            data: raw.data,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_module_record_from_hardcoded_bytes() {
        let mut buffer = vec![];

        let record_type: u64 = 10;
        let sub_type: u64 = 0; // sub-type 0
        let thread_ref: u64 = 0; // Inline process koid
        let module_id: u64 = 99;
        let name_len: u64 = 14; // "test_module.so"
        let build_id_len: u64 = 20;
        let size_words: u64 = 9; // (Header + 64 payload bytes) / 8

        let header_val: u64 = (build_id_len << 52)
            | (name_len << 44)
            | (module_id << 28)
            | (thread_ref << 20)
            | (sub_type << 16)
            | (size_words << 4)
            | record_type;

        buffer.extend_from_slice(&header_val.to_le_bytes());
        buffer.extend_from_slice(&[0x88, 0x13, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // Ticks(5000)
        buffer.extend_from_slice(&[0x7b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // ProcessKoid(123)
        buffer.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // ThreadKoid(0)
        buffer.extend_from_slice(&[
            // "test_module.so" (14 bytes) + 2 padding
            0x74, 0x65, 0x73, 0x74, 0x5f, 0x6d, 0x6f, 0x64, 0x75, 0x6c, 0x65, 0x2e, 0x73, 0x6f,
            0x00, 0x00,
        ]);
        buffer.extend_from_slice(&[
            // Build ID (20 bytes): 68920bd06a8149b58004cea8a0dd260b05e87d57
            0x68, 0x92, 0x0b, 0xd0, 0x6a, 0x81, 0x49, 0xb5, 0x80, 0x04, 0xce, 0xa8, 0xa0, 0xdd,
            0x26, 0x0b, 0x05, 0xe8, 0x7d, 0x57,
        ]);
        buffer.extend_from_slice(&[0; 4]); // Build ID padding to 8-byte alignment

        let (empty, raw_record) = RawProfilerRecordType::parse(&buffer).unwrap();
        assert!(empty.is_empty(), "Buffer should be fully consumed");

        let mut ctx = ResolveCtx::new();
        let resolved = ProfilerRecord::resolve(&mut ctx, raw_record).unwrap();

        let expected = ProfilerRecord::Module(ModuleRecord {
            timestamp: ctx.resolve_ticks(Ticks(5000)),
            process: ProcessKoid::from(123u64),
            thread: ThreadKoid::from(0u64),
            module_id: 99,
            name: "test_module.so".into(),
            build_id: vec![
                0x68, 0x92, 0x0b, 0xd0, 0x6a, 0x81, 0x49, 0xb5, 0x80, 0x04, 0xce, 0xa8, 0xa0, 0xdd,
                0x26, 0x0b, 0x05, 0xe8, 0x7d, 0x57,
            ],
        });
        assert_eq!(resolved, expected);
    }

    #[test]
    fn test_parse_mapping_record_from_hardcoded_bytes() {
        let mut buffer = vec![];

        let record_type: u64 = 10;
        let sub_type: u64 = 1; // sub-type 1
        let thread_ref: u64 = 0; // Inline process koid
        let module_id: u64 = 101;
        let flags: u64 = 1;
        let size_words: u64 = 7; // (Header + 48 payload bytes) / 8

        let header_val: u64 = (flags << 44)
            | (module_id << 28)
            | (thread_ref << 20)
            | (sub_type << 16)
            | (size_words << 4)
            | record_type;

        buffer.extend_from_slice(&header_val.to_le_bytes());
        buffer.extend_from_slice(&[0x70, 0x17, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // Ticks(6000)
        buffer.extend_from_slice(&[0xc8, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // ProcessKoid(456)
        buffer.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // ThreadKoid(0)
        buffer.extend_from_slice(&[0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // Start Address(0x1000)
        buffer.extend_from_slice(&[0x00, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // Addr Range(0x2000)
        buffer.extend_from_slice(&[0x00, 0x30, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // Vaddr(0x3000)

        let (empty, raw_record) = RawProfilerRecordType::parse(&buffer).unwrap();
        assert!(empty.is_empty(), "Buffer should be fully consumed");

        let mut ctx = ResolveCtx::new();
        let resolved = ProfilerRecord::resolve(&mut ctx, raw_record).unwrap();

        let expected = ProfilerRecord::Mapping(MappingRecord {
            timestamp: ctx.resolve_ticks(Ticks(6000)),
            process: ProcessKoid::from(456u64),
            thread: ThreadKoid::from(0u64),
            module_id: 101,
            start_addr: 0x1000,
            range: 0x2000,
            vaddr: 0x3000,
            flags: 1,
        });
        assert_eq!(resolved, expected);
    }

    #[test]
    fn test_parse_backtrace_record_from_hardcoded_bytes() {
        let mut buffer = vec![];

        let record_type: u64 = 10;
        let sub_type: u64 = 2; // sub-type 2
        let thread_ref: u64 = 0; // Inline process and thread koid
        let num_records: u64 = 2;
        let flags: u64 = 2;
        let size_words: u64 = 6; // (Header + 40 payload bytes) / 8

        let header_val: u64 = (flags << 36)
            | (num_records << 28)
            | (thread_ref << 20)
            | (sub_type << 16)
            | (size_words << 4)
            | record_type;

        buffer.extend_from_slice(&header_val.to_le_bytes());
        buffer.extend_from_slice(&[0x58, 0x1B, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // Ticks(7000)
        buffer.extend_from_slice(&[0x15, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // ProcessKoid(789)
        buffer.extend_from_slice(&[0xDB, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // ThreadKoid(987)
        buffer.extend_from_slice(&[0x99, 0x14, 0x4b, 0x96, 0x2a, 0x40, 0x00, 0x00]); // Backtrace data[0] (0x402a964b1499)
        buffer.extend_from_slice(&[0x3b, 0x29, 0xe0, 0x4a, 0x00, 0x00, 0x00, 0x00]); // Backtrace data[1] (0x4ae0293b)

        let (empty, raw_record) = RawProfilerRecordType::parse(&buffer).unwrap();
        assert!(empty.is_empty(), "Buffer should be fully consumed");

        let mut ctx = ResolveCtx::new();
        let resolved = ProfilerRecord::resolve(&mut ctx, raw_record).unwrap();

        let expected = ProfilerRecord::Backtrace(BacktraceRecord {
            timestamp: ctx.resolve_ticks(Ticks(7000)),
            process: ProcessKoid::from(789u64),
            thread: ThreadKoid::from(987u64),
            num_records: 2,
            data: vec![0x402a964b1499, 0x4ae0293b],
        });
        assert_eq!(resolved, expected);
    }
}
