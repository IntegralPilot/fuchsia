// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::borrow::Cow;
use std::num::NonZero;
use std::sync::{Arc, Weak};

use crate::command_queue::CommandQueue;
use crate::partition_name;
use block_server::callback_interface::{
    DefaultCallbackBlockService, Interface, Request, Session, SessionManager,
};
use block_server::{DeviceInfo, PartitionInfo};
use fidl_fuchsia_storage_block::{BlockInfo, MAX_TRANSFER_UNBOUNDED};
use fidl_next_fuchsia_hardware_cqhci::EmmcPartitionId;
use mapping::reader::BlockService;
use sdmmc_spec::MMC_BLOCK_SIZE;
use storage_device::buffer::OwnedBuffer;
use storage_device::buffer_allocator::BufferSource;
use storage_device::pinned_buffer_allocator::PinnedBufferAllocator;
const BUFFER_POOL_CAPACITY: usize = 4 * 1024 * 1024; // 4 MiB

pub struct EmmcPartition {
    partition: EmmcPartitionId,
    device_info: DeviceInfo,
    command_queue: std::sync::Weak<CommandQueue>,
}

impl EmmcPartition {
    pub fn new(
        partition: EmmcPartitionId,
        command_queue: std::sync::Weak<CommandQueue>,
        block_info: BlockInfo,
    ) -> Self {
        Self {
            partition,
            device_info: DeviceInfo::Partition(PartitionInfo {
                device_flags: block_info.flags,
                max_transfer_blocks: if block_info.max_transfer_size != MAX_TRANSFER_UNBOUNDED {
                    NonZero::new(block_info.max_transfer_size / block_info.block_size)
                } else {
                    None
                },
                start_block_offset: None,
                block_count: block_info.block_count,
                type_guid: [0u8; 16],
                instance_guid: [0u8; 16],
                name: partition_name(partition).to_string(),
                ..Default::default()
            }),
            command_queue,
        }
    }
}

impl Interface for EmmcPartition {
    type Orchestrator = SessionManager<Self>;

    fn get_info(&self) -> Cow<'_, DeviceInfo> {
        Cow::Borrowed(&self.device_info)
    }

    fn spawn_session(&self, session: Arc<Session<Self>>) {
        std::thread::spawn(move || {
            if let Err(err) = fuchsia_scheduler::set_role_for_this_thread(
                "fuchsia.devices.block.drivers.sdmmc.worker",
            ) {
                log::warn!(err:?; "Failed to set thread role");
            }
            session.run();
        });
    }

    fn on_requests(&self, requests: &[Request]) {
        let Some(command_queue) = self.command_queue.upgrade() else {
            return;
        };
        for request in requests {
            let request_id = request.request_id;
            let trace_flow_id = request.trace_flow_id;
            match &request.operation {
                block_server::Operation::Read {
                    device_block_offset,
                    block_count,
                    _unused,
                    vmo_offset,
                    options,
                } => command_queue.submit_read(
                    self.partition,
                    request_id,
                    *device_block_offset,
                    *block_count,
                    request.vmo.as_ref().unwrap().clone(),
                    *vmo_offset,
                    *options,
                    trace_flow_id,
                ),
                block_server::Operation::Write {
                    device_block_offset,
                    block_count,
                    _unused,
                    vmo_offset,
                    options,
                } => command_queue.submit_write(
                    self.partition,
                    request_id,
                    *device_block_offset,
                    *block_count,
                    request.vmo.as_ref().unwrap().clone(),
                    *vmo_offset,
                    *options,
                    trace_flow_id,
                ),
                block_server::Operation::Flush => {
                    command_queue.submit_flush(self.partition, request_id, trace_flow_id)
                }
                block_server::Operation::Trim { device_block_offset, block_count } => command_queue
                    .submit_trim(
                        self.partition,
                        request_id,
                        *device_block_offset,
                        *block_count,
                        trace_flow_id,
                    ),
                block_server::Operation::CloseVmo => {
                    unreachable!()
                }
                block_server::Operation::StartDecompressedRead { .. } => {
                    unimplemented!()
                }
                block_server::Operation::ContinueDecompressedRead { .. } => {
                    unimplemented!()
                }
            };
        }
    }

    fn into_block_service(
        self: Arc<Self>,
        orchestrator: &Arc<Self::Orchestrator>,
    ) -> Arc<dyn BlockService> {
        let Some(command_queue) = self.command_queue.upgrade() else {
            return Arc::new(DefaultCallbackBlockService::<Self>::new(orchestrator));
        };
        match PartitionBlockService::new(command_queue, self.partition) {
            Ok(service) => Arc::new(service),
            Err(e) => {
                log::warn!(e:?; "Failed to create PartitionBlockService, using fallback");
                Arc::new(DefaultCallbackBlockService::<Self>::new(orchestrator))
            }
        }
    }
}

pub struct PartitionBlockService {
    command_queue: Weak<CommandQueue>,
    partition: EmmcPartitionId,
    allocator: Arc<PinnedBufferAllocator>,
}

impl PartitionBlockService {
    pub fn new(
        command_queue: Arc<CommandQueue>,
        partition: EmmcPartitionId,
    ) -> Result<Self, anyhow::Error> {
        let page_size = zx::system_get_page_size() as usize;
        let contiguity = command_queue.minimum_contiguity();
        let bti = command_queue
            .bti()
            .duplicate_handle(zx::Rights::SAME_RIGHTS)
            .map_err(|status| anyhow::anyhow!("Failed to duplicate BTI: {status:?}"))?;
        let source = BufferSource::new(BUFFER_POOL_CAPACITY);
        let allocator = Arc::new(PinnedBufferAllocator::new(
            std::cmp::max(MMC_BLOCK_SIZE as usize, page_size),
            source,
            bti,
            contiguity,
        ));
        Ok(Self { command_queue: Arc::downgrade(&command_queue), partition, allocator })
    }
}

impl BlockService for PartitionBlockService {
    fn allocate_buffer(&self, max_len: usize) -> OwnedBuffer {
        let max_len = std::cmp::min(
            std::cmp::min(max_len, mapping::reader::MAX_READ_BUFFER_SIZE),
            self.allocator.buffer_source().size(),
        );
        self.allocator.allocate_buffer_sync_owned(max_len)
    }

    fn read_blocks(
        &self,
        device_offset: u64,
        dest_buffer: OwnedBuffer,
        on_complete: Box<dyn FnOnce(Result<OwnedBuffer, anyhow::Error>) + Send>,
    ) -> Result<(), anyhow::Error> {
        let block_size = MMC_BLOCK_SIZE as u64;
        anyhow::ensure!(
            device_offset.is_multiple_of(block_size)
                && (dest_buffer.len() as u64).is_multiple_of(block_size)
                && (dest_buffer.range().start as u64).is_multiple_of(block_size),
            "Unaligned read parameters"
        );
        let block_offset = device_offset / block_size;
        let command_queue = self
            .command_queue
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("CommandQueue has been shut down"))?;
        command_queue
            .submit_read_direct(
                self.partition,
                block_offset,
                dest_buffer,
                Box::new(move |res| {
                    on_complete(res.map_err(|status| {
                        anyhow::anyhow!("DMA read failed with status: {status:?}")
                    }))
                }),
            )
            .map_err(|status| anyhow::anyhow!("Failed to submit direct read: {status:?}"))?;
        Ok(())
    }
}
