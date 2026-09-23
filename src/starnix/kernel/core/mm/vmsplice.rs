// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mm::MemoryManager;
use crate::mm::memory::MemoryObject;
use crate::vfs::buffers::{InputBuffer, MessageData, OutputBuffer};
use crate::vfs::with_iovec_segments;

use smallvec::SmallVec;
use starnix_sync::{InflightVmsplicedPayloadsLock, LockDepMutex, VmspliceSegmentsLock};
use starnix_uapi::errors::Errno;
use starnix_uapi::range_ext::RangeExt as _;
use starnix_uapi::user_address::UserAddress;
use starnix_uapi::{errno, error};
use std::mem::MaybeUninit;
use std::ops::Range;
use std::sync::{Arc, Weak};

/// A single segment of a `VmsplicePayload`.
#[derive(Clone, Debug)]
pub struct VmsplicePayloadSegment {
    pub addr_offset: UserAddress,
    pub length: usize,
    /// The `MemoryObject` that contains the memory used in this mapping.
    pub memory: Arc<MemoryObject>,
    /// The offset in the `MemoryObject` that corresponds to the base address.
    pub memory_offset: u64,
    /// Whether this segment should be snapshotted on unmap.
    pub should_snapshot_on_unmap: bool,
}

impl VmsplicePayloadSegment {
    fn split_off(&mut self, index: usize) -> Option<Self> {
        if index >= self.length {
            return None;
        }

        let mut mapping = self.clone();
        mapping.length = self.length - index;
        mapping.addr_offset = match mapping.addr_offset + index {
            Ok(new_addr) => new_addr,
            Err(_) => return None,
        };
        mapping.memory_offset += index as u64;

        self.length = index;
        Some(mapping)
    }

    fn truncate(&mut self, limit: usize) {
        // TODO(https://fxbug.dev/335701084): Truncating like this may leave
        // unreachable memory in the VMO that is free to reclaim. We should
        // reclaim the truncated memory if we can guarantee that it is no
        // longer reachable by other means (e.g. other mappings, files, shared
        // memory, etc.).
        self.length = std::cmp::min(self.length, limit);
    }

    fn read_uninit(&self, data: &mut [MaybeUninit<u8>]) -> Result<(), zx::Status> {
        self.memory.read_uninit(data, self.memory_offset)?;
        Ok(())
    }

    /// Reads from the backing memory.
    ///
    /// # Safety
    ///
    /// Callers must guarantee that the buffer is valid to write to.
    unsafe fn raw_read(&self, buffer: *mut u8, buffer_length: usize) -> Result<(), zx::Status> {
        #[allow(clippy::undocumented_unsafe_blocks, reason = "2024 edition migration")]
        unsafe {
            self.memory.read_raw(buffer, buffer_length, self.memory_offset)
        }
    }
}

/// A single payload that may sit in a pipe as a consequence of a `vmsplice(2)`
/// to a pipe.
///
/// A `VmsplicePayload` originally starts with a single segment. The payload
/// may be split up into multiple segments as the payload sits in the pipe.
/// This can happen when a mapping that is also backing a vmsplice-ed payload
/// is modified such that the original segment is partially unmapped.
///
/// When the `VmsplicePayload` is created, it will be appended to its associated
/// memory manager's [`InflightVmsplicedPayloads`]. The list cleans itself when
/// `handle_unmapping` is run.
#[derive(Debug, Default)]
pub struct VmsplicePayload {
    mapping: Weak<MemoryManager>,
    segments: LockDepMutex<SmallVec<[VmsplicePayloadSegment; 1]>, VmspliceSegmentsLock>,
}

impl VmsplicePayload {
    pub fn new(mapping: Weak<MemoryManager>, segment: VmsplicePayloadSegment) -> Arc<Self> {
        Self::new_with_segments(mapping, [segment].into())
    }

    fn new_with_segments(
        mapping: Weak<MemoryManager>,
        segments: SmallVec<[VmsplicePayloadSegment; 1]>,
    ) -> Arc<Self> {
        let mapping_strong = mapping.upgrade();
        let payload = Arc::new(Self { mapping, segments: segments.into() });
        if let Some(mapping) = mapping_strong {
            mapping.inflight_vmspliced_payloads.handle_new_payload(&payload);
        }
        payload
    }
}

impl MessageData for Arc<VmsplicePayload> {
    fn copy_from_user(_data: &mut dyn InputBuffer, _limit: usize) -> Result<Self, Errno> {
        error!(ENOTSUP)
    }

    fn ptr(&self) -> Result<*const u8, Errno> {
        error!(ENOTSUP)
    }

    fn with_bytes<O, F: FnMut(&[u8]) -> Result<O, Errno>>(&self, mut f: F) -> Result<O, Errno> {
        let v = {
            let segments = self.segments.lock();
            let mut v = Vec::with_capacity(segments.iter().map(|s| s.length).sum());
            for segment in segments.iter() {
                segment
                    .read_uninit(&mut v.spare_capacity_mut()[..segment.length])
                    .map_err(|_| errno!(EFAULT))?;
                // SAFETY: The read above succeeded.
                unsafe { v.set_len(v.len() + segment.length) }
            }
            v
        };
        // Don't hold the lock because the callback may perform work which
        // requires taking memory manager or mapping locks. Note that
        // VmsplicePayload is modified while such locks are held (e.g. unmap).
        f(&v)
    }

    fn len(&self) -> usize {
        self.segments.lock().iter().map(|s| s.length).sum()
    }

    fn split_off(&mut self, mut limit: usize) -> Option<Self> {
        let new_segments = {
            let mut segments = self.segments.lock();

            let mut split_at = 0;
            for segment in segments.iter() {
                if limit >= segment.length {
                    limit -= segment.length;
                    split_at += 1;
                } else {
                    break;
                }
            }

            let mut new_segments = SmallVec::new();
            if limit != 0 && split_at < segments.len() {
                new_segments.push(segments[split_at].split_off(limit).unwrap());
                split_at += 1;
            };
            if split_at <= segments.len() {
                new_segments.extend(segments.drain(split_at..));
            }
            new_segments
        };

        if new_segments.is_empty() {
            None
        } else {
            Some(VmsplicePayload::new_with_segments(self.mapping.clone(), new_segments))
        }
    }

    fn truncate(&mut self, mut limit: usize) {
        let mut segments = self.segments.lock();

        segments.retain_mut(|segment| {
            if limit >= segment.length {
                limit -= segment.length;
                true
            } else if limit != 0 {
                segment.truncate(limit);
                limit = 0;
                true
            } else {
                false
            }
        })
    }

    fn clone_at_most(&self, limit: usize) -> Self {
        let mut payload =
            VmsplicePayload::new_with_segments(self.mapping.clone(), self.segments.lock().clone());
        payload.truncate(limit);
        payload
    }

    fn copy_to_user(&self, data: &mut dyn OutputBuffer) -> Result<usize, Errno> {
        let result = with_iovec_segments(data, |iovecs: &mut [syncio::zxio::zx_iovec]| {
            let segments = self.segments.lock();
            let length: usize = segments.iter().map(|s| s.length).sum();

            let mut segments = segments.iter();
            let mut current_segment = segments.next().cloned();
            let mut copied = 0;

            for iovec in iovecs {
                if length == copied {
                    break;
                }

                iovec.capacity = std::cmp::min(iovec.capacity, length - copied);

                let mut iovec_pos = 0;
                while let Some(segment) = &mut current_segment {
                    if iovec_pos == iovec.capacity {
                        break;
                    }

                    let to_read = std::cmp::min(segment.length, iovec.capacity - iovec_pos);
                    let after_read = segment.split_off(to_read);
                    // SAFETY: `iovec.buffer` has at least `iovec.capacity` bytes, and
                    // `iovec_pos + to_read <= iovec.capacity`.
                    unsafe { segment.raw_read((iovec.buffer as *mut u8).add(iovec_pos), to_read) }
                        .map_err(|_| errno!(EFAULT))?;
                    copied += to_read;
                    iovec_pos += to_read;

                    if let Some(after_read) = after_read {
                        *segment = after_read;
                    } else {
                        current_segment = segments.next().cloned();
                    }
                }
            }
            Ok(copied)
        });
        match result {
            Some(result) => {
                let copied = result?;
                // SAFETY: We just successfully read `copied` bytes from the `MemoryObject`
                // to `data`.
                unsafe { data.advance(copied)? };
                Ok(copied)
            }
            None => self.with_bytes(|bytes| data.write(bytes)),
        }
    }
}

/// Keeps track of inflight vmsplice-ed payloads.
///
/// This is needed so that when a mapping is unmapped, inflight vmspliced payloads
/// are updated to hold the (unmapped) bytes without being affected by any writes
/// to the payload's backing `MemoryObject`.
#[derive(Debug, Default)]
pub struct InflightVmsplicedPayloads {
    /// The inflight vmspliced payloads.
    ///
    /// Except when a [`VmsplicePayload`] is dropped, this is modified when the
    /// memory manager's read lock is held. To allow a `vmsplice` operation to a
    /// pipe to be performed without taking the memory manager's lock exclusively,
    /// this is protected by its own `Mutex` instead of relying on the memory
    /// manager's `RwLock`.
    payloads: LockDepMutex<Vec<Weak<VmsplicePayload>>, InflightVmsplicedPayloadsLock>,
}

impl InflightVmsplicedPayloads {
    fn handle_new_payload(&self, payload: &Arc<VmsplicePayload>) {
        self.payloads.lock().push(Arc::downgrade(payload));
    }

    pub fn handle_unmapping(
        &self,
        unmapped_memory: &Arc<MemoryObject>,
        unmapped_range: &Range<UserAddress>,
    ) -> Result<(), Errno> {
        // Iterate over payloads while removing any deleted payload.
        let mut payloads = self.payloads.lock();
        let mut index = 0;
        while index < payloads.len() {
            let Some(payload) = payloads[index].upgrade() else {
                payloads.swap_remove(index);
                continue;
            };
            index += 1;

            let mut segments = payload.segments.lock();
            let mut new_segments = SmallVec::new();

            for segment in segments.iter() {
                let mut segment = segment.clone();
                let segment_end = (segment.addr_offset + segment.length)?;
                let segment_range = segment.addr_offset..segment_end;
                let segment_unmapped_range = unmapped_range.intersect(&segment_range);

                if &segment.memory != unmapped_memory || segment_unmapped_range.is_empty() {
                    // This can happen when say a partial unmapping was performed
                    // on a `VmsplicePayloadSegment` which split it into a mapped
                    // and unmapped set of payloads.
                    new_segments.push(segment);
                    continue;
                }

                // Keep the mapped head.
                if segment_unmapped_range.start != segment_range.start {
                    if let Some(tail) =
                        segment.split_off(segment_unmapped_range.start - segment_range.start)
                    {
                        new_segments.push(segment);
                        segment = tail;
                    }
                }

                // Keep the mapped tail.
                let tail = segment
                    .split_off(segment.length - (segment_range.end - segment_unmapped_range.end));

                // Snapshot the middle, actually unmapped, region if it should be snapshotted.
                // For file-backed mappings, we want to keep referencing the shared VMO so that
                // subsequent writes are visible.
                //
                // NB: we can't use `zx_vmo_transfer_data` because
                // there may be multiple vmsplice payloads mapped
                // to the same VMO region.
                if segment.should_snapshot_on_unmap {
                    let memory = segment
                        .memory
                        .create_child(
                            zx::VmoChildOptions::SNAPSHOT_MODIFIED | zx::VmoChildOptions::NO_WRITE,
                            segment.memory_offset,
                            segment.length as u64,
                        )
                        .map_err(|_| errno!(EFAULT))?;

                    segment.memory = Arc::new(memory);
                    segment.memory_offset = 0;
                }
                new_segments.push(segment);

                if let Some(tail) = tail {
                    new_segments.push(tail);
                }
            }

            *segments = new_segments;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mm::PAGE_SIZE;
    use crate::testing::spawn_kernel_and_run;
    use crate::vfs::VecOutputBuffer;

    #[::fuchsia::test]
    async fn lifecycle() {
        spawn_kernel_and_run(async |current_task| {
            const NUM_PAGES: u64 = 3;
            let page_size = *PAGE_SIZE;

            let mm = current_task.mm().unwrap();

            assert!(mm.inflight_vmspliced_payloads.payloads.lock().is_empty());

            let memory_size = page_size * NUM_PAGES;
            let memory = Arc::new(MemoryObject::from(zx::Vmo::create(memory_size).unwrap()));
            let mut bytes = vec![0; memory_size as usize];
            for i in 0..NUM_PAGES {
                bytes[(page_size * i) as usize..][..(page_size as usize)].fill('A' as u8 + i as u8)
            }
            memory.write(&bytes, 0).unwrap();

            let payload = VmsplicePayload::new(
                Arc::downgrade(&mm),
                VmsplicePayloadSegment {
                    addr_offset: UserAddress::NULL,
                    length: (page_size * NUM_PAGES) as usize,
                    memory: Arc::clone(&memory),
                    memory_offset: 0,
                    should_snapshot_on_unmap: true,
                },
            );
            assert_eq!(mm.inflight_vmspliced_payloads.payloads.lock().len(), 1);
            assert_eq!(payload.segments.lock().len(), 1);

            // A unmapping a different `MemoryObject` should do nothing.
            {
                let memory = Arc::new(MemoryObject::from(zx::Vmo::create(page_size).unwrap()));
                mm.inflight_vmspliced_payloads
                    .handle_unmapping(&memory, &(UserAddress::NULL..(u64::MAX.into())))
                    .unwrap();
                assert_eq!(payload.segments.lock().len(), 1);
            }

            mm.inflight_vmspliced_payloads
                .handle_unmapping(&memory, &(UserAddress::NULL..page_size.into()))
                .unwrap();
            {
                let segments = payload.segments.lock();
                assert_eq!(segments.len(), 2);
                assert!(!Arc::ptr_eq(&segments[0].memory, &memory));
                assert!(Arc::ptr_eq(&segments[1].memory, &memory));
            }
            let mut got = VecOutputBuffer::new(memory_size as usize);
            payload.copy_to_user(&mut got).unwrap();
            assert_eq!(got.data(), &bytes);

            std::mem::drop(payload);

            // Run the unmapping again to ensure payload is dropped.
            mm.inflight_vmspliced_payloads
                .handle_unmapping(&memory, &(UserAddress::NULL..page_size.into()))
                .unwrap();

            assert!(mm.inflight_vmspliced_payloads.payloads.lock().is_empty());
        })
        .await;
    }
}
