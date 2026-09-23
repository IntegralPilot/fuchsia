// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "256"]

use fidl::endpoints::{DiscoverableProtocolMarker, RequestStream};
use fidl_fuchsia_io as fio;
use fidl_fuchsia_unknown as funknown;
use fuchsia_async as fasync;
use futures::StreamExt;
use smallvec::smallvec;
use starnix_core::fs::fuchsia::new_remote_file;
use starnix_core::mm::{MemoryAccessor, MemoryAccessorExt};
use starnix_core::testing::*;
use starnix_core::vfs::SeekTarget;
use starnix_core::vfs::buffers::{
    Buffer, InputBuffer, InputBufferCallback, OutputBuffer, OutputBufferCallback,
    PeekBufferSegmentsCallback, UserBuffersInputBuffer, UserBuffersOutputBuffer,
};
use starnix_types::user_buffer::UserBuffer;
use starnix_uapi::error;
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::user_address::UserAddress;

async fn mock_remote_file_server(server: zx::Channel, stream: Option<zx::Stream>) {
    let mut stream_req =
        funknown::QueryableRequestStream::from_channel(fasync::Channel::from_channel(server));

    if let Some(Ok(funknown::QueryableRequest::Query { responder })) = stream_req.next().await {
        responder.send(fio::FileMarker::PROTOCOL_NAME.as_bytes()).unwrap();
    } else {
        panic!("Expected Query");
    }

    let (inner, terminated) = stream_req.into_inner();
    let mut stream_req = fio::FileRequestStream::from_inner(inner, terminated);

    if let Some(Ok(fio::FileRequest::Describe { responder })) = stream_req.next().await {
        let info = fio::FileInfo { stream: stream.map(|s| s.into()), ..Default::default() };
        responder.send(info).unwrap();
    } else {
        panic!("Expected Describe");
    }

    while let Some(Ok(request)) = stream_req.next().await {
        match request {
            fio::FileRequest::GetAttributes { query: _, responder } => {
                let attrs = fio::NodeAttributes2 {
                    mutable_attributes: fio::MutableNodeAttributes::default(),
                    immutable_attributes: fio::ImmutableNodeAttributes {
                        protocols: Some(fio::NodeProtocolKinds::FILE),
                        abilities: Some(fio::Operations::READ_BYTES | fio::Operations::WRITE_BYTES),
                        ..Default::default()
                    },
                };
                responder
                    .send(Ok((&attrs.mutable_attributes, &attrs.immutable_attributes)))
                    .unwrap();
            }
            fio::FileRequest::ReadAt { count, responder, .. } => {
                let data = vec![0u8; count as usize];
                responder.send(Ok(&data)).unwrap();
            }
            fio::FileRequest::WriteAt { data, responder, .. } => {
                responder.send(Ok(data.len() as u64)).unwrap();
            }
            fio::FileRequest::Close { responder } => {
                responder.send(Ok(())).unwrap();
            }
            _ => {}
        }
    }
}

#[fuchsia::test]
async fn test_fidl_read_fault() {
    let (client, server) = zx::Channel::create();
    fasync::Task::spawn(mock_remote_file_server(server, None)).detach();

    spawn_kernel_and_run(async move |current_task| {
        let file = new_remote_file(&current_task, client.into(), OpenFlags::RDWR)
            .expect("new_remote_file");

        let addr = map_memory(&current_task, UserAddress::default(), 100);

        let mut buffer = UserBuffersOutputBuffer::unified_new(
            &current_task,
            smallvec![UserBuffer { address: addr, length: 100 }],
        )
        .unwrap();

        // Check a successful read.
        assert_eq!(file.read(&current_task, &mut buffer), Ok(100));

        // Unmap so the address becomes bad.
        current_task.mm().unwrap().unmap(addr, 100).unwrap();

        let mut buffer = UserBuffersOutputBuffer::unified_new(
            &current_task,
            smallvec![UserBuffer { address: addr, length: 100 }],
        )
        .unwrap();

        // The address is bad now, so this should result in EFAULT.
        assert_eq!(file.read(&current_task, &mut buffer), error!(EFAULT));

        // Test with NULL address
        let mut buffer = UserBuffersOutputBuffer::unified_new(
            &current_task,
            smallvec![UserBuffer { address: UserAddress::default(), length: 100 }],
        )
        .unwrap();
        assert_eq!(file.read(&current_task, &mut buffer), error!(EFAULT));
    })
    .await;
}

#[fuchsia::test]
async fn test_fidl_write_fault() {
    let (client, server) = zx::Channel::create();
    fasync::Task::spawn(mock_remote_file_server(server, None)).detach();

    spawn_kernel_and_run(async move |current_task| {
        let file = new_remote_file(&current_task, client.into(), OpenFlags::RDWR)
            .expect("new_remote_file");

        let addr = map_memory(&current_task, UserAddress::default(), 100);

        let mut buffer = UserBuffersInputBuffer::unified_new(
            &current_task,
            smallvec![UserBuffer { address: addr, length: 100 }],
        )
        .unwrap();

        // Check a successful write.
        assert_eq!(file.write(&current_task, &mut buffer), Ok(100));

        // Unmap so the address becomes bad.
        current_task.mm().unwrap().unmap(addr, 100).unwrap();

        let mut buffer = UserBuffersInputBuffer::unified_new(
            &current_task,
            smallvec![UserBuffer { address: addr, length: 100 }],
        )
        .unwrap();

        // The address is bad now, so this should result in EFAULT.
        assert_eq!(file.write(&current_task, &mut buffer), error!(EFAULT));

        // Test with NULL address
        let mut buffer = UserBuffersInputBuffer::unified_new(
            &current_task,
            smallvec![UserBuffer { address: UserAddress::default(), length: 100 }],
        )
        .unwrap();
        assert_eq!(file.write(&current_task, &mut buffer), error!(EFAULT));
    })
    .await;
}

#[derive(Debug)]
struct VectoredOnlyInputBuffer<'a, M> {
    inner: UserBuffersInputBuffer<'a, M>,
}

impl<'a, M: starnix_core::mm::TaskMemoryAccessor + std::fmt::Debug> Buffer
    for VectoredOnlyInputBuffer<'a, M>
{
    fn segments_count(&self) -> Result<usize, Errno> {
        self.inner.segments_count()
    }

    fn peek_each_segment(
        &mut self,
        callback: &mut PeekBufferSegmentsCallback<'_>,
    ) -> Result<(), Errno> {
        self.inner.peek_each_segment(callback)
    }
}

impl<'a, M: starnix_core::mm::TaskMemoryAccessor + std::fmt::Debug> InputBuffer
    for VectoredOnlyInputBuffer<'a, M>
{
    fn peek_each(&mut self, _callback: &mut InputBufferCallback<'_>) -> Result<usize, Errno> {
        panic!("peek_each called");
    }

    fn available(&self) -> usize {
        self.inner.available()
    }

    fn bytes_read(&self) -> usize {
        self.inner.bytes_read()
    }

    fn drain(&mut self) -> usize {
        panic!("drain called");
    }

    fn advance(&mut self, length: usize) -> Result<(), Errno> {
        self.inner.advance(length)
    }
}

#[derive(Debug)]
struct VectoredOnlyOutputBuffer<'a, M> {
    inner: UserBuffersOutputBuffer<'a, M>,
}

impl<'a, M: starnix_core::mm::TaskMemoryAccessor + std::fmt::Debug> Buffer
    for VectoredOnlyOutputBuffer<'a, M>
{
    fn segments_count(&self) -> Result<usize, Errno> {
        self.inner.segments_count()
    }

    fn peek_each_segment(
        &mut self,
        callback: &mut PeekBufferSegmentsCallback<'_>,
    ) -> Result<(), Errno> {
        self.inner.peek_each_segment(callback)
    }
}

impl<'a, M: starnix_core::mm::TaskMemoryAccessor + std::fmt::Debug> OutputBuffer
    for VectoredOnlyOutputBuffer<'a, M>
{
    fn write_each(&mut self, _callback: &mut OutputBufferCallback<'_>) -> Result<usize, Errno> {
        panic!("write_each called");
    }

    fn available(&self) -> usize {
        self.inner.available()
    }

    fn bytes_written(&self) -> usize {
        self.inner.bytes_written()
    }

    fn zero(&mut self) -> Result<usize, Errno> {
        panic!("zero called");
    }

    unsafe fn advance(&mut self, length: usize) -> Result<(), Errno> {
        // SAFETY: The caller guarantees that the buffer is initialized.
        unsafe { self.inner.advance(length) }
    }
}

#[fuchsia::test]
async fn test_vectored_stream_io() {
    let vmo = zx::Vmo::create(1024).unwrap();
    let vmo_clone = vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
    let stream =
        zx::Stream::create(zx::StreamOptions::MODE_READ | zx::StreamOptions::MODE_WRITE, &vmo, 0)
            .unwrap();

    let (client, server) = zx::Channel::create();
    fasync::Task::spawn(mock_remote_file_server(server, Some(stream))).detach();

    spawn_kernel_and_run(async move |current_task| {
        let file = new_remote_file(&current_task, client.into(), OpenFlags::RDWR)
            .expect("new_remote_file");

        let addr1 = map_memory(&current_task, UserAddress::default(), 100);
        let addr2 = map_memory(&current_task, UserAddress::default(), 100);

        // Verify that they are non-adjacent.
        assert!(
            addr1.checked_add(100).unwrap() <= addr2 || addr2.checked_add(100).unwrap() <= addr1
        );

        // Test vectored write.
        current_task.write_memory(addr1, &[1u8; 100]).unwrap();
        current_task.write_memory(addr2, &[2u8; 100]).unwrap();

        let mut buffer = VectoredOnlyInputBuffer {
            inner: UserBuffersInputBuffer::unified_new(
                &current_task,
                smallvec![
                    UserBuffer { address: addr1, length: 100 },
                    UserBuffer { address: addr2, length: 100 },
                ],
            )
            .unwrap(),
        };
        assert_eq!(file.write(&current_task, &mut buffer), Ok(200));
        assert_eq!(buffer.bytes_read(), 200);

        let mut vmo_data = vec![0u8; 200];
        vmo_clone.read(&mut vmo_data, 0).unwrap();
        assert_eq!(&vmo_data[0..100], &[1u8; 100]);
        assert_eq!(&vmo_data[100..200], &[2u8; 100]);

        // Reset the file offset to 0.
        file.seek(&current_task, SeekTarget::Set(0)).unwrap();

        // Test vectored read.
        let pattern = (0..200).map(|i| i as u8).collect::<Vec<_>>();
        vmo_clone.write(&pattern, 0).unwrap();

        let mut buffer = VectoredOnlyOutputBuffer {
            inner: UserBuffersOutputBuffer::unified_new(
                &current_task,
                smallvec![
                    UserBuffer { address: addr1, length: 100 },
                    UserBuffer { address: addr2, length: 100 },
                ],
            )
            .unwrap(),
        };
        assert_eq!(file.read(&current_task, &mut buffer), Ok(200));
        assert_eq!(buffer.bytes_written(), 200);

        let mut mem1 = vec![0u8; 100];
        current_task.read_memory_to_slice(addr1, &mut mem1).unwrap();
        assert_eq!(mem1, pattern[0..100]);

        let mut mem2 = vec![0u8; 100];
        current_task.read_memory_to_slice(addr2, &mut mem2).unwrap();
        assert_eq!(mem2, pattern[100..200]);
    })
    .await;
}
