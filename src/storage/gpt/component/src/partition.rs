// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::gpt::GptPartition;
use anyhow::{Context as _, Error};
use block_client::{ReadOptions, VmoId, WriteOptions};
use block_server::async_interface::{Interface, PassthroughSession, SessionManager};
use block_server::{DeviceInfo, OffsetMap};
use fidl::endpoints::{RequestStream, ServerEnd};
use fidl_fuchsia_storage_block as fblock;
use fuchsia_async as fasync;

use fuchsia_sync::Mutex;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::future::Future;
use std::num::NonZero;
use std::sync::{Arc, OnceLock, Weak};

/// A wrapper around a VmoId which keeps it active until all requests which use the Vmoid are
/// complete.  Strong references are held by ongoing requests.
pub struct VmoIdWrapper {
    partition: Weak<GptPartition>,
    vmo_id: VmoId,
}

impl std::ops::Deref for VmoIdWrapper {
    type Target = VmoId;
    fn deref(&self) -> &Self::Target {
        &self.vmo_id
    }
}

impl Drop for VmoIdWrapper {
    fn drop(&mut self) {
        // Turn it into an ID so that if the spawned task is dropped, the assertion in VmoId::drop
        // doesn't fire.  It will mean the ID is leaked, but it's most likely that the server is
        // being shut down anyway so it shouldn't matter.
        let vmo_id = self.vmo_id.take().into_id();
        if let Some(partition) = self.partition.upgrade() {
            fasync::Task::spawn(async move {
                if let Err(e) = partition.detach_vmo(VmoId::new(vmo_id)).await {
                    // When a partition connection is cancelled (e.g. during a table reset),
                    // the block client terminates its FIFO to avoid DMA corruption, so any
                    // subsequent `detach_vmo` will fail with CANCELED or PEER_CLOSED.
                    // This is normal and expected, downgrade this to debug logging to avoid
                    // failing tests on log severity.
                    if e == zx::Status::CANCELED || e == zx::Status::PEER_CLOSED {
                        log::debug!("detach_vmo failed during shutdown: {:?}", e);
                    } else {
                        log::error!("detach_vmo failed: {:?}", e);
                    }
                }
            })
            .detach();
        }
    }
}

/// PartitionBackend is an implementation of block_server's Interface which is backed by a windowed
/// view of the underlying GPT device.
pub struct PartitionBackend {
    partition: Arc<GptPartition>,
    vmo_keys_to_vmoids_map: Mutex<BTreeMap<usize, Arc<VmoIdWrapper>>>,
    offset_map: OffsetMap,
    mapper_key: OnceLock<u64>,
}

impl Interface for PartitionBackend {
    async fn open_session(
        &self,
        session_manager: Arc<SessionManager<Self>>,
        stream: fblock::SessionRequestStream,
        offset_map: OffsetMap,
        block_size: u32,
    ) -> Result<(), Error> {
        if !offset_map.is_empty() {
            // For now, we don't support double-passthrough.  We could as needed for nested GPT.
            let _ = stream.control_handle().shutdown_with_epitaph(zx::Status::NOT_SUPPORTED);
            anyhow::bail!("Client-provided offset maps are not supported");
        }
        if self.offset_map.is_empty() {
            return session_manager
                .serve_session(
                    stream,
                    OffsetMap::empty(),
                    self.get_info().max_transfer_blocks(),
                    block_size,
                )
                .await;
        }
        let (proxy, server_end) = fidl::endpoints::create_proxy::<fblock::SessionMarker>();
        self.partition.open_passthrough_session(server_end, &self.offset_map);
        let passthrough = PassthroughSession::new(proxy);
        passthrough.serve(stream).await
    }

    fn open_mapper_session(
        session_manager: Arc<SessionManager<Self>>,
        session: ServerEnd<fblock::MapperSessionMarker>,
        mapping_vmo: zx::Vmo,
        _block_size: u32,
        port: Option<zx::Port>,
        delivery_queue: Option<zx::Vmo>,
    ) -> Result<impl Future<Output = Result<(), Error>> + Send + 'static, zx::Status> {
        let this = session_manager.interface();
        let Some(gpt) = this.partition.gpt() else {
            return Err(zx::Status::BAD_STATE);
        };
        if this.offset_map.is_empty() || !gpt.has_mapper() {
            return Err(zx::Status::NOT_SUPPORTED);
        }
        let this = this.clone();
        Ok(async move {
            let mut init = false;
            let key = *this.mapper_key.get_or_init(|| {
                init = true;
                gpt.next_partition_key()
            });
            // This is thread-safe because `open_child_session` in the server waits for mappings if
            // they arrive late.
            if init {
                gpt.register_mappings(key, &this.offset_map).await?;
            }
            let session_proxy = gpt.mapper_session_proxy().await;
            session_proxy
                .open_child_session(session, mapping_vmo, key, port, delivery_queue)
                .await
                .context("FIDL error calling OpenChildSession on mapper session")?
                .map_err(|status| {
                    anyhow::anyhow!(
                        "OpenChildSession failed: {:?}",
                        zx::Status::err_from_raw(status)
                    )
                })?;
            Ok(())
        })
    }

    async fn on_attach_vmo(&self, vmo: &zx::Vmo) -> Result<(), zx::Status> {
        let key = std::ptr::from_ref(vmo) as usize;
        // SAFETY: GPT does not map VMOs in its own process, so it cannot violate Rust's aliasing
        // guarantees.  Safety is delegated to the client process that mapped the VMO.
        let vmo_id = unsafe { self.partition.attach_vmo(vmo) }.await?;
        self.vmo_keys_to_vmoids_map.lock().insert(
            key,
            Arc::new(VmoIdWrapper { partition: Arc::downgrade(&self.partition), vmo_id }),
        );
        Ok(())
    }

    fn on_detach_vmo(&self, vmo: &zx::Vmo) {
        // Note that we will not immediately detach the VMO.  This happens when the last reference
        // to it is dropped (in [`VmoIdWrapper::drop`]).
        let key = std::ptr::from_ref(vmo) as usize;
        self.vmo_keys_to_vmoids_map.lock().remove(&key);
    }

    fn get_info(&self) -> Cow<'_, DeviceInfo> {
        Cow::Owned(self.partition.get_info())
    }

    async fn read(
        &self,
        device_block_offset: u64,
        block_count: u32,
        vmo: &Arc<zx::Vmo>,
        vmo_offset: u64, // *bytes* not blocks
        opts: ReadOptions,
        trace_flow_id: Option<NonZero<u64>>,
    ) -> Result<(), zx::Status> {
        let vmo_id = self.get_vmoid(vmo)?;
        self.partition
            .read(device_block_offset, block_count, &vmo_id, vmo_offset, opts, trace_flow_id)
            .await
    }

    async fn write(
        &self,
        device_block_offset: u64,
        length: u32,
        vmo: &Arc<zx::Vmo>,
        vmo_offset: u64, // *bytes* not blocks
        opts: WriteOptions,
        trace_flow_id: Option<NonZero<u64>>,
    ) -> Result<(), zx::Status> {
        let vmo_id = self.get_vmoid(vmo)?;
        self.partition
            .write(device_block_offset, length, &vmo_id, vmo_offset, opts, trace_flow_id)
            .await
    }

    async fn flush(&self, trace_flow_id: Option<NonZero<u64>>) -> Result<(), zx::Status> {
        self.partition.flush(trace_flow_id).await
    }

    async fn trim(
        &self,
        device_block_offset: u64,
        block_count: u32,
        trace_flow_id: Option<NonZero<u64>>,
    ) -> Result<(), zx::Status> {
        self.partition.trim(device_block_offset, block_count, trace_flow_id).await
    }
}

impl PartitionBackend {
    #[cfg(test)]
    pub fn passthrough(&self) -> bool {
        !self.offset_map.is_empty()
    }

    #[cfg(test)]
    pub fn vmo_count(&self) -> usize {
        self.vmo_keys_to_vmoids_map.lock().len()
    }

    /// If `offset_map` is non-empty, the partition will pass through requests using the provided
    /// offset map.  Otherwise, the partition will proxy I/O requests (and `read`, `write`, etc will
    /// be called on this instance).
    pub fn new(partition: Arc<GptPartition>, offset_map: block_server::OffsetMap) -> Arc<Self> {
        Arc::new(Self {
            partition,
            offset_map,
            vmo_keys_to_vmoids_map: Mutex::new(BTreeMap::new()),
            mapper_key: OnceLock::new(),
        })
    }

    /// Updates the info.
    pub fn update_info(&self, info: gpt::PartitionInfo) {
        self.partition.update_info(info)
    }

    fn get_vmoid(&self, vmo: &zx::Vmo) -> Result<Arc<VmoIdWrapper>, zx::Status> {
        let key = std::ptr::from_ref(vmo) as usize;
        self.vmo_keys_to_vmoids_map.lock().get(&key).map(Arc::clone).ok_or(zx::Status::NOT_FOUND)
    }
}
