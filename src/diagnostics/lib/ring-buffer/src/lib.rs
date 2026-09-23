// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_async::{self as fasync};
use futures::task::AtomicWaker;
use std::future::poll_fn;
use std::ops::{Deref, Range};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::task::Poll;
use thiserror::Error;

/// Size of the kernel header in the ring buffer. This is different to the FXT header.
pub const RING_BUFFER_MESSAGE_HEADER_SIZE: usize = 16;

/// Maximum message size. This includes the ring buffer header. This is also the minimum capacity
/// for the ring buffer.
pub const MAX_MESSAGE_SIZE: usize = 65536;

// The ring buffer consists of a head index, tail index on the first page. The ring buffer proper
// starts from the second page. The head and tail indices never wrap; modulo arithmetic is used to
// get to the actual offset in the buffer.
//
// Messages in the ring buffer consist of a 64 bit tag, followed by a 64 bit length. The remainder
// of the message is a message in the diagnostics log format. The tag and length are written by the
// kernel, so can be trusted whilst the rest of the message can't be. Messages will always be 64
// bit aligned, but the length doesn't have to be, which means that when the tail index is advanced,
// it must always be advanced to maintain that alignment.

const HEAD_OFFSET: usize = 0; // Offset of the head index in the ring buffer.
const TAIL_OFFSET: usize = 8; // Offset of the tail index in the ring buffer.

/// RingBuffer wraps a IOBuffer shared region and mapping that uses the ring buffer discipline.
pub struct RingBuffer {
    shared_region: zx::vdso_next::IobSharedRegion,
    _vmar: zx::Vmar,
    base: usize,
    capacity: usize,
}

#[derive(Eq, Error, Debug, PartialEq)]
pub enum Error {
    #[error("attempt to read a message at an unaligned index")]
    BadAlignment,
    #[error("there is not enough room to read the header")]
    TooSmall,
    #[error("bad message length (e.g. too big or beyond bounds)")]
    BadLength,
}

impl RingBuffer {
    /// Returns a new RingBuffer and Reader. `capacity` must be a multiple of the page size and
    /// should be at least `MAX_MESSAGE_SIZE`. The `capacity` does not include the additional page
    /// used to store the head and tail indices.
    pub fn create(capacity: usize) -> Reader {
        let page_size = zx::system_get_page_size() as usize;

        assert_eq!(capacity % page_size, 0);
        assert!(capacity >= MAX_MESSAGE_SIZE);

        let shared_region_size = capacity + page_size;
        let shared_region =
            zx::vdso_next::IobSharedRegion::create(Default::default(), shared_region_size as u64)
                .unwrap();

        // We only need one endpoint.
        let (iob, _) = zx::Iob::create(
            Default::default(),
            &[zx::IobRegion {
                region_type: zx::IobRegionType::Shared {
                    options: Default::default(),
                    region: &shared_region,
                },
                access: zx::IobAccess::EP0_CAN_MAP_READ | zx::IobAccess::EP0_CAN_MAP_WRITE,
                discipline: zx::IobDiscipline::MediatedWriteRingBuffer { tag: 0 },
            }],
        )
        .unwrap();

        let root_vmar = fuchsia_runtime::vmar_root_self();

        // Map the buffer but repeat the mapping for the first 64 KiB of the buffer at the end
        // which makes dealing with wrapping much easier.
        //
        // NOTE: dropping the vmar will drop the mappings.
        let (vmar, base) = root_vmar
            .allocate(
                0,
                shared_region_size + MAX_MESSAGE_SIZE,
                zx::VmarFlags::CAN_MAP_READ
                    | zx::VmarFlags::CAN_MAP_WRITE
                    | zx::VmarFlags::CAN_MAP_SPECIFIC,
            )
            .unwrap();
        vmar.map_iob(
            zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE | zx::VmarFlags::SPECIFIC,
            /* vmar_offset */ 0,
            &iob,
            /* region_index */ 0,
            /* region_offset */ 0,
            /* region_len */ shared_region_size,
        )
        .unwrap();
        vmar.map_iob(
            zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE | zx::VmarFlags::SPECIFIC,
            /* vmar_offset */ shared_region_size,
            &iob,
            /* region_index */ 0,
            /* region_offset */ zx::system_get_page_size() as u64,
            /* region_len */ MAX_MESSAGE_SIZE,
        )
        .unwrap();

        let this = Arc::new(Self { shared_region, _vmar: vmar, base, capacity });
        Reader {
            ring_buffer: this,
            registration: fasync::EHandle::local().register_receiver(Receiver::default()),
        }
    }

    /// Returns an IOBuffer that can be used to write to the ring buffer. A tuple is returned; the
    /// first IOBuffer in the tuple can be written to. The second IOBuffer is the peer and cannot
    /// be written to or mapped but it can be monitored for peer closed.
    pub fn new_iob_writer(&self, tag: u64) -> Result<(zx::Iob, zx::Iob), zx::Status> {
        zx::Iob::create(
            Default::default(),
            &[zx::IobRegion {
                region_type: zx::IobRegionType::Shared {
                    options: Default::default(),
                    region: &self.shared_region,
                },
                access: zx::IobAccess::EP0_CAN_MEDIATED_WRITE,
                discipline: zx::IobDiscipline::MediatedWriteRingBuffer { tag },
            }],
        )
    }

    /// Returns the capacity of the ring buffer.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns the value of the head pointer, read with Acquire ordering (which will synchronise
    /// with an update to the head pointer in the kernel that uses Release ordering).
    pub fn head(&self) -> u64 {
        // SAFETY: This should be aligned and we mapped base, so the pointer should be
        // dereferenceable.
        unsafe { (*((self.base + HEAD_OFFSET) as *const AtomicU64)).load(Ordering::Acquire) }
    }

    /// Returns the value of the tail pointer, read with Acquire ordering (which will synchronise
    /// with an update to the tail via `increment_tail` below; the kernel never changes the tail
    /// pointer).
    pub fn tail(&self) -> u64 {
        // SAFETY: This should be aligned and we mapped base, so the pointer should be
        // dereferenceable.
        unsafe { (*((self.base + TAIL_OFFSET) as *const AtomicU64)).load(Ordering::Acquire) }
    }

    /// Increments the tail pointer, synchronized with Release ordering (which will synchronise with
    /// the kernel that reads the tail pointer using Acquire ordering). `amount` should always be
    /// a multiple of 8. See also `ring_buffer_record_len`.
    pub fn increment_tail(&self, amount: usize) {
        assert_eq!(amount % 8, 0);
        // SAFETY: This should be aligned and we mapped base, so the pointer should be
        // dereferenceable.
        unsafe {
            (*((self.base + TAIL_OFFSET) as *const AtomicU64))
                .fetch_add(amount as u64, Ordering::Release);
        }
    }

    fn index_to_offset(&self, index: u64) -> usize {
        (index % self.capacity as u64) as usize
    }

    /// Reads T at `index` in the buffer.
    ///
    /// # SAFETY
    ///
    /// `index` must have the same alignment as `T`. The read is non-atomic which means it is
    /// undefined behaviour if there is concurrent write access to the same location (across all
    /// processes).
    pub unsafe fn read<T>(&self, index: u64) -> T {
        debug_assert_eq!(index % std::mem::align_of::<T>() as u64, 0);
        debug_assert!(std::mem::size_of::<T>() <= MAX_MESSAGE_SIZE);
        let offset = self.index_to_offset(index);
        unsafe { ((self.base + zx::system_get_page_size() as usize + offset) as *const T).read() }
    }

    /// Returns a slice for the first message in `range`.
    ///
    /// # SAFETY
    ///
    /// The reads are non-atomic so there can be no other concurrent write access to the same range
    /// (across all processes). The returned slice will only remain valid so long as there is no
    /// other concurrent write access to the range.
    pub unsafe fn first_message_in(&self, range: Range<u64>) -> Result<(u64, &[u8]), Error> {
        if !range.start.is_multiple_of(8) {
            return Err(Error::BadAlignment);
        }
        if range.end - range.start < RING_BUFFER_MESSAGE_HEADER_SIZE as u64 {
            return Err(Error::TooSmall);
        }
        let tag = unsafe { self.read(range.start) };
        let message_len: u64 = unsafe { self.read(range.start + 8) };
        if message_len
            > std::cmp::min(range.end - range.start, MAX_MESSAGE_SIZE as u64)
                - RING_BUFFER_MESSAGE_HEADER_SIZE as u64
        {
            return Err(Error::BadLength);
        }
        let index = self.index_to_offset(range.start + 16);
        Ok((tag, unsafe {
            std::slice::from_raw_parts(
                (self.base + zx::system_get_page_size() as usize + index) as *const u8,
                message_len as usize,
            )
        }))
    }
}

/// Provides exclusive read access to the ring buffer.
pub struct Reader {
    ring_buffer: Arc<RingBuffer>,
    registration: fasync::ReceiverRegistration<Receiver>,
}

impl Deref for Reader {
    type Target = Arc<RingBuffer>;
    fn deref(&self) -> &Self::Target {
        &self.ring_buffer
    }
}

impl Reader {
    /// Waits for the head to exceed `index`. Returns head.
    pub async fn wait(&mut self, index: u64) -> u64 {
        poll_fn(|cx| {
            // Check before registering the waker.
            let head = self.head();
            if head > index {
                return Poll::Ready(head);
            }
            self.registration.waker.register(cx.waker());
            if !self.registration.async_wait.swap(true, Ordering::Relaxed) {
                self.shared_region
                    .wait_async(
                        self.registration.port(),
                        self.registration.key(),
                        zx::Signals::IOB_SHARED_REGION_UPDATED,
                        zx::WaitAsyncOpts::empty(),
                    )
                    .unwrap();
            }
            // Check again in case there was a race.
            let head = self.head();
            if head > index { Poll::Ready(head) } else { Poll::Pending }
        })
        .await
    }

    /// Reads a message from `tail`. If no message is ready, this will wait. This will advance
    /// the `tail`.
    pub async fn read_message(&mut self) -> Result<(u64, Vec<u8>), Error> {
        let tail = self.tail();
        let head = self.wait(tail).await;
        // SAFETY: There should be no other concurrent write access to this memory because writing
        // is only allowed via `new_iob_writer` above, and that will always write beyond `head`.
        let message = unsafe {
            self.first_message_in(tail..head).map(|(tag, message)| (tag, message.to_vec()))?
        };
        self.increment_tail(ring_buffer_record_len(message.1.len()));
        Ok(message)
    }
}

#[derive(Default)]
struct Receiver {
    waker: AtomicWaker,
    async_wait: AtomicBool,
}

impl fasync::PacketReceiver for Receiver {
    fn receive_packet(&self, _packet: zx::Packet) {
        self.async_wait.store(false, Ordering::Relaxed);
        self.waker.wake();
    }
}

/// Returns the ring buffer record length given the message length. This accounts for the ring
/// buffer message header and any padding required to maintain alignment.
pub fn ring_buffer_record_len(message_len: usize) -> usize {
    RING_BUFFER_MESSAGE_HEADER_SIZE + message_len.next_multiple_of(8)
}

#[cfg(test)]
mod tests {
    use super::{Error, MAX_MESSAGE_SIZE, RING_BUFFER_MESSAGE_HEADER_SIZE, RingBuffer};
    use futures::stream::FuturesUnordered;
    use futures::{FutureExt, StreamExt};
    use std::sync::atomic::{AtomicU64, Ordering};

    #[fuchsia::test]
    async fn read_message() {
        const TAG: u64 = 56;
        let mut reader = RingBuffer::create(128 * 1024);
        let (iob, _) = reader.new_iob_writer(TAG).unwrap();
        const DATA: &[u8] = b"test";
        iob.write(Default::default(), 0, DATA).unwrap();
        let (tag, data) = reader.read_message().await.expect("read_message failed");
        assert_eq!(tag, TAG);
        assert_eq!(&data, DATA);
    }

    #[fuchsia::test]
    async fn writing_wakes_reader() {
        const TAG: u64 = 56;
        let mut reader = RingBuffer::create(128 * 1024);
        let (iob, _) = reader.new_iob_writer(TAG).unwrap();

        // Use FuturesUnordered so that it uses its own waker.
        let mut read_message = FuturesUnordered::from_iter([reader.read_message()]);

        // Poll the reader once to prime it.
        assert!(read_message.next().now_or_never().is_none());

        const DATA: &[u8] = b"test";
        iob.write(Default::default(), 0, DATA).unwrap();

        // Check the reader is woken.
        let (tag, data) = read_message.next().await.unwrap().expect("read_message failed");
        assert_eq!(tag, TAG);
        assert_eq!(&data, DATA);
    }

    #[fuchsia::test]
    async fn corrupt() {
        let mut reader = RingBuffer::create(128 * 1024);

        const HEAD_OFFSET: usize = 0;
        const TAIL_OFFSET: usize = 8;
        let message_len_offset: usize = zx::system_get_page_size() as usize + 8;

        let base = reader.base;
        let write_u64 = |offset, value| unsafe {
            (*((base + offset) as *const AtomicU64)).store(value, Ordering::Release);
        };

        // Unaligned tail
        write_u64(TAIL_OFFSET, 1);
        write_u64(HEAD_OFFSET, 8);
        assert_eq!(reader.read_message().await, Err(Error::BadAlignment));

        // Too small.
        write_u64(TAIL_OFFSET, 0);
        assert_eq!(reader.read_message().await, Err(Error::TooSmall));

        // Exceeds max message size.
        write_u64(HEAD_OFFSET, 32);
        write_u64(
            message_len_offset,
            (MAX_MESSAGE_SIZE + RING_BUFFER_MESSAGE_HEADER_SIZE + 1) as u64,
        );
        assert_eq!(reader.read_message().await, Err(Error::BadLength));

        // Message too big vs head - tail.
        write_u64(message_len_offset, 17);
        assert_eq!(reader.read_message().await, Err(Error::BadLength));

        // And finally, a valid message, just to make sure there isn't another issue.
        write_u64(message_len_offset, 16);
        assert!(reader.read_message().await.is_ok());
    }
}
