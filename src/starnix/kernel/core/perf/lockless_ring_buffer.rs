// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This is a lockless ring buffer modeled after https://docs.kernel.org/trace/ring-buffer-design.html
use crate::mm::memory::MemoryObject;
use crate::vfs::OutputBuffer;
use fuchsia_rcu::RcuDroppable;
use fuchsia_runtime::vmar_root_self;
use fuchsia_trace;
use shared_buffer::SharedBuffer;
use starnix_logging::{log_error, log_info, log_warn};
use starnix_sync::{LockDepMutex, PerfRingBufferStateLock};
use starnix_types::PAGE_SIZE;
use starnix_uapi::errors::Errno;
use starnix_uapi::{errno, error, from_status_like_fdio};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(RcuDroppable)]
struct Node {
    // page_data is the memory view for this page.
    page_data: SharedBuffer,
    // next and prev are pointers to the next and previous nodes.
    // creating a linked list of pages.
    next: AtomicUsize,
    prev: AtomicUsize,
    // offset to write new data in this page.
    write_offset: AtomicUsize,
    // Number of active writers currently reserving or writing to this page.
    active_writers: AtomicUsize,
}

const FLAG_MASK: usize = 0b11 << 62;
// Normal link between pages in the circular list.
const FLAG_NORMAL: usize = 0b00 << 62;
// Marks the link pointing to the head page (the oldest page with data).
const FLAG_HEADER: usize = 0b01 << 62;
// Set on the link pointing to the head page when it is being swapped by the reader.
const FLAG_UPDATE: usize = 0b10 << 62;

// The highest bit (bit 63) of `active_writers` is used as a flag indicating if the page is active.
// When the page is finalized, this bit is cleared to 0.
const PAGE_ACTIVE_BIT: usize = 1 << 63;
// The second highest bit (bit 62) of `active_writers` is used to coordinate/claim finalization.
const PAGE_FINALIZED_BIT: usize = 1 << 62;
const FLAGS_MASK: usize = PAGE_ACTIVE_BIT | PAGE_FINALIZED_BIT;
const ACTIVE_WRITERS_MASK: usize = !FLAGS_MASK;

// When yielding under high contention, if we busy-spin we risk priority inversion and CPU starvation
// of preempted writers. Sleeping for 50 microseconds gives the OS scheduler sufficient window (accounting
// for 2-5us context switch overhead and 15-30us thread execution time) to reschedule and execute the
// preempted writer so it can release its reservation.
const SPIN_SLEEP_DURATION: std::time::Duration = std::time::Duration::from_micros(50);

// When progressive sleep is triggered (exceeding 1000 yields), we back off with a 10 milliseconds sleep.
// This is a robust scheduling window to guarantee that Zircon schedules the preempted writer thread
// to complete its commit/release logic.
const PROGRESSIVE_SLEEP_DURATION: std::time::Duration = std::time::Duration::from_millis(10);

impl Node {
    fn get_index(val: usize) -> usize {
        val & !FLAG_MASK
    }
    fn get_flags(val: usize) -> usize {
        val & FLAG_MASK
    }
    fn make_val(index: usize, flags: usize) -> usize {
        index | flags
    }

    /// Finalizes the page by writing the data size to the header if tracing is enabled.
    fn finalize(&self) {
        // Attempt to claim the finalization role for this page.
        // If another thread already claimed it, return early to avoid a data race on `page_data`.
        let old_val = self.active_writers.fetch_or(PAGE_FINALIZED_BIT, Ordering::AcqRel);
        if old_val & PAGE_FINALIZED_BIT != 0 {
            return;
        }

        // - Acquire: We load `write_offset` with Acquire ordering to synchronize with all concurrent
        //   writers that updated it via CAS in `try_reserve_on_page`. This ensures that
        //   all non-atomic writes to `page_data` performed by the writers before their reservation was completed
        //   are fully visible to this finalizing thread.
        let write_offset = self.write_offset.load(Ordering::Acquire);
        let data_size = std::cmp::min(write_offset, (*PAGE_SIZE) as usize)
            - LocklessRingBuffer::PAGE_HEADER_SIZE;
        self.page_data.write_at(8, &(data_size as u64).to_le_bytes());

        // Clear the `PAGE_ACTIVE_BIT` from `active_writers` with Release ordering to publish the completed page.
        // Release semantics here guarantees that if a reader observes the PAGE_ACTIVE_BIT being cleared,
        // it is safe to read the page as it will also observe all the completed writes from the writers for that page.
        self.active_writers.fetch_and(!PAGE_ACTIVE_BIT, Ordering::Release);
    }

    /// Releases a writer reservation on this node, and finalizes the node if it was the last writer.
    fn release_writer(&self) {
        // Using Release ordering with fetch_sub make the loading part use Relaxed ordering, and the
        // store part Release. This means the value will be updated before the Acquire loading.
        let prev_writers = self.active_writers.fetch_sub(1, Ordering::Release);

        // If this was the last writer finalizing an active page (`PAGE_ACTIVE_BIT | 1`), we ensure the page is finalized.
        // Note that if the page was already finalized by the advancing thread, `PAGE_FINALIZED_BIT` will be set,
        // meaning `prev_writers` will have it set and we won't enter this branch, which is correct since it's already finalized.
        if prev_writers == PAGE_ACTIVE_BIT | 1 {
            let write_offset = self.write_offset.load(Ordering::Acquire);
            if write_offset >= (*PAGE_SIZE) as usize {
                // Since this thread holds a valid `Reservation`, `ref_count` remains > 0, which blocks
                // `disable()` from completing and shrinking the VMO. This guarantees that the ring VMO
                // remains enabled and valid for memory access.
                // Without this check and finalization on failure, there is a liveness bug where a full page
                // could never be finalized because the successful writer already skipped finalization.
                self.finalize();
            }
        }
    }
}

// We are only 64 bit today, but make it easy to find this assumption if we go to a smaller arch.
static_assertions::const_assert!(std::mem::size_of::<usize>() == 8);

// Use the high bit to indicate the ring is enabled. This makes the default, 0,
// which is disabled, an easy value to reason about. And the ref count is the lower 63 bits.
const RING_ENABLED_BIT: usize = 1 << 63;

#[derive(RcuDroppable)]
pub struct LocklessRingBuffer {
    vmo: MemoryObject,
    mapping: SharedBuffer,
    // linked list of pages that represent the ring, backed by the mapping (and the vmo).
    nodes: Vec<Node>,
    // The page where to read the data from the ring.
    head_page: AtomicUsize,
    // The page where writes add to the ring.
    tail_page: AtomicUsize,

    // A page used by the reader of the ring.
    reader_page: AtomicUsize,

    // Tracks the global number of active readers and writers of the ring in the lower 63 bits.
    // The highest bit (RING_ENABLED_BIT) is set when the ring is enabled.
    ref_count: AtomicUsize,
    // True indicates drop old pages to write new data when the ring is full. If false, writes fail until
    // the data is read, effectively dropping new data and keeping the old data.
    overwrite: bool,
    // Number of dropped pages when overwrite is true.
    dropped_pages: std::sync::atomic::AtomicU64,
    // Used to calculate the delta from the last event. Since this is written to by multiple threads,
    // we keep an atomic value for this.
    prev_timestamp: std::sync::atomic::AtomicU64,
    // The async trace event ID for write events.
    write_event_async_id: fuchsia_trace::Id,
    // Tracks whether a reader is currently active, used to validate that concurrent reads
    // on a single ring buffer are not supported and do not happen. This can be removed if we
    // are convinced it is not necessary.
    reader_active: std::sync::atomic::AtomicBool,
    // Mutex to serialize enable() and disable() calls to prevent racing. This should
    // never be accessed by other methods to avoid locking during reading and writing.
    state_mutex: LockDepMutex<(), PerfRingBufferStateLock>,
}
impl LocklessRingBuffer {
    // This is an Ftrace page header consisting of a u64 timestamp at offset 0 and a u64 data size at offset 8.
    pub const PAGE_HEADER_SIZE: usize = 16;

    /// Creates a new LocklessRingBuffer.
    /// size_bytes: Size of the ring buffer. Must be at least PAGE_SIZE * 3 and will be rounded up to a multiple of PAGE_SIZE.
    /// overwrite: If true, old pages will be dropped when the ring is full. If false, writes will fail until the data is read.
    /// write_event_async_id: The async trace event ID for write events.
    pub fn new(
        size_bytes: usize,
        overwrite: bool,
        write_event_async_id: fuchsia_trace::Id,
    ) -> Result<Self, Errno> {
        let requested_pages = (size_bytes + (*PAGE_SIZE) as usize - 1) / (*PAGE_SIZE) as usize;
        // 3 pages are needed, 1 for the read page, 1 for head, and one to swap.
        let pages = std::cmp::max(3, requested_pages);
        let total_nodes = pages;
        let capacity = total_nodes * (*PAGE_SIZE) as usize;

        // Create VMO
        let vmo: MemoryObject =
            zx::Vmo::create_with_opts(zx::VmoOptions::RESIZABLE, capacity as u64)
                .map_err(|_| errno!(ENOMEM))?
                .into();
        let vmo = vmo.with_zx_name(b"starnix:tracefs");
        // Map VMO
        let addr = vmar_root_self()
            .map(
                0,
                vmo.as_vmo().expect("vmo must exist"),
                0,
                capacity,
                zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE,
            )
            .map_err(|e| from_status_like_fdio!(e))?;
        // SAFETY: The address returned by `vmar.map` is valid for `capacity` bytes.
        let mapping = unsafe { SharedBuffer::new(addr as *mut u8, capacity) };

        // Create nodes
        let mut nodes = Vec::with_capacity(total_nodes);
        let base_ptr = addr as *mut u8;
        for i in 0..total_nodes {
            // SAFETY: `base_ptr` points to a valid mapping of size `capacity`.
            // `i * PAGE_SIZE` is within the bounds of this mapping since `i < total_nodes`.
            // The memory range `[page_ptr, page_ptr + PAGE_SIZE)` is valid and mapped.
            let page_ptr = unsafe { base_ptr.add(i * (*PAGE_SIZE) as usize) };

            nodes.push(Node {
                // SAFETY: `page_ptr` is in bounds of the mapped region and valid for `PAGE_SIZE` bytes.
                page_data: unsafe { SharedBuffer::new(page_ptr, (*PAGE_SIZE) as usize) },
                next: AtomicUsize::new(0),
                prev: AtomicUsize::new(0),
                write_offset: AtomicUsize::new(LocklessRingBuffer::PAGE_HEADER_SIZE),
                // Initialize active_writers to PAGE_ACTIVE_BIT (1) since the page is active.
                active_writers: AtomicUsize::new(PAGE_ACTIVE_BIT),
            });
        }
        // Link first `pages - 1` nodes in a circle. The last page is reserved for the reader initialization.
        let circle_size = pages - 1;
        for i in 0..circle_size {
            let next_idx = (i + 1) % circle_size;
            let prev_idx = (i + circle_size - 1) % circle_size;
            nodes[i].next.store(Node::make_val(next_idx, FLAG_NORMAL), Ordering::Relaxed);
            nodes[i].prev.store(Node::make_val(prev_idx, FLAG_NORMAL), Ordering::Relaxed);
        }
        // Initialize the reader page, mark head page (node 0).
        nodes[circle_size - 1].next.store(Node::make_val(0, FLAG_HEADER), Ordering::Relaxed);
        // Initialize reader page pointers to point to the head page and its predecessor
        nodes[pages - 1].next.store(Node::make_val(0, FLAG_NORMAL), Ordering::Relaxed);
        nodes[pages - 1]
            .prev
            .store(Node::make_val(circle_size - 1, FLAG_NORMAL), Ordering::Relaxed);
        // Initialize reader page size to 0
        nodes[pages - 1].page_data.write_at(8, &0u64.to_le_bytes());
        let buffer = Self {
            vmo,
            mapping,
            nodes,
            head_page: AtomicUsize::new(0),
            tail_page: AtomicUsize::new(0),
            reader_page: AtomicUsize::new(pages - 1),

            ref_count: AtomicUsize::new(RING_ENABLED_BIT),
            overwrite,
            dropped_pages: std::sync::atomic::AtomicU64::new(0),
            prev_timestamp: std::sync::atomic::AtomicU64::new(
                zx::BootInstant::get().into_nanos() as u64
            ),
            write_event_async_id,
            reader_active: std::sync::atomic::AtomicBool::new(false),
            state_mutex: Default::default(),
        };
        Ok(buffer)
    }
    pub fn dropped_pages(&self) -> u64 {
        self.dropped_pages.load(Ordering::Relaxed)
    }
}
// Debugging support for detecting live locks. Consider removing once
// the ring has baked for some time.
#[derive(Default, Debug)]
struct YieldTracker {
    /// Number of times we yielded because the next page was being updated by the reader.
    update_flag: u64,
    /// Number of times we yielded because the page to overwrite still had active writers.
    node_match: u64,
    /// Number of times we yielded because we failed to lock the head page for overwrite.
    head_lock: u64,
}
impl YieldTracker {
    fn total(&self) -> u64 {
        self.update_flag + self.node_match + self.head_lock
    }

    /// Yields or sleeps progressively based on the total retry count.
    fn yield_or_sleep(&self) {
        let total = self.total();
        if total > 1000 {
            std::thread::sleep(PROGRESSIVE_SLEEP_DURATION);
        } else if total > 100 {
            std::thread::sleep(SPIN_SLEEP_DURATION);
        } else {
            std::thread::yield_now();
        }
    }
}

/// Represents a reserved region of the ring buffer that is allocated, but not committed yet.
pub struct Reservation<'a> {
    pub offset: usize,
    pub node_idx: usize,
    pub size: usize,
    buffer: &'a LocklessRingBuffer,
    committed: bool,
}

impl<'a> Reservation<'a> {
    pub fn write_at(&self, rel_offset: usize, data: &[u8]) {
        assert!(rel_offset + data.len() <= self.size, "Write exceeds reservation size");
        self.buffer.mapping.write_at(self.offset + rel_offset, data);
    }

    fn release(&mut self) {
        if self.committed {
            return;
        }

        let node_idx = self.node_idx;
        let node = &self.buffer.nodes[node_idx];

        node.release_writer();

        self.buffer.ref_count.fetch_sub(1, Ordering::Release);
        self.committed = true;
    }
}

impl<'a> Drop for Reservation<'a> {
    fn drop(&mut self) {
        if !self.committed {
            starnix_logging::log_warn!("LocklessRingBuffer: Reservation dropped without commit");
        }
        self.release();
    }
}

impl<'a> std::fmt::Debug for Reservation<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reservation")
            .field("offset", &self.offset)
            .field("node_idx", &self.node_idx)
            .field("size", &self.size)
            .field("committed", &self.committed)
            .finish()
    }
}
#[derive(Debug, PartialEq, Eq)]
enum AdvanceResult {
    /// The tail page has been successfully advanced or transitioned.
    Advanced,
    /// Failed to advance due to transient contention. The caller should yield and retry.
    Yielded,
    /// A hard error occurred (e.g. buffer is full and overwrite is disabled).
    Error(Errno),
}

impl LocklessRingBuffer {
    /// Attempts to reserve space on the current tail page.
    ///
    /// Returns `Ok((offset, now, delta))` if successful, or `Err(())` if the page is full.
    fn try_reserve_on_page(
        &self,
        tail_node: &Node,
        size: usize,
    ) -> Result<(usize, zx::BootInstant, zx::Duration<zx::BootTimeline>), ()> {
        if !self.is_enabled() {
            log_warn!(
                "LocklessRingBuffer: canceling try_reserve_on_page because ring is disabled."
            );
            return Err(());
        }

        // Atomically reserve space on the page using a bounded CAS loop.
        // By verifying that `current_offset + size <= PAGE_SIZE` before executing the CAS,
        // we guarantee that failed reservations never advance `write_offset`, keeping it exactly
        // clamped to valid data boundaries.
        let mut current_offset = tail_node.write_offset.load(Ordering::Acquire);
        loop {
            if current_offset + size > (*PAGE_SIZE) as usize {
                return Err(());
            }
            match tail_node.write_offset.compare_exchange_weak(
                current_offset,
                current_offset + size,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(actual) => current_offset = actual,
            }
        }

        let now_candidate = zx::BootInstant::get().into_nanos() as u64;
        let actual_prev = self.prev_timestamp.fetch_max(now_candidate, Ordering::AcqRel);
        let final_now_nanos = std::cmp::max(now_candidate, actual_prev);
        let now = zx::BootInstant::from_nanos(final_now_nanos as i64);

        // First write on this page. Set timestamp.
        let delta_nanos = if current_offset == LocklessRingBuffer::PAGE_HEADER_SIZE {
            tail_node.page_data.write_at(0, &final_now_nanos.to_le_bytes());
            0
        } else {
            final_now_nanos.saturating_sub(actual_prev)
        };

        let delta = zx::Duration::from_nanos(delta_nanos as i64);

        Ok((current_offset, now, delta))
    }
    /// Advances the tail page to the next page in the ring.
    ///
    /// Handles marking the current page as full, moving the head page if overwrite is enabled,
    /// and updating the global `tail_page` pointer.
    fn advance_to_next_page(
        &self,
        tail_node: &Node,
        tail_val: usize,
        yield_tracker: &mut YieldTracker,
    ) -> AdvanceResult {
        // Short-circuit if another thread has already advanced the tail page past tail_val.
        if self.tail_page.load(Ordering::Acquire) != tail_val {
            return AdvanceResult::Advanced;
        }

        // If there are no active writers, finalize the page.
        // We need to check both here and in commit() (and Drop) to avoid race conditions.
        // Specifically, a writer might finish and see the page is not full yet, while the
        // thread that failed to fit its data and is moving to the next page hasn't marked it full yet.
        // By checking in both places, we ensure that at least one thread sees both conditions.
        if self.is_enabled() {
            tail_node.finalize();
        }

        let next_val = tail_node.next.load(Ordering::Acquire);
        let next_idx = Node::get_index(next_val);
        let next_flags = Node::get_flags(next_val);
        if next_flags == FLAG_UPDATE {
            // Reader is swapping this page. Wait.
            starnix_logging::log_warn!(
                "Reservation yielding due to FLAG_UPDATE on node {}",
                next_idx
            );
            yield_tracker.update_flag += 1;
            yield_tracker.yield_or_sleep();
            return AdvanceResult::Yielded;
        } else if next_flags == FLAG_HEADER {
            if self.overwrite {
                // Check if any active writer is on the page we want to overwrite.
                if (self.nodes[next_idx].active_writers.load(Ordering::Acquire)
                    & ACTIVE_WRITERS_MASK)
                    > 0
                {
                    yield_tracker.node_match += 1;
                    yield_tracker.yield_or_sleep();
                    return AdvanceResult::Yielded;
                }

                starnix_logging::log_warn!(
                    "LocklessRingBuffer Overwriting page {} (overwriting={})",
                    next_idx,
                    self.overwrite
                );
                // Try to lock the head page
                let expected_next = Node::make_val(next_idx, FLAG_HEADER);
                let locked_next = Node::make_val(next_idx, FLAG_UPDATE);
                // AcqRel on success:
                // - Acquire: Ensures we see the up-to-date links of the head page we are about to move.
                // - Release: Ensures our transition to FLAG_UPDATE (locking) is visible to others.
                // Relaxed on failure: We fail to acquire the lock and just retry.
                if tail_node
                    .next
                    .compare_exchange_weak(
                        expected_next,
                        locked_next,
                        Ordering::AcqRel,
                        Ordering::Relaxed,
                    )
                    .is_ok()
                {
                    // Move the head page.
                    let head_node = &self.nodes[next_idx];
                    let head_next_val = head_node.next.load(Ordering::Acquire);
                    let head_next_idx = Node::get_index(head_next_val);

                    // Set FLAG_HEADER on the new head pointer
                    let new_head_next = Node::make_val(head_next_idx, FLAG_HEADER);
                    head_node.next.store(new_head_next, Ordering::Release);

                    // Update global head_page
                    self.head_page.store(head_next_idx, Ordering::Release);

                    // Unlock the old head pointer (tail_node.next) and make it FLAG_NORMAL
                    let unlocked_next = Node::make_val(next_idx, FLAG_NORMAL);
                    tail_node.next.store(unlocked_next, Ordering::Release);
                    self.dropped_pages.fetch_add(1, Ordering::Relaxed);
                    fuchsia_trace::async_instant!(
                        self.write_event_async_id,
                        "starnix:trace_meta",
                        "page dropped"
                    );
                    return AdvanceResult::Advanced;
                } else {
                    // Failed to lock head_page. Retry.
                    yield_tracker.head_lock += 1;
                    yield_tracker.yield_or_sleep();
                    return AdvanceResult::Yielded;
                }
            } else {
                // Buffer is full and overwrite is false.
                starnix_logging::log_error!("LocklessRingBuffer is full");
                return AdvanceResult::Error(errno!(ENOSPC));
            }
        }

        let next_node = &self.nodes[next_idx];
        // Only the thread that successfully transitions write_offset from PAGE_SIZE (or full)
        // to PAGE_HEADER_SIZE is allowed to reset the page and advance the tail pointer.
        // This prevents the tail-advancing pointer race and slow-thread clobbering.
        let (should_advance, won_reset) = {
            let mut current = next_node.write_offset.load(Ordering::Acquire);
            loop {
                // Another thread advanced the tail page.
                if self.tail_page.load(Ordering::Acquire) != tail_val {
                    break (false, false);
                }
                // Edge Case: If `current` is already `PAGE_HEADER_SIZE`, another concurrent writer thread has
                // already won the reset race on this target page. Since `self.tail_page` hasn't advanced yet,
                // we must still return `should_advance = true` to cooperate in advancing the global `tail_page`
                // pointer, while returning `won_reset = false` to avoid redundantly clobbering page metadata.
                if current == LocklessRingBuffer::PAGE_HEADER_SIZE {
                    break (true, false);
                }
                match next_node.write_offset.compare_exchange_weak(
                    current,
                    LocklessRingBuffer::PAGE_HEADER_SIZE,
                    Ordering::AcqRel,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break (true, true),
                    Err(actual) => current = actual,
                }
            }
        };

        if should_advance {
            if won_reset {
                // Reset `active_writers` to PAGE_ACTIVE_BIT (1) since the page is active.
                next_node.active_writers.store(PAGE_ACTIVE_BIT, Ordering::Release);
                // Clear the size field in the page header to avoid reader seeing stale size.
                next_node.page_data.write_at(8, &0u64.to_le_bytes());
            }

            // Try to move tail_page to next_val.
            // If it fails, it means another thread has already successfully advanced tail_page.
            if let Err(err) = self.tail_page.compare_exchange(
                tail_val,
                next_val,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                starnix_logging::log_debug!("Tail page already advanced by another thread: {err}");
            }
        }
        AdvanceResult::Advanced
    }

    pub fn reserve(
        &self,
        size: usize,
    ) -> Result<(Reservation<'_>, zx::BootInstant, zx::Duration<zx::BootTimeline>), Errno> {
        // Check that the reservation is non-zero and fits within a page.
        if size == 0 || size > (*PAGE_SIZE) as usize - LocklessRingBuffer::PAGE_HEADER_SIZE {
            return error!(EINVAL);
        }
        // Increment ref_count if enabled.
        let mut val = self.ref_count.load(Ordering::Acquire);
        loop {
            if val & RING_ENABLED_BIT == 0 {
                return error!(ENOMEM);
            }
            // Acquire on success:
            // - Acquire: Synchronizes with the Release store in `enable()`. This guarantees that if the writer
            //   observes that the ring is enabled, it will also observe all prior state initialization and memory
            //   setups made by `enable()` before starting any writes.
            // - Release: Synchronizes with `disable()`'s Acquire load on `ref_count`, ensuring that if `disable()`
            //   observes a zero active writer count, it is guaranteed that this writer has either not yet entered
            //   or has fully completed its operations.
            // Note: Only the decrement in `release()` needs to be Release to ensure that the data payload writes
            // are fully visible. Since the writer has not yet written any payload data to the page, there are no
            // memory writes that need to be published to other threads via Release ordering at this point.
            match self.ref_count.compare_exchange_weak(
                val,
                val + 1,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => val = actual,
            }
        }

        // Create a scope guard to decrement ref_count on error, panic, or early exit.
        let ref_count_guard = scopeguard::guard(&self.ref_count, |ref_count| {
            ref_count.fetch_sub(1, Ordering::Release);
        });

        // Lock the reservation.
        let mut yield_tracker = YieldTracker::default();
        let result = loop {
            if !self.is_enabled() {
                log_info!("LocklessRingBuffer: canceling reserve because ring is disabled.");
                break error!(ENOMEM);
            }
            let tail_val = self.tail_page.load(Ordering::Acquire);
            let tail_idx = Node::get_index(tail_val);
            let tail_node = &self.nodes[tail_idx];

            // Increment the writer count before trying to reserve space, if it fails decrement it.
            // We use compare_exchange_weak in a loop to ensure we only increment if the PAGE_ACTIVE_BIT is still set.
            // Note: The success and failure orderings are Relaxed because the writer has not yet written any payload
            // data to the page. Therefore, there are no memory writes that need to be published to other threads
            // via Release ordering.
            let mut active_incremented = false;
            let mut old_writers = tail_node.active_writers.load(Ordering::Acquire);
            loop {
                if old_writers & PAGE_ACTIVE_BIT == 0 {
                    break;
                }
                match tail_node.active_writers.compare_exchange_weak(
                    old_writers,
                    old_writers + 1,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        active_incremented = true;
                        break;
                    }
                    Err(actual) => old_writers = actual,
                }
            }

            let reservation_result = if active_incremented {
                self.try_reserve_on_page(tail_node, size)
            } else {
                Err(())
            };

            // Try to claim space on the current page.
            match reservation_result {
                Ok((offset, now, delta)) => {
                    break Ok((
                        Reservation {
                            offset: tail_idx * (*PAGE_SIZE) as usize + offset,
                            node_idx: tail_idx,
                            size,
                            buffer: self,
                            committed: false,
                        },
                        now,
                        delta,
                    ));
                }
                Err(()) => {
                    // Handle the race where the last successful writer decrements active_writers but
                    // sees prev_writers > 1 because another concurrent writer (this thread) has already
                    // incremented it. This thread fails reservation because the page is full, decrements
                    // active_writers to 0, and gets prev_writers == 1. We must check and finalize here to
                    // guarantee that full pages are always finalized even if the finalizer thread failed its reserve.
                    if active_incremented {
                        self.nodes[tail_idx].release_writer();
                    }
                    // Page is full. Advance to the next page.
                    match self.advance_to_next_page(tail_node, tail_val, &mut yield_tracker) {
                        AdvanceResult::Advanced | AdvanceResult::Yielded => {}
                        AdvanceResult::Error(e) => {
                            break Err(e);
                        }
                    }
                }
            }
            // Debug logging for high yield counts.
            if yield_tracker.total() % 100_000_000 == 0 && yield_tracker.total() > 0 {
                log_warn!(
                    "LocklessRingBuffer: spinning in reservation loop.  details = {yield_tracker:?}",
                );
            }
        };
        // Cleanup and return
        let total_yields = yield_tracker.total();
        if total_yields > 500_000 {
            starnix_logging::log_info!(
                "Reservation completed with yields: total={}, details={:?}",
                total_yields,
                yield_tracker
            );
        }
        match result {
            Ok((res, now, delta)) => {
                // Success: Disarm the scope guard so ref_count is NOT decremented.
                scopeguard::ScopeGuard::into_inner(ref_count_guard);
                Ok((res, now, delta))
            }
            Err(e) => {
                // Error: The scope guard will automatically decrement ref_count when dropped.
                Err(e)
            }
        }
    }
    pub fn commit(&self, mut reservation: Reservation<'_>) {
        reservation.release();
    }

    pub fn swap_reader_page(&self) -> Option<usize> {
        let mut retries = 0;
        loop {
            let head_val = self.head_page.load(Ordering::Acquire);
            let head_idx = Node::get_index(head_val);
            let tail_val = self.tail_page.load(Ordering::Acquire);
            let tail_idx = Node::get_index(tail_val);
            if head_idx == tail_idx {
                // Ring is empty or we are writing to the only page.
                return None;
            }
            let head_node = &self.nodes[head_idx];
            // Perform a single Acquire load on `active_writers` to establish happens-before with the finalizing writer.
            // We check concurrently that the page is finalized (PAGE_ACTIVE_BIT is cleared to 0) and that there are no active writers (count is 0).
            let active_writers = head_node.active_writers.load(Ordering::Acquire);
            // We can swap the page if it is no longer active and has no active writers.
            // We ignore the PAGE_FINALIZED_BIT when determining if the page is ready.
            if (active_writers & (PAGE_ACTIVE_BIT | ACTIVE_WRITERS_MASK)) != 0 {
                return None;
            }
            let mut size_bytes = [0u8; 8];
            head_node.page_data.read_at(8, &mut size_bytes);
            let size = u64::from_le_bytes(size_bytes);
            if size == 0 {
                // This can happen even if `head_page != tail_page` (the ring is not empty).
                // Specifically, if a writer advanced the tail to the next page, the global
                // `head_page` pointer may have advanced, but the active writer on this new
                // `head_page` has not yet called `commit()` or finalized the page.
                // Thus, the data size field at offset 8 in the page header remains 0.
                return None;
            }
            let next_val = head_node.next.load(Ordering::Acquire);
            let next_idx = Node::get_index(next_val);
            let prev_val = head_node.prev.load(Ordering::Acquire);
            let prev_idx = Node::get_index(prev_val);
            let prev_node = &self.nodes[prev_idx];
            // Try to lock the head page by setting UPDATE flag on prev.next
            let expected_next = Node::make_val(head_idx, FLAG_HEADER);
            let locked_next = Node::make_val(head_idx, FLAG_UPDATE);
            if prev_node
                .next
                .compare_exchange_weak(
                    expected_next,
                    locked_next,
                    Ordering::AcqRel,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                // Locked! Now perform the swap.
                let reader_idx = self.reader_page.load(Ordering::Acquire);
                let next_node = &self.nodes[next_idx];
                // Update pointers to insert reader node.
                self.nodes[reader_idx]
                    .prev
                    .store(Node::make_val(prev_idx, FLAG_NORMAL), Ordering::Relaxed);
                self.nodes[reader_idx]
                    .next
                    .store(Node::make_val(next_idx, FLAG_HEADER), Ordering::Relaxed);
                next_node.prev.store(Node::make_val(reader_idx, FLAG_NORMAL), Ordering::Relaxed);
                // Reset write offset of the new head page (old reader page)
                self.nodes[reader_idx]
                    .write_offset
                    .store(LocklessRingBuffer::PAGE_HEADER_SIZE, Ordering::Relaxed);
                // Reset `active_writers` to PAGE_ACTIVE_BIT before recycling.
                self.nodes[reader_idx].active_writers.store(PAGE_ACTIVE_BIT, Ordering::Relaxed);
                // Clear the size field in the page header to avoid reader seeing stale size.
                self.nodes[reader_idx].page_data.write_at(8, &0u64.to_le_bytes());
                // Unlock prev_node and set its next to reader_idx as NORMAL
                let unlocked_val = Node::make_val(reader_idx, FLAG_NORMAL);
                prev_node.next.store(unlocked_val, Ordering::Release);
                // Update global pointers
                self.head_page.store(next_idx, Ordering::Release);
                self.reader_page.store(head_idx, Ordering::Release);
                // Return the old head page index (now reader page)
                return Some(head_idx);
            }
            // Failed to lock, retry.
            starnix_logging::log_warn!("swap_reader_page failed to lock, yielding");
            retries += 1;
            if retries >= 100_000 {
                starnix_logging::log_error!("LocklessRingBuffer: HUNG in swap_reader_page loop");
                return None;
            }
            // Failed to acquire the lock for swapping. Spin and retry.
            // Note: The log message above says "yielding" but we actually spin here
            // to avoid context switch overhead for this short wait.
            std::hint::spin_loop();
        }
    }
    pub fn read(&self, buf: &mut dyn OutputBuffer) -> Result<usize, Errno> {
        // Validate that concurrent reads on a single ring buffer do not happen.
        // By design in both ftrace/tracefs and Starnix VFS, only a single reader is active at a time.
        if self.reader_active.swap(true, Ordering::AcqRel) {
            starnix_logging::log_error!(
                "LocklessRingBuffer: concurrent reads detected! Concurrent reads are not supported by design."
            );
            return error!(EBUSY);
        }

        let _lock_guard = scopeguard::guard(&self.reader_active, |reader_active| {
            reader_active.store(false, Ordering::Release);
        });

        // Increment ref_count if enabled to prevent disable() from shrinking the VMO
        // while we are reading from it. We use a compare_exchange loop instead of
        // fetch_add to avoid modifying the counter if the ring is already disabled,
        // which prevents live-locking disable().
        let mut val = self.ref_count.load(Ordering::Acquire);
        loop {
            if val & RING_ENABLED_BIT == 0 {
                return error!(EAGAIN);
            }
            match self.ref_count.compare_exchange_weak(
                val,
                val + 1,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => val = actual,
            }
        }

        let _guard = scopeguard::guard(&self.ref_count, |ref_count| {
            ref_count.fetch_sub(1, Ordering::Release);
        });

        if buf.available() < (*PAGE_SIZE) as usize {
            starnix_logging::log_error!(
                "Buffer is too small: {} bytes (needs {} bytes)",
                buf.available(),
                (*PAGE_SIZE) as usize
            );
            return error!(EINVAL);
        }
        // TODO(https://fxbug.dev/505532201): Consider handling buffers larger than a page.
        // this currently only works for a single page.
        if let Some(idx) = self.swap_reader_page() {
            let node = &self.nodes[idx];
            let mut offset = 0;
            let page_size = (*PAGE_SIZE) as usize;
            let bytes_written = buf.write_each(&mut move |segment| {
                let available = page_size - offset;
                if available == 0 {
                    return Ok(0);
                }
                let size = std::cmp::min(segment.len(), available);
                // SAFETY: We are writing to this segment, so it's safe to treat it as initialized after we write.
                // We cast it to `&mut [u8]` to pass to `read_at`.
                let segment_mut_u8 = unsafe {
                    std::slice::from_raw_parts_mut(segment.as_mut_ptr() as *mut u8, size)
                };
                node.page_data.read_at(offset, segment_mut_u8);
                offset += size;
                Ok(size)
            })?;
            return Ok(bytes_written);
        }
        error!(EAGAIN)
    }

    #[cfg(test)]
    pub(crate) fn size_bytes(&self) -> usize {
        self.nodes.len() * (*PAGE_SIZE) as usize
    }

    pub fn is_enabled(&self) -> bool {
        self.ref_count.load(Ordering::Acquire) & RING_ENABLED_BIT != 0
    }
    pub fn disable(&self) -> Result<u64, Errno> {
        let _lock = self.state_mutex.lock();
        // Clear the enabled bit.
        self.ref_count.fetch_and(!RING_ENABLED_BIT, Ordering::AcqRel);
        let mut yield_count: u64 = 0;
        loop {
            let active_writers = self.ref_count.load(Ordering::Acquire);
            if active_writers == 0 {
                break;
            }
            yield_count += 1;
            if yield_count % 1_000_000 == 0 {
                log_error!(
                    "LocklessRingBuffer: disable is waiting for {active_writers} active writers",
                );
            }
            // We use yield_now() here as a compromise:
            // - spin_wait would waste too much CPU if the writer is descheduled.
            // - sleep adds too much latency for what should be a very short wait (copying data).
            // Yielding gives other threads a chance to run while keeping latency low.
            std::thread::yield_now();
        }
        if yield_count > 500_000 {
            log_warn!("LocklessRingBuffer disable took {} yields", yield_count);
        }
        if let Err(e) = self.vmo.set_size(0) {
            starnix_logging::log_error!(
                "LocklessRingBuffer disable failed to set_size(0): {:?}",
                e
            );
            return Err(from_status_like_fdio!(e));
        }
        let dropped = self.dropped_pages.load(Ordering::Relaxed);
        Ok(dropped)
    }
    pub fn enable(&self) -> Result<zx::BootInstant, Errno> {
        let _lock = self.state_mutex.lock();
        let initial_pages = self.nodes.len() - 1;
        let capacity = self.nodes.len() * (*PAGE_SIZE) as usize;
        if let Err(e) = self.vmo.set_size(capacity as u64) {
            starnix_logging::log_error!("LocklessRingBuffer enable failed to set_size: {:?}", e);
            return Err(from_status_like_fdio!(e));
        }
        let now = zx::BootInstant::get();
        // Reset state
        self.head_page.store(0, Ordering::Release);
        self.tail_page.store(0, Ordering::Release);

        self.reader_page.store(initial_pages, Ordering::Release);

        self.dropped_pages.store(0, Ordering::Release);
        self.prev_timestamp.store(now.into_nanos() as u64, Ordering::Release);

        for i in 0..self.nodes.len() {
            self.nodes[i]
                .write_offset
                .store(LocklessRingBuffer::PAGE_HEADER_SIZE, Ordering::Release);
            self.nodes[i].active_writers.store(PAGE_ACTIVE_BIT, Ordering::Release);
        }
        for i in 0..initial_pages {
            let next_idx = (i + 1) % initial_pages;
            let prev_idx = (i + initial_pages - 1) % initial_pages;
            self.nodes[i].next.store(Node::make_val(next_idx, FLAG_NORMAL), Ordering::Relaxed);
            self.nodes[i].prev.store(Node::make_val(prev_idx, FLAG_NORMAL), Ordering::Relaxed);
        }
        self.nodes[initial_pages - 1].next.store(Node::make_val(0, FLAG_HEADER), Ordering::Relaxed);

        // Initialize reader page size to 0
        self.nodes[initial_pages].page_data.write_at(8, &0u64.to_le_bytes());
        // Initialize first page.
        let _ = self.vmo.as_vmo().expect("vmo must exist").op_range(zx::VmoOp::ZERO, 0, *PAGE_SIZE);
        let nanos = now.into_nanos() as u64;
        self.nodes[0].page_data.write_at(0, &nanos.to_le_bytes());

        // Set enabled bit LAST to ensure state is fully reset before writes start.
        self.ref_count.store(RING_ENABLED_BIT, Ordering::Release);
        Ok(now)
    }
}
impl Drop for LocklessRingBuffer {
    fn drop(&mut self) {
        let (ptr, len) = self.mapping.as_ptr_len();
        // SAFETY: The mapping was created in `new` and is valid for this lifetime.
        // We are freeing it now because the object is being destroyed.
        unsafe {
            let _ = vmar_root_self().unmap(ptr as usize, len);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::buffers::VecOutputBuffer;
    use crate::vfs::{Buffer, OutputBufferCallback, PeekBufferSegmentsCallback};
    use std::sync::Arc;
    #[repr(C)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct TestMessage {
        thread_index: u32,
        timestamp_nanos: u64,
        delta: u64,
        data: [u8; 12],
    }
    impl TestMessage {
        pub const SIZE: usize = 32;
        fn to_bytes(&self) -> [u8; TestMessage::SIZE] {
            let mut bytes = [0u8; TestMessage::SIZE];
            bytes[0..4].copy_from_slice(&self.thread_index.to_le_bytes());
            bytes[4..12].copy_from_slice(&self.timestamp_nanos.to_le_bytes());
            bytes[12..20].copy_from_slice(&self.delta.to_le_bytes());
            bytes[20..32].copy_from_slice(&self.data);
            bytes
        }
        fn from_bytes(bytes: &[u8]) -> Self {
            let mut thread_index = [0u8; 4];
            thread_index.copy_from_slice(&bytes[0..4]);
            let mut timestamp_nanos = [0u8; 8];
            timestamp_nanos.copy_from_slice(&bytes[4..12]);
            let mut delta = [0u8; 8];
            delta.copy_from_slice(&bytes[12..20]);
            let mut data = [0u8; 12];
            data.copy_from_slice(&bytes[20..32]);
            Self {
                thread_index: u32::from_le_bytes(thread_index),
                timestamp_nanos: u64::from_le_bytes(timestamp_nanos),
                delta: u64::from_le_bytes(delta),
                data,
            }
        }
    }
    #[test]
    fn test_init() {
        let buffer =
            LocklessRingBuffer::new(3 * (*PAGE_SIZE) as usize, false, fuchsia_trace::Id::new())
                .unwrap();
        assert_eq!(buffer.size_bytes(), 3 * (*PAGE_SIZE) as usize);
    }
    #[test]
    fn test_reserve() {
        let buffer =
            LocklessRingBuffer::new(3 * (*PAGE_SIZE) as usize, false, fuchsia_trace::Id::new())
                .unwrap();
        let res = buffer.reserve(100).unwrap();
        assert_eq!(res.0.size, 100);
        assert_eq!(res.0.offset, LocklessRingBuffer::PAGE_HEADER_SIZE);
        assert_eq!(res.0.node_idx, 0);
    }
    #[test]
    fn test_commit() {
        let buffer =
            LocklessRingBuffer::new(3 * (*PAGE_SIZE) as usize, false, fuchsia_trace::Id::new())
                .unwrap();
        let res = buffer.reserve(100).unwrap();
        buffer.commit(res.0);
        assert_eq!(buffer.nodes[0].active_writers.load(Ordering::Relaxed), PAGE_ACTIVE_BIT);
    }
    #[test]
    fn test_swap_reader_page_empty() {
        let buffer =
            LocklessRingBuffer::new(3 * (*PAGE_SIZE) as usize, false, fuchsia_trace::Id::new())
                .unwrap();
        assert_eq!(buffer.swap_reader_page(), None);
    }
    #[test]
    fn test_swap_reader_page_success() {
        let buffer =
            LocklessRingBuffer::new(3 * (*PAGE_SIZE) as usize, false, fuchsia_trace::Id::new())
                .unwrap();
        // Fill node 0
        let res1 =
            buffer.reserve((*PAGE_SIZE) as usize - LocklessRingBuffer::PAGE_HEADER_SIZE).unwrap();
        buffer.commit(res1.0);
        // Next reserve should go to node 1
        let res2 = buffer.reserve(100).unwrap();
        assert_eq!(res2.0.node_idx, 1);
        buffer.commit(res2.0);
        // Page 0 has size > 0 and tail advanced to Page 1.
        // So swap_reader_page should succeed!
        let old_head = buffer.swap_reader_page();
        assert_eq!(old_head, Some(0));
        // And head_page should now have advanced to index 1.
        assert_eq!(buffer.head_page.load(Ordering::Relaxed), 1);
    }
    #[test]
    fn test_concurrent_reserve() {
        use std::sync::Arc;
        let buffer = Arc::new(
            LocklessRingBuffer::new(3 * (*PAGE_SIZE) as usize, false, fuchsia_trace::Id::new())
                .unwrap(),
        );
        let mut handles = vec![];
        for _ in 0..5 {
            let buffer_clone = Arc::clone(&buffer);
            let handle = std::thread::spawn(move || {
                for _ in 0..20 {
                    if let Ok(res) = buffer_clone.reserve(10) {
                        buffer_clone.commit(res.0);
                    }
                }
            });
            handles.push(handle);
        }
        for handle in handles {
            handle.join().unwrap();
        }
        for i in 0..buffer.nodes.len() {
            assert_eq!(buffer.nodes[i].active_writers.load(Ordering::Relaxed), PAGE_ACTIVE_BIT);
        }
    }
    #[test]
    fn test_reserve_moves_to_next_page() {
        let buffer =
            LocklessRingBuffer::new(3 * (*PAGE_SIZE) as usize, false, fuchsia_trace::Id::new())
                .unwrap();
        // Reserve most of the page, leaving only 50 bytes
        let res1 = buffer
            .reserve((*PAGE_SIZE) as usize - LocklessRingBuffer::PAGE_HEADER_SIZE - 50)
            .unwrap();
        buffer.commit(res1.0);
        // Try to reserve 100 bytes. It shouldn't fit on node 0.
        let res2 = buffer.reserve(100).unwrap();
        // It should be on node 1.
        assert_eq!(res2.0.node_idx, 1);
        assert_eq!(res2.0.offset, (*PAGE_SIZE) as usize + LocklessRingBuffer::PAGE_HEADER_SIZE);
    }
    #[test]
    fn test_reserve_overwrite() {
        let buffer =
            LocklessRingBuffer::new(3 * (*PAGE_SIZE) as usize, true, fuchsia_trace::Id::new())
                .unwrap();
        // Fill node 0 and commit
        let res1 =
            buffer.reserve((*PAGE_SIZE) as usize - LocklessRingBuffer::PAGE_HEADER_SIZE).unwrap();
        buffer.commit(res1.0);
        // Fill node 1 but do NOT commit
        let _res2 =
            buffer.reserve((*PAGE_SIZE) as usize - LocklessRingBuffer::PAGE_HEADER_SIZE).unwrap();
        // Now tail is at node 1. Next reserve wants to move to node 0.
        // But commit is still at node 0.
        // So it should trigger overwrite.
        let res3 = buffer.reserve(100).unwrap();
        // It should succeed and be on node 0.
        assert_eq!(res3.0.node_idx, 0);
        assert_eq!(buffer.dropped_pages(), 1);
        // And head_page should have advanced to 1.
        assert_eq!(buffer.head_page.load(Ordering::Relaxed), 1);
    }
    #[test]
    fn test_read() {
        let buffer =
            LocklessRingBuffer::new(3 * (*PAGE_SIZE) as usize, false, fuchsia_trace::Id::new())
                .unwrap();
        let res1 =
            buffer.reserve((*PAGE_SIZE) as usize - LocklessRingBuffer::PAGE_HEADER_SIZE).unwrap();
        let data = vec![1u8; (*PAGE_SIZE) as usize - LocklessRingBuffer::PAGE_HEADER_SIZE];
        res1.0.write_at(0, &data);
        buffer.commit(res1.0);
        // Advance commit page to node 1
        let res2 = buffer.reserve(100).unwrap();
        buffer.commit(res2.0);
        let mut dest = VecOutputBuffer::new((*PAGE_SIZE) as usize);
        let result = buffer.read(&mut dest);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), (*PAGE_SIZE) as usize);
        assert_eq!(&dest.data()[LocklessRingBuffer::PAGE_HEADER_SIZE..], &data[..]);
    }
    #[test]
    fn test_enable_disable() {
        let buffer =
            LocklessRingBuffer::new(3 * (*PAGE_SIZE) as usize, false, fuchsia_trace::Id::new())
                .unwrap();
        assert!(buffer.disable().is_ok());
        assert_eq!(buffer.ref_count.load(Ordering::Relaxed) & RING_ENABLED_BIT, 0);
        let res = buffer.reserve(100);
        assert_eq!(res.unwrap_err(), starnix_uapi::errno!(ENOMEM));
        assert!(buffer.enable().is_ok());
        assert_eq!(buffer.size_bytes(), 3 * (*PAGE_SIZE) as usize);
        let res = buffer.reserve(100);
        assert!(res.is_ok());
    }
    // Spawns a reader thread that periodically drains the lockless ring buffer.
    fn start_reader_thread(
        buffer_reader: Arc<LocklessRingBuffer>,
        writers_done_reader: Arc<std::sync::atomic::AtomicBool>,
    ) -> std::thread::JoinHandle<Vec<TestMessage>> {
        start_gated_reader_thread(buffer_reader, writers_done_reader, false)
    }

    // Spawns a reader thread that optionally yields until at least one page has been
    // dropped (overwritten) by writers before draining.
    fn start_gated_reader_thread(
        buffer_reader: Arc<LocklessRingBuffer>,
        writers_done_reader: Arc<std::sync::atomic::AtomicBool>,
        wait_for_dropped_pages: bool,
    ) -> std::thread::JoinHandle<Vec<TestMessage>> {
        std::thread::spawn(move || {
            if wait_for_dropped_pages {
                while buffer_reader.dropped_pages() == 0
                    && !writers_done_reader.load(Ordering::Acquire)
                {
                    std::thread::yield_now();
                }
            }
            let mut all_messages = Vec::new();
            let mut dest = VecOutputBuffer::new((*PAGE_SIZE) as usize);
            let mut consecutive_eagain = 0;
            loop {
                dest.reset();
                match buffer_reader.read(&mut dest) {
                    Ok(bytes_read) => {
                        consecutive_eagain = 0;
                        assert_eq!(bytes_read, (*PAGE_SIZE) as usize);
                        let mut header_ts_bytes = [0u8; 8];
                        header_ts_bytes.copy_from_slice(&dest.data()[0..8]);
                        let header_timestamp = u64::from_le_bytes(header_ts_bytes);
                        let mut size_bytes = [0u8; 8];
                        size_bytes.copy_from_slice(&dest.data()[8..16]);
                        let data_size = u64::from_le_bytes(size_bytes) as usize;
                        let max_offset = LocklessRingBuffer::PAGE_HEADER_SIZE + data_size;
                        let mut offset = LocklessRingBuffer::PAGE_HEADER_SIZE;
                        let mut first_msg = true;
                        while offset + TestMessage::SIZE <= max_offset {
                            let msg_bytes = &dest.data()[offset..offset + TestMessage::SIZE];
                            let msg = TestMessage::from_bytes(msg_bytes);
                            if first_msg {
                                if header_timestamp != msg.timestamp_nanos {
                                    println!(
                                        "HEADER TIMESTAMP MISMATCH: header={}, msg={}",
                                        header_timestamp, msg.timestamp_nanos
                                    );
                                }
                                first_msg = false;
                            }
                            all_messages.push(msg);
                            offset += TestMessage::SIZE;
                        }
                    }
                    Err(e) if e == starnix_uapi::errno!(EAGAIN) => {
                        if writers_done_reader.load(Ordering::Acquire) {
                            let head_val = buffer_reader.head_page.load(Ordering::Relaxed);
                            let tail_val = buffer_reader.tail_page.load(Ordering::Relaxed);
                            // If the writers are done and the reader has caught up to the tail page
                            // index, all readable pages have been drained, so we break the loop.
                            if Node::get_index(head_val) == Node::get_index(tail_val) {
                                break;
                            }
                            consecutive_eagain += 1;
                            assert!(
                                consecutive_eagain < 500,
                                "LocklessRingBuffer reader stuck: consecutive EAGAIN limit exceeded (5s) after writers finished. head_page={}, tail_page={}, reader_page={}",
                                Node::get_index(head_val),
                                Node::get_index(tail_val),
                                buffer_reader.reader_page.load(Ordering::Relaxed)
                            );
                        }
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(e) => panic!("Unexpected error from read: {:?}", e),
                }
            }
            all_messages
        })
    }
    fn check_all_message_data(all_messages: &[TestMessage], num_threads: u32) {
        let mut prev_timestamp = 0;
        let mut thread_counts = vec![0; num_threads as usize];
        let mut out_of_order = 0;
        let mut corrupted = 0;
        for msg in all_messages {
            if msg.timestamp_nanos < prev_timestamp {
                println!("OUT OF ORDER: prev_timestamp={}, current={:?}", prev_timestamp, msg);
                out_of_order += 1;
            }
            prev_timestamp = msg.timestamp_nanos;
            if &msg.data != b"Event data\0\0" || msg.thread_index >= num_threads {
                println!("CORRUPTED: msg={:?}", msg);
                corrupted += 1;
            } else {
                thread_counts[msg.thread_index as usize] += 1;
            }
        }
        println!(
            "TEST_RESULT: Read {} messages, {} out of order, {} corrupted. Thread counts: {:?}",
            all_messages.len(),
            out_of_order,
            corrupted,
            thread_counts
        );
        // We do not assert out_of_order is 0 because wait-free reservation can cause slight reordering of timestamps.
        assert_eq!(corrupted, 0, "Found corrupted messages");
    }
    #[test]
    fn test_concurrent_read_write_4() {
        let num_threads = 4;
        let msgs_per_thread = 64;
        // 4 threads * 64 msgs * 32 bytes = 8192 bytes.
        // Each page can hold (4096 - 16) / 32 = 127 msgs (with 16 bytes unused at the end).
        // 256 msgs will take 2 full pages (254 msgs) + 2 msgs on a 3rd page.
        let buffer = Arc::new(
            LocklessRingBuffer::new(4 * (*PAGE_SIZE) as usize, false, fuchsia_trace::Id::new())
                .unwrap(),
        );
        let writers_done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut handles = vec![];
        // Spawn reader
        let buffer_reader = Arc::clone(&buffer);
        let writers_done_reader = Arc::clone(&writers_done);
        let reader_handle = start_reader_thread(buffer_reader, writers_done_reader);
        // Spawn writers
        for thread_index in 0..num_threads {
            let buffer_clone = Arc::clone(&buffer);
            let handle = std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(10 + thread_index as u64));
                for _ in 0..msgs_per_thread {
                    // Reserve exactly TestMessage::SIZE bytes
                    let (res, now, delta) = buffer_clone.reserve(TestMessage::SIZE).unwrap();
                    let msg = TestMessage {
                        thread_index,
                        timestamp_nanos: now.into_nanos() as u64,
                        delta: delta.into_nanos() as u64,
                        data: *b"Event data\0\0",
                    };
                    res.write_at(0, &msg.to_bytes());
                    buffer_clone.commit(res);
                    std::thread::sleep(std::time::Duration::from_nanos(thread_index as u64));
                }
            });
            handles.push(handle);
        }
        for handle in handles {
            handle.join().unwrap();
        }
        writers_done.store(true, Ordering::Release);
        let all_messages = reader_handle.join().unwrap();
        // Check the messages
        check_all_message_data(&all_messages, num_threads);
        // Trailing messages might remain on the final unfinalized tail page. We expect between 250 and 256 messages to be successfully read.
        assert!(
            all_messages.len() >= 250 && all_messages.len() <= 256,
            "Expected between 250 and 256 messages, got {}",
            all_messages.len()
        );
    }
    #[test]
    fn test_concurrent_read_write_1_thread() {
        let num_threads = 1;
        let msgs_per_thread = 256;
        // 1 thread * 256 msgs = 256 msgs.
        // Each page holds 127 msgs. 256 msgs requires 3 data pages.
        // We use 5 pages total to provide enough capacity.
        let buffer = Arc::new(
            LocklessRingBuffer::new(5 * (*PAGE_SIZE) as usize, true, fuchsia_trace::Id::new())
                .unwrap(),
        );
        let writers_done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut handles = vec![];
        // Spawn reader
        let buffer_reader = Arc::clone(&buffer);
        let writers_done_reader = Arc::clone(&writers_done);
        let reader_handle = start_reader_thread(buffer_reader, writers_done_reader);
        // Spawn writers
        for thread_index in 0..num_threads {
            let buffer_clone = Arc::clone(&buffer);
            let handle = std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(10 + thread_index as u64));
                for _ in 0..msgs_per_thread {
                    let (res, now, delta) = buffer_clone.reserve(TestMessage::SIZE).unwrap();
                    let msg = TestMessage {
                        thread_index,
                        timestamp_nanos: now.into_nanos() as u64,
                        delta: delta.into_nanos() as u64,
                        data: *b"Event data\0\0",
                    };
                    res.write_at(0, &msg.to_bytes());
                    buffer_clone.commit(res);
                    std::thread::sleep(std::time::Duration::from_nanos(thread_index as u64));
                }
            });
            handles.push(handle);
        }
        for handle in handles {
            handle.join().unwrap();
        }
        writers_done.store(true, Ordering::Release);
        let all_messages = reader_handle.join().unwrap();
        check_all_message_data(&all_messages, num_threads);
        assert_eq!(
            all_messages.len(),
            254,
            "Expected exactly 254 messages, got {}",
            all_messages.len()
        );
    }
    #[test]
    fn test_concurrent_read_write_8_threads() {
        let num_threads = 8;
        let msgs_per_thread = 128;
        // 4 threads * 64 msgs * 32 bytes = 8192 bytes.
        // 8 threads * 128 msgs = 1024 msgs.
        // Each page holds 127 msgs. 1024 msgs requires 9 data pages.
        // We use 12 pages total to provide enough capacity so no pages are overwritten.
        let buffer = Arc::new(
            LocklessRingBuffer::new(12 * (*PAGE_SIZE) as usize, false, fuchsia_trace::Id::new())
                .unwrap(),
        );
        let writers_done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let _writers_done_guard = scopeguard::guard(Arc::clone(&writers_done), |done| {
            done.store(true, Ordering::Release);
        });
        let mut handles = vec![];
        // Spawn reader
        let buffer_reader = Arc::clone(&buffer);
        let writers_done_reader = Arc::clone(&writers_done);
        let reader_handle = start_reader_thread(buffer_reader, writers_done_reader);
        // Spawn writers
        for thread_index in 0..num_threads {
            let buffer_clone = Arc::clone(&buffer);
            let handle = std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(10 + thread_index as u64));
                for _ in 0..msgs_per_thread {
                    // Reserve exactly TestMessage::SIZE bytes
                    let mut retries = 0;
                    let (res, now, delta) = loop {
                        match buffer_clone.reserve(TestMessage::SIZE) {
                            Ok(r) => break r,
                            Err(e) if e == starnix_uapi::errno!(ENOSPC) => {
                                retries += 1;
                                assert!(
                                    retries < 100,
                                    "LocklessRingBuffer: ENOSPC transient limit exceeded (100ms). Reader may have hung."
                                );
                                // Under high contention, writers can transiently catch up to the head page.
                                // This is expected and will clear up once the reader thread gets scheduled
                                // and swaps a page.
                                std::thread::sleep(std::time::Duration::from_millis(1));
                            }
                            Err(e) => panic!("Unexpected error: {:?}", e),
                        }
                    };
                    let msg = TestMessage {
                        thread_index,
                        timestamp_nanos: now.into_nanos() as u64,
                        delta: delta.into_nanos() as u64,
                        data: *b"Event data\0\0",
                    };
                    res.write_at(0, &msg.to_bytes());
                    buffer_clone.commit(res);
                    std::thread::sleep(std::time::Duration::from_nanos(thread_index as u64));
                }
            });
            handles.push(handle);
        }
        for handle in handles {
            handle.join().unwrap();
        }

        writers_done.store(true, Ordering::Release);
        let all_messages = reader_handle.join().unwrap();
        // Check the messages
        check_all_message_data(&all_messages, num_threads);
        // The last page in the circle (current tail page) cannot be swapped by the reader
        // (to prevent reading uncommitted/partial data). The number of messages left sitting on
        // the tail page when the writers finish depends entirely on concurrent thread scheduling
        // variance under high contention. We expect between 1008 and 1024 messages to be read
        // (leaving 0 to 16 messages unread on the final tail page).
        assert!(
            all_messages.len() >= 1008 && all_messages.len() <= 1024,
            "Expected between 1008 and 1024 messages, got {}",
            all_messages.len()
        );
    }
    #[test]
    fn test_disable_waits_for_ref_count() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        let buffer = Arc::new(
            LocklessRingBuffer::new(3 * (*PAGE_SIZE) as usize, true, fuchsia_trace::Id::new())
                .unwrap(),
        );
        // Thread 1: Reserves space, keeping ref_count = 1 (plus enabled bit).
        let res = buffer.reserve(100).unwrap();
        let disable_finished = Arc::new(AtomicBool::new(false));
        let disable_finished_clone = Arc::clone(&disable_finished);
        let buffer_clone = Arc::clone(&buffer);
        // Thread 2: Calls disable. This should block because ref_count has active writers.
        let handle = std::thread::spawn(move || {
            buffer_clone.disable().unwrap();
            disable_finished_clone.store(true, Ordering::Relaxed);
        });
        // Wait for Thread 2 to start and set size_bytes to 0
        while buffer.ref_count.load(Ordering::Acquire) & RING_ENABLED_BIT != 0 {
            std::hint::spin_loop();
        }
        // Verify that disable has NOT finished
        assert!(!disable_finished.load(Ordering::Relaxed));
        // Verify that new reserves fail with ENOMEM because disable() cleared RING_ENABLED_BIT
        let res2 = buffer.reserve(100);
        assert_eq!(res2.unwrap_err(), starnix_uapi::errno!(ENOMEM));
        // Now write data and commit the first reservation.
        // If the VMO had been shrunk, write_data would panic/fault here.
        let data = vec![42u8; 100];
        res.0.write_at(0, &data);
        buffer.commit(res.0);
        // Wait for Thread 2 to finish
        handle.join().unwrap();
        // Verify disable finished
        assert!(disable_finished.load(Ordering::Relaxed));
    }
    #[test]
    fn test_reservation_drop_cancels_writer_count() {
        let buffer =
            LocklessRingBuffer::new(3 * (*PAGE_SIZE) as usize, true, fuchsia_trace::Id::new())
                .unwrap();
        // 1. Reserve space. ref_count becomes 1 (plus enabled bit).
        let res = buffer.reserve(100).unwrap();
        assert_eq!(buffer.ref_count.load(Ordering::Relaxed), RING_ENABLED_BIT | 1);
        // 2. Drop the reservation without commit.
        std::mem::drop(res.0);
        // 3. Verify ref_count becomes 0 (plus enabled bit).
        assert_eq!(buffer.ref_count.load(Ordering::Relaxed), RING_ENABLED_BIT);
        // 4. Verify that a new reservation can be committed (proving the dropped one didn't block).
        let res2 = buffer.reserve(100).unwrap();
        buffer.commit(res2.0);
    }
    #[test]
    fn test_overwrite_and_read_4_pages() {
        let num_threads = 1;
        // Use 6 pages total (5 data pages) to allow exactly 4 readable pages
        // (1 page is the active commit page).
        let buffer = Arc::new(
            LocklessRingBuffer::new(6 * (*PAGE_SIZE) as usize, true, fuchsia_trace::Id::new())
                .unwrap(),
        );
        let writers_done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        // 5 data pages capacity + 2 overwritten pages = 7 pages to write.
        // Each page holds exactly 127 messages.
        let msgs_per_thread = 7 * 127;
        let buffer_clone = Arc::clone(&buffer);
        let thread_index = 0;
        let writer_handle = std::thread::spawn(move || {
            for _ in 0..msgs_per_thread {
                let (res, now, delta) = buffer_clone.reserve(TestMessage::SIZE).unwrap();
                let msg = TestMessage {
                    thread_index,
                    timestamp_nanos: now.into_nanos() as u64,
                    delta: delta.into_nanos() as u64,
                    data: *b"Event data\0\0",
                };
                res.write_at(0, &msg.to_bytes());
                buffer_clone.commit(res);
            }
        });
        writer_handle.join().unwrap();
        assert_eq!(buffer.dropped_pages(), 2, "Expected 2 pages to be dropped");
        writers_done.store(true, Ordering::Release);
        let buffer_reader = Arc::clone(&buffer);
        let writers_done_reader = Arc::clone(&writers_done);
        let reader_handle = start_reader_thread(buffer_reader, writers_done_reader);
        let all_messages = reader_handle.join().unwrap();
        check_all_message_data(&all_messages, num_threads);
        assert_eq!(all_messages.len(), 4 * 127, "Expected exactly 4 pages of messages");
    }
    #[test]
    fn test_reserve_full_producer_consumer() {
        let buffer =
            LocklessRingBuffer::new(3 * (*PAGE_SIZE) as usize, false, fuchsia_trace::Id::new())
                .unwrap();
        // Fill node 0
        let res1 =
            buffer.reserve((*PAGE_SIZE) as usize - LocklessRingBuffer::PAGE_HEADER_SIZE).unwrap();
        buffer.commit(res1.0);
        // Fill node 1
        let res2 =
            buffer.reserve((*PAGE_SIZE) as usize - LocklessRingBuffer::PAGE_HEADER_SIZE).unwrap();
        buffer.commit(res2.0);
        // Buffer is full (2 data pages used, head at 0, commit at 1 but next is head).
        // Next reserve should fail with ENOSPC because overwrite is false.
        let res3 = buffer.reserve(100);
        assert_eq!(res3.unwrap_err(), starnix_uapi::errno!(ENOSPC));
    }

    #[test]
    fn test_reserve_zero_size() {
        let buffer =
            LocklessRingBuffer::new(3 * (*PAGE_SIZE) as usize, false, fuchsia_trace::Id::new())
                .unwrap();
        let res = buffer.reserve(0);
        assert_eq!(res.unwrap_err(), starnix_uapi::errno!(EINVAL));
    }
    #[test]
    fn test_concurrent_overwrite_stability() {
        let num_threads = 4;
        let msgs_per_thread = 256;
        // Small buffer to force overwrites
        let buffer = Arc::new(
            LocklessRingBuffer::new(3 * (*PAGE_SIZE) as usize, true, fuchsia_trace::Id::new())
                .unwrap(),
        );
        // Synchronously fill the active circular buffer (2 data pages) to capacity.
        // The reader thread is gated below so it does not drain the buffer until
        // the writer threads wrap around to Node 0 and trigger a dropped page.
        let max_payload = (*PAGE_SIZE) as usize - LocklessRingBuffer::PAGE_HEADER_SIZE;
        let num_msgs = max_payload / TestMessage::SIZE;
        for _ in 0..(2 * num_msgs) {
            let (res, now, delta) = buffer.reserve(TestMessage::SIZE).unwrap();
            let msg = TestMessage {
                thread_index: 0,
                timestamp_nanos: now.into_nanos() as u64,
                delta: delta.into_nanos() as u64,
                data: *b"Event data\0\0",
            };
            res.write_at(0, &msg.to_bytes());
            buffer.commit(res);
        }

        let writers_done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut handles = vec![];

        let buffer_reader = Arc::clone(&buffer);
        let writers_done_reader = Arc::clone(&writers_done);
        // Gate reader until writers wrap around and trigger an overwrite (dropped page),
        // guaranteeing overwrite stability is exercised without relying on timing or sleeps.
        let reader_handle = start_gated_reader_thread(buffer_reader, writers_done_reader, true);

        // Spawn writers
        for thread_index in 0..num_threads {
            let buffer_clone = Arc::clone(&buffer);
            let handle = std::thread::spawn(move || {
                for _ in 0..msgs_per_thread {
                    if let Ok((res, now, delta)) = buffer_clone.reserve(TestMessage::SIZE) {
                        let msg = TestMessage {
                            thread_index,
                            timestamp_nanos: now.into_nanos() as u64,
                            delta: delta.into_nanos() as u64,
                            data: *b"Event data\0\0",
                        };
                        res.write_at(0, &msg.to_bytes());
                        buffer_clone.commit(res);
                    }
                    // Yield to allow other threads to run and cause contention
                    std::thread::yield_now();
                }
            });
            handles.push(handle);
        }
        for handle in handles {
            handle.join().unwrap();
        }
        writers_done.store(true, Ordering::Release);
        let all_messages = reader_handle.join().unwrap();
        // Check that data is not corrupted
        check_all_message_data(&all_messages, num_threads);
        // Verify that some pages were dropped (overwritten)
        assert!(buffer.dropped_pages() > 0, "Expected at least some dropped pages");
    }

    #[test]
    fn test_out_of_order_completion_same_page() {
        let buffer =
            LocklessRingBuffer::new(3 * (*PAGE_SIZE) as usize, false, fuchsia_trace::Id::new())
                .unwrap();

        let header_size = LocklessRingBuffer::PAGE_HEADER_SIZE;
        let page_size = (*PAGE_SIZE) as usize;
        let available_space = page_size - header_size;

        let size1 = available_space / 2;
        let size2 = available_space - size1;

        // Reserve first chunk
        let (res1, _, _) = buffer.reserve(size1).unwrap();
        // Reserve second chunk (fills the page)
        let (res2, _, _) = buffer.reserve(size2).unwrap();

        // Commit second chunk first (out of order)
        buffer.commit(res2);

        // Page should not be finalized yet because res1 is still active
        assert!(buffer.swap_reader_page().is_none());

        // Commit first chunk
        buffer.commit(res1);

        // Force advance tail by making a reservation that won't fit on Page 0.
        let _ = buffer.reserve(10).unwrap();

        // Now page should be finalized
        let swapped = buffer.swap_reader_page();
        assert!(swapped.is_some());
        assert_eq!(swapped.unwrap(), 0); // First data page is node 0
    }

    #[test]
    fn test_concurrent_readers_rejection() {
        #[derive(Debug)]
        struct SleepingOutputBuffer {
            barrier_in_available: Arc<std::sync::Barrier>,
            barrier_test_done: Arc<std::sync::Barrier>,
        }
        impl Buffer for SleepingOutputBuffer {
            fn segments_count(&self) -> Result<usize, Errno> {
                Ok(1)
            }
            fn peek_each_segment(
                &mut self,
                _callback: &mut PeekBufferSegmentsCallback<'_>,
            ) -> Result<(), Errno> {
                Ok(())
            }
        }
        impl OutputBuffer for SleepingOutputBuffer {
            fn write_each(
                &mut self,
                _callback: &mut OutputBufferCallback<'_>,
            ) -> Result<usize, Errno> {
                Ok(0)
            }
            fn available(&self) -> usize {
                // Signal the main thread that we are active and holding the read lock, and block
                // until the main thread reaches its wait point.
                self.barrier_in_available.wait();

                // Block here until the main thread has completed its concurrent read assertion.
                self.barrier_test_done.wait();

                (*PAGE_SIZE) as usize
            }
            fn bytes_written(&self) -> usize {
                0
            }
            fn zero(&mut self) -> Result<usize, Errno> {
                Ok(0)
            }
            unsafe fn advance(&mut self, _length: usize) -> Result<(), Errno> {
                Ok(())
            }
        }

        let buffer = Arc::new(
            LocklessRingBuffer::new(3 * (*PAGE_SIZE) as usize, false, fuchsia_trace::Id::new())
                .unwrap(),
        );

        let barrier_in_available = Arc::new(std::sync::Barrier::new(2));
        let barrier_test_done = Arc::new(std::sync::Barrier::new(2));

        let buffer_clone = Arc::clone(&buffer);
        let barrier_in_available_clone = Arc::clone(&barrier_in_available);
        let barrier_test_done_clone = Arc::clone(&barrier_test_done);
        let handle = std::thread::spawn(move || {
            let mut dest = SleepingOutputBuffer {
                barrier_in_available: barrier_in_available_clone,
                barrier_test_done: barrier_test_done_clone,
            };
            let _ = buffer_clone.read(&mut dest);
        });

        // Wait until the background thread enters `available()` and is holding the read lock.
        barrier_in_available.wait();

        // Perform concurrent read while background thread holds `reader_active` lock.
        let mut dest = VecOutputBuffer::new((*PAGE_SIZE) as usize);
        let res = buffer.read(&mut dest);
        assert_eq!(res.unwrap_err(), starnix_uapi::errno!(EBUSY));

        // Signal the background thread that the assertion is done and it can proceed.
        barrier_test_done.wait();

        handle.join().unwrap();
    }

    #[test]
    fn test_stale_offset_livelock() {
        let num_threads = 8;
        let msgs_per_thread = 200;
        // Small 3-page buffer to force wraparounds and high contention
        let buffer = Arc::new(
            LocklessRingBuffer::new(3 * (*PAGE_SIZE) as usize, true, fuchsia_trace::Id::new())
                .unwrap(),
        );
        let mut handles = vec![];

        for thread_index in 0..num_threads {
            let buffer_clone = Arc::clone(&buffer);
            let handle = std::thread::spawn(move || {
                for _ in 0..msgs_per_thread {
                    if let Ok((res, now, delta)) = buffer_clone.reserve(TestMessage::SIZE) {
                        let msg = TestMessage {
                            thread_index,
                            timestamp_nanos: now.into_nanos() as u64,
                            delta: delta.into_nanos() as u64,
                            data: *b"Event data\0\0",
                        };
                        res.write_at(0, &msg.to_bytes());
                        buffer_clone.commit(res);
                    }
                    std::thread::yield_now();
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let dropped = buffer.dropped_pages();
        println!("High contention completed. Total dropped pages = {}", dropped);
        // Dropped pages should be reasonable and not spike to infinity or cause deadlock
        assert!(dropped > 0);
    }

    #[test]
    fn test_writer_preemption_and_overwrite_prevention() {
        let buffer = Arc::new(
            LocklessRingBuffer::new(3 * (*PAGE_SIZE) as usize, true, fuchsia_trace::Id::new())
                .unwrap(),
        );

        // 1. Thread 1 reserves space on Node 0, but delays committing (preempted)
        let res1 = buffer.reserve(100).unwrap();
        assert_eq!(res1.0.node_idx, 0);

        // 2. Thread 2 fills the rest of Node 0, Node 1, and wraps around to Node 0.
        // Thread 2 should block or spin since Node 0 has active writers (active_writers > 0)
        let buffer_clone = Arc::clone(&buffer);
        let writer_finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let writer_finished_clone = Arc::clone(&writer_finished);

        let handle = std::thread::spawn(move || {
            // Fill Node 0
            let res2 = buffer_clone
                .reserve((*PAGE_SIZE) as usize - LocklessRingBuffer::PAGE_HEADER_SIZE - 100)
                .unwrap();
            buffer_clone.commit(res2.0);

            // Fill Node 1
            let res3 = buffer_clone
                .reserve((*PAGE_SIZE) as usize - LocklessRingBuffer::PAGE_HEADER_SIZE)
                .unwrap();
            buffer_clone.commit(res3.0);

            // Try to reserve again - wraps around to Node 0 (which still has res1 active)
            // This should block/spin until we commit res1.
            let res4 = buffer_clone.reserve(100).unwrap();
            buffer_clone.commit(res4.0);

            writer_finished_clone.store(true, Ordering::Release);
        });

        // Give the thread some time to reach the block point
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(!writer_finished.load(Ordering::Acquire));

        // Now commit res1, which unblocks the wraparound writer
        buffer.commit(res1.0);

        handle.join().unwrap();
        assert!(writer_finished.load(Ordering::Acquire));
    }

    #[test]
    fn test_extreme_disable_enable_stress() {
        let num_threads = 8;
        let msgs_per_thread = 100;
        let buffer = Arc::new(
            LocklessRingBuffer::new(5 * (*PAGE_SIZE) as usize, true, fuchsia_trace::Id::new())
                .unwrap(),
        );
        let mut handles = vec![];

        // 1. Spawn 8 writer threads writing continuously until they successfully write msgs_per_thread.
        for thread_index in 0..num_threads {
            let buffer_clone = Arc::clone(&buffer);
            let handle = std::thread::spawn(move || {
                let mut count = 0;
                while count < msgs_per_thread {
                    match buffer_clone.reserve(TestMessage::SIZE) {
                        Ok((res, now, delta)) => {
                            let msg = TestMessage {
                                thread_index,
                                timestamp_nanos: now.into_nanos() as u64,
                                delta: delta.into_nanos() as u64,
                                data: *b"Event data\0\0",
                            };
                            res.write_at(0, &msg.to_bytes());
                            buffer_clone.commit(res);
                            count += 1;
                        }
                        Err(e) if e == starnix_uapi::errno!(ENOMEM) => {
                            // Expected when disabled. Yield and retry.
                            std::thread::yield_now();
                        }
                        Err(e) => panic!("Unexpected error during reserve: {:?}", e),
                    }
                }
                count
            });
            handles.push(handle);
        }

        // 2. Coordinator thread repeatedly disables and enables the ring buffer.
        let buffer_clone = Arc::clone(&buffer);
        let coordinator = std::thread::spawn(move || {
            for _ in 0..20 {
                std::thread::sleep(std::time::Duration::from_millis(5));
                // Disable the ring buffer. This must successfully wait for all active writers.
                let _dropped = buffer_clone.disable().unwrap();

                // Ensure subsequent reservations fail while disabled.
                let res = buffer_clone.reserve(TestMessage::SIZE);
                assert_eq!(res.unwrap_err(), starnix_uapi::errno!(ENOMEM));

                std::thread::sleep(std::time::Duration::from_millis(2));
                // Re-enable the ring buffer.
                let _now = buffer_clone.enable().unwrap();
            }
        });

        coordinator.join().unwrap();
        let mut total_writes = 0;
        for handle in handles {
            total_writes += handle.join().unwrap();
        }
        println!("Disable/enable stress completed. Total writes = {}", total_writes);
        assert_eq!(total_writes, num_threads * msgs_per_thread);
    }

    #[test]
    fn test_reader_loop_swapping_high_contention() {
        let num_threads = 8usize;
        let msgs_per_thread = 100;
        // 8 threads * 100 msgs * 32 bytes = 25600 bytes.
        // Each page holds 127 messages.
        // Let's make a 10-page buffer to have ample space.
        let buffer = Arc::new(
            LocklessRingBuffer::new(10 * (*PAGE_SIZE) as usize, true, fuchsia_trace::Id::new())
                .unwrap(),
        );
        let writers_done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut handles = vec![];

        // Spawn unified reader thread that loops read() draining all pages.
        let buffer_reader = Arc::clone(&buffer);
        let writers_done_reader = Arc::clone(&writers_done);
        let reader_handle = start_reader_thread(buffer_reader, writers_done_reader);

        // Spawn 8 writer threads.
        for thread_index in 0..num_threads {
            let buffer_clone = Arc::clone(&buffer);
            let handle = std::thread::spawn(move || {
                for _ in 0..msgs_per_thread {
                    let (res, now, delta) = buffer_clone.reserve(TestMessage::SIZE).unwrap();
                    let msg = TestMessage {
                        thread_index: thread_index as u32,
                        timestamp_nanos: now.into_nanos() as u64,
                        delta: delta.into_nanos() as u64,
                        data: *b"Event data\0\0",
                    };
                    res.write_at(0, &msg.to_bytes());
                    buffer_clone.commit(res);
                    // Yield to cause maximum interleaving
                    std::thread::yield_now();
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().unwrap();
        }
        writers_done.store(true, Ordering::Release);
        let all_messages = reader_handle.join().unwrap();
        let messages_read = all_messages.len();
        check_all_message_data(&all_messages, num_threads as u32);
        let dropped = buffer.dropped_pages();
        println!(
            "Reader high contention completed. Messages read = {}, dropped pages = {}",
            messages_read, dropped
        );
        // Verify that the total number of read messages + dropped pages' messages covers all writes.
        let messages_per_page =
            ((*PAGE_SIZE) as usize - LocklessRingBuffer::PAGE_HEADER_SIZE) / TestMessage::SIZE;
        let total_written = num_threads * msgs_per_thread;
        let total_accounted = messages_read + (dropped as usize * messages_per_page);
        assert!(total_accounted >= total_written - messages_per_page);
    }

    #[test]
    fn test_failed_reservation_offset_boundary() {
        let buffer =
            LocklessRingBuffer::new(3 * (*PAGE_SIZE) as usize, true, fuchsia_trace::Id::new())
                .unwrap();
        let page_size = (*PAGE_SIZE) as usize;
        let max_payload = page_size - LocklessRingBuffer::PAGE_HEADER_SIZE;

        // Reserve enough space to almost fill Node 0
        let msg_size = 100;
        let num_msgs = max_payload / msg_size;
        for _ in 0..num_msgs {
            let (res, _, _) = buffer.reserve(msg_size).unwrap();
            buffer.commit(res);
        }

        // Check current write_offset of Node 0
        let expected_offset = LocklessRingBuffer::PAGE_HEADER_SIZE + num_msgs * msg_size;
        assert_eq!(buffer.nodes[0].write_offset.load(Ordering::Acquire), expected_offset);

        // Now try a reservation that exceeds remaining space on Node 0.
        let (res, _, _) = buffer.reserve(msg_size).unwrap();
        assert_eq!(res.node_idx, 1);
        buffer.commit(res);

        // Check write_offset of Node 0.
        // Before CAS fix, write_offset was 4116 (> page_size).
        // After CAS fix, write_offset is exactly expected_offset (4016).
        assert_eq!(buffer.nodes[0].write_offset.load(Ordering::Acquire), expected_offset);
    }
}
