// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::buffer::{BufferAllocator, OwnedBuffer};
use anyhow::Error;
use fuchsia_sync::Mutex;
use std::fmt::Debug;
use std::ops::Range;
use std::ptr::slice_from_raw_parts_mut;
use std::sync::Arc;
use storage_ptr_slice::MutPtrByteSlice;

type CompletionCallback = Box<dyn FnOnce(Result<OwnedBuffer, Error>) + Send>;

enum State {
    /// Initial state while `split`'s closure `f` is executing. If an error is reported before
    /// `f` finishes, `failed` records that error.
    Pending { failed: Option<Error> },
    /// `f` succeeded; all sub-operations are in flight.
    Dispatched { parent_buffer: OwnedBuffer, callback: CompletionCallback },
    /// An error occurred or the callback has already been invoked.
    Completed,
}

struct SplittableBufferInner {
    state: Mutex<State>,
    is_trusted: bool,
}

impl Debug for SplittableBufferInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SplittableBufferInner").finish_non_exhaustive()
    }
}

impl BufferAllocator for SplittableBufferInner {
    fn free_buffer(&self, _range: Range<usize>) {
        // No-op: Dropping the child `OwnedBuffer` drops its `Arc<dyn BufferAllocator>`,
        // which automatically decrements the `Arc` reference count of `SplittableBufferInner`.
    }

    fn is_trusted(&self) -> bool {
        self.is_trusted
    }
}

impl Drop for SplittableBufferInner {
    fn drop(&mut self) {
        if let State::Dispatched { parent_buffer, callback } =
            std::mem::replace(self.state.get_mut(), State::Completed)
        {
            (callback)(Ok(parent_buffer));
        }
    }
}

/// A handle for an individual in-flight sub-read operation.
///
/// Must be consumed via [`merge`] on completion. If dropped before `merge` is called, it
/// automatically fails the parent `SplittableBuffer` operation.
#[derive(Debug)]
pub struct SubHandle {
    inner: Arc<SplittableBufferInner>,
    completed: bool,
}

impl SubHandle {
    /// Invoked in sub-read callbacks. If `f()` returns `Err(e)`, disarms the callback and delivers
    /// `Err(e)` immediately.
    pub fn merge(mut self, f: impl FnOnce() -> Result<(), Error>) {
        self.completed = true;
        if let Err(e) = f() {
            let mut guard = self.inner.state.lock();
            match &mut *guard {
                State::Pending { failed } => {
                    failed.get_or_insert(e);
                }
                State::Dispatched { .. } => {
                    if let State::Dispatched { callback, .. } =
                        std::mem::replace(&mut *guard, State::Completed)
                    {
                        (callback)(Err(e));
                    }
                }
                State::Completed => {}
            }
        }
    }
}

impl Drop for SubHandle {
    fn drop(&mut self) {
        if !self.completed {
            let mut guard = self.inner.state.lock();
            match &mut *guard {
                State::Pending { failed } => {
                    failed.get_or_insert_with(|| {
                        anyhow::anyhow!("Read sub-request dropped before completion")
                    });
                }
                State::Dispatched { .. } => {
                    if let State::Dispatched { callback, .. } =
                        std::mem::replace(&mut *guard, State::Completed)
                    {
                        (callback)(Err(anyhow::anyhow!(
                            "Read sub-request dropped before completion"
                        )));
                    }
                }
                State::Completed => {}
            }
        }
    }
}

/// A wrapper around `OwnedBuffer` that allows carving out independent child `OwnedBuffer`s
/// and delivering the reconstructed original `OwnedBuffer` to a completion callback once all child
/// buffers have been dropped.
#[derive(Debug)]
pub struct SplittableBuffer {
    inner: Arc<SplittableBufferInner>,
    current_ptr: *mut u8,
    remaining_range: Range<usize>,
}

// SAFETY: `current_ptr` points into `inner.parent_buffer`'s VMO / memory region, which can be
// sent across threads.
unsafe impl Send for SplittableBuffer {}
unsafe impl Sync for SplittableBuffer {}

impl SplittableBuffer {
    /// Returns the remaining unallocated range available for splitting.
    pub fn remaining_range(&self) -> Range<usize> {
        self.remaining_range.clone()
    }

    /// Zeroes the next `len` bytes in the buffer and advances the offset.
    ///
    /// # Panics
    ///
    /// Panics if `len` exceeds `remaining_range.len()`.
    pub fn fill_zeros(&mut self, len: usize) {
        assert!(len <= self.remaining_range.len());
        self.remaining_range.start += len;
        let ptr = self.current_ptr;
        self.current_ptr = self.current_ptr.wrapping_add(len);

        // SAFETY: `ptr` points into `inner.parent_buffer`'s allocated memory of at least
        // `len` bytes.
        unsafe {
            std::ptr::write_bytes(ptr, 0, len);
        }
    }

    /// Carves out the first `len` bytes of the remaining unsplit buffer as an `OwnedBuffer`
    /// along with a [`SubHandle`] to track its asynchronous completion.
    ///
    /// # Panics
    ///
    /// Panics if `len` exceeds `remaining_range.len()`.
    pub fn take_prefix(&mut self, len: usize) -> (OwnedBuffer, SubHandle) {
        assert!(len <= self.remaining_range.len());
        let child_range = self.remaining_range.start..self.remaining_range.start + len;
        self.remaining_range.start += len;
        let ptr = self.current_ptr;
        self.current_ptr = self.current_ptr.wrapping_add(len);

        // SAFETY: `child_range` is strictly within the original parent buffer bounds and
        // never overlaps with any other prefix taken from `remaining_range`. The
        // `Arc<SplittableBufferInner>` keeps the parent `OwnedBuffer` alive for `'static`.
        let slice = unsafe { MutPtrByteSlice::new(slice_from_raw_parts_mut(ptr, len)) };
        let buffer =
            OwnedBuffer::new(slice, child_range, self.inner.clone() as Arc<dyn BufferAllocator>);
        let sub_handle = SubHandle { inner: self.inner.clone(), completed: false };
        (buffer, sub_handle)
    }
}

impl OwnedBuffer {
    /// Splits the buffer into one or more child `OwnedBuffer`s via `f`.
    ///
    /// When all child `OwnedBuffer`s and [`SubHandle`]s drop, the merged parent buffer is
    /// automatically delivered to `on_complete(Ok(parent_buffer))`. If an error occurs via `f`
    /// or during sub-operation execution, `on_complete(Err(e))` is delivered.
    pub fn split<R>(
        mut self,
        f: impl FnOnce(&mut SplittableBuffer) -> Result<R, Error>,
        on_complete: impl FnOnce(Result<OwnedBuffer, Error>) + Send + 'static,
    ) -> Result<R, Error> {
        let is_trusted = self.try_as_slice().is_some();
        let remaining_range = self.range();
        let current_ptr = self.as_mut_ptr();
        let inner = Arc::new(SplittableBufferInner {
            state: Mutex::new(State::Pending { failed: None }),
            is_trusted,
        });
        let mut splittable =
            SplittableBuffer { inner: inner.clone(), current_ptr, remaining_range };
        let res = f(&mut splittable);
        match res {
            Ok(val) => {
                let mut guard = inner.state.lock();
                match std::mem::replace(&mut *guard, State::Completed) {
                    State::Pending { failed: Some(e) } => {
                        on_complete(Err(e));
                    }
                    State::Pending { failed: None } => {
                        *guard = State::Dispatched {
                            parent_buffer: self,
                            callback: Box::new(on_complete),
                        };
                    }
                    State::Dispatched { .. } | State::Completed => {}
                }
                Ok(val)
            }
            Err(e) => {
                *inner.state.lock() = State::Completed;
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer_allocator::{BufferAllocator as PoolBufferAllocator, BufferSource};
    use anyhow::anyhow;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[fuchsia::test]
    async fn test_splittable_buffer_with_callback_success() {
        let source = BufferSource::new(4096);
        let pool = Arc::new(PoolBufferAllocator::new(512, source));
        let owned = pool.allocate_buffer_sync_owned(2048);

        let completed = Arc::new(AtomicBool::new(false));
        let completed_clone = completed.clone();

        let mut sub1_opt = None;
        let mut sub2_opt = None;
        let mut child1_opt = None;
        let mut child2_opt = None;

        owned
            .split(
                |splittable| {
                    let (mut child1, sub1) = splittable.take_prefix(1024);
                    let (mut child2, sub2) = splittable.take_prefix(1024);
                    child1.as_mut_ptr_slice().fill(0x11);
                    child2.as_mut_ptr_slice().fill(0x22);
                    sub1_opt = Some(sub1);
                    sub2_opt = Some(sub2);
                    child1_opt = Some(child1);
                    child2_opt = Some(child2);
                    Ok(())
                },
                move |res| {
                    let merged = res.expect("must succeed");
                    assert_eq!(merged.len(), 2048);
                    assert!(
                        merged.as_ptr_slice().subslice(0..1024).iter_as::<u8>().all(|b| b == 0x11)
                    );
                    assert!(
                        merged
                            .as_ptr_slice()
                            .subslice(1024..2048)
                            .iter_as::<u8>()
                            .all(|b| b == 0x22)
                    );
                    completed_clone.store(true, Ordering::Relaxed);
                },
            )
            .unwrap();

        let sub1 = sub1_opt.unwrap();
        let sub2 = sub2_opt.unwrap();
        let child1 = child1_opt.unwrap();
        let child2 = child2_opt.unwrap();

        sub1.merge(|| Ok(()));
        drop(child1);
        assert!(!completed.load(Ordering::Relaxed));

        // When child2 drops and sub2 is merged, all references are gone and callback is fired.
        sub2.merge(|| Ok(()));
        drop(child2);
        assert!(completed.load(Ordering::Relaxed));
    }

    #[fuchsia::test]
    async fn test_splittable_buffer_with_callback_async_error() {
        let source = BufferSource::new(4096);
        let pool = Arc::new(PoolBufferAllocator::new(512, source));
        let owned = pool.allocate_buffer_sync_owned(2048);

        let err_received = Arc::new(AtomicBool::new(false));
        let err_clone = err_received.clone();

        let mut sub1_opt = None;
        let mut sub2_opt = None;
        let mut child1_opt = None;
        let mut child2_opt = None;

        owned
            .split(
                |splittable| {
                    let (child1, sub1) = splittable.take_prefix(1024);
                    let (child2, sub2) = splittable.take_prefix(1024);
                    sub1_opt = Some(sub1);
                    sub2_opt = Some(sub2);
                    child1_opt = Some(child1);
                    child2_opt = Some(child2);
                    Ok(())
                },
                move |res| {
                    assert!(res.is_err());
                    err_clone.store(true, Ordering::Relaxed);
                },
            )
            .unwrap();

        let sub1 = sub1_opt.unwrap();
        let sub2 = sub2_opt.unwrap();
        let child1 = child1_opt.unwrap();
        let child2 = child2_opt.unwrap();

        // Merge error from chunk 1:
        sub1.merge(|| Err(anyhow!("chunk 1 failure")));
        assert!(err_received.load(Ordering::Relaxed));

        // Chunk 2 drops later without error, but callback was already consumed:
        drop(child1);
        drop(child2);
        sub2.merge(|| Ok(()));
    }

    #[fuchsia::test]
    async fn test_splittable_buffer_sub_handle_dropped_fails_operation() {
        let source = BufferSource::new(4096);
        let pool = Arc::new(PoolBufferAllocator::new(512, source));
        let owned = pool.allocate_buffer_sync_owned(2048);

        let err_received = Arc::new(AtomicBool::new(false));
        let err_clone = err_received.clone();

        let mut sub1 = None;
        let mut child1 = None;
        owned
            .split(
                |splittable| {
                    let (c, s) = splittable.take_prefix(1024);
                    child1 = Some(c);
                    sub1 = Some(s);
                    Ok(())
                },
                move |res| {
                    assert!(res.is_err());
                    err_clone.store(true, Ordering::Relaxed);
                },
            )
            .unwrap();

        drop(child1);

        // sub1 is dropped without calling merge:
        drop(sub1);
        assert!(err_received.load(Ordering::Relaxed));
    }

    #[fuchsia::test]
    async fn test_splittable_buffer_sync_error() {
        let source = BufferSource::new(4096);
        let pool = Arc::new(PoolBufferAllocator::new(512, source));
        let owned = pool.allocate_buffer_sync_owned(2048);

        let callback_called = Arc::new(AtomicBool::new(false));
        let callback_clone = callback_called.clone();

        let mut sub1 = None;
        let mut child1 = None;
        let res: Result<(), _> = owned.split(
            |splittable| {
                let (c, s) = splittable.take_prefix(1024);
                child1 = Some(c);
                sub1 = Some(s);
                Err(anyhow!("synchronous dispatch failure"))
            },
            move |_res| {
                callback_clone.store(true, Ordering::Relaxed);
            },
        );

        assert!(res.is_err());
        drop(child1);
        drop(sub1);
        assert!(!callback_called.load(Ordering::Relaxed));
    }

    #[fuchsia::test]
    async fn test_splittable_buffer_fill_zeros() {
        let source = BufferSource::new(4096);
        let pool = Arc::new(PoolBufferAllocator::new(512, source));
        let owned = pool.allocate_buffer_sync_owned(2048);

        owned
            .split(
                |splittable| {
                    splittable.fill_zeros(2048);
                    Ok(())
                },
                |res| {
                    let buffer = res.unwrap();
                    assert_eq!(buffer.len(), 2048);
                    assert!(buffer.as_ptr_slice().iter_as::<u8>().all(|b| b == 0));
                },
            )
            .unwrap();
    }
}
