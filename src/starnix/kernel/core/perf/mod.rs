// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mm::PAGE_SIZE;
use crate::task::dynamic_thread_spawner::SpawnRequestBuilder;
use anyhow::Context;
use fidl_fuchsia_cpu_profiler as profiler;
use fuchsia_component::client::connect_to_protocol;
use fuchsia_runtime;
use futures::StreamExt;
use futures::channel::mpsc as future_mpsc;
use std::collections::HashMap;
use std::error::Error;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, mpsc as sync_mpsc};
use zerocopy::{Immutable, IntoBytes};

use futures::io::{AsyncReadExt, Cursor};
use fxt::TraceRecord;
use fxt::profiler::ProfilerRecord;
use fxt::session::SessionParser;
use seq_lock::{SeqLock, SeqLockable, WriteSize};
use starnix_logging::{log_error, log_info, log_warn, track_stub};
use starnix_sync::{LockDepMutex, LockDepRwLock, PerfEventLevel, PerfFormatIdLookupTableLock};
use starnix_syscalls::{SUCCESS, SyscallArg, SyscallResult};
use starnix_uapi::arch32::{
    PERF_EVENT_IOC_DISABLE, PERF_EVENT_IOC_ENABLE, PERF_EVENT_IOC_ID,
    PERF_EVENT_IOC_MODIFY_ATTRIBUTES, PERF_EVENT_IOC_PAUSE_OUTPUT, PERF_EVENT_IOC_PERIOD,
    PERF_EVENT_IOC_QUERY_BPF, PERF_EVENT_IOC_REFRESH, PERF_EVENT_IOC_RESET, PERF_EVENT_IOC_SET_BPF,
    PERF_EVENT_IOC_SET_FILTER, PERF_EVENT_IOC_SET_OUTPUT, PERF_RECORD_MISC_USER,
    perf_event_sample_format_PERF_SAMPLE_CALLCHAIN, perf_event_sample_format_PERF_SAMPLE_ID,
    perf_event_sample_format_PERF_SAMPLE_IDENTIFIER, perf_event_sample_format_PERF_SAMPLE_IP,
    perf_event_sample_format_PERF_SAMPLE_PERIOD, perf_event_sample_format_PERF_SAMPLE_READ,
    perf_event_sample_format_PERF_SAMPLE_REGS_USER,
    perf_event_sample_format_PERF_SAMPLE_STACK_USER, perf_event_sample_format_PERF_SAMPLE_TID,
    perf_event_sample_format_PERF_SAMPLE_TIME, perf_event_type_PERF_RECORD_LOST,
    perf_event_type_PERF_RECORD_SAMPLE,
};
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::uapi::{
    perf_sample_regs_abi_PERF_SAMPLE_REGS_ABI_32, perf_sample_regs_abi_PERF_SAMPLE_REGS_ABI_64,
    perf_sample_regs_abi_PERF_SAMPLE_REGS_ABI_NONE,
};
use starnix_uapi::user_address::UserRef;
use starnix_uapi::{
    errno, error, from_status_like_fdio, perf_event_attr, perf_event_header,
    perf_event_mmap_page__bindgen_ty_1, perf_event_read_format_PERF_FORMAT_GROUP,
    perf_event_read_format_PERF_FORMAT_ID, perf_event_read_format_PERF_FORMAT_LOST,
    perf_event_read_format_PERF_FORMAT_TOTAL_TIME_ENABLED,
    perf_event_read_format_PERF_FORMAT_TOTAL_TIME_RUNNING, tid_t, uapi,
};

use crate::security::{self, TargetTaskType};
use crate::task::Kernel;
use crate::task::tracing::{LinuxIdentity, PidKoidSession};

static READ_FORMAT_ID_GENERATOR: AtomicU64 = AtomicU64::new(0);
// Size of the VMO backing each event's metadata page and ring buffer; mmap
// lengths up to this size are accepted. perf readers map one metadata page
// plus a power-of-two data area, and commonly ask for megabytes (e.g. 256
// data pages or more), so leave generous headroom. With circular writes the
// data area no longer needs to hold a whole session's records.
//
// We currently preallocate a fixed size for the VMO because we do not dynamically
// resize it when mapped.
// TODO(https://fxbug.dev/540986386): Support dynamic resizing or pass the requested size.
const ESTIMATED_MMAP_BUFFER_SIZE: u64 = 16 * 1024 * 1024;
// Size of a PERF_RECORD_LOST record in bytes:
// perf_event_header (8) + sample_id (8) + lost_events (8) = 24.
const LOST_RECORD_SIZE: u64 = 24;
// FXT magic bytes (little endian).
const FXT_MAGIC_BYTES: [u8; 8] = [0x10, 0x00, 0x04, 0x46, 0x78, 0x54, 0x16, 0x00];
// Register indices in the profiler's register capture block.
const AARCH64_REG_PC: usize = 32;
const AARCH64_REG_SP: usize = 31;
const AARCH32_REG_R15: usize = 15; // AArch32 PC
const AARCH32_REG_R13: usize = 13; // AArch32 SP

mod event;
pub use event::{TraceEvent, TraceEventQueue, TraceEventQueueList};

pub mod lockless_ring_buffer;

#[repr(C)]
#[derive(Copy, Clone, IntoBytes, Immutable)]
struct LostRecord {
    header: perf_event_header,
    sample_id: u64,
    lost_events: u64,
}

#[repr(C)]
#[derive(Copy, Clone, IntoBytes, Immutable)]
struct PerfMetadataHeader {
    version: u32,
    compat_version: u32,
}

#[repr(C)]
#[derive(Copy, Clone, IntoBytes, Immutable)]
struct PerfMetadataValue {
    lock: u32,
    index: u32,
    offset: i64,
    time_enabled: u64,
    time_running: u64,
    __bindgen_anon_1: perf_event_mmap_page__bindgen_ty_1,
    pmc_width: u16,
    time_shift: u16,
    time_mult: u32,
    time_offset: u64,
    time_zero: u64,
    size: u32,
    __reserved_1: u32,
    time_cycles: u64,
    time_mask: u64,
    __reserved: [u8; 928usize],
    data_head: u64,
    data_tail: u64,
    data_offset: u64,
    data_size: u64,
    aux_head: u64,
    aux_tail: u64,
    aux_offset: u64,
    aux_size: u64,
}

// SAFETY: `PerfMetadataValue` can be safely written to shared memory in 8-byte chunks.
// This is because it is composed of two u32s followed by only u64s.
// The first u32 is the `lock` field, which is why HAS_INLINE_SEQUENCE is true.
unsafe impl SeqLockable for PerfMetadataValue {
    const WRITE_SIZE: WriteSize = WriteSize::Eight;
    const HAS_INLINE_SEQUENCE: bool = true;
    const VMO_NAME: &'static [u8] = b"starnix:perf_event";
}

struct PerfState {
    // This table maps a group leader's file object id to its unique u64 "format ID".
    //
    // When a sample is generated for any event in a group, we use this
    // "format ID" from the group leader as the value for *both* the
    // `PERF_SAMPLE_ID` and `PERF_SAMPLE_IDENTIFIER` fields.
    format_id_lookup_table: LockDepMutex<HashMap<FileObjectId, u64>, PerfFormatIdLookupTableLock>,
}

impl Default for PerfState {
    fn default() -> Self {
        Self { format_id_lookup_table: Default::default() }
    }
}

fn get_perf_state(kernel: &Arc<Kernel>) -> Arc<PerfState> {
    kernel.expando.get_or_init(PerfState::default)
}

uapi::check_arch_independent_layout! {
    perf_event_attr {
        type_, // "type" is a reserved keyword so add a trailing underscore.
        size,
        config,
        __bindgen_anon_1,
        sample_type,
        read_format,
        _bitfield_1,
        __bindgen_anon_2,
        bp_type,
        __bindgen_anon_3,
        __bindgen_anon_4,
        branch_sample_type,
        sample_regs_user,
        sample_stack_user,
        clockid,
        sample_regs_intr,
        aux_watermark,
        sample_max_stack,
        __reserved_2,
        aux_sample_size,
        __reserved_3,
        sig_data,
        config3,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum IoctlOp {
    Enable,
    Disable,
}

struct PerfEventFileState {
    attr: perf_event_attr,
    rf_value: u64, // "count" for the config we passed in for the event.
    // The most recent timestamp (ns) where we changed into an enabled state
    // i.e. the most recent time we got an ENABLE ioctl().
    most_recent_enabled_time: u64,
    // Sum of all previous enablement segment durations (ns). If we are
    // currently in an enabled state, explicitly does NOT include the current
    // segment.
    total_time_running: u64,
    rf_id: u64,
    sample_id: u64,
    _rf_lost: u64,
    disabled: u64,
    sample_type: u64,
    // Handle to blob that stores all the perf data that a user may want.
    // At the moment it only stores some metadata and backtraces (bts).
    perf_data_vmo: zx::Vmo,
    // Channel used to send IoctlOps to start/stop sampling.
    ioctl_sender: future_mpsc::Sender<(IoctlOp, sync_mpsc::Sender<()>)>,
}

// Have an implementation for PerfEventFileState because VMO
// doesn't have Default so we can't derive it.
impl PerfEventFileState {
    fn new(
        attr: perf_event_attr,
        rf_value: u64,
        disabled: u64,
        sample_type: u64,
        perf_data_vmo: zx::Vmo,
        ioctl_sender: future_mpsc::Sender<(IoctlOp, sync_mpsc::Sender<()>)>,
    ) -> PerfEventFileState {
        PerfEventFileState {
            attr,
            rf_value,
            most_recent_enabled_time: 0,
            total_time_running: 0,
            rf_id: 0,
            sample_id: 0,
            _rf_lost: 0,
            disabled,
            sample_type,
            perf_data_vmo,
            ioctl_sender,
        }
    }
}

pub struct PerfEventFile {
    _tid: tid_t,
    _cpu: i32,
    perf_event_file: LockDepRwLock<PerfEventFileState, PerfEventLevel>,
    // The security state for this PerfEventFile.
    pub security_state: security::PerfEventState,
    seq_lock: Arc<OnceLock<Result<SeqLock<PerfMetadataHeader, PerfMetadataValue>, Errno>>>,
}

// PerfEventFile object that implements FileOps.
// See https://man7.org/linux/man-pages/man2/perf_event_open.2.html for
// implementation details.
// This object can be saved as a FileDescriptor.
impl FileOps for PerfEventFile {
    // Don't need to implement seek or sync for PerfEventFile.
    fileops_impl_nonseekable!();
    fileops_impl_noop_sync!();

    fn close(self: Box<Self>, file: &FileObjectState, current_task: &CurrentTask) {
        {
            let mut perf_event_file = self.perf_event_file.write();
            // Ensure we disable so we clean up resources if the user closes without disabling.
            perf_event_file.disabled = 1;
            ping_receiver(perf_event_file.ioctl_sender.clone(), IoctlOp::Disable);
        }
        let perf_state = get_perf_state(&current_task.kernel);
        let mut events = perf_state.format_id_lookup_table.lock();
        events.remove(&file.id);
    }

    // See "Reading results" section of https://man7.org/linux/man-pages/man2/perf_event_open.2.html.
    fn read(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        _offset: usize,
        data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        // Create/calculate and return the ReadFormatData object.
        // If we create it earlier we might want to change it and it's immutable once created.
        let read_format_data = {
            // Once we get the `value` or count from kernel, we can change this to a read()
            // call instead of write().
            let mut perf_event_file = self.perf_event_file.write();

            security::check_perf_event_read_access(current_task, &self)?;

            let mut total_time_running_including_curr = perf_event_file.total_time_running;

            // Only update values if enabled (either by perf_event_attr or ioctl ENABLE call).
            if perf_event_file.disabled == 0 {
                // Calculate the value or "count" of the config we're interested in.
                // This value should reflect the value we are counting (defined in the config).
                // E.g. for PERF_COUNT_SW_CPU_CLOCK it would return the value from the CPU clock.
                // For now we just return rf_value + 1.
                track_stub!(
                    TODO("https://fxbug.dev/402938671"),
                    "[perf_event_open] implement read_format value"
                );
                perf_event_file.rf_value += 1;

                // Update time duration.
                let curr_time = zx::MonotonicInstant::get().into_nanos() as u64;
                total_time_running_including_curr +=
                    curr_time - perf_event_file.most_recent_enabled_time;
            }

            let mut output = Vec::<u8>::new();
            let value = perf_event_file.rf_value.to_ne_bytes();
            output.extend(value);

            let read_format = perf_event_file.attr.read_format;

            if (read_format & perf_event_read_format_PERF_FORMAT_TOTAL_TIME_ENABLED as u64) != 0 {
                // Total time (ns) event was enabled and running (currently same as TIME_RUNNING).
                output.extend(total_time_running_including_curr.to_ne_bytes());
            }
            if (read_format & perf_event_read_format_PERF_FORMAT_TOTAL_TIME_RUNNING as u64) != 0 {
                // Total time (ns) event was enabled and running (currently same as TIME_ENABLED).
                output.extend(total_time_running_including_curr.to_ne_bytes());
            }
            if (read_format & perf_event_read_format_PERF_FORMAT_ID as u64) != 0 {
                // Adds a 64-bit unique value that corresponds to the event group.
                output.extend(perf_event_file.rf_id.to_ne_bytes());
            }

            output
        };

        // The regular read() call allows the case where the bytes-we-want-to-read-in won't
        // fit in the output buffer. However, for perf_event_open's read(), "If you attempt to read
        // into a buffer that is not big enough to hold the data, the error ENOSPC results."
        if data.available() < read_format_data.len() {
            return error!(ENOSPC);
        }
        track_stub!(
            TODO("https://fxbug.dev/402453955"),
            "[perf_event_open] implement remaining error handling"
        );

        data.write(&read_format_data)
    }

    fn ioctl(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        op: u32,
        _arg: SyscallArg,
    ) -> Result<SyscallResult, Errno> {
        track_stub!(
            TODO("https://fxbug.dev/405463320"),
            "[perf_event_open] implement PERF_IOC_FLAG_GROUP"
        );
        security::check_perf_event_write_access(current_task, &self)?;
        let mut perf_event_file = self.perf_event_file.write();
        match op {
            PERF_EVENT_IOC_ENABLE => {
                if perf_event_file.disabled != 0 {
                    perf_event_file.disabled = 0; // 0 = false.
                    perf_event_file.most_recent_enabled_time =
                        zx::MonotonicInstant::get().into_nanos() as u64;
                }

                // If we are sampling, invoke the profiler and collect a sample.
                // Currently this is an example sample collection.
                track_stub!(
                    TODO("https://fxbug.dev/398914921"),
                    "[perf_event_open] implement full sampling features"
                );
                if perf_event_file.attr.freq() == 0
                // SAFETY: sample_period is a u64 field in a union with u64 sample_freq.
                // This is always sound regardless of the union's tag.
                    && unsafe { perf_event_file.attr.__bindgen_anon_1.sample_period != 0 }
                {
                    ping_receiver(perf_event_file.ioctl_sender.clone(), IoctlOp::Enable);
                }
                return Ok(SUCCESS);
            }
            PERF_EVENT_IOC_DISABLE => {
                if perf_event_file.disabled == 0 {
                    perf_event_file.disabled = 1; // 1 = true.

                    // Update total_time_running now that the segment has ended.
                    let curr_time = zx::MonotonicInstant::get().into_nanos() as u64;
                    perf_event_file.total_time_running +=
                        curr_time - perf_event_file.most_recent_enabled_time;
                }
                if perf_event_file.attr.freq() == 0
                // SAFETY: sample_period is a u64 field in a union with u64 sample_freq.
                // This is always sound regardless of the union's tag.
                    && unsafe { perf_event_file.attr.__bindgen_anon_1.sample_period != 0 }
                {
                    ping_receiver(perf_event_file.ioctl_sender.clone(), IoctlOp::Disable);
                }
                return Ok(SUCCESS);
            }
            PERF_EVENT_IOC_RESET => {
                perf_event_file.rf_value = 0;
                return Ok(SUCCESS);
            }
            PERF_EVENT_IOC_REFRESH
            | PERF_EVENT_IOC_PERIOD
            | PERF_EVENT_IOC_SET_OUTPUT
            | PERF_EVENT_IOC_SET_FILTER
            | PERF_EVENT_IOC_ID
            | PERF_EVENT_IOC_SET_BPF
            | PERF_EVENT_IOC_PAUSE_OUTPUT
            | PERF_EVENT_IOC_MODIFY_ATTRIBUTES
            | PERF_EVENT_IOC_QUERY_BPF => {
                track_stub!(
                    TODO("https://fxbug.dev/404941053"),
                    "[perf_event_open] implement remaining ioctl() calls"
                );
                return error!(ENOSYS);
            }
            _ => error!(ENOTTY),
        }
    }

    // TODO(https://fxbug.dev/460245383) match behavior when mmap() is called multiple times.
    // Gets called when mmap() is called.
    // Immediately before sampling, this should get called by the user (e.g. the test
    // or Perfetto). We will then write the metadata to the VMO and return the pointer to it.
    fn get_memory(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        length: Option<usize>,
        _prot: ProtectionFlags,
    ) -> Result<Arc<MemoryObject>, Errno> {
        let buffer_size: u64 = length.unwrap_or(0) as u64;
        let page_size = zx::system_get_page_size() as u64;
        if buffer_size <= page_size || buffer_size > ESTIMATED_MMAP_BUFFER_SIZE {
            return error!(EINVAL);
        }
        let data_size = buffer_size - page_size;
        if !data_size.is_power_of_two() {
            return error!(EINVAL);
        }

        self.seq_lock
            .get_or_init(|| {
                let perf_event_file = self.perf_event_file.read();
                let vmo_copy = perf_event_file
                    .perf_data_vmo
                    .as_handle_ref()
                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                    .map_err(|status| from_status_like_fdio!(status))?;
                // SAFETY: See safety requirements on `create_seq_lock`.
                Ok(unsafe { create_seq_lock(&vmo_copy, buffer_size) })
            })
            .as_ref()
            .map_err(|e| e.clone())?;

        // Write to a MemoryObject and return it (expected return type for get_memory()).
        security::check_perf_event_read_access(current_task, &self)?;
        let perf_event_file = self.perf_event_file.read();
        match perf_event_file
            .perf_data_vmo
            .as_handle_ref()
            .duplicate_handle(zx::Rights::SAME_RIGHTS)
        {
            Ok(vmo) => {
                let vmo: zx::Vmo = vmo.into();
                let memory = MemoryObject::from(vmo);
                return Ok(Arc::new(memory));
            }
            Err(_) => {
                track_stub!(
                    TODO("https://fxbug.dev/416323134"),
                    "[perf_event_open] handle get_memory() errors"
                );
                return error!(EINVAL);
            }
        };
    }

    fn write(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        _data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        track_stub!(
            TODO("https://fxbug.dev/394960158"),
            "[perf_event_open] implement perf event functions"
        );
        error!(ENOSYS)
    }
}

// Given a PerfRecordSample struct, write it via the correct output format
// (per https://man7.org/linux/man-pages/man2/perf_event_open.2.html) to the VMO.
// We don't currently support all the sample_types listed in the docs.
// Input:
//    PerfRecordSample { pid: 5, tid: 10, nr: 3, ips[nr]: [111, 222, 333] }
// Human-understandable output:
//    9 1 40 111 5 10 3 111 222 333
// Actual output (no spaces or \n in real output, just making it more readable):
//    0x0000 0x0009                 <-- starts at `offset` bytes
//    0x0001
//    0x0040
//    0x0000 0x0000 0x0000 0x006F   <-- starts at `offset` + 8 bytes
//    0x0000 0x0000 0x0000 0x0005
//    0x0000 0x0000 0x0000 0x0010
//    0x0000 0x0000 0x0000 0x0003
//    0x0000 0x0000 0x0000 0x006F
//    0x0000 0x0000 0x0000 0x00DE
//    0x0000 0x0000 0x0000 0x014D
//
//    Returns the length of bytes written. In above case, 8 + 28 = 36.
//    This information is used to increment the global offset.
//
//    If writing to the VMO fails, we log a warning and return the number of
//    bytes successfully written so far (e.g. only the LOST record if that
//    succeeded, or 0). The caller uses this returned length to increment the
//    VMO write offset and update `data_head`, meaning failed writes are
//    effectively skipped and not exposed to the reader.
fn write_record_to_vmo(
    perf_record_sample: PerfRecordSample<'_>,
    perf_data_vmo: &zx::Vmo,
    sample_type: u64,
    sample_id: u64,
    sample_period: u64,
    read_format: u64,
    head: u64,
    metadata: &PerfMetadataValue,
    lost_events: &mut u64,
) -> u64 {
    let ring_buffer_size = metadata.data_size;
    if ring_buffer_size == 0 {
        return 0;
    }
    // First, build record to determine its size (so that we can fill out `size` in header).
    let mut sample = Vec::<u8>::new();
    // sample_id
    if (sample_type & perf_event_sample_format_PERF_SAMPLE_IDENTIFIER as u64) != 0 {
        sample.extend(sample_id.to_ne_bytes());
    }
    // ip
    if (sample_type & perf_event_sample_format_PERF_SAMPLE_IP as u64) != 0 {
        let ip = perf_record_sample.ips.first().copied().unwrap_or(0);
        sample.extend(ip.to_ne_bytes());
    }

    if (sample_type & perf_event_sample_format_PERF_SAMPLE_TID as u64) != 0 {
        // pid
        sample.extend(perf_record_sample.pid.unwrap_or(0).to_ne_bytes());
        // tid
        sample.extend(perf_record_sample.tid.unwrap_or(0).to_ne_bytes());
    }

    // time: when the sample was taken.
    if (sample_type & perf_event_sample_format_PERF_SAMPLE_TIME as u64) != 0 {
        sample.extend((perf_record_sample.time.into_nanos() as u64).to_ne_bytes());
    }

    // id
    if (sample_type & perf_event_sample_format_PERF_SAMPLE_ID as u64) != 0 {
        sample.extend(sample_id.to_ne_bytes());
    }

    // sample period
    if (sample_type & perf_event_sample_format_PERF_SAMPLE_PERIOD as u64) != 0 {
        sample.extend(sample_period.to_ne_bytes());
    }

    // read_format value.
    if (sample_type & perf_event_sample_format_PERF_SAMPLE_READ as u64) != 0 {
        if (read_format & perf_event_read_format_PERF_FORMAT_GROUP as u64) != 0 {
            // Group reads start with the number of events followed by each
            // event's value; only the timebase event exists.
            sample.extend(1u64.to_ne_bytes());
            sample.extend(0u64.to_ne_bytes());
        } else {
            sample.extend(0u64.to_ne_bytes());
        }
    }

    if (sample_type & perf_event_sample_format_PERF_SAMPLE_CALLCHAIN as u64) != 0 {
        // nr
        sample.extend(perf_record_sample.ips.len().to_ne_bytes());

        // ips[nr] - list of ips, u64 per ip.
        for i in perf_record_sample.ips {
            sample.extend(i.to_ne_bytes());
        }
    }

    // User registers (PERF_SAMPLE_REGS_USER): the ABI tag, then one u64 per
    // bit set in attr.sample_regs_user, in perf register-index order.
    // Readers request the mask for their own architecture -- a 64-bit reader
    // requests the arm64 mask even when profiling 32-bit tasks (and relocates
    // the PC from the PERF_REG_ARM64_PC index itself, matching the Linux
    // kernel's behavior of capturing native registers).
    // If the mask is 0 or the ABI is PERF_SAMPLE_REGS_ABI_NONE, no register
    // values are output.
    if (sample_type & perf_event_sample_format_PERF_SAMPLE_REGS_USER as u64) != 0 {
        if perf_record_sample.regs_abi == perf_sample_regs_abi_PERF_SAMPLE_REGS_ABI_NONE as u64
            || perf_record_sample.sample_regs_user == 0
            || perf_record_sample.regs.is_empty()
        {
            sample.extend((perf_sample_regs_abi_PERF_SAMPLE_REGS_ABI_NONE as u64).to_ne_bytes());
        } else {
            sample.extend(perf_record_sample.regs_abi.to_ne_bytes());
            sample.extend_from_slice(perf_record_sample.regs);
        }
    }

    // User stack (PERF_SAMPLE_STACK_USER): size, the raw bytes starting at
    // the sampled stack pointer, then the filled size. Readers overlay these
    // bytes at the SP reported in REGS_USER, which is why the snapshot must
    // begin exactly at it.
    if (sample_type & perf_event_sample_format_PERF_SAMPLE_STACK_USER as u64) != 0 {
        let fixed_len = std::mem::size_of::<perf_event_header>() + sample.len() + 16;
        let header_space = (u16::MAX as usize).saturating_sub(fixed_len) & !7;
        let requested = (perf_record_sample.sample_stack_user as usize) & !7;
        let dest_len = requested.min(header_space);

        sample.extend((dest_len as u64).to_ne_bytes());
        if dest_len > 0 {
            let data_len = perf_record_sample.stack.len().min(dest_len) & !7;
            sample.extend_from_slice(&perf_record_sample.stack[..data_len]);
            // Pad the rest with zeros.
            sample.resize(sample.len() + (dest_len - data_len), 0);
            // dyn_size
            sample.extend((data_len as u64).to_ne_bytes());
        }
    }
    // The remaining sample_type fields are not implemented.

    // Now that we know the sample size, we can calculate the record size.
    // record_size = perf_event_header_size + sample_size.
    // perf_event_header is defined to be 8 bytes.
    let record_len = std::mem::size_of::<perf_event_header>() + sample.len();
    // Every field above is a u64 (or a u32 pair), so record sizes are always
    // multiples of 8: ring positions stay 8-byte aligned. Since the
    // perf_event_header is also 8 bytes, it can never straddle the end of
    // the data area (which is page-aligned), meaning readers can always
    // read the header contiguously.
    if record_len % 8 != 0 {
        log_error!("Record length {} is not 8-byte aligned, dropping", record_len);
        *lost_events += 1;
        return 0;
    }
    // The header's size field is a u16. A record that exceeds it would
    // silently wrap the size and desynchronize every record after it, so
    // drop it instead.
    let Ok(record_size) = u16::try_from(record_len) else {
        log_warn!("Dropping {} byte perf sample record: exceeds u16 record size", record_len);
        *lost_events += 1;
        return 0;
    };

    let perf_event_header = perf_event_header {
        // These are samples of user-space execution; readers take the
        // sample's cpu mode from the misc bits.
        type_: perf_event_type_PERF_RECORD_SAMPLE,
        misc: PERF_RECORD_MISC_USER as u16,
        size: record_size,
    };

    // data_tail is advanced by userspace as it consumes records and is untrusted.
    // Calculate free space using the standard circular buffer formula, matching Linux's
    // CIRC_SPACE. This naturally handles wrap-around and invalid future tails.
    // ring_buffer_size is guaranteed to be a power of two by checks in get_memory().
    let free_space =
        (metadata.data_tail.wrapping_sub(head).wrapping_sub(1)) & (ring_buffer_size - 1);

    // Drop the sample if the ring buffer is full, matching Linux's
    // non-overwrite mode; the drop is reported via PERF_RECORD_LOST once
    // space frees up.
    if free_space < record_len as u64 {
        *lost_events += 1;
        return 0;
    }

    let mut bytes_written: u64 = 0;

    // If records were dropped earlier, surface a PERF_RECORD_LOST record as
    // soon as there is room for it alongside the current sample.
    if *lost_events > 0 && free_space >= record_len as u64 + LOST_RECORD_SIZE {
        let lost_header = perf_event_header {
            type_: perf_event_type_PERF_RECORD_LOST,
            misc: 0,
            size: LOST_RECORD_SIZE as u16,
        };
        let lost_record = LostRecord { header: lost_header, sample_id, lost_events: *lost_events };
        if write_circular(
            perf_data_vmo,
            metadata.data_offset,
            ring_buffer_size,
            head,
            lost_record.as_bytes(),
        )
        .is_ok()
        {
            *lost_events = 0;
            bytes_written += LOST_RECORD_SIZE;
        }
    }

    let mut record = Vec::with_capacity(record_len);
    record.extend_from_slice(perf_event_header.as_bytes());
    record.extend_from_slice(&sample);

    match write_circular(
        perf_data_vmo,
        metadata.data_offset,
        ring_buffer_size,
        head + bytes_written,
        &record,
    ) {
        // Return the total size we wrote so the caller can advance data_head.
        Ok(()) => bytes_written + record_len as u64,
        Err(e) => {
            log_warn!("Failed to write PerfRecordSample to VMO due to: {}", e);
            bytes_written
        }
    }
}

// Writes `data` into the ring buffer's data area at the position
// corresponding to `head` (a free-running count of bytes ever written),
// splitting the write across the end of the data area when it wraps around.
// Readers read records by checking the header size, reading contiguously,
// and wrapping around to the beginning of the data area if the record is split.
fn write_circular(
    vmo: &zx::Vmo,
    data_offset: u64,
    ring_buffer_size: u64,
    head: u64,
    data: &[u8],
) -> Result<(), zx::Status> {
    let position = head % ring_buffer_size;
    let vmo_offset = data_offset + position;
    if position + data.len() as u64 <= ring_buffer_size {
        vmo.write(data, vmo_offset)?;
    } else {
        let first_len = (ring_buffer_size - position) as usize;
        vmo.write(&data[..first_len], vmo_offset)?;
        vmo.write(&data[first_len..], data_offset)?;
    }
    Ok(())
}

/// Represents a PERF_RECORD_SAMPLE payload to be serialized into the VMO ring buffer.
/// Fields follow the perf ABI order: TID, TIME, ID, PERIOD, READ, CALLCHAIN, REGS_USER, STACK_USER.
#[derive(Debug, Clone)]
struct PerfRecordSample<'a> {
    pid: Option<u32>,
    tid: Option<u32>,
    // Timestamp of when the sample was taken, for PERF_SAMPLE_TIME.
    time: zx::BootInstant,
    // Instruction pointers (currently this is the address). First one is `ip` param.
    ips: Vec<u64>,
    regs: &'a [u8],
    stack: &'a [u8],
    // PERF_SAMPLE_REGS_ABI_32 (1) or PERF_SAMPLE_REGS_ABI_64 (2) for `regs`.
    regs_abi: u64,
    sample_regs_user: u64,
    sample_stack_user: u64,
}

async fn set_up_profiler(
    sample_period: zx::MonotonicDuration,
) -> Result<(profiler::SessionProxy, fidl::AsyncSocket), Errno> {
    // Configuration for how we want to sample.
    let sample = profiler::Sample {
        callgraph: Some(profiler::CallgraphConfig {
            strategy: Some(profiler::CallgraphStrategy::FramePointer),
            ..Default::default()
        }),
        ..Default::default()
    };

    let sampling_config = profiler::SamplingConfig {
        period: Some(sample_period.into_nanos() as u64),
        timebase: Some(profiler::Counter::PlatformIndependent(profiler::CounterId::Nanoseconds)),
        sample: Some(sample),
        ..Default::default()
    };

    track_stub!(
        TODO("https://fxbug.dev/398914921"),
        "[perf_event_open] allow for profiling system-wide not during tests"
    );
    let job = fuchsia_runtime::job_default();
    let koid = job.koid().map_err(|e| errno!(EINVAL, e.to_string()))?;
    let tasks = vec![
        // Should return ~1300 samples for 1000 millis.
        profiler::Task::Job(koid.raw_koid()),
    ];
    let targets = profiler::TargetConfig::Tasks(tasks);
    let config = profiler::Config {
        configs: Some(vec![sampling_config]),
        target: Some(targets),
        ..Default::default()
    };
    let (client, server) = fidl::Socket::create_stream();
    let configure = profiler::SessionConfigureRequest {
        output: Some(server),
        config: Some(config),
        ..Default::default()
    };

    let proxy = connect_to_protocol::<profiler::SessionMarker>()
        .context("Error connecting to Profiler protocol");
    let session_proxy: profiler::SessionProxy = match proxy {
        Ok(p) => p.clone(),
        Err(e) => return error!(EINVAL, e),
    };

    // Must configure before sampling start().
    let config_request = session_proxy.configure(configure).await;
    match config_request {
        Ok(_) => Ok((session_proxy, fidl::AsyncSocket::from_socket(client))),
        Err(e) => return error!(EINVAL, e),
    }
}

// Collects samples and puts backtrace in VMO.
// - Reads in the buffer from the socket for that duration in chunks.
// - Parses the buffer backtraces into PERF_RECORD_SAMPLE format.
// - Writes the PERF_RECORD_SAMPLE into VMO.
async fn stop_and_collect_samples(
    session_proxy: profiler::SessionProxy,
    mut client: fidl::AsyncSocket,
    seq_lock: &OnceLock<Result<SeqLock<PerfMetadataHeader, PerfMetadataValue>, Errno>>,
    perf_data_vmo: &zx::Vmo,
    sample_type: u64,
    sample_id: u64,
    sample_period: u64,
    read_format: u64,
    sample_regs_user: u64,
    sample_stack_user: u64,
    koid_session: Option<&PidKoidSession>,
    vmo_write_offset: &mut u64,
) -> Result<(), Errno> {
    let seq_lock_wrapper = match seq_lock.get() {
        Some(Ok(l)) => Some(l),
        // Initialization failed in a previous mmap() call. Propagate the error.
        Some(Err(e)) => return Err(e.clone()),
        // Not initialized yet (i.e. mmap() hasn't been called). However, we need to drain the
        // socket anyway if there is data as to unblock the profiler writing to the socket.
        None => None,
    };

    let process_socket = async {
        let mut header = [0; 8];
        let mut bytes_read = 0;
        while bytes_read < 8 {
            match client.read(&mut header[bytes_read..]).await {
                Ok(0) => break,
                Ok(n) => bytes_read += n,
                Err(e) => {
                    log_warn!("[perf_event_open] Error reading from socket: {:?}", e);
                    break;
                }
            }
        }

        if bytes_read != 8 || header != FXT_MAGIC_BYTES {
            if bytes_read > 0 {
                log_warn!(
                    "[perf_event_open] Received invalid or non-FXT sample data (bytes_read={})",
                    bytes_read
                );
            }
            return;
        }

        let header_cursor = Cursor::new(header);
        let reader = header_cursor.chain(client);
        let (mut stream, _task) = SessionParser::new_async(reader);
        let mut lost_events: u64 = 0;
        while let Some(record_result) = stream.next().await {
            match record_result {
                Ok(TraceRecord::Profiler(ProfilerRecord::Backtrace(backtrace))) => {
                    if let Some(seq_lock_wrapper) = seq_lock_wrapper {
                        let ips: Vec<u64> = backtrace.data;
                        // Resolve the sampled koids to Linux pid/tid against the live
                        // shared map (one read lock per record; collection is off the hot
                        // path and a live read sees every thread that recorded itself
                        // before it was sampled). If the sample cannot be resolved (e.g.
                        // native Fuchsia thread or without a session), drop the sample.
                        let Some(LinuxIdentity::Thread { pid, tid }) = koid_session.and_then(|s| {
                            s.resolve_koids(
                                zx::Koid::from_raw(backtrace.process.0),
                                zx::Koid::from_raw(backtrace.thread.0),
                            )
                        }) else {
                            continue;
                        };
                        let time = zx::BootInstant::from_nanos(backtrace.timestamp.max(0));
                        let perf_record_sample = PerfRecordSample {
                            pid: Some(pid as u32),
                            tid: Some(tid as u32),
                            time,
                            ips,
                            regs: &[],
                            stack: &[],
                            regs_abi: perf_sample_regs_abi_PERF_SAMPLE_REGS_ABI_NONE as u64,
                            sample_regs_user,
                            sample_stack_user,
                        };
                        let metadata = seq_lock_wrapper.get();
                        let bytes_written = write_record_to_vmo(
                            perf_record_sample,
                            perf_data_vmo,
                            sample_type,
                            sample_id,
                            sample_period,
                            read_format,
                            *vmo_write_offset,
                            &metadata,
                            &mut lost_events,
                        );
                        // Publish data_head after writing; set_value's
                        // release-ordered stores make the record contents
                        // visible to a reader that observes the new head.
                        if bytes_written > 0 {
                            *vmo_write_offset += bytes_written;
                            let mut metadata = seq_lock_wrapper.get();
                            metadata.data_head = *vmo_write_offset;
                            seq_lock_wrapper.set_value(metadata);
                        }
                    }
                }
                Ok(TraceRecord::LargeBlob(large_blob)) => {
                    if let Some(seq_lock_wrapper) = seq_lock_wrapper {
                        // The DWARF strategy delivers each sample as a
                        // "stack_sample" blob: [u64 regs_size][regs bytes]
                        // followed by memory chunks of [u64 base][u64 size]
                        // [bytes] (see the profiler's StackSampler).
                        if large_blob.name != "stack_sample" {
                            continue;
                        }
                        let Some(blob_metadata) = large_blob.metadata else {
                            continue;
                        };
                        let bytes = &large_blob.bytes;
                        if bytes.len() < 8 {
                            continue;
                        }
                        let regs_size =
                            u64::from_ne_bytes(bytes[0..8].try_into().unwrap()) as usize;
                        let mut offset = 8;
                        if regs_size == 0 || bytes.len() < offset + regs_size {
                            continue;
                        }

                        // 33 u64 general registers (r0-r29, lr, sp, pc), with
                        // cpsr following them in the zircon thread state.
                        // Register state of other widths (e.g. an x86_64
                        // thread state) is not supported and skipped by this
                        // size check.
                        const REGS_BYTES: usize = 33 * 8;
                        if regs_size < REGS_BYTES + 8 {
                            continue;
                        }
                        #[cfg(target_arch = "aarch64")]
                        let is_32bit = {
                            let cpsr = u64::from_ne_bytes(
                                bytes[offset + REGS_BYTES..offset + REGS_BYTES + 8]
                                    .try_into()
                                    .unwrap(),
                            );
                            (cpsr & zx::sys::ZX_REG_CPSR_ARCH_32_MASK)
                                == zx::sys::ZX_REG_CPSR_ARCH_32_MASK
                        };
                        #[cfg(not(target_arch = "aarch64"))]
                        let is_32bit = false;

                        let mut regs = bytes[offset..offset + REGS_BYTES].to_vec();
                        offset += regs_size;

                        if is_32bit {
                            // Zircon reports the AArch32 PC in the pc slot
                            // (index 32). 64-bit readers take it from there,
                            // per the Linux compat layout (AArch32 R0-R14
                            // arrive in x0-x14); mirror it into the arm32 R15
                            // slot (index 15) too for 32-bit readers, which
                            // only consume indices 0-15 -- x15 carries no
                            // meaningful value for AArch32 state.
                            let pc_offset = AARCH64_REG_PC * 8;
                            let pc_bytes = regs[pc_offset..pc_offset + 8].to_vec();
                            let r15_offset = AARCH32_REG_R15 * 8;
                            regs[r15_offset..r15_offset + 8].copy_from_slice(&pc_bytes);
                        }

                        let sp = if is_32bit {
                            // The arm32 stack pointer is R13.
                            let r13_offset = AARCH32_REG_R13 * 8;
                            u64::from_ne_bytes(regs[r13_offset..r13_offset + 8].try_into().unwrap())
                        } else {
                            let sp_offset = AARCH64_REG_SP * 8;
                            u64::from_ne_bytes(regs[sp_offset..sp_offset + 8].try_into().unwrap())
                        };
                        let pc_offset = AARCH64_REG_PC * 8;
                        let pc =
                            u64::from_ne_bytes(regs[pc_offset..pc_offset + 8].try_into().unwrap());

                        // Select the memory chunk that contains the sampled
                        // stack pointer and trim it to start exactly there:
                        // readers overlay the STACK_USER bytes at the SP
                        // reported in REGS_USER. The blob can carry several
                        // captures (the thread-state stack, the
                        // restricted-state struct, and a restricted-SP
                        // stack); selecting by SP keeps the registers and the
                        // stack bytes coherent.
                        let mut stack: &[u8] = &[];
                        while offset + 16 <= bytes.len() {
                            let chunk_base =
                                u64::from_ne_bytes(bytes[offset..offset + 8].try_into().unwrap());
                            offset += 8;
                            let chunk_size =
                                u64::from_ne_bytes(bytes[offset..offset + 8].try_into().unwrap());
                            offset += 8;
                            if offset + chunk_size as usize > bytes.len() {
                                break;
                            }
                            let data = &bytes[offset..offset + chunk_size as usize];
                            offset += chunk_size as usize;
                            if stack.is_empty() && chunk_base <= sp && sp < chunk_base + chunk_size
                            {
                                stack = &data[(sp - chunk_base) as usize..];
                            }
                        }
                        if stack.is_empty() {
                            // No capture covers the sampled SP; the sample
                            // cannot be unwound.
                            continue;
                        }

                        let Some(LinuxIdentity::Thread { pid, tid }) = koid_session.and_then(|s| {
                            s.resolve_koids(
                                zx::Koid::from_raw(blob_metadata.process.0),
                                zx::Koid::from_raw(blob_metadata.thread.0),
                            )
                        }) else {
                            continue;
                        };
                        let time = zx::BootInstant::from_nanos(blob_metadata.timestamp.max(0));
                        let regs_abi = if is_32bit {
                            perf_sample_regs_abi_PERF_SAMPLE_REGS_ABI_32 as u64
                        } else {
                            perf_sample_regs_abi_PERF_SAMPLE_REGS_ABI_64 as u64
                        };
                        let perf_record_sample = PerfRecordSample {
                            pid: Some(pid as u32),
                            tid: Some(tid as u32),
                            time,
                            ips: vec![pc],
                            regs: &regs,
                            stack,
                            regs_abi,
                            sample_regs_user,
                            sample_stack_user,
                        };
                        let metadata = seq_lock_wrapper.get();
                        let bytes_written = write_record_to_vmo(
                            perf_record_sample,
                            perf_data_vmo,
                            sample_type,
                            sample_id,
                            sample_period,
                            read_format,
                            *vmo_write_offset,
                            &metadata,
                            &mut lost_events,
                        );
                        // Publish data_head after writing; set_value's
                        // release-ordered stores make the record contents
                        // visible to a reader that observes the new head.
                        if bytes_written > 0 {
                            *vmo_write_offset += bytes_written;
                            let mut metadata = seq_lock_wrapper.get();
                            metadata.data_head = *vmo_write_offset;
                            seq_lock_wrapper.set_value(metadata);
                        }
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    log_warn!("[perf_event_open] Error parsing FXT: {:?}", e);
                    break;
                }
            }
        }
    };

    let (stats, ()) = futures::join!(session_proxy.stop(), process_socket);

    let samples_collected = match stats {
        Ok(stats) => stats.samples_collected.unwrap_or(0),
        Err(e) => return error!(EINVAL, e),
    };

    track_stub!(
        TODO("https://fxbug.dev/422502681"),
        "[perf_event_open] symbolize sample output and delete the below log_info"
    );
    log_info!("profiler samples_collected: {:?}", samples_collected);

    let reset_status = session_proxy.reset().await;
    return match reset_status {
        Ok(_) => Ok(()),
        Err(e) => error!(EINVAL, e),
    };
}

// Notifies other thread that we should start/stop sampling.
// Once sampling is complete, that profiler session is no longer needed.
// At that point, send back notification so that this is no longer blocking
// (e.g. so that other profiler sessions can start).
fn ping_receiver(
    mut ioctl_sender: future_mpsc::Sender<(IoctlOp, sync_mpsc::Sender<()>)>,
    command: IoctlOp,
) {
    log_info!("[perf_event_open] Received sampling command: {:?}", command);
    let (profiling_complete_sender, profiling_complete_receiver) = sync_mpsc::channel::<()>();
    match ioctl_sender.try_send((command, profiling_complete_sender)) {
        Ok(_) => (),
        Err(e) => {
            if e.is_full() {
                log_warn!("[perf_event_open] Failed to send {:?}: Channel full", command);
            } else if e.is_disconnected() {
                log_warn!("[perf_event_open] Failed to send {:?}: Receiver disconnected", command);
            } else {
                log_warn!("[perf_event_open] Failed to send {:?} due to {:?}", command, e.source());
            }
        }
    };
    // Block on / wait until profiling is complete before returning.
    // This notifies that the profiler is free to be used for another session.
    let _ = profiling_complete_receiver.recv().unwrap();
}

// Creates a seq lock for the given VMO. Initializes the seq lock with
// known initial values (unknown values default to 0).
// Does NOT actually save this as a memory object until mmap() is called.
//
// # Safety
//
// The caller must ensure that the kernel maintains exclusive write access to this VMO and
// there are only atomic accesses to this memory (see seq_lock lib.rs for details).
unsafe fn create_seq_lock(
    vmo_handle_ref: &zx::NullableHandle,
    buffer_size: u64,
) -> SeqLock<PerfMetadataHeader, PerfMetadataValue> {
    // Currently we hardcode everything just to get something E2E working.
    let metadata_header = PerfMetadataHeader { version: 1, compat_version: 2 };
    let page_size = *PAGE_SIZE;
    let metadata_value = PerfMetadataValue {
        lock: 0,
        index: 3,
        offset: 19337,
        time_enabled: 0,
        time_running: 0,
        __bindgen_anon_1: perf_event_mmap_page__bindgen_ty_1 { capabilities: 30 },
        pmc_width: 0,
        time_shift: 0,
        time_mult: 0,
        time_offset: 0,
        time_zero: 0,
        size: 0,
        __reserved_1: 0,
        time_cycles: 0,
        time_mask: 0,
        __reserved: [0; 928usize],
        // This first page (metadata) has finished writing. Start data_head at 0.
        data_head: 0,
        // Start reading from 0; it is the user's responsibility to increment on their end.
        data_tail: 0,
        // We know the data will start after 1 page size so we can set this now.
        data_offset: page_size,
        data_size: buffer_size - page_size,
        aux_head: 0,
        aux_tail: 0,
        aux_offset: 0,
        aux_size: 0,
    };
    let vmo = zx::Vmo::from(vmo_handle_ref.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap());

    // Create a SeqLock and safely initialize the `header` and `value` for it.
    // SeqLock is formatted thusly:
    //   header_struct : any size, params `version` and `compat_version` should not change
    //   sequence_counter : u32, this is the lock and should increment
    //   value_struct : any size, each param can change
    //
    // SAFETY: See safety requirements on `create_seq_lock`.
    unsafe {
        SeqLock::new_from_vmo(metadata_header, metadata_value, vmo)
            .expect("failed to create seq_lock for perf metadata")
    }
}

pub fn sys_perf_event_open(
    current_task: &CurrentTask,
    attr: UserRef<perf_event_attr>,
    // Note that this is pid in Linux docs.
    tid: tid_t,
    cpu: i32,
    group_fd: FdNumber,
    _flags: u64,
) -> Result<SyscallResult, Errno> {
    // So far, the implementation only sets the read_data_format according to the "Reading results"
    // section of https://man7.org/linux/man-pages/man2/perf_event_open.2.html for a single event.
    // Other features will be added in the future (see below track_stubs).
    let perf_event_attrs: perf_event_attr = current_task.read_object(attr)?;

    if (perf_event_attrs.sample_type & perf_event_sample_format_PERF_SAMPLE_STACK_USER as u64) != 0
    {
        if perf_event_attrs.sample_stack_user % 8 != 0 {
            return error!(EINVAL);
        }
    }

    if tid == -1 && cpu == -1 {
        return error!(EINVAL);
    }

    let target_task_type = match tid {
        -1 => TargetTaskType::AllTasks,
        0 => TargetTaskType::CurrentTask,
        _ => {
            track_stub!(TODO("https://fxbug.dev/409621963"), "[perf_event_open] implement tid > 0");
            return error!(ENOSYS);
        }
    };
    security::check_perf_event_open_access(
        current_task,
        target_task_type,
        &perf_event_attrs,
        perf_event_attrs.type_.try_into()?,
    )?;

    // Channel used to send info between notifier and spawned task thread.
    // We somewhat arbitrarily picked 8 for now in case we get a bunch of ioctls that are in
    // quick succession (instead of something lower).
    let (sender, mut receiver) = future_mpsc::channel(8);

    let mut perf_event_file = PerfEventFileState::new(
        perf_event_attrs,
        0,
        perf_event_attrs.disabled(),
        perf_event_attrs.sample_type,
        zx::Vmo::create(ESTIMATED_MMAP_BUFFER_SIZE).unwrap(),
        sender,
    );

    let read_format = perf_event_attrs.read_format;

    if (read_format & perf_event_read_format_PERF_FORMAT_TOTAL_TIME_ENABLED as u64) != 0
        || (read_format & perf_event_read_format_PERF_FORMAT_TOTAL_TIME_RUNNING as u64) != 0
    {
        // Only keep track of most_recent_enabled_time if we are currently in ENABLED state,
        // as otherwise this param shouldn't be used for calculating anything.
        if perf_event_file.disabled == 0 {
            perf_event_file.most_recent_enabled_time =
                zx::MonotonicInstant::get().into_nanos() as u64;
        }
        // Initialize this to 0 as we will need to return a time duration later during read().
        perf_event_file.total_time_running = 0;
    }

    let event_id = READ_FORMAT_ID_GENERATOR.fetch_add(1, Ordering::Relaxed);
    perf_event_file.rf_id = event_id;

    if group_fd.raw() == -1 {
        perf_event_file.sample_id = event_id;
    } else {
        let group_file = current_task.files().get(group_fd)?;
        let group_file_object_id = group_file.id;
        let perf_state = get_perf_state(&current_task.kernel);
        let events = perf_state.format_id_lookup_table.lock();
        if let Some(rf_id) = events.get(&group_file_object_id) {
            perf_event_file.sample_id = *rf_id;
        } else {
            return error!(EINVAL);
        }
    }

    if (read_format & perf_event_read_format_PERF_FORMAT_GROUP as u64) != 0 {
        track_stub!(
            TODO("https://fxbug.dev/402238049"),
            "[perf_event_open] implement read_format group"
        );
        return error!(ENOSYS);
    }
    if (read_format & perf_event_read_format_PERF_FORMAT_LOST as u64) != 0 {
        track_stub!(
            TODO("https://fxbug.dev/402260383"),
            "[perf_event_open] implement read_format lost"
        );
    }

    // Set up notifier for handling ioctl calls to enable/disable sampling.
    let mut vmo_handle_copy =
        perf_event_file.perf_data_vmo.as_handle_ref().duplicate_handle(zx::Rights::SAME_RIGHTS);

    // SAFETY: sample_period is a u64 field in a union with u64 sample_freq.
    // This is always sound regardless of the union's tag.
    let sample_period_in_ticks = unsafe { perf_event_file.attr.__bindgen_anon_1.sample_period };
    // The sample period from the PERF_COUNT_SW_CPU_CLOCK is
    // 1 nanosecond per tick. Convert this duration into zx::duration.
    let zx_sample_period = zx::MonotonicDuration::from_nanos(sample_period_in_ticks as i64);

    // SeqLock does not get instantiated with metadata values until mmap() is called.
    let seq_lock =
        Arc::new(OnceLock::<Result<SeqLock<PerfMetadataHeader, PerfMetadataValue>, Errno>>::new());
    let cloned_seq_lock = Arc::clone(&seq_lock);
    let mut vmo_write_offset = 0;

    let closure = async move |kthread_task: &CurrentTask| {
        let mut profiler_state: Option<(profiler::SessionProxy, fidl::AsyncSocket)> = None;
        // Held while sampling is enabled so pid/koid mappings are recorded for the
        // profiling session. Dropping it (including when this kthread exits with sampling
        // still enabled) releases this file's interest in the shared manager.
        let mut pid_koid_session: Option<PidKoidSession> = None;

        // This loop will wait for messages from the sender.
        while let Some((command, profiling_complete_receiver)) = receiver.next().await {
            match command {
                IoctlOp::Enable => {
                    match set_up_profiler(zx_sample_period).await {
                        Ok((session_proxy, client)) => {
                            // Record pid/koid mappings before the profiler starts sampling
                            // so every sampled thread can be resolved. If starting the
                            // profiler fails, dropping the unstored session ends the
                            // recording interest automatically.
                            let session = kthread_task.kernel().trace_event_manager.open();
                            let start_request = profiler::SessionStartRequest {
                                buffer_results: Some(true),
                                buffer_size_mb: Some(8 as u64),
                                ..Default::default()
                            };
                            if let Err(e) = session_proxy.start(&start_request).await {
                                log_warn!("Failed to start profiling: {}", e);
                            } else {
                                profiler_state = Some((session_proxy, client));
                                pid_koid_session = Some(session);
                            }
                        }
                        Err(e) => {
                            log_warn!("Failed to profile: {}", e);
                        }
                    };
                    // Send notification anyway to unblock the ioctl caller.
                    let _ = profiling_complete_receiver.send(());
                }
                IoctlOp::Disable => {
                    if let Some((session_proxy, client)) = profiler_state.take() {
                        let handle = vmo_handle_copy
                            .as_mut()
                            .expect("Failed to get VMO handle")
                            .as_handle_ref()
                            .duplicate_handle(zx::Rights::SAME_RIGHTS)
                            .unwrap();

                        if let Err(e) = stop_and_collect_samples(
                            session_proxy,
                            client,
                            &cloned_seq_lock,
                            &zx::Vmo::from(handle),
                            perf_event_file.sample_type,
                            perf_event_file.sample_id,
                            sample_period_in_ticks,
                            perf_event_file.attr.read_format,
                            perf_event_file.attr.sample_regs_user,
                            perf_event_file.attr.sample_stack_user as u64,
                            pid_koid_session.as_ref(),
                            &mut vmo_write_offset,
                        )
                        .await
                        {
                            log_warn!("Failed to collect sample: {:?}", e);
                        }
                    }
                    // Sampling is disabled: drop this file's recording session.
                    pid_koid_session = None;
                    // Send notification anyway to unblock the ioctl caller.
                    let _ = profiling_complete_receiver.send(());
                }
            }
        }
        // If the command channel closed with sampling still enabled (e.g. the perf event
        // file was closed), dropping pid_koid_session here ends the recording interest.
    };
    let req = SpawnRequestBuilder::new()
        .with_debug_name("perf-event-sampler")
        .with_async_closure(closure)
        .build();
    current_task.kernel().kthreads.spawner().spawn_from_request(req);

    let file = Box::new(PerfEventFile {
        _tid: tid,
        _cpu: cpu,
        perf_event_file: perf_event_file.into(),
        security_state: security::perf_event_alloc(current_task),
        seq_lock: seq_lock,
    });
    // TODO: https://fxbug.dev/404739824 - Confirm whether to handle this as a "private" node.
    let file_handle = Anon::new_private_file(current_task, file, OpenFlags::RDWR, "[perf_event]");
    let file_object_id = file_handle.id;
    let file_descriptor: Result<FdNumber, Errno> =
        current_task.add_file(file_handle, FdFlags::empty());

    match file_descriptor {
        Ok(fd) => {
            if group_fd.raw() == -1 {
                let perf_state = get_perf_state(&current_task.kernel);
                let mut events = perf_state.format_id_lookup_table.lock();
                events.insert(file_object_id, event_id);
            }
            Ok(fd.into())
        }
        Err(_) => {
            track_stub!(
                TODO("https://fxbug.dev/402453955"),
                "[perf_event_open] implement remaining error handling"
            );
            error!(EMFILE)
        }
    }
}
// Syscalls for arch32 usage
#[cfg(target_arch = "aarch64")]
mod arch32 {
    pub use super::sys_perf_event_open as sys_arch32_perf_event_open;
}

#[cfg(target_arch = "aarch64")]
pub use arch32::*;

use crate::mm::memory::MemoryObject;
use crate::mm::{MemoryAccessorExt, ProtectionFlags};
use crate::task::CurrentTask;
use crate::vfs::{
    Anon, FdFlags, FdNumber, FileObject, FileObjectId, FileObjectState, FileOps, InputBuffer,
    OutputBuffer,
};
use crate::{fileops_impl_nonseekable, fileops_impl_noop_sync};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::tracing::{TracePerformanceEventManager, ZirconIdentity};
    use fidl::endpoints::create_proxy;
    use fuchsia_async as fasync;

    #[::fuchsia::test]
    async fn test_stop_and_collect_samples_socket_full() {
        let (session_proxy, session_stream) = create_proxy::<profiler::SessionMarker>();
        let (client_socket, server_socket) = zx::Socket::create_stream();

        // Fill server_socket until it is no longer writable.
        // Start with FXT_MAGIC_BYTES so process_socket recognizes FXT format.
        let _ = server_socket.write(&FXT_MAGIC_BYTES);
        let buf = [0u8; 1024];
        while match server_socket.write(&buf) {
            Ok(_) => true,
            Err(zx::Status::SHOULD_WAIT) => false,
            Err(e) => panic!("unexpected error filling socket: {:?}", e),
        } {}

        let mock_service = async move {
            let mut session_stream = session_stream.into_stream();
            let mut server_socket = Some(server_socket);
            // The profiler is currently single threaded and blocks if the socket is full. Model
            // this here to ensure we don't deadlock if the socket fills up.
            while let Some(Ok(request)) = session_stream.next().await {
                match request {
                    profiler::SessionRequest::Stop { responder } => {
                        if let Some(socket) = server_socket.take() {
                            let _ =
                                fasync::OnSignals::new(&socket, zx::Signals::SOCKET_WRITABLE).await;
                            drop(socket);
                        }
                        let _ = responder.send(&profiler::SessionResult::default());
                    }
                    profiler::SessionRequest::Reset { responder } => {
                        let _ = responder.send();
                    }
                    _ => {}
                }
            }
        };

        let client = fidl::AsyncSocket::from_socket(client_socket);
        let seq_lock = OnceLock::new();
        let perf_data_vmo = zx::Vmo::create(ESTIMATED_MMAP_BUFFER_SIZE).unwrap();
        let mut vmo_write_offset = 0;

        let test_task = stop_and_collect_samples(
            session_proxy,
            client,
            &seq_lock,
            &perf_data_vmo,
            0,
            0,
            0,
            0,
            0,
            0,
            None,
            &mut vmo_write_offset,
        );

        let ((), result) = futures::join!(mock_service, test_task);
        assert!(result.is_ok());
    }

    fn write_fxt_backtrace_record(
        buf: &mut Vec<u8>,
        ticks: u64,
        process_koid: u64,
        thread_koid: u64,
        ips: &[u64],
    ) {
        let record_type: u64 = 10;
        let sub_type: u64 = 2; // Backtrace
        let thread_ref: u64 = 0; // Inline process and thread koid
        let num_records: u64 = ips.len() as u64;
        let flags: u64 = 0;
        let size_words: u64 = 4 + num_records;

        let header_val: u64 = (flags << 36)
            | (num_records << 28)
            | (thread_ref << 20)
            | (sub_type << 16)
            | (size_words << 4)
            | record_type;

        buf.extend_from_slice(&header_val.to_le_bytes());
        buf.extend_from_slice(&ticks.to_le_bytes());
        buf.extend_from_slice(&process_koid.to_le_bytes());
        buf.extend_from_slice(&thread_koid.to_le_bytes());
        for ip in ips {
            buf.extend_from_slice(&ip.to_le_bytes());
        }
    }

    #[::fuchsia::test]
    async fn test_stop_and_collect_samples_resolves_pid_tid() {
        let (session_proxy, session_stream) = create_proxy::<profiler::SessionMarker>();
        let (client_socket, server_socket) = zx::Socket::create_stream();

        let mut socket_data = Vec::new();
        socket_data.extend_from_slice(&FXT_MAGIC_BYTES);
        // Mapped sample: process 1001, thread 1002 -> should resolve to pid 42, tid 43.
        write_fxt_backtrace_record(&mut socket_data, 1000, 1001, 1002, &[0x12345678]);
        // Unmapped sample: process 9999, thread 9998 -> should be dropped.
        write_fxt_backtrace_record(&mut socket_data, 2000, 9999, 9998, &[0x87654321]);
        server_socket.write(&socket_data).expect("failed to write FXT data to socket");

        let mock_service = async move {
            let mut session_stream = session_stream.into_stream();
            let mut server_socket = Some(server_socket);
            while let Some(Ok(request)) = session_stream.next().await {
                match request {
                    profiler::SessionRequest::Stop { responder } => {
                        // Drop the server socket to signal EOF to the reader.
                        drop(server_socket.take());
                        let _ = responder.send(&profiler::SessionResult::default());
                    }
                    profiler::SessionRequest::Reset { responder } => {
                        let _ = responder.send();
                    }
                    _ => {}
                }
            }
        };

        // Set up the pid/koid manager and register the mapping.
        let manager = Arc::new(TracePerformanceEventManager::new(std::sync::Weak::new()));
        let session = manager.open();
        manager.record(
            42,
            43,
            ZirconIdentity { process: zx::Koid::from_raw(1001), thread: zx::Koid::from_raw(1002) },
        );

        let perf_data_vmo = zx::Vmo::create(ESTIMATED_MMAP_BUFFER_SIZE).unwrap();
        let vmo_handle_copy =
            perf_data_vmo.as_handle_ref().duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
        let seq_lock = OnceLock::new();
        // SAFETY: The test maintains exclusive write access to this VMO.
        let _ = seq_lock
            .set(Ok(unsafe { create_seq_lock(&vmo_handle_copy, ESTIMATED_MMAP_BUFFER_SIZE) }));

        let sample_type = (perf_event_sample_format_PERF_SAMPLE_IP
            | perf_event_sample_format_PERF_SAMPLE_TID) as u64;
        let mut vmo_write_offset = 0;
        let client = fidl::AsyncSocket::from_socket(client_socket);

        let test_task = stop_and_collect_samples(
            session_proxy,
            client,
            &seq_lock,
            &perf_data_vmo,
            sample_type,
            0,
            0,
            0,
            0,
            0,
            Some(&session),
            &mut vmo_write_offset,
        );

        let ((), result) = futures::join!(mock_service, test_task);
        assert!(result.is_ok());

        // Header (8 bytes) + IP (8 bytes) + PID/TID (8 bytes) = 24 bytes.
        let expected_record_size: u64 = 24;
        assert_eq!(vmo_write_offset, expected_record_size);

        let metadata = seq_lock.get().unwrap().as_ref().unwrap().get();
        assert_eq!(metadata.data_head, expected_record_size);

        let mut record_bytes = [0u8; 24];
        perf_data_vmo.read(&mut record_bytes, metadata.data_offset).unwrap();

        let record_type = u32::from_ne_bytes(record_bytes[0..4].try_into().unwrap());
        assert_eq!(record_type, perf_event_type_PERF_RECORD_SAMPLE);

        let misc = u16::from_ne_bytes(record_bytes[4..6].try_into().unwrap());
        assert_eq!(misc, PERF_RECORD_MISC_USER as u16);

        let size = u16::from_ne_bytes(record_bytes[6..8].try_into().unwrap());
        assert_eq!(size, expected_record_size as u16);

        let ip = u64::from_ne_bytes(record_bytes[8..16].try_into().unwrap());
        assert_eq!(ip, 0x12345678);

        let pid = u32::from_ne_bytes(record_bytes[16..20].try_into().unwrap());
        assert_eq!(pid, 42);

        let tid = u32::from_ne_bytes(record_bytes[20..24].try_into().unwrap());
        assert_eq!(tid, 43);

        // Verify that unmapped sample was dropped and no extra bytes were written.
        let mut trailing_bytes = [0u8; 24];
        perf_data_vmo
            .read(&mut trailing_bytes, metadata.data_offset + expected_record_size)
            .unwrap();
        assert_eq!(trailing_bytes, [0u8; 24]);
    }

    #[::fuchsia::test]
    async fn test_write_record_to_vmo_regs_and_stack() {
        let perf_data_vmo = zx::Vmo::create(ESTIMATED_MMAP_BUFFER_SIZE).unwrap();
        let vmo_handle_copy =
            perf_data_vmo.as_handle_ref().duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
        // SAFETY: The test maintains exclusive write access to this VMO.
        let seq_lock = unsafe { create_seq_lock(&vmo_handle_copy, ESTIMATED_MMAP_BUFFER_SIZE) };
        let metadata = seq_lock.get();
        let mut lost_events = 0;
        let regs_data = [0x11u8; 16];
        let stack_data = [0x22u8; 16];
        let sample = PerfRecordSample {
            pid: Some(10),
            tid: Some(20),
            time: zx::BootInstant::from_nanos(123_456_789),
            ips: vec![0xdeadbeef],
            regs: &regs_data,
            stack: &stack_data,
            regs_abi: perf_sample_regs_abi_PERF_SAMPLE_REGS_ABI_32 as u64,
            sample_regs_user: 3,
            sample_stack_user: 16,
        };
        let sample_type = (perf_event_sample_format_PERF_SAMPLE_IP
            | perf_event_sample_format_PERF_SAMPLE_TID
            | perf_event_sample_format_PERF_SAMPLE_TIME
            | perf_event_sample_format_PERF_SAMPLE_REGS_USER
            | perf_event_sample_format_PERF_SAMPLE_STACK_USER) as u64;
        let written = write_record_to_vmo(
            sample,
            &perf_data_vmo,
            sample_type,
            0,
            0,
            0,
            0,
            &metadata,
            &mut lost_events,
        );
        assert!(written > 0);

        let mut record_bytes = vec![0u8; written as usize];
        perf_data_vmo.read(&mut record_bytes, metadata.data_offset).unwrap();

        let record_type = u32::from_ne_bytes(record_bytes[0..4].try_into().unwrap());
        assert_eq!(record_type, perf_event_type_PERF_RECORD_SAMPLE);
        let misc = u16::from_ne_bytes(record_bytes[4..6].try_into().unwrap());
        assert_eq!(misc, PERF_RECORD_MISC_USER as u16);
        let size = u16::from_ne_bytes(record_bytes[6..8].try_into().unwrap());
        assert_eq!(size as usize, written as usize);

        let ip = u64::from_ne_bytes(record_bytes[8..16].try_into().unwrap());
        assert_eq!(ip, 0xdeadbeef);

        let pid = u32::from_ne_bytes(record_bytes[16..20].try_into().unwrap());
        assert_eq!(pid, 10);
        let tid = u32::from_ne_bytes(record_bytes[20..24].try_into().unwrap());
        assert_eq!(tid, 20);

        let time = u64::from_ne_bytes(record_bytes[24..32].try_into().unwrap());
        assert_eq!(time, 123_456_789);

        // REGS_USER: abi (8 bytes) + regs (16 bytes)
        let abi = u64::from_ne_bytes(record_bytes[32..40].try_into().unwrap());
        assert_eq!(abi, perf_sample_regs_abi_PERF_SAMPLE_REGS_ABI_32 as u64);
        assert_eq!(&record_bytes[40..56], &regs_data);

        // STACK_USER: dest_len (8 bytes) + data (16 bytes) + dyn_size (8 bytes)
        let stack_len = u64::from_ne_bytes(record_bytes[56..64].try_into().unwrap());
        assert_eq!(stack_len, 16);
        assert_eq!(&record_bytes[64..80], &stack_data);
        let dyn_size = u64::from_ne_bytes(record_bytes[80..88].try_into().unwrap());
        assert_eq!(dyn_size, 16);
    }

    #[::fuchsia::test]
    async fn test_write_record_to_vmo_regs_abi_64() {
        let perf_data_vmo = zx::Vmo::create(ESTIMATED_MMAP_BUFFER_SIZE).unwrap();
        let vmo_handle_copy =
            perf_data_vmo.as_handle_ref().duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
        // SAFETY: The test maintains exclusive write access to this VMO.
        let seq_lock = unsafe { create_seq_lock(&vmo_handle_copy, ESTIMATED_MMAP_BUFFER_SIZE) };
        let metadata = seq_lock.get();
        let mut lost_events = 0;
        let regs_data = [0x44u8; 16];
        let stack_data = [0x55u8; 16];
        let sample = PerfRecordSample {
            pid: Some(10),
            tid: Some(20),
            time: zx::BootInstant::from_nanos(123_456_789),
            ips: vec![0xdeadbeef],
            regs: &regs_data,
            stack: &stack_data,
            regs_abi: perf_sample_regs_abi_PERF_SAMPLE_REGS_ABI_64 as u64,
            sample_regs_user: 3,
            sample_stack_user: 16,
        };
        let sample_type = (perf_event_sample_format_PERF_SAMPLE_IP
            | perf_event_sample_format_PERF_SAMPLE_TID
            | perf_event_sample_format_PERF_SAMPLE_TIME
            | perf_event_sample_format_PERF_SAMPLE_REGS_USER
            | perf_event_sample_format_PERF_SAMPLE_STACK_USER) as u64;
        let written = write_record_to_vmo(
            sample,
            &perf_data_vmo,
            sample_type,
            0,
            0,
            0,
            0,
            &metadata,
            &mut lost_events,
        );
        assert!(written > 0);

        let mut record_bytes = vec![0u8; written as usize];
        perf_data_vmo.read(&mut record_bytes, metadata.data_offset).unwrap();

        // REGS_USER: abi (8 bytes) + regs (16 bytes)
        let abi = u64::from_ne_bytes(record_bytes[32..40].try_into().unwrap());
        assert_eq!(abi, perf_sample_regs_abi_PERF_SAMPLE_REGS_ABI_64 as u64);
        assert_eq!(&record_bytes[40..56], &regs_data);
    }

    #[::fuchsia::test]
    async fn test_write_record_to_vmo_regs_abi_none_when_empty() {
        let perf_data_vmo = zx::Vmo::create(ESTIMATED_MMAP_BUFFER_SIZE).unwrap();
        let vmo_handle_copy =
            perf_data_vmo.as_handle_ref().duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
        // SAFETY: The test maintains exclusive write access to this VMO.
        let seq_lock = unsafe { create_seq_lock(&vmo_handle_copy, ESTIMATED_MMAP_BUFFER_SIZE) };
        let metadata = seq_lock.get();
        let mut lost_events = 0;
        let stack_data = [0x33u8; 8];
        let sample = PerfRecordSample {
            pid: Some(10),
            tid: Some(20),
            time: zx::BootInstant::from_nanos(999),
            ips: vec![0x1000],
            regs: &[],
            stack: &stack_data,
            regs_abi: perf_sample_regs_abi_PERF_SAMPLE_REGS_ABI_NONE as u64,
            sample_regs_user: 3,
            sample_stack_user: 8,
        };
        let sample_type = (perf_event_sample_format_PERF_SAMPLE_IP
            | perf_event_sample_format_PERF_SAMPLE_TID
            | perf_event_sample_format_PERF_SAMPLE_TIME
            | perf_event_sample_format_PERF_SAMPLE_REGS_USER
            | perf_event_sample_format_PERF_SAMPLE_STACK_USER) as u64;
        let written = write_record_to_vmo(
            sample,
            &perf_data_vmo,
            sample_type,
            0,
            0,
            0,
            0,
            &metadata,
            &mut lost_events,
        );
        assert!(written > 0);

        let mut record_bytes = vec![0u8; written as usize];
        perf_data_vmo.read(&mut record_bytes, metadata.data_offset).unwrap();

        // REGS_USER: abi (8 bytes) should be PERF_SAMPLE_REGS_ABI_NONE (0), and no regs follow.
        let abi = u64::from_ne_bytes(record_bytes[32..40].try_into().unwrap());
        assert_eq!(abi, perf_sample_regs_abi_PERF_SAMPLE_REGS_ABI_NONE as u64);

        // STACK_USER immediately follows the ABI tag at offset 40.
        let stack_len = u64::from_ne_bytes(record_bytes[40..48].try_into().unwrap());
        assert_eq!(stack_len, 8);
        assert_eq!(&record_bytes[48..56], &stack_data);
        let dyn_size = u64::from_ne_bytes(record_bytes[56..64].try_into().unwrap());
        assert_eq!(dyn_size, 8);
    }
}
