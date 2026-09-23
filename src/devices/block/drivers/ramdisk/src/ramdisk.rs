// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use block_protocol;
use block_server::async_interface::{Interface, SessionManager};
use block_server::{BlockServer, DeviceInfo};
use fidl::endpoints::{DiscoverableProtocolMarker, RequestStream};
use fidl_fuchsia_driver_token as ftoken;
use fidl_fuchsia_hardware_ramdisk as framdisk;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_storage_block::{BlockRequestStream, MapperRequestStream};
use fuchsia_async as fasync;
use fuchsia_async::condition::Condition;
use futures::TryStreamExt;
use std::borrow::Cow;
use std::num::NonZeroU64;
use std::sync::Arc;
use std::task::Poll;
use vfs::ExecutionScope;
use vfs::directory::helper::DirectlyMutable;
use vfs::directory::serve_on;
use vfs::directory::simple::Simple;
use vfs::service::endpoint;
use zx::{self, Status};

pub struct Ramdisk {
    ramdisk: Arc<RamdiskInner>,
    block_server: Arc<BlockServer<SessionManager<RamdiskInner>>>,
}

struct RamdiskState {
    block_counts: framdisk::BlockWriteCounts,
    pre_sleep_write_block_count: Option<u64>,
    blocks_written_since_last_barrier: Vec<u64>,
    flags: framdisk::RamdiskFlag,
}

struct RamdiskInner {
    scope: ExecutionScope,
    block_size: u32,
    block_count: u64,
    vmo: zx::Vmo,
    state: Condition<RamdiskState>,
    partition_info: block_server::PartitionInfo,
    node_token: Option<zx::Event>,
}

impl Ramdisk {
    pub fn new(
        scope: ExecutionScope,
        vmo: zx::Vmo,
        partition_info: block_server::PartitionInfo,
        block_size: u32,
        node_token: Option<zx::Event>,
    ) -> Result<Ramdisk, Status> {
        let block_count = partition_info.block_count;

        let state = Condition::new(RamdiskState {
            block_counts: framdisk::BlockWriteCounts { received: 0, successful: 0, failed: 0 },
            pre_sleep_write_block_count: None,
            blocks_written_since_last_barrier: Vec::new(),
            flags: framdisk::RamdiskFlag::empty(),
        });

        let ramdisk = Arc::new(RamdiskInner {
            scope,
            block_size,
            block_count,
            vmo,
            state,
            partition_info,
            node_token,
        });

        let block_server = Arc::new(BlockServer::new(block_size, ramdisk.clone()));

        Ok(Self { ramdisk, block_server })
    }

    pub fn token_request_handler(
        &self,
    ) -> impl Fn(ExecutionScope, fasync::Channel) + Send + Sync + 'static {
        let ramdisk = self.ramdisk.clone();
        move |_scope, channel| {
            let requests = ftoken::NodeTokenRequestStream::from_channel(channel);
            let ramdisk_clone = ramdisk.clone();
            ramdisk.scope.spawn(async move {
                let _ = requests
                    .try_for_each(|request| {
                        let ramdisk = ramdisk_clone.clone();
                        async move {
                            match request {
                                ftoken::NodeTokenRequest::Get { responder } => {
                                    let token = ramdisk
                                        .node_token
                                        .as_ref()
                                        .map(|t| t.duplicate_handle(zx::Rights::SAME_RIGHTS))
                                        .transpose();
                                    let res = match token {
                                        Ok(Some(t)) => Ok(t),
                                        Ok(None) => Err(zx::Status::NOT_FOUND.into_raw()),
                                        Err(s) => Err(s.into_raw()),
                                    };
                                    let _ = responder.send(res);
                                    Ok(())
                                }
                            }
                        }
                    })
                    .await;
            });
        }
    }

    pub fn block_request_handler(
        &self,
    ) -> impl Fn(ExecutionScope, fasync::Channel) + Send + Sync + 'static {
        let ramdisk = self.ramdisk.clone();
        let block_server = self.block_server.clone();
        move |_scope, channel| {
            let requests = BlockRequestStream::from_channel(channel);
            let block_server = block_server.clone();
            ramdisk.scope.spawn(async move {
                if let Err(error) = block_server.handle_requests(requests).await {
                    log::error!(error:?; "handle_requests failed");
                }
            });
        }
    }

    pub fn mapper_request_handler(
        &self,
    ) -> impl Fn(ExecutionScope, fasync::Channel) + Send + Sync + 'static {
        let ramdisk = self.ramdisk.clone();
        let block_server = self.block_server.clone();
        move |_scope, channel| {
            let requests = MapperRequestStream::from_channel(channel);
            let block_server = block_server.clone();
            ramdisk.scope.spawn(async move {
                if let Err(error) = block_server.handle_mapper_requests(requests).await {
                    log::error!(error:?; "handle_mapper_requests failed");
                }
            });
        }
    }

    pub fn serve(
        &self,
        scope: &ExecutionScope,
        server_end: fidl::endpoints::ServerEnd<fio::DirectoryMarker>,
    ) {
        let svc_dir = Simple::new();
        let ramdisk = self.ramdisk.clone();
        svc_dir
            .add_entry(
                fidl_fuchsia_hardware_ramdisk::RamdiskMarker::PROTOCOL_NAME,
                endpoint(move |_scope, channel| {
                    let ramdisk_clone = ramdisk.clone();
                    let stream = framdisk::RamdiskRequestStream::from_channel(channel);
                    ramdisk.scope.spawn(async move {
                        let _ = stream
                            .try_for_each(|request| ramdisk_clone.handle_ramdisk_request(request))
                            .await;
                    });
                }),
            )
            .unwrap();

        svc_dir
            .add_entry(
                fidl_fuchsia_storage_block::BlockMarker::PROTOCOL_NAME,
                endpoint(self.block_request_handler()),
            )
            .unwrap();

        svc_dir
            .add_entry(
                fidl_fuchsia_storage_block::MapperMarker::PROTOCOL_NAME,
                endpoint(self.mapper_request_handler()),
            )
            .unwrap();

        let dir = Simple::new();
        dir.add_entry("svc", svc_dir).unwrap();

        serve_on(dir, fio::PERM_READABLE, scope.clone(), server_end);
    }
}

impl RamdiskInner {
    fn read_impl(
        &self,
        vmo: &zx::Vmo,
        block_count: u32,
        device_block_offset: u64,
        vmo_offset: u64,
    ) -> Result<(), Status> {
        let length = block_count as u64 * self.block_size as u64;
        let offset = device_block_offset * self.block_size as u64;
        vmo.write(&self.vmo.read_to_vec(offset, length)?, vmo_offset)
    }

    fn write_impl(
        &self,
        vmo: &zx::Vmo,
        block_count: u32,
        device_block_offset: u64,
        vmo_offset: u64,
        fua: bool,
    ) -> Result<(), Status> {
        let block_count = block_count as u64;
        let length = (block_count * self.block_size as u64) as usize;
        let offset = device_block_offset * self.block_size as u64;

        let buf = vmo.read_to_vec(vmo_offset, length as u64)?;
        let result = self.vmo.write(&buf, offset);

        let mut state = self.state.lock();

        if result.is_err() {
            state.block_counts.failed += block_count;
            return result;
        }

        state.block_counts.successful += block_count;
        if let Some(count) = &mut state.pre_sleep_write_block_count {
            *count = count.saturating_sub(block_count);
        }

        // If force-unit-access was set, pretend these blocks were written and
        // flushed so that they don't get discarded if the discard feature is
        // used.
        if !fua {
            state
                .blocks_written_since_last_barrier
                .extend(device_block_offset..device_block_offset + block_count);
        }

        Ok(())
    }

    fn should_fail_requests(&self, state: &RamdiskState) -> bool {
        state.pre_sleep_write_block_count == Some(0)
            && !state.flags.contains(framdisk::RamdiskFlag::RESUME_ON_WAKE)
    }

    async fn handle_ramdisk_request(
        self: &Arc<Self>,
        request: framdisk::RamdiskRequest,
    ) -> Result<(), fidl::Error> {
        match request {
            framdisk::RamdiskRequest::SetFlags { flags, responder } => {
                {
                    self.state.lock().flags = flags;
                }
                responder.send()?;
            }
            framdisk::RamdiskRequest::Wake { responder } => {
                let mut state = self.state.lock();
                if state.flags.contains(framdisk::RamdiskFlag::DISCARD_NOT_FLUSHED_ON_WAKE) {
                    let fill = vec![0xaf; self.block_size as usize];
                    let discard_all = !state.flags.contains(framdisk::RamdiskFlag::DISCARD_RANDOM);
                    for block in state
                        .blocks_written_since_last_barrier
                        .drain(..)
                        .filter(|_| discard_all || rand::random())
                    {
                        let offset = block * self.block_size as u64;
                        self.vmo.write(&fill, offset).unwrap();
                    }
                }
                state.block_counts =
                    framdisk::BlockWriteCounts { received: 0, successful: 0, failed: 0 };
                state.pre_sleep_write_block_count = None;
                for waker in state.drain_wakers() {
                    waker.wake();
                }
                responder.send()?;
            }
            framdisk::RamdiskRequest::SleepAfter { count, responder } => {
                {
                    let mut state = self.state.lock();
                    state.block_counts =
                        framdisk::BlockWriteCounts { received: 0, successful: 0, failed: 0 };
                    state.pre_sleep_write_block_count = Some(count);
                }
                responder.send()?;
            }
            framdisk::RamdiskRequest::GetBlockCounts { responder } => {
                let counts = self.state.lock().block_counts;
                responder.send(&counts)?;
            }
        }
        Ok(())
    }

    async fn maybe_sleep(&self) -> Result<(), zx::Status> {
        self.state
            .when(|state| {
                if state.pre_sleep_write_block_count.is_none_or(|c| c > 0)
                    || !state.flags.contains(framdisk::RamdiskFlag::RESUME_ON_WAKE)
                {
                    Poll::Ready(if self.should_fail_requests(&state) {
                        Err(Status::UNAVAILABLE)
                    } else {
                        Ok(())
                    })
                } else {
                    Poll::Pending
                }
            })
            .await
    }
}

impl Interface for RamdiskInner {
    fn get_info(&self) -> Cow<'_, DeviceInfo> {
        Cow::Owned(DeviceInfo::Partition(self.partition_info.clone()))
    }

    async fn read(
        &self,
        mut device_block_offset: u64,
        mut block_count: u32,
        vmo: &Arc<zx::Vmo>,
        mut vmo_offset: u64,
        _opts: block_protocol::ReadOptions,
        _trace_flow_id: Option<NonZeroU64>,
    ) -> Result<(), Status> {
        if device_block_offset >= self.block_count
            || self.block_count - device_block_offset < block_count as u64
        {
            return Err(Status::OUT_OF_RANGE);
        }

        let mut pre_sleep_count = 0;
        {
            let state = self.state.lock();
            if let Some(count) = state.pre_sleep_write_block_count {
                if block_count as u64 >= count {
                    pre_sleep_count = count as u32;
                }
            }
        }

        if pre_sleep_count > 0 {
            self.read_impl(vmo, pre_sleep_count, device_block_offset, vmo_offset)?;
            block_count -= pre_sleep_count as u32;
            device_block_offset += pre_sleep_count as u64;
            vmo_offset += pre_sleep_count as u64 * self.block_size as u64;
        }

        if block_count > 0 {
            self.maybe_sleep().await?;
            self.read_impl(vmo, block_count, device_block_offset, vmo_offset)?;
        }

        Ok(())
    }

    async fn write(
        &self,
        mut device_block_offset: u64,
        mut block_count: u32,
        vmo: &Arc<zx::Vmo>,
        mut vmo_offset: u64,
        opts: block_protocol::WriteOptions,
        _trace_flow_id: Option<NonZeroU64>,
    ) -> Result<(), Status> {
        if device_block_offset >= self.block_count
            || self.block_count - device_block_offset < block_count as u64
        {
            return Err(Status::OUT_OF_RANGE);
        }

        let mut pre_sleep_count = 0;
        {
            let mut state = self.state.lock();
            state.block_counts.received += block_count as u64;
            if let Some(count) = state.pre_sleep_write_block_count {
                if block_count as u64 >= count {
                    pre_sleep_count = count as u32;
                }
            }

            if opts.flags.contains(block_protocol::WriteFlags::PRE_BARRIER) {
                state.blocks_written_since_last_barrier.clear();
            }
        }

        if pre_sleep_count > 0 {
            self.write_impl(
                vmo,
                pre_sleep_count,
                device_block_offset,
                vmo_offset,
                opts.flags.contains(block_protocol::WriteFlags::FORCE_ACCESS),
            )?;
            block_count -= pre_sleep_count;
            device_block_offset += pre_sleep_count as u64;
            vmo_offset += pre_sleep_count as u64 * self.block_size as u64;
        }

        if block_count > 0 {
            self.maybe_sleep()
                .await
                .inspect_err(|_| self.state.lock().block_counts.failed += block_count as u64)?;
            self.write_impl(
                vmo,
                block_count,
                device_block_offset,
                vmo_offset,
                opts.flags.contains(block_protocol::WriteFlags::FORCE_ACCESS),
            )?;
        }

        Ok(())
    }

    async fn flush(&self, _trace_flow_id: Option<NonZeroU64>) -> Result<(), Status> {
        if self.should_fail_requests(&self.state.lock()) {
            Err(zx::Status::UNAVAILABLE)
        } else {
            self.state.lock().blocks_written_since_last_barrier.clear();
            Ok(())
        }
    }

    async fn trim(
        &self,
        _device_block_offset: u64,
        _block_count: u32,
        _trace_flow_id: Option<NonZeroU64>,
    ) -> Result<(), Status> {
        Err(zx::Status::NOT_SUPPORTED)
    }
}
