// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use flex_fuchsia_memory_stacktrack_client as fstacktrack_client;
use measure_tape_for_snapshot_element::Measurable;
use zx_types::ZX_CHANNEL_MAX_MSG_BYTES;

use crate::Error;

// Number of bytes the header of a vector occupies in a fidl message.
// TODO(https://fxbug.dev/42181010): This should be a constant in a FIDL library.
const FIDL_VECTOR_HEADER_BYTES: usize = 16;

// Number of bytes the header of a fidl message occupies.
// TODO(https://fxbug.dev/42181010): This should be a constant in a FIDL library.
const FIDL_HEADER_BYTES: usize = 16;

// Size of the fixed part of a `SnapshotReceiver/Batch` FIDL message. The actual size is given by
// this number plus the size of each element in the batch.
const EMPTY_BUFFER_SIZE: usize = FIDL_HEADER_BYTES + FIDL_VECTOR_HEADER_BYTES;

/// Implements pagination on top of a SnapshotReceiver channel.
pub struct Streamer<'a> {
    dest: &'a mut fstacktrack_client::SnapshotReceiverProxy,
    buffer: Vec<fstacktrack_client::SnapshotElement>,
    buffer_size: usize,
}

impl<'a> Streamer<'a> {
    /// Prepares to send a snapshot over the given channel.
    ///
    /// Takes a mutable reference to be sure that nobody else can write into the channel at the
    /// same time.
    pub fn new(dest: &'a mut fstacktrack_client::SnapshotReceiverProxy) -> Streamer<'a> {
        Streamer { dest, buffer: Vec::new(), buffer_size: EMPTY_BUFFER_SIZE }
    }

    /// Sends the given `elem`.
    ///
    /// This method internally flushes the outgoing buffer, if necessary, so that it never exceeds
    /// the maximum allowed size.
    pub async fn push_element(
        mut self,
        elem: fstacktrack_client::SnapshotElement,
    ) -> Result<Self, Error> {
        let elem_size = elem.measure().num_bytes;

        // Flush the current buffer if the new element would not fit in it.
        if self.buffer_size + elem_size > ZX_CHANNEL_MAX_MSG_BYTES as usize {
            self.flush_buffer().await?;
        }

        // Append the new element.
        self.buffer.push(elem);
        self.buffer_size += elem_size;

        Ok(self)
    }

    /// Sends the end-of-snapshot marker.
    pub async fn end_of_snapshot(mut self) -> Result<(), Error> {
        // Send the last elements in the queue.
        if !self.buffer.is_empty() {
            self.flush_buffer().await?;
        }

        // Send an empty batch to signal the end of the snapshot.
        self.flush_buffer().await?;

        Ok(())
    }

    async fn flush_buffer(&mut self) -> Result<(), Error> {
        // Read and reset the buffer.
        let buffer = std::mem::replace(&mut self.buffer, Vec::new());
        self.buffer_size = EMPTY_BUFFER_SIZE;

        // Send it.
        let fut = self.dest.batch(&buffer);
        Ok(fut.await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::create_client;
    use fuchsia_async as fasync;
    use std::collections::HashMap;
    use test_case::test_case;

    use crate::snapshot::Snapshot;

    const FAKE_PAGE_SIZE: u64 = 4096;
    const FAKE_REGION_ADDRESS: u64 = 0x10000;
    const FAKE_REGION_SIZE: u64 = 0x2000;
    const FAKE_REGION_NAME: &str = "libexample.so";
    const FAKE_REGION_VADDR: u64 = 0x5000;
    const FAKE_REGION_BUILD_ID: &[u8] = &[0xAA, 0xBB, 0xCC, 0xDD];

    fn generate_fake_threads(n: usize) -> HashMap<u64, Vec<(u64, u64)>> {
        let mut result = HashMap::new();
        for i in 0..n as u64 {
            let koid = 1000 + i;

            // Generate a stack trace with a pseudo-random number of fake frames.
            let mut frames = Vec::new();
            for j in 0..(i % 321) {
                frames.push(((i + j) ^ 0x1111, (i + j) ^ 0x2222));
            }

            result.insert(koid, frames);
        }
        result
    }

    #[test_case(0)]
    #[test_case(1)]
    #[test_case(100000)]
    #[fuchsia::test]
    async fn test_streamer(num_threads: usize) {
        let fake_threads = generate_fake_threads(num_threads);

        let client = create_client();
        let (mut receiver_proxy, receiver_stream) =
            client.create_proxy_and_stream::<fstacktrack_client::SnapshotReceiverMarker>();
        let receive_worker = fasync::Task::local(Snapshot::receive_from(receiver_stream));

        // Transmit a snapshot with a page size, stack traces and an executable region.
        let mut streamer = Streamer::new(&mut receiver_proxy)
            .push_element(fstacktrack_client::SnapshotElement::PageSize(FAKE_PAGE_SIZE))
            .await
            .unwrap()
            .push_element(fstacktrack_client::SnapshotElement::ExecutableRegion(
                fstacktrack_client::ExecutableRegion {
                    address: Some(FAKE_REGION_ADDRESS),
                    size: Some(FAKE_REGION_SIZE),
                    name: Some(FAKE_REGION_NAME.to_string()),
                    vaddr: Some(FAKE_REGION_VADDR),
                    build_id: Some(fstacktrack_client::BuildId {
                        value: FAKE_REGION_BUILD_ID.to_vec(),
                    }),
                    ..Default::default()
                },
            ))
            .await
            .unwrap();
        for (koid, frames) in &fake_threads {
            streamer = streamer
                .push_element(fstacktrack_client::SnapshotElement::StackTrace(
                    fstacktrack_client::StackTrace {
                        thread_koid: Some(*koid),
                        frames: Some(
                            frames
                                .iter()
                                .map(|(pc, fp)| fstacktrack_client::CallFrame {
                                    program_address: *pc,
                                    frame_pointer: *fp,
                                })
                                .collect(),
                        ),
                        ..Default::default()
                    },
                ))
                .await
                .unwrap();
        }
        streamer.end_of_snapshot().await.unwrap();

        // Receive the snapshot we just transmitted and verify its contents.
        let received_snapshot = receive_worker.await.unwrap();
        assert_eq!(received_snapshot.page_size, FAKE_PAGE_SIZE);

        let mut received_stack_traces: HashMap<u64, &crate::snapshot::StackTrace> =
            received_snapshot.stack_traces.iter().map(|(koid, trace)| (*koid, trace)).collect();
        assert_eq!(received_snapshot.executable_regions.len(), 1);
        let (address, region) = received_snapshot.executable_regions.iter().next().unwrap();
        assert_eq!(*address, FAKE_REGION_ADDRESS);
        assert_eq!(region.size, FAKE_REGION_SIZE);
        assert_eq!(region.vaddr, FAKE_REGION_VADDR);
        assert_eq!(region.name, FAKE_REGION_NAME);
        assert_eq!(region.build_id, FAKE_REGION_BUILD_ID);
        for (expected_koid, expected_frames) in &fake_threads {
            let received_trace = received_stack_traces.remove(expected_koid).unwrap();

            assert_eq!(received_trace.frames.len(), expected_frames.len());
            for (received_frame, (expected_pc, expected_fp)) in
                received_trace.frames.iter().zip(expected_frames)
            {
                assert_eq!(received_frame.program_address, *expected_pc);
                assert_eq!(received_frame.frame_pointer, *expected_fp);
            }
        }
        assert!(received_stack_traces.is_empty(), "all the entries have been removed");
    }
}
