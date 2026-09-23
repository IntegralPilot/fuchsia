// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::async_interface::Interface;
use crate::{BlockServer, ReadOptions, TraceFlowId, WriteOptions};
use block_protocol::{BlockFifoCommand, BlockFifoRequest, BlockFifoResponse};
use fidl_fuchsia_storage_block as fblock;
use fidl_fuchsia_storage_block::{BlockIoFlag, BlockMarker, BlockOpcode};
use fuchsia_async::{self as fasync, TimeoutExt};
use futures::StreamExt;
use futures::channel::{mpsc, oneshot};
use rand::RngExt as _;
use rand::seq::SliceRandom as _;
use std::borrow::Cow;
use std::num::NonZero;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use zstd;

const BLOCK_SIZE: u32 = 512;

struct MockInterface {
    read_hook: Option<
        Box<
            dyn Fn(
                    u64,
                    u32,
                    &Arc<zx::Vmo>,
                    u64,
                ) -> futures::future::BoxFuture<'static, Result<(), zx::Status>>
                + Send
                + Sync,
        >,
    >,
    info: crate::DeviceInfo,
}

impl Default for MockInterface {
    fn default() -> Self {
        Self {
            read_hook: None,
            info: crate::DeviceInfo::Block(crate::BlockInfo {
                block_count: 100,
                device_flags: fblock::DeviceFlag::empty(),
                max_transfer_blocks: NonZero::new(u32::MAX),
            }),
        }
    }
}

impl Interface for MockInterface {
    async fn on_attach_vmo(&self, _vmo: &zx::Vmo) -> Result<(), zx::Status> {
        Ok(())
    }

    fn get_info(&self) -> Cow<'_, crate::DeviceInfo> {
        Cow::Borrowed(&self.info)
    }

    async fn read(
        &self,
        device_block_offset: u64,
        block_count: u32,
        vmo: &Arc<zx::Vmo>,
        vmo_offset: u64,
        _opts: ReadOptions,
        _trace_flow_id: TraceFlowId,
    ) -> Result<(), zx::Status> {
        if let Some(hook) = &self.read_hook {
            hook(device_block_offset, block_count, vmo, vmo_offset).await
        } else {
            Ok(())
        }
    }

    async fn write(
        &self,
        _device_block_offset: u64,
        _block_count: u32,
        _vmo: &Arc<zx::Vmo>,
        _vmo_offset: u64,
        _write_opts: WriteOptions,
        _trace_flow_id: TraceFlowId,
    ) -> Result<(), zx::Status> {
        Ok(())
    }

    async fn flush(&self, _trace_flow_id: TraceFlowId) -> Result<(), zx::Status> {
        Ok(())
    }

    async fn trim(
        &self,
        _device_block_offset: u64,
        _block_count: u32,
        _trace_flow_id: TraceFlowId,
    ) -> Result<(), zx::Status> {
        Ok(())
    }
}

fn compress(data: &[u8]) -> Vec<u8> {
    zstd::bulk::Compressor::new(0).unwrap().compress(data).unwrap()
}

struct TestFixture {
    _session_proxy: fblock::SessionProxy,
    vmo: zx::Vmo,
    vmoid: fblock::VmoId,
    fifo: fasync::Fifo<BlockFifoResponse, BlockFifoRequest>,
}

impl TestFixture {
    async fn new(
        mock_interface: MockInterface,
        mapping: Option<fblock::BlockOffsetMapping>,
        vmo_size: u64,
    ) -> Self {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<BlockMarker>();

        fasync::Task::spawn(async move {
            let block_server = BlockServer::new(BLOCK_SIZE, Arc::new(mock_interface));
            block_server.handle_requests(stream).await.unwrap();
        })
        .detach();

        let (session_proxy, server) = fidl::endpoints::create_proxy();
        if let Some(mapping) = mapping {
            proxy.open_session_with_options(server, &[mapping]).unwrap();
        } else {
            proxy.open_session(server).unwrap();
        }

        let vmo = zx::Vmo::create(vmo_size).unwrap();
        let vmoid = session_proxy
            .attach_vmo(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
            .await
            .unwrap()
            .unwrap();

        let fifo = fasync::Fifo::from_fifo(session_proxy.get_fifo().await.unwrap().unwrap());

        Self { _session_proxy: session_proxy, vmo, vmoid, fifo }
    }

    async fn send_requests(&mut self, requests: &[BlockFifoRequest]) {
        self.fifo.async_io().1.write_entries(requests).await.unwrap();
    }

    async fn read_response(&mut self) -> BlockFifoResponse {
        let mut response = BlockFifoResponse::default();
        self.fifo.async_io().0.read_entries(&mut response).await.unwrap();
        response
    }

    async fn read_response_with_timeout(
        &mut self,
        timeout: zx::MonotonicDuration,
    ) -> Option<BlockFifoResponse> {
        let mut response = BlockFifoResponse::default();
        match self
            .fifo
            .async_io()
            .0
            .read_entries(&mut response)
            .on_timeout(fasync::MonotonicInstant::after(timeout), || Err(zx::Status::TIMED_OUT))
            .await
        {
            Ok(_) => Some(response),
            Err(_) => None,
        }
    }
}

#[derive(Clone)]
struct TestData {
    uncompressed_data: Vec<u8>,

    // The compressed data will include the padding and be a multiple of the BLOCK_SIZE.
    compressed_data: Vec<u8>,

    // The number of bytes excluding padding.
    total_compressed_bytes: u32,

    // The padding at the beginning.
    compressed_prefix_bytes: u16,
}

impl TestData {
    fn new(size: usize) -> Self {
        const PADDING: usize = 23;

        let (uncompressed_data, unaligned_compressed_data) = loop {
            let mut uncompressed_data = vec![0; size];
            rand::rng().fill(&mut uncompressed_data[..]);
            let compressed_data = compress(&uncompressed_data);

            // We want the compressed data to be within a block in size compared to the uncompressed
            // data. It's random so we should check.
            if compressed_data.len() + PADDING > size
                && compressed_data.len() + PADDING <= size + BLOCK_SIZE as usize
            {
                break (uncompressed_data, compressed_data);
            }
        };

        let total_compressed_bytes = unaligned_compressed_data.len() as u32;

        let mut compressed_data = vec![0; PADDING];
        compressed_data.extend(unaligned_compressed_data);
        compressed_data.resize(compressed_data.len().next_multiple_of(BLOCK_SIZE as usize), 0);

        Self {
            uncompressed_data,
            compressed_data,
            total_compressed_bytes,
            compressed_prefix_bytes: PADDING as u16,
        }
    }

    fn total_compressed_blocks(&self) -> u32 {
        self.compressed_data.len() as u32 / BLOCK_SIZE
    }
}

#[fuchsia::test]
async fn test_decompression() {
    let test_data = TestData::new(BLOCK_SIZE as usize);
    let read_count = Arc::new(AtomicU32::new(0));
    let read_count_clone = read_count.clone();
    let compressed_data = test_data.compressed_data.clone();

    let mut fixture = TestFixture::new(
        MockInterface {
            read_hook: Some(Box::new(move |device_block_offset, block_count, vmo, vmo_offset| {
                let compressed_data = compressed_data.clone();
                let read_count = read_count_clone.clone();
                let vmo = vmo.clone();
                Box::pin(async move {
                    let offset = device_block_offset as usize * BLOCK_SIZE as usize;
                    let len = block_count as usize * BLOCK_SIZE as usize;
                    vmo.write(&compressed_data[offset..offset + len], vmo_offset).unwrap();
                    read_count.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                })
            })),
            ..MockInterface::default()
        },
        None,
        zx::system_get_page_size() as u64,
    )
    .await;

    fixture
        .send_requests(&[BlockFifoRequest {
            command: BlockFifoCommand {
                opcode: BlockOpcode::Read.into_primitive(),
                flags: BlockIoFlag::DECOMPRESS_WITH_ZSTD.bits(),
                ..Default::default()
            },
            vmoid: fixture.vmoid.id,
            length: test_data.total_compressed_blocks(),
            vmo_offset: 0,
            dev_offset: 0,
            uncompressed_bytes: BLOCK_SIZE,
            total_compressed_bytes: test_data.total_compressed_bytes,
            compressed_prefix_bytes: test_data.compressed_prefix_bytes,
            ..Default::default()
        }])
        .await;

    assert_eq!(fixture.read_response().await.status, zx::sys::ZX_OK);

    let mut buf = vec![0; BLOCK_SIZE as usize];
    fixture.vmo.read(&mut buf, 0).unwrap();
    assert_eq!(buf, test_data.uncompressed_data);

    assert_eq!(read_count.load(Ordering::Relaxed), 1);
}

#[fuchsia::test]
async fn test_fragmented_fifo_requests() {
    let test_data = TestData::new(2 * BLOCK_SIZE as usize);
    let read_count = Arc::new(AtomicU32::new(0));
    let read_count_clone = read_count.clone();
    let compressed_data = test_data.compressed_data.clone();

    let mut fixture = TestFixture::new(
        MockInterface {
            read_hook: Some(Box::new(move |device_block_offset, block_count, vmo, vmo_offset| {
                let compressed_data = compressed_data.clone();
                let read_count = read_count_clone.clone();
                let vmo = vmo.clone();
                Box::pin(async move {
                    let offset = device_block_offset as usize * BLOCK_SIZE as usize;
                    let len = block_count as usize * BLOCK_SIZE as usize;
                    vmo.write(&compressed_data[offset..offset + len], vmo_offset).unwrap();
                    read_count.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                })
            })),
            ..MockInterface::default()
        },
        None,
        zx::system_get_page_size() as u64,
    )
    .await;

    // Split the request into two.
    let total_blocks = test_data.total_compressed_blocks();
    let length1 = total_blocks / 2;
    let length2 = total_blocks - length1;

    fixture
        .send_requests(&[
            BlockFifoRequest {
                command: BlockFifoCommand {
                    opcode: BlockOpcode::Read.into_primitive(),
                    flags: (BlockIoFlag::DECOMPRESS_WITH_ZSTD | BlockIoFlag::GROUP_ITEM).bits(),
                    ..Default::default()
                },
                vmoid: fixture.vmoid.id,
                length: length1,
                vmo_offset: 0,
                dev_offset: 0,
                uncompressed_bytes: 2 * BLOCK_SIZE,
                total_compressed_bytes: test_data.total_compressed_bytes,
                compressed_prefix_bytes: test_data.compressed_prefix_bytes,
                group: 1,
                ..Default::default()
            },
            BlockFifoRequest {
                command: BlockFifoCommand {
                    opcode: BlockOpcode::Read.into_primitive(),
                    flags: (BlockIoFlag::DECOMPRESS_WITH_ZSTD
                        | BlockIoFlag::GROUP_ITEM
                        | BlockIoFlag::GROUP_LAST)
                        .bits(),
                    ..Default::default()
                },
                vmoid: fixture.vmoid.id,
                length: length2,
                vmo_offset: 0,
                dev_offset: length1 as u64,
                group: 1,
                ..Default::default()
            },
        ])
        .await;

    assert_eq!(fixture.read_response().await.status, zx::sys::ZX_OK);

    let mut buf = vec![0; 2 * BLOCK_SIZE as usize];
    fixture.vmo.read(&mut buf, 0).unwrap();
    assert_eq!(buf, test_data.uncompressed_data);

    assert_eq!(read_count.load(Ordering::Relaxed), 2);
}

#[fuchsia::test]
async fn test_fragmented_device_reads() {
    let test_data = TestData::new(2 * BLOCK_SIZE as usize);
    let read_count = Arc::new(AtomicU32::new(0));
    let read_count_clone = read_count.clone();
    let compressed_data = test_data.compressed_data.clone();

    // Split the device read into two.
    let total_blocks = test_data.total_compressed_blocks();
    let len1 = total_blocks / 2;

    let mut fixture = TestFixture::new(
        MockInterface {
            info: crate::DeviceInfo::Block(crate::BlockInfo {
                block_count: 1000,
                device_flags: fblock::DeviceFlag::empty(),
                max_transfer_blocks: NonZero::new(len1),
            }),
            read_hook: Some(Box::new(move |device_block_offset, block_count, vmo, vmo_offset| {
                let compressed_data = compressed_data.clone();
                let read_count = read_count_clone.clone();
                let vmo = vmo.clone();
                Box::pin(async move {
                    let offset = device_block_offset as usize * BLOCK_SIZE as usize;
                    let length = block_count as usize * BLOCK_SIZE as usize;
                    let data = if offset >= compressed_data.len() {
                        vec![0; length]
                    } else if offset + length > compressed_data.len() {
                        let mut data = compressed_data[offset..].to_vec();
                        data.resize(length, 0);
                        data
                    } else {
                        compressed_data[offset..offset + length].to_vec()
                    };
                    vmo.write(&data, vmo_offset).unwrap();
                    read_count.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                })
            })),
            ..MockInterface::default()
        },
        Some(fblock::BlockOffsetMapping { target_block_offset: 0, length: 100 }),
        zx::system_get_page_size() as u64,
    )
    .await;

    // Send the request, but the device will read it in two chunks.
    fixture
        .send_requests(&[BlockFifoRequest {
            command: BlockFifoCommand {
                opcode: BlockOpcode::Read.into_primitive(),
                flags: BlockIoFlag::DECOMPRESS_WITH_ZSTD.bits(),
                ..Default::default()
            },
            vmoid: fixture.vmoid.id,
            length: total_blocks,
            vmo_offset: 0,
            dev_offset: 0,
            uncompressed_bytes: 2 * BLOCK_SIZE,
            total_compressed_bytes: test_data.total_compressed_bytes,
            compressed_prefix_bytes: test_data.compressed_prefix_bytes,
            ..Default::default()
        }])
        .await;

    assert_eq!(fixture.read_response().await.status, zx::sys::ZX_OK);

    let mut buf = vec![0; 2 * BLOCK_SIZE as usize];
    fixture.vmo.read(&mut buf, 0).unwrap();
    assert_eq!(buf, test_data.uncompressed_data);

    // The block server should have issued two reads.
    assert_eq!(read_count.load(Ordering::Relaxed), total_blocks.div_ceil(len1));
}

#[fuchsia::test]
async fn test_invalid_decompression_on_write() {
    let mut fixture =
        TestFixture::new(MockInterface::default(), None, zx::system_get_page_size() as u64).await;
    fixture
        .send_requests(&[BlockFifoRequest {
            command: BlockFifoCommand {
                opcode: BlockOpcode::Write.into_primitive(),
                flags: BlockIoFlag::DECOMPRESS_WITH_ZSTD.bits(),
                ..Default::default()
            },
            vmoid: fixture.vmoid.id,
            length: 1,
            ..Default::default()
        }])
        .await;

    assert_eq!(zx::Status::ok(fixture.read_response().await.status), Err(zx::Status::INVALID_ARGS));
}

#[fuchsia::test]
async fn test_invalid_parameters_in_subsequent_request() {
    let mut fixture =
        TestFixture::new(MockInterface::default(), None, zx::system_get_page_size() as u64).await;

    fixture
        .send_requests(&[
            BlockFifoRequest {
                command: BlockFifoCommand {
                    opcode: BlockOpcode::Read.into_primitive(),
                    flags: (BlockIoFlag::DECOMPRESS_WITH_ZSTD | BlockIoFlag::GROUP_ITEM).bits(),
                    ..Default::default()
                },
                vmoid: fixture.vmoid.id,
                length: 1,
                uncompressed_bytes: BLOCK_SIZE,
                total_compressed_bytes: 2 * BLOCK_SIZE,
                group: 1,
                ..Default::default()
            },
            BlockFifoRequest {
                command: BlockFifoCommand {
                    opcode: BlockOpcode::Read.into_primitive(),
                    flags: (BlockIoFlag::GROUP_ITEM | BlockIoFlag::GROUP_LAST).bits(),
                    ..Default::default()
                },
                vmoid: fixture.vmoid.id,
                length: 1,
                // Invalid: these should be 0 in subsequent requests
                uncompressed_bytes: BLOCK_SIZE,
                total_compressed_bytes: 2 * BLOCK_SIZE,
                group: 1,
                ..Default::default()
            },
        ])
        .await;

    // The first request is processed, but the group fails on the second.
    assert_eq!(zx::Status::ok(fixture.read_response().await.status), Err(zx::Status::INVALID_ARGS));
}

#[fuchsia::test]
async fn test_invalid_compressed_size() {
    let mut fixture =
        TestFixture::new(MockInterface::default(), None, zx::system_get_page_size() as u64).await;

    // compressed_prefix_bytes + total_compressed_bytes >= length * block_size
    fixture
        .send_requests(&[BlockFifoRequest {
            command: BlockFifoCommand {
                opcode: BlockOpcode::Read.into_primitive(),
                flags: BlockIoFlag::DECOMPRESS_WITH_ZSTD.bits(),
                ..Default::default()
            },
            vmoid: fixture.vmoid.id,
            length: 1,
            uncompressed_bytes: BLOCK_SIZE,
            total_compressed_bytes: BLOCK_SIZE,
            compressed_prefix_bytes: 1,
            ..Default::default()
        }])
        .await;

    assert_eq!(zx::Status::ok(fixture.read_response().await.status), Err(zx::Status::INVALID_ARGS));
}

#[fuchsia::test]
async fn test_length_greater_than_compressed_blocks() {
    let mut fixture =
        TestFixture::new(MockInterface::default(), None, zx::system_get_page_size() as u64).await;

    fixture
        .send_requests(&[
            BlockFifoRequest {
                command: BlockFifoCommand {
                    opcode: BlockOpcode::Read.into_primitive(),
                    flags: (BlockIoFlag::DECOMPRESS_WITH_ZSTD | BlockIoFlag::GROUP_ITEM).bits(),
                    ..Default::default()
                },
                vmoid: fixture.vmoid.id,
                length: 3, // Greater than compressed blocks.
                uncompressed_bytes: BLOCK_SIZE,
                total_compressed_bytes: 2 * BLOCK_SIZE,
                ..Default::default()
            },
            BlockFifoRequest {
                command: BlockFifoCommand {
                    opcode: BlockOpcode::Read.into_primitive(),
                    flags: (BlockIoFlag::DECOMPRESS_WITH_ZSTD
                        | BlockIoFlag::GROUP_ITEM
                        | BlockIoFlag::GROUP_LAST)
                        .bits(),
                    ..Default::default()
                },
                vmoid: fixture.vmoid.id,
                length: 1,
                ..Default::default()
            },
        ])
        .await;

    assert_eq!(zx::Status::ok(fixture.read_response().await.status), Err(zx::Status::INVALID_ARGS));
}

#[fuchsia::test]
async fn test_group_length_less_than_compressed_blocks() {
    let mut fixture =
        TestFixture::new(MockInterface::default(), None, zx::system_get_page_size() as u64).await;

    fixture
        .send_requests(&[BlockFifoRequest {
            command: BlockFifoCommand {
                opcode: BlockOpcode::Read.into_primitive(),
                flags: BlockIoFlag::DECOMPRESS_WITH_ZSTD.bits(),
                ..Default::default()
            },
            vmoid: fixture.vmoid.id,
            length: 1, // Less than total_compressed_blocks
            uncompressed_bytes: BLOCK_SIZE,
            total_compressed_bytes: 2 * BLOCK_SIZE,
            ..Default::default()
        }])
        .await;

    assert_eq!(zx::Status::ok(fixture.read_response().await.status), Err(zx::Status::INVALID_ARGS));
}

#[fuchsia::test]
async fn test_too_much_decompression() {
    let mut fixture =
        TestFixture::new(MockInterface::default(), None, zx::system_get_page_size() as u64).await;

    let total_compressed_bytes = fblock::MAX_DECOMPRESSED_BYTES as u32 + 1;

    fixture
        .send_requests(&[BlockFifoRequest {
            command: BlockFifoCommand {
                opcode: BlockOpcode::Read.into_primitive(),
                flags: BlockIoFlag::DECOMPRESS_WITH_ZSTD.bits(),
                ..Default::default()
            },
            vmoid: fixture.vmoid.id,
            length: total_compressed_bytes.div_ceil(BLOCK_SIZE),
            uncompressed_bytes: BLOCK_SIZE,
            total_compressed_bytes,
            ..Default::default()
        }])
        .await;

    assert_eq!(zx::Status::ok(fixture.read_response().await.status), Err(zx::Status::OUT_OF_RANGE));
}

#[fuchsia::test]
async fn test_decompression_buffer_exhaustion() {
    const NUM_REQUESTS: usize = 16;
    const UNCOMPRESSED_SIZE: usize = 9 * 1024 * 1024;
    const VMO_SIZE: u64 = (NUM_REQUESTS * UNCOMPRESSED_SIZE) as u64;

    // The buffer allocator uses a Buddy allocator, and we're allocating chunks of 9 MiB, so that
    // gets rounded to 16 MiB.
    const UNBLOCKED_REQUESTS: usize = fblock::MAX_DECOMPRESSED_BYTES as usize / (16 * 1024 * 1024);

    // Create two sets of test_data and alternate them.
    let mut test_data = Vec::new();
    for _ in 0..2 {
        test_data.push(TestData::new(UNCOMPRESSED_SIZE));
    }
    let compressed_blocks = test_data[0].total_compressed_blocks();
    assert_eq!(test_data[1].total_compressed_blocks(), compressed_blocks);
    let test_data = Arc::new(test_data);

    let read_count = Arc::new(AtomicU32::new(0));
    let read_count_clone = read_count.clone();
    let test_data_clone = test_data.clone();

    // Channel to control when reads complete.
    let (tx, mut rx) = mpsc::unbounded();

    let mut fixture = TestFixture::new(
        MockInterface {
            info: crate::DeviceInfo::Block(crate::BlockInfo {
                block_count: u32::MAX as u64,
                device_flags: fblock::DeviceFlag::empty(),
                max_transfer_blocks: NonZero::new(u32::MAX),
            }),
            read_hook: Some(Box::new(move |device_block_offset, block_count, vmo, vmo_offset| {
                let (sender, receiver) = oneshot::channel();
                tx.unbounded_send(sender).unwrap();

                let test_data = test_data_clone.clone();
                let read_count = read_count_clone.clone();
                let vmo = vmo.clone();
                Box::pin(async move {
                    let _ = receiver.await;

                    let request_index = device_block_offset as usize / compressed_blocks as usize;
                    let data = &test_data[request_index % 2];

                    let offset = (device_block_offset as usize * BLOCK_SIZE as usize)
                        % data.compressed_data.len();
                    let len = block_count as usize * BLOCK_SIZE as usize;

                    vmo.write(&data.compressed_data[offset..offset + len], vmo_offset).unwrap();
                    read_count.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                })
            })),
            ..MockInterface::default()
        },
        None,
        VMO_SIZE,
    )
    .await;

    let mut requests = Vec::new();
    let mut dev_offset = 0;
    for i in 0..NUM_REQUESTS {
        let data = &test_data[i % 2];
        requests.push(BlockFifoRequest {
            command: BlockFifoCommand {
                opcode: BlockOpcode::Read.into_primitive(),
                flags: BlockIoFlag::DECOMPRESS_WITH_ZSTD.bits(),
                ..Default::default()
            },
            vmoid: fixture.vmoid.id,
            reqid: i as u32,
            length: compressed_blocks,
            vmo_offset: (i * UNCOMPRESSED_SIZE / BLOCK_SIZE as usize) as u64,
            dev_offset,
            uncompressed_bytes: UNCOMPRESSED_SIZE as u32,
            total_compressed_bytes: data.total_compressed_bytes,
            compressed_prefix_bytes: data.compressed_prefix_bytes,
            ..Default::default()
        });
        dev_offset += compressed_blocks as u64;
    }

    fixture.send_requests(&requests).await;

    // Verify no responses yet.
    assert!(
        fixture.read_response_with_timeout(zx::MonotonicDuration::from_millis(100)).await.is_none()
    );

    let mut requests = Vec::new();
    for _ in 0..UNBLOCKED_REQUESTS {
        requests.push(rx.next().await.unwrap());
    }

    // Make sure we're blocked.
    fasync::Timer::new(std::time::Duration::from_millis(100)).await;
    assert!(rx.try_next().is_err());

    requests.shuffle(&mut rand::rng());

    for _ in UNBLOCKED_REQUESTS..NUM_REQUESTS {
        // Complete one request.
        requests.pop().unwrap().send(()).unwrap();

        // That should free up another one.
        requests.push(rx.next().await.unwrap());
    }

    // Complete all the requests now.
    for request in requests {
        request.send(()).unwrap();
    }

    // Read all the responses.
    for _ in 0..NUM_REQUESTS {
        let response = fixture.read_response().await;
        assert_eq!(response.status, zx::sys::ZX_OK);
    }

    let mut buf = vec![0; UNCOMPRESSED_SIZE];
    for i in 0..NUM_REQUESTS {
        fixture.vmo.read(&mut buf, (i * UNCOMPRESSED_SIZE) as u64).unwrap();
        assert_eq!(buf, test_data[i % 2].uncompressed_data);
    }

    assert_eq!(read_count.load(Ordering::Relaxed), NUM_REQUESTS as u32);
}

#[fuchsia::test]
async fn test_invalid_vmo_offset() {
    let mut fixture =
        TestFixture::new(MockInterface::default(), None, zx::system_get_page_size() as u64).await;

    fixture
        .send_requests(&[BlockFifoRequest {
            command: BlockFifoCommand {
                opcode: BlockOpcode::Read.into_primitive(),
                flags: BlockIoFlag::DECOMPRESS_WITH_ZSTD.bits(),
                ..Default::default()
            },
            vmoid: fixture.vmoid.id,
            length: 1,
            vmo_offset: zx::system_get_page_size() as u64 / BLOCK_SIZE as u64 + 1,
            uncompressed_bytes: BLOCK_SIZE,
            total_compressed_bytes: BLOCK_SIZE,
            ..Default::default()
        }])
        .await;

    assert_eq!(zx::Status::ok(fixture.read_response().await.status), Err(zx::Status::OUT_OF_RANGE));
}

#[fuchsia::test]
async fn test_invalid_uncompressed_bytes() {
    let mut fixture =
        TestFixture::new(MockInterface::default(), None, zx::system_get_page_size() as u64).await;

    fixture
        .send_requests(&[BlockFifoRequest {
            command: BlockFifoCommand {
                opcode: BlockOpcode::Read.into_primitive(),
                flags: BlockIoFlag::DECOMPRESS_WITH_ZSTD.bits(),
                ..Default::default()
            },
            vmoid: fixture.vmoid.id,
            length: 1,
            vmo_offset: 0,
            uncompressed_bytes: zx::system_get_page_size() + 1,
            total_compressed_bytes: BLOCK_SIZE,
            ..Default::default()
        }])
        .await;

    assert_eq!(zx::Status::ok(fixture.read_response().await.status), Err(zx::Status::OUT_OF_RANGE));
}

#[fuchsia::test]
async fn test_alignment() {
    let (tx, mut rx) = mpsc::unbounded();

    let mut fixture = TestFixture::new(
        MockInterface {
            read_hook: Some(Box::new(move |_, _, _, _| {
                let (sender, receiver) = oneshot::channel();
                tx.unbounded_send(sender).unwrap();

                Box::pin(async move {
                    let () = receiver.await.unwrap();
                    Ok(())
                })
            })),
            ..MockInterface::default()
        },
        None,
        zx::system_get_page_size() as u64,
    )
    .await;

    let test_data = TestData::new(1 as usize);

    fixture
        .send_requests(&[
            BlockFifoRequest {
                command: BlockFifoCommand {
                    opcode: BlockOpcode::Read.into_primitive(),
                    flags: BlockIoFlag::DECOMPRESS_WITH_ZSTD.bits(),
                    ..Default::default()
                },
                vmoid: fixture.vmoid.id,
                length: test_data.total_compressed_blocks(),
                vmo_offset: 0,
                uncompressed_bytes: test_data.uncompressed_data.len() as u32,
                total_compressed_bytes: test_data.total_compressed_bytes,
                compressed_prefix_bytes: test_data.compressed_prefix_bytes,
                ..Default::default()
            },
            BlockFifoRequest {
                command: BlockFifoCommand {
                    opcode: BlockOpcode::Read.into_primitive(),
                    flags: BlockIoFlag::DECOMPRESS_WITH_ZSTD.bits(),
                    ..Default::default()
                },
                reqid: 1,
                vmoid: fixture.vmoid.id,
                length: test_data.total_compressed_blocks(),
                vmo_offset: 1,
                uncompressed_bytes: test_data.uncompressed_data.len() as u32,
                total_compressed_bytes: test_data.total_compressed_bytes,
                compressed_prefix_bytes: test_data.compressed_prefix_bytes,
                ..Default::default()
            },
        ])
        .await;

    rx.next().await.unwrap();
    rx.next().await.unwrap();
}
