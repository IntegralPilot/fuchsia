// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::args::{Arg, RawArg};
use crate::session::ResolveCtx;
use crate::string::{RawByteStringRef, StringRef};
use crate::thread::{ProcessKoid, ProcessRef};
use crate::{
    KERNEL_OBJ_RECORD_TYPE, ParseResult, Provider, USERSPACE_OBJ_RECORD_TYPE, trace_header,
};
use flyweights::{FlyByteStr, FlyStr};
use nom::Parser;
use nom::combinator::all_consuming;
use nom::number::complete::le_u64;

#[derive(Clone, Debug, PartialEq)]
pub struct UserspaceObjRecord {
    pub provider: Option<Provider>,
    pub pointer: u64,
    pub process: ProcessKoid,
    pub name: FlyStr,
    pub args: Vec<Arg>,
}

impl UserspaceObjRecord {
    pub(super) fn resolve(ctx: &mut ResolveCtx, raw: RawUserspaceObjRecord<'_>) -> Self {
        Self {
            provider: ctx.current_provider(),
            pointer: raw.pointer,
            process: ctx.resolve_process(raw.process),
            name: ctx.resolve_str(raw.name),
            args: Arg::resolve_n(ctx, raw.args),
        }
    }
}

#[derive(Debug, PartialEq)]
pub(super) struct RawUserspaceObjRecord<'a> {
    pointer: u64,
    process: ProcessRef,
    name: StringRef<'a>,
    args: Vec<RawArg<'a>>,
}

impl<'a> RawUserspaceObjRecord<'a> {
    pub(super) fn parse(buf: &'a [u8]) -> ParseResult<'a, Self> {
        let (buf, header) = UserspaceObjHeader::parse(buf)?;
        let (rem, payload) = header.take_payload(buf)?;
        let (payload, pointer) = le_u64(payload)?;
        let (payload, process) = ProcessRef::parse(header.process_ref(), payload)?;
        let (payload, name) = StringRef::parse(header.name_ref(), payload)?;
        let (empty, args) =
            all_consuming(|p| RawArg::parse_n(header.num_args(), p)).parse(payload)?;
        assert!(empty.is_empty(), "all_consuming must not return any remaining buffer");
        Ok((rem, Self { pointer, process, name, args }))
    }
}

trace_header! {
    UserspaceObjHeader (USERSPACE_OBJ_RECORD_TYPE) {
        u8, process_ref: 16, 23;
        u16, name_ref: 24, 39;
        u8, num_args: 40, 43;
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct KernelObjRecord {
    pub provider: Option<Provider>,
    pub koid: u64,
    pub ty: KernelObjType,
    pub name: FlyByteStr,
    pub args: Vec<Arg>,
}

impl KernelObjRecord {
    /// By convention kobj records have a "process" arg which lists the process koid.
    pub fn process(&self) -> Option<ProcessKoid> {
        for arg in &self.args {
            if arg.name == "process" {
                if let Some(k) = arg.value.kernel_obj() {
                    return Some(ProcessKoid(k));
                }
            }
        }
        None
    }

    pub(super) fn resolve(ctx: &mut ResolveCtx, raw: RawKernelObjRecord<'_>) -> Self {
        Self {
            provider: ctx.current_provider(),
            koid: raw.koid,
            ty: raw.ty,
            name: ctx.resolve_bstr(raw.name),
            args: Arg::resolve_n(ctx, raw.args),
        }
    }
}

#[derive(Debug, PartialEq)]
pub(crate) struct RawKernelObjRecord<'a> {
    koid: u64,
    ty: KernelObjType,
    name: RawByteStringRef<'a>,
    args: Vec<RawArg<'a>>,
}

impl<'a> RawKernelObjRecord<'a> {
    pub(super) fn parse(buf: &'a [u8]) -> ParseResult<'a, Self> {
        let (buf, header) = KernelObjHeader::parse(buf)?;
        let ty = KernelObjType::from(header.kernel_obj_type());
        let (rem, payload) = header.take_payload(buf)?;
        let (payload, koid) = le_u64(payload)?;
        let (payload, name) = RawByteStringRef::parse(header.name_ref(), payload)?;
        let (empty, args) =
            all_consuming(|p| RawArg::parse_n(header.num_args(), p)).parse(payload)?;
        assert!(empty.is_empty(), "all_consuming must not return any remaining buffer");
        Ok((rem, Self { koid, name, args, ty }))
    }
}

trace_header! {
    KernelObjHeader (KERNEL_OBJ_RECORD_TYPE) {
        u32, kernel_obj_type: 16, 23;
        u16, name_ref: 24, 39;
        u8, num_args: 40, 43;
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum KernelObjType {
    None,
    Process,
    Thread,
    Vmo,
    Channel,
    Event,
    Port,
    Interrupt,
    PciDevice,
    DebugLog,
    Socket,
    Resource,
    EventPair,
    Job,
    Vmar,
    Fifo,
    Guest,
    Vcpu,
    Timer,
    Iommu,
    Bti,
    Profile,
    Pmt,
    SuspendToken,
    Pager,
    Exception,
    Clock,
    Stream,
    Msi,
    Iob,
    Unknown(u32),
}

impl From<u32> for KernelObjType {
    fn from(raw: u32) -> Self {
        use zx_types::*;
        match raw {
            ZX_OBJ_TYPE_NONE => Self::None,
            ZX_OBJ_TYPE_PROCESS => Self::Process,
            ZX_OBJ_TYPE_THREAD => Self::Thread,
            ZX_OBJ_TYPE_VMO => Self::Vmo,
            ZX_OBJ_TYPE_CHANNEL => Self::Channel,
            ZX_OBJ_TYPE_EVENT => Self::Event,
            ZX_OBJ_TYPE_PORT => Self::Port,
            ZX_OBJ_TYPE_INTERRUPT => Self::Interrupt,
            ZX_OBJ_TYPE_PCI_DEVICE => Self::PciDevice,
            ZX_OBJ_TYPE_DEBUGLOG => Self::DebugLog,
            ZX_OBJ_TYPE_SOCKET => Self::Socket,
            ZX_OBJ_TYPE_RESOURCE => Self::Resource,
            ZX_OBJ_TYPE_EVENTPAIR => Self::EventPair,
            ZX_OBJ_TYPE_JOB => Self::Job,
            ZX_OBJ_TYPE_VMAR => Self::Vmar,
            ZX_OBJ_TYPE_FIFO => Self::Fifo,
            ZX_OBJ_TYPE_GUEST => Self::Guest,
            ZX_OBJ_TYPE_VCPU => Self::Vcpu,
            ZX_OBJ_TYPE_TIMER => Self::Timer,
            ZX_OBJ_TYPE_IOMMU => Self::Iommu,
            ZX_OBJ_TYPE_BTI => Self::Bti,
            ZX_OBJ_TYPE_PROFILE => Self::Profile,
            ZX_OBJ_TYPE_PMT => Self::Pmt,
            ZX_OBJ_TYPE_SUSPEND_TOKEN => Self::SuspendToken,
            ZX_OBJ_TYPE_PAGER => Self::Pager,
            ZX_OBJ_TYPE_EXCEPTION => Self::Exception,
            ZX_OBJ_TYPE_CLOCK => Self::Clock,
            ZX_OBJ_TYPE_STREAM => Self::Stream,
            ZX_OBJ_TYPE_MSI => Self::Msi,
            ZX_OBJ_TYPE_IOB => Self::Iob,
            unknown => Self::Unknown(unknown),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RawTraceRecord;
    use crate::args::RawArgValue;
    use crate::fxt_builder::FxtBuilder;
    use std::num::{NonZeroU8, NonZeroU16};

    #[test]
    fn userspace_obj_no_args() {
        let mut header = UserspaceObjHeader::empty();
        header.set_process_ref(12);
        header.set_name_ref(18);
        header.set_num_args(0);

        assert_parses_to_record!(
            FxtBuilder::new(header).atom(1024u64.to_le_bytes()).build(),
            RawTraceRecord::UserspaceObj(RawUserspaceObjRecord {
                process: ProcessRef::Index(NonZeroU8::new(12).unwrap()),
                name: StringRef::Index(NonZeroU16::new(18).unwrap()),
                pointer: 1024,
                args: vec![],
            },),
        );
    }

    #[test]
    fn userspace_obj_with_args() {
        let mut header = UserspaceObjHeader::empty();
        header.set_process_ref(12);
        header.set_name_ref(18);
        header.set_num_args(3);

        let mut first_arg_header = crate::args::BaseArgHeader::empty();
        let first_arg_value = 1007.893f64;
        first_arg_header.set_name_ref(10);
        first_arg_header.set_raw_type(crate::args::F64_ARG_TYPE);

        let second_arg_value = "123-456-7890";
        let mut second_arg_header = crate::args::StringHeader::empty();
        second_arg_header.set_name_ref(11);
        second_arg_header.set_value_ref(
            fxt_layout::StringRefHeader::inline(second_arg_value.len() as u16).bits(),
        );

        let third_arg_name = "hello";
        let mut third_arg_header = crate::args::U32Header::empty();
        third_arg_header
            .set_name_ref(fxt_layout::StringRefHeader::inline(third_arg_name.len() as u16).bits());
        third_arg_header.set_value(23);

        assert_parses_to_record!(
            FxtBuilder::new(header)
                .atom(2048u64.to_le_bytes())
                .atom(FxtBuilder::new(first_arg_header).atom(first_arg_value.to_le_bytes()).build())
                .atom(FxtBuilder::new(second_arg_header).atom(second_arg_value).build())
                .atom(FxtBuilder::new(third_arg_header).atom(third_arg_name).build())
                .build(),
            RawTraceRecord::UserspaceObj(RawUserspaceObjRecord {
                process: ProcessRef::Index(NonZeroU8::new(12).unwrap()),
                name: StringRef::Index(NonZeroU16::new(18).unwrap()),
                pointer: 2048,
                args: vec![
                    RawArg {
                        name: StringRef::Index(NonZeroU16::new(10).unwrap()),
                        value: RawArgValue::Double(first_arg_value),
                    },
                    RawArg {
                        name: StringRef::Index(NonZeroU16::new(11).unwrap()),
                        value: RawArgValue::String(StringRef::Inline(second_arg_value)),
                    },
                    RawArg {
                        name: StringRef::Inline(third_arg_name),
                        value: RawArgValue::Unsigned32(23),
                    },
                ],
            })
        );
    }

    #[test]
    fn kernel_obj_no_args() {
        let mut header = KernelObjHeader::empty();
        header.set_kernel_obj_type(zx_types::ZX_OBJ_TYPE_CHANNEL);
        header.set_name_ref(19);
        header.set_num_args(0);

        assert_parses_to_record!(
            FxtBuilder::new(header).atom(64u64.to_le_bytes()).build(),
            RawTraceRecord::KernelObj(RawKernelObjRecord {
                koid: 64,
                ty: KernelObjType::Channel,
                name: RawByteStringRef::Index(NonZeroU16::new(19).unwrap()),
                args: vec![],
            },),
        );
    }

    #[test]
    fn kernel_obj_with_args() {
        let mut header = KernelObjHeader::empty();
        header.set_kernel_obj_type(zx_types::ZX_OBJ_TYPE_FIFO);
        header.set_name_ref(91);
        header.set_num_args(3);

        let koid = 128u64;

        let mut first_arg_header = crate::args::BaseArgHeader::empty();
        first_arg_header.set_name_ref(10);
        first_arg_header.set_raw_type(crate::args::F64_ARG_TYPE);

        let second_arg_value = "123-456-7890";
        let mut second_arg_header = crate::args::StringHeader::empty();
        second_arg_header.set_name_ref(11);
        second_arg_header.set_value_ref(
            fxt_layout::StringRefHeader::inline(second_arg_value.len() as u16).bits(),
        );

        let third_arg_name = "hello";
        let mut third_arg_header = crate::args::U32Header::empty();
        third_arg_header
            .set_name_ref(fxt_layout::StringRefHeader::inline(third_arg_name.len() as u16).bits());
        third_arg_header.set_value(23);

        assert_parses_to_record!(
            FxtBuilder::new(header)
                .atom(koid.to_le_bytes())
                .atom(FxtBuilder::new(first_arg_header).atom(1007.893f64.to_le_bytes()).build())
                .atom(FxtBuilder::new(second_arg_header).atom(second_arg_value).build())
                .atom(FxtBuilder::new(third_arg_header).atom(third_arg_name).build())
                .build(),
            RawTraceRecord::KernelObj(RawKernelObjRecord {
                koid,
                ty: KernelObjType::Fifo,
                name: RawByteStringRef::Index(NonZeroU16::new(91).unwrap()),
                args: vec![
                    RawArg {
                        name: StringRef::Index(NonZeroU16::new(10).unwrap()),
                        value: RawArgValue::Double(1007.893)
                    },
                    RawArg {
                        name: StringRef::Index(NonZeroU16::new(11).unwrap()),
                        value: RawArgValue::String(StringRef::Inline(second_arg_value)),
                    },
                    RawArg { name: StringRef::Inline("hello"), value: RawArgValue::Unsigned32(23) },
                ],
            }),
        );
    }
}
