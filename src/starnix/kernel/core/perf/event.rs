// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::perf::lockless_ring_buffer::LocklessRingBuffer;
use crate::task::Kernel;
use crate::vfs::OutputBuffer;
use fuchsia_rcu::RcuDroppableArc;
use starnix_logging::log_error;
use starnix_uapi::errors::Errno;
use zerocopy::native_endian::{I32, U16, U32, U64};
use zerocopy::{Immutable, IntoBytes, Unaligned};
use zx::BootTimeline;

// The default ring buffer size (2MB).
// TODO(https://fxbug.dev/357665908): This should be based on /sys/kernel/tracing/buffer_size_kb.
const DEFAULT_RING_BUFFER_SIZE_BYTES: usize = 2097152;

// The event id for atrace events.
const FTRACE_PRINT_ID: U16 = U16::new(5);

// Used for inspect tracking.
const DROPPED_PAGES: &str = "dropped_pages";

const MAX_TIME_DELTA_NANOS: u32 = (1 << 27) - 1;

#[repr(C)]
#[derive(Debug, Default, IntoBytes, Immutable, Unaligned)]
struct PrintEventHeader {
    common_type: U16,
    common_flags: u8,
    common_preempt_count: u8,
    common_pid: I32,
    ip: U64,
}

#[repr(C)]
#[derive(Debug, IntoBytes, Immutable, Unaligned)]
struct PrintEvent {
    header: PrintEventHeader,
}

impl PrintEvent {
    fn new(pid: i32) -> Self {
        Self {
            header: PrintEventHeader {
                common_type: FTRACE_PRINT_ID,
                common_pid: I32::new(pid),
                // Perfetto doesn't care about any other field.
                ..Default::default()
            },
        }
    }

    fn size(&self) -> usize {
        std::mem::size_of::<PrintEventHeader>()
    }
}

#[repr(C)]
#[derive(Debug, Default, IntoBytes, PartialEq, Immutable, Unaligned)]
struct TraceEventHeader {
    // u32 where:
    //   type_or_length: bottom 5 bits. If 0, `data` is read for length. Always set to 0 for now.
    //   time_delta: top 27 bits
    time_delta: U32,

    // If type_or_length is 0, holds the length of the trace message.
    // We always write length here for simplicity.
    data: U32,
}

impl TraceEventHeader {
    fn new(size: usize) -> Self {
        // The size reported in the event's header includes the size of `size` (a u32) and the size
        // of the event data.
        let size = (std::mem::size_of::<u32>() + size) as u32;
        Self { time_delta: U32::new(0), data: U32::new(size) }
    }

    fn set_time_delta(&mut self, nanos: u32) {
        // The max delta is capped at MAX_TIME_DELTA_NANOS so it fits into 27 bits.
        // another option is to just shift it over, but that could lead to other misleading
        // deltas if the value is large.
        let saturated_nanos = nanos.min(MAX_TIME_DELTA_NANOS);
        // Move into the high 27 bits reserving the lower 5 bits for the type_or_length value.
        self.time_delta = U32::new(saturated_nanos << 5);
    }
}

#[repr(C)]
#[derive(Debug, IntoBytes, Immutable, Unaligned)]
pub struct TraceEvent {
    /// Common metadata among all trace event types.
    header: TraceEventHeader, // u64

    /// The event data.
    ///
    /// Atrace events are reported as PrintFtraceEvents. When we support multiple types of events,
    /// this can be updated to be more generic.
    event: PrintEvent,
}

impl TraceEvent {
    pub fn new(pid: i32, data_len: usize) -> Self {
        let event = PrintEvent::new(pid);
        // +1 because we append a trailing '\n' to the data when we serialize.
        let header = TraceEventHeader::new(event.size() + data_len + 1);
        Self { header, event }
    }

    fn size(&self) -> usize {
        // The header's data size doesn't include the time_delta size.
        std::mem::size_of::<u32>() + self.header.data.get() as usize
    }
}

/// Stores all trace events.
pub struct TraceEventQueue {
    /// The trace events.
    ring_buffer: RcuDroppableArc<LocklessRingBuffer>,

    /// Async ID for read track grouping.
    pub async_id_read: fuchsia_trace::Id,

    /// Async ID for write track grouping.
    pub async_id_write: fuchsia_trace::Id,

    /// CPU ID for trace tracks.
    pub cpu_id: u32,
}

const STATIC_READ_TRACK_NAMES: [&str; 8] = [
    "Tracefs read 0",
    "Tracefs read 1",
    "Tracefs read 2",
    "Tracefs read 3",
    "Tracefs read 4",
    "Tracefs read 5",
    "Tracefs read 6",
    "Tracefs read 7",
];

const STATIC_WRITE_TRACK_NAMES: [&str; 8] = [
    "Tracefs write 0",
    "Tracefs write 1",
    "Tracefs write 2",
    "Tracefs write 3",
    "Tracefs write 4",
    "Tracefs write 5",
    "Tracefs write 6",
    "Tracefs write 7",
];

impl<'a> TraceEventQueue {
    pub(crate) fn new(cpu_id: u32) -> Result<Self, Errno> {
        let async_id_read = fuchsia_trace::Id::new();
        let async_id_write = fuchsia_trace::Id::new();
        let ring_buffer = Arc::new(
            LocklessRingBuffer::new(DEFAULT_RING_BUFFER_SIZE_BYTES, true, async_id_write)
                .map_err(|_| starnix_uapi::errno!(ENOMEM))?,
        );
        ring_buffer.disable()?;

        Ok(Self {
            ring_buffer: RcuDroppableArc::new(ring_buffer),
            async_id_read,
            async_id_write,
            cpu_id,
        })
    }

    pub fn read_track_name(&self) -> std::borrow::Cow<'static, str> {
        if let Some(&name) = STATIC_READ_TRACK_NAMES.get(self.cpu_id as usize) {
            std::borrow::Cow::Borrowed(name)
        } else {
            std::borrow::Cow::Owned(format!("Tracefs read {}", self.cpu_id))
        }
    }

    pub fn write_track_name(&self) -> std::borrow::Cow<'static, str> {
        if let Some(&name) = STATIC_WRITE_TRACK_NAMES.get(self.cpu_id as usize) {
            std::borrow::Cow::Borrowed(name)
        } else {
            std::borrow::Cow::Owned(format!("Tracefs write {}", self.cpu_id))
        }
    }

    fn enable(&self) -> Result<zx::BootInstant, Errno> {
        self.ring_buffer.read().enable()
    }

    /// Disables the event queue and resets it to empty.
    /// The number of dropped pages are recorded for reading via tracefs.
    fn disable(&self) -> Result<u64, Errno> {
        self.ring_buffer.read().disable()
    }

    /// Reads a page worth of events. Currently only reads pages that are full.
    ///
    /// From https://docs.kernel.org/trace/ring-buffer-design.html, when memory is mapped, a reader
    /// page can be swapped with the header page to avoid copying memory.
    pub fn read(&self, buf: &mut dyn OutputBuffer) -> Result<usize, Errno> {
        self.ring_buffer.read().read(buf)
    }

    /// Write `event` into `ring_buffer`.
    /// If `event` does not fit in the current page, move on to the next.
    ///
    /// Should eventually allow for a writer to preempt another writer.
    /// See https://docs.kernel.org/trace/ring-buffer-design.html.
    /// Returns the delta duration between this event and the previous event written.
    pub fn push_event(
        &self,
        mut event: TraceEvent,
        data: &[u8],
    ) -> Result<zx::Duration<BootTimeline>, Errno> {
        let size = event.size();
        let ring_buffer = self.ring_buffer.read();

        let (res, _timestamp, delta) = match ring_buffer.reserve(size) {
            Ok(res) => res,
            Err(e) if e == starnix_uapi::errno!(EINVAL) => {
                log_error!("Invalid reservation size: {}", size);
                return Err(starnix_uapi::errno!(EINVAL));
            }
            Err(e) if e == starnix_uapi::errno!(ENOSPC) => {
                log_error!("Ring buffer full, dropping event of size: {}", size);
                return Err(starnix_uapi::errno!(ENOSPC));
            }
            Err(e) => return Err(e),
        };

        let nanos = delta.into_nanos().try_into().unwrap_or(u32::MAX);
        event.header.set_time_delta(nanos);

        let bytes = event.as_bytes();
        res.write_at(0, bytes);
        res.write_at(bytes.len(), data);
        res.write_at(bytes.len() + data.len(), b"\n");

        ring_buffer.commit(res);

        Ok(delta)
    }
}

pub struct TraceEventQueueList {
    pub queues: Vec<Arc<TraceEventQueue>>,
    tracing_enabled: Arc<AtomicBool>,
    tracefs_node: fuchsia_inspect::Node,
}

impl TraceEventQueueList {
    pub fn from(kernel: &Kernel) -> Arc<Self> {
        kernel.expando.get_or_init(|| {
            let num_cpus = zx::system_get_num_cpus();
            let tracing_enabled = Arc::new(AtomicBool::new(false));
            let tracefs_node = kernel.inspect_node.create_child("tracefs");

            let mut queues = vec![];
            for cpu in 0..num_cpus {
                let queue = Arc::new(TraceEventQueue::new(cpu as u32).expect("create queue"));
                queues.push(queue);
            }
            Self { queues, tracing_enabled, tracefs_node }
        })
    }

    pub fn is_enabled(&self) -> bool {
        self.tracing_enabled.load(Ordering::Acquire)
    }

    pub fn enable(&self) -> Result<(), Errno> {
        let mut first_error = None;
        for queue in &self.queues {
            if let Err(e) = queue.enable() {
                if first_error.is_none() {
                    first_error = Some(e);
                }
            }
        }
        if let Some(e) = first_error {
            return Err(e);
        }
        self.tracing_enabled.store(true, Ordering::Release);
        Ok(())
    }

    pub fn disable(&self) -> Result<(), Errno> {
        // Set disabled to stop new access to the queue, then clean up each one.
        self.tracing_enabled.store(false, Ordering::Release);
        let mut first_error = None;
        let mut total_dropped = 0;
        for queue in &self.queues {
            match queue.disable() {
                Ok(dropped) => total_dropped += dropped,
                Err(e) => {
                    if first_error.is_none() {
                        first_error = Some(e);
                    }
                }
            }
        }

        self.tracefs_node.record_uint(DROPPED_PAGES, total_dropped);

        if let Some(e) = first_error {
            return Err(e);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_RING_BUFFER_SIZE_BYTES, LocklessRingBuffer, TraceEvent, TraceEventQueue};
    use crate::vfs::OutputBuffer;
    use crate::vfs::buffers::VecOutputBuffer;

    use starnix_types::PAGE_SIZE;
    use starnix_uapi::error;

    #[fuchsia::test]
    fn trace_event_queue_empty_errors() {
        let queue = TraceEventQueue::new(0).unwrap();

        let mut buffer = VecOutputBuffer::new(*PAGE_SIZE as usize);
        assert_eq!(queue.read(&mut buffer), error!(EAGAIN));

        let data = b"B|1234|slice_name";
        let event = TraceEvent::new(1234, data.len());
        assert_eq!(queue.push_event(event, data), error!(ENOMEM));
    }

    #[fuchsia::test]
    fn read_empty_queue() {
        let queue = TraceEventQueue::new(0).expect("create queue");
        let mut buffer = VecOutputBuffer::new(*PAGE_SIZE as usize);
        assert_eq!(queue.read(&mut buffer), error!(EAGAIN));
    }

    #[fuchsia::test]
    fn enable_disable_queue() {
        let queue = TraceEventQueue::new(0).expect("create queue");
        assert!(!queue.ring_buffer.read().is_enabled());

        // Enable tracing and check the queue's state.
        assert!(queue.enable().is_ok());
        assert_eq!(queue.ring_buffer.read().size_bytes(), DEFAULT_RING_BUFFER_SIZE_BYTES);

        // Confirm we can push an event.
        let data = b"B|1234|slice_name";
        let event = TraceEvent::new(1234, data.len());
        let result = queue.push_event(event, data);

        assert!(result.is_ok());
        assert_eq!(result.as_ref().unwrap().into_nanos(), 0);

        // Disable tracing and check that the queue's state has been reset.
        assert!(queue.disable().is_ok());
        assert!(!queue.ring_buffer.read().is_enabled());
    }

    #[fuchsia::test]
    fn create_trace_event() {
        // Create an event.
        let event = TraceEvent::new(1234, b"B|1234|slice_name".len());
        let event_size = event.size();
        assert_eq!(event_size, 42);
    }

    // This can be removed when we support reading incomplete pages.
    #[fuchsia::test]
    fn single_trace_event_fails_read() {
        let queue = TraceEventQueue::new(0).expect("create queue");
        queue.enable().expect("enable queue");
        // Create an event.
        let data = b"B|1234|slice_name";
        let event = TraceEvent::new(1234, data.len());

        // Push the event into the queue.
        let result = queue.push_event(event, data);
        assert!(result.is_ok());
        assert_eq!(result.ok().expect("delta").into_nanos(), 0);

        let mut buffer = VecOutputBuffer::new(*PAGE_SIZE as usize);
        assert_eq!(queue.read(&mut buffer), error!(EAGAIN));
    }

    #[fuchsia::test]
    fn page_overflow() {
        let queue = TraceEventQueue::new(0).expect("create queue");
        let queue_start_timestamp = queue.enable().expect("enable queue");

        let pid = 1234;
        let data = b"B|1234|loooooooooooooooooooooooooooooooooooooooooooooooooooooooooo\
        ooooooooooooooooooooooooooooooooooooooooooooooooooooooooongevent";
        let expected_event = TraceEvent::new(pid, data.len());
        assert_eq!(expected_event.size(), 155);

        let events_per_page =
            (*PAGE_SIZE as usize - LocklessRingBuffer::PAGE_HEADER_SIZE) / expected_event.size();

        // Push the event into the queue.
        for i in 0..=events_per_page {
            let event = TraceEvent::new(pid, data.len());
            let result = queue.push_event(event, data);
            assert!(result.is_ok());
            let delta = result.ok().expect("delta").into_nanos();
            // The first event on Page 0 (i == 0) and the first event on Page 1 (i == events_per_page,
            // due to overflow since a page holds exactly events_per_page events of size 155) must
            // have a time delta of exactly 0 in their event headers as per the Ftrace format.
            if i == 0 || i == events_per_page {
                assert_eq!(delta, 0);
            } else {
                assert!(delta > 0);
            }
        }

        // Read a page of data.
        let mut buffer = VecOutputBuffer::new(*PAGE_SIZE as usize);
        assert_eq!(queue.read(&mut buffer), Ok(*PAGE_SIZE as usize));
        assert_eq!(buffer.bytes_written() as u64, *PAGE_SIZE);

        // Verify timestamp is monotonic
        let actual_ts_bytes = &buffer.data()[0..8];
        let actual_ts = u64::from_le_bytes(actual_ts_bytes.try_into().unwrap());
        assert!(actual_ts >= queue_start_timestamp.into_nanos() as u64);

        // Verify size of events
        let actual_size_bytes = &buffer.data()[8..16];
        let expected_size_bytes = &((expected_event.size() * events_per_page) as u64).to_le_bytes();
        assert_eq!(actual_size_bytes, expected_size_bytes);

        // Try reading another page.
        let mut buffer = VecOutputBuffer::new(*PAGE_SIZE as usize);
        assert_eq!(queue.read(&mut buffer), error!(EAGAIN));
    }
}
