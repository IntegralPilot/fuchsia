// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Typed bitfields and enums for the Fuchsia Trace Format (FXT).
//!
//! Defined according to `docs/reference/tracing/trace-format.md` and matching
//! C++ definitions in `<lib/fxt/record_types.h>` and `<lib/fxt/fields.h>`.

#![no_std]

use bitrs::{bitfield_repr, layout};

/// The four byte value present in a magic number record.
pub const MAGIC_VALUE: u32 = 0x16547846;

/// Enumerates all known record types.
#[bitfield_repr(u8)]
pub enum RecordType {
    Metadata = 0,
    Initialization = 1,
    String = 2,
    Thread = 3,
    Event = 4,
    Blob = 5,
    UserspaceObject = 6,
    KernelObject = 7,
    Scheduler = 8,
    Log = 9,
    Profiler = 10,
    /// The kLargeRecord uses a 32-bit size field.
    LargeBlob = 15,
}

/// ProfilerRecordType enumerates all known profiler record sub-types.
#[bitfield_repr(u8)]
pub enum ProfilerRecordType {
    Module = 0,
    Mmap = 1,
    Backtrace = 2,
}

#[bitfield_repr(u8)]
pub enum LargeRecordType {
    Blob = 0,
}

/// MetadataType enumerates all known trace metadata types.
#[bitfield_repr(u8)]
pub enum MetadataType {
    ProviderInfo = 1,
    ProviderSection = 2,
    ProviderEvent = 3,
    TraceInfo = 4,
}

/// Enumerates all provider events.
#[bitfield_repr(u8)]
pub enum ProviderEventType {
    BufferOverflow = 0,
}

/// Enumerates all known trace info types.
#[bitfield_repr(u8)]
pub enum TraceInfoType {
    MagicNumber = 0,
}

/// Whether a String/Thread Ref is inline or referenced as an id.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum RefType {
    Inline,
    Id,
}

/// Enumerates all known argument types.
#[bitfield_repr(u8)]
pub enum ArgumentType {
    Null = 0,
    Int32 = 1,
    Uint32 = 2,
    Int64 = 3,
    Uint64 = 4,
    Double = 5,
    String = 6,
    Pointer = 7,
    Koid = 8,
    Bool = 9,
    Blob = 10,
}

/// EventType enumerates all known trace event types.
#[bitfield_repr(u8)]
pub enum EventType {
    Instant = 0,
    Counter = 1,
    DurationBegin = 2,
    DurationEnd = 3,
    DurationComplete = 4,
    AsyncBegin = 5,
    AsyncInstant = 6,
    AsyncEnd = 7,
    FlowBegin = 8,
    FlowStep = 9,
    FlowEnd = 10,
}

/// SchedulerEventType enumerates all known scheduler event types.
#[bitfield_repr(u8)]
pub enum SchedulerEventType {
    LegacyContextSwitch = 0,
    ContextSwitch = 1,
    ThreadWakeup = 2,
}

/// Specifies the scope of instant events.
#[bitfield_repr(u8)]
pub enum EventScope {
    Thread = 0,
    Process = 1,
    Global = 2,
}

/// Trace provider id in a trace session.
pub type ProviderId = u32;

#[bitfield_repr(u8)]
pub enum BlobType {
    Data = 1,
    LastBranch = 2,
    Perfetto = 3,
}

#[bitfield_repr(u8)]
pub enum LargeBlobFormat {
    Metadata = 0,
    NoMetadata = 1,
}

layout!({
    /// Header word for a standard FXT record.
    pub struct RecordHeader(u64);
    {
        let record_size @ 15..4;
        let record_type @ 3..0: RecordType;
    }
});

layout!({
    /// Header word for a large FXT record (RecordType = 15).
    pub struct LargeRecordHeader(u64);
    {
        let large_type @ 39..36;
        let record_size @ 35..4;
        let record_type @ 3..0: RecordType = RecordType::LargeBlob;
    }
});

layout!({
    /// 16-bit String Reference header entry.
    pub struct StringRefHeader(u16);
    {
        let is_inline @ 15;
        let id_or_len @ 14..0;
    }
});

impl StringRefHeader {
    /// Constructs a `StringRefHeader` for an indexed string ref.
    pub fn indexed(id: u16) -> Self {
        let mut hdr = Self::new();
        hdr.set_id_or_len(id);
        hdr
    }

    /// Constructs a `StringRefHeader` for an inline string ref.
    pub fn inline(len: u16) -> Self {
        let mut hdr = Self::new();
        hdr.set_is_inline(true).set_id_or_len(len);
        hdr
    }
}

layout!({
    /// 8-bit Thread Reference header entry.
    pub struct ThreadRefHeader(u8);
    {
        let is_inline @ 7;
        let id_or_len @ 6..0;
    }
});

impl ThreadRefHeader {
    /// Constructs a `ThreadRefHeader` for an indexed thread ref.
    pub fn indexed(id: u8) -> Self {
        let mut hdr = Self::new();
        hdr.set_id_or_len(id);
        hdr
    }

    /// Constructs a `ThreadRefHeader` for an inline thread ref.
    pub fn inline(len: u8) -> Self {
        let mut hdr = Self::new();
        hdr.set_is_inline(true).set_id_or_len(len);
        hdr
    }
}

layout!({
    /// Header word for an FXT Argument.
    pub struct ArgumentHeader(u64);
    {
        let value_bits @ 63..32;
        let name_ref @ 31..16;
        let size_words @ 15..4;
        let arg_type @ 3..0: ArgumentType;
    }
});

impl ArgumentHeader {
    /// Constructs an `ArgumentHeader` for an argument with the given name reference, size in words, and argument type.
    pub fn for_argument(name_ref: u16, size_words: u16, arg_type: ArgumentType) -> Self {
        let mut hdr = Self::new();
        hdr.set_arg_type(arg_type).set_size_words(size_words).set_name_ref(name_ref);
        hdr
    }
}

layout!({
    /// Header word for an FXT Kernel Object record (RecordType = 7).
    pub struct KernelObjectRecordHeader(u64);
    {
        let arg_count @ 43..40;
        let name_ref @ 39..24;
        let obj_type @ 23..16;
        let record_size @ 15..4;
        let record_type @ 3..0: RecordType = RecordType::KernelObject;
    }
});

layout!({
    /// Header word for an FXT Userspace Object record (RecordType = 6).
    pub struct UserspaceObjectRecordHeader(u64);
    {
        let arg_count @ 43..40;
        let name_ref @ 39..24;
        let process_thread_ref @ 23..16;
        let record_size @ 15..4;
        let record_type @ 3..0: RecordType = RecordType::UserspaceObject;
    }
});

layout!({
    /// Header word for an FXT Event record (RecordType = 4).
    pub struct EventRecordHeader(u64);
    {
        let name_ref @ 63..48;
        let category_ref @ 47..32;
        let thread_ref @ 31..24;
        let arg_count @ 23..20;
        let event_type @ 19..16: EventType;
        let record_size @ 15..4;
        let record_type @ 3..0: RecordType = RecordType::Event;
    }
});

layout!({
    /// Header word for an FXT String record (RecordType = 2).
    pub struct StringRecordHeader(u64);
    {
        let string_len @ 46..32;
        let string_index @ 30..16;
        let record_size @ 15..4;
        let record_type @ 3..0: RecordType = RecordType::String;
    }
});

layout!({
    /// Header word for an FXT Thread record (RecordType = 3).
    pub struct ThreadRecordHeader(u64);
    {
        let thread_index @ 23..16;
        let record_size @ 15..4;
        let record_type @ 3..0: RecordType = RecordType::Thread;
    }
});

layout!({
    /// Header word for an FXT Blob record (RecordType = 5).
    pub struct BlobRecordHeader(u64);
    {
        let blob_type @ 55..48: BlobType;
        let blob_size @ 46..32;
        let name_ref @ 31..16;
        let record_size @ 15..4;
        let record_type @ 3..0: RecordType = RecordType::Blob;
    }
});

layout!({
    /// Header word for an FXT Log record (RecordType = 9).
    pub struct LogRecordHeader(u64);
    {
        let thread_ref @ 39..32;
        let log_message_len @ 30..16;
        let record_size @ 15..4;
        let record_type @ 3..0: RecordType = RecordType::Log;
    }
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_header() {
        let mut rec = RecordHeader::new();
        rec.set_record_size(12).set_record_type(RecordType::Event);
        assert_eq!(rec.record_type(), RecordType::Event);
        assert_eq!(rec.record_size(), 12);
        let raw = rec.bits();
        assert_eq!(raw & 0xf, 4);
        assert_eq!((raw >> 4) & 0xfff, 12);
    }

    #[test]
    fn test_large_record_header() {
        let mut lrec = LargeRecordHeader::default();
        lrec.set_record_size(0x1000).set_large_type(2);
        assert_eq!(lrec.record_type(), RecordType::LargeBlob);
        assert_eq!(lrec.record_size(), 0x1000);
        assert_eq!(lrec.large_type(), 2);
        let raw = lrec.bits();
        assert_eq!(raw & 0xf, 15);
        assert_eq!((raw >> 4) & 0xffffffff, 0x1000);
        assert_eq!((raw >> 36) & 0xf, 2);
    }

    #[test]
    fn test_string_ref_header() {
        let mut interned = StringRefHeader::new();
        interned.set_id_or_len(42);
        assert_eq!(interned.id_or_len(), 42);
        assert!(!interned.is_inline());
        assert_eq!(interned.bits(), 42);

        let mut inline = StringRefHeader::new();
        inline.set_is_inline(true).set_id_or_len(15);
        assert!(inline.is_inline());
        assert_eq!(inline.id_or_len(), 15);
        assert_eq!(inline.bits(), 0x800f);
    }

    #[test]
    fn test_thread_ref_header() {
        let mut interned = ThreadRefHeader::new();
        interned.set_id_or_len(5);
        assert_eq!(interned.id_or_len(), 5);
        assert!(!interned.is_inline());
        assert_eq!(interned.bits(), 5);

        let mut inline = ThreadRefHeader::new();
        inline.set_is_inline(true).set_id_or_len(3);
        assert!(inline.is_inline());
        assert_eq!(inline.id_or_len(), 3);
        assert_eq!(inline.bits(), 0x83);
    }

    #[test]
    fn test_argument_header() {
        let mut arg = ArgumentHeader::for_argument(10, 2, ArgumentType::Uint64);
        assert_eq!(arg.arg_type(), ArgumentType::Uint64);
        assert_eq!(arg.size_words(), 2);
        assert_eq!(arg.name_ref(), 10);
        assert_eq!(arg.value_bits(), 0);

        arg.set_value_bits(0x12345678);
        assert_eq!(arg.value_bits(), 0x12345678);
        let raw = arg.bits();
        assert_eq!(raw & 0xf, 4);
        assert_eq!((raw >> 4) & 0xfff, 2);
        assert_eq!((raw >> 16) & 0xffff, 10);
        assert_eq!((raw >> 32) & 0xffffffff, 0x12345678);
    }

    #[test]
    fn test_kernel_object_record_header() {
        let mut ko = KernelObjectRecordHeader::default();
        ko.set_record_size(6).set_obj_type(1).set_name_ref(0x8009).set_arg_count(1);
        assert_eq!(ko.record_type(), RecordType::KernelObject);
        assert_eq!(ko.record_size(), 6);
        assert_eq!(ko.obj_type(), 1);
        assert_eq!(ko.name_ref(), 0x8009);
        assert_eq!(ko.arg_count(), 1);
        let raw = ko.bits();
        assert_eq!(raw & 0xf, 7);
        assert_eq!((raw >> 4) & 0xfff, 6);
        assert_eq!((raw >> 16) & 0xff, 1);
        assert_eq!((raw >> 24) & 0xffff, 0x8009);
        assert_eq!((raw >> 40) & 0xf, 1);
    }

    #[test]
    fn test_event_record_header() {
        let mut ev = EventRecordHeader::default();
        ev.set_record_size(5)
            .set_event_type(EventType::Instant)
            .set_arg_count(1)
            .set_thread_ref(0)
            .set_category_ref(10)
            .set_name_ref(20);
        assert_eq!(ev.record_type(), RecordType::Event);
        assert_eq!(ev.record_size(), 5);
        assert_eq!(ev.event_type(), EventType::Instant);
        assert_eq!(ev.arg_count(), 1);
        assert_eq!(ev.thread_ref(), 0);
        assert_eq!(ev.category_ref(), 10);
        assert_eq!(ev.name_ref(), 20);
        let raw = ev.bits();
        assert_eq!(raw & 0xf, 4);
        assert_eq!((raw >> 4) & 0xfff, 5);
        assert_eq!((raw >> 16) & 0xf, 0);
        assert_eq!((raw >> 20) & 0xf, 1);
        assert_eq!((raw >> 24) & 0xff, 0);
        assert_eq!((raw >> 32) & 0xffff, 10);
        assert_eq!((raw >> 48) & 0xffff, 20);
    }
}
