// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use block_client::{BlockClient as _, BufferSlice, MutableBufferSlice};
use block_protocol::{BlockFifoCommand, BlockFifoRequest, BlockFifoResponse};
use block_server::{BlockInfo, DeviceInfo};
use fidl_fuchsia_hardware_ramdisk as framdisk;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_storage_block as fblock;
use fidl_fuchsia_storage_block::{BlockIoFlag, BlockOpcode};
use fs_management::format::constants::FVM_PARTITION_LABEL;
use fuchsia_async as fasync;
use std::num::NonZero;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use test_case::test_case;
use test_vmo_backed_block_server::{InitialContents, Observer, VmoBackedServerOptions, WriteCache};

// Make the block device big enough so that we can have a request which creates more than
// block_server::MAX_REQUESTS.
const MAX_TRANSFER_BLOCKS: u32 = 10;
const NUM_BLOCKS: u64 = 10_000;
const BLOCK_SIZE: u32 = 512;
// The FIFO server can handle up to 64 simultaneous requests, so do one more than that to
// exercise handling really big requests.
const REQ_BLOCKS: usize = 65 * MAX_TRANSFER_BLOCKS as usize;
const REQ_SIZE: u64 = REQ_BLOCKS as u64 * BLOCK_SIZE as u64;

async fn test_request_splitting_client_fn(
    proxy: fblock::BlockProxy,
    last_trim_length_fn: Option<Box<dyn Fn() -> u32>>,
) {
    let bs = BLOCK_SIZE as usize;

    let (session_proxy, server) = fidl::endpoints::create_proxy();

    proxy.open_session(server).unwrap();

    let vmo = zx::Vmo::create(REQ_SIZE).unwrap();
    let vmo_id = session_proxy
        .attach_vmo(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
        .await
        .unwrap()
        .unwrap();

    let mut fifo = fasync::Fifo::from_fifo(session_proxy.get_fifo().await.unwrap().unwrap());
    let (mut reader, mut writer) = fifo.async_io();

    // Fill in a predictable pattern so we can detect reading/writing to the correct blocks.
    for i in 0..REQ_BLOCKS {
        vmo.write(&[i as u8], (i * bs) as u64).expect("vmo write failed");
    }

    writer
        .write_entries(&BlockFifoRequest {
            command: BlockFifoCommand {
                opcode: BlockOpcode::Write.into_primitive(),
                ..Default::default()
            },
            vmoid: vmo_id.id,
            dev_offset: 10,
            length: REQ_BLOCKS as u32,
            vmo_offset: 0,
            ..Default::default()
        })
        .await
        .unwrap();
    let mut response = BlockFifoResponse::default();
    reader.read_entries(&mut response).await.unwrap();
    assert_eq!(response.status, zx::sys::ZX_OK);

    for i in 0..REQ_BLOCKS {
        vmo.write(&[0 as u8], (i * bs) as u64).expect("vmo write failed");
    }

    writer
        .write_entries(&BlockFifoRequest {
            command: BlockFifoCommand {
                opcode: BlockOpcode::Read.into_primitive(),
                ..Default::default()
            },
            vmoid: vmo_id.id,
            dev_offset: 10,
            length: REQ_BLOCKS as u32,
            vmo_offset: 0,
            ..Default::default()
        })
        .await
        .unwrap();
    let mut response = BlockFifoResponse::default();
    reader.read_entries(&mut response).await.unwrap();
    assert_eq!(response.status, zx::sys::ZX_OK);

    for i in 0..REQ_BLOCKS {
        let mut buf = [0u8];
        vmo.read(&mut buf, (i * bs) as u64).expect("vmo read failed");
        assert_eq!(buf[0], i as u8);
    }

    if let Some(last_trim_length_fn) = last_trim_length_fn.as_ref() {
        assert_eq!(0, last_trim_length_fn());
        writer
            .write_entries(&BlockFifoRequest {
                command: BlockFifoCommand {
                    opcode: BlockOpcode::Trim.into_primitive(),
                    ..Default::default()
                },
                group: 1,
                dev_offset: 0,
                length: 400,
                ..Default::default()
            })
            .await
            .unwrap();
        let mut response = BlockFifoResponse::default();
        reader.read_entries(&mut response).await.unwrap();
        assert_eq!(response.status, zx::sys::ZX_OK);
        assert_eq!(400, last_trim_length_fn());
    }

    // OK, now put several big requests in a group and make sure that works.
    writer
        .write_entries(&BlockFifoRequest {
            command: BlockFifoCommand {
                opcode: BlockOpcode::Write.into_primitive(),
                flags: BlockIoFlag::GROUP_ITEM.bits(),
                ..Default::default()
            },
            group: 1,
            vmoid: vmo_id.id,
            dev_offset: 0,
            length: 1,
            vmo_offset: 0,
            ..Default::default()
        })
        .await
        .unwrap();
    writer
        .write_entries(&BlockFifoRequest {
            command: BlockFifoCommand {
                opcode: BlockOpcode::Read.into_primitive(),
                flags: BlockIoFlag::GROUP_ITEM.bits(),
                ..Default::default()
            },
            group: 1,
            vmoid: vmo_id.id,
            dev_offset: 0,
            length: (REQ_BLOCKS / 2) as u32,
            vmo_offset: 0,
            ..Default::default()
        })
        .await
        .unwrap();
    writer
        .write_entries(&BlockFifoRequest {
            command: BlockFifoCommand {
                opcode: BlockOpcode::Read.into_primitive(),
                flags: (BlockIoFlag::GROUP_ITEM | BlockIoFlag::GROUP_LAST).bits(),
                ..Default::default()
            },
            group: 1,
            vmoid: vmo_id.id,
            dev_offset: REQ_BLOCKS as u64 / 2,
            length: REQ_BLOCKS as u32 / 2,
            vmo_offset: REQ_BLOCKS as u64 / 2,
            ..Default::default()
        })
        .await
        .unwrap();
    let mut response = BlockFifoResponse::default();
    reader.read_entries(&mut response).await.unwrap();
    assert_eq!(response.status, zx::sys::ZX_OK);
    assert_eq!(response.group, 1);
}

#[fuchsia::test]
async fn test_request_splitting_rust_server() {
    let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fblock::BlockMarker>();

    // Records the length of the last trim command.
    struct TrimObserver(Arc<AtomicU32>);
    impl Observer for TrimObserver {
        fn trim(&self, _device_block_offset: u64, block_count: u32) {
            self.0.store(block_count, Ordering::Relaxed)
        }
    }

    let last_trim_length = Arc::new(AtomicU32::new(0));
    let last_trim_length_clone = last_trim_length.clone();

    let server = async {
        let block_server = VmoBackedServerOptions {
            initial_contents: InitialContents::FromCapacity(NUM_BLOCKS),
            block_size: BLOCK_SIZE,
            info: DeviceInfo::Block(BlockInfo {
                max_transfer_blocks: NonZero::new(MAX_TRANSFER_BLOCKS),
                ..Default::default()
            }),
            observer: Some(Box::new(TrimObserver(last_trim_length_clone))),
            ..Default::default()
        }
        .build()
        .unwrap();
        block_server.serve(stream).await.unwrap();
    };

    let client = test_request_splitting_client_fn(
        proxy,
        Some(Box::new(move || last_trim_length.load(Ordering::Relaxed))),
    );

    futures::join!(server, client);
}

#[fuchsia::test]
async fn test_request_splitting_cpp_server() {
    let (proxy, server) = fidl::endpoints::create_proxy::<fblock::BlockMarker>();

    let ramdisk = ramdevice_client::RamdiskClientBuilder::new(BLOCK_SIZE as u64, NUM_BLOCKS)
        .max_transfer_blocks(MAX_TRANSFER_BLOCKS)
        .build()
        .await
        .expect("Failed to create ramdisk");
    ramdisk.connect(server.into_channel().into()).expect("Failed to connect to ramdisk");

    test_request_splitting_client_fn(proxy, None).await;
}

#[fuchsia::test]
async fn test_group_with_close() {
    let (proxy, server) = fidl::endpoints::create_proxy::<fblock::BlockMarker>();

    let ramdisk = ramdevice_client::RamdiskClientBuilder::new(BLOCK_SIZE as u64, NUM_BLOCKS)
        .build()
        .await
        .expect("Failed to create ramdisk");
    ramdisk.connect(server.into_channel().into()).expect("Failed to connect to ramdisk");

    let (session_proxy, server) = fidl::endpoints::create_proxy();

    proxy.open_session(server).unwrap();

    let vmo1 = zx::Vmo::create(zx::system_get_page_size() as u64).unwrap();
    let vmo_id1 = session_proxy
        .attach_vmo(vmo1.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
        .await
        .unwrap()
        .unwrap();

    let vmo2 = zx::Vmo::create(zx::system_get_page_size() as u64).unwrap();
    let vmo_id2 = session_proxy
        .attach_vmo(vmo2.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
        .await
        .unwrap()
        .unwrap();

    let mut fifo = fasync::Fifo::<_, BlockFifoRequest>::from_fifo(
        session_proxy.get_fifo().await.unwrap().unwrap(),
    );
    let (mut reader, mut writer) = fifo.async_io();

    writer
        .write_entries(&[
            BlockFifoRequest {
                command: BlockFifoCommand {
                    opcode: BlockOpcode::Read.into_primitive(),
                    flags: BlockIoFlag::GROUP_ITEM.bits(),
                    ..Default::default()
                },
                group: 7,
                vmoid: vmo_id1.id,
                length: 1,
                ..Default::default()
            },
            BlockFifoRequest {
                command: BlockFifoCommand {
                    opcode: BlockOpcode::Read.into_primitive(),
                    flags: (BlockIoFlag::GROUP_ITEM | BlockIoFlag::GROUP_LAST).bits(),
                    ..Default::default()
                },
                group: 7,
                reqid: 10,
                vmoid: vmo_id2.id,
                length: 1,
                ..Default::default()
            },
            BlockFifoRequest {
                command: BlockFifoCommand {
                    opcode: BlockOpcode::CloseVmo.into_primitive(),
                    ..Default::default()
                },
                reqid: 11,
                vmoid: vmo_id1.id,
                ..Default::default()
            },
            BlockFifoRequest {
                command: BlockFifoCommand {
                    opcode: BlockOpcode::CloseVmo.into_primitive(),
                    ..Default::default()
                },
                reqid: 12,
                vmoid: vmo_id2.id,
                ..Default::default()
            },
        ])
        .await
        .unwrap();

    let mut responses = 0;
    for _ in 0..3 {
        let mut response = BlockFifoResponse::default();
        reader.read_entries(&mut response).await.unwrap();
        assert_eq!(response.status, zx::sys::ZX_OK);
        assert!(response.reqid >= 10 && response.reqid < 13);
        let bit = 1 << (response.reqid - 10);
        assert_eq!(responses & bit, 0);
        responses |= bit;
    }
}

#[fuchsia::test]
async fn test_gpt_on_ramdisk() {
    let ramdisk = ramdevice_client::RamdiskClientBuilder::new(BLOCK_SIZE as u64, NUM_BLOCKS)
        .max_transfer_blocks(MAX_TRANSFER_BLOCKS)
        .build()
        .await
        .expect("Failed to create ramdisk");

    const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
    const PART1_INSTANCE_GUID: [u8; 16] = [2u8; 16];
    const PART1_NAME: &str = "part";
    const PART2_INSTANCE_GUID: [u8; 16] = [3u8; 16];
    const PART2_NAME: &str = FVM_PARTITION_LABEL;
    {
        let (proxy, server) = fidl::endpoints::create_proxy::<fblock::BlockMarker>();
        ramdisk.connect(server.into_channel().into()).expect("Failed to connect to ramdisk");
        let client = Arc::new(block_client::RemoteBlockClient::new(proxy).await.unwrap());
        gpt::Gpt::format(
            client,
            vec![
                gpt::PartitionInfo {
                    label: PART1_NAME.to_string(),
                    type_guid: gpt::Guid::from_bytes(PART_TYPE_GUID),
                    instance_guid: gpt::Guid::from_bytes(PART1_INSTANCE_GUID),
                    start_block: 3000,
                    num_blocks: 200,
                    flags: 0xabcd,
                },
                gpt::PartitionInfo {
                    label: PART2_NAME.to_string(),
                    type_guid: gpt::Guid::from_bytes(PART_TYPE_GUID),
                    instance_guid: gpt::Guid::from_bytes(PART2_INSTANCE_GUID),
                    start_block: 3200,
                    num_blocks: 400,
                    flags: 0xabcd,
                },
            ],
        )
        .await
        .unwrap();
    }

    let (proxy, server) = fidl::endpoints::create_proxy::<fblock::BlockMarker>();
    ramdisk.connect(server.into_channel().into()).expect("Failed to connect to ramdisk");

    let partitions_dir = vfs::directory::immutable::simple();
    let partitions_dir_clone = partitions_dir.clone();
    let runner = gpt_component::gpt::GptManager::new(proxy, partitions_dir_clone)
        .await
        .expect("load should succeed");

    let part1_dir = vfs::serve_directory(
        partitions_dir.clone(),
        vfs::path::Path::validate_and_split("part-000").unwrap(),
        vfs::execution_scope::ExecutionScope::new(),
        fio::PERM_READABLE,
    );
    let part1_block = fuchsia_component_client::connect_to_named_protocol_at_dir_root::<
        fblock::BlockMarker,
    >(&part1_dir, "volume")
    .expect("Failed to open Volume service");

    let part2_dir = vfs::serve_directory(
        partitions_dir.clone(),
        vfs::path::Path::validate_and_split("part-001").unwrap(),
        vfs::execution_scope::ExecutionScope::new(),
        fio::PERM_READABLE,
    );
    let part2_block = fuchsia_component_client::connect_to_named_protocol_at_dir_root::<
        fblock::BlockMarker,
    >(&part2_dir, "volume")
    .expect("Failed to open Volume service");

    {
        let client1 = block_client::RemoteBlockClient::new(part1_block).await.unwrap();
        let client2 = block_client::RemoteBlockClient::new(part2_block).await.unwrap();
        const BS: usize = BLOCK_SIZE as usize;
        const LEN: u64 = 50 * BS as u64;
        let vmo = zx::Vmo::create(LEN).unwrap();
        // SAFETY: Test code. We attach the same VMO to two clients to test multi-partition I/O.
        // We ensure no concurrent conflicting I/O is performed.
        let vmoid1 = unsafe { client1.attach_vmo(&vmo) }.await.expect("attach_vmo failed");
        let vmoid2 = unsafe { client2.attach_vmo(&vmo) }.await.expect("attach_vmo failed");

        vmo.write(&[0x11u8; LEN as usize], 0).unwrap();
        client1
            .write_at(BufferSlice::new_with_vmo_id(&vmoid1, 0, LEN), BS as u64 * 150)
            .await
            .expect("write failed");
        vmo.write(&[0u8; LEN as usize], 0).unwrap();
        client1
            .read_at(MutableBufferSlice::new_with_vmo_id(&vmoid1, 0, LEN), BS as u64 * 150)
            .await
            .expect("read failed");
        assert_eq!(&vmo.read_to_vec::<u8>(0, LEN).unwrap()[..], &[0x11u8; LEN as usize]);

        vmo.write(&[0x22u8; LEN as usize], 0).unwrap();
        client2
            .write_at(BufferSlice::new_with_vmo_id(&vmoid2, 0, BS as u64), 0)
            .await
            .expect("write failed");
        vmo.write(&[0u8; LEN as usize], 0).unwrap();
        client2
            .read_at(MutableBufferSlice::new_with_vmo_id(&vmoid2, 0, BS as u64), 0)
            .await
            .expect("read failed");
        assert_eq!(&vmo.read_to_vec::<u8>(0, BS as u64).unwrap()[..], &[0x22u8; BS]);

        // Write past end
        vmo.write(&[0x33u8; LEN as usize], 0).unwrap();
        client1
            .write_at(BufferSlice::new_with_vmo_id(&vmoid1, 0, 2 * BS as u64), BS as u64 * 199)
            .await
            .expect_err("write past end should fail");
        // Other partition should be unchanged
        vmo.write(&[0u8; LEN as usize], 0).unwrap();
        client2
            .read_at(MutableBufferSlice::new_with_vmo_id(&vmoid2, 0, BS as u64), 0)
            .await
            .expect("read failed");
        assert_eq!(&vmo.read_to_vec::<u8>(0, BS as u64).unwrap()[..], &[0x22u8; BS]);

        client1.detach_vmo(vmoid1).await.unwrap();
        client2.detach_vmo(vmoid2).await.unwrap();
    }

    runner.shutdown().await;
}

// The test uses a separate ramdisk for an underlying block device, and runs a local GPT instance on
// top of that.  Then, the test uses synchronous interfaces to send a request to the session.  If
// passthrough is enabled, the GPT component will be completely bypassed by requests, and this will
// work.  If passthrough is not enabled, then the test will deadlock (as the sync call will block
// forever, because the test is single-threaded).
// Note that the test must be executed on a single thread to be useful.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PassthroughTestCase {
    SinglePartition,
    ContiguousMergedPartition,
    DiscontiguousMergedPartition,
}

#[test_case(PassthroughTestCase::SinglePartition; "single_partition")]
#[test_case(PassthroughTestCase::ContiguousMergedPartition; "contiguous_merged_partition")]
#[test_case(PassthroughTestCase::DiscontiguousMergedPartition; "discontiguous_merged_partition")]
#[fuchsia::test]
async fn test_gpt_passthrough(case: PassthroughTestCase) {
    let ramdisk = ramdevice_client::RamdiskClientBuilder::new(BLOCK_SIZE as u64, NUM_BLOCKS)
        .max_transfer_blocks(MAX_TRANSFER_BLOCKS)
        .build()
        .await
        .expect("Failed to create ramdisk");

    const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
    const PART_INSTANCE_GUID: [u8; 16] = [2u8; 16];
    let (partitions, merge_super_and_userdata) = match case {
        PassthroughTestCase::SinglePartition => (
            vec![gpt::PartitionInfo {
                label: FVM_PARTITION_LABEL.to_string(),
                type_guid: gpt::Guid::from_bytes(PART_TYPE_GUID),
                instance_guid: gpt::Guid::from_bytes(PART_INSTANCE_GUID),
                start_block: 4,
                num_blocks: 4,
                flags: 0xabcd,
            }],
            false,
        ),
        PassthroughTestCase::ContiguousMergedPartition => (
            vec![
                gpt::PartitionInfo {
                    label: "super".to_string(),
                    type_guid: gpt::Guid::from_bytes(PART_TYPE_GUID),
                    instance_guid: gpt::Guid::from_bytes(PART_INSTANCE_GUID),
                    start_block: 4,
                    num_blocks: 2,
                    flags: 0xabcd,
                },
                gpt::PartitionInfo {
                    label: "userdata".to_string(),
                    type_guid: gpt::Guid::from_bytes(PART_TYPE_GUID),
                    instance_guid: gpt::Guid::from_bytes(PART_INSTANCE_GUID),
                    start_block: 6,
                    num_blocks: 2,
                    flags: 0xabcd,
                },
            ],
            true,
        ),
        PassthroughTestCase::DiscontiguousMergedPartition => (
            vec![
                gpt::PartitionInfo {
                    label: "super".to_string(),
                    type_guid: gpt::Guid::from_bytes(PART_TYPE_GUID),
                    instance_guid: gpt::Guid::from_bytes(PART_INSTANCE_GUID),
                    start_block: 4,
                    num_blocks: 2,
                    flags: 0xabcd,
                },
                gpt::PartitionInfo {
                    label: "userdata".to_string(),
                    type_guid: gpt::Guid::from_bytes(PART_TYPE_GUID),
                    instance_guid: gpt::Guid::from_bytes(PART_INSTANCE_GUID),
                    start_block: 10,
                    num_blocks: 2,
                    flags: 0xabcd,
                },
            ],
            true,
        ),
    };

    {
        let (proxy, server) = fidl::endpoints::create_proxy::<fblock::BlockMarker>();
        ramdisk.connect(server.into_channel().into()).expect("Failed to connect to ramdisk");
        let client = Arc::new(block_client::RemoteBlockClient::new(proxy).await.unwrap());
        gpt::Gpt::format(client, partitions).await.unwrap();
    }

    let (proxy, server) = fidl::endpoints::create_proxy::<fblock::BlockMarker>();
    ramdisk.connect(server.into_channel().into()).expect("Failed to connect to ramdisk");

    let partitions_dir = vfs::directory::immutable::simple();
    let partitions_dir_clone = partitions_dir.clone();
    let runner = gpt_component::gpt::GptManager::new_with_config(
        proxy,
        partitions_dir_clone,
        gpt_component::config::Config { merge_super_and_userdata, ..Default::default() },
    )
    .await
    .expect("load should succeed");

    let part_dir = vfs::serve_directory(
        partitions_dir.clone(),
        vfs::path::Path::validate_and_split("part-000").unwrap(),
        vfs::execution_scope::ExecutionScope::new(),
        fio::PERM_READABLE,
    );
    let part_block = fuchsia_component_client::connect_to_named_protocol_at_dir_root::<
        fblock::BlockMarker,
    >(&part_dir, "volume")
    .expect("Failed to open Volume service");

    // 1. Send synchronous Flush to verify passthrough avoids deadlocking on a single thread.
    {
        let (session_proxy, server) = fidl::endpoints::create_proxy();
        part_block.open_session(server).unwrap();

        let fifo: zx::Fifo<BlockFifoResponse, BlockFifoRequest> =
            session_proxy.get_fifo().await.unwrap().unwrap().into();

        fifo.write(&[BlockFifoRequest {
            command: BlockFifoCommand {
                opcode: BlockOpcode::Flush.into_primitive(),
                ..Default::default()
            },
            ..Default::default()
        }])
        .unwrap();
        loop {
            match fifo.read_one() {
                Ok(response) => {
                    zx::Status::ok(response.status).expect("Flush failed");
                    break;
                }
                Err(zx::Status::SHOULD_WAIT) => {
                    fifo.wait_one(zx::Signals::FIFO_READABLE, zx::MonotonicInstant::INFINITE)
                        .unwrap();
                }
                err => {
                    err.unwrap();
                }
            }
        }
    }

    // 2. Write canary values to the partition and verify the underlying physical blocks.
    {
        let client = block_client::RemoteBlockClient::new(part_block).await.unwrap();
        let canary1 = vec![0xaau8; BLOCK_SIZE as usize];
        let canary2 = vec![0xbbu8; BLOCK_SIZE as usize];

        // Virtual block 0 corresponds to physical block 4 across all cases.
        client
            .write_at(block_client::BufferSlice::Memory(&canary1), 0)
            .await
            .expect("write canary1 failed");

        // Virtual block 2 maps to physical block 6 for Single/Contiguous, or 10 for Discontiguous.
        client
            .write_at(block_client::BufferSlice::Memory(&canary2), 2 * BLOCK_SIZE as u64)
            .await
            .expect("write canary2 failed");

        // Connect directly to the underlying raw ramdisk and verify the physical blocks.
        let (raw_proxy, raw_server) = fidl::endpoints::create_proxy::<fblock::BlockMarker>();
        ramdisk.connect(raw_server.into_channel().into()).expect("Failed to connect to ramdisk");
        let raw_client = block_client::RemoteBlockClient::new(raw_proxy).await.unwrap();

        let mut read_buf = vec![0u8; BLOCK_SIZE as usize];
        let offset1 = 4 * BLOCK_SIZE as u64;
        let offset2 = match case {
            PassthroughTestCase::DiscontiguousMergedPartition => 10 * BLOCK_SIZE as u64,
            _ => 6 * BLOCK_SIZE as u64,
        };

        raw_client
            .read_at(block_client::MutableBufferSlice::Memory(&mut read_buf), offset1)
            .await
            .expect("read first physical block failed");
        assert_eq!(read_buf, canary1);

        raw_client
            .read_at(block_client::MutableBufferSlice::Memory(&mut read_buf), offset2)
            .await
            .expect("read second physical block failed");
        assert_eq!(read_buf, canary2);
    }

    runner.shutdown().await;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BarrierFuaTestCase {
    SimulatedBarrier,
    Barrier,
    SimulatedFua,
    Fua,
}

#[test_case(BarrierFuaTestCase::SimulatedBarrier; "simulated_barrier")]
#[test_case(BarrierFuaTestCase::Barrier; "barrier")]
#[test_case(BarrierFuaTestCase::SimulatedFua; "simulated_fua")]
#[test_case(BarrierFuaTestCase::Fua; "fua")]
#[fuchsia::test]
async fn test_barriers_and_fua_rust_server(case: BarrierFuaTestCase) {
    let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fblock::BlockMarker>();

    /// An Observer that shuffles writes and discards some of the tail since last flush/barrier.
    /// It also verifies the correct number of flushes occurred.
    struct ShufflingObserver(AtomicUsize, Arc<AtomicBool>);
    let closed: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
    let closed_clone = closed.clone();

    impl Drop for ShufflingObserver {
        fn drop(&mut self) {
            assert_eq!(self.0.load(Ordering::Relaxed), 0);
        }
    }

    impl Observer for ShufflingObserver {
        fn flush(&self, _writes: Option<&mut WriteCache>) {
            assert_ne!(self.0.fetch_sub(1, Ordering::Relaxed), 0);
        }
        fn close(&self, writes: Option<&mut WriteCache>) {
            if let Some(writes) = writes {
                writes.shuffle();
                writes.discard_some();
            }
            self.1.store(true, Ordering::Relaxed);
        }
    }

    let device_flags = match case {
        BarrierFuaTestCase::Barrier => fblock::DeviceFlag::BARRIER_SUPPORT,
        BarrierFuaTestCase::Fua => fblock::DeviceFlag::FUA_SUPPORT,
        _ => fblock::DeviceFlag::empty(),
    };
    let expected_flushes = match case {
        BarrierFuaTestCase::SimulatedBarrier | BarrierFuaTestCase::SimulatedFua => 1,
        _ => 0,
    };
    let vmo = zx::Vmo::create(4 * BLOCK_SIZE as u64).unwrap();
    let duplicate_vmo = vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
    let server = fasync::Task::spawn(async move {
        let block_server = VmoBackedServerOptions {
            initial_contents: InitialContents::FromVmo(duplicate_vmo),
            block_size: BLOCK_SIZE,
            info: DeviceInfo::Block(BlockInfo {
                device_flags,
                max_transfer_blocks: NonZero::new(1),
                ..Default::default()
            }),
            observer: Some(Box::new(ShufflingObserver(
                AtomicUsize::new(expected_flushes),
                closed_clone,
            ))),
            write_tracking: true,
            max_jitter_usec: Some(5000),
            ..Default::default()
        }
        .build()
        .unwrap();
        block_server.serve(stream).await.unwrap();
    });

    {
        let (session_proxy, session_server) = fidl::endpoints::create_proxy();

        proxy.open_session(session_server).unwrap();

        let transfer_vmo = zx::Vmo::create(4 * BLOCK_SIZE as u64).unwrap();
        let vmo_id = session_proxy
            .attach_vmo(transfer_vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
            .await
            .unwrap()
            .unwrap();

        let mut fifo = fasync::Fifo::from_fifo(session_proxy.get_fifo().await.unwrap().unwrap());
        let (mut reader, mut writer) = fifo.async_io();

        // Perform two writes, with the first having FUA or the second having PRE_BARRIER.
        // Both writes are big enough that they need to be split.
        transfer_vmo.write(&[0x11u8; BLOCK_SIZE as usize], 0).expect("vmo write failed");
        transfer_vmo
            .write(&[0x22u8; BLOCK_SIZE as usize], 1 * BLOCK_SIZE as u64)
            .expect("vmo write failed");
        transfer_vmo
            .write(&[0x33u8; BLOCK_SIZE as usize], 2 * BLOCK_SIZE as u64)
            .expect("vmo write failed");
        transfer_vmo
            .write(&[0x44u8; BLOCK_SIZE as usize], 3 * BLOCK_SIZE as u64)
            .expect("vmo write failed");

        let write1_flags = match case {
            BarrierFuaTestCase::Fua | BarrierFuaTestCase::SimulatedFua => {
                BlockIoFlag::FORCE_ACCESS.bits()
            }
            _ => BlockIoFlag::empty().bits(),
        };
        let write2_flags = match case {
            BarrierFuaTestCase::Barrier | BarrierFuaTestCase::SimulatedBarrier => {
                BlockIoFlag::PRE_BARRIER.bits()
            }
            _ => BlockIoFlag::empty().bits(),
        };

        let mut response = BlockFifoResponse::default();
        writer
            .write_entries(&BlockFifoRequest {
                command: BlockFifoCommand {
                    opcode: BlockOpcode::Write.into_primitive(),
                    flags: write1_flags,
                    ..Default::default()
                },
                vmoid: vmo_id.id,
                dev_offset: 0,
                length: 2,
                vmo_offset: 0,
                ..Default::default()
            })
            .await
            .unwrap();
        reader.read_entries(&mut response).await.unwrap();
        assert_eq!(response.status, zx::sys::ZX_OK);
        writer
            .write_entries(&BlockFifoRequest {
                command: BlockFifoCommand {
                    opcode: BlockOpcode::Write.into_primitive(),
                    flags: write2_flags,
                    ..Default::default()
                },
                vmoid: vmo_id.id,
                dev_offset: 2,
                length: 2,
                vmo_offset: 2,
                ..Default::default()
            })
            .await
            .unwrap();
        reader.read_entries(&mut response).await.unwrap();
        assert_eq!(response.status, zx::sys::ZX_OK);
    }
    drop(proxy);
    server.await;

    assert!(closed.load(Ordering::Relaxed));

    let bs = BLOCK_SIZE as usize;
    let vmo_contents = vmo.read_to_vec::<u8>(0, 4 * BLOCK_SIZE as u64).unwrap();
    // If *either* blocks 2,3 were written, then *both* of blocks 0,1 must have been written too.
    if &vmo_contents[2 * bs..3 * bs] == &[0x33u8; BLOCK_SIZE as usize]
        || &vmo_contents[3 * bs..] == &[0x44u8; BLOCK_SIZE as usize]
    {
        assert!(
            &vmo_contents[0..bs] == &[0x11u8; BLOCK_SIZE as usize]
                && &vmo_contents[bs..2 * bs] == &[0x22u8; BLOCK_SIZE as usize],
            "Writes were reordered across barrier"
        );
    }
}

#[test_case(BarrierFuaTestCase::SimulatedBarrier; "simulated_barrier")]
#[test_case(BarrierFuaTestCase::Barrier; "barrier")]
#[test_case(BarrierFuaTestCase::SimulatedFua; "simulated_fua")]
#[test_case(BarrierFuaTestCase::Fua; "fua")]
#[fuchsia::test]
async fn test_barriers_and_fua_cpp_server(case: BarrierFuaTestCase) {
    let (proxy, server) = fidl::endpoints::create_proxy::<fblock::BlockMarker>();

    let device_flags = match case {
        BarrierFuaTestCase::Barrier => fblock::DeviceFlag::BARRIER_SUPPORT,
        BarrierFuaTestCase::Fua => fblock::DeviceFlag::FUA_SUPPORT,
        _ => fblock::DeviceFlag::empty(),
    };
    let ramdisk = ramdevice_client::RamdiskClientBuilder::new(BLOCK_SIZE as u64, NUM_BLOCKS)
        .max_transfer_blocks(MAX_TRANSFER_BLOCKS)
        .device_flags(device_flags)
        .build()
        .await
        .expect("Failed to create ramdisk");
    ramdisk.connect(server.into_channel().into()).expect("Failed to connect to ramdisk");

    let ramdisk_proxy = ramdisk.open_ramdisk().expect("failed to open ramdisk protocol");
    ramdisk_proxy
        .set_flags(
            framdisk::RamdiskFlag::RESUME_ON_WAKE
                | framdisk::RamdiskFlag::DISCARD_NOT_FLUSHED_ON_WAKE,
        )
        .await
        .expect("set_flags failed");

    ramdisk_proxy.sleep_after(2).await.expect("sleep_after failed");

    let (session_proxy, session_server) = fidl::endpoints::create_proxy();
    proxy.open_session(session_server).unwrap();

    let vmo = zx::Vmo::create(REQ_SIZE).unwrap();
    let vmo_id = session_proxy
        .attach_vmo(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
        .await
        .unwrap()
        .unwrap();

    let mut fifo = fasync::Fifo::from_fifo(session_proxy.get_fifo().await.unwrap().unwrap());
    let (mut reader, mut writer) = fifo.async_io();

    // Perform two writes, the second with PRE_BARRIER set.
    vmo.write(&[0x11u8; BLOCK_SIZE as usize], 0).expect("vmo write failed");
    vmo.write(&[0x22u8; BLOCK_SIZE as usize], BLOCK_SIZE as u64).expect("vmo write failed");

    let write1_flags = match case {
        BarrierFuaTestCase::Fua | BarrierFuaTestCase::SimulatedFua => {
            BlockIoFlag::FORCE_ACCESS.bits()
        }
        _ => BlockIoFlag::empty().bits(),
    };
    let write2_flags = match case {
        BarrierFuaTestCase::Barrier | BarrierFuaTestCase::SimulatedBarrier => {
            BlockIoFlag::PRE_BARRIER.bits()
        }
        _ => BlockIoFlag::empty().bits(),
    };

    let mut response = BlockFifoResponse::default();
    writer
        .write_entries(&BlockFifoRequest {
            command: BlockFifoCommand {
                opcode: BlockOpcode::Write.into_primitive(),
                flags: write1_flags,
                ..Default::default()
            },
            vmoid: vmo_id.id,
            dev_offset: 0,
            length: 1,
            vmo_offset: 0,
            ..Default::default()
        })
        .await
        .unwrap();
    reader.read_entries(&mut response).await.unwrap();
    assert_eq!(response.status, zx::sys::ZX_OK);

    writer
        .write_entries(&BlockFifoRequest {
            command: BlockFifoCommand {
                opcode: BlockOpcode::Write.into_primitive(),
                flags: write2_flags,
                ..Default::default()
            },
            vmoid: vmo_id.id,
            dev_offset: 1,
            length: 1,
            vmo_offset: 1,
            ..Default::default()
        })
        .await
        .unwrap();

    futures::join!(
        async {
            // Once we're received both writes, wake up, which will discard unflushed blocks.
            loop {
                let counts =
                    ramdisk_proxy.get_block_counts().await.expect("get_block_counts failed");
                if counts.received >= 2 {
                    break;
                }
                fasync::Timer::new(std::time::Duration::from_millis(10)).await;
            }
            ramdisk_proxy.wake().await.expect("wake failed");
        },
        async {
            // No response will be delivered until wake is called.
            reader.read_entries(&mut response).await.unwrap();
        },
    );
    assert_eq!(response.status, zx::sys::ZX_OK);

    vmo.write(&[0u8; BLOCK_SIZE as usize * 2], 0).unwrap();
    writer
        .write_entries(&BlockFifoRequest {
            command: BlockFifoCommand {
                opcode: BlockOpcode::Read.into_primitive(),
                ..Default::default()
            },
            vmoid: vmo_id.id,
            dev_offset: 0,
            length: 2,
            vmo_offset: 0,
            ..Default::default()
        })
        .await
        .unwrap();
    reader.read_entries(&mut response).await.unwrap();
    assert_eq!(response.status, zx::sys::ZX_OK);

    let bs = BLOCK_SIZE as usize;
    let vmo_contents = vmo.read_to_vec::<u8>(0, BLOCK_SIZE as u64 * 2).unwrap();

    // Write 1 should be present.
    assert_eq!(&vmo_contents[0..bs], &[0x11u8; BLOCK_SIZE as usize], "Write 1 was lost!");

    // Write 2 should be discarded.
    assert_ne!(&vmo_contents[bs..], &[0x22u8; BLOCK_SIZE as usize], "Write 2 was written!");
}
