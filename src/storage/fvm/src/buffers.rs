// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module manages staging buffers used by FVM and zxcrypt.
//!
//! It provides `BufferPool` which manages a memory-mapped VMO split into fixed-size buffers,
//! and `BufferGuard` which provides exclusive safe access to a single buffer.
//!
//! To minimize resting memory usage, the pool tracks the high watermark of concurrently
//! active buffers and decommits unused buffers when the pool is idle.

use event_listener::Event;
use fuchsia_async as fasync;
use fuchsia_sync::Mutex;
use std::sync::{Arc, Weak};
use storage_ptr_slice::{MutPtrByteSlice, PtrByteSlice};

pub const BUFFER_SIZE: usize = 1048576;
pub const BUFFER_COUNT: usize = 16;
pub const TOTAL_SIZE: usize = BUFFER_SIZE * BUFFER_COUNT;

struct PoolState {
    /// Offsets of available buffers in the VMO.
    buffers: Vec<usize>,
    /// High watermark of concurrently allocated buffers during the current reaping interval.
    high_watermark: usize,
    /// Handle to the active idle decommit task. Dropping this cancels the timer.
    reaper_task: Option<fasync::Task<()>>,
}

/// A pool of fixed-size staging buffers backed by a mapped VMO.
///
/// For shared pools (FVM-device), the VMO is registered with the block device and has a valid
/// `VmoId`. For private pools (zxcrypt staging), the VMO is local to FVM and uses `VMOID_INVALID`.
pub struct BufferPool {
    /// The underlying VMO.
    vmo: zx::Vmo,
    /// The base address of the mapped VMO.
    addr: usize,
    /// Event used to notify waiters when a buffer is returned to the pool.
    event: Event,
    /// Mutable state of the pool.
    state: Mutex<PoolState>,
}

impl BufferPool {
    /// Creates a new `BufferPool` by mapping the provided VMO.
    pub fn new(vmo: zx::Vmo) -> Result<Self, zx::Status> {
        let addr = fuchsia_runtime::vmar_root_self().map(
            0,
            &vmo,
            0,
            TOTAL_SIZE,
            zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE,
        )?;
        Ok(Self {
            vmo,
            addr,
            event: Event::new(),
            state: Mutex::new(PoolState {
                buffers: (0..TOTAL_SIZE).into_iter().step_by(BUFFER_SIZE).collect(),
                high_watermark: 0,
                reaper_task: None,
            }),
        })
    }

    /// Allocates a buffer from the pool, blocking if no buffers are available.
    ///
    /// Watermark tracking:
    /// - Always keeps at least 1 buffer committed.
    /// - Spawns a lazy 10-second timer when active buffers transition from 1 to 2.
    pub async fn get_buffer(self: &Arc<Self>) -> BufferGuard {
        loop {
            let listener = {
                let mut state = self.state.lock();
                if let Some(offset) = state.buffers.pop() {
                    let in_use = BUFFER_COUNT - state.buffers.len();
                    state.high_watermark = std::cmp::max(state.high_watermark, in_use);

                    // Start the reaper timer if we cross the threshold of 1 in-use buffer.
                    if in_use == 2 && state.reaper_task.is_none() {
                        let this = Arc::downgrade(self);
                        state.reaper_task = Some(fasync::Task::spawn(async move {
                            Self::run_reaper(this).await;
                        }));
                    }

                    let ptr = (self.addr + offset) as *mut u8;
                    return BufferGuard { pool: self.clone(), offset, ptr };
                }
                self.event.listen()
            };
            listener.await
        }
    }

    /// Background task that runs periodically to reap unused committed pages.
    async fn run_reaper(this: Weak<Self>) {
        loop {
            fasync::Timer::new(fasync::MonotonicInstant::after(zx::Duration::from_seconds(10)))
                .await;
            let Some(pool) = this.upgrade() else {
                break;
            };

            if !pool.reap() {
                break;
            }
        }
    }

    /// Reaps unused buffers based on the high watermark observed during the interval.
    ///
    /// Returns `true` if the timer should keep running, or `false` if it should stop
    /// because usage has returned to baseline (<= 1 buffer).
    fn reap(&self) -> bool {
        let mut state = self.state.lock();
        // Always keep at least 1 buffer committed.
        let target = std::cmp::max(state.high_watermark, 1);

        // Decommit buffers from the free list that exceed the target watermark.
        let to_decommit = std::cmp::min(state.buffers.len(), BUFFER_COUNT - target);

        for &offset in &state.buffers[..to_decommit] {
            // SAFETY: The VMO is owned by the pool and remains valid.
            let result = self.vmo.op_range(zx::VmoOp::DECOMMIT, offset as u64, BUFFER_SIZE as u64);

            if let Err(error) = result {
                log::warn!(error:?; "Failed to decommit buffer");
            }
        }

        // Reset watermark to current in-use for the next interval.
        let in_use = BUFFER_COUNT - state.buffers.len();
        state.high_watermark = in_use;

        // Stop the timer if we are back to baseline usage.
        if target == 1 && state.high_watermark <= 1 {
            state.reaper_task = None;
            false
        } else {
            true
        }
    }
}

impl Drop for BufferPool {
    fn drop(&mut self) {
        // SAFETY: We mapped this address in `new`, and no references or pointers into the mapping
        // (i.e. those from BufferGuard) remain at this point.
        let result = unsafe { fuchsia_runtime::vmar_root_self().unmap(self.addr, TOTAL_SIZE) };

        let _ = result;
    }
}

/// A guard providing exclusive safe access to a staging buffer.
pub struct BufferGuard {
    pool: Arc<BufferPool>,
    offset: usize,
    ptr: *mut u8,
}

impl BufferGuard {
    /// Returns the offset of this buffer in the VMO.
    pub fn vmo_offset(&self) -> u64 {
        self.offset as u64
    }

    /// Returns a read-only slice wrapper.
    pub fn as_ptr_slice(&self) -> PtrByteSlice<'_> {
        // SAFETY: The pointer is valid because the pool is kept alive by `BufferGuard`'s Arc.
        // Construction of `PtrByteSlice` is safe because we only allow read-only access.
        let slice =
            unsafe { PtrByteSlice::new(std::ptr::slice_from_raw_parts(self.ptr, BUFFER_SIZE)) };

        slice
    }

    /// Returns a mutable slice wrapper.
    pub fn as_mut_ptr_slice(&mut self) -> MutPtrByteSlice<'_> {
        // SAFETY: Construction of `MutPtrByteSlice` is safe because we have exclusive access
        // (ensured by `&mut self` and the pool's allocator).
        let slice = unsafe {
            MutPtrByteSlice::new(std::ptr::slice_from_raw_parts_mut(self.ptr, BUFFER_SIZE))
        };

        slice
    }
}

impl Drop for BufferGuard {
    fn drop(&mut self) {
        {
            let mut state = self.pool.state.lock();
            state.buffers.push(self.offset);
        }
        self.pool.event.notify_additional_relaxed(1);
    }
}

// SAFETY: The only raw pointer is `ptr` which points to thread-safe mapped memory
// (synchronized via `state` lock).
unsafe impl Send for BufferGuard {}
unsafe impl Sync for BufferGuard {}

#[cfg(test)]
mod tests {
    use super::{BUFFER_COUNT, BUFFER_SIZE, BufferPool, TOTAL_SIZE};
    use fuchsia_async::{self as fasync, TestExecutor};
    use std::sync::Arc;
    use std::task::Poll;

    #[test]
    fn test_get_buffer() {
        let mut exec = TestExecutor::new();
        let vmo = zx::Vmo::create(TOTAL_SIZE as u64).unwrap();
        let pool = Arc::new(BufferPool::new(vmo).unwrap());
        let mut bufs = Vec::new();
        let mut offsets = Vec::new();

        fn check_no_overlap(offsets: &[u64], offset: u64) {
            for &o in offsets {
                assert!(offset >= o + BUFFER_SIZE as u64 || offset + BUFFER_SIZE as u64 <= o);
            }
        }

        for i in 0..BUFFER_COUNT {
            let mut fut = Box::pin(pool.get_buffer());
            let Poll::Ready(mut buf) = exec.run_until_stalled(&mut fut) else {
                panic!("Expected ready");
            };
            check_no_overlap(&offsets, buf.vmo_offset());
            offsets.push(buf.vmo_offset());
            buf.as_mut_ptr_slice().fill(i as u8);
            bufs.push(buf);
        }

        // Check that all the buffers contain the expected fill.
        for (i, buf) in bufs.iter().enumerate() {
            assert_eq!(buf.as_ptr_slice().to_vec(), vec![i as u8; BUFFER_SIZE]);
        }

        // The next buffer we get should stall.
        let mut pending_buf = Box::pin(pool.get_buffer());
        assert!(exec.run_until_stalled(&mut pending_buf).is_pending());

        // Drop one buffer, and it should unblock the pending buffer.
        bufs.pop();
        offsets.pop();

        assert!(exec.run_until_stalled(&mut pending_buf).is_ready());
    }

    #[test]
    fn test_watermark_decommit() {
        let mut exec = fasync::TestExecutorBuilder::new().fake_time(true).build();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));

        let vmo = zx::Vmo::create(TOTAL_SIZE as u64).unwrap();
        let vmo_clone = vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
        let pool = Arc::new(BufferPool::new(vmo).unwrap());

        let initial_committed = vmo_clone.info().unwrap().committed_bytes;
        assert_eq!(initial_committed, 0);

        // 1. Allocate buf1 and write to it. Watermark is 1.
        let mut fut1 = Box::pin(pool.get_buffer());
        let Poll::Ready(mut buf1) = exec.run_until_stalled(&mut fut1) else {
            panic!("Expected ready");
        };
        buf1.as_mut_ptr_slice().fill(1);
        let committed_1 = vmo_clone.info().unwrap().committed_bytes;
        assert_eq!(committed_1, BUFFER_SIZE as u64);

        // 2. Allocate buf2 and write to it. Watermark goes to 2.
        let mut fut2 = Box::pin(pool.get_buffer());
        let Poll::Ready(mut buf2) = exec.run_until_stalled(&mut fut2) else {
            panic!("Expected ready");
        };
        buf2.as_mut_ptr_slice().fill(2);
        let committed_2 = vmo_clone.info().unwrap().committed_bytes;
        assert_eq!(committed_2, 2 * BUFFER_SIZE as u64);

        // Timer should have been spawned.
        {
            let state = pool.state.lock();
            assert!(state.reaper_task.is_some());
        }

        // 3. Release buf2. Watermark is still 2 for this interval.
        std::mem::drop(buf2);
        assert_eq!(vmo_clone.info().unwrap().committed_bytes, 2 * BUFFER_SIZE as u64);

        // 4. Advance time by 10 seconds to trigger first reap.
        let mut advance_fut1 = Box::pin(fasync::TestExecutor::advance_to(
            fasync::MonotonicInstant::after(zx::Duration::from_seconds(10)),
        ));
        let Poll::Ready(()) = exec.run_until_stalled(&mut advance_fut1) else {
            panic!("Expected advance_to to complete");
        };
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

        // First reap: watermark was 2, so it should keep 2 committed.
        assert_eq!(vmo_clone.info().unwrap().committed_bytes, 2 * BUFFER_SIZE as u64);
        {
            let state = pool.state.lock();
            assert!(state.reaper_task.is_some());
            assert_eq!(state.high_watermark, 1); // Reset to current in-use (buf1 is still alive)
        }

        // 5. Advance time by another 10 seconds to trigger second reap.
        let mut advance_fut2 = Box::pin(fasync::TestExecutor::advance_to(
            fasync::MonotonicInstant::after(zx::Duration::from_seconds(10)),
        ));
        let Poll::Ready(()) = exec.run_until_stalled(&mut advance_fut2) else {
            panic!("Expected advance_to to complete");
        };
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

        // Second reap: watermark was 1, so it should decommit down to 1.
        assert_eq!(vmo_clone.info().unwrap().committed_bytes, BUFFER_SIZE as u64);
        {
            let state = pool.state.lock();
            assert!(state.reaper_task.is_none());
        }
    }
}
