// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use delivery_blob::compression::{ChunkedArchiveError, DataBuffer};
use fuchsia_sync::Mutex;
use mapping::{
    DELIVERY_DATA_SIZE, DeliveryCommand, DeliveryHandler, PENDING_DELIVERY_COMMANDS_CAPACITY,
    PageRequest, RawDeliveryCommand,
};
use std::ops::Range;
use std::sync::Arc;
use storage_ptr_slice::MutPtrByteSlice;
use vmo_fifo::SyncSender;

/// Manages data verification and page supply via the delivery queue VMO.
pub struct Verifier {
    sender: Mutex<Option<SyncSender<RawDeliveryCommand>>>,
}

impl Verifier {
    pub fn new(delivery_queue: zx::Vmo) -> Self {
        let sender = if delivery_queue.get_size().unwrap_or(0) > 0 {
            // Payloads in the delivery queue are transferred to the kernel pager via
            // `zx_pager_supply_pages`, which requires the source offset to be page-aligned. Set
            // the alignment of payload offsets to be page aligned.
            SyncSender::<RawDeliveryCommand>::new(
                delivery_queue,
                zx::system_get_page_size() as usize,
                PENDING_DELIVERY_COMMANDS_CAPACITY,
            )
            .ok()
        } else {
            None
        };
        Self { sender: Mutex::new(sender) }
    }
}

impl DeliveryHandler for Verifier {
    type Request = Buffer;

    /// Returns a [`PageRequest`] implementation for delivering page-in data.
    fn get_page_request(self: &Arc<Self>, key: u64, original_range: Range<u64>) -> Buffer {
        Buffer {
            verifier: Arc::clone(self),
            key,
            read_range: original_range,
            write_cursor: 0,
            delivered_len: 0,
            data: Vec::new(),
        }
    }

    /// Registers a blob's Merkle tree leaf hashes with the verifier via the delivery queue.
    fn register_blob(&self, key: u64, merkle_leaves: &[[u8; 32]]) -> Result<(), anyhow::Error> {
        let mut sender_guard = self.sender.lock();
        if let Some(sender) = sender_guard.as_mut() {
            let leaf_bytes: &[u8] = merkle_leaves.as_flattened();
            let mut payload = sender.reserve_payload(leaf_bytes.len())?;
            payload.data().copy_from_slice(leaf_bytes);
            let cmd = DeliveryCommand::RegisterBlob {
                key,
                offset: payload.offset(),
                length: leaf_bytes.len() as u32,
            };
            payload.commit(cmd.into())?;
        }
        Ok(())
    }
}

/// Implementation of [`PageRequest`] for verifier deliveries that forwards
/// unverified chunks via the delivery queue.
pub struct Buffer {
    verifier: Arc<Verifier>,
    key: u64,
    read_range: Range<u64>,
    // Write cursor into `self.data` advanced by calls to [`DataBuffer::commit`].
    write_cursor: usize,
    // Bytes delivered to the verifier queue so far.
    delivered_len: usize,
    data: Vec<u8>,
}

impl Buffer {
    fn deliver_chunk(&mut self, size: usize) -> Result<(), ChunkedArchiveError> {
        // TODO(https://fxbug.dev/530494057): Optimize buffer management / payload reservations.
        let chunk_data = &self.data[self.delivered_len..self.delivered_len + size];
        let offset = self.read_range.start + self.delivered_len as u64;

        let mut sender_guard = self.verifier.sender.lock();
        if let Some(sender) = sender_guard.as_mut() {
            let page_size = zx::system_get_page_size() as usize;
            let aligned_size = size.next_multiple_of(page_size);
            let mut payload = sender.reserve_payload(aligned_size).map_err(|e| {
                log::error!(e:?; "Verifier::deliver_chunk: reserve_payload failed");
                ChunkedArchiveError::IntegrityError
            })?;

            let payload_data = payload.data();
            payload_data.subslice_mut(0..size).copy_from_slice(chunk_data);
            payload_data.subslice_mut(size..aligned_size).fill(0);
            let cmd = DeliveryCommand::Data {
                key: self.key,
                target_offset: offset,
                length: aligned_size as u32,
                offset: payload.offset(),
            };
            payload.commit(cmd.into()).map_err(|_| ChunkedArchiveError::IntegrityError)?;
        }

        self.delivered_len += size;
        Ok(())
    }
}

impl DataBuffer for Buffer {
    fn range(&self) -> Range<u64> {
        self.read_range.clone()
    }

    /// Returns a mutable slice into the mapped transfer buffer.
    ///
    /// # Panics
    ///
    /// Panics if `prepare()` has not been called prior to accessing this method.
    fn mut_ptr_slice(&mut self) -> MutPtrByteSlice<'_> {
        assert!(!self.data.is_empty(), "prepare must be called before accessing mut_ptr_slice");
        MutPtrByteSlice::from(&mut self.data[self.write_cursor..])
    }

    fn commit(&mut self, size: usize) -> Result<(), ChunkedArchiveError> {
        self.write_cursor += size;

        // Deliver complete DELIVERY_DATA_SIZE chunks to the verifier queue as data is committed.
        while self.write_cursor - self.delivered_len >= DELIVERY_DATA_SIZE {
            self.deliver_chunk(DELIVERY_DATA_SIZE)?;
        }

        // Deliver any trailing remainder once the entire prepared range has been committed.
        if self.write_cursor == self.data.len() && self.delivered_len < self.write_cursor {
            self.deliver_chunk(self.write_cursor - self.delivered_len)?;
        }

        Ok(())
    }
}

impl PageRequest for Buffer {
    fn prepare(&mut self, read_range: Range<u64>) -> Result<(), ChunkedArchiveError> {
        assert!(self.data.is_empty(), "prepare must only be called once");
        self.read_range = read_range;
        let len = (self.read_range.end - self.read_range.start) as usize;
        self.data = vec![0u8; len];
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mapping::DELIVERY_DATA_COMMAND;

    #[fuchsia::test]
    fn test_verifier_buffer_incremental_commit() {
        let delivery_queue = zx::Vmo::create(65536).unwrap();
        let mut receiver = vmo_fifo::Receiver::<RawDeliveryCommand>::new(
            delivery_queue.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
            PENDING_DELIVERY_COMMANDS_CAPACITY,
        )
        .unwrap();

        let verifier = Arc::new(Verifier::new(delivery_queue));
        let key = 42u64;

        let mut request = verifier.get_page_request(key, 0..8192);
        request.prepare(0..8192).expect("prepare");

        // Fill first page (4096 bytes) with 0xAA
        let page1_data = vec![0xAAu8; 4096];
        request.mut_ptr_slice().subslice_mut(0..4096).copy_from_slice(&page1_data);
        request.commit(4096).expect("commit page 1");

        // Verify mut_ptr_slice advanced to second page
        assert_eq!(request.mut_ptr_slice().len(), 4096);
        // Not all data has been committed yet, so no command should be delivered yet.
        assert!(receiver.is_empty());

        // Fill second page (4096 bytes) with 0xBB
        let page2_data = vec![0xBBu8; 4096];
        request.mut_ptr_slice().subslice_mut(0..4096).copy_from_slice(&page2_data);
        request.commit(4096).expect("commit page 2");

        // Verify remaining space is 0
        assert_eq!(request.mut_ptr_slice().len(), 0);

        // Check single consolidated message delivered on queue
        let msg = receiver.peek().expect("msg");
        assert_eq!(msg.opcode, DELIVERY_DATA_COMMAND);
        assert_eq!(msg.key, key);
        assert_eq!(msg.length, 8192);
        assert_eq!(msg.target_offset, 0);
        let mut buf = vec![0u8; 8192];
        msg.payload_slice(msg.offset, msg.length).copy_to_slice(&mut buf);
        assert_eq!(&buf[..4096], &page1_data[..]);
        assert_eq!(&buf[4096..], &page2_data[..]);
        msg.pop().expect("pop msg");
    }

    #[fuchsia::test]
    fn test_verifier_buffer_page_aligned_commits() {
        let delivery_queue = zx::Vmo::create(65536).unwrap();
        let mut receiver = vmo_fifo::Receiver::<RawDeliveryCommand>::new(
            delivery_queue.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
            PENDING_DELIVERY_COMMANDS_CAPACITY,
        )
        .unwrap();

        let verifier = Arc::new(Verifier::new(delivery_queue));
        let key = 43u64;

        let mut request = verifier.get_page_request(key, 0..5000);
        request.prepare(0..5000).expect("prepare");

        let data = vec![0xCCu8; 5000];

        // Commit first 4096 bytes
        request.mut_ptr_slice().subslice_mut(0..4096).copy_from_slice(&data[..4096]);
        request.commit(4096).expect("commit 4096 bytes");
        assert!(receiver.is_empty());

        // Commit remaining 904 bytes (completes the prepared range)
        request.mut_ptr_slice().subslice_mut(0..904).copy_from_slice(&data[4096..5000]);
        request.commit(904).expect("commit 904 bytes");

        // Check single message delivered, page-aligned to 8192
        let msg = receiver.peek().expect("msg");
        assert_eq!(msg.target_offset, 0);
        assert_eq!(msg.length, 8192);
        let mut buf = vec![0u8; 8192];
        msg.payload_slice(msg.offset, msg.length).copy_to_slice(&mut buf);
        assert_eq!(&buf[..5000], &data[..]);
        assert_eq!(&buf[5000..], &[0u8; 3192]);
        msg.pop().expect("pop msg");
    }

    #[fuchsia::test]
    fn test_verifier_buffer_delivery_data_size_chunking() {
        let total_size = DELIVERY_DATA_SIZE * 2;
        let delivery_queue = zx::Vmo::create(total_size as u64 + 65536).unwrap();
        let mut receiver = vmo_fifo::Receiver::<RawDeliveryCommand>::new(
            delivery_queue.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
            PENDING_DELIVERY_COMMANDS_CAPACITY,
        )
        .unwrap();

        let verifier = Arc::new(Verifier::new(delivery_queue));
        let key = 44u64;

        let mut request = verifier.get_page_request(key, 0..total_size as u64);
        request.prepare(0..total_size as u64).expect("prepare");

        let chunk_32k = vec![0x55u8; 32 * 1024];

        // Push 3 chunks of 32 KiB (96 KiB) - less than DELIVERY_DATA_SIZE
        for _ in 0..3 {
            request.mut_ptr_slice().subslice_mut(0..32 * 1024).copy_from_slice(&chunk_32k);
            request.commit(32 * 1024).expect("commit");
        }
        assert!(receiver.is_empty());

        // Push 4th chunk (now 128 KiB == DELIVERY_DATA_SIZE)
        request.mut_ptr_slice().subslice_mut(0..32 * 1024).copy_from_slice(&chunk_32k);
        request.commit(32 * 1024).expect("commit");

        // 1st 128 KiB message delivered
        let msg1 = receiver.peek().expect("msg1");
        assert_eq!(msg1.target_offset, 0);
        assert_eq!(msg1.length, DELIVERY_DATA_SIZE as u32);
        msg1.pop().expect("pop msg1");

        // Push remaining 4 chunks (another 128 KiB)
        for _ in 0..4 {
            request.mut_ptr_slice().subslice_mut(0..32 * 1024).copy_from_slice(&chunk_32k);
            request.commit(32 * 1024).expect("commit");
        }

        // 2nd 128 KiB message delivered
        let msg2 = receiver.peek().expect("msg2");
        assert_eq!(msg2.target_offset, DELIVERY_DATA_SIZE as u64);
        assert_eq!(msg2.length, DELIVERY_DATA_SIZE as u32);
        msg2.pop().expect("pop msg2");
    }

    #[fuchsia::test]
    #[should_panic(expected = "prepare must only be called once")]
    fn test_verifier_buffer_prepare_multiple_times_panics() {
        let delivery_queue = zx::Vmo::create(4096).unwrap();
        let verifier = Arc::new(Verifier::new(delivery_queue));

        let key = 46u64;
        let mut request = verifier.get_page_request(key, 0..8192);
        request.prepare(0..4096).expect("first prepare");
        let _ = request.prepare(0..8192);
    }

    #[fuchsia::test]
    fn test_verifier_buffer_supplies_pages_via_delivery_receiver() {
        let pager = Arc::new(zx::Pager::create(zx::PagerOptions::empty()).unwrap());
        let port = zx::Port::create();
        let key = 44u64;
        let paged_vmo = pager.create_vmo(zx::VmoOptions::empty(), &port, key, 8192).unwrap();

        let delivery_queue = zx::Vmo::create(65536).unwrap();
        let vmo_provider = Arc::new(blob_pager_and_verifier::TestVmoProvider::new(
            pager.clone(),
            delivery_queue.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
        ));
        vmo_provider
            .register_vmo(key, paged_vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap());
        let receiver = vmo_fifo::Receiver::<RawDeliveryCommand>::new(
            delivery_queue.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
            PENDING_DELIVERY_COMMANDS_CAPACITY,
        )
        .unwrap();
        let _processor = blob_pager_and_verifier::DeliveryQueueProcessor::spawn(
            receiver,
            vmo_provider,
            delivery_queue.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
        )
        .unwrap();

        let verifier = Arc::new(Verifier::new(delivery_queue));
        let mut request = verifier.get_page_request(key, 0..8192);
        request.prepare(0..8192).expect("prepare");

        let page1_data = vec![0xAAu8; 4096];
        request.mut_ptr_slice().subslice_mut(0..4096).copy_from_slice(&page1_data);
        request.commit(4096).expect("commit page 1");

        let page2_data = vec![0xBBu8; 4096];
        request.mut_ptr_slice().subslice_mut(0..4096).copy_from_slice(&page2_data);
        request.commit(4096).expect("commit page 2");

        let mut read_buf = vec![0u8; 8192];
        paged_vmo.read(&mut read_buf, 0).expect("read paged_vmo");
        assert_eq!(&read_buf[..4096], &page1_data[..]);
        assert_eq!(&read_buf[4096..8192], &page2_data[..]);
    }

    #[fuchsia::test]
    fn test_verifier_register_blob() {
        let delivery_queue = zx::Vmo::create(65536).unwrap();
        let mut receiver = vmo_fifo::Receiver::<RawDeliveryCommand>::new(
            delivery_queue.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
            PENDING_DELIVERY_COMMANDS_CAPACITY,
        )
        .unwrap();

        let verifier = Arc::new(Verifier::new(delivery_queue));
        let key = 42u64;
        let leaves = [[0xABu8; 32], [0xCDu8; 32]];

        verifier.register_blob(key, &leaves).expect("register_blob failed");

        let msg = receiver.peek().expect("peek msg");
        assert_eq!(msg.opcode, mapping::DELIVERY_REGISTER_BLOB_COMMAND);
        assert_eq!(msg.key, key);
        assert_eq!(msg.length, 64);
        let mut buf = vec![0u8; 64];
        msg.payload_slice(msg.offset, msg.length).copy_to_slice(&mut buf);
        assert_eq!(&buf[..32], &[0xABu8; 32]);
        assert_eq!(&buf[32..], &[0xCDu8; 32]);
        msg.pop().expect("pop msg failed");
    }
}
