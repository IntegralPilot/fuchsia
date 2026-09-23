// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::arch_rs::{InterruptDisableGuard, curr_cpu_num, ints_disabled};
use crate::kernel::thread::{FxtRef, ThreadPtr};
pub use crate::kernel::types::Koid;
pub use crate::platform_rs::timer::{InstantBootTicks, timer_current_boot_ticks};
use core::cell::UnsafeCell;
use core::mem::{MaybeUninit, size_of};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};
use core::{ffi, ptr, slice};
pub use fxt_layout::{
    ArgumentHeader, ArgumentType, EventRecordHeader, EventType, KernelObjectRecordHeader,
    LargeRecordHeader, RecordHeader, RecordType, StringRefHeader,
};
use kalloc::Box;
pub use kstring::declare_interned_category;
use kstring::declare_interned_string;
use kstring::interned_category::InternedCategory;
pub use kstring::interned_string::InternedString;
use spsc_buffer::{Buffer, NoOpAllocator, Reservation};
use zx_status::Status;
use zx_types::zx_status_t;

use crate::user_copy::UserOutPtr;

unsafe extern "C" {
    fn cpp_ktrace_read_user(
        ptr: UserOutPtr<u8>,
        offset: u32,
        len: usize,
        out_actual: *mut usize,
    ) -> zx_status_t;
    fn cpp_ktrace_control(action: u32, options: u32) -> zx_status_t;
}

// Re-export the ktrace macros from the sub-crate.
#[allow(unused_imports)]
pub use ktrace_macro::*;

// LINT.IfChange(KTraceState)
#[repr(C)]
pub struct KTraceState {
    pub categories_bitmask: AtomicU32,
    pub writes_enabled: AtomicBool,
}
const _: () = assert!(size_of::<KTraceState>() == 8);
// LINT.ThenChange(//zircon/kernel/include/lib/ktrace.h:KTraceState)

declare_interned_category!(META_CAT, "kernel:meta", extern);
declare_interned_string!(DROP_STATS_REF, "drop_stats", extern);
declare_interned_string!(NUM_RECORDS_REF, "num_records", extern);
declare_interned_string!(NUM_BYTES_REF, "num_bytes", extern);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    Thread = 0,
    Cpu = 1,
}

/// The value of a trace argument.
pub enum ArgValue<'a> {
    Null,
    Bool(bool),
    Int32(i32),
    Uint32(u32),
    Int64(i64),
    Uint64(u64),
    Double(f64),
    String(&'a str),
    Pointer(usize),
    Koid(u64),
}

impl<'a> From<bool> for ArgValue<'a> {
    fn from(v: bool) -> Self {
        ArgValue::Bool(v)
    }
}
impl<'a> From<i32> for ArgValue<'a> {
    fn from(v: i32) -> Self {
        ArgValue::Int32(v)
    }
}
impl<'a> From<u32> for ArgValue<'a> {
    fn from(v: u32) -> Self {
        ArgValue::Uint32(v)
    }
}
impl<'a> From<i64> for ArgValue<'a> {
    fn from(v: i64) -> Self {
        ArgValue::Int64(v)
    }
}
impl<'a> From<u64> for ArgValue<'a> {
    fn from(v: u64) -> Self {
        ArgValue::Uint64(v)
    }
}
impl<'a> From<f64> for ArgValue<'a> {
    fn from(v: f64) -> Self {
        ArgValue::Double(v)
    }
}
impl<'a> From<&'a str> for ArgValue<'a> {
    fn from(v: &'a str) -> Self {
        ArgValue::String(v)
    }
}

/// A wrapper for KOID values passed as trace arguments.
impl<'a> From<Koid> for ArgValue<'a> {
    fn from(v: Koid) -> Self {
        ArgValue::Koid(v.0)
    }
}

/// Represents an FXT string reference that can either be an interned string or an inline string byte slice.
#[derive(Clone, Copy)]
pub enum StringRef<'a> {
    Interned(&'static InternedString),
    Inline(&'a [u8]),
}

impl<'a> From<&'static InternedString> for StringRef<'a> {
    fn from(s: &'static InternedString) -> Self {
        StringRef::Interned(s)
    }
}

impl<'a> From<&'a [u8]> for StringRef<'a> {
    fn from(s: &'a [u8]) -> Self {
        StringRef::Inline(s)
    }
}

impl<'a> From<&'a str> for StringRef<'a> {
    fn from(s: &'a str) -> Self {
        StringRef::Inline(s.as_bytes())
    }
}

impl<'a> StringRef<'a> {
    /// Returns the FXT header entry for this string reference.
    pub fn header_entry(&self) -> u16 {
        match self {
            StringRef::Interned(s) => {
                let mut hdr = StringRefHeader::new();
                hdr.set_id_or_len(s.id());
                hdr.bits()
            }
            StringRef::Inline(b) => {
                let len = b.len().min(0x7fff) as u16;
                let mut hdr = StringRefHeader::new();
                hdr.set_is_inline(true).set_id_or_len(len);
                hdr.bits()
            }
        }
    }

    /// Returns the payload size in 64-bit words required to store this string inline.
    pub fn payload_words(&self) -> usize {
        match self {
            StringRef::Interned(_) => 0,
            StringRef::Inline(b) => {
                let len = b.len().min(0x7fff);
                len.div_ceil(8)
            }
        }
    }

    /// Writes the inline string payload if applicable.
    pub fn write(&self, res: &mut KTraceReservation<'_>) -> Result<(), Status> {
        match self {
            StringRef::Interned(_) => Ok(()),
            StringRef::Inline(b) => {
                let len = b.len().min(0x7fff);
                res.write_bytes(&b[..len])
            }
        }
    }
}

pub struct Argument<'a> {
    name: &'static InternedString,
    value: ArgValue<'a>,
}

impl<'a> Argument<'a> {
    pub fn new(name: &'static InternedString, value: impl Into<ArgValue<'a>>) -> Self {
        Self { name, value: value.into() }
    }

    /// Returns the size of this argument in 64-bit words.
    fn size_words(&self) -> usize {
        match &self.value {
            ArgValue::Null | ArgValue::Bool(_) | ArgValue::Int32(_) | ArgValue::Uint32(_) => 1,
            ArgValue::Int64(_)
            | ArgValue::Uint64(_)
            | ArgValue::Double(_)
            | ArgValue::Pointer(_)
            | ArgValue::Koid(_) => 2,
            ArgValue::String(s) => 1 + s.len().min(0x7fff).div_ceil(8),
        }
    }

    fn write(&self, res: &mut KTraceReservation<'_>) -> Result<(), Status> {
        let name_id = self.name.id();
        let size_words = self.size_words() as u16;

        match &self.value {
            ArgValue::Null => {
                let header = ArgumentHeader::for_argument(name_id, size_words, ArgumentType::Null);
                res.write_word(header.bits())?;
            }
            ArgValue::Int32(v) => {
                let mut header =
                    ArgumentHeader::for_argument(name_id, size_words, ArgumentType::Int32);
                header.set_value_bits(*v as u32);
                res.write_word(header.bits())?;
            }
            ArgValue::Uint32(v) => {
                let mut header =
                    ArgumentHeader::for_argument(name_id, size_words, ArgumentType::Uint32);
                header.set_value_bits(*v);
                res.write_word(header.bits())?;
            }
            ArgValue::Int64(v) => {
                let header = ArgumentHeader::for_argument(name_id, size_words, ArgumentType::Int64);
                res.write_word(header.bits())?;
                res.write_word(*v as u64)?;
            }
            ArgValue::Uint64(v) => {
                let header =
                    ArgumentHeader::for_argument(name_id, size_words, ArgumentType::Uint64);
                res.write_word(header.bits())?;
                res.write_word(*v)?;
            }
            ArgValue::Double(v) => {
                let header =
                    ArgumentHeader::for_argument(name_id, size_words, ArgumentType::Double);
                res.write_word(header.bits())?;
                res.write_word(v.to_bits())?;
            }
            ArgValue::String(s) => {
                let mut header =
                    ArgumentHeader::for_argument(name_id, size_words, ArgumentType::String);
                let string_len = s.len().min(0x7fff) as u16;
                let mut str_ref = StringRefHeader::new();
                str_ref.set_is_inline(true).set_id_or_len(string_len);
                header.set_value_bits(str_ref.bits() as u32);
                res.write_word(header.bits())?;
                res.write_bytes(&s.as_bytes()[..string_len as usize])?;
            }
            ArgValue::Pointer(v) => {
                let header =
                    ArgumentHeader::for_argument(name_id, size_words, ArgumentType::Pointer);
                res.write_word(header.bits())?;
                res.write_word(*v as u64)?;
            }
            ArgValue::Koid(v) => {
                let header = ArgumentHeader::for_argument(name_id, size_words, ArgumentType::Koid);
                res.write_word(header.bits())?;
                res.write_word(*v)?;
            }
            ArgValue::Bool(v) => {
                let mut header =
                    ArgumentHeader::for_argument(name_id, size_words, ArgumentType::Bool);
                header.set_value_bits(*v as u32);
                res.write_word(header.bits())?;
            }
        }
        Ok(())
    }
}

pub struct KTraceScope<'a, const N: usize> {
    category: &'static InternedCategory,
    name: &'static InternedString,
    timestamp: InstantBootTicks,
    context: Context,
    args: [Argument<'a>; N],
}

impl<'a, const N: usize> KTraceScope<'a, N> {
    #[inline(never)]
    #[cold]
    pub fn begin(
        category: &'static InternedCategory,
        name: &'static InternedString,
        context: Context,
        args: [Argument<'a>; N],
    ) -> Self {
        let timestamp = timer_current_boot_ticks();
        Self { category, name, timestamp, context, args }
    }
}

impl<'a, const N: usize> Drop for KTraceScope<'a, N> {
    #[inline(never)]
    #[cold]
    fn drop(&mut self) {
        let end_time = timer_current_boot_ticks();
        let ktrace = KTrace::get_instance();
        ktrace.emit_event(
            EventType::DurationComplete,
            self.category,
            self.name,
            self.timestamp,
            self.context,
            Some(end_time.0 as u64),
            &self.args,
        );
    }
}

// LINT.IfChange(DroppedRecordDurationEvent)
#[repr(C)]
struct DroppedRecordDurationEvent {
    header: u64,
    start: InstantBootTicks,
    process_id: u64,
    thread_id: u64,
    num_dropped_arg: u64,
    bytes_dropped_arg: u64,
    end: InstantBootTicks,
}
const _: () = assert!(size_of::<DroppedRecordDurationEvent>() == 56);
// LINT.ThenChange(//zircon/kernel/lib/percpu_writer/include/lib/percpu_writer/buffer.h:DroppedRecordDurationEvent)

// LINT.IfChange(DroppedRecordStats)
/// This struct keeps track of the duration, number, and size of trace records dropped when the
/// buffer is full. These statistics are emitted to the trace buffer as a duration as soon as
/// space is available to do so, at which point the values are reset to 0, or false in the case
/// of has_dropped.
#[repr(C)]
#[derive(Default, Debug, Clone)]
pub struct DroppedRecordStats {
    pub first_dropped: InstantBootTicks,
    pub last_dropped: InstantBootTicks,

    /// By storing num_dropped and bytes_dropped in 32-bit values, we ensure that they can each
    /// be stored in a single 64-bit word in the FXT record we emit when space is available.
    pub num_dropped: u32,
    pub bytes_dropped: u32,
    pub has_dropped: bool,
}
const _: () = assert!(size_of::<DroppedRecordStats>() == 32);
// LINT.ThenChange(//zircon/kernel/lib/percpu_writer/include/lib/percpu_writer/buffer.h:DroppedRecordStats)

impl DroppedRecordStats {
    pub fn reset(&mut self) {
        self.first_dropped = InstantBootTicks(0);
        self.last_dropped = InstantBootTicks(0);
        self.num_dropped = 0;
        self.bytes_dropped = 0;
        self.has_dropped = false;
    }

    pub fn track(&mut self, now: InstantBootTicks, size: u32) {
        if !self.has_dropped {
            self.first_dropped = now;
            self.has_dropped = true;
        }
        self.last_dropped = now;
        self.num_dropped = self.num_dropped.wrapping_add(1);
        self.bytes_dropped = self.bytes_dropped.wrapping_add(size);
    }

    pub fn has_dropped(&self) -> bool {
        self.has_dropped
    }
}

/// A Rust implementation of `percpu_writer::Buffer` that wraps an existing
/// `spsc_buffer::Buffer` and tracks dropped trace records.
pub struct KTraceBuffer {
    // We use `NonNull` raw pointers instead of `&'static mut` because the underlying memory is
    // allocated and owned by the C++ layer. Holding long-lived `&mut` references would assert
    // perpetual exclusive ownership (`noalias`), violating Rust's aliasing rules across the FFI
    // boundary. `NonNull` allows us to create bounded, localized references only when accessed
    // with interrupts disabled.
    buffer: NonNull<Buffer<NoOpAllocator>>,
    drop_stats: NonNull<DroppedRecordStats>,
    cpu_ref_header_entry: u16,
    pub process_koid: u64,
    pub thread_koid: u64,
}

// SAFETY: Access to the per-CPU buffers is synchronized by the caller (typically via disabling
// interrupts).
unsafe impl Send for KTraceBuffer {}
unsafe impl Sync for KTraceBuffer {}

impl KTraceBuffer {
    /// Constructs a `KTraceBuffer` from raw pointers to an existing `spsc_buffer::Buffer`,
    /// `DroppedRecordStats`, and its metadata.
    pub fn new(
        buffer: NonNull<Buffer<NoOpAllocator>>,
        drop_stats: NonNull<DroppedRecordStats>,
        cpu_ref_header_entry: u16,
        process_koid: u64,
        thread_koid: u64,
    ) -> Self {
        Self { buffer, drop_stats, cpu_ref_header_entry, process_koid, thread_koid }
    }

    #[inline(always)]
    fn buffer(&self) -> &Buffer<NoOpAllocator> {
        unsafe { self.buffer.as_ref() }
    }

    #[inline(always)]
    fn buffer_mut<'a>(&mut self) -> &'a mut Buffer<NoOpAllocator> {
        unsafe { &mut *self.buffer.as_ptr() }
    }

    #[inline(always)]
    pub fn drop_stats(&self) -> &DroppedRecordStats {
        unsafe { self.drop_stats.as_ref() }
    }

    #[inline(always)]
    pub fn drop_stats_mut<'a>(&mut self) -> &'a mut DroppedRecordStats {
        unsafe { &mut *self.drop_stats.as_ptr() }
    }

    /// Returns the size of the backing storage.
    pub fn size(&self) -> u32 {
        self.buffer().size()
    }

    /// Drains the underlying Buffer.
    pub fn drain(&self) -> Result<(), Status> {
        self.buffer().drain()
    }

    /// Copies `len` bytes out of the buffer using the provided `copy_fn`.
    pub fn read<F>(&self, copy_fn: F, len: u32) -> Result<u32, Status>
    where
        F: FnMut(u32, &[u8]) -> Result<(), Status>,
    {
        self.buffer().read(copy_fn, len)
    }

    /// Reserves a block of the given size in the buffer, interposing to write dropped record
    /// statistics if any were tracked.
    pub fn reserve(&mut self, header: u64) -> Result<KTraceReservation<'_>, Status> {
        debug_assert!(ints_disabled());
        // Compute the number of bytes we need to reserve from the provided fxt header.
        let rec_hdr = RecordHeader::from(header);
        let num_words = if rec_hdr.record_type() == RecordType::LargeBlob {
            // Large record
            LargeRecordHeader::from(header).record_size()
        } else {
            // Normal record
            rec_hdr.record_size() as u32
        };
        let size = num_words.checked_mul(8).ok_or(Status::INVALID_ARGS)?;

        let mut total_size = size;
        let event = if self.drop_stats().has_dropped() {
            total_size += size_of::<DroppedRecordDurationEvent>() as u32;
            Some(self.serialize_drop_stats())
        } else {
            None
        };

        match self.buffer_mut().reserve(total_size) {
            Err(status) => {
                let now = timer_current_boot_ticks();
                self.drop_stats_mut().track(now, size);
                Err(status)
            }
            Ok(mut res) => {
                if let Some(event) = event {
                    // SAFETY: DroppedRecordDurationEvent is repr(C) and has a defined binary
                    // layout.
                    let event_bytes = unsafe {
                        slice::from_raw_parts(
                            ptr::from_ref(&event).cast::<u8>(),
                            size_of::<DroppedRecordDurationEvent>(),
                        )
                    };
                    res.write(event_bytes)?;
                    self.drop_stats_mut().reset();
                }
                KTraceReservation::new(res, header)
            }
        }
    }

    /// Emit the dropped record stats to the trace buffer if we're currently tracking them.
    pub fn emit_drop_stats(&mut self) -> Result<(), Status> {
        debug_assert!(ints_disabled());
        if !self.drop_stats().has_dropped() {
            return Ok(());
        }

        // Serialize the event first so we release the borrow on self before calling reserve.
        let event = self.serialize_drop_stats();

        let mut res = self.buffer_mut().reserve(size_of::<DroppedRecordDurationEvent>() as u32)?;
        // SAFETY: DroppedRecordDurationEvent is repr(C) and has a defined binary layout.
        let event_bytes = unsafe {
            slice::from_raw_parts(
                ptr::from_ref(&event).cast::<u8>(),
                size_of::<DroppedRecordDurationEvent>(),
            )
        };
        res.write(event_bytes)?;
        res.commit()?;

        self.drop_stats_mut().reset();
        Ok(())
    }

    fn serialize_drop_stats(&self) -> DroppedRecordDurationEvent {
        let record_size_words = (size_of::<DroppedRecordDurationEvent>() / 8) as u16;
        let mut header = EventRecordHeader::default();
        header
            .set_record_size(record_size_words)
            .set_event_type(EventType::DurationComplete)
            .set_arg_count(2)
            .set_thread_ref(self.cpu_ref_header_entry as u8)
            .set_category_ref(META_CAT.label().id())
            .set_name_ref(DROP_STATS_REF.id());

        let drop_stats = self.drop_stats();
        let mut num_dropped_arg =
            ArgumentHeader::for_argument(NUM_RECORDS_REF.id(), 1, ArgumentType::Uint32);
        num_dropped_arg.set_value_bits(drop_stats.num_dropped);

        let mut bytes_dropped_arg =
            ArgumentHeader::for_argument(NUM_BYTES_REF.id(), 1, ArgumentType::Uint32);
        bytes_dropped_arg.set_value_bits(drop_stats.bytes_dropped);

        DroppedRecordDurationEvent {
            header: header.bits(),
            start: drop_stats.first_dropped,
            process_id: self.process_koid,
            thread_id: self.thread_koid,
            num_dropped_arg: num_dropped_arg.bits(),
            bytes_dropped_arg: bytes_dropped_arg.bits(),
            end: drop_stats.last_dropped,
        }
    }
}

/// KTraceReservation encapsulates a pending write to the buffer.
#[derive(Debug)]
pub struct KTraceReservation<'a> {
    reservation: Reservation<'a>,
}

impl<'a> KTraceReservation<'a> {
    fn new(reservation: Reservation<'a>, header: u64) -> Result<Self, Status> {
        let mut this = Self { reservation };
        if let Err(e) = this.write_word(header) {
            this.reservation.cancel();
            return Err(e);
        }
        Ok(this)
    }

    /// Writes a single 64-bit word into the reservation.
    pub fn write_word(&mut self, word: u64) -> Result<(), Status> {
        debug_assert!(ints_disabled());
        self.reservation.write(&word.to_ne_bytes())
    }

    /// Writes a byte slice into the reservation, padding to an 8-byte boundary.
    pub fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), Status> {
        debug_assert!(ints_disabled());
        self.reservation.write(bytes)?;
        let num_bytes = bytes.len();
        let aligned_bytes = (num_bytes + 7) & !7;
        let num_zeros_to_write = aligned_bytes - num_bytes;
        if num_zeros_to_write > 0 {
            let zero = [0u8; 8];
            self.reservation.write(&zero[..num_zeros_to_write])?;
        }
        Ok(())
    }

    /// Commits the reservation, making it visible to the reader.
    pub fn commit(self) -> Result<(), Status> {
        debug_assert!(ints_disabled());
        self.reservation.commit()
    }
}

/// A pure Rust implementation of KTrace.
pub struct KTrace {
    /// Reference to the shared C++ KTraceState.
    // TODO(https://fxbug.dev/517305548): This should be made a direct allocation, and not a
    // reference, once the C++ implementation is removed.
    state: &'static KTraceState,

    /// A heap-allocated slice of atomic pointers to per-CPU buffers.
    /// Allocated once at boot time, and accessed completely lock-free on the hot path by CPU ID.
    buffers: Box<[AtomicPtr<KTraceBuffer>]>,
}

// SAFETY: KTrace is a global singleton. Access to the per-CPU buffers is synchronized.
unsafe impl Sync for KTrace {}
unsafe impl Send for KTrace {}

#[repr(transparent)]
struct KTraceSingleton(UnsafeCell<MaybeUninit<KTrace>>);

unsafe impl Sync for KTraceSingleton {}
unsafe impl Send for KTraceSingleton {}

static INSTANCE: KTraceSingleton = KTraceSingleton(UnsafeCell::new(MaybeUninit::uninit()));

impl KTrace {
    /// Retrieve the global instance of KTrace.
    pub fn get_instance() -> &'static Self {
        // SAFETY: KTrace must be initialized during kernel boot.
        unsafe { &*INSTANCE.0.get().cast::<KTrace>() }
    }

    /// Returns the raw pointer to the KTraceBuffer for the given CPU.
    pub fn get_buffer(&self, cpu: usize) -> *mut KTraceBuffer {
        self.buffers[cpu].load(Ordering::Acquire)
    }

    /// Reserves a slot of memory to write a record into.
    ///
    /// # Safety
    ///
    /// This method MUST be invoked with interrupts disabled to enforce the single-writer invariant.
    pub unsafe fn reserve(&self, header: u64) -> Result<KTraceReservation<'_>, Status> {
        debug_assert!(ints_disabled());
        if !self.writes_enabled() {
            return Err(Status::BAD_STATE);
        }

        let cpu_num = curr_cpu_num() as usize;
        if cpu_num >= self.buffers.len() {
            return Err(Status::BAD_STATE);
        }

        let ptr = self.buffers[cpu_num].load(Ordering::Acquire);
        if ptr.is_null() {
            return Err(Status::BAD_STATE);
        }

        let buf = unsafe { &mut *ptr };
        buf.reserve(header)
    }

    /// Returns true if writes are currently enabled.
    pub fn writes_enabled(&self) -> bool {
        self.state.writes_enabled.load(Ordering::Acquire)
    }

    /// Returns the categories bitmask.
    pub fn categories_bitmask(&self) -> u32 {
        self.state.categories_bitmask.load(Ordering::Acquire)
    }

    /// Returns true if the given category is enabled.
    pub fn is_category_enabled(&self, category: &InternedCategory) -> bool {
        let category_index = category.index();
        if category_index == InternedCategory::INVALID_INDEX {
            return false;
        }
        let bitmask = self.categories_bitmask();
        (bitmask & (1 << category_index)) != 0
    }

    /// Reads up to `len` bytes from the ktrace buffer starting at `offset` into `ptr`.
    ///
    /// Returns the number of bytes actually read.
    pub fn read_user(&self, ptr: UserOutPtr<u8>, offset: u32, len: usize) -> Result<usize, Status> {
        let mut actual = 0usize;
        // SAFETY: `cpp_ktrace_read_user` reads trace data from the global ktrace buffer into the
        // user buffer represented by `ptr` and writes the actual count into `actual`.
        let status = unsafe { cpp_ktrace_read_user(ptr, offset, len, &mut actual) };
        Status::ok(status)?;
        Ok(actual)
    }

    /// Performs a control operation on the ktrace subsystem.
    pub fn control(&self, action: u32, options: u32) -> Result<(), Status> {
        // SAFETY: `cpp_ktrace_control` starts, stops, or rewinds tracing using validated action/options.
        let status = unsafe { cpp_ktrace_control(action, options) };
        Status::ok(status)
    }

    /// Low-level helper to write an FXT kernel object record.
    ///
    /// This is not inlined to reduce code size at the instrumentation sites.
    #[inline(never)]
    #[cold]
    pub fn emit_kernel_object_outlined<'a>(
        &self,
        koid: u64,
        obj_type: u32,
        name: impl Into<StringRef<'a>>,
        args: &[Argument<'_>],
    ) {
        let _guard = InterruptDisableGuard::new();
        if !self.writes_enabled() {
            return;
        }

        let name = name.into();
        let base_size = 2; // Header, KOID
        let name_size = name.payload_words();
        let args = if args.len() > 15 { &args[..15] } else { args };
        let args_size: usize = args.iter().map(|a| a.size_words()).sum();
        let total_size_words = base_size + name_size + args_size;

        if total_size_words > 0xfff {
            return;
        }

        let mut header = KernelObjectRecordHeader::default();
        header
            .set_record_size(total_size_words as u16)
            .set_obj_type(obj_type as u8)
            .set_name_ref(name.header_entry())
            .set_arg_count(args.len() as u8);

        // SAFETY: Interrupts are disabled by `_guard`, guaranteeing mutual exclusion during reservation.
        if let Ok(mut res) = unsafe { self.reserve(header.bits()) } {
            let _ = res.write_word(koid);
            let _ = name.write(&mut res);
            for arg in args {
                let _ = arg.write(&mut res);
            }
            let _ = res.commit();
        }
    }

    /// Low-level helper to write a generic FXT event record.
    ///
    /// This is not inlined to reduce code size at the instrumentation sites.
    #[allow(clippy::too_many_arguments)]
    #[inline(never)]
    #[cold]
    pub fn emit_event(
        &self,
        event_type: EventType,
        category: &InternedCategory,
        name: &InternedString,
        timestamp: InstantBootTicks,
        context: Context,
        content: Option<u64>,
        args: &[Argument<'_>],
    ) {
        let _guard = InterruptDisableGuard::new();
        if !self.writes_enabled() {
            return;
        }

        // 1. Get the process/thread KOIDs for the context.
        let (process_koid, thread_koid) = match context {
            Context::Thread => {
                // SAFETY: ktrace is initialized and running after threading has been initialized.
                let FxtRef { pid, tid } = unsafe { ThreadPtr::current().fxt_ref() };
                (pid, tid)
            }
            Context::Cpu => {
                let cpu = curr_cpu_num() as usize;
                if cpu >= self.buffers.len() {
                    return;
                }
                let ptr = self.buffers[cpu].load(Ordering::Acquire);
                if ptr.is_null() {
                    return;
                }
                let buf = unsafe { &*ptr };
                (buf.process_koid, buf.thread_koid)
            }
        };

        // 2. Calculate the record size.
        let base_size = 4; // Header, Timestamp, Process KOID, Thread KOID
        let content_size = if content.is_some() { 1 } else { 0 };
        let args = if args.len() > 15 { &args[..15] } else { args };
        let args_size: usize = args.iter().map(|a| a.size_words()).sum();
        let total_size_words = base_size + content_size + args_size;

        if total_size_words > 0xfff {
            return;
        }

        // 3. Construct the header.
        let mut header = EventRecordHeader::default();
        header
            .set_record_size(total_size_words as u16)
            .set_event_type(event_type)
            .set_arg_count(args.len() as u8)
            .set_category_ref(category.label().id())
            .set_name_ref(name.id());

        // 4. Reserve space and write the record.
        if let Ok(mut res) = unsafe { self.reserve(header.bits()) } {
            let _ = res.write_word(timestamp.0 as u64);
            let _ = res.write_word(process_koid);
            let _ = res.write_word(thread_koid);

            for arg in args {
                let _ = arg.write(&mut res);
            }

            if let Some(c) = content {
                let _ = res.write_word(c);
            }

            let _ = res.commit();
        }
    }
}

/// Initializes the global KTrace instance with the given number of CPU buffers.
///
/// # Safety
///
/// This must be called at most once during kernel boot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_ktrace_init(num_buffers: u32, state_ptr: *mut ffi::c_void) -> i32 {
    if num_buffers == 0 || state_ptr.is_null() {
        return Status::INVALID_ARGS.into_raw();
    }

    // SAFETY: The caller guarantees that state_ptr points to a valid KTraceState instance
    // which has static storage duration (lives forever) and is safe to access concurrently
    // (since its fields are atomic).
    let state = unsafe { &*state_ptr.cast::<KTraceState>() };

    let buffers = match Box::<[AtomicPtr<KTraceBuffer>]>::try_new_zeroed_slice(num_buffers as usize)
    {
        Ok(b) => b,
        Err(_) => return Status::NO_MEMORY.into_raw(),
    };

    let ktrace = KTrace { state, buffers };

    unsafe {
        let slot = INSTANCE.0.get();
        ptr::write(slot.cast::<KTrace>(), ktrace);
    }

    zx_status::sys::ZX_OK
}

/// Initializes the KTraceBuffer for a specific CPU using a pointer to the C++ SpscBuffer.
///
/// # Safety
///
/// - `spsc_buffer_ptr` must point to a valid C++ `SpscBuffer` instance
///   which is binary-compatible with `spsc_buffer::Buffer<NoOpAllocator>`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_ktrace_init_cpu_buffer(
    cpu_num: u32,
    spsc_buffer_ptr: *mut ffi::c_void,
    drop_stats_ptr: *mut ffi::c_void,
    process_koid: u64,
    thread_koid: u64,
    cpu_ref_header_entry: u16,
) -> i32 {
    let ktrace = KTrace::get_instance();
    if cpu_num >= ktrace.buffers.len() as u32 {
        return Status::INVALID_ARGS.into_raw();
    }

    let buffer = match NonNull::new(spsc_buffer_ptr.cast::<Buffer<NoOpAllocator>>()) {
        Some(b) => b,
        None => return Status::INVALID_ARGS.into_raw(),
    };
    let drop_stats = match NonNull::new(drop_stats_ptr.cast::<DroppedRecordStats>()) {
        Some(d) => d,
        None => return Status::INVALID_ARGS.into_raw(),
    };

    // SAFETY: `spsc_buffer_ptr` is verified to be non-null above, and the caller guarantees that it
    // points to a valid `SpscBuffer` whose memory layout is compatible with
    // `Buffer<NoOpAllocator>`.
    if !unsafe { buffer.as_ref().is_valid() } {
        return Status::BAD_STATE.into_raw();
    }

    // Allocate the KTraceBuffer on the heap.
    let buf =
        KTraceBuffer::new(buffer, drop_stats, cpu_ref_header_entry, process_koid, thread_koid);

    let boxed_buf = match Box::try_new(buf) {
        Ok(b) => b,
        Err(_) => return Status::NO_MEMORY.into_raw(),
    };

    let raw_ptr = Box::into_raw(boxed_buf);

    // Store the pointer atomically.
    let old_ptr = ktrace.buffers[cpu_num as usize].swap(raw_ptr, Ordering::AcqRel);
    if !old_ptr.is_null() {
        // If there was a previous buffer, reclaim and drop it.
        unsafe {
            let _ = Box::from_raw(old_ptr);
        }
    }

    zx_status::sys::ZX_OK
}

/// KTrace tests
#[cfg(all(not(gcc), ktest))]
#[unittest::suite(name = "ktrace_rust")]
mod tests {
    use crate::arch_rs::{InterruptDisableGuard, curr_cpu_num, max_num_cpus};
    use crate::kernel::thread::{FxtRef, ThreadPtr};
    use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use core::{ffi, ptr};
    use spsc_buffer::Buffer;
    use unittest::{
        assert_eq, assert_ok, expect_eq, expect_false, expect_ne, expect_true, unwrap_ok,
    };

    declare_interned_category!(MEMORY_CAT, "kernel:memory", extern);
    declare_interned_category!(SCHED_CAT, "kernel:sched", extern);
    declare_interned_category!(CONTENTION_CAT, "kernel:contention", extern);
    declare_interned_category!(IPC_CAT, "kernel:ipc", extern);
    declare_interned_category!(IRQ_CAT, "kernel:irq", extern);

    unsafe extern "C" {
        fn ktrace_restore_rust_singleton();
    }

    /// Initialization and size/metadata attributes.
    #[test]
    fn init_and_size() {
        let mut storage = [0u8; 256];
        let mut inner_buf = unsafe { Buffer::from_raw_parts(storage.as_mut_ptr(), storage.len()) };
        let mut stats = DroppedRecordStats::default();
        let kbuf = KTraceBuffer::new(
            NonNull::new(ptr::from_mut(&mut inner_buf)).unwrap(),
            NonNull::new(ptr::from_mut(&mut stats)).unwrap(),
            1,
            100,
            200,
        );

        expect_eq!(kbuf.size(), 256);
        expect_eq!(kbuf.process_koid, 100);
        expect_eq!(kbuf.thread_koid, 200);
    }

    // Verifies that constructing or operating on buffers with null or invalid storage fails
    // gracefully with an error code rather than panicking on slice::from_raw_parts.
    //
    /// Buffer operations with null/invalid storage fail gracefully without panicking.
    #[test]
    fn invalid_buffer_guards() {
        let _guard = InterruptDisableGuard::new();

        // 1. rust_ktrace_init_cpu_buffer with null pointers.
        let mut stats = DroppedRecordStats::default();
        let stats_ptr = ptr::from_mut(&mut stats).cast::<ffi::c_void>();
        let status_null_buf =
            unsafe { rust_ktrace_init_cpu_buffer(0, ptr::null_mut(), stats_ptr, 100, 200, 1) };
        expect_eq!(status_null_buf, Status::INVALID_ARGS.into_raw());

        let mut storage = [0u8; 256];
        let mut valid_inner =
            unsafe { Buffer::from_raw_parts(storage.as_mut_ptr(), storage.len()) };
        let valid_buf_ptr = ptr::from_mut(&mut valid_inner).cast::<ffi::c_void>();
        let status_null_stats =
            unsafe { rust_ktrace_init_cpu_buffer(0, valid_buf_ptr, ptr::null_mut(), 100, 200, 1) };
        expect_eq!(status_null_stats, Status::INVALID_ARGS.into_raw());

        // 2. rust_ktrace_init_cpu_buffer with invalid backing buffer (null storage / 0 size).
        let mut invalid_inner = Buffer::<NoOpAllocator>::empty();
        let invalid_buf_ptr = ptr::from_mut(&mut invalid_inner).cast::<ffi::c_void>();
        let status_invalid =
            unsafe { rust_ktrace_init_cpu_buffer(0, invalid_buf_ptr, stats_ptr, 100, 200, 1) };
        expect_eq!(status_invalid, Status::BAD_STATE.into_raw());

        // 3. Operating on KTraceBuffer wrapping an empty/invalid buffer with null storage.
        let mut kbuf = KTraceBuffer::new(
            NonNull::new(ptr::from_mut(&mut invalid_inner)).unwrap(),
            NonNull::new(ptr::from_mut(&mut stats)).unwrap(),
            1,
            100,
            200,
        );

        let header = 4u64 | (2u64 << 4);
        expect_true!(kbuf.reserve(header).err() == Some(Status::BAD_STATE));
        let read_res = kbuf.read(|_, _| Ok(()), 16);
        expect_true!(read_res.err() == Some(Status::BAD_STATE));
    }

    /// Reservation, writing words, and committing.
    #[test]
    fn test_write() {
        let _guard = InterruptDisableGuard::new();
        let mut storage = [0u8; 256];
        let mut inner_buf = unsafe { Buffer::from_raw_parts(storage.as_mut_ptr(), storage.len()) };
        let mut stats = DroppedRecordStats::default();
        let mut kbuf = KTraceBuffer::new(
            NonNull::new(ptr::from_mut(&mut inner_buf)).unwrap(),
            NonNull::new(ptr::from_mut(&mut stats)).unwrap(),
            1,
            100,
            200,
        );

        // Reserve 16 bytes (2 words).
        let header = 4u64 | (2u64 << 4);
        let mut res = unwrap_ok!(kbuf.reserve(header));

        // Write a word (8 bytes).
        assert_ok!(res.write_word(0xabcdef0123456789));
        assert_ok!(res.commit());

        // Read it back.
        let mut read_bytes = [0u8; 16];
        let read_len = unwrap_ok!(kbuf.read(
            |offset, src| {
                read_bytes[offset as usize..offset as usize + src.len()].copy_from_slice(src);
                Ok(())
            },
            16,
        ));

        assert_eq!(read_len, 16);
        expect_eq!(u64::from_ne_bytes(read_bytes[0..8].try_into().unwrap()), header);
        expect_eq!(u64::from_ne_bytes(read_bytes[8..16].try_into().unwrap()), 0xabcdef0123456789);
    }

    // Verifies tracking of dropped records and their subsequent serialization when space becomes
    // available.
    //
    /// Dropped record tracking and serialization.
    #[test]
    fn dropped_record_tracking() {
        let _guard = InterruptDisableGuard::new();
        let mut storage = [0u8; 128]; // small buffer
        let mut inner_buf = unsafe { Buffer::from_raw_parts(storage.as_mut_ptr(), storage.len()) };
        let mut stats = DroppedRecordStats::default();
        let mut kbuf = KTraceBuffer::new(
            NonNull::new(ptr::from_mut(&mut inner_buf)).unwrap(),
            NonNull::new(ptr::from_mut(&mut stats)).unwrap(),
            1,
            100,
            200,
        );

        // Reserve almost all space.
        // 128 bytes total. Let's reserve 96 bytes (12 words).
        let header = 4u64 | (12u64 << 4);
        let mut res = unwrap_ok!(kbuf.reserve(header));
        assert_ok!(res.write_bytes(&[0u8; 88]));
        assert_ok!(res.commit());

        // Now, try to reserve 64 bytes (8 words). This should fail because there are only 32 bytes
        // left.
        let header2 = 4u64 | (8u64 << 4);
        expect_true!(kbuf.reserve(header2).err() == Some(Status::NO_SPACE));

        // This failed reservation should have been tracked!
        expect_true!(kbuf.drop_stats().has_dropped());
        expect_eq!(kbuf.drop_stats().num_dropped, 1);
        expect_eq!(kbuf.drop_stats().bytes_dropped, 64);

        // Now, drain the buffer to free all space.
        assert_ok!(kbuf.drain());

        // Now, reserve 16 bytes (2 words).
        // Since first_dropped was set, this reservation should successfully write the 56-byte
        // dropped records duration event first!
        let header3 = 4u64 | (2u64 << 4);
        let mut res3 = unwrap_ok!(kbuf.reserve(header3));
        assert_ok!(res3.write_bytes(&[0u8; 8]));
        assert_ok!(res3.commit());

        // The dropped stats should have been reset!
        expect_false!(kbuf.drop_stats().has_dropped());
        expect_eq!(kbuf.drop_stats().num_dropped, 0);
        expect_eq!(kbuf.drop_stats().bytes_dropped, 0);

        // Let's read the buffer content.
        let mut read_bytes = [0u8; 72];
        let read_len = unwrap_ok!(kbuf.read(
            |offset, src| {
                read_bytes[offset as usize..offset as usize + src.len()].copy_from_slice(src);
                Ok(())
            },
            72,
        ));
        expect_eq!(read_len, 72);

        // Verify the DroppedRecordDurationEvent header:
        let event_header = u64::from_ne_bytes(read_bytes[0..8].try_into().unwrap());

        expect_eq!(event_header & 0xf, 4);
        expect_eq!((event_header >> 4) & 0xfff, 7);
        expect_eq!((event_header >> 16) & 0xf, 4);
        expect_eq!((event_header >> 20) & 0xf, 2);
        expect_eq!((event_header >> 24) & 0xff, 1);
        expect_eq!((event_header >> 32) & 0xffff, u64::from(META_CAT.label().id()));
        expect_eq!((event_header >> 48) & 0xffff, u64::from(DROP_STATS_REF.id()));

        // Verify process_koid (100) and thread_koid (200)
        expect_eq!(u64::from_ne_bytes(read_bytes[16..24].try_into().unwrap()), 100);
        expect_eq!(u64::from_ne_bytes(read_bytes[24..32].try_into().unwrap()), 200);

        // Verify the two arguments:
        let num_dropped_arg = u64::from_ne_bytes(read_bytes[32..40].try_into().unwrap());
        expect_eq!(num_dropped_arg & 0xf, 2);
        expect_eq!((num_dropped_arg >> 4) & 0xfff, 1);
        expect_eq!((num_dropped_arg >> 16) & 0xffff, u64::from(NUM_RECORDS_REF.id()));
        expect_eq!((num_dropped_arg >> 32) & 0xffffffff, 1);

        let bytes_dropped_arg = u64::from_ne_bytes(read_bytes[40..48].try_into().unwrap());
        expect_eq!(bytes_dropped_arg & 0xf, 2);
        expect_eq!((bytes_dropped_arg >> 16) & 0xffff, u64::from(NUM_BYTES_REF.id()));
        expect_eq!((bytes_dropped_arg >> 32) & 0xffffffff, 64);

        // Verify the reservation header3:
        let res_header = u64::from_ne_bytes(read_bytes[56..64].try_into().unwrap());
        expect_eq!(res_header, header3);
    }

    /// Direct invocation of KTraceBuffer::emit_drop_stats.
    #[test]
    fn emit_drop_stats() {
        let _guard = InterruptDisableGuard::new();
        let mut storage = [0u8; 128];
        let mut inner_buf = unsafe { Buffer::from_raw_parts(storage.as_mut_ptr(), storage.len()) };
        let mut stats = DroppedRecordStats::default();
        let mut kbuf = KTraceBuffer::new(
            NonNull::new(ptr::from_mut(&mut inner_buf)).unwrap(),
            NonNull::new(ptr::from_mut(&mut stats)).unwrap(),
            1,
            100,
            200,
        );

        // 1. Force a failed reservation to track a dropped record.
        let header = 4u64 | (32u64 << 4);
        expect_true!(kbuf.reserve(header).err() == Some(Status::NO_SPACE));

        expect_true!(kbuf.drop_stats().has_dropped());
        expect_eq!(kbuf.drop_stats().num_dropped, 1);
        expect_eq!(kbuf.drop_stats().bytes_dropped, 256);

        // 2. Call emit_drop_stats directly.
        assert_ok!(kbuf.emit_drop_stats());

        expect_false!(kbuf.drop_stats().has_dropped());
        expect_eq!(kbuf.drop_stats().num_dropped, 0);
        expect_eq!(kbuf.drop_stats().bytes_dropped, 0);

        // 3. Read and verify the event.
        let mut read_bytes = [0u8; 56];
        let read_len = unwrap_ok!(kbuf.read(
            |offset, src| {
                read_bytes[offset as usize..offset as usize + src.len()].copy_from_slice(src);
                Ok(())
            },
            56,
        ));

        expect_eq!(read_len, 56);

        let event_header = u64::from_ne_bytes(read_bytes[0..8].try_into().unwrap());
        expect_eq!(event_header & 0xf, 4);
        expect_eq!((event_header >> 4) & 0xfff, 7);
        expect_eq!((event_header >> 16) & 0xf, 4);
        expect_eq!((event_header >> 20) & 0xf, 2);
        expect_eq!((event_header >> 24) & 0xff, 1);
        expect_eq!((event_header >> 32) & 0xffff, u64::from(META_CAT.label().id()));
        expect_eq!((event_header >> 48) & 0xffff, u64::from(DROP_STATS_REF.id()));

        expect_eq!(u64::from_ne_bytes(read_bytes[16..24].try_into().unwrap()), 100);
        expect_eq!(u64::from_ne_bytes(read_bytes[24..32].try_into().unwrap()), 200);

        let num_dropped_arg = u64::from_ne_bytes(read_bytes[32..40].try_into().unwrap());
        expect_eq!(num_dropped_arg & 0xf, 2);
        expect_eq!((num_dropped_arg >> 16) & 0xffff, u64::from(NUM_RECORDS_REF.id()));
        expect_eq!((num_dropped_arg >> 32) & 0xffffffff, 1);

        let bytes_dropped_arg = u64::from_ne_bytes(read_bytes[40..48].try_into().unwrap());
        expect_eq!(bytes_dropped_arg & 0xf, 2);
        expect_eq!((bytes_dropped_arg >> 16) & 0xffff, u64::from(NUM_BYTES_REF.id()));
        expect_eq!((bytes_dropped_arg >> 32) & 0xffffffff, 256);
    }

    // Verifies the full global lifecycle of KTrace: initialization, category bitmasks, CPU buffer
    // allocation, and high-level tracing macro execution.
    //
    /// Validate full global lifecycle.
    #[test]
    fn global_lifecycle() {
        struct RestoreGuard;
        impl Drop for RestoreGuard {
            fn drop(&mut self) {
                unsafe {
                    ktrace_restore_rust_singleton();
                }
            }
        }
        let _restore_guard = RestoreGuard;
        let _guard = InterruptDisableGuard::new();

        // Initialize indices of the categories we're testing.
        META_CAT.set_index(0, kstring::interned_category::InternedCategory::INVALID_INDEX);
        MEMORY_CAT.set_index(1, kstring::interned_category::InternedCategory::INVALID_INDEX);
        SCHED_CAT.set_index(2, kstring::interned_category::InternedCategory::INVALID_INDEX);
        CONTENTION_CAT.set_index(3, kstring::interned_category::InternedCategory::INVALID_INDEX);
        IPC_CAT.set_index(4, kstring::interned_category::InternedCategory::INVALID_INDEX);
        IRQ_CAT.set_index(5, kstring::interned_category::InternedCategory::INVALID_INDEX);

        // 1. Initialize the global KTrace instance with the system CPU count buffers and a local
        // mock state.
        let num_cpus = max_num_cpus();
        let mut local_state = KTraceState {
            categories_bitmask: AtomicU32::new(0),
            writes_enabled: AtomicBool::new(false),
        };
        let local_state_ptr = ptr::from_mut(&mut local_state).cast::<ffi::c_void>();

        let status = unsafe { rust_ktrace_init(num_cpus, local_state_ptr) };
        expect_eq!(status, 0);

        let ktrace = KTrace::get_instance();

        // 2. Verify initial states.
        expect_false!(ktrace.writes_enabled());
        expect_eq!(ktrace.categories_bitmask(), 0);
        expect_false!(ktrace.is_category_enabled(META_CAT));
        expect_false!(ktrace.is_category_enabled(IRQ_CAT));

        // 3. Test writes_enabled.
        local_state.writes_enabled.store(true, Ordering::Release);
        expect_true!(ktrace.writes_enabled());
        local_state.writes_enabled.store(false, Ordering::Release);
        expect_false!(ktrace.writes_enabled());

        // 4. Test categories_bitmask.
        let mask = (1 << MEMORY_CAT.index()) | (1 << CONTENTION_CAT.index());
        local_state.categories_bitmask.store(mask, Ordering::Release);
        expect_eq!(ktrace.categories_bitmask(), mask);
        expect_false!(ktrace.is_category_enabled(META_CAT));
        expect_true!(ktrace.is_category_enabled(MEMORY_CAT));
        expect_false!(ktrace.is_category_enabled(SCHED_CAT));
        expect_true!(ktrace.is_category_enabled(CONTENTION_CAT));
        expect_false!(ktrace.is_category_enabled(IPC_CAT));

        // 5. Test CPU buffer initialization and reserve.
        let mut storage = [0u8; 256];
        let mut inner_buf = unsafe { Buffer::from_raw_parts(storage.as_mut_ptr(), storage.len()) };
        let inner_buf_ptr = ptr::from_mut(&mut inner_buf).cast::<ffi::c_void>();
        let mut stats = DroppedRecordStats::default();
        let stats_ptr = ptr::from_mut(&mut stats).cast::<ffi::c_void>();

        // Initialize current CPU buffer.
        let cpu = curr_cpu_num();
        let init_status = unsafe {
            rust_ktrace_init_cpu_buffer(
                cpu, // cpu_num
                inner_buf_ptr,
                stats_ptr,
                100, // process_koid
                200, // thread_koid
                1,   // cpu_ref_header_entry
            )
        };
        expect_eq!(init_status, 0);

        // If writes are disabled, reserving should fail.
        let header = 4u64 | (2u64 << 4); // Event with 2 words
        expect_true!(unsafe { ktrace.reserve(header) }.err() == Some(Status::BAD_STATE));

        // Enable writes.
        local_state.writes_enabled.store(true, Ordering::Release);

        // Now reserve should succeed!
        let mut res = unwrap_ok!(unsafe { ktrace.reserve(header) });
        assert_ok!(res.write_word(0x1234567890abcdef));
        assert_ok!(res.commit());

        // Read and verify the written record from the current CPU buffer.
        let ptr = ktrace.get_buffer(cpu as usize);
        expect_false!(ptr.is_null());
        let buf = unsafe { &*ptr };

        let mut read_bytes = [0u8; 16];
        let read_len = unwrap_ok!(buf.read(
            |offset, src| {
                read_bytes[offset as usize..offset as usize + src.len()].copy_from_slice(src);
                Ok(())
            },
            16,
        ));

        expect_eq!(read_len, 16);
        expect_eq!(u64::from_ne_bytes(read_bytes[0..8].try_into().unwrap()), header);
        expect_eq!(u64::from_ne_bytes(read_bytes[8..16].try_into().unwrap()), 0x1234567890abcdef);

        // 6. Test Rust macros!
        let mask = (1 << META_CAT.index()) | (1 << MEMORY_CAT.index());
        local_state.categories_bitmask.store(mask, Ordering::Release);
        expect_true!(ktrace.is_category_enabled(META_CAT));

        // Let's emit an instant event using the macro!
        instant!(META_CAT, "my_event", "arg1" => 42i32, "arg2" => "hello");

        // Now let's read the record back and verify it!
        let mut read_bytes = [0u8; 128];
        let read_len = unwrap_ok!(buf.read(
            |offset, src| {
                read_bytes[offset as usize..offset as usize + src.len()].copy_from_slice(src);
                Ok(())
            },
            56,
        ));

        expect_eq!(read_len, 56);

        // Verify Header Word
        let header_word = u64::from_ne_bytes(read_bytes[0..8].try_into().unwrap());
        expect_eq!(header_word & 0xf, 4);
        expect_eq!((header_word >> 4) & 0xfff, 7);
        expect_eq!((header_word >> 16) & 0xf, 0);
        expect_eq!((header_word >> 20) & 0xf, 2);
        expect_eq!((header_word >> 32) & 0xffff, u64::from(META_CAT.label().id()));
        let name_id = (header_word >> 48) & 0xffff;
        expect_ne!(name_id, 0);

        // Verify Timestamp (word 1)
        let timestamp_word = u64::from_ne_bytes(read_bytes[8..16].try_into().unwrap());
        expect_ne!(timestamp_word, 0);

        // Verify KOIDs (words 2 & 3)
        let process_koid = u64::from_ne_bytes(read_bytes[16..24].try_into().unwrap());
        let thread_koid = u64::from_ne_bytes(read_bytes[24..32].try_into().unwrap());
        // SAFETY: Tests run after threading has been initialized.
        let FxtRef { pid: expected_proc, tid: expected_thread } =
            unsafe { ThreadPtr::current().fxt_ref() };
        expect_eq!(process_koid, expected_proc);
        expect_eq!(thread_koid, expected_thread);

        // Verify Argument 1 ("arg1" => 42i32) (word 4)
        let arg1_header = u64::from_ne_bytes(read_bytes[32..40].try_into().unwrap());
        expect_eq!(arg1_header & 0xf, 1);
        expect_eq!((arg1_header >> 4) & 0xfff, 1);
        expect_eq!((arg1_header >> 32) & 0xffffffff, 42);
        let arg1_name_id = (arg1_header >> 16) & 0xffff;
        expect_ne!(arg1_name_id, 0);

        // Verify Argument 2 ("arg2" => "hello") (words 5 & 6)
        let arg2_header = u64::from_ne_bytes(read_bytes[40..48].try_into().unwrap());
        expect_eq!(arg2_header & 0xf, 6);
        expect_eq!((arg2_header >> 4) & 0xfff, 2);
        expect_eq!((arg2_header >> 32) & 0xffff, 0x8005);
        let arg2_name_id = (arg2_header >> 16) & 0xffff;
        expect_ne!(arg2_name_id, 0);

        let arg2_val = &read_bytes[48..56];
        expect_true!(&arg2_val[0..5] == b"hello");
        expect_true!(arg2_val[5..8] == [0, 0, 0]);

        // 7. Test kernel_object! macro with dynamic inline name and Koid argument.
        kernel_object!(META_CAT, 200u64, 1u32, &b"test_proc"[..], "job" => Koid(300u64));

        let mut read_bytes = [0u8; 128];
        let read_len = unwrap_ok!(buf.read(
            |offset, src| {
                read_bytes[offset as usize..offset as usize + src.len()].copy_from_slice(src);
                Ok(())
            },
            48,
        ));

        expect_eq!(read_len, 48);

        // Verify KernelObject Header (word 0): type=7, size=6 words (48 bytes), obj_type=1, name=0x8009, args=1
        let ko_header = u64::from_ne_bytes(read_bytes[0..8].try_into().unwrap());
        expect_eq!(ko_header & 0xf, 7);
        expect_eq!((ko_header >> 4) & 0xfff, 6);
        expect_eq!((ko_header >> 16) & 0xff, 1);
        expect_eq!((ko_header >> 24) & 0xffff, 0x8009);
        expect_eq!((ko_header >> 40) & 0xf, 1);

        // Verify KOID (word 1)
        expect_eq!(u64::from_ne_bytes(read_bytes[8..16].try_into().unwrap()), 200);

        // Verify Inline Name (words 2 & 3: 9 bytes padded to 16 bytes)
        expect_true!(&read_bytes[16..25] == b"test_proc");
        expect_true!(read_bytes[25..32] == [0; 7]);

        // Verify Koid Argument (words 4 & 5: arg header + koid value)
        let arg_header = u64::from_ne_bytes(read_bytes[32..40].try_into().unwrap());
        expect_eq!(arg_header & 0xf, 8); // ArgumentType::kKoid (8)
        expect_eq!((arg_header >> 4) & 0xfff, 2); // ArgumentSize = 2 words
        expect_ne!((arg_header >> 16) & 0xffff, 0); // NameStringRef
        expect_eq!(u64::from_ne_bytes(read_bytes[40..48].try_into().unwrap()), 300);
    }

    /// Verifies StringRef encoding and payload calculation for interned and inline strings.
    #[test]
    fn test_string_ref() {
        // Interned string ref
        let interned_ref = StringRef::from(DROP_STATS_REF);
        expect_eq!(interned_ref.header_entry(), DROP_STATS_REF.id());
        expect_eq!(interned_ref.payload_words(), 0);

        // Inline string ref from byte slice
        let inline_bytes_ref = StringRef::from(&b"foo"[..]);
        expect_eq!(inline_bytes_ref.header_entry(), 0x8003);
        expect_eq!(inline_bytes_ref.payload_words(), 1);

        let inline_longer_ref = StringRef::from(&b"longer_test_string"[..]);
        expect_eq!(inline_longer_ref.header_entry(), 0x8012);
        expect_eq!(inline_longer_ref.payload_words(), 3);

        // Inline string ref from &str
        let inline_str_ref = StringRef::from("hello_world");
        expect_eq!(inline_str_ref.header_entry(), 0x800b);
        expect_eq!(inline_str_ref.payload_words(), 2);
    }

    /// Verifies Koid argument wrapping and size calculation.
    #[test]
    fn test_koid_argument() {
        let koid_val = Koid(0xfeedface);
        let arg_val: ArgValue<'_> = koid_val.into();
        match arg_val {
            ArgValue::Koid(v) => {
                expect_eq!(v, 0xfeedface);
            }
            _ => panic!("Expected ArgValue::Koid"),
        }

        let arg = Argument::new(DROP_STATS_REF, koid_val);
        expect_eq!(arg.size_words(), 2);
    }

    /// Verifies argument size and header encoding across all argument types.
    #[test]
    fn test_all_argument_types() {
        let _guard = InterruptDisableGuard::new();
        let mut storage = [0u8; 1024];
        let mut inner_buf = unsafe { Buffer::from_raw_parts(storage.as_mut_ptr(), storage.len()) };
        let mut stats = DroppedRecordStats::default();
        let mut kbuf = KTraceBuffer::new(
            NonNull::new(ptr::from_mut(&mut inner_buf)).unwrap(),
            NonNull::new(ptr::from_mut(&mut stats)).unwrap(),
            1,
            100,
            200,
        );

        // Test each argument type writing via a reservation.
        let args = [
            Argument::new(DROP_STATS_REF, ArgValue::Null),
            Argument::new(DROP_STATS_REF, true),
            Argument::new(DROP_STATS_REF, -123i32),
            Argument::new(DROP_STATS_REF, 456u32),
            Argument::new(DROP_STATS_REF, -789i64),
            Argument::new(DROP_STATS_REF, 101112u64),
            Argument::new(DROP_STATS_REF, 123.456f64),
            Argument::new(DROP_STATS_REF, "hello_world"),
            Argument::new(DROP_STATS_REF, ArgValue::Pointer(0x12345678)),
            Argument::new(DROP_STATS_REF, Koid(9999)),
        ];

        let total_arg_words: usize = args.iter().map(|a| a.size_words()).sum();
        let total_words = 1 + total_arg_words; // 1 header + args
        let header = 4u64 | ((total_words as u64) << 4);

        let mut res = unwrap_ok!(kbuf.reserve(header));
        for arg in &args {
            assert_ok!(arg.write(&mut res));
        }
        assert_ok!(res.commit());

        let mut read_bytes = [0u8; 1024];
        let read_len = unwrap_ok!(kbuf.read(
            |offset, src| {
                read_bytes[offset as usize..offset as usize + src.len()].copy_from_slice(src);
                Ok(())
            },
            (total_words * 8) as u32,
        ));
        expect_eq!(read_len, (total_words * 8) as u32);

        let mut word_idx = 1; // skip record header

        // 0: Null (1 word)
        let null_hdr =
            u64::from_ne_bytes(read_bytes[word_idx * 8..(word_idx + 1) * 8].try_into().unwrap());
        expect_eq!(null_hdr & 0xf, 0); // kNull
        expect_eq!((null_hdr >> 4) & 0xfff, 1);
        word_idx += 1;

        // 1: Bool (1 word)
        let bool_hdr =
            u64::from_ne_bytes(read_bytes[word_idx * 8..(word_idx + 1) * 8].try_into().unwrap());
        expect_eq!(bool_hdr & 0xf, 9); // kBool
        expect_eq!((bool_hdr >> 4) & 0xfff, 1);
        expect_eq!((bool_hdr >> 32) & 1, 1);
        word_idx += 1;

        // 2: Int32 (1 word)
        let i32_hdr =
            u64::from_ne_bytes(read_bytes[word_idx * 8..(word_idx + 1) * 8].try_into().unwrap());
        expect_eq!(i32_hdr & 0xf, 1); // kInt32
        expect_eq!((i32_hdr >> 4) & 0xfff, 1);
        expect_eq!((i32_hdr >> 32) as u32 as i32, -123);
        word_idx += 1;

        // 3: Uint32 (1 word)
        let u32_hdr =
            u64::from_ne_bytes(read_bytes[word_idx * 8..(word_idx + 1) * 8].try_into().unwrap());
        expect_eq!(u32_hdr & 0xf, 2); // kUint32
        expect_eq!((u32_hdr >> 4) & 0xfff, 1);
        expect_eq!((u32_hdr >> 32) as u32, 456);
        word_idx += 1;

        // 4: Int64 (2 words)
        let i64_hdr =
            u64::from_ne_bytes(read_bytes[word_idx * 8..(word_idx + 1) * 8].try_into().unwrap());
        expect_eq!(i64_hdr & 0xf, 3); // kInt64
        expect_eq!((i64_hdr >> 4) & 0xfff, 2);
        let i64_val = i64::from_ne_bytes(
            read_bytes[(word_idx + 1) * 8..(word_idx + 2) * 8].try_into().unwrap(),
        );
        expect_eq!(i64_val, -789);
        word_idx += 2;

        // 5: Uint64 (2 words)
        let u64_hdr =
            u64::from_ne_bytes(read_bytes[word_idx * 8..(word_idx + 1) * 8].try_into().unwrap());
        expect_eq!(u64_hdr & 0xf, 4); // kUint64
        expect_eq!((u64_hdr >> 4) & 0xfff, 2);
        let u64_val = u64::from_ne_bytes(
            read_bytes[(word_idx + 1) * 8..(word_idx + 2) * 8].try_into().unwrap(),
        );
        expect_eq!(u64_val, 101112);
        word_idx += 2;

        // 6: Double (2 words)
        let d_hdr =
            u64::from_ne_bytes(read_bytes[word_idx * 8..(word_idx + 1) * 8].try_into().unwrap());
        expect_eq!(d_hdr & 0xf, 5); // kDouble
        expect_eq!((d_hdr >> 4) & 0xfff, 2);
        let d_bits = u64::from_ne_bytes(
            read_bytes[(word_idx + 1) * 8..(word_idx + 2) * 8].try_into().unwrap(),
        );
        expect_eq!(d_bits, 123.456f64.to_bits());
        word_idx += 2;

        // 7: String (1 header + 2 payload words = 3 words)
        let s_hdr =
            u64::from_ne_bytes(read_bytes[word_idx * 8..(word_idx + 1) * 8].try_into().unwrap());
        expect_eq!(s_hdr & 0xf, 6); // kString
        expect_eq!((s_hdr >> 4) & 0xfff, 3);
        expect_eq!((s_hdr >> 32) & 0xffff, 0x800b); // 11 bytes inline
        expect_true!(&read_bytes[(word_idx + 1) * 8..(word_idx + 1) * 8 + 11] == b"hello_world");
        word_idx += 3;

        // 8: Pointer (2 words)
        let ptr_hdr =
            u64::from_ne_bytes(read_bytes[word_idx * 8..(word_idx + 1) * 8].try_into().unwrap());
        expect_eq!(ptr_hdr & 0xf, 7); // kPointer
        expect_eq!((ptr_hdr >> 4) & 0xfff, 2);
        let ptr_val = usize::from_ne_bytes(
            read_bytes[(word_idx + 1) * 8..(word_idx + 2) * 8].try_into().unwrap(),
        );
        expect_eq!(ptr_val, 0x12345678);
        word_idx += 2;

        // 9: Koid (2 words)
        let koid_hdr =
            u64::from_ne_bytes(read_bytes[word_idx * 8..(word_idx + 1) * 8].try_into().unwrap());
        expect_eq!(koid_hdr & 0xf, 8); // kKoid
        expect_eq!((koid_hdr >> 4) & 0xfff, 2);
        let koid_val = u64::from_ne_bytes(
            read_bytes[(word_idx + 1) * 8..(word_idx + 2) * 8].try_into().unwrap(),
        );
        expect_eq!(koid_val, 9999);
        word_idx += 2;

        expect_eq!(word_idx, total_words);
    }

    /// A scope with arguments can be bound to a local variable.
    #[test]
    fn scope_owns_arguments() {
        let value = timer_current_boot_ticks().0 as u64;
        let scope = begin_scope!(META_CAT, "scope_owns_arguments", "val" => value);

        expect_ne!(scope.timestamp.0, 0);
        expect_eq!(scope.args.len(), 1);
        match scope.args[0].value {
            ArgValue::Uint64(v) => {
                expect_eq!(v, value);
            }
            _ => panic!("Expected ArgValue::Uint64"),
        }

        // Dropping `scope` here emits the complete event from the arguments it owns.
    }

    /// Verifies bitfield layout construction and encoding.
    #[test]
    fn test_bitfield_layouts() {
        // RecordHeader
        let mut rec = RecordHeader::new();
        rec.set_record_size(12).set_record_type(RecordType::Event);
        expect_eq!(rec.record_type(), RecordType::Event);
        expect_eq!(rec.record_size(), 12);
        let rec_raw = rec.bits();
        expect_eq!(rec_raw & 0xf, 4);
        expect_eq!((rec_raw >> 4) & 0xfff, 12);

        // LargeRecordHeader
        let mut lrec = LargeRecordHeader::default();
        lrec.set_record_size(0x1000).set_large_type(2);
        expect_eq!(lrec.record_type(), RecordType::LargeBlob);
        expect_eq!(lrec.record_size(), 0x1000);
        expect_eq!(lrec.large_type(), 2);
        let lrec_raw = lrec.bits();
        expect_eq!(lrec_raw & 0xf, 15);
        expect_eq!((lrec_raw >> 4) & 0xffffffff, 0x1000);
        expect_eq!((lrec_raw >> 36) & 0xf, 2);

        // StringRefHeader
        let mut interned = StringRefHeader::new();
        interned.set_id_or_len(42);
        expect_eq!(interned.id_or_len(), 42);
        expect_false!(interned.is_inline());
        expect_eq!(interned.bits(), 42);

        let mut inline = StringRefHeader::new();
        inline.set_is_inline(true).set_id_or_len(15);
        expect_true!(inline.is_inline());
        expect_eq!(inline.id_or_len(), 15);
        expect_eq!(inline.bits(), 0x800f);

        // KernelObjectRecordHeader
        let mut ko = KernelObjectRecordHeader::default();
        ko.set_record_size(6).set_obj_type(1).set_name_ref(0x8009).set_arg_count(1);
        expect_eq!(ko.record_type(), RecordType::KernelObject);
        expect_eq!(ko.record_size(), 6);
        expect_eq!(ko.obj_type(), 1);
        expect_eq!(ko.name_ref(), 0x8009);
        expect_eq!(ko.arg_count(), 1);
        let ko_raw = ko.bits();
        expect_eq!(ko_raw & 0xf, 7);
        expect_eq!((ko_raw >> 4) & 0xfff, 6);
        expect_eq!((ko_raw >> 16) & 0xff, 1);
        expect_eq!((ko_raw >> 24) & 0xffff, 0x8009);
        expect_eq!((ko_raw >> 40) & 0xf, 1);

        // EventRecordHeader
        let mut ev = EventRecordHeader::default();
        ev.set_record_size(5)
            .set_event_type(EventType::Instant)
            .set_arg_count(1)
            .set_thread_ref(0)
            .set_category_ref(10)
            .set_name_ref(20);
        expect_eq!(ev.record_type(), RecordType::Event);
        expect_eq!(ev.record_size(), 5);
        expect_eq!(ev.event_type(), EventType::Instant);
        expect_eq!(ev.arg_count(), 1);
        expect_eq!(ev.thread_ref(), 0);
        expect_eq!(ev.category_ref(), 10);
        expect_eq!(ev.name_ref(), 20);
        let ev_raw = ev.bits();
        expect_eq!(ev_raw & 0xf, 4);
        expect_eq!((ev_raw >> 4) & 0xfff, 5);
        expect_eq!((ev_raw >> 16) & 0xf, 0);
        expect_eq!((ev_raw >> 20) & 0xf, 1);
        expect_eq!((ev_raw >> 24) & 0xff, 0);
        expect_eq!((ev_raw >> 32) & 0xffff, 10);
        expect_eq!((ev_raw >> 48) & 0xffff, 20);
    }
}

//
// Zircon C++ kernel-tests FFI tests.
//

/// Test-only FFI helper to write a single word record from Rust.
///
/// # Safety
///
/// This must be called with interrupts disabled.
#[cfg(ktest)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_ktrace_test_interop(header: u64, val: u64) -> i32 {
    let ktrace = KTrace::get_instance();
    // SAFETY: The caller guarantees interrupts are disabled.
    if let Ok(mut res) = unsafe { ktrace.reserve(header) } {
        let _ = res.write_word(val);
        let _ = res.commit();
        0
    } else {
        -1
    }
}

/// Test-only FFI helper to exercise all ktrace macros from Rust.
///
/// # Safety
///
/// This must be called with interrupts disabled and KTrace active.
#[cfg(ktest)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_ktrace_test_macros() {
    // Exercise each macro. We'll write specific, distinguishable events.
    instant!("kernel:meta", "rust_instant", Context::Thread, "val" => 101u32);
    duration_begin!("kernel:meta", "rust_duration", Context::Thread, "val" => 102u32);
    duration_end!("kernel:meta", "rust_duration", Context::Thread, "val" => 103u32);
    counter!("kernel:meta", "rust_counter", 104u64, "val" => 105u32);
    flow_begin!("kernel:meta", "rust_flow", 106u64, "val" => 107u32);
    flow_step!("kernel:meta", "rust_flow", 106u64, "val" => 108u32);
    flow_end!("kernel:meta", "rust_flow", 106u64, "val" => 109u32);
    complete!("kernel:meta", "rust_complete", InstantBootTicks(110i64), "val" => 111u32);
    kernel_object!("kernel:meta", 112u64, 1u32, "rust_kernel_obj", "val" => 113u32);
    kernel_object_always!(114u64, 2u32, "rust_kernel_obj_always", "val" => 115u32);
    kernel_object!("kernel:meta", 116u64, 1u32, &b"dynamic_proc"[..], "job" => Koid(117u64));
}
