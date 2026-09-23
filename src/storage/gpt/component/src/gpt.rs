// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::config::Config;
use crate::partition::PartitionBackend;
use crate::partitions_directory::PartitionsDirectory;
use anyhow::{Context as _, Error, ensure};
use block_client::{
    BlockClient as _, BufferSlice, MutableBufferSlice, ReadOptions, RemoteBlockClient, VmoId,
    WriteOptions,
};
use block_server::async_interface::SessionManager;
use block_server::{BlockServer, OffsetMap};

use fidl::endpoints::ServerEnd;
use fidl_fuchsia_storage_block as fblock;
use fidl_fuchsia_storage_partitions as fpartitions;
use fs_management::format::constants::{
    ALL_BENCHMARK_PARTITION_LABELS, ALL_SYSTEM_PARTITION_LABELS,
};
use fuchsia_async as fasync;
use fuchsia_sync::Mutex;
use futures::stream::TryStreamExt as _;
use std::collections::BTreeMap;
use std::num::NonZero;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, Weak};

fn partition_directory_entry_name(index: u32) -> String {
    format!("part-{:03}", index)
}

// We use heuristics to decide which partitions to pass through.
// Partitions which are passed through consume more resources on the underlying block device (e.g. a
// dedicated per-session thread in some implementations), but have better performance due to not
// needing to proxy requests through this component.  As such, the idea is that we only pass through
// "hot" partitions.
// This list should stay small.
fn should_passthrough_partition(info: &block_server::PartitionInfo) -> bool {
    // Partition contains the main filesystem
    ALL_SYSTEM_PARTITION_LABELS.contains(&info.name.as_str())
    // Partitions are used for benchmarks which should replicate the performance
    // of the main filesystem
    || ALL_BENCHMARK_PARTITION_LABELS.contains(&info.name.as_str())
    // We always pass through composite partitions, since the mechanism we use for passthrough is to
    // open sessions with an OffsetMap.  We could locally resolve the offsets, but there's no reason
    // to implement that right now.
    || info.start_block_offset.is_none()
}

fn single_partition_mapping(info: &block_server::PartitionInfo) -> Result<OffsetMap, Error> {
    Ok(OffsetMap::new(vec![block_server::BlockOffsetMapping {
        target_block_offset: info.start_block_offset.ok_or(zx::Status::INVALID_ARGS)?,
        length: info.block_count,
    }])?)
}

/// A single partition in a GPT device.
pub struct GptPartition {
    gpt: Weak<GptManager>,
    info: Mutex<block_server::PartitionInfo>,
    block_client: Arc<RemoteBlockClient>,
}

fn trace_id(trace_flow_id: Option<NonZero<u64>>) -> u64 {
    trace_flow_id.map(|v| v.get()).unwrap_or_default()
}

impl GptPartition {
    pub fn new(
        gpt: &Arc<GptManager>,
        block_client: Arc<RemoteBlockClient>,
        info: block_server::PartitionInfo,
    ) -> Arc<Self> {
        Arc::new(Self { gpt: Arc::downgrade(gpt), info: Mutex::new(info), block_client })
    }

    pub async fn terminate(&self) {
        if let Err(error) = self.block_client.close().await {
            log::warn!(error:?; "Failed to close block client");
        }
    }

    pub fn update_info(&self, info: gpt::PartitionInfo) {
        *self.info.lock() = info.into();
    }

    pub fn block_size(&self) -> u32 {
        self.block_client.block_size()
    }

    pub fn block_count(&self) -> u64 {
        self.info.lock().block_count
    }

    /// Attaches the VMO.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the VMO is only attached once.  The reason for this is that
    /// if the far end suddenly disconnects, it is not safe to assume the VMO will not be written to
    /// in any way: the VMO could be the target of an ongoing DMA transfer.
    ///
    /// The caller must also ensure that no references are held during I/O as this would be
    /// undefined behavior.  The caller may hold pointers, which does not lead to undefined
    /// behavior; Rust does not make the same assumptions as references for pointers.
    pub async unsafe fn attach_vmo(&self, vmo: &zx::Vmo) -> Result<VmoId, zx::Status> {
        // SAFETY: The caller must guarantee that the VMO is only attached once and no references
        // are held during I/O.
        unsafe { self.block_client.attach_vmo(vmo) }.await
    }

    pub async fn detach_vmo(&self, vmoid: VmoId) -> Result<(), zx::Status> {
        self.block_client.detach_vmo(vmoid).await
    }

    pub fn open_passthrough_session(
        &self,
        session: ServerEnd<fblock::SessionMarker>,
        offset_map: &OffsetMap,
    ) {
        if let Some(gpt) = self.gpt.upgrade() {
            let mappings: Vec<fblock::BlockOffsetMapping> = offset_map.into();
            if let Err(error) = gpt.block_proxy.open_session_with_options(session, &mappings[..]) {
                // Client errors normally come back on `session` but that was already consumed.  The
                // client will get a PEER_CLOSED without an epitaph.
                log::warn!(error:?; "Failed to open passthrough session");
            }
        } else {
            if let Err(error) = session.close_with_epitaph(zx::Status::BAD_STATE) {
                log::warn!(error:?; "Failed to send session epitaph");
            }
        }
    }

    /// Returns the parent [`GptManager`] if it is still running.
    pub fn gpt(&self) -> Option<Arc<GptManager>> {
        self.gpt.upgrade()
    }

    pub fn get_info(&self) -> block_server::DeviceInfo {
        let mut info = self.info.lock().clone();
        info.device_flags = self.block_client.block_flags();
        info.max_transfer_blocks = self.block_client.max_transfer_blocks();
        block_server::DeviceInfo::Partition(info)
    }

    pub async fn read(
        &self,
        device_block_offset: u64,
        block_count: u32,
        vmo_id: &VmoId,
        vmo_offset: u64, // *bytes* not blocks
        opts: ReadOptions,
        trace_flow_id: Option<NonZero<u64>>,
    ) -> Result<(), zx::Status> {
        let dev_offset = self
            .absolute_offset(device_block_offset, block_count)
            .map(|offset| offset * self.block_size() as u64)?;
        let buffer = MutableBufferSlice::new_with_vmo_id(
            vmo_id,
            vmo_offset,
            (block_count * self.block_size()) as u64,
        );
        self.block_client
            .read_at_with_opts_traced(buffer, dev_offset, opts, trace_id(trace_flow_id))
            .await
    }

    pub async fn write(
        &self,
        device_block_offset: u64,
        block_count: u32,
        vmo_id: &VmoId,
        vmo_offset: u64, // *bytes* not blocks
        opts: WriteOptions,
        trace_flow_id: Option<NonZero<u64>>,
    ) -> Result<(), zx::Status> {
        let dev_offset = self
            .absolute_offset(device_block_offset, block_count)
            .map(|offset| offset * self.block_size() as u64)?;
        let buffer = BufferSlice::new_with_vmo_id(
            vmo_id,
            vmo_offset,
            (block_count * self.block_size()) as u64,
        );
        self.block_client
            .write_at_with_opts_traced(buffer, dev_offset, opts, trace_id(trace_flow_id))
            .await
    }

    pub async fn flush(&self, trace_flow_id: Option<NonZero<u64>>) -> Result<(), zx::Status> {
        self.block_client.flush_traced(trace_id(trace_flow_id)).await
    }

    pub async fn trim(
        &self,
        device_block_offset: u64,
        block_count: u32,
        trace_flow_id: Option<NonZero<u64>>,
    ) -> Result<(), zx::Status> {
        let dev_offset = self
            .absolute_offset(device_block_offset, block_count)
            .map(|offset| offset * self.block_size() as u64)?;
        let len = block_count as u64 * self.block_size() as u64;
        let end = dev_offset.checked_add(len).ok_or(zx::Status::OUT_OF_RANGE)?;

        self.block_client.trim_traced(dev_offset..end, trace_id(trace_flow_id)).await
    }

    // Converts a relative range specified by [offset, offset+len) into an absolute offset in the
    // GPT device, performing bounds checking within the partition.  Returns ZX_ERR_OUT_OF_RANGE for
    // an invalid offset/len.
    fn absolute_offset(&self, mut offset: u64, len: u32) -> Result<u64, zx::Status> {
        let info = self.info.lock();
        let Some(start_block) = info.start_block_offset else {
            // This indicates that a composite partition was not passed through, which is an error
            // in this library.
            return Err(zx::Status::BAD_STATE);
        };
        offset = offset.checked_add(start_block).ok_or(zx::Status::OUT_OF_RANGE)?;
        let end = offset.checked_add(len as u64).ok_or(zx::Status::OUT_OF_RANGE)?;
        if end > start_block + info.block_count {
            Err(zx::Status::OUT_OF_RANGE)
        } else {
            Ok(offset)
        }
    }
}

struct PendingTransaction {
    transaction: gpt::Transaction,
    client_koid: zx::Koid,
    // A list of indexes for partitions which were added in the transaction.  When committing, all
    // newly created partitions are published.
    added_partitions: Vec<u32>,
    // A task which waits for the client end to be closed and clears the pending transaction.
    _signal_task: fasync::Task<()>,
}

struct Inner {
    gpt: gpt::Gpt,
    partitions: BTreeMap<u32, Arc<BlockServer<SessionManager<PartitionBackend>>>>,
    // We track these separately so that we do not update them during transaction commit.
    composite_partitions: BTreeMap<u32, Arc<BlockServer<SessionManager<PartitionBackend>>>>,
    // Exposes all partitions for discovery by other components.  Should be kept in sync with
    // `partitions`.
    partitions_dir: PartitionsDirectory,
    pending_transaction: Option<PendingTransaction>,
}

impl Inner {
    /// Ensures that `transaction` matches our pending transaction.
    fn ensure_transaction_matches(&self, transaction: &zx::EventPair) -> Result<(), zx::Status> {
        if let Some(pending) = self.pending_transaction.as_ref() {
            if transaction.koid()? == pending.client_koid {
                Ok(())
            } else {
                Err(zx::Status::BAD_HANDLE)
            }
        } else {
            Err(zx::Status::BAD_STATE)
        }
    }

    fn bind_partition(
        &mut self,
        parent: &Arc<GptManager>,
        index: u32,
        info: block_server::PartitionInfo,
        composite_mappings: OffsetMap,
        composite_indexes: Vec<usize>,
    ) -> Result<(), Error> {
        ensure!(
            composite_indexes.is_empty() == composite_mappings.is_empty(),
            "Composite partitions must provide mappings"
        );
        let passthrough = should_passthrough_partition(&info);
        let mappings = if passthrough && composite_mappings.is_empty() {
            // Synthesize a mapping for a non-composite passthrough partition.
            single_partition_mapping(&info)?
        } else {
            // Either this is a composite partition which already has a mapping, or it is a
            // non-composite partition which is not passed through (in which case this is an empty
            // mapping).
            composite_mappings
        };
        log::debug!(
            "GPT part {index}{}{}: {info:?}",
            if !composite_indexes.is_empty() { " (composite)" } else { "" },
            if passthrough { " (passthrough)" } else { "" },
        );
        let partition = PartitionBackend::new(
            GptPartition::new(parent, self.gpt.client().clone(), info),
            mappings,
        );
        let block_server = Arc::new(BlockServer::new(parent.block_size, partition));
        if !composite_indexes.is_empty() {
            self.partitions_dir.add_composite(
                &partition_directory_entry_name(index),
                Arc::downgrade(&block_server),
                Arc::downgrade(parent),
                composite_indexes,
            );
            self.composite_partitions.insert(index, block_server);
        } else {
            self.partitions_dir.add_partition(
                &partition_directory_entry_name(index),
                Arc::downgrade(&block_server),
                Arc::downgrade(parent),
                index as usize,
            );
            self.partitions.insert(index, block_server);
        }
        Ok(())
    }

    fn bind_super_and_userdata_partition(
        &mut self,
        parent: &Arc<GptManager>,
        super_partition: (u32, gpt::PartitionInfo),
        userdata_partition: (u32, gpt::PartitionInfo),
    ) -> Result<(), Error> {
        let extent1 = block_server::BlockOffsetMapping {
            target_block_offset: super_partition.1.start_block,
            length: super_partition.1.num_blocks,
        };
        let extent2 = block_server::BlockOffsetMapping {
            target_block_offset: userdata_partition.1.start_block,
            length: userdata_partition.1.num_blocks,
        };
        let mappings =
            block_server::OffsetMap::new(block_server::coalesce_mappings(vec![extent1, extent2]))?;
        let info = block_server::PartitionInfo {
            // TODO(https://fxbug.dev/443980711): This should come from configuration.
            name: "super_and_userdata".to_string(),
            type_guid: super_partition.1.type_guid.to_bytes(),
            instance_guid: super_partition.1.instance_guid.to_bytes(),
            block_count: mappings.total_blocks(),
            ..Default::default()
        };
        log::debug!(
            "GPT merged parts {:?} + {:?} -> {info:?}",
            super_partition.1,
            userdata_partition.1
        );
        self.bind_partition(
            parent,
            super_partition.0,
            info,
            mappings,
            vec![super_partition.0 as usize, userdata_partition.0 as usize],
        )
    }

    async fn bind_all_partitions(&mut self, parent: &Arc<GptManager>) -> Result<(), Error> {
        self.partitions.clear();
        self.composite_partitions.clear();
        self.partitions_dir.clear().await;

        let mut partitions = self.gpt.partitions().clone();
        if parent.config.merge_super_and_userdata {
            // Attempt to merge the first `super` and `userdata` we find.  The rest will be treated
            // as regular partitions.
            let super_part = match partitions
                .iter()
                .find(|(_, info)| info.label == "super")
                .map(|(index, _)| *index)
            {
                Some(index) => partitions.remove_entry(&index),
                None => None,
            };
            let userdata_part = match partitions
                .iter()
                .find(|(_, info)| info.label == "userdata")
                .map(|(index, _)| *index)
            {
                Some(index) => partitions.remove_entry(&index),
                None => None,
            };
            if super_part.is_some() && userdata_part.is_some() {
                let super_part = super_part.unwrap();
                let userdata_part = userdata_part.unwrap();
                self.bind_super_and_userdata_partition(parent, super_part, userdata_part)?;
            } else if super_part.is_some() || userdata_part.is_some() {
                log::warn!("Only one of super/userdata found; not merging");
                let (index, info) = super_part.or(userdata_part).unwrap();
                self.bind_partition(
                    parent,
                    index,
                    block_server::PartitionInfo::from(&info),
                    OffsetMap::empty(),
                    vec![],
                )?;
            }
        }
        for (index, info) in partitions {
            self.bind_partition(
                parent,
                index,
                block_server::PartitionInfo::from(&info),
                OffsetMap::empty(),
                vec![],
            )?;
        }
        Ok(())
    }

    fn add_partition(&mut self, info: gpt::PartitionInfo) -> Result<usize, gpt::AddPartitionError> {
        let pending = self.pending_transaction.as_mut().unwrap();
        let idx = self.gpt.add_partition(&mut pending.transaction, info)?;
        pending.added_partitions.push(idx as u32);
        Ok(idx)
    }
}

/// Encodes partition `offset_map` into raw extent bytes for the mapping VMO FIFO.
///
/// Returns the encoded payload bytes, total uncompressed size, extent count, and base device
/// offset.
fn offset_map_to_extents(
    offset_map: &OffsetMap,
    block_size: u32,
) -> Result<(Vec<u8>, u64, u32, u64), Error> {
    let mappings = offset_map.mappings();
    let block_size = block_size as u64;
    let base_device_offset =
        mappings.iter().map(|m| m.target_block_offset * block_size).min().unwrap_or(0);
    let mut running_logical = 0u64;
    let mut extents = Vec::with_capacity(mappings.len());
    let num_mappings = mappings.len();
    for (i, m) in mappings.iter().enumerate() {
        let is_last = i == num_mappings - 1;
        let total_bytes = m.length * block_size;
        let len_bytes = if is_last {
            // Because we only support 4 KiB mappings, we can support misalignment if it's the last
            // range (because the unaligned tail wouldn't be usable anyway).
            total_bytes - (total_bytes % mapping::BLOCK_SIZE)
        } else {
            // Because we only support 4 KiB mappings, we cannot handle misalignment across two
            // adjacent ranges.
            ensure!(total_bytes % mapping::BLOCK_SIZE == 0, zx::Status::NOT_SUPPORTED);
            total_bytes
        };
        if len_bytes == 0 {
            continue;
        }
        let dev_offset_bytes = m.target_block_offset * block_size;
        let extent = mapping::Extent::new(
            running_logical..running_logical + len_bytes,
            Some(dev_offset_bytes),
        );
        running_logical += len_bytes;
        extents.push(extent);
    }
    let extents = mapping::Extents::try_new(extents, base_device_offset)?;
    let mut payload_bytes = Vec::with_capacity(mappings.len() * std::mem::size_of::<u64>());
    let mut extent_count = 0u32;
    for w in mapping::Extents::encode_extents_with_base_offset(&extents) {
        payload_bytes.extend_from_slice(&w.to_le_bytes());
        extent_count += 1;
    }
    Ok((payload_bytes, running_logical, extent_count, base_device_offset))
}

struct MapperSessionState {
    session_proxy: fblock::MapperSessionProxy,
    sender: futures::lock::Mutex<vmo_fifo::AsyncSender<mapping::RawMappingCommand>>,
}

/// Runs a GPT device.
pub struct GptManager {
    config: Config,
    block_proxy: fblock::BlockProxy,
    mapper_proxy: Option<fblock::MapperProxy>,
    mapper_session: OnceLock<MapperSessionState>,
    next_partition_key: AtomicU64,
    block_size: u32,
    block_count: u64,
    inner: futures::lock::Mutex<Inner>,
    shutdown: AtomicBool,
}

impl std::fmt::Debug for GptManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        f.debug_struct("GptManager")
            .field("block_size", &self.block_size)
            .field("block_count", &self.block_count)
            .finish()
    }
}

impl GptManager {
    pub async fn new(
        block_proxy: fblock::BlockProxy,
        partitions_dir: Arc<vfs::directory::immutable::Simple>,
    ) -> Result<Arc<Self>, Error> {
        Self::new_with_config(block_proxy, partitions_dir, Config::default()).await
    }

    /// Creates a new [`GptManager`] with an optional mapper proxy and default configuration.
    pub async fn new_with_mapper(
        block_proxy: fblock::BlockProxy,
        mapper_proxy: Option<fblock::MapperProxy>,
        partitions_dir: Arc<vfs::directory::immutable::Simple>,
    ) -> Result<Arc<Self>, Error> {
        Self::new_with_config_and_mapper(
            block_proxy,
            mapper_proxy,
            partitions_dir,
            Config::default(),
        )
        .await
    }

    pub async fn new_with_config(
        block_proxy: fblock::BlockProxy,
        partitions_dir: Arc<vfs::directory::immutable::Simple>,
        config: Config,
    ) -> Result<Arc<Self>, Error> {
        Self::new_with_config_and_mapper(block_proxy, None, partitions_dir, config).await
    }

    /// Creates a new [`GptManager`] with custom config and an optional mapper proxy.
    pub async fn new_with_config_and_mapper(
        block_proxy: fblock::BlockProxy,
        mapper_proxy: Option<fblock::MapperProxy>,
        partitions_dir: Arc<vfs::directory::immutable::Simple>,
        config: Config,
    ) -> Result<Arc<Self>, Error> {
        log::info!("Binding to GPT");
        let client = Arc::new(RemoteBlockClient::new(block_proxy.clone()).await?);
        let block_size = client.block_size();
        let block_count = client.block_count();
        let gpt = gpt::Gpt::open(client).await.context("Failed to load GPT")?;

        let mapper_proxy = match mapper_proxy {
            Some(proxy) => Some(proxy),
            None => {
                let (proxy, server) = fidl::endpoints::create_proxy::<fblock::MapperMarker>();
                if block_proxy.connect_mapper(server).await.is_ok_and(|res| res.is_ok()) {
                    Some(proxy)
                } else {
                    None
                }
            }
        };

        let this = Arc::new(Self {
            config,
            block_proxy,
            mapper_proxy,
            mapper_session: OnceLock::new(),
            next_partition_key: AtomicU64::new(1),
            block_size,
            block_count,
            inner: futures::lock::Mutex::new(Inner {
                gpt,
                partitions: BTreeMap::new(),
                composite_partitions: BTreeMap::new(),
                partitions_dir: PartitionsDirectory::new(partitions_dir),
                pending_transaction: None,
            }),
            shutdown: AtomicBool::new(false),
        });
        this.inner.lock().await.bind_all_partitions(&this).await?;
        log::info!("Starting all partitions OK!");
        Ok(this)
    }

    /// Returns the initialized [`MapperSessionState`] for the parent device's mapper session,
    /// lazily opening the session on first access.
    ///
    /// # Panics
    ///
    /// Panics if the parent device was not configured with a mapper proxy.
    async fn mapper_state(&self) -> &MapperSessionState {
        let mapper_proxy = self.mapper_proxy.as_ref().unwrap();

        let mut init_server = None;
        let state = self.mapper_session.get_or_init(|| {
            let (session_proxy, session_server) =
                fidl::endpoints::create_proxy::<fblock::MapperSessionMarker>();
            let parent_mapping_vmo = zx::Vmo::create(mapping::MAPPING_VMO_SIZE).unwrap();
            let sender = vmo_fifo::AsyncSender::<mapping::RawMappingCommand>::new(
                parent_mapping_vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                1024,
                mapping::PENDING_COMMANDS_CAPACITY,
            )
            .unwrap();
            init_server = Some((session_server, parent_mapping_vmo));
            MapperSessionState { session_proxy, sender: futures::lock::Mutex::new(sender) }
        });
        // Note: A concurrent caller may see `self.mapper_session` as already initialized and
        // return `state` while the initializing thread is still awaiting `open_session`. This is
        // fine because FIDL channel message ordering ensures subsequent requests on `session_proxy`
        // are processed after the session is opened, and any failure will close the channel.
        if let Some((session_server, parent_mapping_vmo)) = init_server {
            match mapper_proxy.open_session(session_server, parent_mapping_vmo, None, None).await {
                Ok(Err(status)) => {
                    log::warn!(
                        status:? = zx::Status::err_from_raw(status);
                        "Failed to open mapper session on parent device"
                    );
                }
                Err(error) => {
                    log::warn!(error:?; "FIDL error calling Mapper.OpenSession on parent device");
                }
                Ok(Ok(())) => {}
            }
        }
        state
    }

    /// Returns a reference to the parent device's [`fblock::MapperSessionProxy`].
    ///
    /// # Panics
    ///
    /// Panics if the parent device was not configured with a mapper proxy.
    pub async fn mapper_session_proxy(&self) -> &fblock::MapperSessionProxy {
        &self.mapper_state().await.session_proxy
    }

    /// Allocates and returns the next unique partition key for mapper sessions.
    pub fn next_partition_key(&self) -> u64 {
        self.next_partition_key.fetch_add(1, Ordering::Relaxed)
    }

    /// Registers the extent mapping for a partition under `key` with the parent mapper session.
    ///
    /// # Panics
    ///
    /// Panics if the parent device was not configured with a mapper proxy.
    pub async fn register_mappings(&self, key: u64, offset_map: &OffsetMap) -> Result<(), Error> {
        let state = self.mapper_state().await;
        let mut sender = state.sender.lock().await;

        let (payload_bytes, stored_size, extent_count, device_offset) =
            offset_map_to_extents(offset_map, self.block_size)?;
        let mut payload_buf = sender.reserve_payload(payload_bytes.len()).await?;
        let cmd = mapping::RawMappingCommand {
            opcode: mapping::MAPPINGS_COMMAND,
            offset: payload_buf.offset(),
            key,
            stored_size,
            device_offset,
            metadata_count: 0,
            extent_count,
        };
        payload_buf.data().copy_from_slice(&payload_bytes);
        payload_buf.commit(cmd).await?;
        Ok(())
    }

    /// Returns `true` if this GPT instance was configured with a mapper proxy.
    pub fn has_mapper(&self) -> bool {
        self.mapper_proxy.is_some()
    }

    pub fn block_size(&self) -> u32 {
        self.block_size
    }

    pub fn block_count(&self) -> u64 {
        self.block_count
    }

    pub async fn create_transaction(self: &Arc<Self>) -> Result<zx::EventPair, zx::Status> {
        let mut inner = self.inner.lock().await;
        if inner.pending_transaction.is_some() {
            return Err(zx::Status::ALREADY_EXISTS);
        }
        let transaction = inner.gpt.create_transaction().unwrap();
        let (client_end, server_end) = zx::EventPair::create();
        let client_koid = client_end.koid()?;
        let signal_waiter = fasync::OnSignals::new(server_end, zx::Signals::EVENTPAIR_PEER_CLOSED);
        let this = self.clone();
        let task = fasync::Task::spawn(async move {
            let _ = signal_waiter.await;
            let mut inner = this.inner.lock().await;
            if inner.pending_transaction.as_ref().map_or(false, |t| t.client_koid == client_koid) {
                inner.pending_transaction = None;
            }
        });
        inner.pending_transaction = Some(PendingTransaction {
            transaction,
            client_koid,
            added_partitions: vec![],
            _signal_task: task,
        });
        Ok(client_end)
    }

    pub async fn commit_transaction(
        self: &Arc<Self>,
        transaction: zx::EventPair,
    ) -> Result<(), zx::Status> {
        let mut inner = self.inner.lock().await;
        inner.ensure_transaction_matches(&transaction)?;
        let pending = std::mem::take(&mut inner.pending_transaction).unwrap();
        let partitions = pending.transaction.partitions.clone();
        if let Err(error) = inner.gpt.commit_transaction(pending.transaction).await {
            log::warn!(error:?; "Failed to commit transaction");
            return Err(zx::Status::IO);
        }
        // Everything after this point should be infallible.
        for (info, idx) in partitions
            .iter()
            .zip(0u32..)
            .filter(|(info, idx)| !info.is_nil() && !pending.added_partitions.contains(idx))
        {
            // Some physical partitions are not tracked in `inner.partitions` (e.g. when we use an
            // composite partition to combine two physical partitions).  In this case, we still need
            // to propagate the info in the underlying transaction, but there's no need to update
            // the in-memory info.
            // Note that composite partitions can't be changed by transactions anyways, so the info
            // we propagate should be exactly what it was when we created the transaction.
            if let Some(part) = inner.partitions.get(&idx) {
                part.session_manager().interface().update_info(info.clone());
            }
        }
        for idx in pending.added_partitions {
            if let Some(gpt_info) = inner.gpt.partitions().get(&idx).cloned() {
                let partition_info = block_server::PartitionInfo::from(&gpt_info);
                if let Err(error) =
                    inner.bind_partition(self, idx, partition_info, OffsetMap::empty(), vec![])
                {
                    log::error!(error:?; "Failed to bind partition");
                }
            }
        }
        Ok(())
    }

    pub async fn add_partition(
        &self,
        request: fpartitions::PartitionsManagerAddPartitionRequest,
    ) -> Result<(), zx::Status> {
        let mut inner = self.inner.lock().await;
        inner.ensure_transaction_matches(
            request.transaction.as_ref().ok_or(zx::Status::BAD_HANDLE)?,
        )?;
        let info = gpt::PartitionInfo {
            label: request.name.ok_or(zx::Status::INVALID_ARGS)?,
            type_guid: request
                .type_guid
                .map(|value| gpt::Guid::from_bytes(value.value))
                .ok_or(zx::Status::INVALID_ARGS)?,
            instance_guid: request
                .instance_guid
                .map(|value| gpt::Guid::from_bytes(value.value))
                .unwrap_or_else(|| gpt::Guid::generate()),
            start_block: 0,
            num_blocks: request.num_blocks.ok_or(zx::Status::INVALID_ARGS)?,
            flags: request.flags.unwrap_or_default(),
        };
        let idx = inner.add_partition(info)?;
        let partition =
            inner.pending_transaction.as_ref().unwrap().transaction.partitions.get(idx).unwrap();
        log::info!(
            "Allocated partition {:?} at {:?}",
            partition.label,
            partition.start_block..partition.start_block + partition.num_blocks
        );
        Ok(())
    }

    pub async fn handle_partitions_requests(
        &self,
        gpt_index: usize,
        mut requests: fpartitions::PartitionRequestStream,
    ) -> Result<(), zx::Status> {
        while let Some(request) = requests.try_next().await.unwrap() {
            match request {
                fpartitions::PartitionRequest::UpdateMetadata { payload, responder } => {
                    responder
                        .send(
                            self.update_partition_metadata(gpt_index, payload)
                                .await
                                .map_err(|status| status.into_raw()),
                        )
                        .unwrap_or_else(
                            |error| log::error!(error:?; "Failed to send UpdateMetadata response"),
                        );
                }
            }
        }
        Ok(())
    }

    async fn update_partition_metadata(
        &self,
        gpt_index: usize,
        request: fpartitions::PartitionUpdateMetadataRequest,
    ) -> Result<(), zx::Status> {
        let mut inner = self.inner.lock().await;
        inner.ensure_transaction_matches(
            request.transaction.as_ref().ok_or(zx::Status::BAD_HANDLE)?,
        )?;

        let transaction = &mut inner.pending_transaction.as_mut().unwrap().transaction;
        let entry = transaction.partitions.get_mut(gpt_index).ok_or(zx::Status::BAD_STATE)?;
        if let Some(type_guid) = request.type_guid.as_ref().cloned() {
            entry.type_guid = gpt::Guid::from_bytes(type_guid.value);
        }
        if let Some(flags) = request.flags.as_ref() {
            entry.flags = *flags;
        }
        Ok(())
    }

    pub async fn handle_composite_partitions_requests(
        &self,
        gpt_indexes: Vec<usize>,
        mut requests: fpartitions::OverlayPartitionRequestStream,
    ) -> Result<(), zx::Status> {
        while let Some(request) = requests.try_next().await.unwrap() {
            match request {
                fpartitions::OverlayPartitionRequest::GetPartitions { responder } => {
                    match self.get_composite_partition_info(&gpt_indexes[..]).await {
                        Ok(partitions) => responder.send(Ok(&partitions[..])),
                        Err(status) => responder.send(Err(status.into_raw())),
                    }
                    .unwrap_or_else(
                        |error| log::error!(error:?; "Failed to send GetPartitions response"),
                    );
                }
            }
        }
        Ok(())
    }

    async fn get_composite_partition_info(
        &self,
        gpt_indexes: &[usize],
    ) -> Result<Vec<fpartitions::PartitionInfo>, zx::Status> {
        fn convert_partition_info(info: &gpt::PartitionInfo) -> fpartitions::PartitionInfo {
            fpartitions::PartitionInfo {
                name: Some(info.label.to_string()),
                type_guid: Some(fblock::Guid { value: info.type_guid.to_bytes() }),
                instance_guid: Some(fblock::Guid { value: info.instance_guid.to_bytes() }),
                start_block_offset: Some(info.start_block),
                num_blocks: Some(info.num_blocks),
                flags: Some(info.flags),
                ..Default::default()
            }
        }

        let inner = self.inner.lock().await;
        let mut partitions = vec![];
        for index in gpt_indexes {
            let index: u32 = *index as u32;
            partitions.push(
                inner
                    .gpt
                    .partitions()
                    .get(&index)
                    .map(convert_partition_info)
                    .ok_or(zx::Status::BAD_STATE)?,
            );
        }
        Ok(partitions)
    }

    pub async fn reset_partition_table(
        self: &Arc<Self>,
        partitions: Vec<gpt::PartitionInfo>,
    ) -> Result<(), zx::Status> {
        let mut inner = self.inner.lock().await;
        if inner.pending_transaction.is_some() {
            return Err(zx::Status::BAD_STATE);
        }

        // Sever all connections and clear existing partitions before writing the new partition
        // table.
        inner.partitions.clear();
        inner.composite_partitions.clear();
        inner.partitions_dir.clear().await;

        // If in-flight I/O on any cleared partition was cancelled, the shared block client's FIFO
        // will have been terminated to prevent DMA memory corruption. Re-establish a fresh block
        // client so we can commit the new partition table metadata and share it with the new
        // partitions.
        let client =
            Arc::new(RemoteBlockClient::new(self.block_proxy.clone()).await.map_err(|e| {
                log::error!(e:?; "Failed to re-establish block client");
                zx::Status::IO
            })?);
        inner.gpt.set_client(client);

        log::info!("Resetting gpt.  Expect data loss!!!");
        let mut transaction = inner.gpt.create_transaction().unwrap();
        transaction.partitions = partitions;
        inner.gpt.commit_transaction(transaction).await?;

        if let Err(error) = inner.bind_all_partitions(&self).await {
            log::error!(error:?; "Failed to rebind partitions");
            return Err(zx::Status::BAD_STATE);
        }
        log::info!("Rebinding partitions OK!");
        Ok(())
    }

    pub async fn shutdown(self: Arc<Self>) {
        log::info!("Shutting down gpt");
        let mut inner = self.inner.lock().await;
        inner.partitions_dir.clear().await;
        inner.partitions.clear();
        inner.composite_partitions.clear();
        if let Some(state) = self.mapper_session.get() {
            let _ = state.session_proxy.close().await;
        }
        self.shutdown.store(true, Ordering::Relaxed);
        log::info!("Shutting down gpt OK");
    }
}

impl Drop for GptManager {
    fn drop(&mut self) {
        assert!(self.shutdown.load(Ordering::Relaxed), "Did you forget to shutdown?");
    }
}

#[cfg(test)]
mod tests {
    use super::GptManager;
    use block_client::{
        BlockClient as _, BlockDeviceFlag, BufferSlice, MutableBufferSlice, RemoteBlockClient,
        WriteFlags,
    };
    use block_server::{BlockInfo, DeviceInfo, OffsetMap, WriteOptions};
    use fidl_fuchsia_io as fio;
    use fidl_fuchsia_storage_block as fblock;
    use fidl_fuchsia_storage_partitions as fpartitions;
    use fuchsia_async as fasync;
    use fuchsia_component::client::connect_to_named_protocol_at_dir_root;
    use futures::StreamExt as _;
    use gpt::{Gpt, Guid, PartitionInfo};
    use std::num::NonZero;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use test_vmo_backed_block_server::{
        InitialContents, Observer, VmoBackedServer, VmoBackedServerOptions, WriteAction,
    };

    async fn setup(
        block_size: u32,
        block_count: u64,
        partitions: Vec<PartitionInfo>,
    ) -> (Arc<VmoBackedServer>, Arc<vfs::directory::immutable::Simple>) {
        setup_with_options(
            VmoBackedServerOptions {
                initial_contents: InitialContents::FromCapacity(block_count),
                block_size,
                ..Default::default()
            },
            partitions,
        )
        .await
    }

    async fn setup_with_options(
        opts: VmoBackedServerOptions<'_>,
        partitions: Vec<PartitionInfo>,
    ) -> (Arc<VmoBackedServer>, Arc<vfs::directory::immutable::Simple>) {
        let server = Arc::new(opts.build().unwrap());
        {
            let (block_client, block_server) =
                fidl::endpoints::create_proxy::<fblock::BlockMarker>();
            let volume_stream = fidl::endpoints::ServerEnd::<fblock::BlockMarker>::from(
                block_server.into_channel(),
            )
            .into_stream();
            let server_clone = server.clone();
            let _task = fasync::Task::spawn(async move { server_clone.serve(volume_stream).await });
            let client = Arc::new(RemoteBlockClient::new(block_client).await.unwrap());
            Gpt::format(client, partitions).await.unwrap();
        }
        (server, vfs::directory::immutable::simple())
    }

    #[fuchsia::test]
    async fn load_unformatted_gpt() {
        let server =
            Arc::new(VmoBackedServer::new(8, 512, &[]).expect("Failed to create VmoBackedServer"));

        GptManager::new(server.connect(), vfs::directory::immutable::simple())
            .await
            .expect_err("load should fail");
    }

    #[fuchsia::test]
    async fn load_formatted_empty_gpt() {
        let (block_device, partitions_dir) = setup(512, 8, vec![]).await;

        let runner = GptManager::new(block_device.connect(), partitions_dir)
            .await
            .expect("load should succeed");
        runner.shutdown().await;
    }

    #[fuchsia::test]
    async fn load_formatted_gpt_with_one_partition() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_GUID: [u8; 16] = [2u8; 16];
        const PART_NAME: &str = "part";

        let (block_device, partitions_dir) = setup(
            512,
            8,
            vec![PartitionInfo {
                label: PART_NAME.to_string(),
                type_guid: Guid::from_bytes(PART_TYPE_GUID),
                instance_guid: Guid::from_bytes(PART_INSTANCE_GUID),
                start_block: 4,
                num_blocks: 1,
                flags: 0,
            }],
        )
        .await;

        let partitions_dir_clone = partitions_dir.clone();
        let runner = GptManager::new(block_device.connect(), partitions_dir_clone)
            .await
            .expect("load should succeed");
        partitions_dir.get_entry("part-000").expect("No entry found");
        partitions_dir.get_entry("part-001").map(|_| ()).expect_err("Extra entry found");
        runner.shutdown().await;
    }

    #[fuchsia::test]
    async fn load_formatted_gpt_with_two_partitions() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_1_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_2_GUID: [u8; 16] = [3u8; 16];
        const PART_1_NAME: &str = "part1";
        const PART_2_NAME: &str = "part2";

        let (block_device, partitions_dir) = setup(
            512,
            8,
            vec![
                PartitionInfo {
                    label: PART_1_NAME.to_string(),
                    type_guid: Guid::from_bytes(PART_TYPE_GUID),
                    instance_guid: Guid::from_bytes(PART_INSTANCE_1_GUID),
                    start_block: 4,
                    num_blocks: 1,
                    flags: 0,
                },
                PartitionInfo {
                    label: PART_2_NAME.to_string(),
                    type_guid: Guid::from_bytes(PART_TYPE_GUID),
                    instance_guid: Guid::from_bytes(PART_INSTANCE_2_GUID),
                    start_block: 5,
                    num_blocks: 1,
                    flags: 0,
                },
            ],
        )
        .await;

        let partitions_dir_clone = partitions_dir.clone();
        let runner = GptManager::new(block_device.connect(), partitions_dir_clone)
            .await
            .expect("load should succeed");
        partitions_dir.get_entry("part-000").expect("No entry found");
        partitions_dir.get_entry("part-001").expect("No entry found");
        partitions_dir.get_entry("part-002").map(|_| ()).expect_err("Extra entry found");
        runner.shutdown().await;
    }

    #[fuchsia::test]
    async fn partition_io() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_GUID: [u8; 16] = [2u8; 16];
        const PART_NAME: &str = "part";

        let (block_device, partitions_dir) = setup(
            512,
            8,
            vec![PartitionInfo {
                label: PART_NAME.to_string(),
                type_guid: Guid::from_bytes(PART_TYPE_GUID),
                instance_guid: Guid::from_bytes(PART_INSTANCE_GUID),
                start_block: 4,
                num_blocks: 2,
                flags: 0,
            }],
        )
        .await;

        let partitions_dir_clone = partitions_dir.clone();
        let runner = GptManager::new(block_device.connect(), partitions_dir_clone)
            .await
            .expect("load should succeed");

        let proxy = vfs::serve_directory(
            partitions_dir.clone(),
            vfs::path::Path::validate_and_split("part-000").unwrap(),
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE,
        );
        let block = connect_to_named_protocol_at_dir_root::<fblock::BlockMarker>(&proxy, "volume")
            .expect("Failed to open block service");
        let client = RemoteBlockClient::new(block).await.expect("Failed to create block client");

        assert_eq!(client.block_count(), 2);
        assert_eq!(client.block_size(), 512);

        let buf = vec![0xabu8; 512];
        client.write_at(BufferSlice::Memory(&buf[..]), 0).await.expect("write_at failed");
        client
            .write_at(BufferSlice::Memory(&buf[..]), 1024)
            .await
            .expect_err("write_at should fail when writing past partition end");
        let mut buf2 = vec![0u8; 512];
        client.read_at(MutableBufferSlice::Memory(&mut buf2[..]), 0).await.expect("read_at failed");
        assert_eq!(buf, buf2);
        client
            .read_at(MutableBufferSlice::Memory(&mut buf2[..]), 1024)
            .await
            .expect_err("read_at should fail when reading past partition end");
        client.trim(512..1024).await.expect("trim failed");
        client.trim(1..512).await.expect_err("trim with invalid range should fail");
        client.trim(1024..1536).await.expect_err("trim past end of partition should fail");
        runner.shutdown().await;

        // Ensure writes persisted to the partition.
        let mut buf = vec![0u8; 512];
        let client =
            RemoteBlockClient::new(block_device.connect::<fblock::BlockProxy>()).await.unwrap();
        client.read_at(MutableBufferSlice::Memory(&mut buf[..]), 2048).await.unwrap();
        assert_eq!(&buf[..], &[0xabu8; 512]);
    }

    #[fuchsia::test]
    async fn load_formatted_gpt_with_invalid_primary_header() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_1_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_2_GUID: [u8; 16] = [3u8; 16];
        const PART_1_NAME: &str = "part1";
        const PART_2_NAME: &str = "part2";

        let (block_device, partitions_dir) = setup(
            512,
            8,
            vec![
                PartitionInfo {
                    label: PART_1_NAME.to_string(),
                    type_guid: Guid::from_bytes(PART_TYPE_GUID),
                    instance_guid: Guid::from_bytes(PART_INSTANCE_1_GUID),
                    start_block: 4,
                    num_blocks: 1,
                    flags: 0,
                },
                PartitionInfo {
                    label: PART_2_NAME.to_string(),
                    type_guid: Guid::from_bytes(PART_TYPE_GUID),
                    instance_guid: Guid::from_bytes(PART_INSTANCE_2_GUID),
                    start_block: 5,
                    num_blocks: 1,
                    flags: 0,
                },
            ],
        )
        .await;
        {
            let (client, stream) =
                fidl::endpoints::create_proxy_and_stream::<fblock::BlockMarker>();
            let server = block_device.clone();
            let _task = fasync::Task::spawn(async move { server.serve(stream).await });
            let client = RemoteBlockClient::new(client).await.unwrap();
            client.write_at(BufferSlice::Memory(&[0xffu8; 512]), 512).await.unwrap();
        }

        let runner = GptManager::new(block_device.connect(), partitions_dir.clone())
            .await
            .expect("load should succeed");
        partitions_dir.get_entry("part-000").expect("No entry found");
        partitions_dir.get_entry("part-001").expect("No entry found");
        runner.shutdown().await;
    }

    #[fuchsia::test]
    async fn load_formatted_gpt_with_invalid_primary_partition_table() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_1_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_2_GUID: [u8; 16] = [3u8; 16];
        const PART_1_NAME: &str = "part1";
        const PART_2_NAME: &str = "part2";

        let (block_device, partitions_dir) = setup(
            512,
            8,
            vec![
                PartitionInfo {
                    label: PART_1_NAME.to_string(),
                    type_guid: Guid::from_bytes(PART_TYPE_GUID),
                    instance_guid: Guid::from_bytes(PART_INSTANCE_1_GUID),
                    start_block: 4,
                    num_blocks: 1,
                    flags: 0,
                },
                PartitionInfo {
                    label: PART_2_NAME.to_string(),
                    type_guid: Guid::from_bytes(PART_TYPE_GUID),
                    instance_guid: Guid::from_bytes(PART_INSTANCE_2_GUID),
                    start_block: 5,
                    num_blocks: 1,
                    flags: 0,
                },
            ],
        )
        .await;
        {
            let (client, stream) =
                fidl::endpoints::create_proxy_and_stream::<fblock::BlockMarker>();
            let server = block_device.clone();
            let _task = fasync::Task::spawn(async move { server.serve(stream).await });
            let client = RemoteBlockClient::new(client).await.unwrap();
            client.write_at(BufferSlice::Memory(&[0xffu8; 512]), 1024).await.unwrap();
        }

        let runner = GptManager::new(block_device.connect(), partitions_dir.clone())
            .await
            .expect("load should succeed");
        partitions_dir.get_entry("part-000").expect("No entry found");
        partitions_dir.get_entry("part-001").expect("No entry found");
        runner.shutdown().await;
    }

    #[fuchsia::test]
    async fn force_access_passed_through() {
        const BLOCK_SIZE: u32 = 512;
        const BLOCK_COUNT: u64 = 1024;

        struct ForceAccessObserver(Arc<AtomicBool>);

        impl Observer for ForceAccessObserver {
            fn write(
                &self,
                _device_block_offset: u64,
                _block_count: u32,
                _vmo: &Arc<zx::Vmo>,
                _vmo_offset: u64,
                opts: WriteOptions,
            ) -> WriteAction {
                assert_eq!(
                    opts.flags.contains(WriteFlags::FORCE_ACCESS),
                    self.0.load(Ordering::Relaxed)
                );
                WriteAction::Write
            }
        }

        let expect_force_access = Arc::new(AtomicBool::new(false));
        let (server, partitions_dir) = setup_with_options(
            VmoBackedServerOptions {
                initial_contents: InitialContents::FromCapacity(BLOCK_COUNT),
                block_size: BLOCK_SIZE,
                observer: Some(Box::new(ForceAccessObserver(expect_force_access.clone()))),
                info: DeviceInfo::Block(BlockInfo {
                    device_flags: fblock::DeviceFlag::FUA_SUPPORT,
                    ..Default::default()
                }),
                ..Default::default()
            },
            vec![PartitionInfo {
                label: "foo".to_string(),
                type_guid: Guid::from_bytes([1; 16]),
                instance_guid: Guid::from_bytes([2; 16]),
                start_block: 4,
                num_blocks: 1,
                flags: 0,
            }],
        )
        .await;

        let manager = GptManager::new(server.connect(), partitions_dir.clone()).await.unwrap();

        let proxy = vfs::serve_directory(
            partitions_dir.clone(),
            vfs::path::Path::validate_and_split("part-000").unwrap(),
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE,
        );
        let block = connect_to_named_protocol_at_dir_root::<fblock::BlockMarker>(&proxy, "volume")
            .expect("Failed to open block service");
        let client = RemoteBlockClient::new(block).await.expect("Failed to create block client");

        let buffer = vec![0; BLOCK_SIZE as usize];
        client.write_at(BufferSlice::Memory(&buffer), 0).await.unwrap();

        expect_force_access.store(true, Ordering::Relaxed);

        client
            .write_at_with_opts(
                BufferSlice::Memory(&buffer),
                0,
                WriteOptions { flags: WriteFlags::FORCE_ACCESS, ..Default::default() },
            )
            .await
            .unwrap();

        manager.shutdown().await;
    }

    #[fuchsia::test]
    async fn barrier_passed_through() {
        const BLOCK_SIZE: u32 = 512;
        const BLOCK_COUNT: u64 = 1024;

        struct BarrierObserver(Arc<AtomicBool>);

        impl Observer for BarrierObserver {
            fn write(
                &self,
                _device_block_offset: u64,
                _block_count: u32,
                _vmo: &Arc<zx::Vmo>,
                _vmo_offset: u64,
                opts: WriteOptions,
            ) -> WriteAction {
                assert_eq!(
                    opts.flags.contains(WriteFlags::PRE_BARRIER),
                    self.0.load(Ordering::Relaxed)
                );
                WriteAction::Write
            }
        }

        let expect_barrier = Arc::new(AtomicBool::new(false));
        let (server, partitions_dir) = setup_with_options(
            VmoBackedServerOptions {
                initial_contents: InitialContents::FromCapacity(BLOCK_COUNT),
                block_size: BLOCK_SIZE,
                observer: Some(Box::new(BarrierObserver(expect_barrier.clone()))),
                info: DeviceInfo::Block(BlockInfo {
                    device_flags: fblock::DeviceFlag::BARRIER_SUPPORT,
                    ..Default::default()
                }),
                ..Default::default()
            },
            vec![PartitionInfo {
                label: "foo".to_string(),
                type_guid: Guid::from_bytes([1; 16]),
                instance_guid: Guid::from_bytes([2; 16]),
                start_block: 4,
                num_blocks: 1,
                flags: 0,
            }],
        )
        .await;

        let manager = GptManager::new(server.connect(), partitions_dir.clone()).await.unwrap();

        let proxy = vfs::serve_directory(
            partitions_dir.clone(),
            vfs::path::Path::validate_and_split("part-000").unwrap(),
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE,
        );
        let block = connect_to_named_protocol_at_dir_root::<fblock::BlockMarker>(&proxy, "volume")
            .expect("Failed to open block service");
        let client = RemoteBlockClient::new(block).await.expect("Failed to create block client");

        let buffer = vec![0; BLOCK_SIZE as usize];
        client.write_at(BufferSlice::Memory(&buffer), 0).await.unwrap();

        expect_barrier.store(true, Ordering::Relaxed);
        client
            .write_at_with_opts(
                BufferSlice::Memory(&buffer),
                0,
                WriteOptions { flags: WriteFlags::PRE_BARRIER, ..Default::default() },
            )
            .await
            .unwrap();

        manager.shutdown().await;
    }

    #[fuchsia::test]
    async fn commit_transaction() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_1_INSTANCE_GUID: [u8; 16] = [2u8; 16];
        const PART_1_NAME: &str = "part";
        const PART_2_INSTANCE_GUID: [u8; 16] = [3u8; 16];
        const PART_2_NAME: &str = "part2";

        let (block_device, partitions_dir) = setup(
            512,
            16,
            vec![
                PartitionInfo {
                    label: PART_1_NAME.to_string(),
                    type_guid: Guid::from_bytes(PART_TYPE_GUID),
                    instance_guid: Guid::from_bytes(PART_1_INSTANCE_GUID),
                    start_block: 4,
                    num_blocks: 1,
                    flags: 0,
                },
                PartitionInfo {
                    label: PART_2_NAME.to_string(),
                    type_guid: Guid::from_bytes(PART_TYPE_GUID),
                    instance_guid: Guid::from_bytes(PART_2_INSTANCE_GUID),
                    start_block: 5,
                    num_blocks: 1,
                    flags: 0,
                },
            ],
        )
        .await;
        let runner = GptManager::new(block_device.connect(), partitions_dir.clone())
            .await
            .expect("load should succeed");

        let part_0_dir = vfs::serve_directory(
            partitions_dir.clone(),
            vfs::Path::validate_and_split("part-000").unwrap(),
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE,
        );
        let part_1_dir = vfs::serve_directory(
            partitions_dir.clone(),
            vfs::Path::validate_and_split("part-001").unwrap(),
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE,
        );
        let part_0_proxy = connect_to_named_protocol_at_dir_root::<fpartitions::PartitionMarker>(
            &part_0_dir,
            "partition",
        )
        .expect("Failed to open Partition service");
        let part_1_proxy = connect_to_named_protocol_at_dir_root::<fpartitions::PartitionMarker>(
            &part_1_dir,
            "partition",
        )
        .expect("Failed to open Partition service");

        let transaction = runner.create_transaction().await.expect("Failed to create transaction");
        part_0_proxy
            .update_metadata(fpartitions::PartitionUpdateMetadataRequest {
                transaction: Some(transaction.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap()),
                type_guid: Some(fblock::Guid { value: [0xffu8; 16] }),
                ..Default::default()
            })
            .await
            .expect("FIDL error")
            .expect("Failed to update_metadata");
        part_1_proxy
            .update_metadata(fpartitions::PartitionUpdateMetadataRequest {
                transaction: Some(transaction.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap()),
                flags: Some(1234),
                ..Default::default()
            })
            .await
            .expect("FIDL error")
            .expect("Failed to update_metadata");
        runner.commit_transaction(transaction).await.expect("Failed to commit transaction");

        // Ensure the changes have propagated to the correct partitions.
        let part_0_block =
            connect_to_named_protocol_at_dir_root::<fblock::BlockMarker>(&part_0_dir, "volume")
                .expect("Failed to open Volume service");
        let (status, guid) = part_0_block.get_type_guid().await.expect("FIDL error");
        assert_eq!(status, zx::sys::ZX_OK);
        assert_eq!(guid.unwrap().value, [0xffu8; 16]);
        let part_1_block =
            connect_to_named_protocol_at_dir_root::<fblock::BlockMarker>(&part_1_dir, "volume")
                .expect("Failed to open Volume service");
        let metadata =
            part_1_block.get_metadata().await.expect("FIDL error").expect("get_metadata failed");
        assert_eq!(metadata.type_guid.unwrap().value, PART_TYPE_GUID);
        assert_eq!(metadata.flags, Some(1234));

        runner.shutdown().await;
    }

    #[fuchsia::test]
    async fn commit_transaction_with_io_error() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_1_INSTANCE_GUID: [u8; 16] = [2u8; 16];
        const PART_1_NAME: &str = "part";
        const PART_2_INSTANCE_GUID: [u8; 16] = [3u8; 16];
        const PART_2_NAME: &str = "part2";

        #[derive(Clone)]
        struct TransactionObserver(Arc<AtomicBool>);
        impl Observer for TransactionObserver {
            fn write(
                &self,
                _device_block_offset: u64,
                _block_count: u32,
                _vmo: &Arc<zx::Vmo>,
                _vmo_offset: u64,
                _opts: WriteOptions,
            ) -> WriteAction {
                if self.0.load(Ordering::Relaxed) { WriteAction::Fail } else { WriteAction::Write }
            }
        }
        let observer = TransactionObserver(Arc::new(AtomicBool::new(false)));
        let (block_device, partitions_dir) = setup_with_options(
            VmoBackedServerOptions {
                initial_contents: InitialContents::FromCapacity(16),
                block_size: 512,
                observer: Some(Box::new(observer.clone())),
                ..Default::default()
            },
            vec![
                PartitionInfo {
                    label: PART_1_NAME.to_string(),
                    type_guid: Guid::from_bytes(PART_TYPE_GUID),
                    instance_guid: Guid::from_bytes(PART_1_INSTANCE_GUID),
                    start_block: 4,
                    num_blocks: 1,
                    flags: 0,
                },
                PartitionInfo {
                    label: PART_2_NAME.to_string(),
                    type_guid: Guid::from_bytes(PART_TYPE_GUID),
                    instance_guid: Guid::from_bytes(PART_2_INSTANCE_GUID),
                    start_block: 5,
                    num_blocks: 1,
                    flags: 0,
                },
            ],
        )
        .await;
        let runner = GptManager::new(block_device.connect(), partitions_dir.clone())
            .await
            .expect("load should succeed");

        let part_0_dir = vfs::serve_directory(
            partitions_dir.clone(),
            vfs::Path::validate_and_split("part-000").unwrap(),
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE,
        );
        let part_1_dir = vfs::serve_directory(
            partitions_dir.clone(),
            vfs::Path::validate_and_split("part-001").unwrap(),
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE,
        );
        let part_0_proxy = connect_to_named_protocol_at_dir_root::<fpartitions::PartitionMarker>(
            &part_0_dir,
            "partition",
        )
        .expect("Failed to open Partition service");
        let part_1_proxy = connect_to_named_protocol_at_dir_root::<fpartitions::PartitionMarker>(
            &part_1_dir,
            "partition",
        )
        .expect("Failed to open Partition service");

        let transaction = runner.create_transaction().await.expect("Failed to create transaction");
        part_0_proxy
            .update_metadata(fpartitions::PartitionUpdateMetadataRequest {
                transaction: Some(transaction.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap()),
                type_guid: Some(fblock::Guid { value: [0xffu8; 16] }),
                ..Default::default()
            })
            .await
            .expect("FIDL error")
            .expect("Failed to update_metadata");
        part_1_proxy
            .update_metadata(fpartitions::PartitionUpdateMetadataRequest {
                transaction: Some(transaction.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap()),
                flags: Some(1234),
                ..Default::default()
            })
            .await
            .expect("FIDL error")
            .expect("Failed to update_metadata");

        observer.0.store(true, Ordering::Relaxed); // Fail the next write
        runner.commit_transaction(transaction).await.expect_err("Commit transaction should fail");

        // Ensure the changes did not get applied.
        let part_0_block =
            connect_to_named_protocol_at_dir_root::<fblock::BlockMarker>(&part_0_dir, "volume")
                .expect("Failed to open Volume service");
        let (status, guid) = part_0_block.get_type_guid().await.expect("FIDL error");
        assert_eq!(status, zx::sys::ZX_OK);
        assert_eq!(guid.unwrap().value, [2u8; 16]);
        let part_1_block =
            connect_to_named_protocol_at_dir_root::<fblock::BlockMarker>(&part_1_dir, "volume")
                .expect("Failed to open Volume service");
        let metadata =
            part_1_block.get_metadata().await.expect("FIDL error").expect("get_metadata failed");
        assert_eq!(metadata.type_guid.unwrap().value, PART_TYPE_GUID);
        assert_eq!(metadata.flags, Some(0));

        runner.shutdown().await;
    }

    #[fuchsia::test]
    async fn reset_partition_tables() {
        // The test will reset the tables from ["part", "part2"] to
        // ["part3", <empty>, "part4", <125 empty entries>].
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_1_INSTANCE_GUID: [u8; 16] = [2u8; 16];
        const PART_1_NAME: &str = "part";
        const PART_2_INSTANCE_GUID: [u8; 16] = [3u8; 16];
        const PART_2_NAME: &str = "part2";
        const PART_3_NAME: &str = "part3";
        const PART_4_NAME: &str = "part4";

        let (block_device, partitions_dir) = setup(
            512,
            1048576 / 512,
            vec![
                PartitionInfo {
                    label: PART_1_NAME.to_string(),
                    type_guid: Guid::from_bytes(PART_TYPE_GUID),
                    instance_guid: Guid::from_bytes(PART_1_INSTANCE_GUID),
                    start_block: 4,
                    num_blocks: 1,
                    flags: 0,
                },
                PartitionInfo {
                    label: PART_2_NAME.to_string(),
                    type_guid: Guid::from_bytes(PART_TYPE_GUID),
                    instance_guid: Guid::from_bytes(PART_2_INSTANCE_GUID),
                    start_block: 5,
                    num_blocks: 1,
                    flags: 0,
                },
            ],
        )
        .await;
        let runner = GptManager::new(block_device.connect(), partitions_dir.clone())
            .await
            .expect("load should succeed");
        let nil_entry = PartitionInfo {
            label: "".to_string(),
            type_guid: Guid::from_bytes([0u8; 16]),
            instance_guid: Guid::from_bytes([0u8; 16]),
            start_block: 0,
            num_blocks: 0,
            flags: 0,
        };
        let mut new_partitions = vec![nil_entry; 128];
        new_partitions[0] = PartitionInfo {
            label: PART_3_NAME.to_string(),
            type_guid: Guid::from_bytes(PART_TYPE_GUID),
            instance_guid: Guid::from_bytes([1u8; 16]),
            start_block: 64,
            num_blocks: 2,
            flags: 0,
        };
        new_partitions[2] = PartitionInfo {
            label: PART_4_NAME.to_string(),
            type_guid: Guid::from_bytes(PART_TYPE_GUID),
            instance_guid: Guid::from_bytes([2u8; 16]),
            start_block: 66,
            num_blocks: 4,
            flags: 0,
        };
        runner.reset_partition_table(new_partitions).await.expect("reset_partition_table failed");
        partitions_dir.get_entry("part-000").expect("No entry found");
        partitions_dir.get_entry("part-001").map(|_| ()).expect_err("Extra entry found");
        partitions_dir.get_entry("part-002").expect("No entry found");

        let proxy = vfs::serve_directory(
            partitions_dir.clone(),
            vfs::path::Path::validate_and_split("part-000").unwrap(),
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE,
        );
        let block = connect_to_named_protocol_at_dir_root::<fblock::BlockMarker>(&proxy, "volume")
            .expect("Failed to open block service");
        let (status, name) = block.get_name().await.expect("FIDL error");
        assert_eq!(status, zx::sys::ZX_OK);
        assert_eq!(name.unwrap(), PART_3_NAME);

        runner.shutdown().await;
    }

    #[fuchsia::test]
    async fn reset_partition_tables_fails_if_too_many_partitions() {
        let (block_device, partitions_dir) = setup(512, 8, vec![]).await;
        let runner = GptManager::new(block_device.connect(), partitions_dir.clone())
            .await
            .expect("load should succeed");
        let nil_entry = PartitionInfo {
            label: "".to_string(),
            type_guid: Guid::from_bytes([0u8; 16]),
            instance_guid: Guid::from_bytes([0u8; 16]),
            start_block: 0,
            num_blocks: 0,
            flags: 0,
        };
        let new_partitions = vec![nil_entry; 128];
        runner
            .reset_partition_table(new_partitions)
            .await
            .expect_err("reset_partition_table should fail");

        runner.shutdown().await;
    }

    #[fuchsia::test]
    async fn reset_partition_tables_fails_if_too_large_partitions() {
        let (block_device, partitions_dir) = setup(512, 64, vec![]).await;
        let runner = GptManager::new(block_device.connect(), partitions_dir.clone())
            .await
            .expect("load should succeed");
        let new_partitions = vec![
            PartitionInfo {
                label: "a".to_string(),
                type_guid: Guid::from_bytes([1u8; 16]),
                instance_guid: Guid::from_bytes([1u8; 16]),
                start_block: 4,
                num_blocks: 2,
                flags: 0,
            },
            PartitionInfo {
                label: "b".to_string(),
                type_guid: Guid::from_bytes([2u8; 16]),
                instance_guid: Guid::from_bytes([2u8; 16]),
                start_block: 6,
                num_blocks: 200,
                flags: 0,
            },
        ];
        runner
            .reset_partition_table(new_partitions)
            .await
            .expect_err("reset_partition_table should fail");

        runner.shutdown().await;
    }

    #[fuchsia::test]
    async fn reset_partition_tables_fails_if_partition_overlaps_metadata() {
        let (block_device, partitions_dir) = setup(512, 64, vec![]).await;
        let runner = GptManager::new(block_device.connect(), partitions_dir.clone())
            .await
            .expect("load should succeed");
        let new_partitions = vec![PartitionInfo {
            label: "a".to_string(),
            type_guid: Guid::from_bytes([1u8; 16]),
            instance_guid: Guid::from_bytes([1u8; 16]),
            start_block: 1,
            num_blocks: 2,
            flags: 0,
        }];
        runner
            .reset_partition_table(new_partitions)
            .await
            .expect_err("reset_partition_table should fail");

        runner.shutdown().await;
    }

    #[fuchsia::test]
    async fn reset_partition_tables_fails_if_partitions_overlap() {
        let (block_device, partitions_dir) = setup(512, 64, vec![]).await;
        let runner = GptManager::new(block_device.connect(), partitions_dir.clone())
            .await
            .expect("load should succeed");
        let new_partitions = vec![
            PartitionInfo {
                label: "a".to_string(),
                type_guid: Guid::from_bytes([1u8; 16]),
                instance_guid: Guid::from_bytes([1u8; 16]),
                start_block: 32,
                num_blocks: 2,
                flags: 0,
            },
            PartitionInfo {
                label: "b".to_string(),
                type_guid: Guid::from_bytes([2u8; 16]),
                instance_guid: Guid::from_bytes([2u8; 16]),
                start_block: 33,
                num_blocks: 1,
                flags: 0,
            },
        ];
        runner
            .reset_partition_table(new_partitions)
            .await
            .expect_err("reset_partition_table should fail");

        runner.shutdown().await;
    }

    #[fuchsia::test]
    async fn add_partition() {
        let (block_device, partitions_dir) = setup(512, 64, vec![PartitionInfo::nil(); 64]).await;
        let runner = GptManager::new(block_device.connect(), partitions_dir.clone())
            .await
            .expect("load should succeed");

        let transaction = runner.create_transaction().await.expect("Create transaction failed");
        let request = fpartitions::PartitionsManagerAddPartitionRequest {
            transaction: Some(transaction.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap()),
            name: Some("a".to_string()),
            type_guid: Some(fblock::Guid { value: [1u8; 16] }),
            num_blocks: Some(2),
            ..Default::default()
        };
        runner.add_partition(request).await.expect("add_partition failed");
        runner.commit_transaction(transaction).await.expect("add_partition failed");

        let proxy = vfs::serve_directory(
            partitions_dir.clone(),
            vfs::path::Path::validate_and_split("part-000").unwrap(),
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE,
        );
        let block = connect_to_named_protocol_at_dir_root::<fblock::BlockMarker>(&proxy, "volume")
            .expect("Failed to open block service");
        let client: RemoteBlockClient =
            RemoteBlockClient::new(block).await.expect("Failed to create block client");

        assert_eq!(client.block_count(), 2);
        assert_eq!(client.block_size(), 512);

        runner.shutdown().await;
    }

    #[fuchsia::test]
    async fn partition_info() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_GUID: [u8; 16] = [2u8; 16];
        const PART_NAME: &str = "part";

        let (block_device, partitions_dir) = setup_with_options(
            VmoBackedServerOptions {
                initial_contents: InitialContents::FromCapacity(16),
                block_size: 512,
                info: DeviceInfo::Block(BlockInfo {
                    max_transfer_blocks: NonZero::new(2),
                    device_flags: BlockDeviceFlag::READONLY
                        | BlockDeviceFlag::REMOVABLE
                        | BlockDeviceFlag::ZSTD_DECOMPRESSION_SUPPORT,
                    ..Default::default()
                }),
                ..Default::default()
            },
            vec![PartitionInfo {
                label: PART_NAME.to_string(),
                type_guid: Guid::from_bytes(PART_TYPE_GUID),
                instance_guid: Guid::from_bytes(PART_INSTANCE_GUID),
                start_block: 4,
                num_blocks: 1,
                flags: 0xabcd,
            }],
        )
        .await;

        let partitions_dir_clone = partitions_dir.clone();
        let runner = GptManager::new(block_device.connect(), partitions_dir_clone)
            .await
            .expect("load should succeed");

        let part_dir = vfs::serve_directory(
            partitions_dir.clone(),
            vfs::path::Path::validate_and_split("part-000").unwrap(),
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE,
        );
        let part_block =
            connect_to_named_protocol_at_dir_root::<fblock::BlockMarker>(&part_dir, "volume")
                .expect("Failed to open Volume service");
        let info: fblock::BlockInfo =
            part_block.get_info().await.expect("FIDL error").expect("get_info failed");
        assert_eq!(info.block_count, 1);
        assert_eq!(info.block_size, 512);
        assert_eq!(
            info.flags,
            BlockDeviceFlag::READONLY
                | BlockDeviceFlag::REMOVABLE
                | BlockDeviceFlag::ZSTD_DECOMPRESSION_SUPPORT
        );
        assert_eq!(info.max_transfer_size, 1024);

        let metadata: fblock::PartitionInfo =
            part_block.get_metadata().await.expect("FIDL error").expect("get_metadata failed");
        assert_eq!(metadata.name, Some(PART_NAME.to_string()));
        assert_eq!(metadata.type_guid.unwrap().value, PART_TYPE_GUID);
        assert_eq!(metadata.instance_guid.unwrap().value, PART_INSTANCE_GUID);
        assert_eq!(metadata.start_block_offset, Some(4));
        assert_eq!(metadata.num_blocks, Some(1));
        assert_eq!(metadata.flags, Some(0xabcd));

        runner.shutdown().await;
    }

    #[fuchsia::test]
    async fn nested_gpt() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_GUID: [u8; 16] = [2u8; 16];
        const PART_NAME: &str = "part";

        let vmo = zx::Vmo::create(64 * 512).unwrap();
        let vmo_clone = vmo.create_child(zx::VmoChildOptions::REFERENCE, 0, 0).unwrap();
        let (outer_block_device, outer_partitions_dir) = setup_with_options(
            VmoBackedServerOptions {
                initial_contents: InitialContents::FromVmo(vmo_clone),
                block_size: 512,
                info: DeviceInfo::Block(BlockInfo {
                    device_flags: BlockDeviceFlag::READONLY | BlockDeviceFlag::REMOVABLE,
                    ..Default::default()
                }),
                ..Default::default()
            },
            vec![PartitionInfo {
                label: PART_NAME.to_string(),
                type_guid: Guid::from_bytes(PART_TYPE_GUID),
                instance_guid: Guid::from_bytes(PART_INSTANCE_GUID),
                start_block: 4,
                num_blocks: 16,
                flags: 0xabcd,
            }],
        )
        .await;

        let outer_partitions_dir_clone = outer_partitions_dir.clone();
        let outer_runner =
            GptManager::new(outer_block_device.connect(), outer_partitions_dir_clone)
                .await
                .expect("load should succeed");

        let outer_part_dir = vfs::serve_directory(
            outer_partitions_dir.clone(),
            vfs::path::Path::validate_and_split("part-000").unwrap(),
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE,
        );
        let part_block =
            connect_to_named_protocol_at_dir_root::<fblock::BlockMarker>(&outer_part_dir, "volume")
                .expect("Failed to open Block service");

        let client = Arc::new(RemoteBlockClient::new(part_block.clone()).await.unwrap());
        let _ = gpt::Gpt::format(
            client,
            vec![PartitionInfo {
                label: PART_NAME.to_string(),
                type_guid: Guid::from_bytes(PART_TYPE_GUID),
                instance_guid: Guid::from_bytes(PART_INSTANCE_GUID),
                start_block: 5,
                num_blocks: 1,
                flags: 0xabcd,
            }],
        )
        .await
        .unwrap();

        let partitions_dir = vfs::directory::immutable::simple();
        let partitions_dir_clone = partitions_dir.clone();
        let runner =
            GptManager::new(part_block, partitions_dir_clone).await.expect("load should succeed");
        let part_dir = vfs::serve_directory(
            partitions_dir.clone(),
            vfs::path::Path::validate_and_split("part-000").unwrap(),
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE,
        );
        let inner_part_block =
            connect_to_named_protocol_at_dir_root::<fblock::BlockMarker>(&part_dir, "volume")
                .expect("Failed to open Block service");

        let client =
            RemoteBlockClient::new(inner_part_block).await.expect("Failed to create block client");
        assert_eq!(client.block_count(), 1);
        assert_eq!(client.block_size(), 512);

        let buffer = vec![0xaa; 512];
        client.write_at(BufferSlice::Memory(&buffer), 0).await.unwrap();
        client
            .write_at(BufferSlice::Memory(&buffer), 512)
            .await
            .expect_err("Write past end should fail");
        client.flush().await.unwrap();

        runner.shutdown().await;
        outer_runner.shutdown().await;

        // Check that the write targeted the correct block (4 + 5 = 9)
        let data = vmo.read_to_vec::<u8>(9 * 512, 512).unwrap();
        assert_eq!(&data[..], &buffer[..]);
    }

    #[fuchsia::test]
    async fn open_session_with_options_is_rejected() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_GUID: [u8; 16] = [2u8; 16];
        const PART_NAME: &str = "foo";

        let (block_device, partitions_dir) = setup_with_options(
            VmoBackedServerOptions {
                initial_contents: InitialContents::FromCapacity(16),
                block_size: 512,
                ..Default::default()
            },
            vec![PartitionInfo {
                label: PART_NAME.to_string(),
                type_guid: Guid::from_bytes(PART_TYPE_GUID),
                instance_guid: Guid::from_bytes(PART_INSTANCE_GUID),
                start_block: 4,
                num_blocks: 2,
                flags: 0xabcd,
            }],
        )
        .await;

        let partitions_dir_clone = partitions_dir.clone();
        let runner = GptManager::new(block_device.connect(), partitions_dir_clone)
            .await
            .expect("load should succeed");

        let part_dir = vfs::serve_directory(
            partitions_dir.clone(),
            vfs::path::Path::validate_and_split("part-000").unwrap(),
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE,
        );

        let part_block =
            connect_to_named_protocol_at_dir_root::<fblock::BlockMarker>(&part_dir, "volume")
                .expect("Failed to open Block service");

        // Attempting to open a session with a valid offset map should fail.
        let (session, server_end) = fidl::endpoints::create_proxy::<fblock::SessionMarker>();
        part_block
            .open_session_with_options(
                server_end,
                &[fblock::BlockOffsetMapping { target_block_offset: 1, length: 2 }],
            )
            .expect("FIDL error");
        session
            .get_fifo()
            .await
            .expect_err("Session should be closed because nested mappings are not supported");

        runner.shutdown().await;
    }

    #[fuchsia::test]
    async fn test_open_session_with_options_rejects_nested_mappings() {
        let (block_device, partitions_dir) = setup(
            512,
            2048,
            vec![
                PartitionInfo {
                    label: "super".to_string(),
                    type_guid: Guid::from_bytes([1; 16]),
                    instance_guid: Guid::from_bytes([2; 16]),
                    start_block: 34,
                    num_blocks: 10,
                    flags: 0,
                },
                PartitionInfo {
                    label: "userdata".to_string(),
                    type_guid: Guid::from_bytes([1; 16]),
                    instance_guid: Guid::from_bytes([3; 16]),
                    start_block: 50,
                    num_blocks: 10,
                    flags: 0,
                },
            ],
        )
        .await;

        let partitions_dir_clone = partitions_dir.clone();
        let runner = GptManager::new_with_config(
            block_device.connect(),
            partitions_dir_clone,
            crate::config::Config { merge_super_and_userdata: true, ..Default::default() },
        )
        .await
        .expect("load should succeed");

        let part_dir = vfs::serve_directory(
            partitions_dir.clone(),
            vfs::path::Path::validate_and_split("part-000").unwrap(),
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE,
        );

        let part_block =
            connect_to_named_protocol_at_dir_root::<fblock::BlockMarker>(&part_dir, "volume")
                .expect("Failed to open Block service");

        let metadata: fblock::PartitionInfo =
            part_block.get_metadata().await.expect("FIDL error").expect("get_metadata failed");
        assert_eq!(metadata.name, Some("super_and_userdata".to_string()));
        assert!(metadata.start_block_offset.is_none());
        assert!(metadata.flags.is_none());

        // Attempting to open a session with an offset map on a merged GPT partition should fail
        // because it has static mappings, and nested mappings are not supported.
        let (session, server_end) = fidl::endpoints::create_proxy::<fblock::SessionMarker>();
        part_block
            .open_session_with_options(
                server_end,
                &[fblock::BlockOffsetMapping { target_block_offset: 0, length: 3 }],
            )
            .expect("FIDL error");
        session.get_fifo().await.expect_err("Session should be closed due to nested mapping");

        {
            let inner = runner.inner.lock().await;
            let backend = inner.composite_partitions.get(&0).unwrap().session_manager().interface();
            assert!(backend.passthrough());
        }

        runner.shutdown().await;
    }

    #[fuchsia::test]
    async fn test_vmos_detached_on_session_close() {
        let (block_device, partitions_dir) = setup(
            512,
            100,
            vec![PartitionInfo {
                type_guid: Guid::from_bytes([2u8; 16]),
                instance_guid: Guid::from_bytes([2u8; 16]),
                start_block: 34,
                num_blocks: 10,
                flags: 0,
                label: "test".to_string(),
            }],
        )
        .await;

        let runner = GptManager::new(block_device.connect(), partitions_dir.clone()).await.unwrap();
        let proxy = vfs::serve_directory(
            partitions_dir.clone(),
            vfs::path::Path::validate_and_split("part-000").unwrap(),
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE,
        );
        let block = connect_to_named_protocol_at_dir_root::<fblock::BlockMarker>(&proxy, "volume")
            .expect("Failed to open block service");
        let client = RemoteBlockClient::new(block).await.expect("Failed to create block client");

        {
            let inner = runner.inner.lock().await;
            let backend = inner.partitions.get(&0).unwrap().session_manager().interface();
            assert_eq!(backend.vmo_count(), 1);
        }

        client.close().await.expect("Failed to close client");

        {
            let inner = runner.inner.lock().await;
            let backend = inner.partitions.get(&0).unwrap().session_manager().interface();
            assert_eq!(backend.vmo_count(), 0);
        }

        runner.shutdown().await;
    }

    #[test]
    fn test_should_passthrough_partition() {
        use super::{ALL_SYSTEM_PARTITION_LABELS, should_passthrough_partition};

        let system_label = ALL_SYSTEM_PARTITION_LABELS[0].to_string();

        // Single mapping on a system label partition -> should passthrough.
        let single_config = block_server::PartitionInfo {
            name: system_label.clone(),
            type_guid: [1; 16],
            instance_guid: [2; 16],
            flags: Some(0),
            start_block_offset: Some(0),
            block_count: 100,
            ..Default::default()
        };
        assert!(should_passthrough_partition(&single_config));

        // Multiple mappings on the exact same system label partition -> should passthrough.
        let multi_config = block_server::PartitionInfo {
            name: system_label,
            type_guid: [1; 16],
            instance_guid: [2; 16],
            flags: Some(0),
            start_block_offset: Some(0),
            block_count: 200,
            ..Default::default()
        };
        assert!(should_passthrough_partition(&multi_config));
    }

    #[fuchsia::test]
    async fn test_merged_partition_passthrough_behavior() {
        // Test Case 1: Discontiguous -> passthrough = false
        {
            let (block_device, partitions_dir) = setup(
                512,
                2048,
                vec![
                    PartitionInfo {
                        label: "super".to_string(),
                        type_guid: Guid::from_bytes([1; 16]),
                        instance_guid: Guid::from_bytes([2; 16]),
                        start_block: 34,
                        num_blocks: 10,
                        flags: 0,
                    },
                    PartitionInfo {
                        label: "userdata".to_string(),
                        type_guid: Guid::from_bytes([1; 16]),
                        instance_guid: Guid::from_bytes([3; 16]),
                        start_block: 50, // Discontiguous (44 != 50)
                        num_blocks: 10,
                        flags: 0,
                    },
                ],
            )
            .await;

            let runner = GptManager::new_with_config(
                block_device.connect(),
                partitions_dir,
                crate::config::Config { merge_super_and_userdata: true, ..Default::default() },
            )
            .await
            .expect("load should succeed");

            {
                let inner = runner.inner.lock().await;
                let backend =
                    inner.composite_partitions.get(&0).unwrap().session_manager().interface();
                assert!(backend.passthrough());
            }
            runner.shutdown().await;
        }

        // Test Case 2: Contiguous -> passthrough = true (after coalescing it will be 1 mapping)
        {
            let (block_device, partitions_dir) = setup(
                512,
                2048,
                vec![
                    PartitionInfo {
                        label: "super".to_string(),
                        type_guid: Guid::from_bytes([1; 16]),
                        instance_guid: Guid::from_bytes([2; 16]),
                        start_block: 34,
                        num_blocks: 10,
                        flags: 0,
                    },
                    PartitionInfo {
                        label: "userdata".to_string(),
                        type_guid: Guid::from_bytes([1; 16]),
                        instance_guid: Guid::from_bytes([3; 16]),
                        start_block: 44, // Contiguous (34 + 10 = 44)
                        num_blocks: 10,
                        flags: 0,
                    },
                ],
            )
            .await;

            let runner = GptManager::new_with_config(
                block_device.connect(),
                partitions_dir,
                crate::config::Config { merge_super_and_userdata: true, ..Default::default() },
            )
            .await
            .expect("load should succeed");

            {
                let inner = runner.inner.lock().await;
                let backend =
                    inner.composite_partitions.get(&0).unwrap().session_manager().interface();
                assert!(backend.passthrough());
            }
            runner.shutdown().await;
        }
    }

    #[fuchsia::test]
    async fn reset_partition_table_severs_existing_connections() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_1_INSTANCE_GUID: [u8; 16] = [2u8; 16];
        const PART_1_NAME: &str = "part";

        let (block_device, partitions_dir) = setup(
            512,
            1048576 / 512,
            vec![PartitionInfo {
                label: PART_1_NAME.to_string(),
                type_guid: Guid::from_bytes(PART_TYPE_GUID),
                instance_guid: Guid::from_bytes(PART_1_INSTANCE_GUID),
                start_block: 4,
                num_blocks: 10,
                flags: 0,
            }],
        )
        .await;

        let runner = GptManager::new(block_device.connect(), partitions_dir.clone())
            .await
            .expect("load should succeed");

        let part_0_dir = vfs::serve_directory(
            partitions_dir.clone(),
            vfs::path::Path::validate_and_split("part-000").unwrap(),
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE,
        );

        let part_0_block =
            connect_to_named_protocol_at_dir_root::<fblock::BlockMarker>(&part_0_dir, "volume")
                .expect("Failed to open Volume service");
        let part_0_partition =
            connect_to_named_protocol_at_dir_root::<fpartitions::PartitionMarker>(
                &part_0_dir,
                "partition",
            )
            .expect("Failed to open Partition service");

        let (part_0_block_clone, server_end) =
            fidl::endpoints::create_proxy::<fblock::BlockMarker>();
        part_0_dir
            .open(
                "volume",
                fio::Flags::PROTOCOL_SERVICE,
                &fio::Options::default(),
                server_end.into_channel(),
            )
            .expect("Failed to open volume");

        let client =
            RemoteBlockClient::new(part_0_block).await.expect("Failed to create block client");

        let buf = vec![0xabu8; 512];
        client.write_at(BufferSlice::Memory(&buf[..]), 0).await.expect("write_at failed");

        let nil_entry = PartitionInfo {
            label: "".to_string(),
            type_guid: Guid::from_bytes([0u8; 16]),
            instance_guid: Guid::from_bytes([0u8; 16]),
            start_block: 0,
            num_blocks: 0,
            flags: 0,
        };
        let mut new_partitions = vec![nil_entry; 128];
        new_partitions[0] = PartitionInfo {
            label: "part_new".to_string(),
            type_guid: Guid::from_bytes(PART_TYPE_GUID),
            instance_guid: Guid::from_bytes([1u8; 16]),
            start_block: 64,
            num_blocks: 2,
            flags: 0,
        };

        runner.reset_partition_table(new_partitions).await.expect("reset_partition_table failed");

        // The old partition connection should be severed.
        let transaction = runner.create_transaction().await.expect("Failed to create transaction");
        part_0_partition
            .update_metadata(fpartitions::PartitionUpdateMetadataRequest {
                transaction: Some(transaction),
                flags: Some(1234),
                ..Default::default()
            })
            .await
            .expect_err(
                "update_metadata on stale partition connection should fail with PEER_CLOSED",
            );

        // The old block connection should be severed (get_name should fail with PEER_CLOSED).
        part_0_block_clone
            .get_name()
            .await
            .expect_err("get_name on stale block connection should fail");

        // Subsequent writes on the old client should fail because the session/connection was
        // severed.
        client
            .write_at(BufferSlice::Memory(&buf[..]), 0)
            .await
            .expect_err("write_at on stale client should fail");

        runner.shutdown().await;
    }

    #[fuchsia::test(threads = 2)]
    async fn reset_partition_table_with_in_flight_io_succeeds() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_1_INSTANCE_GUID: [u8; 16] = [2u8; 16];
        const PART_1_NAME: &str = "part";

        struct PauseObserver {
            started_tx: std::sync::mpsc::Sender<()>,
            resume_rx: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
        }

        impl Observer for PauseObserver {
            fn read(
                &self,
                device_block_offset: u64,
                _block_count: u32,
                _vmo: &Arc<zx::Vmo>,
                _vmo_offset: u64,
            ) {
                // Only pause partition reads (LBA 4 is the start of the partition).
                if device_block_offset >= 4 {
                    let _ = self.started_tx.send(());
                    let _ = self.resume_rx.lock().unwrap().recv();
                }
            }
        }

        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();

        let (block_device, partitions_dir) = setup_with_options(
            VmoBackedServerOptions {
                initial_contents: InitialContents::FromCapacity(1048576 / 512),
                block_size: 512,
                observer: Some(Box::new(PauseObserver {
                    started_tx,
                    resume_rx: std::sync::Mutex::new(resume_rx),
                })),
                ..Default::default()
            },
            vec![PartitionInfo {
                label: PART_1_NAME.to_string(),
                type_guid: Guid::from_bytes(PART_TYPE_GUID),
                instance_guid: Guid::from_bytes(PART_1_INSTANCE_GUID),
                start_block: 4,
                num_blocks: 10,
                flags: 0,
            }],
        )
        .await;

        let runner = GptManager::new(block_device.connect(), partitions_dir.clone())
            .await
            .expect("load should succeed");

        let part_0_dir = vfs::serve_directory(
            partitions_dir.clone(),
            vfs::path::Path::validate_and_split("part-000").unwrap(),
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE,
        );

        let part_0_block =
            connect_to_named_protocol_at_dir_root::<fblock::BlockMarker>(&part_0_dir, "volume")
                .expect("Failed to open Volume service");

        let client =
            RemoteBlockClient::new(part_0_block).await.expect("Failed to create block client");

        // Spawn a background read task that will pause in PauseObserver at the disk level.
        let mut buf = vec![0u8; 512];
        let read_task = fasync::Task::spawn(async move {
            client.read_at(MutableBufferSlice::Memory(&mut buf[..]), 0).await
        });

        // Deterministically wait for the read to arrive at the underlying disk.
        started_rx.recv().expect("Failed to receive read start notification");

        let nil_entry = PartitionInfo {
            label: "".to_string(),
            type_guid: Guid::from_bytes([0u8; 16]),
            instance_guid: Guid::from_bytes([0u8; 16]),
            start_block: 0,
            num_blocks: 0,
            flags: 0,
        };
        let mut new_partitions = vec![nil_entry; 128];
        new_partitions[0] = PartitionInfo {
            label: "part_new".to_string(),
            type_guid: Guid::from_bytes(PART_TYPE_GUID),
            instance_guid: Guid::from_bytes([1u8; 16]),
            start_block: 64,
            num_blocks: 2,
            flags: 0,
        };

        // Reset partition table while the read is guaranteed to be in-flight at the disk level.
        // Once reset_partition_table severs the partition connections, resume the observer so
        // the mock disk unblocks.
        let reset_fut = runner.reset_partition_table(new_partitions);
        let resume_task = fasync::Task::spawn(async move {
            fasync::Timer::new(std::time::Duration::from_millis(50)).await;
            let _ = resume_tx.send(());
        });

        reset_fut.await.expect("reset_partition_table failed");
        resume_task.await;

        // The in-flight read should fail because its connection was severed by table reset.
        read_task.await.expect_err("in-flight read should fail");

        runner.shutdown().await;
    }

    #[fuchsia::test]
    async fn test_mapper_passthrough_on_partition() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_GUID: [u8; 16] = [2u8; 16];
        const PART_NAME: &str = "super";

        let (block_device, partitions_dir) = setup(
            512,
            64,
            vec![PartitionInfo {
                label: PART_NAME.to_string(),
                type_guid: Guid::from_bytes(PART_TYPE_GUID),
                instance_guid: Guid::from_bytes(PART_INSTANCE_GUID),
                start_block: 8,
                num_blocks: 16,
                flags: 0,
            }],
        )
        .await;

        let mapper_proxy = block_device.connect_mapper();
        let partitions_dir_clone = partitions_dir.clone();
        let runner = GptManager::new_with_mapper(
            block_device.connect(),
            Some(mapper_proxy),
            partitions_dir_clone,
        )
        .await
        .expect("load should succeed");

        let part_dir = vfs::serve_directory(
            partitions_dir.clone(),
            vfs::path::Path::validate_and_split("part-000").unwrap(),
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE,
        );
        let part_mapper =
            connect_to_named_protocol_at_dir_root::<fblock::MapperMarker>(&part_dir, "mapper")
                .expect("Failed to open Mapper service");

        let (_session_proxy, session_server_end) =
            fidl::endpoints::create_proxy::<fblock::MapperSessionMarker>();
        let mapping_vmo = zx::Vmo::create(mapping::MAPPING_VMO_SIZE).unwrap();
        let port = zx::Port::create();
        let delivery_queue = zx::Vmo::create(mapping::DELIVERY_VMO_SIZE).unwrap();

        part_mapper
            .open_session(session_server_end, mapping_vmo, Some(port), Some(delivery_queue))
            .await
            .expect("FIDL open_session failed")
            .expect("open_session returned error");

        runner.shutdown().await;
    }

    #[fuchsia::test]
    async fn test_mapper_not_supported_without_parent_mapper() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_GUID: [u8; 16] = [2u8; 16];
        const PART_NAME: &str = "super";

        let (block_device, partitions_dir) = setup(
            512,
            64,
            vec![PartitionInfo {
                label: PART_NAME.to_string(),
                type_guid: Guid::from_bytes(PART_TYPE_GUID),
                instance_guid: Guid::from_bytes(PART_INSTANCE_GUID),
                start_block: 8,
                num_blocks: 16,
                flags: 0,
            }],
        )
        .await;

        let (block_proxy, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<fblock::BlockMarker>();
        let underlying: fblock::BlockProxy = block_device.connect();
        let _block_task = fasync::Task::spawn(async move {
            while let Some(Ok(request)) = stream.next().await {
                match request {
                    fblock::BlockRequest::ConnectMapper { responder, .. } => {
                        let _ = responder.send(Err(zx::Status::NOT_SUPPORTED.into_raw()));
                    }
                    fblock::BlockRequest::GetInfo { responder } => {
                        let _ = responder
                            .send(underlying.get_info().await.unwrap().as_ref().map_err(|s| *s));
                    }
                    fblock::BlockRequest::OpenSession { session, .. } => {
                        let _ = underlying.open_session(session);
                    }
                    _ => unimplemented!(),
                }
            }
        });

        let runner = GptManager::new(block_proxy, partitions_dir.clone())
            .await
            .expect("load should succeed");

        let part_dir = vfs::serve_directory(
            partitions_dir.clone(),
            vfs::path::Path::validate_and_split("part-000").unwrap(),
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE,
        );
        let part_block =
            connect_to_named_protocol_at_dir_root::<fblock::BlockMarker>(&part_dir, "volume")
                .expect("Failed to open Block service");
        let (part_mapper, mapper_server_end) =
            fidl::endpoints::create_proxy::<fblock::MapperMarker>();
        part_block
            .connect_mapper(mapper_server_end)
            .await
            .expect("FIDL connect_mapper failed")
            .expect("connect_mapper returned error");

        let (_session_proxy, session_server_end) =
            fidl::endpoints::create_proxy::<fblock::MapperSessionMarker>();
        let mapping_vmo = zx::Vmo::create(mapping::MAPPING_VMO_SIZE).unwrap();
        let port = zx::Port::create();
        let delivery_queue = zx::Vmo::create(mapping::DELIVERY_VMO_SIZE).unwrap();

        assert_eq!(
            part_mapper
                .open_session(session_server_end, mapping_vmo, Some(port), Some(delivery_queue))
                .await
                .expect("FIDL open_session failed"),
            Err(zx::Status::NOT_SUPPORTED.into_raw())
        );

        runner.shutdown().await;
    }

    #[fuchsia::test]
    async fn reset_partition_table_severs_passthrough_connections() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_1_INSTANCE_GUID: [u8; 16] = [2u8; 16];
        const PART_1_NAME: &str = "fvm";

        let (block_device, partitions_dir) = setup(
            512,
            1048576 / 512,
            vec![PartitionInfo {
                label: PART_1_NAME.to_string(),
                type_guid: Guid::from_bytes(PART_TYPE_GUID),
                instance_guid: Guid::from_bytes(PART_1_INSTANCE_GUID),
                start_block: 4,
                num_blocks: 10,
                flags: 0,
            }],
        )
        .await;

        let runner = GptManager::new(block_device.connect(), partitions_dir.clone())
            .await
            .expect("load should succeed");

        let part_0_dir = vfs::serve_directory(
            partitions_dir.clone(),
            vfs::path::Path::validate_and_split("part-000").unwrap(),
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE,
        );

        let part_0_block =
            connect_to_named_protocol_at_dir_root::<fblock::BlockMarker>(&part_0_dir, "volume")
                .expect("Failed to open Volume service");

        let (part_0_block_clone, server_end) =
            fidl::endpoints::create_proxy::<fblock::BlockMarker>();
        part_0_dir
            .open(
                "volume",
                fio::Flags::PROTOCOL_SERVICE,
                &fio::Options::default(),
                server_end.into_channel(),
            )
            .expect("Failed to open volume");

        let client =
            RemoteBlockClient::new(part_0_block).await.expect("Failed to create block client");

        let buf = vec![0xabu8; 512];
        client.write_at(BufferSlice::Memory(&buf[..]), 0).await.expect("write_at failed");

        let nil_entry = PartitionInfo {
            label: "".to_string(),
            type_guid: Guid::from_bytes([0u8; 16]),
            instance_guid: Guid::from_bytes([0u8; 16]),
            start_block: 0,
            num_blocks: 0,
            flags: 0,
        };
        let mut new_partitions = vec![nil_entry; 128];
        new_partitions[0] = PartitionInfo {
            label: "part_new".to_string(),
            type_guid: Guid::from_bytes(PART_TYPE_GUID),
            instance_guid: Guid::from_bytes([1u8; 16]),
            start_block: 64,
            num_blocks: 2,
            flags: 0,
        };

        runner.reset_partition_table(new_partitions).await.expect("reset_partition_table failed");

        // The old block connection should be severed (get_name should fail with PEER_CLOSED).
        part_0_block_clone
            .get_name()
            .await
            .expect_err("get_name on stale block connection should fail");

        // Subsequent writes on the old client should fail because the session/connection was
        // severed.
        client
            .write_at(BufferSlice::Memory(&buf[..]), 0)
            .await
            .expect_err("write_at on stale client should fail");

        runner.shutdown().await;
    }

    #[fuchsia::test]
    async fn test_mapper_passthrough_on_composite_partition() {
        let (block_device, partitions_dir) = setup(
            512,
            64,
            vec![
                PartitionInfo {
                    label: "super".to_string(),
                    type_guid: Guid::from_bytes([1u8; 16]),
                    instance_guid: Guid::from_bytes([2u8; 16]),
                    start_block: 8,
                    num_blocks: 8,
                    flags: 0,
                },
                PartitionInfo {
                    label: "userdata".to_string(),
                    type_guid: Guid::from_bytes([1u8; 16]),
                    instance_guid: Guid::from_bytes([3u8; 16]),
                    start_block: 16,
                    num_blocks: 8,
                    flags: 0,
                },
            ],
        )
        .await;

        let mapper_proxy = block_device.connect_mapper();
        let partitions_dir_clone = partitions_dir.clone();
        let config = crate::config::Config { merge_super_and_userdata: true, ..Default::default() };
        let runner = GptManager::new_with_config_and_mapper(
            block_device.connect(),
            Some(mapper_proxy),
            partitions_dir_clone,
            config,
        )
        .await
        .expect("load should succeed");

        let part_dir = vfs::serve_directory(
            partitions_dir.clone(),
            vfs::path::Path::validate_and_split("part-000").unwrap(),
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE,
        );
        let part_mapper =
            connect_to_named_protocol_at_dir_root::<fblock::MapperMarker>(&part_dir, "mapper")
                .expect("Failed to open Mapper service");

        let (_session_proxy, session_server_end) =
            fidl::endpoints::create_proxy::<fblock::MapperSessionMarker>();
        let mapping_vmo = zx::Vmo::create(mapping::MAPPING_VMO_SIZE).unwrap();
        let port = zx::Port::create();
        let delivery_queue = zx::Vmo::create(mapping::DELIVERY_VMO_SIZE).unwrap();

        part_mapper
            .open_session(session_server_end, mapping_vmo, Some(port), Some(delivery_queue))
            .await
            .expect("FIDL open_session failed")
            .expect("open_session returned error");

        runner.shutdown().await;
    }

    #[fuchsia::test]
    async fn test_register_mappings_payload_preserved() {
        let (block_device, partitions_dir) = setup(512, 64, vec![]).await;

        let (mapper_proxy, mut mapper_stream) =
            fidl::endpoints::create_proxy_and_stream::<fblock::MapperMarker>();

        let (commands_tx, mut commands_rx) = futures::channel::mpsc::unbounded();
        let _mapper_task = fasync::Task::spawn(async move {
            if let Some(Ok(fblock::MapperRequest::OpenSession { mapping_vmo, responder, .. })) =
                mapper_stream.next().await
            {
                let vmo_dup = mapping_vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
                let _receiver_thread = std::thread::spawn(move || {
                    let mut receiver = vmo_fifo::Receiver::<mapping::RawMappingCommand>::new(
                        vmo_dup,
                        mapping::PENDING_COMMANDS_CAPACITY,
                    )
                    .unwrap();
                    while let Ok(msg) = receiver.peek() {
                        let cmd = *msg;
                        let payload_len = cmd.extent_count as u32 * 8;
                        let payload_slice = msg.payload_slice(cmd.offset, payload_len);
                        let mut payload = vec![0u8; payload_len as usize];
                        payload_slice.copy_to_slice(&mut payload);
                        commands_tx.unbounded_send((cmd, payload)).unwrap();
                        let _ = msg.pop();
                    }
                });
                responder.send(Ok(())).unwrap();
            }
        });

        let runner =
            GptManager::new_with_mapper(block_device.connect(), Some(mapper_proxy), partitions_dir)
                .await
                .expect("load should succeed");

        let offset_map1 = block_server::OffsetMap::new(vec![block_server::BlockOffsetMapping {
            target_block_offset: 0,
            length: 8,
        }])
        .unwrap();
        let offset_map2 = block_server::OffsetMap::new(vec![block_server::BlockOffsetMapping {
            target_block_offset: 8,
            length: 8,
        }])
        .unwrap();
        runner.register_mappings(1, &offset_map1).await.expect("register 1 failed");
        runner.register_mappings(2, &offset_map2).await.expect("register 2 failed");

        let (cmd1, payload1) = commands_rx.next().await.expect("expected first command");
        let (cmd2, payload2) = commands_rx.next().await.expect("expected second command");

        assert_eq!(cmd1.key, 1);
        assert_eq!(cmd2.key, 2);

        let (expected_payload1, _, _, expected_device_offset1) =
            super::offset_map_to_extents(&offset_map1, runner.block_size()).unwrap();
        let (expected_payload2, _, _, expected_device_offset2) =
            super::offset_map_to_extents(&offset_map2, runner.block_size()).unwrap();
        assert_eq!(cmd1.device_offset, expected_device_offset1);
        assert_eq!(cmd2.device_offset, expected_device_offset2);
        assert_eq!(payload1, expected_payload1);
        assert_eq!(payload2, expected_payload2);

        runner.shutdown().await;
    }

    #[fuchsia::test]
    async fn test_offset_map_to_extents_unaligned_length() {
        let offset_map = OffsetMap::new(vec![block_server::BlockOffsetMapping {
            target_block_offset: 34,
            length: 114654,
        }])
        .unwrap();

        let (payload, logical_len, count, base_offset) =
            super::offset_map_to_extents(&offset_map, 512).unwrap();

        assert_eq!(base_offset, 34 * 512);
        // 114654 * 512 = 58702848 bytes, rounded down to nearest 4KB is 58699776 bytes.
        assert_eq!(logical_len, 58699776);
        assert_eq!(count, 1);
        assert_eq!(payload.len(), 8);
    }

    #[fuchsia::test]
    async fn test_offset_map_to_extents_multiple_mappings() {
        // First mapping is 4 KiB aligned (8 blocks of 512 = 4096 bytes).
        // Second mapping is unaligned (11 blocks of 512 = 5632 bytes -> rounded down to 4096
        // bytes).
        let offset_map = OffsetMap::new(vec![
            block_server::BlockOffsetMapping { target_block_offset: 34, length: 8 },
            block_server::BlockOffsetMapping { target_block_offset: 50, length: 11 },
        ])
        .unwrap();

        let (payload, logical_len, count, base_offset) =
            super::offset_map_to_extents(&offset_map, 512).unwrap();

        assert_eq!(base_offset, 34 * 512);
        assert_eq!(logical_len, 8192);
        assert_eq!(count, 2);
        assert_eq!(payload.len(), 16);
    }

    #[fuchsia::test]
    async fn test_offset_map_to_extents_earlier_mapping_unaligned_fails() {
        // First mapping is not 4 KiB aligned (7 blocks of 512 = 3584 bytes).
        // Second mapping is 8 blocks (4096 bytes).
        let offset_map = OffsetMap::new(vec![
            block_server::BlockOffsetMapping { target_block_offset: 34, length: 7 },
            block_server::BlockOffsetMapping { target_block_offset: 50, length: 8 },
        ])
        .unwrap();

        let err = super::offset_map_to_extents(&offset_map, 512).unwrap_err();
        assert_eq!(err.root_cause().downcast_ref::<zx::Status>(), Some(&zx::Status::NOT_SUPPORTED));
    }

    #[fuchsia::test]
    async fn test_offset_map_to_extents_lowest_physical_block_base_offset() {
        // First logical mapping is at a higher physical block (50 * 512).
        // Second logical mapping is at a lower physical block (34 * 512).
        let offset_map = OffsetMap::new(vec![
            block_server::BlockOffsetMapping { target_block_offset: 50, length: 8 },
            block_server::BlockOffsetMapping { target_block_offset: 34, length: 8 },
        ])
        .unwrap();

        let (payload, logical_len, count, base_offset) =
            super::offset_map_to_extents(&offset_map, 512).unwrap();

        assert_eq!(base_offset, 34 * 512);
        assert_eq!(logical_len, 8192);
        assert_eq!(count, 2);
        assert_eq!(payload.len(), 16);
    }
}
